use anyhow::{anyhow, Result};
use http_client::HttpClient;
use oauth2::{
    basic::BasicClient, AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, ResourceServerUrl, Scope, TokenResponse, TokenUrl,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Config {
    pub client_id: String,
    pub authorization_url: Option<String>,
    pub token_url: Option<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProtectedResourceMetadata {
    resource: String,
    #[serde(rename = "authorization_servers")]
    authorization_servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuthorizationServerMetadata {
    issuer: Option<String>,
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(rename = "code_challenge_methods_supported")]
    code_challenge_methods_supported: Option<Vec<String>>,
}

pub struct OAuth2TokenManager {
    config: OAuth2Config,
    http_client: Arc<dyn HttpClient>,
    access_token: Arc<Mutex<Option<String>>>,
    refresh_token: Arc<Mutex<Option<String>>>,
}

impl OAuth2TokenManager {
    pub fn new(config: OAuth2Config, http_client: Arc<dyn HttpClient>) -> Self {
        Self {
            config,
            http_client,
            access_token: Arc::new(Mutex::new(None)),
            refresh_token: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn discover_and_authorize(&self, resource_url: &str) -> Result<String> {
        let (auth_url, token_url) = self.discover_oauth_endpoints(resource_url).await?;

        self.perform_pkce_flow(&auth_url, &token_url).await
    }

    async fn discover_oauth_endpoints(&self, resource_url: &str) -> Result<(String, String)> {
        if let (Some(auth_url), Some(token_url)) = (
            self.config.authorization_url.as_ref(),
            self.config.token_url.as_ref(),
        ) {
            return Ok((auth_url.clone(), token_url.clone()));
        }

        let base_url = Url::parse(resource_url)?;
        let protected_resource_url = format!(
            "{}://{}/.well-known/oauth-protected-resource",
            base_url.scheme(),
            base_url.host_str().ok_or_else(|| anyhow!("Invalid URL"))?
        );

        let protected_resource_response = self
            .http_client
            .get(
                &protected_resource_url,
                Default::default(),
                true,
            )
            .await?;

        if protected_resource_response.status() != 200 {
            anyhow::bail!(
                "Failed to fetch protected resource metadata: {}",
                protected_resource_response.status()
            );
        }

        let body = String::from_utf8(
            protected_resource_response
                .into_body()
                .await?
                .to_vec()
        )?;
        let protected_resource: ProtectedResourceMetadata = serde_json::from_str(&body)?;

        let auth_server_url = protected_resource
            .authorization_servers
            .first()
            .ok_or_else(|| anyhow!("No authorization servers found"))?;

        let auth_server_metadata_url = format!("{}/.well-known/oauth-authorization-server", auth_server_url);

        let auth_server_response = self
            .http_client
            .get(
                &auth_server_metadata_url,
                Default::default(),
                true,
            )
            .await?;

        if auth_server_response.status() != 200 {
            anyhow::bail!(
                "Failed to fetch authorization server metadata: {}",
                auth_server_response.status()
            );
        }

        let body = String::from_utf8(
            auth_server_response
                .into_body()
                .await?
                .to_vec()
        )?;
        let auth_server: AuthorizationServerMetadata = serde_json::from_str(&body)?;

        if let Some(methods) = &auth_server.code_challenge_methods_supported {
            if !methods.contains(&"S256".to_string()) {
                anyhow::bail!("Authorization server does not support S256 PKCE");
            }
        } else {
            anyhow::bail!("Authorization server does not advertise PKCE support");
        }

        Ok((
            auth_server.authorization_endpoint,
            auth_server.token_endpoint,
        ))
    }

    async fn perform_pkce_flow(&self, auth_url: &str, token_url: &str) -> Result<String> {
        let client = BasicClient::new(ClientId::new(self.config.client_id.clone()))
            .set_auth_uri(AuthUrl::new(auth_url.to_string())?)
            .set_token_uri(TokenUrl::new(token_url.to_string())?)
            .set_redirect_uri(RedirectUrl::new("http://localhost:8080/callback".to_string())?);

        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        let mut auth_request = client
            .authorize_url(|| CsrfToken::new_random())
            .set_pkce_challenge(pkce_challenge);

        for scope in &self.config.scopes {
            auth_request = auth_request.add_scope(Scope::new(scope.clone()));
        }

        let (auth_url, csrf_token) = auth_request.url();

        log::info!("Please visit this URL to authorize: {}", auth_url);

        let (auth_code, state) = self.start_callback_server().await?;

        if state != csrf_token.secret() {
            anyhow::bail!("CSRF token mismatch");
        }

        let token_result = client
            .exchange_code(AuthorizationCode::new(auth_code))
            .set_pkce_verifier(pkce_verifier)
            .request_async(oauth2::reqwest::async_http_client)
            .await?;

        let access_token = token_result.access_token().secret().clone();
        *self.access_token.lock() = Some(access_token.clone());

        if let Some(refresh_token) = token_result.refresh_token() {
            *self.refresh_token.lock() = Some(refresh_token.secret().clone());
        }

        Ok(access_token)
    }

    async fn start_callback_server(&self) -> Result<(String, String)> {
        use futures::channel::oneshot;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:8080")?;
        let (sender, receiver) = oneshot::channel();
        let sender = Arc::new(Mutex::new(Some(sender)));

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(mut stream) = stream {
                    use std::io::{Read, Write};

                    let mut buffer = [0; 1024];
                    if let Ok(size) = stream.read(&mut buffer) {
                        let request = String::from_utf8_lossy(&buffer[..size]);

                        if let Some(query_start) = request.find("GET /?") {
                            let query = &request[query_start + 6..];
                            if let Some(query_end) = query.find(" HTTP") {
                                let query = &query[..query_end];

                                let mut code = None;
                                let mut state = None;

                                for param in query.split('&') {
                                    if let Some((key, value)) = param.split_once('=') {
                                        match key {
                                            "code" => code = Some(value.to_string()),
                                            "state" => state = Some(value.to_string()),
                                            _ => {}
                                        }
                                    }
                                }

                                if let (Some(code), Some(state)) = (code, state) {
                                    let response = "HTTP/1.1 200 OK\r\n\r\nAuthorization successful! You can close this window.";
                                    stream.write_all(response.as_bytes()).ok();

                                    if let Some(sender) = sender.lock().take() {
                                        sender.send((code, state)).ok();
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        });

        let (code, state) = receiver.await?;
        Ok((code, state))
    }

    pub fn get_access_token(&self) -> Option<String> {
        self.access_token.lock().clone()
    }

    pub async fn ensure_valid_token(&self, resource_url: &str) -> Result<String> {
        if let Some(token) = self.get_access_token() {
            return Ok(token);
        }

        self.discover_and_authorize(resource_url).await
    }
}
