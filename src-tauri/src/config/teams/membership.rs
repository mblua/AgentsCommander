use super::agent_fqn_from_path;
use super::agent_name_from_path;
use super::agent_segment_is_filesystem_dir;
use super::enumerate_project_dirs;
use super::is_valid_wg_local_shape;
use super::split_project_prefix;
use super::DiscoveredTeam;
use super::ResolutionError;
use crate::config::ac_root::existing_ac_root;
use crate::config::ac_root::find_ac_root_segment;
use std::path::Path;
use std::path::PathBuf;

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
pub(super) fn resolve_agent_ref(project_folder: &str, agent_ref: &str) -> String {
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
pub(super) fn resolve_agent_path(ac_root: &Path, agent_ref: &str) -> Option<PathBuf> {
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
pub(super) fn extract_wg_team(agent_name: &str) -> Option<&str> {
    let (_, local) = split_project_prefix(agent_name);
    let prefix = local.split('/').next()?;
    if !crate::config::entity_prefix::has_entity_prefix(prefix) {
        return None;
    }
    crate::config::entity_prefix::strip_entity_prefix(prefix)
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
pub(super) fn is_coordinator(agent_name: &str, team: &DiscoveredTeam) -> bool {
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
                        "[teams] is_coordinator: reject-unqualified → false — agent='{}' \
                         team='{}/{}' coord='{}' (suffix would match)",
                        agent_name,
                        team.project,
                        team.name,
                        coord_name
                    );
                }
                return false;
            };
            if wg_team == team.name
                && agent_project == team.project
                && agent_suffix(agent_name) == agent_suffix(coord_name)
            {
                log::trace!(
                    "[teams] is_coordinator: wg-aware-match → true — agent='{}' team='{}/{}' \
                     coord='{}' agent_project='{}'",
                    agent_name,
                    team.project,
                    team.name,
                    coord_name,
                    agent_project
                );
                return true;
            }
            if wg_team == team.name
                && agent_project != team.project
                && agent_suffix(agent_name) == agent_suffix(coord_name)
            {
                log::trace!(
                    "[teams] is_coordinator: reject-project-mismatch → false — agent='{}' \
                     agent_project='{}' team_project='{}' team='{}' coord='{}'",
                    agent_name,
                    agent_project,
                    team.project,
                    team.name,
                    coord_name
                );
            }
            if wg_team == team.name
                && agent_project == team.project
                && agent_suffix(agent_name) != agent_suffix(coord_name)
            {
                log::trace!(
                    "[teams] is_coordinator: reject-suffix-mismatch → false — agent='{}' \
                     team='{}/{}' coord='{}' agent_suffix='{}' coord_suffix='{}'",
                    agent_name,
                    team.project,
                    team.name,
                    coord_name,
                    agent_suffix(agent_name),
                    agent_suffix(coord_name)
                );
            }
            if wg_team == team.name
                && agent_project != team.project
                && agent_suffix(agent_name) != agent_suffix(coord_name)
            {
                log::trace!(
                    "[teams] is_coordinator: reject-both-mismatch → false — agent='{}' \
                     agent_project='{}' team_project='{}' team='{}' coord='{}' agent_suffix='{}' \
                     coord_suffix='{}'",
                    agent_name,
                    agent_project,
                    team.project,
                    team.name,
                    coord_name,
                    agent_suffix(agent_name),
                    agent_suffix(coord_name)
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

/// Resolve whether the agent running at `working_directory` is a coordinator of any discovered
/// team.
/// Thin wrapper so call sites don't have to duplicate the `agent_fqn_from_path` +
/// `is_any_coordinator` pair.
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
    if crate::config::entity_prefix::has_entity_prefix(from_local)
        && crate::config::entity_prefix::has_entity_prefix(to_local)
    {
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
