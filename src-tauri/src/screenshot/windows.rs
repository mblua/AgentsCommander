//! #714 Windows-native screenshot runtime.
//!
//! Owns every native dependency for the feature: `xcap` desktop capture, `image`
//! crop/encode, the clipboard plugin, the global-shortcut plugin, overlay window
//! creation, and the capture lifecycle state machine. The non-Windows sibling
//! (`super::unsupported`) mirrors this public surface without any of it.
//!
//! Cleanup invariant: Rust owns overlay teardown. Every terminal path (confirm
//! success/failure, explicit cancel, overlay build failure, stale overlay IPC,
//! and `WindowEvent::Destroyed`) routes through either `confirm_selection`'s own
//! teardown or [`clear_capture_and_destroy_overlays`], both of which are
//! idempotent because they destroy overlays by label prefix and only act when a
//! matching capture is still live.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use chrono::Utc;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};
use uuid::Uuid;

use super::{
    HotkeyModifier, ParsedHotkey, ScreenshotCaptureEvent, ScreenshotCaptureResult,
    ScreenshotCaptureState, ScreenshotHotkeyState, ScreenshotHotkeyStatus, ScreenshotOverlayState,
    ScreenshotSelection,
};
use crate::session::manager::SessionManager;

/// Upper bound on the `-<n>.png` collision suffix, mirroring
/// `phone::messaging::MAX_COLLISION_SUFFIX`.
const MAX_COLLISION_SUFFIX: u32 = 9999;

const SAVED_EVENT: &str = "screenshot_capture_saved";
const FAILED_EVENT: &str = "screenshot_capture_failed";

// ── Lifecycle + runtime state ──────────────────────────────────────────────

/// Capture lifecycle. `Starting` is the gate that blocks concurrent hotkey
/// starts; it is reserved under the mutex before desktop capture and atomically
/// replaced by `Active` only if the same capture id is still pending. `Finishing`
/// is the teardown gate: `confirm_selection` holds it (instead of dropping
/// straight to `Idle`) while it saves the PNG and destroys overlays, so a second
/// hotkey press cannot start a capture that photographs the still-visible
/// overlays. The `Uuid` is the finishing capture's id, so the final reset to
/// `Idle` never clobbers a state another path already changed.
pub enum ScreenshotCaptureLifecycle {
    Idle,
    Starting(ScreenshotCaptureStarting),
    Active(ActiveScreenshotCapture),
    Finishing(Uuid),
}

pub struct ScreenshotCaptureStarting {
    pub id: Uuid,
    pub target_session_id: Uuid,
    pub target_session_name: String,
    pub target_directory: PathBuf,
    pub created_at: chrono::DateTime<Utc>,
}

pub struct ActiveScreenshotCapture {
    pub id: Uuid,
    pub target_session_id: Uuid,
    pub target_session_name: String,
    pub target_directory: PathBuf,
    pub monitors: Vec<CapturedMonitor>,
    pub overlay_labels: Vec<String>,
    pub created_at: chrono::DateTime<Utc>,
}

pub struct CapturedMonitor {
    /// Per-capture loop index (0..n). Stable key between capture storage and the
    /// overlay's `getOverlayState`/selection, independent of OS monitor ids.
    pub monitor_id: u32,
    /// xcap monitor name, kept for placement matching and diagnostics.
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub image: image::RgbaImage,
    pub preview_data_url: String,
}

/// Global-hotkey runtime. `registered_shortcut` is the plugin handle held so a
/// settings change can unregister the previous key before/after registering the
/// new one. Commands never expose it; they clone `status`. `Default` yields the
/// default status (see `ScreenshotHotkeyStatus::default`) and no registration.
#[derive(Default)]
pub struct ScreenshotHotkeyRuntime {
    pub status: ScreenshotHotkeyStatus,
    pub registered_shortcut: Option<Shortcut>,
}

// ── Hotkey registration ────────────────────────────────────────────────────

fn parsed_to_shortcut(parsed: &ParsedHotkey) -> Result<Shortcut, String> {
    let modifiers = match parsed.modifier {
        HotkeyModifier::Ctrl => Modifiers::CONTROL,
    };
    let code = key_char_to_code(parsed.key)
        .ok_or_else(|| format!("key '{}' has no global-shortcut code", parsed.key))?;
    Ok(Shortcut::new(Some(modifiers), code))
}

/// Map a normalized alphanumeric key char to a `Code` via its PascalCase name
/// (`Key{A..Z}` / `Digit{0..9}`), the string form `keyboard_types::Code`
/// round-trips through `FromStr`.
fn key_char_to_code(c: char) -> Option<Code> {
    let up = c.to_ascii_uppercase();
    let name = if up.is_ascii_alphabetic() {
        format!("Key{up}")
    } else if up.is_ascii_digit() {
        format!("Digit{up}")
    } else {
        return None;
    };
    name.parse::<Code>().ok()
}

/// Register the configured global hotkey, unregistering any previously-registered
/// one. Returns `Err` only when the OS refuses the registration (e.g. another app
/// already owns the combo); the previous hotkey is then kept registered. Syntax
/// errors also return `Err`, but settings validation blocks those before here.
///
/// Never propagate this `Err` into a settings-save failure: callers in
/// `commands::config` swallow it and surface a visible event instead.
pub fn register_configured_hotkey(app: &AppHandle, configured: &str) -> Result<(), String> {
    let parsed = super::parse_screenshot_hotkey(configured)?;
    let shortcut = parsed_to_shortcut(&parsed)?;

    let gs = app.global_shortcut();
    let state = app.state::<ScreenshotHotkeyState>();
    let mut runtime = state.lock().unwrap();

    // No-op save of the same key: it is already ours, so just record success and
    // avoid a spurious "already registered" error from re-registering.
    if gs.is_registered(shortcut) {
        runtime.registered_shortcut = Some(shortcut);
        runtime.status = hotkey_status_after_attempt(configured, Ok(()));
        return Ok(());
    }

    match gs.register(shortcut) {
        Ok(()) => {
            if let Some(prev) = runtime.registered_shortcut.take() {
                if prev != shortcut {
                    let _ = gs.unregister(prev);
                }
            }
            runtime.registered_shortcut = Some(shortcut);
            runtime.status = hotkey_status_after_attempt(configured, Ok(()));
            Ok(())
        }
        Err(e) => {
            // The configured shortcut did NOT register. Keep any previously
            // registered fallback alive (do not take `registered_shortcut`) so it
            // keeps working and a later save can unregister it, but report the
            // configured shortcut as NOT registered so the UI surfaces the
            // conflict (Grinch #714 LOW): `status.registered` describes the
            // configured shortcut, not the internal fallback.
            let msg = e.to_string();
            runtime.status = hotkey_status_after_attempt(configured, Err(msg.as_str()));
            Err(msg)
        }
    }
}

/// Compute the frontend-facing hotkey status after a registration attempt. Pure,
/// so the "`registered` describes the CONFIGURED shortcut" invariant is
/// unit-testable: on failure `registered` is false and `error` is set even when an
/// internal fallback shortcut is still alive, which is what lets the sidebar's
/// `!registered && error` check warn that the configured combo does nothing.
fn hotkey_status_after_attempt(
    configured: &str,
    outcome: Result<(), &str>,
) -> ScreenshotHotkeyStatus {
    match outcome {
        Ok(()) => ScreenshotHotkeyStatus {
            configured: configured.to_string(),
            registered: true,
            error: None,
        },
        Err(msg) => ScreenshotHotkeyStatus {
            configured: configured.to_string(),
            registered: false,
            error: Some(msg.to_string()),
        },
    }
}

// ── Hotkey entry point ─────────────────────────────────────────────────────

/// Outcome of a `begin_capture` attempt. `Busy` is the typed no-op path for a
/// debounced repeat press (a capture is already starting/active/finishing): the
/// hotkey handler must NOT surface it, because bringing the app forward mid-flight
/// would pollute the frozen screenshot and show a bogus failure. Only a genuine
/// `Err` is surfaced.
pub enum BeginOutcome {
    Started,
    Busy,
}

/// Global-shortcut handler entry. Spawns the async capture and, on any genuine
/// failure that happens before overlays exist, surfaces the error visibly because
/// the hotkey normally fires while another app is focused. A `Busy` outcome (a
/// rapid repeat press while a capture is in flight) is a silent no-op.
pub fn begin_capture_from_hotkey(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        match begin_capture(app.clone()).await {
            Ok(BeginOutcome::Started) => {}
            Ok(BeginOutcome::Busy) => {
                log::debug!("[screenshot] hotkey ignored: a capture is already in progress");
            }
            Err(e) => surface_hotkey_failure(&app, &e),
        }
    });
}

/// Emit the failure event AND bring the app forward so the toast is actually
/// visible when the hotkey fired over another foreground app.
fn surface_hotkey_failure(app: &AppHandle, message: &str) {
    log::warn!("[screenshot] capture failed before overlay: {message}");
    let _ = app.emit(
        FAILED_EVENT,
        ScreenshotCaptureEvent {
            message: message.to_string(),
        },
    );
    if let Some(win) = app
        .get_webview_window("main")
        .or_else(|| app.get_webview_window("sidebar"))
    {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        let _ = win.request_user_attention(Some(tauri::UserAttentionType::Critical));
    }
}

// ── Capture ────────────────────────────────────────────────────────────────

/// Resolve active session, capture all monitors, store `Active`, open overlays.
/// Every early-return path leaves the lifecycle `Idle` (no leaked `Starting`).
/// Returns `BeginOutcome::Busy` (a no-op, not an error) when a capture is already
/// in flight, so a debounced repeat hotkey press is silently ignored.
pub async fn begin_capture(app: AppHandle) -> Result<BeginOutcome, String> {
    // 0. Busy pre-gate. If a capture is already in flight, bail out as a typed
    //    Busy no-op BEFORE any fallible active-session/CWD resolution below. A
    //    debounced repeat press during an in-flight capture whose active session
    //    has since been cleared/deleted or switched to an unresolvable CWD would
    //    otherwise surface an Err from that resolution and raise AC forward
    //    mid-capture (polluting the frozen screenshot and showing a bogus
    //    failure). The locked reserve in step 2 stays the race-closing
    //    compare-and-set for the Idle path.
    {
        let state = app.state::<ScreenshotCaptureState>();
        if let Some(outcome) = busy_pregate(state.inner()).await {
            return Ok(outcome);
        }
    }

    // 1. Resolve the active session and its owning replica root.
    let selection = app
        .state::<crate::session::selection::SelectionCoordinator>()
        .snapshot()
        .await?;
    let session_mgr = app.state::<std::sync::Arc<tokio::sync::RwLock<SessionManager>>>();
    let sessions = session_mgr.read().await.list_sessions().await;
    let active_id = resolve_live_capture_session(&selection, &sessions)?;
    let session = session_mgr
        .read()
        .await
        .get_session(active_id)
        .await
        .ok_or_else(|| "The active session could not be found".to_string())?;

    let target_directory = resolve_session_replica_root(&session.working_directory)?;
    let session_id = session.id;
    let session_name = session.name.clone();

    // 2. Reserve `Starting` under the mutex BEFORE the expensive capture so a
    //    second rapid hotkey press is rejected instead of double-capturing.
    let capture_id = Uuid::new_v4();
    {
        let state = app.state::<ScreenshotCaptureState>();
        let mut guard = state.lock().await;
        if !matches!(&*guard, ScreenshotCaptureLifecycle::Idle) {
            // A capture is already starting/active/finishing: a debounced repeat
            // press. No-op (do NOT surface a failure or raise the window, which
            // would pollute the in-flight frozen capture).
            return Ok(BeginOutcome::Busy);
        }
        *guard = ScreenshotCaptureLifecycle::Starting(ScreenshotCaptureStarting {
            id: capture_id,
            target_session_id: session_id,
            target_session_name: session_name.clone(),
            target_directory: target_directory.clone(),
            created_at: Utc::now(),
        });
    }

    // 3. Capture every monitor off the async thread (xcap + PNG encode are sync).
    let monitors = match capture_all_monitors().await {
        Ok(m) if !m.is_empty() => m,
        Ok(_) => {
            reset_starting_if_matches(&app, capture_id).await;
            return Err("No monitors were found to capture".to_string());
        }
        Err(e) => {
            reset_starting_if_matches(&app, capture_id).await;
            return Err(e);
        }
    };

    // 4. Compute overlay placement, then swap `Starting` -> `Active` iff our
    //    capture is still the pending one.
    let placements = build_placements(&app, capture_id, &monitors);
    let overlay_labels: Vec<String> = placements.iter().map(|p| p.label.clone()).collect();
    {
        let state = app.state::<ScreenshotCaptureState>();
        let mut guard = state.lock().await;
        let still_ours =
            matches!(&*guard, ScreenshotCaptureLifecycle::Starting(s) if s.id == capture_id);
        if !still_ours {
            // Superseded/cancelled while capturing: do not open overlays. This is
            // a no-op, not a visible failure.
            log::debug!("[screenshot] capture {capture_id} superseded before overlays opened");
            return Ok(BeginOutcome::Busy);
        }
        *guard = ScreenshotCaptureLifecycle::Active(ActiveScreenshotCapture {
            id: capture_id,
            target_session_id: session_id,
            target_session_name: session_name,
            target_directory,
            monitors,
            overlay_labels,
            created_at: Utc::now(),
        });
    }

    // 5. Open overlays AFTER the capture is stored so overlay IPC sees `Active`.
    if let Err(e) = open_overlay_windows(&app, capture_id, &placements) {
        let state = app.state::<ScreenshotCaptureState>();
        let _ = clear_capture_and_destroy_overlays(
            &app,
            state.inner(),
            Some(capture_id),
            &format!("overlay build failed: {e}"),
        )
        .await;
        return Err(e);
    }
    Ok(BeginOutcome::Started)
}

fn resolve_live_capture_session(
    selection: &crate::session::selection::SessionSelection,
    sessions: &[crate::session::session::SessionInfo],
) -> Result<Uuid, String> {
    if selection.mode() != crate::session::selection::SelectionMode::Live
        || !selection.displayable()
    {
        return Err("No active session is selected in AgentsCommander".to_string());
    }
    let id = selection
        .id()
        .ok_or_else(|| "No active session is selected in AgentsCommander".to_string())?;
    sessions
        .iter()
        .any(|session| session.id == id.to_string())
        .then_some(id)
        .ok_or_else(|| "The active session could not be found".to_string())
}

/// Busy pre-gate for [`begin_capture`]: lock the lifecycle and return
/// `Some(BeginOutcome::Busy)` when a capture is already in flight (any non-Idle
/// state: `Starting`/`Active`/`Finishing`). Returns `None` only for `Idle`, so
/// the caller may proceed to active-session/CWD resolution.
///
/// This MUST run before any fallible active-session/CWD resolution in
/// `begin_capture`; otherwise a second hotkey press during an in-flight capture
/// whose active session has since been cleared/deleted or switched to an
/// unresolvable CWD would return a visible `Err` from that resolution and raise
/// AgentsCommander forward mid-capture instead of being a silent `Busy` no-op.
/// Taking only `&ScreenshotCaptureState` (no `AppHandle`/`SessionManager`) keeps
/// the before-resolution ordering structurally unit-testable.
async fn busy_pregate(state: &ScreenshotCaptureState) -> Option<BeginOutcome> {
    let guard = state.lock().await;
    if matches!(&*guard, ScreenshotCaptureLifecycle::Idle) {
        None
    } else {
        Some(BeginOutcome::Busy)
    }
}

async fn reset_starting_if_matches(app: &AppHandle, id: Uuid) {
    let state = app.state::<ScreenshotCaptureState>();
    let mut guard = state.lock().await;
    if matches!(&*guard, ScreenshotCaptureLifecycle::Starting(s) if s.id == id) {
        *guard = ScreenshotCaptureLifecycle::Idle;
    }
}

/// Capture every monitor with `xcap` and encode a base64 PNG preview per monitor.
/// All `xcap` getters are fallible and re-query the OS, so each is read once.
async fn capture_all_monitors() -> Result<Vec<CapturedMonitor>, String> {
    tokio::task::spawn_blocking(|| -> Result<Vec<CapturedMonitor>, String> {
        let monitors = xcap::Monitor::all().map_err(|e| format!("xcap monitor enumeration failed: {e}"))?;
        let mut captured = Vec::with_capacity(monitors.len());
        for (idx, monitor) in monitors.iter().enumerate() {
            let image = monitor
                .capture_image()
                .map_err(|e| format!("monitor {idx} capture failed: {e}"))?;
            let x = monitor.x().map_err(|e| e.to_string())?;
            let y = monitor.y().map_err(|e| e.to_string())?;
            let scale_factor = monitor.scale_factor().map_err(|e| e.to_string())? as f64;
            let name = monitor.name().unwrap_or_else(|_| format!("monitor-{idx}"));
            // The captured bitmap is the authoritative size for crop math; do not
            // trust monitor.width()/height() for it (plan §B, dev-rust §B).
            let width = image.width();
            let height = image.height();
            let preview_data_url = encode_preview_data_url(&image, width, height)?;
            log::info!(
                "[screenshot] monitor {idx} '{name}' origin=({x},{y}) size={width}x{height} scale={scale_factor}"
            );
            captured.push(CapturedMonitor {
                monitor_id: idx as u32,
                name,
                x,
                y,
                width,
                height,
                scale_factor,
                image,
                preview_data_url,
            });
        }
        Ok(captured)
    })
    .await
    .map_err(|e| format!("monitor capture task panicked: {e}"))?
}

/// PNG-encode the RGBA buffer (no extra clone of the image) into a data URL.
fn encode_preview_data_url(
    image: &image::RgbaImage,
    width: u32,
    height: u32,
) -> Result<String, String> {
    let mut png_bytes: Vec<u8> = Vec::new();
    PngEncoder::new(&mut png_bytes)
        .write_image(image.as_raw(), width, height, ExtendedColorType::Rgba8)
        .map_err(|e| format!("PNG preview encode failed: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Ok(format!("data:image/png;base64,{b64}"))
}

// ── Overlay windows ────────────────────────────────────────────────────────

struct OverlayPlacement {
    label: String,
    monitor_id: u32,
    /// Physical pixels.
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
}

/// Decide where each overlay goes. Prefer Tauri's own monitor geometry (physical
/// position/size + per-monitor scale) matched to the xcap monitor by name, then
/// by nearest origin; fall back to the xcap geometry if no Tauri monitor matches.
/// Placement is decoupled from xcap coordinate semantics this way (plan §C).
fn build_placements(
    app: &AppHandle,
    capture_id: Uuid,
    monitors: &[CapturedMonitor],
) -> Vec<OverlayPlacement> {
    let tauri_monitors = app.available_monitors().unwrap_or_default();
    monitors
        .iter()
        .map(|m| {
            let (x, y, width, height, scale_factor) = match_tauri_monitor(&tauri_monitors, m)
                .map(|tm| {
                    let pos = tm.position();
                    let size = tm.size();
                    (pos.x, pos.y, size.width, size.height, tm.scale_factor())
                })
                .unwrap_or((m.x, m.y, m.width, m.height, m.scale_factor));
            if width != m.width || height != m.height {
                log::warn!(
                    "[screenshot] monitor {} placement size {}x{} differs from captured bitmap {}x{}; using placement for window, bitmap for crop",
                    m.monitor_id, width, height, m.width, m.height
                );
            }
            OverlayPlacement {
                label: overlay_label(capture_id, m.monitor_id),
                monitor_id: m.monitor_id,
                x,
                y,
                width,
                height,
                scale_factor,
            }
        })
        .collect()
}

/// Match an xcap monitor to a Tauri monitor by name, else by nearest origin.
fn match_tauri_monitor<'a>(
    tauri_monitors: &'a [tauri::Monitor],
    captured: &CapturedMonitor,
) -> Option<&'a tauri::Monitor> {
    if let Some(by_name) = tauri_monitors
        .iter()
        .find(|tm| tm.name().map(|n| n.as_str()) == Some(captured.name.as_str()))
    {
        return Some(by_name);
    }
    tauri_monitors.iter().min_by_key(|tm| {
        let pos = tm.position();
        let dx = (pos.x - captured.x).unsigned_abs() as u64;
        let dy = (pos.y - captured.y).unsigned_abs() as u64;
        dx + dy
    })
}

fn overlay_label(capture_id: Uuid, monitor_id: u32) -> String {
    format!("screenshot-overlay-{}-{}", capture_id.simple(), monitor_id)
}

/// Build one transparent, always-on-top overlay per monitor. Mirrors the
/// Resource Monitor builder (`commands::window`): logical inner size/position from
/// the monitor scale, corrected to physical bounds post-build. On any failure the
/// caller clears state and destroys already-created overlays.
fn open_overlay_windows(
    app: &AppHandle,
    capture_id: Uuid,
    placements: &[OverlayPlacement],
) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    for p in placements {
        let scale = if p.scale_factor > 0.0 {
            p.scale_factor
        } else {
            1.0
        };
        let url = format!(
            "index.html?window=screenshot-overlay&captureId={capture_id}&monitorId={}",
            p.monitor_id
        );
        let window = crate::apply_isolated_webview_data_directory(|| {
            WebviewWindowBuilder::new(app, &p.label, WebviewUrl::App(url.into()))
        })
        .map_err(|error| error.to_string())?
        .title("Screenshot Capture")
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(true)
        .zoom_hotkeys_enabled(false)
        .inner_size(p.width as f64 / scale, p.height as f64 / scale)
        .position(p.x as f64 / scale, p.y as f64 / scale)
        .build()
        .map_err(|e| format!("failed to create overlay '{}': {e}", p.label))?;

        window
            .set_size(tauri::Size::Physical(tauri::PhysicalSize {
                width: p.width,
                height: p.height,
            }))
            .map_err(|e| format!("failed to size overlay '{}': {e}", p.label))?;
        window
            .set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                x: p.x,
                y: p.y,
            }))
            .map_err(|e| format!("failed to position overlay '{}': {e}", p.label))?;
    }
    Ok(())
}

/// Destroy every overlay window belonging to a capture id (by label prefix).
/// Idempotent: missing windows are skipped. Returns how many were destroyed.
fn destroy_overlays_for_capture(app: &AppHandle, capture_id: Uuid) -> usize {
    let prefix = format!("screenshot-overlay-{}-", capture_id.simple());
    let mut destroyed = 0;
    for (label, window) in app.webview_windows() {
        if label.starts_with(&prefix) {
            match window.destroy() {
                Ok(()) => destroyed += 1,
                Err(e) => log::warn!("[screenshot] failed to destroy overlay '{label}': {e}"),
            }
        }
    }
    destroyed
}

// ── Overlay IPC ────────────────────────────────────────────────────────────

enum OverlayDecision {
    Ready(Box<ScreenshotOverlayState>),
    Starting,
    Stale,
}

/// Return overlay state for an `Active` capture matching `capture_id`. While the
/// capture is still `Starting`, report that. For a stale id, destroy that id's
/// overlays without touching a different active capture.
pub async fn overlay_state(
    app: &AppHandle,
    state: &ScreenshotCaptureState,
    capture_id: &str,
    monitor_id: u32,
) -> Result<ScreenshotOverlayState, String> {
    let decision = {
        let guard = state.lock().await;
        match &*guard {
            ScreenshotCaptureLifecycle::Active(c) if c.id.to_string() == capture_id => {
                match c.monitors.iter().find(|m| m.monitor_id == monitor_id) {
                    Some(m) => OverlayDecision::Ready(Box::new(ScreenshotOverlayState {
                        capture_id: c.id.to_string(),
                        monitor_id: m.monitor_id,
                        monitor_x: m.x,
                        monitor_y: m.y,
                        width: m.width,
                        height: m.height,
                        scale_factor: m.scale_factor,
                        image_data_url: m.preview_data_url.clone(),
                        session_id: c.target_session_id.to_string(),
                        session_name: c.target_session_name.clone(),
                        // Strip the Windows `\\?\` verbatim prefix that `canonicalize`
                        // leaves on the resolved root, so the overlay shows a clean save
                        // location (shared #730 helper; internal PathBuf checks keep the
                        // raw canonical value).
                        target_directory:
                            crate::path_utils::path_to_string_without_windows_verbatim_prefix(
                                &c.target_directory,
                            ),
                    })),
                    None => OverlayDecision::Stale,
                }
            }
            ScreenshotCaptureLifecycle::Starting(s) if s.id.to_string() == capture_id => {
                OverlayDecision::Starting
            }
            _ => OverlayDecision::Stale,
        }
    };

    match decision {
        OverlayDecision::Ready(state) => Ok(*state),
        OverlayDecision::Starting => Err("Screenshot capture is still starting".to_string()),
        OverlayDecision::Stale => {
            if let Ok(id) = Uuid::parse_str(capture_id) {
                destroy_overlays_for_capture(app, id);
            }
            Err("Screenshot capture is no longer active".to_string())
        }
    }
}

// ── Confirm ────────────────────────────────────────────────────────────────

/// Crop the selection out of the stored monitor bitmap, save a PNG into the
/// resolved replica root, copy the path to the clipboard, and tear down overlays.
/// The async mutex is dropped before any file I/O. Any post-take failure destroys
/// overlays, deletes a partial PNG, and emits a failure event.
pub async fn confirm_selection(
    app: &AppHandle,
    state: &ScreenshotCaptureState,
    selection: ScreenshotSelection,
) -> Result<ScreenshotCaptureResult, String> {
    // Take the active capture iff it matches, and move to `Finishing` (NOT `Idle`)
    // so a new hotkey capture cannot start and photograph the still-visible
    // overlays while we save and tear down. The guard is dropped before file I/O.
    let capture = {
        let mut guard = state.lock().await;
        let is_match = matches!(
            &*guard,
            ScreenshotCaptureLifecycle::Active(c) if c.id.to_string() == selection.capture_id
        );
        if !is_match {
            return Err("Screenshot capture is no longer active".to_string());
        }
        let ScreenshotCaptureLifecycle::Active(c) =
            std::mem::replace(&mut *guard, ScreenshotCaptureLifecycle::Idle)
        else {
            unreachable!("matched Active above")
        };
        // Install the finishing gate keyed by this capture's id; the transient
        // `Idle` above is never observed because the lock is still held.
        *guard = ScreenshotCaptureLifecycle::Finishing(c.id);
        c
    };

    let capture_id = capture.id;
    let result = save_selection(capture, &selection).await;
    let outcome = match result {
        Ok(saved) => {
            // Best-effort clipboard copy; the PNG already exists, so a clipboard
            // failure must not fail the capture (plan §E).
            if let Err(e) = app.clipboard().write_text(saved.path.clone()) {
                log::warn!("[screenshot] clipboard copy failed: {e}");
            }
            let _ = app.emit(SAVED_EVENT, saved.clone());
            destroy_overlays_for_capture(app, capture_id);
            Ok(saved)
        }
        Err((partial, msg)) => {
            if let Some(path) = partial {
                let _ = std::fs::remove_file(&path);
            }
            destroy_overlays_for_capture(app, capture_id);
            let _ = app.emit(
                FAILED_EVENT,
                ScreenshotCaptureEvent {
                    message: msg.clone(),
                },
            );
            Err(msg)
        }
    };

    // Overlays are destroyed; release the finishing gate so the next hotkey press
    // can start a fresh capture. Runs on both success and failure.
    finish_capture_to_idle(state, capture_id).await;
    outcome
}

/// Release the `Finishing` gate installed by `confirm_selection` once its overlays
/// are destroyed, returning the lifecycle to `Idle`. Guarded by id so it never
/// clobbers a state some other path already changed.
async fn finish_capture_to_idle(state: &ScreenshotCaptureState, capture_id: Uuid) {
    let mut guard = state.lock().await;
    if matches!(&*guard, ScreenshotCaptureLifecycle::Finishing(id) if *id == capture_id) {
        *guard = ScreenshotCaptureLifecycle::Idle;
    }
}

/// Validate the selection, crop, and write the PNG. On error the second tuple
/// element is the message and the first is a partial file to delete, if any.
async fn save_selection(
    capture: ActiveScreenshotCapture,
    selection: &ScreenshotSelection,
) -> Result<ScreenshotCaptureResult, (Option<PathBuf>, String)> {
    let monitor = capture
        .monitors
        .into_iter()
        .find(|m| m.monitor_id == selection.monitor_id)
        .ok_or_else(|| {
            (
                None,
                format!("monitor {} not in capture", selection.monitor_id),
            )
        })?;

    let (iw, ih) = (monitor.image.width(), monitor.image.height());
    validate_selection(selection, iw, ih).map_err(|e| (None, e))?;

    let target = capture.target_directory.clone();
    let filename = build_filename(capture.target_session_id);
    let image = monitor.image;
    let (sx, sy, sw, sh) = (selection.x, selection.y, selection.width, selection.height);

    let saved_path =
        tokio::task::spawn_blocking(move || -> Result<PathBuf, (Option<PathBuf>, String)> {
            // Revalidate the destination immediately before creating the file.
            revalidate_replica_target(&target).map_err(|e| (None, e))?;
            let (path, file) = create_png_file(&target, &filename).map_err(|e| (None, e))?;

            let cropped = image::imageops::crop_imm(&image, sx, sy, sw, sh).to_image();
            let mut writer = std::io::BufWriter::new(file);
            if let Err(e) = image::DynamicImage::ImageRgba8(cropped)
                .write_to(&mut writer, image::ImageFormat::Png)
            {
                return Err((Some(path), format!("PNG encode/write failed: {e}")));
            }
            if let Err(e) = writer.flush() {
                return Err((Some(path), format!("PNG flush failed: {e}")));
            }
            drop(writer);

            // Final containment check: the canonical saved file must live directly
            // inside the canonical replica root.
            let canonical = std::fs::canonicalize(&path).map_err(|e| {
                (
                    Some(path.clone()),
                    format!("canonicalize saved file failed: {e}"),
                )
            })?;
            let canonical_target = std::fs::canonicalize(&target).map_err(|e| {
                (
                    Some(canonical.clone()),
                    format!("canonicalize target failed: {e}"),
                )
            })?;
            let parent_ok = canonical
                .parent()
                .map(|p| p == canonical_target)
                .unwrap_or(false);
            if !parent_ok {
                return Err((
                    Some(canonical),
                    "saved file escaped the replica root".to_string(),
                ));
            }
            Ok(canonical)
        })
        .await
        .map_err(|e| (None, format!("save task panicked: {e}")))??;

    // `saved_path` is canonical (verbatim `\\?\` on Windows). Every containment
    // check above ran on the raw canonical PathBuf; only the user-facing string
    // (clipboard, saved event, result payload, log) is normalized via the shared
    // #730 helper so it matches the clean paths the rest of the app now emits.
    let path_string =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(&saved_path);
    log::info!(
        "[screenshot] saved {} for session '{}'",
        path_string,
        capture.target_session_name
    );
    Ok(ScreenshotCaptureResult {
        path: path_string,
        session_id: capture.target_session_id.to_string(),
        session_name: capture.target_session_name,
    })
}

fn validate_selection(
    selection: &ScreenshotSelection,
    image_w: u32,
    image_h: u32,
) -> Result<(), String> {
    if selection.width < 2 || selection.height < 2 {
        return Err("Selection is too small".to_string());
    }
    let x_end = selection.x.checked_add(selection.width);
    let y_end = selection.y.checked_add(selection.height);
    match (x_end, y_end) {
        (Some(xe), Some(ye)) if xe <= image_w && ye <= image_h => Ok(()),
        _ => Err("Selection is outside the captured image".to_string()),
    }
}

fn build_filename(session_id: Uuid) -> String {
    let short = &session_id.simple().to_string()[..8];
    // Local time: the filename is user-facing, so it should match the user's clock.
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    format!("agentscommander-screenshot-{stamp}-{short}.png")
}

/// Collision loop mirroring `phone::messaging::create_message_file`: never
/// overwrite an existing file; retry with `-<n>.png` up to the bound.
fn create_png_file(dir: &Path, base_filename: &str) -> Result<(PathBuf, std::fs::File), String> {
    let stem = base_filename
        .strip_suffix(".png")
        .ok_or_else(|| format!("filename '{base_filename}' must end in .png"))?;
    for n in 0..=MAX_COLLISION_SUFFIX {
        let filename = if n == 0 {
            base_filename.to_string()
        } else {
            format!("{stem}-{n}.png")
        };
        let path = dir.join(&filename);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("failed to create {}: {e}", path.display())),
        }
    }
    Err(format!(
        "exhausted {MAX_COLLISION_SUFFIX} filename collisions for '{base_filename}'"
    ))
}

// ── Cancel + cleanup ───────────────────────────────────────────────────────

/// Explicit cancel (Escape / overlay close request). Idempotent.
pub async fn cancel_capture(
    app: &AppHandle,
    state: &ScreenshotCaptureState,
    capture_id: &str,
) -> Result<(), String> {
    let id = Uuid::parse_str(capture_id).map_err(|_| "invalid capture id".to_string())?;
    clear_capture_and_destroy_overlays(app, state, Some(id), "user cancelled").await
}

/// Centralized, idempotent cleanup: take a matching `Starting`/`Active` capture
/// to `Idle` and destroy its overlays by label prefix. `capture_id == None`
/// clears whatever is active. Safe to call repeatedly; later calls are no-ops.
pub async fn clear_capture_and_destroy_overlays(
    app: &AppHandle,
    state: &ScreenshotCaptureState,
    capture_id: Option<Uuid>,
    reason: &str,
) -> Result<(), String> {
    let cleared_id = {
        let mut guard = state.lock().await;
        let id = match &*guard {
            ScreenshotCaptureLifecycle::Active(c)
                if capture_id.is_none() || capture_id == Some(c.id) =>
            {
                Some(c.id)
            }
            ScreenshotCaptureLifecycle::Starting(s)
                if capture_id.is_none() || capture_id == Some(s.id) =>
            {
                Some(s.id)
            }
            _ => None,
        };
        if id.is_some() {
            *guard = ScreenshotCaptureLifecycle::Idle;
        }
        id
    };

    // Destroy overlays for whichever id we cleared, or for the requested id even
    // if the state was already idle (scrubs windows left by a prior partial run).
    let destroy_id = cleared_id.or(capture_id);
    if let Some(id) = destroy_id {
        let n = destroy_overlays_for_capture(app, id);
        if n > 0 || cleared_id.is_some() {
            log::info!("[screenshot] cleared capture {id} ({reason}); destroyed {n} overlay(s)");
        }
    }
    Ok(())
}

/// `RunEvent::WindowEvent::Destroyed` hook for `screenshot-overlay-*` labels.
/// If the destroyed overlay belongs to the live capture, clear state and destroy
/// its siblings; otherwise no-op (our own teardown already cleared state).
pub async fn handle_overlay_window_destroyed(app: AppHandle, label: String) {
    let Some(hex) = capture_hex_from_overlay_label(&label) else {
        return;
    };
    let state = app.state::<ScreenshotCaptureState>();
    let active_id = {
        let guard = state.lock().await;
        match &*guard {
            ScreenshotCaptureLifecycle::Active(c) => Some(c.id),
            ScreenshotCaptureLifecycle::Starting(s) => Some(s.id),
            // Finishing: `confirm_selection` owns teardown and will reset to
            // `Idle`, so the Destroyed events it triggers must not re-enter cleanup.
            ScreenshotCaptureLifecycle::Finishing(_) => None,
            ScreenshotCaptureLifecycle::Idle => None,
        }
    };
    if let Some(id) = active_id {
        if id.simple().to_string() == hex {
            let _ = clear_capture_and_destroy_overlays(
                &app,
                state.inner(),
                Some(id),
                "overlay window destroyed",
            )
            .await;
        }
    }
}

fn capture_hex_from_overlay_label(label: &str) -> Option<&str> {
    let rest = label.strip_prefix("screenshot-overlay-")?;
    let (hex, _idx) = rest.rsplit_once('-')?;
    if hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(hex)
    } else {
        None
    }
}

// ── Replica-root resolution + symlink guard ────────────────────────────────

/// True iff `meta` is a symlink or (Windows) any reparse point (junction).
/// Inlined from the private `session_context::is_link_or_reparse` per its own
/// doc-comment, which asks callers to inline rather than widen its API.
fn is_link_or_reparse(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Walk up `start` to the owning `wg-*/__agent_*` replica directory by name.
/// Pure path logic (no filesystem access): a session launched from
/// `__agent_x\child` resolves to `__agent_x`. Returns `None` for ad-hoc CWDs,
/// `repo-*` siblings, project roots, and root-agent paths.
fn find_replica_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("__agent_") {
                let parent_is_wg = dir
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .map(|p| p.starts_with("wg-"))
                    .unwrap_or(false);
                if parent_is_wg {
                    return Some(dir.to_path_buf());
                }
            }
        }
        current = dir.parent();
    }
    None
}

/// Resolve and validate the only legal write target for a capture: the canonical
/// owning WG replica root of the active session's CWD. Rejects non-replica CWDs,
/// symlinks/reparse points, and roots that fail WG replica identity validation.
fn resolve_session_replica_root(working_directory: &str) -> Result<PathBuf, String> {
    // Canonicalize the session CWD BEFORE walking ancestors so `..` and symlink
    // components cannot spoof a `__agent_*` ancestor. Example spoof:
    // `...\wg-6\__agent_x\..\repo-AgentsCommander` resolves to the repo sibling
    // (which must be rejected), yet a raw name-walk would see the `__agent_x`
    // ancestor and save there. Canonicalize requires the path to exist, which is
    // always true for a live session's CWD; a missing/unresolvable CWD is rejected.
    let canonical_cwd = std::fs::canonicalize(working_directory)
        .map_err(|e| format!("Cannot resolve the active session directory: {e}"))?;
    let root = find_replica_root(&canonical_cwd).ok_or_else(|| {
        "The active session is not inside a workgroup replica directory, so there is no place to save the screenshot".to_string()
    })?;

    let meta = std::fs::symlink_metadata(&root)
        .map_err(|e| format!("Cannot read the replica directory: {e}"))?;
    if is_link_or_reparse(&meta) {
        return Err("The replica directory is a symlink or reparse point".to_string());
    }
    if !meta.is_dir() {
        return Err("The replica path is not a directory".to_string());
    }

    let canonical = std::fs::canonicalize(&root)
        .map_err(|e| format!("Cannot canonicalize the replica directory: {e}"))?;

    // Structure + persisted-identity validation against the existing helpers.
    crate::config::replica_identity::expected_wg_replica_identity(&canonical)?;
    crate::config::replica_identity::read_wg_replica_config_read_only(&canonical)?;
    Ok(canonical)
}

/// Re-run the FULL start-time validation on the target immediately before creating
/// the PNG, because the directory could have changed since capture start. This is
/// the symlink/reparse + directory guard, plus a re-canonicalization and a re-run
/// of the WG replica structure + persisted-identity checks. It closes the window
/// where the replica root is deleted and recreated as a plain, non-replica
/// directory between capture and confirm (which the final parent-containment check
/// alone would still accept). `target` is the canonical root resolved at start.
fn revalidate_replica_target(target: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(target)
        .map_err(|e| format!("Cannot read the replica directory: {e}"))?;
    if is_link_or_reparse(&meta) {
        return Err("The replica directory is a symlink or reparse point".to_string());
    }
    if !meta.is_dir() {
        return Err("The replica path is not a directory".to_string());
    }

    // `target` was already canonical at start; if re-canonicalizing yields a
    // different path the directory was replaced under us, so reject.
    let canonical = std::fs::canonicalize(target)
        .map_err(|e| format!("Cannot canonicalize the replica directory: {e}"))?;
    if canonical.as_path() != target {
        return Err("The replica directory changed between capture and save".to_string());
    }

    // Same structure + persisted-identity validation used at capture start.
    crate::config::replica_identity::expected_wg_replica_identity(&canonical)?;
    crate::config::replica_identity::read_wg_replica_config_read_only(&canonical)?;
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_capture_resolution_rejects_none_and_dormant_before_accepting_live() {
        let manager = crate::session::manager::SessionManager::new();
        let session = manager
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/work".to_string(),
                None,
                None,
                Vec::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("capture session fixture");
        let rows = manager.list_sessions().await;
        let none = crate::session::selection::SessionSelection::none_for_test();
        let dormant = crate::session::selection::SessionSelection::dormant_for_test(session.id, 7);
        let live = crate::session::selection::SessionSelection::live_for_test(session.id);

        assert_eq!(
            resolve_live_capture_session(&none, &rows).unwrap_err(),
            "No active session is selected in AgentsCommander"
        );
        assert_eq!(
            resolve_live_capture_session(&dormant, &rows).unwrap_err(),
            "No active session is selected in AgentsCommander"
        );
        assert_eq!(
            resolve_live_capture_session(&live, &rows).unwrap(),
            session.id
        );
    }

    #[test]
    fn shortcut_conversion_maps_modifier_and_code() {
        let parsed = super::super::parse_screenshot_hotkey("Ctrl+Q").unwrap();
        let shortcut = parsed_to_shortcut(&parsed).unwrap();
        assert_eq!(shortcut.mods, Modifiers::CONTROL);
        assert_eq!(shortcut.key, Code::KeyQ);
    }

    #[test]
    fn shortcut_conversion_maps_digit() {
        let parsed = super::super::parse_screenshot_hotkey("Ctrl+1").unwrap();
        let shortcut = parsed_to_shortcut(&parsed).unwrap();
        assert_eq!(shortcut.key, Code::Digit1);
    }

    #[test]
    fn find_replica_root_accepts_replica_and_subdirs() {
        let replica = Path::new(r"C:\proj\.ac\wg-6-team\__agent_dev_rust");
        assert_eq!(find_replica_root(replica).as_deref(), Some(replica));

        let child = Path::new(r"C:\proj\.ac\wg-6-team\__agent_dev_rust\sub\deeper");
        assert_eq!(
            find_replica_root(child).as_deref(),
            Some(Path::new(r"C:\proj\.ac\wg-6-team\__agent_dev_rust"))
        );
    }

    #[test]
    fn find_replica_root_rejects_non_replica_paths() {
        // Ad-hoc CWD.
        assert!(find_replica_root(Path::new(r"C:\tmp")).is_none());
        // repo-* workgroup sibling (parent is wg-*, but name is not __agent_*).
        assert!(
            find_replica_root(Path::new(r"C:\proj\.ac\wg-6-team\repo-AgentsCommander")).is_none()
        );
        // __agent_* whose parent is NOT a wg-* folder.
        assert!(find_replica_root(Path::new(r"C:\proj\__agent_x")).is_none());
        // Root-agent directory name.
        assert!(find_replica_root(Path::new(r"C:\proj\.ac\ac-root-agent")).is_none());
    }

    #[test]
    fn resolve_rejects_missing_and_non_replica() {
        // Non-replica path: rejected before any filesystem access.
        assert!(resolve_session_replica_root(r"C:\definitely\not\a\replica").is_err());
        // Replica-shaped but missing on disk: symlink_metadata fails.
        let missing = format!(r"C:\proj\.ac\wg-6-team\__agent_{}", Uuid::new_v4().simple());
        assert!(resolve_session_replica_root(&missing).is_err());
    }

    #[test]
    fn resolve_rejects_regular_file() {
        // A file shaped like a replica root must be rejected (is_dir == false).
        let dir = std::env::temp_dir().join(format!("wg-6-team-{}", Uuid::new_v4().simple()));
        let replica = dir.join("__agent_filecase");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&replica, b"not a dir").unwrap();
        let result = resolve_session_replica_root(&replica.to_string_lossy());
        let _ = std::fs::remove_file(&replica);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_rejects_dotdot_spoof_to_repo_sibling() {
        // A CWD that walks INTO the replica then back out via `..` into a `repo-*`
        // workgroup sibling must resolve (canonically) to the repo and be rejected,
        // not spoof the `__agent_*` ancestor via a raw path-name walk (Grinch #714
        // MEDIUM). Requires real dirs so `canonicalize` can resolve `..`.
        let base = std::env::temp_dir().join(format!("ac-shot-spoof-{}", Uuid::new_v4().simple()));
        let wg = base.join("wg-6-dev-team");
        let replica = wg.join("__agent_x");
        let repo = wg.join("repo-foo");
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::create_dir_all(&repo).unwrap();

        let spoof = replica.join("..").join("repo-foo");
        let result = resolve_session_replica_root(&spoof.to_string_lossy());

        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "`..` spoof into a repo sibling must be rejected, got {result:?}"
        );
    }

    #[test]
    fn resolve_rejects_missing_child_of_replica() {
        // The replica exists, but the session CWD points to a now-missing child.
        // With no existing path to canonicalize, resolution must fail rather than
        // fall back to the raw replica ancestor (Grinch #714 MEDIUM).
        let base =
            std::env::temp_dir().join(format!("ac-shot-missing-{}", Uuid::new_v4().simple()));
        let replica = base.join("wg-6-dev-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();

        let missing_child = replica.join("gone");
        let result = resolve_session_replica_root(&missing_child.to_string_lossy());

        let _ = std::fs::remove_dir_all(&base);
        assert!(result.is_err(), "missing child CWD must be rejected");
    }

    #[test]
    fn revalidate_rejects_plain_recreated_dir() {
        // A directory shaped like a replica root but WITHOUT the WG replica config
        // / matrix (models delete + recreate-as-plain between capture and confirm)
        // must be rejected at confirm time by the re-run identity checks, even
        // though it passes the symlink/is_dir guard (Grinch #714 MEDIUM).
        let base = std::env::temp_dir().join(format!("ac-shot-reval-{}", Uuid::new_v4().simple()));
        let replica = base.join("wg-6-dev-team").join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        let canonical = std::fs::canonicalize(&replica).unwrap();

        let result = revalidate_replica_target(&canonical);

        let _ = std::fs::remove_dir_all(&base);
        assert!(
            result.is_err(),
            "plain (non-config) replica-shaped dir must be rejected at confirm"
        );
    }

    #[tokio::test]
    async fn finishing_gate_blocks_then_releases() {
        // The `Finishing` lifecycle is non-Idle, so begin_capture's Idle gate
        // rejects a new capture (returns Busy) until confirm_selection destroys the
        // overlays and releases the gate (Grinch #714 HIGH finishing race).
        let id = Uuid::new_v4();
        let state: ScreenshotCaptureState = std::sync::Arc::new(tokio::sync::Mutex::new(
            ScreenshotCaptureLifecycle::Finishing(id),
        ));

        // Non-Idle while finishing => a new hotkey press would be Busy.
        assert!(!matches!(
            &*state.lock().await,
            ScreenshotCaptureLifecycle::Idle
        ));

        // A mismatched id must NOT release the gate.
        finish_capture_to_idle(&state, Uuid::new_v4()).await;
        assert!(matches!(
            &*state.lock().await,
            ScreenshotCaptureLifecycle::Finishing(_)
        ));

        // The matching id releases the gate to Idle.
        finish_capture_to_idle(&state, id).await;
        assert!(matches!(
            &*state.lock().await,
            ScreenshotCaptureLifecycle::Idle
        ));
    }

    #[tokio::test]
    async fn finish_capture_to_idle_ignores_non_finishing() {
        // Defensive: releasing the gate never clobbers a Starting/Active state.
        let state: ScreenshotCaptureState = std::sync::Arc::new(tokio::sync::Mutex::new(
            ScreenshotCaptureLifecycle::Starting(ScreenshotCaptureStarting {
                id: Uuid::new_v4(),
                target_session_id: Uuid::new_v4(),
                target_session_name: "s".to_string(),
                target_directory: PathBuf::from("x"),
                created_at: Utc::now(),
            }),
        ));
        finish_capture_to_idle(&state, Uuid::new_v4()).await;
        assert!(matches!(
            &*state.lock().await,
            ScreenshotCaptureLifecycle::Starting(_)
        ));
    }

    #[tokio::test]
    async fn busy_pregate_returns_busy_for_non_idle_before_resolution() {
        // Idle: no short-circuit, so begin_capture proceeds to active-session/CWD
        // resolution.
        let idle: ScreenshotCaptureState =
            std::sync::Arc::new(tokio::sync::Mutex::new(ScreenshotCaptureLifecycle::Idle));
        assert!(busy_pregate(&idle).await.is_none());

        // Every non-Idle state returns Busy. Because busy_pregate takes ONLY the
        // lifecycle state (no AppHandle/SessionManager) it structurally cannot run
        // the active-session/CWD resolution begin_capture does afterwards, proving
        // the Busy no-op happens before any fallible resolution could surface an
        // Err and raise AC mid-capture (Grinch #714 HIGH pre-gate ordering).
        let starting: ScreenshotCaptureState = std::sync::Arc::new(tokio::sync::Mutex::new(
            ScreenshotCaptureLifecycle::Starting(ScreenshotCaptureStarting {
                id: Uuid::new_v4(),
                target_session_id: Uuid::new_v4(),
                target_session_name: "s".to_string(),
                target_directory: PathBuf::from("x"),
                created_at: Utc::now(),
            }),
        ));
        assert!(matches!(
            busy_pregate(&starting).await,
            Some(BeginOutcome::Busy)
        ));

        let active: ScreenshotCaptureState = std::sync::Arc::new(tokio::sync::Mutex::new(
            ScreenshotCaptureLifecycle::Active(ActiveScreenshotCapture {
                id: Uuid::new_v4(),
                target_session_id: Uuid::new_v4(),
                target_session_name: "s".to_string(),
                target_directory: PathBuf::from("x"),
                monitors: Vec::new(),
                overlay_labels: Vec::new(),
                created_at: Utc::now(),
            }),
        ));
        assert!(matches!(
            busy_pregate(&active).await,
            Some(BeginOutcome::Busy)
        ));

        let finishing: ScreenshotCaptureState = std::sync::Arc::new(tokio::sync::Mutex::new(
            ScreenshotCaptureLifecycle::Finishing(Uuid::new_v4()),
        ));
        assert!(matches!(
            busy_pregate(&finishing).await,
            Some(BeginOutcome::Busy)
        ));
    }

    #[test]
    fn hotkey_status_reflects_configured_shortcut_only() {
        // Success: the configured shortcut is active.
        let ok = hotkey_status_after_attempt("Ctrl+Q", Ok(()));
        assert_eq!(ok.configured, "Ctrl+Q");
        assert!(ok.registered);
        assert!(ok.error.is_none());

        // Failure (e.g. OS conflict): the configured shortcut is NOT active even if
        // an internal fallback stays registered. `registered` must be false and the
        // error present so the sidebar's `!registered && error` check warns that the
        // configured combo does nothing (Grinch #714 LOW).
        let err = hotkey_status_after_attempt("Ctrl+P", Err("hotkey already registered"));
        assert_eq!(err.configured, "Ctrl+P");
        assert!(!err.registered);
        assert_eq!(err.error.as_deref(), Some("hotkey already registered"));
    }

    #[test]
    fn validate_selection_bounds() {
        let sel = |x, y, w, h| ScreenshotSelection {
            capture_id: "c".to_string(),
            monitor_id: 0,
            x,
            y,
            width: w,
            height: h,
        };
        // OK: inside bounds.
        assert!(validate_selection(&sel(0, 0, 10, 10), 100, 100).is_ok());
        assert!(validate_selection(&sel(90, 90, 10, 10), 100, 100).is_ok());
        // Too small.
        assert!(validate_selection(&sel(0, 0, 1, 10), 100, 100).is_err());
        assert!(validate_selection(&sel(0, 0, 10, 1), 100, 100).is_err());
        // Out of bounds.
        assert!(validate_selection(&sel(95, 0, 10, 10), 100, 100).is_err());
        assert!(validate_selection(&sel(0, 95, 10, 10), 100, 100).is_err());
    }

    #[test]
    fn crop_returns_requested_dimensions_and_pixels() {
        // 4x4 image; top-left 2x2 is red, rest is blue. Crop (0,0,2,2) -> all red.
        let mut img = image::RgbaImage::new(4, 4);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = if x < 2 && y < 2 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            };
        }
        let crop = image::imageops::crop_imm(&img, 0, 0, 2, 2).to_image();
        assert_eq!(crop.width(), 2);
        assert_eq!(crop.height(), 2);
        assert_eq!(*crop.get_pixel(0, 0), image::Rgba([255, 0, 0, 255]));
        assert_eq!(*crop.get_pixel(1, 1), image::Rgba([255, 0, 0, 255]));

        // Crop the bottom-right 2x2 -> all blue, and origin must be respected.
        let crop_br = image::imageops::crop_imm(&img, 2, 2, 2, 2).to_image();
        assert_eq!(*crop_br.get_pixel(0, 0), image::Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn create_png_file_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("ac-shot-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();

        let (p0, f0) = create_png_file(&dir, "shot.png").unwrap();
        drop(f0);
        assert!(p0.ends_with("shot.png"));

        let (p1, f1) = create_png_file(&dir, "shot.png").unwrap();
        drop(f1);
        assert!(p1.ends_with("shot-1.png"), "got {p1:?}");
        assert_ne!(p0, p1);
        assert!(p0.exists() && p1.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn overlay_label_and_hex_round_trip() {
        let id = Uuid::new_v4();
        let label = overlay_label(id, 3);
        assert!(label.starts_with("screenshot-overlay-"));
        assert_eq!(
            capture_hex_from_overlay_label(&label),
            Some(id.simple().to_string().as_str())
        );
        assert!(capture_hex_from_overlay_label("terminal-abc").is_none());
        assert!(capture_hex_from_overlay_label("screenshot-overlay-xyz-0").is_none());
    }

    #[test]
    fn build_filename_shape() {
        let id = Uuid::new_v4();
        let name = build_filename(id);
        assert!(name.starts_with("agentscommander-screenshot-"));
        assert!(name.ends_with(".png"));
        // Contains the 8-char short id.
        assert!(name.contains(&id.simple().to_string()[..8]));
    }
}
