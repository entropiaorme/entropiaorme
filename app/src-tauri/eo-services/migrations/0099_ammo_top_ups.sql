-- The PED Card no longer decides whether a session's use of an item is a
-- cost, and a session whose Universal Ammo was topped up keeps the rest of
-- its weapon bucket measured. Additive and forward-only.
--
-- A consumable leaving beside a Card movement nothing explains is now used
-- up like any other, so the reason 'departureUnexplained' leaves the
-- closed sets of the stored measured costs and of the resolution. The
-- stored costs and their resolution are derived: the app rebuilds them
-- from the readings and the tracked figures whenever their rules change,
-- as they do with this version. A row carrying the reason (a bucket's, in
-- practice) is therefore not copied: its session's costs read as stale
-- and are rebuilt by the new rules, as every other session's are.
--
-- The weapon bucket of the stored measured costs may be 'mixed': measured
-- (its amount) but for the ammo fired in the steps Universal Ammo was
-- topped up in, with no Shrapnel conversion or strongbox opening behind
-- it. account_session_cost_top_ups names those steps, by the readings'
-- times either side in whole milliseconds, one row each. The resolution
-- keeps such a weapon bucket as tracked, with the reason 'ammoToppedUp',
-- and session_cost_resolution_weapon carries its parts beside it, as
-- session_cost_resolution_armour does for armour: the measured amount, the
-- tracked spend of the kills logged in those steps, and the scale that
-- carries the sum onto the session's tracked shots (NULL when none carry
-- it).
--
-- SQLite needs a table rebuild to change a CHECK; every other row is
-- copied unchanged with every column in its place. Nothing else about the
-- tables moves.
PRAGMA foreign_keys = OFF;

CREATE TABLE account_session_costs_new (
    session_id          TEXT PRIMARY KEY REFERENCES tracking_sessions(id),
    costs_version       INTEGER NOT NULL CHECK (costs_version >= 1),
    measure_version     INTEGER NOT NULL CHECK (measure_version >= 1),
    measure_computed_at REAL NOT NULL,
    status              TEXT NOT NULL CHECK (status IN ('measured', 'acrossGap', 'unmeasured')),
    reason              TEXT CHECK (reason IN (
        'partialCoverage', 'gap', 'ammoUnsettled', 'limitedSetUnconfirmed', 'trackedOnly',
        'playInGap', 'chatLogMissesGap', 'chatLogUnchecked', 'gapUnaccounted'
    )),
    first_reading_id    INTEGER REFERENCES account_readings(id),
    last_reading_id     INTEGER REFERENCES account_readings(id),
    computed_at         REAL NOT NULL,
    roles_fingerprint   TEXT CHECK (roles_fingerprint IS NULL OR length(roles_fingerprint) > 0),
    CHECK ((status = 'unmeasured') = (reason IS NOT NULL)),
    CHECK ((first_reading_id IS NULL) = (last_reading_id IS NULL)),
    CHECK (status != 'acrossGap' OR first_reading_id IS NOT NULL)
);

INSERT INTO account_session_costs_new
    (session_id, costs_version, measure_version, measure_computed_at, status, reason,
     first_reading_id, last_reading_id, computed_at, roles_fingerprint)
SELECT session_id, costs_version, measure_version, measure_computed_at, status, reason,
       first_reading_id, last_reading_id, computed_at, roles_fingerprint
FROM account_session_costs
WHERE reason IS NOT 'departureUnexplained';

DROP TABLE account_session_costs;
ALTER TABLE account_session_costs_new RENAME TO account_session_costs;

CREATE TABLE account_session_cost_buckets_new (
    session_id       TEXT NOT NULL REFERENCES account_session_costs(session_id),
    bucket           TEXT NOT NULL CHECK (bucket IN (
        'weapon', 'enhancer', 'harvesting', 'healing', 'armour', 'consumables', 'travel',
        'fieldLogistics', 'unclassified'
    )),
    basis            TEXT NOT NULL CHECK (basis IN ('measured', 'mixed', 'unmeasured')),
    reason           TEXT CHECK (reason IN (
        'partialCoverage', 'gap', 'ammoUnsettled', 'limitedSetUnconfirmed', 'trackedOnly',
        'playInGap', 'chatLogMissesGap', 'chatLogUnchecked', 'gapUnaccounted', 'overheadInGap'
    )),
    amount_micro_ped INTEGER,
    precision        TEXT CHECK (precision IN ('exact', 'wholePec')),
    PRIMARY KEY (session_id, bucket),
    CHECK ((basis = 'unmeasured') = (reason IS NOT NULL)),
    CHECK ((basis = 'unmeasured') = (amount_micro_ped IS NULL)),
    CHECK ((amount_micro_ped IS NULL) = (precision IS NULL)),
    CHECK (reason != 'overheadInGap' OR bucket IN ('travel', 'fieldLogistics', 'unclassified')),
    CHECK (basis != 'mixed' OR bucket = 'weapon')
);

INSERT INTO account_session_cost_buckets_new
    (session_id, bucket, basis, reason, amount_micro_ped, precision)
SELECT session_id, bucket, basis, reason, amount_micro_ped, precision
FROM account_session_cost_buckets
WHERE reason IS NOT 'departureUnexplained';

DROP TABLE account_session_cost_buckets;
ALTER TABLE account_session_cost_buckets_new RENAME TO account_session_cost_buckets;

CREATE TABLE account_session_cost_top_ups (
    session_id TEXT NOT NULL REFERENCES account_session_costs(session_id),
    from_ms    INTEGER NOT NULL,
    to_ms      INTEGER NOT NULL CHECK (to_ms >= from_ms),
    PRIMARY KEY (session_id, from_ms)
);

CREATE TABLE session_cost_resolution_buckets_new (
    session_id   TEXT NOT NULL REFERENCES session_cost_resolutions(session_id),
    bucket       TEXT NOT NULL CHECK (bucket IN (
        'weapon', 'enhancer', 'harvesting', 'healing', 'armour', 'consumables'
    )),
    basis        TEXT NOT NULL CHECK (basis IN ('measured', 'tracked')),
    reason       TEXT CHECK (reason IN (
        'partialCoverage', 'gap', 'ammoUnsettled', 'ammoToppedUp', 'limitedSetUnconfirmed',
        'trackedOnly', 'playInGap', 'chatLogMissesGap', 'chatLogUnchecked', 'gapUnaccounted'
    )),
    measured_ped REAL,
    scale        REAL CHECK (scale IS NULL OR scale >= 0),
    PRIMARY KEY (session_id, bucket),
    CHECK ((basis = 'measured') = (reason IS NULL)),
    CHECK ((basis = 'measured') = (measured_ped IS NOT NULL)),
    CHECK (basis = 'measured' OR scale IS NULL),
    CHECK (reason != 'ammoToppedUp' OR bucket = 'weapon')
);

INSERT INTO session_cost_resolution_buckets_new
    (session_id, bucket, basis, reason, measured_ped, scale)
SELECT session_id, bucket, basis, reason, measured_ped, scale
FROM session_cost_resolution_buckets
WHERE reason IS NOT 'departureUnexplained';

DROP TABLE session_cost_resolution_buckets;
ALTER TABLE session_cost_resolution_buckets_new RENAME TO session_cost_resolution_buckets;

CREATE TABLE session_cost_resolution_weapon (
    session_id   TEXT PRIMARY KEY REFERENCES session_cost_resolutions(session_id),
    measured_ped REAL NOT NULL CHECK (measured_ped >= 0),
    tracked_ped  REAL NOT NULL CHECK (tracked_ped >= 0),
    scale        REAL CHECK (scale IS NULL OR scale >= 0)
);

PRAGMA foreign_keys = ON;
