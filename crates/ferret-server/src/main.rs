mod api;
mod config;
mod db;
mod db_category;
mod interpret;
mod llm;
mod notify;
mod pipeline;
mod politeness;
mod scheduler;
mod seeds;
mod scrape;
mod search;
mod state;
mod watches;
mod websearch;

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

    // TOML base + DB override (editable from the UI, applied live)
    let llm_override = llm::load_override(&db).await;
    let llm_runtime = llm::build_runtime(
        llm::effective(&config.llm, llm_override.as_ref()).context("configuring llm")?,
    );
    if llm_runtime.status.enabled {
        tracing::info!(
            base_url = llm_runtime.settings.base_url,
            model = llm_runtime.settings.model,
            from_override = llm_runtime.settings.from_override,
            "llm layer enabled"
        );
    }
    let llm_handle: llm::LlmHandle = Arc::new(tokio::sync::RwLock::new(llm_runtime));

    let families = Arc::new(config.families.clone());

    // live watch queries feeding the query-driven sources
    let shared_queries: state::SharedQueries = Arc::new(tokio::sync::RwLock::new(Vec::new()));
    state::refresh_watch_queries(&db, &shared_queries)
        .await
        .context("loading watch queries")?;
    db.seed_categories(&seeds::builtin(&families))
        .await
        .context("seeding categories")?;

    let mut sources: Vec<(Arc<dyn DealSource>, Duration)> = config
        .sources
        .iter()
        .map(|sc| {
            let client = politeness::scrape_client(
                Duration::from_millis(sc.delay_ms),
                sc.max_concurrency,
            );
            let source: Arc<dyn DealSource> = Arc::new(GenericSource::new(sc.clone(), client, Some(shared_queries.clone())));
            (source, Duration::from_secs(sc.interval_minutes * 60))
        })
        .collect();
    // enabled is enough — watch-driven queries can arrive at runtime
    if config.leboncoin.enabled {
        let lbc = &config.leboncoin;
        let client = politeness::scrape_client(Duration::from_millis(lbc.delay_ms), 1);
        sources.push((
            Arc::new(scrape::leboncoin::LeboncoinSource::new(lbc.clone(), client, Some(shared_queries.clone()))),
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
    if config.ebay.enabled {
        let ebay = &config.ebay;
        let client = politeness::scrape_client(Duration::from_millis(ebay.delay_ms), 1);
        sources.push((
            Arc::new(scrape::ebay::EbaySource::new(ebay.clone(), client, Some(shared_queries.clone()))),
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
        llm_handle.clone(),
        statuses.clone(),
    );

    let search_context = Arc::new(search::SearchContext {
        config: config.clone(),
        families: families.clone(),
        scrape: config.scrape.clone(),
    });
    let mut app = api::router(state::AppState {
        db,
        families,
        notifier: notifier_api,
        statuses: statuses.clone(),
        llm: llm_handle,
        search: search_context,
        jobs: Arc::new(tokio::sync::RwLock::new(Default::default())),
        shared_queries,
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
