//! Narrow filesystem identity primitives for privileged PTY input.
//!
//! These helpers do not repair configuration. They reject link/reparse
//! components, bind checks to stable filesystem object IDs, and cap every
//! security-bearing read.

use std::collections::HashSet;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use sha2::{Digest, Sha256};

#[cfg(any(target_os = "linux", target_os = "android"))]
const UNIX_O_NOFOLLOW: i32 = 0x0002_0000;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
const UNIX_O_NOFOLLOW: i32 = 0x0000_0100;

#[cfg(unix)]
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

impl RetainedDirectory {
    pub fn identity(&self) -> &VerifiedPathIdentity {
        &self.identity
    }

    pub fn verify_current(&self) -> Result<(), String> {
        let metadata = self
            .handle
            .metadata()
            .map_err(|_| "unsafe_path".to_string())?;
        let (object_id, _) = handle_identity(&self.handle)?;
        if !metadata.is_dir()
            || is_link_or_reparse(&metadata)
            || object_id != self.identity.object_id
        {
            return Err("unsafe_path".to_string());
        }
        let current = verify_directory(self.identity.canonical_path.as_path())?;
        if same_object(&current, &self.identity) {
            Ok(())
        } else {
            Err("unsafe_path".to_string())
        }
    }

    pub fn sync_best_effort(&self) {
        let _ = self.handle.sync_all();
    }

    pub fn create_new_private_file(&self, path: &Path) -> Result<File, String> {
        self.create_new_file(path, false)
    }

    pub fn create_new_output_file(&self, path: &Path) -> Result<File, String> {
        self.create_new_file(path, true)
    }

    fn create_new_file(&self, path: &Path, lock_output_leaf: bool) -> Result<File, String> {
        #[cfg(unix)]
        let _ = self.checked_unix_child_name(path)?;
        self.verify_current()?;
        #[cfg(unix)]
        {
            let _ = lock_output_leaf;
            return self
                .open_unix_child(
                    path,
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
                .map_err(|_| "unsafe_path".to_string());
        }
        #[cfg(not(unix))]
        {
            #[cfg(not(windows))]
            let _ = lock_output_leaf;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                use windows_sys::Win32::Storage::FileSystem::{
                    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
                };
                if lock_output_leaf {
                    options.share_mode(FILE_SHARE_READ);
                }
                options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
            }
            options.open(path).map_err(|_| "unsafe_path".to_string())
        }
    }

    pub fn verify_opened_regular_file(
        &self,
        path: &Path,
        file: &File,
        require_empty: bool,
    ) -> Result<VerifiedPathIdentity, String> {
        #[cfg(unix)]
        {
            let _ = self.checked_unix_child_name(path)?;
            let metadata = file.metadata().map_err(|_| "unsafe_path".to_string())?;
            let (opened_id, links) = handle_identity(file)?;
            if !metadata.is_file()
                || is_link_or_reparse(&metadata)
                || links != 1
                || (require_empty && metadata.len() != 0)
            {
                return Err("unsafe_path".to_string());
            }
            let reopened = self
                .open_unix_child(
                    path,
                    libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0,
                )
                .map_err(|_| "unsafe_path".to_string())?;
            let (reopened_id, reopened_links) = handle_identity(&reopened)?;
            if reopened_id != opened_id || reopened_links != 1 {
                return Err("unsafe_path".to_string());
            }
            return Ok(VerifiedPathIdentity {
                canonical_path: self.canonical_child_path(path)?,
                object_id: opened_id,
                metadata: snapshot(&metadata, links),
                content_sha256: None,
            });
        }
        #[cfg(not(unix))]
        {
            self.verify_current()?;
            verify_opened_regular_file(path, file, require_empty)
        }
    }

    pub fn verify_regular_file(&self, path: &Path) -> Result<VerifiedPathIdentity, String> {
        #[cfg(unix)]
        {
            let file = self
                .open_unix_child(
                    path,
                    libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0,
                )
                .map_err(|_| "unsafe_path".to_string())?;
            return self.verify_opened_regular_file(path, &file, false);
        }
        #[cfg(not(unix))]
        {
            self.verify_current()?;
            verify_regular_file(path)
        }
    }

    #[cfg(unix)]
    pub(crate) fn retain_unix_file_witness(
        &self,
        path: &Path,
        expected: &VerifiedPathIdentity,
    ) -> Result<RetainedUnixFileWitness, String> {
        let (witness, observed) = self.open_unix_file_witness(path)?;
        if !witness.matches(expected, &observed) {
            return Err("unsafe_path".to_string());
        }
        Ok(witness)
    }

    #[cfg(unix)]
    pub(crate) fn cleanup_unix_tracked_file(
        &self,
        path: &Path,
        expected: &VerifiedPathIdentity,
        witness: &RetainedUnixFileWitness,
    ) -> UnixTrackedCleanupOutcome {
        self.cleanup_unix_tracked_file_inner(path, expected, witness, None, |_, _, _| {})
    }

    #[cfg(all(unix, test))]
    pub(crate) fn cleanup_unix_tracked_file_with_hook(
        &self,
        path: &Path,
        expected: &VerifiedPathIdentity,
        witness: &RetainedUnixFileWitness,
        claim_leaf: Option<&std::ffi::OsStr>,
        hook: impl FnMut(UnixTrackedCleanupStage, &Path, &Path),
    ) -> UnixTrackedCleanupOutcome {
        self.cleanup_unix_tracked_file_inner(path, expected, witness, claim_leaf, hook)
    }

    #[cfg(unix)]
    fn cleanup_unix_tracked_file_inner(
        &self,
        path: &Path,
        expected: &VerifiedPathIdentity,
        witness: &RetainedUnixFileWitness,
        claim_leaf: Option<&std::ffi::OsStr>,
        mut hook: impl FnMut(UnixTrackedCleanupStage, &Path, &Path),
    ) -> UnixTrackedCleanupOutcome {
        if self.checked_unix_child_name(path).is_err() {
            return UnixTrackedCleanupOutcome::Uncertain;
        }
        if witness.state(expected) == UnixFileWitnessState::Unlinked {
            return UnixTrackedCleanupOutcome::AlreadyAbsent;
        }
        if witness.state(expected) != UnixFileWitnessState::Linked {
            return UnixTrackedCleanupOutcome::Uncertain;
        }
        let Ok(source) = self.verify_regular_file(path) else {
            return if witness.state(expected) == UnixFileWitnessState::Unlinked {
                UnixTrackedCleanupOutcome::AlreadyAbsent
            } else {
                UnixTrackedCleanupOutcome::Uncertain
            };
        };
        if !witness.matches(expected, &source) {
            return UnixTrackedCleanupOutcome::Uncertain;
        }

        let generated_claim = format!(
            ".{}.terminal-snapshot-private-cleanup",
            uuid::Uuid::new_v4()
        );
        let claim_leaf = claim_leaf.unwrap_or_else(|| std::ffi::OsStr::new(&generated_claim));
        let Ok(claim_path) = self.unix_cleanup_claim_path(path, claim_leaf) else {
            return UnixTrackedCleanupOutcome::Uncertain;
        };

        hook(
            UnixTrackedCleanupStage::BeforeClaimRename,
            path,
            &claim_path,
        );
        if !self.rename_unix_child_no_clobber(path, &claim_path) {
            return match witness.state(expected) {
                UnixFileWitnessState::Unlinked => UnixTrackedCleanupOutcome::AlreadyAbsent,
                UnixFileWitnessState::Linked => match self.verify_regular_file(path) {
                    Ok(current) if witness.matches(expected, &current) => {
                        UnixTrackedCleanupOutcome::SourceRetained
                    }
                    _ => UnixTrackedCleanupOutcome::Uncertain,
                },
                UnixFileWitnessState::Uncertain => UnixTrackedCleanupOutcome::Uncertain,
            };
        }

        let claimed = self.verify_regular_file(&claim_path);
        if !claimed
            .as_ref()
            .is_ok_and(|current| witness.matches(expected, current))
        {
            if witness.state(expected) == UnixFileWitnessState::Unlinked {
                return UnixTrackedCleanupOutcome::AlreadyAbsent;
            }
            if let Ok((unexpected_witness, unexpected_identity)) =
                self.open_unix_file_witness(&claim_path)
            {
                if unexpected_identity.object_id != expected.object_id {
                    hook(UnixTrackedCleanupStage::BeforeRestore, path, &claim_path);
                    let claim_is_continuous =
                        self.verify_regular_file(&claim_path).is_ok_and(|current| {
                            unexpected_witness.matches(&unexpected_identity, &current)
                        });
                    if claim_is_continuous && self.rename_unix_child_no_clobber(&claim_path, path) {
                        let _ = self.verify_regular_file(path).is_ok_and(|current| {
                            unexpected_witness.matches(&unexpected_identity, &current)
                        });
                    }
                }
            }
            return UnixTrackedCleanupOutcome::Uncertain;
        }

        hook(
            UnixTrackedCleanupStage::BeforeClaimUnlink,
            path,
            &claim_path,
        );
        let final_claim = match self.verify_regular_file(&claim_path) {
            Ok(current) if witness.matches(expected, &current) => current,
            _ => {
                return if witness.state(expected) == UnixFileWitnessState::Unlinked {
                    UnixTrackedCleanupOutcome::AlreadyAbsent
                } else {
                    UnixTrackedCleanupOutcome::Uncertain
                };
            }
        };
        let Ok(claim_name) = self.unix_child_cstring(&claim_path) else {
            return UnixTrackedCleanupOutcome::Uncertain;
        };
        use std::os::fd::AsRawFd;
        let removed =
            unsafe { libc::unlinkat(self.handle.as_raw_fd(), claim_name.as_ptr(), 0) } == 0;
        match witness.state(expected) {
            UnixFileWitnessState::Unlinked if removed => UnixTrackedCleanupOutcome::Removed,
            UnixFileWitnessState::Unlinked => UnixTrackedCleanupOutcome::AlreadyAbsent,
            UnixFileWitnessState::Linked => match self.verify_regular_file(&claim_path) {
                Ok(current) if witness.matches(expected, &current) => {
                    UnixTrackedCleanupOutcome::ClaimRetained {
                        path: claim_path,
                        identity: current,
                    }
                }
                _ => {
                    let _ = final_claim;
                    UnixTrackedCleanupOutcome::Uncertain
                }
            },
            UnixFileWitnessState::Uncertain => UnixTrackedCleanupOutcome::Uncertain,
        }
    }

    pub fn child_is_absent(&self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            return match self.open_unix_child(
                path,
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0,
            ) {
                Ok(_) => false,
                Err(error) => unix_child_open_error_is_absent(&error),
            };
        }
        #[cfg(not(unix))]
        {
            self.verify_current().is_ok()
                && std::fs::symlink_metadata(path)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        }
    }

    pub fn publish_new_file_atomic(&self, source: &Path, destination: &Path) -> Result<(), String> {
        #[cfg(unix)]
        {
            let _ = self.checked_unix_child_name(source)?;
            let _ = self.checked_unix_child_name(destination)?;
        }
        self.verify_current()
            .map_err(|_| "atomic_publish_failed".to_string())?;
        #[cfg(target_os = "linux")]
        {
            const RENAME_NOREPLACE: libc::c_uint = libc::RENAME_NOREPLACE;
            let source = self.unix_child_cstring(source)?;
            let destination = self.unix_child_cstring(destination)?;
            use std::os::fd::AsRawFd;
            let directory = self.handle.as_raw_fd();
            let result = unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    directory,
                    source.as_ptr(),
                    directory,
                    destination.as_ptr(),
                    RENAME_NOREPLACE,
                )
            };
            return if result == 0 {
                Ok(())
            } else {
                Err("atomic_publish_failed".to_string())
            };
        }
        #[cfg(target_os = "macos")]
        {
            const RENAME_EXCL: libc::c_uint = libc::RENAME_EXCL;
            let source = self.unix_child_cstring(source)?;
            let destination = self.unix_child_cstring(destination)?;
            unsafe extern "C" {
                fn renameatx_np(
                    old_dir_fd: i32,
                    old_path: *const std::ffi::c_char,
                    new_dir_fd: i32,
                    new_path: *const std::ffi::c_char,
                    flags: u32,
                ) -> i32;
            }
            use std::os::fd::AsRawFd;
            let directory = self.handle.as_raw_fd();
            let result = unsafe {
                renameatx_np(
                    directory,
                    source.as_ptr(),
                    directory,
                    destination.as_ptr(),
                    RENAME_EXCL,
                )
            };
            return if result == 0 {
                Ok(())
            } else {
                Err("atomic_publish_failed".to_string())
            };
        }
        #[cfg(windows)]
        {
            publish_new_file_atomic(source, destination)
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = (source, destination);
            Err("atomic_publish_unsupported".to_string())
        }
    }

    pub fn remove_regular_file_if_same(
        &self,
        path: &Path,
        expected: &VerifiedPathIdentity,
    ) -> bool {
        #[cfg(unix)]
        {
            let Ok(witness) = self.retain_unix_file_witness(path, expected) else {
                return false;
            };
            return matches!(
                self.cleanup_unix_tracked_file(path, expected, &witness),
                UnixTrackedCleanupOutcome::Removed | UnixTrackedCleanupOutcome::AlreadyAbsent
            );
        }
        #[cfg(windows)]
        {
            remove_windows_regular_file_by_handle(path, expected)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let Ok(current) = verify_regular_file(path) else {
                return std::fs::symlink_metadata(path)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
            };
            same_object(expected, &current) && std::fs::remove_file(path).is_ok()
        }
    }

    #[cfg(unix)]
    fn open_unix_file_witness(
        &self,
        path: &Path,
    ) -> Result<(RetainedUnixFileWitness, VerifiedPathIdentity), String> {
        let file = self
            .open_unix_child(
                path,
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0,
            )
            .map_err(|_| "unsafe_path".to_string())?;
        let identity = self.verify_opened_regular_file(path, &file, false)?;
        Ok((
            RetainedUnixFileWitness {
                handle: std::sync::Arc::new(file),
            },
            identity,
        ))
    }

    #[cfg(unix)]
    fn unix_cleanup_claim_path(
        &self,
        source: &Path,
        claim_leaf: &std::ffi::OsStr,
    ) -> Result<PathBuf, String> {
        use std::os::unix::ffi::OsStrExt;

        let _ = self.checked_unix_child_name(source)?;
        if claim_leaf.is_empty() || source.file_name() == Some(claim_leaf) {
            return Err("unsafe_path".to_string());
        }
        let mut components = Path::new(claim_leaf).components();
        if !matches!(components.next(), Some(Component::Normal(name)) if name == claim_leaf)
            || components.next().is_some()
            || std::ffi::CString::new(claim_leaf.as_bytes()).is_err()
        {
            return Err("unsafe_path".to_string());
        }
        Ok(source.with_file_name(claim_leaf))
    }

    #[cfg(unix)]
    fn rename_unix_child_no_clobber(&self, source: &Path, destination: &Path) -> bool {
        let Ok(source) = self.unix_child_cstring(source) else {
            return false;
        };
        let Ok(destination) = self.unix_child_cstring(destination) else {
            return false;
        };
        use std::os::fd::AsRawFd;
        let directory = self.handle.as_raw_fd();

        #[cfg(target_os = "linux")]
        {
            const RENAME_NOREPLACE: libc::c_uint = libc::RENAME_NOREPLACE;
            return unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    directory,
                    source.as_ptr(),
                    directory,
                    destination.as_ptr(),
                    RENAME_NOREPLACE,
                ) == 0
            };
        }
        #[cfg(target_os = "macos")]
        {
            const RENAME_EXCL: libc::c_uint = libc::RENAME_EXCL;
            unsafe extern "C" {
                fn renameatx_np(
                    old_dir_fd: i32,
                    old_path: *const std::ffi::c_char,
                    new_dir_fd: i32,
                    new_path: *const std::ffi::c_char,
                    flags: u32,
                ) -> i32;
            }
            return unsafe {
                renameatx_np(
                    directory,
                    source.as_ptr(),
                    directory,
                    destination.as_ptr(),
                    RENAME_EXCL,
                ) == 0
            };
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (directory, source, destination);
            false
        }
    }

    #[cfg(unix)]
    fn checked_unix_child_name<'a>(&self, path: &'a Path) -> Result<&'a std::ffi::OsStr, String> {
        let parent = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
        let retained_parent = if parent == self.retained_path {
            self.retained_path.as_path()
        } else if parent == self.identity.canonical_path {
            self.identity.canonical_path.as_path()
        } else {
            return Err("unsafe_path".to_string());
        };
        let relative = path
            .strip_prefix(retained_parent)
            .map_err(|_| "unsafe_path".to_string())?;
        let mut components = relative.components();
        let name = match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) if !name.is_empty() => name,
            _ => return Err("unsafe_path".to_string()),
        };
        use std::os::unix::ffi::OsStrExt;
        if name.as_bytes().contains(&0) {
            return Err("unsafe_path".to_string());
        }
        Ok(name)
    }

    #[cfg(unix)]
    fn canonical_child_path(&self, path: &Path) -> Result<PathBuf, String> {
        let name = self.checked_unix_child_name(path)?;
        Ok(self.identity.canonical_path.join(name))
    }

    #[cfg(unix)]
    fn unix_child_cstring(&self, path: &Path) -> Result<std::ffi::CString, String> {
        use std::os::unix::ffi::OsStrExt;
        let name = self.checked_unix_child_name(path)?;
        std::ffi::CString::new(name.as_bytes()).map_err(|_| "unsafe_path".to_string())
    }

    #[cfg(unix)]
    fn open_unix_child(&self, path: &Path, flags: i32, mode: u32) -> std::io::Result<File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = self
            .unix_child_cstring(path)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let descriptor = unsafe {
            // Same variadic promotion rule as the helper's `open_snapshot_leaf_at`: `mode_t` is
            // `u16` on macOS, and an unpromoted `u16` to a variadic function is E0617. This site
            // has never been compiled on macOS because the portable job builds only
            // `session-bridge` and `terminal-snapshot-renderer`, so it would have failed the
            // first time anything built `src-tauri` there.
            libc::openat(
                self.handle.as_raw_fd(),
                name.as_ptr(),
                flags,
                mode as libc::c_uint,
            )
        };
        if descriptor < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(descriptor) })
        }
    }
}

fn is_link_or_reparse(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
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

/// Inspect every existing path component without following a link/reparse
/// component. The path itself must exist.
pub fn verify_component_chain(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                current.push(component.as_os_str());
                continue;
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err("unsafe_path".to_string());
            }
            Component::Normal(part) => current.push(part),
        }
        let metadata =
            std::fs::symlink_metadata(&current).map_err(|_| "unsafe_path".to_string())?;
        if is_link_or_reparse(&metadata) {
            return Err("unsafe_path".to_string());
        }
    }
    Ok(())
}

pub fn verify_directory(path: &Path) -> Result<VerifiedPathIdentity, String> {
    verify_component_chain(path)?;
    let entry_before = std::fs::symlink_metadata(path).map_err(|_| "unsafe_path".to_string())?;
    if !entry_before.is_dir() || is_link_or_reparse(&entry_before) {
        return Err("unsafe_path".to_string());
    }
    let opened = open_directory_no_follow(path)?;
    let opened_metadata = opened.metadata().map_err(|_| "unsafe_path".to_string())?;
    if !opened_metadata.is_dir() || is_link_or_reparse(&opened_metadata) {
        return Err("unsafe_path".to_string());
    }
    let (opened_id, links) = handle_identity(&opened)?;
    let canonical_path = std::fs::canonicalize(path).map_err(|_| "unsafe_path".to_string())?;
    let entry_after = std::fs::symlink_metadata(path).map_err(|_| "unsafe_path".to_string())?;
    if !entry_after.is_dir() || is_link_or_reparse(&entry_after) {
        return Err("unsafe_path".to_string());
    }
    let reopened = open_directory_no_follow(path)?;
    let (reopened_id, _) = handle_identity(&reopened)?;
    if reopened_id != opened_id {
        return Err("unsafe_path".to_string());
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

/// Open one direct child directory below an already retained, verified parent.
///
/// This deliberately does not accept a relative path with multiple components:
/// callers that need a nested layout must retain each component in turn. That
/// keeps the reparse-point and object-identity checks at every boundary rather
/// than turning this primitive into a recursive directory creator.
pub fn open_or_create_verified_child_directory(
    parent: &RetainedDirectory,
    child_name: &std::ffi::OsStr,
) -> Result<RetainedDirectory, String> {
    if child_name.is_empty()
        || Path::new(child_name).components().count() != 1
        || matches!(
            Path::new(child_name).components().next(),
            Some(
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        )
    {
        return Err("unsafe_path".to_string());
    }

    parent.verify_current()?;
    let child = parent.identity().canonical_path.join(child_name);

    match std::fs::symlink_metadata(&child) {
        Ok(metadata) => {
            if !metadata.is_dir() || is_link_or_reparse(&metadata) {
                return Err("unsafe_path".to_string());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&child).map_err(|_| "unsafe_path".to_string())?;
        }
        Err(_) => return Err("unsafe_path".to_string()),
    }

    parent.verify_current()?;
    let retained_child = retain_directory(&child)?;
    parent.verify_current()?;
    Ok(retained_child)
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

fn open_read_no_follow(path: &Path) -> Result<File, String> {
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
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path).map_err(|_| "unsafe_path".to_string())
}

/// Open a regular one-link file, read at most `max_bytes + 1`, and prove that
/// the directory entry still names the opened object afterward.
pub fn read_bounded_regular(
    path: &Path,
    max_bytes: usize,
) -> Result<(Vec<u8>, VerifiedPathIdentity), String> {
    read_bounded_regular_inner(path, max_bytes, || {})
}

fn read_bounded_regular_inner<F>(
    path: &Path,
    max_bytes: usize,
    after_first_snapshot: F,
) -> Result<(Vec<u8>, VerifiedPathIdentity), String>
where
    F: FnOnce(),
{
    let parent = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    verify_component_chain(parent)?;
    let entry_before = std::fs::symlink_metadata(path).map_err(|_| "unsafe_path".to_string())?;
    if !entry_before.is_file() || is_link_or_reparse(&entry_before) {
        return Err("unsafe_path".to_string());
    }

    let mut file = open_read_no_follow(path)?;
    let opened_metadata = file.metadata().map_err(|_| "unsafe_path".to_string())?;
    let (opened_id, opened_links) = handle_identity(&file)?;
    if !opened_metadata.is_file() || is_link_or_reparse(&opened_metadata) || opened_links != 1 {
        return Err("unsafe_path".to_string());
    }

    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024).saturating_add(1));
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "unsafe_path".to_string())?;
    if bytes.len() > max_bytes {
        return Err("capacity_exceeded".to_string());
    }

    after_first_snapshot();
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "unsafe_path".to_string())?;
    let mut verification = [0_u8; 64 * 1024];
    let mut verified_bytes = 0usize;
    loop {
        let read = file
            .read(&mut verification)
            .map_err(|_| "unsafe_path".to_string())?;
        if read == 0 {
            break;
        }
        let end = verified_bytes
            .checked_add(read)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| "unsafe_path".to_string())?;
        if bytes.get(verified_bytes..end) != Some(&verification[..read]) {
            return Err("unsafe_path".to_string());
        }
        verified_bytes = end;
    }
    if verified_bytes != bytes.len() {
        return Err("unsafe_path".to_string());
    }
    let opened_after = file.metadata().map_err(|_| "unsafe_path".to_string())?;
    let (opened_id_after, opened_links_after) = handle_identity(&file)?;
    if opened_id_after != opened_id
        || opened_links_after != opened_links
        || opened_after.len() != opened_metadata.len()
        || opened_after.modified().ok() != opened_metadata.modified().ok()
    {
        return Err("unsafe_path".to_string());
    }

    let entry_after = std::fs::symlink_metadata(path).map_err(|_| "unsafe_path".to_string())?;
    if !entry_after.is_file() || is_link_or_reparse(&entry_after) {
        return Err("unsafe_path".to_string());
    }
    let reopened = open_read_no_follow(path)?;
    let (reopened_id, reopened_links) = handle_identity(&reopened)?;
    if reopened_id != opened_id || reopened_links != 1 {
        return Err("unsafe_path".to_string());
    }

    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    Ok((
        bytes,
        VerifiedPathIdentity {
            canonical_path: std::fs::canonicalize(path).map_err(|_| "unsafe_path".to_string())?,
            object_id: opened_id,
            metadata: snapshot(&opened_metadata, opened_links),
            content_sha256: Some(digest),
        },
    ))
}

#[cfg(windows)]
pub fn opened_file_is_delete_pending(file: &File) -> bool {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileStandardInfo, GetFileInformationByHandleEx, FILE_STANDARD_INFO,
    };

    let mut information = MaybeUninit::<FILE_STANDARD_INFO>::zeroed();
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStandardInfo,
            information.as_mut_ptr().cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    succeeded != 0 && unsafe { information.assume_init() }.DeletePending != 0
}

#[cfg(not(windows))]
pub fn opened_file_is_delete_pending(_file: &File) -> bool {
    false
}

pub fn sync_parent_best_effort(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Remove only the regular one-link object proven by `expected`. Windows
/// deletion is issued through the verified leaf handle, closing the final
/// pathname validation-to-delete race without deleting an attacker replacement.
pub fn remove_regular_file_if_same(path: &Path, expected: &VerifiedPathIdentity) -> bool {
    remove_regular_file_if_same_inner(path, expected, || {})
}

fn remove_regular_file_if_same_inner(
    path: &Path,
    expected: &VerifiedPathIdentity,
    after_validation: impl FnOnce(),
) -> bool {
    let Some(parent_path) = path.parent() else {
        return false;
    };
    let Ok(parent_before) = verify_directory(parent_path) else {
        return false;
    };
    let Ok(current) = verify_regular_file(path) else {
        return std::fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
    };
    if !same_object(expected, &current) {
        return false;
    }
    after_validation();
    let Ok(parent) = retain_immutable_directory(parent_path) else {
        return false;
    };
    if !same_object(&parent_before, parent.identity()) {
        return false;
    }

    #[cfg(windows)]
    {
        remove_windows_regular_file_by_handle(path, expected)
    }
    #[cfg(not(windows))]
    {
        let Ok(current) = verify_regular_file(path) else {
            return std::fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        };
        if !same_object(expected, &current) {
            return false;
        }
        match std::fs::remove_file(path) {
            Ok(()) => true,
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        }
    }
}

#[cfg(windows)]
fn remove_windows_regular_file_by_handle(path: &Path, expected: &VerifiedPathIdentity) -> bool {
    remove_windows_regular_file_by_handle_inner(path, expected, || {})
}

#[cfg(windows)]
fn remove_windows_regular_file_by_handle_inner(
    path: &Path,
    expected: &VerifiedPathIdentity,
    before_delete: impl FnOnce(),
) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, DELETE, FILE_DISPOSITION_INFO,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = OpenOptions::new();
    options
        .read(true)
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let Ok(file) = options.open(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    let Ok((object_id, links)) = handle_identity(&file) else {
        return false;
    };
    if !metadata.is_file()
        || is_link_or_reparse(&metadata)
        || links != 1
        || object_id != expected.object_id
    {
        return false;
    }
    before_delete();
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            std::ptr::addr_of!(disposition).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        ) != 0
    }
}

#[cfg(windows)]
pub fn publish_new_file_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    fn canonical_leaf(path: &Path) -> Result<PathBuf, String> {
        let leaf = path
            .file_name()
            .ok_or_else(|| "atomic_publish_failed".to_string())?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent =
            std::fs::canonicalize(parent).map_err(|_| "atomic_publish_failed".to_string())?;
        Ok(parent.join(leaf))
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;
    // Canonicalizing only each existing parent supplies the Windows verbatim
    // prefix needed beyond MAX_PATH while preserving the un-followed leaf name.
    let source = canonical_leaf(source)?;
    let destination = canonical_leaf(destination)?;
    let existing: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let new: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // Omitting MOVEFILE_REPLACE_EXISTING is the Windows no-clobber contract.
    let moved = unsafe { MoveFileExW(existing.as_ptr(), new.as_ptr(), MOVEFILE_WRITE_THROUGH) };
    if moved == 0 {
        Err("atomic_publish_failed".to_string())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub fn publish_new_file_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    const AT_FDCWD: libc::c_int = libc::AT_FDCWD;
    const RENAME_NOREPLACE: libc::c_uint = libc::RENAME_NOREPLACE;
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| "atomic_publish_failed".to_string())?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| "atomic_publish_failed".to_string())?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            AT_FDCWD,
            source.as_ptr(),
            AT_FDCWD,
            destination.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err("atomic_publish_failed".to_string())
    }
}

#[cfg(target_os = "macos")]
pub fn publish_new_file_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn renamex_np(
            old_path: *const std::ffi::c_char,
            new_path: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }
    const RENAME_EXCL: libc::c_uint = libc::RENAME_EXCL;
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| "atomic_publish_failed".to_string())?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| "atomic_publish_failed".to_string())?;
    let result = unsafe { renamex_np(source.as_ptr(), destination.as_ptr(), RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err("atomic_publish_failed".to_string())
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn publish_new_file_atomic(_source: &Path, _destination: &Path) -> Result<(), String> {
    Err("atomic_publish_unsupported".to_string())
}

#[cfg(windows)]
fn atomic_replace_existing(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }
    let replaced: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let replacement: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    const REPLACEFILE_WRITE_THROUGH: u32 = 0x0000_0001;
    let replaced_ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if replaced_ok == 0 {
        Err("atomic_replace_failed".to_string())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace_existing(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|_| "atomic_replace_failed".to_string())
}

/// Replace an already existing, verified regular file without an unlink gap.
/// The temporary file is written in the same verified directory and synced
/// before the platform's atomic replacement operation.
pub fn replace_regular_file_atomic(
    path: &Path,
    expected: &VerifiedPathIdentity,
    bytes: &[u8],
    max_bytes: usize,
) -> Result<VerifiedPathIdentity, String> {
    if bytes.len() > max_bytes {
        return Err("capacity_exceeded".to_string());
    }
    let (_, original) = read_bounded_regular(path, max_bytes)?;
    if !same_object(expected, &original)
        || expected.content_sha256.is_none()
        || expected.content_sha256 != original.content_sha256
    {
        return Err("unsafe_path".to_string());
    }
    let parent = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    let parent_identity = verify_directory(parent)?;
    let temporary = parent.join(format!(
        ".ac-pty-input-{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(UNIX_O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let result = (|| {
        let mut temporary_file = options
            .open(&temporary)
            .map_err(|_| "atomic_replace_failed".to_string())?;
        temporary_file
            .write_all(bytes)
            .map_err(|_| "atomic_replace_failed".to_string())?;
        temporary_file
            .sync_all()
            .map_err(|_| "atomic_replace_failed".to_string())?;
        drop(temporary_file);

        let current_parent = verify_directory(parent)?;
        if !same_object(&parent_identity, &current_parent) {
            return Err("unsafe_path".to_string());
        }
        let current = open_read_no_follow(path)?;
        let (current_id, current_links) = handle_identity(&current)?;
        if current_id != original.object_id || current_links != 1 {
            return Err("unsafe_path".to_string());
        }
        atomic_replace_existing(&temporary, path)?;
        #[cfg(unix)]
        open_directory_no_follow(parent)?
            .sync_all()
            .map_err(|_| "atomic_replace_failed".to_string())?;

        let (published, identity) = read_bounded_regular(path, max_bytes)?;
        if published != bytes {
            return Err("atomic_replace_failed".to_string());
        }
        Ok(identity)
    })();
    if result.is_err() {
        let _cleanup_result = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn component_eq(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CompareStringOrdinal(
            left: *const u16,
            left_len: i32,
            right: *const u16,
            right_len: i32,
            ignore_case: i32,
        ) -> i32;
    }
    const CSTR_EQUAL: i32 = 2;
    let left: Vec<u16> = left.encode_wide().collect();
    let right: Vec<u16> = right.encode_wide().collect();
    let Ok(left_len) = i32::try_from(left.len()) else {
        return false;
    };
    let Ok(right_len) = i32::try_from(right.len()) else {
        return false;
    };
    unsafe {
        CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, 1) == CSTR_EQUAL
    }
}

#[cfg(not(windows))]
fn component_eq(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    left == right
}

pub fn paths_equivalent(left: &Path, right: &Path) -> bool {
    let mut left = left.components();
    let mut right = right.components();
    loop {
        match (left.next(), right.next()) {
            (Some(left), Some(right)) if component_eq(left.as_os_str(), right.as_os_str()) => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

pub fn path_is_descendant(child: &Path, parent: &Path) -> bool {
    let child_components: Vec<_> = child.components().collect();
    let parent_components: Vec<_> = parent.components().collect();
    child_components.len() >= parent_components.len()
        && child_components
            .iter()
            .zip(parent_components.iter())
            .all(|(child, parent)| component_eq(child.as_os_str(), parent.as_os_str()))
}

pub fn is_verified_descendant(child: &VerifiedPathIdentity, parent: &VerifiedPathIdentity) -> bool {
    path_is_descendant(&child.canonical_path, &parent.canonical_path)
}

pub fn same_object(a: &VerifiedPathIdentity, b: &VerifiedPathIdentity) -> bool {
    a.object_id == b.object_id
}

pub struct ScannedJsonDocument {
    pub value: serde_json::Value,
    pub duplicate_keys: bool,
    pub privileged_candidate: bool,
}

#[derive(Default)]
struct JsonScanState {
    duplicate_keys: bool,
    privileged_candidate: bool,
}

/// Parse one JSON document while retaining whether any object key was
/// duplicated and whether any top-level occurrence identifies privileged PTY
/// input. Escaped JSON key and value spellings are decoded by Serde before the
/// classification is recorded.
pub fn scan_json_document(bytes: &[u8]) -> Result<ScannedJsonDocument, String> {
    let state = std::rc::Rc::new(std::cell::RefCell::new(JsonScanState::default()));
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = ScanningValue {
        state: std::rc::Rc::clone(&state),
        depth: 0,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| "invalid_envelope".to_string())?;
    deserializer
        .end()
        .map_err(|_| "invalid_envelope".to_string())?;
    let state = state.borrow();
    Ok(ScannedJsonDocument {
        value,
        duplicate_keys: state.duplicate_keys,
        privileged_candidate: state.privileged_candidate,
    })
}

/// Parse JSON while rejecting duplicate object keys at every depth.
pub fn parse_json_no_duplicates(bytes: &[u8]) -> Result<serde_json::Value, String> {
    let document = scan_json_document(bytes)?;
    if document.duplicate_keys {
        return Err("invalid_envelope".to_string());
    }
    Ok(document.value)
}

struct ScanningValue {
    state: std::rc::Rc<std::cell::RefCell<JsonScanState>>,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for ScanningValue {
    type Value = serde_json::Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ScanningVisitor {
            state: self.state,
            depth: self.depth,
        })
    }
}

struct ScanningVisitor {
    state: std::rc::Rc<std::cell::RefCell<JsonScanState>>,
    depth: usize,
}

impl<'de> Visitor<'de> for ScanningVisitor {
    type Value = serde_json::Value;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| E::custom("invalid number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(serde_json::Value::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(serde_json::Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        ScanningValue {
            state: self.state,
            depth: self.depth,
        }
        .deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(ScanningValue {
            state: std::rc::Rc::clone(&self.state),
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(serde_json::Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut seen = HashSet::new();
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                self.state.borrow_mut().duplicate_keys = true;
            }
            let value = map.next_value_seed(ScanningValue {
                state: std::rc::Rc::clone(&self.state),
                depth: self.depth + 1,
            })?;
            if self.depth == 0 {
                let privileged = key == "ptyInput"
                    || (key == "action" && value.as_str() == Some("pty-input"))
                    || (key == "kind" && value.as_str() == Some("pty-input-marker"));
                if privileged {
                    self.state.borrow_mut().privileged_candidate = true;
                }
            }
            values.insert(key, value);
        }
        Ok(serde_json::Value::Object(values))
    }
}

/// A caller-owned output file whose leaf handle and parent directory remain
/// retained until the write and identity checks complete.
pub struct VerifiedNewFile {
    path: PathBuf,
    file: File,
    parent: RetainedDirectory,
}

impl VerifiedNewFile {
    /// Failure intentionally leaves the newly created caller-owned file in
    /// place. The CLI must never unlink a path after ownership becomes unclear.
    pub fn write_all_and_sync(self, bytes: &[u8]) -> Result<(), String> {
        self.write_all_and_sync_inner(bytes, || {}, || {})
    }

    fn write_all_and_sync_inner(
        mut self,
        bytes: &[u8],
        after_write: impl FnOnce(),
        after_sync: impl FnOnce(),
    ) -> Result<(), String> {
        self.parent
            .verify_current()
            .map_err(|_| "output_failed".to_string())?;
        self.file
            .write_all(bytes)
            .and_then(|_| self.file.flush())
            .map_err(|_| "output_failed".to_string())?;
        after_write();
        self.parent
            .verify_current()
            .map_err(|_| "output_failed".to_string())?;
        self.file
            .sync_all()
            .map_err(|_| "output_failed".to_string())?;
        after_sync();
        self.parent
            .verify_current()
            .map_err(|_| "output_failed".to_string())?;
        let opened = self
            .parent
            .verify_opened_regular_file(&self.path, &self.file, false)
            .map_err(|_| "output_failed".to_string())?;
        self.parent
            .verify_current()
            .map_err(|_| "output_failed".to_string())?;
        if opened.metadata.links != 1 || opened.metadata.len != bytes.len() as u64 {
            return Err("output_failed".to_string());
        }
        Ok(())
    }
}

pub fn create_terminal_snapshot_output(path: &Path) -> Result<VerifiedNewFile, String> {
    create_terminal_snapshot_output_inner(path, || {})
}

fn create_terminal_snapshot_output_inner(
    path: &Path,
    before_retention: impl FnOnce(),
) -> Result<VerifiedNewFile, String> {
    create_terminal_snapshot_output_with_hooks(path, before_retention, || {})
}

fn create_terminal_snapshot_output_with_hooks(
    path: &Path,
    before_retention: impl FnOnce(),
    before_open: impl FnOnce(),
) -> Result<VerifiedNewFile, String> {
    let validated_parent = validate_terminal_snapshot_output_path_inner(path, || {})?;
    let parent_path = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    before_retention();
    let parent =
        retain_immutable_directory(parent_path).map_err(|_| "output_failed".to_string())?;
    if !same_object(&validated_parent, parent.identity()) {
        return Err("output_failed".to_string());
    }
    before_open();
    let file = parent
        .create_new_output_file(path)
        .map_err(|_| "output_failed".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "output_failed".to_string())?;
        if file
            .metadata()
            .map_err(|_| "output_failed".to_string())?
            .permissions()
            .mode()
            & 0o777
            != 0o600
        {
            return Err("output_failed".to_string());
        }
    }
    parent
        .verify_opened_regular_file(path, &file, true)
        .map_err(|_| "output_failed".to_string())?;
    parent
        .verify_current()
        .map_err(|_| "output_failed".to_string())?;
    Ok(VerifiedNewFile {
        path: path.to_path_buf(),
        file,
        parent,
    })
}

pub fn validate_terminal_snapshot_output_path(path: &Path) -> Result<(), String> {
    validate_terminal_snapshot_output_path_inner(path, || {}).map(|_| ())
}

fn validate_terminal_snapshot_output_path_inner(
    path: &Path,
    before_filesystem: impl FnOnce(),
) -> Result<VerifiedPathIdentity, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("unsafe_path".to_string());
    }
    let extension = path.extension().ok_or_else(|| "unsafe_path".to_string())?;
    #[cfg(unix)]
    let extension_is_png = {
        use std::os::unix::ffi::OsStrExt;
        extension
            .as_bytes()
            .iter()
            .copied()
            .map(|byte| byte.to_ascii_lowercase())
            .eq(b"png".iter().copied())
    };
    #[cfg(not(unix))]
    let extension_is_png = extension
        .to_str()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if !extension_is_png {
        return Err("unsafe_path".to_string());
    }
    #[cfg(windows)]
    validate_windows_snapshot_output_path(path)?;
    before_filesystem();
    let parent = path.parent().ok_or_else(|| "unsafe_path".to_string())?;
    let parent_identity = verify_directory(parent)?;
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(parent_identity),
        _ => Err("unsafe_path".to_string()),
    }
}

#[cfg(windows)]
fn validate_windows_snapshot_output_path(path: &Path) -> Result<(), String> {
    use std::path::Prefix;

    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("unsafe_path".to_string());
    }
    let raw = path
        .as_os_str()
        .to_str()
        .ok_or_else(|| "unsafe_path".to_string())?;
    if raw
        .split(['\\', '/'])
        .any(|component| matches!(component, "." | ".."))
    {
        return Err("unsafe_path".to_string());
    }
    let mut components = path.components();
    let allowed_drive_colon = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(_) => Some(1),
            Prefix::VerbatimDisk(_) => {
                raw.char_indices().find_map(
                    |(index, value)| {
                        if value == ':' {
                            Some(index)
                        } else {
                            None
                        }
                    },
                )
            }
            Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _) => None,
            _ => return Err("unsafe_path".to_string()),
        },
        _ => return Err("unsafe_path".to_string()),
    };
    if raw
        .char_indices()
        .any(|(index, value)| value == ':' && Some(index) != allowed_drive_colon)
    {
        return Err("unsafe_path".to_string());
    }
    for component in path.components() {
        let Component::Normal(value) = component else {
            continue;
        };
        let Some(value) = value.to_str() else {
            return Err("unsafe_path".to_string());
        };
        if value.ends_with([' ', '.']) {
            return Err("unsafe_path".to_string());
        }
        let stem = value
            .split('.')
            .next()
            .unwrap_or(value)
            .trim_end_matches([' ', '.']);
        let reserved = matches!(
            stem.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "CONIN$"
                | "CONOUT$"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "COM¹"
                | "COM²"
                | "COM³"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
                | "LPT¹"
                | "LPT²"
                | "LPT³"
        );
        if reserved
            || value.chars().any(|character| {
                character <= '\u{1f}' || matches!(character, '<' | '>' | '"' | '|' | '?' | '*')
            })
        {
            return Err("unsafe_path".to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_child_creation_accepts_only_one_safe_leaf() {
        let fixture = tempfile::TempDir::new().unwrap();
        let parent = retain_directory(fixture.path()).unwrap();
        let child =
            open_or_create_verified_child_directory(&parent, std::ffi::OsStr::new("leaf")).unwrap();

        assert!(child.identity().canonical_path.ends_with("leaf"));
        assert!(open_or_create_verified_child_directory(
            &parent,
            std::ffi::OsStr::new("nested/leaf")
        )
        .is_err());
        assert!(
            open_or_create_verified_child_directory(&parent, std::ffi::OsStr::new("..")).is_err()
        );
    }

    #[test]
    fn duplicate_json_keys_reject_recursively() {
        assert!(parse_json_no_duplicates(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse_json_no_duplicates(br#"{"a":{"b":1,"b":2}}"#).is_err());
        assert!(parse_json_no_duplicates(br#"{"a":[{"b":1}]}"#).is_ok());
    }

    #[test]
    fn scan_detects_escaped_and_overwritten_privileged_occurrences() {
        let document =
            scan_json_document(br#"{"action":"pty\u002dinput","action":"ordinary","body":"x"}"#)
                .unwrap();
        assert!(document.duplicate_keys);
        assert!(document.privileged_candidate);
        assert_eq!(document.value["action"], "ordinary");

        let standard = scan_json_document(br#"{"body":"pty-input","body":"ordinary"}"#).unwrap();
        assert!(standard.duplicate_keys);
        assert!(!standard.privileged_candidate);
    }

    #[test]
    fn bounded_read_returns_content_fingerprint() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("value.json");
        std::fs::write(&path, b"{}" as &[u8]).unwrap();
        let (bytes, identity) = read_bounded_regular(&path, 16).unwrap();
        assert_eq!(bytes, b"{}");
        assert!(identity.content_sha256.is_some());
        assert!(read_bounded_regular(&path, 1).is_err());
    }

    #[test]
    fn bounded_read_rejects_same_length_in_place_mutation_between_snapshots() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("identity.json");
        std::fs::write(&path, b"first").unwrap();
        let replacement = path.clone();
        let result = read_bounded_regular_inner(&path, 16, move || {
            std::fs::write(replacement, b"other").unwrap();
        });
        assert!(result.is_err());
    }

    #[test]
    fn same_object_removal_rechecks_and_preserves_a_replacement() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("owned-response.json");
        let displaced = directory.path().join("displaced-owned-response");
        std::fs::write(&path, b"owned response").unwrap();
        let expected = verify_regular_file(&path).unwrap();
        let raced_path = path.clone();
        let raced_displaced = displaced.clone();

        assert!(!remove_regular_file_if_same_inner(
            &path,
            &expected,
            move || {
                std::fs::rename(&raced_path, &raced_displaced).unwrap();
                std::fs::write(&raced_path, b"replacement response").unwrap();
            },
        ));
        assert_eq!(std::fs::read(path).unwrap(), b"replacement response");
        assert_eq!(std::fs::read(displaced).unwrap(), b"owned response");
    }

    #[test]
    fn atomic_new_file_publication_never_clobbers_an_existing_destination() {
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("source.tmp");
        let destination = dir.path().join("destination.json");
        std::fs::write(&source, b"new").unwrap();
        std::fs::write(&destination, b"existing").unwrap();

        assert!(publish_new_file_atomic(&source, &destination).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"new");
        assert_eq!(std::fs::read(&destination).unwrap(), b"existing");
    }

    #[test]
    fn atomic_regular_replacement_is_bounded_and_leaves_a_complete_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("marker.json");
        std::fs::write(&path, b"old").unwrap();
        let before = verify_regular_file(&path).unwrap();
        let (_, expected) = read_bounded_regular(&path, 32).unwrap();
        let after = replace_regular_file_atomic(&path, &expected, b"new marker", 32).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new marker");
        assert!(!same_object(&before, &after));
        assert!(replace_regular_file_atomic(&path, &after, &[b'x'; 33], 32).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"new marker");

        let (_, expected) = read_bounded_regular(&path, 32).unwrap();
        std::fs::write(&path, b"other value").unwrap();
        assert!(replace_regular_file_atomic(&path, &expected, b"marker", 32).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"other value");
    }

    #[test]
    fn regular_file_verification_rejects_hard_links() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("identity.json");
        let alias = dir.path().join("identity-alias.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(verify_regular_file(&path).is_err());
        assert!(read_bounded_regular(&path, 16).is_err());
    }

    #[test]
    fn directory_identity_detects_same_spelling_replacement() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("replica");
        let retired = dir.path().join("retired");
        std::fs::create_dir(&path).unwrap();
        let before = verify_directory(&path).unwrap();
        std::fs::rename(&path, &retired).unwrap();
        std::fs::create_dir(&path).unwrap();
        let after = verify_directory(&path).unwrap();
        assert!(!same_object(&before, &after));
    }

    #[test]
    fn component_chain_rejects_a_linked_ancestor_when_supported() {
        let dir = tempfile::TempDir::new().unwrap();
        let real = dir.path().join("real");
        let link = dir.path().join("link");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("identity.json"), b"{}").unwrap();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&real, &link).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
        #[cfg(not(any(unix, windows)))]
        let linked = false;
        if linked {
            assert!(verify_directory(&link).is_err());
            assert!(read_bounded_regular(&link.join("identity.json"), 16).is_err());
        }
    }

    #[test]
    fn terminal_snapshot_post_validation_create_race_is_output_failed() {
        let directory = tempfile::TempDir::new().unwrap();
        let output = directory.path().join("raced.png");
        let raced = output.clone();
        let error = create_terminal_snapshot_output_inner(&output, move || {
            std::fs::write(raced, b"collision").unwrap();
        })
        .err()
        .unwrap();
        assert_eq!(error, "output_failed");
        assert_eq!(std::fs::read(output).unwrap(), b"collision");
    }

    #[cfg(windows)]
    #[test]
    fn terminal_snapshot_parent_replacement_before_create_leaves_no_output() {
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("parent");
        let retired = directory.path().join("retired");
        std::fs::create_dir(&parent).unwrap();
        let output = parent.join("snapshot.png");
        let parent_for_race = parent.clone();
        let retired_for_race = retired.clone();

        let error = create_terminal_snapshot_output_inner(&output, move || {
            std::fs::rename(&parent_for_race, &retired_for_race).unwrap();
            std::fs::create_dir(&parent_for_race).unwrap();
        })
        .err()
        .unwrap();

        assert_eq!(error, "output_failed");
        assert!(
            !output.exists(),
            "replacement parent received an output leaf"
        );
        assert!(
            !retired.join("snapshot.png").exists(),
            "retained parent received an output leaf after identity loss"
        );
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_parent_replacement_before_open_is_confined() {
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("parent");
        let retired = directory.path().join("retired");
        std::fs::create_dir(&parent).unwrap();
        let output = parent.join("snapshot.png");
        let parent_for_race = parent.clone();
        let retired_for_race = retired.clone();

        let error = create_terminal_snapshot_output_with_hooks(
            &output,
            || {},
            move || {
                std::fs::rename(&parent_for_race, &retired_for_race).unwrap();
                std::fs::create_dir(&parent_for_race).unwrap();
            },
        )
        .err()
        .unwrap();

        assert_eq!(error, "output_failed");
        assert!(
            !output.exists(),
            "replacement parent received an output leaf"
        );
        assert!(
            !retired.join("snapshot.png").exists(),
            "retained parent received a leaf after detected identity loss"
        );
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_write_stays_with_retained_parent_object() {
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("parent");
        let retired = directory.path().join("retired");
        std::fs::create_dir(&parent).unwrap();
        let output = parent.join("snapshot.png");
        let file = create_terminal_snapshot_output(&output).unwrap();
        let parent_for_race = parent.clone();
        let retired_for_race = retired.clone();
        let replacement_output = output.clone();

        let error = file
            .write_all_and_sync_inner(
                b"owned bytes",
                move || {
                    std::fs::rename(&parent_for_race, &retired_for_race).unwrap();
                    std::fs::create_dir(&parent_for_race).unwrap();
                    std::fs::write(&replacement_output, b"replacement bytes").unwrap();
                },
                || {},
            )
            .unwrap_err();

        assert_eq!(error, "output_failed");
        assert_eq!(std::fs::read(&output).unwrap(), b"replacement bytes");
        assert_eq!(
            std::fs::read(retired.join("snapshot.png")).unwrap(),
            b"owned bytes"
        );
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_cleanup_final_barrier_preserves_public_replacement() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("response.json");
        std::fs::write(&path, b"tracked response").unwrap();
        let retained = retain_directory(directory.path()).unwrap();
        let expected = retained.verify_regular_file(&path).unwrap();
        let witness = retained.retain_unix_file_witness(&path, &expected).unwrap();

        let outcome = retained.cleanup_unix_tracked_file_with_hook(
            &path,
            &expected,
            &witness,
            Some(std::ffi::OsStr::new(".public-replacement-claim")),
            |stage, source, _| {
                if stage == UnixTrackedCleanupStage::BeforeClaimUnlink {
                    std::fs::write(source, b"replacement response").unwrap();
                }
            },
        );

        assert_eq!(outcome, UnixTrackedCleanupOutcome::Removed);
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement response");
        assert_eq!(witness.state(&expected), UnixFileWitnessState::Unlinked);
        let second = retained.cleanup_unix_tracked_file_with_hook(
            &path,
            &expected,
            &witness,
            Some(std::ffi::OsStr::new(".unused-second-claim")),
            |_, _, _| {},
        );
        assert_eq!(second, UnixTrackedCleanupOutcome::AlreadyAbsent);
        assert_eq!(std::fs::read(path).unwrap(), b"replacement response");
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_cleanup_reverifies_private_claim_at_final_barrier() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("response.json");
        let displaced = directory.path().join("displaced-tracked-response");
        std::fs::write(&path, b"tracked response").unwrap();
        let retained = retain_directory(directory.path()).unwrap();
        let expected = retained.verify_regular_file(&path).unwrap();
        let witness = retained.retain_unix_file_witness(&path, &expected).unwrap();
        let displaced_for_hook = displaced.clone();

        let outcome = retained.cleanup_unix_tracked_file_with_hook(
            &path,
            &expected,
            &witness,
            Some(std::ffi::OsStr::new(".substituted-private-claim")),
            move |stage, _, claim| {
                if stage == UnixTrackedCleanupStage::BeforeClaimUnlink {
                    std::fs::rename(claim, &displaced_for_hook).unwrap();
                    std::fs::write(claim, b"foreign claim").unwrap();
                }
            },
        );

        assert_eq!(outcome, UnixTrackedCleanupOutcome::Uncertain);
        assert_eq!(
            std::fs::read(directory.path().join(".substituted-private-claim")).unwrap(),
            b"foreign claim"
        );
        assert_eq!(std::fs::read(displaced).unwrap(), b"tracked response");
        assert_eq!(witness.state(&expected), UnixFileWitnessState::Linked);
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_cleanup_claim_collision_retains_source_and_collision() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("response.json");
        let claim = directory.path().join(".collided-private-claim");
        std::fs::write(&path, b"tracked response").unwrap();
        std::fs::write(&claim, b"claim collision").unwrap();
        let retained = retain_directory(directory.path()).unwrap();
        let expected = retained.verify_regular_file(&path).unwrap();
        let witness = retained.retain_unix_file_witness(&path, &expected).unwrap();

        let outcome = retained.cleanup_unix_tracked_file_with_hook(
            &path,
            &expected,
            &witness,
            claim.file_name(),
            |_, _, _| {},
        );

        assert_eq!(outcome, UnixTrackedCleanupOutcome::SourceRetained);
        assert_eq!(std::fs::read(path).unwrap(), b"tracked response");
        assert_eq!(std::fs::read(claim).unwrap(), b"claim collision");
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_cleanup_restores_source_substitution_without_deleting_it() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("response.json");
        let displaced = directory.path().join("displaced-tracked-response");
        std::fs::write(&path, b"tracked response").unwrap();
        let retained = retain_directory(directory.path()).unwrap();
        let expected = retained.verify_regular_file(&path).unwrap();
        let witness = retained.retain_unix_file_witness(&path, &expected).unwrap();
        let path_for_hook = path.clone();
        let displaced_for_hook = displaced.clone();

        let outcome = retained.cleanup_unix_tracked_file_with_hook(
            &path,
            &expected,
            &witness,
            Some(std::ffi::OsStr::new(".captured-substitution-claim")),
            move |stage, _, _| {
                if stage == UnixTrackedCleanupStage::BeforeClaimRename {
                    std::fs::rename(&path_for_hook, &displaced_for_hook).unwrap();
                    std::fs::write(&path_for_hook, b"replacement response").unwrap();
                }
            },
        );

        assert_eq!(outcome, UnixTrackedCleanupOutcome::Uncertain);
        assert_eq!(std::fs::read(path).unwrap(), b"replacement response");
        assert_eq!(std::fs::read(displaced).unwrap(), b"tracked response");
        assert!(!directory
            .path()
            .join(".captured-substitution-claim")
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_cleanup_restore_collision_preserves_every_name() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("response.json");
        let displaced = directory.path().join("displaced-tracked-response");
        let claim = directory.path().join(".restore-collision-claim");
        std::fs::write(&path, b"tracked response").unwrap();
        let retained = retain_directory(directory.path()).unwrap();
        let expected = retained.verify_regular_file(&path).unwrap();
        let witness = retained.retain_unix_file_witness(&path, &expected).unwrap();
        let path_for_hook = path.clone();
        let displaced_for_hook = displaced.clone();

        let outcome = retained.cleanup_unix_tracked_file_with_hook(
            &path,
            &expected,
            &witness,
            claim.file_name(),
            move |stage, source, _| match stage {
                UnixTrackedCleanupStage::BeforeClaimRename => {
                    std::fs::rename(&path_for_hook, &displaced_for_hook).unwrap();
                    std::fs::write(&path_for_hook, b"captured replacement").unwrap();
                }
                UnixTrackedCleanupStage::BeforeRestore => {
                    std::fs::write(source, b"restoration collision").unwrap();
                }
                UnixTrackedCleanupStage::BeforeClaimUnlink => {}
            },
        );

        assert_eq!(outcome, UnixTrackedCleanupOutcome::Uncertain);
        assert_eq!(std::fs::read(path).unwrap(), b"restoration collision");
        assert_eq!(std::fs::read(claim).unwrap(), b"captured replacement");
        assert_eq!(std::fs::read(displaced).unwrap(), b"tracked response");
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_cleanup_uses_retained_parent_object() {
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("parent");
        let retired = directory.path().join("retired");
        std::fs::create_dir(&parent).unwrap();
        let output = parent.join("response.json");
        std::fs::write(&output, b"owned response").unwrap();
        let retained = retain_directory(&parent).unwrap();
        let expected = retained.verify_regular_file(&output).unwrap();

        std::fs::rename(&parent, &retired).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(&output, b"replacement response").unwrap();

        assert!(retained.remove_regular_file_if_same(&output, &expected));
        assert!(!retired.join("response.json").exists());
        assert_eq!(std::fs::read(output).unwrap(), b"replacement response");
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_child_api_rejects_invalid_paths_and_linked_leaves() {
        use std::os::unix::ffi::OsStringExt;

        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("parent");
        let wrong_parent = directory.path().join("wrong-parent");
        std::fs::create_dir(&parent).unwrap();
        std::fs::create_dir(&wrong_parent).unwrap();
        let retained = retain_directory(&parent).unwrap();
        let outside = wrong_parent.join("response.json");
        std::fs::write(&outside, b"outside bytes").unwrap();

        for invalid in [
            parent.clone(),
            outside.clone(),
            parent.join("."),
            parent.join(".."),
            parent.join("nested/response.json"),
        ] {
            assert!(retained.checked_unix_child_name(&invalid).is_err());
            assert!(retained.create_new_private_file(&invalid).is_err());
            assert!(!retained.child_is_absent(&invalid));
        }

        let nul_leaf = std::ffi::OsString::from_vec(b"nul\0response.json".to_vec());
        let nul_path = parent.join(nul_leaf);
        assert!(retained.checked_unix_child_name(&nul_path).is_err());
        assert!(!retained.child_is_absent(&nul_path));
        assert_eq!(std::fs::read(outside).unwrap(), b"outside bytes");

        let target = parent.join("target");
        let symlink = parent.join("symlink.json");
        std::fs::write(&target, b"target bytes").unwrap();
        std::os::unix::fs::symlink(&target, &symlink).unwrap();
        assert!(retained.verify_regular_file(&symlink).is_err());
        assert!(!retained.child_is_absent(&symlink));

        let hard_source = parent.join("hard-source");
        let hard_leaf = parent.join("hard.json");
        std::fs::write(&hard_source, b"hard bytes").unwrap();
        std::fs::hard_link(&hard_source, &hard_leaf).unwrap();
        assert!(retained.verify_regular_file(&hard_leaf).is_err());
        assert!(!retained.child_is_absent(&hard_leaf));
        assert!(retained.child_is_absent(&parent.join("missing.json")));
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_absence_classification_uses_originating_error() {
        assert!(unix_child_open_error_is_absent(
            &std::io::Error::from_raw_os_error(libc::ENOENT)
        ));
        for code in [libc::ELOOP, libc::EACCES, libc::EIO, libc::EINVAL] {
            assert!(!unix_child_open_error_is_absent(
                &std::io::Error::from_raw_os_error(code)
            ));
        }
        assert!(!unix_child_open_error_is_absent(&std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "synthetic not-found without errno",
        )));
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_non_utf8_leaf_remains_supported() {
        use std::os::unix::ffi::OsStringExt;

        let directory = tempfile::TempDir::new().unwrap();
        let leaf = std::ffi::OsString::from_vec(b"snapshot-\xff.png".to_vec());
        let output = directory.path().join(leaf);
        create_terminal_snapshot_output(&output)
            .unwrap()
            .write_all_and_sync(b"portable bytes")
            .unwrap();
        assert_eq!(std::fs::read(output).unwrap(), b"portable bytes");
    }

    #[cfg(unix)]
    #[test]
    fn terminal_snapshot_unix_symlink_parent_is_rejected() {
        let directory = tempfile::TempDir::new().unwrap();
        let real = directory.path().join("real");
        let linked = directory.path().join("linked");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let output = linked.join("snapshot.png");

        assert_eq!(
            validate_terminal_snapshot_output_path(&output).unwrap_err(),
            "unsafe_path"
        );
        assert!(!real.join("snapshot.png").exists());
    }

    #[cfg(windows)]
    #[test]
    fn terminal_snapshot_windows_namespaces_reject_lexically_without_filesystem_work() {
        for path in [
            r"\\.\C:\snapshot.png",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\snapshot.png",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\snapshot.png",
            r"\\?\PIPE\snapshot.png",
            r"\??\GLOBALROOT\Device\HarddiskVolume1\snapshot.png",
            r"C:\safe\snapshot.png:stream",
            r"C:\safe\CON.png",
            r"C:\safe\CON .png",
            r"C:\safe\CONIN$.png",
            r"C:\safe\COM¹.png",
            r"C:\safe\LPT³.txt.png",
            r"C:\safe\trailing.\snapshot.png",
            "C:\\safe\\control\u{1f}.png",
            r"C:\safe\..\escape.png",
            r"C:\safe\.\snapshot.png",
        ] {
            assert!(
                validate_terminal_snapshot_output_path_inner(Path::new(path), || {
                    panic!("lexically rejected namespace reached filesystem validation")
                })
                .is_err(),
                "accepted unsafe Windows spelling: {path}"
            );
        }
        for path in [
            r"C:\safe\snapshot.png",
            r"\\server\share\snapshot.PNG",
            r"\\?\C:\safe\snapshot.png",
            r"\\?\UNC\server\share\snapshot.png",
        ] {
            assert!(
                validate_windows_snapshot_output_path(Path::new(path)).is_ok(),
                "rejected allowed Windows spelling: {path}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn retained_directory_has_stable_volume_file_id_and_blocks_replacement() {
        let directory = tempfile::TempDir::new().unwrap();
        let ancestor = directory.path().join("ancestor");
        let parent = ancestor.join("parent");
        let retired_parent = ancestor.join("retired-parent");
        let retired_ancestor = directory.path().join("retired-ancestor");
        std::fs::create_dir(&ancestor).unwrap();
        std::fs::create_dir(&parent).unwrap();
        let retained = retain_directory(&parent).unwrap();
        let identity = retained.identity().object_id;
        let current = verify_directory(&parent).unwrap();
        assert_eq!(identity.volume, current.object_id.volume);
        assert_eq!(identity.file, current.object_id.file);
        assert!(std::fs::rename(&parent, &retired_parent).is_err());
        assert!(std::fs::rename(&ancestor, &retired_ancestor).is_err());
        let retained_clone = retained.clone();
        drop(retained);
        assert!(std::fs::rename(&parent, &retired_parent).is_err());
        retained_clone.verify_current().unwrap();
        drop(retained_clone);
        std::fs::rename(&ancestor, &retired_ancestor).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn terminal_snapshot_output_rejects_junction_and_symlink_parents() {
        let directory = tempfile::TempDir::new().unwrap();
        let real = directory.path().join("real");
        let junction = directory.path().join("junction");
        std::fs::create_dir(&real).unwrap();
        let status = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&real)
            .status()
            .unwrap();
        assert!(status.success(), "failed to create the junction fixture");
        let junction_output = junction.join("snapshot.png");
        assert_eq!(
            validate_terminal_snapshot_output_path(&junction_output).unwrap_err(),
            "unsafe_path"
        );
        assert!(!real.join("snapshot.png").exists());
        std::fs::remove_dir(&junction).unwrap();

        let symlink = directory.path().join("symlink");
        if std::os::windows::fs::symlink_dir(&real, &symlink).is_ok() {
            let symlink_output = symlink.join("snapshot.png");
            assert_eq!(
                validate_terminal_snapshot_output_path(&symlink_output).unwrap_err(),
                "unsafe_path"
            );
            assert!(!real.join("snapshot.png").exists());
            std::fs::remove_dir(&symlink).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn retained_output_parent_and_leaf_block_create_write_sync_swaps() {
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("parent");
        let retired_parent = directory.path().join("retired-parent");
        let displaced_leaf = parent.join("displaced.png");
        let hard_link_leaf = parent.join("hard-link.png");
        std::fs::create_dir(&parent).unwrap();
        let output = parent.join("snapshot.png");
        let parent_before_open = parent.clone();
        let retired_before_open = retired_parent.clone();
        let file = create_terminal_snapshot_output_with_hooks(
            &output,
            || {},
            move || {
                assert!(std::fs::rename(&parent_before_open, &retired_before_open).is_err());
            },
        )
        .unwrap();
        let output_after_write = output.clone();
        let displaced_after_write = displaced_leaf.clone();
        let output_for_link = output.clone();
        let hard_link_after_write = hard_link_leaf.clone();
        let parent_after_sync = parent.clone();
        let retired_after_sync = retired_parent.clone();
        file.write_all_and_sync_inner(
            b"owned bytes",
            move || {
                assert!(
                    std::fs::rename(&output_after_write, &displaced_after_write).is_err(),
                    "opened output leaf was replaceable after write"
                );
                assert!(
                    std::fs::hard_link(&output_for_link, &hard_link_after_write).is_err(),
                    "opened output leaf allowed a hard-link alias"
                );
            },
            move || {
                assert!(
                    std::fs::rename(&parent_after_sync, &retired_after_sync).is_err(),
                    "retained output parent was replaceable after sync"
                );
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"owned bytes");
        assert!(!displaced_leaf.exists());
        assert!(!hard_link_leaf.exists());
        assert!(!retired_parent.exists());
    }

    #[cfg(windows)]
    #[test]
    fn exact_handle_delete_cannot_remove_a_leaf_swap() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("owned-response.json");
        let displaced = directory.path().join("displaced-response.json");
        std::fs::write(&path, b"owned response").unwrap();
        let expected = verify_regular_file(&path).unwrap();
        let path_for_hook = path.clone();
        let displaced_for_hook = displaced.clone();
        assert!(remove_windows_regular_file_by_handle_inner(
            &path,
            &expected,
            move || {
                assert!(
                    std::fs::rename(&path_for_hook, &displaced_for_hook).is_err(),
                    "verified delete handle allowed a final leaf swap"
                );
            },
        ));
        assert!(!path.exists());
        assert!(!displaced.exists());
    }

    #[cfg(windows)]
    #[test]
    fn terminal_snapshot_output_supports_case_unicode_identity_and_long_verbatim_paths() {
        let directory = tempfile::TempDir::new().unwrap();
        let case_parent = directory.path().join("CaseParent");
        std::fs::create_dir(&case_parent).unwrap();
        let upper_parent = PathBuf::from(case_parent.to_string_lossy().to_uppercase());
        assert_eq!(
            retain_directory(&case_parent).unwrap().identity().object_id,
            retain_directory(&upper_parent)
                .unwrap()
                .identity()
                .object_id
        );

        let composed = directory.path().join("\u{e9}");
        let decomposed = directory.path().join("e\u{301}");
        std::fs::create_dir(&composed).unwrap();
        std::fs::create_dir(&decomposed).unwrap();
        assert_ne!(
            verify_directory(&composed).unwrap().object_id,
            verify_directory(&decomposed).unwrap().object_id
        );

        let mut long_parent = std::fs::canonicalize(directory.path()).unwrap();
        while long_parent.as_os_str().len() < 300 {
            long_parent.push("terminal-snapshot-long-segment");
            std::fs::create_dir(&long_parent).unwrap();
        }
        let output = long_parent.join("snapshot.png");
        create_terminal_snapshot_output(&output)
            .unwrap()
            .write_all_and_sync(b"long path")
            .unwrap();
        assert_eq!(std::fs::read(output).unwrap(), b"long path");
    }

    #[test]
    fn terminal_snapshot_output_is_absolute_png_create_new() {
        let directory = tempfile::TempDir::new().unwrap();
        let output = directory.path().join("snapshot.PNG");
        let file = create_terminal_snapshot_output(&output).unwrap();
        file.write_all_and_sync(b"owned bytes").unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"owned bytes");
        assert!(create_terminal_snapshot_output(&output).is_err());
        assert!(validate_terminal_snapshot_output_path(Path::new("relative.png")).is_err());
        assert!(
            validate_terminal_snapshot_output_path(&directory.path().join("wrong.txt")).is_err()
        );
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
#[test]
fn terminal_snapshot_atomic_publish_preserves_collision_missing_source_and_raw_names() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let directory = tempfile::tempdir().unwrap();
    let source = directory
        .path()
        .join(OsString::from_vec(b"source-\xff".to_vec()));
    let destination = directory
        .path()
        .join(OsString::from_vec(b"destination-\xfe".to_vec()));
    std::fs::write(&source, b"source bytes").unwrap();
    std::fs::write(&destination, b"destination bytes").unwrap();

    assert_eq!(
        publish_new_file_atomic(&source, &destination),
        Err("atomic_publish_failed".to_string())
    );
    assert_eq!(std::fs::read(&source).unwrap(), b"source bytes");
    assert_eq!(std::fs::read(&destination).unwrap(), b"destination bytes");

    std::fs::remove_file(&destination).unwrap();
    assert_eq!(publish_new_file_atomic(&source, &destination), Ok(()));
    assert!(!source.exists());
    assert_eq!(std::fs::read(&destination).unwrap(), b"source bytes");

    let missing_source = directory.path().join("missing-source");
    let missing_destination = directory.path().join("missing-destination");
    assert_eq!(
        publish_new_file_atomic(&missing_source, &missing_destination),
        Err("atomic_publish_failed".to_string())
    );
    assert!(!missing_destination.exists());
}

#[cfg(all(test, unix, not(any(target_os = "linux", target_os = "macos"))))]
#[test]
fn terminal_snapshot_atomic_publish_is_stably_unsupported_on_other_unix() {
    let result = publish_new_file_atomic(
        std::path::Path::new("source"),
        std::path::Path::new("destination"),
    );
    assert_eq!(result, Err("atomic_publish_unsupported".to_string()));
}
