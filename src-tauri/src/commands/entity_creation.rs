use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::cli::task_ops;
use crate::commands::ac_discovery::DiscoveryBranchWatcher;
use crate::config::ac_root::existing_ac_root;
use crate::config::coding_agent_profiles::{
    apply_replica_selection_default, read_replica_selection_default,
};
use crate::config::replica_identity::{
    normalize_wg_replica_context_entries, repair_wg_replica_config_value, ROLE_MD_FILENAME,
    WG_REPLICA_REQUIRED_CONTEXT,
};
use crate::config::seed_manifest::{
    ManifestActivationToken, ManifestLifecycleFilter, ProjectSeedManifestGuard, SeedManifestError,
};
use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::git_watcher::{CoordinatorChangedPayload, GitWatcher};
use crate::session::manager::SessionManager;
use crate::session::session::{SessionRepo, SessionStatus};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedEntityResult {
    /// Absolute path to the created directory
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub path: String,
    pub project_name: String,
    pub project_path: String,
    pub is_local: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamAgentSource {
    pub agent_ref: String,
    pub source_project_path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoAssignment {
    pub url: String,
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamConfigResult {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub coordinator: String,
    #[serde(default)]
    pub repos: Vec<RepoAssignment>,
    #[serde(default)]
    pub context_alert_percentages: Vec<u8>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkgroupCloneResult {
    /// Absolute path to the created workgroup directory
    pub path: String,
    /// Repos that failed to clone (url + error message). Empty = all succeeded.
    pub clone_errors: Vec<CloneError>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneError {
    pub url: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncError {
    pub replica: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub workgroups_updated: u32,
    pub replicas_updated: u32,
    pub errors: Vec<SyncError>,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkgroupDiskCreateArgs {
    pub project_path: PathBuf,
    pub team_name: String,
    pub task_title: String,
    // Legacy provisioning path. Ordinary callers should pass None and empty
    // vectors so workgroup creation activates an existing team config.
    pub coordinator: Option<String>,
    pub agents: Vec<String>,
    pub repos: Vec<RepoAssignment>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReplicaDiskCreateArgs {
    pub ac_root: PathBuf,
    pub wg_dir: PathBuf,
    pub agent_path: String,
    pub team_repos: Vec<RepoAssignment>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Agent Matrix / replica directory contract (issue #209)
// ---------------------------------------------------------------------------
//
// Agent Matrices own canonical state (`memory/`, `plans/`, `skills/`) plus
// mailboxes. Workgroup replicas own only mailboxes; their config points back
// to the origin matrix for canonical state.
pub(crate) const AGENT_MATRIX_DIRS: &[&str] = &["memory", "plans", "skills", "inbox", "outbox"];
pub(crate) const AGENT_REPLICA_DIRS: &[&str] = &["inbox", "outbox"];

fn create_agent_matrix_subdirs(agent_dir: &Path) -> Result<(), (&'static str, std::io::Error)> {
    for &sub in AGENT_MATRIX_DIRS {
        std::fs::create_dir_all(agent_dir.join(sub)).map_err(|e| (sub, e))?;
    }
    Ok(())
}

/// Create the full Agent Matrix layout, including the root directory.
///
/// The returned tag is `"agent_dir"` for root failures or the failing subdir
/// name for child directory failures, so callers can preserve diagnostics.
pub(crate) fn create_agent_matrix_layout(
    agent_dir: &Path,
) -> Result<(), (&'static str, std::io::Error)> {
    std::fs::create_dir_all(agent_dir).map_err(|e| ("agent_dir", e))?;
    create_agent_matrix_subdirs(agent_dir)
}

/// Create a new Agent Matrix layout after atomically claiming the root.
///
/// Unlike `create_agent_matrix_layout`, this fails if the root already exists.
/// Use it for user-facing creation paths so concurrent creates cannot overwrite
/// each other's Role.md or config.json.
pub(crate) fn create_new_agent_matrix_layout(
    agent_dir: &Path,
) -> Result<(), (&'static str, std::io::Error)> {
    std::fs::create_dir(agent_dir).map_err(|e| ("agent_dir", e))?;
    create_agent_matrix_subdirs(agent_dir)
}

/// Create the full workgroup replica layout, including the root directory.
///
/// Canonical state is intentionally absent here; replicas reference the origin
/// Agent Matrix through `config.json`.
pub(crate) fn create_agent_replica_layout(
    replica_dir: &Path,
) -> Result<(), (&'static str, std::io::Error)> {
    std::fs::create_dir_all(replica_dir).map_err(|e| ("replica_dir", e))?;
    for &sub in AGENT_REPLICA_DIRS {
        std::fs::create_dir_all(replica_dir.join(sub)).map_err(|e| (sub, e))?;
    }
    Ok(())
}

/// Sanitize a user-provided name into a safe directory component:
/// lowercase, only a-z 0-9 and hyphens, no leading/trailing hyphens.
pub(crate) fn sanitize_name(raw: &str) -> Result<String, String> {
    let sanitized: String = raw
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if sanitized.is_empty() {
        return Err("Name must contain at least one alphanumeric character".into());
    }
    Ok(sanitized)
}

/// Validate that an existing entity name is safe for path operations.
/// Unlike `sanitize_name`, this does NOT transform the name — it just rejects
/// names that contain path traversal or separator characters.
///
/// `pub(crate)` so the sentinel-collision invariant test in
/// `wg_delete_diagnostic::tests` can prove that no valid WG name can collide
/// with the `BLOCKERS:` / `DIRTY_REPOS:` sentinel prefixes.
pub(crate) fn validate_existing_name(name: &str, entity_label: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(format!("{} name cannot be empty", entity_label));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!(
            "Invalid {} name: only alphanumeric characters and hyphens are allowed",
            entity_label
        ));
    }
    Ok(())
}

/// Extract a repo directory name from a git URL.
/// `https://github.com/org/my-repo.git` → `my-repo`
fn repo_dir_name_from_url(url: &str) -> String {
    let without_trailing = url.trim_end_matches('/');
    let last_segment = without_trailing.rsplit('/').next().unwrap_or("repo");
    last_segment
        .strip_suffix(".git")
        .unwrap_or(last_segment)
        .to_string()
}

/// Check if a team-config agent entry (absolute path or dir name) matches a given agent name.
/// `agent_name` is the bare name (e.g., "dev-rust"), not prefixed.
fn agent_matches(team_agent_entry: &str, agent_name: &str) -> bool {
    let entry_dir = Path::new(team_agent_entry)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(team_agent_entry);
    entry_dir == format!("_agent_{}", agent_name) || entry_dir == agent_name
}

/// Parse YAML frontmatter from a Role.md file.
/// Returns (name, description) if found. Strips a leading UTF-8 BOM first so a
/// `Role.md` saved as UTF-8-with-BOM still yields its display name/description
/// — mirrors `role_templates::parse_template_frontmatter`.
fn parse_role_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    if !content.starts_with("---") {
        return (None, None);
    }

    let rest = &content[3..];
    let end = match rest.find("---") {
        Some(i) => i,
        None => return (None, None),
    };

    let frontmatter = &rest[..end];
    let mut name = None;
    let mut description = None;

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("name:") {
            name = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(val) = trimmed.strip_prefix("description:") {
            description = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }

    (name, description)
}

/// Serialize the existing storage snapshot, preserving the original room key.
/// Reading may complete pending Clean recovery under the storage lock.
pub(crate) fn read_room_task_snapshot_json(root: &Path) -> Result<serde_json::Value, String> {
    let snapshot = task_ops::read_snapshot(root).map_err(|error| error.to_string())?;
    let mut value = serde_json::to_value(snapshot).map_err(|error| error.to_string())?;
    value["workgroupRoot"] = serde_json::Value::String(
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(root),
    );
    Ok(value)
}

/// Extract a `title:` field from the YAML frontmatter at the start of `content`.
///
/// Best-effort frontmatter detection — NOT a YAML implementation. Suitable
/// only for the narrow case of one optional scalar field at the top of
/// TASK.md.
///
/// Returns `Some(title)` when:
///   - `content` starts with `---`,
///   - a closing `---` exists,
///   - a line of the form `<key>: <value>` exists between the delimiters
///     where `<key>` matches `title` case-insensitively (`title:`, `Title:`,
///     `TITLE:`, mixed casing all accepted).
///
/// The value half is preserved verbatim (case-sensitive), then stripped of
/// surrounding `"` or `'` quote pairs.
///
/// Returns `None` otherwise (no frontmatter, no title key, or empty value).
///
/// Mirrors `parse_role_frontmatter`'s shape — both speak the same on-disk
/// format. See plan `_plans/107-auto-brief-title.md` §6 for why we do not
/// pull in `serde_yaml`.
pub(crate) fn parse_task_title(content: &str) -> Option<String> {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end = rest.find("---")?;
    let frontmatter = &rest[..end];

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        // Case-insensitive key match on `title:`. Round 2 fold (F3 / G3):
        // agents stochastically capitalize keys (`Title:`, `TITLE:`); a
        // case-sensitive match would let duplicate `title:` lines accumulate
        // across restarts. Split on the first `:` so we compare just the key.
        let Some((key, value_raw)) = trimmed.split_once(':') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("title") {
            continue;
        }
        let value = value_raw
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        if value.is_empty() {
            return None;
        }
        return Some(value);
    }
    None
}

/// TASK.md content for a brand-new workgroup.
///
/// Empty titles start Clean; explicit titles use the title editor's frontmatter.
fn build_task_content(task_title: &str) -> String {
    if task_title.trim().is_empty() {
        return "---\ntitle: 'Clean'\n---\n".to_string();
    }
    // #738: the workgroup-creation title is a human decision, so store it as
    // user-owned (`USER:`). This locks it against coordinator auto-retitle until
    // a Clean resets the task. `user_owned_title` trims and avoids double-prefix.
    let title = task_ops::user_owned_title(task_title);
    let escaped = title.replace('\'', "''");
    format!("---\ntitle: '{}'\n---\n", escaped)
}

/// Validate and prepare the initial task before the GUI creates any files.
fn prepare_new_room_task_content(task_title: &str) -> Result<String, String> {
    let task_title = task_title.trim();
    if !task_title.is_empty() {
        validate_task_title(task_title)?;
    }
    Ok(build_task_content(task_title))
}

/// Build the Role.md body written by `create_agent_matrix`. Kept as a separate
/// pure helper so the legacy (no-template) format can be unit-tested without
/// touching the Tauri command surface. `template` is the resolved template; when
/// `None`, output is byte-identical to the pre-#271 inline `format!`.
///
/// When a template is supplied, its body is inserted as a `## Role Profile`
/// section between the title/description and the mandatory `## Source of Truth`
/// / `## Agent Memory Rule` sections (plan §7) — the mandatory sections always
/// land last so a template body cannot push them off, and Role-Profile content
/// inherits the picker's display order (the section header makes the source of
/// the imported text obvious to a human reader of Role.md).
pub(crate) fn build_role_content(
    safe_name: &str,
    description: &str,
    template: Option<&crate::commands::role_templates::ResolvedRoleTemplate>,
) -> String {
    let desc_yaml = description.replace('\'', "''");
    // Plan §7.2: imported template body is fenced by HTML-comment delimiters so
    // the section boundary stays machine-detectable (and the opening tag
    // records provenance via the template id). Pre-release maintainer review
    // (§10.2) screens template bodies for the literal closing delimiter to
    // prevent a body from "escaping" the fenced section.
    let profile = match template {
        Some(t) => format!(
            "\n## Role Profile\n\n\
             <!-- ac:role-profile source=\"{}\" — imported template body; \
             the AC sections below are mandatory and must stay last -->\n\n\
             {}\n\n\
             <!-- ac:role-profile:end -->\n",
            t.id,
            t.body.trim(),
        ),
        None => String::new(),
    };
    format!(
        "---\nname: '{name}'\ndescription: '{desc_yaml}'\ntype: agent\n---\n\n\
         # {name}\n\n\
         {description}\n\
         {profile}\n\
         ## Source of Truth\n\n\
         This role is defined in Role.md of your Agent Matrix at: .ac/_agent_{name}/\n\
         If you are running as a replica, this file was generated from that source; the \
         Agent Matrix is canonical.\n\n\
         ## Agent Memory Rule\n\n\
         Your Agent Matrix's memory/, plans/, skills/, and Role.md are the single source \
         of persistent knowledge. Use your replica folder only for replica-local scratch, \
         inbox/outbox, and session artifacts. NEVER use external memory systems from the \
         coding agent (e.g., ~/.claude/projects/memory/).\n",
        name = safe_name,
        desc_yaml = desc_yaml,
        description = description,
        profile = profile,
    )
}

pub(crate) struct CreateAgentMatrixDiskArgs<'a> {
    pub project_path: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub role_template_id: Option<&'a str>,
    pub settings: &'a AppSettings,
    pub config_dir: Option<&'a Path>,
}

#[derive(Debug, Clone)]
pub(crate) struct CreatedAgentMatrixOnDisk {
    pub agent_dir: PathBuf,
    pub display_name: String,
    pub safe_name: String,
    pub role_path: PathBuf,
}

fn agent_matrix_display_name(project_path: &Path, safe_name: &str) -> String {
    let project_folder = project_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| project_path.to_string_lossy().to_string());
    format!("{}/{}", project_folder, safe_name)
}

pub(crate) fn create_agent_matrix_on_disk(
    args: CreateAgentMatrixDiskArgs<'_>,
) -> Result<CreatedAgentMatrixOnDisk, String> {
    let safe_name = sanitize_name(args.name)?;
    let project = Path::new(args.project_path);
    let base = selected_ac_root(project)?;

    let agent_dir = base.join(format!("_agent_{}", safe_name));
    if agent_dir.exists() {
        return Err(format!("Agent '{}' already exists", safe_name));
    }

    // Resolve the picked template before any target disk mutation so unknown or
    // unreadable template ids cannot leave a half-built matrix behind.
    let resolved_template: Option<crate::commands::role_templates::ResolvedRoleTemplate> =
        match args
            .role_template_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(id) => {
                let config_dir = args
                    .config_dir
                    .ok_or_else(|| "Could not determine config directory".to_string())?;
                Some(crate::commands::role_templates::resolve_role_template(
                    id,
                    args.settings,
                    config_dir,
                )?)
            }
            None => None,
        };

    create_new_agent_matrix_layout(&agent_dir).map_err(|(sub, e)| match (sub, e.kind()) {
        ("agent_dir", std::io::ErrorKind::AlreadyExists) => {
            format!("Agent '{}' already exists", safe_name)
        }
        ("agent_dir", _) => format!("Failed to create agent directory: {}", e),
        _ => format!("Failed to create {} directory: {}", sub, e),
    })?;

    let role_content = build_role_content(&safe_name, args.description, resolved_template.as_ref());
    let role_path = agent_dir.join("Role.md");
    std::fs::write(&role_path, &role_content)
        .map_err(|e| format!("Failed to write Role.md: {}", e))?;

    if let Some(ref t) = resolved_template {
        if let Some(ref src) = t.skills_src {
            let dst = agent_dir.join("skills");
            let failures = crate::commands::role_templates::copy_dir_recursive(src, &dst);
            if !failures.is_empty() {
                log::error!(
                    "[entity_creation] some files from template '{}' skills/ could not be \
                     copied into {} ({} failure(s)): {}",
                    t.id,
                    dst.display(),
                    failures.len(),
                    failures.join("; ")
                );
            }
        }
    }

    write_local_config_value(
        &agent_dir.join("config.json"),
        default_agent_matrix_config(),
    )?;

    let display_name = agent_matrix_display_name(project, &safe_name);
    Ok(CreatedAgentMatrixOnDisk {
        agent_dir,
        display_name,
        safe_name,
        role_path,
    })
}

pub(crate) struct CreateAgentMatrixFromRoleArgs<'a> {
    pub ac_root: &'a Path,
    pub safe_name: &'a str,
    pub role_bytes: &'a [u8],
}

pub(crate) fn create_agent_matrix_from_role(
    args: CreateAgentMatrixFromRoleArgs<'_>,
) -> Result<CreatedAgentMatrixOnDisk, String> {
    if sanitize_name(args.safe_name)? != args.safe_name {
        return Err(format!(
            "Agent '{}' must already be a lowercase slug",
            args.safe_name
        ));
    }
    validate_existing_name(args.safe_name, "Agent")?;
    let agent_dir = args.ac_root.join(format!("_agent_{}", args.safe_name));
    if agent_dir.exists() {
        return Err(format!("Agent '{}' already exists", args.safe_name));
    }
    create_new_agent_matrix_layout(&agent_dir).map_err(|(sub, e)| match (sub, e.kind()) {
        ("agent_dir", std::io::ErrorKind::AlreadyExists) => {
            format!("Agent '{}' already exists", args.safe_name)
        }
        ("agent_dir", _) => format!("Failed to create agent directory: {}", e),
        _ => format!("Failed to create {} directory: {}", sub, e),
    })?;

    let role_path = agent_dir.join("Role.md");
    std::fs::write(&role_path, args.role_bytes)
        .map_err(|e| format!("Failed to write Role.md: {}", e))?;

    write_local_config_value(
        &agent_dir.join("config.json"),
        default_agent_matrix_config(),
    )?;

    let project_path = args
        .ac_root
        .parent()
        .ok_or_else(|| "Project AC Root has no project parent".to_string())?;
    Ok(CreatedAgentMatrixOnDisk {
        agent_dir,
        display_name: agent_matrix_display_name(project_path, args.safe_name),
        safe_name: args.safe_name.to_string(),
        role_path,
    })
}

fn default_agent_matrix_config() -> serde_json::Value {
    serde_json::json!({
        "tooling": {},
        "context": ["$AGENTSCOMMANDER_CONTEXT", "Role.md"],
    })
}

fn write_local_config_value(config_path: &Path, value: serde_json::Value) -> Result<(), String> {
    crate::config::agent_config::update_agent_config(config_path, |obj, _state| {
        let map = value
            .as_object()
            .ok_or_else(|| "Local config value must be a JSON object".to_string())?;
        *obj = map.clone();
        Ok(())
    })?;
    Ok(())
}

fn selected_ac_root(project: &Path) -> Result<PathBuf, String> {
    existing_ac_root(project)
        .ok_or_else(|| format!(".ac directory not found in {}", project.display()))
}

// Strict Matrix copy and team publication share ephemeral output ownership.
const COPY_PROVENANCE_FILE: &str = ".ac-copy-provenance.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatrixCopyKind {
    Directory,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatrixCopyStamp {
    identity: Option<MatrixCopyIdentity>,
    kind: MatrixCopyKind,
    size: u64,
    modified: std::time::SystemTime,
    digest: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MatrixCopyIdentity(u64, u64);

#[derive(Debug)]
struct OwnedCopyEntry {
    relative: PathBuf,
    identity: MatrixCopyIdentity,
    kind: MatrixCopyKind,
    digest: Option<[u8; 32]>,
}

#[derive(Debug)]
struct OwnedCopyArtifacts {
    root: PathBuf,
    entries: Vec<OwnedCopyEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatrixCopyStage {
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
type MatrixCopyCallback<'a> = dyn FnMut(MatrixCopyStage, &Path) -> Result<(), String> + 'a;

#[derive(Default)]
struct MatrixCopyHooks<'a> {
    #[cfg(test)]
    callback: Option<&'a mut MatrixCopyCallback<'a>>,
    #[cfg(test)]
    classification: Option<(bool, u32, bool, bool)>,
    marker: std::marker::PhantomData<&'a ()>,
}

impl MatrixCopyHooks<'_> {
    fn run(&mut self, stage: MatrixCopyStage, path: &Path) -> Result<(), String> {
        #[cfg(test)]
        if let Some(callback) = &mut self.callback {
            return callback(stage, path);
        }
        let _ = (stage, path);
        Ok(())
    }
}

impl OwnedCopyArtifacts {
    fn handle_identity(file: &File) -> Result<MatrixCopyIdentity, String> {
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

    fn verify_directories(&self, relative: &Path) -> Result<(), String> {
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

    fn create_directory(&mut self, relative: &Path) -> Result<(), String> {
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

    fn create_file(&mut self, relative: &Path) -> Result<File, String> {
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

    fn digest_bytes(bytes: &[u8]) -> [u8; 32] {
        use sha2::Digest;
        sha2::Sha256::digest(bytes).into()
    }

    fn stamp(
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

    fn selected_root_entry(
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

    fn scan(
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

fn validate_copy_provenance(history: Option<&[u8]>) -> Result<serde_json::Value, String> {
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

fn append_copy_provenance(
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

fn cleanup_owned_copy_artifacts(owned: &OwnedCopyArtifacts) -> Result<(), String> {
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
fn copy_agent_matrix_strict(
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

fn copy_agent_matrix_strict_with_hooks(
    source_project: &Path,
    source_ref: &str,
    destination_ac: &Path,
    destination_ref: &str,
    hooks: &mut MatrixCopyHooks<'_>,
) -> Result<OwnedCopyArtifacts, String> {
    use std::io::Write;
    let project = source_project.canonicalize().map_err(|e| e.to_string())?;
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

const TEAM_CONFIG_MUTATION_LOCK_NAME: &str = ".team-config-write.lock";
const TEAM_CONFIG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const TEAM_CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
struct TeamConfigLockTiming {
    poll_interval: Duration,
    timeout: Duration,
}

impl TeamConfigLockTiming {
    const PRODUCTION: Self = Self {
        poll_interval: TEAM_CONFIG_LOCK_POLL_INTERVAL,
        timeout: TEAM_CONFIG_LOCK_TIMEOUT,
    };
}

#[derive(Debug)]
pub(crate) struct TeamConfigMutationGuard {
    _file: File,
    canonical_ac_root: PathBuf,
    lock_path: PathBuf,
}

impl TeamConfigMutationGuard {
    pub(crate) fn acquire(ac_root: &Path) -> Result<Self, String> {
        Self::acquire_with_timing(ac_root, TeamConfigLockTiming::PRODUCTION)
    }

    fn acquire_with_timing(ac_root: &Path, timing: TeamConfigLockTiming) -> Result<Self, String> {
        let requested_lock_path = ac_root.join(TEAM_CONFIG_MUTATION_LOCK_NAME);
        let canonical_ac_root = std::fs::canonicalize(ac_root)
            .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
            .map_err(|e| {
                format!(
                    "Failed to resolve workspace for team config lock '{}': {}",
                    requested_lock_path.display(),
                    e
                )
            })?;
        if !canonical_ac_root.is_dir() {
            return Err(format!(
                "Failed to open team config lock '{}': workspace is not a directory",
                requested_lock_path.display()
            ));
        }
        let lock_path = canonical_ac_root.join(TEAM_CONFIG_MUTATION_LOCK_NAME);

        let open_existing = || -> Result<File, String> {
            validate_team_config_lock_sentinel(&lock_path, &canonical_ac_root)?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(|open_error| {
                    format!(
                        "Failed to open existing team config lock '{}': {}",
                        lock_path.display(),
                        open_error
                    )
                })
        };
        let file = match std::fs::symlink_metadata(&lock_path) {
            Ok(_) => open_existing()?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&lock_path)
                {
                    Ok(file) => file,
                    Err(create_error)
                        if create_error.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        open_existing()?
                    }
                    Err(create_error) => {
                        return Err(format!(
                            "Failed to create team config lock '{}': {}",
                            lock_path.display(),
                            create_error
                        ));
                    }
                }
            }
            Err(e) => {
                return Err(format!(
                    "Failed to inspect team config lock '{}': {}",
                    lock_path.display(),
                    e
                ));
            }
        };
        validate_team_config_lock_sentinel(&lock_path, &canonical_ac_root)?;
        if !file
            .metadata()
            .map_err(|e| {
                format!(
                    "Failed to inspect opened team config lock '{}': {}",
                    lock_path.display(),
                    e
                )
            })?
            .is_file()
        {
            return Err(format!(
                "Team config lock '{}' must be a regular file",
                lock_path.display()
            ));
        }

        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if started.elapsed() >= timing.timeout => {
                    return Err(format!(
                        "Timed out after {} ms waiting for team config lock '{}'",
                        timing.timeout.as_millis(),
                        lock_path.display()
                    ));
                }
                Err(TryLockError::WouldBlock) => {
                    let remaining = timing.timeout.saturating_sub(started.elapsed());
                    let delay = timing.poll_interval.min(remaining);
                    if delay.is_zero() {
                        std::thread::yield_now();
                    } else {
                        std::thread::sleep(delay);
                    }
                }
                Err(TryLockError::Error(e)) => {
                    return Err(format!(
                        "Failed to acquire team config lock '{}': {}",
                        lock_path.display(),
                        e
                    ));
                }
            }
        }

        Ok(Self {
            _file: file,
            canonical_ac_root,
            lock_path,
        })
    }

    fn require_ac_root(&self, ac_root: &Path) -> Result<(), String> {
        let canonical = std::fs::canonicalize(ac_root)
            .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
            .map_err(|e| {
                format!(
                    "Failed to verify workspace for team config lock '{}': {}",
                    self.lock_path.display(),
                    e
                )
            })?;
        if canonical != self.canonical_ac_root {
            return Err(format!(
                "Team config lock '{}' belongs to workspace '{}', not '{}'",
                self.lock_path.display(),
                self.canonical_ac_root.display(),
                canonical.display()
            ));
        }
        Ok(())
    }
}

fn validate_team_config_lock_sentinel(
    lock_path: &Path,
    canonical_ac_root: &Path,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(lock_path).map_err(|e| {
        format!(
            "Failed to inspect team config lock '{}': {}",
            lock_path.display(),
            e
        )
    })?;
    if metadata_is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "Team config lock '{}' must be a regular non-link file",
            lock_path.display()
        ));
    }
    let canonical_lock = std::fs::canonicalize(lock_path)
        .map(|path| crate::path_utils::normalize_windows_verbatim_path_buf(&path))
        .map_err(|e| {
            format!(
                "Failed to resolve team config lock '{}': {}",
                lock_path.display(),
                e
            )
        })?;
    let canonical_parent = canonical_lock.parent().ok_or_else(|| {
        format!(
            "Team config lock '{}' has no canonical parent",
            lock_path.display()
        )
    })?;
    if canonical_parent != canonical_ac_root {
        return Err(format!(
            "Team config lock '{}' escapes canonical workspace '{}'",
            lock_path.display(),
            canonical_ac_root.display()
        ));
    }
    Ok(())
}

async fn acquire_team_config_mutation_guard_async(
    ac_root: &Path,
) -> Result<TeamConfigMutationGuard, String> {
    let ac_root = ac_root.to_path_buf();
    let ac_root_label = ac_root.display().to_string();
    tokio::task::spawn_blocking(move || TeamConfigMutationGuard::acquire(&ac_root))
        .await
        .map_err(|e| {
            format!(
                "Failed to acquire team config lock for workspace '{}': blocking task failed: {}",
                ac_root_label, e
            )
        })?
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum TeamConfigReadError {
    #[error("Team config not found at '{path}': {source}")]
    NotFound {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Team config at '{path}' is unreadable: {source}")]
    Unreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Team config at '{path}' is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("Team config at '{path}' is invalid: {cause}")]
    Invalid { path: PathBuf, cause: String },
}

impl TeamConfigReadError {
    // Consumed by the Phase 2 runtime; Phase 1 pins the stable classification contract.
    pub(crate) fn class(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "not_found",
            Self::Unreadable { .. } => "unreadable",
            Self::Malformed { .. } => "malformed",
            Self::Invalid { .. } => "invalid",
        }
    }
}

pub(crate) fn read_team_config_classified(
    ac_root: &Path,
    team_name: &str,
) -> Result<TeamConfigResult, TeamConfigReadError> {
    let config_path = ac_root
        .join(format!("_team_{}", team_name))
        .join("config.json");
    validate_existing_name(team_name, "Team").map_err(|cause| TeamConfigReadError::Invalid {
        path: config_path.clone(),
        cause,
    })?;
    let content = std::fs::read(&config_path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            TeamConfigReadError::NotFound {
                path: config_path.clone(),
                source,
            }
        } else {
            TeamConfigReadError::Unreadable {
                path: config_path.clone(),
                source,
            }
        }
    })?;
    let config: TeamConfigResult =
        serde_json::from_slice(&content).map_err(|source| TeamConfigReadError::Malformed {
            path: config_path.clone(),
            source,
        })?;
    normalize_team_config_for_project(ac_root, &config).map_err(|cause| {
        TeamConfigReadError::Invalid {
            path: config_path,
            cause,
        }
    })
}

pub(crate) fn read_team_config(
    ac_root: &Path,
    team_name: &str,
) -> Result<TeamConfigResult, String> {
    read_team_config_classified(ac_root, team_name).map_err(|e| e.to_string())
}

fn normalized_team_config_bytes(
    ac_root: &Path,
    config: &TeamConfigResult,
) -> Result<Vec<u8>, String> {
    let normalized = normalize_team_config_for_project(ac_root, config)?;
    serde_json::to_vec_pretty(&normalized)
        .map_err(|e| format!("Failed to serialize config.json: {}", e))
}

pub(crate) fn write_team_config_guarded(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
    guard: &TeamConfigMutationGuard,
) -> Result<PathBuf, String> {
    write_team_config_guarded_with_publisher(
        ac_root,
        team_name,
        config,
        guard,
        crate::config::local_config_io::write_file_atomic,
    )
}

fn write_existing_team_config_guarded(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
    guard: &TeamConfigMutationGuard,
) -> Result<PathBuf, String> {
    guard.require_ac_root(ac_root)?;
    validate_existing_name(team_name, "Team")?;
    let team_dir = ac_root.join(format!("_team_{}", team_name));
    if !team_dir.exists() {
        return Err(format!("Team '{}' not found", team_name));
    }
    write_team_config_guarded(ac_root, team_name, config, guard)
}

fn write_team_config_guarded_with_publisher<P>(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
    guard: &TeamConfigMutationGuard,
    publish: P,
) -> Result<PathBuf, String>
where
    P: FnOnce(&Path, &[u8]) -> Result<(), String>,
{
    guard.require_ac_root(ac_root)?;
    validate_existing_name(team_name, "Team")?;
    let config_bytes = normalized_team_config_bytes(ac_root, config)?;
    let team_dir = ac_root.join(format!("_team_{}", team_name));
    std::fs::create_dir_all(&team_dir)
        .map_err(|e| format!("Failed to create team directory: {}", e))?;
    std::fs::create_dir_all(team_dir.join("memory"))
        .map_err(|e| format!("Failed to create memory directory: {}", e))?;
    let conventions = team_dir.join("conventions.md");
    if !conventions.exists() {
        std::fs::write(&conventions, "")
            .map_err(|e| format!("Failed to write conventions.md: {}", e))?;
    }
    publish(&team_dir.join("config.json"), &config_bytes)
        .map_err(|e| format!("Failed to publish config.json: {}", e))?;
    Ok(team_dir)
}

pub(crate) fn create_new_team_config_on_disk(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
) -> Result<PathBuf, String> {
    let guard = TeamConfigMutationGuard::acquire(ac_root)?;
    create_new_team_config_on_disk_guarded(ac_root, team_name, config, &guard)
}

pub(crate) fn create_new_team_config_on_disk_guarded(
    ac_root: &Path,
    team_name: &str,
    config: &TeamConfigResult,
    guard: &TeamConfigMutationGuard,
) -> Result<PathBuf, String> {
    guard.require_ac_root(ac_root)?;
    validate_existing_name(team_name, "Team")?;
    let config_bytes = normalized_team_config_bytes(ac_root, config)?;
    let team_dir = ac_root.join(format!("_team_{}", team_name));
    std::fs::create_dir(&team_dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("Team '{}' already exists", team_name),
        _ => format!("Failed to create team directory: {}", e),
    })?;
    std::fs::create_dir(team_dir.join("memory"))
        .map_err(|e| format!("Failed to create memory directory: {}", e))?;
    std::fs::write(team_dir.join("conventions.md"), "")
        .map_err(|e| format!("Failed to write conventions.md: {}", e))?;
    crate::config::local_config_io::write_file_atomic(&team_dir.join("config.json"), &config_bytes)
        .map_err(|e| format!("Failed to publish config.json: {}", e))?;
    Ok(team_dir)
}

pub(crate) fn normalize_context_alert_percentages(values: &[u8]) -> Result<Vec<u8>, String> {
    if values.len() > 3 {
        return Err("contextAlertPercentages must contain at most 3 values".to_string());
    }
    if values.iter().any(|value| !(1..=100).contains(value)) {
        return Err(
            "contextAlertPercentages values must be integer percentages from 1 through 100"
                .to_string(),
        );
    }
    let mut normalized = values.to_vec();
    normalized.sort_unstable();
    if normalized.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("contextAlertPercentages values must be distinct".to_string());
    }
    Ok(normalized)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextAlertWriteMode {
    Create,
    Update,
}

fn resolve_context_alert_argument(
    mode: ContextAlertWriteMode,
    supplied: Option<Vec<u8>>,
    current: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    match supplied {
        Some(values) => normalize_context_alert_percentages(&values),
        None if mode == ContextAlertWriteMode::Create => Ok(Vec::new()),
        None => current.map(|values| values.to_vec()).ok_or_else(|| {
            "Current team config is required when contextAlertPercentages is omitted on update"
                .to_string()
        }),
    }
}

pub(crate) fn normalize_team_config_for_project(
    ac_root: &Path,
    config: &TeamConfigResult,
) -> Result<TeamConfigResult, String> {
    let agents = normalize_team_agent_refs(ac_root, &config.agents)?;
    let coordinator = if config.coordinator.trim().is_empty() {
        String::new()
    } else {
        resolve_agent_ref(ac_root, &config.coordinator)?
    };
    if !coordinator.is_empty() && !agents.contains(&coordinator) {
        return Err("Orchestrator must be one of the selected agents".to_string());
    }
    let mut repos = Vec::with_capacity(config.repos.len());
    for repo in &config.repos {
        repos.push(RepoAssignment {
            url: repo.url.clone(),
            agents: normalize_team_agent_refs(ac_root, &repo.agents)?,
        });
    }
    let context_alert_percentages =
        normalize_context_alert_percentages(&config.context_alert_percentages)?;
    Ok(TeamConfigResult {
        agents,
        coordinator,
        repos,
        context_alert_percentages,
    })
}

fn normalize_team_agent_refs(ac_root: &Path, refs: &[String]) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for agent in refs {
        let resolved = resolve_agent_ref(ac_root, agent)?;
        if !normalized.contains(&resolved) {
            normalized.push(resolved);
        }
    }
    Ok(normalized)
}

pub(crate) fn list_workgroup_dirs(ac_root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(ac_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if parse_team_from_workgroup_name(name).is_ok() {
                dirs.push(path);
            }
        }
    }
    dirs.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .cmp(b.file_name().unwrap_or_default())
    });
    dirs
}

pub(crate) fn parse_team_from_workgroup_name(workgroup_name: &str) -> Result<String, String> {
    let rest = crate::config::entity_prefix::strip_entity_prefix(workgroup_name)
        .ok_or_else(|| format!("Invalid room name '{}'", workgroup_name))?;
    let Some((number, team)) = rest.split_once('-') else {
        return Err(format!("Invalid room name '{}'", workgroup_name));
    };
    let parsed = number
        .parse::<u32>()
        .map_err(|_| format!("Invalid room number in '{}'", workgroup_name))?;
    if parsed == 0 || team.is_empty() {
        return Err(format!("Invalid room name '{}'", workgroup_name));
    }
    validate_existing_name(team, "Team")?;
    Ok(team.to_string())
}

pub(crate) fn resolve_agent_ref(ac_root: &Path, raw_agent: &str) -> Result<String, String> {
    let trimmed = raw_agent.trim();
    if trimmed.is_empty() {
        return Err("Agent reference cannot be empty".to_string());
    }
    if trimmed.contains('\0') {
        return Err("Agent reference must not contain NUL".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains(':') {
        return Err(format!(
            "Invalid agent reference '{}': team configs must use portable refs like '_agent_name' \
             or 'name', not filesystem paths",
            raw_agent
        ));
    }
    if Path::new(trimmed).is_absolute() {
        return Err(format!(
            "Invalid agent reference '{}': absolute paths are not allowed in team configs",
            raw_agent
        ));
    }
    let bare = trimmed.strip_prefix("_agent_").unwrap_or(trimmed);
    validate_existing_name(bare, "Agent")?;
    let canonical_ref = format!("_agent_{}", bare);
    let matrix_dir = ac_root.join(&canonical_ref);
    if !matrix_dir.is_dir() {
        return Err(format!("Agent '{}' not found", bare));
    }
    Ok(canonical_ref)
}

pub(crate) fn agent_ref_bare_name(agent_ref: &str) -> String {
    let dir_name = Path::new(agent_ref)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(agent_ref);
    dir_name
        .strip_prefix("_agent_")
        .unwrap_or(dir_name)
        .to_string()
}

pub(crate) async fn create_workgroup_on_disk(
    args: WorkgroupDiskCreateArgs,
) -> Result<WorkgroupCloneResult, String> {
    let safe_team = sanitize_name(&args.team_name)?;
    let task_title = args.task_title.trim().to_string();
    validate_task_title(&task_title)?;
    let base = selected_ac_root(&args.project_path)?;

    if let Err(e) = crate::commands::ac_discovery::ensure_ac_root_gitignore(&base) {
        log::warn!(
            "[create_workgroup] Failed to ensure Project AC Root .gitignore: {}",
            e
        );
    }

    let team_config =
        if !args.agents.is_empty() || args.coordinator.is_some() || !args.repos.is_empty() {
            let coordinator = args.coordinator.clone().ok_or_else(|| {
                "Orchestrator is required when provisioning team config".to_string()
            })?;
            if !args.agents.contains(&coordinator) {
                return Err("Orchestrator must be one of the selected agents".to_string());
            }
            let config = TeamConfigResult {
                agents: args.agents.clone(),
                coordinator,
                repos: args.repos.clone(),
                context_alert_percentages: Vec::new(),
            };
            let config = normalize_team_config_for_project(&base, &config)?;
            let guard = acquire_team_config_mutation_guard_async(&base).await?;
            create_new_team_config_on_disk_guarded(&base, &safe_team, &config, &guard)?;
            drop(guard);
            config
        } else {
            read_team_config(&base, &safe_team)?
        };

    let wg_number = determine_next_wg_number(&base);
    let wg_name = room_dir_name(wg_number, &safe_team, room_number_pad_width());
    let wg_dir = base.join(&wg_name);
    if wg_dir.exists() {
        return Err(format!("Room directory already exists: {}", wg_name));
    }
    std::fs::create_dir_all(&wg_dir)
        .map_err(|e| format!("Failed to create room directory: {}", e))?;
    std::fs::create_dir_all(wg_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME))
        .map_err(|e| format!("Failed to create messaging directory: {}", e))?;
    // #1795: `room-shared/` is a sibling of `messaging/` and `TASK.md`, and it is
    // created with the same hard `?` they use. A half-created room must not be
    // reported as created.
    crate::config::shared_locations::create_room_shared_dir(&wg_dir)
        .map_err(|e| format!("Failed to create room shared directory: {}", e))?;
    std::fs::write(wg_dir.join("TASK.md"), build_task_content(&task_title))
        .map_err(|e| format!("Failed to write TASK.md: {}", e))?;

    for agent_path in &team_config.agents {
        create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
            ac_root: base.clone(),
            wg_dir: wg_dir.clone(),
            agent_path: agent_path.clone(),
            team_repos: team_config.repos.clone(),
        })?;
    }

    let clone_errors = clone_missing_repos_for_workgroup(&wg_dir, &team_config.repos).await;
    let result_path = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&wg_dir);
    log::info!(
        "[entity_creation] Created workgroup: {} ({} clone errors)",
        result_path,
        clone_errors.len()
    );
    Ok(WorkgroupCloneResult {
        path: result_path,
        clone_errors,
    })
}

pub(crate) fn create_or_update_replica_on_disk(
    args: ReplicaDiskCreateArgs,
) -> Result<PathBuf, String> {
    let agent_ref = resolve_agent_ref(&args.ac_root, &args.agent_path)?;
    let agent_name = agent_ref_bare_name(&agent_ref);
    validate_existing_name(&agent_name, "Agent")?;
    let replica_dir = args.wg_dir.join(format!("__agent_{}", agent_name));

    create_agent_replica_layout(&replica_dir).map_err(|(sub, e)| match sub {
        "replica_dir" => format!("Failed to create replica dir for {}: {}", agent_name, e),
        _ => format!("Failed to create {} for {}: {}", sub, agent_name, e),
    })?;

    let assigned_repos: Vec<String> = args
        .team_repos
        .iter()
        .filter(|r| r.agents.iter().any(|a| agent_matches(a, &agent_name)))
        .map(|r| {
            let dir_name = format!("repo-{}", repo_dir_name_from_url(&r.url));
            format!("../{}", dir_name)
        })
        .collect();

    initialize_replica_config_on_disk(&replica_dir, &assigned_repos)?;
    Ok(replica_dir)
}

/// #1939 - guarded, lossless initialization of one Room replica config.
///
/// Shared by `create_or_update_replica_on_disk` and the GUI workgroup loop so
/// both publish through the #1938 sidecar with one merge: repair identity and
/// context, refresh `repos`, and preserve tooling, custom context and unknown
/// keys. A missing config file inside the guard is the first-creation boundary
/// (`try_exists`, so an inspection error is an error, never absence); only then
/// is the Matrix creation default read and materialized. An existing config
/// never reads the default, so a later invalid default cannot block a valid
/// retry, and a concurrent winner's bytes are merged, not overwritten.
fn initialize_replica_config_on_disk(
    replica_dir: &Path,
    assigned_repos: &[String],
) -> Result<(), String> {
    let config_path = replica_dir.join("config.json");
    crate::config::agent_config::update_agent_config(&config_path, |obj, _state| {
        let had_config = config_path.try_exists().map_err(|error| {
            format!(
                "Failed to inspect replica config {}: {}",
                config_path.display(),
                error
            )
        })?;
        let mut config = serde_json::Value::Object(std::mem::take(obj));
        if !had_config {
            // First creation: establish the canonical identity the repair below
            // then validates, exactly like any other identity.
            let expected =
                crate::config::replica_identity::expected_wg_replica_identity(replica_dir)?;
            config["identity"] = serde_json::json!(expected.identity);
        }
        let identity =
            repair_wg_replica_config_value(replica_dir, &mut config, WG_REPLICA_REQUIRED_CONTEXT)?;
        config["repos"] = serde_json::json!(assigned_repos);
        let existing_context: Vec<String> = config
            .get("context")
            .and_then(|value| value.as_array())
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        config["context"] = serde_json::json!(normalize_wg_replica_context_entries(
            &existing_context,
            &["$AGENTSCOMMANDER_CONTEXT"],
            &identity.identity,
            identity.matrix_dir.join(ROLE_MD_FILENAME).exists(),
        ));
        if !had_config {
            if let Some(default) = read_replica_selection_default(&identity.matrix_dir)? {
                apply_replica_selection_default(&mut config, &default)?;
            }
        }
        let final_object = config.as_object_mut().ok_or_else(|| {
            format!(
                "Replica config {} must be a JSON object",
                config_path.display()
            )
        })?;
        *obj = std::mem::take(final_object);
        Ok(())
    })?;
    Ok(())
}

/// #1939 - the GUI workgroup replica loop. Kept as one function so the GUI path
/// and the on-disk path provably share the same guarded initializer instead of
/// a second destructive writer.
fn initialize_workgroup_replicas_on_disk(
    ac_root: &Path,
    wg_dir: &Path,
    agents: &[String],
    repos: &[RepoAssignment],
) -> Result<(), String> {
    for agent_path in agents {
        let agent_ref = resolve_agent_ref(ac_root, agent_path)?;
        let agent_name = agent_ref_bare_name(&agent_ref);
        let replica_dir = wg_dir.join(format!("__agent_{}", agent_name));
        create_agent_replica_layout(&replica_dir).map_err(|(sub, error)| match sub {
            "replica_dir" => format!("Failed to create replica dir for {}: {}", agent_name, error),
            _ => format!("Failed to create {} for {}: {}", sub, agent_name, error),
        })?;
        let assigned_repos: Vec<String> = repos
            .iter()
            .filter(|repo| repo.agents.iter().any(|a| agent_matches(a, &agent_name)))
            .map(|repo| {
                let dir_name = format!("repo-{}", repo_dir_name_from_url(&repo.url));
                format!("../{}", dir_name)
            })
            .collect();
        initialize_replica_config_on_disk(&replica_dir, &assigned_repos)?;
    }
    Ok(())
}

/// Typed outcome of removing one replica directory (#1063, plan section 5.4).
///
/// Replaces the prior bare `Ok(())` so lifecycle callers can distinguish a real
/// removal from a proven absence and prune manifest rows only for `Removed` or an
/// explicit `AlreadyAbsent`. `AlreadyAbsent` is constructed only from an exact
/// `symlink_metadata`/remove `ErrorKind::NotFound`; it never uses `Path::exists`,
/// which collapses access and other inspection errors to false. A plain
/// discovery `NotFound` observed elsewhere never reaches this typed path and
/// therefore never prunes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplicaRemovalOutcome {
    Removed,
    AlreadyAbsent,
    Failed(String),
}

pub(crate) fn remove_replica_dir(replica_dir: &Path) -> ReplicaRemovalOutcome {
    match std::fs::symlink_metadata(replica_dir) {
        Ok(_) => match std::fs::remove_dir_all(replica_dir) {
            Ok(()) => ReplicaRemovalOutcome::Removed,
            // A concurrent removal between the no-follow probe and the delete is
            // proven absent, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                ReplicaRemovalOutcome::AlreadyAbsent
            }
            Err(e) => {
                ReplicaRemovalOutcome::Failed(format!("Failed to delete replica directory: {}", e))
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ReplicaRemovalOutcome::AlreadyAbsent,
        Err(e) => {
            ReplicaRemovalOutcome::Failed(format!("Failed to inspect replica directory: {}", e))
        }
    }
}

// ===========================================================================
// #1063 lifecycle-removal seam (#1065 Stage F: activated recorder).
//
// Lifecycle deletion acquires the project seed-manifest gate so it serializes
// against config-seed publication (plan sections 5.4/6.3): the lock ordering is
// production-active. The manifest prune requires a `ManifestActivationToken`;
// Stage F adds the sole production constructor (`ManifestActivationToken::production`)
// and each production deletion entry point now threads `Some(&token)`, so the
// recorder call is compiled and reached in a production build and the manifest is
// pruned after the logical removal commits. Emission still stays a no-op when the
// gate degraded before contention (`None` gate) or a caller intentionally passes
// no token (`None`), preserving the fail-soft deletion contract. Tests thread a
// token to exercise the real prune, staged-delete rollback, and single final
// snapshot on the same lifecycle path.
// ===========================================================================

/// Acquire the project seed-manifest gate for a still-reversible lifecycle
/// removal.
///
/// * `Ok(Some(guard))` holds the gate through logical removal, rollback, and the
///   (dormant) prune.
/// * `Ok(None)` is a pre-contention lock-capability/I-O degradation: preserve
///   today's untracked deletion behavior and warn (plan section 6.1 lifecycle row).
/// * `Err(msg)` is a retryable busy timeout after observed contention, or an
///   unsafe/reparse project path that must fail closed before mutation.
pub(crate) fn acquire_lifecycle_project_gate(
    project_root: &Path,
) -> Result<Option<ProjectSeedManifestGuard>, String> {
    match ProjectSeedManifestGuard::acquire(project_root) {
        Ok(guard) => Ok(Some(guard)),
        Err(SeedManifestError::UnsafePath { path, reason }) => Err(format!(
            "Refusing deletion: unsafe project seed-manifest path {}: {}",
            path.display(),
            reason
        )),
        Err(SeedManifestError::BusyTimeout { path, waited }) => Err(format!(
            "Project is busy; retry the deletion (seed-manifest lock at {} stayed busy for {:?})",
            path.display(),
            waited
        )),
        Err(SeedManifestError::LockIo {
            saw_contention: true,
            path,
            source,
        }) => Err(format!(
            "Project is busy; retry the deletion (seed-manifest lock I/O at {} after contention: \
             {})",
            path.display(),
            source
        )),
        Err(other) => {
            // Pre-contention capability or other lock-acquisition error: preserve
            // the existing deletion behavior as untracked degradation.
            log::warn!(
                "[entity_creation] #1063 lifecycle removal proceeding without seed-manifest gate \
                 (untracked): {}",
                other
            );
            Ok(None)
        }
    }
}

/// Lifecycle prune (#1063, #1065 Stage F activated). The recorder call is compiled
/// in production; it runs only when a gate is held and an activation token was
/// threaded, so a pre-contention gate degradation or an unactivated caller stays a
/// no-op.
///
/// The `gate` is `None` when the pre-contention degradation above skipped the
/// lock. The `make_filter` closure builds the exact committed-intent
/// `ManifestLifecycleFilter` lazily so no filter is constructed on the no-op path.
fn prune_lifecycle_scope_if_activated(
    gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    make_filter: impl FnOnce() -> Result<ManifestLifecycleFilter, SeedManifestError>,
) {
    if let (Some(guard), Some(token)) = (gate, activation) {
        match make_filter() {
            Ok(filter) => {
                let outcome = guard
                    .publication_permit()
                    .apply_lifecycle_filter(token, filter);
                log::debug!(
                    "[entity_creation] #1063 lifecycle prune outcome: {:?}",
                    outcome
                );
            }
            Err(error) => log::warn!(
                "[entity_creation] #1063 lifecycle filter rejected before prune: {}",
                error
            ),
        }
    }
}

/// Activation-gated prune of the config scope of a single replica `.ac/<workgroup>/__agent_<name>`
/// (#1063, plan section 5.4 "AC removes one replica"). Used by CLI team-member removal.
pub(crate) fn prune_replica_config_scope(
    gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    workgroup: &str,
    agent_name: &str,
) {
    let relative = Path::new(".ac")
        .join(workgroup)
        .join(format!("__agent_{}", agent_name));
    prune_lifecycle_scope_if_activated(gate, activation, move || {
        let identity =
            crate::config::seed_manifest::ManifestPathIdentity::from_relative_path(&relative)?;
        ManifestLifecycleFilter::config_path_prefix(identity)
    });
}

/// Activation-gated prune of every replica config scope under one workgroup `.ac/<workgroup>`
/// (#1063, plan section 5.4 "AC removes a workgroup"). Used by GUI/CLI workgroup deletion.
pub(crate) fn prune_workgroup_config_scope(
    gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    workgroup: &str,
) {
    let relative = Path::new(".ac").join(workgroup);
    prune_lifecycle_scope_if_activated(gate, activation, move || {
        let identity =
            crate::config::seed_manifest::ManifestPathIdentity::from_relative_path(&relative)?;
        ManifestLifecycleFilter::config_path_prefix(identity)
    });
}

/// Activation-gated prune of every replica config scope for a set of workgroups (#1063, plan
/// section 5.4 "AC deletes a team"). Used by GUI team deletion for the committed
/// intent: successfully removed workgroups plus valid matching team workgroup
/// identities that were already absent at the explicit delete event.
pub(crate) fn prune_team_workgroups_scope(
    gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    workgroups: Vec<String>,
) {
    if workgroups.is_empty() {
        return;
    }
    prune_lifecycle_scope_if_activated(gate, activation, move || {
        ManifestLifecycleFilter::workgroup_components(workgroups)
    });
}

/// Activation-gated prune of every replica config scope for one exact agent component
/// `__agent_<name>` across all workgroups (#1063, plan section 5.4 "AC deletes an
/// Agent Matrix and cascaded replicas"). Used by Agent Matrix deletion.
pub(crate) fn prune_agent_component_scope(
    gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    agent_name: &str,
) {
    let component = format!("__agent_{}", agent_name);
    prune_lifecycle_scope_if_activated(gate, activation, move || {
        ManifestLifecycleFilter::replica_component(component)
    });
}

pub(crate) async fn clone_missing_repos_for_workgroup(
    wg_dir: &Path,
    repos: &[RepoAssignment],
) -> Vec<CloneError> {
    let mut clone_errors = Vec::new();
    let mut seen_urls = HashSet::new();
    for repo in repos {
        if !seen_urls.insert(repo.url.clone()) {
            continue;
        }
        let dir_name = format!("repo-{}", repo_dir_name_from_url(&repo.url));
        let target = wg_dir.join(&dir_name);
        if target.exists() {
            continue;
        }
        match git_clone_async(&repo.url, &target).await {
            Ok(_) => log::info!(
                "[entity_creation] Cloned {} -> {}",
                repo.url,
                target.display()
            ),
            Err(e) => {
                log::error!("[entity_creation] Failed to clone {}: {}", repo.url, e);
                clone_errors.push(CloneError {
                    url: repo.url.clone(),
                    error: e,
                });
            }
        }
    }
    clone_errors
}

fn validate_task_title(task_title: &str) -> Result<(), String> {
    if task_title.is_empty() {
        return Err("Task title cannot be empty".to_string());
    }
    if task_title.chars().any(|c| c.is_control() && c != '\t') {
        return Err("Task title must be a single line of printable characters \
             (control characters other than tab are not allowed)"
            .to_string());
    }
    if task_title.chars().count() > 256 {
        return Err("Task title is too long (max 256 characters)".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Create an agent matrix directory inside {project_path}/.ac/_agent_{name}/
///
/// `role_template_id` is the picker selection from `list_role_templates`; when
/// `Some`, the resolved template's body is inserted as a `## Role Profile`
/// section and (if the template ships one) its `skills/` are copied into the
/// new matrix. `None` (or a missing arg from older callers) preserves the
/// pre-#271 behavior exactly.
#[tauri::command]
pub async fn create_agent_matrix(
    settings: State<'_, SettingsState>,
    project_path: String,
    name: String,
    description: String,
    role_template_id: Option<String>,
) -> Result<CreatedEntityResult, String> {
    let settings_snapshot = settings.read().await.clone();
    let config_dir = crate::config::config_dir();

    let created = create_agent_matrix_on_disk(CreateAgentMatrixDiskArgs {
        project_path: &project_path,
        name: &name,
        description: &description,
        role_template_id: role_template_id.as_deref(),
        settings: &settings_snapshot,
        config_dir: config_dir.as_deref(),
    })?;

    let result_path =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(&created.agent_dir);
    log::debug!(
        "[entity_creation] Created agent matrix safe name: {}",
        created.safe_name
    );
    log::info!("[entity_creation] Created agent matrix: {}", result_path);
    Ok(CreatedEntityResult { path: result_path })
}

/// Delete an agent matrix identity from a project.
#[tauri::command]
pub async fn delete_agent_matrix(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    settings: State<'_, SettingsState>,
    project_path: String,
    agent_path: String,
) -> Result<(), String> {
    // #1065 Stage F: activated with the sole production token; no lock-order test barrier.
    delete_agent_matrix_inner(
        &app,
        session_mgr.inner(),
        settings.inner(),
        &project_path,
        &agent_path,
        Some(ManifestActivationToken::production()),
        |_| {},
    )
    .await
}

/// GUI Agent Matrix deletion with the #1063 global lock order (plan sections
/// 5.4/6.3/11.32/11.43).
///
/// Async preflight collects the delete plan under a short preliminary team guard
/// (copying exact raw config bytes and intent), then drops that guard; it runs the
/// public live-session preflight and prepares owned settings/team metadata. The
/// blocking transaction then acquires project → (deletion-only pending-inclusive
/// manager snapshot, both reads dropped) → team → settings, in that order, with no
/// synchronous guard across `.await`. It stages targets, re-reads each planned team
/// config under the reacquired guard requiring exact prepared bytes, journals only
/// successful writes for exact-once reverse restore, removes only the two live agent
/// keys from settings, and on the successful metadata commit point prunes the agent
/// component scope (dormant) before dropping both guards and removing the staged
/// targets. A pre-commit failure rolls staged targets back and prunes only the
/// still-staged (logically absent) targets. Blocker diagnostics run after every guard
/// is released.
pub(crate) async fn delete_agent_matrix_inner<H>(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    settings: &SettingsState,
    project_path: &str,
    agent_path: &str,
    activation: Option<ManifestActivationToken>,
    after_project_before_team: H,
) -> Result<(), String>
where
    H: FnOnce(&Path) + Send + 'static,
{
    let base = selected_ac_root(Path::new(project_path))?;

    // Preflight under a short preliminary team guard, dropped before any wait.
    let plan = {
        let prelim_guard = acquire_team_config_mutation_guard_async(&base).await?;
        let plan = collect_agent_delete_plan_guarded(&base, Path::new(agent_path), &prelim_guard)?;
        drop(prelim_guard);
        plan
    };
    // Public live-session preflight + target validation (guards released).
    preflight_agent_delete(&plan, session_mgr).await?;
    // Owned settings/team metadata snapshot; the async settings read guard is dropped.
    let metadata = prepare_agent_delete_metadata(&plan, settings).await?;

    // Blocking transaction: project -> manager snapshot -> team -> settings.
    let project_root = Path::new(project_path).to_path_buf();
    let base_for_block = base.clone();
    let plan_for_block = plan.clone();
    let metadata_for_block = metadata.clone();
    let settings_for_block = settings.clone();
    let session_mgr_for_block = session_mgr.clone();
    let agent_name = plan.agent_name.clone();
    let outcome = crate::config::seed_manifest::run_blocking_owned(move || {
        run_agent_delete_transaction(
            &base_for_block,
            &project_root,
            &plan_for_block,
            &metadata_for_block,
            &settings_for_block,
            &session_mgr_for_block,
            &agent_name,
            activation.as_ref(),
            after_project_before_team,
            crate::config::settings::save_settings,
        )
    })
    .await
    .map_err(|e| format!("Agent delete blocking preparation failed: {e}"))?;

    match outcome {
        AgentDeleteTransactionOutcome::Committed {
            settings_changed,
            cleanup,
        } => {
            // The agent was removed; a hidden cleanup-dir error still surfaces to keep
            // the existing success/error contract.
            cleanup?;
            log::info!(
                "[entity_creation] Deleted agent matrix identity '{}' at '{}' (targets={}, \
                 team_configs={})",
                plan.agent_ref,
                plan.origin_dir.display(),
                plan.targets.len(),
                metadata.team_mutations.len()
            );
            if settings_changed {
                let _ = app.emit("coding_agent_profiles_updated", serde_json::json!({}));
            }
            emit_coordinator_refresh(app, session_mgr).await;
            Ok(())
        }
        AgentDeleteTransactionOutcome::Blocked { target, raw_error } => {
            // Blocker diagnostics run after every synchronous guard is released.
            log::info!(
                "[entity_creation] delete_agent_matrix: file-in-use detected for '{}' on rename \
                 probe",
                target.label
            );
            let report = crate::commands::wg_delete_diagnostic::diagnose_delete_root_blockers(
                &target.original_path,
                &target.label,
                &raw_error,
                session_mgr,
            )
            .await;
            let json = serde_json::to_string(&report).map_err(|se| {
                format!(
                    "Failed to serialize blocker report: {}; original error: {}",
                    se, raw_error
                )
            })?;
            Err(format!("BLOCKERS:{}", json))
        }
        AgentDeleteTransactionOutcome::Aborted { error } => Err(error),
    }
}

/// Outcome of the Agent Matrix delete blocking transaction (#1063). `Send + 'static`
/// so it can cross the `run_blocking_owned` boundary; the async caller formats it
/// after every synchronous guard has been released.
enum AgentDeleteTransactionOutcome {
    /// Metadata persistence reached its irreversible commit point; the manifest
    /// prune already ran under the gate. `cleanup` is the hidden staged-target
    /// removal result (an error keeps the existing success/error contract).
    Committed {
        settings_changed: bool,
        cleanup: Result<(), String>,
    },
    /// A rename probe was blocked by an open handle; the async caller runs the
    /// blocker diagnostic on the still-intact tree after the gate is released.
    Blocked {
        target: AgentDeleteTarget,
        raw_error: String,
    },
    /// Any pre-commit failure (gate/stage/live-recheck/team/metadata). Staged
    /// targets were rolled back and only still-staged scopes were pruned already.
    Aborted { error: String },
}

/// The Agent Matrix delete blocking transaction (#1063). Runs the project →
/// pending-inclusive manager snapshot → team → settings sequence to completion in
/// blocking work; production drives it inside `run_blocking_owned`, and tests call
/// it directly with an injected `save_settings_fn` and lock-order barrier. The
/// `after_project_before_team` barrier fires while the project gate is held and
/// before the team guard is taken.
#[allow(clippy::too_many_arguments)]
fn run_agent_delete_transaction(
    base: &Path,
    project_root: &Path,
    plan: &AgentDeletePlan,
    metadata: &PreparedAgentDeleteMetadata,
    settings: &SettingsState,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    agent_name: &str,
    activation: Option<&ManifestActivationToken>,
    after_project_before_team: impl FnOnce(&Path),
    save_settings_fn: impl Fn(&AppSettings) -> Result<AppSettings, String>,
) -> AgentDeleteTransactionOutcome {
    let mut project_gate = match acquire_lifecycle_project_gate(project_root) {
        Ok(gate) => gate,
        Err(error) => return AgentDeleteTransactionOutcome::Aborted { error },
    };
    after_project_before_team(base);

    // Stage targets; `stage_agent_delete_targets_with_rename` rolls back its own
    // staged set on failure, so no targets remain staged on Err.
    let staged = match stage_agent_delete_targets_with_rename(
        plan,
        |from: &Path, to: &Path| std::fs::rename(from, to),
        session_mgr,
    ) {
        Ok(staged) => staged,
        Err(AgentDeleteStageError::Blocked { target, raw_error }) => {
            drop(project_gate);
            return AgentDeleteTransactionOutcome::Blocked { target, raw_error };
        }
        Err(AgentDeleteStageError::Other(error))
        | Err(AgentDeleteStageError::RollbackFailed(error)) => {
            drop(project_gate);
            return AgentDeleteTransactionOutcome::Aborted { error };
        }
    };

    // Deletion-only pending-inclusive snapshot: outer manager read then one inner
    // state read, both released before the team guard.
    let live_workdirs = {
        let manager = session_mgr.blocking_read();
        manager.live_working_directories_for_deletion_blocking()
    };
    if live_workdirs
        .iter()
        .any(|workdir| workdir_blocks_delete(workdir, &plan.target_keys))
    {
        let still = rollback_staged_targets_reporting(&staged);
        prune_still_staged_targets(project_gate.as_mut(), activation, &still);
        drop(project_gate);
        return AgentDeleteTransactionOutcome::Aborted {
            error: format_post_stage_live_error(&still),
        };
    }

    // Reacquire the #1056 team guard after the project gate and manager reads.
    let team_guard = match TeamConfigMutationGuard::acquire(base) {
        Ok(guard) => guard,
        Err(error) => {
            let still = rollback_staged_targets_reporting(&staged);
            prune_still_staged_targets(project_gate.as_mut(), activation, &still);
            drop(project_gate);
            return AgentDeleteTransactionOutcome::Aborted {
                error: format!("{}{}", error, still_staged_suffix(&still)),
            };
        }
    };

    match persist_agent_delete_metadata_blocking(metadata, settings, &team_guard, save_settings_fn)
    {
        Ok(()) => {
            // Irreversible commit point reached. Prune the agent component scope
            // under the gate, then release both guards and delete.
            prune_agent_component_scope(project_gate.as_mut(), activation, agent_name);
            drop(team_guard);
            drop(project_gate);
            let cleanup = remove_staged_agent_delete_targets(&staged);
            AgentDeleteTransactionOutcome::Committed {
                settings_changed: metadata.settings_changed.load(Ordering::SeqCst),
                cleanup,
            }
        }
        Err(persist_error) => {
            // Metadata restore already ran inside the persist helper. Roll back
            // staged targets and prune only the still-staged ones.
            drop(team_guard);
            let still = rollback_staged_targets_reporting(&staged);
            prune_still_staged_targets(project_gate.as_mut(), activation, &still);
            drop(project_gate);
            AgentDeleteTransactionOutcome::Aborted {
                error: format!("{}{}", persist_error, still_staged_suffix(&still)),
            }
        }
    }
}

/// Whether a session working directory sits at or under any delete target key.
fn workdir_blocks_delete(workdir: &str, target_keys: &[String]) -> bool {
    let working_key = path_key_for_delete(Path::new(workdir));
    target_keys.iter().any(|root_key| {
        working_key == *root_key || working_key.starts_with(&(root_key.clone() + "/"))
    })
}

/// Generic retryable post-stage delete error (#1063). It never reveals pending
/// session ids, names, statuses, or working directories.
fn format_post_stage_live_error(still_staged: &[StagedAgentDeleteTarget]) -> String {
    format!(
        "Cannot delete agent while a session is active under a delete target; retry after it \
         exits.{}",
        still_staged_suffix(still_staged)
    )
}

/// Suffix describing hidden cleanup dirs that a failed rollback left staged away.
fn still_staged_suffix(still_staged: &[StagedAgentDeleteTarget]) -> String {
    if still_staged.is_empty() {
        return String::new();
    }
    let labels = still_staged
        .iter()
        .map(|target| format!("{} [{}]", target.label, target.original_key))
        .collect::<Vec<_>>()
        .join(", ");
    format!(" Hidden cleanup dir(s) remain: {}", labels)
}

/// Roll back staged agent-delete targets in reverse order and return the targets
/// that could NOT be restored (their `.deleting-*` rename-back failed, so the
/// logical path is absent). The caller prunes only these still-staged scopes; a
/// fully restored rollback returns an empty vec and prunes nothing (plan 5.4).
fn rollback_staged_targets_reporting(
    staged: &[StagedAgentDeleteTarget],
) -> Vec<StagedAgentDeleteTarget> {
    let mut still_staged = Vec::new();
    for target in staged.iter().rev() {
        if let Err(e) = std::fs::rename(&target.staged_path, &target.original_path) {
            log::warn!(
                "[entity_creation] agent-delete rollback left '{}' [{}] staged: {}",
                target.label,
                target.original_key,
                e
            );
            still_staged.push(target.clone());
        }
    }
    still_staged
}

/// Extract `(workgroup, agent)` for a replica delete target so its config scope
/// can be pruned. Returns `None` for the origin `_agent_*` target, which has no
/// v1 manifest row.
fn replica_prefix_for_target(original_path: &Path) -> Option<(String, String)> {
    let agent = original_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("__agent_"))?;
    let workgroup = original_path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())?;
    Some((workgroup.to_string(), agent.to_string()))
}

/// Prune only the still-staged (logically absent) replica scopes after a pre-commit
/// rollback (#1063). Dormant in production; the origin `_agent_*` target is skipped.
fn prune_still_staged_targets(
    mut gate: Option<&mut ProjectSeedManifestGuard>,
    activation: Option<&ManifestActivationToken>,
    still_staged: &[StagedAgentDeleteTarget],
) {
    for target in still_staged {
        if let Some((workgroup, agent)) = replica_prefix_for_target(&target.original_path) {
            prune_replica_config_scope(gate.as_deref_mut(), activation, &workgroup, &agent);
        }
    }
}

/// Read a team `config.json`'s exact current bytes for the #1063 pre-write precheck.
fn read_team_config_bytes(config_path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(config_path).map_err(|e| format!("{}: {}", config_path.display(), e))
}

/// Blocking Agent Matrix metadata commit under the reacquired project→team guards
/// (#1063, plan sections 5.4/11.39). It runs the exact-byte precheck, writes each
/// planned team config while journaling successes, removes only the two live agent
/// keys under `SettingsState::blocking_write`, and on any failure restores the
/// journaled entries once in reverse order (byte-checked) before returning.
fn persist_agent_delete_metadata_blocking(
    metadata: &PreparedAgentDeleteMetadata,
    settings: &SettingsState,
    team_guard: &TeamConfigMutationGuard,
    save_settings_fn: impl Fn(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(), String> {
    team_guard.require_ac_root(&metadata.ac_root)?;
    metadata.settings_changed.store(false, Ordering::SeqCst);

    // Exact-byte precheck under the reacquired #1056 guard: require the exact
    // prepared bytes before any write. The byte check is additional to the guard,
    // never a substitute for it.
    for mutation in &metadata.team_mutations {
        let current = read_team_config_bytes(&mutation.config_path)?;
        if current != mutation.before_json {
            return Err(format!(
                "Team '{}' config changed under the lock before agent delete; aborting without \
                 writes",
                mutation.team_name
            ));
        }
    }

    // Write each planned config, journaling only successful writes for reverse restore.
    let mut journal: Vec<usize> = Vec::new();
    for (index, mutation) in metadata.team_mutations.iter().enumerate() {
        if let Err(e) = write_team_config_json_atomic(
            &metadata.ac_root,
            team_guard,
            &mutation.config_path,
            &mutation.after_json,
        ) {
            let restore = restore_journaled_team_configs(metadata, team_guard, &journal);
            return Err(format!(
                "Failed to write team '{}' config during agent delete: {}{}",
                mutation.team_name,
                e,
                format_restore_suffix(restore)
            ));
        }
        journal.push(index);
    }

    // Settings commit: remove only the two live agent keys under blocking_write.
    let settings_result = {
        let mut guard = settings.blocking_write();
        let removed_auto = guard.auto_self_clear_by_agent.remove(&metadata.agent_name);
        let removed_default = guard
            .coding_agent_profiles
            .default_profile_by_agent
            .remove(&metadata.agent_name);
        let changed = removed_auto.is_some() || removed_default.is_some();
        metadata.settings_changed.store(changed, Ordering::SeqCst);
        if changed {
            let mut candidate = guard.clone();
            if let Err(e) = crate::config::settings::validate_and_repair_settings(&mut candidate) {
                restore_removed_settings(
                    &mut guard,
                    &metadata.agent_name,
                    removed_auto,
                    removed_default,
                );
                Err(format!(
                    "Failed to validate settings after agent delete: {}",
                    e
                ))
            } else {
                match save_settings_fn(&candidate) {
                    Ok(written) => {
                        *guard = written;
                        Ok(())
                    }
                    Err(e) => {
                        restore_removed_settings(
                            &mut guard,
                            &metadata.agent_name,
                            removed_auto,
                            removed_default,
                        );
                        Err(format!("Failed to save settings after agent delete: {}", e))
                    }
                }
            }
        } else {
            Ok(())
        }
    };
    if let Err(e) = settings_result {
        let restore = restore_journaled_team_configs(metadata, team_guard, &journal);
        return Err(format!("{}{}", e, format_restore_suffix(restore)));
    }
    Ok(())
}

/// Restore journaled team configs once, in reverse order, only when the current
/// bytes still equal our written `after_json` (else a lock-unaware writer changed
/// it after our write; leave the newer content and report a typed conflict).
fn restore_journaled_team_configs(
    metadata: &PreparedAgentDeleteMetadata,
    team_guard: &TeamConfigMutationGuard,
    journal: &[usize],
) -> Result<(), String> {
    team_guard.require_ac_root(&metadata.ac_root)?;
    let mut errors = Vec::new();
    for &index in journal.iter().rev() {
        let mutation = &metadata.team_mutations[index];
        match read_team_config_bytes(&mutation.config_path) {
            Ok(current) if current == mutation.after_json => {
                if let Err(e) = write_team_config_json_atomic(
                    &metadata.ac_root,
                    team_guard,
                    &mutation.config_path,
                    &mutation.before_json,
                ) {
                    errors.push(format!("{}: {}", mutation.config_path.display(), e));
                }
            }
            Ok(_) => errors.push(format!(
                "{}: restore conflict (content changed under the lock after write)",
                mutation.config_path.display()
            )),
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Failed to restore team config snapshot(s): {}",
            errors.join("; ")
        ))
    }
}

#[derive(Debug, Clone)]
struct AgentDeleteTarget {
    original_path: PathBuf,
    original_key: String,
    label: String,
}

#[derive(Debug, Clone)]
struct StagedAgentDeleteTarget {
    original_path: PathBuf,
    original_key: String,
    staged_path: PathBuf,
    label: String,
}

#[derive(Debug, Clone)]
struct AgentTeamMutation {
    team_name: String,
    config_path: PathBuf,
    before_json: Vec<u8>,
    after_json: Vec<u8>,
}

#[derive(Debug, Clone)]
struct PreparedAgentDeleteMetadata {
    ac_root: PathBuf,
    team_mutations: Vec<AgentTeamMutation>,
    agent_name: String,
    settings_changed: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
struct AgentDeletePlan {
    ac_root: PathBuf,
    agent_name: String,
    agent_ref: String,
    origin_dir: PathBuf,
    target_keys: Vec<String>,
    targets: Vec<AgentDeleteTarget>,
    team_mutations: Vec<AgentTeamMutation>,
}

fn resolve_agent_delete_identity(
    base: &Path,
    agent_path: &Path,
) -> Result<(String, String, PathBuf), String> {
    if !agent_path.is_absolute() {
        return Err("Agent path must be absolute".to_string());
    }
    let final_segment = agent_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Agent path must end with an agent matrix directory".to_string())?;
    let agent_name = final_segment
        .strip_prefix("_agent_")
        .ok_or_else(|| "Agent path must end with _agent_<name>".to_string())?;
    validate_existing_name(agent_name, "Agent")?;
    validate_delete_root_not_link_or_reparse(agent_path)
        .map_err(|e| format!("Selected agent path is not deletable: {}", e))?;

    let agent_ref = format!("_agent_{}", agent_name);
    let origin_dir = base.join(&agent_ref);
    if path_key_for_delete(agent_path) != path_key_for_delete(&origin_dir) {
        return Err("Agent path does not match the selected project".to_string());
    }
    validate_delete_root_not_link_or_reparse(&origin_dir)
        .map_err(|e| format!("Agent '{}' not found or not deletable: {}", agent_name, e))?;

    Ok((agent_name.to_string(), agent_ref, origin_dir))
}

#[cfg(test)]
fn collect_agent_delete_plan(base: &Path, agent_path: &Path) -> Result<AgentDeletePlan, String> {
    let guard = TeamConfigMutationGuard::acquire(base)?;
    collect_agent_delete_plan_guarded(base, agent_path, &guard)
}

fn collect_agent_delete_plan_guarded(
    base: &Path,
    agent_path: &Path,
    guard: &TeamConfigMutationGuard,
) -> Result<AgentDeletePlan, String> {
    guard.require_ac_root(base)?;
    let (agent_name, agent_ref, origin_dir) = resolve_agent_delete_identity(base, agent_path)?;
    let team_mutations = collect_agent_team_mutations_raw(base, &agent_name, &agent_ref)?;

    let mut targets = vec![AgentDeleteTarget {
        original_key: path_key_for_delete(&origin_dir),
        original_path: origin_dir.clone(),
        label: agent_ref.clone(),
    }];

    for wg_dir in list_workgroup_dirs(base) {
        let replica = wg_dir.join(format!("__agent_{}", agent_name));
        // #1063: exact `symlink_metadata` handling instead of `Path::exists`, which
        // collapses access and other inspection errors to false. Exact `NotFound`
        // means absent and adds no target (committed-intent pruning by agent
        // component still covers any stale rows at commit); every other inspection
        // error aborts preflight; an existing entry must pass the non-reparse
        // directory validation before its prefix is retained as a target.
        match std::fs::symlink_metadata(&replica) {
            Ok(_) => {
                validate_delete_root_not_link_or_reparse(&replica).map_err(|e| {
                    format!(
                        "Agent replica '{}' is not deletable: {}",
                        replica.display(),
                        e
                    )
                })?;
                let wg_name = wg_dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("workgroup");
                targets.push(AgentDeleteTarget {
                    original_key: path_key_for_delete(&replica),
                    original_path: replica,
                    label: format!("{}/__agent_{}", wg_name, agent_name),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(format!(
                    "Failed to inspect agent replica '{}': {}",
                    replica.display(),
                    e
                ));
            }
        }
    }

    targets.sort_by(|a, b| a.original_key.cmp(&b.original_key));
    let mut target_keys = Vec::new();
    for target in &targets {
        for key in target_keys_for_delete(&target.original_path) {
            if !target_keys.contains(&key) {
                target_keys.push(key);
            }
        }
    }

    Ok(AgentDeletePlan {
        ac_root: base.to_path_buf(),
        agent_name,
        agent_ref,
        origin_dir,
        target_keys,
        targets,
        team_mutations,
    })
}

fn collect_agent_team_mutations_raw(
    base: &Path,
    agent_name: &str,
    agent_ref: &str,
) -> Result<Vec<AgentTeamMutation>, String> {
    let mut team_dirs = Vec::new();
    let entries = std::fs::read_dir(base).map_err(|e| {
        format!(
            "Cannot read Project AC Root directory for agent delete: {}",
            e
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            format!(
                "Cannot read directory entry during agent delete scan: {}",
                e
            )
        })?;
        let path = entry.path();
        let Some(dir_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path.is_dir() && dir_name.starts_with("_team_") {
            team_dirs.push(path);
        }
    }
    team_dirs.sort();

    let mut coordinator_blockers = Vec::new();
    let mut mutations = Vec::new();
    for team_dir in team_dirs {
        let dir_name = team_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("_team_unknown");
        let team_name = dir_name.strip_prefix("_team_").unwrap_or(dir_name);
        let config_path = team_dir.join("config.json");
        let before_json = match std::fs::read(&config_path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(format!(
                    "Cannot read team '{}' config.json: {}",
                    team_name, e
                ));
            }
        };
        let mut value: serde_json::Value = match serde_json::from_slice(&before_json) {
            Ok(value) => value,
            Err(e) => {
                let raw = String::from_utf8_lossy(&before_json);
                if raw_text_mentions_agent_token(&raw, agent_name, agent_ref) {
                    return Err(format!(
                        "Cannot delete agent '{}': cannot verify team '{}' references because \
                         config.json is invalid: {}",
                        agent_name, team_name, e
                    ));
                }
                log::warn!(
                    "[entity_creation] Skipping unrelated malformed team config for '{}': {}",
                    team_name,
                    e
                );
                continue;
            }
        };

        if value
            .get("coordinator")
            .and_then(|coordinator| coordinator.as_str())
            .is_some_and(|coordinator| agent_ref_matches_target(coordinator, agent_name))
        {
            coordinator_blockers.push(team_name.to_string());
        }

        let mut changed = false;
        if let Some(agents) = value
            .get_mut("agents")
            .and_then(|agents| agents.as_array_mut())
        {
            let before_len = agents.len();
            agents.retain(|agent| match agent.as_str() {
                Some(agent) => !agent_ref_matches_target(agent, agent_name),
                None => true,
            });
            changed |= agents.len() != before_len;
        }

        if let Some(repos) = value
            .get_mut("repos")
            .and_then(|repos| repos.as_array_mut())
        {
            for repo in repos {
                let Some(repo_obj) = repo.as_object_mut() else {
                    continue;
                };
                let Some(agents) = repo_obj
                    .get_mut("agents")
                    .and_then(|agents| agents.as_array_mut())
                else {
                    continue;
                };
                let before_len = agents.len();
                agents.retain(|agent| match agent.as_str() {
                    Some(agent) => !agent_ref_matches_target(agent, agent_name),
                    None => true,
                });
                changed |= agents.len() != before_len;
            }
        }

        if changed {
            let object = value.as_object_mut().ok_or_else(|| {
                format!(
                    "Cannot delete agent '{}': team '{}' config.json must be an object",
                    agent_name, team_name
                )
            })?;
            normalize_raw_context_alert_percentages(object).map_err(|e| {
                format!(
                    "Cannot delete agent '{}': team '{}' has invalid contextAlertPercentages: {}",
                    agent_name, team_name, e
                )
            })?;
            let mut after_json = serde_json::to_vec_pretty(&value)
                .map_err(|e| format!("Cannot serialize team '{}' config.json: {}", team_name, e))?;
            after_json.push(b'\n');
            mutations.push(AgentTeamMutation {
                team_name: team_name.to_string(),
                config_path,
                before_json,
                after_json,
            });
        }
    }

    if !coordinator_blockers.is_empty() {
        coordinator_blockers.sort();
        return Err(format!(
            "Cannot delete agent '{}': orchestrator of team(s): {}. Reassign the orchestrator \
             first.",
            agent_name,
            coordinator_blockers.join(", ")
        ));
    }

    Ok(mutations)
}

fn normalize_raw_context_alert_percentages(
    object: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let values = match object.get("contextAlertPercentages") {
        Some(value) => serde_json::from_value::<Vec<u8>>(value.clone()).map_err(|e| {
            format!(
                "expected an array of integer percentages representable as u8: {}",
                e
            )
        })?,
        None => Vec::new(),
    };
    let normalized = normalize_context_alert_percentages(&values)?;
    object.insert(
        "contextAlertPercentages".to_string(),
        serde_json::to_value(normalized)
            .map_err(|e| format!("failed to serialize canonical percentages: {}", e))?,
    );
    Ok(())
}

fn agent_ref_matches_target(raw_ref: &str, agent_name: &str) -> bool {
    let bare = raw_agent_ref_bare_name(raw_ref);
    agent_name_matches(&bare, agent_name)
}

fn raw_agent_ref_bare_name(raw_ref: &str) -> String {
    let normalized = raw_ref.replace('\\', "/");
    let final_segment = normalized.rsplit('/').next().unwrap_or(&normalized);
    final_segment
        .strip_prefix("_agent_")
        .unwrap_or(final_segment)
        .to_string()
}

fn raw_text_mentions_agent_token(raw: &str, agent_name: &str, agent_ref: &str) -> bool {
    contains_whole_agent_token(raw, agent_ref) || contains_whole_agent_token(raw, agent_name)
}

fn contains_whole_agent_token(raw: &str, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }

    #[cfg(windows)]
    let (haystack, needle) = (raw.to_ascii_lowercase(), token.to_ascii_lowercase());
    #[cfg(not(windows))]
    let (haystack, needle) = (raw.to_string(), token.to_string());

    let mut start = 0;
    while let Some(offset) = haystack[start..].find(&needle) {
        let match_start = start + offset;
        let match_end = match_start + needle.len();
        let before_ok = haystack[..match_start]
            .chars()
            .next_back()
            .map(|c| !is_agent_token_char(c))
            .unwrap_or(true);
        let after_ok = haystack[match_end..]
            .chars()
            .next()
            .map(|c| !is_agent_token_char(c))
            .unwrap_or(true);
        if before_ok && after_ok {
            return true;
        }
        start = match_end;
    }
    false
}

fn is_agent_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn agent_name_matches(left: &str, right: &str) -> bool {
    #[cfg(windows)]
    {
        left.eq_ignore_ascii_case(right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

async fn preflight_agent_delete(
    plan: &AgentDeletePlan,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    for target in &plan.targets {
        validate_delete_root_not_link_or_reparse(&target.original_path).map_err(|e| {
            format!(
                "Cannot delete agent '{}': target '{}' is not deletable: {}",
                plan.agent_name, target.label, e
            )
        })?;
    }
    ensure_no_live_sessions_under_target_keys(&plan.target_keys, &plan.agent_name, session_mgr)
        .await
}

async fn ensure_no_live_sessions_under_target_keys(
    target_keys: &[String],
    agent_name: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    let manager = { session_mgr.read().await.clone() };
    let sessions = manager.list_sessions().await;
    let blockers: Vec<_> = sessions
        .into_iter()
        .filter(|session| !matches!(session.status, SessionStatus::Exited(_)))
        .filter(|session| {
            let working_dir = Path::new(&session.working_directory);
            let working_key = path_key_for_delete(working_dir);
            target_keys.iter().any(|root_key| {
                working_key == *root_key || working_key.starts_with(&(root_key.clone() + "/"))
            })
        })
        .collect();

    if blockers.is_empty() {
        return Ok(());
    }

    let details = blockers
        .iter()
        .map(|session| {
            format!(
                "  - {} at {} ({:?})",
                session.name, session.working_directory, session.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "Cannot delete agent '{}' while live sessions exist:\n{}",
        agent_name, details
    ))
}

async fn prepare_agent_delete_metadata(
    plan: &AgentDeletePlan,
    settings: &SettingsState,
) -> Result<PreparedAgentDeleteMetadata, String> {
    let settings_snapshot = settings.read().await;
    let settings_changed = settings_snapshot
        .auto_self_clear_by_agent
        .contains_key(&plan.agent_name)
        || settings_snapshot
            .coding_agent_profiles
            .default_profile_by_agent
            .contains_key(&plan.agent_name);

    Ok(PreparedAgentDeleteMetadata {
        ac_root: plan.ac_root.clone(),
        team_mutations: plan.team_mutations.clone(),
        agent_name: plan.agent_name.clone(),
        settings_changed: Arc::new(AtomicBool::new(settings_changed)),
    })
}

// #1063: the async agent-delete metadata helpers below are superseded in
// production by `persist_agent_delete_metadata_blocking` (run under the blocking
// project→team→settings transaction). They are retained only as `#[cfg(test)]`
// scaffolding for the pre-existing focused rollback/restore/format tests.
#[cfg(test)]
async fn persist_agent_delete_metadata_with_saver<S>(
    metadata: &PreparedAgentDeleteMetadata,
    settings: &SettingsState,
    team_guard: &TeamConfigMutationGuard,
    save_settings_fn: S,
) -> Result<(), String>
where
    S: Fn(&AppSettings) -> Result<AppSettings, String>,
{
    persist_agent_delete_metadata_with_writers(
        metadata,
        settings,
        team_guard,
        |config_path, json| {
            write_team_config_json_atomic(&metadata.ac_root, team_guard, config_path, json)
        },
        save_settings_fn,
    )
    .await
}

#[cfg(test)]
async fn persist_agent_delete_metadata_with_writers<T, S>(
    metadata: &PreparedAgentDeleteMetadata,
    settings: &SettingsState,
    team_guard: &TeamConfigMutationGuard,
    mut write_team_config_fn: T,
    save_settings_fn: S,
) -> Result<(), String>
where
    T: FnMut(&Path, &[u8]) -> Result<(), String>,
    S: Fn(&AppSettings) -> Result<AppSettings, String>,
{
    team_guard.require_ac_root(&metadata.ac_root)?;
    metadata.settings_changed.store(false, Ordering::SeqCst);

    for mutation in &metadata.team_mutations {
        if let Err(e) = write_team_config_fn(&mutation.config_path, &mutation.after_json) {
            let restore = restore_team_config_snapshots(metadata, team_guard);
            return Err(format!(
                "Failed to write team '{}' config during agent delete: {}{}",
                mutation.team_name,
                e,
                format_restore_suffix(restore)
            ));
        }
    }

    let settings_result = {
        let mut guard = settings.write().await;
        let removed_auto = guard.auto_self_clear_by_agent.remove(&metadata.agent_name);
        let removed_default = guard
            .coding_agent_profiles
            .default_profile_by_agent
            .remove(&metadata.agent_name);
        let changed = removed_auto.is_some() || removed_default.is_some();
        metadata.settings_changed.store(changed, Ordering::SeqCst);

        if changed {
            let mut candidate = guard.clone();
            if let Err(e) = crate::config::settings::validate_and_repair_settings(&mut candidate) {
                restore_removed_settings(
                    &mut guard,
                    &metadata.agent_name,
                    removed_auto,
                    removed_default,
                );
                Err(format!(
                    "Failed to validate settings after agent delete: {}",
                    e
                ))
            } else {
                match save_settings_fn(&candidate) {
                    Ok(written) => {
                        *guard = written;
                        Ok(())
                    }
                    Err(e) => {
                        restore_removed_settings(
                            &mut guard,
                            &metadata.agent_name,
                            removed_auto,
                            removed_default,
                        );
                        Err(format!("Failed to save settings after agent delete: {}", e))
                    }
                }
            }
        } else {
            Ok(())
        }
    };

    if let Err(e) = settings_result {
        let restore = restore_team_config_snapshots(metadata, team_guard);
        return Err(format!("{}{}", e, format_restore_suffix(restore)));
    }

    Ok(())
}

fn restore_removed_settings(
    settings: &mut AppSettings,
    agent_name: &str,
    removed_auto: Option<bool>,
    removed_default: Option<String>,
) {
    if let Some(value) = removed_auto {
        settings
            .auto_self_clear_by_agent
            .insert(agent_name.to_string(), value);
    }
    if let Some(value) = removed_default {
        settings
            .coding_agent_profiles
            .default_profile_by_agent
            .insert(agent_name.to_string(), value);
    }
}

#[cfg(test)]
async fn restore_agent_delete_metadata_snapshots(
    metadata: &PreparedAgentDeleteMetadata,
    _settings: &SettingsState,
    team_guard: &TeamConfigMutationGuard,
) -> Result<(), String> {
    restore_team_config_snapshots(metadata, team_guard)
}

#[cfg(test)]
fn restore_team_config_snapshots(
    metadata: &PreparedAgentDeleteMetadata,
    team_guard: &TeamConfigMutationGuard,
) -> Result<(), String> {
    team_guard.require_ac_root(&metadata.ac_root)?;
    let mut errors = Vec::new();
    for mutation in &metadata.team_mutations {
        if let Err(e) = write_team_config_json_atomic(
            &metadata.ac_root,
            team_guard,
            &mutation.config_path,
            &mutation.before_json,
        ) {
            errors.push(format!("{}: {}", mutation.config_path.display(), e));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Failed to restore team config snapshot(s): {}",
            errors.join("; ")
        ))
    }
}

fn format_restore_suffix(restore: Result<(), String>) -> String {
    match restore {
        Ok(()) => String::new(),
        Err(e) => format!("; snapshot restore failed: {}", e),
    }
}

fn write_team_config_json_atomic(
    ac_root: &Path,
    team_guard: &TeamConfigMutationGuard,
    config_path: &Path,
    json: &[u8],
) -> Result<(), String> {
    team_guard.require_ac_root(ac_root)?;
    crate::config::local_config_io::write_file_atomic(config_path, json)
}

#[cfg(test)]
fn format_agent_delete_metadata_failure(
    persist_error: String,
    restore_result: Result<(), String>,
    dir_rollback_result: Result<(), String>,
) -> String {
    match (restore_result, dir_rollback_result) {
        (Ok(()), Ok(())) => persist_error,
        (Err(restore), Ok(())) => format!(
            "{}. Metadata snapshot restore also failed: {}",
            persist_error, restore
        ),
        (Ok(()), Err(rollback)) => format!(
            "{}. Directory rollback also failed: {}",
            persist_error, rollback
        ),
        (Err(restore), Err(rollback)) => format!(
            "{}. Metadata snapshot restore also failed: {}. Directory rollback also failed: {}",
            persist_error, restore, rollback
        ),
    }
}

#[cfg(test)]
fn format_agent_delete_post_stage_live_failure(
    live_error: String,
    dir_rollback_result: Result<(), String>,
) -> String {
    match dir_rollback_result {
        Ok(()) => live_error,
        Err(rollback) => format!(
            "Agent delete post-stage live-session check failed and rollback also failed. \
             Live-session error: {}. Rollback error: {}",
            live_error, rollback
        ),
    }
}

/// List all agent matrices across multiple project paths.
/// Scans {project}/.ac/_agent_*/ and reads Role.md frontmatter.
#[tauri::command]
pub async fn list_all_agents(
    project_paths: Vec<String>,
    destination_project_path: Option<String>,
) -> Result<Vec<AgentInfo>, String> {
    list_all_agents_for_destination(&project_paths, destination_project_path.as_deref())
}

fn project_directory_identity(project: &Path) -> Result<(PathBuf, MatrixCopyIdentity), String> {
    let canonical = project.canonicalize().map_err(|e| e.to_string())?;
    selected_ac_root(&canonical)?;
    let handle = OwnedCopyArtifacts::open_existing(&canonical, MatrixCopyKind::Directory)?;
    Ok((canonical, OwnedCopyArtifacts::handle_identity(&handle)?))
}

fn list_all_agents_for_destination(
    project_paths: &[String],
    destination_project_path: Option<&str>,
) -> Result<Vec<AgentInfo>, String> {
    let destination = destination_project_path
        .map(|path| project_directory_identity(Path::new(path)).map(|(_, identity)| identity))
        .transpose()?;
    let mut agents: Vec<AgentInfo> = Vec::new();

    for project_path in project_paths {
        let Ok((project, identity)) = project_directory_identity(Path::new(project_path)) else {
            continue;
        };
        let base = project.as_path();
        let Some(ac_root) = existing_ac_root(base) else {
            continue;
        };

        let project_name = base
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let entries = match std::fs::read_dir(&ac_root) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            if !dir_name.starts_with("_agent_") {
                continue;
            }

            let agent_name_from_dir = dir_name
                .strip_prefix("_agent_")
                .unwrap_or(&dir_name)
                .to_string();

            // Try to read Role.md frontmatter for richer metadata
            let role_path = path.join("Role.md");
            let (fm_name, fm_description) = if role_path.exists() {
                match std::fs::read_to_string(&role_path) {
                    Ok(content) => parse_role_frontmatter(&content),
                    Err(_) => (None, None),
                }
            } else {
                (None, None)
            };

            agents.push(AgentInfo {
                name: fm_name.unwrap_or(agent_name_from_dir),
                description: fm_description.unwrap_or_default(),
                path: path.to_string_lossy().to_string(),
                project_name: project_name.clone(),
                project_path: crate::path_utils::path_to_string_without_windows_verbatim_prefix(
                    base,
                ),
                is_local: destination.map(|destination| destination == identity),
            });
        }
    }

    agents.sort_by_key(|a| a.name.to_lowercase());
    Ok(agents)
}

fn portable_source_ref(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.contains(['/', '\\', ':', '\0']) {
        return Err("Invalid portable agent reference".into());
    }
    let bare = trimmed.strip_prefix("_agent_").unwrap_or(trimmed);
    validate_existing_name(bare, "Agent")?;
    Ok(format!("_agent_{bare}"))
}

fn require_absent(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
        Ok(_) => Err(format!("Destination already exists: {}", path.display())),
    }
}

fn preflight_team_sources(
    project: &Path,
    ac: &Path,
    config: &mut TeamConfigResult,
    sources: Vec<TeamAgentSource>,
) -> Result<Vec<(PathBuf, String)>, String> {
    let (_, destination_identity) = project_directory_identity(project)?;
    let mut selected = HashSet::new();
    for agent in &mut config.agents {
        *agent = portable_source_ref(agent)?;
        if !selected.insert(agent.clone()) {
            return Err(format!("Duplicate selected agent: {agent}"));
        }
    }
    if !config.coordinator.trim().is_empty() {
        config.coordinator = portable_source_ref(&config.coordinator)?;
        if !selected.contains(&config.coordinator) {
            return Err("Orchestrator must be one of the selected agents".into());
        }
    }
    for repo in &mut config.repos {
        for agent in &mut repo.agents {
            *agent = portable_source_ref(agent)?;
            if !selected.contains(agent) {
                return Err("Repository agent must be one of the selected agents".into());
            }
        }
    }
    normalize_context_alert_percentages(&config.context_alert_percentages)?;
    let mut seen = HashSet::new();
    let mut foreign = Vec::new();
    for source in sources {
        let agent = portable_source_ref(&source.agent_ref)?;
        if !selected.contains(&agent) || !seen.insert(agent.clone()) {
            return Err(format!("Extra or duplicate agent source: {agent}"));
        }
        let (source_project, identity) =
            project_directory_identity(Path::new(&source.source_project_path))?;
        let source_ac = selected_ac_root(&source_project)?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        resolve_agent_ref(&source_ac, &agent)?;
        let matrix = source_ac.join(&agent);
        let handle = OwnedCopyArtifacts::open_existing(&matrix, MatrixCopyKind::Directory)?;
        drop(handle);
        let canonical_matrix = matrix.canonicalize().map_err(|e| e.to_string())?;
        let parent = canonical_matrix
            .parent()
            .ok_or("Source Matrix has no parent")?;
        let parent_handle = OwnedCopyArtifacts::open_existing(parent, MatrixCopyKind::Directory)?;
        let ac_handle = OwnedCopyArtifacts::open_existing(&source_ac, MatrixCopyKind::Directory)?;
        if OwnedCopyArtifacts::handle_identity(&parent_handle)?
            != OwnedCopyArtifacts::handle_identity(&ac_handle)?
        {
            return Err("Source Matrix is not a direct project child".into());
        }
        if identity != destination_identity {
            require_absent(&ac.join(&agent))?;
            foreign.push((source_project, agent));
        }
    }
    if seen != selected {
        return Err("Agent sources must cover exactly all selected agents".into());
    }
    Ok(foreign)
}

fn verify_owned_entry(owned: &OwnedCopyArtifacts, entry: &OwnedCopyEntry) -> Result<(), String> {
    owned.verify_directories(&entry.relative)?;
    let path = owned.root.join(&entry.relative);
    let handle = OwnedCopyArtifacts::open_existing(&path, entry.kind)?;
    if OwnedCopyArtifacts::handle_identity(&handle)? != entry.identity {
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

fn verify_team_copies(copies: &[OwnedCopyArtifacts]) -> Result<(), String> {
    for copy in copies {
        for entry in &copy.entries {
            verify_owned_entry(copy, entry)?;
        }
        let history =
            std::fs::read(copy.root.join(COPY_PROVENANCE_FILE)).map_err(|e| e.to_string())?;
        validate_copy_provenance(Some(&history))?;
    }
    Ok(())
}

fn write_owned_team_file(
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

fn publish_owned_team(
    owned: &mut OwnedCopyArtifacts,
    bytes: &[u8],
    copies: &[OwnedCopyArtifacts],
    hooks: &mut MatrixCopyHooks<'_>,
) -> Result<(), String> {
    owned.create_directory(Path::new(""))?;
    owned.create_directory(Path::new("memory"))?;
    hooks.run(MatrixCopyStage::TeamConventions, &owned.root)?;
    write_owned_team_file(owned, Path::new("conventions.md"), &[])?;
    let temporary = PathBuf::from(format!(".config-{}.tmp", uuid::Uuid::new_v4()));
    write_owned_team_file(owned, &temporary, bytes)?;
    hooks.run(MatrixCopyStage::TeamPublication, &owned.root)?;
    verify_team_copies(copies)?;
    for entry in &owned.entries {
        verify_owned_entry(owned, entry)?;
    }
    let temporary_entry = owned.entries.last().expect("temporary entry");
    let identity = temporary_entry.identity;
    let digest = temporary_entry.digest;
    std::fs::hard_link(owned.root.join(&temporary), owned.root.join("config.json"))
        .map_err(|e| format!("Failed to publish config.json: {e}"))?;
    owned.entries.push(OwnedCopyEntry {
        relative: PathBuf::from("config.json"),
        identity,
        kind: MatrixCopyKind::File,
        digest,
    });
    hooks.run(MatrixCopyStage::TeamUnlink, &owned.root)?;
    // Verify both names and their directory chain before removing our temporary.
    for entry in owned.entries.iter().rev().take(2) {
        verify_owned_entry(owned, entry)?;
    }
    std::fs::remove_file(owned.root.join(&temporary)).map_err(|e| e.to_string())?;
    owned.entries.retain(|entry| entry.relative != temporary);
    Ok(())
}

fn create_team_from_sources(
    project: &Path,
    name: &str,
    mut config: TeamConfigResult,
    sources: Option<Vec<TeamAgentSource>>,
    hooks: &mut MatrixCopyHooks<'_>,
) -> Result<PathBuf, String> {
    validate_existing_name(name, "Team")?;
    let ac = selected_ac_root(project)?;
    let guard = TeamConfigMutationGuard::acquire(&ac)?;
    let Some(sources) = sources else {
        return create_new_team_config_on_disk_guarded(&ac, name, &config, &guard);
    };
    let ac = ac.canonicalize().map_err(|e| e.to_string())?;
    let mut team = OwnedCopyArtifacts {
        root: ac.join(format!("_team_{name}")),
        entries: Vec::new(),
    };
    require_absent(&team.root)?;
    let foreign = preflight_team_sources(project, &ac, &mut config, sources)?;
    let mut copies = Vec::new();
    let result = (|| -> Result<(), String> {
        for (source_project, agent) in foreign {
            copies.push(copy_agent_matrix_strict_with_hooks(
                &source_project,
                &agent,
                &ac,
                &agent,
                hooks,
            )?);
        }
        let bytes = normalized_team_config_bytes(&ac, &config)?;
        publish_owned_team(&mut team, &bytes, &copies, hooks)
    })();
    if let Err(primary) = result {
        let mut errors = vec![primary];
        if let Err(error) = hooks.run(MatrixCopyStage::BeforeCleanup, &team.root) {
            errors.push(error);
        }
        for owned in std::iter::once(&team).chain(copies.iter().rev()) {
            if let Err(error) = cleanup_owned_copy_artifacts(owned) {
                errors.push(error);
            }
        }
        return Err(errors.join("; "));
    }
    Ok(team.root)
}

/// Create a team directory inside {project_path}/.ac/_team_{name}/
// Tauri command parameters plus injected state exceed Clippy's argument threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn create_team(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    name: String,
    agents: Vec<String>,
    coordinator: String,
    repos: Vec<RepoAssignment>,
    context_alert_percentages: Option<Vec<u8>>,
    agent_sources: Option<Vec<TeamAgentSource>>,
) -> Result<CreatedEntityResult, String> {
    let safe_name = sanitize_name(&name)?;
    let context_alert_percentages = resolve_context_alert_argument(
        ContextAlertWriteMode::Create,
        context_alert_percentages,
        None,
    )?;
    let config = TeamConfigResult {
        agents,
        coordinator,
        repos,
        context_alert_percentages,
    };
    let team_dir = tokio::task::spawn_blocking(move || {
        create_team_from_sources(
            Path::new(&project_path),
            &safe_name,
            config,
            agent_sources,
            &mut MatrixCopyHooks::default(),
        )
    })
    .await
    .map_err(|e| format!("Team creation worker failed: {e}"))??;

    let result_path = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&team_dir);
    log::info!("[entity_creation] Created team: {}", result_path);
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(CreatedEntityResult { path: result_path })
}

/// Create a workgroup from an existing team.
/// Clones repos async — partial failures are reported but don't rollback the WG.
// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[tauri::command]
pub async fn create_workgroup(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    team_name: String,
    task_title: String,
) -> Result<WorkgroupCloneResult, String> {
    let safe_team = sanitize_name(&team_name)?;
    let task_content = prepare_new_room_task_content(&task_title)?;
    let base = selected_ac_root(Path::new(&project_path))?;

    // Ensure gitignore protects workgroup clones from parent repo operations
    if let Err(e) = crate::commands::ac_discovery::ensure_ac_root_gitignore(&base) {
        log::warn!(
            "[create_workgroup] Failed to ensure Project AC Root .gitignore: {}",
            e
        );
    }

    let team_config = read_team_config(&base, &safe_team)?;

    // Determine next WG number
    let wg_number = determine_next_wg_number(&base);

    let wg_name = room_dir_name(wg_number, &safe_team, room_number_pad_width());
    let wg_dir = base.join(&wg_name);
    if wg_dir.exists() {
        return Err(format!("Room directory already exists: {}", wg_name));
    }
    std::fs::create_dir_all(&wg_dir)
        .map_err(|e| format!("Failed to create room directory: {}", e))?;
    std::fs::create_dir_all(wg_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME))
        .map_err(|e| format!("Failed to create messaging directory: {}", e))?;

    // Write the prepared Clean or user-owned task before replicas and clones.
    std::fs::write(wg_dir.join("TASK.md"), &task_content)
        .map_err(|e| format!("Failed to write TASK.md: {}", e))?;

    let team_agents = team_config.agents;
    let team_repos = team_config.repos;

    // Collect unique repo URLs and their directory names
    let mut unique_repos: Vec<(String, String)> = Vec::new(); // (url, dir_name)
    let mut seen_urls: HashSet<String> = HashSet::new();
    for repo in &team_repos {
        if seen_urls.insert(repo.url.clone()) {
            let dir_name = format!("repo-{}", repo_dir_name_from_url(&repo.url));
            unique_repos.push((repo.url.clone(), dir_name));
        }
    }

    // Create __agent_*/ replica dirs through the same guarded, lossless
    // initialization the CLI/on-disk path uses (#1939): a replica's tooling,
    // custom context and unknown keys survive re-creation.
    initialize_workgroup_replicas_on_disk(&base, &wg_dir, &team_agents, &team_repos)?;

    // Clone repos (async, partial failures logged but don't rollback)
    let mut clone_errors: Vec<CloneError> = Vec::new();
    for (url, dir_name) in &unique_repos {
        let target = wg_dir.join(dir_name);
        match git_clone_async(url, &target).await {
            Ok(_) => {
                log::info!("[entity_creation] Cloned {} → {}", url, target.display());
            }
            Err(e) => {
                log::error!("[entity_creation] Failed to clone {}: {}", url, e);
                clone_errors.push(CloneError {
                    url: url.clone(),
                    error: e,
                });
            }
        }
    }

    let result_path = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&wg_dir);
    log::info!(
        "[entity_creation] Created workgroup: {} ({} clone errors)",
        result_path,
        clone_errors.len()
    );
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(WorkgroupCloneResult {
        path: result_path,
        clone_errors,
    })
}

/// Committed-intent outcome of a team delete's workgroup cascade (#1063).
struct TeamDeleteReport {
    /// Workgroup names whose directory was removed or was already absent at the
    /// explicit delete event: their config scopes are pruned and clocks cleaned.
    removed_workgroups: Vec<String>,
    /// Workgroup names whose removal failed and whose original path remains: rows
    /// and clock keys are preserved.
    failed_workgroups: Vec<String>,
}

/// Scan `base` for the present `room-N-<team>` and legacy `wg-N-<team>`
/// directories owned by `team_name`. Deleting a team removes both kinds, which
/// is the only coherent meaning of deleting a team (#1614).
fn collect_team_workgroup_dirs(base: &Path, team_name: &str) -> Vec<PathBuf> {
    let wg_suffix = format!("-{}", team_name);
    let mut wg_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // #1614: the middle segment comes from the STRIPPED remainder, never
            // from an index. The literal 3 was "wg-".len(); "room-".len() is 5,
            // so an index form would mis-slice every Room name and, for a team
            // name long enough, panic on a reversed range.
            if let Some(rest) = crate::config::entity_prefix::strip_entity_prefix(&name_str) {
                if let Some(middle) = rest.strip_suffix(&wg_suffix) {
                    if middle.parse::<u32>().is_ok() {
                        wg_dirs.push(entry.path());
                    }
                }
            }
        }
    }
    wg_dirs
}

/// Delete a team directory from {project_path}/.ac/_team_{name}/
#[tauri::command]
pub async fn delete_team(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    team_name: String,
) -> Result<(), String> {
    // #1065 Stage F: activated with the sole production token; no lock-order test barrier.
    delete_team_inner(
        &app,
        session_mgr.inner(),
        &project_path,
        &team_name,
        Some(ManifestActivationToken::production()),
        |_| {},
    )
    .await
}

/// GUI team deletion with the #1063 global lock order (plan sections 5.4/6.3).
///
/// Prompts, dirty checks, and the initial owned workgroup snapshot run outside
/// both guards. The blocking commit acquires the project gate then the #1056
/// team-config guard, revalidates the team directory under both, deletes it, and
/// attempts each workgroup while both are held. In the same manifest transaction
/// it prunes the committed intent (workgroups whose removal succeeded plus those
/// already absent at the explicit delete event) and preserves rows for any
/// workgroup whose removal failed and whose path remains. Both guards are dropped
/// before coordinator-clock locking/persistence and the async refresh, and no
/// synchronous guard crosses `.await`.
pub(crate) async fn delete_team_inner<H>(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    project_path: &str,
    team_name: &str,
    activation: Option<ManifestActivationToken>,
    after_project_before_team: H,
) -> Result<(), String>
where
    H: FnOnce(&Path) + Send + 'static,
{
    validate_existing_name(team_name, "Team")?;
    let base = selected_ac_root(Path::new(project_path))?;
    let team_dir = base.join(format!("_team_{}", team_name));
    if !team_dir.exists() {
        return Err(format!("Team '{}' not found", team_name));
    }

    // Initial owned snapshot outside both guards: the present workgroup dirs and a
    // dirty-repo safety check.
    let wg_dirs = collect_team_workgroup_dirs(&base, team_name);
    let dirty_repos = check_workgroup_repos_dirty(&wg_dirs);
    if !dirty_repos.is_empty() {
        let list = dirty_repos
            .iter()
            .map(|(repo, reason)| format!("  - {} ({})", repo, reason))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "Cannot delete team: the following repos have pending work:\n{}\n\nCommit or push \
             changes before deleting.",
            list
        ));
    }

    // Blocking commit: project gate then the #1056 team-config guard. Neither
    // synchronous guard crosses an await.
    let project_root = Path::new(project_path).to_path_buf();
    let team = team_name.to_string();
    let base_owned = base.clone();
    let team_dir_owned = team_dir.clone();
    let wg_dirs_owned = wg_dirs.clone();
    let report = crate::config::seed_manifest::run_blocking_owned(
        move || -> Result<TeamDeleteReport, String> {
            let mut project_gate = acquire_lifecycle_project_gate(&project_root)?;
            after_project_before_team(&base_owned);
            let team_guard = TeamConfigMutationGuard::acquire(&base_owned)?;
            team_guard.require_ac_root(&base_owned)?;

            // Revalidate the team directory under both guards before the commit.
            match std::fs::symlink_metadata(&team_dir_owned) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    drop(team_guard);
                    drop(project_gate);
                    return Err(format!("Team '{}' not found", team));
                }
                Err(e) => {
                    drop(team_guard);
                    drop(project_gate);
                    return Err(format!("Failed to inspect team directory: {}", e));
                }
            }

            // Commit: remove the team directory, then attempt each workgroup while
            // both guards remain held.
            std::fs::remove_dir_all(&team_dir_owned)
                .map_err(|e| format!("Failed to delete team directory: {}", e))?;
            let mut removed_workgroups = Vec::new();
            let mut failed_workgroups = Vec::new();
            for wg in &wg_dirs_owned {
                let wg_name = wg
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match remove_replica_dir(wg) {
                    ReplicaRemovalOutcome::Removed | ReplicaRemovalOutcome::AlreadyAbsent => {
                        removed_workgroups.push(wg_name);
                    }
                    ReplicaRemovalOutcome::Failed(error) => {
                        log::warn!(
                            "[entity_creation] Failed to delete workgroup {}: {}",
                            wg_name,
                            error
                        );
                        failed_workgroups.push(wg_name);
                    }
                }
            }

            // Prune the committed intent (removed plus already-absent) under the
            // gate; failed workgroups keep their rows.
            prune_team_workgroups_scope(
                project_gate.as_mut(),
                activation.as_ref(),
                removed_workgroups.clone(),
            );
            drop(team_guard);
            drop(project_gate);
            Ok(TeamDeleteReport {
                removed_workgroups,
                failed_workgroups,
            })
        },
    )
    .await
    .map_err(|e| format!("Team delete blocking preparation failed: {e}"))??;

    log::info!(
        "[entity_creation] Deleted team: {} (workgroups removed={}, failed={})",
        team_name,
        report.removed_workgroups.len(),
        report.failed_workgroups.len()
    );

    // Post-commit (guards released): drop coordinator-clock keys for removed
    // workgroups and persist once, then refresh.
    let mut team_clock_removed = 0usize;
    if let Some(project_name) = Path::new(project_path).file_name().and_then(|n| n.to_str()) {
        if let Some(clocks) =
            app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            for wg_name in &report.removed_workgroups {
                team_clock_removed += clocks
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove_workgroup(project_name, wg_name);
            }
        }
    }
    if team_clock_removed > 0 {
        if let Some(clocks) =
            app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            let snapshot = { clocks.lock().unwrap_or_else(|e| e.into_inner()).snapshot() };
            if let Err(e) = crate::config::coordinator_clocks::save_map(&snapshot) {
                log::warn!("[delete-team] clocks save failed: {}", e);
            }
        }
    }
    emit_coordinator_refresh(app, session_mgr).await;
    Ok(())
}

/// Delete a single workgroup directory from {project_path}/.ac/{wg_name}/
/// Returns dirty repo list as an Err if any repos have uncommitted/unpushed work.
/// Project-only gated workgroup-delete primitive (#1063, plan sections 5.4/6.3).
///
/// Acquires the project seed-manifest gate, runs `delete` under it, prunes the
/// workgroup config scope when the logical path was removed (`Deleted`/`Partial`),
/// and releases the gate. It NEVER acquires the team-config guard. Blocking:
/// production runs it inside `run_blocking_owned` with `None` activation and a
/// no-op barrier; tests call it directly with `Some(for_test())` and/or an
/// `after_project_acquired` cross-process inversion barrier, and may inject the
/// `delete` outcome to cover `Partial`/`Blocked` pruning.
pub(crate) fn gated_workgroup_delete_with(
    project_root: &Path,
    workgroup_name: &str,
    activation: Option<&ManifestActivationToken>,
    after_project_acquired: impl FnOnce(),
    delete: impl FnOnce() -> WgDeleteOutcome,
) -> Result<WgDeleteOutcome, String> {
    let mut project_gate = acquire_lifecycle_project_gate(project_root)?;
    after_project_acquired();
    let outcome = delete();
    if outcome.logical_path_removed() {
        prune_workgroup_config_scope(project_gate.as_mut(), activation, workgroup_name);
    }
    drop(project_gate);
    Ok(outcome)
}

/// Pass `force = true` to skip the dirty-repo safety check (user already confirmed).
#[tauri::command]
pub async fn delete_workgroup(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    project_path: String,
    workgroup_name: String,
    force: Option<bool>,
) -> Result<(), String> {
    validate_existing_name(&workgroup_name, "Room")?;

    let base = selected_ac_root(Path::new(&project_path))?;

    let wg_dir = base.join(&workgroup_name);
    validate_delete_root_not_link_or_reparse(&wg_dir)?;

    ensure_no_live_sessions_under_manager(&wg_dir, session_mgr.inner()).await?;

    // Safety check: detect dirty repos before deleting (skip if force)
    if !force.unwrap_or(false) {
        let dirty_repos = check_workgroup_repos_dirty(std::slice::from_ref(&wg_dir));
        if !dirty_repos.is_empty() {
            let list = dirty_repos
                .iter()
                .map(|(repo, reason)| format!("  - {} ({})", repo, reason))
                .collect::<Vec<_>>()
                .join("\n");
            // DIRTY_REPOS: prefix is a sentinel the frontend uses to detect this error type
            return Err(format!(
                "DIRTY_REPOS:Cannot delete room: the following repos have pending \
                 work:\n{}\n\nCommit or push changes before deleting.",
                list
            ));
        }
    }

    // Preflight rename probe (#113 follow-up): try an atomic same-parent rename
    // BEFORE remove_dir_all. NTFS rename requires DELETE access on every open
    // handle to the dir or any descendant; if any blocker holds a handle without
    // FILE_SHARE_DELETE (terminal cwd, VSCode workspace open, file watcher,
    // memory-mapped TASK.md), the rename fails atomically — no files touched —
    // and we run the diagnostic on the still-intact tree. On success the dir is
    // re-parented to a sentinel name and removed; the user-visible WG is gone.
    //
    // #1063: acquire the project-only seed-manifest gate around the atomic rename
    // and (#1065 Stage F) activated prune inside blocking work, then release it
    // before the async blocker diagnostics that
    // `delete_workgroup_dir_backend_with_outcome` runs.
    let project_root = Path::new(&project_path).to_path_buf();
    let workgroup = workgroup_name.clone();
    let wg_for_delete = wg_dir.clone();
    let outcome = crate::config::seed_manifest::run_blocking_owned(move || {
        let activation = ManifestActivationToken::production();
        gated_workgroup_delete_with(
            &project_root,
            &workgroup,
            Some(&activation),
            || {},
            || try_atomic_delete_wg(&wg_for_delete),
        )
    })
    .await
    .map_err(|e| format!("Room delete blocking preparation failed: {e}"))??;
    delete_workgroup_dir_backend_with_outcome(
        &wg_dir,
        &workgroup_name,
        session_mgr.inner(),
        outcome,
    )
    .await?;
    // (#621) Drop the workgroup's coordinator_clocks keys from the in-memory store
    // and persist immediately (this command is not on the auto-close flush tick).
    if let Some(project_name) = Path::new(&project_path)
        .file_name()
        .and_then(|n| n.to_str())
    {
        if let Some(clocks) =
            app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
        {
            crate::config::coordinator_clocks::remove_workgroup_in_state(
                clocks.inner(),
                project_name,
                &workgroup_name,
            );
        }
    }
    log::info!(
        "[entity_creation] Deleted workgroup: {} (force={})",
        workgroup_name,
        force.unwrap_or(false)
    );
    emit_coordinator_refresh(&app, session_mgr.inner()).await;
    Ok(())
}

// #1063: production `delete_workgroup` now runs the delete under the project gate
// via `gated_workgroup_delete_with` and interprets the outcome with
// `delete_workgroup_dir_backend_with_outcome`, so this ungated convenience wrapper
// is retained for tests only.
#[cfg(all(test, windows))]
pub(crate) async fn delete_workgroup_dir_backend(
    wg_dir: &Path,
    workgroup_name: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    delete_workgroup_dir_backend_with_outcome(
        wg_dir,
        workgroup_name,
        session_mgr,
        try_atomic_delete_wg(wg_dir),
    )
    .await
}

pub(crate) async fn delete_workgroup_dir_backend_with_outcome(
    wg_dir: &Path,
    workgroup_name: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    outcome: WgDeleteOutcome,
) -> Result<(), String> {
    match outcome {
        WgDeleteOutcome::Deleted => {
            // fall through to success path
        }
        WgDeleteOutcome::Blocked(e) => {
            let raw = e.to_string();
            log::info!(
                "[entity_creation] delete_workgroup: file-in-use detected for '{}' on rename \
                 probe, running blocker diagnostic on intact tree",
                workgroup_name
            );
            let report = crate::commands::wg_delete_diagnostic::diagnose_blockers(
                wg_dir,
                workgroup_name,
                &raw, // raw OS error verbatim — see plan §C.1
                session_mgr,
            )
            .await;
            let json = serde_json::to_string(&report).map_err(|se| {
                format!(
                    "Failed to serialize blocker report: {}; original error: {}",
                    se, raw
                )
            })?;
            return Err(format!("BLOCKERS:{}", json));
        }
        WgDeleteOutcome::Partial { orphan_path, error } => {
            return Err(format!(
                "Partial room delete: renamed '{}' to orphan '{}', but failed to remove orphan: {}",
                wg_dir.display(),
                orphan_path.display(),
                error
            ));
        }
        WgDeleteOutcome::Other(e) => {
            return Err(format!("Failed to delete room directory: {}", e));
        }
    }
    Ok(())
}

/// Update an existing team's config.json in {project_path}/.ac/_team_{name}/
// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn update_team(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    git_watcher: State<'_, Arc<GitWatcher>>,
    discovery_watcher: State<'_, Arc<DiscoveryBranchWatcher>>,
    project_path: String,
    team_name: String,
    agents: Vec<String>,
    coordinator: String,
    repos: Vec<RepoAssignment>,
    context_alert_percentages: Option<Vec<u8>>,
) -> Result<(), String> {
    validate_existing_name(&team_name, "Team")?;
    let base = selected_ac_root(Path::new(&project_path))?;
    let supplied_context_alert_percentages = context_alert_percentages
        .map(|values| normalize_context_alert_percentages(&values))
        .transpose()?;
    let guard = acquire_team_config_mutation_guard_async(&base).await?;

    let team_dir = base.join(format!("_team_{}", team_name));
    if !team_dir.exists() {
        return Err(format!("Team '{}' not found", team_name));
    }

    let context_alert_percentages = match supplied_context_alert_percentages {
        Some(values) => {
            resolve_context_alert_argument(ContextAlertWriteMode::Update, Some(values), None)?
        }
        None => {
            let current =
                read_team_config_classified(&base, &team_name).map_err(|e| e.to_string())?;
            resolve_context_alert_argument(
                ContextAlertWriteMode::Update,
                None,
                Some(&current.context_alert_percentages),
            )?
        }
    };
    let config = normalize_team_config_for_project(
        &base,
        &TeamConfigResult {
            agents,
            coordinator,
            repos,
            context_alert_percentages,
        },
    )?;
    write_existing_team_config_guarded(&base, &team_name, &config, &guard)?;
    drop(guard);

    log::info!("[entity_creation] Updated team: {}", team_name);

    // Propagate repo changes to existing workgroups (async now — awaits SessionManager refresh).
    match sync_workgroup_repos_inner(
        &base,
        &team_name,
        &config.repos,
        session_mgr.inner(),
        git_watcher.inner(),
        discovery_watcher.inner(),
        &app,
    )
    .await
    {
        Ok(result) => {
            log::info!(
                "[entity_creation] Synced {} workgroups, {} replicas for team '{}' ({} errors)",
                result.workgroups_updated,
                result.replicas_updated,
                team_name,
                result.errors.len()
            );
        }
        Err(e) => {
            log::warn!("[entity_creation] Failed to sync workgroup repos: {}", e);
            // Non-fatal: team config was saved successfully
        }
    }

    // Refresh coordinator flags — a team edit can add/remove the coordinator or change its
    // target.
    emit_coordinator_refresh(&app, session_mgr.inner()).await;

    Ok(())
}

/// Canonicalize an absolute or relative repo path and derive (label, absolute_path).
/// Mirrors ac_discovery.rs's source_path production so `Vec<SessionRepo>` equality
/// between the two writers holds (order and path shape both matter).
fn build_session_repo(replica_dir: &Path, rel: &str) -> Option<SessionRepo> {
    let resolved = replica_dir.join(rel);
    let abs = std::fs::canonicalize(&resolved).ok()?;
    let source_path = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&abs);
    let dir = source_path
        .replace('\\', "/")
        .split('/')
        .next_back()
        .unwrap_or("")
        .to_string();
    let label = dir.strip_prefix("repo-").map(str::to_string).unwrap_or(dir);
    Some(SessionRepo {
        label,
        source_path,
        // #1028 - undetected, like `branch` above. This literal is emitted straight to
        // the UI by `sync_workgroup_repos_inner`, so a user-initiated sync shows a
        // violet badge until the next GitWatcher tick (<=5s, forced by the
        // `invalidate_session_cache` call beside that emit). Same bound `branch`
        // already has there.
        branch: None,
        dirty: None,
    })
}

/// Core sync logic — updates repos and context in all replica configs for a team's workgroups.
/// After successful per-replica writes, pushes the new `git_repos` to any matching live session
/// via `refresh_git_repos_for_sessions` + watcher cache invalidation + `session_git_repos` emit.
/// Async so it can await the RwLock on `SessionManager`.
/// #2816 (C3) - site 7 lifted out of `sync_workgroup_repos_inner`, body
/// verbatim, so its refusal of an absent `config.json` and its no-file
/// guarantee are provable by a unit test.
fn apply_wg_replica_repos(
    replica_dir: &Path,
    assigned_repos: &[String],
) -> Result<crate::config::replica_identity::WgReplicaIdentity, String> {
    // Read existing config, preserving identity/tooling/other runtime fields
    let config_path = replica_dir.join("config.json");
    let mut repaired_identity = None;
    crate::config::agent_config::update_existing_agent_config(&config_path, |obj, _state| {
        let mut config = serde_json::Value::Object(std::mem::take(obj));
        let identity =
            repair_wg_replica_config_value(replica_dir, &mut config, WG_REPLICA_REQUIRED_CONTEXT)?;

        // Update repos
        config["repos"] = serde_json::json!(assigned_repos);

        // Context merge: prepend required tokens to maintain consistent ordering
        // with create_workgroup() (which writes [$AC_CONTEXT, $REPOS_INFO] first).
        // Preserve custom non-Role entries while replacing identity-derived Role.md
        // entries with the repaired same-workspace identity.
        let existing_context: Vec<String> = config
            .get("context")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        config["context"] = serde_json::json!(normalize_wg_replica_context_entries(
            &existing_context,
            &["$AGENTSCOMMANDER_CONTEXT"],
            &identity.identity,
            identity.matrix_dir.join(ROLE_MD_FILENAME).exists(),
        ));

        let final_obj = config.as_object_mut().ok_or_else(|| {
            format!(
                "Replica config {} must be a JSON object",
                config_path.display()
            )
        })?;
        *obj = std::mem::take(final_obj);
        repaired_identity = Some(identity);
        Ok(())
    })?;
    Ok(repaired_identity.expect("identity repaired before successful write"))
}

async fn sync_workgroup_repos_inner(
    base: &Path,
    team_name: &str,
    repos: &[RepoAssignment],
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    git_watcher: &Arc<GitWatcher>,
    discovery_watcher: &Arc<DiscoveryBranchWatcher>,
    app: &AppHandle,
) -> Result<SyncResult, String> {
    let mut result = SyncResult {
        workgroups_updated: 0,
        replicas_updated: 0,
        errors: Vec::new(),
    };

    // `updates` is built ONLY from replicas whose config.json write succeeded
    // (Grinch #15 partial-failure filter). In-memory state must match on-disk.
    let mut updates: Vec<(String, Vec<SessionRepo>)> = Vec::new();
    // Replica paths touched successfully — used for `invalidate_replicas` so the next
    // discovery poll re-registers them with fresh data (§3.2.5 / Grinch #17).
    let mut touched_replica_paths: Vec<String> = Vec::new();

    // Find all workgroups for this team (same discovery as delete_team())
    let wg_suffix = format!("-{}", team_name);
    let mut wg_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // #1614: the middle segment comes from the STRIPPED remainder, never
            // from an index. The literal 3 was "wg-".len(); "room-".len() is 5,
            // so an index form would mis-slice every Room name and, for a team
            // name long enough, panic on a reversed range.
            if let Some(rest) = crate::config::entity_prefix::strip_entity_prefix(&name_str) {
                if let Some(middle) = rest.strip_suffix(&wg_suffix) {
                    if middle.parse::<u32>().is_ok() {
                        wg_dirs.push(entry.path());
                    }
                }
            }
        }
    }

    for wg_dir in &wg_dirs {
        let mut wg_touched = false;
        let wg_name = wg_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // List __agent_* directories in this workgroup
        let replica_dirs: Vec<PathBuf> = match std::fs::read_dir(wg_dir) {
            Ok(entries) => entries
                .flatten()
                .filter(|e| {
                    e.path().is_dir() && e.file_name().to_string_lossy().starts_with("__agent_")
                })
                .map(|e| e.path())
                .collect(),
            Err(e) => {
                log::warn!("Failed to read workgroup dir {}: {}", wg_dir.display(), e);
                continue;
            }
        };

        for replica_dir in &replica_dirs {
            let dir_name = replica_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            // __agent_dev-rust → dev-rust
            let replica_name = dir_name.strip_prefix("__agent_").unwrap_or(dir_name);

            // Compute assigned repos (relative strings, written to config.json).
            let assigned_repos: Vec<String> = repos
                .iter()
                .filter(|r| r.agents.iter().any(|a| agent_matches(a, replica_name)))
                .map(|r| {
                    let d = format!("repo-{}", repo_dir_name_from_url(&r.url));
                    format!("../{}", d)
                })
                .collect();

            let _identity = match apply_wg_replica_repos(replica_dir, &assigned_repos) {
                Ok(identity) => identity,
                Err(e) => {
                    result.errors.push(SyncError {
                        replica: dir_name.to_string(),
                        error: e,
                    });
                    continue;
                }
            };

            // Write succeeded — record for in-memory refresh. Canonicalize each repo
            // path so source_path matches DiscoveryBranchWatcher's shape. Order of
            // `assigned_repos` = team config `repos` order, preserved via the filter
            // above — do NOT sort or dedupe.
            let session_repos: Vec<SessionRepo> = assigned_repos
                .iter()
                .filter_map(|rel| build_session_repo(replica_dir, rel))
                .collect();
            let session_name = format!("{}/{}", wg_name, replica_name);
            updates.push((session_name, session_repos));
            touched_replica_paths.push(replica_dir.to_string_lossy().to_string());

            result.replicas_updated += 1;
            wg_touched = true;
        }

        if wg_touched {
            result.workgroups_updated += 1;
        }
    }

    if !result.errors.is_empty() {
        log::warn!(
            "[entity_creation] sync_workgroup_repos for '{}': {} replicas updated, {} errors",
            team_name,
            result.replicas_updated,
            result.errors.len()
        );
    }

    // Refresh live sessions' git_repos in-memory so the sidebar updates before the next
    // discovery poll. CAS-guarded via git_repos_gen bump (Grinch #14 race fix).
    if !updates.is_empty() {
        let changed = {
            let mgr = session_mgr.read().await;
            mgr.refresh_git_repos_for_sessions(&updates).await
        };

        // Force DiscoveryBranchWatcher to re-register these replicas with fresh data
        // on the next `discover_project` call (§3.2.5 / Grinch #17).
        discovery_watcher.invalidate_replicas(&touched_replica_paths);

        for (session_id, repos) in changed {
            // Clear GitWatcher's cache slot so the next tick re-emits with detected branches.
            git_watcher.invalidate_session_cache(session_id);
            let _ = app.emit(
                "session_git_repos",
                serde_json::json!({
                    "sessionId": session_id.to_string(),
                    "repos": repos,
                }),
            );
        }
    }

    Ok(result)
}

/// Refresh `is_coordinator` on every live session and emit `session_coordinator_changed`
/// for those whose flag flipped. Called by team-CRUD commands (§2).
pub(crate) async fn emit_coordinator_refresh(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) {
    let teams = crate::config::teams::discover_teams();
    let changes = {
        let mgr = session_mgr.read().await;
        mgr.refresh_coordinator_flags(&teams).await
    };
    for (id, is_coord) in changes {
        let _ = app.emit(
            "session_coordinator_changed",
            CoordinatorChangedPayload {
                session_id: id.to_string(),
                is_coordinator: is_coord,
            },
        );
    }
}

/// Read a team's config.json and return its contents.
#[tauri::command]
pub async fn get_team_config(
    project_path: String,
    team_name: String,
) -> Result<TeamConfigResult, String> {
    validate_existing_name(&team_name, "Team")?;
    let base = selected_ac_root(Path::new(&project_path))?;
    read_team_config(&base, &team_name)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check all repo-* dirs inside the given workgroup dirs for dirty git state.
/// Returns a list of (repo_display_name, reason) for repos with pending work.
pub(crate) fn check_workgroup_repos_dirty(wg_dirs: &[PathBuf]) -> Vec<(String, String)> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut dirty: Vec<(String, String)> = Vec::new();

    for wg_dir in wg_dirs {
        let wg_name = wg_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let entries = match std::fs::read_dir(wg_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = entry.file_name();
            let dir_name_str = dir_name.to_string_lossy();
            if !dir_name_str.starts_with("repo-") {
                continue;
            }
            if !path.join(".git").exists() {
                continue;
            }

            let display = format!("{}/{}", wg_name, dir_name_str);
            let mut reasons: Vec<&str> = Vec::new();

            // Check for uncommitted changes (staged + unstaged + untracked)
            let mut cmd = std::process::Command::new("git");
            crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
            crate::config::agent_path::apply_search_path_to_std_command(&mut cmd);
            cmd.args(["status", "--porcelain"])
                .current_dir(&path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                #[allow(unused_imports)]
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            if let Ok(output) = cmd.output() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.trim().is_empty() {
                    reasons.push("uncommitted changes");
                }
            }

            // Check for unpushed commits
            let mut cmd2 = std::process::Command::new("git");
            crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd2);
            crate::config::agent_path::apply_search_path_to_std_command(&mut cmd2);
            cmd2.args(["log", "@{upstream}..HEAD", "--oneline"])
                .current_dir(&path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null());
            #[cfg(windows)]
            {
                #[allow(unused_imports)]
                use std::os::windows::process::CommandExt;
                cmd2.creation_flags(CREATE_NO_WINDOW);
            }
            match cmd2.output() {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if !stdout.trim().is_empty() {
                        reasons.push("unpushed commits");
                    }
                }
                _ => {
                    // No upstream configured — local-only branch = unpushed work
                    reasons.push("no remote upstream");
                }
            }

            if !reasons.is_empty() {
                dirty.push((display, reasons.join(", ")));
            }
        }
    }

    dirty
}

/// Result of a preflight-rename `delete_workgroup` attempt.
///
/// `pub(crate)` so the unit tests can pattern-match on the variants.
pub(crate) enum WgDeleteOutcome {
    /// Rename succeeded and the renamed dir was removed.
    Deleted,
    /// Rename succeeded, but deleting the renamed orphan failed. The
    /// user-visible workgroup name is gone, but callers must report this as a
    /// partial failure and skip refresh.
    Partial {
        orphan_path: PathBuf,
        error: std::io::Error,
    },
    /// Rename failed with a Windows file-in-use error. Tree is intact; caller
    /// should run the blocker diagnostic and return `BLOCKERS:` to the frontend.
    Blocked(std::io::Error),
    /// Rename failed with any other error (NotFound, permission, invalid path,
    /// …). Caller passes the raw error through unchanged.
    Other(std::io::Error),
}

impl WgDeleteOutcome {
    /// Whether the original user-visible workgroup path is gone (#1063, plan
    /// section 5.4). `true` for `Deleted` and `Partial` (the atomic rename moved
    /// the original path away, so its replica config rows are no longer active
    /// even when hidden-orphan cleanup failed); `false` for `Blocked`/`Other`,
    /// which leave the path intact and must preserve rows.
    pub(crate) fn logical_path_removed(&self) -> bool {
        matches!(
            self,
            WgDeleteOutcome::Deleted | WgDeleteOutcome::Partial { .. }
        )
    }
}

/// Atomically detect blockers before deleting a workgroup directory.
///
/// Strategy: rename the WG dir to a unique sentinel name in the same parent
/// (NTFS metadata-only operation, fails atomically if any handle blocks it),
/// then `remove_dir_all` the renamed dir. If rename fails with a file-in-use
/// error the WG is still intact, so the caller can run the diagnostic over the
/// original tree and surface a `BLOCKERS:` report.
///
/// Suffix scheme: `.deleting-<wg_name>-<uuid>` — leading `.` keeps any orphan
/// (rare race: rename succeeds but remove_dir_all fails) invisible to the
/// `entity_prefix::has_entity_prefix` filters in `ac_discovery` and
/// `cli::list_peers`, so an orphan won't surface as a ghost room. UUID is
/// used (already in `Cargo.toml`) to guarantee uniqueness across rapid retries.
///
/// `pub(crate)` so unit tests can drive it directly.
pub(crate) fn try_atomic_delete_wg(wg_dir: &Path) -> WgDeleteOutcome {
    try_atomic_delete_wg_with_remove(wg_dir, |path| std::fs::remove_dir_all(path))
}

pub(crate) fn try_atomic_delete_wg_with_remove(
    wg_dir: &Path,
    remove_dir_all: impl FnOnce(&Path) -> std::io::Result<()>,
) -> WgDeleteOutcome {
    let parent = match wg_dir.parent() {
        Some(p) => p,
        None => {
            return WgDeleteOutcome::Other(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "room directory has no parent",
            ));
        }
    };
    let original_name = match wg_dir.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => {
            return WgDeleteOutcome::Other(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "room directory has no filename",
            ));
        }
    };
    let temp_name = format!(".deleting-{}-{}", original_name, uuid::Uuid::new_v4());
    let temp_path = parent.join(&temp_name);

    match std::fs::rename(wg_dir, &temp_path) {
        Ok(()) => {
            if let Err(e) = remove_dir_all(&temp_path) {
                log::warn!(
                    "[entity_creation] Renamed workgroup '{}' to '{}' but remove_dir_all \
                     failed: {}. \
                     User-visible WG is gone; orphan remains on disk.",
                    wg_dir.display(),
                    temp_path.display(),
                    e
                );
                return WgDeleteOutcome::Partial {
                    orphan_path: temp_path,
                    error: e,
                };
            }
            WgDeleteOutcome::Deleted
        }
        Err(e) => {
            if is_rename_blocked_by_handle(&e) {
                WgDeleteOutcome::Blocked(e)
            } else {
                WgDeleteOutcome::Other(e)
            }
        }
    }
}

#[derive(Debug)]
enum AgentDeleteStageError {
    Blocked {
        target: AgentDeleteTarget,
        raw_error: String,
    },
    Other(String),
    RollbackFailed(String),
}

#[cfg(test)]
async fn stage_agent_delete_targets(
    plan: &AgentDeletePlan,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<Vec<StagedAgentDeleteTarget>, String> {
    match stage_agent_delete_targets_with_rename(
        plan,
        |from: &Path, to: &Path| std::fs::rename(from, to),
        session_mgr,
    ) {
        Ok(staged) => Ok(staged),
        Err(AgentDeleteStageError::Blocked { target, raw_error }) => {
            log::info!(
                "[entity_creation] delete_agent_matrix: file-in-use detected for '{}' on rename \
                 probe",
                target.label
            );
            let report = crate::commands::wg_delete_diagnostic::diagnose_delete_root_blockers(
                &target.original_path,
                &target.label,
                &raw_error,
                session_mgr,
            )
            .await;
            let json = serde_json::to_string(&report).map_err(|se| {
                format!(
                    "Failed to serialize blocker report: {}; original error: {}",
                    se, raw_error
                )
            })?;
            Err(format!("BLOCKERS:{}", json))
        }
        Err(AgentDeleteStageError::Other(e)) | Err(AgentDeleteStageError::RollbackFailed(e)) => {
            Err(e)
        }
    }
}

fn stage_agent_delete_targets_with_rename<R>(
    plan: &AgentDeletePlan,
    mut rename: R,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<Vec<StagedAgentDeleteTarget>, AgentDeleteStageError>
where
    R: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    let mut staged = Vec::new();
    for target in &plan.targets {
        let parent = target.original_path.parent().ok_or_else(|| {
            AgentDeleteStageError::Other(format!(
                "Agent delete target '{}' has no parent",
                target.original_path.display()
            ))
        })?;
        let original_name = target
            .original_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                AgentDeleteStageError::Other(format!(
                    "Agent delete target '{}' has no filename",
                    target.original_path.display()
                ))
            })?;
        let staged_path = parent.join(format!(
            ".deleting-{}-{}",
            original_name,
            uuid::Uuid::new_v4()
        ));

        match rename(&target.original_path, &staged_path) {
            Ok(()) => staged.push(StagedAgentDeleteTarget {
                original_path: target.original_path.clone(),
                original_key: target.original_key.clone(),
                staged_path,
                label: target.label.clone(),
            }),
            Err(e) => {
                let raw_error = e.to_string();
                let blocked = is_rename_blocked_by_handle(&e);
                let rollback = rollback_staged_agent_delete_targets_with_rename(&staged, rename);
                if let Err(rollback_err) = rollback {
                    return Err(AgentDeleteStageError::RollbackFailed(
                        format_stage_rollback_failed(&staged, &raw_error, rollback_err),
                    ));
                }
                if blocked {
                    return Err(AgentDeleteStageError::Blocked {
                        target: target.clone(),
                        raw_error,
                    });
                }
                return Err(AgentDeleteStageError::Other(format!(
                    "Failed to stage agent delete target '{}': {}",
                    target.original_path.display(),
                    raw_error
                )));
            }
        }
    }
    Ok(staged)
}

#[cfg(test)]
fn rollback_staged_agent_delete_targets(staged: &[StagedAgentDeleteTarget]) -> Result<(), String> {
    rollback_staged_agent_delete_targets_with_rename(staged, |from: &Path, to: &Path| {
        std::fs::rename(from, to)
    })
}

fn rollback_staged_agent_delete_targets_with_rename<R>(
    staged: &[StagedAgentDeleteTarget],
    mut rename: R,
) -> Result<(), String>
where
    R: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    let mut errors = Vec::new();
    for target in staged.iter().rev() {
        if let Err(e) = rename(&target.staged_path, &target.original_path) {
            errors.push(format!(
                "{} [{}] {} -> {}: {}",
                target.label,
                target.original_key,
                target.staged_path.display(),
                target.original_path.display(),
                e
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Hidden cleanup dirs remain after rollback failure: {}",
            errors.join("; ")
        ))
    }
}

fn remove_staged_agent_delete_targets(staged: &[StagedAgentDeleteTarget]) -> Result<(), String> {
    let mut errors = Vec::new();
    for target in staged {
        if let Err(e) = std::fs::remove_dir_all(&target.staged_path) {
            errors.push(format!(
                "{} [{}] {}: {}",
                target.label,
                target.original_key,
                target.staged_path.display(),
                e
            ));
        } else {
            log::debug!(
                "[entity_creation] Removed staged agent delete target '{}' ({})",
                target.label,
                target.original_key
            );
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Agent was removed, but hidden cleanup dir(s) remain: {}",
            errors.join("; ")
        ))
    }
}

fn format_stage_rollback_failed(
    staged: &[StagedAgentDeleteTarget],
    original_error: &str,
    rollback_error: String,
) -> String {
    let hidden_paths = staged
        .iter()
        .map(|target| target.staged_path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Agent delete staging failed and rollback also failed. Hidden cleanup dirs remain: {}. \
         Original error: {}. Rollback error: {}",
        hidden_paths, original_error, rollback_error
    )
}

pub(crate) fn validate_delete_root_not_link_or_reparse(path: &Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("delete_root_not_directory".to_string());
        }
        Err(e) => return Err(format!("delete_root_metadata_failed: {e}")),
    };

    if delete_root_has_windows_reparse_point(&metadata) {
        return Err("delete_root_is_reparse_point".to_string());
    }
    if metadata_is_link_or_reparse(&metadata) {
        return Err("delete_root_is_symlink".to_string());
    }
    if !metadata.is_dir() {
        return Err("delete_root_not_directory".to_string());
    }
    Ok(())
}

async fn ensure_no_live_sessions_under_manager(
    root: &Path,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Result<(), String> {
    let root_key = path_key_for_delete(root);
    let manager = { session_mgr.read().await.clone() };
    let sessions = manager.list_sessions().await;
    let blockers: Vec<_> = sessions
        .into_iter()
        .filter(|session| !matches!(session.status, SessionStatus::Exited(_)))
        .filter(|session| {
            let working_dir = Path::new(&session.working_directory);
            let working_key = path_key_for_delete(working_dir);
            working_key == root_key || working_key.starts_with(&(root_key.clone() + "/"))
        })
        .collect();

    if blockers.is_empty() {
        return Ok(());
    }

    let details = blockers
        .iter()
        .map(|session| {
            format!(
                "  - {} at {} ({:?})",
                session.name, session.working_directory, session.status
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!(
        "Cannot delete while live sessions exist under {}:\n{}",
        root.display(),
        details
    ))
}

fn path_key_for_delete(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    path_key_for_delete_without_canonicalize(&resolved)
}

fn path_key_for_delete_without_canonicalize(path: &Path) -> String {
    let text = crate::path_utils::path_to_string_without_windows_verbatim_prefix(path);
    text.replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn target_keys_for_delete(path: &Path) -> Vec<String> {
    let canonical_key = path_key_for_delete(path);
    let raw_key = path_key_for_delete_without_canonicalize(path);
    if canonical_key == raw_key {
        vec![canonical_key]
    } else {
        vec![canonical_key, raw_key]
    }
}

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

fn metadata_parts_are_link_or_reparse(is_symlink: bool, file_attributes: u32) -> bool {
    is_symlink || file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

pub(crate) fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        metadata_parts_are_link_or_reparse(
            metadata.file_type().is_symlink(),
            metadata.file_attributes(),
        )
    }
    #[cfg(not(target_os = "windows"))]
    {
        metadata_parts_are_link_or_reparse(metadata.file_type().is_symlink(), 0)
    }
}

#[cfg(target_os = "windows")]
fn delete_root_has_windows_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata_parts_are_link_or_reparse(false, metadata.file_attributes())
}

#[cfg(not(target_os = "windows"))]
fn delete_root_has_windows_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// True iff the rename-probe error indicates a blocker holds an open handle.
///
/// Superset of `is_file_in_use_error`: matches the same {32, 33, 1224} codes
/// PLUS `ERROR_ACCESS_DENIED` (5). Empirically `MoveFileEx` (and therefore
/// `std::fs::rename`) returns 5, not 32, when an existing open handle on the
/// source's descendant lacks `FILE_SHARE_DELETE` — the most common real-world
/// blocker shape (default-share opens by IDEs and terminals). This is the
/// rename-path counterpart to `is_file_in_use_error`, which was tuned for
/// `remove_dir_all` semantics where ACCESS_DENIED typically means a real
/// permission failure (read-only file) rather than a share-mode mismatch.
///
/// `pub(crate)` so unit tests can exercise it without going through `try_atomic_delete_wg`.
pub(crate) fn is_rename_blocked_by_handle(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        const ERROR_ACCESS_DENIED: i32 = 5;
        if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) {
            return true;
        }
        is_file_in_use_error(e)
    }
    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}

/// True iff `e` represents a Windows "file in use" error.
///
/// Matches the Win32 codes that surface when another process holds an open or
/// memory-mapped handle to a file we tried to delete:
/// - `ERROR_SHARING_VIOLATION` (32) — standard open with a deny-share mode.
/// - `ERROR_LOCK_VIOLATION` (33) — byte-range lock collision.
/// - `ERROR_USER_MAPPED_FILE` (1224) — file is mapped into another process's address
///   space. This is the VSCode / IDE memory-mapped-I/O case and was the motivating
///   real-world scenario for the blocker diagnostic. See plan §6.1.
///
/// On non-Windows always returns false: Linux / macOS produce different error codes
/// for "directory not empty due to open file" and we don't run the Restart-Manager
/// diagnostic there.
///
/// `pub(crate)` so the unit test in `wg_delete_diagnostic::tests` can exercise it
/// without moving the test into this module.
#[cfg(any(windows, test))]
pub(crate) fn is_file_in_use_error(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        const ERROR_SHARING_VIOLATION: i32 = 32;
        const ERROR_LOCK_VIOLATION: i32 = 33;
        const ERROR_USER_MAPPED_FILE: i32 = 1224;
        matches!(
            e.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION | ERROR_USER_MAPPED_FILE)
        )
    }
    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}

/// Scan the selected Project AC Root for existing `room-<N>-{team_name}/` dirs and return the
/// **lowest free positive integer** starting at 1.
///
/// #1614: legacy `wg-*` directories are deliberately NOT counted, because the
/// Room and legacy Workgroup slot namespaces are independent; in a root that
/// already holds `wg-1-{team}` the next Room is `room-1-{team}`.
///
/// Issue #177: previously this returned `max(existing) + 1`, which left
/// permanent gaps after a room was destroyed. The new policy reuses
/// any freed numbers so the user-facing sequence stays compact.
///
/// Filtering rules:
/// - Only directories are considered (regular files are ignored).
/// - The directory name must match `room-<positive digits>-<team>`.
/// - Team suffix is ignored for allocation, so room numbers are unique
///   across the whole project.
///
/// Slot 1 is always reachable because the lowest-free search starts at
/// 1 (see the `find` call below); a stray `room-0-{team}` directory ends
/// up in `taken` but is never tested by `find` and so cannot displace
/// slot 1.
///
/// Read-error degradation: if `std::fs::read_dir(ac_root)` fails
/// (permission denied, transient I/O, broken junction, path-too-long
/// on Windows), the function returns `1` as a graceful fallback. The
/// post-allocate `wg_dir.exists()` guard in `create_workgroup` will
/// surface the real condition as an "already exists" error if a
/// `room-1-{team}` is in fact present; otherwise the slot-1 creation
/// succeeds with stale state. Surfacing the read error is tracked
/// separately and is out of scope for #177.
/// #2507 Pure seam: mask -> pad width, applying the fallback.
pub(crate) fn pad_width_for_mask(mask: &str) -> usize {
    crate::config::settings::validate_room_number_mask(mask)
        .unwrap_or_else(|_| crate::config::settings::default_room_number_mask().len())
}

/// #2507 The ONLY place a room directory name is built from a number.
pub(crate) fn room_dir_name(wg_number: u32, suffix: &str, width: usize) -> String {
    use crate::config::entity_prefix::ROOM_DIR_PREFIX;
    format!("{ROOM_DIR_PREFIX}{wg_number:0width$}-{suffix}")
}

/// #2507 Pad width for a NEW room number, from persisted settings.
pub(crate) fn room_number_pad_width() -> usize {
    pad_width_for_mask(&crate::config::settings::load_settings_for_cli().room_number_mask)
}

pub(crate) fn determine_next_wg_number(ac_root: &Path) -> u32 {
    let mut taken: HashSet<u32> = HashSet::new();

    if let Ok(entries) = std::fs::read_dir(ac_root) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(rest) = name_str.strip_prefix(crate::config::entity_prefix::ROOM_DIR_PREFIX)
            {
                if let Some((middle, team)) = rest.split_once('-') {
                    if team.is_empty() {
                        continue;
                    }
                    if let Ok(n) = middle.parse::<u32>() {
                        taken.insert(n);
                    }
                }
            }
        }
    }

    // Lowest free positive integer ≥ 1. The bounded `..=u32::MAX` form avoids
    // any iterator-overflow footgun in debug builds; `find` short-circuits at
    // the first miss so the actual cost is O(taken.len() + 1) in practice.
    // A `0` may end up in `taken` (from a stray `wg-0-{team}`) but is never
    // tested here — the search starts at 1, so slot 1 is always reachable.
    (1u32..=u32::MAX).find(|n| !taken.contains(n)).unwrap_or(1)
}

/// Async git clone with CREATE_NO_WINDOW on Windows.
async fn git_clone_async(url: &str, target: &Path) -> Result<(), String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const GIT_CLONE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
    const GIT_RESET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    let git_target = git_cli_path(target);
    let mut cmd = tokio::process::Command::new("git");
    crate::pty::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut cmd);
    cmd.args(["-c", "core.longpaths=true", "clone", "--depth", "1", url])
        .arg(git_target.as_os_str());
    cmd.kill_on_drop(true);

    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = tokio::time::timeout(GIT_CLONE_TIMEOUT, cmd.output())
        .await
        .map_err(|_| {
            format!(
                "git clone timed out after {} seconds",
                GIT_CLONE_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("Failed to spawn git clone: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        // Cap error message length to avoid sending huge progress output to frontend
        let capped = if trimmed.len() > 512 {
            &trimmed[..512]
        } else {
            trimmed
        };
        return Err(format!("git clone failed: {}", capped));
    }

    if !target.join(".git").join("index").exists() {
        log::warn!(
            "[entity_creation] .git/index missing after clone for {}, running fallback git reset",
            url
        );
        let mut reset_cmd = tokio::process::Command::new("git");
        crate::pty::credentials::scrub_credentials_from_tokio_command(&mut reset_cmd);
        crate::config::agent_path::apply_search_path_to_tokio_command(&mut reset_cmd);
        reset_cmd.args(["reset"]).current_dir(&git_target);
        reset_cmd.kill_on_drop(true);
        #[cfg(windows)]
        {
            reset_cmd.creation_flags(CREATE_NO_WINDOW);
        }
        match tokio::time::timeout(GIT_RESET_TIMEOUT, reset_cmd.output()).await {
            Ok(Ok(output)) if output.status.success() => {}
            Ok(Ok(output)) => {
                log::warn!(
                    "[entity_creation] fallback git reset failed for {}: {}",
                    url,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Ok(Err(e)) => {
                log::warn!(
                    "[entity_creation] failed to spawn fallback git reset for {}: {}",
                    url,
                    e
                );
            }
            Err(_) => {
                log::warn!(
                    "[entity_creation] fallback git reset timed out after {} seconds for {}",
                    GIT_RESET_TIMEOUT.as_secs(),
                    url
                );
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
fn git_cli_path(path: &Path) -> PathBuf {
    crate::path_utils::normalize_windows_verbatim_path_buf(path)
}

#[cfg(not(windows))]
fn git_cli_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(test)]
mod tests;
