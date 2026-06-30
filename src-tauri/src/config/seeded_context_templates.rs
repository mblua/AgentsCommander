use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME: &str = ".agentscommander-context-templates.json";

const STATE_SCHEMA_VERSION: u32 = 1;
static STATE_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const CONTEXT_TEMPLATE_CHANGED: &str =
    "Context template changed on disk; reload the project before overwriting.";
const CONTEXT_TEMPLATE_DEFAULT_CHANGED: &str =
    "Context template default changed; reload the project before overwriting.";

const OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND: &str = "You are the coordinator for your team. You must:\n\
     - Keep your base role; coordination is an additional assignment, not a replacement.\n\
     - Receive team work requests.\n\
     - Clarify scope, outcome, constraints, and acceptance criteria.\n\
     - Always route work to the team member best prepared for each part of the request based on role, skills, and current assignment.\n\
     - Delegate work instead of absorbing technical work when a more specialized agent is available.\n\
     - Sequence work, track progress, surface blockers, and keep ownership clear.\n\
     - Follow up after assignment to verify the assigned agent is active and working.\n\
     - Contact silent or inactive assigned agents up to three total attempts.\n\
     - Require assigned agents to explicitly report completion, outcome, blockers, and verification before treating delegated work as complete.\n\
     - Not infer completion solely from files/logs/artifacts/status flags when the assigned agent has not reported the outcome.\n\
     - Give recommendations to help an agent work better without removing or overriding that agent's role/scope.\n\n\
     ## Sending Screenshots\n\
     As a coordinator, you may need to send screenshots. Use the CLI subcommand:\n\
         telegram-send-image --path <PATH> [--caption <CAPTION>] [--bot-id <ID> | --bot-label <LABEL>]\n\
     - --path is required. --caption is optional and limited to 1024 UTF-16 units.\n\
     - If multiple Telegram bots are configured, use --bot-id or --bot-label.\n\
     - jpg/jpeg/png/webp up to 10 MB use sendPhoto; other formats including GIF use sendDocument up to 50 MB.\n\
     - Symlinks/junctions are rejected.\n\n\
     **Screenshot Capture Paths:**\n\
     - Interactive desktop coordinator: PowerShell System.Drawing / CopyFromScreen can work. Important: cast Measure-Object results to [int] before passing dimensions to Bitmap.\n\
     - Sandboxed harness coordinator: CopyFromScreen may return all-zero/black pixels. In that case ask the user to capture with Greenshot, use latest file from C:\\Users\\maria\\0_greenshot\\, and visually inspect the image content before sending.\n\
     - Do not judge Greenshot screenshot relevance by filename; names can be misleading.\n";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTemplateUpdate {
    pub project_path: String,
    pub workspace_path: String,
    pub file_path: String,
    pub filename: String,
    pub label: String,
    pub current_file_sha256: String,
    pub current_default_sha256: String,
    pub current_default_version: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextTemplateOverwriteResult {
    pub file_path: String,
    pub backup_path: String,
    pub current_default_sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeededContextTemplateState {
    schema_version: u32,
    templates: BTreeMap<String, SeededContextTemplateEntry>,
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
struct SeededContextTemplateEntry {
    template_id: String,
    current_version: u32,
    last_seeded_sha256: Option<String>,
    last_observed_sha256: Option<String>,
    ignored_default_sha256: Option<String>,
    ignored_observed_sha256: Option<String>,
}

#[derive(Clone, Copy)]
struct SeededContextTemplateSpec {
    id: &'static str,
    filename: &'static str,
    label: &'static str,
    current_version: u32,
    current_content: fn() -> &'static str,
    is_known_generated: fn(&str) -> bool,
    project_actionable: bool,
    suppress_unknown_without_state: bool,
}

#[derive(Clone)]
struct FileSnapshot {
    bytes: Vec<u8>,
    content: String,
    sha256: String,
}

struct LoadedState {
    state: SeededContextTemplateState,
    trusted: bool,
    can_persist: bool,
    dirty: bool,
}

impl LoadedState {
    fn trusted_entry(
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

    fn entry_mut(&mut self, spec: SeededContextTemplateSpec) -> &mut SeededContextTemplateEntry {
        let entry = self.state.templates.entry(spec.id.to_string()).or_default();
        entry.template_id = spec.id.to_string();
        entry.current_version = spec.current_version;
        entry
    }

    fn mark_seeded(&mut self, spec: SeededContextTemplateSpec, current_default_sha256: &str) {
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

    fn mark_observed(&mut self, spec: SeededContextTemplateSpec, current_file_sha256: &str) {
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

    fn mark_ignored(
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

fn project_specs() -> [SeededContextTemplateSpec; 2] {
    [
        SeededContextTemplateSpec {
            id: "global",
            filename: crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME,
            label: "AgentsCommander shared context",
            current_version: 1,
            current_content: crate::config::session_context::get_default_agent_template,
            is_known_generated: is_known_generated_global_template,
            project_actionable: true,
            suppress_unknown_without_state: true,
        },
        SeededContextTemplateSpec {
            id: "coordinator",
            filename: crate::config::session_context::COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            label: "Coordinator context",
            current_version: 2,
            current_content: crate::config::session_context::get_default_coordinator_template,
            is_known_generated: is_known_generated_coordinator_template,
            project_actionable: true,
            suppress_unknown_without_state: false,
        },
    ]
}

fn root_spec() -> SeededContextTemplateSpec {
    SeededContextTemplateSpec {
        id: "rootAgent",
        filename: crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME,
        label: "Root agent context",
        current_version: 4,
        current_content: crate::config::root_agent::default_root_context_template,
        is_known_generated: crate::config::root_agent::is_known_generated_root_context_template,
        project_actionable: false,
        suppress_unknown_without_state: false,
    }
}

fn project_spec_by_filename(filename: &str) -> Option<SeededContextTemplateSpec> {
    project_specs()
        .into_iter()
        .find(|spec| spec.filename == filename)
}

fn actionable_project_spec_by_filename(
    filename: &str,
) -> Result<SeededContextTemplateSpec, String> {
    if filename.contains('/') || filename.contains('\\') {
        return Err("Context template filename is not managed by AgentsCommander".to_string());
    }
    let spec = project_spec_by_filename(filename)
        .ok_or_else(|| "Context template filename is not managed by AgentsCommander".to_string())?;
    if !spec.project_actionable {
        return Err("Context template filename is not actionable for this project".to_string());
    }
    Ok(spec)
}

fn is_known_generated_global_template(content: &str) -> bool {
    content == crate::config::session_context::get_default_agent_template()
}

fn is_known_generated_coordinator_template(content: &str) -> bool {
    content == crate::config::session_context::get_default_coordinator_template()
        || content == OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{:x}", digest)
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
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

fn validate_existing_dir(path: &Path, label: &str) -> Result<(), String> {
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

fn validate_existing_file(path: &Path, label: &str) -> Result<(), String> {
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

fn read_validated_snapshot(path: &Path, label: &str) -> Result<Option<FileSnapshot>, String> {
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

fn load_state(workspace_dir: &Path, strict: bool) -> Result<LoadedState, String> {
    let path = workspace_dir.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
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
            "Context template state schema version {} is unsupported; reload or upgrade AgentsCommander.",
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

fn unique_state_temp_path(path: &Path) -> PathBuf {
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

fn cleanup_temp(path: &Path) {
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

fn persist_state(workspace_dir: &Path, state: &SeededContextTemplateState) -> Result<(), String> {
    let path = workspace_dir.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
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

fn persist_state_best_effort(workspace_dir: &Path, loaded: &LoadedState) {
    if !loaded.dirty || !loaded.can_persist {
        return;
    }
    if let Err(e) = persist_state(workspace_dir, &loaded.state) {
        log::warn!(
            "[context-templates] failed to persist state in {}: {}",
            workspace_dir.display(),
            e
        );
    }
}

fn persist_state_strict(workspace_dir: &Path, loaded: &LoadedState) -> Result<(), String> {
    if !loaded.can_persist {
        return Err("Context template state cannot be safely persisted".to_string());
    }
    if loaded.dirty {
        persist_state(workspace_dir, &loaded.state)?;
    }
    Ok(())
}

fn make_update(
    project_dir: &Path,
    workspace_dir: &Path,
    path: &Path,
    spec: SeededContextTemplateSpec,
    current_file_sha256: String,
    current_default_sha256: String,
) -> ContextTemplateUpdate {
    ContextTemplateUpdate {
        project_path: display_path(project_dir),
        workspace_path: display_path(workspace_dir),
        file_path: display_path(path),
        filename: spec.filename.to_string(),
        label: spec.label.to_string(),
        current_file_sha256,
        current_default_sha256,
        current_default_version: spec.current_version,
    }
}

fn create_missing_template(path: &Path, content: &str) -> Result<(), String> {
    crate::config::session_context::write_template_if_missing(path, content)?;
    validate_existing_file(path, "Context template")
}

fn auto_update_generated_template(
    path: &Path,
    spec: SeededContextTemplateSpec,
    expected_file_sha256: &str,
) -> Result<bool, String> {
    let Some(snapshot) = read_validated_snapshot(path, "Context template")? else {
        return Ok(false);
    };
    if snapshot.sha256 != expected_file_sha256 {
        log::warn!(
            "[context-templates] {} changed before generated update; preserving current content",
            path.display()
        );
        return Ok(false);
    }
    if !(spec.is_known_generated)(&snapshot.content) {
        log::warn!(
            "[context-templates] {} no longer matches a known generated default; preserving current content",
            path.display()
        );
        return Ok(false);
    }
    crate::config::session_context::atomically_replace_context_template(
        path,
        (spec.current_content)(),
    )?;
    Ok(true)
}

fn sync_one_template(
    project_dir: Option<&Path>,
    workspace_dir: &Path,
    spec: SeededContextTemplateSpec,
    loaded: &mut LoadedState,
    allow_create_missing: bool,
    return_pending: bool,
) -> Result<Option<ContextTemplateUpdate>, String> {
    let path = workspace_dir.join(spec.filename);
    let current_default = (spec.current_content)();
    let current_default_sha256 = sha256_hex(current_default.as_bytes());
    let mut snapshot = read_validated_snapshot(&path, "Context template")?;

    if snapshot.is_none() {
        if !allow_create_missing {
            return Ok(None);
        }
        create_missing_template(&path, current_default)?;
        snapshot = read_validated_snapshot(&path, "Context template")?;
        if let Some(snapshot) = &snapshot {
            if snapshot.sha256 == current_default_sha256 {
                loaded.mark_seeded(spec, &current_default_sha256);
                return Ok(None);
            }
        }
    }

    let Some(snapshot) = snapshot else {
        return Ok(None);
    };

    if snapshot.sha256 == current_default_sha256 {
        loaded.mark_seeded(spec, &current_default_sha256);
        return Ok(None);
    }

    let trusted_entry = loaded.trusted_entry(spec).cloned();
    if let Some(entry) = trusted_entry.as_ref() {
        if entry.last_seeded_sha256.as_deref() == Some(snapshot.sha256.as_str())
            && entry.last_seeded_sha256.as_deref() != Some(current_default_sha256.as_str())
            && (spec.is_known_generated)(&snapshot.content)
        {
            if auto_update_generated_template(&path, spec, &snapshot.sha256)? {
                loaded.mark_seeded(spec, &current_default_sha256);
            }
            return Ok(None);
        }
    }

    let has_valid_entry = trusted_entry.is_some();
    if !has_valid_entry && (spec.is_known_generated)(&snapshot.content) {
        if auto_update_generated_template(&path, spec, &snapshot.sha256)? {
            loaded.mark_seeded(spec, &current_default_sha256);
        }
        return Ok(None);
    }

    if spec.suppress_unknown_without_state && !has_valid_entry {
        log::debug!(
            "[context-templates] preserving ambiguous global context template {} without prompting",
            path.display()
        );
        return Ok(None);
    }

    if let Some(entry) = trusted_entry.as_ref() {
        if entry.ignored_observed_sha256.as_deref() == Some(snapshot.sha256.as_str())
            && entry.ignored_default_sha256.as_deref() == Some(current_default_sha256.as_str())
        {
            return Ok(None);
        }
    }

    loaded.mark_observed(spec, &snapshot.sha256);
    if return_pending {
        let project_dir = project_dir.ok_or_else(|| {
            format!(
                "Cannot create context template update for {} without a project path",
                path.display()
            )
        })?;
        Ok(Some(make_update(
            project_dir,
            workspace_dir,
            &path,
            spec,
            snapshot.sha256,
            current_default_sha256,
        )))
    } else {
        log::warn!(
            "[context-templates] preserving customized context template {}; a newer default is available",
            path.display()
        );
        Ok(None)
    }
}

fn compute_pending_update(
    project_dir: &Path,
    workspace_dir: &Path,
    spec: SeededContextTemplateSpec,
    loaded: &LoadedState,
) -> Result<Option<ContextTemplateUpdate>, String> {
    let path = workspace_dir.join(spec.filename);
    let current_default = (spec.current_content)();
    let current_default_sha256 = sha256_hex(current_default.as_bytes());
    let Some(snapshot) = read_validated_snapshot(&path, "Context template")? else {
        return Ok(None);
    };
    if snapshot.sha256 == current_default_sha256 {
        return Ok(None);
    }

    let trusted_entry = loaded.trusted_entry(spec);
    let has_valid_entry = trusted_entry.is_some();
    if !has_valid_entry && (spec.is_known_generated)(&snapshot.content) {
        return Ok(None);
    }
    if spec.suppress_unknown_without_state && !has_valid_entry {
        return Ok(None);
    }
    if let Some(entry) = trusted_entry {
        if entry.ignored_observed_sha256.as_deref() == Some(snapshot.sha256.as_str())
            && entry.ignored_default_sha256.as_deref() == Some(current_default_sha256.as_str())
        {
            return Ok(None);
        }
    }

    Ok(Some(make_update(
        project_dir,
        workspace_dir,
        &path,
        spec,
        snapshot.sha256,
        current_default_sha256,
    )))
}

fn validate_project_workspace_dir(workspace_dir: &Path) -> Result<PathBuf, String> {
    validate_existing_dir(workspace_dir, "Project AC Root")?;
    let name = workspace_dir.file_name().and_then(|name| name.to_str());
    if name != Some(crate::config::workspace::canonical_workspace_dir_label()) {
        return Err(format!(
            "{} is not a Project AC Root directory",
            workspace_dir.display()
        ));
    }
    workspace_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("Project AC Root {} has no parent", workspace_dir.display()))
}

fn validate_project_workspace_for_scan(
    project_dir: &Path,
    workspace_dir: &Path,
) -> Result<(), String> {
    validate_existing_dir(workspace_dir, "Project AC Root")?;
    let expected = crate::config::workspace::workspace_dir_for_project(project_dir);
    if workspace_dir != expected {
        return Err(format!(
            "Project AC Root {} is not the canonical child of {}",
            workspace_dir.display(),
            project_dir.display()
        ));
    }
    Ok(())
}

pub fn ensure_project_context_templates(workspace_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(workspace_dir).map_err(|e| {
        format!(
            "failed to create context templates directory {}: {}",
            workspace_dir.display(),
            e
        )
    })?;
    validate_existing_dir(workspace_dir, "Context template directory")?;
    let mut loaded = load_state(workspace_dir, false)?;
    for spec in project_specs() {
        sync_one_template(None, workspace_dir, spec, &mut loaded, true, false)?;
    }
    persist_state_best_effort(workspace_dir, &loaded);
    Ok(())
}

pub fn scan_project_context_template_updates(
    project_dir: &Path,
    workspace_dir: &Path,
) -> Result<Vec<ContextTemplateUpdate>, String> {
    validate_project_workspace_for_scan(project_dir, workspace_dir)?;
    let mut loaded = load_state(workspace_dir, false)?;
    let mut updates = Vec::new();
    for spec in project_specs() {
        if let Some(update) = sync_one_template(
            Some(project_dir),
            workspace_dir,
            spec,
            &mut loaded,
            false,
            true,
        )? {
            updates.push(update);
        }
    }
    persist_state_best_effort(workspace_dir, &loaded);
    dedupe_context_template_updates(&mut updates);
    Ok(updates)
}

pub fn sync_project_context_template_for_read(
    context_dir: &Path,
    filename: &str,
) -> Result<(), String> {
    let Some(spec) = project_spec_by_filename(filename) else {
        return Ok(());
    };
    validate_existing_dir(context_dir, "Context template directory")?;
    let mut loaded = load_state(context_dir, false)?;
    let result = sync_one_template(None, context_dir, spec, &mut loaded, true, false);
    persist_state_best_effort(context_dir, &loaded);
    result.map(|_| ())
}

pub fn ensure_root_context_template(config_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(config_dir).map_err(|e| {
        format!(
            "Failed to create root agent config directory {}: {}",
            config_dir.display(),
            e
        )
    })?;
    validate_existing_dir(config_dir, "Root agent config directory")?;
    let mut loaded = load_state(config_dir, false)?;
    sync_one_template(None, config_dir, root_spec(), &mut loaded, true, false)?;
    persist_state_best_effort(config_dir, &loaded);
    Ok(())
}

fn validate_expected_hashes(
    project_dir: &Path,
    workspace_dir: &Path,
    spec: SeededContextTemplateSpec,
    loaded: &LoadedState,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
) -> Result<(), String> {
    let current_default_sha256 = sha256_hex((spec.current_content)().as_bytes());
    if current_default_sha256 != expected_default_sha256 {
        return Err(CONTEXT_TEMPLATE_DEFAULT_CHANGED.to_string());
    }
    let Some(pending) = compute_pending_update(project_dir, workspace_dir, spec, loaded)? else {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    };
    if pending.current_file_sha256 != expected_file_sha256
        || pending.current_default_sha256 != expected_default_sha256
    {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    }
    Ok(())
}

pub fn dismiss_context_template_update(
    workspace_dir: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
) -> Result<(), String> {
    let spec = actionable_project_spec_by_filename(filename)?;
    let project_dir = validate_project_workspace_dir(workspace_dir)?;
    let mut loaded = load_state(workspace_dir, true)?;
    validate_expected_hashes(
        &project_dir,
        workspace_dir,
        spec,
        &loaded,
        expected_file_sha256,
        expected_default_sha256,
    )?;
    let path = workspace_dir.join(spec.filename);
    let Some(snapshot) = read_validated_snapshot(&path, "Context template")? else {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    };
    if snapshot.sha256 != expected_file_sha256 {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    }
    loaded.mark_ignored(spec, expected_file_sha256, expected_default_sha256);
    persist_state_strict(workspace_dir, &loaded)
}

pub fn overwrite_context_template_with_default(
    workspace_dir: &Path,
    filename: &str,
    expected_file_sha256: &str,
    expected_default_sha256: &str,
) -> Result<ContextTemplateOverwriteResult, String> {
    let spec = actionable_project_spec_by_filename(filename)?;
    let project_dir = validate_project_workspace_dir(workspace_dir)?;
    let mut loaded = load_state(workspace_dir, true)?;
    validate_expected_hashes(
        &project_dir,
        workspace_dir,
        spec,
        &loaded,
        expected_file_sha256,
        expected_default_sha256,
    )?;

    let path = workspace_dir.join(spec.filename);
    let Some(snapshot) = read_validated_snapshot(&path, "Context template")? else {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    };
    if snapshot.sha256 != expected_file_sha256 {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    }
    if sha256_hex((spec.current_content)().as_bytes()) != expected_default_sha256 {
        return Err(CONTEXT_TEMPLATE_DEFAULT_CHANGED.to_string());
    }

    let backup_path = create_backup(&path, &snapshot.bytes)?;
    let Some(snapshot_after_backup) = read_validated_snapshot(&path, "Context template")? else {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    };
    if snapshot_after_backup.sha256 != expected_file_sha256 {
        return Err(CONTEXT_TEMPLATE_CHANGED.to_string());
    }

    if let Err(e) = crate::config::session_context::atomically_replace_context_template(
        &path,
        (spec.current_content)(),
    ) {
        log::warn!(
            "[context-templates] replacement failed after backup {} was created: {}",
            backup_path.display(),
            e
        );
        return Err(e);
    }

    loaded.mark_seeded(spec, expected_default_sha256);
    persist_state_strict(workspace_dir, &loaded)?;
    Ok(ContextTemplateOverwriteResult {
        file_path: display_path(&path),
        backup_path: display_path(&backup_path),
        current_default_sha256: expected_default_sha256.to_string(),
    })
}

fn create_backup(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for context template {}",
            path.display()
        )
    })?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid context template filename {}", path.display()))?;
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%SZ").to_string();

    for index in 0..1000_u32 {
        let backup_name = match index {
            0 => format!("{filename}.bak"),
            1 => format!("{filename}.{timestamp}.bak"),
            n => format!("{filename}.{timestamp}.{n}.bak"),
        };
        let backup_path = parent.join(backup_name);
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup_path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to create backup context template {}: {}",
                    backup_path.display(),
                    e
                ))
            }
        };
        if let Err(e) = file.write_all(bytes) {
            return Err(format!(
                "Failed to write backup context template {}: {}",
                backup_path.display(),
                e
            ));
        }
        if let Err(e) = file.flush() {
            return Err(format!(
                "Failed to flush backup context template {}: {}",
                backup_path.display(),
                e
            ));
        }
        if let Err(e) = file.sync_all() {
            return Err(format!(
                "Failed to sync backup context template {}: {}",
                backup_path.display(),
                e
            ));
        }
        drop(file);
        if let Ok(dir) = std::fs::File::open(parent) {
            if let Err(e) = dir.sync_all() {
                log::warn!(
                    "[context-templates] failed to sync backup directory {}: {}",
                    parent.display(),
                    e
                );
            }
        }
        return Ok(backup_path);
    }

    Err(format!(
        "Failed to create a unique backup path for {}",
        path.display()
    ))
}

pub fn dedupe_context_template_updates(updates: &mut Vec<ContextTemplateUpdate>) {
    let mut seen = HashSet::new();
    updates.retain(|update| {
        seen.insert((
            update.workspace_path.clone(),
            update.filename.clone(),
            update.current_file_sha256.clone(),
            update.current_default_sha256.clone(),
        ))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::session_context::{
        get_default_coordinator_template, COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
        GLOBAL_CONTEXT_TEMPLATE_FILENAME,
    };

    fn hash_text(content: &str) -> String {
        sha256_hex(content.as_bytes())
    }

    #[test]
    fn old_coordinator_default_is_known_generated_without_raise_hand() {
        assert!(
            !OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.contains("## Raising Your Hand")
        );
        assert!(
            OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.contains("## Sending Screenshots")
        );
        assert!(
            OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.contains("names can be misleading.")
        );
        assert!(is_known_generated_coordinator_template(
            OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND
        ));
    }

    #[test]
    fn scan_existing_ac_does_not_create_missing_templates() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create workspace");

        let updates =
            scan_project_context_template_updates(temp.path(), &workspace).expect("scan updates");

        assert!(updates.is_empty());
        assert!(!workspace.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME).exists());
        assert!(!workspace
            .join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .exists());
    }

    #[test]
    fn read_sync_creates_missing_coordinator_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create workspace");

        sync_project_context_template_for_read(&workspace, COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .expect("sync for read");

        let content =
            std::fs::read_to_string(workspace.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("read coordinator");
        assert_eq!(content, get_default_coordinator_template());
    }

    #[test]
    fn read_sync_updates_old_generated_coordinator_template() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create workspace");
        std::fs::write(
            workspace.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND,
        )
        .expect("write old coordinator");

        sync_project_context_template_for_read(&workspace, COORDINATOR_CONTEXT_TEMPLATE_FILENAME)
            .expect("sync for read");

        let content =
            std::fs::read_to_string(workspace.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("read coordinator");
        assert_eq!(content, get_default_coordinator_template());
    }

    #[test]
    fn custom_coordinator_is_preserved_and_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create workspace");
        let custom = "custom coordinator guidance";
        std::fs::write(
            workspace.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            custom,
        )
        .expect("write custom coordinator");

        let updates =
            scan_project_context_template_updates(temp.path(), &workspace).expect("scan updates");

        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].filename, COORDINATOR_CONTEXT_TEMPLATE_FILENAME);
        assert_eq!(
            std::fs::read_to_string(workspace.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("read coordinator"),
            custom
        );
    }

    #[test]
    fn global_unknown_without_state_is_not_reported() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create workspace");
        std::fs::write(
            workspace.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
            "legacy rendered global body with project paths",
        )
        .expect("write global");

        let updates =
            scan_project_context_template_updates(temp.path(), &workspace).expect("scan updates");

        assert!(updates.is_empty());
    }

    #[test]
    fn forged_manifest_does_not_auto_overwrite_custom_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create workspace");
        let custom = "custom coordinator with forged seeded hash";
        std::fs::write(
            workspace.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            custom,
        )
        .expect("write custom coordinator");
        let mut state = SeededContextTemplateState::default();
        state.templates.insert(
            "coordinator".to_string(),
            SeededContextTemplateEntry {
                template_id: "coordinator".to_string(),
                current_version: 1,
                last_seeded_sha256: Some(hash_text(custom)),
                last_observed_sha256: None,
                ignored_default_sha256: None,
                ignored_observed_sha256: None,
            },
        );
        persist_state(&workspace, &state).expect("persist forged state");

        let updates =
            scan_project_context_template_updates(temp.path(), &workspace).expect("scan updates");

        assert_eq!(updates.len(), 1);
        assert_eq!(
            std::fs::read_to_string(workspace.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("read coordinator"),
            custom
        );
    }

    #[test]
    fn dismiss_suppresses_same_file_and_default_hash() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create workspace");
        let custom = "custom coordinator guidance";
        std::fs::write(
            workspace.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            custom,
        )
        .expect("write custom coordinator");
        let update = scan_project_context_template_updates(temp.path(), &workspace)
            .expect("scan updates")
            .pop()
            .expect("pending update");

        dismiss_context_template_update(
            &workspace,
            &update.filename,
            &update.current_file_sha256,
            &update.current_default_sha256,
        )
        .expect("dismiss");

        let updates =
            scan_project_context_template_updates(temp.path(), &workspace).expect("scan again");
        assert!(updates.is_empty());
    }

    #[test]
    fn explicit_keep_repairs_invalid_state_json() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create workspace");
        let custom = "custom coordinator guidance";
        std::fs::write(
            workspace.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            custom,
        )
        .expect("write custom coordinator");
        std::fs::write(
            workspace.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME),
            "not json",
        )
        .expect("write invalid state");

        dismiss_context_template_update(
            &workspace,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            &hash_text(custom),
            &hash_text(get_default_coordinator_template()),
        )
        .expect("dismiss with invalid state");

        let repaired =
            std::fs::read_to_string(workspace.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
                .expect("read repaired state");
        let parsed: SeededContextTemplateState =
            serde_json::from_str(&repaired).expect("state is repaired JSON");
        assert_eq!(parsed.schema_version, STATE_SCHEMA_VERSION);
    }

    #[test]
    fn overwrite_creates_backup_and_writes_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create workspace");
        let custom = "custom coordinator guidance";
        std::fs::write(
            workspace.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME),
            custom,
        )
        .expect("write custom coordinator");
        let update = scan_project_context_template_updates(temp.path(), &workspace)
            .expect("scan updates")
            .pop()
            .expect("pending update");

        let result = overwrite_context_template_with_default(
            &workspace,
            &update.filename,
            &update.current_file_sha256,
            &update.current_default_sha256,
        )
        .expect("overwrite");

        assert_eq!(
            std::fs::read_to_string(workspace.join(COORDINATOR_CONTEXT_TEMPLATE_FILENAME))
                .expect("read coordinator"),
            get_default_coordinator_template()
        );
        assert_eq!(
            std::fs::read_to_string(result.backup_path).expect("read backup"),
            custom
        );
    }

    #[test]
    fn future_schema_is_not_rewritten_by_scan() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create workspace");
        let state_path = workspace.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);
        std::fs::write(&state_path, r#"{"schemaVersion":999,"templates":{}}"#)
            .expect("write future state");

        let updates =
            scan_project_context_template_updates(temp.path(), &workspace).expect("scan updates");

        assert!(updates.is_empty());
        assert_eq!(
            std::fs::read_to_string(state_path).expect("read state"),
            r#"{"schemaVersion":999,"templates":{}}"#
        );
    }

    #[test]
    fn state_directory_blocks_explicit_commands() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join(".ac");
        std::fs::create_dir(&workspace).expect("create workspace");
        std::fs::create_dir(workspace.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME))
            .expect("create state dir");

        let err = dismiss_context_template_update(
            &workspace,
            COORDINATOR_CONTEXT_TEMPLATE_FILENAME,
            "file",
            "default",
        )
        .expect_err("state dir must error");

        assert!(err.contains("state path"));
    }

    #[test]
    fn dedupe_keeps_distinct_file_hashes() {
        let mut updates = vec![
            ContextTemplateUpdate {
                project_path: "project".to_string(),
                workspace_path: "workspace".to_string(),
                file_path: "workspace/Context.coordinator.md".to_string(),
                filename: COORDINATOR_CONTEXT_TEMPLATE_FILENAME.to_string(),
                label: "Coordinator context".to_string(),
                current_file_sha256: "file-a".to_string(),
                current_default_sha256: "default".to_string(),
                current_default_version: 2,
            },
            ContextTemplateUpdate {
                project_path: "project".to_string(),
                workspace_path: "workspace".to_string(),
                file_path: "workspace/Context.coordinator.md".to_string(),
                filename: COORDINATOR_CONTEXT_TEMPLATE_FILENAME.to_string(),
                label: "Coordinator context".to_string(),
                current_file_sha256: "file-a".to_string(),
                current_default_sha256: "default".to_string(),
                current_default_version: 2,
            },
            ContextTemplateUpdate {
                project_path: "project".to_string(),
                workspace_path: "workspace".to_string(),
                file_path: "workspace/Context.coordinator.md".to_string(),
                filename: COORDINATOR_CONTEXT_TEMPLATE_FILENAME.to_string(),
                label: "Coordinator context".to_string(),
                current_file_sha256: "file-b".to_string(),
                current_default_sha256: "default".to_string(),
                current_default_version: 2,
            },
        ];

        dedupe_context_template_updates(&mut updates);

        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].current_file_sha256, "file-a");
        assert_eq!(updates[1].current_file_sha256, "file-b");
    }

    #[test]
    fn root_custom_template_is_preserved() {
        let temp = tempfile::tempdir().expect("tempdir");
        let custom = "custom root context";
        std::fs::write(
            temp.path()
                .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME),
            custom,
        )
        .expect("write root custom");

        ensure_root_context_template(temp.path()).expect("ensure root context");

        assert_eq!(
            std::fs::read_to_string(
                temp.path()
                    .join(crate::config::session_context::ROOT_AGENT_CONTEXT_TEMPLATE_FILENAME)
            )
            .expect("read root context"),
            custom
        );
    }
}
