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

    let families = Arc::new(config.families.clone());

    let sources: Vec<(Arc<dyn DealSource>, Duration)> = config
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
    scheduler::spawn_all(
        sources,
        db.clone(),
        families.clone(),
        config.scrape.clone(),
        notifier,
    );

    let app = api::router(state::AppState { db, families })
        .layer(tower_http::trace::TraceLayer::new_for_http());

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
