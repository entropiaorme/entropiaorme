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
    coordinate_digit_loss, coordinates, field, pinned_frames, plates, resources, row_is,
    sale_queue, sale_queue_digit_loss, same_skill, skill_vocabulary, skills, terminal, tool_bar,
    windows, Corpus, Outcome, Replay, Shard, REPAIR_COST, TRADE_TOTAL,
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

/// A replay passes when nothing fails it: no wrong value, no refusal
/// unexplained, and no frame missing a manifest entry it needs.
fn check(outcome: Outcome) {
    assert!(
        outcome.passed(),
        "{}",
        outcome
            .failures
            .iter()
            .chain(outcome.missing.iter().map(|line| line as &String))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
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

/// The manifest names the frames the tests below are pinned to, each one
/// a frame of the corpus.
#[test]
fn the_manifest_names_every_pinned_frame() {
    let Some((corpus, _)) = corpus() else { return };
    check(pinned_frames(&corpus));
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

/// The frame refused for a covered title loses only the rows the labels
/// say are readable there, as many as the manifest says; the rest are
/// under the window in front.
#[test]
fn a_frame_refused_for_a_covered_title_hides_only_its_readable_rows() {
    let Some((corpus, _)) = corpus() else { return };
    let case = &corpus
        .cases()
        .unwrap_or_else(|e| panic!("{e}"))
        .covered_title;
    let rows = corpus.labels["frames"][case.frame.as_str()]["skills"]["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("{}: no skills rows", case.frame))
        .clone();
    let readable: Vec<&Value> = rows
        .iter()
        .filter(|row| row["fully_visible"].as_bool() == Some(true))
        .collect();
    assert_eq!(readable.len(), case.readable_rows, "{readable:?}");
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

/// The labelled rows of `frames`, page by page, split into the names the
/// bundled vocabulary knows (with the blank row) and those it lacks.
fn rows_by_vocabulary(
    corpus: &Corpus,
    frames: &[String],
    vocabulary: &SkillVocabulary,
) -> (Vec<Value>, Vec<Value>) {
    frames
        .iter()
        .flat_map(|frame| {
            corpus.labels["frames"][frame.as_str()]["skills"]["rows"]
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
    let frame = &corpus
        .cases()
        .unwrap_or_else(|e| panic!("{e}"))
        .vocabulary_lacks;
    let page = read_skills_page(&corpus.frame(frame), profile, &vocabulary, &read)
        .unwrap_or_else(|e| panic!("{frame}: {e}"));
    assert!(page.whole, "{frame}: {page:?}");
    let (_, lacking) = rows_by_vocabulary(corpus, std::slice::from_ref(frame), &vocabulary);
    assert!(
        !lacking.is_empty(),
        "{frame}: the labels show no skill the vocabulary lacks"
    );
    let unknown: Vec<&SkillRow> = page
        .rows
        .iter()
        .filter(|row| row.name == RowName::Unknown)
        .collect();
    assert_eq!(
        unknown
            .iter()
            .map(|row| (row.raw_name.as_str(), row.points_text.as_str()))
            .collect::<Vec<_>>(),
        lacking
            .iter()
            .map(|row| (
                row["name"].as_str().expect("named"),
                row["points"].as_str().expect("points")
            ))
            .collect::<Vec<_>>()
    );
    for row in unknown {
        let (nearest, score) = nearest_skill(&row.raw_name, &vocabulary).expect("a nearest skill");
        println!(
            "skill the bundled vocabulary lacks: {:?} = {}, nearest {nearest:?} at {score:.1} (near-miss floor {}, sweep floor {})",
            row.raw_name, row.points_text, table.near_miss.score, table.near_miss.sweep_score
        );
    }
}

/// A page's number and total as its labels give them.
fn labelled_page(corpus: &Corpus, frame: &str) -> (Option<(u32, u32)>, Option<u64>) {
    let labels = &corpus.labels["frames"][frame]["skills"];
    let page = labels["page"].as_str().and_then(|p| {
        let (page, count) = p.split_once('/')?;
        Some((page.trim().parse().ok()?, count.trim().parse().ok()?))
    });
    let total = labels["total_text"]
        .as_str()
        .and_then(|t| t.rsplit(' ').next()?.parse().ok());
    (page, total)
}

/// Two different readings of one page (the list before and after a skill
/// rose: the same skills, another total): the later one is kept, each
/// time. Then the next page's rows arriving under the number of the page
/// before it, as a misread pager would bring them, unsettle that page
/// until it is read again.
#[test]
fn rereadings_replace_and_a_misplaced_page_unsettles_its_number() {
    let Some(r) = replay() else { return };
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let vocabulary = skill_vocabulary();
    let case = &corpus.cases().unwrap_or_else(|e| panic!("{e}")).rereading;
    let mut sweep = SkillsSweep::new(&skills_table(profile), &vocabulary);
    let page = |frame: &str| {
        let page = read_skills_page(&corpus.frame(frame), profile, &vocabulary, &read)
            .unwrap_or_else(|e| panic!("{frame}: {e}"));
        assert_eq!(
            (page.page, page.total),
            labelled_page(corpus, frame),
            "{frame}"
        );
        page
    };
    let (older, newer) = (page(&case.older), page(&case.newer));
    let number = older.page.expect("a page number").0;
    assert_eq!(older.page, newer.page, "the two readings are of one page");
    assert_ne!(older.total, newer.total, "the two readings differ");
    assert_eq!(sweep.accept(older.clone()), Admission::Kept);
    assert_eq!(sweep.accept(newer.clone()), Admission::Kept);
    assert_eq!(sweep.page(number).and_then(|p| p.total), newer.total);
    assert_eq!(sweep.accept(older.clone()), Admission::Kept);
    assert_eq!(sweep.page(number).and_then(|p| p.total), older.total);

    let [first, second] = &case.consecutive;
    let (before, after) = (page(first), page(second));
    let (at, count) = before.page.expect("a page number");
    assert_eq!(
        after.page,
        Some((at + 1, count)),
        "the pages are consecutive"
    );
    assert_eq!(sweep.accept(before.clone()), Admission::Kept);
    let mut misplaced = after;
    misplaced.page = before.page;
    assert_eq!(sweep.accept(misplaced), Admission::Conflict);
    assert!(sweep.missing().expect("count known").contains(&at));
    assert_eq!(sweep.accept(before.clone()), Admission::Kept);
    assert_eq!(sweep.page(at), Some(&before));
    println!("skills sweep rereadings: page {number} kept from the later reading each time; page {at} unsettled by a misplaced page and settled by a reread");
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
    let frames = &corpus.cases().unwrap_or_else(|e| panic!("{e}")).sweep;
    let pages: Vec<SkillsPage> = frames
        .iter()
        .map(|name| {
            read_skills_page(&corpus.frame(name), profile, &vocabulary, &read)
                .unwrap_or_else(|e| panic!("{name}: {e}"))
        })
        .collect();
    let mut sweep = SkillsSweep::new(&skills_table(profile), &vocabulary);
    // Out of order, by a stride through the pages that visits each once,
    // and the second page read twice.
    let n = pages.len();
    let stride = (n / 2..n).find(|s| gcd(*s, n) == 1).unwrap_or(1);
    let order: Vec<usize> = (0..n)
        .map(|k| (k * stride + n - 1) % n)
        .chain([1 % n])
        .collect();
    for index in order {
        assert_eq!(
            sweep.accept(pages[index].clone()),
            Admission::Kept,
            "{} (page {}) is whole",
            frames[index],
            index + 1
        );
    }
    assert_eq!(sweep.missing(), Some(Vec::new()));
    let result = sweep.finish().expect("every page is whole");
    let (known, lacking) = rows_by_vocabulary(corpus, frames, &vocabulary);
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
        frames.len(),
        result.skills.len(),
        result.unknown.len(),
        result.checksum,
        result.total
    );
    assert_eq!(result.total, labelled_page(corpus, &frames[0]).1);
    assert_eq!(result.checksum, Checksum::Matches);
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
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
    // A few pages keep the test well inside the runner's ceiling; they
    // include the page with `1,444.77`.
    for name in &corpus
        .cases()
        .unwrap_or_else(|e| panic!("{e}"))
        .dropped_digits
    {
        let name = name.as_str();
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
/// distances read beside them: radars of different sizes agree on metres
/// per pixel within 3%, and each one's edge reaches the distance the
/// manifest gives for it, within 2%.
#[test]
fn the_ring_scale_is_the_same_for_a_default_and_an_enlarged_radar() {
    let Some(r) = replay() else { return };
    let (corpus, profile) = (&r.corpus, &r.profile);
    let read = r.reader();
    let mut scales = Vec::new();
    let cases = &corpus.cases().unwrap_or_else(|e| panic!("{e}")).ring_scale;
    let mut radii = Vec::new();
    for case in cases {
        let (name, edge_m) = (case.frame.as_str(), case.edge_m);
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
        radii.push(radar.radius);
    }
    // The case holds radars of more than one size, or it proves nothing.
    assert!(
        radii.iter().any(|r| (r / radii[0] - 1.0).abs() > 0.1),
        "every radar measured is the same size: {radii:?}"
    );
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

// The tool bar: every frame's bar in the state its labels give (or the
// manifest names), each name read or its misreading counted, and the
// lettering's masks apart between tools and alike within one.

const TOOL_BAR_SHARDS: usize = 4;

macro_rules! tool_bar_shards {
    ($($name:ident: $shard:expr,)*) => {$(
        #[test]
        fn $name() {
            let Some(r) = replay() else { return };
            check(tool_bar(&r, Shard(Some(($shard, TOOL_BAR_SHARDS)))));
        }
    )*};
}

tool_bar_shards! {
    tool_bar_shard_0: 0,
    tool_bar_shard_1: 1,
    tool_bar_shard_2: 2,
    tool_bar_shard_3: 3,
}
