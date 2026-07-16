# ferret Leboncoin Source Plugin — Implementation Plan

**Goal:** First hand-written `DealSource`: Leboncoin (France's main second-hand marketplace), ported from the proven approach in `/projects/ent/outils/veille-prix`.

**Approach (validated by veille-prix in production):**
- Search pages (`https://www.leboncoin.fr/recherche?text=<query>&page=N`) embed a complete
  `__NEXT_DATA__` JSON blob — parse `props.pageProps.searchData.ads` instead of the HTML.
  A search with no results has NO `ads` key (observed 2026-07-05) — that's empty, not an error.
- Leboncoin sits behind DataDome, which fingerprints the client TLS stack: plain HTTP
  clients get 403 while curl passes. Port veille-prix's fallback: try reqwest (through the
  politeness layer, browser-like UA + fr Accept-Language); on HTTP 403/429, retry the same
  URL via `curl -sL --fail --compressed` (tokio subprocess).
- Ads: `subject` → title, `price_cents` (or `price[0]` euros) → price_text `"NNN.NN €"`,
  `url` → listing URL, skip ads whose `status != "active"`.

**Config:** dedicated `[leboncoin]` block (hand-written plugin ≠ declarative `[[sources]]`):
`enabled` (default false), `queries`, `pages_per_query` (default 2), `delay_ms` (default 2000),
`interval_minutes` (default 30). Source id: `"leboncoin"`.

**Files:**
- `config.rs`: `LeboncoinConfig` + `Config.leboncoin`; example TOML block
- `scrape/leboncoin.rs`: `search_url()`, `parse_search_page()` (pure), `curl_args()` (pure),
  `LeboncoinSource` (DealSource impl with curl fallback)
- `tests/fixtures/leboncoin_search.html`: minimal realistic `__NEXT_DATA__` fixture
- `main.rs`: build the source when enabled

**Tests:** fixture parse (2 active ads parsed, inactive ad skipped, price_cents and
price-array variants both handled); missing `ads` key → empty; missing `__NEXT_DATA__` →
error (blocked page must count as a fetch failure for backoff/alerting); `search_url`
encoding; `curl_args` shape; config defaults + example parse. Network fetch itself is not
unit-tested (scheduler backoff + ntfy alert cover live breakage).
