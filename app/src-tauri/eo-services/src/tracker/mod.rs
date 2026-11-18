//! The hunt tracker: the central coordinator that subscribes to the
//! bus, accumulates combat stats, creates kill records on loot events,
//! and persists to the database. The session state is a typestate
//! (`Idle | Active(ActiveSession)`), and the state is owned by a
//! single actor task fed over a typed message channel (see `actor`)
//! rather than shared behind a mutex. `HuntTracker` is the handle:
//! commands are async message calls, and the two hot predicates read
//! a watch channel the actor keeps current.
//!
//! The kills model: shots accumulate with cost; a loot group is a
//! kill (snapshot the accumulator, stamp the configured mob or tag,
//! persist, reset); deaths are invisible; a session ending with
//! unresolved shots carries them as dangling cost.
//!
//! Representation differences from the original, all
//! observation-equivalent: the original's `_last_kill` alias of
//! `session.kills[-1]` is the `last_mut()` of the kills list;
//! phase-keyed tool stats live in an ordered vector rather than an
//! insertion-ordered dict; the original's logging, debug-only
//! performance counters and development-build priming hook are
//! omitted, as is its `enhancer_tt_lookup` provider (stored but never
//! read there).

mod actor;
mod attribution;
#[cfg(test)]
mod attribution_tests;
mod combat;
mod cycled;
mod detection;
#[cfg(test)]
mod detection_tests;
mod doses;
#[cfg(test)]
mod doses_tests;
mod harvest;
mod healing;
#[cfg(test)]
mod healing_tests;
mod held_tool;
#[cfg(test)]
mod held_tool_tests;
mod intervals;
mod loot;
mod mob;
mod persistence;
mod providers;
mod retention;
mod session;
#[cfg(test)]
mod tests;
mod time;
mod weapon_effects;
#[cfg(test)]
mod weapon_effects_tests;
mod weapon_evidence;
mod weapons;

pub use attribution::{
    damage_band_from_props, AttributionKind, CarriedWeapon, DamageBand, HitMargin, BOOSTED_REACH,
    CRITICAL_REACH, DISPLAY_TOLERANCE, REDUCED_REACH,
};
// The recorded session's expected loot reads its armour as the live readout does.
pub(crate) use cycled::read_session_armour;
#[cfg(test)]
pub(crate) use detection::{write_detection, KillDetection, KillReading, StampedMob};
pub use doses::{DoseError, DoseStart};
pub use held_tool::{
    Enhancers, HandSetup, HandView, ManualDeclaration, ScreenTool, SeenTool, ToolDeclared,
    ToolDisplay, ToolSource, Unseen,
};
pub use intervals::{
    ActiveActivity, ActivityKey, ActivityRef, CloseScope, IntervalKind, IntervalSpec, OpenInterval,
};
pub use mob::{
    DeclaredMob, MobDetectionGrade, MobRoute, MobSource, MobSourceView, MobStampSource, Pin,
    PinnedBy,
};
pub use providers::{
    BundledTool, BundledToolResolver, BundledWeapon, CarriedWeaponProfile, DefaultTrackingConfig,
    EquipmentLibrary, EquipmentProfile, GuardrailTool, HarvestGuardrailTools, InertEquipment,
    Providers, TrackingConfig, TreeSize,
};
pub use weapon_evidence::{MismatchDecision, WeaponDecisionError};

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::chatlog_time::ChatLogClock;
use crate::clock::Clock;
use crate::db::{Db, DbError};
use crate::event_bus::EventBus;
use crate::mob_lookup_service::python_whitespace;
use crate::ped::Ped;
use crate::tracking_models::TrackingSession;

use actor::{TrackerActor, TrackerMsg, TrackerStatus};
use session::ActiveSession;
pub use session::SessionFacets;

/// Loot groups with an identical fingerprint within this window are
/// duplicates.
pub const LOOT_DEDUP_WINDOW_SECONDS: f64 = 2.0;

/// Tagging a global/HoF onto the latest kill requires the kill to be
/// at most this many seconds away.
const GLOBAL_CORRELATION_WINDOW_SECONDS: f64 = 5.0;

/// When board evidence sets a guardrail mismatch, immediately preceding
/// evidence-less swings are re-stamped to the evidence tool as long as
/// each chains to the next within this window (the same-tree swing
/// cadence is 2-3 s; a gap past this is a different tree).
const GUARDRAIL_RETRO_WINDOW_SECONDS: f64 = 30.0;

/// Direct board evidence may classify neighbouring boardless swings
/// only inside this same-action window.
const HARVEST_YIELD_WINDOW_SECONDS: f64 = 30.0;

/// The declaration-command preconditions surfaced at the command
/// boundary. One variant, because a declaration needs only a session to
/// act on: the tag-mode variants retired with the exclusive capture
/// model, and the no-open-segment variant with the live rename the
/// unified Activities control replaced (every Activities verb is
/// idempotent over the standing set, so a stale control cannot fail).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TrackerCommandError {
    #[error("No active session")]
    NoActiveSession,
    /// A pin from the screen was asked for while the screen is not read.
    #[error("The screen is not being read")]
    NotReadingScreen,
    /// A pin from the screen was asked for while it reads no creature.
    #[error("No mob is being read")]
    NothingRead,
    /// The creature the screen reads changed after the player asked to
    /// pin it.
    #[error("The mob being read has changed; pin it again")]
    ReadingChanged,
    /// A tool was declared by hand while the screen shows which tool is
    /// in hand: the screen's reading stands.
    #[error("The screen shows the tool in hand")]
    ScreenShowsTool,
}

/// The session typestate: everything session-scoped lives inside the
/// `Active` payload and cannot exist without a session.
#[derive(Default)]
enum SessionState {
    #[default]
    Idle,
    Active(Box<ActiveSession>),
}

impl SessionState {
    fn active(&self) -> Option<&ActiveSession> {
        match self {
            SessionState::Idle => None,
            SessionState::Active(active) => Some(active),
        }
    }

    fn active_mut(&mut self) -> Option<&mut ActiveSession> {
        match self {
            SessionState::Idle => None,
            SessionState::Active(active) => Some(active),
        }
    }
}

/// The equipped harvesting tool. Hotbar-equipment state, NOT session
/// state: a tool equipped during one session
/// stays known into the next. Whether it is currently the *hand* item
/// (versus a weapon equipped after it) is tracked separately on the
/// actor, because loot routing follows the hand item.
pub(super) struct HarvestTool {
    pub(super) name: String,
    pub(super) cost_per_use: Ped,
}

/// The tracker handle: an unbounded sender into the actor plus the
/// watch the actor keeps current. The composition root keeps one
/// `Arc` for the process lifetime as before.
pub struct HuntTracker {
    db: Db,
    sender: mpsc::UnboundedSender<TrackerMsg>,
    status: watch::Receiver<TrackerStatus>,
    doses: crate::consumables::DoseBoard,
    mob_names: Arc<crate::mob_display::MobDisplayNames>,
}

impl HuntTracker {
    /// Build the tracker over an already-migrated pool: spawn the
    /// state-owning actor on the current runtime and wait for its
    /// crash-orphan recovery to finish (a recovery failure surfaces
    /// here, exactly as the blocking constructor's did).
    pub async fn new(
        bus: Arc<EventBus>,
        db: Db,
        clock: Arc<dyn Clock>,
        chatlog_clock: ChatLogClock,
        mut providers: Providers,
    ) -> Result<Arc<Self>, DbError> {
        providers.player_name = providers
            .player_name
            .trim_matches(python_whitespace)
            .to_string();
        let doses = providers.doses.clone();
        // The labels the player chose and the printings read so far: held
        // for naming kills as they are recorded.
        let mob_names = providers.mob_names.clone();
        mob_names.replace(
            db.with_reader(|conn| crate::mob_display::labels(conn))
                .await?,
        );
        let (sender, inbox) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = watch::channel(TrackerStatus::default());
        let (ready_tx, ready_rx) = oneshot::channel();
        tokio::spawn(TrackerActor::run(
            bus,
            db.clone(),
            clock,
            chatlog_clock,
            providers,
            sender.clone(),
            inbox,
            status_tx,
            ready_tx,
        ));
        ready_rx.await.expect("tracker actor start")?;
        Ok(Arc::new(Self {
            db,
            sender,
            status: status_rx,
            doses,
            mob_names,
        }))
    }

    /// One message call: enqueue with a reply channel, await the reply.
    /// The actor lives as long as any handle does, so a dead channel is
    /// a bug, not a condition to handle.
    async fn call<R>(&self, build: impl FnOnce(oneshot::Sender<R>) -> TrackerMsg) -> R {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender
            .send(build(reply_tx))
            .expect("tracker actor alive");
        reply_rx.await.expect("tracker actor replies")
    }

    /// One message call that answers None, rather than failing, when the
    /// actor is gone.
    async fn try_call<R>(&self, build: impl FnOnce(oneshot::Sender<R>) -> TrackerMsg) -> Option<R> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender.send(build(reply_tx)).ok()?;
        reply_rx.await.ok()
    }

    pub fn is_tracking(&self) -> bool {
        self.status.borrow().tracking
    }

    /// The running doses, as the tracker last published them: what every
    /// reader of the reload speed in effect outside the tracker reads.
    pub fn dose_board(&self) -> crate::consumables::DoseBoard {
        self.doses.clone()
    }

    /// What is shown for each mob, as the tracker names kills: whoever
    /// changes a mob's label replaces what it holds.
    pub fn mob_names(&self) -> Arc<crate::mob_display::MobDisplayNames> {
        self.mob_names.clone()
    }

    /// Start a new tracking session; any prior session stops first.
    pub async fn start_session(&self) -> Result<TrackingSession, DbError> {
        self.call(TrackerMsg::Start).await
    }

    /// Stop the active session (None when idle).
    pub async fn stop_session(&self) -> Result<Option<TrackingSession>, DbError> {
        self.call(TrackerMsg::Stop).await
    }

    /// Refresh the carried weapons, the harvest guardrail, and the loot
    /// filter after config changes.
    pub async fn reload_config(&self) {
        self.call(TrackerMsg::ReloadConfig).await
    }

    /// Decide the standing weapon mismatch: confirm the weapon the damage
    /// evidence names, or keep the hotbar's. False when no mismatch stands
    /// (a stale control), which changes nothing.
    pub async fn decide_weapon_mismatch(
        &self,
        decision: MismatchDecision,
    ) -> Result<bool, WeaponDecisionError> {
        self.call(|reply| TrackerMsg::DecideWeaponMismatch(decision, reply))
            .await
    }

    /// Declare the skill boost now in force; the session row keeps
    /// the latest declaration.
    pub async fn set_skill_boost(&self, percent: Option<i64>) -> Result<(), TrackerCommandError> {
        self.call(|reply| TrackerMsg::SetSkillBoost(percent, reply))
            .await
    }

    /// Declare an activity on the running session: open the stretch of
    /// play advancing a quest, or a player-named segment. The default is
    /// the one-tap switch, exclusive across both kinds (declaring the
    /// next boss seals whatever was standing); `additive` co-activates
    /// instead, keeping each kind's own standing rule. Forward-looking,
    /// like every interval write; events already recorded keep the
    /// context they were stamped with. Returns the standing set, in
    /// declaration order.
    pub async fn activate_activity(
        &self,
        activity: ActivityRef,
        additive: bool,
    ) -> Result<Vec<ActiveActivity>, TrackerCommandError> {
        self.call(|reply| TrackerMsg::ActivateActivity {
            activity,
            additive,
            reply,
        })
        .await
    }

    /// End one standing activity (the user's toggle-off, or a quest's
    /// completion closing its stretch), leaving the others running.
    /// Idempotent when nothing matching is standing. Returns the set
    /// still in force.
    pub async fn deactivate_activity(
        &self,
        target: ActivityKey,
    ) -> Result<Vec<ActiveActivity>, TrackerCommandError> {
        self.call(|reply| TrackerMsg::DeactivateActivity { target, reply })
            .await
    }

    /// Reload one exactly identified raw clump from committed active item
    /// rows after quest reward confirmation or reversal. False means the
    /// source is absent from the running session or already matches the DB.
    pub async fn reconcile_loot_source(&self, source_id: &str) -> Result<bool, DbError> {
        self.call(|reply| TrackerMsg::ReconcileLootSource {
            source_id: source_id.to_string(),
            reply,
        })
        .await
    }

    /// Immediately set the declared mob for kill stamping.
    pub async fn set_declared_mob(
        &self,
        mob_name: &str,
        species: &str,
        maturity: &str,
    ) -> Result<(), TrackerCommandError> {
        self.call(|reply| TrackerMsg::SetDeclaredMob {
            name: mob_name.to_string(),
            species: species.to_string(),
            maturity: maturity.to_string(),
            reply,
        })
        .await
    }

    /// A session's armour cost was set directly: the running session
    /// re-reads its share so the live Cycled carries it at once (protection
    /// recordings reach the tracker on the bus instead). True when the
    /// readout changed.
    pub async fn refresh_armour(&self) -> Result<bool, DbError> {
        self.call(TrackerMsg::RefreshArmour).await
    }

    /// Clear the typed creature (the declared mob), returning the released
    /// name. A pin stands.
    pub async fn release_declared_mob(&self) -> Option<String> {
        self.call(TrackerMsg::ReleaseMob).await
    }

    /// Pin the creature the screen reads now (else the one the latest
    /// kills were), provided it is still `expected`; answers the creature
    /// pinned. The screen stops being read until the pin is released.
    pub async fn pin_shown(
        &self,
        expected: DeclaredMob,
    ) -> Result<DeclaredMob, TrackerCommandError> {
        self.call(|reply| TrackerMsg::PinShown { expected, reply })
            .await
    }

    /// Pin a creature the player typed. The screen stops being read until
    /// the pin is released.
    pub async fn pin_typed(&self, mob: DeclaredMob) -> Result<(), TrackerCommandError> {
        self.call(|reply| TrackerMsg::PinTyped { mob, reply }).await
    }

    /// Release the pin, reading the screen again when detection is on;
    /// answers whether a pin stood.
    pub async fn unpin(&self) -> Result<bool, TrackerCommandError> {
        self.call(TrackerMsg::Unpin).await
    }

    /// Start a dose of a consumable by hand (the overlay's or dashboard's
    /// start; a bound item's hotbar key starts one in game). A running dose
    /// of the same item ends where this one starts.
    pub async fn start_dose(
        &self,
        start: DoseStart,
    ) -> Result<crate::consumables::DoseRecord, DoseError> {
        self.call(|reply| TrackerMsg::StartDose(start, reply)).await
    }

    /// End an untimed dose's effect now (the player says it ran out).
    pub async fn end_dose(&self, id: &str) -> Result<crate::consumables::DoseRecord, DoseError> {
        self.call(|reply| TrackerMsg::EndDose(id.to_string(), reply))
            .await
    }

    /// Remove a dose (a misclick): its effect and any cost it booked are
    /// taken back, exactly restorable with [`Self::restore_dose`].
    pub async fn remove_dose(&self, id: &str) -> Result<crate::consumables::DoseRecord, DoseError> {
        self.call(|reply| TrackerMsg::CorrectDose {
            id: id.to_string(),
            restore: false,
            reply,
        })
        .await
    }

    /// Give a removed dose back exactly.
    pub async fn restore_dose(
        &self,
        id: &str,
    ) -> Result<crate::consumables::DoseRecord, DoseError> {
        self.call(|reply| TrackerMsg::CorrectDose {
            id: id.to_string(),
            restore: true,
            reply,
        })
        .await
    }

    /// Where the tool in hand comes from now, and, while it is the screen,
    /// what the screen has shown of it.
    pub async fn tool_source(&self) -> ToolSource {
        self.call(TrackerMsg::ToolSource).await
    }

    /// A tool the player declared by hand (the overlay's equipment picker),
    /// resolved as a press resolves it: taken here while the tool in hand
    /// is read off the screen and the screen cannot say, refused while it
    /// shows the tool; otherwise the declaration goes the way a press does
    /// (see [`ToolDeclared`]). A tracker that is gone reads no screen, so
    /// the declaration goes the way a press does.
    pub async fn declare_tool(
        &self,
        item: crate::hotbar_listener::ResolvedHotbarItem,
        occurred_at: f64,
    ) -> Result<ToolDeclared, TrackerCommandError> {
        self.try_call(|reply| TrackerMsg::DeclareTool {
            item: Box::new(item),
            occurred_at,
            reply,
        })
        .await
        .unwrap_or(Ok(ToolDeclared::KeysAreSource))
    }

    /// Run the dose expiry sweep now (what the wake-up at an expiry does).
    #[cfg(test)]
    async fn wake_doses(&self) {
        self.call(|reply| {
            TrackerMsg::Inspect(Box::new(move |_| {
                let _ = reply.send(());
            }))
        })
        .await
    }

    /// The in-memory aggregate half of the snapshot (see
    /// `session::SessionAggregate`).
    async fn aggregate(
        &self,
    ) -> (
        Option<String>,
        Option<crate::bus_events::HotbarItemKind>,
        Option<session::SessionAggregate>,
    ) {
        self.call(|reply| TrackerMsg::Aggregate(Box::new(reply)))
            .await
    }

    /// Test-only structural inspection: run a probe against the
    /// actor's owned state and return its result.
    #[cfg(test)]
    async fn inspect<R, F>(&self, probe: F) -> R
    where
        R: Send + 'static,
        F: FnOnce(&mut TrackerActor) -> R + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender
            .send(TrackerMsg::Inspect(Box::new(move |actor| {
                let _ = reply_tx.send(probe(actor));
            })))
            .expect("tracker actor alive");
        reply_rx.await.expect("tracker actor replies")
    }
}
