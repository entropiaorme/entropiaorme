//! Parse Entropia Universe chat.log lines into app events, ported from
//! the original Python implementation.
//!
//! The grammar is a rule table over timestamped lines: a system-message
//! family (combat, heals, loot, skill gains, enhancer breaks, missions)
//! and a globals family (kill and item globals, with their
//! Hall-of-Fame variants matched first). One original pattern uses a
//! negative lookahead, reproduced here as an explicit post-match
//! suffix check so the plain regular-expression engine suffices for
//! every first-match case (a pathological line that would need the
//! original's backtracking past the lookahead yields no event here).
//! Timestamps or numeric captures whose conversions differ from the
//! original's (impossible dates, malformed or Unicode-digit numbers
//! inside shape-matching lines, exotic HTML entities) yield no event
//! or decode narrowly instead; the divergence register covers the
//! classes, none of which a client-written line produces.

use std::sync::OnceLock;

use chrono::{Datelike, NaiveDateTime, Timelike};
use regex::{Captures, Regex};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    DamageDealt,
    CriticalHit,
    TargetDodge,
    TargetEvade,
    TargetJam,
    DamageReceived,
    PlayerDodge,
    PlayerEvade,
    PlayerJam,
    MobMiss,
    Deflect,
    SelfHeal,
    Loot,
    SkillGain,
    EnhancerBreak,
    GlobalKill,
    HofKill,
    GlobalItem,
    HofItem,
    MissionComplete,
    MissionReceived,
}

impl EventType {
    /// Every variant, for exhaustive coverage checks (the corpus
    /// differential asserts that every class is driven by some corpus
    /// scenario). Kept in step with the compiler-exhaustive `as_str`
    /// match below; the `all_lists_every_variant_once` test guards it
    /// against drift.
    pub const ALL: [EventType; 21] = [
        EventType::DamageDealt,
        EventType::CriticalHit,
        EventType::TargetDodge,
        EventType::TargetEvade,
        EventType::TargetJam,
        EventType::DamageReceived,
        EventType::PlayerDodge,
        EventType::PlayerEvade,
        EventType::PlayerJam,
        EventType::MobMiss,
        EventType::Deflect,
        EventType::SelfHeal,
        EventType::Loot,
        EventType::SkillGain,
        EventType::EnhancerBreak,
        EventType::GlobalKill,
        EventType::HofKill,
        EventType::GlobalItem,
        EventType::HofItem,
        EventType::MissionComplete,
        EventType::MissionReceived,
    ];

    /// The wire value, matching the backend enum's `.value` strings.
    pub fn as_str(self) -> &'static str {
        match self {
            EventType::DamageDealt => "damage_dealt",
            EventType::CriticalHit => "critical_hit",
            EventType::TargetDodge => "target_dodge",
            EventType::TargetEvade => "target_evade",
            EventType::TargetJam => "target_jam",
            EventType::DamageReceived => "damage_received",
            EventType::PlayerDodge => "player_dodge",
            EventType::PlayerEvade => "player_evade",
            EventType::PlayerJam => "player_jam",
            EventType::MobMiss => "mob_miss",
            EventType::Deflect => "deflect",
            EventType::SelfHeal => "self_heal",
            EventType::Loot => "loot",
            EventType::SkillGain => "skill_gain",
            EventType::EnhancerBreak => "enhancer_break",
            EventType::GlobalKill => "global_kill",
            EventType::HofKill => "hof_kill",
            EventType::GlobalItem => "global_item",
            EventType::HofItem => "hof_item",
            EventType::MissionComplete => "mission_complete",
            EventType::MissionReceived => "mission_received",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatEvent {
    pub event_type: EventType,
    pub timestamp: NaiveDateTime,
    pub data: Map<String, Value>,
    pub raw_line: String,
}

/// An extractor builds the event's data map from the regex captures;
/// None mirrors a conversion the original would crash on.
type Extractor = fn(&Captures) -> Option<Map<String, Value>>;

struct Rule {
    event_type: EventType,
    pattern: Regex,
    extract: Extractor,
    prefix: Option<&'static str>,
}

impl Rule {
    fn matches(&self, text: &str) -> Option<(EventType, Map<String, Value>)> {
        if let Some(prefix) = self.prefix {
            if !text.starts_with(prefix) {
                return None;
            }
        }
        let captures = self.pattern.captures(text)?;
        Some((self.event_type, (self.extract)(&captures)?))
    }
}

const SYSTEM_MARKER: &str = "[System] []";
const GLOBALS_MARKER: &str = "[Globals]";

fn float_group(captures: &Captures, group: usize) -> Option<f64> {
    captures.get(group)?.as_str().parse::<f64>().ok()
}

fn amount_data(captures: &Captures) -> Option<Map<String, Value>> {
    let mut data = Map::new();
    data.insert("amount".into(), Value::from(float_group(captures, 1)?));
    Some(data)
}

fn empty_data(_captures: &Captures) -> Option<Map<String, Value>> {
    Some(Map::new())
}

fn skill_gain_data(captures: &Captures) -> Option<Map<String, Value>> {
    let mut data = Map::new();
    data.insert("amount".into(), Value::from(float_group(captures, 1)?));
    data.insert(
        "skill_name".into(),
        Value::from(captures.get(2)?.as_str().trim()),
    );
    Some(data)
}

fn improved_skill_data(captures: &Captures) -> Option<Map<String, Value>> {
    let mut data = Map::new();
    data.insert("amount".into(), Value::from(float_group(captures, 2)?));
    data.insert("skill_name".into(), Value::from(captures.get(1)?.as_str()));
    Some(data)
}

fn mission_data(captures: &Captures) -> Option<Map<String, Value>> {
    let mut data = Map::new();
    data.insert(
        "mission_name".into(),
        Value::from(captures.get(1)?.as_str().trim()),
    );
    Some(data)
}

fn enhancer_data(captures: &Captures) -> Option<Map<String, Value>> {
    let mut data = Map::new();
    data.insert(
        "enhancer_name".into(),
        Value::from(captures.get(1)?.as_str()),
    );
    data.insert("item_name".into(), Value::from(captures.get(2)?.as_str()));
    data.insert(
        "remaining".into(),
        Value::from(captures.get(3)?.as_str().parse::<i64>().ok()?),
    );
    data.insert(
        "shrapnel_ped".into(),
        Value::from(float_group(captures, 4)?),
    );
    Some(data)
}

fn global_kill_data(captures: &Captures) -> Option<Map<String, Value>> {
    let mut data = Map::new();
    data.insert("player".into(), Value::from(captures.get(1)?.as_str()));
    data.insert("creature".into(), Value::from(captures.get(2)?.as_str()));
    data.insert("value".into(), Value::from(float_group(captures, 3)?));
    Some(data)
}

fn global_item_data(captures: &Captures) -> Option<Map<String, Value>> {
    let mut data = Map::new();
    data.insert("player".into(), Value::from(captures.get(1)?.as_str()));
    data.insert("item".into(), Value::from(captures.get(2)?.as_str()));
    data.insert("value".into(), Value::from(float_group(captures, 3)?));
    Some(data)
}

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("the rule patterns are compile-time constants")
}

fn system_rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        vec![
            Rule {
                event_type: EventType::CriticalHit,
                pattern: regex(
                    r"Critical hit - Additional damage! You inflicted ([\d.]+) points of damage",
                ),
                extract: amount_data,
                prefix: Some("Critical hit"),
            },
            Rule {
                event_type: EventType::DamageDealt,
                pattern: regex(r"You inflicted ([\d.]+) points of damage"),
                extract: amount_data,
                prefix: Some("You inflicted"),
            },
            Rule {
                event_type: EventType::TargetJam,
                pattern: regex(r"The target Jammed your attack"),
                extract: empty_data,
                prefix: Some("The target Jammed"),
            },
            Rule {
                event_type: EventType::TargetDodge,
                pattern: regex(r"The target Dodged your attack"),
                extract: empty_data,
                prefix: Some("The target Dodged"),
            },
            Rule {
                event_type: EventType::TargetEvade,
                pattern: regex(r"The target Evaded your attack"),
                extract: empty_data,
                prefix: Some("The target Evaded"),
            },
            Rule {
                event_type: EventType::DamageReceived,
                pattern: regex(r"You took ([\d.]+) points of damage"),
                extract: amount_data,
                prefix: Some("You took"),
            },
            Rule {
                event_type: EventType::Deflect,
                pattern: regex(r"Damage deflected!"),
                extract: empty_data,
                prefix: Some("Damage deflected"),
            },
            Rule {
                event_type: EventType::PlayerEvade,
                pattern: regex(r"You Evaded the attack"),
                extract: empty_data,
                prefix: Some("You Evaded"),
            },
            Rule {
                event_type: EventType::PlayerDodge,
                pattern: regex(r"You Dodged the attack"),
                extract: empty_data,
                prefix: Some("You Dodged"),
            },
            Rule {
                event_type: EventType::PlayerJam,
                pattern: regex(r"You Jammed the attack"),
                extract: empty_data,
                prefix: Some("You Jammed"),
            },
            Rule {
                event_type: EventType::MobMiss,
                pattern: regex(r"The attack missed you"),
                extract: empty_data,
                prefix: Some("The attack missed"),
            },
            Rule {
                event_type: EventType::SelfHeal,
                pattern: regex(r"You healed yourself ([\d.]+) points"),
                extract: amount_data,
                prefix: Some("You healed"),
            },
            Rule {
                event_type: EventType::EnhancerBreak,
                pattern: regex(concat!(
                    r"Your enhancer (.+?) on your (.+?) broke\. ",
                    r"You have (\d+) enhancers remaining on the item\. ",
                    r"You received ([\d.]+) PED Shrapnel\.\s*",
                )),
                extract: enhancer_data,
                prefix: Some("Your enhancer"),
            },
            Rule {
                event_type: EventType::MissionComplete,
                pattern: regex(r"^Mission completed \((.+)\)$"),
                extract: mission_data,
                prefix: Some("Mission completed"),
            },
            Rule {
                event_type: EventType::MissionReceived,
                pattern: regex(r"^New Mission received \((.+)\)$"),
                extract: mission_data,
                prefix: Some("New Mission received"),
            },
            Rule {
                event_type: EventType::SkillGain,
                pattern: regex(r"^You have gained ([\d.]+) experience in your (.+) skill$"),
                extract: skill_gain_data,
                prefix: Some("You have gained"),
            },
            Rule {
                event_type: EventType::SkillGain,
                pattern: regex(r"^You have gained ([\d.]+) ([A-Z][A-Za-z ]+)$"),
                extract: skill_gain_data,
                prefix: Some("You have gained"),
            },
            Rule {
                event_type: EventType::SkillGain,
                pattern: regex(r"^Your ([A-Z][a-z]+) has improved by ([\d.]+)$"),
                extract: improved_skill_data,
                prefix: Some("Your "),
            },
        ]
    })
}

struct GlobalRule {
    rule: Rule,
    /// The original's negative lookahead, as a forbidden suffix at the
    /// match end.
    forbidden_suffix: Option<&'static str>,
}

fn global_rules() -> &'static [GlobalRule] {
    static RULES: OnceLock<Vec<GlobalRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        vec![
            GlobalRule {
                rule: Rule {
                    event_type: EventType::HofKill,
                    pattern: regex(concat!(
                        r"\[Globals\] \[\] (.+?) killed a creature \((.+?)\) with a value of ([\d.]+) PED! ",
                        r"A record has been added to the Hall of Fame!",
                    )),
                    extract: global_kill_data,
                    prefix: None,
                },
                forbidden_suffix: None,
            },
            GlobalRule {
                rule: Rule {
                    event_type: EventType::GlobalKill,
                    pattern: regex(
                        r"\[Globals\] \[\] (.+?) killed a creature \((.+?)\) with a value of ([\d.]+) PED!",
                    ),
                    extract: global_kill_data,
                    prefix: None,
                },
                forbidden_suffix: None,
            },
            GlobalRule {
                rule: Rule {
                    event_type: EventType::HofItem,
                    pattern: regex(concat!(
                        r"\[Globals\] \[\] (.+?) has found a rare item \((.+?)\) with a value of ([\d.]+) PE[CD]! ",
                        r"A record has been added to the Hall of Fame!",
                    )),
                    extract: global_item_data,
                    prefix: None,
                },
                forbidden_suffix: None,
            },
            GlobalRule {
                rule: Rule {
                    event_type: EventType::GlobalItem,
                    pattern: regex(
                        r"\[Globals\] \[\] (.+?) has found a rare item \((.+?)\) with a value of ([\d.]+) PE[CD]!",
                    ),
                    extract: global_item_data,
                    prefix: None,
                },
                forbidden_suffix: Some(" A record"),
            },
        ]
    })
}

fn line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| regex(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}) (.+)$"))
}

fn quantity_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // A real stacked-item count is >= 1 with no leading zeros; `[1-9]\d*`
    // rejects `x (0)` and `x (007)` forms, which then keep their literal
    // name with quantity 1 rather than splitting to a 0 / leading-zero
    // count. Kept in lockstep with the Python QUANTITY_RE.
    RE.get_or_init(|| regex(r"^(.+?)\s+x\s+\(([1-9]\d*)\)$"))
}

fn loot_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| regex(r"\[System\] \[\] You received (.+?) Value: ([\d.]+) PED"))
}

/// Parse one chat.log line into an event, or None when it carries
/// nothing the tracker consumes.
pub fn parse_line(line: &str) -> Option<ChatEvent> {
    let trimmed = line.trim_matches(crate::mob_lookup_service::python_whitespace);
    let captures = line_re().captures(trimmed)?;

    let timestamp =
        NaiveDateTime::parse_from_str(captures.get(1)?.as_str(), "%Y-%m-%d %H:%M:%S").ok()?;
    // chrono admits two stamps the original's parser rejects: year zero
    // and leap seconds. Reject both so the no-event leg stays a strict
    // superset of the original's crash leg.
    if timestamp.year() == 0 || timestamp.nanosecond() >= 1_000_000_000 {
        return None;
    }
    let raw_content = captures.get(2)?.as_str();
    let content: String = if raw_content.contains('&') {
        html_escape::decode_html_entities(raw_content).into_owned()
    } else {
        raw_content.to_string()
    };

    if content.contains(SYSTEM_MARKER) {
        return parse_system(timestamp, &content, line);
    }
    if content.contains(GLOBALS_MARKER) {
        return parse_global(timestamp, &content, line);
    }
    None
}

fn parse_system(timestamp: NaiveDateTime, content: &str, raw_line: &str) -> Option<ChatEvent> {
    let message = message_of(content);
    if let Some(captures) = loot_re().captures(content) {
        return Some(ChatEvent {
            event_type: EventType::Loot,
            timestamp,
            data: loot_data(&captures)?,
            raw_line: raw_line.to_string(),
        });
    }
    for rule in system_rules() {
        if let Some((event_type, data)) = rule.matches(message) {
            return Some(ChatEvent {
                event_type,
                timestamp,
                data,
                raw_line: raw_line.to_string(),
            });
        }
    }
    None
}

fn parse_global(timestamp: NaiveDateTime, content: &str, raw_line: &str) -> Option<ChatEvent> {
    for global in global_rules() {
        let Some(captures) = (global.rule.pattern).captures(content) else {
            continue;
        };
        if let Some(forbidden) = global.forbidden_suffix {
            let end = captures.get(0).map(|m| m.end()).unwrap_or(0);
            if content[end..].starts_with(forbidden) {
                continue;
            }
        }
        let data = (global.rule.extract)(&captures)?;
        return Some(ChatEvent {
            event_type: global.rule.event_type,
            timestamp,
            data,
            raw_line: raw_line.to_string(),
        });
    }
    None
}

/// `content.split("] ", 2)`: the message body after the channel and
/// speaker brackets, or the whole content when the shape differs.
fn message_of(content: &str) -> &str {
    let parts: Vec<&str> = content.splitn(3, "] ").collect();
    if parts.len() == 3 {
        parts[2]
    } else {
        content
    }
}

fn loot_data(captures: &Captures) -> Option<Map<String, Value>> {
    let raw_name = captures.get(1)?.as_str().trim();
    let value = float_group(captures, 2)?;
    let mut data = Map::new();
    match quantity_re().captures(raw_name) {
        None => {
            data.insert("item_name".into(), Value::from(raw_name));
            data.insert("quantity".into(), Value::from(1));
            data.insert("value".into(), Value::from(value));
        }
        Some(quantity) => {
            data.insert(
                "item_name".into(),
                Value::from(quantity.get(1)?.as_str().trim()),
            );
            data.insert(
                "quantity".into(),
                Value::from(quantity.get(2)?.as_str().parse::<i64>().ok()?),
            );
            data.insert("value".into(), Value::from(value));
        }
    }
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> ChatEvent {
        parse_line(line).expect("the line parses")
    }

    #[test]
    fn all_lists_every_variant_once() {
        use std::collections::BTreeSet;
        // Each `EventType::ALL` entry maps to a distinct wire string, so
        // ALL has no duplicates or omitted-then-aliased entries. The
        // `as_str` match is compiler-exhaustive, so adding a variant
        // forces a new arm there; this count tripwire forces ALL (and
        // the corpus 21/21 coverage assertion) to grow alongside it.
        let names: BTreeSet<&str> = EventType::ALL.iter().map(|e| e.as_str()).collect();
        assert_eq!(names.len(), EventType::ALL.len());
        assert_eq!(names.len(), 21);
    }

    #[test]
    fn combat_lines_carry_amounts_and_flavours() {
        let event = parse("2026-05-19 10:00:00 [System] [] You inflicted 10.5 points of damage");
        assert_eq!(event.event_type, EventType::DamageDealt);
        assert_eq!(event.data["amount"], 10.5);
        assert_eq!(
            event.timestamp,
            NaiveDateTime::parse_from_str("2026-05-19 10:00:00", "%Y-%m-%d %H:%M:%S").unwrap()
        );

        let event = parse(
            "2026-05-19 10:00:00 [System] [] Critical hit - Additional damage! You inflicted 31.5 points of damage",
        );
        assert_eq!(event.event_type, EventType::CriticalHit);
        assert_eq!(event.data["amount"], 31.5);

        for (line, expected) in [
            ("The target Dodged your attack", EventType::TargetDodge),
            ("The target Evaded your attack", EventType::TargetEvade),
            ("The target Jammed your attack", EventType::TargetJam),
            ("You Dodged the attack", EventType::PlayerDodge),
            ("You Evaded the attack", EventType::PlayerEvade),
            ("You Jammed the attack", EventType::PlayerJam),
            ("The attack missed you", EventType::MobMiss),
            ("Damage deflected!", EventType::Deflect),
        ] {
            let event = parse(&format!("2026-05-19 10:00:00 [System] [] {line}"));
            assert_eq!(event.event_type, expected, "{line}");
            assert!(event.data.is_empty());
        }

        let event = parse("2026-05-19 10:00:01 [System] [] You took 7.2 points of damage");
        assert_eq!(event.event_type, EventType::DamageReceived);
        let event = parse("2026-05-19 10:00:01 [System] [] You healed yourself 12.0 points");
        assert_eq!(event.event_type, EventType::SelfHeal);
        assert_eq!(event.data["amount"], 12.0);
    }

    #[test]
    fn loot_lines_split_quantity_names() {
        let event =
            parse("2026-05-19 10:00:02 [System] [] You received Shrapnel x (500) Value: 5.00 PED");
        assert_eq!(event.event_type, EventType::Loot);
        assert_eq!(event.data["item_name"], "Shrapnel");
        assert_eq!(event.data["quantity"], 500);
        assert_eq!(event.data["value"], 5.0);

        let event =
            parse("2026-05-19 10:00:02 [System] [] You received Animal Muscle Oil Value: 0.12 PED");
        assert_eq!(event.data["item_name"], "Animal Muscle Oil");
        assert_eq!(event.data["quantity"], 1);
        let keys: Vec<&String> = event.data.keys().collect();
        assert_eq!(keys, ["item_name", "quantity", "value"]);
    }

    #[test]
    fn zero_and_leading_zero_quantities_keep_literal_name() {
        // A 0 / leading-zero count is not a real stack size; the line
        // keeps its literal item name with quantity 1 rather than
        // splitting "x (0)" / "x (007)" into a 0 / 7 count.
        let event =
            parse("2026-05-19 10:00:02 [System] [] You received Token x (0) Value: 1.00 PED");
        assert_eq!(event.event_type, EventType::Loot);
        assert_eq!(event.data["item_name"], "Token x (0)");
        assert_eq!(event.data["quantity"], 1);

        let event =
            parse("2026-05-19 10:00:02 [System] [] You received Token x (007) Value: 1.00 PED");
        assert_eq!(event.data["item_name"], "Token x (007)");
        assert_eq!(event.data["quantity"], 1);

        // A genuine count (>= 1, no leading zero) still splits.
        let event =
            parse("2026-05-19 10:00:02 [System] [] You received Token x (12) Value: 1.00 PED");
        assert_eq!(event.data["item_name"], "Token");
        assert_eq!(event.data["quantity"], 12);
    }

    #[test]
    fn skill_gain_variants_parse_in_rule_order() {
        let event = parse(
            "2026-05-19 10:00:03 [System] [] You have gained 0.5874 experience in your Rifle skill",
        );
        assert_eq!(event.event_type, EventType::SkillGain);
        assert_eq!(event.data["skill_name"], "Rifle");
        assert_eq!(event.data["amount"], 0.5874);

        let event = parse("2026-05-19 10:00:03 [System] [] You have gained 0.21 Combat Reflexes");
        assert_eq!(event.data["skill_name"], "Combat Reflexes");

        let event = parse("2026-05-19 10:00:03 [System] [] Your Agility has improved by 0.07");
        assert_eq!(event.data["skill_name"], "Agility");
        assert_eq!(event.data["amount"], 0.07);
    }

    #[test]
    fn enhancer_breaks_extract_all_fields() {
        let event = parse(
            "2026-05-19 10:00:04 [System] [] Your enhancer Weapon Damage Enhancer 3 on your ArMatrix LR-35 broke. You have 7 enhancers remaining on the item. You received 0.8000 PED Shrapnel. ",
        );
        assert_eq!(event.event_type, EventType::EnhancerBreak);
        assert_eq!(event.data["enhancer_name"], "Weapon Damage Enhancer 3");
        assert_eq!(event.data["item_name"], "ArMatrix LR-35");
        assert_eq!(event.data["remaining"], 7);
        assert_eq!(event.data["shrapnel_ped"], 0.8);
    }

    #[test]
    fn missions_match_anchored_shapes() {
        let event = parse("2026-05-19 10:00:05 [System] [] Mission completed (Iron Challenge)");
        assert_eq!(event.event_type, EventType::MissionComplete);
        assert_eq!(event.data["mission_name"], "Iron Challenge");
        let event = parse("2026-05-19 10:00:05 [System] [] New Mission received (Daily Hunting)");
        assert_eq!(event.event_type, EventType::MissionReceived);
    }

    #[test]
    fn globals_split_hof_from_plain_with_the_suffix_check() {
        let event = parse(
            "2026-05-19 10:00:06 [Globals] [] Hunter Dude killed a creature (Atrox Young) with a value of 56 PED!",
        );
        assert_eq!(event.event_type, EventType::GlobalKill);
        assert_eq!(event.data["creature"], "Atrox Young");
        assert_eq!(event.data["value"], 56.0);

        let event = parse(
            "2026-05-19 10:00:06 [Globals] [] Hunter Dude killed a creature (Atrox Old) with a value of 300 PED! A record has been added to the Hall of Fame!",
        );
        assert_eq!(event.event_type, EventType::HofKill);

        let event = parse(
            "2026-05-19 10:00:06 [Globals] [] Lucky Finder has found a rare item (Holy Grail) with a value of 90 PED!",
        );
        assert_eq!(event.event_type, EventType::GlobalItem);
        assert_eq!(event.data["player"], "Lucky Finder");
        assert_eq!(event.data["item"], "Holy Grail");
        assert_eq!(event.data["value"], 90.0);

        let event = parse(
            "2026-05-19 10:00:06 [Globals] [] Lucky Finder has found a rare item (Holy Grail) with a value of 90 PED! A record has been added to the Hall of Fame!",
        );
        assert_eq!(event.event_type, EventType::HofItem);
    }

    #[test]
    fn entities_unescape_only_when_an_ampersand_appears() {
        let event = parse(
            "2026-05-19 10:00:07 [System] [] You received Brown &amp; Gold Paint Value: 0.30 PED",
        );
        assert_eq!(event.data["item_name"], "Brown & Gold Paint");
    }

    #[test]
    fn non_event_lines_yield_none() {
        assert!(parse_line("not a chat line").is_none());
        assert!(parse_line("2026-05-19 10:00:00 [Local] [] hello world").is_none());
        assert!(parse_line("2026-05-19 10:00:00 [System] [] something unrecognised").is_none());
        // An impossible date inside a matching shape yields no event
        // where the original would crash; the register covers this.
        assert!(
            parse_line("2026-13-45 10:00:00 [System] [] You took 1.0 points of damage").is_none()
        );
    }

    #[test]
    fn wire_values_match_the_backend_enum() {
        let expected = [
            (EventType::DamageDealt, "damage_dealt"),
            (EventType::CriticalHit, "critical_hit"),
            (EventType::TargetDodge, "target_dodge"),
            (EventType::TargetEvade, "target_evade"),
            (EventType::TargetJam, "target_jam"),
            (EventType::DamageReceived, "damage_received"),
            (EventType::PlayerDodge, "player_dodge"),
            (EventType::PlayerEvade, "player_evade"),
            (EventType::PlayerJam, "player_jam"),
            (EventType::MobMiss, "mob_miss"),
            (EventType::Deflect, "deflect"),
            (EventType::SelfHeal, "self_heal"),
            (EventType::Loot, "loot"),
            (EventType::SkillGain, "skill_gain"),
            (EventType::EnhancerBreak, "enhancer_break"),
            (EventType::GlobalKill, "global_kill"),
            (EventType::HofKill, "hof_kill"),
            (EventType::GlobalItem, "global_item"),
            (EventType::HofItem, "hof_item"),
            (EventType::MissionComplete, "mission_complete"),
            (EventType::MissionReceived, "mission_received"),
        ];
        for (event_type, wire) in expected {
            assert_eq!(event_type.as_str(), wire);
        }
    }
}
