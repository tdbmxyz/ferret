//! Runtime-editable server settings. The TOML config is the base; a
//! DB-stored override (edited from the UI) takes precedence field by field
//! and applies without a restart.

use serde::{Deserialize, Serialize};

/// Effective LLM configuration as reported by `GET /api/settings/llm`.
/// The API key itself never leaves the server — only whether one is set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmSettings {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    /// An override API key is stored on the server.
    pub api_key_set: bool,
    /// True when a DB override is active (vs. pure TOML config).
    pub from_override: bool,
}

/// The three system prompts driving the LLM calls. In override storage an
/// empty field means "use the built-in default".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSet {
    /// Listing review (genuine / stuffed-title / scam).
    pub refine: String,
    /// Free-text search → category + constraints / proposal.
    pub interpret: String,
    /// Category rework per user instruction.
    pub revise: String,
}

/// `GET /api/settings/prompts` — what runs now, and the factory defaults
/// (so the UI can offer a reset and show what changed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptsResponse {
    pub current: PromptSet,
    pub default: PromptSet,
}

/// One message of an LLM revision conversation (role "user"/"assistant").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

/// Body of `POST /api/settings/llm/models` and `/test`: probe an endpoint
/// with the values currently typed in the form. Missing fields fall back
/// to the server's effective settings (including the stored API key).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmProbeRequest {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Answer of `POST /api/settings/llm/test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmProbeResult {
    pub ok: bool,
    pub error: Option<String>,
}

/// `PUT /api/settings/llm` body — replaces the whole override.
/// `api_key: None` keeps the stored key; `Some("")` clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmSettingsUpdate {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
}
