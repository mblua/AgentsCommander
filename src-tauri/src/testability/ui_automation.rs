mod args;
mod backend_watchdog;
mod cli;
mod cli_run;
mod files;
mod liveness;

use self::cli::CliRequest;
pub use self::cli::{
    execute_backend, execute_click, execute_context_click, execute_hover, execute_key,
    execute_pointer, execute_query, execute_set, execute_terminal, execute_type, execute_wait,
};
#[cfg(test)]
use self::cli::{
    key_cli_request, parse_terminal_operation, pointer_cli_request, terminal_cli_request,
    valid_key_chord, valid_pointer_operation, valid_terminal_session_id, UiTerminalOperation,
};
use crate::shutdown::ShutdownSignal;
pub use args::{
    UiBackendArgs, UiClickArgs, UiContextClickArgs, UiHoverArgs, UiKeyArgs, UiPointerArgs,
    UiQueryArgs, UiSetArgs, UiTerminalArgs, UiTypeArgs, UiWaitArgs,
};
use backend_watchdog::handle_backend_request;
#[cfg(test)]
use backend_watchdog::handle_resource_watchdog_backend_request_with_config;
#[cfg(test)]
use cli_run::timeout_phase;
pub use cli_run::{
    automation_not_enabled_json, existing_enabled_session_for_current_config,
    resolve_enabled_from_cli_or_env,
};
use cli_run::{
    expired_response_for_request, is_backend_automation_window, preflight_error, print_stdout_json,
    request_expired, run_cli_request,
};
#[cfg(test)]
use files::SessionReadHookGuard;
use files::{
    cleanup_stale_automation_files, read_session_file_with_retry, retry_remove_file, retry_rename,
    write_json_atomic_new, write_json_atomic_replace,
};
pub use liveness::pid_is_alive;
#[cfg(target_os = "windows")]
use liveness::process_path;
use liveness::{
    available_window_labels, current_exe_path_string, now_unix_ms, paths_equal_for_compare,
    sorted_labels,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{fs, io};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

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
    Pointer,
    Key,
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
            Self::Terminal => Self::Pointer,
            Self::Pointer => Self::Key,
            Self::Key => return None,
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
mod tests;
