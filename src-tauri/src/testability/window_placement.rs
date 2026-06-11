use serde::{Deserialize, Serialize};
use std::path::Path;

pub const TESTABLE_EXE_NAME: &str = "agentscommander_testeable.exe";
pub const PLACEMENT_REQUIRES_TESTABLE: &str = "test_placement_requires_testeable_binary";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TestWindowPlacement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub maximized: bool,
}

pub fn resolve_from_cli_or_env(
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    maximized: bool,
) -> Result<Option<TestWindowPlacement>, String> {
    let exe_path =
        std::env::current_exe().map_err(|e| format!("failed_to_resolve_current_exe: {e}"))?;
    let env_value = std::env::var("AC_TEST_WINDOW_PLACEMENT").ok();
    resolve_from_cli_or_env_for_exe(
        &exe_path,
        env_value.as_deref(),
        x,
        y,
        width,
        height,
        maximized,
    )
}

fn resolve_from_cli_or_env_for_exe(
    exe_path: &Path,
    env_value: Option<&str>,
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    maximized: bool,
) -> Result<Option<TestWindowPlacement>, String> {
    let cli_has_rect = x.is_some() || y.is_some() || width.is_some() || height.is_some();
    let has_input = cli_has_rect || maximized || env_value.is_some();
    if !has_input {
        return Ok(None);
    }

    let exe_name = exe_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if exe_name != TESTABLE_EXE_NAME {
        return Err(PLACEMENT_REQUIRES_TESTABLE.to_string());
    }

    if cli_has_rect || maximized {
        let (Some(x), Some(y), Some(width), Some(height)) = (x, y, width, height) else {
            return Err(
                "test_window_placement_requires_x_y_width_height_when_any_cli_rect_arg_is_present"
                    .to_string(),
            );
        };
        return validate(TestWindowPlacement {
            x,
            y,
            width,
            height,
            maximized,
        })
        .map(Some);
    }

    let raw = env_value.expect("env value exists when input exists");
    let parsed: TestWindowPlacement = serde_json::from_str(raw)
        .map_err(|e| format!("invalid_AC_TEST_WINDOW_PLACEMENT_json: {e}"))?;
    validate(parsed).map(Some)
}

fn validate(placement: TestWindowPlacement) -> Result<TestWindowPlacement, String> {
    for (name, value) in [
        ("x", placement.x),
        ("y", placement.y),
        ("width", placement.width),
        ("height", placement.height),
    ] {
        if !value.is_finite() {
            return Err(format!("test_window_placement_{name}_must_be_finite"));
        }
    }
    if placement.width <= 0.0 {
        return Err("test_window_placement_width_must_be_positive".to_string());
    }
    if placement.height <= 0.0 {
        return Err("test_window_placement_height_must_be_positive".to_string());
    }
    Ok(placement)
}

#[cfg(test)]
mod tests {
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
}
