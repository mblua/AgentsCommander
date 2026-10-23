use super::*;

// ── #2348 - the extracted window-placement helpers ───────────────────────

fn issue_2348_geometry(x: f64, y: f64, width: f64, height: f64) -> WindowGeometry {
    WindowGeometry {
        x,
        y,
        width,
        height,
    }
}

fn issue_2348_test_placement(maximized: bool) -> TestWindowPlacement {
    TestWindowPlacement {
        x: 0.0,
        y: 0.0,
        width: 800.0,
        height: 600.0,
        maximized,
    }
}

#[test]
fn issue_2348_visibility_requires_fifty_pixels_on_a_monitor() {
    let monitors = [(0.0, 0.0, 1920.0, 1080.0, 1.0)];
    assert!(is_visible_on_monitors(
        &issue_2348_geometry(0.0, 0.0, 800.0, 600.0),
        &monitors
    ));
    // 100px overlap with the monitor edge -> visible.
    assert!(is_visible_on_monitors(
        &issue_2348_geometry(-700.0, 0.0, 800.0, 600.0),
        &monitors
    ));
    // 40px overlap -> off-screen.
    assert!(!is_visible_on_monitors(
        &issue_2348_geometry(-760.0, 0.0, 800.0, 600.0),
        &monitors
    ));
    assert!(!is_visible_on_monitors(
        &issue_2348_geometry(5000.0, 0.0, 800.0, 600.0),
        &monitors
    ));
    // No monitors at all: cannot validate, so visible (the existing behavior).
    assert!(is_visible_on_monitors(
        &issue_2348_geometry(5000.0, 0.0, 800.0, 600.0),
        &[]
    ));
}

#[test]
fn issue_2348_physical_to_logical_uses_the_monitor_under_the_center() {
    // Mixed scales: 1.5x primary, 2.0x secondary to its right.
    let monitors = [
        (0.0, 0.0, 1920.0, 1080.0, 1.5),
        (1920.0, 0.0, 3840.0, 1080.0, 2.0),
    ];
    // Center (2000, 400) lands on the 2.0x secondary.
    let logical = physical_to_logical(&issue_2348_geometry(1600.0, 100.0, 800.0, 600.0), &monitors);
    assert_eq!((logical.x, logical.y), (800.0, 50.0));
    assert_eq!((logical.width, logical.height), (400.0, 300.0));
    // Center on the primary uses its 1.5x scale.
    let logical = physical_to_logical(&issue_2348_geometry(100.0, 100.0, 800.0, 600.0), &monitors);
    assert_eq!((logical.width, logical.height), (800.0 / 1.5, 600.0 / 1.5));
    // No monitor contains the center: scale 1.0.
    let logical = physical_to_logical(
        &issue_2348_geometry(-5000.0, -5000.0, 800.0, 600.0),
        &monitors,
    );
    assert_eq!((logical.x, logical.y), (-5000.0, -5000.0));
}

#[test]
fn issue_2348_centered_default_clamps_to_1400x900_and_follows_the_primary_offset() {
    // The no-primary fallback: 1920x1080 at the origin.
    let geo = centered_default_main_geometry(0.0, 0.0, 1920.0, 1080.0);
    assert_eq!((geo.x, geo.y), (260.0, 90.0));
    assert_eq!((geo.width, geo.height), (1400.0, 900.0));
    // A screen narrower than the default keeps its full size at the origin.
    let geo = centered_default_main_geometry(100.0, 50.0, 1000.0, 700.0);
    assert_eq!((geo.x, geo.y), (100.0, 50.0));
    assert_eq!((geo.width, geo.height), (1000.0, 700.0));
    // An offset primary centers the clamped size within the monitor.
    let geo = centered_default_main_geometry(100.0, 50.0, 2000.0, 1200.0);
    assert_eq!((geo.x, geo.y), (400.0, 200.0));
    assert_eq!((geo.width, geo.height), (1400.0, 900.0));
}

#[test]
fn issue_2348_effective_display_state_gives_test_placement_precedence() {
    let normal_test = issue_2348_test_placement(false);
    let maximized_test = issue_2348_test_placement(true);

    // Test branch absent: the saved state applies.
    assert_eq!(
        effective_main_display_state(MainWindowDisplayState::Normal, None),
        MainWindowDisplayState::Normal
    );
    assert_eq!(
        effective_main_display_state(MainWindowDisplayState::Maximized, None),
        MainWindowDisplayState::Maximized
    );
    // Test branch present, maximized=false: a saved maximize is NOT applied.
    assert_eq!(
        effective_main_display_state(MainWindowDisplayState::Maximized, Some(&normal_test)),
        MainWindowDisplayState::Normal
    );
    assert_eq!(
        effective_main_display_state(MainWindowDisplayState::Normal, Some(&normal_test)),
        MainWindowDisplayState::Normal
    );
    // Test branch present, maximized=true: maximize applies even when saved is normal.
    assert_eq!(
        effective_main_display_state(MainWindowDisplayState::Normal, Some(&maximized_test)),
        MainWindowDisplayState::Maximized
    );
    assert_eq!(
        effective_main_display_state(MainWindowDisplayState::Maximized, Some(&maximized_test)),
        MainWindowDisplayState::Maximized
    );
}

#[test]
fn issue_2348_apply_main_display_state_maximizes_only_for_maximized() {
    let calls = std::cell::Cell::new(0);
    apply_main_display_state(MainWindowDisplayState::Normal, || {
        calls.set(calls.get() + 1);
        Ok::<(), String>(())
    });
    assert_eq!(calls.get(), 0, "Normal must not maximize");

    apply_main_display_state(MainWindowDisplayState::Maximized, || {
        calls.set(calls.get() + 1);
        Ok::<(), String>(())
    });
    assert_eq!(calls.get(), 1, "Maximized must maximize exactly once");
}

#[test]
fn issue_2348_apply_main_display_state_survives_a_maximize_failure() {
    let attempted = std::cell::Cell::new(false);
    apply_main_display_state(MainWindowDisplayState::Maximized, || {
        attempted.set(true);
        Err::<(), String>("simulated maximize failure".to_string())
    });
    assert!(attempted.get(), "the maximize attempt must have run");
    // Reaching this line proves the injected failure was handled as a warning
    // and startup continued with a usable normal window.
}
