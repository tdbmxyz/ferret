//! ferret domain: pure types and logic — no I/O. Everything here is
//! deterministic and unit-testable without a runtime.

pub mod attributes;
pub mod deal;
pub mod family;
pub mod listing;
pub mod matching;
pub mod normalize;
pub mod price;
pub mod watch;

pub use attributes::ExtractedAttributes;
pub use deal::{Deal, DealStatus, Flag, PricePoint};
pub use family::{FamilyMatch, ProductFamily};
pub use listing::RawListing;
pub use watch::{Watch, WatchRequest};
