-- Session stats exclusion: keep a recorded session as evidence while taking
-- its figures out of the app-wide statistics, one accounting scope at a time.
--
-- A session whose recording went wrong (a misbehaving input, a mis-parsed
-- log) is still worth keeping to understand what happened, but its figures
-- should stop feeding the aggregates. The two scopes are independent:
--
-- * economy: loot, harvest, every cost bucket, the activity evidence its
--   kills and time provide, and the session-derived ledger entries;
-- * progression: the PES of its skill gains.
--
-- Each column holds the epoch second the scope was excluded, NULL while the
-- session counts (the loot `deactivated_at` idiom at session grain). The
-- note records why, for whoever reads the session later.
--
-- Additive and forward-only: every existing session stays included.
ALTER TABLE tracking_sessions ADD COLUMN economy_excluded_at REAL;
ALTER TABLE tracking_sessions ADD COLUMN progression_excluded_at REAL;
ALTER TABLE tracking_sessions ADD COLUMN exclusion_note TEXT;

-- The session an automatically derived ledger entry was computed from (the
-- session-end enhancer shrapnel rebate), so the entry follows that
-- session's economy scope. NULL for manual entries and for rebates written
-- before this migration, which carried no link.
ALTER TABLE ledger_entries ADD COLUMN source_session_id TEXT;
CREATE INDEX idx_ledger_entries_source_session
    ON ledger_entries(source_session_id)
    WHERE source_session_id IS NOT NULL;
