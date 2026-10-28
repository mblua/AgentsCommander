#[cfg(windows)]
use super::delete_guard::path_key_for_delete_without_canonicalize;
use super::delete_guard::{metadata_is_link_or_reparse, metadata_parts_are_link_or_reparse};
use super::matrix_layout::{default_agent_matrix_config, selected_ac_root, validate_existing_name};
use super::team_config::resolve_agent_ref;
use super::AGENT_MATRIX_DIRS;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

// Strict Matrix copy and team publication share ephemeral output ownership.
pub(super) const COPY_PROVENANCE_FILE: &str = ".ac-copy-provenance.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MatrixCopyKind {
    Directory,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MatrixCopyStamp {
    pub(super) identity: Option<MatrixCopyIdentity>,
    pub(super) kind: MatrixCopyKind,
    pub(super) size: u64,
    pub(super) modified: std::time::SystemTime,
    pub(super) digest: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MatrixCopyIdentity(pub(super) u64, pub(super) u64);

#[derive(Debug)]
pub(super) struct OwnedCopyEntry {
    pub(super) relative: PathBuf,
    pub(super) identity: MatrixCopyIdentity,
    pub(super) kind: MatrixCopyKind,
    pub(super) digest: Option<[u8; 32]>,
}

#[derive(Debug)]
pub(super) struct OwnedCopyArtifacts {
    pub(super) root: PathBuf,
    pub(super) entries: Vec<OwnedCopyEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MatrixCopyStage {
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
pub(super) type MatrixCopyCallback<'a> =
    dyn FnMut(MatrixCopyStage, &Path) -> Result<(), String> + 'a;

#[derive(Default)]
pub(super) struct MatrixCopyHooks<'a> {
    #[cfg(test)]
    pub(super) callback: Option<&'a mut MatrixCopyCallback<'a>>,
    #[cfg(test)]
    pub(super) classification: Option<(bool, u32, bool, bool)>,
    pub(super) marker: std::marker::PhantomData<&'a ()>,
}

impl MatrixCopyHooks<'_> {
    pub(super) fn run(&mut self, stage: MatrixCopyStage, path: &Path) -> Result<(), String> {
        #[cfg(test)]
        if let Some(callback) = &mut self.callback {
            return callback(stage, path);
        }
        let _ = (stage, path);
        Ok(())
    }
}

impl OwnedCopyArtifacts {
    pub(super) fn handle_identity(file: &File) -> Result<MatrixCopyIdentity, String> {
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

    pub(super) fn kind(metadata: &std::fs::Metadata) -> Result<MatrixCopyKind, String> {
        if metadata_is_link_or_reparse(metadata) {
            return Err("Matrix copy rejects links/reparse points".into());
        }
        Self::classify(false, 0, metadata.is_dir(), metadata.is_file())
    }

    pub(super) fn classify(
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

    pub(super) fn source_kind(
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

    pub(super) fn open_existing(path: &Path, kind: MatrixCopyKind) -> Result<File, String> {
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

    pub(super) fn verify_directories(&self, relative: &Path) -> Result<(), String> {
        for entry in &self.entries {
            if entry.kind == MatrixCopyKind::Directory && relative.starts_with(&entry.relative) {
                let path = self.root.join(&entry.relative);
                let file = Self::open_existing(&path, MatrixCopyKind::Directory)?;
                if Self::handle_identity(&file)? != entry.identity {
                    return Err(format!("Copy directory replaced: {}", path.display()));
                }
            }
        }
        Ok(())
    }

    pub(super) fn create_directory(&mut self, relative: &Path) -> Result<(), String> {
        self.verify_directories(relative)?;
        let path = self.root.join(relative);
        std::fs::create_dir(&path).map_err(|e| e.to_string())?;
        // If capture fails, the unrecorded directory is deliberately retained.
        let file = Self::open_existing(&path, MatrixCopyKind::Directory)
            .map_err(|e| format!("{e}; residue: {}", path.display()))?;
        let identity = Self::handle_identity(&file)
            .map_err(|e| format!("{e}; residue: {}", path.display()))?;
        self.entries.push(OwnedCopyEntry {
            relative: relative.to_path_buf(),
            identity,
            kind: MatrixCopyKind::Directory,
            digest: None,
        });
        Ok(())
    }

    pub(super) fn create_file(&mut self, relative: &Path) -> Result<File, String> {
        self.verify_directories(relative)?;
        let path = self.root.join(relative);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let identity = Self::handle_identity(&file)
            .map_err(|e| format!("{e}; residue: {}", path.display()))?;
        self.entries.push(OwnedCopyEntry {
            relative: relative.to_path_buf(),
            identity,
            kind: MatrixCopyKind::File,
            digest: Some(Self::digest_bytes(&[])),
        });
        Ok(file)
    }

    pub(super) fn digest_bytes(bytes: &[u8]) -> [u8; 32] {
        use sha2::Digest;
        sha2::Sha256::digest(bytes).into()
    }

    pub(super) fn stamp(
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

    pub(super) fn read_file(
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
        if Self::stamp(&before, None)?
            != Self::stamp(&input.metadata().map_err(|e| e.to_string())?, None)?
        {
            return Err("Matrix file changed before reading".into());
        }
        let identity = Self::handle_identity(&input)?;
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
        let before_stamp = Self::stamp(&before, None)?;
        if before_stamp != Self::stamp(&after, None)?
            || before_stamp != Self::stamp(&input.metadata().map_err(|e| e.to_string())?, None)?
        {
            return Err(format!("Matrix file changed: {}", path.display()));
        }
        let current = Self::open_existing(path, MatrixCopyKind::File)?;
        if Self::handle_identity(&current)? != identity {
            return Err("Matrix file replaced during reading".into());
        }
        let mut stamp = Self::stamp(&after, Some(digest.finalize().into()))?;
        stamp.identity = Some(identity);
        Ok(stamp)
    }

    pub(super) fn selected_root_entry(
        path: &Path,
        name: &str,
        hooks: &MatrixCopyHooks<'_>,
    ) -> Result<bool, String> {
        let required = match name {
            "Role.md" => Some(MatrixCopyKind::File),
            "skills" => Some(MatrixCopyKind::Directory),
            _ if name.contains("default") => None,
            _ => return Ok(false),
        };
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        let kind = Self::source_kind(&metadata, hooks)?;
        if let Some(expected) = required {
            if kind != expected {
                return Err(format!("Unexpected generic asset type: {name}"));
            }
            return Ok(true);
        }
        Ok(kind == MatrixCopyKind::Directory)
    }

    pub(super) fn scan(
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
            let identity = Self::handle_identity(&handle)?;
            let mut stamp = Self::stamp(&metadata, None)?;
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
                    role |= name == "Role.md";
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
            if root && !role {
                return Err("Missing required Role.md".into());
            }
            let after = Self::open_existing(&path, kind)?;
            if Self::handle_identity(&after)? != identity {
                return Err("Matrix directory replaced".into());
            }
            let mut after_stamp = Self::stamp(&after.metadata().map_err(|e| e.to_string())?, None)?;
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

pub(super) fn validate_copy_provenance(
    history: Option<&[u8]>,
) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = match history {
        Some(bytes) => {
            serde_json::from_slice(bytes).map_err(|e| format!("Invalid copy provenance: {e}"))?
        }
        None => serde_json::json!({"schemaVersion": 1, "copies": []}),
    };
    if value.get("schemaVersion").and_then(|v| v.as_u64()) != Some(1) {
        return Err("Unsupported copy provenance schemaVersion".into());
    }
    let copies = value
        .get("copies")
        .and_then(|v| v.as_array())
        .ok_or("Invalid copy provenance copies")?;
    for event in copies {
        for key in [
            "sourceProjectName",
            "sourceProjectPath",
            "sourceAgentRef",
            "copiedAt",
        ] {
            if event
                .get(key)
                .and_then(|v| v.as_str())
                .is_none_or(|v| v.trim().is_empty())
            {
                return Err(format!("Invalid copy provenance {key}"));
            }
        }
        let reference = event["sourceAgentRef"].as_str().expect("validated string");
        let bare = reference
            .strip_prefix("_agent_")
            .ok_or("Invalid provenance agent ref")?;
        validate_existing_name(bare, "Agent")?;
        if reference.contains(['/', '\\', ':', '\0']) {
            return Err("Invalid provenance agent ref".into());
        }
        let time = chrono::DateTime::parse_from_rfc3339(
            event["copiedAt"].as_str().expect("validated string"),
        )
        .map_err(|e| format!("Invalid copy provenance time: {e}"))?;
        if time.offset().local_minus_utc() != 0 {
            return Err("Copy provenance time must be UTC".into());
        }
    }
    Ok(value)
}

pub(super) fn append_copy_provenance(
    mut value: serde_json::Value,
    source_project: &Path,
    source_ref: &str,
) -> Result<serde_json::Value, String> {
    let name = source_project
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("Invalid source project name")?;
    value["copies"]
        .as_array_mut()
        .ok_or("Invalid copy provenance copies")?
        .push(serde_json::json!({
            "sourceProjectName": name,
            "sourceProjectPath":
                crate::path_utils::path_to_string_without_windows_verbatim_prefix(source_project),
            "sourceAgentRef": source_ref,
            "copiedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        }));
    Ok(value)
}

pub(super) fn cleanup_owned_copy_artifacts(owned: &OwnedCopyArtifacts) -> Result<(), String> {
    let mut residues = Vec::new();
    for entry in owned.entries.iter().rev() {
        let path = owned.root.join(&entry.relative);
        let remove = (|| -> Result<(), String> {
            owned.verify_directories(&entry.relative)?;
            let file = OwnedCopyArtifacts::open_existing(&path, entry.kind)?;
            if OwnedCopyArtifacts::handle_identity(&file)? != entry.identity {
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
                if OwnedCopyArtifacts::handle_identity(&current)? != entry.identity {
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

#[cfg(test)]
pub(super) fn copy_agent_matrix_strict(
    source_project: &Path,
    source_ref: &str,
    destination_ac: &Path,
    destination_ref: &str,
) -> Result<OwnedCopyArtifacts, String> {
    copy_agent_matrix_strict_with_hooks(
        source_project,
        source_ref,
        destination_ac,
        destination_ref,
        &mut MatrixCopyHooks::default(),
    )
}

pub(super) fn copy_agent_matrix_strict_with_hooks(
    source_project: &Path,
    source_ref: &str,
    destination_ac: &Path,
    destination_ref: &str,
    hooks: &mut MatrixCopyHooks<'_>,
) -> Result<OwnedCopyArtifacts, String> {
    use std::io::Write;
    let project = crate::config::projects::canonical_source_project(source_project)
        .map_err(|e| e.to_string())?;
    let ac = selected_ac_root(&project)?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let source_ref = resolve_agent_ref(&ac, source_ref)?;
    let source = ac.join(&source_ref);
    if OwnedCopyArtifacts::kind(&std::fs::symlink_metadata(&source).map_err(|e| e.to_string())?)?
        != MatrixCopyKind::Directory
    {
        return Err("Source Matrix must be a directory".into());
    }
    let source_handle = OwnedCopyArtifacts::open_existing(&source, MatrixCopyKind::Directory)?;
    let source_identity = OwnedCopyArtifacts::handle_identity(&source_handle)?;
    let canonical_source = source.canonicalize().map_err(|e| e.to_string())?;
    let parent = canonical_source
        .parent()
        .ok_or("Source Matrix has no parent")?;
    #[cfg(windows)]
    let same_parent = path_key_for_delete_without_canonicalize(parent)
        == path_key_for_delete_without_canonicalize(&ac);
    #[cfg(not(windows))]
    let same_parent = parent == ac;
    if !same_parent {
        return Err("Source Matrix is not a direct project child".into());
    }
    let source_ref = canonical_source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Source Matrix has no portable name")?;
    let source_name = source_ref
        .strip_prefix("_agent_")
        .ok_or("Invalid source Matrix name")?;
    validate_existing_name(source_name, "Agent")?;
    let bare = destination_ref
        .strip_prefix("_agent_")
        .ok_or("Invalid destination agent ref")?;
    validate_existing_name(bare, "Agent")?;
    if destination_ref.contains(['/', '\\', ':', '\0']) {
        return Err("Invalid destination agent ref".into());
    }
    let destination_ac = destination_ac.canonicalize().map_err(|e| e.to_string())?;
    let root = destination_ac.join(destination_ref);
    if root.starts_with(&canonical_source) {
        return Err("Copy destination lies inside source Matrix".into());
    }
    let mut owned = OwnedCopyArtifacts {
        root,
        entries: Vec::new(),
    };
    owned.create_directory(Path::new(""))?;
    let result = (|| -> Result<(), String> {
        hooks.run(MatrixCopyStage::Reserved, &owned.root)?;
        let mut first = std::collections::BTreeMap::new();
        OwnedCopyArtifacts::scan(&source, Path::new(""), Some(&mut owned), &mut first, hooks)?;
        let provenance = validate_copy_provenance(None)?;
        hooks.run(MatrixCopyStage::BeforeVerify, &source)?;
        let mut second = std::collections::BTreeMap::new();
        OwnedCopyArtifacts::scan(&source, Path::new(""), None, &mut second, hooks)?;
        let current = OwnedCopyArtifacts::open_existing(&source, MatrixCopyKind::Directory)?;
        if OwnedCopyArtifacts::handle_identity(&current)? != source_identity {
            return Err("Source Matrix root replaced".into());
        }
        if first != second {
            return Err("Source Matrix changed during copy".into());
        }
        for &directory in AGENT_MATRIX_DIRS {
            if directory != "skills" || !first.contains_key(Path::new("skills")) {
                owned.create_directory(Path::new(directory))?;
            }
        }
        let config =
            serde_json::to_vec_pretty(&default_agent_matrix_config()).map_err(|e| e.to_string())?;
        write_owned_team_file(&mut owned, Path::new("config.json"), &config)?;
        hooks.run(MatrixCopyStage::ConfigWritten, &owned.root)?;
        let provenance = append_copy_provenance(provenance, &project, source_ref)?;
        let bytes = serde_json::to_vec_pretty(&provenance).map_err(|e| e.to_string())?;
        hooks.run(MatrixCopyStage::Provenance, &owned.root)?;
        let mut output = owned.create_file(Path::new(COPY_PROVENANCE_FILE))?;
        let entry = owned.entries.last_mut().expect("created provenance entry");
        let mut written = 0;
        while written < bytes.len() {
            let count = output.write(&bytes[written..]).map_err(|e| e.to_string())?;
            if count == 0 {
                return Err("Provenance write made no progress".into());
            }
            written += count;
            entry.digest = Some(OwnedCopyArtifacts::digest_bytes(&bytes[..written]));
            hooks.run(MatrixCopyStage::ProvenanceWritten, &owned.root)?;
        }
        output.flush().map_err(|e| e.to_string())?;
        output.sync_all().map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(primary) = result {
        let hook_error = hooks.run(MatrixCopyStage::BeforeCleanup, &owned.root).err();
        let cleanup = cleanup_owned_copy_artifacts(&owned).err();
        return Err(format!(
            "Matrix copy failed: {primary}{}{}",
            hook_error
                .map(|e| format!("; cleanup hook: {e}"))
                .unwrap_or_default(),
            cleanup.map(|e| format!("; {e}")).unwrap_or_default()
        ));
    }
    Ok(owned)
}

pub(super) fn write_owned_team_file(
    owned: &mut OwnedCopyArtifacts,
    relative: &Path,
    bytes: &[u8],
) -> Result<(), String> {
    use std::io::Write;
    let mut file = owned.create_file(relative)?;
    let entry = owned.entries.last_mut().expect("created team file");
    let mut written = 0;
    while written < bytes.len() {
        let count = file.write(&bytes[written..]).map_err(|e| e.to_string())?;
        if count == 0 {
            return Err("Team file write made no progress".into());
        }
        written += count;
        entry.digest = Some(OwnedCopyArtifacts::digest_bytes(&bytes[..written]));
    }
    file.flush().map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}
