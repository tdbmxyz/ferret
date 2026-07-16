//! Shared application state for the axum handlers.

use std::sync::Arc;

use ferret_domain::ProductFamily;

use crate::db::Db;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub families: Arc<Vec<ProductFamily>>,
}
