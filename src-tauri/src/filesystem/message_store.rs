use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub(crate) enum MessagePathError {
    Io(std::io::Error),
    UnsafePath,
}

pub(crate) struct MessageStripeLease {
    _file: File,
}

pub(crate) fn ensure_message_directory(path: &Path) -> Result<(), MessagePathError> {
    std::fs::create_dir_all(path).map_err(MessagePathError::Io)?;
    crate::path_identity::verify_directory(path).map_err(|_| MessagePathError::UnsafePath)?;
    Ok(())
}

pub(crate) fn sqlite_sidecar_paths(path: &Path) -> [PathBuf; 3] {
    let with_suffix = |suffix: &str| {
        let mut value = path.as_os_str().to_os_string();
        value.push(suffix);
        PathBuf::from(value)
    };
    [
        with_suffix("-wal"),
        with_suffix("-shm"),
        with_suffix("-journal"),
    ]
}

pub(crate) fn verify_existing_message_sqlite_files(path: &Path) -> Result<(), MessagePathError> {
    for candidate in std::iter::once(path.to_path_buf()).chain(sqlite_sidecar_paths(path)) {
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {
                crate::path_identity::verify_regular_file(&candidate)
                    .map_err(|_| MessagePathError::UnsafePath)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(MessagePathError::Io(error)),
        }
    }
    Ok(())
}

pub(crate) fn try_message_stripe_lock(
    parent: &Path,
    directory_name: &str,
    filename: impl FnOnce() -> String,
) -> Result<Option<MessageStripeLease>, MessagePathError> {
    let parent_identity =
        crate::path_identity::verify_directory(parent).map_err(|_| MessagePathError::UnsafePath)?;
    let lock_dir = parent.join(directory_name);
    if !lock_dir.exists() {
        match std::fs::create_dir(&lock_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(MessagePathError::Io(error)),
        }
    }
    let lock_dir_identity = crate::path_identity::verify_directory(&lock_dir)
        .map_err(|_| MessagePathError::UnsafePath)?;
    let path = lock_dir.join(filename());
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(0x0002_0000);
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
    let file = options.open(&path).map_err(MessagePathError::Io)?;
    crate::path_identity::verify_opened_regular_file(&path, &file, true)
        .map_err(|_| MessagePathError::UnsafePath)?;
    match file.try_lock() {
        Ok(()) => {
            crate::path_identity::verify_opened_regular_file(&path, &file, true)
                .map_err(|_| MessagePathError::UnsafePath)?;
            let current_parent = crate::path_identity::verify_directory(parent)
                .map_err(|_| MessagePathError::UnsafePath)?;
            let current_lock_dir = crate::path_identity::verify_directory(&lock_dir)
                .map_err(|_| MessagePathError::UnsafePath)?;
            if !crate::path_identity::same_object(&parent_identity, &current_parent)
                || !crate::path_identity::same_object(&lock_dir_identity, &current_lock_dir)
            {
                return Err(MessagePathError::UnsafePath);
            }
            Ok(Some(MessageStripeLease { _file: file }))
        }
        Err(error) => {
            let error: std::io::Error = error.into();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(MessagePathError::Io(error))
            }
        }
    }
}
