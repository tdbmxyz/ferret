# Deal moderation: dismiss / ban / restore

The user had no control over matches (a "Dell Optiplex 3080" PC matched
an RTX 3080 watch — the model token "3080" is genuinely in the title).

## States

`deals.moderation` (migration 0007): `none | dismissed | banned`,
orthogonal to the gone/active lifecycle.

- **dismissed** — hidden and unmatched now; clears automatically when
  the listing goes gone and is later re-acquired (fresh chance).
- **banned** — hidden and unmatched forever; survives every re-scrape
  and revive.
- Setting either state also deletes the deal's `deal_matches` rows
  (match counts and notified prices go with them). Restoring to none
  lets the next tick or a watch re-save re-match it as a fresh event.

## Enforcement

- The pipeline's match/notify step skips moderated deals.
- `retro_match` and the guided preview use the default deal listing,
  which excludes moderated deals.
- `GET /api/deals?hidden=true` lists ONLY moderated deals (review view).
- `PUT /api/deals/{id}/moderation {moderation}`.

## UI

Expanded deal cards get dismiss / ban / restore buttons (with hover
explanations); dismissed/banned badges; a "hidden only" toolbar toggle
shows the moderated list for review and unbanning.
