-- What a session's costs resolve to once its measured costs are known: a
-- projection derived from the stored measured costs and the tracked
-- figures, kept so a reader can weigh tracked events by it in plain SQL.
-- Rebuilt whenever the session's summary is written, and healed when the
-- measured costs it was derived from change. Additive and forward-only.
--
-- One header per session whose measured costs are current, naming when
-- those were computed. One row per cycled bucket: measured, with the
-- measured amount and, for a bucket tracked event by event, the scale
-- that carries it onto those events; or tracked, with the reason. A
-- measured bucket with no scale has no tracked events to carry it and
-- sits whole at the session's grain. A session with no header resolves
-- to its tracked figures throughout.

CREATE TABLE session_cost_resolutions (
    session_id             TEXT PRIMARY KEY REFERENCES tracking_sessions(id),
    resolution_version     INTEGER NOT NULL CHECK (resolution_version >= 1),
    projection_computed_at REAL NOT NULL
);

CREATE TABLE session_cost_resolution_buckets (
    session_id   TEXT NOT NULL REFERENCES session_cost_resolutions(session_id),
    bucket       TEXT NOT NULL CHECK (bucket IN (
        'weapon', 'enhancer', 'harvesting', 'healing', 'armour', 'consumables'
    )),
    basis        TEXT NOT NULL CHECK (basis IN ('measured', 'tracked')),
    reason       TEXT CHECK (reason IN (
        'partialCoverage', 'gap', 'ammoUnsettled', 'departureUnexplained',
        'limitedSetUnconfirmed', 'trackedOnly'
    )),
    measured_ped REAL,
    scale        REAL CHECK (scale IS NULL OR scale >= 0),
    PRIMARY KEY (session_id, bucket),
    CHECK ((basis = 'measured') = (reason IS NULL)),
    CHECK ((basis = 'measured') = (measured_ped IS NOT NULL)),
    CHECK (basis = 'measured' OR scale IS NULL)
);
