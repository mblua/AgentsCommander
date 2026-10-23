use super::pid_is_alive;
#[cfg(target_os = "windows")]
use super::process_path;
use super::{
    current_exe_path_string, now_unix_ms, paths_equal_for_compare, read_session_file_with_retry,
    retry_remove_file, write_json_atomic_new, CliRequest, UiAutomationRequest,
    UiAutomationResponse, UiAutomationSession, BACKEND_AUTOMATION_WINDOW,
    CLI_MAX_AVAILABLE_TARGETS, CLI_MAX_TARGET_TEXT_CHARS, ENV_ENABLE, POLL_MS, REQUESTS_DIR,
    RESPONSES_DIR, SESSION_FILE, UI_AUTOMATION_DIR,
};
use crate::testability::window_placement::TESTABLE_EXE_NAME;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{fs, io};
use uuid::Uuid;

pub(super) fn run_cli_request(input: &CliRequest) -> Result<UiAutomationResponse, Value> {
    ensure_current_exe_is_testable()?;

    let config_dir = config_dir_or_error()?;
    let automation_dir = config_dir.join(UI_AUTOMATION_DIR);
    let session_path = automation_dir.join(SESSION_FILE);
    let session = load_session_for_cli(&session_path, &input.window)?;

    fs::create_dir_all(automation_dir.join(REQUESTS_DIR)).map_err(|e| {
        preflight_error(
            "automation_filesystem_error",
            "Failed to create automation request directory.",
            Some(json!({
                "operation": "create_requests_dir",
                "path": automation_dir.join(REQUESTS_DIR).to_string_lossy(),
                "message": e.to_string(),
            })),
        )
    })?;
    fs::create_dir_all(automation_dir.join(RESPONSES_DIR)).map_err(|e| {
        preflight_error(
            "automation_filesystem_error",
            "Failed to create automation response directory.",
            Some(json!({
                "operation": "create_responses_dir",
                "path": automation_dir.join(RESPONSES_DIR).to_string_lossy(),
                "message": e.to_string(),
            })),
        )
    })?;

    let request_id = Uuid::new_v4().to_string();
    let request = UiAutomationRequest {
        request_id: request_id.clone(),
        token: session.token,
        window: input.window.clone(),
        action: input.action,
        selector: input.selector.clone(),
        value: input.value.clone(),
        expires_at_unix_ms: Some(request_expires_at_unix_ms(input.timeout_ms)),
    };

    let request_path = automation_dir
        .join(REQUESTS_DIR)
        .join(format!("{request_id}.json"));
    let inflight_path = automation_dir
        .join(REQUESTS_DIR)
        .join(format!("{request_id}.inflight.json"));
    let response_path = automation_dir
        .join(RESPONSES_DIR)
        .join(format!("{request_id}.json"));

    write_json_atomic_new(&request_path, &request).map_err(|e| {
        response_error_value(
            &request,
            "automation_filesystem_error",
            "Failed to write automation request file.",
            Some(json!({
                "operation": "write_request",
                "path": request_path.to_string_lossy(),
                "message": e.to_string(),
            })),
        )
    })?;

    let deadline = Instant::now() + Duration::from_millis(input.timeout_ms);
    loop {
        if response_path.exists() {
            let raw = fs::read_to_string(&response_path).map_err(|e| {
                response_error_value(
                    &request,
                    "automation_filesystem_error",
                    "Failed to read automation response file.",
                    Some(json!({
                        "operation": "read_response",
                        "path": response_path.to_string_lossy(),
                        "message": e.to_string(),
                    })),
                )
            })?;
            let response: UiAutomationResponse = serde_json::from_str(&raw).map_err(|e| {
                response_error_value(
                    &request,
                    "automation_filesystem_error",
                    "Automation response file was not valid JSON.",
                    Some(json!({
                        "operation": "parse_response",
                        "path": response_path.to_string_lossy(),
                        "message": e.to_string(),
                    })),
                )
            })?;
            let _ = retry_remove_file(&response_path);
            let _ = retry_remove_file(&request_path);
            let _ = retry_remove_file(&inflight_path);
            return Ok(sanitize_response_for_cli(response));
        }

        if Instant::now() >= deadline {
            let mut timeout = UiAutomationResponse::error_for_request(
                &request,
                "timeout",
                "Automation request timed out before the GUI returned a response.",
            );
            timeout.timeout_ms = Some(input.timeout_ms);
            timeout.phase = Some(timeout_phase(
                &request_path,
                &inflight_path,
                &session_path,
                &input.window,
            ));
            let _ = retry_remove_file(&request_path);
            let _ = retry_remove_file(&inflight_path);
            return Ok(timeout);
        }

        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

pub fn resolve_enabled_from_cli_or_env(ui_automation: bool) -> Result<bool, String> {
    let env_enabled = std::env::var(ENV_ENABLE)
        .ok()
        .is_some_and(|value| value == "1");
    let requested = ui_automation || env_enabled;
    if !requested {
        return Ok(false);
    }
    if !current_exe_is_testable() {
        return Err(preflight_error(
            "refusing_non_testeable_binary",
            "UI automation is only available from agentscommander_testeable.exe.",
            Some(json!({ "expected": TESTABLE_EXE_NAME })),
        )
        .to_string());
    }
    Ok(true)
}

pub fn existing_enabled_session_for_current_config() -> bool {
    let Some(config_dir) = crate::config::config_dir() else {
        return false;
    };
    let session_path = config_dir.join(UI_AUTOMATION_DIR).join(SESSION_FILE);
    let Ok(raw) = read_session_file_with_retry(&session_path) else {
        return false;
    };
    let Ok(session) = serde_json::from_str::<UiAutomationSession>(&raw) else {
        return false;
    };
    validate_session_liveness(&session).is_ok()
}

pub fn automation_not_enabled_json() -> String {
    automation_not_enabled_error().to_string()
}

pub(super) fn automation_not_enabled_error() -> Value {
    preflight_error(
        "automation_not_enabled",
        "A testable GUI is already running without UI automation enabled. Restart it with \
            --ui-automation or AC_UI_AUTOMATION=1.",
        None,
    )
}

pub(super) fn load_session_for_cli(
    path: &Path,
    requested_window: &str,
) -> Result<UiAutomationSession, Value> {
    let raw = match read_session_file_with_retry(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let (error, message) = match crate::config::daemon_pid::detect_daemon_state() {
                crate::config::daemon_pid::DaemonState::Running { .. } => {
                    return Err(automation_not_enabled_error());
                }
                _ => (
                    "automation_session_missing",
                    "No UI automation session file exists for this testable binary.",
                ),
            };
            return Err(preflight_error(error, message, None));
        }
        Err(e) => {
            return Err(preflight_error(
                "automation_filesystem_error",
                "Failed to read UI automation session file.",
                Some(json!({
                    "operation": "read_session",
                    "path": path.to_string_lossy(),
                    "message": e.to_string(),
                })),
            ));
        }
    };

    let session: UiAutomationSession = serde_json::from_str(&raw).map_err(|e| {
        preflight_error(
            "automation_session_stale",
            "UI automation session file is malformed.",
            Some(json!({ "message": e.to_string() })),
        )
    })?;

    if let Err(e) = validate_session_liveness(&session) {
        if matches!(
            crate::config::daemon_pid::detect_daemon_state(),
            crate::config::daemon_pid::DaemonState::Running { .. }
        ) {
            let _ = retry_remove_file(path);
            return Err(automation_not_enabled_error());
        }
        return Err(e);
    }
    if !is_backend_automation_window(requested_window)
        && !session
            .window_labels
            .iter()
            .any(|label| label == requested_window)
    {
        let mut value = preflight_error(
            "window_unavailable",
            "Requested automation window is not registered in the running session.",
            None,
        );
        if let Some(obj) = value.as_object_mut() {
            obj.insert("availableWindows".to_string(), json!(session.window_labels));
        }
        return Err(value);
    }
    Ok(session)
}

pub(super) fn validate_session_liveness(session: &UiAutomationSession) -> Result<(), Value> {
    if session.pid == 0 || !pid_is_alive(session.pid) {
        return Err(preflight_error(
            "automation_session_stale",
            "UI automation session PID is not alive.",
            Some(json!({ "pid": session.pid })),
        ));
    }

    let current_exe = current_exe_path_string();
    if !paths_equal_for_compare(
        &PathBuf::from(&session.exe_path),
        &PathBuf::from(&current_exe),
    ) {
        return Err(preflight_error(
            "automation_session_stale",
            "UI automation session belongs to a different executable path.",
            Some(json!({ "sessionExePath": session.exe_path })),
        ));
    }

    #[cfg(target_os = "windows")]
    if let Some(process_path) = process_path(session.pid) {
        if !paths_equal_for_compare(&process_path, &PathBuf::from(&session.exe_path)) {
            return Err(preflight_error(
                "automation_session_stale",
                "UI automation session PID now belongs to a different executable.",
                Some(json!({ "pid": session.pid })),
            ));
        }
    }

    Ok(())
}

pub(super) fn ensure_current_exe_is_testable() -> Result<(), Value> {
    if current_exe_is_testable() {
        Ok(())
    } else {
        Err(preflight_error(
            "refusing_non_testeable_binary",
            "UI automation is only available from agentscommander_testeable.exe.",
            Some(json!({ "expected": TESTABLE_EXE_NAME })),
        ))
    }
}

pub(super) fn current_exe_is_testable() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .is_some_and(|name| name == TESTABLE_EXE_NAME)
}

pub(super) fn config_dir_or_error() -> Result<PathBuf, Value> {
    crate::config::config_dir().ok_or_else(|| {
        preflight_error(
            "automation_filesystem_error",
            "Could not resolve the current binary config directory.",
            None,
        )
    })
}

pub(super) fn preflight_error(error: &str, message: &str, diagnostics: Option<Value>) -> Value {
    let mut value = json!({
        "ok": false,
        "error": error,
        "message": message,
        "currentExePath": current_exe_path_string(),
    });
    if let Some(config_dir) = crate::config::config_dir() {
        value["configDir"] = json!(config_dir.to_string_lossy().into_owned());
    }
    if let Some(diagnostics) = diagnostics {
        value["diagnostics"] = diagnostics;
    }
    value
}

pub(super) fn response_error_value(
    request: &UiAutomationRequest,
    error: &str,
    message: &str,
    diagnostics: Option<Value>,
) -> Value {
    let mut response = UiAutomationResponse::error_for_request(request, error, message);
    response.diagnostics = diagnostics;
    serde_json::to_value(response).unwrap_or_else(|_| {
        json!({
            "ok": false,
            "error": "json_serialize_failed",
            "message": "Failed to serialize automation response."
        })
    })
}

pub(super) fn request_expires_at_unix_ms(timeout_ms: u64) -> i64 {
    let timeout_ms = timeout_ms.min(i64::MAX as u64) as i64;
    now_unix_ms().saturating_add(timeout_ms)
}

pub(super) fn request_expired(request: &UiAutomationRequest, now_ms: i64) -> bool {
    request
        .expires_at_unix_ms
        .is_some_and(|expires_at| expires_at <= now_ms)
}

pub(super) fn expired_response_for_request(request: &UiAutomationRequest) -> UiAutomationResponse {
    let mut response = UiAutomationResponse::error_for_request(
        request,
        "timeout",
        "Automation request expired before the GUI emitted it to the frontend.",
    );
    response.diagnostics = Some(json!({ "expiresAtUnixMs": request.expires_at_unix_ms }));
    response
}

pub(super) fn sanitize_response_for_cli(
    mut response: UiAutomationResponse,
) -> UiAutomationResponse {
    let truncation = response.available.as_mut().and_then(|available| {
        let total = available.len();
        for target in available.iter_mut() {
            sanitize_target_for_cli(target);
        }
        if total > CLI_MAX_AVAILABLE_TARGETS {
            available.truncate(CLI_MAX_AVAILABLE_TARGETS);
            Some((total, CLI_MAX_AVAILABLE_TARGETS))
        } else {
            None
        }
    });

    if let Some((total, limit)) = truncation {
        add_cli_truncation_diagnostics(&mut response, total, limit);
    }

    response
}

pub(super) fn sanitize_target_for_cli(target: &mut Value) {
    let Some(obj) = target.as_object_mut() else {
        return;
    };
    let Some(Value::String(text)) = obj.get_mut("text") else {
        return;
    };
    if text.chars().count() <= CLI_MAX_TARGET_TEXT_CHARS {
        return;
    }
    let mut truncated = text
        .chars()
        .take(CLI_MAX_TARGET_TEXT_CHARS)
        .collect::<String>();
    truncated.push_str("...");
    *text = truncated;
}

pub(super) fn add_cli_truncation_diagnostics(
    response: &mut UiAutomationResponse,
    available_total: usize,
    available_limit: usize,
) {
    let mut diagnostics = response
        .diagnostics
        .take()
        .filter(|value| value.is_object())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();

    diagnostics.insert("availableTotal".to_string(), json!(available_total));
    diagnostics.insert("availableLimit".to_string(), json!(available_limit));
    diagnostics.insert("availableTruncated".to_string(), json!(true));
    response.diagnostics = Some(Value::Object(diagnostics));
}

pub(super) fn print_stdout_json<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(json) => crate::cli_println!("{json}"),
        Err(e) => crate::cli_println!(
            "{{\"ok\":false,\"error\":\"json_serialize_failed\",\"message\":{}}}",
            serde_json::to_string(&e.to_string()).unwrap_or_else(|_| "\"unknown\"".to_string())
        ),
    }
}

pub(super) fn timeout_phase(
    request_path: &Path,
    inflight_path: &Path,
    session_path: &Path,
    window: &str,
) -> String {
    if request_path.exists() {
        return "awaiting_gui_poller".to_string();
    }
    if inflight_path.exists() {
        if is_backend_automation_window(window) {
            return "awaiting_backend_response".to_string();
        }
        if session_ready_for_window(session_path, window) {
            "awaiting_frontend_response".to_string()
        } else {
            "awaiting_frontend_ready".to_string()
        }
    } else {
        "awaiting_gui_poller".to_string()
    }
}

pub(super) fn session_ready_for_window(path: &Path, window: &str) -> bool {
    if is_backend_automation_window(window) {
        return true;
    }
    read_session_file_with_retry(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<UiAutomationSession>(&raw).ok())
        .is_some_and(|session| {
            session
                .ready_window_labels
                .iter()
                .any(|label| label == window)
        })
}

pub(super) fn is_backend_automation_window(window: &str) -> bool {
    window == BACKEND_AUTOMATION_WINDOW
}
