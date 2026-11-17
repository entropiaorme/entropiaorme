-- Stock records the account readings made on their own: a Shrapnel
-- conversion, a recycling into Nanocubes, or a sale to the Trade Terminal,
-- seen in the step between two consecutive readings. Additive and
-- forward-only.
--
-- stock_records_from_account names, for each such record, where it came
-- from: the two readings the step lies between (reading_id is the later),
-- when each was taken (unix seconds), which part of the step it is (one
-- record per part, so recognising a step again records nothing twice), the
-- item it names first, and the record it became (a conversion or a trade,
-- by its own id). The row is written in the same transaction as its
-- record, and stays when the record is undone: an undone record is never
-- made again from the same step, and whether it stands is read from the
-- record itself. An item whose record from the account was undone is one
-- the player took back, so a later reconciliation of that item leaves it
-- as it is unless told otherwise.
--
-- account_steps_recognised holds the readings whose step has been looked at
-- for records, always the oldest readings first, so the next step to look
-- at is the first reading after the latest marked. The readings stored
-- before this version are marked here: what they hold is history, recorded
-- or not as the player chose, and is never recorded on its own.
--
-- account_recognition_boundary holds the latest reading stored before this
-- version. A step from a reading at or before it reaches back into that
-- history (the first reading after an update may follow one taken days
-- before), so nothing in it is recorded on its own either.

CREATE TABLE stock_records_from_account (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    record_kind     TEXT NOT NULL CHECK (record_kind IN ('conversion', 'trade')),
    record_id       TEXT NOT NULL CHECK (length(trim(record_id)) > 0),
    reading_id      INTEGER NOT NULL,
    prev_reading_id INTEGER NOT NULL,
    seen_from       REAL NOT NULL,
    seen_to         REAL NOT NULL,
    part            TEXT NOT NULL CHECK (length(trim(part)) > 0),
    item_name       TEXT NOT NULL CHECK (length(trim(item_name)) > 0),
    created_at      REAL NOT NULL,
    CHECK (reading_id <> prev_reading_id),
    CHECK (seen_to >= seen_from),
    UNIQUE (reading_id, part),
    UNIQUE (record_kind, record_id)
);

CREATE INDEX idx_stock_records_from_account_item
    ON stock_records_from_account(item_name, seen_to);

CREATE TABLE account_steps_recognised (
    reading_id    INTEGER PRIMARY KEY REFERENCES account_readings(id),
    recognised_at REAL NOT NULL
);

INSERT INTO account_steps_recognised (reading_id, recognised_at)
SELECT id, unixepoch('now') FROM account_readings;

CREATE TABLE account_recognition_boundary (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    last_reading_before INTEGER NOT NULL
);

INSERT INTO account_recognition_boundary (id, last_reading_before)
SELECT 1, COALESCE(MAX(id), 0) FROM account_readings;
