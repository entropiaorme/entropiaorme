-- Protection costs are recorded per stream: one pooled stream for unlimited
-- repairs and one stream per limited set. A recording is spread over the
-- sessions the user ticks when recording it, by observed hits, and nothing
-- is declared during play. Each recording therefore remembers how far along
-- the defence-event stream it reached, so the next recording of the same
-- stream offers only the sessions that played after it.
--
-- Loadouts, protection intervals, and the per-segment session flag fall out
-- of use without being dropped: their rows stay readable history.

ALTER TABLE protection_cost_windows ADD COLUMN evidence_cursor INTEGER;

-- A limited window reached as far as its closing observation did.
UPDATE protection_cost_windows
SET evidence_cursor = (
    SELECT o.defence_event_cursor
    FROM protection_observations o
    WHERE o.id = protection_cost_windows.closing_observation_id
)
WHERE kind = 'limited_decay';

-- A repair window reached at least as far as the hits it settled, and past
-- every hit of a session that had already ended when it was recorded.
UPDATE protection_cost_windows
SET evidence_cursor = max(
    COALESCE((
        SELECT MAX(e.defence_event_id)
        FROM protection_cost_evidence e
        WHERE e.window_id = protection_cost_windows.id
    ), 0),
    COALESCE((
        SELECT MAX(d.id)
        FROM protection_defence_events d
        JOIN tracking_sessions s ON s.id = d.session_id
        WHERE s.ended_at IS NOT NULL
          AND s.ended_at <= protection_cost_windows.created_at
    ), 0)
)
WHERE kind = 'repair';

UPDATE protection_cost_windows
SET evidence_cursor = 0
WHERE evidence_cursor IS NULL;

-- Unlimited protection is one pooled repair stream with nothing to set up, so
-- the unlimited sets created for the declared-loadout model are retired. They
-- are archived rather than deleted: the repairs and intervals that name them
-- stay readable, and an archived set releases its name for a new limited one.
UPDATE protection_sets
SET archived_at = unixepoch('now')
WHERE economy_kind = 'unlimited' AND archived_at IS NULL;

-- Loadouts composed sets for live declaration, which no longer exists.
UPDATE protection_loadouts
SET archived_at = unixepoch('now')
WHERE archived_at IS NULL;

CREATE INDEX idx_protection_defence_session_event
    ON protection_defence_events(session_id, id);
CREATE INDEX idx_protection_cost_allocations_session
    ON protection_cost_allocations(session_id, window_id);
