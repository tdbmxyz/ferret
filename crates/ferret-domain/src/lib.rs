//! ferret domain: pure types and logic — no I/O. Everything here is
//! deterministic and unit-testable without a runtime.

pub mod deal;
pub mod listing;
pub mod watch;

pub use deal::{Deal, Flag};
pub use listing::RawListing;
pub use watch::{Watch, WatchRequest};
