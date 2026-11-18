-- A consumable dose: one use of a stimulant, pill, or similar item, and the
-- effect it grants until its absolute expiry. A dose starts when the player
-- presses the item's hotbar key or starts it by hand, and a healing tool that
-- grants a buff on use (Eir Mk 1) opens one with each paid heal. The row is
-- written as the dose starts, so its expiry survives a restart: the effect
-- keeps counting towards the reload speed in effect, and the next session
-- reads it back, until the expiry passes. The effects are the item's as they
-- stood then, so a later Equipment edit never reinterprets a dose taken
-- before it.
--
-- A dose of an item whose cost is tracked books its dose cost to the session
-- it was taken in, once, in the context it was taken in; one taken outside a
-- session, or of an item whose cost is not tracked, books nothing and still
-- counts its effect. Re-dosing an item whose dose is still running ends the
-- earlier one at the new start (superseded_at) and starts one new expiry;
-- the two never stack. Removing a dose (a misclicked hotbar key) never
-- deletes it: removed_at takes its effect and any cost it booked back, and
-- restoring it clears the mark and gives both back exactly, including the
-- earlier dose it had ended. A dose opened by a paid heal is removed with
-- that heal when a correction says it was not a paid use (removed_by says
-- which), and comes back when the correction is undone.

CREATE TABLE consumable_doses (
    id TEXT PRIMARY KEY,
    equipment_id INTEGER,
    item_name TEXT NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('hotbar', 'manual', 'on_use')),
    session_id TEXT REFERENCES tracking_sessions(id),
    context_id INTEGER,
    interval_id INTEGER,
    started_at REAL NOT NULL,
    expires_at REAL NOT NULL,
    cost_ped REAL NOT NULL DEFAULT 0 CHECK (cost_ped >= 0),
    cost_tracked INTEGER NOT NULL DEFAULT 0 CHECK (cost_tracked IN (0, 1)),
    effects_json TEXT NOT NULL DEFAULT '[]',
    healing_activation_id TEXT,
    supersedes_dose_id TEXT REFERENCES consumable_doses(id),
    superseded_at REAL,
    removed_at REAL,
    removed_by TEXT CHECK (removed_by IS NULL OR removed_by IN ('player', 'heal_correction')),
    CHECK ((removed_at IS NULL) = (removed_by IS NULL)),
    CHECK (expires_at >= started_at),
    CHECK (session_id IS NOT NULL OR cost_ped = 0)
);

CREATE INDEX idx_consumable_doses_expiry ON consumable_doses(expires_at);
CREATE INDEX idx_consumable_doses_session
    ON consumable_doses(session_id, started_at, id);
CREATE INDEX idx_consumable_doses_activation
    ON consumable_doses(healing_activation_id);

-- The consumed-dose cost bucket beside the other consumed inputs, on the
-- session row and both projections that sum it (SUMMARY_VERSION 6 and
-- ROLLUP_VERSION 5 fold it into cycled PED; existing rows heal on the next
-- read, and every session before this migration booked none).
ALTER TABLE tracking_sessions ADD COLUMN consumable_cost REAL DEFAULT 0;
ALTER TABLE session_summaries ADD COLUMN consumable_cost REAL DEFAULT 0;
ALTER TABLE daily_rollups ADD COLUMN consumable_cost REAL;

-- The reload speed in effect when a stored shot or a paid heal was charged.
-- Doses make it vary within a session, so review prices a stored shot at the
-- rate in force when it landed. NULL for rows charged before this migration,
-- which review prices at the rate in force now, as it did before.
ALTER TABLE weapon_shot_evidence ADD COLUMN reload_speed_percent REAL;
ALTER TABLE healing_activations ADD COLUMN reload_speed_percent REAL;
