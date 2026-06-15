//! The process-wide tracing subscriber and the metrics-aggregating layer.
//!
//! [`init`] installs one subscriber for the whole process at the composition
//! root (`lib.rs run()`), before anything else runs, so every `tracing` event
//! the workspace emits is captured from the first instant. The subscriber has
//! three layers:
//!
//! * a fmt layer (human-readable diagnostics; stderr in this round, joined by
//!   the rolling non-blocking file appender once it lands), filtered by an
//!   env-filter (`ENTROPIAORME_LOG`, default `info`);
//! * the [`MetricsLayer`], which turns the database driver's own per-query
//!   `sqlx::query` events into database-latency samples in the in-process
//!   [`eo_wire::metrics`] registry. Capturing the driver's events (rather than
//!   wrapping every call site) means the metric covers every query in the
//!   process, including the tracker's hot per-event writes, with zero touch to
//!   the query code and so zero behaviour risk.
//!
//! The metrics layer carries its OWN target filter (`sqlx::query` only), so it
//! observes query events regardless of the fmt layer's verbosity: turning the
//! console logs down never blinds the database-latency metric.
//!
//! Behaviour-neutral: the subscriber is a pure observer. It reads event fields
//! (durations, counts, statement summaries) and never touches a response body,
//! an event payload, or the database. The fields it records to the registry
//! are timing only; no chatlog content or other PII reaches it.

use std::path::Path;
use std::time::Duration;

use tracing::field::{Field, Visit};
use tracing::Subscriber;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Held for the process lifetime by `run()`. In this round it carries no
/// state (the subscriber writes to stderr, which needs no flush); the rolling
/// non-blocking file appender's `WorkerGuard` moves in here when it lands, so
/// dropping the guard at process exit flushes any buffered log lines.
#[must_use = "dropping the telemetry guard flushes the rolling log appender"]
pub struct TelemetryGuard {
    // The non-blocking appender's flush guard: dropping it (at process exit,
    // when `run()` returns) drains and joins the worker thread so no buffered
    // log line is lost. `None` when the rolling file could not be opened and
    // the subscriber fell back to stderr-only.
    _appender: Option<WorkerGuard>,
}

/// Install the process-wide subscriber. Idempotent: a second call (a test that
/// already installed one) is a no-op rather than a panic, so production's
/// single startup call always wins and tests can install their own first.
pub fn init() -> TelemetryGuard {
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_writer(std::io::stderr)
        .with_filter(default_env_filter());

    // The rolling, non-blocking file layer under the app-data logs directory.
    // Degrade to stderr-only if the directory or appender cannot be opened: a
    // read-only or full data dir must never take the app down over logging.
    let (file_layer, appender_guard) = match build_file_layer(&crate::composition::log_dir()) {
        Ok((layer, guard)) => (Some(layer), Some(guard)),
        Err(err) => {
            // The subscriber is not installed yet, so report directly.
            eprintln!("[telemetry] rolling log file unavailable ({err}); logging to stderr only");
            (None, None)
        }
    };

    let _ = tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .with(metrics_layer())
        .try_init();

    TelemetryGuard {
        _appender: appender_guard,
    }
}

/// The default log filter: INFO, with the database driver's own per-query INFO
/// logging (one line per query) quieted to WARN so neither the console nor the
/// log file is flooded; the metrics layer still observes every query through
/// its own target filter, independent of this one. Overridden wholesale by
/// `ENTROPIAORME_LOG` (standard EnvFilter syntax, e.g. `debug` or
/// `eo::ocr=trace,info`).
fn default_env_filter() -> EnvFilter {
    EnvFilter::try_from_env("ENTROPIAORME_LOG")
        .or_else(|_| EnvFilter::try_new("info,sqlx::query=warn"))
        .unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Build the rolling-file log layer and its flush guard: daily-rotated files
/// with a bounded history under `log_dir`. The non-blocking writer drains on a
/// background worker thread, so a stalled disk never blocks an instrumented
/// path; the returned [`WorkerGuard`] flushes the channel on drop (the shell
/// holds it for the whole process, so it flushes at exit).
///
/// PII boundary: the file layer EXCLUDES the `eo::sidecar` target. That target
/// carries the relocated sidecar's own forwarded sub-process output, which the
/// shell does not control and so must never commit to a persistent file (it
/// still reaches the dev console via stderr). Every other target is the native
/// spine's own structured events, which carry only durations, counts, paths,
/// and error text, never chat-log content.
fn build_file_layer<S>(
    log_dir: &Path,
) -> Result<(impl Layer<S>, WorkerGuard), Box<dyn std::error::Error>>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    std::fs::create_dir_all(log_dir)?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("entropiaorme")
        .filename_suffix("log")
        .max_log_files(7)
        .build(log_dir)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_new("info,sqlx::query=warn,eo::sidecar=off")
        .expect("the static file-log filter directive is valid");
    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(writer)
        .with_filter(filter);
    Ok((layer, guard))
}

/// The metrics-aggregating layer with its own `sqlx::query`-only target
/// filter. Public to the crate so the database-latency integration test can
/// install a metrics-only subscriber without the fmt noise.
pub(crate) fn metrics_layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    // Only the driver's own per-query events ("sqlx::query"), at any level, so
    // the layer sees every completed query regardless of the console filter.
    MetricsLayer.with_filter(Targets::new().with_target("sqlx::query", LevelFilter::TRACE))
}

/// Records the elapsed time of each `sqlx::query` event into the database
/// latency histogram. Stateless: every event reaching it (the target filter
/// guarantees it is a completed query) carries an `elapsed_secs` field.
struct MetricsLayer;

impl<S> Layer<S> for MetricsLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = ElapsedVisitor::default();
        event.record(&mut visitor);
        if let Some(elapsed) = visitor.elapsed {
            eo_wire::metrics::metrics().record_db_query(elapsed);
        }
    }
}

/// Extracts the `elapsed_secs` field (the driver records each query's wall
/// time as fractional seconds) and converts it to a `Duration`.
#[derive(Default)]
struct ElapsedVisitor {
    elapsed: Option<Duration>,
}

impl Visit for ElapsedVisitor {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "elapsed_secs" && value.is_finite() && value >= 0.0 {
            self.elapsed = Some(Duration::from_secs_f64(value));
        }
    }

    // Required, but every field this layer cares about is numeric, so the
    // catch-all does nothing (it must not record statement text: that is the
    // one field that could echo a query, though never a bound value).
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real query under a metrics-only subscriber records a database-latency
    /// sample, proving the driver-event capture path works end to end (the
    /// instrumentation fires for "a DB query" per the round's acceptance).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_database_query_records_a_latency_sample() {
        // A metrics-only global subscriber (no fmt noise). Installed globally
        // so events emitted on the pool's worker threads are captured too.
        let _ = tracing_subscriber::registry()
            .with(metrics_layer())
            .try_init();

        let before = eo_wire::metrics::metrics()
            .snapshot()
            .db_query_latency
            .count;

        let dir = tempfile::tempdir().unwrap();
        let db = eo_services::db::Db::open(&dir.path().join("t.db"))
            .await
            .unwrap();
        // Any query: the schema_master read runs a real statement.
        let _ = db.schema_master().await.unwrap();

        let after = eo_wire::metrics::metrics()
            .snapshot()
            .db_query_latency
            .count;
        assert!(
            after > before,
            "a database query must record at least one latency sample (before={before}, after={after})"
        );
    }

    /// The rolling file appender writes the native spine's structured events
    /// and EXCLUDES the forwarded sidecar output (the one uncontrolled
    /// channel), so no chat-log-derived content can reach the persistent file.
    /// Also proves the appender opens and writes a rotating file at all.
    #[test]
    fn the_rolling_log_writes_structured_events_but_excludes_sidecar_output() {
        let dir = tempfile::tempdir().unwrap();
        let fake_chatlog =
            "2026-05-19 10:00:07 [System] [] You received Shrapnel x (250) Value: 0.025 PED";
        {
            let (file_layer, guard) = build_file_layer::<tracing_subscriber::Registry>(dir.path())
                .expect("the rolling appender builds over a fresh temp dir");
            let subscriber = tracing_subscriber::registry().with(file_layer);
            tracing::subscriber::with_default(subscriber, || {
                tracing::info!(target: "eo::ocr", elapsed_us = 4321u64, "ocr inference");
                // The forwarded sidecar channel, carrying a chat-log-shaped line.
                tracing::info!(target: "eo::sidecar", "{fake_chatlog}");
            });
            // Dropping the guard drains and joins the non-blocking worker, so
            // everything buffered is on disk before we read it.
            drop(guard);
        }
        let mut contents = String::new();
        for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
            contents.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
        assert!(
            contents.contains("ocr inference"),
            "the native spine's structured event is written to the rolling file"
        );
        assert!(
            !contents.contains(fake_chatlog),
            "forwarded sidecar output (the uncontrolled channel) never reaches the persistent file"
        );
        assert!(
            !contents.contains("Shrapnel"),
            "no chat-log-derived content lands in the log file"
        );
    }
}
