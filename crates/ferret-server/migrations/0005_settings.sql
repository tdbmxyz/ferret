-- Runtime-editable settings (key → JSON value). First user: the "llm"
-- override that supersedes the [llm] TOML section without a restart.
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
