-- A mob is shown as the game prints it, and may be given a label.
--
-- Until now a mob's shown name was its maturity, then its name (`Young
-- Atrox`); the game prints the name first (`Atrox Young`), and the
-- application now shows it so. Stored names made the old way are rewritten
-- to the new: exactly those that are the maturity, a space, and the name,
-- so a name the player gave by hand is left alone. Each such row keeps its
-- name and maturity columns, from which either order can be made again.
--
-- `mob_display_names` holds what to show for a mob in place of that: the
-- way its plate prints a maturity graded by number (`04` where the bundled
-- list has `4`), learned from the screen (`source` 'plate'), or a label the
-- player chose ('player'), which a reading never replaces. A label changes
-- only what is shown; `species` and `maturity` are the mob as its kills are
-- recorded.
--
-- `kill_mob_assigned` keeps, for a kill the screen could not name that the
-- player named in the session's review, the stamp the kill had before and
-- what it was settled as, in the columns the other kept answers use, so the
-- review can put the stamp back.
--
-- Additive and forward-only.
CREATE TABLE mob_display_names (
    species    TEXT NOT NULL CHECK (species <> '' AND trim(species) = species),
    maturity   TEXT NOT NULL DEFAULT '',
    label      TEXT NOT NULL
               CHECK (label <> '' AND trim(label) = label AND length(label) <= 64),
    source     TEXT NOT NULL CHECK (source IN ('plate', 'player')),
    updated_at REAL NOT NULL,
    PRIMARY KEY (species, maturity)
);

CREATE TABLE kill_mob_assigned (
    kill_id           TEXT PRIMARY KEY REFERENCES kills(id),
    session_id        TEXT NOT NULL REFERENCES tracking_sessions(id),
    declared_name     TEXT NOT NULL,
    declared_species  TEXT NOT NULL,
    declared_maturity TEXT NOT NULL,
    declared_source   TEXT CHECK (declared_source = 'declared' OR declared_source IS NULL),
    reviewed_at       REAL NOT NULL,
    settled_kind      TEXT NOT NULL
                      CHECK (settled_kind IN ('listed', 'confirmed', 'new_name', 'declared')),
    settled_name      TEXT NOT NULL,
    settled_species   TEXT NOT NULL,
    settled_maturity  TEXT NOT NULL
);
CREATE INDEX idx_kill_mob_assigned_session ON kill_mob_assigned(session_id);

UPDATE kills
   SET mob_name = mob_species || ' ' || mob_maturity
 WHERE COALESCE(mob_maturity, '') <> ''
   AND mob_name = mob_maturity || ' ' || mob_species;

UPDATE kill_mob_detections
   SET mob_name = mob_species || ' ' || mob_maturity
 WHERE mob_maturity <> '' AND mob_name = mob_maturity || ' ' || mob_species;
UPDATE kill_mob_detections
   SET declared_name = declared_species || ' ' || declared_maturity
 WHERE declared_maturity <> ''
   AND declared_name = declared_maturity || ' ' || declared_species;
UPDATE kill_mob_detections
   SET settled_name = settled_species || ' ' || settled_maturity
 WHERE settled_maturity <> ''
   AND settled_name = settled_maturity || ' ' || settled_species;

UPDATE kill_mob_carried
   SET mob_name = mob_species || ' ' || mob_maturity
 WHERE mob_maturity <> '' AND mob_name = mob_maturity || ' ' || mob_species;
UPDATE kill_mob_carried
   SET declared_name = declared_species || ' ' || declared_maturity
 WHERE declared_maturity <> ''
   AND declared_name = declared_maturity || ' ' || declared_species;
UPDATE kill_mob_carried
   SET settled_name = settled_species || ' ' || settled_maturity
 WHERE settled_maturity <> ''
   AND settled_name = settled_maturity || ' ' || settled_species;
