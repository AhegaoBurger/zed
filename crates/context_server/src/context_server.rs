// Zed's context server implementation, now powered by the official RMCP SDK.
// This module provides a unified interface for interacting with context servers
// over different transport protocols like stdio, http, and sse.

#![cfg(feature = "rmcp")]

pub mod settings;

use anyhow::{anyhow, Result};
use gpui::App;
use parking_lot::RwLock;
use rmcp::{
    model::{CallToolRequestParam, Tool},
    service::{role, CallToolResponse},
    transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess},
    Client, ServiceExt,
};
use reqwest::Client as ReqwestClient;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt::Display,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::process::Command;
use util::redact::should_redact;

/// A unique identifier for a context server.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContextServerId(pub Arc<str>);

impl Display for ContextServerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A command to execute a context server as a child process.
#[derive(Deserialize, Serialize, Clone, PartialEq, Eq, JsonSchema)]
pub struct ContextServerCommand {
    #[serde(rename = "command")]
    pub path: PathBuf,
    pub args: Vec<String>,
    pub env: Option<HashMap<String, String>>,
    /// Timeout for tool calls in milliseconds. Defaults to 60000 (60 seconds) if not specified.
    pub timeout: Option<u64>,
}

impl std::fmt::Debug for ContextServerCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let filtered_env = self.env.as_ref().map(|env| {
            env.iter()
                .map(|(k, v)| (k, if should_redact(k) { "[REDACTED]" } else { v }))
                .collect::<Vec<_>>()
        });

        f.debug_struct("ContextServerCommand")
            .field("path", &self.path)
            .field("args", &self.args)
            .field("env", &filtered_env)
            .finish()
    }
}

/// The transport configuration for a context server.
#[derive(Debug, Clone)]
pub enum ContextServerTransport {
    Stdio(ContextServerCommand, Option<PathBuf>),
    Http {
        url: String,
        headers: HashMap<String, String>,
    },
    Sse {
        url: String,
        headers: HashMap<String, String>,
    },
}

/// Represents a connection to a context server.
/// This struct wraps an `rmcp` client and manages its lifecycle.
pub struct ContextServer {
    id: ContextServerId,
    client: RwLock<Option<Client>>,
    configuration: ContextServerTransport,
}

impl ContextServer {
    /// Creates a new context server that communicates over stdio.
    pub fn stdio(
        id: ContextServerId,
        command: ContextServerCommand,
        working_directory: Option<Arc<Path>>,
    ) -> Self {
        Self {
            id,
            client: RwLock::new(None),
            configuration: ContextServerTransport::Stdio(
                command,
                working_directory.map(|p| p.to_path_buf()),
            ),
        }
    }

    /// Creates a new context server that communicates over HTTP.
    pub fn http(id: ContextServerId, url: String, headers: HashMap<String, String>) -> Self {
        Self {
            id,
            client: RwLock::new(None),
            configuration: ContextServerTransport::Http { url, headers },
        }
    }

    /// Creates a new context server that communicates over SSE.
    pub fn sse(id: ContextServerId, url: String, headers: HashMap<String, String>) -> Self {
        Self {
            id,
            client: RwLock::new(None),
            configuration: ContextServerTransport::Sse { url, headers },
        }
    }

    pub fn id(&self) -> ContextServerId {
        self.id.clone()
    }

    pub fn client(&self) -> Option<Client> {
        self.client.read().clone()
    }

    /// Starts the context server and establishes a connection.
    pub async fn start(&self, _cx: &App) -> Result<()> {
        let client: Client = match &self.configuration {
            ContextServerTransport::Stdio(command, working_directory) => {
                let child_process =
                    TokioChildProcess::new(Command::new(&command.path).configure(|cmd| {
                        cmd.args(&command.args);
                        if let Some(env) = &command.env {
                            cmd.envs(env);
                        }
                        if let Some(cwd) = working_directory {
                            cmd.current_dir(cwd);
                        }
                    }))?;
                ().serve(child_process).await?
            }
            ContextServerTransport::Http { url, headers }
            | ContextServerTransport::Sse { url, headers } => {
                let mut builder =
                    StreamableHttpClientTransport::<ReqwestClient>::builder(url.clone());
                for (key, value) in headers {
                    builder = builder.with_header(key.clone(), value.clone());
                }
                let transport = builder.build();
                ().serve(transport).await?
            }
        };

        log::info!(
            "context server {} connected, server info: {:?}",
            self.id,
            client.peer_info()
        );

        *self.client.write() = Some(client);
        Ok(())
    }

    /// Stops the context server and terminates the connection.
    pub async fn stop(&self) -> Result<()> {
        if let Some(client) = self.client.write().take() {
            client.cancel().await?;
        }
        Ok(())
    }

    // Delegated methods to the underlying rmcp client.
    // More methods can be added here as needed.

    pub async fn list_all_tools(&self) -> Result<Vec<Tool>> {
        self.client()
            .ok_or_else(|| anyhow!("client not connected"))?
            .list_all_tools()
            .await
            .map_err(|e| anyhow!("failed to list tools: {}", e))
    }

    pub async fn call_tool(&self, params: CallToolRequestParam) -> Result<CallToolResponse> {
        self.client()
            .ok_or_else(|| anyhow!("client not connected"))?
            .call_tool(params)
            .await
            .map_err(|e| anyhow!("failed to call tool: {}", e))
    }
}
