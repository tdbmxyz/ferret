# Multi-select filters, LLM activity signal, complete zeus config

## Enum filters: any-of

The guided spec controls render enum specs as one checkbox chip per
value instead of a single select. Several checked → `SpecFilter::AnyOf`
(one → `Eq`, none → no filter); `filters_match` already supported it.
Editing a watch round-trips the full selection.

## LLM activity signal + request log

- Migration 0006: `llm_requests` (kind, model, created_at, duration_ms,
  ok, error, prompt_tokens, completion_tokens). Every chat call is timed,
  its token usage captured from the response, and logged fire-and-forget.
- `LlmStatus` gains `busy` (in-flight calls, from a shared atomic) and
  `avg_ms` per kind (average of the last 20 successful calls).
- UI: the sources-strip chip flips to "LLM ⋯ working (n calls)" while
  busy; every LLM button shows a live ticker with the historical
  expectation — "Interpreting… 12s / ~63s".
- Validated live: two concurrent refine calls visible (busy=2),
  avg_ms.refine ≈ 43.5 s recorded from real traffic.

## Ready-to-paste NixOS config

`docs/zeus-config-example.nix`: full `services.ferret` block with every
source built so far — Leboncoin (enabled, baseline queries + watch-driven
ones), Shopify minisforum-eu (validated), eBay (off, with the
fetch_command/Scrapling setup steps), a generic-source template, both
family tables, the LLM base config, and ntfy. The rendered TOML was
booted against the real services as a check.
