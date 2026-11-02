//! Physical Matrix copy, ownership and resource sessions.
use super::deletion::{metadata_is_link_or_reparse, metadata_parts_are_link_or_reparse};
use super::roles;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatrixCopyKind {
    Directory,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MatrixCopyStamp {
    pub(crate) identity: Option<MatrixCopyIdentity>,
    pub(crate) kind: MatrixCopyKind,
    pub(crate) size: u64,
    pub(crate) modified: std::time::SystemTime,
    pub(crate) digest: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MatrixCopyIdentity(pub(crate) u64, pub(crate) u64);

#[derive(Debug)]
pub(crate) struct OwnedCopyEntry {
    pub(crate) relative: PathBuf,
    pub(crate) identity: MatrixCopyIdentity,
    pub(crate) kind: MatrixCopyKind,
    pub(crate) digest: Option<[u8; 32]>,
}

#[derive(Debug)]
pub(crate) struct OwnedCopyArtifacts {
    pub(crate) root: PathBuf,
    pub(crate) entries: Vec<OwnedCopyEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MatrixCopyStage {
    Reserved,
    Classify,
    Streaming,
    BeforeVerify,
    ConfigWritten,
    Provenance,
    ProvenanceWritten,
    BeforeCleanup,
    TeamConventions,
    TeamPublication,
    TeamUnlink,
}

#[cfg(test)]
pub(crate) type MatrixCopyCallback<'a> =
    dyn FnMut(MatrixCopyStage, &Path) -> Result<(), String> + 'a;

#[derive(Default)]
pub(crate) struct MatrixCopyHooks<'a> {
    #[cfg(test)]
    pub(crate) callback: Option<&'a mut MatrixCopyCallback<'a>>,
    #[cfg(test)]
    pub(crate) classification: Option<(bool, u32, bool, bool)>,
    #[cfg(test)]
    pub(crate) observer: Option<LeaseObserver>,
    pub(crate) marker: std::marker::PhantomData<&'a ()>,
}

impl MatrixCopyHooks<'_> {
    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "self selecciona callback cfg(test); conservar el préstamo mutable y \
                orden del seam."
        )
    )]
    pub(crate) fn run(&mut self, stage: MatrixCopyStage, path: &Path) -> Result<(), String> {
        #[cfg(test)]
        if let Some(callback) = &mut self.callback {
            return callback(stage, path);
        }
        let _ = (stage, path);
        Ok(())
    }
}

impl OwnedCopyArtifacts {
    #[cfg(test)]
    pub(crate) fn handle_identity(file: &File) -> Result<MatrixCopyIdentity, String> {
        Self::identity_of_file(file)
    }

    fn identity_of_file(file: &File) -> Result<MatrixCopyIdentity, String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = file.metadata().map_err(|e| e.to_string())?;
            Ok(MatrixCopyIdentity(metadata.dev(), metadata.ino()))
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
            };
            let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
            if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) }
                == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let information = unsafe { information.assume_init() };
            Ok(MatrixCopyIdentity(
                u64::from(information.dwVolumeSerialNumber),
                (u64::from(information.nFileIndexHigh) << 32)
                    | u64::from(information.nFileIndexLow),
            ))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = file;
            Err("Matrix copy identity unsupported on this platform".into())
        }
    }

    fn kind(metadata: &std::fs::Metadata) -> Result<MatrixCopyKind, String> {
        if metadata_is_link_or_reparse(metadata) {
            return Err("Matrix copy rejects links/reparse points".into());
        }
        Self::classify(false, 0, metadata.is_dir(), metadata.is_file())
    }

    fn classify(
        symlink: bool,
        attributes: u32,
        directory: bool,
        file: bool,
    ) -> Result<MatrixCopyKind, String> {
        if metadata_parts_are_link_or_reparse(symlink, attributes) {
            return Err("Matrix copy rejects links/reparse points".into());
        }
        if directory {
            Ok(MatrixCopyKind::Directory)
        } else if file {
            Ok(MatrixCopyKind::File)
        } else {
            Err("Matrix copy rejects special entries".into())
        }
    }

    fn source_kind(
        metadata: &std::fs::Metadata,
        hooks: &MatrixCopyHooks<'_>,
    ) -> Result<MatrixCopyKind, String> {
        #[cfg(test)]
        if let Some((symlink, attributes, directory, file)) = hooks.classification {
            return Self::classify(symlink, attributes, directory, file);
        }
        let _ = hooks;
        Self::kind(metadata)
    }

    fn open_existing(path: &Path, kind: MatrixCopyKind) -> Result<File, String> {
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if Self::kind(&metadata)? != kind {
            return Err(format!("Unexpected copy entry type: {}", path.display()));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
                FILE_SHARE_READ, FILE_SHARE_WRITE,
            };
            options
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
            if kind == MatrixCopyKind::Directory {
                options.access_mode(0);
            }
        }
        let file = options.open(path).map_err(|e| e.to_string())?;
        if Self::kind(&file.metadata().map_err(|e| e.to_string())?)? != kind {
            return Err("Opened copy entry has unexpected type".into());
        }
        Ok(file)
    }

    pub(crate) fn verify_directories(&self, relative: &Path) -> Result<(), String> {
        for entry in &self.entries {
            if entry.kind == MatrixCopyKind::Directory && relative.starts_with(&entry.relative) {
                let path = self.root.join(&entry.relative);
                let file = Self::open_existing(&path, MatrixCopyKind::Directory)?;
                if Self::identity_of_file(&file)? != entry.identity {
                    return Err(format!("Copy directory replaced: {}", path.display()));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn create_directory(&mut self, relative: &Path) -> Result<(), String> {
        self.verify_directories(relative)?;
        let path = self.root.join(relative);
        std::fs::create_dir(&path).map_err(|e| e.to_string())?;
        // If capture fails, the unrecorded directory is deliberately retained.
        let file = Self::open_existing(&path, MatrixCopyKind::Directory)
            .map_err(|e| format!("{e}; residue: {}", path.display()))?;
        let identity = Self::identity_of_file(&file)
            .map_err(|e| format!("{e}; residue: {}", path.display()))?;
        self.entries.push(OwnedCopyEntry {
            relative: relative.to_path_buf(),
            identity,
            kind: MatrixCopyKind::Directory,
            digest: None,
        });
        Ok(())
    }

    fn create_file(&mut self, relative: &Path) -> Result<File, String> {
        self.verify_directories(relative)?;
        let path = self.root.join(relative);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let identity = Self::identity_of_file(&file)
            .map_err(|e| format!("{e}; residue: {}", path.display()))?;
        self.entries.push(OwnedCopyEntry {
            relative: relative.to_path_buf(),
            identity,
            kind: MatrixCopyKind::File,
            digest: Some(Self::digest_bytes(&[])),
        });
        Ok(file)
    }

    pub(crate) fn digest_bytes(bytes: &[u8]) -> [u8; 32] {
        use sha2::Digest;
        sha2::Sha256::digest(bytes).into()
    }

    #[cfg(test)]
    pub(crate) fn stamp(
        metadata: &std::fs::Metadata,
        digest: Option<[u8; 32]>,
    ) -> Result<MatrixCopyStamp, String> {
        Self::stamp_metadata(metadata, digest)
    }

    fn stamp_metadata(
        metadata: &std::fs::Metadata,
        digest: Option<[u8; 32]>,
    ) -> Result<MatrixCopyStamp, String> {
        Ok(MatrixCopyStamp {
            identity: None,
            kind: Self::kind(metadata)?,
            size: metadata.len(),
            modified: metadata.modified().map_err(|e| e.to_string())?,
            digest,
        })
    }

    fn read_file(
        path: &Path,
        mut output: Option<(&mut File, &mut OwnedCopyEntry)>,
        mut buffer: Option<&mut Vec<u8>>,
        hooks: &mut MatrixCopyHooks<'_>,
    ) -> Result<MatrixCopyStamp, String> {
        use sha2::Digest;
        use std::io::{Read, Write};
        hooks.run(MatrixCopyStage::Classify, path)?;
        let before = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if Self::source_kind(&before, hooks)? != MatrixCopyKind::File {
            return Err("Expected regular Matrix file".into());
        }
        let mut input = Self::open_existing(path, MatrixCopyKind::File)?;
        if Self::stamp_metadata(&before, None)?
            != Self::stamp_metadata(&input.metadata().map_err(|e| e.to_string())?, None)?
        {
            return Err("Matrix file changed before reading".into());
        }
        let identity = Self::identity_of_file(&input)?;
        let mut digest = sha2::Sha256::new();
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let count = input.read(&mut chunk).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            if let Some((file, entry)) = output.as_mut() {
                let mut remaining = &chunk[..count];
                while !remaining.is_empty() {
                    let written = file.write(remaining).map_err(|e| e.to_string())?;
                    if written == 0 {
                        return Err("Matrix copy write made no progress".into());
                    }
                    digest.update(&remaining[..written]);
                    entry.digest = Some(digest.clone().finalize().into());
                    remaining = &remaining[written..];
                }
            } else {
                digest.update(&chunk[..count]);
            }
            if let Some(bytes) = &mut buffer {
                bytes.extend_from_slice(&chunk[..count]);
            }
            hooks.run(MatrixCopyStage::Streaming, path)?;
        }
        let after = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        let before_stamp = Self::stamp_metadata(&before, None)?;
        if before_stamp != Self::stamp_metadata(&after, None)?
            || before_stamp
                != Self::stamp_metadata(&input.metadata().map_err(|e| e.to_string())?, None)?
        {
            return Err(format!("Matrix file changed: {}", path.display()));
        }
        let current = Self::open_existing(path, MatrixCopyKind::File)?;
        if Self::identity_of_file(&current)? != identity {
            return Err("Matrix file replaced during reading".into());
        }
        let mut stamp = Self::stamp_metadata(&after, Some(digest.finalize().into()))?;
        stamp.identity = Some(identity);
        Ok(stamp)
    }

    pub(crate) fn selected_root_entry(
        path: &Path,
        name: &str,
        hooks: &MatrixCopyHooks<'_>,
    ) -> Result<bool, String> {
        let required = match name {
            _ if roles::is_copy_role_entry(name) => Some(MatrixCopyKind::File),
            "skills" => Some(MatrixCopyKind::Directory),
            _ if name.contains("default") => None,
            _ => return Ok(false),
        };
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        let kind = Self::source_kind(&metadata, hooks)?;
        if roles::is_copy_role_entry(name) {
            roles::validate_copy_role_kind(kind == MatrixCopyKind::File)?;
            return Ok(true);
        }
        if let Some(expected) = required {
            if kind != expected {
                return Err(format!("Unexpected generic asset type: {name}"));
            }
            return Ok(true);
        }
        Ok(kind == MatrixCopyKind::Directory)
    }

    pub(crate) fn scan(
        source: &Path,
        relative: &Path,
        mut output: Option<&mut Self>,
        inventory: &mut std::collections::BTreeMap<PathBuf, MatrixCopyStamp>,
        hooks: &mut MatrixCopyHooks<'_>,
    ) -> Result<(), String> {
        let path = source.join(relative);
        hooks.run(MatrixCopyStage::Classify, &path)?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        let kind = Self::source_kind(&metadata, hooks)?;
        if kind == MatrixCopyKind::Directory {
            let root = relative.as_os_str().is_empty();
            let handle = Self::open_existing(&path, kind)?;
            let identity = Self::identity_of_file(&handle)?;
            let mut stamp = Self::stamp_metadata(&metadata, None)?;
            stamp.identity = Some(identity);
            if !root {
                inventory.insert(relative.to_path_buf(), stamp.clone());
                if let Some(owned) = &mut output {
                    owned.create_directory(relative)?;
                }
            }
            let mut children = std::fs::read_dir(&path)
                .map_err(|e| e.to_string())?
                .map(|entry| entry.map(|e| e.file_name()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            children.sort();
            let mut role = false;
            for child in children {
                if root {
                    let name = child.to_str().ok_or("Non-UTF-8 Matrix root entry")?;
                    role |= roles::is_copy_role_entry(name);
                    if !Self::selected_root_entry(&path.join(&child), name, hooks)? {
                        continue;
                    }
                }
                Self::scan(
                    source,
                    &relative.join(child),
                    output.as_deref_mut(),
                    inventory,
                    hooks,
                )?;
            }
            if root {
                roles::require_copy_role(role)?;
            }
            let after = Self::open_existing(&path, kind)?;
            if Self::identity_of_file(&after)? != identity {
                return Err("Matrix directory replaced".into());
            }
            let mut after_stamp =
                Self::stamp_metadata(&after.metadata().map_err(|e| e.to_string())?, None)?;
            after_stamp.identity = Some(identity);
            if !root && stamp != after_stamp {
                return Err("Matrix directory changed".into());
            }
        } else {
            let stamp = if let Some(owned) = output {
                let mut file = owned.create_file(relative)?;
                let entry = owned.entries.last_mut().expect("created file entry");
                Self::read_file(&path, Some((&mut file, entry)), None, hooks)?
            } else {
                Self::read_file(&path, None, None, hooks)?
            };
            inventory.insert(relative.to_path_buf(), stamp);
        }
        Ok(())
    }
}

pub(crate) fn cleanup_owned_copy_artifacts(owned: &OwnedCopyArtifacts) -> Result<(), String> {
    let mut residues = Vec::new();
    for entry in owned.entries.iter().rev() {
        let path = owned.root.join(&entry.relative);
        let remove = (|| -> Result<(), String> {
            owned.verify_directories(&entry.relative)?;
            let file = OwnedCopyArtifacts::open_existing(&path, entry.kind)?;
            if OwnedCopyArtifacts::identity_of_file(&file)? != entry.identity {
                return Err("copy output replaced".into());
            }
            if entry.kind == MatrixCopyKind::File {
                let stamp = OwnedCopyArtifacts::read_file(
                    &path,
                    None,
                    None,
                    &mut MatrixCopyHooks::default(),
                )?;
                if stamp.digest != entry.digest {
                    return Err("copy output modified".into());
                }
                drop(file);
                owned.verify_directories(&entry.relative)?;
                let current = OwnedCopyArtifacts::open_existing(&path, entry.kind)?;
                if OwnedCopyArtifacts::identity_of_file(&current)? != entry.identity {
                    return Err("copy output replaced".into());
                }
                drop(current);
                std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            } else {
                drop(file);
                owned.verify_directories(&entry.relative)?;
                std::fs::remove_dir(&path).map_err(|e| e.to_string())?;
            }
            Ok(())
        })();
        if let Err(error) = remove {
            residues.push(format!("{}: {error}", path.display()));
        }
    }
    if residues.is_empty() {
        Ok(())
    } else {
        Err(format!("copy cleanup residues: {}", residues.join("; ")))
    }
}

pub(crate) fn verify_owned_entry(
    owned: &OwnedCopyArtifacts,
    entry: &OwnedCopyEntry,
) -> Result<(), String> {
    owned.verify_directories(&entry.relative)?;
    let path = owned.root.join(&entry.relative);
    let handle = OwnedCopyArtifacts::open_existing(&path, entry.kind)?;
    if OwnedCopyArtifacts::identity_of_file(&handle)? != entry.identity {
        return Err(format!("Output replaced: {}", path.display()));
    }
    if entry.kind == MatrixCopyKind::File {
        let stamp =
            OwnedCopyArtifacts::read_file(&path, None, None, &mut MatrixCopyHooks::default())?;
        if stamp.digest != entry.digest {
            return Err(format!("Output modified: {}", path.display()));
        }
    }
    Ok(())
}

pub(crate) struct SourceMatrixSession {
    #[cfg(not(test))]
    source_handle: File,
    #[cfg(test)]
    source_handle: ObservedFile,
    source_identity: MatrixCopyIdentity,
}

pub(crate) struct VerifiedSourceRoot {
    #[cfg(not(test))]
    _handle: File,
    #[cfg(test)]
    _handle: ObservedFile,
}

pub(crate) fn open_source_matrix(
    path: &Path,
    #[cfg(test)] observer: Option<&LeaseObserver>,
) -> Result<SourceMatrixSession, String> {
    if OwnedCopyArtifacts::kind(&std::fs::symlink_metadata(path).map_err(|e| e.to_string())?)?
        != MatrixCopyKind::Directory
    {
        return Err("Source Matrix must be a directory".into());
    }
    let source_handle = OwnedCopyArtifacts::open_existing(path, MatrixCopyKind::Directory)?;
    #[cfg(test)]
    let source_handle = ObservedFile::new(source_handle, observer, "Source");
    let source_identity = OwnedCopyArtifacts::identity_of_file(&source_handle)?;
    Ok(SourceMatrixSession {
        source_handle,
        source_identity,
    })
}

impl SourceMatrixSession {
    pub(crate) fn reopen_and_verify_root(
        &self,
        path: &Path,
        #[cfg(test)] observer: Option<&LeaseObserver>,
    ) -> Result<VerifiedSourceRoot, String> {
        let current = OwnedCopyArtifacts::open_existing(path, MatrixCopyKind::Directory)?;
        #[cfg(test)]
        let current = ObservedFile::new(current, observer, "Current");
        if OwnedCopyArtifacts::identity_of_file(&current)? != self.source_identity {
            return Err("Source Matrix root replaced".into());
        }
        // Keep the original root lease until the source session leaves scope.
        let _ = &self.source_handle;
        Ok(VerifiedSourceRoot { _handle: current })
    }
}

pub(crate) fn identity_existing_directory(path: &Path) -> Result<MatrixCopyIdentity, String> {
    let handle = OwnedCopyArtifacts::open_existing(path, MatrixCopyKind::Directory)?;
    OwnedCopyArtifacts::identity_of_file(&handle)
}

pub(crate) fn with_source_parent<T, F>(
    matrix: &Path,
    source_ac: &Path,
    #[cfg(test)] observer: Option<&LeaseObserver>,
    body: F,
) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String>,
{
    let handle = OwnedCopyArtifacts::open_existing(matrix, MatrixCopyKind::Directory)?;
    drop(handle);
    let canonical_matrix = matrix.canonicalize().map_err(|e| e.to_string())?;
    let parent = canonical_matrix
        .parent()
        .ok_or("Source Matrix has no parent")?;
    let parent_handle = OwnedCopyArtifacts::open_existing(parent, MatrixCopyKind::Directory)?;
    #[cfg(test)]
    let parent_handle = ObservedFile::new(parent_handle, observer, "Parent");
    let ac_handle = OwnedCopyArtifacts::open_existing(source_ac, MatrixCopyKind::Directory)?;
    #[cfg(test)]
    let ac_handle = ObservedFile::new(ac_handle, observer, "Ac");
    if OwnedCopyArtifacts::identity_of_file(&parent_handle)?
        != OwnedCopyArtifacts::identity_of_file(&ac_handle)?
    {
        return Err("Source Matrix is not a direct project child".into());
    }
    body()
}

pub(crate) fn canonical_source_ac(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|e| e.to_string())
}

pub(crate) fn canonical_source_matrix(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|e| e.to_string())
}

pub(crate) fn canonical_destination_ac(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|e| e.to_string())
}

pub(crate) fn read_copy_history(root: &Path, relative: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(root.join(relative)).map_err(|e| e.to_string())
}

pub(crate) struct OwnedPayloadSession<'a> {
    file: File,
    entry: &'a mut OwnedCopyEntry,
}

impl OwnedCopyArtifacts {
    pub(crate) fn create_payload(
        &mut self,
        relative: &Path,
    ) -> Result<OwnedPayloadSession<'_>, String> {
        let file = self.create_file(relative)?;
        let entry = self.entries.last_mut().expect("created payload entry");
        Ok(OwnedPayloadSession { file, entry })
    }
}

impl OwnedPayloadSession<'_> {
    pub(crate) fn write_payload(
        &mut self,
        bytes: &[u8],
        no_progress: &str,
        mut after_write: impl FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        use std::io::Write;
        let mut written = 0;
        while written < bytes.len() {
            let count = self
                .file
                .write(&bytes[written..])
                .map_err(|e| e.to_string())?;
            if count == 0 {
                return Err(no_progress.into());
            }
            written += count;
            self.entry.digest = Some(OwnedCopyArtifacts::digest_bytes(&bytes[..written]));
            after_write()?;
        }
        self.file.flush().map_err(|e| e.to_string())?;
        self.file.sync_all().map_err(|e| e.to_string())?;
        Ok(())
    }
}

pub(crate) fn write_owned_team_file(
    owned: &mut OwnedCopyArtifacts,
    relative: &Path,
    bytes: &[u8],
) -> Result<(), String> {
    owned
        .create_payload(relative)?
        .write_payload(bytes, "Team file write made no progress", || Ok(()))
}

pub(crate) fn require_absent(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
        Ok(_) => Err(format!("Destination already exists: {}", path.display())),
    }
}

// Only test builds observe the lifetime of the actual opened files.
#[cfg(test)]
type ObservedLeases = Vec<(&'static str, std::rc::Weak<()>)>;

#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct LeaseObserver {
    leases: std::rc::Rc<std::cell::RefCell<ObservedLeases>>,
}

#[cfg(test)]
impl LeaseObserver {
    pub(crate) fn states(&self) -> Vec<(&'static str, usize, bool)> {
        self.leases
            .borrow()
            .iter()
            .enumerate()
            .map(|(ordinal, (role, token))| (*role, ordinal, token.strong_count() > 0))
            .collect()
    }
}

#[cfg(test)]
struct ObservedFile {
    file: File,
    _token: std::rc::Rc<()>,
}

#[cfg(test)]
impl ObservedFile {
    fn new(file: File, observer: Option<&LeaseObserver>, role: &'static str) -> Self {
        let token = std::rc::Rc::new(());
        if let Some(observer) = observer {
            observer
                .leases
                .borrow_mut()
                .push((role, std::rc::Rc::downgrade(&token)));
        }
        Self {
            file,
            _token: token,
        }
    }
}

#[cfg(test)]
impl std::ops::Deref for ObservedFile {
    type Target = File;

    fn deref(&self) -> &File {
        &self.file
    }
}
