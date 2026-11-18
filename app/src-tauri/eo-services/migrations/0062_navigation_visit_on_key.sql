-- A route can record each tree as visited on the game's interact key rather
-- than detecting the cut from harvests. The choice is made when the route is
-- planned and belongs to that run. The per-run update hotkey column stays
-- for existing rows; new runs leave it at its default.
ALTER TABLE navigation_runs ADD COLUMN visit_on_key INTEGER NOT NULL DEFAULT 0;
