-- A sealed session whose readings leave an end unread (a sign-in dropped,
-- the app closed) can be measured across the gap from the nearest reading
-- on its far side, when the game's chat log shows the time the gap spends
-- outside the session clean. Additive and forward-only.
--
-- Such costs are a kind of their own: the header's status 'acrossGap',
-- with no reason, its reading columns naming the first and last readings
-- the measure across the gap used, and one row of
-- account_session_cost_gaps naming each end read across the gap: the
-- reading the gap ends at, how far outside the session it was taken, and
-- how long the readings were missing there, in whole seconds. At least
-- one end is read across the gap.
--
-- A gap that could not be billed keeps every bucket tracked, with the
-- reason: the chat log shows play outside the session in it
-- ('playInGap'), does not cover it ('chatLogMissesGap'), or could not be
-- checked over it ('chatLogUnchecked'); or the readings across it hold a
-- repair or a cost of a kind the session's own play never had
-- ('gapUnaccounted'). An overhead bucket of a session measured across a
-- gap that changed in the gap is not billed ('overheadInGap'). The
-- resolution keeps how long the gap was, so a reader can say so.
--
-- What the chat log showed over a session's gap is kept apart from the
-- projection, in account_session_gap_verdicts, one row per session, with
-- the span it was taken over: the readings either side of the gap and the
-- session's start and end in whole milliseconds. A rebuild of the
-- projection over the same span takes the kept verdict rather than reading
-- the log again; a new span takes a new one. It goes with the session.
--
-- The chat log's stamps are placed in time by the game server's offset
-- from UTC, which the live tail derives. chat_log_time_base remembers the
-- offset last settled, one row: the offset, since when it is known to have
-- held, and the live read that last confirmed it (unix seconds). An offset
-- that settles after a restart at the value remembered is taken to have
-- held since the remembered one did.
--
-- SQLite needs a table rebuild to widen a CHECK; every existing row is
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
        'partialCoverage', 'gap', 'ammoUnsettled', 'departureUnexplained',
        'limitedSetUnconfirmed', 'trackedOnly', 'playInGap', 'chatLogMissesGap',
        'chatLogUnchecked', 'gapUnaccounted'
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
FROM account_session_costs;

DROP TABLE account_session_costs;
ALTER TABLE account_session_costs_new RENAME TO account_session_costs;

CREATE TABLE account_session_cost_buckets_new (
    session_id       TEXT NOT NULL REFERENCES account_session_costs(session_id),
    bucket           TEXT NOT NULL CHECK (bucket IN (
        'weapon', 'enhancer', 'harvesting', 'healing', 'armour', 'consumables', 'travel',
        'fieldLogistics', 'unclassified'
    )),
    basis            TEXT NOT NULL CHECK (basis IN ('measured', 'unmeasured')),
    reason           TEXT CHECK (reason IN (
        'partialCoverage', 'gap', 'ammoUnsettled', 'departureUnexplained',
        'limitedSetUnconfirmed', 'trackedOnly', 'playInGap', 'chatLogMissesGap',
        'chatLogUnchecked', 'gapUnaccounted', 'overheadInGap'
    )),
    amount_micro_ped INTEGER,
    precision        TEXT CHECK (precision IN ('exact', 'wholePec')),
    PRIMARY KEY (session_id, bucket),
    CHECK ((basis = 'measured') = (reason IS NULL)),
    CHECK ((basis = 'measured') = (amount_micro_ped IS NOT NULL)),
    CHECK ((amount_micro_ped IS NULL) = (precision IS NULL)),
    CHECK (reason != 'overheadInGap' OR bucket IN ('travel', 'fieldLogistics', 'unclassified'))
);

INSERT INTO account_session_cost_buckets_new
    (session_id, bucket, basis, reason, amount_micro_ped, precision)
SELECT session_id, bucket, basis, reason, amount_micro_ped, precision
FROM account_session_cost_buckets;

DROP TABLE account_session_cost_buckets;
ALTER TABLE account_session_cost_buckets_new RENAME TO account_session_cost_buckets;

CREATE TABLE account_session_cost_gaps (
    session_id              TEXT PRIMARY KEY REFERENCES account_session_costs(session_id),
    opening_reading_id      INTEGER REFERENCES account_readings(id),
    opening_outside_seconds INTEGER CHECK (opening_outside_seconds >= 0),
    opening_gap_seconds     INTEGER CHECK (opening_gap_seconds >= 0),
    closing_reading_id      INTEGER REFERENCES account_readings(id),
    closing_outside_seconds INTEGER CHECK (closing_outside_seconds >= 0),
    closing_gap_seconds     INTEGER CHECK (closing_gap_seconds >= 0),
    CHECK ((opening_reading_id IS NULL) = (opening_outside_seconds IS NULL)),
    CHECK ((opening_reading_id IS NULL) = (opening_gap_seconds IS NULL)),
    CHECK ((closing_reading_id IS NULL) = (closing_outside_seconds IS NULL)),
    CHECK ((closing_reading_id IS NULL) = (closing_gap_seconds IS NULL)),
    CHECK (opening_reading_id IS NOT NULL OR closing_reading_id IS NOT NULL)
);

CREATE TABLE account_session_gap_verdicts (
    session_id         TEXT PRIMARY KEY REFERENCES tracking_sessions(id),
    opening_reading_id INTEGER REFERENCES account_readings(id),
    closing_reading_id INTEGER REFERENCES account_readings(id),
    started_ms         INTEGER NOT NULL,
    ended_ms           INTEGER NOT NULL CHECK (ended_ms >= started_ms),
    verdict            TEXT NOT NULL CHECK (verdict IN (
        'clean', 'playInGap', 'missesGap', 'unchecked'
    )),
    taken_at           REAL NOT NULL,
    CHECK (opening_reading_id IS NOT NULL OR closing_reading_id IS NOT NULL)
);

CREATE TABLE chat_log_time_base (
    singleton      INTEGER PRIMARY KEY CHECK (singleton = 1),
    offset_seconds INTEGER NOT NULL CHECK (offset_seconds BETWEEN -50400 AND 50400),
    held_since     INTEGER NOT NULL,
    confirmed_at   INTEGER NOT NULL CHECK (confirmed_at >= held_since)
);

CREATE TABLE session_cost_resolution_buckets_new (
    session_id   TEXT NOT NULL REFERENCES session_cost_resolutions(session_id),
    bucket       TEXT NOT NULL CHECK (bucket IN (
        'weapon', 'enhancer', 'harvesting', 'healing', 'armour', 'consumables'
    )),
    basis        TEXT NOT NULL CHECK (basis IN ('measured', 'tracked')),
    reason       TEXT CHECK (reason IN (
        'partialCoverage', 'gap', 'ammoUnsettled', 'departureUnexplained',
        'limitedSetUnconfirmed', 'trackedOnly', 'playInGap', 'chatLogMissesGap',
        'chatLogUnchecked', 'gapUnaccounted'
    )),
    measured_ped REAL,
    scale        REAL CHECK (scale IS NULL OR scale >= 0),
    PRIMARY KEY (session_id, bucket),
    CHECK ((basis = 'measured') = (reason IS NULL)),
    CHECK ((basis = 'measured') = (measured_ped IS NOT NULL)),
    CHECK (basis = 'measured' OR scale IS NULL)
);

INSERT INTO session_cost_resolution_buckets_new
    (session_id, bucket, basis, reason, measured_ped, scale)
SELECT session_id, bucket, basis, reason, measured_ped, scale
FROM session_cost_resolution_buckets;

DROP TABLE session_cost_resolution_buckets;
ALTER TABLE session_cost_resolution_buckets_new RENAME TO session_cost_resolution_buckets;

ALTER TABLE session_cost_resolutions
    ADD COLUMN across_gap_seconds INTEGER CHECK (across_gap_seconds >= 0);

PRAGMA foreign_keys = ON;
