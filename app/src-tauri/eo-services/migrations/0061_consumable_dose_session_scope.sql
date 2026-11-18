-- A consumable dose's effect lives inside the tracking session it was taken
-- in. Stopping the session ends every dose still running in it (ended_at),
-- because play after the stop is not observed: the game's clock keeps
-- running, so no later session can know how much of a dose is left.
--
-- A dose may now also be untimed (expires_at NULL): a dose taken before the
-- session started, declared by the player as still in force. It books no
-- cost (the dose was bought and taken before), has no known expiry, and runs
-- until the player ends it (ended_at) or the session stops.
--
-- expires_at loses its NOT NULL, so the table is rebuilt with every row
-- carried across unchanged. The old table is renamed away first, so the
-- re-dose chain's self-reference names the new table throughout; the check
-- is deferred to the commit, when every dose a re-dose names is present.

PRAGMA defer_foreign_keys = ON;

ALTER TABLE consumable_doses RENAME TO consumable_doses_before_session_scope;

CREATE TABLE consumable_doses (
    id TEXT PRIMARY KEY,
    equipment_id INTEGER,
    item_name TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('hotbar', 'manual', 'on_use')),
    session_id TEXT REFERENCES tracking_sessions(id),
    context_id INTEGER,
    interval_id INTEGER,
    started_at REAL NOT NULL,
    expires_at REAL,
    cost_ped REAL NOT NULL DEFAULT 0 CHECK (cost_ped >= 0),
    cost_tracked INTEGER NOT NULL DEFAULT 0 CHECK (cost_tracked IN (0, 1)),
    effects_json TEXT NOT NULL DEFAULT '[]',
    healing_activation_id TEXT,
    supersedes_dose_id TEXT REFERENCES consumable_doses(id),
    superseded_at REAL,
    removed_at REAL,
    removed_by TEXT CHECK (removed_by IS NULL OR removed_by IN ('player', 'heal_correction')),
    ended_at REAL,
    CHECK ((removed_at IS NULL) = (removed_by IS NULL)),
    CHECK (expires_at IS NULL OR expires_at >= started_at),
    CHECK (ended_at IS NULL OR ended_at >= started_at),
    CHECK (expires_at IS NOT NULL OR cost_ped = 0),
    CHECK (session_id IS NOT NULL OR cost_ped = 0)
);

INSERT INTO consumable_doses (
    id, equipment_id, item_name, source, session_id, context_id, interval_id,
    started_at, expires_at, cost_ped, cost_tracked, effects_json,
    healing_activation_id, supersedes_dose_id, superseded_at, removed_at,
    removed_by
)
SELECT
    id, equipment_id, item_name, source, session_id, context_id, interval_id,
    started_at, expires_at, cost_ped, cost_tracked, effects_json,
    healing_activation_id, supersedes_dose_id, superseded_at, removed_at,
    removed_by
FROM consumable_doses_before_session_scope;

DROP TABLE consumable_doses_before_session_scope;

CREATE INDEX idx_consumable_doses_expiry ON consumable_doses(expires_at);
CREATE INDEX idx_consumable_doses_session
    ON consumable_doses(session_id, started_at, id);
CREATE INDEX idx_consumable_doses_activation
    ON consumable_doses(healing_activation_id);
