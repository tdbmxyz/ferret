-- User verdict on a deal: none | dismissed | banned. Moderated deals are
-- hidden from lists and excluded from watch matching / notifications.
-- 'dismissed' auto-clears when a gone listing is re-acquired.
ALTER TABLE deals ADD COLUMN moderation TEXT NOT NULL DEFAULT 'none';
