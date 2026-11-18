-- Record the opening balance that an over-sized outflow should have revealed.
--
-- An outflow larger than the stock the app knows about records the excess as
-- an opening balance (the player held units from before tracking began). That
-- excess was measured against the sum of an item's positive attribution
-- buckets alone, so an outflow left stranded under a bucket its loot had since
-- moved out of (a session reassigned to another definition or context) still
-- counted as held stock. Outflows sized from that inflated figure could exceed
-- the item's true position without the excess being recorded, leaving the
-- item's arithmetic below zero.
--
-- A position below zero is not a thing an inventory can hold, and leaving it
-- in place would silently swallow the next units looted until the hole filled.
-- The read side now nets every bucket against the whole item, so the excess
-- is recorded here once, exactly as the outflow would have recorded it: an
-- unattributed opening balance belonging to the item's most recent outflow,
-- so undoing that outflow takes the balance with it.
--
-- The base mirrors the position arithmetic: active harvested and hunted loot,
-- and confirmed, unreversed quest-reward stock, plus every movement. Unit TT is
-- the loot's own, falling back to what conversions produced.

INSERT INTO stock_movements
    (item_name, movement_kind, ref_id, source_kind, quantity, tt_value,
     occurred_at, created_at)
SELECT
    p.item_name,
    'opening_balance',
    o.ref_id,
    'unattributed',
    -p.net_qty,
    -p.net_qty * CASE
        WHEN p.base_qty > 1e-9 THEN p.base_tt / p.base_qty
        WHEN p.produced_qty > 1e-9 THEN p.produced_tt / p.produced_qty
        ELSE 0.0
    END,
    o.occurred_at,
    o.created_at
FROM (
    SELECT
        b.item_name,
        b.base_qty,
        b.base_tt,
        b.base_qty + COALESCE((SELECT SUM(m.quantity) FROM stock_movements AS m
                               WHERE m.item_name = b.item_name), 0) AS net_qty,
        COALESCE((SELECT SUM(m.quantity) FROM stock_movements AS m
                  WHERE m.item_name = b.item_name AND m.quantity > 0), 0) AS produced_qty,
        COALESCE((SELECT SUM(m.tt_value) FROM stock_movements AS m
                  WHERE m.item_name = b.item_name AND m.quantity > 0), 0) AS produced_tt
    FROM (
        SELECT
            i.item_name,
            COALESCE((SELECT SUM(l.quantity)
                      FROM harvest_loot_items AS l
                      JOIN harvest_events AS e ON e.id = l.harvest_id
                      WHERE l.item_name = i.item_name AND l.deactivated_at IS NULL), 0)
            + COALESCE((SELECT SUM(li.quantity)
                        FROM kill_loot_items AS li
                        JOIN kills AS k ON k.id = li.kill_id
                        JOIN tracking_sessions AS s ON s.id = k.session_id
                        WHERE li.item_name = i.item_name AND li.deactivated_at IS NULL), 0)
            + COALESCE((SELECT SUM(ri.quantity)
                        FROM session_quest_completion_reward_items AS ri
                        JOIN session_quest_completions AS c ON c.id = ri.completion_id
                        WHERE ri.item_name = i.item_name AND ri.accounting_kind = 'stock'
                          AND c.quest_run_id IS NOT NULL
                          AND NOT EXISTS (SELECT 1 FROM quest_reward_reversals AS rr
                                          WHERE rr.completion_id = c.id)
                          AND COALESCE((SELECT r.outcome FROM quest_reward_reviews AS r
                                        WHERE r.completion_id = c.id
                                        ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1),
                                       c.reward_outcome) = 'confirmed'), 0) AS base_qty,
            COALESCE((SELECT SUM(l.value_ped)
                      FROM harvest_loot_items AS l
                      JOIN harvest_events AS e ON e.id = l.harvest_id
                      WHERE l.item_name = i.item_name AND l.deactivated_at IS NULL), 0)
            + COALESCE((SELECT SUM(li.value_ped)
                        FROM kill_loot_items AS li
                        JOIN kills AS k ON k.id = li.kill_id
                        JOIN tracking_sessions AS s ON s.id = k.session_id
                        WHERE li.item_name = i.item_name AND li.deactivated_at IS NULL), 0)
            + COALESCE((SELECT SUM(ri.value_ped)
                        FROM session_quest_completion_reward_items AS ri
                        JOIN session_quest_completions AS c ON c.id = ri.completion_id
                        WHERE ri.item_name = i.item_name AND ri.accounting_kind = 'stock'
                          AND c.quest_run_id IS NOT NULL
                          AND NOT EXISTS (SELECT 1 FROM quest_reward_reversals AS rr
                                          WHERE rr.completion_id = c.id)
                          AND COALESCE((SELECT r.outcome FROM quest_reward_reviews AS r
                                        WHERE r.completion_id = c.id
                                        ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1),
                                       c.reward_outcome) = 'confirmed'), 0) AS base_tt
        FROM (SELECT DISTINCT item_name FROM stock_movements
              WHERE item_name <> 'Universal Ammo') AS i
    ) AS b
) AS p
JOIN stock_movements AS o ON o.id = (
    SELECT m.id FROM stock_movements AS m
    WHERE m.item_name = p.item_name AND m.quantity < 0 AND m.ref_id IS NOT NULL
    ORDER BY m.created_at DESC, m.id DESC
    LIMIT 1
)
WHERE p.net_qty < -1e-9;
