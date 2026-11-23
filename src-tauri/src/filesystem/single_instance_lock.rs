//! Linux single-instance lock file. The name and the env read belong to
//! `config::profile`; this module owns the directory rule and the disk I/O.

use std::fs::{DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::io::IntoRawFd;
use std::path::{Path, PathBuf};

/// Lock directory: `$XDG_RUNTIME_DIR` when absolute, else `/tmp/agentscommander-<uid>`.
/// Fixed `/tmp` (not `temp_dir()`) so `TMPDIR` never changes the lock identity.
pub(crate) fn lock_dir(runtime_dir: Option<PathBuf>, uid: u32) -> PathBuf {
    match runtime_dir {
        Some(dir) if dir.is_absolute() => dir,
        _ => PathBuf::from(format!("/tmp/agentscommander-{uid}")),
    }
}

/// Real user ID of this process.
pub(crate) fn current_uid() -> u32 {
    // SAFETY: getuid takes no arguments, cannot fail and only reads process state.
    unsafe { libc::getuid() }
}

/// `false` only when another process holds the lock. Every other failure is
/// fail-open (`true`), like the Windows mutex path in `main.rs`. On success the
/// descriptor is left open on purpose until process exit (kernel releases it).
pub(crate) fn try_acquire(dir: &Path, name: &str, uid: u32) -> bool {
    if !trusted_dir(dir, uid) {
        return true;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let Ok(file) = options.open(dir.join(name)) else {
        return true;
    };
    match file.try_lock() {
        Ok(()) => {
            let _held_until_exit = file.into_raw_fd();
            true
        }
        Err(std::fs::TryLockError::WouldBlock) => false,
        Err(std::fs::TryLockError::Error(_)) => true,
    }
}

/// Create the directory 0700 if missing; trust it only when it is a real
/// directory owned by `uid` without group/other access.
fn trusted_dir(dir: &Path, uid: u32) -> bool {
    match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return false,
    }
    std::fs::symlink_metadata(dir).is_ok_and(|meta| {
        meta.file_type().is_dir() && meta.uid() == uid && meta.mode() & 0o077 == 0
    })
}

#[cfg(test)]
mod tests;
