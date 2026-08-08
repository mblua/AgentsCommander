use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::config::settings::WindowGeometry;
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
    let window =
        {
            let mut builder = crate::apply_isolated_webview_data_directory(
                WebviewWindowBuilder::new(transaction.app(), &label, WebviewUrl::App(url.into())),
            )
            .map_err(|error| error.to_string())?
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

    let mut builder = crate::apply_isolated_webview_data_directory(WebviewWindowBuilder::new(
        &app,
        "main",
        WebviewUrl::App("index.html?window=main".into()),
    ))
    .map_err(|error| error.to_string())?
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

    crate::apply_isolated_webview_data_directory(WebviewWindowBuilder::new(
        &app,
        "guide",
        WebviewUrl::App("index.html?window=guide".into()),
    ))
    .map_err(|error| error.to_string())?
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

    crate::apply_isolated_webview_data_directory(WebviewWindowBuilder::new(
        &app,
        "spec-board",
        WebviewUrl::App("index.html?window=spec-board".into()),
    ))
    .map_err(|error| error.to_string())?
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

    let window = crate::apply_isolated_webview_data_directory(WebviewWindowBuilder::new(
        &app,
        RESOURCE_MONITOR_WINDOW_LABEL,
        WebviewUrl::App("index.html?window=resource-monitor".into()),
    ))
    .map_err(|error| error.to_string())?
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

    let mut builder = crate::apply_isolated_webview_data_directory(WebviewWindowBuilder::new(
        &app,
        WATCHERS_WINDOW_LABEL,
        WebviewUrl::App(format!("index.html?window=watchers&sessionId={}", session_id).into()),
    ))
    .map_err(|error| error.to_string())?
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
mod tests {
    use super::{
        get_watchers_scope, open_watchers_window, resource_monitor_placement_for_main,
        PhysicalWindowRect, WatchersScopeState, WindowDestroyAudit, RESOURCE_MONITOR_DOCK_WIDTH,
        WATCHERS_WINDOW_LABEL,
    };
    use crate::config::settings::WindowGeometry;
    use crate::pty::backend::{BackendSpawnSpec, PtyBackend, SessionBackendKind};
    use crate::pty::manager::PtyManager;
    use crate::session::manager::SessionManager;
    use crate::session::selection::{SelectionCoordinator, SelectionMode, SelectionSource};
    use crate::session::session::SessionStatus;
    use crate::web::broadcast::WsBroadcaster;
    use crate::DetachedSessionsState;
    use futures::future::BoxFuture;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;
    use tauri::{Listener, Manager, WebviewUrl, WebviewWindowBuilder};
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    struct GatedLivenessBackend {
        live: AtomicBool,
        calls: AtomicUsize,
        block_call: usize,
        entered: AtomicBool,
        gate: (Mutex<bool>, Condvar),
    }

    impl GatedLivenessBackend {
        fn new(block_call: usize) -> Self {
            Self {
                live: AtomicBool::new(true),
                calls: AtomicUsize::new(0),
                block_call,
                entered: AtomicBool::new(false),
                gate: (Mutex::new(false), Condvar::new()),
            }
        }

        fn lose_liveness_and_release(&self) {
            self.live.store(false, Ordering::SeqCst);
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }
    }

    impl PtyBackend for GatedLivenessBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            _spec: BackendSpawnSpec,
        ) -> BoxFuture<'_, Result<(), crate::errors::AppError>> {
            Box::pin(async { Ok(()) })
        }

        fn write(
            &self,
            _authority: &crate::pty::manager::BackendWriteAuthority,
            id: Uuid,
            _data: &[u8],
        ) -> Result<(), crate::errors::AppError> {
            self.live
                .load(Ordering::SeqCst)
                .then_some(())
                .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
        }

        fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
            self.write(
                &crate::pty::manager::BackendWriteAuthority::for_backend_test(),
                id,
                &[],
            )
        }

        fn kill(&self, _id: Uuid) -> Result<(), crate::errors::AppError> {
            self.live.store(false, Ordering::SeqCst);
            Ok(())
        }

        fn has_session(&self, _id: Uuid) -> bool {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.block_call {
                self.entered.store(true, Ordering::SeqCst);
                let mut released = self.gate.0.lock().unwrap();
                while !*released {
                    released = self.gate.1.wait(released).unwrap();
                }
            }
            self.live.load(Ordering::SeqCst)
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            self.live.load(Ordering::SeqCst).then_some((120, 30))
        }

        fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
            crate::pty::context_scrape::ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn detach_pty_loss_after_window_creation_compensates_and_reconciles_liveness() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/detach-race".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let backend = Arc::new(GatedLivenessBackend::new(2));
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let detached = DetachedSessionsState::default();
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(detached)
            .manage(WsBroadcaster::new())
            .manage(WindowDestroyAudit::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build detach race app");
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in ["session_destroyed", "session_created", "session_switched"] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |_| {
                let _ = events_tx.send(event_name);
            });
        }

        let detach = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.detach(session.id, None, false).await })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            while !backend.entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detach reaches post-window liveness barrier");
        let label = format!("terminal-{}", session.id.to_string().replace('-', ""));
        assert!(app.get_webview_window(&label).is_some());
        assert!(app
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap()
            .contains(&session.id));

        backend.lose_liveness_and_release();
        assert_eq!(
            detach.await.unwrap().unwrap_err(),
            "Session lost liveness during detach"
        );
        assert_eq!(app.state::<WindowDestroyAudit>().count(&label), 1);
        assert!(!app
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap()
            .contains(&session.id));
        let row = manager.read().await.get_session(session.id).await.unwrap();
        assert_eq!(row.status, SessionStatus::Exited(1));
        assert!(!row.was_detached);
        let selection = manager.read().await.selection_payload().await;
        assert_eq!(selection.id(), Some(session.id));
        assert_eq!(selection.mode(), SelectionMode::Dormant);
        assert_eq!(selection.source(), SelectionSource::LivenessReconcile);
        assert_eq!(
            (0..3)
                .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap())
                .collect::<Vec<_>>(),
            vec!["session_destroyed", "session_created", "session_switched"]
        );
        assert!(events_rx.try_recv().is_err());
        coordinator.close_and_join().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attach_post_destroy_pty_loss_clears_intent_preserves_geometry_and_publishes_exit() {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let session = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/attach-race".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();
        let geometry = WindowGeometry {
            x: 17.25,
            y: -44.5,
            width: 923.75,
            height: 611.125,
        };
        let manager_handle = manager.read().await.clone();
        manager_handle.set_was_detached(session.id, true).await;
        manager_handle
            .set_detached_geometry(session.id, geometry.clone())
            .await;
        let backend = Arc::new(GatedLivenessBackend::new(1));
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        pty.lock()
            .unwrap()
            .record_route(session.id, SessionBackendKind::LocalProcess);
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&pty))
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .manage(WindowDestroyAudit::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build attach race app");
        let label = format!("terminal-{}", session.id.to_string().replace('-', ""));
        WebviewWindowBuilder::new(app.handle(), &label, WebviewUrl::App("index.html".into()))
            .build()
            .unwrap();
        app.state::<DetachedSessionsState>()
            .lock()
            .unwrap()
            .insert(session.id);
        coordinator.start(app.handle().clone()).unwrap();
        coordinator.submit_restore_first().await.unwrap().finish();
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        for event_name in [
            "session_destroyed",
            "session_created",
            "terminal_attached",
            "session_switched",
        ] {
            let events_tx = events_tx.clone();
            app.listen_any(event_name, move |_| {
                let _ = events_tx.send(event_name);
            });
        }

        let attach = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.attach(session.id).await })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            while !backend.entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("attach reaches post-destroy liveness barrier");
        assert_eq!(app.state::<WindowDestroyAudit>().count(&label), 1);
        assert!(!app
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap()
            .contains(&session.id));

        backend.lose_liveness_and_release();
        assert_eq!(
            attach.await.unwrap().unwrap_err(),
            "Session has no live PTY"
        );
        let row = manager_handle.get_session(session.id).await.unwrap();
        assert_eq!(row.status, SessionStatus::Exited(1));
        assert!(!row.was_detached);
        let stored_geometry = row.detached_geometry.unwrap();
        assert_eq!(stored_geometry.x.to_bits(), geometry.x.to_bits());
        assert_eq!(stored_geometry.y.to_bits(), geometry.y.to_bits());
        assert_eq!(stored_geometry.width.to_bits(), geometry.width.to_bits());
        assert_eq!(stored_geometry.height.to_bits(), geometry.height.to_bits());
        let selection = manager_handle.selection_payload().await;
        assert_eq!(selection.id(), Some(session.id));
        assert_eq!(selection.mode(), SelectionMode::Dormant);
        assert_eq!(selection.source(), SelectionSource::LivenessReconcile);
        assert_eq!(
            (0..4)
                .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap())
                .collect::<Vec<_>>(),
            vec![
                "session_destroyed",
                "session_created",
                "terminal_attached",
                "session_switched",
            ]
        );
        assert!(events_rx.try_recv().is_err());
        coordinator.close_and_join().await;
    }

    /// An app that manages the scope state, which every `open_watchers_window` call needs.
    fn watchers_app(label: &str) -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .manage(WatchersScopeState::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap_or_else(|_| panic!("build {label} app"))
    }

    /// #1171, test 79a: a second open does not build a second window, and it does re-scope the
    /// one that is already there.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reopening_the_watchers_window_re_scopes_it_instead_of_building_a_second_one() {
        let app = watchers_app("watchers re-scope");
        WebviewWindowBuilder::new(
            app.handle(),
            WATCHERS_WINDOW_LABEL,
            WebviewUrl::App("index.html?window=watchers".into()),
        )
        .build()
        .unwrap();

        let (payloads_tx, payloads_rx) = std::sync::mpsc::channel();
        app.listen_any("watchers_scope_request", move |event| {
            let _ = payloads_tx.send(event.payload().to_string());
        });

        let session_id = Uuid::new_v4();
        open_watchers_window(app.handle().clone(), app.state(), session_id.to_string())
            .await
            .unwrap();

        let windows = app.webview_windows();
        assert_eq!(windows.len(), 1);
        assert!(windows.contains_key(WATCHERS_WINDOW_LABEL));

        let payload: serde_json::Value =
            serde_json::from_str(&payloads_rx.recv_timeout(Duration::from_secs(1)).unwrap())
                .unwrap();
        assert_eq!(payload["sessionId"], session_id.to_string());
        assert!(payloads_rx.try_recv().is_err());
    }

    /// #1171, test 79b: two opens leave the authoritative scope at the SECOND one, whether or
    /// not any listener exists.
    ///
    /// This is the property a Rust-side listener cannot certify. `app.listen_any` registers in
    /// the Rust registry, which is always durable, so no reordering of a `listen_any` test can
    /// model a JavaScript listener that has not been registered yet. Test 79a certifies that
    /// the emit is issued; this one certifies that the pull recovers it when it was not heard.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_last_open_wins_the_scope_with_no_listener_and_with_one() {
        let app = watchers_app("watchers scope pull");
        WebviewWindowBuilder::new(
            app.handle(),
            WATCHERS_WINDOW_LABEL,
            WebviewUrl::App("index.html?window=watchers".into()),
        )
        .build()
        .unwrap();

        assert_eq!(
            get_watchers_scope(app.state()).await.unwrap(),
            None,
            "nothing has been requested yet"
        );

        // No listener at all: the emits go nowhere, exactly as they do while the bundle loads.
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        open_watchers_window(app.handle().clone(), app.state(), first.to_string())
            .await
            .unwrap();
        open_watchers_window(app.handle().clone(), app.state(), second.to_string())
            .await
            .unwrap();
        assert_eq!(
            get_watchers_scope(app.state()).await.unwrap(),
            Some(second.to_string()),
            "the window that subscribes late still pulls the LAST order, not the first"
        );

        // And with a listener the two halves agree: the pull returns what the last emit said.
        let (payloads_tx, payloads_rx) = std::sync::mpsc::channel();
        app.listen_any("watchers_scope_request", move |event| {
            let _ = payloads_tx.send(event.payload().to_string());
        });
        let third = Uuid::new_v4();
        open_watchers_window(app.handle().clone(), app.state(), third.to_string())
            .await
            .unwrap();

        let payload: serde_json::Value =
            serde_json::from_str(&payloads_rx.recv_timeout(Duration::from_secs(1)).unwrap())
                .unwrap();
        assert_eq!(payload["sessionId"], third.to_string());
        assert_eq!(
            get_watchers_scope(app.state()).await.unwrap(),
            Some(third.to_string())
        );
    }

    /// #1171, test 79c: two concurrent opens for two different sessions produce exactly one
    /// window, two `Ok`s and no duplicate-label failure, AND the authoritative scope agrees
    /// with the event the loser emitted.
    ///
    /// Counting windows and `Ok`s is not enough. The defect this test exists for is the
    /// interleaving that leaves the state at one session and the last emitted event at the
    /// other, and a count-only assertion passes straight through it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_concurrent_opens_build_one_window_and_agree_on_the_scope() {
        let app = watchers_app("watchers concurrent open");

        let (payloads_tx, payloads_rx) = std::sync::mpsc::channel();
        app.listen_any("watchers_scope_request", move |event| {
            let _ = payloads_tx.send(event.payload().to_string());
        });

        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let (left, right) = tokio::join!(
            open_watchers_window(app.handle().clone(), app.state(), first.to_string()),
            open_watchers_window(app.handle().clone(), app.state(), second.to_string()),
        );
        assert!(left.is_ok(), "the losing call must not fail on the label");
        assert!(right.is_ok());

        let windows = app.webview_windows();
        assert_eq!(windows.len(), 1);
        assert!(windows.contains_key(WATCHERS_WINDOW_LABEL));

        // Exactly one of the two built the window; the other found it and emitted. Whichever
        // that was, it ran second under the one guard, so it is also the last state write.
        let emitted: serde_json::Value =
            serde_json::from_str(&payloads_rx.recv_timeout(Duration::from_secs(1)).unwrap())
                .unwrap();
        assert!(payloads_rx.try_recv().is_err(), "the builder emits nothing");
        assert_eq!(
            get_watchers_scope(app.state()).await.unwrap(),
            Some(emitted["sessionId"].as_str().unwrap().to_string()),
            "the authoritative scope and the last emitted event are the same call"
        );
    }

    /// The session id is interpolated into the window URL, so anything that is not a UUID is
    /// refused before a window exists to carry it, and before the scope state is touched.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn open_watchers_window_refuses_a_session_id_that_is_not_a_uuid() {
        let app = watchers_app("watchers rejection");

        assert!(open_watchers_window(
            app.handle().clone(),
            app.state(),
            "not-a-uuid&window=main".to_string()
        )
        .await
        .is_err());
        assert!(app.webview_windows().is_empty());
        assert_eq!(get_watchers_scope(app.state()).await.unwrap(), None);
    }
}
