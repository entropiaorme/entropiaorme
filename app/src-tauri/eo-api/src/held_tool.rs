//! The tool in hand across the boundary: where it comes from (the hotbar
//! keys, or the game screen) and, while it is the screen, what the screen
//! shows of it, as the overlay displays it.
//!
//! What the screen shows stays a closed vocabulary across the boundary: a
//! tool (from the library, or known only from the game's data), nothing,
//! a name that is not a tracked tool, a bar that does not say which tool
//! (and why), or a bar out of view. Library entries the reader names by
//! id cross by id, name and the label that tells them apart, so the
//! overlay can say them and declare one.

use std::collections::BTreeMap;

use eo_services::bus_events::HotbarItemKind;
use eo_services::db::Db;
use eo_services::game_ui::tool_names::Candidate;
use eo_services::tool_setup::{part_names, SetupPart, SetupRole};
use eo_services::tool_watch::{HeldTool, ToolSeen, Why};
use eo_services::tracker::{HandSetup, HandView, ScreenTool, ToolDisplay, ToolSource};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ApiError, Nullable};

/// Why the keys are the source of the tool in hand while reading it off
/// the screen is turned on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ToolScreenUnread {
    /// The screen stopped being read during the session (capture was
    /// refused or stopped). Tried again when the setting is turned on
    /// anew, or at the next session.
    Stopped,
    /// Screen capture is not running, so the screen was never read this
    /// session. Tried again when the setting is turned off and on.
    NotStarted,
}

/// A library entry, by id and name, with the label the player tells it
/// apart from the others listed with it by (see [`lookalike_labels`]).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolEntry {
    pub id: i64,
    pub name: String,
    pub label: String,
}

/// A tool the name read could be.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolCandidate {
    pub name: String,
    /// The library entry it is, when the library holds exactly one entry
    /// of it in use; null for a tool known only from the game's data, or
    /// one kept in several entries.
    pub library_id: Nullable<i64>,
}

/// Why the bar does not say which tool is in hand.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolUncertainty {
    /// Library entries in use that read the same on the bar.
    Twins { entries: Vec<ToolEntry> },
    /// Library entries the name is, all of them turned off.
    Disabled { entries: Vec<ToolEntry> },
    /// Tools the name read cannot be told apart from, nearest first, and
    /// how many more there are. Library tools of one name are one
    /// candidate, so candidates differ by name.
    Contended {
        candidates: Vec<ToolCandidate>,
        more: i64,
    },
    /// A name read that is near no tool well enough, as read.
    Unknown { text: String },
    /// The bar could not be read.
    Unreadable,
}

/// What one use of a tool is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ToolUse {
    Shot,
    Heal,
    Swing,
}

/// The part an attachment plays in a set-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ToolPartRole {
    Amplifier,
    Scope,
    Absorber,
    Implant,
}

/// One attachment fitted to the tool in hand.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolPart {
    pub role: ToolPartRole,
    pub name: String,
}

/// A weapon's damage enhancers: how many are still fitted, of those it is
/// set up with (each wears out with use).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolEnhancers {
    pub active: i64,
    pub fitted: i64,
}

/// What a use of the tool in hand is priced at now, and the set-up behind
/// the price: for a weapon, as the session prices its shots.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolSetupView {
    /// What one use is.
    pub per: ToolUse,
    /// The parts fitted, in the order a player names them; null where the
    /// declaration did not say (a healer declared by its key while the bar
    /// was unclear).
    pub parts: Nullable<Vec<ToolPart>>,
    /// Null when the weapon is set up with none (or it is not a weapon).
    pub enhancers: Nullable<ToolEnhancers>,
    /// The price of one use, markup included (PED).
    pub cost_per_use_ped: f64,
    /// The TT one use consumes, before markup (PED), where known.
    pub tt_per_use_ped: Nullable<f64>,
}

impl ToolSetupView {
    fn of(setup: HandSetup) -> Self {
        Self {
            per: match setup.kind {
                HotbarItemKind::Healing => ToolUse::Heal,
                HotbarItemKind::Harvesting => ToolUse::Swing,
                HotbarItemKind::Weapon | HotbarItemKind::Consumable => ToolUse::Shot,
            },
            parts: setup
                .parts
                .map(|parts| parts.into_iter().map(ToolPart::of).collect())
                .into(),
            enhancers: setup
                .enhancers
                .map(|enhancers| ToolEnhancers {
                    active: enhancers.active,
                    fitted: enhancers.fitted,
                })
                .into(),
            cost_per_use_ped: setup.cost_per_use.0,
            tt_per_use_ped: setup.tt_per_use.map(|tt| tt.0).into(),
        }
    }
}

impl ToolPart {
    fn of(part: SetupPart) -> Self {
        Self {
            role: match part.role {
                SetupRole::Amplifier => ToolPartRole::Amplifier,
                SetupRole::Scope => ToolPartRole::Scope,
                SetupRole::Absorber => ToolPartRole::Absorber,
                SetupRole::Implant => ToolPartRole::Implant,
            },
            name: part.name,
        }
    }
}

/// The set-up of `hand`, for the wire.
fn setup_of(hand: Option<&HandView>) -> Nullable<ToolSetupView> {
    hand.and_then(|hand| hand.setup.clone())
        .map(ToolSetupView::of)
        .into()
}

/// What the screen shows is in hand.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolHeld {
    /// Nothing read yet: the screen is not the source, or has not been
    /// looked at this session.
    NotRead,
    /// A tool. `provisional`: known only from the game's data, not the
    /// library, and costed as the bare tool. `setup`: what its uses are
    /// priced at, where the reading knows the hand.
    Tool {
        name: String,
        provisional: bool,
        setup: Nullable<ToolSetupView>,
    },
    /// Nothing is held.
    Nothing,
    /// Something the player said is not a tracked tool.
    NotTracked { name: String },
    /// The bar is in view but does not say which tool is in hand.
    Uncertain { why: ToolUncertainty },
    /// The bar is out of view; the tool in hand when it went, if any, is
    /// still taken to be held (`provisional` and `setup` as for a tool).
    NotVisible {
        last: Nullable<String>,
        provisional: bool,
        setup: Nullable<ToolSetupView>,
    },
}

/// Where the tool in hand comes from, as the overlay shows it. While the
/// keys are the source (the setting off, or no session running) only
/// `notReading` says anything, and the snapshot's `currentTool` keeps its
/// meaning.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolSourceView {
    /// The screen is the source of the tool in hand for the running
    /// session.
    pub reading_screen: bool,
    /// Reading the screen is turned on, but the keys are the source, and
    /// why. Null otherwise.
    pub not_reading: Nullable<ToolScreenUnread>,
    /// The tool held is the player's own declaration, standing until the
    /// screen next shows something conclusive.
    pub manual: bool,
    pub held: ToolHeld,
}

impl ToolSourceView {
    /// The keys are the source: nothing read.
    pub fn keys() -> Self {
        Self {
            reading_screen: false,
            not_reading: None.into(),
            manual: false,
            held: ToolHeld::NotRead,
        }
    }

    fn unread(why: ToolScreenUnread) -> Self {
        Self {
            not_reading: Some(why).into(),
            ..Self::keys()
        }
    }
}

/// A stored library row: id, name, kind, set-up (JSON) and whether it is
/// on.
pub(crate) type LibraryRow = (i64, String, String, String, bool);

/// The names of an entry's attachments, in the order a player names them
/// (None where it has none).
fn setup_parts(properties_json: &str) -> [Option<String>; 4] {
    let props: Value = serde_json::from_str(properties_json).unwrap_or(Value::Null);
    part_names(&props)
}

/// How the player tells apart entries the held-tool bar cannot: each
/// entry's name, then, while two still read alike, its amplifier and its
/// next attachment in turn ("Pistol + A101"), and failing all else its
/// place among the ones still alike ("Pistol (2)"). One label per row, in
/// the order given. The one rule every surface that names such entries
/// uses: the overlay's choices, the equipment page's flags, its switches
/// and the hotbar's idle slots.
pub fn lookalike_labels(rows: &[&LibraryRow]) -> Vec<String> {
    let parts: Vec<[Option<String>; 4]> = rows.iter().map(|row| setup_parts(&row.3)).collect();
    let label_at = |index: usize, depth: usize| -> String {
        let mut label = rows[index].1.clone();
        for part in parts[index].iter().take(depth).flatten() {
            label.push_str(" + ");
            label.push_str(part);
        }
        label
    };
    let all_apart = |labels: &[String]| {
        labels
            .iter()
            .enumerate()
            .all(|(i, label)| labels.iter().skip(i + 1).all(|other| other != label))
    };
    let mut labels: Vec<String> = Vec::new();
    for depth in 0..=SetupRole::ALL.len() {
        labels = (0..rows.len())
            .map(|index| label_at(index, depth))
            .collect();
        if all_apart(&labels) {
            return labels;
        }
    }
    // Still alike: number each one among those that read the same.
    let plain = labels.clone();
    for (index, label) in labels.iter_mut().enumerate() {
        let alike: Vec<usize> = (0..plain.len())
            .filter(|other| plain[*other] == plain[index])
            .collect();
        if alike.len() > 1 {
            let place = alike.iter().position(|other| *other == index).unwrap_or(0) + 1;
            label.push_str(&format!(" ({place})"));
        }
    }
    labels
}

/// The labels of every member of `groups` (entry ids, as the library read
/// serves them), by id.
pub(crate) fn group_labels(
    rows: &[LibraryRow],
    groups: &[Vec<String>],
) -> BTreeMap<String, String> {
    let mut labels = BTreeMap::new();
    for group in groups {
        let members: Vec<&LibraryRow> = group
            .iter()
            .filter_map(|id| rows.iter().find(|row| row.0.to_string() == *id))
            .collect();
        for (row, label) in members.iter().zip(lookalike_labels(&members)) {
            labels.insert(row.0.to_string(), label);
        }
    }
    labels
}

/// The library entries `display` names by id alone, whose rows must be
/// read: twins and turned-off entries, and a library tool held that the
/// reading's own hand does not name.
fn ids_to_read(display: &ToolDisplay) -> Vec<i64> {
    let ToolSource::Screen(screen) = &display.source else {
        return Vec::new();
    };
    if screen.manual.is_some() || screen.unseen.is_some() {
        return Vec::new();
    }
    match screen.last.as_ref().map(|last| &last.seen) {
        Some(ToolSeen::Uncertain(Why::Twins { ids } | Why::Disabled { ids })) => ids.clone(),
        Some(ToolSeen::Holding(HeldTool::Library { id }))
            if hand_named(display.hand.as_ref(), *id).is_none() =>
        {
            vec![*id]
        }
        _ => Vec::new(),
    }
}

/// The hand's name when it is library entry `id`.
fn hand_named(hand: Option<&HandView>, id: i64) -> Option<&str> {
    hand.filter(|hand| hand.library_id == Some(id))
        .map(|hand| hand.name.as_str())
}

/// The view of `display`, reading only the library rows it names by id
/// alone (none while a library tool is held and the reading knows it).
pub(crate) async fn tool_source_view(
    db: &Db,
    display: ToolDisplay,
) -> Result<ToolSourceView, ApiError> {
    let mut rows = Vec::new();
    for id in ids_to_read(&display) {
        if let Some(row) = db
            .equipment_row(id)
            .await
            .map_err(ApiError::internal("held tool library read"))?
        {
            rows.push(row);
        }
    }
    Ok(view_of(display, &rows))
}

/// The view of `display`, the library rows it names given.
pub(crate) fn view_of(display: ToolDisplay, rows: &[LibraryRow]) -> ToolSourceView {
    match display.source {
        ToolSource::Keys => ToolSourceView::keys(),
        ToolSource::ScreenStopped { .. } => ToolSourceView::unread(ToolScreenUnread::Stopped),
        ToolSource::ScreenNotStarted => ToolSourceView::unread(ToolScreenUnread::NotStarted),
        ToolSource::Screen(screen) => screen_view(screen, display.hand, display.unseen_hand, rows),
    }
}

fn screen_view(
    screen: ScreenTool,
    hand: Option<HandView>,
    unseen_hand: Option<HandView>,
    rows: &[LibraryRow],
) -> ToolSourceView {
    let view = |manual: bool, held: ToolHeld| ToolSourceView {
        reading_screen: true,
        not_reading: None.into(),
        manual,
        held,
    };
    if let Some(manual) = screen.manual {
        let declared = hand
            .as_ref()
            .filter(|hand| hand.library_id == Some(manual.equipment_id));
        return view(
            true,
            ToolHeld::Tool {
                setup: setup_of(declared),
                name: manual.name,
                provisional: false,
            },
        );
    }
    let covered = |hand: Option<HandView>| ToolHeld::NotVisible {
        provisional: hand.as_ref().is_some_and(HandView::provisional),
        setup: setup_of(hand.as_ref()),
        last: hand.map(|hand| hand.name).into(),
    };
    if screen.unseen.is_some() {
        return view(false, covered(unseen_hand));
    }
    let Some(last) = screen.last else {
        return view(false, ToolHeld::NotRead);
    };
    let held = match last.seen {
        ToolSeen::Holding(HeldTool::Library { id }) => ToolHeld::Tool {
            name: hand_named(hand.as_ref(), id)
                .map(str::to_string)
                .or_else(|| rows.iter().find(|row| row.0 == id).map(|row| row.1.clone()))
                .unwrap_or_default(),
            provisional: false,
            setup: setup_of(hand.as_ref().filter(|hand| hand.library_id == Some(id))),
        },
        ToolSeen::Holding(HeldTool::Bundled { name, .. }) => ToolHeld::Tool {
            setup: setup_of(
                hand.as_ref()
                    .filter(|hand| hand.provisional() && hand.name == name),
            ),
            name,
            provisional: true,
        },
        ToolSeen::NotTracked { name } => ToolHeld::NotTracked { name },
        ToolSeen::Nothing => ToolHeld::Nothing,
        ToolSeen::Uncertain(why) => ToolHeld::Uncertain {
            why: uncertainty(why, rows),
        },
        ToolSeen::NotVisible => covered(unseen_hand.or(hand)),
    };
    view(false, held)
}

/// The rows of `ids` the library still holds, in that order, each with its
/// label among them.
fn entries(ids: Vec<i64>, rows: &[LibraryRow]) -> Vec<ToolEntry> {
    let members: Vec<&LibraryRow> = ids
        .iter()
        .filter_map(|id| rows.iter().find(|row| row.0 == *id))
        .collect();
    members
        .iter()
        .zip(lookalike_labels(&members))
        .map(|(row, label)| ToolEntry {
            id: row.0,
            name: row.1.clone(),
            label,
        })
        .collect()
}

fn uncertainty(why: Why, rows: &[LibraryRow]) -> ToolUncertainty {
    match why {
        Why::Twins { ids } => ToolUncertainty::Twins {
            entries: entries(ids, rows),
        },
        Why::Disabled { ids } => ToolUncertainty::Disabled {
            entries: entries(ids, rows),
        },
        Why::Contended { candidates, more } => ToolUncertainty::Contended {
            candidates: candidates
                .into_iter()
                .map(|candidate| match candidate {
                    Candidate::Library { ids, name } => ToolCandidate {
                        name,
                        library_id: match ids.as_slice() {
                            [id] => Some(*id),
                            _ => None,
                        }
                        .into(),
                    },
                    Candidate::Bundled(tool) => ToolCandidate {
                        name: tool.name,
                        library_id: None.into(),
                    },
                })
                .collect(),
            more: i64::try_from(more).unwrap_or(i64::MAX),
        },
        Why::Unknown { text } => ToolUncertainty::Unknown { text },
        Why::Unreadable => ToolUncertainty::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use eo_services::game_ui::tool_names::BundledTool;
    use eo_services::tracker::{ManualDeclaration, SeenTool, Unseen};

    use super::*;

    fn row(id: i64, name: &str, setup: &str) -> LibraryRow {
        (id, name.into(), "weapon".into(), setup.into(), true)
    }

    fn rows() -> Vec<LibraryRow> {
        vec![
            row(1, "Pistol", r#"{"amp_entity": {"name": "A101"}}"#),
            row(2, "Pistol", "{}"),
            row(4, "Rifle", "{}"),
        ]
    }

    fn library(id: i64, name: &str) -> HandView {
        HandView {
            name: name.into(),
            library_id: Some(id),
            setup: None,
        }
    }

    fn bundled(name: &str) -> HandView {
        HandView {
            name: name.into(),
            library_id: None,
            setup: None,
        }
    }

    fn display(source: ToolSource, hand: Option<HandView>) -> ToolDisplay {
        ToolDisplay {
            source,
            hand,
            unseen_hand: None,
        }
    }

    fn seen(seen: ToolSeen) -> ToolSource {
        ToolSource::Screen(ScreenTool {
            last: Some(SeenTool { seen, at_ms: 1_000 }),
            ..ScreenTool::default()
        })
    }

    fn held(display: ToolDisplay) -> ToolHeld {
        view_of(display, &rows()).held
    }

    #[test]
    fn the_keys_carry_nothing_read() {
        assert_eq!(
            view_of(display(ToolSource::Keys, None), &rows()),
            ToolSourceView::keys()
        );
        let stopped = view_of(display(ToolSource::ScreenStopped { at_ms: 5 }, None), &[]);
        assert!(!stopped.reading_screen);
        assert_eq!(stopped.not_reading, Some(ToolScreenUnread::Stopped));
        assert_eq!(stopped.held, ToolHeld::NotRead);
        let not_started = view_of(display(ToolSource::ScreenNotStarted, None), &[]);
        assert_eq!(not_started.not_reading, Some(ToolScreenUnread::NotStarted));
    }

    #[test]
    fn a_screen_not_yet_looked_at_has_read_nothing() {
        let view = view_of(
            display(
                ToolSource::Screen(ScreenTool::default()),
                Some(library(1, "Pistol")),
            ),
            &rows(),
        );
        assert!(view.reading_screen);
        assert_eq!(view.held, ToolHeld::NotRead);
    }

    #[test]
    fn a_library_tool_is_named_from_the_hand_without_a_library_read() {
        let holding = display(
            seen(ToolSeen::Holding(HeldTool::Library { id: 4 })),
            Some(library(4, "Rifle")),
        );
        assert!(ids_to_read(&holding).is_empty(), "the hand names it");
        assert_eq!(
            view_of(holding, &[]).held,
            ToolHeld::Tool {
                name: "Rifle".into(),
                provisional: false,
                setup: None.into()
            }
        );
        // A hand that is not that entry (not declared yet): its row is read.
        let unnamed = display(seen(ToolSeen::Holding(HeldTool::Library { id: 4 })), None);
        assert_eq!(ids_to_read(&unnamed), vec![4]);
        assert_eq!(
            held(unnamed),
            ToolHeld::Tool {
                name: "Rifle".into(),
                provisional: false,
                setup: None.into()
            }
        );
        assert_eq!(
            held(display(
                seen(ToolSeen::Holding(HeldTool::Bundled {
                    endpoint: "weapons".into(),
                    item_id: "7".into(),
                    name: "Rusty Rifle".into(),
                })),
                Some(bundled("Rusty Rifle")),
            )),
            ToolHeld::Tool {
                name: "Rusty Rifle".into(),
                provisional: true,
                setup: None.into()
            }
        );
    }

    #[test]
    fn nothing_and_not_tracked_say_so() {
        assert_eq!(
            held(display(seen(ToolSeen::Nothing), None)),
            ToolHeld::Nothing
        );
        assert_eq!(
            held(display(
                seen(ToolSeen::NotTracked {
                    name: "Teleport Chip".into()
                }),
                None
            )),
            ToolHeld::NotTracked {
                name: "Teleport Chip".into()
            }
        );
    }

    #[test]
    fn an_unclear_bar_says_why_with_library_entries_told_apart() {
        let twins = display(
            seen(ToolSeen::Uncertain(Why::Twins { ids: vec![1, 2, 3] })),
            None,
        );
        assert_eq!(ids_to_read(&twins), vec![1, 2, 3]);
        // Entry 3 is gone from the library: it is left out.
        assert_eq!(
            held(twins),
            ToolHeld::Uncertain {
                why: ToolUncertainty::Twins {
                    entries: vec![
                        ToolEntry {
                            id: 1,
                            name: "Pistol".into(),
                            label: "Pistol + A101".into()
                        },
                        ToolEntry {
                            id: 2,
                            name: "Pistol".into(),
                            label: "Pistol".into()
                        },
                    ]
                }
            }
        );
        assert_eq!(
            held(display(
                seen(ToolSeen::Uncertain(Why::Disabled { ids: vec![4] })),
                None
            )),
            ToolHeld::Uncertain {
                why: ToolUncertainty::Disabled {
                    entries: vec![ToolEntry {
                        id: 4,
                        name: "Rifle".into(),
                        label: "Rifle".into()
                    }]
                }
            }
        );
        assert_eq!(
            held(display(
                seen(ToolSeen::Uncertain(Why::Contended {
                    candidates: vec![
                        Candidate::Library {
                            ids: vec![1],
                            name: "Pistol".into()
                        },
                        Candidate::Library {
                            ids: vec![4, 5],
                            name: "Kept Twice".into()
                        },
                        Candidate::Bundled(BundledTool {
                            endpoint: "weapons".into(),
                            item_id: "8".into(),
                            name: "Pistol Mk2".into(),
                        }),
                    ],
                    more: 3,
                })),
                None
            )),
            ToolHeld::Uncertain {
                why: ToolUncertainty::Contended {
                    candidates: vec![
                        ToolCandidate {
                            name: "Pistol".into(),
                            library_id: Some(1).into()
                        },
                        ToolCandidate {
                            name: "Kept Twice".into(),
                            library_id: None.into()
                        },
                        ToolCandidate {
                            name: "Pistol Mk2".into(),
                            library_id: None.into()
                        },
                    ],
                    more: 3,
                }
            }
        );
        assert_eq!(
            held(display(
                seen(ToolSeen::Uncertain(Why::Unknown {
                    text: "Pstl".into()
                })),
                None
            )),
            ToolHeld::Uncertain {
                why: ToolUncertainty::Unknown {
                    text: "Pstl".into()
                }
            }
        );
        assert_eq!(
            held(display(seen(ToolSeen::Uncertain(Why::Unreadable)), None)),
            ToolHeld::Uncertain {
                why: ToolUncertainty::Unreadable
            }
        );
    }

    #[test]
    fn an_out_of_view_bar_carries_the_hand_when_it_went_with_its_tone() {
        let covered = ToolSource::Screen(ScreenTool {
            last: Some(SeenTool {
                seen: ToolSeen::NotVisible,
                at_ms: 2_000,
            }),
            unseen: Some(Unseen { since_ms: 2_000 }),
            manual: None,
        });
        let with = |unseen_hand: Option<HandView>| ToolDisplay {
            source: covered.clone(),
            // The readout's guess plays no part.
            hand: Some(library(9, "Something Else")),
            unseen_hand,
        };
        assert_eq!(
            held(with(Some(library(4, "Rifle")))),
            ToolHeld::NotVisible {
                last: Some("Rifle".to_string()).into(),
                provisional: false,
                setup: None.into()
            }
        );
        assert_eq!(
            held(with(Some(bundled("Rusty Rifle")))),
            ToolHeld::NotVisible {
                last: Some("Rusty Rifle".to_string()).into(),
                provisional: true,
                setup: None.into()
            }
        );
        assert_eq!(
            held(with(None)),
            ToolHeld::NotVisible {
                last: None.into(),
                provisional: false,
                setup: None.into()
            }
        );
        assert!(ids_to_read(&with(None)).is_empty());
    }

    #[test]
    fn a_standing_declaration_is_the_tool_held_and_says_it_was_chosen() {
        let view = view_of(
            display(
                ToolSource::Screen(ScreenTool {
                    last: Some(SeenTool {
                        seen: ToolSeen::Uncertain(Why::Twins { ids: vec![1, 2] }),
                        at_ms: 2_000,
                    }),
                    unseen: None,
                    manual: Some(ManualDeclaration {
                        equipment_id: 1,
                        name: "Pistol".into(),
                        at_ms: 2_500,
                    }),
                }),
                None,
            ),
            &rows(),
        );
        assert!(view.manual);
        assert_eq!(
            view.held,
            ToolHeld::Tool {
                name: "Pistol".into(),
                provisional: false,
                setup: None.into()
            }
        );
    }

    #[test]
    fn lookalikes_are_told_apart_by_set_up_then_by_place() {
        let one = row(1, "Pistol", r#"{"amp_entity": {"name": "A101"}}"#);
        let two = row(2, "Pistol", "{}");
        assert_eq!(lookalike_labels(&[&one, &two]), ["Pistol + A101", "Pistol"]);
        // Names that differ already need nothing more.
        let mk1 = row(3, "Long Rifle Mk1", r#"{"amp_entity": {"name": "A101"}}"#);
        let mk2 = row(4, "Long Rifle Mk2", "{}");
        assert_eq!(
            lookalike_labels(&[&mk1, &mk2]),
            ["Long Rifle Mk1", "Long Rifle Mk2"]
        );
        // One amplifier on both: the next attachment that differs.
        let scoped = row(
            5,
            "Pistol",
            r#"{"amp_entity": {"name": "A101"}, "scope_entity": {"name": "Scope X"}}"#,
        );
        assert_eq!(
            lookalike_labels(&[&one, &scoped]),
            ["Pistol + A101", "Pistol + A101 + Scope X"]
        );
        // Alike in every part: by place.
        let same = row(6, "Pistol", r#"{"amp_entity": {"name": "A101"}}"#);
        assert_eq!(
            lookalike_labels(&[&one, &same, &two]),
            ["Pistol + A101 (1)", "Pistol + A101 (2)", "Pistol"]
        );
        assert!(lookalike_labels(&[]).is_empty());
    }

    #[test]
    fn group_labels_name_every_member_by_id() {
        let labels = group_labels(&rows(), &[vec!["1".into(), "2".into()]]);
        assert_eq!(labels.get("1").map(String::as_str), Some("Pistol + A101"));
        assert_eq!(labels.get("2").map(String::as_str), Some("Pistol"));
        assert!(!labels.contains_key("4"));
    }

    #[test]
    fn the_wire_shape_is_tagged_and_camel_cased() {
        let view = view_of(
            display(
                seen(ToolSeen::Uncertain(Why::Contended {
                    candidates: vec![Candidate::Library {
                        ids: vec![1],
                        name: "Pistol".into(),
                    }],
                    more: 0,
                })),
                None,
            ),
            &rows(),
        );
        assert_eq!(
            serde_json::to_value(&view).unwrap(),
            serde_json::json!({
                "readingScreen": true,
                "notReading": null,
                "manual": false,
                "held": {
                    "kind": "uncertain",
                    "why": {
                        "kind": "contended",
                        "candidates": [{"name": "Pistol", "libraryId": 1}],
                        "more": 0
                    }
                }
            })
        );
        assert_eq!(
            serde_json::to_value(ToolSourceView::unread(ToolScreenUnread::NotStarted)).unwrap()
                ["notReading"],
            "notStarted"
        );
    }
}
