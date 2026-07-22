# Watch price-history charts + explicit report outcomes

## Watch charts (chaos-style)

The temperature-chart experience from chaos, applied to a watch's
prices: `GET /api/watches/{id}/prices` aggregates the watch's matched
deals' `deal_prices` per day into `{day, min_cents, median_cents,
count}`. The UI ports chaos's vendored Apache ECharts + `ChartCanvas`
(trimmed: no connect-groups, no window tooltip formatters) and renders
two lines — "best" (accent; the price you could pay that day) and
"median" (muted; the market) — with axis tooltip, wheel zoom, dblclick
reset, theme colors from CSS vars. A "history" button on each watch row
expands the chart. Option builder is pure JSON, unit-tested.

## Every deal row states its report outcome

`GET /api/deals` rows become `DealRow { ..deal (flattened), matches:
[{watch_id, watch_name, notified_price_cents}] }` (extra field —
backwards compatible for old clients). The card's first badge is now a
definitive verdict for every row:

- "reported: <watches>" (ok) — a notification went out (or the match
  was armed by a retro-match summary).
- "matched <watches> — muted: <reason>" (warn) — reason named from the
  same precedence as the gate: wanted ad / stuffed title / scam
  verdict / not the product / possible stuffing without LLM.
- "not reported: no watch matches" (muted).
- "not reported: dismissed/banned by you" (muted).

Validated live on the acceptance DB: 3 days of chart aggregates,
✓/✗ outcomes per matched deal.
