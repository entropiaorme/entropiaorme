-- How far the player has seen what the account did on its own: the records
-- the readings made with nobody looking, and what they found that waits for
-- a decision. Something recorded or found after `seen_through` (unix
-- seconds) is new to the player, and the app marks it until they look.
-- Additive and forward-only.
--
-- One row. It is seeded at the moment this version arrives, so what the
-- readings did before it is history the player has already lived with,
-- never a flood of marks. The watermark only moves forward.

CREATE TABLE account_activity_seen (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    seen_through REAL NOT NULL,
    updated_at   REAL NOT NULL
);

INSERT INTO account_activity_seen (id, seen_through, updated_at)
VALUES (1, unixepoch('now'), unixepoch('now'));
