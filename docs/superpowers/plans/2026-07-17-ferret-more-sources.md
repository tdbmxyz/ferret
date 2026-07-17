# ferret Additional Sources — Implementation Plan

**Goal:** Two more hand-written sources ported from veille-prix: **Shopify** stores
(official-store JSON catalogs — the "new" market) and **eBay.fr** (new + occasion).
Both produce the shared `RawListing` shape and ride the existing pipeline.

**Shopify** (`[[shopify]]` config array — multiple stores):
- `{base}/products.json?limit=250&page=N` until an empty page (cap 10 pages) — public
  JSON, no anti-bot, robust.
- One listing per *available* variant: title = `product.title + variant.title` (skip
  the literal "Default Title"), price_text = `"{variant.price} {store.currency}"`,
  url = `{base}/products/{handle}`.
- Config per store: `id`, `url`, `currency` (default EUR), `interval_minutes`
  (default 360 — catalogs move slowly), `delay_ms` (default 1000).
- Tests: fixture JSON (two products, variants incl. unavailable + Default Title),
  price_text→parse_price round-trip, pagination URL.

**eBay.fr** (`[ebay]` config block, veille-prix-validated 2026 layout):
- Search `https://www.ebay.fr/sch/i.html?_nkw={query}&_sop=15`, one page per query.
- Parse `li.s-card` blocks with the `scraper` crate: title `.s-card__title`, price
  `.s-card__price` (must contain EUR/€ — filters out-of-zone cards), link
  `a[href*="/itm/"]` → canonical `https://www.ebay.fr/itm/{id}`, dedupe by item id,
  drop the "Shop on eBay" placeholder card (short/absent title).
- Politeness: veille-prix observed IP limiting after ~10 rapid requests → default
  `delay_ms = 30000`; on 403/429 wait 120 s and retry once, then give up (backoff/alert
  handles the rest). Zero results on a page is fine (not an error) — eBay always
  renders the s-card scaffold; a page with no scaffold at all is a hard error.
- Tests: fixture HTML (2 cards + placeholder + USD card), search URL, empty page.

**Wiring:** both sources join the scheduler like leboncoin; example TOML blocks; live
one-shot validation for Shopify (safe) and eBay (single polite request).
