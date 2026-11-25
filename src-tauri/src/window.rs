//! Main-window geometry helpers.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::pty::idle_detector::IdleDetector;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

use crate::config;

/// Check if at least 50px of a window (physical coords) is visible on any monitor.
pub(crate) fn is_visible_on_monitors(
    geo: &config::settings::WindowGeometry,
    monitors: &[(f64, f64, f64, f64, f64)],
) -> bool {
    if monitors.is_empty() {
        return true; // Can't validate, assume OK
    }
    let margin = 50.0;
    monitors.iter().any(|(mx, my, mx2, my2, _)| {
        geo.x + geo.width > mx + margin
            && geo.x < mx2 - margin
            && geo.y + geo.height > my + margin
            && geo.y < my2 - margin
    })
}

/// Convert saved geometry (physical pixels) to logical pixels for the builder.
/// Finds which monitor the geometry center falls on and divides by that scale.
pub(crate) fn physical_to_logical(
    geo: &config::settings::WindowGeometry,
    monitors: &[(f64, f64, f64, f64, f64)],
) -> config::settings::WindowGeometry {
    let cx = geo.x + geo.width / 2.0;
    let cy = geo.y + geo.height / 2.0;
    let scale = monitors
        .iter()
        .find(|(mx, my, mx2, my2, _)| cx >= *mx && cx < *mx2 && cy >= *my && cy < *my2)
        .map_or(1.0, |(_, _, _, _, s)| *s);
    config::settings::WindowGeometry {
        x: geo.x / scale,
        y: geo.y / scale,
        width: geo.width / scale,
        height: geo.height / scale,
    }
}

/// The default "centered main" layout for the given LOGICAL monitor
/// metrics: at most 1400x900, centered on the monitor. With no primary monitor
/// the caller passes the historical 1920x1080 fallback at the origin.
pub(crate) fn centered_default_main_geometry(
    primary_x: f64,
    primary_y: f64,
    screen_w: f64,
    screen_h: f64,
) -> config::settings::WindowGeometry {
    let default_w = screen_w.min(1400.0);
    let default_h = screen_h.min(900.0);
    config::settings::WindowGeometry {
        x: primary_x + (screen_w - default_w) / 2.0,
        y: primary_y + (screen_h - default_h) / 2.0,
        width: default_w,
        height: default_h,
    }
}

/// The display state that applies at startup. A test placement always
/// overrides the saved state, so `saved` is consulted only when the test branch
/// is absent; a test `maximized=false` therefore cannot inherit a saved maximize.
pub(crate) fn effective_main_display_state(
    saved: config::settings::MainWindowDisplayState,
    test_placement: Option<&crate::testability::window_placement::TestWindowPlacement>,
) -> config::settings::MainWindowDisplayState {
    match test_placement {
        Some(test_geo) if test_geo.maximized => config::settings::MainWindowDisplayState::Maximized,
        Some(_) => config::settings::MainWindowDisplayState::Normal,
        None => saved,
    }
}

/// Apply the resolved display state after the window is built.
/// Maximizing is best effort: a failure is logged and the window stays usable in
/// its normal state.
pub(crate) fn apply_main_display_state<E: fmt::Display>(
    state: config::settings::MainWindowDisplayState,
    maximize: impl FnOnce() -> Result<(), E>,
) {
    if state == config::settings::MainWindowDisplayState::Maximized {
        if let Err(e) = maximize() {
            log::warn!(
                "[window-setup] main: failed to maximize the main window; leaving it normal: {e}"
            );
        }
    }
}

pub(crate) fn response_close_wiring_matches(
    state: &crate::pty::manager::ResponseCloseState,
    manager: &SessionManager,
    pty: &PtyManager,
    detector: &IdleDetector,
    menu: &crate::pty::menu_guard::MenuGuard,
) -> bool {
    state.publishers_bound()
        && state.same_state(&manager.response_close_state())
        && state.same_state(&pty.response_close_state())
        && detector.raw_output_observer_identity() == Some(state.wiring_identity())
        && menu
            .response_close_state()
            .is_some_and(|bound| state.same_state(bound))
}

pub(crate) fn validate_explorer_target(path: &str) -> Result<PathBuf, String> {
    let canonical = crate::filesystem::directories::canonical_explorer_target(Path::new(path))
        .map_err(|_| format!("Path does not exist or is inaccessible: {path}"))?;
    if !crate::filesystem::directories::explorer_target_is_dir(&canonical) {
        return Err(format!("Path is not a directory: {path}"));
    }
    Ok(canonical)
}

pub(crate) const MAIN_WINDOW_LABEL: &str = "main";
pub(crate) const RESOURCE_MONITOR_WINDOW_LABEL: &str = "resource-monitor";
const RESOURCE_MONITOR_FLOATING_WIDTH: u32 = 760;
const RESOURCE_MONITOR_FLOATING_HEIGHT: u32 = 560;
const RESOURCE_MONITOR_MIN_WIDTH: u32 = 520;
pub(crate) const RESOURCE_MONITOR_MIN_HEIGHT: u32 = 420;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PhysicalWindowRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl PhysicalWindowRect {
    pub(crate) const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub(crate) fn right(self) -> i32 {
        self.x.saturating_add(u32_to_i32(self.width))
    }

    fn bottom(self) -> i32 {
        self.y.saturating_add(u32_to_i32(self.height))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResourceMonitorPlacement {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MonitorWorkArea {
    pub(crate) rect: PhysicalWindowRect,
    pub(crate) scale_factor: f64,
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

pub(crate) fn resource_monitor_placement_for_main(
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

/// Native window operations the Resource Monitor lifecycle needs. The command edge
/// implements them; this domain only passes `Window` values back to these methods.
pub(crate) trait ResourceMonitorWindows {
    type Window;

    fn lookup(&self, label: &str) -> Option<Self::Window>;
    fn focus(&self, window: &Self::Window) -> Result<(), String>;
    fn outer_rect(&self, window: &Self::Window) -> Result<PhysicalWindowRect, String>;
    fn work_area(&self, window: &Self::Window) -> Result<MonitorWorkArea, String>;
    fn build(
        &self,
        position: (f64, f64),
        size: (f64, f64),
        minimum: (f64, f64),
    ) -> Result<Self::Window, String>;
    fn set_size(&self, window: &Self::Window, width: u32, height: u32) -> Result<(), String>;
    fn outer_size(&self, window: &Self::Window) -> Result<(u32, u32), String>;
    fn set_position(&self, window: &Self::Window, x: i32, y: i32) -> Result<(), String>;
}

/// Open the floating Resource Monitor beside the main window, or focus it when it is
/// already open. The second placement uses the size the new window actually reports.
pub(crate) fn open_resource_monitor(windows: &impl ResourceMonitorWindows) -> Result<(), String> {
    if let Some(existing) = windows.lookup(RESOURCE_MONITOR_WINDOW_LABEL) {
        windows.focus(&existing)?;
        return Ok(());
    }

    let main = windows
        .lookup(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "Main window is not available".to_string())?;
    let main_rect = windows.outer_rect(&main)?;
    let monitor_work_area = windows.work_area(&main)?;
    let placement = resource_monitor_placement_for_main(
        main_rect,
        monitor_work_area.rect,
        RESOURCE_MONITOR_FLOATING_WIDTH,
        RESOURCE_MONITOR_FLOATING_HEIGHT,
    );
    let scale_factor = effective_scale_factor(monitor_work_area.scale_factor);

    let window = windows.build(
        (
            f64::from(placement.x) / scale_factor,
            f64::from(placement.y) / scale_factor,
        ),
        (
            f64::from(placement.width) / scale_factor,
            f64::from(placement.height) / scale_factor,
        ),
        (
            f64::from(RESOURCE_MONITOR_MIN_WIDTH),
            f64::from(RESOURCE_MONITOR_MIN_HEIGHT),
        ),
    )?;

    windows.set_size(&window, placement.width, placement.height)?;
    let (actual_width, actual_height) = windows.outer_size(&window)?;
    let placement = resource_monitor_placement_for_main(
        main_rect,
        monitor_work_area.rect,
        actual_width,
        actual_height,
    );
    windows.set_position(&window, placement.x, placement.y)?;

    Ok(())
}
