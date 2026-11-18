-- Why creature detection gave each kill the answer it did.
--
-- One row for every kill detection answered while the screen was read,
-- named or not: what the screen's own readings came to (`screen`: a grade
-- the kill was given or suggested, or why it named nothing), whether the
-- creature being hunted was carried onto it and, when it was not, why
-- (`continuity`), and the plate readings round the kill (`reads`, a JSON
-- array of objects, each with `at_ms` (from the kill's logged second), `fill`
-- (the creature's health, 0 to 1), `text` (the name as read, or null), and
-- `named` (what the reading named, or null)). It is a diagnostic record:
-- nothing reads it to decide a stamp, and the kill's stamp stays the truth.
--
-- Additive and forward-only.
CREATE TABLE kill_mob_traces (
    kill_id    TEXT PRIMARY KEY REFERENCES kills(id),
    session_id TEXT NOT NULL REFERENCES tracking_sessions(id),
    screen     TEXT NOT NULL CHECK (screen IN (
                   'named', 'guess', 'provisional', 'provisional_guess',
                   'latest_read', 'provisional_latest_read',
                   'no_fight', 'still_read', 'indistinct', 'undecided',
                   'unwatched', 'implausible')),
    continuity TEXT NOT NULL CHECK (continuity IN (
                   'not_needed', 'given', 'plate_read', 'too_few', 'disagree',
                   'mixed', 'over_share', 'contradicted')),
    reads      TEXT NOT NULL CHECK (json_valid(reads) AND json_type(reads) = 'array')
);
CREATE INDEX idx_kill_mob_traces_session ON kill_mob_traces(session_id);
