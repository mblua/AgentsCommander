use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::settings::WindowGeometry;
use crate::session::manager::SessionManager;
use crate::DetachedSessionsState;

const MAIN_WINDOW_LABEL: &str = "main";
const RESOURCE_MONITOR_WINDOW_LABEL: &str = "resource-monitor";
const RESOURCE_MONITOR_FLOATING_WIDTH: u32 = 760;
const RESOURCE_MONITOR_FLOATING_HEIGHT: u32 = 560;
const RESOURCE_MONITOR_DOCK_WIDTH: u32 = 420;
const RESOURCE_MONITOR_MIN_WIDTH: u32 = 520;
const RESOURCE_MONITOR_MIN_HEIGHT: u32 = 420;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PhysicalWindowRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl PhysicalWindowRect {
    const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn right(self) -> i32 {
        self.x.saturating_add(u32_to_i32(self.width))
    }

    fn bottom(self) -> i32 {
        self.y.saturating_add(u32_to_i32(self.height))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResourceMonitorPlacement {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug)]
struct MonitorWorkArea {
    rect: PhysicalWindowRect,
    scale_factor: f64,
}

fn u32_to_i32(value: u32) -> i32 {
    value.min(i32::MAX as u32) as i32
}

fn effective_scale_factor(scale_factor: f64) -> f64 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

fn clamp_position(value: i32, min: i32, max: i32) -> i32 {
    if min > max {
        min
    } else {
        value.clamp(min, max)
    }
}

fn fits_horizontally(x: i32, width: u32, bounds: PhysicalWindowRect) -> bool {
    x >= bounds.x && x.saturating_add(u32_to_i32(width)) <= bounds.right()
}

fn resource_monitor_placement_for_main(
    main_rect: PhysicalWindowRect,
    monitor_bounds: PhysicalWindowRect,
    requested_width: u32,
    requested_height: u32,
) -> ResourceMonitorPlacement {
    let width = requested_width.min(monitor_bounds.width).max(1);
    let height = requested_height.min(monitor_bounds.height).max(1);
    let max_x = monitor_bounds.right().saturating_sub(u32_to_i32(width));
    let max_y = monitor_bounds.bottom().saturating_sub(u32_to_i32(height));

    let right_x = main_rect.right();
    let left_x = main_rect.x.saturating_sub(u32_to_i32(width));
    let x = if fits_horizontally(right_x, width, monitor_bounds) {
        right_x
    } else if fits_horizontally(left_x, width, monitor_bounds) {
        left_x
    } else {
        clamp_position(
            main_rect.right().saturating_sub(u32_to_i32(width)),
            monitor_bounds.x,
            max_x,
        )
    };

    ResourceMonitorPlacement {
        x,
        y: clamp_position(main_rect.y, monitor_bounds.y, max_y),
        width,
        height,
    }
}

fn window_outer_rect<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
) -> Result<PhysicalWindowRect, String> {
    let position = window.outer_position().map_err(|e| e.to_string())?;
    let size = window.outer_size().map_err(|e| e.to_string())?;
    Ok(PhysicalWindowRect::new(
        position.x,
        position.y,
        size.width,
        size.height,
    ))
}

fn monitor_work_area_for_window<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
) -> Result<MonitorWorkArea, String> {
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "No monitor is available for main window".to_string())?;
    let work_area = monitor.work_area();
    Ok(MonitorWorkArea {
        rect: PhysicalWindowRect::new(
            work_area.position.x,
            work_area.position.y,
            work_area.size.width,
            work_area.size.height,
        ),
        scale_factor: monitor.scale_factor(),
    })
}

/// Canonical detach implementation shared by the Tauri command + the Phase 3 restore
/// loop (plan §A2.2.G1 + §A3.2.3). Ordering invariants, in order:
///
/// 1. Focus-existing short-circuit (if the window already exists for this UUID).
/// 2. Build the WebviewWindow. Any build failure returns Err without mutating state.
/// 3. Post-build session-existence recheck (G.7 race). If the session was destroyed
///    between the caller's check and window build, destroy the just-built window and
///    bail with Err — no stale UUID inserted into `DetachedSessionsState`.
/// 4. Insert UUID into `DetachedSessionsState`.
/// 5. Set `Session::was_detached = true` via SessionManager (Fix A — A3.2.3).
///    This is the authoritative source-of-truth for persistence under plan §A3.2.
/// 6. Emit `terminal_detached` for frontend sync.
/// 7. Sibling-switch: if `skip_switch == false`, find the next non-detached session
///    and promote it to active in main. `skip_switch == true` is used by the Phase 3
///    restore path so the restore loop's post-loop `active_id` switch is not raced
///    (§R.10 / §A3.3 / §A2.2.G3).
///
/// `geometry: Some(geo)` uses the given position/size; `None` falls back to
/// default 900×600 (plan §A2.2.G1).
pub(crate) async fn detach_terminal_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    detached: &DetachedSessionsState,
    session_id: &str,
    geometry: Option<WindowGeometry>,
    skip_switch: bool,
) -> Result<String, String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let uuid = Uuid::parse_str(session_id).map_err(|e| e.to_string())?;
    let label = format!("terminal-{}", session_id.replace('-', ""));
    let url = format!("index.html?window=detached&sessionId={}", session_id);

    // Focus-existing short-circuit — matches pre-0.8.0 behavior.
    if let Some(existing) = app.get_webview_window(&label) {
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(label);
    }

    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/icon.png"))
        .expect("Failed to load app icon");

    // Step 2: build first. If build fails, no state mutation — G.1 leak plugged.
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("Terminal [detached]")
        .icon(icon)
        .map_err(|e| e.to_string())?
        .min_inner_size(400.0, 300.0)
        .decorations(false)
        .zoom_hotkeys_enabled(false);
    if let Some(ref geo) = geometry {
        builder = builder
            .inner_size(geo.width, geo.height)
            .position(geo.x, geo.y);
    } else {
        builder = builder.inner_size(900.0, 600.0);
    }
    let win = builder.build().map_err(|e| e.to_string())?;

    // Step 3: post-build session-existence recheck (G.7). If a concurrent destroy
    // removed the session between the caller's check and our window build, roll
    // back by destroying the window and returning Err. NOT inserting into the
    // detached set keeps `DetachedSessionsState` clean for the next action.
    {
        let mgr = session_mgr.read().await;
        if mgr.get_session(uuid).await.is_none() {
            let _ = win.destroy();
            return Err("Session destroyed during window build".into());
        }
    }

    // Step 4: insert UUID into DetachedSessionsState (idempotent).
    {
        let mut set = detached.lock().unwrap();
        set.insert(uuid);
    }

    // Step 5: sync Session::was_detached for persistence (Fix A, plan §A3.2.3).
    {
        let mgr = session_mgr.read().await;
        mgr.set_was_detached(uuid, true).await;
    }

    // Step 6: emit terminal_detached — frontend stores + main-window pre-warm listener
    // (A2.3.G6) subscribe to this.
    let _ = app.emit(
        "terminal_detached",
        serde_json::json!({ "sessionId": session_id, "windowLabel": label }),
    );

    // Step 7: sibling-switch — skip on restore path per R.10 / A3.3 / A2.2.G3.
    if !skip_switch {
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        let next_id = {
            let set = detached.lock().unwrap();
            sessions
                .iter()
                .find(|s| {
                    Uuid::parse_str(&s.id)
                        .ok()
                        .is_some_and(|u| !set.contains(&u))
                })
                .map(|s| s.id.clone())
        };

        if let Some(next_id) = next_id {
            let next_uuid = Uuid::parse_str(&next_id).map_err(|e| e.to_string())?;
            // G.10 tolerance: switch failure logs + emits null rather than propagating.
            match mgr.switch_session(next_uuid).await {
                Ok(()) => {
                    let _ = app.emit("session_switched", serde_json::json!({ "id": next_id }));
                }
                Err(e) => {
                    log::warn!("[detach] switch to sibling {} failed: {}", next_id, e);
                    mgr.clear_active().await;
                    let _ = app.emit(
                        "session_switched",
                        serde_json::json!({ "id": serde_json::Value::Null }),
                    );
                }
            }
        } else {
            mgr.clear_active().await;
            let _ = app.emit(
                "session_switched",
                serde_json::json!({ "id": serde_json::Value::Null }),
            );
        }
    }

    Ok(label)
}

/// Detach a session into its own terminal window.
#[tauri::command]
pub async fn detach_terminal(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    detached: State<'_, DetachedSessionsState>,
    session_id: String,
) -> Result<String, String> {
    // Pull any previously-persisted detached_geometry for this session so the
    // window re-opens where the user last left it. Fresh detach (never opened
    // before) falls back to the 900×600 default inside detach_terminal_inner.
    let geometry = {
        let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
        let mgr = session_mgr.read().await;
        mgr.get_session(uuid)
            .await
            .and_then(|s| s.detached_geometry)
    };
    detach_terminal_inner(
        &app,
        session_mgr.inner(),
        detached.inner(),
        &session_id,
        geometry,
        false,
    )
    .await
}

/// Re-attach a detached session to the main window. Closes the detached window (if any),
/// clears `Session::was_detached` (Fix A — must happen BEFORE emitting events so any
/// intervening snapshot sees the correct state, plan §A3.2.4 / NEW-2), switches the
/// main-pane active session, and emits `terminal_attached` + `session_switched`.
///
/// Plan §A2.2.G5 contract: when the session is absent from `SessionManager`, return
/// `Ok(())` silently without emitting events.
#[tauri::command]
pub async fn attach_terminal(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    detached: State<'_, DetachedSessionsState>,
    session_id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

    // A2.2.G5 contract: silent no-op when the session is gone.
    {
        let mgr = session_mgr.read().await;
        if mgr.get_session(uuid).await.is_none() {
            let mut set = detached.lock().unwrap();
            set.remove(&uuid);
            let label = format!("terminal-{}", session_id.replace('-', ""));
            if let Some(win) = app.get_webview_window(&label) {
                let _ = win.destroy();
            }
            log::info!(
                "[attach] session {} already destroyed; silent no-op",
                session_id
            );
            return Ok(());
        }
    }

    // Close the detached window if present. Use destroy() per R.2 to bypass any
    // onCloseRequested handler (avoids recursion if this runs inside the X-click
    // intercept path on the detached window). Destroy failure is a real attach
    // failure: keep detached state intact so we do not render the same PTY twice.
    let label = format!("terminal-{}", session_id.replace('-', ""));
    if let Some(win) = app.get_webview_window(&label) {
        win.destroy().map_err(|e| {
            format!(
                "Failed to destroy detached window {} during attach: {}",
                label, e
            )
        })?;
    }

    // Remove from DetachedSessionsState only after the detached window is gone.
    {
        let mut set = detached.lock().unwrap();
        set.remove(&uuid);
    }

    let mgr = session_mgr.read().await;
    if mgr.get_session(uuid).await.is_none() {
        log::info!(
            "[attach] session {} already destroyed; silent no-op",
            session_id
        );
        return Ok(());
    }

    // Fix A (§A3.2.4 / NEW-2): clear was_detached BEFORE switch + emit so any
    // snapshot that runs between set_was_detached and emit captures the correct
    // post-attach state.
    mgr.set_was_detached(uuid, false).await;

    // Session lives → promote to active in main.
    mgr.switch_session(uuid).await.map_err(|e| e.to_string())?;
    let _ = app.emit(
        "terminal_attached",
        serde_json::json!({ "sessionId": session_id }),
    );
    let _ = app.emit(
        "session_switched",
        serde_json::json!({ "id": session_id, "userInitiated": true }),
    );

    Ok(())
}

/// Return the list of session IDs currently in `DetachedSessionsState`. Used by
/// the sidebar frontend to hydrate its `detachedIds` store on mount (plan §A2.3.G8).
#[tauri::command]
pub fn list_detached_sessions(detached: State<'_, DetachedSessionsState>) -> Vec<String> {
    let set = detached.lock().unwrap();
    set.iter().map(|u| u.to_string()).collect()
}

/// Record the geometry of a detached window. Called by the frontend on drag/resize
/// (debounced). Persisted via the normal session-snapshot pipeline — the value
/// lives on `Session::detached_geometry` and travels into `PersistedSession` on
/// the next snapshot (plan §Arb-1 / §A2.4.Arb1 / §6.2).
#[tauri::command]
pub async fn set_detached_geometry(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
    geometry: WindowGeometry,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let mgr = session_mgr.read().await;
    mgr.set_detached_geometry(uuid, geometry).await;
    Ok(())
}

/// Open a path in the system file explorer (Explorer, Finder, xdg-open).
#[tauri::command]
pub fn open_in_explorer(path: String) -> Result<(), String> {
    let canonical = std::fs::canonicalize(&path)
        .map_err(|_| format!("Path does not exist or is inaccessible: {}", path))?;
    if !canonical.is_dir() {
        return Err(format!("Path is not a directory: {}", path));
    }
    open::that_detached(canonical).map_err(|e| format!("Failed to open explorer: {}", e))
}

/// Open an http/https URL in the user's default browser.
/// Refuses any other scheme to prevent the frontend from invoking arbitrary
/// shell handlers via crafted URLs. Scheme check is case-insensitive
/// (RFC 3986 §3.1) but the original URL is passed to `open::that_detached`.
#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(format!("Refusing to open non-http(s) URL: {}", url));
    }
    open::that_detached(trimmed).map_err(|e| format!("Failed to open URL: {}", e))
}

/// Ensure the unified main window exists and is focused. In 0.8.0 the main window
/// is always created at startup, so this almost always just shows + focuses it;
/// the recreate branch is defensive cover for the (unexpected) case where main
/// was closed without quitting the app.
///
/// Renamed from `ensure_terminal_window` per R.4 / Arb-3 — 9 callers preserved via
/// the `ensureTerminal` → `focusMain` deprecated alias on the frontend.
#[tauri::command]
pub async fn focus_main_window(app: AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    // Already exists — show (may be hidden) and focus.
    if let Some(win) = app.get_webview_window("main") {
        win.show().map_err(|e| e.to_string())?;
        win.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    // Defensive recreate — main was closed without quitting the app. Uses the saved
    // geometry (or a sensible default) so the window appears where the user last left it.
    let saved = crate::config::settings::load_settings();

    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/icon.png"))
        .expect("Failed to load app icon");

    let mut builder = WebviewWindowBuilder::new(
        &app,
        "main",
        WebviewUrl::App("index.html?window=main".into()),
    )
    .title(crate::config::profile::app_title())
    .icon(icon)
    .map_err(|e| e.to_string())?
    .min_inner_size(800.0, 500.0)
    .decorations(false)
    .zoom_hotkeys_enabled(false);

    if let Some(geo) = &saved.main_geometry {
        builder = builder
            .inner_size(geo.width, geo.height)
            .position(geo.x, geo.y);
    } else {
        builder = builder.inner_size(1400.0, 900.0);
    }

    let win = builder.build().map_err(|e| e.to_string())?;
    win.set_focus().map_err(|e| e.to_string())?;

    Ok(())
}

/// Open the guide window (Hints, Tutorial).
/// If already open, just focus it.
#[tauri::command]
pub async fn open_guide_window(app: AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    if let Some(existing) = app.get_webview_window("guide") {
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/icon.png"))
        .expect("Failed to load app icon");

    WebviewWindowBuilder::new(
        &app,
        "guide",
        WebviewUrl::App("index.html?window=guide".into()),
    )
    .title(format!(
        "Guide — {}",
        crate::config::profile::app_title_suffix()
    ))
    .icon(icon)
    .map_err(|e| e.to_string())?
    .inner_size(720.0, 560.0)
    .min_inner_size(480.0, 380.0)
    .decorations(false)
    .zoom_hotkeys_enabled(false)
    .build()
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Open the floating spec/Mermaid board window.
/// If already open, just focus it.
#[tauri::command]
pub async fn open_spec_board_window(app: AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    if let Some(existing) = app.get_webview_window("spec-board") {
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/icon.png"))
        .expect("Failed to load app icon");

    WebviewWindowBuilder::new(
        &app,
        "spec-board",
        WebviewUrl::App("index.html?window=spec-board".into()),
    )
    .title(format!(
        "Spec Board - {}",
        crate::config::profile::app_title_suffix()
    ))
    .icon(icon)
    .map_err(|e| e.to_string())?
    .inner_size(1200.0, 780.0)
    .min_inner_size(720.0, 460.0)
    .decorations(false)
    .zoom_hotkeys_enabled(false)
    .build()
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Open the floating Resource Monitor window.
/// If already open, just focus it.
#[tauri::command]
pub async fn open_resource_monitor_window(app: AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    if let Some(existing) = app.get_webview_window(RESOURCE_MONITOR_WINDOW_LABEL) {
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    let main = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "Main window is not available".to_string())?;
    let main_rect = window_outer_rect(&main)?;
    let monitor_work_area = monitor_work_area_for_window(&main)?;
    let placement = resource_monitor_placement_for_main(
        main_rect,
        monitor_work_area.rect,
        RESOURCE_MONITOR_FLOATING_WIDTH,
        RESOURCE_MONITOR_FLOATING_HEIGHT,
    );
    let scale_factor = effective_scale_factor(monitor_work_area.scale_factor);

    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/icon.png"))
        .expect("Failed to load app icon");

    let window = WebviewWindowBuilder::new(
        &app,
        RESOURCE_MONITOR_WINDOW_LABEL,
        WebviewUrl::App("index.html?window=resource-monitor".into()),
    )
    .title(format!(
        "Resource Monitor - {}",
        crate::config::profile::app_title_suffix()
    ))
    .icon(icon)
    .map_err(|e| e.to_string())?
    .inner_size(
        placement.width as f64 / scale_factor,
        placement.height as f64 / scale_factor,
    )
    .position(
        placement.x as f64 / scale_factor,
        placement.y as f64 / scale_factor,
    )
    .min_inner_size(
        RESOURCE_MONITOR_MIN_WIDTH as f64,
        RESOURCE_MONITOR_MIN_HEIGHT as f64,
    )
    .decorations(false)
    .zoom_hotkeys_enabled(false)
    .build()
    .map_err(|e| e.to_string())?;

    window
        .set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: placement.width,
            height: placement.height,
        }))
        .map_err(|e| e.to_string())?;
    let actual_size = window.outer_size().map_err(|e| e.to_string())?;
    let placement = resource_monitor_placement_for_main(
        main_rect,
        monitor_work_area.rect,
        actual_size.width,
        actual_size.height,
    );
    window
        .set_position(tauri::Position::Physical(tauri::PhysicalPosition {
            x: placement.x,
            y: placement.y,
        }))
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Move the Resource Monitor beside the main window.
#[tauri::command]
pub async fn dock_resource_monitor_window(app: AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "Main window is not available".to_string())?;
    let monitor = app
        .get_webview_window(RESOURCE_MONITOR_WINDOW_LABEL)
        .ok_or_else(|| "Resource Monitor window is not open".to_string())?;

    let main_rect = window_outer_rect(&main)?;
    let monitor_work_area = monitor_work_area_for_window(&main)?;
    let placement = resource_monitor_placement_for_main(
        main_rect,
        monitor_work_area.rect,
        RESOURCE_MONITOR_DOCK_WIDTH,
        main_rect.height.max(RESOURCE_MONITOR_MIN_HEIGHT),
    );

    monitor
        .set_size(tauri::Size::Physical(tauri::PhysicalSize {
            width: placement.width,
            height: placement.height,
        }))
        .map_err(|e| e.to_string())?;
    let actual_size = monitor.outer_size().map_err(|e| e.to_string())?;
    let placement = resource_monitor_placement_for_main(
        main_rect,
        monitor_work_area.rect,
        actual_size.width,
        actual_size.height,
    );
    monitor
        .set_position(tauri::Position::Physical(tauri::PhysicalPosition {
            x: placement.x,
            y: placement.y,
        }))
        .map_err(|e| e.to_string())?;
    monitor.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        resource_monitor_placement_for_main, PhysicalWindowRect, RESOURCE_MONITOR_DOCK_WIDTH,
    };

    #[test]
    fn resource_monitor_stays_inside_negative_x_monitor_when_right_side_is_full() {
        let main = PhysicalWindowRect::new(-1920, 0, 1920, 1040);
        let monitor = PhysicalWindowRect::new(-1920, 0, 1920, 1040);

        let placement =
            resource_monitor_placement_for_main(main, monitor, RESOURCE_MONITOR_DOCK_WIDTH, 1040);

        assert_eq!(placement.x, -420);
        assert_eq!(placement.y, 0);
        assert_eq!(placement.width, 420);
        assert_eq!(placement.height, 1040);
        assert!(placement.x >= monitor.x);
        assert!(placement.x + placement.width as i32 <= monitor.right());
    }

    #[test]
    fn resource_monitor_uses_right_side_when_it_fits_same_monitor() {
        let main = PhysicalWindowRect::new(80, 40, 1000, 700);
        let monitor = PhysicalWindowRect::new(0, 0, 1920, 1080);

        let placement =
            resource_monitor_placement_for_main(main, monitor, RESOURCE_MONITOR_DOCK_WIDTH, 700);

        assert_eq!(placement.x, 1080);
        assert_eq!(placement.y, 40);
        assert_eq!(placement.width, 420);
        assert_eq!(placement.height, 700);
    }
}
