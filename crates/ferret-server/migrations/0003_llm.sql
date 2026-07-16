-- Optional LLM refinement pass: verdict + reason stored alongside the
-- heuristic flags, never replacing them.
ALTER TABLE deals ADD COLUMN llm_verdict TEXT;
ALTER TABLE deals ADD COLUMN llm_reason TEXT;
