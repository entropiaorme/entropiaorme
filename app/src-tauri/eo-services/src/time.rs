//! Shared CPython-compatible time helpers: the naive-local epoch
//! conversions (fold=0 semantics) and the isoformat renderings every
//! service stamps and renders timestamps through. Factored out of the
//! tracker's time layer once the quest, codex, analytics, and scan
//! surfaces all needed the same conversions; the tracker keeps its own
//! module for the payload-parsing helpers only it uses.

use chrono::{DateTime, NaiveDateTime, Utc};

use crate::chatlog_time::ChatLogReading;
use eo_wire::normalizer::round_half_even;

/// The payload timestamp form (isoformat, with or without fractional
/// seconds) parsed back to the reading it carries. Every string this
/// parses came from the chat log, so it yields a [`ChatLogReading`]
/// rather than a bare naive reading: the log is stamped in the game
/// server's zone, and resolving one of these against the host's would
/// skew it (see [`crate::chatlog_time`]).
pub(crate) fn parse_timestamp_str(raw: &str) -> Option<ChatLogReading> {
    NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S"))
        .ok()
        .map(ChatLogReading::new)
}

/// CPython `datetime.fromtimestamp`'s split of an epoch float into
/// whole seconds and half-even-rounded microseconds.
pub(crate) fn epoch_to_parts(epoch: f64) -> (i64, u32) {
    let mut secs = epoch.trunc() as i64;
    let mut micros = round_half_even((epoch - epoch.trunc()) * 1e6, 0) as i64;
    if micros >= 1_000_000 {
        secs += 1;
        micros -= 1_000_000;
    } else if micros < 0 {
        secs -= 1;
        micros += 1_000_000;
    }
    (secs, micros as u32)
}

/// Resolve a naive local wall-clock reading to the instant it names,
/// with the original's fold=0 rule: the earliest interpretation for
/// ambiguous readings, and a reading inside a DST gap resolved through
/// the neighbouring hour's offset. This is the one local-to-instant
/// boundary: the injected clock's readings resolve here once, and
/// interiors carry the instant.
///
/// A chat-log timestamp is **not** one of these. The log is stamped in
/// the game server's zone, so it resolves through
/// [`crate::chatlog_time::ChatLogClock`] instead; [`ChatLogReading`]
/// exists so that cannot be got wrong by habit.
pub fn resolve_local(reading: NaiveDateTime) -> DateTime<Utc> {
    let resolved = match reading.and_local_timezone(chrono::Local) {
        chrono::LocalResult::Single(instant) => Some(instant),
        chrono::LocalResult::Ambiguous(earliest, _) => Some(earliest),
        chrono::LocalResult::None => (reading + chrono::TimeDelta::hours(1))
            .and_local_timezone(chrono::Local)
            .earliest()
            .map(|shifted| shifted - chrono::TimeDelta::hours(1)),
    };
    resolved
        .map(|instant| instant.with_timezone(&Utc))
        .unwrap_or_default()
}

/// The instant as the epoch-seconds float the database stores
/// (fractional seconds preserved).
pub fn instant_to_epoch(instant: DateTime<Utc>) -> f64 {
    instant.timestamp() as f64 + f64::from(instant.timestamp_subsec_micros()) / 1e6
}

/// The instant a stored epoch float names.
pub fn epoch_to_instant(epoch: f64) -> DateTime<Utc> {
    let (secs, micros) = epoch_to_parts(epoch);
    DateTime::from_timestamp(secs, micros * 1_000).unwrap_or_default()
}

/// Render an instant the way the naive-local surfaces always have:
/// the local wall-clock isoformat (no offset suffix, microseconds only
/// when non-zero). A reading inside a DST gap cannot round-trip (no
/// local wall clock ever showed it); the instant is the truth and the
/// render normalises.
pub fn local_isoformat(instant: DateTime<Utc>) -> String {
    naive_isoformat(instant.with_timezone(&chrono::Local).naive_local())
}

/// The original's naive-local `datetime.timestamp()` (fold=0), kept
/// for the surfaces still carrying naive readings; instant-based
/// interiors use `resolve_local` + `instant_to_epoch` (this is their
/// composition).
pub fn naive_to_epoch(instant: NaiveDateTime) -> f64 {
    // `resolve_local` collapses an unresolvable reading to the epoch
    // origin; the original returned 0.0 there, which is the same value.
    instant_to_epoch(resolve_local(instant))
}

/// The injected clock's wall-clock instant as whole epoch milliseconds:
/// the one time base the creature watcher's looks and the log reader's
/// arrival of a kill line are both stamped on, so the two compare.
pub fn clock_epoch_ms(clock: &dyn crate::clock::Clock) -> i64 {
    (naive_to_epoch(clock.now()) * 1000.0).round() as i64
}

/// A naive `datetime.isoformat()`: no offset suffix, microseconds
/// only when non-zero. The instant-carrying surfaces render through
/// `local_isoformat`, which converts to the local wall clock and then
/// through this.
pub fn naive_isoformat(instant: NaiveDateTime) -> String {
    if instant.and_utc().timestamp_subsec_micros() == 0 {
        instant.format("%Y-%m-%dT%H:%M:%S").to_string()
    } else {
        instant.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
    }
}

/// The original `to_iso_utc` helper: render an epoch float as
/// `datetime.fromtimestamp(ts, tz=UTC).isoformat()` does (the `T`
/// separator, microseconds only when non-zero, `+00:00` suffix).
pub fn to_iso_utc(ts: f64) -> String {
    let (secs, micros) = epoch_to_parts(ts);
    let instant = chrono::DateTime::from_timestamp(secs, micros * 1_000).unwrap_or_default();
    if micros == 0 {
        format!("{}+00:00", instant.format("%Y-%m-%dT%H:%M:%S"))
    } else {
        format!("{}+00:00", instant.format("%Y-%m-%dT%H:%M:%S%.6f"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_to_instant_scales_microseconds_to_nanoseconds() {
        // 1.5s is one whole second plus 500_000 microseconds; the fractional
        // part must reach the instant as microsecond precision, not be
        // divided away.
        let instant = epoch_to_instant(1.5);
        assert_eq!(instant.timestamp(), 1);
        assert_eq!(instant.timestamp_subsec_micros(), 500_000);
        // A whole-second epoch carries no fractional part.
        assert_eq!(epoch_to_instant(42.0).timestamp_subsec_micros(), 0);
    }
}
