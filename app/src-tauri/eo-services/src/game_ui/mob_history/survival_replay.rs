//! A replay of an event survival instance at the level of the locked
//! plate's readings: creatures graded by the instance's difficulty
//! (`Survival Zombie Dog 04`), most of them on the bundled list only at
//! other difficulties, swarming the player together. An opening of one
//! creature, then a swarm of four alike, then a boss; kills two or three
//! shots long, so about one in five dies between looks. It holds the
//! readers to what such a hunt needs: every creature read named at its
//! difficulty, no kill given a creature it was not, and the creature being
//! hunted carried onto kills only while the hunt is of that one creature.
//! A creature on no list (the bundle's or the player's) is never taken for
//! another; kept by the player, it is named.

use super::*;
use crate::game_ui::mob_names::KeptCreature;
use crate::game_ui::test_support::{bundled_mobs, bundled_profile};

/// The creatures of the instance at difficulty 4, as plates print them,
/// and the level the instance gives them.
const DOG: &str = "Survival Zombie Dog 04";
const ZOMBIE: &str = "Survival Zombie 04";
const RAGE: &str = "Survival Rage Zombie 04";
const ACID: &str = "Survival Acid Zombie 04";
const BOSS: &str = "Kal-Rahm The Cannibal S04";
/// On no list at all: an event creature the bundle never saw.
const HOUND: &str = "Survival Zombie Hound 04";
const LEVEL: u32 = 8;

/// A small deterministic generator: the replay is the same every run.
struct Dice(u64);

impl Dice {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    fn pick<'a>(&mut self, of: &[&'a str]) -> &'a str {
        of[((self.unit() * of.len() as f64) as usize).min(of.len() - 1)]
    }
}

/// One kill: the creature, when its lock began and when it died, and when
/// the log stamped and the application read its line.
struct Kill {
    creature: &'static str,
    locked: f64,
    died: f64,
    report: KillReport,
}

/// The instance: `opening` kills of dogs, then `swarm` kills half dogs and
/// half zombies (the hound among them when `hound`), then the boss.
fn instance(dice: &mut Dice, opening: usize, swarm: usize, hound: bool) -> Vec<Kill> {
    let mut zombies = vec![ZOMBIE, RAGE, ACID];
    if hound {
        zombies.push(HOUND);
    }
    let mut kills = Vec::new();
    let mut t = 3.0;
    for k in 0..opening + swarm + 1 {
        let creature = if k == opening + swarm {
            BOSS
        } else if k < opening || dice.unit() < 0.5 {
            DOG
        } else {
            dice.pick(&zombies)
        };
        // A new lock a moment after the last death; two or three shots,
        // a fifth of kills over between two looks; the boss takes a while.
        t += dice.between(0.3, 2.5);
        let fight = if creature == BOSS {
            dice.between(9.0, 12.0)
        } else if dice.unit() < 0.2 {
            dice.between(0.2, 0.8)
        } else {
            dice.between(1.2, 4.5)
        };
        let locked = t;
        let died = t + fight;
        // The log stamps the line to the second it was written, a moment
        // after the death; the application reads it a moment later still,
        // never before the death.
        let written = died + dice.between(0.1, 1.2);
        let logged = written.floor();
        let arrived = written + dice.between(0.02, 0.4);
        kills.push(Kill {
            creature,
            locked,
            died,
            report: KillReport {
                logged_ms: (logged * 1000.0) as i64,
                arrived_ms: (arrived * 1000.0) as i64,
            },
        });
        t = died;
    }
    kills
}

/// How often a plate's name does not read at all, as the replay has it by
/// default, and as play measured it in such a swarm (89 of 419 readings).
const UNREAD: f64 = 0.05;
const UNREAD_IN_PLAY: f64 = 0.21;

/// The name as the recogniser reads it: whole most of the time, a letter
/// off now and then, and not at all `unread` of the time.
fn read(name: &str, dice: &mut Dice, unread: f64) -> Option<String> {
    let u = dice.unit();
    if u < unread {
        return None;
    }
    if u < unread + 0.1 {
        let mut chars: Vec<char> = name.chars().collect();
        let letters: Vec<usize> = (0..chars.len())
            .filter(|i| chars[*i].is_alphabetic())
            .collect();
        let at = letters[((dice.unit() * letters.len() as f64) as usize).min(letters.len() - 1)];
        chars[at] = if chars[at] == 'o' { 'c' } else { 'o' };
        return Some(chars.into_iter().collect());
    }
    Some(name.to_string())
}

/// What the replay made of the kills.
#[derive(Default, Debug)]
struct Outcome {
    right: usize,
    /// At the top grade, which stamps a kill as settled.
    named_wrong: usize,
    wrong: Vec<String>,
    /// Right, by the creature being hunted.
    carried_right: usize,
    carried_wrong: usize,
    /// Each kill given the creature being hunted: when, what it was, and why.
    carried: Vec<String>,
    unnamed: usize,
    /// The lowest grade, a suggestion for the review that stamps nothing:
    /// right, and wrong.
    suggested_right: usize,
    suggested_wrong: Vec<String>,
    /// Kills of each creature named right (by name), and the kills of it.
    named: std::collections::BTreeMap<&'static str, (usize, usize)>,
    /// Kills given the creature being hunted in the opening, and after a
    /// plate of another creature was first read.
    carried_opening: usize,
    carried_swarm: usize,
}

/// Replay the instance against the bundled list and `kept`.
fn replay(seed: u64, kept: Vec<KeptCreature>, hound: bool) -> Outcome {
    replay_reading(seed, kept, hound, UNREAD)
}

/// The same, a plate's name not reading at all `unread` of the time.
fn replay_reading(seed: u64, kept: Vec<KeptCreature>, hound: bool, unread: f64) -> Outcome {
    let profile = bundled_profile();
    let catalogue = MobCatalogue::from_entities(&bundled_mobs(), kept, &profile.mob_names);
    let mut dice = Dice(seed);
    let opening = 40;
    let kills = instance(&mut dice, opening, 160, hound);
    let mut history = MobHistory::new(profile.mob_history.clone(), profile.mob_names.clone());
    let p = &profile.mob_history;
    let wait = p.capture_slack_ms + p.kill_lookahead_ms;
    let end = kills.last().map_or(0.0, |k| k.died) + 10.0;
    let mut stamps: Vec<(usize, KillStamp, Option<KillTrace>)> = Vec::new();
    let mut next = 0;
    // When a plate of a creature other than the opening's first read.
    let mut other_read_ms: Option<i64> = None;
    let phase = dice.unit();
    let mut i = 0;
    loop {
        let t = phase + i as f64 + dice.between(-0.04, 0.04);
        if t > end {
            break;
        }
        let at_ms = (t * 1000.0).round() as i64;
        let lock = kills.iter().find(|k| k.locked <= t && t < k.died);
        let sighting = lock.map(|k| {
            let fill = 1.0 - (t - k.locked) / (k.died - k.locked);
            let raw = read(k.creature, &mut dice, unread);
            let resolved = match &raw {
                Some(text) => resolve(text, Some(LEVEL), &catalogue, &profile.mob_names),
                None => Resolved::Unclear,
            };
            Sighting::resolved((fill * 20.0).round() / 20.0, raw, resolved, &catalogue)
        });
        if lock.is_some_and(|k| k.creature != DOG)
            && sighting.as_ref().is_some_and(|s| s.raw_name.is_some())
        {
            other_read_ms.get_or_insert(at_ms);
        }
        history.observe(at_ms, sighting);
        while next < kills.len() && at_ms >= kills[next].report.arrived_ms + wait {
            let stamp = history.stamp_for_kill(kills[next].report, at_ms);
            assert_ne!(
                stamp,
                KillStamp::NotYet,
                "kill {next} watched far enough past"
            );
            stamps.push((next, stamp, history.take_trace()));
            next += 1;
        }
        i += 1;
    }
    let mut out = Outcome::default();
    for (k, stamp, trace) in stamps {
        let truth = kills[k].creature;
        let entry = out.named.entry(truth).or_default();
        entry.1 += 1;
        let suggestion = match &stamp {
            KillStamp::Stamped(MobStamp::LatestRead { creature, .. }) => Some(shown(creature)),
            KillStamp::Stamped(MobStamp::ProvisionalLatestRead { spelling, .. }) => {
                Some(spelling.clone())
            }
            _ => None,
        };
        if let Some(suggested) = suggestion {
            if label_key(&suggested) == label_key(truth) {
                out.suggested_right += 1;
            } else {
                out.suggested_wrong.push(format!("{truth} as {suggested}"));
            }
            out.unnamed += 1;
            continue;
        }
        let (named, carried) = match &stamp {
            KillStamp::Stamped(
                MobStamp::Named { creature, .. } | MobStamp::Guess { creature, .. },
            ) => (Some(shown(creature)), false),
            KillStamp::Stamped(MobStamp::Continuity { creature, .. }) => {
                (Some(shown(creature)), true)
            }
            KillStamp::Stamped(
                MobStamp::Provisional { spelling, .. }
                | MobStamp::ProvisionalGuess { spelling, .. },
            ) => (Some(spelling.clone()), false),
            KillStamp::Stamped(MobStamp::ProvisionalContinuity { spelling, .. }) => {
                (Some(spelling.clone()), true)
            }
            _ => (None, false),
        };
        let Some(named) = named else {
            out.unnamed += 1;
            continue;
        };
        let right = label_key(&named) == label_key(truth);
        match (right, carried) {
            (true, false) => {
                out.right += 1;
                entry.0 += 1;
            }
            (true, true) => out.carried_right += 1,
            (false, true) => out.carried_wrong += 1,
            (false, false) => {
                if matches!(
                    stamp,
                    KillStamp::Stamped(MobStamp::Named { .. } | MobStamp::Provisional { .. })
                ) {
                    out.named_wrong += 1;
                }
                out.wrong.push(format!(
                    "kill {k} at {} s, {truth} as {named}: reads {:?}",
                    kills[k].report.logged_ms / 1000,
                    trace.as_ref().map(|t| t
                        .reads
                        .iter()
                        .map(|r| (r.at_ms, r.named.clone()))
                        .collect::<Vec<_>>())
                ))
            }
        }
        if carried {
            let t = kills[k].report.logged_ms / 1000;
            out.carried.push(format!(
                "kill {k} at {t} s, {truth} as {named}; reads {:?}",
                trace.map(|t| t
                    .reads
                    .iter()
                    .map(|r| (r.at_ms, r.named.clone()))
                    .collect::<Vec<_>>())
            ));
            if other_read_ms.is_none_or(|t| kills[k].report.logged_ms < t) {
                out.carried_opening += 1;
            } else {
                out.carried_swarm += 1;
            }
        }
    }
    out
}

fn kills_of(out: &Outcome) -> usize {
    out.named.values().map(|(_, of)| *of).sum()
}

/// A known creature as its plate prints it, padded as the instance pads it.
fn shown(creature: &Creature) -> String {
    match creature {
        Creature::Catalogued { name, maturity, .. } | Creature::Species { name, maturity, .. } => {
            match maturity.as_deref() {
                Some(m) if m.chars().all(|c| c.is_ascii_digit()) => format!("{name} {m:0>2}"),
                Some(m) => format!("{name} {m}"),
                None => name.clone(),
            }
        }
        Creature::Confirmed { name } => name.clone(),
    }
}

/// Every creature the bundle has at some difficulty is named at the one
/// printed, none is taken for another, and the creature being hunted is
/// carried only while the hunt is of it alone.
#[test]
fn a_survival_instance_names_each_creature_at_its_difficulty() {
    for seed in [1, 2, 3, 4, 5] {
        let out = replay(seed, Vec::new(), false);
        eprintln!("seed {seed}: {out:#?}");
        // Never wrong at the top grade; a guess, rarely, where the screen
        // cannot tell (a fresh lock read once in the moment the report
        // arrived, then gone).
        assert_eq!(out.named_wrong, 0, "seed {seed}: {:#?}", out.wrong);
        assert!(
            out.wrong.len() * 100 <= kills_of(&out),
            "seed {seed}: {:#?}",
            out.wrong
        );
        // Only before the first plate of another creature is read can a kill
        // of it be taken for the creature being hunted: the screen has
        // shown nothing of it yet.
        for creature in [DOG, ZOMBIE, RAGE, ACID] {
            let (right, of) = out.named[creature];
            assert!(
                right * 2 >= of,
                "seed {seed}: {creature} named {right} of {of}"
            );
        }
        assert_eq!(out.named[BOSS], (1, 1), "seed {seed}: the boss");
        assert_eq!(
            out.carried_swarm, 0,
            "seed {seed}: carried once the hunt was mixed"
        );
    }
}

/// A creature on no list in the swarm is never given another's name: it
/// goes unnamed, or under its own spelling. Kept by the player (at any
/// difficulty), it is named like the bundle's.
#[test]
fn a_creature_on_no_list_is_never_taken_for_another_and_is_named_once_kept() {
    let kept = || {
        vec![KeptCreature {
            name: "Survival Zombie Hound".into(),
            species: None,
            maturity: Some("01".into()),
        }]
    };
    for seed in [11, 12, 13] {
        let undeclared = replay(seed, Vec::new(), true);
        eprintln!("undeclared, seed {seed}: {undeclared:#?}");
        assert_eq!(
            undeclared.named_wrong, 0,
            "seed {seed}: {:#?}",
            undeclared.wrong
        );
        assert!(
            undeclared.wrong.len() * 100 <= kills_of(&undeclared),
            "seed {seed}: {:#?}",
            undeclared.wrong
        );
        assert_eq!(
            undeclared.carried_swarm, 0,
            "seed {seed}: carried once mixed"
        );
        let declared = replay(seed, kept(), true);
        eprintln!("declared, seed {seed}: {declared:#?}");
        assert_eq!(
            declared.named_wrong, 0,
            "seed {seed}: {:#?}",
            declared.wrong
        );
        assert!(
            declared.wrong.len() * 100 <= kills_of(&declared),
            "seed {seed}: {:#?}",
            declared.wrong
        );
        let (right, of) = declared.named[HOUND];
        assert!(
            right * 2 >= of,
            "seed {seed}: the hound named {right} of {of}"
        );
    }
}

/// The swarm read as play read it, one plate in five giving no name at
/// all: a plate whose name never read is only a doubt where every name read
/// round the kill was one creature's, so kills still take their creature,
/// and none is taken for another at the top grade.
#[test]
fn a_swarm_whose_plates_often_do_not_read_still_names_its_kills() {
    for seed in [1, 2, 3, 4, 5] {
        let out = replay_reading(seed, Vec::new(), false, UNREAD_IN_PLAY);
        let kills = kills_of(&out);
        eprintln!(
            "seed {seed}: {} right, {} carried, {} unnamed ({} of them suggested right, {} wrong), {} wrong of {kills}",
            out.right,
            out.carried_right,
            out.unnamed,
            out.suggested_right,
            out.suggested_wrong.len(),
            out.wrong.len()
        );
        assert_eq!(out.named_wrong, 0, "seed {seed}: {:#?}", out.wrong);
        assert!(
            out.wrong.len() * 100 <= kills * 2,
            "seed {seed}: {:#?}",
            out.wrong
        );
        assert!(
            (out.right + out.carried_right) * 100 >= kills * 60,
            "seed {seed}: {} of {kills} named",
            out.right + out.carried_right
        );
        assert_eq!(
            out.carried_swarm, 0,
            "seed {seed}: carried once the hunt was mixed"
        );
        // A lock held from a name read to the death gives the unnamed
        // kills a suggestion, and the suggestions stay right.
        assert!(
            out.suggested_right * 100 >= kills * 9,
            "seed {seed}: {} of {kills} suggested right",
            out.suggested_right
        );
        assert!(
            out.suggested_wrong.len() * 100 <= kills * 2,
            "seed {seed}: {:#?}",
            out.suggested_wrong
        );
    }
}
