-- How well the screen was read during a session, minute by minute.
--
-- One row for about each minute a watcher of the game window ran for a
-- session (`watcher`: the mob plate or the held-tool bar): how many looks
-- came due (`looks`), how many of those were skipped because the game was
-- in the background (`unfocused`), how many the cadence dropped because
-- the look before overran them (`missed`), how many got a frame
-- (`frames`), how many were given up because the pixels took too long to
-- grab (`late`), how many found what the watcher looks for (`found`) and,
-- of those, how many tried its name and could not read it (`unread`); the
-- longest look, the longest capture (finding the game window included)
-- and the longest grab of the pixels alone, in milliseconds; and how
-- contended the machine's processors were when the row was written
-- (`cpu_pressure`: the share of the last minute, 0 to 100, in which some
-- task waited for a processor; null where the system does not say).
--
-- It is a diagnostic record: nothing reads it to decide anything.
--
-- Additive and forward-only.
CREATE TABLE screen_look_minutes (
    session_id     TEXT NOT NULL REFERENCES tracking_sessions(id),
    watcher        TEXT NOT NULL CHECK (watcher IN ('mob', 'tool')),
    started_at     REAL NOT NULL,
    looks          INTEGER NOT NULL CHECK (looks >= 0),
    unfocused      INTEGER NOT NULL CHECK (unfocused >= 0),
    missed         INTEGER NOT NULL CHECK (missed >= 0),
    frames         INTEGER NOT NULL CHECK (frames >= 0),
    late           INTEGER NOT NULL CHECK (late >= 0),
    found          INTEGER NOT NULL CHECK (found >= 0),
    unread         INTEGER NOT NULL CHECK (unread >= 0),
    max_look_ms    INTEGER NOT NULL CHECK (max_look_ms >= 0),
    max_capture_ms INTEGER NOT NULL CHECK (max_capture_ms >= 0),
    max_grab_ms    INTEGER NOT NULL CHECK (max_grab_ms >= 0),
    cpu_pressure   REAL CHECK (cpu_pressure IS NULL OR cpu_pressure >= 0),
    PRIMARY KEY (session_id, watcher, started_at)
);
