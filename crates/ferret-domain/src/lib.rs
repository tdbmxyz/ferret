//! ferret domain: pure types and logic — no I/O. Everything here is
//! deterministic and unit-testable without a runtime.

pub mod attributes;
pub mod category;
pub mod deal;
pub mod family;
pub mod listing;
pub mod matching;
pub mod normalize;
pub mod price;
pub mod refine;
pub mod settings;
pub mod status;
pub mod watch;

pub use attributes::ExtractedAttributes;
pub use category::{
    Category, CategoryOrigin, CategorySpec, CategoryStatus, Interpretation, SpecFilter, SpecKind,
    SpecValue,
};

use serde::{Deserialize, Serialize};

/// `GET /api/health` body — the shells' connectivity probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}
pub use deal::{Deal, DealStatus, Flag, LlmVerdict, PricePoint};
pub use family::{FamilyMatch, ProductFamily};
pub use listing::RawListing;
pub use settings::{LlmSettings, LlmSettingsUpdate};
pub use status::{LlmStatus, SearchJob, SourceProgress, SourceStatus, StatusResponse, TickStats};
pub use watch::{Watch, WatchRequest};
