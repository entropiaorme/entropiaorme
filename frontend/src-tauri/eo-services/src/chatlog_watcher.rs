//! Chat.log file watcher, ported from
//! `backend/services/chatlog_watcher.py`: tails the log and publishes
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
//! Payload timestamps travel as the backend's string form of the
//! parsed instant, so the recorder's symbol numbering keys
//! identically. The original's debug-only performance counters are
//! omitted (this crate has no logging surface); the drain counters
//! they sat beside are load-bearing and fully ported.

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chrono::NaiveDateTime;
use serde_json::{Map, Value};

use crate::chatlog_parser::{parse_line, ChatEvent, EventType};
use crate::event_bus::{EventBus, Topic};

/// Seconds between reads, exactly the original's tail interval.
pub const TAIL_INTERVAL: Duration = Duration::from_millis(100);

const COMBAT_MESSAGE_PREFIXES: [&str; 12] = [
    "Critical hit",
    "You inflicted",
    "The target Jammed",
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
pub type QuestRewardFilter = Arc<dyn Fn(&str, &[Value], &[Value]) -> Option<Value> + Send + Sync>;

/// A verbatim line observer (the recording controller's seam).
pub type LineTap = Arc<dyn Fn(&str) + Send + Sync>;

fn bus_topic(event_type: EventType) -> Option<Topic> {
    match event_type {
        EventType::DamageDealt
        | EventType::CriticalHit
        | EventType::DamageReceived
        | EventType::TargetDodge
        | EventType::TargetEvade
        | EventType::TargetJam
        | EventType::PlayerDodge
        | EventType::PlayerEvade
        | EventType::PlayerJam
        | EventType::MobMiss
        | EventType::Deflect
        | EventType::SelfHeal => Some(Topic::Combat),
        EventType::Loot => Some(Topic::LootGroup),
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

fn is_internal_buffer_type(event_type: EventType) -> bool {
    event_type == EventType::MissionComplete
}

/// `str(datetime)` for a whole-second instant: the payload timestamp
/// form whose recorder symbol keying matches the backend's.
fn timestamp_string(timestamp: NaiveDateTime) -> String {
    timestamp.format("%Y-%m-%d %H:%M:%S").to_string()
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
}

pub struct ChatlogWatcher {
    shared: Arc<Shared>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl ChatlogWatcher {
    pub fn new(
        bus: Arc<EventBus>,
        chatlog_path: impl Into<PathBuf>,
        quest_reward_filter: Option<QuestRewardFilter>,
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
            }),
            thread: Mutex::new(None),
        }
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
    pub fn wait_until_drained(&self, min_lines: u64, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let (lock, condvar) = &self.shared.idle;
        let mut guard = lock.lock().expect("idle lock");
        while self.lines_seen() < min_lines || self.has_pending_tick() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "chatlog watcher did not drain to {min_lines} line(s) within \
                     {timeout:?} (read {}, pending tick={})",
                    self.lines_seen(),
                    self.has_pending_tick()
                ));
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
            buffer.clear();
            let bytes = reader.read_until(b'\n', &mut buffer)?;
            if bytes > 0 {
                // The original reads text with errors="replace".
                let line = String::from_utf8_lossy(&buffer).into_owned();
                if let Some(tap) = shared.line_tap.lock().expect("line tap").clone() {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tap(&line)));
                }
                process_line(shared, &mut tick, &line);
            } else {
                flush_tick(shared, &mut tick);
                signal_idle(shared);
                std::thread::sleep(TAIL_INTERVAL);
            }
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
    shared.lines_seen_total.fetch_add(1, Ordering::SeqCst);
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

    // Quest-reward suppression.
    let completes: Vec<ChatEvent> = mission_events
        .iter()
        .filter(|e| e.event_type == EventType::MissionComplete)
        .cloned()
        .collect();
    if !completes.is_empty() {
        if let Some(filter) = shared.quest_reward_filter.clone() {
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

                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    filter(&mission_name, &loot_data, &skill_data)
                }))
                .unwrap_or(None);

                if let Some(result) = result {
                    if let Some(index) = result
                        .get("suppress_loot_index")
                        .and_then(Value::as_i64)
                        .filter(|i| *i >= 0 && (*i as usize) < loot_events.len())
                    {
                        loot_events.remove(index as usize);
                    }
                    if let Some(index) = result
                        .get("suppress_skill_index")
                        .and_then(Value::as_i64)
                        .filter(|i| *i >= 0 && (*i as usize) < skill_events.len())
                    {
                        skill_events.remove(index as usize);
                    }
                }
            }
        }
    }

    let refund_matches = match_enhancer_shrapnel(&loot_events, &enhancer_events);

    // Enhancer breaks before loot finalisation.
    for event in &enhancer_events {
        let mut payload = Map::new();
        payload.insert("type".into(), Value::from(event.event_type.as_str()));
        payload.insert(
            "timestamp".into(),
            Value::from(timestamp_string(event.timestamp)),
        );
        for (key, value) in &event.data {
            payload.insert(key.clone(), value.clone());
        }
        shared
            .bus
            .publish(Topic::EnhancerBreak, &Value::Object(payload));
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
            items.push(serde_json::json!({
                "item_name": event.data.get("item_name").cloned().unwrap_or(Value::from("")),
                "quantity": event.data.get("quantity").cloned().unwrap_or(Value::from(1)),
                "value_ped": event.data.get("value").cloned().unwrap_or(Value::from(0.0)),
                "is_enhancer_shrapnel": refund_matches[index],
            }));
            total += value;
        }
        let payload = serde_json::json!({
            "type": EventType::Loot.as_str(),
            "timestamp": tick_ts.map(timestamp_string),
            "items": items,
            "total_ped": eo_wire::normalizer::round_half_even(total, 4),
        });
        shared.bus.publish(Topic::LootGroup, &payload);
    }

    // Mission events.
    for event in &mission_events {
        let Some(topic) = bus_topic(event.event_type) else {
            continue;
        };
        let mut payload = Map::new();
        payload.insert("type".into(), Value::from(event.event_type.as_str()));
        payload.insert(
            "timestamp".into(),
            Value::from(timestamp_string(event.timestamp)),
        );
        for (key, value) in &event.data {
            payload.insert(key.clone(), value.clone());
        }
        shared.bus.publish(topic, &Value::Object(payload));
    }

    // Skill events.
    for event in &skill_events {
        let mut payload = Map::new();
        payload.insert("type".into(), Value::from(event.event_type.as_str()));
        payload.insert(
            "timestamp".into(),
            Value::from(timestamp_string(event.timestamp)),
        );
        for (key, value) in &event.data {
            payload.insert(key.clone(), value.clone());
        }
        shared
            .bus
            .publish(Topic::SkillGain, &Value::Object(payload));
    }

    // Everything else.
    for event in &other_events {
        let Some(topic) = bus_topic(event.event_type) else {
            continue;
        };
        let mut payload = Map::new();
        payload.insert("type".into(), Value::from(event.event_type.as_str()));
        payload.insert(
            "timestamp".into(),
            Value::from(timestamp_string(event.timestamp)),
        );
        for (key, value) in &event.data {
            payload.insert(key.clone(), value.clone());
        }
        shared.bus.publish(topic, &Value::Object(payload));
    }

    // The settled-tick boundary lands last, after every per-event
    // publish above has dispatched synchronously.
    let payload = serde_json::json!({ "timestamp": tick_ts.map(timestamp_string) });
    shared.bus.publish(Topic::TickFlushed, &payload);

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
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("chat_testing.log");
        std::fs::File::create(&log_path).unwrap();
        let bus = Arc::new(EventBus::new());
        let stream = Arc::new(Mutex::new(Vec::new()));
        let sink = stream.clone();
        bus.add_tap(move |topic, data| {
            sink.lock().unwrap().push((topic, data.clone()));
        });
        let watcher = ChatlogWatcher::new(bus.clone(), &log_path, filter);
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
        assert_eq!(loot["timestamp"], "2026-05-19 10:00:02");
        assert_eq!(loot["total_ped"], 6.5);
        assert_eq!(loot["items"][0]["item_name"], "Shrapnel");
        assert_eq!(loot["items"][0]["is_enhancer_shrapnel"], false);
        assert_eq!(loot["items"][1]["quantity"], 1);
        assert_eq!(stream[1].1["timestamp"], "2026-05-19 10:00:02");
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
        let filter: QuestRewardFilter = Arc::new(|mission, loot, skills| {
            assert_eq!(mission, "Iron Challenge");
            assert_eq!(loot.len(), 2);
            assert_eq!(skills.len(), 1);
            Some(serde_json::json!({
                "suppress_loot_index": 1,
                "suppress_skill_index": 0,
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
        pipeline.bus.subscribe(Topic::Combat, move |data| {
            sink.lock().unwrap().push(data.clone());
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
        assert_eq!(received[0]["timestamp"], "2026-05-19 10:00:01");
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
        bus.add_tap(move |topic, data| {
            sink.lock().unwrap().push((topic, data.clone()));
        });
        let watcher = ChatlogWatcher::new(bus.clone(), &log_path, None);
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
        let watcher = ChatlogWatcher::new(bus, "/nonexistent/chat.log", None);
        watcher.start();
        assert!(!watcher.is_running());
    }

    #[test]
    fn out_of_range_suppression_indexes_are_ignored() {
        // Exactly the length and negative, in both polarities across
        // the two runs: out of range either way, nothing suppressed,
        // nothing panics in the tail thread.
        for swap in [false, true] {
            let filter: QuestRewardFilter = Arc::new(move |_, loot, skills| {
                let (loot_index, skill_index) = if swap {
                    (-(1 + loot.len() as i64), skills.len() as i64)
                } else {
                    (loot.len() as i64, -(1 + skills.len() as i64))
                };
                Some(serde_json::json!({
                    "suppress_loot_index": loot_index,
                    "suppress_skill_index": skill_index,
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
        bus.add_tap(move |topic, data| {
            sink.lock().unwrap().push((topic, data.clone()));
        });

        let started = Instant::now();
        {
            let watcher = ChatlogWatcher::new(bus.clone(), &log_path, None);
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
}
