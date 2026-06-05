use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::config::replica_identity::{
    agent_bare_name_from_ref, read_and_repair_wg_replica_config, WG_REPLICA_REQUIRED_CONTEXT,
};
use crate::config::workspace::{existing_workspace_dir, find_workspace_segment, has_workspace_dir};

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

    if let Some(ac_idx) = find_workspace_segment(&parts) {
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
        if has_workspace_dir(base) {
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
                if has_workspace_dir(&p) {
                    out.push((name.to_string(), p));
                }
            }
        }
    }
    out
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
    workspace_dir: &Path,
    wg_dir: &Path,
) -> Option<WgCoordinatorReplica> {
    let project = workspace_dir.parent()?.file_name()?.to_str()?.to_string();
    let wg_name = wg_dir.file_name()?.to_str()?.to_string();
    let team = wg_name
        .strip_prefix("wg-")
        .and_then(|s| s.split_once('-').map(|(_, rest)| rest.to_string()))?;

    let team_dir = workspace_dir.join(format!("_team_{}", team));
    let team_config: serde_json::Value = std::fs::read_to_string(team_dir.join("config.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())?;
    let coordinator_ref = team_config.get("coordinator").and_then(|c| c.as_str())?;
    let coordinator_name = agent_bare_name_from_ref(coordinator_ref).ok()?;
    if !workspace_dir
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
        let Some(workspace_dir) = existing_workspace_dir(&project_dir) else {
            continue;
        };
        let wg_dir = workspace_dir.join(wg_name);
        if !wg_dir.is_dir() {
            continue;
        }
        if let Some(resolved) = resolve_wg_coordinator_replica(&workspace_dir, &wg_dir) {
            if resolved.agent_name == agent_name {
                return Some(resolved);
            }
        }
    }

    None
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
            let Some(workspace_dir) = existing_workspace_dir(&dir) else {
                continue;
            };
            let candidate = workspace_dir.join(wg).join(&replica_dir);
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
            let Some(workspace_dir) = existing_workspace_dir(&dir) else {
                continue;
            };
            let candidate = workspace_dir.join(wg).join(&replica_dir);
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
        let origin = find_workspace_segment(&parts)
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

/// Resolve an agent ref to an absolute path given the workspace directory.
fn resolve_agent_path(workspace_dir: &Path, agent_ref: &str) -> Option<PathBuf> {
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

    // Relative to the workspace directory.
    let candidate = workspace_dir.join(trimmed);
    if candidate.is_dir() {
        return Some(candidate);
    }

    // Try parent of the workspace directory (project root).
    if let Some(project_root) = workspace_dir.parent() {
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
/// Scans settings.project_paths (and immediate children) for workspace `_team_*/config.json`.
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

    log::info!(
        "[teams] discovered {} team(s) across {} project path(s)",
        teams.len(),
        settings.project_paths.len()
    );
    teams
}

/// Discover teams in a single project directory.
fn discover_teams_in_project(project_dir: &Path, teams: &mut Vec<DiscoveredTeam>) {
    let Some(workspace_dir) = existing_workspace_dir(project_dir) else {
        return;
    };

    let project_folder = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let entries = match std::fs::read_dir(&workspace_dir) {
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
                // #280 §3.4 — NotFound is an expected state for half-installed
                // team dirs (`_team_foo/` created but no `config.json` yet).
                // Logging WARN at every discovery sweep spams app.log; downgrade
                // NotFound to DEBUG and emit a one-shot INFO per (project,
                // team_dir) per process so the operator still sees the visit.
                // Unexpected IO errors stay at WARN.
                match e.kind() {
                    std::io::ErrorKind::NotFound => {
                        if note_missing_team_config(&project_folder, dir_name) {
                            log::info!(
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
                let path = resolve_agent_path(&workspace_dir, r);
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
            .and_then(|r| resolve_agent_path(&workspace_dir, r));

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
            let workspace_dir = proj_dir.join(".ac");
            std::fs::create_dir_all(&workspace_dir).unwrap();
            for (wg_name, agents) in *wgs {
                let wg_dir = workspace_dir.join(wg_name);
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
        let workspace_dir = project.join(".ac");
        let team_dir = workspace_dir.join("_team_dev-team");
        let origin_tech_lead = workspace_dir.join("_agent_tech-lead");
        let origin_dev_rust = workspace_dir.join("_agent_dev-rust");
        let wg_dir = workspace_dir.join("wg-1-dev-team");
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
            r#"{"agents":["../_agent_dev-rust"],"coordinator":"../_agent_tech-lead"}"#,
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

    fn make_portable_coordinator_fixture(
        spoofed_coordinator_identity: bool,
    ) -> (FixtureRoot, PathBuf, PathBuf, Vec<String>) {
        let tmp = FixtureRoot::new("teams-portable-coord-fixture");
        let project = tmp.path().join("proj-a");
        let workspace_dir = project.join(".ac");
        let team_dir = workspace_dir.join("_team_dev-team");
        let local_tech_lead = workspace_dir.join("_agent_tech-lead");
        let local_dev_rust = workspace_dir.join("_agent_dev-rust");
        let origin = tmp.path().join("origin-matrix").join(".ac");
        let origin_tech_lead = origin.join("_agent_tech-lead");
        let origin_dev_rust = origin.join("_agent_dev-rust");
        let wg_dir = workspace_dir.join("wg-1-dev-team");
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
        (tmp, workspace_dir, wg_dir, paths)
    }

    #[test]
    fn resolve_wg_coordinator_replica_uses_identity_not_dir_name() {
        let (tmp, _paths) = make_coordinator_fixture(false);
        let workspace_dir = tmp.path().join("proj-a").join(".ac");
        let wg_dir = workspace_dir.join("wg-1-dev-team");

        let resolved =
            resolve_wg_coordinator_replica(&workspace_dir, &wg_dir).expect("coordinator");

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
        let workspace_dir = tmp.path().join("proj-a").join(".ac");
        let wg_dir = workspace_dir.join("wg-1-dev-team");

        assert!(resolve_wg_coordinator_replica(&workspace_dir, &wg_dir).is_none());
    }

    #[test]
    fn resolve_wg_coordinator_replica_accepts_portable_ref_with_external_identity() {
        let (_tmp, workspace_dir, wg_dir, _paths) = make_portable_coordinator_fixture(false);

        let resolved = resolve_wg_coordinator_replica(&workspace_dir, &wg_dir)
            .expect("portable coordinator ref should match declared identity agent");

        assert_eq!(resolved.project, "proj-a");
        assert_eq!(resolved.team, "dev-team");
        assert_eq!(resolved.wg_name, "wg-1-dev-team");
        assert_eq!(resolved.agent_name, "tech-lead");
    }

    #[test]
    fn resolve_wg_coordinator_replica_rejects_portable_ref_with_spoofed_identity() {
        let (_tmp, workspace_dir, wg_dir, _paths) = make_portable_coordinator_fixture(true);

        assert!(resolve_wg_coordinator_replica(&workspace_dir, &wg_dir).is_none());
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
        let workspace_dir = project.join(".ac");
        let team_dir = workspace_dir.join("_team_test-team");
        let wg_dir = workspace_dir.join("wg-1-test-team");
        let alpha_matrix = workspace_dir.join("_agent_test-alpha");
        let beta_matrix = workspace_dir.join("_agent_test-beta");
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

        (tmp, workspace_dir, wg_dir)
    }

    /// Stale absolute identity refs are accepted only by repairing them
    /// to the same-workspace local matrix with the same agent basename.
    #[test]
    fn resolve_wg_coordinator_replica_repairs_stale_absolute_refs() {
        let (_tmp, workspace_dir, wg_dir) = make_stale_coordinator_fixture(false);

        let resolved = resolve_wg_coordinator_replica(&workspace_dir, &wg_dir)
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
        let (_tmp, workspace_dir, wg_dir) = make_stale_coordinator_fixture(true);

        // The test-alpha replica has been spoofed to claim the test-beta matrix.
        // The test-beta replica claims the test-beta matrix too. Neither matches
        // the team's coordinator ref (`_agent_test-alpha`), so no coordinator
        // is resolved.
        assert!(resolve_wg_coordinator_replica(&workspace_dir, &wg_dir).is_none());
    }

    /// Relative identity traversing out of the workspace must be repaired
    /// to the same-workspace local matrix, not compared against the stale target.
    #[test]
    fn resolve_wg_coordinator_replica_repairs_stale_relative_identity() {
        let tmp = FixtureRoot::new("teams-stale-rel-fixture");
        let project = tmp.path().join("proj-a");
        let workspace_dir = project.join(".ac");
        let team_dir = workspace_dir.join("_team_test-team");
        let wg_dir = workspace_dir.join("wg-1-test-team");
        let alpha_matrix = workspace_dir.join("_agent_test-alpha");
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

        let resolved = resolve_wg_coordinator_replica(&workspace_dir, &wg_dir)
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
        let (_tmp, _workspace, _wg_dir, paths) = make_portable_coordinator_fixture(false);

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
}
