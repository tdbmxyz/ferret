//! Per-source scheduling: each source runs on its own tokio task and
//! interval — a failing source backs off and alerts, and never blocks or
//! delays other sources.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use ferret_domain::{ProductFamily, SourceStatus, TickStats};

use crate::config::ScrapeConfig;
use crate::db::Db;
use crate::llm::LlmRefiner;
use crate::notify::Notify;
use crate::pipeline;
use crate::scrape::DealSource;
use crate::state::StatusMap;

const BACKOFF_BASE: Duration = Duration::from_secs(60);
const BACKOFF_CAP: Duration = Duration::from_secs(3600);

/// Consecutive-failure accounting for one source: exponential backoff and
/// a single ntfy alert per outage (re-armed on recovery).
pub struct FailureState {
    consecutive: u32,
    alert_after: u32,
    alerted: bool,
}

impl FailureState {
    pub fn new(alert_after: u32) -> Self {
        Self { consecutive: 0, alert_after, alerted: false }
    }

    /// Record a failure; returns how long to back off before the retry.
    pub fn record_failure(&mut self) -> Duration {
        self.consecutive = self.consecutive.saturating_add(1);
        let factor = 2u32.saturating_pow(self.consecutive.saturating_sub(1).min(6));
        (BACKOFF_BASE * factor).min(BACKOFF_CAP)
    }

    /// True exactly once per outage, when the threshold is crossed.
    pub fn should_alert(&mut self) -> bool {
        if !self.alerted && self.consecutive >= self.alert_after {
            self.alerted = true;
            return true;
        }
        false
    }

    pub fn record_success(&mut self) {
        self.consecutive = 0;
        self.alerted = false;
    }
}

/// Spawn one scraping loop per source. Loops run until the process exits
/// (tasks are detached; the axum server owns process lifetime).
pub fn spawn_all(
    sources: Vec<(Arc<dyn DealSource>, Duration)>,
    db: Db,
    families: Arc<Vec<ProductFamily>>,
    scrape: ScrapeConfig,
    notifier: Arc<dyn Notify>,
    refiner: Option<Arc<dyn LlmRefiner>>,
    statuses: StatusMap,
) {
    for (source, interval) in sources {
        let db = db.clone();
        let families = families.clone();
        let scrape = scrape.clone();
        let notifier = notifier.clone();
        let refiner = refiner.clone();
        let statuses = statuses.clone();
        tokio::spawn(async move {
            // register as idle right away so /api/status lists the source
            // before its first tick completes
            statuses.write().await.insert(
                source.id().to_string(),
                SourceStatus::idle(source.id(), interval.as_secs() / 60),
            );
            run_source(source, interval, db, families, scrape, notifier, refiner, statuses).await;
        });
    }
}

async fn record_tick(statuses: &StatusMap, source_id: &str, result: Result<TickStats, String>) {
    let mut map = statuses.write().await;
    if let Some(status) = map.get_mut(source_id) {
        status.last_tick = Some(Utc::now());
        match result {
            Ok(stats) => {
                status.last_stats = Some(stats);
                status.last_error = None;
                status.consecutive_failures = 0;
            }
            Err(error) => {
                status.last_error = Some(error);
                status.consecutive_failures += 1;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_source(
    source: Arc<dyn DealSource>,
    interval: Duration,
    db: Db,
    families: Arc<Vec<ProductFamily>>,
    scrape: ScrapeConfig,
    notifier: Arc<dyn Notify>,
    refiner: Option<Arc<dyn LlmRefiner>>,
    statuses: StatusMap,
) {
    let mut failures = FailureState::new(scrape.failure_alert_after);
    loop {
        match source.fetch().await {
            Ok(listings) => {
                let count = listings.len();
                match pipeline::process_listings(
                    &db,
                    &families,
                    &scrape,
                    source.id(),
                    listings,
                    notifier.as_ref(),
                    refiner.as_deref(),
                )
                .await
                {
                    Ok(stats) => {
                        failures.record_success();
                        tracing::info!(
                            source = source.id(),
                            fetched = count,
                            new = stats.new_deals,
                            updated = stats.updated_deals,
                            skipped = stats.skipped,
                            notified = stats.notified,
                            gone = stats.gone,
                            refined = stats.refined,
                            "tick done"
                        );
                        record_tick(
                            &statuses,
                            source.id(),
                            Ok(TickStats {
                                fetched: count as u64,
                                new_deals: stats.new_deals,
                                updated_deals: stats.updated_deals,
                                skipped: stats.skipped,
                                notified: stats.notified,
                                gone: stats.gone,
                                refined: stats.refined,
                            }),
                        )
                        .await;
                    }
                    Err(e) => {
                        // pipeline (db) errors also count as failures
                        let backoff = failures.record_failure();
                        tracing::error!(source = source.id(), error = %e, ?backoff, "pipeline failed");
                        record_tick(&statuses, source.id(), Err(e.to_string())).await;
                        maybe_alert(&mut failures, source.id(), &e, notifier.as_ref()).await;
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                }
            }
            Err(e) => {
                let backoff = failures.record_failure();
                tracing::warn!(source = source.id(), error = %e, ?backoff, "fetch failed");
                record_tick(&statuses, source.id(), Err(e.to_string())).await;
                maybe_alert(&mut failures, source.id(), &e, notifier.as_ref()).await;
                tokio::time::sleep(backoff).await;
                continue;
            }
        }
        tokio::time::sleep(interval).await;
    }
}

async fn maybe_alert(
    failures: &mut FailureState,
    source_id: &str,
    error: &anyhow::Error,
    notifier: &dyn Notify,
) {
    if failures.should_alert() {
        notifier
            .send(
                &format!("ferret: source {source_id} is failing"),
                &format!("Repeated scrape failures, backing off.\nLast error: {error}"),
                "warning,ferret",
                "high",
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn backoff_grows_and_caps() {
        let mut fs = FailureState::new(3);
        assert_eq!(fs.record_failure(), Duration::from_secs(60));
        assert_eq!(fs.record_failure(), Duration::from_secs(120));
        assert_eq!(fs.record_failure(), Duration::from_secs(240));
        for _ in 0..10 {
            fs.record_failure();
        }
        assert_eq!(fs.record_failure(), Duration::from_secs(3600), "capped at 1h");
    }

    #[test]
    fn alerts_once_at_threshold() {
        let mut fs = FailureState::new(3);
        fs.record_failure();
        fs.record_failure();
        assert!(!fs.should_alert());
        fs.record_failure();
        assert!(fs.should_alert(), "alert at the threshold");
        assert!(!fs.should_alert(), "but only once");
        fs.record_failure();
        assert!(!fs.should_alert(), "still silent while failing");
    }

    #[test]
    fn success_resets_everything() {
        let mut fs = FailureState::new(2);
        fs.record_failure();
        fs.record_failure();
        assert!(fs.should_alert());
        fs.record_success();
        assert_eq!(fs.record_failure(), Duration::from_secs(60), "backoff reset");
        fs.record_failure();
        assert!(fs.should_alert(), "alert re-armed after recovery");
    }
}
