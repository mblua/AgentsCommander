//! Host-only audit log for the control-plane API (#791, §0.5 G8).
//!
//! Appends one line per mint / revoke / authenticated request to
//! `api-audit.log` in `config_dir()`. The log is capped at 10 MB with a single
//! `.1` rotation; the secret and its hash are NEVER logged (only `clientId` /
//! `boundFqn`, which are safe). Auditing NEVER fails closed: any I/O error is
//! logged via `log::warn!` and swallowed, so a full disk cannot take the API
//! down.

use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;

/// Max audit-log size before rotation (10 MB).
const AUDIT_MAX_BYTES: u64 = 10 * 1024 * 1024;

fn audit_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("api-audit.log"))
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
pub fn record_pty_input(metadata: &PtyInputAuditMetadata) {
    let Some(path) = audit_path() else {
        return;
    };
    rotate_if_needed(&path);
    let mut line = match serde_json::to_string(metadata) {
        Ok(line) => line,
        Err(_) => {
            log::warn!("[api-audit] failed to serialize PTY metadata (continuing)");
            return;
        }
    };
    line.push('\n');
    if append(&path, &line).is_err() {
        log::warn!("[api-audit] failed to write PTY metadata code=audit_write_failed (continuing)");
    }
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
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(line.as_bytes())
}

/// If the log is at/over `AUDIT_MAX_BYTES`, move it to `<name>.1` (replacing any
/// prior `.1`) so the live file restarts empty. Best-effort.
fn rotate_if_needed(path: &std::path::Path) {
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return, // no file yet, or unreadable: nothing to rotate
    };
    if size < AUDIT_MAX_BYTES {
        return;
    }
    let rotated = path.with_extension("log.1");
    if let Err(e) = std::fs::rename(path, &rotated) {
        log::warn!("[api-audit] rotation failed (continuing): {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_newlines_and_tabs() {
        assert_eq!(sanitize("a\nb\tc\rd"), "a b c d");
    }

    #[test]
    fn pty_metadata_serialization_has_no_payload_or_secret_field() {
        let metadata = PtyInputAuditMetadata {
            event: "enqueue".into(),
            injection_id: Some(uuid::Uuid::new_v4().to_string()),
            op_id: Some(uuid::Uuid::new_v4().to_string()),
            sender_fqn: Some("project:wg-1-team/lead".into()),
            target_fqn: Some("project:wg-1-team/dev".into()),
            payload_bytes: Some(12),
            payload_sha256: Some("a".repeat(64)),
            source_plane: Some("container_api".into()),
            selected_session_id: None,
            selected_backend: None,
            status: "queued".into(),
            reason_code: None,
            timestamp: "2026-01-01T00:00:00.000Z".into(),
        };
        let value = serde_json::to_value(metadata).unwrap();
        assert!(value.get("payload").is_none());
        assert!(value.get("text").is_none());
        assert!(value.get("token").is_none());
        assert_eq!(value["payloadBytes"], 12);
    }

    #[test]
    fn snapshot_metadata_has_no_content_secret_or_path_fields() {
        let metadata = TerminalSnapshotAuditMetadata {
            event: "terminal_snapshot".into(),
            request_id: Some(uuid::Uuid::new_v4().to_string()),
            requester_fqn: Some("project:wg-1-team/lead".into()),
            target_fqn: Some("project:wg-1-team/dev".into()),
            source_plane: "host_cli".into(),
            format: Some("json".into()),
            selected_session_id: Some(uuid::Uuid::new_v4().to_string()),
            selected_backend: Some("localProcess".into()),
            rows: Some(30),
            columns: Some(120),
            sequence: Some(7),
            captured_at: Some("2026-01-01T00:00:00.000Z".into()),
            payload_bytes: Some(123),
            accepted_at: Some("2026-01-01T00:00:00.000Z".into()),
            completed_at: "2026-01-01T00:00:01.000Z".into(),
            status: "succeeded".into(),
            reason_code: None,
        };
        let diagnostic = format!("{metadata:?}");
        for forbidden in [
            metadata.request_id.as_deref().unwrap(),
            metadata.requester_fqn.as_deref().unwrap(),
            metadata.target_fqn.as_deref().unwrap(),
            metadata.selected_session_id.as_deref().unwrap(),
            metadata.captured_at.as_deref().unwrap(),
            metadata.accepted_at.as_deref().unwrap(),
            metadata.completed_at.as_str(),
            metadata.status.as_str(),
        ] {
            assert!(!diagnostic.contains(forbidden));
        }
        assert!(diagnostic.contains("rows: Some(30)"));
        assert!(diagnostic.contains("payload_bytes: Some(123)"));

        let value = serde_json::to_value(metadata).unwrap();
        for forbidden in [
            "token",
            "nonce",
            "confirmationTag",
            "text",
            "cells",
            "lines",
            "pngBase64",
            "output",
            "path",
            "workingDirectory",
        ] {
            assert!(
                value.get(forbidden).is_none(),
                "forbidden audit field {forbidden}"
            );
        }
    }

    #[test]
    fn append_then_rotate_moves_to_dot_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("api-audit.log");
        // Write >10MB worth so rotation triggers on the next check.
        let big = "x".repeat(1024);
        {
            let mut f = std::fs::File::create(&path).unwrap();
            for _ in 0..(AUDIT_MAX_BYTES / 1024 + 1) {
                f.write_all(big.as_bytes()).unwrap();
            }
        }
        assert!(std::fs::metadata(&path).unwrap().len() >= AUDIT_MAX_BYTES);
        rotate_if_needed(&path);
        let rotated = path.with_extension("log.1");
        assert!(rotated.exists(), "rotated .1 file must exist");
        assert!(!path.exists(), "live log must be moved away on rotation");
    }

    #[test]
    fn append_creates_and_appends() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("api-audit.log");
        append(&path, "line1\n").unwrap();
        append(&path, "line2\n").unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "line1\nline2\n");
    }
}
