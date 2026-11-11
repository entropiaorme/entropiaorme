-- Whether a recorded visit's position was read off the screen.
--
-- A visit can be recorded without one: the interact key was pressed but the
-- position did not read (or did not pass the position checks), a harvest
-- was confirmed without a position, or a tree was put on cooldown from the
-- map. The observed columns are NOT NULL, so such a visit stores the pin's
-- own coordinates and a distance of 0 as a placeholder, not an observation,
-- with position_read = 0. Every reader of observed_lon, observed_lat and
-- observed_distance uses only the rows with position_read = 1.
--
-- Rows recorded before this column existed keep the default of 1: the
-- application has no way to tell them apart now.
--
-- Additive and forward-only.
ALTER TABLE map_pin_visits
    ADD COLUMN position_read INTEGER NOT NULL DEFAULT 1 CHECK (position_read IN (0, 1));
