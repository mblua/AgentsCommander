//! Main-window geometry helpers.

use std::fmt;

use crate::pty::idle_detector::IdleDetector;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

use crate::config;

/// #2348 - check if at least 50px of a window (physical coords) is visible on any monitor.
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
        .map(|(_, _, _, _, s)| *s)
        .unwrap_or(1.0);
    config::settings::WindowGeometry {
        x: geo.x / scale,
        y: geo.y / scale,
        width: geo.width / scale,
        height: geo.height / scale,
    }
}

/// #2348 - the default "centered main" layout for the given LOGICAL monitor
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

/// #2348 - the display state that applies at startup. A test placement always
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

/// #2348 - apply the resolved display state after the window is built.
/// Maximizing is best effort: a failure is logged and the window stays usable in
/// its normal state.
pub(crate) fn apply_main_display_state<E: fmt::Display>(
    state: config::settings::MainWindowDisplayState,
    maximize: impl FnOnce() -> Result<(), E>,
) {
    if state == config::settings::MainWindowDisplayState::Maximized {
        if let Err(e) = maximize() {
            log::warn!(
                "[window-setup] main: failed to maximize the main window; leaving it normal: {}",
                e
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
