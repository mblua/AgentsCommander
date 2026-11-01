mod args;
pub(crate) mod backend_watchdog;
mod cli_run;
mod files;
pub(crate) mod liveness;
mod protocol;

#[cfg(test)]
use self::protocol::UiTerminalOperation;
use self::protocol::{
    parse_terminal_operation, valid_key_chord, valid_pointer_operation, valid_terminal_session_id,
    CliRequest,
};
pub use self::protocol::{
    UiAutomationAction, UiAutomationRequest, UiAutomationResponse, UiAutomationSession,
};
use crate::shutdown::ShutdownSignal;
pub use args::{
    UiBackendArgs, UiClickArgs, UiContextClickArgs, UiHoverArgs, UiKeyArgs, UiPointerArgs,
    UiQueryArgs, UiSetArgs, UiTerminalArgs, UiTypeArgs, UiWaitArgs,
};
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
use futures::future::BoxFuture;
pub use liveness::pid_is_alive;
#[cfg(target_os = "windows")]
use liveness::process_path;
use liveness::{current_exe_path_string, now_unix_ms, paths_equal_for_compare, sorted_labels};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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

pub trait UiAutomationEvents {
    fn automation_request(&self, window: &str, request: &UiAutomationRequest)
        -> Result<(), String>;
}

pub trait UiAutomationHost: UiAutomationEvents + Clone + Send + Sync + 'static {
    fn available_windows(&self) -> Vec<String>;
    fn spawn_background(&self, future: BoxFuture<'static, ()>);
    fn backend_request<'a>(
        &'a self,
        request: &'a UiAutomationRequest,
    ) -> BoxFuture<'a, UiAutomationResponse>;
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

    pub fn start<H: UiAutomationHost>(&self, app: H, shutdown: ShutdownSignal) {
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
        app.clone().spawn_background(Box::pin(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(POLL_MS));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => state.poll_once(&app),
                }
            }
        }));
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
        crate::filesystem::directories::ensure_ui_automation_directory(&self.requests_dir())?;
        crate::filesystem::directories::ensure_ui_automation_directory(&self.responses_dir())?;
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

    fn poll_once<H: UiAutomationHost>(&self, app: &H) {
        if let Err(e) = self.poll_once_inner(app) {
            log::warn!("[ui-automation] poll failed: {}", e);
        }
    }

    fn poll_once_inner<H: UiAutomationHost>(&self, app: &H) -> io::Result<()> {
        self.sync_live_window_labels(app.available_windows())?;
        self.expire_pending_requests();

        let requests_dir = self.requests_dir();
        match crate::filesystem::directories::visit_ui_automation_request_paths(
            &requests_dir,
            |path| {
                let Some(request_file) = RequestFile::from_path(&path) else {
                    return;
                };
                if self.is_pending(&request_file.request_id) {
                    return;
                }
                self.process_request_file(app, request_file);
            },
        ) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
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

    fn process_request_file<H: UiAutomationHost>(&self, app: &H, request_file: RequestFile) {
        let raw = match crate::filesystem::documents::read_ui_automation_text(&request_file.path) {
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

        let available_windows = app.available_windows();
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

        self.emit_frontend_request(app, request, inflight_path);
    }

    fn emit_frontend_request<H: UiAutomationHost>(
        &self,
        app: &H,
        request: UiAutomationRequest,
        inflight_path: PathBuf,
    ) {
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

        if let Err(e) = app.automation_request(&request.window, &request) {
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
                response.available_windows = Some(app.available_windows());
                response.diagnostics = Some(json!({ "message": e.to_string() }));
                let _ = write_json_atomic_new(&pending.response_path, &response);
                let _ = retry_remove_file(&pending.inflight_path);
            }
        }
    }

    fn process_backend_request_file<H: UiAutomationHost>(
        &self,
        app: &H,
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
        app.clone().spawn_background(Box::pin(async move {
            let response = app.backend_request(&request).await;
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
        }));
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

pub fn execute_query(args: UiQueryArgs) -> i32 {
    execute_cli(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::Query,
        value: None,
        timeout_ms: args.timeout_ms,
    })
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
            "Terminal operation must be query, top, bottom, line:N, lines:N, or pages:N in the \
                signed 32-bit range.",
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

fn pointer_cli_request(args: UiPointerArgs) -> Result<CliRequest, Value> {
    if !valid_pointer_operation(&args.operation) {
        return Err(preflight_error(
            "invalid_pointer_operation",
            "Pointer operation must be down, move, move:top, move:bottom, up, or cancel.",
            None,
        ));
    }
    let target_free = matches!(args.operation.as_str(), "up" | "cancel");
    let selector = match (target_free, args.selector) {
        (false, Some(selector)) => selector,
        (false, None) => {
            return Err(preflight_error(
                "pointer_selector_required",
                "Pointer down and move operations require --selector.",
                None,
            ))
        }
        // Empty like `ui-hover --leave`; the frontend echoes it, so `complete()` still matches.
        (true, None) => String::new(),
        (true, Some(_)) => {
            return Err(preflight_error(
                "pointer_selector_not_allowed",
                "Pointer up and cancel operations do not accept --selector.",
                None,
            ))
        }
    };

    Ok(CliRequest {
        window: args.window,
        selector,
        action: UiAutomationAction::Pointer,
        value: Some(args.operation),
        timeout_ms: args.timeout_ms,
    })
}

fn key_cli_request(args: UiKeyArgs) -> Result<CliRequest, Value> {
    if !valid_key_chord(&args.chord) {
        return Err(preflight_error(
            "invalid_key_chord",
            "Key chord must be optional Ctrl+, Alt+, Shift+, Meta+ in that order, then Escape, \
                Enter, Space, Tab, ArrowUp, ArrowDown, ArrowLeft, ArrowRight, Home, or End.",
            None,
        ));
    }

    Ok(CliRequest {
        window: args.window,
        selector: args.selector,
        action: UiAutomationAction::Key,
        value: Some(args.chord),
        timeout_ms: args.timeout_ms,
    })
}

pub fn execute_pointer(args: UiPointerArgs) -> i32 {
    match pointer_cli_request(args) {
        Ok(request) => execute_cli(request),
        Err(error) => {
            print_stdout_json(&error);
            1
        }
    }
}

pub fn execute_key(args: UiKeyArgs) -> i32 {
    match key_cli_request(args) {
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

#[cfg(test)]
mod tests;
