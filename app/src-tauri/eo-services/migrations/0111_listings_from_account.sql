-- Auction listings the account readings created on their own: one item or
-- stack seen moving into the auction in a short step, the PED Card falling
-- by the listing fee. Additive and forward-only.
--
-- Each row names the listing, the two readings the step lies between
-- (reading_id is the later) and when each was taken (unix seconds), the
-- item, the fee in PEC (the Card's fall, exactly) and the range of starting
-- bids that fee reads back as under the fee model, in PEC. The listing
-- itself stores the range's midpoint as its starting bid; the range stays
-- here, so the figure is never read as more than it is.
--
-- It is written in the same transaction as the listing, and it stays when
-- the listing is taken back: one listing per reading and item, never
-- created again from the same step. Whether it stands is read from the
-- listing itself.

CREATE TABLE listings_from_account (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    listing_id      TEXT NOT NULL UNIQUE CHECK (length(trim(listing_id)) > 0),
    reading_id      INTEGER NOT NULL,
    prev_reading_id INTEGER NOT NULL,
    seen_from       REAL NOT NULL,
    seen_to         REAL NOT NULL,
    item_name       TEXT NOT NULL CHECK (length(trim(item_name)) > 0),
    fee_pec         INTEGER NOT NULL CHECK (fee_pec > 0),
    bid_low_pec     INTEGER NOT NULL CHECK (bid_low_pec >= 0),
    bid_high_pec    INTEGER NOT NULL,
    card_change_pec INTEGER,
    created_at      REAL NOT NULL,
    CHECK (reading_id <> prev_reading_id),
    CHECK (seen_to >= seen_from),
    CHECK (bid_high_pec >= bid_low_pec),
    UNIQUE (reading_id, item_name)
);
