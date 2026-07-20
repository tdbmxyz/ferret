# Aggressive notification noise filtering

Goal (user): avoid noise in notifications. Deals stay visible with their
badges — the aggression is in what matches and what pushes.

## 1. Family context words

`[[families]] context = ["rtx", "geforce", …]`: when non-empty, a model
token only counts if a context word is also in the title (word-bounded,
case-insensitive). "Dell Optiplex 3080" stops matching nvidia-rtx.
Empty list = old behavior. Example configs updated.

## 2. Categories require an alias hit

`categorize` no longer accepts bare enum-value hits: at least one alias
must match (aliases still ×2 in scoring). Family-derived seed categories
now get aliases from the family's context words + name.
NOTE for existing DBs: seeding never overwrites, so categories with
empty or too-specific aliases should be widened via the editor (e.g. the
LLM-proposed gpu category: add "rtx", "geforce", "gpu").

## 3. LLM notification gate

The refine pass now also runs for any unverdicted deal that is about to
match a watch (not just heuristically-ambiguous new deals) — one call
per matched deal, before its first push. Notification policy
(`notification_worthy`):

- verdict genuine → push
- verdict stuffed-title or scam → match recorded, push suppressed
- no verdict (LLM off/failed) → the possible-stuffing flag suppresses

Applies to fresh-match pushes AND price-drop re-notifies. Suppressions
are counted (`TickStats.suppressed`) and logged.
