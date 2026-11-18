//! Replays of the interface readers over a corpus of real captures,
//! shared by the test binary that pins them and by the maintainer tool
//! that tries a profile under edit against a whole corpus.
//!
//! A corpus is a directory of full-screen captures (`frames/*.png`), the
//! values on them transcribed independently by two people and reconciled
//! (`labels.json`), and what was measured or decided on each by hand
//! (`manifest.json`, in the shape [`Manifest`] gives). Every replay holds a
//! reader to one rule: a value is read exactly as it was on screen, or it
//! is refused; and each prints a tally of what was read exactly, what was
//! refused and what was WRONG.
//!
//! Real gameplay screens stay out of the public tree, so nothing here
//! names where a corpus is, or any of its frames: the caller says where it
//! is, and everything a replay needs to know about one frame (which
//! refusals are excused and why, which skills pages show whole, which of
//! the sell queue's spinners are greyed, which frames particular tests are
//! pinned to) is the corpus's own manifest data. A frame that needs an
//! entry the manifest lacks is reported as exactly that, apart from wrong
//! values, and fails the replay.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eo_services::game_data_store::GameDataStore;
use eo_services::game_ui::field::{label_key, FieldError, Recogniser};
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
use eo_services::game_ui::tool_bar::{lettering, read_tool_name, tool_bar_state, Lettering};
use eo_services::game_ui::tool_names::{self, ToolCatalogue, ToolMatch};
use eo_services::game_ui::window::find_windows;
use eo_services::ocr_engine::load_bgr_png;
use eo_services::planet_maps::PlanetMapStore;
use eo_services::repair_ocr::{RepairOcrService, RepairProviders, Terminal, TerminalRefusal};
use serde::Deserialize;
use serde_json::Value;

/// The application's bundled resources: the interface profile, the
/// recogniser and its runtime, and the game data the readers confirm
/// names by.
pub fn resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../entropia-orme/resources")
}

/// What was measured or decided by hand about a corpus, frame by frame.
/// An entry this does not know is refused, so a misspelt key cannot
/// silently excuse nothing.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Free text for whoever measures next: how the entries were
    /// measured. No replay reads it.
    #[serde(default, rename = "note")]
    _note: Option<String>,
    /// Every frame, by its name (its file's stem), in the file's order.
    pub frames: serde_json::Map<String, Value>,
    /// The frames particular real-capture tests are pinned to.
    #[serde(default)]
    pub cases: Option<Cases>,
    /// What the corpus is known to cost a reader, which a replay of the
    /// whole corpus must not exceed.
    #[serde(default)]
    pub ceilings: Option<Ceilings>,
}

/// What the corpus is known to cost a reader: counts a replay of the
/// whole corpus may reach and not exceed.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ceilings {
    /// Names on the tool bar whose letters and digits the recogniser
    /// gets wrong.
    pub tool_name_misreads: usize,
}

/// What was measured or decided by hand about one frame.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameEntry {
    /// Kept back to test, once, that a tuned profile generalises.
    pub held_out: bool,
    /// Every client window in the frame, front to back.
    pub windows: Vec<WindowEntry>,
    /// The locked nameplate, for a frame that shows one.
    #[serde(default)]
    pub locked_plate: Option<LockedPlateEntry>,
    /// The skills table, for a frame whose labels carry it.
    #[serde(default)]
    pub skills: Option<SkillsEntry>,
    /// Coordinates the reader is expected to refuse although the labels
    /// show them.
    #[serde(default)]
    pub coords: Option<CoordsEntry>,
    /// The sell queue, for a frame whose labels carry it.
    #[serde(default)]
    pub sale_queue: Option<SaleQueueEntry>,
    /// The state the tool bar's reader must give although the labels show
    /// the bar otherwise (`Absent` for a bar drawn at a scale the profile
    /// does not list), and why.
    #[serde(default)]
    pub tool_bar: Option<Excused>,
    /// The refusal the tool bar's name must be read with although the
    /// labels show it, and why.
    #[serde(default)]
    pub tool_name: Option<Excused>,
}

/// A window's outer rectangle, measured or checked by hand.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowEntry {
    /// `[x0, y0, x1, y1]`, the window's outermost pixels.
    pub rect: [i32; 4],
    /// False for a window with a side hidden, which the locator must
    /// never report as certain.
    #[serde(default = "certain")]
    pub extent_certain: bool,
    /// Its title, where the measurer noted it.
    #[serde(default)]
    pub title: Option<String>,
    /// How it was measured.
    #[serde(default)]
    pub note: Option<String>,
}

fn certain() -> bool {
    true
}

/// A refusal the reader must give, by its variant's name (and, for a
/// refusal that carries the text read, that text), and why the screen
/// justifies it, as a native crop shows.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Excused {
    pub refusal: String,
    #[serde(default)]
    pub raw: Option<String>,
    pub why: String,
}

impl Excused {
    /// Whether `refusal` is this one: the same variant and, where it
    /// carries the text read, the same text.
    fn names<E: std::fmt::Debug>(&self, refusal: &E, raw: Option<&str>) -> bool {
        variant(refusal) == self.refusal && raw == self.raw.as_deref()
    }
}

/// A refusal's variant name, as the manifest gives it.
fn variant<E: std::fmt::Debug>(refusal: &E) -> String {
    format!("{refusal:?}")
        .split([' ', '{', '('])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The locked nameplate's bar and the fields it is expected to refuse.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPlateEntry {
    /// The bar's coloured part, `[x0, y0, x1, y1]`, measured by hand.
    pub bar: [i32; 4],
    /// Each field (`level`, `name`) the reader must refuse, and why.
    #[serde(default)]
    pub refusals: BTreeMap<PlateField, Excused>,
}

/// The fields of a locked plate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlateField {
    Level,
    Name,
}

/// The skills table of one frame.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsEntry {
    /// Whether the page shows whole: every labelled row fully visible and
    /// no scrollbar thumb shorter than its track. Needed wherever the
    /// reader reads a page.
    #[serde(default)]
    pub whole: Option<bool>,
    /// The refusal of the whole frame the reader must give, and why.
    #[serde(default)]
    pub refusal: Option<Excused>,
}

/// The coordinates of one frame the reader must refuse.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordsEntry {
    #[serde(default)]
    pub lon: Option<Excused>,
    #[serde(default)]
    pub lat: Option<Excused>,
}

/// The sell queue of one frame.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaleQueueEntry {
    /// For every labelled row, by its name, the spinners the client draws
    /// greyed (an empty list where none is). A greyed buyout is a
    /// switched-off one. Needed for every labelled row: a row is never
    /// assumed editable.
    pub greyed: BTreeMap<String, Vec<Spinner>>,
}

/// The sell queue's spinners.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spinner {
    Quantity,
    OpeningBid,
    Buyout,
    Days,
}

impl Spinner {
    /// The labels' name for the field.
    fn field(self) -> &'static str {
        match self {
            Spinner::Quantity => "quantity",
            Spinner::OpeningBid => "opening_bid",
            Spinner::Buyout => "buyout",
            Spinner::Days => "days",
        }
    }
}

/// The frames particular real-capture tests are pinned to.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cases {
    /// A skills frame refused because its title is covered, and how many
    /// of its labelled rows are readable all the same.
    pub covered_title: CoveredTitle,
    /// A whole skills page showing a skill the bundled vocabulary lacks.
    pub vocabulary_lacks: String,
    /// Two readings of one skills page before and after a skill rose,
    /// older first, and two consecutive pages.
    pub rereading: Rereading,
    /// One sweep of every page of the whole skills list, in page order,
    /// in one sitting with nothing gained in between.
    pub sweep: Vec<String>,
    /// Skills pages holding figures the recogniser drops a repeated digit
    /// from; among them the figure the failure was first seen on.
    pub dropped_digits: Vec<String>,
    /// Radar frames with the distance the disc's edge stands at, as its
    /// rings' labels give it.
    pub ring_scale: Vec<RingScale>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoveredTitle {
    pub frame: String,
    pub readable_rows: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rereading {
    pub older: String,
    pub newer: String,
    pub consecutive: [String; 2],
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RingScale {
    pub frame: String,
    pub edge_m: f64,
}

/// A corpus of captures, with its labels and its manifest.
pub struct Corpus {
    dir: PathBuf,
    pub labels: Value,
    /// Every frame's manifest entry, in the manifest's order.
    entries: Vec<(String, FrameEntry)>,
    /// The frames particular tests are pinned to, if the manifest names
    /// them.
    cases: Option<Cases>,
    /// What the corpus is known to cost a reader.
    ceilings: Option<Ceilings>,
    /// Whether the frames replayed are the held-out ones or the rest.
    held_out: bool,
}

impl Corpus {
    /// Open the corpus at `dir`. Frames the manifest marks held out are
    /// replayed only when `held_out` asks for them, and then alone: they
    /// exist to test, once, that the readers generalise to an interface
    /// scale nothing was tuned on.
    pub fn open(dir: &Path, held_out: bool) -> Result<Corpus, String> {
        let text = |name: &str| -> Result<(PathBuf, String), String> {
            let path = dir.join(name);
            let text =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok((path, text))
        };
        let (path, labels) = text("labels.json")?;
        let labels: Value =
            serde_json::from_str(&labels).map_err(|e| format!("{}: {e}", path.display()))?;
        let (path, manifest) = text("manifest.json")?;
        let manifest: Manifest =
            serde_json::from_str(&manifest).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut entries = Vec::with_capacity(manifest.frames.len());
        for (name, entry) in manifest.frames {
            let entry: FrameEntry = serde_json::from_value(entry)
                .map_err(|e| format!("{}: frame {name}: {e}", path.display()))?;
            entries.push((name, entry));
        }
        Ok(Corpus {
            labels,
            entries,
            cases: manifest.cases,
            ceilings: manifest.ceilings,
            dir: dir.to_path_buf(),
            held_out,
        })
    }

    /// One frame's manifest entry; an empty one for a frame the manifest
    /// does not list, which every replay then reports as missing what it
    /// needs.
    pub fn entry(&self, name: &str) -> FrameEntry {
        self.entries
            .iter()
            .find(|(frame, _)| frame == name)
            .map(|(_, entry)| entry.clone())
            .unwrap_or_default()
    }

    /// The frames particular tests are pinned to.
    pub fn cases(&self) -> Result<&Cases, String> {
        self.cases
            .as_ref()
            .ok_or_else(|| "the manifest names no frames for the pinned tests (`cases`)".into())
    }

    fn replayed(&self, name: &str) -> bool {
        let held_out = self
            .entries
            .iter()
            .find(|(frame, _)| frame == name)
            .is_some_and(|(_, entry)| entry.held_out);
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
/// the labels do not justify and the manifest gives no reason for. What
/// the corpus lacks for a replay fails it too, reported apart: that is
/// bookkeeping to do, not a reader's fault.
#[derive(Debug, Default)]
pub struct Outcome {
    pub exact: usize,
    pub failures: Vec<String>,
    /// What the corpus lacks: each frame that needs a manifest entry it
    /// does not have, and which; or a frame a check needs.
    pub missing: Vec<String>,
}

impl Outcome {
    /// Whether nothing fails the replay.
    pub fn passed(&self) -> bool {
        self.failures.is_empty() && self.missing.is_empty()
    }
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
    let (mut expected_total, mut matched, mut false_locks, mut refused_ok) = (0, 0, 0, 0);
    let mut failures = Vec::new();
    for (index, (name, entry)) in corpus.entries.iter().enumerate() {
        if !shard.holds(index) || !corpus.replayed(name) {
            continue;
        }
        let image = corpus.frame(name);
        let found = find_windows(&image, chrome);
        let expected: Vec<(Rect, bool, String)> = entry
            .windows
            .iter()
            .map(|w| {
                let [x0, y0, x1, y1] = w.rect;
                // What the measurer noted of it, to say which window a
                // failure is about.
                let noted = [w.title.as_deref(), w.note.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(", ");
                (Rect::new(x0, y0, x1, y1), w.extent_certain, noted)
            })
            .collect();
        let close = |a: &Rect, b: &Rect| {
            (a.x0 - b.x0).abs() <= 2
                && (a.y0 - b.y0).abs() <= 2
                && (a.x1 - b.x1).abs() <= 2
                && (a.y1 - b.y1).abs() <= 2
        };
        for (rect, certain, noted) in &expected {
            expected_total += 1;
            if *certain {
                if found
                    .iter()
                    .any(|f| f.extent_certain() && close(&f.rect, rect))
                {
                    matched += 1;
                } else {
                    failures.push(format!(
                        "{name}: expected {rect:?} ({noted}) not found in {:?}",
                        found.iter().map(|f| f.rect).collect::<Vec<_>>()
                    ));
                }
            } else if found.iter().any(|f| {
                f.extent_certain() && !expected.iter().any(|(e, c, _)| *c && close(&f.rect, e))
            }) {
                failures.push(format!(
                    "{name}: a window with a hidden side was reported as certain"
                ));
            } else {
                refused_ok += 1;
            }
        }
        for f in found.iter().filter(|f| f.extent_certain()) {
            if !expected.iter().any(|(e, _, _)| close(&f.rect, e)) {
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
        missing: Vec::new(),
    }
}

/// Whether the manifest names the frames the pinned tests need, each one
/// a frame of the corpus: a frame missing from the labels is bookkeeping
/// to do, as is a manifest that names none. Nothing here is a value read,
/// so it adds nothing to the count of values read exactly.
pub fn pinned_frames(corpus: &Corpus) -> Outcome {
    let mut outcome = Outcome::default();
    let cases = match corpus.cases() {
        Ok(cases) => cases,
        Err(why) => {
            outcome.missing.push(why);
            return outcome;
        }
    };
    let mut named: Vec<(&str, &String)> = vec![
        ("covered_title", &cases.covered_title.frame),
        ("vocabulary_lacks", &cases.vocabulary_lacks),
        ("rereading", &cases.rereading.older),
        ("rereading", &cases.rereading.newer),
    ];
    named.extend(cases.rereading.consecutive.iter().map(|f| ("rereading", f)));
    named.extend(cases.sweep.iter().map(|f| ("sweep", f)));
    named.extend(cases.dropped_digits.iter().map(|f| ("dropped_digits", f)));
    named.extend(cases.ring_scale.iter().map(|c| ("ring_scale", &c.frame)));
    let total = named.len();
    for (case, frame) in named {
        if corpus.labels["frames"].get(frame.as_str()).is_none() {
            outcome.missing.push(format!(
                "the manifest's case {case} names {frame}, which the labels do not have"
            ));
        }
    }
    let rows = cases.covered_title.readable_rows;
    let edges: Vec<f64> = cases.ring_scale.iter().map(|c| c.edge_m).collect();
    println!(
        "pinned frames: {total} named, {} not in the labels; the covered title keeps {rows} readable rows; ring edges {edges:?} m",
        outcome.missing.len()
    );
    outcome
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
        let frame = image.clone();
        let service = RepairOcrService::new(RepairProviders {
            // The terminal reads ask for the whole window.
            frame: Arc::new(move |_| Ok(GameFrame::whole(frame.clone(), (0, 0)))),
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
            // With the profile and recogniser in hand, the service is
            // unavailable only for a terminal drawn at a scale the profile
            // has no numerals for: the reader's own refusal says so.
            Err(TerminalRefusal::ReaderUnavailable) => {
                match read_window_field(
                    &image,
                    &r.profile,
                    labelled.window,
                    labelled.field,
                    &r.reader(),
                ) {
                    Err(why @ ReadError::Field(FieldError::Scale(_))) => {
                        tally.refused.push(format!("{name} ({want}): {why}"))
                    }
                    other => panic!("{name}: the terminal reader is unavailable ({other:?})"),
                }
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

/// Compare one page's reading with its labels, row by row, for whether
/// it calls itself whole, and for the bar above the table.
fn score_skills_page(
    name: &str,
    labels: &Value,
    page: &SkillsPage,
    should_be_whole: bool,
    rows: &mut Tally,
    bar: &mut Tally,
) {
    let labelled = labels["rows"].as_array().expect("rows");
    let readable = |row: &Value| {
        row["fully_visible"].as_bool() == Some(true)
            && row["name"].is_string()
            && row["points"].is_string()
    };
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

/// Replay the skills table over every frame whose labels carry it: each
/// row's name and points, whether the page reads as whole (as the manifest
/// says it shows), and the bar above the table. A frame the reader refuses
/// although its labels show whole rows must be one whose manifest entry
/// names that refusal and why; every other labelled row is required.
pub fn skills(r: &Replay, shard: Shard) -> Outcome {
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let vocabulary = skill_vocabulary();
    let (mut rows, mut bar) = (Tally::default(), Tally::default());
    let mut unexcused = Vec::new();
    let mut missing = Vec::new();
    for (name, labels) in skills_frames(corpus, shard) {
        let image = corpus.frame(&name);
        let entry = corpus.entry(&name).skills.unwrap_or_default();
        let labelled = labels["rows"].as_array().expect("rows");
        let readable = labelled
            .iter()
            .filter(|row| row["fully_visible"].as_bool() == Some(true) && row["name"].is_string())
            .count();
        match (
            read_skills_page(&image, profile, &vocabulary, &read),
            &entry.refusal,
        ) {
            (Err(SkillsError::NoRecogniser), _) => panic!("the recogniser stopped answering"),
            (Ok(page), Some(excused)) => rows.wrong.push(format!(
                "{name}: read a page where {}: {page:?}",
                excused.why
            )),
            (Err(error), Some(excused)) => {
                println!("  {name}: frame refused: {error} ({})", excused.why);
                if !excused.names(&error, None) {
                    rows.wrong.push(format!(
                        "{name}: refused as {error:?}, where {} ({})",
                        excused.why, excused.refusal
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
                    unexcused.push(format!("{name}: frame refused ({error})"));
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
                let Some(whole) = entry.whole else {
                    missing.push(format!(
                        "{name}: the manifest does not say whether this skills page is whole \
                         (`skills.whole`)"
                    ));
                    continue;
                };
                let before = rows.refused.len();
                score_skills_page(&name, &labels, &page, whole, &mut rows, &mut bar);
                unexcused.extend(rows.refused[before..].iter().cloned());
            }
        }
    }
    let mut outcome = Outcome::default();
    rows.report(&format!("skills rows{shard}"), &mut outcome);
    bar.report(
        &format!("skills total, page and category{shard}"),
        &mut outcome,
    );
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
    outcome.missing = missing;
    outcome
}

/// The text a coordinate's refusal carries, where it carries one.
fn coord_raw(refusal: &CoordError) -> Option<&str> {
    match refusal {
        CoordError::Invalid { raw }
        | CoordError::NoSpace { raw }
        | CoordError::Misplaced { raw }
        | CoordError::WrongWidth { raw } => Some(raw),
        CoordError::OutOfBounds { text } => Some(text),
        _ => None,
    }
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
        let entry = corpus.entry(&name).coords.unwrap_or_default();
        for (key, label, excused) in [("lon", "LON", &entry.lon), ("lat", "LAT", &entry.lat)] {
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
                    Some(excused) if excused.names(&e, coord_raw(&e)) => {
                        tally.refused.push(format!(
                            "{name}: {key} {want}: {e}; {} (excused)",
                            excused.why
                        ));
                    }
                    Some(excused) => mismatched.push(format!(
                        "{name}: {key} {want} refused as {e:?}, where {} ({} {:?})",
                        excused.why, excused.refusal, excused.raw
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
        missing: Vec::new(),
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

/// The greyed spinners of every labelled row of `frame`, by the labels'
/// field names, from the manifest; or, where it lacks an entry for a
/// labelled row, each such row as missing: a row is never assumed
/// editable.
fn sale_greyed(
    corpus: &Corpus,
    frame: &str,
    labels: &Value,
) -> Result<BTreeMap<String, Vec<&'static str>>, Vec<String>> {
    let entry = corpus.entry(frame).sale_queue.unwrap_or_default();
    let mut greyed = BTreeMap::new();
    let mut missing = Vec::new();
    for row in labels["rows"].as_array().expect("rows") {
        let name = row["name"].as_str().expect("named");
        match entry.greyed.get(name) {
            Some(spinners) => {
                greyed.insert(
                    name.to_string(),
                    spinners.iter().map(|s| s.field()).collect(),
                );
            }
            None => missing.push(format!(
                "{frame}: the manifest does not say which of {name:?}'s spinners are greyed \
                 (`sale_queue.greyed`); add the row from the frame's notes"
            )),
        }
    }
    if missing.is_empty() {
        Ok(greyed)
    } else {
        Err(missing)
    }
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
fn score_sale_queue(
    name: &str,
    labels: &Value,
    greyed: &BTreeMap<String, Vec<&'static str>>,
    queue: &SaleQueue,
    fields: &mut FieldTallies,
) {
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
        let locked = greyed
            .get(item.name.text())
            .map(Vec::as_slice)
            .unwrap_or_default();
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
        let greyed = match sale_greyed(corpus, &name, &labels) {
            Ok(greyed) => greyed,
            Err(missing) => {
                outcome.missing.extend(missing);
                continue;
            }
        };
        let image = corpus.frame(&name);
        let queue = match read_sale_queue(&image, profile, &catalogue, &read) {
            Ok(queue) => queue,
            Err(SaleQueueError::NoRecogniser) => panic!("the recogniser stopped answering"),
            Err(error) => {
                // Every whole row and the count are refused with the
                // queue, and none of them is excused.
                println!("  {name}: the queue was refused: {error}");
                for row in labels["rows"].as_array().expect("rows") {
                    if row["fully_visible"].as_bool() == Some(true) {
                        let label = row["name"].as_str().expect("named");
                        let line = format!("{name}: {label} (the queue was refused: {error})");
                        for field in SALE_FIELDS {
                            fields.field(field).refused.push(line.clone());
                        }
                    }
                }
                count.refused.push(format!(
                    "{name}: {} (the queue was refused: {error})",
                    labelled_count(&labels)
                ));
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
        score_sale_queue(&name, &labels, &greyed, &queue, &mut fields);
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
    let (mut damaged, mut whole_read, mut undamaged) = (0, 0, 0);
    let mut by: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut wrong = Vec::new();
    let mut missing = Vec::new();
    let mut frames = 0;
    for (name, labels) in sale_frames(corpus, shard) {
        let greyed = match sale_greyed(corpus, &name, &labels) {
            Ok(greyed) => greyed,
            Err(lacking) => {
                missing.extend(lacking);
                continue;
            }
        };
        frames += 1;
        // The rows with nothing the damage can touch, as the labels show
        // them, whether or not the queue then reads.
        undamaged += labels["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .filter(|row| row["fully_visible"].as_bool() == Some(true))
            .filter(|row| {
                let label = row["name"].as_str().expect("named");
                let locked = greyed.get(label).map(Vec::as_slice).unwrap_or_default();
                !SALE_FIELDS
                    .iter()
                    .filter(|f| **f != "buyout" || !locked.contains(&"buyout"))
                    .any(|f| row[*f].as_str().is_some_and(has_repeat_digit))
            })
            .count();
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
            let locked = greyed
                .get(item.name.text())
                .map(Vec::as_slice)
                .unwrap_or_default();
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
            let locked = greyed.get(label).map(Vec::as_slice).unwrap_or_default();
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
    // nothing to damage still read. Frames that show no such row cannot
    // prove it at all: that is the corpus lacking a frame, not a fault of
    // the reader.
    if frames > 0 && undamaged == 0 {
        missing.push(format!(
            "sale queue digit loss{shard}: no queue row the frames show whole is free of repeated \
             digits, so nothing shows the refusals are the damage's; file a frame that shows one"
        ));
    } else if frames > 0 && whole_read == 0 {
        failures.push(format!(
            "sale queue digit loss{shard}: no row with nothing to damage was read"
        ));
    }
    Outcome {
        exact: whole_read,
        failures,
        missing,
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
    let catalogue = MobCatalogue::from_store(&store, Vec::<String>::new(), &profile.mob_names);
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
        let placed = corpus.entry(&name).locked_plate;
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
        let Some(placed) = placed else {
            if locked.is_some() || want.contains(&true) {
                problems.push(format!(
                    "{name}: no locked plate is placed, found {locked:?}"
                ));
            }
            continue;
        };
        let bar = placed.bar;
        let at = locked.map(|p| [p.bar.x0, p.bar.y0, p.bar.x1, p.bar.y1]);
        if at != Some(bar) {
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
        let excused = |field: PlateField, refusal: &PlateTextError| {
            placed
                .refusals
                .get(&field)
                .is_some_and(|named| named.refusal == variant(refusal))
        };
        match &plate.level {
            Ok(value) if format!("L{value}") == wanted_level => level.exact += 1,
            Ok(value) => level.wrong.push(format!(
                "{name}: read L{value}, the screen shows {wanted_level}"
            )),
            Err(refusal) => {
                if !excused(PlateField::Level, refusal) {
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
            if !excused(PlateField::Name, refusal) {
                problems.push(format!(
                    "{name}: the name was refused ({refusal}) unexcused"
                ));
            } else {
                println!(
                    "  {name}: the name refused as {}, as the manifest says: {:?}",
                    variant(refusal),
                    placed.refusals[&PlateField::Name].why
                );
            }
        }
        for field in [PlateField::Level, PlateField::Name] {
            let named = placed.refusals.get(&field).map(|e| e.refusal.as_str());
            let refused = match field {
                PlateField::Level => plate.level.as_ref().err().map(variant),
                PlateField::Name => plate.name.as_ref().err().map(variant),
            };
            if named.is_some() && refused.as_deref() != named {
                problems.push(format!(
                    "{name}: the manifest expects the {field:?} refused as {named:?}, got {refused:?}"
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

// The tool bar.

/// The item a bar with lettering names, as its labels give it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LabelledItem {
    /// The item's full name, as the bundled game data lists it.
    Listed(String),
    /// An item in no list of the tools a player holds.
    Unlisted,
}

/// What a frame's labels say the tool bar shows: its state as the
/// reader's state is named (`no_panel` as `NoPanel`), and for a bar with
/// lettering the name as the screen shows it (with the dots of a name cut
/// short), with the item it names. `None` for a frame whose labels say
/// nothing of the bar.
type LabelledBar = (String, Option<String>, Option<LabelledItem>);

fn labelled_tool_bar(labels: &Value) -> Option<LabelledBar> {
    let entry = labels.get("tool_bar")?;
    let state = entry["state"].as_str().expect("the tool bar's state");
    let named: String = state
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect();
    let item = match (entry["item"].as_str(), entry["unlisted"].as_bool()) {
        (Some(item), None) => Some(LabelledItem::Listed(item.to_string())),
        (None, Some(true)) => Some(LabelledItem::Unlisted),
        _ => None,
    };
    Some((named, entry["name"].as_str().map(str::to_string), item))
}

/// Every frame's tool bar in the state its labels give, or the one the
/// manifest names with its reason: a bar found where none is, or one read
/// as whole where a window covers it, fails, as does any other state
/// than the labelled one. On every bar with lettering the name is read
/// and compared with the screen's: a refusal the manifest does not excuse
/// fails, a misread is counted and printed (the recogniser's reading is
/// matched against the known tools later), and the tally says how many
/// read right in their letters and digits (a name is compared on those
/// alone, the recogniser dropping spaces between words) and how many
/// letter-perfect; whether the name was cut short is told from the
/// pixels and must be right. Every name read is then resolved against the
/// bundled tools: naming another tool than the labelled one, or any tool
/// for an item in no list, fails; resolving to the labelled tool (or to
/// nothing, for an item in no list) is right; a contention or nothing
/// where a tool is labelled is counted as unresolved and printed.
///
/// Over the whole corpus (not a shard of it), two more rules hold: the
/// misreads may not outnumber the manifest's ceiling, and the lettering's
/// masks are compared between every pair of frames with lettering. Two
/// tools' masks must always differ by more than the profile's threshold
/// for reading again; one tool's masks may differ by more (a reload
/// darkening the fill), which costs only a needless read and is counted.
pub fn tool_bar(r: &Replay, shard: Shard) -> Outcome {
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let mut outcome = Outcome::default();
    let (mut states, mut right, mut perfect, mut lettered) = (0usize, 0usize, 0usize, 0usize);
    let (mut misread, mut refused) = (Vec::new(), Vec::new());
    let mut masks: Vec<(String, String, Lettering)> = Vec::new();
    let store = GameDataStore::new(&resources().join("snapshot")).expect("the bundled game data");
    let catalogue = ToolCatalogue::from_store(&store, Vec::new(), Vec::new());
    let (mut resolved, mut unresolved) = (0usize, Vec::new());
    let frames: Vec<(String, &Value)> = corpus
        .frames()
        .into_iter()
        .enumerate()
        .filter(|(index, _)| shard.holds(*index))
        .map(|(_, frame)| frame)
        .collect();
    for (name, labels) in frames {
        let Some((labelled, truth, item)) = labelled_tool_bar(labels) else {
            outcome
                .missing
                .push(format!("{name}: the labels say nothing of the tool bar"));
            continue;
        };
        let entry = corpus.entry(&name);
        let expected = entry
            .tool_bar
            .as_ref()
            .map_or(labelled.clone(), |e| e.refusal.clone());
        let image = corpus.frame(&name);
        let state = tool_bar_state(&image, profile);
        let got = variant(&state);
        if got != expected {
            let why = match (expected.as_str(), got.as_str()) {
                ("Absent", _) => "a bar was found where none is",
                ("Covered", "Lettered" | "Empty") => "a covered bar was taken as whole",
                _ => "the state is not the labelled one",
            };
            outcome.failures.push(format!(
                "tool bar: {name}: {why}: expected {expected}, found {state:?}"
            ));
            continue;
        }
        states += 1;
        if let Some(e) = &entry.tool_bar {
            println!(
                "  {name}: {got} as the manifest says, though the labels show {labelled}: {:?}",
                e.why
            );
        }
        let Some(mask) = lettering(&image, &state, profile) else {
            println!("  {name}: {got}");
            continue;
        };
        let (Some(truth), Some(item)) = (truth, item) else {
            outcome.missing.push(format!(
                "{name}: the labels give the tool bar lettering but no name, or not the item it names (`item`, or `unlisted`)"
            ));
            continue;
        };
        lettered += 1;
        masks.push((name.clone(), truth.clone(), mask));
        match read_tool_name(&image, &state, profile, &read) {
            Ok(tool) => {
                let shown = if tool.cut_short {
                    format!("{}...", tool.text)
                } else {
                    tool.text.clone()
                };
                if tool.cut_short != truth.ends_with("...") {
                    outcome.failures.push(format!(
                        "tool bar: {name}: read as cut short {}, the screen shows {truth:?}",
                        tool.cut_short
                    ));
                }
                // A name is compared on its letters and digits: the
                // recogniser may drop the spaces between words.
                let visible = truth.strip_suffix("...").unwrap_or(&truth);
                if tool.key() == label_key(visible) {
                    right += 1;
                    perfect += usize::from(shown == truth);
                    println!("  {name}: {shown:?}");
                } else {
                    misread.push(format!(
                        "{name}: read {shown:?}, the screen shows {truth:?}"
                    ));
                }
                let found = tool_names::resolve(&tool, &catalogue, &profile.tool_names);
                match resolution(&found, &item) {
                    Resolution::Right => resolved += 1,
                    Resolution::Unresolved => {
                        unresolved.push(format!("{name}: read {shown:?}, {found:?}"))
                    }
                    Resolution::Wrong => outcome.failures.push(format!(
                        "tool bar: {name}: read {shown:?}, resolved to {found:?}, but the item is {item:?}"
                    )),
                }
            }
            Err(refusal) => {
                let line = format!("{name} ({truth:?}): {refusal}");
                match &entry.tool_name {
                    Some(e) if e.names(&refusal, None) => {
                        println!("  refused as the manifest says: {line}: {:?}", e.why);
                    }
                    _ => outcome
                        .failures
                        .push(format!("tool bar: the name was refused unexcused: {line}")),
                }
                refused.push(line);
            }
        }
    }
    println!(
        "tool bar{shard}: {states} states as labelled; {lettered} with lettering: {right} read right in their letters and digits ({perfect} letter-perfect, spaces too), {} misread, {} refused",
        misread.len(),
        refused.len(),
    );
    for line in &misread {
        println!("  misread: {line}");
    }
    println!(
        "tool bar{shard}: of the names read, {resolved} resolved to the labelled item, {} unresolved, none to another",
        unresolved.len()
    );
    for line in &unresolved {
        println!("  unresolved: {line}");
    }
    for line in &refused {
        println!("  refused: {line}");
    }
    if shard.0.is_some() {
        println!(
            "tool bar{shard}: the misread ceiling and the lettering's pairs are held over the whole corpus only"
        );
        outcome.exact += states + right + resolved;
        return outcome;
    }
    match corpus.ceilings.as_ref().map(|c| c.tool_name_misreads) {
        None => outcome.missing.push(
            "the manifest gives no ceiling for the tool bar's misread names (`ceilings`)".into(),
        ),
        Some(most) if misread.len() > most => outcome.failures.push(format!(
            "tool bar: {} names misread in their letters and digits, more than the corpus's ceiling of {most}",
            misread.len()
        )),
        Some(most) => println!("tool bar: {} misread, the ceiling is {most}", misread.len()),
    }
    let limit = profile.tool_bar.lettering.max_differing;
    let (mut same_most, mut same_over) = (None::<(usize, String)>, 0usize);
    let mut other_least: Option<(usize, String)> = None;
    for (i, (a, a_name, a_mask)) in masks.iter().enumerate() {
        for (b, b_name, b_mask) in &masks[i + 1..] {
            let differing = a_mask.differing(b_mask).unwrap_or(usize::MAX);
            let pair = format!("{a} and {b}");
            if a_name == b_name {
                same_over += usize::from(differing > limit);
                if same_most.as_ref().is_none_or(|(n, _)| differing > *n) {
                    same_most = Some((differing, pair));
                }
            } else {
                if differing <= limit {
                    outcome.failures.push(format!(
                        "tool bar: two tools' lettering differ by {differing} pixels, no more than {limit}: {pair}"
                    ));
                }
                if other_least.as_ref().is_none_or(|(n, _)| differing < *n) {
                    other_least = Some((differing, pair));
                }
            }
        }
    }
    let area = masks.first().map_or(0, |(_, _, m)| {
        let (w, h) = m.size();
        (w * h) as usize
    });
    println!(
        "tool bar lettering: {area} pixels; one tool differs in at most {:?}, {same_over} pairs over {limit} (a needless read each); two tools in at least {:?}",
        same_most.as_ref().map(|(n, pair)| format!("{n} ({pair})")),
        other_least.as_ref().map(|(n, pair)| format!("{n} ({pair})")),
    );
    outcome.exact += states + right + resolved;
    outcome
}

/// How a resolved name stands against the item its frame is labelled
/// with.
enum Resolution {
    Right,
    Unresolved,
    Wrong,
}

fn resolution(found: &ToolMatch, item: &LabelledItem) -> Resolution {
    match (found, item) {
        (ToolMatch::Bundled { name, .. }, LabelledItem::Listed(item)) if name == item => {
            Resolution::Right
        }
        (ToolMatch::Unknown { .. }, LabelledItem::Unlisted) => Resolution::Right,
        (ToolMatch::Contended { .. } | ToolMatch::Unknown { .. }, LabelledItem::Listed(_)) => {
            Resolution::Unresolved
        }
        _ => Resolution::Wrong,
    }
}
