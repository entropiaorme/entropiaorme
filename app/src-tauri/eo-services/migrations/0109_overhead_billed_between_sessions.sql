-- The steps between sessions whose overhead is billed, as plain rows.
-- Additive and forward-only.
--
-- account_between_sessions_steps holds every step measured between
-- sessions. A step a session's measure, or its costs read across a gap,
-- came to cover is that session's and is not billed. Whether a step stands
-- is decided when it is read, so it can change as sessions are measured,
-- sealed or deleted without anything being rewritten.
--
-- overhead_billed_between_sessions is that decision, made in one place: the
-- steps that stand, with when each was seen and what it bills by category
-- in PED. The cost resolver reads it and hands it to the day rollup and the
-- Overview, which therefore need to know nothing of readings, measures or
-- how a step comes to be covered.

CREATE VIEW overhead_billed_between_sessions AS
SELECT b.reading_id,
       b.seen_from,
       b.seen_to,
       b.travel_ped,
       b.field_logistics_ped,
       b.unclassified_ped
FROM account_between_sessions_steps b
WHERE NOT EXISTS (SELECT 1 FROM account_session_measures m
                  WHERE m.first_reading_id < b.reading_id
                    AND b.reading_id <= m.last_reading_id)
  AND NOT EXISTS (SELECT 1 FROM account_session_costs c
                  WHERE c.first_reading_id < b.reading_id
                    AND b.reading_id <= c.last_reading_id);
