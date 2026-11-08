-- Many-to-many item conversions: hand-ins, opened boxes, and anything else
-- where several things go in and something else comes out.
--
-- A conversion's activity composition rides forward onto what it produced, as
-- recycling has always done, so a later sale credits the gameplay that earned
-- the inputs. Three facts now travel with converted stock beyond its quantity
-- and TT: markup actually paid for bought inputs (a cost the sale must recover
-- before anything is gained), markup a player has declared an input of their
-- own to be worth (a claim settled only when a sale confirms it), and the root
-- item and count the stock descends from, so a breakdown can name it.

ALTER TABLE stock_conversions ADD COLUMN recognised_markup REAL;
ALTER TABLE stock_conversions ADD COLUMN ped_amount REAL;
ALTER TABLE stock_conversions ADD COLUMN itemised INTEGER NOT NULL DEFAULT 0
    CHECK (itemised IN (0, 1));

-- What went in and came out, as the player recorded it. Retained after an undo
-- as the record of what was corrected.
CREATE TABLE stock_conversion_lines (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    conversion_id     TEXT NOT NULL REFERENCES stock_conversions(id),
    side              TEXT NOT NULL CHECK (side IN ('input', 'output')),
    position          INTEGER NOT NULL,
    item_name         TEXT NOT NULL,
    quantity          REAL NOT NULL CHECK (quantity > 0),
    tt_value          REAL NOT NULL CHECK (tt_value >= 0),
    -- Inputs only: drawn from stock, bought for this, or a whole asset.
    acquisition       TEXT CHECK (acquisition IN ('held', 'bought', 'asset')),
    -- Inputs only: markup paid on a bought line, or declared worth on one of
    -- the player's own.
    markup            REAL CHECK (markup >= 0),
    -- Outputs only: which side of Inventory the result joined.
    holding           TEXT CHECK (holding IN ('loot', 'asset')),
    -- The asset consumed by an input, or created by an output.
    inventory_item_id TEXT
);

CREATE INDEX idx_stock_conversion_lines_conversion
    ON stock_conversion_lines(conversion_id, side, position);

-- The immutable split of whatever a conversion recognised on the day, per
-- source. A sale's split is derived from its movement rows; a conversion has
-- no such outflow to read once its value has been settled in PED.
CREATE TABLE stock_conversion_credits (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    conversion_id         TEXT NOT NULL REFERENCES stock_conversions(id),
    source_kind           TEXT NOT NULL CHECK (source_kind IN ('harvest', 'hunt', 'quest')),
    yield_tier            TEXT,
    mob_species           TEXT,
    session_definition_id INTEGER,
    quest_id              INTEGER,
    activity_context_id   INTEGER,
    origin_item           TEXT,
    origin_quantity       REAL,
    amount                REAL NOT NULL
);

CREATE INDEX idx_stock_conversion_credits_conversion
    ON stock_conversion_credits(conversion_id);

ALTER TABLE stock_movements ADD COLUMN paid_markup REAL NOT NULL DEFAULT 0;
ALTER TABLE stock_movements ADD COLUMN declared_markup REAL;
ALTER TABLE stock_movements ADD COLUMN source_line_id INTEGER
    REFERENCES stock_conversion_lines(id);
ALTER TABLE stock_movements ADD COLUMN origin_item TEXT;
ALTER TABLE stock_movements ADD COLUMN origin_quantity REAL;
-- Set on the provenance rows of an output held as a whole asset. Those rows
-- describe where the asset came from and are never part of the fungible stock
-- arithmetic.
ALTER TABLE stock_movements ADD COLUMN inventory_item_id TEXT;

CREATE INDEX idx_stock_movements_inventory_item
    ON stock_movements(inventory_item_id)
    WHERE inventory_item_id IS NOT NULL;

ALTER TABLE private_sales ADD COLUMN paid_markup REAL NOT NULL DEFAULT 0;

-- An asset can now be consumed by a conversion, or leave the list while its
-- origin record stays. Widening a CHECK needs a new column: the table is not
-- rebuilt, because databases differ in which historical columns it carries and
-- a rebuild would have to name them all.
DROP INDEX idx_inventory_items_state;

ALTER TABLE inventory_items ADD COLUMN state_widened TEXT NOT NULL DEFAULT 'held'
    CHECK (state_widened IN ('held', 'listed', 'sold', 'converted', 'removed'));
UPDATE inventory_items SET state_widened = state;
ALTER TABLE inventory_items DROP COLUMN state;
ALTER TABLE inventory_items RENAME COLUMN state_widened TO state;

CREATE INDEX idx_inventory_items_state
    ON inventory_items(state, acquired_at DESC);
