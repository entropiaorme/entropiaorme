//! Reviewing, after a session, which creatures its kills were detected
//! as.
//!
//! Every answer creature detection gave a kill is kept (see the tracker's
//! `detection`): the creature named and at which grade, the text read
//! (none for the creature being hunted, which rests on earlier kills), and
//! the stamp the kill was recorded with. The review lists them grouped by
//! creature and grade, with how many kills each holds and an example of
//! what was read, the least certain first and the kills given the creature
//! being hunted last, and lets the player settle a group in one action for
//! all of its kills:
//!
//! - name the creature: a listed creature (typed as the list knows it, by
//!   its species and maturity, or by its species alone); a name the player
//!   confirmed before; or a new name, which is kept so the name reader
//!   treats it as known from then on. A name that is a listed creature
//!   misspelt is taken as that listed creature, never kept as a new one:
//!   the reader would drop it anyway, since it would blur the creature it
//!   is close to;
//! - or put back the stamp each kill was recorded with (the typed mob, or
//!   no creature).
//!
//! Kills taking a pinned creature are the player's choice, so the review
//! asks nothing about them, with one exception: a creature on no list
//! pinned from what the screen read carries the screen's spelling, still
//! unconfirmed, so its kills are listed as one group to confirm or correct.
//! (For a pinned kill, putting back the stamp means the one the kill would
//! have had without the pin.)
//!
//! Either way the kills' stamps are then the player's word: the source is
//! `declared` and the detection grade is cleared, while the answer
//! detection gave stays kept beside them with when it was reviewed. A
//! group can be settled again; nothing is ever deleted. The session's
//! summary, days and settled cells are repaired in the same transaction.
//! Only an ended session is reviewed: a running one may still be stamping
//! its kills.

use std::sync::Arc;

use crate::clock::Clock;
use crate::db::{Db, DbError};
use crate::game_data_store::GameDataStore;
use crate::game_ui::field::label_key;
use crate::game_ui::mob_names::{
    numbered_maturity, resolve, split_numbered, KeptCreature, MobCatalogue, MobNameProfile,
    Resolved, Spelling,
};
use crate::mob_watch::CatalogueCell;
use crate::time::naive_to_epoch;
use crate::tracker::{DeclaredMob, MobDetectionGrade, MobStampSource};

/// A creature as a kill's stamp carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedCreature {
    pub name: String,
    pub species: String,
    pub maturity: String,
}

/// Which group of a session's detections: the creature named, at one
/// grade (with the session's key for a creature on no list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionGroupRef {
    pub grade: MobDetectionGrade,
    pub creature: DetectedCreature,
    pub provisional_key: Option<String>,
}

/// One group of a session's detections, as the review lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionGroup {
    pub group: DetectionGroupRef,
    pub kill_count: i64,
    /// The text read most often among the group's kills; none for the
    /// creature being hunted, which no reading of these kills gave.
    pub example_text: Option<String>,
    /// What the group's kills are stamped as now, when they all agree.
    pub current_name: Option<String>,
    /// The player has settled every kill of the group.
    pub reviewed: bool,
}

/// A session's detections as the review lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionReview {
    /// The session has ended, so its detections can be settled.
    pub ended: bool,
    pub groups: Vec<DetectionGroup>,
}

/// What the player settles a group as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewTarget {
    /// The creature named so (see the module doc for how it is read).
    Creature(String),
    /// The declared mob each kill had.
    Declared,
}

/// What a group was settled as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// A creature on the bundled list.
    Listed(DetectedCreature),
    /// A name the player had confirmed before.
    Confirmed(DetectedCreature),
    /// A new name, now kept as confirmed.
    NewName(DetectedCreature),
    /// Each kill's declared mob, as it had it.
    Declared,
}

#[derive(Debug, thiserror::Error)]
pub enum MobReviewError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("{0}")]
    Invalid(String),
    /// What is kept no longer reads as anything this build knows: a fault
    /// of the store, not of the request.
    #[error("{0}")]
    Corrupt(String),
}

/// What settling a group came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettleOutcome {
    Settled(Settled),
    /// The typed name came only loosely near this creature: nothing was
    /// changed; the player can choose it in one action.
    DidYouMean(DetectedCreature),
}

/// The longest name a player may type for a creature.
pub const MAX_NAME_CHARS: usize = 64;

/// The name the summaries read as no creature at all.
const NO_CREATURE: &str = "Unknown";

/// Unicode's format characters (category Cf): invisible marks that would
/// make two names that print alike differ.
fn is_format(c: char) -> bool {
    matches!(
        c as u32,
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

impl From<rusqlite::Error> for MobReviewError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Db(DbError::Sqlite(error))
    }
}

/// The least certain grades first: those are the ones to check (a creature
/// on no list the player pinned has its spelling to confirm, beside the
/// others on no list). The kills given the creature being hunted come after
/// every grade the screen gave, as a quiet account rather than a question.
fn review_order(grade: MobDetectionGrade) -> u8 {
    match grade {
        MobDetectionGrade::ProvisionalLatestRead => 0,
        MobDetectionGrade::LatestRead => 1,
        MobDetectionGrade::ProvisionalGuess => 2,
        MobDetectionGrade::Guess => 3,
        MobDetectionGrade::Provisional => 4,
        MobDetectionGrade::ProvisionalPinned => 5,
        MobDetectionGrade::Named => 6,
        MobDetectionGrade::ProvisionalContinuity => 7,
        MobDetectionGrade::Continuity => 8,
    }
}

/// Where a kept creature came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddedBy {
    /// Confirmed in a session's review; retired when no kill carries it.
    Review,
    /// Entered or edited in the creature list; kept until removed.
    Player,
}

impl AddedBy {
    fn as_str(self) -> &'static str {
        match self {
            AddedBy::Review => "review",
            AddedBy::Player => "player",
        }
    }
}

/// A creature the player keeps for the name reader, as the creature list
/// shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptEntry {
    pub name: String,
    pub species: Option<String>,
    /// The maturity as entered; a number stands for every number.
    pub maturity: Option<String>,
    /// Whether the maturity is a number, read at whatever number a plate
    /// prints.
    pub any_number: bool,
    pub added_by: AddedBy,
}

/// The creatures the player keeps, oldest first.
pub fn kept_creatures(conn: &rusqlite::Connection) -> Result<Vec<KeptCreature>, DbError> {
    Ok(kept_rows(conn)?
        .into_iter()
        .map(|(creature, _)| creature)
        .collect())
}

fn kept_rows(conn: &rusqlite::Connection) -> Result<Vec<(KeptCreature, AddedBy)>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT name, species, maturity, added_by FROM confirmed_mob_names \
         ORDER BY confirmed_at, name",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                KeptCreature {
                    name: row.get(0)?,
                    species: row.get(1)?,
                    maturity: row.get(2)?,
                },
                if row.get::<_, String>(3)? == "player" {
                    AddedBy::Player
                } else {
                    AddedBy::Review
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The catalogue a name read off a plate is matched against: the bundled
/// creatures and every creature the player keeps.
pub async fn build_catalogue(
    db: &Db,
    store: &GameDataStore,
    names: &MobNameProfile,
) -> Result<MobCatalogue, DbError> {
    let kept = db.with_reader(|conn| kept_creatures(conn)).await?;
    Ok(MobCatalogue::from_store(store, kept, names))
}

pub struct MobReviewService {
    db: Db,
    clock: Arc<dyn Clock>,
    catalogue: Arc<CatalogueCell>,
    store: Option<Arc<GameDataStore>>,
    names: Option<MobNameProfile>,
}

impl MobReviewService {
    /// A review over `catalogue` (shared with the watcher, and rebuilt
    /// from `store` when a name is confirmed). Without the name profile no
    /// typed name can be checked, and naming a creature is refused.
    pub fn new(
        db: Db,
        clock: Arc<dyn Clock>,
        catalogue: Arc<CatalogueCell>,
        store: Option<Arc<GameDataStore>>,
        names: Option<MobNameProfile>,
    ) -> Self {
        Self {
            db,
            clock,
            catalogue,
            store,
            names,
        }
    }

    /// Every creature and suggestion detection gave the session's kills,
    /// grouped by creature and grade, the least certain first, then the
    /// largest.
    pub async fn detections(&self, session_id: &str) -> Result<DetectionReview, MobReviewError> {
        let session_id = session_id.to_string();
        self.db
            .with_reader(move |conn| Ok(read_groups(conn, &session_id)))
            .await?
    }

    /// Settle every kill of `group` in the ended session as `target`.
    pub async fn settle(
        &self,
        session_id: &str,
        group: DetectionGroupRef,
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
                let outcome = settle_group(&tx, &session_id, &group, &write, now)?;
                if outcome.is_ok() {
                    tx.commit()?;
                }
                Ok(outcome)
            })
            .await??;
        if names_changed {
            // The settlement stands; a catalogue left as it was only means
            // the watcher matches as before until the next rebuild.
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

    /// Rebuild the shared catalogue with every confirmed name: the next
    /// look matches with it.
    async fn rebuild_catalogue(&self) -> Result<(), MobReviewError> {
        let (Some(store), Some(names)) = (&self.store, &self.names) else {
            return Ok(());
        };
        let catalogue = build_catalogue(&self.db, store, names).await?;
        self.catalogue.replace(catalogue);
        Ok(())
    }

    /// `typed` as a creature's name, its spacing made single, once it can
    /// be one: not empty, nothing invisible, not too long, not the word for
    /// no creature, mostly letters. With the name profile it is read by.
    fn checked_name(&self, typed: &str) -> Result<(String, &MobNameProfile), MobReviewError> {
        let text = typed.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return Err(MobReviewError::Invalid("Type the creature's name".into()));
        }
        if typed.chars().any(|c| c.is_control() || is_format(c)) {
            return Err(MobReviewError::Invalid(
                "A creature's name cannot hold control or invisible characters".into(),
            ));
        }
        if text.chars().count() > MAX_NAME_CHARS {
            return Err(MobReviewError::Invalid(format!(
                "A creature's name is at most {MAX_NAME_CHARS} characters"
            )));
        }
        if text.eq_ignore_ascii_case(NO_CREATURE) {
            return Err(MobReviewError::Invalid(format!(
                "\"{NO_CREATURE}\" means no creature; choose the declared mob instead"
            )));
        }
        let Some(names) = &self.names else {
            return Err(MobReviewError::Invalid(
                "Creature names cannot be checked on this installation".into(),
            ));
        };
        let letters = text.chars().filter(|c| c.is_alphabetic()).count();
        let visible = text.chars().filter(|c| !c.is_whitespace()).count();
        if letters < names.min_letters.max(1)
            || (letters as f64) < names.min_letter_share * visible as f64
        {
            return Err(MobReviewError::Invalid(format!(
                "\"{text}\" does not read as a creature's name"
            )));
        }
        Ok((text, names))
    }

    /// What a typed name is: a listed creature, a confirmed name, or a new
    /// name the reader would keep; or, read only loosely as a creature,
    /// the creature it may have meant. `confirmed` are the names the player
    /// has confirmed so far.
    fn read_typed(
        &self,
        typed: &str,
        kept: Vec<KeptCreature>,
    ) -> Result<SettleOutcome, MobReviewError> {
        let (text, names) = self.checked_name(typed)?;
        let catalogue = self.catalogue.current();
        // A listed species typed without its maturity.
        let key = label_key(&text);
        if let Some(species) = catalogue.mobs().iter().find_map(|mob| {
            let species = mob.species.clone().unwrap_or_else(|| mob.name.clone());
            (label_key(&species) == key).then_some(species)
        }) {
            return Ok(SettleOutcome::Settled(Settled::Listed(stamp_of(
                species,
                String::new(),
            ))));
        }
        // A near miss is the player's typing slip and stands for the
        // creature; a loose match is only a guess, offered back.
        let settle_or_offer =
            |spelling: Spelling, creature: DetectedCreature, settled: Settled| match spelling {
                Spelling::Exact | Spelling::Near => SettleOutcome::Settled(settled),
                Spelling::Loose => SettleOutcome::DidYouMean(creature),
            };
        match resolve(&text, None, &catalogue, names) {
            Resolved::Known {
                name,
                species,
                maturity,
                spelling,
            } => {
                let creature = stamp_of(species.unwrap_or(name), maturity.unwrap_or_default());
                Ok(settle_or_offer(
                    spelling,
                    creature.clone(),
                    Settled::Listed(creature),
                ))
            }
            Resolved::Species {
                species,
                maturity,
                spelling,
                ..
            } => {
                let creature = stamp_of(species, maturity.unwrap_or_default());
                Ok(settle_or_offer(
                    spelling,
                    creature.clone(),
                    Settled::Listed(creature),
                ))
            }
            Resolved::Confirmed { name, spelling } => {
                let creature = stamp_of(name, String::new());
                Ok(settle_or_offer(
                    spelling,
                    creature.clone(),
                    Settled::Confirmed(creature),
                ))
            }
            Resolved::New { .. } => {
                // Kept only if the reader would keep it beside every
                // creature already kept: a name close to a listed creature
                // is dropped from the catalogue. A name ending in a number
                // is kept at every number.
                let creature = match split_numbered(&text, names) {
                    Some((name, maturity)) => KeptCreature {
                        name,
                        species: None,
                        maturity: Some(maturity),
                    },
                    None => KeptCreature::from(text.clone()),
                };
                if !self.kept_reads_as_itself(&creature, kept, &text) {
                    return Err(MobReviewError::Invalid(format!(
                        "\"{text}\" is too close to a creature already known to keep as a new \
                         name; type the known creature as it is named"
                    )));
                }
                Ok(SettleOutcome::Settled(Settled::NewName(stamp_of(
                    creature.name,
                    creature.maturity.unwrap_or_default(),
                ))))
            }
            Resolved::Unclear => Err(MobReviewError::Invalid(format!(
                "\"{text}\" is as close to two listed creatures as to either; type it in full"
            ))),
        }
    }
}

/// A creature as the creature list would keep it, read from what the
/// player typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryCheck {
    /// Ready to keep: the name, and the maturity when one was typed (a
    /// number stands for every number).
    Ready {
        name: String,
        maturity: Option<String>,
        any_number: bool,
    },
    /// The reader already knows it: the creature by its own name (with the
    /// maturity read, when it has one), and as its kills are recorded (its
    /// species and maturity).
    Known {
        name: String,
        maturity: Option<String>,
        creature: DetectedCreature,
    },
    /// It cannot be kept, and why.
    Refused { reason: String },
}

/// The creature list: the creatures the player keeps for the name reader.
impl MobReviewService {
    /// Every kept creature, oldest first.
    pub async fn entries(&self) -> Result<Vec<KeptEntry>, MobReviewError> {
        let rows = self.db.with_reader(|conn| kept_rows(conn)).await?;
        let names = self.names.clone();
        Ok(rows
            .into_iter()
            .map(|(creature, added_by)| KeptEntry {
                any_number: names.as_ref().is_some_and(|names| {
                    creature
                        .maturity
                        .as_deref()
                        .and_then(|m| numbered_maturity(m, names))
                        .is_some()
                }),
                name: creature.name,
                species: creature.species,
                maturity: creature.maturity,
                added_by,
            })
            .collect())
    }

    /// What keeping `typed` (as its plate prints it, with `species` when
    /// given) would come to, in place of the kept creature `replacing`.
    pub async fn check_entry(
        &self,
        typed: &str,
        replacing: Option<&str>,
    ) -> Result<EntryCheck, MobReviewError> {
        let (text, names) = match self.checked_name(typed) {
            Ok(checked) => checked,
            Err(MobReviewError::Invalid(reason)) => return Ok(EntryCheck::Refused { reason }),
            Err(other) => return Err(other),
        };
        let Some(store) = &self.store else {
            return Ok(EntryCheck::Refused {
                reason: "Creature names cannot be checked on this installation".into(),
            });
        };
        let mut kept = self.db.with_reader(|conn| kept_creatures(conn)).await?;
        if let Some(replacing) = replacing {
            kept.retain(|k| !k.name.eq_ignore_ascii_case(replacing));
        }
        let creature = match split_numbered(&text, names) {
            Some((name, maturity)) => KeptCreature {
                name,
                species: None,
                maturity: Some(maturity),
            },
            None => KeptCreature::from(text.clone()),
        };
        let without = MobCatalogue::from_store(store, kept.clone(), names);
        let known = |name: String, species: String, maturity: Option<String>| EntryCheck::Known {
            name,
            creature: stamp_of(species, maturity.clone().unwrap_or_default()),
            maturity,
        };
        match resolve(&text, None, &without, names) {
            Resolved::Known {
                name,
                species,
                maturity,
                spelling: Spelling::Exact | Spelling::Near,
            } => {
                let species = species.unwrap_or_else(|| name.clone());
                return Ok(known(name, species, maturity));
            }
            Resolved::Species {
                name,
                species,
                maturity,
                spelling: Spelling::Exact | Spelling::Near,
            } => return Ok(known(name, species, maturity)),
            Resolved::Confirmed {
                name,
                spelling: Spelling::Exact | Spelling::Near,
            } => return Ok(known(name.clone(), name, None)),
            _ => {}
        }
        if !self.kept_reads_as_itself(&creature, kept, &text) {
            return Ok(EntryCheck::Refused {
                reason: format!(
                    "\"{text}\" is too close to a creature already known: the reader could not \
                     tell the two apart"
                ),
            });
        }
        let any_number = creature
            .maturity
            .as_deref()
            .and_then(|m| numbered_maturity(m, names))
            .is_some();
        Ok(EntryCheck::Ready {
            name: creature.name,
            maturity: creature.maturity,
            any_number,
        })
    }

    /// Keep `typed` (as its plate prints it), with `species` when given, in
    /// place of the kept creature `replacing`; the list as it now stands.
    pub async fn save_entry(
        &self,
        typed: &str,
        species: Option<&str>,
        replacing: Option<&str>,
    ) -> Result<Vec<KeptEntry>, MobReviewError> {
        let (name, maturity) = match self.check_entry(typed, replacing).await? {
            EntryCheck::Ready { name, maturity, .. } => (name, maturity),
            EntryCheck::Known { name, .. } => {
                return Err(MobReviewError::Invalid(format!("Already known as {name}")))
            }
            EntryCheck::Refused { reason } => return Err(MobReviewError::Invalid(reason)),
        };
        let species = species
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty());
        if species
            .as_ref()
            .is_some_and(|s| s.chars().count() > MAX_NAME_CHARS)
        {
            return Err(MobReviewError::Invalid(format!(
                "A species is at most {MAX_NAME_CHARS} characters"
            )));
        }
        let now = naive_to_epoch(self.clock.now());
        let replacing = replacing.map(str::to_string);
        self.db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                if let Some(replacing) = &replacing {
                    tx.execute(
                        "DELETE FROM confirmed_mob_names WHERE name = ?",
                        rusqlite::params![replacing],
                    )?;
                }
                tx.execute(
                    "INSERT INTO confirmed_mob_names (name, species, maturity, confirmed_at, added_by) \
                     VALUES (?1, ?2, ?3, ?4, ?5) \
                     ON CONFLICT(name) DO UPDATE SET species = ?2, maturity = ?3, added_by = ?5",
                    rusqlite::params![name, species, maturity, now, AddedBy::Player.as_str()],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        self.rebuild_catalogue().await?;
        self.entries().await
    }

    /// Stop keeping the creature `name`; kills already given it keep it.
    pub async fn remove_entry(&self, name: &str) -> Result<Vec<KeptEntry>, MobReviewError> {
        let name = name.to_string();
        let removed = self
            .db
            .with_writer(move |conn| {
                Ok(conn.execute(
                    "DELETE FROM confirmed_mob_names WHERE name = ?",
                    rusqlite::params![name],
                )?)
            })
            .await?;
        if removed == 0 {
            return Err(MobReviewError::NotFound("No kept creature has that name"));
        }
        self.rebuild_catalogue().await?;
        self.entries().await
    }

    /// Whether `creature`, kept beside `kept` (without itself), would be read
    /// back as itself from `text`, as its plate prints it: a creature too
    /// close to one already known would be dropped from the catalogue.
    fn kept_reads_as_itself(
        &self,
        creature: &KeptCreature,
        kept: Vec<KeptCreature>,
        text: &str,
    ) -> bool {
        let (Some(store), Some(names)) = (&self.store, &self.names) else {
            return false;
        };
        let mut probe = kept;
        probe.retain(|k| !k.name.eq_ignore_ascii_case(&creature.name));
        probe.push(creature.clone());
        let catalogue = MobCatalogue::from_store(store, probe, names);
        match resolve(text, None, &catalogue, names) {
            Resolved::Confirmed {
                name,
                spelling: Spelling::Exact,
            } => name.eq_ignore_ascii_case(&creature.name),
            Resolved::Known {
                name,
                maturity,
                spelling: Spelling::Exact,
                ..
            } => {
                name.eq_ignore_ascii_case(&creature.name)
                    && maturity.is_some() == creature.maturity.is_some()
            }
            _ => false,
        }
    }
}

/// The stamp of a creature named by its species and maturity, as the
/// declaration would give it.
fn stamp_of(species: String, maturity: String) -> DetectedCreature {
    let declared = DeclaredMob::from_parts(species, maturity);
    DetectedCreature {
        name: declared.name,
        species: declared.species,
        maturity: declared.maturity,
    }
}

/// One kept answer with its kill's stamp now.
struct Row {
    group: DetectionGroupRef,
    read_text: Option<String>,
    current: String,
    reviewed: bool,
}

fn read_groups(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<DetectionReview, MobReviewError> {
    use rusqlite::OptionalExtension as _;
    let ended: Option<bool> = conn
        .query_row(
            "SELECT ended_at IS NOT NULL AND is_active = 0 FROM tracking_sessions WHERE id = ?",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(ended) = ended else {
        return Err(MobReviewError::NotFound("Session not found"));
    };
    // The screen's answers and the kills given a creature nothing read for
    // them gave (the creature being hunted, or an unlisted pin), in the
    // order the kills were recorded.
    let mut stmt = conn.prepare(
        "SELECT d.grade, d.mob_name, d.mob_species, d.mob_maturity, d.provisional_key, \
                d.read_text, COALESCE(k.mob_name, ''), d.reviewed_at IS NOT NULL, \
                k.timestamp, d.kill_id \
         FROM kill_mob_detections d JOIN kills k ON k.id = d.kill_id \
         WHERE d.session_id = ?1 \
         UNION ALL \
         SELECT c.grade, c.mob_name, c.mob_species, c.mob_maturity, c.provisional_key, NULL, \
                COALESCE(k.mob_name, ''), c.reviewed_at IS NOT NULL, k.timestamp, c.kill_id \
         FROM kill_mob_carried c JOIN kills k ON k.id = c.kill_id \
         WHERE c.session_id = ?1 \
         ORDER BY 9, 10",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                DetectedCreature {
                    name: row.get(1)?,
                    species: row.get(2)?,
                    maturity: row.get(3)?,
                },
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, bool>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut parsed = Vec::with_capacity(rows.len());
    for (grade, creature, provisional_key, read_text, current, reviewed) in rows {
        let grade = MobDetectionGrade::parse(&grade).ok_or_else(|| {
            MobReviewError::Corrupt(format!("a kept detection has an unknown grade {grade:?}"))
        })?;
        // A reading belongs to exactly the grades that rest on the kill's
        // own fight.
        if read_text.is_some() == grade.carried() {
            return Err(MobReviewError::Corrupt(format!(
                "a kept detection at grade {grade:?} disagrees with its reading"
            )));
        }
        parsed.push(Row {
            group: DetectionGroupRef {
                grade,
                creature,
                provisional_key,
            },
            read_text,
            current,
            reviewed,
        });
    }
    let mut groups: Vec<(DetectionGroup, Vec<(String, i64)>)> = Vec::new();
    for row in parsed {
        let at = match groups.iter().position(|(g, _)| g.group == row.group) {
            Some(at) => at,
            None => {
                groups.push((
                    DetectionGroup {
                        group: row.group.clone(),
                        kill_count: 0,
                        example_text: None,
                        current_name: Some(row.current.clone()),
                        reviewed: true,
                    },
                    Vec::new(),
                ));
                groups.len() - 1
            }
        };
        let (group, texts) = &mut groups[at];
        group.kill_count += 1;
        group.reviewed &= row.reviewed;
        if group.current_name.as_deref() != Some(row.current.as_str()) {
            group.current_name = None;
        }
        if let Some(read_text) = row.read_text {
            match texts.iter_mut().find(|(text, _)| *text == read_text) {
                Some(entry) => entry.1 += 1,
                None => texts.push((read_text, 1)),
            }
        }
    }
    let mut out: Vec<DetectionGroup> = groups
        .into_iter()
        .map(|(mut group, texts)| {
            let most = texts.iter().map(|t| t.1).max().unwrap_or(0);
            group.example_text = texts.into_iter().find(|t| t.1 == most).map(|t| t.0);
            group
        })
        .collect();
    out.sort_by_key(|group| {
        (
            review_order(group.group.grade),
            std::cmp::Reverse(group.kill_count),
        )
    });
    Ok(DetectionReview { ended, groups: out })
}

/// Where the screen's answers are kept.
const DETECTIONS_TABLE: &str = "kill_mob_detections";
/// Where the kills given a creature nothing read for them gave are kept.
const CARRIED_TABLE: &str = "kill_mob_carried";

/// The refusal for a settlement that would move kills between creatures
/// after some of the session's hunted loot has left stock.
const LOOT_ALREADY_MOVED: &str = "Some of this session's loot has already been sold or \
converted, so its creature can no longer be changed here";

/// Settle `group`'s kills as `settled`; answers whether the confirmed names
/// changed (one kept, or one retired), so the catalogue is rebuilt.
fn settle_group(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    group: &DetectionGroupRef,
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
    // The group's kills, from the table its grade is kept in: the screen's
    // answers, or the kills given a creature nothing read for them gave.
    let table = if group.grade.carried() {
        CARRIED_TABLE
    } else {
        DETECTIONS_TABLE
    };
    let sql = format!(
        "SELECT d.kill_id, d.declared_name, d.declared_species, d.declared_maturity, \
                d.declared_source, COALESCE(k.mob_name, ''), COALESCE(k.mob_species, '') \
         FROM {table} d JOIN kills k ON k.id = d.kill_id \
         WHERE d.session_id = ? AND d.grade = ? AND d.mob_name = ? AND d.mob_species = ? \
           AND d.mob_maturity = ? AND d.provisional_key IS ?"
    );
    let kind = group.grade.as_str();
    let mut stmt = tx.prepare(&sql)?;
    let kills = stmt
        .query_map(
            rusqlite::params![
                session_id,
                kind,
                group.creature.name,
                group.creature.species,
                group.creature.maturity,
                group.provisional_key,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    DetectedCreature {
                        name: row.get(1)?,
                        species: row.get(2)?,
                        maturity: row.get(3)?,
                    },
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    if kills.is_empty() {
        return Ok(Err(MobReviewError::NotFound(
            "No kills of this session were detected as that creature",
        )));
    }
    let stamp_for = |declared: &DetectedCreature| -> DetectedCreature {
        match settled {
            Settled::Listed(c) | Settled::Confirmed(c) | Settled::NewName(c) => c.clone(),
            Settled::Declared => declared.clone(),
        }
    };
    // Stock outflows keep the species their loot was allocated under; there
    // is no correction that moves them, so a settlement that would change a
    // kill's species is refused once any of the session's hunted loot has
    // left stock.
    let moves_species = kills
        .iter()
        .any(|(_, declared, _, _, species)| stamp_for(declared).species != *species);
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
    // Confirmed names the group's kills carry now: retired below when no
    // kill carries them any more.
    let mut carried: Vec<String> = Vec::new();
    for (_, _, _, current, species) in &kills {
        let kept: Option<String> = tx
            .query_row(
                "SELECT name FROM confirmed_mob_names WHERE name = ?1 OR name = ?2 LIMIT 1",
                rusqlite::params![current, species],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(name) = kept {
            if !carried.contains(&name) {
                carried.push(name);
            }
        }
    }
    let kind = match settled {
        Settled::Listed(_) => "listed",
        Settled::Confirmed(_) => "confirmed",
        Settled::NewName(_) => "new_name",
        Settled::Declared => "declared",
    };
    for (kill_id, declared, declared_source, _, _) in &kills {
        let stamp = stamp_for(declared);
        let source = match settled {
            Settled::Declared => declared_source.as_deref(),
            _ => Some(MobStampSource::Declared.as_str()),
        };
        // The older rename's restore point is cleared: the settlement row
        // keeps both what detection said and the declared stamp.
        tx.execute(
            "UPDATE kills SET mob_name = ?, mob_species = ?, mob_maturity = ?, \
             mob_stamp_source = ?, mob_detection_grade = NULL, mob_read_text = NULL, \
             original_mob_name = NULL WHERE id = ?",
            rusqlite::params![stamp.name, stamp.species, stamp.maturity, source, kill_id],
        )?;
        tx.execute(
            &format!(
                "UPDATE {table} SET reviewed_at = ?, settled_kind = ?, \
                 settled_name = ?, settled_species = ?, settled_maturity = ? WHERE kill_id = ?"
            ),
            rusqlite::params![
                now,
                kind,
                stamp.name,
                stamp.species,
                stamp.maturity,
                kill_id
            ],
        )?;
    }
    let mut names_changed = false;
    for name in carried {
        let retired = tx.execute(
            "DELETE FROM confirmed_mob_names WHERE name = ?1 AND added_by = 'review' \
             AND NOT EXISTS (SELECT 1 FROM kills \
                 WHERE mob_name = ?1 COLLATE NOCASE OR mob_species = ?1 COLLATE NOCASE)",
            rusqlite::params![name],
        )?;
        names_changed |= retired > 0;
    }
    if let Settled::NewName(creature) = settled {
        names_changed |= tx.execute(
            "INSERT OR IGNORE INTO confirmed_mob_names (name, maturity, confirmed_at) \
             VALUES (?, NULLIF(?, ''), ?)",
            rusqlite::params![creature.species, creature.maturity, now],
        )? > 0;
    }
    crate::session_summary::write_session_summary(tx, session_id)?;
    crate::daily_rollup::refresh_session_days(tx, session_id)?;
    crate::session_rollup::recompute_session(tx, session_id)?;
    Ok(Ok(names_changed))
}

#[cfg(test)]
mod tests;
