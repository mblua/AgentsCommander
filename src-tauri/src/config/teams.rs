use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::config::ac_root::{find_ac_root_segment, has_ac_root};

mod discovery;
mod membership;
mod replica;
mod routes;

#[cfg(all(test, windows))]
pub(crate) use discovery::create_test_junction;
pub use discovery::discover_teams;
pub(crate) use discovery::discover_teams_for_cli;
pub(crate) use discovery::discover_teams_from_project_paths;
#[cfg(all(test, windows))]
use discovery::discover_teams_in_project;
pub(crate) use discovery::discover_teams_in_project_filtered;
pub use membership::can_communicate;
pub use membership::is_any_coordinator;
pub use membership::is_coordinator_for_cwd;
pub use membership::is_coordinator_of;
pub use membership::is_in_team;
pub use membership::resolve_agent_target;
#[cfg(test)]
use membership::{extract_wg_team, is_coordinator};
pub use replica::resolve_wg_coordinator_replica;
pub(crate) use replica::validate_pty_input_target_syntax;
pub(crate) use replica::validate_terminal_snapshot_target_syntax;
pub use replica::verified_wg_coordinator_target;
pub use replica::PtyInputAuthorityKind;
#[cfg(test)]
use replica::TerminalSnapshotAuthorityKind;
pub(crate) use replica::TerminalSnapshotTargetIdentity;
pub use replica::VerifiedPtyInputIdentity;
pub use replica::VerifiedPtyInputRoute;
pub use replica::VerifiedTerminalSnapshotRoute;
#[cfg(test)]
use replica::{identity_compare_key, logical_path_resolve, team_members};
pub(crate) use routes::discover_verified_terminal_snapshot_targets;
pub(crate) use routes::discover_verified_terminal_snapshot_targets_counted;
pub(crate) use routes::pty_input_create_gate_key_from_cwd;
#[cfg(all(test, unix))]
use routes::resolve_pty_input_target;
pub(crate) use routes::strict_wg_replica_anchor_from_cwd;
pub use routes::verify_pty_input_coordinator_root;
pub(crate) use routes::verify_pty_input_replica_cwd;
pub use routes::verify_pty_input_route;
pub(crate) use routes::verify_terminal_snapshot_root_identity;
pub(crate) use routes::verify_terminal_snapshot_route;
pub(crate) use routes::SnapshotDiscoverySkips;

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
            if crate::config::entity_prefix::has_entity_prefix(wg)
                && agent_dir.starts_with("__agent_")
            {
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
            if crate::config::entity_prefix::has_entity_prefix(wg)
                && agent_dir.starts_with("__agent_")
            {
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
         Use the 'name' field from `list-peers-lean` (e.g. 'project:room-N-team/agent' \
         for Room replicas, 'project/agent' for origin agents). Directory names \
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
    let Some(rest) = crate::config::entity_prefix::strip_entity_prefix(prefix) else {
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

/// Where a candidate project directory came from. A caller-supplied
/// (`Configured`) path stays fail-closed; a `ScannedChild` found by walking the
/// caller's tree is skipped and counted when it fails identity verification
/// (#2228).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectOrigin {
    Configured,
    ScannedChild,
}

fn enumerate_project_dirs_strict(
    project_paths: &[String],
    skipped_children: &mut usize,
) -> Result<Vec<(String, PathBuf, ProjectOrigin)>, String> {
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
            out.push((name.to_string(), base.clone(), ProjectOrigin::Configured));
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
            // #2228: a scanned neighbour that fails identity verification is
            // irrelevant to the caller's request and can never be emitted as a
            // target, so it is skipped and counted instead of aborting the
            // whole scan. The caller-supplied path at the top of this loop
            // stays fail-closed.
            let Ok(child_identity) = crate::path_identity::verify_directory(&entry.path()) else {
                *skipped_children = skipped_children.saturating_add(1);
                continue;
            };
            let Ok(child_ac_root) = strict_project_ac_root(&child_identity.canonical_path) else {
                *skipped_children = skipped_children.saturating_add(1);
                continue;
            };
            if child_ac_root.is_some() {
                out.push((
                    name,
                    child_identity.canonical_path,
                    ProjectOrigin::ScannedChild,
                ));
                if out.len() > 1_024 {
                    return Err("invalid_target".to_string());
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
