-- Listing outcomes the account readings recorded on their own: a listing
-- that sold, or one that came back from the auction, seen in the step
-- between two consecutive readings. Additive and forward-only.
--
-- Each row names the listing, what became of it, the two readings the step
-- lies between (reading_id is the later) and when each was taken (unix
-- seconds). It is written in the same transaction as the outcome, and it
-- stays when the outcome is taken back: an outcome taken back is never
-- recorded again from the same step, and whether it stands is read from
-- the listing itself.

CREATE TABLE listing_outcomes_from_account (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id      TEXT NOT NULL CHECK (length(trim(listing_id)) > 0),
    outcome         TEXT NOT NULL CHECK (outcome IN ('sold', 'returned')),
    reading_id      INTEGER NOT NULL,
    prev_reading_id INTEGER NOT NULL,
    seen_from       REAL NOT NULL,
    seen_to         REAL NOT NULL,
    created_at      REAL NOT NULL,
    CHECK (reading_id <> prev_reading_id),
    CHECK (seen_to >= seen_from),
    UNIQUE (reading_id, listing_id)
);

CREATE INDEX idx_listing_outcomes_from_account_listing
    ON listing_outcomes_from_account(listing_id, seen_to);
