//! The ETL pipeline: raw listings → normalize → extract → score → dedupe →
//! watch match → persist → notify. Pure logic lives in ferret-domain; this
//! module only sequences it and talks to storage/notifier.

use chrono::Utc;
use ferret_domain::{
    Deal, Flag, ProductFamily, RawListing, attributes, family, matching, normalize, price,
};
use uuid::Uuid;

use crate::config::ScrapeConfig;
use crate::db::Db;
use crate::notify::Notify;

/// How many recent observations feed the rolling median.
const PRICE_WINDOW: u32 = 50;

#[derive(Debug, Default, PartialEq)]
pub struct PipelineStats {
    pub new_deals: u64,
    pub updated_deals: u64,
    pub skipped: u64,
    pub notified: u64,
}

/// Process one batch of raw listings from a single scheduler tick.
pub async fn process_listings(
    db: &Db,
    families: &[ProductFamily],
    scrape: &ScrapeConfig,
    listings: Vec<RawListing>,
    notifier: &dyn Notify,
) -> anyhow::Result<PipelineStats> {
    let mut stats = PipelineStats::default();
    let watches = db.list_watches().await?;

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

        // -- extract + score --
        let attrs = attributes::extract(&title);
        let fam = family::match_families(&title, families);

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
            first_seen: now,
            last_seen: now,
        };

        // -- dedupe / persist --
        let (stored, was_new) = db.upsert_deal(&deal).await?;
        if was_new {
            stats.new_deals += 1;
        } else {
            stats.updated_deals += 1;
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
        for watch in &watches {
            if !matching::watch_matches(watch, &stored) {
                continue;
            }
            let fresh_match = db.insert_match(stored.id, watch.id).await?;
            if !fresh_match {
                continue;
            }
            let mut tags: Vec<String> = vec!["moneybag".into()];
            tags.extend(stored.flags.iter().map(|f| {
                serde_json::to_string(f).expect("flag serializes").trim_matches('"').to_string()
            }));
            let price_major = stored.price_cents as f64 / 100.0;
            notifier
                .send(
                    &format!("{}: {:.2} {}", watch.name, price_major, stored.currency),
                    &format!("{}\n{}", stored.title, stored.canonical_url),
                    &tags.join(","),
                    "default",
                )
                .await;
            db.mark_notified(stored.id, watch.id).await?;
            stats.notified += 1;
        }
    }
    Ok(stats)
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
        }]
    }

    async fn setup() -> (Db, RecordingNotifier) {
        let db = Db::connect(Path::new(":memory:")).await.unwrap();
        db.create_watch(&WatchRequest {
            name: "RTX 3080".into(),
            family: Some("nvidia-rtx".into()),
            model: Some("3080".into()),
            min_capacity_gb: None,
            max_price_cents: Some(50_000),
            active: true,
        })
        .await
        .unwrap();
        (db, RecordingNotifier::default())
    }

    #[tokio::test]
    async fn matching_listing_is_persisted_and_notified() {
        let (db, notifier) = setup().await;
        let stats = process_listings(
            &db,
            &families(),
            &ScrapeConfig::default(),
            vec![listing("RTX 3080 FE occasion", "450 €", "https://ex.com/1?utm_source=x")],
            &notifier,
        )
        .await
        .unwrap();

        assert_eq!(stats.new_deals, 1);
        assert_eq!(stats.notified, 1);

        let deals = db.list_deals(None).await.unwrap();
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
        process_listings(&db, &families(), &ScrapeConfig::default(), vec![l.clone()], &notifier)
            .await
            .unwrap();
        let stats =
            process_listings(&db, &families(), &ScrapeConfig::default(), vec![l], &notifier)
                .await
                .unwrap();

        assert_eq!(stats.new_deals, 0);
        assert_eq!(stats.updated_deals, 1);
        assert_eq!(stats.notified, 0, "same deal, no second push");
        assert_eq!(notifier.sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn stuffed_listing_matches_with_flag() {
        let (db, notifier) = setup().await;
        process_listings(
            &db,
            &families(),
            &ScrapeConfig::default(),
            vec![listing(
                "Brackets for 3070 3080 3090 4080 4090",
                "400 €",
                "https://ex.com/stuffed",
            )],
            &notifier,
        )
        .await
        .unwrap();

        let deals = db.list_deals(None).await.unwrap();
        assert_eq!(deals.len(), 1);
        assert!(deals[0].flags.contains(&ferret_domain::Flag::PossibleStuffing));
        // still notified — stuffing is a signal, not a filter
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].1.contains("possible-stuffing"), "flag rides in the tags");
    }

    #[tokio::test]
    async fn unparseable_price_is_skipped() {
        let (db, notifier) = setup().await;
        let stats = process_listings(
            &db,
            &families(),
            &ScrapeConfig::default(),
            vec![listing("RTX 3080", "Contact seller", "https://ex.com/1")],
            &notifier,
        )
        .await
        .unwrap();
        assert_eq!(stats.skipped, 1);
        assert!(db.list_deals(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn price_outlier_flagged_after_history_builds() {
        let (db, notifier) = setup().await;
        // seed 5 unambiguous observations around 450 €
        for price in [44_000, 45_000, 46_000, 45_500, 44_500] {
            db.record_price("nvidia-rtx", "3080", price).await.unwrap();
        }
        process_listings(
            &db,
            &families(),
            &ScrapeConfig::default(),
            vec![listing("RTX 3080 cheap!!", "100 €", "https://ex.com/scam")],
            &notifier,
        )
        .await
        .unwrap();

        let deals = db.list_deals(None).await.unwrap();
        assert!(deals[0].flags.contains(&ferret_domain::Flag::PriceOutlier));
    }
}
