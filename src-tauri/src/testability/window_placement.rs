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
    let env_value = crate::config::environment::test_window_placement();
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
mod tests;
