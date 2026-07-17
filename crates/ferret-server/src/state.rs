//! Shared application state for the axum handlers.

use std::collections::HashMap;
use std::sync::Arc;

use ferret_domain::{ProductFamily, SourceStatus};
use tokio::sync::RwLock;

use crate::db::Db;
use crate::notify::Notify;

/// Per-source liveness, written by the scheduler, read by `/api/status`.
pub type StatusMap = Arc<RwLock<HashMap<String, SourceStatus>>>;

/// Active watches' search queries, merged into each query-driven source's
/// configured queries at fetch time. Refreshed by the watch API handlers.
pub type SharedQueries = Arc<RwLock<Vec<String>>>;

/// Keep every ACTIVE watch's queries in the scrape rotation (deduped,
/// capped — heavy watch counts must not starve the politeness budget).
pub const MAX_WATCH_QUERIES: usize = 20;

pub async fn refresh_watch_queries(db: &Db, shared: &SharedQueries) -> crate::db::Result<()> {
    let mut queries: Vec<String> = Vec::new();
    for watch in db.list_watches().await? {
        if !watch.active {
            continue;
        }
        for q in watch.queries {
            let q = q.trim().to_lowercase();
            if !q.is_empty() && !queries.contains(&q) {
                queries.push(q);
            }
        }
    }
    queries.truncate(MAX_WATCH_QUERIES);
    *shared.write().await = queries;
    Ok(())
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub families: Arc<Vec<ProductFamily>>,
    pub notifier: Arc<dyn Notify>,
    pub statuses: StatusMap,
    /// Live LLM layer (refiner + interpreter), swapped when settings change.
    pub llm: crate::llm::LlmHandle,
    /// Config context for building one-shot guided-creation searches.
    pub search: Arc<crate::search::SearchContext>,
    pub jobs: crate::search::JobMap,
    pub shared_queries: SharedQueries,
}
