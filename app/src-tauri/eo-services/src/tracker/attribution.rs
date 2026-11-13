//! Unified weapon attribution: which weapon an offensive chat-log line
//! came from, and therefore what it cost.
//!
//! One engine, no modes. A hotbar press declares the weapon in hand and is
//! the primary signal; each carried weapon's damage band quietly validates
//! it. Every observation resolves to one of four states:
//!
//! - **Agrees**: the declared weapon explains it (its band fits, it has no
//!   band to contradict, the hit lands in the margin around its band, or
//!   the previous weapon's shot is still landing just after a switch).
//!   Priced to that weapon.
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
//! A band is what a weapon lists, not everything it can print. Mob armour
//! and sub-maximal skill lower a hit, and a temporary damage buff raises
//! one, up to [`BOOSTED_REACH`] of the band's ceiling. So a hit outside the
//! declared weapon's band that no other carried weapon explains still
//! agrees with the declared weapon: expected, priced as usual, and marked
//! boosted or reduced so the player can catch a run of them they cannot
//! account for. A hit past the boosted reach is out of profile and stays
//! unresolved.
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

use std::collections::{BTreeSet, VecDeque};

use serde_json::Value;

use crate::cost_engine::weapon_damage_profile_from_props;
use crate::weapon_effect::{effect_profile_from_props, WeaponEffectProfile};

/// A critical hit reaches at most this multiple of a weapon's maximum.
pub const CRITICAL_REACH: f64 = 3.0;

/// Chat-log damage prints to one decimal, so a printed figure can sit up
/// to half a step outside the band it was rolled from.
pub const DISPLAY_TOLERANCE: f64 = 0.05;

/// How far a temporary damage buff can carry a hit past its weapon's
/// ceiling, as a multiple of that ceiling.
pub const BOOSTED_REACH: f64 = 1.5;

/// How far below a weapon's floor post-play review still offers it for an
/// unpriced hit, as a multiple of that floor. The held weapon has no such
/// limit: armour can push its hits arbitrarily low.
pub const REDUCED_REACH: f64 = 0.5;

/// After a hotbar switch, a shot the previous weapon already fired can
/// still arrive for this long (the same delivery tail healing allows).
pub(super) const SWITCH_TAIL_SECONDS: f64 = 1.25;

/// The latest hits of the held weapon the overlay's damage cue reads.
const CUE_HITS: usize = 20;

/// A reduced cue needs most of the last few hits below the band: one low
/// hit is ordinary armour, while a run says something is different.
const REDUCED_CUE_SPAN: usize = 5;
const REDUCED_CUE_HITS: usize = 3;

/// Which side of a weapon's listed range a hit landed on, inside the margin
/// the weapon can still explain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HitMargin {
    /// Above the listed range: a temporary damage buff.
    Boosted,
    /// Below the listed range: armour, or sub-maximal skill.
    Reduced,
}

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
        amount >= self.min - DISPLAY_TOLERANCE
            && amount <= self.ceiling(critical) + DISPLAY_TOLERANCE
    }

    /// The most a hit can print without a buff.
    fn ceiling(&self, critical: bool) -> f64 {
        if critical {
            self.max * CRITICAL_REACH
        } else {
            self.max
        }
    }

    /// Whether the hit falls short of the band's floor.
    pub fn undershoots(&self, amount: f64) -> bool {
        amount < self.min - DISPLAY_TOLERANCE
    }

    /// The margin a hit outside the band sits in. None inside the band, and
    /// past either reach.
    pub fn margin(&self, amount: f64, critical: bool) -> Option<HitMargin> {
        let ceiling = self.ceiling(critical);
        if amount > ceiling + DISPLAY_TOLERANCE {
            return (amount <= ceiling * BOOSTED_REACH + DISPLAY_TOLERANCE)
                .then_some(HitMargin::Boosted);
        }
        if self.undershoots(amount) {
            return (amount >= self.min * REDUCED_REACH - DISPLAY_TOLERANCE)
                .then_some(HitMargin::Reduced);
        }
        None
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
    /// The hit lands above its band, within the boosted reach, and no other
    /// weapon explains it.
    AboveBand,
    /// The previous weapon's shot, landing just after the switch.
    InFlight,
    /// A countered shot inherits the declared weapon.
    Countered,
    /// The player kept the declared weapon over this evidence.
    Kept,
}

impl AgreeReason {
    /// The margin of a hit agreed to from outside its weapon's band.
    pub(super) fn margin(self) -> Option<HitMargin> {
        match self {
            AgreeReason::AboveBand => Some(HitMargin::Boosted),
            AgreeReason::BelowBand => Some(HitMargin::Reduced),
            _ => None,
        }
    }
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
    /// row: evidence that overrode a declared weapon, hits outside the held
    /// weapon's band, unresolved shots, and effect ticks.
    pub(super) fn kind(&self) -> AttributionKind {
        match self {
            Attribution::Agrees { reason, .. } => match reason.margin() {
                Some(HitMargin::Boosted) => AttributionKind::Boosted,
                Some(HitMargin::Reduced) => AttributionKind::Reduced,
                None => AttributionKind::Agrees,
            },
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
    Boosted,
    Reduced,
}

impl AttributionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AttributionKind::Agrees => "agrees",
            AttributionKind::EffectTick => "effect_tick",
            AttributionKind::Evidence => "evidence",
            AttributionKind::Unresolved => "unresolved",
            AttributionKind::Boosted => "boosted",
            AttributionKind::Reduced => "reduced",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "agrees" => Some(AttributionKind::Agrees),
            "effect_tick" => Some(AttributionKind::EffectTick),
            "evidence" => Some(AttributionKind::Evidence),
            "unresolved" => Some(AttributionKind::Unresolved),
            "boosted" => Some(AttributionKind::Boosted),
            "reduced" => Some(AttributionKind::Reduced),
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
    /// Hits inside the declared weapon's band, and the shots that inherit it.
    pub agreed: i64,
    /// Hits priced to the declared weapon from outside its band.
    pub in_margin: i64,
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
    /// The declared weapon's latest hits, oldest first, each with the
    /// margin it landed in. Cleared with the regime.
    recent_hits: VecDeque<Option<HitMargin>>,
    /// A tool known only from the game's data, tested beside the carried
    /// weapons while it is held and for the switch tail after.
    provisional: Option<ProvisionalBand>,
    /// The shots kept past the regime floor, while something may still
    /// move them (see [`ShotRetention`]); None keeps none.
    retention: Option<ShotRetention>,
}

/// A held tool the equipment library does not hold, known only from the
/// game's data: its name and band, apart from the carried weapons, since
/// no library entry stands behind it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProvisionalBand {
    pub(super) name: String,
    pub(super) band: Option<DamageBand>,
    /// When it stops being tested, once it is no longer held: the end of
    /// the switch tail. None while it is held.
    pub(super) until: Option<f64>,
}

/// Where and how a shot was recorded, beyond what the regime log keeps:
/// what a stored row written for it later needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ShotContext {
    pub(super) context_id: Option<i64>,
    pub(super) reload_speed_percent: f64,
}

/// One shot kept by the retention, with where it was recorded.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RetainedShot {
    pub(super) record: ShotRecord,
    pub(super) context: ShotContext,
    /// What the shot added to the unpriced shots' bound while it is
    /// unpriced; None while it is priced.
    pub(super) charge: Option<UnpricedCharge>,
}

/// What one unpriced shot adds to the session's disclosure of unpriced
/// cost: the dearest carried weapon that could explain it, or a count of
/// shots no weapon explains.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum UnpricedCharge {
    Bound(crate::ped::Ped),
    NoFit,
}

/// The shots something read after them may still move: every shot of the
/// last `window` seconds, and every shot since `pinned` while it is set.
/// The regime log forgets what no decision can reach; a change read off
/// the screen can reach further, since it can arrive after shots it
/// predates, and a tool unseen for a while is settled only when the
/// screen shows it again.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct ShotRetention {
    window: f64,
    pinned: Option<f64>,
    /// Every shot observed at or after this moment is kept: when the
    /// retention began, raised as older shots are let go.
    floor: f64,
    shots: Vec<RetainedShot>,
}

impl ShotRetention {
    fn prune(&mut self, latest: f64) {
        let mut floor = latest - self.window;
        if let Some(pinned) = self.pinned {
            floor = floor.min(pinned);
        }
        self.shots.retain(|shot| shot.record.observed_at >= floor);
        self.floor = self.floor.max(floor);
    }
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

    /// What the held weapon's latest hits say about its damage: boosted as
    /// soon as any of them landed above its band, reduced once most of the
    /// last few landed below it. Nothing while a mismatch stands, which
    /// already questions the weapon, or with no weapon declared.
    pub(super) fn hit_cue(&self) -> Option<HitMargin> {
        if self.declared.is_none() || self.mismatch.is_some() {
            return None;
        }
        if self.recent_hits.contains(&Some(HitMargin::Boosted)) {
            return Some(HitMargin::Boosted);
        }
        let reduced = self
            .recent_hits
            .iter()
            .rev()
            .take(REDUCED_CUE_SPAN)
            .filter(|margin| **margin == Some(HitMargin::Reduced))
            .count();
        (reduced >= REDUCED_CUE_HITS).then_some(HitMargin::Reduced)
    }

    /// The carried weapons whose margin holds a hit.
    pub(super) fn in_margin(&self, amount: f64, critical: bool) -> Vec<String> {
        self.carried
            .iter()
            .filter(|weapon| {
                weapon
                    .band
                    .is_some_and(|band| band.margin(amount, critical).is_some())
            })
            .map(|weapon| weapon.name.clone())
            .collect()
    }

    #[cfg(test)]
    pub(super) fn log(&self) -> &[ShotRecord] {
        &self.log
    }

    #[cfg(test)]
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
        self.recent_hits.clear();
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

    /// Nothing is in hand from `at` (a tool read off the screen was put
    /// away, or the screen cannot say which is held): a new regime with no
    /// weapon declared, whose shots are classified as with none ever
    /// declared, save that the weapon declared until now may still land a
    /// shot for a moment.
    pub(super) fn undeclare(&mut self, at: f64) {
        if let Some(previous) = self.declared.take() {
            self.previous = Some((previous, at));
        }
        self.recording = None;
        self.floor();
    }

    /// Forget the weapon whose shots may still be landing: it was not
    /// seen in hand before the change, so no later shot is its by timing.
    pub(super) fn forget_previous(&mut self) {
        self.previous = None;
    }

    /// Test a tool known only from the game's data beside the carried
    /// weapons, for as long as it is held.
    pub(super) fn carry_provisional(&mut self, name: &str, band: Option<DamageBand>) {
        self.provisional = Some(ProvisionalBand {
            name: name.to_string(),
            band,
            until: None,
        });
    }

    /// The provisional tool is no longer held from `at`: it is tested for
    /// the switch tail after, then not at all.
    pub(super) fn release_provisional(&mut self, at: f64) {
        if let Some(provisional) = self.provisional.as_mut() {
            if provisional.until.is_none() {
                provisional.until = Some(at + SWITCH_TAIL_SECONDS);
            }
        }
    }

    /// Forget the provisional tool once its tail has passed at `now`.
    pub(super) fn expire_provisional(&mut self, now: f64) {
        if self
            .provisional
            .as_ref()
            .and_then(|provisional| provisional.until)
            .is_some_and(|until| now > until)
        {
            self.provisional = None;
        }
    }

    pub(super) fn provisional_band(&self) -> Option<&ProvisionalBand> {
        self.provisional.as_ref()
    }

    #[cfg(test)]
    pub(super) fn provisional(&self) -> Option<&ProvisionalBand> {
        self.provisional.as_ref()
    }

    /// Keep every shot of the last `window` seconds past the regime floor
    /// (see [`ShotRetention`]), until [`Self::stop_retaining`].
    pub(super) fn retain_shots(&mut self, window: f64, now: f64) {
        let retention = self.retention.get_or_insert_with(|| ShotRetention {
            floor: now,
            ..ShotRetention::default()
        });
        retention.window = window;
    }

    /// The moment from which every shot is retained; None while none are.
    pub(super) fn retention_floor(&self) -> Option<f64> {
        self.retention.as_ref().map(|retention| retention.floor)
    }

    /// Take the retained shots observed at or after `from` out of the
    /// retention, oldest first, to be placed again.
    pub(super) fn take_retained_since(&mut self, from: f64) -> Vec<RetainedShot> {
        let Some(retention) = self.retention.as_mut() else {
            return Vec::new();
        };
        let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut retention.shots)
            .into_iter()
            .partition(|shot| shot.record.observed_at >= from);
        retention.shots = kept;
        taken
    }

    /// Put a shot back in the retention as it was, in its place by time.
    pub(super) fn restore_retained(&mut self, shot: RetainedShot) {
        if let Some(retention) = self.retention.as_mut() {
            let index = retention
                .shots
                .iter()
                .position(|kept| {
                    (kept.record.observed_at, kept.record.seq)
                        > (shot.record.observed_at, shot.record.seq)
                })
                .unwrap_or(retention.shots.len());
            retention.shots.insert(index, shot);
        }
    }

    pub(super) fn stop_retaining(&mut self) {
        self.retention = None;
    }

    /// Keep every shot since `since` as well, however old, while set; None
    /// lets them go once they leave the window.
    pub(super) fn pin_retention(&mut self, since: Option<f64>, now: f64) {
        if let Some(retention) = self.retention.as_mut() {
            retention.pinned = since;
            retention.prune(now);
        }
    }

    /// The retained shots observed at or after `from`, oldest first.
    pub(super) fn retained_since(&self, from: f64) -> Vec<RetainedShot> {
        self.retention
            .as_ref()
            .map(|retention| {
                retention
                    .shots
                    .iter()
                    .filter(|shot| shot.record.observed_at >= from)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Remember a shot of the current regime (see [`Self::remember`]), and
    /// keep it with where it was recorded while shots are retained.
    pub(super) fn remember_shot(
        &mut self,
        record: ShotRecord,
        context: ShotContext,
        charge: Option<UnpricedCharge>,
    ) {
        if let Some(retention) = self.retention.as_mut() {
            let latest = record.observed_at;
            retention.shots.push(RetainedShot {
                record: record.clone(),
                context,
                charge,
            });
            retention.prune(latest);
        }
        self.remember(record);
    }

    /// What a retained shot now adds to the unpriced shots' bound.
    pub(super) fn set_charge(&mut self, seq: u64, charge: Option<UnpricedCharge>) {
        if let Some(retention) = self.retention.as_mut() {
            for shot in &mut retention.shots {
                if shot.record.seq == seq {
                    shot.charge = charge;
                }
            }
        }
    }

    /// Whether an open effect window could tick an observation at `at`.
    pub(super) fn ticks_at(&self, observation: Observation, at: f64) -> bool {
        match observation {
            Observation::Hit { amount, critical } => !self.ticking(at, amount, critical).is_empty(),
            Observation::Countered => false,
        }
    }

    /// Take one shot's tally back out: it is about to be placed again.
    pub(super) fn untally(&mut self, attribution: &Attribution) {
        Self::tally(&mut self.counts, attribution, -1);
    }

    /// A shot moved to another weapon (or to none): how it is attributed,
    /// where it is booked, at what, and its stored row, in the regime log
    /// and the retention alike.
    pub(super) fn rebook(
        &mut self,
        seq: u64,
        attribution: Option<&Attribution>,
        booked: Option<String>,
        cost: crate::ped::Ped,
        evidence_id: Option<String>,
    ) {
        let retained = self
            .retention
            .as_mut()
            .into_iter()
            .flat_map(|retention| retention.shots.iter_mut().map(|shot| &mut shot.record));
        for record in self.log.iter_mut().chain(retained) {
            if record.seq != seq {
                continue;
            }
            if let Some(attribution) = attribution {
                record.attribution = attribution.clone();
            }
            record.booked = booked.clone();
            record.cost = cost;
            if evidence_id.is_some() {
                record.evidence_id = evidence_id.clone();
            }
        }
    }

    /// Move one shot's tally from how it was attributed to how it is now.
    pub(super) fn recount(&mut self, from: &Attribution, to: &Attribution) {
        Self::tally(&mut self.counts, from, -1);
        Self::tally(&mut self.counts, to, 1);
    }

    fn tally(counts: &mut AttributionCounts, attribution: &Attribution, step: i64) {
        match attribution {
            Attribution::Agrees { reason, .. } if reason.margin().is_some() => {
                counts.in_margin += step
            }
            Attribution::Agrees { .. } => counts.agreed += step,
            Attribution::Evidence { .. } => counts.evidenced += step,
            Attribution::Unresolved => counts.unresolved += step,
            Attribution::EffectTick { .. } => counts.effect_ticks += step,
        }
    }

    fn band_of(&self, tool: &str) -> Option<DamageBand> {
        self.carried
            .iter()
            .find(|weapon| weapon.name == tool)
            .and_then(|weapon| weapon.band)
            .or_else(|| {
                self.provisional
                    .as_ref()
                    .filter(|provisional| provisional.name == tool)
                    .and_then(|provisional| provisional.band)
            })
    }

    fn fitting(&self, amount: f64, critical: bool) -> Vec<String> {
        self.carried
            .iter()
            .filter(|weapon| weapon.band.is_some_and(|band| band.fits(amount, critical)))
            .map(|weapon| weapon.name.clone())
            .collect()
    }

    /// The carried weapons whose band fits a hit at `at`, and the
    /// provisional tool's while it is still tested then.
    fn fitting_at(&self, at: f64, amount: f64, critical: bool) -> Vec<String> {
        let mut fits = self.fitting(amount, critical);
        if let Some(provisional) = &self.provisional {
            let tested = provisional.until.is_none_or(|until| at <= until);
            if tested
                && provisional
                    .band
                    .is_some_and(|band| band.fits(amount, critical))
                && !fits.contains(&provisional.name)
            {
                fits.push(provisional.name.clone());
            }
        }
        fits
    }

    /// The weapon declared before the latest change, when a hit at `at`
    /// lands inside the switch tail and its band fits: a shot it fired
    /// before the change, still arriving.
    fn in_flight(&self, at: f64, amount: f64, critical: bool) -> Option<String> {
        self.in_flight_since(at, amount, critical)
            .map(|(previous, _)| previous)
    }

    /// [`Self::in_flight`], with when the change was.
    fn in_flight_since(&self, at: f64, amount: f64, critical: bool) -> Option<(String, f64)> {
        self.previous.as_ref().and_then(|(previous, switched_at)| {
            let in_tail = at >= switched_at - 0.05 && at - switched_at <= SWITCH_TAIL_SECONDS;
            (in_tail
                && self
                    .band_of(previous)
                    .is_some_and(|band| band.fits(amount, critical)))
            .then(|| (previous.clone(), *switched_at))
        })
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
        let fits = self.fitting_at(at, amount, critical);
        let ticking = self.ticking(at, amount, critical);
        let Some(declared) = self.declared.clone() else {
            return self.classify_undeclared(amount, critical, at, fits, &ticking);
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

        let in_flight = self.in_flight(at, amount, critical);
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
                    reason: "below the held weapon's range".to_string(),
                    fits,
                    windows: Vec::new(),
                }
            }
            [] if band.margin(amount, critical) == Some(HitMargin::Boosted) => Resolved {
                attribution: Attribution::Agrees {
                    tool: declared,
                    reason: AgreeReason::AboveBand,
                },
                reason: "above the held weapon's range".to_string(),
                fits,
                windows: Vec::new(),
            },
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
        critical: bool,
        at: f64,
        fits: Vec<String>,
        ticking: &[&OffensiveEffectWindow],
    ) -> Resolved {
        // The weapon declared until nothing was (a tool read off the screen
        // put away) may still be landing a shot, and competes with an effect
        // it does not own. With nothing in hand there is no declaration for
        // the tail to stand beside, so it takes a shot only when that
        // weapon alone explains it, or when the shot was seen before the
        // tool was put away (one already in flight).
        let landing = self
            .in_flight_since(at, amount, critical)
            .filter(|(previous, switched_at)| {
                at < *switched_at || fits.as_slice() == std::slice::from_ref(previous)
            })
            .map(|(previous, _)| previous);
        if let Some(previous) = landing {
            if !ticking.is_empty() && !Self::all_owned_by(ticking, &previous) {
                return Self::concurrent(
                    ticking,
                    fits,
                    &format!("{previous}'s shot still landing"),
                );
            }
            if ticking.is_empty() {
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
        }
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
                match reason.margin() {
                    Some(_) => self.counts.in_margin += 1,
                    None => self.counts.agreed += 1,
                }
                if matches!(
                    reason,
                    AgreeReason::Fits | AgreeReason::AboveBand | AgreeReason::BelowBand
                ) {
                    self.recent_hits.push_back(reason.margin());
                    if self.recent_hits.len() > CUE_HITS {
                        self.recent_hits.pop_front();
                    }
                }
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
        let retained = self
            .retention
            .as_mut()
            .into_iter()
            .flat_map(|retention| retention.shots.iter_mut().map(|shot| &mut shot.record));
        for record in self.log.iter_mut().chain(retained) {
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
    /// countered shot, a hit either weapon explains, a hit in the margin
    /// around the declared band, and an unresolved hit the evidence weapon
    /// fits (unless an open effect could equally have ticked it) all move.
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
                    AgreeReason::BelowBand
                    | AgreeReason::AboveBand
                    | AgreeReason::Countered
                    | AgreeReason::Unvalidated => true,
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
            match &record.attribution {
                Attribution::Agrees { reason, .. } if reason.margin().is_some() => {
                    self.counts.in_margin -= 1;
                }
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
        self.recent_hits.clear();
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
    fn a_margin_runs_from_half_the_floor_to_half_again_the_ceiling() {
        let band = DamageBand {
            min: 10.0,
            max: 20.0,
        };
        assert_eq!(band.margin(15.0, false), None, "inside the band");
        assert_eq!(
            band.margin(20.05, false),
            None,
            "print rounding, not a buff"
        );
        assert_eq!(band.margin(20.1, false), Some(HitMargin::Boosted));
        assert_eq!(band.margin(30.05, false), Some(HitMargin::Boosted));
        assert_eq!(band.margin(30.1, false), None, "past the boosted reach");
        assert_eq!(band.margin(9.9, false), Some(HitMargin::Reduced));
        assert_eq!(band.margin(4.95, false), Some(HitMargin::Reduced));
        assert_eq!(band.margin(4.9, false), None, "past the reduced reach");
        // A critical's reach stands in for the ceiling, buffed the same way.
        assert_eq!(band.margin(60.0, true), None, "inside a critical's reach");
        assert_eq!(band.margin(60.1, true), Some(HitMargin::Boosted));
        assert_eq!(band.margin(90.1, true), None);
    }

    #[test]
    fn a_hit_above_the_held_weapons_band_is_priced_to_it_as_boosted() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Pistol", 5.0, 10.0),
            weapon(2, "Cannon", 20.0, 40.0),
        ]);
        runtime.declare("Pistol", 0.0);
        assert_eq!(
            observe(&mut runtime, hit(12.0), 1.0),
            agrees("Pistol", AgreeReason::AboveBand)
        );
        assert_eq!(
            observe(&mut runtime, hit(15.0), 2.0),
            agrees("Pistol", AgreeReason::AboveBand),
            "the top of the boosted reach"
        );
        assert_eq!(
            observe(&mut runtime, hit(15.1), 3.0),
            Attribution::Unresolved,
            "past it, the hit is out of profile"
        );
        assert_eq!(
            observe(&mut runtime, hit(25.0), 4.0),
            Attribution::Evidence {
                tool: "Cannon".to_string(),
                confirmed: false,
            },
            "another weapon's band still says a switch was missed"
        );
        let counts = runtime.counts();
        assert_eq!((counts.agreed, counts.in_margin), (0, 2));
        assert_eq!(counts.unresolved, 1);
    }

    #[test]
    fn with_nothing_declared_no_margin_names_a_weapon() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![weapon(1, "Pistol", 5.0, 10.0)]);
        assert_eq!(
            observe(&mut runtime, hit(12.0), 1.0),
            Attribution::Unresolved
        );
    }

    #[test]
    fn margin_hits_keep_their_classification_on_their_stored_row() {
        let kind = |reason| {
            Attribution::Agrees {
                tool: "Pistol".to_string(),
                reason,
            }
            .kind()
        };
        assert_eq!(kind(AgreeReason::AboveBand), AttributionKind::Boosted);
        assert_eq!(kind(AgreeReason::BelowBand), AttributionKind::Reduced);
        assert_eq!(kind(AgreeReason::Fits), AttributionKind::Agrees);
        for variant in [AttributionKind::Boosted, AttributionKind::Reduced] {
            assert_eq!(AttributionKind::parse(variant.as_str()), Some(variant));
        }
    }

    #[test]
    fn confirming_a_switch_moves_the_margin_hits_since_the_last_proof() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Pistol", 5.0, 10.0),
            weapon(2, "Cannon", 20.0, 40.0),
        ]);
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(12.0), 1.0);
        observe(&mut runtime, hit(3.0), 2.0);
        observe(&mut runtime, hit(25.0), 3.0);
        assert_eq!(runtime.counts().in_margin, 2);
        let decision = runtime.confirm(4.0).expect("a mismatch stands");
        assert_eq!(
            decision.moves.len(),
            2,
            "both margin hits were the cannon's"
        );
        let counts = runtime.counts();
        assert_eq!((counts.in_margin, counts.evidenced), (0, 3));
    }

    #[test]
    fn the_cue_reads_boosted_from_any_recent_hit_and_reduced_from_a_run() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![weapon(1, "Pistol", 5.0, 10.0)]);
        assert_eq!(runtime.hit_cue(), None, "nothing declared");
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(7.0), 1.0);
        assert_eq!(runtime.hit_cue(), None, "inside the band");

        observe(&mut runtime, hit(12.0), 2.0);
        assert_eq!(runtime.hit_cue(), Some(HitMargin::Boosted));
        for at in 0..CUE_HITS {
            assert_eq!(runtime.hit_cue(), Some(HitMargin::Boosted));
            observe(&mut runtime, hit(7.0), 3.0 + at as f64);
        }
        assert_eq!(runtime.hit_cue(), None, "the buff has run out");

        // One low hit is ordinary armour; a run of them is not.
        observe(&mut runtime, hit(3.0), 40.0);
        observe(&mut runtime, hit(3.0), 41.0);
        assert_eq!(runtime.hit_cue(), None);
        observe(&mut runtime, hit(3.0), 42.0);
        assert_eq!(runtime.hit_cue(), Some(HitMargin::Reduced));

        runtime.declare("Pistol", 43.0);
        assert_eq!(runtime.hit_cue(), None, "a press starts over");
    }

    #[test]
    fn a_standing_mismatch_takes_over_from_the_cue() {
        let mut runtime = AttributionRuntime::default();
        runtime.set_carried(vec![
            weapon(1, "Pistol", 5.0, 10.0),
            weapon(2, "Cannon", 20.0, 40.0),
        ]);
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(12.0), 1.0);
        assert_eq!(runtime.hit_cue(), Some(HitMargin::Boosted));
        observe(&mut runtime, hit(25.0), 2.0);
        assert!(runtime.mismatch().is_some());
        assert_eq!(runtime.hit_cue(), None);
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
                in_margin: 0,
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
            observe(&mut runtime, hit(61.0), 3.0),
            Attribution::Unresolved,
            "past the boosted reach of every band"
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
                agreed: 3,
                in_margin: 2,
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
                in_margin: 0,
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
                        counts.agreed
                            + counts.in_margin
                            + counts.evidenced
                            + counts.unresolved
                            + counts.effect_ticks,
                        observed
                    );
                    prop_assert!(
                        counts.agreed >= 0
                            && counts.in_margin >= 0
                            && counts.evidenced >= 0
                            && counts.unresolved >= 0
                    );
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

    /// Nothing in hand: a new regime with no weapon declared. Each case is
    /// (the hit, seconds after the tool was put away, how it resolves).
    #[test]
    fn with_nothing_in_hand_shots_resolve_as_with_none_declared() {
        let cases: &[(f64, f64, Attribution)] = &[
            // The pistol alone fits, inside the tail: still landing.
            (7.0, 0.5, agrees("Pistol", AgreeReason::InFlight)),
            // Past the tail, the damage evidence names the one fit.
            (7.0, 2.0, evidence("Pistol")),
            // The pistol and the rifle both fit: past the tail, unresolved.
            (9.0, 2.0, Attribution::Unresolved),
            // Inside the tail, but the rifle fits too: with nothing in hand
            // the tail does not choose between them.
            (9.0, 1.0, Attribution::Unresolved),
            // Seen before the tool was put away, it is still landing.
            (9.0, -0.02, agrees("Pistol", AgreeReason::InFlight)),
            // The cannon alone, inside the tail: evidence, not the pistol.
            (30.0, 0.5, evidence("Cannon")),
            // No weapon fits: unresolved, never the tool put away.
            (3.0, 0.5, Attribution::Unresolved),
        ];
        for (amount, after, expected) in cases {
            let mut runtime = runtime();
            runtime.declare("Pistol", 0.0);
            runtime.undeclare(10.0);
            assert_eq!(runtime.declared(), None);
            assert_eq!(
                runtime.classify(hit(*amount), 10.0 + after).attribution,
                *expected,
                "{amount} at {after}s"
            );
        }
    }

    #[test]
    fn putting_the_tool_away_starts_a_regime_no_evidence_reaches_past() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(30.0), 1.0);
        assert!(runtime.mismatch().is_some());
        runtime.undeclare(2.0);
        assert!(runtime.mismatch().is_none());
        assert!(runtime.log().is_empty());
        assert_eq!(runtime.hit_cue(), None);
        // Put away again with nothing in hand: the tail stays the first's.
        runtime.undeclare(5.0);
        assert_eq!(
            runtime.classify(hit(7.0), 2.5).attribution,
            agrees("Pistol", AgreeReason::InFlight)
        );
        runtime.forget_previous();
        assert_eq!(
            runtime.classify(hit(7.0), 2.5).attribution,
            evidence("Pistol")
        );
    }

    #[test]
    fn a_tool_known_only_from_the_game_data_is_tested_while_held_and_for_its_tail() {
        let mut runtime = runtime();
        runtime.carry_provisional(
            "Rusty",
            Some(DamageBand {
                min: 100.0,
                max: 200.0,
            }),
        );
        runtime.declare("Rusty", 0.0);
        assert_eq!(
            runtime.classify(hit(150.0), 1.0).attribution,
            agrees("Rusty", AgreeReason::Fits)
        );
        assert!(runtime
            .carried()
            .iter()
            .all(|weapon| weapon.name != "Rusty"));
        runtime.declare("Cannon", 5.0);
        runtime.release_provisional(5.0);
        // Inside the tail it still lands; its fit counts beside the others'.
        assert_eq!(
            runtime.classify(hit(150.0), 5.5).attribution,
            agrees("Rusty", AgreeReason::InFlight)
        );
        runtime.expire_provisional(5.5);
        assert!(runtime.provisional().is_some());
        runtime.expire_provisional(7.0);
        assert!(runtime.provisional().is_none());
        assert_eq!(
            runtime.classify(hit(150.0), 7.0).attribution,
            Attribution::Unresolved
        );
    }

    #[test]
    fn retained_shots_outlive_the_regime_floor_and_follow_kills_and_moves() {
        let context = ShotContext {
            context_id: Some(4),
            reload_speed_percent: 0.0,
        };
        let mut runtime = runtime();
        runtime.retain_shots(5.0, 0.0);
        assert_eq!(runtime.retention_floor(), Some(0.0));
        runtime.declare("Pistol", 0.0);
        for at in [1.0, 2.0, 3.0] {
            let resolved = runtime.classify(hit(7.0), at);
            let seq = runtime.apply(&resolved, at);
            runtime.remember_shot(
                ShotRecord {
                    seq,
                    observed_at: at,
                    observation: hit(7.0),
                    attribution: resolved.attribution.clone(),
                    fits: resolved.fits.clone(),
                    windows: Vec::new(),
                    location: ShotLocation::Pending,
                    evidence_id: None,
                    booked: Some("Pistol".into()),
                    cost: Ped(0.1),
                    opened_window: None,
                },
                context,
                None,
            );
        }
        // Proof of the pistol forgets the older shots in the log, and a new
        // regime forgets them all; the retention keeps them.
        runtime.declare("Cannon", 4.0);
        assert!(runtime.log().is_empty());
        assert_eq!(runtime.retained_since(0.0).len(), 3);
        assert_eq!(runtime.retained_since(2.0).len(), 2);
        runtime.settle_kill("k1");
        runtime.rebook(
            1,
            Some(&Attribution::Unresolved),
            None,
            Ped::ZERO,
            Some("row".into()),
        );
        let kept = runtime.retained_since(0.0);
        assert!(kept
            .iter()
            .all(|shot| shot.record.location == ShotLocation::Kill("k1".into())));
        assert_eq!(kept[1].record.booked, None);
        assert_eq!(kept[1].record.evidence_id.as_deref(), Some("row"));
        assert_eq!(kept[1].record.attribution, Attribution::Unresolved);
        assert_eq!(kept[1].context, context);
        // Taken out to be placed again, and put back in its place by time.
        let taken = runtime.take_retained_since(2.0);
        assert_eq!(taken.len(), 2);
        assert_eq!(runtime.retained_since(0.0).len(), 1);
        for shot in taken.into_iter().rev() {
            runtime.restore_retained(shot);
        }
        let order: Vec<u64> = runtime
            .retained_since(0.0)
            .iter()
            .map(|shot| shot.record.seq)
            .collect();
        assert_eq!(order, [0, 1, 2]);
        runtime.set_charge(2, Some(UnpricedCharge::NoFit));
        assert_eq!(
            runtime.retained_since(3.0)[0].charge,
            Some(UnpricedCharge::NoFit)
        );
        // Pinned, an old shot stays past the window; unpinned, it goes.
        runtime.pin_retention(Some(1.0), 20.0);
        assert_eq!(runtime.retained_since(0.0).len(), 3);
        runtime.pin_retention(None, 20.0);
        assert!(runtime.retained_since(0.0).is_empty());
        assert_eq!(
            runtime.retention_floor(),
            Some(15.0),
            "nothing older is kept now"
        );
        runtime.stop_retaining();
        assert!(runtime.retained_since(0.0).is_empty());
    }

    #[test]
    fn recounting_moves_one_tally_to_another() {
        let mut runtime = runtime();
        runtime.declare("Pistol", 0.0);
        observe(&mut runtime, hit(7.0), 1.0);
        observe(&mut runtime, hit(4.0), 1.5);
        runtime.recount(
            &agrees("Pistol", AgreeReason::Fits),
            &Attribution::Unresolved,
        );
        runtime.recount(
            &agrees("Pistol", AgreeReason::BelowBand),
            &evidence("Rifle"),
        );
        let counts = runtime.counts();
        assert_eq!(
            (
                counts.agreed,
                counts.in_margin,
                counts.unresolved,
                counts.evidenced
            ),
            (0, 0, 1, 1)
        );
    }

    mod nothing_in_hand_properties {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// With nothing in hand past the tail, a hit is priced only when
            /// exactly one carried weapon explains it, and never to a tool
            /// by being the last in hand.
            #[test]
            fn past_the_tail_only_a_lone_fit_is_priced(
                amount in 0.0f64..130.0,
                critical in any::<bool>(),
                after in 1.3f64..60.0,
            ) {
                let mut runtime = runtime();
                runtime.declare("Rifle", 0.0);
                runtime.undeclare(10.0);
                let resolved = runtime.classify(Observation::Hit { amount, critical }, 10.0 + after);
                match resolved.attribution.priced_tool() {
                    Some(tool) => prop_assert_eq!(resolved.fits.clone(), vec![tool.to_string()]),
                    None => prop_assert!(resolved.fits.len() != 1),
                }
                let in_flight = matches!(
                    resolved.attribution,
                    Attribution::Agrees { reason: AgreeReason::InFlight, .. }
                );
                prop_assert!(!in_flight);
            }
        }
    }
}
