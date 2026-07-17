//! Ad-hoc search jobs for guided watch creation: fan the drafted queries
//! out to the query-capable sources once, pipe results through the normal
//! pipeline (they persist as ordinary deals) and report per-source
//! progress. NEVER runs the gone-marking lifecycle — these are partial
//! fetches.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ferret_domain::{ProductFamily, SearchJob, SourceProgress};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::config::{Config, ScrapeConfig};
use crate::db::Db;
use crate::notify::Notify;
use crate::politeness;
use crate::scrape::DealSource;

pub type JobMap = Arc<RwLock<HashMap<Uuid, SearchJob>>>;

/// Everything needed to build one-shot sources for a query list.
pub struct SearchContext {
    pub config: Config,
    pub families: Arc<Vec<ProductFamily>>,
    pub scrape: ScrapeConfig,
}

/// One-shot instances of every query-capable, enabled source, configured
/// with exactly the drafted queries (single page — this is a taste, the
/// scheduled rotation does the deep passes).
pub fn one_shot_sources(context: &SearchContext, queries: &[String]) -> Vec<Arc<dyn DealSource>> {
    let mut sources: Vec<Arc<dyn DealSource>> = Vec::new();
    let queries: Vec<String> = queries.to_vec();

    if context.config.leboncoin.enabled {
        let mut cfg = context.config.leboncoin.clone();
        cfg.queries = queries.clone();
        cfg.pages_per_query = 1;
        let client = politeness::scrape_client(Duration::from_millis(cfg.delay_ms), 1);
        sources.push(Arc::new(crate::scrape::leboncoin::LeboncoinSource::new(cfg, client, None)));
    }
    if context.config.ebay.enabled {
        let mut cfg = context.config.ebay.clone();
        cfg.queries = queries.clone();
        let client = politeness::scrape_client(Duration::from_millis(cfg.delay_ms), 1);
        sources.push(Arc::new(crate::scrape::ebay::EbaySource::new(cfg, client, None)));
    }
    for sc in &context.config.sources {
        if sc.url.contains("{query}") {
            let mut cfg = sc.clone();
            cfg.queries = queries.clone();
            cfg.max_pages = 1;
            let client = politeness::scrape_client(Duration::from_millis(cfg.delay_ms), 1);
            sources.push(Arc::new(crate::scrape::generic::GenericSource::new(cfg, client, None)));
        }
    }
    sources
}

/// Register a job and run every source concurrently. Returns immediately;
/// progress lands in `jobs` (polled by `GET /api/searches/{id}`).
pub async fn spawn_job(
    db: Db,
    context: &SearchContext,
    notifier: Arc<dyn Notify>,
    jobs: JobMap,
    sources: Vec<Arc<dyn DealSource>>,
) -> Uuid {
    let id = Uuid::new_v4();
    let job = SearchJob {
        id,
        sources: sources
            .iter()
            .map(|s| (s.id().to_string(), SourceProgress::Pending))
            .collect(),
        done: sources.is_empty(),
    };
    jobs.write().await.insert(id, job);

    let families = context.families.clone();
    let scrape = context.scrape.clone();
    tokio::spawn(async move {
        let mut handles = Vec::new();
        for source in sources {
            let db = db.clone();
            let families = families.clone();
            let scrape = scrape.clone();
            let notifier = notifier.clone();
            let jobs = jobs.clone();
            handles.push(tokio::spawn(async move {
                let source_id = source.id().to_string();
                let progress = match source.fetch().await {
                    Ok(listings) => {
                        let count = listings.len() as u64;
                        match crate::pipeline::process_listings(
                            &db,
                            &families,
                            &scrape,
                            &source_id,
                            listings,
                            notifier.as_ref(),
                            None,  // no LLM refinement on taste fetches
                            false, // PARTIAL fetch: never mark deals gone
                        )
                        .await
                        {
                            Ok(_) => SourceProgress::Done { listings: count },
                            Err(e) => SourceProgress::Error { message: e.to_string() },
                        }
                    }
                    Err(e) => SourceProgress::Error { message: e.to_string() },
                };
                if let Some(job) = jobs.write().await.get_mut(&id) {
                    job.sources.insert(source_id, progress);
                }
            }));
        }
        for handle in handles {
            let _ = handle.await;
        }
        if let Some(job) = jobs.write().await.get_mut(&id) {
            job.done = true;
        }
    });
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use chrono::Utc;
    use ferret_domain::RawListing;

    struct MockSource {
        id: &'static str,
        result: std::result::Result<Vec<RawListing>, String>,
    }

    #[async_trait::async_trait]
    impl DealSource for MockSource {
        fn id(&self) -> &str {
            self.id
        }
        async fn fetch(&self) -> anyhow::Result<Vec<RawListing>> {
            match &self.result {
                Ok(l) => Ok(l.clone()),
                Err(e) => Err(anyhow::anyhow!("{e}")),
            }
        }
    }

    fn listing(url: &str) -> RawListing {
        RawListing {
            source_id: "mock-ok".into(),
            title: "RTX 3080 FE".into(),
            price_text: "450 €".into(),
            url: url.into(),
            scraped_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn job_runs_sources_reports_progress_and_skips_lifecycle() {
        let db = Db::connect(Path::new(":memory:")).await.unwrap();
        // pre-existing deal from the same source, NOT in the search results —
        // must stay active (partial fetch never marks gone)
        crate::pipeline::process_listings(
            &db,
            &[],
            &crate::config::ScrapeConfig::default(),
            "mock-ok",
            vec![listing("https://ex.com/old")],
            &crate::notify::NoopNotifier,
            None,
            true,
        )
        .await
        .unwrap();

        let jobs: JobMap = Arc::new(RwLock::new(HashMap::new()));
        let context = SearchContext {
            config: crate::config::Config::default(),
            families: Arc::new(vec![]),
            scrape: Default::default(),
        };
        let sources: Vec<Arc<dyn DealSource>> = vec![
            Arc::new(MockSource {
                id: "mock-ok",
                result: Ok(vec![listing("https://ex.com/new")]),
            }),
            Arc::new(MockSource { id: "mock-bad", result: Err("blocked".into()) }),
        ];
        let id = spawn_job(
            db.clone(),
            &context,
            Arc::new(crate::notify::NoopNotifier),
            jobs.clone(),
            sources,
        )
        .await;

        // wait for completion
        for _ in 0..100 {
            if jobs.read().await.get(&id).unwrap().done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let job = jobs.read().await.get(&id).unwrap().clone();
        assert!(job.done);
        assert_eq!(job.sources["mock-ok"], SourceProgress::Done { listings: 1 });
        assert!(matches!(job.sources["mock-bad"], SourceProgress::Error { .. }));

        // both deals exist and BOTH are active (no gone-marking)
        let deals = db.list_deals(None).await.unwrap();
        assert_eq!(deals.len(), 2);
        assert!(deals.iter().all(|d| d.status == ferret_domain::DealStatus::Active));
    }

    #[test]
    fn one_shot_sources_respect_enabled_flags() {
        let mut config = crate::config::Config::default();
        let context = SearchContext {
            config: config.clone(),
            families: Arc::new(vec![]),
            scrape: Default::default(),
        };
        assert!(one_shot_sources(&context, &["x".into()]).is_empty(), "nothing enabled");

        config.leboncoin.enabled = true;
        let context = SearchContext {
            config,
            families: Arc::new(vec![]),
            scrape: Default::default(),
        };
        let sources = one_shot_sources(&context, &["x".into()]);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].id(), "leboncoin");
    }
}
