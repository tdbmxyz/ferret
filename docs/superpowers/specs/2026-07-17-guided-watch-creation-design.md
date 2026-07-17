# ferret — Guided watch creation (Part B) design

Date: 2026-07-17 · Status: approved (brainstorming with user)

## Vision (user's flow)

Single text input ("4TB HDD") → ferret interprets what product is meant →
confirmation screen with sample offers (cancel if nothing matches) → typed
filters on the category's characteristics → saved watch, editable later.

Decisions from brainstorming: offers are **DB-first with live search
streaming in behind**; categories are **curated seed + LLM-proposed new ones
that the user reviews before activation**.

## Data model

```
categories      slug PK, label, aliases (JSON array), origin (curated|llm),
                status (active|proposed), created_at
category_specs  (category_slug FK, key) PK, label, kind (number|enum|boolean),
                unit, allowed_values (JSON, enums), extraction_hint
deals           + category, specs (JSON object key→value)
watches         + category, spec_filters (JSON array), queries (JSON array)
```

`SpecFilter { key, op: min|max|eq|any_of, value }` — typed, evaluated by a
pure domain function over `deal.specs`.

**Families fold in:** `[[families]]` TOML seeds categories carrying a `model`
enum-spec; stuffing detection and price history derive sibling lists from
category specs (config remains a seed source, not a second system).

## Flow

1. `POST /api/interpret {text}` — alias/keyword heuristics answer instantly
   when confident; else the local LLM (existing fail-open client) maps text →
   known category + constraint filters ("4TB" → capacity ≥ 4000). No match →
   LLM drafts a proposed category+specs (status=proposed) for user review.
2. Confirmation: stored deals matching the interpretation render at once; a
   background search job fans the derived queries to query-capable sources,
   results stream in (deals persist through the normal pipeline; the ad-hoc
   path NEVER runs the gone-marking lifecycle). Cancel = no watch, proposal
   discarded.
3. Spec filters render from `category_specs`: number → min/max inputs, enum →
   chips, boolean → toggle.
4. Save: watch stores category, spec_filters, queries (editable). Active
   watches' queries join the scheduled rotation via a shared query set
   (config ∪ watch queries, deduped, capped at 20) read by Leboncoin/eBay/
   `{query}` generic sources each tick.
5. Edit: same screen pre-filled.

## Matching & extraction

`watch_matches` extends: category set → deal.category must equal AND all
spec_filters pass. Legacy fields (family/model/capacity/price bounds) keep
working. Deal categorization at scrape time: model-spec hit or word-bounded
alias hit; spec values extracted per category_specs (numbers by unit regex,
enums by allowed-value match), LLM refinement fills gaps for ambiguous
listings (existing gate).

## API

- `GET/POST/PUT/DELETE /api/categories` (+ approve = PUT status active)
- `POST /api/interpret`
- `POST /api/searches {queries}` → job id; `GET /api/searches/{id}` →
  per-source pending/done(n)/error. Jobs live in AppState; results land as
  ordinary deals.

## Phasing

B1 schema + seeds + categorization/extraction + matching · B2 interpret +
guided create UI (DB offers only) · B3 background search jobs + streaming ·
B4 proposal review UI. Each phase ships usable.

## Testing

Domain: spec-filter eval, extraction per kind, categorization. Server:
interpret with mocked LLM (map / propose / fail-open), search-job flow with
mocked sources (no gone-marking), family→category seeding, watch queries
joining the rotation. UI compiles for wasm; manual flow smoke test.

## Risks

Watch-derived queries raise scrape volume (dedupe + cap; eBay stays behind
its stealth fetch_command). LLM latency on interpret (heuristics first,
spinner for the LLM path).
