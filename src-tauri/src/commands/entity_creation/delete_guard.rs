#[cfg(test)]
pub(crate) use crate::config::entity_creation::directories::is_file_in_use_error;
use crate::session::manager::SessionManager;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::sync::Arc;

#[cfg(test)]
pub(super) use crate::config::entity_creation::directories::FILE_ATTRIBUTE_REPARSE_POINT;
pub(crate) use crate::config::entity_creation::directories::{
    is_rename_blocked_by_handle, metadata_is_link_or_reparse, metadata_parts_are_link_or_reparse,
};

pub(crate) fn validate_delete_root_not_link_or_reparse(path: &Path) -> Result<(), String> {
    crate::config::entity_creation::directories::validate_delete_root(path)
}

pub(super) async fn ensure_no_live_sessions_under_manager(
    root: &Path,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    let root_key = path_key_for_delete(root);
    let manager = { session_mgr.read().await.clone() };
    let sessions = manager.list_sessions().await;
    let blockers: Vec<_> = sessions
        .into_iter()
        .filter(|session| !matches!(session.status, SessionStatus::Exited(_)))
        .filter(|session| {
            let working_dir = Path::new(&session.working_directory);
            let working_key = path_key_for_delete(working_dir);
            working_key == root_key || working_key.starts_with(&(root_key.clone() + "/"))
        })
        .collect();

    if blockers.is_empty() {
        return Ok(());
    }

    let details = blockers
        .iter()
        .map(|session| {
            format!(
                "  - {} at {} ({:?})",
                session.name, session.working_directory, session.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "Cannot delete while live sessions exist under {}:\n{}",
        root.display(),
        details
    ))
}

pub(super) fn path_key_for_delete(path: &Path) -> String {
    let resolved =
        crate::config::entity_creation::directories::canonical_delete_path_or_original(path);
    path_key_for_delete_without_canonicalize(&resolved)
}

pub(super) fn path_key_for_delete_without_canonicalize(path: &Path) -> String {
    let text = crate::path_utils::path_to_string_without_windows_verbatim_prefix(path);
    text.replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

pub(super) fn target_keys_for_delete(path: &Path) -> Vec<String> {
    let canonical_key = path_key_for_delete(path);
    let raw_key = path_key_for_delete_without_canonicalize(path);
    if canonical_key == raw_key {
        vec![canonical_key]
    } else {
        vec![canonical_key, raw_key]
    }
}
