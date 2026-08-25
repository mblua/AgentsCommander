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

pub const UI_AUTOMATION_DIR: &str = crate::config::instance_artifacts::UI_AUTOMATION_DIR_NAME;
pub const SESSION_FILE: &str = "session.json";
pub const REQUESTS_DIR: &str = "requests";
pub const RESPONSES_DIR: &str = "responses";
pub const ENV_ENABLE: &str = "AC_UI_AUTOMATION";
pub const BACKEND_AUTOMATION_WINDOW: &str = "__backend";
pub const RESOURCE_WATCHDOG_BACKEND_SELECTOR: &str = "resourceMonitor.watchdog";

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const POLL_MS: u64 = 50;
const FS_RETRY_COUNT: usize = 8;
const FS_RETRY_DELAY_MS: u64 = 25;
const SESSION_READ_RETRY_COUNT: usize = 8;
const SESSION_READ_RETRY_DELAY_MS: u64 = 25;
const CLI_MAX_AVAILABLE_TARGETS: usize = 8;
const CLI_MAX_TARGET_TEXT_CHARS: usize = 80;

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
pub struct UiTerminalArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub session_id: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
    pub operation: String,
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
pub struct UiContextClickArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    #[arg(long)]
    pub selector: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

#[derive(Debug, Args)]
pub struct UiHoverArgs {
    #[arg(long, default_value = "main")]
    pub window: String,
    /// Required unless --leave.
    #[arg(long, required_unless_present = "leave")]
    pub selector: Option<String>,
    /// Park the pointer nowhere: fire the leave chain on whatever is currently hovered
    /// (up to <html>, relatedTarget null) and clear the sticky pointer. Target-free on
    /// purpose: the thing you want to release is normally already gone (the menu was torn
    /// down) or re-minted by <For>, and a cleanup step that fails when its subject is
    /// missing is not a cleanup step. Never fails.
    #[arg(long, conflicts_with = "selector")]
    pub leave: bool,
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
pub struct UiTypeArgs {
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
pub struct UiBackendArgs {
    #[arg(long)]
    pub selector: String,
    #[arg(long)]
    pub value: Option<String>,
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
    ContextClick,
    Hover,
    SetValue,
    TypeText,
    Backend,
    Terminal,
}

impl UiAutomationAction {
    /// #944 - exhaustive on purpose. A new variant is a COMPILE error here, and the
    /// `None` terminator bounds the walk, so a variant cannot be silently left out of
    /// the iteration the way a hand-written `[UiAutomationAction; N]` array lets it be
    /// (name the variant in the forced match arm, forget the array, and the parity test
    /// still passes).
    #[cfg(test)]
    fn next_variant(self) -> Option<Self> {
        Some(match self {
            Self::Query => Self::Click,
            Self::Click => Self::ContextClick,
            Self::ContextClick => Self::Hover,
            Self::Hover => Self::SetValue,
            Self::SetValue => Self::TypeText,
            Self::TypeText => Self::Backend,
            Self::Backend => Self::Terminal,
            Self::Terminal => return None,
        })
    }

    /// Residual hole, stated so nobody trusts this further than it goes: the walk is
    /// SEEDED by hand at `Self::Query`. A variant inserted at the HEAD (before `Query`,
    /// wired `New => Query`) is compile-forced into `next_variant` and into
    /// `action_wire_name`, and yet is never yielded here, so the parity test below would
    /// not see it. It only bites a Rust-ONLY head-insertion: add the same member to the
    /// types.ts union and the set comparison goes red at once. Appending a variant (the
    /// normal case, and what #944 itself did) is fully covered. Closing the hole outright
    /// needs a derive macro (strum) or a hand-written length, and a hand-written length is
    /// the exact weakness this walk replaced.
    #[cfg(test)]
    fn all() -> impl Iterator<Item = Self> {
        std::iter::successors(Some(Self::Query), |action| action.next_variant())
    }
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
            self.cleanup_session_file_unchecked();
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
        if self.inner.enabled {
            self.cleanup_session_file_unchecked();
        }
    }

    fn cleanup_session_file_unchecked(&self) {
        if let Err(e) = retry_remove_file(&self.inner.session_path) {
            log::warn!(
                "[ui-automation] failed to remove session file {}: {}",
                self.inner.session_path.display(),
                e
            );
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
            let mut labels = self
                .inner
                .window_labels
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            labels.insert(caller_label.to_string());
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
        self.sync_live_window_labels(available_window_labels(app))?;
        self.expire_pending_requests();

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

    fn sync_live_window_labels(&self, live_labels: Vec<String>) -> io::Result<()> {
        let live_labels = live_labels.into_iter().collect::<HashSet<_>>();
        let mut changed = false;

        {
            let mut labels = self
                .inner
                .window_labels
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if *labels != live_labels {
                *labels = live_labels.clone();
                changed = true;
            }
        }

        {
            let mut ready = self
                .inner
                .ready_window_labels
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let before_len = ready.len();
            ready.retain(|label| live_labels.contains(label));
            if ready.len() != before_len {
                changed = true;
            }
        }

        if changed {
            self.write_session_snapshot()?;
        }
        Ok(())
    }

    fn expire_pending_requests(&self) {
        let now = now_unix_ms();
        let expired = {
            let mut pending_map = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            let expired_ids: Vec<String> = pending_map
                .iter()
                .filter(|(_, pending)| request_expired(&pending.request, now))
                .map(|(request_id, _)| request_id.clone())
                .collect();
            expired_ids
                .into_iter()
                .filter_map(|request_id| pending_map.remove(&request_id))
                .collect::<Vec<_>>()
        };

        for pending in expired {
            let response = expired_response_for_request(&pending.request);
            if !pending.response_path.exists() {
                let _ = write_json_atomic_new(&pending.response_path, &response);
            }
            let _ = retry_remove_file(&pending.inflight_path);
        }
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

        if is_backend_automation_window(&request.window) {
            self.process_backend_request_file(app, &request_file, &request);
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

    fn process_backend_request_file(
        &self,
        app: &AppHandle,
        request_file: &RequestFile,
        request: &UiAutomationRequest,
    ) {
        let inflight_path = match self.ensure_inflight(request_file) {
            Ok(path) => path,
            Err(error) => {
                let mut response = UiAutomationResponse::error_for_request(
                    request,
                    "automation_filesystem_error",
                    "Failed to mark backend automation request as inflight.",
                );
                response.diagnostics = Some(json!({ "message": error.to_string() }));
                let _ = self.write_direct_response(request_file, &response);
                return;
            }
        };
        let response_path = self.response_path(&request.request_id);
        let pending = PendingRequest {
            request: request.clone(),
            response_path,
            inflight_path,
        };
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(request.request_id.clone(), pending);

        let state = self.clone();
        let app = app.clone();
        let request = request.clone();
        tauri::async_runtime::spawn(async move {
            let response = handle_backend_request(&app, &request).await;
            let pending = state
                .inner
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&request.request_id);
            let Some(pending) = pending else {
                return;
            };
            if let Err(error) = write_json_atomic_new(&pending.response_path, &response) {
                log::warn!(
                    "[ui-automation] backend response write failed request={}: {}",
                    request.request_id,
                    error
                );
            }
            let _ = retry_remove_file(&pending.inflight_path);
        });
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiTerminalOperation {
    Query,
    Top,
    Bottom,
    Line(i32),
    Lines(i32),
    Pages(i32),
}

fn parse_terminal_operation(value: &str) -> Option<UiTerminalOperation> {
    match value {
        "query" => return Some(UiTerminalOperation::Query),
        "top" => return Some(UiTerminalOperation::Top),
        "bottom" => return Some(UiTerminalOperation::Bottom),
        _ => {}
    }

    let parse_unsigned = |operand: &str| {
        let bytes = operand.as_bytes();
        let canonical = bytes == b"0"
            || (bytes
                .first()
                .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
                && bytes[1..].iter().all(u8::is_ascii_digit));
        canonical.then(|| operand.parse::<i32>().ok()).flatten()
    };
    let parse_signed = |operand: &str| {
        let bytes = operand.as_bytes();
        let canonical_unsigned = bytes == b"0"
            || (bytes
                .first()
                .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
                && bytes[1..].iter().all(u8::is_ascii_digit));
        let canonical_negative = bytes.first() == Some(&b'-')
            && bytes.get(1).is_some_and(|byte| matches!(byte, b'1'..=b'9'))
            && bytes[2..].iter().all(u8::is_ascii_digit);
        (canonical_unsigned || canonical_negative)
            .then(|| operand.parse::<i32>().ok())
            .flatten()
    };

    if let Some(operand) = value.strip_prefix("line:") {
        return parse_unsigned(operand).map(UiTerminalOperation::Line);
    }
    if let Some(operand) = value.strip_prefix("lines:") {
        return parse_signed(operand).map(UiTerminalOperation::Lines);
    }
    if let Some(operand) = value.strip_prefix("pages:") {
        return parse_signed(operand).map(UiTerminalOperation::Pages);
    }
    None
}

fn valid_terminal_session_id(session_id: &str) -> bool {
    !session_id.is_empty() && session_id.len() <= 256 && !session_id.chars().any(char::is_control)
}

fn terminal_cli_request(args: UiTerminalArgs) -> Result<CliRequest, Value> {
    if !valid_terminal_session_id(&args.session_id) {
        return Err(preflight_error(
            "invalid_terminal_session_id",
            "Terminal session id must contain 1 to 256 UTF-8 bytes and no control characters.",
            None,
        ));
    }
    if parse_terminal_operation(&args.operation).is_none() {
        return Err(preflight_error(
            "invalid_terminal_operation",
            "Terminal operation must be query, top, bottom, line:N, lines:N, or pages:N in the signed 32-bit range.",
            None,
        ));
    }

    Ok(CliRequest {
        window: args.window,
        selector: format!("terminal.session.{}", args.session_id),
        action: UiAutomationAction::Terminal,
        value: Some(args.operation),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_terminal(args: UiTerminalArgs) -> i32 {
    match terminal_cli_request(args) {
        Ok(request) => execute_cli(request),
        Err(error) => {
            print_stdout_json(&error);
            1
        }
    }
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

pub fn execute_context_click(args: UiContextClickArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::ContextClick,
        value: None,
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_hover(args: UiHoverArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        // Empty for the target-free leave form. The bridge intercepts `value == "leave"`
        // before it resolves any selector, and the frontend echoes the request's selector
        // back, so the window/action/selector equality check in `complete()` still matches.
        // No line number on purpose: that pointer read :317-321, then :361-364, and rotted
        // both times inside the very commit that wrote it (this file grew above it each
        // time). `fn complete` is the stable anchor; a number here is a comment that lies
        // on a schedule.
        selector: args.selector.unwrap_or_default(),
        action: UiAutomationAction::Hover,
        value: args.leave.then(|| "leave".to_string()),
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

pub fn execute_type(args: UiTypeArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::TypeText,
        value: Some(args.value),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_backend(args: UiBackendArgs) -> i32 {
    execute_cli(CliRequest {
        window: BACKEND_AUTOMATION_WINDOW.to_string(),
        selector: args.selector,
        action: UiAutomationAction::Backend,
        value: args.value,
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
            print_stdout_json(&timeout);
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
                print_stdout_json(&error);
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
            print_stdout_json(&response);
            1
        }
        Err(error) => {
            print_stdout_json(&error);
            1
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackendWatchdogMode {
    Sample,
    Warn,
    KillGroup,
    Tick,
    /// #1151 - runs the production quarantine-retry gate and loop, so native verification
    /// of the orphan path is a deterministic, assertable artifact rather than log scraping
    /// behind a 15 second backoff.
    QuarantineRetry,
}

impl BackendWatchdogMode {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("sample") {
            "sample" => Ok(Self::Sample),
            "warn" => Ok(Self::Warn),
            "killGroup" | "kill-group" => Ok(Self::KillGroup),
            "tick" => Ok(Self::Tick),
            "quarantineRetry" | "quarantine-retry" => Ok(Self::QuarantineRetry),
            other => Err(format!(
                "Unsupported resource monitor watchdog mode '{other}'. Expected sample, warn, killGroup, tick, or quarantineRetry."
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Sample => "sample",
            Self::Warn => "warn",
            Self::KillGroup => "killGroup",
            Self::Tick => "tick",
            Self::QuarantineRetry => "quarantineRetry",
        }
    }
}

async fn handle_backend_request(
    app: &AppHandle,
    request: &UiAutomationRequest,
) -> UiAutomationResponse {
    if request.action != UiAutomationAction::Backend {
        return UiAutomationResponse::error_for_request(
            request,
            "unsupported_action",
            "Backend automation window only supports backend requests.",
        );
    }

    match request.selector.as_str() {
        RESOURCE_WATCHDOG_BACKEND_SELECTOR => {
            handle_resource_watchdog_backend_request(app, request).await
        }
        _ => UiAutomationResponse::error_for_request(
            request,
            "unsupported_selector",
            "Backend automation selector is not supported.",
        ),
    }
}

async fn handle_resource_watchdog_backend_request(
    app: &AppHandle,
    request: &UiAutomationRequest,
) -> UiAutomationResponse {
    let cfg = crate::config::settings::load_settings();
    handle_resource_watchdog_backend_request_with_config(app, request, &cfg).await
}

async fn handle_resource_watchdog_backend_request_with_config<R: tauri::Runtime>(
    app: &AppHandle<R>,
    request: &UiAutomationRequest,
    cfg: &crate::config::settings::AppSettings,
) -> UiAutomationResponse {
    let mode = match BackendWatchdogMode::parse(request.value.as_deref()) {
        Ok(mode) => mode,
        Err(message) => {
            return UiAutomationResponse::error_for_request(request, "invalid_value", &message);
        }
    };

    let Some(monitor) = app.try_state::<Arc<crate::resource_monitor::ResourceMonitorState>>()
    else {
        return UiAutomationResponse::error_for_request(
            request,
            "backend_state_missing",
            "Resource monitor state is not registered.",
        );
    };

    let limits = crate::resource_monitor::ResourceLimits::from(cfg);
    let resource_monitor_enabled = monitor.is_effectively_enabled(cfg.resource_monitor_enabled);
    let snapshot = monitor.snapshot(limits);
    let groups = snapshot
        .groups
        .iter()
        .filter_map(|group| {
            Uuid::parse_str(&group.session_id)
                .ok()
                .map(|session_id| (session_id, group.clone()))
        })
        .collect::<Vec<_>>();
    let decisions = crate::resource_monitor::watchdog::evaluate_watchdog_groups(&groups, limits);
    let warn_matches = decisions
        .iter()
        .filter(|decision| decision.warn_required)
        .count();
    let kill_matches = decisions
        .iter()
        .filter(|decision| decision.kill_required)
        .count();

    let kill_enabled = resource_monitor_enabled
        && (matches!(mode, BackendWatchdogMode::KillGroup)
            || (matches!(mode, BackendWatchdogMode::Tick)
                && cfg.resource_watchdog_action
                    == crate::config::settings::ResourceWatchdogAction::KillGroup));

    let mut kill_results = Vec::new();
    if kill_enabled {
        for decision in decisions.iter().filter(|decision| decision.kill_required) {
            let Some(coordinator) =
                app.try_state::<crate::session::selection::SelectionCoordinator>()
            else {
                kill_results.push(json!({
                    "ok": false,
                    "sessionId": decision.session_id,
                    "message": "selectionCoordinatorUnavailable",
                }));
                continue;
            };
            match coordinator
                .watchdog_resource_kill(decision.session_id)
                .await
            {
                Ok(crate::session::selection::WatchdogKillOutcome::Completed(result)) => {
                    kill_results.push(json!({
                        "ok": true,
                        "result": result,
                    }))
                }
                // #1151 - `alreadyPending` keeps its value and type byte-identical so every
                // existing automation assertion still passes; `reason` is the additive
                // discriminator that makes the split observable from outside the process.
                Ok(crate::session::selection::WatchdogKillOutcome::AlreadyInFlight) => kill_results
                    .push(json!({
                        "ok": true,
                        "sessionId": decision.session_id,
                        "alreadyPending": true,
                        "reason": "alreadyInFlight",
                    })),
                Ok(crate::session::selection::WatchdogKillOutcome::NoPublicSession) => kill_results
                    .push(json!({
                        "ok": true,
                        "sessionId": decision.session_id,
                        "alreadyPending": true,
                        "reason": "noPublicSession",
                    })),
                Err(message) => kill_results.push(json!({
                    "ok": false,
                    "sessionId": decision.session_id,
                    "message": message,
                })),
            }
        }
    }

    // #1151 - run the PRODUCTION quarantine-retry gate and loop, not a mirrored copy, so
    // these reports are evidence about shipped dispatch logic. The coordinator is resolved
    // ONCE before the call, because the loop takes it by reference for every group.
    let mut quarantine_retries = Vec::new();
    if resource_monitor_enabled
        && matches!(
            mode,
            BackendWatchdogMode::QuarantineRetry | BackendWatchdogMode::Tick
        )
    {
        match app.try_state::<crate::session::selection::SelectionCoordinator>() {
            Some(coordinator) => {
                for report in crate::resource_monitor::watchdog::run_quarantine_retries(
                    &monitor,
                    coordinator.inner(),
                    &groups,
                )
                .await
                {
                    quarantine_retries.push(serde_json::to_value(&report).unwrap_or_else(
                        |error| json!({ "ok": false, "message": error.to_string() }),
                    ));
                }
            }
            None => quarantine_retries.push(json!({
                "ok": false,
                "message": "selectionCoordinatorUnavailable",
            })),
        }
    }

    let state = if !resource_monitor_enabled {
        "disabled"
    } else if !kill_results.is_empty() {
        "enforcing"
    } else if kill_matches > 0 {
        "critical"
    } else if warn_matches > 0 {
        "warn"
    } else {
        "ok"
    };

    let snapshot_diagnostics = if monitor.supports_process_tree_enforcement() {
        json!({
            "capturedAt": snapshot.captured_at,
            "overallState": snapshot.overall_state,
            "activeAgentGroups": snapshot.active_agent_groups,
            "appPrivateBytes": snapshot.app_private_bytes,
            "networkState": snapshot.network_state,
            "networkSummary": snapshot.network_summary,
            "warnings": snapshot.warnings,
        })
    } else {
        json!({
            "overallState": snapshot.overall_state,
            "activeAgentGroups": snapshot.active_agent_groups,
            "appPrivateBytes": snapshot.app_private_bytes,
            "networkState": snapshot.network_state,
            "networkSummary": snapshot.network_summary,
            "warnings": snapshot.warnings,
        })
    };

    UiAutomationResponse {
        ok: true,
        request_id: request.request_id.clone(),
        window: request.window.clone(),
        action: request.action,
        selector: request.selector.clone(),
        target: Some(json!({
            "testId": request.selector,
            "role": "backend",
            "state": state,
            "tag": "backend",
            "text": format!(
                "resource monitor watchdog {}: {} group(s), {} warn match(es), {} kill match(es)",
                mode.as_str(),
                groups.len(),
                warn_matches,
                kill_matches
            ),
            "visible": true,
            "disabled": false,
        })),
        error: None,
        message: None,
        available: None,
        diagnostics: Some(json!({
            "mode": mode.as_str(),
            "configuredAction": cfg.resource_watchdog_action,
            "resourceMonitorEnabled": resource_monitor_enabled,
            "killApplied": kill_enabled,
            "limits": {
                "maxConcurrentAgentGroups": limits.max_concurrent_agent_processes,
                "groupWarnPrivateBytes": limits.group_warn_private_bytes,
                "groupKillPrivateBytes": limits.group_kill_private_bytes,
                "processKillPrivateBytes": limits.process_kill_private_bytes,
            },
            // #1151 - `snapshot` is captured BEFORE the action loop and is deliberately
            // left that way. Every post-retry assertion reads `quarantineRetries[i]`
            // instead; reading `snapshot.activeAgentGroups` after a successful reclaim
            // still shows the pre-action figure and produces a false FAIL.
            "snapshot": snapshot_diagnostics,
            "decisions": decisions,
            "killResults": kill_results,
            "quarantineRetries": quarantine_retries,
        })),
        available_windows: None,
        timeout_ms: None,
        phase: None,
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

fn automation_not_enabled_error() -> Value {
    preflight_error(
        "automation_not_enabled",
        "A testable GUI is already running without UI automation enabled. Restart it with --ui-automation or AC_UI_AUTOMATION=1.",
        None,
    )
}

fn load_session_for_cli(path: &Path, requested_window: &str) -> Result<UiAutomationSession, Value> {
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

fn sanitize_response_for_cli(mut response: UiAutomationResponse) -> UiAutomationResponse {
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

fn sanitize_target_for_cli(target: &mut Value) {
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

fn add_cli_truncation_diagnostics(
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

fn print_stdout_json<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(json) => crate::cli_println!("{json}"),
        Err(e) => crate::cli_println!(
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

fn session_ready_for_window(path: &Path, window: &str) -> bool {
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

fn is_backend_automation_window(window: &str) -> bool {
    window == BACKEND_AUTOMATION_WINDOW
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
        } else {
            (name.strip_suffix(".json")?, RequestFileKind::Ready)
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
    use crate::config::settings::{AppSettings, ResourceWatchdogAction};
    use crate::resource_monitor::registry::{
        ProcessTreeBackend, ResourceError, ResourceLaunchRegistration,
    };
    use crate::resource_monitor::types::{
        ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory,
        ResourceLaunchMetadata, TerminateOutcome,
    };
    use crate::session::manager::SessionManager;
    use crate::session::selection::{
        CriticalAdmissionKind, SelectionCoordinator, WatchdogKillOutcome,
    };
    use crate::web::broadcast::WsBroadcaster;
    use crate::DetachedSessionsState;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    struct UnsupportedAutomationBackend {
        observe_tree_calls: AtomicUsize,
        observe_identity_calls: AtomicUsize,
        terminate_verified_calls: AtomicUsize,
        current_process_memory_calls: AtomicUsize,
    }

    impl ProcessTreeBackend for UnsupportedAutomationBackend {
        fn supports_process_tree_enforcement(&self) -> bool {
            false
        }

        fn observe_tree(
            &self,
            _root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
            self.observe_tree_calls.fetch_add(1, Ordering::SeqCst);
            Ok(ObservedProcessTree::default())
        }

        fn observe_identity(&self, _pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            self.observe_identity_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }

        fn terminate_verified(
            &self,
            _process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            self.terminate_verified_calls.fetch_add(1, Ordering::SeqCst);
            Ok(TerminateOutcome::AlreadyGone)
        }

        fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
            self.current_process_memory_calls
                .fetch_add(1, Ordering::SeqCst);
            Ok(ProcessMemory::default())
        }
    }

    /// #1151 - supported process-tree backend for the automation tests. `observe_tree`
    /// synthesises a root plus one child from the requested root identity, so every
    /// registered group gets distinct identities and a private-bytes total above the
    /// kill thresholds these tests configure. A pid marked stubborn fails
    /// `terminate_verified`, which is what parks a group in `Quarantined`; marking it
    /// gone afterwards is what lets a later retry verify the cleanup.
    #[derive(Default)]
    struct AutomationProcessBackend {
        stubborn: Mutex<HashSet<u32>>,
        gone: Mutex<HashSet<u32>>,
        observe_tree_calls: AtomicUsize,
        observe_identity_calls: AtomicUsize,
        terminate_verified_calls: AtomicUsize,
    }

    impl AutomationProcessBackend {
        fn identity(pid: u32) -> ProcessIdentity {
            ProcessIdentity {
                pid,
                creation_time_100ns: u64::from(pid),
            }
        }

        fn child_pid(root_pid: u32) -> u32 {
            root_pid + 1
        }

        fn observed(pid: u32, depth: u32, parent_pid: Option<u32>) -> ObservedProcess {
            ObservedProcess {
                identity: Self::identity(pid),
                parent_pid,
                parent_identity: parent_pid.map(Self::identity),
                exe_name: format!("p{pid}"),
                depth,
                private_bytes: Some(5_000),
                working_set_bytes: Some(5_000),
                cpu_percent: None,
                kill_allowed: true,
            }
        }

        fn mark_stubborn(&self, pid: u32) {
            self.stubborn.lock().unwrap().insert(pid);
        }

        fn mark_gone(&self, pid: u32) {
            self.stubborn.lock().unwrap().remove(&pid);
            self.gone.lock().unwrap().insert(pid);
        }
    }

    impl ProcessTreeBackend for AutomationProcessBackend {
        fn observe_tree(
            &self,
            root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
            self.observe_tree_calls.fetch_add(1, Ordering::SeqCst);
            let gone = self.gone.lock().unwrap();
            let child_pid = Self::child_pid(root.pid);
            let mut processes = Vec::new();
            if !gone.contains(&root.pid) {
                processes.push(Self::observed(root.pid, 0, None));
            }
            if !gone.contains(&child_pid) {
                processes.push(Self::observed(child_pid, 1, Some(root.pid)));
            }
            let errors = if gone.contains(&root.pid) {
                vec![format!("root pid {} was not in process snapshot", root.pid)]
            } else {
                Vec::new()
            };
            Ok(ObservedProcessTree { processes, errors })
        }

        fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
            self.observe_identity_calls.fetch_add(1, Ordering::SeqCst);
            if self.gone.lock().unwrap().contains(&pid) {
                Ok(None)
            } else {
                Ok(Some(Self::identity(pid)))
            }
        }

        fn terminate_verified(
            &self,
            process: &ObservedProcess,
        ) -> Result<TerminateOutcome, ResourceError> {
            self.terminate_verified_calls.fetch_add(1, Ordering::SeqCst);
            let pid = process.identity.pid;
            if self.gone.lock().unwrap().contains(&pid) {
                return Ok(TerminateOutcome::AlreadyGone);
            }
            if self.stubborn.lock().unwrap().contains(&pid) {
                return Err(ResourceError::Message(format!(
                    "pid {pid}: process still alive after terminate"
                )));
            }
            self.gone.lock().unwrap().insert(pid);
            Ok(TerminateOutcome::Terminated)
        }

        fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
            Ok(ProcessMemory::default())
        }
    }

    fn automation_settings() -> AppSettings {
        AppSettings {
            resource_monitor_enabled: true,
            resource_watchdog_action: ResourceWatchdogAction::KillGroup,
            max_concurrent_agent_processes: 4,
            agent_group_warn_private_bytes: 100,
            agent_group_kill_private_bytes: 200,
            agent_process_kill_private_bytes: 300,
            ..AppSettings::default()
        }
    }

    fn register_automation_group(
        monitor: &crate::resource_monitor::ResourceMonitorState,
        limits: crate::resource_monitor::ResourceLimits,
        session_id: Uuid,
        root_pid: u32,
    ) -> ProcessIdentity {
        let permit = monitor.try_reserve_agent_slot(limits).unwrap().unwrap();
        let mut registration = ResourceLaunchRegistration::new(
            monitor.clone(),
            permit,
            ResourceLaunchMetadata {
                session_id,
                name: "agent".to_string(),
                agent_id: None,
                agent_label: None,
                workgroup: None,
                agent: None,
                project: None,
            },
        );
        registration
            .register_root_pid(root_pid)
            .expect("register automation root pid")
            .expect("supported backend observes the root identity")
    }

    fn watchdog_backend_request(mode: &str) -> UiAutomationRequest {
        UiAutomationRequest {
            request_id: Uuid::new_v4().to_string(),
            token: "token".to_string(),
            window: BACKEND_AUTOMATION_WINDOW.to_string(),
            action: UiAutomationAction::Backend,
            selector: RESOURCE_WATCHDOG_BACKEND_SELECTOR.to_string(),
            value: Some(mode.to_string()),
            expires_at_unix_ms: Some(now_unix_ms() + 1_000),
        }
    }

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

    fn action_wire_name(action: UiAutomationAction) -> &'static str {
        match action {
            UiAutomationAction::Query => "query",
            UiAutomationAction::Click => "click",
            UiAutomationAction::ContextClick => "contextClick",
            UiAutomationAction::Hover => "hover",
            UiAutomationAction::SetValue => "setValue",
            UiAutomationAction::TypeText => "typeText",
            UiAutomationAction::Backend => "backend",
            UiAutomationAction::Terminal => "terminal",
        }
    }

    #[tokio::test]
    async fn unsupported_backend_reports_disabled_and_never_kills() {
        let backend = Arc::new(UnsupportedAutomationBackend::default());
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>,
        ));
        let app = tauri::test::mock_builder()
            .manage(monitor)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build unsupported resource-watchdog automation app");
        let cfg = AppSettings {
            resource_monitor_enabled: true,
            resource_watchdog_action: ResourceWatchdogAction::KillGroup,
            max_concurrent_agent_processes: 7,
            agent_group_warn_private_bytes: 101,
            agent_group_kill_private_bytes: 202,
            agent_process_kill_private_bytes: 303,
            ..AppSettings::default()
        };

        for mode in ["sample", "warn", "killGroup", "tick", "quarantineRetry"] {
            let request = UiAutomationRequest {
                request_id: Uuid::new_v4().to_string(),
                token: "token".to_string(),
                window: BACKEND_AUTOMATION_WINDOW.to_string(),
                action: UiAutomationAction::Backend,
                selector: RESOURCE_WATCHDOG_BACKEND_SELECTOR.to_string(),
                value: Some(mode.to_string()),
                expires_at_unix_ms: Some(now_unix_ms() + 1_000),
            };

            let response =
                handle_resource_watchdog_backend_request_with_config(app.handle(), &request, &cfg)
                    .await;

            assert!(response.ok);
            assert_eq!(
                response.target,
                Some(json!({
                    "testId": RESOURCE_WATCHDOG_BACKEND_SELECTOR,
                    "role": "backend",
                    "state": "disabled",
                    "tag": "backend",
                    "text": format!(
                        "resource monitor watchdog {mode}: 0 group(s), 0 warn match(es), 0 kill match(es)"
                    ),
                    "visible": true,
                    "disabled": false,
                }))
            );
            assert_eq!(
                response.diagnostics,
                Some(json!({
                    "mode": mode,
                    "configuredAction": cfg.resource_watchdog_action,
                    "resourceMonitorEnabled": false,
                    "killApplied": false,
                    "limits": {
                        "maxConcurrentAgentGroups": 7,
                        "groupWarnPrivateBytes": 101,
                        "groupKillPrivateBytes": 202,
                        "processKillPrivateBytes": 303,
                    },
                    "snapshot": {
                        "overallState": "unknown",
                        "activeAgentGroups": 0,
                        "appPrivateBytes": null,
                        "networkState": "unknown",
                        "networkSummary": "Socket attribution unavailable",
                        "warnings": [],
                    },
                    "decisions": [],
                    "killResults": [],
                    "quarantineRetries": [],
                }))
            );
        }

        assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.terminate_verified_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            backend.current_process_memory_calls.load(Ordering::SeqCst),
            0
        );
    }

    // #1151 (A3) - the automation `killResults` contract for the two split outcomes.
    // `alreadyPending` keeps its value and type byte-identical so no existing automation
    // breaks; `reason` is the additive discriminator that makes the split observable.
    #[tokio::test]
    async fn watchdog_kill_results_preserve_already_pending_contract() {
        use crate::pty::backend::SessionBackendKind;

        let backend = Arc::new(AutomationProcessBackend::default());
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>,
        ));
        let cfg = automation_settings();
        let limits = crate::resource_monitor::ResourceLimits::from(&cfg);

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let live = manager
            .read()
            .await
            .create_session(
                "shell".to_string(),
                Vec::new(),
                "C:/automation-kill-results".to_string(),
                None,
                None,
                Vec::new(),
                false,
                SessionBackendKind::LocalProcess,
            )
            .await
            .unwrap();

        // One group whose public row exists (the in-flight case) and one whose row never
        // did (the orphan case). Both are Running with private bytes over the kill limit,
        // so the hook produces a kill decision for each.
        let orphan_id = Uuid::new_v4();
        register_automation_group(&monitor, limits, live.id, 5_100);
        register_automation_group(&monitor, limits, orphan_id, 5_200);

        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(Arc::clone(&monitor))
            .manage(coordinator.clone())
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build kill-results automation app");
        coordinator.start(app.handle().clone()).unwrap();
        let restore = coordinator.submit_restore_first().await.unwrap();

        // Fill the queue so the first watchdog kill parks holding ONLY its critical key,
        // without ever enqueueing a job.
        let mut reservations = Vec::new();
        while let Ok(reservation) = coordinator.reserve_auto_close() {
            reservations.push(reservation);
        }
        let waiter = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.watchdog_resource_kill(live.id).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while !coordinator
                .critical_key_registered_for_test(live.id, CriticalAdmissionKind::WatchdogKill)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the parked watchdog kill registers its critical key");

        assert!(matches!(
            coordinator.watchdog_resource_kill(live.id).await.unwrap(),
            WatchdogKillOutcome::AlreadyInFlight
        ));
        assert!(matches!(
            coordinator.watchdog_resource_kill(orphan_id).await.unwrap(),
            WatchdogKillOutcome::NoPublicSession
        ));

        let response = handle_resource_watchdog_backend_request_with_config(
            app.handle(),
            &watchdog_backend_request("killGroup"),
            &cfg,
        )
        .await;
        assert!(response.ok);
        let diagnostics = response.diagnostics.expect("watchdog diagnostics");
        let kill_results = diagnostics["killResults"]
            .as_array()
            .expect("killResults array")
            .clone();
        assert_eq!(kill_results.len(), 2);
        let entry_for = |session_id: Uuid| {
            kill_results
                .iter()
                .find(|entry| entry["sessionId"] == json!(session_id))
                .cloned()
                .unwrap_or_else(|| panic!("kill result for session {session_id}"))
        };
        assert_eq!(
            entry_for(live.id),
            json!({
                "ok": true,
                "sessionId": live.id,
                "alreadyPending": true,
                "reason": "alreadyInFlight",
            })
        );
        assert_eq!(
            entry_for(orphan_id),
            json!({
                "ok": true,
                "sessionId": orphan_id,
                "alreadyPending": true,
                "reason": "noPublicSession",
            })
        );

        restore.finish();
        coordinator.close_and_join().await;
        drop(reservations);
        let _ = waiter.await;
    }

    /// #1151 - registers a group, quarantines its cleanup on a stubborn child, and leaves
    /// the retry backoff already satisfied. No public session row is ever created, which
    /// is precisely what makes the group an orphan.
    fn orphan_automation_group(
        monitor: &crate::resource_monitor::ResourceMonitorState,
        backend: &AutomationProcessBackend,
        limits: crate::resource_monitor::ResourceLimits,
        session_id: Uuid,
        root_pid: u32,
    ) -> ProcessIdentity {
        backend.mark_stubborn(AutomationProcessBackend::child_pid(root_pid));
        let root = register_automation_group(monitor, limits, session_id, root_pid);
        let quarantine = monitor
            .kill_group(
                session_id,
                crate::resource_monitor::types::ResourceKillReason::SessionDestroy,
            )
            .expect("first cleanup runs");
        assert!(quarantine.quarantined);
        monitor.test_backdate_quarantine_retry(session_id);
        root
    }

    async fn automation_app_with_coordinator(
        monitor: Arc<crate::resource_monitor::ResourceMonitorState>,
    ) -> (tauri::App<tauri::test::MockRuntime>, SelectionCoordinator) {
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
        let app = tauri::test::mock_builder()
            .manage(Arc::clone(&manager))
            .manage(monitor)
            .manage(coordinator.clone())
            .manage(DetachedSessionsState::default())
            .manage(WsBroadcaster::new())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build quarantine-retry automation app");
        coordinator
            .start(app.handle().clone())
            .expect("start quarantine-retry coordinator");
        coordinator
            .submit_restore_first()
            .await
            .expect("restore-first admitted")
            .finish();
        (app, coordinator)
    }

    // #1151 (A1) - the deterministic native artifact for a successful reclaim. Everything
    // is read from quarantineRetries[0], NEVER from diagnostics.snapshot, which is captured
    // before the action loop and would still read activeAgentGroups = 1 on a correct build.
    #[tokio::test]
    async fn quarantine_retry_mode_reports_completed_orphan_cleanup() {
        let backend = Arc::new(AutomationProcessBackend::default());
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>,
        ));
        let cfg = automation_settings();
        let limits = crate::resource_monitor::ResourceLimits::from(&cfg);

        let session_id = Uuid::new_v4();
        let root = orphan_automation_group(&monitor, &backend, limits, session_id, 6_100);
        assert_eq!(monitor.active_agent_groups(), 1);
        // The blocker finally exits, which is the only thing a retry can discover.
        backend.mark_gone(AutomationProcessBackend::child_pid(6_100));

        let (app, coordinator) = automation_app_with_coordinator(Arc::clone(&monitor)).await;
        let response = handle_resource_watchdog_backend_request_with_config(
            app.handle(),
            &watchdog_backend_request("quarantineRetry"),
            &cfg,
        )
        .await;

        assert!(response.ok);
        let diagnostics = response.diagnostics.expect("watchdog diagnostics");
        let retries = diagnostics["quarantineRetries"]
            .as_array()
            .expect("quarantineRetries array");
        assert_eq!(retries.len(), 1);
        assert_eq!(retries[0]["sessionId"], json!(session_id));
        assert_eq!(retries[0]["path"], json!("orphan"));
        assert_eq!(retries[0]["rootPid"], json!(root.pid));
        assert_eq!(retries[0]["quarantined"], json!(false));
        assert_eq!(retries[0]["stillCountsTowardAdmission"], json!(false));
        assert_eq!(retries[0]["activeAgentGroups"], json!(0));
        // The pre-action snapshot deliberately still shows the group counted.
        assert_eq!(diagnostics["snapshot"]["activeAgentGroups"], json!(1));
        coordinator.close_and_join().await;
    }

    // #1151 (A2) - the safety artifact. An owned descendant that is still unverifiable
    // keeps blocking capacity, and the report says so instead of claiming a reclaim.
    #[tokio::test]
    async fn quarantine_retry_mode_reports_still_blocked_group() {
        let backend = Arc::new(AutomationProcessBackend::default());
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>,
        ));
        let cfg = automation_settings();
        let limits = crate::resource_monitor::ResourceLimits::from(&cfg);

        let session_id = Uuid::new_v4();
        // The blocker is deliberately NOT marked gone.
        orphan_automation_group(&monitor, &backend, limits, session_id, 6_200);

        let (app, coordinator) = automation_app_with_coordinator(Arc::clone(&monitor)).await;
        let response = handle_resource_watchdog_backend_request_with_config(
            app.handle(),
            &watchdog_backend_request("quarantineRetry"),
            &cfg,
        )
        .await;

        let diagnostics = response.diagnostics.expect("watchdog diagnostics");
        let retries = diagnostics["quarantineRetries"]
            .as_array()
            .expect("quarantineRetries array");
        assert_eq!(retries.len(), 1);
        assert_eq!(retries[0]["path"], json!("orphan"));
        assert_eq!(retries[0]["quarantined"], json!(true));
        assert_eq!(retries[0]["stillCountsTowardAdmission"], json!(true));
        assert_eq!(retries[0]["activeAgentGroups"], json!(1));
        assert_eq!(monitor.active_agent_groups(), 1);
        coordinator.close_and_join().await;
    }

    // #1151 (A4) - an unsupported backend stays at zero admission and zero enforcement
    // calls in the new mode too.
    #[tokio::test]
    async fn unsupported_backend_quarantine_retry_is_disabled() {
        let backend = Arc::new(UnsupportedAutomationBackend::default());
        let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>,
        ));
        let cfg = automation_settings();

        let (app, coordinator) = automation_app_with_coordinator(Arc::clone(&monitor)).await;
        let response = handle_resource_watchdog_backend_request_with_config(
            app.handle(),
            &watchdog_backend_request("quarantineRetry"),
            &cfg,
        )
        .await;

        assert!(response.ok);
        assert_eq!(response.target.expect("target")["state"], json!("disabled"));
        let diagnostics = response.diagnostics.expect("watchdog diagnostics");
        assert_eq!(diagnostics["quarantineRetries"], json!([]));
        assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.terminate_verified_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            backend.current_process_memory_calls.load(Ordering::SeqCst),
            0
        );
        coordinator.close_and_join().await;
    }

    #[test]
    fn action_serializes_camel_case() {
        for action in UiAutomationAction::all() {
            assert_eq!(
                serde_json::to_string(&action).unwrap(),
                format!("\"{}\"", action_wire_name(action))
            );
        }
    }

    #[test]
    fn terminal_operation_parser_accepts_exact_i32_boundaries() {
        assert_eq!(
            parse_terminal_operation("query"),
            Some(UiTerminalOperation::Query)
        );
        assert_eq!(
            parse_terminal_operation("top"),
            Some(UiTerminalOperation::Top)
        );
        assert_eq!(
            parse_terminal_operation("bottom"),
            Some(UiTerminalOperation::Bottom)
        );
        assert_eq!(
            parse_terminal_operation("line:0"),
            Some(UiTerminalOperation::Line(0))
        );
        assert_eq!(
            parse_terminal_operation("line:2147483647"),
            Some(UiTerminalOperation::Line(i32::MAX))
        );
        for (prefix, constructor) in [
            (
                "lines:",
                UiTerminalOperation::Lines as fn(i32) -> UiTerminalOperation,
            ),
            (
                "pages:",
                UiTerminalOperation::Pages as fn(i32) -> UiTerminalOperation,
            ),
        ] {
            assert_eq!(
                parse_terminal_operation(&format!("{prefix}-2147483648")),
                Some(constructor(i32::MIN))
            );
            assert_eq!(
                parse_terminal_operation(&format!("{prefix}0")),
                Some(constructor(0))
            );
            assert_eq!(
                parse_terminal_operation(&format!("{prefix}2147483647")),
                Some(constructor(i32::MAX))
            );
        }
    }

    #[test]
    fn terminal_operation_parser_rejects_every_noncanonical_grammar_class() {
        for value in [
            "",
            "QUERY",
            " query",
            "query ",
            "line:",
            "line:+1",
            "line:-1",
            "line:00",
            "line:01",
            "line:-0",
            "line:1.0",
            "line:1e2",
            "line:1:2",
            "line:2147483648",
            "lines:",
            "lines:+1",
            "lines:01",
            "lines:-0",
            "lines:1.0",
            "lines:1e2",
            "lines:1:2",
            "lines:2147483648",
            "lines:-2147483649",
            "pages:+1",
            "pages:01",
            "pages:-0",
            "pages:2147483648",
            "pages:-2147483649",
            "unknown:1",
        ] {
            assert_eq!(
                parse_terminal_operation(value),
                None,
                "unexpectedly accepted {value:?}"
            );
        }
    }

    #[test]
    fn terminal_session_id_validation_uses_exact_utf8_bytes_and_control_rule() {
        assert!(!valid_terminal_session_id(""));
        assert!(!valid_terminal_session_id("bad\nvalue"));
        assert!(!valid_terminal_session_id("bad\u{7f}value"));
        assert!(valid_terminal_session_id("a"));
        assert!(valid_terminal_session_id(&"a".repeat(256)));
        assert!(!valid_terminal_session_id(&"a".repeat(257)));
        assert!(valid_terminal_session_id(&"é".repeat(128)));
        assert!(!valid_terminal_session_id(&"é".repeat(129)));
    }

    #[test]
    fn terminal_cli_request_preserves_session_and_derives_fixed_selector() {
        let session_id = " exact-é session ".to_string();
        let request = terminal_cli_request(UiTerminalArgs {
            window: "terminal".to_string(),
            session_id: session_id.clone(),
            timeout_ms: 4321,
            operation: "pages:-2".to_string(),
        })
        .expect("valid terminal request");

        assert_eq!(request.window, "terminal");
        assert_eq!(request.selector, format!("terminal.session.{session_id}"));
        assert_eq!(request.action, UiAutomationAction::Terminal);
        assert_eq!(request.value.as_deref(), Some("pages:-2"));
        assert_eq!(request.timeout_ms, 4321);
    }

    #[test]
    fn terminal_cli_request_rejects_before_request_construction() {
        let invalid_session = terminal_cli_request(UiTerminalArgs {
            window: "main".to_string(),
            session_id: "bad\nvalue".to_string(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            operation: "query".to_string(),
        })
        .unwrap_err();
        assert_eq!(invalid_session["error"], "invalid_terminal_session_id");

        let invalid_operation = terminal_cli_request(UiTerminalArgs {
            window: "main".to_string(),
            session_id: "session-a".to_string(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            operation: "line:01".to_string(),
        })
        .unwrap_err();
        assert_eq!(invalid_operation["error"], "invalid_terminal_operation");
    }

    /// #944 - the Rust enum and the `UiAutomationAction` union in `src/shared/types.ts`
    /// are two closed lists that MUST agree. They are not generated from one another
    /// (no ts-rs / typeshare / specta in this crate), so nothing but this test stops
    /// them drifting.
    ///
    /// Drift is NOT a crash, and an earlier draft of this comment claimed it was: the
    /// request file is written and read by the same Rust enum, so serde never sees an
    /// action it does not know. What actually happens is worse in one way. A Rust-only
    /// addition ships a verb that exists, is documented, and fails 100% of the time at
    /// runtime, because the bridge's catch-all (automation-bridge.ts:181-188) answers
    /// `unsupported_action`. Today that is caught only by a human running the acceptance
    /// runbook. This test moves it to `cargo test`.
    #[test]
    fn ui_automation_action_wire_names_match_typescript_union() {
        // First test in this crate to read OUTSIDE the crate root. `CARGO_MANIFEST_DIR`
        // expands at COMPILE time to the absolute path of src-tauri, so the process cwd
        // is irrelevant, and CI checks out the full tree (no sparse-checkout).
        let types_ts = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/shared/types.ts"),
        )
        .expect("read src/shared/types.ts");

        let start = types_ts
            .find("export type UiAutomationAction =")
            .expect("UiAutomationAction union not found in types.ts");

        // Strip `//` comments FIRST, before anything else reads this text. Two hazards,
        // and the ORDER is what defuses them: a comment inside the union carrying a quote
        // would inject a phantom member, and one carrying a `;` would truncate the union
        // early. The first revision of this test stripped AFTER `find(';')`, which left the
        // truncation hazard fully live while this comment claimed it was handled. That is
        // precisely the defect #944 exists to kill (a comment asserting a protection the
        // code does not have), so it is called out here rather than quietly repaired:
        // strip, THEN find the terminator. Both are false REDs, and a parity test that
        // cries wolf is a parity test somebody deletes.
        let stripped: String = types_ts[start..]
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");

        let end = stripped
            .find(';')
            .expect("unterminated UiAutomationAction union");

        let members: HashSet<String> = stripped[..end]
            .split('"')
            .skip(1)
            .step_by(2)
            .map(|s| s.to_string())
            .collect();

        assert!(
            !members.is_empty(),
            "parsed zero members from the UiAutomationAction union; the parser or the union format changed"
        );

        let rust: HashSet<String> = UiAutomationAction::all()
            .map(|action| action_wire_name(action).to_string())
            .collect();

        assert_eq!(
            members, rust,
            "UiAutomationAction is out of sync between src/shared/types.ts and ui_automation.rs"
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
    fn frontend_ready_registers_dynamic_caller_label() {
        let tmp = tempfile::tempdir().unwrap();
        let state = UiAutomationState::new(true, tmp.path().to_path_buf());
        state.initialize_files().unwrap();

        state
            .mark_frontend_ready("resource-monitor", Some("resource-monitor"))
            .unwrap();

        let raw = fs::read_to_string(&state.inner.session_path).unwrap();
        let session: UiAutomationSession = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            session.window_labels,
            vec!["main".to_string(), "resource-monitor".to_string()]
        );
        assert_eq!(
            session.ready_window_labels,
            vec!["resource-monitor".to_string()]
        );
    }

    #[test]
    fn live_window_sync_prunes_closed_dynamic_windows() {
        let tmp = tempfile::tempdir().unwrap();
        let state = UiAutomationState::new(true, tmp.path().to_path_buf());
        state.initialize_files().unwrap();
        state
            .mark_frontend_ready("resource-monitor", Some("resource-monitor"))
            .unwrap();

        state
            .sync_live_window_labels(vec!["main".to_string()])
            .unwrap();

        let raw = fs::read_to_string(&state.inner.session_path).unwrap();
        let session: UiAutomationSession = serde_json::from_str(&raw).unwrap();
        assert_eq!(session.window_labels, vec!["main".to_string()]);
        assert!(session.ready_window_labels.is_empty());
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
    fn expire_pending_requests_writes_timeout_response() {
        let tmp = tempfile::tempdir().unwrap();
        let state = UiAutomationState::new(true, tmp.path().to_path_buf());
        fs::create_dir_all(state.requests_dir()).unwrap();
        fs::create_dir_all(state.responses_dir()).unwrap();

        let request_id = Uuid::new_v4().to_string();
        let mut request = sample_request(request_id.clone(), "missing.target");
        request.expires_at_unix_ms = Some(now_unix_ms() - 1);
        let response_path = state.response_path(&request_id);
        let inflight_path = state.inflight_path(&request_id);
        fs::write(&inflight_path, "{}").unwrap();
        state.inner.pending.lock().unwrap().insert(
            request_id,
            PendingRequest {
                request,
                response_path: response_path.clone(),
                inflight_path: inflight_path.clone(),
            },
        );

        state.expire_pending_requests();

        let raw = fs::read_to_string(response_path).unwrap();
        let written: UiAutomationResponse = serde_json::from_str(&raw).unwrap();
        assert!(!written.ok);
        assert_eq!(written.error.as_deref(), Some("timeout"));
        assert!(!inflight_path.exists());
        assert!(state.inner.pending.lock().unwrap().is_empty());
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
