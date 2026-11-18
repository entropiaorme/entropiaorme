-- How each kill was named, and the kills named by the creature being
-- hunted.
--
-- A session has one source for its kills' creature, steered by the
-- player. While the game screen is read, a kill is named by what the
-- screen shows for it; failing that by the creature the latest kills were
-- (continuity); failing both by the creature the player typed as a
-- backstop. The player can instead pin a creature (the one the screen is
-- reading, or one typed), and every kill takes it until the pin is
-- released. Without screen reading, the typed creature is pinned.
--
-- `mob_route` records how the application named the kill when it was
-- recorded or re-stamped while the session ran:
--   named, guess, provisional, provisional_guess
--                          the screen's own evidence for the kill, at that
--                          grade (as `mob_detection_grade` holds it);
--   continuity, provisional_continuity
--                          the creature being hunted, a listed one or one
--                          on no list, given where the screen could not
--                          settle the kill;
--   pinned_read            the creature pinned from what the screen read;
--   pinned_typed           the creature pinned by typing it;
--   backstop               the typed backstop, where nothing else named it.
-- NULL is a kill named by nothing (its stamp is "Unknown"), or one recorded
-- before routes were kept, whose source and grade columns still say all
-- that is known of it. A review after the session settles the stamp and
-- leaves the route as the application gave it: the settlement is kept on
-- the answer it settled.
ALTER TABLE kills ADD COLUMN mob_route TEXT
    CHECK (mob_route IS NULL OR mob_route IN (
        'named', 'guess', 'provisional', 'provisional_guess',
        'continuity', 'provisional_continuity',
        'pinned_read', 'pinned_typed', 'backstop'));

-- Every kill given a creature that nothing read for the kill itself gave,
-- kept whatever happens to the kill's stamp later, for the review:
--   continuity, provisional_continuity
--                          the creature being hunted (one on a list, or one
--                          on no list under the session's name for it);
--   provisional_pinned     a creature on no list the player pinned from what
--                          the screen read: the player chose it, but its
--                          spelling is the screen's and still unconfirmed.
-- It rests on earlier kills or on the pin, not on anything read for this
-- kill, so it has no text or time read of its own. The creature (for one on
-- no list, the spelling, with the session's key telling apart names read at
-- different levels), and the stamp the kill would have had without it (the
-- typed creature, or none), which a review can put back. `reviewed_at` and
-- the `settled_` columns are as for `kill_mob_detections`.
CREATE TABLE kill_mob_carried (
    kill_id           TEXT PRIMARY KEY REFERENCES kills(id),
    session_id        TEXT NOT NULL REFERENCES tracking_sessions(id),
    grade             TEXT NOT NULL CHECK (grade IN (
                          'continuity', 'provisional_continuity', 'provisional_pinned')),
    mob_name          TEXT NOT NULL CHECK (trim(mob_name) <> ''),
    mob_species       TEXT NOT NULL,
    mob_maturity      TEXT NOT NULL,
    provisional_key   TEXT CHECK (provisional_key IS NULL OR provisional_key <> ''),
    declared_name     TEXT NOT NULL,
    declared_species  TEXT NOT NULL,
    declared_maturity TEXT NOT NULL,
    declared_source   TEXT CHECK (declared_source = 'declared' OR declared_source IS NULL),
    reviewed_at       REAL,
    settled_kind      TEXT CHECK (settled_kind IN ('listed', 'confirmed', 'new_name', 'declared')
                                  OR settled_kind IS NULL),
    settled_name      TEXT,
    settled_species   TEXT,
    settled_maturity  TEXT,
    CHECK ((settled_kind IS NULL) = (reviewed_at IS NULL)),
    CHECK ((settled_kind IS NULL) = (settled_name IS NULL)
           AND (settled_kind IS NULL) = (settled_species IS NULL)
           AND (settled_kind IS NULL) = (settled_maturity IS NULL)),
    CHECK ((provisional_key IS NOT NULL) = (grade <> 'continuity'))
);
CREATE INDEX idx_kill_mob_carried_session ON kill_mob_carried(session_id);

-- Additive and forward-only.
