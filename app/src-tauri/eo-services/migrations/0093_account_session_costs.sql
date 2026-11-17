-- What a sealed session's measure says it cost, by bucket and by item: a
-- projection derived from account_session_measures, written beside the
-- seal and rebuilt on read when its rules, or the measure it was derived
-- from, are not the current ones. Plain columns throughout, so a reader
-- needs no JSON. Additive and forward-only.
--
-- One header per session, naming the measure it was derived from (its
-- rules' version and when it was computed), the readings bounding the
-- measured window, and a digest of the equipment library, protection sets
-- and game data its items were placed by: a change to any of them leaves
-- it to be rebuilt. One row per bucket: measured with an amount, or
-- unmeasured with the reason, never both. One row per item line, in a
-- stable order. Amounts are integer micro-PED with their precision:
-- exact, or resting on the site's values truncated to the whole PEC.
-- Units are given for the lines that count units (ammo burned, a
-- consumable used up) and only for those. A consumable used up is charged
-- at TT, or at the acquisition markup the player recorded for it (kept on
-- the line), or not at all here, with the reason, when the player
-- accounts for it through the ledger. Only a sealed session is
-- stored, so neither "still running" nor "no readings" is ever a stored
-- reason. The closed sets are checked here as well as read as enums.

CREATE TABLE account_session_costs (
    session_id          TEXT PRIMARY KEY REFERENCES tracking_sessions(id),
    costs_version       INTEGER NOT NULL CHECK (costs_version >= 1),
    measure_version     INTEGER NOT NULL CHECK (measure_version >= 1),
    measure_computed_at REAL NOT NULL,
    status              TEXT NOT NULL CHECK (status IN ('measured', 'unmeasured')),
    reason              TEXT CHECK (reason IN (
        'partialCoverage', 'gap', 'ammoUnsettled', 'departureUnexplained',
        'limitedSetUnconfirmed', 'trackedOnly'
    )),
    first_reading_id    INTEGER REFERENCES account_readings(id),
    last_reading_id     INTEGER REFERENCES account_readings(id),
    computed_at         REAL NOT NULL,
    roles_fingerprint   TEXT CHECK (roles_fingerprint IS NULL OR length(roles_fingerprint) > 0),
    CHECK ((status = 'measured') = (reason IS NULL)),
    CHECK ((first_reading_id IS NULL) = (last_reading_id IS NULL))
);

CREATE TABLE account_session_cost_buckets (
    session_id       TEXT NOT NULL REFERENCES account_session_costs(session_id),
    bucket           TEXT NOT NULL CHECK (bucket IN (
        'weapon', 'enhancer', 'harvesting', 'healing', 'armour', 'consumables', 'travel',
        'fieldLogistics', 'unclassified'
    )),
    basis            TEXT NOT NULL CHECK (basis IN ('measured', 'unmeasured')),
    reason           TEXT CHECK (reason IN (
        'partialCoverage', 'gap', 'ammoUnsettled', 'departureUnexplained',
        'limitedSetUnconfirmed', 'trackedOnly'
    )),
    amount_micro_ped INTEGER,
    precision        TEXT CHECK (precision IN ('exact', 'wholePec')),
    PRIMARY KEY (session_id, bucket),
    CHECK ((basis = 'measured') = (reason IS NULL)),
    CHECK ((basis = 'measured') = (amount_micro_ped IS NOT NULL)),
    CHECK ((amount_micro_ped IS NULL) = (precision IS NULL))
);

CREATE TABLE account_session_cost_lines (
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
        'mindEssence', 'fieldLogistics', 'unclassified'
    )),
    kind             TEXT NOT NULL CHECK (kind IN (
        'ammoBurned', 'decay', 'limitedDecay', 'consumed'
    )),
    item_name        TEXT NOT NULL CHECK (length(trim(item_name)) > 0),
    units            INTEGER CHECK (units IS NULL OR units > 0),
    amount_micro_ped INTEGER,
    precision        TEXT CHECK (precision IN ('exact', 'wholePec')),
    markup_hundredths INTEGER CHECK (markup_hundredths IS NULL OR markup_hundredths > 0),
    reason           TEXT CHECK (reason IN ('boughtThroughLedger')),
    PRIMARY KEY (session_id, position),
    CHECK ((kind IN ('ammoBurned', 'consumed')) = (units IS NOT NULL)),
    CHECK ((amount_micro_ped IS NULL) = (precision IS NULL)),
    CHECK ((amount_micro_ped IS NULL) = (reason IS NOT NULL)),
    CHECK (kind = 'consumed' OR (reason IS NULL AND markup_hundredths IS NULL)),
    CHECK (markup_hundredths IS NULL OR amount_micro_ped IS NOT NULL)
);
