//! Replays of the interface readers over a corpus of real captures,
//! shared by the test binary that pins them and by the maintainer tool
//! that tries a profile under edit against a whole corpus.
//!
//! A corpus is a directory of full-screen captures (`frames/*.png`), the
//! values on them transcribed independently by two people and reconciled
//! (`labels.json`), and what was measured on each by hand (`manifest.json`:
//! its windows' rectangles, the locked nameplate's bar, and whether the
//! frame is held out). Every replay holds a reader to one rule: a value is
//! read exactly as it was on screen, or it is refused; and each prints a
//! tally of what was read exactly, what was refused and what was WRONG.
//!
//! Real gameplay screens stay out of the public tree, so nothing here
//! names where a corpus is: the caller says. What is here of the corpus
//! these replays were written against is what a refusal needs to be
//! excused: which frame, which refusal, and why.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eo_services::game_data_store::GameDataStore;
use eo_services::game_ui::field::Recogniser;
use eo_services::game_ui::geometry::{BgrImage, Rect};
use eo_services::game_ui::locate::{read_window_field, ReadError};
use eo_services::game_ui::mob_names::{resolve, MobCatalogue, Resolved};
use eo_services::game_ui::plates::{
    find_locked_plate, find_plates, read_locked_plate, PlateScratch, PlateTextError,
};
use eo_services::game_ui::profile::UiProfile;
use eo_services::game_ui::radar::{
    read_coordinates, read_planet_label, CoordError, CoordinateBounds,
};
use eo_services::game_ui::sale_queue::{
    read_sale_queue, Buyout, ItemCatalogue, ItemName, QueueCount, QueuedItem, RowRefusal,
    SaleQueue, SaleQueueError, Setting, ITEM_ENDPOINTS,
};
use eo_services::game_ui::seams::{frame_reader, GameFrame};
use eo_services::game_ui::skills::{
    read_skills_page, RowName, SkillRow, SkillVocabulary, SkillsError, SkillsPage,
};
use eo_services::game_ui::window::find_windows;
use eo_services::ocr_engine::load_bgr_png;
use eo_services::planet_maps::PlanetMapStore;
use eo_services::repair_ocr::{RepairOcrService, RepairProviders, Terminal, TerminalRefusal};
use serde_json::Value;

/// The application's bundled resources: the interface profile, the
/// recogniser and its runtime, and the game data the readers confirm
/// names by.
pub fn resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../entropia-orme/resources")
}

/// A corpus of captures, with its labels and its manifest.
pub struct Corpus {
    dir: PathBuf,
    pub labels: Value,
    manifest: Value,
    /// Whether the frames replayed are the held-out ones or the rest.
    held_out: bool,
}

impl Corpus {
    /// Open the corpus at `dir`. Frames the manifest marks held out are
    /// replayed only when `held_out` asks for them, and then alone: they
    /// exist to test, once, that the readers generalise to an interface
    /// scale nothing was tuned on.
    pub fn open(dir: &Path, held_out: bool) -> Result<Corpus, String> {
        let json = |name: &str| -> Result<Value, String> {
            let path = dir.join(name);
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
        };
        Ok(Corpus {
            labels: json("labels.json")?,
            manifest: json("manifest.json")?,
            dir: dir.to_path_buf(),
            held_out,
        })
    }

    fn replayed(&self, name: &str) -> bool {
        let held_out = self.manifest["frames"][name]["held_out"]
            .as_bool()
            .unwrap_or(false);
        held_out == self.held_out
    }

    /// The frames to replay, with their labels.
    pub fn frames(&self) -> Vec<(String, &Value)> {
        self.labels["frames"]
            .as_object()
            .expect("frames")
            .iter()
            .filter(|(name, _)| self.replayed(name))
            .map(|(name, labels)| (name.clone(), labels))
            .collect()
    }

    /// One frame's pixels.
    pub fn frame(&self, name: &str) -> BgrImage {
        let path = self.dir.join("frames").join(format!("{name}.png"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        let (data, h, w) = load_bgr_png(&bytes).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        BgrImage { data, h, w }
    }
}

/// What a replay runs with: the corpus, the profile under test and the
/// recogniser.
pub struct Replay {
    pub corpus: Corpus,
    pub profile: UiProfile,
    pub read: Arc<dyn Recogniser>,
}

impl Replay {
    /// The recogniser as a reader hands a crop to it.
    pub fn reader(&self) -> impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync {
        let read = frame_reader(self.read.clone());
        move |image: &BgrImage| read(image)
    }
}

/// Which part of the corpus one call replays: every frame, or one share
/// of them. An unoptimised build takes seconds a frame, so the tests
/// replay in shares that each finish well inside the runner's ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shard(pub Option<(usize, usize)>);

impl Shard {
    fn holds(self, index: usize) -> bool {
        self.0.is_none_or(|(shard, of)| index % of == shard)
    }
}

impl std::fmt::Display for Shard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some((shard, _)) => write!(f, ", shard {shard}"),
            None => Ok(()),
        }
    }
}

/// What a replay came to: how many values read exactly, and everything
/// that fails it. A wrong value accepted always fails; so does a refusal
/// the labels do not justify and the replay has no stated reason for.
#[derive(Debug, Default)]
pub struct Outcome {
    pub exact: usize,
    pub failures: Vec<String>,
}

/// A labelled field of a window, and the terminal read that answers it:
/// where it is in the profile, and where in a frame's labels.
#[derive(Debug, Clone, Copy)]
pub struct LabelledField {
    pub window: &'static str,
    pub field: &'static str,
    pub surface: &'static str,
    pub key: &'static str,
    /// What the field's tally is called,
    pub what: &'static str,
    pub terminal: Terminal,
    /// and the tally of the terminal read.
    pub terminal_what: &'static str,
}

/// The repair terminal's cost.
pub const REPAIR_COST: LabelledField = LabelledField {
    window: "repair_terminal",
    field: "repair_cost",
    surface: "repair",
    key: "repair_cost",
    what: "repair cost",
    terminal: Terminal::Repair,
    terminal_what: "repair terminal read",
};

/// The trade terminal's total.
pub const TRADE_TOTAL: LabelledField = LabelledField {
    window: "trade_terminal",
    field: "total",
    surface: "trade",
    key: "total",
    what: "trade terminal total",
    terminal: Terminal::Trade,
    terminal_what: "trade terminal read",
};

/// The window locator against the rectangles the manifest gives, each
/// measured or checked by hand: a window whose extent is certain is found
/// within two pixels, one with a hidden side is never reported as
/// certain, and nothing else is found. Needs no recogniser.
pub fn windows(corpus: &Corpus, profile: &UiProfile, shard: Shard) -> Outcome {
    let chrome = &profile.chrome;
    let frames = corpus.manifest["frames"].as_object().expect("frames");
    let (mut expected_total, mut matched, mut false_locks, mut refused_ok) = (0, 0, 0, 0);
    let mut failures = Vec::new();
    for (index, (name, entry)) in frames.iter().enumerate() {
        if !shard.holds(index) || !corpus.replayed(name) {
            continue;
        }
        let image = corpus.frame(name);
        let found = find_windows(&image, chrome);
        let expected: Vec<(Rect, bool)> = entry["windows"]
            .as_array()
            .expect("windows")
            .iter()
            .map(|w| {
                let r: Vec<i32> = w["rect"]
                    .as_array()
                    .expect("rect")
                    .iter()
                    .map(|v| v.as_i64().expect("a coordinate") as i32)
                    .collect();
                (
                    Rect::new(r[0], r[1], r[2], r[3]),
                    w["extent_certain"].as_bool().unwrap_or(true),
                )
            })
            .collect();
        let close = |a: &Rect, b: &Rect| {
            (a.x0 - b.x0).abs() <= 2
                && (a.y0 - b.y0).abs() <= 2
                && (a.x1 - b.x1).abs() <= 2
                && (a.y1 - b.y1).abs() <= 2
        };
        for (rect, certain) in &expected {
            expected_total += 1;
            if *certain {
                if found
                    .iter()
                    .any(|f| f.extent_certain() && close(&f.rect, rect))
                {
                    matched += 1;
                } else {
                    failures.push(format!(
                        "{name}: expected {rect:?} not found in {:?}",
                        found.iter().map(|f| f.rect).collect::<Vec<_>>()
                    ));
                }
            } else if found.iter().any(|f| {
                f.extent_certain() && !expected.iter().any(|(e, c)| *c && close(&f.rect, e))
            }) {
                failures.push(format!(
                    "{name}: a window with a hidden side was reported as certain"
                ));
            } else {
                refused_ok += 1;
            }
        }
        for f in found.iter().filter(|f| f.extent_certain()) {
            if !expected.iter().any(|(e, _)| close(&f.rect, e)) {
                false_locks += 1;
                failures.push(format!("{name}: false window {:?}", f.rect));
            }
        }
    }
    println!(
        "corpus windows{shard}: {expected_total} expected, {matched} found within 2 px, {refused_ok} correctly reported uncertain, {false_locks} false"
    );
    Outcome {
        exact: matched,
        failures,
    }
}

/// What became of the labelled values of one surface.
#[derive(Default)]
struct Tally {
    exact: usize,
    refused: Vec<String>,
    wrong: Vec<String>,
}

impl Tally {
    /// Print the tally, and carry it into `outcome`: every wrong value
    /// is a failure.
    fn report(&self, what: &str, outcome: &mut Outcome) {
        println!(
            "{what}: {} read exactly, {} refused, {} WRONG",
            self.exact,
            self.refused.len(),
            self.wrong.len()
        );
        for line in &self.refused {
            println!("  refused: {line}");
        }
        outcome.exact += self.exact;
        outcome.failures.extend(
            self.wrong
                .iter()
                .map(|line| format!("{what}: a wrong value was accepted: {line}")),
        );
    }

    /// Report a surface every frame of which shows its value in full: a
    /// refusal there is a failure too.
    fn finish_unrefused(&self, what: &str) -> Outcome {
        let mut outcome = Outcome::default();
        self.report(what, &mut outcome);
        outcome.failures.extend(
            self.refused.iter().map(|line| {
                format!("{what}: a value the screen shows in full was refused: {line}")
            }),
        );
        outcome
    }
}

/// Replay one labelled field of one window kind over every frame whose
/// labels carry it. Every frame that shows the field shows it in full,
/// so a refusal is a failure.
pub fn field(r: &Replay, labelled: &LabelledField) -> Outcome {
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let LabelledField {
        window,
        field,
        surface,
        key: label_key,
        ..
    } = *labelled;
    let mut tally = Tally::default();
    for (name, labels) in corpus.frames() {
        let Some(want) = labels
            .get(surface)
            .and_then(|s| s.get(label_key))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let image = corpus.frame(&name);
        match read_window_field(&image, profile, window, field, &read) {
            Ok(got) if got.parsed.text == want => tally.exact += 1,
            Ok(got) => tally.wrong.push(format!(
                "{name}: read {:?}, the screen shows {want:?}",
                got.parsed.text
            )),
            Err(ReadError::NoRecogniser) => panic!("the recogniser stopped answering"),
            Err(error) => tally.refused.push(format!("{name} ({want}): {error}")),
        }
    }
    tally.finish_unrefused(labelled.what)
}

/// Replay one terminal's figure through the service the app reads it
/// with: the whole frame handed over as the game window, the bundled
/// profile, the bundled recogniser. The amount it answers must be the
/// labelled figure's own number, not merely a well-formed one.
pub fn terminal(r: &Replay, labelled: &LabelledField) -> Outcome {
    let corpus = &r.corpus;
    let profile = Arc::new(r.profile.clone());
    let LabelledField {
        terminal,
        surface,
        key: label_key,
        ..
    } = *labelled;
    let mut tally = Tally::default();
    for (name, labels) in corpus.frames() {
        let Some(want) = labels
            .get(surface)
            .and_then(|s| s.get(label_key))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let image = corpus.frame(&name);
        let read = r.reader();
        let service = RepairOcrService::new(RepairProviders {
            // The terminal reads ask for the whole window.
            frame: Arc::new(move |_| Ok(GameFrame::whole(image.clone(), (0, 0)))),
            profile: Some(profile.clone()),
            read_text: Some(Arc::new(read)),
        });
        let printed = want
            .strip_suffix(" PED")
            .unwrap_or_else(|| panic!("{name}: the label {want:?} is not an amount of PED"));
        let number: f64 = printed
            .replace(',', "")
            .parse()
            .unwrap_or_else(|e| panic!("{name}: the label {want:?} is not an amount: {e}"));
        let got = match terminal {
            Terminal::Repair => service.scan_repair_cost(),
            Terminal::Trade => service.scan_trade_terminal_value(),
        };
        match got {
            Ok(got) if got.text == printed && got.value_ped == number => tally.exact += 1,
            Ok(got) => tally.wrong.push(format!(
                "{name}: read {:?} as {}, the screen shows {want:?}",
                got.text, got.value_ped
            )),
            Err(TerminalRefusal::ReaderUnavailable) => {
                panic!("the terminal reader is unavailable")
            }
            Err(refusal) => tally.refused.push(format!("{name} ({want}): {refusal}")),
        }
    }
    tally.finish_unrefused(labelled.terminal_what)
}

/// The skill names and categories the application ships.
pub fn skill_vocabulary() -> SkillVocabulary {
    let path = resources().join("snapshot/skills.json");
    let entities: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}")),
    )
    .unwrap_or_else(|e| panic!("{path:?}: {e}"));
    SkillVocabulary::from_entities(&entities)
}

/// Whether a skill named `name` is the row the labels call `label`. A
/// label ending in an ellipsis is the client's cut name, which reads as
/// the one skill it begins; an empty label is the row the client draws
/// without a name.
pub fn same_skill(name: Option<&str>, label: &str) -> bool {
    match (name, label.strip_suffix("...")) {
        (None, _) => label.is_empty(),
        (Some(name), Some(stem)) => name.starts_with(stem) && name.len() > stem.len(),
        (Some(name), None) => name == label,
    }
}

/// Whether a row read is the row the labels call `label`: a known skill
/// by its name, a skill the vocabulary lacks by exactly the text the
/// screen shows.
pub fn row_is(row: &SkillRow, label: &str) -> bool {
    match &row.name {
        RowName::Known { name, .. } => same_skill(Some(name), label),
        RowName::Blank => label.is_empty(),
        RowName::Unknown => row.raw_name.trim() == label,
    }
}

/// The frames whose labels carry the skills table, less the held-out
/// ones, in shard `shard`.
fn skills_frames(corpus: &Corpus, shard: Shard) -> Vec<(String, Value)> {
    corpus
        .frames()
        .into_iter()
        .filter(|(_, labels)| labels.get("skills").is_some())
        .enumerate()
        .filter(|(i, _)| shard.holds(*i))
        .map(|(_, (name, labels))| (name, labels["skills"].clone()))
        .collect()
}

/// Frames that show their page whole. A page is whole only where every
/// labelled row is fully visible and the table's scrollbar shows the
/// whole list in view: no thumb (f13, f15, f62 to f74). Elsewhere a
/// thumb shorter than its track shows rows out of view, even where every
/// row in view is whole: f48 (a 280-pixel thumb on a 440-pixel track)
/// and f16 (445 on 548) both show the Combat category's second page,
/// which the thumbs put at eleven rows, nine of them in f16's view and
/// seven in f48's; f05, f06 and f14 show part of their page with a thumb
/// as well, and the rest have rows cut by the view.
const WHOLE_FRAMES: [&str; 15] = [
    "f13", "f15", "f62", "f63", "f64", "f65", "f66", "f67", "f68", "f69", "f70", "f71", "f72",
    "f73", "f74",
];

/// Compare one page's reading with its labels, row by row, for whether
/// it calls itself whole, and for the bar above the table.
fn score_skills_page(
    name: &str,
    labels: &Value,
    page: &SkillsPage,
    rows: &mut Tally,
    bar: &mut Tally,
) {
    let labelled = labels["rows"].as_array().expect("rows");
    let readable = |row: &Value| {
        row["fully_visible"].as_bool() == Some(true)
            && row["name"].is_string()
            && row["points"].is_string()
    };
    let should_be_whole = WHOLE_FRAMES.contains(&name);
    if page.whole != should_be_whole {
        rows.wrong.push(format!(
            "{name}: the page reads as whole {}, the screen shows it whole {should_be_whole}",
            page.whole
        ));
    }
    if should_be_whole && !labelled.iter().all(readable) {
        rows.wrong.push(format!(
            "{name}: listed as whole, but its labels mark a row not fully visible"
        ));
    }
    let mut matched = vec![false; labelled.len()];
    for got in &page.rows {
        match &got.name {
            RowName::Known {
                name: skill,
                near_miss: true,
            } => println!(
                "  {name}: {:?} read as {skill:?} by a near miss",
                got.raw_name
            ),
            RowName::Unknown => println!(
                "  {name}: {:?} = {} is a skill the vocabulary lacks",
                got.raw_name, got.points_text
            ),
            _ => {}
        }
        let found = labelled
            .iter()
            .position(|row| row["name"].as_str().is_some_and(|l| row_is(got, l)));
        match found {
            Some(i) if matched[i] => rows.wrong.push(format!(
                "{name}: {:?} = {} matches a row already read",
                got.name, got.points_text
            )),
            Some(i) if !readable(&labelled[i]) => rows.wrong.push(format!(
                "{name}: read {:?} = {}, a row the screen does not show whole",
                got.name, got.points_text
            )),
            Some(i) if labelled[i]["points"].as_str() == Some(got.points_text.as_str()) => {
                matched[i] = true;
                rows.exact += 1;
            }
            Some(i) => rows.wrong.push(format!(
                "{name}: {:?} read {:?}, the screen shows {:?}",
                got.name, got.points_text, labelled[i]["points"]
            )),
            None => rows.wrong.push(format!(
                "{name}: read {:?} (raw {:?}) = {}, which is no row on the screen",
                got.name, got.raw_name, got.points_text
            )),
        }
    }
    for (i, row) in labelled.iter().enumerate() {
        if readable(row) && !matched[i] {
            rows.refused.push(format!(
                "{name}: {:?} = {} (unread rows: {:?})",
                row["name"], row["points"], page.unread
            ));
        }
    }
    let total_label = labels["total_text"].as_str().and_then(|t| {
        t.rsplit(' ')
            .next()
            .and_then(|digits| digits.parse::<u64>().ok())
    });
    let page_label = labels["page"].as_str().map(|p| p.replace(' ', ""));
    let fields = [
        (
            "total",
            total_label.map(|t| t.to_string()),
            page.total.map(|t| t.to_string()),
        ),
        (
            "page",
            page_label,
            page.page.map(|(i, n)| format!("{i}/{n}")),
        ),
        (
            "category",
            labels["category"].as_str().map(String::from),
            page.category.clone(),
        ),
    ];
    for (what, want, got) in fields {
        match (want, got) {
            (Some(want), Some(got)) if want == got => bar.exact += 1,
            (Some(want), Some(got)) => bar.wrong.push(format!(
                "{name}: {what} read {got:?}, the screen shows {want:?}"
            )),
            (Some(want), None) => bar.refused.push(format!("{name}: {what} {want:?}")),
            (None, Some(got)) => bar.wrong.push(format!(
                "{name}: {what} read {got:?} where the labels have none"
            )),
            (None, None) => {}
        }
    }
}

/// Frames the reader refuses although their labels show whole rows: the
/// refusal it must give, and why. Every other labelled row is required.
const SKILLS_REFUSALS: [(&str, SkillsError, &str); 3] = [
    (
        "f47",
        SkillsError::ExtentUncertain,
        "the desktop scaled the frame, so the window's extent is uncertain",
    ),
    (
        "f18",
        SkillsError::WindowNotFound,
        "the window runs off the frame's left edge, which cuts the start of its title (so \
         the window cannot be identified) and the names",
    ),
    (
        "f17",
        SkillsError::WindowNotFound,
        "the Inventory window covers the skills window's title, so the window cannot be \
         identified; the labels mark every row but one under the Inventory window",
    ),
];

/// Replay the skills table over every frame whose labels carry it: each
/// row's name and points, whether the page reads as whole, and the bar
/// above the table.
pub fn skills(r: &Replay, shard: Shard) -> Outcome {
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let vocabulary = skill_vocabulary();
    let (mut rows, mut bar) = (Tally::default(), Tally::default());
    for (name, labels) in skills_frames(corpus, shard) {
        let image = corpus.frame(&name);
        let excused = SKILLS_REFUSALS.iter().find(|(f, _, _)| *f == name);
        let labelled = labels["rows"].as_array().expect("rows");
        let readable = labelled
            .iter()
            .filter(|row| row["fully_visible"].as_bool() == Some(true) && row["name"].is_string())
            .count();
        match (
            read_skills_page(&image, profile, &vocabulary, &read),
            excused,
        ) {
            (Err(SkillsError::NoRecogniser), _) => panic!("the recogniser stopped answering"),
            (Ok(page), Some((_, _, why))) => rows
                .wrong
                .push(format!("{name}: read a page where {why}: {page:?}")),
            (Err(error), Some((_, want, why))) => {
                println!("  {name}: frame refused: {error} ({why})");
                if error != *want {
                    rows.wrong.push(format!(
                        "{name}: refused as {error:?}, where {why} ({want:?})"
                    ));
                }
                for _ in 0..readable {
                    rows.refused
                        .push(format!("{name}: frame refused ({error})"));
                }
            }
            (Err(error), None) => {
                println!("  {name}: frame refused: {error}");
                for _ in 0..readable {
                    rows.refused
                        .push(format!("{name}: frame refused ({error})"));
                }
            }
            (Ok(page), None) => {
                println!(
                    "  {name}: {} rows read, {} unread, {} partial, whole {}",
                    page.rows.len(),
                    page.unread.len(),
                    page.partial,
                    page.whole
                );
                score_skills_page(&name, &labels, &page, &mut rows, &mut bar);
            }
        }
    }
    let mut outcome = Outcome::default();
    rows.report(&format!("skills rows{shard}"), &mut outcome);
    bar.report(
        &format!("skills total, page and category{shard}"),
        &mut outcome,
    );
    let unexcused: Vec<&String> = rows
        .refused
        .iter()
        .filter(|line| {
            !SKILLS_REFUSALS
                .iter()
                .any(|(frame, _, _)| line.starts_with(&format!("{frame}:")))
        })
        .collect();
    outcome.failures.extend(
        unexcused
            .iter()
            .map(|line| format!("skills: a row the screen shows whole was refused: {line}")),
    );
    outcome.failures.extend(
        bar.refused
            .iter()
            .map(|line| format!("skills: the bar above the table was refused: {line}")),
    );
    outcome
}

/// Values the coordinates reader refuses although their labels show them:
/// the frame, the value, the refusal it must give, and why. Every other
/// labelled value is required to read exactly.
fn coord_refusals() -> Vec<(&'static str, &'static str, CoordError, &'static str)> {
    vec![
        (
            "f44",
            "lon",
            CoordError::Invalid {
                raw: "ON68520".into(),
            },
            "the Trade Terminal's right edge lies over the L of LON, which reads as ON",
        ),
        (
            "f46",
            "lon",
            CoordError::Invalid {
                raw: "ON68520".into(),
            },
            "the Trade Terminal's right edge lies over the L of LON, which reads as ON",
        ),
        (
            "f25",
            "lat",
            CoordError::NotFound,
            "pale sky through dark trees behind LAT joins the lettering into a run too tall \
             and too wide to stand aligned with the disc",
        ),
        (
            "f29",
            "lat",
            CoordError::NotFound,
            "sunlit foliage around LAT joins the lettering into a run taller than lettering",
        ),
        (
            "f53",
            "lon",
            CoordError::Invalid {
                raw: "LON.67835".into(),
            },
            "the recogniser read a speck of the sand between LON and the digits as a stop",
        ),
        (
            "f54",
            "lat",
            CoordError::Misplaced {
                raw: " LAT 87750".into(),
            },
            "a rock beside the last digit of LAT joins its lettering, so its end lies past the \
             disc's edge",
        ),
        (
            "f29",
            "lon",
            CoordError::NoSpace {
                raw: "LON 67707".into(),
            },
            "sunlit foliage left of LON joins the lettering, so the label is not where it is \
             predicted to end",
        ),
        (
            "f31",
            "lat",
            CoordError::NoSpace {
                raw: "LAT87925".into(),
            },
            "foliage bridges the space after LAT, so where the label ends cannot be found",
        ),
    ]
}

/// What a frame's labels say about one coordinate: `Some(Some(text))`
/// for a value on screen, `Some(None)` for one the labels mark covered,
/// and `None` when the labels have no coordinates (the radar is hidden).
fn coord_label(labels: &Value, key: &str) -> Option<Option<String>> {
    let coords = labels.get("coords").filter(|c| !c.is_null())?;
    Some(coords[key].as_str().map(String::from))
}

/// The coordinate range of Calypso, where every frame of the corpus was
/// captured, from the application's bundled planet maps.
fn calypso() -> CoordinateBounds {
    let store = PlanetMapStore::new(&resources().join("maps")).expect("the bundled maps load");
    let bounds = store
        .record("Calypso")
        .and_then(|record| record.calibration.as_ref())
        .expect("Calypso's map is calibrated")
        .bounds;
    CoordinateBounds {
        lon: (bounds.lon_min, bounds.lon_max),
        lat: (bounds.lat_min, bounds.lat_max),
    }
}

/// Every name the bundled planet maps give a planet, as the radar may print
/// it: each map's name and the game's own name for it.
fn planet_names() -> Vec<String> {
    let store = PlanetMapStore::new(&resources().join("maps")).expect("the bundled maps load");
    let mut names: Vec<String> = Vec::new();
    for record in store.records() {
        names.push(record.name.clone());
        names.extend(record.technical_name.clone());
    }
    names
}

/// The frames to replay for coordinates in shard `shard`.
fn coordinate_frames(corpus: &Corpus, shard: Shard) -> Vec<(String, &Value)> {
    corpus
        .frames()
        .into_iter()
        .enumerate()
        .filter(|(index, _)| shard.holds(*index))
        .map(|(_, frame)| frame)
        .collect()
}

/// Replay the coordinates under the radar, and the planet's name over
/// it, on every frame.
pub fn coordinates(r: &Replay, shard: Shard) -> Outcome {
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let bounds = calypso();
    let mut tally = Tally::default();
    let mut planet = Tally::default();
    let names = planet_names();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut covered = 0;
    let refusals = coord_refusals();
    let mut mismatched = Vec::new();
    for (name, labels) in coordinate_frames(corpus, shard) {
        let image = corpus.frame(&name);
        let got = read_coordinates(&image, profile, &bounds, &read);
        // Every frame was captured on Calypso: the name above the radar
        // reads as Calypso or as nothing, never as another planet.
        if let Ok(found) = &got {
            match read_planet_label(&image, &found.radar, profile, &names, &read) {
                Some(planet_name) if planet_name == "Calypso" => planet.exact += 1,
                Some(other) => planet
                    .wrong
                    .push(format!("{name}: the radar's planet read as {other:?}")),
                None => planet
                    .refused
                    .push(format!("{name}: the planet's name did not read")),
            }
        }
        if labels.get("coords").is_none_or(Value::is_null) {
            // The radar is hidden: under a maximised window in this corpus.
            match &got {
                Err(CoordError::RadarNotFound) => {
                    covered += 2;
                    println!("  {name}: no radar, as the labels say");
                }
                other => tally.wrong.push(format!(
                    "{name}: the radar is hidden, but the reader answered {other:?}"
                )),
            }
            continue;
        }
        for (key, label) in [("lon", "LON"), ("lat", "LAT")] {
            let excused = refusals.iter().find(|(f, k, _, _)| *f == name && *k == key);
            let want = coord_label(labels, key).expect("the frame has coordinates");
            let value = match &got {
                Ok(c) if key == "lon" => c.lon.clone(),
                Ok(c) => c.lat.clone(),
                Err(CoordError::NoRecogniser) => panic!("the recogniser stopped answering"),
                Err(e) => Err(e.clone()),
            };
            match (want, value) {
                (Some(want), Ok(v)) if v.text == format!("{label} {want}") => tally.exact += 1,
                (Some(want), Ok(v)) => tally.wrong.push(format!(
                    "{name}: {key} read {:?} (raw {:?}), the screen shows {want:?}",
                    v.text, v.raw
                )),
                (None, Ok(v)) => tally.wrong.push(format!(
                    "{name}: {key} read {:?}, a value the screen does not show whole",
                    v.text
                )),
                (None, Err(e)) => {
                    covered += 1;
                    println!("  {name}: {key} is covered, refused: {e}");
                }
                (Some(want), Err(e)) => match excused {
                    Some((_, _, expected, why)) if *expected == e => {
                        tally
                            .refused
                            .push(format!("{name}: {key} {want}: {e}; {why} (excused)"));
                    }
                    Some((_, _, expected, why)) => mismatched.push(format!(
                        "{name}: {key} {want} refused as {e:?}, where {why} ({expected:?})"
                    )),
                    None => tally.refused.push(format!("{name}: {key} {want}: {e}")),
                },
            }
        }
    }
    println!("coordinates{shard}: {covered} covered values refused as they must be");
    let mut outcome = Outcome::default();
    tally.report(&format!("coordinates{shard}"), &mut outcome);
    planet.report(&format!("planet name{shard}"), &mut outcome);
    let unexcused: Vec<&String> = tally
        .refused
        .iter()
        .filter(|line| !line.ends_with("(excused)"))
        .collect();
    outcome.failures.extend(
        unexcused
            .iter()
            .map(|line| format!("coordinates: a value the screen shows was refused: {line}")),
    );
    outcome.failures.extend(
        mismatched.iter().map(|line| {
            format!("coordinates: refused for another reason than the one known: {line}")
        }),
    );
    outcome
}

/// The recogniser's known failure applied to the coordinates: one of the
/// first pair of repeated digits in any reading is dropped.
fn collapsing_any(
    read: impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync,
) -> impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync {
    move |image: &BgrImage| {
        let (text, confidence) = read(image)?;
        let chars: Vec<char> = text.chars().collect();
        let Some(at) = chars
            .windows(2)
            .position(|pair| pair[0] == pair[1] && pair[0].is_ascii_digit())
        else {
            return Some((text, confidence));
        };
        let mut short = chars;
        short.remove(at);
        Some((short.into_iter().collect(), confidence))
    }
}

/// Whether a coordinate has the same digit twice running, which the
/// damaging recogniser merges.
fn has_repeat(value: &str) -> bool {
    value.as_bytes().windows(2).any(|pair| pair[0] == pair[1])
}

/// Every coordinate frame read with a recogniser that drops one of every
/// repeated pair of digits. A coordinate a digit short still parses, so
/// each damaged value must be refused by something else: its own width
/// against its label, the planet's bounds, or the comparison of the two
/// values. Every value returned must still be the one on screen.
pub fn coordinate_digit_loss(r: &Replay, shard: Shard) -> Outcome {
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = collapsing_any(r.reader());
    let bounds = calypso();
    let (mut damaged, mut whole_read, mut whole_refused) = (0, 0, 0);
    let mut by: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut wrong = Vec::new();
    for (name, labels) in coordinate_frames(corpus, shard) {
        if labels.get("coords").is_none_or(Value::is_null) {
            continue;
        }
        let image = corpus.frame(&name);
        let got = match read_coordinates(&image, profile, &bounds, &read) {
            Ok(got) => got,
            Err(CoordError::NoRecogniser) => panic!("the recogniser stopped answering"),
            Err(_) => continue,
        };
        for (key, label, value) in [("lon", "LON", &got.lon), ("lat", "LAT", &got.lat)] {
            let Some(want) = coord_label(labels, key).flatten() else {
                if let Ok(v) = value {
                    wrong.push(format!(
                        "{name}: {key} read {:?} where it is covered",
                        v.text
                    ));
                }
                continue;
            };
            let hit = has_repeat(&want);
            damaged += hit as usize;
            match value {
                Ok(v) if v.text == format!("{label} {want}") => whole_read += 1,
                Ok(v) => wrong.push(format!(
                    "{name}: {key} read {:?}, the screen shows {want}",
                    v.text
                )),
                Err(e) if hit => {
                    let how = match e {
                        CoordError::WrongWidth { .. } => "its own width",
                        CoordError::NoSpace { .. } => "its label not found",
                        CoordError::Misplaced { .. } => "its place",
                        CoordError::OutOfBounds { .. } => "the planet's bounds",
                        CoordError::WidthsDisagree { .. } => "the other value",
                        CoordError::Invalid { .. } => "the grammar",
                        _ => "a refusal before reading",
                    };
                    *by.entry(how).or_default() += 1;
                }
                Err(_) => whole_refused += 1,
            }
        }
    }
    println!(
        "digit loss{shard}: {damaged} values damaged, refused by {by:?}; {whole_read} undamaged read exactly, {whole_refused} undamaged refused; {} WRONG",
        wrong.len()
    );
    Outcome {
        exact: whole_read,
        failures: wrong
            .iter()
            .map(|line| format!("digit loss: a damaged coordinate was accepted: {line}"))
            .collect(),
    }
}

/// The fields of a queued row, as the labels name them, with the read
/// row's canonical text for each. A switched-off buyout carries no price
/// and is compared apart.
fn queued_fields(item: &QueuedItem) -> [(&'static str, Option<String>); 8] {
    [
        ("name", Some(item.name.text().to_string())),
        ("tt", Some(item.tt.text.clone())),
        ("quantity", Some(item.quantity.text.clone())),
        ("opening_bid", Some(item.opening_bid.text.clone())),
        (
            "buyout",
            match &item.buyout {
                Buyout::Off => None,
                Buyout::Price(price) => Some(price.text.clone()),
            },
        ),
        ("days", Some(item.days.text.clone())),
        ("fee", Some(item.fee.text.clone())),
        ("markup", Some(item.markup.text.clone())),
    ]
}

/// The labels' field names, in the order a row prints them.
const SALE_FIELDS: [&str; 8] = [
    "name",
    "tt",
    "quantity",
    "opening_bid",
    "buyout",
    "days",
    "fee",
    "markup",
];

/// The frames whose labels carry the sell queue, less the held-out ones,
/// in shard `shard`.
fn sale_frames(corpus: &Corpus, shard: Shard) -> Vec<(String, Value)> {
    corpus
        .frames()
        .into_iter()
        .filter(|(_, labels)| labels.get("sale_queue").is_some())
        .enumerate()
        .filter(|(i, _)| shard.holds(*i))
        .map(|(_, (name, labels))| (name, labels["sale_queue"].clone()))
        .collect()
}

/// The count the labels' heading text gives: `Sell Queue (3)` is 3.
fn labelled_count(labels: &Value) -> u32 {
    let text = labels["queue_text"].as_str().expect("queue text");
    text.rsplit_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{text:?} has no count"))
}

/// The spinners the client draws greyed, for every queued row of every
/// frame, as the labels' notes record them (f59 shows the same queue as
/// f58 and f60, its notes saying so). A greyed buyout is a switched-off
/// one. A row missing here fails the replay rather than being assumed
/// editable.
const SALE_LOCKED: [(&str, &str, &[&str]); 20] = [
    ("f55", "Electric Attack Nanochip 3", &["quantity", "buyout"]),
    ("f56", "Electric Attack Nanochip 3", &["quantity"]),
    ("f57", "Electric Attack Nanochip 3", &["quantity"]),
    ("f57", "Output Amplifier Component", &["buyout"]),
    ("f57", "Skyripper Loyalist (L)", &["quantity", "buyout"]),
    ("f58", "Electric Attack Nanochip 3", &["quantity"]),
    ("f58", "Eyeshadow (Gold)", &["buyout"]),
    ("f58", "Eyeshadow (Olive Drab)", &["buyout"]),
    ("f58", "Output Amplifier Component", &["buyout"]),
    ("f58", "Skyripper Loyalist (L)", &["quantity", "buyout"]),
    ("f59", "Electric Attack Nanochip 3", &["quantity"]),
    ("f59", "Eyeshadow (Gold)", &["buyout"]),
    ("f59", "Eyeshadow (Olive Drab)", &["buyout"]),
    ("f59", "Output Amplifier Component", &["buyout"]),
    ("f59", "Skyripper Loyalist (L)", &["quantity", "buyout"]),
    ("f60", "Electric Attack Nanochip 3", &["quantity"]),
    ("f60", "Eyeshadow (Gold)", &["buyout"]),
    ("f60", "Eyeshadow (Olive Drab)", &["buyout"]),
    ("f60", "Output Amplifier Component", &["buyout"]),
    ("f60", "Skyripper Loyalist (L)", &["quantity", "buyout"]),
];

/// The greyed spinners of one row; `None` where the table has no entry
/// for it.
fn sale_locked(frame: &str, row: &str) -> Option<&'static [&'static str]> {
    SALE_LOCKED
        .iter()
        .find(|(f, r, _)| *f == frame && *r == row)
        .map(|(_, _, locked)| *locked)
}

/// The labelled rows of `frame` the table of greyed spinners has no entry
/// for, each as a failure: a row is never assumed editable.
fn unlisted_sale_rows(frame: &str, labels: &Value) -> Vec<String> {
    labels["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["name"].as_str().expect("named"))
        .filter(|row| sale_locked(frame, row).is_none())
        .map(|row| {
            format!(
                "{frame}: {row:?} has no entry in the table of greyed spinners; add one from \
                 the frame's notes before replaying it"
            )
        })
        .collect()
}

/// The application's bundled item data, as the reader confirms names by.
fn item_catalogue() -> ItemCatalogue {
    let mut entities = Vec::new();
    for endpoint in ITEM_ENDPOINTS {
        let path = resources().join(format!("snapshot/{endpoint}.json"));
        let list: Vec<Value> = serde_json::from_str(
            &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}")),
        )
        .unwrap_or_else(|e| panic!("{path:?}: {e}"));
        entities.extend(list);
    }
    ItemCatalogue::from_entities(&entities)
}

/// Per field, what became of every labelled value in a fully visible row.
#[derive(Default)]
struct FieldTallies(std::collections::BTreeMap<&'static str, Tally>);

impl FieldTallies {
    fn field(&mut self, field: &'static str) -> &mut Tally {
        self.0.entry(field).or_default()
    }

    fn report(&self, what: &str, outcome: &mut Outcome) {
        for field in SALE_FIELDS {
            if let Some(tally) = self.0.get(field) {
                tally.report(&format!("{what} {field}"), outcome);
            }
        }
        for (field, tally) in &self.0 {
            if !SALE_FIELDS.contains(field) {
                tally.report(&format!("{what} {field}"), outcome);
            }
        }
    }
}

/// Compare one reading of the queue with its labels.
fn score_sale_queue(name: &str, labels: &Value, queue: &SaleQueue, fields: &mut FieldTallies) {
    let labelled = labels["rows"].as_array().expect("rows");
    let visible = |row: &Value| row["fully_visible"].as_bool() == Some(true);
    let mut matched = vec![false; labelled.len()];
    for item in &queue.rows {
        let found = labelled
            .iter()
            .position(|row| row["name"].as_str() == Some(item.name.text()));
        let i = match found {
            Some(i) if matched[i] => {
                fields
                    .field("name")
                    .wrong
                    .push(format!("{name}: {:?} read twice", item.name.text()));
                continue;
            }
            Some(i) if !visible(&labelled[i]) => {
                fields.field("name").wrong.push(format!(
                    "{name}: {:?} read, a row the screen does not show whole",
                    item.name.text()
                ));
                continue;
            }
            Some(i) => i,
            None => {
                fields.field("name").wrong.push(format!(
                    "{name}: read {:?}, which is no row on the screen ({item:?})",
                    item.name.text()
                ));
                continue;
            }
        };
        matched[i] = true;
        let locked = sale_locked(name, item.name.text()).unwrap_or_default();
        for (field, got) in queued_fields(item) {
            let want = labelled[i][field].as_str().expect("labelled");
            // A greyed buyout is switched off: its grey number is no
            // price, and must not be returned as one.
            let want = (field != "buyout" || !locked.contains(&"buyout")).then_some(want);
            if got.as_deref() == want {
                fields.field(field).exact += 1;
            } else {
                fields.field(field).wrong.push(format!(
                    "{name}: {} {field} read {got:?}, the screen shows {want:?}",
                    item.name.text()
                ));
            }
        }
        // Whether each spinner is greyed, as the screen shows it.
        for (field, setting) in [
            ("quantity", item.quantity.setting),
            ("opening_bid", item.opening_bid.setting),
            ("days", item.days.setting),
        ] {
            let want = if locked.contains(&field) {
                Setting::Locked
            } else {
                Setting::Editable
            };
            if setting == want {
                fields.field("spinner setting").exact += 1;
            } else {
                fields.field("spinner setting").wrong.push(format!(
                    "{name}: {} {field} read as {setting:?}, the screen shows it {want:?}",
                    item.name.text()
                ));
            }
        }
        match &item.name {
            ItemName::Catalogued(_) => fields.field("name confirmed by the catalogue").exact += 1,
            ItemName::Listed(_) => fields.field("name confirmed by the inventory list").exact += 1,
            ItemName::Unconfirmed(_) => fields.field("name unconfirmed").exact += 1,
        }
    }
    for (i, row) in labelled.iter().enumerate() {
        if !visible(row) || matched[i] {
            continue;
        }
        let label = row["name"].as_str().expect("named");
        let unread = queue
            .unread
            .iter()
            .find(|u| u.raw_name.as_deref().map(str::trim) == Some(label));
        let line = format!("{name}: {label} ({:?})", unread.map(|u| u.refusals.clone()));
        for field in SALE_FIELDS {
            fields.field(field).refused.push(line.clone());
        }
    }
}

/// Replay the auction's sell queue over every frame whose labels carry
/// it: each row field by field, which spinners are greyed, what bears
/// each name out, and the heading's count.
pub fn sale_queue(r: &Replay, shard: Shard) -> Outcome {
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let catalogue = item_catalogue();
    let mut fields = FieldTallies::default();
    let mut count = Tally::default();
    let mut outcome = Outcome::default();
    for (name, labels) in sale_frames(corpus, shard) {
        let unlisted = unlisted_sale_rows(&name, &labels);
        if !unlisted.is_empty() {
            outcome.failures.extend(unlisted);
            continue;
        }
        let image = corpus.frame(&name);
        let queue = match read_sale_queue(&image, profile, &catalogue, &read) {
            Ok(queue) => queue,
            Err(SaleQueueError::NoRecogniser) => panic!("the recogniser stopped answering"),
            Err(error) => {
                outcome
                    .failures
                    .push(format!("{name}: the queue was refused: {error}"));
                continue;
            }
        };
        println!(
            "  {name}: {} rows read, {} unread, {} partial, count {:?}",
            queue.rows.len(),
            queue.unread.len(),
            queue.partial,
            queue.count
        );
        let labelled = labels["rows"].as_array().expect("rows");
        let cut = labelled
            .iter()
            .filter(|row| row["fully_visible"].as_bool() != Some(true))
            .count();
        if queue.partial != cut {
            outcome.failures.push(format!(
                "{name}: {} rows read as partly in view, the screen shows {cut}",
                queue.partial
            ));
        }
        score_sale_queue(&name, &labels, &queue, &mut fields);
        let want = labelled_count(&labels);
        let whole = cut == 0 && labelled.len() == want as usize;
        match queue.count {
            QueueCount::Complete(n) if n == want && whole => count.exact += 1,
            QueueCount::Incomplete { heading, .. } if heading == want && !whole => {
                count.exact += 1
            }
            QueueCount::Unread => count.refused.push(format!("{name}: {want}")),
            other => count.wrong.push(format!(
                "{name}: the count reads {other:?}, the screen shows {want} with every row whole {whole}"
            )),
        }
    }
    fields.report(&format!("sale queue{shard},"), &mut outcome);
    count.report(&format!("sale queue count{shard}"), &mut outcome);
    let refused: std::collections::BTreeSet<&String> =
        fields.0.values().flat_map(|t| t.refused.iter()).collect();
    outcome.failures.extend(
        refused
            .iter()
            .map(|line| format!("sale queue: a row the screen shows whole was refused: {line}")),
    );
    outcome.failures.extend(
        count
            .refused
            .iter()
            .map(|line| format!("sale queue: the heading's count was refused: {line}")),
    );
    outcome
}

/// A recogniser that drops one of every repeated pair of digits in every
/// reading: `100.00%` reads as `10.0%`, `11.52 PED` as `1.52 PED`.
fn dropping_repeats(
    read: impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync,
) -> impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync {
    move |image: &BgrImage| {
        let (text, confidence) = read(image)?;
        Some((drop_repeated_digits(&text), confidence))
    }
}

fn drop_repeated_digits(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        out.push(chars[i]);
        let pair = i + 1 < chars.len() && chars[i].is_ascii_digit() && chars[i] == chars[i + 1];
        i += if pair { 2 } else { 1 };
    }
    out
}

/// Every sell-queue frame read with a recogniser that drops one of every
/// repeated pair of digits. A value a digit short still parses, so each
/// damaged value must be refused by something else: its width, or the
/// game's own figures disagreeing. Every row returned must still be the
/// one on screen, field for field, and rows with nothing to damage must
/// still read: the replay proves the refusals are the damage's, not the
/// reader's failing everything.
pub fn sale_queue_digit_loss(r: &Replay, shard: Shard) -> Outcome {
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = dropping_repeats(r.reader());
    let catalogue = item_catalogue();
    let (mut damaged, mut whole_read) = (0, 0);
    let mut by: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut wrong = Vec::new();
    let mut frames = 0;
    for (name, labels) in sale_frames(corpus, shard) {
        frames += 1;
        let unlisted = unlisted_sale_rows(&name, &labels);
        if !unlisted.is_empty() {
            wrong.extend(unlisted);
            continue;
        }
        let image = corpus.frame(&name);
        let queue = match read_sale_queue(&image, profile, &catalogue, &read) {
            Ok(queue) => queue,
            Err(SaleQueueError::NoRecogniser) => panic!("the recogniser stopped answering"),
            Err(error) => {
                wrong.push(format!("{name}: the queue was refused: {error}"));
                continue;
            }
        };
        let labelled = labels["rows"].as_array().expect("rows");
        for item in &queue.rows {
            let Some(row) = labelled
                .iter()
                .find(|row| row["name"].as_str() == Some(item.name.text()))
            else {
                wrong.push(format!(
                    "{name}: a row was read that is not on screen: {item:?}"
                ));
                continue;
            };
            let locked = sale_locked(&name, item.name.text()).unwrap_or_default();
            for (field, got) in queued_fields(item) {
                let want = row[field].as_str();
                let want = want.filter(|_| field != "buyout" || !locked.contains(&"buyout"));
                if got.as_deref() != want {
                    wrong.push(format!(
                        "{name}: {} {field} read {got:?}, the screen shows {want:?}",
                        item.name.text()
                    ));
                }
            }
            whole_read += 1;
        }
        for row in labelled {
            if row["fully_visible"].as_bool() != Some(true) {
                continue;
            }
            let label = row["name"].as_str().expect("named");
            let locked = sale_locked(&name, label).unwrap_or_default();
            // A switched-off buyout's number is never read, so its digits
            // are not damaged.
            let hit: Vec<&str> = SALE_FIELDS
                .iter()
                .filter(|f| **f != "buyout" || !locked.contains(&"buyout"))
                .filter(|f| row[**f].as_str().is_some_and(has_repeat_digit))
                .copied()
                .collect();
            let returned = queue.rows.iter().any(|item| item.name.text() == label);
            if hit.is_empty() {
                if !returned {
                    wrong.push(format!(
                        "{name}: {label} has nothing to damage, yet is unread"
                    ));
                }
                continue;
            }
            damaged += hit.len();
            if returned {
                wrong.push(format!("{name}: {label} was returned with {hit:?} damaged"));
                continue;
            }
            let Some(unread) = queue
                .unread
                .iter()
                .find(|u| u.raw_name.as_deref().map(str::trim) == Some(label))
            else {
                wrong.push(format!(
                    "{name}: {label} has {hit:?} damaged and was neither read nor refused"
                ));
                continue;
            };
            for refusal in &unread.refusals {
                let how = match refusal {
                    RowRefusal::WrongWidth { .. } => "its width",
                    RowRefusal::Disagrees(_) => "the game's own figures",
                    other => {
                        wrong.push(format!(
                            "{name}: {label} refused for another reason than its damage: {other}"
                        ));
                        continue;
                    }
                };
                *by.entry(how).or_default() += 1;
            }
        }
    }
    println!(
        "sale queue digit loss{shard}: {damaged} values damaged, refusals by {by:?}; {whole_read} undamaged rows read exactly; {} WRONG",
        wrong.len()
    );
    let mut failures: Vec<String> = wrong
        .iter()
        .map(|line| {
            format!(
                "sale queue digit loss: a damaged value was accepted, or a refusal was not the \
                 damage's: {line}"
            )
        })
        .collect();
    // The replay proves the refusals are the damage's only if rows with
    // nothing to damage still read.
    if frames > 0 && whole_read == 0 {
        failures.push(format!(
            "sale queue digit loss{shard}: no row with nothing to damage was read"
        ));
    }
    Outcome {
        exact: whole_read,
        failures,
    }
}

/// Whether a value has the same digit twice running.
fn has_repeat_digit(value: &str) -> bool {
    value
        .as_bytes()
        .windows(2)
        .any(|pair| pair[0] == pair[1] && pair[0].is_ascii_digit())
}

// Creatures' nameplates.

/// The plates a frame's labels list, top to bottom, each by whether it
/// is the locked one; none for a frame whose labels list no plates.
fn labelled_plates(labels: &Value) -> Vec<bool> {
    labels
        .get("plates")
        .and_then(Value::as_array)
        .map(|plates| {
            plates
                .iter()
                .map(|plate| {
                    plate["locked"]
                        .as_bool()
                        .expect("a plate says whether it is locked")
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The refusal's name, as the manifest gives it.
fn refusal_name(refusal: &PlateTextError) -> String {
    format!("{refusal:?}")
        .split([' ', '{', '('])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Every plate the labels list is found, and no plate anywhere else: not
/// the player's own health bar, the tool bar beside it, the progress bars
/// in windows, or pictures of items. The locked plate is the one the
/// manifest places, measured by hand from the frame's pixels. Its level
/// must read exactly or be refused, every refusal must be one the
/// manifest names with its reason, and its name, whatever the recogniser
/// makes of it, must never resolve as a creature on the bundled list: the
/// creature in these frames is not on it.
pub fn plates(r: &Replay, shard: Shard) -> Outcome {
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let store =
        GameDataStore::new(&resources().join("snapshot")).expect("the bundled game data loads");
    let catalogue = MobCatalogue::from_store(&store, Vec::new(), &profile.mob_names);
    let (mut found, mut missed, mut false_plates) = (0usize, Vec::new(), Vec::new());
    let mut level = Tally::default();
    let mut problems = Vec::new();
    let frames: Vec<(String, &Value)> = corpus
        .frames()
        .into_iter()
        .enumerate()
        .filter(|(index, _)| shard.holds(*index))
        .map(|(_, frame)| frame)
        .collect();
    for (name, labels) in frames {
        let want = labelled_plates(labels);
        let placed = &corpus.manifest["frames"][name.as_str()]["locked_plate"];
        let image = corpus.frame(&name);
        let plates = find_plates(&image, profile);
        let got: Vec<bool> = plates.iter().map(|p| p.is_locked()).collect();
        if want.is_empty() {
            false_plates.extend(plates.iter().map(|p| format!("{name}: {p:?}")));
            continue;
        }
        if got.len() < want.len() {
            missed.push(format!(
                "{name}: {} listed, {} found: {plates:?}",
                want.len(),
                got.len()
            ));
        } else if got.len() > want.len() {
            false_plates.push(format!(
                "{name}: {} listed, {} found: {plates:?}",
                want.len(),
                got.len()
            ));
        } else if got != want {
            missed.push(format!(
                "{name}: the lock is on plate {want:?}, found on {got:?}"
            ));
        } else {
            found += got.len();
        }
        println!("  {name}: {plates:?}");
        let mut scratch = PlateScratch::default();
        let locked = find_locked_plate(&image, profile, &mut scratch);
        let Some(bar) = placed["bar"].as_array() else {
            if locked.is_some() || want.contains(&true) {
                problems.push(format!(
                    "{name}: no locked plate is placed, found {locked:?}"
                ));
            }
            continue;
        };
        let bar: Vec<i64> = bar.iter().filter_map(Value::as_i64).collect();
        let at = locked.map(|p| {
            vec![
                p.bar.x0 as i64,
                p.bar.y0 as i64,
                p.bar.x1 as i64,
                p.bar.y1 as i64,
            ]
        });
        if at.as_ref() != Some(&bar) {
            problems.push(format!(
                "{name}: the locked bar is at {bar:?}, found {at:?}"
            ));
            continue;
        }
        let Some(plate) = read_locked_plate(&image, profile, &read) else {
            problems.push(format!("{name}: the locked plate did not read"));
            continue;
        };
        let wanted_level = labels["plates"]
            .as_array()
            .and_then(|p| p.iter().find(|p| p["locked"].as_bool() == Some(true)))
            .and_then(|p| p["level"].as_str())
            .expect("the locked plate has a level");
        // A refusal must be the one the manifest names for that field.
        let excused = |field: &str, refusal: &PlateTextError| {
            let named = &placed["refusals"][field];
            named["refusal"].as_str() == Some(refusal_name(refusal).as_str())
                && named["why"].as_str().is_some()
        };
        match &plate.level {
            Ok(value) if format!("L{value}") == wanted_level => level.exact += 1,
            Ok(value) => level.wrong.push(format!(
                "{name}: read L{value}, the screen shows {wanted_level}"
            )),
            Err(refusal) => {
                if !excused("level", refusal) {
                    problems.push(format!(
                        "{name}: the level was refused ({refusal}) unexcused"
                    ));
                }
                level
                    .refused
                    .push(format!("{name} ({wanted_level}): {refusal}"));
            }
        }
        if let Err(refusal) = &plate.name {
            if !excused("name", refusal) {
                problems.push(format!(
                    "{name}: the name was refused ({refusal}) unexcused"
                ));
            } else {
                println!(
                    "  {name}: the name refused as {refusal_name}, as the manifest says: {}",
                    placed["refusals"]["name"]["why"],
                    refusal_name = refusal_name(refusal)
                );
            }
        }
        for field in ["level", "name"] {
            let named = placed["refusals"][field]["refusal"].as_str();
            let refused = match field {
                "level" => plate.level.as_ref().err().map(refusal_name),
                _ => plate.name.as_ref().err().map(refusal_name),
            };
            if named.is_some() && refused.as_deref() != named {
                problems.push(format!(
                    "{name}: the manifest expects the {field} refused as {named:?}, got {refused:?}"
                ));
            }
        }
        let resolved = plate.name.as_ref().ok().map(|raw| {
            resolve(
                raw,
                plate.level.as_ref().ok().copied(),
                &catalogue,
                &profile.mob_names,
            )
        });
        println!(
            "  {name}: locked plate at {:?}, fill {:.3}; level {:?}; name {:?}, resolved {resolved:?}",
            plate.bar, plate.fill, plate.level, plate.name
        );
        if matches!(
            resolved,
            Some(Resolved::Known { .. } | Resolved::Confirmed { .. } | Resolved::Species { .. })
        ) {
            problems.push(format!("{name}: {:?} resolved as {resolved:?}", plate.name));
        }
    }
    println!(
        "plates{shard}: {found} found, {} missed, {} false",
        missed.len(),
        false_plates.len()
    );
    let mut outcome = Outcome::default();
    level.report(&format!("plate level{shard}"), &mut outcome);
    outcome.exact += found;
    outcome.failures.extend(
        missed
            .iter()
            .map(|line| format!("plates: a plate the labels list was not found: {line}")),
    );
    outcome.failures.extend(
        false_plates
            .iter()
            .map(|line| format!("plates: a plate was found that is not there: {line}")),
    );
    outcome
        .failures
        .extend(problems.iter().map(|line| format!("plates: {line}")));
    outcome
}
