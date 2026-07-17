//! Saved product-type watches and their API request/response shapes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub id: Uuid,
    /// Display name, e.g. "4TB HDD" or "RTX 3080".
    pub name: String,
    /// Product family the watch targets (must exist in the family tables
    /// for stuffing/outlier signals; free-text watches still match on the
    /// other filters).
    pub family: Option<String>,
    /// Exact model within the family, e.g. "3080".
    pub model: Option<String>,
    /// Minimum extracted capacity in decimal GB.
    pub min_capacity_gb: Option<i64>,
    /// Plausibility floor: filters accessories and scam placeholder
    /// prices that title-match the product (veille-prix pattern).
    pub min_price_cents: Option<i64>,
    pub max_price_cents: Option<i64>,
    /// Category the watch targets (guided creation); when set, only deals
    /// categorized the same can match, and `spec_filters` apply.
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub spec_filters: Vec<crate::category::SpecFilter>,
    /// Search queries this watch feeds into the scheduled scrape rotation.
    #[serde(default)]
    pub queries: Vec<String>,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchRequest {
    pub name: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub min_capacity_gb: Option<i64>,
    #[serde(default)]
    pub min_price_cents: Option<i64>,
    #[serde(default)]
    pub max_price_cents: Option<i64>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub spec_filters: Vec<crate::category::SpecFilter>,
    #[serde(default)]
    pub queries: Vec<String>,
    #[serde(default = "default_true")]
    pub active: bool,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deal::Flag;

    #[test]
    fn llm_verdict_serializes_kebab_case() {
        use crate::deal::LlmVerdict;
        assert_eq!(
            serde_json::to_string(&LlmVerdict::StuffedTitle).unwrap(),
            "\"stuffed-title\""
        );
        assert_eq!(
            serde_json::from_str::<LlmVerdict>("\"scam\"").unwrap(),
            LlmVerdict::Scam
        );
    }

    #[test]
    fn flag_serializes_kebab_case() {
        assert_eq!(
            serde_json::to_string(&Flag::PossibleStuffing).unwrap(),
            "\"possible-stuffing\""
        );
        assert_eq!(
            serde_json::to_string(&Flag::PriceOutlier).unwrap(),
            "\"price-outlier\""
        );
    }

    #[test]
    fn watch_request_defaults() {
        let req: WatchRequest = serde_json::from_str(r#"{"name": "4TB HDD"}"#).unwrap();
        assert_eq!(req.name, "4TB HDD");
        assert!(req.active);
        assert!(req.family.is_none());
    }

    #[test]
    fn watch_round_trips() {
        let w = Watch {
            id: Uuid::nil(),
            name: "RTX 3080".into(),
            family: Some("nvidia-rtx".into()),
            model: Some("3080".into()),
            min_capacity_gb: None,
            min_price_cents: None,
            max_price_cents: Some(50_000),
            category: None,
            spec_filters: vec![],
            queries: vec![],
            active: true,
            created_at: DateTime::UNIX_EPOCH,
        };
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(serde_json::from_str::<Watch>(&json).unwrap(), w);
    }
}
