//! Event-stream fingerprint serialiser: Rust port of
//! `FingerprintRecorder.serialize` from the original Python implementation.
//!
//! Given the recorded `(topic, payload)` events of a scenario in publish
//! order, this produces the canonical JSONL the committed `fingerprint.jsonl`
//! golden holds: one compact line per event (`{"payload": <normalised>,
//! "topic": <topic>}`, keys sorted), in publish order, with a trailing newline
//! whenever any events were recorded. The payload is normalised through the
//! shared [`Normalizer`]; the topic is verbatim.

use serde_json::{Map, Value};

use crate::normalizer::{to_python_json, Normalizer};

/// Serialise recorded events as canonical fingerprint JSONL.
///
/// `events` is the publish-order `(topic, payload)` stream; the payloads are
/// the JSON wire form (a `datetime` already reduced to its `isoformat()`, a
/// domain-event envelope already `model_dump(mode="json")`'d), so they
/// normalise identically to the live Python objects. The `normalizer` is
/// threaded in (not owned) so the DB snapshot can continue its symbol table,
/// exactly as `GoldenSet` shares one `Normalizer` across both surfaces.
pub fn serialize_events(events: &[(String, Value)], normalizer: &mut Normalizer) -> String {
    if events.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::with_capacity(events.len());
    for (topic, payload) in events {
        let mut entry = Map::new();
        entry.insert("topic".to_string(), Value::String(topic.clone()));
        entry.insert("payload".to_string(), normalizer.normalize(payload));
        lines.push(to_python_json(&Value::Object(entry), None));
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_event_stream_serialises_to_empty_string() {
        let mut norm = Normalizer::new();
        assert_eq!(serialize_events(&[], &mut norm), "");
    }

    #[test]
    fn events_render_one_sorted_compact_line_each_with_trailing_newline() {
        let mut norm = Normalizer::new();
        let events = vec![
            (
                "session_started".to_string(),
                json!({"session_id": "11111111-1111-1111-1111-111111111111"}),
            ),
            (
                "combat".to_string(),
                json!({"type": "damage_dealt", "amount": 10.5}),
            ),
        ];
        let out = serialize_events(&events, &mut norm);
        assert_eq!(
            out,
            "{\"payload\": {\"session_id\": \"<UUID_1>\"}, \"topic\": \"session_started\"}\n\
             {\"payload\": {\"amount\": 10.5, \"type\": \"damage_dealt\"}, \"topic\": \"combat\"}\n"
        );
    }
}
