//! Recordings beside the account readings: a session whose armour the
//! readings measured is never covered by a recording, every session they
//! did not measure stays coverable, what a recording measures that the
//! readings already billed is taken off, a limited set measures from its
//! pieces in an account reading, and a set's pieces are named once and
//! belong to a set by time.

use super::tests::{close, harness, ids, limited, play, Played};
use super::*;
use crate::account::reading::Container;
use crate::account::store::{record_reading, Purpose};
use crate::account::test_support::reading;
use crate::session_costs::{
    self, Bucket, CostBasis, MEASURED_COSTS_VERSION, MEASURED_MEASURE_VERSION,
};

const C: Container = Container::Carried;
const HELMET: &str = "Pegasus Helmet (M,L)";
const HARNESS: &str = "Pegasus Harness (M,L)";

/// Store a session's measured costs as the account side does at seal, its
/// armour measured in parts: the unlimited armour at `unlimited` PED and
/// each `(set, cost in PED)`; then bring its figures up to date.
async fn measure(db: &Db, id: &str, unlimited: f64, sets: &[(i64, f64)]) {
    measure_with(db, id, unlimited, sets, None).await;
}

/// [`measure`], with `unpriced` PED of TT lost by limited pieces no
/// confirmed set holds, which leaves the armour bucket waiting.
async fn measure_with(
    db: &Db,
    id: &str,
    unlimited: f64,
    sets: &[(i64, f64)],
    unpriced: Option<f64>,
) {
    let id = id.to_string();
    let sets = sets.to_vec();
    db.with_writer(move |conn| {
        let micro = |ped: f64| (ped * 1_000_000.0).round() as i64;
        conn.execute(
            "INSERT INTO account_session_costs (session_id, costs_version, measure_version, \
             measure_computed_at, status, reason, computed_at) \
             VALUES (?1, ?2, ?3, 1.0, 'measured', NULL, 2.0)",
            rusqlite::params![id, MEASURED_COSTS_VERSION, MEASURED_MEASURE_VERSION],
        )?;
        let armour = unlimited + sets.iter().map(|(_, cost)| cost).sum::<f64>();
        for bucket in [
            "weapon",
            "enhancer",
            "harvesting",
            "healing",
            "armour",
            "consumables",
            "travel",
            "fieldLogistics",
            "unclassified",
        ] {
            if bucket == "armour" && unpriced.is_some() {
                conn.execute(
                    "INSERT INTO account_session_cost_buckets \
                     (session_id, bucket, basis, reason, amount_micro_ped, precision) \
                     VALUES (?1, 'armour', 'unmeasured', 'limitedSetUnconfirmed', NULL, NULL)",
                    [&id],
                )?;
                continue;
            }
            let amount = if bucket == "armour" { armour } else { 0.0 };
            conn.execute(
                "INSERT INTO account_session_cost_buckets \
                 (session_id, bucket, basis, reason, amount_micro_ped, precision) \
                 VALUES (?1, ?2, 'measured', NULL, ?3, 'exact')",
                rusqlite::params![id, bucket, micro(amount)],
            )?;
        }
        if let Some(lost) = unpriced {
            conn.execute(
                "INSERT INTO account_session_cost_armour_parts \
                 (session_id, part, set_id, lost_micro_ped, lost_precision, markup_hundredths, \
                  amount_micro_ped, precision) \
                 VALUES (?1, 'unconfirmed', NULL, ?2, 'exact', NULL, NULL, NULL)",
                rusqlite::params![id, micro(lost)],
            )?;
        }
        conn.execute(
            "INSERT INTO account_session_cost_armour_parts \
             (session_id, part, set_id, lost_micro_ped, lost_precision, markup_hundredths, \
              amount_micro_ped, precision) \
             VALUES (?1, 'unlimited', NULL, ?2, 'exact', NULL, ?2, 'exact')",
            rusqlite::params![id, micro(unlimited)],
        )?;
        for (set_id, cost) in sets {
            conn.execute(
                "INSERT INTO account_session_cost_armour_parts \
                 (session_id, part, set_id, lost_micro_ped, lost_precision, markup_hundredths, \
                  amount_micro_ped, precision) \
                 VALUES (?1, 'limitedSet', ?2, ?3, 'exact', 10000, ?3, 'exact')",
                rusqlite::params![id, set_id, micro(cost)],
            )?;
        }
        session_costs::refresh_derived(conn, &id)?;
        Ok(())
    })
    .await
    .expect("measured costs stored");
}

/// Store an account reading at `at` holding `rows` of
/// `(name, quantity, value in PEC)`, carried.
async fn read_account(db: &Db, at: f64, rows: &[(&'static str, i64, i64)]) {
    let rows: Vec<(&'static str, Container, i64, i64)> = rows
        .iter()
        .map(|(name, quantity, value)| (*name, C, *quantity, *value))
        .collect();
    db.with_writer(move |conn| {
        record_reading(conn, &reading(at, 10_000, &rows), Purpose::Manual, None)
            .expect("a stored reading");
        Ok(())
    })
    .await
    .expect("reading stored");
}

/// Play the session `id` from `at` with ten hits, read whole: an account
/// reading holding `before` just ahead of it and one holding `after` at its
/// stop, as `(name, value in PEC)` of single carried items; then seal its
/// measured costs from them.
async fn read_session(
    db: &Db,
    id: &str,
    at: f64,
    before: &[(&'static str, i64)],
    after: &[(&'static str, i64)],
) {
    play(db, Played::new(id, at, &[(None, 10)])).await;
    let row = |(name, value): &(&'static str, i64)| (*name, C, 1, *value);
    let before: Vec<_> = before.iter().map(row).collect();
    let after: Vec<_> = after.iter().map(row).collect();
    let id = id.to_string();
    db.with_writer(move |conn| {
        record_reading(
            conn,
            &reading(at - 5.0, 10_000, &before),
            Purpose::Manual,
            None,
        )
        .expect("the reading ahead of it");
        record_reading(
            conn,
            &reading(at + 1.0, 10_000, &after),
            Purpose::Session,
            Some(&id),
        )
        .expect("the reading at its stop");
        crate::account::costs::session_costs(conn, &id, None, 4_000_000_000.0, None)
            .expect("its costs")
            .expect("a known session");
        Ok(())
    })
    .await
    .expect("session read");
}

/// A session's stored armour parts, as the readings billed them.
async fn armour_parts(db: &Db, id: &str) -> Vec<crate::account::costs::ArmourPart> {
    let id = id.to_string();
    db.with_writer(move |conn| {
        Ok(
            crate::account::costs::session_costs(conn, &id, None, 4_000_000_000.0, None)
                .expect("its costs")
                .expect("a known session")
                .armour,
        )
    })
    .await
    .expect("armour parts")
}

async fn resolved_armour(db: &Db, id: &str) -> session_costs::SessionCosts {
    let id = id.to_string();
    db.with_reader(move |conn| Ok(session_costs::resolve(conn, &id)?))
        .await
        .expect("resolved costs")
}

fn message(error: ProtectionError) -> &'static str {
    match error {
        ProtectionError::Invalid(message) | ProtectionError::Conflict(message) => message,
        other => panic!("not a refusal: {other:?}"),
    }
}

const PLATE: &str = "Armor Plating Mk. 6A";

#[tokio::test]
async fn a_repair_after_a_measured_session_bills_only_what_the_readings_did_not() {
    let (_dir, db, _clock, service) = harness().await;
    // Unmeasured, then measured: the plate lost 1.13 in the second.
    play(&db, Played::new("u1", 10.0, &[(None, 10)])).await;
    read_session(&db, "m2", 100.0, &[(PLATE, 2_500)], &[(PLATE, 2_387)]).await;

    let candidates = service
        .recording_candidates(ProtectionStream::Unlimited)
        .await
        .unwrap();
    // The unmeasured session before the measured one stays coverable.
    assert_eq!(ids(&candidates.sessions), ["u1"]);
    assert_eq!(candidates.measured_sessions, 1);
    assert!(close(candidates.billed_from_account_ped, 1.13));
    assert!(close(candidates.account_tolerance_ped, 0.01));

    let all_billed = service
        .confirm_repair_cost("r0", 1.13, vec!["u1".into()])
        .await
        .unwrap_err();
    assert_eq!(
        message(all_billed),
        "Everything this repair restored was already billed from your account"
    );
    let below = service
        .confirm_repair_cost("r0", 1.0, vec!["u1".into()])
        .await
        .unwrap_err();
    assert_eq!(
        message(below),
        "This repair is less than the decay already billed from your account since the last repair"
    );
    let measured = service
        .confirm_repair_cost("r0", 2.0, vec!["m2".into()])
        .await
        .unwrap_err();
    assert_eq!(
        message(measured),
        "A chosen session's armour was measured from your account"
    );

    // 2.00 restored: 1.13 already billed by the measured session, 0.87
    // for the unmeasured one.
    let outcome = service
        .confirm_repair_cost("r1", 2.0, vec!["u1".into()])
        .await
        .unwrap();
    assert!(close(outcome.cost_window.cost_ped, 0.87));
    assert_eq!(outcome.cost_window.allocations.len(), 1);
    assert!(close(outcome.cost_window.allocations[0].cost_ped, 0.87));
    assert!(close(
        resolved_armour(&db, "m2").await.resolved.armour,
        1.13
    ));
    assert!(close(
        resolved_armour(&db, "u1").await.resolved.armour,
        0.87
    ));
}

#[tokio::test]
async fn a_repair_seen_as_it_happened_settles_the_decay_before_it() {
    let (_dir, db, _clock, service) = harness().await;
    play(&db, Played::new("u1", 10.0, &[(None, 10)])).await;
    read_session(&db, "m2", 100.0, &[(PLATE, 2_500)], &[(PLATE, 2_387)]).await;
    // The repair, seen in the readings after the session, with no play
    // after it: it is the repair being recorded.
    read_account(&db, 150.0, &[(PLATE, 1, 2_500)]).await;

    let candidates = service
        .recording_candidates(ProtectionStream::Unlimited)
        .await
        .unwrap();
    assert!(close(candidates.billed_from_account_ped, 1.13));
    let outcome = service
        .confirm_repair_cost("r", 2.0, vec!["u1".into()])
        .await
        .unwrap();
    assert!(close(outcome.cost_window.cost_ped, 0.87));
}

#[tokio::test]
async fn a_repair_the_readings_saw_before_later_play_settled_what_came_before() {
    let (_dir, db, _clock, service) = harness().await;
    read_session(&db, "m1", 100.0, &[(PLATE, 2_500)], &[(PLATE, 2_387)]).await;
    // Repaired, seen by the readings; then played without them.
    read_account(&db, 150.0, &[(PLATE, 1, 2_500)]).await;
    play(&db, Played::new("u2", 200.0, &[(None, 10)])).await;

    let candidates = service
        .recording_candidates(ProtectionStream::Unlimited)
        .await
        .unwrap();
    assert_eq!(ids(&candidates.sessions), ["u2"]);
    assert_eq!(candidates.billed_from_account_ped, 0.0);
    let outcome = service
        .confirm_repair_cost("r", 0.5, vec!["u2".into()])
        .await
        .unwrap();
    assert!(close(outcome.cost_window.cost_ped, 0.5));
}

#[tokio::test]
async fn a_recording_with_no_unmeasured_session_to_cover_is_refused_with_its_reason() {
    let (_dir, db, _clock, service) = harness().await;
    play(&db, Played::new("a", 10.0, &[(None, 10)])).await;
    play(&db, Played::new("b", 20.0, &[(None, 10)])).await;
    measure(&db, "a", 0.5, &[]).await;
    measure(&db, "b", 0.6, &[]).await;

    let candidates = service
        .recording_candidates(ProtectionStream::Unlimited)
        .await
        .unwrap();
    assert!(candidates.sessions.is_empty());
    assert_eq!(candidates.measured_sessions, 2);
    let refused = service
        .confirm_repair_cost("r", 1.1, vec![])
        .await
        .unwrap_err();
    assert_eq!(
        message(refused),
        "Every session since the last repair was measured from your account"
    );

    // With nothing played at all, a repair still books to no session, as
    // before: there was no measurement to make it redundant.
    let (_dir, _db, _clock, fresh) = harness().await;
    let outcome = fresh.confirm_repair_cost("r", 1.1, vec![]).await.unwrap();
    assert_eq!(outcome.cost_window.status, CostStatus::Pending);
}

#[tokio::test]
async fn a_measured_session_is_never_charged_twice_for_armour() {
    let (_dir, db, _clock, service) = harness().await;
    play(&db, Played::new("a", 10.0, &[(None, 10)])).await;
    play(&db, Played::new("b", 20.0, &[(None, 10)])).await;
    // A repair recorded before the readings measured "a" reached it.
    service
        .confirm_repair_cost("r", 3.0, vec!["a".into()])
        .await
        .unwrap();
    measure(&db, "a", 1.13, &[]).await;
    measure(&db, "b", 0.4, &[]).await;

    let a = resolved_armour(&db, "a").await;
    assert!(close(a.resolved.armour, 1.13), "{}", a.resolved.armour);
    assert_eq!(a.cost_basis(Bucket::Armour), CostBasis::Measured);
    // Its old share is not spread anywhere else either.
    assert!(close(resolved_armour(&db, "b").await.resolved.armour, 0.4));

    // A measured session's armour is recorded: nothing waits on it.
    assert_eq!(service.session_unrecorded_hits("b").await.unwrap(), 0);
    assert!(service
        .sessions_with_unrecorded_hits(vec!["a".into(), "b".into()])
        .await
        .unwrap()
        .is_empty());
    assert_eq!(service.overview().await.unwrap().unrecorded.sessions, 0);
    // Its tracked figure, set beside the measured one, still lacks a
    // recording.
    assert_eq!(
        service
            .sessions_without_a_recording(vec!["a".into(), "b".into()])
            .await
            .unwrap(),
        ["b"]
    );
}

#[tokio::test]
async fn a_limited_set_measures_from_its_pieces_in_the_latest_account_reading() {
    let (_dir, db, _clock, service) = harness().await;
    let set = limited(&service, "Pegasus", 140.0).await;
    let stream = ProtectionStream::Limited { set_id: set.id };
    // Played and measured before the reading: nothing of it is left to
    // cover, so the reading after it can be the baseline.
    play(&db, Played::new("earlier", 10.0, &[(None, 10)])).await;
    measure(&db, "earlier", 0.4, &[]).await;
    read_account(&db, 100.0, &[(HELMET, 1, 1_000), (HARNESS, 1, 2_000)]).await;
    service
        .confirm_set_pieces(set.id, vec![HELMET.into(), HARNESS.into()])
        .await
        .unwrap();
    play(&db, Played::new("later", 200.0, &[(None, 20)])).await;

    let candidates = service.recording_candidates(stream).await.unwrap();
    assert_eq!(candidates.baseline_source, Some(BaselineSource::Account));
    assert_eq!(candidates.baseline_tt_ped, Some(30.0));
    assert_eq!(candidates.since, Some(100.0));
    assert!(close(candidates.account_tolerance_ped, 0.02));
    assert_eq!(candidates.billed_from_account_ped, 0.0);
    assert_eq!(ids(&candidates.sessions), ["later"]);

    let outcome = service
        .confirm_observation(
            set.id,
            "tt",
            29.5,
            ObservationSource::Manual,
            None,
            None,
            vec!["later".into()],
        )
        .await
        .unwrap();
    let window = outcome.cost_window.expect("a measured loss");
    assert!(close(window.consumed_tt_ped.unwrap(), 0.5));
    assert!(close(window.cost_ped, 0.7));
    assert_eq!(window.allocations.len(), 1);
    // The baseline it stood on is kept as a reading from the account.
    let opening: i64 = db
        .with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT opening_observation_id FROM protection_cost_windows WHERE id = ?1",
                [window.id],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    let opening = db
        .with_reader(move |conn| {
            read::read_observation(conn, opening).map_err(super::protection_decode)
        })
        .await
        .unwrap();
    assert_eq!(opening.source, ObservationSource::Account);
    assert!(close(opening.tt_value_ped, 30.0));
    assert_eq!(opening.observed_at, 100.0);

    // Undoing the recording takes its account baseline with it: the set
    // measures from the account reading again.
    service
        .undo(UndoTarget::Recording {
            window_id: window.id,
        })
        .await
        .unwrap();
    let again = service.recording_candidates(stream).await.unwrap();
    assert_eq!(again.baseline_source, Some(BaselineSource::Account));
    assert_eq!(ids(&again.sessions), ["later"]);
    let live: i64 = db
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM protection_observations WHERE superseded_at IS NULL",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(live, 0);
}

#[tokio::test]
async fn a_set_s_own_reading_newer_than_the_account_s_stays_its_baseline() {
    let (_dir, db, _clock, service) = harness().await;
    let set = limited(&service, "Pegasus", 140.0).await;
    read_account(&db, 100.0, &[(HELMET, 1, 1_000)]).await;
    service
        .confirm_set_pieces(set.id, vec![HELMET.into()])
        .await
        .unwrap();
    // Taken now, long after the account reading.
    service
        .confirm_observation(
            set.id,
            "own",
            9.8,
            ObservationSource::Ocr,
            Some("9.80"),
            None,
            vec![],
        )
        .await
        .unwrap();
    let candidates = service
        .recording_candidates(ProtectionStream::Limited { set_id: set.id })
        .await
        .unwrap();
    assert_eq!(
        candidates.baseline_source,
        Some(BaselineSource::TradeTerminal)
    );
    assert_eq!(candidates.baseline_tt_ped, Some(9.8));
    assert_eq!(candidates.account_tolerance_ped, 0.0);
}

#[tokio::test]
async fn a_set_measured_from_the_readings_is_not_recorded_again() {
    let (_dir, db, _clock, service) = harness().await;
    let set = limited(&service, "Pegasus", 140.0).await;
    let stream = ProtectionStream::Limited { set_id: set.id };
    read_account(&db, 5.0, &[(HELMET, 1, 1_000)]).await;
    service
        .confirm_set_pieces(set.id, vec![HELMET.into()])
        .await
        .unwrap();
    play(&db, Played::new("hunt", 10.0, &[(None, 10)])).await;
    measure(&db, "hunt", 1.0, &[(set.id, 0.462)]).await;

    // Its pieces' last reading is before the hunt; the hunt's own share of
    // the set was measured, so a Trade Terminal reading has nothing to
    // cover.
    let candidates = service.recording_candidates(stream).await.unwrap();
    assert!(candidates.sessions.is_empty());
    assert_eq!(candidates.measured_sessions, 1);
    let refused = service
        .confirm_observation(
            set.id,
            "tt",
            9.5,
            ObservationSource::Manual,
            None,
            None,
            vec![],
        )
        .await
        .unwrap_err();
    assert_eq!(
        message(refused),
        "Every session since this set's last reading was measured from your account"
    );
    let chosen = service
        .confirm_observation(
            set.id,
            "tt",
            9.5,
            ObservationSource::Manual,
            None,
            None,
            vec!["hunt".into()],
        )
        .await
        .unwrap_err();
    assert_eq!(
        message(chosen),
        "A chosen session's armour was measured from your account"
    );
}

/// The same hunt measured twice: from its readings, and, its readings
/// lost, from a Trade Terminal reading against the reading before it.
#[tokio::test]
async fn a_trade_terminal_reading_and_the_readings_agree_within_a_pec_per_piece() {
    // The pieces' true TT before and after; the site shows each truncated
    // to the PEC.
    let before = [(HELMET, 10.004_3), (HARNESS, 20.009_9)];
    let after = [(HELMET, 9.675_1), (HARNESS, 19.502_8)];
    let site = |pieces: &[(&'static str, f64); 2]| -> Vec<(&'static str, i64, i64)> {
        pieces
            .iter()
            .map(|(name, ped)| (*name, 1, (ped * 100.0).floor() as i64))
            .collect()
    };

    // Read whole: the readings' loss is what the projection bills.
    let mut conn = crate::db::migrated_in_memory().unwrap();
    conn.execute(
        "INSERT INTO protection_sets (kind, name, economy_kind, markup_percent, created_at) \
         VALUES ('armour', 'Pegasus', 'limited', 140, 1)",
        [],
    )
    .unwrap();
    for name in [HELMET, HARNESS] {
        conn.execute(
            "INSERT INTO protection_set_pieces (set_id, item_name, confirmed_at) \
             SELECT id, ?1, 1.0 FROM protection_sets",
            [name],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES ('hunt', 1000.0, 1060.0, 0)",
        [],
    )
    .unwrap();
    for (at, pieces, tag) in [(990.0, &before, None), (1_060.0, &after, Some("hunt"))] {
        let rows: Vec<(&str, Container, i64, i64)> = site(pieces)
            .into_iter()
            .map(|(name, quantity, value)| (name, C, quantity, value))
            .collect();
        record_reading(
            &mut conn,
            &reading(at, 10_000, &rows),
            Purpose::Session,
            tag,
        )
        .unwrap();
    }
    let costs = crate::account::costs::session_costs(&mut conn, "hunt", None, 5_000.0, None)
        .unwrap()
        .expect("the hunt");
    let read_loss = costs
        .armour
        .iter()
        .find_map(|part| match part {
            crate::account::costs::ArmourPart::Set { lost, .. } => Some(lost.micro_ped()),
            _ => None,
        })
        .expect("the set's part") as f64
        / 1_000_000.0;

    // Its readings lost: a Trade Terminal reading of the true value.
    let (_dir, db, _clock, service) = harness().await;
    let set = limited(&service, "Pegasus", 140.0).await;
    read_account(&db, 990.0, &site(&before)).await;
    service
        .confirm_set_pieces(set.id, vec![HELMET.into(), HARNESS.into()])
        .await
        .unwrap();
    play(&db, Played::new("hunt", 1_000.0, &[(None, 10)])).await;
    let true_after: f64 = after.iter().map(|(_, ped)| ped).sum();
    let outcome = service
        .confirm_observation(
            set.id,
            "tt",
            true_after,
            ObservationSource::Manual,
            None,
            None,
            vec!["hunt".into()],
        )
        .await
        .unwrap();
    let terminal_loss = outcome.cost_window.unwrap().consumed_tt_ped.unwrap();
    assert!(
        (terminal_loss - read_loss).abs() <= 0.02 + 1e-9,
        "{terminal_loss} against {read_loss}"
    );
}

#[tokio::test]
async fn suggestions_come_only_from_limited_protection_the_set_s_name_names() {
    let (_dir, db, _clock, service) = harness().await;
    let set = limited(&service, "Pegasus", 140.0).await;
    let elite = limited(&service, "Pegasus Elite", 160.0).await;
    db.with_writer(|conn| {
        // The player's rifle shares the maker's name.
        conn.execute(
            "INSERT INTO equipment_library (name, item_type, properties_json) \
             VALUES ('Pegasus Rifle (L)', 'weapon', '{}')",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    read_account(
        &db,
        100.0,
        &[
            (HELMET, 1, 1_000),
            ("Pegasus Gloves (M)", 1, 900),
            ("Pegasus Rifle (L)", 1, 5_000),
            ("Pegasus Elite Harness (L)", 1, 3_000),
            ("Armor Plating Mk. 5B (L)", 1, 4_000),
            ("Animal Oil Residue (L)", 10, 10),
        ],
    )
    .await;

    let pieces = service.set_pieces(set.id).await.unwrap();
    assert!(pieces.confirmed.is_empty());
    assert_eq!(pieces.suggested, [HELMET]);
    // The other limited armour and plates, to add by hand.
    assert_eq!(
        pieces.others,
        ["Armor Plating Mk. 5B (L)", "Pegasus Elite Harness (L)"]
    );
    assert_eq!(pieces.reading_taken_at, Some(100.0));
    let elite_pieces = service.set_pieces(elite.id).await.unwrap();
    assert_eq!(elite_pieces.suggested, ["Pegasus Elite Harness (L)"]);

    // Confirmed, a piece leaves the suggestions.
    let confirmed = service
        .confirm_set_pieces(set.id, vec![HELMET.into()])
        .await
        .unwrap();
    assert_eq!(
        confirmed
            .confirmed
            .iter()
            .map(|piece| piece.name.as_str())
            .collect::<Vec<_>>(),
        [HELMET]
    );
    assert!(confirmed.suggested.is_empty());
    assert_eq!(service.overview().await.unwrap().sets[0].pieces, [HELMET]);
}

#[tokio::test]
async fn a_piece_belongs_to_one_set_in_use_and_to_an_item_a_reading_held() {
    let (_dir, db, _clock, service) = harness().await;
    let first = limited(&service, "Pegasus", 140.0).await;
    let second = limited(&service, "Pegasus Spare", 120.0).await;
    read_account(&db, 100.0, &[(HELMET, 1, 1_000)]).await;
    service
        .confirm_set_pieces(first.id, vec![HELMET.into()])
        .await
        .unwrap();

    let taken = service
        .confirm_set_pieces(second.id, vec![HELMET.into()])
        .await
        .unwrap_err();
    assert_eq!(
        message(taken),
        "An item can be a piece of only one set in use"
    );
    let unknown = service
        .confirm_set_pieces(second.id, vec!["Pegasus Boots (M,L)".into()])
        .await
        .unwrap_err();
    assert_eq!(
        message(unknown),
        "Only an item an account reading has held can be a piece"
    );

    // Once the first set is out of use, its piece is free to move.
    service.archive_set(first.id).await.unwrap();
    service
        .confirm_set_pieces(second.id, vec![HELMET.into()])
        .await
        .unwrap();
    // A refused confirmation changed nothing; clearing a set is allowed.
    let cleared = service.confirm_set_pieces(second.id, vec![]).await.unwrap();
    assert!(cleared.confirmed.is_empty());
}

#[tokio::test]
async fn each_unmeasured_session_is_covered_once_between_readings_of_a_set() {
    let (_dir, db, clock, service) = harness().await;
    let set_id = limited(&service, "Hyperion", 150.0).await.id;
    let reading = |token: &'static str, tt: f64, sessions: Vec<String>| {
        let service = &service;
        async move {
            service
                .confirm_observation(
                    set_id,
                    token,
                    tt,
                    ObservationSource::Manual,
                    None,
                    None,
                    sessions,
                )
                .await
                .unwrap()
        }
    };
    reading("o", 30.0, vec![]).await;
    clock.advance(60.0).unwrap();
    play(&db, Played::new("u1", 10.0, &[(None, 10)])).await;
    let first = reading("r", 29.0, vec!["u1".into()]).await;
    assert!(close(first.cost_window.unwrap().cost_ped, 1.5));
    clock.advance(60.0).unwrap();
    play(&db, Played::new("u2", 20.0, &[(None, 10)])).await;

    let candidates = service
        .recording_candidates(ProtectionStream::Limited { set_id })
        .await
        .unwrap();
    assert_eq!(ids(&candidates.sessions), ["u2"]);
    let second = reading("t", 28.5, vec!["u2".into()]).await;
    assert!(close(second.cost_window.unwrap().cost_ped, 0.75));
    assert!(close(resolved_armour(&db, "u1").await.resolved.armour, 1.5));
    assert!(close(
        resolved_armour(&db, "u2").await.resolved.armour,
        0.75
    ));
}

#[tokio::test]
async fn a_session_played_while_the_readings_were_off_stays_coverable() {
    let (_dir, db, _clock, service) = harness().await;
    let set = limited(&service, "Pegasus", 140.0).await;
    read_account(&db, 5.0, &[(HELMET, 1, 1_000), (HARNESS, 1, 2_000)]).await;
    service
        .confirm_set_pieces(set.id, vec![HELMET.into(), HARNESS.into()])
        .await
        .unwrap();
    // The sign-in dropped for this one; the readings resume after it.
    play(&db, Played::new("u", 10.0, &[(None, 10)])).await;
    read_session(
        &db,
        "m",
        100.0,
        &[(HELMET, 980), (HARNESS, 2_000)],
        &[(HELMET, 970), (HARNESS, 2_000)],
    )
    .await;

    let candidates = service
        .recording_candidates(ProtectionStream::Limited { set_id: set.id })
        .await
        .unwrap();
    // The baseline is the reading before the unmeasured session, not the
    // latest one, which would leave it out.
    assert_eq!(candidates.baseline_source, Some(BaselineSource::Account));
    assert_eq!(candidates.since, Some(5.0));
    assert_eq!(candidates.baseline_tt_ped, Some(30.0));
    assert_eq!(ids(&candidates.sessions), ["u"]);
    // The measured session's 0.10 of the set is already billed.
    assert!(close(candidates.billed_from_account_ped, 0.1));

    let outcome = service
        .confirm_observation(
            set.id,
            "tt",
            29.5,
            ObservationSource::Manual,
            None,
            None,
            vec!["u".into()],
        )
        .await
        .unwrap();
    let window = outcome.cost_window.unwrap();
    assert!(close(window.consumed_tt_ped.unwrap(), 0.4));
    assert!(close(window.cost_ped, 0.56));
}

#[tokio::test]
async fn an_account_reading_missing_a_piece_is_no_baseline() {
    let (_dir, db, _clock, service) = harness().await;
    let set = limited(&service, "Pegasus", 140.0).await;
    read_account(&db, 5.0, &[(HELMET, 1, 1_000), (HARNESS, 1, 2_000)]).await;
    service
        .confirm_set_pieces(set.id, vec![HELMET.into(), HARNESS.into()])
        .await
        .unwrap();
    read_account(&db, 50.0, &[(HELMET, 1, 990)]).await;
    play(&db, Played::new("u", 60.0, &[(None, 10)])).await;

    let candidates = service
        .recording_candidates(ProtectionStream::Limited { set_id: set.id })
        .await
        .unwrap();
    assert_eq!(candidates.baseline_tt_ped, None);
    assert!(candidates.sessions.is_empty());
    assert_eq!(candidates.incomplete_account_reading_at, Some(50.0));
}

#[tokio::test]
async fn a_piece_moved_to_a_new_set_leaves_earlier_sessions_at_the_old_markup() {
    let (_dir, db, _clock, service) = harness().await;
    let old = limited(&service, "Pegasus", 140.0).await;
    read_account(&db, 5.0, &[(HELMET, 1, 1_000)]).await;
    service
        .confirm_set_pieces(old.id, vec![HELMET.into()])
        .await
        .unwrap();
    read_session(&db, "s1", 100.0, &[(HELMET, 1_000)], &[(HELMET, 967)]).await;

    // The old set goes out of use and the helmet joins a new one.
    service.archive_set(old.id).await.unwrap();
    let new = limited(&service, "Pegasus Again", 120.0).await;
    service
        .confirm_set_pieces(new.id, vec![HELMET.into()])
        .await
        .unwrap();
    let later = 2_000_000_000.0;
    read_session(&db, "s2", later, &[(HELMET, 967)], &[(HELMET, 934)]).await;

    use crate::account::amount::Amount;
    use crate::account::costs::ArmourPart;
    let set_part = |parts: Vec<ArmourPart>| {
        parts.into_iter().find_map(|part| match part {
            ArmourPart::Set {
                set_id,
                markup_hundredths,
                cost,
                ..
            } => Some((set_id, markup_hundredths, cost)),
            _ => None,
        })
    };
    assert_eq!(
        set_part(armour_parts(&db, "s1").await),
        Some((old.id, 14_000, Amount::WholePec { micro_ped: 462_000 }))
    );
    assert_eq!(
        set_part(armour_parts(&db, "s2").await),
        Some((new.id, 12_000, Amount::WholePec { micro_ped: 396_000 }))
    );

    // Billed with it, the new set's markup is fixed now.
    let fixed = service
        .update_set(new.id, "Pegasus Again", 130.0)
        .await
        .unwrap_err();
    assert_eq!(
        message(fixed),
        "A set's markup cannot change once a session has been billed with it"
    );
    // And the old set cannot come back holding the helmet.
    let restored = service.restore_set(old.id).await.unwrap_err();
    assert_eq!(
        message(restored),
        "A piece of this set is now a piece of a set in use"
    );
}

#[tokio::test]
async fn a_set_with_no_pieces_is_recorded_only_where_it_may_have_worn_unmeasured() {
    let (_dir, db, clock, service) = harness().await;
    let set = limited(&service, "Hyperion", 150.0).await;
    service
        .confirm_observation(
            set.id,
            "o",
            30.0,
            ObservationSource::Manual,
            None,
            None,
            vec![],
        )
        .await
        .unwrap();
    clock.advance(60.0).unwrap();
    for (id, at) in [("whole", 10.0), ("waiting", 20.0), ("unread", 30.0)] {
        play(&db, Played::new(id, at, &[(None, 10)])).await;
    }
    // Read whole with no limited piece unpriced: nothing of the set wore.
    measure(&db, "whole", 1.0, &[]).await;
    // A limited piece no confirmed set holds fell: the set may have worn.
    measure_with(&db, "waiting", 1.0, &[], Some(0.2)).await;
    // The days are rolled up to a few days on, as the app keeps them.
    db.with_writer(|conn| {
        crate::daily_rollup::heal_rollups(conn, 3.0 * 86_400.0)?;
        Ok(())
    })
    .await
    .unwrap();

    let candidates = service
        .recording_candidates(ProtectionStream::Limited { set_id: set.id })
        .await
        .unwrap();
    assert_eq!(ids(&candidates.sessions), ["waiting", "unread"]);

    // The waiting session's net is not final until a set's recording
    // reaches it.
    let page = vec!["whole".to_string(), "waiting".to_string()];
    assert_eq!(
        service
            .sessions_with_unrecorded_hits(page.clone())
            .await
            .unwrap(),
        ["waiting"]
    );
    service
        .confirm_observation(
            set.id,
            "r",
            29.6,
            ObservationSource::Manual,
            None,
            None,
            vec!["waiting".into()],
        )
        .await
        .unwrap();
    assert!(service
        .sessions_with_unrecorded_hits(page)
        .await
        .unwrap()
        .is_empty());

    // The recording reaches every figure at once: its resolution, and the
    // day it was played on, which sums the resolved armour.
    let waiting = resolved_armour(&db, "waiting").await;
    assert!(close(waiting.resolved.armour, 1.6));
    assert_eq!(waiting.cost_basis(Bucket::Armour), CostBasis::Mixed);
    let day: f64 = db
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT armour_cost FROM daily_rollups WHERE day = '1970-01-01'",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(close(day, 1.0 + 1.6), "{day}");
}
