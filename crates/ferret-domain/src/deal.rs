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
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}
