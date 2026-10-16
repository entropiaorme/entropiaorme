-- Per-weapon cells of a settled session, so skilling reads that narrow
-- recorded play to one weapon fold cells rather than raw kills.
--
-- A weapon's part of a session is the kills it made alone (a kill shared
-- with another tool is no one's). play_seconds is the time leading up to
-- those kills and active_seconds the session's whole active time (every
-- gap between recorded kills and harvesting swings short of a break), so
-- play_seconds / active_seconds is the weapon's share of the session.
CREATE TABLE session_weapon_rollups (
    session_id     TEXT NOT NULL,
    tool_name      TEXT NOT NULL,
    kills          INTEGER NOT NULL,
    cycled_ped     REAL NOT NULL,
    loot_tt        REAL NOT NULL,
    play_seconds   REAL NOT NULL,
    active_seconds REAL NOT NULL,
    PRIMARY KEY (session_id, tool_name)
);

-- The skill gains that led up to a weapon's own kills: each gain belongs
-- to the next kill or swing in its session.
CREATE TABLE session_weapon_pes_rollups (
    session_id TEXT NOT NULL,
    tool_name  TEXT NOT NULL,
    skill_name TEXT NOT NULL,
    pes        REAL NOT NULL,
    PRIMARY KEY (session_id, tool_name, skill_name)
);

-- The pace of a weapon's own kills per mob: kills made within a break of
-- the kill before, the gaps that led up to them, and the shots they took.
CREATE TABLE session_weapon_pace_rollups (
    session_id   TEXT NOT NULL,
    tool_name    TEXT NOT NULL,
    mob_species  TEXT NOT NULL,
    mob_maturity TEXT NOT NULL,
    kills        INTEGER NOT NULL,
    gap_seconds  REAL NOT NULL,
    shots        REAL NOT NULL,
    PRIMARY KEY (session_id, tool_name, mob_species, mob_maturity)
);

CREATE INDEX idx_session_weapon_rollups_tool
    ON session_weapon_rollups (tool_name);
CREATE INDEX idx_session_weapon_pace_rollups_mob
    ON session_weapon_pace_rollups (mob_species, mob_maturity);
