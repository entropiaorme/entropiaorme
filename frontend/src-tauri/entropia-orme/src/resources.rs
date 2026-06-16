//! Periodic process-resource sampling for the drift signals.
//!
//! A background thread samples the process's resident-set size and OS handle
//! count on a fixed cadence, records them into the in-process metrics registry
//! (so the hidden metrics page shows them live), and emits each sample as a
//! structured `tracing` event (so the rolling log file carries the
//! resource-drift series even on a shipped, non-dev build). The two are the
//! monotonic-growth signals a leak shows up in.
//!
//! Sampling is best-effort and never on a hot path: a detached thread that
//! sleeps between samples, so it cannot stall the producers or the HTTP
//! surface. Off Windows there is no portable resident-set/handle source wired
//! (the app ships Windows-only), so the sampler simply records nothing.

use std::time::Duration;

/// How often the resource sampler takes a reading. Frequent enough to resolve a
/// slow leak over a session, infrequent enough to be free.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);

/// Spawn the detached resource-sampling thread. Best-effort: if the thread
/// cannot be spawned the drift gauges simply stay unset (the page renders a
/// dash), which never affects the rest of the app.
pub fn spawn_resource_sampler() {
    let _ = std::thread::Builder::new()
        .name("eo-resource-sampler".to_string())
        .spawn(|| loop {
            if let Some((rss_bytes, handle_count)) = sample_process_resources() {
                let metrics = eo_wire::metrics::metrics();
                metrics.set_rss_bytes(rss_bytes);
                metrics.set_handle_count(handle_count);
                tracing::info!(
                    target: "eo::resource",
                    rss_bytes,
                    handle_count,
                    "resource sample"
                );
            }
            std::thread::sleep(SAMPLE_INTERVAL);
        });
}

/// Sample `(resident_set_bytes, handle_count)` for the current process.
#[cfg(windows)]
fn sample_process_resources() -> Option<(u64, u64)> {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};

    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no close;
    // the two queries only read process counters into stack-owned outputs sized
    // by `cb`. No pointer outlives this scope.
    unsafe {
        let process = GetCurrentProcess();
        let mut counters = PROCESS_MEMORY_COUNTERS::default();
        let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        GetProcessMemoryInfo(process, &mut counters, cb).ok()?;
        let mut handle_count: u32 = 0;
        let handles = match GetProcessHandleCount(process, &mut handle_count) {
            Ok(()) => u64::from(handle_count),
            Err(_) => 0,
        };
        Some((counters.WorkingSetSize as u64, handles))
    }
}

/// No portable resident-set/handle source off Windows (the app ships
/// Windows-only); the sampler records nothing there.
#[cfg(not(windows))]
fn sample_process_resources() -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn sampling_reads_a_plausible_resident_set_and_handle_count() {
        let (rss, handles) =
            sample_process_resources().expect("the Windows process counters are readable");
        assert!(rss > 0, "the test process has a non-zero resident set");
        assert!(handles > 0, "the test process holds at least one OS handle");
    }
}
