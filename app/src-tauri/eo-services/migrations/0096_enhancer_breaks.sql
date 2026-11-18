-- Each enhancer break the tracker applied to the weapon in use: what is
-- known of it, and what that prices it at. Additive and forward-only: a
-- session recorded before this table existed has no rows and keeps its
-- figures.
--
-- A break consumes the enhancer: its TT and the markup the player paid for
-- it, less the Shrapnel the game returns, which the break's own chat line
-- states. Its inputs are kept (the enhancer and weapon names, the stated
-- return) beside what priced it: the TT the game data lists for the
-- enhancer and the markup set on the weapon, so a row whose inputs no
-- longer match the game data or the library is priced again. The return
-- deducted is the stated return, as far as the session's Shrapnel tagged as
-- enhancer returns reaches: a return that stayed loot is never deducted as
-- well. A break whose markup is not set costs its TT less the return, and
-- says so; a break whose TT is not known, or whose return exceeds the known
-- TT, has no cost and says why.
--
-- The cost is carried by the kill the break's shots went towards (in that
-- kill's enhancer cost, kept in step with these rows), or, for a break
-- after the session's last kill, by the session itself: such a row has no
-- kill. The row is dated by the chat line, never by when it was saved, and
-- keeps the activity context in force at the break.

CREATE TABLE enhancer_breaks (
    id            TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES tracking_sessions(id),
    kill_id       TEXT REFERENCES kills(id),
    context_id    INTEGER,
    observed_at   REAL NOT NULL,
    enhancer_name TEXT NOT NULL CHECK (length(enhancer_name) > 0),
    weapon_name   TEXT NOT NULL CHECK (length(weapon_name) > 0),
    returned_ped  REAL NOT NULL CHECK (returned_ped >= 0),
    deducted_ped  REAL NOT NULL CHECK (deducted_ped >= 0 AND deducted_ped <= returned_ped),
    markup_percent REAL CHECK (markup_percent IS NULL OR markup_percent >= 100),
    pricing       TEXT NOT NULL
                  CHECK (pricing IN ('priced', 'markupNotSet', 'ttUnknown', 'returnAboveTt')),
    tt_ped        REAL CHECK (tt_ped IS NULL OR tt_ped >= 0),
    markup_ped    REAL CHECK (markup_ped IS NULL OR markup_ped >= 0),
    cost_ped      REAL CHECK (cost_ped IS NULL OR cost_ped >= 0),
    CHECK ((pricing = 'ttUnknown') = (tt_ped IS NULL)),
    CHECK ((pricing = 'priced') = (markup_ped IS NOT NULL)),
    CHECK ((pricing IN ('priced', 'markupNotSet')) = (cost_ped IS NOT NULL))
);

CREATE INDEX idx_enhancer_breaks_session ON enhancer_breaks(session_id);
CREATE INDEX idx_enhancer_breaks_kill ON enhancer_breaks(kill_id) WHERE kill_id IS NOT NULL;
