use std::sync::Arc;

use super::*;
use crate::clock::MockClock;

pub(super) async fn harness() -> (tempfile::TempDir, Db, Arc<MockClock>, ProtectionService) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open(&dir.path().join("test.db"))
        .await
        .expect("open db");
    let clock = Arc::new(MockClock::new(None, 0.0));
    let service = ProtectionService::new(db.clone(), clock.clone());
    (dir, db, clock, service)
}

/// A played session: its hits, grouped by the context each landed in.
/// `None` is a hit recorded outside any context.
pub(super) struct Played<'a> {
    id: &'a str,
    started_at: f64,
    definition: Option<(i64, &'a str)>,
    hits: &'a [(Option<&'a str>, i64)],
}

impl<'a> Played<'a> {
    pub(super) fn new(id: &'a str, started_at: f64, hits: &'a [(Option<&'a str>, i64)]) -> Self {
        Self {
            id,
            started_at,
            definition: None,
            hits,
        }
    }

    pub(super) fn under(mut self, definition_id: i64, name: &'a str) -> Self {
        self.definition = Some((definition_id, name));
        self
    }
}

/// Record a finished session and its hits. Contexts are created per
/// label, so one label names one segment of the session.
pub(super) async fn play(db: &Db, played: Played<'_>) {
    let id = played.id.to_string();
    let started_at = played.started_at;
    let definition = played.definition.map(|(id, name)| (id, name.to_string()));
    let hits: Vec<(Option<String>, i64)> = played
        .hits
        .iter()
        .map(|(label, count)| (label.map(str::to_string), *count))
        .collect();
    db.with_writer(move |conn| {
        if let Some((definition_id, name)) = &definition {
            conn.execute(
                "INSERT OR IGNORE INTO session_definitions (id, name) VALUES (?1, ?2)",
                rusqlite::params![definition_id, name],
            )?;
        }
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active, definition_id) \
             VALUES (?1, ?2, ?3, 0, ?4)",
            rusqlite::params![
                id,
                started_at,
                started_at + 1.0,
                definition.as_ref().map(|(id, _)| *id)
            ],
        )?;
        let mut contexts = std::collections::BTreeMap::new();
        for (label, count) in hits {
            let context_id = label.map(|label| {
                *contexts.entry(label).or_insert_with(|| {
                    conn.execute(
                        "INSERT INTO session_contexts (session_id, created_at) VALUES (?1, ?2)",
                        rusqlite::params![id, started_at],
                    )
                    .expect("context");
                    conn.last_insert_rowid()
                })
            });
            for index in 0..count {
                // Alternate damage and deflection: both are one equal hit.
                let deflected = index % 2 == 1;
                conn.execute(
                    "INSERT INTO protection_defence_events \
                     (session_id, context_id, damage, deflected) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        id,
                        context_id,
                        (!deflected).then_some(25.0),
                        deflected as i64
                    ],
                )?;
            }
        }
        Ok(())
    })
    .await
    .expect("play session");
}

pub(super) async fn session_armour(db: &Db, session_id: &str) -> f64 {
    let session_id = session_id.to_string();
    db.with_reader(move |conn| {
        Ok(conn.query_row(
            "SELECT COALESCE(armour_cost, 0) FROM tracking_sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )?)
    })
    .await
    .expect("armour cost")
}

/// Each context's summed allocation for a session, oldest context first.
pub(super) async fn context_costs(db: &Db, session_id: &str) -> Vec<f64> {
    let session_id = session_id.to_string();
    db.with_reader(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT SUM(cost_ped) FROM protection_cost_context_allocations \
             WHERE session_id = ?1 GROUP BY context_key ORDER BY context_key",
        )?;
        let rows = stmt
            .query_map([session_id], |row| row.get::<_, f64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .await
    .expect("context costs")
}

pub(super) fn ids(candidates: &[CandidateSession]) -> Vec<&str> {
    candidates.iter().map(|c| c.session_id.as_str()).collect()
}

pub(super) fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

pub(super) async fn limited(service: &ProtectionService, name: &str, markup: f64) -> ProtectionSet {
    service
        .create_set(ProtectionSetKind::Armour, name, markup)
        .await
        .expect("create set")
}

#[tokio::test]
async fn one_session_repair_spreads_over_its_segments_by_hits() {
    let (_dir, db, _clock, service) = harness().await;
    play(
        &db,
        Played::new(
            "dailies",
            10.0,
            &[(Some("boss"), 40), (Some("mission"), 20), (None, 20)],
        ),
    )
    .await;

    let candidates = service
        .recording_candidates(ProtectionStream::Unlimited)
        .await
        .unwrap();
    assert_eq!(ids(&candidates.sessions), ["dailies"]);
    assert_eq!(candidates.sessions[0].hit_count, 80);
    assert!(candidates.since.is_none());
    assert!(candidates.earlier.is_empty());

    let outcome = service
        .confirm_repair_cost("r1", 8.0, vec!["dailies".into()])
        .await
        .unwrap();
    assert_eq!(outcome.cost_window.status, CostStatus::Booked);
    assert_eq!(outcome.cost_window.kind, CostKind::Repair);
    assert_eq!(outcome.cost_window.allocations.len(), 1);
    assert!(close(session_armour(&db, "dailies").await, 8.0));
    // The unassigned stretch sorts first (context key -1).
    let costs = context_costs(&db, "dailies").await;
    assert_eq!(costs.len(), 3);
    assert!(close(costs[0], 2.0));
    assert!(close(costs[1], 4.0));
    assert!(close(costs[2], 2.0));
}

#[tokio::test]
async fn streams_covering_one_session_add_up() {
    let (_dir, db, clock, service) = harness().await;
    let set = limited(&service, "Hyperion", 200.0).await;
    service
        .confirm_observation(
            set.id,
            "base",
            50.0,
            ObservationSource::Manual,
            None,
            None,
            vec![],
        )
        .await
        .unwrap();
    play(
        &db,
        Played::new(
            "dailies",
            10.0,
            &[
                (Some("boss"), 40),
                (Some("mission"), 20),
                (Some("free"), 20),
            ],
        ),
    )
    .await;
    clock.advance(60.0).unwrap();

    service
        .confirm_repair_cost("r1", 8.0, vec!["dailies".into()])
        .await
        .unwrap();
    let closing = service
        .confirm_observation(
            set.id,
            "close",
            42.0,
            ObservationSource::Manual,
            None,
            None,
            vec!["dailies".into()],
        )
        .await
        .unwrap();
    let window = closing.cost_window.expect("a measured loss");
    assert!(close(window.cost_ped, 16.0));
    assert_eq!(window.set_name.as_deref(), Some("Hyperion"));

    assert!(close(session_armour(&db, "dailies").await, 24.0));
    let costs = context_costs(&db, "dailies").await;
    assert!(close(costs[0], 12.0));
    assert!(close(costs[1], 6.0));
    assert!(close(costs[2], 6.0));
}

#[tokio::test]
async fn a_deferred_repair_spreads_over_only_the_ticked_sessions() {
    let (_dir, db, _clock, service) = harness().await;
    play(
        &db,
        Played::new("aris-1", 10.0, &[(None, 30)]).under(2, "ARIS Dailies"),
    )
    .await;
    play(
        &db,
        Played::new("trees", 20.0, &[(None, 50)]).under(3, "Tree Cutting"),
    )
    .await;
    play(
        &db,
        Played::new("aris-2", 30.0, &[(None, 10)]).under(2, "ARIS Dailies"),
    )
    .await;

    let candidates = service
        .recording_candidates(ProtectionStream::Unlimited)
        .await
        .unwrap();
    assert_eq!(ids(&candidates.sessions), ["aris-1", "trees", "aris-2"]);
    assert_eq!(
        candidates.sessions[1].definition_name.as_deref(),
        Some("Tree Cutting")
    );

    let outcome = service
        .confirm_repair_cost("r1", 4.0, vec!["aris-1".into(), "aris-2".into()])
        .await
        .unwrap();
    assert_eq!(outcome.cost_window.allocations.len(), 2);
    assert!(close(session_armour(&db, "aris-1").await, 3.0));
    assert!(close(session_armour(&db, "aris-2").await, 1.0));
    assert!(close(session_armour(&db, "trees").await, 0.0));
}

#[tokio::test]
async fn the_next_repair_looks_back_only_to_the_previous_one() {
    let (_dir, db, clock, service) = harness().await;
    play(&db, Played::new("first", 10.0, &[(None, 10)])).await;
    service
        .confirm_repair_cost("r1", 1.0, vec!["first".into()])
        .await
        .unwrap();
    clock.advance(60.0).unwrap();
    play(&db, Played::new("second", 100.0, &[(None, 5)])).await;

    let candidates = service
        .recording_candidates(ProtectionStream::Unlimited)
        .await
        .unwrap();
    assert_eq!(ids(&candidates.sessions), ["second"]);
    assert!(candidates.since.is_some());
    assert_eq!(ids(&candidates.earlier), ["first"]);
    assert!(candidates.earlier[0].covered);
    assert!(!candidates.sessions[0].covered);
}

/// A piece left unrepaired last time was worn in sessions an earlier
/// repair already covered. Re-including one adds to it without touching
/// what the earlier repair booked.
#[tokio::test]
async fn an_earlier_session_can_be_re_included_without_double_booking() {
    let (_dir, db, clock, service) = harness().await;
    play(&db, Played::new("first", 10.0, &[(None, 10)])).await;
    let earlier = service
        .confirm_repair_cost("r1", 2.0, vec!["first".into()])
        .await
        .unwrap();
    clock.advance(60.0).unwrap();
    play(&db, Played::new("second", 100.0, &[(None, 30)])).await;

    let later = service
        .confirm_repair_cost("r2", 4.0, vec!["first".into(), "second".into()])
        .await
        .unwrap();
    // The re-included session is weighed by all of its hits.
    let first = later
        .cost_window
        .allocations
        .iter()
        .find(|a| a.session_id == "first")
        .unwrap();
    assert_eq!(first.hit_count, 10);
    assert!(close(first.cost_ped, 1.0));
    assert!(close(session_armour(&db, "first").await, 3.0));
    assert!(close(session_armour(&db, "second").await, 3.0));

    let window_id = earlier.cost_window.id;
    let untouched: f64 = db
        .with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT SUM(cost_ped) FROM protection_cost_allocations WHERE window_id = ?1",
                [window_id],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(close(untouched, 2.0));
}

#[tokio::test]
async fn a_limited_set_measures_from_its_baseline_and_cannot_reach_past_it() {
    let (_dir, db, clock, service) = harness().await;
    let set = limited(&service, "Pegasus plates", 150.0).await;
    play(&db, Played::new("before", 10.0, &[(None, 10)])).await;

    let before_baseline = service
        .recording_candidates(ProtectionStream::Limited { set_id: set.id })
        .await
        .unwrap();
    assert!(before_baseline.sessions.is_empty());
    assert!(before_baseline.baseline_tt_ped.is_none());

    let baseline = service
        .confirm_observation(
            set.id,
            "b",
            30.0,
            ObservationSource::Ocr,
            Some("30.00"),
            None,
            vec![],
        )
        .await
        .unwrap();
    assert!(baseline.cost_window.is_none(), "a baseline books nothing");
    clock.advance(60.0).unwrap();
    play(&db, Played::new("after", 100.0, &[(None, 20)])).await;

    let candidates = service
        .recording_candidates(ProtectionStream::Limited { set_id: set.id })
        .await
        .unwrap();
    assert_eq!(ids(&candidates.sessions), ["after"]);
    assert!(
        candidates.earlier.is_empty(),
        "limited sets cannot re-cover"
    );
    assert_eq!(candidates.baseline_tt_ped, Some(30.0));

    let refused = service
        .confirm_observation(
            set.id,
            "c",
            28.0,
            ObservationSource::Manual,
            None,
            None,
            vec!["before".into()],
        )
        .await;
    assert!(matches!(refused, Err(ProtectionError::Invalid(_))));

    let measured = service
        .confirm_observation(
            set.id,
            "c",
            28.0,
            ObservationSource::Manual,
            None,
            None,
            vec!["after".into()],
        )
        .await
        .unwrap();
    let window = measured.cost_window.unwrap();
    assert!(close(window.consumed_tt_ped.unwrap(), 2.0));
    assert!(close(window.cost_ped, 3.0));
    assert!(close(session_armour(&db, "after").await, 3.0));
    assert!(close(session_armour(&db, "before").await, 0.0));
}

#[tokio::test]
async fn an_increased_reading_needs_a_reset_which_books_nothing() {
    let (_dir, db, clock, service) = harness().await;
    let set = limited(&service, "Hyperion", 180.0).await;
    service
        .confirm_observation(
            set.id,
            "b",
            20.0,
            ObservationSource::Manual,
            None,
            None,
            vec![],
        )
        .await
        .unwrap();
    clock.advance(60.0).unwrap();
    play(&db, Played::new("s", 100.0, &[(None, 5)])).await;

    let refused = service
        .confirm_observation(
            set.id,
            "up",
            25.0,
            ObservationSource::Manual,
            None,
            None,
            vec![],
        )
        .await;
    assert!(matches!(refused, Err(ProtectionError::Conflict(_))));

    let reset = service
        .confirm_observation(
            set.id,
            "up",
            25.0,
            ObservationSource::Manual,
            None,
            Some("Pieces replaced"),
            vec!["s".into()],
        )
        .await
        .unwrap();
    assert!(reset.cost_window.is_none());
    assert!(close(session_armour(&db, "s").await, 0.0));
    let after_reset = service
        .recording_candidates(ProtectionStream::Limited { set_id: set.id })
        .await
        .unwrap();
    assert!(
        after_reset.sessions.is_empty(),
        "the reset is the new baseline"
    );
    assert_eq!(after_reset.baseline_tt_ped, Some(25.0));
}

#[tokio::test]
async fn a_recording_with_no_session_ticked_stays_unattributed() {
    let (_dir, db, _clock, service) = harness().await;
    play(&db, Played::new("s", 10.0, &[(None, 5)])).await;
    let outcome = service
        .confirm_repair_cost("r1", 3.0, vec![])
        .await
        .unwrap();
    assert_eq!(outcome.cost_window.status, CostStatus::Pending);
    assert_eq!(
        outcome.cost_window.reason.as_deref(),
        Some(UNATTRIBUTED_REASON)
    );
    assert!(outcome.cost_window.allocations.is_empty());
    assert!(close(session_armour(&db, "s").await, 0.0));
}

#[tokio::test]
async fn confirmations_are_idempotent_on_their_token() {
    let (_dir, db, clock, service) = harness().await;
    play(&db, Played::new("s", 10.0, &[(None, 4)])).await;
    let first = service
        .confirm_repair_cost("same", 2.0, vec!["s".into()])
        .await
        .unwrap();
    let repeat = service
        .confirm_repair_cost("same", 9.0, vec!["s".into()])
        .await
        .unwrap();
    assert_eq!(first, repeat);
    assert!(close(session_armour(&db, "s").await, 2.0));

    let set = limited(&service, "Hyperion", 120.0).await;
    service
        .confirm_observation(
            set.id,
            "b",
            10.0,
            ObservationSource::Manual,
            None,
            None,
            vec![],
        )
        .await
        .unwrap();
    clock.advance(10.0).unwrap();
    play(&db, Played::new("t", 100.0, &[(None, 4)])).await;
    let closing = service
        .confirm_observation(
            set.id,
            "c",
            9.0,
            ObservationSource::Manual,
            None,
            None,
            vec!["t".into()],
        )
        .await
        .unwrap();
    let repeated = service
        .confirm_observation(
            set.id,
            "c",
            1.0,
            ObservationSource::Manual,
            None,
            None,
            vec!["t".into()],
        )
        .await
        .unwrap();
    assert_eq!(closing, repeated);
}

#[tokio::test]
async fn unknown_sessions_and_sessions_without_hits_are_refused() {
    let (_dir, db, _clock, service) = harness().await;
    play(&db, Played::new("quiet", 10.0, &[])).await;
    let unknown = service
        .confirm_repair_cost("r1", 1.0, vec!["missing".into()])
        .await;
    assert!(matches!(unknown, Err(ProtectionError::Invalid(_))));
    let quiet = service
        .confirm_repair_cost("r2", 1.0, vec!["quiet".into()])
        .await;
    assert!(matches!(quiet, Err(ProtectionError::Invalid(_))));
    let windows: i64 = db
        .with_reader(|conn| {
            Ok(
                conn.query_row("SELECT COUNT(*) FROM protection_cost_windows", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(windows, 0, "a refusal writes nothing");
}

#[tokio::test]
async fn unrecorded_hits_clear_once_any_recording_covers_the_session() {
    let (_dir, db, _clock, service) = harness().await;
    play(&db, Played::new("a", 10.0, &[(None, 3)])).await;
    play(&db, Played::new("b", 20.0, &[(None, 7)])).await;

    let overview = service.overview().await.unwrap();
    assert_eq!(overview.unrecorded.sessions, 2);
    assert_eq!(overview.unrecorded.hits, 10);
    assert_eq!(service.session_unrecorded_hits("b").await.unwrap(), 7);

    service
        .confirm_repair_cost("r1", 1.0, vec!["b".into()])
        .await
        .unwrap();
    let overview = service.overview().await.unwrap();
    assert_eq!(overview.unrecorded.sessions, 1);
    assert_eq!(overview.unrecorded.hits, 3);
    assert_eq!(service.session_unrecorded_hits("b").await.unwrap(), 0);
    assert_eq!(overview.recent_cost_windows.len(), 1);
}

#[tokio::test]
async fn a_set_markup_freezes_at_its_first_reading() {
    let (_dir, _db, _clock, service) = harness().await;
    let set = limited(&service, "Hyperion", 120.0).await;
    let corrected = service.update_set(set.id, "Hyperion", 130.0).await.unwrap();
    assert!(close(corrected.markup_percent, 130.0));
    service
        .confirm_observation(
            set.id,
            "b",
            10.0,
            ObservationSource::Manual,
            None,
            None,
            vec![],
        )
        .await
        .unwrap();
    let locked = service.update_set(set.id, "Hyperion", 140.0).await;
    assert!(matches!(locked, Err(ProtectionError::Conflict(_))));
    let renamed = service
        .update_set(set.id, "Hyperion (L)", 130.0)
        .await
        .unwrap();
    assert_eq!(renamed.name, "Hyperion (L)");

    service.archive_set(set.id).await.unwrap();
    assert!(service.overview().await.unwrap().sets.is_empty());
    assert!(matches!(
        service
            .recording_candidates(ProtectionStream::Limited { set_id: set.id })
            .await,
        Err(ProtectionError::NotFound(_))
    ));
}

/// A session no recording of any stream reaches is flagged as such in the
/// earlier list, apart from being uncovered by the stream at hand.
#[tokio::test]
async fn earlier_sessions_say_whether_any_recording_reaches_them() {
    let (_dir, db, clock, service) = harness().await;
    let set = limited(&service, "Hyperion", 150.0).await;
    service
        .confirm_observation(
            set.id,
            "b",
            20.0,
            ObservationSource::Manual,
            None,
            None,
            vec![],
        )
        .await
        .unwrap();
    play(&db, Played::new("limited-only", 10.0, &[(None, 4)])).await;
    play(&db, Played::new("forgotten", 20.0, &[(None, 6)])).await;
    play(&db, Played::new("repaired", 30.0, &[(None, 2)])).await;
    clock.advance(10.0).unwrap();
    service
        .confirm_observation(
            set.id,
            "c",
            19.0,
            ObservationSource::Manual,
            None,
            None,
            vec!["limited-only".into()],
        )
        .await
        .unwrap();
    service
        .confirm_repair_cost("r1", 1.0, vec!["repaired".into()])
        .await
        .unwrap();
    clock.advance(10.0).unwrap();
    play(&db, Played::new("new", 100.0, &[(None, 3)])).await;

    let candidates = service
        .recording_candidates(ProtectionStream::Unlimited)
        .await
        .unwrap();
    assert_eq!(ids(&candidates.sessions), ["new"]);
    assert!(candidates.sessions[0].unrecorded);
    let flags: Vec<(&str, bool, bool)> = candidates
        .earlier
        .iter()
        .map(|c| (c.session_id.as_str(), c.covered, c.unrecorded))
        .collect();
    assert_eq!(
        flags,
        [
            ("limited-only", false, false),
            ("forgotten", false, true),
            ("repaired", true, false),
        ]
    );
}

/// Each stream says how far behind it is, so Equipment can show when it
/// was last recorded and what has been played since.
#[tokio::test]
async fn every_stream_reports_the_play_since_its_last_recording() {
    let (_dir, db, clock, service) = harness().await;
    play(&db, Played::new("a", 10.0, &[(None, 3)])).await;
    play(&db, Played::new("b", 20.0, &[(None, 4)])).await;

    let before = service.overview().await.unwrap().unlimited;
    assert_eq!(
        before,
        StreamBacklog {
            last_recorded_at: None,
            sessions: 2,
            hits: 7
        }
    );

    service
        .confirm_repair_cost("r1", 1.0, vec!["a".into(), "b".into()])
        .await
        .unwrap();
    let after = service.overview().await.unwrap().unlimited;
    assert!(after.last_recorded_at.is_some());
    assert_eq!((after.sessions, after.hits), (0, 0));

    let set = limited(&service, "Hyperion", 150.0).await;
    let unread = service.overview().await.unwrap();
    assert_eq!(
        unread.sets[0].backlog,
        StreamBacklog::default(),
        "no reading, no backlog"
    );
    service
        .confirm_observation(
            set.id,
            "b",
            20.0,
            ObservationSource::Manual,
            None,
            None,
            vec![],
        )
        .await
        .unwrap();
    clock.advance(10.0).unwrap();
    play(&db, Played::new("c", 100.0, &[(None, 5)])).await;

    let overview = service.overview().await.unwrap();
    assert_eq!(
        (overview.unlimited.sessions, overview.unlimited.hits),
        (1, 5)
    );
    assert_eq!(
        (
            overview.sets[0].backlog.sessions,
            overview.sets[0].backlog.hits
        ),
        (1, 5)
    );
    assert!(overview.sets[0].backlog.last_recorded_at.is_some());
}

mod conservation {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        /// Whatever the hit distribution, a recording's stored allocations
        /// sum to its cost, never go negative, and give each session a
        /// share proportional to its hits.
        #[test]
        fn a_recording_conserves_its_total(
            sessions in prop::collection::vec(
                prop::collection::vec(1i64..40, 1..4),
                1..5,
            ),
            cost_cents in 0u32..100_000,
        ) {
            let cost = f64::from(cost_cents) / 100.0;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let (_dir, db, _clock, service) = harness().await;
                let labels = ["a", "b", "c"];
                let mut chosen = Vec::new();
                let mut hits_by_session = Vec::new();
                for (index, contexts) in sessions.iter().enumerate() {
                    let id = format!("s{index}");
                    let hits: Vec<(Option<&str>, i64)> = contexts
                        .iter()
                        .enumerate()
                        .map(|(i, count)| (Some(labels[i]), *count))
                        .collect();
                    play(&db, Played::new(&id, index as f64 * 10.0, &hits)).await;
                    hits_by_session.push((id.clone(), contexts.iter().sum::<i64>()));
                    chosen.push(id);
                }
                let outcome = service
                    .confirm_repair_cost("r", cost, chosen)
                    .await
                    .unwrap();
                let total_hits: i64 = hits_by_session.iter().map(|(_, h)| h).sum();
                let allocated: f64 = outcome.cost_window.allocations.iter().map(|a| a.cost_ped).sum();
                prop_assert!((allocated - cost).abs() < 1e-6);
                for allocation in &outcome.cost_window.allocations {
                    prop_assert!(allocation.cost_ped >= 0.0);
                    let hits = hits_by_session
                        .iter()
                        .find(|(id, _)| *id == allocation.session_id)
                        .unwrap()
                        .1;
                    prop_assert_eq!(allocation.hit_count, hits);
                    let expected = cost * hits as f64 / total_hits as f64;
                    prop_assert!((allocation.cost_ped - expected).abs() < 1e-6);
                }
                let context_total: f64 = db
                    .with_reader(|conn| {
                        Ok(conn.query_row(
                            "SELECT COALESCE(SUM(cost_ped), 0) FROM protection_cost_context_allocations",
                            [],
                            |row| row.get(0),
                        )?)
                    })
                    .await
                    .unwrap();
                prop_assert!((context_total - cost).abs() < 1e-6);
                Ok(())
            })?;
        }
    }
}
