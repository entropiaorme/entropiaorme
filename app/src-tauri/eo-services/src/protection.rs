//! Protection setup and measurement accounting.
//!
//! Armour and plates are independent economic layers. Named loadouts
//! compose them for live selection, while limited layers reconcile two
//! confirmed Trade Terminal observations. Both limited decay and unlimited
//! repair readings settle compatible defensive evidence, which may span
//! several sessions when the user postpones recording.

use std::sync::Arc;

use rusqlite::OptionalExtension;

use crate::clock::Clock;
use crate::db::{Db, DbError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionSetKind {
    Armour,
    Plates,
}

impl ProtectionSetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Armour => "armour",
            Self::Plates => "plates",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtectionError> {
        match value {
            "armour" => Ok(Self::Armour),
            "plates" => Ok(Self::Plates),
            _ => Err(ProtectionError::Stored("unknown protection set kind")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionEconomyKind {
    Limited,
    Unlimited,
}

impl ProtectionEconomyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Limited => "limited",
            Self::Unlimited => "unlimited",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtectionError> {
        match value {
            "limited" => Ok(Self::Limited),
            "unlimited" => Ok(Self::Unlimited),
            _ => Err(ProtectionError::Stored("unknown protection economy kind")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationSource {
    Ocr,
    Manual,
}

impl ObservationSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ocr => "ocr",
            Self::Manual => "manual",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtectionError> {
        match value {
            "ocr" => Ok(Self::Ocr),
            "manual" => Ok(Self::Manual),
            _ => Err(ProtectionError::Stored("unknown observation source")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconciliationStatus {
    Booked,
    Pending,
}

impl ReconciliationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Booked => "booked",
            Self::Pending => "pending",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtectionError> {
        match value {
            "booked" => Ok(Self::Booked),
            "pending" => Ok(Self::Pending),
            _ => Err(ProtectionError::Stored("unknown reconciliation status")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionSet {
    pub id: i64,
    pub kind: ProtectionSetKind,
    pub name: String,
    pub economy_kind: ProtectionEconomyKind,
    pub markup_percent: Option<f64>,
    pub created_at: f64,
    pub archived_at: Option<f64>,
    pub latest_observation: Option<ProtectionObservation>,
    pub pending_reconciliations: i64,
    pub basis_locked: bool,
    pub unsettled_damage: f64,
    pub unsettled_deflections: i64,
    pub unsettled_sessions: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionLoadout {
    pub id: i64,
    pub name: String,
    pub armour: Option<ProtectionSetRef>,
    pub plates: Option<ProtectionSetRef>,
    pub archived_at: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionSetRef {
    pub id: i64,
    pub name: String,
    pub economy_kind: ProtectionEconomyKind,
    pub markup_percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionObservation {
    pub id: i64,
    pub set_id: i64,
    pub tt_value_ped: f64,
    pub source: ObservationSource,
    pub raw_text: Option<String>,
    pub observed_at: f64,
    pub reset_reason: Option<String>,
    pub defence_event_cursor: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionCostAllocation {
    pub session_id: String,
    pub hit_count: i64,
    pub allocation_share: f64,
    pub cost_ped: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionCostWindow {
    pub id: i64,
    pub kind: String,
    pub set_id: Option<i64>,
    pub armour_set_id: Option<i64>,
    pub plate_set_id: Option<i64>,
    pub consumed_tt_ped: Option<f64>,
    pub markup_percent: Option<f64>,
    pub cost_ped: f64,
    pub cost_known: bool,
    pub status: ReconciliationStatus,
    pub reason: Option<String>,
    pub created_at: f64,
    pub allocations: Vec<ProtectionCostAllocation>,
}

const RESET_UNPRICED_REASON: &str =
    "Baseline reset left prior defensive evidence without a measurable cost";

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionReconciliation {
    pub id: i64,
    pub set_id: i64,
    pub opening_observation_id: i64,
    pub closing_observation_id: i64,
    pub consumed_tt_ped: f64,
    pub markup_percent: f64,
    pub cost_ped: f64,
    pub status: ReconciliationStatus,
    pub session_id: Option<String>,
    pub reason: Option<String>,
    pub created_at: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObservationOutcome {
    pub observation: ProtectionObservation,
    pub reconciliation: Option<ProtectionReconciliation>,
    pub cost_window: Option<ProtectionCostWindow>,
}

/// How many sessions the pending-attribution read offers at once. A
/// backlog longer than this is a catalogue, not a prompt; the oldest
/// drop off rather than turning the recording surface into a list to
/// scroll.
const PENDING_ATTRIBUTION_LIMIT: i64 = 12;

/// One session whose defence evidence is not yet attributed to a setup,
/// and so cannot be settled by any cost that names one.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingProtectionSession {
    pub session_id: String,
    /// The session's own name, else its definition's; absent for a
    /// session recorded under neither.
    pub name: Option<String>,
    pub started_at: f64,
    /// Absent while the session is still running.
    pub ended_at: Option<f64>,
    pub defence_event_count: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepairOutcome {
    pub cost_window: ProtectionCostWindow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionOverview {
    pub sets: Vec<ProtectionSet>,
    pub loadouts: Vec<ProtectionLoadout>,
    pub active_loadout_id: Option<i64>,
    pub recent_reconciliations: Vec<ProtectionReconciliation>,
    pub recent_cost_windows: Vec<ProtectionCostWindow>,
}

/// An immutable snapshot placed on a live protection interval.
#[derive(Debug, Clone, PartialEq)]
pub struct ProtectionSelection {
    pub loadout_id: i64,
    pub loadout_name: String,
    pub armour: Option<ProtectionSetRef>,
    pub plates: Option<ProtectionSetRef>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProtectionError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("stored protection data is invalid: {0}")]
    Stored(&'static str),
}

impl From<rusqlite::Error> for ProtectionError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(DbError::Sqlite(error))
    }
}

pub struct ProtectionService {
    db: Db,
    clock: Arc<dyn Clock>,
}

/// Resolve the persisted default for session-start stamping. The tracker
/// uses this read directly so configuration and the opening interval share
/// the same database truth without a second service owner.
pub async fn active_selection(db: &Db) -> Result<Option<ProtectionSelection>, DbError> {
    db.with_reader(|conn| {
        let id: Option<i64> = conn
            .query_row(
                "SELECT active_loadout_id FROM protection_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        id.map(|id| read_selection(conn, id).map_err(protection_decode))
            .transpose()
    })
    .await
}

impl ProtectionService {
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    fn now(&self) -> f64 {
        self.clock.now().and_utc().timestamp_micros() as f64 / 1_000_000.0
    }

    pub async fn overview(&self) -> Result<ProtectionOverview, ProtectionError> {
        self.db
            .with_reader(|conn| read_overview(conn))
            .await
            .map_err(ProtectionError::from)
    }

    pub async fn create_set(
        &self,
        kind: ProtectionSetKind,
        name: &str,
        economy_kind: ProtectionEconomyKind,
        markup_percent: Option<f64>,
    ) -> Result<ProtectionSet, ProtectionError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ProtectionError::Invalid("Set name is required"));
        }
        let markup = match economy_kind {
            ProtectionEconomyKind::Limited => match markup_percent {
                Some(value) if value.is_finite() && value >= 100.0 => Some(value),
                _ => {
                    return Err(ProtectionError::Invalid(
                        "Limited sets require an average markup of at least 100%",
                    ))
                }
            },
            ProtectionEconomyKind::Unlimited => None,
        };
        let name = name.to_string();
        let kind_text = kind.as_str();
        let economy_text = economy_kind.as_str();
        let now = self.now();
        let id = self
            .db
            .with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO protection_sets \
                     (kind, name, economy_kind, markup_percent, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![kind_text, name, economy_text, markup, now],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .await
            .map_err(map_constraint("An active set already uses that name"))?;
        self.set_by_id(id).await
    }

    pub async fn update_set(
        &self,
        set_id: i64,
        name: &str,
        economy_kind: ProtectionEconomyKind,
        markup_percent: Option<f64>,
    ) -> Result<ProtectionSet, ProtectionError> {
        let existing = self.set_by_id(set_id).await?;
        if existing.archived_at.is_some() {
            return Err(ProtectionError::NotFound("Protection set not found"));
        }
        let name = name.trim();
        if name.is_empty() {
            return Err(ProtectionError::Invalid("Set name is required"));
        }
        let markup = match economy_kind {
            ProtectionEconomyKind::Limited => match markup_percent {
                Some(value) if value.is_finite() && value >= 100.0 => Some(value),
                _ => {
                    return Err(ProtectionError::Invalid(
                        "Limited sets require an average markup of at least 100%",
                    ))
                }
            },
            ProtectionEconomyKind::Unlimited => None,
        };
        let basis_changed =
            existing.economy_kind != economy_kind || existing.markup_percent != markup;
        let has_recorded_use = self
            .db
            .with_reader(move |conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM session_protection_intervals \
                     WHERE armour_set_id = ?1 OR plate_set_id = ?1)",
                    [set_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(Into::into)
            })
            .await?;
        if basis_changed && (existing.latest_observation.is_some() || has_recorded_use) {
            return Err(ProtectionError::Conflict(
                "A set's economic basis cannot change after its first observation or recorded use",
            ));
        }

        let name = name.to_string();
        let economy_text = economy_kind.as_str();
        self.db
            .with_writer(move |conn| {
                conn.execute(
                    "UPDATE protection_sets SET name = ?1, economy_kind = ?2, markup_percent = ?3 \
                     WHERE id = ?4 AND archived_at IS NULL",
                    rusqlite::params![name, economy_text, markup, set_id],
                )?;
                Ok(())
            })
            .await
            .map_err(map_constraint("An active set already uses that name"))?;
        self.set_by_id(set_id).await
    }

    pub async fn create_loadout(
        &self,
        name: &str,
        armour_set_id: Option<i64>,
        plate_set_id: Option<i64>,
    ) -> Result<ProtectionLoadout, ProtectionError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ProtectionError::Invalid("Loadout name is required"));
        }
        if armour_set_id.is_none()
            && plate_set_id.is_none()
            && !name.eq_ignore_ascii_case("No protection")
        {
            return Err(ProtectionError::Invalid(
                "An empty loadout must be named No protection",
            ));
        }
        self.validate_component(armour_set_id, ProtectionSetKind::Armour)
            .await?;
        self.validate_component(plate_set_id, ProtectionSetKind::Plates)
            .await?;
        let name = name.to_string();
        let now = self.now();
        let id = self
            .db
            .with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO protection_loadouts \
                     (name, armour_set_id, plate_set_id, created_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![name, armour_set_id, plate_set_id, now],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .await
            .map_err(map_constraint("An active loadout already uses that name"))?;
        self.loadout_by_id(id).await
    }

    pub async fn update_loadout(
        &self,
        loadout_id: i64,
        name: &str,
        armour_set_id: Option<i64>,
        plate_set_id: Option<i64>,
    ) -> Result<ProtectionLoadout, ProtectionError> {
        self.loadout_by_id(loadout_id).await?;
        let name = name.trim();
        if name.is_empty() {
            return Err(ProtectionError::Invalid("Loadout name is required"));
        }
        if armour_set_id.is_none()
            && plate_set_id.is_none()
            && !name.eq_ignore_ascii_case("No protection")
        {
            return Err(ProtectionError::Invalid(
                "An empty loadout must be named No protection",
            ));
        }
        self.validate_component(armour_set_id, ProtectionSetKind::Armour)
            .await?;
        self.validate_component(plate_set_id, ProtectionSetKind::Plates)
            .await?;
        let name = name.to_string();
        self.db
            .with_writer(move |conn| {
                conn.execute(
                    "UPDATE protection_loadouts \
                     SET name = ?1, armour_set_id = ?2, plate_set_id = ?3 \
                     WHERE id = ?4 AND archived_at IS NULL",
                    rusqlite::params![name, armour_set_id, plate_set_id, loadout_id],
                )?;
                Ok(())
            })
            .await
            .map_err(map_constraint("An active loadout already uses that name"))?;
        self.loadout_by_id(loadout_id).await
    }

    pub async fn archive_set(&self, set_id: i64) -> Result<(), ProtectionError> {
        let now = self.now();
        let outcome = self
            .db
            .with_writer(move |conn| {
                let referenced: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM protection_loadouts \
                     WHERE archived_at IS NULL AND (armour_set_id = ?1 OR plate_set_id = ?1)",
                    [set_id],
                    |row| row.get(0),
                )?;
                if referenced > 0 {
                    return Ok(ArchiveOutcome::Referenced);
                }
                let changed = conn.execute(
                    "UPDATE protection_sets SET archived_at = ?1 \
                     WHERE id = ?2 AND archived_at IS NULL",
                    rusqlite::params![now, set_id],
                )?;
                Ok(if changed == 0 {
                    ArchiveOutcome::Missing
                } else {
                    ArchiveOutcome::Archived
                })
            })
            .await?;
        match outcome {
            ArchiveOutcome::Archived => Ok(()),
            ArchiveOutcome::Referenced => Err(ProtectionError::Conflict(
                "Archive loadouts using this set first",
            )),
            ArchiveOutcome::Missing => Err(ProtectionError::NotFound("Protection set not found")),
        }
    }

    pub async fn archive_loadout(&self, loadout_id: i64) -> Result<(), ProtectionError> {
        let now = self.now();
        let changed = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let changed = tx.execute(
                    "UPDATE protection_loadouts SET archived_at = ?1 \
                     WHERE id = ?2 AND archived_at IS NULL",
                    rusqlite::params![now, loadout_id],
                )?;
                tx.execute(
                    "UPDATE protection_state SET active_loadout_id = NULL, updated_at = ?1 \
                     WHERE singleton = 1 AND active_loadout_id = ?2",
                    rusqlite::params![now, loadout_id],
                )?;
                tx.commit()?;
                Ok(changed)
            })
            .await?;
        if changed == 0 {
            return Err(ProtectionError::NotFound("Protection loadout not found"));
        }
        Ok(())
    }

    pub async fn persist_active_loadout(&self, loadout_id: i64) -> Result<(), ProtectionError> {
        self.loadout_by_id(loadout_id).await?;
        let now = self.now();
        self.db
            .with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO protection_state(singleton, active_loadout_id, updated_at) \
                     VALUES (1, ?1, ?2) \
                     ON CONFLICT(singleton) DO UPDATE SET \
                     active_loadout_id = excluded.active_loadout_id, updated_at = excluded.updated_at",
                    rusqlite::params![loadout_id, now],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    pub async fn active_selection(&self) -> Result<Option<ProtectionSelection>, ProtectionError> {
        self.db
            .with_reader(|conn| {
                let id: Option<i64> = conn
                    .query_row(
                        "SELECT active_loadout_id FROM protection_state WHERE singleton = 1",
                        [],
                        |row| row.get(0),
                    )
                    .optional()?
                    .flatten();
                id.map(|id| read_selection(conn, id).map_err(protection_decode))
                    .transpose()
            })
            .await
            .map_err(ProtectionError::from)
    }

    pub async fn selection(&self, loadout_id: i64) -> Result<ProtectionSelection, ProtectionError> {
        self.db
            .with_reader(move |conn| read_selection(conn, loadout_id).map_err(protection_decode))
            .await
            .map_err(ProtectionError::from)
    }

    /// The sessions still waiting to be told which setup was worn.
    ///
    /// A session that opted out of per-segment attribution carries its
    /// defence evidence unattributed until a setup is named for it, and
    /// unattributed evidence cannot be settled by any cost that names an
    /// armour or plate set: the layer filter has nothing to match. That
    /// is what leaves a postponed session stranded, and it is invisible
    /// unless the recording surface says so, hence this read.
    ///
    /// The running session appears here like any other, because naming
    /// its setup is no longer something that waits for it to end.
    pub async fn pending_attribution(
        &self,
    ) -> Result<Vec<PendingProtectionSession>, ProtectionError> {
        self.db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT s.id, COALESCE(s.session_name, d.name), s.started_at, s.ended_at, \
                            COUNT(e.id) \
                     FROM tracking_sessions s \
                     LEFT JOIN session_definitions d ON d.id = s.definition_id \
                     JOIN protection_defence_events e ON e.session_id = s.id \
                     WHERE s.track_protection_costs != 0 \
                       AND s.track_protection_by_segment = 0 \
                       AND e.protection_interval_id IS NULL \
                       AND NOT EXISTS (SELECT 1 FROM protection_cost_evidence ce \
                                       WHERE ce.defence_event_id = e.id) \
                     GROUP BY s.id \
                     ORDER BY s.started_at DESC \
                     LIMIT ?1",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![PENDING_ATTRIBUTION_LIMIT], |row| {
                        Ok(PendingProtectionSession {
                            session_id: row.get(0)?,
                            name: row.get(1)?,
                            started_at: row.get(2)?,
                            ended_at: row.get(3)?,
                            defence_event_count: row.get(4)?,
                        })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .await
            .map_err(ProtectionError::from)
    }

    /// Attach one whole-session protection setup after an opted-out session
    /// ends. Raw event contexts remain untouched, but allocation deliberately
    /// collapses them to session grain through the stamped session policy.
    pub async fn assign_session_loadout(
        &self,
        session_id: &str,
        loadout_id: i64,
    ) -> Result<ProtectionSelection, ProtectionError> {
        let selection = self.selection(loadout_id).await?;
        let session_id = session_id.to_string();
        let stored = selection.clone();
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let session = tx
                    .query_row(
                        "SELECT started_at, ended_at, track_protection_costs, \
                                track_protection_by_segment \
                         FROM tracking_sessions WHERE id = ?1",
                        [&session_id],
                        |row| {
                            Ok((
                                row.get::<_, f64>(0)?,
                                row.get::<_, Option<f64>>(1)?,
                                row.get::<_, i64>(2)? != 0,
                                row.get::<_, i64>(3)? != 0,
                            ))
                        },
                    )
                    .optional()?;
                let Some((started_at, ended_at, tracks_costs, tracks_segments)) = session else {
                    return Ok(false);
                };
                if !tracks_costs || tracks_segments || ended_at.is_none() {
                    return Ok(false);
                }

                let claimed: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM protection_cost_evidence e \
                     JOIN protection_defence_events d ON d.id = e.defence_event_id \
                     WHERE d.session_id = ?1)",
                    [&session_id],
                    |row| row.get(0),
                )?;

                // id-order: insertion (the session's first protection
                // interval is the one to reuse).
                let existing: Option<(i64, i64)> = tx
                    .query_row(
                        "SELECT i.id, p.loadout_id FROM session_intervals i \
                         JOIN session_protection_intervals p ON p.interval_id = i.id \
                         WHERE i.session_id = ?1 AND i.kind = 'protection' \
                         ORDER BY i.id LIMIT 1",
                        [&session_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                if let Some((interval_id, existing_loadout)) = existing {
                    if existing_loadout == stored.loadout_id {
                        tx.commit()?;
                        return Ok(true);
                    }
                    if claimed {
                        return Ok(false);
                    }
                    tx.execute(
                        "UPDATE session_intervals SET label = ?1, ref_id = ?2 WHERE id = ?3",
                        rusqlite::params![stored.loadout_name, stored.loadout_id, interval_id],
                    )?;
                    tx.execute(
                        "UPDATE session_protection_intervals SET \
                         loadout_id = ?1, loadout_name = ?2, \
                         armour_set_id = ?3, armour_set_name = ?4, \
                         armour_economy_kind = ?5, armour_markup_percent = ?6, \
                         plate_set_id = ?7, plate_set_name = ?8, \
                         plate_economy_kind = ?9, plate_markup_percent = ?10 \
                         WHERE interval_id = ?11",
                        rusqlite::params![
                            stored.loadout_id,
                            stored.loadout_name,
                            stored.armour.as_ref().map(|set| set.id),
                            stored.armour.as_ref().map(|set| set.name.as_str()),
                            stored.armour.as_ref().map(|set| set.economy_kind.as_str()),
                            stored.armour.as_ref().and_then(|set| set.markup_percent),
                            stored.plates.as_ref().map(|set| set.id),
                            stored.plates.as_ref().map(|set| set.name.as_str()),
                            stored.plates.as_ref().map(|set| set.economy_kind.as_str()),
                            stored.plates.as_ref().and_then(|set| set.markup_percent),
                            interval_id,
                        ],
                    )?;
                    tx.commit()?;
                    return Ok(true);
                }
                // Settled evidence is no longer a blanket refusal: the
                // back-stamp below takes only what no cost has claimed, so
                // naming a setup can no longer move what one bought. What
                // must still hold is that there is something left to
                // attribute, or the declaration would record an answer
                // about no evidence at all.
                let unsettled: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM protection_defence_events d \
                     WHERE d.session_id = ?1 \
                       AND NOT EXISTS (SELECT 1 FROM protection_cost_evidence ce \
                                       WHERE ce.defence_event_id = d.id))",
                    [&session_id],
                    |row| row.get(0),
                )?;
                if !unsettled {
                    return Ok(false);
                }

                tx.execute(
                    "INSERT INTO session_intervals \
                     (session_id, kind, label, ref_id, started_at, ended_at) \
                     VALUES (?1, 'protection', ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        session_id,
                        stored.loadout_name,
                        stored.loadout_id,
                        started_at,
                        ended_at,
                    ],
                )?;
                let interval_id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO session_protection_intervals \
                     (interval_id, loadout_id, loadout_name, armour_set_id, armour_set_name, \
                      armour_economy_kind, armour_markup_percent, plate_set_id, plate_set_name, \
                      plate_economy_kind, plate_markup_percent) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    rusqlite::params![
                        interval_id,
                        stored.loadout_id,
                        stored.loadout_name,
                        stored.armour.as_ref().map(|set| set.id),
                        stored.armour.as_ref().map(|set| set.name.as_str()),
                        stored.armour.as_ref().map(|set| set.economy_kind.as_str()),
                        stored.armour.as_ref().and_then(|set| set.markup_percent),
                        stored.plates.as_ref().map(|set| set.id),
                        stored.plates.as_ref().map(|set| set.name.as_str()),
                        stored.plates.as_ref().map(|set| set.economy_kind.as_str()),
                        stored.plates.as_ref().and_then(|set| set.markup_percent),
                    ],
                )?;
                // Only evidence no settled cost has claimed. A window that
                // already paid for a hit named the setup standing then, and
                // re-pointing it would silently move what that cost bought.
                tx.execute(
                    "UPDATE protection_defence_events SET protection_interval_id = ?1 \
                     WHERE session_id = ?2 \
                       AND NOT EXISTS (SELECT 1 FROM protection_cost_evidence ce \
                                       WHERE ce.defence_event_id = protection_defence_events.id)",
                    rusqlite::params![interval_id, session_id],
                )?;
                tx.commit()?;
                Ok(true)
            })
            .await?
            .then_some(())
            .ok_or(ProtectionError::Conflict(
                "That session cannot take a whole-session armour setup: it needs armour costs \
                 enabled, per-segment attribution off, and defence evidence no recorded cost has \
                 settled yet",
            ))?;
        Ok(selection)
    }

    pub async fn confirm_observation(
        &self,
        set_id: i64,
        client_token: &str,
        tt_value_ped: f64,
        source: ObservationSource,
        raw_text: Option<&str>,
        reset_reason: Option<&str>,
    ) -> Result<ObservationOutcome, ProtectionError> {
        if client_token.trim().is_empty() {
            return Err(ProtectionError::Invalid("Observation token is required"));
        }
        if !tt_value_ped.is_finite() || tt_value_ped < 0.0 {
            return Err(ProtectionError::Invalid("TT value must be zero or greater"));
        }
        let set = self.set_by_id(set_id).await?;
        if set.economy_kind != ProtectionEconomyKind::Limited {
            return Err(ProtectionError::Invalid(
                "TT observations are only used for limited protection",
            ));
        }
        let now = self.now();
        let token = client_token.trim().to_string();
        let source_text = source.as_str();
        let raw_text = raw_text
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        let reset_reason = reset_reason
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .map(str::to_string);
        let markup = set.markup_percent.expect("limited set carries markup");

        let outcome = self
            .db
            .with_writer(move |conn| {
                if let Some(existing_id) = conn
                    .query_row(
                        "SELECT id FROM protection_observations WHERE client_token = ?1",
                        [&token],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?
                {
                    return read_observation_outcome(conn, existing_id)
                        .map(Box::new)
                        .map(ObservationWriteOutcome::Saved);
                }

                let previous = conn
                    .query_row(
                        "SELECT id, tt_value_ped, observed_at, COALESCE(defence_event_cursor, 0) \
                         FROM protection_observations \
                         WHERE set_id = ?1 ORDER BY observed_at DESC, id DESC LIMIT 1",
                        [set_id],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, f64>(1)?,
                                row.get::<_, f64>(2)?,
                                row.get::<_, i64>(3)?,
                            ))
                        },
                    )
                    .optional()?;

                if reset_reason.is_none()
                    && previous.is_some_and(|(_, value, _, _)| tt_value_ped > value + 0.000_000_1)
                {
                    return Ok(ObservationWriteOutcome::Increased);
                }

                let tx = conn.transaction()?;
                // id-order: cursor (the observation closes the defence-event
                // stream at the highest id seen so far).
                let closing_cursor: i64 = tx.query_row(
                    "SELECT COALESCE(MAX(id), 0) FROM protection_defence_events",
                    [],
                    |row| row.get(0),
                )?;
                tx.execute(
                    "INSERT INTO protection_observations \
                     (set_id, client_token, tt_value_ped, source, raw_text, observed_at, \
                      reset_reason, defence_event_cursor) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        set_id,
                        token,
                        tt_value_ped,
                        source_text,
                        raw_text,
                        now,
                        reset_reason.as_deref(),
                        closing_cursor
                    ],
                )?;
                let observation_id = tx.last_insert_rowid();

                if reset_reason.is_none() {
                    if let Some((opening_id, opening_value, _opening_at, opening_cursor)) = previous
                    {
                        let consumed_tt = (opening_value - tt_value_ped).max(0.0);
                        let cost = consumed_tt * markup / 100.0;
                        create_cost_window(
                            &tx,
                            CostWindowSpec {
                                kind: "limited_decay",
                                set_id: Some(set_id),
                                armour_set_id: None,
                                plate_set_id: None,
                                opening_observation_id: Some(opening_id),
                                closing_observation_id: Some(observation_id),
                                opening_cursor,
                                closing_cursor: Some(closing_cursor),
                                consumed_tt_ped: Some(consumed_tt),
                                markup_percent: Some(markup),
                                cost_ped: cost,
                                client_token: None,
                                created_at: now,
                            },
                        )?;
                    }
                } else if let Some((opening_id, _opening_value, _opening_at, opening_cursor)) =
                    previous
                {
                    create_unpriced_reset_window(
                        &tx,
                        CostWindowSpec {
                            kind: "limited_decay",
                            set_id: Some(set_id),
                            armour_set_id: None,
                            plate_set_id: None,
                            opening_observation_id: Some(opening_id),
                            closing_observation_id: Some(observation_id),
                            opening_cursor,
                            closing_cursor: Some(closing_cursor),
                            consumed_tt_ped: Some(0.0),
                            markup_percent: Some(markup),
                            cost_ped: 0.0,
                            client_token: None,
                            created_at: now,
                        },
                    )?;
                }
                tx.commit()?;
                read_observation_outcome(conn, observation_id)
                    .map(Box::new)
                    .map(ObservationWriteOutcome::Saved)
            })
            .await?;
        match outcome {
            ObservationWriteOutcome::Saved(outcome) => Ok(*outcome),
            ObservationWriteOutcome::Increased => Err(ProtectionError::Conflict(
                "TT value increased; reset the baseline instead",
            )),
        }
    }

    pub async fn confirm_repair_cost(
        &self,
        client_token: &str,
        armour_set_id: Option<i64>,
        plate_set_id: Option<i64>,
        cost_ped: f64,
    ) -> Result<RepairOutcome, ProtectionError> {
        if client_token.trim().is_empty() {
            return Err(ProtectionError::Invalid("Repair token is required"));
        }
        if !cost_ped.is_finite() || cost_ped < 0.0 {
            return Err(ProtectionError::Invalid(
                "Repair cost must be zero or greater",
            ));
        }
        for (set_id, expected) in [
            (armour_set_id, ProtectionSetKind::Armour),
            (plate_set_id, ProtectionSetKind::Plates),
        ] {
            if let Some(set_id) = set_id {
                let set = self.set_by_id(set_id).await?;
                if set.kind != expected || set.economy_kind != ProtectionEconomyKind::Unlimited {
                    return Err(ProtectionError::Invalid(
                        "Repair costs require matching unlimited protection sets",
                    ));
                }
            }
        }
        let token = format!("repair:{}", client_token.trim());
        let now = self.now();
        let id = self.db.with_writer(move |conn| {
            if let Some(id) = conn.query_row(
                "SELECT id FROM protection_cost_windows WHERE client_token = ?1 AND kind = 'repair'",
                [&token],
                |row| row.get::<_, i64>(0),
            ).optional()? {
                return Ok(id);
            }
            let tx = conn.transaction()?;
            let id = create_cost_window(
                &tx,
                CostWindowSpec {
                    kind: "repair",
                    set_id: None,
                    armour_set_id,
                    plate_set_id,
                    opening_observation_id: None,
                    closing_observation_id: None,
                    opening_cursor: 0,
                    closing_cursor: None,
                    consumed_tt_ped: None,
                    markup_percent: None,
                    cost_ped,
                    client_token: Some(token.clone()),
                    created_at: now,
                },
            )?;
            tx.commit()?;
            Ok(id)
        }).await?;
        let cost_window = self
            .db
            .with_reader(move |conn| read_cost_window(conn, id).map_err(protection_decode))
            .await?;
        Ok(RepairOutcome { cost_window })
    }

    async fn validate_component(
        &self,
        set_id: Option<i64>,
        expected: ProtectionSetKind,
    ) -> Result<(), ProtectionError> {
        let Some(set_id) = set_id else {
            return Ok(());
        };
        let set = self.set_by_id(set_id).await?;
        if set.archived_at.is_some() || set.kind != expected {
            return Err(ProtectionError::Invalid(
                "Loadout component has the wrong kind or is archived",
            ));
        }
        Ok(())
    }

    async fn set_by_id(&self, id: i64) -> Result<ProtectionSet, ProtectionError> {
        let row = self
            .db
            .with_reader(move |conn| read_set(conn, id).map_err(protection_decode))
            .await?;
        row.ok_or(ProtectionError::NotFound("Protection set not found"))
    }

    async fn loadout_by_id(&self, id: i64) -> Result<ProtectionLoadout, ProtectionError> {
        let row = self
            .db
            .with_reader(move |conn| read_loadout(conn, id).map_err(protection_decode))
            .await?;
        row.filter(|loadout| loadout.archived_at.is_none())
            .ok_or(ProtectionError::NotFound("Protection loadout not found"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveOutcome {
    Archived,
    Referenced,
    Missing,
}

enum ObservationWriteOutcome {
    Saved(Box<ObservationOutcome>),
    Increased,
}

fn map_constraint(message: &'static str) -> impl FnOnce(DbError) -> ProtectionError {
    move |error| match error {
        DbError::Sqlite(rusqlite::Error::SqliteFailure(_, _)) => ProtectionError::Conflict(message),
        other => ProtectionError::Db(other),
    }
}

struct CostWindowSpec {
    kind: &'static str,
    set_id: Option<i64>,
    armour_set_id: Option<i64>,
    plate_set_id: Option<i64>,
    opening_observation_id: Option<i64>,
    closing_observation_id: Option<i64>,
    opening_cursor: i64,
    closing_cursor: Option<i64>,
    consumed_tt_ped: Option<f64>,
    markup_percent: Option<f64>,
    cost_ped: f64,
    client_token: Option<String>,
    created_at: f64,
}

#[derive(Debug)]
struct EvidenceRow {
    event_id: i64,
    session_id: String,
    context_id: Option<i64>,
    damage: Option<f64>,
    deflected: bool,
    armour_set_id: Option<i64>,
    plate_set_id: Option<i64>,
}

type ContextEvidence = (Option<i64>, f64, i64, i64);

fn create_cost_window(
    tx: &rusqlite::Transaction<'_>,
    spec: CostWindowSpec,
) -> Result<i64, DbError> {
    let candidates = read_eligible_evidence(tx, &spec)?;
    let status = if candidates.is_empty() {
        "pending"
    } else {
        "booked"
    };
    let reason = candidates
        .is_empty()
        .then_some("No unsettled defensive evidence matches this cost window");
    tx.execute(
        "INSERT INTO protection_cost_windows \
         (kind, set_id, armour_set_id, plate_set_id, opening_observation_id, \
          closing_observation_id, consumed_tt_ped, markup_percent, cost_ped, status, \
          reason, client_token, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            spec.kind,
            spec.set_id,
            spec.armour_set_id,
            spec.plate_set_id,
            spec.opening_observation_id,
            spec.closing_observation_id,
            spec.consumed_tt_ped,
            spec.markup_percent,
            spec.cost_ped,
            status,
            reason,
            spec.client_token,
            spec.created_at,
        ],
    )?;
    let window_id = tx.last_insert_rowid();
    if candidates.is_empty() {
        return Ok(window_id);
    }

    use std::collections::BTreeMap;
    let mut contexts: BTreeMap<(String, i64), ContextEvidence> = BTreeMap::new();
    for event in &candidates {
        let context_key = event.context_id.unwrap_or(-1);
        let entry = contexts
            .entry((event.session_id.clone(), context_key))
            .or_insert((event.context_id, 0.0, 0, 0));
        entry.1 += event.damage.unwrap_or(0.0);
        entry.2 += i64::from(event.deflected);
        entry.3 += 1;

        if let Some(set_id) = spec.set_id {
            tx.execute(
                "INSERT OR IGNORE INTO protection_cost_evidence \
                 (window_id, set_id, defence_event_id) VALUES (?1, ?2, ?3)",
                rusqlite::params![window_id, set_id, event.event_id],
            )?;
        } else if spec.armour_set_id.is_none() && spec.plate_set_id.is_none() {
            tx.execute(
                "INSERT INTO protection_cost_evidence \
                 (window_id, set_id, defence_event_id) VALUES (?1, NULL, ?2)",
                rusqlite::params![window_id, event.event_id],
            )?;
        } else {
            for set_id in [
                spec.armour_set_id
                    .filter(|id| event.armour_set_id == Some(*id)),
                spec.plate_set_id
                    .filter(|id| event.plate_set_id == Some(*id)),
            ]
            .into_iter()
            .flatten()
            {
                tx.execute(
                    "INSERT OR IGNORE INTO protection_cost_evidence \
                     (window_id, set_id, defence_event_id) VALUES (?1, ?2, ?3)",
                    rusqlite::params![window_id, set_id, event.event_id],
                )?;
            }
        }
    }

    let total_hits: i64 = contexts.values().map(|(_, _, _, hits)| hits).sum();
    let context_count = contexts.len();
    let mut allocated = 0.0;
    let mut sessions: BTreeMap<String, (f64, i64, i64, f64, f64)> = BTreeMap::new();
    for (index, ((session_id, context_key), (context_id, damage, deflections, hits))) in
        contexts.into_iter().enumerate()
    {
        let share = if total_hits > 0 {
            hits as f64 / total_hits as f64
        } else {
            1.0 / context_count as f64
        };
        let allocation = if index + 1 == context_count {
            (spec.cost_ped - allocated).max(0.0)
        } else {
            spec.cost_ped * share
        };
        allocated += allocation;
        tx.execute(
            "INSERT INTO protection_cost_context_allocations \
             (window_id, session_id, context_key, context_id, damage_weight, deflection_count, \
              allocation_share, cost_ped, hit_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                window_id,
                session_id,
                context_key,
                context_id,
                damage,
                deflections,
                share,
                allocation,
                hits,
            ],
        )?;
        let session = sessions.entry(session_id).or_default();
        session.0 += damage;
        session.1 += deflections;
        session.2 += hits;
        session.3 += share;
        session.4 += allocation;
    }
    for (session_id, (damage, deflections, hits, share, allocation)) in sessions {
        tx.execute(
            "INSERT INTO protection_cost_allocations \
             (window_id, session_id, damage_weight, deflection_count, allocation_share, cost_ped, hit_count) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                window_id,
                session_id,
                damage,
                deflections,
                share,
                allocation,
                hits,
            ],
        )?;
        let started_at: f64 = tx.query_row(
            "SELECT started_at FROM tracking_sessions WHERE id = ?1",
            [&session_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE tracking_sessions SET armour_cost = COALESCE(armour_cost, 0) + ?1 \
             WHERE id = ?2",
            rusqlite::params![allocation, session_id],
        )?;
        crate::daily_rollup::refresh_days(tx, [crate::daily_rollup::epoch_day(started_at)])?;
        crate::session_summary::write_session_summary(tx, &session_id)?;
    }
    Ok(window_id)
}

fn create_unpriced_reset_window(
    tx: &rusqlite::Transaction<'_>,
    spec: CostWindowSpec,
) -> Result<Option<i64>, DbError> {
    let candidates = read_eligible_evidence(tx, &spec)?;
    if candidates.is_empty() {
        return Ok(None);
    }

    tx.execute(
        "INSERT INTO protection_cost_windows \
         (kind, set_id, opening_observation_id, closing_observation_id, consumed_tt_ped, \
          markup_percent, cost_ped, status, reason, created_at) \
         VALUES ('limited_decay', ?1, ?2, ?3, 0, ?4, 0, 'pending', ?5, ?6)",
        rusqlite::params![
            spec.set_id,
            spec.opening_observation_id,
            spec.closing_observation_id,
            spec.markup_percent,
            RESET_UNPRICED_REASON,
            spec.created_at,
        ],
    )?;
    let window_id = tx.last_insert_rowid();
    for event in candidates {
        tx.execute(
            "INSERT INTO protection_cost_evidence \
             (window_id, set_id, defence_event_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![window_id, spec.set_id, event.event_id],
        )?;
    }
    Ok(Some(window_id))
}

fn read_eligible_evidence(
    tx: &rusqlite::Transaction<'_>,
    spec: &CostWindowSpec,
) -> Result<Vec<EvidenceRow>, DbError> {
    let layer_filter = if let Some(set_id) = spec.set_id {
        let kind: String = tx.query_row(
            "SELECT kind FROM protection_sets WHERE id = ?1",
            [set_id],
            |row| row.get(0),
        )?;
        match kind.as_str() {
            "armour" => format!("sp.armour_set_id = {set_id}"),
            "plates" => format!("sp.plate_set_id = {set_id}"),
            _ => "0".to_string(),
        }
    } else {
        match (spec.armour_set_id, spec.plate_set_id) {
            (Some(armour), Some(plates)) => {
                format!("(sp.armour_set_id = {armour} OR sp.plate_set_id = {plates})")
            }
            (Some(armour), None) => format!("sp.armour_set_id = {armour}"),
            (None, Some(plates)) => format!("sp.plate_set_id = {plates}"),
            (None, None) => "1".to_string(),
        }
    };
    let claim_filter =
        if spec.set_id.is_none() && spec.armour_set_id.is_none() && spec.plate_set_id.is_none() {
            "NOT EXISTS (SELECT 1 FROM protection_cost_evidence ce \
         WHERE ce.defence_event_id = d.id)"
                .to_string()
        } else {
            let ids = [spec.set_id, spec.armour_set_id, spec.plate_set_id]
                .into_iter()
                .flatten()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "NOT EXISTS (SELECT 1 FROM protection_cost_evidence ce \
             WHERE ce.defence_event_id = d.id AND (ce.set_id IS NULL OR ce.set_id IN ({ids})))"
            )
        };
    let closing = spec.closing_cursor.unwrap_or(i64::MAX);
    // id-order: cursor (the window is bounded by recorded stream positions).
    let sql = format!(
        "SELECT d.id, d.session_id, \
                CASE WHEN s.track_protection_by_segment != 0 THEN d.context_id END, \
                d.damage, d.deflected, \
                sp.armour_set_id, sp.plate_set_id \
         FROM protection_defence_events d \
         LEFT JOIN tracking_sessions s ON s.id = d.session_id \
         LEFT JOIN session_protection_intervals sp ON sp.interval_id = d.protection_interval_id \
         WHERE d.id > ?1 AND d.id <= ?2 AND ({layer_filter}) AND {claim_filter} \
         ORDER BY d.id"
    );
    let mut stmt = tx.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![spec.opening_cursor, closing], |row| {
        Ok(EvidenceRow {
            event_id: row.get(0)?,
            session_id: row.get(1)?,
            context_id: row.get(2)?,
            damage: row.get(3)?,
            deflected: row.get::<_, i64>(4)? != 0,
            armour_set_id: row.get(5)?,
            plate_set_id: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn read_set(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<Option<ProtectionSet>, ProtectionError> {
    let row = conn
        .query_row(
            "SELECT id, kind, name, economy_kind, markup_percent, created_at, archived_at \
             FROM protection_sets WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, Option<f64>>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((id, kind, name, economy, markup, created_at, archived_at)) = row else {
        return Ok(None);
    };
    let latest_observation = read_latest_observation(conn, id)?;
    let pending_reconciliations = conn.query_row(
        "SELECT COUNT(*) FROM protection_cost_windows \
         WHERE (set_id = ?1 OR armour_set_id = ?1 OR plate_set_id = ?1) \
           AND status = 'pending'",
        [id],
        |row| row.get(0),
    )?;
    let (unsettled_damage, unsettled_deflections, unsettled_sessions) =
        read_unsettled_evidence(conn, id, ProtectionSetKind::parse(&kind)?)?;
    let basis_locked = latest_observation.is_some()
        || conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_protection_intervals \
             WHERE armour_set_id = ?1 OR plate_set_id = ?1)",
            [id],
            |row| row.get::<_, bool>(0),
        )?;
    Ok(Some(ProtectionSet {
        id,
        kind: ProtectionSetKind::parse(&kind)?,
        name,
        economy_kind: ProtectionEconomyKind::parse(&economy)?,
        markup_percent: markup,
        created_at,
        archived_at,
        latest_observation,
        pending_reconciliations,
        basis_locked,
        unsettled_damage,
        unsettled_deflections,
        unsettled_sessions,
    }))
}

fn read_loadout(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<Option<ProtectionLoadout>, ProtectionError> {
    let row = conn
        .query_row(
            "SELECT id, name, armour_set_id, plate_set_id, archived_at \
             FROM protection_loadouts WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                ))
            },
        )
        .optional()?;
    row.map(|(id, name, armour, plates, archived_at)| {
        Ok(ProtectionLoadout {
            id,
            name,
            armour: armour.map(|id| read_set_ref(conn, id)).transpose()?,
            plates: plates.map(|id| read_set_ref(conn, id)).transpose()?,
            archived_at,
        })
    })
    .transpose()
}

fn read_set_ref(conn: &rusqlite::Connection, id: i64) -> Result<ProtectionSetRef, ProtectionError> {
    let row = conn
        .query_row(
            "SELECT id, name, economy_kind, markup_percent FROM protection_sets WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<f64>>(3)?,
                ))
            },
        )
        .optional()?;
    let (id, name, economy, markup) = row.ok_or(ProtectionError::Stored("missing loadout set"))?;
    Ok(ProtectionSetRef {
        id,
        name,
        economy_kind: ProtectionEconomyKind::parse(&economy)?,
        markup_percent: markup,
    })
}

fn read_selection(
    conn: &rusqlite::Connection,
    loadout_id: i64,
) -> Result<ProtectionSelection, ProtectionError> {
    let loadout = read_loadout(conn, loadout_id)?
        .filter(|loadout| loadout.archived_at.is_none())
        .ok_or(ProtectionError::NotFound("Protection loadout not found"))?;
    Ok(ProtectionSelection {
        loadout_id: loadout.id,
        loadout_name: loadout.name,
        armour: loadout.armour,
        plates: loadout.plates,
    })
}

fn read_observation(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<ProtectionObservation, ProtectionError> {
    let row = conn
        .query_row(
            "SELECT id, set_id, tt_value_ped, source, raw_text, observed_at, reset_reason, \
                    COALESCE(defence_event_cursor, 0) \
             FROM protection_observations WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()?
        .ok_or(ProtectionError::Stored("missing protection observation"))?;
    Ok(ProtectionObservation {
        id: row.0,
        set_id: row.1,
        tt_value_ped: row.2,
        source: ObservationSource::parse(&row.3)?,
        raw_text: row.4,
        observed_at: row.5,
        reset_reason: row.6,
        defence_event_cursor: row.7,
    })
}

fn read_latest_observation(
    conn: &rusqlite::Connection,
    set_id: i64,
) -> Result<Option<ProtectionObservation>, ProtectionError> {
    let id = conn
        .query_row(
            "SELECT id FROM protection_observations WHERE set_id = ?1 \
             ORDER BY observed_at DESC, id DESC LIMIT 1",
            [set_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    id.map(|id| read_observation(conn, id)).transpose()
}

fn read_unsettled_evidence(
    conn: &rusqlite::Connection,
    set_id: i64,
    kind: ProtectionSetKind,
) -> Result<(f64, i64, i64), ProtectionError> {
    let economy: String = conn.query_row(
        "SELECT economy_kind FROM protection_sets WHERE id = ?1",
        [set_id],
        |row| row.get(0),
    )?;
    let cursor = if economy == "limited" {
        // The first baseline is the evidence horizon. Later resets start new
        // measurement windows, but must not make older unclaimed defence
        // disappear from the outstanding-evidence readout.
        conn.query_row(
            "SELECT COALESCE(MIN(defence_event_cursor), 0) \
             FROM protection_observations WHERE set_id = ?1",
            [set_id],
            |row| row.get::<_, i64>(0),
        )?
    } else {
        0
    };
    let column = match kind {
        ProtectionSetKind::Armour => "armour_set_id",
        ProtectionSetKind::Plates => "plate_set_id",
    };
    // id-order: cursor (events past the set's recorded stream position).
    let sql = format!(
        "SELECT COALESCE(SUM(COALESCE(d.damage, 0)), 0), \
                COALESCE(SUM(d.deflected), 0), COUNT(DISTINCT d.session_id) \
         FROM protection_defence_events d \
         JOIN session_protection_intervals sp ON sp.interval_id = d.protection_interval_id \
         WHERE sp.{column} = ?1 AND d.id > ?2 \
           AND NOT EXISTS (SELECT 1 FROM protection_cost_evidence ce \
               WHERE ce.defence_event_id = d.id \
                 AND (ce.set_id IS NULL OR ce.set_id = ?1))"
    );
    conn.query_row(&sql, rusqlite::params![set_id, cursor], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })
    .map_err(Into::into)
}

fn read_cost_window(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<ProtectionCostWindow, ProtectionError> {
    let row = conn
        .query_row(
            "SELECT id, kind, set_id, armour_set_id, plate_set_id, consumed_tt_ped, \
                    markup_percent, cost_ped, status, reason, created_at \
             FROM protection_cost_windows WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                    row.get::<_, Option<f64>>(6)?,
                    row.get::<_, f64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, f64>(10)?,
                ))
            },
        )
        .optional()?
        .ok_or(ProtectionError::Stored("missing protection cost window"))?;
    let mut stmt = conn.prepare(
        "SELECT session_id, hit_count, allocation_share, cost_ped \
         FROM protection_cost_allocations WHERE window_id = ?1 ORDER BY session_id",
    )?;
    let allocations = stmt
        .query_map([id], |allocation| {
            Ok(ProtectionCostAllocation {
                session_id: allocation.get(0)?,
                hit_count: allocation.get(1)?,
                allocation_share: allocation.get(2)?,
                cost_ped: allocation.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let cost_known = row.9.as_deref() != Some(RESET_UNPRICED_REASON);
    Ok(ProtectionCostWindow {
        id: row.0,
        kind: row.1,
        set_id: row.2,
        armour_set_id: row.3,
        plate_set_id: row.4,
        consumed_tt_ped: row.5,
        markup_percent: row.6,
        cost_ped: row.7,
        cost_known,
        status: ReconciliationStatus::parse(&row.8)?,
        reason: row.9,
        created_at: row.10,
        allocations,
    })
}

fn read_reconciliation(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<ProtectionReconciliation, ProtectionError> {
    let row = conn
        .query_row(
            "SELECT id, set_id, opening_observation_id, closing_observation_id, \
                    consumed_tt_ped, markup_percent, cost_ped, status, session_id, reason, created_at \
             FROM protection_reconciliations WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, f64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, f64>(10)?,
                ))
            },
        )
        .optional()?
        .ok_or(ProtectionError::Stored("missing protection reconciliation"))?;
    Ok(ProtectionReconciliation {
        id: row.0,
        set_id: row.1,
        opening_observation_id: row.2,
        closing_observation_id: row.3,
        consumed_tt_ped: row.4,
        markup_percent: row.5,
        cost_ped: row.6,
        status: ReconciliationStatus::parse(&row.7)?,
        session_id: row.8,
        reason: row.9,
        created_at: row.10,
    })
}

fn read_observation_outcome(
    conn: &rusqlite::Connection,
    observation_id: i64,
) -> Result<ObservationOutcome, DbError> {
    let observation = read_observation(conn, observation_id).map_err(protection_decode)?;
    let reconciliation_id = conn
        .query_row(
            "SELECT id FROM protection_reconciliations WHERE closing_observation_id = ?1",
            [observation_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let reconciliation = reconciliation_id
        .map(|id| read_reconciliation(conn, id).map_err(protection_decode))
        .transpose()?;
    let cost_window_id = conn
        .query_row(
            "SELECT id FROM protection_cost_windows WHERE closing_observation_id = ?1",
            [observation_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let cost_window = cost_window_id
        .map(|id| read_cost_window(conn, id).map_err(protection_decode))
        .transpose()?;
    Ok(ObservationOutcome {
        observation,
        reconciliation,
        cost_window,
    })
}

fn protection_decode(error: ProtectionError) -> DbError {
    match error {
        ProtectionError::Db(error) => error,
        other => DbError::Decode {
            context: "stored protection record",
            source: serde_json::Error::io(std::io::Error::other(other.to_string())),
        },
    }
}

fn read_overview(conn: &rusqlite::Connection) -> Result<ProtectionOverview, DbError> {
    let mut set_stmt = conn.prepare(
        "SELECT id FROM protection_sets WHERE archived_at IS NULL ORDER BY kind, lower(name), id",
    )?;
    let set_ids = set_stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let sets = set_ids
        .into_iter()
        .map(|id| {
            read_set(conn, id)
                .map_err(protection_decode)?
                .ok_or_else(|| protection_decode(ProtectionError::Stored("missing set")))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut loadout_stmt = conn.prepare(
        "SELECT id FROM protection_loadouts WHERE archived_at IS NULL ORDER BY created_at, id",
    )?;
    let loadout_ids = loadout_stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let loadouts = loadout_ids
        .into_iter()
        .map(|id| {
            read_loadout(conn, id)
                .map_err(protection_decode)?
                .ok_or_else(|| protection_decode(ProtectionError::Stored("missing loadout")))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let active_loadout_id = conn
        .query_row(
            "SELECT active_loadout_id FROM protection_state WHERE singleton = 1",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?
        .flatten();

    let mut rec_stmt = conn.prepare(
        "SELECT id FROM protection_reconciliations ORDER BY created_at DESC, id DESC LIMIT 12",
    )?;
    let rec_ids = rec_stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let recent_reconciliations = rec_ids
        .into_iter()
        .map(|id| read_reconciliation(conn, id).map_err(protection_decode))
        .collect::<Result<Vec<_>, _>>()?;

    let mut window_stmt = conn.prepare(
        "SELECT id FROM protection_cost_windows ORDER BY created_at DESC, id DESC LIMIT 12",
    )?;
    let window_ids = window_stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let recent_cost_windows = window_ids
        .into_iter()
        .map(|id| read_cost_window(conn, id).map_err(protection_decode))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ProtectionOverview {
        sets,
        loadouts,
        active_loadout_id,
        recent_reconciliations,
        recent_cost_windows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;

    async fn harness() -> (tempfile::TempDir, Db, Arc<MockClock>, ProtectionService) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("test.db"))
            .await
            .expect("open db");
        let clock = Arc::new(MockClock::new(None, 0.0));
        let service = ProtectionService::new(db.clone(), clock.clone());
        (dir, db, clock, service)
    }

    async fn limited_armour(
        service: &ProtectionService,
        name: &str,
        markup: f64,
    ) -> (ProtectionSet, ProtectionLoadout) {
        let set = service
            .create_set(
                ProtectionSetKind::Armour,
                name,
                ProtectionEconomyKind::Limited,
                Some(markup),
            )
            .await
            .expect("create set");
        let loadout = service
            .create_loadout(name, Some(set.id), None)
            .await
            .expect("create loadout");
        (set, loadout)
    }

    async fn seed_completed_session(
        db: &Db,
        session_id: &str,
        loadout: &ProtectionLoadout,
        set: &ProtectionSet,
        started_at: f64,
        ended_at: f64,
    ) {
        let session_id = session_id.to_string();
        let loadout_id = loadout.id;
        let loadout_name = loadout.name.clone();
        let set_id = set.id;
        let set_name = set.name.clone();
        let markup = set.markup_percent;
        let economy = set.economy_kind.as_str();
        db.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
                 VALUES (?1, ?2, ?3, 0)",
                rusqlite::params![session_id, started_at, ended_at],
            )?;
            conn.execute(
                "INSERT INTO session_intervals \
                 (session_id, kind, label, ref_id, started_at, ended_at) \
                 VALUES (?1, 'protection', ?2, ?3, ?4, ?5)",
                rusqlite::params![session_id, loadout_name, loadout_id, started_at, ended_at],
            )?;
            let interval_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO session_protection_intervals \
                 (interval_id, loadout_id, loadout_name, armour_set_id, armour_set_name, \
                  armour_economy_kind, armour_markup_percent) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    interval_id,
                    loadout_id,
                    loadout_name,
                    set_id,
                    set_name,
                    economy,
                    markup
                ],
            )?;
            conn.execute(
                "INSERT INTO protection_defence_events \
                 (session_id, protection_interval_id, damage, deflected) \
                 VALUES (?1, ?2, 100, 0)",
                rusqlite::params![session_id, interval_id],
            )?;
            Ok(())
        })
        .await
        .expect("seed session");
    }

    #[tokio::test]
    async fn a_single_completed_session_books_markup_adjusted_decay_once() {
        let (_dir, db, clock, service) = harness().await;
        let (set, loadout) = limited_armour(&service, "Adjusted Pixie", 125.0).await;
        let opening = service
            .confirm_observation(set.id, "open", 10.0, ObservationSource::Manual, None, None)
            .await
            .expect("opening observation");
        assert!(opening.cost_window.is_none());
        let opening_at = opening.observation.observed_at;

        seed_completed_session(
            &db,
            "session-1",
            &loadout,
            &set,
            opening_at + 1.0,
            opening_at + 5.0,
        )
        .await;
        clock.advance(10.0).expect("advance");

        let closing = service
            .confirm_observation(set.id, "close", 8.0, ObservationSource::Manual, None, None)
            .await
            .expect("closing observation");
        let window = closing.cost_window.as_ref().expect("cost window");
        assert_eq!(window.status, ReconciliationStatus::Booked);
        assert_eq!(window.allocations.len(), 1);
        assert_eq!(window.allocations[0].session_id, "session-1");
        assert!((window.cost_ped - 2.5).abs() < 1e-9);

        let repeated = service
            .confirm_observation(set.id, "close", 7.0, ObservationSource::Manual, None, None)
            .await
            .expect("idempotent repeat");
        assert_eq!(repeated, closing);
        let booked: f64 = db
            .with_reader(|conn| {
                conn.query_row(
                    "SELECT armour_cost FROM tracking_sessions WHERE id = 'session-1'",
                    [],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .await
            .expect("booked cost");
        assert!((booked - 2.5).abs() < 1e-9);
    }

    #[tokio::test]
    async fn deferred_limited_decay_spreads_across_sessions_by_hit_count_not_damage() {
        let (_dir, db, clock, service) = harness().await;
        let (set, loadout) = limited_armour(&service, "Deferred armour", 120.0).await;
        service
            .confirm_observation(
                set.id,
                "deferred-open",
                20.0,
                ObservationSource::Manual,
                None,
                None,
            )
            .await
            .expect("opening observation");
        seed_completed_session(&db, "earlier", &loadout, &set, 1.0, 2.0).await;
        seed_completed_session(&db, "later", &loadout, &set, 3.0, 4.0).await;
        db.with_writer(|conn| {
            conn.execute(
                "UPDATE protection_defence_events SET damage = 500 WHERE session_id = 'earlier'",
                [],
            )?;
            conn.execute(
                "UPDATE protection_defence_events SET damage = 700 WHERE session_id = 'later'",
                [],
            )?;
            Ok(())
        })
        .await
        .expect("weight evidence");
        clock.advance(10.0).expect("advance");

        let closing = service
            .confirm_observation(
                set.id,
                "deferred-close",
                10.0,
                ObservationSource::Manual,
                None,
                None,
            )
            .await
            .expect("closing observation");
        let window = closing.cost_window.expect("cost window");
        assert_eq!(window.allocations.len(), 2);
        assert!((window.cost_ped - 12.0).abs() < 1e-9);
        assert_eq!(window.allocations[0].hit_count, 1);
        assert_eq!(window.allocations[1].hit_count, 1);
        assert!((window.allocations[0].cost_ped - 6.0).abs() < 1e-9);
        assert!((window.allocations[1].cost_ped - 6.0).abs() < 1e-9);
        let (context_count, context_cost): (i64, f64) = db
            .with_reader(move |conn| {
                conn.query_row(
                    "SELECT COUNT(*), SUM(cost_ped) FROM protection_cost_context_allocations \
                     WHERE window_id = ?1",
                    [window.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(Into::into)
            })
            .await
            .expect("context allocations");
        assert_eq!(context_count, 2);
        assert!((context_cost - 12.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn deferred_unlimited_repair_consumes_all_unsettled_sessions() {
        let (_dir, db, _clock, service) = harness().await;
        let set = service
            .create_set(
                ProtectionSetKind::Armour,
                "Unlimited armour",
                ProtectionEconomyKind::Unlimited,
                None,
            )
            .await
            .expect("create set");
        let loadout = service
            .create_loadout("Unlimited", Some(set.id), None)
            .await
            .expect("create loadout");
        seed_completed_session(&db, "repair-earlier", &loadout, &set, 1.0, 2.0).await;
        seed_completed_session(&db, "repair-later", &loadout, &set, 3.0, 4.0).await;

        let outcome = service
            .confirm_repair_cost("repair-window", Some(set.id), None, 3.0)
            .await
            .expect("repair cost");
        assert_eq!(outcome.cost_window.allocations.len(), 2);
        assert!(outcome
            .cost_window
            .allocations
            .iter()
            .all(|allocation| (allocation.cost_ped - 1.5).abs() < 1e-9));
        let repeated = service
            .confirm_repair_cost("repair-window", Some(set.id), None, 9.0)
            .await
            .expect("idempotent repair repeat");
        assert_eq!(repeated, outcome);
        let overview = service.overview().await.expect("overview");
        let refreshed = overview
            .sets
            .iter()
            .find(|candidate| candidate.id == set.id)
            .unwrap();
        assert_eq!(refreshed.unsettled_sessions, 0);
    }

    #[tokio::test]
    async fn deflections_and_damage_events_are_equal_weight_hits() {
        let (_dir, db, _clock, service) = harness().await;
        let set = service
            .create_set(
                ProtectionSetKind::Armour,
                "Deflection armour",
                ProtectionEconomyKind::Unlimited,
                None,
            )
            .await
            .expect("create set");
        let loadout = service
            .create_loadout("Deflection loadout", Some(set.id), None)
            .await
            .expect("create loadout");
        seed_completed_session(&db, "one-deflection", &loadout, &set, 1.0, 2.0).await;
        seed_completed_session(&db, "two-deflections", &loadout, &set, 3.0, 4.0).await;
        db.with_writer(|conn| {
            conn.execute(
                "UPDATE protection_defence_events SET damage = NULL, deflected = 1",
                [],
            )?;
            let interval_id: i64 = conn.query_row(
                "SELECT protection_interval_id FROM protection_defence_events \
                 WHERE session_id = 'two-deflections' LIMIT 1",
                [],
                |row| row.get(0),
            )?;
            conn.execute(
                "INSERT INTO protection_defence_events \
                 (session_id, protection_interval_id, damage, deflected) \
                 VALUES ('two-deflections', ?1, NULL, 1)",
                [interval_id],
            )?;
            Ok(())
        })
        .await
        .expect("deflection evidence");

        let outcome = service
            .confirm_repair_cost("deflections", Some(set.id), None, 3.0)
            .await
            .expect("repair cost");
        assert_eq!(outcome.cost_window.allocations.len(), 2);
        assert!((outcome.cost_window.allocations[0].cost_ped - 1.0).abs() < 1e-9);
        assert!((outcome.cost_window.allocations[1].cost_ped - 2.0).abs() < 1e-9);
    }

    /// A session that opted out of per-segment attribution carries its
    /// evidence unattributed until a setup is named, and unattributed
    /// evidence cannot be settled by any cost that names a set. The
    /// recording surface has to be able to say which sessions are in
    /// that state, or a postponed one is stranded silently.
    #[tokio::test]
    async fn pending_attribution_names_the_sessions_still_owed_a_setup() {
        let (_dir, db, _clock, service) = harness().await;
        let set = service
            .create_set(
                ProtectionSetKind::Armour,
                "Hyperion",
                ProtectionEconomyKind::Unlimited,
                None,
            )
            .await
            .expect("create set");
        let loadout = service
            .create_loadout("Hyperion + 5B", Some(set.id), None)
            .await
            .expect("create loadout");
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO session_definitions (id, name) VALUES (3, 'Caly AI Dailies')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, definition_id, \
                  track_protection_costs, track_protection_by_segment) \
                 VALUES ('postponed', 10, 20, 0, 3, 1, 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracking_sessions \
                 (id, started_at, is_active, track_protection_costs, \
                  track_protection_by_segment) \
                 VALUES ('running', 30, 1, 1, 0)",
                [],
            )?;
            // By segment: declared through the per-segment route, never here.
            conn.execute(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, track_protection_costs, \
                  track_protection_by_segment) \
                 VALUES ('by-segment', 5, 8, 0, 1, 1)",
                [],
            )?;
            for session in ["postponed", "running", "by-segment"] {
                conn.execute(
                    "INSERT INTO protection_defence_events \
                     (session_id, damage, deflected) VALUES (?1, 80, 0)",
                    rusqlite::params![session],
                )?;
            }
            Ok(())
        })
        .await
        .expect("seed sessions");

        let pending = service
            .pending_attribution()
            .await
            .expect("read pending attribution");
        let ids: Vec<&str> = pending.iter().map(|row| row.session_id.as_str()).collect();
        assert_eq!(ids, vec!["running", "postponed"], "newest first");
        let postponed = &pending[1];
        assert_eq!(postponed.name.as_deref(), Some("Caly AI Dailies"));
        assert_eq!(postponed.ended_at, Some(20.0));
        assert_eq!(postponed.defence_event_count, 1);
        assert_eq!(pending[0].ended_at, None, "the running session is running");

        service
            .assign_session_loadout("postponed", loadout.id)
            .await
            .expect("name the postponed session's setup");
        let pending = service
            .pending_attribution()
            .await
            .expect("read pending attribution");
        let ids: Vec<&str> = pending.iter().map(|row| row.session_id.as_str()).collect();
        assert_eq!(ids, vec!["running"], "a named session stops being owed");
    }

    /// A window that already paid for a hit named the setup standing
    /// then. Naming a different one later must not move what that cost
    /// bought.
    #[tokio::test]
    async fn naming_a_setup_never_moves_evidence_a_cost_already_settled() {
        let (_dir, db, _clock, service) = harness().await;
        let set = service
            .create_set(
                ProtectionSetKind::Armour,
                "Hyperion",
                ProtectionEconomyKind::Unlimited,
                None,
            )
            .await
            .expect("create set");
        let first = service
            .create_loadout("Hyperion + 5B", Some(set.id), None)
            .await
            .expect("create loadout");
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, track_protection_costs, \
                  track_protection_by_segment) \
                 VALUES ('settled', 10, 20, 0, 1, 0)",
                [],
            )?;
            // One hit a generic repair already paid for, one it did not.
            conn.execute(
                "INSERT INTO protection_defence_events \
                 (session_id, damage, deflected) VALUES ('settled', 80, 0), ('settled', 40, 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO protection_cost_windows \
                 (kind, cost_ped, status, created_at) VALUES ('repair', 1.55, 'booked', 0)",
                [],
            )?;
            // id-order: insertion (the first event this test seeded).
            conn.execute(
                "INSERT INTO protection_cost_evidence (window_id, set_id, defence_event_id) \
                 SELECT 1, NULL, MIN(id) FROM protection_defence_events",
                [],
            )?;
            Ok(())
        })
        .await
        .expect("seed settled session");

        service
            .assign_session_loadout("settled", first.id)
            .await
            .expect("name the setup");

        let (settled_attributed, unsettled_attributed): (i64, i64) = db
            .with_reader(|conn| {
                Ok(conn.query_row(
                    "SELECT \
                       SUM(CASE WHEN EXISTS (SELECT 1 FROM protection_cost_evidence ce \
                                             WHERE ce.defence_event_id = d.id) \
                                 AND d.protection_interval_id IS NOT NULL \
                                THEN 1 ELSE 0 END), \
                       SUM(CASE WHEN NOT EXISTS (SELECT 1 FROM protection_cost_evidence ce \
                                                 WHERE ce.defence_event_id = d.id) \
                                 AND d.protection_interval_id IS NOT NULL \
                                THEN 1 ELSE 0 END) \
                     FROM protection_defence_events d WHERE d.session_id = 'settled'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .await
            .expect("count attributed");
        assert_eq!(
            settled_attributed, 0,
            "a hit a settled cost paid for keeps the attribution that cost was recorded against"
        );
        assert_eq!(
            unsettled_attributed, 1,
            "the hit still owed a cost is what the new setup takes"
        );
    }

    #[tokio::test]
    async fn opted_out_session_accepts_one_whole_session_loadout_after_stop() {
        let (_dir, db, _clock, service) = harness().await;
        let set = service
            .create_set(
                ProtectionSetKind::Armour,
                "Whole-session armour",
                ProtectionEconomyKind::Unlimited,
                None,
            )
            .await
            .expect("create set");
        let loadout = service
            .create_loadout("Whole-session setup", Some(set.id), None)
            .await
            .expect("create loadout");
        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, track_protection_by_segment) \
                 VALUES ('whole-session', 10, 20, 0, 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO protection_defence_events \
                 (session_id, damage, deflected) VALUES ('whole-session', 80, 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO protection_defence_events \
                 (session_id, damage, deflected) VALUES ('whole-session', NULL, 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracking_sessions \
                 (id, started_at, ended_at, is_active, track_protection_costs, \
                  track_protection_by_segment) \
                 VALUES ('armour-disabled', 10, 20, 0, 0, 0)",
                [],
            )?;
            Ok(())
        })
        .await
        .expect("seed opted-out session");

        let assigned = service
            .assign_session_loadout("whole-session", loadout.id)
            .await
            .expect("assign whole-session setup");
        assert_eq!(assigned.loadout_id, loadout.id);
        service
            .assign_session_loadout("whole-session", loadout.id)
            .await
            .expect("same setup is idempotent");
        assert!(matches!(
            service
                .assign_session_loadout("armour-disabled", loadout.id)
                .await,
            Err(ProtectionError::Conflict(_))
        ));

        let outcome = service
            .confirm_repair_cost("whole-session-window", Some(set.id), None, 4.0)
            .await
            .expect("book repair cost");
        assert_eq!(outcome.cost_window.allocations.len(), 1);
        assert_eq!(
            outcome.cost_window.allocations[0].session_id,
            "whole-session"
        );
        assert_eq!(outcome.cost_window.allocations[0].hit_count, 2);
        assert!((outcome.cost_window.allocations[0].cost_ped - 4.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn ambiguous_windows_stay_pending_and_increases_require_a_reset() {
        let (_dir, db, clock, service) = harness().await;
        let (measured, measured_loadout) = limited_armour(&service, "Measured armour", 110.0).await;
        let (other, other_loadout) = limited_armour(&service, "Other armour", 120.0).await;
        let opening = service
            .confirm_observation(
                measured.id,
                "pending-open",
                20.0,
                ObservationSource::Ocr,
                Some("20.00"),
                None,
            )
            .await
            .expect("opening observation");
        let opening_at = opening.observation.observed_at;
        seed_completed_session(
            &db,
            "session-other",
            &other_loadout,
            &other,
            opening_at + 1.0,
            opening_at + 4.0,
        )
        .await;
        clock.advance(5.0).expect("advance");

        let closing = service
            .confirm_observation(
                measured.id,
                "pending-close",
                19.0,
                ObservationSource::Manual,
                None,
                None,
            )
            .await
            .expect("pending observation");
        let window = closing.cost_window.expect("cost window");
        assert_eq!(window.status, ReconciliationStatus::Pending);
        assert!(window.allocations.is_empty());

        let increase = service
            .confirm_observation(
                measured.id,
                "increase",
                19.5,
                ObservationSource::Manual,
                None,
                None,
            )
            .await;
        assert!(matches!(increase, Err(ProtectionError::Conflict(_))));
        seed_completed_session(
            &db,
            "session-before-reset",
            &measured_loadout,
            &measured,
            opening_at + 5.0,
            opening_at + 6.0,
        )
        .await;
        let reset = service
            .confirm_observation(
                measured.id,
                "reset",
                19.5,
                ObservationSource::Manual,
                None,
                Some("Replaced a piece"),
            )
            .await
            .expect("reset baseline");
        let reset_window = reset.cost_window.expect("unpriced reset window");
        assert_eq!(reset_window.status, ReconciliationStatus::Pending);
        assert!(!reset_window.cost_known);
        assert!(reset_window.allocations.is_empty());
        let overview = service.overview().await.expect("overview after reset");
        let measured = overview
            .sets
            .iter()
            .find(|candidate| candidate.id == measured.id)
            .expect("measured set");
        assert_eq!(measured.unsettled_damage, 0.0);
        assert_eq!(measured.unsettled_sessions, 0);
        assert_eq!(
            measured.pending_reconciliations, 2,
            "the no-match measurement and unpriced reset both remain pending"
        );
    }

    #[tokio::test]
    async fn configuration_edits_preserve_observation_economics_and_relationships() {
        let (_dir, _db, _clock, service) = harness().await;
        let set = service
            .create_set(
                ProtectionSetKind::Armour,
                "Test armour",
                ProtectionEconomyKind::Limited,
                Some(120.0),
            )
            .await
            .expect("create set");
        let edited = service
            .update_set(
                set.id,
                "Renamed armour",
                ProtectionEconomyKind::Limited,
                Some(125.0),
            )
            .await
            .expect("edit unused set");
        assert_eq!(edited.name, "Renamed armour");
        assert_eq!(edited.markup_percent, Some(125.0));

        let loadout = service
            .create_loadout("First loadout", Some(set.id), None)
            .await
            .expect("create loadout");
        let edited_loadout = service
            .update_loadout(loadout.id, "Renamed loadout", Some(set.id), None)
            .await
            .expect("edit loadout");
        assert_eq!(edited_loadout.name, "Renamed loadout");
        assert_eq!(
            edited_loadout.armour.as_ref().map(|armour| armour.id),
            Some(set.id)
        );

        service
            .confirm_observation(
                set.id,
                "basis-lock",
                10.0,
                ObservationSource::Manual,
                None,
                None,
            )
            .await
            .expect("set baseline");
        let basis_change = service
            .update_set(
                set.id,
                "Renamed again",
                ProtectionEconomyKind::Limited,
                Some(130.0),
            )
            .await;
        assert!(matches!(basis_change, Err(ProtectionError::Conflict(_))));

        let renamed = service
            .update_set(
                set.id,
                "Renamed again",
                ProtectionEconomyKind::Limited,
                Some(125.0),
            )
            .await
            .expect("rename observed set");
        assert_eq!(renamed.name, "Renamed again");
    }
}
