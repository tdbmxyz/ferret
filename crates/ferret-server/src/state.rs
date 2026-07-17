//! Shared application state for the axum handlers.

use std::collections::HashMap;
use std::sync::Arc;

use ferret_domain::{ProductFamily, SourceStatus};
use tokio::sync::RwLock;

use crate::db::Db;
use crate::notify::Notify;

/// Per-source liveness, written by the scheduler, read by `/api/status`.
pub type StatusMap = Arc<RwLock<HashMap<String, SourceStatus>>>;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub families: Arc<Vec<ProductFamily>>,
    pub notifier: Arc<dyn Notify>,
    pub statuses: StatusMap,
}
