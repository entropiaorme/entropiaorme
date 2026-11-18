-- What travel and the like cost between tracked sessions, as the readings
-- taken meanwhile measure it. Additive and forward-only.
--
-- account_between_sessions_steps holds each step between two consecutive
-- readings taken outside any session (the later may be a session's first,
-- taken as it started) that wore or used something up: the readings it
-- lies between (reading_id is the later; a reading a deleted session took
-- may since be gone), when each was taken (unix seconds), its measure, and
-- the overhead it bills by category in PED (travel, field logistics, and
-- what the app cannot place yet), priced by the rules (costs_version) and
-- the roles (roles_fingerprint) named, and priced again when either
-- changes. A step a session's measure, or its costs read across a gap,
-- came to cover is that session's, and is not billed here.
--
-- Each day's rollup adds the overhead of the steps that ended on it to its
-- overhead by category, credited to no session, and keeps that part apart
-- (overhead_between_sessions, NULL when no step billed on the day), since
-- no rate may hold it.

CREATE TABLE account_between_sessions_steps (
    reading_id          INTEGER PRIMARY KEY,
    prev_reading_id     INTEGER NOT NULL,
    seen_from           REAL NOT NULL,
    seen_to             REAL NOT NULL,
    measure_version     INTEGER NOT NULL CHECK (measure_version >= 1),
    measure_json        TEXT NOT NULL CHECK (
        json_valid(measure_json) AND json_type(measure_json) = 'object'
    ),
    travel_ped          REAL NOT NULL CHECK (travel_ped >= 0),
    field_logistics_ped REAL NOT NULL CHECK (field_logistics_ped >= 0),
    unclassified_ped    REAL NOT NULL CHECK (unclassified_ped >= 0),
    costs_version       INTEGER NOT NULL,
    roles_fingerprint   TEXT NOT NULL,
    computed_at         REAL NOT NULL,
    CHECK (reading_id <> prev_reading_id),
    CHECK (seen_to >= seen_from)
);

CREATE INDEX idx_account_between_sessions_steps_seen_to
    ON account_between_sessions_steps(seen_to);

ALTER TABLE daily_rollups ADD COLUMN overhead_between_sessions REAL;
