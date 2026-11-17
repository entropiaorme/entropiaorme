//! What the account readings propose, over the typed facade: the recorded
//! hunt's openings proposed from readings taken live and from readings a
//! previous run stored; each proposed once however often it is examined;
//! claimed, recorded, linked, dismissed, and pending again once its record
//! is taken back; a claim with no link shown as possibly recorded, and one
//! released after a refused recording; the opening before the hunt
//! recorded with its ammo taken as the keys less what else came out;
//! listings sold (with and without a price) and returned, and a match too
//! uncertain to propose; and the readings a strongbox run asks for, under
//! the minute floor and without a connection.

use std::sync::Arc;

use eo_api::account::{AccountPlace, MeasuredSettlement};
use eo_api::account_proposals::{
    AccountProposal, AccountProposalDismissInput, AccountProposalKind, AccountProposalOutcome,
    AccountProposalsClaimInput, AccountProposalsInput, AccountProposalsLinkInput,
    AccountRunReading,
};
use eo_api::analytics::{ActivityUndoInput, AuctionConfirmInput, AuctionExpireInput};
use eo_api::strongbox::{
    StrongboxIntake, StrongboxItemInput, StrongboxOpeningInput, StrongboxSource,
};
use eo_api::ApiError;
use eo_services::account::reading::Reading;
use eo_services::account::recorder::{MemoryFlag, ReadingMode, RecorderParts};
use eo_services::account::scripted::{
    recorded_hunt_responses, responses, PacedClock, ScriptedRead,
};
use eo_services::account::store::{record_reading, Purpose};
use eo_services::account::AccountRecorder;
use serde_json::Value;

use crate::account_facade::{
    close, harness_with, record_the_hunt, the_hunt_from_its_opening_reading, Harness, Planned,
    Seed, HUNT,
};

/// Readings a previous run stored, a minute apart and ending a minute
/// before the clock's start, with `listings` (id, item, quantity, TT) on
/// auction.
fn stored_before(
    readings: Vec<(Value, Value)>,
    apart: f64,
    listings: &'static [(&'static str, &'static str, i64, f64)],
) -> Seed {
    Box::new(move |conn, clock: &PacedClock| {
        let count = readings.len() as f64;
        for (index, (items, balance)) in readings.iter().enumerate() {
            let at = clock.epoch_at(-(count - index as f64) * apart);
            let reading = Reading::from_site(at, items, balance).expect("a whole reading");
            record_reading(conn, &reading, Purpose::Run, None).expect("stored");
        }
        for (id, item, quantity, tt) in listings {
            conn.execute(
                "INSERT INTO auction_listings (id, item_name, quantity, tt_value, starting_bid, \
                     listed_at, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, 1.0, '2026-06-30', 1, 1)",
                rusqlite::params![id, item, quantity, tt],
            )
            .expect("a listing");
        }
    })
}

/// Once the connection has examined what it has.
async fn settled(harness: &Harness) {
    harness.until(|status| !status.working).await;
}

async fn proposals(harness: &Harness, kind: Option<AccountProposalKind>) -> Vec<AccountProposal> {
    harness.api.account_proposals(kind).await.unwrap()
}

struct OpeningFigures {
    boxes: Vec<(String, i64)>,
    keys: f64,
    ammo: Option<f64>,
    contents: Vec<(String, f64)>,
    possible: Vec<(String, f64)>,
    settled: bool,
    paid_ammo_alone: Option<i64>,
}

fn opening(proposal: &AccountProposal) -> OpeningFigures {
    let AccountProposalOutcome::Opening {
        boxes,
        keys,
        ammo,
        contents,
        possible_contents,
        settlement,
        paid_ammo_alone,
        ..
    } = &proposal.outcome
    else {
        panic!("an opening: {proposal:?}");
    };
    OpeningFigures {
        boxes: boxes.iter().map(|b| (b.name.clone(), b.count)).collect(),
        keys: keys.ped,
        ammo: ammo.as_ref().map(|ammo| ammo.amount.ped),
        contents: contents
            .iter()
            .map(|item| (item.item.name.clone(), item.amount.ped))
            .collect(),
        possible: possible_contents
            .iter()
            .map(|item| (item.item.name.clone(), item.amount.ped))
            .collect(),
        settled: *settlement == MeasuredSettlement::Settled,
        paid_ammo_alone: paid_ammo_alone.0,
    }
}

#[tokio::test(start_paused = true)]
async fn an_opening_in_a_hunt_read_live_is_one_pending_proposal_with_its_figures() {
    let harness = harness_with(
        the_hunt_from_its_opening_reading(),
        &[HUNT],
        Box::new(|_, _| {}),
    )
    .await;
    record_the_hunt(&harness).await;
    let found = proposals(&harness, None).await;
    assert_eq!(found.len(), 1, "{found:?}");
    let proposal = &found[0];
    assert_eq!(proposal.session_id.as_deref(), Some("hunt"));
    let figures = opening(proposal);
    assert_eq!(figures.boxes, vec![("Halloween Strongbox".to_string(), 7)]);
    assert!(close(figures.keys, 70.0));
    assert!(close(figures.ammo.expect("settled ammo"), 60.0));
    assert_eq!(figures.contents.len(), 1);
    assert_eq!(figures.contents[0].0, "Skeleton Pants Red (F)");
    assert!(close(figures.contents[0].1, 10.0));
    assert!(figures.possible.is_empty());
    assert!(figures.settled);
    assert_eq!(figures.paid_ammo_alone, Some(6));
    // The window it happened in crosses the wire as two instants.
    let wire = serde_json::to_value(proposal).unwrap();
    assert_eq!(wire["outcome"]["kind"], "opening");
    assert!(wire["between"]["from"].is_string() && wire["between"]["to"].is_string());
    assert!(proposals(&harness, Some(AccountProposalKind::ListingSold))
        .await
        .is_empty());
}

#[tokio::test(start_paused = true)]
async fn readings_a_previous_run_stored_are_examined_once_at_start() {
    let harness = harness_with(
        recorded_hunt_responses(),
        &[],
        stored_before(recorded_hunt_responses(), 60.0, &[]),
    )
    .await;
    settled(&harness).await;
    let found = proposals(&harness, Some(AccountProposalKind::Opening)).await;
    assert_eq!(found.len(), 2, "{found:?}");
    // The latest seen first: the hunt's seven, then the five before it.
    assert_eq!(opening(&found[0]).boxes[0].1, 7);
    let before = opening(&found[1]);
    assert_eq!(before.boxes, vec![("Halloween Strongbox".to_string(), 5)]);
    assert!(close(before.keys, 50.0));
    assert!(close(before.ammo.expect("settled ammo"), 49.95));
    assert!(before.contents.is_empty());
    assert_eq!(
        before.possible,
        vec![("Halloween Fireworks".to_string(), 0.0)]
    );
    assert!(before.settled);
    assert_eq!(before.paid_ammo_alone, Some(4));
    assert_eq!(found[1].session_id.as_deref(), None);

    // Examined again, directly and by a later start: nothing twice.
    let reading = found[0].reading_id;
    let game_data = harness.game_data.clone();
    let added = harness
        .db
        .with_writer(move |conn| {
            let roles =
                eo_services::account::roles::KnownRoles::read(conn, Some(&game_data)).unwrap();
            Ok(eo_services::account::proposals::detect(
                conn,
                eo_services::account::store::ReadingId(reading),
                &roles,
                1.0,
            )
            .unwrap())
        })
        .await
        .unwrap();
    assert_eq!(added, 0);
    harness.recorder.shutdown().await;
    let later = AccountRecorder::start(
        &harness.bus,
        RecorderParts {
            db: harness.db.clone(),
            clock: harness.clock.clone(),
            site: harness.site.clone(),
            flag: Arc::new(MemoryFlag::new(false)),
            game_data: Some(harness.game_data.clone()),
            chat_log: None,
            changed: None,
            mode: ReadingMode::OnDemand,
        },
        &tokio::runtime::Handle::current(),
    );
    let _ = later
        .watch_status()
        .wait_for(|status| !status.working)
        .await;
    assert_eq!(later.proposals(None).await.unwrap().len(), 2);
    let stored: i64 = harness
        .db
        .with_reader(|conn| {
            Ok(
                conn.query_row("SELECT COUNT(*) FROM account_proposals", [], |row| {
                    row.get(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(stored, 2);
}

fn the_seven() -> StrongboxOpeningInput {
    StrongboxOpeningInput {
        box_item: "Halloween Strongbox".into(),
        sources: vec![StrongboxSource::Bought {
            quantity: 7.0,
            markup_paid: 0.0,
        }],
        key_unit_tt: 10.0,
        key_markup_paid: None,
        contents: vec![StrongboxItemInput {
            item_name: "Skeleton Pants Red (F)".into(),
            quantity: 1.0,
            tt_value: 10.0,
            recycle: false,
        }],
        ammo_ped: 60.0,
        intake: StrongboxIntake::Export,
        opened_at: None,
    }
}

fn record(id: i64, record_id: &str) -> AccountProposalsLinkInput {
    AccountProposalsLinkInput {
        ids: vec![id],
        record_id: record_id.to_string(),
    }
}

fn set(ids: &[i64]) -> AccountProposalsInput {
    AccountProposalsInput { ids: ids.to_vec() }
}

fn claiming(ids: &[i64]) -> AccountProposalsClaimInput {
    AccountProposalsClaimInput {
        ids: ids.to_vec(),
        retake: Vec::new(),
    }
}

#[tokio::test(start_paused = true)]
async fn a_proposal_recorded_leaves_and_comes_back_when_its_opening_is_taken_back() {
    let harness = harness_with(
        the_hunt_from_its_opening_reading(),
        &[HUNT],
        Box::new(|_, _| {}),
    )
    .await;
    record_the_hunt(&harness).await;
    let id = proposals(&harness, None).await[0].id;

    // Claimed, recorded through the strongbox command, then linked.
    harness
        .api
        .account_proposals_claim(claiming(&[id]))
        .await
        .unwrap();
    let opened = harness.api.strongbox_open(the_seven()).await.unwrap();
    harness
        .api
        .account_proposals_link(record(id, &opened.id))
        .await
        .unwrap();
    assert!(proposals(&harness, None).await.is_empty());
    // Marking again with the same record changes nothing.
    harness
        .api
        .account_proposals_link(record(id, &opened.id))
        .await
        .unwrap();
    // A record that is not a standing opening is refused.
    assert!(matches!(
        harness
            .api
            .account_proposals_link(record(id, "no-such-opening"))
            .await,
        Err(ApiError::Conflict { .. })
    ));

    // Taken back: pending again, the same proposal.
    harness
        .api
        .strongbox_opening_undo(ActivityUndoInput {
            id: opened.id.clone(),
        })
        .await
        .unwrap();
    let again = proposals(&harness, None).await;
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id, id);

    // Recorded again but not yet marked: the opening stands for it, so it
    // is not offered to be recorded a second time.
    let second = harness.api.strongbox_open(the_seven()).await.unwrap();
    assert!(proposals(&harness, None).await.is_empty());
    harness
        .api
        .account_proposals_link(record(id, &second.id))
        .await
        .unwrap();
    harness
        .api
        .strongbox_opening_undo(ActivityUndoInput { id: second.id })
        .await
        .unwrap();
    assert_eq!(proposals(&harness, None).await.len(), 1);

    // Dismissed: gone, and dismissing again changes nothing.
    harness
        .api
        .account_proposal_dismiss(AccountProposalDismissInput { id })
        .await
        .unwrap();
    assert!(proposals(&harness, None).await.is_empty());
    harness
        .api
        .account_proposal_dismiss(AccountProposalDismissInput { id })
        .await
        .unwrap();
    let third = harness.api.strongbox_open(the_seven()).await.unwrap();
    assert!(matches!(
        harness
            .api
            .account_proposals_link(record(id, &third.id))
            .await,
        Err(ApiError::Conflict { .. })
    ));
    assert!(matches!(
        harness
            .api
            .account_proposal_dismiss(AccountProposalDismissInput { id: 999 })
            .await,
        Err(ApiError::NotFound { .. })
    ));
}

/// Shrapnel on auction, then gone with the Card up 12.50 PED.
fn a_sale(ammo_after: i64) -> Vec<(Value, Value)> {
    responses(&[
        (
            10_000,
            &[
                ("Universal Ammo", "CARRIED", 50_000, 500),
                ("Shrapnel", "AUCTION", 100_000, 1_000),
            ],
        ),
        (
            11_250,
            &[("Universal Ammo", "CARRIED", ammo_after, ammo_after / 100)],
        ),
    ])
}

const ONE_LISTING: &[(&str, &str, i64, f64)] = &[("shrapnel", "Shrapnel", 100_000, 10.0)];

fn sold(proposal: &AccountProposal) -> (String, Option<f64>) {
    let AccountProposalOutcome::ListingSold {
        listing_id,
        proceeds,
        ..
    } = &proposal.outcome
    else {
        panic!("a sale: {proposal:?}");
    };
    (
        listing_id.clone(),
        proceeds.0.as_ref().map(|amount| amount.ped),
    )
}

#[tokio::test(start_paused = true)]
async fn a_listing_sold_carries_the_card_s_rise_only_when_it_can_be_read() {
    // A minute apart, nothing else moving: the price is offered.
    let harness = harness_with(
        a_sale(50_000),
        &[],
        stored_before(a_sale(50_000), 60.0, ONE_LISTING),
    )
    .await;
    settled(&harness).await;
    let found = proposals(&harness, Some(AccountProposalKind::ListingSold)).await;
    assert_eq!(found.len(), 1);
    let (listing, price) = sold(&found[0]);
    assert_eq!(listing, "shrapnel");
    assert!(close(price.expect("a price"), 12.5));
    // Confirmed through the listing's own command, then marked.
    harness
        .api
        .auction_listing_confirm(AuctionConfirmInput {
            listing_id: "shrapnel".into(),
            final_price: 12.5,
            sale_fee: 0.0,
            resolved_at: None,
        })
        .await
        .unwrap();
    assert!(proposals(&harness, None).await.is_empty());
    harness
        .api
        .account_proposals_link(record(found[0].id, "shrapnel"))
        .await
        .unwrap();
    // The sale reverted: the proposal is open again.
    harness
        .api
        .auction_sale_revert(ActivityUndoInput {
            id: "shrapnel".into(),
        })
        .await
        .unwrap();
    assert_eq!(proposals(&harness, None).await.len(), 1);

    // Four minutes apart: no price, the player types it.
    let harness = harness_with(
        a_sale(50_000),
        &[],
        stored_before(a_sale(50_000), 240.0, ONE_LISTING),
    )
    .await;
    settled(&harness).await;
    let found = proposals(&harness, None).await;
    assert_eq!(found.len(), 1);
    assert_eq!(sold(&found[0]).1, None);

    // Ammo burned beside it could have moved the Card: no price.
    let harness = harness_with(
        a_sale(40_000),
        &[],
        stored_before(a_sale(40_000), 60.0, ONE_LISTING),
    )
    .await;
    settled(&harness).await;
    let found = proposals(&harness, None).await;
    assert_eq!(sold(&found[0]).1, None);
    let wire = serde_json::to_value(&found[0]).unwrap();
    assert_eq!(wire["outcome"]["proceeds"], Value::Null);
}

#[tokio::test(start_paused = true)]
async fn a_listing_returned_is_proposed_as_expired_and_an_ambiguous_one_not_at_all() {
    let returned = responses(&[
        (10_000, &[("Shrapnel", "AUCTION", 100_000, 1_000)]),
        (10_000, &[("Shrapnel", "STORAGE (Calypso)", 100_000, 1_000)]),
    ]);
    let harness = harness_with(
        returned.clone(),
        &[],
        stored_before(returned.clone(), 60.0, ONE_LISTING),
    )
    .await;
    settled(&harness).await;
    let found = proposals(&harness, None).await;
    assert_eq!(found.len(), 1);
    let AccountProposalOutcome::ListingReturned {
        listing_id,
        returned_to,
        ..
    } = &found[0].outcome
    else {
        panic!("a return: {:?}", found[0]);
    };
    assert_eq!(listing_id, "shrapnel");
    assert_eq!(
        returned_to,
        &AccountPlace::Storage {
            planet: "Calypso".into()
        }
    );
    // Marked recorded only as the outcome proposed.
    assert!(matches!(
        harness
            .api
            .account_proposals_link(record(found[0].id, "shrapnel"))
            .await,
        Err(ApiError::Conflict { .. })
    ));
    harness
        .api
        .auction_listing_expire(AuctionExpireInput {
            listing_id: "shrapnel".into(),
            resolved_at: None,
        })
        .await
        .unwrap();
    harness
        .api
        .account_proposals_link(record(found[0].id, "shrapnel"))
        .await
        .unwrap();
    assert!(proposals(&harness, None).await.is_empty());

    // Two listings it could be: nothing is proposed.
    const TWO: &[(&str, &str, i64, f64)] = &[
        ("one", "Shrapnel", 100_000, 10.0),
        ("two", "Shrapnel", 100_000, 10.0),
    ];
    let harness = harness_with(
        a_sale(50_000),
        &[],
        stored_before(a_sale(50_000), 60.0, TWO),
    )
    .await;
    settled(&harness).await;
    assert!(proposals(&harness, None).await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_run_s_readings_hold_the_minute_floor_and_need_a_connection() {
    let plan: [Planned; 0] = [];
    let harness = harness_with(a_sale(50_000), &plan, Box::new(|_, _| {})).await;
    let refused = harness.api.account_run_begin().await.unwrap();
    assert_eq!(refused, AccountRunReading::NotConnected);
    assert_eq!(
        serde_json::to_value(&refused).unwrap(),
        serde_json::json!({"outcome": "notConnected"})
    );
    assert_eq!(
        harness.api.account_run_done().await.unwrap(),
        AccountRunReading::NotConnected
    );

    assert_eq!(
        harness.connect().await,
        eo_services::account::ConnectOutcome::Connected
    );
    harness.at(10.0).await;
    // The connect reading, ten seconds old, is the run's start.
    let AccountRunReading::Read { reading_id, .. } = harness.api.account_run_begin().await.unwrap()
    else {
        panic!("a reading");
    };
    assert_eq!(
        harness.site.calls().len(),
        2,
        "nothing read inside the floor"
    );
    harness.at(20.0).await;
    let (done, ()) = tokio::join!(harness.api.account_run_done(), async {
        harness.at(54.0).await;
        assert_eq!(harness.site.calls().len(), 2);
        harness.at(56.0).await;
    });
    let AccountRunReading::Read {
        reading_id: last, ..
    } = done.unwrap()
    else {
        panic!("a reading");
    };
    assert!(last > reading_id);
    assert_eq!(harness.site.calls().len(), 4);
    harness.api.account_run_cancel().await.unwrap();
    harness.at(400.0).await;
    // Only the keep-alive asks the site anything more: a balance, no items.
    let items = harness
        .site
        .calls()
        .iter()
        .filter(|call| **call == ScriptedRead::Items)
        .count();
    assert_eq!(items, 2, "reading stopped once done");
}

/// The opening of five before the hunt, from readings a previous run
/// stored.
async fn the_five(harness: &Harness) -> AccountProposal {
    settled(harness).await;
    proposals(harness, Some(AccountProposalKind::Opening))
        .await
        .into_iter()
        .find(|proposal| opening(proposal).boxes[0].1 == 5)
        .expect("the opening of five")
}

fn five_with_ammo(ammo_ped: f64) -> StrongboxOpeningInput {
    StrongboxOpeningInput {
        box_item: "Halloween Strongbox".into(),
        sources: vec![StrongboxSource::Bought {
            quantity: 5.0,
            markup_paid: 0.0,
        }],
        key_unit_tt: 10.0,
        key_markup_paid: None,
        contents: vec![],
        ammo_ped,
        intake: StrongboxIntake::Export,
        opened_at: None,
    }
}

#[tokio::test(start_paused = true)]
async fn the_five_records_with_its_ammo_as_the_keys_less_what_else_came_out() {
    let harness = harness_with(
        recorded_hunt_responses(),
        &[],
        stored_before(recorded_hunt_responses(), 60.0, &[]),
    )
    .await;
    let five = the_five(&harness).await;
    let figures = opening(&five);
    // Read as 49.95 PED of ammo beside a fireworks with no TT; the keys
    // are 50.00 PED.
    assert!(close(figures.ammo.unwrap(), 49.95));
    assert!(close(figures.keys, 50.0));
    let ammo = figures.keys - figures.contents.iter().map(|(_, ped)| ped).sum::<f64>();
    assert!(close(ammo, 50.0));

    harness
        .api
        .account_proposals_claim(claiming(&[five.id]))
        .await
        .unwrap();
    // The figure as read is refused by the opening's own balance rule, and
    // the claim is released.
    assert!(harness
        .api
        .strongbox_open(five_with_ammo(49.95))
        .await
        .is_err());
    harness
        .api
        .account_proposals_release(set(&[five.id]))
        .await
        .unwrap();
    let again = the_five(&harness).await;
    assert!(!again.possibly_recorded, "reviewable again");

    harness
        .api
        .account_proposals_claim(claiming(&[five.id]))
        .await
        .unwrap();
    let opened = harness
        .api
        .strongbox_open(five_with_ammo(ammo))
        .await
        .unwrap();
    harness
        .api
        .account_proposals_link(record(five.id, &opened.id))
        .await
        .unwrap();
    assert!(proposals(&harness, None)
        .await
        .iter()
        .all(|proposal| proposal.id != five.id));
}

#[tokio::test(start_paused = true)]
async fn a_claim_never_linked_is_listed_as_possibly_recorded_after_a_restart() {
    let harness = harness_with(
        recorded_hunt_responses(),
        &[],
        stored_before(recorded_hunt_responses(), 60.0, &[]),
    )
    .await;
    let five = the_five(&harness).await;
    harness
        .api
        .account_proposals_claim(claiming(&[five.id]))
        .await
        .unwrap();
    let wire = serde_json::to_value(the_five(&harness).await).unwrap();
    assert_eq!(wire["possiblyRecorded"], true);
    // Not claimed twice.
    assert!(matches!(
        harness
            .api
            .account_proposals_claim(claiming(&[five.id]))
            .await,
        Err(ApiError::Conflict { .. })
    ));

    harness.recorder.shutdown().await;
    let later = AccountRecorder::start(
        &harness.bus,
        RecorderParts {
            db: harness.db.clone(),
            clock: harness.clock.clone(),
            site: harness.site.clone(),
            flag: Arc::new(MemoryFlag::new(false)),
            game_data: Some(harness.game_data.clone()),
            chat_log: None,
            changed: None,
            mode: ReadingMode::OnDemand,
        },
        &tokio::runtime::Handle::current(),
    );
    let _ = later
        .watch_status()
        .wait_for(|status| !status.working)
        .await;
    let listed = later.proposals(None).await.unwrap();
    let still = listed
        .iter()
        .find(|proposal| proposal.id.0 == five.id)
        .expect("still listed");
    assert!(still.claimed);
}

// ── What the readings record on their own ───────────────────────────

/// A reading of what the player carries, `rows` as (name, quantity, value
/// in PEC), with `balance` PEC on the Card.
fn carried(at: f64, balance: i64, rows: &[(&str, i64, i64)]) -> Reading {
    use eo_services::account::{Container, Holding, HoldingKey, Pec, Row};
    let mut holdings: std::collections::BTreeMap<HoldingKey, Vec<Row>> =
        std::collections::BTreeMap::new();
    for (name, quantity, value) in rows {
        holdings
            .entry(HoldingKey {
                name: (*name).to_string(),
                container: Container::Carried,
            })
            .or_default()
            .push(Row {
                quantity: *quantity,
                value: Pec(*value),
            });
    }
    Reading::new(
        at,
        Pec(balance),
        holdings
            .into_iter()
            .map(|(key, rows)| (key, Holding::new(rows).expect("a whole holding")))
            .collect(),
    )
    .expect("a whole reading")
}

/// Hunted Shrapnel held as stock, and two idle readings a previous run
/// stored, a minute apart, between which it was all turned into ammo.
fn shrapnel_converted_while_idle() -> Seed {
    Box::new(|conn, clock: &PacedClock| {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
             VALUES ('earlier', ?1, ?2, 0)",
            rusqlite::params![clock.epoch_at(-7_200.0), clock.epoch_at(-3_600.0)],
        )
        .expect("a session");
        conn.execute(
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
             shots_fired, damage_dealt, cost_ped, loot_total_ped, is_global, is_hof) \
             VALUES ('k', 'earlier', 'Atrox', 'Atrox', '', ?1, 1, 1.0, 0.1, 10.0, 0, 0)",
            [clock.epoch_at(-5_000.0)],
        )
        .expect("a kill");
        conn.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped) \
             VALUES ('k', 'Shrapnel', 100000, 10.0)",
            [],
        )
        .expect("its loot");
        let before = carried(
            clock.epoch_at(-120.0),
            5_000,
            &[
                ("Shrapnel", 100_000, 1_000),
                ("Universal Ammo", 50_000, 500),
            ],
        );
        let after = carried(
            clock.epoch_at(-60.0),
            5_000,
            &[
                ("Universal Ammo", 50_000, 500),
                ("Universal Ammo", 101_000, 1_010),
            ],
        );
        record_reading(conn, &before, Purpose::Ambient, None).expect("stored");
        record_reading(conn, &after, Purpose::Ambient, None).expect("stored");
    })
}

#[tokio::test(start_paused = true)]
async fn a_shrapnel_conversion_the_readings_saw_is_recorded_on_its_own_and_offered_to_the_form() {
    use eo_api::analytics::{Profession, StockConversionFromAccountInput};
    let harness = harness_with(Vec::new(), &[], shrapnel_converted_while_idle()).await;
    settled(&harness).await;
    let history = harness
        .api
        .activity_history(Profession::Inventory)
        .await
        .unwrap();
    let entry = history
        .iter()
        .find(|entry| entry.kind == "conversion")
        .expect("recorded on its own");
    assert_eq!(entry.item_name, "Shrapnel");
    let step = entry
        .recorded_from_account
        .as_ref()
        .expect("marked as recorded from the account");
    assert_eq!(
        step.to,
        eo_services::time::to_iso_utc(harness.clock.epoch_at(-60.0))
    );
    assert_eq!(
        step.from,
        eo_services::time::to_iso_utc(harness.clock.epoch_at(-120.0))
    );
    let recent = harness
        .api
        .stock_conversion_from_account(StockConversionFromAccountInput {
            item_name: "Shrapnel".to_string(),
        })
        .await
        .unwrap();
    let recent = recent.as_ref().expect("within the hour");
    assert_eq!(recent.id, entry.id);
    assert!(close(recent.quantity, 100_000.0));
    assert!(close(recent.tt_value, 10.0));
    // Nothing of an item the readings never converted.
    let none = harness
        .api
        .stock_conversion_from_account(StockConversionFromAccountInput {
            item_name: "Iron Stone".to_string(),
        })
        .await
        .unwrap();
    assert!(none.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_purchase_at_auction_outside_a_session_is_proposed_with_its_markup() {
    let seed: Seed = Box::new(|conn, clock: &PacedClock| {
        let before = carried(clock.epoch_at(-120.0), 5_000, &[]);
        let after = carried(clock.epoch_at(-60.0), 3_800, &[("Garcen Grease", 10, 200)]);
        record_reading(conn, &before, Purpose::Ambient, None).expect("stored");
        record_reading(conn, &after, Purpose::Ambient, None).expect("stored");
    });
    let harness = harness_with(Vec::new(), &[], seed).await;
    settled(&harness).await;
    let found = proposals(&harness, Some(AccountProposalKind::Purchase)).await;
    assert_eq!(found.len(), 1, "{found:?}");
    let AccountProposalOutcome::Purchase {
        item_name,
        units,
        tt,
        paid,
        markup,
        arrived_in,
    } = &found[0].outcome
    else {
        panic!("a purchase: {:?}", found[0]);
    };
    assert_eq!(item_name, "Garcen Grease");
    assert_eq!(*units, 10);
    assert!(close(tt.ped, 2.0));
    assert!(close(paid.ped, 12.0));
    assert!(close(markup.ped, 10.0));
    assert_eq!(*arrived_in, AccountPlace::Carried);
    assert!(found[0].session_id.is_none());

    // Recorded as a new holding, and linked to the purchase it made: the
    // asset's highest purchase id.
    use eo_api::analytics::{AssetPurchaseInput, InventoryItemInput};
    let created = harness
        .api
        .inventory_create(InventoryItemInput {
            name: "Garcen Grease".to_string(),
            quantity: Some(10.0),
            tt_value: 2.0,
            markup_paid: 10.0,
            notes: None,
            acquired_at: None,
        })
        .await
        .unwrap();
    let purchase = created
        .purchases
        .iter()
        .map(|purchase| purchase.id)
        .max()
        .expect("the purchase it made");
    harness
        .api
        .account_proposals_link(AccountProposalsLinkInput {
            ids: vec![found[0].id],
            record_id: purchase.to_string(),
        })
        .await
        .unwrap();
    assert!(proposals(&harness, Some(AccountProposalKind::Purchase))
        .await
        .is_empty());
    // A purchase added to it, dated before the first, is still the highest
    // id: the one a proposal recorded through it links to.
    let added = harness
        .api
        .inventory_purchase_add(
            created.id.clone(),
            AssetPurchaseInput {
                quantity: 5.0,
                tt_value: 1.0,
                markup_paid: 3.0,
                acquired_at: Some("2026-01-01".to_string()),
            },
        )
        .await
        .unwrap();
    let newest = added
        .purchases
        .iter()
        .max_by_key(|purchase| purchase.id)
        .expect("purchases");
    assert!(close(newest.quantity, 5.0));
    assert_ne!(newest.id, purchase);
}

#[tokio::test(start_paused = true)]
async fn a_trade_terminal_sale_the_readings_saw_is_offered_to_the_trade_form() {
    use eo_api::analytics::StockSaleFromAccountInput;
    let seed: Seed = Box::new(|conn, clock: &PacedClock| {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
             VALUES ('earlier', ?1, ?2, 0)",
            rusqlite::params![clock.epoch_at(-7_200.0), clock.epoch_at(-3_600.0)],
        )
        .expect("a session");
        conn.execute(
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
             shots_fired, damage_dealt, cost_ped, loot_total_ped, is_global, is_hof) \
             VALUES ('k', 'earlier', 'Atrox', 'Atrox', '', ?1, 1, 1.0, 0.1, 10.0, 0, 0)",
            [clock.epoch_at(-5_000.0)],
        )
        .expect("a kill");
        conn.execute(
            "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped) \
             VALUES ('k', 'Animal Oil Residue', 1000, 10.0)",
            [],
        )
        .expect("its loot");
        let before = carried(
            clock.epoch_at(-120.0),
            5_000,
            &[("Animal Oil Residue", 1_000, 1_000)],
        );
        let after = carried(clock.epoch_at(-60.0), 6_000, &[]);
        record_reading(conn, &before, Purpose::Ambient, None).expect("stored");
        record_reading(conn, &after, Purpose::Ambient, None).expect("stored");
    });
    let harness = harness_with(Vec::new(), &[], seed).await;
    settled(&harness).await;
    let sold = harness
        .api
        .stock_sale_from_account(StockSaleFromAccountInput {
            item_name: "Animal Oil Residue".to_string(),
        })
        .await
        .unwrap();
    let sold = sold.as_ref().expect("within the hour");
    assert!(close(sold.quantity, 1_000.0));
    assert!(close(sold.tt_value, 10.0));
    assert_eq!(
        sold.recorded_at,
        eo_services::time::to_iso_utc(harness.clock.epoch_at(-60.0))
    );
}
