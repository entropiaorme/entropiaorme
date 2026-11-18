//! Quest families: variants of one repeatable slot that cool as a unit.
//!
//! A family models a quest giver's per-line slot ("Daily Hunting 1"):
//! its members are the rotating variants ("Daily Hunting 1: Weak
//! Mortirex"), and availability is a family fact, because completing or
//! collecting today's variant gates every sibling. The family carries
//! the cooldown (hours plus an anchor); members contribute the anchor
//! instants (their last start or last completion), and the derived
//! expiry surfaces on every member row (`crud.rs`).
//!
//! Membership maintenance is deliberately asymmetric: CREATING a family
//! (or renaming one) sweeps unattached quests whose colon-split family
//! part matches, and CREATING a quest (manually or from a received
//! mission line) attaches by the same split; an UPDATE to a quest never
//! re-attaches implicitly, so a deliberate detach stays detached.

use serde_json::{json, Map, Value};

use crate::time::to_iso_utc;

use super::missions::normalize_quest_name;
use super::payload::value_to_sql;
use super::{QuestError, QuestService};

/// When a cooldown timer starts: at the member's last recorded start
/// (the NPC hands the mission over; the observed daily behaviour) or at
/// its last recorded completion (the pre-family model, and the natural
/// shape for boss runs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CooldownAnchor {
    Pickup,
    Completion,
}

/// When a cooldown anchored at `instant` and running for `hours` lifts
/// (epoch seconds); None when either half is absent or the window is
/// not positive, which is how "groups without gating" is stored. The
/// one place the window arithmetic lives, so the quest row, the family
/// row and the Activities offerings cannot drift apart on it.
pub(crate) fn cooldown_lift(instant: Option<f64>, hours: Option<f64>) -> Option<f64> {
    match (instant, hours) {
        (Some(instant), Some(hours)) if hours > 0.0 => Some(instant + hours * 3600.0),
        _ => None,
    }
}

/// The instant a completion-anchored timer runs from: the later of the
/// last completion and the last abandon. Giving today's mission up holds
/// the slot exactly as finishing it would, only without the reward.
pub(crate) fn completion_anchor_instant(
    last_completed: Option<f64>,
    last_abandoned: Option<f64>,
) -> Option<f64> {
    match (last_completed, last_abandoned) {
        (Some(completed), Some(abandoned)) => Some(completed.max(abandoned)),
        (instant, None) | (None, instant) => instant,
    }
}

/// The refusal a curated row answers any authoring write with.
pub(super) fn curated_refusal(what: &str) -> QuestError {
    QuestError::Invalid(format!(
        "This {what} is maintained by the app and cannot be edited"
    ))
}

impl CooldownAnchor {
    pub fn as_str(self) -> &'static str {
        match self {
            CooldownAnchor::Pickup => "pickup",
            CooldownAnchor::Completion => "completion",
        }
    }

    /// Parse the stored/wire vocabulary; anything else is a caller error.
    pub fn parse(value: &str) -> Result<Self, QuestError> {
        match value {
            "pickup" => Ok(CooldownAnchor::Pickup),
            "completion" => Ok(CooldownAnchor::Completion),
            other => Err(QuestError::Invalid(format!(
                "cooldown_anchor must be 'pickup' or 'completion', not '{other}'"
            ))),
        }
    }
}

/// The family SELECT: every column plus the derived anchor instants
/// (latest member start / completion) and the member count, so the
/// management surface can show availability without a second read.
const FAMILY_SELECT: &str = "\
    SELECT f.id, f.name, f.planet, f.cooldown_hours, f.cooldown_anchor, \
           f.is_active, f.created_at, f.updated_at, f.catalogue_key, \
           (SELECT COUNT(*) FROM quests m \
            WHERE m.family_id = f.id AND m.is_active = 1) AS member_count, \
           (SELECT MAX(m.last_started_at) FROM quests m \
            WHERE m.family_id = f.id) AS last_started_at, \
           (SELECT MAX(c.completed_at) \
            FROM session_quest_completions c \
            JOIN quests m ON m.id = c.quest_id \
            WHERE m.family_id = f.id \
              AND NOT EXISTS (SELECT 1 FROM quest_cooldown_resets r \
                              WHERE r.completion_id = c.id)) AS last_completed_at, \
           (SELECT MAX(r.abandoned_at) FROM quest_runs r \
            JOIN quests m ON m.id = r.quest_id \
            WHERE m.family_id = f.id) AS last_abandoned_at \
    FROM quest_families f";

impl QuestService {
    // ── Family CRUD ─────────────────────────────────────────────────

    /// List families (active only by default), newest-authored last.
    pub async fn get_families(&self, active_only: bool) -> Result<Vec<Value>, QuestError> {
        let where_clause = if active_only {
            "WHERE f.is_active = 1"
        } else {
            ""
        };
        let sql = format!("{FAMILY_SELECT} {where_clause} ORDER BY f.created_at ASC");
        Ok(self
            .db
            .with_reader(move |conn| {
                let mut stmt = conn.prepare(&sql)?;
                let mut rows = stmt.query([])?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    out.push(Value::Object(row_to_family(row)));
                }
                Ok(out)
            })
            .await?)
    }

    /// A single family by ID; `None` when absent.
    pub async fn get_family(&self, family_id: i64) -> Result<Option<Value>, QuestError> {
        let sql = format!("{FAMILY_SELECT} WHERE f.id = ?");
        Ok(self
            .db
            .with_reader(move |conn| {
                use rusqlite::OptionalExtension as _;
                Ok(conn
                    .query_row(&sql, rusqlite::params![family_id], |row| {
                        Ok(Value::Object(row_to_family(row)))
                    })
                    .optional()?)
            })
            .await?)
    }

    /// Create a family and sweep unattached matching variants into it.
    pub async fn create_family(&self, data: &Value) -> Result<Value, QuestError> {
        let name = require_family_name(data.get("name"))?;
        let cooldown_hours = validate_cooldown_hours(data.get("cooldown_hours"))?;
        let anchor = match data.get("cooldown_anchor").and_then(Value::as_str) {
            Some(anchor) => CooldownAnchor::parse(anchor)?,
            None => CooldownAnchor::Pickup,
        };
        let planet = match data.get("planet").and_then(Value::as_str) {
            Some(planet) if !planet.trim().is_empty() => planet.trim().to_string(),
            _ => "Calypso".to_string(),
        };

        let insert_name = name.clone();
        let family_id = self
            .db
            .with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO quest_families (name, planet, cooldown_hours, cooldown_anchor) \
                     VALUES (?, ?, ?, ?)",
                    rusqlite::params![insert_name, planet, cooldown_hours, anchor.as_str()],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .await?;

        self.attach_matching_quests(family_id, &name).await?;
        Ok(self
            .get_family(family_id)
            .await?
            .expect("the family was just inserted"))
    }

    /// Update a family's fields (present keys bind, absent keys keep);
    /// `None` when absent. A rename sweeps newly matching unattached
    /// quests; existing members stay members. A soft-deleted family
    /// reads as absent: mutating one would let a rename re-attach
    /// active quests to a dead family, undoing exactly the detach its
    /// deletion performed.
    pub async fn update_family(
        &self,
        family_id: i64,
        data: &Value,
    ) -> Result<Option<Value>, QuestError> {
        let Some(existing) = self.get_family(family_id).await? else {
            return Ok(None);
        };
        if existing.get("is_active").and_then(Value::as_i64) != Some(1) {
            return Ok(None);
        }
        if existing.get("curated").and_then(Value::as_bool) == Some(true) {
            return Err(curated_refusal("quest family"));
        }

        let mut updates: Vec<(&str, Value)> = Vec::new();
        if let Some(value) = data.get("name") {
            let name = require_family_name(Some(value))?;
            updates.push(("name", json!(name)));
        }
        if let Some(value) = data.get("planet") {
            let planet = value.as_str().map(str::trim).unwrap_or("");
            if planet.is_empty() {
                return Err(QuestError::Invalid(
                    "A family's planet cannot be blank".to_string(),
                ));
            }
            updates.push(("planet", json!(planet)));
        }
        if let Some(value) = data.get("cooldown_hours") {
            let hours = validate_cooldown_hours(Some(value))?;
            updates.push(("cooldown_hours", json!(hours)));
        }
        if let Some(value) = data.get("cooldown_anchor") {
            let anchor = value
                .as_str()
                .map(CooldownAnchor::parse)
                .transpose()?
                .ok_or_else(|| {
                    QuestError::Invalid(
                        "cooldown_anchor must be 'pickup' or 'completion', not null".to_string(),
                    )
                })?;
            updates.push(("cooldown_anchor", json!(anchor.as_str())));
        }

        if !updates.is_empty() {
            let now = self.now_epoch();
            let set_clause = updates
                .iter()
                .map(|(key, _)| format!("{key} = ?"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql =
                format!("UPDATE quest_families SET {set_clause}, updated_at = ? WHERE id = ?");
            let mut params: Vec<rusqlite::types::Value> = updates
                .iter()
                .map(|(_, value)| value_to_sql(value))
                .collect();
            params.push(rusqlite::types::Value::Real(now));
            params.push(rusqlite::types::Value::Integer(family_id));
            self.db
                .with_writer(move |conn| {
                    conn.execute(&sql, rusqlite::params_from_iter(params))?;
                    Ok(())
                })
                .await?;
        }

        if let Some(renamed) = data.get("name").and_then(Value::as_str) {
            let renamed = renamed.trim().to_string();
            self.attach_matching_quests(family_id, &renamed).await?;
        }
        self.get_family(family_id).await
    }

    /// Soft-delete a family and detach its members in one transaction:
    /// a deleted family must stop gating availability, and a member
    /// pointing at a dead family would keep its timers alive.
    pub async fn delete_family(&self, family_id: i64) -> Result<bool, QuestError> {
        if let Some(family) = self.get_family(family_id).await? {
            if family.get("curated").and_then(Value::as_bool) == Some(true) {
                return Err(curated_refusal("quest family"));
            }
        }
        Ok(self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let affected = tx.execute(
                    "UPDATE quest_families SET is_active = 0 WHERE id = ? AND is_active = 1",
                    rusqlite::params![family_id],
                )?;
                if affected > 0 {
                    tx.execute(
                        "UPDATE quests SET family_id = NULL WHERE family_id = ?",
                        rusqlite::params![family_id],
                    )?;
                }
                tx.commit()?;
                Ok(affected > 0)
            })
            .await?)
    }

    // ── Membership maintenance ──────────────────────────────────────

    /// Attach every active, unattached quest whose colon-split family
    /// part equals `family_name` (normalised). Never steals a member
    /// from another family.
    async fn attach_matching_quests(
        &self,
        family_id: i64,
        family_name: &str,
    ) -> Result<(), QuestError> {
        let family_norm = normalize_quest_name(family_name);
        if family_norm.is_empty() {
            return Ok(());
        }
        let candidates: Vec<(i64, String)> = self
            .db
            .with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, name FROM quests \
                     WHERE is_active = 1 AND family_id IS NULL AND name LIKE '%:%'",
                )?;
                let mut rows = stmt.query([])?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    out.push((row.get::<_, i64>(0)?, row.get::<_, String>(1)?));
                }
                Ok(out)
            })
            .await?;
        let members: Vec<i64> = candidates
            .into_iter()
            .filter(|(_, name)| {
                super::missions::variant_family_part(name).is_some_and(|part| part == family_norm)
            })
            .map(|(id, _)| id)
            .collect();
        if members.is_empty() {
            return Ok(());
        }
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                for quest_id in members {
                    tx.execute(
                        "UPDATE quests SET family_id = ? WHERE id = ? AND family_id IS NULL",
                        rusqlite::params![family_id, quest_id],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Find the active family whose name matches `family_part`
    /// (normalised equality); the auto-attach and auto-create lookups.
    pub(super) async fn find_family_by_norm(
        &self,
        family_part: &str,
    ) -> Result<Option<(i64, String)>, QuestError> {
        let families: Vec<(i64, String, String)> = self
            .db
            .with_reader(|conn| {
                let mut stmt = conn
                    .prepare("SELECT id, name, planet FROM quest_families WHERE is_active = 1")?;
                let mut rows = stmt.query([])?;
                let mut out = Vec::new();
                while let Some(row) = rows.next()? {
                    out.push((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ));
                }
                Ok(out)
            })
            .await?;
        Ok(families
            .into_iter()
            .find(|(_, name, _)| normalize_quest_name(name) == family_part)
            .map(|(id, _, planet)| (id, planet)))
    }

    /// Validate a wire/service `family_id` value: null detaches; a
    /// non-null id must name an active family.
    pub(super) async fn validate_family_ref(
        &self,
        value: &Value,
    ) -> Result<Option<i64>, QuestError> {
        if value.is_null() {
            return Ok(None);
        }
        let family_id = value.as_i64().ok_or_else(|| {
            QuestError::Invalid("family_id must be an integer or null".to_string())
        })?;
        match self.get_family(family_id).await? {
            Some(family) if family.get("is_active").and_then(Value::as_i64) == Some(1) => {
                Ok(Some(family_id))
            }
            _ => Err(QuestError::Invalid(format!(
                "family {family_id} does not exist or is not active"
            ))),
        }
    }
}

/// One family row to its dict shape, with the anchor-aware derived
/// cooldown expiry (UTC ISO instant), exactly the member rows' rule:
/// pickup anchors on the latest member start, completion on the latest
/// member completion.
fn row_to_family(row: &rusqlite::Row) -> Map<String, Value> {
    let mut family = Map::new();
    family.insert("id".into(), json!(row.get_unwrap::<_, i64>("id")));
    family.insert("name".into(), json!(row.get_unwrap::<_, String>("name")));
    family.insert(
        "planet".into(),
        json!(row.get_unwrap::<_, String>("planet")),
    );
    let cooldown_hours = row.get_unwrap::<_, Option<f64>>("cooldown_hours");
    family.insert("cooldown_hours".into(), json!(cooldown_hours));
    let anchor = row.get_unwrap::<_, String>("cooldown_anchor");
    family.insert("cooldown_anchor".into(), json!(anchor));
    family.insert(
        "is_active".into(),
        json!(row.get_unwrap::<_, i64>("is_active")),
    );
    family.insert(
        "created_at".into(),
        json!(row.get_unwrap::<_, f64>("created_at")),
    );
    family.insert(
        "updated_at".into(),
        json!(row.get_unwrap::<_, Option<f64>>("updated_at")),
    );
    family.insert(
        "member_count".into(),
        json!(row.get_unwrap::<_, i64>("member_count")),
    );
    let catalogue_key = row.get_unwrap::<_, Option<String>>("catalogue_key");
    family.insert("curated".into(), json!(catalogue_key.is_some()));
    family.insert("catalogue_key".into(), json!(catalogue_key));
    let last_started = row.get_unwrap::<_, Option<f64>>("last_started_at");
    let last_completed = row.get_unwrap::<_, Option<f64>>("last_completed_at");
    let last_abandoned = row.get_unwrap::<_, Option<f64>>("last_abandoned_at");
    family.insert("last_started_at".into(), json!(last_started));
    family.insert("last_completed_at".into(), json!(last_completed));
    family.insert("last_abandoned_at".into(), json!(last_abandoned));

    let anchor_instant = match anchor.as_str() {
        "pickup" => last_started,
        _ => completion_anchor_instant(last_completed, last_abandoned),
    };
    let expires = cooldown_lift(anchor_instant, cooldown_hours).map(to_iso_utc);
    family.insert("cooldown_expires_at".into(), json!(expires));
    family
}

/// A family's name, trimmed and required non-empty.
fn require_family_name(value: Option<&Value>) -> Result<String, QuestError> {
    let name = value
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if name.is_empty() {
        return Err(QuestError::Invalid(
            "A family needs a non-empty name".to_string(),
        ));
    }
    Ok(name.to_string())
}

/// Cooldown hours: null means "groups without gating"; a number must
/// be positive.
fn validate_cooldown_hours(value: Option<&Value>) -> Result<Option<f64>, QuestError> {
    match value {
        None => Ok(None),
        Some(Value::Null) => Ok(None),
        Some(value) => {
            let hours = value.as_f64().ok_or_else(|| {
                QuestError::Invalid("cooldown_hours must be a number or null".to_string())
            })?;
            if hours <= 0.0 {
                return Err(QuestError::Invalid(
                    "cooldown_hours must be positive (or null for no gate)".to_string(),
                ));
            }
            Ok(Some(hours))
        }
    }
}
