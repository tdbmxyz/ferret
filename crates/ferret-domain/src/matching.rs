//! Matching persisted deals against active watches. Pure predicate —
//! flags (stuffing, outlier) never veto a match, they ride along.

use crate::deal::Deal;
use crate::watch::Watch;

/// Does this deal satisfy this watch? Every filter the watch sets must
/// hold; filters the watch leaves unset are ignored. Flags are not
/// consulted — a flagged deal matches and surfaces with its badges.
pub fn watch_matches(watch: &Watch, deal: &Deal) -> bool {
    if !watch.active {
        return false;
    }
    if let Some(family) = &watch.family
        && deal.family.as_ref() != Some(family)
    {
        return false;
    }
    if let Some(model) = &watch.model
        && !deal.models.contains(model)
    {
        return false;
    }
    if let Some(min_gb) = watch.min_capacity_gb
        && deal.capacity_gb.is_none_or(|c| c < min_gb)
    {
        return false;
    }
    if let Some(category) = &watch.category {
        if deal.category.as_ref() != Some(category) {
            return false;
        }
        if !crate::category::filters_match(&watch.spec_filters, &deal.specs) {
            return false;
        }
    }
    if let Some(min) = watch.min_price_cents
        && deal.price_cents < min
    {
        return false;
    }
    if let Some(max) = watch.max_price_cents
        && deal.price_cents > max
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use uuid::Uuid;

    fn deal() -> Deal {
        Deal {
            id: Uuid::nil(),
            source_id: "src".into(),
            canonical_url: "https://ex.com/1".into(),
            title: "RTX 3080 10GB".into(),
            price_cents: 45_000,
            currency: "EUR".into(),
            family: Some("nvidia-rtx".into()),
            models: vec!["3080".into()],
            capacity_gb: Some(10),
            condition: None,
            stuffing_score: 0.0,
            flags: vec![],
            status: crate::deal::DealStatus::Active,
            llm_verdict: None,
            llm_reason: None,
            category: None,
            specs: Default::default(),
            first_seen: DateTime::UNIX_EPOCH,
            last_seen: DateTime::UNIX_EPOCH,
        }
    }

    fn watch() -> Watch {
        Watch {
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
        }
    }

    #[test]
    fn matches_family_model_and_price() {
        assert!(watch_matches(&watch(), &deal()));
    }

    #[test]
    fn price_over_budget_rejects() {
        let mut d = deal();
        d.price_cents = 60_000;
        assert!(!watch_matches(&watch(), &d));
    }

    #[test]
    fn wrong_model_rejects() {
        let mut w = watch();
        w.model = Some("3090".into());
        assert!(!watch_matches(&w, &deal()));
    }

    #[test]
    fn stuffed_listing_containing_watched_model_still_matches() {
        // spec: stuffing is a signal, not a filter
        let mut d = deal();
        d.models = vec!["3070".into(), "3080".into(), "3090".into()];
        d.stuffing_score = 0.4;
        assert!(watch_matches(&watch(), &d));
    }

    #[test]
    fn capacity_floor_enforced() {
        let mut w = watch();
        w.family = None;
        w.model = None;
        w.min_capacity_gb = Some(4000);
        let mut d = deal();
        d.capacity_gb = Some(2000);
        assert!(!watch_matches(&w, &d));
        d.capacity_gb = Some(4000);
        assert!(watch_matches(&w, &d));
        d.capacity_gb = None; // watch demands capacity, deal has none
        assert!(!watch_matches(&w, &d));
    }

    #[test]
    fn price_floor_filters_implausible_listings() {
        // veille-prix pattern: a floor filters accessories ("support RTX
        // 3080, 15 €") and scam placeholder prices
        let mut w = watch();
        w.min_price_cents = Some(20_000);
        let mut d = deal();
        d.price_cents = 1_500;
        assert!(!watch_matches(&w, &d));
        d.price_cents = 45_000;
        assert!(watch_matches(&w, &d));
    }

    #[test]
    fn category_watch_gates_on_category_and_spec_filters() {
        use crate::category::{SpecFilter, SpecValue};
        let mut w = watch();
        w.family = None;
        w.model = None;
        w.category = Some("hdd".into());
        w.spec_filters = vec![SpecFilter::Min { key: "capacity".into(), value: 4000.0 }];

        let mut d = deal();
        assert!(!watch_matches(&w, &d), "uncategorized deal rejected");

        d.category = Some("hdd".into());
        assert!(!watch_matches(&w, &d), "capacity missing → filter fails");

        d.specs.insert("capacity".into(), SpecValue::Number(4000.0));
        assert!(watch_matches(&w, &d));

        d.category = Some("ssd".into());
        assert!(!watch_matches(&w, &d), "wrong category rejected");
    }

    #[test]
    fn inactive_watch_never_matches() {
        let mut w = watch();
        w.active = false;
        assert!(!watch_matches(&w, &deal()));
    }
}
