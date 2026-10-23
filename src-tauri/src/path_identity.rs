//! Narrow filesystem identity primitives for privileged PTY input.
//!
//! These helpers do not repair configuration. They reject link/reparse
//! components, bind checks to stable filesystem object IDs, and cap every
//! security-bearing read.

use std::fs::File;
use std::fs::Metadata;
use std::fs::OpenOptions;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

mod json;
mod regular;
mod retained;
mod terminal_output;

pub use json::parse_json_no_duplicates;
pub use json::scan_json_document;
#[cfg(all(test, windows))]
use regular::atomic_replace_existing;
pub use regular::is_verified_descendant;
use regular::open_read_no_follow;
#[cfg(not(unix))]
pub use regular::opened_file_is_delete_pending;
pub use regular::paths_equivalent;
pub use regular::publish_new_file_atomic;
pub use regular::read_bounded_regular;
#[cfg(test)]
use regular::read_bounded_regular_inner;
pub use regular::remove_regular_file_if_same;
#[cfg(test)]
use regular::remove_regular_file_if_same_inner;
#[cfg(all(test, windows))]
use regular::remove_windows_regular_file_by_handle_inner;
pub use regular::replace_regular_file_atomic;
pub use regular::same_object;
pub use regular::sync_parent_best_effort;
pub use terminal_output::create_terminal_snapshot_output;
#[cfg(test)]
use terminal_output::create_terminal_snapshot_output_inner;
#[cfg(test)]
use terminal_output::create_terminal_snapshot_output_with_hooks;
pub use terminal_output::validate_terminal_snapshot_output_path;
#[cfg(all(test, windows))]
use terminal_output::validate_terminal_snapshot_output_path_inner;
#[cfg(all(test, windows))]
use terminal_output::validate_windows_snapshot_output_path;

#[cfg(any(target_os = "linux", target_os = "android"))]
const UNIX_O_NOFOLLOW: i32 = 0x0002_0000;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const UNIX_O_NOFOLLOW: i32 = 0x0000_0100;

// Only `child_is_absent` reaches this, and off Windows that helper survives
// solely for its own unit tests (see its gate below).
#[cfg(all(unix, test))]
fn unix_child_open_error_is_absent(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENOENT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileObjectId {
    pub volume: u64,
    pub file: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedMetadata {
    pub is_dir: bool,
    pub is_file: bool,
    pub len: u64,
    pub links: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPathIdentity {
    pub canonical_path: PathBuf,
    pub object_id: FileObjectId,
    pub metadata: VerifiedMetadata,
    pub content_sha256: Option<[u8; 32]>,
}

/// A verified directory retained by filesystem-object handle while a privileged
/// child operation is in progress.
#[derive(Clone)]
pub struct RetainedDirectory {
    identity: VerifiedPathIdentity,
    #[cfg(unix)]
    retained_path: PathBuf,
    handle: std::sync::Arc<File>,
}

#[cfg(unix)]
#[derive(Clone)]
pub(crate) struct RetainedUnixFileWitness {
    handle: std::sync::Arc<File>,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnixFileWitnessState {
    Linked,
    Unlinked,
    Uncertain,
}

#[cfg(unix)]
// The `Before` prefix is the contract, not noise: each variant names the moment
// the cleanup hook fires, which is BEFORE the step it is named after. Renaming to
// `Restore` / `ClaimRename` / `ClaimUnlink` would name the steps instead and lose
// that. clippy's shared-prefix heuristic cannot distinguish the two cases.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnixTrackedCleanupStage {
    BeforeClaimRename,
    BeforeRestore,
    BeforeClaimUnlink,
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnixTrackedCleanupOutcome {
    Removed,
    AlreadyAbsent,
    SourceRetained,
    ClaimRetained {
        path: PathBuf,
        identity: VerifiedPathIdentity,
    },
    Uncertain,
}

#[cfg(unix)]
impl RetainedUnixFileWitness {
    pub(crate) fn state(&self, expected: &VerifiedPathIdentity) -> UnixFileWitnessState {
        let Ok(metadata) = self.handle.metadata() else {
            return UnixFileWitnessState::Uncertain;
        };
        let Ok((object_id, links)) = handle_identity(&self.handle) else {
            return UnixFileWitnessState::Uncertain;
        };
        if !metadata.is_file() || is_link_or_reparse(&metadata) || object_id != expected.object_id {
            return UnixFileWitnessState::Uncertain;
        }
        match links {
            0 => UnixFileWitnessState::Unlinked,
            1 => UnixFileWitnessState::Linked,
            _ => UnixFileWitnessState::Uncertain,
        }
    }

    pub(crate) fn matches(
        &self,
        expected: &VerifiedPathIdentity,
        observed: &VerifiedPathIdentity,
    ) -> bool {
        self.state(expected) == UnixFileWitnessState::Linked
            && observed.object_id == expected.object_id
    }
}

fn link_flags_indicate_link_or_reparse(is_symlink: bool, attributes: u32) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    is_symlink || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn is_link_or_reparse(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    let attributes = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
    };
    #[cfg(not(windows))]
    let attributes = 0;
    link_flags_indicate_link_or_reparse(metadata.file_type().is_symlink(), attributes)
}

#[cfg(unix)]
fn handle_identity(file: &File) -> Result<(FileObjectId, u64), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(|_| "unsafe_path".to_string())?;
    Ok((
        FileObjectId {
            volume: metadata.dev(),
            file: metadata.ino(),
        },
        metadata.nlink(),
    ))
}

#[cfg(windows)]
fn handle_identity(file: &File) -> Result<(FileObjectId, u64), String> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err("unsafe_path".to_string());
    }
    let information = unsafe { information.assume_init() };
    Ok((
        FileObjectId {
            volume: u64::from(information.dwVolumeSerialNumber),
            file: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
        },
        u64::from(information.nNumberOfLinks),
    ))
}

#[cfg(not(any(unix, windows)))]
fn handle_identity(file: &File) -> Result<(FileObjectId, u64), String> {
    let metadata = file.metadata().map_err(|_| "unsafe_path".to_string())?;
    Ok((
        FileObjectId {
            volume: 0,
            file: metadata.len(),
        },
        1,
    ))
}

fn snapshot(metadata: &Metadata, links: u64) -> VerifiedMetadata {
    VerifiedMetadata {
        is_dir: metadata.is_dir(),
        is_file: metadata.is_file(),
        len: metadata.len(),
        links,
    }
}

/// Which identity rule rejected a path. The vocabulary is closed and every
/// variant renders to a compile-time snake_case literal, so no filesystem text
/// can ever escape through a rule name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRule {
    ParentComponent,
    ComponentUnreadable,
    ComponentIsLink,
    NotADirectory,
    OpenFailed,
    HandleIdentityUnavailable,
    CanonicalizeFailed,
    IdentityChangedDuringCheck,
}

impl PathRule {
    pub const fn as_str(self) -> &'static str {
        match self {
            PathRule::ParentComponent => "parent_component",
            PathRule::ComponentUnreadable => "component_unreadable",
            PathRule::ComponentIsLink => "component_is_link",
            PathRule::NotADirectory => "not_a_directory",
            PathRule::OpenFailed => "open_failed",
            PathRule::HandleIdentityUnavailable => "handle_identity_unavailable",
            PathRule::CanonicalizeFailed => "canonicalize_failed",
            PathRule::IdentityChangedDuringCheck => "identity_changed_during_check",
        }
    }
}

/// Inspect every existing path component without following a link/reparse
/// component. The path itself must exist.
pub fn verify_component_chain(path: &Path) -> Result<(), String> {
    verify_component_chain_reason(path).map_err(|_| "unsafe_path".to_string())
}

/// Same checks as [`verify_component_chain`], reporting which rule rejected the
/// path instead of collapsing every rejection to `unsafe_path`.
pub fn verify_component_chain_reason(path: &Path) -> Result<(), PathRule> {
    verify_component_chain_with_inspector(path, inspect_component_link)
}

fn inspect_component_link(path: &Path) -> Result<bool, PathRule> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| PathRule::ComponentUnreadable)?;
    Ok(is_link_or_reparse(&metadata))
}

fn verify_component_chain_with_inspector(
    path: &Path,
    mut inspect: impl FnMut(&Path) -> Result<bool, PathRule>,
) -> Result<(), PathRule> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                current.push(component.as_os_str());
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(PathRule::ParentComponent);
            }
            Component::Normal(part) => current.push(part),
        }
        if inspect(&current)? {
            return Err(PathRule::ComponentIsLink);
        }
    }
    Ok(())
}

pub fn verify_directory(path: &Path) -> Result<VerifiedPathIdentity, String> {
    verify_directory_reason(path).map_err(|_| "unsafe_path".to_string())
}

/// Same checks as [`verify_directory`], reporting which rule rejected the path
/// instead of collapsing every rejection to `unsafe_path`.
pub fn verify_directory_reason(path: &Path) -> Result<VerifiedPathIdentity, PathRule> {
    verify_directory_with_inspector(path, inspect_component_link)
}

fn verify_directory_with_inspector(
    path: &Path,
    inspect: impl FnMut(&Path) -> Result<bool, PathRule>,
) -> Result<VerifiedPathIdentity, PathRule> {
    verify_component_chain_with_inspector(path, inspect)?;
    let entry_before =
        std::fs::symlink_metadata(path).map_err(|_| PathRule::ComponentUnreadable)?;
    if !entry_before.is_dir() || is_link_or_reparse(&entry_before) {
        return Err(PathRule::NotADirectory);
    }
    let opened = open_directory_no_follow(path).map_err(|_| PathRule::OpenFailed)?;
    let opened_metadata = opened
        .metadata()
        .map_err(|_| PathRule::HandleIdentityUnavailable)?;
    if !opened_metadata.is_dir() || is_link_or_reparse(&opened_metadata) {
        return Err(PathRule::NotADirectory);
    }
    let (opened_id, links) =
        handle_identity(&opened).map_err(|_| PathRule::HandleIdentityUnavailable)?;
    let canonical_path = std::fs::canonicalize(path).map_err(|_| PathRule::CanonicalizeFailed)?;
    let entry_after = std::fs::symlink_metadata(path).map_err(|_| PathRule::ComponentUnreadable)?;
    if !entry_after.is_dir() || is_link_or_reparse(&entry_after) {
        return Err(PathRule::NotADirectory);
    }
    let reopened = open_directory_no_follow(path).map_err(|_| PathRule::OpenFailed)?;
    let (reopened_id, _) =
        handle_identity(&reopened).map_err(|_| PathRule::HandleIdentityUnavailable)?;
    if reopened_id != opened_id {
        return Err(PathRule::IdentityChangedDuringCheck);
    }
    Ok(VerifiedPathIdentity {
        canonical_path,
        object_id: opened_id,
        metadata: snapshot(&opened_metadata, links),
        content_sha256: None,
    })
}

fn open_directory_no_follow(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(UNIX_O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
            FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path).map_err(|_| "unsafe_path".to_string())
}

fn open_retained_directory(path: &Path, share_write: bool) -> Result<File, String> {
    #[cfg(not(windows))]
    let _ = share_write;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(UNIX_O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };
        let share_mode = if share_write {
            FILE_SHARE_READ | FILE_SHARE_WRITE
        } else {
            FILE_SHARE_READ
        };
        options
            .share_mode(share_mode)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path).map_err(|_| "unsafe_path".to_string())
}

pub fn retain_directory(path: &Path) -> Result<RetainedDirectory, String> {
    retain_directory_inner(path, true)
}

fn retain_immutable_directory(path: &Path) -> Result<RetainedDirectory, String> {
    retain_directory_inner(path, false)
}

fn retain_directory_inner(path: &Path, share_write: bool) -> Result<RetainedDirectory, String> {
    let before = verify_directory(path)?;
    let handle = open_retained_directory(path, share_write)?;
    let metadata = handle.metadata().map_err(|_| "unsafe_path".to_string())?;
    let (object_id, _) = handle_identity(&handle)?;
    if !metadata.is_dir() || is_link_or_reparse(&metadata) || object_id != before.object_id {
        return Err("unsafe_path".to_string());
    }
    let current = verify_directory(path)?;
    if !same_object(&before, &current) {
        return Err("unsafe_path".to_string());
    }
    Ok(RetainedDirectory {
        identity: current,
        #[cfg(unix)]
        retained_path: path.to_path_buf(),
        handle: std::sync::Arc::new(handle),
    })
}

pub fn verify_regular_file(path: &Path) -> Result<VerifiedPathIdentity, String> {
    let file = open_read_no_follow(path)?;
    verify_opened_regular_file(path, &file, false)
}

pub fn verify_opened_regular_file(
    path: &Path,
    file: &File,
    require_empty: bool,
) -> Result<VerifiedPathIdentity, String> {
    let parent = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    verify_component_chain(parent)?;
    let entry = std::fs::symlink_metadata(path).map_err(|_| "unsafe_path".to_string())?;
    if !entry.is_file() || is_link_or_reparse(&entry) {
        return Err("unsafe_path".to_string());
    }
    let metadata = file.metadata().map_err(|_| "unsafe_path".to_string())?;
    let (opened_id, links) = handle_identity(file)?;
    if !metadata.is_file()
        || is_link_or_reparse(&metadata)
        || links != 1
        || (require_empty && metadata.len() != 0)
    {
        return Err("unsafe_path".to_string());
    }
    let reopened = open_read_no_follow(path)?;
    let (reopened_id, reopened_links) = handle_identity(&reopened)?;
    if reopened_id != opened_id || reopened_links != 1 {
        return Err("unsafe_path".to_string());
    }
    Ok(VerifiedPathIdentity {
        canonical_path: std::fs::canonicalize(path).map_err(|_| "unsafe_path".to_string())?,
        object_id: opened_id,
        metadata: snapshot(&metadata, links),
        content_sha256: None,
    })
}

#[cfg(test)]
mod tests;
