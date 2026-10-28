//! Host-only audit log for the control-plane API (#791, §0.5 G8).
//!
//! Appends one line per mint / revoke / authenticated request to
//! `api-audit.log` in `config_dir()`. The log is capped at 10 MB with a single
//! `.1` rotation; the secret and its hash are NEVER logged (only `clientId` /
//! `boundFqn`, which are safe). Auditing NEVER fails closed: any I/O error is
//! logged via `log::warn!` and swallowed, so a full disk cannot take the API
//! down.

use std::path::PathBuf;

use serde::Serialize;

use crate::config::instance_artifacts::API_AUDIT_LOG_FILE_NAME;
use crate::filesystem::activity_log;

/// Max audit-log size before rotation (10 MB).
const AUDIT_MAX_BYTES: u64 = 10 * 1024 * 1024;

fn audit_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join(API_AUDIT_LOG_FILE_NAME))
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PtyInputAuditMetadata {
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub injection_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_fqn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_fqn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_plane: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_backend: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
    pub timestamp: String,
}

/// Write one payload-free PTY-input audit object through the same fail-soft
/// rotation policy as the general API audit. Callers may supply only verified
/// identities and fixed reason codes.
fn record_audit_metadata(metadata: &impl serde::Serialize, record_kind: &str) {
    let Some(path) = audit_path() else {
        return;
    };
    rotate_if_needed(&path);
    let mut line = match serde_json::to_string(metadata) {
        Ok(line) => line,
        Err(_) => {
            log::warn!("[api-audit] failed to serialize {record_kind} metadata (continuing)");
            return;
        }
    };
    line.push('\n');
    if append(&path, &line).is_err() {
        log::warn!(
            "[api-audit] failed to write {record_kind} metadata \
             code=audit_write_failed (continuing)"
        );
    }
}

pub fn record_pty_input(metadata: &PtyInputAuditMetadata) {
    record_audit_metadata(metadata, "PTY");
}

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSnapshotAuditMetadata {
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requester_fqn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_fqn: Option<String>,
    pub source_plane: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_backend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub columns: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<String>,
    pub completed_at: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

impl std::fmt::Debug for TerminalSnapshotAuditMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotAuditMetadata")
            .field("has_request_id", &self.request_id.is_some())
            .field("has_requester", &self.requester_fqn.is_some())
            .field("has_target", &self.target_fqn.is_some())
            .field("has_format", &self.format.is_some())
            .field("has_selected_session", &self.selected_session_id.is_some())
            .field("has_selected_backend", &self.selected_backend.is_some())
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .field("sequence", &self.sequence)
            .field("has_captured_at", &self.captured_at.is_some())
            .field("payload_bytes", &self.payload_bytes)
            .field("has_accepted_at", &self.accepted_at.is_some())
            .field("has_reason_code", &self.reason_code.is_some())
            .finish_non_exhaustive()
    }
}

pub fn record_terminal_snapshot(metadata: &TerminalSnapshotAuditMetadata) {
    let Some(path) = audit_path() else {
        return;
    };
    rotate_if_needed(&path);
    let mut line = match serde_json::to_string(metadata) {
        Ok(line) => line,
        Err(_) => {
            log::warn!("[api-audit] snapshot metadata serialization failed");
            return;
        }
    };
    line.push('\n');
    if append(&path, &line).is_err() {
        log::warn!("[api-audit] snapshot metadata write failed");
    }
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WindowScreenshotAuditStatus {
    Succeeded,
    InvalidWindowId,
    WindowNotFound,
    CaptureBusy,
    CaptureTooLarge,
    CaptureUnavailable,
}

#[cfg(target_os = "windows")]
pub(crate) struct WindowScreenshotAuditResult {
    pub status: WindowScreenshotAuditStatus,
}

#[cfg(all(test, target_os = "windows"))]
std::thread_local! {
    static WINDOW_SCREENSHOT_AUDITS_FOR_TEST:
        std::cell::RefCell<Option<Vec<WindowScreenshotAuditStatus>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, target_os = "windows"))]
pub(crate) struct WindowScreenshotAuditCaptureForTest;

#[cfg(all(test, target_os = "windows"))]
impl Drop for WindowScreenshotAuditCaptureForTest {
    fn drop(&mut self) {
        WINDOW_SCREENSHOT_AUDITS_FOR_TEST.with(|audits| {
            *audits.borrow_mut() = None;
        });
    }
}

#[cfg(all(test, target_os = "windows"))]
pub(crate) fn lock_window_screenshot_audits_for_test() -> WindowScreenshotAuditCaptureForTest {
    WINDOW_SCREENSHOT_AUDITS_FOR_TEST.with(|audits| {
        let mut audits = audits.borrow_mut();
        assert!(
            audits.is_none(),
            "screenshot audit capture is already active"
        );
        *audits = Some(Vec::new());
    });
    WindowScreenshotAuditCaptureForTest
}

#[cfg(all(test, target_os = "windows"))]
pub(crate) fn take_window_screenshot_audits_for_test() -> Vec<WindowScreenshotAuditStatus> {
    WINDOW_SCREENSHOT_AUDITS_FOR_TEST.with(|audits| {
        let mut audits = audits.borrow_mut();
        match audits.as_mut() {
            Some(audits) => std::mem::take(audits),
            None => Vec::new(),
        }
    })
}

#[cfg(any(target_os = "windows", test))]
#[derive(serde::Serialize)]
struct WindowScreenshotAuditMetadata {
    event: &'static str,
    operation: &'static str,
    status: WindowScreenshotAuditStatus,
    timestamp: String,
}

#[cfg(any(target_os = "windows", test))]
impl WindowScreenshotAuditMetadata {
    fn new(status: WindowScreenshotAuditStatus) -> Self {
        Self {
            event: "window_screenshot",
            operation: "window_screenshot",
            status,
            timestamp: crate::phone::types::canonical_pty_timestamp(chrono::Utc::now()),
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn record_window_screenshot_result(_event: &str, result: &WindowScreenshotAuditResult) {
    #[cfg(test)]
    {
        WINDOW_SCREENSHOT_AUDITS_FOR_TEST.with(|audits| {
            if let Some(audits) = audits.borrow_mut().as_mut() {
                audits.push(result.status);
            }
        });
    }

    let metadata = WindowScreenshotAuditMetadata::new(result.status);
    record_audit_metadata(&metadata, "window screenshot");
}

pub fn record_pty_input_result(event: &str, result: &crate::phone::types::PtyInputResult) {
    let source_plane = result.source_plane.map(|source| match source {
        crate::phone::types::PtyInputSourcePlane::HostCli => "host_cli".to_string(),
        crate::phone::types::PtyInputSourcePlane::ContainerApi => "container_api".to_string(),
    });
    let status = serde_json::to_value(result.status)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "indeterminate".to_string());
    record_pty_input(&PtyInputAuditMetadata {
        event: event.to_string(),
        injection_id: Some(result.injection_id.clone()),
        op_id: result.op_id.clone(),
        sender_fqn: result.sender.clone(),
        target_fqn: result.target.clone(),
        payload_bytes: result.payload_bytes,
        payload_sha256: result.payload_sha256.clone(),
        source_plane,
        selected_session_id: result.selected_session_id.clone(),
        selected_backend: result.selected_backend.clone(),
        status,
        reason_code: result
            .reason
            .as_ref()
            .map(|reason| crate::phone::types::pty_input_reason_code_name(reason.code).to_string()),
        timestamp: crate::phone::types::canonical_pty_timestamp(chrono::Utc::now()),
    });
}

/// Append a single audit line `(ts, clientId, boundFqn, op, outcome)`.
/// Best-effort: never returns an error, never panics.
pub fn record(client_id: &str, bound_fqn: &str, op: &str, outcome: &str) {
    let Some(path) = audit_path() else {
        return;
    };
    // Rotate BEFORE appending if the current file is at/over the cap.
    rotate_if_needed(&path);

    let ts = chrono::Utc::now().to_rfc3339();
    // Sanitize embedded newlines so one field cannot forge extra log lines.
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\n",
        ts,
        sanitize(client_id),
        sanitize(bound_fqn),
        sanitize(op),
        sanitize(outcome),
    );
    if let Err(e) = append(&path, &line) {
        log::warn!("[api-audit] failed to write audit line (continuing): {}", e);
    }
}

fn sanitize(field: &str) -> String {
    field.replace(['\n', '\r', '\t'], " ")
}

fn append(path: &std::path::Path, line: &str) -> std::io::Result<()> {
    activity_log::append_activity(path, line)
}

/// If the log is at/over `AUDIT_MAX_BYTES`, move it to `<name>.1` (replacing any
/// prior `.1`) so the live file restarts empty. Best-effort.
fn rotate_if_needed(path: &std::path::Path) {
    let size = match activity_log::activity_metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return, // no file yet, or unreadable: nothing to rotate
    };
    if size < AUDIT_MAX_BYTES {
        return;
    }
    let rotated = path.with_extension("log.1");
    if let Err(e) = activity_log::rename_activity(path, &rotated) {
        log::warn!("[api-audit] rotation failed (continuing): {}", e);
    }
}

#[cfg(test)]
mod tests;
