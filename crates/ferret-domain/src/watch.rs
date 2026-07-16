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
    pub max_price_cents: Option<i64>,
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
    pub max_price_cents: Option<i64>,
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
            max_price_cents: Some(50_000),
            active: true,
            created_at: DateTime::UNIX_EPOCH,
        };
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(serde_json::from_str::<Watch>(&json).unwrap(), w);
    }
}
