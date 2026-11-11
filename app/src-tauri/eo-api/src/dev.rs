//! The developer-tools family: the in-process metrics snapshot, the
//! crash-reporting opt-in toggle, and the two maintenance actions
//! (database compaction and the projection rebuild-and-verify).
//!
//! Every operation is gated on developer mode, read FRESH from the settings
//! file on each call (never cached), so a toggle in Settings takes effect
//! without a restart. When developer mode is off the operation returns
//! [`ApiError::NotFound`] (`kind: "notFound"`), keeping the whole family off
//! a default install and off the equivalence-covered surface.
//!
//! The family is native-only: it never had a Python arm, carries no
//! corpus golden and no OpenAPI path, so nothing here re-pins a frozen
//! contract. Contract lineage: two shapes softened at the typed-command
//! crossing, both invisible to
//! the sole consumer (the hidden developer-metrics page): the malformed-
//! body 400 the crash-reporting write answered is unrepresentable over a
//! typed `bool` argument, and the "no data dir / no composed database"
//! 404 legs collapse (the facade always carries both). The response bytes
//! are unchanged: [`MetricsSnapshot`] mirrors the [`eo_wire::metrics`]
//! snapshot field-for-field (bridged through `serde_json` and pinned
//! byte-identical below), and the crash-reporting / compaction / rebuild
//! bodies match the hand-built HTTP JSON exactly.

use eo_services::auction_fee_research::{ResearchError, ResearchStatus};
use eo_services::config_service::load_config_readonly;
use eo_services::maintenance::rebuild_and_verify;
use eo_services::observability_config::{crash_reporting_enabled, set_crash_reporting_enabled};
use eo_services::time::naive_to_epoch;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Nullable;
use crate::{Api, ApiError};

/// The compacted-copy name written beside the live database; the name is
/// a stable contract, so the reported path never moves.
const COMPACTED_DB_NAME: &str = "entropia_orme-compacted.db";

// ── Response DTOs ───────────────────────────────────────────────────

/// One latency-histogram bucket: the inclusive upper bound in
/// microseconds (`None` for the final overflow bucket) and its count.
/// Mirrors `eo_wire::metrics::Bucket`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistogramBucket {
    pub bound_us: Nullable<u64>,
    pub count: u64,
}

/// A point-in-time read of one latency histogram: the total count, the
/// microsecond sum (so a mean is recoverable), and the per-bucket counts.
/// Mirrors `eo_wire::metrics::HistogramSnapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistogramSnapshot {
    pub count: u64,
    pub sum_us: u64,
    pub buckets: Vec<HistogramBucket>,
}

/// A point-in-time read of the process telemetry registry: event
/// throughput, the OCR / database latency histograms, and the
/// resource-drift gauges. Counts and durations only; no PII. Mirrors
/// `eo_wire::metrics::MetricsSnapshot` field-for-field, so
/// `serde_json::to_string` yields identical bytes (pinned in the tests below).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MetricsSnapshot {
    pub events_published: u64,
    pub ocr_latency: HistogramSnapshot,
    pub db_query_latency: HistogramSnapshot,
    /// The creature watcher's cost per look: capturing the game window,
    /// searching it for the locked plate, and reading the plate found.
    pub plate_capture_latency: HistogramSnapshot,
    pub plate_search_latency: HistogramSnapshot,
    pub plate_read_latency: HistogramSnapshot,
    pub rss_bytes: u64,
    pub handle_count: u64,
}

/// The crash-reporting opt-in state, a one-key body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CrashReportingStatus {
    pub crash_reporting_enabled: bool,
}

/// The compaction result: the path of the written compacted copy and its
/// size in bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CompactResult {
    pub path: String,
    pub bytes: u64,
}

/// One projection table's verdict from a rebuild-and-verify run: the
/// table name, whether its rebuilt rows were byte-identical to the
/// incrementally-maintained ones, and the row count after the rebuild.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct TableVerdict {
    pub table: String,
    pub matched: bool,
    pub row_count: i64,
}

/// The rebuild-and-verify report: whether every projection rebuilt
/// byte-identically, and the per-table verdicts in their stable order.
/// `allMatched` keeps its camelCase HTTP key; the verdict fields keep
/// their snake_case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct RebuildReport {
    #[serde(rename = "allMatched")]
    pub all_matched: bool,
    pub tables: Vec<TableVerdict>,
}

/// The last attempt made by the research collector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuctionFeeCaptureStatus {
    pub sample: i64,
    pub accepted: bool,
    pub message: String,
}

/// Full main-window state for the development research session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuctionFeeResearchStatus {
    pub active: bool,
    pub busy: bool,
    pub sample_count: i64,
    pub output_dir: Nullable<String>,
    pub last_capture: Nullable<AuctionFeeCaptureStatus>,
}

/// Least-privilege state exposed to the floating overlay. It deliberately
/// omits the filesystem path and every OCR field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuctionFeeOverlayStatus {
    pub active: bool,
    pub busy: bool,
    pub sample_count: i64,
    pub message: Nullable<String>,
    pub failed: bool,
}

// ── Facade methods ──────────────────────────────────────────────────

impl Api {
    /// Shell-only state check for choosing which authority the capture
    /// overlay represents. This is not an IPC operation and exposes no data.
    pub fn auction_fee_research_active_for_shell(&self) -> bool {
        self.auction_fee_research.is_active()
    }

    /// Shell teardown used when the floating window closes, even if the
    /// developer-mode setting changed while it was open.
    pub fn stop_auction_fee_research_for_shell(&self) {
        self.spacebar.set_research_enabled(false);
        self.auction_fee_research.stop();
    }

    pub fn dev_auction_fee_research_start(&self) -> Result<AuctionFeeResearchStatus, ApiError> {
        self.require_developer_mode()?;
        let status = self.auction_fee_research.start().map_err(research_error)?;
        self.spacebar.set_research_enabled(true);
        Ok(research_status(status))
    }

    pub fn dev_auction_fee_research_stop(&self) -> Result<AuctionFeeResearchStatus, ApiError> {
        self.require_developer_mode()?;
        self.spacebar.set_research_enabled(false);
        Ok(research_status(self.auction_fee_research.stop()))
    }

    pub fn dev_auction_fee_research_status(&self) -> Result<AuctionFeeResearchStatus, ApiError> {
        self.require_developer_mode()?;
        Ok(research_status(self.auction_fee_research.status()))
    }

    /// Capture through the floating overlay while returning status only.
    /// The full observation goes to the local dataset, never the renderer.
    pub fn dev_auction_fee_research_capture(&self) -> Result<AuctionFeeOverlayStatus, ApiError> {
        self.require_developer_mode()?;
        let _ = self
            .auction_fee_research
            .capture()
            .map_err(research_error)?;
        Ok(research_overlay_status(self.auction_fee_research.status()))
    }

    pub fn dev_auction_fee_research_overlay_status(
        &self,
    ) -> Result<AuctionFeeOverlayStatus, ApiError> {
        self.require_developer_mode()?;
        Ok(research_overlay_status(self.auction_fee_research.status()))
    }

    /// The in-process metrics snapshot (throughput counts, latency
    /// histograms, resource-drift gauges). Gate-off => [`ApiError::NotFound`].
    pub fn dev_metrics(&self) -> Result<MetricsSnapshot, ApiError> {
        self.require_developer_mode()?;
        let snapshot = eo_wire::metrics::metrics().snapshot();
        // The wire snapshot is a plain serde struct; bridge it into the
        // typed DTO through a JSON value (the byte-parity pin below proves
        // the two serialise identically).
        let value = serde_json::to_value(&snapshot)
            .map_err(ApiError::internal("metrics snapshot encode"))?;
        serde_json::from_value(value).map_err(ApiError::internal("metrics snapshot decode"))
    }

    /// The current crash-reporting opt-in. Gate-off => [`ApiError::NotFound`].
    pub fn dev_crash_reporting(&self) -> Result<CrashReportingStatus, ApiError> {
        self.require_developer_mode()?;
        Ok(CrashReportingStatus {
            crash_reporting_enabled: crash_reporting_enabled(&self.data_dir),
        })
    }

    /// Set the crash-reporting opt-in, acknowledging with the resulting
    /// state. Gate-off => [`ApiError::NotFound`]; a persist failure =>
    /// [`ApiError::Internal`]. (A malformed body is unrepresentable over
    /// the typed `bool`.)
    pub fn dev_set_crash_reporting(&self, enabled: bool) -> Result<CrashReportingStatus, ApiError> {
        self.require_developer_mode()?;
        set_crash_reporting_enabled(&self.data_dir, enabled)
            .map_err(ApiError::internal("crash reporting write"))?;
        Ok(CrashReportingStatus {
            crash_reporting_enabled: enabled,
        })
    }

    /// Compact the database into a fresh copy via `VACUUM INTO`, reclaiming
    /// the free pages churn leaves behind. Writes `entropia_orme-compacted.db`
    /// beside the live database (never locking the live file for the rewrite)
    /// and returns its path and byte size. Gate-off => [`ApiError::NotFound`].
    pub async fn dev_compact_database(&self) -> Result<CompactResult, ApiError> {
        self.require_developer_mode()?;
        let dest = self.data_dir.join(COMPACTED_DB_NAME);
        // VACUUM INTO refuses to overwrite an existing file; clear any prior
        // copy from an earlier compaction first.
        let _ = std::fs::remove_file(&dest);
        self.db
            .vacuum_into(&dest)
            .await
            .map_err(ApiError::internal("database compaction"))?;
        let bytes = std::fs::metadata(&dest).map(|meta| meta.len()).unwrap_or(0);
        Ok(CompactResult {
            path: dest.to_string_lossy().into_owned(),
            bytes,
        })
    }

    /// Rebuild every read-model projection from the raw tracking tables and
    /// report whether each rebuilt byte-identically to the incrementally-
    /// maintained rows (the CQRS rebuildability proof, runnable on demand).
    /// Gate-off => [`ApiError::NotFound`].
    pub async fn dev_rebuild_projections(&self) -> Result<RebuildReport, ApiError> {
        self.require_developer_mode()?;
        // A maintenance action, off the equivalence surface: the wall clock
        // sets the heal watermark. The rebuild and the incremental heal share
        // this `now`, so the equality proof is independent of its value.
        let now = naive_to_epoch(chrono::Utc::now().naive_utc());
        let report = rebuild_and_verify(&self.db, now)
            .await
            .map_err(ApiError::internal("projection rebuild"))?;
        Ok(RebuildReport {
            all_matched: report.all_matched(),
            tables: report
                .tables
                .into_iter()
                .map(|verdict| TableVerdict {
                    table: verdict.table.to_string(),
                    matched: verdict.matched,
                    row_count: verdict.row_count as i64,
                })
                .collect(),
        })
    }

    /// Whether developer mode is currently enabled, read FRESH from the
    /// settings file on each call (never cached), so the hidden dev-tools
    /// gate reflects a toggle without a restart. An unreadable or malformed
    /// config reads as off (the default).
    fn developer_mode(&self) -> bool {
        load_config_readonly(&self.data_dir)
            .map(|config| config.developer_mode_enabled)
            .unwrap_or(false)
    }

    /// The dev-family gate: `Ok` when developer mode is on, else not-found
    /// (a gated-off dev command is indistinguishable from an absent one).
    fn require_developer_mode(&self) -> Result<(), ApiError> {
        if self.developer_mode() {
            Ok(())
        } else {
            Err(ApiError::not_found("Not Found"))
        }
    }
}

fn research_error(error: ResearchError) -> ApiError {
    match error {
        ResearchError::Unavailable | ResearchError::Inactive => {
            ApiError::invalid_state(error.to_string())
        }
        ResearchError::Busy => ApiError::invalid_state(error.to_string()),
        ResearchError::Io(_) | ResearchError::Encode => {
            ApiError::internal("auction fee research capture")(error)
        }
    }
}

fn research_status(status: ResearchStatus) -> AuctionFeeResearchStatus {
    AuctionFeeResearchStatus {
        active: status.active,
        busy: status.busy,
        sample_count: status.sample_count as i64,
        output_dir: status
            .output_dir
            .map(|path| path.to_string_lossy().into_owned())
            .into(),
        last_capture: status
            .last_capture
            .map(|capture| AuctionFeeCaptureStatus {
                sample: capture.sample as i64,
                accepted: capture.accepted,
                message: capture.message,
            })
            .into(),
    }
}

fn research_overlay_status(status: ResearchStatus) -> AuctionFeeOverlayStatus {
    let failed = status
        .last_capture
        .as_ref()
        .is_some_and(|capture| !capture.accepted);
    AuctionFeeOverlayStatus {
        active: status.active,
        busy: status.busy,
        sample_count: status.sample_count as i64,
        message: status.last_capture.map(|capture| capture.message).into(),
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_metrics_dto_serialises_byte_identically_to_the_wire_snapshot() {
        // Record onto the process registry so the snapshot carries a
        // non-zero counter alongside its always-present histograms,
        // exercising every field name.
        eo_wire::metrics::metrics().record_event_published();
        let snapshot = eo_wire::metrics::metrics().snapshot();
        let value = serde_json::to_value(&snapshot).unwrap();
        let dto: MetricsSnapshot = serde_json::from_value(value).unwrap();
        assert_eq!(
            serde_json::to_string(&dto).unwrap(),
            serde_json::to_string(&snapshot).unwrap(),
            "the DTO mirror must serialise byte-identically to the wire snapshot"
        );
    }

    #[test]
    fn the_rebuild_report_keeps_the_camelcase_all_matched_key() {
        let report = RebuildReport {
            all_matched: true,
            tables: vec![TableVerdict {
                table: "daily_rollup".into(),
                matched: true,
                row_count: 3,
            }],
        };
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            serde_json::json!({
                "allMatched": true,
                "tables": [{"table": "daily_rollup", "matched": true, "row_count": 3}]
            })
        );
    }

    #[test]
    fn the_crash_reporting_body_carries_the_snake_case_key() {
        assert_eq!(
            serde_json::to_value(CrashReportingStatus {
                crash_reporting_enabled: true,
            })
            .unwrap(),
            serde_json::json!({"crash_reporting_enabled": true})
        );
    }
}
