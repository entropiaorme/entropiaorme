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

use eo_services::game_ui::field::Recogniser;
use eo_services::game_ui::geometry::BgrImage;
use eo_services::game_ui::locate::{read_window_field, ReadError};
use eo_services::game_ui::profile::UiProfile;
use eo_services::game_ui::radar::{find_radar, ring_scale};
use eo_services::game_ui::skills::{
    nearest_skill, read_skills_page, Admission, Checksum, NameReading, RowName, SkillRow,
    SkillVocabulary, SkillsPage, SkillsSweep, SkillsTableProfile,
};
use eo_services::ocr_engine::OcrEngine;
use serde_json::Value;

mod game_ui_replay;

use game_ui_replay::{
    coordinate_digit_loss, coordinates, field, plates, resources, row_is, sale_queue,
    sale_queue_digit_loss, same_skill, skill_vocabulary, skills, terminal, windows, Corpus,
    Outcome, Replay, Shard, REPAIR_COST, TRADE_TOTAL,
};

fn profile() -> UiProfile {
    UiProfile::load(&resources().join("ui_profile.json"))
        .expect("the bundled interface profile loads")
}

/// The corpus and the bundled profile, where this machine has a corpus.
fn corpus() -> Option<(Corpus, UiProfile)> {
    let Some(dir) = std::env::var_os("EO_GAME_UI_CORPUS_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
    else {
        eprintln!(
            "skipped: EO_GAME_UI_CORPUS_DIR is not set, so there are no real captures to replay"
        );
        return None;
    };
    let held_out = std::env::var_os("EO_GAME_UI_HELD_OUT").is_some();
    let corpus = Corpus::open(&dir, held_out).unwrap_or_else(|error| panic!("{error}"));
    Some((corpus, profile()))
}

/// What a replay that reads runs with: the corpus, the bundled profile
/// and the bundled recogniser.
fn replay() -> Option<Replay> {
    let (corpus, profile) = corpus()?;
    let read: Arc<dyn Recogniser> = recogniser()?;
    Some(Replay {
        corpus,
        profile,
        read,
    })
}

/// A replay passes when nothing fails it.
fn check(outcome: Outcome) {
    assert!(
        outcome.failures.is_empty(),
        "{}",
        outcome.failures.join("\n")
    );
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

/// The window locator against every capture, in shards: an unoptimised
/// build takes a few seconds a frame. It needs no recogniser.
const WINDOW_SHARDS: usize = 8;

macro_rules! window_shards {
    ($($name:ident: $shard:expr,)*) => {$(
        #[test]
        fn $name() {
            let Some((corpus, profile)) = corpus() else { return };
            check(windows(&corpus, &profile, Shard(Some(($shard, WINDOW_SHARDS)))));
        }
    )*};
}

window_shards! {
    corpus_windows_shard_0: 0,
    corpus_windows_shard_1: 1,
    corpus_windows_shard_2: 2,
    corpus_windows_shard_3: 3,
    corpus_windows_shard_4: 4,
    corpus_windows_shard_5: 5,
    corpus_windows_shard_6: 6,
    corpus_windows_shard_7: 7,
}

/// Every repair frame shows its cost in full, so a refusal fails too.
#[test]
fn repair_cost_is_read_exactly_or_refused() {
    let Some(r) = replay() else { return };
    check(field(&r, &REPAIR_COST));
}

/// Every trade frame shows its total in full, so a refusal fails too.
#[test]
fn trade_terminal_total_is_read_exactly_or_refused() {
    let Some(r) = replay() else { return };
    check(field(&r, &TRADE_TOTAL));
}

/// A frame with the terminal closed must refuse, not read something
/// else's number.
#[test]
fn a_closed_terminal_is_a_refusal() {
    let Some(r) = replay() else { return };
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
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
            let got = read_window_field(&image, profile, window, field, &read);
            assert!(
                matches!(got, Err(ReadError::WindowNotFound)),
                "{name}: {window}.{field} answered {got:?} with the terminal closed"
            );
        }
        checked += 1;
    }
    println!("closed terminal: {checked} frames with other windows open, all refused");
}

#[test]
fn the_repair_terminal_read_answers_the_amount_on_screen() {
    let Some(r) = replay() else { return };
    let outcome = terminal(&r, &REPAIR_COST);
    assert!(outcome.exact > 0, "the corpus has repair frames to read");
    check(outcome);
}

#[test]
fn the_trade_terminal_read_answers_the_amount_on_screen() {
    let Some(r) = replay() else { return };
    let outcome = terminal(&r, &TRADE_TOTAL);
    assert!(outcome.exact > 0, "the corpus has trade frames to read");
    check(outcome);
}

/// The one frame refused for a covered title loses only the rows the
/// labels say are readable there; the rest are under the window in front.
#[test]
fn a_frame_refused_for_a_covered_title_hides_only_one_readable_row() {
    let Some((corpus, _)) = corpus() else { return };
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
            let Some(r) = replay() else { return };
            check(skills(&r, Shard(Some(($shard, SKILLS_SHARDS)))));
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
    let Some(r) = replay() else { return };
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let table = skills_table(profile);
    let vocabulary = skill_vocabulary();
    let page =
        read_skills_page(&corpus.frame("f66"), profile, &vocabulary, &read).expect("f66 reads");
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
    let Some(r) = replay() else { return };
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let vocabulary = skill_vocabulary();
    let mut sweep = SkillsSweep::new(&skills_table(profile), &vocabulary);
    let page = |frame: &str| {
        read_skills_page(&corpus.frame(frame), profile, &vocabulary, &read)
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
    let Some(r) = replay() else { return };
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let vocabulary = skill_vocabulary();
    let pages: Vec<SkillsPage> = SWEEP_FRAMES
        .iter()
        .map(|name| {
            read_skills_page(&corpus.frame(name), profile, &vocabulary, &read)
                .unwrap_or_else(|e| panic!("{name}: {e}"))
        })
        .collect();
    let mut sweep = SkillsSweep::new(&skills_table(profile), &vocabulary);
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
    let (known, lacking) = sweep_labels(corpus, &vocabulary);
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
    let near_misses = result
        .skills
        .iter()
        .filter(|s| s.reading != NameReading::Spelled)
        .count();
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
    let Some(r) = replay() else { return };
    let (corpus, profile) = (&r.corpus, &r.profile);
    let collapsed = Arc::new(Mutex::new(Vec::new()));
    let read = collapsing(r.reader(), collapsed.clone());
    let vocabulary = skill_vocabulary();
    let (mut exact, mut refused) = (0usize, 0usize);
    let mut wrong = Vec::new();
    let mut seen_the_first_case = false;
    // Three pages keep the test well inside the runner's ceiling; they
    // include the page with `1,444.77`.
    for name in ["f65", "f71", "f72"] {
        let labels = &corpus.labels["frames"][name]["skills"];
        let image = corpus.frame(name);
        let page = read_skills_page(&image, profile, &vocabulary, &read)
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

const COORD_SHARDS: usize = 8;

macro_rules! coord_shards {
    ($($name:ident, $loss:ident: $shard:expr,)*) => {$(
        #[test]
        fn $name() {
            let Some(r) = replay() else { return };
            check(coordinates(&r, Shard(Some(($shard, COORD_SHARDS)))));
        }

        #[test]
        fn $loss() {
            let Some(r) = replay() else { return };
            check(coordinate_digit_loss(&r, Shard(Some(($shard, COORD_SHARDS)))));
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
    let Some(r) = replay() else { return };
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let mut scales = Vec::new();
    for (name, edge_m) in [
        ("f05", 218.0),
        ("f26", 218.0),
        ("f27", 218.0),
        ("f30", 329.0),
        ("f32", 329.0),
    ] {
        let image = corpus.frame(name);
        let radar = find_radar(&image, profile).unwrap_or_else(|| panic!("{name}: no radar"));
        let scale = ring_scale(&image, &radar, profile, &read)
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

const SALE_SHARDS: usize = 4;

macro_rules! sale_shards {
    ($($name:ident, $loss:ident: $shard:expr,)*) => {$(
        #[test]
        fn $name() {
            let Some(r) = replay() else { return };
            check(sale_queue(&r, Shard(Some(($shard, SALE_SHARDS)))));
        }

        /// Every shard holds a row with nothing to damage, which must
        /// read: the replay fails a shard where none did.
        #[test]
        fn $loss() {
            let Some(r) = replay() else { return };
            check(sale_queue_digit_loss(&r, Shard(Some(($shard, SALE_SHARDS)))));
        }
    )*};
}

sale_shards! {
    sale_queue_shard_0, sale_queue_digit_loss_shard_0: 0,
    sale_queue_shard_1, sale_queue_digit_loss_shard_1: 1,
    sale_queue_shard_2, sale_queue_digit_loss_shard_2: 2,
    sale_queue_shard_3, sale_queue_digit_loss_shard_3: 3,
}

// Creatures' nameplates: every plate the labels list is found and no
// plate anywhere else, the locked plate is the one the manifest places,
// and its level reads exactly or is refused as the manifest says.

const PLATE_SHARDS: usize = 4;

macro_rules! plate_shards {
    ($($name:ident: $shard:expr,)*) => {$(
        #[test]
        fn $name() {
            let Some(r) = replay() else { return };
            check(plates(&r, Shard(Some(($shard, PLATE_SHARDS)))));
        }
    )*};
}

plate_shards! {
    plates_shard_0: 0,
    plates_shard_1: 1,
    plates_shard_2: 2,
    plates_shard_3: 3,
}
