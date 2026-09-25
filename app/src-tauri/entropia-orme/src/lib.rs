#[cfg(test)]
mod command_acl;
mod commands;
mod composition;
mod crash;
mod resources;
mod substrate;
mod telemetry;
mod updater;

#[cfg(feature = "e2e-stub")]
mod e2e_stub;

use std::sync::Mutex;

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

/// Whether `(x, y)` lies on a live monitor with enough margin that the
/// window's grab area is reachable. The X screen (and the Windows virtual
/// desktop) is the bounding box of all monitors, so an asymmetric layout
/// contains dead space no monitor covers; a window placed there renders
/// nowhere while every geometry API happily reports it. A stored or
/// default position is applied only when this holds.
fn position_on_a_monitor(window: &tauri::WebviewWindow, x: i32, y: i32) -> bool {
    const GRAB_MARGIN: i32 = 40;
    let Ok(monitors) = window.available_monitors() else {
        return false;
    };
    monitors.iter().any(|monitor| {
        let position = monitor.position();
        let size = monitor.size();
        x >= position.x
            && x <= position.x + size.width as i32 - GRAB_MARGIN
            && y >= position.y
            && y <= position.y + size.height as i32 - GRAB_MARGIN
    })
}

/// A safe on-screen anchor: `offset` into the primary monitor (or the
/// first one when no primary is reported), never the raw screen origin,
/// which on a multi-monitor layout can sit in dead space.
fn monitor_anchor(
    window: &tauri::WebviewWindow,
    offset: (i32, i32),
) -> tauri::PhysicalPosition<i32> {
    let monitor = window.primary_monitor().ok().flatten().or_else(|| {
        window
            .available_monitors()
            .ok()
            .and_then(|m| m.into_iter().next())
    });
    let base = monitor
        .map(|m| *m.position())
        .unwrap_or(tauri::PhysicalPosition::new(0, 0));
    tauri::PhysicalPosition::new(base.x + offset.0, base.y + offset.1)
}

#[tauri::command]
async fn toggle_overlay(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            // Apply the persisted position on EVERY show, validated against
            // the live monitor layout. The overlay webview also restores it
            // once at boot, but that happens while the window is hidden and
            // an X11 window manager re-places a window at map time, so the
            // boot-time restore does not survive to the first show there;
            // the show path is the only reliable place. An off-monitor or
            // absent stored position falls back to a primary-monitor anchor
            // rather than the screen origin (dead space on some layouts).
            let stored = match commands::facade(&app) {
                Ok(facade) => facade.settings_overlay_position().await.ok(),
                Err(_) => None,
            };
            let position = stored
                .and_then(|p| p.x.0.zip(p.y.0))
                .map(|(x, y)| tauri::PhysicalPosition::new(x as i32, y as i32))
                .filter(|p| position_on_a_monitor(&window, p.x, p.y))
                .unwrap_or_else(|| monitor_anchor(&window, (60, 60)));
            let _ = window.set_position(position);
            let _ = window.show();
            // The overlay is a pre-spawned hidden window shown without focus, so
            // no focus/visibility event reaches its webview on show. Signal the
            // show explicitly so it can re-read config/runtime state that no
            // backend event announces (otherwise it stays stale until restart).
            let _ = app.emit("overlay-shown", ());
        }
    }
}

#[tauri::command]
fn toggle_cartography_overlay(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("cartography-overlay") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let position = monitor_anchor(&window, (60, 150));
            let _ = window.set_position(position);
            let _ = window.show();
        }
    }
}

#[tauri::command]
async fn show_navigation_overlays(app: tauri::AppHandle) {
    let geometry = match commands::facade(&app) {
        Ok(facade) => facade.radar_geometry().await.ok().and_then(|value| value.0),
        Err(_) => None,
    };
    if let Some(radar) = app.get_webview_window("radar-guidance") {
        if let Some(geometry) = geometry {
            let diameter = (geometry.radius_px * 2.0).round().max(16.0) as u32;
            let x = (geometry.centre_x as f64 - geometry.radius_px).round() as i32;
            let y = (geometry.centre_y as f64 - geometry.radius_px).round() as i32;
            let _ = radar.set_size(tauri::PhysicalSize::new(diameter, diameter));
            let _ = radar.set_position(tauri::PhysicalPosition::new(x, y));
            let _ = radar.show();
            // GTK does not allocate the hidden pre-spawned window's native
            // surface until its first map. Tao's Linux hit-test path expects
            // that surface to exist, so click-through must be enabled only
            // after show maps it; doing this while hidden panics inside Tao.
            let _ = radar.set_ignore_cursor_events(true);
        }
    }
    if let Some(hud) = app.get_webview_window("navigation-hud") {
        let position = geometry
            .map(|geometry| {
                tauri::PhysicalPosition::new(
                    (geometry.centre_x as f64 + geometry.radius_px + 12.0).round() as i32,
                    (geometry.centre_y as f64 - geometry.radius_px).round() as i32,
                )
            })
            .filter(|position| position_on_a_monitor(&hud, position.x, position.y))
            .unwrap_or_else(|| monitor_anchor(&hud, (60, 250)));
        let _ = hud.set_position(position);
        let _ = hud.show();
    }
}

#[tauri::command]
fn hide_navigation_overlays(app: tauri::AppHandle) {
    for label in ["navigation-hud", "radar-guidance"] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.hide();
        }
    }
}

/// Hand route-area selection from the floating HUD to the main Maps surface.
/// Window orchestration stays in the shell; the event carries only transient
/// UI context and does not enter the domain-event spine.
#[tauri::command(rename_all = "snake_case")]
fn begin_navigation_area_selection(
    app: tauri::AppHandle,
    request_id: u32,
    planet: String,
    map_view_id: Option<i64>,
) -> Result<(), String> {
    let planet = planet.trim();
    if request_id == 0 || planet.is_empty() || planet.len() > 80 {
        return Err("invalid route-area selection request".into());
    }
    app.emit_to(
        "main",
        "navigation-area-selection-requested",
        serde_json::json!({
            "requestId": request_id,
            "planet": planet,
            "mapViewId": map_view_id,
        }),
    )
    .map_err(|error| error.to_string())?;
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
    Ok(())
}

#[tauri::command]
fn show_scan_overlay(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("scan-overlay") {
        // Position near a monitor's top-left so the overlay never collides
        // with a bottom-right docked in-game skills/professions panel. The
        // anchor is monitor-relative, not the raw screen origin, which on a
        // multi-monitor layout can lie outside every panel.
        let _ = window.set_position(monitor_anchor(&window, (40, 40)));
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// The capture overlay's window label, shared with the command that decides
/// whether a read came from it.
pub const SALE_CAPTURE_OVERLAY: &str = "overlay-sale-capture";

/// One-shot authority granted only by the main window opening the capture
/// overlay. The floating renderer can request a normal listing read once; it
/// cannot mint or retain screen-capture authority for itself.
#[derive(Default)]
struct SaleCaptureAuthority(Mutex<bool>);

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayCaptureReply {
    message: String,
    failed: bool,
}

/// The sale-window capture button, put where a fullscreen game leaves it
/// reachable. It is the main window's button in another place and nothing
/// more: the values it reads are reviewed back in the form, not here.
#[tauri::command]
fn show_sale_capture_overlay(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    authority: tauri::State<'_, SaleCaptureAuthority>,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("sale capture can only be opened from the main window".into());
    }
    let research_active = commands::facade(&app)
        .map(|api| api.auction_fee_research_active_for_shell())
        .unwrap_or(false);
    *authority.0.lock().expect("sale capture authority") = !research_active;
    if let Some(window) = app.get_webview_window(SALE_CAPTURE_OVERLAY) {
        // Top-left of the monitor, clear of a bottom-right docked sale
        // window, which is the one part of the screen it must never cover.
        let _ = window.set_position(monitor_anchor(&window, (40, 40)));
        let _ = window.show();
        let _ = window.set_focus();
    }
    Ok(())
}

#[tauri::command]
fn hide_sale_capture_overlay(
    app: tauri::AppHandle,
    authority: tauri::State<'_, SaleCaptureAuthority>,
) {
    *authority.0.lock().expect("sale capture authority") = false;
    if let Ok(api) = commands::facade(&app) {
        if api.auction_fee_research_active_for_shell() {
            api.stop_auction_fee_research_for_shell();
        }
    }
    if let Some(window) = app.get_webview_window(SALE_CAPTURE_OVERLAY) {
        let _ = window.hide();
    }
}

/// Consume the normal listing-read authority and return status only. The
/// complete typed read waits in the facade for the main Inventory form.
#[tauri::command]
async fn capture_sale_from_overlay(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    authority: tauri::State<'_, SaleCaptureAuthority>,
) -> Result<OverlayCaptureReply, String> {
    if window.label() != SALE_CAPTURE_OVERLAY {
        return Err("sale capture is available only from its overlay".into());
    }
    let allowed = {
        let mut armed = authority
            .0
            .lock()
            .map_err(|_| "capture authority unavailable")?;
        std::mem::replace(&mut *armed, false)
    };
    if !allowed {
        return Err("sale capture is not armed; reopen it from Inventory".into());
    }
    let api = commands::facade(&app).map_err(|error| error.to_string())?;
    let read = tokio::task::spawn_blocking(move || api.inventory_sale_window_capture())
        .await
        .map_err(|_| "sale capture task failed".to_string())?
        .map_err(|error| error.to_string())?;
    let error = read.error.0;
    Ok(OverlayCaptureReply {
        failed: error.is_some(),
        message: error.unwrap_or_else(|| "Captured. Check the main window.".into()),
    })
}

#[tauri::command]
fn hide_scan_overlay(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("scan-overlay") {
        let _ = window.hide();
    }
}

/// The manual-scan capture preview PNG for `page`, base64-encoded for an
/// `<img>` `data:` URL. The facade returns raw image bytes, which cannot ride
/// the typed-DTO command surface (the bindings are JSON), so this stays a
/// bespoke command outside the manifest, base64-encoding the bytes here.
#[tauri::command]
async fn capture_png(app: tauri::AppHandle, page: u32) -> Result<String, String> {
    use base64::Engine as _;
    let facade = commands::facade(&app).map_err(|error| error.to_string())?;
    let bytes = facade
        .scan_capture_png(i64::from(page))
        .map_err(|error| error.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

/// A bundled planet map's raster, base64-encoded for an `<img>` `data:` URL
/// (the MIME type rides the typed `planet_maps_list` read). The facade
/// returns raw image bytes, which cannot ride the typed-DTO command surface
/// (the bindings are JSON), so this stays a bespoke command outside the
/// manifest, base64-encoding the bytes here.
#[tauri::command]
async fn planet_map_image(app: tauri::AppHandle, planet: String) -> Result<String, String> {
    use base64::Engine as _;
    let facade = commands::facade(&app).map_err(|error| error.to_string())?;
    let bytes = facade
        .planet_map_image(&planet)
        .map_err(|error| error.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

// Holds the substrate's live producer spine so the Tauri exit seam can
// stop it deterministically. The substrate task composes the producers
// inside its own async context (after the database opens) and hands the
// spine here; `RunEvent::Exit` then stops the chat-log tail thread and
// ends any open session before the process tears down. There is no
// graceful-shutdown signal into the substrate's compose path, so this exit
// seam is the producer teardown path.
struct Producers(Mutex<Option<composition::ProducerState>>);

// Holds the manual-scan input listener and the scan state machine so the exit
// seam tears them down: the spacebar listener detaches its share of the shared
// OS keyboard hook (the hotbar listener detaches the other share via
// `ProducerState::stop`), and the scan resets any in-flight capture state.
// Both are the same `Arc`s the facade's scan family serves over.
struct ScanInput {
    spacebar: std::sync::Arc<composition::SpacebarCaptureListener>,
    skill_scan: std::sync::Arc<composition::SkillScanManual>,
    coord_confirm: std::sync::Arc<eo_services::coord_capture::CoordConfirmListener>,
    radar_confirm: std::sync::Arc<eo_services::coord_capture::CoordConfirmListener>,
}

// Holds the composed database handle so the exit seam can run the
// once-per-lifecycle `PRAGMA optimize` at shutdown. The composition task
// hands it here once the database opens; the typed-command facade holds its
// own clone for serving.
struct ShutdownDb(eo_services::db::Db);

#[cfg(windows)]
struct RuntimeWindowIcons(Mutex<Vec<isize>>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Force the GTK/webview stack onto X11 (XWayland) on Linux before any
    // GTK initialisation. Under a native Wayland backend a client cannot
    // position its own windows or hold always-on-top, which the overlay
    // and its satellite popups depend on; XWayland restores both with no
    // behaviour change to the rest of the app. An explicit operator value
    // wins (so a user who has a reason to run native Wayland can), but the
    // default is the backend the overlay UX actually works on. Must run
    // before `gtk::init` (reached via the Tauri builder), hence first.
    #[cfg(target_os = "linux")]
    if std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "x11");
    }

    // Point the screen-capture seam's restore-token store at the app data
    // dir, so the one-time ScreenCast consent persists across launches and
    // later captures acquire the stream silently. Linux-only (the seam
    // reads this only there); harmless to set elsewhere.
    #[cfg(target_os = "linux")]
    if std::env::var_os("EO_CAPTURE_TOKEN_PATH").is_none() {
        let token = composition::data_dir().join("capture-restore-token");
        std::env::set_var("EO_CAPTURE_TOKEN_PATH", token);
    }

    // Install the process-wide tracing subscriber first, before anything
    // else runs, so every diagnostic and every instrumented seam is captured
    // from the first instant. The guard is held for the whole process so the
    // rolling log appender flushes at exit.
    let _telemetry = telemetry::init();

    // Install the default-off, opt-in crash reporter's panic hook. By default
    // it adds nothing to the standard panic behaviour; only when the user has
    // opted in does a panic write a PII-scrubbed, local-only report.
    crash::install_panic_hook(composition::data_dir());

    // Start the periodic resource sampler feeding the drift gauges (the metrics
    // page reads them live; each sample is also logged so the rolling file
    // carries the resource-drift series for long-running-session leak detection).
    resources::spawn_resource_sampler();

    let app = tauri::Builder::default()
        // The startup readiness record, managed on the builder so it exists
        // before any window can ask for it.
        .manage(substrate::SubstrateReadiness::default())
        .manage(SaleCaptureAuthority::default())
        // The shell plugin stays for its `open` API (external links route to
        // the OS browser via `$lib/utils/openExternal`); the sidecar/execute
        // usage was removed when the Python backend was decommissioned.
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            toggle_overlay,
            toggle_cartography_overlay,
            show_scan_overlay,
            show_sale_capture_overlay,
            hide_sale_capture_overlay,
            capture_sale_from_overlay,
            hide_scan_overlay,
            capture_png,
            planet_map_image,
            // The typed IPC commands (held in lock-step with the eo-api
            // manifest by the parity test in `commands`).
            commands::equipment_search,
            commands::equipment_library,
            commands::equipment_add,
            commands::equipment_update,
            commands::equipment_delete,
            commands::equipment_detail,
            commands::protection_overview,
            commands::protection_set_create,
            commands::protection_set_update,
            commands::protection_loadout_create,
            commands::protection_loadout_update,
            commands::protection_set_archive,
            commands::protection_loadout_archive,
            commands::protection_select,
            commands::protection_assign_session_loadout,
            commands::protection_pending_attribution,
            commands::protection_observation_confirm,
            commands::protection_repair_confirm,
            commands::protection_trade_terminal_scan,
            commands::character_calibration,
            commands::character_stats,
            commands::character_skills,
            commands::character_professions,
            commands::character_prospect_options,
            commands::character_prospect,
            commands::character_profession_optimizer,
            commands::character_path_optimizer,
            commands::character_hp_optimizer,
            commands::character_activity_recommender,
            commands::settings_get,
            commands::settings_overlay_position,
            commands::settings_set_overlay_position,
            commands::settings_update,
            commands::codex_species,
            commands::codex_species_ranks,
            commands::codex_recommend,
            commands::codex_meta_attributes,
            commands::codex_calibrate,
            commands::codex_claim,
            commands::codex_unclaim,
            commands::codex_meta_claim,
            commands::codex_mastery_options,
            commands::codex_mastery_claim,
            commands::codex_mastery_unclaim,
            commands::quests_list,
            commands::quest_get,
            commands::quest_create,
            commands::quest_update,
            commands::quest_delete,
            commands::quest_start,
            commands::quest_complete,
            commands::quest_hand_in_begin,
            commands::quest_hand_in_state,
            commands::quest_hand_in_wait,
            commands::quest_hand_in_cancel,
            commands::quest_hand_in_confirm,
            commands::quest_rewards_unresolved,
            commands::quest_reward_review,
            commands::quest_cancel,
            commands::quests_mobs,
            commands::quests_analytics,
            commands::quest_families_list,
            commands::quest_family_create,
            commands::quest_family_update,
            commands::quest_family_delete,
            commands::session_definitions_list,
            commands::session_definition_create,
            commands::session_definition_update,
            commands::session_definition_archive,
            commands::session_definition_restore,
            commands::tracking_definition_select,
            commands::analytics_overview,
            commands::analytics_hunting,
            commands::analytics_harvest,
            commands::analytics_hunting_activity,
            commands::activity_stock,
            commands::hunting_realised_markup,
            commands::harvest_realised_markup,
            commands::auction_listings,
            commands::auction_listing_create,
            commands::auction_listing_confirm,
            commands::auction_listing_expire,
            commands::stock_convert,
            commands::stock_private_sale,
            commands::stock_remove,
            commands::stock_shrapnel_convert,
            commands::activity_history,
            commands::auction_sale_revert,
            commands::auction_listing_undo,
            commands::stock_conversion_undo,
            commands::private_sale_undo,
            commands::stock_removal_undo,
            commands::ledger_list,
            commands::ledger_summary,
            commands::ledger_create,
            commands::ledger_delete,
            commands::ledger_presets_list,
            commands::ledger_preset_create,
            commands::ledger_preset_delete,
            commands::inventory_list,
            commands::inventory_create,
            commands::inventory_update,
            commands::inventory_delete,
            commands::inventory_sell,
            commands::inventory_sale_window_capture,
            commands::inventory_sale_window_take_capture,
            commands::inventory_draft_resolve,
            commands::inventory_equipment_listing_create,
            commands::inventory_equipment_trade,
            commands::market_paste_preview,
            commands::market_paste_commit,
            commands::market_unit_price_set,
            commands::market_overview,
            commands::market_contribution_batch,
            commands::market_auction_packet_threshold,
            commands::market_break_even,
            commands::market_mob_ranking,
            commands::market_harvest_markups,
            commands::market_hunt_markups,
            commands::market_item_history,
            commands::scan_status,
            commands::scan_start,
            commands::scan_capture,
            commands::scan_cancel,
            commands::scan_undo,
            commands::scan_process,
            commands::scan_accept,
            commands::scan_reject,
            commands::scan_pending,
            commands::scan_spacebar_capture,
            commands::dev_auction_fee_research_start,
            commands::dev_auction_fee_research_stop,
            commands::dev_auction_fee_research_status,
            commands::dev_auction_fee_research_capture,
            commands::dev_auction_fee_research_overlay_status,
            commands::tracking_sessions,
            commands::tracking_session_detail,
            commands::tracking_session_intervals,
            commands::tracking_manual_mob_suggestions,
            commands::tracking_snapshot,
            commands::tracking_start,
            commands::tracking_stop,
            commands::tracking_release_mob,
            commands::tracking_manual_mob_lock,
            commands::tracking_session_config,
            commands::tracking_activity_options,
            commands::tracking_activity_activate,
            commands::tracking_activity_deactivate,
            commands::tracking_rename_mob,
            commands::tracking_reassign_session,
            commands::tracking_restore_mob,
            commands::tracking_loot_item_activate,
            commands::tracking_loot_item_deactivate,
            commands::tracking_armour_cost,
            commands::tracking_repair_scan,
            commands::tracking_session_delete,
            commands::demo_analytics_overview,
            commands::demo_analytics_hunting,
            commands::demo_analytics_hunting_activity,
            commands::demo_analytics_harvest,
            commands::demo_ledger_list,
            commands::demo_ledger_summary,
            commands::demo_ledger_presets_list,
            commands::demo_inventory_list,
            commands::demo_tracking_sessions,
            commands::demo_tracking_session_detail,
            commands::demo_tracking_snapshot,
            commands::dev_metrics,
            commands::dev_crash_reporting,
            commands::dev_set_crash_reporting,
            commands::dev_compact_database,
            commands::dev_rebuild_projections,
            commands::planet_maps_list,
            commands::map_pins_list,
            commands::map_pins_viewport,
            commands::map_pin_nearby,
            commands::map_views_list,
            commands::map_view_create,
            commands::map_view_rename,
            commands::map_view_delete,
            commands::map_pin_create,
            commands::map_pin_update,
            commands::map_pin_delete,
            commands::map_pin_cooldown,
            commands::pin_configs_list,
            commands::pin_config_create,
            commands::pin_config_update,
            commands::pin_config_delete,
            commands::pin_config_reorder,
            commands::maps_calibration_start,
            commands::maps_calibration_cancel,
            commands::maps_calibration_status,
            commands::maps_scan_coordinates,
            commands::navigation_snapshot,
            commands::navigation_start,
            commands::navigation_update_position,
            commands::navigation_mark_visited,
            commands::navigation_skip,
            commands::navigation_resolve_harvest,
            commands::navigation_undo,
            commands::navigation_end,
            commands::radar_calibration_start,
            commands::radar_calibration_cancel,
            commands::radar_calibration_status,
            commands::radar_geometry,
            show_navigation_overlays,
            hide_navigation_overlays,
            begin_navigation_area_selection,
            updater::check_for_update,
            updater::download_update,
            updater::install_update,
            updater::get_update_channel,
            updater::set_update_channel,
            substrate::substrate_ready,
            substrate::restart_app
        ])
        .setup(|app| {
            // `app` is unused on a non-Windows debug build (the runtime icon
            // install is Windows-only); keep the binding alive there.
            let _ = &app;
            // Hold the downloaded-but-not-yet-installed update between the
            // updater's download and install commands (the deferred-install
            // split). Registered before any command can run.
            app.manage(updater::PendingUpdate::default());
            #[cfg(windows)]
            install_runtime_window_icons(app.handle());
            // Register a system-tray presence on Linux (a StatusNotifierItem
            // on the session bus). The overlay windows are undecorated and
            // skip the taskbar, so on a Linux desktop the tray is the app's
            // reliable handle for raising the main window or quitting.
            // Windows keeps its taskbar presence and builds no tray.
            #[cfg(target_os = "linux")]
            if let Err(error) = install_tray(app.handle()) {
                tracing::warn!(target: "eo::shell", %error, "system tray unavailable");
            }
            // The single pure-Rust binary: the frontend reaches the backend
            // through the in-process IPC command (no inbound socket) and every
            // route is served natively (the Python sidecar has been
            // decommissioned). Startup composes the native spine off
            // the setup path, publishes it to the IPC command when ready,
            // and settles the readiness record every window awaits.
            // Dev and release compose identically; the resource dir (the
            // bundled snapshot / model / demo assets) resolves only in the
            // installed build, dev falling back to the repository copies.
            compose_substrate(app.handle().clone(), app.path().resource_dir().ok());
            Ok(())
        })
        .on_window_event(|window, event| {
            // The tracking, scan, and cartography overlay windows are configured invisible
            // but still count toward Tauri's "exit when all windows close"
            // tally — closing main alone leaves them open and the app
            // keeps running headless. Treat main-window close as a
            // request to exit the whole app.
            if let WindowEvent::CloseRequested { .. } = event {
                if window.label() == "main" {
                    window.app_handle().exit(0);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            run_exit_teardown(app);
        }
    });
}

/// Wind the app's live faculties down deterministically before the process
/// ends. Driven from two paths: the Tauri `RunEvent::Exit` seam (a normal
/// quit), and the updater's `on_before_exit` hook (an updater install calls
/// `std::process::exit`, which bypasses the event loop, so that path must run
/// this itself). Idempotent: the producer handle is taken out under its lock,
/// so a second call is a no-op.
pub(crate) fn run_exit_teardown(app: &tauri::AppHandle) {
    #[cfg(windows)]
    destroy_runtime_window_icons(app);

    // Stop the producer spine: end any open session so its stop events
    // publish, then stop the chat-log tail thread. The substrate's compose
    // task has no shutdown signal, so this is where the producers wind down.
    if let Some(state) = app.try_state::<Producers>() {
        if let Ok(mut guard) = state.0.lock() {
            if let Some(producers) = guard.take() {
                producers.stop();
            }
        }
    }

    // Tear down the scan input listener and reset the scan: the spacebar
    // listener detaches its share of the shared OS hook.
    if let Some(state) = app.try_state::<ScanInput>() {
        state.spacebar.stop();
        state.coord_confirm.stop();
        state.radar_confirm.stop();
        state.skill_scan.shutdown();
    }

    // With the producers stopped (no writes in flight), refresh the database's
    // planner statistics via `PRAGMA optimize` before the connection closes: the
    // recommended once-per-lifecycle maintenance call, kept off the hot path by
    // running only here at exit.
    if let Some(db) = app.try_state::<ShutdownDb>() {
        if tauri::async_runtime::block_on(db.0.optimize_on_shutdown()) {
            tracing::info!(target: "eo::db", "ran PRAGMA optimize on shutdown");
        }
    }
}

/// Build the Linux system tray: an icon that registers as a
/// StatusNotifierItem on the session bus, with a menu to raise the main
/// window or quit. Left-clicking the icon also raises the main window.
/// Linux-only; the tray is the app's reliable desktop handle where the
/// overlay windows are undecorated and skip the taskbar.
#[cfg(target_os = "linux")]
fn install_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::{TrayIconBuilder, TrayIconEvent};

    let show = MenuItemBuilder::with_id("tray-show", "Show EntropiaOrme").build(app)?;
    let quit = MenuItemBuilder::with_id("tray-quit", "Quit").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&show, &quit]).build()?;

    let raise_main = |app: &tauri::AppHandle| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    };

    let mut builder = TrayIconBuilder::with_id("eo-tray")
        .tooltip("EntropiaOrme")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "tray-show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(move |tray, event| {
            if let TrayIconEvent::Click { .. } = event {
                raise_main(tray.app_handle());
            }
        });
    // Prefer the bundled window icon; the tray still registers without one.
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg(windows)]
fn install_runtime_window_icons(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        match windows_runtime_icons::set_for_main_window(&window) {
            Ok(handles) => {
                app.manage(RuntimeWindowIcons(Mutex::new(handles)));
            }
            Err(err) => tracing::warn!(target: "eo::icon", "runtime icon install failed: {err}"),
        }
    }
}

#[cfg(windows)]
fn destroy_runtime_window_icons(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<RuntimeWindowIcons>() {
        if let Ok(mut handles) = state.0.lock() {
            windows_runtime_icons::destroy(handles.drain(..));
        }
    }
}

/// Compose the native backend spine and publish it to the typed commands.
/// The single pure-Rust binary serves every backend call over a typed Tauri
/// command dispatched into the composed facade: there is no socket, no
/// sidecar, and no proxy. This composes the native service spine off the setup
/// path, and on success installs the services and publishes the facade to the
/// typed commands (see [`install_native_services`]).
///
/// Either way it then settles the startup readiness record exactly once
/// ([`substrate::SubstrateReadiness`]): the webview's typed transport holds
/// every facade command until that answer, so no command dispatches while the
/// facade is still composing. A declined composition settles as a failure
/// carrying its reason (an unopenable database, one below the supported
/// baseline, missing game data) and the backend does not come up for the
/// session. Composition and installation run in their own task so that even a
/// panic inside either settles the record rather than leaving every window
/// waiting (the facade is published last, so a panic before it leaves the
/// typed commands answering their not-ready contract, and `failed` is true).
fn compose_substrate(app: tauri::AppHandle, resource_dir: Option<std::path::PathBuf>) {
    tauri::async_runtime::spawn(async move {
        #[cfg(debug_assertions)]
        simulate_slow_start().await;
        let installing = {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                match composition::compose_native(resource_dir).await {
                    composition::Composition::Ready(composed) => {
                        install_native_services(&app, composed);
                        substrate::SubstrateOutcome::Ready
                    }
                    composition::Composition::Declined(decline) => {
                        tracing::error!(
                            target: "eo::substrate",
                            "native services did not compose; the backend is unavailable for this session"
                        );
                        decline.into()
                    }
                }
            })
        };
        let outcome = match installing.await {
            Ok(outcome) => outcome,
            Err(error) => {
                tracing::error!(
                    target: "eo::substrate",
                    %error,
                    "native composition stopped unexpectedly; the backend is unavailable for this session"
                );
                substrate::Decline::new(substrate::DeclineReason::Unexpected, error.to_string())
                    .into()
            }
        };
        app.state::<substrate::SubstrateReadiness>().settle(outcome);
    });
}

/// Debug builds only: hold startup back by `ENTROPIAORME_STARTUP_DELAY_MS`
/// milliseconds, so the slow-start path (the startup indicator and the pages'
/// loading placeholders) can be exercised on a machine that composes quickly.
/// Release builds compile this out and ignore the variable.
#[cfg(debug_assertions)]
async fn simulate_slow_start() {
    let delay = std::env::var("ENTROPIAORME_STARTUP_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    if let Some(ms) = delay {
        tracing::info!(target: "eo::substrate", ms, "simulating a slow start");
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
}

/// Install the composed services into the Tauri-managed exit seam, then publish
/// the facade to the typed commands (so they answer only once every service is
/// present). The caller settles the readiness record after this returns.
fn install_native_services(app: &tauri::AppHandle, composed: composition::Composed) {
    // Clones for the exit seam, taken before the originals move into the
    // installed bundle below: the spacebar listener detaches its share of
    // the shared OS hook and the scan resets in-flight state on close.
    let exit_spacebar = composed.spacebar_listener.clone();
    let exit_skill_scan = composed.skill_scan.clone();
    let exit_coord_confirm = composed.coord_confirm.clone();
    let exit_radar_confirm = composed.radar_confirm.clone();
    // The typed-command facade, published to its managed slot below.
    let composed_api = composed.api.clone();
    // The composed database handle, held so the exit seam can run the
    // once-per-lifecycle `PRAGMA optimize` on close.
    let shutdown_db = composed.db.clone();
    // Forward the producer spine's domain events onto the Tauri event bus, the
    // native replacement for the frontend's old EventSource relay. Subscribes
    // the producer spine's typed domain channel while the spine is still in
    // hand (it moves into managed state just below).
    spawn_domain_event_bridge(app, &composed.producers.domain_bus_handle());
    // Hand the producer spine to the exit seam so it stops the tail thread,
    // the hotbar listener, and ends any session on close.
    app.manage(Producers(Mutex::new(Some(composed.producers))));
    // No separate lifetime anchor for the OCR engine: the scan services
    // captured their own clones at compose time and are themselves held
    // for the app's lifetime, so the warmed engine (and its ONNX session)
    // lives exactly as long as its consumers; the ORT environment
    // self-releases at process exit.
    // Hand the scan input listener and the scan state machine to the exit
    // seam for deterministic teardown.
    app.manage(ScanInput {
        spacebar: exit_spacebar,
        skill_scan: exit_skill_scan,
        coord_confirm: exit_coord_confirm,
        radar_confirm: exit_radar_confirm,
    });
    // Hold the database handle for the exit-seam `PRAGMA optimize`.
    app.manage(ShutdownDb(shutdown_db));
    // Publish the composed facade to the typed commands LAST: until now the
    // typed commands answer their not-ready contract, so by the time any
    // request dispatches every native service is present (there is no
    // absent-service window to fall back from).
    app.manage(commands::ApiFacade(composed_api));
}

/// Map a dotted domain wire topic to its colon-form Tauri event name (Tauri
/// event names forbid dots). Mirrors the frontend's `toTauriEventName`.
fn domain_topic_to_tauri_event(topic: &str) -> String {
    topic.replace('.', ":")
}

/// Forward the producer spine's domain events onto the Tauri event bus: the
/// native replacement for the frontend's old `EventSource` relay. Subscribes
/// the producer spine's typed domain channel and re-emits each envelope on
/// the colon-form Tauri topic every window subscribes to
/// (`tracking:session:updated`, `scan:status:changed`). The webview sees the
/// identical envelope JSON the wire contract pins (`eo-wire`'s serde shape),
/// so the topic-aware consumers are unchanged. A lagging receiver skips
/// ahead to live events (drop-oldest, the same shedding the frame queues
/// applied); the channel closing ends the task with the spine. Initial state
/// is not this bridge's concern: every consumer attaches its listener and then
/// reads its snapshot itself, a read the typed transport holds until the
/// facade is ready.
fn spawn_domain_event_bridge(
    app: &tauri::AppHandle,
    domain_bus: &std::sync::Arc<eo_wire::bus::DomainBus>,
) {
    let mut receiver = domain_bus.subscribe();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let _ = app.emit(domain_topic_to_tauri_event(event.topic()).as_str(), &event);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        target: "eo::substrate",
                        skipped,
                        "domain-event bridge lagged; skipping to live events"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[cfg(windows)]
mod windows_runtime_icons {
    use tauri::WebviewWindow;
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{HWND, LPARAM, WPARAM},
            System::LibraryLoader::GetModuleHandleW,
            UI::{
                HiDpi::GetDpiForWindow,
                WindowsAndMessaging::{
                    DestroyIcon, LoadImageW, SendMessageW, HICON, ICON_BIG, ICON_SMALL,
                    ICON_SMALL2, IMAGE_ICON, LR_DEFAULTCOLOR, WM_SETICON,
                },
            },
        },
    };

    const APP_ICON_RESOURCE_ID: u16 = 32512;
    const USER_DEFAULT_SCREEN_DPI: u32 = 96;
    const ICON_SIZES: [i32; 8] = [16, 20, 24, 32, 48, 64, 128, 256];

    pub fn set_for_main_window(window: &WebviewWindow) -> Result<Vec<isize>, String> {
        let hwnd = window
            .hwnd()
            .map_err(|err| format!("failed to resolve main HWND: {err}"))?;
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let dpi = if dpi == 0 {
            USER_DEFAULT_SCREEN_DPI
        } else {
            dpi
        };

        // Tauri's default Windows path decodes only the first ICO entry and
        // sets ICON_SMALL. Load from the embedded icon group instead so the
        // taskbar gets a DPI-appropriate ICON_BIG handle.
        let small_icon_size = choose_icon_size(16, dpi);
        let taskbar_icon_size = choose_icon_size(24, dpi);

        let small_icon = load_icon(small_icon_size)?;
        let taskbar_icon = load_icon(taskbar_icon_size)?;

        set_window_icon(hwnd, ICON_SMALL, small_icon);
        set_window_icon(hwnd, ICON_SMALL2, small_icon);
        set_window_icon(hwnd, ICON_BIG, taskbar_icon);

        Ok(vec![small_icon.0 as isize, taskbar_icon.0 as isize])
    }

    pub fn destroy(handles: impl Iterator<Item = isize>) {
        for handle in handles {
            let _ = unsafe { DestroyIcon(HICON(handle as _)) };
        }
    }

    fn choose_icon_size(base_size: u32, dpi: u32) -> i32 {
        let desired = base_size
            .saturating_mul(dpi)
            .div_ceil(USER_DEFAULT_SCREEN_DPI) as i32;
        ICON_SIZES
            .iter()
            .copied()
            .find(|size| *size >= desired)
            .unwrap_or(256)
    }

    fn load_icon(size: i32) -> Result<HICON, String> {
        let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
            .map(Into::into)
            .ok();
        let resource = PCWSTR::from_raw(APP_ICON_RESOURCE_ID as usize as *const u16);
        let handle =
            unsafe { LoadImageW(module, resource, IMAGE_ICON, size, size, LR_DEFAULTCOLOR) }
                .map_err(|err| format!("failed to load {size}x{size} icon resource: {err}"))?;

        Ok(HICON(handle.0))
    }

    fn set_window_icon(hwnd: HWND, icon_type: u32, icon: HICON) {
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(icon_type as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{choose_icon_size, ICON_SIZES};

        #[test]
        fn icon_sizes_are_strictly_ascending() {
            // choose_icon_size's `find` returns the first entry >= desired,
            // which is only the *smallest sufficient* size if the table is
            // sorted; this pins that load-bearing ordering invariant.
            assert!(ICON_SIZES.windows(2).all(|pair| pair[0] < pair[1]));
        }

        #[test]
        fn standard_dpi_maps_to_exact_base_sizes() {
            assert_eq!(choose_icon_size(16, 96), 16);
            assert_eq!(choose_icon_size(24, 96), 24);
        }

        #[test]
        fn scaled_dpi_rounds_up_to_next_available_size() {
            // 150% scaling: 16 -> 24 (exact), 24 -> 36 -> next size up is 48.
            assert_eq!(choose_icon_size(16, 144), 24);
            assert_eq!(choose_icon_size(24, 144), 48);
            // 125% scaling: 16 -> 20 (exact table entry via div_ceil).
            assert_eq!(choose_icon_size(16, 120), 20);
        }

        #[test]
        fn oversized_demand_clamps_to_largest_icon() {
            assert_eq!(choose_icon_size(256, 480), 256);
            // Saturating multiply keeps absurd DPI values from overflowing.
            assert_eq!(choose_icon_size(u32::MAX, u32::MAX), 256);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::domain_topic_to_tauri_event;

    #[test]
    fn domain_topics_namespace_dots_to_colons_for_the_tauri_bus() {
        assert_eq!(
            domain_topic_to_tauri_event("tracking.session.updated"),
            "tracking:session:updated"
        );
        assert_eq!(
            domain_topic_to_tauri_event("scan.status.changed"),
            "scan:status:changed"
        );
    }

    #[tokio::test]
    async fn the_domain_bridge_receives_typed_envelopes_with_their_wire_shape() {
        // The bridge's emit payload is the typed envelope itself; its serde
        // value equals the pinned wire JSON, so the webview's topic-aware
        // consumers see the exact shape the old EventSource data field
        // carried.
        use eo_wire::domain_events::{
            DomainEvent, ScanPhase, ScanStatusChanged, ScanStatusChangedPayload,
            ScanStatusChangedTag,
        };
        let bus = eo_wire::bus::DomainBus::new(8);
        let mut receiver = bus.subscribe();
        let event = DomainEvent::ScanStatusChanged(ScanStatusChanged {
            topic: ScanStatusChangedTag,
            event_version: 1,
            occurred_at: "2024-12-31T21:20:00+00:00".into(),
            payload: ScanStatusChangedPayload {
                phase: ScanPhase::Capturing,
            },
        });
        bus.publish(event.clone());
        let received = receiver.recv().await.expect("the envelope arrives");
        assert_eq!(received, event);
        assert_eq!(
            domain_topic_to_tauri_event(received.topic()),
            "scan:status:changed"
        );
        assert_eq!(
            serde_json::to_value(&received).expect("envelopes serialise"),
            serde_json::from_str::<serde_json::Value>(&received.to_wire_json())
                .expect("the wire JSON parses"),
        );
    }

    /// The frontend reaches the backend only through the in-process IPC command,
    /// so the substrate binds no inbound listener (see `compose_substrate`) and
    /// the security policy grants no loopback origin: the CSP carries no
    /// `127.0.0.1`/`8421` in connect-src or img-src. The external news origin,
    /// the IPC scheme, and the base64 image `data:` source survive.
    #[test]
    fn the_security_policy_grants_no_loopback_origin() {
        let conf = include_str!("../tauri.conf.json");
        let after = conf
            .split("\"csp\":")
            .nth(1)
            .expect("the security CSP is configured");
        let csp = after
            .split('"')
            .nth(1)
            .expect("the CSP is a string literal");
        assert!(
            !csp.contains("127.0.0.1") && !csp.contains("8421"),
            "the CSP must grant no loopback origin once the frontend is IPC-only: {csp}"
        );
        assert!(
            csp.contains("https://entropiaorme.com"),
            "the external news origin must survive the loopback strip: {csp}"
        );
        let connect_src = csp
            .split(';')
            .find(|directive| directive.trim_start().starts_with("connect-src "))
            .expect("connect-src is configured");
        assert!(
            connect_src
                .split_whitespace()
                .any(|source| source == "https://market-data.entropiaorme.com"),
            "the market-data origin must stay pinned in connect-src: {csp}"
        );
        assert!(csp.contains("ipc:"), "the IPC scheme must remain: {csp}");
        assert!(
            csp.contains("img-src 'self' data:"),
            "img-src keeps data: for the base64 capture preview: {csp}"
        );
    }

    /// The bundle ships no Python sidecar. The packaging spec declares
    /// no `externalBin`, and the shell `execute`/sidecar capability is gone
    /// (only `open` survives, for external links), so the installed artefact
    /// carries the single native binary alone.
    #[test]
    fn the_bundle_declares_no_sidecar_binary() {
        let conf = include_str!("../tauri.conf.json");
        assert!(
            !conf.contains("externalBin"),
            "the packaging spec must declare no externalBin once the sidecar is decommissioned"
        );
        let capabilities = include_str!("../capabilities/default.json");
        assert!(
            !capabilities.contains("shell:allow-execute"),
            "the shell execute/sidecar capability must be gone: {capabilities}"
        );
        assert!(
            capabilities.contains("shell:allow-open"),
            "the shell open capability must survive for external links: {capabilities}"
        );
    }

    #[test]
    fn the_capture_overlay_has_a_dedicated_least_privilege_capability() {
        let default_capability = include_str!("../capabilities/default.json");
        let capture_capability = include_str!("../capabilities/sale-capture-overlay.json");
        assert!(
            !default_capability.contains("overlay-sale-capture"),
            "the capture webview must not inherit the main capability: {default_capability}"
        );
        assert!(capture_capability.contains("\"windows\": [\"overlay-sale-capture\"]"));
        assert!(capture_capability.contains("sale-capture-commands"));
        for forbidden in [
            "shell:allow-open",
            "core:window:allow-create",
            "core:webview:allow-create-webview-window",
            "trusted-commands",
            "store:allow-set",
            "store:allow-get",
            "store:allow-load",
            "core:path:allow-join",
            "core:path:allow-resolve-directory",
        ] {
            assert!(
                !capture_capability.contains(forbidden),
                "the capture capability must not grant {forbidden}: {capture_capability}"
            );
        }
        // A button that reads the screen and dismisses itself needs those two
        // commands, plus the startup readiness answer its typed transport
        // awaits (whether the backend is up and, if not, the closed reason;
        // the logged detail goes to the main window alone). Anything else
        // here would be a window on the whole ledger reachable from a surface
        // that floats over the game.
        assert_eq!(
            crate::command_acl::SALE_CAPTURE_COMMANDS,
            [
                "capture_sale_from_overlay",
                "dev_auction_fee_research_capture",
                "dev_auction_fee_research_overlay_status",
                "hide_sale_capture_overlay",
                "substrate_ready"
            ]
        );
        // Notably not the collect verb: the form takes the waiting read, the
        // overlay never needs to see one back.
        assert!(!crate::command_acl::SALE_CAPTURE_COMMANDS
            .contains(&"inventory_sale_window_take_capture"));
        assert!(
            !crate::command_acl::SALE_CAPTURE_COMMANDS.contains(&"inventory_sale_window_capture")
        );
        assert!(!crate::command_acl::SALE_CAPTURE_COMMANDS.contains(&"auction_listing_create"));
    }

    #[test]
    fn the_cartography_overlay_has_a_dedicated_least_privilege_capability() {
        let default_capability = include_str!("../capabilities/default.json");
        let cartography_capability = include_str!("../capabilities/cartography-overlay.json");
        assert!(
            !default_capability.contains("cartography-overlay"),
            "the cartography webview must not inherit the main capability: {default_capability}"
        );
        assert!(cartography_capability.contains("\"windows\": [\"cartography-overlay\"]"));
        assert!(cartography_capability.contains("cartography-commands"));
        for forbidden in [
            "shell:allow-open",
            "core:window:allow-create",
            "core:webview:allow-create-webview-window",
            "trusted-commands",
            // The palette is DB-backed now, not a preference blob: the overlay
            // reaches no store or path permission (store:allow-set would let a
            // compromised overlay write backend-consumed settings).
            "store:allow-set",
            "store:allow-get",
            "store:allow-load",
            "core:path:allow-join",
            "core:path:allow-resolve-directory",
        ] {
            assert!(
                !cartography_capability.contains(forbidden),
                "the cartography capability must not grant {forbidden}: {cartography_capability}"
            );
        }
        assert!(crate::command_acl::APP_COMMANDS.contains(&"map_pin_create"));
        assert!(crate::command_acl::APP_COMMANDS.contains(&"settings_update"));
        assert_eq!(
            crate::command_acl::CARTOGRAPHY_COMMANDS,
            [
                "map_pin_create",
                "map_pin_nearby",
                "map_views_list",
                "maps_scan_coordinates",
                "pin_configs_list",
                "planet_maps_list",
                "substrate_ready"
            ]
        );
        assert!(!crate::command_acl::CARTOGRAPHY_COMMANDS.contains(&"map_pin_delete"));
        assert!(!crate::command_acl::CARTOGRAPHY_COMMANDS.contains(&"settings_update"));
    }

    #[test]
    fn the_navigation_hud_keeps_a_narrow_selection_and_route_capability() {
        let capability = include_str!("../capabilities/navigation-overlay.json");
        assert!(capability.contains("\"windows\": [\"navigation-hud\"]"));
        assert!(capability.contains("navigation-commands"));
        assert!(capability.contains("core:event:allow-emit"));
        for forbidden in [
            "trusted-commands",
            "shell:allow-open",
            "core:window:allow-create",
            "core:webview:allow-create-webview-window",
            "store:allow-set",
        ] {
            assert!(
                !capability.contains(forbidden),
                "the navigation capability must not grant {forbidden}: {capability}"
            );
        }
        assert_eq!(
            crate::command_acl::NAVIGATION_COMMANDS,
            [
                "begin_navigation_area_selection",
                "navigation_start",
                "maps_scan_coordinates",
                "navigation_snapshot",
                "navigation_update_position",
                "navigation_mark_visited",
                "navigation_skip",
                "navigation_resolve_harvest",
                "navigation_undo",
                "navigation_end",
                "hide_navigation_overlays",
                "substrate_ready",
            ]
        );
    }
}
