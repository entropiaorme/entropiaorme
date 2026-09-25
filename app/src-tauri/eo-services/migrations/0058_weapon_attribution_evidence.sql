-- Weapon attribution is one engine: the hotbar declares the weapon in hand,
-- and each carried weapon's damage band validates the declaration.
--
-- Most shots agree with the hotbar and live only in their kill's weapon
-- phases. The shots kept one by one are those a player may want to inspect
-- or correct: damage evidence that overrode the hotbar's weapon, shots no
-- single carried weapon explains (recorded without a price), and ticks of
-- an effect window an earlier paid activation owns. A row is written in the
-- same transaction as the kill its shot settled into, or with the session's
-- stop for a shot after its last kill (kill_id NULL: the session's dangling
-- cost). `candidates_json` keeps the carried weapons at the time, each with
-- whether its band fitted the shot, so review can offer them later.
--
-- A live decision on a standing mismatch is its own row, named by every
-- stored shot it repriced. A post-play correction prices one unpriced shot
-- from a carried weapon as configured at correction time; undoing it
-- returns the shot to unpriced exactly, and the correction row stays as
-- provenance with the time it was undone.
--
-- The session row gains its final tallies of shots that agreed with the
-- hotbar and shots attributed by damage evidence. Sessions recorded earlier
-- keep them NULL: their attribution was never tallied.

CREATE TABLE weapon_attribution_reviews (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES tracking_sessions(id),
    decision TEXT NOT NULL CHECK (decision IN ('confirmed', 'kept')),
    hotbar_tool TEXT NOT NULL,
    evidence_tool TEXT NOT NULL,
    mismatch_since REAL NOT NULL,
    decided_at REAL NOT NULL,
    repriced_shots INTEGER NOT NULL,
    cost_delta_ped REAL NOT NULL
);

CREATE INDEX idx_weapon_attribution_reviews_session
    ON weapon_attribution_reviews(session_id, decided_at, id);

CREATE TABLE weapon_shot_evidence (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES tracking_sessions(id),
    kill_id TEXT REFERENCES kills(id),
    context_id INTEGER,
    observed_at REAL NOT NULL,
    amount REAL,
    critical INTEGER NOT NULL DEFAULT 0,
    attribution TEXT NOT NULL CHECK (
        attribution IN ('evidence', 'unresolved', 'effect_tick')
    ),
    hotbar_tool TEXT,
    tool_name TEXT,
    cost_per_shot REAL NOT NULL DEFAULT 0,
    candidates_json TEXT NOT NULL DEFAULT '[]',
    reason TEXT NOT NULL,
    effect_window_id TEXT,
    review_id TEXT REFERENCES weapon_attribution_reviews(id),
    correction_id TEXT REFERENCES weapon_attribution_corrections(id)
);

CREATE INDEX idx_weapon_shot_evidence_session
    ON weapon_shot_evidence(session_id, attribution, observed_at, id);
CREATE INDEX idx_weapon_shot_evidence_kill
    ON weapon_shot_evidence(kill_id);

CREATE TABLE weapon_attribution_corrections (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES tracking_sessions(id),
    evidence_id TEXT NOT NULL REFERENCES weapon_shot_evidence(id),
    equipment_id INTEGER,
    tool_name TEXT NOT NULL,
    cost_per_shot REAL NOT NULL,
    corrected_at REAL NOT NULL,
    undone_at REAL
);

CREATE INDEX idx_weapon_attribution_corrections_session
    ON weapon_attribution_corrections(session_id, corrected_at, id);

ALTER TABLE tracking_sessions ADD COLUMN weapon_shots_agreed INTEGER;
ALTER TABLE tracking_sessions ADD COLUMN weapon_shots_evidenced INTEGER;
