//! Config-driven product family tables and the stuffing score.
//!
//! A family lists sibling models (e.g. all RTX xx80 GPUs). A title
//! enumerating many siblings is likely SEO-stuffed: the score is a SIGNAL
//! attached to the deal, never a hard filter.

use regex::{Regex, escape};
use serde::{Deserialize, Serialize};

/// One product family from config (`[[families]]` in ferret.toml).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductFamily {
    /// Stable id used by watches and price history, e.g. "nvidia-rtx".
    pub name: String,
    /// Sibling model tokens, matched word-bounded case-insensitive,
    /// e.g. ["3060", "3070", "3080", "3090", "4080"].
    pub models: Vec<String>,
}

/// Result of matching a title against the family tables.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FamilyMatch {
    /// Family with the most model hits (None when no model matched).
    pub family: Option<String>,
    /// Every model of that family present in the title.
    pub models: Vec<String>,
    /// 0.0 = at most one model; → 1.0 as the title enumerates the family.
    pub stuffing_score: f64,
}

/// Match a title against every family table; return the family with the
/// most model hits, the models found, and the stuffing score.
///
/// Score: 0 or 1 model → 0.0; otherwise `(hits - 1) / (family_size - 1)`,
/// i.e. the fraction of *additional* siblings enumerated.
pub fn match_families(title: &str, families: &[ProductFamily]) -> FamilyMatch {
    let mut best = FamilyMatch::default();
    for family in families {
        let hits: Vec<String> = family
            .models
            .iter()
            .filter(|model| {
                // word-bounded, case-insensitive; models come from config so
                // building the regex per call is fine at this scale
                Regex::new(&format!(r"(?i)\b{}\b", escape(model)))
                    .map(|re| re.is_match(title))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        if hits.len() > best.models.len() {
            let score = if hits.len() <= 1 || family.models.len() <= 1 {
                0.0
            } else {
                (hits.len() - 1) as f64 / (family.models.len() - 1) as f64
            };
            best = FamilyMatch {
                family: Some(family.name.clone()),
                models: hits,
                stuffing_score: score,
            };
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu_family() -> ProductFamily {
        ProductFamily {
            name: "nvidia-rtx".into(),
            models: ["2080", "3060", "3070", "3080", "3090", "4080", "4090"]
                .map(String::from)
                .to_vec(),
        }
    }

    #[test]
    fn single_model_scores_zero() {
        let m = match_families("RTX 3080 Founders Edition 10GB", &[gpu_family()]);
        assert_eq!(m.family.as_deref(), Some("nvidia-rtx"));
        assert_eq!(m.models, vec!["3080"]);
        assert_eq!(m.stuffing_score, 0.0);
    }

    #[test]
    fn stuffed_title_scores_high() {
        let m = match_families(
            "GPU riser for 2080 3060 3070 3080 3090 4080 4090",
            &[gpu_family()],
        );
        assert_eq!(m.family.as_deref(), Some("nvidia-rtx"));
        assert_eq!(m.models.len(), 7);
        assert_eq!(m.stuffing_score, 1.0);
    }

    #[test]
    fn two_models_score_partial() {
        // 2 of 7 models → (2-1)/(7-1) ≈ 0.1667
        let m = match_families("RTX 3080 or 3090, you pick", &[gpu_family()]);
        assert_eq!(m.models.len(), 2);
        assert!((m.stuffing_score - 1.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn model_must_be_word_bounded() {
        // "30809" must not match "3080"
        let m = match_families("Part number 30809", &[gpu_family()]);
        assert_eq!(m.family, None);
        assert!(m.models.is_empty());
    }

    #[test]
    fn no_match_is_empty_default() {
        let m = match_families("4TB IronWolf NAS drive", &[gpu_family()]);
        assert_eq!(m, FamilyMatch::default());
    }

    #[test]
    fn picks_family_with_most_hits() {
        let other = ProductFamily {
            name: "amd-rx".into(),
            models: vec!["6800".into(), "6900".into()],
        };
        let m = match_families("RTX 3080 3090 vs RX 6800", &[gpu_family(), other]);
        assert_eq!(m.family.as_deref(), Some("nvidia-rtx"));
    }
}
