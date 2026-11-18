-- A quest reward handed in at its giver arrives as a loot clump with no
-- mob, which the tracker records as a kill row. Once a hand-in claims the
-- clump, that row is the reward's evidence and no longer a kill.
-- Additive and forward-only.
--
-- reward_claimed_at is when the claim was made (NULL while the row is a
-- kill); undoing the reward clears it. The row and its loot lines stay as
-- the evidence the claim points at. A reward already undone keeps its clump
-- claimed but has its lines back as loot, so it is a kill and stays unmarked.
--
-- counted_kills is every kill a hunting figure counts: each count, per-kill
-- figure, breakdown and projection reads it rather than the table, so a
-- claimed arrival leaves every one of them at once. It carries the rowid,
-- which a view otherwise hides, so a tiebreak by insertion order reads the
-- same through it.

ALTER TABLE kills ADD COLUMN reward_claimed_at REAL;

UPDATE kills
SET reward_claimed_at = (
    SELECT c.completed_at
    FROM quest_reward_clumps q
    JOIN session_quest_completions c ON c.id = q.claimed_completion_id
    WHERE q.source_kind = 'kill' AND q.source_record_id = kills.id
)
WHERE id IN (
    SELECT source_record_id
    FROM quest_reward_clumps
    WHERE source_kind = 'kill' AND claimed_completion_id IS NOT NULL
      AND NOT EXISTS (
          SELECT 1 FROM quest_reward_reversals rr
          WHERE rr.completion_id = quest_reward_clumps.claimed_completion_id
      )
);

CREATE VIEW counted_kills AS
SELECT rowid AS rowid, * FROM kills WHERE reward_claimed_at IS NULL;
