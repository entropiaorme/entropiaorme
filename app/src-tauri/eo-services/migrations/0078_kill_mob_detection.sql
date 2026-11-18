-- Which creature each kill was, as the game screen showed it.
--
-- While a session runs, the application can read the name over the
-- creature the player is locked on to and, once the chat log reports a
-- kill, decide which creature the kill was. The decision comes with a
-- grade:
--   named              a listed creature, read whole and repeatedly in its
--                      own fight;
--   guess              a listed creature the screen leans to without
--                      proving it;
--   provisional        a creature on no list, under the spelling read most
--                      often, read at a level;
--   provisional_guess  the same, leaned to but not proved;
--   latest_read        the creature read last before the kill, where the
--                      screen shows no lean: wrong too often to stamp;
--   provisional_latest_read  the same for a creature on no list.
--
-- The upper four grades stamp the kill: its mob columns then hold the
-- detected creature, `mob_stamp_source` is 'detected', and the two columns
-- below say at which grade and from what text. Every other kill leaves
-- them empty, which is all a kill recorded before this migration says.
ALTER TABLE kills ADD COLUMN mob_detection_grade TEXT
    CHECK (
        mob_detection_grade IS NULL
        OR (mob_detection_grade IN ('named', 'guess', 'provisional', 'provisional_guess')
            AND mob_stamp_source = 'detected')
    );
ALTER TABLE kills ADD COLUMN mob_read_text TEXT
    CHECK (
        (mob_read_text IS NULL) = (mob_detection_grade IS NULL)
        AND (mob_read_text IS NULL OR trim(mob_read_text) <> '')
    );

-- Every answer detection gave a kill, at any grade, kept whatever happens
-- to the kill's stamp later: the creature it named (for a creature on no
-- list, the spelling, with the session's key telling apart names read at
-- different levels), the text most often read for it, when its plate was
-- last read, and the stamp the player's declaration gave the kill, which
-- a review can put back. A lowest-grade answer is stored here only, as a
-- suggestion; the kill keeps its declared stamp. `reviewed_at` is when the
-- player last settled the kill's creature after the session, and the
-- `settled_` columns what they settled it as: a listed creature, a name
-- confirmed before, a new name kept from then on, or the declared stamp
-- put back (with the stamp the kill was given), so a kill detected and then
-- confirmed by the player reads apart from one declared from the start.
CREATE TABLE kill_mob_detections (
    kill_id           TEXT PRIMARY KEY REFERENCES kills(id),
    session_id        TEXT NOT NULL REFERENCES tracking_sessions(id),
    grade             TEXT NOT NULL CHECK (grade IN (
                          'named', 'guess', 'provisional', 'provisional_guess',
                          'latest_read', 'provisional_latest_read')),
    mob_name          TEXT NOT NULL CHECK (trim(mob_name) <> ''),
    mob_species       TEXT NOT NULL,
    mob_maturity      TEXT NOT NULL,
    provisional_key   TEXT,
    read_text         TEXT NOT NULL CHECK (trim(read_text) <> ''),
    read_at           REAL NOT NULL,
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
    CHECK (
        (provisional_key IS NOT NULL)
        = (grade IN ('provisional', 'provisional_guess', 'provisional_latest_read'))
    )
);
CREATE INDEX idx_kill_mob_detections_session ON kill_mob_detections(session_id);

-- Names the player has confirmed for creatures the bundled list lacks. The
-- name reader treats them as known from then on. A name that is a listed
-- creature, or close to one, is never kept: it is that creature, misread.
-- A name no kill carries any more is retired.
-- Species and maturity are kept when the player gives them.
CREATE TABLE confirmed_mob_names (
    name         TEXT PRIMARY KEY COLLATE NOCASE
                 CHECK (name <> '' AND trim(name) = name AND length(name) <= 64),
    species      TEXT,
    maturity     TEXT,
    confirmed_at REAL NOT NULL
);

-- Additive and forward-only.
