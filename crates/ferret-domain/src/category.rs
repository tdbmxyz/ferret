//! Product categories and their typed spec dimensions — the backbone of
//! guided watch creation. A category (HDD, GPU…) declares which
//! characteristics exist (capacity, rpm, model…), each with a kind; deals
//! get spec VALUES extracted from titles, watches carry spec FILTERS
//! evaluated against them. All pure logic, exhaustively unit-tested.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};



#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpecKind {
    Number,
    Enum,
    Boolean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CategoryOrigin {
    Curated,
    Llm,
    /// Created or hand-edited from the UI.
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CategoryStatus {
    Active,
    /// Drafted by the LLM, awaiting user review — never used for
    /// categorization until approved.
    Proposed,
}

/// One characteristic a category's products have.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategorySpec {
    /// Stable key, e.g. "capacity", "rpm", "model".
    pub key: String,
    pub label: String,
    pub kind: SpecKind,
    /// Number kind: the unit values are expressed in ("GB", "rpm"…).
    /// "GB" gets the full capacity treatment (TB/To/Go variants, ×1000).
    #[serde(default)]
    pub unit: Option<String>,
    /// Enum kind: the allowed values, matched word-bounded in titles.
    #[serde(default)]
    pub allowed_values: Vec<String>,
    /// Boolean kind: comma-separated keywords whose presence means true.
    #[serde(default)]
    pub extraction_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Category {
    /// Stable id, e.g. "hdd".
    pub slug: String,
    pub label: String,
    /// Words/phrases that identify the category in titles and searches.
    pub aliases: Vec<String>,
    pub origin: CategoryOrigin,
    pub status: CategoryStatus,
    pub specs: Vec<CategorySpec>,
    pub created_at: DateTime<Utc>,
}

/// An extracted spec value on a deal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SpecValue {
    Number(f64),
    Bool(bool),
    Text(String),
}

/// A watch's constraint on one spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum SpecFilter {
    Min { key: String, value: f64 },
    Max { key: String, value: f64 },
    Eq { key: String, value: String },
    AnyOf { key: String, values: Vec<String> },
    Is { key: String, value: bool },
}

impl SpecFilter {
    pub fn key(&self) -> &str {
        match self {
            SpecFilter::Min { key, .. }
            | SpecFilter::Max { key, .. }
            | SpecFilter::Eq { key, .. }
            | SpecFilter::AnyOf { key, .. }
            | SpecFilter::Is { key, .. } => key,
        }
    }
}

/// Do the extracted values satisfy every filter? A filter whose key was
/// never extracted FAILS — a watch demanding 4 TB must not match a listing
/// whose capacity is unknown (same semantics as `min_capacity_gb`).
pub fn filters_match(filters: &[SpecFilter], specs: &HashMap<String, SpecValue>) -> bool {
    filters.iter().all(|f| match (f, specs.get(f.key())) {
        (SpecFilter::Min { value, .. }, Some(SpecValue::Number(n))) => n >= value,
        (SpecFilter::Max { value, .. }, Some(SpecValue::Number(n))) => n <= value,
        (SpecFilter::Eq { value, .. }, Some(SpecValue::Text(t))) => t.eq_ignore_ascii_case(value),
        (SpecFilter::AnyOf { values, .. }, Some(SpecValue::Text(t))) => {
            values.iter().any(|v| t.eq_ignore_ascii_case(v))
        }
        (SpecFilter::Is { value, .. }, Some(SpecValue::Bool(b))) => b == value,
        _ => false,
    })
}

fn word_bounded(title: &str, needle: &str) -> bool {
    regex::Regex::new(&format!(r"(?i)\b{}\b", regex::escape(needle)))
        .map(|re| re.is_match(title))
        .unwrap_or(false)
}

/// Extract this category's spec values from a (cleaned) title.
pub fn extract_specs(title: &str, category: &Category) -> HashMap<String, SpecValue> {
    let mut out = HashMap::new();
    for spec in &category.specs {
        match spec.kind {
            SpecKind::Number => {
                let value = match spec.unit.as_deref() {
                    // capacity gets the shared GB/TB/Go/To treatment
                    Some("GB") => crate::attributes::extract(title).capacity_gb.map(|g| g as f64),
                    Some(unit) => regex::Regex::new(&format!(
                        r"(?i)(\d+(?:[.,]\d+)?)\s*{}\b",
                        regex::escape(unit)
                    ))
                    .ok()
                    .and_then(|re| re.captures(title))
                    .and_then(|c| c[1].replace(',', ".").parse::<f64>().ok()),
                    None => None,
                };
                if let Some(n) = value {
                    out.insert(spec.key.clone(), SpecValue::Number(n));
                }
            }
            SpecKind::Enum => {
                if let Some(hit) =
                    spec.allowed_values.iter().find(|v| word_bounded(title, v))
                {
                    out.insert(spec.key.clone(), SpecValue::Text(hit.clone()));
                }
            }
            SpecKind::Boolean => {
                if let Some(hints) = &spec.extraction_hint
                    && hints
                        .split(',')
                        .map(str::trim)
                        .filter(|h| !h.is_empty())
                        .any(|h| title.to_lowercase().contains(&h.to_lowercase()))
                {
                    out.insert(spec.key.clone(), SpecValue::Bool(true));
                }
            }
        }
    }
    // condition rides along for every category (shared heuristic)
    if let Some(condition) = crate::attributes::extract(title).condition {
        out.entry("condition".into()).or_insert(SpecValue::Text(condition));
    }
    out
}

/// Which ACTIVE category does this title belong to? Alias hits count 2,
/// enum-value hits (e.g. a model number) count 1; best nonzero score wins.
pub fn categorize<'a>(title: &str, categories: &'a [Category]) -> Option<&'a Category> {
    categories
        .iter()
        .filter(|c| c.status == CategoryStatus::Active)
        .map(|c| {
            let alias_hits = c.aliases.iter().filter(|a| word_bounded(title, a)).count() * 2;
            let value_hits: usize = c
                .specs
                .iter()
                .filter(|s| s.kind == SpecKind::Enum)
                .map(|s| s.allowed_values.iter().filter(|v| word_bounded(title, v)).count())
                .sum();
            (alias_hits + value_hits, c)
        })
        .filter(|(score, _)| *score > 0)
        .max_by_key(|(score, _)| *score)
        .map(|(_, c)| c)
}

/// Result of interpreting a free-text product search ("4TB HDD").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interpretation {
    /// The known category the text mapped to, when one did.
    pub category: Option<Category>,
    /// Constraints pre-derived from the text ("4TB" → capacity ≥ 4000).
    pub constraints: Vec<SpecFilter>,
    /// Search queries the future watch should feed the scrape rotation.
    pub queries: Vec<String>,
    /// LLM-drafted NEW category (status=proposed) when nothing known
    /// matched — persisted only after user review.
    pub proposal: Option<Category>,
    /// "heuristic" | "llm" | "none" — how the answer was produced.
    pub via: String,
    /// Whether an LLM was available for the ladder — lets the UI explain
    /// a "none" honestly (unknown product vs. no LLM configured).
    #[serde(default)]
    pub llm_active: bool,
    /// Set when the LLM step was attempted and failed (fail-open) — shown
    /// to the user instead of silently pretending nothing matched.
    #[serde(default)]
    pub llm_error: Option<String>,
}

/// Instant, deterministic interpretation: categorize the text itself, then
/// turn the values it carries into filters (a number the user typed is a
/// floor — "4TB HDD" means at least 4 TB; an enum/boolean value is exact).
pub fn interpret_heuristic<'a>(
    text: &str,
    categories: &'a [Category],
) -> Option<(&'a Category, Vec<SpecFilter>)> {
    let category = categorize(text, categories)?;
    let constraints = extract_specs(text, category)
        .into_iter()
        .filter(|(key, _)| key != "condition") // typing "hdd occasion" shouldn't lock condition
        .map(|(key, value)| match value {
            SpecValue::Number(n) => SpecFilter::Min { key, value: n },
            SpecValue::Text(t) => SpecFilter::Eq { key, value: t },
            SpecValue::Bool(b) => SpecFilter::Is { key, value: b },
        })
        .collect();
    Some((category, constraints))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize;

    fn hdd() -> Category {
        Category {
            slug: "hdd".into(),
            label: "Hard drive".into(),
            aliases: ["hdd", "disque dur", "ironwolf", "wd red", "nas"]
                .map(String::from)
                .to_vec(),
            origin: CategoryOrigin::Curated,
            status: CategoryStatus::Active,
            specs: vec![
                CategorySpec {
                    key: "capacity".into(),
                    label: "Capacity (GB)".into(),
                    kind: SpecKind::Number,
                    unit: Some("GB".into()),
                    allowed_values: vec![],
                    extraction_hint: None,
                },
                CategorySpec {
                    key: "rpm".into(),
                    label: "Rotation speed".into(),
                    kind: SpecKind::Number,
                    unit: Some("rpm".into()),
                    allowed_values: vec![],
                    extraction_hint: None,
                },
            ],
            created_at: chrono::DateTime::UNIX_EPOCH,
        }
    }

    fn gpu() -> Category {
        Category {
            slug: "gpu".into(),
            label: "Graphics card".into(),
            aliases: ["gpu", "rtx", "carte graphique"].map(String::from).to_vec(),
            origin: CategoryOrigin::Curated,
            status: CategoryStatus::Active,
            specs: vec![CategorySpec {
                key: "model".into(),
                label: "Model".into(),
                kind: SpecKind::Enum,
                unit: None,
                allowed_values: ["3070", "3080", "3090"].map(String::from).to_vec(),
                extraction_hint: None,
            }],
            created_at: chrono::DateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn extracts_number_specs() {
        let specs = extract_specs("Seagate IronWolf 4TB 7200 rpm neuf", &hdd());
        assert_eq!(specs.get("capacity"), Some(&SpecValue::Number(4000.0)));
        assert_eq!(specs.get("rpm"), Some(&SpecValue::Number(7200.0)));
        assert_eq!(specs.get("condition"), Some(&SpecValue::Text("new".into())));
    }

    #[test]
    fn extracts_enum_specs() {
        let specs = extract_specs("RTX 3080 occasion", &gpu());
        assert_eq!(specs.get("model"), Some(&SpecValue::Text("3080".into())));
        assert_eq!(specs.get("condition"), Some(&SpecValue::Text("used".into())));
    }

    #[test]
    fn filters_enforce_ops_and_missing_keys_fail() {
        let mut specs = HashMap::new();
        specs.insert("capacity".to_string(), SpecValue::Number(4000.0));
        specs.insert("condition".to_string(), SpecValue::Text("used".into()));

        let min = SpecFilter::Min { key: "capacity".into(), value: 4000.0 };
        let max = SpecFilter::Max { key: "capacity".into(), value: 2000.0 };
        let cond = SpecFilter::AnyOf {
            key: "condition".into(),
            values: vec!["new".into(), "used".into()],
        };
        let rpm = SpecFilter::Min { key: "rpm".into(), value: 5400.0 };

        assert!(filters_match(&[min.clone(), cond], &specs));
        assert!(!filters_match(&[max], &specs), "over the max");
        assert!(!filters_match(&[rpm], &specs), "missing key fails");
        assert!(filters_match(&[], &specs), "no filters = pass");
    }

    #[test]
    fn categorize_by_alias_and_enum_values() {
        let cats = [hdd(), gpu()];
        assert_eq!(categorize("Seagate IronWolf 4To NAS", &cats).unwrap().slug, "hdd");
        assert_eq!(categorize("RTX 3080 Founders", &cats).unwrap().slug, "gpu");
        // bare model number, no alias — enum value hit still categorizes
        assert_eq!(categorize("MSI 3080 Ventus", &cats).unwrap().slug, "gpu");
        assert!(categorize("Chaise de bureau", &cats).is_none());
    }

    #[test]
    fn proposed_categories_never_categorize() {
        let mut proposed = hdd();
        proposed.status = CategoryStatus::Proposed;
        assert!(categorize("IronWolf 4TB", &[proposed]).is_none());
    }

    #[test]
    fn interpret_heuristic_maps_text_to_category_and_floors() {
        let cats = [hdd(), gpu()];
        let (cat, constraints) = interpret_heuristic("4TB HDD", &cats).unwrap();
        assert_eq!(cat.slug, "hdd");
        assert_eq!(
            constraints,
            vec![SpecFilter::Min { key: "capacity".into(), value: 4000.0 }]
        );

        let (cat, constraints) = interpret_heuristic("rtx 3080", &cats).unwrap();
        assert_eq!(cat.slug, "gpu");
        assert_eq!(constraints, vec![SpecFilter::Eq { key: "model".into(), value: "3080".into() }]);

        assert!(interpret_heuristic("machine à café", &cats).is_none());
    }

    #[test]
    fn spec_filter_serde_shape() {
        let f = SpecFilter::Min { key: "capacity".into(), value: 4000.0 };
        assert_eq!(
            serde_json::to_string(&f).unwrap(),
            r#"{"op":"min","key":"capacity","value":4000.0}"#
        );
    }

    #[test]
    fn clean_title_is_expected_by_extractors() {
        // extraction runs on cleaned titles: nbsp collapse must not break units
        let title = normalize::clean_title("WD\u{a0}Red 4 To 5400 rpm");
        let specs = extract_specs(&title, &hdd());
        assert_eq!(specs.get("capacity"), Some(&SpecValue::Number(4000.0)));
    }
}
