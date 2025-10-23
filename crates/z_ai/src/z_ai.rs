use anyhow::Result;
use serde::{Deserialize, Serialize};
use strum::EnumIter;

pub const ZAI_API_URL: &str = "https://api.z.ai/api/paas/v4";

#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, EnumIter)]
pub enum Model {
    #[default]
    #[serde(rename = "glm-4.6")]
    Glm46,
    #[serde(rename = "glm-4.5")]
    Glm45,
    #[serde(rename = "glm-4.5-air")]
    Glm45Air,
    #[serde(rename = "custom")]
    Custom {
        name: String,
        /// The name displayed in the UI, such as in the assistant panel model dropdown menu.
        display_name: Option<String>,
        max_tokens: u64,
        max_output_tokens: Option<u64>,
        max_completion_tokens: Option<u64>,
        supports_images: Option<bool>,
        supports_tools: Option<bool>,
        parallel_tool_calls: Option<bool>,
    },
}

impl Model {
    pub fn default_fast() -> Self {
        Self::Glm45Air
    }

    pub fn from_id(id: &str) -> Result<Self> {
        match id {
            "glm-4.6" => Ok(Self::Glm46),
            "glm-4.5" => Ok(Self::Glm45),
            "glm-4.5-air" => Ok(Self::Glm45Air),
            _ => anyhow::bail!("invalid model id '{id}'"),
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Glm46 => "glm-4.6",
            Self::Glm45 => "glm-4.5",
            Self::Glm45Air => "glm-4.5-air",
            Self::Custom { name, .. } => name,
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            Self::Glm46 => "GLM-4.6",
            Self::Glm45 => "GLM-4.5",
            Self::Glm45Air => "GLM-4.5 Air",
            Self::Custom {
                name, display_name, ..
            } => display_name.as_ref().unwrap_or(name),
        }
    }

    pub fn max_token_count(&self) -> u64 {
        match self {
            Self::Glm46 => 200_000,
            Self::Glm45 | Self::Glm45Air => 128_000,
            Self::Custom { max_tokens, .. } => *max_tokens,
        }
    }

    pub fn max_output_tokens(&self) -> Option<u64> {
        match self {
            Self::Glm46 => Some(64_000),
            Self::Glm45 | Self::Glm45Air => Some(8_192),
            Self::Custom {
                max_output_tokens, ..
            } => *max_output_tokens,
        }
    }

    pub fn supports_parallel_tool_calls(&self) -> bool {
        match self {
            Self::Glm46 | Self::Glm45 | Self::Glm45Air => true,
            Self::Custom {
                parallel_tool_calls: Some(support),
                ..
            } => *support,
            Self::Custom { .. } => false,
        }
    }

    pub fn supports_prompt_cache_key(&self) -> bool {
        false
    }

    pub fn supports_tool(&self) -> bool {
        match self {
            Self::Glm46 | Self::Glm45 | Self::Glm45Air => true,
            Self::Custom {
                supports_tools: Some(support),
                ..
            } => *support,
            Self::Custom { .. } => false,
        }
    }

    pub fn supports_images(&self) -> bool {
        match self {
            Self::Glm46 | Self::Glm45 => true,
            Self::Glm45Air => false,
            Self::Custom {
                supports_images: Some(support),
                ..
            } => *support,
            Self::Custom { .. } => false,
        }
    }
}
