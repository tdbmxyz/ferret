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
    /// A buy request ("Recherche RTX 5090"), not an offer to sell.
    WantedAd,
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

/// User verdict on a deal, orthogonal to its lifecycle status.
/// Moderated deals never match watches and never notify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Moderation {
    #[default]
    None,
    /// Hidden for now — clears automatically if the listing disappears
    /// and is later re-acquired (it may be relevant again).
    Dismissed,
    /// Never show or match this listing again, ever.
    Banned,
}

/// LLM relevance verdict for an ambiguous listing. A second, independent
/// signal — heuristic flags are kept untouched next to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LlmVerdict {
    /// An offer to sell the working product itself as the main item.
    Genuine,
    /// Title enumerates sibling models for search visibility.
    StuffedTitle,
    Scam,
    /// Not the product: a PC/laptop merely containing it, an accessory,
    /// an empty box, a for-parts unit, a wanted ad…
    Irrelevant,
}

/// One dated price observation for a deal — at most one per day, the
/// latest wins. The basis for "price dropped since notified" alerts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricePoint {
    /// ISO date (UTC) of the observation.
    pub day: String,
    pub price_cents: i64,
}

/// Daily aggregate over every deal matched by one watch — the watch's
/// price-history chart (min = best buy that day, median = the market).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchPricePoint {
    /// ISO date (UTC).
    pub day: String,
    pub min_cents: i64,
    pub median_cents: i64,
    /// Deals observed that day.
    pub count: i64,
}

/// One watch a deal matched, and whether that match was pushed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchInfo {
    pub watch_id: Uuid,
    pub watch_name: String,
    /// Set when a notification went out (at that price).
    pub notified_price_cents: Option<i64>,
}

/// API row of `GET /api/deals`: the deal plus its match outcomes, so the
/// UI can always say whether (and why not) a deal was reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DealRow {
    #[serde(flatten)]
    pub deal: Deal,
    #[serde(default)]
    pub matches: Vec<MatchInfo>,
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
    /// Product category (guided-watch system), when the title matched one.
    #[serde(default)]
    pub category: Option<String>,
    /// Spec values extracted per the category's spec definitions.
    #[serde(default)]
    pub specs: std::collections::HashMap<String, crate::category::SpecValue>,
    /// User verdict: dismissed/banned deals are hidden and never match.
    #[serde(default)]
    pub moderation: Moderation,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}
