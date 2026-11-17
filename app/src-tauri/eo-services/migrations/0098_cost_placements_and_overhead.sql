-- What a session cost beyond performing its activity (its overhead: travel,
-- field logistics, and what the app cannot place yet) joins the session's
-- costs, and an item the app cannot place can be placed by the player once.
-- Additive and forward-only.
--
-- cost_item_placements holds the player's own placement of an item, by its
-- exact name: the category its use costs as from then on, in every session,
-- or 'notACost' for an item whose use is no cost at all. It answers before
-- the equipment library and the game data.
--
-- The stored cost lines gain the category 'travel' (an item the player
-- placed as travel: a vehicle, its fuel), the reason 'notPlaced' (units of
-- a stack nothing names, used up: listed for the player to place, and no
-- cost until they do) and, for a limited piece's line, the confirmed set
-- that priced it (NULL while no confirmed set holds the piece). SQLite needs a table rebuild to widen a CHECK; every existing row
-- is copied unchanged with every column in its place, its set unknown
-- (the rules' version moves with this change, so every stored projection is
-- rebuilt and its lines name their sets again).
--
-- What a session's overhead resolves to is kept on its resolution, by
-- category: the measured amount in PED, or NULL where the category was not
-- measured. Its summary keeps the total, and each day's rollup the sum of
-- its sessions' overhead by category, on each session's start day.
PRAGMA foreign_keys = OFF;

CREATE TABLE cost_item_placements (
    item_name TEXT PRIMARY KEY CHECK (length(item_name) > 0 AND item_name = trim(item_name)),
    category  TEXT NOT NULL CHECK (category IN (
        'weapon', 'amplifier', 'attachment', 'mindforceImplant', 'harvestingTool',
        'healingTool', 'healingConsumable', 'unlimitedArmour', 'unlimitedPlate',
        'consumable', 'travel', 'fieldLogistics', 'notACost'
    )),
    placed_at REAL NOT NULL
);

CREATE TABLE account_session_cost_lines_new (
    session_id       TEXT NOT NULL REFERENCES account_session_costs(session_id),
    position         INTEGER NOT NULL CHECK (position >= 0),
    bucket           TEXT NOT NULL CHECK (bucket IN (
        'weapon', 'enhancer', 'harvesting', 'healing', 'armour', 'consumables', 'travel',
        'fieldLogistics', 'unclassified'
    )),
    category         TEXT NOT NULL CHECK (category IN (
        'ammo', 'weapon', 'amplifier', 'attachment', 'mindforceImplant', 'enhancer',
        'harvestingTool', 'healingTool', 'healingConsumable', 'unlimitedArmour',
        'unlimitedPlate', 'limitedArmour', 'limitedPlate', 'consumable', 'teleportChip',
        'mindEssence', 'fieldLogistics', 'travel', 'unclassified'
    )),
    kind             TEXT NOT NULL CHECK (kind IN (
        'ammoBurned', 'decay', 'limitedDecay', 'consumed'
    )),
    item_name        TEXT NOT NULL CHECK (length(trim(item_name)) > 0),
    units            INTEGER CHECK (units IS NULL OR units > 0),
    amount_micro_ped INTEGER,
    precision        TEXT CHECK (precision IN ('exact', 'wholePec')),
    markup_hundredths INTEGER CHECK (markup_hundredths IS NULL OR markup_hundredths > 0),
    reason           TEXT CHECK (reason IN ('boughtThroughLedger', 'notPlaced')),
    set_id           INTEGER REFERENCES protection_sets(id),
    PRIMARY KEY (session_id, position),
    CHECK ((kind IN ('ammoBurned', 'consumed')) = (units IS NOT NULL)),
    CHECK ((amount_micro_ped IS NULL) = (precision IS NULL)),
    CHECK ((amount_micro_ped IS NULL) = (reason IS NOT NULL)),
    CHECK (kind = 'consumed' OR (reason IS NULL AND markup_hundredths IS NULL)),
    CHECK (markup_hundredths IS NULL OR amount_micro_ped IS NOT NULL),
    CHECK (set_id IS NULL OR kind = 'limitedDecay')
);

INSERT INTO account_session_cost_lines_new
    (session_id, position, bucket, category, kind, item_name, units, amount_micro_ped,
     precision, markup_hundredths, reason, set_id)
SELECT session_id, position, bucket, category, kind, item_name, units, amount_micro_ped,
       precision, markup_hundredths, reason, NULL
FROM account_session_cost_lines;

DROP TABLE account_session_cost_lines;
ALTER TABLE account_session_cost_lines_new RENAME TO account_session_cost_lines;

ALTER TABLE session_cost_resolutions
    ADD COLUMN overhead_travel_ped REAL CHECK (overhead_travel_ped >= 0);
ALTER TABLE session_cost_resolutions
    ADD COLUMN overhead_field_logistics_ped REAL CHECK (overhead_field_logistics_ped >= 0);
ALTER TABLE session_cost_resolutions
    ADD COLUMN overhead_unclassified_ped REAL CHECK (overhead_unclassified_ped >= 0);

ALTER TABLE session_summaries ADD COLUMN overhead_cost REAL NOT NULL DEFAULT 0;

ALTER TABLE daily_rollups ADD COLUMN overhead_travel REAL;
ALTER TABLE daily_rollups ADD COLUMN overhead_field_logistics REAL;
ALTER TABLE daily_rollups ADD COLUMN overhead_unclassified REAL;

PRAGMA foreign_keys = ON;
