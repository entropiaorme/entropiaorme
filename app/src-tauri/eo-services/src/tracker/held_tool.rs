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
//! switch tail's to place, as after a press. What the player decided on a
//! mismatch stands: a shot a decision moved is never placed again. A heal
//! already billed is not placed again either: a late look at the bar
//! changes which shots its weapon fired, not which heals were paid.
//!
//! The observations kept for this cost memory in proportion to the shots
//! fired while the bar is covered, with no cap (see `retention`), and are
//! settled in steps in proportion to their number. While a covered stretch
//! is owed, the damage-over-time windows opened during it are kept too, so
//! memory also grows with the paid hits of weapons with an effect fired
//! meanwhile.
//!
//! A look taken back because the shots it placed again could not be saved
//! puts back the shots, the tool in hand, the healing state, the declared
//! healer, the harvesting tool and what the screen showed before it. It does
//! not put back the weapon prices worked out meanwhile (they are worked out
//! again from the same stored set-up), a tool sighting already noted (it was
//! seen), a covered stretch already settled earlier in the same look (that
//! settlement was saved), or the readout updates already sent.

use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::bus_events::{
    ActiveHarvestToolChangedPayload, ActiveHealToolChangedPayload, BusEvent, HotbarIntentPayload,
    HotbarItemKind, IntentSource,
};
use crate::hotbar_listener::ResolvedHotbarItem;
use crate::ped::Ped;
use crate::screen_watch::WatchEnded;
use crate::tool_setup::{SetupPart, ToolSetup};
use crate::tool_watch::{HeldTool, ToolPulse, ToolPulseSink, ToolSeen, Why};

use super::actor::{TrackerActor, TrackerMsg};
use super::attribution::{Attribution, Observation, RetainedShot, ShotLocation, ShotRecord};
use super::healing::close_healing_intent;
use super::providers::Providers;
use super::session::ActiveSession;
use super::time::{instant_to_epoch, resolve_local};
use super::weapon_evidence::{unbook, write_moved, Backup, Moved, ShotEvidence, ShotMove};
use super::weapons::WeaponRuntime;
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
    /// milliseconds). Tried again when the setting is turned on anew, or
    /// at the next session.
    ScreenStopped { at_ms: i64 },
    /// The keys, because the screen could not be read at all: the watch did
    /// not start (capture is not running, or nothing to read the bar with
    /// was loaded). Tried again at every settings change and at the next
    /// session.
    ScreenNotStarted,
}

/// Why the keys are the source while the reading is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScreenOff {
    /// The watch ended by itself at this moment (epoch milliseconds).
    Stopped { at_ms: i64 },
    /// The watch did not start.
    NotStarted,
}

/// A tool unseen when the reading ended whose settlement could not be
/// saved then: the stop settles it, in its own transaction.
#[derive(Debug, Clone)]
pub(super) struct Unsettled {
    pub(super) hold: Hold,
    /// When the reading ended: shots after it were booked with the keys
    /// the source, and stand.
    pub(super) until: f64,
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

/// The tool in hand as the reading knows it, for display: its name, its
/// library entry, or that it is known only from the game's data, and the
/// set-up its uses are priced at.
#[derive(Debug, Clone, PartialEq)]
pub struct HandView {
    pub name: String,
    /// The library entry it is; None for a tool known only from the game's
    /// data.
    pub library_id: Option<i64>,
    pub setup: Option<HandSetup>,
}

/// What a use of the tool in hand is priced at now, and the set-up behind
/// the price: for a weapon, its props as the session prices its shots (the
/// enhancers still fitted included); for any other tool, the price it was
/// declared at.
#[derive(Debug, Clone, PartialEq)]
pub struct HandSetup {
    pub kind: HotbarItemKind,
    /// The parts fitted; None where the declaration did not say (a healer
    /// declared by its key while the bar was unclear).
    pub parts: Option<Vec<SetupPart>>,
    /// A weapon's damage enhancers: still fitted, and as set up. None when
    /// it is set up with none.
    pub enhancers: Option<Enhancers>,
    pub cost_per_use: Ped,
    /// The TT of one use, before markup, where it is known.
    pub tt_per_use: Option<Ped>,
}

/// A weapon's damage enhancers: how many are still fitted, of those set
/// up (each wears out with use).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enhancers {
    pub active: i64,
    pub fitted: i64,
}

impl HandView {
    /// Known only from the game's data, costed as the bare tool.
    pub fn provisional(&self) -> bool {
        self.library_id.is_none()
    }
}

/// Where the tool in hand comes from, with the reading's own knowledge of
/// the hand: the tool in hand now, and the one in hand when the bar went
/// out of view (while it is).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDisplay {
    pub source: ToolSource,
    pub hand: Option<HandView>,
    pub unseen_hand: Option<HandView>,
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
        /// The price of one use it was declared at.
        cost_per_use: Ped,
        /// What it was resolved with; None when it was declared from an
        /// intent alone.
        setup: Option<ToolSetup>,
    },
    /// Known only from the game's data, by the name its shots are booked
    /// under, at its bare price.
    Provisional {
        endpoint: String,
        item_id: String,
        name: String,
        kind: HotbarItemKind,
        cost_per_use: Ped,
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

    fn view(&self, weapons: &WeaponRuntime) -> HandView {
        HandView {
            name: self.name().to_string(),
            library_id: match self {
                InHand::Library { id, .. } => Some(*id),
                InHand::Provisional { .. } => None,
            },
            setup: Some(self.setup(weapons)),
        }
    }

    /// What a use of it is priced at now: a weapon as the session prices
    /// its shots, when the session prices it; anything else (or a weapon
    /// the session has no props for) at the price it was declared at.
    fn setup(&self, weapons: &WeaponRuntime) -> HandSetup {
        let (kind, cost_per_use) = match self {
            InHand::Library {
                kind, cost_per_use, ..
            }
            | InHand::Provisional {
                kind, cost_per_use, ..
            } => (*kind, *cost_per_use),
        };
        if kind == HotbarItemKind::Weapon {
            if let Some(priced) = weapons.shot_pricing(self.name()) {
                return HandSetup {
                    kind,
                    parts: Some(priced.parts),
                    enhancers: (priced.enhancers.fitted > 0).then_some(priced.enhancers),
                    cost_per_use: priced.cost_per_shot,
                    tt_per_use: Some(priced.tt_per_shot),
                };
            }
        }
        match self {
            InHand::Library { setup, .. } => HandSetup {
                kind,
                parts: setup.as_ref().map(|setup| setup.parts.clone()),
                enhancers: None,
                cost_per_use,
                tt_per_use: setup
                    .as_ref()
                    .and_then(|setup| setup.tt_per_use_ped)
                    .map(Ped),
            },
            // The bare tool: nothing fitted, no markup over TT.
            InHand::Provisional { .. } => HandSetup {
                kind,
                parts: Some(Vec::new()),
                enhancers: None,
                cost_per_use,
                tt_per_use: None,
            },
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
        match (&active.held_tool, active.screen_off) {
            (Some(state), _) => ToolSource::Screen(state.view()),
            (None, Some(ScreenOff::Stopped { at_ms })) => ToolSource::ScreenStopped { at_ms },
            (None, Some(ScreenOff::NotStarted)) => ToolSource::ScreenNotStarted,
            (None, None) => ToolSource::Keys,
        }
    }

    /// Where the tool in hand comes from now, with the reading's knowledge
    /// of the hand (none while the keys are the source).
    pub(super) fn tool_display(&self) -> ToolDisplay {
        let source = self.tool_source();
        let Some((state, weapons)) = self
            .session
            .active()
            .and_then(|active| Some((active.held_tool.as_ref()?, &active.weapons)))
        else {
            return ToolDisplay {
                source,
                hand: None,
                unseen_hand: None,
            };
        };
        ToolDisplay {
            source,
            hand: state.hand.as_ref().map(|hand| hand.view(weapons)),
            unseen_hand: state
                .hold
                .as_ref()
                .and_then(|hold| hold.hand.as_ref())
                .map(|hand| hand.view(weapons)),
        }
    }

    /// Whether the screen is the source of the tool in hand for the
    /// running session.
    pub(super) fn screen_is_source(&self) -> bool {
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
        self.take_declaration(&payload, Some(item.setup));
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
    /// anew; one whose watch never started is tried at every sync.
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
                active.screen_off = None;
            }
        } else if !running {
            let stopped = self
                .session
                .active()
                .is_some_and(|active| matches!(active.screen_off, Some(ScreenOff::Stopped { .. })));
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
        active.screen_off = None;
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
        // A watch that did not take (capture not running, or nothing to read
        // the bar with) reads nothing: the keys stay the source.
        if !self.providers.tool_detector.watches(&session_id) {
            if let Some(active) = self.session.active_mut() {
                active.held_tool = None;
                if active.unsettled.is_empty() {
                    active.weapons.attribution.stop_retaining();
                }
                active.screen_off = Some(ScreenOff::NotStarted);
            }
        }
    }

    /// End the reading for the rest of the session at `at` (the setting
    /// turned off, or the watch ended by itself): a tool unseen is settled
    /// as the bar showing something else would settle it, the sightings
    /// are kept with their shots so far, and a tool known only from the
    /// game's data leaves the hand, since no key can declare it. Any other
    /// tool stays declared. A settlement that could not be saved keeps the
    /// reading, and its unseen tool, for the stop to settle.
    async fn end_tool_reading(&mut self, at: f64, watch_ended: bool) {
        // Every shot recorded so far, the one at this very moment included,
        // was fired while the tool was unseen.
        let until = at.next_up();
        if let Err(hold) = self.settle_hold(until).await {
            // Not saved: the stop settles it, while the keys are the source.
            if let Some(active) = self.session.active_mut() {
                // Owed: nothing since the earliest stretch owed began is let
                // go until the stop, whatever pin a later reading sets.
                let since = active
                    .unsettled
                    .iter()
                    .map(|owed| owed.hold.since)
                    .fold(hold.since, f64::min);
                active.weapons.attribution.owe_retention(Some(since), since);
                active.unsettled.push(Unsettled { hold, until });
            }
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
                active.screen_off = Some(ScreenOff::Stopped {
                    at_ms: epoch_ms(at),
                });
            }
        }
    }

    /// Stop the watcher and forget the session's reading.
    pub(super) fn drop_tool_reading(&mut self) {
        let Some(active) = self.session.active_mut() else {
            return;
        };
        if active.held_tool.take().is_some() {
            // A settlement still owed keeps its shots until the stop.
            if active.unsettled.is_empty() {
                active.weapons.attribution.stop_retaining();
            }
            self.providers.tool_detector.stop();
        }
    }

    /// Settle a tool unseen since the hold began, up to `until`: every shot
    /// booked to it since is left unpriced. A failed write answers the hold
    /// back, the shots as they were booked.
    async fn settle_hold(&mut self, until: f64) -> Result<(), Hold> {
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        let Some(active) = self.session.active_mut() else {
            return Ok(());
        };
        let Some(hold) = active
            .held_tool
            .as_mut()
            .and_then(|state| state.hold.take())
        else {
            return Ok(());
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
            Ok(())
        } else {
            Err(hold)
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
            let hold = active
                .held_tool
                .as_mut()
                .and_then(|state| state.hold.take());
            let unsettled = std::mem::take(&mut active.unsettled);
            if hold.is_none() && unsettled.is_empty() {
                None
            } else {
                let mut moves = Vec::new();
                for Unsettled { hold, until } in &unsettled {
                    if let Some(hand) = &hold.hand {
                        moves.extend(unseen_moves(active, hold.since, Some(*until), hand.name()));
                    }
                }
                if let Some(hold) = &hold {
                    if let Some(hand) = &hold.hand {
                        moves.extend(unseen_moves(active, hold.since, None, hand.name()));
                    }
                }
                let mut backup = Backup::of(active, held_item);
                let moved = Self::move_shots(providers, active, &moves, None, &mut backup);
                Some(StopSettlement {
                    backup,
                    hold,
                    unsettled,
                    moved,
                })
            }
        };
        let now = instant_to_epoch(resolve_local(self.clock.now()));
        self.close_showing(now);
        self.write_sightings(true).await;
        settlement
    }

    /// The stop failed: the session stays live, and so do the shots and
    /// the unseen tool the settlement moved.
    pub(super) fn restore_stop_settlement(
        &mut self,
        backup: Backup,
        hold: Option<Hold>,
        unsettled: Vec<Unsettled>,
    ) {
        let Self {
            session, held_item, ..
        } = &mut *self;
        if let Some(active) = session.active_mut() {
            backup.restore(active, held_item);
            active.unsettled = unsettled;
        }
        if let Some(hold) = hold {
            self.restore_hold(hold);
        }
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
                    self.take_declaration(payload, None);
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
    fn take_declaration(&mut self, payload: &HotbarIntentPayload, setup: Option<ToolSetup>) {
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
            cost_per_use: Ped(payload.cost_per_use_ped),
            setup,
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
        let before = (state.view.clone(), state.applied_at);
        // A look older than every shot still kept is taken as of the oldest
        // moment from which every shot is kept: older shots cannot be
        // placed again and keep their bookings.
        let at = floor.map_or(seen_at, |floor| seen_at.max(floor));
        let clamped = at > seen_at;
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
            if let Err(hold) = self.settle_hold(at).await {
                self.restore_hold(hold);
            }
        } else if let Some(active) = self.session.active_mut() {
            if let Some(state) = active.held_tool.as_mut() {
                if state.hold.take().is_some() {
                    active.weapons.attribution.pin_retention(None, now);
                }
            }
        }
        self.close_showing(at);

        // Everything the change touches is saved first: if the shots it
        // places again cannot be saved, the look is taken back whole.
        let Some(look) = self.look_backup(before) else {
            return;
        };
        let declared_before = self.declared_weapon();
        match pulse.seen {
            ToolSeen::Holding(tool) => {
                let held = self
                    .session
                    .active()
                    .and_then(|active| active.held_tool.as_ref())
                    .and_then(|state| state.hand.as_ref())
                    .is_some_and(|hand| hand.is(&tool));
                if !held {
                    self.declare_seen(tool, at).await;
                }
            }
            ToolSeen::Nothing | ToolSeen::NotTracked { .. } => {
                self.undeclare_hand(at);
            }
            ToolSeen::Uncertain(why) => {
                if let Why::Unknown { text } = &why {
                    self.see_unknown(text, at);
                }
                self.undeclare_hand(at);
                // Unclear is not a putting away: no shot is the last tool's
                // by timing, a tool known only from the game's data included.
                if let Some(active) = self.session.active_mut() {
                    active.weapons.attribution.forget_previous();
                    active.weapons.attribution.end_provisional(at);
                }
            }
            ToolSeen::NotVisible => unreachable!("handled above"),
        }
        if unconfirmed || clamped {
            // The tool in hand before was not seen up to the change (or the
            // change is taken at a moment the screen did not show): no
            // later shot is its by timing.
            if let Some(active) = self.session.active_mut() {
                active.weapons.attribution.forget_previous();
            }
        }
        // Shots are placed again only when the weapon in hand changed: a
        // healer or a finder taken up leaves every shot where it was.
        if self.declared_weapon() != declared_before {
            self.place_again_since(look, at).await;
        } else if let Some(active) = self.session.active_mut() {
            look.backup.commit(active);
        }
        self.write_sightings(false).await;
        self.nudge_readout();
    }

    /// The weapon declared now, the tool known only from the game's data
    /// included.
    fn declared_weapon(&self) -> Option<String> {
        self.session
            .active()
            .and_then(|active| active.weapons.attribution.declared().map(str::to_string))
    }

    /// Save what a change of the tool in hand touches: the shots and the
    /// attribution state (see [`Backup`]), the hand and what the screen
    /// showed before the look, the healing state, the declared healer, the
    /// harvesting tool and a tool known only from the game's data.
    fn look_backup(&mut self, before: (ScreenTool, Option<f64>)) -> Option<LookBackup> {
        let Self {
            session,
            held_item,
            declared_healer,
            harvest_tool,
            ..
        } = &mut *self;
        let active = session.active_mut()?;
        let hand = active.held_tool.as_ref()?.hand.clone();
        Some(LookBackup {
            backup: Backup::of(active, held_item),
            hand,
            view: before.0,
            applied_at: before.1,
            healing: active.healing.clone(),
            declared_healer: declared_healer.clone(),
            harvest_tool: harvest_tool
                .as_ref()
                .map(|tool| (tool.name.clone(), tool.cost_per_use)),
            provisional: active.weapons.provisional_props(),
        })
    }

    /// Take a look back whole: the shots and state as they stood, and what
    /// the screen showed before it, so the next look (or the stop) decides.
    fn restore_look(&mut self, look: LookBackup) {
        let Self {
            session,
            held_item,
            declared_healer,
            harvest_tool,
            ..
        } = &mut *self;
        let Some(active) = session.active_mut() else {
            return;
        };
        look.backup.restore(active, held_item);
        active.healing = look.healing;
        active.weapons.set_provisional_props(look.provisional);
        if let Some(state) = active.held_tool.as_mut() {
            state.hand = look.hand;
            state.view = look.view;
            state.applied_at = look.applied_at;
        }
        *declared_healer = look.declared_healer;
        *harvest_tool = look
            .harvest_tool
            .map(|(name, cost_per_use)| super::HarvestTool { name, cost_per_use });
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
                        cost_per_use: Ped(item.cost_per_use_ped),
                        setup: Some(item.setup.clone()),
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
                        kind: tool.kind,
                        cost_per_use: Ped(tool.cost_per_use_ped),
                    });
                }
                close_healing_intent(&mut active.healing, at);
                match tool.kind {
                    HotbarItemKind::Weapon => {
                        active.healing.weapon_lifesteal_percent =
                            tool.weapon.as_ref().and_then(|weapon| {
                                crate::equipment_pricing::lifesteal_percent_from_props(
                                    &serde_json::Value::Object(weapon.props.clone()).to_string(),
                                )
                            });
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
        if self.write_placed(moved).await {
            if let Some(active) = self.session.active_mut() {
                backup.commit(active);
            }
            true
        } else {
            let Self {
                session, held_item, ..
            } = &mut *self;
            if let Some(active) = session.active_mut() {
                backup.restore(active, held_item);
            }
            false
        }
    }

    async fn write_placed(&mut self, moved: Moved) -> bool {
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
                false
            }
        }
    }

    /// Place again, as the regime that began at `at` places them, every
    /// kept observation at or after `at`: what a look arriving after shots
    /// it predates owes them. If that cannot be saved, the look is taken
    /// back whole: the overlay keeps showing what the screen showed before
    /// it, the tool in hand and every shot stay as they were, and the next
    /// look the watcher tells (or the stop, for a tool unseen) decides.
    async fn place_again_since(&mut self, mut look: LookBackup, at: f64) {
        let moved = {
            let Self {
                session, providers, ..
            } = &mut *self;
            let Some(active) = session.active_mut() else {
                return;
            };
            let moved = Self::place_shots_again(providers, active, at, &mut look.backup);
            active.dirty = true;
            moved
        };
        let saved = match moved {
            Some(moved) => self.write_placed(moved).await,
            None => true,
        };
        if saved {
            if let Some(active) = self.session.active_mut() {
                look.backup.commit(active);
            }
        } else {
            self.restore_look(look);
        }
    }

    /// Put back what could not be placed again: each as it was, a shot in
    /// the regime log too. A tick of a window taken back loses its link to
    /// the window (in memory and in its row), since the hit that opened it
    /// was placed again.
    fn restore_unplaced(
        active: &mut ActiveSession,
        unplaced: Vec<RetainedShot>,
        moved: &mut Moved,
    ) {
        for mut shot in unplaced {
            let withdrawn = |id: &String| moved.window_deletes.contains(id);
            if let Attribution::EffectTick { window_id } = &mut shot.record.attribution {
                let linked = window_id.as_ref().is_some_and(withdrawn)
                    || shot.record.windows.iter().any(withdrawn);
                if linked {
                    *window_id = None;
                    shot.record
                        .windows
                        .retain(|id| !moved.window_deletes.contains(id));
                    if let Some(evidence_id) = &shot.record.evidence_id {
                        match active
                            .accumulator
                            .evidence
                            .iter_mut()
                            .find(|row| &row.id == evidence_id)
                        {
                            Some(row) => {
                                row.effect_window_id = None;
                                row.effect_candidates.retain(|window| {
                                    !moved.window_deletes.contains(&window.window_id)
                                });
                            }
                            None => moved.tick_unlinks.push(evidence_id.clone()),
                        }
                    }
                }
            }
            active.weapons.attribution.restore_unplaced(shot);
        }
    }

    /// Take every kept observation at or after `at` back out of where it
    /// was recorded (its phase, its counts, its row, the tallies, the
    /// unpriced bound, the effect window its hit opened), then record each
    /// again in order, exactly as the shot path records an observation
    /// arriving now, into the place its cost sits (the accumulator, or the
    /// kill it settled into): an effect tick may become a shot and a shot a
    /// tick, and a paid hit opens its effect again only if it still pays
    /// for one. A shot the player's decision moved keeps what was decided.
    /// None when nothing was placed again.
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
        // What could not be placed again goes back as it was, once every
        // window taken back is known.
        let mut unplaced: Vec<RetainedShot> = Vec::new();
        // The rows waiting with the accumulator, found by id once rather
        // than searched for each shot; those taken out go in one pass.
        let pending_rows: std::collections::HashSet<String> = active
            .accumulator
            .evidence
            .iter()
            .map(|row| row.id.clone())
            .collect();
        #[cfg(test)]
        {
            active.steps += pending_rows.len() as u64;
        }
        let mut taken_rows: std::collections::HashSet<String> = std::collections::HashSet::new();
        for shot in shots {
            let record = &shot.record;
            let tick = matches!(record.attribution, Attribution::EffectTick { .. });
            let critical = matches!(record.observation, Observation::Hit { critical: true, .. });
            let kill = match &record.location {
                ShotLocation::Pending => None,
                ShotLocation::Kill(kill_id) => {
                    match active
                        .session
                        .kills
                        .iter()
                        .rposition(|kill| &kill.id == kill_id)
                    {
                        Some(index) => Some(index),
                        None => {
                            unplaced.push(shot);
                            continue;
                        }
                    }
                }
            };
            if let Some(index) = kill {
                backup.save_kill(index, &active.session.kills[index]);
            }
            if !tick {
                let amount = super::attribution::AttributionRuntime::amount_of(record);
                let tool_stats = match kill {
                    Some(index) => &mut active.session.kills[index].tool_stats,
                    None => &mut active.accumulator.tool_stats,
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
                    unplaced.push(shot);
                    continue;
                }
                match kill {
                    Some(index) => {
                        let kill = &mut active.session.kills[index];
                        kill.shots_fired -= 1;
                        kill.critical_hits -= i64::from(critical);
                        kill.cost_ped = kill.cost_ped - record.cost;
                    }
                    None => {
                        active.accumulator.shots_fired -= 1;
                        active.accumulator.critical_hits -= i64::from(critical);
                    }
                }
                moved.cost_delta = moved.cost_delta - record.cost;
            }
            if let Some(evidence_id) = &record.evidence_id {
                #[cfg(test)]
                {
                    active.steps += 1;
                }
                if pending_rows.contains(evidence_id) {
                    taken_rows.insert(evidence_id.clone());
                } else {
                    moved.row_deletes.push(evidence_id.clone());
                }
            }
            active.weapons.attribution.untally(&record.attribution);
            if let Some(charge) = shot.charge {
                Self::unnote_unpriced(active, charge);
            }
            if let Some(window) = &record.opened_window {
                active.weapons.attribution.withdraw_effect_window(window);
                moved.window_deletes.push(window.clone());
            }
            taken.push(shot);
        }
        if !taken_rows.is_empty() {
            #[cfg(test)]
            {
                active.steps += active.accumulator.evidence.len() as u64;
            }
            active
                .accumulator
                .evidence
                .retain(|row| !taken_rows.contains(&row.id));
        }
        Self::restore_unplaced(active, unplaced, &mut moved);
        if taken.is_empty() {
            return if moved.tick_unlinks.is_empty() {
                None
            } else {
                Some(moved)
            };
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
            let location = record.location;
            let kill = match &location {
                ShotLocation::Pending => None,
                ShotLocation::Kill(kill_id) => active
                    .session
                    .kills
                    .iter()
                    .rposition(|kill| &kill.id == kill_id),
            };
            let resolved = active
                .weapons
                .attribution
                .classify(observation, observed_at);
            let seq = active.weapons.attribution.apply(&resolved, observed_at);
            let (amount, critical) = match observation {
                Observation::Hit { amount, critical } => (amount, critical),
                Observation::Countered => (0.0, false),
            };
            let mut place_row = |active: &mut ActiveSession, mut row: ShotEvidence| -> String {
                row.context_id = context.context_id;
                row.reload_speed_percent = context.reload_speed_percent;
                let id = row.id.clone();
                match &location {
                    ShotLocation::Pending => active.accumulator.evidence.push(row),
                    ShotLocation::Kill(kill_id) => {
                        moved.row_inserts.push((row, kill_id.clone()));
                    }
                }
                id
            };
            if matches!(resolved.attribution, Attribution::EffectTick { .. }) {
                let row = ShotEvidence::for_shot(
                    active,
                    observation,
                    observed_at,
                    &resolved,
                    None,
                    Ped::ZERO,
                );
                let evidence_id = place_row(active, row);
                active.weapons.attribution.retain_tick(
                    ShotRecord {
                        seq,
                        observed_at,
                        observation,
                        attribution: resolved.attribution,
                        fits: resolved.fits,
                        windows: resolved.windows,
                        location,
                        evidence_id: Some(evidence_id),
                        booked: None,
                        cost: Ped::ZERO,
                        opened_window: None,
                    },
                    context,
                );
                continue;
            }
            let charge = matches!(resolved.attribution, Attribution::Unresolved)
                .then(|| Self::note_unpriced(providers, active, observation, &resolved.fits));
            let tool = resolved.attribution.priced_tool().map(str::to_string);
            let looters = active.hunting_looters;
            let cost = match kill {
                None => {
                    active.accumulator.shots_fired += 1;
                    active.accumulator.critical_hits += i64::from(critical);
                    Self::book_into(
                        providers,
                        &mut active.weapons,
                        looters,
                        &mut active.accumulator.tool_stats,
                        tool.as_deref(),
                        amount,
                        critical,
                    )
                }
                Some(index) => {
                    let cost = Self::book_into(
                        providers,
                        &mut active.weapons,
                        looters,
                        &mut active.session.kills[index].tool_stats,
                        tool.as_deref(),
                        amount,
                        critical,
                    );
                    let kill = &mut active.session.kills[index];
                    kill.shots_fired += 1;
                    kill.critical_hits += i64::from(critical);
                    kill.cost_ped += cost;
                    cost
                }
            };
            moved.cost_delta += cost;
            moved.repriced += 1;
            let evidence_id =
                ShotEvidence::keeps_row(&resolved.attribution, &active.weapons.attribution).then(
                    || {
                        let row = ShotEvidence::for_shot(
                            active,
                            observation,
                            observed_at,
                            &resolved,
                            tool.clone(),
                            cost,
                        );
                        place_row(active, row)
                    },
                );
            // A paid hit of a weapon with an effect opens it, as on arrival.
            let activation = tool.as_deref().and_then(|tool| {
                super::weapon_effects::EffectActivation::for_shot(
                    active,
                    tool,
                    observation,
                    observed_at,
                    cost,
                )
            });
            let opened_window = activation.map(|mut activation| {
                activation.context_id = context.context_id;
                active
                    .weapons
                    .attribution
                    .open_effect_window(activation.window.clone());
                let id = activation.window.id.clone();
                moved.window_inserts.push(activation);
                id
            });
            active.weapons.attribution.remember_shot(
                ShotRecord {
                    seq,
                    observed_at,
                    observation,
                    attribution: resolved.attribution,
                    fits: resolved.fits,
                    windows: resolved.windows,
                    location,
                    evidence_id,
                    booked: tool,
                    cost,
                    opened_window,
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

/// What a change of the tool in hand touches, saved before it.
struct LookBackup {
    backup: Backup,
    hand: Option<InHand>,
    view: ScreenTool,
    applied_at: Option<f64>,
    healing: super::healing::HealingRuntime,
    declared_healer: Option<super::healing::HealingIntent>,
    harvest_tool: Option<(String, Ped)>,
    provisional: Option<(String, std::sync::Arc<serde_json::Value>)>,
}

/// A tool unseen at the stop, settled in memory: the write the stop's
/// transaction makes, and what to put back if it fails.
pub(super) struct StopSettlement {
    pub(super) backup: Backup,
    pub(super) hold: Option<Hold>,
    pub(super) unsettled: Vec<Unsettled>,
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
        // What the player decided stands.
        .filter(|shot| shot.decided_by.is_none())
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
