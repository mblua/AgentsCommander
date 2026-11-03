//! Main window creation for the `.setup` hook.

use tauri::{WebviewUrl, WebviewWindowBuilder};

use super::geometry;
use crate::testability::window_placement::TestWindowPlacement;
use crate::{
    apply_main_display_state, centered_default_main_geometry, config, effective_main_display_state,
};

pub(super) fn create_main_window(
    app: &mut tauri::App,
    test_window_placement: Option<&TestWindowPlacement>,
) -> Result<(), Box<dyn std::error::Error>> {
    let icon = tauri::image::Image::from_bytes(include_bytes!("../../../icons/icon.png"))
        .expect("Failed to load app icon");

    // Load saved window geometry
    let saved_settings = config::settings::load_settings();

    // Collect available monitor bounds (physical) + scale factor for geometry validation
    // Tuple: (x, y, x2, y2, scale_factor) — all positions/sizes in physical pixels
    let monitors = available_monitor_bounds(app);

    geometry::log_monitors(&monitors);

    let default_main = default_main_geometry(app);

    let main_geo = geometry::resolve_main_geometry(
        test_window_placement,
        saved_settings.main_geometry.as_ref(),
        &monitors,
        &default_main,
    );

    // #2348: resolve the startup display state once. A test placement's
    // `maximized` boolean overrides the saved state entirely; otherwise the
    // saved state applies (and a saved maximize is only a request, never a
    // fatal failure: `apply_main_display_state` logs and keeps the window
    // usable).
    let effective_display_state = effective_main_display_state(
        saved_settings.main_window_display_state,
        test_window_placement,
    );

    // Create the unified Main window (replaces sidebar + terminal windows).
    let main_win = WebviewWindowBuilder::new(
        app,
        "main",
        WebviewUrl::App("index.html?window=main".into()),
    )
    .title(config::profile::app_title())
    .icon(icon)
    .expect("Failed to set main window icon")
    .min_inner_size(800.0, 500.0)
    .decorations(false)
    .zoom_hotkeys_enabled(false)
    .inner_size(main_geo.width, main_geo.height)
    .position(main_geo.x, main_geo.y)
    .build()?;

    if let Some(test_geo) = test_window_placement {
        let native_handled = apply_test_window_placement(&main_win, test_geo);
        if effective_display_state == config::settings::MainWindowDisplayState::Maximized
            && !native_handled
        {
            if let Err(e) = main_win.maximize() {
                log::warn!("[test-window] failed to maximize main window: {}", e);
            }
        }
        log_main_window_info(&main_win);
    } else {
        apply_main_display_state(effective_display_state, || main_win.maximize());
    }

    if saved_settings.main_always_on_top {
        let _ = main_win.set_always_on_top(true);
    }

    // Suppress unused variable warning
    let _ = &main_win;
    Ok(())
}

fn available_monitor_bounds(app: &tauri::App) -> Vec<(f64, f64, f64, f64, f64)> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let pos = m.position();
            let size = m.size();
            (
                pos.x as f64,
                pos.y as f64,
                pos.x as f64 + size.width as f64,
                pos.y as f64 + size.height as f64,
                m.scale_factor(),
            )
        })
        .collect()
}

fn default_main_geometry(app: &tauri::App) -> config::settings::WindowGeometry {
    // Determine primary monitor size for the default "centered main" layout.
    // Convert to logical pixels (physical / scale) since WebviewWindowBuilder
    // ::inner_size() and ::position() expect logical coordinates.
    let primary = app.primary_monitor().ok().flatten();
    let primary_scale = primary.as_ref().map(|m| m.scale_factor()).unwrap_or(1.0);
    let (screen_w, screen_h) = primary
        .as_ref()
        .map(|m| {
            let s = m.size();
            (
                s.width as f64 / primary_scale,
                s.height as f64 / primary_scale,
            )
        })
        .unwrap_or((1920.0, 1080.0));
    let primary_x = primary
        .as_ref()
        .map(|m| m.position().x as f64 / primary_scale)
        .unwrap_or(0.0);
    let primary_y = primary
        .as_ref()
        .map(|m| m.position().y as f64 / primary_scale)
        .unwrap_or(0.0);

    // Default main window: centered at 1400×900, or the primary monitor size
    // minus a small margin if the screen is narrower than 1400.
    centered_default_main_geometry(primary_x, primary_y, screen_w, screen_h)
}

fn log_main_window_info(win: &tauri::WebviewWindow) {
    let pid = std::process::id();
    let pos = win.outer_position().ok();
    let size = win.outer_size().ok();
    let maximized = win.is_maximized().ok();
    log::info!(
        "[test-window] actual pid={} position={:?} size={:?} maximized={:?}",
        pid,
        pos,
        size,
        maximized
    );
    println!(
        concat!(
            "{{\"event\":\"testWindowInfo\",\"pid\":{},\"position\":{},",
            "\"size\":{},\"maximized\":{}}}"
        ),
        pid,
        serde_json::to_string(&pos.map(|p| serde_json::json!({ "x": p.x, "y": p.y })))
            .unwrap_or_else(|_| "null".to_string()),
        serde_json::to_string(
            &size.map(|s| serde_json::json!({ "width": s.width, "height": s.height }))
        )
        .unwrap_or_else(|_| "null".to_string()),
        serde_json::to_string(&maximized).unwrap_or_else(|_| "null".to_string())
    );
}

fn apply_test_window_placement(
    win: &tauri::WebviewWindow,
    geo: &crate::testability::window_placement::TestWindowPlacement,
) -> bool {
    let x = geo.x.round() as i32;
    let y = geo.y.round() as i32;
    let width = geo.width.round().max(1.0) as u32;
    let height = geo.height.round().max(1.0) as u32;

    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::{POINT, RECT};
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowPlacement, IsZoomed, SetWindowPlacement, SetWindowPos, ShowWindow,
            SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_RESTORE, SW_SHOWMAXIMIZED,
            WINDOWPLACEMENT,
        };

        match win.hwnd() {
            Ok(hwnd) => unsafe {
                let requested = RECT {
                    left: x,
                    top: y,
                    right: x.saturating_add(width as i32),
                    bottom: y.saturating_add(height as i32),
                };
                let monitor = MonitorFromRect(&requested, MONITOR_DEFAULTTONEAREST);
                if monitor.is_null() {
                    log::warn!(
                        "[test-window] MonitorFromRect returned null for requested \
                            rect ({}, {}) {}x{}",
                        x,
                        y,
                        width,
                        height
                    );
                } else {
                    let mut monitor_info = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        rcMonitor: RECT {
                            left: 0,
                            top: 0,
                            right: 0,
                            bottom: 0,
                        },
                        rcWork: RECT {
                            left: 0,
                            top: 0,
                            right: 0,
                            bottom: 0,
                        },
                        dwFlags: 0,
                    };
                    if GetMonitorInfoW(monitor, &mut monitor_info) == 0 {
                        log::warn!(
                            "[test-window] GetMonitorInfoW failed for requested \
                                rect ({}, {}) {}x{}",
                            x,
                            y,
                            width,
                            height
                        );
                    } else {
                        log::info!(
                            "[test-window] selected monitor rect=({}, {}) {}x{} \
                                work=({}, {}) {}x{} for requested rect ({}, {}) \
                                {}x{} maximized={}",
                            monitor_info.rcMonitor.left,
                            monitor_info.rcMonitor.top,
                            monitor_info.rcMonitor.right - monitor_info.rcMonitor.left,
                            monitor_info.rcMonitor.bottom - monitor_info.rcMonitor.top,
                            monitor_info.rcWork.left,
                            monitor_info.rcWork.top,
                            monitor_info.rcWork.right - monitor_info.rcWork.left,
                            monitor_info.rcWork.bottom - monitor_info.rcWork.top,
                            x,
                            y,
                            width,
                            height,
                            geo.maximized
                        );
                    }
                }

                if IsZoomed(hwnd.0.cast()) != 0 || geo.maximized {
                    ShowWindow(hwnd.0.cast(), SW_RESTORE);
                }
                let ok = SetWindowPos(
                    hwnd.0.cast(),
                    std::ptr::null_mut(),
                    x,
                    y,
                    width as i32,
                    height as i32,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                if ok == 0 {
                    log::warn!("[test-window] native SetWindowPos failed");
                }
                if geo.maximized {
                    let mut placement = WINDOWPLACEMENT {
                        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                        flags: 0,
                        showCmd: SW_SHOWMAXIMIZED as u32,
                        ptMinPosition: POINT { x: -1, y: -1 },
                        ptMaxPosition: POINT { x: -1, y: -1 },
                        rcNormalPosition: requested,
                    };
                    if GetWindowPlacement(hwnd.0.cast(), &mut placement) == 0 {
                        log::warn!("[test-window] GetWindowPlacement failed before maximize");
                    }
                    placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
                    placement.showCmd = SW_SHOWMAXIMIZED as u32;
                    placement.rcNormalPosition = requested;
                    if SetWindowPlacement(hwnd.0.cast(), &placement) == 0 {
                        log::warn!("[test-window] SetWindowPlacement maximize failed");
                        ShowWindow(hwnd.0.cast(), SW_SHOWMAXIMIZED);
                    }
                }
                return true;
            },
            Err(e) => {
                log::warn!("[test-window] failed to get HWND: {}", e);
            }
        }
    }

    if let Err(e) = win.set_size(tauri::Size::Physical(tauri::PhysicalSize { width, height })) {
        log::warn!("[test-window] failed to set physical size: {}", e);
    }
    if let Err(e) = win.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y })) {
        log::warn!("[test-window] failed to set physical position: {}", e);
    }
    false
}
