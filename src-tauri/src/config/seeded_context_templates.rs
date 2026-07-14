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

/// #979: the standalone global context template that older builds seeded into the
/// APP CONFIG directory (307 bytes; it predates `## Core Concepts`). Retirement may
/// delete only bytes AgentsCommander provably generated itself, so this snapshot is
/// frozen and pinned by a length + SHA-256 test.
///
/// Do NOT replace this literal with `include_str!`: a raw string literal normalizes
/// `\r\n` to `\n` at compile time and `include_str!` does not, so on a CRLF checkout
/// `include_str!` would silently stop recognizing the generated default and every
/// retirement would fall through to "custom".
const STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS: &str = r#"# AgentsCommander Context

You are running inside an AgentsCommander session - a terminal session manager that coordinates multiple AI agents.

{{WRITE_RESTRICTIONS}}

{{DELEGATED_TASK_REPORTING}}

{{SKILLS_SECTION}}

{{WORKSPACE_REPOS}}

{{CLI_CONTEXT}}

{{SESSION_CREDENTIALS}}

{{INTER_AGENT_MESSAGING}}
"#;

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

/// #979: exact recognition of a STANDALONE (app-config) generated global context.
///
/// True only for byte-for-byte UTF-8 equality with the current built-in default or
/// with the frozen 307-byte snapshot above. No normalization of whitespace, line
/// endings, BOMs, or trailing newlines, and seed-state hashes are never consulted:
/// a CRLF copy, an invalid-UTF-8 file, a one-byte edit, or a state entry claiming
/// ownership is UNKNOWN, and unknown content is backed up, never deleted.
///
/// Deliberately separate from `is_known_generated_global_template`, which drives
/// PROJECT auto-update behavior through `project_specs()`. Root retirement must not
/// widen that.
fn is_known_generated_standalone_global_template(content: &str) -> bool {
    content == crate::config::session_context::get_default_agent_template()
        || content == STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS
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

/// #979: retire the standalone global context template that older builds seeded
/// into the app-config directory next to `ac-root-agent`.
///
/// Conservative and lossless. Known generated bytes are deleted; every other byte
/// sequence, including invalid UTF-8, is moved to an inert timestamped backup and
/// kept. On any uncertain classification, bytes are preserved.
///
/// The only caller (`root_agent::ensure_root_agent_dir_at`) consumes every `Err` as
/// a warning and continues, so this may report failures freely. It must simply never
/// destroy bytes and never recreate the active global name.
///
/// File retirement runs BEFORE state cleanup on purpose: two directory entries
/// cannot change in one filesystem transaction, so the sequence is made retryable
/// instead. After any crash boundary the live global is either still present (and
/// retried next run) or already absent with its bytes in an inert backup. Never
/// reverse the order: it would erase the only ownership record while leaving a
/// still-active custom global on disk.
pub(crate) fn retire_standalone_global_context(config_dir: &Path) -> Result<(), String> {
    retire_standalone_global_context_with(
        config_dir,
        crate::config::root_agent::atomic_replace_existing,
    )
}

/// Test seam for the failure paths, mirroring the closure-based filesystem seam in
/// `session_context::migrate_legacy_agent_context_template_with`. `publish` moves the
/// live entry onto the reserved inert name. Production always passes
/// `atomic_replace_existing`; this is not a second production algorithm.
fn retire_standalone_global_context_with(
    config_dir: &Path,
    publish: impl Fn(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    validate_existing_dir(config_dir, "Root agent config directory")?;

    let live_path =
        config_dir.join(crate::config::session_context::GLOBAL_CONTEXT_TEMPLATE_FILENAME);
    match std::fs::symlink_metadata(&live_path) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                // Never follow, delete, or move a symlink, reparse point, or
                // non-file. The caller warns and continues; the entry survives.
                return Err(format!(
                    "Standalone global context {} exists but is not a regular file",
                    live_path.display()
                ));
            }
            retire_live_standalone_global(&live_path, publish)?;
        }
        // No live file: nothing to move, but a stale `global` state entry may still
        // need to converge.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to inspect standalone global context {}: {}",
                live_path.display(),
                e
            ))
        }
    }

    remove_global_state_entry(config_dir)
}

fn retire_live_standalone_global(
    live_path: &Path,
    publish: impl Fn(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    let backup_path = reserve_retired_backup_path(live_path)?;

    // Same-directory rename / ReplaceFileW, never a copy followed by a delete, so the
    // bytes are never decoded, re-encoded, or truncated. `atomic_replace_existing`
    // publishes `temp -> dest`, so `(live, backup)` is the correct argument order.
    if let Err(e) = publish(live_path, &backup_path) {
        // Never blindly delete the destination after a failed move. Remove the
        // reservation ONLY when the source is still a regular file AND the
        // destination is still the zero-byte reservation this call created. An empty
        // custom source is a valid unknown file, so "the source disappeared" is never
        // proof that an empty destination is disposable.
        let source_intact = std::fs::symlink_metadata(live_path)
            .map(|m| !is_link_or_reparse(&m) && m.is_file())
            .unwrap_or(false);
        let dest_is_reservation = std::fs::symlink_metadata(&backup_path)
            .map(|m| !is_link_or_reparse(&m) && m.is_file() && m.len() == 0)
            .unwrap_or(false);
        if source_intact && dest_is_reservation {
            if let Err(cleanup_error) = std::fs::remove_file(&backup_path) {
                log::warn!(
                    "[979] failed to remove the unused retirement reservation {}: {}",
                    backup_path.display(),
                    cleanup_error
                );
            }
        } else {
            log::warn!(
                "[979] preserving {} after a failed retirement move: the source or the destination is no longer the entry this call created",
                backup_path.display()
            );
        }
        return Err(e);
    }

    if let Some(parent) = backup_path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            if let Err(e) = dir.sync_all() {
                log::warn!(
                    "[979] failed to sync the retirement directory {}: {}",
                    parent.display(),
                    e
                );
            }
        }
    }

    // Re-check before reading: a concurrent replacement could have swapped the inert
    // name for a link, a reparse point, or a directory.
    let metadata = std::fs::symlink_metadata(&backup_path).map_err(|e| {
        format!(
            "Failed to inspect retired context backup {}: {}",
            backup_path.display(),
            e
        )
    })?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "Retired context backup {} is no longer a regular file",
            backup_path.display()
        ));
    }

    // Classify from RAW BYTES. `read_validated_snapshot` is deliberately NOT reused:
    // it converts to `String` and errors on invalid UTF-8, while #979 requires invalid
    // bytes to SURVIVE. Invalid UTF-8 is automatically custom.
    let bytes = std::fs::read(&backup_path).map_err(|e| {
        format!(
            "Failed to read retired context backup {}: {}",
            backup_path.display(),
            e
        )
    })?;
    let is_generated = std::str::from_utf8(&bytes)
        .ok()
        .is_some_and(is_known_generated_standalone_global_template);

    if !is_generated {
        log::warn!(
            "[979] {} held unknown or custom content and was moved to the inert backup {}; the Root Agent no longer consumes it",
            live_path.display(),
            backup_path.display()
        );
        return Ok(());
    }

    // Known generated bytes: re-read and compare immediately before deleting. If they
    // differ or cannot be read, keep the backup and report. A delete failure leaves an
    // inert backup and returns an error; it never restores the active global name.
    let recheck = std::fs::read(&backup_path).map_err(|e| {
        format!(
            "Failed to re-read retired context backup {} before deleting it: {}",
            backup_path.display(),
            e
        )
    })?;
    if recheck != bytes {
        return Err(format!(
            "Retired context backup {} changed while it was being classified; keeping it",
            backup_path.display()
        ));
    }
    std::fs::remove_file(&backup_path).map_err(|e| {
        format!(
            "Failed to delete the retired generated context backup {}: {}",
            backup_path.display(),
            e
        )
    })?;
    log::info!(
        "[979] retired the standalone generated global context {}",
        live_path.display()
    );
    Ok(())
}

/// Reserve a unique inert same-directory name with `create_new(true)` and drop the
/// handle before the move: the shared atomic primitive (and Windows replacement
/// semantics) require the destination handle to be closed.
///
/// The name is ALWAYS timestamped, so it can never collide with `create_backup`'s
/// `{f}.bak` / `{f}.{ts}.bak` / `{f}.{ts}.{n}.bak` shapes. `create_backup` itself is
/// unusable here: it write_all's a COPY and its first name is untimestamped.
fn reserve_retired_backup_path(live_path: &Path) -> Result<PathBuf, String> {
    let parent = live_path.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for standalone global context {}",
            live_path.display()
        )
    })?;
    let filename = live_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid standalone global context {}", live_path.display()))?;
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%SZ").to_string();

    for index in 0..1000_u32 {
        let backup_name = match index {
            0 => format!("{filename}.retired-{timestamp}.bak"),
            n => format!("{filename}.retired-{timestamp}.{n}.bak"),
        };
        let backup_path = parent.join(backup_name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup_path)
        {
            Ok(file) => {
                drop(file);
                return Ok(backup_path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to reserve the retirement backup path {}: {}",
                    backup_path.display(),
                    e
                ))
            }
        }
    }

    Err(format!(
        "Failed to reserve a unique retirement backup path for {}",
        live_path.display()
    ))
}

/// #979 remove ONLY the portable `global` state entry, and never rewrite state this
/// function did not change.
///
/// `persist_state_strict` must never be called here. It writes whenever `dirty` is
/// set, regardless of whether the caller removed anything, and `load_state` returns
/// an EMPTY templates map with `trusted: true`, `can_persist: true`, `dirty: true` on
/// malformed JSON at EITHER strictness. Composing the two would overwrite a corrupt
/// manifest with `{"schemaVersion":1,"templates":{}}` and destroy the `coordinator`
/// and `rootAgent` entries this function is required to preserve.
///
/// `strict = true` is a hazard in its own right: a symlinked or unstattable state
/// file makes `load_state` return `Err`, and although the caller is best-effort, a
/// stale entry is not worth the noise. Never infer generated ownership from
/// `lastSeededSha256` or any other state field.
fn remove_global_state_entry(config_dir: &Path) -> Result<(), String> {
    let mut loaded = load_state(config_dir, false)?;
    if !loaded.trusted || !loaded.can_persist || loaded.dirty {
        log::warn!(
            "[979] portable context-template state at {} is unreadable or malformed; leaving it untouched (the standalone global is already retired)",
            config_dir.display()
        );
        return Ok(());
    }
    // `templates` is a BTreeMap, so the removed entry is the ONLY proof that a write
    // is warranted. Only `global` may ever be removed: `coordinator` and `rootAgent`
    // stay.
    match loaded.state.templates.remove("global") {
        None => Ok(()),
        Some(_) => persist_state(config_dir, &loaded.state),
    }
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

    // ---------------------------------------------------------------- #979 retirement

    fn live_global(config_dir: &Path) -> PathBuf {
        config_dir.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME)
    }

    fn retired_backups(config_dir: &Path) -> Vec<PathBuf> {
        let prefix = format!("{}.retired-", GLOBAL_CONTEXT_TEMPLATE_FILENAME);
        let mut found: Vec<PathBuf> = std::fs::read_dir(config_dir)
            .expect("read config dir")
            .filter_map(|entry| {
                let path = entry.expect("dir entry").path();
                let name = path.file_name()?.to_str()?.to_string();
                (name.starts_with(&prefix) && name.ends_with(".bak")).then_some(path)
            })
            .collect();
        found.sort();
        found
    }

    fn state_path(config_dir: &Path) -> PathBuf {
        config_dir.join(SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)
    }

    /// Every custom or unknown byte sequence must end up in exactly one inert backup,
    /// byte-for-byte, with the active global name gone.
    fn assert_custom_bytes_preserved(config_dir: &Path, original: &[u8]) {
        assert!(
            !live_global(config_dir).exists(),
            "the active global name must be retired"
        );
        let backups = retired_backups(config_dir);
        assert_eq!(backups.len(), 1, "expected exactly one inert backup");
        assert_eq!(
            std::fs::read(&backups[0]).expect("read backup"),
            original,
            "custom bytes must survive byte-for-byte"
        );
    }

    #[test]
    fn frozen_standalone_global_snapshot_is_pinned() {
        // #979 4.3.A: the 307-byte snapshot must never drift. If this fails, do NOT
        // "fix" the expectation: a changed literal silently widens or narrows what
        // retirement is willing to DELETE.
        assert_eq!(
            STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS.len(),
            307,
            "the frozen standalone global snapshot must stay 307 bytes"
        );
        assert_eq!(
            hash_text(STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS),
            "e0cbc16fbef5bf5ae116e5268b24a987be6834eaac50e7ac4441a57fc90678f3"
        );
        // It predates Core Concepts; that is exactly why Root would have lost the
        // section if the prologue had been assembled from the seven mandatory blocks.
        assert!(!STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS.contains("## Core Concepts"));
        assert!(is_known_generated_standalone_global_template(
            STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS
        ));
        assert!(is_known_generated_standalone_global_template(
            crate::config::session_context::get_default_agent_template()
        ));
    }

    #[test]
    fn retire_deletes_the_current_generated_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let current = crate::config::session_context::get_default_agent_template();
        std::fs::write(live_global(temp.path()), current).expect("write generated global");

        retire_standalone_global_context(temp.path()).expect("retire");

        assert!(!live_global(temp.path()).exists());
        assert!(
            retired_backups(temp.path()).is_empty(),
            "known generated bytes are deleted, not retained"
        );
    }

    #[test]
    fn retire_deletes_the_frozen_generated_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            live_global(temp.path()),
            STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS,
        )
        .expect("write frozen global");

        retire_standalone_global_context(temp.path()).expect("retire");

        assert!(!live_global(temp.path()).exists());
        assert!(retired_backups(temp.path()).is_empty());
    }

    #[test]
    fn retire_backs_up_a_one_byte_edit_of_a_generated_default() {
        let temp = tempfile::tempdir().expect("tempdir");
        let edited = format!("{}X", STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS);
        std::fs::write(live_global(temp.path()), &edited).expect("write edited global");

        retire_standalone_global_context(temp.path()).expect("retire");

        assert_custom_bytes_preserved(temp.path(), edited.as_bytes());
    }

    #[test]
    fn retire_treats_normalized_variants_of_a_generated_default_as_custom() {
        // #979 4.3.A: NO normalization. A CRLF copy, a BOM, trailing whitespace, and a
        // zero-byte file are each UNKNOWN and must be preserved, never deleted.
        let frozen = STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS;
        let crlf = frozen.replace('\n', "\r\n").into_bytes();
        let mut bom = vec![0xEF_u8, 0xBB, 0xBF];
        bom.extend_from_slice(frozen.as_bytes());
        let trailing = format!("{}\n", frozen).into_bytes();
        let variants: Vec<(&str, Vec<u8>)> = vec![
            ("crlf", crlf),
            ("bom", bom),
            ("trailing whitespace", trailing),
            ("zero-byte", Vec::new()),
        ];

        for (label, bytes) in variants {
            let temp = tempfile::tempdir().expect("tempdir");
            std::fs::write(live_global(temp.path()), &bytes).expect("write variant");

            retire_standalone_global_context(temp.path()).expect("retire");

            assert!(!live_global(temp.path()).exists(), "case: {}", label);
            let backups = retired_backups(temp.path());
            assert_eq!(backups.len(), 1, "case: {}", label);
            assert_eq!(
                std::fs::read(&backups[0]).expect("read backup"),
                bytes,
                "case: {}: bytes must survive",
                label
            );
        }
    }

    #[test]
    fn retire_preserves_invalid_utf8_bytes() {
        // Invalid UTF-8 is automatically custom: the classifier reads RAW BYTES and
        // never goes through the String-converting snapshot reader.
        let temp = tempfile::tempdir().expect("tempdir");
        let bytes = vec![0x00_u8, 0xFF, 0xFE, 0x41, 0x80, 0x0A];
        std::fs::write(live_global(temp.path()), &bytes).expect("write invalid utf-8");

        retire_standalone_global_context(temp.path()).expect("retire");

        assert_custom_bytes_preserved(temp.path(), &bytes);
    }

    #[test]
    fn retire_ignores_forged_state_claiming_a_custom_file_is_seeded() {
        // State is never classification evidence. A forged `global` entry claiming the
        // custom file is generated must not license deleting it.
        let temp = tempfile::tempdir().expect("tempdir");
        let custom = "# Custom Global\n\nKEEP_THESE_BYTES\n";
        std::fs::write(live_global(temp.path()), custom).expect("write custom global");
        let forged = format!(
            r#"{{"schemaVersion":1,"templates":{{"global":{{"templateId":"global","currentVersion":1,"lastSeededSha256":"{}"}}}}}}"#,
            hash_text(custom)
        );
        std::fs::write(state_path(temp.path()), &forged).expect("write forged state");

        retire_standalone_global_context(temp.path()).expect("retire");

        assert_custom_bytes_preserved(temp.path(), custom.as_bytes());
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(state_path(temp.path())).expect("read state"))
                .expect("parse state");
        assert!(
            state["templates"]["global"].is_null(),
            "the stale entry converges"
        );
    }

    #[test]
    fn retire_removes_only_the_global_state_entry() {
        // #979 G3: `coordinator`, `rootAgent`, and unrelated keys must all survive.
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            live_global(temp.path()),
            crate::config::session_context::get_default_agent_template(),
        )
        .expect("write generated global");
        let state = r#"{"schemaVersion":1,"templates":{"coordinator":{"templateId":"coordinator","currentVersion":2},"global":{"templateId":"global","currentVersion":1},"rootAgent":{"templateId":"rootAgent","currentVersion":4},"unrelated":{"templateId":"unrelated","currentVersion":7}}}"#;
        std::fs::write(state_path(temp.path()), state).expect("write state");

        retire_standalone_global_context(temp.path()).expect("retire");

        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(state_path(temp.path())).expect("read state"))
                .expect("parse state");
        assert!(parsed["templates"]["global"].is_null());
        assert_eq!(parsed["templates"]["coordinator"]["currentVersion"], 2);
        assert_eq!(parsed["templates"]["rootAgent"]["currentVersion"], 4);
        assert_eq!(parsed["templates"]["unrelated"]["currentVersion"], 7);
    }

    #[test]
    fn retire_leaves_malformed_state_untouched_and_still_retires_the_file() {
        // #979 G3 / 4.3.C. This is the case that `persist_state_strict` would destroy:
        // `load_state` returns an EMPTY map with `dirty: true` on malformed JSON, and
        // the strict wrapper writes whenever `dirty` is set, so the "safe" idiom would
        // overwrite this file with `{"templates":{}}` and wipe `coordinator` and
        // `rootAgent`. Retirement must return Ok, leave the state byte-identical, and
        // still retire the live global.
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            live_global(temp.path()),
            crate::config::session_context::get_default_agent_template(),
        )
        .expect("write generated global");
        let malformed = "{not json at all";
        std::fs::write(state_path(temp.path()), malformed).expect("write malformed state");

        retire_standalone_global_context(temp.path()).expect("retirement must return Ok");

        assert!(
            !live_global(temp.path()).exists(),
            "the live global is still retired"
        );
        assert_eq!(
            std::fs::read(state_path(temp.path())).expect("read state"),
            malformed.as_bytes(),
            "a malformed state file must be left byte-identical"
        );
    }

    #[test]
    fn retire_converges_a_stale_state_entry_without_a_live_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = r#"{"schemaVersion":1,"templates":{"global":{"templateId":"global","currentVersion":1},"rootAgent":{"templateId":"rootAgent","currentVersion":4}}}"#;
        std::fs::write(state_path(temp.path()), state).expect("write state");

        retire_standalone_global_context(temp.path()).expect("retire");

        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(state_path(temp.path())).expect("read state"))
                .expect("parse state");
        assert!(parsed["templates"]["global"].is_null());
        assert_eq!(parsed["templates"]["rootAgent"]["currentVersion"], 4);
        assert!(retired_backups(temp.path()).is_empty());
    }

    #[test]
    fn retire_is_idempotent_and_never_rewrites_an_untouched_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let custom = "# Custom Global\n\nKEEP\n";
        std::fs::write(live_global(temp.path()), custom).expect("write custom global");

        retire_standalone_global_context(temp.path()).expect("retire once");
        let backups_after_first = retired_backups(temp.path());
        assert_eq!(backups_after_first.len(), 1);
        let state_exists_after_first = state_path(temp.path()).exists();

        retire_standalone_global_context(temp.path()).expect("retire twice");

        assert_eq!(
            retired_backups(temp.path()),
            backups_after_first,
            "a second call creates no new backup"
        );
        assert_eq!(
            state_path(temp.path()).exists(),
            state_exists_after_first,
            "with no `global` entry to remove, the state file is never written"
        );
    }

    #[test]
    fn retire_refuses_a_directory_at_the_live_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let live = live_global(temp.path());
        std::fs::create_dir_all(&live).expect("create dir at the live path");
        std::fs::write(live.join("inner.md"), "KEEP_ME\n").expect("write inner");

        let err = retire_standalone_global_context(temp.path())
            .expect_err("a non-file at the live path must be reported");
        assert!(err.contains("is not a regular file"), "{}", err);

        assert!(live.is_dir(), "the entry must be preserved");
        assert_eq!(
            std::fs::read_to_string(live.join("inner.md")).expect("read inner"),
            "KEEP_ME\n"
        );
        assert!(retired_backups(temp.path()).is_empty());
    }

    #[test]
    fn retire_refuses_a_symlink_at_the_live_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = temp.path().join("elsewhere.md");
        std::fs::write(&target, "TARGET_BYTES\n").expect("write target");
        let live = live_global(temp.path());
        // Windows may deny symlink creation without developer mode; keep the project
        // convention of returning early when it does. The directory case above always
        // runs.
        if try_symlink_file(&target, &live).is_err() {
            return;
        }

        let err = retire_standalone_global_context(temp.path())
            .expect_err("a symlink at the live path must be reported");
        assert!(err.contains("is not a regular file"), "{}", err);

        assert!(std::fs::symlink_metadata(&live)
            .expect("inspect link")
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(&target).expect("read target"),
            "TARGET_BYTES\n",
            "the symlink must never be followed, moved, or deleted"
        );
        assert!(retired_backups(temp.path()).is_empty());
    }

    #[test]
    fn retire_cleans_up_the_reservation_on_a_definite_pre_move_failure() {
        // The publish seam fails BEFORE moving anything: the source is intact and the
        // destination is still this call's zero-byte reservation, so the reservation is
        // safe to remove.
        let temp = tempfile::tempdir().expect("tempdir");
        let custom = "# Custom Global\n\nKEEP\n";
        std::fs::write(live_global(temp.path()), custom).expect("write custom global");

        let err = retire_standalone_global_context_with(temp.path(), |_, _| {
            Err("simulated pre-move failure".to_string())
        })
        .expect_err("the publish failure must be reported");
        assert!(err.contains("simulated pre-move failure"), "{}", err);

        assert_eq!(
            std::fs::read_to_string(live_global(temp.path())).expect("read live global"),
            custom,
            "the live global must be untouched"
        );
        assert!(
            retired_backups(temp.path()).is_empty(),
            "the unused reservation must be cleaned up"
        );
    }

    #[test]
    fn retire_preserves_the_destination_when_the_source_disappeared() {
        // The AMBIGUOUS failure: the publish seam reports an error but the source is
        // gone. An empty custom source is a valid unknown file, so source disappearance
        // is never proof that an empty destination is disposable. Keep the destination.
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(live_global(temp.path()), "# Custom Global\n").expect("write custom global");

        let err = retire_standalone_global_context_with(temp.path(), |source, _| {
            std::fs::remove_file(source).expect("simulate a vanished source");
            Err("simulated ambiguous failure".to_string())
        })
        .expect_err("the publish failure must be reported");
        assert!(err.contains("simulated ambiguous failure"), "{}", err);

        let backups = retired_backups(temp.path());
        assert_eq!(
            backups.len(),
            1,
            "an ambiguous failure must PRESERVE the destination, never delete it"
        );
    }

    #[test]
    fn retire_leaves_a_project_global_and_its_state_untouched() {
        // The whole point of #979: retiring the APP CONFIG directory must not touch the
        // project's `.ac` global or its project state.
        let temp = tempfile::tempdir().expect("tempdir");
        let config_dir = temp.path().join("portable-config");
        let project_ac = temp.path().join("project").join(".ac");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        std::fs::create_dir_all(&project_ac).expect("create project .ac");

        std::fs::write(
            live_global(&config_dir),
            crate::config::session_context::get_default_agent_template(),
        )
        .expect("write app-config global");

        let project_global = "# Project Global\n\nEDITABLE_PROJECT_GLOBAL\n";
        std::fs::write(live_global(&project_ac), project_global).expect("write project global");
        let project_state = r#"{"schemaVersion":1,"templates":{"global":{"templateId":"global","currentVersion":1}}}"#;
        std::fs::write(state_path(&project_ac), project_state).expect("write project state");

        retire_standalone_global_context(&config_dir).expect("retire");

        assert!(!live_global(&config_dir).exists());
        assert_eq!(
            std::fs::read(live_global(&project_ac)).expect("read project global"),
            project_global.as_bytes(),
            "the project global must be byte-for-byte unchanged"
        );
        assert_eq!(
            std::fs::read(state_path(&project_ac)).expect("read project state"),
            project_state.as_bytes(),
            "the project state must be byte-for-byte unchanged"
        );
    }

    #[cfg(unix)]
    fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn try_symlink_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }
}
