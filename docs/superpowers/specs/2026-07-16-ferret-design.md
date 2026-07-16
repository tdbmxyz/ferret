# ferret — design spec

Date: 2026-07-16
Status: approved — implementation plan at docs/superpowers/plans/2026-07-16-ferret-core-backend.md

## Purpose

A self-hosted deal tracker: scrapes retailer/marketplace listings and
VPS/hosting-deal boards, extracts structured product attributes, filters out
scam/SEO-stuffed listings, and notifies on genuine matches for saved
product-type watches (e.g. "4TB HDD", "16GB DDR4", "RTX 3080"). Architected
like `yomu` and `chaos`: a Rust workspace with a domain crate, an axum
server, a thin HTTP client, shared Leptos UI, a Trunk web frontend, and a
Tauri shell targeting Android primarily (desktop secondary).

Single-user, self-hosted on zeus — same trust model as chaos (LAN/tailnet,
no auth system needed).

## Why axum (not actix-web)

Carried over from yomu/chaos, and still the right call for ferret
specifically:

- Shared foundation with `reqwest`/`tower-http`/`tonic` (all built on
  `tower`+`hyper`) — lets the scraper's politeness/rate-limit layer be
  written as a `tower::Layer` and reused across the outbound scraping client
  and the inbound API server.
- Plain-function handlers with typed extractors, no macro-based routing —
  easier to unit-test extraction/ETL logic directly.
- Maintained inside `tokio-rs`, tracks tokio/hyper closely.
- actix-web's actor runtime buys nothing here — ferret's workload is
  scheduled background tasks (tokio) plus a small REST API, exactly axum's
  native shape.
- Performance parity is irrelevant at this traffic scale (single-user
  homelab app).

## Crate layout

```
ferret/
  crates/
    ferret-domain/    # Deal, Watch, ProductFamily/sibling tables, matching + scoring logic (no I/O)
    ferret-server/    # axum server: scheduler, DealSource impls, ETL, storage, notifier, REST API
    ferret-client/    # typed HTTP client for the server API
    ferret-ui/        # shared Leptos components/views
    ferret-web/       # Trunk-built web frontend (PWA)
    ferret-desktop/   # Tauri shell — Android primary target, desktop secondary
```

Follows the same crate-per-concern split as chaos, including config style
(TOML, `ferret.example.toml`), migrations (sqlx), and NixOS deployment
module shape (`nix/module.nix`).

## Scraping

`DealSource` trait is the common interface across two implementation styles:

- **Generic declarative scraper** — for static-HTML sources: a config entry
  (URL template + CSS selectors for title/price/link/pagination) interpreted
  by one generic engine built on the `scraper` crate (CSS-selector HTML
  parsing, wraps html5ever) + `reqwest` for HTTP. Covers the common case
  without a code change per source.
- **Hand-written plugins** — for sources needing JS rendering, auth, or
  complex pagination/anti-bot handling: implement `DealSource` directly using
  `chromiumoxide` (headless Chrome via CDP) for JS-heavy sources, or
  `reqwest` directly for simple authenticated APIs.

Both styles produce the same raw listing shape (title, price, currency, url,
source id, scraped_at) consumed by the ETL pipeline below.

**Politeness**: per-source configurable delay/concurrency cap, implemented
as a `tower::Layer` on the scraping `reqwest` client.

**Failure isolation**: each source's `fetch()` runs independently per
scheduler tick; a failing source is retried with backoff and logged, and
does not block or delay other sources. A source failing repeatedly over a
configurable window triggers an ntfy alert (reusing the existing
alerting-pipeline pattern) rather than failing silently.

## ETL pipeline

```
scheduler (tokio interval per source)
  → DealSource::fetch()                     raw listings
  → normalize                               currency → decimal, canonical URL, trimmed title
  → extract attributes                      regex/config-driven: capacity, spec tokens, model, condition
  → family/sibling match                    config-driven ProductFamily tables → stuffing_score
  → [optional LLM pass, ambiguous cases]    see below
  → dedupe                                  by (source, canonical_url); update if changed, insert if new
  → match against active Watches            spec filters + price-outlier check + stuffing_score threshold
  → persist matched Deal                    SQLite via sqlx + migrations
  → notify                                  ntfy publish, topic `deals-zeus`, tagged with flags
  → UI                                      client queries server API, lists deals per watch with flag badges
```

- **Rolling median price** is computed per (product family + exact
  spec/model), not per watch, so outlier detection stays meaningful as more
  watches are added.
- **Stuffing score is a signal, not a hard filter**: a listing matching the
  watched model (e.g. "3080") still surfaces even when it also enumerates
  sibling models (e.g. "2080 3080 3090 4080 4090 5080 5090") — it's tagged
  `possible-stuffing` for the user to eyeball, never silently dropped.
- **Product family/sibling tables are config-driven** (TOML, loaded at
  runtime), not hardcoded — consistent with chaos.toml/servicesList — so
  adding e.g. a new GPU generation doesn't require a code change.

## Optional LLM refinement pass

Gated behind the heuristic pass, only invoked on **ambiguous** listings
(inconclusive regex extraction, or borderline stuffing score) — the common
case never touches the LLM.

- **Backend**: configurable, an `[llm]` block (`enabled`, `base_url`,
  `model`, optional `api_key_file`) — defaults to the self-hosted llama-cpp
  instance already running on zeus, swappable to any OpenAI-compatible
  external API.
- **Task**: a single structured-output (JSON-schema-constrained) call per
  ambiguous listing that both (a) refines attribute extraction where regex
  was inconclusive, and (b) returns a relevance verdict (genuine match /
  stuffed-title / scam) with a short reason.
- **Fail-open**: LLM errors/timeouts fall back to the heuristic-only
  verdict — the LLM is a refinement layer, never a hard dependency for the
  pipeline to proceed.
- **Storage**: LLM verdict + reason are stored alongside heuristic flags,
  never overwriting them — the UI surfaces both signals independently.

## Notifications & deployment

- ntfy topic `deals-zeus` on the existing self-hosted ntfy instance
  (`notify.zeus.balem.fr`), same auth/token pattern as chaos's
  `chaos-zeus` topic and alertmanager's `alerts-zeus` topic.
- Deployed on zeus via a NixOS module (`nix/module.nix`) following the same
  shape as `chaos`'s: `services.ferret` options, `$PUBLIC_DOMAIN`
  envsubst-substituted config, agenix-managed secrets (LLM API key if an
  external backend is configured, ntfy token).

## Testing

- **ETL unit tests**: attribute extraction, family/sibling matching, stuffing
  scoring, price-outlier detection — pure functions in `ferret-domain`, no
  I/O, straightforward to test exhaustively.
- **Scraper tests**: fixture-based HTML snapshot tests per source (saved
  HTML fixtures → expected parsed listings), mirroring yomu-source's
  `tests/` pattern — catches selector breakage without hitting live sites.
- **Pipeline integration test**: full ETL flow with mocked `DealSource`
  implementations, verifying dedupe/matching/notification behavior
  end-to-end without network access.
- **LLM pass**: tested with a mocked backend returning canned structured
  responses — verifies merge/fail-open behavior, not model quality.

## Out of scope (v1)

- Arbitrary per-URL price tracking (v1 is product-type watches only, per
  explicit requirement).
- Multi-user/auth (single-user, same trust model as chaos).
- Seller/listing-metadata-based trust scoring (deferred; price-outlier +
  stuffing heuristics only for v1, per the approaches discussion).
