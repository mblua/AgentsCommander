//! #2079 macOS Screen Recording gate, moved out of native.rs by #2774; leaf: no
//! `super` paths.

/// The refusal message when macOS has not granted Screen Recording.
#[cfg(target_os = "macos")]
pub const MACOS_SCREEN_RECORDING_REQUIRED: &str = "Screenshot capture needs the Screen \
    Recording permission. Allow AgentsCommander in System Settings > Privacy & Security > \
    Screen Recording, then quit and reopen AgentsCommander.";

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub(super) fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Refuse to capture when macOS has not granted Screen Recording. Without the
/// permission xcap's `CGWindowListCreateImage` succeeds but returns
/// wallpaper-only pixels (no other apps' windows), so capturing without this
/// check fails silently; the grant takes effect only after the app restarts,
/// which the refusal message tells the user to do.
#[cfg(target_os = "macos")]
pub(super) fn ensure_macos_screen_capture_access() -> Result<(), String> {
    // SAFETY: argument-less CoreGraphics queries (macOS 10.15+), thread-safe.
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return Ok(());
    }
    // Registers the app in the Screen Recording list; prompts only the first time.
    let _ = unsafe { CGRequestScreenCaptureAccess() };
    Err(MACOS_SCREEN_RECORDING_REQUIRED.to_string())
}
