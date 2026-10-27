use clap::{Args, Subcommand};
use serde::Serialize;
use std::ops::ControlFlow;

use crate::cli::workgroup::{
    build_new_team_config, clone_missing_for_config, push_unique, resolve_cli_ac_root,
    resolve_cli_project, write_refresh,
};
use crate::commands::entity_creation::{
    acquire_lifecycle_project_gate, agent_ref_bare_name, create_new_team_config_on_disk,
    create_or_update_replica_on_disk, normalize_team_config_for_project,
    parse_team_from_workgroup_name, prune_replica_config_scope, read_team_config,
    remove_replica_dir, resolve_agent_ref, sanitize_name, validate_existing_name,
    write_team_config_guarded, ReplicaDiskCreateArgs, ReplicaRemovalOutcome, RepoAssignment,
    TeamConfigMutationGuard, TeamConfigResult,
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
    /// Add an agent to one room and team config
    AddMember(TeamAddMemberArgs),
    /// Remove an agent from one room and team config
    RemoveMember(TeamRemoveMemberArgs),
}

#[derive(Args)]
struct TeamListArgs {
    #[arg(long)]
    project: String,
    #[arg(long = "room", alias = "workgroup", value_name = "ROOM")]
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
        help = "Existing agent matrix name or _agent_<name> reference. \
                Automatically included in the roster"
    )]
    coordinator: String,
    #[arg(
        long = "agent",
        help = "Existing agent matrix name or _agent_<name> reference. Repeat for multiple members"
    )]
    agents: Vec<String>,
    #[arg(
        long = "repo",
        help = "Define a repo available to the team when rooms are created. \
                Repeat for multiple repos"
    )]
    repos: Vec<String>,
    #[arg(
        long = "repo-agents",
        help = "Define team repo access for room creation as URL=agent-a,agent-b"
    )]
    repo_agents: Vec<String>,
    #[arg(
        long = "repo-exclude-agents",
        help = "Define team repo access for room creation as URL=excluded-agent-a,excluded-agent-b"
    )]
    repo_exclude_agents: Vec<String>,
}

#[derive(Args)]
struct TeamAddMemberArgs {
    #[arg(long)]
    project: String,
    #[arg(long = "room", alias = "workgroup", value_name = "ROOM")]
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
    #[arg(long = "room", alias = "workgroup", value_name = "ROOM")]
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
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let safe_team = sanitize_name(&args.team)?;
    let config = build_new_team_config(
        &ac_root,
        &args.coordinator,
        &args.agents,
        &args.repos,
        &args.repo_agents,
        &args.repo_exclude_agents,
    )?;
    let team_dir = create_new_team_config_on_disk(&ac_root, &safe_team, &config)?;
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
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let mut items = Vec::new();
    if let Some(workgroup) = args.workgroup {
        validate_existing_name(&workgroup, "Room")?;
        let team = parse_team_from_workgroup_name(&workgroup)?;
        let config = read_team_config(&ac_root, &team)?;
        items.push(TeamListItem {
            team,
            workgroup: Some(workgroup),
            agents: config.agents,
            coordinator: config.coordinator,
            repos: config.repos,
            context_alert_percentages: config.context_alert_percentages,
        });
    } else {
        let _ = crate::filesystem::directories::visit_cli_discovery_entries(&ac_root, |entry| {
            let Ok((path, entry_name)) = entry else {
                return ControlFlow::<()>::Continue(());
            };
            if !crate::filesystem::directories::cli_discovery_is_dir(&path) {
                return ControlFlow::Continue(());
            }
            let name = entry_name.to_string_lossy().to_string();
            let Some(team) = name.strip_prefix("_team_") else {
                return ControlFlow::Continue(());
            };
            if let Ok(config) = read_team_config(&ac_root, team) {
                items.push(TeamListItem {
                    team: team.to_string(),
                    workgroup: None,
                    agents: config.agents,
                    coordinator: config.coordinator,
                    repos: config.repos,
                    context_alert_percentages: config.context_alert_percentages,
                });
            }

            ControlFlow::Continue(())
        });
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
        return Err("Orchestrator must be one of the selected agents".to_string());
    }
    Ok((config, !was_present))
}

fn remove_member_from_team_config(
    mut config: TeamConfigResult,
    agent_ref: &str,
) -> Result<(TeamConfigResult, bool), String> {
    if config.coordinator == agent_ref {
        return Err(
            "Cannot remove the current orchestrator without choosing a replacement".to_string(),
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
    validate_existing_name(&args.workgroup, "Room")?;
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let guard = TeamConfigMutationGuard::acquire(&ac_root)?;
    let wg_dir = ac_root.join(&args.workgroup);
    if !crate::filesystem::directories::cli_discovery_is_dir(&wg_dir) {
        return Err(format!("Room '{}' not found", args.workgroup));
    }
    let team = parse_team_from_workgroup_name(&args.workgroup)?;
    let config = normalize_team_config_for_project(&ac_root, &read_team_config(&ac_root, &team)?)?;
    let agent_ref = resolve_agent_ref(&ac_root, &args.agent)?;
    let (config, added) = add_member_to_team_config(config, &agent_ref, args.coordinator)?;
    write_team_config_guarded(&ac_root, &team, &config, &guard)?;
    drop(guard);

    let replica_dir = create_or_update_replica_on_disk(ReplicaDiskCreateArgs {
        ac_root: ac_root.clone(),
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
    // #1065 Stage F: activated with the sole production token; no lock-order test barrier.
    let activation = crate::config::seed_manifest::ManifestActivationToken::production();
    remove_member_hooked(args, Some(&activation), |_| {})
}

/// CLI team-member removal with the #1063 global lock order.
///
/// Preflight and initial intent run outside both guards; the blocking commit
/// then acquires the project seed-manifest gate first and the #1056
/// `TeamConfigMutationGuard` second (plan sections 5.4/6.3), re-reads and
/// revalidates the current team config under both, computes the mutation from
/// that current value, and holds both across the membership commit, typed replica
/// removal, and manifest prune before dropping them ahead of refresh and output.
/// `remove_member` is synchronous, so no guard ever crosses `.await`.
///
/// `activation` is `Some(ManifestActivationToken::production())` in production
/// (#1065 Stage F); tests may pass `Some(for_test())` or `None`.
/// `after_project_before_team` is a `#[cfg(test)]` cross-process inversion barrier
/// that fires after the project gate is held and before the team guard; production
/// passes a no-op.
fn remove_member_hooked(
    args: TeamRemoveMemberArgs,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
    after_project_before_team: impl FnOnce(&std::path::Path),
) -> Result<(), String> {
    validate_existing_name(&args.workgroup, "Room")?;
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let wg_dir = ac_root.join(&args.workgroup);
    if !crate::filesystem::directories::cli_discovery_is_dir(&wg_dir) {
        return Err(format!("Room '{}' not found", args.workgroup));
    }
    let team = parse_team_from_workgroup_name(&args.workgroup)?;
    let agent_ref = resolve_agent_ref(&ac_root, &args.agent)?;
    let agent_name = agent_ref_bare_name(&agent_ref);
    let replica_dir = wg_dir.join(format!("__agent_{}", agent_name));
    crate::cli::session_safety::ensure_no_live_sessions_under(&replica_dir)?;

    // Blocking commit: project gate first, then the #1056 team-config guard.
    let mut project_gate = acquire_lifecycle_project_gate(&project_path)?;
    after_project_before_team(&ac_root);
    let guard = TeamConfigMutationGuard::acquire(&ac_root)?;

    // Re-read and revalidate the current team config under both guards, then
    // recompute the mutation from that current value.
    let config = normalize_team_config_for_project(&ac_root, &read_team_config(&ac_root, &team)?)?;
    let (config, removed) = remove_member_from_team_config(config, &agent_ref)?;
    write_team_config_guarded(&ac_root, &team, &config, &guard)?;

    // Typed replica removal; prune only after a proven removal or explicit absence.
    match remove_replica_dir(&replica_dir) {
        ReplicaRemovalOutcome::Removed | ReplicaRemovalOutcome::AlreadyAbsent => {
            prune_replica_config_scope(
                project_gate.as_mut(),
                activation,
                &args.workgroup,
                &agent_name,
            );
        }
        ReplicaRemovalOutcome::Failed(error) => {
            // The team-config mutation committed, but the replica path may still
            // exist; preserve rows and surface the failure.
            drop(guard);
            drop(project_gate);
            return Err(error);
        }
    }
    drop(guard);
    drop(project_gate);

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

/// #1063 Stage D-owned support for the co-located exact ignored cross-process
/// lock-order inversion CHILD HELPERS in `cli/team.rs` and `cli/workgroup.rs`.
///
/// A future Stage E parent spawns a child by its frozen fully-qualified name via
/// `current_exe --exact <fqn> --ignored`, passing the child-mode action, a per-spawn
/// nonce, and a control directory through the env vars below. Without all three, the
/// child no-ops with no guard and no mutation, so a bare `cargo test -- --ignored`
/// run is safe. This module and the helpers are `#[cfg(test)]`-only; production is
/// unchanged and non-emitting. Stage D owns only the child helpers; the parent
/// spawn/drain/watchdog machinery is Stage E.
#[cfg(test)]
pub(crate) mod stage_d_lock_order_child;

#[cfg(test)]
mod tests;
