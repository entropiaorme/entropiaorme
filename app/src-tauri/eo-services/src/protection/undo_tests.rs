//! Undoing recordings, restoring removed sets, and the session-list read
//! of which sessions still await a recording.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::tests::{close, context_costs, harness, ids, limited, play, session_armour, Played};
use super::*;

/// The materialised summary's armour cost; a session never summarised
/// reads as zero, as the list does.
async fn summary_armour(db: &Db, session_id: &str) -> f64 {
    use rusqlite::OptionalExtension;

    let session_id = session_id.to_string();
    db.with_reader(move |conn| {
        Ok(conn
            .query_row(
                "SELECT COALESCE(armour_cost, 0) FROM session_summaries WHERE session_id = ?1",
                [session_id],
                |row| row.get::<_, f64>(0),
            )
            .optional()?
            .unwrap_or(0.0))
    })
    .await
    .expect("summary armour cost")
}

async fn reading(
    service: &ProtectionService,
    set_id: i64,
    token: &str,
    tt: f64,
    sessions: &[&str],
) -> ObservationOutcome {
    service
        .confirm_observation(
            set_id,
            token,
            tt,
            ObservationSource::Manual,
            None,
            None,
            sessions.iter().map(|id| id.to_string()).collect(),
        )
        .await
        .expect("reading")
}

#[tokio::test]
async fn undoing_a_repair_gives_every_share_back_and_offers_its_sessions_again() {
    let (_dir, db, clock, service) = harness().await;
    play(
        &db,
        Played::new("a", 10.0, &[(Some("boss"), 30), (None, 10)]),
    )
    .await;
    play(&db, Played::new("b", 20.0, &[(None, 20)])).await;
    let before = service
        .recording_candidates(ProtectionStream::Unlimited)
        .await
        .unwrap();

    let repair = service
        .confirm_repair_cost("r1", 6.0, vec!["a".into(), "b".into()])
        .await
        .unwrap()
        .cost_window;
    assert!(close(session_armour(&db, "a").await, 4.0));
    assert!(close(summary_armour(&db, "a").await, 4.0));
    clock.advance(30.0).unwrap();

    service
        .undo(UndoTarget::Recording {
            window_id: repair.id,
        })
        .await
        .unwrap();

    for session in ["a", "b"] {
        assert_eq!(session_armour(&db, session).await, 0.0, "{session}");
        assert_eq!(summary_armour(&db, session).await, 0.0, "{session}");
    }
    let after = service
        .recording_candidates(ProtectionStream::Unlimited)
        .await
        .unwrap();
    assert_eq!(after, before, "the stream falls back to where it was");
    assert_eq!(
        service.session_unrecorded_hits("a").await.unwrap(),
        40,
        "undone cost no longer covers the session"
    );

    // Provenance survives, marked as undone and no longer undoable.
    let overview = service.overview().await.unwrap();
    let kept = &overview.recent_cost_windows[0];
    assert_eq!(kept.id, repair.id);
    assert!(kept.superseded_at.is_some());
    assert!(!kept.undoable);
    assert_eq!(kept.allocations.len(), 2);
    assert_eq!(overview.unrecorded.sessions, 2);

    // Recording afresh is the correction.
    service
        .confirm_repair_cost("r2", 3.0, vec!["a".into(), "b".into()])
        .await
        .unwrap();
    assert!(close(session_armour(&db, "a").await, 2.0));
    assert!(close(session_armour(&db, "b").await, 1.0));
    let costs = context_costs(&db, "a").await;
    assert_eq!(costs.len(), 2, "undone and live shares both kept");
}

#[tokio::test]
async fn only_the_latest_recording_of_a_stream_can_be_undone() {
    let (_dir, db, clock, service) = harness().await;
    play(&db, Played::new("a", 10.0, &[(None, 10)])).await;
    let first = service
        .confirm_repair_cost("r1", 1.0, vec!["a".into()])
        .await
        .unwrap()
        .cost_window;
    clock.advance(10.0).unwrap();
    play(&db, Played::new("b", 20.0, &[(None, 10)])).await;
    let second = service
        .confirm_repair_cost("r2", 2.0, vec!["b".into()])
        .await
        .unwrap()
        .cost_window;

    let overview = service.overview().await.unwrap();
    let undoable: Vec<(i64, bool)> = overview
        .recent_cost_windows
        .iter()
        .map(|window| (window.id, window.undoable))
        .collect();
    assert_eq!(undoable, [(second.id, true), (first.id, false)]);

    let refused = service
        .undo(UndoTarget::Recording {
            window_id: first.id,
        })
        .await;
    assert!(matches!(refused, Err(ProtectionError::Conflict(_))));
    assert!(close(session_armour(&db, "a").await, 1.0), "nothing moved");

    service
        .undo(UndoTarget::Recording {
            window_id: second.id,
        })
        .await
        .unwrap();
    let again = service
        .undo(UndoTarget::Recording {
            window_id: second.id,
        })
        .await;
    assert!(matches!(again, Err(ProtectionError::Conflict(_))));

    // With the second gone, the first is the latest again.
    service
        .undo(UndoTarget::Recording {
            window_id: first.id,
        })
        .await
        .unwrap();
    assert_eq!(session_armour(&db, "a").await, 0.0);
    let missing = service
        .undo(UndoTarget::Recording { window_id: 9_999 })
        .await;
    assert!(matches!(missing, Err(ProtectionError::NotFound(_))));
}

#[tokio::test]
async fn undoing_a_limited_reading_restores_its_baseline() {
    let (_dir, db, clock, service) = harness().await;
    let set = limited(&service, "Hyperion", 200.0).await;
    let base = reading(&service, set.id, "base", 50.0, &[]).await;
    play(&db, Played::new("a", 10.0, &[(None, 10)])).await;
    clock.advance(60.0).unwrap();

    let measured = reading(&service, set.id, "close", 45.0, &["a"]).await;
    let window = measured.cost_window.expect("a measured loss");
    assert!(close(session_armour(&db, "a").await, 10.0));
    let set_row = service.overview().await.unwrap().sets.remove(0);
    assert!(set_row.latest_observation.as_ref().unwrap().measured);

    // A reading that booked a cost is undone through its cost.
    let refused = service
        .undo(UndoTarget::Reading {
            observation_id: measured.observation.id,
        })
        .await;
    assert!(matches!(refused, Err(ProtectionError::Conflict(_))));

    service
        .undo(UndoTarget::Recording {
            window_id: window.id,
        })
        .await
        .unwrap();
    assert_eq!(session_armour(&db, "a").await, 0.0);
    let candidates = service
        .recording_candidates(ProtectionStream::Limited { set_id: set.id })
        .await
        .unwrap();
    assert_eq!(candidates.baseline_tt_ped, Some(50.0));
    assert_eq!(ids(&candidates.sessions), ["a"]);
    let set_row = service.overview().await.unwrap().sets.remove(0);
    let latest = set_row.latest_observation.expect("the baseline again");
    assert_eq!(latest.id, base.observation.id);
    assert!(!latest.measured);

    // A mistyped reading is corrected by recording it again.
    let corrected = reading(&service, set.id, "close-2", 48.0, &["a"]).await;
    assert!(close(corrected.cost_window.unwrap().cost_ped, 4.0));
    assert!(close(session_armour(&db, "a").await, 4.0));
}

#[tokio::test]
async fn undoing_a_first_reading_unlocks_the_markup_again() {
    let (_dir, _db, _clock, service) = harness().await;
    let set = limited(&service, "Hyperion", 200.0).await;
    let base = reading(&service, set.id, "base", 50.0, &[]).await;
    assert!(
        service.update_set(set.id, "Hyperion", 180.0).await.is_err(),
        "frozen by the reading"
    );

    service
        .undo(UndoTarget::Reading {
            observation_id: base.observation.id,
        })
        .await
        .unwrap();
    let set_row = service.overview().await.unwrap().sets.remove(0);
    assert!(set_row.latest_observation.is_none());
    assert!(!set_row.basis_locked);
    service.update_set(set.id, "Hyperion", 180.0).await.unwrap();

    // The next reading is a fresh baseline.
    let next = reading(&service, set.id, "base-2", 49.0, &[]).await;
    assert!(next.cost_window.is_none());
}

#[tokio::test]
async fn a_removed_set_can_be_restored_unless_its_name_is_taken() {
    let (_dir, _db, _clock, service) = harness().await;
    let set = limited(&service, "Hyperion", 200.0).await;
    reading(&service, set.id, "base", 50.0, &[]).await;
    service.archive_set(set.id).await.unwrap();

    let overview = service.overview().await.unwrap();
    assert!(overview.sets.is_empty());
    assert_eq!(overview.removed_sets.len(), 1);
    assert_eq!(overview.removed_sets[0].id, set.id);

    let restored = service.restore_set(set.id).await.unwrap();
    assert!(restored.archived_at.is_none());
    assert!(restored.basis_locked, "its readings came back with it");
    assert_eq!(restored.latest_observation.unwrap().tt_value_ped, 50.0);
    assert!(service.overview().await.unwrap().removed_sets.is_empty());

    service.archive_set(set.id).await.unwrap();
    limited(&service, "hyperion", 150.0).await;
    let taken = service.restore_set(set.id).await;
    assert!(matches!(taken, Err(ProtectionError::Conflict(_))));
    let absent = service.restore_set(9_999).await;
    assert!(matches!(absent, Err(ProtectionError::NotFound(_))));
}

#[tokio::test]
async fn a_recording_lists_each_sessions_share_by_segment() {
    let (_dir, db, _clock, service) = harness().await;
    play(
        &db,
        Played::new("dailies", 10.0, &[(Some("boss"), 30), (None, 10)]).under(2, "ARIS Dailies"),
    )
    .await;
    // Name the boss stretch as the tracker would: a segment interval.
    db.with_writer(|conn| {
        conn.execute(
            "INSERT INTO session_intervals (session_id, kind, label, started_at) \
             VALUES ('dailies', 'segment', 'Boss room', 10.0)",
            [],
        )?;
        let interval = conn.last_insert_rowid();
        // id-order: insertion (the session's only context is its boss stretch).
        conn.execute(
            "INSERT INTO session_context_intervals (context_id, interval_id) \
             SELECT MIN(id), ?1 FROM session_contexts WHERE session_id = 'dailies'",
            [interval],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let window = service
        .confirm_repair_cost("r1", 8.0, vec!["dailies".into()])
        .await
        .unwrap()
        .cost_window;
    let allocation = &window.allocations[0];
    assert_eq!(allocation.definition_name.as_deref(), Some("ARIS Dailies"));
    assert_eq!(allocation.started_at, 10.0);
    let contexts: Vec<(Option<&str>, i64)> = allocation
        .contexts
        .iter()
        .map(|context| (context.label.as_deref(), context.hit_count))
        .collect();
    assert_eq!(contexts, [(None, 10), (Some("Boss room"), 30)]);
    let context_total: f64 = allocation.contexts.iter().map(|c| c.cost_ped).sum();
    assert!(close(context_total, 8.0));
}

#[tokio::test]
async fn a_session_list_learns_which_sessions_await_a_recording() {
    let (_dir, db, _clock, service) = harness().await;
    play(&db, Played::new("a", 10.0, &[(None, 3)])).await;
    play(&db, Played::new("b", 20.0, &[(None, 4)])).await;
    play(&db, Played::new("quiet", 30.0, &[])).await;
    let page = vec!["b".to_string(), "quiet".into(), "a".into(), "absent".into()];

    let before = service
        .sessions_with_unrecorded_hits(page.clone())
        .await
        .unwrap();
    assert_eq!(before, ["b", "a"]);

    let repair = service
        .confirm_repair_cost("r1", 1.0, vec!["a".into()])
        .await
        .unwrap()
        .cost_window;
    assert_eq!(
        service
            .sessions_with_unrecorded_hits(page.clone())
            .await
            .unwrap(),
        ["b"]
    );
    service
        .undo(UndoTarget::Recording {
            window_id: repair.id,
        })
        .await
        .unwrap();
    assert_eq!(
        service.sessions_with_unrecorded_hits(page).await.unwrap(),
        ["b", "a"]
    );
    assert!(service
        .sessions_with_unrecorded_hits(Vec::new())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn every_committed_write_is_announced_and_a_refusal_is_not() {
    let (_dir, db, _clock, service) = harness().await;
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let service = service.with_changed(Arc::new(move || {
        seen.fetch_add(1, Ordering::SeqCst);
    }));
    play(&db, Played::new("a", 10.0, &[(None, 3)])).await;

    let set = limited(&service, "Hyperion", 200.0).await;
    service
        .update_set(set.id, "Hyperion II", 200.0)
        .await
        .unwrap();
    let repair = service
        .confirm_repair_cost("r1", 1.0, vec!["a".into()])
        .await
        .unwrap()
        .cost_window;
    reading(&service, set.id, "base", 50.0, &[]).await;
    assert_eq!(count.load(Ordering::SeqCst), 4);

    let refused = service
        .confirm_repair_cost("r2", 1.0, vec!["absent".into()])
        .await;
    assert!(refused.is_err());
    assert!(service
        .undo(UndoTarget::Recording { window_id: 9_999 })
        .await
        .is_err());
    service
        .confirm_repair_cost("r1", 1.0, vec!["a".into()])
        .await
        .unwrap();
    reading(&service, set.id, "base", 50.0, &[]).await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        4,
        "refusals and replays change nothing"
    );

    service
        .undo(UndoTarget::Recording {
            window_id: repair.id,
        })
        .await
        .unwrap();
    service.archive_set(set.id).await.unwrap();
    service.restore_set(set.id).await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 7);
}

mod reversal {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]

        /// Undoing any latest recording leaves every session's cost, its
        /// summary, and the stream's candidates exactly as they were
        /// before it, whatever the hits and whichever sessions were ticked.
        #[test]
        fn undo_is_an_exact_reversal(
            sessions in prop::collection::vec(1i64..40, 1..5),
            ticked in prop::collection::vec(any::<bool>(), 5),
            earlier_cents in 0u32..10_000,
            cost_cents in 0u32..100_000,
        ) {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let (_dir, db, clock, service) = harness().await;
                let mut ids = Vec::new();
                for (index, hits) in sessions.iter().enumerate() {
                    let id = format!("s{index}");
                    let played = [(None, *hits)];
                    play(&db, Played::new(&id, index as f64 * 10.0, &played)).await;
                    ids.push(id);
                }
                // An earlier recording over the first session, which the
                // undo must leave untouched.
                service
                    .confirm_repair_cost("earlier", f64::from(earlier_cents) / 100.0, vec![ids[0].clone()])
                    .await
                    .unwrap();
                clock.advance(10.0).unwrap();
                play(&db, Played::new("late", 100.0, &[(None, 7)])).await;
                ids.push("late".into());

                let mut before = Vec::new();
                for id in &ids {
                    before.push((session_armour(&db, id).await, summary_armour(&db, id).await));
                }
                let candidates = service
                    .recording_candidates(ProtectionStream::Unlimited)
                    .await
                    .unwrap();
                let chosen: Vec<String> = candidates
                    .sessions
                    .iter()
                    .chain(&candidates.earlier)
                    .zip(&ticked)
                    .filter(|(_, tick)| **tick)
                    .map(|(candidate, _)| candidate.session_id.clone())
                    .collect();
                let window = service
                    .confirm_repair_cost("latest", f64::from(cost_cents) / 100.0, chosen)
                    .await
                    .unwrap()
                    .cost_window;
                service
                    .undo(UndoTarget::Recording { window_id: window.id })
                    .await
                    .unwrap();

                for (id, (armour, summary)) in ids.iter().zip(before) {
                    prop_assert!((session_armour(&db, id).await - armour).abs() < 1e-9);
                    prop_assert!((summary_armour(&db, id).await - summary).abs() < 1e-9);
                }
                let after = service
                    .recording_candidates(ProtectionStream::Unlimited)
                    .await
                    .unwrap();
                prop_assert_eq!(after, candidates);
                Ok(())
            })?;
        }
    }
}
