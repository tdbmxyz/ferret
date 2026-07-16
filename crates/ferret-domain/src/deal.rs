//! A persisted deal: a normalized, attribute-extracted, scored listing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Heuristic warning flags attached to a deal. Signals, never hard
/// filters — a flagged deal still surfaces, tagged for the user to eyeball.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Flag {
    /// Title enumerates several sibling models (SEO stuffing).
    PossibleStuffing,
    /// Price is far below the rolling median for this family+model.
    PriceOutlier,
}

/// Lifecycle of a deal on its source. A deal is never deleted: it goes
/// `gone` when a successful scrape no longer sees it, and revives to
/// `active` if it reappears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DealStatus {
    #[default]
    Active,
    Gone,
}

/// LLM relevance verdict for an ambiguous listing. A second, independent
/// signal — heuristic flags are kept untouched next to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LlmVerdict {
    Genuine,
    StuffedTitle,
    Scam,
}

/// One dated price observation for a deal — at most one per day, the
/// latest wins. The basis for "price dropped since notified" alerts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricePoint {
    /// ISO date (UTC) of the observation.
    pub day: String,
    pub price_cents: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deal {
    pub id: Uuid,
    pub source_id: String,
    pub canonical_url: String,
    pub title: String,
    pub price_cents: i64,
    /// ISO currency code, e.g. "EUR".
    pub currency: String,
    /// Product family the title matched (config-driven tables), if any.
    pub family: Option<String>,
    /// All family models found in the title. One entry = unambiguous.
    pub models: Vec<String>,
    /// Decimal gigabytes (1 TB = 1000 GB) when a capacity was extracted.
    pub capacity_gb: Option<i64>,
    /// "new" | "used" | "refurbished" when detected.
    pub condition: Option<String>,
    /// 0.0 = single model mentioned; → 1.0 as the title enumerates the
    /// whole family.
    pub stuffing_score: f64,
    pub flags: Vec<Flag>,
    pub status: DealStatus,
    /// Verdict of the optional LLM refinement pass; None when the listing
    /// was unambiguous or the pass is disabled/failed.
    pub llm_verdict: Option<LlmVerdict>,
    /// Short model-written justification for the verdict.
    pub llm_reason: Option<String>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}
