-- Guided watch creation: product categories with typed spec dimensions.
-- categories.status='proposed' = LLM-drafted, awaiting user review.

CREATE TABLE categories (
    slug       TEXT PRIMARY KEY,
    label      TEXT NOT NULL,
    aliases    TEXT NOT NULL DEFAULT '[]',    -- JSON array
    origin     TEXT NOT NULL DEFAULT 'curated', -- curated | llm
    status     TEXT NOT NULL DEFAULT 'active',  -- active | proposed
    created_at TEXT NOT NULL
);

CREATE TABLE category_specs (
    category_slug   TEXT NOT NULL REFERENCES categories (slug) ON DELETE CASCADE,
    key             TEXT NOT NULL,
    label           TEXT NOT NULL,
    kind            TEXT NOT NULL,              -- number | enum | boolean
    unit            TEXT,
    allowed_values  TEXT NOT NULL DEFAULT '[]', -- JSON array (enum kind)
    extraction_hint TEXT,
    position        INTEGER NOT NULL DEFAULT 0, -- stable display order
    PRIMARY KEY (category_slug, key)
);

ALTER TABLE deals ADD COLUMN category TEXT;
ALTER TABLE deals ADD COLUMN specs TEXT NOT NULL DEFAULT '{}';      -- JSON object
ALTER TABLE watches ADD COLUMN category TEXT;
ALTER TABLE watches ADD COLUMN spec_filters TEXT NOT NULL DEFAULT '[]'; -- JSON array
ALTER TABLE watches ADD COLUMN queries TEXT NOT NULL DEFAULT '[]';      -- JSON array
