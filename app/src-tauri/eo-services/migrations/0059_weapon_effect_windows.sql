-- A weapon with a declared damage-over-time effect opens an effect window
-- with each paid hit: for the effect's duration, the damage lines its ticks
-- print are outcomes of that one activation, never new shots. The window is
-- written as the activation lands, so it survives a restart and keeps
-- explaining ticks in the next session until its absolute expiry. It keeps
-- the activation's provenance (the weapon, the hit that paid for it, the
-- price that hit was booked at, the context it landed in) and the effect
-- profile as declared then, so a later Equipment edit never reinterprets it.
-- The activation's cost itself lives with its shot, in the kill it settles
-- into; the window adds none. A window opened by a hit damage evidence named
-- is withdrawn when the player keeps the hotbar's weapon over that evidence:
-- it stays as provenance, and explains no later tick.
--
-- A stored shot (an effect tick, or an unresolved hit an open effect could
-- equally have ticked) keeps the open windows that explained it, so review
-- can show a tick's candidates when several effects overlapped, and can
-- offer an unresolved hit to the effect it may have come from.
--
-- A post-play correction either prices a stored shot from a carried weapon
-- (as before; now a tick can be priced as a paid shot too) or marks an
-- unresolved hit as a tick of one of its candidate effects. Earlier rows are
-- all of the first kind.

CREATE TABLE weapon_effect_windows (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES tracking_sessions(id),
    equipment_id INTEGER,
    tool_name TEXT NOT NULL,
    context_id INTEGER,
    started_at REAL NOT NULL,
    expires_at REAL NOT NULL,
    hit_amount REAL,
    critical INTEGER NOT NULL DEFAULT 0,
    cost_per_shot REAL NOT NULL DEFAULT 0 CHECK (cost_per_shot >= 0),
    tick_min REAL NOT NULL,
    tick_max REAL NOT NULL,
    profile_json TEXT NOT NULL,
    withdrawn_at REAL,
    withdrawn_by_review_id TEXT REFERENCES weapon_attribution_reviews(id),
    CHECK (expires_at >= started_at),
    CHECK (tick_min >= 0 AND tick_max >= tick_min)
);

CREATE INDEX idx_weapon_effect_windows_expiry
    ON weapon_effect_windows(expires_at);
CREATE INDEX idx_weapon_effect_windows_session
    ON weapon_effect_windows(session_id, started_at, id);

ALTER TABLE weapon_shot_evidence
    ADD COLUMN effect_candidates_json TEXT NOT NULL DEFAULT '[]';

ALTER TABLE weapon_attribution_corrections
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'priced'
    CHECK (kind IN ('priced', 'effect_tick'));
ALTER TABLE weapon_attribution_corrections
    ADD COLUMN effect_window_id TEXT;
