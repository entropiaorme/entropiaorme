-- Stock of unknown origin: something the account readings saw arrive
-- during a tracked session that nothing the app recorded explains (no loot
-- line, no recorded quest reward, no conversion, no opening), which the
-- player says to hold as stock without knowing where it came from. Additive
-- and forward-only.
--
-- Each row is one such record; its stock is the unattributed movement it
-- writes (movement_kind 'opening_balance', ref_id the record's id), which
-- funds no activity. Taking it back removes that movement and marks the
-- record undone; the row stays, so whether the record still stands is read
-- from it.
--
-- proposal_id names the arrival proposal it was recorded from, when it was:
-- recording that arrival again while this record stands writes nothing new,
-- and an arrival whose record stands but was never marked recorded keeps it
-- as its handle.

CREATE TABLE stock_unknown_origins (
    id          TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
    item_name   TEXT NOT NULL CHECK (length(trim(item_name)) > 0),
    quantity    REAL NOT NULL CHECK (quantity > 0),
    tt_value    REAL NOT NULL CHECK (tt_value >= 0),
    found_at    TEXT NOT NULL,
    created_at  REAL NOT NULL,
    undone_at   TEXT,
    proposal_id INTEGER
);
