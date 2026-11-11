//! Which creature each kill was, from the locked plate read over time.
//!
//! A watcher reads the locked plate about once a second while the player
//! hunts, and hands each reading here with the time it was read; nothing
//! here reads a clock. Readings group into fights: readings close in time
//! whose health does not climb back up, of one creature. A creature's
//! health only falls while it is fought, so a reading that shows more
//! health than a fight had is another creature: the player locked a fresh
//! one, or briefly locked another mid-fight. A reading that names a known
//! creature strictly joins only a fight that names the same one, the one
//! its health continues best, so returning from a brief lock carries on the
//! fight it left. Any other reading (unclear, unfamiliar, or a known name
//! read only loosely) joins only the damaged plate read on the pulse just
//! before, its health going on from it; a second creature in a fight shows
//! in its vote, which then names nothing.
//!
//! The chat log reports a kill up to a couple of seconds after the
//! creature died, stamped to the whole second; the application also knows
//! when it read the line, and the line cannot have arrived before the
//! death. What the screen shows of a death is that the creature's plate
//! stops being read, so a kill is answered only once the screen has been
//! watched for a while after the report arrived. A creature read after the
//! report arrived was alive after it, so it was not this kill. Every other
//! fight read in the moments the death can lie in is gathered, a fight
//! whose readings decide nothing counting as a creature of its own, and a
//! creature is given only when all of them agree on one: the kill goes to
//! that creature's fight that stopped being read nearest the death. When
//! they name more than one creature the answer is no creature, or at most
//! a suggestion; the only fight left out is one the player had clearly
//! left, another plate having been read for a while before the kill can
//! have happened (and one left damaged may have bled out, so it keeps the
//! answer to a guess). A creature still read after the kill is alive, so a
//! kill whose only creature is still being read was something else's (area
//! damage). Health never chooses: a creature killed in one or two shots
//! stops being read at full health.
//!
//! The fight's creature is decided by vote over its readings, since single
//! readings of a name are often a character or two off. A creature is
//! named only on two strict readings of its own, the fight's last among
//! them. An unfamiliar name joins the session's other spellings of the same
//! creature: a spelling read for the first time is taken as an earlier one
//! close to it read at the same level, never across levels or with a level
//! unread, and a name whose level never read is only ever suggested. A
//! creature that was not read is never named, and a split vote names
//! nothing.
//!
//! A kill is given one of four grades. [`MobStamp::Named`]: a creature on
//! the list, read strictly at least twice in its own fight, its reading the
//! last one before the death; safe to record. [`MobStamp::Guess`]: a
//! creature on the list the screen leans to without proving it; to be shown
//! as a suggestion. [`MobStamp::Provisional`] and
//! [`MobStamp::ProvisionalGuess`]: the same two grades for a creature on no
//! list, under the session's name for it. [`MobStamp::LatestRead`] and
//! [`MobStamp::ProvisionalLatestRead`], the lowest grade: where the creatures
//! read in the death's moments leave the screen without a lean, the creature
//! read last (in a fight read at least twice, its own readings deciding it);
//! to be recorded as such and corrected where it was not the one. Anything
//! else is [`KillStamp::Unnamed`], with the reason.
//!
//! Only what a kill could still need is kept: fights end their life a
//! fixed time after their last reading, there are at most so many, a fight
//! keeps only its latest readings, and the session's spellings are capped.

use std::collections::VecDeque;

use serde::Deserialize;

use crate::fuzzy_match::ratio;

use super::field::label_key;
use super::mob_names::{plate_strings, resolve, MobCatalogue, MobNameProfile, Resolved, Spelling};
use super::plates::PlateRead;

/// How readings are grouped into fights, kills given fights, and fights
/// voted on, as the profile describes it. The values assume the watcher
/// reads the locked plate about once a second.
#[derive(Debug, Clone, Deserialize)]
pub struct MobHistoryProfile {
    /// How often the watcher reads the locked plate: the cadence every
    /// span below is measured against.
    pub pulse_ms: i64,
    /// A reading joins a fight read at most this long before it.
    pub join_gap_ms: i64,
    /// A reading may show this much more health than its fight's last
    /// (the bar's ends are measured to a column) and still continue it.
    pub rise_tolerance: f64,
    /// The watcher reads at least this often: a reading interval and one
    /// reading missed. A longer gap between pulses round a kill means the
    /// screen was not watched then.
    pub continue_ms: i64,
    /// Health below this is damage: a plate showing it says which
    /// creature it is (two fresh creatures both show full health).
    pub damaged_fill: f64,
    /// Consecutive pulses whose locked plate shows damage, the later no
    /// more than this above the earlier
    pub same_plate_rise: f64,
    /// and at most this much lower per second, show the same creature,
    /// whatever its name read as.
    pub same_plate_drop_per_s: f64,
    /// A kill is given a fight last read from this long before its logged
    /// second: the earliest death the report can follow, less a reading
    /// interval and one reading missed.
    pub kill_lookback_ms: i64,
    /// A kill is answered only once the screen has been read this long
    /// after the report arrived (and the capture slack): long enough for a
    /// living creature to have been read again, with one reading missed.
    pub kill_lookahead_ms: i64,
    /// The log stamps its lines to this resolution: a death lies before
    /// its line's stamp plus this.
    pub log_resolution_ms: i64,
    /// How long a pulse's capture can lag its time stamp: a reading later
    /// than a report's arrival by more than this is of a creature alive
    /// after that report.
    pub capture_slack_ms: i64,
    /// The longest the chat log takes to report a death: a fight read
    /// since before the report's time less this was locked while the kill
    /// happened.
    pub report_delay_ms: i64,
    /// A report is believed only when it arrived no more than this before
    /// its logged second (the two clocks' disagreement)
    pub report_early_ms: i64,
    /// and no more than the log's resolution, the longest report delay and
    /// this after it (the application's own delay in reading the line).
    pub report_slack_ms: i64,
    /// A fight the player had clearly left is one after whose last reading
    /// another plate was read over at least this span, ending before the
    /// earliest moment the kill can have happened.
    pub left_span_ms: i64,
    /// A kill not yet watched far enough past is given up on, as not
    /// watched, once the latest pulse or the caller's clock is this far
    /// past the time it needs.
    pub give_up_ms: i64,
    /// At most this many pulses are kept (the last `retain_ms` of them).
    pub max_pulses: usize,
    /// Fewest readings that must vote for a creature to be named.
    pub min_votes: usize,
    /// An unfamiliar name is given to a kill only when at least this many
    /// of the fight's own readings vote for it: a single garbled reading is
    /// not a creature.
    pub provisional_reads: usize,
    /// The creature named must have at least this share of the votes,
    /// and more than any other.
    pub vote_share: f64,
    /// A spelling of an unfamiliar name read for the first time is taken
    /// as an earlier one at least this similar (0 to 100), at the same
    /// level.
    pub consolidate_score: f64,
    /// A fight is forgotten this long after its last reading.
    pub retain_ms: i64,
    /// A fight keeps at most this many of its latest readings.
    pub max_reads: usize,
    /// At most this many fights are kept. A fight a report could still
    /// need is forgotten only when no other can be, and kills whose moments
    /// it was read in then answer as not watched.
    pub max_fights: usize,
    /// At most this many unfamiliar names no kill has been given are
    /// remembered,
    pub spellings: usize,
    /// and this many that a kill has (their keys stand in stamps),
    pub stamped_spellings: usize,
    /// each with at most this many of its spellings and of the texts read
    /// with them, the most read kept.
    pub spelling_texts: usize,
}

/// One reading of the locked plate, its name resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Sighting {
    /// The creature's health, 0 to 1.
    pub fill: f64,
    /// The name as the recogniser read it, when it read.
    pub raw_name: Option<String>,
    pub resolved: Resolved,
}

impl Sighting {
    /// `read`'s name resolved against `catalogue`, with its level when
    /// the level read; a name that did not read resolves as unclear.
    pub fn of(read: &PlateRead, catalogue: &MobCatalogue, names: &MobNameProfile) -> Self {
        let resolved = match &read.name {
            Ok(raw) => resolve(raw, read.level.as_ref().ok().copied(), catalogue, names),
            Err(_) => Resolved::Unclear,
        };
        Sighting {
            fill: read.fill,
            raw_name: read.name.as_ref().ok().cloned(),
            resolved,
        }
    }
}

/// A creature the application knows by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Creature {
    /// One on the bundled creature list.
    Catalogued {
        name: String,
        species: Option<String>,
        maturity: Option<String>,
    },
    /// A name the player has confirmed.
    Confirmed { name: String },
    /// A species at a maturity, among creatures the bundled list tells
    /// apart only by planet (see [`Resolved::Species`]).
    Species {
        name: String,
        species: String,
        maturity: Option<String>,
    },
}

/// What a stamp rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    /// The name as the recogniser most often read it among the votes.
    pub raw_text: String,
    /// When the fight's first and last readings were taken.
    pub first_read_ms: i64,
    pub last_read_ms: i64,
    /// The votes for the creature, and all the votes cast.
    pub votes: usize,
    pub of: usize,
}

/// The creature a kill was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobStamp {
    /// A creature the application knows, which at least one of the fight's
    /// own readings spelled whole.
    Named {
        creature: Creature,
        evidence: Evidence,
    },
    /// A creature the application knows, read only in near misses: a guess,
    /// to be shown as one and never kept unchecked.
    Guess {
        creature: Creature,
        evidence: Evidence,
    },
    /// A creature the application does not know, by the session's name for
    /// it: `key` stays the same all session and no two names share one (it
    /// carries the level read with the name), and the spelling it stands
    /// for is the one most often read (see [`MobHistory::provisional_names`]).
    /// Only a name whose level was read is given as one.
    Provisional {
        key: String,
        spelling: String,
        evidence: Evidence,
    },
    /// A creature the application does not know, suggested for the kill
    /// but not proved by the screen (the unfamiliar counterpart of
    /// [`MobStamp::Guess`]): shown for the player to confirm, never kept
    /// unchecked.
    ProvisionalGuess {
        key: String,
        spelling: String,
        evidence: Evidence,
    },
    /// The lowest grade: a creature the application knows, read strictly
    /// in the fight read last before the death, where other creatures read
    /// in the death's moments leave the screen without a lean. To be shown
    /// as the latest creature read, and corrected where it was not the one.
    LatestRead {
        creature: Creature,
        evidence: Evidence,
    },
    /// The lowest grade for a creature the application does not know (the
    /// unfamiliar counterpart of [`MobStamp::LatestRead`]).
    ProvisionalLatestRead {
        key: String,
        spelling: String,
        evidence: Evidence,
    },
}

impl MobStamp {
    pub fn evidence(&self) -> &Evidence {
        match self {
            MobStamp::Named { evidence, .. }
            | MobStamp::Guess { evidence, .. }
            | MobStamp::Provisional { evidence, .. }
            | MobStamp::ProvisionalGuess { evidence, .. }
            | MobStamp::LatestRead { evidence, .. }
            | MobStamp::ProvisionalLatestRead { evidence, .. } => evidence,
        }
    }
}

/// Why a kill was given no creature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unnamed {
    /// No fight stopped being read shortly before the kill.
    NoFight,
    /// The player was locked on a creature that lived on through the
    /// kill: the kill was something else's.
    StillRead,
    /// Fights of different creatures stopped too close together to say
    /// which was killed.
    Indistinct,
    /// The fight's readings do not agree on a creature.
    Undecided,
    /// The screen was not watched all through the kill's moments: the
    /// watcher stalled or stopped, or more was read than could be kept.
    Unwatched,
    /// The report's arrival is too far from its logged second for either
    /// to be believed.
    Implausible,
}

/// A kill as the chat log reported it: the line's stamp (the second it
/// was written, in milliseconds) and when the application read the line,
/// on the clock the pulses are stamped with. A report never arrives before
/// the death it reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KillReport {
    pub logged_ms: i64,
    pub arrived_ms: i64,
}

/// The answer for one kill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillStamp {
    /// The screen has not been watched long enough after the kill to tell;
    /// ask again after more pulses.
    NotYet,
    /// No creature can be given to the kill; the caller falls back to its
    /// own default.
    Unnamed(Unnamed),
    Stamped(MobStamp),
}

/// A creature a reading votes for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Vote {
    Known(Creature),
    /// An unfamiliar name, by the session's number for it.
    New(usize),
}

struct Reading {
    at_ms: i64,
    fill: f64,
    raw: Option<String>,
    /// What it names, if anything clear at the strict floor.
    vote: Option<Vote>,
    /// The known creature it names only loosely (see
    /// [`Spelling::Loose`]): it takes no part in grouping readings or in a
    /// named stamp, only in a guess.
    loose: Option<Creature>,
    /// Whether it spelled a known name whole.
    spelled: bool,
    /// The reading's text as a key, for comparing with names.
    key: Vec<char>,
}

struct Fight {
    readings: VecDeque<Reading>,
    /// A kill has been named, or given a provisional name, from this
    /// fight: no later kill can have it.
    claimed: bool,
    /// The grade a kill was given this fight's creature at, when that was a
    /// weak one: the fight stays in contention for later kills, which it
    /// can give no more than that grade.
    given: Option<Given>,
    /// What its readings name, with how many name it.
    tally: Vec<(Vote, usize)>,
}

impl Fight {
    fn last(&self) -> &Reading {
        self.readings.back().expect("a fight has a reading")
    }

    fn first_ms(&self) -> i64 {
        self.readings.front().map_or(0, |r| r.at_ms)
    }

    fn count(&mut self, vote: &Vote, up: bool) {
        match self.tally.iter_mut().position(|(v, _)| v == vote) {
            Some(i) if up => self.tally[i].1 += 1,
            Some(i) => {
                self.tally[i].1 -= 1;
                if self.tally[i].1 == 0 {
                    self.tally.remove(i);
                }
            }
            None if up => self.tally.push((vote.clone(), 1)),
            None => {}
        }
    }

    /// What most of its readings name, when one thing has more than any
    /// other.
    fn leader(&self) -> Option<&Vote> {
        let most = self.tally.iter().map(|t| t.1).max()?;
        let mut at_most = self.tally.iter().filter(|t| t.1 == most);
        let first = at_most.next()?;
        at_most.next().is_none().then_some(&first.0)
    }

    fn push(&mut self, reading: Reading, max: usize) {
        if let Some(vote) = &reading.vote {
            self.count(&vote.clone(), true);
        }
        self.readings.push_back(reading);
        while self.readings.len() > max.max(1) {
            if let Some(vote) = self.readings.pop_front().and_then(|r| r.vote) {
                self.count(&vote, false);
            }
        }
    }
}

/// An unfamiliar name seen this session.
struct Unfamiliar {
    id: usize,
    /// The key of its first spelling.
    key: Vec<char>,
    /// The session's key for it: its first spelling's key, the level read
    /// with it and its number, so no two names share one.
    name: String,
    /// The level read with it, when one was; fixed when it is first read.
    level: Option<u32>,
    /// Its spellings, by key, and the texts read with them, with counts.
    spellings: Vec<(Vec<char>, Kept)>,
    texts: Vec<(String, Kept)>,
    count: usize,
    /// A kill has been given it: its key stands in a stamp.
    stamped: bool,
}

impl Unfamiliar {
    /// The text most often read for it, by the reads counted for each text
    /// kept. Among texts read equally often, the one nearest the others
    /// read (each weighed by its reads): misreadings each stray from the
    /// name a character or two, and further from one another.
    fn spelling(&self) -> String {
        let most = self.texts.iter().map(|(_, n)| n.read()).max();
        let central = |text: &str| -> f64 {
            let key = label_key(text);
            self.texts
                .iter()
                .filter(|(other, _)| other != text)
                .map(|(other, n)| ratio(&key, &label_key(other)) * n.read() as f64)
                .sum()
        };
        let mut best: Option<(&String, f64)> = None;
        for (text, n) in &self.texts {
            if Some(n.read()) != most {
                continue;
            }
            let near = central(text);
            if best.is_none_or(|(_, b)| near > b) {
                best = Some((text, near));
            }
        }
        best.map(|(text, _)| text.clone())
            .unwrap_or_else(|| self.key.iter().collect())
    }
}

/// The readings of the locked plate, and the fights they make.
pub struct MobHistory {
    profile: MobHistoryProfile,
    names: MobNameProfile,
    fights: Vec<Fight>,
    unfamiliar: Vec<Unfamiliar>,
    next_id: usize,
    /// When each pulse came, oldest first.
    pulses: VecDeque<i64>,
    /// The latest pulse with no plate locked.
    last_empty: Option<i64>,
    /// When the readings of fights forgotten while a report could still
    /// need them were taken: a kill read then is not answered.
    lost: VecDeque<(i64, i64)>,
}

impl MobHistory {
    /// A history voting by `profile`, folding near spellings of a known
    /// creature onto it as strictly as `names` matches a name alone.
    pub fn new(profile: MobHistoryProfile, names: MobNameProfile) -> Self {
        Self {
            profile,
            names,
            fights: Vec::new(),
            unfamiliar: Vec::new(),
            next_id: 0,
            pulses: VecDeque::new(),
            last_empty: None,
            lost: VecDeque::new(),
        }
    }

    /// Record one pulse at `at_ms`: the locked plate read, or `None` when
    /// no plate was locked. The watcher calls this on every pulse, whether
    /// or not a plate was locked, in time order; a pulse earlier than the
    /// last is ignored.
    pub fn observe(&mut self, at_ms: i64, sighting: Option<Sighting>) {
        if self.pulses.back().is_some_and(|last| at_ms < *last) {
            return;
        }
        let retain = self.profile.retain_ms;
        self.pulses.push_back(at_ms);
        while self
            .pulses
            .front()
            .is_some_and(|first| at_ms - first > retain)
            || self.pulses.len() > self.profile.max_pulses.max(1)
        {
            self.pulses.pop_front();
        }
        self.fights.retain(|f| at_ms - f.last().at_ms <= retain);
        while self.lost.front().is_some_and(|(_, to)| at_ms - to > retain) {
            self.lost.pop_front();
        }
        let Some(sighting) = sighting else {
            self.last_empty = Some(at_ms);
            return;
        };
        if !sighting.fill.is_finite() {
            return;
        }
        let known = |creature: Creature, spelling: &Spelling| match spelling {
            Spelling::Loose => (None, Some(creature), false),
            Spelling::Exact => (Some(Vote::Known(creature)), None, true),
            Spelling::Near => (Some(Vote::Known(creature)), None, false),
        };
        let (vote, loose, spelled) = match &sighting.resolved {
            Resolved::Known {
                name,
                species,
                maturity,
                spelling,
            } => known(
                Creature::Catalogued {
                    name: name.clone(),
                    species: species.clone(),
                    maturity: maturity.clone(),
                },
                spelling,
            ),
            Resolved::Confirmed { name, spelling } => {
                known(Creature::Confirmed { name: name.clone() }, spelling)
            }
            Resolved::Species {
                name,
                species,
                maturity,
                spelling,
            } => known(
                Creature::Species {
                    name: name.clone(),
                    species: species.clone(),
                    maturity: maturity.clone(),
                },
                spelling,
            ),
            Resolved::New { text, level } => {
                (Some(Vote::New(self.identify(text, *level))), None, false)
            }
            Resolved::Unclear => (None, None, false),
        };
        let p = &self.profile;
        let fill = sighting.fill;
        // Only one plate is locked at a time: a damaged plate read on the
        // pulse right after a damaged plate, its health going on from it, is
        // the same creature. A reading with no known name read strictly
        // (unclear, unfamiliar, or a known name only loosely) joins a fight
        // that way only: a second creature in the fight shows in its vote.
        // Health that is full on either side is a fresh lock as much as the
        // same creature.
        let strict = matches!(vote, Some(Vote::Known(_)));
        let previous = self.pulses.iter().rev().nth(1).copied();
        let same_plate = previous
            .filter(|prev| !strict && at_ms - prev <= p.continue_ms && fill < p.damaged_fill)
            .and_then(|prev| {
                self.fights.iter().position(|f| {
                    let last = f.last();
                    let seconds = (at_ms - prev) as f64 / 1000.0;
                    !f.claimed
                        && last.at_ms == prev
                        && last.fill < p.damaged_fill
                        && fill <= last.fill + p.same_plate_rise
                        && last.fill - fill <= p.same_plate_drop_per_s * seconds
                })
            });
        // A reading that names a known creature strictly joins, of the
        // fights its health can continue (never across a pulse with nothing
        // locked) and that name no other creature more, one whose readings
        // mostly name the same, then the one whose health it continues most
        // closely, then the one read most recently.
        let mut best: Option<(usize, bool, std::cmp::Reverse<u64>, i64)> = None;
        for (i, fight) in self.fights.iter().enumerate() {
            let Some(known @ Vote::Known(_)) = &vote else {
                break;
            };
            let last = fight.last();
            // A pulse with nothing locked since the fight's last reading
            // ended it: whatever is read next is a lock begun afresh.
            if fight.claimed
                || at_ms < last.at_ms
                || at_ms - last.at_ms > p.join_gap_ms
                || fill > last.fill + p.rise_tolerance
                || self.last_empty.is_some_and(|empty| empty > last.at_ms)
            {
                continue;
            }
            // Never a fight that names another known creature, nor one in
            // which anything else has more readings than this creature: a
            // fight led by an unfamiliar name is that creature, and a known
            // one near it is another.
            let own = fight
                .tally
                .iter()
                .find(|(v, _)| v == known)
                .map_or(0, |t| t.1);
            let other_known = fight
                .tally
                .iter()
                .any(|(v, _)| matches!(v, Vote::Known(_)) && v != known);
            if other_known || fight.tally.iter().any(|(_, n)| *n > own) {
                continue;
            }
            let agrees = fight.leader() == Some(known);
            let closeness = std::cmp::Reverse((last.fill - fill).abs().to_bits());
            if best.is_none_or(|(_, a, c, t)| (agrees, closeness, last.at_ms) > (a, c, t)) {
                best = Some((i, agrees, closeness, last.at_ms));
            }
        }
        let reading = Reading {
            at_ms,
            fill,
            key: sighting
                .raw_name
                .as_deref()
                .map(label_key)
                .unwrap_or_default(),
            raw: sighting.raw_name,
            vote,
            loose,
            spelled,
        };
        match same_plate.or(best.map(|b| b.0)) {
            Some(i) => self.fights[i].push(reading, p.max_reads),
            None => {
                let mut fight = Fight {
                    readings: VecDeque::new(),
                    claimed: false,
                    given: None,
                    tally: Vec::new(),
                };
                fight.push(reading, p.max_reads);
                self.fights.push(fight);
            }
        }
        // Past the cap, a fight a kill can no longer have goes first, then
        // one no report can still need, then the oldest; that last is noted,
        // so kills read in its time are not answered without it.
        let horizon = self.horizon_ms();
        while self.fights.len() > p.max_fights.max(1) {
            let needed = |f: &Fight| !f.claimed && at_ms - f.last().at_ms <= horizon;
            let oldest = (0..self.fights.len())
                .min_by_key(|i| {
                    let f = &self.fights[*i];
                    (!f.claimed, needed(f), f.last().at_ms)
                })
                .expect("fights");
            let gone = self.fights.remove(oldest);
            if needed(&gone) {
                self.lost.push_back((gone.first_ms(), gone.last().at_ms));
            }
        }
    }

    /// The session's number for an unfamiliar name read as `text` at
    /// `level`. With its level read: the name at that level it was read as
    /// before; else, for a spelling read for the first time, the one name at
    /// that level close to it; else a new one. With its level unread: the
    /// one name it was read as before, spelled exactly so (among several,
    /// the one whose level is unread too); else a new one. A spelling is
    /// never taken as a close one across levels, nor where a level is
    /// unread: another maturity's name can be as close.
    fn identify(&mut self, text: &str, level: Option<u32>) -> usize {
        let key = label_key(text);
        let spelled = |u: &Unfamiliar| u.spellings.iter().any(|(k, _)| *k == key);
        let index = match level {
            Some(read) => self
                .unfamiliar
                .iter()
                .position(|u| u.level == Some(read) && spelled(u))
                .or_else(|| {
                    let scored: Vec<(usize, f64)> = self
                        .unfamiliar
                        .iter()
                        .enumerate()
                        .filter(|(_, u)| u.level == Some(read))
                        .map(|(i, u)| {
                            let near = u
                                .spellings
                                .iter()
                                .map(|(k, _)| ratio(&key, k))
                                .fold(0.0, f64::max);
                            (i, near)
                        })
                        .filter(|(_, near)| *near >= self.profile.consolidate_score)
                        .collect();
                    let top = scored.iter().map(|s| s.1).fold(f64::MIN, f64::max);
                    match scored
                        .iter()
                        .filter(|s| s.1 >= top)
                        .collect::<Vec<_>>()
                        .as_slice()
                    {
                        [(i, _)] => Some(*i),
                        _ => None,
                    }
                }),
            None => {
                let holding: Vec<usize> = (0..self.unfamiliar.len())
                    .filter(|i| spelled(&self.unfamiliar[*i]))
                    .collect();
                match holding.as_slice() {
                    [only] => Some(*only),
                    _ => holding
                        .iter()
                        .copied()
                        .find(|i| self.unfamiliar[*i].level.is_none()),
                }
            }
        };
        let cap = self.profile.spelling_texts.max(1);
        let index = match index {
            Some(i) => i,
            None => {
                let first: String = key.iter().collect();
                let at = level.map_or("?".to_string(), |l| l.to_string());
                self.unfamiliar.push(Unfamiliar {
                    id: self.next_id,
                    name: format!("{first} L{at} #{}", self.next_id),
                    key: key.clone(),
                    level,
                    spellings: Vec::new(),
                    texts: Vec::new(),
                    count: 0,
                    stamped: false,
                });
                self.next_id += 1;
                self.unfamiliar.len() - 1
            }
        };
        let u = &mut self.unfamiliar[index];
        u.count += 1;
        count_kept(&mut u.spellings, key, cap);
        count_kept(&mut u.texts, text.trim().to_string(), cap);
        let id = u.id;
        self.forget_unfamiliar(id);
        id
    }

    /// Forget unfamiliar names past the caps, the oldest first, never one
    /// a kept fight still names, nor `keep`. Names no kill has been given
    /// go at their cap; names a kill has been given are kept to their own,
    /// larger cap, past which the oldest of them goes too (its key then no
    /// longer appears in [`MobHistory::provisional_names`]).
    fn forget_unfamiliar(&mut self, keep: usize) {
        let named: Vec<usize> = self
            .fights
            .iter()
            .flat_map(|f| f.tally.iter())
            .filter_map(|(v, _)| match v {
                Vote::New(id) => Some(*id),
                Vote::Known(_) => None,
            })
            .collect();
        for (stamped, cap) in [
            (false, self.profile.spellings.max(1)),
            (true, self.profile.stamped_spellings.max(1)),
        ] {
            while self
                .unfamiliar
                .iter()
                .filter(|u| u.stamped == stamped)
                .count()
                > cap
            {
                let Some(oldest) = (0..self.unfamiliar.len())
                    .filter(|i| {
                        let u = &self.unfamiliar[*i];
                        u.stamped == stamped && u.id != keep && !named.contains(&u.id)
                    })
                    .min_by_key(|i| self.unfamiliar[*i].id)
                else {
                    break;
                };
                self.unfamiliar.remove(oldest);
            }
        }
    }

    /// The session's unfamiliar names: each one's key, which stamps carry
    /// and which never changes, with the spelling most often read for it.
    pub fn provisional_names(&self) -> Vec<(String, String)> {
        self.unfamiliar
            .iter()
            .map(|u| (u.name.clone(), u.spelling()))
            .collect()
    }

    /// How long after its last reading a report can still need a fight:
    /// the look-back, a report as late as is believed, the capture slack,
    /// the look-ahead and the give-up margin.
    fn horizon_ms(&self) -> i64 {
        let p = &self.profile;
        p.kill_lookback_ms
            + p.log_resolution_ms
            + p.report_delay_ms
            + p.report_slack_ms
            + p.capture_slack_ms
            + p.kill_lookahead_ms
            + p.give_up_ms
    }

    /// Whether a pulse came at least every `continue_ms` from `from` to
    /// `to`.
    fn watched(&self, from: i64, to: i64) -> bool {
        let gap = self.profile.continue_ms;
        let before = self.pulses.iter().rev().find(|t| **t < from).copied();
        let mut last = before.unwrap_or(from);
        for &t in self.pulses.iter().filter(|t| **t >= from) {
            if t - last > gap {
                return false;
            }
            last = t;
            if t >= to {
                return true;
            }
        }
        false
    }

    /// The report for a kill whose line's arrival the caller does not know:
    /// it is taken to have arrived as late as it could, which keeps every
    /// creature read until then in contention.
    pub fn report_without_arrival(&self, logged_ms: i64) -> KillReport {
        KillReport {
            logged_ms,
            arrived_ms: logged_ms + self.profile.log_resolution_ms + self.profile.report_delay_ms,
        }
    }

    /// The creature for a kill the chat log reported; `now_ms` is the
    /// caller's clock, for giving up on a kill the watcher never covers.
    ///
    /// It rests on two premises, which play can check: a dead creature's
    /// plate is no longer shown or locked; and a kill's log line never
    /// arrives before the death it reports. The caller must stamp the
    /// report's arrival on the same clock as the pulses. A report whose
    /// arrival is further from its logged second than the profile believes
    /// is not answered.
    ///
    /// The death lies from the logged second less the longest report delay
    /// to the earlier of the logged second's end and the report's arrival.
    /// `NotYet` while pulses have not yet reached the arrival plus the
    /// capture slack and the look-ahead; once they have, or once the latest
    /// pulse or `now_ms` is past that by the give-up margin, a definite
    /// answer. Every fight read from the look-back before the logged second
    /// to the latest moment of death (and the capture slack) is gathered,
    /// less any read after the report arrived (a creature alive after its
    /// supposed death) and any the player had clearly left (another plate
    /// read over the left span, ending before the earliest moment of
    /// death). Each fight gathered counts as the creature its readings
    /// decide, or, undecided, as a creature of its own. The kill is given a
    /// creature only when they all agree on one: then it goes to that
    /// creature's fight that stopped being read nearest the death. Where
    /// they disagree only because of fights that stopped before the last
    /// two readings of the fight read on the last pulse before the report
    /// arrived, that fight's creature is suggested.
    ///
    /// Only a named or provisional answer takes its fight from later kills.
    /// The answer is at most a [`MobStamp::Guess`] or
    /// [`MobStamp::ProvisionalGuess`] when it is a suggestion; when a fight
    /// read after the report arrived was also read within the death's
    /// moments (it may have been locked when the kill came); when a fight
    /// the player left was left damaged (it may have bled out); when its fight
    /// was given to an earlier kill as a guess; and when the fight itself
    /// is not clearly that creature (see the vote).
    ///
    /// By design these go unnamed: a kill whose report arrives while
    /// another creature still to be read was already locked inside the
    /// death's moments (kills of different creatures in quick succession
    /// without the arrival to tell them apart); a kill in one shot followed
    /// at once by a lock on another creature; a kill of a creature never
    /// locked (area damage), unless the only other creature read in its
    /// moments is one the player left at once and went on reading another
    /// past the report, which the screen cannot tell from a kill of it; and
    /// any kill whose moments the watcher did not cover.
    pub fn stamp_for_kill(&mut self, report: KillReport, now_ms: i64) -> KillStamp {
        let p = &self.profile;
        let KillReport {
            logged_ms,
            arrived_ms,
        } = report;
        let lag = arrived_ms - logged_ms;
        if lag < -p.report_early_ms
            || lag > p.log_resolution_ms + p.report_delay_ms + p.report_slack_ms
        {
            return KillStamp::Unnamed(Unnamed::Implausible);
        }
        let alive_after = arrived_ms + p.capture_slack_ms;
        let end = alive_after + p.kill_lookahead_ms;
        let latest = self.pulses.back().copied();
        if latest.is_none_or(|l| l < end) {
            let clock = latest.map_or(now_ms, |l| l.max(now_ms));
            return if clock > end + p.give_up_ms {
                KillStamp::Unnamed(Unnamed::Unwatched)
            } else {
                KillStamp::NotYet
            };
        }
        let from = logged_ms - p.kill_lookback_ms;
        if !self.watched(from, end) || self.lost.iter().any(|(a, b)| *b >= from && *a <= end) {
            return KillStamp::Unnamed(Unnamed::Unwatched);
        }
        let latest_death = (logged_ms + p.log_resolution_ms).min(arrived_ms);
        let window_end = latest_death + p.capture_slack_ms;
        let earliest_death = logged_ms - p.report_delay_ms;
        let gathered: Vec<usize> = (0..self.fights.len())
            .filter(|i| {
                let f = &self.fights[*i];
                !f.claimed
                    && f.readings
                        .iter()
                        .any(|r| r.at_ms >= from && r.at_ms <= window_end)
            })
            .collect();
        // A creature read after the report arrived was alive after the
        // death it reports: not the creature killed. Read also within the
        // death's moments, it may have been the one locked when the kill
        // came, and then nothing is named over it.
        let alive = |i: &usize| self.fights[*i].last().at_ms > alive_after;
        let any_alive = gathered.iter().any(alive);
        let alive_rival = gathered.iter().any(|i| {
            alive(i)
                && self.fights[*i]
                    .readings
                    .iter()
                    .any(|r| r.at_ms >= earliest_death && r.at_ms <= window_end)
        });
        // A fight the player had clearly left: after its last reading,
        // another plate was read over the left span before the earliest
        // moment of death.
        let left = |i: usize| {
            let last = self.fights[i].last().at_ms;
            self.fights.iter().enumerate().any(|(j, g)| {
                let mut since = g
                    .readings
                    .iter()
                    .map(|r| r.at_ms)
                    .filter(|t| *t > last && *t <= earliest_death);
                match (since.next(), since.next_back()) {
                    (Some(first), Some(latest)) => j != i && latest - first >= p.left_span_ms,
                    _ => false,
                }
            })
        };
        // A creature the player left damaged may have bled out, or been
        // killed by another: left, it keeps any answer to a guess.
        let left_damaged = gathered
            .iter()
            .any(|i| !alive(i) && left(*i) && self.fights[*i].last().fill < p.damaged_fill);
        let doubted = alive_rival || left_damaged;
        let considered: Vec<(usize, Option<MobStamp>)> = gathered
            .iter()
            .copied()
            .filter(|i| !alive(i) && !left(*i))
            .map(|i| (i, self.decide(&self.fights[i])))
            .collect();
        // What each names: its decision, or, undecided, a creature of its
        // own that agrees with none.
        let mut identities: Vec<Identity> = Vec::new();
        for (i, decided) in &considered {
            let id = match decided {
                Some(stamp) => identity(stamp),
                None => Identity::Unknown(*i),
            };
            if !identities.contains(&id) {
                identities.push(id);
            }
        }
        let stopped = |i: usize| self.fights[i].last().at_ms <= window_end;
        match identities.as_slice() {
            [] if any_alive => KillStamp::Unnamed(Unnamed::StillRead),
            [] => KillStamp::Unnamed(Unnamed::NoFight),
            [only] => {
                let chosen = considered
                    .iter()
                    .filter(|(i, d)| {
                        stopped(*i) && d.as_ref().is_some_and(|s| identity(s) == *only)
                    })
                    .max_by_key(|(i, _)| self.fights[*i].last().at_ms)
                    .map(|(i, d)| (*i, d.clone()));
                let any_stopped = considered.iter().any(|(i, _)| stopped(*i));
                match chosen {
                    Some((i, Some(stamp))) => self.settle(i, stamp, doubted),
                    _ if any_stopped => KillStamp::Unnamed(Unnamed::Undecided),
                    _ => KillStamp::Unnamed(Unnamed::StillRead),
                }
            }
            _ => self.suggest(&considered, arrived_ms, latest_death, window_end),
        }
    }

    /// Give fight `i`'s creature to a kill as `stamp`; as a guess when
    /// `doubted`; and no higher than the grade the fight's creature was
    /// given to a kill at before, when that was a weak one. Only a named or
    /// provisional answer takes the fight from later kills.
    fn settle(&mut self, i: usize, stamp: MobStamp, doubted: bool) -> KillStamp {
        let fight = &mut self.fights[i];
        let stamp = match fight.given {
            Some(Given::LatestRead) => latest_read(stamp),
            Some(Given::Guess) => weakened(stamp),
            None if doubted => weakened(stamp),
            None => stamp,
        };
        let given = match &stamp {
            MobStamp::Named { .. } => {
                fight.claimed = true;
                None
            }
            MobStamp::Provisional { key, .. } => {
                fight.claimed = true;
                if let Some(u) = self.unfamiliar.iter_mut().find(|u| u.name == *key) {
                    u.stamped = true;
                }
                None
            }
            MobStamp::Guess { .. } | MobStamp::ProvisionalGuess { .. } => Some(Given::Guess),
            MobStamp::LatestRead { .. } | MobStamp::ProvisionalLatestRead { .. } => {
                Some(Given::LatestRead)
            }
        };
        fight.given = fight.given.max(given);
        KillStamp::Stamped(stamp)
    }

    /// When the fights read in a kill's moments name more than one
    /// creature, a suggestion where the screen leans: the fight that
    /// stopped last, read on the last pulse before the report arrived and
    /// read at least twice, every fight naming another creature having
    /// stopped before its last two readings. A known creature is given as a
    /// guess, an unfamiliar one as a provisional guess, never more.
    /// Otherwise the latest creature read, if any (see
    /// [`MobHistory::latest_read`]).
    fn suggest(
        &mut self,
        considered: &[(usize, Option<MobStamp>)],
        arrived_ms: i64,
        latest_death: i64,
        window_end: i64,
    ) -> KillStamp {
        let Some((chosen, decided)) = considered
            .iter()
            .filter(|(i, _)| self.fights[*i].last().at_ms <= window_end)
            .max_by_key(|(i, _)| self.fights[*i].last().at_ms)
        else {
            return self.latest_read(considered, latest_death);
        };
        let Some(stamp) = decided else {
            return self.latest_read(considered, latest_death);
        };
        let fight = &self.fights[*chosen];
        let Some(second_last) = fight.readings.iter().rev().nth(1).map(|r| r.at_ms) else {
            return self.latest_read(considered, latest_death);
        };
        let theirs = identity(stamp);
        let clear = considered.iter().all(|(i, d)| {
            *i == *chosen
                || self.fights[*i].last().at_ms < second_last
                || d.as_ref().is_some_and(|s| identity(s) == theirs)
        });
        let last_pulse = self
            .pulses
            .iter()
            .rev()
            .find(|t| **t <= arrived_ms)
            .copied();
        if !clear || last_pulse != Some(fight.last().at_ms) {
            return self.latest_read(considered, latest_death);
        }
        self.settle(*chosen, stamp.clone(), true)
    }

    /// Where the fights read in a kill's moments leave the screen without
    /// a lean, the creature read last: the fight with the latest reading
    /// at or before the latest moment of death that was read at least twice
    /// (a glimpse is passed over), when its own readings decide a creature
    /// (a known one read strictly at least once, or an unfamiliar one by the
    /// rule for provisional names) and no fight naming anything else stopped
    /// within a pulse of it. Given at the lowest grade, and the fight stays
    /// in contention. Otherwise the kill names nothing.
    fn latest_read(
        &mut self,
        considered: &[(usize, Option<MobStamp>)],
        latest_death: i64,
    ) -> KillStamp {
        let indistinct = KillStamp::Unnamed(Unnamed::Indistinct);
        let last = |i: usize| self.fights[i].last().at_ms;
        let mut stopped: Vec<&(usize, Option<MobStamp>)> = considered
            .iter()
            .filter(|(i, _)| last(*i) <= latest_death)
            .collect();
        stopped.sort_by_key(|(i, _)| std::cmp::Reverse(last(*i)));
        let Some((chosen, Some(stamp))) = stopped
            .iter()
            .find(|(i, _)| self.fights[*i].readings.len() >= 2)
            .map(|(i, d)| (*i, d.clone()))
        else {
            return indistinct;
        };
        let theirs = identity(&stamp);
        if let Identity::Known(creature) = &theirs {
            let own = Some(Vote::Known(creature.clone()));
            if !self.fights[chosen].readings.iter().any(|r| r.vote == own) {
                return indistinct;
            }
        }
        // Within a pulse: no pulse came between the two last readings.
        let at = last(chosen);
        let before = self.pulses.iter().rev().find(|t| **t < at).copied();
        let after = self.pulses.iter().find(|t| **t > at).copied();
        let close = stopped.iter().any(|(i, d)| {
            *i != chosen
                && before.is_none_or(|b| last(*i) >= b)
                && after.is_none_or(|a| last(*i) <= a)
                && d.as_ref().is_none_or(|s| identity(s) != theirs)
        });
        if close {
            return indistinct;
        }
        let fight = &mut self.fights[chosen];
        fight.given = fight.given.max(Some(Given::LatestRead));
        KillStamp::Stamped(latest_read(stamp))
    }

    /// The creature most of `fight`'s readings name, when the vote is
    /// decisive. Every reading with a name votes, a loose one for the
    /// creature it loosely names, an unfamiliar spelling folded onto the
    /// fight's leader for it. A known creature is named only when its own
    /// strict readings (not loose, not folded) carry the vote, one of them
    /// spelled it whole, and the fight was read more than once; otherwise
    /// it is a guess, and nothing when its loose readings are needed while a
    /// strict reading of the fight names another creature.
    fn decide(&self, fight: &Fight) -> Option<MobStamp> {
        let p = &self.profile;
        let ballots = self.ballots(fight);
        let total = ballots.len();
        if total < p.min_votes.max(1) {
            return None;
        }
        let mut tally: Vec<(&Vote, usize)> = Vec::new();
        for (vote, _, _) in &ballots {
            match tally.iter_mut().find(|(v, _)| *v == vote) {
                Some(entry) => entry.1 += 1,
                None => tally.push((vote, 1)),
            }
        }
        tally.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let (mut winner, mut votes) = (tally[0].0.clone(), tally[0].1);
        let runner_up = tally.get(1).map_or(0, |next| next.1);
        let carries = |n: usize| n > runner_up && (n as f64) >= p.vote_share * total as f64;
        // With no reading naming any known creature, an unfamiliar name
        // read at least `provisional_reads` times is the fight's creature
        // when no other unfamiliar name was read twice: one-off garbles do
        // not outvote it.
        let names_known = fight
            .readings
            .iter()
            .any(|r| matches!(r.vote, Some(Vote::Known(_))) || r.loose.is_some());
        let unfamiliar_votes: Vec<(&Vote, usize)> = tally
            .iter()
            .filter(|(v, _)| matches!(v, Vote::New(_)))
            .map(|(v, n)| (*v, *n))
            .collect();
        let unopposed = !names_known
            && unfamiliar_votes
                .first()
                .is_some_and(|(_, n)| *n >= p.provisional_reads.max(1))
            && unfamiliar_votes.iter().skip(1).all(|(_, n)| *n < 2);
        if !carries(votes) {
            if !unopposed {
                return None;
            }
            winner = unfamiliar_votes[0].0.clone();
            votes = unfamiliar_votes[0].1;
        }
        // A second creature in the fight: anything other than the leader
        // read twice (another known creature, strictly or loosely, or another
        // unfamiliar name). A repeated other name on a damaged plate cannot
        // be told from a second creature, so it is not outvoted.
        let mut others: Vec<(Vote, usize)> = Vec::new();
        for (vote, _, folded) in &ballots {
            if !*folded && *vote != winner {
                match others.iter_mut().find(|(v, _)| v == vote) {
                    Some(entry) => entry.1 += 1,
                    None => others.push((vote.clone(), 1)),
                }
            }
        }
        if others.iter().any(|(_, n)| *n >= 2) {
            return None;
        }
        let behind: Vec<(&Reading, bool)> = ballots
            .iter()
            .filter(|(v, _, _)| *v == winner)
            .map(|(_, r, folded)| (*r, *folded))
            .collect();
        let raw_text = commonest(behind.iter().filter_map(|(r, _)| r.raw.as_deref()))?;
        let evidence = Evidence {
            raw_text,
            first_read_ms: fight.first_ms(),
            last_read_ms: fight.last().at_ms,
            votes,
            of: total,
        };
        Some(match winner {
            Vote::Known(creature) => {
                let strict = behind
                    .iter()
                    .filter(|(r, folded)| r.loose.is_none() && !folded)
                    .count();
                let own = Some(Vote::Known(creature.clone()));
                // Spelled whole by a reading that named it itself.
                let spelled = behind
                    .iter()
                    .any(|(r, folded)| !folded && r.spelled && r.vote == own);
                // Named only on two strict readings of its own at least, the
                // fight's last among them: a plate last seen as anything else
                // was not clearly this creature when it died.
                let last_its_own = fight.last().vote == own;
                if carries(strict) && strict >= 2 && spelled && last_its_own {
                    MobStamp::Named { creature, evidence }
                } else if carries(strict) {
                    MobStamp::Guess { creature, evidence }
                } else {
                    let other_strict = fight
                        .readings
                        .iter()
                        .any(|r| matches!(&r.vote, Some(Vote::Known(c)) if *c != creature));
                    if other_strict {
                        return None;
                    }
                    MobStamp::Guess { creature, evidence }
                }
            }
            Vote::New(id) => {
                if votes < p.provisional_reads.max(1) {
                    return None;
                }
                // A name whose level never read may be another maturity's
                // name misread: a suggestion only.
                let unfamiliar = self.unfamiliar.iter().find(|u| u.id == id)?;
                let (key, spelling) = (unfamiliar.name.clone(), unfamiliar.spelling());
                if unfamiliar.level.is_some() {
                    MobStamp::Provisional {
                        key,
                        spelling,
                        evidence,
                    }
                } else {
                    MobStamp::ProvisionalGuess {
                        key,
                        spelling,
                        evidence,
                    }
                }
            }
        })
    }

    /// Each reading's vote, with the reading and whether its vote was
    /// folded. A known creature that leads the fight on its own strict
    /// readings (more than one) takes an unfamiliar spelling that comes as
    /// close to it as a name read alone must: the same creature misread
    /// further. A reading that names a creature only loosely votes for it.
    fn ballots<'f>(&self, fight: &'f Fight) -> Vec<(Vote, &'f Reading, bool)> {
        let mut known: Vec<(&Creature, usize)> = Vec::new();
        for (vote, n) in &fight.tally {
            if let Vote::Known(c) = vote {
                known.push((c, *n));
            }
        }
        let most = known.iter().map(|k| k.1).max().unwrap_or(0);
        let leaders: Vec<&Creature> = known.iter().filter(|k| k.1 == most).map(|k| k.0).collect();
        let fold: Option<(Vote, Vec<Vec<char>>)> = match leaders.as_slice() {
            [leader] if most >= 2 => {
                let keys = match leader {
                    Creature::Catalogued { name, maturity, .. }
                    | Creature::Species { name, maturity, .. } => {
                        plate_strings(name, maturity.as_deref(), &self.names)
                            .iter()
                            .map(|s| label_key(s))
                            .collect()
                    }
                    Creature::Confirmed { name } => vec![label_key(name)],
                };
                Some((Vote::Known((*leader).clone()), keys))
            }
            _ => None,
        };
        // The unfamiliar name the fight's readings most often give (more
        // than one of them) takes the fight's other unfamiliar spellings
        // close to it at its level: the same creature, misread another way.
        let mut unfamiliar: Vec<(usize, usize)> = Vec::new();
        for (vote, n) in &fight.tally {
            if let Vote::New(id) = vote {
                unfamiliar.push((*id, *n));
            }
        }
        let most_new = unfamiliar.iter().map(|u| u.1).max().unwrap_or(0);
        let new_leaders: Vec<usize> = unfamiliar
            .iter()
            .filter(|u| u.1 == most_new)
            .map(|u| u.0)
            .collect();
        let new_leader = match new_leaders.as_slice() {
            [id] if most_new >= 2 => self.unfamiliar.iter().find(|u| u.id == *id),
            _ => None,
        };
        let near_leader = |r: &Reading, id: usize| {
            let Some(leader) = new_leader else {
                return false;
            };
            let level = self
                .unfamiliar
                .iter()
                .find(|u| u.id == id)
                .and_then(|u| u.level);
            id != leader.id
                && leader.level.is_some()
                && leader.level == level
                && leader
                    .spellings
                    .iter()
                    .any(|(k, _)| ratio(&r.key, k) >= self.profile.consolidate_score)
        };
        fight
            .readings
            .iter()
            .filter_map(|r| {
                let vote = match (&r.vote, &r.loose) {
                    (Some(vote), _) => vote.clone(),
                    (None, Some(creature)) => Vote::Known(creature.clone()),
                    _ => return None,
                };
                match (&vote, &fold) {
                    (Vote::New(_), Some((leader, keys)))
                        if keys
                            .iter()
                            .any(|k| ratio(&r.key, k) >= self.names.known_score) =>
                    {
                        Some((leader.clone(), r, true))
                    }
                    (Vote::New(id), _) if near_leader(r, *id) => {
                        let leader = new_leader.expect("a leader");
                        Some((Vote::New(leader.id), r, true))
                    }
                    _ => Some((vote, r, false)),
                }
            })
            .collect()
    }
}

/// The weak grade a fight's creature was given to a kill at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Given {
    Guess,
    LatestRead,
}

/// Who a fight names: a known creature, an unfamiliar name by its key, or,
/// for a fight whose readings decide nothing, a creature of its own (by
/// the fight).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Identity {
    Known(Creature),
    New(String),
    Unknown(usize),
}

/// `stamp` at the lowest grade.
fn latest_read(stamp: MobStamp) -> MobStamp {
    match stamp {
        MobStamp::Named { creature, evidence }
        | MobStamp::Guess { creature, evidence }
        | MobStamp::LatestRead { creature, evidence } => {
            MobStamp::LatestRead { creature, evidence }
        }
        MobStamp::Provisional {
            key,
            spelling,
            evidence,
        }
        | MobStamp::ProvisionalGuess {
            key,
            spelling,
            evidence,
        }
        | MobStamp::ProvisionalLatestRead {
            key,
            spelling,
            evidence,
        } => MobStamp::ProvisionalLatestRead {
            key,
            spelling,
            evidence,
        },
    }
}

/// `stamp` as no more than a suggestion.
fn weakened(stamp: MobStamp) -> MobStamp {
    match stamp {
        MobStamp::Named { creature, evidence } => MobStamp::Guess { creature, evidence },
        MobStamp::Provisional {
            key,
            spelling,
            evidence,
        } => MobStamp::ProvisionalGuess {
            key,
            spelling,
            evidence,
        },
        weak => weak,
    }
}

fn identity(stamp: &MobStamp) -> Identity {
    match stamp {
        MobStamp::Named { creature, .. }
        | MobStamp::Guess { creature, .. }
        | MobStamp::LatestRead { creature, .. } => Identity::Known(creature.clone()),
        MobStamp::Provisional { key, .. }
        | MobStamp::ProvisionalGuess { key, .. }
        | MobStamp::ProvisionalLatestRead { key, .. } => Identity::New(key.clone()),
    }
}

/// The text given most often, the first given among equals.
fn commonest<'a>(texts: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut counted: Vec<(String, usize)> = Vec::new();
    for text in texts {
        match counted.iter_mut().find(|(t, _)| t == text) {
            Some(entry) => entry.1 += 1,
            None => counted.push((text.to_string(), 1)),
        }
    }
    commonest_counted(&counted)
}

/// The most counted text, the first among equals.
fn commonest_counted(counted: &[(String, usize)]) -> Option<String> {
    let most = counted.iter().map(|c| c.1).max()?;
    counted.iter().find(|c| c.1 == most).map(|c| c.0.clone())
}

/// How often a kept spelling or text was read: its share of the slot's
/// count, and the count the slot held when it took the place of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Kept {
    total: usize,
    inherited: usize,
}

impl Kept {
    /// The reads counted for this spelling or text itself.
    fn read(self) -> usize {
        self.total - self.inherited
    }
}

/// Count `item` among `entries`, keeping at most `cap` of them. When full, a
/// newcomer takes the place of the least read, starting one past its count
/// (which it keeps apart as inherited): a spelling read again and again is
/// kept however many one-off garbles were read before it.
fn count_kept<T: PartialEq>(entries: &mut Vec<(T, Kept)>, item: T, cap: usize) {
    if let Some(entry) = entries.iter_mut().find(|(k, _)| *k == item) {
        entry.1.total += 1;
    } else if entries.len() < cap {
        entries.push((
            item,
            Kept {
                total: 1,
                inherited: 0,
            },
        ));
    } else if let Some(least) = entries.iter_mut().min_by_key(|(_, n)| n.total) {
        let inherited = least.1.total;
        *least = (
            item,
            Kept {
                total: inherited + 1,
                inherited,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_ui::geometry::Rect;
    use crate::game_ui::mob_names::CatalogueMob;
    use crate::game_ui::plates::PlateTextError;
    use crate::game_ui::test_support::{bundled_mobs, bundled_profile};

    /// A history whose watcher has been pulsing once a second for a while.
    fn history() -> MobHistory {
        let profile = bundled_profile();
        let mut h = MobHistory::new(profile.mob_history, profile.mob_names);
        for t in (-20_000..0).step_by(1000) {
            h.observe(t, None);
        }
        h
    }

    /// Pulses with nothing locked, a second apart, up to (not at) `until`.
    fn pad(h: &mut MobHistory, until: i64) {
        let mut t = h.pulses.back().copied().unwrap_or(until) + 1000;
        while t < until {
            h.observe(t, None);
            t += 1000;
        }
    }

    /// One reading at `at_ms`, the pulses before it kept up.
    fn read_at(h: &mut MobHistory, at_ms: i64, sighting: Sighting) {
        pad(h, at_ms);
        h.observe(at_ms, Some(sighting));
    }

    /// When a report must have been watched past.
    fn watched_until(report: KillReport) -> i64 {
        let p = bundled_profile().mob_history;
        report.arrived_ms + p.capture_slack_ms + p.kill_lookahead_ms
    }

    /// Ask for `report`, pulsing on (nothing locked) until the screen has
    /// been watched far enough past it.
    fn answer(h: &mut MobHistory, report: KillReport) -> KillStamp {
        let end = watched_until(report);
        while h.pulses.back().is_none_or(|t| *t < end) {
            let t = h.pulses.back().map_or(end, |t| t + 1000);
            h.observe(t, None);
        }
        let now = *h.pulses.back().expect("pulses");
        h.stamp_for_kill(report, now)
    }

    /// Ask for the kill logged at `logged_ms`, its arrival not known.
    fn kill(h: &mut MobHistory, logged_ms: i64) -> KillStamp {
        let report = h.report_without_arrival(logged_ms);
        answer(h, report)
    }

    /// Ask for the kill logged at `logged_ms` whose line arrived at
    /// `arrived_ms`.
    fn kill_arrived(h: &mut MobHistory, logged_ms: i64, arrived_ms: i64) -> KillStamp {
        answer(
            h,
            KillReport {
                logged_ms,
                arrived_ms,
            },
        )
    }

    fn mob(name: &str, maturity: Option<&str>, level: u32) -> CatalogueMob {
        CatalogueMob {
            name: name.into(),
            species: Some(name.into()),
            maturity: maturity.map(String::from),
            level: Some(level),
        }
    }

    /// A small list of creatures, and names confirmed by the player.
    fn catalogue(confirmed: Vec<String>) -> MobCatalogue {
        MobCatalogue::new(
            [
                mob("Combibo", Some("Young"), 3),
                mob("Combibo", Some("Mature"), 4),
                mob("Atrox", Some("Young"), 4),
                mob("Atrox", Some("Old"), 6),
                mob("Snarg", Some("Old"), 14),
                mob("Snarg", Some("Old Alpha"), 19),
                mob("Drone", Some("Gen 01"), 10),
                mob("Drone", Some("Gen 07"), 40),
            ],
            confirmed,
            &bundled_profile().mob_names,
        )
    }

    /// A reading of `text` at `level`, resolved against `catalogue`.
    fn seen(catalogue: &MobCatalogue, text: &str, level: Option<u32>, fill: f64) -> Sighting {
        Sighting {
            fill,
            raw_name: Some(text.to_string()),
            resolved: resolve(text, level, catalogue, &bundled_profile().mob_names),
        }
    }

    /// Feed `sightings` one a second from `start_ms`; the last's time.
    fn feed(h: &mut MobHistory, start_ms: i64, sightings: Vec<Sighting>) -> i64 {
        let mut at = start_ms;
        for sighting in sightings {
            read_at(h, at, sighting);
            at += 1000;
        }
        at - 1000
    }

    fn creature(name: &str, maturity: &str) -> Creature {
        Creature::Catalogued {
            name: name.into(),
            species: Some(name.into()),
            maturity: Some(maturity.into()),
        }
    }

    fn combibo(maturity: &str) -> Creature {
        creature("Combibo", maturity)
    }

    fn named(stamp: &KillStamp) -> Option<&Creature> {
        match stamp {
            KillStamp::Stamped(MobStamp::Named { creature, .. }) => Some(creature),
            _ => None,
        }
    }

    fn latest(stamp: &KillStamp) -> Option<&Creature> {
        match stamp {
            KillStamp::Stamped(MobStamp::LatestRead { creature, .. }) => Some(creature),
            _ => None,
        }
    }

    fn guessed(stamp: &KillStamp) -> Option<&Creature> {
        match stamp {
            KillStamp::Stamped(MobStamp::Guess { creature, .. }) => Some(creature),
            _ => None,
        }
    }

    #[test]
    fn a_fight_is_stamped_once_the_screen_has_been_watched_long_enough_past_the_kill() {
        let c = catalogue(Vec::new());
        let mut h = history();
        let last = feed(
            &mut h,
            10_000,
            vec![
                seen(&c, "Combibo Young", Some(3), 1.0),
                seen(&c, "Combibo Yuong", Some(3), 0.8),
                seen(&c, "Combib0 Young", Some(3), 0.6),
                seen(&c, "#%", None, 0.4),
                seen(&c, "Combibo Young", Some(3), 0.1),
            ],
        );
        let at = (last + 1500) / 1000 * 1000;
        let report = h.report_without_arrival(at);
        // Pulses still arriving short of the look-ahead: not yet, whatever
        // the caller's clock says short of the give-up margin.
        pad(&mut h, at + 2000);
        assert_eq!(h.stamp_for_kill(report, at + 2000), KillStamp::NotYet);
        assert_eq!(
            h.stamp_for_kill(report, watched_until(report) + 4000),
            KillStamp::NotYet
        );
        let stamp = kill(&mut h, at);
        assert_eq!(
            stamp,
            KillStamp::Stamped(MobStamp::Named {
                creature: combibo("Young"),
                evidence: Evidence {
                    raw_text: "Combibo Young".into(),
                    first_read_ms: 10_000,
                    last_read_ms: last,
                    votes: 4,
                    of: 4,
                },
            })
        );
        // The fight is given to one kill only.
        assert_eq!(kill(&mut h, at), KillStamp::Unnamed(Unnamed::NoFight));
    }

    #[test]
    fn a_kill_the_watcher_did_not_cover_is_unwatched() {
        let c = catalogue(Vec::new());
        // The watcher stalled for four seconds inside the kill's moments.
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Combibo Young", Some(3), 0.6),
                seen(&c, "Combibo Young", Some(3), 0.2),
            ],
        );
        h.observe(last + 4000, None);
        assert_eq!(
            kill(&mut h, last + 1000),
            KillStamp::Unnamed(Unnamed::Unwatched)
        );
        // The watcher stopped: the caller's clock passes the give-up margin.
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Combibo Young", Some(3), 0.6),
                seen(&c, "Combibo Young", Some(3), 0.2),
            ],
        );
        let report = h.report_without_arrival(last + 1000);
        let give_up = bundled_profile().mob_history.give_up_ms;
        assert_eq!(
            h.stamp_for_kill(report, watched_until(report) + give_up),
            KillStamp::NotYet
        );
        assert_eq!(
            h.stamp_for_kill(report, watched_until(report) + give_up + 1),
            KillStamp::Unnamed(Unnamed::Unwatched)
        );
        // A watcher that only started after the kill's look-back began.
        let profile = bundled_profile();
        let mut h = MobHistory::new(profile.mob_history, profile.mob_names);
        h.observe(5500, Some(seen(&c, "Combibo Young", Some(3), 0.6)));
        h.observe(6500, Some(seen(&c, "Combibo Young", Some(3), 0.2)));
        assert_eq!(kill(&mut h, 7000), KillStamp::Unnamed(Unnamed::Unwatched));
    }

    #[test]
    fn an_add_killed_after_the_player_clearly_left_an_earlier_fight_is_guessed_as_the_add() {
        let c = catalogue(Vec::new());
        let mut h = history();
        // Mature fought down to a sliver and left at 3 s, inside the kill's
        // look-back; Young read from 4 s, over the left span before the
        // earliest moment of the kill reported at 7 s, then killed. Without
        // the player having clearly left Mature, two creatures are read in
        // the kill's moments and nothing could be given. Mature was left
        // damaged and may have bled out, so Young is only guessed.
        feed(
            &mut h,
            1000,
            vec![
                seen(&c, "Combibo Mature", Some(4), 0.6),
                seen(&c, "Combibo Mature", Some(4), 0.3),
                seen(&c, "Combibo Mature", Some(4), 0.05),
            ],
        );
        let last = feed(
            &mut h,
            4000,
            vec![
                seen(&c, "Combibo Young", Some(3), 1.0),
                seen(&c, "Combibo Young", Some(3), 0.6),
                seen(&c, "Combibo Young", Some(3), 0.3),
            ],
        );
        assert_eq!(last, 6000);
        let stamp = kill(&mut h, 7000);
        assert_eq!(named(&stamp), None);
        assert_eq!(guessed(&stamp), Some(&combibo("Young")));
    }

    #[test]
    fn a_fight_left_only_inside_the_kills_moments_is_still_a_rival() {
        // Timeline 3 widened: Young read to 7 s, Mature read at 8 s and 9 s
        // (over the left span, but not before the earliest moment of the
        // kill reported at 10 s). Young may have died at 9.4 s, so nothing
        // is named or suggested; Mature, read twice and last, is the latest
        // creature read.
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            5000,
            vec![
                seen(&c, "Combibo Young", Some(3), 0.4),
                seen(&c, "Combibo Young", Some(3), 0.3),
                seen(&c, "Combibo Young", Some(3), 0.15),
            ],
        );
        feed(
            &mut h,
            8000,
            vec![
                seen(&c, "Combibo Mature", Some(4), 1.0),
                seen(&c, "Combibo Mature", Some(4), 1.0),
            ],
        );
        assert_eq!(latest(&kill(&mut h, 10_000)), Some(&combibo("Mature")));
    }

    #[test]
    fn creatures_killed_in_one_or_two_shots_are_stamped_a_single_reading_only_as_a_guess() {
        let c = catalogue(Vec::new());
        let mut h = history();
        read_at(&mut h, 5000, seen(&c, "Combibo Young", Some(3), 1.0));
        assert_eq!(guessed(&kill(&mut h, 6000)), Some(&combibo("Young")));
        feed(
            &mut h,
            20_000,
            vec![
                seen(&c, "Combibo Mature", Some(4), 1.0),
                seen(&c, "Combibo Mature", Some(4), 0.45),
            ],
        );
        assert_eq!(named(&kill(&mut h, 22_000)), Some(&combibo("Mature")));
    }

    #[test]
    fn a_kill_by_area_damage_while_the_locked_creature_lives_on_is_not_stamped() {
        let c = catalogue(Vec::new());
        let mut h = history();
        let readings = (0..12)
            .map(|k| seen(&c, "Combibo Mature", Some(4), 1.0 - k as f64 * 0.05))
            .collect();
        let last = feed(&mut h, 0, readings);
        assert_eq!(kill(&mut h, 6000), KillStamp::Unnamed(Unnamed::StillRead));
        // The fight is still there for its own kill.
        assert_eq!(named(&kill(&mut h, last + 1000)), Some(&combibo("Mature")));
    }

    #[test]
    fn an_area_damage_kill_after_the_lock_moved_on_twice_is_not_given_the_first_creature() {
        // Timeline 4: Young stops at 6 s, Mature read 7 s to 9 s, the lock
        // moves to an Atrox read at 10 s and 11 s; a kill logged at 10 s,
        // arriving at 11.5 s, is something never locked. Young was clearly
        // left; Mature and Atrox are both read in the death's moments. The
        // screen leans to the Atrox, read last and on the pulse before the
        // report, so it is suggested, never named: what the screen shows is
        // also what a kill of the Atrox shows.
        let c = catalogue(Vec::new());
        let lead = |h: &mut MobHistory| {
            feed(
                h,
                4000,
                vec![
                    seen(&c, "Combibo Young", Some(3), 0.5),
                    seen(&c, "Combibo Young", Some(3), 0.3),
                    seen(&c, "Combibo Young", Some(3), 0.1),
                ],
            );
            feed(
                h,
                7000,
                vec![
                    seen(&c, "Combibo Mature", Some(4), 1.0),
                    seen(&c, "Combibo Mature", Some(4), 0.9),
                    seen(&c, "Combibo Mature", Some(4), 0.8),
                ],
            );
        };
        let mut h = history();
        lead(&mut h);
        feed(
            &mut h,
            10_000,
            vec![
                seen(&c, "Atrox Old", Some(6), 1.0),
                seen(&c, "Atrox Old", Some(6), 0.9),
            ],
        );
        let stamp = kill_arrived(&mut h, 10_000, 11_500);
        assert_eq!(named(&stamp), None);
        assert_eq!(guessed(&stamp), Some(&creature("Atrox", "Old")));
        // Were the Atrox read on past the report's arrival, it is alive
        // after the report; but it was read within the death's moments too,
        // so it may be what was locked when the kill came: Mature is only
        // guessed.
        let mut h = history();
        lead(&mut h);
        let readings = (0..5)
            .map(|k| seen(&c, "Atrox Old", Some(6), 1.0 - k as f64 * 0.1))
            .collect();
        feed(&mut h, 10_000, readings);
        let stamp = kill_arrived(&mut h, 10_000, 11_500);
        assert_eq!(named(&stamp), None);
        assert_eq!(guessed(&stamp), Some(&combibo("Mature")));
    }

    #[test]
    fn a_brief_lock_on_another_creature_mid_fight_takes_neither_the_kill_nor_the_fight() {
        let c = catalogue(Vec::new());
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Combibo Young", Some(3), 1.0),
                seen(&c, "Combibo Young", Some(3), 0.8),
                seen(&c, "Combibo Young", Some(3), 0.6),
                seen(&c, "Combibo Mature", Some(4), 0.95),
                seen(&c, "Combibo Mature", Some(4), 0.95),
                seen(&c, "Combibo Young", Some(3), 0.55),
                seen(&c, "Combibo Yuong", Some(3), 0.3),
                seen(&c, "Combibo Young", Some(3), 0.2),
                seen(&c, "Combibo Young", Some(3), 0.05),
            ],
        );
        match kill(&mut h, last + 1000) {
            KillStamp::Stamped(MobStamp::Named { creature, evidence }) => {
                assert_eq!(creature, combibo("Young"));
                assert_eq!((evidence.votes, evidence.of), (7, 7));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            kill(&mut h, last + 2000),
            KillStamp::Unnamed(Unnamed::NoFight)
        );
    }

    #[test]
    fn a_misreading_on_the_last_reading_of_another_listed_creature_names_nothing() {
        // Timeline 1: Gen 01 read to 8 s; at 9 s the name reads as Gen 07,
        // its level not read; the kill is reported at 10 s. A name read
        // strictly never joins a fight by health alone: it may be a second
        // creature already damaged.
        let c = catalogue(Vec::new());
        for fill in [0.15, 1.0] {
            let mut h = history();
            let readings = (0..9)
                .map(|k| seen(&c, "Drone Gen 01", Some(10), 1.0 - k as f64 * 0.1))
                .collect();
            feed(&mut h, 0, readings);
            read_at(&mut h, 9000, seen(&c, "Drone Gen 07", None, fill));
            assert_eq!(h.fights.len(), 2);
            assert_eq!(
                kill(&mut h, 10_000),
                KillStamp::Unnamed(Unnamed::Indistinct),
                "{fill}"
            );
        }
    }

    #[test]
    fn a_second_creature_already_damaged_is_never_outvoted_by_the_first() {
        // Young read eight times down to half health; the player switches to
        // a creature already at 0.45, reads it twice, and it dies.
        let c = catalogue(Vec::new());
        let young = |h: &mut MobHistory| {
            let readings = (0..8)
                .map(|k| seen(&c, "Combibo Young", Some(3), 1.0 - k as f64 * 0.07))
                .collect();
            feed(h, 0, readings);
        };
        // A listed creature: its own fight, and both are read in the death's
        // moments; it was read twice and last, so it is the latest creature
        // read.
        let mut h = history();
        young(&mut h);
        feed(
            &mut h,
            8000,
            vec![
                seen(&c, "Combibo Mature", Some(4), 0.45),
                seen(&c, "Combibo Mature", Some(4), 0.4),
            ],
        );
        assert_eq!(
            latest(&kill_arrived(&mut h, 10_000, 10_200)),
            Some(&combibo("Mature"))
        );
        // An unlisted creature under a consistent spelling: its first reading
        // joins Young's fight by health, its second does too, and the fight
        // then holds a second creature: nothing.
        let mut h = history();
        young(&mut h);
        feed(
            &mut h,
            8000,
            vec![
                seen(&c, "Zombie Fleshripper", Some(25), 0.45),
                seen(&c, "Zombie Fleshripper", Some(25), 0.4),
            ],
        );
        assert_eq!(
            kill_arrived(&mut h, 10_000, 10_200),
            KillStamp::Unnamed(Unnamed::Undecided)
        );
    }

    #[test]
    fn a_brief_lock_priming_a_neighbours_fight_names_nothing() {
        // Timeline 1, second form: an unlisted Elder, a brief lock on a
        // fresh Atrox Old, back to the Elder, whose next reading comes out
        // near Atrox Old and joins the Old fight; kill reported at 6 s.
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            0,
            vec![
                seen(&c, "Atrox Elder", Some(9), 0.9),
                seen(&c, "Atrox Elder", Some(9), 0.8),
                seen(&c, "Atrox Old", Some(6), 0.95),
                seen(&c, "Atrox Old", Some(6), 0.95),
                seen(&c, "Atrox Elder", Some(9), 0.7),
                seen(&c, "Atrox Older", None, 0.6),
            ],
        );
        assert_eq!(kill(&mut h, 6000), KillStamp::Unnamed(Unnamed::Indistinct));
    }

    #[test]
    fn two_kills_of_different_creatures_seconds_apart_are_told_apart_by_the_reports_arrival() {
        // Timeline 2: Young read to 6 s, dies at 6.5 s; Mature read at 7 s
        // and 8 s, dies at 8.3 s; logged at 7 s and 8 s.
        let c = catalogue(Vec::new());
        let fights = |h: &mut MobHistory| {
            feed(
                h,
                3000,
                vec![
                    seen(&c, "Combibo Young", Some(3), 0.5),
                    seen(&c, "Combibo Young", Some(3), 0.3),
                    seen(&c, "Combibo Young", Some(3), 0.1),
                ],
            );
            feed(
                h,
                7000,
                vec![
                    seen(&c, "Combibo Mature", Some(4), 1.0),
                    seen(&c, "Combibo Mature", Some(4), 0.5),
                ],
            );
        };
        // Arriving at 7.2 s and 8.5 s: Mature was read after the first
        // report arrived, so the first kill is Young's; but Mature was read
        // at 7 s, within the first death's moments, so Young is only
        // guessed, and stays in contention for the second kill, whose
        // creature is then only suggested.
        let mut h = history();
        fights(&mut h);
        let first = kill_arrived(&mut h, 7000, 7200);
        assert_eq!(named(&first), None);
        assert_eq!(guessed(&first), Some(&combibo("Young")));
        let second = kill_arrived(&mut h, 8000, 8500);
        assert_eq!(named(&second), None);
        assert_eq!(guessed(&second), Some(&combibo("Mature")));
        // Without the arrivals, both creatures are in contention for both,
        // and nothing is named or suggested: each kill is given Mature, the
        // latest creature read, at the lowest grade (for the first, a miss).
        let mut h = history();
        fights(&mut h);
        assert_eq!(latest(&kill(&mut h, 7000)), Some(&combibo("Mature")));
        assert_eq!(latest(&kill(&mut h, 8000)), Some(&combibo("Mature")));
        // The same creature twice: each kill a fight, nearest first.
        let mut h = history();
        feed(
            &mut h,
            3000,
            vec![
                seen(&c, "Combibo Young", Some(3), 0.5),
                seen(&c, "Combibo Young", Some(3), 0.3),
                seen(&c, "Combibo Young", Some(3), 0.1),
            ],
        );
        feed(
            &mut h,
            7000,
            vec![
                seen(&c, "Combibo Young", Some(3), 1.0),
                seen(&c, "Combibo Young", Some(3), 0.5),
            ],
        );
        assert_eq!(named(&kill(&mut h, 7000)), Some(&combibo("Young")));
        assert_eq!(named(&kill(&mut h, 8000)), Some(&combibo("Young")));
    }

    #[test]
    fn a_kill_followed_at_once_by_a_lock_on_another_kind_is_named_only_when_the_lock_came_after_the_death(
    ) {
        // Young read to 6 s and killed at 6.4 s; Mature locked next and read
        // on through the report.
        let c = catalogue(Vec::new());
        let run = |mature_from: i64, logged: i64, arrived: i64| {
            let mut h = history();
            feed(
                &mut h,
                2000,
                (0..5)
                    .map(|k| seen(&c, "Combibo Young", Some(3), 0.9 - k as f64 * 0.2))
                    .collect(),
            );
            let readings = (0..6)
                .map(|k| seen(&c, "Combibo Mature", Some(4), 1.0 - k as f64 * 0.1))
                .collect();
            feed(&mut h, mature_from, readings);
            kill_arrived(&mut h, logged, arrived)
        };
        // Mature first read at 7 s; logged 7 s, arrived 7.3 s: the death can
        // lie as late as 7.3 s, after Mature's first reading, so Mature may
        // have been locked when the kill came. A guess.
        let stamp = run(7000, 7000, 7300);
        assert_eq!(named(&stamp), None);
        assert_eq!(guessed(&stamp), Some(&combibo("Young")));
        // Logged 6 s, arrived 6.9 s, Mature first read at 7.3 s, past the
        // latest death and the capture slack: Young is named.
        assert_eq!(named(&run(7300, 6000, 6900)), Some(&combibo("Young")));
        // First read at the capture slack's edge, still within it: a guess.
        let stamp = run(7200, 6000, 6900);
        assert_eq!(named(&stamp), None);
        assert_eq!(guessed(&stamp), Some(&combibo("Young")));
    }

    #[test]
    fn an_undecided_report_takes_no_fight_from_later_kills() {
        // A fight whose readings split, read up to 5 s; an area-damage kill
        // logged at 4 s arrives at 4.9 s. An undecided answer takes no fight
        // from later kills: it stays in contention, as a creature of its own.
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            1000,
            vec![
                seen(&c, "Combibo Young", Some(3), 0.9),
                seen(&c, "Zombie Fleshripper", Some(3), 0.7),
                seen(&c, "Combibo Young", Some(3), 0.5),
                seen(&c, "Zombie Fleshripper", Some(3), 0.3),
                seen(&c, "Combibo Young", Some(3), 0.1),
            ],
        );
        assert_eq!(
            kill_arrived(&mut h, 4000, 4900),
            KillStamp::Unnamed(Unnamed::Undecided)
        );
        assert!(!h.fights[0].claimed);
        assert_eq!(
            kill_arrived(&mut h, 5000, 5600),
            KillStamp::Unnamed(Unnamed::Undecided)
        );
        assert!(!h.fights[0].claimed);
    }

    #[test]
    fn a_repeated_other_spelling_on_a_damaged_plate_is_not_outvoted() {
        // A misreading too far off to match, read twice, beside the
        // creature's own name: it cannot be told from a second creature, so
        // nothing is named.
        let c = catalogue(Vec::new());
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Combibo Young", Some(3), 0.9),
                seen(&c, "Cornblbo Yuong", Some(3), 0.8),
                seen(&c, "Combibo Young", Some(3), 0.7),
                seen(&c, "Cornblbo Yuong", Some(3), 0.6),
                seen(&c, "Combibo Young", Some(3), 0.5),
                seen(&c, "Combibo Young", Some(3), 0.4),
            ],
        );
        assert_eq!(h.fights.len(), 1);
        assert_eq!(
            kill(&mut h, last + 1000),
            KillStamp::Unnamed(Unnamed::Undecided)
        );
        // Read once among unclear readings on the same damaged plate, an
        // unfamiliar spelling joins the fight by health and is outvoted.
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Combibo Young", Some(3), 0.9),
                seen(&c, "Combibo Young", Some(3), 0.8),
                seen(&c, "Cornblbo Yuong", Some(3), 0.7),
                seen(&c, "Combibo Young", Some(3), 0.6),
                seen(&c, "#%", None, 0.5),
                seen(&c, "Combibo Young", Some(3), 0.4),
            ],
        );
        assert_eq!(h.fights.len(), 1);
        assert_eq!(named(&kill(&mut h, last + 1000)), Some(&combibo("Young")));
    }

    #[test]
    fn an_unfamiliar_name_read_again_and_again_is_not_outvoted_by_one_off_garble() {
        let c = catalogue(Vec::new());
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Zombie Fleshripper", Some(25), 0.9),
                seen(&c, "Qwxv Plorn", Some(25), 0.7),
                seen(&c, "Zombie Fleshripper", Some(25), 0.5),
                seen(&c, "Trel Mibbo", Some(25), 0.3),
                seen(&c, "Glonk Wesk", Some(25), 0.1),
            ],
        );
        match kill(&mut h, last + 1000) {
            KillStamp::Stamped(MobStamp::Provisional {
                spelling, evidence, ..
            }) => {
                assert_eq!(spelling, "Zombie Fleshripper");
                assert_eq!((evidence.votes, evidence.of), (2, 5));
            }
            other => panic!("{other:?}"),
        }
        // A second unfamiliar name read twice: nothing.
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Zombie Fleshripper", Some(25), 0.9),
                seen(&c, "Qwxv Plorn", Some(25), 0.7),
                seen(&c, "Zombie Fleshripper", Some(25), 0.5),
                seen(&c, "Qwxv Plorn", Some(25), 0.3),
                seen(&c, "Glonk Wesk", Some(25), 0.1),
            ],
        );
        assert!(!matches!(kill(&mut h, last + 1000), KillStamp::Stamped(_)));
    }

    #[test]
    fn a_misspelling_read_again_and_again_stays_with_its_name_once_one_offs_fill_the_spellings_kept(
    ) {
        // The spellings kept fill with one-off misreadings before a steady
        // one is first read. The steady one must still be remembered: were
        // it not, a later garble as close to it as the name is would leave
        // it two names to join, and it would become a name of its own.
        let c = catalogue(Vec::new());
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            [
                "Zombie Fleshripper",
                "Zombie Flesbripper",
                "Zombie Fleshriuper",
                "Zombie Fleshrlpper",
                "Zornbic Fleshripper",
                "Zornblc Fleshfiper",
                "Zornbic Fleshripper",
            ]
            .iter()
            .enumerate()
            .map(|(k, text)| seen(&c, text, Some(25), 1.0 - k as f64 * 0.1))
            .collect(),
        );
        let start = last + 10_000;
        let last = feed(
            &mut h,
            start,
            (0..3)
                .map(|k| seen(&c, "Zornbic Fleshripper", Some(25), 0.9 - k as f64 * 0.2))
                .collect(),
        );
        match kill(&mut h, last + 1000) {
            KillStamp::Stamped(MobStamp::Provisional { key, .. }) => {
                assert_eq!(key, "ZOMBIEFLESHRIPPER L25 #0");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_fight_whose_name_never_reads_is_a_creature_of_its_own() {
        // Mature read 5 s to 8 s down to 0.3; the player switches to a
        // creature whose name never reads, read at 9 s (full) and 10 s
        // (0.7), which dies at 10.4 s: logged 10 s, arrived 10.5 s.
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            5000,
            (0..4)
                .map(|k| seen(&c, "Combibo Mature", Some(4), 0.9 - k as f64 * 0.2))
                .collect(),
        );
        let unread = |fill: f64| Sighting {
            fill,
            raw_name: None,
            resolved: Resolved::Unclear,
        };
        feed(&mut h, 9000, vec![unread(1.0), unread(0.7)]);
        assert_eq!(
            kill_arrived(&mut h, 10_000, 10_500),
            KillStamp::Unnamed(Unnamed::Indistinct)
        );
        // Read twice, damaged, its name never read: the latest fight decides
        // nothing, so there is no latest creature read either.
        let mut h = history();
        feed(
            &mut h,
            5000,
            (0..4)
                .map(|k| seen(&c, "Combibo Mature", Some(4), 0.9 - k as f64 * 0.2))
                .collect(),
        );
        feed(&mut h, 9000, vec![unread(0.9), unread(0.7)]);
        assert_eq!(h.fights.len(), 2);
        assert_eq!(
            kill_arrived(&mut h, 10_000, 10_500),
            KillStamp::Unnamed(Unnamed::Indistinct)
        );
    }

    #[test]
    fn no_latest_creature_read_when_another_stopped_within_a_pulse_of_it() {
        // Young and Mature read in turn, a pulse each, twice each: the two
        // last readings are a pulse apart.
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            4000,
            vec![
                seen(&c, "Combibo Young", Some(3), 0.9),
                seen(&c, "Combibo Mature", Some(4), 0.9),
                seen(&c, "Combibo Young", Some(3), 0.7),
                seen(&c, "Combibo Mature", Some(4), 0.7),
            ],
        );
        assert_eq!(h.fights.len(), 2);
        assert_eq!(kill(&mut h, 8000), KillStamp::Unnamed(Unnamed::Indistinct));
    }

    #[test]
    fn a_fight_given_as_the_latest_creature_read_gives_later_kills_no_more() {
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            0,
            (0..4)
                .map(|k| seen(&c, "Combibo Young", Some(3), 0.9 - k as f64 * 0.2))
                .collect(),
        );
        let i = h.fights.len() - 1;
        h.fights[i].given = Some(Given::LatestRead);
        let stamp = kill(&mut h, 4000);
        assert_eq!(latest(&stamp), Some(&combibo("Young")));
        assert!(!h.fights[i].claimed);
    }

    #[test]
    fn the_spelling_shown_for_an_unfamiliar_name_is_the_one_read_most_itself() {
        let name = |texts: Vec<(&str, usize, usize)>| Unfamiliar {
            id: 0,
            key: label_key("Zombie Fleshripper"),
            name: "ZOMBIEFLESHRIPPER L25 #0".into(),
            level: Some(25),
            spellings: Vec::new(),
            texts: texts
                .into_iter()
                .map(|(t, total, inherited)| (t.to_string(), Kept { total, inherited }))
                .collect(),
            count: 0,
            stamped: false,
        };
        // A slot's count inherited from the spellings it took the place of is
        // not this spelling's.
        assert_eq!(
            name(vec![
                ("Zombie Fleshripper", 5, 0),
                ("Zornbie Flesbripper", 30, 29)
            ])
            .spelling(),
            "Zombie Fleshripper"
        );
        // A session's end in a simulated hunt: the name and its most common
        // misreading each read 27 times; the name is the one nearest the
        // other readings.
        assert_eq!(
            name(vec![
                ("Zombi FIeshripper", 17, 16),
                ("Zornbie Flesbripper", 28, 1),
                ("Zombie FIeshripper", 21, 1),
                ("Zombie Fleshripper", 28, 1),
            ])
            .spelling(),
            "Zombie Fleshripper"
        );
    }

    #[test]
    fn a_named_kill_needs_two_strict_readings_of_its_own() {
        // A read once at 9 s; a creature at full health whose name does not
        // read, at 10 s, is a fresh lock, not A's fight going on.
        let c = catalogue(Vec::new());
        let unread = |fill: f64| Sighting {
            fill,
            raw_name: Some("Xq".into()),
            resolved: Resolved::Unclear,
        };
        let mut h = history();
        read_at(&mut h, 9000, seen(&c, "Atrox Old", Some(6), 1.0));
        read_at(&mut h, 10_000, unread(1.0));
        assert_eq!(h.fights.len(), 2);
        assert_eq!(
            kill_arrived(&mut h, 10_000, 10_500),
            KillStamp::Unnamed(Unnamed::Indistinct)
        );
        // An unclear reading and one strict one: two readings, one vote. A
        // guess.
        let mut h = history();
        read_at(&mut h, 0, unread(0.9));
        read_at(&mut h, 1000, seen(&c, "Atrox Old", Some(6), 0.7));
        assert_eq!(h.fights.len(), 1);
        let stamp = kill(&mut h, 2000);
        assert_eq!(named(&stamp), None);
        assert_eq!(guessed(&stamp), Some(&creature("Atrox", "Old")));
    }

    #[test]
    fn a_plate_last_seen_as_anything_else_gives_no_more_than_a_guess() {
        // Young read eight times down to 0.51; an unlisted creature already
        // at 0.45 is read once at 8 s, joining the fight by health, and dies.
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            0,
            (0..8)
                .map(|k| seen(&c, "Combibo Young", Some(3), 1.0 - k as f64 * 0.07))
                .collect(),
        );
        read_at(&mut h, 8000, seen(&c, "Zombie Fleshripper", Some(25), 0.45));
        assert_eq!(h.fights.len(), 1);
        let stamp = kill_arrived(&mut h, 8000, 8500);
        assert_eq!(named(&stamp), None);
        assert_eq!(guessed(&stamp), Some(&combibo("Young")));
        // The same with an unclear last reading.
        let mut h = history();
        feed(
            &mut h,
            0,
            (0..4)
                .map(|k| seen(&c, "Combibo Young", Some(3), 0.9 - k as f64 * 0.2))
                .collect(),
        );
        read_at(&mut h, 4000, seen(&c, "Xq", Some(3), 0.05));
        let stamp = kill(&mut h, 5000);
        assert_eq!(named(&stamp), None);
        assert_eq!(guessed(&stamp), Some(&combibo("Young")));
        // Ending on its own reading, it is named.
        let mut h = history();
        feed(
            &mut h,
            0,
            (0..4)
                .map(|k| seen(&c, "Combibo Young", Some(3), 0.9 - k as f64 * 0.2))
                .collect(),
        );
        assert_eq!(named(&kill(&mut h, 4000)), Some(&combibo("Young")));
    }

    #[test]
    fn a_creature_read_in_the_deaths_moments_and_alive_after_the_report_keeps_a_rival_to_a_guess() {
        // Mature clicked at 8 s and 9 s at full health; at 10 s the player
        // locks a Young, kills it in one shot at 10.3 s and locks another
        // Young before 11 s, read on (joining the first Young's fight).
        // Logged 10 s, arrived 10.4 s. The Young fight is alive after the
        // report, but it was read at 10 s, within the death's moments.
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            8000,
            vec![
                seen(&c, "Combibo Mature", Some(4), 1.0),
                seen(&c, "Combibo Mature", Some(4), 1.0),
            ],
        );
        feed(
            &mut h,
            10_000,
            (0..4)
                .map(|k| seen(&c, "Combibo Young", Some(3), 1.0 - k as f64 * 0.2))
                .collect(),
        );
        let stamp = kill_arrived(&mut h, 10_000, 10_400);
        assert_eq!(named(&stamp), None);
        assert_eq!(guessed(&stamp), Some(&combibo("Mature")));
    }

    #[test]
    fn a_creature_left_damaged_keeps_the_next_kill_to_a_guess() {
        // Young left at 0.4 at 3 s; Mature read from 4 s to 7 s and killed at
        // 7.4 s; Young bleeds out at 7.1 s (logged 7 s, arrived 7.2 s). The
        // player had clearly left Young, but it was left damaged.
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            1000,
            (0..3)
                .map(|k| seen(&c, "Combibo Young", Some(3), 0.8 - k as f64 * 0.2))
                .collect(),
        );
        feed(
            &mut h,
            4000,
            (0..4)
                .map(|k| seen(&c, "Combibo Mature", Some(4), 1.0 - k as f64 * 0.2))
                .collect(),
        );
        let stamp = kill_arrived(&mut h, 7000, 7200);
        assert_eq!(named(&stamp), None);
        assert_eq!(guessed(&stamp), Some(&combibo("Mature")));
        // Left at full health, it cannot have bled out: Mature is named.
        let mut h = history();
        feed(
            &mut h,
            1000,
            (0..3)
                .map(|_| seen(&c, "Combibo Young", Some(3), 1.0))
                .collect(),
        );
        feed(
            &mut h,
            4000,
            (0..4)
                .map(|k| seen(&c, "Combibo Mature", Some(4), 1.0 - k as f64 * 0.2))
                .collect(),
        );
        assert_eq!(
            named(&kill_arrived(&mut h, 7000, 7200)),
            Some(&combibo("Mature"))
        );
    }

    #[test]
    fn a_guess_leaves_its_fight_in_contention_for_later_kills() {
        // A bleeds from 6 s to 9 s; B is locked at 9.2 s and read at 10 s
        // and 11 s. A bleeds out at 11.05 s (logged 11 s, arrived 11.1 s);
        // B is killed at 11.3 s (logged 11 s, arrived 11.4 s).
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            6000,
            (0..4)
                .map(|k| seen(&c, "Combibo Young", Some(3), 0.8 - k as f64 * 0.2))
                .collect(),
        );
        feed(
            &mut h,
            10_000,
            vec![
                seen(&c, "Combibo Mature", Some(4), 1.0),
                seen(&c, "Combibo Mature", Some(4), 0.8),
            ],
        );
        let first = kill_arrived(&mut h, 11_000, 11_100);
        assert_eq!(named(&first), None);
        assert!(h.fights.iter().all(|f| !f.claimed));
        let second = kill_arrived(&mut h, 11_000, 11_400);
        assert_eq!(named(&second), None, "{second:?}");
        // A fight given to a kill as a guess gives later kills no more than
        // a guess, though it is the only one left.
        let mut h = history();
        feed(
            &mut h,
            0,
            (0..4)
                .map(|k| seen(&c, "Combibo Young", Some(3), 0.9 - k as f64 * 0.2))
                .collect(),
        );
        let i = h.fights.len() - 1;
        h.fights[i].given = Some(Given::Guess);
        let stamp = kill(&mut h, 4000);
        assert_eq!(guessed(&stamp), Some(&combibo("Young")));
    }

    #[test]
    fn an_unfamiliar_name_never_takes_another_maturitys_through_an_unread_level() {
        let c = catalogue(Vec::new());
        let fight = |h: &mut MobHistory, at: i64, text: &str, level: Option<u32>| {
            let last = feed(
                h,
                at,
                (0..3)
                    .map(|k| seen(&c, text, level, 0.9 - k as f64 * 0.2))
                    .collect(),
            );
            kill(h, last + 1000)
        };
        let provisional = |stamp: &KillStamp| match stamp {
            KillStamp::Stamped(MobStamp::Provisional { key, spelling, .. }) => {
                Some((key.clone(), spelling.clone()))
            }
            _ => None,
        };
        let suggested = |stamp: &KillStamp| match stamp {
            KillStamp::Stamped(MobStamp::ProvisionalGuess { key, spelling, .. }) => {
                Some((key.clone(), spelling.clone()))
            }
            _ => None,
        };
        // `Neo Alpha` at 30 first, then `Neo Old Alpha` whose level never
        // reads: its own name, and only a suggestion.
        let mut h = history();
        let alpha = provisional(&fight(&mut h, 0, "Neo Alpha", Some(30))).expect("provisional");
        assert_eq!(alpha.1, "Neo Alpha");
        let old = fight(&mut h, 20_000, "Neo Old Alpha", None);
        let (key, spelling) = suggested(&old).expect("a suggestion");
        assert_eq!(spelling, "Neo Old Alpha");
        assert_ne!(key, alpha.0);
        // The same text with its level unread is the name read so before.
        let again = fight(&mut h, 40_000, "Neo Alpha", None);
        assert_eq!(provisional(&again).map(|p| p.0), Some(alpha.0.clone()));
        // The other order: `Neo Alpha` at 30 is not taken for the name read
        // without a level.
        let mut h = history();
        let (old_key, _) =
            suggested(&fight(&mut h, 0, "Neo Old Alpha", None)).expect("a suggestion");
        let (key, spelling) =
            provisional(&fight(&mut h, 20_000, "Neo Alpha", Some(30))).expect("provisional");
        assert_eq!(spelling, "Neo Alpha");
        assert_ne!(key, old_key);
    }

    #[test]
    fn one_text_read_at_two_levels_is_two_names_with_two_keys() {
        let c = catalogue(Vec::new());
        let mut h = history();
        let mut keys = Vec::new();
        for (at, level) in [(0, 25), (20_000, 30)] {
            let last = feed(
                &mut h,
                at,
                (0..3)
                    .map(|k| seen(&c, "Zombie Fleshripper", Some(level), 0.9 - k as f64 * 0.2))
                    .collect(),
            );
            match kill(&mut h, last + 1000) {
                KillStamp::Stamped(MobStamp::Provisional { key, .. }) => keys.push(key),
                other => panic!("{other:?}"),
            }
        }
        assert_ne!(keys[0], keys[1]);
        let names = h.provisional_names();
        for key in &keys {
            assert_eq!(names.iter().filter(|(k, _)| k == key).count(), 1, "{key}");
        }
    }

    #[test]
    fn a_report_whose_arrival_is_too_far_from_its_logged_second_is_not_answered() {
        let p = bundled_profile().mob_history;
        let c = catalogue(Vec::new());
        let young = || {
            let mut h = history();
            feed(
                &mut h,
                0,
                (0..4)
                    .map(|k| seen(&c, "Combibo Young", Some(3), 0.9 - k as f64 * 0.2))
                    .collect(),
            );
            h
        };
        let logged = 4000;
        let latest = p.log_resolution_ms + p.report_delay_ms + p.report_slack_ms;
        for (lag, believed) in [
            (-p.report_early_ms, true),
            (-p.report_early_ms - 1, false),
            (latest, true),
            (latest + 1, false),
        ] {
            let stamp = kill_arrived(&mut young(), logged, logged + lag);
            assert_eq!(
                stamp == KillStamp::Unnamed(Unnamed::Implausible),
                !believed,
                "{lag}: {stamp:?}"
            );
        }
    }

    #[test]
    fn a_fight_forgotten_while_a_report_could_need_it_leaves_its_kills_unanswered() {
        let two_fights = || {
            let mut profile = bundled_profile();
            profile.mob_history.max_fights = 2;
            let mut h = MobHistory::new(profile.mob_history, profile.mob_names);
            for t in (-20_000..0).step_by(1000) {
                h.observe(t, None);
            }
            h
        };
        let c = catalogue(Vec::new());
        // Three fights a few seconds apart, kept to two: the first is
        // forgotten while a report could still need it.
        let mut h = two_fights();
        for (at, text, level) in [
            (0, "Combibo Young", 3),
            (4000, "Combibo Mature", 4),
            (8000, "Atrox Old", 6),
        ] {
            feed(
                &mut h,
                at,
                (0..2)
                    .map(|k| seen(&c, text, Some(level), 0.9 - k as f64 * 0.2))
                    .collect(),
            );
        }
        assert_eq!(h.fights.len(), 2);
        assert_eq!(kill(&mut h, 2000), KillStamp::Unnamed(Unnamed::Unwatched));
        // One no report can need any more goes without a trace.
        let mut h = two_fights();
        read_at(&mut h, 0, seen(&c, "Combibo Young", Some(3), 0.9));
        read_at(&mut h, 18_000, seen(&c, "Combibo Mature", Some(4), 0.9));
        read_at(&mut h, 19_000, seen(&c, "Atrox Old", Some(6), 1.0));
        assert_eq!(h.fights.len(), 2);
        assert!(h.lost.is_empty());
    }

    #[test]
    fn no_suggestion_while_another_creature_was_read_after_the_last_but_one_reading() {
        // Mature read at 4 s and 5 s, a Young glimpsed at 6 s, Mature again
        // at 7 s, read on the last pulse before the report arrived (logged 7
        // s, arrived 7.2 s): the Young stopped after Mature's last reading
        // but one, so nothing is suggested.
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            4000,
            vec![
                seen(&c, "Combibo Mature", Some(4), 0.9),
                seen(&c, "Combibo Mature", Some(4), 0.7),
                seen(&c, "Combibo Young", Some(3), 1.0),
                seen(&c, "Combibo Mature", Some(4), 0.5),
            ],
        );
        assert_eq!(h.fights.len(), 2);
        assert_eq!(
            kill_arrived(&mut h, 7000, 7200),
            KillStamp::Unnamed(Unnamed::Indistinct)
        );
    }

    #[test]
    fn a_creature_read_within_the_capture_slack_of_the_arrival_is_not_alive_after_it() {
        let p = bundled_profile().mob_history;
        let c = catalogue(Vec::new());
        // Young's last reading at 6 s; logged 5 s.
        let young = || {
            let mut h = history();
            feed(
                &mut h,
                3000,
                (0..4)
                    .map(|k| seen(&c, "Combibo Young", Some(3), 0.9 - k as f64 * 0.2))
                    .collect(),
            );
            h
        };
        let arrived = 6000 - p.capture_slack_ms;
        assert_eq!(
            named(&kill_arrived(&mut young(), 5000, arrived)),
            Some(&combibo("Young"))
        );
        assert_eq!(
            kill_arrived(&mut young(), 5000, arrived - 1),
            KillStamp::Unnamed(Unnamed::StillRead)
        );
    }

    #[test]
    fn a_creature_the_screen_leans_to_is_suggested_and_a_last_glimpse_is_not() {
        // A Zombie read at 164.6 s to 166.6 s, Combibo Mature locked and read
        // once at 167.6 s, nothing locked at 168.6 s, the Zombie read again at
        // 169.6 s and 170.6 s; logged 171 s, arrived 171.42 s. The Mature
        // stopped before the Zombie's last two readings, the last of which is
        // the last pulse before the report arrived: the Zombie is suggested.
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            164_600,
            vec![
                seen(&c, "Zombie Fleshripper", Some(25), 0.95),
                seen(&c, "Zombie Fleshripper", Some(25), 0.75),
                seen(&c, "Zombie Fleshripper", Some(25), 0.6),
                seen(&c, "Combibo Mature", Some(4), 1.0),
            ],
        );
        h.observe(168_600, None);
        feed(
            &mut h,
            169_600,
            vec![
                seen(&c, "Zombie Fleshripper", Some(25), 0.3),
                seen(&c, "Zombie Fleshriper", Some(25), 0.05),
            ],
        );
        match kill_arrived(&mut h, 171_000, 171_420) {
            KillStamp::Stamped(MobStamp::ProvisionalGuess { spelling, .. }) => {
                assert_eq!(spelling, "Zombie Fleshripper")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn fresh_creatures_locked_by_turns_at_full_health_stay_apart() {
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            0,
            vec![
                seen(&c, "Combibo Young", Some(3), 1.0),
                seen(&c, "Combibo Mature", Some(4), 1.0),
                seen(&c, "Combibo Young", Some(3), 1.0),
                seen(&c, "Combibo Mature", Some(4), 1.0),
            ],
        );
        assert_eq!(h.fights.len(), 2);
    }

    #[test]
    fn a_one_reading_glimpse_of_another_creature_is_passed_over_for_the_latest_creature_read() {
        // Timeline 3: Young read to 7 s, missed at 8 s, Mature glimpsed at
        // 9 s at full health, Young dies at 9.4 s, reported at 10 s. Nothing
        // is named or suggested; the glimpse is read once, so Young, read
        // last before it, is the latest creature read.
        let c = catalogue(Vec::new());
        let mut h = history();
        feed(
            &mut h,
            5000,
            vec![
                seen(&c, "Combibo Young", Some(3), 0.4),
                seen(&c, "Combibo Young", Some(3), 0.3),
                seen(&c, "Combibo Young", Some(3), 0.15),
            ],
        );
        h.observe(8000, None);
        read_at(&mut h, 9000, seen(&c, "Combibo Mature", Some(4), 1.0));
        assert_eq!(latest(&kill(&mut h, 10_000)), Some(&combibo("Young")));
    }

    #[test]
    fn an_unknown_creature_whose_name_is_near_a_known_one_is_never_named_as_it() {
        // `Atrox Elder` is not on the list; `Atrox Old` is, 78 similar. A
        // fresh `Atrox Old` is locked briefly mid-fight; the Elder's next
        // reading comes out near `Atrox Old` and joins the Old's fight, and
        // the Elder's later readings join that by health. The fight then
        // holds two creatures: nothing is named.
        let c = catalogue(Vec::new());
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Atrox Elder", Some(9), 0.9),
                seen(&c, "Atrox Elder", Some(9), 0.8),
                seen(&c, "Atrox Old", Some(6), 0.95),
                seen(&c, "Atrox Old", Some(6), 0.95),
                seen(&c, "Atrox Elder", Some(9), 0.7),
                seen(&c, "Atrox Older", None, 0.6),
                seen(&c, "Atrox Eldr", Some(9), 0.5),
                seen(&c, "Atrox Elder", Some(9), 0.4),
                seen(&c, "Atrox Elder", Some(9), 0.3),
                seen(&c, "Atrox Elder", Some(9), 0.2),
                seen(&c, "Atrox Elder", Some(9), 0.1),
            ],
        );
        assert_eq!(
            kill(&mut h, last + 1000),
            KillStamp::Unnamed(Unnamed::Undecided)
        );
    }

    #[test]
    fn the_same_misreading_on_every_reading_gives_a_guess_or_a_provisional_name_never_a_named_one()
    {
        let c = catalogue(Vec::new());
        // A misreading still near the name: a guess.
        let mut h = history();
        let readings = (0..5)
            .map(|k| seen(&c, "Combibo Yuong", Some(3), 1.0 - k as f64 * 0.2))
            .collect();
        let last = feed(&mut h, 0, readings);
        assert_eq!(guessed(&kill(&mut h, last + 1000)), Some(&combibo("Young")));
        // One too far off to match: the creature is taken for one the
        // application does not know, under that spelling.
        let mut h = history();
        let readings = (0..5)
            .map(|k| seen(&c, "Cornblbo Yuong", Some(3), 1.0 - k as f64 * 0.2))
            .collect();
        let last = feed(&mut h, 0, readings);
        match kill(&mut h, last + 1000) {
            KillStamp::Stamped(MobStamp::Provisional { key, spelling, .. }) => assert_eq!(
                (key.as_str(), spelling.as_str()),
                ("CORNBLBOYUONG L3 #0", "Cornblbo Yuong")
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_valid_level_belonging_to_another_maturity_names_nothing() {
        // `Combibo Young` read every time with Mature's level.
        let c = catalogue(Vec::new());
        let mut h = history();
        let readings = (0..5)
            .map(|k| seen(&c, "Combibo Young", Some(4), 1.0 - k as f64 * 0.2))
            .collect();
        let last = feed(&mut h, 0, readings);
        assert_eq!(
            kill(&mut h, last + 1000),
            KillStamp::Unnamed(Unnamed::Undecided)
        );
        // `Old` read off an `Old Alpha` plate likewise.
        let mut h = history();
        let readings = (0..4)
            .map(|k| seen(&c, "Snarg Old", Some(19), 1.0 - k as f64 * 0.25))
            .collect();
        let last = feed(&mut h, 0, readings);
        assert_eq!(
            kill(&mut h, last + 1000),
            KillStamp::Unnamed(Unnamed::Undecided)
        );
    }

    #[test]
    fn near_identical_maturities_are_each_named_as_themselves() {
        let c = catalogue(Vec::new());
        // An Old Alpha, one reading of which lost `Alpha` (and is unclear
        // for carrying the Old Alpha's level), then an Old.
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Snarg Old Alpha", Some(19), 0.9),
                seen(&c, "Snarg Old", Some(19), 0.7),
                seen(&c, "Snarg 0ld Alpha", Some(19), 0.5),
                seen(&c, "Snarg Old Alpha", Some(19), 0.3),
            ],
        );
        match kill(&mut h, last + 1000) {
            KillStamp::Stamped(MobStamp::Named { creature, evidence }) => {
                assert_eq!(creature, creature_of("Snarg", "Old Alpha"));
                assert_eq!((evidence.votes, evidence.of), (3, 3));
            }
            other => panic!("{other:?}"),
        }
        let last = feed(
            &mut h,
            last + 10_000,
            vec![
                seen(&c, "Snarg Old", Some(14), 0.8),
                seen(&c, "Snarg 0ld", Some(14), 0.4),
                seen(&c, "Snarg Old", Some(14), 0.1),
            ],
        );
        assert_eq!(
            named(&kill(&mut h, last + 1000)),
            Some(&creature_of("Snarg", "Old"))
        );
        // A Gen 01, one reading of which took the 1 for a 7 (with its
        // level, so it is unclear, not Gen 07).
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Drone Gen 01", Some(10), 0.9),
                seen(&c, "Drone Gen 0l", Some(10), 0.6),
                seen(&c, "Drone Gen 07", Some(10), 0.4),
                seen(&c, "Drone Gen 01", Some(10), 0.1),
            ],
        );
        match kill(&mut h, last + 1000) {
            KillStamp::Stamped(MobStamp::Named { creature, evidence }) => {
                assert_eq!(creature, creature_of("Drone", "Gen 01"));
                assert_eq!((evidence.votes, evidence.of), (3, 3));
            }
            other => panic!("{other:?}"),
        }
    }

    fn creature_of(name: &str, maturity: &str) -> Creature {
        creature(name, maturity)
    }

    #[test]
    fn a_loose_match_can_support_only_a_guess_and_never_against_a_strict_reading() {
        // `Combibo Yuong` with one more slip is only loosely Young.
        let c = catalogue(Vec::new());
        let loose = "Cornbibo Yuong";
        assert!(matches!(
            resolve(loose, Some(3), &c, &bundled_profile().mob_names),
            Resolved::Known {
                spelling: Spelling::Loose,
                ..
            }
        ));
        // Loose readings alone: a guess.
        let mut h = history();
        let readings = (0..4)
            .map(|k| seen(&c, loose, Some(3), 1.0 - k as f64 * 0.3))
            .collect();
        let last = feed(&mut h, 0, readings);
        assert_eq!(guessed(&kill(&mut h, last + 1000)), Some(&combibo("Young")));
        // With a strict reading of Mature in the fight: its readings name two
        // creatures, so nothing.
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, loose, Some(3), 0.9),
                seen(&c, "Combibo Mature", Some(4), 0.7),
                seen(&c, loose, Some(3), 0.5),
                seen(&c, loose, Some(3), 0.2),
            ],
        );
        assert_eq!(
            kill(&mut h, last + 1000),
            KillStamp::Unnamed(Unnamed::Indistinct)
        );
        // An exact reading and loose ones, not enough strict votes alone:
        // a guess, never named on the strength of loose readings.
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Combibo Young", Some(3), 0.9),
                seen(&c, "Zombie Fleshripper", Some(3), 0.7),
                seen(&c, loose, Some(3), 0.5),
                seen(&c, loose, Some(3), 0.3),
            ],
        );
        match kill(&mut h, last + 1000) {
            KillStamp::Stamped(MobStamp::Guess { creature, evidence }) => {
                assert_eq!(creature, combibo("Young"));
                assert_eq!((evidence.votes, evidence.of), (3, 4));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_confirmed_name_read_with_its_level_is_named() {
        let c = catalogue(vec!["Zombie Fleshripper".into()]);
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Zombie Fleshripper", Some(25), 0.9),
                seen(&c, "Zombie Fleshriper", Some(25), 0.6),
                seen(&c, "Zombie Fleshripper", Some(25), 0.2),
            ],
        );
        assert_eq!(
            named(&kill(&mut h, last + 1000)),
            Some(&Creature::Confirmed {
                name: "Zombie Fleshripper".into()
            })
        );
    }

    #[test]
    fn spellings_folded_onto_the_leader_carry_the_vote_but_cannot_earn_a_name() {
        // Read with a level the list does not give Young, a near spelling
        // is refused as a name on its own; in a fight Young leads, it votes
        // for Young, but Young's own strict readings are too few to name it.
        // A spelling further off than a name alone may be stays apart.
        let c = catalogue(Vec::new());
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Combibo Young", Some(3), 1.0),
                seen(&c, "Combibo Young", Some(3), 0.8),
                seen(&c, "Combibo Yuong", Some(7), 0.6),
                seen(&c, "Cornblbo Yuong", Some(3), 0.4),
                seen(&c, "Combibo Young", Some(3), 0.1),
            ],
        );
        match kill(&mut h, last + 1000) {
            KillStamp::Stamped(MobStamp::Guess { creature, evidence }) => {
                assert_eq!(creature, combibo("Young"));
                assert_eq!((evidence.votes, evidence.of), (4, 5));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_fight_split_between_a_known_creature_and_an_unfamiliar_name_names_nothing() {
        let c = catalogue(Vec::new());
        let mut h = history();
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Combibo Young", Some(3), 0.9),
                seen(&c, "Zombie Fleshripper", Some(3), 0.7),
                seen(&c, "Combibo Young", Some(3), 0.5),
                seen(&c, "Zombie Fleshripper", Some(3), 0.3),
                seen(&c, "Combibo Young", Some(3), 0.1),
            ],
        );
        assert_eq!(
            kill(&mut h, last + 1000),
            KillStamp::Unnamed(Unnamed::Undecided)
        );
    }

    #[test]
    fn unfamiliar_names_keep_one_key_all_session_and_maturities_of_other_levels_stay_apart() {
        let c = catalogue(Vec::new());
        let mut h = history();
        let mut keys = Vec::new();
        let mut at = 0;
        for (texts, level) in [
            (
                [
                    "Zombie Fleshriper",
                    "Zombie Fleshripper",
                    "Zombie Fleshripper",
                ],
                25,
            ),
            (["Zombie Young", "Zombie Young", "Zombie Yung"], 5),
            (["Zombie Old", "Zombie Old", "Zombie Old"], 8),
            (
                [
                    "Zombie Fleshripper",
                    "Zombic Fleshripper",
                    "Zombie Fleshripper",
                ],
                25,
            ),
        ] {
            let readings = texts
                .iter()
                .enumerate()
                .map(|(k, t)| seen(&c, t, Some(level), 0.9 - 0.2 * k as f64))
                .collect();
            let last = feed(&mut h, at, readings);
            match kill(&mut h, last + 1000) {
                KillStamp::Stamped(MobStamp::Provisional { key, .. }) => keys.push(key),
                other => panic!("{texts:?}: {other:?}"),
            }
            at = last + 10_000;
        }
        // The same creature early and late, under its first spelling's key.
        assert_eq!(keys[0], keys[3]);
        assert_eq!(keys[0], "ZOMBIEFLESHRIPER L25 #0");
        assert!(keys[1] != keys[2] && keys[1] != keys[0] && keys[2] != keys[0]);
        let names: std::collections::HashMap<String, String> =
            h.provisional_names().into_iter().collect();
        assert_eq!(names[&keys[0]], "Zombie Fleshripper");
        assert_eq!(names[&keys[1]], "Zombie Young");
        assert_eq!(names[&keys[2]], "Zombie Old");
    }

    #[test]
    fn a_pulse_with_nothing_locked_ends_the_fight() {
        // The plate gone for a pulse: whatever is read next is a lock begun
        // afresh, even of the same creature at the same health (a fresh
        // creature can be read at low health only by joining a dead fight).
        let c = catalogue(Vec::new());
        let mut h = history();
        h.observe(0, Some(seen(&c, "Combibo Young", Some(3), 0.6)));
        h.observe(1000, None);
        h.observe(2000, Some(seen(&c, "Combibo Young", Some(3), 0.55)));
        h.observe(3000, Some(seen(&c, "Combibo Young", Some(3), 0.05)));
        assert_eq!(h.fights.len(), 2);
        // Both name one creature: the kill takes the one nearest the death.
        match kill(&mut h, 4000) {
            KillStamp::Stamped(MobStamp::Named { creature, evidence }) => {
                assert_eq!(creature, combibo("Young"));
                assert_eq!(evidence.first_read_ms, 2000);
            }
            other => panic!("{other:?}"),
        }
        // A pulse out of time order is ignored.
        let last = *h.pulses.back().expect("pulses");
        h.observe(last - 500, Some(seen(&c, "Combibo Mature", Some(4), 0.9)));
        assert_eq!(h.fights.len(), 2);
        assert_eq!(h.pulses.back(), Some(&last));
    }

    /// A name of letters only for `k`, so that every one resolves as an
    /// unfamiliar name: `Creature Baab`.
    fn letters(k: u32) -> String {
        let mut word = String::new();
        let mut n = k;
        for _ in 0..4 {
            word.push((b'a' + (n % 26) as u8) as char);
            n /= 26;
        }
        format!("Creature B{word}")
    }

    #[test]
    fn the_history_keeps_only_what_a_kill_could_still_need() {
        let profile = bundled_profile().mob_history;
        let c = catalogue(Vec::new());
        let mut h = history();
        // An hour of creatures of a new name each, every one at a level of
        // its own so none is taken for another.
        for k in 0..3600i64 {
            let reading = seen(
                &c,
                &letters(k as u32),
                Some(k as u32 + 1),
                1.0 - (k % 10) as f64 / 10.0,
            );
            assert!(matches!(reading.resolved, Resolved::New { .. }));
            h.observe(k * 1000, Some(reading));
        }
        assert!(h.fights.len() <= profile.max_fights);
        assert!(h.unfamiliar.len() <= profile.spellings);
        assert!(h.pulses.len() <= profile.max_pulses);
        assert!(h
            .fights
            .iter()
            .all(|f| f.readings.len() <= profile.max_reads));
        // A long fight keeps only its latest readings.
        let mut h = history();
        for k in 0..1000i64 {
            h.observe(k * 1000, Some(seen(&c, "Combibo Young", Some(3), 0.5)));
        }
        assert_eq!(h.fights.len(), 1);
        assert_eq!(h.fights[0].readings.len(), profile.max_reads);
        assert_eq!(
            h.fights[0].tally.iter().map(|t| t.1).sum::<usize>(),
            profile.max_reads
        );
    }

    #[test]
    fn an_unfamiliar_name_a_kill_was_given_outlives_the_cap_of_those_none_was() {
        let mut profile = bundled_profile();
        profile.mob_history.spellings = 3;
        let mut h = MobHistory::new(profile.mob_history, profile.mob_names);
        for t in (-20_000..0).step_by(1000) {
            h.observe(t, None);
        }
        let c = catalogue(Vec::new());
        let last = feed(
            &mut h,
            0,
            vec![
                seen(&c, "Zombie Fleshripper", Some(25), 0.6),
                seen(&c, "Zombie Fleshripper", Some(25), 0.4),
            ],
        );
        let key = match kill(&mut h, last + 1000) {
            KillStamp::Stamped(MobStamp::Provisional { key, .. }) => key,
            other => panic!("{other:?}"),
        };
        // Many other unfamiliar names, each read once, long after.
        for k in 0..20u32 {
            let t = 30_000 + 30_000 * k as i64;
            read_at(&mut h, t, seen(&c, &letters(k), Some(k + 1), 1.0));
        }
        let names: Vec<String> = h.provisional_names().into_iter().map(|n| n.0).collect();
        assert!(names.contains(&key), "{names:?}");
        assert!(h.unfamiliar.iter().filter(|u| !u.stamped).count() <= 3);
    }

    #[test]
    fn a_sighting_takes_the_level_and_name_that_read() {
        let names = bundled_profile().mob_names;
        let catalogue = MobCatalogue::from_entities(&bundled_mobs(), Vec::new(), &names);
        let read = PlateRead {
            bar: Rect::new(0, 0, 163, 5),
            fill: 0.5,
            level: Err(PlateTextError::Invalid { raw: "125".into() }),
            name: Ok("Combibo Young".into()),
        };
        let sighting = Sighting::of(&read, &catalogue, &names);
        assert_eq!(sighting.fill, 0.5);
        assert_eq!(sighting.raw_name.as_deref(), Some("Combibo Young"));
        assert!(matches!(
            sighting.resolved,
            Resolved::Known {
                spelling: Spelling::Exact,
                ..
            }
        ));
        let unread = PlateRead {
            name: Err(PlateTextError::Covered),
            ..read
        };
        assert_eq!(
            Sighting::of(&unread, &catalogue, &names),
            Sighting {
                fill: 0.5,
                raw_name: None,
                resolved: Resolved::Unclear
            }
        );
    }

    // The simulated hunt: a seeded script of fights drawn frame by frame,
    // the locked plate found and read on the pixels, the recogniser's
    // mistakes modelled on its measured rates and repeated per creature
    // (it reads the same lettering the same way), and every kill stamped
    // from the history once the screen has been read far enough past it.

    use std::sync::Mutex;

    use crate::game_ui::geometry::BgrImage;
    use crate::game_ui::plates::{
        find_locked_plate, find_plates, plates_near, read_plate, PlateKind, PlateScratch,
    };
    use crate::game_ui::test_support::{scenery, Lcg, LineAlign, PlateDrawing, RadarScene};

    /// A kind of creature the hunt meets.
    struct Kind {
        name: &'static str,
        maturity: Option<&'static str>,
        level: u32,
        /// Whether the bundled creature list has it.
        listed: bool,
        /// Whether its name ever reads, and its level.
        name_reads: bool,
        level_reads: bool,
    }

    impl Kind {
        fn plate(&self) -> String {
            match self.maturity {
                Some(m) => format!("{} {m}", self.name),
                None => self.name.to_string(),
            }
        }
    }

    /// Two maturities of one species, two other creatures, one the bundled
    /// list does not have, two maturities of another it does not have (the
    /// level of one of which never reads), and one whose name never reads.
    const KINDS: [Kind; 8] = [
        Kind {
            name: "Combibo",
            maturity: Some("Young"),
            level: 3,
            listed: true,
            name_reads: true,
            level_reads: true,
        },
        Kind {
            name: "Combibo",
            maturity: Some("Mature"),
            level: 4,
            listed: true,
            name_reads: true,
            level_reads: true,
        },
        Kind {
            name: "Allophyl",
            maturity: Some("Old"),
            level: 18,
            listed: true,
            name_reads: true,
            level_reads: true,
        },
        Kind {
            name: "Molisk",
            maturity: Some("Hunter"),
            level: 27,
            listed: true,
            name_reads: true,
            level_reads: true,
        },
        Kind {
            name: "Zombie Fleshripper",
            maturity: None,
            level: 25,
            listed: false,
            name_reads: true,
            level_reads: true,
        },
        Kind {
            name: "Qwerl",
            maturity: Some("Alpha"),
            level: 30,
            listed: false,
            name_reads: true,
            level_reads: true,
        },
        Kind {
            name: "Qwerl",
            maturity: Some("Old Alpha"),
            level: 33,
            listed: false,
            name_reads: true,
            level_reads: false,
        },
        Kind {
            name: "Combibo",
            maturity: Some("Dominant"),
            level: 6,
            listed: true,
            name_reads: false,
            level_reads: true,
        },
    ];

    /// The kind whose name never reads.
    const UNREADABLE: usize = 7;

    /// How often each kind is hunted: in a harsh hunt, and in an ordinary
    /// one (without the unlisted pair).
    const MIX: [usize; 11] = [0, 0, 0, 1, 1, 2, 4, 4, 3, 5, 6];
    const ORDINARY_MIX: [usize; 9] = [0, 0, 0, 1, 1, 2, 4, 4, 3];

    /// A stretch of time one creature is locked, its health going from
    /// one share to another; `last_of` names the kill whose final stretch
    /// it is.
    struct Lock {
        from: f64,
        to: f64,
        kind: usize,
        fill: (f64, f64),
        last_of: Option<usize>,
    }

    /// What a kill is, for telling how it should come out.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Category {
        /// An ordinary fight to the death (with or without a brief lock
        /// on another creature mid-fight).
        Plain,
        /// Its last reading lost its last word.
        Truncated,
        /// Followed within 0.6 s by a lock on a creature of another kind:
        /// both are read in the kill's moments.
        DifferentRelock,
        /// One of two kills under two seconds apart, of two maturities.
        FastAlternating,
        /// Killed in a moment, read once at most.
        OneShot,
        /// Its last reading read exactly as another listed creature.
        LastMisread,
        /// A creature never locked, killed by area damage mid-fight.
        AreaDamage,
        /// A creature whose name never reads, locked at full health in the
        /// middle of another fight and killed.
        Unreadable,
        /// A creature left bleeding, dying under a second before the next
        /// one, locked since, is killed.
        Bleed,
        /// The creature killed under a second after another bled out.
        CloseDeaths,
    }

    impl Category {
        /// Whether the kill is reported apart from those the screen can
        /// tell: killed in a moment, its last reading altered, never locked
        /// or never read, not locked when it died, or within a second of
        /// such a death.
        fn ambiguous(self) -> bool {
            matches!(
                self,
                Category::OneShot
                    | Category::LastMisread
                    | Category::Truncated
                    | Category::AreaDamage
                    | Category::Unreadable
                    | Category::Bleed
                    | Category::CloseDeaths
            )
        }
    }

    /// How a kill's last reading is misread, if it is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LastRead {
        Plain,
        As(usize),
        Truncated,
    }

    struct Kill {
        /// The creature killed; none for one never locked.
        kind: Option<usize>,
        /// From the fight's first lock to the death.
        fight: (f64, f64),
        /// When the application reads the chat log's line for it: at most
        /// the longest report delay after the death. The line's stamp is the
        /// whole second it falls in.
        reported: f64,
        category: Category,
        last_read: LastRead,
    }

    struct Script {
        locks: Vec<Lock>,
        kills: Vec<Kill>,
        end: f64,
        brief_locks: usize,
        quick_relocks: usize,
    }

    /// A hunt of `kills` fights: the kind sometimes changes between
    /// fights; about one fight in seven is broken by a brief lock on
    /// another creature; about three kills in ten are followed by a lock
    /// on the next creature within 0.6 s. Some fights are one-shot kills,
    /// most of them followed at once by a lock on another of the same kind;
    /// some are a pair of kills of two maturities under two seconds apart;
    /// some end on a reading misread as another listed creature or cut
    /// short; during some long fights a creature never locked dies by area
    /// damage; in some the player switches at full health to a creature
    /// whose name never reads and kills that; and in some the player leaves
    /// a creature bleeding for another, and the two die under a second
    /// apart. An ordinary hunt (`harsh` false) has none of the area damage,
    /// the switches to a creature whose name never reads, the bleeding, the
    /// unlisted pair, or the lock on the same kind after a one-shot kill.
    fn script(rng: &mut Lcg, kills: usize, harsh: bool) -> Script {
        let mix: &[usize] = if harsh { &MIX } else { &ORDINARY_MIX };
        let pick = |rng: &mut Lcg| mix[rng.range(0, mix.len() as i32 - 1) as usize];
        let mut out = Script {
            locks: Vec::new(),
            kills: Vec::new(),
            end: 0.0,
            brief_locks: 0,
            quick_relocks: 0,
        };
        let mut t = 2.0;
        let mut kind = pick(rng);
        // The kind and length forced on the next fight, for a fast pair.
        let mut forced: Option<(usize, f64)> = None;
        // The next fight is of the same kind, as after a one-shot kill.
        let mut same_kind = false;
        for _ in 0..kills {
            let index = out.kills.len();
            let special = rng.unit();
            let keep = std::mem::take(&mut same_kind);
            if harsh && forced.is_none() && (0.34..0.44).contains(&special) {
                // Another creature first: left at 0.3 for one whose name
                // never reads, or left bleeding for one killed just after it.
                let unreadable = special < 0.39;
                let first = rng.uniform(3.0, 6.0);
                let left_at = if unreadable { 0.3 } else { 0.2 };
                out.locks.push(Lock {
                    from: t,
                    to: t + first,
                    kind,
                    fill: (1.0, left_at),
                    last_of: None,
                });
                let from = t + first + rng.uniform(0.2, 0.5);
                let next = if unreadable {
                    UNREADABLE
                } else {
                    loop {
                        let other = pick(rng);
                        if other != kind {
                            break other;
                        }
                    }
                };
                let length = rng.uniform(1.5, 4.0);
                let death = from + length;
                out.locks.push(Lock {
                    from,
                    to: death,
                    kind: next,
                    fill: (1.0, 0.0),
                    last_of: Some(out.kills.len() + usize::from(!unreadable)),
                });
                if !unreadable {
                    let bled = (death - rng.uniform(0.2, 0.9)).max(t + first + 0.05);
                    out.kills.push(Kill {
                        kind: Some(kind),
                        fight: (t, bled),
                        reported: bled + rng.uniform(0.0, 2.0),
                        category: Category::Bleed,
                        last_read: LastRead::Plain,
                    });
                }
                out.kills.push(Kill {
                    kind: Some(next),
                    fight: (from, death),
                    reported: death + rng.uniform(0.0, 2.0),
                    category: if unreadable {
                        Category::Unreadable
                    } else {
                        Category::CloseDeaths
                    },
                    last_read: LastRead::Plain,
                });
                if !unreadable {
                    kind = next;
                }
                t = death + rng.uniform(1.5, 6.0);
                continue;
            }
            let (length, category) = match forced.take() {
                Some((k, length)) => {
                    kind = k;
                    (length, Category::FastAlternating)
                }
                None => {
                    if !keep && rng.unit() < 0.3 {
                        kind = pick(rng);
                    }
                    if special < 0.08 {
                        (rng.uniform(0.3, 0.9), Category::OneShot)
                    } else {
                        (rng.uniform(4.0, 14.0), Category::Plain)
                    }
                }
            };
            let last_read = if category != Category::Plain {
                LastRead::Plain
            } else if (0.14..0.20).contains(&special) {
                let other = if kind <= 1 { 1 - kind } else { (kind + 1) % 4 };
                LastRead::As(other)
            } else if (0.20..0.26).contains(&special) {
                LastRead::Truncated
            } else {
                LastRead::Plain
            };
            let category = match last_read {
                LastRead::As(_) => Category::LastMisread,
                LastRead::Truncated => Category::Truncated,
                LastRead::Plain => category,
            };
            let detour = category == Category::Plain && rng.unit() < 1.0 / 7.0;
            let death = if detour {
                out.brief_locks += 1;
                let half = length * rng.uniform(0.3, 0.6);
                let other = loop {
                    let other = pick(rng);
                    if other != kind {
                        break other;
                    }
                };
                let brief = rng.uniform(1.0, 3.0);
                out.locks.push(Lock {
                    from: t,
                    to: t + half,
                    kind,
                    fill: (1.0, 0.5),
                    last_of: None,
                });
                out.locks.push(Lock {
                    from: t + half + 0.2,
                    to: t + half + 0.2 + brief,
                    kind: other,
                    fill: (1.0, rng.uniform(0.8, 1.0)),
                    last_of: None,
                });
                let back = t + half + 0.4 + brief;
                out.locks.push(Lock {
                    from: back,
                    to: back + length - half,
                    kind,
                    fill: (0.5, 0.0),
                    last_of: Some(index),
                });
                back + length - half
            } else {
                out.locks.push(Lock {
                    from: t,
                    to: t + length,
                    kind,
                    fill: (1.0, 0.0),
                    last_of: Some(index),
                });
                t + length
            };
            out.kills.push(Kill {
                kind: Some(kind),
                fight: (t, death),
                reported: death + rng.uniform(0.0, 2.0),
                category,
                last_read,
            });
            // Area damage kills a creature never locked during a long fight.
            if harsh && category == Category::Plain && length > 6.0 && rng.unit() < 0.1 {
                let at = t + length * rng.uniform(0.3, 0.6);
                out.kills.push(Kill {
                    kind: None,
                    fight: (at, at),
                    reported: at + rng.uniform(0.0, 2.0),
                    category: Category::AreaDamage,
                    last_read: LastRead::Plain,
                });
            }
            // A fast pair: the next fight is the other maturity, locked at
            // once and killed within two seconds.
            if forced.is_none()
                && category != Category::FastAlternating
                && kind <= 1
                && (0.26..0.34).contains(&special)
            {
                forced = Some((1 - kind, rng.uniform(1.2, 1.9)));
            }
            same_kind = harsh && category == Category::OneShot && rng.unit() < 0.6;
            let quick = forced.is_some() || same_kind || rng.unit() < 0.3;
            t = death
                + if quick {
                    out.quick_relocks += 1;
                    rng.uniform(0.1, 0.6)
                } else {
                    rng.uniform(1.5, 6.0)
                };
        }
        // A kill followed at once by a lock on another kind.
        let starts: Vec<(f64, usize)> = out.locks.iter().map(|l| (l.from, l.kind)).collect();
        for kill in out.kills.iter_mut() {
            let Some(kind) = kill.kind else { continue };
            let relock = starts
                .iter()
                .any(|(from, k)| *from > kill.fight.1 && *from <= kill.fight.1 + 0.6 && *k != kind);
            if relock && matches!(kill.category, Category::Plain | Category::Truncated) {
                kill.category = Category::DifferentRelock;
            }
        }
        let last_report = out.kills.iter().map(|k| k.reported).fold(t, f64::max);
        out.end = last_report + 6.0;
        out
    }

    /// The recogniser's confusions between characters.
    const CONFUSIONS: [(&str, &str); 13] = [
        ("l", "I"),
        ("I", "l"),
        ("O", "0"),
        ("0", "O"),
        ("rn", "m"),
        ("m", "rn"),
        ("e", "c"),
        ("a", "o"),
        ("i", "l"),
        ("t", "f"),
        ("h", "b"),
        ("S", "5"),
        ("B", "8"),
    ];

    /// `text` with `k` mistakes: a confusion, a dropped character or a
    /// wrong one.
    fn misread(text: &str, k: usize, rng: &mut Lcg) -> String {
        let mut s = text.to_string();
        for _ in 0..k {
            let mut chars: Vec<char> = s.chars().collect();
            let i = rng.range(0, chars.len() as i32 - 1) as usize;
            let op = rng.unit();
            if op < 0.5 {
                let found: Vec<&(&str, &str)> =
                    CONFUSIONS.iter().filter(|(a, _)| s.contains(a)).collect();
                if !found.is_empty() {
                    let (a, b) = found[rng.range(0, found.len() as i32 - 1) as usize];
                    s = s.replacen(a, b, 1);
                    continue;
                }
            }
            if op < 0.75 && chars.len() > 4 {
                chars.remove(i);
            } else {
                let letters: Vec<char> = "abcdefghilmnorstu".chars().collect();
                chars[i] = letters[rng.range(0, letters.len() as i32 - 1) as usize];
            }
            s = chars.into_iter().collect();
        }
        s
    }

    /// How the recogniser misreads each kind's name: a few misreadings it
    /// makes again and again, most often the first.
    fn misreadings(rng: &mut Lcg) -> Vec<Vec<String>> {
        KINDS
            .iter()
            .map(|kind| {
                (0..4)
                    .map(|_| {
                        let k = if rng.unit() < 0.6 { 1 } else { 2 };
                        misread(&kind.plate(), k, rng)
                    })
                    .collect()
            })
            .collect()
    }

    /// The name as the recogniser reads it: whole 37.5% of the time, one of
    /// its usual misreadings (one or two characters off) 56%, and garbled
    /// 6.5%.
    fn read_name(kind: usize, usual: &[Vec<String>], rng: &mut Lcg) -> String {
        let u = rng.unit();
        if u < 0.375 {
            KINDS[kind].plate()
        } else if u < 0.935 {
            let v = rng.unit();
            let which = if v < 0.4 {
                0
            } else if v < 0.7 {
                1
            } else if v < 0.9 {
                2
            } else {
                3
            };
            usual[kind][which].clone()
        } else {
            misread(&KINDS[kind].plate(), 5, rng)
        }
    }

    /// What became of one kill.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Outcome {
        NamedRight,
        NamedWrong,
        GuessRight,
        GuessWrong,
        ProvisionalRight,
        ProvisionalWrong,
        SuggestedRight,
        SuggestedWrong,
        /// The creature killed under a misspelling of its name.
        ProvisionalMisspelt,
        SuggestedMisspelt,
        /// The lowest grade, known or unfamiliar.
        LatestRight,
        LatestWrong,
        LatestMisspelt,
        Unnamed(UnnamedReason),
    }

    /// [`Unnamed`], ordered for counting.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum UnnamedReason {
        NoFight,
        StillRead,
        Indistinct,
        Undecided,
        Unwatched,
        Implausible,
    }

    impl Outcome {
        /// Stamped with the right creature: named, or provisional.
        fn stamped_right(self) -> bool {
            matches!(self, Outcome::NamedRight | Outcome::ProvisionalRight)
        }

        /// Right, counting suggestions (guesses) and the lowest grade too.
        fn right(self) -> bool {
            self.stamped_right()
                || matches!(
                    self,
                    Outcome::GuessRight | Outcome::SuggestedRight | Outcome::LatestRight
                )
        }
    }

    /// What became of the kills of one hunt.
    #[derive(Default, Debug)]
    struct Tally {
        outcomes: Vec<(Category, Outcome, String)>,
        off_screen: Vec<String>,
        pulses_locked: usize,
        locked_read: usize,
        false_locks: usize,
        bystanders: usize,
        bystanders_found: usize,
        brief_locks: usize,
        quick_relocks: usize,
    }

    impl Tally {
        /// Outcome counts among kills `which` admits.
        fn counts(&self, which: impl Fn(Category) -> bool) -> Vec<(Outcome, usize)> {
            let mut counts: std::collections::BTreeMap<Outcome, usize> = Default::default();
            for (category, outcome, _) in &self.outcomes {
                if which(*category) {
                    *counts.entry(*outcome).or_default() += 1;
                }
            }
            counts.into_iter().collect()
        }
    }

    /// The kind a known creature is, if the hunt has it.
    fn kind_of(creature: &Creature) -> Option<usize> {
        KINDS.iter().position(|kind| match creature {
            Creature::Catalogued { name, maturity, .. } => {
                kind.listed && kind.name == name && kind.maturity == maturity.as_deref()
            }
            Creature::Confirmed { .. } | Creature::Species { .. } => false,
        })
    }

    /// A hunt of `kills` fights, harsh (see [`script`]: and with names that
    /// sometimes do not read and levels sometimes missing from the line) or
    /// ordinary.
    fn hunt(seed: u64, kills: usize, line: LineAlign, harsh: bool) -> Tally {
        let profile = bundled_profile();
        let catalogue =
            MobCatalogue::from_entities(&bundled_mobs(), Vec::new(), &profile.mob_names);
        for kind in KINDS.iter().filter(|k| !k.listed) {
            assert!(
                catalogue.mobs().iter().all(|m| m.name != kind.name),
                "the hunt's unlisted {} is not on the list",
                kind.name
            );
        }
        let mut rng = Lcg(seed);
        let plan = script(&mut rng, kills, harsh);
        let usual = misreadings(&mut rng);
        // How often each kind's name does not read at all, besides the
        // chance any pulse has.
        let unread: Vec<f64> = KINDS.iter().map(|_| rng.uniform(0.0, 0.15)).collect();
        let mut tally = Tally {
            brief_locks: plan.brief_locks,
            quick_relocks: plan.quick_relocks,
            ..Tally::default()
        };
        let (w, h) = (640, 300);
        let grounds: Vec<BgrImage> = (0..3).map(|k| scenery(w, h, seed * 10 + k)).collect();
        let bystanders = [
            (MIX[rng.range(0, 8) as usize], (12, 60)),
            (MIX[rng.range(0, 8) as usize], (30, 220)),
        ];
        let wait = profile.mob_history.capture_slack_ms + profile.mob_history.kill_lookahead_ms;
        let report_of = |kill: &Kill| KillReport {
            logged_ms: kill.reported.floor() as i64 * 1000,
            arrived_ms: (kill.reported * 1000.0).round() as i64,
        };
        let mut history = MobHistory::new(profile.mob_history.clone(), profile.mob_names.clone());
        let mut resolved: std::collections::HashMap<(String, Option<u32>), Resolved> =
            std::collections::HashMap::new();
        let mut order: Vec<usize> = (0..plan.kills.len()).collect();
        order.sort_by(|a, b| plan.kills[*a].reported.total_cmp(&plan.kills[*b].reported));
        let mut stamps: Vec<(usize, KillStamp)> = Vec::new();
        let mut position = (360, 150);
        let mut next = 0;
        let mut observed = 0i64;
        let mut scratch = PlateScratch::default();
        let phase = rng.unit();
        let mut i = 0usize;
        loop {
            let t = phase + i as f64 + rng.uniform(-0.04, 0.04);
            if t > plan.end {
                break;
            }
            while next < order.len() {
                let k = order[next];
                let report = report_of(&plan.kills[k]);
                if observed < report.arrived_ms + wait {
                    break;
                }
                let verdict = history.stamp_for_kill(report, observed);
                stamps.push((k, verdict));
                next += 1;
            }
            let mut scene = RadarScene::over(grounds[(i / 200) % grounds.len()].clone());
            let shown = (i / 37) % 3;
            for (kind, at) in bystanders.iter().take(shown) {
                let kind = &KINDS[*kind];
                scene.plate(
                    &PlateDrawing {
                        line,
                        ..PlateDrawing::washed(*at)
                    },
                    &format!("L{}", kind.level),
                    &kind.plate(),
                );
            }
            // The locked creature's health drops in the bar's visible
            // steps; a creature at none is dead and has no plate.
            let truth = plan
                .locks
                .iter()
                .find(|l| l.from <= t && t < l.to)
                .map(|l| {
                    let fill = l.fill.0 + (l.fill.1 - l.fill.0) * (t - l.from) / (l.to - l.from);
                    // Its last reading, when the kill's last one is misread.
                    let last = l
                        .last_of
                        .filter(|_| t >= l.to - 1.2)
                        .map(|k| plan.kills[k].last_read)
                        .unwrap_or(LastRead::Plain);
                    (l.kind, (fill * 20.0).round() / 20.0, last)
                })
                .filter(|(_, fill, _)| *fill > 0.02);
            if let Some((kind, fill, _)) = truth {
                position = (
                    (position.0 + rng.range(-25, 25)).clamp(290, 450),
                    (position.1 + rng.range(-15, 15)).clamp(40, 270),
                );
                let k = &KINDS[kind];
                // Now and then the level is missing from the line, and the
                // name's first word stands where it would be.
                let plate = k.plate();
                let (level_text, name_text) = match plate.split_once(' ') {
                    Some((first, rest)) if harsh && rng.unit() < 0.03 => {
                        (first.to_string(), rest.to_string())
                    }
                    _ => (format!("L{}", k.level), plate.clone()),
                };
                scene.plate(
                    &PlateDrawing {
                        line,
                        ..PlateDrawing::locked(position, fill)
                    },
                    &level_text,
                    &name_text,
                );
                tally.pulses_locked += 1;
            }
            if i.is_multiple_of(10) {
                let plates = find_plates(&scene.image, &profile);
                tally.bystanders += shown;
                tally.bystanders_found += plates
                    .iter()
                    .filter(|p| p.kind == PlateKind::Unlocked)
                    .count();
            }
            let inner = scene.recogniser();
            let dice = Mutex::new(Lcg(rng.range(0, 1 << 30) as u64));
            let truth_texts = truth.map(|(kind, _, last)| {
                (
                    format!("L{}", KINDS[kind].level),
                    KINDS[kind].plate(),
                    kind,
                    last,
                )
            });
            let usual_ref = &usual;
            let unread_ref = &unread;
            let noisy = move |crop: &BgrImage| {
                let (text, confidence) = inner(crop)?;
                let mut dice = dice.lock().unwrap_or_else(|p| p.into_inner());
                let read = match &truth_texts {
                    Some((_, name, kind, _))
                        if text == *name
                            && (!KINDS[*kind].name_reads
                                || (harsh && dice.unit() < unread_ref[*kind] + 0.03)) =>
                    {
                        String::new()
                    }
                    // A level that never reads: a glyph too many.
                    Some((level, _, kind, _)) if text == *level && !KINDS[*kind].level_reads => {
                        format!("{level}{}", &level[level.len() - 1..])
                    }
                    Some((_, name, _, LastRead::As(other))) if text == *name => {
                        KINDS[*other].plate()
                    }
                    Some((_, name, _, LastRead::Truncated)) if text == *name => name
                        .rsplit_once(' ')
                        .map_or(name.clone(), |(head, _)| head.to_string()),
                    Some((_, name, kind, LastRead::Plain)) if text == *name => {
                        read_name(*kind, usual_ref, &mut dice)
                    }
                    Some((level, _, _, _)) if text == *level && dice.unit() < 0.125 => {
                        format!("1{}", &level[1..])
                    }
                    _ => text,
                };
                Some((read, confidence))
            };
            // The watcher's read, less the search for windows: the scene has
            // none.
            let locked = find_locked_plate(&scene.image, &profile, &mut scratch);
            let read = locked.and_then(|plate| {
                let others = plates_near(&scene.image, &plate, &profile);
                read_plate(&scene.image, &plate, &[], &others, &profile, &noisy)
            });
            let sighting = match (truth, read) {
                (None, None) => None,
                (None, Some(_)) => {
                    tally.false_locks += 1;
                    None
                }
                (Some(_), None) => None,
                (Some((_, fill, _)), Some(read)) => {
                    assert!(
                        (read.fill - fill).abs() <= 1.0 / 164.0,
                        "{read:?} at {fill}"
                    );
                    tally.locked_read += 1;
                    let resolution = match &read.name {
                        Ok(raw) => {
                            let level = read.level.as_ref().ok().copied();
                            resolved
                                .entry((raw.clone(), level))
                                .or_insert_with(|| {
                                    resolve(raw, level, &catalogue, &profile.mob_names)
                                })
                                .clone()
                        }
                        Err(_) => Resolved::Unclear,
                    };
                    Some(Sighting {
                        fill: read.fill,
                        raw_name: read.name.ok(),
                        resolved: resolution,
                    })
                }
            };
            observed = (t * 1000.0).round() as i64;
            history.observe(observed, sighting);
            i += 1;
        }
        assert_eq!(next, order.len(), "every kill was watched far enough past");
        let spellings: std::collections::HashMap<String, String> =
            history.provisional_names().into_iter().collect();
        for (k, stamp) in stamps {
            let kill = &plan.kills[k];
            let what = format!(
                "{} ({:?}) killed at {:.2} s",
                kill.kind
                    .map_or("a creature never locked".into(), |k| KINDS[k].plate()),
                kill.category,
                kill.fight.1
            );
            let seen: Vec<usize> = plan
                .locks
                .iter()
                .filter(|l| l.to >= kill.fight.0 - 5.0 && l.from <= kill.reported)
                .map(|l| l.kind)
                .collect();
            let outcome = match &stamp {
                KillStamp::Stamped(MobStamp::Named { creature, .. })
                | KillStamp::Stamped(MobStamp::Guess { creature, .. })
                | KillStamp::Stamped(MobStamp::LatestRead { creature, .. }) => {
                    let kind = kind_of(creature);
                    if kind.is_none_or(|k| !seen.contains(&k)) {
                        tally.off_screen.push(format!("{what}: {stamp:?}"));
                    }
                    let right = kind.is_some() && kind == kill.kind;
                    match (&stamp, right) {
                        (KillStamp::Stamped(MobStamp::Named { .. }), true) => Outcome::NamedRight,
                        (KillStamp::Stamped(MobStamp::Named { .. }), false) => Outcome::NamedWrong,
                        (KillStamp::Stamped(MobStamp::Guess { .. }), true) => Outcome::GuessRight,
                        (KillStamp::Stamped(MobStamp::Guess { .. }), false) => Outcome::GuessWrong,
                        (_, true) => Outcome::LatestRight,
                        (_, false) => Outcome::LatestWrong,
                    }
                }
                KillStamp::Stamped(MobStamp::ProvisionalLatestRead { key, .. }) => {
                    let spelling = spellings.get(key).cloned().unwrap_or_default();
                    let unlisted = kill.kind.filter(|k| !KINDS[*k].listed);
                    let nearest = (0..KINDS.len()).max_by(|a, b| {
                        let near =
                            |k: usize| ratio(&label_key(&spelling), &label_key(&KINDS[k].plate()));
                        near(*a).total_cmp(&near(*b))
                    });
                    match unlisted {
                        Some(k) if spelling == KINDS[k].plate() => Outcome::LatestRight,
                        Some(k) if !harsh && nearest == Some(k) => Outcome::LatestMisspelt,
                        _ => Outcome::LatestWrong,
                    }
                }
                KillStamp::Stamped(MobStamp::Provisional { key, .. })
                | KillStamp::Stamped(MobStamp::ProvisionalGuess { key, .. }) => {
                    // Judged by the session's spelling for the key at the
                    // hunt's end. An ordinary hunt tells a misspelling of
                    // the creature killed (the spelling nearer its plate
                    // than any other kind's) apart from another creature.
                    let spelling = spellings.get(key).cloned().unwrap_or_default();
                    let guess =
                        matches!(stamp, KillStamp::Stamped(MobStamp::ProvisionalGuess { .. }));
                    let unlisted = kill.kind.filter(|k| !KINDS[*k].listed);
                    let nearest = (0..KINDS.len()).max_by(|a, b| {
                        let near =
                            |k: usize| ratio(&label_key(&spelling), &label_key(&KINDS[k].plate()));
                        near(*a).total_cmp(&near(*b))
                    });
                    match (unlisted, guess) {
                        (Some(k), false) if spelling == KINDS[k].plate() => {
                            Outcome::ProvisionalRight
                        }
                        (Some(k), true) if spelling == KINDS[k].plate() => Outcome::SuggestedRight,
                        (Some(k), false) if !harsh && nearest == Some(k) => {
                            Outcome::ProvisionalMisspelt
                        }
                        (Some(k), true) if !harsh && nearest == Some(k) => {
                            Outcome::SuggestedMisspelt
                        }
                        (_, false) => Outcome::ProvisionalWrong,
                        (_, true) => Outcome::SuggestedWrong,
                    }
                }
                KillStamp::Unnamed(why) => Outcome::Unnamed(match why {
                    Unnamed::NoFight => UnnamedReason::NoFight,
                    Unnamed::StillRead => UnnamedReason::StillRead,
                    Unnamed::Indistinct => UnnamedReason::Indistinct,
                    Unnamed::Undecided => UnnamedReason::Undecided,
                    Unnamed::Unwatched => UnnamedReason::Unwatched,
                    Unnamed::Implausible => UnnamedReason::Implausible,
                }),
                KillStamp::NotYet => panic!("{what}: asked once watched far enough past it"),
            };
            tally
                .outcomes
                .push((kill.category, outcome, format!("{what}: {stamp:?}")));
        }
        tally
    }

    /// One seeded hunt of fifty fights, read once a second. Each seed is a
    /// test of its own, so that every one finishes well inside the test
    /// runner's limit in an unoptimised build. No kill of any kind may be
    /// named, or given a provisional name, wrongly. Of the kills whose
    /// creature the screen can tell, at least `floor` must come out right
    /// (counting suggestions): the figure measured when the rules were
    /// settled, so that a change for the worse is caught. The others (see
    /// [`Category::ambiguous`]) are reported apart.
    fn simulated_hunt(seed: u64, line: LineAlign, floor: usize) {
        let tally = hunt(seed, 50, line, true);
        let tellable = |c: Category| !c.ambiguous();
        let count = |which: &dyn Fn(Category) -> bool, good: &dyn Fn(Outcome) -> bool| {
            tally
                .outcomes
                .iter()
                .filter(|o| which(o.0) && good(o.1))
                .count()
        };
        let all = |_: Category| true;
        let any = |_: Outcome| true;
        let stamped = |o: Outcome| o.stamped_right();
        let right = |o: Outcome| o.right();
        let wrong_guess = |o: Outcome| matches!(o, Outcome::GuessWrong | Outcome::SuggestedWrong);
        let latest_right = |o: Outcome| o == Outcome::LatestRight;
        let latest_wrong = |o: Outcome| o == Outcome::LatestWrong;
        println!(
            "simulated hunt, seed {seed}, line {line:?}, at 1 Hz: {} brief locks on another \
             creature, {} locks within 0.6 s of a kill. Stamped right (named or provisional): \
             {} of {} tellable, {} of {} all; right counting suggestions: {} of {} tellable, {} \
             of {} all; wrong guesses: {} tellable, {} all; latest read: {} right, {} wrong \
             tellable, {} right, {} wrong all. Tellable: {:?}",
            tally.brief_locks,
            tally.quick_relocks,
            count(&tellable, &stamped),
            count(&tellable, &any),
            count(&all, &stamped),
            count(&all, &any),
            count(&tellable, &right),
            count(&tellable, &any),
            count(&all, &right),
            count(&all, &any),
            count(&tellable, &wrong_guess),
            count(&all, &wrong_guess),
            count(&tellable, &latest_right),
            count(&tellable, &latest_wrong),
            count(&all, &latest_right),
            count(&all, &latest_wrong),
            tally.counts(tellable)
        );
        for category in [
            Category::DifferentRelock,
            Category::FastAlternating,
            Category::OneShot,
            Category::LastMisread,
            Category::Truncated,
            Category::AreaDamage,
            Category::Unreadable,
            Category::Bleed,
            Category::CloseDeaths,
        ] {
            println!("  {category:?}: {:?}", tally.counts(|c| c == category));
        }
        println!(
            "plates: the locked plate read on {} of {} pulses with one drawn, {} false locks; \
             {} of {} other creatures' plates found",
            tally.locked_read,
            tally.pulses_locked,
            tally.false_locks,
            tally.bystanders_found,
            tally.bystanders
        );
        for (category, outcome, line) in &tally.outcomes {
            if !category.ambiguous() && !outcome.right() {
                println!("  not right: {line}");
            }
        }
        let wrong: Vec<&String> = tally
            .outcomes
            .iter()
            .filter(|o| matches!(o.1, Outcome::NamedWrong | Outcome::ProvisionalWrong))
            .map(|o| &o.2)
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
        let off_screen: Vec<&String> = tally
            .off_screen
            .iter()
            .filter(|line| line.contains("Named"))
            .collect();
        assert!(off_screen.is_empty(), "{off_screen:#?}");
        assert_eq!(tally.false_locks, 0);
        assert_eq!(tally.locked_read, tally.pulses_locked);
        assert_eq!(tally.bystanders_found, tally.bystanders);
        let total = count(&tellable, &any);
        assert!(total >= 30, "{total}");
        let right_count = count(&tellable, &right);
        assert!(
            right_count >= floor,
            "{right_count} of {total} tellable kills right, below {floor}"
        );
    }

    /// One seeded ordinary hunt of fifty fights, read once a second:
    /// creatures fought one after another, with quick locks on the next and
    /// brief locks on others, the recogniser's misreadings repeated per
    /// creature, its level misreadings, and every report's arrival, but none
    /// of a harsh hunt's adversities. No kill may be named, or given a
    /// provisional name, wrongly (a misspelling of the creature killed is
    /// counted apart, and not as right), and at least `floor` kills must be
    /// answered right in any grade: the figure measured when the rules were
    /// settled.
    fn ordinary_hunt(seed: u64, line: LineAlign, floor: usize) {
        let tally = hunt(seed, 50, line, false);
        let count =
            |which: &dyn Fn(Outcome) -> bool| tally.outcomes.iter().filter(|o| which(o.1)).count();
        let of = |outcome: Outcome| count(&|o| o == outcome);
        let unnamed: Vec<(Outcome, usize)> = tally
            .counts(|_| true)
            .into_iter()
            .filter(|(o, _)| matches!(o, Outcome::Unnamed(_)))
            .collect();
        let right = count(&|o| o.right());
        let wrong = count(&|o| {
            matches!(
                o,
                Outcome::NamedWrong
                    | Outcome::GuessWrong
                    | Outcome::ProvisionalWrong
                    | Outcome::SuggestedWrong
                    | Outcome::LatestWrong
            )
        });
        println!(
            "ordinary hunt, seed {seed}, line {line:?}, at 1 Hz, {} kills: Named {} right, {} \
             wrong; Guess {} right, {} wrong; Provisional {} right, {} wrong; ProvisionalGuess \
             {} right, {} wrong; latest read {} right, {} wrong; misspelt (the creature killed, \
             under a misspelling) {} Provisional, {} ProvisionalGuess, {} latest read; unnamed \
             {:?}. Answered right {}, answered wrong {}.",
            tally.outcomes.len(),
            of(Outcome::NamedRight),
            of(Outcome::NamedWrong),
            of(Outcome::GuessRight),
            of(Outcome::GuessWrong),
            of(Outcome::ProvisionalRight),
            of(Outcome::ProvisionalWrong),
            of(Outcome::SuggestedRight),
            of(Outcome::SuggestedWrong),
            of(Outcome::LatestRight),
            of(Outcome::LatestWrong),
            of(Outcome::ProvisionalMisspelt),
            of(Outcome::SuggestedMisspelt),
            of(Outcome::LatestMisspelt),
            unnamed,
            right,
            wrong
        );
        for (_, outcome, line) in &tally.outcomes {
            if !outcome.right() {
                println!("  not right: {line}");
            }
        }
        assert_eq!(of(Outcome::NamedWrong), 0);
        assert_eq!(of(Outcome::ProvisionalWrong), 0);
        assert!(right >= floor, "{right} answered right, below {floor}");
    }

    // The floors are the kills each ordinary seed answered right, in any
    // grade, when the rules were settled: measured, not chosen. 42, 44, 45
    // and 36 of 50.
    macro_rules! ordinary_hunts {
        ($($name:ident: $seed:expr, $line:expr, $floor:expr,)*) => {$(
            #[test]
            fn $name() {
                ordinary_hunt($seed, $line, $floor);
            }
        )*};
    }

    ordinary_hunts! {
        an_ordinary_hunt_answers_its_kills_seed_11: 11, LineAlign::Left, 42,
        an_ordinary_hunt_answers_its_kills_seed_12: 12, LineAlign::Centred, 44,
        an_ordinary_hunt_answers_its_kills_seed_13: 13, LineAlign::Left, 45,
        an_ordinary_hunt_answers_its_kills_seed_14: 14, LineAlign::Centred, 36,
    }

    // The floors are the tellable kills each seed got right (counting
    // suggestions and the latest creature read) when the rules were
    // settled: 32 of 36, 32 of 35, 28 of 32 and 28 of 33.
    macro_rules! simulated_hunts {
        ($($name:ident: $seed:expr, $line:expr, $floor:expr,)*) => {$(
            #[test]
            fn $name() {
                simulated_hunt($seed, $line, $floor);
            }
        )*};
    }

    simulated_hunts! {
        a_simulated_hunt_stamps_its_kills_with_the_creature_fought_seed_11: 11, LineAlign::Left, 32,
        a_simulated_hunt_stamps_its_kills_with_the_creature_fought_seed_12: 12, LineAlign::Centred, 32,
        a_simulated_hunt_stamps_its_kills_with_the_creature_fought_seed_13: 13, LineAlign::Left, 28,
        a_simulated_hunt_stamps_its_kills_with_the_creature_fought_seed_14: 14, LineAlign::Centred, 28,
    }
}
