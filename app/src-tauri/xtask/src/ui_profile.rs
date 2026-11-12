//! Four whole-tree rules keeping the game-interface readers on their one
//! pipeline (`eo-services/src/game_ui/`) and everything they know about how
//! the client draws its interface in its one data file (the interface
//! profile, `entropia-orme/resources/ui_profile.json`):
//!
//!   - Rule (no reader bypasses the pipeline): screen pixels reach the
//!     recogniser only through the seams the composition root builds. The
//!     gated functions are derived from the modules that define them: every
//!     public function of the screen-capture modules and of the recogniser
//!     module is gated unless it is listed below as one that touches no
//!     pixels, with its reason, so a new capture function is gated the
//!     moment it exists. Outside its own module, a gated function may be
//!     named only by an allowlisted file, for the names its entry lists.
//!     The recogniser is handed to the readers as a trait, and a call to
//!     one of that trait's methods is as much a way to the recogniser as
//!     a call to the engine: the methods are read from the trait's own
//!     definition and gated too. Inside the readers such a call is the
//!     pipeline itself; anywhere else it is a bypass unless allowlisted.
//!     Implementing the trait is a definition, not a call. Being textual,
//!     the scan knows a call by the method's name reached through `.` (or
//!     by the trait's path) in a file that names the trait, which every
//!     file calling it on an engine must; a call on a trait object in a
//!     file that never names the trait is beyond it.
//!     The allowlist only shrinks: an entry whose file no longer names the
//!     function fails, as does a helper entry naming no public function.
//!   - Rule (no layout literals in the readers): in the code of
//!     `eo-services/src/game_ui/` outside `#[cfg(test)]` items and test-only
//!     modules, a number is suspect, because a threshold, offset, size,
//!     ratio or colour belongs in the profile. The only integer literals
//!     allowed are those of value 0, 1, 2 and 3; the only float literals,
//!     0.0, 0.5, 1.0 and 2.0. Three structural contexts are allowed
//!     whatever the value: an array's length (`[T; N]`, `[x; N]`), a tuple
//!     field (`pair.1`), and a binary literal (`0b0100`, a bit pattern
//!     rather than a magnitude). Any other literal passes only on a line
//!     whose trailing comment starts `// not layout:` and gives a reason;
//!     the guard counts these, refuses one without a reason, and refuses
//!     one on a line with nothing for it to excuse.
//!   - Rule (the profile rejects what it does not know): every struct in
//!     those readers that derives `Deserialize` carries
//!     `#[serde(deny_unknown_fields)]`, so a stale or misspelt entry in the
//!     profile fails to load instead of being ignored.
//!   - Rule (the readers' test fixtures never ship): the `test-support`
//!     feature of `eo-services` is enabled only from a `[dev-dependencies]`
//!     table, never from a normal or workspace dependency, another feature,
//!     or its own crate's `default`.
//!
//! Whole-tree rather than diff-scoped, like the sibling guards: the tree was
//! driven to zero offending sites. The scan is textual (xtask keeps its
//! dependency set minimal): comments, strings and character literals are
//! blanked before any match, so prose and messages never register as code.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use regex::Regex;

use crate::git;

/// The modules whose public functions reach the screen.
const CAPTURE_MODULES: &[&str] = &[
    "app/src-tauri/eo-services/src/screen_capture/mod.rs",
    "app/src-tauri/eo-services/src/eu_window.rs",
];

/// The module whose public functions reach the recogniser.
const RECOGNISER_MODULES: &[&str] = &["app/src-tauri/eo-services/src/ocr_engine.rs"];

/// The trait a recogniser is handed to the readers as: the reader module
/// that defines it, and its name. Its methods are gated outside the
/// readers.
const SEAM_TRAIT: (&str, &str) = (
    "app/src-tauri/eo-services/src/game_ui/field.rs",
    "Recogniser",
);

/// Public functions of those modules that touch no pixels: `(module, name,
/// reason)`. Everything else they make public is gated.
const NOT_GATED: &[(&str, &str, &str)] = &[
    (
        "app/src-tauri/eo-services/src/screen_capture/mod.rs",
        "frame_to_png",
        "encodes a frame already captured",
    ),
    (
        "app/src-tauri/eo-services/src/screen_capture/mod.rs",
        "write_debug_frame",
        "saves a frame already captured",
    ),
    (
        "app/src-tauri/eo-services/src/eu_window.rs",
        "find_game_window",
        "finds the game's window without capturing it",
    ),
    (
        "app/src-tauri/eo-services/src/eu_window.rs",
        "get_window_geometry",
        "measures the game's window without capturing it",
    ),
    (
        "app/src-tauri/eo-services/src/eu_window.rs",
        "capturable_part",
        "rectangle arithmetic",
    ),
    (
        "app/src-tauri/eo-services/src/eu_window.rs",
        "game_window_present",
        "whether the game's window exists",
    ),
    (
        "app/src-tauri/eo-services/src/eu_window.rs",
        "game_focus",
        "which window holds keyboard focus",
    ),
    (
        "app/src-tauri/eo-services/src/eu_window.rs",
        "cursor_position",
        "where the pointer is",
    ),
    (
        "app/src-tauri/eo-services/src/ocr_engine.rs",
        "load_bgr_png",
        "decodes an image file",
    ),
    (
        "app/src-tauri/eo-services/src/ocr_engine.rs",
        "resize_bilinear",
        "resamples an image the caller holds",
    ),
    (
        "app/src-tauri/eo-services/src/ocr_engine.rs",
        "resize_bilinear_into",
        "resamples an image the caller holds",
    ),
    (
        "app/src-tauri/eo-services/src/ocr_engine.rs",
        "preprocess",
        "normalises an image the caller holds",
    ),
    (
        "app/src-tauri/eo-services/src/ocr_engine.rs",
        "preprocess_into",
        "normalises an image the caller holds",
    ),
    (
        "app/src-tauri/eo-services/src/ocr_engine.rs",
        "load_dict",
        "reads the recogniser's alphabet",
    ),
    (
        "app/src-tauri/eo-services/src/ocr_engine.rs",
        "ctc_decode",
        "decodes model output the caller holds",
    ),
    (
        "app/src-tauri/eo-services/src/ocr_engine.rs",
        "new",
        "loads the recogniser",
    ),
    (
        "app/src-tauri/eo-services/src/ocr_engine.rs",
        "new_with_providers",
        "loads the recogniser",
    ),
    (
        "app/src-tauri/eo-services/src/ocr_engine.rs",
        "provider",
        "names the recogniser's execution provider",
    ),
    (
        "app/src-tauri/eo-services/src/ocr_engine.rs",
        "warm_up",
        "runs the model once on a blank image of its own",
    ),
];

/// A file allowed to name gated functions: the names, and why.
struct Allowed {
    path: &'static str,
    names: &'static [&'static str],
    reason: &'static str,
}

/// Where the pipeline's seams are built, and the tools and tests that
/// legitimately reach the screen or the recogniser directly.
const ALLOWED: &[Allowed] = &[
    Allowed {
        path: "app/src-tauri/entropia-orme/src/composition.rs",
        names: &[
            "capture_game_window",
            "capture_game_window_running",
            "capture_running",
            "start_capture",
            "recognize_bgr",
        ],
        reason: "the composition root: builds the frame seam, the capture probes and the \
recogniser seam the readers are handed",
    },
    Allowed {
        path: "app/src-tauri/eo-services/src/eu_window.rs",
        names: &[
            "capture_region_bgr",
            "capture_region_bgr_running",
            "capturable_area",
            "capturable_area_running",
            "capture_running",
            "start_capture",
        ],
        reason: "the frame seam's constructor: captures the game window's client area",
    },
    Allowed {
        path: "app/src-tauri/eo-services/examples/capture_probe.rs",
        names: &["capture_region_png", "capture_frame_generation"],
        reason: "a maintainer tool that saves what the capturer sees",
    },
    Allowed {
        path: "app/src-tauri/eo-services/tests/ocr_bench_differential.rs",
        names: &["recognize_png"],
        reason: "the recogniser's own differential test",
    },
];

/// Readers not yet on the pipeline: debt, kept visible here until it is
/// paid. Same shape and the same ratchet as [`ALLOWED`].
const TO_MIGRATE: &[Allowed] = &[Allowed {
    path: "app/src-tauri/entropia-orme/src/composition.rs",
    names: &["capture_region_bgr"],
    reason: "the hand-drawn coordinate region: coord_capture reads a raw screen rectangle \
and hands it to the recogniser itself, not through game_ui",
}];

/// The readers whose code holds no layout numbers.
const READERS_ROOT: &str = "app/src-tauri/eo-services/src/game_ui/";

/// The feature that exposes the readers' test fixtures, and its crate.
const FIXTURE_FEATURE: &str = "test-support";
const FIXTURE_CRATE: &str = "eo-services";

/// The comment that excuses a literal on its line.
const EXEMPTION: &str = "// not layout:";

const ALLOWED_INTEGERS: &[u128] = &[0, 1, 2, 3];
const ALLOWED_FLOATS: &[f64] = &[0.0, 0.5, 1.0, 2.0];

/// A single guard violation: file, 1-based line number, rule, detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    pub lineno: usize,
    pub rule: &'static str,
    pub detail: String,
}

impl Finding {
    fn new(path: &str, lineno: usize, rule: &'static str, detail: String) -> Self {
        Self {
            path: path.to_string(),
            lineno,
            rule,
            detail,
        }
    }
}

// ---------------------------------------------------------------------------
// Lexing: blank comments, strings and character literals.
// ---------------------------------------------------------------------------

/// A source with its comments, strings and character literals blanked
/// (byte offsets and newlines kept), and its line comments by line.
struct Masked {
    code: String,
    /// Where each line starts.
    starts: Vec<usize>,
    /// 1-based line -> the text of the `//` comment that ends it.
    line_comments: BTreeMap<usize, String>,
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn blank(out: &mut [u8], from: usize, to: usize) {
    for byte in &mut out[from..to] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

/// The byte offset at which each line of `text` starts.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            text.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        )
        .collect()
}

/// The 1-based line holding `offset`, given the text's line starts.
fn line_at(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|start| *start <= offset)
}

/// The end of a raw string starting at `i` (`r"`, `r#"`, `br"`, `cr#"`),
/// or `None` when `i` does not start one.
fn raw_string_end(b: &[u8], i: usize) -> Option<usize> {
    if i > 0 && is_ident(b[i - 1]) {
        return None;
    }
    let mut k = i;
    if matches!(b[k], b'b' | b'c') {
        k += 1;
    }
    if b.get(k) != Some(&b'r') {
        return None;
    }
    k += 1;
    let hashes = b[k..].iter().take_while(|c| **c == b'#').count();
    k += hashes;
    if b.get(k) != Some(&b'"') {
        return None;
    }
    k += 1;
    while k < b.len() {
        if b[k] == b'"'
            && b[k + 1..]
                .iter()
                .take(hashes)
                .filter(|c| **c == b'#')
                .count()
                == hashes
        {
            return Some(k + 1 + hashes);
        }
        k += 1;
    }
    Some(b.len())
}

fn mask(text: &str) -> Masked {
    let b = text.as_bytes();
    let n = b.len();
    let starts = line_starts(text);
    let mut out = b.to_vec();
    let mut line_comments = BTreeMap::new();
    let mut i = 0;
    while i < n {
        if b[i..].starts_with(b"//") {
            let end = b[i..].iter().position(|c| *c == b'\n').map_or(n, |p| i + p);
            line_comments.insert(line_at(&starts, i), text[i..end].trim_end().to_string());
            blank(&mut out, i, end);
            i = end;
        } else if b[i..].starts_with(b"/*") {
            let (mut depth, mut j) = (0usize, i);
            while j < n {
                if b[j..].starts_with(b"/*") {
                    depth += 1;
                    j += 2;
                } else if b[j..].starts_with(b"*/") {
                    depth -= 1;
                    j += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            blank(&mut out, i, j);
            i = j;
        } else if let Some(end) = raw_string_end(b, i) {
            blank(&mut out, i, end);
            i = end;
        } else if b[i] == b'"' {
            let mut j = i + 1;
            while j < n && b[j] != b'"' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(n);
            blank(&mut out, i, end);
            i = end;
        } else if b[i] == b'\'' {
            // A character literal, or else a lifetime or a label.
            let end = if b.get(i + 1) == Some(&b'\\') {
                // The escape's own first character may be a quote (`'\''`).
                let from = (i + 3).min(n);
                b[from..]
                    .iter()
                    .take(12)
                    .position(|c| *c == b'\'')
                    .map(|p| from + p + 1)
            } else {
                text[i + 1..].chars().next().and_then(|c| {
                    let close = i + 1 + c.len_utf8();
                    (b.get(close) == Some(&b'\'')).then_some(close + 1)
                })
            };
            match end {
                Some(end) => {
                    blank(&mut out, i, end);
                    i = end;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    Masked {
        code: String::from_utf8(out).expect("blanking keeps the text valid"),
        starts,
        line_comments,
    }
}

// ---------------------------------------------------------------------------
// Blocks: which items are tests, modules, implementations.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// An item under `#[cfg(test)]`.
    Test,
    /// An inline module, public or not.
    Module { public: bool },
    /// An `impl` block.
    Impl,
    /// Anything else: a function body, an expression, a struct.
    Other,
}

/// One braced block (or, for a test item without braces, the item): from
/// the start of its head to its close.
#[derive(Debug, Clone, Copy)]
struct Block {
    kind: Kind,
    head: usize,
    open: usize,
    close: usize,
}

fn attribute_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"#!?\[[^\]]*\]").expect("valid attribute pattern"))
}

fn module_head_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(pub(\s*\([^)]*\))?\s+)?mod\s+\w+\s*$").expect("valid module pattern")
    })
}

fn impl_head_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(unsafe\s+)?impl\b").expect("valid impl pattern"))
}

fn is_test_head(head: &str) -> bool {
    let squeezed: String = head.chars().filter(|c| !c.is_whitespace()).collect();
    squeezed.contains("#[cfg(test)]")
}

fn classify_head(head: &str) -> Kind {
    if is_test_head(head) {
        return Kind::Test;
    }
    let bare = attribute_re().replace_all(head, " ");
    let bare = bare.trim();
    if let Some(caps) = module_head_re().captures(bare) {
        return Kind::Module {
            public: caps.get(1).is_some(),
        };
    }
    if impl_head_re().is_match(bare) {
        return Kind::Impl;
    }
    Kind::Other
}

fn blocks(code: &str) -> Vec<Block> {
    let b = code.as_bytes();
    let mut out = Vec::new();
    let mut stack: Vec<(Kind, usize, usize)> = Vec::new();
    let mut head = 0;
    for (i, byte) in b.iter().enumerate() {
        match *byte {
            b'{' => {
                stack.push((classify_head(&code[head..i]), head, i));
                head = i + 1;
            }
            b'}' => {
                if let Some((kind, start, open)) = stack.pop() {
                    out.push(Block {
                        kind,
                        head: start,
                        open,
                        close: i,
                    });
                }
                head = i + 1;
            }
            b';' => {
                if is_test_head(&code[head..i]) {
                    out.push(Block {
                        kind: Kind::Test,
                        head,
                        open: i,
                        close: i,
                    });
                }
                head = i + 1;
            }
            _ => {}
        }
    }
    out
}

fn in_test(blocks: &[Block], at: usize) -> bool {
    blocks
        .iter()
        .any(|b| b.kind == Kind::Test && b.head <= at && at <= b.close)
}

// ---------------------------------------------------------------------------
// Rule 1: gated functions.
// ---------------------------------------------------------------------------

/// A gated function: where it is defined, its name, and whether it is
/// called as a method.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Gated {
    pub module: String,
    pub name: String,
    pub method: bool,
    /// For a method of the recogniser trait, the trait's name: such a
    /// method is gated outside the readers, and known by a call through
    /// `.` or the trait's path in a file that names the trait.
    pub seam: Option<String>,
}

/// The methods the trait `name` declares in `text`: the functions at the
/// top level of its body. `None` when `text` defines no such trait.
pub fn trait_methods(text: &str, name: &str) -> Option<Vec<String>> {
    let masked = mask(text);
    let code = &masked.code;
    let head = Regex::new(&format!(r"\btrait\s+{}\b[^{{;]*\{{", regex::escape(name)))
        .expect("valid trait pattern");
    let open = head.find(code)?.end();
    let mut depth = 1usize;
    let mut close = code.len();
    for (i, byte) in code.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    close = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &code[open..close];
    let declared = Regex::new(r"\bfn\s+(\w+)").expect("valid function pattern");
    let mut methods = Vec::new();
    let mut depth = 0usize;
    let mut from = 0;
    // Only the body's own level: a default method's body may define
    // functions of its own.
    for (i, byte) in body.bytes().enumerate() {
        match byte {
            b'{' => {
                if depth == 0 {
                    methods.extend(
                        declared
                            .captures_iter(&body[from..i])
                            .map(|c| c[1].to_string()),
                    );
                }
                depth += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    from = i + 1;
                }
            }
            _ => {}
        }
    }
    methods.extend(
        declared
            .captures_iter(&body[from..])
            .map(|c| c[1].to_string()),
    );
    Some(methods)
}

/// The recogniser trait's methods, gated: from the text of the module
/// that defines the trait.
fn seam_methods(module: &str, text: &str, name: &str) -> Vec<Gated> {
    trait_methods(text, name)
        .unwrap_or_default()
        .into_iter()
        .map(|method| Gated {
            module: module.to_string(),
            name: method,
            method: true,
            seam: Some(name.to_string()),
        })
        .collect()
}

fn public_fn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\bpub(\s*\([^)]*\))?\s+((const|async|unsafe|extern)\s+)*fn\s+(\w+)")
            .expect("valid public function pattern")
    })
}

/// The public functions a module defines, `(name, is a method)`: those at
/// its top level, in its public inline modules, and in its `impl` blocks;
/// not those in private inline modules, function bodies or tests.
pub fn public_functions(text: &str) -> Vec<(String, bool)> {
    let masked = mask(text);
    let blocks = blocks(&masked.code);
    let mut out = Vec::new();
    for caps in public_fn_re().captures_iter(&masked.code) {
        let at = caps.get(0).expect("whole match").start();
        let enclosing: Vec<&Block> = blocks
            .iter()
            .filter(|b| b.open < at && at < b.close)
            .collect();
        let hidden = enclosing.iter().any(|b| {
            matches!(
                b.kind,
                Kind::Test | Kind::Other | Kind::Module { public: false }
            )
        });
        if hidden || in_test(&blocks, at) {
            continue;
        }
        let method = enclosing.iter().any(|b| b.kind == Kind::Impl);
        out.push((caps[4].to_string(), method));
    }
    out
}

/// Every public function of the capture and recogniser modules that is not
/// listed as touching no pixels, from the modules' texts; and the helper
/// entries that name no public function.
fn gated_functions(modules: &BTreeMap<String, String>) -> (Vec<Gated>, Vec<Finding>) {
    let mut gated = Vec::new();
    let mut findings = Vec::new();
    for (module, text) in modules {
        let defined = public_functions(text);
        for (name, method) in &defined {
            let helper = NOT_GATED.iter().any(|(m, n, _)| m == module && n == name);
            if !helper {
                gated.push(Gated {
                    module: module.clone(),
                    name: name.clone(),
                    method: *method,
                    seam: None,
                });
            }
        }
        for (_, name, _) in NOT_GATED.iter().filter(|(m, _, _)| m == module) {
            if !defined.iter().any(|(d, _)| d == name) {
                findings.push(Finding::new(
                    module,
                    0,
                    "stale-helper",
                    format!(
                        "`{name}` is listed as touching no pixels but is no longer a public \
function here; remove its entry"
                    ),
                ));
            }
        }
    }
    gated.sort();
    gated.dedup();
    (gated, findings)
}

/// Where `path` names a gated function outside the function's own module:
/// `(name, line)`. A definition, a field of the same name, and (for a free
/// function) a field or method reached through `.` are not references.
///
/// A method of the recogniser trait is gated outside the readers only,
/// and its name is as common as `read`, so it is a reference only where
/// it is a call (`(` follows) reached through `.` or through the trait's
/// own path, in a file that names the trait. An implementation of the
/// trait declares the method and calls nothing, so it is not one.
pub fn references(path: &str, text: &str, gated: &[Gated]) -> Vec<(String, usize)> {
    let posix = path.replace('\\', "/");
    let candidates: Vec<&Gated> = gated
        .iter()
        .filter(|g| g.module != posix && text.contains(g.name.as_str()))
        .filter(|g| g.seam.is_none() || !posix.starts_with(READERS_ROOT))
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }
    let masked = mask(text);
    let code = &masked.code;
    let mut out = Vec::new();
    for g in candidates {
        let re = Regex::new(&format!(r"\b{}\b", regex::escape(&g.name))).expect("valid name");
        let names_trait = g.seam.as_ref().is_some_and(|seam| {
            Regex::new(&format!(r"\b{}\b", regex::escape(seam)))
                .expect("valid trait name")
                .is_match(code)
        });
        for m in re.find_iter(code) {
            let before = code[..m.start()].trim_end();
            let after = code[m.end()..].trim_start();
            if let Some(seam) = &g.seam {
                let through_dot = before.ends_with('.') && !before.ends_with("..");
                let through_trait = before
                    .strip_suffix("::")
                    .map(|path| path.trim_end().trim_end_matches('>').trim_end())
                    .is_some_and(|path| {
                        path.ends_with(seam.as_str())
                            && !path[..path.len() - seam.len()]
                                .bytes()
                                .last()
                                .is_some_and(is_ident)
                    });
                if names_trait && after.starts_with('(') && (through_dot || through_trait) {
                    out.push((g.name.clone(), line_at(&masked.starts, m.start())));
                }
                continue;
            }
            let definition = before.ends_with("fn")
                && !before[..before.len() - 2]
                    .bytes()
                    .last()
                    .is_some_and(is_ident);
            let field = after.starts_with(':') && !after.starts_with("::");
            let through_dot = before.ends_with('.') && !before.ends_with("..");
            if definition || field || (!g.method && through_dot) {
                continue;
            }
            out.push((g.name.clone(), line_at(&masked.starts, m.start())));
        }
    }
    out
}

/// Check every file's references against the allowlists, and the
/// allowlists against the references.
fn check_references(
    found: &BTreeMap<String, Vec<(String, usize)>>,
    allowed: &[&Allowed],
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (path, refs) in found {
        for (name, lineno) in refs {
            let ok = allowed
                .iter()
                .any(|a| a.path == path && a.names.contains(&name.as_str()));
            if !ok {
                findings.push(Finding::new(
                    path,
                    *lineno,
                    "bypass",
                    format!(
                        "`{name}` reaches the screen or the recogniser directly; take the \
frame and recogniser seams from game_ui instead"
                    ),
                ));
            }
        }
    }
    for entry in allowed {
        for name in entry.names {
            let used = found
                .get(entry.path)
                .is_some_and(|refs| refs.iter().any(|(n, _)| n == name));
            if !used {
                findings.push(Finding::new(
                    entry.path,
                    0,
                    "stale-allowance",
                    format!(
                        "allowlisted for `{name}` ({}) but no longer names it; remove the entry",
                        entry.reason
                    ),
                ));
            }
        }
    }
    findings
}

// ---------------------------------------------------------------------------
// Rule 2: layout literals.
// ---------------------------------------------------------------------------

/// What the literal scan of one reader found: violations, and the
/// exemptions that excused a literal, `(line, reason)`.
#[derive(Debug, Default)]
pub struct LiteralScan {
    pub findings: Vec<Finding>,
    pub exemptions: Vec<(usize, String)>,
}

/// A numeric literal starting at `i` of `code`: its end, and whether the
/// guard allows it whatever line it is on.
fn literal_at(code: &[u8], i: usize) -> (usize, bool) {
    let digits_from = |mut j: usize, radix: u32| {
        while j < code.len() && (code[j] == b'_' || (code[j] as char).is_digit(radix)) {
            j += 1;
        }
        j
    };
    let suffix_from = |mut j: usize| {
        while j < code.len() && is_ident(code[j]) {
            j += 1;
        }
        j
    };
    // A tuple field: `pair.1`, `nested.0.1`.
    if i > 0 && code[i - 1] == b'.' && !(i > 1 && code[i - 2] == b'.') {
        return (digits_from(i, 10), true);
    }
    let radix = match (code[i], code.get(i + 1).copied()) {
        (b'0', Some(b'x')) => Some(16),
        (b'0', Some(b'o')) => Some(8),
        (b'0', Some(b'b')) => Some(2),
        _ => None,
    };
    if let Some(radix) = radix {
        let end_digits = digits_from(i + 2, radix);
        let end = suffix_from(end_digits);
        if radix == 2 {
            return (end, true);
        }
        let digits: String = code[i + 2..end_digits]
            .iter()
            .filter(|c| **c != b'_')
            .map(|c| *c as char)
            .collect();
        let value = u128::from_str_radix(&digits, radix).ok();
        return (end, value.is_some_and(|v| ALLOWED_INTEGERS.contains(&v)));
    }
    let mut j = digits_from(i, 10);
    let mut float = false;
    if code.get(j) == Some(&b'.')
        && code
            .get(j + 1)
            .is_none_or(|c| *c != b'.' && !(c.is_ascii_alphabetic() || *c == b'_'))
    {
        float = true;
        j = digits_from(j + 1, 10);
    }
    if matches!(code.get(j).copied(), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(code.get(j + 1).copied(), Some(b'+' | b'-')));
        if code.get(j + 1 + sign).is_some_and(u8::is_ascii_digit) {
            float = true;
            j = digits_from(j + 1 + sign, 10);
        }
    }
    let end = suffix_from(j);
    if code[j..end].starts_with(b"f") {
        float = true;
    }
    let number: String = code[i..j]
        .iter()
        .filter(|c| **c != b'_')
        .map(|c| *c as char)
        .collect();
    let allowed = if float {
        number
            .parse::<f64>()
            .is_ok_and(|v| ALLOWED_FLOATS.contains(&v))
    } else {
        number
            .parse::<u128>()
            .is_ok_and(|v| ALLOWED_INTEGERS.contains(&v))
    };
    // An array's length: `[T; N]` or `[value; N]`.
    let array_length = code[..i].trim_ascii_end().ends_with(b";")
        && code[end..].trim_ascii_start().starts_with(b"]");
    (end, allowed || array_length)
}

/// Apply the layout-literal rule to one reader's text.
pub fn scan_literals(path: &str, text: &str) -> LiteralScan {
    let posix = path.replace('\\', "/");
    let masked = mask(text);
    let code = masked.code.as_bytes();
    let blocks = blocks(&masked.code);
    let mut suspects: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut i = 0;
    while i < code.len() {
        if !code[i].is_ascii_digit() || (i > 0 && is_ident(code[i - 1])) {
            i += 1;
            continue;
        }
        let (end, allowed) = literal_at(code, i);
        if !allowed && !in_test(&blocks, i) {
            suspects
                .entry(line_at(&masked.starts, i))
                .or_default()
                .push(masked.code[i..end].to_string());
        }
        i = end.max(i + 1);
    }
    let mut scan = LiteralScan::default();
    for (lineno, comment) in &masked.line_comments {
        let Some(reason) = comment.strip_prefix(EXEMPTION) else {
            continue;
        };
        if in_test(&blocks, masked.starts[lineno - 1]) {
            continue;
        }
        if reason.trim().is_empty() {
            scan.findings.push(Finding::new(
                &posix,
                *lineno,
                "exemption-without-reason",
                format!("`{EXEMPTION}` must say why the number on its line is not layout"),
            ));
        } else if suspects.remove(lineno).is_some() {
            scan.exemptions.push((*lineno, reason.trim().to_string()));
        } else {
            scan.findings.push(Finding::new(
                &posix,
                *lineno,
                "idle-exemption",
                format!("`{EXEMPTION}` on a line with no literal for it to excuse; remove it"),
            ));
        }
    }
    for (lineno, literals) in suspects {
        scan.findings.push(Finding::new(
            &posix,
            lineno,
            "layout-literal",
            format!(
                "{} in a reader; a measurement of the client's interface belongs in the \
interface profile (a number that is not layout says why on a trailing `{EXEMPTION}` comment)",
                literals.join(", ")
            ),
        ));
    }
    scan
}

fn test_module_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"#\[cfg\(([^\]]*)\)\]\s*(#\[[^\]]*\]\s*)*(pub(\s*\([^)]*\))?\s+)?mod\s+(\w+)\s*;",
        )
        .expect("valid test module pattern")
    })
}

fn test_word_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\btest\b").expect("valid test word pattern"))
}

/// The directory prefixes of the modules `path` declares only for tests
/// (`#[cfg(test)] mod tests;`, `#[cfg(any(test, feature = ..))] mod x;`):
/// each covers `<dir>/<name>.rs` and everything under `<dir>/<name>/`.
pub fn test_only_children(path: &str, text: &str) -> Vec<String> {
    let posix = path.replace('\\', "/");
    let dir = match posix.rsplit_once('/') {
        Some((parent, file)) if file == "mod.rs" || file == "lib.rs" || file == "main.rs" => {
            parent.to_string()
        }
        Some((parent, file)) => format!("{parent}/{}", file.trim_end_matches(".rs")),
        None => String::new(),
    };
    let masked = mask(text);
    test_module_re()
        .captures_iter(&masked.code)
        .filter(|caps| test_word_re().is_match(&caps[1]))
        .map(|caps| format!("{dir}/{}", &caps[5]))
        .collect()
}

fn under_test_module(path: &str, test_modules: &[String]) -> bool {
    test_modules
        .iter()
        .any(|m| path == format!("{m}.rs") || path.starts_with(&format!("{m}/")))
}

// ---------------------------------------------------------------------------
// Rule 3: profile structs reject unknown fields.
// ---------------------------------------------------------------------------

fn derive_deserialize_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"#\[derive\([^\]]*\bDeserialize\b[^\]]*\)\]").expect("valid derive pattern")
    })
}

/// The attributes written directly above `at`.
fn leading_attributes(code: &str, at: usize) -> &str {
    let mut start = at;
    loop {
        let before = code[..start].trim_end();
        if !before.ends_with(']') {
            return &code[start..at];
        }
        match before.rfind("#[") {
            Some(open) => start = open,
            None => return &code[start..at],
        }
    }
}

fn trailing_attributes_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(\s*#\[[^\]]*\])*\s*(pub(\s*\([^)]*\))?\s+)?(struct|enum|union)\s+(\w+)")
            .expect("valid item pattern")
    })
}

/// Apply the unknown-fields rule to one reader's text.
pub fn scan_unknown_fields(path: &str, text: &str) -> Vec<Finding> {
    let posix = path.replace('\\', "/");
    let masked = mask(text);
    let code = &masked.code;
    let blocks = blocks(code);
    let mut findings = Vec::new();
    for derive in derive_deserialize_re().find_iter(code) {
        if in_test(&blocks, derive.start()) {
            continue;
        }
        let Some(item) = trailing_attributes_re().captures(&code[derive.end()..]) else {
            continue;
        };
        if &item[4] != "struct" {
            continue;
        }
        let attributes = format!("{}{}", leading_attributes(code, derive.start()), &item[0]);
        if !attributes.contains("deny_unknown_fields") {
            findings.push(Finding::new(
                &posix,
                line_at(&masked.starts, derive.start()),
                "unknown-fields-accepted",
                format!(
                    "`{}` deserialises profile data without `#[serde(deny_unknown_fields)]`; \
a stale or misspelt profile entry would be ignored",
                    &item[5]
                ),
            ));
        }
    }
    findings
}

// ---------------------------------------------------------------------------
// Rule 4: the fixtures feature.
// ---------------------------------------------------------------------------

fn fixture_mention_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"(^|[^\w-]){}([^\w-]|$)",
            regex::escape(FIXTURE_FEATURE)
        ))
        .expect("valid feature pattern")
    })
}

fn feature_definition_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(r#"^"?{}"?\s*="#, regex::escape(FIXTURE_FEATURE)))
            .expect("valid definition pattern")
    })
}

/// A manifest line with its `#` comment removed (a `#` inside a string is
/// kept).
fn strip_toml_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '#') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// The dotted segments of a table header, quotes removed.
fn header_segments(header: &str) -> Vec<String> {
    let mut segments = vec![String::new()];
    let mut quote: Option<char> = None;
    for c in header.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '.') => segments.push(String::new()),
            _ => segments.last_mut().expect("one segment").push(c),
        }
    }
    segments.into_iter().map(|s| s.trim().to_string()).collect()
}

/// Apply the fixtures-feature rule to one Cargo manifest.
pub fn scan_manifest(path: &str, text: &str) -> Vec<Finding> {
    let posix = path.replace('\\', "/");
    let mut findings = Vec::new();
    let mut table: Vec<String> = Vec::new();
    let mut package: Option<String> = None;
    for (idx, raw) in text.lines().enumerate() {
        let line = strip_toml_comment(raw).trim();
        if let Some(header) = line.strip_prefix('[') {
            let header = header.trim_start_matches('[');
            let header = header.split(']').next().unwrap_or("");
            table = header_segments(header);
            continue;
        }
        if table == ["package"] {
            if let Some(name) = line
                .strip_prefix("name")
                .map(str::trim_start)
                .and_then(|rest| rest.strip_prefix('='))
            {
                package = Some(name.trim().trim_matches('"').to_string());
            }
        }
        if !fixture_mention_re().is_match(line) {
            continue;
        }
        let dev_table = table.iter().any(|s| s == "dev-dependencies");
        let own_definition = package.as_deref() == Some(FIXTURE_CRATE)
            && table == ["features"]
            && feature_definition_re().is_match(line);
        if !(dev_table || own_definition) {
            findings.push(Finding::new(
                &posix,
                idx + 1,
                "fixtures-outside-dev",
                format!(
                    "`{FIXTURE_CRATE}`'s `{FIXTURE_FEATURE}` feature is enabled from `[{}]`; it \
exposes test fixtures and may be enabled only from `[dev-dependencies]`, never by default",
                    table.join(".")
                ),
            ));
        }
    }
    findings
}

// ---------------------------------------------------------------------------
// The whole tree.
// ---------------------------------------------------------------------------

/// What the guard found over the tree.
#[derive(Debug, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub gated: Vec<Gated>,
    pub exemptions: Vec<(String, usize, String)>,
}

fn read(repo_root: &std::path::Path, path: &str) -> Result<Option<String>, String> {
    match std::fs::read_to_string(repo_root.join(path)) {
        Ok(text) => Ok(Some(text)),
        // A tracked file deleted from the working tree carries nothing to
        // scan.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        // Any other read failure must fail loudly: a guard that skips an
        // unreadable source can return a false clean.
        Err(e) => Err(format!("ui-profile: cannot read {path}: {e}")),
    }
}

fn evaluate(repo_root: &std::path::Path) -> Result<Report, String> {
    let tracked = git::run(&["ls-files", "--", "app/src-tauri"], repo_root)?;
    let tracked: Vec<&str> = tracked.lines().collect();
    let mut report = Report::default();

    // Rule 1.
    let mut modules = BTreeMap::new();
    for module in CAPTURE_MODULES.iter().chain(RECOGNISER_MODULES) {
        let text = read(repo_root, module)?
            .ok_or_else(|| format!("ui-profile: {module} is missing; update the guard"))?;
        modules.insert(module.to_string(), text);
    }
    let (mut gated, stale) = gated_functions(&modules);
    report.findings.extend(stale);
    let (seam_module, seam_trait) = SEAM_TRAIT;
    let seam_text = read(repo_root, seam_module)?
        .ok_or_else(|| format!("ui-profile: {seam_module} is missing; update the guard"))?;
    let seam = seam_methods(seam_module, &seam_text, seam_trait);
    if seam.is_empty() {
        return Err(format!(
            "ui-profile: {seam_module} declares no method of a trait `{seam_trait}`; update the guard"
        ));
    }
    gated.extend(seam);
    gated.sort();
    gated.dedup();
    let mut found: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    for path in tracked.iter().filter(|p| p.ends_with(".rs")) {
        if let Some(text) = read(repo_root, path)? {
            let refs = references(path, &text, &gated);
            if !refs.is_empty() {
                found.insert(path.to_string(), refs);
            }
        }
    }
    let allowed: Vec<&Allowed> = ALLOWED.iter().chain(TO_MIGRATE).collect();
    report.findings.extend(check_references(&found, &allowed));
    report.gated = gated;

    // Rules 2 and 3.
    let readers: Vec<&str> = tracked
        .iter()
        .copied()
        .filter(|p| p.starts_with(READERS_ROOT) && p.ends_with(".rs"))
        .collect();
    let mut texts = BTreeMap::new();
    for path in &readers {
        if let Some(text) = read(repo_root, path)? {
            texts.insert(path.to_string(), text);
        }
    }
    let test_modules: Vec<String> = texts
        .iter()
        .flat_map(|(path, text)| test_only_children(path, text))
        .collect();
    for (path, text) in &texts {
        if under_test_module(path, &test_modules) {
            continue;
        }
        let scan = scan_literals(path, text);
        report.findings.extend(scan.findings);
        report.exemptions.extend(
            scan.exemptions
                .into_iter()
                .map(|(lineno, reason)| (path.clone(), lineno, reason)),
        );
        report.findings.extend(scan_unknown_fields(path, text));
    }

    // Rule 4.
    for path in tracked.iter().filter(|p| p.ends_with("Cargo.toml")) {
        if let Some(text) = read(repo_root, path)? {
            report.findings.extend(scan_manifest(path, &text));
        }
    }
    Ok(report)
}

pub fn run(args: &[String]) -> Result<i32, String> {
    let warn_only = args.iter().any(|a| a == "--warn-only");
    let repo_root = git::repo_root()?;
    let report = evaluate(&repo_root)?;

    let names: BTreeSet<&str> = report.gated.iter().map(|g| g.name.as_str()).collect();
    let migrate: usize = TO_MIGRATE.iter().map(|a| a.names.len()).sum();
    println!(
        "ui-profile: {} gated capture and recogniser functions ({}); {} allowlisted files, {} \
names still to migrate; {} `{EXEMPTION}` exemptions in the readers.",
        names.len(),
        names.into_iter().collect::<Vec<_>>().join(", "),
        ALLOWED.len(),
        migrate,
        report.exemptions.len(),
    );

    if report.findings.is_empty() {
        println!(
            "ui-profile: every reader keeps to the pipeline and keeps its layout in the profile."
        );
        return Ok(0);
    }

    eprintln!(
        "ui-profile: the game-interface readers have left their pipeline or their profile.\n\n\
Screen pixels reach the recogniser only through game_ui's seams, the readers' layout \
numbers live in the interface profile, the profile rejects unknown entries, and the \
readers' test fixtures never ship. Offenders:\n"
    );
    for f in &report.findings {
        eprintln!("  {}:{}: [{}] {}", f.path, f.lineno, f.rule, f.detail);
    }

    if warn_only {
        eprintln!("\nui-profile: --warn-only set; exiting 0 despite the findings above.");
        return Ok(0);
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURE: &str = "app/src-tauri/eo-services/src/screen_capture/mod.rs";

    fn rules(findings: &[Finding]) -> Vec<&'static str> {
        findings.iter().map(|f| f.rule).collect()
    }

    fn capture_module() -> BTreeMap<String, String> {
        BTreeMap::from([(
            CAPTURE.to_string(),
            "pub fn capture_region_bgr(x: i64) -> Option<BgrImage> { platform::grab(x) }\n\
             pub fn frame_to_png(f: &BgrImage) -> Option<Vec<u8>> { None }\n\
             pub fn write_debug_frame() {}\n\
             mod platform { pub fn grab(x: i64) -> Option<BgrImage> { None } }\n"
                .to_string(),
        )])
    }

    fn gated_names(modules: &BTreeMap<String, String>) -> Vec<String> {
        gated_functions(modules)
            .0
            .into_iter()
            .map(|g| g.name)
            .collect()
    }

    // Rule 1 ---------------------------------------------------------------

    #[test]
    fn ui_profile_a_capture_call_outside_the_allowlist_is_a_bypass() {
        let gated = gated_functions(&capture_module()).0;
        let text = "fn read() {\n    let shot = crate::screen_capture::capture_region_bgr(0, 0, 9, 9);\n}\n";
        let refs = references("app/src-tauri/eo-services/src/rogue.rs", text, &gated);
        assert_eq!(refs, [("capture_region_bgr".to_string(), 2)]);
        let found = BTreeMap::from([("app/src-tauri/eo-services/src/rogue.rs".to_string(), refs)]);
        let findings = check_references(&found, &[]);
        assert_eq!(rules(&findings), ["bypass"]);
        assert_eq!(findings[0].lineno, 2);
    }

    #[test]
    fn ui_profile_an_allowlisted_call_passes_and_an_unused_allowance_is_stale() {
        let used = Allowed {
            path: "app/src-tauri/a.rs",
            names: &["capture_region_bgr"],
            reason: "test",
        };
        let unused = Allowed {
            path: "app/src-tauri/b.rs",
            names: &["capture_region_bgr"],
            reason: "test",
        };
        let found = BTreeMap::from([(
            "app/src-tauri/a.rs".to_string(),
            vec![("capture_region_bgr".to_string(), 3)],
        )]);
        let findings = check_references(&found, &[&used, &unused]);
        assert_eq!(rules(&findings), ["stale-allowance"]);
        assert_eq!(findings[0].path, "app/src-tauri/b.rs");
    }

    #[test]
    fn ui_profile_an_allowance_covers_only_the_names_it_lists() {
        let entry = Allowed {
            path: "app/src-tauri/a.rs",
            names: &["start_capture"],
            reason: "test",
        };
        let found = BTreeMap::from([(
            "app/src-tauri/a.rs".to_string(),
            vec![
                ("start_capture".to_string(), 1),
                ("capture_region_bgr".to_string(), 2),
            ],
        )]);
        let findings = check_references(&found, &[&entry]);
        assert_eq!(rules(&findings), ["bypass"]);
        assert_eq!(findings[0].lineno, 2);
    }

    #[test]
    fn ui_profile_a_new_public_capture_function_is_gated_unasked() {
        let mut modules = capture_module();
        modules
            .get_mut(CAPTURE)
            .unwrap()
            .push_str("pub fn capture_everything() -> Vec<u8> { vec![] }\n");
        let names = gated_names(&modules);
        assert_eq!(names, ["capture_everything", "capture_region_bgr"]);
        let gated = gated_functions(&modules).0;
        let refs = references(
            "app/src-tauri/eo-services/src/rogue.rs",
            "let all = screen_capture::capture_everything();",
            &gated,
        );
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn ui_profile_helpers_private_modules_and_tests_are_not_gated() {
        let mut modules = capture_module();
        modules.get_mut(CAPTURE).unwrap().push_str(
            "#[cfg(test)]\nmod tests { pub fn capture_fixture() {} }\n\
             fn private_grab() {}\n",
        );
        assert_eq!(gated_names(&modules), ["capture_region_bgr"]);
    }

    #[test]
    fn ui_profile_a_helper_entry_for_a_function_that_is_gone_is_stale() {
        let modules = BTreeMap::from([(
            CAPTURE.to_string(),
            "pub fn capture_region_bgr() {}\npub fn frame_to_png() {}\n".to_string(),
        )]);
        let findings = gated_functions(&modules).1;
        assert_eq!(rules(&findings), ["stale-helper"]);
        assert!(findings[0].detail.contains("write_debug_frame"));
    }

    #[test]
    fn ui_profile_methods_are_gated_and_reached_through_a_dot() {
        let modules = BTreeMap::from([(
            "app/src-tauri/eo-services/src/ocr_engine.rs".to_string(),
            "pub struct OcrEngine;\nimpl OcrEngine {\n    pub fn recognize_bgr(&self) {}\n}\n"
                .to_string(),
        )]);
        let gated = gated_functions(&modules).0;
        assert_eq!(
            gated,
            [Gated {
                module: "app/src-tauri/eo-services/src/ocr_engine.rs".to_string(),
                name: "recognize_bgr".to_string(),
                method: true,
                seam: None,
            }]
        );
        let refs = references("app/src-tauri/x.rs", "engine.recognize_bgr(&img)", &gated);
        assert_eq!(refs.len(), 1);
    }

    const FIELD: &str = "app/src-tauri/eo-services/src/game_ui/field.rs";

    /// The recogniser trait as the readers declare it, gated.
    fn seam() -> Vec<Gated> {
        seam_methods(
            FIELD,
            "/// The recognition seam.\n\
             pub trait Recogniser: Send + Sync {\n    \
                 fn read(&self, image: &BgrImage) -> Option<(String, f64)>;\n\
             }\n\
             pub fn read_run(recogniser: &dyn Recogniser) { recogniser.read(&crop()); }\n",
            "Recogniser",
        )
    }

    #[test]
    fn ui_profile_the_recogniser_traits_methods_are_read_from_its_definition() {
        assert_eq!(
            seam(),
            [Gated {
                module: FIELD.to_string(),
                name: "read".to_string(),
                method: true,
                seam: Some("Recogniser".to_string()),
            }]
        );
        // Every method the trait declares, with or without a default
        // body, and none that a body defines for itself or that another
        // trait or a free function declares.
        let text = "trait Other { fn glance(&self); }\n\
                    pub trait Recogniser {\n    \
                        fn read(&self, image: &BgrImage) -> Option<(String, f64)>;\n    \
                        fn read_all(&self, images: &[BgrImage]) -> usize {\n        \
                            fn count(n: usize) -> usize { n }\n        \
                            count(images.len())\n    \
                        }\n    \
                        fn warm(&self);\n\
                    }\n\
                    fn after() {}\n";
        assert_eq!(
            trait_methods(text, "Recogniser"),
            Some(vec![
                "read".to_string(),
                "read_all".to_string(),
                "warm".to_string()
            ])
        );
        assert_eq!(
            trait_methods("pub trait Recogniserish { fn read(&self); }", "Recogniser"),
            None
        );
        assert_eq!(trait_methods("fn read() {}", "Recogniser"), None);
    }

    #[test]
    fn ui_profile_a_call_to_the_recogniser_trait_outside_the_readers_is_a_bypass() {
        let gated = seam();
        let rogue = "app/src-tauri/eo-services/src/rogue.rs";
        // On an engine, through a dot; on a trait object; by the trait's
        // path, plainly and qualified.
        for (text, line) in [
            ("use crate::game_ui::field::Recogniser;\nfn peek(engine: &OcrEngine, crop: &BgrImage) {\n    let text = engine.read(crop);\n}\n", 3),
            ("fn peek(seam: &dyn Recogniser, crop: &BgrImage) {\n    let text = seam\n        .read(crop);\n}\n", 3),
            ("use crate::game_ui::field::Recogniser;\nfn peek(engine: &OcrEngine, crop: &BgrImage) {\n    Recogniser::read(engine, crop);\n}\n", 3),
            ("fn peek(engine: &OcrEngine, crop: &BgrImage) {\n    <OcrEngine as crate::game_ui::field::Recogniser>::read(engine, crop);\n}\n", 2),
        ] {
            let refs = references(rogue, text, &gated);
            assert_eq!(refs, [("read".to_string(), line)], "{text}");
            let found = BTreeMap::from([(rogue.to_string(), refs)]);
            assert_eq!(rules(&check_references(&found, &[])), ["bypass"]);
            // Listed for the method with its reason, the file passes; an
            // entry for another name does not cover it.
            let listed = Allowed {
                path: "app/src-tauri/eo-services/src/rogue.rs",
                names: &["read"],
                reason: "test",
            };
            assert!(check_references(&found, &[&listed]).is_empty());
            let other = Allowed {
                path: "app/src-tauri/eo-services/src/rogue.rs",
                names: &["recognize_bgr"],
                reason: "test",
            };
            assert_eq!(
                rules(&check_references(&found, &[&other])),
                ["bypass", "stale-allowance"]
            );
        }
    }

    #[test]
    fn ui_profile_the_same_call_inside_the_readers_is_the_pipeline() {
        let gated = seam();
        let text = "use super::field::Recogniser;\nfn level(recogniser: &dyn Recogniser, crop: &BgrImage) {\n    let text = recogniser.read(crop);\n}\n";
        for reader in [
            "app/src-tauri/eo-services/src/game_ui/plates.rs",
            "app/src-tauri/eo-services/src/game_ui/seams.rs",
            FIELD,
        ] {
            assert!(references(reader, text, &gated).is_empty(), "{reader}");
        }
        // A module beside the readers' directory is not in it.
        assert_eq!(
            references(
                "app/src-tauri/eo-services/src/game_ui_extra.rs",
                text,
                &gated
            )
            .len(),
            1
        );
    }

    #[test]
    fn ui_profile_implementing_the_recogniser_trait_is_a_definition_not_a_call() {
        let gated = seam();
        // The engine's implementation, and a stand-in a test defines.
        let engine = "impl crate::game_ui::field::Recogniser for OcrEngine {\n    fn read(&self, image: &BgrImage) -> Option<(String, f64)> {\n        self.recognize(image).ok()\n    }\n}\n";
        let stand_in = "#[cfg(test)]\nmod tests {\n    use crate::game_ui::field::Recogniser;\n    struct Blind;\n    impl Recogniser for Blind {\n        fn read(&self, _: &BgrImage) -> Option<(String, f64)> { None }\n    }\n}\n";
        for text in [engine, stand_in] {
            assert!(
                references("app/src-tauri/eo-services/src/ocr_engine.rs", text, &gated).is_empty(),
                "{text}"
            );
        }
    }

    #[test]
    fn ui_profile_another_read_is_not_the_recogniser_traits() {
        let gated = seam();
        // Reading a file or a lock, another type's own `read`, a field and
        // a closure of that name, in a file that names the trait; and a
        // call through a dot in a file that does not name it.
        let text = "use crate::game_ui::field::Recogniser;\n\
                    fn load(path: &Path, seam: std::sync::Arc<dyn Recogniser>) {\n    \
                        let bytes = std::fs::read(path);\n    \
                        let text = fs::read_to_string(path);\n    \
                        let providers = Providers { read: seam, read_text: None };\n    \
                        let held = providers.read;\n    \
                        let read = |crop: &BgrImage| None::<String>;\n    \
                        read(&crop());\n\
                    }\n";
        assert!(references("app/src-tauri/x.rs", text, &gated).is_empty());
        let unnamed = "fn tail(lock: &RwLock<State>, file: &mut File, buffer: &mut [u8]) {\n    let state = lock.read();\n    file.read(buffer);\n}\n";
        assert!(references("app/src-tauri/x.rs", unnamed, &gated).is_empty());
    }

    #[test]
    fn ui_profile_the_live_recogniser_trait_declares_the_method_the_guard_gates() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let (module, name) = SEAM_TRAIT;
        let text = std::fs::read_to_string(root.join(module)).expect("the trait's module reads");
        let gated = seam_methods(module, &text, name);
        assert!(!gated.is_empty(), "{module} declares no `{name}` method");
        assert!(module.starts_with(READERS_ROOT));
        assert!(gated
            .iter()
            .all(|g| g.seam.as_deref() == Some(name) && g.method));
    }

    #[test]
    fn ui_profile_fields_definitions_comments_and_strings_are_not_references() {
        let mut modules = capture_module();
        modules
            .get_mut(CAPTURE)
            .unwrap()
            .push_str("pub fn start_capture() -> bool { true }\n");
        let gated = gated_functions(&modules).0;
        let text = "\
pub struct Probes { pub start_capture: Probe }
fn wire() -> Probes { Probes { start_capture: Arc::new(|| true) } }
fn poll(p: &Probes) -> bool { (p.start_capture)() }
// capture_region_bgr is reached through the frame seam
fn start_capture() -> bool { false }
const NOTE: &str = \"capture_region_bgr\";
";
        assert!(references("app/src-tauri/x.rs", text, &gated).is_empty());
    }

    #[test]
    fn ui_profile_a_module_may_name_what_it_defines() {
        let modules = capture_module();
        let gated = gated_functions(&modules).0;
        let text = &modules[CAPTURE];
        assert!(references(CAPTURE, text, &gated).is_empty());
    }

    #[test]
    fn ui_profile_every_public_function_of_the_live_modules_is_classified() {
        // Read the real modules: a public function added there is gated, and
        // a helper entry naming one that is gone is stale.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let modules: BTreeMap<String, String> = CAPTURE_MODULES
            .iter()
            .chain(RECOGNISER_MODULES)
            .map(|m| {
                let text = std::fs::read_to_string(root.join(m)).expect("module reads");
                (m.to_string(), text)
            })
            .collect();
        let (gated, stale) = gated_functions(&modules);
        assert!(stale.is_empty(), "{stale:?}");
        let names: BTreeSet<&str> = gated.iter().map(|g| g.name.as_str()).collect();
        for expected in [
            "capture_region_png",
            "capture_region_bgr",
            "capture_region_bgr_running",
            "start_capture",
            "capture_running",
            "capturable_area",
            "capturable_area_running",
            "capture_game_window",
            "capture_game_window_running",
            "recognize_bgr",
            "recognize_png",
        ] {
            assert!(
                names.contains(expected),
                "{expected} is not gated: {names:?}"
            );
        }
        for (module, name, _) in NOT_GATED {
            assert!(
                !gated.iter().any(|g| &g.module == module && &g.name == name),
                "{name} is listed as a helper but gated"
            );
        }
    }

    // Rule 2 ---------------------------------------------------------------

    const READER: &str = "app/src-tauri/eo-services/src/game_ui/reader.rs";

    #[test]
    fn ui_profile_a_layout_literal_in_a_reader_is_flagged() {
        let scan = scan_literals(READER, "fn ok(x: i32) -> bool {\n    x > 14\n}\n");
        assert_eq!(rules(&scan.findings), ["layout-literal"]);
        assert_eq!(scan.findings[0].lineno, 2);
        assert!(scan.findings[0].detail.contains("14"));
        for line in [
            "let share = 0.97;",
            "let c = 0xfee0;",
            "let t = 1e-9;",
            "let n = 4usize;",
            "let f = 2.5f32;",
            "let r = 1.5;",
            "let k = 1_000;",
            "for i in 0..4 {}",
            "let x = 7.;",
        ] {
            let scan = scan_literals(READER, line);
            assert_eq!(rules(&scan.findings), ["layout-literal"], "{line}");
        }
    }

    #[test]
    fn ui_profile_small_numbers_and_structural_contexts_pass() {
        let text = "\
const FLAGS: u8 = 0b0100;
type Counts = [u16; 37];
fn f(pair: (i32, i32), a: &[f32; 4]) -> f32 {
    let z = [0u8; 12];
    let v = pair.1 + (pair.0).max(3) + nested.0.1;
    let w = 0.5 * a[0] + 2.0 - 1.0 + 0.0;
    let s = &v[1..3];
    x.max(0) as f32 + 1u8 as f32 + 2_i32 as f32
}
";
        let scan = scan_literals(READER, text);
        assert!(scan.findings.is_empty(), "{:?}", scan.findings);
    }

    #[test]
    fn ui_profile_numbers_in_comments_strings_and_tests_are_not_code() {
        let text = "\
//! The client draws its ridge 2 to 3 pixels wide, 40 px from the corner.
/// Measured at 1440p.
fn label() -> &'static str {
    let c = '9';
    let r = r#\"width 640\"#;
    \"width 640 at 97%\" /* a 12 px gap */
}
#[cfg(test)]
mod tests {
    #[test]
    fn draws() { let w = 640; assert_eq!(w * 3, 1920); }
}
#[cfg(test)]
const SAMPLE: u32 = 640;
";
        let scan = scan_literals(READER, text);
        assert!(scan.findings.is_empty(), "{:?}", scan.findings);
    }

    #[test]
    fn ui_profile_an_exemption_with_a_reason_passes_and_is_counted() {
        let text = "const TURN: f64 = 360.0; // not layout: the degrees of a full turn\n";
        let scan = scan_literals(READER, text);
        assert!(scan.findings.is_empty(), "{:?}", scan.findings);
        assert_eq!(
            scan.exemptions,
            [(1, "the degrees of a full turn".to_string())]
        );
    }

    #[test]
    fn ui_profile_an_exemption_without_a_reason_is_flagged() {
        for text in [
            "const TURN: f64 = 360.0; // not layout:\n",
            "const TURN: f64 = 360.0; // not layout:   \n",
        ] {
            let scan = scan_literals(READER, text);
            assert_eq!(
                rules(&scan.findings),
                ["exemption-without-reason", "layout-literal"]
            );
            assert!(scan.exemptions.is_empty());
        }
    }

    #[test]
    fn ui_profile_an_exemption_with_nothing_to_excuse_is_flagged() {
        let scan = scan_literals(READER, "let x = y + 1; // not layout: a stray note\n");
        assert_eq!(rules(&scan.findings), ["idle-exemption"]);
    }

    #[test]
    fn ui_profile_an_exemption_excuses_only_its_own_line() {
        let text = "const A: f64 = 360.0; // not layout: a full turn\nconst B: f64 = 97.0;\n";
        let scan = scan_literals(READER, text);
        assert_eq!(rules(&scan.findings), ["layout-literal"]);
        assert_eq!(scan.findings[0].lineno, 2);
    }

    #[test]
    fn ui_profile_modules_declared_for_tests_are_test_code() {
        let children = test_only_children(
            "app/src-tauri/eo-services/src/game_ui/mod.rs",
            "pub mod window;\n#[cfg(any(test, feature = \"test-support\"))]\n#[doc(hidden)]\npub mod test_support;\n",
        );
        assert_eq!(
            children,
            ["app/src-tauri/eo-services/src/game_ui/test_support"]
        );
        assert!(under_test_module(
            "app/src-tauri/eo-services/src/game_ui/test_support.rs",
            &children
        ));
        assert!(!under_test_module(
            "app/src-tauri/eo-services/src/game_ui/window.rs",
            &children
        ));
        let nested = test_only_children(
            "app/src-tauri/eo-services/src/game_ui/radar.rs",
            "#[cfg(test)]\nmod tests;\n",
        );
        assert_eq!(
            nested,
            ["app/src-tauri/eo-services/src/game_ui/radar/tests"]
        );
    }

    // Rule 3 ---------------------------------------------------------------

    #[test]
    fn ui_profile_a_profile_struct_accepting_unknown_fields_is_flagged() {
        let text = "\
/// The ridge.
#[derive(Debug, Clone, Deserialize)]
pub struct RidgeProfile {
    pub width: u8,
}
";
        let findings = scan_unknown_fields(READER, text);
        assert_eq!(rules(&findings), ["unknown-fields-accepted"]);
        assert!(findings[0].detail.contains("RidgeProfile"));
    }

    #[test]
    fn ui_profile_a_profile_struct_rejecting_unknown_fields_passes() {
        for text in [
            "#[derive(Debug, Deserialize)]\n#[serde(deny_unknown_fields)]\npub struct A { pub x: u8 }\n",
            "#[serde(deny_unknown_fields)]\n#[derive(Debug, Deserialize)]\nstruct B { x: u8 }\n",
            "#[derive(Debug, Deserialize)]\n#[serde(rename_all = \"snake_case\")]\npub enum Side { Left }\n",
            "#[derive(Debug, Clone)]\npub struct NotProfile { pub x: u8 }\n",
        ] {
            assert!(scan_unknown_fields(READER, text).is_empty(), "{text}");
        }
    }

    // Rule 4 ---------------------------------------------------------------

    const OWN: &str = "\
[package]
name = \"eo-services\"

[features]
# Exposes the readers' test fixtures; see test-support below.
linux-capture = [\"dep:ashpd\"]
test-support = []
";

    #[test]
    fn ui_profile_the_fixtures_feature_from_dev_dependencies_passes() {
        assert!(scan_manifest("app/src-tauri/eo-services/Cargo.toml", OWN).is_empty());
        let consumer = "\
[package]
name = \"eo-api\"

[dependencies]
eo-services = { workspace = true }

[dev-dependencies]
eo-services = { workspace = true, features = [\"test-support\"] }

[target.'cfg(windows)'.dev-dependencies.eo-services]
features = [\"test-support\"]
";
        assert!(scan_manifest("app/src-tauri/eo-api/Cargo.toml", consumer).is_empty());
    }

    #[test]
    fn ui_profile_the_fixtures_feature_outside_dev_dependencies_is_flagged() {
        for (manifest, line) in [
            (
                "[package]\nname = \"eo-api\"\n[dependencies]\neo-services = { workspace = true, features = [\"test-support\"] }\n",
                4,
            ),
            (
                "[workspace.dependencies]\neo-services = { path = \"eo-services\", features = [\"test-support\"] }\n",
                2,
            ),
            (
                "[package]\nname = \"eo-api\"\n[features]\nfixtures = [\"eo-services/test-support\"]\n",
                4,
            ),
            (
                "[package]\nname = \"eo-api\"\n[dependencies.eo-services]\nfeatures = [\"test-support\"]\n",
                4,
            ),
        ] {
            let findings = scan_manifest("app/src-tauri/x/Cargo.toml", manifest);
            assert_eq!(rules(&findings), ["fixtures-outside-dev"], "{manifest}");
            assert_eq!(findings[0].lineno, line, "{manifest}");
        }
    }

    #[test]
    fn ui_profile_the_fixtures_feature_on_by_default_is_flagged() {
        for default in [
            "default = [\"test-support\"]\n",
            "default = [\n    \"linux-capture\",\n    \"test-support\",\n]\n",
        ] {
            let manifest = format!("{OWN}{default}");
            let findings = scan_manifest("app/src-tauri/eo-services/Cargo.toml", &manifest);
            assert_eq!(rules(&findings), ["fixtures-outside-dev"], "{default}");
        }
    }

    // Lexing ---------------------------------------------------------------

    #[test]
    fn ui_profile_masking_keeps_offsets_and_reads_trailing_comments() {
        let text =
            "let a = \"x // y\"; // not layout: z\nlet b = 'q'; let c: &'static str = \"\";\n";
        let masked = mask(text);
        assert_eq!(masked.code.len(), text.len());
        assert_eq!(
            masked.line_comments.get(&1).map(String::as_str),
            Some("// not layout: z")
        );
        assert!(!masked.code.contains('q'));
        assert!(masked.code.contains("&'static str"));
    }
}
