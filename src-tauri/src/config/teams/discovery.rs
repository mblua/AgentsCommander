use super::membership::resolve_agent_path;
use super::membership::resolve_agent_ref;
use super::note_missing_team_config;
use super::DiscoveredTeam;
use crate::config::ac_root::existing_ac_root;
use crate::filesystem::ac_discovery;
use std::path::Path;
use std::path::PathBuf;

/// Discover all teams from all known project paths.
/// Scans settings.project_paths (and immediate children) for Project AC Root `_team_*/config.json`.
pub fn discover_teams() -> Vec<DiscoveredTeam> {
    let settings = crate::config::settings::load_settings();
    discover_teams_from_project_paths(&settings.project_paths)
}

/// CLI coordinator discovery fails closed on an unreadable registered source.
/// Root/master bypass and the caller's existing empty-team denial stay unchanged.
pub(crate) fn discover_teams_for_cli() -> Vec<DiscoveredTeam> {
    match crate::config::settings::load_settings_for_cli_strict() {
        Ok(settings) => discover_teams_from_project_paths(&settings.project_paths),
        Err(error) => {
            log::error!("[cli] {error}");
            Vec::new()
        }
    }
}

/// Scan a captured registered-project slice without loading or saving settings.
pub(crate) fn discover_teams_from_project_paths(project_paths: &[String]) -> Vec<DiscoveredTeam> {
    let mut teams = Vec::new();

    for repo_path in project_paths {
        log::trace!(
            "[teams] discover_teams: scanning project_path='{}'",
            repo_path
        );
        let base = Path::new(repo_path);
        if !crate::filesystem::projects::project_path_is_dir(base) {
            log::trace!(
                "[teams] discover_teams: project_path skipped (not a directory) — path='{}'",
                repo_path
            );
            continue;
        }

        // Check base and immediate children (same pattern as ac_discovery)
        let mut dirs_to_check = vec![base.to_path_buf()];
        let _ = crate::filesystem::projects::visit_project_kind_children(base, |entry| {
            if let Ok(p) = entry {
                if crate::filesystem::projects::project_path_is_dir(&p) {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') {
                        dirs_to_check.push(p);
                    }
                }
            }
            std::ops::ControlFlow::Continue(())
        });

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
        project_paths.len()
    );
    teams
}

/// Discover teams in a single project directory.
pub(super) fn discover_teams_in_project(project_dir: &Path, teams: &mut Vec<DiscoveredTeam>) {
    discover_teams_in_project_filtered(project_dir, teams, |_| true);
}

/// Predicate runs before any followed team-directory or config read.
pub(crate) fn discover_teams_in_project_filtered(
    project_dir: &Path,
    teams: &mut Vec<DiscoveredTeam>,
    accept_team: impl Fn(&Path) -> bool,
) {
    let Some(ac_root) = existing_ac_root(project_dir) else {
        return;
    };

    let project_folder = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let entries = match ac_discovery::team_discovery_entries(&ac_root) {
        Ok(e) => e,
        Err(_) => return,
    };

    for (team_dir, entry_name) in entries.flatten() {
        log::trace!(
            "[teams] discover_teams_in_project: inspecting entry — project='{}' entry='{}'",
            project_folder,
            entry_name.to_string_lossy()
        );
        if !accept_team(&team_dir) {
            continue;
        }
        if !ac_discovery::discovery_path_is_dir(&team_dir) {
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
        let raw = match ac_discovery::read_team_discovery_config(&config_path) {
            Ok(s) => s,
            Err(e) => {
                log_team_config_read_error(&project_folder, dir_name, &config_path, &e);
                continue;
            }
        };
        let parsed: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                log::warn!(
                    "[teams] dropped team — project='{}' team_dir='{}' reason='parse_failed' \
                     err='{}' path='{}'",
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
            "[teams] discovered team — project='{}' team='{}' coord_name={:?} coord_path={:?} \
             agent_count={}",
            pushed.project,
            pushed.name,
            pushed.coordinator_name,
            pushed
                .coordinator_path
                .as_ref()
                .map(|p| p.display().to_string()),
            pushed.agent_names.len()
        );
    }
}

fn log_team_config_read_error(
    project_folder: &str,
    dir_name: &str,
    config_path: &Path,
    e: &std::io::Error,
) {
    // #280 §3.4: NotFound is an expected state for half-installed
    // team dirs (`_team_foo/` created but no `config.json` yet).
    // Logging WARN at every discovery sweep spams app.log, so both
    // the one-shot per-(project, team_dir) visit record and the
    // per-drop record below are DEBUG (#612 downgraded the prior
    // one-shot INFO). Unexpected IO errors stay at WARN.
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            if note_missing_team_config(project_folder, dir_name) {
                log::debug!(
                    "[teams] team config missing (logged once per startup) — \
                     project='{}' team_dir='{}' path='{}'",
                    project_folder,
                    dir_name,
                    config_path.display()
                );
            }
            log::debug!(
                "[teams] dropped team — project='{}' team_dir='{}' reason='not_found' \
                 path='{}'",
                project_folder,
                dir_name,
                config_path.display()
            );
        }
        _ => {
            log::warn!(
                "[teams] dropped team — project='{}' team_dir='{}' \
                 reason='read_failed' err='{}' path='{}'",
                project_folder,
                dir_name,
                e,
                config_path.display()
            );
        }
    }
}

/// Reuse the existing classified test-only junction spawn site.
#[cfg(all(test, windows))]
pub(crate) fn create_test_junction(link: &Path, target: &Path) {
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("mklink /J must run");
    assert!(
        output.status.success(),
        "mklink /J must create junction {} -> {}: stdout={} stderr={}",
        link.display(),
        target.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
