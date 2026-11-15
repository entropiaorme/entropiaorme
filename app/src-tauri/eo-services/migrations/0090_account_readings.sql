-- Readings of the player's own inventory and PED Card from the account
-- site, and the measure each tracked session derives from them. The
-- measure stands beside the tracked figures and never feeds them.
-- Additive and forward-only.
--
-- A reading stores the absolute state of each holding (an item name in one
-- container): its total quantity, its total truncated value, and its rows.
-- A keyframe stores every holding; any other reading stores only the
-- holdings whose state differs from the reading before it, with an empty
-- state (no rows) for a holding that is gone. Absolute states rather than
-- differences, so no error accumulates along the chain and any two
-- readings compare directly. The chain follows the order readings were
-- stored (prev_reading_id), never their times. The purpose and the
-- container kind are closed sets the app reads as enums; an unknown stored
-- value is refused when read, not here.

CREATE TABLE account_readings (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    taken_at        REAL NOT NULL,
    purpose         TEXT NOT NULL,
    session_id      TEXT REFERENCES tracking_sessions(id),
    balance_pec     INTEGER NOT NULL CHECK (balance_pec >= 0),
    is_keyframe     INTEGER NOT NULL CHECK (is_keyframe IN (0, 1)),
    prev_reading_id INTEGER REFERENCES account_readings(id),
    row_count       INTEGER NOT NULL CHECK (row_count >= 0),
    CHECK (is_keyframe = 1 OR prev_reading_id IS NOT NULL)
);

CREATE INDEX idx_account_readings_taken ON account_readings(taken_at, id);
CREATE INDEX idx_account_readings_session
    ON account_readings(session_id, taken_at) WHERE session_id IS NOT NULL;

CREATE TABLE account_reading_holdings (
    reading_id     INTEGER NOT NULL REFERENCES account_readings(id),
    item_name      TEXT NOT NULL CHECK (length(trim(item_name)) > 0),
    container_kind TEXT NOT NULL,
    container_name TEXT NOT NULL,
    quantity       INTEGER NOT NULL CHECK (quantity >= 0),
    value_pec      INTEGER NOT NULL CHECK (value_pec >= 0),
    row_count      INTEGER NOT NULL CHECK (row_count >= 0),
    rows_json      TEXT NOT NULL CHECK (
        json_valid(rows_json)
        AND json_type(rows_json) = 'array'
        AND json_array_length(rows_json) = row_count
    ),
    PRIMARY KEY (reading_id, item_name, container_kind, container_name),
    CHECK (CASE WHEN container_kind IN ('carried', 'auction') THEN container_name = ''
                ELSE length(trim(container_name)) > 0 END),
    CHECK (row_count > 0 OR (quantity = 0 AND value_pec = 0))
);

CREATE TABLE account_session_measures (
    session_id       TEXT PRIMARY KEY REFERENCES tracking_sessions(id),
    first_reading_id INTEGER REFERENCES account_readings(id),
    last_reading_id  INTEGER REFERENCES account_readings(id),
    sealed_at        REAL,
    coverage         TEXT NOT NULL CHECK (coverage IN ('complete', 'partial')),
    reading_count    INTEGER NOT NULL CHECK (reading_count >= 0),
    gap_count        INTEGER NOT NULL CHECK (gap_count >= 0),
    measure_version  INTEGER NOT NULL CHECK (measure_version >= 1),
    measure_json     TEXT NOT NULL CHECK (
        json_valid(measure_json) AND json_type(measure_json) = 'object'
    ),
    computed_at      REAL NOT NULL,
    CHECK ((first_reading_id IS NULL) = (last_reading_id IS NULL)),
    CHECK ((first_reading_id IS NULL) = (reading_count = 0)),
    CHECK (coverage = 'partial' OR (gap_count = 0 AND reading_count >= 2))
);
