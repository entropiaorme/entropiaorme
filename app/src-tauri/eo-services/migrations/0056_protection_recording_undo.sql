-- A protection recording can be undone. Undoing never deletes: the
-- recording, its reading, and the sessions it was spread over stay as
-- provenance, marked with when they stopped counting. Every read of the
-- stream's position, its coverage, and its costs skips superseded rows, so
-- undoing the latest recording of a stream hands its sessions back to the
-- next recording exactly as they were before.

ALTER TABLE protection_cost_windows ADD COLUMN superseded_at REAL;
ALTER TABLE protection_observations ADD COLUMN superseded_at REAL;
