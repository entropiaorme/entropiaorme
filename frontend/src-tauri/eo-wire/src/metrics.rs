//! Process-global, lock-free telemetry registry for the observability floor.
//!
//! A single static the whole workspace records into from its hot paths
//! (event-bus publishes, OCR inferences, database queries, HTTP requests)
//! and a periodic resource sampler (RSS / handle count). The registry is the
//! aggregation sink the hidden devtools metrics page reads, and the source
//! the rolling structured logs' drift samples derive from, so the soak's
//! drift gates (RSS trend, handle count, per-event latencies) are measured
//! from telemetry rather than eyeballed.
//!
//! Behaviour-neutral by construction: every record path is a single atomic
//! add or store (no lock, no allocation, never unwinds), so instrumentation
//! on a hot path or a synchronous event-bus tap can never block, allocate,
//! or panic into the producer it observes. Nothing here touches a response
//! body, an event payload, or the database state, and no recorded field
//! carries chatlog content or any other PII (durations, counts, and
//! process-resource gauges only).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Inclusive upper bounds, in microseconds, of the latency histogram
/// buckets. A request/query/inference whose elapsed time exceeds the last
/// bound lands in the implicit overflow bucket. The spread (50us to 1s)
/// spans the OCR inference range (sub-ms warm CPU reads to multi-hundred-ms
/// DirectML shader-cold runs) and the single-connection DB pool's
/// acquire-plus-execute latencies.
const LATENCY_BUCKET_BOUNDS_US: [u64; 14] = [
    50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000, 500_000,
    1_000_000,
];

/// One bucket count plus the bucket's inclusive upper bound, for the
/// serialised snapshot. The overflow bucket reports `bound_us = None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bucket {
    /// The bucket's inclusive upper bound in microseconds, or `None` for the
    /// final overflow bucket (anything above the last bound).
    pub bound_us: Option<u64>,
    pub count: u64,
}

/// A read-only snapshot of a [`LatencyHistogram`]: the per-bucket counts,
/// the total count, and the microsecond sum (so a mean is recoverable).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistogramSnapshot {
    pub count: u64,
    pub sum_us: u64,
    pub buckets: Vec<Bucket>,
}

impl HistogramSnapshot {
    /// The arithmetic mean in microseconds, or `None` when nothing has been
    /// recorded (so the page renders a dash rather than a divide-by-zero).
    pub fn mean_us(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum_us as f64 / self.count as f64)
    }
}

/// A fixed-bucket latency histogram over atomic counters: lock-free,
/// allocation-free, and safe to update from any thread, including a
/// synchronous event-bus publisher tap. Keeps a running count and a
/// microsecond sum alongside the buckets so a mean is recoverable.
#[derive(Debug)]
pub struct LatencyHistogram {
    // One slot per `LATENCY_BUCKET_BOUNDS_US` entry, plus a final overflow
    // slot for anything above the last bound.
    buckets: [AtomicU64; LATENCY_BUCKET_BOUNDS_US.len() + 1],
    count: AtomicU64,
    sum_us: AtomicU64,
}

impl LatencyHistogram {
    const fn new() -> Self {
        Self {
            buckets: [const { AtomicU64::new(0) }; LATENCY_BUCKET_BOUNDS_US.len() + 1],
            count: AtomicU64::new(0),
            sum_us: AtomicU64::new(0),
        }
    }

    /// Record one observation. Saturates an absurd duration to `u64::MAX`
    /// microseconds rather than overflowing (a clock anomaly cannot panic an
    /// instrumented hot path).
    pub fn record(&self, elapsed: Duration) {
        let us = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        let index = LATENCY_BUCKET_BOUNDS_US
            .iter()
            .position(|&bound| us <= bound)
            .unwrap_or(LATENCY_BUCKET_BOUNDS_US.len());
        self.buckets[index].fetch_add(1, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_us.fetch_add(us, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> HistogramSnapshot {
        let buckets = self
            .buckets
            .iter()
            .enumerate()
            .map(|(index, slot)| Bucket {
                bound_us: LATENCY_BUCKET_BOUNDS_US.get(index).copied(),
                count: slot.load(Ordering::Relaxed),
            })
            .collect();
        HistogramSnapshot {
            count: self.count.load(Ordering::Relaxed),
            sum_us: self.sum_us.load(Ordering::Relaxed),
            buckets,
        }
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

/// The process-wide telemetry registry. Construct a fresh instance in tests
/// ([`Metrics::new`] is `const`); production records into the single global
/// returned by [`metrics`].
#[derive(Debug)]
pub struct Metrics {
    events_published: AtomicU64,
    http_requests: AtomicU64,
    ocr_latency: LatencyHistogram,
    db_query_latency: LatencyHistogram,
    http_request_latency: LatencyHistogram,
    // Drift gauges, set by the periodic resource sampler. `0` means
    // "not yet sampled" (the page renders a dash); resident-set bytes and
    // the OS handle/descriptor count are the two monotonic-growth signals
    // the soak's drift gate watches.
    rss_bytes: AtomicU64,
    handle_count: AtomicU64,
}

impl Metrics {
    pub const fn new() -> Self {
        Self {
            events_published: AtomicU64::new(0),
            http_requests: AtomicU64::new(0),
            ocr_latency: LatencyHistogram::new(),
            db_query_latency: LatencyHistogram::new(),
            http_request_latency: LatencyHistogram::new(),
            rss_bytes: AtomicU64::new(0),
            handle_count: AtomicU64::new(0),
        }
    }

    /// Count one event-bus publish (the per-service event-throughput
    /// signal). Called from `EventBus::publish` on the publisher's thread.
    pub fn record_event_published(&self) {
        self.events_published.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one OCR inference's wall-clock latency.
    pub fn record_ocr_latency(&self, elapsed: Duration) {
        self.ocr_latency.record(elapsed);
    }

    /// Record one database query's wall-clock latency (acquire plus execute,
    /// as observed by the driver: the single-connection pool serialises both
    /// into one figure).
    pub fn record_db_query(&self, elapsed: Duration) {
        self.db_query_latency.record(elapsed);
    }

    /// Record one served HTTP request: bumps the count and the latency
    /// histogram.
    pub fn record_http_request(&self, elapsed: Duration) {
        self.http_requests.fetch_add(1, Ordering::Relaxed);
        self.http_request_latency.record(elapsed);
    }

    /// Set the latest resident-set-size sample, in bytes.
    pub fn set_rss_bytes(&self, bytes: u64) {
        self.rss_bytes.store(bytes, Ordering::Relaxed);
    }

    /// Set the latest OS handle/descriptor-count sample.
    pub fn set_handle_count(&self, count: u64) {
        self.handle_count.store(count, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            events_published: self.events_published.load(Ordering::Relaxed),
            http_requests: self.http_requests.load(Ordering::Relaxed),
            ocr_latency: self.ocr_latency.snapshot(),
            db_query_latency: self.db_query_latency.snapshot(),
            http_request_latency: self.http_request_latency.snapshot(),
            rss_bytes: self.rss_bytes.load(Ordering::Relaxed),
            handle_count: self.handle_count.load(Ordering::Relaxed),
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// A serialisable point-in-time read of the registry, the body the hidden
/// devtools metrics route returns and the shape the rolling logs' drift
/// samples mirror. Counts and durations only: no PII, ever.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricsSnapshot {
    pub events_published: u64,
    pub http_requests: u64,
    pub ocr_latency: HistogramSnapshot,
    pub db_query_latency: HistogramSnapshot,
    pub http_request_latency: HistogramSnapshot,
    pub rss_bytes: u64,
    pub handle_count: u64,
}

static METRICS: Metrics = Metrics::new();

/// The process-wide telemetry registry every instrumented seam records into
/// and the devtools metrics route reads from.
pub fn metrics() -> &'static Metrics {
    &METRICS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_registry_reads_all_zeroes() {
        let m = Metrics::new();
        let snap = m.snapshot();
        assert_eq!(snap.events_published, 0);
        assert_eq!(snap.http_requests, 0);
        assert_eq!(snap.ocr_latency.count, 0);
        assert_eq!(snap.db_query_latency.count, 0);
        assert_eq!(snap.http_request_latency.count, 0);
        assert_eq!(snap.rss_bytes, 0);
        assert_eq!(snap.handle_count, 0);
        assert_eq!(snap.ocr_latency.mean_us(), None);
    }

    #[test]
    fn counters_and_gauges_record() {
        let m = Metrics::new();
        m.record_event_published();
        m.record_event_published();
        m.record_http_request(Duration::from_millis(3));
        m.set_rss_bytes(1_234_567);
        m.set_handle_count(42);
        let snap = m.snapshot();
        assert_eq!(snap.events_published, 2);
        assert_eq!(snap.http_requests, 1);
        assert_eq!(snap.http_request_latency.count, 1);
        assert_eq!(snap.rss_bytes, 1_234_567);
        assert_eq!(snap.handle_count, 42);
    }

    #[test]
    fn histogram_buckets_by_upper_bound_and_tracks_the_mean() {
        let m = Metrics::new();
        // 30us -> first bucket (<= 50us); 300us -> the <= 500us bucket;
        // 2s -> the overflow bucket (above the 1s last bound).
        m.record_ocr_latency(Duration::from_micros(30));
        m.record_ocr_latency(Duration::from_micros(300));
        m.record_ocr_latency(Duration::from_secs(2));
        let h = m.snapshot().ocr_latency;
        assert_eq!(h.count, 3);
        assert_eq!(
            h.buckets[0],
            Bucket {
                bound_us: Some(50),
                count: 1
            }
        );
        // 300us lands in the <= 500us bucket (index 3: 50,100,250,500).
        assert_eq!(
            h.buckets[3],
            Bucket {
                bound_us: Some(500),
                count: 1
            }
        );
        // The overflow bucket is the last slot, reported with no bound.
        let overflow = h.buckets.last().unwrap();
        assert_eq!(overflow.bound_us, None);
        assert_eq!(overflow.count, 1);
        // Mean over 30 + 300 + 2_000_000 us.
        assert_eq!(h.mean_us(), Some((30.0 + 300.0 + 2_000_000.0) / 3.0));
    }

    #[test]
    fn the_snapshot_round_trips_through_json() {
        let m = Metrics::new();
        m.record_db_query(Duration::from_micros(120));
        let snap = m.snapshot();
        let json = serde_json::to_string(&snap).expect("snapshot serialises");
        let back: MetricsSnapshot = serde_json::from_str(&json).expect("snapshot deserialises");
        assert_eq!(snap, back);
    }
}
