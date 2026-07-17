//! Watch-side actions that need more than storage: retro-matching a saved
//! watch against the deals we already have, with a single summary
//! notification — the immediate "your search is live" feedback.

use ferret_domain::{DealStatus, Watch, matching};

use crate::db::Db;
use crate::notify::Notify;

/// Match `watch` against every ACTIVE stored deal. New matches are marked
/// notified at the deal's current price — that arms the price-drop
/// re-notification without pushing N historical deals — and ONE summary
/// notification reports the result. Returns (matched now, best price).
pub async fn retro_match(
    db: &Db,
    notifier: &dyn Notify,
    watch: &Watch,
    action: &str, // "created" | "updated"
) -> anyhow::Result<(u64, Option<i64>)> {
    let mut matched = 0u64;
    let mut best: Option<(i64, String)> = None;
    if watch.active {
        for deal in db.list_deals(None).await? {
            if deal.status != DealStatus::Active || !matching::watch_matches(watch, &deal) {
                continue;
            }
            if db.insert_match(deal.id, watch.id).await? {
                db.mark_notified(deal.id, watch.id, deal.price_cents).await?;
            }
            matched += 1;
            if best.as_ref().is_none_or(|(p, _)| deal.price_cents < *p) {
                best = Some((deal.price_cents, deal.currency.clone()));
            }
        }
    }

    let message = match &best {
        Some((price, currency)) => format!(
            "{matched} existing deal{} match — best {:.2} {currency}.\nNew finds will be pushed here.",
            if matched == 1 { "" } else { "s" },
            *price as f64 / 100.0,
        ),
        None if watch.active => {
            "No existing deals match yet — sources will pick it up on their next pass.".to_string()
        }
        None => "Watch is paused — no notifications until resumed.".to_string(),
    };
    notifier
        .send(
            &format!("ferret: watch '{}' {action}", watch.name),
            &message,
            "mag,ferret",
            "default",
        )
        .await;
    Ok((matched, best.map(|(p, _)| p)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    use chrono::Utc;
    use ferret_domain::{Deal, Flag, WatchRequest};
    use uuid::Uuid;

    #[derive(Default)]
    struct RecordingNotifier {
        sent: Mutex<Vec<(String, String)>>, // (title, message)
    }

    #[async_trait::async_trait]
    impl Notify for RecordingNotifier {
        async fn send(&self, title: &str, message: &str, _tags: &str, _priority: &str) {
            self.sent.lock().unwrap().push((title.into(), message.into()));
        }
    }

    fn deal(url: &str, price: i64) -> Deal {
        Deal {
            id: Uuid::new_v4(),
            source_id: "src".into(),
            canonical_url: url.into(),
            title: "RTX 3080".into(),
            price_cents: price,
            currency: "EUR".into(),
            family: Some("nvidia-rtx".into()),
            models: vec!["3080".into()],
            capacity_gb: None,
            condition: None,
            stuffing_score: 0.0,
            flags: Vec::<Flag>::new(),
            status: DealStatus::Active,
            llm_verdict: None,
            llm_reason: None,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
        }
    }

    #[tokio::test]
    async fn retro_match_populates_matches_and_sends_one_summary() {
        let db = Db::connect(Path::new(":memory:")).await.unwrap();
        db.upsert_deal(&deal("https://ex.com/1", 45_000)).await.unwrap();
        db.upsert_deal(&deal("https://ex.com/2", 42_000)).await.unwrap();
        // a gone deal must not count
        let (gone, _) = db.upsert_deal(&deal("https://ex.com/3", 30_000)).await.unwrap();
        db.mark_gone("src", &["https://ex.com/1".into(), "https://ex.com/2".into()].into())
            .await
            .unwrap();
        assert!(gone.id != Uuid::nil());

        let notifier = RecordingNotifier::default();
        let watch = db
            .create_watch(&WatchRequest {
                name: "RTX 3080".into(),
                family: Some("nvidia-rtx".into()),
                model: Some("3080".into()),
                min_capacity_gb: None,
                min_price_cents: None,
                max_price_cents: Some(50_000),
                active: true,
            })
            .await
            .unwrap();

        let (matched, best) = retro_match(&db, &notifier, &watch, "created").await.unwrap();
        assert_eq!(matched, 2, "gone deal excluded");
        assert_eq!(best, Some(42_000));

        // matches visible immediately
        assert_eq!(db.list_deals(Some(watch.id)).await.unwrap().len(), 2);
        // exactly ONE summary push, mentioning count and best price
        {
            let sent = notifier.sent.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert!(sent[0].1.contains("2 existing deals match"), "{}", sent[0].1);
            assert!(sent[0].1.contains("420.00 EUR"), "{}", sent[0].1);
        }

        // notified price is armed → later drop can re-notify
        let deals = db.list_deals(Some(watch.id)).await.unwrap();
        for d in &deals {
            assert!(db.notified_price(d.id, watch.id).await.unwrap().is_some());
        }
    }

    #[tokio::test]
    async fn retro_match_empty_db_still_acknowledges() {
        let db = Db::connect(Path::new(":memory:")).await.unwrap();
        let notifier = RecordingNotifier::default();
        let watch = db
            .create_watch(&WatchRequest {
                name: "4TB HDD".into(),
                family: None,
                model: None,
                min_capacity_gb: Some(4000),
                min_price_cents: None,
                max_price_cents: None,
                active: true,
            })
            .await
            .unwrap();
        let (matched, best) = retro_match(&db, &notifier, &watch, "created").await.unwrap();
        assert_eq!((matched, best), (0, None));
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].1.contains("No existing deals match yet"));
    }
}
