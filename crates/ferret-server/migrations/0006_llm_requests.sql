-- Every LLM call: what ran, how long, how many tokens. Feeds the UI's
-- "usually takes ~N s" expectation and future budgeting.
CREATE TABLE llm_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,            -- refine | interpret | revise
    model TEXT NOT NULL,
    created_at TEXT NOT NULL,
    duration_ms INTEGER NOT NULL,
    ok INTEGER NOT NULL,
    error TEXT,
    prompt_tokens INTEGER,
    completion_tokens INTEGER
);
CREATE INDEX idx_llm_requests_kind ON llm_requests (kind, ok, id);
