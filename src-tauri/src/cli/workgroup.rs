use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::cli::create_agent_matrix::{write_project_refresh_request, ProjectRefreshRequest};
use crate::commands::entity_creation::{
    check_workgroup_repos_dirty, clone_missing_repos_for_workgroup, create_workgroup_on_disk,
    list_workgroup_dirs, read_team_config, resolve_agent_ref, sanitize_name,
    validate_existing_name, AgentMatrixSettingsFlags, RepoAssignment, TeamConfigResult,
    WgDeleteOutcome, WorkgroupDiskCreateArgs,
};
use crate::config::projects::resolve_project_reference;

#[derive(Args)]
pub struct WorkgroupArgs {
    #[command(subcommand)]
    command: WorkgroupCommand,
}

#[derive(Subcommand)]
enum WorkgroupCommand {
    /// List workgroups in a project
    List(WorkgroupListArgs),
    /// Create an auto-numbered workgroup
    Add(WorkgroupAddArgs),
    /// Remove a workgroup
    Remove(WorkgroupRemoveArgs),
}

#[derive(Args)]
struct WorkgroupListArgs {
    #[arg(long)]
    project: String,
}

#[derive(Args)]
struct WorkgroupAddArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    team: String,
    #[arg(long)]
    title: String,
    #[arg(long)]
    coordinator: String,
    #[arg(long = "agent")]
    agents: Vec<String>,
    #[arg(long = "repo")]
    repos: Vec<String>,
    #[arg(long = "repo-agents")]
    repo_agents: Vec<String>,
    #[arg(long = "repo-exclude-agents")]
    repo_exclude_agents: Vec<String>,
}

#[derive(Args)]
struct WorkgroupRemoveArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    workgroup: String,
    #[arg(long = "force-dirty")]
    force_dirty: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkgroupListItem {
    name: String,
    team: String,
    path: String,
    has_messaging: bool,
    has_task: bool,
    replicas: Vec<String>,
}

pub fn execute(args: WorkgroupArgs) -> i32 {
    let result = match args.command {
        WorkgroupCommand::List(args) => list(args),
        WorkgroupCommand::Add(args) => add(args),
        WorkgroupCommand::Remove(args) => remove(args),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

pub(crate) fn resolve_cli_project(project: &str) -> Result<PathBuf, String> {
    let settings = crate::config::settings::load_settings_for_cli();
    let resolved =
        resolve_project_reference(&settings.project_paths, project).map_err(|e| e.to_string())?;
    Ok(resolved.path)
}

pub(crate) fn write_refresh(project_path: &Path, changed_path: &Path, name: &str, reason: &str) {
    let request = ProjectRefreshRequest {
        id: uuid::Uuid::new_v4().to_string(),
        project_path: project_path.to_string_lossy().to_string(),
        agent_path: changed_path.to_string_lossy().to_string(),
        agent_name: name.to_string(),
        reason: reason.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = write_project_refresh_request(&request) {
        eprintln!("Warning: failed to request project refresh: {}", e);
    }
}

fn list(args: WorkgroupListArgs) -> Result<(), String> {
    let project_path = resolve_cli_project(&args.project)?;
    let ac_new = project_path.join(".ac-new");
    let items: Vec<WorkgroupListItem> = list_workgroup_dirs(&ac_new)
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            let team =
                crate::commands::entity_creation::parse_team_from_workgroup_name(&name).ok()?;
            let replicas = list_replicas(&path);
            Some(WorkgroupListItem {
                name,
                team,
                path: path.to_string_lossy().to_string(),
                has_messaging: path
                    .join(crate::phone::messaging::MESSAGING_DIR_NAME)
                    .is_dir(),
                has_task: path.join("TASK.md").is_file(),
                replicas,
            })
        })
        .collect();
    print_json(&items)
}

fn add(args: WorkgroupAddArgs) -> Result<(), String> {
    let project_path = resolve_cli_project(&args.project)?;
    let ac_new = project_path.join(".ac-new");
    let safe_team = sanitize_name(&args.team)?;
    let final_config = build_final_team_config(
        &ac_new,
        &safe_team,
        &args.coordinator,
        &args.agents,
        &args.repos,
        &args.repo_agents,
        &args.repo_exclude_agents,
    )?;
    let settings = crate::config::settings::load_settings_for_cli();
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| format!("Failed to create async runtime: {}", e))?;
    let result = runtime.block_on(create_workgroup_on_disk(WorkgroupDiskCreateArgs {
        project_path: project_path.clone(),
        team_name: safe_team,
        task_title: args.title,
        coordinator: Some(final_config.coordinator.clone()),
        agents: final_config.agents.clone(),
        repos: final_config.repos.clone(),
        settings_flags: AgentMatrixSettingsFlags::from_settings(&settings),
    }))?;
    let changed_path = PathBuf::from(&result.path);
    write_refresh(
        &project_path,
        &changed_path,
        changed_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workgroup"),
        "workgroupCreated",
    );
    print_json(&result)
}

fn remove(args: WorkgroupRemoveArgs) -> Result<(), String> {
    validate_existing_name(&args.workgroup, "Workgroup")?;
    let project_path = resolve_cli_project(&args.project)?;
    let ac_new = project_path.join(".ac-new");
    let wg_dir = ac_new.join(&args.workgroup);
    if !wg_dir.is_dir() {
        return Err(format!("Workgroup '{}' not found", args.workgroup));
    }
    crate::cli::session_safety::ensure_no_live_sessions_under(&wg_dir)?;
    if !args.force_dirty {
        let dirty = check_workgroup_repos_dirty(std::slice::from_ref(&wg_dir));
        if !dirty.is_empty() {
            let list = dirty
                .iter()
                .map(|(repo, reason)| format!("  - {} ({})", repo, reason))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(format!(
                "Cannot delete workgroup: the following repos have pending work:\n{}\n\nCommit or push changes before deleting, or pass --force-dirty.",
                list
            ));
        }
    }
    match crate::commands::entity_creation::try_atomic_delete_wg(&wg_dir) {
        WgDeleteOutcome::Deleted => {}
        WgDeleteOutcome::Blocked(e) => {
            return Err(format!("Failed to delete workgroup, file in use: {}", e));
        }
        WgDeleteOutcome::Other(e) => {
            return Err(format!("Failed to delete workgroup directory: {}", e));
        }
    }
    write_refresh(&project_path, &wg_dir, &args.workgroup, "workgroupRemoved");
    if std::env::var_os("AC_MACHINE_OUTPUT").is_some() {
        print_json(&serde_json::json!({
            "workgroup": args.workgroup,
            "path": wg_dir.to_string_lossy(),
            "removed": true
        }))
    } else {
        crate::cli_println!("Removed workgroup {}", args.workgroup);
        Ok(())
    }
}

pub(crate) fn build_final_team_config(
    ac_new: &Path,
    team_name: &str,
    coordinator: &str,
    agents: &[String],
    repos: &[String],
    repo_agents: &[String],
    repo_exclude_agents: &[String],
) -> Result<TeamConfigResult, String> {
    let existing = read_team_config(ac_new, team_name).ok();
    let mut roster = Vec::new();
    if let Some(config) = existing.as_ref() {
        for agent in &config.agents {
            push_unique(&mut roster, resolve_agent_ref(ac_new, agent)?);
        }
    }
    for agent in agents {
        push_unique(&mut roster, resolve_agent_ref(ac_new, agent)?);
    }
    let coordinator = resolve_agent_ref(ac_new, coordinator)?;
    push_unique(&mut roster, coordinator.clone());
    if roster.is_empty() {
        return Err("At least one team agent is required".to_string());
    }
    let repo_config =
        if repos.is_empty() && repo_agents.is_empty() && repo_exclude_agents.is_empty() {
            existing.map(|config| config.repos).unwrap_or_default()
        } else {
            build_repo_assignments(ac_new, &roster, repos, repo_agents, repo_exclude_agents)?
        };
    Ok(TeamConfigResult {
        agents: roster,
        coordinator,
        repos: repo_config,
    })
}

fn build_repo_assignments(
    ac_new: &Path,
    roster: &[String],
    repos: &[String],
    repo_agents: &[String],
    repo_exclude_agents: &[String],
) -> Result<Vec<RepoAssignment>, String> {
    let mut order = Vec::new();
    let mut default_urls = BTreeSet::new();
    for repo in repos {
        let url = repo.trim().to_string();
        if url.is_empty() {
            return Err("--repo cannot be empty".to_string());
        }
        if default_urls.insert(url.clone()) {
            order.push(url);
        }
    }

    let include = parse_assignment_specs(repo_agents, "--repo-agents")?;
    let exclude = parse_assignment_specs(repo_exclude_agents, "--repo-exclude-agents")?;
    for url in include.keys() {
        if exclude.contains_key(url) {
            return Err(format!(
                "Repo '{}' cannot use both --repo-agents and --repo-exclude-agents",
                url
            ));
        }
        if !order.contains(url) {
            order.push(url.clone());
        }
    }
    for url in exclude.keys() {
        if !order.contains(url) {
            order.push(url.clone());
        }
    }

    let mut out = Vec::new();
    for url in order {
        let agents = if let Some(list) = include.get(&url) {
            resolve_assignment_agents(ac_new, roster, list)?
        } else if let Some(list) = exclude.get(&url) {
            let excluded = resolve_assignment_agents(ac_new, roster, list)?;
            roster
                .iter()
                .filter(|agent| !excluded.contains(agent))
                .cloned()
                .collect()
        } else {
            roster.to_vec()
        };
        out.push(RepoAssignment { url, agents });
    }
    Ok(out)
}

fn parse_assignment_specs(
    specs: &[String],
    flag: &str,
) -> Result<HashMap<String, Vec<String>>, String> {
    let mut out = HashMap::new();
    for spec in specs {
        let Some((url, agents)) = spec.split_once('=') else {
            return Err(format!("{} expects URL=agent-a,agent-b", flag));
        };
        let url = url.trim();
        if url.is_empty() {
            return Err(format!("{} URL cannot be empty", flag));
        }
        if out.contains_key(url) {
            return Err(format!("{} repeated for repo '{}'", flag, url));
        }
        let agents = agents
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if agents.is_empty() {
            return Err(format!("{} must list at least one agent", flag));
        }
        out.insert(url.to_string(), agents);
    }
    Ok(out)
}

fn resolve_assignment_agents(
    ac_new: &Path,
    roster: &[String],
    agents: &[String],
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for agent in agents {
        let resolved = resolve_agent_ref(ac_new, agent)?;
        if !roster.contains(&resolved) {
            return Err(format!(
                "Repo assignment references agent '{}' which is not in the final team roster",
                agent
            ));
        }
        push_unique(&mut out, resolved);
    }
    Ok(out)
}

pub(crate) fn push_unique(items: &mut Vec<String>, value: String) {
    if !items.contains(&value) {
        items.push(value);
    }
}

fn list_replicas(wg_dir: &Path) -> Vec<String> {
    let mut replicas = Vec::new();
    if let Ok(entries) = std::fs::read_dir(wg_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(agent) = name.strip_prefix("__agent_") {
                replicas.push(agent.to_string());
            }
        }
    }
    replicas.sort();
    replicas
}

pub(crate) async fn clone_missing_for_config(
    wg_dir: &Path,
    repos: &[RepoAssignment],
) -> Vec<crate::commands::entity_creation::CloneError> {
    clone_missing_repos_for_workgroup(wg_dir, repos).await
}

fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize JSON output: {}", e))?;
    crate::cli_println!("{}", json);
    Ok(())
}
