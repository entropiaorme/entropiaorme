-- Whether adding a sighted tool to the library created its entry.
--
-- A sighting settled as added names the library entry it became: an entry
-- the addition created, or one of the same item the library already held,
-- which the addition only linked to. `entry_created` is 1 for an entry the
-- addition created and 0 otherwise (an entry linked to, and every sighting
-- not added). Only a sighting settled as added may hold 1.
--
-- Every existing sighting starts at 0.
--
-- Additive and forward-only.
ALTER TABLE tool_sightings ADD COLUMN entry_created INTEGER NOT NULL DEFAULT 0
    CHECK (entry_created IN (0, 1) AND (entry_created = 0 OR settled_kind IS 'added'));
