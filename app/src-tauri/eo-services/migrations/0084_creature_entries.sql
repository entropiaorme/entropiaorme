-- Where each creature the player keeps for the name reader came from.
--
-- A kept creature is a name confirmed in a session's review, or one the
-- player entered in the creature list. A kept creature whose maturity is a
-- number (`04`, `S04`) is read at whatever number its plate prints, as the
-- bundled list's creatures graded by number are; `maturity` holds the number
-- as entered, which shows how the plate prints it.
--
-- `added_by` is 'review' for a name confirmed after a session, retired when
-- no kill carries it any more, and 'player' for one entered or edited in the
-- creature list, which stays until the player removes it. Every existing
-- name was confirmed in a review.
--
-- Additive and forward-only.
ALTER TABLE confirmed_mob_names ADD COLUMN added_by TEXT NOT NULL DEFAULT 'review'
    CHECK (added_by IN ('review', 'player'));
