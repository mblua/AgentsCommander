use super::handle_identity;
use super::is_link_or_reparse;
#[cfg(windows)]
use super::regular::publish_new_file_atomic;
#[cfg(windows)]
use super::regular::remove_windows_regular_file_by_handle;
use super::regular::same_object;
#[cfg(unix)]
use super::snapshot;
#[cfg(all(unix, test))]
use super::unix_child_open_error_is_absent;
use super::verify_directory;
#[cfg(not(unix))]
use super::verify_opened_regular_file;
#[cfg(not(unix))]
use super::verify_regular_file;
use super::RetainedDirectory;
#[cfg(unix)]
use super::RetainedUnixFileWitness;
#[cfg(unix)]
use super::UnixFileWitnessState;
#[cfg(unix)]
use super::UnixTrackedCleanupOutcome;
#[cfg(unix)]
use super::UnixTrackedCleanupStage;
use super::VerifiedPathIdentity;
use std::fs::File;
#[cfg(not(unix))]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::path::Component;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

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
            self.open_unix_child(
                path,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
            .map_err(|_| "unsafe_path".to_string())
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
            Ok(VerifiedPathIdentity {
                canonical_path: self.canonical_child_path(path)?,
                object_id: opened_id,
                metadata: snapshot(&metadata, links),
                content_sha256: None,
            })
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
            self.verify_opened_regular_file(path, &file, false)
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

    // Every production caller sits in a `#[cfg(not(unix))]` arm
    // (`phone::terminal_snapshot` and `pty::terminal_snapshot`); on Unix the
    // method survives only for `terminal_snapshot_unix_child_api_*`.
    #[cfg(any(not(unix), test))]
    pub fn child_is_absent(&self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            match self.open_unix_child(
                path,
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0,
            ) {
                Ok(_) => false,
                Err(error) => unix_child_open_error_is_absent(&error),
            }
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
            if result == 0 {
                Ok(())
            } else {
                Err("atomic_publish_failed".to_string())
            }
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
            if result == 0 {
                Ok(())
            } else {
                Err("atomic_publish_failed".to_string())
            }
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

    #[cfg_attr(
        windows,
        expect(
            clippy::unused_self,
            reason = "En Unix usa self para witness; en Windows usa handles libres. Preservar \
                API uniforme por plataforma."
        )
    )]
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
            matches!(
                self.cleanup_unix_tracked_file(path, expected, &witness),
                UnixTrackedCleanupOutcome::Removed | UnixTrackedCleanupOutcome::AlreadyAbsent
            )
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
            unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    directory,
                    source.as_ptr(),
                    directory,
                    destination.as_ptr(),
                    RENAME_NOREPLACE,
                ) == 0
            }
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
            unsafe {
                renameatx_np(
                    directory,
                    source.as_ptr(),
                    directory,
                    destination.as_ptr(),
                    RENAME_EXCL,
                ) == 0
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (directory, source, destination);
            false
        }
    }

    #[cfg(unix)]
    pub(super) fn checked_unix_child_name<'a>(
        &self,
        path: &'a Path,
    ) -> Result<&'a std::ffi::OsStr, String> {
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
