//! The ETL pipeline: raw listings → normalize → extract → score → dedupe →
//! watch match → persist → notify → lifecycle. Pure logic lives in
//! ferret-domain; this module only sequences it and talks to
//! storage/notifier.
//!
//! Lifecycle (patterns adopted from ent/veille-prix): every price change is
//! recorded per deal; deals a successful tick no longer sees go `gone`
//! (revived if reseen); an already-notified match is re-notified when its
//! price drops by `renotify_drop_pct`.

use std::collections::HashSet;

use chrono::Utc;
use ferret_domain::{
    Deal, DealStatus, Flag, ProductFamily, RawListing, attributes, category, family, matching,
    normalize, price, refine,
};
use uuid::Uuid;

use crate::config::ScrapeConfig;
use crate::db::{Db, UpsertOutcome};
use crate::llm::{LlmRefiner, RefineInput};
use crate::notify::Notify;

/// How many recent observations feed the rolling median.
const PRICE_WINDOW: u32 = 50;

#[derive(Debug, Default, PartialEq)]
pub struct PipelineStats {
    pub new_deals: u64,
    pub updated_deals: u64,
    pub skipped: u64,
    pub notified: u64,
    /// Deals of this source no longer seen by this tick.
    pub gone: u64,
    /// New ambiguous deals refined by the LLM pass this tick.
    pub refined: u64,
    /// Watch matches recorded WITHOUT a push (noise verdict/heuristics).
    pub suppressed: u64,
}

/// Process one source's fetch.
///
/// `run_lifecycle` must be true ONLY for a scheduler tick's full fetch: it
/// scopes the disappearance pass (deals of `source_id` absent from
/// `listings` go gone). Ad-hoc guided-creation searches are PARTIAL fetches
/// and pass false — they must never make unrelated deals "disappear".
#[allow(clippy::too_many_arguments)]
pub async fn process_listings(
    db: &Db,
    families: &[ProductFamily],
    scrape: &ScrapeConfig,
    source_id: &str,
    listings: Vec<RawListing>,
    notifier: &dyn Notify,
    refiner: Option<&dyn LlmRefiner>,
    run_lifecycle: bool,
) -> anyhow::Result<PipelineStats> {
    let mut stats = PipelineStats::default();
    let watches = db.list_watches().await?;
    let categories = db.list_categories().await?;
    let mut seen_urls: HashSet<String> = HashSet::new();

    for raw in listings {
        // -- normalize --
        let title = normalize::clean_title(&raw.title);
        let Some((price_cents, currency)) = normalize::parse_price(&raw.price_text) else {
            tracing::debug!(source = raw.source_id, title, "skipping: unparseable price");
            stats.skipped += 1;
            continue;
        };
        let Some(canonical_url) = normalize::canonical_url(&raw.url) else {
            tracing::debug!(source = raw.source_id, url = raw.url, "skipping: bad url");
            stats.skipped += 1;
            continue;
        };
        seen_urls.insert(canonical_url.clone());

        // -- extract + score --
        let attrs = attributes::extract(&title);
        let fam = family::match_families(&title, families);
        let (deal_category, deal_specs) = match category::categorize(&title, &categories) {
            Some(cat) => (Some(cat.slug.clone()), category::extract_specs(&title, cat)),
            None => (None, Default::default()),
        };

        let mut flags = Vec::new();
        if fam.models.len() >= 2 && fam.stuffing_score >= scrape.stuffing_threshold {
            flags.push(Flag::PossibleStuffing);
        }
        // outlier check needs an unambiguous (family, model) identity
        if let (Some(family_name), [model]) = (&fam.family, fam.models.as_slice()) {
            let history = db.recent_prices(family_name, model, PRICE_WINDOW).await?;
            if price::is_outlier(price_cents, &history, scrape.outlier_ratio) {
                flags.push(Flag::PriceOutlier);
            }
        }

        let now = Utc::now();
        let deal = Deal {
            id: Uuid::new_v4(),
            source_id: raw.source_id.clone(),
            canonical_url,
            title,
            price_cents,
            currency,
            family: fam.family.clone(),
            models: fam.models.clone(),
            capacity_gb: attrs.capacity_gb,
            condition: attrs.condition,
            stuffing_score: fam.stuffing_score,
            flags,
            status: DealStatus::Active,
            llm_verdict: None,
            llm_reason: None,
            category: deal_category,
            specs: deal_specs,
            moderation: Default::default(),
            first_seen: now,
            last_seen: now,
        };

        // -- dedupe / persist --
        let (stored, outcome) = db.upsert_deal(&deal).await?;
        let was_new = outcome == UpsertOutcome::New;
        if was_new {
            stats.new_deals += 1;
        } else {
            stats.updated_deals += 1;
        }

        // -- optional LLM refinement, fail-open on any error. Runs for new
        //    ambiguous deals AND for any unverdicted deal about to match a
        //    watch: the verdict gates notifications (noise filter), so a
        //    match must be looked at before it pings the user --
        let mut stored = stored;
        let would_match = stored.moderation == ferret_domain::Moderation::None
            && watches.iter().any(|w| matching::watch_matches(w, &stored));
        if let Some(refiner) = refiner
            && stored.llm_verdict.is_none()
            && ((was_new
                && stored.family.is_some()
                && refine::needs_refinement(
                    stored.family.as_deref(),
                    &stored.models,
                    &stored.flags,
                ))
                || would_match)
        {
            let family_label = stored
                .family
                .clone()
                .or_else(|| stored.category.clone())
                .unwrap_or_else(|| "unknown".into());
            let input = RefineInput {
                title: &stored.title,
                price_cents: stored.price_cents,
                currency: &stored.currency,
                family: &family_label,
                models: &stored.models,
                flags: &stored.flags,
            };
            match refiner.refine(&input).await {
                Ok(r) => {
                    stored = db
                        .apply_refinement(
                            stored.id,
                            r.verdict,
                            &r.reason,
                            r.capacity_gb,
                            r.condition.as_deref(),
                        )
                        .await?;
                    stats.refined += 1;
                }
                Err(e) => {
                    tracing::warn!(deal = %stored.id, error = %e,
                        "llm refinement failed — keeping heuristic-only verdict");
                }
            }
        }

        // -- price history: only unambiguous listings feed the median,
        //    and only on first sight (re-scrapes would skew it) --
        if was_new
            && !stored.flags.contains(&Flag::PriceOutlier)
            && let (Some(family_name), [model]) = (&stored.family, stored.models.as_slice())
        {
            db.record_price(family_name, model, stored.price_cents).await?;
        }

        // -- match watches + notify --
        // user-moderated deals (dismissed/banned) never match or notify
        if stored.moderation != ferret_domain::Moderation::None {
            continue;
        }
        for watch in &watches {
            if !matching::watch_matches(watch, &stored) {
                continue;
            }
            let fresh_match = db.insert_match(stored.id, watch.id).await?;
            let price_major = stored.price_cents as f64 / 100.0;
            if fresh_match && !notification_worthy(&stored) {
                // match recorded (visible in the UI with its badges) but
                // the push is suppressed — noise, per verdict/heuristics
                stats.suppressed += 1;
                tracing::info!(deal = %stored.id, watch = %watch.name, title = stored.title,
                    "match suppressed as noise");
            } else if fresh_match {
                let mut tags: Vec<String> = vec!["moneybag".into()];
                tags.extend(stored.flags.iter().map(flag_tag));
                notifier
                    .send(
                        &format!("{}: {:.2} {}", watch.name, price_major, stored.currency),
                        &format!("{}\n{}", stored.title, stored.canonical_url),
                        &tags.join(","),
                        "default",
                    )
                    .await;
                db.mark_notified(stored.id, watch.id, stored.price_cents).await?;
                stats.notified += 1;
            } else if let Some(prev) = db.notified_price(stored.id, watch.id).await?
                && (stored.price_cents as f64)
                    <= (prev as f64) * (1.0 - scrape.renotify_drop_pct / 100.0)
                && notification_worthy(&stored)
            {
                // known match, but the price dropped since we last pinged
                let mut tags: Vec<String> = vec!["chart_with_downwards_trend".into()];
                tags.extend(stored.flags.iter().map(flag_tag));
                notifier
                    .send(
                        &format!(
                            "{}: {:.2} → {:.2} {}",
                            watch.name,
                            prev as f64 / 100.0,
                            price_major,
                            stored.currency
                        ),
                        &format!("Price drop\n{}\n{}", stored.title, stored.canonical_url),
                        &tags.join(","),
                        "default",
                    )
                    .await;
                db.mark_notified(stored.id, watch.id, stored.price_cents).await?;
                stats.notified += 1;
            }
        }
    }

    // -- lifecycle: deals of this source the tick no longer sees --
    if run_lifecycle {
        stats.gone = db.mark_gone(source_id, &seen_urls).await?;
    }

    Ok(stats)
}

/// Should a watch match ping the user? Aggressive noise policy: an LLM
/// verdict of stuffed-title/scam suppresses the push; without a verdict
/// (LLM off or failed) the possible-stuffing heuristic suppresses it.
/// Suppressed matches stay visible in the UI with their badges.
fn notification_worthy(deal: &Deal) -> bool {
    match deal.llm_verdict {
        Some(ferret_domain::LlmVerdict::StuffedTitle | ferret_domain::LlmVerdict::Scam) => false,
        Some(ferret_domain::LlmVerdict::Genuine) => true,
        None => !deal.flags.contains(&Flag::PossibleStuffing),
    }
}

fn flag_tag(flag: &Flag) -> String {
    serde_json::to_string(flag).expect("flag serializes").trim_matches('"').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    use chrono::Utc;
    use ferret_domain::{ProductFamily, RawListing, WatchRequest};

    use crate::config::ScrapeConfig;
    use crate::db::Db;
    use crate::notify::Notify;

    /// Records every notification instead of publishing.
    #[derive(Default)]
    struct RecordingNotifier {
        sent: Mutex<Vec<(String, String)>>, // (title, tags)
    }

    #[async_trait::async_trait]
    impl Notify for RecordingNotifier {
        async fn send(&self, title: &str, _message: &str, tags: &str, _priority: &str) {
            self.sent.lock().unwrap().push((title.into(), tags.into()));
        }
    }

    fn listing(title: &str, price: &str, url: &str) -> RawListing {
        RawListing {
            source_id: "test-src".into(),
            title: title.into(),
            price_text: price.into(),
            url: url.into(),
            scraped_at: Utc::now(),
        }
    }

    fn families() -> Vec<ProductFamily> {
        vec![ProductFamily {
            name: "nvidia-rtx".into(),
            models: ["3070", "3080", "3090", "4080", "4090"].map(String::from).to_vec(),
            context: vec![],
        }]
    }

    /// Canned or failing refiner that counts its calls.
    struct MockRefiner {
        calls: Mutex<u32>,
        result: Option<crate::llm::Refinement>, // None = simulate an LLM error
    }

    impl MockRefiner {
        fn returning(r: crate::llm::Refinement) -> Self {
            Self { calls: Mutex::new(0), result: Some(r) }
        }
        fn failing() -> Self {
            Self { calls: Mutex::new(0), result: None }
        }
        fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl LlmRefiner for MockRefiner {
        async fn refine(&self, _input: &RefineInput<'_>) -> anyhow::Result<crate::llm::Refinement> {
            *self.calls.lock().unwrap() += 1;
            self.result.clone().ok_or_else(|| anyhow::anyhow!("llm backend down"))
        }
    }

    async fn run(
        db: &Db,
        listings: Vec<RawListing>,
        notifier: &RecordingNotifier,
    ) -> PipelineStats {
        run_with(db, listings, notifier, None).await
    }

    async fn run_with(
        db: &Db,
        listings: Vec<RawListing>,
        notifier: &RecordingNotifier,
        refiner: Option<&dyn LlmRefiner>,
    ) -> PipelineStats {
        process_listings(
            db,
            &families(),
            &ScrapeConfig::default(),
            "test-src",
            listings,
            notifier,
            refiner,
            true,
        )
        .await
        .unwrap()
    }

    async fn setup() -> (Db, RecordingNotifier) {
        let db = Db::connect(Path::new(":memory:")).await.unwrap();
        db.create_watch(&WatchRequest {
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
        })
        .await
        .unwrap();
        (db, RecordingNotifier::default())
    }

    #[tokio::test]
    async fn matching_listing_is_persisted_and_notified() {
        let (db, notifier) = setup().await;
        let stats = run(
            &db,
            vec![listing("RTX 3080 FE occasion", "450 €", "https://ex.com/1?utm_source=x")],
            &notifier,
        )
        .await;

        assert_eq!(stats.new_deals, 1);
        assert_eq!(stats.notified, 1);

        let deals = db.list_deals(None, false).await.unwrap();
        assert_eq!(deals.len(), 1);
        assert_eq!(deals[0].canonical_url, "https://ex.com/1"); // tracking stripped
        assert_eq!(deals[0].price_cents, 45_000);
        assert_eq!(deals[0].models, vec!["3080"]);
        assert!(deals[0].flags.is_empty());

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].0.contains("RTX 3080"), "notification titled by watch");
    }

    #[tokio::test]
    async fn rescrape_does_not_renotify() {
        let (db, notifier) = setup().await;
        let l = listing("RTX 3080 FE", "450 €", "https://ex.com/1");
        run(&db, vec![l.clone()], &notifier).await;
        let stats = run(&db, vec![l], &notifier).await;

        assert_eq!(stats.new_deals, 0);
        assert_eq!(stats.updated_deals, 1);
        assert_eq!(stats.notified, 0, "same deal same price, no second push");
        assert_eq!(notifier.sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn price_drop_renotifies_small_change_does_not() {
        let (db, notifier) = setup().await;
        run(&db, vec![listing("RTX 3080 FE", "450 €", "https://ex.com/1")], &notifier).await;

        // -2% : below the 5% threshold → silent
        let stats = run(&db, vec![listing("RTX 3080 FE", "441 €", "https://ex.com/1")], &notifier)
            .await;
        assert_eq!(stats.notified, 0);

        // -10% vs last NOTIFIED price (450) → re-notify
        let stats = run(&db, vec![listing("RTX 3080 FE", "405 €", "https://ex.com/1")], &notifier)
            .await;
        assert_eq!(stats.notified, 1);
        {
            let sent = notifier.sent.lock().unwrap();
            assert_eq!(sent.len(), 2);
            assert!(sent[1].0.contains("450.00 → 405.00"), "drop shown in title: {}", sent[1].0);
            assert!(sent[1].1.contains("chart_with_downwards_trend"));
        }

        // price history recorded every change
        let deal_id = db.list_deals(None, false).await.unwrap()[0].id;
        let prices = db.deal_prices(deal_id).await.unwrap();
        assert_eq!(prices.last().unwrap().price_cents, 40_500);
    }

    #[tokio::test]
    async fn unseen_deal_goes_gone_and_revives() {
        let (db, notifier) = setup().await;
        run(&db, vec![listing("RTX 3080 FE", "450 €", "https://ex.com/1")], &notifier).await;

        // next tick: the listing is no longer published
        let stats = run(&db, vec![], &notifier).await;
        assert_eq!(stats.gone, 1);
        assert_eq!(db.list_deals(None, false).await.unwrap()[0].status, DealStatus::Gone);

        // it reappears → revived, no duplicate notification
        let stats = run(&db, vec![listing("RTX 3080 FE", "450 €", "https://ex.com/1")], &notifier)
            .await;
        assert_eq!(stats.gone, 0);
        assert_eq!(stats.notified, 0);
        assert_eq!(db.list_deals(None, false).await.unwrap()[0].status, DealStatus::Active);
    }

    #[tokio::test]
    async fn stuffed_listing_matches_with_flag() {
        let (db, notifier) = setup().await;
        run(
            &db,
            vec![listing(
                "Brackets for 3070 3080 3090 4080 4090",
                "400 €",
                "https://ex.com/stuffed",
            )],
            &notifier,
        )
        .await;

        let deals = db.list_deals(None, false).await.unwrap();
        assert_eq!(deals.len(), 1);
        assert!(deals[0].flags.contains(&ferret_domain::Flag::PossibleStuffing));
        // noise policy: the match is recorded and visible, the push is not
        assert_eq!(notifier.sent.lock().unwrap().len(), 0, "stuffed = no push");
        assert!(!db.list_deals(only_watch(&db).await, false).await.unwrap().is_empty(),
            "match still recorded for the UI");
    }

    /// The single test watch's id (setup creates exactly one).
    async fn only_watch(db: &Db) -> Option<uuid::Uuid> {
        Some(db.list_watches().await.unwrap()[0].id)
    }

    #[tokio::test]
    async fn ambiguous_new_deal_is_refined_and_merged() {
        use ferret_domain::LlmVerdict;
        let (db, notifier) = setup().await;
        let refiner = MockRefiner::returning(crate::llm::Refinement {
            verdict: LlmVerdict::StuffedTitle,
            reason: "accessory listing".into(),
            capacity_gb: Some(10),
            condition: None,
        });
        let stats = run_with(
            &db,
            vec![listing("Riser for 3070 3080 3090 4080 4090", "400 €", "https://ex.com/s")],
            &notifier,
            Some(&refiner),
        )
        .await;

        assert_eq!(stats.refined, 1);
        assert_eq!(refiner.calls(), 1);
        let deals = db.list_deals(None, false).await.unwrap();
        assert_eq!(deals[0].llm_verdict, Some(LlmVerdict::StuffedTitle));
        assert_eq!(deals[0].llm_reason.as_deref(), Some("accessory listing"));
        assert_eq!(deals[0].capacity_gb, Some(10), "empty capacity filled by LLM");
        // heuristic signal untouched; match recorded but the stuffed-title
        // verdict suppresses the push (noise policy)
        assert!(deals[0].flags.contains(&ferret_domain::Flag::PossibleStuffing));
        assert_eq!(notifier.sent.lock().unwrap().len(), 0);
        assert_eq!(stats.suppressed, 1);
    }

    #[tokio::test]
    async fn refinement_never_overwrites_heuristic_condition() {
        use ferret_domain::LlmVerdict;
        let (db, notifier) = setup().await;
        let refiner = MockRefiner::returning(crate::llm::Refinement {
            verdict: LlmVerdict::Genuine,
            reason: "bundle of two GPUs".into(),
            capacity_gb: None,
            condition: Some("new".into()), // heuristics will already say "used"
        });
        run_with(
            &db,
            vec![listing("RTX 3080 + 3090 occasion", "800 €", "https://ex.com/b")],
            &notifier,
            Some(&refiner),
        )
        .await;

        let deals = db.list_deals(None, false).await.unwrap();
        assert_eq!(deals[0].condition.as_deref(), Some("used"), "heuristic wins");
        assert_eq!(deals[0].llm_verdict, Some(LlmVerdict::Genuine));
    }

    #[tokio::test]
    async fn llm_failure_is_fail_open() {
        let (db, notifier) = setup().await;
        let refiner = MockRefiner::failing();
        let stats = run_with(
            &db,
            vec![listing("Riser for 3070 3080 3090", "400 €", "https://ex.com/s")],
            &notifier,
            Some(&refiner),
        )
        .await;

        assert_eq!(stats.refined, 0);
        assert_eq!(refiner.calls(), 1);
        let deals = db.list_deals(None, false).await.unwrap();
        assert_eq!(deals.len(), 1, "deal persisted despite LLM failure");
        assert_eq!(deals[0].llm_verdict, None);
        // no verdict → heuristics gate: the possible-stuffing flag
        // suppresses the push, the match itself is recorded
        assert_eq!(notifier.sent.lock().unwrap().len(), 0);
        assert_eq!(stats.suppressed, 1);
    }

    #[tokio::test]
    async fn llm_calls_go_to_matches_and_ambiguity_only() {
        use ferret_domain::LlmVerdict;
        let (db, notifier) = setup().await;
        let refiner = MockRefiner::returning(crate::llm::Refinement {
            verdict: LlmVerdict::Genuine,
            reason: String::new(),
            capacity_gb: None,
            condition: None,
        });
        run_with(
            &db,
            vec![
                // clean AND matches the watch → one gate call, then notified
                listing("RTX 3080 FE occasion", "450 €", "https://ex.com/1"),
                // no family, no match → never touches the LLM
                listing("4TB IronWolf NAS", "90 €", "https://ex.com/2"),
            ],
            &notifier,
            Some(&refiner),
        )
        .await;
        assert_eq!(refiner.calls(), 1, "one call: the notification gate on the match");
        assert_eq!(notifier.sent.lock().unwrap().len(), 1, "genuine verdict → push goes out");
        let deals = db.list_deals(None, false).await.unwrap();
        let matched: Vec<_> = deals.iter().filter(|d| d.llm_verdict.is_some()).collect();
        assert_eq!(matched.len(), 1, "only the matched deal got a verdict");
    }

    #[tokio::test]
    async fn rescrape_does_not_rerefine() {
        use ferret_domain::LlmVerdict;
        let (db, notifier) = setup().await;
        let refiner = MockRefiner::returning(crate::llm::Refinement {
            verdict: LlmVerdict::StuffedTitle,
            reason: "stuffed".into(),
            capacity_gb: None,
            condition: None,
        });
        let l = listing("Riser for 3070 3080 3090", "400 €", "https://ex.com/s");
        run_with(&db, vec![l.clone()], &notifier, Some(&refiner)).await;
        let stats = run_with(&db, vec![l], &notifier, Some(&refiner)).await;

        assert_eq!(refiner.calls(), 1, "only the first sighting is refined");
        assert_eq!(stats.refined, 0);
        // stored verdict survives the re-scrape
        let deals = db.list_deals(None, false).await.unwrap();
        assert_eq!(deals[0].llm_verdict, Some(LlmVerdict::StuffedTitle));
    }

    #[tokio::test]
    async fn deals_get_categorized_with_spec_values() {
        let (db, notifier) = setup().await;
        db.seed_categories(&crate::seeds::builtin(&[])).await.unwrap();
        run(
            &db,
            vec![listing("Seagate IronWolf 4TB NAS 7200rpm", "90 €", "https://ex.com/hdd")],
            &notifier,
        )
        .await;
        let deals = db.list_deals(None, false).await.unwrap();
        assert_eq!(deals[0].category.as_deref(), Some("hdd"));
        assert_eq!(
            deals[0].specs.get("capacity"),
            Some(&ferret_domain::SpecValue::Number(4000.0))
        );
        assert_eq!(
            deals[0].specs.get("rpm"),
            Some(&ferret_domain::SpecValue::Number(7200.0))
        );
    }

    #[tokio::test]
    async fn unparseable_price_is_skipped() {
        let (db, notifier) = setup().await;
        let stats =
            run(&db, vec![listing("RTX 3080", "Contact seller", "https://ex.com/1")], &notifier)
                .await;
        assert_eq!(stats.skipped, 1);
        assert!(db.list_deals(None, false).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn price_outlier_flagged_after_history_builds() {
        let (db, notifier) = setup().await;
        // seed 5 unambiguous observations around 450 €
        for price in [44_000, 45_000, 46_000, 45_500, 44_500] {
            db.record_price("nvidia-rtx", "3080", price).await.unwrap();
        }
        run(&db, vec![listing("RTX 3080 cheap!!", "100 €", "https://ex.com/scam")], &notifier)
            .await;

        let deals = db.list_deals(None, false).await.unwrap();
        assert!(deals[0].flags.contains(&ferret_domain::Flag::PriceOutlier));
    }
}
