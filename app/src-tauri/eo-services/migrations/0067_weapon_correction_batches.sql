-- Weapon correction batches: one assignment of every unpriced shot of a
-- session that a chosen weapon fits, kept together so it can be undone as
-- a whole.
--
-- Each shot the batch prices still gets its own correction row, undoable
-- on its own; batch_id ties those rows together. NULL for a correction made
-- one shot at a time, and for every correction written before batches
-- existed.
--
-- Additive and forward-only.
ALTER TABLE weapon_attribution_corrections ADD COLUMN batch_id TEXT;
CREATE INDEX idx_weapon_attribution_corrections_batch
    ON weapon_attribution_corrections(batch_id)
    WHERE batch_id IS NOT NULL;
