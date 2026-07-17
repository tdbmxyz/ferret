# LLM usability: discovery, probing, resilience, web context

Feedback from the first zeus LLM attempt: it failed silently (blank model
→ TOML default `local` → backend 404, invisible to the user).

## Errors are surfaced, not swallowed

`Interpretation` gains `llm_error` — the LLM step still fails open to
via="none", but the backend's message (kept verbatim, e.g. "404: model
'local' not found") is shown in the guided flow instead of the misleading
"couldn't identify a product".

## Endpoint discovery & probing

- `POST /api/settings/llm/models` → `GET {base_url}/models` (standard
  OpenAI catalog), answers a sorted id list. The ⚙ panel's "List models"
  button fills a dropdown next to the model input.
- `POST /api/settings/llm/test` → one real tiny completion; answers
  `{ok, error}`. "Test" button in the panel.
- Both accept the form's current (unsaved) values; missing fields fall
  back to the effective settings including the stored API key.

## Call resilience

- Backend error bodies are propagated into the error message (no more
  bare `error_for_status`).
- A 4xx while `response_format` is set retries once without it — the
  prompts already demand a bare JSON object (ollama-style backends).
- Answers are trimmed to their outermost `{…}` before parsing, so
  markdown fences and prose around the JSON don't break anything.

## Web context for interpretation

`websearch::snippets` fetches one DuckDuckGo HTML page (key-less, 6 s
timeout, ads filtered, top 5 title—snippet lines) and passes it to the
interpret prompt as `web_search_results`. Strictly fail-open; only runs
when the heuristic missed AND an LLM is configured — never per listing,
never on heuristic hits. Injected as a closure so tests stay offline.
