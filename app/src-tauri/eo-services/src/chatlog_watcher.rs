//! Chat.log file watcher: tails the log and publishes
//! parsed events on the in-process bus for the tracker to consume.
//!
//! The tail is a deliberate 100ms polling loop (part of the recorded
//! scenario timing model, never a filesystem-notification API), and
//! the event contract buffers recognised lines sharing one
//! one-second timestamp into a tick. When the timestamp advances or
//! the file goes idle, the tick closes: loot lines become one grouped
//! event, a completed mission may invoke the quest-reward filter to
//! suppress one loot item or skill gain, enhancer breaks match
//! same-tick shrapnel refunds, and a tick-flushed signal lands last.
//! Payload timestamps travel as the string form of the parsed
//! instant, the shape the recorder's symbol numbering keys on. The
//! drain counters are load-bearing (the replay corpus asserts them);
//! this crate has no logging surface.

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{NaiveDateTime, Utc};
use serde_json::{json, Value};

use crate::bus_events::{
    BusEvent, CombatPayload, EnhancerBreakPayload, EnhancerBreakTag, GlobalPayload,
    HarvestFailPayload, HarvestFailTag, LootGroupPayload, LootItem, LootTag,
    MissionReceivedPayload, MissionReceivedTag, SkillGainPayload, SkillGainTag, TickFlushedPayload,
};
use crate::chatlog_parser::{parse_line, ChatEvent, EventType};
use crate::chatlog_time::{ChatLogClock, ChatLogReading};
use crate::event_bus::{EventBus, Topic};
use crate::ped::Ped;

/// Seconds between reads, exactly the original's tail interval.
pub const TAIL_INTERVAL: Duration = Duration::from_millis(100);

const COMBAT_MESSAGE_PREFIXES: [&str; 13] = [
    "Critical hit",
    "You inflicted",
    "The target Jammed",
    "You missed",
    "The target Dodged",
    "The target Evaded",
    "You took",
    "Damage deflected",
    "You Evaded",
    "You Dodged",
    "You Jammed",
    "The attack missed",
    "You healed",
];

/// The quest-reward filter: receives the mission name, the tick's loot
/// items, and its skill gains; may return indexes to suppress.
pub type QuestRewardFilter =
    Arc<dyn Fn(&str, &[Value], &[Value], bool) -> Option<Value> + Send + Sync>;

/// One raw loot line of a mission-less tick, before the ordinary loot
/// blacklist or a signal-reward suppression can remove it.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalLoot {
    pub item_name: String,
    pub quantity: i64,
    pub value_ped: Ped,
}

/// Stable raw evidence for one mission-less loot clump. `source_id` also
/// rides the ordinary loot-group event internally, allowing a later manual
/// hand-in confirmation to reclassify exactly that acquisition.
#[derive(Debug, Clone, PartialEq)]
pub struct RawLootClump {
    pub source_id: String,
    pub timestamp: Option<String>,
    pub items: Vec<SignalLoot>,
}

/// The raw-loot probe: receives a tick carrying no mission completion.
/// Fire-and-forget by contract: the tail thread never blocks on it, so an
/// implementation dispatches its own async work.
pub type SignalLootProbe = Arc<dyn Fn(RawLootClump) + Send + Sync>;
pub type SignalRewardFilter = Arc<dyn Fn(&[Value]) -> Option<Value> + Send + Sync>;

/// One mission completion a flushed tick carried, handed to the
/// post-publish completion probe with the loot and skill picture the
/// suppression filter saw for the same line (the completion check
/// re-derives the suppressed-reward description from it).
#[derive(Debug, Clone)]
pub struct MissionCompletion {
    pub mission_name: String,
    pub loot_items: Vec<Value>,
    pub skill_gains: Vec<Value>,
    pub isolated: bool,
}

/// The mission-completion probe, invoked strictly AFTER a tick's
/// publishes: by then the tick's loot (the final objective kill and
/// the payout) has dispatched to the consumers, so the completion
/// (and the declared-stretch close it carries) is ordered after the
/// tick's own attribution stamping. Fire-and-forget by contract, like
/// the signal probe.
pub type MissionCompleteProbe = Arc<dyn Fn(Vec<MissionCompletion>) + Send + Sync>;

/// A verbatim line observer (the recording controller's seam).
pub type LineTap = Arc<dyn Fn(&str) + Send + Sync>;

/// Whether a parsed event type reaches the bus at all (`None` only
/// for the internally buffered MissionComplete): the line-level
/// buffering filter.
fn bus_topic(event_type: EventType) -> Option<Topic> {
    match event_type {
        EventType::DamageDealt
        | EventType::CriticalHit
        | EventType::DamageReceived
        | EventType::TargetDodge
        | EventType::TargetEvade
        | EventType::TargetJam
        | EventType::TargetMiss
        | EventType::PlayerDodge
        | EventType::PlayerEvade
        | EventType::PlayerJam
        | EventType::MobMiss
        | EventType::Deflect
        | EventType::SelfHeal => Some(Topic::Combat),
        EventType::Loot => Some(Topic::LootGroup),
        EventType::HarvestFail => Some(Topic::HarvestFail),
        EventType::SkillGain => Some(Topic::SkillGain),
        EventType::EnhancerBreak => Some(Topic::EnhancerBreak),
        EventType::GlobalKill | EventType::HofKill | EventType::GlobalItem | EventType::HofItem => {
            Some(Topic::Global)
        }
        EventType::MissionReceived => Some(Topic::MissionReceived),
        // Buffered for the quest-reward filter, never published.
        EventType::MissionComplete => None,
    }
}

/// The typed bus event for one parsed chat event: the seam where the
/// parser's capture map becomes a compiler-checked payload. The field
/// reads default exactly as the previous consumers did (an extractor
/// always sets its fields, so the defaults are dead in practice). Loot
/// events group per tick and are built at the grouping site; a
/// MissionComplete is buffered for the quest-reward filter and never
/// published.
fn typed_bus_event(event: &ChatEvent) -> Option<BusEvent> {
    let timestamp = timestamp_string(event.timestamp);
    let float = |key: &str| -> f64 { event.data.get(key).and_then(Value::as_f64).unwrap_or(0.0) };
    let text = |key: &str| -> String {
        event
            .data
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let combat = |payload: CombatPayload| Some(BusEvent::Combat(payload));
    match event.event_type {
        EventType::DamageDealt => combat(CombatPayload::DamageDealt {
            amount: float("amount"),
            timestamp,
        }),
        EventType::CriticalHit => combat(CombatPayload::CriticalHit {
            amount: float("amount"),
            timestamp,
        }),
        EventType::DamageReceived => combat(CombatPayload::DamageReceived {
            amount: float("amount"),
            timestamp,
        }),
        EventType::SelfHeal => combat(CombatPayload::SelfHeal {
            amount: float("amount"),
            timestamp,
        }),
        EventType::TargetDodge => combat(CombatPayload::TargetDodge { timestamp }),
        EventType::TargetEvade => combat(CombatPayload::TargetEvade { timestamp }),
        EventType::TargetJam => combat(CombatPayload::TargetJam { timestamp }),
        EventType::TargetMiss => combat(CombatPayload::TargetMiss { timestamp }),
        EventType::PlayerDodge => combat(CombatPayload::PlayerDodge { timestamp }),
        EventType::PlayerEvade => combat(CombatPayload::PlayerEvade { timestamp }),
        EventType::PlayerJam => combat(CombatPayload::PlayerJam { timestamp }),
        EventType::MobMiss => combat(CombatPayload::MobMiss { timestamp }),
        EventType::Deflect => combat(CombatPayload::Deflect { timestamp }),
        EventType::SkillGain => Some(BusEvent::SkillGain(SkillGainPayload {
            kind: SkillGainTag,
            timestamp,
            amount: float("amount"),
            skill_name: text("skill_name"),
        })),
        EventType::EnhancerBreak => Some(BusEvent::EnhancerBreak(EnhancerBreakPayload {
            kind: EnhancerBreakTag,
            timestamp,
            enhancer_name: text("enhancer_name"),
            item_name: text("item_name"),
            remaining: event
                .data
                .get("remaining")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            shrapnel_ped: float("shrapnel_ped"),
        })),
        EventType::GlobalKill => Some(BusEvent::Global(GlobalPayload::GlobalKill {
            timestamp,
            player: text("player"),
            creature: text("creature"),
            value: float("value"),
        })),
        EventType::HofKill => Some(BusEvent::Global(GlobalPayload::HofKill {
            timestamp,
            player: text("player"),
            creature: text("creature"),
            value: float("value"),
        })),
        EventType::GlobalItem => Some(BusEvent::Global(GlobalPayload::GlobalItem {
            timestamp,
            player: text("player"),
            item: text("item"),
            value: float("value"),
        })),
        EventType::HofItem => Some(BusEvent::Global(GlobalPayload::HofItem {
            timestamp,
            player: text("player"),
            item: text("item"),
            value: float("value"),
        })),
        EventType::MissionReceived => Some(BusEvent::MissionReceived(MissionReceivedPayload {
            kind: MissionReceivedTag,
            timestamp,
            mission_name: text("mission_name"),
        })),
        EventType::HarvestFail => Some(BusEvent::HarvestFail(HarvestFailPayload {
            kind: HarvestFailTag,
            timestamp,
        })),
        EventType::Loot | EventType::MissionComplete => None,
    }
}

fn is_internal_buffer_type(event_type: EventType) -> bool {
    event_type == EventType::MissionComplete
}

/// `datetime.isoformat()` for a whole-second instant: the payload
/// timestamp form, chosen so the recorder's symbol table keys the same
/// instant identically whether it arrives from a tick payload here or
/// from a model-dumped wire string downstream.
fn timestamp_string(timestamp: NaiveDateTime) -> String {
    timestamp.format("%Y-%m-%dT%H:%M:%S").to_string()
}

struct Shared {
    bus: Arc<EventBus>,
    path: Mutex<PathBuf>,
    running: AtomicBool,
    ready: (Mutex<bool>, Condvar),
    idle: (Mutex<()>, Condvar),
    lines_seen_total: AtomicU64,
    pending_tick: AtomicBool,
    line_tap: Mutex<Option<LineTap>>,
    quest_reward_filter: Option<QuestRewardFilter>,
    signal_reward_filter: OnceLock<SignalRewardFilter>,
    signal_loot_probe: OnceLock<SignalLootProbe>,
    mission_complete_probe: OnceLock<MissionCompleteProbe>,
    /// The base the lines this watcher reads resolve against. The tail
    /// is the only surface that can see a line's reading beside the
    /// instant it arrived, so it is the only one that can derive the
    /// game server's offset; see [`crate::chatlog_time`].
    chatlog_clock: ChatLogClock,
}

pub struct ChatlogWatcher {
    shared: Arc<Shared>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// The watcher failed to drain within the deadline (see
/// [`ChatlogWatcher::wait_until_drained`]): a test-surface diagnostic
/// carrying what the tail loop had actually processed when time ran
/// out.
#[derive(Debug, thiserror::Error)]
#[error(
    "chatlog watcher did not drain to {min_lines} line(s) within {timeout:?} \
     (read {read}, pending tick={pending_tick})"
)]
pub struct DrainTimeout {
    pub min_lines: u64,
    pub timeout: Duration,
    pub read: u64,
    pub pending_tick: bool,
}

impl ChatlogWatcher {
    pub fn new(
        bus: Arc<EventBus>,
        chatlog_path: impl Into<PathBuf>,
        quest_reward_filter: Option<QuestRewardFilter>,
        chatlog_clock: ChatLogClock,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                bus,
                path: Mutex::new(chatlog_path.into()),
                running: AtomicBool::new(false),
                ready: (Mutex::new(false), Condvar::new()),
                idle: (Mutex::new(()), Condvar::new()),
                lines_seen_total: AtomicU64::new(0),
                pending_tick: AtomicBool::new(false),
                line_tap: Mutex::new(None),
                quest_reward_filter,
                signal_reward_filter: OnceLock::new(),
                signal_loot_probe: OnceLock::new(),
                mission_complete_probe: OnceLock::new(),
                chatlog_clock,
            }),
            thread: Mutex::new(None),
        }
    }

    /// Wire the signal-loot probe. Composition calls this once, after
    /// the quest service exists; a second call is ignored.
    pub fn set_signal_loot_probe(&self, probe: SignalLootProbe) {
        let _ = self.shared.signal_loot_probe.set(probe);
    }

    pub fn set_signal_reward_filter(&self, filter: SignalRewardFilter) {
        let _ = self.shared.signal_reward_filter.set(filter);
    }

    /// Wire the mission-completion probe. Composition calls this once,
    /// after the quest service exists; a second call is ignored.
    pub fn set_mission_complete_probe(&self, probe: MissionCompleteProbe) {
        let _ = self.shared.mission_complete_probe.set(probe);
    }

    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    /// The file the tail loop is bound to.
    pub fn path(&self) -> PathBuf {
        self.shared.path.lock().expect("watcher path").clone()
    }

    /// Install a verbatim line observer.
    pub fn set_line_tap(&self, tap: LineTap) {
        *self.shared.line_tap.lock().expect("line tap") = Some(tap);
    }

    /// Remove the line observer.
    pub fn clear_line_tap(&self) {
        *self.shared.line_tap.lock().expect("line tap") = None;
    }

    /// Cumulative count of chat lines the tail loop has read since
    /// start (the watcher seeks to end-of-file first, so against a
    /// file empty at start this equals the lines appended since).
    pub fn lines_seen(&self) -> u64 {
        self.shared.lines_seen_total.load(Ordering::SeqCst)
    }

    /// True while parsed events are buffered awaiting a tick flush.
    pub fn has_pending_tick(&self) -> bool {
        self.shared.pending_tick.load(Ordering::SeqCst)
    }

    /// Block until the tail loop has read at least `min_lines` lines
    /// and flushed any pending tick. The timeout always runs on the
    /// real clock: a watcher that never drains is a bug to surface,
    /// not a flake to sleep through.
    pub fn wait_until_drained(
        &self,
        min_lines: u64,
        timeout: Duration,
    ) -> Result<(), DrainTimeout> {
        let deadline = Instant::now() + timeout;
        let (lock, condvar) = &self.shared.idle;
        let mut guard = lock.lock().expect("idle lock");
        while self.lines_seen() < min_lines || self.has_pending_tick() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(DrainTimeout {
                    min_lines,
                    timeout,
                    read: self.lines_seen(),
                    pending_tick: self.has_pending_tick(),
                });
            }
            let (next, _timed_out) = condvar
                .wait_timeout(guard, remaining)
                .expect("idle condvar");
            guard = next;
        }
        Ok(())
    }

    /// Start tailing in a background thread; blocks until the tail
    /// loop has opened the file and seeked to its end, so writes
    /// issued immediately afterwards cannot be missed.
    pub fn start(&self) {
        if self.is_running() {
            return;
        }
        if !self.path().is_file() {
            // The original logs a warning and declines to start.
            return;
        }
        {
            let (lock, _) = &self.shared.ready;
            *lock.lock().expect("ready lock") = false;
        }
        self.shared.running.store(true, Ordering::SeqCst);
        let shared = self.shared.clone();
        let handle = std::thread::Builder::new()
            .name("chatlog-watcher".into())
            .spawn(move || tail_loop(&shared))
            .expect("watcher thread spawns");
        *self.thread.lock().expect("thread handle") = Some(handle);

        let (lock, condvar) = &self.shared.ready;
        let guard = lock.lock().expect("ready lock");
        let (_guard, result) = condvar
            .wait_timeout_while(guard, Duration::from_secs(5), |ready| !*ready)
            .expect("ready condvar");
        let _ = result; // The original logs on a missed deadline; the
                        // start still returns either way.
    }

    /// Stop the watcher (joins the tail thread; the loop exits within
    /// one tail interval).
    pub fn stop(&self) {
        self.shared.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread.lock().expect("thread handle").take() {
            let _ = handle.join();
        }
    }

    /// Stop, update the path, reset the tick, and start again.
    pub fn restart(&self, new_path: impl Into<PathBuf>) {
        self.stop();
        *self.shared.path.lock().expect("watcher path") = new_path.into();
        self.shared.pending_tick.store(false, Ordering::SeqCst);
        self.start();
    }
}

impl Drop for ChatlogWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

fn signal_ready(shared: &Shared) {
    let (lock, condvar) = &shared.ready;
    *lock.lock().expect("ready lock") = true;
    condvar.notify_all();
}

fn signal_idle(shared: &Shared) {
    let (lock, condvar) = &shared.idle;
    let _guard = lock.lock().expect("idle lock");
    condvar.notify_all();
}

fn tail_loop(shared: &Shared) {
    let path = shared.path.lock().expect("watcher path").clone();
    let mut tick = TickBuffer::default();

    let mut run = || -> std::io::Result<()> {
        let file = std::fs::File::open(&path)?;
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::End(0))?;
        signal_ready(shared);

        let mut buffer = Vec::new();
        while shared.running.load(Ordering::SeqCst) {
            let bytes = reader.read_until(b'\n', &mut buffer)?;
            if bytes == 0 {
                flush_tick(shared, &mut tick);
                signal_idle(shared);
                std::thread::sleep(TAIL_INTERVAL);
                continue;
            }
            // A read can surface the head of an in-flight append (the
            // writer's flush is not atomic with respect to a tailing
            // reader). Hold the partial and resume reading: the next
            // pass appends the remainder, so the line processes whole
            // instead of as two unparseable fragments.
            if buffer.last() != Some(&b'\n') {
                continue;
            }
            // The original reads text with errors="replace".
            let line = String::from_utf8_lossy(&buffer).into_owned();
            buffer.clear();
            if let Some(tap) = shared.line_tap.lock().expect("line tap").clone() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tap(&line)));
            }
            process_line(shared, &mut tick, &line);
        }
        // The shutdown pass processes a final unterminated line the
        // way the original's readline-at-EOF does.
        if !buffer.is_empty() {
            let line = String::from_utf8_lossy(&buffer).into_owned();
            if let Some(tap) = shared.line_tap.lock().expect("line tap").clone() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tap(&line)));
            }
            process_line(shared, &mut tick, &line);
        }
        flush_tick(shared, &mut tick);
        Ok(())
    };
    if run().is_err() {
        shared.running.store(false, Ordering::SeqCst);
    }
    // Unblock start() and wake drain waiters even if the loop never
    // reached its seek, so a crashed watcher surfaces as a failed
    // re-check rather than a hang.
    signal_ready(shared);
    signal_idle(shared);
}

#[derive(Default)]
struct TickBuffer {
    timestamp: Option<NaiveDateTime>,
    events: Vec<ChatEvent>,
}

fn process_line(shared: &Shared, tick: &mut TickBuffer, line: &str) {
    // Parse and buffer the line first, then count it as seen. The
    // `lines_seen` bump MUST be last: it is the (test-only) drain barrier's
    // signal, and `pending_tick` is set inside `process_parsed_line` before
    // it. Counting the line before its events were buffered left a window
    // where a drain waiter saw `lines_seen >= n` with `pending_tick` still
    // false and returned before the line's events were dispatchable: an
    // intermittent "drained but event not yet published" flake under load.
    // Every line (event-bearing or skipped) still counts exactly once.
    process_parsed_line(shared, tick, line);
    shared.lines_seen_total.fetch_add(1, Ordering::SeqCst);
}

fn process_parsed_line(shared: &Shared, tick: &mut TickBuffer, line: &str) {
    if can_skip_idle_combat_line(shared, line) {
        return;
    }
    let Some(event) = parse_line(line) else {
        return;
    };
    if bus_topic(event.event_type).is_none() && !is_internal_buffer_type(event.event_type) {
        return;
    }

    if let Some(current) = tick.timestamp {
        if event.timestamp != current {
            flush_tick(shared, tick);
        }
    }
    tick.timestamp = Some(event.timestamp);
    tick.events.push(event);
    shared.pending_tick.store(true, Ordering::SeqCst);
}

/// Fast-path: skip combat parsing when nobody subscribes to combat.
fn can_skip_idle_combat_line(shared: &Shared, line: &str) -> bool {
    if shared.bus.has_subscribers(Topic::Combat) {
        return false;
    }
    let marker = "[System] [] ";
    let Some(index) = line.find(marker) else {
        return false;
    };
    let message = &line[index + marker.len()..];
    COMBAT_MESSAGE_PREFIXES
        .iter()
        .any(|prefix| message.starts_with(prefix))
}

fn flush_tick(shared: &Shared, tick: &mut TickBuffer) {
    if tick.events.is_empty() {
        tick.timestamp = None;
        shared.pending_tick.store(false, Ordering::SeqCst);
        return;
    }

    let events = std::mem::take(&mut tick.events);
    let tick_ts = tick.timestamp;

    // Read the tick's own reading against the moment it reached us
    // before anything downstream resolves it: the tail seeks to
    // end-of-file and polls, so this line was appended moments ago and
    // the gap is the game server's offset from UTC. `Utc::now()` here
    // is the live boundary itself, not a logical clock: what is being
    // measured is when a real file's real bytes arrived.
    if let Some(reading) = tick_ts {
        shared
            .chatlog_clock
            .observe(ChatLogReading::new(reading), Utc::now());
    }

    let mut loot_events: Vec<ChatEvent> = Vec::new();
    let mut skill_events: Vec<ChatEvent> = Vec::new();
    let mut mission_events: Vec<ChatEvent> = Vec::new();
    let mut enhancer_events: Vec<ChatEvent> = Vec::new();
    let mut other_events: Vec<ChatEvent> = Vec::new();
    for event in events {
        match event.event_type {
            EventType::Loot => loot_events.push(event),
            EventType::SkillGain => skill_events.push(event),
            EventType::MissionComplete | EventType::MissionReceived => mission_events.push(event),
            EventType::EnhancerBreak => enhancer_events.push(event),
            _ => other_events.push(event),
        }
    }

    // Quest-reward suppression, plus the completion entries for the
    // post-publish probe. Each entry snapshots the loot and skill
    // picture as the filter saw it for that line, so the completion
    // check downstream derives the same suppressed-reward description.
    let completes: Vec<ChatEvent> = mission_events
        .iter()
        .filter(|e| e.event_type == EventType::MissionComplete)
        .cloned()
        .collect();
    let mut mission_completions: Vec<MissionCompletion> = Vec::with_capacity(completes.len());
    let isolated_completion_tick = other_events.is_empty() && enhancer_events.is_empty();
    for complete in &completes {
        let mission_name = complete
            .data
            .get("mission_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let loot_data: Vec<Value> = loot_events
            .iter()
            .map(|e| {
                serde_json::json!({
                    "item_name": e.data.get("item_name").cloned().unwrap_or(Value::from("")),
                    "quantity": e.data.get("quantity").cloned().unwrap_or(Value::from(1)),
                    "value": e.data.get("value").cloned().unwrap_or(Value::from(0.0)),
                })
            })
            .collect();
        let skill_data: Vec<Value> = skill_events
            .iter()
            .map(|e| {
                serde_json::json!({
                    "skill_name": e.data.get("skill_name").cloned().unwrap_or(Value::from("")),
                    "amount": e.data.get("amount").cloned().unwrap_or(Value::from(0.0)),
                })
            })
            .collect();

        if let Some(filter) = shared.quest_reward_filter.clone() {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                filter(
                    &mission_name,
                    &loot_data,
                    &skill_data,
                    isolated_completion_tick,
                )
            }))
            .unwrap_or(None);

            if let Some(result) = result {
                let mut loot_indices: Vec<usize> = result
                    .get("suppress_loot_indices")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .map(|index| index as usize)
                    .filter(|index| *index < loot_events.len())
                    .collect();
                loot_indices.sort_unstable();
                loot_indices.dedup();
                for index in loot_indices.into_iter().rev() {
                    loot_events.remove(index);
                }
                let mut skill_indices: Vec<usize> = result
                    .get("suppress_skill_indices")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .map(|index| index as usize)
                    .filter(|index| *index < skill_events.len())
                    .collect();
                skill_indices.sort_unstable();
                skill_indices.dedup();
                for index in skill_indices.into_iter().rev() {
                    skill_events.remove(index);
                }
            }
        }
        mission_completions.push(MissionCompletion {
            mission_name,
            loot_items: loot_data,
            skill_gains: skill_data,
            isolated: isolated_completion_tick,
        });
    }

    // A mission-less loot tick may complete a signal quest. Snapshot its
    // evidence before reward suppression so the same marker can prove the
    // completion and be removed from ordinary loot. A tick with a mission
    // completion belongs to that mission path instead, so a daily voucher
    // cannot masquerade as a boss marker. The probe runs only after every
    // publish below, allowing the clump to stamp into the declared stretch
    // before the completion closes it.
    let loot_source_id = (!loot_events.is_empty()).then(|| uuid::Uuid::new_v4().to_string());
    let raw_loot_clump: Option<RawLootClump> = if completes.is_empty() && !loot_events.is_empty() {
        shared.signal_loot_probe.get().and_then(|_| {
            let source_id = loot_source_id.clone()?;
            let items = loot_events
                .iter()
                .filter_map(|e| {
                    Some(SignalLoot {
                        item_name: e.data.get("item_name")?.as_str()?.to_string(),
                        quantity: e
                            .data
                            .get("quantity")
                            .and_then(Value::as_i64)
                            .unwrap_or(1)
                            .max(1),
                        value_ped: Ped(e.data.get("value").and_then(Value::as_f64).unwrap_or(0.0)),
                    })
                })
                .collect();
            Some(RawLootClump {
                source_id,
                timestamp: tick_ts.map(timestamp_string),
                items,
            })
        })
    } else {
        None
    };
    if completes.is_empty() {
        if let Some(filter) = shared.signal_reward_filter.get() {
            let loot_data: Vec<Value> = loot_events
                .iter()
                .map(|event| {
                    json!({
                        "item_name": event.data.get("item_name").cloned().unwrap_or_default(),
                        "quantity": event.data.get("quantity").cloned().unwrap_or(json!(1)),
                        "value": event.data.get("value").cloned().unwrap_or(json!(0.0)),
                    })
                })
                .collect();
            if let Some(result) = filter(&loot_data) {
                let mut indices: Vec<usize> = result
                    .get("suppress_loot_indices")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .map(|index| index as usize)
                    .filter(|index| *index < loot_events.len())
                    .collect();
                indices.sort_unstable();
                indices.dedup();
                for index in indices.into_iter().rev() {
                    loot_events.remove(index);
                }
            }
        }
    }

    let refund_matches = match_enhancer_shrapnel(&loot_events, &enhancer_events);

    // Enhancer breaks before loot finalisation.
    for event in &enhancer_events {
        if let Some(event) = typed_bus_event(event) {
            shared.bus.publish(&event);
        }
    }

    // The grouped loot event.
    if !loot_events.is_empty() {
        let mut items = Vec::new();
        let mut total = 0.0;
        for (index, event) in loot_events.iter().enumerate() {
            let value = event
                .data
                .get("value")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            items.push(LootItem {
                item_name: event
                    .data
                    .get("item_name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                quantity: event
                    .data
                    .get("quantity")
                    .and_then(Value::as_i64)
                    .unwrap_or(1),
                value_ped: value,
                is_enhancer_shrapnel: refund_matches[index],
            });
            total += value;
        }
        shared.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
            kind: LootTag,
            source_id: loot_source_id,
            timestamp: tick_ts.map(timestamp_string),
            items,
            total_ped: eo_wire::normalizer::round_half_even(total, 4),
        }));
    }

    // Mission events.
    for event in &mission_events {
        if let Some(event) = typed_bus_event(event) {
            shared.bus.publish(&event);
        }
    }

    // Skill events.
    for event in &skill_events {
        if let Some(event) = typed_bus_event(event) {
            shared.bus.publish(&event);
        }
    }

    // Everything else.
    for event in &other_events {
        if let Some(event) = typed_bus_event(event) {
            shared.bus.publish(&event);
        }
    }

    // The settled-tick boundary lands last, after every per-event
    // publish above has dispatched synchronously.
    shared
        .bus
        .publish(&BusEvent::TickFlushed(TickFlushedPayload {
            timestamp: tick_ts.map(timestamp_string),
        }));

    // Completions fire strictly after every publish, one rule for
    // both quest kinds: by now the tick's loot (the final objective
    // kill, the payout, a boss's clump) has dispatched into the
    // consumers' inboxes, so the completion (and the declared-stretch
    // close it carries) is ordered after the tick's own attribution
    // stamping. The suppression filter already answered pre-publish,
    // so a suppressed reward echo never reached anyone.
    if !mission_completions.is_empty() {
        if let Some(probe) = shared.mission_complete_probe.get() {
            let probe = probe.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                probe(mission_completions)
            }));
        }
    }
    if let (Some(clump), Some(probe)) = (raw_loot_clump, shared.signal_loot_probe.get()) {
        if !clump.items.is_empty() {
            let probe = probe.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| probe(clump)));
        }
    }

    tick.timestamp = None;
    shared.pending_tick.store(false, Ordering::SeqCst);
}

/// Flag same-tick shrapnel loot matching enhancer refund values
/// (first unmatched shrapnel wins per refund, 1e-9 tolerance).
fn match_enhancer_shrapnel(loot_events: &[ChatEvent], enhancer_events: &[ChatEvent]) -> Vec<bool> {
    let mut matches = vec![false; loot_events.len()];
    let refunds: Vec<f64> = enhancer_events
        .iter()
        .filter_map(|event| event.data.get("shrapnel_ped").and_then(Value::as_f64))
        .filter(|refund| *refund > 0.0)
        .collect();

    for refund in refunds {
        for (index, loot_event) in loot_events.iter().enumerate() {
            if matches[index] {
                continue;
            }
            let name = loot_event
                .data
                .get("item_name")
                .and_then(Value::as_str)
                .unwrap_or("");
            if name.to_lowercase() != "shrapnel" {
                continue;
            }
            let loot_ped = loot_event
                .data
                .get("value")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            if (loot_ped - refund).abs() < 1e-9 {
                matches[index] = true;
                break;
            }
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    struct Pipeline {
        _dir: tempfile::TempDir,
        log_path: PathBuf,
        bus: Arc<EventBus>,
        watcher: ChatlogWatcher,
        stream: Arc<Mutex<Vec<(Topic, Value)>>>,
    }

    fn pipeline(filter: Option<QuestRewardFilter>) -> Pipeline {
        pipeline_on_chatlog_clock(filter, ChatLogClock::host_local())
    }

    fn pipeline_on_chatlog_clock(
        filter: Option<QuestRewardFilter>,
        chatlog_clock: ChatLogClock,
    ) -> Pipeline {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("chat_testing.log");
        std::fs::File::create(&log_path).unwrap();
        let bus = Arc::new(EventBus::new());
        let stream = Arc::new(Mutex::new(Vec::new()));
        let sink = stream.clone();
        bus.add_tap(move |event| {
            sink.lock()
                .unwrap()
                .push((event.topic(), event.payload_value()));
        });
        let watcher = ChatlogWatcher::new(bus.clone(), &log_path, filter, chatlog_clock);
        watcher.start();
        Pipeline {
            _dir: dir,
            log_path,
            bus,
            watcher,
            stream,
        }
    }

    fn append(pipeline: &Pipeline, lines: &[&str]) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&pipeline.log_path)
            .unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        file.flush().unwrap();
    }

    fn drain(pipeline: &Pipeline, min_lines: u64) {
        pipeline
            .watcher
            .wait_until_drained(min_lines, Duration::from_secs(10))
            .unwrap();
    }

    fn append_raw(pipeline: &Pipeline, text: &str) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&pipeline.log_path)
            .unwrap();
        write!(file, "{text}").unwrap();
        file.flush().unwrap();
    }

    /// The tail is the only surface that sees a reading beside the
    /// instant it arrived, so it is where the game server's offset from
    /// UTC gets derived. Stamp a line the way a server two hours ahead
    /// of UTC would and the watcher should settle on that, whatever
    /// zone the machine running the test is in.
    #[test]
    fn the_tail_derives_the_server_offset_from_a_live_line() {
        let chatlog_clock = ChatLogClock::observed();
        let pipeline = pipeline_on_chatlog_clock(None, chatlog_clock.clone());
        assert_eq!(
            chatlog_clock.server_offset_seconds(),
            None,
            "nothing read yet"
        );

        let server_now = Utc::now() + chrono::TimeDelta::hours(2);
        append(
            &pipeline,
            &[&format!(
                "{} [System] [] You received Shrapnel x (100) Value: 0.01 PED",
                server_now.format("%Y-%m-%d %H:%M:%S")
            )],
        );
        drain(&pipeline, 1);

        assert_eq!(
            chatlog_clock.server_offset_seconds(),
            Some(2 * 3600),
            "the live gap between the reading and its arrival is the server's offset"
        );
    }

    #[test]
    fn partial_appends_process_as_whole_lines() {
        let pipeline = pipeline(None);
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = received.clone();
        pipeline.bus.subscribe(Topic::Combat, move |event| {
            sink.lock().unwrap().push(event.payload_value());
        });

        // The head of an in-flight append: the tail must hold it
        // rather than parse the fragment. The pause spans several
        // tail intervals so the partial is observably available.
        append_raw(
            &pipeline,
            "2026-05-19 10:00:01 [System] [] You inflicted 12.0 points of dam",
        );
        std::thread::sleep(TAIL_INTERVAL * 3);
        assert_eq!(pipeline.watcher.lines_seen(), 0, "no whole line yet");
        append_raw(&pipeline, "age\n");
        drain(&pipeline, 1);
        {
            let received = received.lock().unwrap();
            assert_eq!(received.len(), 1, "the joined line parses once");
            assert_eq!(received[0]["amount"], 12.0);
        }

        // A final unterminated line still processes at shutdown, the
        // way the original's readline-at-EOF surfaces it.
        append_raw(
            &pipeline,
            "2026-05-19 10:00:02 [System] [] You inflicted 7.5 points of damage",
        );
        std::thread::sleep(TAIL_INTERVAL * 3);
        pipeline.watcher.stop();
        let received = received.lock().unwrap();
        assert_eq!(received.len(), 2, "the shutdown pass takes the tail line");
        assert_eq!(received[1]["amount"], 7.5);
    }

    #[test]
    fn ticks_group_loot_and_signal_flush_last() {
        let pipeline = pipeline(None);
        append(
            &pipeline,
            &[
                "2026-05-19 10:00:02 [System] [] You received Shrapnel x (500) Value: 5.00 PED",
                "2026-05-19 10:00:02 [System] [] You received Wool Value: 1.50 PED",
                "2026-05-19 10:00:03 [System] [] You have gained 0.21 Combat Reflexes",
            ],
        );
        drain(&pipeline, 3);
        pipeline.watcher.stop();

        let stream = pipeline.stream.lock().unwrap();
        let topics: Vec<Topic> = stream.iter().map(|(topic, _)| *topic).collect();
        assert_eq!(
            topics,
            [
                Topic::LootGroup,
                Topic::TickFlushed,
                Topic::SkillGain,
                Topic::TickFlushed
            ],
            "tick one groups loot; tick two carries the gain"
        );
        let loot = &stream[0].1;
        assert_eq!(loot["type"], "loot");
        assert_eq!(loot["timestamp"], "2026-05-19T10:00:02");
        assert_eq!(loot["total_ped"], 6.5);
        assert_eq!(loot["items"][0]["item_name"], "Shrapnel");
        assert_eq!(loot["items"][0]["is_enhancer_shrapnel"], false);
        assert_eq!(loot["items"][1]["quantity"], 1);
        assert_eq!(stream[1].1["timestamp"], "2026-05-19T10:00:02");
    }

    #[test]
    fn enhancer_breaks_flag_matching_shrapnel_and_emit_first() {
        let pipeline = pipeline(None);
        append(
            &pipeline,
            &[
                "2026-05-19 10:00:04 [System] [] Your enhancer Weapon Damage Enhancer 3 on your ArMatrix LR-35 broke. You have 7 enhancers remaining on the item. You received 0.8000 PED Shrapnel. ",
                "2026-05-19 10:00:04 [System] [] You received Shrapnel x (80) Value: 0.80 PED",
                "2026-05-19 10:00:04 [System] [] You received Shrapnel x (10) Value: 0.10 PED",
            ],
        );
        drain(&pipeline, 3);
        pipeline.watcher.stop();

        let stream = pipeline.stream.lock().unwrap();
        assert_eq!(stream[0].0, Topic::EnhancerBreak);
        assert_eq!(stream[0].1["remaining"], 7);
        assert_eq!(stream[1].0, Topic::LootGroup);
        let items = stream[1].1["items"].as_array().unwrap();
        assert_eq!(items[0]["is_enhancer_shrapnel"], true, "0.80 matches");
        assert_eq!(items[1]["is_enhancer_shrapnel"], false);
    }

    #[test]
    fn quest_filter_suppresses_indexed_rewards() {
        let filter: QuestRewardFilter = Arc::new(|mission, loot, skills, _| {
            assert_eq!(mission, "Iron Challenge");
            assert_eq!(loot.len(), 2);
            assert_eq!(skills.len(), 1);
            Some(serde_json::json!({
                "suppress_loot_indices": [1],
                "suppress_skill_indices": [0],
            }))
        });
        let pipeline = pipeline(Some(filter));
        append(
            &pipeline,
            &[
                "2026-05-19 10:00:05 [System] [] You received Wool Value: 1.50 PED",
                "2026-05-19 10:00:05 [System] [] You received Reward Token Value: 5.00 PED",
                "2026-05-19 10:00:05 [System] [] You have gained 0.21 Combat Reflexes",
                "2026-05-19 10:00:05 [System] [] Mission completed (Iron Challenge)",
            ],
        );
        drain(&pipeline, 4);
        pipeline.watcher.stop();

        let stream = pipeline.stream.lock().unwrap();
        let topics: Vec<Topic> = stream.iter().map(|(topic, _)| *topic).collect();
        assert_eq!(
            topics,
            [Topic::LootGroup, Topic::TickFlushed],
            "the suppressed skill never publishes; mission completes are internal"
        );
        let items = stream[0].1["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "the reward token is suppressed");
        assert_eq!(items[0]["item_name"], "Wool");
    }

    #[test]
    fn combat_skips_without_subscribers_and_flows_with_them() {
        let pipeline = pipeline(None);
        append(
            &pipeline,
            &["2026-05-19 10:00:00 [System] [] You inflicted 10.5 points of damage"],
        );
        drain(&pipeline, 1);
        {
            let stream = pipeline.stream.lock().unwrap();
            assert!(
                stream.is_empty(),
                "no combat subscriber: the fast path skips parsing entirely"
            );
        }

        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = received.clone();
        pipeline.bus.subscribe(Topic::Combat, move |event| {
            sink.lock().unwrap().push(event.payload_value());
        });
        append(
            &pipeline,
            &["2026-05-19 10:00:01 [System] [] You inflicted 12.0 points of damage"],
        );
        drain(&pipeline, 2);
        pipeline.watcher.stop();
        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0]["type"], "damage_dealt");
        assert_eq!(received[0]["amount"], 12.0);
        assert_eq!(received[0]["timestamp"], "2026-05-19T10:00:01");
    }

    #[test]
    fn seek_to_end_skips_history_and_restart_rebinds() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("chat_testing.log");
        std::fs::write(
            &log_path,
            "2026-05-19 09:59:59 [System] [] You received Old Loot Value: 9.99 PED
",
        )
        .unwrap();
        let bus = Arc::new(EventBus::new());
        let stream = Arc::new(Mutex::new(Vec::new()));
        let sink = stream.clone();
        bus.add_tap(move |event| {
            sink.lock()
                .unwrap()
                .push((event.topic(), event.payload_value()));
        });
        let watcher = ChatlogWatcher::new(bus.clone(), &log_path, None, ChatLogClock::host_local());
        watcher.start();
        assert!(watcher.is_running());

        let second = dir.path().join("chat_two.log");
        std::fs::File::create(&second).unwrap();
        watcher.restart(&second);
        assert_eq!(watcher.path(), second);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&second)
            .unwrap();
        writeln!(
            file,
            "2026-05-19 10:00:00 [System] [] You received New Loot Value: 1.00 PED"
        )
        .unwrap();
        watcher
            .wait_until_drained(1, Duration::from_secs(10))
            .unwrap();
        watcher.stop();
        assert!(!watcher.is_running());

        let stream = stream.lock().unwrap();
        let loot: Vec<&Value> = stream
            .iter()
            .filter(|(topic, _)| *topic == Topic::LootGroup)
            .map(|(_, data)| data)
            .collect();
        assert_eq!(loot.len(), 1, "history is never replayed");
        assert_eq!(loot[0]["items"][0]["item_name"], "New Loot");
    }

    #[test]
    fn line_taps_observe_verbatim_lines() {
        let pipeline = pipeline(None);
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        pipeline.watcher.set_line_tap(Arc::new(move |line: &str| {
            sink.lock().unwrap().push(line.to_string());
        }));
        append(
            &pipeline,
            &["2026-05-19 10:00:00 [Local] [] untracked chatter"],
        );
        drain(&pipeline, 1);
        pipeline.watcher.clear_line_tap();
        append(&pipeline, &["2026-05-19 10:00:01 [Local] [] more chatter"]);
        drain(&pipeline, 2);
        pipeline.watcher.stop();
        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("untracked chatter"));
        assert!(lines[0].ends_with('\n'), "the tap sees the verbatim line");
    }

    #[test]
    fn missing_files_decline_to_start() {
        let bus = Arc::new(EventBus::new());
        let watcher = ChatlogWatcher::new(
            bus,
            "/nonexistent/chat.log",
            None,
            ChatLogClock::host_local(),
        );
        watcher.start();
        assert!(!watcher.is_running());
    }

    #[test]
    fn out_of_range_suppression_indexes_are_ignored() {
        // Exactly the length and negative, in both polarities across
        // the two runs: out of range either way, nothing suppressed,
        // nothing panics in the tail thread.
        for swap in [false, true] {
            let filter: QuestRewardFilter = Arc::new(move |_, loot, skills, _| {
                let (loot_index, skill_index) = if swap {
                    (-(1 + loot.len() as i64), skills.len() as i64)
                } else {
                    (loot.len() as i64, -(1 + skills.len() as i64))
                };
                Some(serde_json::json!({
                    "suppress_loot_indices": [loot_index],
                    "suppress_skill_indices": [skill_index],
                }))
            });
            let pipeline = pipeline(Some(filter));
            append(
                &pipeline,
                &[
                    "2026-05-19 10:00:05 [System] [] You received Wool Value: 1.50 PED",
                    "2026-05-19 10:00:05 [System] [] You have gained 0.21 Combat Reflexes",
                    "2026-05-19 10:00:05 [System] [] Mission completed (Iron Challenge)",
                ],
            );
            drain(&pipeline, 3);
            pipeline.watcher.stop();
            let stream = pipeline.stream.lock().unwrap();
            let items = stream[0].1["items"].as_array().unwrap();
            assert_eq!(items.len(), 1, "nothing suppressed (swap={swap})");
            assert_eq!(stream[1].0, Topic::SkillGain, "the gain still publishes");
        }
    }

    #[test]
    fn zero_value_refunds_never_flag_shrapnel() {
        let pipeline = pipeline(None);
        append(
            &pipeline,
            &[
                "2026-05-19 10:00:04 [System] [] Your enhancer Weapon Damage Enhancer 3 on your ArMatrix LR-35 broke. You have 7 enhancers remaining on the item. You received 0.0000 PED Shrapnel. ",
                "2026-05-19 10:00:04 [System] [] You received Shrapnel x (1) Value: 0.00 PED",
            ],
        );
        drain(&pipeline, 2);
        pipeline.watcher.stop();
        let stream = pipeline.stream.lock().unwrap();
        let items = stream[1].1["items"].as_array().unwrap();
        assert_eq!(
            items[0]["is_enhancer_shrapnel"], false,
            "a zero refund matches nothing"
        );
    }

    #[test]
    fn start_returns_promptly_and_drop_stops_the_thread() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("chat_testing.log");
        std::fs::File::create(&log_path).unwrap();
        let bus = Arc::new(EventBus::new());
        let stream = Arc::new(Mutex::new(Vec::new()));
        let sink = stream.clone();
        bus.add_tap(move |event| {
            sink.lock()
                .unwrap()
                .push((event.topic(), event.payload_value()));
        });

        let started = Instant::now();
        {
            let watcher =
                ChatlogWatcher::new(bus.clone(), &log_path, None, ChatLogClock::host_local());
            watcher.start();
            assert!(
                started.elapsed() < Duration::from_secs(4),
                "start returns once the ready gate signals, not at the deadline"
            );
        } // Dropped while running: the tail thread must stop.

        let mut handle = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap();
        use std::io::Write as _;
        writeln!(
            handle,
            "2026-05-19 10:00:00 [System] [] You received Wool Value: 1.50 PED"
        )
        .unwrap();
        drop(handle);
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            stream.lock().unwrap().is_empty(),
            "a dropped watcher no longer tails"
        );
    }

    /// The signal-loot probe fires for a loot tick with NO mission
    /// completion (the whole tick's item names, in order) and never for
    /// a tick that carries one: a mission tick is the mission
    /// machinery's, so a daily's marker item cannot masquerade as an
    /// instance boss's. The probe fires strictly AFTER the tick's loot
    /// publish: the completion it triggers closes the declared stretch,
    /// and the clump that pays for the run must be dispatched for
    /// attribution stamping before anything can close it.
    #[test]
    fn the_signal_probe_sees_only_mission_less_loot_ticks_after_their_publish() {
        let pipeline = pipeline(None);
        // One ordered log for both observers: bus taps and probe calls
        // interleave in dispatch order.
        let order = Arc::new(Mutex::new(Vec::<String>::new()));
        let probed = Arc::new(Mutex::new(Vec::<RawLootClump>::new()));
        let sink = probed.clone();
        let probe_order = order.clone();
        pipeline
            .watcher
            .set_signal_loot_probe(Arc::new(move |loot| {
                probe_order.lock().unwrap().push("probe".to_string());
                sink.lock().unwrap().push(loot);
            }));
        let tap_order = order.clone();
        pipeline.bus.add_tap(move |event| {
            tap_order
                .lock()
                .unwrap()
                .push(format!("{:?}", event.topic()));
        });

        // A boss-shaped tick: loot only, no mission line.
        append(
            &pipeline,
            &[
                "2026-05-19 10:00:00 [System] [] You received Shrapnel x (4639) Value: 0.4639 PED",
                "2026-05-19 10:00:00 [System] [] You received Hyperion Daily Voucher x (1) Value: 0 PED",
                "2026-05-19 10:00:01 [System] [] You inflicted 10.0 points of damage",
            ],
        );
        drain(&pipeline, 3);
        let evidence = probed.lock().unwrap();
        assert_eq!(evidence.len(), 1);
        assert!(!evidence[0].source_id.is_empty());
        assert_eq!(
            evidence[0].items,
            vec![
                SignalLoot {
                    item_name: "Shrapnel".to_string(),
                    quantity: 4639,
                    value_ped: Ped(0.4639),
                },
                SignalLoot {
                    item_name: "Hyperion Daily Voucher".to_string(),
                    quantity: 1,
                    value_ped: Ped::ZERO,
                },
            ],
            "each entry carries its line's stacked quantity"
        );
        drop(evidence);
        {
            let order = order.lock().unwrap();
            let probe_at = order.iter().position(|entry| entry == "probe");
            let loot_at = order.iter().position(|entry| entry.contains("Loot"));
            assert!(
                loot_at.is_some() && probe_at > loot_at,
                "the probe fires after the tick's loot publish, not before: {order:?}"
            );
        }

        // A daily-shaped tick: the same marker beside a mission
        // completion. The probe must not fire for it.
        probed.lock().unwrap().clear();
        append(
            &pipeline,
            &[
                "2026-05-19 10:00:05 [System] [] You received Hyperion Daily Voucher x (1) Value: 0 PED",
                "2026-05-19 10:00:05 [System] [] Mission completed (Iron Challenge)",
                "2026-05-19 10:00:06 [System] [] You inflicted 10.0 points of damage",
            ],
        );
        drain(&pipeline, 6);
        assert!(
            probed.lock().unwrap().is_empty(),
            "a mission tick never reaches the signal probe"
        );
    }

    /// The mission-completion probe fires strictly AFTER a mission
    /// tick's publishes, one ordering rule for both quest kinds: the
    /// tick's own loot (the final objective kill, the payout) is
    /// dispatched for attribution stamping before the completion can
    /// close the declared stretch. Each entry carries the loot and
    /// skill picture the suppression filter saw for its line.
    #[test]
    fn the_mission_probe_fires_after_the_ticks_publishes() {
        let pipeline = pipeline(None);
        let order = Arc::new(Mutex::new(Vec::<String>::new()));
        let probed = Arc::new(Mutex::new(Vec::<MissionCompletion>::new()));
        let sink = probed.clone();
        let probe_order = order.clone();
        pipeline
            .watcher
            .set_mission_complete_probe(Arc::new(move |completions| {
                probe_order.lock().unwrap().push("probe".to_string());
                sink.lock().unwrap().extend(completions);
            }));
        let tap_order = order.clone();
        pipeline.bus.add_tap(move |event| {
            tap_order
                .lock()
                .unwrap()
                .push(format!("{:?}", event.topic()));
        });

        append(
            &pipeline,
            &[
                "2026-05-19 10:00:00 [System] [] You received Shrapnel x (4639) Value: 0.4639 PED",
                "2026-05-19 10:00:00 [System] [] Mission completed (ARIS - Daily Hunting 1: Faint Fieroids)",
                "2026-05-19 10:00:01 [System] [] You inflicted 10.0 points of damage",
            ],
        );
        drain(&pipeline, 3);
        {
            let probed = probed.lock().unwrap();
            assert_eq!(probed.len(), 1);
            assert_eq!(
                probed[0].mission_name,
                "ARIS - Daily Hunting 1: Faint Fieroids"
            );
            assert_eq!(probed[0].loot_items[0]["item_name"], "Shrapnel");
        }
        let order = order.lock().unwrap();
        let probe_at = order.iter().position(|entry| entry == "probe");
        let loot_at = order.iter().position(|entry| entry.contains("Loot"));
        let flushed_at = order.iter().position(|entry| entry.contains("TickFlushed"));
        assert!(
            loot_at.is_some()
                && flushed_at.is_some()
                && probe_at > loot_at
                && probe_at > flushed_at,
            "the completion probe fires after the tick's publishes: {order:?}"
        );
    }
}
