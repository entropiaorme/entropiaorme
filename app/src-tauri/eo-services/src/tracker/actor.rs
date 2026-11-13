//! The tracker actor: one task owns all tracker state, fed by a typed
//! message channel. There is no lock; exclusive access IS the task.
//!
//! Messaging uses call semantics (every message carries a completion
//! reply, and senders wait for it) rather than fire-and-forget. That
//! is a deliberate translation of the original's serialisation
//! contract: a producer returning from `publish` has always meant "the
//! tracker has fully absorbed this event, including its persistence",
//! and the frozen replay fingerprints pin the resulting event
//! interleaving byte-for-byte. Call semantics keep both properties
//! (and today's backpressure) while still deleting the mutex, the
//! lock-order doctrine, and the sync->async `block_on` bridges: inside
//! the actor, the database is simply awaited. Decoupling producers
//! from persistence latency (a free-running mailbox) is a measurable
//! follow-up, not a default.

use std::collections::BTreeSet;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, watch};

use crate::bus_events::BusEvent;
use crate::chatlog_time::ChatLogClock;
use crate::clock::Clock;
use crate::db::{Db, DbError};
use crate::event_bus::{EventBus, Registration, Topic};
use crate::loot_filter::normalize_blacklist;
use crate::tracking_models::TrackingSession;

use crate::consumables::{DoseRecord, DoseRemoval, DoseSource};

use super::doses::{DoseError, DoseRuntime, DoseStart};
use super::intervals::{ActiveActivity, ActivityKey, ActivityRef};
use super::providers::Providers;
use super::session::SessionAggregate;
use super::weapon_evidence::{MismatchDecision, WeaponDecisionError};
use super::{HarvestTool, SessionState, TrackerCommandError};

/// The cheap, always-current readout of the actor's session phase,
/// published on every transition so callers answer `is_tracking`
/// without a message round-trip.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct TrackerStatus {
    pub(super) tracking: bool,
}

/// One message into the actor. Every variant carries its reply; the
/// sender waits (see the module doc for why calls are synchronous).
pub(super) enum TrackerMsg {
    /// A bus event forwarded by a subscription; the reply closes the
    /// rendezvous once the event is fully absorbed.
    Event(BusEvent, oneshot::Sender<()>),
    Start(oneshot::Sender<Result<TrackingSession, DbError>>),
    Stop(oneshot::Sender<Result<Option<TrackingSession>, DbError>>),
    /// The in-memory half of the snapshot: the detected tool plus the
    /// session aggregate (None when idle). The session-scoped database
    /// reads run on the caller's side, off the actor.
    Aggregate(Box<AggregateReply>),
    /// The in-memory half of the snapshot together with where the tool in
    /// hand comes from, read in one turn so the two agree.
    AggregateWithTool(Box<AggregateWithToolReply>),
    ReloadConfig(oneshot::Sender<()>),
    /// The player's decision on a standing weapon mismatch.
    DecideWeaponMismatch(
        MismatchDecision,
        oneshot::Sender<Result<bool, WeaponDecisionError>>,
    ),
    SetSkillBoost(
        Option<i64>,
        oneshot::Sender<Result<(), TrackerCommandError>>,
    ),
    SetDeclaredMob {
        name: String,
        species: String,
        maturity: String,
        reply: oneshot::Sender<Result<(), TrackerCommandError>>,
    },
    /// The user declared an activity: open the stretch of play advancing
    /// a quest, or a player-named segment. Exclusive across both kinds
    /// by default (the one-tap switch); additive co-activates instead.
    /// The reply carries the standing set, in declaration order.
    ActivateActivity {
        activity: ActivityRef,
        additive: bool,
        reply: oneshot::Sender<Result<Vec<ActiveActivity>, TrackerCommandError>>,
    },
    /// One standing activity ended (the user's toggle-off, or a quest's
    /// completion closing its stretch), leaving the others open. The
    /// reply carries the set still in force.
    DeactivateActivity {
        target: ActivityKey,
        reply: oneshot::Sender<Result<Vec<ActiveActivity>, TrackerCommandError>>,
    },
    /// Reconcile a committed quest-reward source into the live aggregate.
    ReconcileLootSource {
        source_id: String,
        reply: oneshot::Sender<Result<bool, DbError>>,
    },
    ReleaseMob(oneshot::Sender<Option<String>>),
    /// Pin the creature the screen reads, provided it is still the one
    /// the player saw.
    PinShown {
        expected: super::mob::DeclaredMob,
        reply: oneshot::Sender<Result<super::mob::DeclaredMob, TrackerCommandError>>,
    },
    /// Pin a creature the player typed.
    PinTyped {
        mob: super::mob::DeclaredMob,
        reply: oneshot::Sender<Result<(), TrackerCommandError>>,
    },
    /// Release the pin; the reply says whether one stood.
    Unpin(oneshot::Sender<Result<bool, TrackerCommandError>>),
    /// A session's armour cost was set directly (outside a protection
    /// recording, which announces itself on the bus): re-read the running
    /// session's share. The reply says whether the live readout changed.
    RefreshArmour(oneshot::Sender<Result<bool, DbError>>),
    /// Start a dose of a consumable by hand.
    StartDose(DoseStart, oneshot::Sender<Result<DoseRecord, DoseError>>),
    /// End an untimed dose's effect now.
    EndDose(String, oneshot::Sender<Result<DoseRecord, DoseError>>),
    /// Remove a dose, or restore a removed one (`restore`).
    CorrectDose {
        id: String,
        restore: bool,
        reply: oneshot::Sender<Result<DoseRecord, DoseError>>,
    },
    /// The wake-up at a dose's expiry: nothing to do beyond the sweep every
    /// message runs first.
    DoseWake,
    /// One look of the creature watcher: no reply, since the watcher's
    /// thread never waits on the tracker (see `detection`).
    MobPulse(crate::mob_watch::MobPulse),
    /// The creature watcher's watch for this session ended by itself: no
    /// reply, as for a look.
    WatchEnded(String),
    /// One change the held-tool watcher saw: no reply, since its thread
    /// never waits on the tracker (see `held_tool`).
    /// Each carries the watch it came from.
    ToolPulse(crate::tool_watch::ToolPulse, u64),
    /// The held-tool watcher's watch for this session ended by itself.
    ToolWatchEnded(String, u64),
    /// Where the tool in hand comes from, and what the screen showed of it.
    ToolSource(oneshot::Sender<super::held_tool::ToolSource>),
    /// A tool the player declared by hand, resolved as a press resolves it.
    DeclareTool {
        item: Box<crate::hotbar_listener::ResolvedHotbarItem>,
        occurred_at: f64,
        reply: oneshot::Sender<Result<super::held_tool::ToolDeclared, TrackerCommandError>>,
    },
    /// Test-only structural inspection: run a closure against the
    /// actor's owned state (the typestate replaced the lockable state
    /// tests used to peek at).
    #[cfg(test)]
    Inspect(Box<dyn FnOnce(&mut TrackerActor) + Send>),
}

type AggregateReply = oneshot::Sender<(
    Option<String>,
    Option<crate::bus_events::HotbarItemKind>,
    Option<SessionAggregate>,
)>;

type AggregateWithToolReply = oneshot::Sender<(
    (
        Option<String>,
        Option<crate::bus_events::HotbarItemKind>,
        Option<SessionAggregate>,
    ),
    super::held_tool::ToolDisplay,
)>;

tokio::task_local! {
    /// Set while the actor dispatches, so a bus subscriber reacting to
    /// a tracker-emitted event by publishing a tracker-subscribed
    /// topic is detected instead of deadlocking the rendezvous. (The
    /// old mutex had the same non-reentrancy; this makes it loud.)
    static IN_TRACKER_ACTOR: ();
}

/// Whether the current context is the tracker actor itself.
pub(super) fn in_tracker_actor() -> bool {
    IN_TRACKER_ACTOR.try_with(|()| ()).is_ok()
}

/// The state-owning task. Field-for-field this is the old mutex-held
/// state plus the collaborators the handlers always reached through
/// `self`; the handler methods across the sibling modules are `impl
/// TrackerActor` and access it directly.
pub(super) struct TrackerActor {
    pub(super) bus: Arc<EventBus>,
    pub(super) db: Db,
    pub(super) clock: Arc<dyn Clock>,
    /// The base chat-log readings resolve against; shared with the
    /// watcher that publishes them, so the offset it derives from the
    /// live tail is the one the payloads resolve through.
    pub(super) chatlog_clock: ChatLogClock,
    pub(super) providers: Providers,
    pub(super) session: SessionState,
    pub(super) loot_blacklist: BTreeSet<String>,
    /// The last harvesting tool seen (hotbar-equipment state). Wood
    /// loot groups and failed swings price against it; routing itself
    /// is by the wood taxonomy, so it works with no hotbar signal.
    pub(super) harvest_tool: Option<HarvestTool>,
    /// The resolved harvest guardrail (config-derived), refreshed at
    /// session start and on config reload; None while disabled.
    pub(super) harvest_guardrail: Option<super::providers::HarvestGuardrailTools>,
    /// The one item currently held in the game. This is display truth only;
    /// each accounting domain retains its own attribution state.
    pub(super) held_item: Option<(String, crate::bus_events::HotbarItemKind)>,
    /// The healer last declared (pressed or picked in the overlay), held for
    /// the process like `held_item`: a keyless session starts with it in hand.
    pub(super) declared_healer: Option<super::healing::HealingIntent>,
    /// The consumable doses still running (tracker state, not session
    /// state: a dose outlives the session it was taken in).
    pub(super) doses: DoseRuntime,
    /// The actor's own sender, cloned into the bus forwarders it
    /// installs at session start and into the dose wake-up.
    pub(super) sender: mpsc::UnboundedSender<TrackerMsg>,
    subscriptions: Vec<(Topic, Registration)>,
    status: watch::Sender<TrackerStatus>,
    /// The detection setting as last applied: turning it on lets the
    /// watcher ask for a capture the desktop refused before.
    pub(super) detection_setting: bool,
    /// The held-tool reading setting as last applied, likewise.
    pub(super) tool_reading_setting: bool,
    /// Counts the held-tool watches started: a message of an earlier one
    /// is dropped.
    pub(super) tool_watch_generation: u64,
}

impl TrackerActor {
    /// Build the actor, run crash-orphan recovery, and serve messages
    /// until every sender is gone. `ready` resolves once recovery has
    /// finished (the constructor awaits it, so a recovery failure
    /// surfaces there exactly as it did in the blocking constructor).
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run(
        bus: Arc<EventBus>,
        db: Db,
        clock: Arc<dyn Clock>,
        chatlog_clock: ChatLogClock,
        providers: Providers,
        sender: mpsc::UnboundedSender<TrackerMsg>,
        mut inbox: mpsc::UnboundedReceiver<TrackerMsg>,
        status: watch::Sender<TrackerStatus>,
        ready: oneshot::Sender<Result<(), DbError>>,
    ) {
        let mut actor = TrackerActor {
            bus,
            db,
            clock,
            chatlog_clock,
            providers,
            session: SessionState::Idle,
            loot_blacklist: BTreeSet::new(),
            harvest_tool: None,
            harvest_guardrail: None,
            held_item: None,
            declared_healer: None,
            doses: DoseRuntime::default(),
            sender,
            subscriptions: Vec::new(),
            status,
            detection_setting: false,
            tool_reading_setting: false,
            tool_watch_generation: 0,
        };
        actor.detection_setting = actor.providers.config.mob_detection_enabled();
        actor.tool_reading_setting = actor.providers.config.tool_reading_enabled();
        actor.refresh_loot_filter();

        let recovered = actor.recover_orphaned_sessions().await;
        let failed = recovered.is_err();
        if !failed {
            // No session is live after recovery, so no dose runs either.
            actor.settle_stray_doses().await;
            actor.restore_doses().await;
        }
        let _ = ready.send(recovered);
        if failed {
            return;
        }

        while let Some(message) = inbox.recv().await {
            IN_TRACKER_ACTOR.scope((), actor.dispatch(message)).await;
        }
        // Every handle and forwarder is gone; drop any remaining
        // registrations (stop already removed them on the normal path).
        actor.unsubscribe_handlers();
    }

    async fn dispatch(&mut self, message: TrackerMsg) {
        // A dose whose expiry passed since the last message ends first, so
        // nothing below is priced or stamped under it.
        self.sweep_doses().await;
        match message {
            TrackerMsg::Event(event, done) => {
                self.on_event(&event).await;
                let _ = done.send(());
            }
            TrackerMsg::Start(reply) => {
                let _ = reply.send(self.start_session().await);
            }
            TrackerMsg::Stop(reply) => {
                let _ = reply.send(self.stop_session().await);
            }
            TrackerMsg::Aggregate(reply) => {
                let _ = reply.send(self.aggregate());
            }
            TrackerMsg::AggregateWithTool(reply) => {
                let _ = reply.send((self.aggregate(), self.tool_display()));
            }
            TrackerMsg::ReloadConfig(reply) => {
                self.reload_config();
                self.sync_detection().await;
                self.sync_tool_reading().await;
                let _ = reply.send(());
            }
            TrackerMsg::DecideWeaponMismatch(decision, reply) => {
                let _ = reply.send(self.decide_weapon_mismatch(decision).await);
            }
            TrackerMsg::SetSkillBoost(percent, reply) => {
                let _ = reply.send(self.set_skill_boost(percent).await);
            }
            TrackerMsg::ActivateActivity {
                activity,
                additive,
                reply,
            } => {
                let _ = reply.send(self.activate_activity(activity, additive).await);
            }
            TrackerMsg::DeactivateActivity { target, reply } => {
                let _ = reply.send(self.deactivate_activity(target).await);
            }
            TrackerMsg::ReconcileLootSource { source_id, reply } => {
                let _ = reply.send(self.reconcile_loot_source(&source_id).await);
            }
            TrackerMsg::SetDeclaredMob {
                name,
                species,
                maturity,
                reply,
            } => {
                let _ = reply.send(self.set_declared_mob(&name, &species, &maturity));
            }
            TrackerMsg::ReleaseMob(reply) => {
                let _ = reply.send(self.release_declared_mob());
            }
            TrackerMsg::PinShown { expected, reply } => {
                let _ = reply.send(self.pin_shown(expected).await);
            }
            TrackerMsg::PinTyped { mob, reply } => {
                let _ = reply.send(self.pin_typed(mob).await);
            }
            TrackerMsg::Unpin(reply) => {
                let _ = reply.send(self.unpin().await);
            }
            TrackerMsg::RefreshArmour(reply) => {
                let _ = reply.send(self.refresh_armour().await);
            }
            TrackerMsg::StartDose(start, reply) => {
                let _ = reply.send(self.start_dose(start, DoseSource::Manual, None).await);
            }
            TrackerMsg::EndDose(id, reply) => {
                let _ = reply.send(self.end_dose(&id).await);
            }
            TrackerMsg::CorrectDose { id, restore, reply } => {
                let result = if restore {
                    self.restore_dose(&id, DoseRemoval::Player).await
                } else {
                    self.remove_dose(&id, DoseRemoval::Player).await
                };
                let _ = reply.send(result);
            }
            TrackerMsg::DoseWake => {}
            TrackerMsg::MobPulse(pulse) => self.on_mob_pulse(pulse).await,
            TrackerMsg::WatchEnded(session_id) => self.on_watch_ended(&session_id).await,
            TrackerMsg::ToolPulse(pulse, generation) => {
                self.on_tool_pulse(pulse, generation).await;
            }
            TrackerMsg::ToolWatchEnded(session_id, generation) => {
                self.on_tool_watch_ended(&session_id, generation).await;
            }
            TrackerMsg::ToolSource(reply) => {
                let _ = reply.send(self.tool_source());
            }
            TrackerMsg::DeclareTool {
                item,
                occurred_at,
                reply,
            } => {
                let _ = reply.send(self.declare_by_hand(*item, occurred_at).await);
            }
            #[cfg(test)]
            TrackerMsg::Inspect(probe) => probe(self),
        }
    }

    /// Route one forwarded bus event to its handler. The loot and
    /// global handlers persist, so they are async; the rest mutate
    /// owned memory only.
    async fn on_event(&mut self, event: &BusEvent) {
        match event {
            BusEvent::Combat(_) => self.on_combat(event).await,
            BusEvent::LootGroup(_) => self.on_loot(event).await,
            BusEvent::ActiveToolChanged(_) => self.on_tool_changed(event),
            BusEvent::ActiveHealToolChanged(_) => self.on_heal_tool_changed(event),
            BusEvent::ActiveHarvestToolChanged(_) => self.on_harvest_tool_changed(event),
            BusEvent::HotbarIntent(_) => self.on_intent(event).await,
            BusEvent::HarvestFail(_) => self.on_harvest_fail(event).await,
            BusEvent::Global(_) => self.on_global(event).await,
            BusEvent::EnhancerBreak(_) => self.on_enhancer_break(event),
            BusEvent::TickFlushed(_) => self.on_tick_flushed(event),
            BusEvent::HealingUpdated(_) => self.on_healing_updated().await,
            BusEvent::ProtectionUpdated(_) => self.on_protection_updated().await,
            _ => {}
        }
    }

    /// Publish the current session phase for the handle's cheap reads.
    pub(super) fn publish_status(&self) {
        let status = TrackerStatus {
            tracking: self.session.active().is_some(),
        };
        let _ = self.status.send(status);
    }

    /// Install the bus forwarders for the session's lifetime. Each
    /// forwarder performs the rendezvous documented on `TrackerMsg`:
    /// enqueue, then wait for the actor's completion reply, yielding
    /// its runtime slot when the publisher is a runtime worker (the
    /// demo player) and parking directly on a plain producer thread
    /// (the chat-log tail, the hotbar listener).
    pub(super) fn subscribe_handlers(&mut self) {
        if !self.subscriptions.is_empty() {
            return;
        }
        for topic in [
            Topic::Combat,
            Topic::LootGroup,
            Topic::ActiveToolChanged,
            Topic::ActiveHealToolChanged,
            Topic::ActiveHarvestToolChanged,
            Topic::HotbarIntent,
            Topic::HarvestFail,
            Topic::Global,
            Topic::EnhancerBreak,
            Topic::TickFlushed,
            Topic::HealingUpdated,
            Topic::ProtectionUpdated,
        ] {
            let sender = self.sender.clone();
            let registration = self.bus.subscribe(topic, move |event| {
                if in_tracker_actor() {
                    // A subscriber to a tracker-emitted event published
                    // a tracker-subscribed topic back while the actor
                    // is mid-dispatch; completing the rendezvous would
                    // wait on ourselves. No current producer does this;
                    // drop loudly rather than deadlock.
                    tracing::error!(
                        target: "eo::tracker",
                        topic = topic.as_str(),
                        "tracker-subscribed topic published from the tracker's own dispatch; dropped",
                    );
                    return;
                }
                let (done_tx, done_rx) = oneshot::channel();
                if sender.send(TrackerMsg::Event(event.clone(), done_tx)).is_err() {
                    return;
                }
                if tokio::runtime::Handle::try_current().is_ok() {
                    tokio::task::block_in_place(|| {
                        let _ = done_rx.blocking_recv();
                    });
                } else {
                    let _ = done_rx.blocking_recv();
                }
            });
            self.subscriptions.push((topic, registration));
        }
    }

    pub(super) fn unsubscribe_handlers(&mut self) {
        for (topic, registration) in self.subscriptions.drain(..) {
            self.bus.unsubscribe(topic, registration);
        }
    }

    /// Publish the coarse, frontend-facing tracking.session.updated
    /// event: the typed envelope rides the bus directly. `occurred_at`
    /// is stamped from the domain timestamp that triggered the event,
    /// not a fresh clock read, so the event is deterministic under
    /// replay.
    pub(super) fn emit_session_event(
        &self,
        reason: eo_wire::domain_events::TrackingReason,
        status: eo_wire::domain_events::TrackingStatus,
        occurred_ts: f64,
        session_id: Option<&str>,
    ) {
        use eo_wire::domain_events::{
            TrackingSessionUpdated, TrackingSessionUpdatedPayload, TrackingSessionUpdatedTag,
        };
        self.bus
            .publish(&BusEvent::TrackingSessionUpdated(TrackingSessionUpdated {
                topic: TrackingSessionUpdatedTag,
                event_version: 1,
                occurred_at: super::time::to_iso_utc(occurred_ts),
                payload: TrackingSessionUpdatedPayload {
                    session_id: session_id.map(str::to_string),
                    status,
                    reason,
                },
            }));
    }

    pub(super) fn refresh_loot_filter(&mut self) {
        let blacklist = self.providers.config.loot_filter_blacklist();
        self.loot_blacklist = normalize_blacklist(Some(blacklist.iter().map(String::as_str)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_tracker_actor_reads_the_reentrancy_task_local() {
        // Outside the actor's dispatch scope: not the actor.
        assert!(!in_tracker_actor());
        // Inside the scope the reentrancy guard reports true.
        IN_TRACKER_ACTOR
            .scope((), async {
                assert!(in_tracker_actor(), "the dispatch scope is detected");
            })
            .await;
        // The scope does not leak past its future.
        assert!(!in_tracker_actor());
    }
}
