-- Healing evidence can be corrected after play, and a healing effect outlives
-- a restart.
--
-- A correction never deletes. Marking a paid activation as not a paid use
-- supersedes it and its effect window; marking an output as a paid use mints
-- a new activation that names the correction. Every output a correction
-- moves keeps what it was before in its prior_* columns, so undoing the
-- correction restores it exactly, and the correction row itself stays as
-- provenance with the time it was undone. An activation a correction minted
-- keeps the checked provenance 'direct' (the player confirmed the output as
-- a direct paid use) and is recognised by its correction_id.
--
-- Effect windows are read back by their absolute expiry, and each healer's
-- latest paid use by observation time, when a session starts; the expiry and
-- observation indexes serve those reads. The confirming-output index serves
-- the "already a paid use" check, and the activation index the backfill below.

CREATE TABLE healing_corrections (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES tracking_sessions(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('not_paid_use', 'paid_use')),
    activation_id TEXT NOT NULL REFERENCES healing_activations(id) ON DELETE CASCADE,
    output_id TEXT REFERENCES healing_outputs(id) ON DELETE SET NULL,
    cost_delta_ped REAL NOT NULL,
    corrected_at REAL NOT NULL,
    undone_at REAL
);

CREATE INDEX idx_healing_corrections_session
    ON healing_corrections(session_id, corrected_at, id);

ALTER TABLE healing_activations ADD COLUMN superseded_at REAL;
ALTER TABLE healing_activations ADD COLUMN confirming_output_id TEXT
    REFERENCES healing_outputs(id) ON DELETE SET NULL;
ALTER TABLE healing_activations ADD COLUMN correction_id TEXT
    REFERENCES healing_corrections(id) ON DELETE SET NULL;

ALTER TABLE healing_effect_windows ADD COLUMN superseded_at REAL;

ALTER TABLE healing_outputs ADD COLUMN correction_id TEXT
    REFERENCES healing_corrections(id) ON DELETE SET NULL;
ALTER TABLE healing_outputs ADD COLUMN prior_classification TEXT CHECK (
    prior_classification IS NULL
    OR prior_classification IN ('direct', 'effect', 'passive', 'unattributed')
);
ALTER TABLE healing_outputs ADD COLUMN prior_activation_id TEXT;
ALTER TABLE healing_outputs ADD COLUMN prior_effect_window_id TEXT;
ALTER TABLE healing_outputs ADD COLUMN prior_reason TEXT;

CREATE INDEX idx_healing_outputs_activation
    ON healing_outputs(activation_id);

-- Link every existing activation to the output that confirmed it: the output
-- written with it, or the earlier output a delayed hotbar occurrence
-- reconciled.
UPDATE healing_activations
SET confirming_output_id = (
    SELECT o.id FROM healing_outputs o
    WHERE o.activation_id = healing_activations.id
      AND o.reason IN (
          'confirmed a paid healing activation',
          'reconciled with an earlier hotbar occurrence'
      )
    ORDER BY o.observed_at, o.id
    LIMIT 1
);

CREATE INDEX idx_healing_effect_windows_expiry
    ON healing_effect_windows(expires_at);
CREATE INDEX idx_healing_activations_observed
    ON healing_activations(observed_at);
CREATE INDEX idx_healing_activations_confirming_output
    ON healing_activations(confirming_output_id);
CREATE INDEX idx_healing_outputs_correction
    ON healing_outputs(correction_id);
