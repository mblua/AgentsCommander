//! #714 Native screenshot runtime (Windows / Linux-X11 / macOS).
//!
//! Compiled on Windows, Linux/X11 and macOS. Owns every native dependency for
//! the feature: `xcap` desktop capture, `image` crop/encode, the clipboard
//! plugin, the global-shortcut plugin, overlay window creation, and the capture
//! lifecycle state machine. Only the #1285 window-capture region stays
//! Windows-only. The every-other-target sibling (`super::unsupported`) mirrors
//! this public surface without any native screenshot crates.
//!
//! Cleanup invariant: Rust owns overlay teardown. Every terminal path (confirm
//! success/failure, explicit cancel, overlay build failure, stale overlay IPC,
//! and `WindowEvent::Destroyed`) routes through either `confirm_selection`'s own
//! teardown or [`clear_capture_and_destroy_overlays`], both of which are
//! idempotent because they destroy overlays by label prefix and only act when a
//! matching capture is still live.

#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use base64::Engine as _;
use chrono::Utc;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};
use uuid::Uuid;

use super::{
    HotkeyModifier, ParsedHotkey, ScreenshotCaptureEvent, ScreenshotCaptureResult,
    ScreenshotCaptureState, ScreenshotHost, ScreenshotHotkeyStatus, ScreenshotOverlayState,
    ScreenshotSelection,
};
#[cfg(test)]
use crate::filesystem::screenshot_files::create_png_file;
#[cfg(test)]
use crate::screenshot::{MonitorGeometry, PlacementFrame};

mod confirm;
#[cfg(target_os = "linux")]
mod linux_display;
mod overlay;
mod replica_root;
#[cfg(target_os = "windows")]
mod window_capture;

#[cfg(test)]
pub(crate) use confirm::complete_confirmation;
pub use confirm::confirm_selection;
#[cfg(test)]
use confirm::{
    build_filename, finish_capture_to_idle, prepare_screenshots_directory, save_selection,
    validate_selection,
};
#[cfg(target_os = "linux")]
pub use linux_display::{
    classify_display_server, display_env_snapshot, DisplayEnvSnapshot, LinuxDisplayServer,
    WAYLAND_UNSUPPORTED,
};
pub use overlay::overlay_state;
#[cfg(test)]
use overlay::{
    build_placement, match_monitor, overlay_label, overlay_window_traits, placement_frame,
    xcap_logical_frame, OverlayWindowTraits, PlacementGeometry,
};
use overlay::{build_placements, destroy_overlays_for_capture, open_overlay_windows};
#[cfg(test)]
use replica_root::find_replica_root;
use replica_root::{resolve_session_replica_root, revalidate_replica_target};
#[cfg(target_os = "windows")]
pub(crate) use window_capture::{
    capture_window_png, parse_window_id, WindowScreenshotCaptureError,
};
#[cfg(all(test, target_os = "windows"))]
use window_capture::{capture_window_worker, BoundedPngWriter, TargetWindowCaptureOps};

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

// ── macOS Screen Recording permission (#2079) ──────────────────────────────

/// The refusal message when macOS has not granted Screen Recording.
#[cfg(target_os = "macos")]
pub const MACOS_SCREEN_RECORDING_REQUIRED: &str = "Screenshot capture needs the Screen \
    Recording permission. Allow AgentsCommander in System Settings > Privacy & Security > \
    Screen Recording, then quit and reopen AgentsCommander.";

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Refuse to capture when macOS has not granted Screen Recording. Without the
/// permission xcap's `CGWindowListCreateImage` succeeds but returns
/// wallpaper-only pixels (no other apps' windows), so capturing without this
/// check fails silently; the grant takes effect only after the app restarts,
/// which the refusal message tells the user to do.
#[cfg(target_os = "macos")]
fn ensure_macos_screen_capture_access() -> Result<(), String> {
    // SAFETY: argument-less CoreGraphics queries (macOS 10.15+), thread-safe.
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return Ok(());
    }
    // Registers the app in the Screen Recording list; prompts only the first time.
    let _ = unsafe { CGRequestScreenCaptureAccess() };
    Err(MACOS_SCREEN_RECORDING_REQUIRED.to_string())
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
pub fn register_configured_hotkey<H: ScreenshotHost>(
    app: &H,
    configured: &str,
) -> Result<(), String> {
    let parsed = super::parse_screenshot_hotkey(configured)?;
    let shortcut = parsed_to_shortcut(&parsed)?;

    #[cfg(target_os = "linux")]
    if classify_display_server(&app.display_snapshot()) == LinuxDisplayServer::Wayland {
        // The snapshot is READ here and TAKEN in run() during builder assembly
        // (§3.5.1). Do not replace this with a call to display_env_snapshot():
        // by the time this line runs, GTK has already unsetenv()'d
        // WAYLAND_SOCKET and a fresh read would answer X11 for a Wayland
        // session reached by an inherited fd.
        // Must precede app.global_shortcut(): under XWayland the grab SUCCEEDS
        // and then never fires. An honest unregistered status here is what
        // turns a silently dead hotkey into the sticky toast that
        // src/sidebar/listeners-screenshot.ts:47 already renders from
        // `!registered && error`. No frontend change.
        let state = app.hotkey_state();
        let mut runtime = state.lock().unwrap();
        runtime.registered_shortcut = None;
        runtime.status = hotkey_status_after_attempt(configured, Err(WAYLAND_UNSUPPORTED));
        return Err(WAYLAND_UNSUPPORTED.to_string());
    }

    app.with_shortcuts(|gs| {
        let state = app.hotkey_state();
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
    })
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
/// hotkey handler must NOT surface it, because bringing the app forward
/// mid-flight would pollute the frozen screenshot and show a bogus failure.
/// Only a genuine `Err` is surfaced.
///
/// The payload is the capture THIS press is entitled to cancel. It is `None`
/// whenever the press must not cancel anything:
///   - `Finishing` — another path already owns that capture's teardown;
///   - "superseded before overlays opened" (the tail of `begin_capture`) — the
///     reporting task is its own stale self, and the lifecycle now belongs to a
///     later press it never observed.
///
/// Naming the capture instead of passing `None` is what stops the Linux
/// second-press cancel from destroying an unrelated capture (#1842).
pub enum BeginOutcome {
    Started,
    Busy(Option<Uuid>),
}

/// The capture a repeat press may cancel. Mirrors the `Starting`/`Active` arms
/// of `clear_capture_and_destroy_overlays`, which are the only states it clears.
/// Matched exhaustively on purpose: a new lifecycle state must be a compile
/// error here, never a silent `None`.
fn cancellable_capture_id(state: &ScreenshotCaptureLifecycle) -> Option<Uuid> {
    match state {
        ScreenshotCaptureLifecycle::Starting(s) => Some(s.id),
        ScreenshotCaptureLifecycle::Active(c) => Some(c.id),
        ScreenshotCaptureLifecycle::Finishing(_) | ScreenshotCaptureLifecycle::Idle => None,
    }
}

/// Global-shortcut handler entry. Spawns the async capture and, on any genuine
/// failure that happens before overlays exist, surfaces the error visibly because
/// the hotkey normally fires while another app is focused. A `Busy` outcome (a
/// rapid repeat press while a capture is in flight) is a silent no-op.
pub fn begin_capture_from_hotkey<H: ScreenshotHost>(app: H) {
    app.clone().spawn_background(Box::pin(async move {
        match begin_capture(app.clone()).await {
            Ok(BeginOutcome::Started) => {}
            Ok(BeginOutcome::Busy(cancellable)) => {
                // Cancelling on a repeat press is Linux- and macOS-specific:
                // Escape reaches the overlay only if the WM granted keyboard
                // focus, and Mutter declined `focused(true)` in 4 of 6 spike
                // runs (E5a); macOS activates apps cooperatively, so overlay
                // focus is not guaranteed there either. A fullscreen,
                // always-on-top, decorationless, skip-taskbar overlay across
                // every monitor that cannot be dismissed is the worst failure
                // available, and the global grab is ours and
                // focus-independent. Accepted cost: an accidental double-tap
                // cancels instead of being ignored (epic R4). Windows keeps
                // today's silent no-op.
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                if let Some(id) = cancellable {
                    let state = app.capture_state();
                    let _ = clear_capture_and_destroy_overlays(
                        &app,
                        &state,
                        Some(id),
                        "second hotkey press cancelled the capture",
                    )
                    .await;
                } else {
                    log::debug!("[screenshot] hotkey ignored: nothing this press may cancel");
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                {
                    // Bind the payload so it is not `unused` under
                    // `cargo clippy --all-targets -- -D warnings` off Linux
                    // and macOS.
                    let _ = cancellable;
                    log::debug!("[screenshot] hotkey ignored: a capture is already in progress");
                }
            }
            Err(e) => surface_hotkey_failure(&app, &e),
        }
    }));
}

/// Emit the failure event AND bring the app forward so the toast is actually
/// visible when the hotkey fired over another foreground app.
fn surface_hotkey_failure<H: ScreenshotHost>(app: &H, message: &str) {
    log::warn!("[screenshot] capture failed before overlay: {message}");
    app.capture_failed(ScreenshotCaptureEvent {
        message: message.to_string(),
    });
    app.focus_capture_failure();
}

// ── Capture ────────────────────────────────────────────────────────────────

/// Resolve active session, capture all monitors, store `Active`, open overlays.
/// Every early-return path leaves the lifecycle `Idle` (no leaked `Starting`).
/// Returns `BeginOutcome::Busy` (a no-op, not an error) when a capture is already
/// in flight, so a debounced repeat hotkey press is silently ignored.
pub async fn begin_capture<H: ScreenshotHost>(app: H) -> Result<BeginOutcome, String> {
    // 0. Busy pre-gate. If a capture is already in flight, bail out as a typed
    //    Busy no-op BEFORE any fallible active-session/CWD resolution below. A
    //    debounced repeat press during an in-flight capture whose active session
    //    has since been cleared/deleted or switched to an unresolvable CWD would
    //    otherwise surface an Err from that resolution and raise AC forward
    //    mid-capture (polluting the frozen screenshot and showing a bogus
    //    failure). The locked reserve in step 2 stays the race-closing
    //    compare-and-set for the Idle path.
    {
        let state = app.capture_state();
        if let Some(outcome) = busy_pregate(&state).await {
            return Ok(outcome);
        }
    }

    // 1. Resolve the active session and its owning replica root.
    let selection = app.selection().snapshot().await?;
    let session_mgr = app.sessions();
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
        let state = app.capture_state();
        let mut guard = state.lock().await;
        if !matches!(&*guard, ScreenshotCaptureLifecycle::Idle) {
            // A capture is already starting/active/finishing: a debounced repeat
            // press. No-op (do NOT surface a failure or raise the window, which
            // would pollute the in-flight frozen capture). Answer from the lock
            // that observed the state: the payload is the capture this press is
            // entitled to cancel.
            return Ok(BeginOutcome::Busy(cancellable_capture_id(&guard)));
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
        let state = app.capture_state();
        let mut guard = state.lock().await;
        let still_ours =
            matches!(&*guard, ScreenshotCaptureLifecycle::Starting(s) if s.id == capture_id);
        if !still_ours {
            // Superseded/cancelled while capturing: do not open overlays. This is
            // a no-op, not a visible failure. `None`: this task no longer owns
            // the lifecycle, so it must not cancel whatever does.
            log::debug!("[screenshot] capture {capture_id} superseded before overlays opened");
            return Ok(BeginOutcome::Busy(None));
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
        let state = app.capture_state();
        let _ = clear_capture_and_destroy_overlays(
            &app,
            &state,
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
        Some(BeginOutcome::Busy(cancellable_capture_id(&guard)))
    }
}

async fn reset_starting_if_matches<H: ScreenshotHost>(app: &H, id: Uuid) {
    let state = app.capture_state();
    let mut guard = state.lock().await;
    if matches!(&*guard, ScreenshotCaptureLifecycle::Starting(s) if s.id == id) {
        *guard = ScreenshotCaptureLifecycle::Idle;
    }
}

/// Capture every monitor with `xcap` and encode a base64 PNG preview per monitor.
/// All `xcap` getters are fallible and re-query the OS, so each is read once.
async fn capture_all_monitors() -> Result<Vec<CapturedMonitor>, String> {
    tokio::task::spawn_blocking(|| -> Result<Vec<CapturedMonitor>, String> {
        #[cfg(target_os = "macos")]
        ensure_macos_screen_capture_access()?;
        let monitors =
            xcap::Monitor::all().map_err(|e| format!("xcap monitor enumeration failed: {e}"))?;
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
                "[screenshot] monitor {idx} '{name}' origin=({x},{y}) size={width}x{height} \
                    scale={scale_factor}"
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

// ── Cancel + cleanup ───────────────────────────────────────────────────────

/// Explicit cancel (Escape / overlay close request). Idempotent.
pub async fn cancel_capture<H: ScreenshotHost>(
    app: &H,
    state: &ScreenshotCaptureState,
    capture_id: &str,
) -> Result<(), String> {
    let id = Uuid::parse_str(capture_id).map_err(|_| "invalid capture id".to_string())?;
    clear_capture_and_destroy_overlays(app, state, Some(id), "user cancelled").await
}

/// Centralized, idempotent cleanup: take a matching `Starting`/`Active` capture
/// to `Idle` and destroy its overlays by label prefix. `capture_id == None`
/// clears whatever is active. Safe to call repeatedly; later calls are no-ops.
pub async fn clear_capture_and_destroy_overlays<H: ScreenshotHost>(
    app: &H,
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
pub async fn handle_overlay_window_destroyed<H: ScreenshotHost>(app: H, label: String) {
    let Some(hex) = capture_hex_from_overlay_label(&label) else {
        return;
    };
    let state = app.capture_state();
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
                &state,
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

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
