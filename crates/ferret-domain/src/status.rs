//! Scheduler / source liveness types surfaced by `GET /api/status` — the
//! UI's answer to "is anything actually scraping, and did my watch match?".

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Outcome counters of one scheduler tick (mirror of the pipeline stats).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TickStats {
    pub fetched: u64,
    pub new_deals: u64,
    pub updated_deals: u64,
    pub skipped: u64,
    pub notified: u64,
    pub gone: u64,
    pub refined: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceStatus {
    pub source_id: String,
    pub interval_minutes: u64,
    /// None until the first tick completes.
    pub last_tick: Option<DateTime<Utc>>,
    pub last_stats: Option<TickStats>,
    /// Set when the last tick failed (cleared on success).
    pub last_error: Option<String>,
    pub consecutive_failures: u32,
}

impl SourceStatus {
    pub fn idle(source_id: &str, interval_minutes: u64) -> Self {
        Self {
            source_id: source_id.to_string(),
            interval_minutes,
            last_tick: None,
            last_stats: None,
            last_error: None,
            consecutive_failures: 0,
        }
    }
}

/// Whether the LLM layer (refinement + interpretation) is live, shown in
/// the sources strip so a silent heuristic-only setup is visible.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmStatus {
    pub enabled: bool,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub sources: Vec<SourceStatus>,
    /// Current match count per watch id.
    pub watch_matches: HashMap<Uuid, i64>,
    // default: older servers don't send it — the UI then shows nothing
    #[serde(default)]
    pub llm: LlmStatus,
}

/// Per-source progress of an ad-hoc guided-creation search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum SourceProgress {
    Pending,
    Done { listings: u64 },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchJob {
    pub id: Uuid,
    pub sources: HashMap<String, SourceProgress>,
    pub done: bool,
}
