use std::path::{Path, PathBuf};

/// A team discovered from `_team_*/config.json` in `.ac-new/` project directories.
#[derive(Debug, Clone)]
pub struct DiscoveredTeam {
    pub name: String,
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

/// Resolve an agent ref (from team config) to a display name.
/// Handles relative refs like `_agent_foo` and absolute paths.
fn resolve_agent_ref(project_folder: &str, agent_ref: &str) -> String {
    let normalized = agent_ref.replace('\\', "/");
    let trimmed = normalized
        .trim_start_matches("../")
        .trim_start_matches("./");

    if trimmed.contains(':') || trimmed.starts_with('/') {
        // Absolute path: extract origin project from folder before .ac-new
        let parts: Vec<&str> = trimmed.split('/').collect();
        let origin = parts
            .iter()
            .position(|p| *p == ".ac-new")
            .and_then(|i| if i > 0 { Some(parts[i - 1]) } else { None })
            .unwrap_or(project_folder);
        let dir_name = parts.last().unwrap_or(&trimmed);
        let agent_name = dir_name
            .strip_prefix("__agent_")
            .or_else(|| dir_name.strip_prefix("_agent_"))
            .unwrap_or(dir_name);
        format!("{}/{}", origin, agent_name)
    } else {
        // Relative ref: extract last component and strip prefix
        let last = trimmed.split('/').last().unwrap_or(trimmed);
        let agent_name = last
            .strip_prefix("__agent_")
            .or_else(|| last.strip_prefix("_agent_"))
            .unwrap_or(last);
        format!("{}/{}", project_folder, agent_name)
    }
}

/// Resolve an agent ref to an absolute path given the .ac-new directory.
fn resolve_agent_path(ac_new_dir: &Path, agent_ref: &str) -> Option<PathBuf> {
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

    // Relative to .ac-new/
    let candidate = ac_new_dir.join(trimmed);
    if candidate.is_dir() {
        return Some(candidate);
    }

    // Try parent of .ac-new/ (project root)
    if let Some(project_root) = ac_new_dir.parent() {
        let candidate = project_root.join(trimmed);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    None
}

/// Extract team name from a WG-style agent name.
/// "wg-1-ac-devs/dev-rust" → Some("ac-devs")
/// "some-project/agent" → None
fn extract_wg_team(agent_name: &str) -> Option<&str> {
    let prefix = agent_name.split('/').next()?;
    if !prefix.starts_with("wg-") {
        return None;
    }
    prefix
        .strip_prefix("wg-")
        .and_then(|s| s.split_once('-').map(|(_, team)| team))
}

/// Extract agent suffix (part after '/') from an agent name.
fn agent_suffix(name: &str) -> &str {
    name.split('/').last().unwrap_or(name)
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
    // WG-aware: if agent is a WG replica belonging to this team, match by suffix
    if let Some(wg_team) = extract_wg_team(agent_name) {
        if wg_team == team.name {
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
fn is_coordinator(agent_name: &str, team: &DiscoveredTeam) -> bool {
    if let Some(ref coord_name) = team.coordinator_name {
        if agent_matches_member(agent_name, coord_name, team.coordinator_path.as_ref()) {
            return true;
        }
        // WG-aware: if agent is a WG replica of this team's coordinator, match by suffix.
        // By design, any coordinator with the same suffix in any WG of the same team name
        // has cross-WG authority. This enables coordinator replicas (e.g., wg-2/tech-lead)
        // to manage agents in teams originally defined with wg-1/tech-lead as coordinator.
        if let Some(wg_team) = extract_wg_team(agent_name) {
            if wg_team == team.name && agent_suffix(agent_name) == agent_suffix(coord_name) {
                return true;
            }
        }
    }
    false
}

/// Check if sender is a coordinator of any team that contains target as a member.
pub fn is_coordinator_of(sender: &str, target: &str, teams: &[DiscoveredTeam]) -> bool {
    teams.iter().any(|team| {
        is_coordinator(sender, team) && is_in_team(target, team)
    })
}

/// Check if an agent is a coordinator of ANY discovered team.
pub fn is_any_coordinator(agent_name: &str, teams: &[DiscoveredTeam]) -> bool {
    teams.iter().any(|t| is_coordinator(agent_name, t))
}

/// Resolve whether the agent running at `working_directory` is a coordinator of any discovered team.
/// Thin wrapper so call sites don't have to duplicate the `agent_name_from_path` + `is_any_coordinator` pair.
pub fn is_coordinator_for_cwd(working_directory: &str, teams: &[DiscoveredTeam]) -> bool {
    let agent_name = agent_name_from_path(working_directory);
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

    // Rule 2: WG-scoped (agents in the same workgroup can communicate)
    if from.starts_with("wg-") && to.starts_with("wg-") {
        let from_wg = from.split('/').next().unwrap_or("");
        let to_wg = to.split('/').next().unwrap_or("");
        if !from_wg.is_empty() && from_wg == to_wg {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Validation #16: `is_coordinator_for_cwd` correctness guard.
    /// Live sessions always run inside WG replica dirs (`wg-*/__agent_*`); the function
    /// consumes those via `agent_name_from_path` + WG-aware `is_coordinator`.
    #[test]
    fn is_coordinator_for_cwd_matches_wg_replica() {
        let teams = vec![DiscoveredTeam {
            name: "dev-team".into(),
            agent_names: vec!["wg-1-dev-team/dev-rust".into()],
            agent_paths: vec![None],
            coordinator_name: Some("wg-1-dev-team/tech-lead".into()),
            coordinator_path: None,
        }];

        // Coordinator replica (any WG of the same team) resolves true.
        let coord_cwd = "C:/repos/foo/.ac-new/wg-4-dev-team/__agent_tech-lead";
        assert!(is_coordinator_for_cwd(coord_cwd, &teams));

        // Non-coordinator member of the team → false.
        let member_cwd = "C:/repos/foo/.ac-new/wg-4-dev-team/__agent_dev-rust";
        assert!(!is_coordinator_for_cwd(member_cwd, &teams));

        // Unrelated agent outside any team → false.
        let other_cwd = "C:/repos/foo/.ac-new/wg-9-other-team/__agent_dev-rust";
        assert!(!is_coordinator_for_cwd(other_cwd, &teams));
    }

    /// Empty teams list → nothing is a coordinator.
    #[test]
    fn is_coordinator_for_cwd_empty_teams() {
        let teams: Vec<DiscoveredTeam> = vec![];
        let cwd = "C:/repos/foo/.ac-new/wg-1-dev-team/__agent_tech-lead";
        assert!(!is_coordinator_for_cwd(cwd, &teams));
    }
}

/// Discover all teams from all known project paths.
/// Scans settings.project_paths (and immediate children) for `.ac-new/_team_*/config.json`.
pub fn discover_teams() -> Vec<DiscoveredTeam> {
    let settings = crate::config::settings::load_settings();
    let mut teams = Vec::new();

    for repo_path in &settings.project_paths {
        let base = Path::new(repo_path);
        if !base.is_dir() {
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
            discover_teams_in_project(&project_dir, &mut teams);
        }
    }

    teams
}

/// Discover teams in a single project directory.
fn discover_teams_in_project(project_dir: &Path, teams: &mut Vec<DiscoveredTeam>) {
    let ac_new = project_dir.join(".ac-new");
    if !ac_new.is_dir() {
        return;
    }

    let project_folder = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let entries = match std::fs::read_dir(&ac_new) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
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
        let parsed: serde_json::Value = match std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
        {
            Some(v) => v,
            None => continue,
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
                let path = resolve_agent_path(&ac_new, r);
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
            .and_then(|r| resolve_agent_path(&ac_new, r));

        teams.push(DiscoveredTeam {
            name: team_name,
            agent_names,
            agent_paths,
            coordinator_name,
            coordinator_path,
        });
    }
}
