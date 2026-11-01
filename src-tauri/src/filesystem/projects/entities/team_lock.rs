//! Physical lock handle and waiting; excludes team config policy and error messages.
use super::deletion::metadata_is_link_or_reparse;
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockStep {
    ResolveWorkspace,
    InspectSentinel,
    OpenExisting,
    CreateSentinel,
    InspectOpened,
    ResolveSentinel,
    Acquire,
    VerifyWorkspace,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LockCause {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("workspace is not a directory")]
    NotDirectory,
    #[error("not a regular file")]
    NonRegular,
    #[error("not a regular non-link file")]
    NonRegularOrLink,
    #[error("missing canonical parent")]
    MissingParent,
    #[error("escaped workspace {canonical_root:?}")]
    EscapedParent { canonical_root: PathBuf },
    #[error("wrong workspace {expected:?}, {actual:?}")]
    WrongWorkspace { expected: PathBuf, actual: PathBuf },
    #[error("timeout after {milliseconds} ms")]
    Timeout { milliseconds: u128 },
}

#[derive(Debug, thiserror::Error)]
#[error("{step:?} at {path:?}: {cause}")]
pub(crate) struct LockFailure {
    pub(crate) step: LockStep,
    pub(crate) path: PathBuf,
    #[source]
    pub(crate) cause: LockCause,
}
impl LockFailure {
    fn new(step: LockStep, path: &Path, cause: LockCause) -> Self {
        Self {
            step,
            path: path.to_path_buf(),
            cause,
        }
    }
}
#[derive(Debug)]
pub(crate) struct LockedTeam {
    _file: File,
    canonical_ac_root: PathBuf,
    lock_path: PathBuf,
}
impl LockedTeam {
    pub(crate) fn canonical_ac_root(&self) -> &Path {
        &self.canonical_ac_root
    }
    pub(crate) fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}
pub(crate) fn acquire(
    ac_root: &Path,
    sentinel_name: &str,
    poll_interval: Duration,
    timeout: Duration,
) -> Result<LockedTeam, LockFailure> {
    let requested_lock_path = ac_root.join(sentinel_name);
    let canonical_ac_root = std::fs::canonicalize(ac_root)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
        .map_err(|e| {
            LockFailure::new(
                LockStep::ResolveWorkspace,
                &requested_lock_path,
                LockCause::Io(e),
            )
        })?;
    if !canonical_ac_root.is_dir() {
        return Err(LockFailure::new(
            LockStep::ResolveWorkspace,
            &requested_lock_path,
            LockCause::NotDirectory,
        ));
    }
    let lock_path = canonical_ac_root.join(sentinel_name);

    let open_existing = || -> Result<File, LockFailure> {
        validate_team_config_lock_sentinel(&lock_path, &canonical_ac_root)?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|open_error| {
                LockFailure::new(
                    LockStep::OpenExisting,
                    &lock_path,
                    LockCause::Io(open_error),
                )
            })
    };
    let file = match std::fs::symlink_metadata(&lock_path) {
        Ok(_) => open_existing()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(file) => file,
                Err(create_error) if create_error.kind() == std::io::ErrorKind::AlreadyExists => {
                    open_existing()?
                }
                Err(create_error) => {
                    return Err(LockFailure::new(
                        LockStep::CreateSentinel,
                        &lock_path,
                        LockCause::Io(create_error),
                    ));
                }
            }
        }
        Err(e) => {
            return Err(LockFailure::new(
                LockStep::InspectSentinel,
                &lock_path,
                LockCause::Io(e),
            ));
        }
    };
    validate_team_config_lock_sentinel(&lock_path, &canonical_ac_root)?;
    if !file
        .metadata()
        .map_err(|e| LockFailure::new(LockStep::InspectOpened, &lock_path, LockCause::Io(e)))?
        .is_file()
    {
        return Err(LockFailure::new(
            LockStep::InspectOpened,
            &lock_path,
            LockCause::NonRegular,
        ));
    }

    wait_for_lock(&file, &lock_path, poll_interval, timeout)?;

    Ok(LockedTeam {
        _file: file,
        canonical_ac_root,
        lock_path,
    })
}

fn wait_for_lock(
    file: &File,
    lock_path: &Path,
    poll_interval: Duration,
    timeout: Duration,
) -> Result<(), LockFailure> {
    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if started.elapsed() >= timeout => {
                return Err(LockFailure::new(
                    LockStep::Acquire,
                    lock_path,
                    LockCause::Timeout {
                        milliseconds: timeout.as_millis(),
                    },
                ));
            }
            Err(TryLockError::WouldBlock) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                let delay = poll_interval.min(remaining);
                #[cfg(test)]
                tests::observe_delay(delay, remaining);
                if delay.is_zero() {
                    std::thread::yield_now();
                } else {
                    std::thread::sleep(delay);
                }
            }
            Err(TryLockError::Error(e)) => {
                return Err(LockFailure::new(
                    LockStep::Acquire,
                    lock_path,
                    LockCause::Io(e),
                ));
            }
        }
    }
    Ok(())
}

impl LockedTeam {
    pub(crate) fn require_workspace(&self, ac_root: &Path) -> Result<(), LockFailure> {
        let canonical = std::fs::canonicalize(ac_root)
            .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
            .map_err(|e| {
                LockFailure::new(
                    LockStep::VerifyWorkspace,
                    self.lock_path(),
                    LockCause::Io(e),
                )
            })?;
        if canonical != self.canonical_ac_root() {
            return Err(LockFailure::new(
                LockStep::VerifyWorkspace,
                &self.lock_path,
                LockCause::WrongWorkspace {
                    expected: self.canonical_ac_root.clone(),
                    actual: canonical,
                },
            ));
        }
        Ok(())
    }
}

fn validate_team_config_lock_sentinel(
    lock_path: &Path,
    canonical_ac_root: &Path,
) -> Result<(), LockFailure> {
    let metadata = std::fs::symlink_metadata(lock_path)
        .map_err(|e| LockFailure::new(LockStep::InspectSentinel, lock_path, LockCause::Io(e)))?;
    if metadata_is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(LockFailure::new(
            LockStep::InspectSentinel,
            lock_path,
            LockCause::NonRegularOrLink,
        ));
    }
    let canonical_lock = std::fs::canonicalize(lock_path)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
        .map_err(|e| LockFailure::new(LockStep::ResolveSentinel, lock_path, LockCause::Io(e)))?;
    let canonical_parent = canonical_lock.parent().ok_or_else(|| {
        LockFailure::new(
            LockStep::ResolveSentinel,
            lock_path,
            LockCause::MissingParent,
        )
    })?;
    if canonical_parent != canonical_ac_root {
        return Err(LockFailure::new(
            LockStep::ResolveSentinel,
            lock_path,
            LockCause::EscapedParent {
                canonical_root: canonical_ac_root.to_path_buf(),
            },
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
