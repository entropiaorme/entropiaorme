-- Reconciling held stock against what the account shows. Additive and
-- forward-only.
--
-- stock_reconciliation_draft holds the one reconciliation being reviewed:
-- what was observed (an account reading or a pasted inventory), each
-- item's quantity in the app beside the quantity observed, and what the
-- player chose to do about each difference. It is a working document,
-- replaced by the next plan and gone once applied, so it is kept whole as
-- one JSON document; every revision is validated before it is written.
--
-- stock_reconciliations holds each reconciliation applied, one row each,
-- under the draft's id (so applying the same draft again finds it and
-- changes nothing), with what it was observed from and when. Undoing it
-- takes back every record it made and marks it undone; the row stays.
--
-- stock_reconciliation_lines holds, per item it settled or left, the
-- app's quantity, the quantity observed, and the disposition chosen.
--
-- stock_reconciliation_records names each stock record a reconciliation
-- made through the existing writes (a removal, a conversion, a sale at TT,
-- or stock of unknown origin), by the record's own id, so undoing the
-- reconciliation takes back exactly those.
--
-- stock_reconciliation_acknowledged names each record the account readings
-- made, and the player took back, that a reconciliation settled a line of
-- its item over. Until one standing reconciliation has, the item is left as
-- it is by default in every reconciliation, whatever came between.

CREATE TABLE stock_reconciliation_draft (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    draft_id    TEXT NOT NULL CHECK (length(trim(draft_id)) > 0),
    draft_json  TEXT NOT NULL,
    updated_at  REAL NOT NULL
);

CREATE TABLE stock_reconciliations (
    id           TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
    source       TEXT NOT NULL CHECK (source IN ('reading', 'paste')),
    reading_id   INTEGER,
    observed_at  REAL NOT NULL,
    applied_at   REAL NOT NULL,
    undone_at    REAL,
    CHECK ((source = 'reading') = (reading_id IS NOT NULL))
);

CREATE TABLE stock_reconciliation_lines (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    reconciliation_id  TEXT NOT NULL REFERENCES stock_reconciliations(id),
    item_name          TEXT NOT NULL CHECK (length(trim(item_name)) > 0),
    app_quantity       REAL NOT NULL CHECK (app_quantity >= 0),
    observed_quantity  REAL NOT NULL CHECK (observed_quantity >= 0),
    disposition        TEXT NOT NULL CHECK (disposition IN (
                           'removed', 'shrapnel_converted', 'conversion_group',
                           'unknown_origin', 'left_as_is')),
    UNIQUE (reconciliation_id, item_name)
);

CREATE TABLE stock_reconciliation_records (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    reconciliation_id  TEXT NOT NULL REFERENCES stock_reconciliations(id),
    record_kind        TEXT NOT NULL CHECK (record_kind IN (
                           'removal', 'conversion', 'trade', 'unknown_origin')),
    record_id          TEXT NOT NULL CHECK (length(trim(record_id)) > 0),
    UNIQUE (record_kind, record_id)
);

CREATE INDEX idx_stock_reconciliation_records_reconciliation
    ON stock_reconciliation_records(reconciliation_id);

CREATE TABLE stock_reconciliation_acknowledged (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    reconciliation_id  TEXT NOT NULL REFERENCES stock_reconciliations(id),
    record_id          TEXT NOT NULL CHECK (length(trim(record_id)) > 0),
    UNIQUE (reconciliation_id, record_id)
);
