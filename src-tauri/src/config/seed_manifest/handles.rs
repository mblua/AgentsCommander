use super::{FileIdentity, ManifestDegradedReason, ManifestState, SeedManifestError};
use crate::filesystem::seed_manifest::{self as manifest_disk, ManifestFile as File};
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
    pub(crate) fn from_file(path: &Path, file: impl Into<File>) -> Result<Self, SeedManifestError> {
        let file = file.into();
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
    match manifest_disk::inspect_manifest_path(path) {
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
    match manifest_disk::inspect_manifest_path(path) {
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

pub(crate) use crate::filesystem::seed_manifest::{
    is_windows_reparse, open_directory_no_follow, open_regular_no_follow,
};
pub(crate) fn handle_facts(file: &File) -> io::Result<HandleFacts> {
    let facts = crate::filesystem::seed_manifest::handle_facts(file)?;
    Ok(HandleFacts {
        identity: FileIdentity {
            volume: facts.identity.volume,
            file: facts.identity.file,
        },
        is_directory: facts.is_directory,
        is_regular_file: facts.is_regular_file,
        is_reparse: facts.is_reparse,
        links: facts.links,
        length: facts.length,
        attributes: facts.attributes,
        creation_time: facts.creation_time,
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
