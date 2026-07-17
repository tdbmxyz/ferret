mod api;
mod config;
mod db;
mod llm;
mod notify;
mod pipeline;
mod politeness;
mod scheduler;
mod scrape;
mod state;
mod watches;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

use crate::notify::{NoopNotifier, Notify, NtfyNotifier};
use crate::scrape::DealSource;
use crate::scrape::generic::GenericSource;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = config::load().context("loading configuration")?;
    tracing::info!(
        sources = config.sources.len(),
        families = config.families.len(),
        "configuration loaded"
    );

    let db = db::Db::connect(&config.db_path)
        .await
        .with_context(|| format!("opening database {}", config.db_path.display()))?;

    let notifier: Arc<dyn Notify> = match NtfyNotifier::new(&config.notifications)
        .context("configuring ntfy notifier")?
    {
        Some(n) => {
            tracing::info!("ntfy notifications enabled");
            Arc::new(n)
        }
        None => Arc::new(NoopNotifier),
    };

    let refiner: Option<Arc<dyn llm::LlmRefiner>> = llm::OpenAiRefiner::new(&config.llm)
        .context("configuring llm refiner")?
        .map(|r| {
            tracing::info!(base_url = config.llm.base_url, "llm refinement enabled");
            Arc::new(r) as Arc<dyn llm::LlmRefiner>
        });

    let families = Arc::new(config.families.clone());

    let mut sources: Vec<(Arc<dyn DealSource>, Duration)> = config
        .sources
        .iter()
        .map(|sc| {
            let client = politeness::scrape_client(
                Duration::from_millis(sc.delay_ms),
                sc.max_concurrency,
            );
            let source: Arc<dyn DealSource> = Arc::new(GenericSource::new(sc.clone(), client));
            (source, Duration::from_secs(sc.interval_minutes * 60))
        })
        .collect();
    if config.leboncoin.enabled && !config.leboncoin.queries.is_empty() {
        let lbc = &config.leboncoin;
        let client = politeness::scrape_client(Duration::from_millis(lbc.delay_ms), 1);
        sources.push((
            Arc::new(scrape::leboncoin::LeboncoinSource::new(lbc.clone(), client)),
            Duration::from_secs(lbc.interval_minutes * 60),
        ));
    }
    for shop in &config.shopify {
        let client = politeness::scrape_client(Duration::from_millis(shop.delay_ms), 1);
        sources.push((
            Arc::new(scrape::shopify::ShopifySource::new(shop.clone(), client)),
            Duration::from_secs(shop.interval_minutes * 60),
        ));
    }
    if config.ebay.enabled && !config.ebay.queries.is_empty() {
        let ebay = &config.ebay;
        let client = politeness::scrape_client(Duration::from_millis(ebay.delay_ms), 1);
        sources.push((
            Arc::new(scrape::ebay::EbaySource::new(ebay.clone(), client)),
            Duration::from_secs(ebay.interval_minutes * 60),
        ));
    }
    let statuses: state::StatusMap = Arc::new(tokio::sync::RwLock::new(Default::default()));
    let notifier_api = notifier.clone();
    scheduler::spawn_all(
        sources,
        db.clone(),
        families.clone(),
        config.scrape.clone(),
        notifier,
        refiner,
        statuses.clone(),
    );

    let mut app = api::router(state::AppState {
        db,
        families,
        notifier: notifier_api,
        statuses: statuses.clone(),
    })
        // The Tauri webview is a foreign origin and the trust model is
        // LAN/tailnet single-user with no cookies — permissive CORS is fine.
        .layer(tower_http::cors::CorsLayer::permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http());
    if let Some(dir) = &config.static_dir {
        let index = dir.join("index.html");
        app = app.fallback_service(
            tower_http::services::ServeDir::new(dir)
                .fallback(tower_http::services::ServeFile::new(index)),
        );
        tracing::info!(dir = %dir.display(), "serving web frontend");
    }

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding {}", config.listen))?;
    tracing::info!(listen = %config.listen, "ferret-server up");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("server error")
}
