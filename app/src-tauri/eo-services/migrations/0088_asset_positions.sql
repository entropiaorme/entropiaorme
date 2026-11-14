-- Assets held by quantity, built up from purchases at a running average cost.
--
-- An asset row is now a position: how many units are held (`quantity`) with
-- the TT and markup paid of those units as totals. Each purchase that built it
-- is kept in `asset_purchases`, so buying more moves the average and the
-- history of what was paid stays readable.
--
-- Selling, listing, or converting part of a position carves the units out as a
-- row of their own (`position_id` names the position they came from), carrying
-- the position's average cost for that share. The carved row then goes through
-- the same whole-asset lifecycle as any other. Units that come back unsold
-- rejoin their position and the carved row is retired as `merged`, so its
-- listing history still has a row to point at.
--
-- `drawn_through` is the last purchase of its position a disposal's cost drew
-- on. A purchase at or before it is fixed while that disposal stands, because
-- a recorded result was worked out from what it cost.
--
-- Additive and forward-only: every existing asset becomes a position of one
-- unit with one purchase.

ALTER TABLE inventory_items ADD COLUMN quantity REAL NOT NULL DEFAULT 1
    CHECK (quantity > 0);
ALTER TABLE inventory_items ADD COLUMN position_id TEXT;
ALTER TABLE inventory_items ADD COLUMN drawn_through INTEGER;

CREATE INDEX idx_inventory_items_position
    ON inventory_items(position_id)
    WHERE position_id IS NOT NULL;

-- Widening the state vocabulary needs a new column, as when converted and
-- removed were added: the table is not rebuilt, because databases differ in
-- which historical columns it carries.
DROP INDEX idx_inventory_items_state;

ALTER TABLE inventory_items ADD COLUMN state_widened TEXT NOT NULL DEFAULT 'held'
    CHECK (state_widened IN ('held', 'listed', 'sold', 'converted', 'removed', 'merged'));
UPDATE inventory_items SET state_widened = state;
ALTER TABLE inventory_items DROP COLUMN state;
ALTER TABLE inventory_items RENAME COLUMN state_widened TO state;

CREATE INDEX idx_inventory_items_state
    ON inventory_items(state, acquired_at DESC);

-- Every acquisition of a position: a purchase the player entered, or the
-- output of a conversion (`conversion_id`). The position's totals are the sum
-- of its purchases less whatever has been carved out of it and not returned.
CREATE TABLE asset_purchases (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    position_id   TEXT NOT NULL,
    quantity      REAL NOT NULL CHECK (quantity > 0),
    tt_value      REAL NOT NULL,
    markup_paid   REAL NOT NULL,
    acquired_at   TEXT NOT NULL,
    conversion_id TEXT,
    created_at    REAL NOT NULL
);

CREATE INDEX idx_asset_purchases_position
    ON asset_purchases(position_id, created_at);

INSERT INTO asset_purchases
    (position_id, quantity, tt_value, markup_paid, acquired_at, conversion_id, created_at)
SELECT
    i.id,
    i.quantity,
    i.tt_value,
    i.markup_paid,
    i.acquired_at,
    (SELECT l.conversion_id FROM stock_conversion_lines l
     WHERE l.side = 'output' AND l.inventory_item_id = i.id LIMIT 1),
    COALESCE(i.updated_at, unixepoch('now'))
FROM inventory_items i;

-- What has already left holdings drew on the purchase just recorded for it.
UPDATE inventory_items
SET drawn_through = (
    SELECT MAX(p.id) FROM asset_purchases p WHERE p.position_id = inventory_items.id
)
WHERE state <> 'held';
