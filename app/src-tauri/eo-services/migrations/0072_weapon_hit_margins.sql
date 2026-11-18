-- Hits outside a held weapon's listed damage range, kept shot by shot.
--
-- A temporary damage buff lifts a regular hit above its weapon's listed
-- maximum, and mob armour pushes one below its minimum. Both are priced to
-- the weapon in hand, and both are worth seeing afterwards: a run of them
-- that the player cannot account for says the wrong weapon was declared.
-- They join the kept shots under two new classifications, so review can
-- count them per weapon and move any that were really fired by another.
--
-- SQLite needs a table rebuild to widen a CHECK; every existing row is
-- copied unchanged, and nothing else about the table moves.
--
-- A correction can now replace a price rather than only supply a missing
-- one, so it records the per-shot cost it replaced: undoing it restores
-- that cost exactly. NULL for a correction of a shot that had no price.
--
-- Additive and forward-only.
PRAGMA foreign_keys = OFF;

CREATE TABLE weapon_shot_evidence_new (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES tracking_sessions(id),
    kill_id TEXT REFERENCES kills(id),
    context_id INTEGER,
    observed_at REAL NOT NULL,
    amount REAL,
    critical INTEGER NOT NULL DEFAULT 0,
    attribution TEXT NOT NULL CHECK (
        attribution IN ('evidence', 'unresolved', 'effect_tick', 'boosted', 'reduced')
    ),
    hotbar_tool TEXT,
    tool_name TEXT,
    cost_per_shot REAL NOT NULL DEFAULT 0,
    candidates_json TEXT NOT NULL DEFAULT '[]',
    reason TEXT NOT NULL,
    effect_window_id TEXT,
    review_id TEXT REFERENCES weapon_attribution_reviews(id),
    correction_id TEXT REFERENCES weapon_attribution_corrections(id),
    effect_candidates_json TEXT NOT NULL DEFAULT '[]',
    reload_speed_percent REAL
);

INSERT INTO weapon_shot_evidence_new
    (id, session_id, kill_id, context_id, observed_at, amount, critical, attribution,
     hotbar_tool, tool_name, cost_per_shot, candidates_json, reason, effect_window_id,
     review_id, correction_id, effect_candidates_json, reload_speed_percent)
SELECT id, session_id, kill_id, context_id, observed_at, amount, critical, attribution,
       hotbar_tool, tool_name, cost_per_shot, candidates_json, reason, effect_window_id,
       review_id, correction_id, effect_candidates_json, reload_speed_percent
FROM weapon_shot_evidence;

DROP TABLE weapon_shot_evidence;
ALTER TABLE weapon_shot_evidence_new RENAME TO weapon_shot_evidence;

CREATE INDEX idx_weapon_shot_evidence_session
    ON weapon_shot_evidence(session_id, attribution, observed_at, id);
CREATE INDEX idx_weapon_shot_evidence_kill
    ON weapon_shot_evidence(kill_id);

PRAGMA foreign_keys = ON;

ALTER TABLE weapon_attribution_corrections ADD COLUMN replaced_cost_per_shot REAL;
