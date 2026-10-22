-- Codex meta calibration: the player's own correction to the metas earned.
--
-- Earned metas derive from recorded species ranks, so they miss any meta
-- the app cannot trace to a species (a species the catalogue does not yet
-- carry, or ranks earned before tracking began without being calibrated).
-- The correction is kept as an adjustment on top of the derived count, so
-- every breakpoint reached afterwards still raises the total.
--
-- Additive and forward-only: a database without a row has no adjustment.
CREATE TABLE codex_meta_calibration (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    adjustment  INTEGER NOT NULL,
    updated_at  REAL NOT NULL DEFAULT (unixepoch('now'))
);
