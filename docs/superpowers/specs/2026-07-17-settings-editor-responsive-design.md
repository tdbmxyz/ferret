# POC feedback round 2: LLM settings, spec editor, mobile layout

Three refinements from using the guided-creation POC on a phone.

## 1. LLM configuration ("Both", per user decision)

The `[llm]` TOML section stays the base (NixOS/agenix-friendly). A DB-stored
override — settings table, key `llm` — supersedes it field by field and is
editable from the UI's ⚙ panel:

- `GET /api/settings/llm` → effective view `{enabled, base_url, model,
  api_key_set, from_override}` (the key itself never leaves the server).
- `PUT /api/settings/llm` `{enabled, base_url, model, api_key?}` — replaces
  the override wholesale; `api_key` omitted keeps the stored key, `""`
  clears it. Blank url/model fall back to the TOML values.
- `DELETE /api/settings/llm` — drop the override, back to TOML.

Changes apply live: `AppState.llm` is an `Arc<RwLock<LlmRuntime>>` holding
the refiner + interpreter trait objects; the scheduler re-reads it each
tick and the API handlers per request, so no restart is needed.

Honesty in the UI: `/api/status` gains `llm: {enabled, model}` (chip in the
sources strip: "LLM ✓ model" / "LLM off"), and `/api/interpret` gains
`llm_active` so a via="none" answer can say "no LLM configured — heuristics
only" instead of a generic "couldn't identify".

## 2. Category / spec editor

The Categories tab gains a full editor (create + edit): label, aliases,
and the spec table — per spec: key, label, kind (number/enum/boolean),
unit (number), allowed values (enum), yes-keywords hint (boolean), with
add/remove rows. Slug is immutable once created. Saving goes through the
existing `POST /api/categories` upsert. New `CategoryOrigin::User` marks
hand-made categories.

## 3. Mobile layout

No horizontal page scroll on phones: global `min-width: 0` +
`overflow-x: hidden` backstop, wrapping flex rows (guided input, save row,
deal/watch rows, spec controls), tab nav scrolls inside the top bar,
`overflow-wrap: anywhere` on deal titles, sparkline scales down.
