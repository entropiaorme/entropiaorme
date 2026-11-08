-- What a session's kills add up to, so a session definition's lifetime figures
-- can offer everything the live session view does without touching the kills
-- table on a read. Each column either sums across sessions or folds by maximum
-- or recency, and each is a pure function of the kills table, so the summary
-- stays rebuildable.
--
-- The multiplier is kept as a sum and a count rather than an average, which is
-- what lets the lifetime figure be the mean over every kill and not the mean of
-- each session's mean. A kill with no recorded weapon cost has no multiplier.
--
-- Existing rows fill here in one pass. A summary version bump would instead
-- recompute every row on the writer the first time any summary is read.
-- Additive and forward-only.
ALTER TABLE session_summaries ADD COLUMN kill_damage_total REAL NOT NULL DEFAULT 0;
ALTER TABLE session_summaries ADD COLUMN kill_damage_max REAL NOT NULL DEFAULT 0;
ALTER TABLE session_summaries ADD COLUMN kill_shots INTEGER NOT NULL DEFAULT 0;
ALTER TABLE session_summaries ADD COLUMN kill_crits INTEGER NOT NULL DEFAULT 0;
ALTER TABLE session_summaries ADD COLUMN kill_globals INTEGER NOT NULL DEFAULT 0;
ALTER TABLE session_summaries ADD COLUMN kill_hofs INTEGER NOT NULL DEFAULT 0;
ALTER TABLE session_summaries ADD COLUMN multiplier_sum REAL NOT NULL DEFAULT 0;
ALTER TABLE session_summaries ADD COLUMN multiplier_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE session_summaries ADD COLUMN multiplier_max REAL;
ALTER TABLE session_summaries ADD COLUMN last_kill_at REAL;
ALTER TABLE session_summaries ADD COLUMN last_kill_loot REAL;
ALTER TABLE session_summaries ADD COLUMN last_kill_multiplier REAL;

UPDATE session_summaries
SET kill_damage_total = totals.damage_total,
    kill_damage_max = totals.damage_max,
    kill_shots = totals.shots,
    kill_crits = totals.crits,
    kill_globals = totals.globals,
    kill_hofs = totals.hofs,
    multiplier_sum = totals.multiplier_sum,
    multiplier_count = totals.multiplier_count,
    multiplier_max = totals.multiplier_max
FROM (
    SELECT session_id,
           COALESCE(SUM(damage_dealt), 0) AS damage_total,
           COALESCE(MAX(damage_dealt), 0) AS damage_max,
           COALESCE(SUM(shots_fired), 0) AS shots,
           COALESCE(SUM(critical_hits), 0) AS crits,
           COALESCE(SUM(is_global), 0) AS globals,
           COALESCE(SUM(is_hof), 0) AS hofs,
           COALESCE(SUM(CASE WHEN cost_ped > 0 THEN loot_total_ped / cost_ped END), 0)
               AS multiplier_sum,
           COUNT(CASE WHEN cost_ped > 0 THEN 1 END) AS multiplier_count,
           MAX(CASE WHEN cost_ped > 0 THEN loot_total_ped / cost_ped END) AS multiplier_max
    FROM kills
    GROUP BY session_id
) AS totals
WHERE totals.session_id = session_summaries.session_id;

UPDATE session_summaries
SET last_kill_at = latest.timestamp,
    last_kill_loot = latest.loot_total_ped,
    last_kill_multiplier = CASE WHEN latest.cost_ped > 0
                                THEN latest.loot_total_ped / latest.cost_ped END
FROM (
    SELECT session_id, timestamp, loot_total_ped, cost_ped,
           ROW_NUMBER() OVER (PARTITION BY session_id
                              ORDER BY timestamp DESC, rowid DESC) AS position
    FROM kills
) AS latest
WHERE latest.position = 1
  AND latest.session_id = session_summaries.session_id;
