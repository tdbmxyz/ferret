-- Deal lifecycle (patterns adopted from ent/veille-prix):
--   * deals.status — 'gone' when a successful scrape no longer sees the
--     listing; revives to 'active' if it reappears. Never deleted.
--   * deal_prices — dated per-deal price history (one row per day, latest
--     wins), basis for price-drop re-notifications.
--   * deal_matches.notified_price_cents — price at last notification, so a
--     drop can be measured and re-notified.
--   * watches.min_price_cents — plausibility floor (filters accessories and
--     scam placeholder prices that title-match the product).

ALTER TABLE deals ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE watches ADD COLUMN min_price_cents INTEGER;
ALTER TABLE deal_matches ADD COLUMN notified_price_cents INTEGER;

CREATE TABLE deal_prices (
    deal_id     TEXT NOT NULL REFERENCES deals (id) ON DELETE CASCADE,
    day         TEXT NOT NULL,
    price_cents INTEGER NOT NULL,
    PRIMARY KEY (deal_id, day)
);
