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
#[cfg(test)]
fn build_send_body(bound_root: &str, basename: &str, from: &str) -> Result<String, ApiError> {
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
    pub archive_warning: Option<String>,
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
    let archive_warning = (|| -> Result<(), String> {
        let room = crate::phone::messaging::workgroup_root(Path::new(bound_root))
            .map_err(|e| e.to_string())?;
        crate::phone::messaging::ensure_workgroup_root_is_authoritative(&room)?;
        let ac_root = room.parent().ok_or("room has no Project AC Root")?;
        crate::phone::messaging::archive_message_snapshot(
            ac_root,
            &abs,
            basename,
            body.as_bytes(),
        )?;
        Ok(())
    })()
    .err()
    .map(|error| format!("Message archive warning: {}: {error}", abs.display()));
    Ok(SendFileContent {
        body,
        source_ref: basename.to_string(),
        archive_warning,
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
        let settings = state
            .current_read()
            .await
            .map_err(|error| ApiError::ServiceUnavailable(error.to_string()))?;
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
mod tests;
