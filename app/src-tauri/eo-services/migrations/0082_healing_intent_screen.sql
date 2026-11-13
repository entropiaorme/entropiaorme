-- A paid heal's intent can be the tool in hand read off the game screen.
--
-- healing_activations.intent_source names the evidence behind a paid use;
-- 'screen' joins 'key', 'overlay' and 'inferred': the healer was in hand as
-- the screen showed it.
--
-- SQLite needs a table rebuild to widen a CHECK; every existing row is
-- copied unchanged with every column in its place, and the table's
-- indexes are recreated as they were. Nothing else about the table moves.
--
-- Additive and forward-only.
PRAGMA foreign_keys = OFF;

CREATE TABLE healing_activations_new (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES tracking_sessions(id) ON DELETE CASCADE,
    equipment_id INTEGER REFERENCES equipment_library(id) ON DELETE SET NULL,
    tool_name TEXT NOT NULL,
    intent_at REAL NOT NULL,
    observed_at REAL NOT NULL,
    chat_timestamp TEXT NOT NULL,
    context_id INTEGER REFERENCES session_contexts(id) ON DELETE SET NULL,
    cost_ped REAL NOT NULL CHECK (cost_ped >= 0),
    profile_json TEXT NOT NULL,
    provenance TEXT NOT NULL CHECK (
        provenance IN ('direct', 'health_capped', 'retrospective')
    ),
    superseded_at REAL,
    confirming_output_id TEXT REFERENCES healing_outputs(id) ON DELETE SET NULL,
    correction_id TEXT REFERENCES healing_corrections(id) ON DELETE SET NULL,
    reload_speed_percent REAL,
    intent_source TEXT CHECK (
        intent_source IS NULL OR intent_source IN ('key', 'overlay', 'inferred', 'screen')
    )
);

INSERT INTO healing_activations_new
    (id, session_id, equipment_id, tool_name, intent_at, observed_at, chat_timestamp,
     context_id, cost_ped, profile_json, provenance, superseded_at, confirming_output_id,
     correction_id, reload_speed_percent, intent_source)
SELECT id, session_id, equipment_id, tool_name, intent_at, observed_at, chat_timestamp,
       context_id, cost_ped, profile_json, provenance, superseded_at, confirming_output_id,
       correction_id, reload_speed_percent, intent_source
FROM healing_activations;

DROP TABLE healing_activations;
ALTER TABLE healing_activations_new RENAME TO healing_activations;

CREATE INDEX idx_healing_activations_session
    ON healing_activations(session_id, observed_at, id);
CREATE INDEX idx_healing_activations_observed
    ON healing_activations(observed_at);
CREATE INDEX idx_healing_activations_confirming_output
    ON healing_activations(confirming_output_id);

PRAGMA foreign_keys = ON;
