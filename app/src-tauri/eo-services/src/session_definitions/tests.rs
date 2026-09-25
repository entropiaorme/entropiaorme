use std::sync::Arc;

use rusqlite::params;

use crate::db::Db;

use super::{
    RosterEntryInput, RosterEntryKind, SessionDefinitionError, SessionDefinitionInput,
    SessionDefinitionService,
};

async fn service(dir: &std::path::Path) -> (Arc<SessionDefinitionService>, Db) {
    let db = Db::open(&dir.join("entropia_orme.db")).await.unwrap();
    let clock = Arc::new(crate::clock::MockClock::new(
        Some(
            chrono::NaiveDateTime::parse_from_str("2026-03-01 12:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap(),
        ),
        0.0,
    ));
    let svc = SessionDefinitionService::new(db.clone(), clock);
    (svc, db)
}

async fn seed_family(db: &Db, name: &str) -> i64 {
    let name = name.to_string();
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO quest_families (name) VALUES (?)",
            params![name],
        )?;
        Ok(conn.last_insert_rowid())
    })
    .await
    .unwrap()
}

async fn seed_quest(db: &Db, name: &str) -> i64 {
    let name = name.to_string();
    db.with_writer(move |conn| {
        conn.execute("INSERT INTO quests (name) VALUES (?)", params![name])?;
        Ok(conn.last_insert_rowid())
    })
    .await
    .unwrap()
}

fn input(name: &str, roster: Vec<RosterEntryInput>) -> SessionDefinitionInput {
    SessionDefinitionInput {
        name: name.to_string(),
        ad_hoc_segments: false,
        track_protection_costs: true,
        roster,
    }
}

fn family_entry(ref_id: i64) -> RosterEntryInput {
    RosterEntryInput {
        kind: RosterEntryKind::QuestFamily,
        ref_id: Some(ref_id),
        label: None,
    }
}

fn quest_entry(ref_id: i64) -> RosterEntryInput {
    RosterEntryInput {
        kind: RosterEntryKind::Quest,
        ref_id: Some(ref_id),
        label: None,
    }
}

fn segment_entry(label: &str) -> RosterEntryInput {
    RosterEntryInput {
        kind: RosterEntryKind::Segment,
        ref_id: None,
        label: Some(label.to_string()),
    }
}

#[tokio::test]
async fn definition_crud_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let family_id = seed_family(&db, "ARIS - Daily Hunting 1").await;
    let quest_id = seed_quest(&db, "The Ultimate Threat").await;

    let created = svc
        .create(SessionDefinitionInput {
            name: "  ARIS Dailies  ".to_string(),
            ad_hoc_segments: true,
            track_protection_costs: true,
            roster: vec![
                family_entry(family_id),
                quest_entry(quest_id),
                segment_entry("Warm-up"),
            ],
        })
        .await
        .unwrap();
    assert_eq!(created.name, "ARIS Dailies");
    assert!(created.ad_hoc_segments);
    assert!(created.is_active);
    assert_eq!(created.instance_count, 0);
    assert_eq!(created.roster.len(), 3);
    assert_eq!(created.roster[0].kind, RosterEntryKind::QuestFamily);
    assert_eq!(
        created.roster[0].display_name.as_deref(),
        Some("ARIS - Daily Hunting 1")
    );
    assert_eq!(created.roster[1].kind, RosterEntryKind::Quest);
    assert_eq!(
        created.roster[1].display_name.as_deref(),
        Some("The Ultimate Threat")
    );
    assert_eq!(created.roster[2].kind, RosterEntryKind::Segment);
    assert_eq!(created.roster[2].display_name.as_deref(), Some("Warm-up"));
    assert_eq!(
        created
            .roster
            .iter()
            .map(|entry| entry.position)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    // The seeded protected default is always there, so the authored one
    // is the second entry rather than the only one.
    let listed = svc.list(true).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed[0].is_protected);
    assert_eq!(listed[1].id, created.id);
    assert_eq!(listed[1].roster.len(), 3);

    let fetched = svc.get(created.id).await.unwrap().unwrap();
    assert_eq!(fetched.name, "ARIS Dailies");
}

#[tokio::test]
async fn a_definition_can_opt_out_of_armour_costs() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _) = service(dir.path()).await;

    let created = svc
        .create(SessionDefinitionInput {
            name: "Offensive costs only".to_string(),
            ad_hoc_segments: false,
            track_protection_costs: false,
            roster: Vec::new(),
        })
        .await
        .unwrap();

    assert!(!created.track_protection_costs);
}

#[tokio::test]
async fn update_replaces_the_roster_wholesale() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let family_id = seed_family(&db, "ARIS - Daily Hunting 1").await;

    let created = svc
        .create(input(
            "General Hunting",
            vec![family_entry(family_id), segment_entry("Travel")],
        ))
        .await
        .unwrap();

    let updated = svc
        .update(
            created.id,
            SessionDefinitionInput {
                name: "General Hunting".to_string(),
                ad_hoc_segments: true,
                track_protection_costs: true,
                roster: vec![segment_entry("Grind")],
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert!(updated.ad_hoc_segments);
    assert_eq!(updated.roster.len(), 1);
    assert_eq!(updated.roster[0].display_name.as_deref(), Some("Grind"));
    assert_eq!(updated.roster[0].position, 0);
    assert!(updated.updated_at.is_some());
}

#[tokio::test]
async fn archive_preserves_the_roster_and_restore_returns_it_to_play() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;

    let created = svc
        .create(input("ARIS Dailies", vec![segment_entry("Warm-up")]))
        .await
        .unwrap();
    let archived = svc.archive(created.id).await.unwrap().unwrap();
    assert_eq!(
        archived
            .fallback
            .as_ref()
            .map(|fallback| fallback.1.as_str()),
        Some("Default Tracking")
    );
    assert!(!archived.definition.is_active);
    assert_eq!(archived.definition.roster.len(), 1);
    assert_eq!(
        archived.definition.roster[0].display_name.as_deref(),
        Some("Warm-up")
    );
    // Idempotent: a second archive finds nothing active.
    assert!(svc.archive(created.id).await.unwrap().is_none());

    // Gone from the active list (which keeps the protected default),
    // still addressable by direct read with its authored roster intact.
    let remaining = svc.list(true).await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].is_protected);
    let inactive = svc.get(created.id).await.unwrap().unwrap();
    assert!(!inactive.is_active);
    assert_eq!(inactive.roster.len(), 1);
    assert!(svc.get_active(created.id).await.unwrap().is_none());

    // An archived definition reads as absent on update.
    let result = svc
        .update(created.id, input("Renamed", vec![]))
        .await
        .unwrap();
    assert!(result.is_none());

    let restored = svc.restore(created.id).await.unwrap().unwrap();
    assert!(restored.is_active);
    assert_eq!(restored.roster.len(), 1);
    assert_eq!(restored.roster[0].display_name.as_deref(), Some("Warm-up"));
    assert!(svc.restore(created.id).await.unwrap().is_none());
}

#[tokio::test]
async fn restore_refuses_a_name_that_became_active_while_archived() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;

    let first = svc.create(input("Seasonal", vec![])).await.unwrap();
    svc.archive(first.id).await.unwrap().unwrap();
    svc.create(input("seasonal", vec![])).await.unwrap();

    let result = svc.restore(first.id).await;
    assert!(matches!(result, Err(SessionDefinitionError::Invalid(_))));
    assert!(!svc.get(first.id).await.unwrap().unwrap().is_active);
}

#[tokio::test]
async fn names_are_unique_among_active_definitions() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;

    let first = svc.create(input("ARIS Dailies", vec![])).await.unwrap();
    let dup = svc.create(input("aris dailies", vec![])).await;
    assert!(matches!(dup, Err(SessionDefinitionError::Invalid(_))));

    // Updating a definition to its own name is not a collision.
    assert!(svc
        .update(first.id, input("ARIS Dailies", vec![]))
        .await
        .unwrap()
        .is_some());

    // A second definition cannot rename into the taken name.
    let second = svc.create(input("General Hunting", vec![])).await.unwrap();
    let clash = svc.update(second.id, input("ARIS Dailies", vec![])).await;
    assert!(matches!(clash, Err(SessionDefinitionError::Invalid(_))));

    // Archiving frees the active name.
    assert!(svc.archive(first.id).await.unwrap().is_some());
    assert!(svc.create(input("ARIS Dailies", vec![])).await.is_ok());
}

#[tokio::test]
async fn blank_names_and_malformed_roster_entries_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;

    assert!(matches!(
        svc.create(input("   ", vec![])).await,
        Err(SessionDefinitionError::Invalid(_))
    ));
    assert!(matches!(
        svc.create(input("A", vec![segment_entry("  ")])).await,
        Err(SessionDefinitionError::Invalid(_))
    ));
    assert!(matches!(
        svc.create(input(
            "A",
            vec![RosterEntryInput {
                kind: RosterEntryKind::QuestFamily,
                ref_id: None,
                label: None,
            }]
        ))
        .await,
        Err(SessionDefinitionError::Invalid(_))
    ));
    // A reference must name an ACTIVE target.
    assert!(matches!(
        svc.create(input("A", vec![family_entry(999)])).await,
        Err(SessionDefinitionError::Invalid(_))
    ));
    let quest_id = seed_quest(&db, "Retired").await;
    db.with_writer(move |conn| {
        conn.execute(
            "UPDATE quests SET is_active = 0 WHERE id = ?",
            params![quest_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(matches!(
        svc.create(input("A", vec![quest_entry(quest_id)])).await,
        Err(SessionDefinitionError::Invalid(_))
    ));
    // A failed create leaves nothing behind: only the seeded default.
    let all = svc.list(false).await.unwrap();
    assert_eq!(all.len(), 1);
    assert!(all[0].is_protected);
}

#[tokio::test]
async fn a_deleted_target_surfaces_as_a_missing_reference() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let family_id = seed_family(&db, "ARIS - Daily Hunting 1").await;

    let created = svc
        .create(input("ARIS Dailies", vec![family_entry(family_id)]))
        .await
        .unwrap();

    db.with_writer(move |conn| {
        conn.execute(
            "UPDATE quest_families SET is_active = 0 WHERE id = ?",
            params![family_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let read = svc.get(created.id).await.unwrap().unwrap();
    assert_eq!(read.roster.len(), 1);
    assert_eq!(read.roster[0].ref_id, Some(family_id));
    assert!(read.roster[0].display_name.is_none());
}

#[tokio::test]
async fn instance_count_counts_referencing_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;

    let created = svc.create(input("ARIS Dailies", vec![])).await.unwrap();
    let definition_id = created.id;
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, is_active, definition_id) \
             VALUES ('s-1', 1000.0, 0, ?1), ('s-2', 2000.0, 0, ?1), ('s-3', 3000.0, 0, NULL)",
            params![definition_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let read = svc.get(created.id).await.unwrap().unwrap();
    assert_eq!(read.instance_count, 2);
}

/// Tracking always needs a definition to run under, so the seeded one
/// refuses archiving while staying an ordinary definition in every other
/// respect, and "nothing chosen" resolves to it.
#[tokio::test]
async fn the_protected_default_cannot_be_archived_and_backs_every_selection() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;

    let seeded = svc.list(true).await.unwrap();
    assert_eq!(seeded.len(), 1);
    let default_id = seeded[0].id;
    assert_eq!(seeded[0].name, "Default Tracking");

    // Renaming is allowed: protection guards existence, not identity.
    svc.update(default_id, input("General Play", vec![]))
        .await
        .unwrap();
    assert_eq!(
        svc.archive(default_id).await.unwrap_err().to_string(),
        "General Play cannot be archived; tracking always needs one to run under"
    );
    assert!(svc.get_active(default_id).await.unwrap().is_some());

    // Nothing chosen, an unknown id, and an archived selection all resolve
    // to it; a live selection resolves to itself.
    let authored = svc.create(input("ARIS Dailies", vec![])).await.unwrap();
    for configured in [None, Some(9999), Some(default_id)] {
        assert_eq!(
            super::resolve_selection(&db, configured).await.unwrap(),
            Some((default_id, "General Play".to_string(), true))
        );
    }
    assert_eq!(
        super::resolve_selection(&db, Some(authored.id))
            .await
            .unwrap(),
        Some((authored.id, "ARIS Dailies".to_string(), true))
    );
    svc.archive(authored.id).await.unwrap();
    assert_eq!(
        super::resolve_selection(&db, Some(authored.id))
            .await
            .unwrap(),
        Some((default_id, "General Play".to_string(), true))
    );
}

// ── Lifetime aggregate ──────────────────────────────────────────────

/// One ended instance: a kill carrying loot, an armour cost standing in
/// for cycled spend, and a skill gain. Written raw so the aggregate is
/// exercised through the real summary pipeline rather than a hand-built
/// summary row.
#[allow(clippy::too_many_arguments)]
async fn seed_instance(
    db: &Db,
    id: &str,
    definition_id: Option<i64>,
    started: f64,
    ended: f64,
    cost: f64,
    loot: f64,
    skill_ped: f64,
) {
    let id = id.to_string();
    db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions \
             (id, started_at, ended_at, is_active, armour_cost, definition_id) \
             VALUES (?1, ?2, ?3, 0, ?4, ?5)",
            params![id, started, ended, cost, definition_id],
        )?;
        conn.execute(
            "INSERT INTO kills (id, session_id, mob_name, timestamp, loot_total_ped) \
             VALUES (?1, ?2, 'Atrox', ?3, ?4)",
            params![format!("{id}-k1"), id, started + 1.0, loot],
        )?;
        conn.execute(
            "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES (?1, ?2, 'Rifle', 1.0, ?3)",
            params![id, started + 1.0, skill_ped],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn lifetime_stats_sum_the_definitions_instances() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let definition = svc.create(input("ARIS Dailies", vec![])).await.unwrap();

    // Two instances of wildly different size. A three-hour grind that
    // returned 60% and a four-minute run that returned 500%.
    seed_instance(
        &db,
        "s1",
        Some(definition.id),
        0.0,
        10_800.0,
        100.0,
        60.0,
        5.0,
    )
    .await;
    seed_instance(
        &db,
        "s2",
        Some(definition.id),
        20_000.0,
        20_240.0,
        2.0,
        10.0,
        0.5,
    )
    .await;

    let stats = svc.lifetime_stats(definition.id).await.unwrap();
    assert_eq!(stats.instance_count, 2);
    assert!((stats.cycled - 102.0).abs() < 1e-9);
    assert!((stats.loot_tt - 70.0).abs() < 1e-9);
    assert!((stats.pes - 5.5).abs() < 1e-9);
    assert!((stats.duration_seconds - 11_040.0).abs() < 1e-6);

    // The whole point of summing the parts: the honest lifetime rate is
    // 70/102 = ~69%, NOT the mean of the two instance rates (60% and
    // 500%), which would read ~280% off four minutes of luck.
    let ratio_of_sums = stats.loot_tt / stats.cycled;
    assert!((ratio_of_sums - 0.686_274_5).abs() < 1e-6);
    let mean_of_rates = (60.0 / 100.0 + 10.0 / 2.0) / 2.0;
    assert!(mean_of_rates > 2.5, "the trap this aggregate avoids");
}

#[tokio::test]
async fn lifetime_stats_count_only_what_they_sum() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let definition = svc.create(input("ARIS Dailies", vec![])).await.unwrap();

    seed_instance(
        &db,
        "real",
        Some(definition.id),
        0.0,
        3600.0,
        10.0,
        8.0,
        1.0,
    )
    .await;
    // Started and abandoned: no cycled spend, so it never summarises and
    // must not inflate the span the surface discloses.
    seed_instance(
        &db,
        "cancelled",
        Some(definition.id),
        5000.0,
        5001.0,
        0.0,
        0.0,
        0.0,
    )
    .await;

    let stats = svc.lifetime_stats(definition.id).await.unwrap();
    assert_eq!(stats.instance_count, 1);
    assert!((stats.cycled - 10.0).abs() < 1e-9);
}

#[tokio::test]
async fn lifetime_stats_ignore_other_definitions_and_unattached_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, db) = service(dir.path()).await;
    let mine = svc.create(input("ARIS Dailies", vec![])).await.unwrap();
    let theirs = svc.create(input("General Hunting", vec![])).await.unwrap();

    seed_instance(&db, "mine", Some(mine.id), 0.0, 3600.0, 10.0, 8.0, 1.0).await;
    seed_instance(
        &db,
        "theirs",
        Some(theirs.id),
        4000.0,
        7600.0,
        99.0,
        99.0,
        9.0,
    )
    .await;
    // A legacy session, recorded before definitions existed.
    seed_instance(&db, "legacy", None, 8000.0, 11_600.0, 55.0, 55.0, 5.0).await;

    let stats = svc.lifetime_stats(mine.id).await.unwrap();
    assert_eq!(stats.instance_count, 1);
    assert!((stats.cycled - 10.0).abs() < 1e-9);
    assert!((stats.loot_tt - 8.0).abs() < 1e-9);
}

#[tokio::test]
async fn lifetime_stats_of_a_definition_never_run_are_all_zero() {
    let dir = tempfile::tempdir().unwrap();
    let (svc, _db) = service(dir.path()).await;
    let definition = svc.create(input("Fresh", vec![])).await.unwrap();

    let stats = svc.lifetime_stats(definition.id).await.unwrap();
    assert_eq!(stats, super::DefinitionLifetimeStats::default());
}
