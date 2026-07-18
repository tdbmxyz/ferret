# LLM polish: truncation fix, category revision, shared-spec sync

Feedback round: "Test passed but interpretation failed", manual spec
edits are tedious, and renaming a shared spec meant editing 3 categories.

## Why Test ≠ interpret, fixed

- All chat bodies now set explicit `max_tokens` (refine 600, interpret /
  revise 2000): ollama-style backends default to ~128 output tokens,
  which truncated every big JSON answer into a parse error while the tiny
  Test call sailed through.
- Interpret/revise get a per-request timeout of max(configured, 120 s) —
  long generations on slow local models no longer die at the 30 s default.
  The client-side interpret deadline went 60 → 180 s.
- The structured-output retry now covers parse failures too, not just
  4xx: any failed `response_format` attempt is retried once plain, and
  errors accumulate both attempts' messages.
- `extract_json` skips `<think>…</think>` blocks (reasoning models) before
  hunting for the JSON object — braces inside the thoughts can't hijack it.

## LLM category revision

`POST /api/categories/revise {category, instruction}` → the LLM returns
the full revised category (same proposal schema as interpret, now with
`extraction_hint`). Slug, origin, status, created_at are preserved
server-side; nothing persists until the user saves. UI: an "ask the LLM to
rework it" input + button inside the category editor; the answer loads
into the editor fields for review.

## Shared-spec sync

The editor gains a "propagate spec renames to other categories" checkbox
(default on): after saving, specs in other categories with the same key +
kind get the edited label/unit/extraction_hint mirrored onto them.
allowed_values stay per-category. Seeds no longer bake the unit into the
label ("Capacity (GB)" → "Capacity") and the guided controls skip the
"(unit)" suffix when there is none.
