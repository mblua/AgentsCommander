use crate::session::manager::SessionManager;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::sync::Arc;

pub(crate) fn validate_delete_root_not_link_or_reparse(path: &Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("delete_root_not_directory".to_string());
        }
        Err(e) => return Err(format!("delete_root_metadata_failed: {e}")),
    };

    if delete_root_has_windows_reparse_point(&metadata) {
        return Err("delete_root_is_reparse_point".to_string());
    }
    if metadata_is_link_or_reparse(&metadata) {
        return Err("delete_root_is_symlink".to_string());
    }
    if !metadata.is_dir() {
        return Err("delete_root_not_directory".to_string());
    }
    Ok(())
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
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
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

pub(super) const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

pub(super) fn metadata_parts_are_link_or_reparse(is_symlink: bool, file_attributes: u32) -> bool {
    is_symlink || file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

pub(crate) fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        metadata_parts_are_link_or_reparse(
            metadata.file_type().is_symlink(),
            metadata.file_attributes(),
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        metadata_parts_are_link_or_reparse(metadata.file_type().is_symlink(), 0)
    }
}

#[cfg(target_os = "windows")]
pub(super) fn delete_root_has_windows_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata_parts_are_link_or_reparse(false, metadata.file_attributes())
}

#[cfg(not(target_os = "windows"))]
pub(super) fn delete_root_has_windows_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// True iff the rename-probe error indicates a blocker holds an open handle.
///
/// Superset of `is_file_in_use_error`: matches the same {32, 33, 1224} codes
/// PLUS `ERROR_ACCESS_DENIED` (5). Empirically `MoveFileEx` (and therefore
/// `std::fs::rename`) returns 5, not 32, when an existing open handle on the
/// source's descendant lacks `FILE_SHARE_DELETE` — the most common real-world
/// blocker shape (default-share opens by IDEs and terminals). This is the
/// rename-path counterpart to `is_file_in_use_error`, which was tuned for
/// `remove_dir_all` semantics where ACCESS_DENIED typically means a real
/// permission failure (read-only file) rather than a share-mode mismatch.
///
/// `pub(crate)` so unit tests can exercise it without going through `try_atomic_delete_wg`.
pub(crate) fn is_rename_blocked_by_handle(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        const ERROR_ACCESS_DENIED: i32 = 5;
        if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) {
            return true;
        }
        is_file_in_use_error(e)
    }
    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}

/// True iff `e` represents a Windows "file in use" error.
///
/// Matches the Win32 codes that surface when another process holds an open or
/// memory-mapped handle to a file we tried to delete:
/// - `ERROR_SHARING_VIOLATION` (32) — standard open with a deny-share mode.
/// - `ERROR_LOCK_VIOLATION` (33) — byte-range lock collision.
/// - `ERROR_USER_MAPPED_FILE` (1224) — file is mapped into another process's address
///   space. This is the VSCode / IDE memory-mapped-I/O case and was the motivating
///   real-world scenario for the blocker diagnostic. See plan §6.1.
///
/// On non-Windows always returns false: Linux / macOS produce different error codes
/// for "directory not empty due to open file" and we don't run the Restart-Manager
/// diagnostic there.
///
/// `pub(crate)` so the unit test in `wg_delete_diagnostic::tests` can exercise it
/// without moving the test into this module.
#[cfg(any(windows, test))]
pub(crate) fn is_file_in_use_error(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        const ERROR_SHARING_VIOLATION: i32 = 32;
        const ERROR_LOCK_VIOLATION: i32 = 33;
        const ERROR_USER_MAPPED_FILE: i32 = 1224;
        matches!(
            e.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION | ERROR_USER_MAPPED_FILE)
        )
    }
    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}
