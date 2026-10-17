-- Curated quests: the catalogue the app ships, abandoned runs, the planet a
-- session's quest offering follows, and declared reward provenance.
--
-- Additive and forward-only. No existing row changes meaning: a NULL
-- catalogue key is a user-authored quest, a NULL abandon instant is a
-- cancellation, and a NULL quest planet keeps a session on its authored
-- roster.

-- The shipped catalogue adopts quest and family rows by a stable key, so a
-- row's history stays attached across renames the catalogue makes.
ALTER TABLE quests ADD COLUMN catalogue_key TEXT;
CREATE UNIQUE INDEX idx_quests_catalogue_key
    ON quests(catalogue_key) WHERE catalogue_key IS NOT NULL;

ALTER TABLE quest_families ADD COLUMN catalogue_key TEXT;
CREATE UNIQUE INDEX idx_quest_families_catalogue_key
    ON quest_families(catalogue_key) WHERE catalogue_key IS NOT NULL;

-- An abandoned run: the mission was given up in game. It stays a cancelled
-- run (no completion, no reward), but the instant is kept because giving up
-- today's mission holds the slot until the next one can be collected.
ALTER TABLE quest_runs ADD COLUMN abandoned_at REAL;

-- The planet whose quests a session offers. NULL keeps the authored roster.
ALTER TABLE session_definitions ADD COLUMN quest_planet TEXT;

-- A session whose quest roster all lives on one curated planet follows that
-- planet; a roster of the player's own quests stays hand-picked.
UPDATE session_definitions
SET quest_planet = (
    SELECT MIN(planet) FROM (
        SELECT q.planet AS planet
        FROM session_definition_roster r
        JOIN quests q ON q.id = r.ref_id
        WHERE r.definition_id = session_definitions.id AND r.kind = 'quest'
        UNION ALL
        SELECT f.planet AS planet
        FROM session_definition_roster r
        JOIN quest_families f ON f.id = r.ref_id
        WHERE r.definition_id = session_definitions.id AND r.kind = 'quest_family'
    )
)
WHERE (
    SELECT COUNT(DISTINCT planet) FROM (
        SELECT q.planet AS planet
        FROM session_definition_roster r
        JOIN quests q ON q.id = r.ref_id
        WHERE r.definition_id = session_definitions.id AND r.kind = 'quest'
        UNION ALL
        SELECT f.planet AS planet
        FROM session_definition_roster r
        JOIN quest_families f ON f.id = r.ref_id
        WHERE r.definition_id = session_definitions.id AND r.kind = 'quest_family'
    )
) = 1
AND (
    SELECT MIN(planet) FROM (
        SELECT q.planet AS planet
        FROM session_definition_roster r
        JOIN quests q ON q.id = r.ref_id
        WHERE r.definition_id = session_definitions.id AND r.kind = 'quest'
        UNION ALL
        SELECT f.planet AS planet
        FROM session_definition_roster r
        JOIN quest_families f ON f.id = r.ref_id
        WHERE r.definition_id = session_definitions.id AND r.kind = 'quest_family'
    )
) IN ('ARIS', 'Cyrene');

-- A reward the player declares as paid, with no recorded line to show for it,
-- is kept distinguishable from one observed in the chat log.
ALTER TABLE quest_reward_reviews
    ADD COLUMN basis TEXT NOT NULL DEFAULT 'observed'
        CHECK (basis IN ('observed', 'declared'));
ALTER TABLE session_quest_completion_reward_items
    ADD COLUMN evidence_basis TEXT NOT NULL DEFAULT 'observed'
        CHECK (evidence_basis IN ('observed', 'declared'));
