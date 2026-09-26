//! The live doses, as every reader of the reload speed in effect sees them.
//!
//! The tracker owns the dose lifecycle and is the board's only writer: it
//! publishes the doses whose expiry is still ahead after every change it
//! commits (and at start-up, from the persisted rows). Readers on any thread
//! (the hotbar resolver, the equipment library, the facade's weapon pricing)
//! filter by their own reading of the clock, so a dose stops counting the
//! moment its absolute expiry passes whether or not the tracker has swept it
//! yet. The board is a projection of persisted state, never its source.

use std::sync::{Arc, RwLock};

use crate::clock::Clock;
use crate::passive_effects::{reload_speed_in_effect, PassiveEffectSource};
use crate::time::{instant_to_epoch, resolve_local};

use super::profile::{effects_reload_speed_percent, DoseEffect};

/// How a dose started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoseSource {
    /// The item's hotbar key, pressed in game.
    Hotbar,
    /// Started by hand, on the overlay or the dashboard.
    Manual,
    /// A healing tool's buff, opened by a paid heal.
    OnUse,
}

impl DoseSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hotbar => "hotbar",
            Self::Manual => "manual",
            Self::OnUse => "on_use",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "hotbar" => Self::Hotbar,
            "manual" => Self::Manual,
            "on_use" => Self::OnUse,
            _ => return None,
        })
    }
}

/// One dose whose effect has not ended.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveDose {
    pub id: String,
    /// Absent once the item was deleted from the equipment library.
    pub equipment_id: Option<i64>,
    pub item_name: String,
    pub source: DoseSource,
    pub session_id: Option<String>,
    pub started_at: f64,
    pub expires_at: f64,
    pub cost_ped: f64,
    pub effects: Vec<DoseEffect>,
}

impl LiveDose {
    /// Whether the dose's effect is in force at `now`.
    pub fn in_effect_at(&self, now: f64) -> bool {
        self.started_at <= now && now < self.expires_at
    }

    /// The reload speed the dose's effects add, percent, as printed.
    pub fn reload_speed_percent(&self) -> f64 {
        effects_reload_speed_percent(&self.effects)
    }
}

/// The published live doses. Cloning shares the one board.
#[derive(Clone)]
pub struct DoseBoard {
    doses: Arc<RwLock<Arc<Vec<LiveDose>>>>,
    clock: Arc<dyn Clock>,
}

impl DoseBoard {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            doses: Arc::new(RwLock::new(Arc::new(Vec::new()))),
            clock,
        }
    }

    /// Replace the published doses (the tracker, after a commit).
    pub fn publish(&self, doses: Vec<LiveDose>) {
        let mut guard = self
            .doses
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Arc::new(doses);
    }

    /// The published doses, whatever their expiry.
    pub fn doses(&self) -> Arc<Vec<LiveDose>> {
        self.doses
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Now, as the board's clock reads it: epoch seconds.
    pub fn now(&self) -> f64 {
        instant_to_epoch(resolve_local(self.clock.now()))
    }

    /// The doses in force at `now`.
    pub fn in_effect_at(&self, now: f64) -> Vec<LiveDose> {
        self.doses()
            .iter()
            .filter(|dose| dose.in_effect_at(now))
            .cloned()
            .collect()
    }

    /// Each dose in force at `now`, as the reload speed it adds.
    pub fn consumed_reload_at(&self, now: f64) -> Vec<f64> {
        self.doses()
            .iter()
            .filter(|dose| dose.in_effect_at(now))
            .map(LiveDose::reload_speed_percent)
            .filter(|percent| *percent != 0.0)
            .collect()
    }

    /// The reload speed in effect at `now`: the enabled equipped sources and
    /// the doses in force, each under the game's limits.
    pub fn reload_speed_percent_at(&self, sources: &[PassiveEffectSource], now: f64) -> f64 {
        reload_speed_in_effect(
            crate::passive_effects::equipped_reload_magnitudes(sources),
            self.consumed_reload_at(now),
        )
    }

    /// The reload speed in effect now.
    pub fn reload_speed_percent(&self, sources: &[PassiveEffectSource]) -> f64 {
        self.reload_speed_percent_at(sources, self.now())
    }

    /// When the reload speed in effect next changes on its own: the
    /// earliest expiry after `now` among the published doses.
    pub fn next_expiry_after(&self, now: f64) -> Option<f64> {
        self.doses()
            .iter()
            .map(|dose| dose.expires_at)
            .filter(|expires_at| *expires_at > now)
            .reduce(f64::min)
    }
}

impl std::fmt::Debug for DoseBoard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoseBoard")
            .field("doses", &self.doses())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::MockClock;
    use crate::passive_effects::{PassiveEffect, PassiveEffectKind};

    fn dose(id: &str, started_at: f64, expires_at: f64, reload: f64) -> LiveDose {
        LiveDose {
            id: id.into(),
            equipment_id: Some(1),
            item_name: "Nanobots - Adrenaline Boost".into(),
            source: DoseSource::Hotbar,
            session_id: None,
            started_at,
            expires_at,
            cost_ped: 0.0,
            effects: vec![DoseEffect::new(
                "Reload Speed Increased",
                Some(reload),
                Some("%"),
            )],
        }
    }

    fn ring(percent: f64) -> PassiveEffectSource {
        PassiveEffectSource {
            id: "ring".into(),
            name: "Ring".into(),
            enabled: true,
            effects: vec![PassiveEffect {
                kind: PassiveEffectKind::ReloadSpeed,
                magnitude_percent: percent,
            }],
        }
    }

    #[test]
    fn a_dose_counts_from_its_start_until_its_absolute_expiry() {
        let board = DoseBoard::new(Arc::new(MockClock::new(None, 0.0)));
        board.publish(vec![dose("a", 100.0, 160.0, 10.0)]);
        assert_eq!(board.consumed_reload_at(99.0), Vec::<f64>::new());
        assert_eq!(board.consumed_reload_at(100.0), vec![10.0]);
        assert_eq!(board.consumed_reload_at(159.9), vec![10.0]);
        assert_eq!(board.consumed_reload_at(160.0), Vec::<f64>::new());
    }

    #[test]
    fn doses_join_equipment_under_the_consumed_limit() {
        let board = DoseBoard::new(Arc::new(MockClock::new(None, 0.0)));
        board.publish(vec![
            dose("a", 0.0, 100.0, 10.0),
            dose("b", 0.0, 50.0, 15.0),
        ]);
        // 14% equipped, 25% consumed held at 20%: 34 held at 30.
        assert_eq!(board.reload_speed_percent_at(&[ring(14.0)], 10.0), 30.0);
        // Once the second dose ends: 14 + 10.
        assert_eq!(board.reload_speed_percent_at(&[ring(14.0)], 60.0), 24.0);
        assert_eq!(board.next_expiry_after(10.0), Some(50.0));
        assert_eq!(board.next_expiry_after(60.0), Some(100.0));
        assert_eq!(board.next_expiry_after(100.0), None);
    }
}
