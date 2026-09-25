//! Healing effect windows across restarts, new sessions, and corrections,
//! and the billing invariants over generated rotations.
//!
//! A restart is simulated the way it happens: the old process's bus goes
//! quiet, and a fresh bus and tracker open the same database, whose
//! construction recovers the orphaned session.

use std::sync::Arc;

use super::tests::{healer_intent, naive, rig, weapon_intent, Rig};
use super::{ActivityKey, ActivityRef, HuntTracker, Providers};
use crate::bus_events::{BusEvent, CombatPayload};
use crate::chatlog_time::ChatLogClock;
use crate::clock::Clock;
use crate::event_bus::EventBus;
use crate::healing_profile::{HealingMode, HealingProfile};
use crate::healing_review::{CorrectionTarget, HealingReviewService};
use crate::ped::Ped;
use crate::time::naive_to_epoch;

/// One configured healing item.
#[derive(Clone)]
struct Tool {
    id: i64,
    name: &'static str,
    cost: f64,
    reload: f64,
    profile: HealingProfile,
    /// An amount inside its confirming interval.
    confirm: f64,
    /// An amount inside its tick interval, when it has an effect.
    tick: Option<f64>,
}

fn fap() -> Tool {
    Tool {
        id: 9,
        name: "FAP",
        cost: 0.03,
        reload: 3.0,
        profile: HealingProfile {
            direct_min: Some(60.0),
            direct_max: Some(100.0),
            ..HealingProfile::default()
        },
        confirm: 80.0,
        tick: None,
    }
}

fn restoration() -> Tool {
    Tool {
        id: 8,
        name: "Restoration chip",
        cost: 0.04,
        reload: 10.0,
        profile: HealingProfile {
            mode: HealingMode::Compound,
            direct_min: Some(28.0),
            direct_max: Some(32.0),
            effect_duration_seconds: Some(20.0),
            tick_min: Some(9.0),
            tick_max: Some(11.0),
            tick_seconds: Some(2.0),
            ..HealingProfile::default()
        },
        confirm: 30.0,
        tick: Some(10.0),
    }
}

fn regeneration() -> Tool {
    Tool {
        id: 7,
        name: "Regeneration chip",
        cost: 0.02,
        reload: 8.0,
        profile: HealingProfile {
            mode: HealingMode::OverTime,
            effect_duration_seconds: Some(12.0),
            tick_min: Some(4.0),
            tick_max: Some(6.0),
            tick_seconds: Some(2.0),
            ..HealingProfile::default()
        },
        confirm: 5.0,
        tick: Some(5.0),
    }
}

fn tools() -> [Tool; 3] {
    [fap(), restoration(), regeneration()]
}

/// One process lifetime over the rig's database and clock.
struct Process {
    bus: Arc<EventBus>,
    tracker: Arc<HuntTracker>,
}

fn boot(rig: &Rig) -> Process {
    let bus = Arc::new(EventBus::new());
    let tracker = rig
        .wait(HuntTracker::new(
            bus.clone(),
            rig.db.clone(),
            rig.clock.clone(),
            ChatLogClock::host_local(),
            Providers::default(),
        ))
        .unwrap();
    Process { bus, tracker }
}

fn now(rig: &Rig) -> f64 {
    naive_to_epoch(rig.clock.now())
}

fn press(rig: &Rig, process: &Process, tool: &Tool) {
    process.bus.publish(&healer_intent(
        tool.id,
        tool.name,
        tool.cost,
        tool.reload,
        now(rig),
        tool.profile.clone(),
    ));
}

fn heal(rig: &Rig, process: &Process, amount: f64) {
    process
        .bus
        .publish(&BusEvent::Combat(CombatPayload::SelfHeal {
            amount,
            timestamp: rig.clock.now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        }));
}

fn announce_correction(process: &Process) {
    use eo_wire::domain_events::{HealingUpdated, HealingUpdatedPayload, HealingUpdatedTag};
    process
        .bus
        .publish(&BusEvent::HealingUpdated(HealingUpdated {
            topic: HealingUpdatedTag,
            event_version: 1,
            occurred_at: "2026-01-01T00:00:00+00:00".into(),
            payload: HealingUpdatedPayload {},
        }));
}

/// The latest output's classification and the tool of the activation it
/// names, if any.
fn latest_output(rig: &Rig) -> (String, Option<String>, Option<i64>) {
    rig.wait(rig.db.with_reader(|conn| {
        Ok(conn.query_row(
            "SELECT o.classification, a.tool_name, o.context_id FROM healing_outputs o \
             LEFT JOIN healing_activations a ON a.id = o.activation_id \
             ORDER BY o.observed_at DESC, o.rowid DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?)
    }))
    .unwrap()
}

fn window_count(rig: &Rig, process: &Process) -> usize {
    rig.probe(&process.tracker, |actor| {
        actor
            .session
            .active()
            .map(|active| active.healing.effect_windows.len())
            .unwrap_or(0)
    })
}

#[test]
fn an_open_effect_window_survives_a_restart_with_its_provenance() {
    let rig = rig();
    let first = boot(&rig);
    let crashed = rig.wait(first.tracker.start_session()).unwrap();
    let restoration = restoration();
    press(&rig, &first, &restoration);
    heal(&rig, &first, restoration.confirm);
    rig.clock.advance(2.0).unwrap();
    heal(&rig, &first, 10.0);

    // The process dies mid-effect; the next one recovers the orphan and
    // starts a new session while the effect is still running in the game.
    rig.clock.advance(3.0).unwrap();
    let second = boot(&rig);
    assert_eq!(
        rig.scalar_i64(
            "SELECT is_active FROM tracking_sessions WHERE id = ?",
            &[&crashed.id],
        ),
        0
    );
    let resumed = rig.wait(second.tracker.start_session()).unwrap();
    assert_eq!(window_count(&rig, &second), 1);
    let resumed_context = rig.probe(&second.tracker, |actor| {
        actor.session.active().unwrap().intervals.context_id()
    });

    rig.clock.advance(1.0).unwrap();
    heal(&rig, &second, 10.0);
    let (classification, tool, context) = latest_output(&rig);
    assert_eq!(classification, "effect");
    assert_eq!(tool.as_deref(), Some("Restoration chip"));
    assert_eq!(
        context, resumed_context,
        "the tick stamps the context it lands in"
    );
    rig.probe(&second.tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(
            active.heal_cost,
            Ped::ZERO,
            "a carried-over tick costs nothing"
        );
        assert_eq!(active.healing.effect_output_count, 1);
    });
    rig.wait(second.tracker.stop_session()).unwrap();
    assert_eq!(
        rig.scalar_f64(
            "SELECT heal_cost FROM tracking_sessions WHERE id = ?",
            &[&resumed.id],
        ),
        0.0
    );
    assert!(
        (rig.scalar_f64(
            "SELECT heal_cost FROM tracking_sessions WHERE id = ?",
            &[&crashed.id],
        ) - 0.04)
            .abs()
            < 1e-12,
        "the activation's cost stays with the session that paid it"
    );
}

#[test]
fn an_expired_window_closes_by_the_clock_and_restoring_twice_changes_nothing() {
    let rig = rig();
    let first = boot(&rig);
    rig.wait(first.tracker.start_session()).unwrap();
    let restoration = restoration();
    press(&rig, &first, &restoration);
    heal(&rig, &first, restoration.confirm);
    rig.wait(first.tracker.stop_session()).unwrap();

    // Two back-to-back session starts read the same window back once each.
    rig.wait(first.tracker.start_session()).unwrap();
    assert_eq!(window_count(&rig, &first), 1);
    rig.wait(first.tracker.stop_session()).unwrap();
    rig.wait(first.tracker.start_session()).unwrap();
    assert_eq!(window_count(&rig, &first), 1);

    // Past expiry plus the delivery tail, a tick no longer belongs to it.
    rig.clock.advance(22.0).unwrap();
    heal(&rig, &first, 10.0);
    assert_eq!(latest_output(&rig).0, "unattributed");
    assert_eq!(window_count(&rig, &first), 0);
    rig.wait(first.tracker.stop_session()).unwrap();
    rig.wait(first.tracker.start_session()).unwrap();
    assert_eq!(
        window_count(&rig, &first),
        0,
        "an expired window is not restored"
    );
}

#[test]
fn a_cooldown_survives_a_restart() {
    let rig = rig();
    let first = boot(&rig);
    rig.wait(first.tracker.start_session()).unwrap();
    let fap = fap();
    press(&rig, &first, &fap);
    heal(&rig, &first, fap.confirm);

    rig.clock.advance(1.0).unwrap();
    let second = boot(&rig);
    let session = rig.wait(second.tracker.start_session()).unwrap();
    press(&rig, &second, &fap);
    heal(&rig, &second, fap.confirm);
    rig.probe(&second.tracker, |actor| {
        assert_eq!(
            actor.session.active().unwrap().healing.activation_count,
            0,
            "a retry inside the reload cannot bill across a restart"
        );
    });
    rig.clock.advance(3.0).unwrap();
    press(&rig, &second, &fap);
    heal(&rig, &second, fap.confirm);
    rig.wait(second.tracker.stop_session()).unwrap();
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_activations WHERE session_id = ?",
            &[&session.id],
        ),
        1
    );
}

#[test]
fn a_correction_takes_a_carried_window_back_and_its_undo_restores_it() {
    let rig = rig();
    let process = boot(&rig);
    let paid = rig.wait(process.tracker.start_session()).unwrap();
    let restoration = restoration();
    press(&rig, &process, &restoration);
    heal(&rig, &process, restoration.confirm);
    rig.wait(process.tracker.stop_session()).unwrap();
    rig.wait(process.tracker.start_session()).unwrap();
    assert_eq!(window_count(&rig, &process), 1);

    let activation_id: String = rig
        .wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT id FROM healing_activations WHERE session_id = ?1",
                [&paid.id],
                |row| row.get(0),
            )?)
        }))
        .unwrap();
    let review = HealingReviewService::new(rig.db.clone(), rig.clock.clone());
    let correction = rig
        .wait(review.correct(CorrectionTarget::NotPaidUse {
            activation_id: activation_id.clone(),
        }))
        .unwrap();
    announce_correction(&process);
    assert_eq!(window_count(&rig, &process), 0);
    rig.clock.advance(2.0).unwrap();
    heal(&rig, &process, 10.0);
    assert_eq!(latest_output(&rig).0, "unattributed");

    rig.wait(review.undo(&correction.id)).unwrap();
    announce_correction(&process);
    assert_eq!(window_count(&rig, &process), 1);
    rig.clock.advance(2.0).unwrap();
    heal(&rig, &process, 10.0);
    assert_eq!(latest_output(&rig).0, "effect");
}

#[test]
fn a_context_change_during_an_effect_leaves_cost_where_it_was_paid() {
    let rig = rig();
    let process = boot(&rig);
    rig.wait(process.tracker.start_session()).unwrap();
    let restoration = restoration();
    press(&rig, &process, &restoration);
    heal(&rig, &process, restoration.confirm);
    let paying_context = rig.probe(&process.tracker, |actor| {
        actor.session.active().unwrap().intervals.context_id()
    });
    rig.wait(process.tracker.activate_activity(
        ActivityRef::Segment {
            name: "Boss".into(),
        },
        false,
    ))
    .unwrap();
    let boss_context = rig.probe(&process.tracker, |actor| {
        actor.session.active().unwrap().intervals.context_id()
    });
    assert_ne!(paying_context, boss_context);
    rig.clock.advance(2.0).unwrap();
    heal(&rig, &process, 10.0);
    assert_eq!(latest_output(&rig).2, boss_context);
    let activation_context: Option<i64> = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(
                conn.query_row("SELECT context_id FROM healing_activations", [], |row| {
                    row.get(0)
                })?,
            )
        }))
        .unwrap();
    assert_eq!(activation_context, paying_context);
}

/// One step of a generated healing rotation.
#[derive(Debug, Clone)]
enum Op {
    Press(usize),
    Weapon,
    Confirm(usize),
    Tick(usize),
    Stray,
    Advance(f64),
    Segment(bool),
    Restart,
}

/// Everything a rotation persisted, keyed by readable identity rather than
/// generated ids, so two runs of related rotations compare directly.
#[derive(Debug, Default, Clone, PartialEq)]
struct Evidence {
    /// (id, heal cost, still active), in start order.
    sessions: Vec<(String, f64, bool)>,
    activations: Vec<Activation>,
    outputs: Vec<Output>,
    windows: Vec<Window>,
}

#[derive(Debug, Clone, PartialEq)]
struct Activation {
    id: String,
    session_id: String,
    tool: String,
    equipment_id: i64,
    observed_at: f64,
    cost: f64,
    context_id: Option<i64>,
    confirming_output_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct Output {
    id: String,
    session_id: String,
    activation_id: Option<String>,
    effect_window_id: Option<String>,
    observed_at: f64,
    amount: f64,
    classification: String,
    context_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
struct Window {
    id: String,
    activation_id: String,
    started_at: f64,
    expires_at: f64,
}

fn read_evidence(rig: &Rig) -> Evidence {
    rig.wait(rig.db.with_reader(|conn| {
        let sessions = conn
            .prepare(
                "SELECT id, COALESCE(heal_cost, 0), is_active FROM tracking_sessions \
                 ORDER BY started_at, rowid",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let activations = conn
            .prepare(
                "SELECT id, session_id, tool_name, equipment_id, observed_at, cost_ped, \
                        context_id, confirming_output_id \
                 FROM healing_activations WHERE superseded_at IS NULL \
                 ORDER BY observed_at, rowid",
            )?
            .query_map([], |row| {
                Ok(Activation {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    tool: row.get(2)?,
                    equipment_id: row.get(3)?,
                    observed_at: row.get(4)?,
                    cost: row.get(5)?,
                    context_id: row.get(6)?,
                    confirming_output_id: row.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let outputs = conn
            .prepare(
                "SELECT id, session_id, activation_id, effect_window_id, observed_at, amount, \
                        classification, context_id \
                 FROM healing_outputs ORDER BY observed_at, rowid",
            )?
            .query_map([], |row| {
                Ok(Output {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    activation_id: row.get(2)?,
                    effect_window_id: row.get(3)?,
                    observed_at: row.get(4)?,
                    amount: row.get(5)?,
                    classification: row.get(6)?,
                    context_id: row.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let windows = conn
            .prepare(
                "SELECT id, activation_id, started_at, expires_at FROM healing_effect_windows \
                 WHERE superseded_at IS NULL ORDER BY started_at, rowid",
            )?
            .query_map([], |row| {
                Ok(Window {
                    id: row.get(0)?,
                    activation_id: row.get(1)?,
                    started_at: row.get(2)?,
                    expires_at: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Evidence {
            sessions,
            activations,
            outputs,
            windows,
        })
    }))
    .unwrap()
}

/// Whether any live window would explain a restoration tick right now,
/// inside its duration proper (the delivery tail is left out, so an
/// injected tick is never a borderline case).
fn restoration_window_open(rig: &Rig, process: &Process) -> bool {
    let at = now(rig);
    rig.probe(&process.tracker, move |actor| {
        actor.session.active().is_some_and(|active| {
            active.healing.effect_windows.iter().any(|window| {
                window.profile.tick_matches(10.0)
                    && window.started_at <= at
                    && at <= window.expires_at
            })
        })
    })
}

/// Play a rotation, optionally slipping an extra restoration tick in
/// after every step while a restoration window is open, then stop.
/// Answers the evidence and the clock time after `mark` steps.
fn play(ops: &[Op], inject_ticks: bool, mark: usize) -> (Evidence, f64) {
    let rig = rig();
    let tools = tools();
    let mut process = boot(&rig);
    rig.wait(process.tracker.start_session()).unwrap();
    let mut segment = false;
    let mut marked = now(&rig);
    for (index, op) in ops.iter().enumerate() {
        match op {
            Op::Press(tool) => press(&rig, &process, &tools[*tool]),
            Op::Weapon => process.bus.publish(&weapon_intent(now(&rig), None)),
            Op::Confirm(tool) => heal(&rig, &process, tools[*tool].confirm),
            Op::Tick(tool) => heal(&rig, &process, tools[*tool].tick.unwrap_or(10.0)),
            Op::Stray => heal(&rig, &process, 150.0),
            Op::Advance(seconds) => rig.clock.advance(*seconds).unwrap(),
            Op::Segment(on) => {
                if *on && !segment {
                    rig.wait(process.tracker.activate_activity(
                        ActivityRef::Segment {
                            name: "Boss".into(),
                        },
                        false,
                    ))
                    .unwrap();
                } else if !*on && segment {
                    rig.wait(
                        process
                            .tracker
                            .deactivate_activity(ActivityKey::Segment("Boss".into())),
                    )
                    .unwrap();
                }
                segment = *on;
            }
            Op::Restart => {
                process = boot(&rig);
                rig.wait(process.tracker.start_session()).unwrap();
                segment = false;
            }
        }
        if inject_ticks && restoration_window_open(&rig, &process) {
            heal(&rig, &process, 10.0);
        }
        if index + 1 == mark {
            marked = now(&rig);
        }
    }
    rig.wait(process.tracker.stop_session()).unwrap();
    (read_evidence(&rig), marked)
}

fn tool_named(name: &str) -> Tool {
    tools()
        .into_iter()
        .find(|tool| tool.name == name)
        .expect("a configured tool")
}

/// The billing invariants every rotation must keep.
fn assert_invariants(evidence: &Evidence) {
    for (session_id, heal_cost, active) in &evidence.sessions {
        assert!(!active, "every session is closed");
        let billed: f64 = evidence
            .activations
            .iter()
            .filter(|activation| &activation.session_id == session_id)
            .map(|activation| activation.cost)
            .sum();
        assert!(
            (heal_cost - billed).abs() < 1e-9,
            "session heal cost {heal_cost} equals its paid activations {billed}"
        );
    }
    for activation in &evidence.activations {
        let tool = tool_named(&activation.tool);
        assert!(
            (activation.cost - tool.cost).abs() < 1e-12,
            "one use costs one use"
        );
        let confirming = evidence
            .outputs
            .iter()
            .find(|output| Some(&output.id) == activation.confirming_output_id.as_ref())
            .expect("every activation names the output that confirmed it");
        assert_eq!(confirming.activation_id.as_ref(), Some(&activation.id));
        assert_eq!(confirming.session_id, activation.session_id);
        assert_eq!(
            confirming.context_id, activation.context_id,
            "cost stays in the context it was paid in"
        );
    }
    for tool in tools() {
        let mut uses = evidence
            .activations
            .iter()
            .filter(|activation| activation.equipment_id == tool.id)
            .map(|activation| activation.observed_at)
            .collect::<Vec<_>>();
        uses.sort_by(f64::total_cmp);
        for pair in uses.windows(2) {
            assert!(
                pair[1] - pair[0] >= tool.reload - 1e-9,
                "{} billed twice inside its reload",
                tool.name
            );
        }
    }
    for output in &evidence.outputs {
        let Some(window_id) = &output.effect_window_id else {
            continue;
        };
        let window = evidence
            .windows
            .iter()
            .find(|window| &window.id == window_id)
            .expect("an explained tick names a live window");
        assert_eq!(output.activation_id.as_ref(), Some(&window.activation_id));
        assert!(output.observed_at + 0.05 >= window.started_at);
        assert!(output.observed_at <= window.expires_at + 1.25);
    }
}

/// Paid uses by readable identity: tool, time, cost, and the session's
/// start order.
fn paid_uses(evidence: &Evidence) -> Vec<(String, f64, f64, usize)> {
    evidence
        .activations
        .iter()
        .map(|activation| {
            let session = evidence
                .sessions
                .iter()
                .position(|(id, _, _)| id == &activation.session_id)
                .expect("an activation's session exists");
            (
                activation.tool.clone(),
                activation.observed_at,
                activation.cost,
                session,
            )
        })
        .collect()
}

/// One output as provenance comparison sees it: when, how much, how it was
/// explained, and the paying activation by tool and time.
type Explained = (f64, f64, String, Option<(String, f64)>);

/// Outputs observed after `from`, with the paying activation named by tool
/// and time.
fn explained_after(evidence: &Evidence, from: f64) -> Vec<Explained> {
    evidence
        .outputs
        .iter()
        .filter(|output| output.observed_at > from)
        .map(|output| {
            let paid_by = output.activation_id.as_ref().map(|id| {
                let activation = evidence
                    .activations
                    .iter()
                    .find(|activation| &activation.id == id)
                    .expect("a named activation exists");
                (activation.tool.clone(), activation.observed_at)
            });
            (
                output.observed_at,
                output.amount,
                output.classification.clone(),
                paid_by,
            )
        })
        .collect()
}

mod rotations {
    use proptest::prelude::*;

    use super::*;

    fn step() -> impl Strategy<Value = Op> {
        prop_oneof![
            3 => (0usize..3).prop_map(Op::Press),
            1 => Just(Op::Weapon),
            3 => (0usize..3).prop_map(Op::Confirm),
            3 => (1usize..3).prop_map(Op::Tick),
            1 => Just(Op::Stray),
            4 => prop::sample::select(vec![0.5, 1.0, 2.0, 5.0, 15.0]).prop_map(Op::Advance),
            1 => any::<bool>().prop_map(Op::Segment),
            1 => Just(Op::Restart),
        ]
    }

    fn afterwards() -> impl Strategy<Value = Op> {
        prop_oneof![
            3 => (1usize..3).prop_map(Op::Tick),
            1 => Just(Op::Stray),
            2 => prop::sample::select(vec![0.5, 1.0, 2.0, 5.0]).prop_map(Op::Advance),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        /// Whatever the rotation, restarts, and context changes: a session's
        /// heal cost is exactly its paid activations, each priced once at its
        /// tool's cost, confirmed by one output in the context it was paid
        /// in, never inside its reload; every explained tick lies inside the
        /// live window that names its activation.
        #[test]
        fn billing_invariants_hold_for_any_rotation(
            ops in prop::collection::vec(step(), 1..40),
        ) {
            let (evidence, _) = play(&ops, false, 0);
            assert_invariants(&evidence);
        }

        /// Extra ticks arriving while an effect is open never add or move a
        /// paid use: the effect's activation is billed once however many
        /// ticks it produces.
        #[test]
        fn ticks_inside_an_open_effect_never_bill(
            ops in prop::collection::vec(step(), 1..40),
        ) {
            let (plain, _) = play(&ops, false, 0);
            let (ticking, _) = play(&ops, true, 0);
            assert_invariants(&ticking);
            prop_assert_eq!(paid_uses(&plain), paid_uses(&ticking));
            let plain_costs: Vec<f64> = plain.sessions.iter().map(|session| session.1).collect();
            let ticking_costs: Vec<f64> = ticking.sessions.iter().map(|session| session.1).collect();
            prop_assert_eq!(plain_costs, ticking_costs);
        }

        /// Once the healer is put away, a restart (or two) changes nothing
        /// about how later outputs are explained: running effects keep
        /// their paying activation and expired ones explain nothing.
        #[test]
        fn a_restart_is_invisible_to_effect_provenance(
            before in prop::collection::vec(step(), 0..25),
            restarts in 1usize..3,
            after in prop::collection::vec(afterwards(), 1..20),
        ) {
            let mut plain = before.clone();
            plain.push(Op::Weapon);
            plain.push(Op::Advance(2.0));
            let mark = plain.len();
            let mut restarted = plain.clone();
            restarted.extend(std::iter::repeat_n(Op::Restart, restarts));
            plain.extend(after.iter().cloned());
            restarted.extend(after.iter().cloned());

            let (plain_evidence, from) = play(&plain, false, mark);
            let (restarted_evidence, restarted_from) = play(&restarted, false, mark);
            assert_invariants(&restarted_evidence);
            prop_assert_eq!(from, restarted_from);
            prop_assert_eq!(
                explained_after(&plain_evidence, from),
                explained_after(&restarted_evidence, from)
            );
        }
    }
}

#[test]
fn the_rig_clock_starts_where_the_intents_expect() {
    // The generated rotations stamp intents from the injected clock, the
    // same instant the tracker stamps outputs with.
    let rig = rig();
    assert_eq!(now(&rig), naive_to_epoch(naive("2026-01-01T00:00:00")));
}
