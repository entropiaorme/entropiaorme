-- Things read off the game screen as the tool in hand that are not entries
-- of the player's equipment library, and names the player said are not
-- tracked tools.
--
-- `tool_sightings` holds at most one row per session per distinct thing:
--   bundled   a tool of the bundled game data, by its collection
--             (`endpoint`), its id there and its name;
--   unknown   a name read that matched no tool, by a key of the text and
--             the text as read.
-- `first_seen` and `last_seen` are observed times (epoch seconds). For a
-- bundled tool, `shots` and `cost_per_shot` are the shots booked to it and
-- the bare cost of each (PED), NULL while not known; an unknown name has
-- neither. The settlement columns record what became of a sighting (added
-- to the library, with the entry it became; dismissed; or left) and when,
-- NULL while unsettled.
--
-- `dismissed_tool_names` holds the names the player said are not tracked
-- tools, unique without regard to case, and when each was dismissed.

CREATE TABLE tool_sightings (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES tracking_sessions(id),
    kind          TEXT NOT NULL CHECK (kind IN ('bundled', 'unknown')),
    endpoint      TEXT,
    item_id       TEXT,
    item_name     TEXT,
    text_key      TEXT,
    text_read     TEXT,
    first_seen    REAL NOT NULL,
    last_seen     REAL NOT NULL,
    shots         INTEGER CHECK (shots IS NULL OR shots >= 0),
    cost_per_shot REAL CHECK (cost_per_shot IS NULL OR cost_per_shot >= 0),
    settled_kind  TEXT CHECK (settled_kind IN ('added', 'dismissed', 'left')
                              OR settled_kind IS NULL),
    settled_at    REAL,
    equipment_id  INTEGER REFERENCES equipment_library(id),
    CHECK (kind <> 'bundled' OR (
        endpoint IS NOT NULL AND item_id IS NOT NULL AND item_name IS NOT NULL
        AND text_key IS NULL AND text_read IS NULL)),
    CHECK (kind <> 'unknown' OR (
        text_key IS NOT NULL AND trim(text_key) <> '' AND text_read IS NOT NULL
        AND endpoint IS NULL AND item_id IS NULL AND item_name IS NULL
        AND shots IS NULL AND cost_per_shot IS NULL)),
    CHECK (last_seen >= first_seen),
    CHECK ((settled_kind IS NULL) = (settled_at IS NULL)),
    CHECK (equipment_id IS NULL OR settled_kind = 'added')
);
CREATE UNIQUE INDEX idx_tool_sightings_bundled
    ON tool_sightings(session_id, endpoint, item_id) WHERE kind = 'bundled';
CREATE UNIQUE INDEX idx_tool_sightings_unknown
    ON tool_sightings(session_id, text_key) WHERE kind = 'unknown';

CREATE TABLE dismissed_tool_names (
    name         TEXT PRIMARY KEY COLLATE NOCASE CHECK (trim(name) <> ''),
    dismissed_at REAL NOT NULL
);

-- Additive and forward-only.
