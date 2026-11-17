//! The game's chat log read after the fact, over one span of time.
//!
//! The chat log only grows, and a long-played account's runs to
//! gigabytes, so a span is never found by reading from the top. Each line
//! opens with the time the game logged it, in the game server's own zone,
//! and those stamps resolve to instants through the shared
//! [`ChatLogClock`], exactly as the live tail's do. The file is bisected on
//! byte offsets by those stamps to the first line the span reaches, and
//! only the span itself is then read, line by line. Memory stays bounded
//! by one line, and every byte read counts against a budget: a span that
//! would take more than [`SCAN_BUDGET_BYTES`] to check is reported as not
//! checked, never as clean.
//!
//! A span is clean only when the log covers it (its first stamped line is
//! at or before the span's start, and a stamped line comes after the
//! span's end) and none of its lines inside the span is of a kind asked
//! about. A log that starts after the span begins misses it for good; one
//! with no line after the span's end yet has only not reached it, and may
//! once the game writes on. A line is stamped to the whole second, so a
//! line counts as inside when any part of its second is. The clock's
//! offset must be known from the live tail, and known to have held since
//! the span began: a stamp resolved against a guessed zone, or against an
//! offset that moved since (a daylight-saving step), could move the span
//! by hours, so neither is checked.
//!
//! Blocking file reads: run on the blocking pool, never on a runtime
//! worker.

use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::NaiveDateTime;

use crate::chatlog_parser::{parse_line, EventType};
use crate::chatlog_time::{ChatLogClock, ChatLogReading};

/// The most a span's check reads, bisection included. A long idle span
/// with busy chat channels reads about half a megabyte an hour, so this
/// covers days; a span past it is not checked.
pub const SCAN_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

/// The most of one line kept in memory; the rest of a longer line is read
/// past (and counted), never kept. Every line the game writes is far
/// shorter.
const LINE_LIMIT: usize = 16 * 1024;

/// How far a stamp may fall behind the latest one before the log's order
/// is not trusted: lines from several channels can land a moment out of
/// order, but a stamp an hour back (a server clock stepped back) would
/// leave the bisection's answer unsound.
const BACKWARDS_TOLERANCE_SECONDS: f64 = 60.0;

/// The width of a line's stamp, `YYYY-MM-DD HH:MM:SS`.
const STAMP_WIDTH: usize = 19;

/// A UTF-8 byte order mark, which a file's first line may open with.
const BYTE_ORDER_MARK: &[u8] = b"\xEF\xBB\xBF";

/// Where the game's chat log is, read at the moment of use (the player may
/// point the app at another file), and the time base its stamps resolve
/// by.
#[derive(Clone)]
pub struct ChatLogSource {
    path: Arc<dyn Fn() -> PathBuf + Send + Sync>,
    clock: ChatLogClock,
}

impl ChatLogSource {
    pub fn new(path: impl Fn() -> PathBuf + Send + Sync + 'static, clock: ChatLogClock) -> Self {
        ChatLogSource {
            path: Arc::new(path),
            clock,
        }
    }

    /// A log at one fixed path.
    pub fn at(path: impl Into<PathBuf>, clock: ChatLogClock) -> Self {
        let path = path.into();
        ChatLogSource::new(move || path.clone(), clock)
    }

    /// What the log shows over `from..=to` (unix seconds): whether any line
    /// inside it is of one of `kinds`.
    pub fn check(&self, from: f64, to: f64, kinds: &[EventType]) -> SpanCheck {
        check_span(
            &(self.path)(),
            &self.clock,
            from,
            to,
            kinds,
            SCAN_BUDGET_BYTES,
        )
        .check
    }
}

impl fmt::Debug for ChatLogSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatLogSource")
            .field("path", &(self.path)())
            .finish_non_exhaustive()
    }
}

/// What the chat log shows over a span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanCheck {
    /// The log covers the span, and no line inside it is of a kind asked
    /// about.
    Clean,
    /// A line of this kind falls inside the span.
    Holds(EventType),
    /// The log starts after the span begins: it misses the span for good.
    StartsLater,
    /// The log has no line after the span's end yet: the game has not
    /// written past it, and may still.
    NotYetPast,
    /// The log could not be checked over the span.
    Unchecked(Unchecked),
}

/// Why a span could not be checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unchecked {
    /// The log could not be opened or read.
    Unreadable,
    /// Checking it would read more than [`SCAN_BUDGET_BYTES`].
    TooLong,
    /// The game server's offset from UTC is not known yet (nothing has
    /// been read live from the log since the app started), so no stamp
    /// can be placed in time.
    TimeBaseUnknown,
    /// The offset known now was settled or moved after the span began, so
    /// it cannot be shown to be the one the span was logged under.
    TimeBaseMoved,
    /// The log's stamps run backwards inside the span, so where the span
    /// lies in the file is not known.
    TimeRanBackwards,
}

/// A check, with how many bytes it read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpanScan {
    pub check: SpanCheck,
    pub bytes_read: u64,
}

/// Check the log at `path` over `from..=to`, reading at most `budget`
/// bytes.
pub(crate) fn check_span(
    path: &Path,
    clock: &ChatLogClock,
    from: f64,
    to: f64,
    kinds: &[EventType],
    budget: u64,
) -> SpanScan {
    let unchecked = |why| SpanScan {
        check: SpanCheck::Unchecked(why),
        bytes_read: 0,
    };
    match clock.offset_held_since() {
        None => return unchecked(Unchecked::TimeBaseUnknown),
        Some(since) if since as f64 > from => return unchecked(Unchecked::TimeBaseMoved),
        Some(_) => {}
    }
    let opened = File::open(path).and_then(|file| Ok((file.metadata()?.len(), file)));
    let Ok((len, file)) = opened else {
        return unchecked(Unchecked::Unreadable);
    };
    let mut scanner = Scanner {
        reader: BufReader::new(Counted {
            inner: file,
            read: 0,
        }),
        position: 0,
        clock: clock.clone(),
        budget,
        line: Vec::new(),
    };
    let check = match scan(&mut scanner, len, from, to, kinds) {
        Ok(check) => check,
        Err(Halt::Io) => SpanCheck::Unchecked(Unchecked::Unreadable),
        Err(Halt::Budget) => SpanCheck::Unchecked(Unchecked::TooLong),
    };
    SpanScan {
        check,
        bytes_read: scanner.reader.get_ref().read,
    }
}

/// Whether a line stamped `stamp` (the start of its second) reaches into a
/// span starting at `from`.
fn reaches(stamp: f64, from: f64) -> bool {
    stamp + 1.0 > from
}

fn scan(
    scanner: &mut Scanner,
    len: u64,
    from: f64,
    to: f64,
    kinds: &[EventType],
) -> Result<SpanCheck, Halt> {
    // The log must reach back to the span's start.
    match scanner.first_stamped_from(0)? {
        Some((_, first)) if first <= from => {}
        _ => return Ok(SpanCheck::StartsLater),
    }
    // The first offset whose first stamped line reaches the span: every
    // line before it was logged before the span began.
    let (mut low, mut high) = (0u64, len);
    while low < high {
        let middle = low + (high - low) / 2;
        match scanner.first_stamped_from(middle)? {
            Some((start, stamp)) if !reaches(stamp, from) => low = (middle + 1).max(start + 1),
            _ => high = middle,
        }
    }
    let Some((start, _)) = scanner.first_stamped_from(low)? else {
        return Ok(SpanCheck::NotYetPast);
    };
    scanner.seek(start)?;
    let mut latest = f64::NEG_INFINITY;
    while scanner.next_line()? {
        let Some(stamp) = scanner.stamp() else {
            continue;
        };
        if stamp < latest - BACKWARDS_TOLERANCE_SECONDS {
            return Ok(SpanCheck::Unchecked(Unchecked::TimeRanBackwards));
        }
        latest = latest.max(stamp);
        if stamp > to {
            return Ok(SpanCheck::Clean);
        }
        if reaches(stamp, from) {
            let text = String::from_utf8_lossy(&scanner.line);
            if let Some(event) = parse_line(&text) {
                if kinds.contains(&event.event_type) {
                    return Ok(SpanCheck::Holds(event.event_type));
                }
            }
        }
    }
    // The log has not reached past the span's end yet.
    Ok(SpanCheck::NotYetPast)
}

/// Why a scan stopped short.
enum Halt {
    Io,
    Budget,
}

/// A reader that counts the bytes it hands out.
struct Counted<R> {
    inner: R,
    read: u64,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.read += read as u64;
        Ok(read)
    }
}

impl<R: Seek> Seek for Counted<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

struct Scanner {
    reader: BufReader<Counted<File>>,
    /// The offset of the next byte the scanner hands out.
    position: u64,
    clock: ChatLogClock,
    budget: u64,
    /// The line last read, up to [`LINE_LIMIT`] bytes of it.
    line: Vec<u8>,
}

impl Scanner {
    fn seek(&mut self, to: u64) -> Result<(), Halt> {
        self.reader
            .seek(SeekFrom::Start(to))
            .map_err(|_| Halt::Io)?;
        self.position = to;
        Ok(())
    }

    /// Read the next line, through its newline; `false` at the end of the
    /// file.
    fn next_line(&mut self) -> Result<bool, Halt> {
        self.line.clear();
        let mut consumed = 0usize;
        loop {
            if self.reader.get_ref().read > self.budget {
                return Err(Halt::Budget);
            }
            let available = self.reader.fill_buf().map_err(|_| Halt::Io)?;
            if available.is_empty() {
                self.position += consumed as u64;
                return Ok(consumed > 0);
            }
            let (take, ends) = match available.iter().position(|&byte| byte == b'\n') {
                Some(newline) => (newline + 1, true),
                None => (available.len(), false),
            };
            let room = LINE_LIMIT.saturating_sub(self.line.len());
            self.line.extend_from_slice(&available[..take.min(room)]);
            self.reader.consume(take);
            consumed += take;
            if ends {
                self.position += consumed as u64;
                return Ok(true);
            }
        }
    }

    /// The instant the line last read was logged at, when it opens with a
    /// stamp.
    fn stamp(&self) -> Option<f64> {
        let line = self
            .line
            .strip_prefix(BYTE_ORDER_MARK)
            .unwrap_or(&self.line);
        let text = std::str::from_utf8(line.get(..STAMP_WIDTH)?).ok()?;
        let reading = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").ok()?;
        Some(self.clock.resolve_epoch(ChatLogReading::new(reading)))
    }

    /// The first stamped line starting at or after byte `at`: where it
    /// starts and when it was logged; `None` when no stamped line follows.
    /// Leaves that line as the one last read.
    fn first_stamped_from(&mut self, at: u64) -> Result<Option<(u64, f64)>, Halt> {
        if at == 0 {
            self.seek(0)?;
        } else {
            // From the byte before: when it ends a line, `at` starts one;
            // otherwise the line under `at` is passed over.
            self.seek(at - 1)?;
            if !self.next_line()? {
                return Ok(None);
            }
        }
        loop {
            let start = self.position;
            if !self.next_line()? {
                return Ok(None);
            }
            if let Some(stamp) = self.stamp() {
                return Ok(Some((start, stamp)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    /// 2026-11-16 12:00:00 UTC.
    const BASE: f64 = 1_794_830_400.0;

    const PLAY: &[EventType] = &[EventType::DamageDealt, EventType::Loot];

    fn stamp_of(at: f64) -> String {
        chrono::DateTime::from_timestamp(at as i64, 0)
            .expect("an instant")
            .naive_utc()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    }

    fn chat(at: f64) -> String {
        format!(
            "{} [#calypso] [Someone Else] anyone selling oil?\n",
            stamp_of(at)
        )
    }

    fn shot(at: f64) -> String {
        format!(
            "{} [System] [] You inflicted 12.5 points of damage\n",
            stamp_of(at)
        )
    }

    /// A log of one chat line a second over `seconds` from `BASE`, with the
    /// lines `extra` gives inserted after the line of their second.
    fn log(seconds: i64, extra: &[(i64, String)]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().expect("a temp file");
        let mut text = String::new();
        for second in 0..seconds {
            text.push_str(&chat(BASE + second as f64));
            for (_, line) in extra.iter().filter(|(at, _)| *at == second) {
                text.push_str(line);
            }
            if text.len() > 1 << 20 {
                file.write_all(text.as_bytes()).expect("written");
                text.clear();
            }
        }
        file.write_all(text.as_bytes()).expect("written");
        file.flush().expect("flushed");
        file
    }

    fn utc() -> ChatLogClock {
        ChatLogClock::pinned(0)
    }

    fn check(file: &tempfile::NamedTempFile, from: f64, to: f64) -> SpanScan {
        check_span(file.path(), &utc(), from, to, PLAY, SCAN_BUDGET_BYTES)
    }

    /// Two days of chat, about eleven megabytes: an hour inside it is found
    /// by bisection and read alone, a small part of the file.
    #[test]
    fn a_span_is_found_by_bisection_without_reading_the_file_whole() {
        let file = log(2 * 86_400, &[]);
        let len = file.as_file().metadata().unwrap().len();
        assert!(len > 8 << 20, "{len}");
        let from = BASE + 100_000.0;
        let scan = check(&file, from, from + 3_600.0);
        assert_eq!(scan.check, SpanCheck::Clean);
        // The hour's lines are about 230 KB; the bisection adds a buffer or
        // two a probe, about thirty probes.
        assert!(scan.bytes_read < 1 << 20, "{} of {len}", scan.bytes_read);
    }

    #[test]
    fn a_line_of_a_kind_asked_about_inside_the_span_is_found() {
        let file = log(4_000, &[(2_000, shot(BASE + 2_000.0))]);
        assert_eq!(
            check(&file, BASE + 1_000.0, BASE + 3_000.0).check,
            SpanCheck::Holds(EventType::DamageDealt)
        );
        // Outside the span, the same line is not this span's.
        assert_eq!(
            check(&file, BASE + 2_001.0, BASE + 3_000.0).check,
            SpanCheck::Clean
        );
        assert_eq!(
            check(&file, BASE + 1_000.0, BASE + 1_999.0).check,
            SpanCheck::Clean
        );
        // A line in the second the span starts in, or ends in, is inside.
        assert_eq!(
            check(&file, BASE + 2_000.5, BASE + 3_000.0).check,
            SpanCheck::Holds(EventType::DamageDealt)
        );
        assert_eq!(
            check(&file, BASE + 1_000.0, BASE + 2_000.0).check,
            SpanCheck::Holds(EventType::DamageDealt)
        );
    }

    #[test]
    fn a_line_of_another_kind_inside_the_span_leaves_it_clean() {
        let healed = format!(
            "{} [System] [] You healed yourself 12.0 points\n",
            stamp_of(BASE + 2_000.0)
        );
        let file = log(4_000, &[(2_000, healed)]);
        assert_eq!(
            check(&file, BASE + 1_000.0, BASE + 3_000.0).check,
            SpanCheck::Clean
        );
    }

    /// A log starting after the span misses it for good; one that has not
    /// written past it yet has only not reached it, and once it has, the
    /// span is checked.
    #[test]
    fn a_log_starting_after_the_span_misses_it_and_one_not_past_it_has_not_reached_it() {
        let mut file = log(4_000, &[]);
        assert_eq!(
            check(&file, BASE - 1.0, BASE + 3_000.0).check,
            SpanCheck::StartsLater
        );
        assert_eq!(
            check(&file, BASE + 1_000.0, BASE + 3_999.0).check,
            SpanCheck::NotYetPast
        );
        assert_eq!(
            check(&file, BASE + 4_500.0, BASE + 5_000.0).check,
            SpanCheck::NotYetPast
        );
        assert_eq!(check(&file, BASE, BASE + 3_998.0).check, SpanCheck::Clean);
        let empty = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(
            check(&empty, BASE, BASE + 1.0).check,
            SpanCheck::StartsLater
        );
        // The game writes on.
        file.write_all(chat(BASE + 4_100.0).as_bytes()).unwrap();
        file.flush().unwrap();
        assert_eq!(
            check(&file, BASE + 1_000.0, BASE + 3_999.0).check,
            SpanCheck::Clean
        );
    }

    /// An offset settled after the span began cannot be shown to be the one
    /// the span was logged under; one settled before it can.
    #[test]
    fn an_offset_settled_after_the_span_began_leaves_it_unchecked() {
        let file = log(4_000, &[]);
        let clock = ChatLogClock::observed();
        let settled_at = chrono::DateTime::from_timestamp(BASE as i64 + 2_000, 0).unwrap();
        clock.observe(ChatLogReading::new(settled_at.naive_utc()), settled_at);
        let at = |from: f64, to: f64| {
            check_span(file.path(), &clock, from, to, PLAY, SCAN_BUDGET_BYTES).check
        };
        assert_eq!(
            at(BASE + 1_000.0, BASE + 3_000.0),
            SpanCheck::Unchecked(Unchecked::TimeBaseMoved)
        );
        assert_eq!(at(BASE + 2_500.0, BASE + 3_000.0), SpanCheck::Clean);
    }

    #[test]
    fn a_span_past_the_budget_is_not_checked() {
        let file = log(20_000, &[]);
        let scan = check_span(
            file.path(),
            &utc(),
            BASE + 100.0,
            BASE + 19_000.0,
            PLAY,
            256 * 1024,
        );
        assert_eq!(scan.check, SpanCheck::Unchecked(Unchecked::TooLong));
    }

    #[test]
    fn without_the_server_s_offset_or_the_file_nothing_is_checked() {
        let file = log(100, &[]);
        assert_eq!(
            check_span(
                file.path(),
                &ChatLogClock::observed(),
                BASE + 10.0,
                BASE + 20.0,
                PLAY,
                SCAN_BUDGET_BYTES
            )
            .check,
            SpanCheck::Unchecked(Unchecked::TimeBaseUnknown)
        );
        let missing = file.path().with_extension("gone");
        assert_eq!(
            check_span(&missing, &utc(), BASE, BASE + 1.0, PLAY, SCAN_BUDGET_BYTES).check,
            SpanCheck::Unchecked(Unchecked::Unreadable)
        );
    }

    /// The stamps are the server's digits: a server two hours ahead of UTC
    /// logs a line at 14:00 for an instant of 12:00 UTC.
    #[test]
    fn stamps_resolve_through_the_server_s_offset() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for second in 0..100 {
            let digits = BASE + 7_200.0 + second as f64;
            let line = if second == 50 {
                format!(
                    "{} [System] [] You inflicted 3.0 points of damage\n",
                    stamp_of(digits)
                )
            } else {
                chat(digits)
            };
            file.write_all(line.as_bytes()).unwrap();
        }
        file.flush().unwrap();
        let ahead = ChatLogClock::pinned(7_200);
        let at = |from: f64, to: f64| {
            check_span(file.path(), &ahead, from, to, PLAY, SCAN_BUDGET_BYTES).check
        };
        assert_eq!(
            at(BASE + 40.0, BASE + 60.0),
            SpanCheck::Holds(EventType::DamageDealt)
        );
        assert_eq!(at(BASE + 10.0, BASE + 20.0), SpanCheck::Clean);
    }

    #[test]
    fn stamps_running_back_inside_the_span_leave_it_unchecked() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for second in [0, 10, 20, 30, 40] {
            file.write_all(chat(BASE + second as f64).as_bytes())
                .unwrap();
        }
        // An hour back, then on again.
        for second in [-3_600, 50, 60] {
            file.write_all(chat(BASE + second as f64).as_bytes())
                .unwrap();
        }
        file.flush().unwrap();
        assert_eq!(
            check(&file, BASE + 25.0, BASE + 55.0).check,
            SpanCheck::Unchecked(Unchecked::TimeRanBackwards)
        );
    }
}
