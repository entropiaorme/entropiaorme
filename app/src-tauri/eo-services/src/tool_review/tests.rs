//! Settling sightings: what each action keeps and refuses, that adding
//! moves no cost, and that every undo puts back what it changed.

use super::*;
use crate::clock::MockClock;

/// The bundled weapon's cost per shot without attachments, as the tracker
/// booked its shots.
const BARE_COST: f64 = 0.05;

struct Rig {
    _dir: tempfile::TempDir,
    db: Db,
    review: ToolReviewService,
}

async fn fresh() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("entropia_orme.db"))
        .await
        .unwrap();
    let review = ToolReviewService::new(db.clone(), Arc::new(MockClock::new(None, 0.0)));
    Rig {
        _dir: dir,
        db,
        review,
    }
}

/// The props the add path stores for the sighted weapon picked bare.
fn bare_weapon() -> NewEntry {
    NewEntry {
        name: "Sollomate Opalo".into(),
        properties_json: r#"{"weapon_entity": {"id": 42, "name": "Sollomate Opalo"}, "weapon_catalog_id": "42", "amp_entity": null, "weapon_markup": 100}"#.into(),
    }
}

/// A session (ended unless `active`) whose kills were made with a weapon
/// of the bundled data the library does not hold, its shots booked under
/// its name at its bare cost, and a kill made with a library weapon, its
/// summary, days and cells settled as the tracker leaves them. It saw the
/// bundled weapon (`b`, 3 shots), a bundled healer (`h`), and two names
/// matching no tool (`u`, `v`).
async fn seed(db: &Db, session_id: &'static str, active: bool) {
    db.with_writer(move |conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
             VALUES (?1, 1000, ?2, ?3)",
            rusqlite::params![session_id, (!active).then_some(2000.0), active as i64],
        )?;
        let id = |name: &str| format!("{session_id}-{name}");
        for (kill, at, shots, cost) in [("k1", 1100.0, 2, 0.1), ("k2", 1200.0, 3, 0.25)] {
            tx.execute(
                "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
                 timestamp, shots_fired, cost_ped, loot_total_ped) \
                 VALUES (?1, ?2, 'Young Atrox', 'Atrox', 'Young', ?3, ?4, ?5, 0.4)",
                rusqlite::params![id(kill), session_id, at, shots, cost],
            )?;
        }
        tx.execute(
            "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
             critical_hits, cost_per_shot) VALUES \
             (?1, 'Sollomate Opalo', 2, 20, 0, ?3), \
             (?2, 'Sollomate Opalo', 1, 9, 0, ?3), \
             (?2, 'Pistol', 2, 18, 0, 0.1)",
            rusqlite::params![id("k1"), id("k2"), BARE_COST],
        )?;
        tx.execute(
            "INSERT INTO tool_sightings (id, session_id, kind, endpoint, item_id, item_name, \
             first_seen, last_seen, shots, cost_per_shot) VALUES \
             (?1, ?3, 'bundled', 'weapons', '42', 'Sollomate Opalo', 1050, 1250, 3, ?4), \
             (?2, ?3, 'bundled', 'medical_tools', '7', 'Vivo T1', 1300, 1310, 0, 0.01)",
            rusqlite::params![id("b"), id("h"), session_id, BARE_COST],
        )?;
        tx.execute(
            "INSERT INTO tool_sightings (id, session_id, kind, text_key, text_read, first_seen, \
             last_seen) VALUES \
             (?1, ?3, 'unknown', 'teleport chip', 'Teleport  Chip', 1000, 1000), \
             (?2, ?3, 'unknown', 'repair unit', 'Repair Unit', 1400, 1400)",
            rusqlite::params![id("u"), id("v"), session_id],
        )?;
        crate::session_summary::write_session_summary(&tx, session_id)?;
        crate::daily_rollup::refresh_session_days(&tx, session_id)?;
        crate::session_rollup::recompute_session(&tx, session_id)?;
        tx.commit()?;
        Ok(())
    })
    .await
    .unwrap();
}

/// The undo context of an entry the add path would still build as
/// [`bare_weapon`], held nowhere in the settings.
fn bare_context() -> UndoContext {
    UndoContext {
        bare: Some(bare_weapon()),
        in_settings: false,
    }
}

/// Every row of every table but `skip`, as text, table by table: two
/// equal dumps are an unchanged database.
async fn dump(db: &Db, skip: &'static [&'static str]) -> Vec<String> {
    db.with_reader(move |conn| {
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' ORDER BY name",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = Vec::new();
        for table in tables.iter().filter(|t| !skip.contains(&t.as_str())) {
            let mut stmt = conn.prepare(&format!("SELECT * FROM {table}"))?;
            let width = stmt.column_count();
            let mut rows: Vec<String> = stmt
                .query_map([], |row| {
                    let values = (0..width)
                        .map(|i| {
                            row.get::<_, rusqlite::types::Value>(i)
                                .map(|v| format!("{v:?}"))
                        })
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(format!("{table}: {}", values.join(" | ")))
                })?
                .collect::<rusqlite::Result<_>>()?;
            rows.sort();
            out.extend(rows);
        }
        Ok(out)
    })
    .await
    .unwrap()
}

/// Everything a cost, a kill or a rollup lives in: every table but the
/// library and the sightings themselves.
const NOT_COSTS: &[&str] = &[
    "equipment_library",
    "tool_sightings",
    "dismissed_tool_names",
];

fn sighting<'a>(review: &'a SightingReview, id: &str) -> &'a ToolSighting {
    review
        .sightings
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("{id} listed"))
}

async fn library(db: &Db) -> Vec<(i64, String, String, Option<String>, bool)> {
    db.with_reader(|conn| {
        Ok(conn
            .prepare(
                "SELECT id, name, item_type, catalog_id, enabled FROM equipment_library \
                 ORDER BY created_at, id",
            )?
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .await
    .unwrap()
}

async fn dismissed(db: &Db) -> Vec<String> {
    db.with_reader(|conn| crate::tool_catalogue::dismissed_names(conn))
        .await
        .unwrap()
}

async fn add_entry(db: &Db, name: &'static str, catalog_id: &'static str, enabled: bool) -> i64 {
    db.with_writer(move |conn| {
        let id = insert_equipment_row(conn, name, "weapon", Some(catalog_id), "{}")?;
        conn.execute(
            "UPDATE equipment_library SET enabled = ?1 WHERE id = ?2",
            rusqlite::params![enabled, id],
        )?;
        Ok(id)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn the_review_asks_about_tools_first_then_names_then_lists_what_is_settled() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    rig.review.leave("s", "s-h").await.unwrap().unwrap();

    let review = rig.review.sightings("s").await.unwrap();
    assert!(review.ended);
    let order: Vec<&str> = review.sightings.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(order, ["s-b", "s-u", "s-v", "s-h"]);
    assert_eq!(
        sighting(&review, "s-b").sighted,
        Sighted::Bundled {
            endpoint: "weapons".into(),
            item_id: "42".into(),
            name: "Sollomate Opalo".into(),
            kind: ToolKind::Weapon,
            shots: Some(3),
        }
    );
    assert_eq!(
        sighting(&review, "s-u").sighted,
        Sighted::Unknown {
            text: "Teleport  Chip".into()
        }
    );
    assert_eq!(sighting(&review, "s-h").settled, Some(Settled::Left));
    assert_eq!(sighting(&review, "s-b").settled, None);

    assert!(matches!(
        rig.review.sightings("nowhere").await,
        Err(ToolReviewError::SessionNotFound)
    ));
}

#[tokio::test]
async fn adding_creates_the_bare_entry_and_moves_no_cost() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    let before = dump(&rig.db, NOT_COSTS).await;

    let settled = rig
        .review
        .add("s", "s-b", bare_weapon())
        .await
        .unwrap()
        .unwrap();
    let Settlement::Added {
        equipment_id,
        created,
        kind,
        enabled,
    } = settled
    else {
        panic!("added: {settled:?}");
    };
    assert!(created && enabled);
    assert_eq!(kind, ToolKind::Weapon);
    assert_eq!(
        library(&rig.db).await,
        [(
            equipment_id,
            "Sollomate Opalo".to_string(),
            "weapon".to_string(),
            Some("42".to_string()),
            true
        )]
    );
    let props: String = rig
        .db
        .with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT properties_json FROM equipment_library WHERE id = ?1",
                [equipment_id],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(props, bare_weapon().properties_json);

    // The session's shots were booked under the tool's name at the cost
    // the bare entry prices them at: no kill, phase, summary, day or cell
    // moves.
    assert_eq!(dump(&rig.db, NOT_COSTS).await, before);
    let review = rig.review.sightings("s").await.unwrap();
    assert_eq!(
        sighting(&review, "s-b").settled,
        Some(Settled::Added { created: true })
    );
}

#[tokio::test]
async fn adding_links_to_the_entry_of_that_item_already_held() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    let off = add_entry(&rig.db, "Sollomate Opalo", "42", false).await;
    let on = add_entry(&rig.db, "Sollomate Opalo", "42", true).await;
    add_entry(&rig.db, "Herman ARK-0", "8", true).await;

    let settled = rig
        .review
        .add("s", "s-b", bare_weapon())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        settled,
        Settlement::Added {
            equipment_id: on,
            created: false,
            kind: ToolKind::Weapon,
            enabled: true,
        },
        "the entry in use, before the one turned off ({off})"
    );
    assert_eq!(library(&rig.db).await.len(), 3, "no second entry");
    let review = rig.review.sightings("s").await.unwrap();
    assert_eq!(
        sighting(&review, "s-b").settled,
        Some(Settled::Added { created: false })
    );
}

#[tokio::test]
async fn a_healing_tool_is_added_as_a_healing_entry() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    let healer = NewEntry {
        name: "Vivo T1".into(),
        properties_json: r#"{"tool_catalog_id": "7", "markup": 100}"#.into(),
    };
    let settled = rig.review.add("s", "s-h", healer).await.unwrap().unwrap();
    assert!(matches!(
        settled,
        Settlement::Added {
            kind: ToolKind::Healing,
            created: true,
            ..
        }
    ));
    assert_eq!(library(&rig.db).await[0].2, "healing");
}

#[tokio::test]
async fn each_refusal_changes_nothing() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    seed(&rig.db, "live", true).await;
    let before = dump(&rig.db, &[]).await;

    let refused = |outcome: Outcome<Settlement>| outcome.unwrap().unwrap_err();
    assert_eq!(
        refused(rig.review.add("live", "live-b", bare_weapon()).await),
        Refusal::SessionRunning
    );
    assert_eq!(
        refused(rig.review.add("s", "s-u", bare_weapon()).await),
        Refusal::NotATool
    );
    assert_eq!(
        refused(rig.review.add("s", "live-b", bare_weapon()).await),
        Refusal::OtherSession
    );
    assert_eq!(
        refused(rig.review.leave("s", "live-u").await),
        Refusal::OtherSession
    );
    assert_eq!(
        refused(rig.review.dismiss("s", "nothing").await),
        Refusal::NotFound
    );
    assert_eq!(
        refused(rig.review.undo("s", "s-u", UndoContext::default()).await),
        Refusal::NotSettled
    );
    assert_eq!(
        refused(rig.review.undo("live", "s-u", UndoContext::default()).await),
        Refusal::OtherSession
    );
    assert_eq!(dump(&rig.db, &[]).await, before);

    // A running session's names can be dismissed or left meanwhile.
    rig.review.leave("live", "live-v").await.unwrap().unwrap();
}

#[tokio::test]
async fn a_settled_sighting_refuses_a_second_settlement_naming_what_it_is() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    rig.review
        .add("s", "s-b", bare_weapon())
        .await
        .unwrap()
        .unwrap();
    rig.review.dismiss("s", "s-u").await.unwrap().unwrap();
    rig.review.leave("s", "s-v").await.unwrap().unwrap();
    let before = dump(&rig.db, &[]).await;

    for (id, was) in [
        ("s-b", SettledKind::Added),
        ("s-u", SettledKind::Dismissed),
        ("s-v", SettledKind::Left),
    ] {
        let refusal = Refusal::AlreadySettled(was);
        assert_eq!(
            rig.review.leave("s", id).await.unwrap(),
            Err(refusal),
            "{id}"
        );
        assert_eq!(rig.review.dismiss("s", id).await.unwrap(), Err(refusal));
        assert_eq!(
            rig.review.add("s", id, bare_weapon()).await.unwrap(),
            Err(refusal)
        );
    }
    assert_eq!(dump(&rig.db, &[]).await, before);
}

#[tokio::test]
async fn a_dismissed_name_is_kept_tidy_and_asked_about_in_no_other_session() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    seed(&rig.db, "t", false).await;

    assert_eq!(
        rig.review.dismiss("s", "s-u").await.unwrap(),
        Ok(Settlement::Dismissed)
    );
    assert_eq!(dismissed(&rig.db).await, ["Teleport Chip"]);
    let here = rig.review.sightings("s").await.unwrap();
    assert_eq!(sighting(&here, "s-u").settled, Some(Settled::Dismissed));
    let there = rig.review.sightings("t").await.unwrap();
    assert!(
        there.sightings.iter().all(|s| s.id != "t-u"),
        "the other session's sighting of the name asks nothing more"
    );

    // A bundled tool the player says they do not track is dismissed by
    // its name.
    rig.review.dismiss("s", "s-b").await.unwrap().unwrap();
    assert_eq!(
        dismissed(&rig.db).await,
        ["Sollomate Opalo", "Teleport Chip"]
    );
}

#[tokio::test]
async fn undoing_a_dismissal_takes_the_name_off_unless_another_dismissal_carries_it() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    seed(&rig.db, "t", false).await;
    rig.review.dismiss("s", "s-u").await.unwrap().unwrap();
    rig.review.dismiss("t", "t-u").await.unwrap().unwrap();

    assert_eq!(
        rig.review
            .undo("s", "s-u", UndoContext::default())
            .await
            .unwrap(),
        Ok(Settlement::Reopened {
            was: SettledKind::Dismissed,
            entry: EntryOnUndo::None
        })
    );
    assert_eq!(
        dismissed(&rig.db).await,
        ["Teleport Chip"],
        "the other session's dismissal still carries it"
    );
    rig.review
        .undo("t", "t-u", UndoContext::default())
        .await
        .unwrap()
        .unwrap();
    assert!(dismissed(&rig.db).await.is_empty());
    let review = rig.review.sightings("s").await.unwrap();
    assert_eq!(sighting(&review, "s-u").settled, None, "asked again");
}

#[tokio::test]
async fn leaving_and_its_undo_touch_nothing_else() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    let before = dump(&rig.db, &[]).await;
    rig.review.leave("s", "s-b").await.unwrap().unwrap();
    assert_eq!(
        rig.review
            .undo("s", "s-b", UndoContext::default())
            .await
            .unwrap(),
        Ok(Settlement::Reopened {
            was: SettledKind::Left,
            entry: EntryOnUndo::None
        })
    );
    assert_eq!(dump(&rig.db, &[]).await, before);
}

#[tokio::test]
async fn undoing_an_addition_takes_away_the_entry_it_made_and_restores_every_figure() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    let before = dump(&rig.db, &[]).await;
    rig.review
        .add("s", "s-b", bare_weapon())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        rig.review.undo("s", "s-b", bare_context()).await.unwrap(),
        Ok(Settlement::Reopened {
            was: SettledKind::Added,
            entry: EntryOnUndo::Removed
        })
    );
    assert_eq!(dump(&rig.db, &[]).await, before, "every table as it was");
}

#[tokio::test]
async fn undoing_an_addition_keeps_an_entry_it_only_linked_to() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    add_entry(&rig.db, "Sollomate Opalo", "42", true).await;
    rig.review
        .add("s", "s-b", bare_weapon())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rig.review.undo("s", "s-b", bare_context()).await.unwrap(),
        Ok(Settlement::Reopened {
            was: SettledKind::Added,
            entry: EntryOnUndo::None
        })
    );
    assert_eq!(library(&rig.db).await.len(), 1);
}

/// One way the entry an addition made can be in use or changed after the
/// addition, as the database or the settings show it.
struct Since {
    why: &'static str,
    /// Written after the addition, given the entry's id.
    sql: fn(i64) -> String,
    /// The settings hold the entry.
    in_settings: bool,
    because: KeptBecause,
}

#[tokio::test]
async fn undoing_an_addition_keeps_an_entry_that_may_be_in_use_and_says_why() {
    let cases = [
        Since {
            why: "set up",
            sql: |id| {
                format!(
                    "UPDATE equipment_library SET properties_json = '{{\"amp_entity\": 1}}' \
                     WHERE id = {id}"
                )
            },
            in_settings: false,
            because: KeptBecause::Changed,
        },
        Since {
            why: "turned off",
            sql: |id| format!("UPDATE equipment_library SET enabled = 0 WHERE id = {id}"),
            in_settings: false,
            because: KeptBecause::Changed,
        },
        Since {
            why: "another detected tool became it",
            sql: |id| {
                format!(
                    "UPDATE tool_sightings SET settled_kind = 'added', settled_at = 1, \
                     equipment_id = {id} WHERE id = 's-h'"
                )
            },
            in_settings: false,
            because: KeptBecause::OtherSighting,
        },
        Since {
            why: "a session started since",
            sql: |_| {
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
                 VALUES ('later', 4e9, 4.1e9, 0)"
                    .to_string()
            },
            in_settings: false,
            because: KeptBecause::InSession,
        },
        Since {
            // The player was in another session when they added the tool:
            // its reader may have booked the new entry since.
            why: "a session running when it was added",
            sql: |_| {
                "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES ('running', 1500, 1)"
                    .to_string()
            },
            in_settings: false,
            because: KeptBecause::InSession,
        },
        Since {
            why: "a session that ended after it was added",
            sql: |_| {
                "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
                 VALUES ('ran', 1500, 4e9, 0)"
                    .to_string()
            },
            in_settings: false,
            because: KeptBecause::InSession,
        },
        Since {
            why: "a dose names it",
            sql: |id| {
                format!(
                    "INSERT INTO consumable_doses (id, equipment_id, item_name, source, \
                     started_at) VALUES ('d', {id}, 'Sollomate Opalo', 'manual', 1)"
                )
            },
            in_settings: false,
            because: KeptBecause::Referenced,
        },
        Since {
            why: "a shot carried it",
            sql: |id| {
                format!(
                    "INSERT INTO weapon_shot_evidence (id, session_id, observed_at, amount, \
                     critical, attribution, cost_per_shot, candidates_json, reason) VALUES \
                     ('e', 's', 1150, 9, 0, 'unresolved', 0, \
                     '[{{\"equipmentId\": {id}, \"name\": \"Sollomate Opalo\", \"fits\": true}}]', \
                     'seeded')"
                )
            },
            in_settings: false,
            because: KeptBecause::Referenced,
        },
        Since {
            why: "bound to a hotbar slot",
            sql: |_| String::new(),
            in_settings: true,
            because: KeptBecause::InSettings,
        },
    ];
    for case in cases {
        let rig = fresh().await;
        seed(&rig.db, "s", false).await;
        let Settlement::Added { equipment_id, .. } = rig
            .review
            .add("s", "s-b", bare_weapon())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("added");
        };
        let sql = (case.sql)(equipment_id);
        rig.db
            .with_writer(move |conn| {
                conn.execute_batch(&sql)?;
                Ok(())
            })
            .await
            .unwrap();
        let before = dump(&rig.db, &["tool_sightings"]).await;
        let context = UndoContext {
            in_settings: case.in_settings,
            ..bare_context()
        };
        assert_eq!(
            rig.review.undo("s", "s-b", context).await.unwrap(),
            Ok(Settlement::Reopened {
                was: SettledKind::Added,
                entry: EntryOnUndo::Kept(case.because)
            }),
            "{}",
            case.why
        );
        assert_eq!(
            dump(&rig.db, &["tool_sightings"]).await,
            before,
            "{}: the entry and every row naming it stay as they were",
            case.why
        );
        let review = rig.review.sightings("s").await.unwrap();
        assert_eq!(
            sighting(&review, "s-b").settled,
            None,
            "{}: asked again",
            case.why
        );
    }

    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    rig.review
        .add("s", "s-b", bare_weapon())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rig.review
            .undo("s", "s-b", UndoContext::default())
            .await
            .unwrap(),
        Ok(Settlement::Reopened {
            was: SettledKind::Added,
            entry: EntryOnUndo::Kept(KeptBecause::NotInGameData)
        }),
        "nothing to compare it with"
    );
}

#[tokio::test]
async fn a_heal_marked_as_paid_with_the_added_healer_keeps_it_through_an_undo() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    rig.db
        .with_writer(|conn| {
            conn.execute(
                "INSERT INTO healing_outputs (id, session_id, observed_at, chat_timestamp, \
                 amount, classification, reason) \
                 VALUES ('heal', 's', 1305, 't', 10, 'unattributed', 'seeded')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let healer = NewEntry {
        name: "Vivo T1".into(),
        properties_json: r#"{"tool_entity": {"id": 7, "name": "Vivo T1", "economy": {"decay": 1}}, "tool_catalog_id": "7", "markup": 100, "healing_profile": {"mode": "direct", "direct_min": 8, "direct_max": 12}}"#.into(),
    };
    let Settlement::Added { equipment_id, .. } = rig
        .review
        .add("s", "s-h", healer.clone())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("added");
    };
    // The heal is marked as a paid use of the new entry, as the review
    // tells the player to.
    let healing = crate::healing_review::HealingReviewService::new(
        rig.db.clone(),
        Arc::new(MockClock::new(None, 0.0)),
    );
    healing
        .correct(crate::healing_review::CorrectionTarget::PaidUse {
            output_id: "heal".into(),
            equipment_id,
        })
        .await
        .unwrap();
    let before = dump(&rig.db, &["tool_sightings"]).await;

    let context = UndoContext {
        bare: Some(healer),
        in_settings: false,
    };
    assert_eq!(
        rig.review.undo("s", "s-h", context).await.unwrap(),
        Ok(Settlement::Reopened {
            was: SettledKind::Added,
            entry: EntryOnUndo::Kept(KeptBecause::Referenced)
        })
    );
    assert_eq!(
        dump(&rig.db, &["tool_sightings"]).await,
        before,
        "the entry and the paid use naming it stay"
    );
}

#[tokio::test]
async fn deleting_a_session_releases_the_names_only_it_set_aside() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    seed(&rig.db, "t", false).await;
    rig.review.dismiss("s", "s-u").await.unwrap().unwrap();
    rig.review.dismiss("t", "t-u").await.unwrap().unwrap();
    rig.review.dismiss("s", "s-b").await.unwrap().unwrap();

    crate::tracking_reads::delete_session_impl(&rig.db, "s")
        .await
        .unwrap();
    assert_eq!(
        dismissed(&rig.db).await,
        ["Teleport Chip"],
        "the other session's dismissal still carries its name"
    );
    crate::tracking_reads::delete_session_impl(&rig.db, "t")
        .await
        .unwrap();
    assert!(dismissed(&rig.db).await.is_empty());
}

#[tokio::test]
async fn names_compare_as_the_reader_compares_them() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    seed(&rig.db, "t", false).await;
    // The other session read the same chip with other punctuation.
    rig.db
        .with_writer(|conn| {
            conn.execute(
                "UPDATE tool_sightings SET text_read = 'Teleport-Chip', text_key = 'teleport-chip' \
                 WHERE id = 't-u'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    rig.review.dismiss("s", "s-u").await.unwrap().unwrap();
    let there = rig.review.sightings("t").await.unwrap();
    assert!(
        there.sightings.iter().all(|s| s.id != "t-u"),
        "the reader takes them as one name"
    );

    // Brought back from the list by either spelling.
    assert!(rig.review.track_again("teleport-chip").await.unwrap());
    assert!(dismissed(&rig.db).await.is_empty());
    assert!(!rig.review.track_again("Teleport Chip").await.unwrap());
    let there = rig.review.sightings("t").await.unwrap();
    assert_eq!(sighting(&there, "t-u").settled, None, "asked about again");
    assert_eq!(
        rig.review.dismissed().await.unwrap(),
        Vec::<DismissedName>::new()
    );
}

#[tokio::test]
async fn undoing_an_addition_whose_entry_was_deleted_reopens_it_alone() {
    let rig = fresh().await;
    seed(&rig.db, "s", false).await;
    let Settlement::Added { equipment_id, .. } = rig
        .review
        .add("s", "s-b", bare_weapon())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("added");
    };
    rig.db.delete_equipment(equipment_id).await.unwrap();
    assert_eq!(
        rig.review.undo("s", "s-b", bare_context()).await.unwrap(),
        Ok(Settlement::Reopened {
            was: SettledKind::Added,
            entry: EntryOnUndo::None
        })
    );
    // Added again, a new entry is made.
    let again = rig
        .review
        .add("s", "s-b", bare_weapon())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(again, Settlement::Added { created: true, .. }));
}
