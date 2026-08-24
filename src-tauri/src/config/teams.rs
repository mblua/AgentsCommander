use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::config::replica_identity::{
    agent_bare_name_from_ref, read_and_repair_wg_replica_config, WG_REPLICA_REQUIRED_CONTEXT,
};
use crate::config::ac_root::{existing_ac_root, find_ac_root_segment, has_ac_root};

/// #280 §3.4 — record whether the missing-config one-shot INFO has already
/// fired for a given `(project, team_dir)` pair this process. Returns
/// `true` on the first call for that pair, `false` thereafter. Resets on
/// process restart. Process-local because the dedup is per-instance.
fn note_missing_team_config(project: &str, team_dir: &str) -> bool {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    let m = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    g.insert((project.to_string(), team_dir.to_string()))
}

/// A team discovered from `_team_*/config.json` in Project AC Root directories.
#[derive(Debug, Clone)]
pub struct DiscoveredTeam {
    pub name: String,
    /// Project folder this team was discovered in (dir name, not path). Forms
    /// the left-hand side of the canonical FQN for WG replicas matched to this
    /// team, and gates cross-project leakage in WG-aware membership checks.
    pub project: String,
    /// Agent display names in "project/agent" format (from resolve_agent_ref).
    /// Index-aligned with `agent_paths` — both vecs always have the same length.
    pub agent_names: Vec<String>,
    /// Absolute paths to agent directories (resolved from team config refs).
    /// `None` entries mean the directory was not found on disk.
    pub agent_paths: Vec<Option<PathBuf>>,
    /// Coordinator display name
    pub coordinator_name: Option<String>,
    /// Absolute path to coordinator directory
    pub coordinator_path: Option<PathBuf>,
}

/// Derive agent name (parent/folder) from a path, stripping `__agent_`/`_agent_` prefixes.
pub fn agent_name_from_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let components: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if components.len() >= 2 {
        let parent = components[components.len() - 2];
        let last = components[components.len() - 1];
        let stripped = last
            .strip_prefix("__agent_")
            .or_else(|| last.strip_prefix("_agent_"))
            .unwrap_or(last);
        format!("{}/{}", parent, stripped)
    } else {
        normalized
    }
}

/// Split a possibly-qualified agent name into (project, local) parts.
/// Returns `(None, name)` when no `:` separator is present (backward-compat path).
pub fn split_project_prefix(name: &str) -> (Option<&str>, &str) {
    match name.split_once(':') {
        Some((proj, local)) if !proj.is_empty() && !local.is_empty() => (Some(proj), local),
        _ => (None, name),
    }
}

/// Derive the fully-qualified agent name from a CWD.
///
/// - WG replica CWD `<...>/<project>/.ac/wg-N-team/__agent_alice[/...]`
///   → `<project>:wg-N-team/alice`
/// - Non-WG CWD `<...>/<project>/<agent>`
///   → `<project>/<agent>` (unchanged from `agent_name_from_path`)
///
/// Uses `rposition` so a pathological path containing an earlier workspace
/// segment (e.g. `C:/.ac/repos/proj/.ac/wg-1-devs/__agent_x`) anchors
/// on the right-most occurrence — the identity anchor. Subdirectories inside
/// a replica (`.ac/wg-1-devs/__agent_alice/some/deep`) resolve to the
/// owning replica's FQN, consistent with "alice owns her subdirs".
pub fn agent_fqn_from_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();

    if let Some(ac_idx) = find_ac_root_segment(&parts) {
        if ac_idx > 0 && ac_idx + 2 < parts.len() {
            let project = parts[ac_idx - 1];
            let wg = parts[ac_idx + 1];
            let agent_dir = parts[ac_idx + 2];
            if wg.starts_with("wg-") && agent_dir.starts_with("__agent_") {
                let agent = agent_dir.strip_prefix("__agent_").unwrap_or(agent_dir);
                return format!("{}:{}/{}", project, wg, agent);
            }
        }
    }

    agent_name_from_path(path)
}

/// Derive the `(workgroup, agent)` identity from a CWD for the Resource
/// Monitor's human-readable agent-group label (#516).
///
/// - WG replica CWD `<...>/<project>/.ac/wg-N-team/__agent_alice[/...]`
///   becomes `(Some("wg-N-team"), Some("alice"))`.
/// - Any other shape (origin agent, root agent, ad-hoc shell, unparseable)
///   becomes `(None, None)`.
///
/// Anchors on the right-most `.ac` workspace segment via `find_ac_root_segment`,
/// identical to `agent_fqn_from_path`, so subdirectories inside a replica resolve
/// to the owning replica's identity. The workgroup is the bare `wg-N-team` segment
/// (not project-prefixed), and the agent is the replica dir with `__agent_` stripped.
/// The root-agent label fallback is applied by the caller (which knows
/// `is_root_agent`); this returns only the raw pair and never panics.
pub fn workgroup_and_agent_from_path(path: &str) -> (Option<String>, Option<String>) {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();

    if let Some(ac_idx) = find_ac_root_segment(&parts) {
        if ac_idx + 2 < parts.len() {
            let wg = parts[ac_idx + 1];
            let agent_dir = parts[ac_idx + 2];
            if wg.starts_with("wg-") && agent_dir.starts_with("__agent_") {
                let agent = agent_dir.strip_prefix("__agent_").unwrap_or(agent_dir);
                return (Some(wg.to_string()), Some(agent.to_string()));
            }
        }
    }

    (None, None)
}

/// Derive the project folder name from a CWD: the directory immediately
/// preceding the `.ac` workspace segment. Mirrors the project anchor used by
/// `agent_fqn_from_path` (#566). Returns `None` when there is no workspace
/// segment (origin agents, ad-hoc shells) or `.ac` is the first segment.
///
/// Deliberately more permissive than `workgroup_and_agent_from_path`: it
/// returns `Some(project)` for ANY cwd with a `.ac` segment that has a parent,
/// so root agents (`<proj>/.ac/ac-root-agent`) and origin/matrix agents
/// (`<proj>/.ac/_agent_x`) also carry a project even when their wg/agent are
/// `None`. Only a cwd with no `.ac` segment at all yields `None`.
pub fn project_from_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    let ac_idx = find_ac_root_segment(&parts)?;
    (ac_idx > 0).then(|| parts[ac_idx - 1].to_string())
}

// ── FQN resolution (shared between CLI and mailbox — §AR2-shared) ──

/// Error type for `resolve_agent_target`. Each variant carries the data needed to
/// produce an actionable user message via `thiserror::Display`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResolutionError {
    /// Target string is neither FQN (contains `:`), nor a WG-local-form
    /// (`wg-N-team/agent`), nor a bare agent name. Examples: empty, contains
    /// path separators, has >1 colons, qualified but RHS has wrong shape.
    #[error("target '{0}' is not a valid agent name shape")]
    InvalidShape(String),

    /// Target is fully qualified (`proj:wg-N/agent`) but no matching replica
    /// exists on disk under the `project_paths` scan.
    #[error("target '{0}' is qualified but not found in any known project")]
    UnknownQualified(String),

    /// Target is unqualified (WG-local) and scan found zero matching replicas.
    #[error("target '{0}' not found in any known project")]
    NoMatch(String),

    /// Target is unqualified and matches >1 replica across projects. Candidates
    /// are FQN so the user can re-issue the command with a project-qualified form.
    #[error("target '{target}' is ambiguous; candidates: {}", candidates.join(", "))]
    Ambiguous {
        target: String,
        candidates: Vec<String>,
    },

    /// Target's agent segment starts with the on-disk replica/matrix prefix
    /// (`__agent_` or `_agent_`) — i.e. the caller passed a filesystem directory
    /// name instead of a peer FQN. Common Codex-agent fallback bug when
    /// `list-peers` returned empty (Issue #134).
    #[error(
        "target '{0}' looks like a filesystem directory name, not a peer FQN. \
         Use the 'name' field from `list-peers-lean` (e.g. 'project:wg-N-team/agent' \
         for WG replicas, 'project/agent' for origin agents). Directory names \
         like '__agent_*' or '_agent_*' are NEVER valid --to values."
    )]
    LooksLikeFilesystemDir(String),
}

/// Does the agent segment of `target` start with the on-disk replica/matrix
/// prefix? Checks the right-hand-most `/`-delimited segment after stripping
/// an optional `proj:` prefix — catches `__agent_x`, `proj/__agent_x`,
/// `wg-1-team/__agent_x`, and `proj:wg-1-team/__agent_x` alike.
fn agent_segment_is_filesystem_dir(target: &str) -> bool {
    let after_colon = target.split_once(':').map(|(_, l)| l).unwrap_or(target);
    let agent = after_colon
        .rsplit_once('/')
        .map(|(_, a)| a)
        .unwrap_or(after_colon);
    agent.starts_with("__agent_") || agent.starts_with("_agent_")
}

/// Validate that a qualified target's right-hand side is shaped
/// `wg-<digits>-<team>/<agent>` (§G2-7 optional hardening). Returns true on match.
fn is_valid_wg_local_shape(local: &str) -> bool {
    let Some((prefix, agent)) = local.split_once('/') else {
        return false;
    };
    if agent.is_empty() {
        return false;
    }
    let Some(rest) = prefix.strip_prefix("wg-") else {
        return false;
    };
    let Some((digits, team)) = rest.split_once('-') else {
        return false;
    };
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && !team.is_empty()
}

/// Enumerate project folders reachable from `project_paths`, mirroring the
/// base-plus-immediate-non-dot-children scan in `discover_teams_in_project`.
/// Returns `(project_folder_name, project_dir_path)` pairs.
fn enumerate_project_dirs(project_paths: &[String]) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for rp in project_paths {
        let base = Path::new(rp);
        if !base.is_dir() {
            continue;
        }

        // Include base itself if it contains a Project AC Root.
        if has_ac_root(base) {
            if let Some(name) = base.file_name().and_then(|n| n.to_str()) {
                out.push((name.to_string(), base.to_path_buf()));
            }
        }

        // Plus immediate non-dot children that contain a Project AC Root.
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let p = entry.path();
                if !p.is_dir() {
                    continue;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with('.') {
                    continue;
                }
                if has_ac_root(&p) {
                    out.push((name.to_string(), p));
                }
            }
        }
    }
    out
}

fn strict_project_ac_root(project: &Path) -> Result<Option<PathBuf>, String> {
    let ac_root = project.join(crate::config::ac_root::CANONICAL_AC_ROOT_DIR);
    if !ac_root
        .try_exists()
        .map_err(|_| "unsafe_path".to_string())?
    {
        return Ok(None);
    }
    let identity = crate::path_identity::verify_directory(&ac_root)?;
    if identity
        .canonical_path
        .file_name()
        .and_then(|value| value.to_str())
        != Some(crate::config::ac_root::CANONICAL_AC_ROOT_DIR)
    {
        return Err("unsafe_path".to_string());
    }
    Ok(Some(ac_root))
}

fn enumerate_project_dirs_strict(
    project_paths: &[String],
) -> Result<Vec<(String, PathBuf)>, String> {
    if project_paths.len() > 1_024 {
        return Err("invalid_target".to_string());
    }
    let mut out = Vec::new();
    let mut scanned_entries = 0usize;
    for configured in project_paths {
        if configured.is_empty() || configured.contains('\0') {
            return Err("unsafe_path".to_string());
        }
        let base_identity = crate::path_identity::verify_directory(Path::new(configured))?;
        let base = base_identity.canonical_path;
        if strict_project_ac_root(&base)?.is_some() {
            let name = base
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| "unsafe_path".to_string())?;
            out.push((name.to_string(), base.clone()));
        }
        let entries = std::fs::read_dir(&base).map_err(|_| "unsafe_path".to_string())?;
        for entry in entries {
            let entry = entry.map_err(|_| "unsafe_path".to_string())?;
            scanned_entries = scanned_entries.saturating_add(1);
            if scanned_entries > 1_024 {
                return Err("invalid_target".to_string());
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "unsafe_path".to_string())?;
            if name.starts_with('.') {
                continue;
            }
            let metadata = entry.metadata().map_err(|_| "unsafe_path".to_string())?;
            if !metadata.is_dir() {
                continue;
            }
            let child_identity = crate::path_identity::verify_directory(&entry.path())?;
            if strict_project_ac_root(&child_identity.canonical_path)?.is_some() {
                out.push((name, child_identity.canonical_path));
                if out.len() > 1_024 {
                    return Err("invalid_target".to_string());
                }
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgCoordinatorReplica {
    pub project: String,
    pub team: String,
    pub wg_name: String,
    pub agent_name: String,
    pub replica_dir: PathBuf,
}

/// Normalize a path string to a stable comparison form.
///
/// - Strip the Windows extended-length (`\\?\`) prefix.
/// - Replace `\` with `/`.
/// - Lowercase (case-insensitive comparison; matches the convention used by
///   `cli/list_peers::norm_path`).
/// - Trim trailing `/`.
#[cfg(test)]
fn normalize_path_for_compare(s: &str) -> String {
    let stripped = s.strip_prefix(r"\\?\").unwrap_or(s);
    stripped
        .replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string()
}

/// Logical resolution of `.` and `..` components in a path string (no disk
/// access). Used as a fallback when `std::fs::canonicalize()` fails because
/// the target doesn't exist — e.g., legacy identity refs that point to a
/// previous workspace folder name (see #299).
///
/// Returns a forward-slash string. `..` components above the root of an
/// absolute path are dropped; `..` components at the front of a purely
/// relative path are preserved.
#[cfg(test)]
fn logical_path_resolve(path: &Path) -> String {
    use std::path::Component;
    let mut components: Vec<Component> = Vec::new();
    let mut rooted = false;
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                rooted = true;
                components.push(component);
            }
            Component::Normal(_) => {
                components.push(component);
            }
            Component::ParentDir => {
                if matches!(components.last(), Some(Component::Normal(_))) {
                    components.pop();
                } else if !rooted {
                    components.push(component);
                }
                // else: rooted with no Normal segments to pop → drop the `..`.
            }
            Component::CurDir => {}
        }
    }
    let mut out = String::new();
    for c in &components {
        match c {
            Component::Prefix(p) => {
                out.push_str(&p.as_os_str().to_string_lossy());
            }
            Component::RootDir => {
                out.push('/');
            }
            Component::Normal(s) => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(&s.to_string_lossy());
            }
            Component::ParentDir => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str("..");
            }
            Component::CurDir => {}
        }
    }
    out
}

/// Stable comparison key for an identity ref path.
///
/// 1. Prefers `std::fs::canonicalize()` — same behavior as the legacy
///    `canonical_path_string` when the target exists on disk.
/// 2. Falls back to pure logical resolution when canonicalize fails (the
///    typical post-workspace-rename state, where both the team coordinator
///    ref and the replica identity ref point to a folder that no longer
///    exists — see #299).
///
/// Both branches feed `normalize_path_for_compare`, so two refs pointing at
/// the same conceptual location produce equal keys whether or not the
/// target exists on disk.
///
/// Identity-based authorization is preserved: the comparison uses only the
/// declared `identity` field from `config.json`, never the replica directory
/// name (`__agent_*`). A spoofed replica that declares a different matrix
/// path still produces a different key from the team coordinator ref.
#[cfg(test)]
fn identity_compare_key(path: &Path) -> String {
    let raw = match std::fs::canonicalize(path) {
        Ok(canon) => canon.to_string_lossy().into_owned(),
        Err(_) => logical_path_resolve(path),
    };
    normalize_path_for_compare(&raw)
}

pub fn resolve_wg_coordinator_replica(
    ac_root: &Path,
    wg_dir: &Path,
) -> Option<WgCoordinatorReplica> {
    let project = ac_root.parent()?.file_name()?.to_str()?.to_string();
    let wg_name = wg_dir.file_name()?.to_str()?.to_string();
    let team = wg_name
        .strip_prefix("wg-")
        .and_then(|s| s.split_once('-').map(|(_, rest)| rest.to_string()))?;

    let team_dir = ac_root.join(format!("_team_{}", team));
    let team_config: serde_json::Value = std::fs::read_to_string(team_dir.join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())?;
    let coordinator_ref = team_config.get("coordinator").and_then(|c| c.as_str())?;
    let coordinator_name = agent_bare_name_from_ref(coordinator_ref).ok()?;
    if !ac_root
        .join(format!("_agent_{}", coordinator_name))
        .is_dir()
    {
        return None;
    }

    for replica_entry in std::fs::read_dir(wg_dir).ok()?.flatten() {
        let replica_dir = replica_entry.path();
        if !replica_dir.is_dir() {
            continue;
        }
        let _dir_name = match replica_dir.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.starts_with("__agent_") => n,
            _ => continue,
        };
        let Ok((_config, identity)) =
            read_and_repair_wg_replica_config(&replica_dir, WG_REPLICA_REQUIRED_CONTEXT)
        else {
            continue;
        };
        if identity.agent_name == coordinator_name {
            return Some(WgCoordinatorReplica {
                project,
                team,
                wg_name,
                agent_name: identity.agent_name,
                replica_dir,
            });
        }
    }

    None
}

pub fn verified_wg_coordinator_target(
    target: &str,
    project_paths: &[String],
) -> Option<WgCoordinatorReplica> {
    let (Some(project), local) = split_project_prefix(target) else {
        return None;
    };
    if !is_valid_wg_local_shape(local) {
        return None;
    }
    let (wg_name, agent_name) = local.split_once('/')?;

    for (project_name, project_dir) in enumerate_project_dirs(project_paths) {
        if project_name != project {
            continue;
        }
        let Some(ac_root) = existing_ac_root(&project_dir) else {
            continue;
        };
        let wg_dir = ac_root.join(wg_name);
        if !wg_dir.is_dir() {
            continue;
        }
        if let Some(resolved) = resolve_wg_coordinator_replica(&ac_root, &wg_dir) {
            if resolved.agent_name == agent_name {
                return Some(resolved);
            }
        }
    }

    None
}

// Privileged PTY-input routing is intentionally separate from broad message
// discovery and `can_communicate`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyInputAuthorityKind {
    Coordinator,
    Root,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPtyInputIdentity {
    pub canonical_fqn: String,
    pub project: String,
    pub workgroup: String,
    pub agent: String,
    pub replica_root: PathBuf,
    pub matrix_root: PathBuf,
    pub is_coordinator: bool,
    pub project_identity: crate::path_identity::VerifiedPathIdentity,
    pub ac_root_identity: crate::path_identity::VerifiedPathIdentity,
    pub workgroup_identity: crate::path_identity::VerifiedPathIdentity,
    pub replica_identity: crate::path_identity::VerifiedPathIdentity,
    pub matrix_identity: crate::path_identity::VerifiedPathIdentity,
    /// Permanent physical sender incarnation. This is derived only from the
    /// verified replica/root directory object and deliberately excludes mutable
    /// config bytes. It keys GET and permanent idempotency history.
    pub incarnation_fingerprint: String,
    /// Mutable authority/config snapshot used only while an operation can still
    /// actuate. Benign or authority-relevant config edits change this value.
    pub authority_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPtyInputRoute {
    pub sender: VerifiedPtyInputIdentity,
    pub target: VerifiedPtyInputIdentity,
    pub kind: PtyInputAuthorityKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalSnapshotAuthorityKind {
    Coordinator,
    Root,
}

#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedTerminalSnapshotRoute {
    pub sender: VerifiedPtyInputIdentity,
    pub target: VerifiedPtyInputIdentity,
    pub kind: TerminalSnapshotAuthorityKind,
}

impl std::fmt::Debug for VerifiedTerminalSnapshotRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedTerminalSnapshotRoute")
            .field("kind", &self.kind)
            .field("sender_is_coordinator", &self.sender.is_coordinator)
            .field("target_is_coordinator", &self.target.is_coordinator)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TerminalSnapshotTargetIdentity {
    pub canonical_fqn: String,
    pub replica_root: PathBuf,
    pub project: String,
    pub workgroup: String,
    pub team: String,
    pub is_coordinator: bool,
}

impl std::fmt::Debug for TerminalSnapshotTargetIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotTargetIdentity")
            .field("is_coordinator", &self.is_coordinator)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StrictPtyFqn {
    project: String,
    workgroup: String,
    team: String,
    agent: String,
}

fn forbidden_identity_scalar(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

fn parse_strict_pty_fqn(value: &str) -> Result<StrictPtyFqn, String> {
    if value.is_empty() || value.len() > 1_024 || value.matches(':').count() != 1 {
        return Err("invalid_target".to_string());
    }
    let (project, local) = value
        .split_once(':')
        .ok_or_else(|| "invalid_target".to_string())?;
    if project.is_empty()
        || matches!(project, "." | "..")
        || project.chars().any(forbidden_identity_scalar)
        || project
            .chars()
            .any(|ch| matches!(ch, '/' | '\\' | '*' | '?' | '"' | '<' | '>' | '|'))
        || local.matches('/').count() != 1
    {
        return Err("invalid_target".to_string());
    }
    let (workgroup, agent) = local
        .split_once('/')
        .ok_or_else(|| "invalid_target".to_string())?;
    let rest = workgroup
        .strip_prefix("wg-")
        .ok_or_else(|| "invalid_target".to_string())?;
    let (digits, team) = rest
        .split_once('-')
        .ok_or_else(|| "invalid_target".to_string())?;
    if digits.is_empty()
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
        || team.is_empty()
        || !team
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || agent.is_empty()
        || !agent
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("invalid_target".to_string());
    }
    Ok(StrictPtyFqn {
        project: project.to_string(),
        workgroup: workgroup.to_string(),
        team: team.to_string(),
        agent: agent.to_string(),
    })
}

pub(crate) fn validate_pty_input_target_syntax(value: &str) -> Result<(), String> {
    let parsed = parse_strict_pty_fqn(value)?;
    let reconstructed = format!("{}:{}/{}", parsed.project, parsed.workgroup, parsed.agent);
    if reconstructed != value {
        return Err("invalid_target".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalSnapshotTargetSyntax {
    Workgroup,
    Origin,
    Root,
}

fn terminal_snapshot_target_syntax(value: &str) -> Result<TerminalSnapshotTargetSyntax, String> {
    if value == crate::config::root_agent::ROOT_AGENT_SENDER {
        return Ok(TerminalSnapshotTargetSyntax::Root);
    }
    if validate_pty_input_target_syntax(value).is_ok() {
        return Ok(TerminalSnapshotTargetSyntax::Workgroup);
    }
    if value.is_empty()
        || value.len() > 1_024
        || value.contains(':')
        || value.matches('/').count() != 1
    {
        return Err("invalid_target".to_string());
    }
    let (project, agent) = value
        .split_once('/')
        .ok_or_else(|| "invalid_target".to_string())?;
    if [project, agent].iter().any(|component| {
        component.is_empty()
            || matches!(*component, "." | "..")
            || component.chars().any(forbidden_identity_scalar)
            || !component
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return Err("invalid_target".to_string());
    }
    Ok(TerminalSnapshotTargetSyntax::Origin)
}

pub(crate) fn validate_terminal_snapshot_target_syntax(value: &str) -> Result<(), String> {
    terminal_snapshot_target_syntax(value).map(|_| ())
}

fn identity_fingerprint(identities: &[&crate::path_identity::VerifiedPathIdentity]) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"ac-pty-input-identity-v1");
    for identity in identities {
        digest.update(identity.object_id.volume.to_be_bytes());
        digest.update(identity.object_id.file.to_be_bytes());
        if let Some(content) = identity.content_sha256 {
            digest.update(content);
        }
    }
    format!("{:x}", digest.finalize())
}

fn incarnation_fingerprint(anchor: &crate::path_identity::VerifiedPathIdentity) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"ac-pty-input-sender-incarnation-v1");
    digest.update(anchor.object_id.volume.to_be_bytes());
    digest.update(anchor.object_id.file.to_be_bytes());
    format!("{:x}", digest.finalize())
}

fn read_identity_json(
    path: &Path,
) -> Result<
    (
        serde_json::Value,
        crate::path_identity::VerifiedPathIdentity,
    ),
    String,
> {
    let (bytes, identity) = crate::path_identity::read_bounded_regular(path, 1024 * 1024)?;
    let value = crate::path_identity::parse_json_no_duplicates(&bytes)?;
    if !value.is_object() {
        return Err("sender_identity_invalid".to_string());
    }
    Ok((value, identity))
}

fn identity_name_eq(left: &str, right: &str) -> bool {
    crate::path_identity::paths_equivalent(Path::new(left), Path::new(right))
}

fn team_members(
    ac_root: &Path,
    team: &str,
) -> Result<
    (
        String,
        Vec<String>,
        crate::path_identity::VerifiedPathIdentity,
    ),
    String,
> {
    let team_dir = ac_root.join(format!("_team_{team}"));
    crate::path_identity::verify_directory(&team_dir)?;
    let (value, config_identity) = read_identity_json(&team_dir.join("config.json"))?;
    let coordinator = value
        .get("coordinator")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "sender_identity_invalid".to_string())
        .and_then(|value| {
            agent_bare_name_from_ref(value).map_err(|_| "sender_identity_invalid".to_string())
        })?;
    let values = value
        .get("agents")
        .and_then(serde_json::Value::as_array)
        .filter(|values| values.len() <= 1_024)
        .ok_or_else(|| "sender_identity_invalid".to_string())?;
    let mut members: Vec<String> = Vec::with_capacity(values.len());
    let mut seen: Vec<String> = Vec::with_capacity(values.len());
    for value in values {
        let raw = value
            .as_str()
            .ok_or_else(|| "sender_identity_invalid".to_string())?;
        let member =
            agent_bare_name_from_ref(raw).map_err(|_| "sender_identity_invalid".to_string())?;
        if seen
            .iter()
            .any(|existing| identity_name_eq(existing, &member))
        {
            return Err("sender_identity_invalid".to_string());
        }
        crate::path_identity::verify_directory(&ac_root.join(format!("_agent_{member}")))?;
        seen.push(member.clone());
        // #1245: the application's team-config writer requires the coordinator
        // to appear in `agents` (commands/entity_creation.rs::
        // normalize_team_config_for_project, and rule 4 of
        // docs/agent-matrix-conventions.md). The coordinator is returned
        // separately and must not also be counted as an ordinary member, so its
        // entry is consumed here instead of rejecting the whole config. `seen`
        // still covers it, so a repeated coordinator entry stays rejected.
        if !identity_name_eq(&member, &coordinator) {
            members.push(member);
        }
    }
    crate::path_identity::verify_directory(&ac_root.join(format!("_agent_{coordinator}")))?;
    Ok((coordinator, members, config_identity))
}

fn verify_replica(
    project_dir: &Path,
    ac_root: &Path,
    parsed: &StrictPtyFqn,
) -> Result<VerifiedPtyInputIdentity, String> {
    let actual_project = project_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "invalid_target".to_string())?;
    if actual_project != parsed.project {
        return Err("invalid_target".to_string());
    }
    let replica_root = ac_root
        .join(&parsed.workgroup)
        .join(format!("__agent_{}", parsed.agent));
    let project_identity = crate::path_identity::verify_directory(project_dir)?;
    let ac_root_identity = crate::path_identity::verify_directory(ac_root)?;
    let workgroup_root = ac_root.join(&parsed.workgroup);
    let workgroup_identity = crate::path_identity::verify_directory(&workgroup_root)?;
    let replica_identity = crate::path_identity::verify_directory(&replica_root)?;
    if workgroup_identity
        .canonical_path
        .file_name()
        .and_then(|value| value.to_str())
        != Some(parsed.workgroup.as_str())
        || replica_identity
            .canonical_path
            .file_name()
            .and_then(|value| value.to_str())
            != Some(format!("__agent_{}", parsed.agent).as_str())
    {
        return Err("invalid_target".to_string());
    }
    let (replica_config, replica_config_identity) =
        read_identity_json(&replica_root.join("config.json"))?;
    // Use the repository's read-only identity reader, now bounded and
    // duplicate-aware, and require it to describe the retained security snapshot.
    let (read_only_config, resolved) =
        crate::config::replica_identity::read_wg_replica_config_read_only(&replica_root)
            .map_err(|_| "sender_identity_invalid".to_string())?;
    let persisted_identity = replica_config
        .get("identity")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "sender_identity_invalid".to_string())?;
    if read_only_config != replica_config
        || resolved.agent_name != parsed.agent
        || persisted_identity != resolved.identity
    {
        return Err("sender_identity_invalid".to_string());
    }
    let matrix_root = ac_root.join(format!("_agent_{}", parsed.agent));
    let matrix_identity = crate::path_identity::verify_directory(&matrix_root)?;
    if matrix_identity
        .canonical_path
        .file_name()
        .and_then(|value| value.to_str())
        != Some(format!("_agent_{}", parsed.agent).as_str())
    {
        return Err("invalid_target".to_string());
    }
    let (coordinator, members, team_config_identity) = team_members(ac_root, &parsed.team)?;
    let is_coordinator = identity_name_eq(&coordinator, &parsed.agent);
    if !is_coordinator
        && !members
            .iter()
            .any(|member| identity_name_eq(member, &parsed.agent))
    {
        return Err("target_not_member".to_string());
    }
    let canonical_fqn = format!("{}:{}/{}", actual_project, parsed.workgroup, parsed.agent);
    let authority_fingerprint = identity_fingerprint(&[
        &project_identity,
        &ac_root_identity,
        &workgroup_identity,
        &replica_identity,
        &replica_config_identity,
        &matrix_identity,
        &team_config_identity,
    ]);
    let incarnation_fingerprint = incarnation_fingerprint(&replica_identity);
    Ok(VerifiedPtyInputIdentity {
        canonical_fqn,
        project: actual_project.to_string(),
        workgroup: parsed.workgroup.clone(),
        agent: parsed.agent.clone(),
        replica_root,
        matrix_root,
        is_coordinator,
        project_identity,
        ac_root_identity,
        workgroup_identity,
        replica_identity,
        incarnation_fingerprint,
        matrix_identity,
        authority_fingerprint,
    })
}

pub(crate) fn strict_wg_replica_anchor_from_cwd(cwd: &Path) -> Result<Option<PathBuf>, String> {
    let cwd_identity = crate::path_identity::verify_directory(cwd)?;
    // #1535: an anchor binds only when it belongs to the project that owns
    // the cwd, i.e. its `wg_replica_layout.ac_root` equals the closest `.ac`
    // ancestor of the cwd. A `__agent_*` replica of an ANCESTOR project is
    // never an anchor for a nested-project cwd, so the ancestor's live
    // session can no longer produce a sessionRace false positive.
    let owning_ac_root = cwd_identity.canonical_path.ancestors().find(|path| {
        path.file_name()
            .and_then(|value| value.to_str())
            .is_some_and(crate::config::ac_root::is_ac_root_name)
    });
    for path in cwd_identity.canonical_path.ancestors() {
        let is_replica = path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with("__agent_"));
        if !is_replica {
            continue;
        }
        // No `.ac` ancestor at all: no project owns the cwd, so no anchor can
        // bind (every valid wg-replica layout requires an `.ac` root).
        let Some(owning_ac_root) = owning_ac_root else {
            return Ok(None);
        };
        if !path.starts_with(owning_ac_root) {
            // The anchor belongs to an ancestor project's `.ac`; it never
            // binds to this cwd. Keep walking: the cwd's own project may
            // still hold a valid anchor above the current path.
            continue;
        }
        let Some(layout) = crate::config::ac_root::wg_replica_layout_from_agent_dir(path)?
        else {
            continue;
        };
        if layout.ac_root != owning_ac_root {
            // Belt-and-braces: the layout's own root must be the owning root.
            // Unreachable for real on-disk layouts (a valid layout under the
            // owning root always reports the owning root), kept as a guard.
            continue;
        }
        return Ok(Some(path.to_path_buf()));
    }
    Ok(None)
}

/// Derive the universal create-gate key from the verified directory layout
/// alone. Ordinary session creation must not depend on team membership,
/// replica config, matrix config, or the privileged SQLite store being healthy.
/// For a target that is eligible for privileged PTY input this reconstructs the
/// exact canonical FQN, so ordinary and privileged creates contend on the same
/// stripe and exact key. Structurally valid legacy layouts that cannot form a
/// privileged FQN still receive a stable physical-replica key.
pub(crate) fn pty_input_create_gate_key_from_cwd(cwd: &Path) -> Result<Option<String>, String> {
    let Some(replica_root) = strict_wg_replica_anchor_from_cwd(cwd)? else {
        return Ok(None);
    };
    let layout = crate::config::ac_root::wg_replica_layout_from_agent_dir(&replica_root)?
        .ok_or_else(|| "target_create_gate_unavailable".to_string())?;
    let project = layout
        .project_dir
        .file_name()
        .and_then(|value| value.to_str());
    if let Some(project) = project {
        let candidate = format!("{}:{}/{}", project, layout.wg_name, layout.agent_name);
        if parse_strict_pty_fqn(&candidate).is_ok() {
            return Ok(Some(candidate));
        }
    }

    let replica = crate::path_identity::verify_directory(&replica_root)?;
    Ok(Some(format!(
        "physical-wg-replica:{:016x}:{:016x}",
        replica.object_id.volume, replica.object_id.file
    )))
}

fn replica_anchor_from_cwd(cwd: &Path) -> Result<PathBuf, String> {
    strict_wg_replica_anchor_from_cwd(cwd)?.ok_or_else(|| "sender_identity_invalid".to_string())
}

fn verify_sender_replica(cwd: &Path) -> Result<VerifiedPtyInputIdentity, String> {
    let replica = replica_anchor_from_cwd(cwd)?;
    let workgroup = replica
        .parent()
        .ok_or_else(|| "sender_identity_invalid".to_string())?;
    let ac_root = workgroup
        .parent()
        .ok_or_else(|| "sender_identity_invalid".to_string())?;
    let project = ac_root
        .parent()
        .ok_or_else(|| "sender_identity_invalid".to_string())?;
    let fqn = agent_fqn_from_path(
        replica
            .to_str()
            .ok_or_else(|| "sender_identity_invalid".to_string())?,
    );
    let parsed = parse_strict_pty_fqn(&fqn)?;
    let identity = verify_replica(project, ac_root, &parsed)?;
    let cwd_identity = crate::path_identity::verify_directory(cwd)?;
    if !crate::path_identity::is_verified_descendant(&cwd_identity, &identity.replica_identity) {
        return Err("sender_identity_invalid".to_string());
    }
    Ok(identity)
}

/// Read-only coordinator proof used while minting a container scope and while
/// revalidating live coordinator authority.
pub(crate) fn verify_pty_input_replica_cwd(
    root: &Path,
) -> Result<VerifiedPtyInputIdentity, String> {
    verify_sender_replica(root)
}

pub fn verify_pty_input_coordinator_root(root: &Path) -> Result<VerifiedPtyInputIdentity, String> {
    let identity = verify_sender_replica(root)?;
    if !identity.is_coordinator {
        return Err("sender_not_coordinator".to_string());
    }
    Ok(identity)
}

fn find_target_identity(
    parsed: &StrictPtyFqn,
    project_paths: &[String],
) -> Result<VerifiedPtyInputIdentity, String> {
    if project_paths.len() > 1_024 {
        return Err("invalid_target".to_string());
    }
    let candidates = enumerate_project_dirs_strict(project_paths)?;
    let mut matching_projects = Vec::new();
    let mut seen_objects = std::collections::HashSet::new();
    for (project_name, project_dir) in candidates {
        if !crate::path_identity::paths_equivalent(
            Path::new(&project_name),
            Path::new(&parsed.project),
        ) {
            continue;
        }
        let identity = crate::path_identity::verify_directory(&project_dir)?;
        if seen_objects.insert(identity.object_id) {
            matching_projects.push((project_name, project_dir));
        }
    }
    if matching_projects.len() != 1 || matching_projects[0].0 != parsed.project {
        return Err("invalid_target".to_string());
    }
    let (_, project_dir) = matching_projects.remove(0);
    let ac_root =
        strict_project_ac_root(&project_dir)?.ok_or_else(|| "invalid_target".to_string())?;
    crate::path_identity::verify_directory(&ac_root.join(&parsed.workgroup))?;
    let identity = verify_replica(&project_dir, &ac_root, parsed)?;
    let reconstructed = format!(
        "{}:{}/{}",
        identity.project, identity.workgroup, identity.agent
    );
    let supplied = format!("{}:{}/{}", parsed.project, parsed.workgroup, parsed.agent);
    if reconstructed != supplied {
        return Err("invalid_target".to_string());
    }
    Ok(identity)
}

fn resolve_verified_wg_target(
    target_fqn: &str,
    project_paths: &[String],
) -> Result<VerifiedPtyInputIdentity, String> {
    let parsed = parse_strict_pty_fqn(target_fqn)?;
    let target = find_target_identity(&parsed, project_paths)?;
    if target.canonical_fqn != target_fqn {
        return Err("invalid_target".to_string());
    }
    Ok(target)
}

pub(crate) fn resolve_pty_input_target(
    target_fqn: &str,
    project_paths: &[String],
) -> Result<VerifiedPtyInputIdentity, String> {
    resolve_verified_wg_target(target_fqn, project_paths)
}

pub(crate) fn discover_verified_terminal_snapshot_targets(
    project_paths: &[String],
) -> Result<Vec<TerminalSnapshotTargetIdentity>, String> {
    const TARGET_CAP: usize = 4_096;
    let projects = enumerate_project_dirs_strict(project_paths)?;
    let mut project_names = std::collections::HashSet::new();
    let mut project_objects = std::collections::HashSet::new();
    for (name, path) in &projects {
        let identity = crate::path_identity::verify_directory(path)?;
        if !project_names.insert(name.clone()) || !project_objects.insert(identity.object_id) {
            return Err("ambiguous_project".to_string());
        }
    }

    let mut targets = Vec::new();
    let mut target_names = std::collections::HashSet::new();
    let mut target_objects = std::collections::HashSet::new();
    let mut scanned_entries = 0usize;
    for (project, project_dir) in projects {
        let ac_root =
            strict_project_ac_root(&project_dir)?.ok_or_else(|| "unsafe_path".to_string())?;
        let workgroups = std::fs::read_dir(&ac_root).map_err(|_| "unsafe_path".to_string())?;
        for workgroup in workgroups {
            let workgroup = workgroup.map_err(|_| "unsafe_path".to_string())?;
            scanned_entries = scanned_entries.saturating_add(1);
            if scanned_entries > TARGET_CAP * 4 {
                return Err("target_limit".to_string());
            }
            let name = workgroup
                .file_name()
                .into_string()
                .map_err(|_| "unsafe_path".to_string())?;
            if !name.starts_with("wg-") {
                continue;
            }
            let workgroup_identity = crate::path_identity::verify_directory(&workgroup.path())?;
            let replicas = std::fs::read_dir(&workgroup_identity.canonical_path)
                .map_err(|_| "unsafe_path".to_string())?;
            for replica in replicas {
                let replica = replica.map_err(|_| "unsafe_path".to_string())?;
                scanned_entries = scanned_entries.saturating_add(1);
                if scanned_entries > TARGET_CAP * 4 {
                    return Err("target_limit".to_string());
                }
                let replica_name = replica
                    .file_name()
                    .into_string()
                    .map_err(|_| "unsafe_path".to_string())?;
                let Some(agent) = replica_name.strip_prefix("__agent_") else {
                    continue;
                };
                let candidate = format!("{project}:{name}/{agent}");
                let parsed = parse_strict_pty_fqn(&candidate)?;
                let identity = verify_replica(&project_dir, &ac_root, &parsed)?;
                if !target_names.insert(identity.canonical_fqn.clone())
                    || !target_objects.insert(identity.replica_identity.object_id)
                {
                    return Err("ambiguous_target".to_string());
                }
                targets.push(TerminalSnapshotTargetIdentity {
                    canonical_fqn: identity.canonical_fqn,
                    replica_root: identity.replica_root,
                    project: identity.project,
                    workgroup: identity.workgroup,
                    team: parsed.team,
                    is_coordinator: identity.is_coordinator,
                });
                if targets.len() > TARGET_CAP {
                    return Err("target_limit".to_string());
                }
            }
        }
    }
    targets.sort_by(|left, right| left.canonical_fqn.cmp(&right.canonical_fqn));
    Ok(targets)
}

pub(crate) fn verify_terminal_snapshot_root_identity(
    root: &Path,
) -> Result<VerifiedPtyInputIdentity, String> {
    let root_identity = crate::config::root_agent::verify_live_root_agent_path(root)?;
    Ok(VerifiedPtyInputIdentity {
        canonical_fqn: crate::config::root_agent::ROOT_AGENT_SENDER.to_string(),
        project: String::new(),
        workgroup: String::new(),
        agent: "root-agent".to_string(),
        replica_root: root_identity.canonical_path.clone(),
        matrix_root: root_identity.canonical_path.clone(),
        is_coordinator: false,
        project_identity: root_identity.clone(),
        ac_root_identity: root_identity.clone(),
        workgroup_identity: root_identity.clone(),
        replica_identity: root_identity.clone(),
        matrix_identity: root_identity.clone(),
        incarnation_fingerprint: incarnation_fingerprint(&root_identity),
        authority_fingerprint: identity_fingerprint(&[&root_identity]),
    })
}

/// Verify the only two privileged routes. No discovery repair or broad
/// communication predicate is used.
pub fn verify_pty_input_route(
    sender_cwd: &Path,
    sender_is_root: bool,
    target_fqn: &str,
    project_paths: &[String],
) -> Result<VerifiedPtyInputRoute, String> {
    if sender_is_root {
        let sender = verify_terminal_snapshot_root_identity(sender_cwd)?;
        let target = resolve_pty_input_target(target_fqn, project_paths)?;
        if !target.is_coordinator {
            return Err("target_out_of_scope".to_string());
        }
        return Ok(VerifiedPtyInputRoute {
            sender,
            target,
            kind: PtyInputAuthorityKind::Root,
        });
    }

    // Prove coordinator authority before any target hierarchy lookup. Negative
    // senders therefore cannot use this resolver as a privileged target oracle.
    let sender = verify_pty_input_coordinator_root(sender_cwd)?;
    let target = resolve_pty_input_target(target_fqn, project_paths)?;
    if sender.project != target.project || sender.workgroup != target.workgroup {
        return Err("target_out_of_scope".to_string());
    }
    if target.is_coordinator {
        return Err("target_is_coordinator".to_string());
    }
    if sender.canonical_fqn == target.canonical_fqn {
        return Err("target_out_of_scope".to_string());
    }
    Ok(VerifiedPtyInputRoute {
        sender,
        target,
        kind: PtyInputAuthorityKind::Coordinator,
    })
}

/// Verify the distinct #1173 read capability. This deliberately does not call
/// the PTY-input route policy, mint input authority, or change Root actuation.
pub(crate) fn verify_terminal_snapshot_route(
    sender_cwd: &Path,
    sender_is_root: bool,
    target_fqn: &str,
    project_paths: &[String],
) -> Result<VerifiedTerminalSnapshotRoute, String> {
    let syntax = terminal_snapshot_target_syntax(target_fqn)?;
    if sender_is_root {
        let sender = verify_terminal_snapshot_root_identity(sender_cwd)?;
        if syntax != TerminalSnapshotTargetSyntax::Workgroup {
            return Err("target_out_of_scope".to_string());
        }
        let target = resolve_verified_wg_target(target_fqn, project_paths)?;
        return Ok(VerifiedTerminalSnapshotRoute {
            sender,
            target,
            kind: TerminalSnapshotAuthorityKind::Root,
        });
    }

    // Prove the Coordinator before any target identity walk. This preserves the
    // no-oracle ordering for workers and origin senders.
    let sender = verify_pty_input_coordinator_root(sender_cwd)?;
    if syntax != TerminalSnapshotTargetSyntax::Workgroup {
        return Err("target_out_of_scope".to_string());
    }
    let target = resolve_verified_wg_target(target_fqn, project_paths)?;
    if sender.project != target.project
        || sender.workgroup != target.workgroup
        || target.is_coordinator
        || sender.canonical_fqn == target.canonical_fqn
    {
        return Err("target_out_of_scope".to_string());
    }
    Ok(VerifiedTerminalSnapshotRoute {
        sender,
        target,
        kind: TerminalSnapshotAuthorityKind::Coordinator,
    })
}

/// Resolve an agent target to a canonical FQN.
///
/// Accepts:
/// - Fully qualified WG: `<project>:<wg-N-team>/<agent>` → validated shape,
///   existence checked against `project_paths`.
/// - Origin form: `<project>/<agent>` (no colon, not WG-shaped) → returned as-is
///   (origin agents are conventionally unique; §AR2-G7).
/// - Unqualified WG: `wg-N-team/<agent>` → resolved by two-level scan across
///   `project_paths`. Unambiguous → qualified FQN returned; ambiguous → error.
/// - Bare `<agent>` (no `/`): returned as-is (Decision 2 step 3 — legacy).
///
/// Reject-on-ambiguity semantics are identical for CLI and mailbox callers.
pub fn resolve_agent_target(
    target: &str,
    project_paths: &[String],
) -> Result<String, ResolutionError> {
    // Canonical Root Agent reply target. Symmetric with `ROOT_AGENT_SENDER`
    // appearing as `msg.from` on root-originated messages. See #293.
    // Identity-verified-coordinator gating happens later (CLI:
    // `coordinator_to_root_target_allowed`; mailbox:
    // `validate_coordinator_to_root_route`).
    if crate::config::root_agent::is_root_agent_target(target) {
        return Ok(target.to_string());
    }

    // Basic shape guard.
    if target.is_empty() || target.contains('\0') {
        return Err(ResolutionError::InvalidShape(target.to_string()));
    }

    // Issue #134: reject filesystem-directory names (`__agent_*`, `_agent_*`)
    // anywhere in the agent segment. `agent_name_from_path` strips these
    // prefixes when deriving display names, so a legitimate peer FQN never
    // contains them — accepting them silently routes the message into the void.
    if agent_segment_is_filesystem_dir(target) {
        return Err(ResolutionError::LooksLikeFilesystemDir(target.to_string()));
    }

    // Case 1: fully qualified (`<project>:<local>`).
    // Require exactly one colon, non-empty project, local shaped `wg-N-team/agent`.
    if target.contains(':') {
        // More than one colon is invalid shape.
        if target.matches(':').count() != 1 {
            return Err(ResolutionError::InvalidShape(target.to_string()));
        }
        let (project, local) = match split_project_prefix(target) {
            (Some(p), l) => (p, l),
            _ => return Err(ResolutionError::InvalidShape(target.to_string())),
        };
        if !is_valid_wg_local_shape(local) {
            return Err(ResolutionError::InvalidShape(target.to_string()));
        }

        // Existence check.
        let agent = local.split_once('/').map(|(_, a)| a).unwrap_or_default();
        let wg = local.split_once('/').map(|(w, _)| w).unwrap_or_default();
        let replica_dir = format!("__agent_{}", agent);
        for (name, dir) in enumerate_project_dirs(project_paths) {
            if name != project {
                continue;
            }
            let Some(ac_root) = existing_ac_root(&dir) else {
                continue;
            };
            let candidate = ac_root.join(wg).join(&replica_dir);
            if candidate.is_dir() {
                return Ok(target.to_string());
            }
        }
        return Err(ResolutionError::UnknownQualified(target.to_string()));
    }

    // Case 2: unqualified WG-local form (`wg-N-team/agent`).
    if is_valid_wg_local_shape(target) {
        let wg = target.split_once('/').map(|(w, _)| w).unwrap_or_default();
        let agent = target.split_once('/').map(|(_, a)| a).unwrap_or_default();
        let replica_dir = format!("__agent_{}", agent);
        let mut candidates: Vec<String> = Vec::new();
        for (name, dir) in enumerate_project_dirs(project_paths) {
            let Some(ac_root) = existing_ac_root(&dir) else {
                continue;
            };
            let candidate = ac_root.join(wg).join(&replica_dir);
            if candidate.is_dir() {
                let fqn = format!("{}:{}/{}", name, wg, agent);
                if !candidates.contains(&fqn) {
                    candidates.push(fqn);
                }
            }
        }
        return match candidates.len() {
            0 => Err(ResolutionError::NoMatch(target.to_string())),
            1 => Ok(candidates.pop().unwrap()),
            _ => Err(ResolutionError::Ambiguous {
                target: target.to_string(),
                candidates,
            }),
        };
    }

    // Case 3: origin form or bare — return as-is (legacy delegation).
    Ok(target.to_string())
}

/// Resolve an agent ref (from team config) to a display name.
/// Handles relative refs like `_agent_foo` and absolute paths.
fn resolve_agent_ref(project_folder: &str, agent_ref: &str) -> String {
    let normalized = agent_ref.replace('\\', "/");
    let trimmed = normalized
        .trim_start_matches("../")
        .trim_start_matches("./");

    if trimmed.contains(':') || trimmed.starts_with('/') {
        // Absolute path: extract origin project from folder before the Project AC Root marker
        let parts: Vec<&str> = trimmed.split('/').collect();
        let origin = find_ac_root_segment(&parts)
            .and_then(|i| (i > 0).then_some(parts[i - 1]))
            .unwrap_or(project_folder);
        let dir_name = parts.last().unwrap_or(&trimmed);
        let agent_name = dir_name
            .strip_prefix("__agent_")
            .or_else(|| dir_name.strip_prefix("_agent_"))
            .unwrap_or(dir_name);
        format!("{}/{}", origin, agent_name)
    } else {
        // Relative ref: extract last component and strip prefix
        let last = trimmed.split('/').next_back().unwrap_or(trimmed);
        let agent_name = last
            .strip_prefix("__agent_")
            .or_else(|| last.strip_prefix("_agent_"))
            .unwrap_or(last);
        format!("{}/{}", project_folder, agent_name)
    }
}

/// Resolve an agent ref to an absolute path given the Project AC Root directory.
fn resolve_agent_path(ac_root: &Path, agent_ref: &str) -> Option<PathBuf> {
    let normalized = agent_ref.replace('\\', "/");
    let trimmed = normalized
        .trim_start_matches("../")
        .trim_start_matches("./");

    // Check if it's an absolute path
    if trimmed.contains(':') || trimmed.starts_with('/') {
        let p = PathBuf::from(trimmed);
        if p.is_dir() {
            return Some(p);
        }
        return None;
    }

    // Relative to the Project AC Root directory.
    let candidate = ac_root.join(trimmed);
    if candidate.is_dir() {
        return Some(candidate);
    }

    // Try parent of the Project AC Root directory (project root).
    if let Some(project_root) = ac_root.parent() {
        let candidate = project_root.join(trimmed);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    None
}

/// Extract team name from a WG-style agent name. Peels optional `<project>:`
/// prefix before inspecting the local part.
///
/// - `"proj-a:wg-1-ac-devs/dev-rust"` → `Some("ac-devs")`
/// - `"wg-1-ac-devs/dev-rust"` → `Some("ac-devs")`
/// - `"some-project/agent"` → `None`
fn extract_wg_team(agent_name: &str) -> Option<&str> {
    let (_, local) = split_project_prefix(agent_name);
    let prefix = local.split('/').next()?;
    if !prefix.starts_with("wg-") {
        return None;
    }
    prefix
        .strip_prefix("wg-")
        .and_then(|s| s.split_once('-').map(|(_, team)| team))
}

/// Extract agent suffix (part after '/') from an agent name.
fn agent_suffix(name: &str) -> &str {
    name.split('/').next_back().unwrap_or(name)
}

/// Check if an agent name matches a team member (by display name or path-derived name).
fn agent_matches_member(
    agent_name: &str,
    member_display_name: &str,
    member_path: Option<&PathBuf>,
) -> bool {
    if agent_name == member_display_name {
        return true;
    }
    if let Some(path) = member_path {
        let path_name = agent_name_from_path(&path.to_string_lossy());
        if agent_name == path_name {
            return true;
        }
    }
    false
}

/// Check if an agent belongs to a team (as a regular member OR as the coordinator).
pub fn is_in_team(agent_name: &str, team: &DiscoveredTeam) -> bool {
    // Check regular members
    for (i, display_name) in team.agent_names.iter().enumerate() {
        let path = team.agent_paths.get(i).and_then(|p| p.as_ref());
        if agent_matches_member(agent_name, display_name, path) {
            return true;
        }
    }
    // Check coordinator
    if let Some(ref coord_name) = team.coordinator_name {
        if agent_matches_member(agent_name, coord_name, team.coordinator_path.as_ref()) {
            return true;
        }
    }
    // WG-aware: if agent is a WG replica belonging to this team, match by suffix.
    // §DR8/§5.3: lenient `None => true` tolerance — unqualified agent_name matches
    // any project's team of the same name (transition aid for Decision 3's
    // tolerate-on-read). Strict semantics live in `is_coordinator` only.
    if let Some(wg_team) = extract_wg_team(agent_name) {
        let (agent_project, _) = split_project_prefix(agent_name);
        let project_matches = match agent_project {
            Some(p) => p == team.project,
            None => true,
        };
        if wg_team == team.name && project_matches {
            let suffix = agent_suffix(agent_name);
            for member_name in &team.agent_names {
                if suffix == agent_suffix(member_name) {
                    return true;
                }
            }
            if let Some(ref coord_name) = team.coordinator_name {
                if suffix == agent_suffix(coord_name) {
                    return true;
                }
            }
        }
    }
    false
}

/// Check if an agent is a coordinator of a team.
///
/// §AR2-strict: the WG-aware branch enforces **strict** project matching
/// (`None => false`). An unqualified `agent_name` (no `:` prefix) CANNOT hold
/// coordinator authority — the authorization gate for destructive operations
/// must not tolerate legacy names. `is_in_team` and `can_communicate` remain
/// lenient for display/reachability paths (§DR8).
fn is_coordinator(agent_name: &str, team: &DiscoveredTeam) -> bool {
    if let Some(ref coord_name) = team.coordinator_name {
        if agent_matches_member(agent_name, coord_name, team.coordinator_path.as_ref()) {
            log::trace!(
                "[teams] is_coordinator: direct-match → true — agent='{}' team='{}/{}' coord='{}'",
                agent_name,
                team.project,
                team.name,
                coord_name
            );
            return true;
        }
        // WG-aware: if agent is a WG replica of this team's coordinator, match by suffix.
        // Cross-WG authority within the same team is allowed (wg-2/tech-lead can manage
        // agents in teams originally defined with wg-1/tech-lead as coordinator). Cross-
        // project authority is NOT allowed — the project guard below enforces this.
        if let Some(wg_team) = extract_wg_team(agent_name) {
            let (agent_project, _) = split_project_prefix(agent_name);
            let Some(agent_project) = agent_project else {
                // Strict: unqualified `agent_name` cannot hold coordinator authority.
                if wg_team == team.name && agent_suffix(agent_name) == agent_suffix(coord_name) {
                    log::trace!(
                        "[teams] is_coordinator: reject-unqualified → false — agent='{}' team='{}/{}' coord='{}' (suffix would match)",
                        agent_name, team.project, team.name, coord_name
                    );
                }
                return false;
            };
            if wg_team == team.name
                && agent_project == team.project
                && agent_suffix(agent_name) == agent_suffix(coord_name)
            {
                log::trace!(
                    "[teams] is_coordinator: wg-aware-match → true — agent='{}' team='{}/{}' coord='{}' agent_project='{}'",
                    agent_name, team.project, team.name, coord_name, agent_project
                );
                return true;
            }
            if wg_team == team.name
                && agent_project != team.project
                && agent_suffix(agent_name) == agent_suffix(coord_name)
            {
                log::trace!(
                    "[teams] is_coordinator: reject-project-mismatch → false — agent='{}' agent_project='{}' team_project='{}' team='{}' coord='{}'",
                    agent_name, agent_project, team.project, team.name, coord_name
                );
            }
            if wg_team == team.name
                && agent_project == team.project
                && agent_suffix(agent_name) != agent_suffix(coord_name)
            {
                log::trace!(
                    "[teams] is_coordinator: reject-suffix-mismatch → false — agent='{}' team='{}/{}' coord='{}' agent_suffix='{}' coord_suffix='{}'",
                    agent_name, team.project, team.name, coord_name,
                    agent_suffix(agent_name), agent_suffix(coord_name)
                );
            }
            if wg_team == team.name
                && agent_project != team.project
                && agent_suffix(agent_name) != agent_suffix(coord_name)
            {
                log::trace!(
                    "[teams] is_coordinator: reject-both-mismatch → false — agent='{}' agent_project='{}' team_project='{}' team='{}' coord='{}' agent_suffix='{}' coord_suffix='{}'",
                    agent_name, agent_project, team.project, team.name, coord_name,
                    agent_suffix(agent_name), agent_suffix(coord_name)
                );
            }
        }
    }
    false
}

/// Check if sender is a coordinator of any team that contains target as a member.
pub fn is_coordinator_of(sender: &str, target: &str, teams: &[DiscoveredTeam]) -> bool {
    teams
        .iter()
        .any(|team| is_coordinator(sender, team) && is_in_team(target, team))
}

/// Check if an agent is a coordinator of ANY discovered team.
pub fn is_any_coordinator(agent_name: &str, teams: &[DiscoveredTeam]) -> bool {
    teams.iter().any(|t| is_coordinator(agent_name, t))
}

/// Resolve whether the agent running at `working_directory` is a coordinator of any discovered team.
/// Thin wrapper so call sites don't have to duplicate the `agent_fqn_from_path` + `is_any_coordinator` pair.
///
/// §DR2: uses `agent_fqn_from_path` so WG replicas get project-precise
/// coordinator checks. `is_coordinator` is strict (§AR2-strict) — the FQN
/// here ensures cross-project coordinator flags never leak.
pub fn is_coordinator_for_cwd(working_directory: &str, teams: &[DiscoveredTeam]) -> bool {
    let agent_name = agent_fqn_from_path(working_directory);
    is_any_coordinator(&agent_name, teams)
}

/// Check if two agents can communicate based on discovery-based team routing rules.
///
/// Rules:
/// 1. Same team (member or coordinator) → allowed
/// 2. WG-scoped: agents in the same workgroup → allowed
/// 3. Both are coordinators (of any team) → allowed (cross-team coordinator chat)
/// 4. Otherwise → denied
pub fn can_communicate(from: &str, to: &str, teams: &[DiscoveredTeam]) -> bool {
    // Rule 1: Same team (includes both regular members and coordinator)
    for team in teams {
        if is_in_team(from, team) && is_in_team(to, team) {
            return true;
        }
    }

    // Rule 2: WG-scoped (agents in the same workgroup can communicate).
    // §5.5: peel optional `<project>:` prefix from both sides; require same
    // project when both are qualified, lenient when either is unqualified
    // (transition aid for Decision 3's tolerate-on-read).
    let (from_proj, from_local) = split_project_prefix(from);
    let (to_proj, to_local) = split_project_prefix(to);
    if from_local.starts_with("wg-") && to_local.starts_with("wg-") {
        let from_wg = from_local.split('/').next().unwrap_or("");
        let to_wg = to_local.split('/').next().unwrap_or("");
        let project_match = match (from_proj, to_proj) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        };
        if !from_wg.is_empty() && from_wg == to_wg && project_match {
            return true;
        }
    }

    // Rule 3: Coordinator-to-coordinator (any teams)
    let from_is_coordinator = teams.iter().any(|t| is_coordinator(from, t));
    let to_is_coordinator = teams.iter().any(|t| is_coordinator(to, t));
    if from_is_coordinator && to_is_coordinator {
        return true;
    }

    false
}

/// Discover all teams from all known project paths.
/// Scans settings.project_paths (and immediate children) for Project AC Root `_team_*/config.json`.
pub fn discover_teams() -> Vec<DiscoveredTeam> {
    let settings = crate::config::settings::load_settings();
    let mut teams = Vec::new();

    for repo_path in &settings.project_paths {
        log::trace!(
            "[teams] discover_teams: scanning project_path='{}'",
            repo_path
        );
        let base = Path::new(repo_path);
        if !base.is_dir() {
            log::trace!(
                "[teams] discover_teams: project_path skipped (not a directory) — path='{}'",
                repo_path
            );
            continue;
        }

        // Check base and immediate children (same pattern as ac_discovery)
        let mut dirs_to_check = vec![base.to_path_buf()];
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') {
                        dirs_to_check.push(p);
                    }
                }
            }
        }

        for project_dir in dirs_to_check {
            let teams_before = teams.len();
            log::trace!(
                "[teams] discover_teams: entering project_dir='{}'",
                project_dir.display()
            );
            discover_teams_in_project(&project_dir, &mut teams);
            log::trace!(
                "[teams] discover_teams: project_dir='{}' produced {} team(s)",
                project_dir.display(),
                teams.len() - teams_before
            );
        }
    }

    log::debug!(
        "[teams] discovered {} team(s) across {} project path(s)",
        teams.len(),
        settings.project_paths.len()
    );
    teams
}

/// Discover teams in a single project directory.
fn discover_teams_in_project(project_dir: &Path, teams: &mut Vec<DiscoveredTeam>) {
    let Some(ac_root) = existing_ac_root(project_dir) else {
        return;
    };

    let project_folder = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let entries = match std::fs::read_dir(&ac_root) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        log::trace!(
            "[teams] discover_teams_in_project: inspecting entry — project='{}' entry='{}'",
            project_folder,
            entry.file_name().to_string_lossy()
        );
        let team_dir = entry.path();
        if !team_dir.is_dir() {
            continue;
        }

        let dir_name = match team_dir.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.starts_with("_team_") => n,
            _ => continue,
        };

        let team_name = dir_name
            .strip_prefix("_team_")
            .unwrap_or(dir_name)
            .to_string();

        let config_path = team_dir.join("config.json");
        let raw = match std::fs::read_to_string(&config_path) {
            Ok(s) => s,
            Err(e) => {
                // #280 §3.4: NotFound is an expected state for half-installed
                // team dirs (`_team_foo/` created but no `config.json` yet).
                // Logging WARN at every discovery sweep spams app.log, so both
                // the one-shot per-(project, team_dir) visit record and the
                // per-drop record below are DEBUG (#612 downgraded the prior
                // one-shot INFO). Unexpected IO errors stay at WARN.
                match e.kind() {
                    std::io::ErrorKind::NotFound => {
                        if note_missing_team_config(&project_folder, dir_name) {
                            log::debug!(
                                "[teams] team config missing (logged once per startup) — project='{}' team_dir='{}' path='{}'",
                                project_folder,
                                dir_name,
                                config_path.display()
                            );
                        }
                        log::debug!(
                            "[teams] dropped team — project='{}' team_dir='{}' reason='not_found' path='{}'",
                            project_folder,
                            dir_name,
                            config_path.display()
                        );
                    }
                    _ => {
                        log::warn!(
                            "[teams] dropped team — project='{}' team_dir='{}' reason='read_failed' err='{}' path='{}'",
                            project_folder,
                            dir_name,
                            e,
                            config_path.display()
                        );
                    }
                }
                continue;
            }
        };
        let parsed: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                log::warn!(
                    "[teams] dropped team — project='{}' team_dir='{}' reason='parse_failed' err='{}' path='{}'",
                    project_folder,
                    dir_name,
                    e,
                    config_path.display()
                );
                continue;
            }
        };

        // Resolve agents — build names and paths in a single pass to keep indices aligned
        let agent_refs: Vec<String> = parsed
            .get("agents")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let (agent_names, agent_paths): (Vec<String>, Vec<Option<PathBuf>>) = agent_refs
            .iter()
            .map(|r| {
                let name = resolve_agent_ref(&project_folder, r);
                let path = resolve_agent_path(&ac_root, r);
                (name, path)
            })
            .unzip();

        // Resolve coordinator
        let coordinator_ref = parsed
            .get("coordinator")
            .and_then(|c| c.as_str())
            .map(String::from);

        let coordinator_name = coordinator_ref
            .as_ref()
            .map(|r| resolve_agent_ref(&project_folder, r));

        let coordinator_path = coordinator_ref
            .as_ref()
            .and_then(|r| resolve_agent_path(&ac_root, r));

        teams.push(DiscoveredTeam {
            name: team_name,
            project: project_folder.clone(),
            agent_names,
            agent_paths,
            coordinator_name,
            coordinator_path,
        });
        let pushed = teams.last().expect("just pushed");
        log::debug!(
            "[teams] discovered team — project='{}' team='{}' coord_name={:?} coord_path={:?} agent_count={}",
            pushed.project,
            pushed.name,
            pushed.coordinator_name,
            pushed.coordinator_path.as_ref().map(|p| p.display().to_string()),
            pushed.agent_names.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper-function tests (AR2-tests 1-7) ──

    #[test]
    fn agent_fqn_from_path_wg_replica() {
        let cwd = "C:/repos/proj-a/.ac/wg-1-devs/__agent_alice";
        assert_eq!(agent_fqn_from_path(cwd), "proj-a:wg-1-devs/alice");
    }

    #[test]
    fn agent_fqn_from_path_wg_replica_windows_style() {
        let cwd = "C:/repos/proj-a/.ac/wg-1-devs/__agent_alice";
        assert_eq!(agent_fqn_from_path(cwd), "proj-a:wg-1-devs/alice");
    }

    #[test]
    fn agent_fqn_from_path_origin() {
        // Non-WG (origin) path: falls back to agent_name_from_path shape.
        let cwd = "C:/repos/my-project/tech-lead";
        assert_eq!(agent_fqn_from_path(cwd), "my-project/tech-lead");
    }

    /// §G6 case 1: subdirectory inside a replica still resolves to the replica's FQN.
    #[test]
    fn agent_fqn_from_path_deeper_cwd_returns_replica_fqn() {
        let cwd = "C:/repos/proj-a/.ac/wg-1-devs/__agent_alice/some/deep/subdir";
        assert_eq!(agent_fqn_from_path(cwd), "proj-a:wg-1-devs/alice");
    }

    /// §G6 case 3: Windows UNC `\\?\` prefix must still resolve correctly.
    #[test]
    fn agent_fqn_from_path_handles_unc_prefix() {
        let cwd = r"\\?\C:\repos\proj-a\.ac\wg-1-devs\__agent_alice";
        assert_eq!(agent_fqn_from_path(cwd), "proj-a:wg-1-devs/alice");
    }

    /// §G6 case 2: parent path containing `.ac` anchors on the right-most one (rposition).
    #[test]
    fn agent_fqn_from_path_parent_ac_prefix() {
        let cwd = "C:/.ac/repos/proj-a/.ac/wg-1-devs/__agent_alice";
        assert_eq!(agent_fqn_from_path(cwd), "proj-a:wg-1-devs/alice");
    }

    // ── #516 workgroup_and_agent_from_path ──

    #[test]
    fn workgroup_and_agent_from_path_wg_replica() {
        let cwd = "C:/repos/proj-a/.ac/wg-5-dev-team/__agent_dev-rust";
        assert_eq!(
            workgroup_and_agent_from_path(cwd),
            (
                Some("wg-5-dev-team".to_string()),
                Some("dev-rust".to_string())
            )
        );
    }

    /// A deep subdir inside a replica (and Windows backslashes) still resolves
    /// to the owning replica's WG/agent pair.
    #[test]
    fn workgroup_and_agent_from_path_deeper_cwd() {
        let cwd = r"C:\repos\proj-a\.ac\wg-1-devs\__agent_alice\repo-x\src";
        assert_eq!(
            workgroup_and_agent_from_path(cwd),
            (Some("wg-1-devs".to_string()), Some("alice".to_string()))
        );
    }

    #[test]
    fn workgroup_and_agent_from_path_non_wg_returns_none() {
        assert_eq!(
            workgroup_and_agent_from_path("C:/repos/my-project/tech-lead"),
            (None, None)
        );
    }

    /// Root-agent dir has no `wg-`/`__agent_` segments: the helper returns
    /// `(None, None)` and the caller applies the "Root agent" label fallback.
    #[test]
    fn workgroup_and_agent_from_path_root_agent_dir_returns_none() {
        assert_eq!(
            workgroup_and_agent_from_path("C:/repos/proj-a/.ac/ac-root-agent"),
            (None, None)
        );
    }

    // ── #566 project_from_path ──

    #[test]
    fn project_from_path_wg_replica() {
        let cwd = "C:/repos/AgentsCommander_ac/.ac/wg-5-dev-team/__agent_dev-rust";
        assert_eq!(
            project_from_path(cwd),
            Some("AgentsCommander_ac".to_string())
        );
    }

    /// Deep subdir inside a replica (and Windows backslashes) still anchors on
    /// the right-most `.ac` and returns the owning project.
    #[test]
    fn project_from_path_deeper_cwd_windows_style() {
        let cwd = r"C:\repos\proj-a\.ac\wg-1-devs\__agent_alice\repo-x\src";
        assert_eq!(project_from_path(cwd), Some("proj-a".to_string()));
    }

    /// Permissive by design: root agents carry a project even though their
    /// wg/agent resolve to `(None, None)` / the "Root agent" label fallback.
    #[test]
    fn project_from_path_root_agent() {
        assert_eq!(
            project_from_path("C:/repos/proj-a/.ac/ac-root-agent"),
            Some("proj-a".to_string())
        );
    }

    /// Origin / matrix agents also carry a project (the parent of `.ac`).
    #[test]
    fn project_from_path_origin_agent() {
        assert_eq!(
            project_from_path("C:/repos/proj-a/.ac/_agent_architect"),
            Some("proj-a".to_string())
        );
    }

    /// No `.ac` segment at all (ad-hoc / non-AC shell) -> `None`.
    #[test]
    fn project_from_path_no_ac_root_segment_returns_none() {
        assert_eq!(project_from_path("C:/repos/my-project/tech-lead"), None);
    }

    /// `.ac` as the first segment has no parent project dir -> `None`.
    #[test]
    fn project_from_path_ac_first_segment_returns_none() {
        assert_eq!(project_from_path("/.ac/wg-1-devs/__agent_alice"), None);
    }

    #[test]
    fn split_project_prefix_present() {
        assert_eq!(
            split_project_prefix("proj-a:wg-1-devs/alice"),
            (Some("proj-a"), "wg-1-devs/alice")
        );
    }

    #[test]
    fn split_project_prefix_absent() {
        assert_eq!(
            split_project_prefix("wg-1-devs/alice"),
            (None, "wg-1-devs/alice")
        );
        // Empty-project edge: `:foo` is not treated as qualified.
        assert_eq!(split_project_prefix(":foo"), (None, ":foo"));
        // Empty-local edge: `foo:` is not treated as qualified.
        assert_eq!(split_project_prefix("foo:"), (None, "foo:"));
    }

    #[test]
    fn extract_wg_team_peels_project_prefix() {
        assert_eq!(
            extract_wg_team("proj-a:wg-1-dev-team/alice"),
            Some("dev-team")
        );
        assert_eq!(extract_wg_team("wg-1-dev-team/alice"), Some("dev-team"));
        assert_eq!(extract_wg_team("origin-proj/alice"), None);
    }

    // ── resolve_agent_target tests (AR2-tests 12-16) ──

    /// Auto-cleaned temp dir for fixture roots. Matches the convention used in
    /// `phone/messaging.rs` tests — no new crate dependencies.
    struct FixtureRoot(PathBuf);
    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl FixtureRoot {
        fn new(prefix: &str) -> Self {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::process::id().hash(&mut h);
            std::thread::current().id().hash(&mut h);
            let path = std::env::temp_dir().join(format!(
                "{}-{}-{}",
                prefix,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                h.finish()
            ));
            std::fs::create_dir_all(&path).expect("fixture root");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    /// Build a fake project layout on disk so `resolve_agent_target` can scan it.
    /// `projects` is a slice of `(project_name, &[(wg_name, &[agent_short])])`.
    // The nested-slice shape is the most direct way to express the fixture; a
    // type alias would obscure the structure at the only call sites.
    #[allow(clippy::type_complexity)]
    fn make_project_fixture(projects: &[(&str, &[(&str, &[&str])])]) -> (FixtureRoot, Vec<String>) {
        let tmp = FixtureRoot::new("teams-fixture");
        for (proj_name, wgs) in projects {
            let proj_dir = tmp.path().join(proj_name);
            std::fs::create_dir_all(&proj_dir).unwrap();
            let ac_root = proj_dir.join(".ac");
            std::fs::create_dir_all(&ac_root).unwrap();
            for (wg_name, agents) in *wgs {
                let wg_dir = ac_root.join(wg_name);
                std::fs::create_dir_all(&wg_dir).unwrap();
                for agent in *agents {
                    let replica = wg_dir.join(format!("__agent_{}", agent));
                    std::fs::create_dir_all(&replica).unwrap();
                }
            }
        }
        let paths = vec![tmp.path().to_string_lossy().to_string()];
        (tmp, paths)
    }

    fn make_coordinator_fixture(spoofed_coordinator_identity: bool) -> (FixtureRoot, Vec<String>) {
        let tmp = FixtureRoot::new("teams-coord-fixture");
        let project = tmp.path().join("proj-a");
        let ac_root = project.join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let origin_tech_lead = ac_root.join("_agent_tech-lead");
        let origin_dev_rust = ac_root.join("_agent_dev-rust");
        let wg_dir = ac_root.join("wg-1-dev-team");
        let tech_lead_replica = wg_dir.join("__agent_tech-lead");
        let dev_rust_replica = wg_dir.join("__agent_dev-rust");

        for dir in [
            &team_dir,
            &origin_tech_lead,
            &origin_dev_rust,
            &tech_lead_replica,
            &dev_rust_replica,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }

        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"coordinator":"../_agent_tech-lead"}"#,
        )
        .unwrap();
        let tech_lead_identity = if spoofed_coordinator_identity {
            "../../_agent_dev-rust"
        } else {
            "../../_agent_tech-lead"
        };
        std::fs::write(
            tech_lead_replica.join("config.json"),
            format!(r#"{{"identity":"{}"}}"#, tech_lead_identity),
        )
        .unwrap();
        std::fs::write(
            dev_rust_replica.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();

        let paths = vec![tmp.path().to_string_lossy().to_string()];
        (tmp, paths)
    }

    /// #1245: the team-config shape the application's writer actually produces,
    /// with the coordinator listed inside `agents`. A third member exists so the
    /// surviving member order can be pinned once the coordinator entry is
    /// consumed; `make_coordinator_fixture` has only one ordinary member.
    fn make_writer_shaped_team_fixture() -> (FixtureRoot, Vec<String>) {
        let tmp = FixtureRoot::new("teams-writer-shape-fixture");
        let ac_root = tmp.path().join("proj-a").join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let wg_dir = ac_root.join("wg-1-dev-team");

        std::fs::create_dir_all(&team_dir).unwrap();
        for agent in ["tech-lead", "dev-rust", "dev-ts"] {
            std::fs::create_dir_all(ac_root.join(format!("_agent_{agent}"))).unwrap();
            let replica = wg_dir.join(format!("__agent_{agent}"));
            std::fs::create_dir_all(&replica).unwrap();
            std::fs::write(
                replica.join("config.json"),
                format!(r#"{{"identity":"../../_agent_{agent}"}}"#),
            )
            .unwrap();
        }
        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead","../_agent_dev-ts"],"coordinator":"../_agent_tech-lead"}"#,
        )
        .unwrap();

        let paths = vec![tmp.path().to_string_lossy().to_string()];
        (tmp, paths)
    }

    #[test]
    fn team_config_with_coordinator_in_agents_verifies_and_excludes_the_coordinator_from_members() {
        let (fixture, paths) = make_writer_shaped_team_fixture();
        let ac_root = fixture.path().join("proj-a").join(".ac");
        let wg_dir = ac_root.join("wg-1-dev-team");
        let tech_lead = wg_dir.join("__agent_tech-lead");
        let dev_rust = wg_dir.join("__agent_dev-rust");

        let route =
            verify_pty_input_route(&tech_lead, false, "proj-a:wg-1-dev-team/dev-rust", &paths)
                .unwrap();
        assert_eq!(route.sender.canonical_fqn, "proj-a:wg-1-dev-team/tech-lead");
        assert!(route.sender.is_coordinator);

        assert!(
            verify_pty_input_route(&dev_rust, false, "proj-a:wg-1-dev-team/dev-ts", &paths)
                .is_err(),
            "a worker sender must still be rejected"
        );

        let snapshot = verify_terminal_snapshot_route(
            &tech_lead,
            false,
            "proj-a:wg-1-dev-team/dev-ts",
            &paths,
        )
        .unwrap();
        assert_eq!(snapshot.kind, TerminalSnapshotAuthorityKind::Coordinator);

        // The coordinator is still not a valid capture target even though it now
        // appears in `agents`. This guards the authorization matrix. It cannot
        // observe the member exclusion: `verify_replica` derives
        // `is_coordinator` from the `coordinator` key and short-circuits the
        // `target_not_member` gate, so it holds for any `members` content.
        assert!(verify_terminal_snapshot_route(
            &tech_lead,
            false,
            "proj-a:wg-1-dev-team/tech-lead",
            &paths,
        )
        .is_err());

        // Only a direct call can pin the exclusion. Asserting the whole vector
        // also pins that consuming the coordinator entry leaves the order and
        // the count of the remaining members untouched.
        let (coordinator, members, _) = team_members(&ac_root, "dev-team").unwrap();
        assert_eq!(coordinator, "tech-lead");
        assert_eq!(members, vec!["dev-rust".to_string(), "dev-ts".to_string()]);
    }

    #[test]
    fn team_config_rejects_repeated_agent_and_repeated_coordinator_entries() {
        let (fixture, paths) = make_writer_shaped_team_fixture();
        let ac_root = fixture.path().join("proj-a").join(".ac");
        let team_config = ac_root.join("_team_dev-team").join("config.json");
        let tech_lead = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");

        for agents in [
            r#"["../_agent_dev-rust","../_agent_dev-rust","../_agent_tech-lead"]"#,
            r#"["../_agent_tech-lead","../_agent_tech-lead","../_agent_dev-rust"]"#,
            // Equivalent spellings collapse in `agent_bare_name_from_ref`, so a
            // repeated coordinator is caught whichever way it is written.
            r#"["../_agent_tech-lead","_agent_tech-lead","../_agent_dev-rust"]"#,
        ] {
            std::fs::write(
                &team_config,
                format!(r#"{{"agents":{agents},"coordinator":"../_agent_tech-lead"}}"#),
            )
            .unwrap();
            assert!(
                verify_pty_input_route(&tech_lead, false, "proj-a:wg-1-dev-team/dev-rust", &paths)
                    .is_err(),
                "duplicate detection must not weaken for agents {agents}"
            );
        }
    }

    #[test]
    fn strict_pty_target_syntax_rejects_aliases_paths_and_wildcards() {
        assert!(validate_pty_input_target_syntax("proj-a:wg-1-dev-team/dev-rust").is_ok());
        for invalid in [
            "dev-rust",
            "wg-1-dev-team/dev-rust",
            "proj-a/dev-rust",
            "proj-a:wg-1-dev-team/__agent_dev-rust",
            "proj-a:wg-1-dev-team/dev_rust",
            "proj-a:wg-x-dev-team/dev-rust",
            "proj-a:wg-1-dev-team/dev-rust/extra",
            "proj*:wg-1-dev-team/dev-rust",
            "../proj:wg-1-dev-team/dev-rust",
            "proj-a:wg-1-dev-team/dev-rust\u{202e}",
        ] {
            assert!(
                validate_pty_input_target_syntax(invalid).is_err(),
                "invalid={invalid:?}"
            );
        }
    }

    #[test]
    fn terminal_snapshot_syntax_accepts_policy_denied_origin_and_root_targets() {
        assert!(validate_terminal_snapshot_target_syntax("proj-a:wg-1-dev-team/dev-rust").is_ok());
        assert!(validate_terminal_snapshot_target_syntax("proj-a/dev-rust").is_ok());
        assert!(validate_terminal_snapshot_target_syntax(
            crate::config::root_agent::ROOT_AGENT_SENDER
        )
        .is_ok());
        assert!(validate_terminal_snapshot_target_syntax("*").is_err());
    }

    #[test]
    fn terminal_snapshot_coordinator_policy_is_distinct_from_pty_input() {
        let (fixture, paths) = make_coordinator_fixture(false);
        let ac_root = fixture.path().join("proj-a").join(".ac");
        let coordinator = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
        let route = verify_terminal_snapshot_route(
            &coordinator,
            false,
            "proj-a:wg-1-dev-team/dev-rust",
            &paths,
        )
        .unwrap();
        assert_eq!(route.kind, TerminalSnapshotAuthorityKind::Coordinator);
        assert_eq!(route.target.canonical_fqn, "proj-a:wg-1-dev-team/dev-rust");
        let diagnostic = format!("{route:?}");
        let sender_path = route.sender.replica_root.to_string_lossy().into_owned();
        let target_path = route.target.replica_root.to_string_lossy().into_owned();
        for forbidden in [
            route.sender.canonical_fqn.as_str(),
            route.target.canonical_fqn.as_str(),
            sender_path.as_str(),
            target_path.as_str(),
            route.sender.incarnation_fingerprint.as_str(),
            route.target.authority_fingerprint.as_str(),
        ] {
            assert!(!diagnostic.contains(forbidden));
        }
        assert!(diagnostic.contains("kind: Coordinator"));
        assert!(verify_terminal_snapshot_route(
            &coordinator,
            false,
            "proj-a:wg-1-dev-team/tech-lead",
            &paths,
        )
        .is_err());
        assert!(verify_terminal_snapshot_route(
            &coordinator,
            false,
            crate::config::root_agent::ROOT_AGENT_SENDER,
            &paths,
        )
        .is_err());
    }

    #[test]
    fn terminal_snapshot_target_debug_omits_identity_and_path_text() {
        const AUTH_CANARY: &str = "AUTH_1173_T8F2";
        const PATH_CANARY: &str = r"C:\PATH_1173_T8F2\replica";
        let target = TerminalSnapshotTargetIdentity {
            canonical_fqn: AUTH_CANARY.to_string(),
            replica_root: PathBuf::from(PATH_CANARY),
            project: AUTH_CANARY.to_string(),
            workgroup: AUTH_CANARY.to_string(),
            team: AUTH_CANARY.to_string(),
            is_coordinator: true,
        };
        let diagnostic = format!("{target:?}");
        assert!(!diagnostic.contains(AUTH_CANARY));
        assert!(!diagnostic.contains(PATH_CANARY));
        assert!(diagnostic.contains("is_coordinator: true"));
    }

    #[test]
    fn privileged_route_is_exact_and_uses_duplicate_free_identity_snapshots() {
        let (fixture, mut paths) = make_coordinator_fixture(false);
        let ac_root = fixture.path().join("proj-a").join(".ac");
        let coordinator = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
        let route =
            verify_pty_input_route(&coordinator, false, "proj-a:wg-1-dev-team/dev-rust", &paths)
                .unwrap();
        assert_eq!(route.sender.canonical_fqn, "proj-a:wg-1-dev-team/tech-lead");
        assert_eq!(route.target.canonical_fqn, "proj-a:wg-1-dev-team/dev-rust");
        assert_eq!(route.kind, PtyInputAuthorityKind::Coordinator);

        paths.push(paths[0].clone());
        assert!(verify_pty_input_route(
            &coordinator,
            false,
            "proj-a:wg-1-dev-team/dev-rust",
            &paths,
        )
        .is_ok());
        assert!(verify_pty_input_route(
            &coordinator,
            false,
            "Proj-a:wg-1-dev-team/dev-rust",
            &paths,
        )
        .is_err());
        assert!(verify_pty_input_route(
            &coordinator,
            false,
            "proj-a:wg-1-dev-team/tech-lead",
            &paths,
        )
        .is_err());

        std::fs::write(
            ac_root.join("_team_dev-team").join("config.json"),
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"agents":[],"coordinator":"../_agent_tech-lead"}"#,
        )
        .unwrap();
        assert!(verify_pty_input_route(
            &coordinator,
            false,
            "proj-a:wg-1-dev-team/dev-rust",
            &paths,
        )
        .is_err());
    }

    #[test]
    fn sender_incarnation_survives_benign_config_content_changes() {
        let (fixture, paths) = make_coordinator_fixture(false);
        let ac_root = fixture.path().join("proj-a").join(".ac");
        let coordinator = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
        let first =
            verify_pty_input_route(&coordinator, false, "proj-a:wg-1-dev-team/dev-rust", &paths)
                .unwrap();

        std::fs::write(
            coordinator.join("config.json"),
            r#"{"identity":"../../_agent_tech-lead","benign":"changed"}"#,
        )
        .unwrap();
        std::fs::write(
            ac_root.join("_team_dev-team").join("config.json"),
            r#"{"agents":["../_agent_dev-rust","../_agent_tech-lead"],"coordinator":"../_agent_tech-lead","benign":"changed"}"#,
        )
        .unwrap();
        let second =
            verify_pty_input_route(&coordinator, false, "proj-a:wg-1-dev-team/dev-rust", &paths)
                .unwrap();

        assert_eq!(
            first.sender.incarnation_fingerprint, second.sender.incarnation_fingerprint,
            "the permanent sender incarnation must not include mutable config bytes"
        );
        assert_ne!(
            first.sender.authority_fingerprint, second.sender.authority_fingerprint,
            "queued authority revalidation must still observe config content changes"
        );
    }

    #[test]
    fn privileged_route_rejects_noncanonical_identity_and_broken_project_roots() {
        let (fixture, mut paths) = make_coordinator_fixture(false);
        let ac_root = fixture.path().join("proj-a").join(".ac");
        let coordinator = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
        let coordinator_config = coordinator.join("config.json");
        std::fs::write(
            &coordinator_config,
            r#"{"identity":"elsewhere/_agent_tech-lead"}"#,
        )
        .unwrap();
        assert!(verify_pty_input_route(
            &coordinator,
            false,
            "proj-a:wg-1-dev-team/dev-rust",
            &paths,
        )
        .is_err());

        std::fs::write(
            &coordinator_config,
            r#"{"identity":"../../_agent_tech-lead"}"#,
        )
        .unwrap();
        paths.push(
            fixture
                .path()
                .join("missing-project-root")
                .to_string_lossy()
                .to_string(),
        );
        assert!(verify_pty_input_route(
            &coordinator,
            false,
            "proj-a:wg-1-dev-team/dev-rust",
            &paths,
        )
        .is_err());
    }

    #[test]
    fn privileged_route_rejects_duplicate_replica_identity_and_worker_sender() {
        let (fixture, paths) = make_coordinator_fixture(false);
        let ac_root = fixture.path().join("proj-a").join(".ac");
        let coordinator = ac_root.join("wg-1-dev-team").join("__agent_tech-lead");
        let worker = ac_root.join("wg-1-dev-team").join("__agent_dev-rust");
        assert!(
            verify_pty_input_route(&worker, false, "proj-a:wg-1-dev-team/tech-lead", &paths,)
                .is_err()
        );
        std::fs::write(
            worker.join("config.json"),
            r#"{"identity":"../../_agent_dev-rust","identity":"../../_agent_dev-rust"}"#,
        )
        .unwrap();
        assert!(verify_pty_input_route(
            &coordinator,
            false,
            "proj-a:wg-1-dev-team/dev-rust",
            &paths,
        )
        .is_err());
    }

    fn make_portable_coordinator_fixture(
        spoofed_coordinator_identity: bool,
    ) -> (FixtureRoot, PathBuf, PathBuf, Vec<String>) {
        let tmp = FixtureRoot::new("teams-portable-coord-fixture");
        let project = tmp.path().join("proj-a");
        let ac_root = project.join(".ac");
        let team_dir = ac_root.join("_team_dev-team");
        let local_tech_lead = ac_root.join("_agent_tech-lead");
        let local_dev_rust = ac_root.join("_agent_dev-rust");
        let origin = tmp.path().join("origin-matrix").join(".ac");
        let origin_tech_lead = origin.join("_agent_tech-lead");
        let origin_dev_rust = origin.join("_agent_dev-rust");
        let wg_dir = ac_root.join("wg-1-dev-team");
        let tech_lead_replica = wg_dir.join("__agent_tech-lead");
        let dev_rust_replica = wg_dir.join("__agent_dev-rust");

        for dir in [
            &team_dir,
            &local_tech_lead,
            &local_dev_rust,
            &origin_tech_lead,
            &origin_dev_rust,
            &tech_lead_replica,
            &dev_rust_replica,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }

        std::fs::write(
            team_dir.join("config.json"),
            r#"{"agents":["_agent_dev-rust"],"coordinator":"_agent_tech-lead"}"#,
        )
        .unwrap();

        let tech_lead_identity = if spoofed_coordinator_identity {
            &origin_dev_rust
        } else {
            &origin_tech_lead
        }
        .to_string_lossy()
        .replace('\\', "/");
        std::fs::write(
            tech_lead_replica.join("config.json"),
            format!(r#"{{"identity":"{}"}}"#, tech_lead_identity),
        )
        .unwrap();

        let dev_rust_identity = origin_dev_rust.to_string_lossy().replace('\\', "/");
        std::fs::write(
            dev_rust_replica.join("config.json"),
            format!(r#"{{"identity":"{}"}}"#, dev_rust_identity),
        )
        .unwrap();

        let paths = vec![tmp.path().to_string_lossy().to_string()];
        (tmp, ac_root, wg_dir, paths)
    }

    #[test]
    fn resolve_wg_coordinator_replica_uses_identity_not_dir_name() {
        let (tmp, _paths) = make_coordinator_fixture(false);
        let ac_root = tmp.path().join("proj-a").join(".ac");
        let wg_dir = ac_root.join("wg-1-dev-team");

        let resolved =
            resolve_wg_coordinator_replica(&ac_root, &wg_dir).expect("coordinator");

        assert_eq!(resolved.project, "proj-a");
        assert_eq!(resolved.team, "dev-team");
        assert_eq!(resolved.wg_name, "wg-1-dev-team");
        assert_eq!(resolved.agent_name, "tech-lead");
        assert_eq!(
            identity_compare_key(&resolved.replica_dir),
            identity_compare_key(&wg_dir.join(format!("__agent_{}", resolved.agent_name)))
        );
    }

    #[test]
    fn resolve_wg_coordinator_replica_rejects_spoofed_name() {
        let (tmp, _paths) = make_coordinator_fixture(true);
        let ac_root = tmp.path().join("proj-a").join(".ac");
        let wg_dir = ac_root.join("wg-1-dev-team");

        assert!(resolve_wg_coordinator_replica(&ac_root, &wg_dir).is_none());
    }

    #[test]
    fn resolve_wg_coordinator_replica_accepts_portable_ref_with_external_identity() {
        let (_tmp, ac_root, wg_dir, _paths) = make_portable_coordinator_fixture(false);

        let resolved = resolve_wg_coordinator_replica(&ac_root, &wg_dir)
            .expect("portable coordinator ref should match declared identity agent");

        assert_eq!(resolved.project, "proj-a");
        assert_eq!(resolved.team, "dev-team");
        assert_eq!(resolved.wg_name, "wg-1-dev-team");
        assert_eq!(resolved.agent_name, "tech-lead");
    }

    #[test]
    fn resolve_wg_coordinator_replica_rejects_portable_ref_with_spoofed_identity() {
        let (_tmp, ac_root, wg_dir, _paths) = make_portable_coordinator_fixture(true);

        assert!(resolve_wg_coordinator_replica(&ac_root, &wg_dir).is_none());
    }

    /// Build a fixture where both team config and replica configs reference
    /// a renamed folder (`renamed-workspace`) that no longer exists on disk.
    /// Mirrors the post-workspace-rename state from #299: persisted refs are
    /// stale, but the same-workspace local matrices exist and are the only valid
    /// authority targets.
    fn make_stale_coordinator_fixture(
        replica_spoofs_identity: bool,
    ) -> (FixtureRoot, PathBuf, PathBuf) {
        let tmp = FixtureRoot::new("teams-stale-coord-fixture");
        let project = tmp.path().join("proj-a");
        let ac_root = project.join(".ac");
        let team_dir = ac_root.join("_team_test-team");
        let wg_dir = ac_root.join("wg-1-test-team");
        let alpha_matrix = ac_root.join("_agent_test-alpha");
        let beta_matrix = ac_root.join("_agent_test-beta");
        let alpha_replica = wg_dir.join("__agent_test-alpha");
        let beta_replica = wg_dir.join("__agent_test-beta");

        for dir in [
            &team_dir,
            &alpha_matrix,
            &beta_matrix,
            &alpha_replica,
            &beta_replica,
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }

        // Stale absolute path (the `renamed-workspace` folder is never created).
        let stale_coord = tmp
            .path()
            .join("renamed-workspace")
            .join(".ac")
            .join("_agent_test-alpha");
        let stale_coord_str = stale_coord.to_string_lossy().replace('\\', "/");

        // Team config: coordinator points to the stale absolute path.
        std::fs::write(
            team_dir.join("config.json"),
            format!(r#"{{"coordinator":"{}"}}"#, stale_coord_str),
        )
        .unwrap();

        // Test-alpha replica identity: also stale. If `replica_spoofs_identity`,
        // points to a DIFFERENT stale matrix (the test-beta slot) — must reject.
        let alpha_identity = if replica_spoofs_identity {
            tmp.path()
                .join("renamed-workspace")
                .join(".ac")
                .join("_agent_test-beta")
                .to_string_lossy()
                .replace('\\', "/")
        } else {
            stale_coord_str.clone()
        };
        std::fs::write(
            alpha_replica.join("config.json"),
            format!(r#"{{"identity":"{}"}}"#, alpha_identity),
        )
        .unwrap();

        // Test-beta replica identity: stale, points at the test-beta matrix.
        // Not the coordinator — must not match.
        let beta_identity = tmp
            .path()
            .join("renamed-workspace")
            .join(".ac")
            .join("_agent_test-beta")
            .to_string_lossy()
            .replace('\\', "/");
        std::fs::write(
            beta_replica.join("config.json"),
            format!(r#"{{"identity":"{}"}}"#, beta_identity),
        )
        .unwrap();

        (tmp, ac_root, wg_dir)
    }

    /// Stale absolute identity refs are accepted only by repairing them
    /// to the same-workspace local matrix with the same agent basename.
    #[test]
    fn resolve_wg_coordinator_replica_repairs_stale_absolute_refs() {
        let (_tmp, ac_root, wg_dir) = make_stale_coordinator_fixture(false);

        let resolved = resolve_wg_coordinator_replica(&ac_root, &wg_dir)
            .expect("same-workspace repair should resolve coordinator with stale refs");

        assert_eq!(resolved.agent_name, "test-alpha");
        assert_eq!(resolved.team, "test-team");
        assert_eq!(resolved.wg_name, "wg-1-test-team");
        let repaired_config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(wg_dir.join("__agent_test-alpha").join("config.json"))
                .expect("read repaired config"),
        )
        .expect("parse repaired config");
        assert_eq!(repaired_config["identity"], "../../_agent_test-alpha");
    }

    /// #299: even when refs are stale, spoofing must still be rejected — a
    /// replica that declares a stale identity ref naming a DIFFERENT matrix
    /// (e.g. `_agent_test-beta`) must not be accepted as coordinator.
    #[test]
    fn resolve_wg_coordinator_replica_rejects_spoofed_stale_identity() {
        let (_tmp, ac_root, wg_dir) = make_stale_coordinator_fixture(true);

        // The test-alpha replica has been spoofed to claim the test-beta matrix.
        // The test-beta replica claims the test-beta matrix too. Neither matches
        // the team's coordinator ref (`_agent_test-alpha`), so no coordinator
        // is resolved.
        assert!(resolve_wg_coordinator_replica(&ac_root, &wg_dir).is_none());
    }

    /// Relative identity traversing out of the workspace must be repaired
    /// to the same-workspace local matrix, not compared against the stale target.
    #[test]
    fn resolve_wg_coordinator_replica_repairs_stale_relative_identity() {
        let tmp = FixtureRoot::new("teams-stale-rel-fixture");
        let project = tmp.path().join("proj-a");
        let ac_root = project.join(".ac");
        let team_dir = ac_root.join("_team_test-team");
        let wg_dir = ac_root.join("wg-1-test-team");
        let alpha_matrix = ac_root.join("_agent_test-alpha");
        let alpha_replica = wg_dir.join("__agent_test-alpha");

        for dir in [&team_dir, &alpha_matrix, &alpha_replica] {
            std::fs::create_dir_all(dir).unwrap();
        }

        // Team config: stale absolute coordinator path
        // -> tmp/renamed-workspace/.ac/_agent_test-alpha
        let stale_abs = tmp
            .path()
            .join("renamed-workspace")
            .join(".ac")
            .join("_agent_test-alpha");
        let stale_abs_str = stale_abs.to_string_lossy().replace('\\', "/");
        std::fs::write(
            team_dir.join("config.json"),
            format!(r#"{{"coordinator":"{}"}}"#, stale_abs_str),
        )
        .unwrap();

        // Replica identity: relative path that traverses out to the same
        // legacy folder. From `<tmp>/proj-a/.ac/wg-1-test-team/__agent_test-alpha`,
        // `../../../../renamed-workspace/.ac/_agent_test-alpha` resolves to
        // `<tmp>/renamed-workspace/.ac/_agent_test-alpha` — the same logical
        // target as the team coordinator ref.
        std::fs::write(
            alpha_replica.join("config.json"),
            r#"{"identity":"../../../../renamed-workspace/.ac/_agent_test-alpha"}"#,
        )
        .unwrap();

        let resolved = resolve_wg_coordinator_replica(&ac_root, &wg_dir)
            .expect("same-workspace repair should resolve stale relative identity");
        assert_eq!(resolved.agent_name, "test-alpha");
    }

    // ── #299 logical_path_resolve unit tests ──────────────────────────

    #[test]
    fn logical_path_resolve_resolves_parent_dirs() {
        let p = Path::new("/a/b/c/../d");
        assert_eq!(logical_path_resolve(p), "/a/b/d");
    }

    #[test]
    fn logical_path_resolve_drops_parent_above_root() {
        let p = Path::new("/a/../../b");
        // `/a/..` → `/`, then `/..` is dropped (can't go above root).
        assert_eq!(logical_path_resolve(p), "/b");
    }

    #[test]
    fn logical_path_resolve_preserves_relative_parent_prefix() {
        let p = Path::new("../../foo");
        assert_eq!(logical_path_resolve(p), "../../foo");
    }

    #[test]
    fn logical_path_resolve_strips_current_dir() {
        let p = Path::new("/a/./b/./c");
        assert_eq!(logical_path_resolve(p), "/a/b/c");
    }

    // ── #299 identity_compare_key unit tests ──────────────────────────

    #[test]
    fn identity_compare_key_equal_for_stale_paths_pointing_to_same_target() {
        // Two refs expressed differently but logically pointing at the same
        // non-existent legacy location should produce equal keys.
        let a = Path::new("/some/where/legacy/.ac/_agent_test-alpha");
        let b = Path::new("/some/where/proj/.ac/wg-1-test-team/__agent_test-alpha/../../../../legacy/.ac/_agent_test-alpha");
        assert_eq!(identity_compare_key(a), identity_compare_key(b));
    }

    #[test]
    fn identity_compare_key_differs_for_different_matrix_names() {
        // Stale refs to different matrix dirs must produce different keys —
        // protects spoofing when both refs are stale.
        let a = Path::new("/some/where/legacy/.ac/_agent_test-alpha");
        let b = Path::new("/some/where/legacy/.ac/_agent_test-beta");
        assert_ne!(identity_compare_key(a), identity_compare_key(b));
    }

    #[test]
    fn identity_compare_key_case_insensitive() {
        // Comparison is case-insensitive (Windows convention; matches
        // cli/list_peers::norm_path).
        let a = Path::new("/Some/Where/.ac/_Agent_Test-Alpha");
        let b = Path::new("/some/where/.ac/_agent_test-alpha");
        assert_eq!(identity_compare_key(a), identity_compare_key(b));
    }

    #[test]
    fn verified_wg_coordinator_target_rejects_origin_coordinator() {
        let (_tmp, paths) = make_coordinator_fixture(false);

        assert!(verified_wg_coordinator_target("proj-a/tech-lead", &paths).is_none());
    }

    #[test]
    fn verified_wg_coordinator_target_rejects_wrong_wg_member() {
        let (_tmp, paths) = make_coordinator_fixture(false);

        assert!(verified_wg_coordinator_target("proj-a:wg-1-dev-team/dev-rust", &paths).is_none());
    }

    #[test]
    fn verified_wg_coordinator_target_accepts_identity_verified_coordinator() {
        let (_tmp, paths) = make_coordinator_fixture(false);

        let resolved = verified_wg_coordinator_target("proj-a:wg-1-dev-team/tech-lead", &paths)
            .expect("verified coordinator");

        assert_eq!(resolved.agent_name, "tech-lead");
        assert_eq!(resolved.team, "dev-team");
    }

    #[test]
    fn verified_wg_coordinator_target_accepts_portable_ref_with_external_identity() {
        let (_tmp, _ac_root, _wg_dir, paths) = make_portable_coordinator_fixture(false);

        let resolved = verified_wg_coordinator_target("proj-a:wg-1-dev-team/tech-lead", &paths)
            .expect("portable verified coordinator");

        assert_eq!(resolved.agent_name, "tech-lead");
        assert_eq!(resolved.team, "dev-team");
    }

    #[test]
    fn resolve_agent_target_passes_through_qualified() {
        let (_tmp, paths) = make_project_fixture(&[("proj-a", &[("wg-1-devs", &["alice"])])]);
        let fqn = "proj-a:wg-1-devs/alice";
        assert_eq!(resolve_agent_target(fqn, &paths).unwrap(), fqn);
    }

    #[test]
    fn resolve_agent_target_qualifies_unambiguous_unqualified() {
        let (_tmp, paths) = make_project_fixture(&[("proj-a", &[("wg-1-devs", &["alice"])])]);
        let unqualified = "wg-1-devs/alice";
        assert_eq!(
            resolve_agent_target(unqualified, &paths).unwrap(),
            "proj-a:wg-1-devs/alice"
        );
    }

    #[test]
    fn resolve_agent_target_rejects_ambiguous() {
        let (_tmp, paths) = make_project_fixture(&[
            ("proj-a", &[("wg-1-devs", &["alice"])]),
            ("proj-b", &[("wg-1-devs", &["alice"])]),
        ]);
        let err = resolve_agent_target("wg-1-devs/alice", &paths).unwrap_err();
        match err {
            ResolutionError::Ambiguous { target, candidates } => {
                assert_eq!(target, "wg-1-devs/alice");
                assert!(candidates.contains(&"proj-a:wg-1-devs/alice".to_string()));
                assert!(candidates.contains(&"proj-b:wg-1-devs/alice".to_string()));
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected Ambiguous, got {:?}", other),
        }
    }

    #[test]
    fn resolve_agent_target_rejects_unknown() {
        let (_tmp, paths) = make_project_fixture(&[("proj-a", &[("wg-1-devs", &["alice"])])]);
        // Qualified-but-missing.
        assert!(matches!(
            resolve_agent_target("proj-c:wg-1-devs/alice", &paths).unwrap_err(),
            ResolutionError::UnknownQualified(_)
        ));
        // Unqualified, zero candidates.
        assert!(matches!(
            resolve_agent_target("wg-9-none/nobody", &paths).unwrap_err(),
            ResolutionError::NoMatch(_)
        ));
        // Invalid shape: empty.
        assert!(matches!(
            resolve_agent_target("", &paths).unwrap_err(),
            ResolutionError::InvalidShape(_)
        ));
        // Invalid shape: double colon.
        assert!(matches!(
            resolve_agent_target("a:b:wg-1/x", &paths).unwrap_err(),
            ResolutionError::InvalidShape(_)
        ));
        // Invalid shape: qualified with non-WG local.
        assert!(matches!(
            resolve_agent_target("proj-a:not-wg/alice", &paths).unwrap_err(),
            ResolutionError::InvalidShape(_)
        ));
    }

    /// §DR4: `project_paths` entry is a parent dir containing sibling projects.
    #[test]
    fn resolve_agent_target_two_level_scan() {
        let tmp = FixtureRoot::new("teams-two-level");
        // Lay out: tmp/ contains proj-a/ and proj-b/, each with a .ac/ + colliding replica.
        for proj in ["proj-a", "proj-b"] {
            let replica = tmp
                .path()
                .join(proj)
                .join(".ac")
                .join("wg-1-devs")
                .join("__agent_alice");
            std::fs::create_dir_all(&replica).unwrap();
        }
        // project_paths = [tmp] (parent only — must descend one level).
        let paths = vec![tmp.path().to_string_lossy().to_string()];
        let err = resolve_agent_target("wg-1-devs/alice", &paths).unwrap_err();
        match err {
            ResolutionError::Ambiguous { candidates, .. } => {
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected Ambiguous (two-level scan), got {:?}", other),
        }
    }

    // Origin-form and bare inputs pass through.
    #[test]
    fn resolve_agent_target_origin_and_bare_passthrough() {
        let (_tmp, paths) = make_project_fixture(&[]);
        assert_eq!(
            resolve_agent_target("some-project/agent", &paths).unwrap(),
            "some-project/agent"
        );
        assert_eq!(
            resolve_agent_target("bare-agent", &paths).unwrap(),
            "bare-agent"
        );
    }

    /// Issue #134: filesystem directory names like `__agent_shipper` are never
    /// valid peer FQNs. They must reject early with `LooksLikeFilesystemDir`
    /// regardless of where they appear (bare, after a project prefix, after a
    /// WG-local prefix, or fully qualified).
    #[test]
    fn resolve_agent_target_rejects_filesystem_dir_names() {
        let (_tmp, paths) = make_project_fixture(&[]);
        for bad in [
            "__agent_shipper",
            "__agent_dev-rust",
            "_agent_architect",
            "_agent_tech-lead",
            "some-project/__agent_shipper",
            "wg-1-devs/__agent_alice",
            "proj-a:wg-1-devs/__agent_alice",
        ] {
            match resolve_agent_target(bad, &paths) {
                Err(ResolutionError::LooksLikeFilesystemDir(s)) => assert_eq!(s, bad),
                other => panic!(
                    "expected LooksLikeFilesystemDir for {:?}, got {:?}",
                    bad, other
                ),
            }
        }
    }

    #[test]
    fn resolve_agent_target_accepts_root_agent_uri_verbatim() {
        let paths: Vec<String> = vec![];
        assert_eq!(
            resolve_agent_target(crate::config::root_agent::ROOT_AGENT_SENDER, &paths).unwrap(),
            crate::config::root_agent::ROOT_AGENT_SENDER
        );
    }

    #[test]
    fn resolve_agent_target_still_rejects_other_multi_colon_strings() {
        let paths: Vec<String> = vec![];
        assert!(matches!(
            resolve_agent_target("a:b:wg-1/x", &paths),
            Err(ResolutionError::InvalidShape(_))
        ));
    }

    /// Validation #16: `is_coordinator_for_cwd` correctness guard.
    /// Live sessions always run inside WG replica dirs (`wg-*/__agent_*`); the function
    /// consumes those via `agent_name_from_path` + WG-aware `is_coordinator`.
    #[test]
    fn is_coordinator_for_cwd_matches_wg_replica() {
        let teams = vec![DiscoveredTeam {
            name: "dev-team".into(),
            project: "foo".into(),
            agent_names: vec!["foo/dev-rust".into()],
            agent_paths: vec![None],
            coordinator_name: Some("foo/tech-lead".into()),
            coordinator_path: None,
        }];

        // Coordinator replica (any WG of the same team) resolves true.
        let coord_cwd = "C:/repos/foo/.ac/wg-4-dev-team/__agent_tech-lead";
        assert!(is_coordinator_for_cwd(coord_cwd, &teams));

        // Non-coordinator member of the team → false.
        let member_cwd = "C:/repos/foo/.ac/wg-4-dev-team/__agent_dev-rust";
        assert!(!is_coordinator_for_cwd(member_cwd, &teams));

        // Unrelated agent outside any team → false.
        let other_cwd = "C:/repos/foo/.ac/wg-9-other-team/__agent_dev-rust";
        assert!(!is_coordinator_for_cwd(other_cwd, &teams));
    }

    /// Empty teams list → nothing is a coordinator.
    #[test]
    fn is_coordinator_for_cwd_empty_teams() {
        let teams: Vec<DiscoveredTeam> = vec![];
        let cwd = "C:/repos/foo/.ac/wg-1-dev-team/__agent_tech-lead";
        assert!(!is_coordinator_for_cwd(cwd, &teams));
    }

    // ── Team-membership tests (AR2-tests 8-11) ──

    fn dev_team(project: &str) -> DiscoveredTeam {
        DiscoveredTeam {
            name: "dev-team".into(),
            project: project.into(),
            agent_names: vec![format!("{}/dev-rust", project)],
            agent_paths: vec![None],
            coordinator_name: Some(format!("{}/tech-lead", project)),
            coordinator_path: None,
        }
    }

    /// §DR7: WG-aware `is_in_team` must not cross project boundaries when
    /// both sides are qualified.
    #[test]
    fn is_in_team_rejects_cross_project_wg_match() {
        let team_a = dev_team("proj-a");
        let team_b = dev_team("proj-b");
        let agent_in_a = "proj-a:wg-1-dev-team/dev-rust";
        assert!(is_in_team(agent_in_a, &team_a));
        assert!(!is_in_team(agent_in_a, &team_b));
    }

    /// §DR7: agents in colliding same-named WG teams across projects MUST NOT
    /// communicate via the same-WG rule.
    #[test]
    fn can_communicate_rejects_cross_project_same_wg() {
        let team_a = dev_team("proj-a");
        let team_b = dev_team("proj-b");
        let teams = vec![team_a, team_b];
        let from = "proj-a:wg-1-dev-team/alice";
        let to = "proj-b:wg-1-dev-team/bob";
        assert!(!can_communicate(from, to, &teams));
    }

    /// §DR7: lenient tolerance for legacy-unqualified names — unqualified
    /// pairs on the same WG can still communicate during the migration window.
    #[test]
    fn can_communicate_allows_legacy_unqualified() {
        let teams = vec![dev_team("proj-a")];
        let from = "wg-1-dev-team/alice";
        let to = "wg-1-dev-team/bob";
        assert!(can_communicate(from, to, &teams));
    }

    /// §DR7: `is_coordinator_for_cwd` resolves project from the CWD so
    /// coordinators in different projects with same-named teams are isolated.
    #[test]
    fn is_coordinator_for_cwd_project_qualified() {
        let teams = vec![dev_team("proj-a"), dev_team("proj-b")];
        // tech-lead of proj-a's dev-team.
        let coord_a_cwd = "C:/repos/proj-a/.ac/wg-1-dev-team/__agent_tech-lead";
        // tech-lead of proj-b's dev-team.
        let coord_b_cwd = "C:/repos/proj-b/.ac/wg-1-dev-team/__agent_tech-lead";
        assert!(is_coordinator_for_cwd(coord_a_cwd, &teams));
        assert!(is_coordinator_for_cwd(coord_b_cwd, &teams));
    }

    /// Issue #77 regression guard: `is_any_coordinator` is the hot path used by
    /// `commands::ac_discovery` to populate `AcAgentReplica.isCoordinator`. The
    /// §AR2-strict gate in `is_coordinator` requires a project-qualified FQN —
    /// callers that pass an unqualified WG-local name will silently get `false`
    /// (which is exactly the bug fixed in #77). This test pins the contract so
    /// no future refactor can re-introduce the regression.
    #[test]
    fn is_any_coordinator_requires_qualified_fqn() {
        let teams = vec![dev_team("foo")];

        // 1. Project-qualified WG replica matching the team's project → true.
        assert!(is_any_coordinator("foo:wg-1-dev-team/tech-lead", &teams));

        // 2. Unqualified WG replica (legacy shape) → false. §AR2-strict guard.
        assert!(!is_any_coordinator("wg-1-dev-team/tech-lead", &teams));

        // 3. Cross-project qualified → false (project mismatch).
        assert!(!is_any_coordinator("bar:wg-1-dev-team/tech-lead", &teams));
    }

    /// §AR2-strict: unqualified `from` (legacy) MUST NOT grant coordinator
    /// authority even if the local part matches. Locks in the §DR8/§G13 call.
    #[test]
    fn is_coordinator_rejects_legacy_unqualified_from() {
        let teams = [dev_team("proj-a")];
        // Legacy-unqualified name — local part matches the team coordinator, but
        // with no project prefix the strict rule rejects.
        assert!(!is_coordinator("wg-1-dev-team/tech-lead", &teams[0]));
        // For completeness, the fully-qualified form DOES grant authority.
        assert!(is_coordinator("proj-a:wg-1-dev-team/tech-lead", &teams[0]));
    }

    /// #280 §3.4 — `note_missing_team_config` is a process-local one-shot
    /// dedup keyed on `(project, team_dir)`. First sighting returns true so
    /// the caller emits the INFO; later sightings return false so the WARN
    /// storm collapses to a single line per unique pair per process.
    #[test]
    fn note_missing_team_config_returns_true_first_time_only() {
        // Use unique pair to avoid collisions with other tests' state — the
        // helper's HashSet is process-global.
        let project = "proj-test-280-3-4";
        let dir = "_team_foo_test_280_3_4";
        assert!(note_missing_team_config(project, dir));
        assert!(!note_missing_team_config(project, dir));
        // A different pair is independent.
        let dir2 = "_team_bar_test_280_3_4";
        assert!(note_missing_team_config(project, dir2));
        assert!(!note_missing_team_config(project, dir2));
    }

    // ──────────────────────────────────────────────────────────────────────
    // #1535 walker confinement (plan §6, T1-T7)
    // ──────────────────────────────────────────────────────────────────────

    /// #1535 T1: the exact bug shape — a nested project's origin matrix dir
    /// under an ancestor workgroup replica. The only `__agent_*` ancestor
    /// (`__agent_alice`) belongs to the ANCESTOR project's `.ac`, so it must
    /// not bind: the walker returns `Ok(None)` and the sessionRace gate is
    /// skipped (pre-fix: `Ok(Some(alice))` → false-positive duplicate).
    #[test]
    fn strict_wg_replica_anchor_from_cwd_nested_project_origin_cwd_binds_nothing() {
        let temp = tempfile::TempDir::new().unwrap();
        let nested_origin = temp
            .path()
            .join("proj-outer")
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_alice")
            .join("nested-proj")
            .join(".ac")
            .join("_agent_phase0");
        std::fs::create_dir_all(&nested_origin).unwrap();

        let anchor = strict_wg_replica_anchor_from_cwd(&nested_origin)
            .expect("walk must not error");
        assert_eq!(
            anchor, None,
            "the ancestor project's replica must never anchor a nested-project cwd"
        );
    }

    /// #1535 T2: a nested project with its OWN replica anchors that replica,
    /// not the ancestor's.
    #[test]
    fn strict_wg_replica_anchor_from_cwd_nested_project_replica_binds_own_anchor() {
        let temp = tempfile::TempDir::new().unwrap();
        let alice = temp
            .path()
            .join("proj-outer")
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_alice");
        let bob = alice
            .join("nested-proj")
            .join(".ac")
            .join("wg-2-devs")
            .join("__agent_bob");
        for dir in [&alice, &bob] {
            std::fs::create_dir_all(dir).unwrap();
        }

        let anchor = strict_wg_replica_anchor_from_cwd(&bob).expect("walk must not error");
        let anchor = anchor.expect("the nested project's own replica must bind");
        assert_eq!(
            std::fs::canonicalize(&anchor).unwrap(),
            std::fs::canonicalize(&bob).unwrap(),
            "the nested replica must anchor itself, not the ancestor replica"
        );
        assert!(
            !anchor.ends_with("__agent_alice"),
            "the ancestor replica must not anchor a nested-project cwd"
        );
    }

    /// #1535 T3 regression: an ordinary replica cwd still anchors itself.
    #[test]
    fn strict_wg_replica_anchor_from_cwd_own_replica_regression() {
        let temp = tempfile::TempDir::new().unwrap();
        let alice = temp
            .path()
            .join("proj")
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_alice");
        std::fs::create_dir_all(&alice).unwrap();

        let anchor =
            strict_wg_replica_anchor_from_cwd(&alice).expect("walk must not error");
        assert_eq!(
            std::fs::canonicalize(anchor.expect("own replica must bind")).unwrap(),
            std::fs::canonicalize(&alice).unwrap()
        );
    }

    /// #1535 T4 regression: a deeper cwd inside an ordinary replica still
    /// resolves to the same own anchor.
    #[test]
    fn strict_wg_replica_anchor_from_cwd_deeper_cwd_inside_own_replica_regression() {
        let temp = tempfile::TempDir::new().unwrap();
        let deep = temp
            .path()
            .join("proj")
            .join(".ac")
            .join("wg-1-devs")
            .join("__agent_alice")
            .join("some")
            .join("deep")
            .join("subdir");
        std::fs::create_dir_all(&deep).unwrap();

        let anchor = strict_wg_replica_anchor_from_cwd(&deep).expect("walk must not error");
        assert_eq!(
            std::fs::canonicalize(anchor.expect("deeper cwd must bind the own replica"))
            .unwrap(),
            std::fs::canonicalize(
                temp.path()
                    .join("proj")
                    .join(".ac")
                    .join("wg-1-devs")
                    .join("__agent_alice")
            )
            .unwrap()
        );
    }

    /// #1535 T5/T6/T7 fixture: `strict_target_fixture`-shaped outer project
    /// (`proj/.ac/{_team_team, _agent_lead, _agent_dev-one, wg-1-team/...}`)
    /// plus a nested project with its own full workgroup inside
    /// `__agent_dev-one`: `nested-proj/.ac/{_team_team, _agent_lead2,
    /// _agent_inner2, wg-2-team/{__agent_lead2, __agent_inner2}}` and an
    /// origin-only dir `_agent_phase0` (the #1535 bug shape). The nested
    /// replica `config.json` identity is `../../_agent_inner2`; the nested
    /// team config names the nested origin matrices.
    fn nested_strict_target_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::TempDir::new().unwrap();
        let project = temp.path().join("proj");
        let ac_root = project.join(".ac");
        let team = ac_root.join("_team_team");
        let wg = ac_root.join("wg-1-team");
        let outer_dev_one = wg.join("__agent_dev-one");
        for directory in [
            &team,
            &ac_root.join("_agent_lead"),
            &ac_root.join("_agent_dev-one"),
            &wg.join("__agent_lead"),
            &outer_dev_one,
        ] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(
            team.join("config.json"),
            r#"{"agents":["../_agent_dev-one","../_agent_lead"],"coordinator":"../_agent_lead"}"#,
        )
        .unwrap();
        for (replica, identity) in [
            (&wg.join("__agent_lead"), "../../_agent_lead"),
            (&outer_dev_one, "../../_agent_dev-one"),
        ] {
            std::fs::write(
                replica.join("config.json"),
                format!(r#"{{"identity":"{identity}"}}"#),
            )
            .unwrap();
        }

        let nested_ac_root = outer_dev_one.join("nested-proj").join(".ac");
        let nested_team = nested_ac_root.join("_team_team");
        let nested_wg = nested_ac_root.join("wg-2-team");
        let nested_inner2 = nested_wg.join("__agent_inner2");
        let nested_origin = nested_ac_root.join("_agent_phase0");
        for directory in [
            &nested_team,
            &nested_ac_root.join("_agent_lead2"),
            &nested_ac_root.join("_agent_inner2"),
            &nested_wg.join("__agent_lead2"),
            &nested_inner2,
            &nested_origin,
        ] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(
            nested_team.join("config.json"),
            r#"{"agents":["../_agent_lead2","../_agent_inner2"],"coordinator":"../_agent_lead2"}"#,
        )
        .unwrap();
        std::fs::write(
            nested_inner2.join("config.json"),
            r#"{"identity":"../../_agent_inner2"}"#,
        )
        .unwrap();

        (temp, nested_inner2, nested_origin)
    }

    /// #1535 T5: a nested project's replica resolves to its own verified
    /// sender identity (project/workgroup/agent of the NESTED project), not
    /// the ancestor workgroup's.
    #[test]
    fn verify_pty_input_replica_cwd_nested_replica_resolves_own_fqn() {
        let (_temp, nested_inner2, _nested_origin) = nested_strict_target_fixture();
        let identity = verify_pty_input_replica_cwd(&nested_inner2)
            .expect("nested replica must resolve its own identity");
        assert_eq!(identity.canonical_fqn, "nested-proj:wg-2-team/inner2");
        assert_eq!(identity.project, "nested-proj");
        assert_eq!(identity.workgroup, "wg-2-team");
        assert_eq!(identity.agent, "inner2");
    }

    /// #1535 T6: the #1535 bug-shaped cwd (an origin matrix dir, not a
    /// replica) resolves to `Err("sender_identity_invalid")` — and never to
    /// the outer workgroup.
    #[test]
    fn verify_pty_input_replica_cwd_nested_origin_cwd_is_invalid() {
        let (_temp, _nested_inner2, nested_origin) = nested_strict_target_fixture();
        let err = verify_pty_input_replica_cwd(&nested_origin).expect_err("origin dir must not resolve");
        assert_eq!(err, "sender_identity_invalid");
    }

    /// #1535 T7: the create-gate key derives from the NESTED project for a
    /// nested replica cwd, and is `None` (gate skipped) for the nested origin
    /// cwd.
    #[test]
    fn pty_input_create_gate_key_from_cwd_nested_replica_uses_own_fqn_and_nested_origin_is_none() {
        let (_temp, nested_inner2, nested_origin) = nested_strict_target_fixture();
        assert_eq!(
            pty_input_create_gate_key_from_cwd(&nested_inner2).expect("key must not error"),
            Some("nested-proj:wg-2-team/inner2".to_string())
        );
        assert_eq!(
            pty_input_create_gate_key_from_cwd(&nested_origin).expect("key must not error"),
            None
        );
    }
}
