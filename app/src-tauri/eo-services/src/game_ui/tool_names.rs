//! Resolving the name read off the held-tool bar to a tool.
//!
//! A name read off the bar is matched against three lists, in order: the
//! player's equipment library, the names the player has dismissed as not
//! a tracked tool, and the tools the bundled game data knows a player can
//! hold.
//!
//! A read is compared on its letters and digits alone, case-folded (see
//! [`label_key`]): the recogniser may drop the spaces between words, and
//! commas and case never decide a match. A name the client cut short is
//! compared against the start of each candidate, no further than the read
//! runs; the client cuts only long names, so a read marked cut short with
//! too short a key names nothing. The recogniser's confidence plays no
//! part.
//!
//! The answer decides which tool a player's shots are costed to, so a
//! wrong single answer is the failure to avoid above all: when a read
//! cannot be told for one tool, the answer says so.
//!
//! - **The library** answers a read that spells one of its tools
//!   outright, whatever the other lists hold: the player's own library is
//!   taken as the truth. A read only near a library tool is that tool when
//!   it reaches the library's floor and leads, by the library's margin,
//!   every other tool by another name: of the library, of the dismissed
//!   names and of the bundled list (the bundled tool of the library tool's
//!   own name is that tool, not a rival). Of entries of one name, or the
//!   entries a cut read spells alike, the enabled ones are the answer;
//!   tools of different names the read is only near are never decided by
//!   which is enabled, but reported as contended.
//! - **A dismissed name** answers likewise, by its own floor and margin,
//!   when no library tool does.
//! - **The bundled list** names a tool the player does not own only when
//!   the read spells it exactly, and no other: an item missing from the
//!   bundled data is as near a listed neighbour as a misread of that
//!   neighbour is, so a near read is only ever offered, as a contention,
//!   nearest first.
//! - A read that is not cut short and whose key is the start of a
//!   candidate's, or the other way about (`... Ancient` and
//!   `... Ancient (L)`), is another name than that candidate: it is never
//!   the single answer, only a candidate of a contention.
//!
//! A known limit: a cut read that spells the start of a library tool is
//! that tool, although tools the player does not own may show the same
//! start on the bar.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::fuzzy_match::ratio;
use crate::game_data_store::GameDataStore;

use super::field::{key_counts, label_key, similarity_bound, too_few_letters, KeyCounts};
use super::tool_bar::ToolName;

/// The bundled game data's collections whose entities are tools a
/// player holds in hand, as the tool bar names them.
pub const TOOL_ENDPOINTS: [&str; 3] = ["harvesting_tools", "medical_tools", "weapons"];

/// How strictly a name read off the tool bar must match a known tool, as
/// the profile describes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolNameProfile {
    /// A read with fewer letters than this names no tool, unless it
    /// spells one exactly (a model number such as `M2100` has one letter).
    pub min_letters: usize,
    /// A read marked cut short with a key shorter than this names no tool.
    /// The client cuts only names too long for the bar: the bundled names
    /// it cuts show 21 to 29 letters and digits in the thirty-one
    /// characters it leaves, so this sits a little below 21, for a
    /// character or two the recogniser drops.
    pub min_cut_key: usize,
    /// A name cut short is compared with the start of each candidate as
    /// long as the read, and as many characters either side of that.
    pub cut_tolerance: usize,
    /// How near a read must come to an entry of the player's library.
    pub library: TierProfile,
    /// How near a read must come to a name the player has dismissed.
    pub dismissed: TierProfile,
    /// How near a read must come to a tool of the bundled game data to be
    /// offered as a candidate.
    pub bundled: BundledProfile,
    /// The most tools a contention lists.
    pub max_candidates: usize,
    /// Two library entries whose names are both longer than this many
    /// characters, and agree in letters and digits over this many, show
    /// alike on the bar. The client cuts by width, leaving about
    /// thirty-one characters; this sits below that, so that entries are
    /// flagged too readily rather than missed.
    pub twin_visible: usize,
}

/// How near a read must come to one candidate of the library, or of the
/// dismissed names, to be it.
///
/// A near match must lead every other tool by the margin, of its own list
/// and of the others. The margin is set so that a read with one character
/// wrong never names a different tool, unless the wrong character spells
/// that other tool exactly (which no matcher can tell from a right read).
/// On the similarity scale (a hundred times one less the edit steps over
/// the two keys' lengths, a substitution costing two steps), a read of
/// `n` characters one substitution off the right tool scores at least
/// `100 (1 - 1/n)` against it. Short of spelling it, a read is at least
/// one step from any other tool, which then scores at most
/// `100 (1 - 1/(2n + 1))`, the case where that tool's compared key is one
/// character longer. So another tool can lead the right one by at most
/// `100 (1/n - 1/(2n + 1))`, and only when it reaches the floor, which
/// needs `2n + 1 >= 100 / (100 - floor)`. The lead is greatest at the
/// shortest such read; with the floor at 90 that read is five characters
/// long and the lead at most 10.9, so the margin is 11. Taking a near
/// match at all then needs a lead of 11 over every rival, which a read of
/// a long name with a one-character sibling rarely has: such a read is
/// reported as contended rather than guessed. The margin guards only
/// against misreads of tools some list knows: a tool no list knows is as
/// near its listed neighbour as a misread is, which is why the bundled
/// list, which holds tools the player has not confirmed owning, takes
/// exact spellings only. The module's tests recompute this bound from the
/// profile's numbers.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierProfile {
    /// Least similarity (0 to 100) at which a read is near a candidate.
    pub floor: f64,
    /// Least lead of the nearest candidate over the next (another tool)
    /// for a near read to be the nearest.
    pub margin: f64,
}

/// How near a read must come to a bundled tool to be offered as one of
/// the tools it may be. A bundled tool is the answer only when the read
/// spells it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundledProfile {
    /// Least similarity (0 to 100) at which a read is near a tool.
    pub floor: f64,
}

/// What kind of tool a library entry is. Consumables are never held, so
/// never on the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Weapon,
    Healing,
    Tool,
}

/// One entry of the player's equipment library. Names are not unique:
/// one weapon kept in two set-ups is two entries of one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryTool {
    pub id: i64,
    pub name: String,
    pub kind: ToolKind,
    /// Whether the player has the entry in use: of entries the bar cannot
    /// tell apart, the enabled one is the answer.
    pub enabled: bool,
}

/// One tool the bundled game data knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundledTool {
    /// The collection it is listed in (one of [`TOOL_ENDPOINTS`]).
    pub endpoint: String,
    /// Its `id` in that collection.
    pub item_id: String,
    pub name: String,
}

/// A candidate with its key.
#[derive(Debug, Clone)]
struct Keyed<T> {
    tool: T,
    key: Vec<char>,
    counts: KeyCounts,
    /// The first candidate of its list with the same key: candidates of
    /// one key are one tool as far as the bar can say.
    same: usize,
}

fn keyed<T>(list: impl IntoIterator<Item = (T, String)>, merge_same: bool) -> Vec<Keyed<T>> {
    let mut out: Vec<Keyed<T>> = Vec::new();
    for (tool, name) in list {
        let key = label_key(&name);
        if key.is_empty() {
            continue;
        }
        let same = if merge_same {
            out.iter().position(|k| k.key == key).unwrap_or(out.len())
        } else {
            out.len()
        };
        out.push(Keyed {
            tool,
            counts: key_counts(&key),
            key,
            same,
        });
    }
    out
}

/// The tools a name read off the bar is matched against.
#[derive(Debug, Clone, Default)]
pub struct ToolCatalogue {
    library: Vec<Keyed<LibraryTool>>,
    dismissed: Vec<Keyed<String>>,
    bundled: Vec<Keyed<BundledTool>>,
}

impl ToolCatalogue {
    /// A catalogue of the player's `library`, the tools of the bundled
    /// game data, and the names the player has dismissed.
    pub fn new(
        library: impl IntoIterator<Item = LibraryTool>,
        bundled: impl IntoIterator<Item = BundledTool>,
        dismissed: impl IntoIterator<Item = String>,
    ) -> Self {
        ToolCatalogue {
            library: keyed(
                library.into_iter().map(|t| {
                    let name = t.name.clone();
                    (t, name)
                }),
                true,
            ),
            dismissed: keyed(dismissed.into_iter().map(|n| (n.clone(), n)), true),
            bundled: keyed(
                bundled.into_iter().map(|t| {
                    let name = t.name.clone();
                    (t, name)
                }),
                false,
            ),
        }
    }

    /// From the bundled game data's entities, by collection: each one's
    /// `name` and `id` (a string, or a number written out).
    pub fn from_entities<'a>(
        library: impl IntoIterator<Item = LibraryTool>,
        collections: impl IntoIterator<Item = (&'a str, &'a [Value])>,
        dismissed: impl IntoIterator<Item = String>,
    ) -> Self {
        let bundled = collections.into_iter().flat_map(|(endpoint, entities)| {
            entities.iter().filter_map(move |entity| {
                let name = entity.get("name")?.as_str()?.trim();
                let item_id = match entity.get("id")? {
                    Value::String(id) => id.clone(),
                    Value::Number(id) => id.to_string(),
                    _ => return None,
                };
                (!name.is_empty()).then(|| BundledTool {
                    endpoint: endpoint.to_string(),
                    item_id,
                    name: name.to_string(),
                })
            })
        });
        Self::new(library, bundled.collect::<Vec<_>>(), dismissed)
    }

    /// From the bundled game data's tool collections ([`TOOL_ENDPOINTS`]).
    pub fn from_store(
        store: &GameDataStore,
        library: impl IntoIterator<Item = LibraryTool>,
        dismissed: impl IntoIterator<Item = String>,
    ) -> Self {
        Self::from_entities(
            library,
            TOOL_ENDPOINTS
                .iter()
                .map(|endpoint| (*endpoint, store.get_entities(endpoint))),
            dismissed,
        )
    }

    pub fn library(&self) -> impl Iterator<Item = &LibraryTool> {
        self.library.iter().map(|k| &k.tool)
    }

    pub fn bundled(&self) -> impl Iterator<Item = &BundledTool> {
        self.bundled.iter().map(|k| &k.tool)
    }
}

/// Which tool a name read off the bar is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolMatch {
    /// One enabled library entry.
    Library { id: i64 },
    /// Enabled library entries this read cannot tell apart: one name kept
    /// twice, or a name cut short that fits several. Sorted.
    Twins { ids: Vec<i64> },
    /// Library entries, all of them disabled. Sorted.
    Disabled { ids: Vec<i64> },
    /// A name the player has dismissed as not a tracked tool, as it was
    /// dismissed.
    NotTracked { name: String },
    /// No library answer; one tool of the bundled game data.
    Bundled {
        endpoint: String,
        item_id: String,
        name: String,
    },
    /// Tools this read cannot tell apart, of the bundled game data and
    /// perhaps of the library, nearest first, as many as the profile
    /// lists, and how many more there are.
    Contended {
        candidates: Vec<Candidate>,
        more: usize,
    },
    /// Near nothing well enough to say: the text as read.
    Unknown { text: String },
}

/// One of the tools a contended read may be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Candidate {
    /// A tool of the player's library: its enabled entries, or all its
    /// entries when none is enabled (sorted), by the name they share.
    Library { ids: Vec<i64>, name: String },
    /// A tool of the bundled game data.
    Bundled(BundledTool),
}

impl Candidate {
    /// The tool's name.
    pub fn name(&self) -> &str {
        match self {
            Candidate::Library { name, .. } => name,
            Candidate::Bundled(tool) => &tool.name,
        }
    }
}

/// Whether `read` spells `candidate`: the whole of it, or the start of it
/// for a name cut short.
fn spells(read: &[char], cut_short: bool, candidate: &[char]) -> bool {
    if cut_short {
        candidate.starts_with(read)
    } else {
        candidate == read
    }
}

/// The lengths of a candidate's start a name cut short is compared with:
/// as long as the read, give or take the profile's tolerance. `None` for
/// a candidate too short to have been cut to the read.
fn cut_lengths(
    read: &[char],
    candidate: &[char],
    profile: &ToolNameProfile,
) -> Option<(usize, usize)> {
    let least = read.len().saturating_sub(profile.cut_tolerance).max(1);
    let most = (read.len() + profile.cut_tolerance).min(candidate.len());
    (candidate.len() >= least).then_some((least, most))
}

/// How near `read` (a key) comes to `candidate` (a key), out of a
/// hundred: a name not cut short is compared whole; one cut short with
/// the candidate's start, at each length the tolerance allows, the best
/// taken. `None` when a name cut short cannot be the candidate's start.
pub fn score(
    read: &[char],
    cut_short: bool,
    candidate: &[char],
    profile: &ToolNameProfile,
) -> Option<f64> {
    if !cut_short {
        return Some(ratio(read, candidate));
    }
    let (least, most) = cut_lengths(read, candidate, profile)?;
    (least..=most)
        .map(|end| ratio(read, &candidate[..end]))
        .reduce(f64::max)
}

/// Whether `read` could score `least` or more against `candidate`, by
/// letter counts alone.
fn could_reach<T>(
    read: &[char],
    read_counts: &KeyCounts,
    cut_short: bool,
    candidate: &Keyed<T>,
    least: f64,
    profile: &ToolNameProfile,
) -> bool {
    if !cut_short {
        return similarity_bound(
            read_counts,
            read.len(),
            &candidate.counts,
            candidate.key.len(),
        ) >= least;
    }
    let Some((shortest, longest)) = cut_lengths(read, &candidate.key, profile) else {
        return false;
    };
    let counts = if longest == candidate.key.len() {
        candidate.counts
    } else {
        key_counts(&candidate.key[..longest])
    };
    // A shorter start has no more letters in common, and the score is
    // greatest over the shortest length compared.
    similarity_bound(read_counts, read.len(), &counts, shortest) >= least
}

/// How a read scored against one tool of a list.
#[derive(Debug, Clone, Copy)]
struct Scored {
    tool: usize,
    score: f64,
    spelled: bool,
    /// The length of the tool's key.
    len: usize,
    /// The read is not cut short and its key is the start of the tool's,
    /// or the tool's of its own, short of spelling it: another name.
    prefixed: bool,
}

/// Each tool of `list` once (candidates of one key together), with how
/// `read` scored against it; tools out of reach of `least` are left out.
fn scored<T>(
    list: &[Keyed<T>],
    read: &[char],
    cut_short: bool,
    least: f64,
    profile: &ToolNameProfile,
) -> Vec<Scored> {
    let read_counts = key_counts(read);
    list.iter()
        .enumerate()
        .filter(|(i, k)| k.same == *i)
        .filter_map(|(i, k)| {
            let spelled = spells(read, cut_short, &k.key);
            if !spelled && !could_reach(read, &read_counts, cut_short, k, least, profile) {
                return None;
            }
            let score = score(read, cut_short, &k.key, profile)?;
            let prefixed =
                !cut_short && !spelled && (k.key.starts_with(read) || read.starts_with(&k.key));
            Some(Scored {
                tool: i,
                score,
                spelled,
                len: k.key.len(),
                prefixed,
            })
        })
        .collect()
}

/// What one list makes of a read.
#[derive(Debug, PartialEq)]
enum Decision {
    /// The tools the read spells (by the index of their first candidates),
    /// those whose keys are nearest the read's length first.
    Spelled(Vec<usize>),
    /// One tool the read is near, clear of the list's others.
    One(usize),
    /// Tools the read is near and cannot tell apart, nearest first.
    Several(Vec<usize>),
    /// None near enough.
    Nothing,
}

/// The tools of `scored` the read spells, those whose keys are nearest
/// the read's length first (else in the list's order).
fn spelled(scored: &[Scored], read_len: usize) -> Vec<usize> {
    let mut spelled: Vec<&Scored> = scored.iter().filter(|s| s.spelled).collect();
    spelled.sort_by_key(|s| s.len.abs_diff(read_len));
    spelled.into_iter().map(|s| s.tool).collect()
}

/// `scored` nearest first; tools scoring alike stay in the list's order.
fn nearest_first(scored: &[Scored]) -> Vec<Scored> {
    let mut sorted = scored.to_vec();
    sorted.sort_by(|a, b| b.score.total_cmp(&a.score));
    sorted
}

/// A list's decision: the tools the read spells; else the nearest, if it
/// reaches the floor, leads the next by the margin and is not another name
/// by a start; else the tools at the floor within the margin of the
/// nearest, if there are several or the nearest is another name by a
/// start.
fn decide(scored: &[Scored], read_len: usize, tier: &TierProfile) -> Decision {
    let spelled = spelled(scored, read_len);
    if !spelled.is_empty() {
        return Decision::Spelled(spelled);
    }
    let sorted = nearest_first(scored);
    let Some(best) = sorted.first() else {
        return Decision::Nothing;
    };
    if best.score < tier.floor {
        return Decision::Nothing;
    }
    let next = sorted.get(1).map_or(f64::NEG_INFINITY, |s| s.score);
    if !best.prefixed && best.score - next >= tier.margin {
        return Decision::One(best.tool);
    }
    let contending: Vec<usize> = sorted
        .iter()
        .take_while(|s| s.score >= tier.floor && best.score - s.score < tier.margin)
        .map(|s| s.tool)
        .collect();
    if contending.len() > 1 || best.prefixed {
        Decision::Several(contending)
    } else {
        Decision::Nothing
    }
}

/// Sorted, each once.
fn sorted(mut ids: Vec<i64>) -> Vec<i64> {
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// The tools of `list` (of other keys than `keys`) that the read comes
/// within `margin` of `best` to: rivals that deny a near match its lead.
fn rivals<T>(
    list: &[Keyed<T>],
    keys: &[&[char]],
    best: f64,
    read: &[char],
    cut_short: bool,
    margin: f64,
    profile: &ToolNameProfile,
) -> Vec<Scored> {
    scored(list, read, cut_short, best - margin, profile)
        .into_iter()
        .filter(|s| best - s.score < margin && !keys.contains(&list[s.tool].key.as_slice()))
        .collect()
}

/// A contention among `scored` candidates, nearest first (in the order
/// given where they score alike), as many as the profile lists.
fn contention(mut scored: Vec<(f64, Candidate)>, profile: &ToolNameProfile) -> ToolMatch {
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    ToolMatch::Contended {
        more: scored.len().saturating_sub(profile.max_candidates),
        candidates: scored
            .into_iter()
            .take(profile.max_candidates)
            .map(|(_, candidate)| candidate)
            .collect(),
    }
}

fn bundled_match(tool: &BundledTool) -> ToolMatch {
    ToolMatch::Bundled {
        endpoint: tool.endpoint.clone(),
        item_id: tool.item_id.clone(),
        name: tool.name.clone(),
    }
}

/// The ids of the library entries of `tools` (by their first entries)
/// that are enabled, or of all of them when none is, sorted; and whether
/// they are the enabled ones.
fn library_ids(catalogue: &ToolCatalogue, tools: &[usize]) -> (Vec<i64>, bool) {
    let entries: Vec<&LibraryTool> = catalogue
        .library
        .iter()
        .filter(|k| tools.contains(&k.same))
        .map(|k| &k.tool)
        .collect();
    let enabled = sorted(entries.iter().filter(|t| t.enabled).map(|t| t.id).collect());
    if enabled.is_empty() {
        (sorted(entries.iter().map(|t| t.id).collect()), false)
    } else {
        (enabled, true)
    }
}

/// The library tool whose first entry is `tool`, as a candidate.
fn library_candidate(catalogue: &ToolCatalogue, tool: usize) -> Candidate {
    Candidate::Library {
        ids: library_ids(catalogue, &[tool]).0,
        name: catalogue.library[tool].tool.name.clone(),
    }
}

/// The answer the library's entries of `tools` give together: the one
/// enabled entry, the enabled entries, or the disabled ones.
fn library_answer(catalogue: &ToolCatalogue, tools: &[usize]) -> ToolMatch {
    match library_ids(catalogue, tools) {
        (ids, true) if ids.len() == 1 => ToolMatch::Library { id: ids[0] },
        (ids, true) => ToolMatch::Twins { ids },
        (ids, false) => ToolMatch::Disabled { ids },
    }
}

fn score_of(scores: &[Scored], tool: usize) -> f64 {
    scores
        .iter()
        .find(|s| s.tool == tool)
        .map_or(f64::NEG_INFINITY, |s| s.score)
}

/// The library's answer (see the module's notes).
fn from_library(
    catalogue: &ToolCatalogue,
    read: &[char],
    cut_short: bool,
    profile: &ToolNameProfile,
) -> Option<ToolMatch> {
    let tier = &profile.library;
    let scores = scored(
        &catalogue.library,
        read,
        cut_short,
        tier.floor - tier.margin,
        profile,
    );
    let tools = match decide(&scores, read.len(), tier) {
        Decision::Spelled(tools) => return Some(library_answer(catalogue, &tools)),
        Decision::One(tool) => vec![tool],
        Decision::Several(tools) => tools,
        Decision::Nothing => return None,
    };
    let best = tools
        .iter()
        .map(|t| score_of(&scores, *t))
        .fold(f64::NEG_INFINITY, f64::max);
    let keys: Vec<&[char]> = tools
        .iter()
        .map(|t| catalogue.library[*t].key.as_slice())
        .collect();
    let m = tier.margin;
    let bundled = rivals(&catalogue.bundled, &keys, best, read, cut_short, m, profile);
    let dismissed = rivals(
        &catalogue.dismissed,
        &keys,
        best,
        read,
        cut_short,
        m,
        profile,
    );
    let spelt_bundled: Vec<&Scored> = bundled.iter().filter(|s| s.spelled).collect();
    let spelt_dismissed: Vec<&Scored> = dismissed.iter().filter(|s| s.spelled).collect();
    match (spelt_bundled.as_slice(), spelt_dismissed.as_slice()) {
        ([one], []) => return Some(bundled_match(&catalogue.bundled[one.tool].tool)),
        ([], [one]) => {
            return Some(ToolMatch::NotTracked {
                name: catalogue.dismissed[one.tool].tool.clone(),
            })
        }
        _ => {}
    }
    if let [tool] = tools.as_slice() {
        let another_name = scores.iter().any(|s| s.tool == *tool && s.prefixed);
        if bundled.is_empty() && dismissed.is_empty() && !another_name {
            return Some(library_answer(catalogue, &tools));
        }
        if bundled.is_empty() && !dismissed.is_empty() {
            // Near a dismissed name as near as this: neither answers.
            return None;
        }
    }
    let mut candidates: Vec<(f64, Candidate)> = tools
        .iter()
        .map(|t| (score_of(&scores, *t), library_candidate(catalogue, *t)))
        .collect();
    candidates.extend(bundled.iter().map(|s| {
        (
            s.score,
            Candidate::Bundled(catalogue.bundled[s.tool].tool.clone()),
        )
    }));
    Some(contention(candidates, profile))
}

/// A dismissed name the read is. A read only near one must lead every
/// library and bundled tool by another name by the dismissed names'
/// margin, or the dismissed names do not answer.
fn from_dismissed(
    catalogue: &ToolCatalogue,
    read: &[char],
    cut_short: bool,
    profile: &ToolNameProfile,
) -> Option<ToolMatch> {
    let tier = &profile.dismissed;
    let scores = scored(
        &catalogue.dismissed,
        read,
        cut_short,
        tier.floor - tier.margin,
        profile,
    );
    let found = match decide(&scores, read.len(), tier) {
        Decision::Spelled(spelled) => *spelled.first()?,
        Decision::One(one) => {
            let best = score_of(&scores, one);
            let key = [catalogue.dismissed[one].key.as_slice()];
            let m = tier.margin;
            if !rivals(&catalogue.library, &key, best, read, cut_short, m, profile).is_empty()
                || !rivals(&catalogue.bundled, &key, best, read, cut_short, m, profile).is_empty()
            {
                return None;
            }
            one
        }
        Decision::Several(_) | Decision::Nothing => return None,
    };
    Some(ToolMatch::NotTracked {
        name: catalogue.dismissed[found].tool.clone(),
    })
}

/// The bundled list's answer: the one tool the read spells; else the
/// tools it spells, or is near, as a contention, with any library tool
/// as near as those; but nothing when a dismissed name is as near.
fn from_bundled(
    catalogue: &ToolCatalogue,
    read: &[char],
    cut_short: bool,
    profile: &ToolNameProfile,
) -> Option<ToolMatch> {
    let floor = profile.bundled.floor;
    let scores = scored(&catalogue.bundled, read, cut_short, floor, profile);
    let as_candidate = |s: &Scored| {
        (
            s.score,
            Candidate::Bundled(catalogue.bundled[s.tool].tool.clone()),
        )
    };
    let spelt = spelled(&scores, read.len());
    match spelt.as_slice() {
        [one] => return Some(bundled_match(&catalogue.bundled[*one].tool)),
        [_, _, ..] => {
            let by_tool = |t: &usize| scores.iter().find(|s| s.tool == *t).map(as_candidate);
            return Some(contention(
                spelt.iter().filter_map(by_tool).collect(),
                profile,
            ));
        }
        [] => {}
    }
    let near: Vec<Scored> = nearest_first(&scores)
        .into_iter()
        .filter(|s| s.score >= floor)
        .collect();
    let best = near.first()?.score;
    let keys: Vec<&[char]> = near
        .iter()
        .map(|s| catalogue.bundled[s.tool].key.as_slice())
        .collect();
    let d = &profile.dismissed;
    if !rivals(
        &catalogue.dismissed,
        &keys,
        best,
        read,
        cut_short,
        d.margin,
        profile,
    )
    .is_empty()
    {
        return None;
    }
    let l = &profile.library;
    let mut candidates: Vec<(f64, Candidate)> = near.iter().map(as_candidate).collect();
    candidates.extend(
        rivals(
            &catalogue.library,
            &keys,
            best,
            read,
            cut_short,
            l.margin,
            profile,
        )
        .iter()
        .map(|s| (s.score, library_candidate(catalogue, s.tool))),
    );
    Some(contention(candidates, profile))
}

/// Whether `read` spells a tool of any list.
fn spells_any(catalogue: &ToolCatalogue, read: &[char], cut_short: bool) -> bool {
    catalogue
        .library
        .iter()
        .any(|k| spells(read, cut_short, &k.key))
        || catalogue
            .dismissed
            .iter()
            .any(|k| spells(read, cut_short, &k.key))
        || catalogue
            .bundled
            .iter()
            .any(|k| spells(read, cut_short, &k.key))
}

/// Resolve `name`, read off the tool bar, against `catalogue`: the
/// library first, then the dismissed names, then the bundled tools; what
/// none of them answers is [`ToolMatch::Unknown`], as is a read marked cut
/// short whose key is too short for the client to have cut it, and a read
/// with too few letters to be a name that spells nothing exactly.
pub fn resolve(name: &ToolName, catalogue: &ToolCatalogue, profile: &ToolNameProfile) -> ToolMatch {
    let read = name.key();
    let cut = name.cut_short;
    let unknown = || ToolMatch::Unknown {
        text: name.text.clone(),
    };
    // No share of letters is asked of a tool's name: model numbers are
    // mostly digits.
    if read.is_empty()
        || (cut && read.len() < profile.min_cut_key)
        || (too_few_letters(&name.text, profile.min_letters, 0.0)
            && !spells_any(catalogue, &read, cut))
    {
        return unknown();
    }
    from_library(catalogue, &read, cut, profile)
        .or_else(|| from_dismissed(catalogue, &read, cut, profile))
        .or_else(|| from_bundled(catalogue, &read, cut, profile))
        .unwrap_or_else(unknown)
}

/// An entry's key, and the key of its start where it may be cut short.
type Shown = (Vec<char>, Option<Vec<char>>);

/// The groups of library entries the bar could not tell apart, enabled
/// or not: entries of one key; and entries whose names are both longer
/// than the profile's `twin_visible` characters (so may be cut short) and
/// agree in letters and digits over that many characters. Entries alike
/// through a third are one group, so the groups are disjoint. Each group
/// is sorted, and holds two entries or more.
pub fn indistinguishable(entries: &[LibraryTool], profile: &ToolNameProfile) -> Vec<Vec<i64>> {
    let shown: Vec<Option<Shown>> = entries
        .iter()
        .map(|entry| {
            let key = label_key(&entry.name);
            let chars: Vec<char> = entry.name.trim().chars().collect();
            let start = (chars.len() > profile.twin_visible)
                .then(|| label_key(&chars[..profile.twin_visible].iter().collect::<String>()));
            (!key.is_empty()).then_some((key, start))
        })
        .collect();
    // Each entry joined to the first entry of its key, and to the first
    // of its start where it may be cut short: a chain of alike entries is
    // one group.
    let mut parent: Vec<usize> = (0..entries.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut first: HashMap<(bool, &[char]), usize> = HashMap::new();
    for (i, entry) in shown.iter().enumerate() {
        let Some((key, start)) = entry else {
            continue;
        };
        let marks = [
            Some((false, key.as_slice())),
            start.as_deref().map(|s| (true, s)),
        ];
        for mark in marks.into_iter().flatten() {
            let other = *first.entry(mark).or_insert(i);
            let (a, b) = (root(&mut parent, i), root(&mut parent, other));
            parent[a.max(b)] = a.min(b);
        }
    }
    let group: Vec<usize> = (0..entries.len()).map(|i| root(&mut parent, i)).collect();
    let mut by_root: HashMap<usize, Vec<i64>> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        if shown[i].is_some() {
            by_root.entry(group[i]).or_default().push(entry.id);
        }
    }
    let mut groups: Vec<Vec<i64>> = by_root
        .into_values()
        .map(sorted)
        .filter(|ids| ids.len() > 1)
        .collect();
    groups.sort();
    groups
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use proptest::prelude::*;
    use proptest::sample::Index;

    use super::*;
    use crate::game_ui::test_support::{bundled_collection, bundled_profile};

    const MAYHEM: &str = "Exceptional Mayhem Electric Attack Chip 15, Perfected (L)";

    fn profile() -> ToolNameProfile {
        bundled_profile().tool_names
    }

    /// The bundled tool collections, loaded once.
    fn collections() -> &'static Vec<(&'static str, Vec<Value>)> {
        static LOADED: OnceLock<Vec<(&'static str, Vec<Value>)>> = OnceLock::new();
        LOADED.get_or_init(|| {
            TOOL_ENDPOINTS
                .iter()
                .map(|endpoint| (*endpoint, bundled_collection(endpoint)))
                .collect()
        })
    }

    /// The bundled tools, with `library` and `dismissed`.
    fn bundled_with(library: Vec<LibraryTool>, dismissed: Vec<String>) -> ToolCatalogue {
        ToolCatalogue::from_entities(
            library,
            collections().iter().map(|(e, list)| (*e, list.as_slice())),
            dismissed,
        )
    }

    /// The bundled tools alone, built once.
    fn bundled() -> &'static ToolCatalogue {
        static BUILT: OnceLock<ToolCatalogue> = OnceLock::new();
        BUILT.get_or_init(|| bundled_with(Vec::new(), Vec::new()))
    }

    fn read(text: &str, cut_short: bool) -> ToolName {
        ToolName {
            text: text.into(),
            cut_short,
        }
    }

    fn entry(id: i64, name: &str, enabled: bool) -> LibraryTool {
        LibraryTool {
            id,
            name: name.into(),
            kind: ToolKind::Weapon,
            enabled,
        }
    }

    fn tool(name: &str) -> BundledTool {
        BundledTool {
            endpoint: "weapons".into(),
            item_id: format!("id-{name}"),
            name: name.into(),
        }
    }

    /// A catalogue of made-up bundled tools only.
    fn of(names: &[&str]) -> ToolCatalogue {
        ToolCatalogue::new(Vec::new(), names.iter().map(|n| tool(n)), Vec::new())
    }

    /// The name of the one bundled tool a match names.
    fn bundled_name(found: &ToolMatch) -> Option<&str> {
        match found {
            ToolMatch::Bundled { name, .. } => Some(name.as_str()),
            _ => None,
        }
    }

    fn unknown(text: &str) -> ToolMatch {
        ToolMatch::Unknown { text: text.into() }
    }

    #[test]
    fn a_tool_in_no_list_is_unknown_and_after_dismissal_not_tracked() {
        let p = profile();
        for text in [
            "Teleportation Chip 1",
            "Portable Repair Unit (L)",
            "Portable TT Unit (L)",
        ] {
            assert_eq!(
                resolve(&read(text, false), bundled(), &p),
                unknown(text),
                "{text}"
            );
        }
        let dismissed = bundled_with(Vec::new(), vec!["Teleportation Chip 1".into()]);
        for text in [
            "Teleportation Chip 1",
            "TeleportationChip1",
            "Teleportatlon Chip 1",
        ] {
            assert_eq!(
                resolve(&read(text, false), &dismissed, &p),
                ToolMatch::NotTracked {
                    name: "Teleportation Chip 1".into()
                },
                "{text}"
            );
        }
        // A dismissed name answers only for itself.
        assert_eq!(
            resolve(&read("Portable TT Unit (L)", false), &dismissed, &p),
            unknown("Portable TT Unit (L)")
        );
    }

    /// Reads of the cut name `Exceptional Mayhem Electric Att...` off
    /// recorded and live frames.
    const MAYHEM_READS: [&str; 4] = [
        "Exceptional Mayhem ElectricAl",
        "Exceptional MayhemBlectricAl",
        "Exceptional Mayhem Electric A",
        "Exceptional Mayhem Electric Att",
    ];

    #[test]
    fn reads_of_a_cut_name_resolve_to_the_library_entry_or_else_are_offered() {
        let p = profile();
        let library = bundled_with(vec![entry(7, MAYHEM, true)], Vec::new());
        for text in MAYHEM_READS {
            assert_eq!(
                resolve(&read(text, true), &library, &p),
                ToolMatch::Library { id: 7 },
                "{text}"
            );
        }
        // Against the bundled list alone, a read spelling the chip's start
        // is the chip; a misread of it is only offered, the chip first.
        for text in [
            "Exceptional Mayhem Electric A",
            "Exceptional Mayhem Electric Att",
        ] {
            let found = resolve(&read(text, true), bundled(), &p);
            let ToolMatch::Bundled {
                endpoint,
                item_id,
                name,
            } = found
            else {
                panic!("{text}: {found:?}")
            };
            assert_eq!(
                (endpoint.as_str(), item_id.as_str(), name.as_str()),
                ("weapons", "ac59058ab1a3", MAYHEM)
            );
        }
        for text in [
            "Exceptional Mayhem ElectricAl",
            "Exceptional MayhemBlectricAl",
        ] {
            match resolve(&read(text, true), bundled(), &p) {
                ToolMatch::Contended { candidates, .. } => {
                    assert_eq!(candidates[0].name(), MAYHEM, "{text}: {candidates:?}")
                }
                other => panic!("{text}: {other:?}"),
            }
        }
        // The neighbours the bundled list holds are there, and far off.
        for neighbour in [
            "Exceptional Mayhem BP-70, Perfected (L)",
            "Exceptional Mayhem Arsonistic Chip 15 (L)",
            "Exceptional Mayhem Nano-Blade, Modified (L)",
        ] {
            assert!(
                bundled().bundled().any(|t| t.name == neighbour),
                "{neighbour}"
            );
        }
    }

    #[test]
    fn a_read_spelling_one_tool_exactly_wins_and_one_character_wrong_is_contended() {
        let p = profile();
        let found = resolve(&read("Hyperion Aurora TC-1,Improv", true), bundled(), &p);
        assert_eq!(
            bundled_name(&found),
            Some("Hyperion Aurora TC-1, Improved (L)"),
            "{found:?}"
        );
        // The siblings are near: 95.7 for OC-1.
        let near = score(
            &label_key("Hyperion Aurora TC-1,Improv"),
            true,
            &label_key("Hyperion Aurora OC-1, Improved (L)"),
            &p,
        )
        .unwrap();
        assert!(near > 95.0, "{near}");
        match resolve(&read("Hyperion Aurora TG-1,Improv", true), bundled(), &p) {
            ToolMatch::Contended { candidates, .. } => {
                let names: Vec<&str> = candidates.iter().map(|c| c.name()).collect();
                assert!(
                    names.contains(&"Hyperion Aurora TC-1, Improved (L)"),
                    "{names:?}"
                );
                assert!(
                    names.contains(&"Hyperion Aurora GR-1, Improved (L)"),
                    "{names:?}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn text_read_from_under_a_window_is_unknown() {
        let p = profile();
        for (text, cut) in [
            ("300.0%Electri At", true),
            ("994.26 PED MAX COST:2.8833", false),
            ("TeleporButcherina", false),
            ("MSHERE", false),
            ("Hien", false),
        ] {
            assert_eq!(
                resolve(&read(text, cut), bundled(), &p),
                unknown(text),
                "{text}"
            );
        }
    }

    #[test]
    fn entries_of_one_name_are_twins_unless_all_but_one_are_disabled() {
        let p = profile();
        let name = "Sollomate Opalo";
        let both =
            |a: bool, b: bool| bundled_with(vec![entry(4, name, a), entry(2, name, b)], Vec::new());
        let found = |catalogue: &ToolCatalogue| resolve(&read(name, false), catalogue, &p);
        assert_eq!(
            found(&both(true, true)),
            ToolMatch::Twins { ids: vec![2, 4] }
        );
        assert_eq!(found(&both(true, false)), ToolMatch::Library { id: 4 });
        assert_eq!(found(&both(false, true)), ToolMatch::Library { id: 2 });
        assert_eq!(
            found(&both(false, false)),
            ToolMatch::Disabled { ids: vec![2, 4] }
        );
        // A misread of the name finds them all the same.
        assert_eq!(
            resolve(&read("Sollomate 0palo", false), &both(true, true), &p),
            ToolMatch::Twins { ids: vec![2, 4] }
        );
    }

    #[test]
    fn a_name_cut_short_that_fits_several_entries_is_twins_unless_one_is_enabled() {
        let p = profile();
        let long = |suffix: &str| format!("Exceptional Mayhem Electric Attack Chip {suffix}");
        let library = |second: bool| {
            vec![
                entry(1, &long("15, Perfected (L)"), true),
                entry(3, &long("15, Modified (L)"), second),
            ]
        };
        let cut = read("Exceptional Mayhem Electric Att", true);
        assert_eq!(
            resolve(&cut, &bundled_with(library(true), Vec::new()), &p),
            ToolMatch::Twins { ids: vec![1, 3] }
        );
        assert_eq!(
            resolve(&cut, &bundled_with(library(false), Vec::new()), &p),
            ToolMatch::Library { id: 1 }
        );
        // Read whole, the names differ.
        assert_eq!(
            resolve(
                &read(&long("15, Perfected (L)"), false),
                &bundled_with(library(true), Vec::new()),
                &p
            ),
            ToolMatch::Library { id: 1 }
        );
    }

    #[test]
    fn a_disabled_entry_the_read_spells_is_not_taken_for_an_enabled_one_it_is_near() {
        let p = profile();
        let catalogue = ToolCatalogue::new(
            vec![
                entry(1, "Sollomate Opalo", true),
                entry(2, "Sollomate Opala", false),
            ],
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            resolve(&read("Sollomate Opala", false), &catalogue, &p),
            ToolMatch::Disabled { ids: vec![2] }
        );
    }

    #[test]
    fn a_library_answer_beats_the_bundled_list_and_dismissal_comes_between() {
        let p = profile();
        let catalogue = ToolCatalogue::new(
            vec![entry(9, "Kirov Blaster", true)],
            vec![tool("Kirov Blaster"), tool("Morka Rifle")],
            vec!["Kirov Blaster".into(), "Morka Rifle".into()],
        );
        assert_eq!(
            resolve(&read("Kirov Blaster", false), &catalogue, &p),
            ToolMatch::Library { id: 9 }
        );
        assert_eq!(
            resolve(&read("Morka Rifle", false), &catalogue, &p),
            ToolMatch::NotTracked {
                name: "Morka Rifle".into()
            }
        );
    }

    #[test]
    fn a_whole_name_is_compared_whole_and_a_cut_one_with_the_start_of_each_tool() {
        let p = profile();
        let long = "Kirov Blaster Mark Seven Deluxe";
        let catalogue = of(&[long, "Kirov Blaster Ma"]);
        // Not cut short: the start of the longer name is not it.
        assert_eq!(
            bundled_name(&resolve(
                &read("Kirov Blaster Mark Sev", false),
                &catalogue,
                &p
            )),
            None
        );
        // Cut short: it is, and the name shorter than the read by more
        // than the tolerance cannot be.
        assert_eq!(
            bundled_name(&resolve(
                &read("Kirov Blaster Mark Sev", true),
                &catalogue,
                &p
            )),
            Some(long)
        );
        assert_eq!(
            score(
                &label_key("Kirov Blaster Mark Sev"),
                true,
                &label_key("Kirov Blaster Ma"),
                &p
            ),
            None
        );
        // Within the tolerance a shorter name is compared.
        assert!(score(
            &label_key("Kirov Blaster Mark Se"),
            true,
            &label_key("Kirov Blaster Mark"),
            &p
        )
        .is_some());
    }

    #[test]
    fn a_read_marked_cut_short_with_too_short_a_key_names_nothing() {
        let p = profile();
        let catalogue = of(&["Pistol", "Reaper", "ReaperX Mark Two Deluxe Edition"]);
        for text in ["Pistol", "Reaper", "ReaperX"] {
            assert_eq!(
                resolve(&read(text, true), &catalogue, &p),
                unknown(text),
                "{text}"
            );
        }
        // Read whole, the same names are named.
        assert_eq!(
            bundled_name(&resolve(&read("Pistol", false), &catalogue, &p)),
            Some("Pistol")
        );
        // At the profile's least key, a cut read is compared.
        let at_least: String = label_key("ReaperX Mark Two Deluxe Edition")[..p.min_cut_key]
            .iter()
            .collect();
        assert_eq!(
            bundled_name(&resolve(&read(&at_least, true), &catalogue, &p)),
            Some("ReaperX Mark Two Deluxe Edition")
        );
    }

    #[test]
    fn spacing_case_and_punctuation_never_decide() {
        let p = profile();
        let catalogue = of(&["Hyperion Aurora TC-1, Improved (L)"]);
        for text in [
            "Hyperion Aurora TC-1, Improved (L)",
            "hyperion aurora tc1 improved l",
            "HyperionAuroraTC-1,Improved(L)",
        ] {
            assert_eq!(
                bundled_name(&resolve(&read(text, false), &catalogue, &p)),
                Some("Hyperion Aurora TC-1, Improved (L)"),
                "{text}"
            );
        }
    }

    #[test]
    fn too_few_letters_name_no_tool_unless_spelled_exactly() {
        let p = profile();
        let catalogue = of(&["Ox", "Hel"]);
        for text in ["Ox", "Hel"] {
            assert_eq!(
                bundled_name(&resolve(&read(text, false), &catalogue, &p)),
                Some(text)
            );
        }
        assert_eq!(resolve(&read("0x2", false), &catalogue, &p), unknown("0x2"));
        assert_eq!(resolve(&read("Oy", false), &catalogue, &p), unknown("Oy"));
        // A model number on the real list.
        assert_eq!(
            bundled_name(&resolve(&read("M2100", false), bundled(), &p)),
            Some("M2100")
        );
        assert_eq!(
            resolve(&read("M2101", false), bundled(), &p),
            unknown("M2101")
        );
    }

    #[test]
    fn a_bundled_tool_is_the_answer_only_when_spelled() {
        let p = profile();
        // One wrong character in eleven: 90.9, alone, and only offered.
        let one = of(&["Morka Riflex"]);
        assert_eq!(
            resolve(&read("Morka Rlflex", false), &one, &p),
            ToolMatch::Contended {
                candidates: vec![Candidate::Bundled(tool("Morka Riflex"))],
                more: 0
            }
        );
        // Two wrong: 81.8, under the floor.
        assert_eq!(
            resolve(&read("Morka Rlflax", false), &one, &p),
            unknown("Morka Rlflax")
        );
        // Two tools as near as each other: both offered.
        let two = of(&["Morka Riflex", "Morka Riflax"]);
        match resolve(&read("Morka Riflox", false), &two, &p) {
            ToolMatch::Contended { candidates, more } => {
                assert_eq!(
                    candidates,
                    vec![
                        Candidate::Bundled(tool("Morka Riflex")),
                        Candidate::Bundled(tool("Morka Riflax"))
                    ]
                );
                assert_eq!(more, 0);
            }
            other => panic!("{other:?}"),
        }
        // Spelled exactly, the near twin does not matter.
        assert_eq!(
            bundled_name(&resolve(&read("Morka Riflax", false), &two, &p)),
            Some("Morka Riflax")
        );
        // Several tools spelled exactly contend.
        let alike = ToolCatalogue::new(
            Vec::new(),
            vec![
                tool("Morka Riflex"),
                BundledTool {
                    endpoint: "medical_tools".into(),
                    ..tool("Morka Riflex")
                },
            ],
            Vec::new(),
        );
        assert!(matches!(
            resolve(&read("Morka Riflex", false), &alike, &p),
            ToolMatch::Contended { candidates, more: 0 } if candidates.len() == 2
        ));
    }

    #[test]
    fn an_item_missing_from_the_bundled_list_is_never_named_as_its_neighbour() {
        // Toggling the `(L)` marker over real names: an item one step from
        // a listed one is not that one.
        let p = profile();
        for (missing, listed) in [
            (
                "Hyperion Aurora TC-1, Improved",
                "Hyperion Aurora TC-1, Improved (L)",
            ),
            ("Sollomate Opalo (L)", "Sollomate Opalo"),
        ] {
            assert!(bundled().bundled().any(|t| t.name == listed), "{listed}");
            assert!(!bundled().bundled().any(|t| t.name == missing), "{missing}");
            let found = resolve(&read(missing, false), bundled(), &p);
            assert_eq!(bundled_name(&found), None, "{missing}: {found:?}");
        }
    }

    /// The worst cases the library's margin is set by: a five- and a
    /// six-character name read with one character wrong, beside a tool one
    /// step from the read. The wrong tool leads by 10.9 and 9.0: refused at
    /// the profile's margin, taken at a margin just under each lead.
    #[test]
    fn a_library_rival_ahead_by_less_than_the_margin_is_refused_however_near() {
        let p = profile();
        for (right, wrong, text) in [
            ("Bolar", "Bolxar", "Bolxr"),
            ("Bolarq", "Bolxraq", "Bolxrq"),
        ] {
            let key = label_key(text);
            let right_score = score(&key, false, &label_key(right), &p).unwrap();
            let wrong_score = score(&key, false, &label_key(wrong), &p).unwrap();
            assert!(wrong_score >= p.library.floor, "{wrong_score}");
            let lead = wrong_score - right_score;
            assert!(lead > 8.9, "{lead}");
            let catalogue = ToolCatalogue::new(
                vec![entry(1, right, true), entry(2, wrong, true)],
                Vec::new(),
                Vec::new(),
            );
            assert_eq!(
                resolve(&read(text, false), &catalogue, &p),
                unknown(text),
                "{text}"
            );
            let mut loose = p.clone();
            loose.library.margin = lead - 0.1;
            assert_eq!(
                resolve(&read(text, false), &catalogue, &loose),
                ToolMatch::Library { id: 2 },
                "{text}"
            );
        }
    }

    /// The bound the margins are derived from (see [`TierProfile`]),
    /// recomputed from the profile: for every read length the tolerance
    /// and the floor allow, a read one substitution off the right tool,
    /// against any other tool the read does not spell (`steps` edit steps
    /// away, its compared key `len` long), never leaves the other tool at
    /// the floor and leading by the margin.
    #[test]
    fn the_profile_margins_hold_the_bound_one_wrong_character_sets() {
        let p = profile();
        for tier in [&p.library, &p.dismissed] {
            let mut tightest = f64::INFINITY;
            for n in p.min_letters.max(1)..200 {
                let n_f = n as f64;
                let right = 100.0 * (1.0 - 1.0 / n_f);
                for steps in 1..6usize {
                    for len in n.saturating_sub(steps).max(1)..=n + steps {
                        // Steps and the lengths' difference agree in
                        // parity, and cover it.
                        if (n + len + steps) % 2 != 0 || steps < n.abs_diff(len) {
                            continue;
                        }
                        let other = 100.0 * (1.0 - steps as f64 / (n + len) as f64);
                        if other >= tier.floor {
                            tightest = tightest.min(tier.margin - (other - right));
                        }
                    }
                }
            }
            assert!(tightest > 0.0, "{tier:?}: {tightest}");
        }
    }

    #[test]
    fn a_contention_lists_the_nearest_first_up_to_the_profiles_count() {
        let mut p = profile();
        p.max_candidates = 2;
        // A cut read each spells: those whose names run least past the
        // read first.
        let catalogue = of(&[
            "Opalo Combat Rifle Mark Ccc, Improved (L)",
            "Opalo Combat Rifle Mark Bb (L)",
            "Opalo Combat Rifle Mark D, Modified",
            "Opalo Combat Rifle Mark A",
        ]);
        match resolve(&read("Opalo Combat Rifle Mark", true), &catalogue, &p) {
            ToolMatch::Contended { candidates, more } => {
                assert_eq!(
                    candidates.iter().map(Candidate::name).collect::<Vec<_>>(),
                    vec![
                        "Opalo Combat Rifle Mark A",
                        "Opalo Combat Rifle Mark Bb (L)"
                    ]
                );
                assert_eq!(more, 2);
            }
            other => panic!("{other:?}"),
        }
        // Near reads: the nearest first.
        let catalogue = of(&[
            "Hyperion Aurora PP-1, Improved (L)",
            "Hyperion Aurora OC-1, Improved (L)",
            "Hyperion Aurora TC-1, Improved (L)",
            "Hyperion Aurora GR-1, Improved (L)",
        ]);
        match resolve(&read("Hyperion Aurora TO-1,Improv", true), &catalogue, &p) {
            ToolMatch::Contended { candidates, more } => {
                assert_eq!(
                    candidates.iter().map(Candidate::name).collect::<Vec<_>>(),
                    vec![
                        "Hyperion Aurora OC-1, Improved (L)",
                        "Hyperion Aurora TC-1, Improved (L)"
                    ]
                );
                assert_eq!(more, 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_bundled_tools_are_read_with_their_collection_and_id() {
        let weapons: Vec<Value> = serde_json::from_str(
            r#"[
                {"id": "a1", "name": " Kirov Blaster "},
                {"id": 17, "name": "Morka Rifle"},
                {"id": "b2", "name": "  "},
                {"name": "No Id"},
                {"id": "c3"}
            ]"#,
        )
        .unwrap();
        let catalogue =
            ToolCatalogue::from_entities(Vec::new(), [("weapons", weapons.as_slice())], Vec::new());
        assert_eq!(
            catalogue.bundled().cloned().collect::<Vec<_>>(),
            vec![
                BundledTool {
                    endpoint: "weapons".into(),
                    item_id: "a1".into(),
                    name: "Kirov Blaster".into()
                },
                BundledTool {
                    endpoint: "weapons".into(),
                    item_id: "17".into(),
                    name: "Morka Rifle".into()
                },
            ]
        );
        // From the store: every tool collection, and nothing else.
        let store = GameDataStore::new(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../entropia-orme/resources/snapshot"),
        )
        .unwrap();
        let catalogue = ToolCatalogue::from_store(&store, Vec::new(), Vec::new());
        let expected: usize = TOOL_ENDPOINTS
            .iter()
            .map(|e| store.get_entities(e).len())
            .sum();
        assert_eq!(catalogue.bundled().count(), expected);
        assert!(catalogue.bundled().any(|t| t.name == MAYHEM));
        assert!(catalogue
            .bundled()
            .all(|t| TOOL_ENDPOINTS.contains(&t.endpoint.as_str())));
    }

    #[test]
    fn entries_the_bar_cannot_tell_apart_are_grouped() {
        let p = profile();
        let entries = vec![
            entry(5, "Sollomate Opalo", true),
            entry(1, "sollomate  opalo", false),
            entry(2, MAYHEM, true),
            entry(
                8,
                "Exceptional Mayhem Electric Attack Chip 15, Modified (L)",
                true,
            ),
            entry(3, "Exceptional Mayhem Electric", true),
            entry(4, "Morka Rifle", true),
            entry(10, "EWE EP-40 Mercenary, Modified (L)", true),
            entry(11, "EWE EP-40 Mercenary, Modified, SGA Edition", false),
            entry(12, "Omegaton M2870, Ancient (range 38.5)", true),
            entry(13, "Omegaton M2870, Ancient (range 55.0)", true),
            // One entry kept twice under one id is one entry.
            entry(4, "Morka Rifle", true),
        ];
        assert_eq!(
            indistinguishable(&entries, &p),
            vec![vec![1, 5], vec![2, 8], vec![10, 11], vec![12, 13]]
        );
        // Names too short to be cut are told apart by their whole names.
        let short = vec![
            entry(1, "Morka Rifle Twenty Eight Lon", true),
            entry(2, "Morka Rifle Twenty Eight Lo", true),
        ];
        assert!(indistinguishable(&short, &p).is_empty());
        // Equal only over fewer characters than the profile's count: told
        // apart.
        let mut longer = p.clone();
        longer.twin_visible = 50;
        assert_eq!(indistinguishable(&entries, &longer), vec![vec![1, 5]]);
    }

    #[test]
    fn the_twin_count_flags_every_pair_of_bundled_names_the_bar_shows_alike() {
        // Over the real list: names the client cuts (longer than the
        // thirty-one characters it leaves) whose shown starts agree, or
        // agree over twenty-nine characters for a name cut a little
        // earlier, are all flagged.
        let p = profile();
        let names: Vec<String> = bundled().bundled().map(|t| t.name.clone()).collect();
        let entries: Vec<LibraryTool> = names
            .iter()
            .enumerate()
            .map(|(i, n)| entry(i as i64, n, true))
            .collect();
        let groups = indistinguishable(&entries, &p);
        let group_of = |id: usize| groups.iter().position(|g| g.contains(&(id as i64)));
        let mut checked = 0;
        for at in [29usize, 31] {
            let mut by: HashMap<(bool, Vec<char>), Vec<usize>> = HashMap::new();
            for (i, n) in names.iter().enumerate() {
                let chars: Vec<char> = n.chars().collect();
                by.entry((false, label_key(n))).or_default().push(i);
                if chars.len() > at {
                    by.entry((true, label_key(&chars[..at].iter().collect::<String>())))
                        .or_default()
                        .push(i);
                }
            }
            for alike in by.values().filter(|v| v.len() > 1) {
                for b in &alike[1..] {
                    checked += 1;
                    assert!(
                        group_of(alike[0]).is_some() && group_of(alike[0]) == group_of(*b),
                        "{:?} {:?}",
                        names[alike[0]],
                        names[*b]
                    );
                }
            }
        }
        assert!(checked > 100, "{checked}");
    }

    const TC1: &str = "Hyperion Aurora TC-1, Improved (L)";
    const OC1: &str = "Hyperion Aurora OC-1, Improved (L)";

    #[test]
    fn a_read_spelling_a_bundled_tool_is_not_taken_for_a_library_tool_it_is_near() {
        let p = profile();
        let library = bundled_with(vec![entry(3, TC1, true)], Vec::new());
        for (text, cut) in [("Hyperion Aurora OC-1,Improv", true), (OC1, false)] {
            let found = resolve(&read(text, cut), &library, &p);
            assert_eq!(bundled_name(&found), Some(OC1), "{text}: {found:?}");
        }
        // Spelling the library's tool, it is the library's.
        for (text, cut) in [("Hyperion Aurora TC-1,Improv", true), (TC1, false)] {
            assert_eq!(
                resolve(&read(text, cut), &library, &p),
                ToolMatch::Library { id: 3 },
                "{text}"
            );
        }
    }

    #[test]
    fn a_read_near_a_library_tool_and_its_bundled_siblings_alike_is_contended() {
        let p = profile();
        let library = bundled_with(vec![entry(3, TC1, true)], Vec::new());
        match resolve(&read("Hyperion Aurora TG-1,Improv", true), &library, &p) {
            ToolMatch::Contended { candidates, .. } => {
                assert!(
                    candidates.contains(&Candidate::Library {
                        ids: vec![3],
                        name: TC1.into()
                    }),
                    "{candidates:?}"
                );
                let names: Vec<&str> = candidates.iter().map(Candidate::name).collect();
                assert!(
                    names.contains(&"Hyperion Aurora GR-1, Improved (L)"),
                    "{names:?}"
                );
                // The bundled entry of the library tool's own name is that
                // tool, listed once, as the library's.
                assert_eq!(names.iter().filter(|n| **n == TC1).count(), 1, "{names:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_library_tool_with_no_bundled_tool_near_it_still_takes_a_near_read() {
        let p = profile();
        let library = bundled_with(
            vec![
                entry(7, MAYHEM, true),
                entry(8, "Teleportation Chip 1", true),
            ],
            Vec::new(),
        );
        for (text, cut, id) in [
            ("Exceptional MayhemBlectricAl", true, 7),
            ("Exceptional Mayhem ElectricAl", true, 7),
            ("Teleportatlon Chip 1", false, 8),
        ] {
            assert_eq!(
                resolve(&read(text, cut), &library, &p),
                ToolMatch::Library { id },
                "{text}"
            );
        }
    }

    #[test]
    fn a_near_dismissed_name_does_not_beat_a_tool_the_read_spells() {
        let p = profile();
        // `Morka Riflex` dismissed; the read spells `Morka Riflax`, which
        // the library holds, or else the bundled list.
        let in_library = ToolCatalogue::new(
            vec![entry(2, "Morka Riflax", true)],
            Vec::new(),
            vec!["Morka Riflex".into()],
        );
        assert_eq!(
            resolve(&read("Morka Riflax", false), &in_library, &p),
            ToolMatch::Library { id: 2 }
        );
        let bundled_only = ToolCatalogue::new(
            Vec::new(),
            vec![tool("Morka Riflax")],
            vec!["Morka Riflex".into()],
        );
        assert_eq!(
            bundled_name(&resolve(&read("Morka Riflax", false), &bundled_only, &p)),
            Some("Morka Riflax")
        );
        // Near both and spelling neither: the dismissed name does not
        // answer, nor can the bundled list.
        assert!(matches!(
            resolve(&read("Morka Riflox", false), &bundled_only, &p),
            ToolMatch::Unknown { .. }
        ));
        // Alone, a near read is the dismissed name.
        let alone = ToolCatalogue::new(Vec::new(), Vec::new(), vec!["Morka Riflex".into()]);
        assert_eq!(
            resolve(&read("Morka Riflox", false), &alone, &p),
            ToolMatch::NotTracked {
                name: "Morka Riflex".into()
            }
        );
    }

    #[test]
    fn tools_of_different_names_are_never_decided_by_which_is_enabled() {
        let p = profile();
        let library = bundled_with(
            vec![
                entry(1, "EMT Kit Ek-2600, Adjusted", true),
                entry(2, "EMT Kit Ek-2350, Adjusted", false),
            ],
            Vec::new(),
        );
        match resolve(&read("EMT Kit Ek-23S0, Adjusted", false), &library, &p) {
            ToolMatch::Contended { candidates, .. } => {
                for (ids, name) in [
                    (vec![2], "EMT Kit Ek-2350, Adjusted"),
                    (vec![1], "EMT Kit Ek-2600, Adjusted"),
                ] {
                    assert!(
                        candidates.contains(&Candidate::Library {
                            ids,
                            name: name.into()
                        }),
                        "{candidates:?}"
                    );
                }
            }
            other => panic!("{other:?}"),
        }
        // Spelled, the disabled entry is the answer, as disabled.
        assert_eq!(
            resolve(&read("EMT Kit Ek-2350, Adjusted", false), &library, &p),
            ToolMatch::Disabled { ids: vec![2] }
        );
    }

    #[test]
    fn one_entry_listed_twice_is_one_entry() {
        let p = profile();
        let catalogue = ToolCatalogue::new(
            vec![
                entry(4, "Sollomate Opalo", true),
                entry(4, "Sollomate Opalo", true),
            ],
            Vec::new(),
            Vec::new(),
        );
        for text in ["Sollomate Opalo", "Sollomate 0palo"] {
            assert_eq!(
                resolve(&read(text, false), &catalogue, &p),
                ToolMatch::Library { id: 4 },
                "{text}"
            );
        }
        let twins = ToolCatalogue::new(
            vec![
                entry(4, "Sollomate Opalo", true),
                entry(6, "Sollomate Opalo", true),
                entry(4, "Sollomate Opalo", true),
            ],
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(
            resolve(&read("Sollomate Opalo", false), &twins, &p),
            ToolMatch::Twins { ids: vec![4, 6] }
        );
    }

    #[test]
    fn a_near_library_tool_does_not_beat_a_dismissed_name_the_read_spells_or_is_as_near() {
        let p = profile();
        let catalogue = ToolCatalogue::new(
            vec![entry(1, "Morka Riflex", true)],
            Vec::new(),
            vec!["Morka Riflax".into()],
        );
        assert_eq!(
            resolve(&read("Morka Riflax", false), &catalogue, &p),
            ToolMatch::NotTracked {
                name: "Morka Riflax".into()
            }
        );
        assert_eq!(
            resolve(&read("Morka Riflox", false), &catalogue, &p),
            unknown("Morka Riflox")
        );
        // Spelled, the library's tool is the library's.
        assert_eq!(
            resolve(&read("Morka Riflex", false), &catalogue, &p),
            ToolMatch::Library { id: 1 }
        );
    }

    #[test]
    fn a_whole_name_that_starts_another_is_another_name() {
        let p = profile();
        let name =
            |text: &str| ToolCatalogue::new(vec![entry(1, text, true)], Vec::new(), Vec::new());
        let offered = ToolMatch::Contended {
            candidates: vec![Candidate::Library {
                ids: vec![1],
                name: "Kirov Blaster Rifle (L)".into(),
            }],
            more: 0,
        };
        assert_eq!(
            resolve(
                &read("Kirov Blaster Rifle", false),
                &name("Kirov Blaster Rifle (L)"),
                &p
            ),
            offered
        );
        assert_eq!(
            resolve(
                &read("Kirov Blaster Rifle (L)", false),
                &name("Kirov Blaster Rifle"),
                &p
            ),
            ToolMatch::Contended {
                candidates: vec![Candidate::Library {
                    ids: vec![1],
                    name: "Kirov Blaster Rifle".into(),
                }],
                more: 0,
            }
        );
        // Against the real list, which holds the name without the marker.
        let library = bundled_with(vec![entry(1, "Sollomate Opalo (L)", true)], Vec::new());
        assert_eq!(
            bundled_name(&resolve(&read("Sollomate Opalo", false), &library, &p)),
            Some("Sollomate Opalo")
        );
        let library = bundled_with(vec![entry(1, "Sollomate Opalo", true)], Vec::new());
        assert_ne!(
            resolve(&read("Sollomate Opalo (L)", false), &library, &p),
            ToolMatch::Library { id: 1 }
        );
    }

    /// The letters of a read, as `resolve` counts them.
    fn letters(text: &str) -> usize {
        text.chars().filter(|c| c.is_alphabetic()).count()
    }

    /// Which tool, by its collection and id, a single answer names.
    fn named(found: &ToolMatch) -> Option<(&str, &str)> {
        match found {
            ToolMatch::Bundled {
                endpoint, item_id, ..
            } => Some((endpoint.as_str(), item_id.as_str())),
            _ => None,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Any bundled name, read exactly to any length with enough
        /// letters, cut short or whole: that tool, or a contention it is
        /// in (or that lists only the nearest of more), never another tool
        /// and never unknown.
        #[test]
        fn an_exact_start_of_a_bundled_name_is_that_tool_or_contended(
            which in any::<Index>(),
            cut in any::<Index>(),
            dotted in any::<bool>(),
        ) {
            let p = profile();
            let catalogue = bundled();
            let own = &catalogue.bundled[which.index(catalogue.bundled.len())];
            let n = cut.index(own.key.len()) + 1;
            let text: String = own.key[..n].iter().collect();
            let cut_short = n < own.key.len() || dotted;
            prop_assume!(!cut_short || n >= p.min_cut_key);
            match resolve(&read(&text, cut_short), catalogue, &p) {
                ToolMatch::Contended { candidates, more } => {
                    prop_assert!(
                        candidates.contains(&Candidate::Bundled(own.tool.clone())) || more > 0,
                        "{text:?}"
                    );
                }
                found => prop_assert_eq!(
                    named(&found),
                    Some((own.tool.endpoint.as_str(), own.tool.item_id.as_str())),
                    "{:?}: {:?}", text, found
                ),
            }
        }

        /// The same with one character substituted: never another single
        /// tool, unless the read spells that tool exactly.
        #[test]
        fn one_wrong_character_never_names_another_bundled_tool(
            which in any::<Index>(),
            cut in any::<Index>(),
            at in any::<Index>(),
            dotted in any::<bool>(),
            wrong in "[A-Z0-9]",
        ) {
            let p = profile();
            let catalogue = bundled();
            let own = &catalogue.bundled[which.index(catalogue.bundled.len())];
            let n = cut.index(own.key.len()) + 1;
            let mut key = own.key[..n].to_vec();
            let at = at.index(n);
            let wrong = wrong.chars().next().unwrap();
            prop_assume!(key[at] != wrong);
            key[at] = wrong;
            let text: String = key.iter().collect();
            prop_assume!(letters(&text) >= p.min_letters);
            let cut_short = n < own.key.len() || dotted;
            prop_assume!(!catalogue.bundled.iter().any(|k| spells(&key, cut_short, &k.key)));
            let found = resolve(&read(&text, cut_short), catalogue, &p);
            if let Some(tool) = named(&found) {
                prop_assert_eq!(
                    tool,
                    (own.tool.endpoint.as_str(), own.tool.item_id.as_str()),
                    "{:?} from {:?}", text, own.tool.name
                );
            }
        }

        /// With a library holding a bundled tool (the read's own, or one
        /// listed near it), one character substituted never makes the read
        /// that library tool unless the unchanged read spelled it, the
        /// changed read spells it, or no tool of another name comes within
        /// the margin of it.
        #[test]
        fn one_wrong_character_never_names_a_library_tool_another_comes_near(
            which in any::<Index>(),
            offset in -3i64..=3,
            cut in any::<Index>(),
            at in any::<Index>(),
            dotted in any::<bool>(),
            wrong in "[A-Z0-9]",
        ) {
            let p = profile();
            let all = &bundled().bundled;
            let own_at = which.index(all.len());
            let own = &all[own_at];
            let held = &all[(own_at as i64 + offset).clamp(0, all.len() as i64 - 1) as usize];
            let catalogue = bundled_with(vec![entry(1, &held.tool.name, true)], Vec::new());
            let n = cut.index(own.key.len()) + 1;
            let right = own.key[..n].to_vec();
            let mut key = right.clone();
            let at = at.index(n);
            let wrong = wrong.chars().next().unwrap();
            prop_assume!(key[at] != wrong);
            key[at] = wrong;
            let text: String = key.iter().collect();
            prop_assume!(letters(&text) >= p.min_letters);
            let cut_short = n < own.key.len() || dotted;
            let found = resolve(&read(&text, cut_short), &catalogue, &p);
            if found == (ToolMatch::Library { id: 1 }) {
                let library = score(&key, cut_short, &held.key, &p).unwrap_or(0.0);
                let rivalled = all.iter().any(|k| {
                    k.key != held.key
                        && score(&key, cut_short, &k.key, &p)
                            .is_some_and(|s| library - s < p.library.margin)
                });
                prop_assert!(
                    spells(&right, cut_short, &held.key)
                        || spells(&key, cut_short, &held.key)
                        || !rivalled,
                    "{:?} from {:?} taken for {:?}", text, own.tool.name, held.tool.name
                );
            }
        }

        /// A library holding a read's tool disabled and a tool listed near
        /// it enabled: with one character substituted, the read is never
        /// the enabled tool unless it spells it, or the unchanged read
        /// did (the two show alike).
        #[test]
        fn a_disabled_tool_read_with_one_character_wrong_is_never_its_enabled_sibling(
            which in any::<Index>(),
            offset in 1i64..=3,
            below in any::<bool>(),
            cut in any::<Index>(),
            at in any::<Index>(),
            dotted in any::<bool>(),
            wrong in "[A-Z0-9]",
        ) {
            let p = profile();
            let all = &bundled().bundled;
            let own_at = which.index(all.len());
            let own = &all[own_at];
            let step = if below { -offset } else { offset };
            let sibling = &all[(own_at as i64 + step).clamp(0, all.len() as i64 - 1) as usize];
            prop_assume!(sibling.key != own.key);
            let catalogue = bundled_with(
                vec![entry(1, &sibling.tool.name, true), entry(2, &own.tool.name, false)],
                Vec::new(),
            );
            let n = cut.index(own.key.len()) + 1;
            let right = own.key[..n].to_vec();
            let mut key = right.clone();
            let at = at.index(n);
            let wrong = wrong.chars().next().unwrap();
            prop_assume!(key[at] != wrong);
            key[at] = wrong;
            let text: String = key.iter().collect();
            let cut_short = n < own.key.len() || dotted;
            let found = resolve(&read(&text, cut_short), &catalogue, &p);
            prop_assert!(
                found != (ToolMatch::Library { id: 1 })
                    || spells(&right, cut_short, &sibling.key)
                    || spells(&key, cut_short, &sibling.key),
                "{:?} from {:?} taken for {:?}", text, own.tool.name, sibling.tool.name
            );
        }

        /// Case, spacing and punctuation in a read never change the
        /// answer (an unknown read keeps its text as read).
        #[test]
        fn the_answer_ignores_case_spacing_and_punctuation(
            which in any::<Index>(),
            cut in any::<Index>(),
            dotted in any::<bool>(),
            noise in proptest::collection::vec((any::<bool>(), "[ ,.()'-]{0,2}"), 64),
        ) {
            let p = profile();
            let catalogue = bundled();
            let own = &catalogue.bundled[which.index(catalogue.bundled.len())];
            let chars: Vec<char> = own.tool.name.chars().collect();
            let n = cut.index(chars.len()) + 1;
            let plain: String = chars[..n].iter().collect();
            let noisy: String = chars[..n]
                .iter()
                .zip(noise.iter().cycle())
                .map(|(c, (upper, extra))| {
                    let c = if *upper { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() };
                    let c = if c.is_whitespace() { String::new() } else { c.to_string() };
                    format!("{c}{extra}")
                })
                .collect();
            let plain_found = resolve(&read(&plain, dotted), catalogue, &p);
            let noisy_found = resolve(&read(&noisy, dotted), catalogue, &p);
            let without_text = |found: ToolMatch| match found {
                ToolMatch::Unknown { .. } => unknown(""),
                other => other,
            };
            prop_assert_eq!(without_text(plain_found), without_text(noisy_found), "{:?}", noisy);
        }

        /// Two entries are in one group exactly when the bar cannot tell
        /// them apart, which is symmetric; and no entry is in two groups.
        #[test]
        fn indistinguishable_groups_are_symmetric_and_disjoint(
            names in proptest::collection::vec(
                (0usize..3, "[a-c ]{0,6}"),
                1..12,
            ),
        ) {
            let p = profile();
            let stems = [
                "Exceptional Mayhem Electric Att",
                "Exceptional Mayhem Electr",
                "Kirov",
            ];
            let entries: Vec<LibraryTool> = names
                .iter()
                .enumerate()
                .map(|(i, (stem, tail))| entry(i as i64, &format!("{}{tail}", stems[*stem]), true))
                .collect();
            let groups = indistinguishable(&entries, &p);
            let group_of = |id: i64| groups.iter().position(|g| g.contains(&id));
            for group in &groups {
                prop_assert!(group.len() > 1);
            }
            let held: usize = groups.iter().map(Vec::len).sum();
            let distinct: std::collections::BTreeSet<i64> = groups.iter().flatten().copied().collect();
            prop_assert_eq!(held, distinct.len());
            let n = p.twin_visible;
            let start = |name: &str| {
                let chars: Vec<char> = name.trim().chars().collect();
                (chars.len() > n).then(|| label_key(&chars[..n].iter().collect::<String>()))
            };
            let alike = |a: &LibraryTool, b: &LibraryTool| {
                label_key(&a.name) == label_key(&b.name)
                    || (start(&a.name).is_some() && start(&a.name) == start(&b.name))
            };
            for a in &entries {
                for b in &entries {
                    if a.id != b.id && alike(a, b) {
                        prop_assert!(
                            group_of(a.id).is_some() && group_of(a.id) == group_of(b.id),
                            "{:?} {:?}", a.name, b.name
                        );
                    }
                }
            }
            // Every group is held together by entries alike in pairs.
            for group in &groups {
                let mut reached = vec![group[0]];
                let mut grew = true;
                while grew {
                    grew = false;
                    for id in group {
                        if reached.contains(id) {
                            continue;
                        }
                        let entry = &entries[*id as usize];
                        if reached.iter().any(|r| alike(&entries[*r as usize], entry)) {
                            reached.push(*id);
                            grew = true;
                        }
                    }
                }
                prop_assert_eq!(reached.len(), group.len(), "{:?}", group);
            }
        }
    }
}
