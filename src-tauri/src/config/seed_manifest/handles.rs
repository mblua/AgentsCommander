use super::{FileIdentity, ManifestDegradedReason, ManifestState, SeedManifestError};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::{fmt, io};

#[derive(Debug)]
pub(crate) struct HandleFacts {
    pub(crate) identity: FileIdentity,
    pub(crate) is_directory: bool,
    pub(crate) is_regular_file: bool,
    pub(crate) is_reparse: bool,
    pub(crate) links: u64,
    pub(crate) length: u64,
    pub(crate) attributes: u32,
    pub(crate) creation_time: u64,
}

#[derive(Debug)]
pub(crate) struct PinnedDirectory {
    pub(crate) path: PathBuf,
    pub(crate) file: File,
    pub(crate) identity: FileIdentity,
}

impl PinnedDirectory {
    pub(crate) fn open(path: &Path) -> Result<Self, SeedManifestError> {
        let file = open_directory_no_follow(path).map_err(|source| {
            classify_open_error(path, source, "open directory without following links", true)
        })?;
        let facts = handle_facts(&file).map_err(|source| SeedManifestError::Io {
            operation: "inspect opened directory",
            path: path.to_path_buf(),
            source,
        })?;
        if !facts.is_directory || facts.is_reparse {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "opened handle is not a real non-reparse directory".to_string(),
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
            identity: facts.identity,
        })
    }

    pub(crate) fn revalidate(&self) -> Result<(), SeedManifestError> {
        self.revalidate_at(&self.path)
    }

    pub(crate) fn revalidate_at(&self, path: &Path) -> Result<(), SeedManifestError> {
        let reopened = Self::open(path)?;
        if reopened.identity != self.identity {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "directory identity changed while the project gate was held".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct OpenedRegularFile {
    pub(crate) path: PathBuf,
    pub(crate) file: File,
    pub(crate) identity: FileIdentity,
    pub(crate) links: u64,
    pub(crate) length: u64,
    pub(crate) attributes: u32,
    pub(crate) creation_time: u64,
}

impl OpenedRegularFile {
    pub(crate) fn from_file(path: &Path, file: File) -> Result<Self, SeedManifestError> {
        let facts = handle_facts(&file).map_err(|source| SeedManifestError::Io {
            operation: "inspect opened regular file",
            path: path.to_path_buf(),
            source,
        })?;
        if !facts.is_regular_file || facts.is_reparse {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "opened handle is not a real non-reparse regular file".to_string(),
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
            identity: facts.identity,
            links: facts.links,
            length: facts.length,
            attributes: facts.attributes,
            creation_time: facts.creation_time,
        })
    }

    pub(crate) fn open_existing(path: &Path, writable: bool) -> Result<Self, SeedManifestError> {
        let file = open_regular_no_follow(path, writable, false).map_err(|source| {
            classify_open_error(
                path,
                source,
                "open regular file without following links",
                false,
            )
        })?;
        Self::from_file(path, file)
    }

    pub(crate) fn from_lock_file(path: &Path, file: File) -> Result<Self, SeedManifestError> {
        let facts = handle_facts(&file).map_err(|source| SeedManifestError::LockIo {
            path: path.to_path_buf(),
            saw_contention: false,
            source,
        })?;
        if !facts.is_regular_file || facts.is_reparse {
            return Err(SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "opened lock handle is not a real non-reparse regular file".to_string(),
            });
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
            identity: facts.identity,
            links: facts.links,
            length: facts.length,
            attributes: facts.attributes,
            creation_time: facts.creation_time,
        })
    }

    pub(crate) fn open_existing_lock(path: &Path) -> Result<Self, SeedManifestError> {
        let file = open_regular_no_follow(path, true, false)
            .map_err(|source| classify_lock_open_error(path, source))?;
        Self::from_lock_file(path, file)
    }

    pub(crate) fn create_new(path: &Path) -> Result<Self, SeedManifestError> {
        let file = open_regular_no_follow(path, true, true).map_err(|source| {
            SeedManifestError::TempFile {
                operation: "create-new",
                path: path.to_path_buf(),
                source,
            }
        })?;
        Self::from_file(path, file)
    }

    pub(crate) fn refresh_facts(&mut self) -> Result<(), SeedManifestError> {
        let facts = handle_facts(&self.file).map_err(|source| SeedManifestError::Io {
            operation: "refresh opened regular-file identity",
            path: self.path.clone(),
            source,
        })?;
        if !facts.is_regular_file || facts.is_reparse || facts.identity != self.identity {
            return Err(SeedManifestError::UnsafePath {
                path: self.path.clone(),
                reason: "opened regular-file handle changed type or identity".to_string(),
            });
        }
        self.links = facts.links;
        self.length = facts.length;
        self.attributes = facts.attributes;
        self.creation_time = facts.creation_time;
        Ok(())
    }
}

pub(crate) fn classify_open_error(
    path: &Path,
    source: io::Error,
    operation: &'static str,
    expect_directory: bool,
) -> SeedManifestError {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || is_windows_reparse(&metadata) => {
            SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "path is a symlink, junction, or reparse point".to_string(),
            }
        }
        Ok(metadata)
            if (expect_directory && !metadata.is_dir())
                || (!expect_directory && !metadata.is_file()) =>
        {
            SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: if expect_directory {
                    "path is not a real directory".to_string()
                } else {
                    "path is not a real regular file".to_string()
                },
            }
        }
        _ => SeedManifestError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        },
    }
}

pub(crate) fn classify_lock_open_error(path: &Path, source: io::Error) -> SeedManifestError {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || is_windows_reparse(&metadata) => {
            SeedManifestError::UnsafePath {
                path: path.to_path_buf(),
                reason: "lock path is a symlink, junction, or reparse point".to_string(),
            }
        }
        Ok(metadata) if !metadata.is_file() => SeedManifestError::UnsafePath {
            path: path.to_path_buf(),
            reason: "lock path is not a real regular file".to_string(),
        },
        Ok(_) | Err(_) => SeedManifestError::LockIo {
            path: path.to_path_buf(),
            saw_contention: false,
            source,
        },
    }
}

pub(crate) fn is_exact_not_found(error: &io::Error) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};

        error.raw_os_error().is_some_and(|raw| {
            u32::try_from(raw)
                .map(|raw| raw == ERROR_FILE_NOT_FOUND || raw == ERROR_PATH_NOT_FOUND)
                .unwrap_or(false)
        })
    }

    #[cfg(not(windows))]
    {
        error.kind() == io::ErrorKind::NotFound
    }
}

pub(crate) fn is_windows_reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

#[cfg(unix)]
pub(crate) fn open_directory_no_follow(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(windows)]
pub(crate) fn open_directory_no_follow(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_directory_no_follow(path: &Path) -> io::Result<File> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not a real directory",
        ));
    }
    File::open(path)
}

#[cfg(unix)]
pub(crate) fn open_regular_no_follow(
    path: &Path,
    writable: bool,
    create_new: bool,
) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(writable)
        .custom_flags(libc::O_NOFOLLOW);
    if create_new {
        options.create_new(true);
    }
    options.open(path)
}

#[cfg(windows)]
pub(crate) fn open_regular_no_follow(
    path: &Path,
    writable: bool,
    create_new: bool,
) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(writable)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    if create_new {
        options.create_new(true);
    }
    options.open(path)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_regular_no_follow(
    path: &Path,
    writable: bool,
    create_new: bool,
) -> io::Result<File> {
    if !create_new {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path is not a real regular file",
            ));
        }
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(writable)
        .create_new(create_new)
        .open(path)
}

#[cfg(unix)]
pub(crate) fn handle_facts(file: &File) -> io::Result<HandleFacts> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    Ok(HandleFacts {
        identity: FileIdentity {
            volume: metadata.dev(),
            file: u128::from(metadata.ino()),
        },
        is_directory: metadata.is_dir(),
        is_regular_file: metadata.is_file(),
        is_reparse: false,
        links: metadata.nlink(),
        length: metadata.len(),
        attributes: 0,
        creation_time: 0,
    })
}

#[cfg(windows)]
pub(crate) fn handle_facts(file: &File) -> io::Result<HandleFacts> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let success =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, info.as_mut_ptr()) };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    let length = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    let creation_time = (u64::from(info.ftCreationTime.dwHighDateTime) << 32)
        | u64::from(info.ftCreationTime.dwLowDateTime);
    let is_directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    let is_reparse = info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    Ok(HandleFacts {
        identity: FileIdentity {
            volume: u64::from(info.dwVolumeSerialNumber),
            file: u128::from(index),
        },
        is_directory,
        is_regular_file: !is_directory,
        is_reparse,
        links: u64::from(info.nNumberOfLinks),
        length,
        attributes: info.dwFileAttributes,
        creation_time,
    })
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn handle_facts(file: &File) -> io::Result<HandleFacts> {
    let metadata = file.metadata()?;
    Ok(HandleFacts {
        identity: FileIdentity {
            volume: 0,
            file: u128::from(metadata.len()),
        },
        is_directory: metadata.is_dir(),
        is_regular_file: metadata.is_file(),
        is_reparse: false,
        links: 1,
        length: metadata.len(),
        attributes: 0,
        creation_time: 0,
    })
}

#[derive(Debug)]
pub(crate) struct PinnedOwnerChain {
    pub(crate) directories: Vec<PinnedDirectory>,
}

impl PinnedOwnerChain {
    pub(crate) fn revalidate(&self) -> Result<(), SeedManifestError> {
        for directory in &self.directories {
            directory.revalidate()?;
        }
        Ok(())
    }
}

pub(crate) enum CanonicalSnapshot {
    Writable {
        raw: Vec<u8>,
        state: ManifestState,
        canonical: Option<OpenedRegularFile>,
    },
    ReadOnly {
        reason: ManifestDegradedReason,
        canonical: Option<OpenedRegularFile>,
    },
}

impl fmt::Debug for CanonicalSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Writable {
                raw,
                state,
                canonical,
            } => formatter
                .debug_struct("WritableCanonicalSnapshot")
                .field("raw_len", &raw.len())
                .field("row_count", &state.rows.len())
                .field("canonical_present", &canonical.is_some())
                .finish(),
            Self::ReadOnly { reason, canonical } => formatter
                .debug_struct("ReadOnlyCanonicalSnapshot")
                .field("reason", reason)
                .field("canonical_present", &canonical.is_some())
                .finish(),
        }
    }
}

impl CanonicalSnapshot {
    pub(crate) fn reason(&self) -> Option<ManifestDegradedReason> {
        match self {
            Self::Writable { .. } => None,
            Self::ReadOnly { reason, .. } => Some(*reason),
        }
    }

    pub(crate) fn has_valid_existing_canonical(&self) -> bool {
        matches!(
            self,
            Self::Writable {
                canonical: Some(_),
                ..
            }
        )
    }
}
