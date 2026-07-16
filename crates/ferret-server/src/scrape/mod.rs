//! Deal sources. Two implementation styles behind one trait: the generic
//! declarative engine (config-driven, static HTML) and — later —
//! hand-written plugins for JS-heavy/authenticated sources.

pub mod generic;

use ferret_domain::RawListing;

#[async_trait::async_trait]
pub trait DealSource: Send + Sync {
    /// Stable config id, e.g. "hddboard".
    fn id(&self) -> &str;
    /// Fetch the current listings. Errors are handled by the scheduler
    /// (backoff + failure alert) — implementations just bubble them up.
    async fn fetch(&self) -> anyhow::Result<Vec<RawListing>>;
}
