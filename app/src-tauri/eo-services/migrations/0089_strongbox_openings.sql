-- A run of strongbox openings, kept as one record over the conversions that
-- carry its accounting.
--
-- Opening boxes is a conversion (boxes and their keys in, what they held
-- out), and choosing to turn some of what came out into Nanocubes is a second
-- conversion per item. The conversions are the books; this row names the run
-- they belong to, so the run can be read back whole and taken back whole.
--
-- `box_markup_paid` is what the opened boxes cost above TT, fixed when the
-- run was recorded: the markup paid on boxes bought for it plus the cost of
-- any held as assets. `intake` says whether the result was read from the
-- account site's Inventory Changes export or entered by hand.
--
-- Additive and forward-only.

CREATE TABLE strongbox_openings (
    id                    TEXT PRIMARY KEY,
    box_item              TEXT NOT NULL,
    box_count             REAL NOT NULL CHECK (box_count > 0),
    box_markup_paid       REAL NOT NULL DEFAULT 0 CHECK (box_markup_paid >= 0),
    intake                TEXT NOT NULL CHECK (intake IN ('export', 'manual')),
    opening_conversion_id TEXT NOT NULL REFERENCES stock_conversions(id),
    opened_at             TEXT NOT NULL,
    created_at            REAL NOT NULL,
    undone_at             TEXT
);

CREATE INDEX idx_strongbox_openings_opened
    ON strongbox_openings(opened_at, created_at);

-- The Nanocube conversions a run made of what it opened, one per item.
CREATE TABLE strongbox_opening_recycles (
    opening_id    TEXT NOT NULL REFERENCES strongbox_openings(id),
    conversion_id TEXT NOT NULL REFERENCES stock_conversions(id),
    PRIMARY KEY (opening_id, conversion_id)
);
