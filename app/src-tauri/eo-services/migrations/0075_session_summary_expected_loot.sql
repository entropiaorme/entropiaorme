-- What each recorded session's Cycled was expected to return, at every looter
-- basis, so a session definition's lifetime can carry the live expected figure.
-- Each basis keeps the point estimate and the two ends of its span, which all
-- sum across sessions; each session is read at its own session-start looter
-- levels.
--
-- NULL for a session that recorded no weapon evidence, which is every session
-- from before that evidence existed. The session is then left out of the
-- family's figure rather than counted as earning nothing.
--
-- Evaluating these needs the expected-return model, so the existing sessions
-- that do carry evidence fill from the summary reader rather than here.
-- Additive and forward-only.
ALTER TABLE session_summaries ADD COLUMN expected_tt_mean REAL;
ALTER TABLE session_summaries ADD COLUMN expected_tt_mean_low REAL;
ALTER TABLE session_summaries ADD COLUMN expected_tt_mean_high REAL;
ALTER TABLE session_summaries ADD COLUMN expected_tt_animal REAL;
ALTER TABLE session_summaries ADD COLUMN expected_tt_animal_low REAL;
ALTER TABLE session_summaries ADD COLUMN expected_tt_animal_high REAL;
ALTER TABLE session_summaries ADD COLUMN expected_tt_mutant REAL;
ALTER TABLE session_summaries ADD COLUMN expected_tt_mutant_low REAL;
ALTER TABLE session_summaries ADD COLUMN expected_tt_mutant_high REAL;
ALTER TABLE session_summaries ADD COLUMN expected_tt_robot REAL;
ALTER TABLE session_summaries ADD COLUMN expected_tt_robot_low REAL;
ALTER TABLE session_summaries ADD COLUMN expected_tt_robot_high REAL;
