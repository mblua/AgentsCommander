//! Physical entity deletion; excludes lifecycle state and locks.
use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn validate_delete_root(path: &Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("delete_root_not_directory".to_string());
        }
        Err(e) => return Err(format!("delete_root_metadata_failed: {e}")),
    };

    validate_delete_root_parts(
        delete_root_has_windows_reparse_point(&metadata),
        metadata_is_link_or_reparse(&metadata),
        metadata.is_dir(),
    )
}

pub(crate) fn validate_delete_root_parts(
    is_reparse: bool,
    is_symlink: bool,
    is_dir: bool,
) -> Result<(), String> {
    if is_reparse {
        return Err("delete_root_is_reparse_point".to_string());
    }
    if is_symlink {
        return Err("delete_root_is_symlink".to_string());
    }
    if !is_dir {
        return Err("delete_root_not_directory".to_string());
    }
    Ok(())
}

pub(crate) const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

pub(crate) fn metadata_parts_are_link_or_reparse(is_symlink: bool, file_attributes: u32) -> bool {
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
pub(crate) fn delete_root_has_windows_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata_parts_are_link_or_reparse(false, metadata.file_attributes())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn delete_root_has_windows_reparse_point(_metadata: &std::fs::Metadata) -> bool {
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

/// Typed outcome of removing one replica directory (#1063, plan section 5.4).
///
/// Replaces the prior bare `Ok(())` so lifecycle callers can distinguish a real
/// removal from a proven absence and prune manifest rows only for `Removed` or an
/// explicit `AlreadyAbsent`. `AlreadyAbsent` is constructed only from an exact
/// `symlink_metadata`/remove `ErrorKind::NotFound`; it never uses `Path::exists`,
/// which collapses access and other inspection errors to false. A plain
/// discovery `NotFound` observed elsewhere never reaches this typed path and
/// therefore never prunes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplicaRemovalOutcome {
    Removed,
    AlreadyAbsent,
    Failed(String),
}

/// Result of a preflight-rename `delete_workgroup` attempt.
///
/// `pub(crate)` so the unit tests can pattern-match on the variants.
pub(crate) enum WgDeleteOutcome {
    /// Rename succeeded and the renamed dir was removed.
    Deleted,
    /// Rename succeeded, but deleting the renamed orphan failed. The
    /// user-visible workgroup name is gone, but callers must report this as a
    /// partial failure and skip refresh.
    Partial {
        orphan_path: PathBuf,
        error: std::io::Error,
    },
    /// Rename failed with a Windows file-in-use error. Tree is intact; caller
    /// should run the blocker diagnostic and return `BLOCKERS:` to the frontend.
    Blocked(std::io::Error),
    /// Rename failed with any other error (NotFound, permission, invalid path,
    /// …). Caller passes the raw error through unchanged.
    Other(std::io::Error),
}

impl WgDeleteOutcome {
    /// Whether the original user-visible workgroup path is gone (#1063, plan
    /// section 5.4). `true` for `Deleted` and `Partial` (the atomic rename moved
    /// the original path away, so its replica config rows are no longer active
    /// even when hidden-orphan cleanup failed); `false` for `Blocked`/`Other`,
    /// which leave the path intact and must preserve rows.
    pub(crate) fn logical_path_removed(&self) -> bool {
        matches!(
            self,
            WgDeleteOutcome::Deleted | WgDeleteOutcome::Partial { .. }
        )
    }
}

/// Atomically detect blockers before deleting a workgroup directory.
///
/// Strategy: rename the WG dir to a unique sentinel name in the same parent
/// (NTFS metadata-only operation, fails atomically if any handle blocks it),
/// then `remove_dir_all` the renamed dir. If rename fails with a file-in-use
/// error the WG is still intact, so the caller can run the diagnostic over the
/// original tree and surface a `BLOCKERS:` report.
///
/// Suffix scheme: `.deleting-<wg_name>-<uuid>` — leading `.` keeps any orphan
/// (rare race: rename succeeds but remove_dir_all fails) invisible to the
/// `entity_prefix::has_entity_prefix` filters in `ac_discovery` and
/// `cli::list_peers`, so an orphan won't surface as a ghost room. UUID is
/// used (already in `Cargo.toml`) to guarantee uniqueness across rapid retries.
///
/// `pub(crate)` so unit tests can drive it directly.
pub(crate) fn rename_room_for_delete(
    wg_dir: &Path,
    remove_dir_all: impl FnOnce(&Path) -> std::io::Result<()>,
) -> WgDeleteOutcome {
    let parent = match wg_dir.parent() {
        Some(p) => p,
        None => {
            return WgDeleteOutcome::Other(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "room directory has no parent",
            ));
        }
    };
    let original_name = match wg_dir.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => {
            return WgDeleteOutcome::Other(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "room directory has no filename",
            ));
        }
    };
    let temp_name = format!(".deleting-{}-{}", original_name, uuid::Uuid::new_v4());
    let temp_path = parent.join(&temp_name);

    match std::fs::rename(wg_dir, &temp_path) {
        Ok(()) => {
            if let Err(e) = remove_dir_all(&temp_path) {
                log::warn!(
                    "[entity_creation] Renamed workgroup '{}' to '{}' but remove_dir_all \
                     failed: {}. \
                     User-visible WG is gone; orphan remains on disk.",
                    wg_dir.display(),
                    temp_path.display(),
                    e
                );
                return WgDeleteOutcome::Partial {
                    orphan_path: temp_path,
                    error: e,
                };
            }
            WgDeleteOutcome::Deleted
        }
        Err(e) => {
            if is_rename_blocked_by_handle(&e) {
                WgDeleteOutcome::Blocked(e)
            } else {
                WgDeleteOutcome::Other(e)
            }
        }
    }
}

pub(crate) fn remove_replica_directory(path: &Path) -> ReplicaRemovalOutcome {
    remove_replica_directory_with(
        path,
        |path| std::fs::symlink_metadata(path).map(|_| ()),
        |path| std::fs::remove_dir_all(path),
    )
}

pub(crate) fn remove_replica_directory_with<P, R>(
    path: &Path,
    probe: P,
    remove: R,
) -> ReplicaRemovalOutcome
where
    P: FnOnce(&Path) -> io::Result<()>,
    R: FnOnce(&Path) -> io::Result<()>,
{
    match probe(path) {
        Ok(()) => match remove(path) {
            Ok(()) => ReplicaRemovalOutcome::Removed,
            Err(e) if e.kind() == io::ErrorKind::NotFound => ReplicaRemovalOutcome::AlreadyAbsent,
            Err(e) => {
                ReplicaRemovalOutcome::Failed(format!("Failed to delete replica directory: {}", e))
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => ReplicaRemovalOutcome::AlreadyAbsent,
        Err(e) => {
            ReplicaRemovalOutcome::Failed(format!("Failed to inspect replica directory: {}", e))
        }
    }
}

pub(crate) fn canonical_delete_path_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn inspect_team_for_delete(path: &Path) -> io::Result<()> {
    std::fs::symlink_metadata(path).map(|_| ())
}

pub(crate) fn remove_team_directory(path: &Path) -> Result<(), String> {
    std::fs::remove_dir_all(path).map_err(|e| format!("Failed to delete team directory: {}", e))
}

pub(crate) fn stage_agent_target(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

pub(crate) fn restore_agent_target(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

pub(crate) fn remove_staged_agent_target(path: &Path) -> io::Result<()> {
    std::fs::remove_dir_all(path)
}

pub(crate) fn remove_room_orphan(path: &Path) -> io::Result<()> {
    std::fs::remove_dir_all(path)
}
