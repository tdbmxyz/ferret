# ferret Core Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the ferret core backend: a Rust workspace with `ferret-domain` (pure matching/scoring/extraction logic) and `ferret-server` (axum API, declarative scraper, tower politeness layer, sqlx/SQLite storage, scheduler, ntfy notifier).

**Architecture:** Two crates. `ferret-domain` holds all pure functions (price parsing, URL canonicalization, attribute extraction, family/stuffing scoring, outlier detection, watch matching) — exhaustively unit-tested, zero I/O. `ferret-server` wires them into an ETL pipeline: scheduler → `DealSource::fetch()` → normalize → extract → score → dedupe/upsert → watch match → ntfy notify, plus a REST API for watches/deals. Conventions copied from the sibling `chaos` project (figment TOML config, sqlx migrations, best-effort ntfy notifier, edition 2024 workspace).

**Tech Stack:** Rust edition 2024, axum 0.8, tokio, tower, sqlx 0.8 (SQLite), reqwest 0.12 (rustls), scraper 0.23, figment, regex, serde, chrono, uuid, thiserror, anyhow.

**Out of scope for this plan** (follow-up plans): LLM refinement pass, `ferret-client`/`ferret-ui`/`ferret-web`/`ferret-desktop`, chromiumoxide hand-written sources, NixOS module. The `[llm]` config block and hand-written `DealSource` impls slot in later without schema changes.

---

## File structure

```
ferret/
  Cargo.toml                         # workspace (Task 1)
  rust-toolchain.toml                # stable + rustfmt/clippy (Task 1)
  .gitignore                         # target/, *.db (Task 1)
  crates/
    ferret-domain/
      Cargo.toml
      src/
        lib.rs                       # module decls + re-exports
        listing.rs                   # RawListing (Task 2)
        deal.rs                      # Deal, Flag (Task 2)
        watch.rs                     # Watch + API DTOs (Task 2)
        family.rs                    # ProductFamily, match_models, stuffing_score (Task 5)
        normalize.rs                 # parse_price, canonical_url, clean_title (Task 3)
        attributes.rs                # ExtractedAttributes, extract() (Task 4)
        price.rs                     # rolling median + is_outlier (Task 6)
        matching.rs                  # watch_matches() (Task 7)
    ferret-server/
      Cargo.toml
      ferret.example.toml            # documented example config (Task 8)
      migrations/
        0001_init.sql                # deals, watches, deal_matches, price_history (Task 9)
      src/
        main.rs                      # wiring (Task 15)
        config.rs                    # figment Config (Task 8)
        db.rs                        # Db: pool, migrations, repos (Task 9)
        politeness.rs                # tower PolitenessLayer + HttpService (Task 10)
        scrape/
          mod.rs                     # DealSource trait (Task 11)
          generic.rs                 # declarative scraper: parse_listings + GenericSource (Task 11)
        notify.rs                    # Notify trait + NtfyNotifier (Task 12)
        pipeline.rs                  # process_listings ETL (Task 13)
        scheduler.rs                 # per-source interval loop + backoff + failure alert (Task 14)
        api.rs                       # axum router: watches CRUD, deals list (Task 15)
        state.rs                     # AppState (Task 15)
      tests/
        fixtures/
          example_board.html         # scraper fixture (Task 11)
        pipeline.rs                  # end-to-end ETL integration test (Task 13)
```

Money is always integer cents (`i64`) + a currency string. Capacity is always decimal gigabytes (`i64`, 1 TB = 1000 GB). Timestamps are `chrono::DateTime<Utc>`, stored as RFC3339 TEXT. UUIDs stored as hyphenated TEXT. Flags stored as a JSON array TEXT column. These conventions match `chaos`.

---

### Task 1: Workspace scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `.gitignore`
- Create: `crates/ferret-domain/Cargo.toml`
- Create: `crates/ferret-domain/src/lib.rs`
- Create: `crates/ferret-server/Cargo.toml`
- Create: `crates/ferret-server/src/main.rs`

- [ ] **Step 1: Write workspace `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = ["crates/ferret-domain", "crates/ferret-server"]

[workspace.package]
version = "0.1.0"
edition = "2024"
authors = ["Tibo <thibaudbalem@gmail.com>"]
repository = "https://github.com/tdbmxyz/ferret"

[workspace.dependencies]
# internal crates
ferret-domain = { path = "crates/ferret-domain" }

# shared
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["serde", "v4"] }
url = { version = "2", features = ["serde"] }
thiserror = "2"
regex = "1"

# server
anyhow = "1"
futures = "0.3"
axum = "0.8"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync", "time"] }
tower = { version = "0.5", features = ["util"] }
tower-http = { version = "0.6", features = ["trace", "cors"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
figment = { version = "0.10", features = ["toml", "env"] }
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "sqlite", "chrono", "migrate", "derive", "macros"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "gzip"] }
scraper = "0.23"
async-trait = "0.1"

[profile.release]
lto = "thin"
strip = true
```

- [ ] **Step 2: Write `rust-toolchain.toml`**

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy", "rust-src", "rust-analyzer"]
profile = "default"
```

(No wasm/Android targets yet — those arrive with the frontend plan.)

- [ ] **Step 3: Write `.gitignore`**

```gitignore
/target
*.db
*.db-shm
*.db-wal
ferret.toml
```

- [ ] **Step 4: Write `crates/ferret-domain/Cargo.toml`**

```toml
[package]
name = "ferret-domain"
description = "Shared domain types, extraction and matching logic for ferret"
version.workspace = true
edition.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
uuid.workspace = true
url.workspace = true
regex.workspace = true

[dev-dependencies]
```

- [ ] **Step 5: Write `crates/ferret-domain/src/lib.rs`**

```rust
//! ferret domain: pure types and logic — no I/O. Everything here is
//! deterministic and unit-testable without a runtime.
```

- [ ] **Step 6: Write `crates/ferret-server/Cargo.toml`**

```toml
[package]
name = "ferret-server"
description = "ferret server: scraper, ETL pipeline, storage, REST API"
version.workspace = true
edition.workspace = true

[dependencies]
ferret-domain.workspace = true
anyhow.workspace = true
async-trait.workspace = true
axum.workspace = true
chrono.workspace = true
figment.workspace = true
futures.workspace = true
regex.workspace = true
reqwest.workspace = true
scraper.workspace = true
serde.workspace = true
serde_json.workspace = true
sqlx.workspace = true
thiserror.workspace = true
tokio.workspace = true
tower.workspace = true
tower-http.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
url.workspace = true
uuid.workspace = true

[dev-dependencies]
```

- [ ] **Step 7: Write `crates/ferret-server/src/main.rs`**

```rust
fn main() {
    println!("ferret-server");
}
```

- [ ] **Step 8: Verify the workspace builds**

Run: `cargo build --workspace`
Expected: compiles with no errors (warnings about unused deps are fine at this stage; sqlx/axum etc. are used from Task 8 on).

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml rust-toolchain.toml .gitignore crates
git commit -m "chore: scaffold ferret workspace (domain + server crates)"
```

---

### Task 2: Domain core types

**Files:**
- Create: `crates/ferret-domain/src/listing.rs`
- Create: `crates/ferret-domain/src/deal.rs`
- Create: `crates/ferret-domain/src/watch.rs`
- Modify: `crates/ferret-domain/src/lib.rs`

These are plain data types; the "test" is a serde round-trip covering the API contract (the client crate will rely on these field names later).

- [ ] **Step 1: Write the failing test in `crates/ferret-domain/src/watch.rs` (types referenced don't exist yet)**

Create the three files with types AND tests together — for pure data types the round-trip test is written against the type in the same step, then verified.

`crates/ferret-domain/src/listing.rs`:

```rust
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
```

`crates/ferret-domain/src/deal.rs`:

```rust
//! A persisted deal: a normalized, attribute-extracted, scored listing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Heuristic warning flags attached to a deal. Signals, never hard
/// filters — a flagged deal still surfaces, tagged for the user to eyeball.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Flag {
    /// Title enumerates several sibling models (SEO stuffing).
    PossibleStuffing,
    /// Price is far below the rolling median for this family+model.
    PriceOutlier,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deal {
    pub id: Uuid,
    pub source_id: String,
    pub canonical_url: String,
    pub title: String,
    pub price_cents: i64,
    /// ISO currency code, e.g. "EUR".
    pub currency: String,
    /// Product family the title matched (config-driven tables), if any.
    pub family: Option<String>,
    /// All family models found in the title. One entry = unambiguous.
    pub models: Vec<String>,
    /// Decimal gigabytes (1 TB = 1000 GB) when a capacity was extracted.
    pub capacity_gb: Option<i64>,
    /// "new" | "used" | "refurbished" when detected.
    pub condition: Option<String>,
    /// 0.0 = single model mentioned; → 1.0 as the title enumerates the
    /// whole family.
    pub stuffing_score: f64,
    pub flags: Vec<Flag>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}
```

`crates/ferret-domain/src/watch.rs`:

```rust
//! Saved product-type watches and their API request/response shapes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub id: Uuid,
    /// Display name, e.g. "4TB HDD" or "RTX 3080".
    pub name: String,
    /// Product family the watch targets (must exist in the family tables
    /// for stuffing/outlier signals; free-text watches still match on the
    /// other filters).
    pub family: Option<String>,
    /// Exact model within the family, e.g. "3080".
    pub model: Option<String>,
    /// Minimum extracted capacity in decimal GB.
    pub min_capacity_gb: Option<i64>,
    pub max_price_cents: Option<i64>,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchRequest {
    pub name: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub min_capacity_gb: Option<i64>,
    #[serde(default)]
    pub max_price_cents: Option<i64>,
    #[serde(default = "default_true")]
    pub active: bool,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deal::Flag;

    #[test]
    fn flag_serializes_kebab_case() {
        assert_eq!(
            serde_json::to_string(&Flag::PossibleStuffing).unwrap(),
            "\"possible-stuffing\""
        );
        assert_eq!(
            serde_json::to_string(&Flag::PriceOutlier).unwrap(),
            "\"price-outlier\""
        );
    }

    #[test]
    fn watch_request_defaults() {
        let req: WatchRequest = serde_json::from_str(r#"{"name": "4TB HDD"}"#).unwrap();
        assert_eq!(req.name, "4TB HDD");
        assert!(req.active);
        assert!(req.family.is_none());
    }

    #[test]
    fn watch_round_trips() {
        let w = Watch {
            id: Uuid::nil(),
            name: "RTX 3080".into(),
            family: Some("nvidia-rtx".into()),
            model: Some("3080".into()),
            min_capacity_gb: None,
            max_price_cents: Some(50_000),
            active: true,
            created_at: DateTime::UNIX_EPOCH,
        };
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(serde_json::from_str::<Watch>(&json).unwrap(), w);
    }
}
```

`crates/ferret-domain/src/lib.rs` (replace):

```rust
//! ferret domain: pure types and logic — no I/O. Everything here is
//! deterministic and unit-testable without a runtime.

pub mod deal;
pub mod listing;
pub mod watch;

pub use deal::{Deal, Flag};
pub use listing::RawListing;
pub use watch::{Watch, WatchRequest};
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p ferret-domain`
Expected: 3 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/ferret-domain
git commit -m "feat(domain): core types — RawListing, Deal, Flag, Watch"
```

---

### Task 3: Normalization — price parsing, canonical URL, title cleanup

**Files:**
- Create: `crates/ferret-domain/src/normalize.rs`
- Modify: `crates/ferret-domain/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/ferret-domain/src/normalize.rs` with ONLY the test module first:

```rust
//! Normalization of raw scraped values: price text → integer cents,
//! listing URL → canonical form, title whitespace cleanup.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_eu_style_price() {
        // French marketplaces: "1 234,56 €" (narrow nbsp or space separators)
        assert_eq!(parse_price("1 234,56 €"), Some((123_456, "EUR".into())));
        assert_eq!(parse_price("49,99€"), Some((4_999, "EUR".into())));
    }

    #[test]
    fn parses_us_style_price() {
        assert_eq!(parse_price("$1,234.56"), Some((123_456, "USD".into())));
        assert_eq!(parse_price("USD 12.00"), Some((1_200, "USD".into())));
    }

    #[test]
    fn parses_bare_integer_price() {
        assert_eq!(parse_price("120 €"), Some((12_000, "EUR".into())));
    }

    #[test]
    fn rejects_priceless_text() {
        assert_eq!(parse_price("Contact seller"), None);
        assert_eq!(parse_price(""), None);
    }

    #[test]
    fn canonical_url_strips_tracking_and_fragment() {
        assert_eq!(
            canonical_url("https://ex.com/item/42?utm_source=x&ref=abc&id=7#frag").unwrap(),
            "https://ex.com/item/42?id=7"
        );
    }

    #[test]
    fn canonical_url_lowercases_host_keeps_path_case() {
        assert_eq!(
            canonical_url("https://Ex.COM/Item/42").unwrap(),
            "https://ex.com/Item/42"
        );
    }

    #[test]
    fn canonical_url_rejects_garbage() {
        assert!(canonical_url("not a url").is_none());
    }

    #[test]
    fn clean_title_collapses_whitespace() {
        assert_eq!(clean_title("  RTX\u{a0}3080\n 10GB  "), "RTX 3080 10GB");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod normalize;` to `lib.rs`, then:

Run: `cargo test -p ferret-domain normalize`
Expected: FAIL to compile — `parse_price`, `canonical_url`, `clean_title` not found.

- [ ] **Step 3: Write the implementation (above the test module in `normalize.rs`)**

```rust
use std::sync::LazyLock;

use regex::Regex;
use url::Url;

/// Query params that never identify the product — stripped during
/// canonicalization so retargeted URLs dedupe to one deal.
const TRACKING_PARAMS: &[&str] = &[
    "ref", "referrer", "aff", "affid", "tag", "fbclid", "gclid", "mc_cid", "mc_eid",
];

static PRICE_RE: LazyLock<Regex> = LazyLock::new(|| {
    // number with optional thousands separators and optional decimal part
    Regex::new(r"(\d{1,3}(?:[ \u{a0}\u{202f},.]\d{3})*|\d+)(?:[.,](\d{1,2}))?").unwrap()
});

/// Parse a scraped price string into `(cents, currency)`.
///
/// Handles EU ("1 234,56 €") and US ("$1,234.56") styles. The currency is
/// inferred from the symbol/code found in the text; defaults to EUR when a
/// number is present but no currency marker is (self-hosted EU bias, and
/// sources can be assumed single-currency).
pub fn parse_price(text: &str) -> Option<(i64, String)> {
    let currency = if text.contains('€') || text.to_uppercase().contains("EUR") {
        "EUR"
    } else if text.contains('$') || text.to_uppercase().contains("USD") {
        "USD"
    } else if text.contains('£') || text.to_uppercase().contains("GBP") {
        "GBP"
    } else {
        "EUR"
    };
    let caps = PRICE_RE.captures(text)?;
    let whole: String = caps[1].chars().filter(|c| c.is_ascii_digit()).collect();
    let whole: i64 = whole.parse().ok()?;
    let cents = match caps.get(2) {
        Some(frac) => {
            let f: i64 = frac.as_str().parse().ok()?;
            if frac.as_str().len() == 1 { f * 10 } else { f }
        }
        None => 0,
    };
    Some((whole * 100 + cents, currency.to_string()))
}

/// Canonicalize a listing URL: lowercase host, drop the fragment, drop
/// tracking query params (`utm_*` and the known list), keep the rest.
pub fn canonical_url(raw: &str) -> Option<String> {
    let mut url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.set_fragment(None);
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !k.starts_with("utm_") && !TRACKING_PARAMS.contains(&k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(kept).finish();
    }
    Some(url.to_string())
}

/// Collapse all whitespace runs (including nbsp) to single spaces and trim.
pub fn clean_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}
```

Note: one EU subtlety the tests pin down — in `"1 234,56"` the space is a thousands separator handled by the first capture group, and `",56"` lands in the decimal group. `"$1,234.56"` works because `1,234` matches the separator alternation and `.56` is the decimal group.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ferret-domain normalize`
Expected: 8 tests PASS. If `parses_eu_style_price` fails on `"1 234,56 €"`, check that the regex's separator class includes a plain space — a common miss.

- [ ] **Step 5: Commit**

```bash
git add crates/ferret-domain
git commit -m "feat(domain): normalization — price parsing, canonical URLs, title cleanup"
```

---

### Task 4: Attribute extraction (capacity, condition)

**Files:**
- Create: `crates/ferret-domain/src/attributes.rs`
- Modify: `crates/ferret-domain/src/lib.rs`

Model extraction is family-table-driven and lives in Task 5; this task covers the regex-driven attributes.

- [ ] **Step 1: Write the failing tests**

Create `crates/ferret-domain/src/attributes.rs` with the types and test module:

```rust
//! Regex-driven attribute extraction from listing titles: capacity and
//! condition. Model extraction is family-table-driven — see `family`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExtractedAttributes {
    /// Decimal gigabytes (1 TB = 1000 GB, 1 To = 1000 Go).
    pub capacity_gb: Option<i64>,
    /// "new" | "used" | "refurbished".
    pub condition: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(title: &str) -> Option<i64> {
        extract(title).capacity_gb
    }

    #[test]
    fn extracts_tb_capacity() {
        assert_eq!(cap("Seagate IronWolf 4TB NAS"), Some(4000));
        assert_eq!(cap("WD Red 4 To 3.5\""), Some(4000));
    }

    #[test]
    fn extracts_gb_capacity() {
        assert_eq!(cap("Crucial 16GB DDR4 3200"), Some(16));
        assert_eq!(cap("Kit 2x8 Go DDR4"), Some(8)); // first capacity token wins
    }

    #[test]
    fn extracts_fractional_tb() {
        assert_eq!(cap("SSD 1.5TB"), Some(1500));
        assert_eq!(cap("SSD 1,5 To"), Some(1500));
    }

    #[test]
    fn no_capacity_in_gpu_title() {
        // "3080" must not parse as capacity
        assert_eq!(cap("RTX 3080 Founders Edition"), None);
    }

    #[test]
    fn detects_condition() {
        assert_eq!(extract("RTX 3080 occasion").condition.as_deref(), Some("used"));
        assert_eq!(extract("RTX 3080 used, works").condition.as_deref(), Some("used"));
        assert_eq!(extract("Disque neuf sous blister").condition.as_deref(), Some("new"));
        assert_eq!(
            extract("Serveur reconditionné DL380").condition.as_deref(),
            Some("refurbished")
        );
        assert_eq!(
            extract("HP refurb server").condition.as_deref(),
            Some("refurbished")
        );
        assert_eq!(extract("RTX 3080").condition, None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod attributes;` and `pub use attributes::ExtractedAttributes;` to `lib.rs`, then:

Run: `cargo test -p ferret-domain attributes`
Expected: FAIL to compile — `extract` not found.

- [ ] **Step 3: Write the implementation (above the test module)**

```rust
use std::sync::LazyLock;

use regex::Regex;

static CAPACITY_RE: LazyLock<Regex> = LazyLock::new(|| {
    // 4TB / 4 To / 16GB / 16 Go / 1.5TB / 1,5 To. The unit is required and
    // word-bounded on the right; the number is deliberately NOT left-bounded
    // so kit notation like "2x8 Go" still matches ("x8" has no \b before 8).
    Regex::new(r"(?i)(\d+(?:[.,]\d+)?)\s*(tb|to|gb|go)\b").unwrap()
});

/// (keyword, canonical condition) — first match in title order wins.
const CONDITIONS: &[(&str, &str)] = &[
    ("reconditionné", "refurbished"),
    ("reconditionne", "refurbished"),
    ("refurbished", "refurbished"),
    ("refurb", "refurbished"),
    ("occasion", "used"),
    ("used", "used"),
    ("neuf", "new"),
    ("neuve", "new"),
    ("brand new", "new"),
    (" new", "new"),
];

/// Extract capacity and condition from a (cleaned) listing title.
pub fn extract(title: &str) -> ExtractedAttributes {
    let capacity_gb = CAPACITY_RE.captures(title).and_then(|caps| {
        let n: f64 = caps[1].replace(',', ".").parse().ok()?;
        let gb = match caps[2].to_lowercase().as_str() {
            "tb" | "to" => n * 1000.0,
            _ => n,
        };
        Some(gb.round() as i64)
    });

    let lower = title.to_lowercase();
    let condition = CONDITIONS
        .iter()
        .find(|(kw, _)| lower.contains(kw))
        .map(|(_, canon)| canon.to_string());

    ExtractedAttributes { capacity_gb, condition }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ferret-domain attributes`
Expected: 5 tests PASS. Watch the `" new"` keyword (leading space) — it exists so "used, works like new" doesn't override, but table order already ensures `used` wins since it appears earlier in `CONDITIONS`; the space just avoids matching "renewed"-style substrings.

- [ ] **Step 5: Commit**

```bash
git add crates/ferret-domain
git commit -m "feat(domain): regex attribute extraction — capacity, condition"
```

---

### Task 5: Product families — model matching + stuffing score

**Files:**
- Create: `crates/ferret-domain/src/family.rs`
- Modify: `crates/ferret-domain/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/ferret-domain/src/family.rs`:

```rust
//! Config-driven product family tables and the stuffing score.
//!
//! A family lists sibling models (e.g. all RTX xx80 GPUs). A title
//! enumerating many siblings is likely SEO-stuffed: the score is a SIGNAL
//! attached to the deal, never a hard filter.

use serde::{Deserialize, Serialize};

/// One product family from config (`[[families]]` in ferret.toml).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductFamily {
    /// Stable id used by watches and price history, e.g. "nvidia-rtx".
    pub name: String,
    /// Sibling model tokens, matched word-bounded case-insensitive,
    /// e.g. ["3060", "3070", "3080", "3090", "4080"].
    pub models: Vec<String>,
}

/// Result of matching a title against the family tables.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FamilyMatch {
    /// Family with the most model hits (None when no model matched).
    pub family: Option<String>,
    /// Every model of that family present in the title.
    pub models: Vec<String>,
    /// 0.0 = at most one model; → 1.0 as the title enumerates the family.
    pub stuffing_score: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu_family() -> ProductFamily {
        ProductFamily {
            name: "nvidia-rtx".into(),
            models: ["2080", "3060", "3070", "3080", "3090", "4080", "4090"]
                .map(String::from)
                .to_vec(),
        }
    }

    #[test]
    fn single_model_scores_zero() {
        let m = match_families("RTX 3080 Founders Edition 10GB", &[gpu_family()]);
        assert_eq!(m.family.as_deref(), Some("nvidia-rtx"));
        assert_eq!(m.models, vec!["3080"]);
        assert_eq!(m.stuffing_score, 0.0);
    }

    #[test]
    fn stuffed_title_scores_high() {
        let m = match_families(
            "GPU riser for 2080 3060 3070 3080 3090 4080 4090",
            &[gpu_family()],
        );
        assert_eq!(m.family.as_deref(), Some("nvidia-rtx"));
        assert_eq!(m.models.len(), 7);
        assert_eq!(m.stuffing_score, 1.0);
    }

    #[test]
    fn two_models_score_partial() {
        // 2 of 7 models → (2-1)/(7-1) ≈ 0.1667
        let m = match_families("RTX 3080 or 3090, you pick", &[gpu_family()]);
        assert_eq!(m.models.len(), 2);
        assert!((m.stuffing_score - 1.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn model_must_be_word_bounded() {
        // "30809" must not match "3080"
        let m = match_families("Part number 30809", &[gpu_family()]);
        assert_eq!(m.family, None);
        assert!(m.models.is_empty());
    }

    #[test]
    fn no_match_is_empty_default() {
        let m = match_families("4TB IronWolf NAS drive", &[gpu_family()]);
        assert_eq!(m, FamilyMatch::default());
    }

    #[test]
    fn picks_family_with_most_hits() {
        let other = ProductFamily {
            name: "amd-rx".into(),
            models: vec!["6800".into(), "6900".into()],
        };
        let m = match_families("RTX 3080 3090 vs RX 6800", &[gpu_family(), other]);
        assert_eq!(m.family.as_deref(), Some("nvidia-rtx"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod family;` and `pub use family::{FamilyMatch, ProductFamily};` to `lib.rs`, then:

Run: `cargo test -p ferret-domain family`
Expected: FAIL to compile — `match_families` not found.

- [ ] **Step 3: Write the implementation (above the test module)**

```rust
use regex::escape;
use regex::Regex;

/// Match a title against every family table; return the family with the
/// most model hits, the models found, and the stuffing score.
///
/// Score: 0 or 1 model → 0.0; otherwise `(hits - 1) / (family_size - 1)`,
/// i.e. the fraction of *additional* siblings enumerated.
pub fn match_families(title: &str, families: &[ProductFamily]) -> FamilyMatch {
    let mut best = FamilyMatch::default();
    for family in families {
        let hits: Vec<String> = family
            .models
            .iter()
            .filter(|model| {
                // word-bounded, case-insensitive; models come from config so
                // building the regex per call is fine at this scale
                Regex::new(&format!(r"(?i)\b{}\b", escape(model)))
                    .map(|re| re.is_match(title))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        if hits.len() > best.models.len() {
            let score = if hits.len() <= 1 || family.models.len() <= 1 {
                0.0
            } else {
                (hits.len() - 1) as f64 / (family.models.len() - 1) as f64
            };
            best = FamilyMatch {
                family: Some(family.name.clone()),
                models: hits,
                stuffing_score: score,
            };
        }
    }
    best
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ferret-domain family`
Expected: 6 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ferret-domain
git commit -m "feat(domain): family model matching and stuffing score"
```

---

### Task 6: Rolling-median price outlier detection

**Files:**
- Create: `crates/ferret-domain/src/price.rs`
- Modify: `crates/ferret-domain/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/ferret-domain/src/price.rs`:

```rust
//! Price-outlier detection against the rolling median of recent observed
//! prices for the same (family, model). Too-good-to-be-true prices are a
//! scam signal — flagged, never dropped.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_of_odd_and_even_counts() {
        assert_eq!(median(&[3, 1, 2]), Some(2));
        assert_eq!(median(&[4, 1, 2, 3]), Some(2)); // lower of the two middles
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn outlier_when_far_below_median() {
        // median 50_000, ratio 0.5 → outlier under 25_000
        let history = [48_000, 50_000, 52_000, 51_000, 49_000];
        assert!(is_outlier(20_000, &history, 0.5));
        assert!(!is_outlier(30_000, &history, 0.5));
        assert!(!is_outlier(50_000, &history, 0.5));
    }

    #[test]
    fn no_outlier_with_thin_history() {
        // fewer than MIN_HISTORY observations → never an outlier
        assert!(!is_outlier(1, &[50_000, 51_000], 0.5));
        assert!(!is_outlier(1, &[], 0.5));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod price;` to `lib.rs`, then:

Run: `cargo test -p ferret-domain price`
Expected: FAIL to compile — `median`, `is_outlier` not found.

- [ ] **Step 3: Write the implementation (above the test module)**

```rust
/// Below this many observations the median is noise — never flag.
pub const MIN_HISTORY: usize = 5;

/// Median price in cents. Even-length inputs return the lower middle —
/// exactness doesn't matter for a threshold signal.
pub fn median(prices: &[i64]) -> Option<i64> {
    if prices.is_empty() {
        return None;
    }
    let mut sorted = prices.to_vec();
    sorted.sort_unstable();
    Some(sorted[(sorted.len() - 1) / 2])
}

/// True when `price` is below `ratio × median(history)` and there is
/// enough history to trust the median.
pub fn is_outlier(price: i64, history: &[i64], ratio: f64) -> bool {
    if history.len() < MIN_HISTORY {
        return false;
    }
    match median(history) {
        Some(m) => (price as f64) < (m as f64) * ratio,
        None => false,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ferret-domain price`
Expected: 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ferret-domain
git commit -m "feat(domain): rolling-median price outlier detection"
```

---

### Task 7: Watch matching

**Files:**
- Create: `crates/ferret-domain/src/matching.rs`
- Modify: `crates/ferret-domain/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/ferret-domain/src/matching.rs`:

```rust
//! Matching persisted deals against active watches. Pure predicate —
//! flags (stuffing, outlier) never veto a match, they ride along.

use crate::deal::Deal;
use crate::watch::Watch;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use uuid::Uuid;

    fn deal() -> Deal {
        Deal {
            id: Uuid::nil(),
            source_id: "src".into(),
            canonical_url: "https://ex.com/1".into(),
            title: "RTX 3080 10GB".into(),
            price_cents: 45_000,
            currency: "EUR".into(),
            family: Some("nvidia-rtx".into()),
            models: vec!["3080".into()],
            capacity_gb: Some(10),
            condition: None,
            stuffing_score: 0.0,
            flags: vec![],
            first_seen: DateTime::UNIX_EPOCH,
            last_seen: DateTime::UNIX_EPOCH,
        }
    }

    fn watch() -> Watch {
        Watch {
            id: Uuid::nil(),
            name: "RTX 3080".into(),
            family: Some("nvidia-rtx".into()),
            model: Some("3080".into()),
            min_capacity_gb: None,
            max_price_cents: Some(50_000),
            active: true,
            created_at: DateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn matches_family_model_and_price() {
        assert!(watch_matches(&watch(), &deal()));
    }

    #[test]
    fn price_over_budget_rejects() {
        let mut d = deal();
        d.price_cents = 60_000;
        assert!(!watch_matches(&watch(), &d));
    }

    #[test]
    fn wrong_model_rejects() {
        let mut w = watch();
        w.model = Some("3090".into());
        assert!(!watch_matches(&w, &deal()));
    }

    #[test]
    fn stuffed_listing_containing_watched_model_still_matches() {
        // spec: stuffing is a signal, not a filter
        let mut d = deal();
        d.models = vec!["3070".into(), "3080".into(), "3090".into()];
        d.stuffing_score = 0.4;
        assert!(watch_matches(&watch(), &d));
    }

    #[test]
    fn capacity_floor_enforced() {
        let mut w = watch();
        w.family = None;
        w.model = None;
        w.min_capacity_gb = Some(4000);
        let mut d = deal();
        d.capacity_gb = Some(2000);
        assert!(!watch_matches(&w, &d));
        d.capacity_gb = Some(4000);
        assert!(watch_matches(&w, &d));
        d.capacity_gb = None; // watch demands capacity, deal has none
        assert!(!watch_matches(&w, &d));
    }

    #[test]
    fn inactive_watch_never_matches() {
        let mut w = watch();
        w.active = false;
        assert!(!watch_matches(&w, &deal()));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod matching;` to `lib.rs`, then:

Run: `cargo test -p ferret-domain matching`
Expected: FAIL to compile — `watch_matches` not found.

- [ ] **Step 3: Write the implementation (above the test module)**

```rust
/// Does this deal satisfy this watch? Every filter the watch sets must
/// hold; filters the watch leaves unset are ignored. Flags are not
/// consulted — a flagged deal matches and surfaces with its badges.
pub fn watch_matches(watch: &Watch, deal: &Deal) -> bool {
    if !watch.active {
        return false;
    }
    if let Some(family) = &watch.family
        && deal.family.as_ref() != Some(family)
    {
        return false;
    }
    if let Some(model) = &watch.model
        && !deal.models.contains(model)
    {
        return false;
    }
    if let Some(min_gb) = watch.min_capacity_gb
        && !deal.capacity_gb.is_some_and(|c| c >= min_gb)
    {
        return false;
    }
    if let Some(max) = watch.max_price_cents
        && deal.price_cents > max
    {
        return false;
    }
    true
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ferret-domain matching` — expected: 6 tests PASS.
Then run the full domain suite: `cargo test -p ferret-domain` — all tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ferret-domain
git commit -m "feat(domain): watch matching predicate"
```

---

### Task 8: Server config (figment) + example TOML

**Files:**
- Create: `crates/ferret-server/src/config.rs`
- Create: `crates/ferret-server/ferret.example.toml`
- Modify: `crates/ferret-server/src/main.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/ferret-server/src/config.rs` starting with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let config: Config = Figment::from(Serialized::defaults(Config::default()))
            .extract()
            .unwrap();
        assert_eq!(config.listen.port(), 4800);
        assert!(config.sources.is_empty());
        assert!(config.notifications.ntfy_url.is_none());
    }

    #[test]
    fn example_config_parses() {
        let config: Config = Figment::from(Serialized::defaults(Config::default()))
            .merge(Toml::string(include_str!("../ferret.example.toml")))
            .extract()
            .unwrap();
        assert!(!config.sources.is_empty());
        assert!(!config.families.is_empty());
        let src = &config.sources[0];
        assert!(!src.id.is_empty());
        assert!(src.interval_minutes >= 1);
    }
}
```

- [ ] **Step 2: Add `mod config;` to `main.rs` and run tests to verify failure**

`main.rs`:

```rust
mod config;

fn main() {
    println!("ferret-server");
}
```

Run: `cargo test -p ferret-server config`
Expected: FAIL to compile — `Config` etc. not found.

- [ ] **Step 3: Write the implementation (above the test module in `config.rs`)**

```rust
//! Server configuration.
//!
//! Layered with figment: built-in defaults ← TOML file ← `FERRET_*` env
//! vars. The TOML file is `$FERRET_CONFIG`, or `./ferret.toml` if present.
//! On NixOS the file will be generated by the `services.ferret` module.

use std::net::SocketAddr;
use std::path::PathBuf;

use ferret_domain::ProductFamily;
use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub listen: SocketAddr,
    /// SQLite database file (created on first start).
    pub db_path: PathBuf,
    pub scrape: ScrapeConfig,
    /// Declarative static-HTML sources (generic scraper engine).
    pub sources: Vec<SourceConfig>,
    /// Product family / sibling-model tables (stuffing + outlier signals).
    pub families: Vec<ProductFamily>,
    pub notifications: NotificationsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: ([0, 0, 0, 0], 4800).into(),
            db_path: "ferret.db".into(),
            scrape: ScrapeConfig::default(),
            sources: Vec::new(),
            families: Vec::new(),
            notifications: NotificationsConfig::default(),
        }
    }
}

/// Global scraping knobs; per-source politeness lives on `SourceConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrapeConfig {
    /// A deal priced below `ratio × rolling median` gets `price-outlier`.
    pub outlier_ratio: f64,
    /// Stuffing score at or above this gets `possible-stuffing`.
    pub stuffing_threshold: f64,
    /// Consecutive failures of one source before an ntfy alert fires.
    pub failure_alert_after: u32,
}

impl Default for ScrapeConfig {
    fn default() -> Self {
        Self {
            outlier_ratio: 0.5,
            stuffing_threshold: 0.25,
            failure_alert_after: 5,
        }
    }
}

/// One declarative source: URL + CSS selectors, interpreted by the
/// generic scraper engine. JS-heavy/authenticated sources get hand-written
/// `DealSource` impls instead (later plan).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    /// Stable id, e.g. "hddboard".
    pub id: String,
    /// Page to fetch. `{page}` is replaced by the page number when
    /// `max_pages > 1`.
    pub url: String,
    /// CSS selector for one listing container.
    pub item_selector: String,
    /// Selectors relative to the item container.
    pub title_selector: String,
    pub price_selector: String,
    /// Selector for the link (`href` attribute is taken); defaults to the
    /// item container itself when it is an `<a>`.
    #[serde(default)]
    pub link_selector: Option<String>,
    /// Scrape interval.
    #[serde(default = "default_interval")]
    pub interval_minutes: u64,
    /// Minimum delay between requests to this source.
    #[serde(default = "default_delay")]
    pub delay_ms: u64,
    /// Max concurrent requests to this source.
    #[serde(default = "default_concurrency")]
    pub max_concurrency: usize,
    /// Pages fetched per tick (page 1..=max_pages via `{page}`).
    #[serde(default = "default_pages")]
    pub max_pages: u32,
}

fn default_interval() -> u64 {
    30
}
fn default_delay() -> u64 {
    2000
}
fn default_concurrency() -> usize {
    1
}
fn default_pages() -> u32 {
    1
}

/// Push notifications via ntfy. The whole feature is off when `ntfy_url`
/// is `None` (the default).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationsConfig {
    /// ntfy server base URL, e.g. `https://notify.zeus.balem.fr`.
    pub ntfy_url: Option<url::Url>,
    /// Topic notifications are published to, e.g. "deals-zeus".
    pub topic: String,
    /// Bearer token for protected topics; read from this file at startup
    /// (agenix-managed on NixOS).
    pub token_file: Option<PathBuf>,
}

/// Load configuration: defaults ← TOML ← env.
pub fn load() -> figment::Result<Config> {
    let path = std::env::var("FERRET_CONFIG").unwrap_or_else(|_| "ferret.toml".into());
    Figment::from(Serialized::defaults(Config::default()))
        .merge(Toml::file(path))
        .merge(Env::prefixed("FERRET_").split("__"))
        .extract()
}
```

- [ ] **Step 4: Write `crates/ferret-server/ferret.example.toml`**

```toml
# Example ferret server configuration.
# Copy to ferret.toml (or point FERRET_CONFIG at it) and adjust.
# On NixOS this file is generated by the services.ferret module.

listen = "0.0.0.0:4800"
db_path = "ferret.db"

[scrape]
outlier_ratio = 0.5          # price < 0.5 × rolling median → price-outlier flag
stuffing_threshold = 0.25    # stuffing score ≥ 0.25 → possible-stuffing flag
failure_alert_after = 5      # consecutive source failures before an ntfy alert

# ---- declarative sources (generic scraper engine) ----
# One entry per static-HTML source. JS-heavy or authenticated sources are
# hand-written DealSource plugins instead (not yet implemented).

[[sources]]
id = "example-board"
url = "https://deals.example.com/hardware?page={page}"
item_selector = "div.listing"
title_selector = "h2.title"
price_selector = "span.price"
link_selector = "a.listing-link"
interval_minutes = 30
delay_ms = 2000              # politeness: min delay between requests
max_concurrency = 1          # politeness: concurrent requests cap
max_pages = 3

# ---- product family tables ----
# Sibling-model lists drive stuffing detection and per-model price history.
# Adding a generation is a config change, never a code change.

[[families]]
name = "nvidia-rtx"
models = ["2060", "2070", "2080", "3060", "3070", "3080", "3090", "4060", "4070", "4080", "4090", "5080", "5090"]

[[families]]
name = "ddr4-kit"
models = ["8GB", "16GB", "32GB", "64GB"]

# ---- notifications ----
# Push via ntfy; omit the section (or leave ntfy_url unset) to disable.
# [notifications]
# ntfy_url = "https://notify.zeus.balem.fr"
# topic = "deals-zeus"
# token_file = "/run/agenix/ferret-ntfy-token"
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p ferret-server config`
Expected: 2 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/ferret-server
git commit -m "feat(server): figment config with sources, families, notifications"
```

---

### Task 9: Storage — migrations + Db

**Files:**
- Create: `crates/ferret-server/migrations/0001_init.sql`
- Create: `crates/ferret-server/src/db.rs`
- Modify: `crates/ferret-server/src/main.rs`

- [ ] **Step 1: Write the migration**

`crates/ferret-server/migrations/0001_init.sql`:

```sql
-- Conventions (same as chaos): UUIDs as hyphenated TEXT, timestamps as
-- RFC3339 TEXT, JSON arrays in TEXT columns, money in integer cents.

CREATE TABLE watches (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    family          TEXT,
    model           TEXT,
    min_capacity_gb INTEGER,
    max_price_cents INTEGER,
    active          INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL
);

CREATE TABLE deals (
    id             TEXT PRIMARY KEY,
    source_id      TEXT NOT NULL,
    canonical_url  TEXT NOT NULL,
    title          TEXT NOT NULL,
    price_cents    INTEGER NOT NULL,
    currency       TEXT NOT NULL,
    family         TEXT,
    models         TEXT NOT NULL DEFAULT '[]',   -- JSON array of model strings
    capacity_gb    INTEGER,
    condition      TEXT,
    stuffing_score REAL NOT NULL DEFAULT 0,
    flags          TEXT NOT NULL DEFAULT '[]',   -- JSON array of Flag
    first_seen     TEXT NOT NULL,
    last_seen      TEXT NOT NULL,
    UNIQUE (source_id, canonical_url)
);

CREATE INDEX deals_family_model ON deals (family);

-- Watch ↔ deal matches; notified guards against duplicate pushes.
CREATE TABLE deal_matches (
    deal_id    TEXT NOT NULL REFERENCES deals (id) ON DELETE CASCADE,
    watch_id   TEXT NOT NULL REFERENCES watches (id) ON DELETE CASCADE,
    matched_at TEXT NOT NULL,
    notified   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (deal_id, watch_id)
);

-- Rolling price observations per (family, exact model) — the basis for
-- outlier detection. Only unambiguous (single-model) listings feed it.
CREATE TABLE price_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    family      TEXT NOT NULL,
    model       TEXT NOT NULL,
    price_cents INTEGER NOT NULL,
    observed_at TEXT NOT NULL
);

CREATE INDEX price_history_family_model ON price_history (family, model, observed_at);
```

- [ ] **Step 2: Write the failing tests**

Create `crates/ferret-server/src/db.rs` with the test module at the bottom (types referenced come in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ferret_domain::{Flag, WatchRequest};

    async fn test_db() -> Db {
        Db::connect(Path::new(":memory:")).await.unwrap()
    }

    fn deal(url: &str, price: i64) -> Deal {
        Deal {
            id: Uuid::new_v4(),
            source_id: "src".into(),
            canonical_url: url.into(),
            title: "RTX 3080".into(),
            price_cents: price,
            currency: "EUR".into(),
            family: Some("nvidia-rtx".into()),
            models: vec!["3080".into()],
            capacity_gb: None,
            condition: None,
            stuffing_score: 0.0,
            flags: vec![Flag::PossibleStuffing],
            first_seen: Utc::now(),
            last_seen: Utc::now(),
        }
    }

    #[tokio::test]
    async fn watch_crud_round_trip() {
        let db = test_db().await;
        let req = WatchRequest {
            name: "RTX 3080".into(),
            family: Some("nvidia-rtx".into()),
            model: Some("3080".into()),
            min_capacity_gb: None,
            max_price_cents: Some(50_000),
            active: true,
        };
        let created = db.create_watch(&req).await.unwrap();
        let listed = db.list_watches().await.unwrap();
        assert_eq!(listed, vec![created.clone()]);

        let mut update = req.clone();
        update.active = false;
        let updated = db.update_watch(created.id, &update).await.unwrap();
        assert!(!updated.active);

        db.delete_watch(created.id).await.unwrap();
        assert!(db.list_watches().await.unwrap().is_empty());
        assert!(matches!(
            db.delete_watch(created.id).await,
            Err(DbError::NotFound)
        ));
    }

    #[tokio::test]
    async fn upsert_deal_inserts_then_updates() {
        let db = test_db().await;
        let d = deal("https://ex.com/1", 45_000);
        let (stored, was_new) = db.upsert_deal(&d).await.unwrap();
        assert!(was_new);
        assert_eq!(stored.flags, vec![Flag::PossibleStuffing]);

        // same (source, canonical_url), new price → update, keep first_seen/id
        let mut d2 = deal("https://ex.com/1", 42_000);
        d2.id = Uuid::new_v4();
        let (updated, was_new) = db.upsert_deal(&d2).await.unwrap();
        assert!(!was_new);
        assert_eq!(updated.id, stored.id);
        assert_eq!(updated.price_cents, 42_000);
        assert_eq!(updated.first_seen, stored.first_seen);

        assert_eq!(db.list_deals(None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn price_history_round_trip() {
        let db = test_db().await;
        for p in [40_000, 45_000, 50_000] {
            db.record_price("nvidia-rtx", "3080", p).await.unwrap();
        }
        let prices = db.recent_prices("nvidia-rtx", "3080", 50).await.unwrap();
        assert_eq!(prices.len(), 3);
        assert!(db.recent_prices("nvidia-rtx", "3090", 50).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn deal_match_insert_is_idempotent() {
        let db = test_db().await;
        let w = db
            .create_watch(&WatchRequest {
                name: "w".into(),
                family: None,
                model: None,
                min_capacity_gb: None,
                max_price_cents: None,
                active: true,
            })
            .await
            .unwrap();
        let (d, _) = db.upsert_deal(&deal("https://ex.com/1", 45_000)).await.unwrap();

        assert!(db.insert_match(d.id, w.id).await.unwrap()); // new match
        assert!(!db.insert_match(d.id, w.id).await.unwrap()); // already known

        let deals = db.list_deals(Some(w.id)).await.unwrap();
        assert_eq!(deals.len(), 1);
    }
}
```

Note the derive on `WatchRequest`: the test clones it — `WatchRequest` already derives `Clone` from Task 2.

- [ ] **Step 3: Write the implementation (above the test module)**

```rust
//! SQLite persistence. All row ↔ domain-type mapping happens here and only
//! here — handlers and the pipeline never see SQL types.

use std::path::Path;

use chrono::{DateTime, Utc};
use ferret_domain::{Deal, Flag, Watch, WatchRequest};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("not found")]
    NotFound,
    #[error("invalid stored data: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    pub async fn connect(path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1) // single-user app; avoids SQLite write contention
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    // ---- watches ----

    pub async fn create_watch(&self, req: &WatchRequest) -> Result<Watch> {
        let watch = Watch {
            id: Uuid::new_v4(),
            name: req.name.clone(),
            family: req.family.clone(),
            model: req.model.clone(),
            min_capacity_gb: req.min_capacity_gb,
            max_price_cents: req.max_price_cents,
            active: req.active,
            created_at: Utc::now(),
        };
        sqlx::query(
            "INSERT INTO watches (id, name, family, model, min_capacity_gb, max_price_cents, active, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(watch.id.to_string())
        .bind(&watch.name)
        .bind(&watch.family)
        .bind(&watch.model)
        .bind(watch.min_capacity_gb)
        .bind(watch.max_price_cents)
        .bind(watch.active)
        .bind(watch.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(watch)
    }

    pub async fn list_watches(&self) -> Result<Vec<Watch>> {
        let rows = sqlx::query("SELECT * FROM watches ORDER BY created_at")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_watch).collect()
    }

    pub async fn update_watch(&self, id: Uuid, req: &WatchRequest) -> Result<Watch> {
        let result = sqlx::query(
            "UPDATE watches SET name = ?, family = ?, model = ?, min_capacity_gb = ?,
             max_price_cents = ?, active = ? WHERE id = ?",
        )
        .bind(&req.name)
        .bind(&req.family)
        .bind(&req.model)
        .bind(req.min_capacity_gb)
        .bind(req.max_price_cents)
        .bind(req.active)
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        let row = sqlx::query("SELECT * FROM watches WHERE id = ?")
            .bind(id.to_string())
            .fetch_one(&self.pool)
            .await?;
        row_to_watch(&row)
    }

    pub async fn delete_watch(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM watches WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    // ---- deals ----

    /// Insert the deal or, when (source_id, canonical_url) already exists,
    /// update its mutable fields keeping id and first_seen. Returns the
    /// stored deal and whether it was new.
    pub async fn upsert_deal(&self, deal: &Deal) -> Result<(Deal, bool)> {
        let existing = sqlx::query("SELECT * FROM deals WHERE source_id = ? AND canonical_url = ?")
            .bind(&deal.source_id)
            .bind(&deal.canonical_url)
            .fetch_optional(&self.pool)
            .await?;
        match existing {
            None => {
                sqlx::query(
                    "INSERT INTO deals (id, source_id, canonical_url, title, price_cents, currency,
                     family, models, capacity_gb, condition, stuffing_score, flags, first_seen, last_seen)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(deal.id.to_string())
                .bind(&deal.source_id)
                .bind(&deal.canonical_url)
                .bind(&deal.title)
                .bind(deal.price_cents)
                .bind(&deal.currency)
                .bind(&deal.family)
                .bind(serde_json::to_string(&deal.models).expect("serializing models"))
                .bind(deal.capacity_gb)
                .bind(&deal.condition)
                .bind(deal.stuffing_score)
                .bind(serde_json::to_string(&deal.flags).expect("serializing flags"))
                .bind(deal.first_seen.to_rfc3339())
                .bind(deal.last_seen.to_rfc3339())
                .execute(&self.pool)
                .await?;
                Ok((deal.clone(), true))
            }
            Some(row) => {
                let stored = row_to_deal(&row)?;
                sqlx::query(
                    "UPDATE deals SET title = ?, price_cents = ?, currency = ?, family = ?,
                     models = ?, capacity_gb = ?, condition = ?, stuffing_score = ?, flags = ?,
                     last_seen = ? WHERE id = ?",
                )
                .bind(&deal.title)
                .bind(deal.price_cents)
                .bind(&deal.currency)
                .bind(&deal.family)
                .bind(serde_json::to_string(&deal.models).expect("serializing models"))
                .bind(deal.capacity_gb)
                .bind(&deal.condition)
                .bind(deal.stuffing_score)
                .bind(serde_json::to_string(&deal.flags).expect("serializing flags"))
                .bind(deal.last_seen.to_rfc3339())
                .bind(stored.id.to_string())
                .execute(&self.pool)
                .await?;
                let merged = Deal {
                    id: stored.id,
                    first_seen: stored.first_seen,
                    ..deal.clone()
                };
                Ok((merged, false))
            }
        }
    }

    /// Deals, newest last_seen first; filtered to one watch's matches when
    /// `watch_id` is set.
    pub async fn list_deals(&self, watch_id: Option<Uuid>) -> Result<Vec<Deal>> {
        let rows = match watch_id {
            Some(w) => {
                sqlx::query(
                    "SELECT d.* FROM deals d
                     JOIN deal_matches m ON m.deal_id = d.id
                     WHERE m.watch_id = ? ORDER BY d.last_seen DESC",
                )
                .bind(w.to_string())
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query("SELECT * FROM deals ORDER BY last_seen DESC")
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        rows.iter().map(row_to_deal).collect()
    }

    // ---- matches ----

    /// Record that a deal matches a watch. Returns true when the match is
    /// new (i.e. a notification should fire), false when already known.
    pub async fn insert_match(&self, deal_id: Uuid, watch_id: Uuid) -> Result<bool> {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO deal_matches (deal_id, watch_id, matched_at, notified)
             VALUES (?, ?, ?, 0)",
        )
        .bind(deal_id.to_string())
        .bind(watch_id.to_string())
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn mark_notified(&self, deal_id: Uuid, watch_id: Uuid) -> Result<()> {
        sqlx::query("UPDATE deal_matches SET notified = 1 WHERE deal_id = ? AND watch_id = ?")
            .bind(deal_id.to_string())
            .bind(watch_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ---- price history ----

    pub async fn record_price(&self, family: &str, model: &str, price_cents: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO price_history (family, model, price_cents, observed_at) VALUES (?, ?, ?, ?)",
        )
        .bind(family)
        .bind(model)
        .bind(price_cents)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Most recent `limit` observed prices for (family, model).
    pub async fn recent_prices(&self, family: &str, model: &str, limit: u32) -> Result<Vec<i64>> {
        let rows = sqlx::query(
            "SELECT price_cents FROM price_history WHERE family = ? AND model = ?
             ORDER BY observed_at DESC LIMIT ?",
        )
        .bind(family)
        .bind(model)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| r.get::<i64, _>("price_cents")).collect())
    }
}

// ---- row mapping ----

fn parse_uuid(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| DbError::Corrupt(format!("bad uuid {s:?}: {e}")))
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| DbError::Corrupt(format!("bad timestamp {s:?}: {e}")))
}

fn row_to_watch(row: &sqlx::sqlite::SqliteRow) -> Result<Watch> {
    Ok(Watch {
        id: parse_uuid(&row.get::<String, _>("id"))?,
        name: row.get("name"),
        family: row.get("family"),
        model: row.get("model"),
        min_capacity_gb: row.get("min_capacity_gb"),
        max_price_cents: row.get("max_price_cents"),
        active: row.get("active"),
        created_at: parse_ts(&row.get::<String, _>("created_at"))?,
    })
}

fn row_to_deal(row: &sqlx::sqlite::SqliteRow) -> Result<Deal> {
    let models: Vec<String> = serde_json::from_str(&row.get::<String, _>("models"))
        .map_err(|e| DbError::Corrupt(format!("bad models json: {e}")))?;
    let flags: Vec<Flag> = serde_json::from_str(&row.get::<String, _>("flags"))
        .map_err(|e| DbError::Corrupt(format!("bad flags json: {e}")))?;
    Ok(Deal {
        id: parse_uuid(&row.get::<String, _>("id"))?,
        source_id: row.get("source_id"),
        canonical_url: row.get("canonical_url"),
        title: row.get("title"),
        price_cents: row.get("price_cents"),
        currency: row.get("currency"),
        family: row.get("family"),
        models,
        capacity_gb: row.get("capacity_gb"),
        condition: row.get("condition"),
        stuffing_score: row.get("stuffing_score"),
        flags,
        first_seen: parse_ts(&row.get::<String, _>("first_seen"))?,
        last_seen: parse_ts(&row.get::<String, _>("last_seen"))?,
    })
}
```

Add `mod db;` to `main.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ferret-server db`
Expected: 4 tests PASS. Gotcha: `Db::connect(Path::new(":memory:"))` — sqlx treats the literal filename `:memory:` as in-memory; with `max_connections(1)` the single connection keeps the database alive for the whole test.

- [ ] **Step 5: Commit**

```bash
git add crates/ferret-server
git commit -m "feat(server): sqlite storage — watches, deals, matches, price history"
```

---

### Task 10: Politeness tower layer + HTTP service

**Files:**
- Create: `crates/ferret-server/src/politeness.rs`
- Modify: `crates/ferret-server/src/main.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/ferret-server/src/politeness.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::time::Duration;
    use tokio::time::Instant;
    use tower::{service_fn, Layer, Service, ServiceExt};

    #[tokio::test(start_paused = true)]
    async fn enforces_min_delay_between_calls() {
        let inner = service_fn(|_req: ()| async { Ok::<_, Infallible>(()) });
        let mut svc = PolitenessLayer::new(Duration::from_millis(500), 1).layer(inner);

        let start = Instant::now();
        svc.ready().await.unwrap().call(()).await.unwrap();
        let first = start.elapsed();
        svc.ready().await.unwrap().call(()).await.unwrap();
        let second = start.elapsed();

        assert!(first < Duration::from_millis(10), "first call is immediate");
        assert!(
            second >= Duration::from_millis(500),
            "second call waited the delay, got {second:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn caps_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (fl, pk) = (in_flight.clone(), peak.clone());
        let inner = service_fn(move |_req: ()| {
            let (fl, pk) = (fl.clone(), pk.clone());
            async move {
                let now = fl.fetch_add(1, Ordering::SeqCst) + 1;
                pk.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                fl.fetch_sub(1, Ordering::SeqCst);
                Ok::<_, Infallible>(())
            }
        });
        let svc = PolitenessLayer::new(Duration::ZERO, 2).layer(inner);

        let futs: Vec<_> = (0..6)
            .map(|_| {
                let mut svc = svc.clone();
                tokio::spawn(async move { svc.ready().await.unwrap().call(()).await.unwrap() })
            })
            .collect();
        for f in futs {
            f.await.unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= 2, "peak concurrency ≤ cap");
    }
}
```

- [ ] **Step 2: Add `mod politeness;` to `main.rs`, run tests to verify failure**

Run: `cargo test -p ferret-server politeness`
Expected: FAIL to compile — `PolitenessLayer` not found.

- [ ] **Step 3: Write the implementation (above the test module)**

```rust
//! Per-source politeness as a `tower::Layer`: a minimum delay between
//! requests plus a concurrency cap. One layer instance per source; shared
//! by clone. Reusable over any tower `Service` (here: the reqwest adapter).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore};
use tokio::time::Instant;

#[derive(Clone)]
pub struct PolitenessLayer {
    state: Arc<State>,
}

struct State {
    min_delay: Duration,
    semaphore: Arc<Semaphore>,
    last_request: Mutex<Option<Instant>>,
}

impl PolitenessLayer {
    pub fn new(min_delay: Duration, max_concurrency: usize) -> Self {
        Self {
            state: Arc::new(State {
                min_delay,
                semaphore: Arc::new(Semaphore::new(max_concurrency.max(1))),
                last_request: Mutex::new(None),
            }),
        }
    }
}

impl<S> tower::Layer<S> for PolitenessLayer {
    type Service = Politeness<S>;

    fn layer(&self, inner: S) -> Self::Service {
        Politeness { inner, state: self.state.clone() }
    }
}

#[derive(Clone)]
pub struct Politeness<S> {
    inner: S,
    state: Arc<State>,
}

impl<S, R> tower::Service<R> for Politeness<S>
where
    S: tower::Service<R> + Clone + Send + 'static,
    S::Future: Send,
    R: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: R) -> Self::Future {
        let state = self.state.clone();
        // Take the ready inner service, leave a fresh clone behind
        // (standard tower pattern for boxed futures).
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            let _permit = state
                .semaphore
                .clone()
                .acquire_owned()
                .await
                .expect("politeness semaphore closed");
            {
                let mut last = state.last_request.lock().await;
                if let Some(prev) = *last {
                    let elapsed = prev.elapsed();
                    if elapsed < state.min_delay {
                        tokio::time::sleep(state.min_delay - elapsed).await;
                    }
                }
                *last = Some(Instant::now());
            }
            inner.call(req).await
        })
    }
}

/// reqwest as a tower `Service` so politeness (and any future middleware)
/// can wrap the outbound scraping client.
#[derive(Clone)]
pub struct HttpService {
    client: reqwest::Client,
}

impl HttpService {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("ferret/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("building scrape http client"),
        }
    }
}

impl tower::Service<reqwest::Request> for HttpService {
    type Response = reqwest::Response;
    type Error = reqwest::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: reqwest::Request) -> Self::Future {
        let client = self.client.clone();
        Box::pin(async move { client.execute(req).await })
    }
}

/// The polite per-source scrape client: politeness layer over reqwest.
pub type ScrapeClient = Politeness<HttpService>;

pub fn scrape_client(min_delay: Duration, max_concurrency: usize) -> ScrapeClient {
    use tower::Layer as _;
    PolitenessLayer::new(min_delay, max_concurrency).layer(HttpService::new())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ferret-server politeness`
Expected: 2 tests PASS. (`start_paused` makes the delay test instant — tokio auto-advances the paused clock through the sleeps.)

- [ ] **Step 5: Commit**

```bash
git add crates/ferret-server
git commit -m "feat(server): tower politeness layer over a reqwest service"
```

---

### Task 11: DealSource trait + generic declarative scraper

**Files:**
- Create: `crates/ferret-server/src/scrape/mod.rs`
- Create: `crates/ferret-server/src/scrape/generic.rs`
- Create: `crates/ferret-server/tests/fixtures/example_board.html`
- Modify: `crates/ferret-server/src/main.rs`

- [ ] **Step 1: Write the HTML fixture**

`crates/ferret-server/tests/fixtures/example_board.html`:

```html
<!doctype html>
<html>
  <body>
    <main>
      <div class="listing">
        <h2 class="title">Seagate IronWolf 4TB NAS — neuf</h2>
        <span class="price">89,99 €</span>
        <a class="listing-link" href="/item/1?utm_source=feed">details</a>
      </div>
      <div class="listing">
        <h2 class="title">RTX 3080 Founders Edition occasion</h2>
        <span class="price">450 €</span>
        <a class="listing-link" href="https://deals.example.com/item/2">details</a>
      </div>
      <div class="listing">
        <!-- broken listing: no price — must be skipped, not crash -->
        <h2 class="title">Mystery item</h2>
        <a class="listing-link" href="/item/3">details</a>
      </div>
    </main>
  </body>
</html>
```

- [ ] **Step 2: Write the `DealSource` trait in `crates/ferret-server/src/scrape/mod.rs`**

```rust
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
```

- [ ] **Step 3: Write the failing parser tests in `crates/ferret-server/src/scrape/generic.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SourceConfig;

    fn source_config() -> SourceConfig {
        SourceConfig {
            id: "example-board".into(),
            url: "https://deals.example.com/hardware?page={page}".into(),
            item_selector: "div.listing".into(),
            title_selector: "h2.title".into(),
            price_selector: "span.price".into(),
            link_selector: Some("a.listing-link".into()),
            interval_minutes: 30,
            delay_ms: 0,
            max_concurrency: 1,
            max_pages: 1,
        }
    }

    #[test]
    fn parses_fixture_listings() {
        let html = include_str!("../../tests/fixtures/example_board.html");
        let base = url::Url::parse("https://deals.example.com/hardware").unwrap();
        let listings = parse_listings(html, &source_config(), &base);

        assert_eq!(listings.len(), 2, "broken third listing is skipped");

        assert_eq!(listings[0].title, "Seagate IronWolf 4TB NAS — neuf");
        assert_eq!(listings[0].price_text, "89,99 €");
        // relative href resolved against the page URL
        assert_eq!(
            listings[0].url,
            "https://deals.example.com/item/1?utm_source=feed"
        );
        assert_eq!(listings[0].source_id, "example-board");

        assert_eq!(listings[1].url, "https://deals.example.com/item/2");
    }

    #[test]
    fn page_url_substitution() {
        assert_eq!(
            page_url(&source_config(), 2),
            "https://deals.example.com/hardware?page=2"
        );
        let mut cfg = source_config();
        cfg.url = "https://ex.com/deals".into(); // no {page} placeholder
        assert_eq!(page_url(&cfg, 2), "https://ex.com/deals");
    }
}
```

- [ ] **Step 4: Add modules and run tests to verify failure**

Add `mod scrape;` to `main.rs`.

Run: `cargo test -p ferret-server generic`
Expected: FAIL to compile — `parse_listings`, `page_url` not found.

- [ ] **Step 5: Write the implementation (above the test module in `generic.rs`)**

```rust
//! The generic declarative scraper: one engine interpreting per-source
//! config (URL template + CSS selectors). Parsing is a pure function over
//! the fetched HTML — fixture-testable without any network.

use chrono::Utc;
use ferret_domain::RawListing;
use scraper::{Html, Selector};
use url::Url;

use crate::config::SourceConfig;
use crate::politeness::ScrapeClient;
use crate::scrape::DealSource;

use tower::{Service, ServiceExt};

/// Build the URL for one page: `{page}` substituted when present.
pub fn page_url(config: &SourceConfig, page: u32) -> String {
    config.url.replace("{page}", &page.to_string())
}

/// Parse one fetched page into raw listings. Listings missing a title,
/// price, or resolvable link are skipped (logged), never fatal.
pub fn parse_listings(html: &str, config: &SourceConfig, base: &Url) -> Vec<RawListing> {
    let Ok(item_sel) = Selector::parse(&config.item_selector) else {
        tracing::error!(source = config.id, selector = config.item_selector, "bad item selector");
        return Vec::new();
    };
    let Ok(title_sel) = Selector::parse(&config.title_selector) else {
        tracing::error!(source = config.id, "bad title selector");
        return Vec::new();
    };
    let Ok(price_sel) = Selector::parse(&config.price_selector) else {
        tracing::error!(source = config.id, "bad price selector");
        return Vec::new();
    };
    let link_sel = config
        .link_selector
        .as_deref()
        .and_then(|s| Selector::parse(s).ok());

    let doc = Html::parse_document(html);
    let now = Utc::now();
    let mut listings = Vec::new();
    for item in doc.select(&item_sel) {
        let title = item
            .select(&title_sel)
            .next()
            .map(|el| el.text().collect::<String>());
        let price = item
            .select(&price_sel)
            .next()
            .map(|el| el.text().collect::<String>());
        // link: explicit selector, else the item element itself must carry href
        let href = match &link_sel {
            Some(sel) => item.select(sel).next().and_then(|el| el.value().attr("href")),
            None => item.value().attr("href"),
        };
        let (Some(title), Some(price), Some(href)) = (title, price, href) else {
            tracing::debug!(source = config.id, "skipping incomplete listing");
            continue;
        };
        let Ok(url) = base.join(href) else {
            tracing::debug!(source = config.id, href, "skipping unresolvable link");
            continue;
        };
        listings.push(RawListing {
            source_id: config.id.clone(),
            title: title.trim().to_string(),
            price_text: price.trim().to_string(),
            url: url.to_string(),
            scraped_at: now,
        });
    }
    listings
}

/// A declarative source: config + a polite HTTP client.
pub struct GenericSource {
    config: SourceConfig,
    client: ScrapeClient,
}

impl GenericSource {
    pub fn new(config: SourceConfig, client: ScrapeClient) -> Self {
        Self { config, client }
    }
}

#[async_trait::async_trait]
impl DealSource for GenericSource {
    fn id(&self) -> &str {
        &self.config.id
    }

    async fn fetch(&self) -> anyhow::Result<Vec<RawListing>> {
        let mut all = Vec::new();
        for page in 1..=self.config.max_pages {
            let url = page_url(&self.config, page);
            let base = Url::parse(&url)?;
            let request = reqwest::Request::new(reqwest::Method::GET, base.clone());
            let mut client = self.client.clone();
            let response = client
                .ready()
                .await?
                .call(request)
                .await?
                .error_for_status()?;
            let html = response.text().await?;
            let listings = parse_listings(&html, &self.config, &base);
            let empty = listings.is_empty();
            all.extend(listings);
            // stop paginating once a page yields nothing
            if empty {
                break;
            }
            // URL without {page} can't paginate — one fetch only
            if !self.config.url.contains("{page}") {
                break;
            }
        }
        Ok(all)
    }
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p ferret-server generic`
Expected: 2 tests PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/ferret-server
git commit -m "feat(server): DealSource trait + generic declarative scraper with fixture tests"
```

---

### Task 12: Notifier — Notify trait + ntfy implementation

**Files:**
- Create: `crates/ferret-server/src/notify.rs`
- Modify: `crates/ferret-server/src/main.rs`

The trait exists so the pipeline integration test (Task 13) can record notifications instead of hitting the network. The ntfy impl itself is best-effort and is exercised for its URL/header construction only.

- [ ] **Step 1: Write the failing test**

Create `crates/ferret-server/src/notify.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_endpoint_from_url_and_topic() {
        let config = NotificationsConfig {
            ntfy_url: Some(url::Url::parse("https://notify.zeus.balem.fr").unwrap()),
            topic: "deals-zeus".into(),
            token_file: None,
        };
        let notifier = NtfyNotifier::new(&config).unwrap().unwrap();
        assert_eq!(notifier.endpoint.as_str(), "https://notify.zeus.balem.fr/deals-zeus");
    }

    #[test]
    fn disabled_when_url_unset() {
        let notifier = NtfyNotifier::new(&NotificationsConfig::default()).unwrap();
        assert!(notifier.is_none());
    }

    #[test]
    fn empty_topic_is_an_error() {
        let config = NotificationsConfig {
            ntfy_url: Some(url::Url::parse("https://ntfy.sh").unwrap()),
            topic: "".into(),
            token_file: None,
        };
        assert!(NtfyNotifier::new(&config).is_err());
    }
}
```

- [ ] **Step 2: Add `mod notify;` to `main.rs`, run tests to verify failure**

Run: `cargo test -p ferret-server notify`
Expected: FAIL to compile.

- [ ] **Step 3: Write the implementation (above the test module)**

```rust
//! Deal notifications via ntfy. Best-effort by design: a failed publish is
//! a warning in the log, never an error that stalls the pipeline. Fully
//! off (no client, no task) when `[notifications].ntfy_url` is unset.

use std::time::Duration;

use url::Url;

use crate::config::NotificationsConfig;

/// Pipeline-facing notification sink; the integration test provides a
/// recording impl instead of hitting ntfy.
#[async_trait::async_trait]
pub trait Notify: Send + Sync {
    /// Publish one notification. Infallible from the caller's view.
    async fn send(&self, title: &str, message: &str, tags: &str, priority: &str);
}

/// No-op sink used when notifications are disabled.
pub struct NoopNotifier;

#[async_trait::async_trait]
impl Notify for NoopNotifier {
    async fn send(&self, _title: &str, _message: &str, _tags: &str, _priority: &str) {}
}

pub struct NtfyNotifier {
    http: reqwest::Client,
    /// `{ntfy_url}/{topic}` — ntfy publishes with a plain POST to the topic.
    pub(crate) endpoint: Url,
    token: Option<String>,
}

impl NtfyNotifier {
    /// `None` when notifications aren't configured (`ntfy_url` unset).
    pub fn new(config: &NotificationsConfig) -> anyhow::Result<Option<Self>> {
        let Some(base) = config.ntfy_url.clone() else {
            return Ok(None);
        };
        let topic = config.topic.trim();
        anyhow::ensure!(
            !topic.is_empty(),
            "notifications.ntfy_url is set but notifications.topic is empty"
        );
        let endpoint = base
            .join(topic)
            .map_err(|e| anyhow::anyhow!("joining ntfy topic onto {base}: {e}"))?;
        let token = match &config.token_file {
            Some(path) => Some(
                std::fs::read_to_string(path)
                    .map_err(|e| anyhow::anyhow!("reading ntfy token {}: {e}", path.display()))?
                    .trim()
                    .to_string(),
            ),
            None => None,
        };
        Ok(Some(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent(concat!("ferret/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("building ntfy http client"),
            endpoint,
            token,
        }))
    }
}

#[async_trait::async_trait]
impl Notify for NtfyNotifier {
    async fn send(&self, title: &str, message: &str, tags: &str, priority: &str) {
        let mut request = self
            .http
            .post(self.endpoint.clone())
            .header("Title", title)
            .header("Tags", tags)
            .header("Priority", priority)
            .body(message.to_string());
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        match request.send().await {
            Ok(resp) if !resp.status().is_success() => {
                tracing::warn!(status = %resp.status(), "ntfy publish rejected");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "ntfy publish failed"),
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ferret-server notify`
Expected: 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ferret-server
git commit -m "feat(server): Notify trait + best-effort ntfy notifier"
```

---

### Task 13: ETL pipeline + integration test

**Files:**
- Create: `crates/ferret-server/src/pipeline.rs`
- Modify: `crates/ferret-server/src/main.rs`

NOTE: `ferret-server` is a binary crate, so `tests/` integration tests can't reach its modules — the end-to-end test lives in `pipeline.rs`'s `#[cfg(test)]` module instead. The `tests/fixtures/` directory (Task 11) is just fixture storage reached via `include_str!`.

- [ ] **Step 1: Write the failing end-to-end test**

Create `crates/ferret-server/src/pipeline.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    use chrono::Utc;
    use ferret_domain::{ProductFamily, RawListing, WatchRequest};

    use crate::config::ScrapeConfig;
    use crate::db::Db;
    use crate::notify::Notify;

    /// Records every notification instead of publishing.
    #[derive(Default)]
    struct RecordingNotifier {
        sent: Mutex<Vec<(String, String)>>, // (title, tags)
    }

    #[async_trait::async_trait]
    impl Notify for RecordingNotifier {
        async fn send(&self, title: &str, _message: &str, tags: &str, _priority: &str) {
            self.sent.lock().unwrap().push((title.into(), tags.into()));
        }
    }

    fn listing(title: &str, price: &str, url: &str) -> RawListing {
        RawListing {
            source_id: "test-src".into(),
            title: title.into(),
            price_text: price.into(),
            url: url.into(),
            scraped_at: Utc::now(),
        }
    }

    fn families() -> Vec<ProductFamily> {
        vec![ProductFamily {
            name: "nvidia-rtx".into(),
            models: ["3070", "3080", "3090", "4080", "4090"].map(String::from).to_vec(),
        }]
    }

    async fn setup() -> (Db, RecordingNotifier) {
        let db = Db::connect(Path::new(":memory:")).await.unwrap();
        db.create_watch(&WatchRequest {
            name: "RTX 3080".into(),
            family: Some("nvidia-rtx".into()),
            model: Some("3080".into()),
            min_capacity_gb: None,
            max_price_cents: Some(50_000),
            active: true,
        })
        .await
        .unwrap();
        (db, RecordingNotifier::default())
    }

    #[tokio::test]
    async fn matching_listing_is_persisted_and_notified() {
        let (db, notifier) = setup().await;
        let stats = process_listings(
            &db,
            &families(),
            &ScrapeConfig::default(),
            vec![listing("RTX 3080 FE occasion", "450 €", "https://ex.com/1?utm_source=x")],
            &notifier,
        )
        .await
        .unwrap();

        assert_eq!(stats.new_deals, 1);
        assert_eq!(stats.notified, 1);

        let deals = db.list_deals(None).await.unwrap();
        assert_eq!(deals.len(), 1);
        assert_eq!(deals[0].canonical_url, "https://ex.com/1"); // tracking stripped
        assert_eq!(deals[0].price_cents, 45_000);
        assert_eq!(deals[0].models, vec!["3080"]);
        assert!(deals[0].flags.is_empty());

        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].0.contains("RTX 3080"), "notification titled by watch");
    }

    #[tokio::test]
    async fn rescrape_does_not_renotify() {
        let (db, notifier) = setup().await;
        let l = listing("RTX 3080 FE", "450 €", "https://ex.com/1");
        process_listings(&db, &families(), &ScrapeConfig::default(), vec![l.clone()], &notifier)
            .await
            .unwrap();
        let stats =
            process_listings(&db, &families(), &ScrapeConfig::default(), vec![l], &notifier)
                .await
                .unwrap();

        assert_eq!(stats.new_deals, 0);
        assert_eq!(stats.updated_deals, 1);
        assert_eq!(stats.notified, 0, "same deal, no second push");
        assert_eq!(notifier.sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn stuffed_listing_matches_with_flag() {
        let (db, notifier) = setup().await;
        process_listings(
            &db,
            &families(),
            &ScrapeConfig::default(),
            vec![listing(
                "Brackets for 3070 3080 3090 4080 4090",
                "400 €",
                "https://ex.com/stuffed",
            )],
            &notifier,
        )
        .await
        .unwrap();

        let deals = db.list_deals(None).await.unwrap();
        assert_eq!(deals.len(), 1);
        assert!(deals[0].flags.contains(&ferret_domain::Flag::PossibleStuffing));
        // still notified — stuffing is a signal, not a filter
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].1.contains("possible-stuffing"), "flag rides in the tags");
    }

    #[tokio::test]
    async fn unparseable_price_is_skipped() {
        let (db, notifier) = setup().await;
        let stats = process_listings(
            &db,
            &families(),
            &ScrapeConfig::default(),
            vec![listing("RTX 3080", "Contact seller", "https://ex.com/1")],
            &notifier,
        )
        .await
        .unwrap();
        assert_eq!(stats.skipped, 1);
        assert!(db.list_deals(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn price_outlier_flagged_after_history_builds() {
        let (db, notifier) = setup().await;
        // seed 5 unambiguous observations around 450 €
        for (i, price) in [44_000, 45_000, 46_000, 45_500, 44_500].iter().enumerate() {
            db.record_price("nvidia-rtx", "3080", *price).await.unwrap();
            let _ = i;
        }
        process_listings(
            &db,
            &families(),
            &ScrapeConfig::default(),
            vec![listing("RTX 3080 cheap!!", "100 €", "https://ex.com/scam")],
            &notifier,
        )
        .await
        .unwrap();

        let deals = db.list_deals(None).await.unwrap();
        assert!(deals[0].flags.contains(&ferret_domain::Flag::PriceOutlier));
    }
}
```

- [ ] **Step 2: Add `mod pipeline;` to `main.rs`, run tests to verify failure**

Run: `cargo test -p ferret-server pipeline`
Expected: FAIL to compile — `process_listings`, `PipelineStats` not found.

- [ ] **Step 3: Write the implementation (above the test module)**

```rust
//! The ETL pipeline: raw listings → normalize → extract → score → dedupe →
//! watch match → persist → notify. Pure logic lives in ferret-domain; this
//! module only sequences it and talks to storage/notifier.

use chrono::Utc;
use ferret_domain::{attributes, family, matching, normalize, price, Deal, Flag, ProductFamily,
    RawListing};
use uuid::Uuid;

use crate::config::ScrapeConfig;
use crate::db::Db;
use crate::notify::Notify;

/// How many recent observations feed the rolling median.
const PRICE_WINDOW: u32 = 50;

#[derive(Debug, Default, PartialEq)]
pub struct PipelineStats {
    pub new_deals: u64,
    pub updated_deals: u64,
    pub skipped: u64,
    pub notified: u64,
}

/// Process one batch of raw listings from a single scheduler tick.
pub async fn process_listings(
    db: &Db,
    families: &[ProductFamily],
    scrape: &ScrapeConfig,
    listings: Vec<RawListing>,
    notifier: &dyn Notify,
) -> anyhow::Result<PipelineStats> {
    let mut stats = PipelineStats::default();
    let watches = db.list_watches().await?;

    for raw in listings {
        // -- normalize --
        let title = normalize::clean_title(&raw.title);
        let Some((price_cents, currency)) = normalize::parse_price(&raw.price_text) else {
            tracing::debug!(source = raw.source_id, title, "skipping: unparseable price");
            stats.skipped += 1;
            continue;
        };
        let Some(canonical_url) = normalize::canonical_url(&raw.url) else {
            tracing::debug!(source = raw.source_id, url = raw.url, "skipping: bad url");
            stats.skipped += 1;
            continue;
        };

        // -- extract + score --
        let attrs = attributes::extract(&title);
        let fam = family::match_families(&title, families);

        let mut flags = Vec::new();
        if fam.models.len() >= 2 && fam.stuffing_score >= scrape.stuffing_threshold {
            flags.push(Flag::PossibleStuffing);
        }
        // outlier check needs an unambiguous (family, model) identity
        if let (Some(family_name), [model]) = (&fam.family, fam.models.as_slice()) {
            let history = db.recent_prices(family_name, model, PRICE_WINDOW).await?;
            if price::is_outlier(price_cents, &history, scrape.outlier_ratio) {
                flags.push(Flag::PriceOutlier);
            }
        }

        let now = Utc::now();
        let deal = Deal {
            id: Uuid::new_v4(),
            source_id: raw.source_id.clone(),
            canonical_url,
            title,
            price_cents,
            currency,
            family: fam.family.clone(),
            models: fam.models.clone(),
            capacity_gb: attrs.capacity_gb,
            condition: attrs.condition,
            stuffing_score: fam.stuffing_score,
            flags,
            first_seen: now,
            last_seen: now,
        };

        // -- dedupe / persist --
        let (stored, was_new) = db.upsert_deal(&deal).await?;
        if was_new {
            stats.new_deals += 1;
        } else {
            stats.updated_deals += 1;
        }

        // -- price history: only unambiguous listings feed the median,
        //    and only on first sight (re-scrapes would skew it) --
        if was_new
            && !stored.flags.contains(&Flag::PriceOutlier)
            && let (Some(family_name), [model]) = (&stored.family, stored.models.as_slice())
        {
            db.record_price(family_name, model, stored.price_cents).await?;
        }

        // -- match watches + notify --
        for watch in &watches {
            if !matching::watch_matches(watch, &stored) {
                continue;
            }
            let fresh_match = db.insert_match(stored.id, watch.id).await?;
            if !fresh_match {
                continue;
            }
            let mut tags: Vec<String> = vec!["moneybag".into()];
            tags.extend(stored.flags.iter().map(|f| {
                serde_json::to_string(f).expect("flag serializes").trim_matches('"').to_string()
            }));
            let price_eur = stored.price_cents as f64 / 100.0;
            notifier
                .send(
                    &format!("{}: {:.2} {}", watch.name, price_eur, stored.currency),
                    &format!("{}\n{}", stored.title, stored.canonical_url),
                    &tags.join(","),
                    "default",
                )
                .await;
            db.mark_notified(stored.id, watch.id).await?;
            stats.notified += 1;
        }
    }
    Ok(stats)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ferret-server pipeline`
Expected: 5 tests PASS. Gotcha to check if `price_outlier_flagged_after_history_builds` fails: `MIN_HISTORY` is 5 — the test seeds exactly 5 observations.

- [ ] **Step 5: Run the whole workspace suite and commit**

Run: `cargo test --workspace`
Expected: all tests PASS.

```bash
git add crates/ferret-server
git commit -m "feat(server): ETL pipeline with end-to-end integration tests"
```

---

### Task 14: Scheduler — per-source loop, backoff, failure alert

**Files:**
- Create: `crates/ferret-server/src/scheduler.rs`
- Modify: `crates/ferret-server/src/main.rs`

The scheduler's failure accounting is the testable core; extract it as a pure-ish struct so tests don't need timers.

- [ ] **Step 1: Write the failing test**

Create `crates/ferret-server/src/scheduler.rs` with the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn backoff_grows_and_caps() {
        let mut fs = FailureState::new(3);
        assert_eq!(fs.record_failure(), Duration::from_secs(60));
        assert_eq!(fs.record_failure(), Duration::from_secs(120));
        assert_eq!(fs.record_failure(), Duration::from_secs(240));
        for _ in 0..10 {
            fs.record_failure();
        }
        assert_eq!(fs.record_failure(), Duration::from_secs(3600), "capped at 1h");
    }

    #[test]
    fn alerts_once_at_threshold() {
        let mut fs = FailureState::new(3);
        fs.record_failure();
        fs.record_failure();
        assert!(!fs.should_alert());
        fs.record_failure();
        assert!(fs.should_alert(), "alert at the threshold");
        assert!(!fs.should_alert(), "but only once");
        fs.record_failure();
        assert!(!fs.should_alert(), "still silent while failing");
    }

    #[test]
    fn success_resets_everything() {
        let mut fs = FailureState::new(2);
        fs.record_failure();
        fs.record_failure();
        assert!(fs.should_alert());
        fs.record_success();
        assert_eq!(fs.record_failure(), Duration::from_secs(60), "backoff reset");
        fs.record_failure();
        assert!(fs.should_alert(), "alert re-armed after recovery");
    }
}
```

- [ ] **Step 2: Add `mod scheduler;` to `main.rs`, run tests to verify failure**

Run: `cargo test -p ferret-server scheduler`
Expected: FAIL to compile — `FailureState` not found.

- [ ] **Step 3: Write the implementation (above the test module)**

```rust
//! Per-source scheduling: each source runs on its own tokio task and
//! interval — a failing source backs off and alerts, and never blocks or
//! delays other sources.

use std::sync::Arc;
use std::time::Duration;

use ferret_domain::ProductFamily;

use crate::config::ScrapeConfig;
use crate::db::Db;
use crate::notify::Notify;
use crate::pipeline;
use crate::scrape::DealSource;

const BACKOFF_BASE: Duration = Duration::from_secs(60);
const BACKOFF_CAP: Duration = Duration::from_secs(3600);

/// Consecutive-failure accounting for one source: exponential backoff and
/// a single ntfy alert per outage (re-armed on recovery).
pub struct FailureState {
    consecutive: u32,
    alert_after: u32,
    alerted: bool,
}

impl FailureState {
    pub fn new(alert_after: u32) -> Self {
        Self { consecutive: 0, alert_after, alerted: false }
    }

    /// Record a failure; returns how long to back off before the retry.
    pub fn record_failure(&mut self) -> Duration {
        self.consecutive = self.consecutive.saturating_add(1);
        let factor = 2u32.saturating_pow(self.consecutive.saturating_sub(1).min(6));
        (BACKOFF_BASE * factor).min(BACKOFF_CAP)
    }

    /// True exactly once per outage, when the threshold is crossed.
    pub fn should_alert(&mut self) -> bool {
        if !self.alerted && self.consecutive >= self.alert_after {
            self.alerted = true;
            return true;
        }
        false
    }

    pub fn record_success(&mut self) {
        self.consecutive = 0;
        self.alerted = false;
    }
}

/// Spawn one scraping loop per source. Loops run until the process exits
/// (tasks are detached; the axum server owns process lifetime).
pub fn spawn_all(
    sources: Vec<(Arc<dyn DealSource>, Duration)>,
    db: Db,
    families: Arc<Vec<ProductFamily>>,
    scrape: ScrapeConfig,
    notifier: Arc<dyn Notify>,
) {
    for (source, interval) in sources {
        let db = db.clone();
        let families = families.clone();
        let scrape = scrape.clone();
        let notifier = notifier.clone();
        tokio::spawn(async move {
            run_source(source, interval, db, families, scrape, notifier).await;
        });
    }
}

async fn run_source(
    source: Arc<dyn DealSource>,
    interval: Duration,
    db: Db,
    families: Arc<Vec<ProductFamily>>,
    scrape: ScrapeConfig,
    notifier: Arc<dyn Notify>,
) {
    let mut failures = FailureState::new(scrape.failure_alert_after);
    loop {
        match source.fetch().await {
            Ok(listings) => {
                let count = listings.len();
                match pipeline::process_listings(&db, &families, &scrape, listings, notifier.as_ref())
                    .await
                {
                    Ok(stats) => {
                        failures.record_success();
                        tracing::info!(
                            source = source.id(),
                            fetched = count,
                            new = stats.new_deals,
                            updated = stats.updated_deals,
                            skipped = stats.skipped,
                            notified = stats.notified,
                            "tick done"
                        );
                    }
                    Err(e) => {
                        // pipeline (db) errors also count as failures
                        let backoff = failures.record_failure();
                        tracing::error!(source = source.id(), error = %e, ?backoff, "pipeline failed");
                        maybe_alert(&mut failures, source.id(), &e, notifier.as_ref()).await;
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                }
            }
            Err(e) => {
                let backoff = failures.record_failure();
                tracing::warn!(source = source.id(), error = %e, ?backoff, "fetch failed");
                maybe_alert(&mut failures, source.id(), &e, notifier.as_ref()).await;
                tokio::time::sleep(backoff).await;
                continue;
            }
        }
        tokio::time::sleep(interval).await;
    }
}

async fn maybe_alert(
    failures: &mut FailureState,
    source_id: &str,
    error: &anyhow::Error,
    notifier: &dyn Notify,
) {
    if failures.should_alert() {
        notifier
            .send(
                &format!("ferret: source {source_id} is failing"),
                &format!("Repeated scrape failures, backing off.\nLast error: {error}"),
                "warning,ferret",
                "high",
            )
            .await;
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ferret-server scheduler`
Expected: 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ferret-server
git commit -m "feat(server): per-source scheduler with backoff and failure alerts"
```

---

### Task 15: REST API + main wiring

**Files:**
- Create: `crates/ferret-server/src/api.rs`
- Create: `crates/ferret-server/src/state.rs`
- Modify: `crates/ferret-server/src/main.rs`

- [ ] **Step 1: Write `crates/ferret-server/src/state.rs`**

```rust
//! Shared application state for the axum handlers.

use std::sync::Arc;

use ferret_domain::ProductFamily;

use crate::db::Db;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub families: Arc<Vec<ProductFamily>>,
}
```

- [ ] **Step 2: Write the failing API tests + router in `crates/ferret-server/src/api.rs`**

axum routers are testable in-process with `tower::ServiceExt::oneshot`:

```rust
//! REST API. Single-user LAN/tailnet trust model — no auth (same as chaos
//! pre-auth). Errors map: NotFound → 404, Corrupt/Sqlx → 500.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use ferret_domain::WatchRequest;
use serde::Deserialize;
use uuid::Uuid;

use crate::db::DbError;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/watches", get(list_watches).post(create_watch))
        .route("/api/watches/{id}", axum::routing::put(update_watch).delete(delete_watch))
        .route("/api/deals", get(list_deals))
        .route("/api/families", get(list_families))
        .with_state(state)
}

struct ApiError(DbError);

impl From<DbError> for ApiError {
    fn from(e: DbError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            DbError::NotFound => StatusCode::NOT_FOUND,
            _ => {
                tracing::error!(error = %self.0, "api database error");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        (status, self.0.to_string()).into_response()
    }
}

async fn list_watches(State(state): State<AppState>) -> Result<Response, ApiError> {
    Ok(Json(state.db.list_watches().await?).into_response())
}

async fn create_watch(
    State(state): State<AppState>,
    Json(req): Json<WatchRequest>,
) -> Result<Response, ApiError> {
    let watch = state.db.create_watch(&req).await?;
    Ok((StatusCode::CREATED, Json(watch)).into_response())
}

async fn update_watch(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<WatchRequest>,
) -> Result<Response, ApiError> {
    Ok(Json(state.db.update_watch(id, &req).await?).into_response())
}

async fn delete_watch(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    state.db.delete_watch(id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
struct DealsQuery {
    watch_id: Option<Uuid>,
}

async fn list_deals(
    State(state): State<AppState>,
    Query(q): Query<DealsQuery>,
) -> Result<Response, ApiError> {
    Ok(Json(state.db.list_deals(q.watch_id).await?).into_response())
}

async fn list_families(State(state): State<AppState>) -> Response {
    Json(state.families.as_ref().clone()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path as FsPath;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::Request;
    use ferret_domain::Watch;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::db::Db;

    async fn app() -> Router {
        let db = Db::connect(FsPath::new(":memory:")).await.unwrap();
        router(AppState { db, families: Arc::new(Vec::new()) })
    }

    async fn body_json<T: serde::de::DeserializeOwned>(resp: Response) -> T {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn watch_lifecycle_over_http() {
        let app = app().await;

        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/watches")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name": "4TB HDD", "min_capacity_gb": 4000}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let created: Watch = body_json(resp).await;
        assert_eq!(created.name, "4TB HDD");

        let resp = app
            .clone()
            .oneshot(Request::get("/api/watches").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let listed: Vec<Watch> = body_json(resp).await;
        assert_eq!(listed.len(), 1);

        let resp = app
            .clone()
            .oneshot(
                Request::delete(format!("/api/watches/{}", created.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = app
            .oneshot(
                Request::delete(format!("/api/watches/{}", created.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn deals_endpoint_returns_empty_list() {
        let resp = app()
            .await
            .oneshot(Request::get("/api/deals").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let deals: Vec<ferret_domain::Deal> = body_json(resp).await;
        assert!(deals.is_empty());
    }
}
```

Add to `crates/ferret-server/Cargo.toml` `[dev-dependencies]`:

```toml
http-body-util = "0.1"
```

- [ ] **Step 3: Run API tests to verify failure, then wire modules**

Run: `cargo test -p ferret-server api`
Expected: FAIL to compile until `mod api; mod state;` are added to `main.rs` — add them, re-run, tests PASS (2 tests).

- [ ] **Step 4: Write the final `crates/ferret-server/src/main.rs`**

```rust
mod api;
mod config;
mod db;
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
use crate::scrape::generic::GenericSource;
use crate::scrape::DealSource;

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
```

- [ ] **Step 5: Full verification**

Run: `cargo test --workspace`
Expected: every test PASSES.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean (fix anything it raises).

Manual smoke test:

```bash
cargo run -p ferret-server &
sleep 2
curl -s -X POST localhost:4800/api/watches -H 'content-type: application/json' \
  -d '{"name": "RTX 3080", "family": "nvidia-rtx", "model": "3080", "max_price_cents": 50000}'
curl -s localhost:4800/api/watches
curl -s localhost:4800/api/deals
kill %1
```

Expected: POST returns the created watch JSON with an id; GETs return `[...]` with the watch / `[]` for deals.

- [ ] **Step 6: Commit**

```bash
git add crates/ferret-server
git commit -m "feat(server): REST API and main wiring — scheduler, notifier, axum server"
```

---

## Follow-up plans (not in this document)

1. **LLM refinement pass** — `[llm]` config block, structured-output call on ambiguous listings, fail-open merge, mocked-backend tests.
2. **Frontend** — `ferret-client` (typed HTTP client), `ferret-ui` (Leptos components), `ferret-web` (Trunk PWA), `ferret-desktop` (Tauri, Android primary).
3. **Deployment** — `nix/module.nix` NixOS module (`services.ferret`), agenix secrets, flake, justfile — mirroring chaos.
4. **Real sources** — replace `example-board` with actual retailer/board configs; first hand-written `DealSource` plugin (chromiumoxide) when a JS-heavy source lands.
