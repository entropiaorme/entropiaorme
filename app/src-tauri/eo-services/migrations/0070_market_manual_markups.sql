-- Player-supplied markup and live-valuation marks: informational market data,
-- like every market_* table. Nothing here references an accounting table, and
-- no accounting table references these.

-- A markup the player enters by hand for one item: a percentage of TT, or a
-- PED amount above TT per unit (how the auction quotes items that do not
-- stack). Append-only: the latest row per item stands, and a 'cleared' row
-- withdraws the entry. Against a market import, whichever was recorded more
-- recently is the one used. unit_tt_ped is the item's TT per unit where the
-- entry was made, so a per-unit entry still reads as a percentage elsewhere.
CREATE TABLE market_manual_markups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    item_name TEXT NOT NULL CHECK (trim(item_name) != ''),
    kind TEXT NOT NULL CHECK (kind IN ('percent', 'ped_per_unit', 'cleared')),
    value REAL,
    unit_tt_ped REAL CHECK (unit_tt_ped IS NULL OR unit_tt_ped >= 0),
    recorded_at REAL NOT NULL,
    CHECK (
        (kind = 'cleared' AND value IS NULL)
        OR (kind = 'percent' AND value > 0)
        OR (kind = 'ped_per_unit' AND value >= 0)
    )
);

CREATE INDEX idx_market_manual_markups_item_recorded
    ON market_manual_markups(item_name, recorded_at, id);

-- Items the player marked to be valued at the Nanocube markup in the live
-- session figures, whatever their own market reading says. One row per marked
-- item; unmarking deletes it.
CREATE TABLE market_live_nanocube_marks (
    item_name TEXT PRIMARY KEY CHECK (trim(item_name) != ''),
    marked_at REAL NOT NULL
);
