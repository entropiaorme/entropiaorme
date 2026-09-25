//! The session-interval engine: intervals and the contexts that stamp
//! events with them.
//!
//! A session is not uniform. A pill holds for part of it, a quest
//! spans a stretch, and a player-drawn segment will slice one run. All
//! three are the same primitive, so they share this engine and differ
//! only by [`IntervalKind`].
//!
//! Two responsibilities, deliberately split (see `0019_session_intervals`):
//! an interval is authoritative for duration and cost, and a context is
//! authoritative for attribution. Opening or closing any interval mints
//! a fresh context; every event written afterwards stamps that context.
//! Attribution therefore never compares a wall-clock declaration against
//! a chat-log event timestamp, which is the one thing that would be
//! silently wrong here (the two clocks are an hour apart).

use crate::db::{Db, DbError};

/// What an interval records. An open vocabulary rather than a closed
/// enum at the schema boundary: adding a kind is a product decision, and
/// must not need a migration or a schema change on the event tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalKind {
    /// A skill-affecting modifier in force (a pill, a ring). Carries a
    /// magnitude, where zero means "declared, nothing in force".
    Modifier,
    /// A player-drawn slice of the session (one instance run, one
    /// rotation), opened and sealed from the overlay; sequential, so
    /// opening one closes the standing one.
    Segment,
    /// The stretch a declared quest spans.
    Quest,
    /// Reserved for a later consumable-timer kind; nothing writes it
    /// yet, and the engine needs no change when something does.
    Consumable,
    /// A protection loadout declared during play. Retired: protection
    /// costs are now spread by hits when recorded, so nothing opens one;
    /// the kind is still read so older sessions stay interpretable.
    Protection,
}

impl IntervalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            IntervalKind::Modifier => "modifier",
            IntervalKind::Segment => "segment",
            IntervalKind::Quest => "quest",
            IntervalKind::Consumable => "consumable",
            IntervalKind::Protection => "protection",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "modifier" => IntervalKind::Modifier,
            "segment" => IntervalKind::Segment,
            "quest" => IntervalKind::Quest,
            "consumable" => IntervalKind::Consumable,
            "protection" => IntervalKind::Protection,
            _ => return None,
        })
    }
}

/// The two interval kinds the Activities control declares, and the set
/// an exclusive switch seals: one control offers both, so a tap moving
/// from a quest to a segment must end the quest's stretch as surely as
/// a tap from one quest to another does.
pub const ACTIVITY_KINDS: [IntervalKind; 2] = [IntervalKind::Quest, IntervalKind::Segment];

/// A declaration of what the play from now on is. A quest stretch is
/// identified by the quest it advances and labelled with its name; a
/// segment is identified by the name the player gave it, which is all
/// such a slice has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityRef {
    Quest { quest_id: i64, name: String },
    Segment { name: String },
}

/// Which standing activity a deactivation ends. A quest needs only its
/// id, which is why this is not the declaration type above: the
/// completion bridge closing a stretch has no name to hand over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityKey {
    Quest(i64),
    Segment(String),
}

/// One standing activity, as the Activities control renders it. The
/// quest id is what a chip matches its roster row on; a segment carries
/// none, so its name is its identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveActivity {
    pub kind: IntervalKind,
    pub name: String,
    pub quest_id: Option<i64>,
}

/// Which standing intervals an open seals in the same transaction.
///
/// The scope is a property of the gesture, not of the kind: the same
/// Segment interval is opened sequentially by a boundary declaration
/// (which seals the previous slice and nothing else) and exclusively by
/// the Activities control's tap (which seals whatever activity was
/// standing, whichever kind it was).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseScope {
    /// Stack: nothing standing is sealed.
    Nothing,
    /// Seal the standing intervals of the opening kind, for the kinds
    /// that admit one at a time (a modifier declaration replaces the
    /// standing one; a segment boundary seals the previous slice).
    SameKind,
    /// Seal the standing intervals of any of these kinds, for a switch
    /// whose target may be of a different kind than what it replaces.
    Kinds(Vec<IntervalKind>),
}

impl CloseScope {
    /// Whether an open interval falls in scope for an open of `opening`.
    fn seals(&self, opening: IntervalKind, standing: IntervalKind) -> bool {
        match self {
            CloseScope::Nothing => false,
            CloseScope::SameKind => standing == opening,
            CloseScope::Kinds(kinds) => kinds.contains(&standing),
        }
    }
}

/// What to open, as one value rather than a run of positional
/// arguments whose order would be easy to transpose at a call site.
#[derive(Debug, Clone, PartialEq)]
pub struct IntervalSpec {
    pub kind: IntervalKind,
    pub label: Option<String>,
    pub ref_id: Option<i64>,
    pub magnitude: Option<f64>,
    /// What this open seals first; same-kind by default, which is the
    /// rule for every kind that admits one at a time.
    pub closes: CloseScope,
}

impl IntervalSpec {
    pub fn new(kind: IntervalKind) -> Self {
        Self {
            kind,
            label: None,
            ref_id: None,
            magnitude: None,
            closes: CloseScope::SameKind,
        }
    }

    pub fn label(mut self, label: Option<String>) -> Self {
        self.label = label;
        self
    }

    pub fn ref_id(mut self, ref_id: Option<i64>) -> Self {
        self.ref_id = ref_id;
        self
    }

    pub fn magnitude(mut self, magnitude: Option<f64>) -> Self {
        self.magnitude = magnitude;
        self
    }

    pub fn stacking(mut self) -> Self {
        self.closes = CloseScope::Nothing;
        self
    }

    /// Seal the standing intervals of these kinds instead of only the
    /// opening kind's: the Activities control's exclusive switch.
    pub fn closes(mut self, scope: CloseScope) -> Self {
        self.closes = scope;
        self
    }
}

/// One open interval, as the live session holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenInterval {
    pub id: i64,
    pub kind: IntervalKind,
    pub label: Option<String>,
    pub ref_id: Option<i64>,
    pub magnitude: Option<f64>,
}

/// The session's live interval state: which intervals are open, and the
/// context that identifies that set for stamping.
///
/// Held inside `ActiveSession`, so it is dropped wholesale when the
/// session stops and no interval state can leak into the next one.
#[derive(Debug, Default)]
pub struct IntervalState {
    open: Vec<OpenInterval>,
    context_id: Option<i64>,
}

impl IntervalState {
    /// The context every event written right now stamps. None only
    /// before the session's opening context has been minted.
    pub fn context_id(&self) -> Option<i64> {
        self.context_id
    }

    /// The open interval of a kind that admits only one at a time.
    pub fn open_of_kind(&self, kind: IntervalKind) -> Option<&OpenInterval> {
        self.open.iter().find(|interval| interval.kind == kind)
    }

    /// Every open interval of any of these kinds, in the order it was
    /// opened: the standing set an exclusive switch seals, and the
    /// order the Activities readout lists (opening order keeps a chip
    /// from jumping when another activity joins it).
    pub fn open_of_kinds(
        &self,
        kinds: &[IntervalKind],
    ) -> impl DoubleEndedIterator<Item = &OpenInterval> + '_ {
        let kinds = kinds.to_vec();
        self.open
            .iter()
            .filter(move |interval| kinds.contains(&interval.kind))
    }

    /// The open interval of a stacking kind that points at `ref_id`.
    pub fn open_of_ref(&self, kind: IntervalKind, ref_id: i64) -> Option<&OpenInterval> {
        self.open
            .iter()
            .find(|interval| interval.kind == kind && interval.ref_id == Some(ref_id))
    }

    /// The open interval of a kind carrying this label, compared
    /// case-insensitively: how a segment activity is identified, since
    /// a segment carries no reference of its own and its name is what
    /// the player typed.
    pub fn open_of_label(&self, kind: IntervalKind, label: &str) -> Option<&OpenInterval> {
        self.open.iter().find(|interval| {
            interval.kind == kind
                && interval
                    .label
                    .as_deref()
                    .is_some_and(|open| open.eq_ignore_ascii_case(label))
        })
    }

    /// The modifier magnitude in force, as the denormalised per-row
    /// mirror wants it. None when no modifier is declared at all, which
    /// is a different fact from a declared zero.
    pub fn modifier_magnitude(&self) -> Option<f64> {
        self.open_of_kind(IntervalKind::Modifier)
            .and_then(|interval| interval.magnitude)
    }
}

/// Stamp `ended_at` on the given interval rows, in one transaction so a
/// partial close cannot leave some members of a set open. Idempotent per
/// row: an already-ended row is left as it was.
async fn end_rows(db: &Db, now: f64, ids: impl IntoIterator<Item = i64>) -> Result<(), DbError> {
    let ids: Vec<i64> = ids.into_iter().collect();
    if ids.is_empty() {
        return Ok(());
    }
    db.with_writer(move |conn| {
        let tx = conn.transaction()?;
        for id in &ids {
            tx.execute(
                "UPDATE session_intervals SET ended_at = ? WHERE id = ? AND ended_at IS NULL",
                rusqlite::params![now, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
    .await
}

/// Mint a context for the given open set and make it current. Every
/// mutation routes through here, so a context and the set it names are
/// written together or not at all.
async fn mint_context(
    db: &Db,
    session_id: &str,
    now: f64,
    open: &[OpenInterval],
) -> Result<i64, DbError> {
    let session = session_id.to_string();
    let members: Vec<i64> = open.iter().map(|interval| interval.id).collect();
    db.with_writer(move |conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO session_contexts (session_id, created_at) VALUES (?, ?)",
            rusqlite::params![session, now],
        )?;
        let context_id = tx.last_insert_rowid();
        for interval_id in &members {
            tx.execute(
                "INSERT INTO session_context_intervals (context_id, interval_id) VALUES (?, ?)",
                rusqlite::params![context_id, interval_id],
            )?;
        }
        tx.commit()?;
        Ok(context_id)
    })
    .await
}

impl IntervalState {
    /// Mint the session's opening context: the empty set. An event
    /// stamped with it was recorded under the interval model with nothing
    /// declared, which the record must be able to tell apart from an
    /// event that predates the model (those carry no context at all).
    pub async fn open_session(
        &mut self,
        db: &Db,
        session_id: &str,
        now: f64,
    ) -> Result<(), DbError> {
        let context_id = mint_context(db, session_id, now, &[]).await?;
        self.open.clear();
        self.context_id = Some(context_id);
        Ok(())
    }

    /// Open an interval and adopt the context that includes it.
    ///
    /// The spec's [`CloseScope`] seals the standing intervals it names
    /// first, in the same motion: declaring a new modifier replaces the
    /// standing one rather than stacking a second silently, and the
    /// Activities switch seals whatever activity was standing.
    ///
    /// The whole transition is one transaction (the closes, the insert,
    /// the fresh context and its membership), and memory adopts the new
    /// state only after it commits: a failure leaves the database and
    /// the live set exactly as they were, so events keep stamping a
    /// context that still describes them.
    pub async fn open_interval(
        &mut self,
        db: &Db,
        session_id: &str,
        now: f64,
        spec: IntervalSpec,
    ) -> Result<i64, DbError> {
        let IntervalSpec {
            kind,
            label,
            ref_id,
            magnitude,
            closes,
        } = spec;
        let closing: Vec<i64> = self
            .open
            .iter()
            .filter(|interval| closes.seals(kind, interval.kind))
            .map(|interval| interval.id)
            .collect();
        let survivors: Vec<i64> = self
            .open
            .iter()
            .filter(|interval| !closing.contains(&interval.id))
            .map(|interval| interval.id)
            .collect();
        let session = session_id.to_string();
        let insert_label = label.clone();
        let kind_str = kind.as_str();
        let closing_tx = closing.clone();
        let (interval_id, context_id) = db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                for id in &closing_tx {
                    tx.execute(
                        "UPDATE session_intervals SET ended_at = ? \
                         WHERE id = ? AND ended_at IS NULL",
                        rusqlite::params![now, id],
                    )?;
                }
                tx.execute(
                    "INSERT INTO session_intervals \
                     (session_id, kind, label, ref_id, magnitude, started_at) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                    rusqlite::params![session, kind_str, insert_label, ref_id, magnitude, now],
                )?;
                let interval_id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO session_contexts (session_id, created_at) VALUES (?, ?)",
                    rusqlite::params![session, now],
                )?;
                let context_id = tx.last_insert_rowid();
                for member in survivors.iter().chain(std::iter::once(&interval_id)) {
                    tx.execute(
                        "INSERT INTO session_context_intervals (context_id, interval_id) \
                         VALUES (?, ?)",
                        rusqlite::params![context_id, member],
                    )?;
                }
                tx.commit()?;
                Ok((interval_id, context_id))
            })
            .await?;
        self.open.retain(|interval| !closing.contains(&interval.id));
        self.open.push(OpenInterval {
            id: interval_id,
            kind,
            label,
            ref_id,
            magnitude,
        });
        self.context_id = Some(context_id);
        Ok(interval_id)
    }

    /// Close every open interval of a kind and adopt the narrower
    /// context. Returns what was closed.
    pub async fn close_kind(
        &mut self,
        db: &Db,
        session_id: &str,
        now: f64,
        kind: IntervalKind,
    ) -> Result<Vec<OpenInterval>, DbError> {
        self.close_matching(db, session_id, now, |interval| interval.kind == kind)
            .await
    }

    /// Close the open interval of a kind that points at `ref_id`, and
    /// adopt the narrower context. Returns what was closed.
    ///
    /// Distinct from [`close_kind`](Self::close_kind) because a stacking
    /// kind ends one member at a time: three daily quests can run at
    /// once, and finishing one must not end the other two.
    pub async fn close_ref(
        &mut self,
        db: &Db,
        session_id: &str,
        now: f64,
        kind: IntervalKind,
        ref_id: i64,
    ) -> Result<Vec<OpenInterval>, DbError> {
        self.close_matching(db, session_id, now, |interval| {
            interval.kind == kind && interval.ref_id == Some(ref_id)
        })
        .await
    }

    /// Close specific open intervals by id, and adopt the narrower
    /// context: how a kind without a reference of its own (a segment,
    /// identified by its name) ends exactly the one the user named.
    pub async fn close_ids(
        &mut self,
        db: &Db,
        session_id: &str,
        now: f64,
        ids: &[i64],
    ) -> Result<Vec<OpenInterval>, DbError> {
        let ids = ids.to_vec();
        self.close_matching(db, session_id, now, move |interval| {
            ids.contains(&interval.id)
        })
        .await
    }

    /// Close every open interval of these kinds EXCEPT the one with
    /// `keep_id`, and adopt the narrower context: the exclusive switch
    /// onto an activity that is already standing. The kept interval
    /// survives with its stretch intact, because closing and reopening
    /// it would split one continuous stretch into two.
    pub async fn close_kinds_except_id(
        &mut self,
        db: &Db,
        session_id: &str,
        now: f64,
        kinds: &[IntervalKind],
        keep_id: i64,
    ) -> Result<Vec<OpenInterval>, DbError> {
        self.close_matching(db, session_id, now, |interval| {
            kinds.contains(&interval.kind) && interval.id != keep_id
        })
        .await
    }

    /// The shared close transition: end the matching rows and mint the
    /// narrower context in one transaction, adopting both in memory only
    /// after the commit. A failure therefore leaves the standing set and
    /// its context untouched rather than half-closed.
    async fn close_matching(
        &mut self,
        db: &Db,
        session_id: &str,
        now: f64,
        matches: impl Fn(&OpenInterval) -> bool,
    ) -> Result<Vec<OpenInterval>, DbError> {
        let closing_ids: Vec<i64> = self
            .open
            .iter()
            .filter(|interval| matches(interval))
            .map(|interval| interval.id)
            .collect();
        if closing_ids.is_empty() {
            return Ok(Vec::new());
        }
        let keeping_ids: Vec<i64> = self
            .open
            .iter()
            .filter(|interval| !closing_ids.contains(&interval.id))
            .map(|interval| interval.id)
            .collect();
        let session = session_id.to_string();
        let closing_tx = closing_ids.clone();
        let context_id = db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                for id in &closing_tx {
                    tx.execute(
                        "UPDATE session_intervals SET ended_at = ? \
                         WHERE id = ? AND ended_at IS NULL",
                        rusqlite::params![now, id],
                    )?;
                }
                tx.execute(
                    "INSERT INTO session_contexts (session_id, created_at) VALUES (?, ?)",
                    rusqlite::params![session, now],
                )?;
                let context_id = tx.last_insert_rowid();
                for member in &keeping_ids {
                    tx.execute(
                        "INSERT INTO session_context_intervals (context_id, interval_id) \
                         VALUES (?, ?)",
                        rusqlite::params![context_id, member],
                    )?;
                }
                tx.commit()?;
                Ok(context_id)
            })
            .await?;
        let (closed, keeping): (Vec<_>, Vec<_>) = std::mem::take(&mut self.open)
            .into_iter()
            .partition(|interval| closing_ids.contains(&interval.id));
        self.open = keeping;
        self.context_id = Some(context_id);
        Ok(closed)
    }

    /// Close everything still open at the session's end, so no interval
    /// outlives the session that owns it. The rows end in one
    /// transaction; the in-memory clear follows the commit (the whole
    /// `ActiveSession` drops immediately afterwards anyway).
    pub async fn close_session(&mut self, db: &Db, now: f64) -> Result<(), DbError> {
        let ids: Vec<i64> = self.open.iter().map(|interval| interval.id).collect();
        end_rows(db, now, ids).await?;
        self.open.clear();
        self.context_id = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_kind_round_trips_its_wire_string() {
        for kind in [
            IntervalKind::Modifier,
            IntervalKind::Segment,
            IntervalKind::Quest,
            IntervalKind::Consumable,
            IntervalKind::Protection,
        ] {
            assert_eq!(IntervalKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(IntervalKind::parse("nonsense"), None);
    }

    /// A declared zero is a real modifier magnitude: it records "I am
    /// deliberately unboosted", which is the baseline an effect can be
    /// measured against. Only an absent interval means "not declared".
    #[test]
    fn a_declared_zero_magnitude_is_not_an_absent_modifier() {
        let mut state = IntervalState::default();
        assert_eq!(state.modifier_magnitude(), None);

        state.open.push(OpenInterval {
            id: 1,
            kind: IntervalKind::Modifier,
            label: None,
            ref_id: None,
            magnitude: Some(0.0),
        });
        assert_eq!(state.modifier_magnitude(), Some(0.0));
    }
}
