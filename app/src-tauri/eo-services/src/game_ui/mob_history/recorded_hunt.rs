//! Hunts recorded in play, replayed through the history as it now is.
//!
//! A record holds what one hunt's looks read of the locked plate (when,
//! the health shown, the name as the recogniser gave it) and its kills
//! (the logged second, and the span the report's arrival is known to lie
//! in), with what the application answered at the time. The replay
//! resolves each name afresh, runs the history over the looks, and
//! reports what each kill is given now, beside what it was given then. A
//! record taken at another cadence of looks carries the history values of
//! that cadence, and is replayed with them: the rules are the ones under
//! test, the timing is the hunt's own.
//!
//! Nobody wrote down which creature each kill really was, so a record
//! proves no answer right. What it can hold the history to is the rule
//! that makes an unnamed kill honest: no kill is given a creature that no
//! look read before the kill's death.
//!
//! Recordings of play stay out of the public tree: the replay runs only
//! where `EO_MOB_HUNT_RECORDS` names a directory of records, and says so
//! when it skips.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use super::*;
use crate::game_ui::profile::UiProfile;
use crate::game_ui::test_support::bundled_mobs;

#[derive(Deserialize)]
struct Record {
    pulses: Vec<Pulse>,
    kills: Vec<RecordedKill>,
    /// History values to replay with in place of the bundled profile's,
    /// by name.
    #[serde(default)]
    history: serde_json::Map<String, serde_json::Value>,
}

/// The bundled profile, with `record`'s own history values over it.
fn profile_for(record: &Record) -> UiProfile {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../entropia-orme/resources/ui_profile.json");
    let text = std::fs::read_to_string(&path).expect("the bundled interface profile reads");
    let mut profile: serde_json::Value =
        serde_json::from_str(&text).expect("the bundled interface profile parses");
    for (name, value) in &record.history {
        profile["mob_history"][name] = value.clone();
    }
    UiProfile::from_json(&profile.to_string()).expect("the record's profile loads")
}

#[derive(Deserialize)]
struct Pulse {
    at_ms: i64,
    fill: Option<f64>,
    text: Option<String>,
}

#[derive(Deserialize)]
struct RecordedKill {
    logged_ms: i64,
    /// The report arrived this long after the logged second at the
    /// earliest, and before this long after it.
    arrived_from: i64,
    arrived_before: i64,
    /// What the screen's readings came to at the time, and the route the
    /// kill's creature was recorded by.
    screen: Option<String>,
    route: Option<String>,
    mob: Option<String>,
    suggested: Option<String>,
}

/// What a replayed kill is given: the grade, and the creature as shown.
#[derive(Debug, Clone, PartialEq)]
struct Given {
    grade: &'static str,
    creature: Option<String>,
}

fn records() -> Vec<(String, Record)> {
    let Some(dir) = std::env::var_os("EO_MOB_HUNT_RECORDS")
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
    else {
        eprintln!("skipped: EO_MOB_HUNT_RECORDS is not set, so there are no recorded hunts");
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("the records directory lists") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_some_and(|e| e == "json") {
            let text = std::fs::read_to_string(&path).expect("a record reads");
            let record = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            let name = path
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            found.push((name, record));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// The level plates are resolved with: records do not hold the level
/// read, so the replay resolves names with none unless told one.
fn level() -> Option<u32> {
    std::env::var("EO_MOB_HUNT_LEVEL").ok()?.parse().ok()
}

fn shown(creature: &Creature) -> String {
    match creature {
        Creature::Catalogued { name, maturity, .. } | Creature::Species { name, maturity, .. } => {
            match maturity {
                Some(m) => format!("{name} {m}"),
                None => name.clone(),
            }
        }
        Creature::Confirmed { name } => name.clone(),
    }
}

/// Replay `record`, each report taken to have arrived `lag_ms` after its
/// logged second where the record allows that, else as near it as it does.
fn replay(record: &Record, lag_ms: i64) -> Vec<Given> {
    let profile = profile_for(record);
    let catalogue = MobCatalogue::from_entities(
        &bundled_mobs(),
        Vec::<crate::game_ui::mob_names::KeptCreature>::new(),
        &profile.mob_names,
    );
    let mut history = MobHistory::new(profile.mob_history.clone(), profile.mob_names.clone());
    let reports: Vec<KillReport> = record
        .kills
        .iter()
        .map(|k| {
            let lag = lag_ms.clamp(k.arrived_from, (k.arrived_before - 1).max(k.arrived_from));
            KillReport {
                logged_ms: k.logged_ms,
                arrived_ms: k.logged_ms + lag,
            }
        })
        .collect();
    let mut given: Vec<Option<Given>> = vec![None; reports.len()];
    let level = level();
    for pulse in &record.pulses {
        let sighting = pulse.fill.map(|fill| {
            let resolved = match &pulse.text {
                Some(text) => resolve(text, level, &catalogue, &profile.mob_names),
                None => Resolved::Unclear,
            };
            Sighting::resolved(fill, pulse.text.clone(), resolved, &catalogue)
        });
        history.observe(pulse.at_ms, sighting);
        for (k, report) in reports.iter().enumerate() {
            if given[k].is_some() || report.arrived_ms > pulse.at_ms {
                continue;
            }
            let (grade, creature) = match history.stamp_for_kill(*report, pulse.at_ms) {
                KillStamp::NotYet => continue,
                KillStamp::Unnamed(why) => (why.label(), None),
                KillStamp::Stamped(stamp) => {
                    let who = match &stamp {
                        MobStamp::Named { creature, .. }
                        | MobStamp::Guess { creature, .. }
                        | MobStamp::LatestRead { creature, .. }
                        | MobStamp::Continuity { creature, .. } => shown(creature),
                        MobStamp::Provisional { spelling, .. }
                        | MobStamp::ProvisionalGuess { spelling, .. }
                        | MobStamp::ProvisionalLatestRead { spelling, .. }
                        | MobStamp::ProvisionalContinuity { spelling, .. } => spelling.clone(),
                    };
                    (stamp.grade_label(), Some(who))
                }
            };
            given[k] = Some(Given { grade, creature });
        }
    }
    given
        .into_iter()
        .map(|g| {
            g.unwrap_or(Given {
                grade: "not_yet",
                creature: None,
            })
        })
        .collect()
}

/// The names the looks read from `from_ms` up to `to_ms`, as shown.
fn names_read(record: &Record, from_ms: i64, to_ms: i64) -> Vec<String> {
    let profile = profile_for(record);
    let catalogue = MobCatalogue::from_entities(
        &bundled_mobs(),
        Vec::<crate::game_ui::mob_names::KeptCreature>::new(),
        &profile.mob_names,
    );
    let level = level();
    record
        .pulses
        .iter()
        .filter(|p| p.at_ms >= from_ms && p.at_ms <= to_ms)
        .filter_map(|p| p.text.as_ref())
        .filter_map(
            |text| match resolve(text, level, &catalogue, &profile.mob_names) {
                Resolved::Known { name, maturity, .. }
                | Resolved::Species { name, maturity, .. } => Some(match maturity {
                    Some(m) => format!("{name} {m}"),
                    None => name,
                }),
                Resolved::Confirmed { name, .. } => Some(name),
                Resolved::New { text, .. } => Some(text),
                Resolved::Unclear => None,
            },
        )
        .collect()
}

fn key(text: &str) -> String {
    // A number is shown as the list has it (`4`) and printed padded (`04`).
    text.split_whitespace()
        .map(|word| match word.parse::<u32>() {
            Ok(n) => n.to_string(),
            Err(_) => word.to_lowercase(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every recorded hunt, replayed at each of a few report lags: what the
/// kills are given now beside what they were given then, and no kill given
/// a creature no look read before its death.
#[test]
fn recorded_hunts_give_no_kill_a_creature_unread_before_its_death() {
    for (name, record) in records() {
        let profile = profile_for(&record);
        let p = &profile.mob_history;
        for lag_ms in [100, 500] {
            let given = replay(&record, lag_ms);
            let mut table: BTreeMap<&'static str, usize> = BTreeMap::new();
            let mut same_outcome = 0;
            let mut changed: Vec<String> = Vec::new();
            let mut newly: Vec<String> = Vec::new();
            let mut moved: BTreeMap<(String, &'static str), usize> = BTreeMap::new();
            for (kill, now) in record.kills.iter().zip(&given) {
                *table.entry(now.grade).or_default() += 1;
                let then_grade = match (kill.route.as_deref(), kill.screen.as_deref()) {
                    (Some("continuity" | "provisional_continuity"), _) => "continuity",
                    (_, Some(screen)) => screen,
                    _ => "untraced",
                };
                if then_grade == now.grade {
                    same_outcome += 1;
                } else {
                    *moved
                        .entry((then_grade.to_string(), now.grade))
                        .or_default() += 1;
                }
                let then_creature = kill
                    .suggested
                    .as_deref()
                    .or(kill.mob.as_deref().filter(|m| *m != "Unknown"));
                if let (None, Some(now_creature)) = (then_creature, &now.creature) {
                    newly.push(format!(
                        "kill at {}: {now_creature} ({})",
                        kill.logged_ms, now.grade
                    ));
                }
                if let (Some(then), Some(now_creature)) = (then_creature, &now.creature) {
                    if key(then) != key(now_creature) {
                        changed.push(format!(
                            "kill at {}: {then} then, {now_creature} now ({})",
                            kill.logged_ms, now.grade
                        ));
                    }
                }
                // The honest bound: a creature given was read by a look in
                // the kill's moments, up to the latest its death can lie.
                if let Some(creature) = &now.creature {
                    if !matches!(now.grade, "continuity") {
                        let latest_death =
                            kill.logged_ms + p.log_resolution_ms + p.capture_slack_ms;
                        let read = names_read(
                            &record,
                            kill.logged_ms - p.kill_lookback_ms - p.join_gap_ms,
                            latest_death,
                        );
                        assert!(
                            read.iter().any(|r| key(r) == key(creature)),
                            "{name}, lag {lag_ms}: the kill at {} is given {creature} ({}), \
                             which no look read before it: {read:?}",
                            kill.logged_ms,
                            now.grade
                        );
                    }
                }
            }
            eprintln!(
                "{name}, lag {lag_ms} ms: {} kills, {same_outcome} answered as then; now {table:?}",
                record.kills.len()
            );
            eprintln!("  moved: {moved:?}");
            for line in &newly {
                eprintln!("  given a creature it had none of: {line}");
            }
            for line in &changed {
                eprintln!("  changed creature: {line}");
            }
        }
    }
}
