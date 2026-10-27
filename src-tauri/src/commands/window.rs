use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::settings::{AppSettings, MainWindowDisplayState, WindowGeometry};
use crate::session::manager::{CommitDecision, LifecycleMutations, SessionManager};
use crate::session::selection::{
    SelectionCause, SelectionCoordinator, SelectionSource, SelectionTransaction,
};
use crate::session::session::SessionStatus;
use crate::DetachedSessionsState;

#[cfg(test)]
#[derive(Default)]
struct WindowDestroyAudit(std::sync::Mutex<Vec<String>>);

#[cfg(test)]
impl WindowDestroyAudit {
    fn record(&self, label: &str) {
        self.0.lock().unwrap().push(label.to_string());
    }

    fn count(&self, label: &str) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|observed| observed.as_str() == label)
            .count()
    }
}

const MAIN_WINDOW_LABEL: &str = "main";
const RESOURCE_MONITOR_WINDOW_LABEL: &str = "resource-monitor";
/// #1171 - the singleton watcher activity window.
///
/// `pub(crate)` because the watcher sink checks for this window before emitting: when it does
/// not exist, no `watcher_matches` event is produced at all, which is the case that holds
/// most of the time.
pub(crate) const WATCHERS_WINDOW_LABEL: &str = "watchers";
const RESOURCE_MONITOR_FLOATING_WIDTH: u32 = 760;
const RESOURCE_MONITOR_FLOATING_HEIGHT: u32 = 560;
const RESOURCE_MONITOR_DOCK_WIDTH: u32 = 420;
const RESOURCE_MONITOR_MIN_WIDTH: u32 = 520;
const RESOURCE_MONITOR_MIN_HEIGHT: u32 = 420;
/// #1171 - the size the activity window opens at when no geometry was ever saved. Wider
/// than tall because the table is four columns (time, watcher, session, captures) and the
/// captures cell is the one that must not be cramped.
const WATCHERS_DEFAULT_WIDTH: f64 = 980.0;
const WATCHERS_DEFAULT_HEIGHT: f64 = 640.0;
const WATCHERS_MIN_WIDTH: f64 = 640.0;
const WATCHERS_MIN_HEIGHT: f64 = 420.0;

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
pub(crate) async fn detach_terminal_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _detached: &DetachedSessionsState,
    session_id: &str,
    geometry: Option<WindowGeometry>,
    skip_switch: bool,
) -> Result<String, String> {
    let uuid = Uuid::parse_str(session_id).map_err(|error| error.to_string())?;
    if session_mgr.read().await.get_session(uuid).await.is_none() {
        return Err("Session not found".to_string());
    }
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    coordinator.detach(uuid, geometry, skip_switch).await
}

pub(crate) async fn execute_detach_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    geometry: Option<WindowGeometry>,
    suppress_selection: bool,
) -> Result<String, String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let snapshot = transaction.aggregate_snapshot().await;
    let record = snapshot
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| "Session not found".to_string())?;
    if matches!(record.status, SessionStatus::Exited(_))
        || !transaction.runtime_snapshot(session_id).has_pty
    {
        return Err("Session has no live PTY".to_string());
    }
    let session_id_string = session_id.to_string();
    let label = format!("terminal-{}", session_id_string.replace('-', ""));
    if let Some(existing) = transaction.app().get_webview_window(&label) {
        existing.set_focus().map_err(|error| error.to_string())?;
        return Ok(label);
    }

    let url = format!("index.html?window=detached&sessionId={session_id_string}");
    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/icon.png"))
        .map_err(|error| format!("Failed to load app icon: {error}"))?;
    let window = {
        let mut builder =
            WebviewWindowBuilder::new(transaction.app(), &label, WebviewUrl::App(url.into()))
                .title("Terminal [detached]")
                .icon(icon)
                .map_err(|error| error.to_string())?
                .min_inner_size(400.0, 300.0)
                .decorations(false)
                .zoom_hotkeys_enabled(false);
        if let Some(ref geometry) = geometry {
            builder = builder
                .inner_size(geometry.width, geometry.height)
                .position(geometry.x, geometry.y);
        } else {
            builder = builder.inner_size(900.0, 600.0);
        }
        builder.build().map_err(|error| error.to_string())?
    };

    transaction
        .app()
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(session_id);
    let window_still_present = transaction.app().get_webview_window(&label).is_some();
    let session_still_present = transaction
        .manager()
        .await
        .get_session(session_id)
        .await
        .is_some();
    let runtime_still_live = transaction.runtime_snapshot(session_id).has_pty;
    let still_valid = window_still_present && session_still_present && runtime_still_live;
    if !still_valid {
        let destroy_result = window.destroy();
        if let Err(error) = &destroy_result {
            log::warn!(
                "[detach] compensating window destroy failed session={}: {}",
                session_id,
                error
            );
        }
        #[cfg(test)]
        if destroy_result.is_ok() {
            if let Some(audit) = transaction.app().try_state::<WindowDestroyAudit>() {
                audit.record(&label);
            }
        }
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        if session_still_present && !runtime_still_live {
            transaction
                .reconcile_route_loss_inline(session_id, 1)
                .await?;
        }
        return Err("Session lost liveness during detach".to_string());
    }

    let final_snapshot = transaction.aggregate_snapshot().await;
    let decision = if final_snapshot.selection.id() != Some(session_id) {
        CommitDecision::Keep
    } else if suppress_selection {
        CommitDecision::Clear
    } else {
        first_live_fallback(transaction, &final_snapshot.sessions, session_id)
            .unwrap_or(CommitDecision::Clear)
    };
    let mut mutations = LifecycleMutations::default();
    mutations.set_detached_intent(session_id, true);
    let cause = if suppress_selection {
        SelectionCause::Restore
    } else {
        SelectionCause::Detach
    };
    let committed = transaction.commit(decision, cause, mutations).await?;
    transaction
        .persist(
            if suppress_selection {
                SelectionSource::Restore
            } else {
                SelectionSource::Detach
            },
            Some(session_id),
        )
        .await;
    if let Err(error) = transaction.app().emit(
        "terminal_detached",
        serde_json::json!({
            "sessionId": session_id_string,
            "windowLabel": label,
        }),
    ) {
        log::warn!(
            "[detach] terminal_detached publication failed session={}: {}",
            session_id,
            error
        );
    }
    if let Some(selection) = committed.selection.as_ref() {
        transaction.publish_selection(selection);
    }
    Ok(label)
}

fn first_live_fallback<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    sessions: &[crate::session::session::Session],
    excluded: Uuid,
) -> Option<CommitDecision> {
    sessions.iter().find_map(|candidate| {
        if candidate.id == excluded || matches!(candidate.status, SessionStatus::Exited(_)) {
            return None;
        }
        transaction.live_decision(candidate.id)
    })
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
    _session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    _detached: State<'_, DetachedSessionsState>,
    session_id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    coordinator.attach(uuid).await
}

pub(crate) async fn execute_attach_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) -> Result<(), String> {
    let manager = transaction.manager().await;
    let Some(record) = manager.get_session(session_id).await else {
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
        if let Some(window) = transaction.app().get_webview_window(&label) {
            if let Err(error) = window.destroy() {
                log::warn!(
                    "[attach] stale detached window cleanup failed session={}: {}",
                    session_id,
                    error
                );
            }
        }
        return Ok(());
    };

    let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
    if let Some(window) = transaction.app().get_webview_window(&label) {
        window.destroy().map_err(|error| {
            format!(
                "Failed to destroy detached window {} during attach: {}",
                label, error
            )
        })?;
        #[cfg(test)]
        if let Some(audit) = transaction.app().try_state::<WindowDestroyAudit>() {
            audit.record(&label);
        }
    }
    transaction
        .app()
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&session_id);

    let runtime = transaction.runtime_snapshot(session_id);
    let liveness_lost = !runtime.has_pty && !matches!(record.status, SessionStatus::Exited(_));
    let mut mutations = LifecycleMutations::default();
    let (decision, cause, source) = match record.status {
        SessionStatus::Exited(_) => (
            transaction
                .dormant_decision(session_id)
                .ok_or_else(|| "Attached dormant session remained detached".to_string())?,
            SelectionCause::Attach,
            SelectionSource::Attach,
        ),
        _ if runtime.has_pty => (
            transaction
                .live_decision(session_id)
                .ok_or_else(|| "Attached session is not displayable".to_string())?,
            SelectionCause::Attach,
            SelectionSource::Attach,
        ),
        _ => {
            mutations.mark_exited(session_id, 1);
            (
                transaction.dormant_decision(session_id).ok_or_else(|| {
                    "Attached liveness-loss session remained detached".to_string()
                })?,
                SelectionCause::LivenessReconcile,
                SelectionSource::LivenessReconcile,
            )
        }
    };
    mutations.set_detached_intent(session_id, false);
    let committed = transaction.commit(decision, cause, mutations).await?;
    transaction.persist(source, Some(session_id)).await;
    if liveness_lost && !committed.changed_rows.is_empty() {
        transaction.publish_destroyed(session_id);
        for row in &committed.changed_rows {
            transaction.publish_created(row);
        }
        for cleared in &committed.cleared_raise_hand_ids {
            transaction.publish_communication_cleared(*cleared);
        }
    }
    if let Err(error) = transaction.app().emit(
        "terminal_attached",
        serde_json::json!({ "sessionId": session_id.to_string() }),
    ) {
        log::warn!(
            "[attach] terminal_attached publication failed session={}: {}",
            session_id,
            error
        );
    }
    if let Some(selection) = committed.selection.as_ref() {
        transaction.publish_selection(selection);
    }
    if runtime.has_pty || matches!(record.status, SessionStatus::Exited(_)) {
        Ok(())
    } else {
        Err("Session has no live PTY".to_string())
    }
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

/// #1171 - record the geometry of the watcher activity window.
///
/// A DEDICATED single-field command, following `set_detached_geometry` above, and
/// deliberately not `initWindowGeometry` (`src/shared/window-geometry.ts:26-47`), which
/// performs a debounced read-modify-write of the WHOLE `AppSettings`. That race is already
/// documented in this repository (`commands/config.rs:653-655`) and defended against with an
/// explicit list of fields restored from live memory (`config.rs:611-624`, `:647-655`);
/// adding `watchers` to that list would make it six fields long and would leave the new
/// window as a whole-object writer. This touches one field and cannot clobber another.
#[tauri::command]
pub async fn set_watchers_geometry(
    settings: State<'_, crate::config::settings::SettingsState>,
    geometry: WindowGeometry,
) -> Result<(), String> {
    crate::commands::config::persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.watchers_geometry = Some(geometry);
    })
    .await
}

/// #2348 - the narrow owner of the main window's saved placement pair
/// (`mainGeometry` + `mainWindowDisplayState`).
///
/// Holds the managed settings write guard across the persistence call, so the
/// overlay rejection (inside `set_main_window_placement`) and the in-memory
/// publish are atomic against every other settings writer. A rejection returns
/// the stable `main_window_placement_overlay_pinned` and changes neither memory
/// nor the base JSON.
#[tauri::command]
pub async fn set_main_window_placement(
    settings: State<'_, crate::config::settings::SettingsState>,
    geometry: WindowGeometry,
    display_state: MainWindowDisplayState,
) -> Result<(), String> {
    set_main_window_placement_inner(settings.inner(), geometry, display_state).await
}

pub(crate) async fn set_main_window_placement_inner(
    settings: &crate::config::settings::SettingsState,
    geometry: WindowGeometry,
    display_state: MainWindowDisplayState,
) -> Result<(), String> {
    set_main_window_placement_inner_with_saver(
        settings,
        geometry,
        display_state,
        |current, geometry, display_state| {
            crate::config::settings::set_main_window_placement(current, geometry, display_state)
        },
    )
    .await
}

/// Saver seam mirroring the `*_with_saver` pairs in `commands/config.rs`, so a
/// unit test can drive the real guard/publish split without reaching
/// `config::config_dir()` and the developer's own settings.json.
async fn set_main_window_placement_inner_with_saver(
    settings: &crate::config::settings::SettingsState,
    geometry: WindowGeometry,
    display_state: MainWindowDisplayState,
    save: impl FnOnce(
        &AppSettings,
        &WindowGeometry,
        MainWindowDisplayState,
    ) -> Result<AppSettings, String>,
) -> Result<(), String> {
    let mut guard = settings.write().await;
    let written = save(&guard, &geometry, display_state)?;
    *guard = written;
    Ok(())
}

/// Open a path in the system file explorer (Explorer, Finder, xdg-open).
#[tauri::command]
pub fn open_in_explorer(path: String) -> Result<(), String> {
    let canonical = crate::window::validate_explorer_target(&path)?;
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

/// #1171 - the most recently requested scope of the watcher activity window, and the one
/// critical section that keeps it and the last emitted `watchers_scope_request` in agreement.
///
/// An event alone cannot carry a re-scope. The window label exists the moment
/// `WebviewWindowBuilder::build` returns, while the JavaScript listener exists only after the
/// bundle loads, Solid mounts and the subscription completes an IPC round trip. Tauri queues
/// nothing for a listener that does not exist yet and `emit_to` returns `Ok` either way, so
/// the backend cannot even observe the loss: a second open during the load focuses a window
/// that is not listening, emits, and the user's order is dropped in silence.
///
/// The fix is the shape the matches already use, subscribe first and then reconcile with a
/// pull, applied to the scope: this state holds the authoritative value, it is written before
/// every emit, and `get_watchers_scope` lets the window read it after its subscribe.
///
/// The mutex is `tokio`'s and is private to this module, so it can be held across the whole
/// body of `open_watchers_window` and cannot deadlock against anything else. It is contended
/// only by a second open.
#[derive(Default)]
pub struct WatchersScopeState {
    scope: tokio::sync::Mutex<Option<Uuid>>,
}

/// #1171 - the durable half of the scope handover: what `open_watchers_window` last asked for.
///
/// The window calls this in `onMount`, AFTER registering its `watchers_scope_request`
/// listener, and adopts the answer unless an event has been handled since the call was issued.
/// An emit that raced the subscribe is recovered here; an emit after this call reaches a
/// listener that exists.
#[tauri::command]
pub async fn get_watchers_scope(
    scope: State<'_, WatchersScopeState>,
) -> Result<Option<String>, String> {
    let scope = scope.scope.lock().await;
    Ok(scope.as_ref().map(|id| id.to_string()))
}

/// #1171 - open the singleton watcher activity window, or focus it and re-scope it to
/// `session_id`.
///
/// Mould: `open_resource_monitor_window` above, minus the main-window-relative placement,
/// which exists for the Resource Monitor's dock gesture and has no counterpart here.
///
/// Focus-if-exists is not the whole behavior for this window, because every caller names a
/// session: an already-open window is ALSO told to re-scope, through `watchers_scope_request`
/// (plan 4.12). The query parameter is read only on first creation, since the label is a
/// singleton and its URL never changes afterwards, and it is the window's INITIAL scope only -
/// `get_watchers_scope` is the authoritative one.
///
/// Generic over the runtime like its #1171 sibling `get_watcher_activity`
/// (`commands/pty.rs:545`) and `kill_resource_group` (`commands/resource_monitor.rs:45`), so
/// the singleton and re-scope halves are reachable from a `MockRuntime` test.
#[tauri::command]
pub async fn open_watchers_window<R: tauri::Runtime>(
    app: AppHandle<R>,
    scope: State<'_, WatchersScopeState>,
    session_id: String,
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    // Parsed and re-rendered rather than interpolated as received: the value lands in a URL
    // query string, and the canonical hyphenated form cannot carry an `&` or a `#`. Rejecting
    // a non-UUID also matches `get_watcher_activity` (`commands/pty.rs:550`). It happens
    // before the guard is taken, so a rejected id contends with nothing and leaves the
    // authoritative scope alone.
    let session_id = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

    // ONE critical section, from the state write through the existence check and whichever
    // branch runs, including the emit. Not two regions: splitting it leaves this interleaving.
    // A writes scope A and pauses; B writes scope B, creates the window with B in its URL; A
    // resumes, finds a window, and emits A. The authoritative state then says B while the last
    // event says A, so a window that is already listening lands on A and disagrees with
    // `get_watchers_scope`. Holding one guard across the whole body makes the last write and
    // the last emit the same call by construction.
    let mut scope = scope.scope.lock().await;
    *scope = Some(session_id);

    if let Some(existing) = app.get_webview_window(WATCHERS_WINDOW_LABEL) {
        // A focus that fails must not swallow the re-scope, which is the substantive half of
        // this call and the only one the user can see go wrong.
        if let Err(error) = existing.set_focus() {
            log::warn!("[watchers] focusing the activity window failed: {}", error);
        }
        app.emit_to(
            WATCHERS_WINDOW_LABEL,
            "watchers_scope_request",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .map_err(|e| e.to_string())?;
        return Ok(());
    }

    // Geometry is restored here rather than by `initWindowGeometry`, which this window stays
    // away from (plan 4.12); `focus_main_window` (`:635-641`) is the precedent for reading a
    // persisted rect straight into the builder.
    let saved = crate::config::settings::load_settings();

    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/icon.png"))
        .expect("Failed to load app icon");

    let mut builder = WebviewWindowBuilder::new(
        &app,
        WATCHERS_WINDOW_LABEL,
        WebviewUrl::App(format!("index.html?window=watchers&sessionId={}", session_id).into()),
    )
    .title(format!(
        "Watcher Activity - {}",
        crate::config::profile::app_title_suffix()
    ))
    .icon(icon)
    .map_err(|e| e.to_string())?
    .min_inner_size(WATCHERS_MIN_WIDTH, WATCHERS_MIN_HEIGHT)
    .decorations(false)
    .zoom_hotkeys_enabled(false);

    if let Some(geo) = &saved.watchers_geometry {
        builder = builder
            .inner_size(geo.width, geo.height)
            .position(geo.x, geo.y);
    } else {
        builder = builder.inner_size(WATCHERS_DEFAULT_WIDTH, WATCHERS_DEFAULT_HEIGHT);
    }

    builder.build().map_err(|e| e.to_string())?;

    Ok(())
}

#[cfg(test)]
mod tests;
