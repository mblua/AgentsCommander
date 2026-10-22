#[cfg(not(any(unix, windows)))]
use super::atomic_publish_manifest;
use super::{
    classify_lock_open_error, is_exact_not_found, open_regular_no_follow, FileIdentity,
    OpenedRegularFile, SeedManifestError, MAX_LOCK_POLL, RAW_COMPARE_BUFFER_BYTES,
    SEED_MANIFEST_TEMP_PREFIX, SEED_MANIFEST_TEMP_SUFFIX,
};
use std::fs::File;
use std::io;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Instant;
use uuid::Uuid;

pub(crate) fn open_or_create_lock_file(
    path: &Path,
    deadline: Instant,
) -> Result<File, SeedManifestError> {
    loop {
        match open_regular_no_follow(path, true, false) {
            Ok(file) => {
                let opened = OpenedRegularFile::from_lock_file(path, file)?;
                return Ok(opened.file);
            }
            Err(error) if is_exact_not_found(&error) => {
                match open_regular_no_follow(path, true, true) {
                    Ok(file) => {
                        let opened = OpenedRegularFile::from_lock_file(path, file)?;
                        return Ok(opened.file);
                    }
                    Err(create_error) if create_error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(create_error) => {
                        return Err(classify_lock_open_error(path, create_error));
                    }
                }
            }
            Err(error) => {
                return Err(classify_lock_open_error(path, error));
            }
        }
        if Instant::now() >= deadline {
            return Err(SeedManifestError::LockIo {
                path: path.to_path_buf(),
                saw_contention: false,
                source: io::Error::new(
                    io::ErrorKind::TimedOut,
                    "lock-file create/open race did not stabilize before the deadline",
                ),
            });
        }
    }
}

pub(crate) fn acquire_kernel_lock(
    file: &File,
    path: &Path,
    started: Instant,
    deadline: Instant,
) -> Result<(), SeedManifestError> {
    let mut saw_contention = false;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(()),
            Err(std::fs::TryLockError::WouldBlock) => {
                saw_contention = true;
            }
            Err(std::fs::TryLockError::Error(source)) => {
                return Err(SeedManifestError::LockIo {
                    path: path.to_path_buf(),
                    saw_contention,
                    source,
                });
            }
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(SeedManifestError::BusyTimeout {
                path: path.to_path_buf(),
                waited: started.elapsed(),
            });
        }
        let remaining = deadline.saturating_duration_since(now);
        std::thread::sleep(remaining.min(MAX_LOCK_POLL));
    }
}

pub(crate) fn is_exact_temp_name(name: &str) -> bool {
    let Some(uuid_text) = name
        .strip_prefix(SEED_MANIFEST_TEMP_PREFIX)
        .and_then(|name| name.strip_suffix(SEED_MANIFEST_TEMP_SUFFIX))
    else {
        return false;
    };
    Uuid::parse_str(uuid_text)
        .map(|uuid| uuid.hyphenated().to_string() == uuid_text)
        .unwrap_or(false)
}

#[cfg(unix)]
pub(crate) fn native_name_has_temp_shape(name: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;

    let bytes = name.as_bytes();
    bytes.starts_with(SEED_MANIFEST_TEMP_PREFIX.as_bytes())
        && bytes.ends_with(SEED_MANIFEST_TEMP_SUFFIX.as_bytes())
}

#[cfg(windows)]
pub(crate) fn native_name_has_temp_shape(name: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;

    let units = name.encode_wide().collect::<Vec<_>>();
    let prefix = SEED_MANIFEST_TEMP_PREFIX.encode_utf16().collect::<Vec<_>>();
    let suffix = SEED_MANIFEST_TEMP_SUFFIX.encode_utf16().collect::<Vec<_>>();
    units.starts_with(&prefix) && units.ends_with(&suffix)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn native_name_has_temp_shape(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.starts_with(SEED_MANIFEST_TEMP_PREFIX) && name.ends_with(SEED_MANIFEST_TEMP_SUFFIX)
    })
}

pub(crate) fn remove_if_same_identity(
    path: &Path,
    expected: FileIdentity,
) -> Result<(), SeedManifestError> {
    let reopened = OpenedRegularFile::open_existing(path, false)?;
    if reopened.identity != expected || reopened.links != 1 {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "temp identity changed or acquired another hard link before removal"
                .to_string(),
        });
    }
    std::fs::remove_file(path).map_err(|source| SeedManifestError::TempFile {
        operation: "remove unchanged temp",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(windows))]
pub(crate) fn cleanup_current_owned_temp(
    temp: &OpenedRegularFile,
    _expected_bytes: Option<&[u8]>,
) -> Result<(), SeedManifestError> {
    remove_if_same_identity(&temp.path, temp.identity)
}

#[cfg(unix)]
pub(crate) fn remove_prior_unix_temp_if_safe(path: &Path) -> Result<(), SeedManifestError> {
    let first = OpenedRegularFile::open_existing(path, false)?;
    if first.links != 1 {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "prior temp has more than one hard link".to_string(),
        });
    }
    let second = OpenedRegularFile::open_existing(path, false)?;
    if second.identity != first.identity || second.links != 1 {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "prior temp path identity changed during cleanup proof".to_string(),
        });
    }
    std::fs::remove_file(path).map_err(|source| SeedManifestError::TempFile {
        operation: "remove proved stale Unix temp",
        path: path.to_path_buf(),
        source,
    })
}

pub(crate) fn verify_temp_path(
    path: &Path,
    expected: &OpenedRegularFile,
    bytes: &[u8],
) -> Result<(), SeedManifestError> {
    let mut reopened = OpenedRegularFile::open_existing(path, false)?;
    if reopened.identity != expected.identity || reopened.links != 1 {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "temp identity changed or has more than one hard link".to_string(),
        });
    }
    if reopened.length != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        || !stream_matches(&mut reopened.file, bytes).map_err(|source| {
            SeedManifestError::TempFile {
                operation: "verify temp bytes",
                path: path.to_path_buf(),
                source,
            }
        })?
    {
        return Err(SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "temp length or bytes changed before atomic publication".to_string(),
        });
    }
    Ok(())
}

pub(crate) fn stream_matches(file: &mut File, expected: &[u8]) -> io::Result<bool> {
    file.seek(SeekFrom::Start(0))?;
    let mut offset = 0_usize;
    let mut buffer = [0_u8; RAW_COMPARE_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(offset == expected.len());
        }
        let Some(end) = offset.checked_add(read) else {
            return Ok(false);
        };
        if end > expected.len() || buffer[..read] != expected[offset..end] {
            return Ok(false);
        }
        offset = end;
    }
}

#[cfg(unix)]
pub(crate) fn atomic_publish_manifest(
    temp: &Path,
    canonical: &Path,
    _had_canonical: bool,
) -> Result<(), SeedManifestError> {
    std::fs::rename(temp, canonical).map_err(|source| SeedManifestError::AtomicReplace {
        operation: "rename",
        temp: temp.to_path_buf(),
        canonical: canonical.to_path_buf(),
        source,
    })
}
