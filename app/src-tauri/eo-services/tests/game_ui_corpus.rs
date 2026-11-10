//! The reading pipeline against real captures of the client.
//!
//! The unit tests draw their own windows, which proves the geometry
//! but not that the recogniser can read what the client actually
//! renders. These tests replay a private corpus of full-screen
//! captures, each value in it transcribed independently by two people
//! and reconciled, and hold the pipeline to one rule: a value is read
//! exactly as it was on screen, or it is refused. A wrong value
//! accepted is the only failure that matters.
//!
//! Real gameplay screens stay out of the public tree, so every test
//! here runs only where `EO_GAME_UI_CORPUS_DIR` names the corpus and
//! the bundled ONNX Runtime is loadable, and says why when it skips.
//! Frames marked held out in the corpus are replayed only when
//! `EO_GAME_UI_HELD_OUT` is set: they exist to test, once, that the
//! pipeline generalises to an interface scale nothing was tuned on.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use eo_services::game_ui::locate::{read_window_field, ReadError};
use eo_services::game_ui::profile::UiProfile;
use eo_services::game_ui::radar::{
    find_radar, read_coordinates, ring_scale, CoordError, CoordinateBounds,
};
use eo_services::game_ui::skills::{
    nearest_skill, read_skills_page, Admission, Checksum, RowName, SkillRow, SkillVocabulary,
    SkillsError, SkillsPage, SkillsSweep, SkillsTableProfile,
};
use eo_services::ocr_engine::{load_bgr_png, OcrEngine};
use eo_services::planet_maps::PlanetMapStore;
use eo_services::repair_ocr::{RepairOcrService, RepairProviders, Terminal, TerminalRefusal};
use eo_services::skill_panel::BgrImage;
use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn resources() -> PathBuf {
    repo_root().join("app/src-tauri/entropia-orme/resources")
}

fn profile() -> UiProfile {
    UiProfile::load(&resources().join("ui_profile.json"))
        .expect("the bundled interface profile loads")
}

struct Corpus {
    dir: PathBuf,
    labels: Value,
    manifest: Value,
}

impl Corpus {
    fn open() -> Option<Corpus> {
        let Some(dir) = std::env::var_os("EO_GAME_UI_CORPUS_DIR")
            .map(PathBuf::from)
            .filter(|d| d.is_dir())
        else {
            eprintln!("skipped: EO_GAME_UI_CORPUS_DIR is not set, so there are no real captures to replay");
            return None;
        };
        let json = |name: &str| -> Value {
            serde_json::from_str(
                &std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}")),
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"))
        };
        Some(Corpus {
            labels: json("labels.json"),
            manifest: json("manifest.json"),
            dir,
        })
    }

    /// The frames to replay, with their labels: every frame, less the
    /// held-out ones unless those were asked for.
    fn frames(&self) -> Vec<(String, &Value)> {
        let held_out_wanted = std::env::var_os("EO_GAME_UI_HELD_OUT").is_some();
        self.labels["frames"]
            .as_object()
            .expect("frames")
            .iter()
            .filter(|(name, _)| {
                let held_out = self.manifest["frames"][name.as_str()]["held_out"]
                    .as_bool()
                    .unwrap_or(false);
                held_out == held_out_wanted
            })
            .map(|(name, labels)| (name.clone(), labels))
            .collect()
    }

    fn frame(&self, name: &str) -> BgrImage {
        let path = self.dir.join("frames").join(format!("{name}.png"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        let (data, h, w) = load_bgr_png(&bytes).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        BgrImage { data, h, w }
    }
}

/// The bundled recogniser, loaded once for the whole test binary. The
/// runtime is pinned to the committed library: left to search, the
/// loader can hang on a foreign-format library instead of reporting it.
fn recogniser() -> Option<Arc<OcrEngine>> {
    static ENGINE: OnceLock<Option<Arc<OcrEngine>>> = OnceLock::new();
    static LOAD: Mutex<()> = Mutex::new(());
    let _guard = LOAD.lock().unwrap_or_else(|p| p.into_inner());
    ENGINE
        .get_or_init(|| {
            #[cfg(target_os = "linux")]
            let dylib = resources().join("ort-linux/libonnxruntime.so");
            #[cfg(not(target_os = "linux"))]
            let dylib = resources().join("ort/onnxruntime.dll");
            if !dylib.is_file() {
                eprintln!(
                    "skipped: the bundled ONNX Runtime is absent at {}",
                    dylib.display()
                );
                return None;
            }
            std::env::set_var("ORT_DYLIB_PATH", &dylib);
            let models = resources().join("models");
            match OcrEngine::new(
                &models.join("svtrv2_rec.onnx"),
                &models.join("ppocr_keys_v1.txt"),
            ) {
                Ok(engine) => Some(Arc::new(engine)),
                Err(error) => {
                    eprintln!("skipped: the recogniser did not load ({error})");
                    None
                }
            }
        })
        .clone()
}

fn reader(engine: Arc<OcrEngine>) -> impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync {
    move |image: &BgrImage| engine.recognize_bgr(&image.data, image.h, image.w).ok()
}

/// What became of one labelled value.
#[derive(Default)]
struct Tally {
    exact: usize,
    refused: Vec<String>,
    wrong: Vec<String>,
}

impl Tally {
    fn report(&self, what: &str) {
        println!(
            "{what}: {} read exactly, {} refused, {} WRONG",
            self.exact,
            self.refused.len(),
            self.wrong.len()
        );
        for line in &self.refused {
            println!("  refused: {line}");
        }
        assert!(
            self.wrong.is_empty(),
            "wrong values were accepted:\n{}",
            self.wrong.join("\n")
        );
    }
}

/// Replay one labelled field of one window kind over every frame whose
/// labels carry `surface.label_key`.
fn replay_field(window: &str, field: &str, surface: &str, label_key: &str) -> Option<Tally> {
    let corpus = Corpus::open()?;
    let engine = recogniser()?;
    let read = reader(engine);
    let profile = profile();
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
        match read_window_field(&image, &profile, window, field, &read) {
            Ok(got) if got.parsed.text == want => tally.exact += 1,
            Ok(got) => tally.wrong.push(format!(
                "{name}: read {:?}, the screen shows {want:?}",
                got.parsed.text
            )),
            Err(ReadError::NoRecogniser) => panic!("the recogniser stopped answering"),
            Err(error) => tally.refused.push(format!("{name} ({want}): {error}")),
        }
    }
    Some(tally)
}

#[test]
fn repair_cost_is_read_exactly_or_refused() {
    let Some(tally) = replay_field("repair_terminal", "repair_cost", "repair", "repair_cost")
    else {
        return;
    };
    tally.report("repair cost");
    assert!(
        tally.refused.is_empty(),
        "every repair frame shows its cost in full"
    );
}

#[test]
fn trade_terminal_total_is_read_exactly_or_refused() {
    let Some(tally) = replay_field("trade_terminal", "total", "trade", "total") else {
        return;
    };
    tally.report("trade terminal total");
    assert!(
        tally.refused.is_empty(),
        "every trade frame shows its total in full"
    );
}

/// A frame with the terminal closed must refuse, not read something
/// else's number.
#[test]
fn a_closed_terminal_is_a_refusal() {
    let Some(corpus) = Corpus::open() else { return };
    let Some(engine) = recogniser() else { return };
    let read = reader(engine);
    let profile = profile();
    let mut checked = 0;
    for (name, labels) in corpus.frames() {
        if labels.get("repair").is_some() || labels.get("trade").is_some() {
            continue;
        }
        // Frames with other windows open are the interesting ones.
        if labels["windows"].as_array().is_none_or(|w| w.is_empty()) || checked >= 6 {
            continue;
        }
        let image = corpus.frame(&name);
        for (window, field) in [
            ("repair_terminal", "repair_cost"),
            ("trade_terminal", "total"),
        ] {
            let got = read_window_field(&image, &profile, window, field, &read);
            assert!(
                matches!(got, Err(ReadError::WindowNotFound)),
                "{name}: {window}.{field} answered {got:?} with the terminal closed"
            );
        }
        checked += 1;
    }
    println!("closed terminal: {checked} frames with other windows open, all refused");
}

/// Replay one terminal's figure through the service the app reads it
/// with: the whole frame handed over as the game window, the bundled
/// profile, the bundled recogniser. The amount it answers must be the
/// labelled figure's own number, not merely a well-formed one.
fn replay_terminal(terminal: Terminal, surface: &str, label_key: &str) -> Option<Tally> {
    let corpus = Corpus::open()?;
    let engine = recogniser()?;
    let profile = Arc::new(profile());
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
        let read = reader(engine.clone());
        let service = RepairOcrService::new(RepairProviders {
            frame: Arc::new(move || Ok(image.clone())),
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
        match service.read(terminal) {
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
    Some(tally)
}

#[test]
fn the_repair_terminal_read_answers_the_amount_on_screen() {
    let Some(tally) = replay_terminal(Terminal::Repair, "repair", "repair_cost") else {
        return;
    };
    tally.report("repair terminal read");
    assert!(tally.exact > 0, "the corpus has repair frames to read");
    assert!(
        tally.refused.is_empty(),
        "every repair frame shows its cost in full"
    );
}

#[test]
fn the_trade_terminal_read_answers_the_amount_on_screen() {
    let Some(tally) = replay_terminal(Terminal::Trade, "trade", "total") else {
        return;
    };
    tally.report("trade terminal read");
    assert!(tally.exact > 0, "the corpus has trade frames to read");
    assert!(
        tally.refused.is_empty(),
        "every trade frame shows its total in full"
    );
}

/// The skill names and categories the application ships.
fn skill_vocabulary() -> SkillVocabulary {
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
fn same_skill(name: Option<&str>, label: &str) -> bool {
    match (name, label.strip_suffix("...")) {
        (None, _) => label.is_empty(),
        (Some(name), Some(stem)) => name.starts_with(stem) && name.len() > stem.len(),
        (Some(name), None) => name == label,
    }
}

/// Whether a row read is the row the labels call `label`: a known skill
/// by its name, a skill the vocabulary lacks by exactly the text the
/// screen shows.
fn row_is(row: &SkillRow, label: &str) -> bool {
    match &row.name {
        RowName::Known { name, .. } => same_skill(Some(name), label),
        RowName::Blank => label.is_empty(),
        RowName::Unknown => row.raw_name.trim() == label,
    }
}

/// The frames whose labels carry the skills table, less the held-out
/// ones, in shard `shard`.
fn skills_frames(corpus: &Corpus, shard: usize) -> Vec<(String, Value)> {
    corpus
        .frames()
        .into_iter()
        .filter(|(_, labels)| labels.get("skills").is_some())
        .enumerate()
        .filter(|(i, _)| i % SKILLS_SHARDS == shard)
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

fn replay_skills_shard(shard: usize) {
    let Some(corpus) = Corpus::open() else { return };
    let Some(engine) = recogniser() else { return };
    let read = reader(engine);
    let profile = profile();
    let vocabulary = skill_vocabulary();
    let (mut rows, mut bar) = (Tally::default(), Tally::default());
    for (name, labels) in skills_frames(&corpus, shard) {
        let image = corpus.frame(&name);
        let excused = SKILLS_REFUSALS.iter().find(|(f, _, _)| *f == name);
        let labelled = labels["rows"].as_array().expect("rows");
        let readable = labelled
            .iter()
            .filter(|row| row["fully_visible"].as_bool() == Some(true) && row["name"].is_string())
            .count();
        match (
            read_skills_page(&image, &profile, &vocabulary, &read),
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
    rows.report(&format!("skills rows, shard {shard}"));
    bar.report(&format!("skills total, page and category, shard {shard}"));
    let unexcused: Vec<&String> = rows
        .refused
        .iter()
        .filter(|line| {
            !SKILLS_REFUSALS
                .iter()
                .any(|(frame, _, _)| line.starts_with(&format!("{frame}:")))
        })
        .collect();
    assert!(
        unexcused.is_empty(),
        "rows the screen shows whole were refused: {unexcused:#?}"
    );
    assert!(
        bar.refused.is_empty(),
        "the bar above the table was refused: {:#?}",
        bar.refused
    );
}

/// The one frame refused for a covered title loses only the rows the
/// labels say are readable there; the rest are under the window in front.
#[test]
fn a_frame_refused_for_a_covered_title_hides_only_one_readable_row() {
    let Some(corpus) = Corpus::open() else { return };
    let rows = corpus.labels["frames"]["f17"]["skills"]["rows"]
        .as_array()
        .expect("f17 rows")
        .clone();
    let readable: Vec<&Value> = rows
        .iter()
        .filter(|row| row["fully_visible"].as_bool() == Some(true))
        .collect();
    assert_eq!(readable.len(), 1, "{readable:?}");
    assert!(rows
        .iter()
        .filter(|row| row["fully_visible"].as_bool() != Some(true))
        .all(|row| row["name"].is_null()));
}

const SKILLS_SHARDS: usize = 8;

macro_rules! skills_shards {
    ($($name:ident: $shard:expr,)*) => {$(
        #[test]
        fn $name() {
            replay_skills_shard($shard);
        }
    )*};
}

skills_shards! {
    skills_rows_shard_0: 0,
    skills_rows_shard_1: 1,
    skills_rows_shard_2: 2,
    skills_rows_shard_3: 3,
    skills_rows_shard_4: 4,
    skills_rows_shard_5: 5,
    skills_rows_shard_6: 6,
    skills_rows_shard_7: 7,
}

/// One sweep of every skill: the thirteen pages of the whole list, read
/// one after another with nothing gained in between.
const SWEEP_FRAMES: [&str; 13] = [
    "f62", "f63", "f64", "f65", "f66", "f67", "f68", "f69", "f70", "f71", "f72", "f73", "f74",
];

/// The sweep's labelled rows, page by page, split into the names the
/// bundled vocabulary knows (with the blank row) and those it lacks.
fn sweep_labels(corpus: &Corpus, vocabulary: &SkillVocabulary) -> (Vec<Value>, Vec<Value>) {
    SWEEP_FRAMES
        .iter()
        .flat_map(|frame| {
            corpus.labels["frames"][*frame]["skills"]["rows"]
                .as_array()
                .expect("rows")
                .clone()
        })
        .partition(|row| {
            let label = row["name"].as_str().expect("named");
            label.is_empty()
                || vocabulary
                    .names()
                    .iter()
                    .any(|n| same_skill(Some(n), label))
        })
}

fn skills_table(profile: &UiProfile) -> SkillsTableProfile {
    profile.windows["skills"]
        .skills_table
        .clone()
        .expect("the profile describes the skills table")
}

/// A skill the bundled vocabulary lacks is read with the text the screen
/// shows and its points, never given a vocabulary name, and leaves its
/// page whole.
#[test]
fn a_skill_the_vocabulary_lacks_is_read_and_leaves_its_page_whole() {
    let Some(corpus) = Corpus::open() else { return };
    let Some(engine) = recogniser() else { return };
    let read = reader(engine);
    let profile = profile();
    let table = skills_table(&profile);
    let vocabulary = skill_vocabulary();
    let page =
        read_skills_page(&corpus.frame("f66"), &profile, &vocabulary, &read).expect("f66 reads");
    assert!(page.whole, "f66: {page:?}");
    let unknown: Vec<&SkillRow> = page
        .rows
        .iter()
        .filter(|row| row.name == RowName::Unknown)
        .collect();
    assert_eq!(unknown.len(), 1, "{unknown:?}");
    assert_eq!(
        (
            unknown[0].raw_name.as_str(),
            unknown[0].points_text.as_str()
        ),
        ("Food Technology", "1")
    );
    let (nearest, score) =
        nearest_skill(&unknown[0].raw_name, &vocabulary).expect("a nearest skill");
    println!(
        "skill the bundled vocabulary lacks: {:?} = {}, nearest {nearest:?} at {score:.1} (near-miss floor {}, sweep floor {})",
        unknown[0].raw_name, unknown[0].points_text, table.near_miss.score, table.near_miss.sweep_score
    );
}

/// Two different readings of page 2 (the list before and after a skill
/// rose: the same skills, another total): the later one is kept, each
/// time. Then page 4's rows arriving under page 3's number, as a misread
/// pager would bring them, unsettle page 3 until it is read again.
#[test]
fn rereadings_replace_and_a_misplaced_page_unsettles_its_number() {
    let Some(corpus) = Corpus::open() else { return };
    let Some(engine) = recogniser() else { return };
    let read = reader(engine);
    let profile = profile();
    let vocabulary = skill_vocabulary();
    let mut sweep = SkillsSweep::new(&skills_table(&profile), &vocabulary);
    let page = |frame: &str| {
        read_skills_page(&corpus.frame(frame), &profile, &vocabulary, &read)
            .unwrap_or_else(|e| panic!("{frame}: {e}"))
    };
    let (older, newer) = (page("f13"), page("f63"));
    assert_eq!((older.page, older.total), (Some((2, 13)), Some(198258)));
    assert_eq!((newer.page, newer.total), (Some((2, 13)), Some(198259)));
    assert_eq!(sweep.accept(older.clone()), Admission::Kept);
    assert_eq!(sweep.accept(newer.clone()), Admission::Kept);
    assert_eq!(sweep.page(2).and_then(|p| p.total), Some(198259));
    assert_eq!(sweep.accept(older), Admission::Kept);
    assert_eq!(sweep.page(2).and_then(|p| p.total), Some(198258));

    let (third, fourth) = (page("f64"), page("f65"));
    assert_eq!(sweep.accept(third.clone()), Admission::Kept);
    let mut misplaced = fourth;
    misplaced.page = Some((3, 13));
    assert_eq!(sweep.accept(misplaced), Admission::Conflict);
    assert!(sweep.missing().expect("count known").contains(&3));
    assert_eq!(sweep.accept(third.clone()), Admission::Kept);
    assert_eq!(sweep.page(3), Some(&third));
    println!("skills sweep rereadings: page 2 kept from the later reading each time; page 3 unsettled by a misplaced page and settled by a reread");
}

/// The sweep's pages, read with the bundled vocabulary, fed out of order
/// and with one page read twice, merge into every skill (those it knows
/// and the one it does not) and add up to the total the panel prints.
#[test]
fn a_sweep_of_every_page_adds_up_to_the_panels_total() {
    let Some(corpus) = Corpus::open() else { return };
    let Some(engine) = recogniser() else { return };
    let read = reader(engine);
    let profile = profile();
    let vocabulary = skill_vocabulary();
    let pages: Vec<SkillsPage> = SWEEP_FRAMES
        .iter()
        .map(|name| {
            read_skills_page(&corpus.frame(name), &profile, &vocabulary, &read)
                .unwrap_or_else(|e| panic!("{name}: {e}"))
        })
        .collect();
    let mut sweep = SkillsSweep::new(&skills_table(&profile), &vocabulary);
    for index in [12usize, 3, 7, 0, 1, 11, 5, 9, 2, 4, 10, 6, 8, 1] {
        assert_eq!(
            sweep.accept(pages[index].clone()),
            Admission::Kept,
            "{} (page {}) is whole",
            SWEEP_FRAMES[index],
            index + 1
        );
    }
    assert_eq!(sweep.missing(), Some(Vec::new()));
    let result = sweep.finish().expect("every page is whole");
    let (known, lacking) = sweep_labels(&corpus, &vocabulary);
    assert_eq!(result.skills.len(), known.len());
    for (got, want) in result.skills.iter().zip(&known) {
        assert!(
            same_skill(got.name.as_deref(), want["name"].as_str().expect("named")),
            "{:?} where the screen shows {:?}",
            got.name,
            want["name"]
        );
        assert_eq!(Some(got.points_text.as_str()), want["points"].as_str());
    }
    assert_eq!(result.unknown.len(), lacking.len());
    for (got, want) in result.unknown.iter().zip(&lacking) {
        assert_eq!(Some(got.raw_name.as_str()), want["name"].as_str());
        assert_eq!(Some(got.points_text.as_str()), want["points"].as_str());
    }
    assert!(result.unresolved.is_empty() && result.duplicates.is_empty());
    let unknown: Vec<String> = result
        .unknown
        .iter()
        .map(|s| format!("{} {}", s.raw_name, s.points_text))
        .collect();
    let near_misses = result.skills.iter().filter(|s| s.near_miss).count();
    println!(
        "skills sweep: {} pages, {} known ({near_misses} by near miss), {} unknown {unknown:?}, checksum {:?}, total {:?}",
        SWEEP_FRAMES.len(),
        result.skills.len(),
        result.unknown.len(),
        result.checksum,
        result.total
    );
    assert_eq!(result.skills.len(), 151);
    assert_eq!(unknown, vec!["Food Technology 1".to_string()]);
    assert_eq!(result.total, Some(198259));
    assert_eq!(result.checksum, Checksum::Matches);
}

/// The recogniser's known failure, made systematic: wherever a number
/// has the same digit twice running, one of the pair is dropped. It has
/// done exactly this to `1,444.77` at 96% confidence.
fn collapsing(
    read: impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync,
    collapsed: Arc<Mutex<Vec<(String, String)>>>,
) -> impl Fn(&BgrImage) -> Option<(String, f64)> + Send + Sync {
    move |image: &BgrImage| {
        let (text, confidence) = read(image)?;
        let numeric = text.chars().any(|c| c.is_ascii_digit())
            && text
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, ',' | '.' | ' '));
        if !numeric {
            return Some((text, confidence));
        }
        let chars: Vec<char> = text.chars().collect();
        let Some(at) = chars
            .windows(2)
            .position(|pair| pair[0] == pair[1] && pair[0].is_ascii_digit())
        else {
            return Some((text, confidence));
        };
        let mut short = chars.clone();
        short.remove(at);
        let short: String = short.into_iter().collect();
        collapsed.lock().unwrap().push((text, short.clone()));
        Some((short, confidence))
    }
}

/// With a recogniser that drops one of every repeated pair of digits,
/// no skill's points may come back wrong: each damaged number must be
/// refused, by its grouping or by its width. The sweep's thirteen pages
/// hold the figure this failure was first seen on.
#[test]
fn a_recogniser_that_drops_repeated_digits_never_gets_a_number_accepted() {
    let Some(corpus) = Corpus::open() else { return };
    let Some(engine) = recogniser() else { return };
    let collapsed = Arc::new(Mutex::new(Vec::new()));
    let read = collapsing(reader(engine), collapsed.clone());
    let profile = profile();
    let vocabulary = skill_vocabulary();
    let (mut exact, mut refused) = (0usize, 0usize);
    let mut wrong = Vec::new();
    let mut seen_the_first_case = false;
    // Three pages keep the test well inside the runner's ceiling; they
    // include the page with `1,444.77`.
    for name in ["f65", "f71", "f72"] {
        let labels = &corpus.labels["frames"][name]["skills"];
        let image = corpus.frame(name);
        let page = read_skills_page(&image, &profile, &vocabulary, &read)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let labelled: Vec<(&str, &str)> = labels["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| {
                (
                    row["name"].as_str().unwrap_or(""),
                    row["points"].as_str().unwrap_or(""),
                )
            })
            .collect();
        for row in &page.rows {
            match labelled.iter().find(|(label, _)| row_is(row, label)) {
                Some((_, points)) if *points == row.points_text => exact += 1,
                Some((label, points)) => wrong.push(format!(
                    "{name}: {label} read as {:?}, the screen shows {points:?}",
                    row.points_text
                )),
                None => wrong.push(format!(
                    "{name}: a row was read that is not on screen: {row:?}"
                )),
            }
        }
        refused += page.unread.len();
        for unread in &page.unread {
            if unread.raw_points.as_deref() == Some("1,44.77") {
                seen_the_first_case = true;
            }
        }
    }
    let damaged = collapsed.lock().unwrap().clone();
    println!(
        "collapsed digits: {} numbers damaged ({}); rows: {exact} read exactly, {refused} refused, {} WRONG",
        damaged.len(),
        damaged
            .iter()
            .map(|(was, became)| format!("{was} -> {became}"))
            .collect::<Vec<_>>()
            .join(", "),
        wrong.len()
    );
    assert!(
        wrong.is_empty(),
        "a damaged number was accepted:\n{}",
        wrong.join("\n")
    );
    assert!(
        seen_the_first_case,
        "`1,444.77` read as `1,44.77` must be among the refusals"
    );
    assert!(refused > 0 && !damaged.is_empty());
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

/// The frames to replay for coordinates in shard `shard`.
fn coordinate_frames(corpus: &Corpus, shard: usize) -> Vec<(String, &Value)> {
    corpus
        .frames()
        .into_iter()
        .enumerate()
        .filter(|(index, _)| index % COORD_SHARDS == shard)
        .map(|(_, frame)| frame)
        .collect()
}

fn replay_coordinates_shard(shard: usize) {
    let Some(corpus) = Corpus::open() else { return };
    let Some(engine) = recogniser() else { return };
    let read = reader(engine);
    let profile = profile();
    let bounds = calypso();
    let mut tally = Tally::default();
    let mut covered = 0;
    let refusals = coord_refusals();
    let mut mismatched = Vec::new();
    for (name, labels) in coordinate_frames(&corpus, shard) {
        let image = corpus.frame(&name);
        let got = read_coordinates(&image, &profile, &bounds, &read);
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
    println!("coordinates shard {shard}: {covered} covered values refused as they must be");
    tally.report(&format!("coordinates, shard {shard}"));
    let unexcused: Vec<&String> = tally
        .refused
        .iter()
        .filter(|line| !line.ends_with("(excused)"))
        .collect();
    assert!(
        unexcused.is_empty(),
        "values the screen shows were refused: {unexcused:#?}"
    );
    assert!(
        mismatched.is_empty(),
        "values were refused for another reason than the one known: {mismatched:#?}"
    );
}

const COORD_SHARDS: usize = 8;

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
fn replay_digit_loss_shard(shard: usize) {
    let Some(corpus) = Corpus::open() else { return };
    let Some(engine) = recogniser() else { return };
    let read = collapsing_any(reader(engine));
    let profile = profile();
    let bounds = calypso();
    let (mut damaged, mut whole_read, mut whole_refused) = (0, 0, 0);
    let mut by: std::collections::BTreeMap<&'static str, usize> = Default::default();
    let mut wrong = Vec::new();
    for (name, labels) in coordinate_frames(&corpus, shard) {
        if labels.get("coords").is_none_or(Value::is_null) {
            continue;
        }
        let image = corpus.frame(&name);
        let got = match read_coordinates(&image, &profile, &bounds, &read) {
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
        "digit loss, shard {shard}: {damaged} values damaged, refused by {by:?}; {whole_read} undamaged read exactly, {whole_refused} undamaged refused; {} WRONG",
        wrong.len()
    );
    assert!(
        wrong.is_empty(),
        "a damaged coordinate was accepted:\n{}",
        wrong.join("\n")
    );
}

macro_rules! coord_shards {
    ($($name:ident, $loss:ident: $shard:expr,)*) => {$(
        #[test]
        fn $name() {
            replay_coordinates_shard($shard);
        }

        #[test]
        fn $loss() {
            replay_digit_loss_shard($shard);
        }
    )*};
}

coord_shards! {
    coordinates_shard_0, digit_loss_shard_0: 0,
    coordinates_shard_1, digit_loss_shard_1: 1,
    coordinates_shard_2, digit_loss_shard_2: 2,
    coordinates_shard_3, digit_loss_shard_3: 3,
    coordinates_shard_4, digit_loss_shard_4: 4,
    coordinates_shard_5, digit_loss_shard_5: 5,
    coordinates_shard_6, digit_loss_shard_6: 6,
    coordinates_shard_7, digit_loss_shard_7: 7,
}

/// The rings give one map scale whatever the radar's size, from the
/// distances read beside them: the default radar (f05, f26, f27) and the
/// enlarged one (f30, f32) agree on metres per pixel within 3%, and their
/// edges reach about 218 m and 329 m.
#[test]
fn the_ring_scale_is_the_same_for_a_default_and_an_enlarged_radar() {
    let Some(corpus) = Corpus::open() else { return };
    let Some(engine) = recogniser() else { return };
    let read = reader(engine);
    let profile = profile();
    let mut scales = Vec::new();
    for (name, edge_m) in [
        ("f05", 218.0),
        ("f26", 218.0),
        ("f27", 218.0),
        ("f30", 329.0),
        ("f32", 329.0),
    ] {
        let image = corpus.frame(name);
        let radar = find_radar(&image, &profile).unwrap_or_else(|| panic!("{name}: no radar"));
        let scale = ring_scale(&image, &radar, &profile, &read)
            .unwrap_or_else(|| panic!("{name}: no ring scale for {radar:?}"));
        println!(
            "ring scale {name}: radius {:.1} px, labels read {:?}, rings {:?}, unlabelled {:?}, {:.4} m/px, edge {:.1} m",
            radar.radius,
            scale.labels,
            scale
                .rings
                .iter()
                .map(|(r, d)| format!("{d} m at {r:.1} px"))
                .collect::<Vec<_>>(),
            scale
                .unlabelled
                .iter()
                .map(|r| format!("{r:.1} px"))
                .collect::<Vec<_>>(),
            scale.metres_per_pixel,
            scale.edge_range_m
        );
        assert!(
            (scale.edge_range_m / edge_m - 1.0).abs() <= 0.02,
            "{name}: the edge is at {:.1} m, not about {edge_m} m",
            scale.edge_range_m
        );
        scales.push(scale.metres_per_pixel);
    }
    let mean = scales.iter().sum::<f64>() / scales.len() as f64;
    for scale in &scales {
        assert!((scale / mean - 1.0).abs() <= 0.03, "{scales:?}");
    }
}
