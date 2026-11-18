-- What the account readings propose: a strongbox opening, a listing sold,
-- or a listing returned, seen in the step between two readings. A proposal
-- is a guess until the player records it through the strongbox or listing
-- commands; it is never a figure, and nothing sums it. Additive and
-- forward-only.
--
-- A proposal names the reading it was seen in and the reading stored
-- before it, with both their times: the outcome happened between them. One
-- proposal per kind, reading and subject (the boxes' names for an opening,
-- the listing's id for a listing), so examining a reading again never adds
-- a second. The kind and the status are closed sets the app reads as
-- enums; an unknown stored value is refused when read, not here. A
-- recorded proposal names the record it became; whether that record still
-- stands is read from the record itself, never stored here.
--
-- Recording an opening from proposals claims them first (claimed_at), then
-- records the opening, then links them to it. A claim with no link after it
-- (the app closed between the two, or the link failed) says the proposals
-- may already be recorded; releasing it clears it, and linking lifts it.
--
-- A reading is examined once, after it is stored; account_readings_examined
-- holds the readings already looked at, so a reading stored as the app
-- closed is examined at the next start. The readings stored before this
-- version are marked examined here: what they hold is history, already
-- recorded or not as the player chose, and is never offered.

CREATE TABLE account_proposals (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    kind            TEXT NOT NULL,
    reading_id      INTEGER NOT NULL REFERENCES account_readings(id),
    prev_reading_id INTEGER NOT NULL REFERENCES account_readings(id),
    seen_from       REAL NOT NULL,
    seen_to         REAL NOT NULL,
    session_id      TEXT REFERENCES tracking_sessions(id),
    subject         TEXT NOT NULL CHECK (length(trim(subject)) > 0),
    payload_version INTEGER NOT NULL CHECK (payload_version >= 1),
    payload_json    TEXT NOT NULL CHECK (
        json_valid(payload_json) AND json_type(payload_json) = 'object'
    ),
    status          TEXT NOT NULL,
    detected_at     REAL NOT NULL,
    decided_at      REAL,
    record_id       TEXT CHECK (record_id IS NULL OR length(trim(record_id)) > 0),
    claimed_at      REAL,
    CHECK (reading_id <> prev_reading_id),
    CHECK (record_id IS NULL OR decided_at IS NOT NULL),
    UNIQUE (kind, reading_id, subject)
);

CREATE INDEX idx_account_proposals_status ON account_proposals(status, kind);

CREATE TABLE account_readings_examined (
    reading_id  INTEGER PRIMARY KEY REFERENCES account_readings(id),
    examined_at REAL NOT NULL
);

INSERT INTO account_readings_examined (reading_id, examined_at)
SELECT id, unixepoch('now') FROM account_readings;
