//! The review as the player reads it: one line for each mob the session's
//! kills were, however sure the screen was of each kill, and one for the
//! kills it could not name.
//!
//! The answers are kept by how sure the screen was (see the parent module);
//! that is how they are settled, not something the player needs to weigh. A
//! mob's line gathers every answer that named it: how many kills, and how
//! many of them the screen was less than sure of. Settling the line settles
//! all of them in one action.
//!
//! The kills the screen answered without naming anything have no answer to
//! settle. Naming them here records the stamp each had beside what it was
//! named (`kill_mob_assigned`), so the stamp can be put back.
//!
//! Also here: the labels the player gives mobs (see [`crate::mob_display`]).

use super::*;
use crate::mob_display::{self, LabelSource, MAX_LABEL_CHARS};

/// Which line of a session's review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewedRef {
    /// The mob detection named, as its kills were recorded (its name and
    /// maturity), with the session's key for a mob on no list.
    Mob {
        species: String,
        maturity: String,
        provisional_key: Option<String>,
    },
    /// The kills the screen could not name.
    Unnamed,
}

/// One line of a session's review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedMob {
    pub mob: ReviewedRef,
    /// The mob as shown; none for the kills the screen could not name.
    pub name: Option<String>,
    pub kill_count: i64,
    /// How many of the kills the screen was less than sure of: guessed,
    /// suggested, or given the mob being hunted.
    pub unsure_count: i64,
    /// The text read most often for the mob's kills, when any was read.
    pub example_text: Option<String>,
    /// What the kills are recorded as now, when they all agree.
    pub current_name: Option<String>,
    /// The mob is on no list: its spelling is the screen's, to be confirmed.
    pub unlisted: bool,
    /// The player has settled every kill of the line.
    pub reviewed: bool,
}

/// A session's review, a line for each mob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobReview {
    pub ended: bool,
    pub mobs: Vec<ReviewedMob>,
}

/// A label a mob is shown by, as the mob list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptLabel {
    pub species: String,
    pub maturity: String,
    /// The mob as the game prints it, without any label.
    pub printed: String,
    pub label: String,
    /// The player chose it; otherwise it is how the mob's plate was read
    /// to print it.
    pub by_player: bool,
}

fn key_of(group: &DetectionGroupRef) -> ReviewedRef {
    ReviewedRef::Mob {
        species: group.creature.species.clone(),
        maturity: group.creature.maturity.clone(),
        provisional_key: group.provisional_key.clone(),
    }
}

/// Whether the screen was sure of a kill at `grade`.
fn sure(grade: MobDetectionGrade) -> bool {
    matches!(
        grade,
        MobDetectionGrade::Named
            | MobDetectionGrade::Provisional
            | MobDetectionGrade::ProvisionalPinned
    )
}

/// The kills of `session_id` the screen answered without naming anything:
/// each with its stamp now, and the stamp it had before the player named it
/// here, when they did.
struct UnnamedKill {
    kill_id: String,
    current: DetectedCreature,
    source: Option<String>,
    /// The stamp kept from before, with its source and what it was settled
    /// as.
    kept: Option<(DetectedCreature, Option<String>, String)>,
}

fn unnamed_kills(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<UnnamedKill>, DbError> {
    let mut statement = conn.prepare(
        "SELECT k.id, COALESCE(k.mob_name, ''), COALESCE(k.mob_species, ''), \
                COALESCE(k.mob_maturity, ''), k.mob_stamp_source, \
                a.declared_name, a.declared_species, a.declared_maturity, a.declared_source, \
                a.settled_kind \
         FROM kills k \
         JOIN kill_mob_traces t ON t.kill_id = k.id \
         LEFT JOIN kill_mob_assigned a ON a.kill_id = k.id \
         WHERE k.session_id = ?1 \
           AND NOT EXISTS (SELECT 1 FROM kill_mob_detections d WHERE d.kill_id = k.id) \
           AND NOT EXISTS (SELECT 1 FROM kill_mob_carried c WHERE c.kill_id = k.id) \
         ORDER BY k.timestamp, k.id",
    )?;
    let rows = statement.query_map(rusqlite::params![session_id], |row| {
        let kept = match row.get::<_, Option<String>>(5)? {
            Some(name) => Some((
                DetectedCreature {
                    name,
                    species: row.get(6)?,
                    maturity: row.get(7)?,
                },
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
            )),
            None => None,
        };
        Ok(UnnamedKill {
            kill_id: row.get(0)?,
            current: DetectedCreature {
                name: row.get(1)?,
                species: row.get(2)?,
                maturity: row.get(3)?,
            },
            source: row.get(4)?,
            kept,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The review of `session_id`, a line for each mob: those still asking for
/// the player first (a mob on no list, unconfirmed), then the most killed,
/// and the kills the screen could not name last.
fn read_mobs(conn: &rusqlite::Connection, session_id: &str) -> Result<MobReview, MobReviewError> {
    let review = read_groups(conn, session_id)?;
    let mut mobs: Vec<(ReviewedMob, i64, i64)> = Vec::new();
    for group in review.groups {
        let key = key_of(&group.group);
        let at = match mobs.iter().position(|(mob, _, _)| mob.mob == key) {
            Some(at) => at,
            None => {
                mobs.push((
                    ReviewedMob {
                        mob: key,
                        name: Some(group.group.creature.name.clone()),
                        kill_count: 0,
                        unsure_count: 0,
                        example_text: None,
                        current_name: group.current_name.clone(),
                        unlisted: false,
                        reviewed: true,
                    },
                    0,
                    0,
                ));
                mobs.len() - 1
            }
        };
        let (mob, most_kills, most_read) = &mut mobs[at];
        if mob.kill_count > 0 && mob.current_name != group.current_name {
            mob.current_name = None;
        }
        mob.kill_count += group.kill_count;
        if !sure(group.group.grade) {
            mob.unsure_count += group.kill_count;
        }
        mob.unlisted |= group.group.grade.provisional();
        mob.reviewed &= group.reviewed;
        // Shown by the name most of its kills were given, with the text
        // most of them were read as.
        if group.kill_count > *most_kills {
            *most_kills = group.kill_count;
            mob.name = Some(group.group.creature.name.clone());
        }
        if group.example_text.is_some() && group.kill_count > *most_read {
            *most_read = group.kill_count;
            mob.example_text = group.example_text;
        }
    }
    let mut mobs: Vec<ReviewedMob> = mobs.into_iter().map(|(mob, _, _)| mob).collect();
    mobs.sort_by_key(|mob| {
        (
            !mob.unlisted || mob.reviewed,
            std::cmp::Reverse(mob.kill_count),
        )
    });
    let unnamed = unnamed_kills(conn, session_id)?;
    if let Some(first) = unnamed.first() {
        let agreed = unnamed
            .iter()
            .all(|kill| kill.current.name == first.current.name);
        let named = !first.current.species.is_empty();
        mobs.push(ReviewedMob {
            mob: ReviewedRef::Unnamed,
            name: None,
            kill_count: unnamed.len() as i64,
            unsure_count: 0,
            example_text: None,
            current_name: (agreed && named).then(|| first.current.name.clone()),
            unlisted: false,
            reviewed: unnamed
                .iter()
                .all(|kill| kill.kept.as_ref().is_some_and(|kept| kept.2 != "declared")),
        });
    }
    Ok(MobReview {
        ended: review.ended,
        mobs,
    })
}

/// Settle the kills the screen could not name as `settled`; answers whether
/// the confirmed names changed.
fn settle_unnamed(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    settled: &Settled,
    now: f64,
) -> Result<Result<bool, MobReviewError>, DbError> {
    use rusqlite::OptionalExtension as _;
    let active: Option<i64> = tx
        .query_row(
            "SELECT is_active FROM tracking_sessions WHERE id = ?",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .optional()?;
    match active {
        None => return Ok(Err(MobReviewError::NotFound("Session not found"))),
        Some(0) => {}
        Some(_) => {
            return Ok(Err(MobReviewError::Conflict(
                "Session record edits are only available after the session has ended",
            )))
        }
    }
    let kills = unnamed_kills(tx, session_id)?;
    if kills.is_empty() {
        return Ok(Err(MobReviewError::NotFound(
            "Every kill of this session was given a mob",
        )));
    }
    // The stamp each kill had before the player named it here.
    let before = |kill: &UnnamedKill| -> (DetectedCreature, Option<String>) {
        match &kill.kept {
            Some((declared, source, _)) => (declared.clone(), source.clone()),
            None => (kill.current.clone(), kill.source.clone()),
        }
    };
    let stamp_for = |kill: &UnnamedKill| -> DetectedCreature {
        match settled {
            Settled::Listed(c) | Settled::Confirmed(c) | Settled::NewName(c) => c.clone(),
            Settled::Declared => before(kill).0,
        }
    };
    let moves_species = kills
        .iter()
        .any(|kill| stamp_for(kill).species != kill.current.species);
    if moves_species {
        let moved: bool = tx.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM stock_movements \
                 WHERE source_kind = 'hunt' AND {} = ?)",
                crate::session_scope::STOCK_MOVEMENT_SESSION
            ),
            rusqlite::params![session_id],
            |row| row.get(0),
        )?;
        if moved {
            return Ok(Err(MobReviewError::Conflict(LOOT_ALREADY_MOVED)));
        }
    }
    let mut carried: Vec<String> = Vec::new();
    for kill in &kills {
        note_kept_name(tx, &kill.current.name, &kill.current.species, &mut carried)?;
    }
    let kind = match settled {
        Settled::Listed(_) => "listed",
        Settled::Confirmed(_) => "confirmed",
        Settled::NewName(_) => "new_name",
        Settled::Declared => "declared",
    };
    for kill in &kills {
        let stamp = stamp_for(kill);
        let (declared, declared_source) = before(kill);
        let source = match settled {
            Settled::Declared => declared_source.clone(),
            _ => Some(MobStampSource::Declared.as_str().to_string()),
        };
        tx.execute(
            "UPDATE kills SET mob_name = ?, mob_species = ?, mob_maturity = ?, \
             mob_stamp_source = ?, original_mob_name = NULL WHERE id = ?",
            rusqlite::params![
                stamp.name,
                stamp.species,
                stamp.maturity,
                source,
                kill.kill_id
            ],
        )?;
        tx.execute(
            "INSERT INTO kill_mob_assigned \
             (kill_id, session_id, declared_name, declared_species, declared_maturity, \
              declared_source, reviewed_at, settled_kind, settled_name, settled_species, \
              settled_maturity) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
             ON CONFLICT(kill_id) DO UPDATE SET \
                 reviewed_at = excluded.reviewed_at, settled_kind = excluded.settled_kind, \
                 settled_name = excluded.settled_name, \
                 settled_species = excluded.settled_species, \
                 settled_maturity = excluded.settled_maturity",
            rusqlite::params![
                kill.kill_id,
                session_id,
                declared.name,
                declared.species,
                declared.maturity,
                declared_source,
                now,
                kind,
                stamp.name,
                stamp.species,
                stamp.maturity,
            ],
        )?;
    }
    finish_settlement(tx, session_id, carried, settled, now).map(Ok)
}

impl MobReviewService {
    /// The session's review, a line for each mob.
    pub async fn mobs(&self, session_id: &str) -> Result<MobReview, MobReviewError> {
        let session_id = session_id.to_string();
        self.db
            .with_reader(move |conn| Ok(read_mobs(conn, &session_id)))
            .await?
    }

    /// Settle every kill of the line `mob` of the ended session as `target`.
    pub async fn settle_mob(
        &self,
        session_id: &str,
        mob: ReviewedRef,
        target: ReviewTarget,
    ) -> Result<SettleOutcome, MobReviewError> {
        let settled = match &target {
            ReviewTarget::Declared => Settled::Declared,
            ReviewTarget::Creature(typed) => {
                let kept = self.db.with_reader(|conn| kept_creatures(conn)).await?;
                match self.read_typed(typed, kept)? {
                    SettleOutcome::Settled(settled) => settled,
                    suggestion @ SettleOutcome::DidYouMean(_) => return Ok(suggestion),
                }
            }
        };
        let now = naive_to_epoch(self.clock.now());
        let session_id = session_id.to_string();
        let write = settled.clone();
        let names_changed = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                let outcome = match &mob {
                    ReviewedRef::Unnamed => settle_unnamed(&tx, &session_id, &write, now)?,
                    ReviewedRef::Mob { .. } => {
                        // Every answer that named the mob, at whatever grade.
                        let groups: Vec<DetectionGroupRef> = match read_groups(&tx, &session_id) {
                            Ok(review) => review
                                .groups
                                .into_iter()
                                .map(|group| group.group)
                                .filter(|group| key_of(group) == mob)
                                .collect(),
                            Err(MobReviewError::Db(error)) => return Err(error),
                            Err(other) => return Ok(Err(other)),
                        };
                        if groups.is_empty() {
                            return Ok(Err(MobReviewError::NotFound(
                                "No kills of this session were detected as that mob",
                            )));
                        }
                        let mut changed = Ok(false);
                        for group in &groups {
                            match settle_group(&tx, &session_id, group, &write, now)? {
                                Ok(names) => changed = changed.map(|c| c || names),
                                Err(refused) => {
                                    changed = Err(refused);
                                    break;
                                }
                            }
                        }
                        changed
                    }
                };
                if outcome.is_ok() {
                    tx.commit()?;
                }
                Ok(outcome)
            })
            .await??;
        if names_changed {
            if let Err(error) = self.rebuild_catalogue().await {
                tracing::warn!(
                    target: "eo::mob_review",
                    %error,
                    "confirmed creature names changed, but the name catalogue was not rebuilt"
                );
            }
        }
        Ok(SettleOutcome::Settled(settled))
    }

    /// Every label a mob is shown by, by mob.
    pub async fn labels(&self) -> Result<Vec<KeptLabel>, MobReviewError> {
        Ok(self.db.with_reader(|conn| kept_labels(conn)).await?)
    }

    /// Show the mob typed as `mob` (as the game prints it, or as it is
    /// labelled now) as `label` from now on and everywhere it is recorded.
    /// The mob must be one the reader knows; the label changes only what is
    /// shown for it.
    pub async fn set_label(
        &self,
        mob: &str,
        label: &str,
    ) -> Result<Vec<KeptLabel>, MobReviewError> {
        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
        if label.is_empty() {
            return Err(MobReviewError::Invalid(
                "Type what to show for the mob".into(),
            ));
        }
        if label.chars().any(|c| c.is_control() || is_format(c)) {
            return Err(MobReviewError::Invalid(
                "A mob's label cannot hold control or invisible characters".into(),
            ));
        }
        if label.chars().count() > MAX_LABEL_CHARS {
            return Err(MobReviewError::Invalid(format!(
                "A mob's label is at most {MAX_LABEL_CHARS} characters"
            )));
        }
        let (species, maturity) = self.known_mob(mob).await?;
        let now = naive_to_epoch(self.clock.now());
        self.write_labels(move |tx| {
            mob_display::set_label(tx, &species, &maturity, &label, LabelSource::Player, now)
        })
        .await
    }

    /// Show the mob recorded as `species` at `maturity` as the game prints
    /// it again.
    pub async fn clear_label(
        &self,
        species: &str,
        maturity: &str,
    ) -> Result<Vec<KeptLabel>, MobReviewError> {
        let (species, maturity) = (species.to_string(), maturity.to_string());
        self.write_labels(move |tx| mob_display::clear_label(tx, &species, &maturity))
            .await
    }

    /// Change the labels by `write`, and bring what the tracker names kills
    /// by into step; the labels as they now stand.
    async fn write_labels(
        &self,
        write: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<bool, DbError> + Send + 'static,
    ) -> Result<Vec<KeptLabel>, MobReviewError> {
        let (held, kept) = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                write(&tx)?;
                let held = mob_display::labels(&tx)?;
                let kept = kept_labels(&tx)?;
                tx.commit()?;
                Ok((held, kept))
            })
            .await?;
        self.labels.replace(held);
        Ok(kept)
    }

    /// The mob typed as `mob`, as its kills are recorded: its name and
    /// maturity. It is read as a label already given, then as the reader
    /// reads a name off a plate.
    async fn known_mob(&self, mob: &str) -> Result<(String, String), MobReviewError> {
        let (text, names) = self.checked_name(mob)?;
        let labelled = self
            .db
            .with_reader(|conn| mob_display::labels(conn))
            .await?;
        let key = label_key(&text);
        if let Some(held) = labelled
            .into_iter()
            .find(|held| label_key(&held.label) == key)
        {
            return Ok((held.species, held.maturity));
        }
        let unknown = || {
            MobReviewError::Invalid(format!(
                "\"{text}\" is not a mob the app knows; type it as the game shows it"
            ))
        };
        match resolve(&text, None, &self.catalogue.current(), names) {
            Resolved::Known {
                name,
                maturity,
                spelling: Spelling::Exact | Spelling::Near,
                ..
            }
            | Resolved::Species {
                name,
                maturity,
                spelling: Spelling::Exact | Spelling::Near,
                ..
            } => Ok((
                MobCatalogue::printed_name(&name).to_string(),
                maturity.unwrap_or_default(),
            )),
            Resolved::Confirmed {
                name,
                spelling: Spelling::Exact | Spelling::Near,
            } => Ok((name, String::new())),
            _ => Err(unknown()),
        }
    }
}

fn kept_labels(conn: &rusqlite::Connection) -> Result<Vec<KeptLabel>, DbError> {
    let mut statement = conn.prepare(
        "SELECT species, maturity, label, source FROM mob_display_names \
         ORDER BY species, maturity",
    )?;
    let rows = statement.query_map([], |row| {
        let species: String = row.get(0)?;
        let maturity: String = row.get(1)?;
        Ok(KeptLabel {
            printed: mob_display::printed(&species, &maturity),
            species,
            maturity,
            label: row.get(2)?,
            by_player: row.get::<_, String>(3)? == "player",
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
