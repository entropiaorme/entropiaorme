//! What the quest catalogue can offer the overlay's Activities control
//! right now: a lean projection, deliberately not the enriched CRUD read.
//!
//! The Activities control refreshes on every tracking frame, so this
//! read carries only what a chip needs (identity, the two lifecycle
//! facts, and the instant the availability gate lifts) and skips the
//! per-quest mob round trips the management surface wants.
//! The cooldown arithmetic is the shared [`cooldown_lift`], so an
//! offering can never disagree with the quest row about when a gate
//! lifts.

use crate::db::{Db, DbError};

use super::families::{completion_anchor_instant, cooldown_lift};

/// The lean projection: the same SELECT shape the enriched read uses,
/// narrowed to the availability picture.
const OFFER_SELECT: &str = "\
    SELECT q.id, q.name, q.planet, q.started_at, q.signal_loot_item, q.completion_mode, \
           q.family_id, \
           q.cooldown_hours, q.cooldown_anchor, q.last_started_at, \
           f.name AS family_name, \
           f.cooldown_hours AS family_cooldown_hours, \
           f.cooldown_anchor AS family_cooldown_anchor, \
           (SELECT MAX(c.completed_at) FROM session_quest_completions c \
            WHERE c.quest_id = q.id \
              AND NOT EXISTS (SELECT 1 FROM quest_cooldown_resets r \
                              WHERE r.completion_id = c.id)) AS last_completed_at, \
           (SELECT MAX(r.abandoned_at) FROM quest_runs r \
            WHERE r.quest_id = q.id) AS last_abandoned_at, \
           (SELECT MAX(r.abandoned_at) FROM quest_runs r \
            JOIN quests m ON m.id = r.quest_id \
            WHERE m.family_id = q.family_id) AS family_last_abandoned_at, \
           (SELECT MAX(m.last_started_at) FROM quests m \
            WHERE m.family_id = q.family_id) AS family_last_started_at, \
           (SELECT MAX(c.completed_at) \
            FROM session_quest_completions c \
            JOIN quests m ON m.id = c.quest_id \
            WHERE m.family_id = q.family_id \
              AND NOT EXISTS (SELECT 1 FROM quest_cooldown_resets r \
                              WHERE r.completion_id = c.id)) AS family_last_completed_at, \
           EXISTS(SELECT 1 FROM quest_runs r WHERE r.quest_id = q.id \
                  AND r.status = 'in_progress' AND r.hand_in_waiting = 1) \
             AS hand_in_waiting \
    FROM quests q \
    LEFT JOIN quest_families f ON f.id = q.family_id AND f.is_active = 1 \
    WHERE q.is_active = 1 \
    ORDER BY q.created_at ASC, q.id ASC";

/// One quest as the Activities control needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct QuestOffer {
    pub id: i64,
    pub name: String,
    /// The planet the quest is played on: the axis a session's quest
    /// offering follows.
    pub planet: String,
    /// The mission log carries it, or a signal run is open on it: the
    /// administrative fact a chip surfaces regardless of any roster.
    pub in_progress: bool,
    /// A signal-completed quest: a standing, repeatable run that
    /// declaring starts and its signal loot ends.
    pub signal_quest: bool,
    /// A run started by declaring it and completed only through the
    /// user-confirmed raw-clump hand-in flow.
    pub manual_hand_in: bool,
    pub hand_in_waiting: bool,
    pub family_id: Option<i64>,
    /// The active family's name, when the quest belongs to one.
    pub family_name: Option<String>,
    /// When the gate on starting this quest lifts (epoch seconds), the
    /// LATER of its own and its family's cooldown: in game the family is
    /// one slot, so whichever timer runs longer is the one that binds.
    /// None when nothing gates it.
    pub available_from: Option<f64>,
    /// The quest's own gate, kept separate from the family gate so a member
    /// row never offers to reset a family-only cooldown.
    pub own_available_from: Option<f64>,
}

impl super::QuestService {
    /// Every active quest as an offering. One statement, no per-quest
    /// round trips.
    pub async fn quest_offers(&self) -> Result<Vec<QuestOffer>, DbError> {
        read_quest_offers(&self.db).await
    }
}

/// The offerings read against a database handle, so a caller holding
/// only the shared handle (the tracking snapshot) needs no service.
pub async fn read_quest_offers(db: &Db) -> Result<Vec<QuestOffer>, DbError> {
    db.with_reader(|conn| {
        let mut stmt = conn.prepare(OFFER_SELECT)?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let own_anchor = row.get::<_, String>("cooldown_anchor")?;
            let own_instant = match own_anchor.as_str() {
                "pickup" => row.get::<_, Option<f64>>("last_started_at")?,
                _ => completion_anchor_instant(
                    row.get("last_completed_at")?,
                    row.get("last_abandoned_at")?,
                ),
            };
            let family_anchor = row.get::<_, Option<String>>("family_cooldown_anchor")?;
            let family_instant = match family_anchor.as_deref() {
                Some("pickup") => row.get::<_, Option<f64>>("family_last_started_at")?,
                Some(_) => completion_anchor_instant(
                    row.get("family_last_completed_at")?,
                    row.get("family_last_abandoned_at")?,
                ),
                None => None,
            };
            let own_lift = cooldown_lift(own_instant, row.get("cooldown_hours")?);
            let family_lift = cooldown_lift(family_instant, row.get("family_cooldown_hours")?);
            out.push(QuestOffer {
                id: row.get("id")?,
                name: row.get("name")?,
                planet: row.get("planet")?,
                in_progress: row.get::<_, Option<f64>>("started_at")?.is_some(),
                signal_quest: row
                    .get::<_, Option<String>>("signal_loot_item")?
                    .is_some_and(|item| !item.trim().is_empty()),
                manual_hand_in: row.get::<_, String>("completion_mode")? == "manual_hand_in",
                hand_in_waiting: row.get::<_, i64>("hand_in_waiting")? != 0,
                family_id: row.get("family_id")?,
                family_name: row.get("family_name")?,
                available_from: match (own_lift, family_lift) {
                    (Some(own), Some(family)) => Some(own.max(family)),
                    (lift, None) | (None, lift) => lift,
                },
                own_available_from: own_lift,
            });
        }
        Ok(out)
    })
    .await
}
