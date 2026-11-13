//! The tool in hand read off the screen, through the real actor: changes
//! from a stand-in watcher and shots and presses from the bus, in either
//! order of arrival, and what each shot is booked to.

use std::sync::{Arc, Mutex};

use super::actor::TrackerMsg;
use super::attribution::{AttributionCounts, ShotLocation, WeaponMismatch};
use super::tests::{carried, carried_cost, hit, rig, Rig};
use super::{
    BundledTool, BundledWeapon, CarriedWeaponProfile, EquipmentLibrary, EquipmentProfile,
    HarvestGuardrailTools, HuntTracker, Providers, ScreenTool, ToolDeclared, ToolSource,
    TrackerCommandError, TrackingConfig,
};
use crate::bus_events::{
    BusEvent, CombatPayload, EnhancerBreakPayload, EnhancerBreakTag, HotbarIntentPayload,
    HotbarItemKind, IntentSource, LootGroupPayload, LootTag,
};
use crate::clock::Clock;
use crate::consumables::ConsumableProfile;
use crate::healing_profile::{HealingMode, HealingProfile};
use crate::hotbar_listener::ResolvedHotbarItem;
use crate::screen_watch::WatchEnded;
use crate::time::{clock_epoch_ms, naive_to_epoch};
use crate::tool_watch::{HeldTool, ToolDetector, ToolPulse, ToolPulseSink, ToolSeen, Why};

/// The configuration a test runs under: the reading switchable mid-session,
/// and whether the machine runs keyless.
#[derive(Default)]
struct ScreenConfig {
    reading: Arc<Mutex<bool>>,
    keyless: bool,
}

impl TrackingConfig for ScreenConfig {
    fn session_name(&self) -> String {
        String::new()
    }

    fn session_definition_id(&self) -> Option<i64> {
        None
    }

    fn declared_skill_boost_percent(&self) -> Option<i64> {
        None
    }

    fn manual_mob(&self) -> Option<(String, String)> {
        None
    }

    fn loot_filter_blacklist(&self) -> Vec<String> {
        Vec::new()
    }

    fn keyless_input(&self) -> bool {
        self.keyless
    }

    fn tool_reading_enabled(&self) -> bool {
        *self.reading.lock().unwrap()
    }
}

/// A library of three weapons on, of which the hotbar binds only the
/// pistol: Pistol 5-10, Cannon 20-40, Rifle 8-16 (overlapping the pistol).
struct Library(Vec<CarriedWeaponProfile>);

fn library_weapons() -> Vec<CarriedWeaponProfile> {
    vec![
        carried(1, "Pistol", 10.0, 0.05),
        carried(2, "Cannon", 40.0, 0.2),
        carried(3, "Rifle", 16.0, 0.1),
    ]
}

impl EquipmentLibrary for Library {
    fn weapon_profile(&self, _tool_name: &str) -> EquipmentProfile {
        None
    }

    fn cost_per_shot(&self, _tool_name: &str) -> f64 {
        0.0
    }

    fn carried_weapons(&self) -> Vec<CarriedWeaponProfile> {
        vec![carried(1, "Pistol", 10.0, 0.05)]
    }

    fn enabled_weapons(&self) -> Vec<CarriedWeaponProfile> {
        self.0.clone()
    }

    fn resolve_harvest_guardrail(&self) -> Option<HarvestGuardrailTools> {
        None
    }
}

fn pistol() -> f64 {
    carried_cost(10.0, 0.05)
}

fn cannon() -> f64 {
    carried_cost(40.0, 0.2)
}

fn rifle() -> f64 {
    carried_cost(16.0, 0.1)
}

fn rusty() -> f64 {
    carried_cost(200.0, 1.0)
}

/// Zapper: its paid hit prints 100-160 and opens an effect ticking 50-75
/// for ten seconds.
fn zapper() -> CarriedWeaponProfile {
    let mut profile = carried(7, "Zapper", 160.0, 2.0);
    profile.props.insert(
        crate::weapon_effect::EFFECT_PROFILE_KEY.to_string(),
        serde_json::json!({
            "mode": "compound",
            "hit_min": 100.0,
            "hit_max": 160.0,
            "duration_seconds": 10.0,
            "tick_min": 50.0,
            "tick_max": 75.0,
            "tick_seconds": 1.0,
        }),
    );
    profile.weapon = super::CarriedWeapon::from_props(
        7,
        "Zapper".into(),
        &serde_json::Value::Object(profile.props.clone()),
    );
    profile
}

/// A watcher that looks at no screen: the test hands the tracker each
/// change through the sink the tracker gave it.
#[derive(Default)]
struct ScriptedWatcher {
    sink: Mutex<Option<(String, ToolPulseSink)>>,
    ended: Mutex<Option<WatchEnded>>,
    starts: Mutex<Vec<String>>,
    stops: Mutex<usize>,
    permits: Mutex<usize>,
    hints: Mutex<usize>,
    /// While set, a start does not take: capture is not running.
    refuse: Mutex<bool>,
}

impl ToolDetector for ScriptedWatcher {
    fn start(&self, session_id: &str, sink: ToolPulseSink, ended: WatchEnded) {
        self.starts.lock().unwrap().push(session_id.to_string());
        if *self.refuse.lock().unwrap() {
            return;
        }
        *self.ended.lock().unwrap() = Some(ended);
        *self.sink.lock().unwrap() = Some((session_id.to_string(), sink));
    }

    fn stop(&self) {
        *self.stops.lock().unwrap() += 1;
        *self.sink.lock().unwrap() = None;
    }

    fn watches(&self, session_id: &str) -> bool {
        self.sink
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|(watched, _)| watched == session_id)
    }

    fn permit_capture(&self) {
        *self.permits.lock().unwrap() += 1;
    }

    fn hint(&self) {
        *self.hints.lock().unwrap() += 1;
    }
}

fn item(id: i64, name: &str, kind: HotbarItemKind) -> ResolvedHotbarItem {
    ResolvedHotbarItem {
        equipment_id: id,
        name: name.into(),
        kind,
        cost_per_use_ped: 0.0,
        reload_seconds: 0.0,
        healing_profile: None,
        lifesteal_percent: None,
        consumable_profile: None,
        setup: Default::default(),
    }
}

/// A healer healing 60-100 directly, 3 s to reload, at 3 PEC a use.
fn fap_profile() -> HealingProfile {
    HealingProfile {
        direct_min: Some(60.0),
        direct_max: Some(100.0),
        ..HealingProfile::default()
    }
}

fn fap() -> ResolvedHotbarItem {
    ResolvedHotbarItem {
        cost_per_use_ped: 0.03,
        reload_seconds: 3.0,
        healing_profile: Some(fap_profile()),
        ..item(4, "FAP", HotbarItemKind::Healing)
    }
}

fn finder() -> ResolvedHotbarItem {
    ResolvedHotbarItem {
        cost_per_use_ped: 0.02,
        ..item(6, "Terratech F1", HotbarItemKind::Harvesting)
    }
}

/// Leech: a weapon whose hits heal half their damage back.
fn leech() -> ResolvedHotbarItem {
    ResolvedHotbarItem {
        lifesteal_percent: Some(50.0),
        ..item(9, "Leech", HotbarItemKind::Weapon)
    }
}

/// Mender: heals 60-100 at once and opens an effect ticking 60-70 for
/// twenty seconds, 3 s to reload, at 4 PEC a use. Its ticks fall inside its
/// direct band, so a tick and a fresh use can read the same.
fn mender() -> ResolvedHotbarItem {
    ResolvedHotbarItem {
        cost_per_use_ped: 0.04,
        reload_seconds: 3.0,
        healing_profile: Some(HealingProfile {
            mode: HealingMode::Compound,
            direct_min: Some(60.0),
            direct_max: Some(100.0),
            effect_duration_seconds: Some(20.0),
            tick_min: Some(60.0),
            tick_max: Some(70.0),
            tick_seconds: Some(2.0),
            ..HealingProfile::default()
        }),
        ..item(10, "Mender", HotbarItemKind::Healing)
    }
}

fn vita() -> ConsumableProfile {
    ConsumableProfile {
        duration_seconds: 60.0,
        effects: Vec::new(),
        tt_value_ped: 1.0,
        markup_percent: 100.0,
        track_cost: true,
    }
}

/// The bundled tools the data knows and the library does not hold: Rusty
/// Rifle (100-200), Old FAP (a healer) and Rusty Finder (a finder).
fn bundled(endpoint: &str, item_id: &str) -> Option<BundledTool> {
    match (endpoint, item_id) {
        ("weapons", "900") => {
            let profile = carried(0, "Rusty Rifle", 200.0, 1.0);
            Some(BundledTool {
                name: "Rusty Rifle".into(),
                kind: HotbarItemKind::Weapon,
                cost_per_use_ped: rusty(),
                weapon: Some(BundledWeapon {
                    band: profile.weapon.band,
                    props: profile.props,
                }),
            })
        }
        ("medical_tools", "901") => Some(BundledTool {
            name: "Old FAP".into(),
            kind: HotbarItemKind::Healing,
            cost_per_use_ped: 0.05,
            weapon: None,
        }),
        ("harvesting_tools", "902") => Some(BundledTool {
            name: "Rusty Finder".into(),
            kind: HotbarItemKind::Harvesting,
            cost_per_use_ped: 0.04,
            weapon: None,
        }),
        _ => None,
    }
}

fn seen_bundled(endpoint: &str, item_id: &str, name: &str) -> ToolSeen {
    ToolSeen::Holding(HeldTool::Bundled {
        endpoint: endpoint.into(),
        item_id: item_id.into(),
        name: name.into(),
    })
}

fn rusty_seen() -> ToolSeen {
    // The bar's spelling may differ from the data's: shots are booked under
    // the data's name.
    seen_bundled("weapons", "900", "Rusty Rifle (L)")
}

fn holding(id: i64) -> ToolSeen {
    ToolSeen::Holding(HeldTool::Library { id })
}

struct Hunt {
    rig: Rig,
    tracker: Arc<HuntTracker>,
    watcher: Arc<ScriptedWatcher>,
    reading: Arc<Mutex<bool>>,
}

fn hunt(reading: bool) -> Hunt {
    hunt_with(reading, false)
}

fn hunt_with(reading: bool, keyless: bool) -> Hunt {
    hunt_over(reading, keyless, library_weapons())
}

fn hunt_over(reading: bool, keyless: bool, weapons: Vec<CarriedWeaponProfile>) -> Hunt {
    let rig = rig();
    let watcher = Arc::new(ScriptedWatcher::default());
    let reading = Arc::new(Mutex::new(reading));
    let tracker = rig.tracker(Providers {
        config: Arc::new(ScreenConfig {
            reading: reading.clone(),
            keyless,
        }),
        equipment: Arc::new(Library(weapons)),
        tool_detector: watcher.clone(),
        items: Some(Arc::new(|id| match id {
            1 => Some(item(1, "Pistol", HotbarItemKind::Weapon)),
            2 => Some(item(2, "Cannon", HotbarItemKind::Weapon)),
            3 => Some(item(3, "Rifle", HotbarItemKind::Weapon)),
            4 => Some(fap()),
            6 => Some(finder()),
            7 => Some(item(7, "Zapper", HotbarItemKind::Weapon)),
            9 => Some(leech()),
            10 => Some(mender()),
            _ => None,
        })),
        bundled_tools: Some(Arc::new(bundled)),
        ..Providers::default()
    });
    Hunt {
        rig,
        tracker,
        watcher,
        reading,
    }
}

/// One phase: its tool, shots and per-shot cost.
type Phase = (String, i64, f64);

/// Everything a placement of shots decides, as a test compares it: each
/// kept shot (time, classification, fits, weapon, price, row kept, where
/// its cost sits) in the order the regime remembers them, the phases, the
/// stored rows, the tallies, the standing mismatch, the unpriced bound,
/// and every kill's cost and phases with its stored rows.
#[derive(Debug, PartialEq)]
struct Placed {
    retained: Vec<String>,
    log: Vec<String>,
    pending: Vec<Phase>,
    rows: Vec<String>,
    counts: AttributionCounts,
    mismatch: Option<WeaponMismatch>,
    bound: (f64, i64),
    kills: Vec<(f64, i64, i64, Vec<Phase>)>,
    stored_rows: Vec<String>,
    windows: Vec<String>,
    stored_windows: Vec<String>,
    shots: (i64, i64),
}

impl Hunt {
    fn start(&self) -> String {
        self.rig.wait(self.tracker.start_session()).unwrap().id
    }

    fn now(&self) -> f64 {
        naive_to_epoch(self.rig.clock.now())
    }

    fn advance(&self, seconds: f64) {
        self.rig.clock.advance(seconds).unwrap();
    }

    fn sink(&self) -> (String, ToolPulseSink) {
        self.watcher
            .sink
            .lock()
            .unwrap()
            .clone()
            .expect("the watch runs")
    }

    /// The watcher tells `seen`, first seen `ago` seconds before now.
    fn see(&self, seen: ToolSeen, ago: f64) {
        let (session_id, sink) = self.sink();
        self.tell(session_id, sink, seen, ago);
    }

    fn tell(&self, session_id: String, sink: ToolPulseSink, seen: ToolSeen, ago: f64) {
        let at_ms = clock_epoch_ms(self.rig.clock.as_ref()) - (ago * 1000.0).round() as i64;
        assert!(sink(ToolPulse {
            session_id,
            at_ms,
            seen,
        }));
        // The actor takes messages in order: a call after the change is
        // answered once the change is applied.
        self.source();
    }

    fn shot(&self, amount: f64) {
        self.rig.bus.publish(&hit(amount));
    }

    fn heal(&self, amount: f64) {
        self.rig
            .bus
            .publish(&BusEvent::Combat(CombatPayload::SelfHeal {
                amount,
                timestamp: self.rig.clock.now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            }));
    }

    fn key(&self, name: &str, kind: HotbarItemKind) {
        self.intent(IntentSource::Key, name, kind);
    }

    fn intent(&self, source: IntentSource, name: &str, kind: HotbarItemKind) {
        let resolved = match name {
            "Pistol" => item(1, "Pistol", kind),
            "Cannon" => item(2, "Cannon", kind),
            "Rifle" => item(3, "Rifle", kind),
            "FAP" => fap(),
            "Mender" => mender(),
            "Restoration" => ResolvedHotbarItem {
                healing_profile: Some(fap_profile()),
                ..item(8, "Restoration", kind)
            },
            other => item(5, other, kind),
        };
        self.rig
            .bus
            .publish(&BusEvent::HotbarIntent(Box::new(HotbarIntentPayload {
                session_id: None,
                slot: if source == IntentSource::Key {
                    "1".into()
                } else {
                    String::new()
                },
                source,
                occurred_at: self.now(),
                equipment_id: resolved.equipment_id,
                item_name: resolved.name,
                item_kind: kind,
                cost_per_use_ped: resolved.cost_per_use_ped,
                reload_seconds: resolved.reload_seconds,
                healing_profile: resolved.healing_profile,
                lifesteal_percent: None,
                consumable_profile: (kind == HotbarItemKind::Consumable).then(vita),
            })));
    }

    fn declare(&self, resolved: ResolvedHotbarItem) -> Result<ToolDeclared, TrackerCommandError> {
        self.rig
            .wait(self.tracker.declare_tool(resolved, self.now()))
    }

    fn loot(&self) {
        self.rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
            kind: LootTag,
            source_id: None,
            arrived_ms: None,
            timestamp: Some("2026-01-01T00:00:02".into()),
            items: vec![],
            total_ped: 0.0,
        }));
    }

    fn source(&self) -> ToolSource {
        self.rig.wait(self.tracker.tool_source())
    }

    fn screen(&self) -> ScreenTool {
        match self.source() {
            ToolSource::Screen(screen) => screen,
            other => panic!("the screen is not the source: {other:?}"),
        }
    }

    fn reload(&self) {
        self.rig.wait(self.tracker.reload_config());
    }

    /// The accumulator's phases as (tool, shots), sorted, with the cost of
    /// each phase checked against the weapon it names.
    fn pending(&self) -> Vec<(String, i64)> {
        let phases: Vec<(String, i64, f64)> = self.rig.probe(&self.tracker, |actor| {
            actor
                .session
                .active()
                .unwrap()
                .accumulator
                .tool_stats
                .iter()
                .map(|(_, stats)| {
                    (
                        stats.tool_name.clone(),
                        stats.shots_fired,
                        stats.cost_per_shot.value(),
                    )
                })
                .filter(|(_, shots, _)| *shots > 0)
                .collect()
        });
        let mut checked: Vec<(String, i64)> = phases
            .into_iter()
            .map(|(tool, shots, cost)| {
                let expected = match tool.as_str() {
                    "Pistol" => pistol(),
                    "Cannon" => cannon(),
                    "Rusty Rifle" => rusty(),
                    "Rifle" => rifle(),
                    _ => 0.0,
                };
                assert!(
                    (cost - expected).abs() < 1e-12,
                    "{tool} is booked at {cost}, not {expected}"
                );
                (tool, shots)
            })
            .collect();
        checked.sort();
        checked
    }

    fn held(&self) -> Option<(String, HotbarItemKind)> {
        self.rig
            .probe(&self.tracker, |actor| actor.held_item.clone())
    }

    fn declared(&self) -> Option<String> {
        self.rig.probe(&self.tracker, |actor| {
            actor
                .session
                .active()
                .and_then(|active| active.weapons.attribution.declared().map(str::to_string))
        })
    }

    /// The unpriced rows waiting with the accumulator.
    fn unpriced_rows(&self) -> usize {
        self.rig.probe(&self.tracker, |actor| {
            actor
                .session
                .active()
                .unwrap()
                .accumulator
                .evidence
                .iter()
                .filter(|row| {
                    row.kind == super::attribution::AttributionKind::Unresolved
                        && row.tool_name.is_none()
                })
                .count()
        })
    }

    fn counts(&self) -> AttributionCounts {
        self.rig.probe(&self.tracker, |actor| {
            actor.session.active().unwrap().weapons.attribution.counts()
        })
    }

    fn bound(&self) -> f64 {
        self.rig.probe(&self.tracker, |actor| {
            actor.session.active().unwrap().unpriced_cost_bound.value()
        })
    }

    fn retained(&self) -> usize {
        self.rig.probe(&self.tracker, |actor| {
            actor
                .session
                .active()
                .unwrap()
                .weapons
                .attribution
                .retained_count()
        })
    }

    fn placed(&self) -> Placed {
        let (
            retained,
            log,
            pending,
            rows,
            counts,
            mismatch,
            bound,
            kills,
            kill_ids,
            windows,
            shots,
        ) = self.rig.probe(&self.tracker, |actor| {
            let active = actor.session.active().unwrap();
            let kill_ids: Vec<String> = active
                .session
                .kills
                .iter()
                .map(|kill| kill.id.clone())
                .collect();
            let place = |location: &ShotLocation| match location {
                ShotLocation::Pending => "pending".to_string(),
                ShotLocation::Kill(id) => format!(
                    "kill {}",
                    kill_ids.iter().position(|kill| kill == id).unwrap()
                ),
            };
            let runtime = &active.weapons.attribution;
            let retained = runtime
                .retained_since(f64::NEG_INFINITY)
                .iter()
                .map(|shot| {
                    let record = &shot.record;
                    let attribution = match &record.attribution {
                        // A tick names its window by an id each run mints.
                        super::attribution::Attribution::EffectTick { window_id } => {
                            format!("tick of one window: {}", window_id.is_some())
                        }
                        other => format!("{other:?}"),
                    };
                    format!(
                        "{:.3} {} {:?} {:?} {:.6} row:{} {} {:?} opened:{} decided:{}",
                        record.observed_at,
                        attribution,
                        record.fits,
                        record.booked,
                        record.cost.value(),
                        record.evidence_id.is_some(),
                        place(&record.location),
                        shot.charge,
                        record.opened_window.is_some(),
                        shot.decided_by.is_some(),
                    )
                })
                .collect::<Vec<_>>();
            let log = runtime
                .log()
                .iter()
                .map(|record| {
                    format!(
                        "{:.3} {:?} {:?}",
                        record.observed_at, record.attribution, record.booked
                    )
                })
                .collect::<Vec<_>>();
            let pending = active
                .accumulator
                .tool_stats
                .iter()
                .map(|(_, stats)| {
                    (
                        stats.tool_name.clone(),
                        stats.shots_fired,
                        stats.cost_per_shot.value(),
                    )
                })
                .collect::<Vec<_>>();
            let rows = active
                .accumulator
                .evidence
                .iter()
                .map(|row| {
                    format!(
                        "{:.3} {:?} {:?} {:?} {:.6} {} {:?} {:?}",
                        row.observed_at,
                        row.kind,
                        row.hotbar_tool,
                        row.tool_name,
                        row.cost_per_shot.value(),
                        row.reason,
                        row.context_id,
                        row.amount,
                    )
                })
                .collect::<Vec<_>>();
            let mut rows = rows;
            rows.sort();
            let kills = active
                .session
                .kills
                .iter()
                .map(|kill| {
                    (
                        kill.cost_ped.value(),
                        kill.shots_fired,
                        kill.critical_hits,
                        kill.tool_stats
                            .iter()
                            .map(|(_, stats)| {
                                (
                                    stats.tool_name.clone(),
                                    stats.shots_fired,
                                    stats.cost_per_shot.value(),
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            (
                retained,
                log,
                pending,
                rows,
                runtime.counts(),
                runtime.mismatch().cloned(),
                (active.unpriced_cost_bound.value(), active.unpriced_no_fit),
                kills,
                kill_ids,
                {
                    let mut windows = runtime
                        .effect_windows()
                        .iter()
                        .map(|window| {
                            format!(
                                "{} {:.3} {:.3}",
                                window.tool, window.started_at, window.expires_at
                            )
                        })
                        .collect::<Vec<_>>();
                    windows.sort();
                    windows
                },
                (
                    active.accumulator.shots_fired,
                    active.accumulator.critical_hits,
                ),
            )
        });
        let stored_rows: Vec<(String, f64, String, Option<String>, f64, String)> = self
            .rig
            .wait(self.rig.db.with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT COALESCE(kill_id, ''), observed_at, attribution, tool_name, \
                     cost_per_shot, reason FROM weapon_shot_evidence ORDER BY observed_at, rowid",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            }))
            .unwrap();
        let mut stored_rows: Vec<String> = stored_rows
            .into_iter()
            .map(|(kill, at, kind, tool, cost, reason)| {
                let kill = kill_ids.iter().position(|id| *id == kill);
                format!("{kill:?} {at:.3} {kind} {tool:?} {cost:.6} {reason}")
            })
            .collect();
        stored_rows.sort();
        let mut stored_windows: Vec<String> = self
            .rig
            .wait(self.rig.db.with_reader(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT tool_name || ' ' || printf('%.3f', started_at) || ' ' || \
                     printf('%.3f', expires_at) || ' ' || (withdrawn_at IS NULL) \
                     FROM weapon_effect_windows",
                )?;
                let rows = stmt.query_map([], |row| row.get(0))?;
                Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
            }))
            .unwrap();
        stored_windows.sort();
        Placed {
            retained,
            log,
            pending,
            rows,
            counts,
            mismatch,
            bound,
            kills,
            stored_rows,
            windows,
            stored_windows,
            shots,
        }
    }
}

fn phases(list: &[(&str, i64)]) -> Vec<(String, i64)> {
    let mut phases: Vec<(String, i64)> = list
        .iter()
        .map(|(tool, shots)| (tool.to_string(), *shots))
        .collect();
    phases.sort();
    phases
}

/// The healer activations stored: (intent source, cost, intent time).
fn activations(hunt: &Hunt) -> Vec<(Option<String>, f64, f64)> {
    hunt.rig
        .wait(hunt.rig.db.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT intent_source, cost_ped, intent_at FROM healing_activations \
                 ORDER BY observed_at, rowid",
            )?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        }))
        .unwrap()
}

/// The heals stored, by classification.
fn heal_classes(hunt: &Hunt) -> Vec<String> {
    hunt.rig
        .wait(hunt.rig.db.with_reader(|conn| {
            let mut stmt =
                conn.prepare("SELECT classification FROM healing_outputs ORDER BY observed_at")?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
        }))
        .unwrap()
}

// 1. The source of the tool.

#[test]
fn a_library_tool_read_off_the_screen_is_declared_and_priced_from_its_entry() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(2), 0.0);
    assert_eq!(hunt.declared().as_deref(), Some("Cannon"));
    assert_eq!(hunt.held(), Some(("Cannon".into(), HotbarItemKind::Weapon)));
    // 3 fits no weapon: held, it is the cannon's, short of its band.
    hunt.shot(30.0);
    hunt.shot(3.0);
    assert_eq!(hunt.pending(), phases(&[("Cannon", 2)]));
}

#[test]
fn a_library_healer_read_off_the_screen_bills_its_heals_on_the_screens_word() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(4), 0.0);
    assert_eq!(hunt.held(), Some(("FAP".into(), HotbarItemKind::Healing)));
    hunt.advance(0.5);
    hunt.heal(80.0);
    let stored = activations(&hunt);
    assert_eq!(stored.len(), 1, "the heal is billed");
    assert_eq!(stored[0].0.as_deref(), Some("screen"));
    assert!((stored[0].1 - 0.03).abs() < 1e-12);
    let heal_cost = hunt.rig.probe(&hunt.tracker, |actor| {
        actor.session.active().unwrap().heal_cost.value()
    });
    assert!((heal_cost - 0.03).abs() < 1e-12);
}

#[test]
fn a_library_finder_read_off_the_screen_is_the_harvesting_tool_in_force() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(6), 0.0);
    let tool = hunt.rig.probe(&hunt.tracker, |actor| {
        actor
            .harvest_tool
            .as_ref()
            .map(|tool| (tool.name.clone(), tool.cost_per_use.value()))
    });
    assert_eq!(tool, Some(("Terratech F1".into(), 0.02)));
    assert_eq!(
        hunt.held(),
        Some(("Terratech F1".into(), HotbarItemKind::Harvesting))
    );
}

#[test]
fn a_tool_read_off_the_screen_serialises_its_source_and_a_key_does_not() {
    let screen = HotbarIntentPayload {
        session_id: None,
        slot: String::new(),
        source: IntentSource::Screen,
        occurred_at: 1.0,
        equipment_id: 1,
        item_name: "Pistol".into(),
        item_kind: HotbarItemKind::Weapon,
        cost_per_use_ped: 0.0,
        reload_seconds: 0.0,
        healing_profile: None,
        lifesteal_percent: None,
        consumable_profile: None,
    };
    let value = serde_json::to_value(&screen).unwrap();
    assert_eq!(value["source"], "screen");
    let key = HotbarIntentPayload {
        source: IntentSource::Key,
        ..screen
    };
    let value = serde_json::to_value(&key).unwrap();
    assert!(
        value.get("source").is_none(),
        "a key press serialises as before"
    );
}

// 2. Keys.

#[test]
fn a_weapon_key_declares_nothing_and_asks_the_watcher_to_look() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.key("Cannon", HotbarItemKind::Weapon);
    assert_eq!(*hunt.watcher.hints.lock().unwrap(), 1);
    assert_eq!(hunt.declared().as_deref(), Some("Pistol"));
    hunt.shot(9.0);
    assert_eq!(hunt.pending(), phases(&[("Pistol", 1)]));
}

#[test]
fn a_consumable_key_still_takes_its_dose() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.key("Vita Bar", HotbarItemKind::Consumable);
    let doses = hunt
        .rig
        .probe(&hunt.tracker, |actor| actor.doses.running.len());
    assert_eq!(doses, 1);
    assert_eq!(*hunt.watcher.hints.lock().unwrap(), 0);
    assert_eq!(hunt.declared().as_deref(), Some("Pistol"));
}

#[test]
fn the_key_of_the_healer_in_hand_is_a_use_and_any_other_key_only_a_hint() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(4), 0.0);
    hunt.advance(0.5);
    hunt.heal(80.0);
    hunt.advance(4.0);
    let key_at = hunt.now();
    hunt.key("FAP", HotbarItemKind::Healing);
    assert_eq!(*hunt.watcher.hints.lock().unwrap(), 0, "a use, not a hint");
    hunt.advance(0.5);
    hunt.heal(80.0);
    let stored = activations(&hunt);
    assert_eq!(stored.len(), 2);
    assert_eq!(
        stored[1].0.as_deref(),
        Some("key"),
        "the press opened the use"
    );
    assert!((stored[1].2 - key_at).abs() < 1e-6);
    // Another healer's key changes nothing in hand and asks for a look.
    hunt.key("Restoration", HotbarItemKind::Healing);
    assert_eq!(*hunt.watcher.hints.lock().unwrap(), 1);
    assert_eq!(hunt.held(), Some(("FAP".into(), HotbarItemKind::Healing)));
}

#[test]
fn without_keys_the_screens_one_intent_stands_for_the_held_healer() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(4), 0.0);
    hunt.advance(0.5);
    hunt.heal(80.0);
    hunt.advance(4.0);
    hunt.heal(80.0);
    let stored = activations(&hunt);
    assert_eq!(stored.len(), 2, "each paid use past the reload is billed");
    assert!(stored
        .iter()
        .all(|(source, _, _)| source.as_deref() == Some("screen")));
}

// 3. The overlay's declaration.

#[test]
fn a_declaration_is_refused_while_the_screen_shows_the_tool() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    assert_eq!(
        hunt.declare(item(2, "Cannon", HotbarItemKind::Weapon)),
        Err(TrackerCommandError::ScreenShowsTool)
    );
    // An overlay declaration arriving on the bus is refused the same way.
    hunt.intent(IntentSource::Overlay, "Cannon", HotbarItemKind::Weapon);
    assert_eq!(hunt.declared().as_deref(), Some("Pistol"));
    assert_eq!(hunt.screen().manual, None);
    // A consumable, used and never held, is never refused.
    assert_eq!(
        hunt.declare(item(5, "Vita Bar", HotbarItemKind::Consumable)),
        Ok(ToolDeclared::KeysAreSource)
    );
}

#[test]
fn a_declaration_stands_where_the_screen_cannot_say_until_it_shows_something() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::Uncertain(Why::Unreadable), 0.0);
    assert_eq!(
        hunt.declare(item(2, "Cannon", HotbarItemKind::Weapon)),
        Ok(ToolDeclared::Taken)
    );
    assert_eq!(hunt.declared().as_deref(), Some("Cannon"));
    let manual = hunt.screen().manual.expect("the declaration stands");
    assert_eq!((manual.equipment_id, manual.name.as_str()), (2, "Cannon"));
    hunt.shot(30.0);
    assert_eq!(hunt.pending(), phases(&[("Cannon", 1)]));

    // Something conclusive ends it.
    hunt.advance(1.0);
    hunt.see(ToolSeen::Nothing, 0.0);
    assert_eq!(hunt.screen().manual, None);
    assert_eq!(hunt.declared(), None);
}

#[test]
fn with_the_keys_the_source_a_declaration_goes_the_way_a_press_does() {
    let hunt = hunt(false);
    assert_eq!(
        hunt.declare(item(2, "Cannon", HotbarItemKind::Weapon)),
        Ok(ToolDeclared::KeysAreSource),
        "no session: the press's own path answers"
    );
    hunt.start();
    assert_eq!(
        hunt.declare(item(2, "Cannon", HotbarItemKind::Weapon)),
        Ok(ToolDeclared::KeysAreSource)
    );
    assert_eq!(hunt.declared(), None, "the tracker itself declares nothing");
}

#[test]
fn a_declaration_while_the_bar_is_covered_stands_and_the_bar_decides_on_its_return() {
    // Back with the tool declared by hand: its shots stand, and the
    // tool covered before is settled as another tool's return settles it.
    for (back, expected) in [
        (holding(2), phases(&[("Unknown", 1), ("Cannon", 1)])),
        (holding(3), phases(&[("Unknown", 1), ("Cannon", 1)])),
    ] {
        let hunt = hunt(true);
        hunt.start();
        hunt.see(holding(1), 0.0);
        hunt.advance(1.0);
        hunt.see(ToolSeen::NotVisible, 0.0);
        hunt.shot(9.0);
        assert_eq!(
            hunt.declare(item(2, "Cannon", HotbarItemKind::Weapon)),
            Ok(ToolDeclared::Taken)
        );
        hunt.shot(30.0);
        hunt.advance(1.0);
        hunt.see(back.clone(), 0.0);
        assert_eq!(hunt.pending(), expected, "back with {back:?}");
    }
    // Back with the tool covered: the pistol's shot stands.
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    hunt.shot(9.0);
    hunt.declare(item(2, "Cannon", HotbarItemKind::Weapon))
        .unwrap();
    hunt.shot(30.0);
    hunt.advance(1.0);
    hunt.see(holding(1), 0.0);
    assert_eq!(hunt.pending(), phases(&[("Pistol", 1), ("Cannon", 1)]));
    assert_eq!(hunt.declared().as_deref(), Some("Pistol"));
}

// 4. Nothing held, or not a tracked tool.

#[test]
fn nothing_held_and_a_name_not_tracked_declare_no_tool() {
    for seen in [
        ToolSeen::Nothing,
        ToolSeen::NotTracked {
            name: "Teleport Chip".into(),
        },
    ] {
        let hunt = hunt(true);
        hunt.start();
        hunt.see(holding(1), 0.0);
        hunt.shot(9.0);
        hunt.advance(2.0);
        hunt.see(seen.clone(), 0.0);
        assert_eq!(hunt.declared(), None);
        assert_eq!(hunt.held(), None, "nothing is shown in hand");
        assert_eq!(hunt.screen().last.unwrap().seen, seen);
        // Past the switch tail, 9 fits the pistol and the rifle alike:
        // with no tool in hand it is unpriced.
        hunt.advance(2.0);
        hunt.shot(9.0);
        assert_eq!(hunt.pending(), phases(&[("Pistol", 1), ("Unknown", 1)]));
    }
}

#[test]
fn the_tool_put_away_keeps_only_a_shot_it_alone_explains_inside_the_tail() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(2), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::Nothing, 0.0);
    hunt.advance(0.5);
    // 30 fits the cannon alone, half a second after it was put away.
    hunt.shot(30.0);
    hunt.advance(2.0);
    // 25 also fits the cannon alone, but well past the tail: the damage
    // evidence names it, as with nothing declared.
    hunt.shot(25.0);
    assert_eq!(hunt.pending(), phases(&[("Cannon", 2)]));
    let counts = hunt.counts();
    assert_eq!((counts.agreed, counts.evidenced), (1, 1));

    // The pistol put away: 9 inside its tail fits the rifle too.
    let hunt2 = self::hunt(true);
    hunt2.start();
    hunt2.see(holding(1), 0.0);
    hunt2.advance(1.0);
    hunt2.see(ToolSeen::Nothing, 0.0);
    hunt2.advance(0.3);
    hunt2.shot(9.0);
    assert_eq!(hunt2.pending(), phases(&[("Unknown", 1)]));
}

// 5. Uncertain.

#[test]
fn an_uncertain_bar_credits_no_shot_to_the_last_tool_and_keeps_why() {
    let hunt = hunt(true);
    let session = hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(2.0);
    hunt.see(
        ToolSeen::Uncertain(Why::Unknown {
            text: "Teleport  Chip".into(),
        }),
        0.0,
    );
    hunt.advance(2.0);
    hunt.shot(9.0);
    assert_eq!(hunt.pending(), phases(&[("Unknown", 1)]));
    assert_eq!(
        hunt.screen().last.unwrap().seen,
        ToolSeen::Uncertain(Why::Unknown {
            text: "Teleport  Chip".into()
        })
    );
    assert_eq!(
        hunt.rig.scalar_string(
            "SELECT text_key || '|' || text_read FROM tool_sightings \
             WHERE session_id = ? AND kind = 'unknown'",
            &[&session],
        ),
        "teleport chip|Teleport  Chip"
    );
}

#[test]
fn an_uncertain_bar_has_no_tail_for_the_last_tool() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::Uncertain(Why::Unreadable), 0.0);
    hunt.advance(0.3);
    // The pistol and the rifle both fit, three tenths after: unpriced.
    hunt.shot(9.0);
    // The pistol alone fits: its own damage names it.
    hunt.shot(7.0);
    assert_eq!(hunt.pending(), phases(&[("Pistol", 1), ("Unknown", 1)]));
    let counts = hunt.counts();
    assert_eq!(
        (counts.agreed, counts.evidenced, counts.unresolved),
        (0, 1, 1),
        "never an in-flight agreement"
    );
}

#[test]
fn names_matching_no_tool_are_kept_once_each_and_only_up_to_the_cap() {
    let hunt = hunt(true);
    let session = hunt.start();
    for _ in 0..2 {
        for index in 0..super::held_tool::UNKNOWN_NAMES_PER_SESSION + 4 {
            hunt.advance(1.0);
            hunt.see(
                ToolSeen::Uncertain(Why::Unknown {
                    text: format!("junk {index}"),
                }),
                0.0,
            );
            hunt.advance(1.0);
            hunt.see(ToolSeen::Nothing, 0.0);
        }
    }
    assert_eq!(
        hunt.rig.scalar_i64(
            "SELECT COUNT(*) FROM tool_sightings WHERE session_id = ?",
            &[&session],
        ),
        super::held_tool::UNKNOWN_NAMES_PER_SESSION as i64
    );
}

#[test]
fn with_the_screen_the_source_the_no_weapon_declared_warning_stands_down() {
    let warnings = |reading: bool| -> Vec<String> {
        let hunt = hunt(reading);
        hunt.start();
        if reading {
            hunt.see(ToolSeen::Nothing, 0.0);
        }
        // 3 fits no weapon: unpriced with nothing in hand either way.
        for _ in 0..crate::weapon_review::UNDECLARED_WARNING_SHOTS {
            hunt.advance(2.0);
            hunt.shot(3.0);
        }
        hunt.rig.probe(&hunt.tracker, |actor| {
            actor.session.active().unwrap().warnings.clone()
        })
    };
    assert_eq!(warnings(false).len(), 1, "with keys, the warning is raised");
    assert!(warnings(true).is_empty(), "the screen says nothing is held");
}

// 6. Not visible.

#[test]
fn shots_while_the_bar_is_covered_stand_when_the_same_tool_comes_back() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    assert!(hunt.screen().unseen.is_some(), "the tool is unseen");
    assert_eq!(hunt.declared().as_deref(), Some("Pistol"), "still declared");
    hunt.shot(9.0);
    hunt.shot(9.0);
    hunt.advance(1.0);
    hunt.see(holding(1), 0.0);
    assert_eq!(hunt.screen().unseen, None);
    assert_eq!(hunt.pending(), phases(&[("Pistol", 2)]));
    assert_eq!(hunt.unpriced_rows(), 0);
}

#[test]
fn shots_while_the_bar_is_covered_are_left_unpriced_when_another_tool_comes_back() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.shot(9.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    hunt.shot(9.0);
    hunt.shot(9.0);
    // Countered by the cannon's evidence: booked to it, not to the pistol.
    hunt.shot(30.0);
    hunt.advance(3.0);
    hunt.see(holding(2), 0.0);
    assert_eq!(
        hunt.pending(),
        phases(&[("Pistol", 1), ("Unknown", 2), ("Cannon", 1)])
    );
    assert_eq!(hunt.unpriced_rows(), 2, "each unpriced shot keeps its row");
    assert_eq!(hunt.counts().unresolved, 2);
    // Each could have been the rifle's, the dearer weapon both fit.
    assert!((hunt.bound() - 2.0 * rifle()).abs() < 1e-12);
}

#[test]
fn every_shot_of_a_long_cover_is_left_unpriced_when_another_tool_comes_back() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(10.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    for _ in 0..30 {
        hunt.advance(1.0);
        hunt.shot(9.0);
    }
    hunt.advance(1.0);
    hunt.see(holding(2), 0.0);
    assert_eq!(hunt.pending(), phases(&[("Unknown", 30)]));
    assert!((hunt.bound() - 30.0 * rifle()).abs() < 1e-9);
}

#[test]
fn a_covered_shot_settled_into_a_kill_moves_in_the_kill_and_its_stored_row() {
    let hunt = hunt(true);
    let session = hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    hunt.shot(9.0);
    hunt.loot();
    hunt.advance(1.0);
    hunt.see(ToolSeen::Nothing, 0.0);
    assert_eq!(
        hunt.rig.scalar_i64(
            "SELECT COUNT(*) FROM weapon_shot_evidence WHERE session_id = ? \
             AND attribution = 'unresolved' AND tool_name IS NULL AND kill_id IS NOT NULL",
            &[&session],
        ),
        1
    );
    assert_eq!(
        hunt.rig.scalar_string(
            "SELECT group_concat(tool_name || ':' || shots_fired) FROM kill_tool_stats",
            &[],
        ),
        "Unknown:1"
    );
    assert_eq!(
        hunt.rig.scalar_f64("SELECT cost_ped FROM kills", &[]),
        0.0,
        "the kill's cost follows its shot"
    );
}

#[test]
fn a_session_ending_while_the_bar_is_covered_leaves_those_shots_unpriced() {
    // What the app's exit does too: it stops the running session first.
    let hunt = hunt(true);
    let session = hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.shot(9.0);
    hunt.loot();
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    for _ in 0..30 {
        hunt.advance(1.0);
        hunt.shot(9.0);
    }
    hunt.advance(1.0);
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert_eq!(
        hunt.rig.scalar_f64(
            "SELECT dangling_cost FROM tracking_sessions WHERE id = ?",
            &[&session],
        ),
        0.0
    );
    assert_eq!(
        hunt.rig.scalar_i64(
            "SELECT COUNT(*) FROM weapon_shot_evidence WHERE session_id = ? \
             AND attribution = 'unresolved' AND kill_id IS NULL",
            &[&session],
        ),
        30
    );
    assert_eq!(
        hunt.rig.scalar_string(
            "SELECT group_concat(tool_name || ':' || shots_fired) FROM kill_tool_stats",
            &[],
        ),
        "Pistol:1",
        "the shot seen in hand stands"
    );
}

#[test]
fn a_settlement_that_cannot_be_saved_keeps_the_unseen_tool_and_its_shots() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    hunt.shot(9.0);
    hunt.loot();
    hunt.rig
        .wait(hunt.rig.db.with_writer(|conn| {
            conn.execute_batch("ALTER TABLE kill_tool_stats RENAME TO kill_tool_stats_gone")?;
            Ok(())
        }))
        .unwrap();
    hunt.advance(1.0);
    hunt.see(holding(2), 0.0);
    assert!(hunt.screen().unseen.is_some(), "the hold stands");
    let kill_phases = hunt.rig.probe(&hunt.tracker, |actor| {
        actor.session.active().unwrap().session.kills[0]
            .tool_stats
            .iter()
            .map(|(_, stats)| stats.tool_name.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(kill_phases, ["Pistol"], "memory is put back");
    // A stop that fails leaves the session live and the hold intact.
    assert!(hunt.rig.wait(hunt.tracker.stop_session()).is_err());
    assert!(hunt.screen().unseen.is_some());
    // Once it can be saved, the next stop settles it.
    hunt.rig
        .wait(hunt.rig.db.with_writer(|conn| {
            conn.execute_batch("ALTER TABLE kill_tool_stats_gone RENAME TO kill_tool_stats")?;
            Ok(())
        }))
        .unwrap();
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert_eq!(
        hunt.rig.scalar_string(
            "SELECT group_concat(tool_name || ':' || shots_fired) FROM kill_tool_stats",
            &[],
        ),
        "Unknown:1"
    );
}

// 7. A tool known only from the game's data.

#[test]
fn a_bundled_tool_is_declared_bare_kept_apart_and_its_sighting_kept() {
    let hunt = hunt(true);
    let session = hunt.start();
    hunt.see(rusty_seen(), 0.0);
    assert_eq!(hunt.declared().as_deref(), Some("Rusty Rifle"));
    let (provisional, carried): (Option<String>, Vec<String>) =
        hunt.rig.probe(&hunt.tracker, |actor| {
            let weapons = &actor.session.active().unwrap().weapons;
            (
                weapons.provisional_name().map(str::to_string),
                weapons
                    .attribution
                    .carried()
                    .iter()
                    .map(|weapon| weapon.name.clone())
                    .collect(),
            )
        });
    assert_eq!(provisional.as_deref(), Some("Rusty Rifle"));
    assert!(
        !carried.contains(&"Rusty Rifle".to_string()),
        "never a library weapon"
    );
    hunt.shot(150.0);
    assert_eq!(hunt.pending(), phases(&[("Rusty Rifle", 1)]));

    // Put away: tested for the switch tail, then not at all.
    hunt.advance(1.0);
    hunt.see(holding(2), 0.0);
    hunt.advance(3.0);
    hunt.shot(150.0);
    let (tested, props) = hunt.rig.probe(&hunt.tracker, |actor| {
        let weapons = &actor.session.active().unwrap().weapons;
        (
            weapons.attribution.provisional().is_some(),
            weapons.provisional_name().is_some(),
        )
    });
    assert!(!tested && !props, "its band and its props are gone");
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    let row = hunt.rig.scalar_string(
        "SELECT endpoint || '|' || item_id || '|' || item_name || '|' || shots || '|' || \
         printf('%.6f', cost_per_shot) || '|' || (last_seen > first_seen) \
         FROM tool_sightings WHERE session_id = ? AND kind = 'bundled'",
        &[&session],
    );
    assert_eq!(row, format!("weapons|900|Rusty Rifle|1|{:.6}|1", rusty()));
}

#[test]
fn a_bundled_finder_is_the_harvesting_tool_in_force_at_its_bare_cost() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(seen_bundled("harvesting_tools", "902", "Rusty Finder"), 0.0);
    let tool = hunt.rig.probe(&hunt.tracker, |actor| {
        actor
            .harvest_tool
            .as_ref()
            .map(|tool| (tool.name.clone(), tool.cost_per_use.value()))
    });
    assert_eq!(tool, Some(("Rusty Finder".into(), 0.04)));
}

#[test]
fn a_bundled_healers_heals_stay_visible_as_heals_no_paid_use_explains() {
    let hunt = hunt_with(true, true);
    hunt.start();
    hunt.see(seen_bundled("medical_tools", "901", "Old FAP"), 0.0);
    assert_eq!(
        hunt.held(),
        Some(("Old FAP".into(), HotbarItemKind::Healing))
    );
    hunt.advance(0.5);
    hunt.heal(80.0);
    assert!(
        activations(&hunt).is_empty(),
        "never billed, never a silent zero"
    );
    assert_eq!(heal_classes(&hunt), ["unattributed"]);
}

#[test]
fn turning_the_reading_off_releases_a_bundled_weapon_with_its_tail() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(rusty_seen(), 0.0);
    hunt.shot(150.0);
    *hunt.reading.lock().unwrap() = false;
    hunt.reload();
    assert_eq!(
        hunt.declared(),
        None,
        "no key can declare it, so it leaves the hand"
    );
    hunt.key("Pistol", HotbarItemKind::Weapon);
    hunt.advance(3.0);
    hunt.shot(150.0);
    hunt.shot(160.0);
    hunt.reload();
    hunt.shot(150.0);
    assert_eq!(
        hunt.pending(),
        phases(&[("Rusty Rifle", 1), ("Unknown", 3)]),
        "later shots never go to the bundled weapon"
    );
}

// 8. Carried weapons.

#[test]
fn the_carried_set_is_every_enabled_weapon_while_the_screen_is_the_source() {
    let names = |hunt: &Hunt| -> Vec<String> {
        hunt.rig.probe(&hunt.tracker, |actor| {
            actor
                .session
                .active()
                .unwrap()
                .weapons
                .attribution
                .carried()
                .iter()
                .map(|weapon| weapon.name.clone())
                .collect()
        })
    };
    let on = hunt(true);
    on.start();
    assert_eq!(names(&on), ["Pistol", "Cannon", "Rifle"]);
    let off = hunt(false);
    off.start();
    assert_eq!(names(&off), ["Pistol"]);
    // The switch moves the set with it.
    *off.reading.lock().unwrap() = true;
    off.reload();
    assert_eq!(names(&off), ["Pistol", "Cannon", "Rifle"]);
}

// 9. What the tracker knows for display.

#[test]
fn the_source_in_force_and_what_the_screen_showed_are_one_typed_value() {
    let hunt = hunt(false);
    hunt.start();
    assert_eq!(hunt.source(), ToolSource::Keys);
    *hunt.reading.lock().unwrap() = true;
    hunt.reload();
    assert_eq!(hunt.source(), ToolSource::Screen(ScreenTool::default()));
    hunt.see(holding(1), 0.0);
    let screen = hunt.screen();
    let last = screen.last.unwrap();
    assert_eq!(last.seen, holding(1));
    assert_eq!(last.at_ms, clock_epoch_ms(hunt.rig.clock.as_ref()));
    assert_eq!(screen.unseen, None);
    assert_eq!(screen.manual, None);
}

// 10. Life.

#[test]
fn the_watch_runs_with_the_session_and_the_setting() {
    let hunt = hunt(true);
    let first = hunt.start();
    assert_eq!(
        *hunt.watcher.starts.lock().unwrap(),
        std::slice::from_ref(&first)
    );
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert_eq!(*hunt.watcher.stops.lock().unwrap(), 1);
    assert_eq!(hunt.source(), ToolSource::Keys);

    // Turning off mid-session returns to keys, the tool last in hand still
    // declared; turning on again permits capture and starts a fresh watch.
    let second = hunt.start();
    hunt.see(holding(1), 0.0);
    *hunt.reading.lock().unwrap() = false;
    hunt.reload();
    assert_eq!(*hunt.watcher.stops.lock().unwrap(), 2);
    assert_eq!(hunt.source(), ToolSource::Keys);
    assert_eq!(hunt.declared().as_deref(), Some("Pistol"));
    hunt.key("Cannon", HotbarItemKind::Weapon);
    assert_eq!(
        hunt.declared().as_deref(),
        Some("Cannon"),
        "keys declare again"
    );
    *hunt.reading.lock().unwrap() = true;
    hunt.reload();
    assert_eq!(*hunt.watcher.permits.lock().unwrap(), 1);
    assert_eq!(
        *hunt.watcher.starts.lock().unwrap(),
        [first, second.clone(), second]
    );
}

#[test]
fn a_look_for_another_session_or_after_the_session_is_dropped() {
    let hunt = hunt(true);
    hunt.start();
    let (session_id, sink) = hunt.sink();
    hunt.tell("another".into(), sink.clone(), holding(2), 0.0);
    assert_eq!(hunt.declared(), None);
    assert_eq!(hunt.screen().last, None);
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    hunt.tell(session_id, sink, holding(2), 0.0);
    assert_eq!(hunt.source(), ToolSource::Keys);
}

#[test]
fn a_tool_held_before_the_session_is_not_carried_in_and_the_first_look_declares() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(2), 0.0);
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    hunt.start();
    assert_eq!(hunt.declared(), None);
    assert_eq!(hunt.held(), None);
    hunt.see(holding(2), 0.0);
    assert_eq!(hunt.declared().as_deref(), Some("Cannon"));
}

#[test]
fn a_watch_ending_by_itself_hands_the_source_back_to_the_keys() {
    let hunt = hunt(true);
    let session = hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    hunt.shot(9.0);
    hunt.advance(1.0);
    let ended = hunt.watcher.ended.lock().unwrap().clone().unwrap();
    hunt.watcher.stop();
    ended(&session);
    // The covered shot is settled first.
    assert_eq!(hunt.pending(), phases(&[("Unknown", 1)]));
    let ended_at = clock_epoch_ms(hunt.rig.clock.as_ref());
    assert_eq!(hunt.source(), ToolSource::ScreenStopped { at_ms: ended_at });
    assert_eq!(hunt.declared().as_deref(), Some("Pistol"), "still in hand");
    // Keys work again, and shots booked live keep their bookings at stop.
    hunt.key("Cannon", HotbarItemKind::Weapon);
    assert_eq!(hunt.declared().as_deref(), Some("Cannon"));
    for _ in 0..10 {
        hunt.advance(1.0);
        hunt.shot(30.0);
    }
    assert_eq!(hunt.retained(), 0, "nothing is kept for a screen not read");
    // A reload with the setting still on does not start the screen again.
    hunt.reload();
    assert_eq!(hunt.source(), ToolSource::ScreenStopped { at_ms: ended_at });
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert_eq!(
        hunt.rig
            .scalar_i64("SELECT COUNT(*) FROM kill_tool_stats", &[]),
        0,
        "no kill settled"
    );
    assert_eq!(
        hunt.rig.scalar_i64(
            "SELECT COUNT(*) FROM weapon_shot_evidence WHERE session_id = ? \
             AND attribution = 'unresolved'",
            &[&session],
        ),
        1,
        "only the covered shot is unpriced"
    );
    // A new session tries the screen again.
    hunt.start();
    assert!(matches!(hunt.source(), ToolSource::Screen(_)));
}

#[test]
fn turning_the_setting_off_and_on_tries_the_screen_again_after_its_watch_ended() {
    let hunt = hunt(true);
    let session = hunt.start();
    let ended = hunt.watcher.ended.lock().unwrap().clone().unwrap();
    hunt.watcher.stop();
    ended(&session);
    assert!(matches!(hunt.source(), ToolSource::ScreenStopped { .. }));
    *hunt.reading.lock().unwrap() = false;
    hunt.reload();
    assert_eq!(hunt.source(), ToolSource::Keys);
    *hunt.reading.lock().unwrap() = true;
    hunt.reload();
    assert!(matches!(hunt.source(), ToolSource::Screen(_)));
}

#[test]
fn a_look_of_a_watch_already_replaced_is_dropped() {
    let hunt = hunt(true);
    hunt.start();
    let (session_id, stale) = hunt.sink();
    *hunt.reading.lock().unwrap() = false;
    hunt.reload();
    *hunt.reading.lock().unwrap() = true;
    hunt.reload();
    hunt.tell(session_id, stale, holding(2), 0.0);
    assert_eq!(hunt.screen().last, None);
    assert_eq!(hunt.declared(), None);
}

#[test]
fn turning_the_setting_off_and_on_keeps_one_sighting_per_tool() {
    let hunt = hunt(true);
    let session = hunt.start();
    hunt.see(rusty_seen(), 0.0);
    hunt.shot(150.0);
    hunt.advance(1.0);
    *hunt.reading.lock().unwrap() = false;
    hunt.reload();
    // The reading's end keeps the shots so far.
    assert_eq!(
        hunt.rig.scalar_i64(
            "SELECT shots FROM tool_sightings WHERE session_id = ?",
            &[&session],
        ),
        1
    );
    *hunt.reading.lock().unwrap() = true;
    hunt.reload();
    hunt.advance(1.0);
    hunt.see(rusty_seen(), 0.0);
    hunt.shot(150.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::Nothing, 0.0);
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert_eq!(
        hunt.rig.scalar_string(
            "SELECT COUNT(*) || '|' || MAX(shots) || '|' || \
             (MAX(last_seen) - MIN(first_seen)) FROM tool_sightings WHERE session_id = ?",
            &[&session],
        ),
        "1|2|3.0",
        "one row, both stretches' shots, first seen in the first stretch"
    );
}

// 9 (keyless). The declared healer.

#[test]
fn a_keyless_session_reading_the_screen_infers_no_healer_it_does_not_show() {
    let hunt = hunt_with(false, true);
    hunt.start();
    hunt.intent(IntentSource::Overlay, "FAP", HotbarItemKind::Healing);
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    // A session starting on the screen does not carry the healer in.
    *hunt.reading.lock().unwrap() = true;
    hunt.start();
    hunt.advance(5.0);
    hunt.heal(80.0);
    assert_eq!(activations(&hunt).len(), 0);
    // Shown in hand, its uses are inferred; once the screen shows a
    // weapon, they are not.
    hunt.see(holding(4), 0.0);
    hunt.advance(0.5);
    hunt.heal(80.0);
    assert_eq!(activations(&hunt).len(), 1);
    hunt.see(holding(1), 0.0);
    hunt.advance(5.0);
    hunt.heal(80.0);
    assert_eq!(
        activations(&hunt).len(),
        1,
        "the weapon in hand heals nothing"
    );
}

// 11. Order of arrival.

/// What a script places, its look told before or after the shots it
/// predates: everything [`Placed`] compares.
fn placed_either_way(script: impl Fn(&Hunt, bool)) -> (Placed, Placed) {
    let in_order = hunt(true);
    in_order.start();
    script(&in_order, true);
    let late = hunt(true);
    late.start();
    script(&late, false);
    (in_order.placed(), late.placed())
}

#[test]
fn a_change_arriving_after_shots_it_predates_places_them_as_if_it_came_first() {
    // The pistol in hand; three seconds later the cannon is seen, and a
    // shot two seconds after that fits the pistol and the rifle only.
    let script = |hunt: &Hunt, look_first: bool| {
        hunt.see(holding(1), 0.0);
        hunt.advance(3.0);
        if look_first {
            hunt.see(holding(2), 0.0);
            hunt.advance(2.0);
            hunt.shot(9.0);
            hunt.shot(30.0);
        } else {
            hunt.advance(2.0);
            hunt.shot(9.0);
            hunt.shot(30.0);
            hunt.see(holding(2), 2.0);
        }
    };
    let (in_order, late) = placed_either_way(script);
    assert_eq!(late, in_order);
    let mut pending: Vec<(String, i64)> = in_order
        .pending
        .iter()
        .map(|(tool, shots, _)| (tool.clone(), *shots))
        .collect();
    pending.sort();
    assert_eq!(pending, phases(&[("Unknown", 1), ("Cannon", 1)]));
    assert_eq!(in_order.counts.evidenced, 0, "a plain cannon agreement");
}

#[test]
fn a_late_change_places_shots_settled_into_a_kill_as_if_it_came_first() {
    let script = |hunt: &Hunt, look_first: bool| {
        hunt.see(holding(1), 0.0);
        hunt.advance(3.0);
        if look_first {
            hunt.see(holding(2), 0.0);
        }
        hunt.advance(2.0);
        hunt.shot(9.0);
        hunt.shot(30.0);
        hunt.shot(31.0);
        hunt.loot();
        hunt.shot(9.0);
        if !look_first {
            hunt.see(holding(2), 2.0);
        }
    };
    let (in_order, late) = placed_either_way(script);
    assert_eq!(late, in_order);
}

#[test]
fn an_unclear_bar_arriving_after_shots_it_predates_unprices_them() {
    let script = |hunt: &Hunt, look_first: bool| {
        hunt.see(holding(1), 0.0);
        hunt.advance(3.0);
        let unclear = ToolSeen::Uncertain(Why::Unreadable);
        if look_first {
            hunt.see(unclear, 0.0);
            hunt.advance(2.0);
            hunt.shot(9.0);
            hunt.shot(7.0);
        } else {
            hunt.advance(2.0);
            hunt.shot(9.0);
            hunt.shot(7.0);
            hunt.see(unclear, 2.0);
        }
    };
    let (in_order, late) = placed_either_way(script);
    assert_eq!(late, in_order);
    assert_eq!(in_order.counts.unresolved, 1);
}

#[test]
fn a_late_tool_takes_a_shot_back_out_of_unpriced_and_off_the_bound() {
    let script = |hunt: &Hunt, look_first: bool| {
        hunt.see(ToolSeen::Nothing, 0.0);
        hunt.advance(9.5);
        if look_first {
            hunt.see(holding(1), 0.0);
            hunt.advance(0.5);
            hunt.shot(9.0);
            hunt.advance(2.0);
        } else {
            hunt.advance(0.5);
            hunt.shot(9.0);
            hunt.advance(2.0);
            hunt.see(holding(1), 2.5);
        }
    };
    let (in_order, late) = placed_either_way(script);
    assert_eq!(late, in_order);
    assert_eq!(
        in_order.bound,
        (0.0, 0),
        "the shot is priced: nothing is owed"
    );
    assert!(in_order.rows.is_empty(), "no unpriced row stands for it");
}

#[test]
fn a_shot_arriving_after_a_change_it_predates_is_the_switch_tails() {
    // The shot was fired with the pistol just before the change; it is read
    // half a second after the cannon was seen.
    let script = |hunt: &Hunt, look_first: bool| {
        hunt.see(holding(1), 0.0);
        hunt.advance(3.0);
        if look_first {
            hunt.see(holding(2), 0.0);
            hunt.advance(0.5);
            hunt.shot(7.0);
        } else {
            hunt.advance(0.5);
            hunt.shot(7.0);
            hunt.see(holding(2), 0.5);
        }
    };
    let (in_order, late) = placed_either_way(script);
    assert_eq!(late, in_order);
    assert_eq!(in_order.pending[0].0, "Pistol");
}

#[test]
fn a_covered_bar_arriving_late_holds_the_shots_it_predates() {
    let script = |hunt: &Hunt, look_first: bool| {
        hunt.see(holding(1), 0.0);
        hunt.advance(1.0);
        if look_first {
            hunt.see(ToolSeen::NotVisible, 0.0);
            hunt.advance(1.0);
            hunt.shot(9.0);
        } else {
            hunt.advance(1.0);
            hunt.shot(9.0);
            hunt.see(ToolSeen::NotVisible, 1.0);
        }
        hunt.advance(1.0);
        hunt.see(holding(2), 0.0);
    };
    let (in_order, late) = placed_either_way(script);
    assert_eq!(late, in_order);
    assert_eq!(in_order.counts.unresolved, 1);
}

// 13. Order of looks.

#[test]
fn a_look_older_than_one_already_applied_is_dropped() {
    let hunt = hunt(true);
    hunt.start();
    hunt.advance(10.0);
    hunt.see(holding(2), 0.0);
    hunt.see(holding(1), 1.0);
    assert_eq!(hunt.declared().as_deref(), Some("Cannon"));
    assert_eq!(hunt.screen().last.unwrap().seen, holding(2));
}

#[test]
fn a_look_older_than_the_shots_kept_is_taken_as_of_the_oldest_one_kept() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    for _ in 0..20 {
        hunt.advance(1.0);
        hunt.shot(9.0);
    }
    // Seen at the second shot, told after the twentieth: only the last five
    // seconds of shots are still kept, so the change is taken as of the
    // oldest of them. Older shots keep their bookings; a moment the screen
    // did not show starts no switch tail, so none placed again stays the
    // pistol's by timing.
    hunt.see(holding(2), 18.0);
    let floor = hunt.now() - 5.0;
    assert_eq!(
        hunt.screen().last.unwrap().at_ms,
        (floor * 1000.0).round() as i64
    );
    assert_eq!(hunt.pending(), phases(&[("Pistol", 14), ("Unknown", 6)]));
}

// With the reading off.

#[test]
fn with_the_reading_off_a_look_is_dropped_and_nothing_changes() {
    let hunt = hunt(false);
    let session = hunt.start();
    assert!(
        hunt.watcher.starts.lock().unwrap().is_empty(),
        "no watch starts"
    );
    hunt.key("Pistol", HotbarItemKind::Weapon);
    hunt.shot(9.0);
    let before = (hunt.pending(), hunt.declared(), hunt.held());
    hunt.tracker
        .sender
        .send(TrackerMsg::ToolPulse(
            ToolPulse {
                session_id: session,
                at_ms: clock_epoch_ms(hunt.rig.clock.as_ref()),
                seen: holding(2),
            },
            0,
        ))
        .unwrap();
    assert_eq!(hunt.source(), ToolSource::Keys);
    assert_eq!((hunt.pending(), hunt.declared(), hunt.held()), before);
    assert_eq!(*hunt.watcher.hints.lock().unwrap(), 0, "a key asks nothing");
    assert_eq!(hunt.retained(), 0, "nothing is kept");
}

// Second round: placing again, effects, decisions, failed writes, cost.

#[test]
fn a_healer_look_places_nothing_again_and_the_log_holds_each_shot_once() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(2.0);
    // Fits the pistol and the rifle: the pistol's.
    hunt.shot(9.0);
    // A first-aid pack seen a second before the shot, told after it.
    hunt.see(holding(4), 1.0);
    let seqs: Vec<u64> = hunt.rig.probe(&hunt.tracker, |actor| {
        let runtime = &actor.session.active().unwrap().weapons.attribution;
        runtime.log().iter().map(|record| record.seq).collect()
    });
    let mut unique = seqs.clone();
    unique.dedup();
    assert_eq!(seqs, unique, "each shot once in the regime log");
    assert_eq!(seqs.len(), 1);
    // Only the rifle fits: a mismatch, confirmed.
    hunt.shot(12.0);
    assert!(hunt
        .rig
        .wait(
            hunt.tracker
                .decide_weapon_mismatch(super::MismatchDecision::Confirm)
        )
        .unwrap());
    assert_eq!(hunt.pending(), phases(&[("Rifle", 2)]));
    assert_eq!(hunt.counts().agreed + hunt.counts().evidenced, 2);
}

#[test]
fn an_uncertain_bar_ends_a_bundled_tool_at_once_with_no_tail() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(rusty_seen(), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::Uncertain(Why::Unreadable), 0.0);
    hunt.advance(0.5);
    // Only the bundled rifle could explain 150.
    hunt.shot(150.0);
    assert_eq!(hunt.pending(), phases(&[("Unknown", 1)]));
}

#[test]
fn a_late_change_to_a_weapon_with_an_effect_places_its_hit_and_ticks_as_on_arrival() {
    let script = |hunt: &Hunt, look_first: bool| {
        hunt.see(holding(1), 0.0);
        hunt.advance(9.5);
        if look_first {
            hunt.see(holding(7), 0.0);
        }
        hunt.advance(0.5);
        hunt.shot(130.0);
        hunt.advance(0.5);
        hunt.shot(60.0);
        hunt.advance(0.5);
        hunt.shot(60.0);
        if !look_first {
            hunt.see(holding(7), 1.5);
        }
    };
    let (in_order, late) = placed_effect_either_way(script);
    assert_eq!(late, in_order);
    assert_eq!(in_order.shots, (1, 0), "one paid hit, two free ticks");
    assert_eq!(in_order.counts.effect_ticks, 2);
    assert_eq!(in_order.windows.len(), 1);
    assert_eq!(in_order.stored_windows.len(), 1);
}

#[test]
fn a_late_change_away_from_a_weapon_with_an_effect_places_its_ticks_as_on_arrival() {
    let script = |hunt: &Hunt, look_first: bool| {
        hunt.see(holding(7), 0.0);
        hunt.advance(1.0);
        hunt.shot(130.0);
        hunt.advance(4.0);
        if look_first {
            hunt.see(holding(2), 0.0);
        }
        hunt.advance(1.0);
        hunt.shot(60.0);
        hunt.shot(30.0);
        hunt.advance(0.5);
        hunt.shot(130.0);
        if !look_first {
            hunt.see(holding(2), 1.5);
        }
    };
    let (in_order, late) = placed_effect_either_way(script);
    assert_eq!(late, in_order);
}

#[test]
fn a_late_unclear_bar_over_an_effect_places_its_ticks_and_hits_as_on_arrival() {
    let script = |hunt: &Hunt, look_first: bool| {
        hunt.see(holding(7), 0.0);
        hunt.advance(1.0);
        hunt.shot(130.0);
        hunt.advance(2.0);
        let unclear = ToolSeen::Uncertain(Why::Unreadable);
        if look_first {
            hunt.see(unclear.clone(), 0.0);
        }
        hunt.advance(1.0);
        hunt.shot(60.0);
        hunt.advance(0.5);
        hunt.shot(130.0);
        if !look_first {
            hunt.see(unclear, 1.5);
        }
    };
    let (in_order, late) = placed_effect_either_way(script);
    assert_eq!(late, in_order);
    assert_eq!(
        in_order.windows.len(),
        2,
        "the hit after opens its own window"
    );
}

/// [`placed_either_way`] over a library holding the weapon with an effect.
fn placed_effect_either_way(script: impl Fn(&Hunt, bool)) -> (Placed, Placed) {
    let mut weapons = library_weapons();
    weapons.push(zapper());
    let in_order = hunt_over(true, false, weapons.clone());
    in_order.start();
    script(&in_order, true);
    let late = hunt_over(true, false, weapons);
    late.start();
    script(&late, false);
    (in_order.placed(), late.placed())
}

#[test]
fn a_late_look_leaves_what_the_player_decided() {
    let hunt = hunt(true);
    let session = hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(2.0);
    // Short of the pistol's band: the pistol's, as a reduced hit.
    hunt.shot(3.0);
    // Only the cannon fits: a mismatch, confirmed, which moves the short
    // hit to the cannon.
    hunt.shot(30.0);
    assert!(hunt
        .rig
        .wait(
            hunt.tracker
                .decide_weapon_mismatch(super::MismatchDecision::Confirm)
        )
        .unwrap());
    assert_eq!(hunt.pending(), phases(&[("Cannon", 2)]));
    let review = || {
        hunt.rig.scalar_string(
            "SELECT repriced_shots || '|' || printf('%.6f', cost_delta_ped) \
             FROM weapon_attribution_reviews WHERE session_id = ?",
            &[&session],
        )
    };
    let before = review();
    // The rifle seen before both shots, told after the decision.
    hunt.see(holding(3), 1.0);
    assert_eq!(
        hunt.pending(),
        phases(&[("Cannon", 2)]),
        "the decision stands"
    );
    assert_eq!(review(), before);
    let decided_row: (Option<String>, String) = hunt.rig.probe(&hunt.tracker, |actor| {
        let row = actor
            .session
            .active()
            .unwrap()
            .accumulator
            .evidence
            .iter()
            .find(|row| row.amount == Some(3.0))
            .unwrap()
            .clone();
        (row.review_id, row.tool_name.unwrap_or_default())
    });
    assert!(decided_row.0.is_some());
    assert_eq!(decided_row.1, "Cannon");
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert_eq!(
        hunt.rig.scalar_i64(
            "SELECT COUNT(*) FROM weapon_shot_evidence WHERE session_id = ? \
             AND review_id IS NOT NULL",
            &[&session],
        ),
        1
    );
}

fn break_kill_phases(hunt: &Hunt) {
    hunt.rig
        .wait(hunt.rig.db.with_writer(|conn| {
            conn.execute_batch("ALTER TABLE kill_tool_stats RENAME TO kill_tool_stats_gone")?;
            Ok(())
        }))
        .unwrap();
}

fn mend_kill_phases(hunt: &Hunt) {
    hunt.rig
        .wait(hunt.rig.db.with_writer(|conn| {
            conn.execute_batch("ALTER TABLE kill_tool_stats_gone RENAME TO kill_tool_stats")?;
            Ok(())
        }))
        .unwrap();
}

#[test]
fn a_failed_settlement_at_switch_off_hands_the_source_to_the_keys_and_the_stop_settles_it() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    hunt.shot(9.0);
    hunt.loot();
    break_kill_phases(&hunt);
    hunt.advance(1.0);
    *hunt.reading.lock().unwrap() = false;
    hunt.reload();
    assert_eq!(hunt.source(), ToolSource::Keys);
    assert_eq!(
        *hunt.watcher.stops.lock().unwrap(),
        1,
        "the watcher is stopped"
    );
    hunt.key("Cannon", HotbarItemKind::Weapon);
    assert_eq!(hunt.declared().as_deref(), Some("Cannon"), "keys declare");
    hunt.shot(3.0);
    mend_kill_phases(&hunt);
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert_eq!(
        hunt.rig.scalar_string(
            "SELECT group_concat(tool_name || ':' || shots_fired) FROM kill_tool_stats",
            &[],
        ),
        "Unknown:1",
        "the covered shot is settled at the stop"
    );
}

#[test]
fn a_failed_settlement_when_the_watch_ends_hands_the_source_to_the_keys() {
    let hunt = hunt(true);
    let session = hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    hunt.shot(9.0);
    hunt.loot();
    break_kill_phases(&hunt);
    hunt.advance(1.0);
    let ended = hunt.watcher.ended.lock().unwrap().clone().unwrap();
    hunt.watcher.stop();
    ended(&session);
    assert!(matches!(hunt.source(), ToolSource::ScreenStopped { .. }));
    hunt.key("Cannon", HotbarItemKind::Weapon);
    assert_eq!(hunt.declared().as_deref(), Some("Cannon"));
    mend_kill_phases(&hunt);
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert_eq!(
        hunt.rig.scalar_string(
            "SELECT group_concat(tool_name || ':' || shots_fired) FROM kill_tool_stats",
            &[],
        ),
        "Unknown:1"
    );
}

#[test]
fn a_placement_that_cannot_be_saved_takes_the_look_back_whole() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(2.0);
    hunt.shot(9.0);
    hunt.loot();
    break_kill_phases(&hunt);
    // The cannon seen before the shot, told after it settled into a kill.
    hunt.see(holding(2), 1.0);
    assert_eq!(
        hunt.declared().as_deref(),
        Some("Pistol"),
        "the hand is put back"
    );
    assert_eq!(hunt.held(), Some(("Pistol".into(), HotbarItemKind::Weapon)));
    let screen = hunt.screen();
    assert_eq!(
        screen.last.unwrap().seen,
        holding(1),
        "as if the look never came"
    );
    let kill = hunt.rig.probe(&hunt.tracker, |actor| {
        actor.session.active().unwrap().session.kills[0]
            .tool_stats
            .iter()
            .map(|(_, stats)| stats.tool_name.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(kill, ["Pistol"]);
    mend_kill_phases(&hunt);
    // The next look decides.
    hunt.see(holding(2), 0.0);
    assert_eq!(hunt.declared().as_deref(), Some("Cannon"));
}

#[test]
fn settling_ten_thousand_covered_shots_takes_steps_in_proportion() {
    const SHOTS: u64 = 10_000;
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    for _ in 0..SHOTS {
        hunt.shot(9.0);
    }
    let steps = |hunt: &Hunt| {
        hunt.rig.probe(&hunt.tracker, |actor| {
            actor
                .session
                .active()
                .unwrap()
                .weapons
                .attribution
                .retention_steps()
        })
    };
    let before = steps(&hunt);
    let all_before = work(&hunt);
    hunt.advance(1.0);
    hunt.see(holding(2), 0.0);
    let taken = steps(&hunt) - before;
    let all = work(&hunt) - all_before;
    assert_eq!(hunt.pending(), phases(&[("Unknown", SHOTS as i64)]));
    // Each shot settled is found by its number a fixed few times.
    assert!(taken <= 4 * SHOTS, "{taken} steps for {SHOTS} shots");
    // The regime log is found by halving and saved once; the rows are
    // indexed once: about log2 of the shots per shot over everything.
    assert!(
        all <= 24 * SHOTS,
        "{all} steps over everything for {SHOTS} shots"
    );
}

#[test]
fn a_gone_tracker_lets_a_declaration_go_the_way_a_press_does() {
    let rig = rig();
    let (sender, inbox) = tokio::sync::mpsc::unbounded_channel();
    drop(inbox);
    let (_status_tx, status) = tokio::sync::watch::channel(super::actor::TrackerStatus::default());
    let gone = HuntTracker {
        db: rig.db.clone(),
        sender,
        status,
        doses: crate::consumables::DoseBoard::new(rig.clock.clone()),
    };
    assert_eq!(
        rig.wait(gone.declare_tool(item(2, "Cannon", HotbarItemKind::Weapon), 1.0)),
        Ok(ToolDeclared::KeysAreSource)
    );
}

#[test]
fn a_watch_that_does_not_start_leaves_the_keys_the_source() {
    let hunt = hunt(true);
    *hunt.watcher.refuse.lock().unwrap() = true;
    hunt.start();
    assert_eq!(hunt.source(), ToolSource::ScreenNotStarted);
    assert_eq!(hunt.retained(), 0);
    hunt.key("Cannon", HotbarItemKind::Weapon);
    assert_eq!(
        hunt.declared().as_deref(),
        Some("Cannon"),
        "the key declares"
    );
    assert_eq!(*hunt.watcher.hints.lock().unwrap(), 0);
    // Every settings change tries again; while capture is still not
    // running, nothing changes.
    hunt.reload();
    assert_eq!(hunt.source(), ToolSource::ScreenNotStarted);
    // Turned off and on with capture now running: the screen takes over.
    *hunt.watcher.refuse.lock().unwrap() = false;
    *hunt.reading.lock().unwrap() = false;
    hunt.reload();
    assert_eq!(hunt.source(), ToolSource::Keys);
    *hunt.reading.lock().unwrap() = true;
    hunt.reload();
    assert!(matches!(hunt.source(), ToolSource::Screen(_)));
    hunt.see(holding(1), 0.0);
    assert_eq!(hunt.declared().as_deref(), Some("Pistol"));
}

#[test]
fn a_new_session_tries_a_watch_that_did_not_start_again() {
    let hunt = hunt(true);
    *hunt.watcher.refuse.lock().unwrap() = true;
    hunt.start();
    assert_eq!(hunt.source(), ToolSource::ScreenNotStarted);
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    *hunt.watcher.refuse.lock().unwrap() = false;
    hunt.start();
    assert!(matches!(hunt.source(), ToolSource::Screen(_)));
}

// Third round.

#[test]
fn an_owed_stretch_survives_the_reading_turned_on_again_a_cover_and_the_bar_back() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(3), 0.0);
    hunt.advance(100.0);
    // Covered from T100 with the rifle in hand, a shot a second to T200.
    hunt.see(ToolSeen::NotVisible, 0.0);
    for _ in 0..100 {
        hunt.advance(1.0);
        hunt.shot(12.0);
    }
    // A kill settles the stretch's shots; turned off at T200, the
    // settlement cannot be saved.
    hunt.loot();
    break_kill_phases(&hunt);
    *hunt.reading.lock().unwrap() = false;
    hunt.reload();
    mend_kill_phases(&hunt);
    // On again at T300, covered at T320, the bar back at T330.
    hunt.advance(100.0);
    *hunt.reading.lock().unwrap() = true;
    hunt.reload();
    hunt.see(holding(3), 0.0);
    hunt.advance(20.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    hunt.advance(10.0);
    hunt.see(holding(3), 0.0);
    hunt.advance(1.0);
    hunt.rig.wait(hunt.tracker.stop_session()).unwrap();
    assert_eq!(
        hunt.rig.scalar_string(
            "SELECT group_concat(tool_name || ':' || shots_fired) FROM kill_tool_stats",
            &[],
        ),
        "Unknown:100",
        "every shot of the owed stretch is unpriced at the stop"
    );
}

/// Shots that could not be placed again go back where a decision can
/// still reach them.
#[test]
fn a_shot_that_cannot_be_placed_again_goes_back_into_the_regime_log() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(2.0);
    hunt.shot(9.0);
    // Its phase is gone from under it: it cannot be taken back out.
    let seq = hunt.rig.probe(&hunt.tracker, |actor| {
        let active = actor.session.active_mut().unwrap();
        active.accumulator.tool_stats.clear();
        active.weapons.attribution.log()[0].seq
    });
    hunt.see(holding(2), 1.0);
    let logged: Vec<u64> = hunt.rig.probe(&hunt.tracker, |actor| {
        let runtime = &actor.session.active().unwrap().weapons.attribution;
        runtime.log().iter().map(|record| record.seq).collect()
    });
    assert_eq!(logged, [seq]);
    assert_eq!(hunt.retained(), 1);
}

#[test]
fn a_tick_left_as_it_was_loses_the_window_its_opener_gave_up() {
    let mut weapons = library_weapons();
    weapons.push(zapper());
    let hunt = hunt_over(true, false, weapons);
    hunt.start();
    hunt.see(holding(7), 0.0);
    hunt.advance(2.0);
    hunt.shot(130.0);
    hunt.advance(0.5);
    hunt.shot(60.0);
    // The tick is made to sit in a kill that is gone: it cannot be placed
    // again, while its opener is.
    let tick = hunt.rig.probe(&hunt.tracker, |actor| {
        let runtime = &mut actor.session.active_mut().unwrap().weapons.attribution;
        let tick = runtime
            .retained_since(f64::NEG_INFINITY)
            .into_iter()
            .find(|shot| {
                matches!(
                    shot.record.attribution,
                    super::attribution::Attribution::EffectTick { .. }
                )
            })
            .unwrap()
            .record
            .seq;
        runtime.retained_mut(tick).unwrap().record.location = ShotLocation::Kill("gone".into());
        tick
    });
    hunt.see(holding(2), 1.0);
    let (attribution, windows, row_window) = hunt.rig.probe(&hunt.tracker, move |actor| {
        let active = actor.session.active().unwrap();
        let shot = active
            .weapons
            .attribution
            .retained_since(f64::NEG_INFINITY)
            .into_iter()
            .find(|shot| shot.record.seq == tick)
            .unwrap();
        let row = active
            .accumulator
            .evidence
            .iter()
            .find(|row| Some(&row.id) == shot.record.evidence_id.as_ref())
            .unwrap()
            .effect_window_id
            .clone();
        (shot.record.attribution, shot.record.windows, row)
    });
    assert_eq!(
        attribution,
        super::attribution::Attribution::EffectTick { window_id: None }
    );
    assert!(windows.is_empty());
    assert_eq!(row_window, None);
}

#[test]
fn a_decision_that_cannot_be_saved_rolls_back_its_marks_with_the_screen_the_source() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(2.0);
    hunt.shot(3.0);
    hunt.shot(30.0);
    let before = hunt.placed();
    hunt.rig
        .wait(hunt.rig.db.with_writer(|conn| {
            conn.execute_batch(
                "ALTER TABLE weapon_attribution_reviews RENAME TO weapon_attribution_reviews_gone",
            )?;
            Ok(())
        }))
        .unwrap();
    assert_eq!(
        hunt.rig.wait(
            hunt.tracker
                .decide_weapon_mismatch(super::MismatchDecision::Confirm)
        ),
        Err(super::WeaponDecisionError::NotSaved)
    );
    assert_eq!(hunt.placed(), before, "no shot is marked decided");
    hunt.rig
        .wait(hunt.rig.db.with_writer(|conn| {
            conn.execute_batch(
                "ALTER TABLE weapon_attribution_reviews_gone RENAME TO weapon_attribution_reviews",
            )?;
            Ok(())
        }))
        .unwrap();
    assert_eq!(
        hunt.rig.wait(
            hunt.tracker
                .decide_weapon_mismatch(super::MismatchDecision::Confirm)
        ),
        Ok(true)
    );
    assert!(hunt
        .placed()
        .retained
        .iter()
        .any(|shot| shot.ends_with("decided:true")));
}

/// Every step taken over the kept shots, the regime log, the
/// accumulator's rows and the kills.
fn work(hunt: &Hunt) -> u64 {
    hunt.rig.probe(&hunt.tracker, |actor| {
        let active = actor.session.active().unwrap();
        let runtime = &active.weapons.attribution;
        runtime.retention_steps() + runtime.log_steps() + active.steps
    })
}

#[test]
fn settling_covered_shots_with_rows_waiting_and_stored_takes_steps_in_proportion() {
    const SHOTS: u64 = 4_000;
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    // Short of the pistol's band: each keeps a row; every hundred settle
    // into a kill, so rows wait both stored and with the accumulator.
    for index in 0..SHOTS {
        hunt.shot(3.0);
        if index % 100 == 99 {
            hunt.loot();
        }
    }
    hunt.shot(3.0);
    let before = work(&hunt);
    hunt.advance(1.0);
    hunt.see(holding(2), 0.0);
    let taken = work(&hunt) - before;
    // Each move finds its shot in the regime log by halving (about twelve
    // comparisons for four thousand) and its row by id: n log n, where a
    // scan per move would take thousands of steps a shot.
    assert!(
        taken <= 24 * SHOTS,
        "{taken} steps settling {SHOTS} shots with their rows"
    );
}

#[test]
fn placing_a_long_pinned_stretch_again_takes_steps_in_proportion() {
    const SHOTS: u64 = 4_000;
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    for _ in 0..SHOTS {
        hunt.advance(0.001);
        hunt.shot(9.0);
    }
    let before = work(&hunt);
    // Back, with the cannon, seen a second after the cover began: the
    // first thousand are settled, the rest placed again.
    hunt.see(holding(2), SHOTS as f64 * 0.001 - 1.0);
    let taken = work(&hunt) - before;
    assert_eq!(hunt.pending(), phases(&[("Unknown", SHOTS as i64)]));
    assert!(
        taken <= 24 * SHOTS,
        "{taken} steps placing {SHOTS} shots again"
    );
}

// The set-up of the tool in hand, as the overlay shows it.

/// The cannon fitted with an amplifier, two damage enhancers, and its
/// weapon at 150% markup.
fn amped_cannon() -> CarriedWeaponProfile {
    let mut profile = carried(2, "Cannon", 40.0, 0.2);
    profile.props.insert(
        "amp_entity".into(),
        serde_json::json!({"name": "A101", "economy": {"decay": 0.1, "ammo_burn": 0}}),
    );
    profile
        .props
        .insert("damage_enhancers".into(), serde_json::json!(2));
    profile
        .props
        .insert("weapon_markup".into(), serde_json::json!(150));
    profile
}

/// The amped cannon's price and TT per shot (PED) with `enhancers` fitted.
fn amped_cannon_price(enhancers: i64) -> (f64, f64) {
    let priced = crate::cost_engine::cost_per_shot_from_props(
        &serde_json::Value::Object(amped_cannon().props),
        Some(enhancers),
    );
    (
        priced["totalCostPerUse"].as_f64().unwrap() / 100.0,
        crate::tool_setup::tt_per_shot_pec(&priced) / 100.0,
    )
}

impl Hunt {
    fn hand(&self) -> Option<super::HandView> {
        self.rig
            .probe(&self.tracker, |actor| actor.tool_display().hand)
    }

    fn setup(&self) -> super::HandSetup {
        self.hand()
            .and_then(|hand| hand.setup)
            .expect("the hand has a set-up")
    }

    /// The per-shot price the session booked the shots of `name` at.
    fn booked_price(&self, name: &str) -> f64 {
        let name = name.to_string();
        self.rig.probe(&self.tracker, move |actor| {
            actor
                .session
                .active()
                .unwrap()
                .accumulator
                .tool_stats
                .iter()
                .find(|(_, stats)| stats.tool_name == name)
                .map(|(_, stats)| stats.cost_per_shot.value())
                .expect("shots were booked")
        })
    }
}

#[test]
fn a_weapon_in_hand_shows_its_parts_enhancers_and_the_price_its_shots_are_booked_at() {
    let hunt = hunt_over(
        true,
        false,
        vec![carried(1, "Pistol", 10.0, 0.05), amped_cannon()],
    );
    hunt.start();
    hunt.see(holding(2), 0.0);
    let (price, tt) = amped_cannon_price(2);
    let setup = hunt.setup();
    assert_eq!(setup.kind, HotbarItemKind::Weapon);
    assert_eq!(
        setup.parts,
        Some(vec![crate::tool_setup::SetupPart {
            role: crate::tool_setup::SetupRole::Amplifier,
            name: "A101".into()
        }])
    );
    assert_eq!(
        setup.enhancers,
        Some(super::Enhancers {
            active: 2,
            fitted: 2
        })
    );
    assert!((setup.cost_per_use.value() - price).abs() < 1e-12);
    assert!((setup.tt_per_use.unwrap().value() - tt).abs() < 1e-12);
    assert!(price > tt, "the markup moves the price");

    hunt.shot(30.0);
    assert!(
        (hunt.booked_price("Cannon") - setup.cost_per_use.value()).abs() < 1e-12,
        "what is shown is what a shot is booked at"
    );

    // One enhancer wears out: the price shown follows.
    hunt.rig
        .bus
        .publish(&BusEvent::EnhancerBreak(EnhancerBreakPayload {
            kind: EnhancerBreakTag,
            timestamp: "2026-01-01T00:00:01".into(),
            enhancer_name: "Damage Enhancer 1".into(),
            item_name: "Cannon".into(),
            remaining: 1,
            shrapnel_ped: 0.0,
        }));
    let worn = hunt.setup();
    assert_eq!(
        worn.enhancers,
        Some(super::Enhancers {
            active: 1,
            fitted: 2
        })
    );
    assert!((worn.cost_per_use.value() - amped_cannon_price(1).0).abs() < 1e-12);
    assert!(worn.cost_per_use.value() < setup.cost_per_use.value());
}

#[test]
fn a_weapon_set_up_without_enhancers_or_parts_shows_none() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(1), 0.0);
    let setup = hunt.setup();
    assert_eq!(setup.parts, Some(Vec::new()));
    assert_eq!(setup.enhancers, None);
    assert!((setup.cost_per_use.value() - pistol()).abs() < 1e-12);
    assert_eq!(hunt.hand().unwrap().library_id, Some(1));
}

#[test]
fn a_healer_or_finder_in_hand_shows_the_price_it_was_declared_at() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(4), 0.0);
    let healer = hunt.setup();
    assert_eq!(healer.kind, HotbarItemKind::Healing);
    assert_eq!(healer.parts, Some(Vec::new()), "resolved with its parts");
    assert_eq!(healer.enhancers, None);
    assert!((healer.cost_per_use.value() - 0.03).abs() < 1e-12);
    assert_eq!(healer.tt_per_use, None);

    hunt.advance(1.0);
    hunt.see(holding(6), 0.0);
    let finder = hunt.setup();
    assert_eq!(finder.kind, HotbarItemKind::Harvesting);
    assert!((finder.cost_per_use.value() - 0.02).abs() < 1e-12);
}

#[test]
fn a_tool_known_only_from_the_game_data_shows_bare_at_the_price_it_is_booked_at() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(rusty_seen(), 0.0);
    let hand = hunt.hand().unwrap();
    assert_eq!(hand.library_id, None);
    let setup = hand.setup.unwrap();
    assert_eq!(setup.parts, Some(Vec::new()));
    assert_eq!(setup.enhancers, None);
    assert!((setup.cost_per_use.value() - rusty()).abs() < 1e-12);
    hunt.shot(150.0);
    assert!((hunt.booked_price("Rusty Rifle") - rusty()).abs() < 1e-12);
}

#[test]
fn a_covered_bar_keeps_the_set_up_of_the_tool_in_hand_when_it_went() {
    let hunt = hunt_over(
        true,
        false,
        vec![carried(1, "Pistol", 10.0, 0.05), amped_cannon()],
    );
    hunt.start();
    hunt.see(holding(2), 0.0);
    hunt.advance(1.0);
    hunt.see(ToolSeen::NotVisible, 0.0);
    let unseen = hunt
        .rig
        .probe(&hunt.tracker, |actor| actor.tool_display().unseen_hand)
        .and_then(|hand| hand.setup)
        .expect("the tool in hand when the bar went");
    assert_eq!(unseen.parts.map(|parts| parts.len()), Some(1));
    assert!((unseen.cost_per_use.value() - amped_cannon_price(2).0).abs() < 1e-12);
}

#[test]
fn a_declaration_from_an_intent_alone_does_not_claim_to_know_the_parts() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(ToolSeen::Uncertain(Why::Unreadable), 0.0);
    hunt.intent(IntentSource::Overlay, "FAP", HotbarItemKind::Healing);
    let setup = hunt.setup();
    assert_eq!(setup.kind, HotbarItemKind::Healing);
    assert_eq!(setup.parts, None);
    assert!((setup.cost_per_use.value() - 0.03).abs() < 1e-12);
}

// The healer in hand while the screen is the source: holding it is a
// standing claim, not a press.

#[test]
fn a_heal_below_the_held_healers_band_is_not_billed_without_a_press() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(4), 0.0);
    hunt.advance(0.5);
    hunt.heal(80.0);
    assert_eq!(activations(&hunt).len(), 1, "an in-band heal is a use");
    // Past the reload, a small heal (a regeneration tick, a buff): it
    // could be a use capped by full health, or passive healing; with no
    // press it cannot be told apart, so it costs nothing.
    hunt.advance(4.0);
    hunt.heal(20.0);
    assert_eq!(activations(&hunt).len(), 1, "not billed");
    assert_eq!(
        heal_classes(&hunt).last().map(String::as_str),
        Some("unattributed")
    );
}

#[test]
fn a_capped_heal_on_a_press_of_the_held_healer_is_billed_and_a_late_one_is_not() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(4), 0.0);
    hunt.advance(4.0);
    hunt.key("FAP", HotbarItemKind::Healing);
    hunt.advance(0.3);
    hunt.heal(20.0);
    let stored = activations(&hunt);
    assert_eq!(stored.len(), 1, "the press makes the small heal a use");
    assert_eq!(stored[0].0.as_deref(), Some("key"));

    // A press whose edge has passed vouches for nothing.
    hunt.advance(4.0);
    hunt.key("FAP", HotbarItemKind::Healing);
    hunt.advance(2.0);
    hunt.heal(20.0);
    assert_eq!(activations(&hunt).len(), 1);
    // An in-band heal past the reload is still a use without the edge.
    hunt.advance(2.0);
    hunt.heal(80.0);
    assert_eq!(activations(&hunt).len(), 2);
}

#[test]
fn a_heal_the_held_weapons_lifesteal_explains_is_not_a_use_of_the_healer() {
    let weapons = || {
        vec![
            carried(1, "Pistol", 10.0, 0.05),
            carried(9, "Leech", 160.0, 0.5),
        ]
    };
    let hunt = hunt_over(true, false, weapons());
    hunt.start();
    hunt.see(holding(9), 0.0);
    hunt.shot(150.0);
    // Swapped to the healer as the hit's lifesteal (75) lands, inside the
    // healer's band.
    hunt.see(holding(4), 0.0);
    hunt.heal(75.0);
    assert!(
        activations(&hunt).is_empty(),
        "the lifesteal, not the healer"
    );
    assert_eq!(heal_classes(&hunt), vec!["passive".to_string()]);

    // A heal the lifesteal does not explain is the healer's.
    let hunt = hunt_over(true, false, weapons());
    hunt.start();
    hunt.see(holding(9), 0.0);
    hunt.shot(150.0);
    hunt.see(holding(4), 0.0);
    hunt.heal(98.0);
    assert_eq!(activations(&hunt).len(), 1);
}

#[test]
fn the_screen_seeing_a_healer_gives_a_tick_no_edge_and_a_press_does() {
    let hunt = hunt(true);
    hunt.start();
    hunt.see(holding(10), 0.0);
    hunt.advance(0.5);
    hunt.heal(90.0);
    assert_eq!(
        activations(&hunt).len(),
        1,
        "the first use opens its effect"
    );

    // Put away and taken up again past the reload: a heal inside the
    // effect's ticks and the direct band is the effect's tick.
    hunt.advance(4.0);
    hunt.see(holding(1), 0.0);
    hunt.see(holding(10), 0.0);
    hunt.advance(0.3);
    hunt.heal(65.0);
    assert_eq!(activations(&hunt).len(), 1, "a tick, not a use");
    assert_eq!(
        heal_classes(&hunt).last().map(String::as_str),
        Some("effect")
    );

    // A press of the held healer is a use: the same heal is its output.
    hunt.advance(1.0);
    hunt.key("Mender", HotbarItemKind::Healing);
    hunt.advance(0.3);
    hunt.heal(65.0);
    let stored = activations(&hunt);
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[1].0.as_deref(), Some("key"));
}
