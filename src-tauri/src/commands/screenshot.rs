//! #714 Screenshot capture Tauri commands.
//!
//! Thin IPC wrappers over `crate::screenshot`. All capture-state mutation,
//! overlay teardown, and clipboard/clock side effects live in the screenshot
//! module; these handlers only marshal arguments and managed state.

use tauri::{AppHandle, Manager, State};

use crate::config::settings::SettingsState;
use crate::screenshot::{
    ScreenshotCaptureResult, ScreenshotCaptureState, ScreenshotHotkeyState, ScreenshotHotkeyStatus,
    ScreenshotOverlayState, ScreenshotSelection,
};

/// Return the frozen monitor image + metadata for one overlay window.
#[tauri::command]
pub async fn screenshot_get_overlay_state(
    app: AppHandle,
    state: State<'_, ScreenshotCaptureState>,
    capture_id: String,
    monitor_id: u32,
) -> Result<ScreenshotOverlayState, String> {
    crate::screenshot::overlay_state(&app, state.inner(), &capture_id, monitor_id).await
}

/// Confirm a selection: crop, save the PNG into the replica root, copy the path,
/// and tear down overlays. Backend owns all cleanup.
#[tauri::command]
pub async fn screenshot_confirm_selection(
    app: AppHandle,
    state: State<'_, ScreenshotCaptureState>,
    selection: ScreenshotSelection,
) -> Result<ScreenshotCaptureResult, String> {
    crate::screenshot::confirm_selection(&app, state.inner(), selection).await
}

/// Cancel an in-progress capture (Escape / overlay close). Idempotent.
#[tauri::command]
pub async fn screenshot_cancel_capture(
    app: AppHandle,
    state: State<'_, ScreenshotCaptureState>,
    capture_id: String,
) -> Result<(), String> {
    crate::screenshot::cancel_capture(&app, state.inner(), &capture_id).await
}

/// Current global-hotkey registration status (configured combo + whether the OS
/// accepted it + any conflict error). The sidebar reads this on mount.
#[tauri::command]
pub fn screenshot_get_hotkey_status(
    state: State<'_, ScreenshotHotkeyState>,
) -> ScreenshotHotkeyStatus {
    state.lock().unwrap().status.clone()
}

/// Re-read the configured hotkey from settings and (re-)register it. Useful after
/// a Settings save and in manual testing; returns the resulting status. A failed
/// OS registration is reflected in the status, not returned as an error.
#[tauri::command]
pub async fn screenshot_reload_hotkey(
    app: AppHandle,
    settings: State<'_, SettingsState>,
) -> Result<ScreenshotHotkeyStatus, String> {
    let configured = settings
        .current_read()
        .await
        .map_err(|error| error.to_string())?
        .screenshot_capture_hotkey
        .clone();
    // Ignore the Err: an OS conflict is captured in the status below; settings
    // validation already rejected any syntactically invalid value.
    let _ = crate::screenshot::register_configured_hotkey(&app, &configured);
    let status = app
        .state::<ScreenshotHotkeyState>()
        .lock()
        .unwrap()
        .status
        .clone();
    Ok(status)
}

impl<R: tauri::Runtime> crate::screenshot::ScreenshotHotkeyOwner for AppHandle<R> {
    fn hotkey_state(&self) -> ScreenshotHotkeyState {
        self.state::<ScreenshotHotkeyState>().inner().clone()
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl<R: tauri::Runtime> crate::screenshot::ShortcutRegistrar
    for tauri_plugin_global_shortcut::GlobalShortcut<R>
{
    fn is_registered(&self, shortcut: tauri_plugin_global_shortcut::Shortcut) -> bool {
        tauri_plugin_global_shortcut::GlobalShortcut::is_registered(self, shortcut)
    }

    fn register(&self, shortcut: tauri_plugin_global_shortcut::Shortcut) -> Result<(), String> {
        tauri_plugin_global_shortcut::GlobalShortcut::register(self, shortcut)
            .map_err(|error| error.to_string())
    }

    fn unregister(&self, shortcut: tauri_plugin_global_shortcut::Shortcut) -> Result<(), String> {
        tauri_plugin_global_shortcut::GlobalShortcut::unregister(self, shortcut)
            .map_err(|error| error.to_string())
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl crate::screenshot::ScreenshotHost for AppHandle {
    fn capture_state(&self) -> ScreenshotCaptureState {
        self.state::<ScreenshotCaptureState>().inner().clone()
    }

    fn selection(&self) -> crate::session::selection::SelectionCoordinator {
        self.state::<crate::session::selection::SelectionCoordinator>()
            .inner()
            .clone()
    }

    fn sessions(
        &self,
    ) -> std::sync::Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>> {
        self.state::<std::sync::Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>()
            .inner()
            .clone()
    }

    fn available_monitors(&self) -> Vec<crate::screenshot::MonitorGeometry> {
        tauri::AppHandle::available_monitors(self)
            .unwrap_or_default()
            .iter()
            .map(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                crate::screenshot::MonitorGeometry {
                    name: monitor.name().cloned(),
                    position: (position.x, position.y),
                    size: (size.width, size.height),
                    scale: monitor.scale_factor(),
                }
            })
            .collect()
    }

    fn with_shortcuts<T, F: FnOnce(&dyn crate::screenshot::ShortcutRegistrar) -> T>(
        &self,
        callback: F,
    ) -> T {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        let registrar = self.global_shortcut();
        callback(registrar)
    }

    #[cfg(target_os = "linux")]
    fn display_snapshot(&self) -> crate::screenshot::DisplayEnvSnapshot {
        self.state::<crate::screenshot::DisplayEnvSnapshot>()
            .inner()
            .clone()
    }

    fn open_overlay(&self, p: &crate::screenshot::OverlayWindowSpec) -> Result<(), String> {
        use crate::screenshot::PlacementFrame;
        use tauri::{WebviewUrl, WebviewWindowBuilder};

        let url = p.url.clone();
        let logical_w = p.logical_width;
        let logical_h = p.logical_height;
        let logical_x = p.logical_x;
        let logical_y = p.logical_y;
        let builder = WebviewWindowBuilder::new(self, &p.label, WebviewUrl::App(url.into()))
            .title("Screenshot Capture")
            .decorations(false);
        // Tauri gates `transparent` behind `macos-private-api` on macOS; the
        // overlay paints an opaque frozen image, so transparency is unnecessary
        // there.
        #[cfg(not(target_os = "macos"))]
        let builder = builder.transparent(true);
        let window = builder
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(p.resizable)
            .focused(true)
            .zoom_hotkeys_enabled(false)
            .inner_size(logical_w, logical_h)
            .position(logical_x, logical_y)
            .build()
            .map_err(|e| format!("failed to create overlay '{}': {e}", p.label))?;

        // Post-build correction, in the placement's own frame. `Physical` is the
        // historical path byte for byte; `Logical` is macOS, where the overlay
        // must be stated in points or a scaled display mode undersizes it.
        let (size, position) = match p.frame {
            PlacementFrame::Physical => (
                tauri::Size::Physical(tauri::PhysicalSize {
                    width: p.width,
                    height: p.height,
                }),
                tauri::Position::Physical(tauri::PhysicalPosition { x: p.x, y: p.y }),
            ),
            PlacementFrame::Logical => (
                tauri::Size::Logical(tauri::LogicalSize {
                    width: p.width as f64,
                    height: p.height as f64,
                }),
                tauri::Position::Logical(tauri::LogicalPosition {
                    x: p.x as f64,
                    y: p.y as f64,
                }),
            ),
        };
        window
            .set_size(size)
            .map_err(|e| format!("failed to size overlay '{}': {e}", p.label))?;
        window
            .set_position(position)
            .map_err(|e| format!("failed to position overlay '{}': {e}", p.label))?;

        if p.fullscreen {
            // AFTER set_position: fullscreen targets the monitor the window
            // is on. Tauri returns Ok(()) even when the WM declines (E5a), so
            // this is best-effort and must not fail the capture.
            if let Err(e) = window.set_fullscreen(true) {
                log::warn!("[screenshot] overlay '{}' fullscreen refused: {e}", p.label);
            }
        }
        Ok(())
    }

    fn destroy_capture_overlays(&self, capture_id: uuid::Uuid) -> usize {
        let prefix = format!("screenshot-overlay-{}-", capture_id.simple());
        let mut destroyed = 0;
        for (label, window) in self.webview_windows() {
            if label.starts_with(&prefix) {
                match window.destroy() {
                    Ok(()) => destroyed += 1,
                    Err(e) => log::warn!("[screenshot] failed to destroy overlay '{label}': {e}"),
                }
            }
        }
        destroyed
    }

    fn copy_path(&self, path: String) -> Result<(), String> {
        use tauri_plugin_clipboard_manager::ClipboardExt;

        self.clipboard()
            .write_text(path)
            .map_err(|error| error.to_string())
    }

    fn focus_capture_failure(&self) {
        if let Some(win) = self
            .get_webview_window("main")
            .or_else(|| self.get_webview_window("sidebar"))
        {
            let _ = win.unminimize();
            let _ = win.show();
            let _ = win.set_focus();
            let _ = win.request_user_attention(Some(tauri::UserAttentionType::Critical));
        }
    }

    fn spawn_background(
        &self,
        future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
    ) {
        tauri::async_runtime::spawn(future);
    }
}
