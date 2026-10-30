use crate::config::settings::WindowGeometry;
use crate::testability::window_placement::TestWindowPlacement;
use crate::{is_visible_on_monitors, physical_to_logical};

pub(crate) fn resolve_main_geometry(
    test: Option<&TestWindowPlacement>,
    saved: Option<&WindowGeometry>,
    monitors: &[(f64, f64, f64, f64, f64)],
    default: &WindowGeometry,
) -> WindowGeometry {
    // Resolve main geometry: saved (physical) -> validate -> convert to logical ->
    // fallback.
    // First-boot-after-upgrade users will have `main_geometry` seeded from legacy
    // `terminal_geometry` via the migration in `config::settings::load_settings`.
    if let Some(test_geo) = test {
        let requested = WindowGeometry {
            x: test_geo.x,
            y: test_geo.y,
            width: test_geo.width,
            height: test_geo.height,
        };
        let logical = physical_to_logical(&requested, monitors);
        log::info!(
            "[test-window] requested physical ({}, {}) {}x{} maximized={} -> logical \
                ({}, {}) {}x{}",
            requested.x,
            requested.y,
            requested.width,
            requested.height,
            test_geo.maximized,
            logical.x,
            logical.y,
            logical.width,
            logical.height
        );
        logical
    } else {
        match saved {
            Some(geo) if is_visible_on_monitors(geo, monitors) => {
                let logical = physical_to_logical(geo, monitors);
                log::info!(
                    "[window-setup] main: saved physical ({}, {}) {}x{} -> logical ({}, \
                        {}) {}x{}",
                    geo.x,
                    geo.y,
                    geo.width,
                    geo.height,
                    logical.x,
                    logical.y,
                    logical.width,
                    logical.height
                );
                logical
            }
            Some(geo) => {
                log::warn!(
                    "[window-setup] main: saved geometry ({}, {}) {}x{} is off-screen, \
                        falling back to centered default",
                    geo.x,
                    geo.y,
                    geo.width,
                    geo.height
                );
                default.clone()
            }
            None => {
                log::info!("[window-setup] main: no saved geometry, using centered default");
                default.clone()
            }
        }
    }
}

pub(crate) fn log_monitors(monitors: &[(f64, f64, f64, f64, f64)]) {
    log::info!("[window-setup] {} monitors detected", monitors.len());
    for (i, (mx, my, mx2, my2, scale)) in monitors.iter().enumerate() {
        log::info!(
            "[window-setup]   monitor {}: ({}, {}) -> ({}, {}) scale={}",
            i,
            mx,
            my,
            mx2,
            my2,
            scale
        );
    }
}
