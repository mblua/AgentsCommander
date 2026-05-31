use clap::{Args, Subcommand};
use serde::Serialize;

use crate::cli::workgroup::{
    clone_missing_for_config, push_unique, resolve_cli_project, resolve_cli_workspace,
    write_refresh,
};
use crate::commands::entity_creation::{
    agent_ref_bare_name, create_or_update_replica_on_disk, normalize_team_config_for_project,
    parse_team_from_workgroup_name, read_team_config, remove_replica_dir, resolve_agent_ref,
    validate_existing_name, write_team_config, AgentMatrixSettingsFlags, ReplicaDiskCreateArgs,
    RepoAssignment,
};

#[derive(Args)]
pub struct TeamArgs {
    #[command(subcommand)]
    command: TeamCommand,
}

#[derive(Subcommand)]
enum TeamCommand {
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
                });
            }
        }
        items.sort_by(|a, b| a.team.cmp(&b.team));
    }
    print_json(&items)
}

fn add_member(args: TeamAddMemberArgs) -> Result<(), String> {
    validate_existing_name(&args.workgroup, "Workgroup")?;
    let project_path = resolve_cli_project(&args.project)?;
    let workspace_dir = resolve_cli_workspace(&project_path)?;
    let wg_dir = workspace_dir.join(&args.workgroup);
    if !wg_dir.is_dir() {
        return Err(format!("Workgroup '{}' not found", args.workgroup));
    }
    let team = parse_team_from_workgroup_name(&args.workgroup)?;
    let mut config = normalize_team_config_for_project(
        &workspace_dir,
        &read_team_config(&workspace_dir, &team)?,
    )?;
    let agent_ref = resolve_agent_ref(&workspace_dir, &args.agent)?;
    let was_present = config.agents.contains(&agent_ref);
    push_unique(&mut config.agents, agent_ref.clone());
    if args.coordinator {
        config.coordinator = agent_ref.clone();
    }
    if !config.coordinator.is_empty() && !config.agents.contains(&config.coordinator) {
        return Err("Coordinator must be one of the selected agents".to_string());
    }
    write_team_config(&workspace_dir, &team, &config)?;

    let settings = crate::config::settings::load_settings_for_cli();
    let replica_dir = create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
        wg_dir: wg_dir.clone(),
        agent_path: agent_ref.clone(),
        team_repos: config.repos.clone(),
        settings_flags: AgentMatrixSettingsFlags::from_settings(&settings),
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
        added: !was_present,
        clone_errors,
    })
}

fn remove_member(args: TeamRemoveMemberArgs) -> Result<(), String> {
    validate_existing_name(&args.workgroup, "Workgroup")?;
    let project_path = resolve_cli_project(&args.project)?;
    let workspace_dir = resolve_cli_workspace(&project_path)?;
    let wg_dir = workspace_dir.join(&args.workgroup);
    if !wg_dir.is_dir() {
        return Err(format!("Workgroup '{}' not found", args.workgroup));
    }
    let team = parse_team_from_workgroup_name(&args.workgroup)?;
    let mut config = normalize_team_config_for_project(
        &workspace_dir,
        &read_team_config(&workspace_dir, &team)?,
    )?;
    let agent_ref = resolve_agent_ref(&workspace_dir, &args.agent)?;
    if config.coordinator == agent_ref {
        return Err(
            "Cannot remove the current coordinator without choosing a replacement".to_string(),
        );
    }
    let agent_name = agent_ref_bare_name(&agent_ref);
    let replica_dir = wg_dir.join(format!("__agent_{}", agent_name));
    crate::cli::session_safety::ensure_no_live_sessions_under(&replica_dir)?;
    let before = config.agents.len();
    config.agents.retain(|agent| agent != &agent_ref);
    for repo in &mut config.repos {
        repo.agents.retain(|agent| agent != &agent_ref);
    }
    write_team_config(&workspace_dir, &team, &config)?;
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
        removed: config.agents.len() != before,
    })
}

fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed to serialize JSON output: {}", e))?;
    crate::cli_println!("{}", json);
    Ok(())
}
