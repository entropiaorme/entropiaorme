-- A limited set's pieces, named once, and what billing a session's armour
-- from them keeps. Additive and forward-only.
--
-- A limited set's pieces are the exact item names its armour or plates
-- carry in the account readings, confirmed by the player once. A piece
-- that loses TT in a session the readings measure is billed at the markup
-- of the set it belonged to then. Each confirmation is kept with when it
-- ended (the piece removed, or its set taken out of use), so a piece can
-- move to a new set while the sessions billed under the old one keep its
-- markup. A piece belongs to at most one set at a time: one open
-- confirmation per item.
--
-- A sealed session's armour, as its measure bills it, in parts: the
-- unlimited armour and plates at the TT they lost, each limited set with
-- confirmed pieces at the TT its pieces lost and the set's markup, and the
-- TT lost by limited pieces no confirmed set holds, which has no price.
-- Written beside the session's measured costs, only for a session the
-- readings measure whole, and only while those costs are current. One row
-- per part, the unconfirmed part only when such a piece fell.
--
-- What a session's armour resolves to when its unlimited part is measured
-- and its limited part is not: the measured parts, and the recorded costs
-- of the limited sets the readings did not price, kept apart so a reader
-- can sum them in plain SQL.
--
-- A Trade Terminal reading of a limited set measures against the set's
-- pieces in the latest account reading when that is newer than the set's
-- own last reading. That baseline is kept as a reading of the set, dated by
-- the account reading, so the recording it measures stands on it like any
-- other; this table says which account reading it came from. The source
-- column of such a reading, which admits only the player's two ways of
-- entering a figure, holds 'manual'; this table is what says where the
-- figure came from.

CREATE TABLE protection_set_pieces (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    set_id       INTEGER NOT NULL REFERENCES protection_sets(id),
    item_name    TEXT NOT NULL CHECK (length(trim(item_name)) > 0 AND item_name = trim(item_name)),
    confirmed_at REAL NOT NULL,
    removed_at   REAL CHECK (removed_at IS NULL OR removed_at >= confirmed_at)
);

CREATE UNIQUE INDEX idx_protection_set_pieces_open
    ON protection_set_pieces(item_name) WHERE removed_at IS NULL;

CREATE INDEX idx_protection_set_pieces_set ON protection_set_pieces(set_id, item_name);

CREATE TABLE account_session_cost_armour_parts (
    session_id        TEXT NOT NULL REFERENCES account_session_costs(session_id),
    part              TEXT NOT NULL CHECK (part IN ('unlimited', 'limitedSet', 'unconfirmed')),
    set_id            INTEGER REFERENCES protection_sets(id),
    lost_micro_ped    INTEGER NOT NULL CHECK (lost_micro_ped >= 0),
    lost_precision    TEXT NOT NULL CHECK (lost_precision IN ('exact', 'wholePec')),
    markup_hundredths INTEGER CHECK (markup_hundredths IS NULL OR markup_hundredths >= 10000),
    amount_micro_ped  INTEGER CHECK (amount_micro_ped IS NULL OR amount_micro_ped >= 0),
    precision         TEXT CHECK (precision IN ('exact', 'wholePec')),
    CHECK ((part = 'limitedSet') = (set_id IS NOT NULL)),
    CHECK ((part = 'limitedSet') = (markup_hundredths IS NOT NULL)),
    CHECK ((part = 'unconfirmed') = (amount_micro_ped IS NULL)),
    CHECK ((amount_micro_ped IS NULL) = (precision IS NULL))
);

CREATE UNIQUE INDEX idx_account_session_cost_armour_parts_part
    ON account_session_cost_armour_parts(session_id, part, COALESCE(set_id, 0));

CREATE TABLE session_cost_resolution_armour (
    session_id   TEXT PRIMARY KEY REFERENCES session_cost_resolutions(session_id),
    measured_ped REAL NOT NULL CHECK (measured_ped >= 0),
    tracked_ped  REAL NOT NULL CHECK (tracked_ped >= 0)
);

CREATE TABLE protection_observation_readings (
    observation_id     INTEGER PRIMARY KEY REFERENCES protection_observations(id),
    account_reading_id INTEGER NOT NULL REFERENCES account_readings(id)
);
