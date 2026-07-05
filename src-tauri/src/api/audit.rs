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

/// Max audit-log size before rotation (10 MB).
const AUDIT_MAX_BYTES: u64 = 10 * 1024 * 1024;

fn audit_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("api-audit.log"))
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
