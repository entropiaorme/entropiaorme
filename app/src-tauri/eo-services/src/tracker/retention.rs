//! The shots a later look at the held tool may still move.
//!
//! The regime log forgets what no decision on a mismatch can reach. A
//! change of the tool in hand read off the screen can reach further: it
//! can arrive after shots it predates, and a tool unseen while a window
//! covers the bar is settled only when the bar is seen again. So while the
//! screen is the source, every observation (a shot or an effect tick) of
//! the last few seconds is kept here, with where it was recorded, and every
//! one since a covered stretch began while the stretch lasts. Memory is
//! proportional to the observations kept: a long covered stretch keeps
//! every observation of it until it is settled, with no cap, since an
//! observation dropped could no longer be settled.
//!
//! Kept observations are found by their sequence number and by their time
//! in logarithmic steps, so settling or placing again n observations takes
//! n log n steps. A change that may have to be taken back journals what it
//! touches, so putting it back costs as much as the change did.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::attribution::ShotLocation;
use super::attribution::ShotRecord;

/// Where and how an observation was recorded, beyond what the regime log
/// keeps: what a stored row written for it later needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ShotContext {
    pub(super) context_id: Option<i64>,
    pub(super) reload_speed_percent: f64,
}

/// What one unpriced shot adds to the session's disclosure of unpriced
/// cost: the dearest carried weapon that could explain it, or a count of
/// shots no weapon explains.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum UnpricedCharge {
    Bound(crate::ped::Ped),
    NoFit,
}

/// One observation kept, with where it was recorded.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RetainedShot {
    pub(super) record: ShotRecord,
    pub(super) context: ShotContext,
    /// What the shot added to the unpriced shots' bound while it is
    /// unpriced; None while it is priced.
    pub(super) charge: Option<UnpricedCharge>,
    /// The player's decision on a mismatch that moved it, by its review:
    /// what the player decided stands, so nothing read later moves it.
    pub(super) decided_by: Option<String>,
}

/// What a change touched, as it stood before: each observation (None: it
/// was not kept), and the window, pin and floor.
#[derive(Debug, Clone, Default)]
struct Journal {
    shots: HashMap<u64, Option<RetainedShot>>,
    meta: Option<(f64, Option<f64>, Option<f64>, f64)>,
}

/// An observation's place in time, for ordering: whole microseconds.
fn time_key(at: f64) -> i64 {
    (at * 1e6).round() as i64
}

/// The observations kept: every one of the last `window` seconds, and every
/// one since `pinned` while it is set.
#[derive(Debug, Clone, Default)]
pub(super) struct ShotRetention {
    window: f64,
    pinned: Option<f64>,
    /// While a covered stretch's settlement is owed, the moment it began:
    /// nothing since is let go, whatever pin is set meanwhile.
    owed: Option<f64>,
    /// Every observation at or after this moment is kept: when the
    /// retention began, raised as older ones are let go.
    floor: f64,
    shots: BTreeMap<u64, RetainedShot>,
    by_time: BTreeSet<(i64, u64)>,
    /// While a change may be taken back: each observation as it stood
    /// before the change first touched it (None: it was not kept), and the
    /// window, pin and floor before.
    journal: Option<Journal>,
    /// Steps taken over kept observations, for tests that bound the cost.
    #[cfg(test)]
    pub(super) steps: u64,
}

impl ShotRetention {
    pub(super) fn new(window: f64, now: f64) -> Self {
        Self {
            window,
            floor: now,
            ..Self::default()
        }
    }

    fn step(&mut self) {
        #[cfg(test)]
        {
            self.steps += 1;
        }
    }

    fn touch(&mut self, seq: u64) {
        if let Some(journal) = self.journal.as_mut() {
            journal
                .shots
                .entry(seq)
                .or_insert_with(|| self.shots.get(&seq).cloned());
        }
    }

    fn touch_meta(&mut self) {
        let meta = (self.window, self.pinned, self.owed, self.floor);
        if let Some(journal) = self.journal.as_mut() {
            journal.meta.get_or_insert(meta);
        }
    }

    pub(super) fn floor(&self) -> f64 {
        self.floor
    }

    pub(super) fn set_window(&mut self, window: f64) {
        self.touch_meta();
        self.window = window;
    }

    fn insert(&mut self, shot: RetainedShot) {
        self.step();
        let seq = shot.record.seq;
        self.touch(seq);
        if let Some(old) = self.shots.insert(seq, shot.clone()) {
            self.by_time
                .remove(&(time_key(old.record.observed_at), seq));
        }
        self.by_time
            .insert((time_key(shot.record.observed_at), seq));
    }

    fn remove(&mut self, seq: u64) -> Option<RetainedShot> {
        self.step();
        self.touch(seq);
        let shot = self.shots.remove(&seq)?;
        self.by_time
            .remove(&(time_key(shot.record.observed_at), seq));
        Some(shot)
    }

    /// Let go of what neither the window before `latest` nor the pin keeps.
    pub(super) fn prune(&mut self, latest: f64) {
        let mut floor = latest - self.window;
        for since in [self.pinned, self.owed].into_iter().flatten() {
            floor = floor.min(since);
        }
        let old: Vec<u64> = self
            .by_time
            .range(..(time_key(floor), 0))
            .map(|(_, seq)| *seq)
            .collect();
        for seq in old {
            self.remove(seq);
        }
        if floor > self.floor {
            self.touch_meta();
            self.floor = floor;
        }
    }

    /// Keep every observation since `since` too, however old, while set.
    pub(super) fn pin(&mut self, since: Option<f64>, now: f64) {
        self.touch_meta();
        self.pinned = since;
        self.prune(now);
    }

    /// Keep every observation since `since` while a settlement of them is
    /// owed, however the pin moves meanwhile; None once it is paid.
    pub(super) fn owe(&mut self, since: Option<f64>, now: f64) {
        self.touch_meta();
        self.owed = since;
        self.prune(now);
    }

    /// Keep an observation just recorded.
    pub(super) fn keep(&mut self, shot: RetainedShot) {
        let latest = shot.record.observed_at;
        self.insert(shot);
        self.prune(latest);
    }

    /// Put an observation back as it was.
    pub(super) fn restore(&mut self, shot: RetainedShot) {
        self.insert(shot);
    }

    /// The observations at or after `from`, oldest first.
    pub(super) fn since(&self, from: f64) -> Vec<RetainedShot> {
        self.by_time
            .range((time_key(from), 0)..)
            .filter_map(|(_, seq)| self.shots.get(seq).cloned())
            .collect()
    }

    /// Take the observations at or after `from` out, oldest first, leaving
    /// those a decision moved.
    pub(super) fn take_since(&mut self, from: f64) -> Vec<RetainedShot> {
        let seqs: Vec<u64> = self
            .by_time
            .range((time_key(from), 0)..)
            .map(|(_, seq)| *seq)
            .filter(|seq| {
                self.shots
                    .get(seq)
                    .is_some_and(|shot| shot.decided_by.is_none())
            })
            .collect();
        seqs.into_iter()
            .filter_map(|seq| self.remove(seq))
            .collect()
    }

    /// The kept observation with this sequence number, to change.
    pub(super) fn get_mut(&mut self, seq: u64) -> Option<&mut RetainedShot> {
        self.step();
        if !self.shots.contains_key(&seq) {
            return None;
        }
        self.touch(seq);
        self.shots.get_mut(&seq)
    }

    /// Every pending observation settled into this kill.
    pub(super) fn settle_kill(&mut self, kill_id: &str) {
        let pending: Vec<u64> = self
            .shots
            .iter()
            .filter(|(_, shot)| shot.record.location == ShotLocation::Pending)
            .map(|(seq, _)| *seq)
            .collect();
        for seq in pending {
            if let Some(shot) = self.get_mut(seq) {
                shot.record.location = ShotLocation::Kill(kill_id.to_string());
            }
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.shots.len()
    }

    /// Start keeping what a change touches, so it can be taken back.
    pub(super) fn begin_journal(&mut self) {
        self.journal = Some(Journal::default());
    }

    /// The change stands: forget what it touched.
    pub(super) fn commit(&mut self) {
        self.journal = None;
    }

    /// Take the change back: every observation it touched as it stood.
    pub(super) fn rollback(&mut self) {
        let Some(Journal { shots: saved, meta }) = self.journal.take() else {
            return;
        };
        for (seq, before) in saved {
            if let Some(now) = self.shots.remove(&seq) {
                self.by_time
                    .remove(&(time_key(now.record.observed_at), seq));
            }
            if let Some(before) = before {
                self.by_time
                    .insert((time_key(before.record.observed_at), seq));
                self.shots.insert(seq, before);
            }
        }
        if let Some((window, pinned, owed, floor)) = meta {
            self.window = window;
            self.pinned = pinned;
            self.owed = owed;
            self.floor = floor;
        }
    }
}
