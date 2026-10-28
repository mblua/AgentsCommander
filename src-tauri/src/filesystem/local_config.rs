//! Physical local-config sidecar locks, staging and publication operations.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CONFIG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// #1938 - a held sidecar lock. The `File` IS the lock: dropping this guard
/// closes the handle and releases the OS lock. The sidecar file itself is
/// deliberately left on disk.
#[derive(Debug)]
pub(crate) struct SidecarWriteLock {
    _file: std::fs::File,
}

pub(crate) fn ensure_local_config_parent(parent: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(parent)
}

pub(crate) fn canonical_local_config_parent(parent: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(parent)
}

pub(crate) fn local_config_target_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn rename_local_config_stage(stage: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(stage, target)
}

pub(crate) fn remove_local_config_stage(stage: &Path) -> std::io::Result<()> {
    std::fs::remove_file(stage)
}

#[cfg(not(windows))]
pub(crate) fn sync_local_config_directory(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent).and_then(|directory| directory.sync_all())
}

#[cfg(windows)]
pub(crate) fn replace_local_config_once(
    path_wide: &[u16],
    tmp_wide: &[u16],
) -> Result<(), std::io::Error> {
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};
    let ok = unsafe {
        ReplaceFileW(
            path_wide.as_ptr(),
            tmp_wide.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

pub(crate) fn acquire_local_config_sidecar_lock(
    lock_path: &Path,
    timeout: Duration,
    timeout_marker: &str,
    subject: &str,
) -> Result<SidecarWriteLock, String> {
    let mut chars = subject.chars();
    let subject_capitalized: String = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    match std::fs::symlink_metadata(lock_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!(
                "{} '{}' must be a regular non-symlink file",
                subject_capitalized,
                lock_path.display()
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to inspect {} '{}': {}",
                subject,
                lock_path.display(),
                e
            ));
        }
    }

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| {
            format!(
                "Failed to open {} '{}': {}",
                subject,
                lock_path.display(),
                e
            )
        })?;
    if !file
        .metadata()
        .map_err(|e| {
            format!(
                "Failed to inspect opened {} '{}': {}",
                subject,
                lock_path.display(),
                e
            )
        })?
        .is_file()
    {
        return Err(format!(
            "{} '{}' must be a regular file",
            subject_capitalized,
            lock_path.display()
        ));
    }

    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() >= timeout => {
                return Err(format!(
                    "{}: timed out after {} ms waiting for {} '{}'",
                    timeout_marker,
                    timeout.as_millis(),
                    subject,
                    lock_path.display()
                ));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                std::thread::sleep(CONFIG_LOCK_POLL_INTERVAL.min(remaining));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(format!(
                    "Failed to acquire {} '{}': {}",
                    subject,
                    lock_path.display(),
                    e
                ));
            }
        }
    }

    Ok(SidecarWriteLock { _file: file })
}

/// Owned stage handle. Dropping it only closes the file.
#[derive(Debug)]
pub(crate) struct LocalConfigStage {
    file: std::fs::File,
}

pub(crate) fn read_local_config_text(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn inspect_local_config_target(path: &Path) -> std::io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn read_local_config_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn create_local_config_stage(path: &Path) -> std::io::Result<LocalConfigStage> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|file| LocalConfigStage { file })
}

pub(crate) fn reopen_local_config_source_stage(path: &Path) -> std::io::Result<LocalConfigStage> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map(|file| LocalConfigStage { file })
}

pub(crate) fn write_local_config_stage(
    stage: &mut LocalConfigStage,
    bytes: &[u8],
) -> std::io::Result<()> {
    stage.file.write_all(bytes)
}

pub(crate) fn flush_local_config_stage(stage: &mut LocalConfigStage) -> std::io::Result<()> {
    stage.file.flush()
}

pub(crate) fn sync_local_config_stage(stage: &LocalConfigStage) -> std::io::Result<()> {
    stage.file.sync_all()
}

pub(crate) fn read_local_config_source_stage(
    stage: &mut LocalConfigStage,
) -> std::io::Result<Vec<u8>> {
    stage.file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    stage.file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(crate) fn verify_opened_local_config_stage(
    parent: &crate::path_identity::RetainedDirectory,
    path: &Path,
    stage: &LocalConfigStage,
    require_empty: bool,
) -> Result<crate::path_identity::VerifiedPathIdentity, String> {
    parent.verify_opened_regular_file(path, &stage.file, require_empty)
}

pub(crate) fn link_absent_local_config_stage(stage: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::hard_link(stage, target)
}
