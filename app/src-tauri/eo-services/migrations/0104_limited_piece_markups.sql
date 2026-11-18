-- Limited armour and plates priced piece by piece. Additive and
-- forward-only.
--
-- limited_piece_markups holds the markup the player paid for each limited
-- piece (armour or plate), by the item's exact name as the account names
-- it, one row per value set. A value applies from `effective_from` on; the
-- first value set for a piece has none, so it covers the piece's wear from
-- the start, and every later one applies from when it was set. A row with
-- no markup returns the piece to its TT from then on. Wear in a session is
-- billed at the value in force when the session started.
--
-- account_session_cost_armour_pieces holds, per session measured whole,
-- what each limited piece lost and what it billed: at its markup, or at the
-- TT it lost when none was set. Unlimited armour stays in
-- account_session_cost_armour_parts.
--
-- cost_item_placements may place an item as a limited armour piece or a
-- limited plate: the player's declaration that it is one. SQLite needs a
-- table rebuild to widen a CHECK; every existing row is copied unchanged.

CREATE TABLE limited_piece_markups (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    item_name       TEXT NOT NULL CHECK (length(trim(item_name)) > 0),
    markup_percent  REAL CHECK (markup_percent IS NULL
                                OR (markup_percent >= 100 AND markup_percent <= 100000)),
    effective_from  REAL,
    set_at          REAL NOT NULL
);

CREATE INDEX idx_limited_piece_markups_item ON limited_piece_markups(item_name, set_at);

CREATE TABLE account_session_cost_armour_pieces (
    session_id         TEXT NOT NULL REFERENCES account_session_costs(session_id),
    item_name          TEXT NOT NULL CHECK (length(trim(item_name)) > 0),
    lost_micro_ped     INTEGER NOT NULL CHECK (lost_micro_ped >= 0),
    lost_precision     TEXT NOT NULL CHECK (lost_precision IN ('exact', 'wholePec')),
    markup_hundredths  INTEGER CHECK (markup_hundredths IS NULL OR markup_hundredths >= 10000),
    amount_micro_ped   INTEGER NOT NULL CHECK (amount_micro_ped >= 0),
    precision          TEXT NOT NULL CHECK (precision IN ('exact', 'wholePec')),
    PRIMARY KEY (session_id, item_name)
);

CREATE TABLE cost_item_placements_new (
    item_name TEXT PRIMARY KEY CHECK (length(item_name) > 0 AND item_name = trim(item_name)),
    category  TEXT NOT NULL CHECK (category IN (
        'weapon', 'amplifier', 'attachment', 'mindforceImplant', 'harvestingTool',
        'healingTool', 'healingConsumable', 'unlimitedArmour', 'unlimitedPlate',
        'limitedArmour', 'limitedPlate', 'consumable', 'travel', 'fieldLogistics',
        'notACost'
    )),
    placed_at REAL NOT NULL
);

INSERT INTO cost_item_placements_new (item_name, category, placed_at)
SELECT item_name, category, placed_at FROM cost_item_placements;

DROP TABLE cost_item_placements;

ALTER TABLE cost_item_placements_new RENAME TO cost_item_placements;
