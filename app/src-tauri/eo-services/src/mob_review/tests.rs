use super::*;
use crate::clock::MockClock;
use crate::game_ui::test_support::bundled_profile;
use crate::tracker::{write_detection, KillDetection, KillReading, StampedMob};

struct Rig {
    _dir: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
    db: Db,
    review: MobReviewService,
    catalogue: Arc<CatalogueCell>,
}

const MOBS: &str = r#"[
    {"id": 1, "name": "Atrox", "species": {"name": "Atrox"},
     "maturities": [{"name": "Young", "level": 5}, {"name": "Old", "level": 12}]},
    {"id": 2, "name": "Daikiba", "species": {"name": "Daikiba"},
     "maturities": [{"name": "Mature", "level": 9}]}
]"#;

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = dir.path().join("snapshot");
    std::fs::create_dir_all(&snapshot).unwrap();
    std::fs::write(snapshot.join("mobs.json"), MOBS).unwrap();
    let store = Arc::new(GameDataStore::new(&snapshot).unwrap());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let db = runtime
        .block_on(Db::open(&dir.path().join("entropia_orme.db")))
        .unwrap();
    let names = bundled_profile().mob_names;
    let catalogue = Arc::new(CatalogueCell::new(
        runtime
            .block_on(build_catalogue(&db, &store, &names))
            .unwrap(),
    ));
    let review = MobReviewService::new(
        db.clone(),
        Arc::new(MockClock::new(None, 0.0)),
        catalogue.clone(),
        Some(store),
        Some(names),
    );
    Rig {
        _dir: dir,
        runtime,
        db,
        review,
        catalogue,
    }
}

fn stamped(
    name: &str,
    species: &str,
    maturity: &str,
    source: Option<MobStampSource>,
) -> StampedMob {
    StampedMob {
        name: name.into(),
        species: species.into(),
        maturity: maturity.into(),
        source,
    }
}

fn detected(name: &str, species: &str, maturity: &str) -> StampedMob {
    stamped(name, species, maturity, Some(MobStampSource::Detected))
}

fn declared() -> StampedMob {
    stamped(
        "Young Atrox",
        "Atrox",
        "Young",
        Some(MobStampSource::Declared),
    )
}

impl Rig {
    /// An ended session (or a running one) with no kills yet.
    fn session(&self, id: &str, active: bool) {
        let id = id.to_string();
        self.runtime
            .block_on(self.db.with_writer(move |conn| {
                conn.execute(
                    "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, heal_cost) \
                     VALUES (?, 1000.0, ?, ?, 1.0)",
                    rusqlite::params![id, (!active).then_some(2000.0), i64::from(active)],
                )?;
                Ok(())
            }))
            .unwrap();
    }

    /// A kill recorded with `declared`, then given detection's answer.
    fn kill(
        &self,
        session: &str,
        kill: &str,
        (grade, creature, key, text): (MobDetectionGrade, StampedMob, Option<&str>, &str),
        declared: StampedMob,
    ) {
        let answer = KillDetection {
            kill_id: kill.into(),
            session_id: session.into(),
            grade,
            creature,
            provisional_key: key.map(str::to_string),
            read: (!grade.carried()).then(|| KillReading {
                text: text.into(),
                at: 1500.0,
            }),
            declared: declared.clone(),
        };
        self.runtime
            .block_on(self.db.with_writer(move |conn| {
                let tx = conn.transaction()?;
                tx.execute(
                    "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
                     mob_stamp_source, timestamp, cost_ped, loot_total_ped) \
                     VALUES (?, ?, ?, ?, ?, ?, 1500.0, 0.5, 0.4)",
                    rusqlite::params![
                        answer.kill_id,
                        answer.session_id,
                        declared.name,
                        declared.species,
                        declared.maturity,
                        declared.source.map(MobStampSource::as_str),
                    ],
                )?;
                write_detection(&tx, &answer)?;
                tx.commit()?;
                Ok(())
            }))
            .unwrap();
    }

    fn stamp(&self, kill: &str) -> (String, String, String, Option<String>, Option<String>) {
        let kill = kill.to_string();
        self.runtime
            .block_on(self.db.with_reader(move |conn| {
                Ok(conn.query_row(
                    "SELECT mob_name, mob_species, mob_maturity, mob_stamp_source, \
                     mob_detection_grade FROM kills WHERE id = ?",
                    rusqlite::params![kill],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )?)
            }))
            .unwrap()
    }

    fn groups(&self, session: &str) -> Vec<DetectionGroup> {
        self.runtime
            .block_on(self.review.detections(session))
            .unwrap()
            .groups
    }

    fn settle(
        &self,
        session: &str,
        group: &DetectionGroupRef,
        target: ReviewTarget,
    ) -> Result<Settled, MobReviewError> {
        match self.offer(session, group, target)? {
            SettleOutcome::Settled(settled) => Ok(settled),
            SettleOutcome::DidYouMean(creature) => panic!("offered {creature:?} instead"),
        }
    }

    fn offer(
        &self,
        session: &str,
        group: &DetectionGroupRef,
        target: ReviewTarget,
    ) -> Result<SettleOutcome, MobReviewError> {
        self.runtime
            .block_on(self.review.settle(session, group.clone(), target))
    }

    fn scalar<T: rusqlite::types::FromSql + Send + 'static>(&self, sql: &'static str) -> T {
        self.runtime
            .block_on(
                self.db
                    .with_reader(move |conn| Ok(conn.query_row(sql, [], |row| row.get(0))?)),
            )
            .unwrap()
    }

    fn execute(&self, sql: &'static str) {
        self.runtime
            .block_on(self.db.with_writer(move |conn| {
                conn.execute_batch(sql)?;
                Ok(())
            }))
            .unwrap();
    }

    fn confirmed(&self) -> Vec<String> {
        self.runtime
            .block_on(self.db.with_reader(|conn| confirmed_names(conn)))
            .unwrap()
    }
}

fn some(text: &str) -> Option<String> {
    Some(text.to_string())
}

/// A hunt with a provisional creature (two kills), a named one, and a
/// suggestion the kill did not take.
fn hunt(rig: &Rig) {
    rig.session("s1", false);
    let foo = detected("Gloomrat", "Gloomrat", "");
    rig.kill(
        "s1",
        "k1",
        (
            MobDetectionGrade::Provisional,
            foo.clone(),
            Some("GLOOMRAT L4 #0"),
            "Gloomrat",
        ),
        declared(),
    );
    rig.kill(
        "s1",
        "k2",
        (
            MobDetectionGrade::Provisional,
            foo,
            Some("GLOOMRAT L4 #0"),
            "Gloomrot",
        ),
        declared(),
    );
    rig.kill(
        "s1",
        "k3",
        (
            MobDetectionGrade::Named,
            detected("Old Atrox", "Atrox", "Old"),
            None,
            "Atrox Old",
        ),
        declared(),
    );
    rig.kill(
        "s1",
        "k4",
        (
            MobDetectionGrade::LatestRead,
            detected("Mature Daikiba", "Daikiba", "Mature"),
            None,
            "Daikiba Mature",
        ),
        stamped("Unknown", "", "", None),
    );
}

#[test]
fn the_review_groups_by_creature_and_grade_the_least_certain_first() {
    let rig = rig();
    hunt(&rig);
    let groups = rig.groups("s1");
    let listed: Vec<(MobDetectionGrade, &str, i64, &str, Option<&str>)> = groups
        .iter()
        .map(|g| {
            (
                g.group.grade,
                g.group.creature.name.as_str(),
                g.kill_count,
                g.example_text.as_deref().unwrap_or_default(),
                g.current_name.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (
                MobDetectionGrade::LatestRead,
                "Mature Daikiba",
                1,
                "Daikiba Mature",
                Some("Unknown")
            ),
            (
                MobDetectionGrade::Provisional,
                "Gloomrat",
                2,
                "Gloomrat",
                Some("Gloomrat")
            ),
            (
                MobDetectionGrade::Named,
                "Old Atrox",
                1,
                "Atrox Old",
                Some("Old Atrox")
            ),
        ],
        "a suggestion keeps the kill's own stamp; a stamped group shows what it stamped"
    );
    assert!(groups.iter().all(|g| !g.reviewed));
    assert_eq!(
        groups[1].group.provisional_key.as_deref(),
        Some("GLOOMRAT L4 #0")
    );
    assert!(matches!(
        rig.runtime.block_on(rig.review.detections("nope")),
        Err(MobReviewError::NotFound(_))
    ));
}

#[test]
fn naming_a_new_creature_renames_its_kills_and_teaches_the_reader_the_name() {
    let rig = rig();
    hunt(&rig);
    let names = bundled_profile().mob_names;
    assert!(matches!(
        resolve("Gloomrat", Some(4), &rig.catalogue.current(), &names),
        Resolved::New { .. }
    ));
    let group = rig.groups("s1")[1].group.clone();
    let settled = rig
        .settle("s1", &group, ReviewTarget::Creature("  Gloomrat ".into()))
        .unwrap();
    assert_eq!(
        settled,
        Settled::NewName(DetectedCreature {
            name: "Gloomrat".into(),
            species: "Gloomrat".into(),
            maturity: String::new(),
        })
    );
    for kill in ["k1", "k2"] {
        assert_eq!(
            rig.stamp(kill),
            (
                "Gloomrat".into(),
                "Gloomrat".into(),
                String::new(),
                some("declared"),
                None
            ),
            "the player's word, no longer a provisional stamp"
        );
    }
    assert_eq!(rig.confirmed(), ["Gloomrat"]);
    assert_eq!(
        resolve("Gloomrat", Some(4), &rig.catalogue.current(), &names),
        Resolved::Confirmed {
            name: "Gloomrat".into(),
            spelling: Spelling::Exact
        },
        "the next read of the name resolves as known"
    );
    let after = rig.groups("s1");
    assert!(after[1].reviewed, "the group is settled but still listed");
    assert_eq!(after[1].current_name.as_deref(), Some("Gloomrat"));
    // The other groups were not touched.
    assert_eq!(rig.stamp("k3").3, some("detected"));
    assert_eq!(rig.stamp("k3").4, some("named"));

    // Naming it again confirms the kept name, adding nothing.
    assert!(matches!(
        rig.settle("s1", &group, ReviewTarget::Creature("gloomrat".into())),
        Ok(Settled::Confirmed(_))
    ));
    assert_eq!(rig.confirmed(), ["Gloomrat"]);
}

#[test]
fn a_listed_creature_misspelt_is_that_creature_never_a_new_name() {
    let rig = rig();
    hunt(&rig);
    let group = rig.groups("s1")[1].group.clone();
    let settled = rig
        .settle("s1", &group, ReviewTarget::Creature("Atrx Young".into()))
        .unwrap();
    assert_eq!(
        settled,
        Settled::Listed(DetectedCreature {
            name: "Young Atrox".into(),
            species: "Atrox".into(),
            maturity: "Young".into(),
        })
    );
    assert_eq!(rig.stamp("k2").0, "Young Atrox");
    assert!(rig.confirmed().is_empty(), "nothing kept as a new name");

    // A species alone is that species at no maturity.
    let settled = rig
        .settle("s1", &group, ReviewTarget::Creature("daikiba".into()))
        .unwrap();
    assert_eq!(
        settled,
        Settled::Listed(DetectedCreature {
            name: "Daikiba".into(),
            species: "Daikiba".into(),
            maturity: String::new(),
        })
    );
}

#[test]
fn a_suggestion_is_confirmed_once_for_all_its_kills_and_any_group_can_go_back_to_the_declaration() {
    let rig = rig();
    hunt(&rig);
    let groups = rig.groups("s1");
    let suggestion = groups[0].group.clone();
    rig.settle(
        "s1",
        &suggestion,
        ReviewTarget::Creature("Daikiba Mature".into()),
    )
    .unwrap();
    assert_eq!(
        rig.stamp("k4"),
        (
            "Mature Daikiba".into(),
            "Daikiba".into(),
            "Mature".into(),
            some("declared"),
            None
        )
    );
    // Back to the declaration: the kill had none, so it is Unknown with no
    // source, exactly as recorded.
    rig.settle("s1", &suggestion, ReviewTarget::Declared)
        .unwrap();
    assert_eq!(
        rig.stamp("k4"),
        ("Unknown".into(), String::new(), String::new(), None, None)
    );

    let named = groups[2].group.clone();
    assert!(matches!(
        rig.settle("s1", &named, ReviewTarget::Declared),
        Ok(Settled::Declared)
    ));
    assert_eq!(
        rig.stamp("k3"),
        (
            "Young Atrox".into(),
            "Atrox".into(),
            "Young".into(),
            some("declared"),
            None
        )
    );
    // The session's settled cells follow the stamps.
    let species: Vec<(String, i64)> = rig
        .runtime
        .block_on(rig.db.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT mob_species, kills FROM session_kill_rollups WHERE session_id = 's1' \
                 ORDER BY mob_species",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }))
        .unwrap();
    assert_eq!(
        species,
        [
            (String::new(), 1),
            ("Atrox".to_string(), 1),
            ("Gloomrat".to_string(), 2)
        ]
    );
}

#[test]
fn refusals_change_nothing() {
    let rig = rig();
    hunt(&rig);
    rig.session("live", true);
    rig.kill(
        "live",
        "k9",
        (
            MobDetectionGrade::Named,
            detected("Old Atrox", "Atrox", "Old"),
            None,
            "Atrox Old",
        ),
        declared(),
    );
    let group = rig.groups("s1")[1].group.clone();
    let live = rig.groups("live")[0].group.clone();
    let ended = |session: &str| {
        rig.runtime
            .block_on(rig.review.detections(session))
            .unwrap()
            .ended
    };
    assert!(ended("s1"));
    assert!(!ended("live"), "a running session's review is not offered");
    assert!(matches!(
        rig.settle("live", &live, ReviewTarget::Declared),
        Err(MobReviewError::Conflict(_))
    ));
    assert_eq!(
        rig.stamp("k9").3,
        some("detected"),
        "a running session is left alone"
    );
    for typed in ["", "  ", "3", "12-4!"] {
        assert!(
            matches!(
                rig.settle("s1", &group, ReviewTarget::Creature(typed.into())),
                Err(MobReviewError::Invalid(_))
            ),
            "{typed:?} is refused"
        );
    }
    let mut gone = group.clone();
    gone.provisional_key = None;
    assert!(matches!(
        rig.settle("s1", &gone, ReviewTarget::Declared),
        Err(MobReviewError::NotFound(_))
    ));
    assert!(matches!(
        rig.settle("nope", &group, ReviewTarget::Declared),
        Err(MobReviewError::NotFound(_))
    ));
    assert_eq!(rig.stamp("k1").4, some("provisional"));
    assert!(rig.confirmed().is_empty());
}

#[test]
fn a_loose_match_is_offered_back_and_a_near_miss_of_a_kept_name_is_that_name() {
    let rig = rig();
    hunt(&rig);
    let group = rig.groups("s1")[1].group.clone();
    // Far enough off a listed creature to be only a guess: nothing changes,
    // the creature is offered in one action.
    let offered = rig
        .offer("s1", &group, ReviewTarget::Creature("Atrok Yung".into()))
        .unwrap();
    assert_eq!(
        offered,
        SettleOutcome::DidYouMean(DetectedCreature {
            name: "Young Atrox".into(),
            species: "Atrox".into(),
            maturity: "Young".into(),
        })
    );
    assert_eq!(rig.stamp("k1").4, some("provisional"), "nothing changed");

    rig.settle("s1", &group, ReviewTarget::Creature("Gloomrat".into()))
        .unwrap();
    // A typing slip of the kept name is the kept name, not another one.
    let settled = rig
        .settle("s1", &group, ReviewTarget::Creature("Gloomrut".into()))
        .unwrap();
    assert!(
        matches!(settled, Settled::Confirmed(ref c) if c.name == "Gloomrat"),
        "{settled:?}"
    );
    assert_eq!(rig.confirmed(), ["Gloomrat"]);
}

#[test]
fn a_kept_name_no_kill_carries_any_more_is_retired() {
    let rig = rig();
    hunt(&rig);
    let names = bundled_profile().mob_names;
    let group = rig.groups("s1")[1].group.clone();
    rig.settle("s1", &group, ReviewTarget::Creature("Gloomrat".into()))
        .unwrap();
    assert_eq!(rig.confirmed(), ["Gloomrat"]);
    // Settled to something else: no kill carries the name now.
    rig.settle("s1", &group, ReviewTarget::Declared).unwrap();
    assert!(rig.confirmed().is_empty(), "the name is retired");
    assert!(
        matches!(
            resolve("Gloomrat", Some(4), &rig.catalogue.current(), &names),
            Resolved::New { .. }
        ),
        "and the reader no longer knows it"
    );
}

#[test]
fn typed_names_are_held_to_what_a_name_can_be() {
    let rig = rig();
    hunt(&rig);
    let group = rig.groups("s1")[1].group.clone();
    let long = "G".repeat(MAX_NAME_CHARS) + "x";
    for typed in [
        long.as_str(),
        "Gloom\u{7}rat",
        "Gloom\u{200B}rat",
        "Gloomrat\u{202E}",
        "unknown",
        "UNKNOWN",
    ] {
        assert!(
            matches!(
                rig.settle("s1", &group, ReviewTarget::Creature(typed.into())),
                Err(MobReviewError::Invalid(_))
            ),
            "{typed:?} is refused"
        );
    }
    let longest = "Gloomrat ".to_string() + &"x".repeat(MAX_NAME_CHARS - 9);
    assert_eq!(longest.chars().count(), MAX_NAME_CHARS);
    assert!(matches!(
        rig.settle("s1", &group, ReviewTarget::Creature(longest)),
        Ok(Settled::NewName(_))
    ));
    assert_eq!(rig.stamp("k1").4, None);
}

#[test]
fn a_creature_cannot_be_changed_once_its_loot_has_left_stock() {
    let rig = rig();
    hunt(&rig);
    // Some of the session's hunted loot was sold.
    rig.execute(
        "INSERT INTO session_contexts (id, session_id, created_at) VALUES (900, 's1', 1000.0);
         INSERT INTO stock_movements (item_name, movement_kind, source_kind, mob_species, \
             quantity, tt_value, occurred_at, created_at, activity_context_id) \
         VALUES ('Animal Hide', 'listing', 'hunt', 'Gloomrat', -1, 0.4, '2026-01-01', 1.0, 900);",
    );
    let group = rig.groups("s1")[1].group.clone();
    let refused = rig.settle("s1", &group, ReviewTarget::Declared);
    assert!(
        matches!(refused, Err(MobReviewError::Conflict(message)) if message.contains("already been sold or converted")),
    );
    assert_eq!(rig.stamp("k1").4, some("provisional"), "nothing changed");
    // A settlement that moves no kill to another species still stands.
    assert!(rig
        .settle("s1", &group, ReviewTarget::Creature("Gloomrat".into()))
        .is_ok());
}

#[test]
fn a_settlement_is_kept_with_its_answer_and_repairs_the_session_figures() {
    let rig = rig();
    hunt(&rig);
    rig.execute("UPDATE kills SET original_mob_name = 'Gloomy' WHERE id = 'k1'");
    // The cached figures start from the detected stamps.
    rig.runtime
        .block_on(rig.db.with_writer(|conn| {
            let tx = conn.transaction()?;
            crate::session_summary::write_session_summary(&tx, "s1")?;
            tx.commit()?;
            Ok(())
        }))
        .unwrap();
    assert_eq!(
        rig.scalar::<Option<String>>(
            "SELECT dominant_mob FROM session_summaries WHERE session_id = 's1'"
        ),
        some("Gloomrat"),
        "two Gloomrat of three creature kills"
    );
    // The days are rolled up to a watermark past the session; emptied, a
    // settlement must roll the session's days up again.
    rig.execute(
        "INSERT INTO daily_rollup_meta (id, rolled_through) VALUES (1, '2100-01-01');
         DELETE FROM daily_rollups;",
    );
    let suggestion = rig.groups("s1")[0].group.clone();
    let named = rig.groups("s1")[2].group.clone();
    for group in [&suggestion, &named] {
        rig.settle("s1", group, ReviewTarget::Creature("Daikiba Mature".into()))
            .unwrap();
    }
    assert_eq!(
        rig.scalar::<Option<String>>(
            "SELECT dominant_mob FROM session_summaries WHERE session_id = 's1'"
        ),
        None,
        "rewritten from the settled stamps: two Gloomrat, two Daikiba"
    );
    assert!(
        rig.scalar::<i64>("SELECT COUNT(*) FROM daily_rollups") > 0,
        "the session's days are rolled up again"
    );
    let kept: (String, String, String, String) = rig
        .runtime
        .block_on(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT grade, settled_kind, settled_name, declared_name \
                 FROM kill_mob_detections WHERE kill_id = 'k3'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?)
        }))
        .unwrap();
    assert_eq!(
        kept,
        (
            "named".to_string(),
            "listed".to_string(),
            "Mature Daikiba".to_string(),
            "Young Atrox".to_string()
        ),
        "what detection said, what it was settled as, and the declaration"
    );
    // Settling the provisional group clears the older rename's restore point.
    let provisional = rig.groups("s1")[1].group.clone();
    rig.settle(
        "s1",
        &provisional,
        ReviewTarget::Creature("Gloomrat".into()),
    )
    .unwrap();
    assert_eq!(
        rig.scalar::<Option<String>>("SELECT original_mob_name FROM kills WHERE id = 'k1'"),
        None
    );
}

#[test]
fn deleting_a_session_removes_its_detections() {
    let rig = rig();
    hunt(&rig);
    rig.runtime
        .block_on(crate::tracking_reads::delete_session_impl(&rig.db, "s1"))
        .unwrap();
    assert_eq!(
        rig.scalar::<i64>("SELECT COUNT(*) FROM kill_mob_detections"),
        0
    );
}

/// The hunt above, then kills given the creature being hunted (two of a
/// listed creature, one of a creature on no list) and one kill taking a
/// pinned creature.
fn hunt_with_continuity(rig: &Rig) {
    hunt(rig);
    for kill in ["k5", "k6"] {
        rig.kill(
            "s1",
            kill,
            (
                MobDetectionGrade::Continuity,
                detected("Old Atrox", "Atrox", "Old"),
                None,
                "",
            ),
            declared(),
        );
    }
    rig.kill(
        "s1",
        "k7",
        (
            MobDetectionGrade::ProvisionalContinuity,
            detected("Gloomrat", "Gloomrat", ""),
            Some("GLOOMRAT L4 #0"),
            "",
        ),
        stamped("Unknown", "", "", None),
    );
    rig.execute(
        "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
         mob_stamp_source, mob_route, timestamp, cost_ped, loot_total_ped) \
         VALUES ('k8', 's1', 'Mature Daikiba', 'Daikiba', 'Mature', 'declared', \
                 'pinned_read', 1600.0, 0.5, 0.4)",
    );
}

/// A listed group: grade, creature, kills, the text read, and the stamp now.
type Listed<'a> = (
    MobDetectionGrade,
    &'a str,
    i64,
    Option<&'a str>,
    Option<&'a str>,
);

#[test]
fn continuity_kills_are_listed_last_without_a_reading_and_pinned_kills_not_at_all() {
    let rig = rig();
    hunt_with_continuity(&rig);
    let groups = rig.groups("s1");
    let listed: Vec<Listed<'_>> = groups
        .iter()
        .map(|g| {
            (
                g.group.grade,
                g.group.creature.name.as_str(),
                g.kill_count,
                g.example_text.as_deref(),
                g.current_name.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (
                MobDetectionGrade::LatestRead,
                "Mature Daikiba",
                1,
                Some("Daikiba Mature"),
                Some("Unknown")
            ),
            (
                MobDetectionGrade::Provisional,
                "Gloomrat",
                2,
                Some("Gloomrat"),
                Some("Gloomrat")
            ),
            (
                MobDetectionGrade::Named,
                "Old Atrox",
                1,
                Some("Atrox Old"),
                Some("Old Atrox")
            ),
            // The creature being hunted, after every grade the screen
            // gave: no text was read for these kills.
            (
                MobDetectionGrade::ProvisionalContinuity,
                "Gloomrat",
                1,
                None,
                Some("Gloomrat")
            ),
            (
                MobDetectionGrade::Continuity,
                "Old Atrox",
                2,
                None,
                Some("Old Atrox")
            ),
        ]
    );
    assert_eq!(
        groups[3].group.provisional_key.as_deref(),
        Some("GLOOMRAT L4 #0")
    );
    // The pinned kill is the player's own choice: no group holds it.
    let total: i64 = groups.iter().map(|g| g.kill_count).sum();
    assert_eq!(total, 7);
    // A continuity kill keeps no reading of its own: its stamp carries no
    // grade or text, only its route.
    assert_eq!(
        rig.stamp("k5"),
        (
            "Old Atrox".into(),
            "Atrox".into(),
            "Old".into(),
            some("detected"),
            None
        )
    );
    assert_eq!(
        rig.scalar::<String>("SELECT mob_route FROM kills WHERE id = 'k5'"),
        "continuity"
    );
    assert_eq!(
        rig.scalar::<Option<String>>("SELECT mob_read_text FROM kills WHERE id = 'k5'"),
        None
    );
    assert_eq!(
        rig.scalar::<String>("SELECT mob_route FROM kills WHERE id = 'k7'"),
        "provisional_continuity"
    );
}

#[test]
fn a_continuity_group_settles_in_one_action_like_any_other() {
    let rig = rig();
    hunt_with_continuity(&rig);
    let group = DetectionGroupRef {
        grade: MobDetectionGrade::Continuity,
        creature: DetectedCreature {
            name: "Old Atrox".into(),
            species: "Atrox".into(),
            maturity: "Old".into(),
        },
        provisional_key: None,
    };
    // Put back the stamp each kill was recorded with.
    assert_eq!(
        rig.settle("s1", &group, ReviewTarget::Declared).unwrap(),
        Settled::Declared
    );
    for kill in ["k5", "k6"] {
        assert_eq!(
            rig.stamp(kill),
            (
                "Young Atrox".into(),
                "Atrox".into(),
                "Young".into(),
                some("declared"),
                None
            )
        );
    }
    assert_eq!(
        rig.scalar::<i64>(
            "SELECT COUNT(*) FROM kill_mob_carried \
             WHERE settled_kind = 'declared' AND reviewed_at IS NOT NULL"
        ),
        2
    );
    // The route stays as the session gave it; the settlement is kept on
    // the answer.
    assert_eq!(
        rig.scalar::<String>("SELECT mob_route FROM kills WHERE id = 'k6'"),
        "continuity"
    );
    // The group is reviewed and can be settled again, as a named creature.
    let reviewed = rig.groups("s1");
    assert!(reviewed.last().unwrap().reviewed);
    assert_eq!(
        rig.settle("s1", &group, ReviewTarget::Creature("Daikiba".into()))
            .unwrap(),
        Settled::Listed(DetectedCreature {
            name: "Daikiba".into(),
            species: "Daikiba".into(),
            maturity: String::new(),
        })
    );
    assert_eq!(rig.stamp("k5").0, "Daikiba");
    assert_eq!(
        rig.scalar::<String>("SELECT settled_kind FROM kill_mob_carried WHERE kill_id = 'k5'"),
        "listed"
    );
    // The unlisted continuity group is told apart by its key and grade: the
    // listed grade does not reach it.
    let wrong_grade = DetectionGroupRef {
        grade: MobDetectionGrade::Continuity,
        creature: DetectedCreature {
            name: "Gloomrat".into(),
            species: "Gloomrat".into(),
            maturity: String::new(),
        },
        provisional_key: Some("GLOOMRAT L4 #0".into()),
    };
    assert!(matches!(
        rig.settle("s1", &wrong_grade, ReviewTarget::Declared),
        Err(MobReviewError::NotFound(_))
    ));
    let unlisted = DetectionGroupRef {
        grade: MobDetectionGrade::ProvisionalContinuity,
        ..wrong_grade
    };
    assert_eq!(
        rig.settle("s1", &unlisted, ReviewTarget::Declared).unwrap(),
        Settled::Declared
    );
    assert_eq!(rig.stamp("k7").0, "Unknown");
    // The pinned kill was never touched.
    assert_eq!(rig.stamp("k8").0, "Mature Daikiba");
}

#[test]
fn deleting_a_session_removes_its_continuity_kills() {
    let rig = rig();
    hunt_with_continuity(&rig);
    rig.runtime
        .block_on(crate::tracking_reads::delete_session_impl(&rig.db, "s1"))
        .unwrap();
    assert_eq!(
        rig.scalar::<i64>("SELECT COUNT(*) FROM kill_mob_carried"),
        0
    );
    assert_eq!(rig.scalar::<i64>("SELECT COUNT(*) FROM kills"), 0);
}

/// Each row of the session's mob breakdown: name, kills, how sure.
fn breakdown(rig: &Rig) -> Vec<(String, i64, String)> {
    let detail = rig
        .runtime
        .block_on(crate::tracking_reads::get_session_impl(
            &rig.db, "s1", 5000.0,
        ))
        .unwrap()
        .expect("the session");
    let mut rows: Vec<(String, i64, String)> = detail["mobBreakdown"]
        .as_array()
        .expect("a breakdown")
        .iter()
        .map(|row| {
            (
                row["currentName"].as_str().unwrap().to_string(),
                row["killCount"].as_i64().unwrap(),
                row["certainty"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    rows.sort();
    rows
}

#[test]
fn an_unlisted_creature_carried_on_reads_as_unlisted_and_merges_with_its_detections() {
    let rig = rig();
    hunt_with_continuity(&rig);
    // Gloomrat: two provisional detections and one continuity kill, all
    // unconfirmed, are one row; the Old Atrox continuity kills are as
    // recorded, with the named one.
    assert_eq!(
        breakdown(&rig),
        [
            ("Gloomrat".to_string(), 3, "unlisted".to_string()),
            ("Mature Daikiba".to_string(), 1, "recorded".to_string()),
            ("Old Atrox".to_string(), 3, "recorded".to_string()),
            ("Unknown".to_string(), 1, "recorded".to_string()),
        ]
    );
    // Settled, the continuity kill is the player's word: recorded.
    let unlisted = DetectionGroupRef {
        grade: MobDetectionGrade::ProvisionalContinuity,
        creature: DetectedCreature {
            name: "Gloomrat".into(),
            species: "Gloomrat".into(),
            maturity: String::new(),
        },
        provisional_key: Some("GLOOMRAT L4 #0".into()),
    };
    rig.settle("s1", &unlisted, ReviewTarget::Creature("Gloomrat".into()))
        .unwrap();
    let rows = breakdown(&rig);
    assert!(rows.contains(&("Gloomrat".to_string(), 1, "recorded".to_string())));
    assert!(rows.contains(&("Gloomrat".to_string(), 2, "unlisted".to_string())));
}

/// A kill pinned to a creature on no list read off the screen (`k9`), and
/// one pinned to a listed creature (`k8`, from the hunt above).
fn hunt_with_pins(rig: &Rig) {
    hunt_with_continuity(rig);
    rig.kill(
        "s1",
        "k9",
        (
            MobDetectionGrade::ProvisionalPinned,
            stamped("Shadowmaw", "Shadowmaw", "", Some(MobStampSource::Declared)),
            Some("SHADOWMAW L7 #2"),
            "",
        ),
        declared(),
    );
    rig.execute(
        "UPDATE kills SET mob_name = 'Shadowmaw', mob_species = 'Shadowmaw', mob_maturity = '', \
         mob_stamp_source = 'declared', mob_route = 'pinned_read' WHERE id = 'k9'",
    );
}

#[test]
fn a_pinned_unlisted_creature_is_listed_to_confirm_and_a_pinned_listed_one_is_not() {
    let rig = rig();
    hunt_with_pins(&rig);
    let groups = rig.groups("s1");
    let pinned: Vec<_> = groups
        .iter()
        .filter(|g| g.group.grade == MobDetectionGrade::ProvisionalPinned)
        .collect();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].group.creature.name, "Shadowmaw");
    assert_eq!(pinned[0].kill_count, 1);
    assert_eq!(pinned[0].example_text, None);
    assert_eq!(
        pinned[0].group.provisional_key.as_deref(),
        Some("SHADOWMAW L7 #2")
    );
    // Asked about beside the other unlisted names, before the named ones.
    let grades: Vec<_> = groups.iter().map(|g| g.group.grade).collect();
    let at = |grade| grades.iter().position(|g| *g == grade).unwrap();
    assert!(at(MobDetectionGrade::Provisional) < at(MobDetectionGrade::ProvisionalPinned));
    assert!(at(MobDetectionGrade::ProvisionalPinned) < at(MobDetectionGrade::Named));
    // The kill pinned to a listed creature is in no group.
    let total: i64 = groups.iter().map(|g| g.kill_count).sum();
    assert_eq!(total, 8);
    // Unconfirmed, the breakdown says so.
    assert!(breakdown(&rig).contains(&("Shadowmaw".to_string(), 1, "unlisted".to_string())));

    // Confirmed as a new name in one action: kept, and recorded.
    let group = pinned[0].group.clone();
    assert_eq!(
        rig.settle("s1", &group, ReviewTarget::Creature("Shadowmaw".into()))
            .unwrap(),
        Settled::NewName(DetectedCreature {
            name: "Shadowmaw".into(),
            species: "Shadowmaw".into(),
            maturity: String::new(),
        })
    );
    assert!(rig.confirmed().contains(&"Shadowmaw".to_string()));
    assert!(breakdown(&rig).contains(&("Shadowmaw".to_string(), 1, "recorded".to_string())));
    // Put back instead: the stamp it would have had without the pin.
    rig.settle("s1", &group, ReviewTarget::Declared).unwrap();
    assert_eq!(rig.stamp("k9").0, "Young Atrox");
    assert_eq!(rig.stamp("k8").0, "Mature Daikiba");
}

#[test]
fn settling_kills_carried_on_repairs_the_session_figures_and_respects_sold_loot() {
    let rig = rig();
    hunt_with_continuity(&rig);
    rig.runtime
        .block_on(rig.db.with_writer(|conn| {
            let tx = conn.transaction()?;
            crate::session_summary::write_session_summary(&tx, "s1")?;
            tx.commit()?;
            Ok(())
        }))
        .unwrap();
    assert_ne!(
        rig.scalar::<Option<String>>(
            "SELECT dominant_mob FROM session_summaries WHERE session_id = 's1'"
        ),
        some("Gloomrat"),
        "three Old Atrox, two of them carried on, beside three Gloomrat"
    );
    rig.execute(
        "INSERT INTO daily_rollup_meta (id, rolled_through) VALUES (1, '2100-01-01');
         DELETE FROM daily_rollups;",
    );
    let group = DetectionGroupRef {
        grade: MobDetectionGrade::Continuity,
        creature: DetectedCreature {
            name: "Old Atrox".into(),
            species: "Atrox".into(),
            maturity: "Old".into(),
        },
        provisional_key: None,
    };
    rig.settle("s1", &group, ReviewTarget::Creature("Gloomrat".into()))
        .unwrap();
    assert_eq!(
        rig.scalar::<Option<String>>(
            "SELECT dominant_mob FROM session_summaries WHERE session_id = 's1'"
        ),
        some("Gloomrat"),
        "rewritten from the settled stamps"
    );
    assert!(
        rig.scalar::<i64>("SELECT COUNT(*) FROM daily_rollups") > 0,
        "the session's days are rolled up again"
    );

    // Once hunted loot has left stock, a move to another creature is refused
    // and nothing changes.
    rig.execute(
        "INSERT INTO session_contexts (id, session_id, created_at) VALUES (900, 's1', 1000.0);
         INSERT INTO stock_movements (item_name, movement_kind, source_kind, mob_species, \
             quantity, tt_value, occurred_at, created_at, activity_context_id) \
         VALUES ('Animal Hide', 'listing', 'hunt', 'Gloomrat', -1, 0.4, '2026-01-01', 1.0, 900);",
    );
    let refused = rig.settle("s1", &group, ReviewTarget::Declared);
    assert!(
        matches!(refused, Err(MobReviewError::Conflict(message)) if message.contains("already been sold or converted")),
    );
    assert_eq!(rig.stamp("k5").0, "Gloomrat", "nothing changed");
}
