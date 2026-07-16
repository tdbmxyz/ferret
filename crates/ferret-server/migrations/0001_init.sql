-- Conventions (same as chaos): UUIDs as hyphenated TEXT, timestamps as
-- RFC3339 TEXT, JSON arrays in TEXT columns, money in integer cents.

CREATE TABLE watches (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    family          TEXT,
    model           TEXT,
    min_capacity_gb INTEGER,
    max_price_cents INTEGER,
    active          INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL
);

CREATE TABLE deals (
    id             TEXT PRIMARY KEY,
    source_id      TEXT NOT NULL,
    canonical_url  TEXT NOT NULL,
    title          TEXT NOT NULL,
    price_cents    INTEGER NOT NULL,
    currency       TEXT NOT NULL,
    family         TEXT,
    models         TEXT NOT NULL DEFAULT '[]',   -- JSON array of model strings
    capacity_gb    INTEGER,
    condition      TEXT,
    stuffing_score REAL NOT NULL DEFAULT 0,
    flags          TEXT NOT NULL DEFAULT '[]',   -- JSON array of Flag
    first_seen     TEXT NOT NULL,
    last_seen      TEXT NOT NULL,
    UNIQUE (source_id, canonical_url)
);

CREATE INDEX deals_family_model ON deals (family);

-- Watch ↔ deal matches; notified guards against duplicate pushes.
CREATE TABLE deal_matches (
    deal_id    TEXT NOT NULL REFERENCES deals (id) ON DELETE CASCADE,
    watch_id   TEXT NOT NULL REFERENCES watches (id) ON DELETE CASCADE,
    matched_at TEXT NOT NULL,
    notified   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (deal_id, watch_id)
);

-- Rolling price observations per (family, exact model) — the basis for
-- outlier detection. Only unambiguous (single-model) listings feed it.
CREATE TABLE price_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    family      TEXT NOT NULL,
    model       TEXT NOT NULL,
    price_cents INTEGER NOT NULL,
    observed_at TEXT NOT NULL
);

CREATE INDEX price_history_family_model ON price_history (family, model, observed_at);
