use super::{SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME, STATE_SCHEMA_VERSION, STATE_TEMP_COUNTER};
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SeededContextTemplateState {
    pub(crate) schema_version: u32,
    pub(crate) templates: BTreeMap<String, SeededContextTemplateEntry>,
}

impl Default for SeededContextTemplateState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            templates: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SeededContextTemplateEntry {
    pub(crate) template_id: String,
    /// #1748: `None` means a distribution-owned template, which records no version.
    pub(crate) current_version: Option<u32>,
    pub(crate) last_seeded_sha256: Option<String>,
    pub(crate) last_observed_sha256: Option<String>,
    pub(crate) ignored_default_sha256: Option<String>,
    pub(crate) ignored_observed_sha256: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct SeededContextTemplateSpec {
    pub(crate) id: &'static str,
    pub(crate) filename: &'static str,
    pub(crate) label: &'static str,
    pub(crate) current_version: u32,
    pub(crate) current_content: fn() -> &'static str,
    /// #1748: `None` for a distribution-owned spec, which consults no recognizer.
    pub(crate) is_known_generated: Option<fn(&str) -> bool>,
    pub(crate) project_actionable: bool,
    pub(crate) suppress_unknown_without_state: bool,
    /// #1748: the file is owned by the distribution. It is repaired on the
    /// reporting sync path instead of being offered as a pending update.
    pub(crate) distribution_owned: bool,
}

#[derive(Clone)]
pub(crate) struct FileSnapshot {
    pub(crate) bytes: Vec<u8>,
    pub(crate) content: String,
    pub(crate) sha256: String,
}

pub(crate) struct LoadedState {
    pub(crate) state: SeededContextTemplateState,
    pub(crate) trusted: bool,
    pub(crate) can_persist: bool,
    pub(crate) dirty: bool,
}

impl LoadedState {
    pub(crate) fn trusted_entry(
        &self,
        spec: SeededContextTemplateSpec,
    ) -> Option<&SeededContextTemplateEntry> {
        if !self.trusted {
            return None;
        }
        self.state
            .templates
            .get(spec.id)
            .filter(|entry| entry.template_id == spec.id)
    }

    pub(crate) fn entry_mut(
        &mut self,
        spec: SeededContextTemplateSpec,
    ) -> &mut SeededContextTemplateEntry {
        let next_version = (!spec.distribution_owned).then_some(spec.current_version);
        // #1748: for a distribution-owned spec this is the ONLY field this call can change,
        // and every downstream guard compares hashes, so nothing else would ever mark the
        // state dirty and an already-in-sync installation would keep its stale version on
        // disk forever. Compare through a shared borrow first, so the flag is set on the
        // real delta and the &mut below is taken only once.
        if spec.distribution_owned {
            let already_recorded = self.state.templates.get(spec.id).is_some_and(|entry| {
                entry.template_id == spec.id && entry.current_version == next_version
            });
            if !already_recorded {
                self.dirty = true;
            }
        }
        let entry = self.state.templates.entry(spec.id.to_string()).or_default();
        entry.template_id = spec.id.to_string();
        entry.current_version = next_version;
        entry
    }

    pub(crate) fn mark_seeded(
        &mut self,
        spec: SeededContextTemplateSpec,
        current_default_sha256: &str,
    ) {
        if !self.trusted {
            return;
        }
        let entry = self.entry_mut(spec);
        let next = Some(current_default_sha256.to_string());
        if entry.last_seeded_sha256 != next
            || entry.last_observed_sha256.is_some()
            || entry.ignored_default_sha256.is_some()
            || entry.ignored_observed_sha256.is_some()
        {
            entry.last_seeded_sha256 = next;
            entry.last_observed_sha256 = None;
            entry.ignored_default_sha256 = None;
            entry.ignored_observed_sha256 = None;
            self.dirty = true;
        }
    }

    pub(crate) fn mark_observed(
        &mut self,
        spec: SeededContextTemplateSpec,
        current_file_sha256: &str,
    ) {
        if !self.trusted {
            return;
        }
        let entry = self.entry_mut(spec);
        let next = Some(current_file_sha256.to_string());
        if entry.last_observed_sha256 != next {
            entry.last_observed_sha256 = next;
            self.dirty = true;
        }
    }

    pub(crate) fn mark_ignored(
        &mut self,
        spec: SeededContextTemplateSpec,
        current_file_sha256: &str,
        current_default_sha256: &str,
    ) {
        let entry = self.entry_mut(spec);
        let observed = Some(current_file_sha256.to_string());
        let default = Some(current_default_sha256.to_string());
        if entry.ignored_observed_sha256 != observed || entry.ignored_default_sha256 != default {
            entry.ignored_observed_sha256 = observed;
            entry.ignored_default_sha256 = default;
            entry.last_observed_sha256 = Some(current_file_sha256.to_string());
            self.dirty = true;
        }
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{:x}", digest)
}

/// #1748: the operator-owned override that replaces this template at render time.
/// Kept in lockstep with `config::local_overlay`, whose PRIVATE `MARKDOWN_LOCAL_SUFFIX`
/// (`local_overlay.rs:28`) is the other half of the contract. The lockstep holds for a
/// `.md` filename, which is the only kind any spec carries; for any other extension
/// `markdown_override_path` returns `None` and this helper has no counterpart.
pub(crate) fn local_override_filename(filename: &str) -> String {
    format!(
        "{}.local.md",
        filename.strip_suffix(".md").unwrap_or(filename)
    )
}

pub(crate) fn display_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

pub(crate) fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        false
    }
}

pub(crate) fn validate_existing_dir(path: &Path, label: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to inspect {} {}: {}", label, path.display(), e))?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "{} {} exists but is not a regular directory",
            label,
            path.display()
        ));
    }
    Ok(())
}

pub(crate) fn validate_existing_file(path: &Path, label: &str) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to inspect {} {}: {}", label, path.display(), e))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "{} {} exists but is not a regular file",
            label,
            path.display()
        ));
    }
    Ok(())
}

pub(crate) fn read_validated_snapshot(
    path: &Path,
    label: &str,
) -> Result<Option<FileSnapshot>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                return Err(format!(
                    "{} {} exists but is not a regular file",
                    label,
                    path.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "Failed to inspect {} {}: {}",
                label,
                path.display(),
                e
            ))
        }
    }

    let bytes = std::fs::read(path)
        .map_err(|e| format!("Failed to read {} {}: {}", label, path.display(), e))?;
    let sha256 = sha256_hex(&bytes);
    let content = String::from_utf8(bytes.clone())
        .map_err(|e| format!("{} {} is not valid UTF-8: {}", label, path.display(), e))?;
    Ok(Some(FileSnapshot {
        bytes,
        content,
        sha256,
    }))
}

pub(crate) fn load_state(ac_root: &Path, strict: bool) -> Result<LoadedState, String> {
    let path = ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                let message = format!(
                    "Context template state path {} exists but is not a regular file",
                    path.display()
                );
                if strict {
                    return Err(message);
                }
                log::warn!(
                    "[context-templates] {}; skipping state persistence",
                    message
                );
                return Ok(LoadedState {
                    state: SeededContextTemplateState::default(),
                    trusted: false,
                    can_persist: false,
                    dirty: false,
                });
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedState {
                state: SeededContextTemplateState::default(),
                trusted: true,
                can_persist: true,
                dirty: false,
            })
        }
        Err(e) => {
            let message = format!(
                "Failed to inspect context template state {}: {}",
                path.display(),
                e
            );
            if strict {
                return Err(message);
            }
            log::warn!(
                "[context-templates] {}; skipping state persistence",
                message
            );
            return Ok(LoadedState {
                state: SeededContextTemplateState::default(),
                trusted: false,
                can_persist: false,
                dirty: false,
            });
        }
    }

    let bytes = std::fs::read(&path).map_err(|e| {
        format!(
            "Failed to read context template state {}: {}",
            path.display(),
            e
        )
    })?;
    let state = match serde_json::from_slice::<SeededContextTemplateState>(&bytes) {
        Ok(state) => state,
        Err(e) => {
            log::warn!(
                "[context-templates] invalid state JSON at {}; treating as empty: {}",
                path.display(),
                e
            );
            return Ok(LoadedState {
                state: SeededContextTemplateState::default(),
                trusted: true,
                can_persist: true,
                dirty: true,
            });
        }
    };

    if state.schema_version != STATE_SCHEMA_VERSION {
        let message = format!(
            "Context template state schema version {} is unsupported; reload or upgrade \
                AgentsCommander.",
            state.schema_version
        );
        if strict {
            return Err(message);
        }
        log::warn!(
            "[context-templates] {}; skipping state persistence",
            message
        );
        return Ok(LoadedState {
            state: SeededContextTemplateState::default(),
            trusted: false,
            can_persist: false,
            dirty: false,
        });
    }

    Ok(LoadedState {
        state,
        trusted: true,
        can_persist: true,
        dirty: false,
    })
}

pub(crate) fn unique_state_temp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
    let counter = STATE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        counter
    ))
}

pub(crate) fn cleanup_temp(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!(
                "[context-templates] failed to remove temporary file {}: {}",
                path.display(),
                e
            );
        }
    }
}

pub(crate) fn persist_state(
    ac_root: &Path,
    state: &SeededContextTemplateState,
) -> Result<(), String> {
    let path = ac_root.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                return Err(format!(
                    "Context template state path {} exists but is not a regular file",
                    path.display()
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to inspect context template state {}: {}",
                path.display(),
                e
            ))
        }
    }

    let content = serde_json::to_vec_pretty(state)
        .map_err(|e| format!("Failed to serialize context template state: {}", e))?;
    let temp = unique_state_temp_path(&path);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| {
            format!(
                "Failed to create temporary context template state {}: {}",
                temp.display(),
                e
            )
        })?;
    if let Err(e) = file.write_all(&content) {
        drop(file);
        cleanup_temp(&temp);
        return Err(format!(
            "Failed to write temporary context template state {}: {}",
            temp.display(),
            e
        ));
    }
    if let Err(e) = file.flush() {
        drop(file);
        cleanup_temp(&temp);
        return Err(format!(
            "Failed to flush temporary context template state {}: {}",
            temp.display(),
            e
        ));
    }
    if let Err(e) = file.sync_all() {
        drop(file);
        cleanup_temp(&temp);
        return Err(format!(
            "Failed to sync temporary context template state {}: {}",
            temp.display(),
            e
        ));
    }
    drop(file);

    if let Err(e) = crate::config::root_agent::atomic_replace_existing(&temp, &path) {
        cleanup_temp(&temp);
        return Err(e);
    }
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            if let Err(e) = dir.sync_all() {
                log::warn!(
                    "[context-templates] failed to sync state directory {}: {}",
                    parent.display(),
                    e
                );
            }
        }
    }
    Ok(())
}

pub(crate) fn persist_state_best_effort(ac_root: &Path, loaded: &LoadedState) {
    if !loaded.dirty || !loaded.can_persist {
        return;
    }
    if let Err(e) = persist_state(ac_root, &loaded.state) {
        log::warn!(
            "[context-templates] failed to persist state in {}: {}",
            ac_root.display(),
            e
        );
    }
}

pub(crate) fn persist_state_strict(ac_root: &Path, loaded: &LoadedState) -> Result<(), String> {
    if !loaded.can_persist {
        return Err("Context template state cannot be safely persisted".to_string());
    }
    if loaded.dirty {
        persist_state(ac_root, &loaded.state)?;
    }
    Ok(())
}
