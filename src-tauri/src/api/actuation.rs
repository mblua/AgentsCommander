//! The resolution and routing seam for API sends (#791 §8, §0.5 HIGH-3).
//!
//! Both the filesystem poller and the API resolve and route through the SAME
//! `can_communicate` / `resolve_agent_target`, so the two planes cannot diverge
//! in canonicalization for the non-root verb set. Actuation is NOT performed
//! here: `api/handlers/send.rs` queues the send durably and `api/dispatcher.rs`
//! delivers it through `MailboxPoller::deliver_wake_with_origin` with
//! `WakeDeliveryOrigin::DbQueue` (#1177 removed the inline delivery path that
//! used to live in this file). Root-Agent routing is intentionally API-excluded
//! in increment 1 (`can_communicate` does not model the root branches) and
//! rejected at the boundary.

use std::path::Path;

use tauri::Manager;

use crate::api::message_store::INLINE_BODY_MAX_BYTES;
use crate::config::settings::SettingsState;

use super::error::ApiError;

/// Reject either direction of a Root-Agent send (§0.5 HIGH-3). Pure, so it is
/// unit-testable without an `AppHandle`.
pub fn reject_if_root(from: &str, to: &str) -> Result<(), ApiError> {
    if from == crate::config::root_agent::ROOT_AGENT_SENDER
        || crate::config::root_agent::is_root_agent_target(to)
    {
        return Err(ApiError::Forbidden(
            "root-agent not reachable over the API in increment 1".to_string(),
        ));
    }
    Ok(())
}

/// Build the host-absolute notification body for a `send`, mirroring
/// `cli/send.rs`'s `--send` path exactly: validate the bare filename, resolve
/// it inside the sender's `<workgroup>/messaging/` on the HOST (canonicalized-
/// parent confinement), and format the pointer notification. The container path
/// is never embedded because the CLI helper canonicalizes host-side.
pub fn build_send_body(bound_root: &str, basename: &str, from: &str) -> Result<String, ApiError> {
    let abs = resolve_send_file_path(bound_root, basename)?;

    // UNC-strip at the single emission site, mirroring cli/send.rs.
    let abs_str = abs.to_string_lossy();
    let abs_display = abs_str.trim_start_matches(r"\\?\");
    let body = crate::phone::messaging::format_file_notification(abs_display);

    // PTY_SAFE_MAX clamp, mirroring the live wake path (get_output = false).
    let overhead = crate::phone::messaging::PTY_WRAP_FIXED + from.len();
    if body.len() + overhead > crate::phone::messaging::PTY_SAFE_MAX {
        return Err(ApiError::BadRequest(
            "notification exceeds PTY-safe length; shorten the slug or use a shallower room path"
                .to_string(),
        ));
    }
    Ok(body)
}

#[derive(Debug)]
pub struct SendFileContent {
    pub body: String,
    pub source_ref: String,
}

pub fn read_send_file_content(
    bound_root: &str,
    basename: &str,
) -> Result<SendFileContent, ApiError> {
    let abs = resolve_send_file_path(bound_root, basename)?;
    let bytes = std::fs::read(&abs)
        .map_err(|e| ApiError::BadRequest(format!("message file not readable: {}", e)))?;
    if bytes.len() > INLINE_BODY_MAX_BYTES {
        return Err(ApiError::PayloadTooLarge(format!(
            "message file exceeds inline cap ({} bytes)",
            INLINE_BODY_MAX_BYTES
        )));
    }
    let body = String::from_utf8(bytes)
        .map_err(|e| ApiError::BadRequest(format!("message file is not UTF-8: {}", e)))?;
    Ok(SendFileContent {
        body,
        source_ref: basename.to_string(),
    })
}

fn resolve_send_file_path(
    bound_root: &str,
    basename: &str,
) -> Result<std::path::PathBuf, ApiError> {
    crate::phone::messaging::validate_filename_only(basename)
        .map_err(|e| ApiError::BadRequest(format!("invalid filename: {}", e)))?;

    let agent_root = Path::new(bound_root);
    let wg_root = crate::phone::messaging::workgroup_root(agent_root)
        .map_err(|e| ApiError::BadRequest(format!("bound replica is not under a room: {}", e)))?;
    let msg_dir = crate::phone::messaging::messaging_dir(&wg_root)
        .map_err(|e| ApiError::Internal(format!("cannot resolve messaging dir: {}", e)))?;
    let abs = crate::phone::messaging::resolve_existing_message(&msg_dir, basename)
        .map_err(|e| ApiError::BadRequest(format!("message file not found: {}", e)))?;
    Ok(abs)
}

/// Resolve and authorize the API send target without delivering.
pub async fn resolve_api_send_target<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    from: &str,
    to: &str,
) -> Result<String, ApiError> {
    reject_if_root(from, to)?;

    // Project paths: the same source `process_message` reads (SettingsState).
    let paths = {
        let state = app.state::<SettingsState>();
        let settings = state.read().await;
        settings.project_paths.clone()
    };

    // (1) canonicalize the target (belt-and-braces; the CLI resolves too).
    let resolved_to = crate::config::teams::resolve_agent_target(to, &paths)
        .map_err(|e| ApiError::BadRequest(format!("unresolvable target: {}", e)))?;

    // Re-check after resolution: a bare name could canonicalize to the root.
    reject_if_root(from, &resolved_to)?;

    // (2) route via the SAME predicate the poller uses.
    if !crate::config::teams::can_communicate(
        from,
        &resolved_to,
        &crate::config::teams::discover_teams(),
    ) {
        return Err(ApiError::Forbidden(format!(
            "routing rejected: '{}' cannot reach '{}'",
            from, resolved_to
        )));
    }
    Ok(resolved_to)
}

/// In-process `list-peers-lean` for the caller's bound replica, reusing the
/// exact CLI discovery + `reachable` computation (no subprocess).
pub fn lean_peers_via_api(
    bound_root: &str,
) -> Result<Vec<crate::cli::list_peers::LeanPeerInfo>, ApiError> {
    crate::cli::list_peers::lean_peers(bound_root)
        .map_err(|e| ApiError::Internal(format!("peer discovery failed: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_if_root_blocks_root_target_and_sender() {
        let root = crate::config::root_agent::ROOT_AGENT_SENDER;
        assert!(reject_if_root("proj:wg-1/dev", "proj:wg-1/lead").is_ok());
        assert!(reject_if_root(root, "proj:wg-1/lead").is_err());
        assert!(reject_if_root("proj:wg-1/dev", root).is_err());
        assert_eq!(
            reject_if_root(root, "x").unwrap_err().status(),
            axum::http::StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn build_send_body_resolves_existing_message_and_formats_pointer() {
        // Real WG-replica layout with a messaging file present.
        let temp = tempfile::TempDir::new().unwrap();
        let wg_root = temp.path().join("proj-x").join(".ac").join("wg-1-team");
        let replica = wg_root.join("__agent_dev-rust");
        let messaging = wg_root.join("messaging");
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::create_dir_all(&messaging).unwrap();
        let fname = "20260704-000000-wg1-a-to-wg1-b-hello.md";
        std::fs::write(messaging.join(fname), "hi").unwrap();

        let body = build_send_body(
            replica.to_str().unwrap(),
            fname,
            "proj-x:wg-1-team/dev-rust",
        )
        .expect("existing message should resolve");
        assert!(body.starts_with(crate::phone::messaging::FILE_NOTIFICATION_PREFIX));
        assert!(body.contains(fname));
        // The container path is never embedded: the body carries a host path.
        assert!(!body.contains("/workspace/"));
    }

    #[test]
    fn build_send_body_rejects_path_traversal_basename() {
        let temp = tempfile::TempDir::new().unwrap();
        let replica = temp
            .path()
            .join("proj")
            .join(".ac")
            .join("wg-1")
            .join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        let err =
            build_send_body(replica.to_str().unwrap(), "..\\secret.md", "proj:wg-1/x").unwrap_err();
        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn build_send_body_rejects_missing_message_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let wg_root = temp.path().join("proj").join(".ac").join("wg-1");
        let replica = wg_root.join("__agent_x");
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::create_dir_all(wg_root.join("messaging")).unwrap();
        let err = build_send_body(replica.to_str().unwrap(), "nope.md", "proj:wg-1/x").unwrap_err();
        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn read_send_file_content_ingests_utf8_body_without_host_path() {
        let temp = tempfile::TempDir::new().unwrap();
        let wg_root = temp.path().join("proj-x").join(".ac").join("wg-1-team");
        let replica = wg_root.join("__agent_dev-rust");
        let messaging = wg_root.join("messaging");
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::create_dir_all(&messaging).unwrap();
        let fname = "20260704-000000-wg1-a-to-wg1-b-hello.md";
        std::fs::write(messaging.join(fname), "hello body").unwrap();

        let got = read_send_file_content(replica.to_str().unwrap(), fname).unwrap();

        assert_eq!(got.body, "hello body");
        assert_eq!(got.source_ref, fname);
        assert!(!got.body.contains("Process this inter-agent message:"));
    }

    #[test]
    fn read_send_file_content_rejects_oversize_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let wg_root = temp.path().join("proj-x").join(".ac").join("wg-1-team");
        let replica = wg_root.join("__agent_dev-rust");
        let messaging = wg_root.join("messaging");
        std::fs::create_dir_all(&replica).unwrap();
        std::fs::create_dir_all(&messaging).unwrap();
        let fname = "20260704-000000-wg1-a-to-wg1-b-large.md";
        std::fs::write(messaging.join(fname), vec![b'x'; INLINE_BODY_MAX_BYTES + 1]).unwrap();

        let err = read_send_file_content(replica.to_str().unwrap(), fname).unwrap_err();

        assert_eq!(err.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    }
}
