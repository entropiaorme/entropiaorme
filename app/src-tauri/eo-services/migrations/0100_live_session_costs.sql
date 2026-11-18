-- What a tracked session still running has cost so far, as the account
-- readings measure it up to the latest of its readings. Additive and
-- forward-only.
--
-- One row per running session, replaced after each of its readings, so the
-- live figures can take the measured costs up to that reading and carry on
-- from the tracked events after it, instead of stepping when the session's
-- costs are sealed at its close. measured_through is when that reading was
-- taken (unix seconds). Each bucket's amount is in micro-PED, NULL where
-- the readings could not settle it so far (the bucket stays tracked); the
-- enhancer, which the readings never measure, has no column. A row is
-- derived: it is dropped when the session's costs are sealed, or with the
-- session, and is read only while its session runs, by the rules it was
-- written under (costs_version and measure_version).
CREATE TABLE account_live_session_costs (
    session_id                TEXT PRIMARY KEY REFERENCES tracking_sessions(id),
    costs_version             INTEGER NOT NULL CHECK (costs_version >= 1),
    measure_version           INTEGER NOT NULL CHECK (measure_version >= 1),
    reading_id                INTEGER NOT NULL REFERENCES account_readings(id),
    measured_through          REAL NOT NULL,
    weapon_micro_ped          INTEGER CHECK (weapon_micro_ped >= 0),
    harvesting_micro_ped      INTEGER CHECK (harvesting_micro_ped >= 0),
    healing_micro_ped         INTEGER CHECK (healing_micro_ped >= 0),
    armour_micro_ped          INTEGER CHECK (armour_micro_ped >= 0),
    consumables_micro_ped     INTEGER CHECK (consumables_micro_ped >= 0),
    travel_micro_ped          INTEGER CHECK (travel_micro_ped >= 0),
    field_logistics_micro_ped INTEGER CHECK (field_logistics_micro_ped >= 0),
    unclassified_micro_ped    INTEGER CHECK (unclassified_micro_ped >= 0),
    computed_at               REAL NOT NULL
);
