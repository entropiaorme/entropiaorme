//! The tool in hand read off the game screen: the held-tool watcher's
//! half in the tracker, and what the tracker does with what it reads
//! while the reading is on.
//!
//! With the reading on, the screen is the source of the tool in hand. A
//! weapon, healing tool or harvesting tool the bar names is declared at
//! the moment the frame that showed it was captured, through the same
//! handling a hotbar press of the item gets; a library entry is resolved
//! as a press resolves it. A hotbar key then only says what was pressed:
//! a consumable's key still takes its dose, the key of the healer the
//! screen shows in hand is a use of it (as a press always is), and any
//! other key asks the watcher to look at once, declaring nothing. The
//! overlay's declaration is the player's word where the screen cannot say
//! (the bar unclear or out of view, or not read yet), and stands until the
//! screen next shows something conclusive; while the screen shows the
//! tool in hand, a declaration is refused.
//!
//! What the bar shows decides the tool in hand from the moment it was
//! seen:
//!
//! - a tool: that tool is declared;
//! - nothing held, or a name the player said is not a tracked tool: the
//!   tool was put away, so no tool is declared. A shot only the tool put
//!   away explains, or one seen before it was put away, is still its for
//!   the switch tail; any other shot is classified as with none declared;
//! - unclear: no tool is declared and there is no tail, so a shot fired
//!   meanwhile is never credited to the last tool by timing (damage
//!   evidence still names a carried weapon a hit fits alone);
//! - out of view: not a change of tool. The tool stays declared and its
//!   shots are booked to it, but it is unseen from that moment. When the
//!   bar shows the same tool again, those shots stand; when it shows
//!   anything else, or the session ends first, every shot booked to the
//!   tool since it went unseen is left unpriced.
//!
//! A watch that ends by itself (capture refused or stopped) hands the
//! source back to the keys for the rest of the session, the tool in hand
//! still declared, after a tool unseen then is settled as above; turning
//! the setting off and on, or a new session, tries the screen again.
//!
//! A tool the library does not hold, known only from the bundled game
//! data, is declared at its bare cost and band, kept apart from the
//! library's weapons so it never passes for one, and its sighting is
//! kept for the review after the session, as is a name read that matches
//! no tool.
//!
//! Looks reach the tracker on its own channel, chat-log shots and hotbar
//! presses over the bus, and their order of arrival is not their order in
//! time. Every look is placed by when its frame was captured: the shots
//! already booked at or after that moment are taken back and placed again
//! in order, exactly as they would have been had the look arrived first
//! (their weapon, price, classification, stored row, tallies and the
//! regime's own memory of them). A look older than the shots still kept is
//! taken as of the oldest one kept, and a look older than one already
//! applied is dropped. A shot arriving after a change it predates is the
//! switch tail's to place, as after a press.

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::bus_events::{
    ActiveHarvestToolChangedPayload, ActiveHealToolChangedPayload, BusEvent, HotbarIntentPayload,
    HotbarItemKind, IntentSource,
};
use crate::hotbar_listener::ResolvedHotbarItem;
use crate::ped::Ped;
use crate::screen_watch::WatchEnded;
use crate::tool_watch::{HeldTool, ToolPulse, ToolPulseSink, ToolSeen, Why};

use super::actor::{TrackerActor, TrackerMsg};
use super::attribution::{Attribution, Observation, RetainedShot, ShotLocation, ShotRecord};
use super::healing::close_healing_intent;
use super::providers::Providers;
use super::session::ActiveSession;
use super::time::{instant_to_epoch, resolve_local};
use super::weapon_evidence::{unbook, write_moved, Backup, Moved, ShotEvidence, ShotMove};
use super::TrackerCommandError;

/// How long after a shot a look that predates it may still arrive and
/// place it. A change of tool is told at once and an unclear or unseen bar
/// after the watcher's settle time (half a second in the interface
/// profile), stamped with when it was first seen; this bounds both with
/// room for a busy tracker.
pub(super) const LATE_LOOK_SECONDS: f64 = 5.0;

/// The most names matching no tool kept per session. Lettering read
/// under a window or mid-change can be junk the matcher cannot place, and
/// each distinct junk text would otherwise be a row of its own.
pub(super) const UNKNOWN_NAMES_PER_SESSION: usize = 16;

/// Why a shot booked to the tool in hand was left unpriced: the bar was
/// out of view, and did not show the tool again.
const UNSEEN_REASON: &str = "the held tool was out of view and was not seen again in hand";

/// Where the tool in hand comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolSource {
    /// The hotbar's keys and the overlay's declaration.
    Keys,
    /// The screen, read for the running session.
    Screen(ScreenTool),
    /// The keys again, because the screen stopped being read during the
    /// session (capture was refused or stopped), since this moment (epoch
    /// milliseconds).
    ScreenStopped { at_ms: i64 },
}

/// What the screen has shown of the tool in hand this session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScreenTool {
    /// The last thing the bar showed, and when it was first seen; None
    /// until its first look.
    pub last: Option<SeenTool>,
    /// Set while the bar is out of view: the tool in hand is unseen.
    pub unseen: Option<Unseen>,
    /// The player's declaration standing where the screen could not say.
    pub manual: Option<ManualDeclaration>,
}

/// One thing the bar showed, and when its frame was captured (epoch
/// milliseconds).
#[derive(Debug, Clone, PartialEq)]
pub struct SeenTool {
    pub seen: ToolSeen,
    pub at_ms: i64,
}

/// The tool in hand unseen since a moment (epoch milliseconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unseen {
    pub since_ms: i64,
}

/// A library entry the player declared in hand, and when (epoch
/// milliseconds).
#[derive(Debug, Clone, PartialEq)]
pub struct ManualDeclaration {
    pub equipment_id: i64,
    pub name: String,
    pub at_ms: i64,
}

/// What became of a tool the player declared by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDeclared {
    /// The screen is the source and could not say: the declaration stands.
    Taken,
    /// The keys are the source: the declaration goes the way a press does.
    KeysAreSource,
}

/// The tool in hand as the screen (or the player, where it could not
/// say) declared it.
#[derive(Debug, Clone, PartialEq)]
enum InHand {
    Library {
        id: i64,
        name: String,
        kind: HotbarItemKind,
    },
    /// Known only from the game's data, by the name its shots are booked
    /// under.
    Provisional {
        endpoint: String,
        item_id: String,
        name: String,
    },
}

impl InHand {
    fn is(&self, tool: &HeldTool) -> bool {
        match (self, tool) {
            (InHand::Library { id, .. }, HeldTool::Library { id: seen }) => id == seen,
            (
                InHand::Provisional {
                    endpoint, item_id, ..
                },
                HeldTool::Bundled {
                    endpoint: seen_endpoint,
                    item_id: seen_item,
                    ..
                },
            ) => endpoint == seen_endpoint && item_id == seen_item,
            _ => false,
        }
    }

    fn name(&self) -> &str {
        match self {
            InHand::Library { name, .. } | InHand::Provisional { name, .. } => name,
        }
    }
}

/// The tool in hand going unseen, and what was in hand then.
#[derive(Debug, Clone)]
pub(super) struct Hold {
    /// Epoch seconds.
    since: f64,
    since_ms: i64,
    hand: Option<InHand>,
}

/// What a sighting is of.
#[derive(Debug, Clone, PartialEq)]
enum Sighted {
    /// A tool of the bundled game data, by the name its shots are booked
    /// under.
    Bundled {
        endpoint: String,
        item_id: String,
        name: String,
    },
    /// A name read matching no tool: its key and the text as read.
    Unknown { key: String, text: String },
}

impl Sighted {
    fn same(&self, other: &Sighted) -> bool {
        match (self, other) {
            (
                Sighted::Bundled {
                    endpoint, item_id, ..
                },
                Sighted::Bundled {
                    endpoint: other_endpoint,
                    item_id: other_item,
                    ..
                },
            ) => endpoint == other_endpoint && item_id == other_item,
            (Sighted::Unknown { key, .. }, Sighted::Unknown { key: other, .. }) => key == other,
            _ => false,
        }
    }
}

/// One thing read that is not a library entry, kept for the review after
/// the session.
#[derive(Debug, Clone)]
struct Sighting {
    sighted: Sighted,
    /// Epoch seconds.
    first: f64,
    last: f64,
    /// A bundled tool's bare cost per shot (PED), as it was declared at.
    cost_per_shot: Option<f64>,
}

/// The running session's reading of the tool in hand.
#[derive(Debug, Default)]
pub(super) struct SessionHeldTool {
    view: ScreenTool,
    hand: Option<InHand>,
    hold: Option<Hold>,
    /// When the latest look applied was seen (epoch seconds): an older
    /// look is dropped.
    applied_at: Option<f64>,
    sightings: Vec<Sighting>,
    /// The sighting the bar shows now.
    showing: Option<usize>,
}

impl SessionHeldTool {
    fn view(&self) -> ScreenTool {
        ScreenTool {
            unseen: self.hold.as_ref().map(|hold| Unseen {
                since_ms: hold.since_ms,
            }),
            ..self.view.clone()
        }
    }

    /// Whether the screen states the tool in hand now: its last look was
    /// conclusive and it is in view.
    fn states_tool(&self) -> bool {
        self.hold.is_none()
            && self
                .view
                .last
                .as_ref()
                .is_some_and(|last| conclusive(&last.seen))
    }
}

/// Whether the bar said outright what is in hand.
fn conclusive(seen: &ToolSeen) -> bool {
    matches!(
        seen,
        ToolSeen::Holding(_) | ToolSeen::NotTracked { .. } | ToolSeen::Nothing
    )
}

/// The key a name read is told apart by: its words, lower-cased.
fn unknown_key(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn epoch_ms(at: f64) -> i64 {
    (at * 1000.0).round() as i64
}

impl TrackerActor {
    /// Where the tool in hand comes from now.
    pub(super) fn tool_source(&self) -> ToolSource {
        let Some(active) = self.session.active() else {
            return ToolSource::Keys;
        };
        match (&active.held_tool, active.screen_stopped) {
            (Some(state), _) => ToolSource::Screen(state.view()),
            (None, Some(at_ms)) => ToolSource::ScreenStopped { at_ms },
            (None, None) => ToolSource::Keys,
        }
    }

    /// Whether the screen is the source of the tool in hand for the
    /// running session.
    fn screen_is_source(&self) -> bool {
        self.session
            .active()
            .is_some_and(|active| active.held_tool.is_some())
    }

    fn screen_states_tool(&self) -> bool {
        self.session
            .active()
            .and_then(|active| active.held_tool.as_ref())
            .is_some_and(SessionHeldTool::states_tool)
    }

    /// A tool the player declared by hand (the overlay's equipment picker),
    /// resolved as a press resolves it. While the screen is the source, the
    /// declaration is taken here, and stands, where the screen cannot say,
    /// and refused where it shows the tool in hand; otherwise (the keys the
    /// source, no session, or a consumable, which is used and never held)
    /// it goes the way a press does.
    pub(super) async fn declare_by_hand(
        &mut self,
        item: ResolvedHotbarItem,
        occurred_at: f64,
    ) -> Result<ToolDeclared, TrackerCommandError> {
        if item.kind == HotbarItemKind::Consumable || !self.screen_is_source() {
            return Ok(ToolDeclared::KeysAreSource);
        }
        if self.screen_states_tool() {
            return Err(TrackerCommandError::ScreenShowsTool);
        }
        let session_id = self
            .session
            .active()
            .map(|active| active.session.id.clone());
        let payload = HotbarIntentPayload {
            session_id,
            slot: String::new(),
            source: IntentSource::Overlay,
            occurred_at,
            equipment_id: item.equipment_id,
            item_name: item.name,
            item_kind: item.kind,
            cost_per_use_ped: item.cost_per_use_ped,
            reload_seconds: item.reload_seconds,
            healing_profile: item.healing_profile,
            lifesteal_percent: item.lifesteal_percent,
            consumable_profile: None,
        };
        self.take_declaration(&payload);
        self.on_hotbar_intent(&BusEvent::HotbarIntent(Box::new(payload)))
            .await;
        Ok(ToolDeclared::Taken)
    }

    /// Bring the reading in line with the setting: start it for the
    /// running session when it turns on (letting the watcher ask again
    /// for a capture the desktop refused), end it when it turns off. From
    /// then on the keys are the source again, with the tool last in hand
    /// still declared, as a pressed one would be. A reading whose watch
    /// ended by itself is tried again only when the setting is turned on
    /// anew.
    pub(super) async fn sync_tool_reading(&mut self) {
        let enabled = self.providers.config.tool_reading_enabled();
        let turned_on = enabled && !self.tool_reading_setting;
        if turned_on {
            self.providers.tool_detector.permit_capture();
        }
        self.tool_reading_setting = enabled;
        let running = self.screen_is_source();
        if !enabled {
            if running {
                let now = instant_to_epoch(resolve_local(self.clock.now()));
                self.end_tool_reading(now, false).await;
                self.nudge_readout();
            }
            if let Some(active) = self.session.active_mut() {
                active.screen_stopped = None;
            }
        } else if !running {
            let stopped = self
                .session
                .active()
                .is_some_and(|active| active.screen_stopped.is_some());
            if turned_on || !stopped {
                self.start_tool_reading().await;
                self.nudge_readout();
            }
        }
    }

    /// Whether a session starting now takes the tool in hand from the
    /// screen: its first look declares it, so neither a tool held before
    /// the start nor a healer declared before it is carried in.
    pub(super) fn session_reads_tool(&self) -> bool {
        self.providers.config.tool_reading_enabled()
    }

    /// Start reading the tool in hand for the running session, with what
    /// it has already seen of tools the library does not hold.
    pub(super) async fn start_tool_reading(&mut self) {
        if !self.providers.config.tool_reading_enabled() {
            return;
        }
        let Some(session_id) = self
            .session
            .active()
            .map(|active| active.session.id.clone())
        else {
            return;
        };
        let read_id = session_id.clone();
        let sightings = self
            .db
            .with_reader(move |conn| read_sightings(conn, &read_id))
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(target: "eo::tracker", %error, "held-tool sightings could not be read");
                Vec::new()
            });
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        self.tool_watch_generation += 1;
        let generation = self.tool_watch_generation;
        let Some(active) = self.session.active_mut() else {
            return;
        };
        active.held_tool = Some(SessionHeldTool {
            sightings,
            ..SessionHeldTool::default()
        });
        active.screen_stopped = None;
        active
            .weapons
            .attribution
            .retain_shots(LATE_LOOK_SECONDS, now);
        let sender = self.sender.clone();
        // Delivered while the tracker lives; once it is gone the watch ends.
        // Each carries the watch it came from, so a look of a watch already
        // replaced is dropped.
        let looks = sender.clone();
        let sink: ToolPulseSink = std::sync::Arc::new(move |pulse| {
            looks.send(TrackerMsg::ToolPulse(pulse, generation)).is_ok()
        });
        let ended: WatchEnded = std::sync::Arc::new(move |session_id: &str| {
            let _ = sender.send(TrackerMsg::ToolWatchEnded(
                session_id.to_string(),
                generation,
            ));
        });
        self.providers.tool_detector.start(&session_id, sink, ended);
    }

    /// End the reading for the rest of the session at `at` (the setting
    /// turned off, or the watch ended by itself): a tool unseen is settled
    /// as the bar showing something else would settle it, the sightings
    /// are kept with their shots so far, and a tool known only from the
    /// game's data leaves the hand, since no key can declare it. Any other
    /// tool stays declared. A settlement that could not be saved keeps the
    /// reading, and its unseen tool, for the stop to settle.
    async fn end_tool_reading(&mut self, at: f64, watch_ended: bool) {
        if !self.settle_hold(at).await {
            return;
        }
        self.close_showing(at);
        self.write_sightings(true).await;
        let provisional = self
            .session
            .active()
            .and_then(|active| active.held_tool.as_ref())
            .and_then(|state| state.hand.as_ref())
            .is_some_and(|hand| matches!(hand, InHand::Provisional { .. }));
        if provisional {
            self.undeclare_hand(at);
        }
        self.drop_tool_reading();
        if watch_ended {
            if let Some(active) = self.session.active_mut() {
                active.screen_stopped = Some(epoch_ms(at));
            }
        }
    }

    /// Stop the watcher and forget the session's reading.
    pub(super) fn drop_tool_reading(&mut self) {
        let Some(active) = self.session.active_mut() else {
            return;
        };
        if active.held_tool.take().is_some() {
            active.weapons.attribution.stop_retaining();
            self.providers.tool_detector.stop();
        }
    }

    /// Settle a tool unseen since the hold began, up to `until`: every shot
    /// booked to it since is left unpriced. Answers whether nothing was
    /// left to settle; a failed write keeps the hold.
    async fn settle_hold(&mut self, until: f64) -> bool {
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        let Some(active) = self.session.active_mut() else {
            return true;
        };
        let Some(hold) = active
            .held_tool
            .as_mut()
            .and_then(|state| state.hold.take())
        else {
            return true;
        };
        // The moves are taken while the hold still keeps its shots.
        let moves = hold
            .hand
            .as_ref()
            .map(|hand| unseen_moves(active, hold.since, Some(until), hand.name()))
            .unwrap_or_default();
        if self.move_booked(moves).await {
            if let Some(active) = self.session.active_mut() {
                active.weapons.attribution.pin_retention(None, now);
            }
            true
        } else {
            self.restore_hold(hold);
            false
        }
    }

    fn restore_hold(&mut self, hold: Hold) {
        if let Some(active) = self.session.active_mut() {
            let since = hold.since;
            if let Some(state) = active.held_tool.as_mut() {
                state.hold = Some(hold);
            }
            active.weapons.attribution.pin_retention(Some(since), since);
        }
    }

    /// Before the stop sums the session: a tool still unseen is settled in
    /// memory as the bar showing something else would settle it, and the
    /// sightings are brought up to date. The settlement's write goes in the
    /// stop's own transaction; [`Self::restore_stop_settlement`] puts it
    /// back if that fails.
    pub(super) async fn prepare_stop_settlement(&mut self) -> Option<StopSettlement> {
        let settlement = {
            let Self {
                session,
                providers,
                held_item,
                ..
            } = &mut *self;
            let active = session.active_mut()?;
            let hold = active.held_tool.as_mut()?.hold.take();
            match hold {
                Some(hold) => {
                    let moves = hold
                        .hand
                        .as_ref()
                        .map(|hand| unseen_moves(active, hold.since, None, hand.name()))
                        .unwrap_or_default();
                    let mut backup = Backup::of(active, held_item);
                    let moved = Self::move_shots(providers, active, &moves, None, &mut backup);
                    Some(StopSettlement {
                        backup,
                        hold,
                        moved,
                    })
                }
                None => None,
            }
        };
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        self.close_showing(now);
        self.write_sightings(true).await;
        settlement
    }

    /// The stop failed: the session stays live, and so do the shots and
    /// the unseen tool the settlement moved.
    pub(super) fn restore_stop_settlement(&mut self, backup: Backup, hold: Hold) {
        let Self {
            session, held_item, ..
        } = &mut *self;
        if let Some(active) = session.active_mut() {
            backup.restore(active, held_item);
        }
        self.restore_hold(hold);
    }

    /// The watch ended by itself: the screen is no longer read, so the keys
    /// are the source again for the rest of the session (see
    /// [`Self::end_tool_reading`]). A watch already started again, or a
    /// message of a watch already replaced, is left alone.
    pub(super) async fn on_tool_watch_ended(&mut self, session_id: &str, generation: u64) {
        let ours = generation == self.tool_watch_generation
            && self.session.active().is_some_and(|active| {
                active.session.id == session_id && active.held_tool.is_some()
            });
        if !ours || self.providers.tool_detector.watches(session_id) {
            return;
        }
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        let at = self
            .session
            .active()
            .and_then(|active| active.held_tool.as_ref())
            .and_then(|state| state.applied_at)
            .map_or(now, |applied| now.max(applied));
        self.end_tool_reading(at, true).await;
        self.nudge_readout();
    }

    /// A hotbar intent off the bus: what it does while the screen is the
    /// source. A consumable's key is its dose, as ever; the key of the
    /// healer the screen shows in hand is a use of it; any other key says
    /// only what was pressed, so it asks the watcher to look and declares
    /// nothing; the overlay's declaration is taken only where the screen
    /// cannot say. Everything else is handled as it always is.
    pub(super) async fn on_intent(&mut self, event: &BusEvent) {
        let BusEvent::HotbarIntent(payload) = event else {
            return;
        };
        if self.screen_is_source() && payload.item_kind != HotbarItemKind::Consumable {
            match payload.source {
                IntentSource::Key => {
                    if !self.is_held_healer(payload) {
                        self.providers.tool_detector.hint();
                        return;
                    }
                }
                IntentSource::Overlay => {
                    if self.screen_states_tool() {
                        tracing::info!(
                            target: "eo::tracker",
                            "a declared tool was refused: the screen shows the tool in hand"
                        );
                        return;
                    }
                    self.take_declaration(payload);
                }
                IntentSource::Screen => {}
            }
        }
        self.on_hotbar_intent(event).await;
    }

    /// Whether a key intent is for the healer the screen shows in hand.
    fn is_held_healer(&self, payload: &HotbarIntentPayload) -> bool {
        payload.item_kind == HotbarItemKind::Healing
            && self
                .session
                .active()
                .and_then(|active| active.held_tool.as_ref())
                .and_then(|state| state.hand.as_ref())
                .is_some_and(|hand| {
                    matches!(
                        hand,
                        InHand::Library { id, kind: HotbarItemKind::Healing, .. }
                            if *id == payload.equipment_id
                    )
                })
    }

    /// The player's declaration while the screen is the source and cannot
    /// say: it stands until the screen next shows something conclusive.
    fn take_declaration(&mut self, payload: &HotbarIntentPayload) {
        let at = payload.occurred_at;
        self.leave_provisional(at);
        if payload.item_kind != HotbarItemKind::Healing {
            self.forget_declared_healer();
        }
        let Some(state) = self.held_tool_mut() else {
            return;
        };
        state.view.manual = Some(ManualDeclaration {
            equipment_id: payload.equipment_id,
            name: payload.item_name.clone(),
            at_ms: epoch_ms(at),
        });
        state.hand = Some(InHand::Library {
            id: payload.equipment_id,
            name: payload.item_name.clone(),
            kind: payload.item_kind,
        });
    }

    /// One look from the watcher, for the running session's reading. A
    /// look for another session, of a watch already replaced, older than
    /// one already applied, or with the reading off, is dropped.
    pub(super) async fn on_tool_pulse(&mut self, pulse: ToolPulse, generation: u64) {
        if generation != self.tool_watch_generation {
            return;
        }
        let Some(active) = self.session.active_mut() else {
            return;
        };
        if active.session.id != pulse.session_id {
            return;
        }
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        let floor = active.weapons.attribution.retention_floor();
        let Some(state) = active.held_tool.as_mut() else {
            return;
        };
        let seen_at = pulse.at_ms as f64 / 1000.0;
        if state.applied_at.is_some_and(|applied| seen_at < applied) {
            return;
        }
        // A look older than every shot still kept is taken as of the oldest
        // moment from which every shot is kept: older shots cannot be
        // placed again and keep their bookings.
        let at = floor.map_or(seen_at, |floor| seen_at.max(floor));
        state.applied_at = Some(at);
        let at_ms = epoch_ms(at);
        state.view.last = Some(SeenTool {
            seen: pulse.seen.clone(),
            at_ms,
        });
        if conclusive(&pulse.seen) {
            state.view.manual = None;
        }

        if pulse.seen == ToolSeen::NotVisible {
            if state.hold.is_none() {
                state.hold = Some(Hold {
                    since: at,
                    since_ms: at_ms,
                    hand: state.hand.clone(),
                });
                active.weapons.attribution.pin_retention(Some(at), now);
            }
            self.nudge_readout();
            return;
        }

        // The bar is in view again: shots booked to a tool unseen since
        // stand only if it is the same tool now.
        let unconfirmed = state.hold.as_ref().is_some_and(|hold| {
            hold.hand.as_ref().is_some_and(
                |hand| !matches!(&pulse.seen, ToolSeen::Holding(tool) if hand.is(tool)),
            )
        });
        if unconfirmed {
            // A failed settlement keeps the hold, for the next look or the
            // stop to settle again.
            self.settle_hold(at).await;
        } else if let Some(active) = self.session.active_mut() {
            if let Some(state) = active.held_tool.as_mut() {
                if state.hold.take().is_some() {
                    active.weapons.attribution.pin_retention(None, now);
                }
            }
        }
        self.close_showing(at);

        let changed = match pulse.seen {
            ToolSeen::Holding(tool) => {
                let held = self
                    .session
                    .active()
                    .and_then(|active| active.held_tool.as_ref())
                    .and_then(|state| state.hand.as_ref())
                    .is_some_and(|hand| hand.is(&tool));
                if held {
                    false
                } else {
                    self.declare_seen(tool, at).await;
                    true
                }
            }
            ToolSeen::Nothing | ToolSeen::NotTracked { .. } => self.undeclare_hand(at),
            ToolSeen::Uncertain(why) => {
                if let Why::Unknown { text } = &why {
                    self.see_unknown(text, at);
                }
                let changed = self.undeclare_hand(at);
                // Unclear is not a putting away: no shot is the last tool's
                // by timing.
                if let Some(active) = self.session.active_mut() {
                    active.weapons.attribution.forget_previous();
                }
                changed
            }
            ToolSeen::NotVisible => unreachable!("handled above"),
        };
        if unconfirmed {
            // The tool in hand before was not seen up to the change, so no
            // later shot is its by timing.
            if let Some(active) = self.session.active_mut() {
                active.weapons.attribution.forget_previous();
            }
        }
        if changed {
            self.place_again_since(at).await;
        }
        self.write_sightings(false).await;
        self.nudge_readout();
    }

    /// Declare the tool the bar names, from `at`.
    async fn declare_seen(&mut self, tool: HeldTool, at: f64) {
        let Some(session_id) = self
            .session
            .active()
            .map(|active| active.session.id.clone())
        else {
            return;
        };
        match tool {
            HeldTool::Library { id } => {
                let item = match self.providers.items.clone() {
                    Some(items) => tokio::task::spawn_blocking(move || {
                        catch_unwind(AssertUnwindSafe(|| items(id))).unwrap_or(None)
                    })
                    .await
                    .unwrap_or(None),
                    None => None,
                };
                let Some(item) = item.filter(|item| item.kind != HotbarItemKind::Consumable) else {
                    tracing::warn!(
                        target: "eo::tracker",
                        "a tool read off the screen could not be resolved; none is declared"
                    );
                    self.undeclare_hand(at);
                    return;
                };
                self.leave_provisional(at);
                if item.kind != HotbarItemKind::Healing {
                    self.forget_declared_healer();
                }
                if let Some(state) = self.held_tool_mut() {
                    state.hand = Some(InHand::Library {
                        id,
                        name: item.name.clone(),
                        kind: item.kind,
                    });
                }
                let payload = HotbarIntentPayload {
                    session_id: Some(session_id),
                    slot: String::new(),
                    source: IntentSource::Screen,
                    occurred_at: at,
                    equipment_id: item.equipment_id,
                    item_name: item.name,
                    item_kind: item.kind,
                    cost_per_use_ped: item.cost_per_use_ped,
                    reload_seconds: item.reload_seconds,
                    healing_profile: item.healing_profile,
                    lifesteal_percent: item.lifesteal_percent,
                    consumable_profile: None,
                };
                self.on_hotbar_intent(&BusEvent::HotbarIntent(Box::new(payload)))
                    .await;
            }
            HeldTool::Bundled {
                endpoint, item_id, ..
            } => {
                let resolved = match self.providers.bundled_tools.clone() {
                    Some(bundled) => {
                        let (endpoint, item_id) = (endpoint.clone(), item_id.clone());
                        tokio::task::spawn_blocking(move || {
                            catch_unwind(AssertUnwindSafe(|| bundled(&endpoint, &item_id)))
                                .unwrap_or(None)
                        })
                        .await
                        .unwrap_or(None)
                    }
                    None => None,
                };
                let Some(tool) = resolved.filter(|tool| tool.kind != HotbarItemKind::Consumable)
                else {
                    tracing::warn!(
                        target: "eo::tracker",
                        "a bundled tool read off the screen could not be resolved; none is declared"
                    );
                    self.undeclare_hand(at);
                    return;
                };
                self.leave_provisional(at);
                self.forget_declared_healer();
                self.see_bundled(&endpoint, &item_id, &tool.name, tool.cost_per_use_ped, at);
                let Some(active) = self.session.active_mut() else {
                    return;
                };
                if let Some(state) = active.held_tool.as_mut() {
                    state.hand = Some(InHand::Provisional {
                        endpoint,
                        item_id,
                        name: tool.name.clone(),
                    });
                }
                close_healing_intent(&mut active.healing, at);
                match tool.kind {
                    HotbarItemKind::Weapon => {
                        active.healing.weapon_lifesteal_percent = None;
                        if let Some(weapon) = tool.weapon {
                            active.weapons.carry_provisional(&tool.name, weapon);
                        }
                        self.on_weapon_press(&tool.name, at);
                    }
                    HotbarItemKind::Harvesting => {
                        self.on_harvest_tool_changed(&BusEvent::ActiveHarvestToolChanged(
                            ActiveHarvestToolChangedPayload {
                                tool_name: tool.name,
                                cost_per_use_ped: tool.cost_per_use_ped,
                                source: Some(IntentSource::Screen.as_str().to_string()),
                            },
                        ));
                    }
                    // A healing tool known only from the game's data is shown
                    // in hand, but no intent opens for it: a paid heal is
                    // recorded against a library entry, so its heals stay
                    // visible as heals no paid use explains.
                    HotbarItemKind::Healing => {
                        self.on_heal_tool_changed(&BusEvent::ActiveHealToolChanged(
                            ActiveHealToolChangedPayload {
                                tool_name: tool.name,
                                cost_per_use_ped: tool.cost_per_use_ped,
                                reload_seconds: 0.0,
                                source: Some(IntentSource::Screen.as_str().to_string()),
                            },
                        ));
                    }
                    HotbarItemKind::Consumable => unreachable!("filtered above"),
                }
            }
        }
    }

    /// The healer declared before is not in hand: a keyless session must
    /// not infer its uses.
    fn forget_declared_healer(&mut self) {
        self.declared_healer = None;
        if let Some(active) = self.session.active_mut() {
            active.healing.declared_healer = None;
        }
    }

    /// No tool is in hand from `at`. Answers whether that changed
    /// anything.
    fn undeclare_hand(&mut self, at: f64) -> bool {
        self.leave_provisional(at);
        self.forget_declared_healer();
        let Self {
            session, held_item, ..
        } = self;
        let Some(active) = session.active_mut() else {
            return false;
        };
        let had = active
            .held_tool
            .as_mut()
            .and_then(|state| state.hand.take())
            .is_some();
        let declared = active.weapons.attribution.declared().is_some();
        if !had && !declared && held_item.is_none() {
            return false;
        }
        active.weapons.attribution.undeclare(at);
        close_healing_intent(&mut active.healing, at);
        active.healing.weapon_lifesteal_percent = None;
        *held_item = None;
        active.dirty = true;
        true
    }

    /// A provisional tool in hand leaves it at `at`: it is tested for the
    /// switch tail after, then not at all.
    fn leave_provisional(&mut self, at: f64) {
        let Some(active) = self.session.active_mut() else {
            return;
        };
        let provisional = active
            .held_tool
            .as_ref()
            .and_then(|state| state.hand.as_ref())
            .is_some_and(|hand| matches!(hand, InHand::Provisional { .. }));
        if provisional {
            active.weapons.attribution.release_provisional(at);
        }
    }

    fn held_tool_mut(&mut self) -> Option<&mut SessionHeldTool> {
        self.session
            .active_mut()
            .and_then(|active| active.held_tool.as_mut())
    }

    /// The thing shown until now was last seen at `at`.
    fn close_showing(&mut self, at: f64) {
        if let Some(state) = self.held_tool_mut() {
            if let Some(index) = state.showing.take() {
                let sighting = &mut state.sightings[index];
                sighting.last = sighting.last.max(at);
            }
        }
    }

    /// A tool of the bundled game data seen in hand at `at`, by the name
    /// its shots are booked under.
    fn see_bundled(&mut self, endpoint: &str, item_id: &str, name: &str, cost: f64, at: f64) {
        let sighted = Sighted::Bundled {
            endpoint: endpoint.to_string(),
            item_id: item_id.to_string(),
            name: name.to_string(),
        };
        self.see(sighted, at, None, Some(cost));
    }

    /// A name matching no tool, settled as read at `at`. Kept once per key,
    /// up to the session's cap.
    fn see_unknown(&mut self, text: &str, at: f64) {
        let key = unknown_key(text);
        if key.is_empty() {
            return;
        }
        let sighted = Sighted::Unknown {
            key,
            text: text.trim().to_string(),
        };
        self.see(sighted, at, Some(UNKNOWN_NAMES_PER_SESSION), None);
    }

    fn see(&mut self, sighted: Sighted, at: f64, cap: Option<usize>, cost: Option<f64>) {
        let Some(state) = self.held_tool_mut() else {
            return;
        };
        let index = match state
            .sightings
            .iter()
            .position(|kept| kept.sighted.same(&sighted))
        {
            Some(index) => {
                let sighting = &mut state.sightings[index];
                sighting.last = sighting.last.max(at);
                sighting.first = sighting.first.min(at);
                if cost.is_some() {
                    sighting.cost_per_shot = cost;
                }
                index
            }
            None => {
                let unknowns = state
                    .sightings
                    .iter()
                    .filter(|kept| matches!(kept.sighted, Sighted::Unknown { .. }))
                    .count();
                if cap.is_some_and(|cap| unknowns >= cap) {
                    return;
                }
                state.sightings.push(Sighting {
                    sighted,
                    first: at,
                    last: at,
                    cost_per_shot: cost,
                });
                state.sightings.len() - 1
            }
        };
        state.showing = Some(index);
    }

    /// Keep the session's sightings, each by what it is of: as it stands,
    /// and, with `with_shots`, with the shots booked to each bundled tool
    /// this session and its bare cost per shot. A failed write is logged;
    /// the sightings are kept again at the next write.
    async fn write_sightings(&mut self, with_shots: bool) {
        let rows = {
            let Some(active) = self.session.active() else {
                return;
            };
            let session_id = active.session.id.clone();
            let Some(state) = active.held_tool.as_ref() else {
                return;
            };
            let sightings = state.sightings.clone();
            let mut rows = Vec::with_capacity(sightings.len());
            for sighting in sightings {
                let booked = match (&sighting.sighted, with_shots) {
                    (Sighted::Bundled { name, .. }, true) => {
                        Some((booked_shots(active, name), sighting.cost_per_shot))
                    }
                    _ => None,
                };
                rows.push((session_id.clone(), sighting, booked));
            }
            rows
        };
        if rows.is_empty() {
            return;
        }
        let written = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                for (session_id, sighting, booked) in &rows {
                    write_sighting(&tx, session_id, sighting, *booked)?;
                }
                tx.commit()?;
                Ok(())
            })
            .await;
        if let Err(error) = written {
            tracing::warn!(target: "eo::tracker", %error, "a held-tool sighting could not be saved");
        }
    }

    /// Move booked shots through the one path that moves them, in memory
    /// against a backup and then in one write; a failed write puts memory
    /// back, so the shots stay as they were booked everywhere. Answers
    /// whether the move was saved (an empty move is).
    async fn move_booked(&mut self, moves: Vec<ShotMove>) -> bool {
        if moves.is_empty() {
            return true;
        }
        let (backup, moved) = {
            let Self {
                session,
                providers,
                held_item,
                ..
            } = &mut *self;
            let Some(active) = session.active_mut() else {
                return true;
            };
            let mut backup = Backup::of(active, held_item);
            let moved = Self::move_shots(providers, active, &moves, None, &mut backup);
            active.dirty = true;
            (backup, moved)
        };
        self.save_moved(backup, moved).await
    }

    /// Write what a move changed in one transaction; a failed write puts
    /// memory back from `backup`.
    async fn save_moved(&mut self, backup: Backup, moved: Moved) -> bool {
        let stored = self
            .db
            .with_writer(move |conn| {
                let tx = conn.transaction()?;
                write_moved(&tx, &moved, None)?;
                tx.commit()?;
                Ok(())
            })
            .await;
        match stored {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(target: "eo::tracker", %error, "shots placed by the held tool could not be saved");
                let Self {
                    session, held_item, ..
                } = &mut *self;
                if let Some(active) = session.active_mut() {
                    backup.restore(active, held_item);
                }
                false
            }
        }
    }

    /// Place again, as the regime that began at `at` places them, every
    /// kept shot at or after `at`: what a look arriving after shots it
    /// predates owes them.
    async fn place_again_since(&mut self, at: f64) {
        let (backup, moved) = {
            let Self {
                session,
                providers,
                held_item,
                ..
            } = &mut *self;
            let Some(active) = session.active_mut() else {
                return;
            };
            let mut backup = Backup::of(active, held_item);
            let Some(moved) = Self::place_shots_again(providers, active, at, &mut backup) else {
                return;
            };
            active.dirty = true;
            (backup, moved)
        };
        self.save_moved(backup, moved).await;
    }

    /// Take every kept shot at or after `at` back out of its phase, its
    /// row, the tallies and the unpriced bound, then record each again in
    /// order, as the shot path records a shot arriving now, into the place
    /// its cost sits (the accumulator, or the kill it settled into). A shot
    /// an effect window touches (one whose hit opened a window, or that a
    /// window could tick) is left as booked: the windows are paid
    /// activations of their own, not placed again. None when no shot was
    /// placed again.
    fn place_shots_again(
        providers: &Providers,
        active: &mut ActiveSession,
        at: f64,
        backup: &mut Backup,
    ) -> Option<Moved> {
        let shots = active.weapons.attribution.take_retained_since(at);
        if shots.is_empty() {
            return None;
        }
        let mut moved = Moved::default();
        let mut taken: Vec<RetainedShot> = Vec::new();
        for shot in shots {
            let touched = shot.record.opened_window.is_some()
                || !shot.record.windows.is_empty()
                || active
                    .weapons
                    .attribution
                    .ticks_at(shot.record.observation, shot.record.observed_at);
            if touched {
                active.weapons.attribution.restore_retained(shot);
                continue;
            }
            let record = &shot.record;
            let amount = super::attribution::AttributionRuntime::amount_of(record);
            let critical = matches!(record.observation, Observation::Hit { critical: true, .. });
            let Some(tool_stats) = Self::phases_of(active, &record.location, backup) else {
                active.weapons.attribution.restore_retained(shot);
                continue;
            };
            if !unbook(
                tool_stats,
                record.booked.as_deref(),
                record.cost,
                amount,
                critical,
            ) {
                tracing::warn!(
                    target: "eo::tracker",
                    "a shot to place again was not found in its phase; left as booked",
                );
                active.weapons.attribution.restore_retained(shot);
                continue;
            }
            if let ShotLocation::Kill(kill_id) = &record.location {
                if let Some(kill) = active
                    .session
                    .kills
                    .iter_mut()
                    .rev()
                    .find(|kill| &kill.id == kill_id)
                {
                    kill.cost_ped = kill.cost_ped - record.cost;
                }
            }
            if let Some(evidence_id) = &record.evidence_id {
                let pending = active
                    .accumulator
                    .evidence
                    .iter()
                    .any(|row| &row.id == evidence_id);
                if pending {
                    active
                        .accumulator
                        .evidence
                        .retain(|row| &row.id != evidence_id);
                } else {
                    moved.row_deletes.push(evidence_id.clone());
                }
            }
            active.weapons.attribution.untally(&record.attribution);
            if let Some(charge) = shot.charge {
                Self::unnote_unpriced(active, charge);
            }
            moved.cost_delta = moved.cost_delta - record.cost;
            taken.push(shot);
        }
        if taken.is_empty() {
            return None;
        }
        taken.sort_by(|a, b| {
            (a.record.observed_at, a.record.seq)
                .partial_cmp(&(b.record.observed_at, b.record.seq))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for shot in taken {
            let RetainedShot {
                record, context, ..
            } = shot;
            let observation = record.observation;
            let observed_at = record.observed_at;
            let resolved = active
                .weapons
                .attribution
                .classify(observation, observed_at);
            let seq = active.weapons.attribution.apply(&resolved, observed_at);
            let charge = matches!(resolved.attribution, Attribution::Unresolved)
                .then(|| Self::note_unpriced(providers, active, observation, &resolved.fits));
            let (amount, critical) = match observation {
                Observation::Hit { amount, critical } => (amount, critical),
                Observation::Countered => (0.0, false),
            };
            let tool = resolved.attribution.priced_tool().map(str::to_string);
            let looters = active.hunting_looters;
            let cost = match &record.location {
                ShotLocation::Pending => Self::book_into(
                    providers,
                    &mut active.weapons,
                    looters,
                    &mut active.accumulator.tool_stats,
                    tool.as_deref(),
                    amount,
                    critical,
                ),
                ShotLocation::Kill(kill_id) => {
                    let index = active
                        .session
                        .kills
                        .iter()
                        .rposition(|kill| &kill.id == kill_id)
                        .expect("a shot taken back from a kill finds it again");
                    let cost = Self::book_into(
                        providers,
                        &mut active.weapons,
                        looters,
                        &mut active.session.kills[index].tool_stats,
                        tool.as_deref(),
                        amount,
                        critical,
                    );
                    active.session.kills[index].cost_ped += cost;
                    cost
                }
            };
            moved.cost_delta += cost;
            moved.repriced += 1;
            let evidence_id =
                ShotEvidence::keeps_row(&resolved.attribution, &active.weapons.attribution).then(
                    || {
                        let mut row = ShotEvidence::for_shot(
                            active,
                            observation,
                            observed_at,
                            &resolved,
                            tool.clone(),
                            cost,
                        );
                        row.context_id = context.context_id;
                        row.reload_speed_percent = context.reload_speed_percent;
                        let id = row.id.clone();
                        match &record.location {
                            ShotLocation::Pending => active.accumulator.evidence.push(row),
                            ShotLocation::Kill(kill_id) => {
                                moved.row_inserts.push((row, kill_id.clone()));
                            }
                        }
                        id
                    },
                );
            active.weapons.attribution.remember_shot(
                ShotRecord {
                    seq,
                    observed_at,
                    observation,
                    attribution: resolved.attribution,
                    fits: resolved.fits,
                    windows: resolved.windows,
                    location: record.location,
                    evidence_id,
                    booked: tool,
                    cost,
                    opened_window: None,
                },
                context,
                charge,
            );
        }
        // The weapon in hand keeps the enhancer state a press gives it.
        if let Some(declared) = active.weapons.attribution.declared().map(str::to_string) {
            Self::current_cost_for_tool(providers, &mut active.weapons, &declared, Ped::ZERO);
        }
        moved.kills = backup.saved_kills_now(active);
        Some(moved)
    }

    /// The phases a shot's cost sits in: the accumulator's, or those of the
    /// kill it settled into, saved in `backup` before the first change.
    fn phases_of<'a>(
        active: &'a mut ActiveSession,
        location: &ShotLocation,
        backup: &mut Backup,
    ) -> Option<&'a mut Vec<(String, crate::tracking_models::ToolStats)>> {
        match location {
            ShotLocation::Pending => Some(&mut active.accumulator.tool_stats),
            ShotLocation::Kill(kill_id) => {
                let index = active
                    .session
                    .kills
                    .iter()
                    .rposition(|kill| &kill.id == kill_id)?;
                backup.save_kill(index, &active.session.kills[index]);
                Some(&mut active.session.kills[index].tool_stats)
            }
        }
    }

    /// Tell the readouts the tool in hand, or what is known of it, moved.
    fn nudge_readout(&mut self) {
        let Some(session_id) = self.session.active_mut().map(|active| {
            active.dirty = true;
            active.session.id.clone()
        }) else {
            return;
        };
        self.emit_session_event(
            eo_wire::domain_events::TrackingReason::Updated,
            eo_wire::domain_events::TrackingStatus::Active,
            instant_to_epoch(resolve_local(self.clock.now())),
            Some(&session_id),
        );
    }
}

/// A tool unseen at the stop, settled in memory: the write the stop's
/// transaction makes, and what to put back if it fails.
pub(super) struct StopSettlement {
    pub(super) backup: Backup,
    pub(super) hold: Hold,
    pub(super) moved: Moved,
}

/// The shots booked to `tool` since `since` (and before `until`, when
/// given), each to be left unpriced.
fn unseen_moves(
    active: &ActiveSession,
    since: f64,
    until: Option<f64>,
    tool: &str,
) -> Vec<ShotMove> {
    active
        .weapons
        .attribution
        .retained_since(since)
        .into_iter()
        .filter(|shot| until.is_none_or(|until| shot.record.observed_at < until))
        .filter(|shot| shot.record.booked.as_deref() == Some(tool))
        .map(|shot| ShotMove {
            seq: shot.record.seq,
            to_tool: None,
            attribution: Some(Attribution::Unresolved),
            record: shot.record,
            context: Some(shot.context),
            reason: UNSEEN_REASON,
            charge: shot.charge,
        })
        .collect()
}

/// The shots booked to `tool` this session: in its kills and since the
/// last.
fn booked_shots(active: &ActiveSession, tool: &str) -> i64 {
    active
        .session
        .kills
        .iter()
        .flat_map(|kill| kill.tool_stats.iter())
        .chain(active.accumulator.tool_stats.iter())
        .filter(|(_, stats)| stats.tool_name == tool)
        .map(|(_, stats)| stats.shots_fired)
        .sum()
}

/// The sightings a session has already kept, as a reading starting again
/// takes them up.
fn read_sightings(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<Sighting>, crate::db::DbError> {
    let mut stmt = conn.prepare(
        "SELECT kind, endpoint, item_id, item_name, text_key, text_read, first_seen, last_seen, \
         cost_per_shot FROM tool_sightings WHERE session_id = ?1 ORDER BY first_seen, rowid",
    )?;
    let rows = stmt.query_map([session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, f64>(6)?,
            row.get::<_, f64>(7)?,
            row.get::<_, Option<f64>>(8)?,
        ))
    })?;
    let mut sightings = Vec::new();
    for row in rows {
        let (kind, endpoint, item_id, item_name, text_key, text_read, first, last, cost_per_shot) =
            row?;
        let sighted = match (
            kind.as_str(),
            endpoint,
            item_id,
            item_name,
            text_key,
            text_read,
        ) {
            ("bundled", Some(endpoint), Some(item_id), Some(name), _, _) => Sighted::Bundled {
                endpoint,
                item_id,
                name,
            },
            ("unknown", _, _, _, Some(key), Some(text)) => Sighted::Unknown { key, text },
            _ => continue,
        };
        sightings.push(Sighting {
            sighted,
            first,
            last,
            cost_per_shot,
        });
    }
    Ok(sightings)
}

/// Keep one sighting as it stands, one row per session per thing seen;
/// `booked` is a bundled tool's shots and bare cost per shot, when known.
fn write_sighting(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    sighting: &Sighting,
    booked: Option<(i64, Option<f64>)>,
) -> Result<(), crate::db::DbError> {
    let (shots, cost) = match booked {
        Some((shots, cost)) => (Some(shots), cost),
        None => (None, None),
    };
    match &sighting.sighted {
        Sighted::Bundled {
            endpoint,
            item_id,
            name,
        } => {
            tx.execute(
                "INSERT INTO tool_sightings \
                 (id, session_id, kind, endpoint, item_id, item_name, first_seen, last_seen, \
                  shots, cost_per_shot) \
                 VALUES (?1, ?2, 'bundled', ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                 ON CONFLICT (session_id, endpoint, item_id) WHERE kind = 'bundled' \
                 DO UPDATE SET first_seen = MIN(first_seen, excluded.first_seen), \
                  last_seen = MAX(last_seen, excluded.last_seen), \
                  shots = COALESCE(excluded.shots, shots), \
                  cost_per_shot = COALESCE(excluded.cost_per_shot, cost_per_shot)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    session_id,
                    endpoint,
                    item_id,
                    name,
                    sighting.first,
                    sighting.last,
                    shots,
                    cost,
                ],
            )?;
        }
        Sighted::Unknown { key, text } => {
            tx.execute(
                "INSERT INTO tool_sightings \
                 (id, session_id, kind, text_key, text_read, first_seen, last_seen) \
                 VALUES (?1, ?2, 'unknown', ?3, ?4, ?5, ?6) \
                 ON CONFLICT (session_id, text_key) WHERE kind = 'unknown' \
                 DO UPDATE SET first_seen = MIN(first_seen, excluded.first_seen), \
                  last_seen = MAX(last_seen, excluded.last_seen)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    session_id,
                    key,
                    text,
                    sighting.first,
                    sighting.last
                ],
            )?;
        }
    }
    Ok(())
}
