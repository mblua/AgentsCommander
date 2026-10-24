use super::*;

fn exe(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!(r"C:\tmp\{name}"))
}

#[test]
fn no_args_and_no_env_returns_none() {
    let got = resolve_from_cli_or_env_for_exe(
        &exe(TESTABLE_EXE_NAME),
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .unwrap();
    assert_eq!(got, None);
}

#[test]
fn partial_cli_args_error() {
    let err = resolve_from_cli_or_env_for_exe(
        &exe(TESTABLE_EXE_NAME),
        None,
        Some(1.0),
        None,
        Some(100.0),
        Some(100.0),
        false,
    )
    .unwrap_err();
    assert!(err.contains("requires_x_y_width_height"));
}

#[test]
fn cli_args_override_env() {
    let got = resolve_from_cli_or_env_for_exe(
        &exe(TESTABLE_EXE_NAME),
        Some(r#"{"x":9,"y":9,"width":9,"height":9,"maximized":false}"#),
        Some(-1.0),
        Some(-2.0),
        Some(300.0),
        Some(200.0),
        true,
    )
    .unwrap()
    .unwrap();
    assert_eq!(got.x, -1.0);
    assert_eq!(got.y, -2.0);
    assert_eq!(got.width, 300.0);
    assert_eq!(got.height, 200.0);
    assert!(got.maximized);
}

#[test]
fn env_json_parses_negative_coordinates() {
    let got = resolve_from_cli_or_env_for_exe(
        &exe(TESTABLE_EXE_NAME),
        Some(r#"{"x":-2891,"y":-11,"width":1942,"height":1102,"maximized":true}"#),
        None,
        None,
        None,
        None,
        false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(got.x, -2891.0);
    assert_eq!(got.y, -11.0);
    assert!(got.maximized);
}

#[test]
fn invalid_width_errors() {
    let err = resolve_from_cli_or_env_for_exe(
        &exe(TESTABLE_EXE_NAME),
        None,
        Some(0.0),
        Some(0.0),
        Some(0.0),
        Some(100.0),
        false,
    )
    .unwrap_err();
    assert_eq!(err, "test_window_placement_width_must_be_positive");
}

#[test]
fn non_finite_cli_values_are_rejected() {
    for (field, values) in [
        ("x", [f64::NAN, f64::INFINITY, f64::NEG_INFINITY]),
        ("y", [f64::NAN, f64::INFINITY, f64::NEG_INFINITY]),
        ("width", [f64::NAN, f64::INFINITY, f64::NEG_INFINITY]),
        ("height", [f64::NAN, f64::INFINITY, f64::NEG_INFINITY]),
    ] {
        for value in values {
            let (x, y, width, height) = match field {
                "x" => (value, 0.0, 100.0, 100.0),
                "y" => (0.0, value, 100.0, 100.0),
                "width" => (0.0, 0.0, value, 100.0),
                "height" => (0.0, 0.0, 100.0, value),
                _ => unreachable!(),
            };
            let err = resolve_from_cli_or_env_for_exe(
                &exe(TESTABLE_EXE_NAME),
                None,
                Some(x),
                Some(y),
                Some(width),
                Some(height),
                false,
            )
            .unwrap_err();
            assert_eq!(err, format!("test_window_placement_{field}_must_be_finite"));
        }
    }
}

#[test]
fn malformed_env_on_testable_errors() {
    let err = resolve_from_cli_or_env_for_exe(
        &exe(TESTABLE_EXE_NAME),
        Some("not-json"),
        None,
        None,
        None,
        None,
        false,
    )
    .unwrap_err();
    assert!(err.contains("invalid_AC_TEST_WINDOW_PLACEMENT_json"));
}

#[test]
fn malformed_env_on_non_testable_fails_closed() {
    let err = resolve_from_cli_or_env_for_exe(
        &exe("agentscommander.exe"),
        Some("not-json"),
        None,
        None,
        None,
        None,
        false,
    )
    .unwrap_err();
    assert_eq!(err, PLACEMENT_REQUIRES_TESTABLE);
}
