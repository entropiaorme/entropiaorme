-- The session a manually recorded ledger entry is pegged to: a cost or a gain
-- that belongs to a session definition as a whole and is not worth splitting
-- any finer by hand.
--
-- The entry stays an ordinary row of the global ledger. The link only adds it
-- to that session's realised figures, shared across what the session recorded
-- in proportion to what each part cycled. The shares are worked out when read,
-- so a recording re-filed under another definition, or excluded from the stats,
-- moves them without touching this row.
--
-- Distinct from `source_session_id`, which names the single recorded run a
-- derived entry was computed from and leaves the ledger with that run. A pegged
-- entry is money the player confirmed spending or receiving, so it stays.
--
-- NULL for every existing entry and for any entry tied to no session.
-- Additive and forward-only.
ALTER TABLE ledger_entries ADD COLUMN session_definition_id INTEGER;
CREATE INDEX idx_ledger_entries_session_definition
    ON ledger_entries(session_definition_id)
    WHERE session_definition_id IS NOT NULL;
