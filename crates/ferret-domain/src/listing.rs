//! Raw scraped listing — the common shape every `DealSource` produces,
//! before any normalization or extraction.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawListing {
    /// Config id of the source that produced this listing.
    pub source_id: String,
    pub title: String,
    /// Raw price text as found on the page, e.g. "1 234,56 €".
    pub price_text: String,
    /// Absolute listing URL (not yet canonicalized).
    pub url: String,
    pub scraped_at: DateTime<Utc>,
}
