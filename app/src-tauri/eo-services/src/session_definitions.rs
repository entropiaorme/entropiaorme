//! Session definitions: the deliberate activity families tracked
//! sessions are instances of.
//!
//! A definition is authored data, not machinery: a name, an opt-in
//! flag for free-text segment naming, and an ordered roster of the
//! activities the family rehearses (a quest family, a single quest, or
//! a plain segment label). Tracked sessions reference a definition
//! through a nullable id stamped at start, while the session's own
//! `session_name` column is stamped with the definition's name at the
//! moment of selection: the stamp is the durable per-session fact (a
//! later rename or archive never rewrites history), the reference is
//! the instance-to-family identity aggregation reads.
//!
//! The roster is replaced wholesale on update: entries have no update
//! lifecycle of their own. Entry
//! references are validated against ACTIVE targets at write time; a
//! target soft-deleted afterwards surfaces on read as a missing
//! reference rather than silently disappearing, so the author can see
//! and repair the hole.

use std::sync::Arc;

use crate::clock::Clock;
use crate::db::{Db, DbError};

/// The service's error surface: `Invalid` is a caller error carrying
/// the rejection message verbatim, `Conflict` protects live state, and
/// `Db` is an infrastructure failure.
#[derive(Debug, thiserror::Error)]
pub enum SessionDefinitionError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Conflict(String),
    #[error(transparent)]
    Db(#[from] DbError),
}

/// What a roster entry references; the stored/wire vocabulary is the
/// snake_case string form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosterEntryKind {
    /// A quest family (`ref_id` -> `quest_families.id`): the entry
    /// stands for whichever variant the family serves today.
    QuestFamily,
    /// A single quest (`ref_id` -> `quests.id`): a signal-completed
    /// boss or a standalone mission-log quest outside any family.
    Quest,
    /// A plain authored segment label; no reference.
    Segment,
}

impl RosterEntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RosterEntryKind::QuestFamily => "quest_family",
            RosterEntryKind::Quest => "quest",
            RosterEntryKind::Segment => "segment",
        }
    }

    /// Parse the stored/wire vocabulary; anything else is a caller error.
    pub fn parse(value: &str) -> Result<Self, SessionDefinitionError> {
        match value {
            "quest_family" => Ok(RosterEntryKind::QuestFamily),
            "quest" => Ok(RosterEntryKind::Quest),
            "segment" => Ok(RosterEntryKind::Segment),
            other => Err(SessionDefinitionError::Invalid(format!(
                "roster entry kind must be 'quest_family', 'quest' or 'segment', not '{other}'"
            ))),
        }
    }
}

/// A roster entry as authored (create/update input).
#[derive(Debug, Clone)]
pub struct RosterEntryInput {
    pub kind: RosterEntryKind,
    /// Required for the referencing kinds; must name an ACTIVE target.
    pub ref_id: Option<i64>,
    /// Required (non-blank) for `Segment`; ignored otherwise.
    pub label: Option<String>,
}

/// A roster entry as read: the stored fact plus the resolved display
/// name of its target.
#[derive(Debug, Clone)]
pub struct RosterEntry {
    pub id: i64,
    pub position: i64,
    pub kind: RosterEntryKind,
    pub ref_id: Option<i64>,
    pub label: Option<String>,
    /// The referenced target's current name (or the segment label).
    /// `None` for a referencing entry whose target has since been
    /// deleted: the hole is surfaced, never silently dropped.
    pub display_name: Option<String>,
}

/// A session definition as read: the authored fields plus the derived
/// instance count (how many tracked sessions reference it). The count is
/// the aggregation key's own evidence: it is what a
/// lifetime-over-all-instances readout can be computed against without a
/// second round trip, which is why the read model carries it.
#[derive(Debug, Clone)]
pub struct SessionDefinition {
    pub id: i64,
    pub name: String,
    pub ad_hoc_segments: bool,
    pub track_protection_costs: bool,
    pub is_active: bool,
    /// A definition that must not be archived: tracking always has one
    /// to be an instance of, and this is the one that guarantees it.
    /// Protection is about existence, not identity: the row is renamed
    /// and rostered like any other.
    pub is_protected: bool,
    pub created_at: f64,
    pub updated_at: Option<f64>,
    pub instance_count: i64,
    pub roster: Vec<RosterEntry>,
}

/// The completed archive transition. The protected fallback is resolved
/// inside the same database transaction that archives the definition, so
/// callers can persist a new selection without racing a now-inactive row.
#[derive(Debug, Clone)]
pub struct DefinitionArchiveOutcome {
    pub definition: SessionDefinition,
    pub fallback: Option<(i64, String)>,
}

enum ArchiveTransition {
    Archived(Option<(i64, String)>),
    Missing,
    Protected(String),
    InUse,
}

enum RestoreTransition {
    Restored,
    Missing,
    Conflict,
}

/// The lifetime aggregate over one definition's recorded instances:
/// what the family has cost and returned across every session run
/// under it, as opposed to the instance currently in play.
///
/// Every field is a plain sum of per-instance totals, which is what
/// makes the aggregate mean what it says. Derived figures (net, return
/// rate) are deliberately NOT stored here: they are computed from these
/// sums at the point of display, because a rate is the ratio of the
/// summed parts and never the mean of the per-instance rates. Averaging
/// the rates would let a four-minute lucky run outweigh a three-hour
/// grind.
///
/// `instance_count` counts exactly the instances these sums are taken
/// over, so the span a surface discloses can never disagree with the
/// figures beside it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DefinitionLifetimeStats {
    pub instance_count: i64,
    /// Summed cycled spend. Note this rides the summary's basis, which
    /// includes the armour and dangling costs a session only acquires
    /// once it has ended; an in-flight instance's own figure has not
    /// picked those up yet, so the family total is legitimately the
    /// more complete of the two.
    pub cycled: f64,
    pub loot_tt: f64,
    /// Summed skill TT on the raw `SUM(ped_value)` basis, matching the
    /// live readout's own PES figure rather than the summary's
    /// positive-only per-skill total.
    pub pes: f64,
    pub duration_seconds: f64,
}

/// A definition create/update payload. The roster always binds in
/// full: update replaces the stored roster wholesale.
#[derive(Debug, Clone)]
pub struct SessionDefinitionInput {
    pub name: String,
    pub ad_hoc_segments: bool,
    pub track_protection_costs: bool,
    pub roster: Vec<RosterEntryInput>,
}

/// Session-definition lifecycle over the shared DB seam. Pure
/// request/reply state in SQLite; no actor loop and no bus traffic (the
/// frontend refetches after its own mutations, the quest-families
/// precedent).
pub struct SessionDefinitionService {
    db: Db,
    clock: Arc<dyn Clock>,
    /// Whether this process has already converged the summary rows the
    /// lifetime read aggregates over. See [`Self::lifetime_stats`].
    summaries_healed: std::sync::atomic::AtomicBool,
}

const DEFINITION_SELECT: &str = "\
    SELECT d.id, d.name, d.ad_hoc_segments, d.track_protection_costs, \
           d.is_active, d.is_protected, \
           d.created_at, d.updated_at, \
           (SELECT COUNT(*) FROM tracking_sessions s \
            WHERE s.definition_id = d.id) AS instance_count \
    FROM session_definitions d";

/// The roster read: each entry joined against its target's live name.
/// Only ACTIVE targets resolve; a soft-deleted target reads as NULL
/// (`RosterEntry::display_name` documents the contract).
const ROSTER_SELECT: &str = "\
    SELECT r.id, r.position, r.kind, r.ref_id, r.label, \
           CASE r.kind \
             WHEN 'segment' THEN r.label \
             WHEN 'quest_family' THEN \
               (SELECT f.name FROM quest_families f \
                WHERE f.id = r.ref_id AND f.is_active = 1) \
             WHEN 'quest' THEN \
               (SELECT q.name FROM quests q \
                WHERE q.id = r.ref_id AND q.is_active = 1) \
           END AS display_name \
    FROM session_definition_roster r \
    WHERE r.definition_id = ? \
    ORDER BY r.position ASC, r.id ASC";

impl SessionDefinitionService {
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            db,
            clock,
            summaries_healed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// List definitions (active only by default), oldest-authored first.
    pub async fn list(
        &self,
        active_only: bool,
    ) -> Result<Vec<SessionDefinition>, SessionDefinitionError> {
        let where_clause = if active_only {
            "WHERE d.is_active = 1"
        } else {
            ""
        };
        let sql = format!("{DEFINITION_SELECT} {where_clause} ORDER BY d.created_at ASC, d.id ASC");
        Ok(self
            .db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(&sql)?;
                let mut rows = stmt.query([])?;
                let mut headers = Vec::new();
                while let Some(row) = rows.next()? {
                    headers.push(row_to_definition(row)?);
                }
                drop(rows);
                drop(stmt);
                let mut out = Vec::with_capacity(headers.len());
                for mut definition in headers {
                    definition.roster = read_roster(conn, definition.id)?;
                    out.push(definition);
                }
                Ok(out)
            })
            .await?)
    }

    /// A single definition by id; `None` when absent.
    pub async fn get(
        &self,
        definition_id: i64,
    ) -> Result<Option<SessionDefinition>, SessionDefinitionError> {
        let sql = format!("{DEFINITION_SELECT} WHERE d.id = ?");
        Ok(self
            .db
            .with_reader(move |conn| {
                use rusqlite::OptionalExtension as _;
                let header = conn
                    .prepare(&sql)?
                    .query_row(rusqlite::params![definition_id], row_to_definition)
                    .optional()?;
                let Some(mut definition) = header else {
                    return Ok(None);
                };
                definition.roster = read_roster(conn, definition.id)?;
                Ok(Some(definition))
            })
            .await?)
    }

    /// The ACTIVE definition by id; `None` when absent or archived. The
    /// selection/start paths read through this so a stale reference
    /// (a definition archived after being picked) resolves to "none"
    /// rather than stamping a dead id.
    pub async fn get_active(
        &self,
        definition_id: i64,
    ) -> Result<Option<SessionDefinition>, SessionDefinitionError> {
        Ok(self
            .get(definition_id)
            .await?
            .filter(|definition| definition.is_active))
    }

    /// The lifetime aggregate over a definition's ENDED instances.
    ///
    /// Sums the materialised per-session summaries, which exist only
    /// for ended sessions that cycled something over a non-zero
    /// duration. A session started and immediately abandoned therefore
    /// never reaches these totals, and neither does the instance
    /// currently in play: the caller adds the live figures on top,
    /// since only it knows whether the running session belongs to this
    /// definition.
    ///
    /// Summaries converge lazily by design (readers heal what a
    /// version bump or a widened filter left missing), but this read
    /// sits behind the tracking snapshot, which polls. So the heal runs
    /// once per process on success: enough to converge a database on
    /// the first frame after an update, without putting a write on a
    /// hot path.
    ///
    /// The flag is published only once the heal has actually landed. A
    /// heal that fails must leave it clear, or one transient writer
    /// error would silently aggregate over incomplete rows for the rest
    /// of the process. Two callers racing the first heal is harmless by
    /// comparison: `heal_summaries` is idempotent and the writer core
    /// serialises them, so the loser finds nothing left to do.
    pub async fn lifetime_stats(
        &self,
        definition_id: i64,
    ) -> Result<DefinitionLifetimeStats, SessionDefinitionError> {
        use std::sync::atomic::Ordering;
        if !self.summaries_healed.load(Ordering::SeqCst) {
            self.db
                .with_writer(|conn| crate::session_summary::heal_summaries(conn))
                .await?;
            self.summaries_healed.store(true, Ordering::SeqCst);
        }
        Ok(self
            .db
            .with_reader(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*), \
                            COALESCE(SUM(ss.cycled_ped), 0), \
                            COALESCE(SUM(ss.loot_tt), 0), \
                            COALESCE(SUM(ss.activity_skill_tt), 0), \
                            COALESCE(SUM(ss.duration_hours), 0) \
                     FROM session_summaries ss \
                     JOIN tracking_sessions s ON s.id = ss.session_id \
                     WHERE s.definition_id = ?",
                    rusqlite::params![definition_id],
                    |row| {
                        Ok(DefinitionLifetimeStats {
                            instance_count: row.get(0)?,
                            cycled: row.get(1)?,
                            loot_tt: row.get(2)?,
                            pes: row.get(3)?,
                            duration_seconds: row.get::<_, f64>(4)? * 3600.0,
                        })
                    },
                )?)
            })
            .await?)
    }

    /// Create a definition with its roster.
    pub async fn create(
        &self,
        input: SessionDefinitionInput,
    ) -> Result<SessionDefinition, SessionDefinitionError> {
        let name = validated_name(&input.name)?;
        self.require_name_free(&name, None).await?;
        let roster = self.validated_roster(&input.roster).await?;

        let ad_hoc = input.ad_hoc_segments;
        let track_protection_costs = input.track_protection_costs;
        let definition_id = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    // The per-segment armour flag is retired; see the
                    // protection service for why costs no longer need it.
                    "INSERT INTO session_definitions \
                     (name, ad_hoc_segments, track_protection_costs, track_protection_by_segment) \
                     VALUES (?, ?, ?, 0)",
                    rusqlite::params![name, ad_hoc as i64, track_protection_costs as i64],
                )?;
                let definition_id = tx.last_insert_rowid();
                write_roster(&tx, definition_id, &roster)?;
                tx.commit()?;
                Ok(definition_id)
            })
            .await?;
        Ok(self
            .get(definition_id)
            .await?
            .expect("the definition was just inserted"))
    }

    /// Update a definition; the roster is replaced wholesale. `None`
    /// when absent; an archived definition reads as absent (mutating
    /// one would resurrect it into pickers that filter on active).
    pub async fn update(
        &self,
        definition_id: i64,
        input: SessionDefinitionInput,
    ) -> Result<Option<SessionDefinition>, SessionDefinitionError> {
        match self.get(definition_id).await? {
            Some(existing) if existing.is_active => {}
            _ => return Ok(None),
        }
        let name = validated_name(&input.name)?;
        self.require_name_free(&name, Some(definition_id)).await?;
        let roster = self.validated_roster(&input.roster).await?;

        let ad_hoc = input.ad_hoc_segments;
        let track_protection_costs = input.track_protection_costs;
        let now = self.now_epoch();
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "UPDATE session_definitions \
                     SET name = ?, ad_hoc_segments = ?, track_protection_costs = ?, \
                         updated_at = ? \
                     WHERE id = ?",
                    rusqlite::params![
                        name,
                        ad_hoc as i64,
                        track_protection_costs as i64,
                        now,
                        definition_id
                    ],
                )?;
                tx.execute(
                    "DELETE FROM session_definition_roster WHERE definition_id = ?",
                    rusqlite::params![definition_id],
                )?;
                write_roster(&tx, definition_id, &roster)?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        self.get(definition_id).await
    }

    /// Append a segment named during play to a definition's roster, so
    /// it is a one-tap chip next time.
    ///
    /// The one roster write that is not authoring: a name typed into the
    /// Activities control is a declaration as deliberate as one seeded
    /// in the editor, and promoting it is what makes self-named segments
    /// worth switching on. Deduplicated case-insensitively against every
    /// entry's display name (a family's name included), because a second
    /// chip reading the same as an existing one is noise whatever kind
    /// it is. Refuses to resurrect an archived definition, exactly as
    /// [`update`](Self::update) does. Returns whether a row was added.
    pub async fn promote_segment(
        &self,
        definition_id: i64,
        label: &str,
    ) -> Result<bool, SessionDefinitionError> {
        let label = label.trim().to_string();
        if label.is_empty() {
            return Ok(false);
        }
        match self.get(definition_id).await? {
            Some(existing) if existing.is_active => {
                if existing.roster.iter().any(|entry| {
                    entry
                        .display_name
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(&label))
                }) {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
        let now = self.now_epoch();
        Ok(self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                // The position is read inside the transaction: two
                // promotions racing must not land on one position.
                let next_position: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(position), -1) + 1 \
                     FROM session_definition_roster WHERE definition_id = ?",
                    rusqlite::params![definition_id],
                    |row| row.get(0),
                )?;
                let added = tx.execute(
                    "INSERT INTO session_definition_roster \
                     (definition_id, position, kind, ref_id, label) \
                     VALUES (?, ?, 'segment', NULL, ?)",
                    rusqlite::params![definition_id, next_position, label],
                )?;
                tx.execute(
                    "UPDATE session_definitions SET updated_at = ? WHERE id = ?",
                    rusqlite::params![now, definition_id],
                )?;
                tx.commit()?;
                Ok(added > 0)
            })
            .await?)
    }

    /// Archive a definition without touching its roster or recorded
    /// instances. The protected active fallback is resolved in the same
    /// transaction, making the transition safe for a caller whose current
    /// selection names the archived definition.
    pub async fn archive(
        &self,
        definition_id: i64,
    ) -> Result<Option<DefinitionArchiveOutcome>, SessionDefinitionError> {
        let now = self.now_epoch();
        let transition = self
            .db
            .with_writer(move |conn| {
                use rusqlite::OptionalExtension as _;
                let tx = conn.transaction()?;
                let state = tx
                    .query_row(
                        "SELECT is_active, is_protected, name FROM session_definitions WHERE id = ?",
                        rusqlite::params![definition_id],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)? != 0,
                                row.get::<_, i64>(1)? != 0,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .optional()?;
                let Some((is_active, is_protected, name)) = state else {
                    return Ok(ArchiveTransition::Missing);
                };
                if !is_active {
                    return Ok(ArchiveTransition::Missing);
                }
                if is_protected {
                    return Ok(ArchiveTransition::Protected(name));
                }
                let in_use = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM tracking_sessions \
                     WHERE is_active = 1 AND definition_id = ?)",
                    rusqlite::params![definition_id],
                    |row| row.get::<_, i64>(0),
                )? != 0;
                if in_use {
                    return Ok(ArchiveTransition::InUse);
                }

                tx.execute(
                    "UPDATE session_definitions SET is_active = 0, updated_at = ? \
                     WHERE id = ?",
                    rusqlite::params![now, definition_id],
                )?;
                // id-order: insertion (the first seeded protected
                // definition is the fallback).
                let fallback = tx
                    .query_row(
                        "SELECT id, name FROM session_definitions \
                         WHERE is_active = 1 AND is_protected = 1 \
                         ORDER BY id ASC LIMIT 1",
                        [],
                        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()?;
                tx.commit()?;
                Ok(ArchiveTransition::Archived(fallback))
            })
            .await?;
        let fallback = match transition {
            ArchiveTransition::Archived(fallback) => fallback,
            ArchiveTransition::Missing => return Ok(None),
            ArchiveTransition::Protected(name) => {
                return Err(SessionDefinitionError::Invalid(format!(
                    "{name} cannot be archived; tracking always needs one to run under"
                )));
            }
            ArchiveTransition::InUse => {
                return Err(SessionDefinitionError::Conflict(
                    "The session in play cannot be archived until tracking ends".to_string(),
                ));
            }
        };
        let definition = self
            .get(definition_id)
            .await?
            .expect("the archived definition remains addressable");
        Ok(Some(DefinitionArchiveOutcome {
            definition,
            fallback,
        }))
    }

    /// Restore an archived definition to the active catalogue. Its authored
    /// roster and recorded instances have remained intact throughout. A name
    /// that became occupied while it was archived must be resolved by the
    /// user rather than silently splitting one definition's identity.
    pub async fn restore(
        &self,
        definition_id: i64,
    ) -> Result<Option<SessionDefinition>, SessionDefinitionError> {
        let Some(existing) = self.get(definition_id).await? else {
            return Ok(None);
        };
        if existing.is_active {
            return Ok(None);
        }

        let now = self.now_epoch();
        let name = existing.name;
        let candidate = name.to_ascii_lowercase();
        let transition = self
            .db
            .with_writer(move |conn| {
                use rusqlite::OptionalExtension as _;
                let tx = conn.transaction()?;
                let active = tx
                    .query_row(
                        "SELECT is_active FROM session_definitions WHERE id = ?",
                        rusqlite::params![definition_id],
                        |row| Ok(row.get::<_, i64>(0)? != 0),
                    )
                    .optional()?;
                if active != Some(false) {
                    return Ok(RestoreTransition::Missing);
                }
                let taken = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_definitions \
                     WHERE is_active = 1 AND lower(name) = ? AND id != ?)",
                    rusqlite::params![candidate, definition_id],
                    |row| row.get::<_, i64>(0),
                )? != 0;
                if taken {
                    return Ok(RestoreTransition::Conflict);
                }
                tx.execute(
                    "UPDATE session_definitions SET is_active = 1, updated_at = ? \
                     WHERE id = ? AND is_active = 0",
                    rusqlite::params![now, definition_id],
                )?;
                tx.commit()?;
                Ok(RestoreTransition::Restored)
            })
            .await?;
        match transition {
            RestoreTransition::Restored => self.get(definition_id).await,
            RestoreTransition::Missing => Ok(None),
            RestoreTransition::Conflict => Err(SessionDefinitionError::Invalid(format!(
                "A session named '{name}' already exists"
            ))),
        }
    }

    /// Refuse a name already carried by another ACTIVE definition
    /// (case-insensitive): the picker keys on names, and a duplicate
    /// would split one family's instances invisibly.
    async fn require_name_free(
        &self,
        name: &str,
        exclude_id: Option<i64>,
    ) -> Result<(), SessionDefinitionError> {
        // ASCII folding on both sides: SQLite's `lower()` maps only
        // A-Z, so folding the candidate any wider would compare two
        // different rules and let a non-ASCII case variant through as
        // "unique" against a query that never saw it that way.
        let candidate = name.to_ascii_lowercase();
        let taken = self
            .db
            .with_reader(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM session_definitions \
                     WHERE is_active = 1 AND lower(name) = ? AND id != ?",
                    rusqlite::params![candidate, exclude_id.unwrap_or(-1)],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await?;
        if taken > 0 {
            return Err(SessionDefinitionError::Invalid(format!(
                "A session named '{name}' already exists"
            )));
        }
        Ok(())
    }

    /// Validate the authored roster: kinds carry the right payload and
    /// every reference names an ACTIVE target.
    async fn validated_roster(
        &self,
        entries: &[RosterEntryInput],
    ) -> Result<Vec<RosterEntryInput>, SessionDefinitionError> {
        let mut validated = Vec::with_capacity(entries.len());
        for entry in entries {
            let entry = match entry.kind {
                RosterEntryKind::Segment => {
                    let label = entry
                        .label
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_string();
                    if label.is_empty() {
                        return Err(SessionDefinitionError::Invalid(
                            "A segment roster entry needs a non-empty label".to_string(),
                        ));
                    }
                    RosterEntryInput {
                        kind: RosterEntryKind::Segment,
                        ref_id: None,
                        label: Some(label),
                    }
                }
                kind @ (RosterEntryKind::QuestFamily | RosterEntryKind::Quest) => {
                    let Some(ref_id) = entry.ref_id else {
                        return Err(SessionDefinitionError::Invalid(format!(
                            "A {} roster entry needs a ref_id",
                            kind.as_str()
                        )));
                    };
                    self.require_active_target(kind, ref_id).await?;
                    RosterEntryInput {
                        kind,
                        ref_id: Some(ref_id),
                        label: None,
                    }
                }
            };
            validated.push(entry);
        }
        Ok(validated)
    }

    async fn require_active_target(
        &self,
        kind: RosterEntryKind,
        ref_id: i64,
    ) -> Result<(), SessionDefinitionError> {
        let table = match kind {
            RosterEntryKind::QuestFamily => "quest_families",
            RosterEntryKind::Quest => "quests",
            RosterEntryKind::Segment => unreachable!("segments carry no reference"),
        };
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE id = ? AND is_active = 1");
        let found = self
            .db
            .with_reader(move |conn| {
                Ok(conn.query_row(&sql, rusqlite::params![ref_id], |row| row.get::<_, i64>(0))?)
            })
            .await?;
        if found == 0 {
            return Err(SessionDefinitionError::Invalid(format!(
                "{} {ref_id} does not exist or is not active",
                match kind {
                    RosterEntryKind::QuestFamily => "quest family",
                    _ => "quest",
                }
            )));
        }
        Ok(())
    }

    fn now_epoch(&self) -> f64 {
        crate::time::instant_to_epoch(crate::time::resolve_local(self.clock.now()))
    }
}

/// The definition a session is (or would be) an instance of: the
/// configured selection while it is still active, otherwise the
/// protected default. Returns the name alongside the id because both
/// callers stamp or display the two together.
///
/// This is where "nothing chosen" stops being a hole. Settings live in
/// a JSON file, so a fresh install has no selection to read; rather
/// than backfill one, both the idle snapshot and session start resolve
/// through here, and the snapshot therefore shows exactly what a start
/// would stamp. `None` only when no protected definition exists (a
/// database built by a harness that predates the seeding migration).
pub async fn resolve_selection(
    db: &Db,
    configured: Option<i64>,
) -> Result<Option<(i64, String, bool)>, DbError> {
    db.with_reader(move |conn| {
        use rusqlite::OptionalExtension as _;
        if let Some(id) = configured {
            let selected = conn
                .query_row(
                    "SELECT id, name, track_protection_costs \
                     FROM session_definitions \
                     WHERE id = ? AND is_active = 1",
                    rusqlite::params![id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)? != 0,
                        ))
                    },
                )
                .optional()?;
            if selected.is_some() {
                return Ok(selected);
            }
        }
        // id-order: insertion (the first seeded protected definition).
        Ok(conn
            .query_row(
                "SELECT id, name, track_protection_costs \
                 FROM session_definitions \
                 WHERE is_active = 1 AND is_protected = 1 \
                 ORDER BY id ASC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? != 0,
                    ))
                },
            )
            .optional()?)
    })
    .await
}

/// One definition header row (roster filled in by the caller).
fn row_to_definition(row: &rusqlite::Row) -> Result<SessionDefinition, rusqlite::Error> {
    Ok(SessionDefinition {
        id: row.get("id")?,
        name: row.get("name")?,
        ad_hoc_segments: row.get::<_, i64>("ad_hoc_segments")? != 0,
        track_protection_costs: row.get::<_, i64>("track_protection_costs")? != 0,
        is_active: row.get::<_, i64>("is_active")? != 0,
        is_protected: row.get::<_, i64>("is_protected")? != 0,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        instance_count: row.get("instance_count")?,
        roster: Vec::new(),
    })
}

fn read_roster(
    conn: &rusqlite::Connection,
    definition_id: i64,
) -> Result<Vec<RosterEntry>, rusqlite::Error> {
    let mut stmt = conn.prepare(ROSTER_SELECT)?;
    let mut rows = stmt.query(rusqlite::params![definition_id])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let kind = RosterEntryKind::parse(&row.get::<_, String>("kind")?).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                "unknown roster entry kind".into(),
            )
        })?;
        out.push(RosterEntry {
            id: row.get("id")?,
            position: row.get("position")?,
            kind,
            ref_id: row.get("ref_id")?,
            label: row.get("label")?,
            display_name: row.get("display_name")?,
        });
    }
    Ok(out)
}

fn write_roster(
    tx: &rusqlite::Transaction<'_>,
    definition_id: i64,
    roster: &[RosterEntryInput],
) -> Result<(), rusqlite::Error> {
    for (position, entry) in roster.iter().enumerate() {
        tx.execute(
            "INSERT INTO session_definition_roster \
             (definition_id, position, kind, ref_id, label) \
             VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![
                definition_id,
                position as i64,
                entry.kind.as_str(),
                entry.ref_id,
                entry.label
            ],
        )?;
    }
    Ok(())
}

/// A definition's name, trimmed and required non-empty.
fn validated_name(name: &str) -> Result<String, SessionDefinitionError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(SessionDefinitionError::Invalid(
            "A session needs a name".to_string(),
        ));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests;
