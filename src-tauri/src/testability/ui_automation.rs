use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::shutdown::ShutdownSignal;
use crate::testability::window_placement::TESTABLE_EXE_NAME;

pub const UI_AUTOMATION_DIR: &str = "ui-automation";
pub const SESSION_FILE: &str = "session.json";
pub const REQUESTS_DIR: &str = "requests";
pub const RESPONSES_DIR: &str = "responses";
pub const ENV_ENABLE: &str = "AC_UI_AUTOMATION";

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const POLL_MS: u64 = 50;
const FS_RETRY_COUNT: usize = 8;
const FS_RETRY_DELAY_MS: u64 = 25;
const SESSION_READ_RETRY_COUNT: usize = 8;
const SESSION_READ_RETRY_DELAY_MS: u64 = 25;

#[derive(Debug, Args)]
pub struct UiQueryArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiClickArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiSetArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long)]
    pub value: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiWaitArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiAutomationSession {
    pub pid: u32,
    pub token: String,
    pub exe_path: String,
    pub config_dir: String,
    pub window_labels: Vec<String>,
    pub ready_window_labels: Vec<String>,
    pub started_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiAutomationRequest {
    pub request_id: String,
    pub token: String,
    pub window: String,
    pub action: UiAutomationAction,
    pub selector: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UiAutomationAction {
    Query,
    Click,
    SetValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiAutomationResponse {
    pub ok: bool,
    pub request_id: String,
    pub window: String,
    pub action: UiAutomationAction,
    pub selector: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_windows: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingRequest {
    request: UiAutomationRequest,
    response_path: PathBuf,
    inflight_path: PathBuf,
}

#[derive(Clone)]
pub struct UiAutomationState {
    inner: Arc<UiAutomationInner>,
}

struct UiAutomationInner {
    enabled: bool,
    token: String,
    config_dir: PathBuf,
    automation_dir: PathBuf,
    session_path: PathBuf,
    window_labels: Mutex<HashSet<String>>,
    ready_window_labels: Mutex<HashSet<String>>,
    pending: Mutex<HashMap<String, PendingRequest>>,
    available: AtomicBool,
    exe_path: String,
    started_at_unix_ms: i64,
}

impl UiAutomationState {
    pub fn new(enabled: bool, config_dir: PathBuf) -> Self {
        let automation_dir = config_dir.join(UI_AUTOMATION_DIR);
        let session_path = automation_dir.join(SESSION_FILE);
        let exe_path = current_exe_path_string();
        let mut window_labels = HashSet::new();
        window_labels.insert("main".to_string());

        Self {
            inner: Arc::new(UiAutomationInner {
                enabled,
                token: if enabled {
                    Uuid::new_v4().to_string()
                } else {
                    String::new()
                },
                config_dir,
                automation_dir,
                session_path,
                window_labels: Mutex::new(window_labels),
                ready_window_labels: Mutex::new(HashSet::new()),
                pending: Mutex::new(HashMap::new()),
                available: AtomicBool::new(enabled),
                exe_path,
                started_at_unix_ms: now_unix_ms(),
            }),
        }
    }

    pub fn enabled(&self) -> bool {
        self.inner.enabled && self.inner.available.load(Ordering::SeqCst)
    }

    pub fn start(&self, app: AppHandle, shutdown: ShutdownSignal) {
        if !self.inner.enabled {
            return;
        }

        if let Err(e) = self.initialize_files() {
            self.mark_unavailable();
            log::error!("[ui-automation] failed to initialize files: {}", e);
            return;
        }
        self.inner.available.store(true, Ordering::SeqCst);

        let state = self.clone();
        let shutdown = shutdown.token().clone();
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(POLL_MS));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => state.poll_once(&app),
                }
            }
        });
    }

    pub fn cleanup_session_file(&self) {
        if self.enabled() {
            if let Err(e) = retry_remove_file(&self.inner.session_path) {
                log::warn!(
                    "[ui-automation] failed to remove session file {}: {}",
                    self.inner.session_path.display(),
                    e
                );
            }
        }
    }

    pub fn mark_frontend_ready(
        &self,
        caller_label: &str,
        claimed_label: Option<&str>,
    ) -> Result<(), String> {
        if !self.enabled() {
            return Err("automation_not_enabled".to_string());
        }
        if let Some(claimed_label) = claimed_label {
            if claimed_label != caller_label {
                return Err("frontend_ready_window_mismatch".to_string());
            }
        }
        {
            let labels = self
                .inner
                .window_labels
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !labels.contains(caller_label) {
                return Err("window_unavailable".to_string());
            }
        }
        {
            let mut ready = self
                .inner
                .ready_window_labels
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ready.insert(caller_label.to_string());
        }
        self.write_session_snapshot().map_err(|e| e.to_string())
    }

    pub fn complete(&self, caller_label: &str, result: UiAutomationResponse) -> Result<(), String> {
        if !self.enabled() {
            return Err("automation_not_enabled".to_string());
        }

        let pending = {
            let mut pending = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.remove(&result.request_id)
        };
        let Some(pending) = pending else {
            return Err("unknown_request_id".to_string());
        };

        let response = if caller_label != pending.request.window
            || result.window != pending.request.window
            || result.action != pending.request.action
            || result.selector != pending.request.selector
        {
            UiAutomationResponse::error_for_request(
                &pending.request,
                "completion_mismatch",
                "Frontend completion did not match the pending automation request.",
            )
        } else {
            result
        };

        write_json_atomic_new(&pending.response_path, &response).map_err(|e| e.to_string())?;
        let _ = retry_remove_file(&pending.inflight_path);
        Ok(())
    }

    fn initialize_files(&self) -> io::Result<()> {
        cleanup_stale_automation_files(&self.inner.automation_dir)?;
        fs::create_dir_all(self.requests_dir())?;
        fs::create_dir_all(self.responses_dir())?;
        self.write_session_snapshot()
    }

    fn mark_unavailable(&self) {
        self.inner.available.store(false, Ordering::SeqCst);
    }

    fn write_session_snapshot(&self) -> io::Result<()> {
        let window_labels = sorted_labels(
            &self
                .inner
                .window_labels
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        let ready_window_labels = sorted_labels(
            &self
                .inner
                .ready_window_labels
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        let session = UiAutomationSession {
            pid: std::process::id(),
            token: self.inner.token.clone(),
            exe_path: self.inner.exe_path.clone(),
            config_dir: self.inner.config_dir.to_string_lossy().into_owned(),
            window_labels,
            ready_window_labels,
            started_at_unix_ms: self.inner.started_at_unix_ms,
        };
        write_json_atomic_replace(&self.inner.session_path, &session)
    }

    fn poll_once(&self, app: &AppHandle) {
        if let Err(e) = self.poll_once_inner(app) {
            log::warn!("[ui-automation] poll failed: {}", e);
        }
    }

    fn poll_once_inner(&self, app: &AppHandle) -> io::Result<()> {
        let requests_dir = self.requests_dir();
        let entries = match fs::read_dir(&requests_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Some(request_file) = RequestFile::from_path(&path) else {
                continue;
            };
            if self.is_pending(&request_file.request_id) {
                continue;
            }
            self.process_request_file(app, request_file);
        }

        Ok(())
    }

    fn process_request_file(&self, app: &AppHandle, request_file: RequestFile) {
        let raw = match fs::read_to_string(&request_file.path) {
            Ok(raw) => raw,
            Err(e) => {
                log::warn!(
                    "[ui-automation] failed to read request {}: {}",
                    request_file.path.display(),
                    e
                );
                return;
            }
        };
        let request: UiAutomationRequest = match serde_json::from_str(&raw) {
            Ok(request) => request,
            Err(e) => {
                let response = UiAutomationResponse::minimal_error(
                    &request_file.request_id,
                    "main",
                    UiAutomationAction::Query,
                    "",
                    "malformed_request",
                    &format!("Request file was not valid JSON: {e}"),
                );
                let _ = self.write_direct_response(&request_file, &response);
                return;
            }
        };

        if request.request_id != request_file.request_id {
            let response = UiAutomationResponse::error_for_request(
                &request,
                "malformed_request",
                "Request id did not match its request filename.",
            );
            let _ = self.write_direct_response(&request_file, &response);
            return;
        }

        if request_expired(&request, now_unix_ms()) {
            let response = expired_response_for_request(&request);
            let _ = self.write_direct_response(&request_file, &response);
            return;
        }

        if request.token != self.inner.token {
            let response = UiAutomationResponse::error_for_request(
                &request,
                "automation_session_stale",
                "Automation token did not match the running GUI session.",
            );
            let _ = self.write_direct_response(&request_file, &response);
            return;
        }

        let available_windows = available_window_labels(app);
        if !available_windows
            .iter()
            .any(|label| label == &request.window)
        {
            let mut response = UiAutomationResponse::error_for_request(
                &request,
                "window_unavailable",
                "Requested automation window is not live.",
            );
            response.available_windows = Some(available_windows);
            let _ = self.write_direct_response(&request_file, &response);
            return;
        }

        if !self.is_window_ready(&request.window) {
            return;
        }

        let inflight_path = match self.ensure_inflight(&request_file) {
            Ok(path) => path,
            Err(e) => {
                let mut response = UiAutomationResponse::error_for_request(
                    &request,
                    "automation_filesystem_error",
                    "Failed to mark automation request as inflight.",
                );
                response.diagnostics = Some(json!({
                    "operation": "rename_request_inflight",
                    "path": request_file.path.to_string_lossy(),
                    "message": e.to_string(),
                }));
                let _ = self.write_direct_response(&request_file, &response);
                return;
            }
        };

        if request_expired(&request, now_unix_ms()) {
            let response = expired_response_for_request(&request);
            let _ = write_json_atomic_new(&self.response_path(&request.request_id), &response);
            let _ = retry_remove_file(&inflight_path);
            return;
        }

        let response_path = self.response_path(&request.request_id);
        let pending = PendingRequest {
            request: request.clone(),
            response_path,
            inflight_path,
        };
        {
            let mut pending_map = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending_map.insert(request.request_id.clone(), pending);
        }

        if let Err(e) = app.emit_to(&request.window, "ui_automation_request", &request) {
            let pending = {
                let mut pending_map = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
                pending_map.remove(&request.request_id)
            };
            if let Some(pending) = pending {
                let mut response = UiAutomationResponse::error_for_request(
                    &request,
                    "webview_unavailable",
                    "Failed to emit automation request to the requested WebView.",
                );
                response.available_windows = Some(available_window_labels(app));
                response.diagnostics = Some(json!({ "message": e.to_string() }));
                let _ = write_json_atomic_new(&pending.response_path, &response);
                let _ = retry_remove_file(&pending.inflight_path);
            }
        }
    }

    fn is_pending(&self, request_id: &str) -> bool {
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(request_id)
    }

    fn is_window_ready(&self, window: &str) -> bool {
        self.inner
            .ready_window_labels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(window)
    }

    fn ensure_inflight(&self, request_file: &RequestFile) -> io::Result<PathBuf> {
        match request_file.kind {
            RequestFileKind::Inflight => Ok(request_file.path.clone()),
            RequestFileKind::Ready => {
                let inflight_path = self.inflight_path(&request_file.request_id);
                retry_rename(&request_file.path, &inflight_path)?;
                Ok(inflight_path)
            }
        }
    }

    fn write_direct_response(
        &self,
        request_file: &RequestFile,
        response: &UiAutomationResponse,
    ) -> io::Result<()> {
        let response_path = self.response_path(&request_file.request_id);
        write_json_atomic_new(&response_path, response)?;
        let _ = retry_remove_file(&request_file.path);
        Ok(())
    }

    fn requests_dir(&self) -> PathBuf {
        self.inner.automation_dir.join(REQUESTS_DIR)
    }

    fn responses_dir(&self) -> PathBuf {
        self.inner.automation_dir.join(RESPONSES_DIR)
    }

    fn inflight_path(&self, request_id: &str) -> PathBuf {
        self.requests_dir()
            .join(format!("{request_id}.inflight.json"))
    }

    fn response_path(&self, request_id: &str) -> PathBuf {
        self.responses_dir().join(format!("{request_id}.json"))
    }
}

impl UiAutomationResponse {
    fn minimal_error(
        request_id: &str,
        window: &str,
        action: UiAutomationAction,
        selector: &str,
        error: &str,
        message: &str,
    ) -> Self {
        Self {
            ok: false,
            request_id: request_id.to_string(),
            window: window.to_string(),
            action,
            selector: selector.to_string(),
            target: None,
            error: Some(error.to_string()),
            message: Some(message.to_string()),
            available: None,
            diagnostics: None,
            available_windows: None,
            timeout_ms: None,
            phase: None,
        }
    }

    fn error_for_request(request: &UiAutomationRequest, error: &str, message: &str) -> Self {
        Self::minimal_error(
            &request.request_id,
            &request.window,
            request.action,
            &request.selector,
            error,
            message,
        )
    }
}

#[derive(Debug)]
struct CliRequest {
    window: String,
    selector: String,
    action: UiAutomationAction,
    value: Option<String>,
    timeout_ms: u64,
}

pub fn execute_query(args: UiQueryArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::Query,
        value: None,
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_click(args: UiClickArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::Click,
        value: None,
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_set(args: UiSetArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::SetValue,
        value: Some(args.value),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_wait(args: UiWaitArgs) -> i32 {
    let deadline = Instant::now() + Duration::from_millis(args.timeout_ms);
    let mut last_response: Option<UiAutomationResponse> = None;

    loop {
        let remaining_ms = deadline
            .checked_duration_since(Instant::now())
            .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0);
        if remaining_ms == 0 {
            let timeout = last_response.unwrap_or_else(|| {
                UiAutomationResponse::minimal_error(
                    &Uuid::new_v4().to_string(),
                    &args.window,
                    UiAutomationAction::Query,
                    &args.selector,
                    "timeout",
                    "Automation wait timed out before the selector became available.",
                )
            });
            let mut timeout = timeout;
            timeout.ok = false;
            timeout.error = Some("timeout".to_string());
            timeout.message =
                Some("Automation wait timed out before the selector became available.".to_string());
            timeout.timeout_ms = Some(args.timeout_ms);
            timeout.phase = Some("wait_condition_not_met".to_string());
            print_stderr_json(&timeout);
            return 1;
        }

        let input = CliRequest {
            window: args.window.clone(),
            selector: args.selector.clone(),
            action: UiAutomationAction::Query,
            value: None,
            timeout_ms: remaining_ms.min(500),
        };

        match run_cli_request(&input) {
            Ok(response) if response.ok => {
                print_stdout_json(&response);
                return 0;
            }
            Ok(response) => {
                last_response = Some(response);
            }
            Err(error) => {
                print_stderr_json(&error);
                return 1;
            }
        }

        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

fn execute_cli(input: CliRequest) -> i32 {
    match run_cli_request(&input) {
        Ok(response) if response.ok => {
            print_stdout_json(&response);
            0
        }
        Ok(response) => {
            print_stderr_json(&response);
            1
        }
        Err(error) => {
            print_stderr_json(&error);
            1
        }
    }
}

fn run_cli_request(input: &CliRequest) -> Result<UiAutomationResponse, Value> {
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
            return Ok(response);
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
    preflight_error(
        "automation_not_enabled",
        "A testable GUI is already running without UI automation enabled. Restart it with --ui-automation or AC_UI_AUTOMATION=1.",
        None,
    )
    .to_string()
}

fn load_session_for_cli(path: &Path, requested_window: &str) -> Result<UiAutomationSession, Value> {
    let raw = match read_session_file_with_retry(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let state = crate::config::daemon_pid::detect_daemon_state();
            let (error, message) = match state {
                crate::config::daemon_pid::DaemonState::Running { .. } => (
                    "automation_not_enabled",
                    "The testable GUI is running, but UI automation was not enabled at startup.",
                ),
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

    validate_session_liveness(&session)?;
    if !session
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

fn validate_session_liveness(session: &UiAutomationSession) -> Result<(), Value> {
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

fn ensure_current_exe_is_testable() -> Result<(), Value> {
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

fn current_exe_is_testable() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .is_some_and(|name| name == TESTABLE_EXE_NAME)
}

fn config_dir_or_error() -> Result<PathBuf, Value> {
    crate::config::config_dir().ok_or_else(|| {
        preflight_error(
            "automation_filesystem_error",
            "Could not resolve the current binary config directory.",
            None,
        )
    })
}

fn preflight_error(error: &str, message: &str, diagnostics: Option<Value>) -> Value {
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

fn response_error_value(
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

fn request_expires_at_unix_ms(timeout_ms: u64) -> i64 {
    let timeout_ms = timeout_ms.min(i64::MAX as u64) as i64;
    now_unix_ms().saturating_add(timeout_ms)
}

fn request_expired(request: &UiAutomationRequest, now_ms: i64) -> bool {
    request
        .expires_at_unix_ms
        .is_some_and(|expires_at| expires_at <= now_ms)
}

fn expired_response_for_request(request: &UiAutomationRequest) -> UiAutomationResponse {
    let mut response = UiAutomationResponse::error_for_request(
        request,
        "timeout",
        "Automation request expired before the GUI emitted it to the frontend.",
    );
    response.diagnostics = Some(json!({ "expiresAtUnixMs": request.expires_at_unix_ms }));
    response
}

fn print_stdout_json<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(json) => crate::cli_println!("{json}"),
        Err(e) => crate::cli_println!(
            "{{\"ok\":false,\"error\":\"json_serialize_failed\",\"message\":{}}}",
            serde_json::to_string(&e.to_string()).unwrap_or_else(|_| "\"unknown\"".to_string())
        ),
    }
}

fn print_stderr_json<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(json) => eprintln!("{json}"),
        Err(e) => eprintln!(
            "{{\"ok\":false,\"error\":\"json_serialize_failed\",\"message\":{}}}",
            serde_json::to_string(&e.to_string()).unwrap_or_else(|_| "\"unknown\"".to_string())
        ),
    }
}

fn timeout_phase(
    request_path: &Path,
    inflight_path: &Path,
    session_path: &Path,
    window: &str,
) -> String {
    if request_path.exists() {
        return "awaiting_gui_poller".to_string();
    }
    if inflight_path.exists() {
        if session_ready_for_window(session_path, window) {
            "awaiting_frontend_response".to_string()
        } else {
            "awaiting_frontend_ready".to_string()
        }
    } else {
        "awaiting_gui_poller".to_string()
    }
}

fn session_ready_for_window(path: &Path, window: &str) -> bool {
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

fn write_json_atomic_new<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_json_atomic(path, value, false)
}

fn write_json_atomic_replace<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_json_atomic(path, value, true)
}

fn read_session_file_with_retry(path: &Path) -> io::Result<String> {
    let mut last_not_found = None;
    for attempt in 0..SESSION_READ_RETRY_COUNT {
        match fs::read_to_string(path) {
            Ok(raw) => return Ok(raw),
            Err(e)
                if e.kind() == io::ErrorKind::NotFound
                    && attempt + 1 < SESSION_READ_RETRY_COUNT =>
            {
                last_not_found = Some(e);
                std::thread::sleep(Duration::from_millis(SESSION_READ_RETRY_DELAY_MS));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_not_found.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("session file not found: {}", path.display()),
        )
    }))
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T, replace: bool) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    if !replace && path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("destination already exists: {}", path.display()),
        ));
    }

    let tmp_path = path.with_extension("tmp");
    let mut file = if replace {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?
    } else {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?
    };
    serde_json::to_writer(&mut file, value)?;
    file.flush()?;
    drop(file);

    if replace {
        let _ = retry_remove_file(path);
    } else if path.exists() {
        let _ = retry_remove_file(&tmp_path);
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("destination already exists: {}", path.display()),
        ));
    }
    retry_rename(&tmp_path, path)
}

fn retry_rename(from: &Path, to: &Path) -> io::Result<()> {
    retry_fs(|| fs::rename(from, to))
}

fn retry_remove_file(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    retry_fs(|| fs::remove_file(path))
}

fn retry_remove_dir_all(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    retry_fs(|| fs::remove_dir_all(path))
}

fn retry_fs(mut op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let mut last_err = None;
    for attempt in 0..FS_RETRY_COUNT {
        match op() {
            Ok(()) => return Ok(()),
            Err(e) if is_transient_fs_error(&e) && attempt + 1 < FS_RETRY_COUNT => {
                last_err = Some(e);
                std::thread::sleep(Duration::from_millis(FS_RETRY_DELAY_MS));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("filesystem retry failed")))
}

fn is_transient_fs_error(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(32) | Some(33))
}

fn cleanup_stale_automation_files(automation_dir: &Path) -> io::Result<()> {
    let requests = automation_dir.join(REQUESTS_DIR);
    let responses = automation_dir.join(RESPONSES_DIR);
    if requests.exists() {
        retry_remove_dir_all(&requests)?;
    }
    if responses.exists() {
        retry_remove_dir_all(&responses)?;
    }
    Ok(())
}

fn sorted_labels(labels: &HashSet<String>) -> Vec<String> {
    let mut labels: Vec<String> = labels.iter().cloned().collect();
    labels.sort();
    labels
}

fn available_window_labels(app: &AppHandle) -> Vec<String> {
    let mut labels: Vec<String> = app.webview_windows().keys().cloned().collect();
    labels.sort();
    labels
}

fn current_exe_path_string() -> String {
    std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| String::new())
}

fn paths_equal_for_compare(a: &Path, b: &Path) -> bool {
    let a = canonical_for_compare(a);
    let b = canonical_for_compare(b);
    #[cfg(target_os = "windows")]
    {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    }
    #[cfg(not(target_os = "windows"))]
    {
        a == b
    }
}

fn canonical_for_compare(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
pub fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, FALSE, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if handle.is_null() {
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code: u32 = 0;
        let got_code = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        got_code != 0 && code == STILL_ACTIVE as u32
    }
}

#[cfg(not(target_os = "windows"))]
pub fn pid_is_alive(_pid: u32) -> bool {
    true
}

#[cfg(target_os = "windows")]
fn process_path(pid: u32) -> Option<PathBuf> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len);
        let _ = CloseHandle(handle);
        if ok == 0 || len == 0 {
            return None;
        }
        Some(PathBuf::from(String::from_utf16_lossy(
            &buf[..len as usize],
        )))
    }
}

#[derive(Debug)]
struct RequestFile {
    path: PathBuf,
    request_id: String,
    kind: RequestFileKind,
}

#[derive(Debug, Clone, Copy)]
enum RequestFileKind {
    Ready,
    Inflight,
}

impl RequestFile {
    fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        if name.ends_with(".tmp") {
            return None;
        }

        let (request_id, kind) = if let Some(id) = name.strip_suffix(".inflight.json") {
            (id, RequestFileKind::Inflight)
        } else if let Some(id) = name.strip_suffix(".json") {
            (id, RequestFileKind::Ready)
        } else {
            return None;
        };

        if Uuid::parse_str(request_id).is_err() {
            return None;
        }

        Some(Self {
            path: path.to_path_buf(),
            request_id: request_id.to_string(),
            kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request(request_id: String, selector: &str) -> UiAutomationRequest {
        UiAutomationRequest {
            request_id,
            token: "token".to_string(),
            window: "main".to_string(),
            action: UiAutomationAction::Query,
            selector: selector.to_string(),
            value: None,
            expires_at_unix_ms: Some(now_unix_ms() + 1_000),
        }
    }

    #[test]
    fn action_serializes_camel_case() {
        assert_eq!(
            serde_json::to_string(&UiAutomationAction::Query).unwrap(),
            "\"query\""
        );
        assert_eq!(
            serde_json::to_string(&UiAutomationAction::SetValue).unwrap(),
            "\"setValue\""
        );
    }

    #[test]
    fn request_file_parses_ready_and_inflight_names() {
        let id = Uuid::new_v4().to_string();
        let ready = RequestFile::from_path(Path::new(&format!("{id}.json"))).unwrap();
        assert_eq!(ready.request_id, id);
        matches!(ready.kind, RequestFileKind::Ready);

        let inflight = RequestFile::from_path(Path::new(&format!("{id}.inflight.json"))).unwrap();
        assert_eq!(inflight.request_id, id);
        matches!(inflight.kind, RequestFileKind::Inflight);

        assert!(RequestFile::from_path(Path::new("not-a-uuid.json")).is_none());
        assert!(RequestFile::from_path(Path::new(&format!("{id}.tmp"))).is_none());
    }

    #[test]
    fn request_expiry_uses_optional_deadline() {
        let mut request = sample_request(Uuid::new_v4().to_string(), "target");
        request.expires_at_unix_ms = None;
        assert!(!request_expired(&request, 100));

        request.expires_at_unix_ms = Some(99);
        assert!(request_expired(&request, 100));

        request.expires_at_unix_ms = Some(101);
        assert!(!request_expired(&request, 100));
    }

    #[test]
    fn complete_rejects_unknown_request_id() {
        let tmp = tempfile::tempdir().unwrap();
        let state = UiAutomationState::new(true, tmp.path().to_path_buf());
        let response = UiAutomationResponse::minimal_error(
            &Uuid::new_v4().to_string(),
            "main",
            UiAutomationAction::Query,
            "target",
            "frontend_error",
            "frontend failed",
        );

        assert_eq!(
            state.complete("main", response).unwrap_err(),
            "unknown_request_id"
        );
    }

    #[test]
    fn complete_writes_completion_mismatch_response() {
        let tmp = tempfile::tempdir().unwrap();
        let state = UiAutomationState::new(true, tmp.path().to_path_buf());
        fs::create_dir_all(state.requests_dir()).unwrap();
        fs::create_dir_all(state.responses_dir()).unwrap();

        let request_id = Uuid::new_v4().to_string();
        let request = sample_request(request_id.clone(), "expected");
        let response_path = state.response_path(&request_id);
        let inflight_path = state.inflight_path(&request_id);
        fs::write(&inflight_path, "{}").unwrap();
        state.inner.pending.lock().unwrap().insert(
            request_id.clone(),
            PendingRequest {
                request,
                response_path: response_path.clone(),
                inflight_path: inflight_path.clone(),
            },
        );

        let result = UiAutomationResponse {
            ok: true,
            request_id,
            window: "main".to_string(),
            action: UiAutomationAction::Query,
            selector: "different".to_string(),
            target: Some(json!({ "testId": "different" })),
            error: None,
            message: None,
            available: None,
            diagnostics: None,
            available_windows: None,
            timeout_ms: None,
            phase: None,
        };

        state.complete("main", result).unwrap();

        let raw = fs::read_to_string(response_path).unwrap();
        let written: UiAutomationResponse = serde_json::from_str(&raw).unwrap();
        assert!(!written.ok);
        assert_eq!(written.error.as_deref(), Some("completion_mismatch"));
        assert!(!inflight_path.exists());
    }

    #[test]
    fn session_read_retries_transient_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(SESSION_FILE);
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(SESSION_READ_RETRY_DELAY_MS));
            fs::write(writer_path, "{\"ok\":true}").unwrap();
        });

        let raw = read_session_file_with_retry(&path).unwrap();
        writer.join().unwrap();
        assert_eq!(raw, "{\"ok\":true}");
    }

    #[test]
    fn initialization_failure_can_disable_state() {
        let tmp = tempfile::tempdir().unwrap();
        let state = UiAutomationState::new(true, tmp.path().to_path_buf());
        fs::create_dir_all(&state.inner.automation_dir).unwrap();
        fs::write(state.requests_dir(), "not a directory").unwrap();

        assert!(state.initialize_files().is_err());
        state.mark_unavailable();
        assert!(!state.enabled());
    }

    #[test]
    fn cleanup_stale_automation_files_removes_request_and_response_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let automation_dir = tmp.path().join(UI_AUTOMATION_DIR);
        let requests_dir = automation_dir.join(REQUESTS_DIR);
        let responses_dir = automation_dir.join(RESPONSES_DIR);
        fs::create_dir_all(&requests_dir).unwrap();
        fs::create_dir_all(&responses_dir).unwrap();
        fs::write(requests_dir.join("stale.json"), "{}").unwrap();
        fs::write(responses_dir.join("stale.json"), "{}").unwrap();

        cleanup_stale_automation_files(&automation_dir).unwrap();

        assert!(!requests_dir.exists());
        assert!(!responses_dir.exists());
    }

    #[test]
    fn timeout_phase_uses_ready_window_labels_for_inflight() {
        let tmp = tempfile::tempdir().unwrap();
        let request_path = tmp.path().join("request.json");
        let inflight_path = tmp.path().join("request.inflight.json");
        let session_path = tmp.path().join(SESSION_FILE);
        fs::write(&request_path, "{}").unwrap();
        assert_eq!(
            timeout_phase(&request_path, &inflight_path, &session_path, "main"),
            "awaiting_gui_poller"
        );

        fs::remove_file(&request_path).unwrap();
        fs::write(&inflight_path, "{}").unwrap();
        fs::write(
            &session_path,
            serde_json::to_string(&UiAutomationSession {
                pid: std::process::id(),
                token: "token".to_string(),
                exe_path: current_exe_path_string(),
                config_dir: tmp.path().to_string_lossy().into_owned(),
                window_labels: vec!["main".to_string()],
                ready_window_labels: vec![],
                started_at_unix_ms: 1,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            timeout_phase(&request_path, &inflight_path, &session_path, "main"),
            "awaiting_frontend_ready"
        );

        fs::write(
            &session_path,
            serde_json::to_string(&UiAutomationSession {
                pid: std::process::id(),
                token: "token".to_string(),
                exe_path: current_exe_path_string(),
                config_dir: tmp.path().to_string_lossy().into_owned(),
                window_labels: vec!["main".to_string()],
                ready_window_labels: vec!["main".to_string()],
                started_at_unix_ms: 1,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            timeout_phase(&request_path, &inflight_path, &session_path, "main"),
            "awaiting_frontend_response"
        );
    }
}
