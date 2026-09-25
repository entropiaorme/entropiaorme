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
mod combat;
mod harvest;
mod healing;
mod intervals;
mod loot;
mod mob;
mod persistence;
mod providers;
mod session;
#[cfg(test)]
mod tests;
mod time;
mod weapons;

pub use intervals::{
    ActiveActivity, ActivityKey, ActivityRef, CloseScope, IntervalKind, IntervalSpec, OpenInterval,
};
pub use mob::{DeclaredMob, MobStampSource};
pub use providers::{
    DefaultTrackingConfig, EquipmentLibrary, EquipmentProfile, GuardrailTool,
    HarvestGuardrailTools, InertEquipment, Providers, TrackingConfig, TreeSize,
};

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

/// The equipped heal tool. Hotbar-equipment state, NOT session state:
/// a heal tool equipped during one session stays equipped into the
/// next (the original never reset these fields at start or stop; only
/// a heal-tool change or a trifecta reload moves them).
pub(super) struct HealTool {
    pub(super) name: Option<String>,
    pub(super) cost_per_use: Ped,
    pub(super) reload_seconds: f64,
    pub(super) amount_min: Option<f64>,
    pub(super) amount_max: Option<f64>,
}

impl Default for HealTool {
    fn default() -> Self {
        Self {
            name: None,
            cost_per_use: Ped::ZERO,
            reload_seconds: 2.5,
            amount_min: None,
            amount_max: None,
        }
    }
}

/// The equipped harvesting tool. Hotbar-equipment state like
/// `HealTool`, NOT session state: a tool equipped during one session
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

    pub fn is_tracking(&self) -> bool {
        self.status.borrow().tracking
    }

    /// Start a new tracking session; any prior session stops first.
    pub async fn start_session(&self) -> Result<TrackingSession, DbError> {
        self.call(TrackerMsg::Start).await
    }

    /// Stop the active session (None when idle).
    pub async fn stop_session(&self) -> Result<Option<TrackingSession>, DbError> {
        self.call(TrackerMsg::Stop).await
    }

    /// Refresh trifecta-attribution and loot-filter state after config
    /// changes.
    pub async fn reload_config(&self) {
        self.call(TrackerMsg::ReloadConfig).await
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

    /// Clear the declared mob, returning the released name.
    pub async fn release_declared_mob(&self) -> Option<String> {
        self.call(TrackerMsg::ReleaseMob).await
    }

    /// Prime the tracker with a fully-formed demo session (guide-mode
    /// demo playback over a throwaway database only).
    pub async fn prime_demo(
        &self,
        session: TrackingSession,
        declared_mob: Option<DeclaredMob>,
        facets: SessionFacets,
    ) {
        self.call(|reply| TrackerMsg::PrimeDemo {
            session,
            declared_mob,
            facets,
            reply,
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
