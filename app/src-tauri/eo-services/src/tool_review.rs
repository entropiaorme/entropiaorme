//! Settling, after a session, the things read off the screen as the tool
//! in hand that are not entries of the player's equipment library.
//!
//! While the held tool is read, the tracker keeps a sighting (see the
//! tracker's `held_tool`) of each tool of the bundled game data held that
//! the library does not hold, with the shots booked to it, and of each name
//! read that matches no tool at all. The review lists a session's
//! sightings and lets the player settle each one once:
//!
//! - **add** (a tool of the game data only, once the session has ended):
//!   the tool becomes a library entry built as the equipment add path
//!   builds a pick of its kind with nothing else chosen (the caller builds
//!   it, so there is one builder), enabled. When the library already holds
//!   an entry of that item, the sighting links to it instead of creating a
//!   second.
//! - **dismiss**: the name joins the names the player said are not tracked
//!   tools, so the reader takes it from then on as a known thing that is
//!   not a tool. A sighting still unsettled in another session whose name
//!   is dismissed asks nothing more.
//! - **leave**: it is asked about no more for this session.
//!
//! Adding moves no cost. A weapon's shots and a harvesting tool's uses were
//! booked under the tool's name at its cost without attachments, which is
//! how the new entry prices them, and every per-weapon figure keys on the
//! name; they are therefore already what the entry would book, and stay so.
//! A healing tool's heals were left unbilled (a paid heal is recorded
//! against a library entry), and stay so until each is marked as a paid use
//! of the entry in the session's healing review. Nothing here writes a
//! cost, a kill or a rollup.
//!
//! Every settlement can be undone, which asks about the sighting again. An
//! undone dismissal takes the name off the list, unless another dismissed
//! sighting carries it. An undone addition takes away the entry it created
//! only when nothing refers to it and it cannot be in use: it is exactly as
//! the addition made it, still of the game's data, no other sighting became
//! it, no session runs now or ran at or after the addition, no stored row
//! names it, and the player's settings bind or carry it nowhere. Otherwise
//! the entry stays, and the undoing says why. An entry the addition only
//! linked to always stays.
//!
//! Names compare as the reader compares them: by their letters and digits
//! alone ([`label_key`]). A dismissed name no dismissed sighting carries any
//! more (its session deleted) is released, so a name is never left set
//! aside by nothing; the player can also bring any name back from the list
//! of what is not tracked.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use rusqlite::OptionalExtension;

use crate::bundled_tools::kind_of;
use crate::bus_events::HotbarItemKind;
use crate::clock::Clock;
use crate::db::{delete_equipment_row, insert_equipment_row, Db, DbError};
use crate::game_ui::field::label_key;
use crate::game_ui::tool_names::ToolKind;
use crate::time::{instant_to_epoch, resolve_local};

/// What a sighting is of.
#[derive(Debug, Clone, PartialEq)]
pub enum Sighted {
    /// A tool of the bundled game data the library does not hold: its
    /// collection, its id there, its name, what kind of tool it is, and
    /// the shots booked to it (a weapon's; None while not known).
    Bundled {
        endpoint: String,
        item_id: String,
        name: String,
        kind: ToolKind,
        shots: Option<i64>,
    },
    /// A name read that matched no tool, as it was read.
    Unknown { text: String },
}

impl Sighted {
    /// The name the player knows it by: the tool's, or the text read.
    pub fn name(&self) -> &str {
        match self {
            Sighted::Bundled { name, .. } => name,
            Sighted::Unknown { text } => text,
        }
    }
}

/// What a settlement was, apart from what it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettledKind {
    Added,
    Dismissed,
    Left,
}

impl SettledKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Dismissed => "dismissed",
            Self::Left => "left",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "added" => Some(Self::Added),
            "dismissed" => Some(Self::Dismissed),
            "left" => Some(Self::Left),
            _ => None,
        }
    }
}

/// How a sighting was settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// Added to the library: an entry the addition created, or one of the
    /// same item the library already held.
    Added {
        created: bool,
    },
    Dismissed,
    Left,
}

impl Settled {
    pub fn kind(self) -> SettledKind {
        match self {
            Settled::Added { .. } => SettledKind::Added,
            Settled::Dismissed => SettledKind::Dismissed,
            Settled::Left => SettledKind::Left,
        }
    }
}

/// One sighting, as the review lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSighting {
    pub id: String,
    pub sighted: Sighted,
    /// Observed times, epoch seconds.
    pub first_seen: f64,
    pub last_seen: f64,
    /// None while it is still to be settled.
    pub settled: Option<Settled>,
}

/// A session's sightings as the review lists them: the ones still to be
/// settled first, tools of the game data before names read, each in the
/// order first seen.
#[derive(Debug, Clone, PartialEq)]
pub struct SightingReview {
    /// The session has ended, so its tools can be added.
    pub ended: bool,
    pub sightings: Vec<ToolSighting>,
}

/// Why a settlement or its undoing was refused: nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No such sighting (its session may have been deleted).
    NotFound,
    /// The sighting belongs to another session than the one named.
    OtherSession,
    /// It is settled already, as this.
    AlreadySettled(SettledKind),
    /// There is no settlement to undo.
    NotSettled,
    /// A tool is added once its session has ended.
    SessionRunning,
    /// A name that matched no tool cannot be added.
    NotATool,
    /// The tool is no longer in the bundled game data.
    NotInGameData,
    /// The tool cannot be added without choices its data does not give (a
    /// healing tool with no published heal): it is added from Equipment.
    NeedsSetUp,
}

/// A library entry as the equipment add path builds it for a sighted tool
/// with nothing else chosen: the name and stored props. Its kind and item
/// come from the sighting.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEntry {
    pub name: String,
    pub properties_json: String,
}

/// Why the entry an addition created stays when the addition is undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptBecause {
    /// It was set up, renamed or turned off since it was added.
    Changed,
    /// Its tool is no longer in the game's data, so what it was made as
    /// cannot be told.
    NotInGameData,
    /// Another sighting became it.
    OtherSighting,
    /// A session runs now, or ran at or after the addition: it may have
    /// used it.
    InSession,
    /// A stored row names it (a correction, a use, a dose, a shot's
    /// candidates).
    Referenced,
    /// The player's settings bind it to a hotbar slot, carry it, or name
    /// it as a harvesting tool.
    InSettings,
}

/// What undoing an addition did with the entry it created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryOnUndo {
    /// No entry this addition created is in the library: it linked to one
    /// already there, or the entry was deleted since.
    None,
    /// The entry it created was taken out of the library.
    Removed,
    /// The entry it created stays, for this reason.
    Kept(KeptBecause),
}

/// What only the caller can tell about the entry an addition created,
/// for undoing it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UndoContext {
    /// The entry the add path would build for the tool now; None when it
    /// cannot be built (the tool has left the game's data).
    pub bare: Option<NewEntry>,
    /// The player's settings bind the entry to a hotbar slot on any
    /// preset, carry it, or name it as a harvesting tool.
    pub in_settings: bool,
}

/// A name set aside as not a tracked tool.
#[derive(Debug, Clone, PartialEq)]
pub struct DismissedName {
    pub name: String,
    /// Epoch seconds.
    pub dismissed_at: f64,
}

/// What a settlement, or its undoing, did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    Added {
        equipment_id: i64,
        created: bool,
        kind: ToolKind,
        /// The entry is on; one linked to may be turned off.
        enabled: bool,
    },
    Dismissed,
    Left,
    /// The settlement `was` is undone, and the sighting is to be settled
    /// again; `entry` says what became of an entry an addition created.
    Reopened {
        was: SettledKind,
        entry: EntryOnUndo,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ToolReviewError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("Session not found")]
    SessionNotFound,
    /// What is kept no longer reads as anything this build knows: a fault
    /// of the store, not of the request.
    #[error("{0}")]
    Corrupt(String),
}

impl From<rusqlite::Error> for ToolReviewError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(DbError::Sqlite(error))
    }
}

/// A refusal is the inner error, so only a settlement actually made is
/// committed; a failure of the store is the outer one.
pub type Outcome<T> = Result<Result<T, Refusal>, ToolReviewError>;

/// The library's item type for a kind of tool, as the add path stores it.
pub fn item_type(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Weapon => "weapon",
        ToolKind::Healing => "healing",
        ToolKind::Tool => "tool",
    }
}

/// The kind of held tool a bundled collection holds.
fn tool_kind(endpoint: &str) -> Option<ToolKind> {
    match kind_of(endpoint)? {
        HotbarItemKind::Weapon => Some(ToolKind::Weapon),
        HotbarItemKind::Healing => Some(ToolKind::Healing),
        HotbarItemKind::Harvesting => Some(ToolKind::Tool),
        HotbarItemKind::Consumable => None,
    }
}

/// A name as it is kept on the dismissed list: its words, one space apart.
fn tidy(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub struct ToolReviewService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl ToolReviewService {
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    fn now(&self) -> f64 {
        instant_to_epoch(resolve_local(self.clock.now()))
    }

    /// A session's sightings, the ones still to be settled first.
    pub async fn sightings(&self, session_id: &str) -> Result<SightingReview, ToolReviewError> {
        let session_id = session_id.to_string();
        self.db
            .with_reader(move |conn| Ok(read_review(conn, &session_id)))
            .await?
    }

    /// One sighting of the session, as it stands.
    pub async fn sighting(&self, session_id: &str, sighting_id: &str) -> Outcome<ToolSighting> {
        let (session_id, sighting_id) = (session_id.to_string(), sighting_id.to_string());
        self.db
            .with_reader(move |conn| {
                Ok(match read_row(conn, &sighting_id) {
                    Ok(Some(row)) if row.session_id == session_id => Ok(Ok(row.sighting)),
                    Ok(Some(_)) => Ok(Err(Refusal::OtherSession)),
                    Ok(None) => Ok(Err(Refusal::NotFound)),
                    Err(error) => Err(error),
                })
            })
            .await?
    }

    /// Add the sighted tool of an ended session to the library as `entry`,
    /// or link it to the entry of that item already there.
    pub async fn add(
        &self,
        session_id: &str,
        sighting_id: &str,
        entry: NewEntry,
    ) -> Outcome<Settlement> {
        let now = self.now();
        self.write(
            session_id,
            sighting_id,
            move |tx, session_id, sighting_id| add_in(tx, session_id, sighting_id, &entry, now),
        )
        .await
    }

    /// Dismiss the sighted name as not a tracked tool.
    pub async fn dismiss(&self, session_id: &str, sighting_id: &str) -> Outcome<Settlement> {
        let now = self.now();
        self.write(
            session_id,
            sighting_id,
            move |tx, session_id, sighting_id| dismiss_in(tx, session_id, sighting_id, now),
        )
        .await
    }

    /// Leave the sighting: asked about no more for this session.
    pub async fn leave(&self, session_id: &str, sighting_id: &str) -> Outcome<Settlement> {
        let now = self.now();
        self.write(
            session_id,
            sighting_id,
            move |tx, session_id, sighting_id| leave_in(tx, session_id, sighting_id, now),
        )
        .await
    }

    /// Undo the sighting's settlement, so it is asked about again. For an
    /// addition, `context` is what the caller can tell about the entry it
    /// created (see the module doc for when it is taken away).
    pub async fn undo(
        &self,
        session_id: &str,
        sighting_id: &str,
        context: UndoContext,
    ) -> Outcome<Settlement> {
        self.write(
            session_id,
            sighting_id,
            move |tx, session_id, sighting_id| undo_in(tx, session_id, sighting_id, &context),
        )
        .await
    }

    /// The names set aside as not tracked tools, oldest first.
    pub async fn dismissed(&self) -> Result<Vec<DismissedName>, ToolReviewError> {
        Ok(self.db.with_reader(|conn| read_dismissed(conn)).await?)
    }

    /// Bring a name set aside back: the reader matches it again from its
    /// next look. Every kept name reading as `name` leaves the list (the
    /// sightings dismissed as it keep their settlement). Answers whether
    /// any did.
    pub async fn track_again(&self, name: &str) -> Result<bool, ToolReviewError> {
        let key = label_key(name);
        Ok(self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let mut released = false;
                for kept in crate::tool_catalogue::dismissed_names(&tx)? {
                    if label_key(&kept) == key {
                        tx.execute("DELETE FROM dismissed_tool_names WHERE name = ?1", [&kept])?;
                        released = true;
                    }
                }
                tx.commit()?;
                Ok(released)
            })
            .await?)
    }

    /// Run `apply` in one transaction, committed only when it settled.
    async fn write<F>(&self, session_id: &str, sighting_id: &str, apply: F) -> Outcome<Settlement>
    where
        F: FnOnce(&rusqlite::Transaction<'_>, &str, &str) -> Outcome<Settlement> + Send + 'static,
    {
        let (session_id, sighting_id) = (session_id.to_string(), sighting_id.to_string());
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let outcome = match apply(&tx, &session_id, &sighting_id) {
                    Ok(outcome) => outcome,
                    Err(error) => return Ok(Err(error)),
                };
                if outcome.is_ok() {
                    tx.commit()?;
                }
                Ok(Ok(outcome))
            })
            .await?
    }
}

/// One stored sighting with what only the writes need.
struct Row {
    session_id: String,
    sighting: ToolSighting,
    settled_at: Option<f64>,
    equipment_id: Option<i64>,
}

const COLUMNS: &str = "id, session_id, kind, endpoint, item_id, item_name, text_read, \
                       first_seen, last_seen, shots, settled_kind, settled_at, equipment_id, \
                       entry_created";

fn parse_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Row, ToolReviewError>> {
    let id: String = row.get(0)?;
    let kind: String = row.get(2)?;
    let endpoint: Option<String> = row.get(3)?;
    let item_id: Option<String> = row.get(4)?;
    let item_name: Option<String> = row.get(5)?;
    let text_read: Option<String> = row.get(6)?;
    let settled_kind: Option<String> = row.get(10)?;
    let entry_created: bool = row.get(13)?;
    let sighted = match (kind.as_str(), endpoint, item_id, item_name, text_read) {
        ("bundled", Some(endpoint), Some(item_id), Some(name), _) => match tool_kind(&endpoint) {
            Some(tool) => Sighted::Bundled {
                endpoint,
                item_id,
                name,
                kind: tool,
                shots: row.get(9)?,
            },
            None => {
                return Ok(Err(ToolReviewError::Corrupt(format!(
                    "a kept sighting names {endpoint:?}, which holds no tool"
                ))))
            }
        },
        ("unknown", _, _, _, Some(text)) => Sighted::Unknown { text },
        _ => {
            return Ok(Err(ToolReviewError::Corrupt(format!(
                "a kept sighting of kind {kind:?} is incomplete"
            ))))
        }
    };
    let settled = match settled_kind.as_deref().map(SettledKind::parse) {
        None => None,
        Some(Some(SettledKind::Added)) => Some(Settled::Added {
            created: entry_created,
        }),
        Some(Some(SettledKind::Dismissed)) => Some(Settled::Dismissed),
        Some(Some(SettledKind::Left)) => Some(Settled::Left),
        Some(None) => {
            return Ok(Err(ToolReviewError::Corrupt(format!(
                "a kept sighting is settled as {settled_kind:?}"
            ))))
        }
    };
    Ok(Ok(Row {
        session_id: row.get(1)?,
        sighting: ToolSighting {
            id,
            sighted,
            first_seen: row.get(7)?,
            last_seen: row.get(8)?,
            settled,
        },
        settled_at: row.get(11)?,
        equipment_id: row.get(12)?,
    }))
}

fn read_row(
    conn: &rusqlite::Connection,
    sighting_id: &str,
) -> Result<Option<Row>, ToolReviewError> {
    let row = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM tool_sightings WHERE id = ?1"),
            [sighting_id],
            parse_row,
        )
        .optional()?;
    row.transpose()
}

/// Whether the session has ended; None when there is no such session.
fn session_ended(conn: &rusqlite::Connection, session_id: &str) -> Result<Option<bool>, DbError> {
    Ok(conn
        .query_row(
            "SELECT ended_at IS NOT NULL AND is_active = 0 FROM tracking_sessions WHERE id = ?",
            [session_id],
            |row| row.get(0),
        )
        .optional()?)
}

fn read_review(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<SightingReview, ToolReviewError> {
    let Some(ended) = session_ended(conn, session_id)? else {
        return Err(ToolReviewError::SessionNotFound);
    };
    let dismissed: Vec<Vec<char>> = crate::tool_catalogue::dismissed_names(conn)?
        .iter()
        .map(|name| label_key(name))
        .collect();
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM tool_sightings WHERE session_id = ?1 ORDER BY first_seen, rowid"
    ))?;
    let rows = stmt
        .query_map([session_id], parse_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut sightings = Vec::with_capacity(rows.len());
    for row in rows {
        let sighting = row?.sighting;
        // A name dismissed since, from another session, is not asked again.
        if sighting.settled.is_none() && dismissed.contains(&label_key(sighting.sighted.name())) {
            continue;
        }
        sightings.push(sighting);
    }
    sightings.sort_by_key(|sighting| {
        (
            sighting.settled.is_some(),
            matches!(sighting.sighted, Sighted::Unknown { .. }),
        )
    });
    Ok(SightingReview { ended, sightings })
}

/// The sighting of `session_id` still to be settled, or why it cannot be.
fn unsettled(tx: &rusqlite::Transaction<'_>, session_id: &str, sighting_id: &str) -> Outcome<Row> {
    let Some(row) = read_row(tx, sighting_id)? else {
        return Ok(Err(Refusal::NotFound));
    };
    if row.session_id != session_id {
        return Ok(Err(Refusal::OtherSession));
    }
    if let Some(settled) = row.sighting.settled {
        return Ok(Err(Refusal::AlreadySettled(settled.kind())));
    }
    Ok(Ok(row))
}

fn mark_settled(
    tx: &rusqlite::Transaction<'_>,
    sighting_id: &str,
    kind: SettledKind,
    now: f64,
    equipment: Option<(i64, bool)>,
) -> Result<(), DbError> {
    tx.execute(
        "UPDATE tool_sightings SET settled_kind = ?1, settled_at = ?2, equipment_id = ?3, \
         entry_created = ?4 WHERE id = ?5",
        rusqlite::params![
            kind.as_str(),
            now,
            equipment.map(|(id, _)| id),
            equipment.is_some_and(|(_, created)| created),
            sighting_id
        ],
    )?;
    Ok(())
}

fn add_in(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    sighting_id: &str,
    entry: &NewEntry,
    now: f64,
) -> Outcome<Settlement> {
    let row = match unsettled(tx, session_id, sighting_id)? {
        Ok(row) => row,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let Sighted::Bundled { item_id, kind, .. } = &row.sighting.sighted else {
        return Ok(Err(Refusal::NotATool));
    };
    // Shots booked to a tool while its session runs are still being
    // flushed; it is added once they are all kept.
    if session_ended(tx, session_id)? != Some(true) {
        return Ok(Err(Refusal::SessionRunning));
    }
    let item_type = item_type(*kind);
    // The entry of this exact item the library already holds: one in use
    // before one turned off, then the oldest.
    let existing: Option<(i64, bool)> = tx
        .query_row(
            "SELECT id, enabled FROM equipment_library WHERE item_type = ?1 AND catalog_id = ?2 \
             ORDER BY enabled DESC, created_at, id LIMIT 1",
            rusqlite::params![item_type, item_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (equipment_id, created, enabled) = match existing {
        Some((id, enabled)) => (id, false, enabled),
        None => (
            insert_equipment_row(
                tx,
                &entry.name,
                item_type,
                Some(item_id),
                &entry.properties_json,
            )?,
            true,
            true,
        ),
    };
    mark_settled(
        tx,
        sighting_id,
        SettledKind::Added,
        now,
        Some((equipment_id, created)),
    )?;
    Ok(Ok(Settlement::Added {
        equipment_id,
        created,
        kind: *kind,
        enabled,
    }))
}

fn dismiss_in(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    sighting_id: &str,
    now: f64,
) -> Outcome<Settlement> {
    let row = match unsettled(tx, session_id, sighting_id)? {
        Ok(row) => row,
        Err(refusal) => return Ok(Err(refusal)),
    };
    tx.execute(
        "INSERT INTO dismissed_tool_names (name, dismissed_at) VALUES (?1, ?2) \
         ON CONFLICT (name) DO NOTHING",
        rusqlite::params![tidy(row.sighting.sighted.name()), now],
    )?;
    mark_settled(tx, sighting_id, SettledKind::Dismissed, now, None)?;
    Ok(Ok(Settlement::Dismissed))
}

fn leave_in(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    sighting_id: &str,
    now: f64,
) -> Outcome<Settlement> {
    if let Err(refusal) = unsettled(tx, session_id, sighting_id)? {
        return Ok(Err(refusal));
    }
    mark_settled(tx, sighting_id, SettledKind::Left, now, None)?;
    Ok(Ok(Settlement::Left))
}

fn undo_in(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    sighting_id: &str,
    context: &UndoContext,
) -> Outcome<Settlement> {
    let Some(row) = read_row(tx, sighting_id)? else {
        return Ok(Err(Refusal::NotFound));
    };
    if row.session_id != session_id {
        return Ok(Err(Refusal::OtherSession));
    }
    let Some(settled) = row.sighting.settled else {
        return Ok(Err(Refusal::NotSettled));
    };
    let entry = match settled {
        Settled::Left => EntryOnUndo::None,
        Settled::Dismissed => {
            undismiss(tx, &row)?;
            EntryOnUndo::None
        }
        Settled::Added { created } => match (created, row.equipment_id) {
            (true, Some(equipment_id)) => match kept_because(tx, &row, equipment_id, context)? {
                Some(because) => EntryOnUndo::Kept(because),
                None => {
                    delete_equipment_row(tx, equipment_id)?;
                    EntryOnUndo::Removed
                }
            },
            _ => EntryOnUndo::None,
        },
    };
    tx.execute(
        "UPDATE tool_sightings SET settled_kind = NULL, settled_at = NULL, equipment_id = NULL, \
         entry_created = 0 WHERE id = ?1",
        [sighting_id],
    )?;
    Ok(Ok(Settlement::Reopened {
        was: settled.kind(),
        entry,
    }))
}

/// The reader's keys of the names dismissed sightings carry, but for the
/// sighting `except`.
fn dismissed_sighting_keys(
    conn: &rusqlite::Connection,
    except: Option<&str>,
) -> Result<Vec<Vec<char>>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(item_name, text_read) FROM tool_sightings \
         WHERE settled_kind = 'dismissed' AND id IS NOT ?1",
    )?;
    let names = stmt
        .query_map([except], |row| row.get::<_, Option<String>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names.iter().flatten().map(|name| label_key(name)).collect())
}

/// Take the dismissed sighting's name off the list, unless another
/// dismissed sighting carries it.
fn undismiss(tx: &rusqlite::Transaction<'_>, row: &Row) -> Result<(), DbError> {
    let key = label_key(row.sighting.sighted.name());
    if dismissed_sighting_keys(tx, Some(&row.sighting.id))?.contains(&key) {
        return Ok(());
    }
    for name in crate::tool_catalogue::dismissed_names(tx)? {
        if label_key(&name) == key {
            tx.execute("DELETE FROM dismissed_tool_names WHERE name = ?1", [&name])?;
        }
    }
    Ok(())
}

/// Release every dismissed name no dismissed sighting carries any more (a
/// session holding the only one was deleted), inside the caller's
/// transaction. Answers whether any name left the list, so the caller
/// rebuilds the reader's catalogue.
pub fn release_unclaimed_names(tx: &rusqlite::Transaction<'_>) -> Result<bool, DbError> {
    let claimed = dismissed_sighting_keys(tx, None)?;
    let mut released = false;
    for name in crate::tool_catalogue::dismissed_names(tx)? {
        if !claimed.contains(&label_key(&name)) {
            tx.execute("DELETE FROM dismissed_tool_names WHERE name = ?1", [&name])?;
            released = true;
        }
    }
    Ok(released)
}

fn read_dismissed(conn: &rusqlite::Connection) -> Result<Vec<DismissedName>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT name, dismissed_at FROM dismissed_tool_names ORDER BY dismissed_at, name",
    )?;
    let names = stmt
        .query_map([], |row| {
            Ok(DismissedName {
                name: row.get(0)?,
                dismissed_at: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names)
}

/// The tables with an equipment id column, read off the schema itself so
/// a table added later is covered: every row in them naming an entry is a
/// use of it, a correction to it, or a sighting that became it.
fn equipment_id_tables(conn: &rusqlite::Connection) -> Result<Vec<String>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT m.name FROM sqlite_master m, pragma_table_info(m.name) c \
         WHERE m.type = 'table' AND c.name = 'equipment_id' ORDER BY m.name",
    )?;
    let tables = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(tables)
}

/// Why the entry an addition created must stay with it undone; None when
/// it may be taken away: it is still exactly the entry the add path builds
/// for the tool, on, of the sighted item; no other sighting became it; no
/// session runs now or ran at or after the addition; no stored row names
/// it; and the player's settings hold it nowhere.
fn kept_because(
    tx: &rusqlite::Transaction<'_>,
    row: &Row,
    equipment_id: i64,
    context: &UndoContext,
) -> Result<Option<KeptBecause>, ToolReviewError> {
    let Some(bare) = &context.bare else {
        return Ok(Some(KeptBecause::NotInGameData));
    };
    let Some(added_at) = row.settled_at else {
        return Ok(Some(KeptBecause::Changed));
    };
    let Sighted::Bundled { item_id, kind, .. } = &row.sighting.sighted else {
        return Ok(Some(KeptBecause::Changed));
    };
    let stored: Option<(String, String, Option<String>, String, bool)> = tx
        .query_row(
            "SELECT name, item_type, catalog_id, properties_json, enabled \
             FROM equipment_library WHERE id = ?1",
            [equipment_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((name, stored_type, catalog_id, props, enabled)) = stored else {
        return Ok(Some(KeptBecause::Changed));
    };
    let same_props = matches!(
        (
            serde_json::from_str::<serde_json::Value>(&props),
            serde_json::from_str::<serde_json::Value>(&bare.properties_json),
        ),
        (Ok(stored), Ok(built)) if stored == built
    );
    if !(same_props
        && enabled
        && name == bare.name
        && stored_type == item_type(*kind)
        && catalog_id.as_deref() == Some(item_id.as_str()))
    {
        return Ok(Some(KeptBecause::Changed));
    }
    let shared: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM tool_sightings WHERE equipment_id = ?1 AND id <> ?2)",
        rusqlite::params![equipment_id, row.sighting.id],
        |row| row.get(0),
    )?;
    if shared {
        return Ok(Some(KeptBecause::OtherSighting));
    }
    let in_session: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM tracking_sessions \
         WHERE is_active <> 0 OR started_at >= ?1 OR ended_at >= ?1)",
        [added_at],
        |row| row.get(0),
    )?;
    if in_session {
        return Ok(Some(KeptBecause::InSession));
    }
    for table in equipment_id_tables(tx)? {
        if table == "tool_sightings" {
            continue;
        }
        let named: bool = tx.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM \"{table}\" WHERE equipment_id = ?1)"),
            [equipment_id],
            |row| row.get(0),
        )?;
        if named {
            return Ok(Some(KeptBecause::Referenced));
        }
    }
    // A shot keeps the weapons carried when it landed as candidates.
    let a_candidate: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM weapon_shot_evidence e, json_each(e.candidates_json) c \
         WHERE json_extract(c.value, '$.equipmentId') = ?1)",
        [equipment_id],
        |row| row.get(0),
    )?;
    if a_candidate {
        return Ok(Some(KeptBecause::Referenced));
    }
    if context.in_settings {
        return Ok(Some(KeptBecause::InSettings));
    }
    Ok(None)
}
