-- Enhancer markups set per kind and tier, not per weapon. Additive and
-- forward-only.
--
-- enhancer_tier_markups holds the markup the player paid for enhancers of
-- one kind and tier (`weaponDamage` today; other kinds join as rows of
-- the same table), one row per value set. A break is priced at the value
-- in force when it broke, so a later value applies from when it was set and
-- never moves a break already kept. A row with no markup prices that tier
-- at TT from then on. The weapon setting that held a markup per weapon is
-- no longer read.

CREATE TABLE enhancer_tier_markups (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    kind            TEXT NOT NULL CHECK (kind IN ('weaponDamage')),
    tier            INTEGER NOT NULL CHECK (tier BETWEEN 1 AND 10),
    markup_percent  REAL CHECK (markup_percent IS NULL
                                OR (markup_percent >= 100 AND markup_percent <= 100000)),
    set_at          REAL NOT NULL
);

CREATE INDEX idx_enhancer_tier_markups ON enhancer_tier_markups(kind, tier, set_at);
