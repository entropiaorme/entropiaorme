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
//!   explains the magnitude. No shot and no cost. (No producer opens
//!   offensive windows yet; this is the seam damage-over-time fills.)
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
//! The engine is pure state over observations; pricing, persistence, and
//! the kill accumulator belong to the tracker actor that drives it.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::cost_engine::get_weapon_damage_profile;

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

/// A stored weapon's regular-hit band at full skill: its catalogue damage
/// with its amplifier and configured damage enhancers. None when the
/// catalogue exposes no usable damage figure.
pub fn damage_band_from_props(props: &Value) -> Option<DamageBand> {
    let weapon = props.get("weapon_entity")?;
    let enhancers = (props
        .get("damage_enhancers")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as i64)
        .max(0);
    let profile = get_weapon_damage_profile(
        weapon,
        props.get("amp_entity").filter(|amp| !amp.is_null()),
        enhancers,
    )?;
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
}

/// One offensive chat-log observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Observation {
    /// A hit with its printed magnitude.
    Hit { amount: f64, critical: bool },
    /// A jam, dodge, or evade: a shot was fired, but no magnitude says by
    /// what.
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
    EffectTick {
        window_id: String,
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
/// observation (every carried weapon, for a countered shot) and the reason
/// a stored row records.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Resolved {
    pub(super) attribution: Attribution,
    pub(super) fits: Vec<String>,
    pub(super) reason: String,
}

/// An open window of an effect some earlier paid activation owns. Its
/// ticks are outcomes of that activation, never new shots.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OffensiveEffectWindow {
    pub(super) id: String,
    pub(super) started_at: f64,
    pub(super) expires_at: f64,
    pub(super) tick: DamageBand,
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
    pub(super) location: ShotLocation,
    /// The shot's stored evidence row, when its state keeps one.
    pub(super) evidence_id: Option<String>,
    /// The weapon whose phase the shot is counted under (None: unpriced).
    pub(super) booked: Option<String>,
    /// The per-shot cost booked for it (zero when unpriced).
    pub(super) cost: crate::ped::Ped,
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
    #[cfg(test)]
    pub(super) fn open_effect_window(&mut self, window: OffensiveEffectWindow) {
        self.effect_windows.push(window);
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

    fn open_window(&self, at: f64, amount: f64) -> Option<&OffensiveEffectWindow> {
        let mut matches = self.effect_windows.iter().filter(|window| {
            window.started_at <= at + 0.05
                && at <= window.expires_at + SWITCH_TAIL_SECONDS
                && window.tick.fits(amount, false)
        });
        let first = matches.next()?;
        // Two windows explaining the same tick leave its source ambiguous;
        // either way it is no shot, so the first stands in for provenance.
        Some(first)
    }

    /// Classify one observation without changing any state.
    pub(super) fn classify(&self, observation: Observation, at: f64) -> Resolved {
        let (amount, critical) = match observation {
            Observation::Hit { amount, critical } => (amount, critical),
            Observation::Countered => return self.classify_countered(),
        };
        let fits = self.fitting(amount, critical);
        let Some(declared) = self.declared.clone() else {
            return match fits.as_slice() {
                [only] => Resolved {
                    attribution: Attribution::Evidence {
                        tool: only.clone(),
                        confirmed: false,
                    },
                    reason: format!("fits only {only}"),
                    fits,
                },
                [] => Resolved {
                    attribution: Attribution::Unresolved,
                    reason: "fits no carried weapon".to_string(),
                    fits,
                },
                _ => Resolved {
                    attribution: Attribution::Unresolved,
                    reason: "fits several carried weapons".to_string(),
                    fits,
                },
            };
        };

        let declared_band = self.band_of(&declared);
        let Some(band) = declared_band else {
            return Resolved {
                attribution: Attribution::Agrees {
                    tool: declared,
                    reason: AgreeReason::Unvalidated,
                },
                reason: String::new(),
                fits,
            };
        };
        if band.fits(amount, critical) {
            return Resolved {
                attribution: Attribution::Agrees {
                    tool: declared,
                    reason: AgreeReason::Fits,
                },
                reason: String::new(),
                fits,
            };
        }
        if let Some((previous, switched_at)) = &self.previous {
            let in_tail = at >= switched_at - 0.05 && at - switched_at <= SWITCH_TAIL_SECONDS;
            if in_tail
                && self
                    .band_of(previous)
                    .is_some_and(|band| band.fits(amount, critical))
            {
                return Resolved {
                    attribution: Attribution::Agrees {
                        tool: previous.clone(),
                        reason: AgreeReason::InFlight,
                    },
                    reason: String::new(),
                    fits,
                };
            }
        }
        if let Some(window) = self.open_window(at, amount) {
            return Resolved {
                attribution: Attribution::EffectTick {
                    window_id: window.id.clone(),
                },
                reason: "a tick of an open effect window".to_string(),
                fits,
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
                }
            }
            [] if band.undershoots(amount) => Resolved {
                attribution: Attribution::Agrees {
                    tool: declared,
                    reason: AgreeReason::BelowBand,
                },
                reason: String::new(),
                fits,
            },
            [] => Resolved {
                attribution: Attribution::Unresolved,
                reason: "exceeds every carried weapon's reach".to_string(),
                fits,
            },
            _ => Resolved {
                attribution: Attribution::Unresolved,
                reason: "fits several carried weapons other than the hotbar's".to_string(),
                fits,
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
            },
            None => Resolved {
                attribution: Attribution::Unresolved,
                reason: "a countered shot with no weapon known".to_string(),
                fits,
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
    /// band, and an unresolved hit the evidence weapon fits all move.
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
                Attribution::Unresolved => {
                    matches!(record.observation, Observation::Countered)
                        || record.fits.contains(&evidence)
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
        Some(Decision { mismatch, moves })
    }

    /// Keep the declared weapon: the regime's evidence shots of the
    /// mismatch's weapon are repriced back to the declared weapon, and that
    /// weapon's evidence stops overriding it until the next regime.
    pub(super) fn keep(&mut self) -> Option<Decision> {
        let mismatch = self.mismatch.take()?;
        let declared = mismatch.declared.clone();
        let mut moves = Vec::new();
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
            self.counts.evidenced -= 1;
            self.counts.agreed += 1;
            moves.push(Reprice {
                seq: record.seq,
                to_tool: declared.clone(),
                record: before,
            });
        }
        self.kept.insert(mismatch.evidence.clone());
        Some(Decision { mismatch, moves })
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
            location: ShotLocation::Pending,
            evidence_id: None,
            booked: resolved.attribution.priced_tool().map(str::to_string),
            cost: Ped::ZERO,
        });
        resolved.attribution
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
        runtime.open_effect_window(OffensiveEffectWindow {
            id: "w1".to_string(),
            started_at: 1.0,
            expires_at: 11.0,
            tick: DamageBand {
                min: 28.0,
                max: 32.0,
            },
        });
        assert_eq!(
            observe(&mut runtime, hit(30.0), 5.0),
            Attribution::EffectTick {
                window_id: "w1".to_string()
            }
        );
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
        }

        fn step() -> impl Strategy<Value = Step> {
            prop_oneof![
                6 => (0.0f64..130.0, any::<bool>()).prop_map(|(amount, critical)| Step::Hit(amount, critical)),
                2 => Just(Step::Countered),
                1 => (0usize..3).prop_map(Step::Declare),
                1 => Just(Step::Resync),
                1 => Just(Step::Confirm),
                1 => Just(Step::Keep),
            ]
        }

        const NAMES: [&str; 3] = ["Pistol", "Cannon", "Rifle"];

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
