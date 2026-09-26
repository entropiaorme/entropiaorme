//! Unified weapon attribution: which weapon an offensive chat-log line
//! came from, and therefore what it cost.
//!
//! One engine, no modes. A hotbar press declares the weapon in hand and is
//! the primary signal; each carried weapon's damage band quietly validates
//! it. Every observation resolves to one of four states:
//!
//! - **Agrees**: the declared weapon explains it (its band fits, it has no
//!   band to contradict, the hit undershoots its band, or the previous
//!   weapon's shot is still landing just after a switch). Priced to that
//!   weapon.
//! - **Effect tick**: an open effect window an earlier paid activation owns
//!   explains the magnitude. No shot and no cost.
//! - **Evidence**: the magnitude fits exactly one carried weapon, and it is
//!   not the declared one (a missed switch), or nothing is declared at all
//!   (no hotbar signal). Priced to the weapon the evidence names.
//! - **Unresolved**: no honest single source: several weapons fit, or none
//!   does and nothing declared can absorb it. Recorded without a price.
//!
//! Evidence may only override belief inside the current intent regime. A
//! regime starts at a weapon or harvesting-tool press (or the session
//! start), and at an explicit decision on a standing mismatch. When the
//! evidence contradicts a declared weapon, the regime carries a mismatch:
//! the overlay shows what is being recorded instead, and the player can
//! confirm it (the evidence weapon becomes the declared one from now, and
//! the regime's shots it plausibly fired are repriced to it) or keep the
//! declared weapon (the evidence shots are repriced back to it, and that
//! weapon's evidence stops overriding it for the rest of the regime).
//!
//! Hits only ever land below a weapon's band, never above it except as a
//! critical: mob armour and sub-maximal skill lower a hit, and nothing
//! raises one. So a hit below the declared weapon's band that no other
//! carried weapon explains still agrees with the declared weapon, while a
//! hit above every band is out of profile and stays unresolved.
//!
//! A weapon with a declared damage-over-time effect opens an effect window
//! with every paid hit: for the effect's duration, its ticks are outcomes of
//! that one activation. A window is not an alternative to the weapon in
//! hand but a second source running beside it, so intent cannot choose
//! between them: a hit that both the declared weapon and another weapon's
//! open effect explain stays unresolved rather than being priced as a shot
//! or waved through as a tick. A tick that fits the weapon in hand's own
//! effect is a tick (a recast lands its own, distinct outcome). Where no
//! band holds a hit at all, it is short of every source, and the source
//! whose floor sits nearest above it needs the least reduction to explain
//! it: the effect's floor when that is lower than the declared weapon's.
//! Where several windows explain a tick, each stays a candidate; none is
//! chosen by the order it opened in.
//!
//! The engine is pure state over observations; pricing, persistence, and
//! the kill accumulator belong to the tracker actor that drives it.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::cost_engine::weapon_damage_profile_from_props;
use crate::weapon_effect::{effect_profile_from_props, WeaponEffectProfile};

/// A critical hit reaches at most this multiple of a weapon's maximum.
pub const CRITICAL_REACH: f64 = 3.0;

/// Chat-log damage prints to one decimal, so a printed figure can sit up
/// to half a step outside the band it was rolled from.
pub const DISPLAY_TOLERANCE: f64 = 0.05;

/// After a hotbar switch, a shot the previous weapon already fired can
/// still arrive for this long (the same delivery tail healing allows).
pub(super) const SWITCH_TAIL_SECONDS: f64 = 1.25;

/// The damage one regular hit of a weapon can deal at full skill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageBand {
    pub min: f64,
    pub max: f64,
}

impl DamageBand {
    /// Whether a hit of this size can have come from the weapon. A regular
    /// hit lands inside the band; a critical anywhere from the band's floor
    /// (armour can absorb a critical's bonus) to its critical reach.
    pub fn fits(&self, amount: f64, critical: bool) -> bool {
        let ceiling = if critical {
            self.max * CRITICAL_REACH
        } else {
            self.max
        };
        amount >= self.min - DISPLAY_TOLERANCE && amount <= ceiling + DISPLAY_TOLERANCE
    }

    /// Whether the hit falls short of the band's floor.
    pub fn undershoots(&self, amount: f64) -> bool {
        amount < self.min - DISPLAY_TOLERANCE
    }
}

/// A stored weapon's regular-hit band at full skill: the outcome its
/// declared effect's activation prints, when it declares one (observed, so
/// already whatever the attack rate made it), else its catalogue damage
/// with its amplifier and configured damage enhancers, scaled by the
/// attack-rate factor the props carry. None when neither gives a usable
/// figure.
pub fn damage_band_from_props(props: &Value) -> Option<DamageBand> {
    if let Some(effect) = effect_profile_from_props(props) {
        return Some(effect.activation_band());
    }
    let profile = weapon_damage_profile_from_props(props)?;
    let min = profile.get("damageMin")?.as_f64()?;
    let max = profile.get("damageMax")?.as_f64()?;
    (max > 0.0).then_some(DamageBand { min, max })
}

/// One weapon the player carries, as the guardrail sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct CarriedWeapon {
    pub equipment_id: i64,
    pub name: String,
    /// None when the catalogue exposes no usable damage figure: the weapon
    /// can then never be named by evidence, and never contradicted by it.
    pub band: Option<DamageBand>,
    /// The damage-over-time effect each paid hit starts, when declared.
    pub effect: Option<WeaponEffectProfile>,
}

impl CarriedWeapon {
    /// A stored weapon as attribution sees it: the band its shots are
    /// checked against and the effect its paid hits start.
    pub fn from_props(equipment_id: i64, name: String, props: &Value) -> Self {
        Self {
            equipment_id,
            name,
            band: damage_band_from_props(props),
            effect: effect_profile_from_props(props),
        }
    }
}

/// One offensive chat-log observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Observation {
    /// A hit with its printed magnitude.
    Hit { amount: f64, critical: bool },
    /// A jam, dodge, evade, or miss: a shot was fired, but no magnitude
    /// says by what.
    Countered,
}

impl Observation {
    fn amount(self) -> f64 {
        match self {
            Observation::Hit { amount, .. } => amount,
            Observation::Countered => 0.0,
        }
    }
}

/// Why an observation agrees with a weapon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgreeReason {
    /// Its band fits the magnitude.
    Fits,
    /// It has no band to validate against.
    Unvalidated,
    /// The hit falls short of its band and no other weapon explains it.
    BelowBand,
    /// The previous weapon's shot, landing just after the switch.
    InFlight,
    /// A countered shot inherits the declared weapon.
    Countered,
    /// The player kept the declared weapon over this evidence.
    Kept,
}

/// How one observation resolved: the four product states.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Attribution {
    Agrees {
        tool: String,
        reason: AgreeReason,
    },
    /// `window_id` names the window when exactly one explains the tick;
    /// with several, the candidates travel on [`Resolved::windows`].
    EffectTick {
        window_id: Option<String>,
    },
    /// `confirmed` marks a shot the player's confirmation priced rather
    /// than its own magnitude.
    Evidence {
        tool: String,
        confirmed: bool,
    },
    Unresolved,
}

impl Attribution {
    /// The weapon the shot is priced to, when it has one. An effect tick is
    /// not a shot at all.
    pub(super) fn priced_tool(&self) -> Option<&str> {
        match self {
            Attribution::Agrees { tool, .. } | Attribution::Evidence { tool, .. } => Some(tool),
            Attribution::EffectTick { .. } | Attribution::Unresolved => None,
        }
    }

    /// The persisted evidence classification, for the states that keep a
    /// row: evidence that overrode a declared weapon, unresolved shots, and
    /// effect ticks.
    pub(super) fn kind(&self) -> AttributionKind {
        match self {
            Attribution::Agrees { .. } => AttributionKind::Agrees,
            Attribution::EffectTick { .. } => AttributionKind::EffectTick,
            Attribution::Evidence { .. } => AttributionKind::Evidence,
            Attribution::Unresolved => AttributionKind::Unresolved,
        }
    }
}

/// The closed classification vocabulary a stored evidence row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionKind {
    Agrees,
    EffectTick,
    Evidence,
    Unresolved,
}

impl AttributionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AttributionKind::Agrees => "agrees",
            AttributionKind::EffectTick => "effect_tick",
            AttributionKind::Evidence => "evidence",
            AttributionKind::Unresolved => "unresolved",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "agrees" => Some(AttributionKind::Agrees),
            "effect_tick" => Some(AttributionKind::EffectTick),
            "evidence" => Some(AttributionKind::Evidence),
            "unresolved" => Some(AttributionKind::Unresolved),
            _ => None,
        }
    }
}

/// A classification together with the carried weapons whose band fits the
/// observation (every carried weapon, for a countered shot), the open
/// effect windows that could explain it, and the reason a stored row
/// records.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Resolved {
    pub(super) attribution: Attribution,
    pub(super) fits: Vec<String>,
    /// The ids of the open windows that explain the observation: the tick
    /// candidates of an effect tick, and the effects an unresolved hit
    /// could equally have been a tick of.
    pub(super) windows: Vec<String>,
    pub(super) reason: String,
}

/// An open window of an effect some earlier paid activation owns. Its
/// ticks are outcomes of that activation, never new shots.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OffensiveEffectWindow {
    pub(super) id: String,
    /// The weapon whose paid hit opened it.
    pub(super) tool: String,
    pub(super) equipment_id: Option<i64>,
    pub(super) started_at: f64,
    pub(super) expires_at: f64,
    pub(super) tick: DamageBand,
}

impl OffensiveEffectWindow {
    /// Whether an observation at `at` falls inside the window, allowing
    /// the delivery tail past its expiry.
    fn live_at(&self, at: f64) -> bool {
        self.started_at <= at + 0.05 && at <= self.expires_at + SWITCH_TAIL_SECONDS
    }
}

/// A standing disagreement between the declared weapon and the evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponMismatch {
    /// What the hotbar declared.
    pub declared: String,
    /// What the evidence says is being fired, and what is being recorded.
    pub evidence: String,
    /// When the evidence first contradicted the declared weapon.
    pub since: f64,
    /// Shots recorded to the evidence weapon since then.
    pub shots: i64,
}

/// Where a logged shot's cost currently sits.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ShotLocation {
    /// In the kill accumulator, not yet settled into a kill.
    Pending,
    /// Settled into the kill with this id.
    Kill(String),
}

/// One shot of the current regime, remembered so a decision on a standing
/// mismatch can reprice it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ShotRecord {
    pub(super) seq: u64,
    pub(super) observed_at: f64,
    pub(super) observation: Observation,
    pub(super) attribution: Attribution,
    pub(super) fits: Vec<String>,
    /// Open effect windows that could equally explain it.
    pub(super) windows: Vec<String>,
    pub(super) location: ShotLocation,
    /// The shot's stored evidence row, when its state keeps one.
    pub(super) evidence_id: Option<String>,
    /// The weapon whose phase the shot is counted under (None: unpriced).
    pub(super) booked: Option<String>,
    /// The per-shot cost booked for it (zero when unpriced).
    pub(super) cost: crate::ped::Ped,
    /// The effect window its hit opened, when its weapon declares one.
    pub(super) opened_window: Option<String>,
}

/// One logged shot a decision moves to another weapon, with the record as
/// it stood before the decision (where its cost sits, and at what).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Reprice {
    pub(super) seq: u64,
    pub(super) to_tool: String,
    pub(super) record: ShotRecord,
}

/// A decision on a standing mismatch, ready for the actor to price.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Decision {
    pub(super) mismatch: WeaponMismatch,
    pub(super) moves: Vec<Reprice>,
    /// Effect windows the decision took back: those the evidence shots a
    /// keep repriced had opened.
    pub(super) withdrawn: Vec<String>,
}

/// Session tallies of how shots resolved, as they stand after decisions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttributionCounts {
    pub agreed: i64,
    pub evidenced: i64,
    pub unresolved: i64,
    pub effect_ticks: i64,
}

/// The session's attribution state.
#[derive(Debug, Clone, Default)]
pub(super) struct AttributionRuntime {
    carried: Vec<CarriedWeapon>,
    /// The weapon the latest press declared. None before any press, and
    /// always when the hotbar listener is off.
    declared: Option<String>,
    /// The weapon declared before the latest switch, and when the switch
    /// was pressed.
    previous: Option<(String, f64)>,
    /// With nothing declared: the weapon evidence last named.
    recording: Option<String>,
    mismatch: Option<WeaponMismatch>,
    /// Evidence weapons the player kept the declared weapon over, this
    /// regime.
    kept: BTreeSet<String>,
    effect_windows: Vec<OffensiveEffectWindow>,
    log: Vec<ShotRecord>,
    next_seq: u64,
    counts: AttributionCounts,
}

impl AttributionRuntime {
    /// Replace the carried set (session start, or a configuration reload).
    /// The regime's shots stay as recorded; only later observations see the
    /// new bands.
    pub(super) fn set_carried(&mut self, carried: Vec<CarriedWeapon>) {
        self.carried = carried;
    }

    pub(super) fn carried(&self) -> &[CarriedWeapon] {
        &self.carried
    }

    pub(super) fn declared(&self) -> Option<&str> {
        self.declared.as_deref()
    }

    /// The weapon shots are being recorded to right now: the evidence
    /// weapon of a standing mismatch, else the declared weapon, else the
    /// weapon evidence last named.
    pub(super) fn recording(&self) -> Option<&str> {
        self.mismatch
            .as_ref()
            .map(|mismatch| mismatch.evidence.as_str())
            .or(self.declared.as_deref())
            .or(self.recording.as_deref())
    }

    pub(super) fn mismatch(&self) -> Option<&WeaponMismatch> {
        self.mismatch.as_ref()
    }

    pub(super) fn counts(&self) -> AttributionCounts {
        self.counts
    }

    #[cfg(test)]
    pub(super) fn log(&self) -> &[ShotRecord] {
        &self.log
    }

    pub(super) fn record_mut(&mut self, seq: u64) -> Option<&mut ShotRecord> {
        self.log.iter_mut().find(|record| record.seq == seq)
    }

    /// Open an effect window whose ticks are outcomes of an earlier paid
    /// activation.
    pub(super) fn open_effect_window(&mut self, window: OffensiveEffectWindow) {
        self.effect_windows.push(window);
    }

    /// Adopt the persisted effect windows as the live ones (a session's
    /// start: effects paid for before it keep ticking into it).
    pub(super) fn set_effect_windows(&mut self, windows: Vec<OffensiveEffectWindow>) {
        self.effect_windows = windows;
    }

    pub(super) fn effect_windows(&self) -> &[OffensiveEffectWindow] {
        &self.effect_windows
    }

    /// The carried weapon's declared effect, when it has one.
    pub(super) fn effect_of(&self, tool: &str) -> Option<(&CarriedWeapon, &WeaponEffectProfile)> {
        self.carried
            .iter()
            .find(|weapon| weapon.name == tool)
            .and_then(|weapon| weapon.effect.as_ref().map(|effect| (weapon, effect)))
    }

    /// A new regime: no evidence may reach back past this point.
    fn floor(&mut self) {
        self.mismatch = None;
        self.kept.clear();
        self.log.clear();
    }

    /// A weapon press: the pressed weapon is declared from here, a new
    /// regime starts, and the previous weapon's shots may still land for a
    /// moment.
    pub(super) fn declare(&mut self, tool: &str, at: f64) {
        let previous = self.declared.take();
        self.previous = previous
            .filter(|previous| previous != tool)
            .map(|previous| (previous, at));
        self.declared = Some(tool.to_string());
        self.recording = None;
        self.floor();
    }

    /// A harvesting-tool press re-syncs the belief with the game: a new
    /// regime starts with the declared weapon unchanged.
    pub(super) fn resync(&mut self) {
        self.floor();
    }

    fn band_of(&self, tool: &str) -> Option<DamageBand> {
        self.carried
            .iter()
            .find(|weapon| weapon.name == tool)
            .and_then(|weapon| weapon.band)
    }

    fn fitting(&self, amount: f64, critical: bool) -> Vec<String> {
        self.carried
            .iter()
            .filter(|weapon| weapon.band.is_some_and(|band| band.fits(amount, critical)))
            .map(|weapon| weapon.name.clone())
            .collect()
    }

    /// The open windows whose tick band holds the hit.
    fn ticking(&self, at: f64, amount: f64, critical: bool) -> Vec<&OffensiveEffectWindow> {
        self.effect_windows
            .iter()
            .filter(|window| window.live_at(at) && window.tick.fits(amount, critical))
            .collect()
    }

    /// The open windows that best explain a hit short of every band: those
    /// whose tick floor is the nearest above it, provided that floor sits
    /// below `rival_floor` (the declared weapon's, when one competes).
    fn nearest_below(&self, at: f64, amount: f64, rival_floor: f64) -> Vec<&OffensiveEffectWindow> {
        let short: Vec<&OffensiveEffectWindow> = self
            .effect_windows
            .iter()
            .filter(|window| {
                window.live_at(at)
                    && window.tick.undershoots(amount)
                    && window.tick.min < rival_floor
            })
            .collect();
        let Some(nearest) = short
            .iter()
            .map(|window| window.tick.min)
            .min_by(f64::total_cmp)
        else {
            return Vec::new();
        };
        short
            .into_iter()
            .filter(|window| (window.tick.min - nearest).abs() < 1e-9)
            .collect()
    }

    /// Whether every window was opened by `tool`.
    fn all_owned_by(windows: &[&OffensiveEffectWindow], tool: &str) -> bool {
        windows.iter().all(|window| window.tool == tool)
    }

    fn effect_tick(
        windows: &[&OffensiveEffectWindow],
        fits: Vec<String>,
        reason: &str,
    ) -> Resolved {
        let ids: Vec<String> = windows.iter().map(|window| window.id.clone()).collect();
        let window_id = match ids.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        };
        let reason = if window_id.is_some() {
            reason.to_string()
        } else {
            format!("{reason}; several open effects explain it")
        };
        Resolved {
            attribution: Attribution::EffectTick { window_id },
            fits,
            windows: ids,
            reason,
        }
    }

    fn concurrent(windows: &[&OffensiveEffectWindow], fits: Vec<String>, rival: &str) -> Resolved {
        let owners: BTreeSet<&str> = windows.iter().map(|window| window.tool.as_str()).collect();
        let owners: Vec<&str> = owners.into_iter().collect();
        Resolved {
            attribution: Attribution::Unresolved,
            reason: format!(
                "fits {rival} and a tick of the open {} effect",
                owners.join(" and ")
            ),
            windows: windows.iter().map(|window| window.id.clone()).collect(),
            fits,
        }
    }

    /// Classify one observation without changing any state.
    pub(super) fn classify(&self, observation: Observation, at: f64) -> Resolved {
        let (amount, critical) = match observation {
            Observation::Hit { amount, critical } => (amount, critical),
            Observation::Countered => return self.classify_countered(),
        };
        let fits = self.fitting(amount, critical);
        let ticking = self.ticking(at, amount, critical);
        let Some(declared) = self.declared.clone() else {
            return self.classify_undeclared(amount, at, fits, &ticking);
        };

        // A declared weapon with no band cannot be contradicted, so it
        // explains every hit, and competes with any effect ticking beside
        // it exactly as a fitting band does.
        let declared_band = self.band_of(&declared);
        if declared_band.is_none_or(|band| band.fits(amount, critical)) {
            let reason = if declared_band.is_some() {
                AgreeReason::Fits
            } else {
                AgreeReason::Unvalidated
            };
            if ticking.is_empty() {
                return Resolved {
                    attribution: Attribution::Agrees {
                        tool: declared,
                        reason,
                    },
                    reason: String::new(),
                    fits,
                    windows: Vec::new(),
                };
            }
            if Self::all_owned_by(&ticking, &declared) {
                return Self::effect_tick(&ticking, fits, "a tick of the effect in hand");
            }
            return Self::concurrent(&ticking, fits, &format!("the hotbar's {declared}"));
        }
        let band = declared_band.expect("a bandless declared weapon fits every hit");

        let in_flight = self.previous.as_ref().and_then(|(previous, switched_at)| {
            let in_tail = at >= switched_at - 0.05 && at - switched_at <= SWITCH_TAIL_SECONDS;
            (in_tail
                && self
                    .band_of(previous)
                    .is_some_and(|band| band.fits(amount, critical)))
            .then(|| previous.clone())
        });
        if !ticking.is_empty() {
            return match &in_flight {
                Some(previous) if !Self::all_owned_by(&ticking, previous) => {
                    Self::concurrent(&ticking, fits, &format!("{previous}'s shot still landing"))
                }
                _ => Self::effect_tick(&ticking, fits, "a tick of an open effect"),
            };
        }
        if let Some(previous) = in_flight {
            return Resolved {
                attribution: Attribution::Agrees {
                    tool: previous,
                    reason: AgreeReason::InFlight,
                },
                reason: String::new(),
                fits,
                windows: Vec::new(),
            };
        }
        let others: Vec<&String> = fits.iter().filter(|name| **name != declared).collect();
        match others.as_slice() {
            [only] if self.kept.contains(*only) => Resolved {
                attribution: Attribution::Agrees {
                    tool: declared,
                    reason: AgreeReason::Kept,
                },
                reason: String::new(),
                fits,
                windows: Vec::new(),
            },
            [only] => {
                let only = (*only).clone();
                Resolved {
                    reason: format!("fits only {only}, not the hotbar's {declared}"),
                    attribution: Attribution::Evidence {
                        tool: only,
                        confirmed: false,
                    },
                    fits,
                    windows: Vec::new(),
                }
            }
            [] if band.undershoots(amount) => {
                let nearer = self.nearest_below(at, amount, band.min);
                if !nearer.is_empty() {
                    return Self::effect_tick(
                        &nearer,
                        fits,
                        "short of every band, nearest an open effect's ticks",
                    );
                }
                Resolved {
                    attribution: Attribution::Agrees {
                        tool: declared,
                        reason: AgreeReason::BelowBand,
                    },
                    reason: String::new(),
                    fits,
                    windows: Vec::new(),
                }
            }
            [] => Resolved {
                attribution: Attribution::Unresolved,
                reason: "exceeds every carried weapon's reach".to_string(),
                fits,
                windows: Vec::new(),
            },
            _ => Resolved {
                attribution: Attribution::Unresolved,
                reason: "fits several carried weapons other than the hotbar's".to_string(),
                fits,
                windows: Vec::new(),
            },
        }
    }

    /// Classify a hit with no weapon declared. An open effect explains its
    /// ticks unless a carried weapon other than the effect's own also fits:
    /// with nothing declared, that weapon is a second live source.
    fn classify_undeclared(
        &self,
        amount: f64,
        at: f64,
        fits: Vec<String>,
        ticking: &[&OffensiveEffectWindow],
    ) -> Resolved {
        if !ticking.is_empty() {
            let owners: BTreeSet<&str> =
                ticking.iter().map(|window| window.tool.as_str()).collect();
            let rivals: Vec<&String> = fits
                .iter()
                .filter(|name| !owners.contains(name.as_str()))
                .collect();
            return match rivals.as_slice() {
                [] => Self::effect_tick(ticking, fits, "a tick of an open effect"),
                [only] => {
                    let only = (*only).clone();
                    Self::concurrent(ticking, fits, &only)
                }
                _ => Self::concurrent(ticking, fits, "several carried weapons"),
            };
        }
        match fits.as_slice() {
            [only] => Resolved {
                attribution: Attribution::Evidence {
                    tool: only.clone(),
                    confirmed: false,
                },
                reason: format!("fits only {only}"),
                fits,
                windows: Vec::new(),
            },
            [] => {
                // A carried weapon whose floor sits nearer rivals the
                // effect; with nothing declared, that leaves it unresolved.
                let nearest_weapon = self
                    .carried
                    .iter()
                    .filter_map(|weapon| weapon.band)
                    .filter(|band| band.undershoots(amount))
                    .map(|band| band.min)
                    .fold(f64::INFINITY, f64::min);
                let nearer = self.nearest_below(at, amount, nearest_weapon);
                if !nearer.is_empty() {
                    return Self::effect_tick(
                        &nearer,
                        fits,
                        "short of every band, nearest an open effect's ticks",
                    );
                }
                Resolved {
                    attribution: Attribution::Unresolved,
                    reason: "fits no carried weapon".to_string(),
                    fits,
                    windows: Vec::new(),
                }
            }
            _ => Resolved {
                attribution: Attribution::Unresolved,
                reason: "fits several carried weapons".to_string(),
                fits,
                windows: Vec::new(),
            },
        }
    }

    fn classify_countered(&self) -> Resolved {
        let fits: Vec<String> = self
            .carried
            .iter()
            .map(|weapon| weapon.name.clone())
            .collect();
        if let Some(mismatch) = &self.mismatch {
            return Resolved {
                attribution: Attribution::Evidence {
                    tool: mismatch.evidence.clone(),
                    confirmed: false,
                },
                reason: format!("a countered shot while {} is recorded", mismatch.evidence),
                fits,
                windows: Vec::new(),
            };
        }
        if let Some(declared) = &self.declared {
            return Resolved {
                attribution: Attribution::Agrees {
                    tool: declared.clone(),
                    reason: AgreeReason::Countered,
                },
                reason: String::new(),
                fits,
                windows: Vec::new(),
            };
        }
        match &self.recording {
            Some(recording) => Resolved {
                attribution: Attribution::Evidence {
                    tool: recording.clone(),
                    confirmed: false,
                },
                reason: format!("a countered shot while {recording} is recorded"),
                fits,
                windows: Vec::new(),
            },
            None => Resolved {
                attribution: Attribution::Unresolved,
                reason: "a countered shot with no weapon known".to_string(),
                fits,
                windows: Vec::new(),
            },
        }
    }

    /// Absorb a classified observation: update the regime's mismatch, the
    /// weapon being recorded, and the tallies. Returns the sequence number
    /// the shot is remembered under once [`Self::remember`] logs it.
    pub(super) fn apply(&mut self, resolved: &Resolved, at: f64) -> u64 {
        self.effect_windows
            .retain(|window| window.expires_at + SWITCH_TAIL_SECONDS >= at);
        match &resolved.attribution {
            Attribution::Agrees { reason, .. } => {
                self.counts.agreed += 1;
                // A hit only the declared weapon's band explains (and not
                // the standing evidence weapon's) proves the declared weapon
                // is in hand: the mismatch is over.
                if *reason == AgreeReason::Fits {
                    if let Some(mismatch) = &self.mismatch {
                        if !resolved.fits.contains(&mismatch.evidence) {
                            self.mismatch = None;
                        }
                    }
                }
            }
            Attribution::EffectTick { .. } => self.counts.effect_ticks += 1,
            Attribution::Evidence { tool, .. } => {
                self.counts.evidenced += 1;
                match &self.declared {
                    Some(declared) if declared != tool => match &mut self.mismatch {
                        Some(mismatch) if mismatch.evidence == *tool => mismatch.shots += 1,
                        _ => {
                            self.mismatch = Some(WeaponMismatch {
                                declared: declared.clone(),
                                evidence: tool.clone(),
                                since: at,
                                shots: 1,
                            });
                        }
                    },
                    Some(_) => {}
                    None => self.recording = Some(tool.clone()),
                }
            }
            Attribution::Unresolved => self.counts.unresolved += 1,
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// Remember a priced shot of the current regime, for a later decision.
    /// A hit only the declared weapon explains proves it was in hand, so
    /// no decision can reach past it: everything older that a decision
    /// could not move anyway is forgotten, which bounds the log to the
    /// evidence of the regime plus the shots since the last proof.
    pub(super) fn remember(&mut self, record: ShotRecord) {
        let proves_declared = matches!(
            &record.attribution,
            Attribution::Agrees {
                reason: AgreeReason::Fits,
                ..
            }
        ) && record.fits.len() == 1;
        if proves_declared {
            self.log
                .retain(|older| matches!(older.attribution, Attribution::Evidence { .. }));
        }
        self.log.push(record);
    }

    /// Every pending shot settled into this kill.
    pub(super) fn settle_kill(&mut self, kill_id: &str) {
        for record in &mut self.log {
            if record.location == ShotLocation::Pending {
                record.location = ShotLocation::Kill(kill_id.to_string());
            }
        }
    }

    /// Confirm the evidence weapon: it becomes the declared weapon from
    /// now, and the regime's shots it plausibly fired are repriced to it.
    /// Walking back from the newest shot, a hit that fits the declared
    /// weapon but not the evidence one proves the switch came later, and a
    /// shot the player already vouched for (kept) or a previous weapon's
    /// in-flight shot bounds the walk the same way. Before those, a
    /// countered shot, a hit either weapon explains, a hit short of every
    /// band, and an unresolved hit the evidence weapon fits (unless an open
    /// effect could equally have ticked it) all move.
    pub(super) fn confirm(&mut self, at: f64) -> Option<Decision> {
        let mismatch = self.mismatch.take()?;
        let evidence = mismatch.evidence.clone();
        let mut moves = Vec::new();
        for record in self.log.iter_mut().rev() {
            let movable = match &record.attribution {
                Attribution::Agrees { reason, .. } => match reason {
                    AgreeReason::Fits => {
                        if !record.fits.contains(&evidence) {
                            break;
                        }
                        true
                    }
                    AgreeReason::InFlight | AgreeReason::Kept => break,
                    AgreeReason::BelowBand | AgreeReason::Countered | AgreeReason::Unvalidated => {
                        true
                    }
                },
                // A hit an open effect could equally have ticked stays as
                // recorded: the decision is about weapons, not effects.
                Attribution::Unresolved => {
                    matches!(record.observation, Observation::Countered)
                        || (record.fits.contains(&evidence) && record.windows.is_empty())
                }
                Attribution::Evidence { .. } | Attribution::EffectTick { .. } => false,
            };
            if !movable {
                continue;
            }
            match record.attribution {
                Attribution::Agrees { .. } => self.counts.agreed -= 1,
                Attribution::Unresolved => self.counts.unresolved -= 1,
                _ => {}
            }
            self.counts.evidenced += 1;
            let before = record.clone();
            record.attribution = Attribution::Evidence {
                tool: evidence.clone(),
                confirmed: true,
            };
            moves.push(Reprice {
                seq: record.seq,
                to_tool: evidence.clone(),
                record: before,
            });
        }
        moves.reverse();
        // The confirmation is the player's own declaration: a new regime
        // with the evidence weapon in hand.
        let previous = self.declared.replace(evidence.clone());
        self.previous = previous
            .filter(|previous| *previous != evidence)
            .map(|previous| (previous, at));
        self.recording = None;
        self.kept.clear();
        self.log.clear();
        Some(Decision {
            mismatch,
            moves,
            withdrawn: Vec::new(),
        })
    }

    /// Keep the declared weapon: the regime's evidence shots of the
    /// mismatch's weapon are repriced back to the declared weapon, and that
    /// weapon's evidence stops overriding it until the next regime. An
    /// effect one of those shots opened is taken back with it: the player
    /// says no such cast happened, so it explains no later tick.
    pub(super) fn keep(&mut self) -> Option<Decision> {
        let mismatch = self.mismatch.take()?;
        let declared = mismatch.declared.clone();
        let mut moves = Vec::new();
        let mut withdrawn = Vec::new();
        for record in &mut self.log {
            let Attribution::Evidence { tool, .. } = &record.attribution else {
                continue;
            };
            if *tool != mismatch.evidence {
                continue;
            }
            let before = record.clone();
            record.attribution = Attribution::Agrees {
                tool: declared.clone(),
                reason: AgreeReason::Kept,
            };
            if let Some(window) = record.opened_window.take() {
                withdrawn.push(window);
            }
            self.counts.evidenced -= 1;
            self.counts.agreed += 1;
            moves.push(Reprice {
                seq: record.seq,
                to_tool: declared.clone(),
                record: before,
            });
        }
        self.effect_windows
            .retain(|window| !withdrawn.contains(&window.id));
        self.kept.insert(mismatch.evidence.clone());
        Some(Decision {
            mismatch,
            moves,
            withdrawn,
        })
    }

    /// The observation amount of a logged shot (zero for a countered one).
    pub(super) fn amount_of(record: &ShotRecord) -> f64 {
        record.observation.amount()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ped::Ped;

    fn weapon(id: i64, name: &str, min: f64, max: f64) -> CarriedWeapon {
        CarriedWeapon {
            equipment_id: id,
            name: name.to_string(),
            band: Some(DamageBand { min, max }),
            effect: None,
        }
    }

    /// Pistol 5-10, Cannon 20-40, and a Rifle 8-16 overlapping the pistol.
    fn runtime() -> AttributionRuntime {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Pistol", 5.0, 10.0),
            weapon(2, "Cannon", 20.0, 40.0),
            weapon(3, "Rifle", 8.0, 16.0),
        ]);
        runtime
    }

    fn hit(amount: f64) -> Observation {
        Observation::Hit {
            amount,
            critical: false,
        }
    }

    fn crit(amount: f64) -> Observation {
        Observation::Hit {
            amount,
            critical: true,
        }
    }

    /// Classify, apply, and remember one observation, as the actor does.
    fn observe(runtime: &mut AttributionRuntime, observation: Observation, at: f64) -> Attribution {
        let resolved = runtime.classify(observation, at);
        let seq = runtime.apply(&resolved, at);
        runtime.remember(ShotRecord {
            seq,
            observed_at: at,
            observation,
            attribution: resolved.attribution.clone(),
            fits: resolved.fits.clone(),
            windows: resolved.windows.clone(),
            location: ShotLocation::Pending,
            evidence_id: None,
            booked: resolved.attribution.priced_tool().map(str::to_string),
            cost: Ped::ZERO,
            opened_window: None,
        });
        resolved.attribution
    }

    /// An effect window `owner` opened, ticking `tick_min`-`tick_max`.
    fn window(
        id: &str,
        owner: &str,
        started_at: f64,
        expires_at: f64,
        tick_min: f64,
        tick_max: f64,
    ) -> OffensiveEffectWindow {
        OffensiveEffectWindow {
            id: id.to_string(),
            tool: owner.to_string(),
            equipment_id: None,
            started_at,
            expires_at,
            tick: DamageBand {
                min: tick_min,
                max: tick_max,
            },
        }
    }

    fn tick_of(id: &str) -> Attribution {
        Attribution::EffectTick {
            window_id: Some(id.to_string()),
        }
    }

    /// The captured rotation's loadout: the primary chip (with its
    /// amplifier, 95.7-191.4) and an Electrocution chip whose declared
    /// effect lands a 100-160 hit, then ticks 35-75 for 25 seconds.
    fn rotation() -> AttributionRuntime {
        let effect = WeaponEffectProfile {
            mode: crate::weapon_effect::WeaponEffectMode::Compound,
            hit_min: Some(100.0),
            hit_max: Some(160.0),
            duration_seconds: 25.0,
            tick_min: 35.0,
            tick_max: 75.0,
            tick_seconds: Some(1.2),
        };
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Mayhem", 95.7, 191.4),
            CarriedWeapon {
                equipment_id: 2,
                name: "Electrocution".to_string(),
                band: Some(effect.activation_band()),
                effect: Some(effect),
            },
        ]);
        runtime
    }

    fn agrees(tool: &str, reason: AgreeReason) -> Attribution {
        Attribution::Agrees {
            tool: tool.to_string(),
            reason,
        }
    }

    fn evidence(tool: &str) -> Attribution {
        Attribution::Evidence {
            tool: tool.to_string(),
            confirmed: false,
        }
    }

    #[test]
    fn bands_admit_regular_hits_criticals_and_print_rounding() {
        let band = DamageBand {
            min: 5.0,
            max: 10.0,
        };
        assert!(band.fits(5.0, false));
        assert!(band.fits(10.0, false));
        assert!(band.fits(10.05, false), "half a printed step above");
        assert!(!band.fits(10.1, false));
        assert!(band.fits(4.95, false), "half a printed step below");
        assert!(!band.fits(4.9, false));
        assert!(!band.fits(30.0, false));
        assert!(band.fits(30.0, true), "a critical reaches three times");
        assert!(!band.fits(30.1, true));
        assert!(band.fits(6.0, true), "armour can absorb a critical's bonus");
        assert!(!band.fits(4.0, true));
        assert!(band.undershoots(4.9));
        assert!(!band.undershoots(4.95));
        assert!(!band.undershoots(12.0));
    }

    #[test]
    fn with_nothing_declared_one_fitting_weapon_is_evidence() {
        let mut runtime = runtime();
        assert_eq!(observe(&mut runtime, hit(30.0), 1.0), evidence("Cannon"));
        assert_eq!(runtime.recording(), Some("Cannon"));
        assert!(
            runtime.mismatch().is_none(),
            "nothing declared to disagree with"
        );
        // The pistol and the rifle both explain 9: no guess.
        assert_eq!(
            observe(&mut runtime, hit(9.0), 2.0),
            Attribution::Unresolved
        );
        // Above every band.
        assert_eq!(
            observe(&mut runtime, hit(50.0), 3.0),
            Attribution::Unresolved
        );
        // Below every band, with nothing declared to absorb it.
        assert_eq!(
            observe(&mut runtime, hit(2.0), 4.0),
            Attribution::Unresolved
        );
        // A countered shot inherits the weapon evidence last named.
        assert_eq!(
            observe(&mut runtime, Observation::Countered, 5.0),
            evidence("Cannon")
        );
        assert_eq!(
            runtime.counts(),
            AttributionCounts {
                agreed: 0,
                evidenced: 2,
                unresolved: 3,
                effect_ticks: 0,
            }
        );
    }

    #[test]
    fn a_countered_shot_with_no_weapon_known_is_unresolved() {
        let mut runtime = runtime();
        assert_eq!(
            observe(&mut runtime, Observation::Countered, 1.0),
            Attribution::Unresolved
        );
    }

    #[test]
    fn the_declared_weapon_wins_every_hit_its_band_explains() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        // 9 also fits the rifle: intent disambiguates.
        assert_eq!(
            observe(&mut runtime, hit(9.0), 1.0),
            agrees("Pistol", AgreeReason::Fits)
        );
        assert_eq!(
            observe(&mut runtime, crit(25.0), 2.0),
            agrees("Pistol", AgreeReason::Fits)
        );
        assert_eq!(
            observe(&mut runtime, Observation::Countered, 3.0),
            agrees("Pistol", AgreeReason::Countered)
        );
        assert!(runtime.mismatch().is_none());
        assert_eq!(runtime.recording(), Some("Pistol"));
    }

    #[test]
    fn undershooting_the_declared_band_with_no_other_fit_agrees() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Pistol", 5.0, 10.0),
            weapon(2, "Cannon", 20.0, 40.0),
        ]);
        runtime.declare("Cannon", 0.0);
        assert_eq!(
            observe(&mut runtime, hit(15.0), 1.0),
            agrees("Cannon", AgreeReason::BelowBand),
            "above the pistol, short of the cannon: armour, not a switch"
        );
        assert_eq!(
            observe(&mut runtime, hit(2.0), 2.0),
            agrees("Cannon", AgreeReason::BelowBand),
            "short of every band: still the weapon in hand"
        );
        assert_eq!(
            observe(&mut runtime, hit(45.0), 3.0),
            Attribution::Unresolved
        );
    }

    #[test]
    fn a_declared_weapon_without_a_band_is_never_contradicted() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            CarriedWeapon {
                equipment_id: 9,
                name: "Mystery".to_string(),
                band: None,
                effect: None,
            },
            weapon(2, "Cannon", 20.0, 40.0),
        ]);
        runtime.declare("Mystery", 0.0);
        assert_eq!(
            observe(&mut runtime, hit(30.0), 1.0),
            agrees("Mystery", AgreeReason::Unvalidated)
        );
        // A declared weapon outside the carried set is likewise unvalidated.
        runtime.declare("Borrowed", 2.0);
        assert_eq!(
            observe(&mut runtime, hit(30.0), 3.0),
            agrees("Borrowed", AgreeReason::Unvalidated)
        );
    }

    #[test]
    fn one_unambiguous_missed_switch_raises_a_mismatch_and_prices_the_evidence() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        assert_eq!(observe(&mut runtime, hit(30.0), 1.0), evidence("Cannon"));
        let mismatch = runtime.mismatch().unwrap().clone();
        assert_eq!(mismatch.declared, "Pistol");
        assert_eq!(mismatch.evidence, "Cannon");
        assert_eq!(mismatch.since, 1.0);
        assert_eq!(mismatch.shots, 1);
        assert_eq!(runtime.recording(), Some("Cannon"));
        // Countered shots inherit what is being recorded.
        assert_eq!(
            observe(&mut runtime, Observation::Countered, 2.0),
            evidence("Cannon")
        );
        assert_eq!(observe(&mut runtime, hit(22.0), 3.0), evidence("Cannon"));
        assert_eq!(runtime.mismatch().unwrap().shots, 3);
        assert_eq!(runtime.mismatch().unwrap().since, 1.0);
    }

    #[test]
    fn several_other_fitting_weapons_stay_unresolved() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Pistol", 5.0, 10.0),
            weapon(2, "Cannon", 20.0, 40.0),
            weapon(3, "Carbine", 25.0, 45.0),
        ]);
        runtime.declare("Pistol", 0.0);
        assert_eq!(
            observe(&mut runtime, hit(30.0), 1.0),
            Attribution::Unresolved
        );
        assert!(
            runtime.mismatch().is_none(),
            "ambiguity never raises a switch"
        );
    }

    #[test]
    fn a_hit_only_the_declared_weapon_explains_ends_the_mismatch() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(30.0), 1.0);
        // 9 fits the pistol and the rifle, not the cannon: the pistol is in
        // hand after all.
        observe(&mut runtime, hit(9.0), 2.0);
        assert!(runtime.mismatch().is_none());
        // A hit both the declared and the evidence weapon explain does not.
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Pistol", 5.0, 30.0),
            weapon(2, "Cannon", 20.0, 40.0),
        ]);
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(35.0), 1.0);
        observe(&mut runtime, hit(25.0), 2.0);
        assert!(runtime.mismatch().is_some());
    }

    #[test]
    fn the_previous_weapons_shot_landing_after_a_switch_is_no_mismatch() {
        let mut runtime = runtime();
        runtime.declare("Cannon", 0.0);
        runtime.declare("Pistol", 10.0);
        assert_eq!(
            observe(&mut runtime, hit(30.0), 10.8),
            agrees("Cannon", AgreeReason::InFlight)
        );
        assert!(runtime.mismatch().is_none());
        // Past the tail it is evidence of a missed switch back.
        assert_eq!(observe(&mut runtime, hit(30.0), 11.5), evidence("Cannon"));
        assert!(runtime.mismatch().is_some());
        // Re-pressing the same weapon keeps no previous weapon.
        runtime.declare("Pistol", 20.0);
        assert_eq!(observe(&mut runtime, hit(30.0), 20.5), evidence("Cannon"));
    }

    #[test]
    fn an_open_effect_window_explains_its_ticks_without_a_switch() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        runtime.open_effect_window(window("w1", "Zapper", 1.0, 11.0, 28.0, 32.0));
        assert_eq!(observe(&mut runtime, hit(30.0), 5.0), tick_of("w1"));
        assert!(runtime.mismatch().is_none(), "a tick never raises a switch");
        // The declared weapon's own hits still win first.
        assert_eq!(
            observe(&mut runtime, hit(9.0), 6.0),
            agrees("Pistol", AgreeReason::Fits)
        );
        // Past its expiry (and tail) the window no longer explains anything.
        assert_eq!(observe(&mut runtime, hit(30.0), 13.0), evidence("Cannon"));
        assert_eq!(runtime.counts().effect_ticks, 1);
    }

    #[test]
    fn a_hit_both_the_hotbar_weapon_and_another_weapons_effect_explain_is_unresolved() {
        let mut runtime = runtime();
        runtime.declare("Cannon", 0.0);
        runtime.open_effect_window(window("w1", "Zapper", 1.0, 11.0, 28.0, 32.0));
        let resolved = runtime.classify(hit(30.0), 5.0);
        assert_eq!(resolved.attribution, Attribution::Unresolved);
        assert_eq!(resolved.windows, vec!["w1".to_string()]);
        assert!(resolved.reason.contains("Zapper"), "{}", resolved.reason);
        // Outside the tick band the cannon's hits are the cannon's.
        assert_eq!(
            runtime.classify(hit(38.0), 5.5).attribution,
            agrees("Cannon", AgreeReason::Fits)
        );
    }

    #[test]
    fn a_tick_of_the_weapon_in_hands_own_effect_is_a_tick() {
        let mut runtime = rotation();
        runtime.declare("Electrocution", 0.0);
        // A 110 hit opens the effect; the declared weapon's own ticks
        // are ticks even where its hit band would hold them too.
        runtime.open_effect_window(window("e1", "Electrocution", 1.0, 26.0, 35.0, 120.0));
        let resolved = runtime.classify(hit(110.0), 2.0);
        assert_eq!(resolved.attribution, tick_of("e1"));
        assert_eq!(
            resolved.fits,
            vec!["Mayhem".to_string(), "Electrocution".to_string()]
        );
    }

    #[test]
    fn the_captured_rotation_bills_the_opener_once_and_the_primary_normally() {
        let mut runtime = rotation();
        runtime.declare("Electrocution", 0.0);
        // The opener agrees with the chip in hand; its effect opens.
        assert_eq!(
            observe(&mut runtime, hit(129.2), 1.0),
            agrees("Electrocution", AgreeReason::Fits)
        );
        runtime.open_effect_window(window("e1", "Electrocution", 1.0, 26.0, 35.0, 75.0));
        // A sliver short of every band is nearest the effect's floor.
        assert_eq!(observe(&mut runtime, hit(0.8), 1.1), tick_of("e1"));
        assert_eq!(observe(&mut runtime, hit(53.0), 2.0), tick_of("e1"));
        runtime.declare("Mayhem", 2.4);
        assert_eq!(observe(&mut runtime, hit(57.5), 3.0), tick_of("e1"));
        // Short of the primary's band but above every tick: armour on a
        // primary hit, not a tick.
        assert_eq!(
            observe(&mut runtime, hit(90.0), 4.0),
            agrees("Mayhem", AgreeReason::BelowBand)
        );
        assert_eq!(
            observe(&mut runtime, hit(147.7), 4.1),
            agrees("Mayhem", AgreeReason::Fits)
        );
        assert_eq!(observe(&mut runtime, hit(70.4), 5.0), tick_of("e1"));
        // A jam is a shot of the weapon in hand, never a tick.
        assert_eq!(
            observe(&mut runtime, Observation::Countered, 6.0),
            agrees("Mayhem", AgreeReason::Countered)
        );
        assert!(runtime.mismatch().is_none(), "ticks never raise a switch");
        // Past the effect (and its tail) a low hit is the primary's again.
        assert_eq!(
            observe(&mut runtime, hit(57.5), 28.0),
            agrees("Mayhem", AgreeReason::BelowBand)
        );
        assert_eq!(
            runtime.counts(),
            AttributionCounts {
                agreed: 5,
                evidenced: 0,
                unresolved: 0,
                effect_ticks: 4,
            }
        );
    }

    #[test]
    fn a_hit_short_of_every_band_goes_to_the_nearest_floor() {
        let mut runtime = rotation();
        runtime.declare("Mayhem", 0.0);
        runtime.open_effect_window(window("e1", "Electrocution", 1.0, 26.0, 35.0, 75.0));
        assert_eq!(runtime.classify(hit(20.0), 2.0).attribution, tick_of("e1"));
        // An effect whose floor sits above the declared weapon's does not
        // out-reach it.
        let mut runtime = rotation();
        runtime.declare("Mayhem", 0.0);
        runtime.open_effect_window(window("hi", "Electrocution", 1.0, 26.0, 120.0, 130.0));
        assert_eq!(
            runtime.classify(hit(20.0), 2.0).attribution,
            agrees("Mayhem", AgreeReason::BelowBand)
        );
        // Of two effects short of the hit, the nearer floor explains it.
        let mut runtime = rotation();
        runtime.declare("Mayhem", 0.0);
        runtime.open_effect_window(window("far", "Electrocution", 1.0, 26.0, 60.0, 75.0));
        runtime.open_effect_window(window("near", "Zapper", 1.0, 26.0, 30.0, 40.0));
        let resolved = runtime.classify(hit(20.0), 2.0);
        assert_eq!(resolved.attribution, tick_of("near"));
        assert_eq!(resolved.windows, vec!["near".to_string()]);
    }

    #[test]
    fn overlapping_windows_keep_every_candidate_and_choose_none() {
        let mut runtime = rotation();
        runtime.declare("Mayhem", 0.0);
        runtime.open_effect_window(window("e1", "Electrocution", 1.0, 26.0, 35.0, 75.0));
        runtime.open_effect_window(window("e2", "Electrocution", 10.0, 35.0, 35.0, 75.0));
        let resolved = runtime.classify(hit(50.0), 12.0);
        assert_eq!(
            resolved.attribution,
            Attribution::EffectTick { window_id: None }
        );
        assert_eq!(resolved.windows, vec!["e1".to_string(), "e2".to_string()]);
        assert!(resolved.attribution.priced_tool().is_none());
        // Before the second opened, only the first explains a tick.
        assert_eq!(runtime.classify(hit(50.0), 5.0).attribution, tick_of("e1"));
        // A below-floor sliver both windows reach is ambiguous too.
        let resolved = runtime.classify(hit(1.0), 12.0);
        assert_eq!(
            resolved.attribution,
            Attribution::EffectTick { window_id: None }
        );
        assert_eq!(resolved.windows.len(), 2);
    }

    #[test]
    fn with_nothing_declared_an_open_effect_explains_its_ticks() {
        let mut runtime = rotation();
        runtime.open_effect_window(window("e1", "Electrocution", 1.0, 26.0, 35.0, 75.0));
        assert_eq!(runtime.classify(hit(50.0), 2.0).attribution, tick_of("e1"));
        assert_eq!(runtime.classify(hit(3.0), 2.0).attribution, tick_of("e1"));
        // A primary hit is still evidence of the primary.
        assert_eq!(
            runtime.classify(hit(180.0), 2.0).attribution,
            evidence("Mayhem")
        );
        // Short of everything, an effect claims the hit only when its floor
        // is the nearest: a nearer weapon floor leaves it unresolved.
        let mut high = rotation();
        high.open_effect_window(window("h1", "Electrocution", 1.0, 26.0, 120.0, 130.0));
        assert_eq!(
            high.classify(hit(20.0), 2.0).attribution,
            Attribution::Unresolved
        );
        // A tick band the primary's band also holds: two live sources.
        let mut runtime = rotation();
        runtime.open_effect_window(window("e1", "Electrocution", 1.0, 26.0, 35.0, 100.0));
        let resolved = runtime.classify(hit(98.0), 2.0);
        assert_eq!(resolved.attribution, Attribution::Unresolved);
        assert_eq!(resolved.windows, vec!["e1".to_string()]);
        // The effect's own weapon fitting is no rival: over-time-only
        // weapons are confirmed by a tick-shaped hit.
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![weapon(2, "Electrocution", 35.0, 75.0)]);
        runtime.open_effect_window(window("e1", "Electrocution", 1.0, 26.0, 35.0, 75.0));
        assert_eq!(runtime.classify(hit(50.0), 2.0).attribution, tick_of("e1"));
    }

    #[test]
    fn the_previous_weapons_in_flight_shot_competes_with_another_effect() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Mayhem", 150.0, 191.4),
            weapon(2, "Electrocution", 100.0, 140.0),
        ]);
        runtime.declare("Electrocution", 0.0);
        runtime.declare("Mayhem", 1.0);
        // The chip's own effect: a hit its effect ticks and its in-flight
        // shot would print alike is a tick, not a second activation.
        runtime.open_effect_window(window("e1", "Electrocution", 0.5, 25.5, 110.0, 130.0));
        assert_eq!(runtime.classify(hit(120.0), 1.5).attribution, tick_of("e1"));
        // Out of the tick band, the chip's shot still lands as its own.
        assert_eq!(
            runtime.classify(hit(135.0), 1.5).attribution,
            agrees("Electrocution", AgreeReason::InFlight)
        );
        // A third weapon's effect beside the chip's in-flight shot: both
        // explain it.
        runtime.set_effect_windows(vec![window("z1", "Zapper", 0.5, 25.5, 110.0, 130.0)]);
        let resolved = runtime.classify(hit(120.0), 1.5);
        assert_eq!(resolved.attribution, Attribution::Unresolved);
        assert!(
            resolved.reason.contains("still landing"),
            "{}",
            resolved.reason
        );
        // Past the switch tail, the third weapon's effect alone explains it.
        assert_eq!(runtime.classify(hit(120.0), 3.0).attribution, tick_of("z1"));
    }

    #[test]
    fn a_bandless_declared_weapon_competes_with_an_effect_it_does_not_own() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![CarriedWeapon {
            equipment_id: 9,
            name: "Mystery".to_string(),
            band: None,
            effect: None,
        }]);
        runtime.declare("Mystery", 0.0);
        runtime.open_effect_window(window("e1", "Electrocution", 1.0, 26.0, 35.0, 75.0));
        assert_eq!(
            runtime.classify(hit(50.0), 2.0).attribution,
            Attribution::Unresolved
        );
        assert_eq!(
            runtime.classify(hit(90.0), 2.0).attribution,
            agrees("Mystery", AgreeReason::Unvalidated)
        );
        runtime.set_effect_windows(vec![window("m1", "Mystery", 1.0, 26.0, 35.0, 75.0)]);
        assert_eq!(runtime.classify(hit(50.0), 2.0).attribution, tick_of("m1"));
    }

    #[test]
    fn a_window_explains_ticks_until_its_expiry_and_delivery_tail() {
        let mut runtime = rotation();
        runtime.declare("Mayhem", 0.0);
        runtime.open_effect_window(window("e1", "Electrocution", 10.0, 20.0, 35.0, 75.0));
        assert_eq!(
            runtime.classify(hit(50.0), 9.0).attribution,
            agrees("Mayhem", AgreeReason::BelowBand),
            "before its activation landed"
        );
        assert_eq!(runtime.classify(hit(50.0), 9.96).attribution, tick_of("e1"));
        assert_eq!(runtime.classify(hit(50.0), 21.2).attribution, tick_of("e1"));
        assert_eq!(
            runtime.classify(hit(50.0), 21.3).attribution,
            agrees("Mayhem", AgreeReason::BelowBand)
        );
        // Applying an observation past the tail forgets the window.
        observe(&mut runtime, hit(150.0), 30.0);
        assert!(runtime.effect_windows().is_empty());
    }

    #[test]
    fn a_decision_leaves_a_hit_an_effect_could_have_ticked() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Pistol", 5.0, 30.0),
            weapon(2, "Cannon", 20.0, 40.0),
        ]);
        runtime.declare("Pistol", 0.0);
        runtime.open_effect_window(window("z1", "Zapper", 0.0, 60.0, 20.0, 30.0));
        // The pistol and the effect both explain it: unresolved, though
        // the cannon fits it too.
        assert_eq!(
            observe(&mut runtime, hit(25.0), 1.0),
            Attribution::Unresolved
        );
        assert_eq!(observe(&mut runtime, hit(35.0), 2.0), evidence("Cannon"));
        let decision = runtime.confirm(3.0).unwrap();
        assert!(
            decision.moves.is_empty(),
            "the unresolved hit might be a tick: {:?}",
            decision.moves
        );
        assert_eq!(runtime.counts().unresolved, 1);
    }

    #[test]
    fn keeping_the_hotbar_weapon_takes_back_the_effect_its_evidence_opened() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Pistol", 5.0, 10.0),
            weapon(2, "Electrocution", 100.0, 160.0),
        ]);
        runtime.declare("Pistol", 0.0);
        assert_eq!(
            observe(&mut runtime, hit(130.0), 1.0),
            evidence("Electrocution")
        );
        // As the actor does: the priced hit opened its effect.
        let seq = runtime.log().last().unwrap().seq;
        runtime.record_mut(seq).unwrap().opened_window = Some("e1".to_string());
        runtime.open_effect_window(window("e1", "Electrocution", 1.0, 26.0, 35.0, 75.0));
        assert_eq!(runtime.classify(hit(55.0), 2.0).attribution, tick_of("e1"));

        let decision = runtime.keep().unwrap();
        assert_eq!(decision.withdrawn, vec!["e1".to_string()]);
        assert!(runtime.effect_windows().is_empty());
        assert_ne!(runtime.classify(hit(55.0), 3.0).attribution, tick_of("e1"));
        // A confirm takes nothing back.
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Pistol", 5.0, 10.0),
            weapon(2, "Electrocution", 100.0, 160.0),
        ]);
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(130.0), 1.0);
        assert!(runtime.confirm(2.0).unwrap().withdrawn.is_empty());
    }

    #[test]
    fn a_declared_effect_replaces_the_catalogue_band() {
        let props = serde_json::json!({
            "weapon_entity": {"damage": {"electric": 2000.0}},
            "effect_profile": {
                "mode": "compound",
                "hit_min": 100.0,
                "hit_max": 160.0,
                "duration_seconds": 25.0,
                "tick_min": 35.0,
                "tick_max": 75.0,
            },
        });
        assert_eq!(
            damage_band_from_props(&props),
            Some(DamageBand {
                min: 100.0,
                max: 160.0
            })
        );
    }

    #[test]
    fn a_press_starts_a_new_regime() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(30.0), 1.0);
        assert!(runtime.mismatch().is_some());
        runtime.resync();
        assert!(runtime.mismatch().is_none(), "a harvesting press re-syncs");
        assert!(runtime.log().is_empty());
        assert_eq!(runtime.declared(), Some("Pistol"));
        observe(&mut runtime, hit(30.0), 2.0);
        runtime.declare("Cannon", 3.0);
        assert!(runtime.mismatch().is_none());
        assert!(runtime.log().is_empty());
    }

    #[test]
    fn confirming_reprices_the_regime_back_to_the_last_proof_of_the_declared_weapon() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(9.0), 1.0); // pistol or rifle: 0
        observe(&mut runtime, hit(6.0), 2.0); // pistol only: proof, 1
        observe(&mut runtime, Observation::Countered, 3.0); // 2: moves
        observe(&mut runtime, hit(9.0), 4.0); // pistol or rifle; not cannon: proof vs cannon, 3
        observe(&mut runtime, Observation::Countered, 5.0); // 4: moves
        observe(&mut runtime, hit(30.0), 6.0); // cannon evidence, 5
        observe(&mut runtime, Observation::Countered, 7.0); // cannon evidence, 6

        let decision = runtime.confirm(8.0).unwrap();
        assert_eq!(decision.mismatch.evidence, "Cannon");
        assert_eq!(
            decision
                .moves
                .iter()
                .map(|m| (m.seq, m.to_tool.as_str()))
                .collect::<Vec<_>>(),
            vec![(4, "Cannon")]
        );
        assert_eq!(
            decision.moves[0].record.attribution,
            agrees("Pistol", AgreeReason::Countered),
            "the move carries the shot as it stood"
        );
        assert_eq!(runtime.declared(), Some("Cannon"));
        assert!(runtime.mismatch().is_none());
        assert!(
            runtime.log().is_empty(),
            "the confirmation starts a new regime"
        );
        assert_eq!(
            runtime.counts(),
            AttributionCounts {
                agreed: 4,
                evidenced: 3,
                unresolved: 0,
                effect_ticks: 0,
            }
        );
        // The pistol's in-flight shots still land as the pistol's.
        assert_eq!(
            observe(&mut runtime, hit(6.0), 8.5),
            agrees("Pistol", AgreeReason::InFlight)
        );
    }

    #[test]
    fn confirming_moves_shared_band_hits_and_unresolved_hits_the_evidence_fits() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Pistol", 5.0, 22.0),
            weapon(2, "Cannon", 20.0, 40.0),
            weapon(3, "Carbine", 30.0, 45.0),
        ]);
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(21.0), 1.0); // pistol+cannon: agrees, fits cannon too
        observe(&mut runtime, hit(35.0), 2.0); // cannon+carbine: unresolved
        observe(&mut runtime, hit(44.0), 3.0); // carbine only: evidence carbine
        observe(&mut runtime, hit(25.0), 4.0); // cannon only: evidence cannon (replaces)
        assert_eq!(runtime.mismatch().unwrap().evidence, "Cannon");
        let decision = runtime.confirm(5.0).unwrap();
        let moved: Vec<u64> = decision.moves.iter().map(|m| m.seq).collect();
        assert_eq!(
            moved,
            vec![0, 1],
            "the carbine's evidence stays the carbine's"
        );
        assert_eq!(runtime.counts().unresolved, 0);
        assert_eq!(runtime.counts().evidenced, 4);
    }

    #[test]
    fn keeping_reprices_the_evidence_back_and_silences_that_weapon_for_the_regime() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(6.0), 1.0);
        observe(&mut runtime, hit(30.0), 2.0);
        observe(&mut runtime, Observation::Countered, 3.0);
        observe(&mut runtime, hit(9.0), 4.0); // proof: clears, but evidence stays logged
        observe(&mut runtime, hit(31.0), 5.0);
        let decision = runtime.keep().unwrap();
        assert_eq!(
            decision.moves.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![1, 2, 4],
            "every cannon-evidence shot of the regime"
        );
        assert!(decision.moves.iter().all(|m| m.to_tool == "Pistol"));
        assert!(runtime.mismatch().is_none());
        assert_eq!(runtime.declared(), Some("Pistol"));
        // The cannon's band no longer overrides the pistol this regime.
        assert_eq!(
            observe(&mut runtime, hit(30.0), 6.0),
            agrees("Pistol", AgreeReason::Kept)
        );
        assert!(runtime.mismatch().is_none());
        // A confirm has nothing to act on.
        assert!(runtime.confirm(7.0).is_none());
        assert!(runtime.keep().is_none());
        // The next press ends the reprieve.
        runtime.declare("Pistol", 8.0);
        assert_eq!(observe(&mut runtime, hit(30.0), 9.0), evidence("Cannon"));
    }

    #[test]
    fn a_kept_shot_bounds_a_later_confirmation() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(30.0), 1.0); // cannon evidence
        runtime.keep().unwrap();
        observe(&mut runtime, Observation::Countered, 2.0); // agrees pistol (countered)
        observe(&mut runtime, hit(13.0), 3.0); // rifle only: evidence rifle
        let decision = runtime.confirm(4.0).unwrap();
        assert_eq!(decision.mismatch.evidence, "Rifle");
        // The countered shot moves; the walk stops at the kept cannon shot.
        assert_eq!(
            decision.moves.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn settling_a_kill_moves_pending_shots_into_it() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(30.0), 1.0);
        runtime.settle_kill("k1");
        observe(&mut runtime, hit(31.0), 2.0);
        let locations: Vec<ShotLocation> = runtime
            .log()
            .iter()
            .map(|record| record.location.clone())
            .collect();
        assert_eq!(
            locations,
            vec![ShotLocation::Kill("k1".to_string()), ShotLocation::Pending]
        );
    }

    #[test]
    fn the_log_forgets_what_no_decision_can_reach() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, Observation::Countered, 1.0);
        observe(&mut runtime, hit(30.0), 2.0); // evidence: kept in the log
        observe(&mut runtime, hit(9.0), 3.0); // pistol+rifle: no proof on its own
        assert_eq!(runtime.log().len(), 3);
        observe(&mut runtime, hit(6.0), 4.0); // pistol only: proof
        let seqs: Vec<u64> = runtime.log().iter().map(|record| record.seq).collect();
        assert_eq!(seqs, vec![1, 3]);
    }

    #[test]
    fn a_band_derives_from_the_stored_weapon_amp_and_enhancers() {
        let plain = serde_json::json!({
            "weapon_entity": {"damage": {"impact": 10.0}},
        });
        assert_eq!(
            damage_band_from_props(&plain),
            Some(DamageBand {
                min: 5.0,
                max: 10.0
            })
        );
        let amped = serde_json::json!({
            "weapon_entity": {"damage": {"impact": 10.0}},
            "amp_entity": {"damage": {"burn": 4.0}},
        });
        assert_eq!(
            damage_band_from_props(&amped),
            Some(DamageBand {
                min: 7.0,
                max: 14.0
            })
        );
        let null_amp = serde_json::json!({
            "weapon_entity": {"damage": {"impact": 10.0}},
            "amp_entity": null,
        });
        assert_eq!(
            damage_band_from_props(&null_amp),
            damage_band_from_props(&plain)
        );
        let enhanced = serde_json::json!({
            "weapon_entity": {"damage": {"impact": 10.0}},
            "damage_enhancers": 2,
        });
        let band = damage_band_from_props(&enhanced).unwrap();
        assert!(band.max > 10.0, "enhancers raise the band: {band:?}");
        assert_eq!(damage_band_from_props(&serde_json::json!({})), None);
        assert_eq!(
            damage_band_from_props(&serde_json::json!({"weapon_entity": {"economy": {}}})),
            None
        );
    }

    #[test]
    fn a_band_past_the_attack_rate_limit_scales_with_its_cost() {
        let props = serde_json::json!({
            "weapon_entity": {"uses_per_minute": 90, "damage": {"impact": 20.0}},
        });
        let own = damage_band_from_props(&props).unwrap();
        let rated =
            damage_band_from_props(&crate::attack_rate::with_attack_rate(&props, None, 30.0))
                .unwrap();
        assert!((rated.min - 11.7).abs() < 1e-9 && (rated.max - 23.4).abs() < 1e-9);
        // A full hit of the buffed weapon is out of its own-rate profile, and
        // fits once the band carries the rate the server compressed.
        assert!(!own.fits(23.4, false));
        assert!(rated.fits(23.4, false));
        // Within the limit the band is the weapon's own.
        let within =
            damage_band_from_props(&crate::attack_rate::with_attack_rate(&props, None, 10.0))
                .unwrap();
        assert_eq!(within, own);
    }

    #[test]
    fn a_declared_effect_band_is_observed_so_the_attack_rate_leaves_it_alone() {
        let props = serde_json::json!({
            "weapon_entity": {"uses_per_minute": 90, "damage": {"electric": 2000.0}},
            "effect_profile": {
                "mode": "compound",
                "hit_min": 100.0,
                "hit_max": 160.0,
                "duration_seconds": 25.0,
                "tick_min": 35.0,
                "tick_max": 75.0,
            },
        });
        assert_eq!(
            damage_band_from_props(&crate::attack_rate::with_attack_rate(&props, None, 30.0)),
            damage_band_from_props(&props)
        );
    }

    #[test]
    fn stored_kinds_round_trip() {
        for kind in [
            AttributionKind::Agrees,
            AttributionKind::EffectTick,
            AttributionKind::Evidence,
            AttributionKind::Unresolved,
        ] {
            assert_eq!(AttributionKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(AttributionKind::parse("other"), None);
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        #[derive(Debug, Clone)]
        enum Step {
            Hit(f64, bool),
            Countered,
            Declare(usize),
            Resync,
            Confirm,
            Keep,
            /// An effect `OWNERS[owner]` opened now, for `duration` seconds,
            /// ticking `tick_min` to `tick_min + width`.
            Effect {
                owner: usize,
                duration: f64,
                tick_min: f64,
                width: f64,
            },
        }

        fn step() -> impl Strategy<Value = Step> {
            prop_oneof![
                6 => (0.0f64..130.0, any::<bool>()).prop_map(|(amount, critical)| Step::Hit(amount, critical)),
                2 => Just(Step::Countered),
                1 => (0usize..3).prop_map(Step::Declare),
                1 => Just(Step::Resync),
                1 => Just(Step::Confirm),
                1 => Just(Step::Keep),
                1 => (0usize..4, 0.5f64..30.0, 0.0f64..60.0, 0.0f64..30.0).prop_map(
                    |(owner, duration, tick_min, width)| Step::Effect { owner, duration, tick_min, width }
                ),
            ]
        }

        const NAMES: [&str; 3] = ["Pistol", "Cannon", "Rifle"];
        /// The carried weapons, and one weapon nobody carries.
        const OWNERS: [&str; 4] = ["Pistol", "Cannon", "Rifle", "Zapper"];

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(96))]

            /// The tallies always partition the observations; a decision
            /// only moves shots between states, never adds or drops one; a
            /// mismatch never names the declared weapon; a decision only
            /// reaches shots logged in its own regime.
            #[test]
            fn tallies_partition_and_decisions_stay_in_their_regime(
                steps in proptest::collection::vec(step(), 1..60),
            ) {
                let mut runtime = runtime();
                let mut observed = 0i64;
                let mut regime_start = 0u64;
                let mut at = 0.0;
                for step in steps {
                    at += 0.7;
                    match step {
                        Step::Hit(amount, critical) => {
                            let observation = Observation::Hit { amount, critical };
                            let attribution = observe(&mut runtime, observation, at);
                            observed += 1;
                            if let Attribution::Evidence { tool, .. } = &attribution {
                                prop_assert!(runtime.carried().iter().any(|w| &w.name == tool));
                            }
                        }
                        Step::Countered => {
                            observe(&mut runtime, Observation::Countered, at);
                            observed += 1;
                        }
                        Step::Declare(index) => {
                            runtime.declare(NAMES[index], at);
                            regime_start = runtime.next_seq;
                        }
                        Step::Resync => {
                            runtime.resync();
                            regime_start = runtime.next_seq;
                        }
                        Step::Confirm => {
                            let before = runtime.counts();
                            if let Some(decision) = runtime.confirm(at) {
                                prop_assert!(decision.moves.iter().all(|m| m.seq >= regime_start));
                                prop_assert!(decision.moves.iter().all(|m| m.to_tool == decision.mismatch.evidence));
                                prop_assert_eq!(runtime.declared(), Some(decision.mismatch.evidence.as_str()));
                                let after = runtime.counts();
                                prop_assert_eq!(after.evidenced - before.evidenced, decision.moves.len() as i64);
                                regime_start = runtime.next_seq;
                            }
                        }
                        Step::Keep => {
                            if let Some(decision) = runtime.keep() {
                                prop_assert!(decision.moves.iter().all(|m| m.seq >= regime_start));
                                prop_assert!(decision.moves.iter().all(|m| m.to_tool == decision.mismatch.declared));
                            }
                        }
                        Step::Effect { owner, duration, tick_min, width } => {
                            let id = format!("w{}", runtime.effect_windows().len());
                            runtime.open_effect_window(window(
                                &id,
                                OWNERS[owner],
                                at,
                                at + duration,
                                tick_min,
                                tick_min + width,
                            ));
                        }
                    }
                    let counts = runtime.counts();
                    prop_assert_eq!(
                        counts.agreed + counts.evidenced + counts.unresolved + counts.effect_ticks,
                        observed
                    );
                    prop_assert!(counts.agreed >= 0 && counts.evidenced >= 0 && counts.unresolved >= 0);
                    if let Some(mismatch) = runtime.mismatch() {
                        prop_assert_eq!(Some(mismatch.declared.as_str()), runtime.declared());
                        prop_assert_ne!(&mismatch.evidence, &mismatch.declared);
                        prop_assert!(!runtime.kept.contains(&mismatch.evidence));
                    }
                }
            }

            /// Bill once: a hit any open effect's ticks explain is never
            /// priced as a shot; a tick never carries a price, and names its
            /// window exactly when one alone explains it; every window a
            /// classification cites was open; and a hit short of the
            /// declared band is only priced when no open effect's floor
            /// sits nearer above it.
            #[test]
            fn a_hit_an_open_effect_explains_is_never_priced(
                windows in proptest::collection::vec(
                    (0usize..4, 0.0f64..20.0, 0.5f64..30.0, 0.0f64..60.0, 0.0f64..30.0),
                    0..4,
                ),
                declare in proptest::option::of(0usize..3),
                previous in proptest::option::of(0usize..3),
                amount in 0.0f64..130.0,
                critical in any::<bool>(),
                at in 0.0f64..40.0,
            ) {
                let mut runtime = runtime();
                if let Some(index) = previous {
                    runtime.declare(NAMES[index], at - 0.5);
                }
                if let Some(index) = declare {
                    runtime.declare(NAMES[index], at - 0.5);
                }
                for (index, (owner, start, duration, tick_min, width)) in windows.iter().enumerate() {
                    runtime.open_effect_window(window(
                        &format!("w{index}"),
                        OWNERS[*owner],
                        *start,
                        start + duration,
                        *tick_min,
                        tick_min + width,
                    ));
                }
                let resolved = runtime.classify(Observation::Hit { amount, critical }, at);
                let live: Vec<&OffensiveEffectWindow> = runtime
                    .effect_windows()
                    .iter()
                    .filter(|window| window.live_at(at))
                    .collect();
                let ticking = live.iter().any(|window| window.tick.fits(amount, critical));
                if resolved.attribution.priced_tool().is_some() {
                    prop_assert!(!ticking, "priced a hit an open effect explains: {resolved:?}");
                }
                for id in &resolved.windows {
                    prop_assert!(live.iter().any(|window| &window.id == id));
                }
                match &resolved.attribution {
                    Attribution::EffectTick { window_id } => {
                        prop_assert!(!resolved.windows.is_empty());
                        prop_assert_eq!(window_id.is_some(), resolved.windows.len() == 1);
                        prop_assert!(resolved.attribution.priced_tool().is_none());
                    }
                    Attribution::Agrees { tool, reason: AgreeReason::BelowBand } => {
                        let floor = runtime.band_of(tool).unwrap().min;
                        let nearer = live
                            .iter()
                            .any(|window| window.tick.undershoots(amount) && window.tick.min < floor);
                        prop_assert!(!nearer, "priced past a nearer effect floor: {:?}", resolved);
                    }
                    _ => {}
                }
            }

            /// With a weapon declared, a hit its band explains always agrees
            /// with it, whatever else fits.
            #[test]
            fn intent_wins_every_hit_its_band_explains(
                declared in 0usize..3,
                amount in 0.0f64..130.0,
                critical in any::<bool>(),
            ) {
                let mut runtime = runtime();
                runtime.declare(NAMES[declared], 0.0);
                let band = runtime.band_of(NAMES[declared]).unwrap();
                let resolved = runtime.classify(Observation::Hit { amount, critical }, 5.0);
                if band.fits(amount, critical) {
                    prop_assert_eq!(resolved.attribution, agrees(NAMES[declared], AgreeReason::Fits));
                } else {
                    let fits_declared = matches!(
                        resolved.attribution,
                        Attribution::Agrees { reason: AgreeReason::Fits, .. }
                    );
                    prop_assert!(!fits_declared);
                }
            }

            /// Evidence names a weapon only when exactly one band (other than
            /// the declared one) fits; an unresolved hit never carries a tool.
            #[test]
            fn evidence_is_always_unambiguous(
                amount in 0.0f64..130.0,
                critical in any::<bool>(),
                declare in proptest::option::of(0usize..3),
            ) {
                let mut runtime = runtime();
                if let Some(index) = declare {
                    runtime.declare(NAMES[index], 0.0);
                }
                let resolved = runtime.classify(Observation::Hit { amount, critical }, 5.0);
                match &resolved.attribution {
                    Attribution::Evidence { tool, .. } => {
                        let others: Vec<&String> = resolved
                            .fits
                            .iter()
                            .filter(|name| Some(name.as_str()) != runtime.declared())
                            .collect();
                        prop_assert_eq!(others, vec![tool]);
                    }
                    Attribution::Unresolved => {
                        prop_assert!(resolved.attribution.priced_tool().is_none());
                    }
                    _ => {}
                }
            }
        }
    }
}
