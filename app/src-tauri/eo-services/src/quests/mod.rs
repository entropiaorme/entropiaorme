//! Quest service: quest CRUD, cooldown derivation, reward materialisation,
//! mob management, lifecycle actions,
//! (start/complete/cancel with ledger and claim integration), the
//! chat-log mission detection (auto-start, auto-complete, and reward suppression),
//! and per-quest analytics.
//!
//! Payload semantics are an owned contract, pinned by the frozen goldens
//! (ADR-0017): a key that is ABSENT takes the documented default, while a
//! key that is PRESENT binds its value even when null. Truthiness gates
//! (`reward_is_skill`, the mobs list) follow Python falsiness: null,
//! false, zero, and empty strings/arrays/objects all read as false.
//!
//! Row values surface with their stored types (`reward_is_skill` and
//! `is_active` as 0/1 integers, ids as integers); the camelCase wire
//! shaping lives in the facade (`eo-api`), not here.
//!
//! One `QuestService` exists per composition, started with [`start`]
//! (`QuestService::start`): the bus-fed flows (session tracking,
//! mission auto-start) and the watcher's reward-filter calls serialise
//! through a single owning task (see `actor`), while the CRUD, linking,
//! and analytics surfaces are plain `&self` async reads and writes over
//! the shared database handles.

mod actor;
mod analytics;
pub mod catalogue;
mod crud;
#[cfg(test)]
mod curated_tests;
mod families;
mod hand_in;
mod lifecycle;
mod missions;
mod offers;
mod payload;
mod review;
#[cfg(test)]
mod tests;

pub use families::CooldownAnchor;
pub use hand_in::{HandInCandidate, HandInRewardItem, HandInState};
pub use missions::{normalize_quest_name, FUZZY_THRESHOLD};
pub use offers::{read_quest_offers, QuestOffer};

/// The sink a quest completion reports through, so a completed quest's
/// declared stretch (when the user declared one on the running session)
/// closes at the completion moment. Completion is the lifecycle's only
/// interval-layer report: which stretches of play advance a quest is
/// the user's declaration (the Activities control), not something the
/// mission log can witness, so starting a quest opens nothing.
///
/// Injected after construction rather than taken as a constructor
/// argument, because composition builds the quest service before the
/// tracker that owns the interval state. Deliberately NOT a bus topic:
/// the corpus fingerprints capture the published event stream, and the
/// banked port-equivalence captures cannot move.
pub type QuestStretchCloser = Arc<
    dyn Fn(i64) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync,
>;

/// The live tracker's in-memory half of an exact reward reclassification.
/// The quest transaction owns persistence; this sink keeps the running
/// overlay aggregate in step immediately after that commit.
pub type QuestLootReconciler = Arc<
    dyn Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync,
>;

use std::sync::{Arc, OnceLock};

use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};

use crate::chatlog_watcher::{QuestRewardFilter, SignalRewardFilter};
use crate::clock::Clock;
use crate::db::Db;
use crate::event_bus::EventBus;

use actor::QuestMsg;

/// The identifier source for ledger rows and session-less completion
/// keys (random by default; injected by the tests so the committed
/// goldens stamp the same identifiers).
pub type IdSource = Arc<dyn Fn() -> String + Send + Sync>;

/// The service's error surface: `Invalid` carries the rejection
/// messages verbatim as the frozen goldens pin them (including the
/// null-list refusal texts the original implementation left unworded).
/// By owned contract, the typed facade maps every `QuestError` variant
/// (`Invalid` and `Rollup` alike) to its internal-error reply; this family
/// deliberately has no bad-request arm (see `eo-api`'s quests module).
#[derive(Debug, thiserror::Error)]
pub enum QuestError {
    #[error("{0}")]
    Invalid(String),
    /// A daily-rollup refresh failure inside a quest write.
    #[error(transparent)]
    Rollup(#[from] crate::db::DbError),
}

/// Quest operations: CRUD, the completion lifecycle,
/// chat-log mission detection, and the analytics readers.
pub struct QuestService {
    db: Db,
    clock: Arc<dyn Clock>,
    id_source: IdSource,
    /// The active tracking session, kept current by the owning task;
    /// reads are lock-free snapshots.
    session: watch::Receiver<Option<String>>,
    /// The sender into the owning task, for the watcher's
    /// reward-filter rendezvous.
    pump: mpsc::UnboundedSender<QuestMsg>,
    /// Where quest completions are reported to the interval layer, once
    /// composition has wired it. Absent in every test and composition
    /// that has no tracker, which is why every report is best-effort.
    stretch_closer: OnceLock<QuestStretchCloser>,
    loot_reconciler: OnceLock<QuestLootReconciler>,
}

impl QuestService {
    /// Start the quest service: subscribe the permanent bus forwarders
    /// (session start/stop track the active session, and a received
    /// mission auto-starts its matching quest, exactly the original's
    /// constructor-time subscriptions) and spawn the owning task on
    /// `runtime`.
    pub fn start(bus: &Arc<EventBus>, db: Db, clock: Arc<dyn Clock>, runtime: Handle) -> Arc<Self> {
        Self::start_with_id_source(
            bus,
            db,
            clock,
            runtime,
            Arc::new(|| uuid::Uuid::new_v4().to_string()),
        )
    }

    /// [`start`](Self::start) with an explicit identifier source (the
    /// tests pin deterministic ledger and completion keys).
    pub fn start_with_id_source(
        bus: &Arc<EventBus>,
        db: Db,
        clock: Arc<dyn Clock>,
        runtime: Handle,
        id_source: IdSource,
    ) -> Arc<Self> {
        let (pump, inbox) = mpsc::unbounded_channel();
        let (session_tx, session_rx) = watch::channel(None);
        let service = Arc::new(Self {
            db,
            clock,
            id_source,
            session: session_rx,
            pump: pump.clone(),
            stretch_closer: OnceLock::new(),
            loot_reconciler: OnceLock::new(),
        });
        let subscriptions = actor::subscribe_handlers(bus, &pump);
        runtime.spawn(actor::run(
            service.clone(),
            inbox,
            session_tx,
            subscriptions,
        ));
        service
    }

    pub(super) fn next_id(&self) -> String {
        (self.id_source)()
    }

    /// The service's one local-to-instant boundary: the injected
    /// clock's wall-clock reading resolved to the instant it names
    /// (the fold=0 rule), as the epoch-seconds float the database
    /// stores. Every timestamp the service stamps or compares passes
    /// through here; renders back to wire form go through
    /// [`crate::time::to_iso_utc`].
    pub(super) fn now_epoch(&self) -> f64 {
        crate::time::instant_to_epoch(crate::time::resolve_local(self.clock.now()))
    }

    /// A snapshot of the active tracking session id (the owning task
    /// keeps the watch current; an empty string passes through, and
    /// the truthiness gates downstream treat it as no session).
    pub(super) fn current_session(&self) -> Option<String> {
        self.session.borrow().clone()
    }

    /// Wire the interval-layer sink. Composition calls this once, after
    /// the tracker exists; a second call is ignored.
    pub fn set_stretch_closer(&self, closer: QuestStretchCloser) {
        let _ = self.stretch_closer.set(closer);
    }

    /// Wire the live aggregate correction sink. Composition calls this once
    /// after the tracker exists; a second call is ignored.
    pub fn set_loot_reconciler(&self, reconciler: QuestLootReconciler) {
        let _ = self.loot_reconciler.set(reconciler);
    }

    /// Report a quest's completion to the interval layer, if anything
    /// is listening, so a declared stretch of it closes.
    ///
    /// Best-effort by design, and never on the caller's error path: an
    /// interval write that cannot land must not fail the quest action
    /// that prompted it. The quest's own state is the durable record;
    /// the declared stretch is the session's view of it.
    pub(super) async fn report_stretch_closed(&self, quest_id: i64) {
        if let Some(closer) = self.stretch_closer.get() {
            closer(quest_id).await;
        }
    }

    pub(super) async fn report_loot_reconciled(&self, source_id: String) {
        if let Some(reconciler) = self.loot_reconciler.get() {
            reconciler(source_id).await;
        }
    }

    /// The chat-log watcher's reward-filter seam: a synchronous closure
    /// the watcher invokes from its tail thread on a MISSION_COMPLETE
    /// tick. Each call is a rendezvous into the owning task (enqueue,
    /// wait for the reply), so filter decisions serialise with the
    /// session events the task owns; the tick does not publish until
    /// the filter has answered, preserving the original's synchronous
    /// suppression contract. A filter error surfaces as no suppression,
    /// exactly as the original contains a filter exception.
    pub fn watcher_filter(&self) -> QuestRewardFilter {
        let pump = self.pump.clone();
        Arc::new(
            move |mission_name, loot_items, skill_gains, isolated_completion_tick| {
                let (reply_tx, reply_rx) = oneshot::channel();
                let message = QuestMsg::RewardFilter {
                    mission_name: mission_name.to_string(),
                    loot_items: loot_items.to_vec(),
                    skill_gains: skill_gains.to_vec(),
                    isolated_completion_tick,
                    reply: reply_tx,
                };
                if pump.send(message).is_err() {
                    return None;
                }
                let result = if Handle::try_current().is_ok() {
                    tokio::task::block_in_place(|| reply_rx.blocking_recv())
                } else {
                    reply_rx.blocking_recv()
                };
                result.unwrap_or(None)
            },
        )
    }

    pub fn watcher_signal_filter(&self) -> SignalRewardFilter {
        let pump = self.pump.clone();
        Arc::new(move |loot_items| {
            let (reply_tx, reply_rx) = oneshot::channel();
            if pump
                .send(QuestMsg::SignalRewardFilter {
                    loot_items: loot_items.to_vec(),
                    reply: reply_tx,
                })
                .is_err()
            {
                return None;
            }
            let result = if Handle::try_current().is_ok() {
                tokio::task::block_in_place(|| reply_rx.blocking_recv())
            } else {
                reply_rx.blocking_recv()
            };
            result.unwrap_or(None)
        })
    }
}
