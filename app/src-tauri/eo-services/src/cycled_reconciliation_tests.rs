//! Cycled reconciliation across every read that reports it.
//!
//! A session's Cycled is the sum of seven buckets: weapon, enhancer,
//! harvest decay, armour (armour and plates), healing, consumable doses,
//! and dangling unresolved spend. Generated sessions carry arbitrary
//! amounts in every bucket, arbitrary activity contexts (single quests,
//! family quests, segments, and co-active bundles of them), and kills with
//! or without a context stamp. The property then reads every projection
//! and checks that:
//!
//! - the session summary, the session list, and session detail each report
//!   the session's full bucket sum;
//! - the Overview's Cycled is the sum over sessions, its breakdown has one
//!   line per bucket summing to it, and its timeline sums to it;
//! - the narrower activity reads are honest subsets that count each cost
//!   once: the Tree Cutting tiers partition harvest decay, and the Hunting
//!   definitions, their top-level activity rows (bundles included), and
//!   the species rows each partition the kill-grain weapon and enhancer
//!   cost;
//! - the maintained projections equal a rebuild from the raw tables.
//!
//! Some sessions also carry costs the account readings measured, bucket by
//! bucket: a measured bucket is the session's cost wherever a read reports
//! it (at nothing, too), the tracked events share it out below the
//! session, and a bucket the readings left unmeasured keeps its tracked
//! figure.
//!
//! How each stream's amount reaches its bucket (a protection recording's
//! spread over sessions, a dose's booking, a shot correction) is proven
//! where that stream is written; this property starts from the recorded
//! facts. Amounts are whole PEC, so every tracked figure is exact at two
//! decimals; a measured bucket shared over events is exact to a rounding
//! error.

use std::collections::BTreeSet;
use std::sync::Arc;

use proptest::prelude::*;

use crate::analytics::AnalyticsService;
use crate::clock::MockClock;
use crate::db::{Db, DbError};
use crate::time::naive_to_epoch;

/// The activity members a context may name: a standalone quest, two
/// variants of one quest family, and two segments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Member {
    Standalone,
    FamilyA,
    FamilyB,
    LapOne,
    LapTwo,
}

impl Member {
    /// `(kind, label, ref_id)` of the interval this member records.
    fn interval(self) -> (&'static str, &'static str, Option<i64>) {
        match self {
            Self::Standalone => ("quest", "Standalone Quest", Some(11)),
            Self::FamilyA => ("quest", "Daily Hunting 1: Atrox", Some(12)),
            Self::FamilyB => ("quest", "Daily Hunting 1: Daikiba", Some(13)),
            Self::LapOne => ("segment", "Lap One", None),
            Self::LapTwo => ("segment", "Lap Two", None),
        }
    }
}

fn member() -> impl Strategy<Value = Member> {
    prop_oneof![
        Just(Member::Standalone),
        Just(Member::FamilyA),
        Just(Member::FamilyB),
        Just(Member::LapOne),
        Just(Member::LapTwo),
    ]
}

#[derive(Debug, Clone)]
struct GenKill {
    /// Weapon phases: `(shots, cost per shot in PEC)`.
    phases: Vec<(u32, u32)>,
    enhancer_pec: u32,
    species: &'static str,
    /// Index into the session's contexts; None is an unstamped kill.
    context: Option<usize>,
}

#[derive(Debug, Clone)]
struct GenSession {
    /// Hours between the session's end and the frozen clock.
    ended_hours_ago: u32,
    duration_minutes: u32,
    under_definition: bool,
    /// Each context's co-active members (empty is an unscoped context).
    contexts: Vec<BTreeSet<Member>>,
    kills: Vec<GenKill>,
    /// Harvest swings: `(decay in PEC, yield tier)`.
    swings: Vec<(u32, &'static str)>,
    armour_pec: u32,
    heal_pec: u32,
    consumable_pec: u32,
    dangling_pec: u32,
    /// The account readings' measured costs, when the session has them: per
    /// bucket (weapon, enhancer, harvesting, healing, armour, consumables),
    /// the amount in PEC, or unmeasured.
    measured: Option<[Option<u32>; 6]>,
}

fn gen_kill(contexts: usize) -> impl Strategy<Value = GenKill> {
    (
        prop::collection::vec((0u32..30, 1u32..40), 1..3),
        0u32..20,
        prop_oneof![Just(""), Just("Atrox"), Just("Daikiba")],
        prop::option::of(0..contexts.max(1)),
    )
        .prop_map(move |(phases, enhancer_pec, species, context)| GenKill {
            phases,
            enhancer_pec,
            species,
            context: context.filter(|_| contexts > 0),
        })
}

fn gen_session() -> impl Strategy<Value = GenSession> {
    prop::collection::vec(prop::collection::btree_set(member(), 0..3), 0..3).prop_flat_map(
        |contexts| {
            let count = contexts.len();
            (
                Just(contexts),
                0u32..96,
                10u32..240,
                any::<bool>(),
                prop::collection::vec(gen_kill(count), 0..6),
                prop::collection::vec(
                    (
                        1u32..20,
                        prop_oneof![Just("short"), Just("long"), Just("huge")],
                    ),
                    0..4,
                ),
                (0u32..500, 0u32..500, 0u32..500, 0u32..50),
                prop::option::of(prop::array::uniform6(prop::option::of(prop_oneof![
                    Just(0u32),
                    1u32..2000
                ]))),
            )
                .prop_map(
                    |(
                        contexts,
                        ended_hours_ago,
                        duration_minutes,
                        under_definition,
                        kills,
                        swings,
                        (armour_pec, heal_pec, consumable_pec, dangling_pec),
                        measured,
                    )| GenSession {
                        ended_hours_ago,
                        duration_minutes,
                        under_definition,
                        contexts,
                        kills,
                        swings,
                        armour_pec,
                        heal_pec,
                        consumable_pec,
                        dangling_pec,
                        measured,
                    },
                )
        },
    )
}

fn ped(pec: u32) -> f64 {
    f64::from(pec) / 100.0
}

/// One session's expected buckets, in PED, and what its kills and swings
/// carry of them.
#[derive(Debug, Default, Clone, Copy)]
struct Buckets {
    weapon: f64,
    enhancer: f64,
    harvest: f64,
    armour: f64,
    heal: f64,
    consumable: f64,
    dangling: f64,
    /// The weapon and enhancer cost its kills carry.
    kill_grain: f64,
    /// The harvesting cost its swings carry.
    swing_grain: f64,
}

impl Buckets {
    /// The buckets as the chat log tracked them, then resolved against the
    /// session's measured costs, by the rule stated independently here: a
    /// measured bucket is its measured amount, nothing included, shared over
    /// its tracked events by their cost (the weapon's over its kills and its
    /// dangling spend); a bucket left unmeasured stays tracked.
    fn of(session: &GenSession) -> Self {
        let kill_weapon = |kill: &GenKill| -> u64 {
            kill.phases
                .iter()
                .map(|(shots, pec)| u64::from(*shots) * u64::from(*pec))
                .sum()
        };
        let pec = |amount: u64| amount as f64 / 100.0;
        let weapon = pec(session.kills.iter().map(kill_weapon).sum());
        let enhancer = pec(session
            .kills
            .iter()
            .map(|k| u64::from(k.enhancer_pec))
            .sum());
        let harvest = pec(session.swings.iter().map(|(pec, _)| u64::from(*pec)).sum());
        let dangling = ped(session.dangling_pec);
        let mut out = Self {
            weapon,
            enhancer,
            harvest,
            armour: ped(session.armour_pec),
            heal: ped(session.heal_pec),
            consumable: ped(session.consumable_pec),
            dangling,
            kill_grain: weapon + enhancer,
            swing_grain: harvest,
        };
        let Some(measured) = session.measured else {
            return out;
        };
        let taken = |index: usize| -> Option<f64> { measured[index].map(ped) };
        let mut kill_weapon_resolved = weapon;
        if let Some(amount) = taken(0) {
            if weapon + dangling > 0.0 {
                out.weapon = weapon * amount / (weapon + dangling);
                out.dangling = dangling * amount / (weapon + dangling);
                kill_weapon_resolved = out.weapon;
            } else {
                out.weapon = amount;
                kill_weapon_resolved = 0.0;
            }
        }
        let mut kill_enhancer_resolved = enhancer;
        if let Some(amount) = taken(1) {
            out.enhancer = amount;
            if enhancer == 0.0 {
                kill_enhancer_resolved = 0.0;
            } else {
                kill_enhancer_resolved = amount;
            }
        }
        if let Some(amount) = taken(2) {
            out.harvest = amount;
            out.swing_grain = if harvest > 0.0 { amount } else { 0.0 };
        }
        if let Some(amount) = taken(3) {
            out.heal = amount;
        }
        if let Some(amount) = taken(4) {
            out.armour = amount;
        }
        if let Some(amount) = taken(5) {
            out.consumable = amount;
        }
        out.kill_grain = kill_weapon_resolved + kill_enhancer_resolved;
        out
    }

    fn cycled(self) -> f64 {
        self.weapon
            + self.enhancer
            + self.harvest
            + self.armour
            + self.heal
            + self.consumable
            + self.dangling
    }

    fn add(&mut self, other: Self) {
        self.weapon += other.weapon;
        self.enhancer += other.enhancer;
        self.harvest += other.harvest;
        self.armour += other.armour;
        self.heal += other.heal;
        self.consumable += other.consumable;
        self.dangling += other.dangling;
        self.kill_grain += other.kill_grain;
        self.swing_grain += other.swing_grain;
    }
}

/// Write the generated sessions as the tracker would leave them: each kill's
/// `cost_ped` equal to the cost of its weapon phases, and every derived
/// projection written the way a session end writes it.
fn seed(conn: &rusqlite::Connection, now: f64, sessions: &[GenSession]) -> Result<(), DbError> {
    conn.execute_batch(
        "INSERT INTO session_definitions(id, name, ad_hoc_segments, is_active) \
             VALUES(7, 'Dailies', 0, 1); \
         INSERT INTO quest_families(id, name) VALUES(3, 'Daily Hunting 1'); \
         INSERT INTO quests(id, name) VALUES(11, 'Standalone Quest'); \
         INSERT INTO quests(id, name, family_id) VALUES(12, 'Daily Hunting 1: Atrox', 3); \
         INSERT INTO quests(id, name, family_id) VALUES(13, 'Daily Hunting 1: Daikiba', 3);",
    )?;
    for (index, session) in sessions.iter().enumerate() {
        let id = format!("s{index}");
        let ended_at = now - f64::from(session.ended_hours_ago) * 3600.0 - 60.0;
        let started_at = ended_at - f64::from(session.duration_minutes) * 60.0;
        conn.execute(
            "INSERT INTO tracking_sessions(id, started_at, ended_at, is_active, armour_cost, \
             heal_cost, consumable_cost, dangling_cost, definition_id) \
             VALUES(?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                id,
                started_at,
                ended_at,
                ped(session.armour_pec),
                ped(session.heal_pec),
                ped(session.consumable_pec),
                ped(session.dangling_pec),
                session.under_definition.then_some(7_i64),
            ],
        )?;

        let mut context_ids = Vec::with_capacity(session.contexts.len());
        for members in &session.contexts {
            conn.execute(
                "INSERT INTO session_contexts(session_id, created_at) VALUES(?1, ?2)",
                rusqlite::params![id, started_at],
            )?;
            let context_id = conn.last_insert_rowid();
            for member in members {
                let (kind, label, ref_id) = member.interval();
                conn.execute(
                    "INSERT INTO session_intervals(session_id, kind, label, ref_id, \
                     started_at, ended_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![id, kind, label, ref_id, started_at, ended_at],
                )?;
                conn.execute(
                    "INSERT INTO session_context_intervals(context_id, interval_id) \
                     VALUES(?1, ?2)",
                    rusqlite::params![context_id, conn.last_insert_rowid()],
                )?;
            }
            context_ids.push(context_id);
        }

        for (k, kill) in session.kills.iter().enumerate() {
            let kill_id = format!("{id}-k{k}");
            let weapon: u64 = kill
                .phases
                .iter()
                .map(|(shots, pec)| u64::from(*shots) * u64::from(*pec))
                .sum();
            conn.execute(
                "INSERT INTO kills(id, session_id, mob_name, mob_species, mob_maturity, \
                 timestamp, cost_ped, enhancer_cost, loot_total_ped, context_id) \
                 VALUES(?1, ?2, ?3, ?4, 'Young', ?5, ?6, ?7, 0.5, ?8)",
                rusqlite::params![
                    kill_id,
                    id,
                    if kill.species.is_empty() {
                        "Old Tag"
                    } else {
                        kill.species
                    },
                    kill.species,
                    started_at + 1.0 + k as f64,
                    weapon as f64 / 100.0,
                    ped(kill.enhancer_pec),
                    kill.context.map(|index| context_ids[index]),
                ],
            )?;
            for (p, (shots, pec)) in kill.phases.iter().enumerate() {
                conn.execute(
                    "INSERT INTO kill_tool_stats(kill_id, tool_name, shots_fired, cost_per_shot) \
                     VALUES(?1, ?2, ?3, ?4)",
                    rusqlite::params![kill_id, format!("Tool {p}"), shots, ped(*pec)],
                )?;
            }
        }

        for (s, (pec, tier)) in session.swings.iter().enumerate() {
            conn.execute(
                "INSERT INTO harvest_events(id, session_id, timestamp, success, tool_name, \
                 yield_tier, cost_ped, loot_total_ped) VALUES(?1, ?2, ?3, 1, 'Cutter', ?4, ?5, 0.1)",
                rusqlite::params![
                    format!("{id}-h{s}"),
                    id,
                    started_at + 2.0 + s as f64,
                    tier,
                    ped(*pec),
                ],
            )?;
        }

        if let Some(measured) = &session.measured {
            seed_measured_costs(conn, &id, measured)?;
        }

        crate::session_summary::write_session_summary(conn, &id)?;
        crate::daily_rollup::refresh_session_days(conn, &id)?;
        crate::session_rollup::recompute_session(conn, &id)?;
    }
    Ok(())
}

/// The measured costs as the account side stores them for a sealed
/// session, with a measured overhead bucket beside them that no Cycled
/// figure may include.
fn seed_measured_costs(
    conn: &rusqlite::Connection,
    id: &str,
    measured: &[Option<u32>; 6],
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO account_session_costs (session_id, costs_version, measure_version, \
         measure_computed_at, status, reason, computed_at) \
         VALUES (?1, ?2, ?3, 1.0, 'measured', NULL, 2.0)",
        rusqlite::params![
            id,
            crate::session_costs::MEASURED_COSTS_VERSION,
            crate::session_costs::MEASURED_MEASURE_VERSION
        ],
    )?;
    let buckets = [
        "weapon",
        "enhancer",
        "harvesting",
        "healing",
        "armour",
        "consumables",
    ];
    for (bucket, amount) in buckets.iter().zip(measured) {
        match amount {
            Some(pec) => conn.execute(
                "INSERT INTO account_session_cost_buckets \
                 (session_id, bucket, basis, reason, amount_micro_ped, precision) \
                 VALUES (?1, ?2, 'measured', NULL, ?3, 'wholePec')",
                rusqlite::params![id, bucket, i64::from(*pec) * 10_000],
            )?,
            None => conn.execute(
                "INSERT INTO account_session_cost_buckets \
                 (session_id, bucket, basis, reason, amount_micro_ped, precision) \
                 VALUES (?1, ?2, 'unmeasured', 'gap', NULL, NULL)",
                rusqlite::params![id, bucket],
            )?,
        };
    }
    conn.execute(
        "INSERT INTO account_session_cost_buckets \
         (session_id, bucket, basis, reason, amount_micro_ped, precision) \
         VALUES (?1, 'travel', 'measured', NULL, 3000000, 'wholePec')",
        [id],
    )?;
    Ok(())
}

fn close(actual: f64, expected: f64, what: &str) {
    near(actual, expected, 0, what);
}

/// Within the rounding of `parts` figures each shown to the PEC: a measured
/// bucket shared over events leaves fractions of a PEC on each part, while
/// a whole session's figure stays whole.
fn near(actual: f64, expected: f64, parts: usize, what: &str) {
    let tolerance = 1e-6 + 0.005 * parts as f64;
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual} != {expected}"
    );
}

async fn check(sessions: Vec<GenSession>) {
    let naive =
        chrono::NaiveDateTime::parse_from_str("2026-06-01T12:00:00", "%Y-%m-%dT%H:%M:%S").unwrap();
    let now = naive_to_epoch(naive);
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("entropia_orme.db"))
        .await
        .unwrap();
    {
        let sessions = sessions.clone();
        db.with_writer(move |conn| {
            let tx = conn.transaction()?;
            seed(&tx, now, &sessions)?;
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
    }
    let service = AnalyticsService::new(db.clone(), Arc::new(MockClock::new(Some(naive), 0.0)));

    let buckets: Vec<Buckets> = sessions.iter().map(Buckets::of).collect();
    let mut total = Buckets::default();
    for session in &buckets {
        total.add(*session);
    }

    // Per session: the summary, the list row, and the detail agree on the
    // full bucket sum.
    let page = crate::tracking_reads::list_sessions_impl(&db, now, None, Some(200), None)
        .await
        .unwrap();
    let rows = page.sessions.as_array().unwrap().clone();
    for (index, expected) in buckets.iter().enumerate() {
        let id = format!("s{index}");
        let stored: Option<f64> = {
            let id = id.clone();
            db.with_reader(move |conn| {
                use rusqlite::OptionalExtension as _;
                Ok(conn
                    .query_row(
                        "SELECT cycled_ped FROM session_summaries WHERE session_id = ?1",
                        [id],
                        |row| row.get(0),
                    )
                    .optional()?)
            })
            .await
            .unwrap()
        };
        match stored {
            Some(cycled) => close(cycled, expected.cycled(), "summary cycled"),
            None => assert_eq!(expected.cycled(), 0.0, "a costed session has a summary"),
        }
        let row = rows.iter().find(|row| row["id"] == id.as_str()).unwrap();
        close(
            row["cost"].as_f64().unwrap(),
            expected.cycled(),
            "list cost",
        );
        let detail = crate::tracking_reads::get_session_impl(&db, &id, now)
            .await
            .unwrap()
            .unwrap();
        close(
            detail["summary"]["cost"].as_f64().unwrap(),
            expected.cycled(),
            "detail cost",
        );
    }

    // The Overview: Cycled is the sum over sessions, one breakdown line per
    // bucket, and the timeline sums to it.
    let overview = service.overview("all").await.unwrap();
    let losses = &overview.losses_breakdown;
    let lines = &losses.cycled_breakdown;
    close(losses.tracking_cost, total.cycled(), "overview cycled");
    near(lines.weapon.as_f64(), total.weapon, 1, "weapon line");
    close(lines.enhancer.as_f64(), total.enhancer, "enhancer line");
    close(lines.harvest.as_f64(), total.harvest, "harvest line");
    close(lines.armour.as_f64(), total.armour, "armour line");
    close(lines.healing.as_f64(), total.heal, "healing line");
    close(
        lines.consumables.as_f64(),
        total.consumable,
        "consumables line",
    );
    near(lines.dangling.as_f64(), total.dangling, 1, "dangling line");
    let timeline: f64 = overview.timeline.iter().map(|day| day.tracking_cost).sum();
    close(timeline, total.cycled(), "overview timeline");

    // Tree Cutting: the tiers partition harvest decay.
    let harvest = service.harvest("all").await.unwrap();
    let tiers: f64 = harvest
        .tier_comparisons
        .iter()
        .map(|tier| tier.cycled)
        .sum();
    near(
        tiers,
        total.swing_grain,
        harvest.tier_comparisons.len(),
        "harvest tiers",
    );

    // Hunting: kill-grain weapon and enhancer cost of the sessions that
    // hunted, partitioned by definition, by top-level activity row within
    // each definition (a co-active bundle is one row, never one per
    // member), and by species.
    let hunted: f64 = sessions
        .iter()
        .zip(&buckets)
        .filter(|(session, _)| !session.kills.is_empty())
        .map(|(_, buckets)| buckets.kill_grain)
        .sum();
    let hunting = service.hunting_activity("all").await.unwrap();
    near(hunting.overall.cycled, hunted, 1, "hunting overall");
    let definitions: f64 = hunting.definitions.iter().map(|row| row.cycled).sum();
    near(
        definitions,
        hunted,
        hunting.definitions.len(),
        "hunting definitions",
    );
    for definition in &hunting.definitions {
        let activities: f64 = definition.activities.iter().map(|row| row.cycled).sum();
        near(
            activities,
            definition.cycled,
            definition.activities.len() + 1,
            &format!("{} activities", definition.name),
        );
    }
    let species: f64 = hunting.species.iter().map(|row| row.cycled).sum();
    near(species, hunted, hunting.species.len(), "hunting species");
    assert!(hunting.overall.cycled <= losses.tracking_cost + 0.005 + 1e-6);

    // Rebuilding every projection from the raw tables changes nothing.
    let report = crate::maintenance::rebuild_and_verify(&db, now)
        .await
        .unwrap();
    assert!(report.all_matched(), "{:?}", report.tables);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn cycled_reconciles_across_every_projection(
        sessions in prop::collection::vec(gen_session(), 1..5),
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(check(sessions));
    }
}
