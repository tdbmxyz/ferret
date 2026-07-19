# LLM CoT, revision conversations, prompt editing, About

Feedback after the no-thinking fix worked: thinking should be *allowed*
(CoT improves quality) and handled by ferret, revisions should be
conversational, prompts should be user-editable, and versions visible.

## Chain-of-thought is allowed, ferret absorbs it

`enable_thinking=false` is gone. Instead: token budgets sized for
thoughts + answer (refine 4000, interpret/revise 8000, probe 2048),
per-request timeouts of max(config, 300 s) on interpret/revise (client
330 s), `extract_json` strips `<think>` blocks, `content_of` salvages
answers from `reasoning_content` after a clean stop and reports budget
exhaustion explicitly. Validated live: interpret with full reasoning on
Qwen3.6-27B ≈ 63 s → clean proposal.

## Conversational revision

`revise` carries a `history: Vec<ChatTurn>` — the earlier instructions
and category answers of the exchange. Both the guided proposal card
(new "not quite right? tell the LLM" box) and the category editor keep
the conversation in a signal, so follow-ups like "make the spec you just
added an enum" resolve against context (validated live, two turns).
History resets per search / per editor session.

## Editable system prompts

`GET/PUT/DELETE /api/settings/prompts` (settings key `prompts`): the
three system prompts (interpret / revise / refine) with the factory
defaults alongside. Fields left at the default are stored empty so
future default improvements still apply. ⚙ panel gains textareas with
an "edited" badge, Save, and Reset to defaults. Applied live.

## About tab

Client and server each embed a short git commit via `build.rs`
(`GIT_COMMIT` env for nix sandbox builds — wired in the flake — else
`git rev-parse`). `/api/health` now answers `{status, version, commit}`;
the About tab shows client/server version (commit), server URL, and LLM
state.
