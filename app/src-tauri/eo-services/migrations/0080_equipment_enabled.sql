-- Whether the player has each equipment library entry in use.
--
-- One weapon kept in two set-ups is two entries of one name, and two
-- long names the game client cuts short can show alike on screen: such
-- entries cannot be told apart by their names. The player says which of
-- them is in use by leaving it on and turning the others off.
--
-- Every existing entry, and every entry added from now on, starts on.
ALTER TABLE equipment_library ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1
    CHECK (enabled IN (0, 1));

-- Additive and forward-only.
