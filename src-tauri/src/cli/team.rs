use clap::{Args, Subcommand};
use serde::Serialize;

use crate::cli::workgroup::{
    build_new_team_config, clone_missing_for_config, push_unique, resolve_cli_project,
    resolve_cli_workspace, write_refresh,
};
use crate::commands::entity_creation::{
    agent_ref_bare_name, create_new_team_config_on_disk, create_or_update_replica_on_disk,
    normalize_team_config_for_project, parse_team_from_workgroup_name, read_team_config,
    remove_replica_dir, resolve_agent_ref, sanitize_name, validate_existing_name,
    write_team_config_guarded, ReplicaDiskCreateArgs, RepoAssignment, TeamConfigMutationGuard,
    TeamConfigResult,
};

#[derive(Args)]
pub struct TeamArgs {
    #[command(subcommand)]
    command: TeamCommand,
}

#[derive(Subcommand)]
enum TeamCommand {
    /// Create a team configuration from existing agents
    Create(TeamCreateArgs),
    /// List team configuration
    List(TeamListArgs),
    /// Add an agent to one workgroup and team config
    AddMember(TeamAddMemberArgs),
    /// Remove an agent from one workgroup and team config
    RemoveMember(TeamRemoveMemberArgs),
}

#[derive(Args)]
struct TeamListArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    workgroup: Option<String>,
}

#[derive(Args)]
struct TeamCreateArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    team: String,
    #[arg(
        long,
        help = "Existing agent matrix name or _agent_<name> reference. Automatically included in the roster"
    )]
    coordinator: String,
    #[arg(
        long = "agent",
        help = "Existing agent matrix name or _agent_<name> reference. Repeat for multiple members"
    )]
    agents: Vec<String>,
    #[arg(
        long = "repo",
        help = "Define a repo available to the team when workgroups are created. Repeat for multiple repos"
    )]
    repos: Vec<String>,
    #[arg(
        long = "repo-agents",
        help = "Define team repo access for workgroup creation as URL=agent-a,agent-b"
    )]
    repo_agents: Vec<String>,
    #[arg(
        long = "repo-exclude-agents",
        help = "Define team repo access for workgroup creation as URL=excluded-agent-a,excluded-agent-b"
    )]
    repo_exclude_agents: Vec<String>,
}

#[derive(Args)]
struct TeamAddMemberArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    workgroup: String,
    #[arg(long)]
    agent: String,
    #[arg(long)]
    coordinator: bool,
}

#[derive(Args)]
struct TeamRemoveMemberArgs {
    #[arg(long)]
    project: String,
    #[arg(long)]
    workgroup: String,
    #[arg(long)]
    agent: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TeamListItem {
    team: String,
    workgroup: Option<String>,
    agents: Vec<String>,
    coordinator: String,
    repos: Vec<RepoAssignment>,
    context_alert_percentages: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TeamCreateResult {
    team: String,
    path: String,
    agents: Vec<String>,
    coordinator: String,
    repos: Vec<RepoAssignment>,
    context_alert_percentages: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AddMemberResult {
    team: String,
    workgroup: String,
    agent: String,
    replica_path: String,
    coordinator: String,
    added: bool,
    clone_errors: Vec<crate::commands::entity_creation::CloneError>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoveMemberResult {
    team: String,
    workgroup: String,
    agent: String,
    replica_path: String,
    removed: bool,
}

pub fn execute(args: TeamArgs) -> i32 {
    let result = match args.command {
        TeamCommand::Create(args) => create(args),
        TeamCommand::List(args) => list(args),
        TeamCommand::AddMember(args) => add_member(args),
        TeamCommand::RemoveMember(args) => remove_member(args),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

fn create(args: TeamCreateArgs) -> Result<(), String> {
    let project_path = resolve_cli_project(&args.project)?;
    let workspace_dir = resolve_cli_workspace(&project_path)?;
    let safe_team = sanitize_name(&args.team)?;
    let config = build_new_team_config(
        &workspace_dir,
        &args.coordinator,
        &args.agents,
        &args.repos,
        &args.repo_agents,
        &args.repo_exclude_agents,
    )?;
    let team_dir = create_new_team_config_on_disk(&workspace_dir, &safe_team, &config)?;
    write_refresh(&project_path, &team_dir, &safe_team, "teamCreated");
    print_json(&TeamCreateResult {
        team: safe_team,
        path: team_dir.to_string_lossy().to_string(),
        agents: config.agents,
        coordinator: config.coordinator,
        repos: config.repos,
        context_alert_percentages: config.context_alert_percentages,
    })
}

fn list(args: TeamListArgs) -> Result<(), String> {
    let project_path = resolve_cli_project(&args.project)?;
    let workspace_dir = resolve_cli_workspace(&project_path)?;
    let mut items = Vec::new();
    if let Some(workgroup) = args.workgroup {
        validate_existing_name(&workgroup, "Workgroup")?;
        let team = parse_team_from_workgroup_name(&workgroup)?;
        let config = read_team_config(&workspace_dir, &team)?;
        items.push(TeamListItem {
            team,
            workgroup: Some(workgroup),
            agents: config.agents,
            coordinator: config.coordinator,
            repos: config.repos,
            context_alert_percentages: config.context_alert_percentages,
        });
    } else if let Ok(entries) = std::fs::read_dir(&workspace_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(team) = name.strip_prefix("_team_") else {
                continue;
            };
            if let Ok(config) = read_team_config(&workspace_dir, team) {
                items.push(TeamListItem {
                    team: team.to_string(),
                    workgroup: None,
                    agents: config.agents,
                    coordinator: config.coordinator,
                    repos: config.repos,
                    context_alert_percentages: config.context_alert_percentages,
                });
            }
        }
        items.sort_by(|a, b| a.team.cmp(&b.team));
    }
    print_json(&items)
}

fn add_member_to_team_config(
    mut config: TeamConfigResult,
    agent_ref: &str,
    make_coordinator: bool,
) -> Result<(TeamConfigResult, bool), String> {
    let was_present = config.agents.iter().any(|agent| agent == agent_ref);
    push_unique(&mut config.agents, agent_ref.to_string());
    if make_coordinator {
        config.coordinator = agent_ref.to_string();
    }
    if !config.coordinator.is_empty() && !config.agents.contains(&config.coordinator) {
        return Err("Coordinator must be one of the selected agents".to_string());
    }
    Ok((config, !was_present))
}

fn remove_member_from_team_config(
    mut config: TeamConfigResult,
    agent_ref: &str,
) -> Result<(TeamConfigResult, bool), String> {
    if config.coordinator == agent_ref {
        return Err(
            "Cannot remove the current coordinator without choosing a replacement".to_string(),
        );
    }
    let before = config.agents.len();
    config.agents.retain(|agent| agent != agent_ref);
    for repo in &mut config.repos {
        repo.agents.retain(|agent| agent != agent_ref);
    }
    let removed = config.agents.len() != before;
    Ok((config, removed))
}

fn add_member(args: TeamAddMemberArgs) -> Result<(), String> {
    validate_existing_name(&args.workgroup, "Workgroup")?;
    let project_path = resolve_cli_project(&args.project)?;
    let workspace_dir = resolve_cli_workspace(&project_path)?;
    let guard = TeamConfigMutationGuard::acquire(&workspace_dir)?;
    let wg_dir = workspace_dir.join(&args.workgroup);
    if !wg_dir.is_dir() {
        return Err(format!("Workgroup '{}' not found", args.workgroup));
    }
    let team = parse_team_from_workgroup_name(&args.workgroup)?;
    let config = normalize_team_config_for_project(
        &workspace_dir,
        &read_team_config(&workspace_dir, &team)?,
    )?;
    let agent_ref = resolve_agent_ref(&workspace_dir, &args.agent)?;
    let (config, added) = add_member_to_team_config(config, &agent_ref, args.coordinator)?;
    write_team_config_guarded(&workspace_dir, &team, &config, &guard)?;
    drop(guard);

    let replica_dir = create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
        workspace_dir: workspace_dir.clone(),
        wg_dir: wg_dir.clone(),
        agent_path: agent_ref.clone(),
        team_repos: config.repos.clone(),
    })?;
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| format!("Failed to create async runtime: {}", e))?;
    let clone_errors = runtime.block_on(clone_missing_for_config(&wg_dir, &config.repos));
    write_refresh(
        &project_path,
        &replica_dir,
        &format!("{}/{}", args.workgroup, agent_ref_bare_name(&agent_ref)),
        "teamMembershipChanged",
    );
    print_json(&AddMemberResult {
        team,
        workgroup: args.workgroup,
        agent: agent_ref_bare_name(&agent_ref),
        replica_path: replica_dir.to_string_lossy().to_string(),
        coordinator: agent_ref_bare_name(&config.coordinator),
        added,
        clone_errors,
    })
}

fn remove_member(args: TeamRemoveMemberArgs) -> Result<(), String> {
    validate_existing_name(&args.workgroup, "Workgroup")?;
    let project_path = resolve_cli_project(&args.project)?;
    let workspace_dir = resolve_cli_workspace(&project_path)?;
    let guard = TeamConfigMutationGuard::acquire(&workspace_dir)?;
    let wg_dir = workspace_dir.join(&args.workgroup);
    if !wg_dir.is_dir() {
        return Err(format!("Workgroup '{}' not found", args.workgroup));
    }
    let team = parse_team_from_workgroup_name(&args.workgroup)?;
    let config = normalize_team_config_for_project(
        &workspace_dir,
        &read_team_config(&workspace_dir, &team)?,
    )?;
    let agent_ref = resolve_agent_ref(&workspace_dir, &args.agent)?;
    let agent_name = agent_ref_bare_name(&agent_ref);
    let replica_dir = wg_dir.join(format!("__agent_{}", agent_name));
    crate::cli::session_safety::ensure_no_live_sessions_under(&replica_dir)?;
    let (config, removed) = remove_member_from_team_config(config, &agent_ref)?;
    write_team_config_guarded(&workspace_dir, &team, &config, &guard)?;
    drop(guard);

    remove_replica_dir(&replica_dir)?;
    write_refresh(
        &project_path,
        &replica_dir,
        &format!("{}/{}", args.workgroup, agent_name),
        "teamMembershipRemoved",
    );
    print_json(&RemoveMemberResult {
        team,
        workgroup: args.workgroup,
        agent: agent_name,
        replica_path: replica_dir.to_string_lossy().to_string(),
        removed,
    })
}

fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize JSON output: {}", e))?;
    crate::cli_println!("{}", json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_alerts() -> TeamConfigResult {
        TeamConfigResult {
            agents: vec!["_agent_alpha".to_string()],
            coordinator: "_agent_alpha".to_string(),
            repos: vec![RepoAssignment {
                url: "https://example.test/repo.git".to_string(),
                agents: vec!["_agent_alpha".to_string()],
            }],
            context_alert_percentages: vec![50, 75, 90],
        }
    }

    #[test]
    fn roster_helpers_preserve_complete_alert_vector() {
        let original_alerts = config_with_alerts().context_alert_percentages;
        let (added, did_add) =
            add_member_to_team_config(config_with_alerts(), "_agent_beta", false)
                .expect("add member");
        assert!(did_add);
        assert_eq!(added.context_alert_percentages, original_alerts);
        assert_eq!(added.repos[0].agents, vec!["_agent_alpha"]);

        let (removed, did_remove) =
            remove_member_from_team_config(added, "_agent_beta").expect("remove member");
        assert!(did_remove);
        assert_eq!(removed.context_alert_percentages, vec![50, 75, 90]);
        assert_eq!(removed.agents, vec!["_agent_alpha"]);
    }

    #[test]
    fn cli_team_list_and_create_results_include_camel_case_alerts() {
        let list = TeamListItem {
            team: "dev-team".to_string(),
            workgroup: None,
            agents: Vec::new(),
            coordinator: String::new(),
            repos: Vec::new(),
            context_alert_percentages: vec![50, 75],
        };
        let create = TeamCreateResult {
            team: "dev-team".to_string(),
            path: "_team_dev-team".to_string(),
            agents: Vec::new(),
            coordinator: String::new(),
            repos: Vec::new(),
            context_alert_percentages: Vec::new(),
        };

        let list_json = serde_json::to_value(list).expect("list JSON");
        let create_json = serde_json::to_value(create).expect("create JSON");
        assert_eq!(
            list_json["contextAlertPercentages"],
            serde_json::json!([50, 75])
        );
        assert_eq!(
            create_json["contextAlertPercentages"],
            serde_json::json!([])
        );
    }
}
