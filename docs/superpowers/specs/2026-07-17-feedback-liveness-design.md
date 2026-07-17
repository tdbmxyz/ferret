# ferret — Feedback & liveness (Part A) design

Date: 2026-07-17 · Status: approved (brainstorming with user)

## Problem

Creating a watch gives no feedback: matches only happen at scrape time (no
retro-matching), scheduler status lives only in logs, no acknowledgment push,
and the per-deal price history (`deal_prices`) is collected but never shown.

## Design

**Retro-matching.** `POST/PUT /api/watches` runs `watch_matches` over all
existing ACTIVE deals: inserts `deal_matches` rows marked notified at the
deal's current price (arms drop-re-notification without spamming N pushes),
then sends ONE summary ntfy: "Watch 'X' created — N existing deals match,
best 419,99 €" (or "no existing deals match — sources will pick it up").
The UI shows matches immediately after save.

**Status surface.** Scheduler writes per-source `SourceStatus` (last tick
time, last stats or error string, interval, consecutive failures) into a
shared `Arc<RwLock<HashMap<source_id, SourceStatus>>>` in `AppState`.
`GET /api/status` returns `{ sources: [SourceStatus], watch_matches:
{watch_id: count} }`. UI: a sources strip on the Deals view ("leboncoin ✓ 35
listings, 2 min ago · ebay ✗ blocked"), match counts on watch rows.

**Price history plot.** Deal cards expand on click; the expanded card lazily
fetches `/api/deals/{id}/prices` and renders an inline SVG sparkline
(no chart library) with first/last/min labels. One observation renders as a
dot, not a line.

## Components

- `ferret-domain`: `SourceStatus`, `TickStats`, `StatusResponse` types.
- `ferret-server`: `watches::retro_match` (db + notify), status registry in
  state + scheduler writes, `/api/status`; AppState gains the notifier.
- `ferret-client`: `status()` method.
- `ferret-ui`: sources strip, watch counts, expandable deal card + sparkline.

## Testing

Retro-match unit tests (matches inserted, notified price armed, summary push
recorded, no per-deal pushes); status endpoint test; sparkline path
generation pure-function test.
