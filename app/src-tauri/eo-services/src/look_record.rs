//! The stored record of how well the screen was read during a session:
//! the watchers' [`LookMinute`]s, kept in `screen_look_minutes` beside how
//! contended the machine's processors were when each was written.
//!
//! It is a diagnostic record. Nothing in the app reads it to decide
//! anything; it is there so a session whose kills went unnamed, or whose
//! tool in hand lagged, can be told apart afterwards from one the watcher
//! simply could not see: looks skipped, dropped or late, plates found but
//! unread, and a busy machine each leave their own mark.

use crate::db::DbError;
use crate::screen_watch::LookMinute;

/// How contended the processors are, where the system says: the share of
/// the last minute (0 to 100) in which some task waited for one.
pub type CpuPressure = std::sync::Arc<dyn Fn() -> Option<f64> + Send + Sync>;

/// The system's own account of processor pressure (Linux's pressure stall
/// information); `None` elsewhere, or where the kernel does not keep it.
pub fn system_cpu_pressure() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        parse_pressure(&std::fs::read_to_string("/proc/pressure/cpu").ok()?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// The one-minute average of the `some` line of a pressure file.
fn parse_pressure(text: &str) -> Option<f64> {
    text.lines()
        .find_map(|line| line.strip_prefix("some "))?
        .split_whitespace()
        .find_map(|field| field.strip_prefix("avg60="))?
        .parse::<f64>()
        .ok()
        .filter(|share| share.is_finite() && *share >= 0.0)
}

/// Keep `minutes`, each beside `cpu_pressure`. A minute already kept (the
/// same watcher, session and start) is left as it is.
pub fn write(
    tx: &rusqlite::Transaction<'_>,
    minutes: &[LookMinute],
    cpu_pressure: Option<f64>,
) -> Result<(), DbError> {
    let mut insert = tx.prepare(
        "INSERT OR IGNORE INTO screen_look_minutes \
         (session_id, watcher, started_at, looks, unfocused, missed, frames, late, found, \
          unread, max_look_ms, max_capture_ms, max_grab_ms, cpu_pressure) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )?;
    for minute in minutes {
        insert.execute(rusqlite::params![
            minute.session_id,
            minute.watcher,
            minute.started_ms as f64 / 1000.0,
            minute.looks,
            minute.unfocused,
            minute.missed,
            minute.frames,
            minute.late,
            minute.found,
            minute.unread,
            minute.max_look_ms,
            minute.max_capture_ms,
            minute.max_grab_ms,
            cpu_pressure,
        ])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_one_minute_share_of_the_some_line_is_the_pressure() {
        let text = "some avg10=1.20 avg60=35.90 avg300=41.60 total=123456\n\
                    full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
        assert_eq!(parse_pressure(text), Some(35.9));
        assert_eq!(parse_pressure("full avg10=0.00 avg60=2.00\n"), None);
        assert_eq!(parse_pressure("some avg10=1.20\n"), None);
        assert_eq!(parse_pressure("some avg60=many\n"), None);
        assert_eq!(parse_pressure(""), None);
    }
}
