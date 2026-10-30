//! The Linux capturer is a ScreenCast-portal + PipeWire pipeline, held
//! open for the process lifetime behind the same stateless per-call
//! contract. The Windows GDI path can grab any screen rectangle on
//! demand with no prior consent; Wayland cannot, so the first capture
//! lazily opens a consented monitor stream (reusing a persisted restore
//! token on later launches, so consent is asked at most once) and keeps
//! a background GStreamer pipeline pulling the latest full-monitor frame
//! into a shared slot. Each `capture_bgra` then crops that frame to the
//! requested screen rectangle, translating global screen coordinates to
//! frame-local ones through the stream's monitor position. A capture
//! before the stream has produced its first frame, or a rectangle
//! outside the captured monitor, returns None, exactly as the Windows
//! path returns None on a GDI failure.
//!
//! The restore token lives under the app data dir; the module reads its
//! location from `EO_CAPTURE_TOKEN_PATH` (set by the shell at startup)
//! and skips persistence when unset, so a headless test never writes to
//! a real config location.
//!
//! Gated behind the `linux-capture` feature: the GStreamer stack links
//! system libraries the backend members must not require, so the default
//! backend build stands the capturer down (returning `None`, exactly as
//! an unsupported host does) and only the shell enables the real path.

use std::os::unix::fs::OpenOptionsExt as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use ashpd::desktop::PersistMode;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;

/// The most recent full-monitor frame plus where that monitor sits in
/// the global screen space, so a global rect can be cropped out of it.
struct Frame {
    bgra: Vec<u8>,
    width: usize,
    height: usize,
    generation: u64,
    /// Monitor origin in global screen coordinates (the portal
    /// stream position).
    origin_x: i64,
    origin_y: i64,
}

struct Engine {
    latest: Arc<Mutex<Option<Frame>>>,
    healthy: AtomicBool,
    // Kept alive for the process: dropping the pipeline tears down
    // the PipeWire stream, and dropping the session revokes the
    // portal grant.
    _pipeline: gstreamer::Pipeline,
}

static ENGINE: Mutex<Option<Arc<Engine>>> = Mutex::new(None);

/// Take one of this module's locks, ignoring poisoning.
///
/// A panic elsewhere in the process poisons these mutexes, and the
/// default `expect` then turns that one panic into a permanent one:
/// every later capture panics on the lock instead of on the original
/// fault, and screen reading stays dead until the app restarts. What
/// they guard is a cached handle and a frame slot, neither of which a
/// panic can leave half-written into an invalid state, so the guard is
/// safe to take back.
fn guard<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The engine slot: built lazily on first use and kept for the
/// process, but a FAILED start does not latch. A cold-start portal
/// timeout or a declined consent leaves the slot empty, and the next
/// user-invoked capture simply tries again.
fn engine() -> Option<Arc<Engine>> {
    let mut slot = guard(&ENGINE);
    if slot
        .as_ref()
        .is_some_and(|engine| !engine.healthy.load(Ordering::Acquire))
    {
        *slot = None;
    }
    if slot.is_none() {
        match start_engine() {
            Ok(engine) => *slot = Some(engine),
            Err(error) => {
                tracing::warn!(target: "eo::capture", %error, "screen capture engine unavailable");
                return None;
            }
        }
    }
    slot.clone()
}

fn token_path() -> Option<std::path::PathBuf> {
    std::env::var_os("EO_CAPTURE_TOKEN_PATH").map(std::path::PathBuf::from)
}

/// ashpd's zbus connection starts its socket driver on the Tokio runtime
/// that creates it. A temporary current-thread runtime stops driving that
/// connection as soon as `block_on` returns, which can revoke the portal
/// stream before GStreamer receives its first frame. One dedicated worker
/// keeps the D-Bus peer and its portal grant alive for the process while
/// keeping this synchronous capture seam independent of the caller's
/// runtime context.
fn portal_runtime() -> Result<&'static tokio::runtime::Runtime, Box<dyn std::error::Error>> {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    if let Some(runtime) = RUNTIME.get() {
        return Ok(runtime);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("eo-capture-portal")
        .enable_all()
        .build()?;
    let _ = RUNTIME.set(runtime);
    RUNTIME
        .get()
        .ok_or_else(|| std::io::Error::other("capture portal runtime failed to initialise").into())
}

/// Open the portal stream (blocking on its process-lifetime runtime) and return
/// the PipeWire node id, the portal's PipeWire remote fd, and the
/// monitor origin. The fd matters: the screencast node lives on the
/// portal's own PipeWire remote and is access-restricted to it, so a
/// pipeline that connects to the session daemon directly (node path
/// alone) can negotiate a stream that never produces a frame.
fn open_portal() -> Result<(u32, std::os::fd::OwnedFd, i64, i64), Box<dyn std::error::Error>> {
    let runtime = portal_runtime()?;
    runtime.block_on(async {
        let saved = token_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());

        let proxy = Screencast::new().await?;
        let session = proxy.create_session().await?;
        proxy
            .select_sources(
                &session,
                CursorMode::Hidden,
                SourceType::Monitor.into(),
                false,
                saved.as_deref(),
                PersistMode::ExplicitlyRevoked,
            )
            .await?;
        let response = proxy.start(&session, None).await?.response()?;

        if let Some(token) = response.restore_token() {
            if let Some(path) = token_path() {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                // Owner-only mode: the token is a capability at rest
                // (it grants silent re-acquisition of the consented
                // capture stream to a process presenting this app's
                // identity), so it must not be readable by other users.
                let _ = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(path)
                    .and_then(|mut file| {
                        use std::io::Write as _;
                        file.write_all(token.as_bytes())
                    });
            }
        }

        let stream = response
            .streams()
            .first()
            .ok_or("portal returned no stream")?
            .clone();
        let (ox, oy) = stream.position().unwrap_or((0, 0));
        let remote_fd: std::os::fd::OwnedFd = proxy.open_pipe_wire_remote(&session).await?;
        Ok::<_, Box<dyn std::error::Error>>((
            stream.pipe_wire_node_id(),
            remote_fd,
            i64::from(ox),
            i64::from(oy),
        ))
    })
}

fn start_engine() -> Result<Arc<Engine>, Box<dyn std::error::Error>> {
    gstreamer::init()?;
    let (node_id, remote_fd, origin_x, origin_y) = open_portal()?;
    tracing::info!(target: "eo::capture", node_id, "screen-cast portal stream acquired");

    let latest: Arc<Mutex<Option<Frame>>> = Arc::new(Mutex::new(None));
    let pipeline = gstreamer::Pipeline::new();
    // The source rides the portal's own PipeWire remote (its fd),
    // where the consented node is actually reachable; the fd is
    // deliberately leaked to the process lifetime alongside the
    // session grant it belongs to.
    let src = gstreamer::ElementFactory::make("pipewiresrc")
        .property("fd", {
            use std::os::fd::IntoRawFd as _;
            remote_fd.into_raw_fd()
        })
        .property("path", node_id.to_string())
        // Compositor frame delivery is damage-driven: a static (or
        // direct-scanout fullscreen) monitor can produce nothing for
        // seconds. Keepalive re-pushes the last frame on a timer, so
        // the first capture and quiet-screen scans never starve.
        .property("keepalive-time", 500i32)
        // A revoked portal grant must surface through the pipeline bus;
        // the default silently leaves the source connected but frameless,
        // which is indistinguishable from slow negotiation at this seam.
        .property_from_str("on-disconnect", "error")
        .build()?;
    let convert = gstreamer::ElementFactory::make("videoconvert").build()?;
    let caps = gstreamer::Caps::builder("video/x-raw")
        .field("format", "BGRx")
        .build();
    let appsink = AppSink::builder()
        .caps(&caps)
        .max_buffers(1)
        .drop(true)
        .build();

    pipeline.add_many([&src, &convert, appsink.upcast_ref()])?;
    gstreamer::Element::link_many([&src, &convert, appsink.upcast_ref()])?;

    let sink_slot = latest.clone();
    appsink.set_callbacks(
        gstreamer_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gstreamer::FlowError::Eos)?;
                let caps = sample.caps().ok_or(gstreamer::FlowError::Error)?;
                let info = gstreamer_video::VideoInfo::from_caps(caps)
                    .map_err(|_| gstreamer::FlowError::Error)?;
                let buffer = sample.buffer().ok_or(gstreamer::FlowError::Error)?;
                let map = buffer
                    .map_readable()
                    .map_err(|_| gstreamer::FlowError::Error)?;
                let width = info.width() as usize;
                let height = info.height() as usize;
                // GStreamer rows are stride-padded; copy tight BGRA.
                let stride = info.stride()[0] as usize;
                let mut bgra = Vec::with_capacity(width * height * 4);
                for row in 0..height {
                    let start = row * stride;
                    bgra.extend_from_slice(&map[start..start + width * 4]);
                }
                let mut slot = guard(&sink_slot);
                let generation = slot
                    .as_ref()
                    .map_or(1, |frame| frame.generation.saturating_add(1));
                *slot = Some(Frame {
                    bgra,
                    width,
                    height,
                    generation,
                    origin_x,
                    origin_y,
                });
                Ok(gstreamer::FlowSuccess::Ok)
            })
            .build(),
    );

    pipeline.set_state(gstreamer::State::Playing)?;

    // Wait for the first frame so the initial capture does not race
    // the stream startup. Cold portal + PipeWire negotiation can take
    // several seconds; the bound exists to fail rather than hang, and
    // the pipeline bus is drained while waiting so a negotiation
    // failure surfaces as its actual error, never a blind timeout.
    let bus = pipeline.bus().ok_or("pipeline has no bus")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while guard(&latest).is_none() {
        while let Some(message) = bus.pop() {
            use gstreamer::MessageView;
            match message.view() {
                MessageView::Error(error) => {
                    let detail = format!(
                        "pipeline error from {:?}: {} ({:?})",
                        message.src().map(|src| src.path_string()),
                        error.error(),
                        error.debug()
                    );
                    let _ = pipeline.set_state(gstreamer::State::Null);
                    return Err(detail.into());
                }
                MessageView::Warning(warning) => {
                    tracing::warn!(
                        target: "eo::capture",
                        source = ?message.src().map(|src| src.path_string()),
                        warning = %warning.error(),
                        detail = ?warning.debug(),
                        "capture pipeline warning"
                    );
                }
                _ => {}
            }
        }
        if Instant::now() > deadline {
            let _ = pipeline.set_state(gstreamer::State::Null);
            return Err("capture stream produced no frame within 10s".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let generation = latest
        .lock()
        .expect("frame slot")
        .as_ref()
        .map(|frame| frame.generation)
        .unwrap_or_default();
    tracing::info!(target: "eo::capture", generation, "capture stream produced its first frame");

    let engine = Arc::new(Engine {
        latest,
        healthy: AtomicBool::new(true),
        _pipeline: pipeline,
    });
    monitor_pipeline(engine.clone(), bus);
    Ok(engine)
}

/// Keep observing the pipeline after its first frame. A revoked portal
/// grant or downstream failure must evict the cached engine and frame so
/// the next user scan opens a fresh portal stream instead of cropping
/// stale pixels indefinitely.
fn monitor_pipeline(engine: Arc<Engine>, bus: gstreamer::Bus) {
    std::thread::Builder::new()
        .name("eo-capture-bus".to_string())
        .spawn(move || {
            for message in bus.iter_timed(gstreamer::ClockTime::NONE) {
                use gstreamer::MessageView;
                let reason = match message.view() {
                    MessageView::Error(error) => Some(format!(
                        "pipeline error from {:?}: {} ({:?})",
                        message.src().map(|src| src.path_string()),
                        error.error(),
                        error.debug()
                    )),
                    MessageView::Eos(..) => Some("capture pipeline reached end of stream".into()),
                    MessageView::Warning(warning) => {
                        tracing::warn!(
                            target: "eo::capture",
                            source = ?message.src().map(|src| src.path_string()),
                            warning = %warning.error(),
                            detail = ?warning.debug(),
                            "capture pipeline warning"
                        );
                        None
                    }
                    _ => None,
                };
                if let Some(reason) = reason {
                    invalidate_engine(&engine, &reason);
                    break;
                }
            }
        })
        .expect("capture bus monitor thread");
}

fn invalidate_engine(engine: &Arc<Engine>, reason: &str) {
    tracing::warn!(target: "eo::capture", %reason, "screen capture engine invalidated");
    engine.healthy.store(false, Ordering::Release);
    *guard(&engine.latest) = None;
    let _ = engine._pipeline.set_state(gstreamer::State::Null);
    let mut slot = guard(&ENGINE);
    if slot
        .as_ref()
        .is_some_and(|cached| Arc::ptr_eq(cached, engine))
    {
        *slot = None;
    }
}

pub fn capture_bgra(x: i64, y: i64, w: i64, h: i64) -> Option<Vec<u8>> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let engine = engine()?;
    if !engine.healthy.load(Ordering::Acquire) {
        return None;
    }
    let frame_slot = guard(&engine.latest);
    let frame = frame_slot.as_ref()?;

    // Translate the global rectangle into frame-local coordinates.
    let local_x = x - frame.origin_x;
    let local_y = y - frame.origin_y;
    if local_x < 0
        || local_y < 0
        || (local_x + w) as usize > frame.width
        || (local_y + h) as usize > frame.height
    {
        tracing::warn!(
            target: "eo::capture",
            rect = ?(x, y, w, h),
            frame = ?(frame.width, frame.height),
            origin = ?(frame.origin_x, frame.origin_y),
            "capture rectangle falls outside the captured monitor frame"
        );
        return None;
    }

    let (w, h) = (w as usize, h as usize);
    let (local_x, local_y) = (local_x as usize, local_y as usize);
    let mut out = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let start = ((local_y + row) * frame.width + local_x) * 4;
        out.extend_from_slice(&frame.bgra[start..start + w * 4]);
    }
    Some(out)
}

pub fn frame_generation() -> Option<u64> {
    let engine = engine()?;
    let generation = engine
        .latest
        .lock()
        .expect("frame slot")
        .as_ref()
        .map(|frame| frame.generation);
    generation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidation_clears_the_frame_and_cached_engine() {
        gstreamer::init().expect("gstreamer");
        let latest = Arc::new(Mutex::new(Some(Frame {
            bgra: vec![0; 4],
            width: 1,
            height: 1,
            generation: 1,
            origin_x: 0,
            origin_y: 0,
        })));
        let engine = Arc::new(Engine {
            latest: latest.clone(),
            healthy: AtomicBool::new(true),
            _pipeline: gstreamer::Pipeline::new(),
        });
        *guard(&ENGINE) = Some(engine.clone());

        invalidate_engine(&engine, "test failure");

        assert!(!engine.healthy.load(Ordering::Acquire));
        assert!(guard(&latest).is_none());
        assert!(guard(&ENGINE).is_none());
    }

    #[test]
    fn portal_runtime_drives_spawned_work_after_block_on_returns() {
        let runtime = portal_runtime().expect("portal runtime");
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (completed_tx, completed_rx) = std::sync::mpsc::channel();

        runtime.block_on(async move {
            tokio::spawn(async move {
                let _ = release_rx.await;
                let _ = completed_tx.send(());
            });
        });

        release_tx.send(()).expect("release portal task");
        completed_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("portal runtime stopped driving its spawned tasks");
    }
}
