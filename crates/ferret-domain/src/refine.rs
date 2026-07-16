//! Gate for the optional LLM refinement pass: only AMBIGUOUS listings are
//! worth a model call — the common case must never touch the LLM.

use crate::deal::Flag;

/// A listing is ambiguous when it matched a product family AND either
/// enumerates several sibling models (stuffed title? genuine bundle?) or
/// carries a price-outlier flag (scam? genuine deal?). No family match =
/// irrelevant listing = never refined.
pub fn needs_refinement(family: Option<&str>, models: &[String], flags: &[Flag]) -> bool {
    family.is_some() && (models.len() >= 2 || flags.contains(&Flag::PriceOutlier))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn clean_single_model_is_not_ambiguous() {
        assert!(!needs_refinement(Some("nvidia-rtx"), &models(&["3080"]), &[]));
    }

    #[test]
    fn multi_model_title_is_ambiguous() {
        assert!(needs_refinement(Some("nvidia-rtx"), &models(&["3080", "3090"]), &[]));
    }

    #[test]
    fn price_outlier_is_ambiguous() {
        assert!(needs_refinement(
            Some("nvidia-rtx"),
            &models(&["3080"]),
            &[Flag::PriceOutlier]
        ));
    }

    #[test]
    fn no_family_match_is_never_refined() {
        assert!(!needs_refinement(None, &[], &[Flag::PriceOutlier]));
    }
}
