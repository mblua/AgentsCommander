use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::cli::create_agent_matrix::{write_project_refresh_request, ProjectRefreshRequest};
use crate::commands::entity_creation::{
    acquire_lifecycle_project_gate, check_workgroup_repos_dirty, clone_missing_repos_for_workgroup,
    create_workgroup_on_disk, list_workgroup_dirs, prune_workgroup_config_scope, read_team_config,
    resolve_agent_ref, sanitize_name, validate_delete_root_not_link_or_reparse,
    validate_existing_name, RepoAssignment, TeamConfigResult, WgDeleteOutcome,
    WorkgroupDiskCreateArgs,
};
use crate::config::ac_root::existing_ac_root;
use crate::config::projects::resolve_project_reference;

#[derive(Args)]
pub struct WorkgroupArgs {
    #[command(subcommand)]
    command: WorkgroupCommand,
}

#[derive(Subcommand)]
enum WorkgroupCommand {
    /// List rooms in a project
    List(WorkgroupListArgs),
    /// Create an auto-numbered room
    Add(WorkgroupAddArgs),
    /// Remove a room
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
    #[arg(long, hide = true)]
    coordinator: Option<String>,
    #[arg(long = "agent", hide = true)]
    agents: Vec<String>,
    #[arg(long = "repo", hide = true)]
    repos: Vec<String>,
    #[arg(long = "repo-agents", hide = true)]
    repo_agents: Vec<String>,
    #[arg(long = "repo-exclude-agents", hide = true)]
    repo_exclude_agents: Vec<String>,
}

#[derive(Args)]
struct WorkgroupRemoveArgs {
    #[arg(long)]
    project: String,
    #[arg(long = "room", alias = "workgroup", value_name = "ROOM")]
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

pub(crate) fn resolve_cli_ac_root(project_path: &Path) -> Result<PathBuf, String> {
    existing_ac_root(project_path).ok_or_else(|| {
        format!(
            "Project AC Root not found in {} (.ac)",
            project_path.display()
        )
    })
}

pub(crate) fn write_refresh(project_path: &Path, changed_path: &Path, name: &str, reason: &str) {
    let canonical_project_path =
        std::fs::canonicalize(project_path).unwrap_or_else(|_| project_path.to_path_buf());
    let canonical_changed_path =
        std::fs::canonicalize(changed_path).unwrap_or_else(|_| changed_path.to_path_buf());
    let request = ProjectRefreshRequest {
        id: uuid::Uuid::new_v4().to_string(),
        project_path: canonical_project_path.to_string_lossy().to_string(),
        changed_path: Some(canonical_changed_path.to_string_lossy().to_string()),
        changed_name: Some(name.to_string()),
        reason: reason.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = write_project_refresh_request(&request) {
        eprintln!("Warning: failed to request project refresh: {}", e);
    }
}

pub(crate) fn write_project_registration_refresh(project_path: &Path, reason: &str) {
    let canonical_project_path =
        std::fs::canonicalize(project_path).unwrap_or_else(|_| project_path.to_path_buf());
    let request = ProjectRefreshRequest {
        id: uuid::Uuid::new_v4().to_string(),
        project_path: canonical_project_path.to_string_lossy().to_string(),
        changed_path: None,
        changed_name: None,
        reason: reason.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = write_project_refresh_request(&request) {
        eprintln!("Warning: failed to request project refresh: {}", e);
    }
}

fn list(args: WorkgroupListArgs) -> Result<(), String> {
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let items: Vec<WorkgroupListItem> = list_workgroup_dirs(&ac_root)
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
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let safe_team = sanitize_name(&args.team)?;
    let has_legacy_team_flags = args.coordinator.is_some()
        || !args.agents.is_empty()
        || !args.repos.is_empty()
        || !args.repo_agents.is_empty()
        || !args.repo_exclude_agents.is_empty();
    let team_dir = ac_root.join(format!("_team_{}", safe_team));
    let config_path = team_dir.join("config.json");
    let team_config_exists = team_dir.exists() || config_path.exists();
    let provisioning_config = if team_config_exists {
        read_team_config(&ac_root, &safe_team)?;
        if has_legacy_team_flags {
            return Err(format!(
                "Team '{}' already exists. `room add` no longer updates team configuration. Use `team create` before `room add`, or `team add-member` for membership changes.",
                safe_team
            ));
        }
        None
    } else if has_legacy_team_flags {
        let coordinator = args.coordinator.as_deref().ok_or_else(|| {
            "--coordinator is required when supplying team details on room add".to_string()
        })?;
        eprintln!(
            "Warning: created missing team configuration from supplied room details. Prefer creating the team before activating a room."
        );
        Some(build_new_team_config(
            &ac_root,
            coordinator,
            &args.agents,
            &args.repos,
            &args.repo_agents,
            &args.repo_exclude_agents,
        )?)
    } else {
        return Err(format!(
            "Team '{}' config not found. Create it first with `team create`.",
            safe_team
        ));
    };
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| format!("Failed to create async runtime: {}", e))?;
    let (coordinator, agents, repos) = if let Some(config) = provisioning_config {
        (
            Some(config.coordinator.clone()),
            config.agents.clone(),
            config.repos.clone(),
        )
    } else {
        (None, Vec::new(), Vec::new())
    };
    let result = runtime.block_on(create_workgroup_on_disk(WorkgroupDiskCreateArgs {
        project_path: project_path.clone(),
        team_name: safe_team,
        task_title: args.title,
        coordinator,
        agents,
        repos,
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
    // #1065 Stage F: activated with the sole production token; no lock-order test barrier.
    let activation = crate::config::seed_manifest::ManifestActivationToken::production();
    remove_hooked(args, Some(&activation), |_| {})
}

/// CLI workgroup removal with the #1063 project-only lock order.
///
/// Workgroup deletion is a project-only mutation: it acquires the project
/// seed-manifest gate before the atomic rename and NEVER acquires the
/// `TeamConfigMutationGuard` (plan sections 5.4/6.3). It prunes `Deleted`/`Partial`
/// logical paths while holding the gate, then releases the gate before converting
/// the structured outcome into the CLI success/error contract or formatting an
/// error. `remove` is synchronous, so no guard ever crosses `.await`.
///
/// `activation` is `Some(ManifestActivationToken::production())` in production
/// (#1065 Stage F); tests may pass `Some(for_test())` or `None`.
/// `after_project_acquired` is a `#[cfg(test)]` inversion barrier that fires after
/// the project gate is held; production passes a no-op.
fn remove_hooked(
    args: WorkgroupRemoveArgs,
    activation: Option<&crate::config::seed_manifest::ManifestActivationToken>,
    after_project_acquired: impl FnOnce(&Path),
) -> Result<(), String> {
    validate_existing_name(&args.workgroup, "Room")?;
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let wg_dir = ac_root.join(&args.workgroup);
    validate_delete_root_not_link_or_reparse(&wg_dir)?;
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
                "Cannot delete room: the following repos have pending work:\n{}\n\nCommit or push changes before deleting, or pass --force-dirty.",
                list
            ));
        }
    }

    // Project-only gate: acquire before the atomic rename, prune Deleted/Partial
    // while held, then release before formatting the outcome. Never take the team guard.
    let mut project_gate = acquire_lifecycle_project_gate(&project_path)?;
    after_project_acquired(&ac_root);
    let outcome = crate::commands::entity_creation::try_atomic_delete_wg(&wg_dir);
    if outcome.logical_path_removed() {
        prune_workgroup_config_scope(project_gate.as_mut(), activation, &args.workgroup);
    }
    drop(project_gate);

    match cli_remove_refresh_decision(outcome)? {
        RemoveRefreshDecision::EmitWorkgroupRemoved => {}
    }
    // (#621) Drop the workgroup's coordinator_clocks keys. CLI is its own process,
    // so load+save the on-disk map directly (the startup prune backstops the
    // GUI-running-concurrently race).
    if let Some(project_name) = project_path.file_name().and_then(|n| n.to_str()) {
        match crate::config::coordinator_clocks::remove_workgroup_on_disk(
            project_name,
            &args.workgroup,
        ) {
            Ok(n) if n > 0 => {
                log::info!(
                    "[workgroup-remove] dropped {} clock key(s) for {}",
                    n,
                    args.workgroup
                )
            }
            Ok(_) => {}
            Err(e) => log::warn!("[workgroup-remove] clock cleanup failed: {}", e),
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
        crate::cli_println!("Removed room {}", args.workgroup);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoveRefreshDecision {
    EmitWorkgroupRemoved,
}

pub(crate) fn cli_remove_refresh_decision(
    outcome: WgDeleteOutcome,
) -> Result<RemoveRefreshDecision, String> {
    match outcome {
        WgDeleteOutcome::Deleted => {}
        WgDeleteOutcome::Blocked(e) => {
            return Err(format!("Failed to delete room, file in use: {}", e));
        }
        WgDeleteOutcome::Partial { orphan_path, error } => {
            return Err(format!(
                "Failed to fully delete room directory; renamed room to orphan '{}', but failed to remove orphan: {}",
                orphan_path.display(),
                error
            ));
        }
        WgDeleteOutcome::Other(e) => {
            return Err(format!("Failed to delete room directory: {}", e));
        }
    }
    Ok(RemoveRefreshDecision::EmitWorkgroupRemoved)
}

pub(crate) fn build_new_team_config(
    ac_root: &Path,
    coordinator: &str,
    agents: &[String],
    repos: &[String],
    repo_agents: &[String],
    repo_exclude_agents: &[String],
) -> Result<TeamConfigResult, String> {
    let coordinator = resolve_agent_ref(ac_root, coordinator)?;
    let mut roster = vec![coordinator.clone()];
    for agent in agents {
        push_unique(&mut roster, resolve_agent_ref(ac_root, agent)?);
    }
    let repo_config =
        build_repo_assignments(ac_root, &roster, repos, repo_agents, repo_exclude_agents)?;
    Ok(TeamConfigResult {
        agents: roster,
        coordinator,
        repos: repo_config,
        context_alert_percentages: Vec::new(),
    })
}

fn build_repo_assignments(
    ac_root: &Path,
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
            resolve_assignment_agents(ac_root, roster, list)?
        } else if let Some(list) = exclude.get(&url) {
            let excluded = resolve_assignment_agents(ac_root, roster, list)?;
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
    ac_root: &Path,
    roster: &[String],
    agents: &[String],
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for agent in agents {
        let resolved = resolve_agent_ref(ac_root, agent)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_delete_outcome_does_not_authorize_removed_refresh() {
        let outcome = WgDeleteOutcome::Partial {
            orphan_path: PathBuf::from(".deleting-wg-1-test-orphan"),
            error: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "forced remove failure",
            ),
        };

        let err = cli_remove_refresh_decision(outcome)
            .expect_err("partial delete must not authorize workgroupRemoved refresh");
        assert!(err.contains("Failed to fully delete room directory"));
        assert!(err.contains(".deleting-wg-1-test-orphan"));
    }

    #[test]
    fn clean_delete_outcome_authorizes_removed_refresh() {
        assert_eq!(
            cli_remove_refresh_decision(WgDeleteOutcome::Deleted)
                .expect("deleted outcome should refresh"),
            RemoveRefreshDecision::EmitWorkgroupRemoved
        );
    }

    #[test]
    fn cli_team_builder_defaults_context_alerts_to_disabled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ac_root = tmp.path().join(".ac");
        std::fs::create_dir_all(ac_root.join("_agent_coordinator")).expect("coordinator matrix");
        std::fs::create_dir_all(ac_root.join("_agent_member")).expect("member matrix");

        let config = build_new_team_config(
            &ac_root,
            "coordinator",
            &["member".to_string()],
            &[],
            &[],
            &[],
        )
        .expect("build config");
        assert!(config.context_alert_percentages.is_empty());
    }

    #[tokio::test]
    async fn legacy_workgroup_provisioning_never_upserts_concurrent_team_winner() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("Project");
        let ac_root = project.join(".ac");
        std::fs::create_dir_all(ac_root.join("_agent_coordinator")).expect("coordinator matrix");
        let winner = TeamConfigResult {
            agents: vec!["_agent_coordinator".to_string()],
            coordinator: "_agent_coordinator".to_string(),
            repos: Vec::new(),
            context_alert_percentages: vec![75],
        };
        crate::commands::entity_creation::create_new_team_config_on_disk(
            &ac_root, "dev-team", &winner,
        )
        .expect("concurrent winner");

        let err = create_workgroup_on_disk(WorkgroupDiskCreateArgs {
            project_path: project,
            team_name: "dev-team".to_string(),
            task_title: "Race test".to_string(),
            coordinator: Some("_agent_coordinator".to_string()),
            agents: vec!["_agent_coordinator".to_string()],
            repos: Vec::new(),
        })
        .await
        .expect_err("legacy loser must not upsert");

        assert!(err.contains("Team 'dev-team' already exists"), "{err}");
        assert_eq!(
            read_team_config(&ac_root, "dev-team")
                .expect("winner config")
                .context_alert_percentages,
            vec![75]
        );
        assert!(list_workgroup_dirs(&ac_root).is_empty());
    }

    // #1063 Stage D-owned exact ignored CHILD HELPER for the CLI workgroup
    // cross-process lock-order inversion. Frozen fully-qualified name (a Stage E
    // parent spawns this verbatim via `current_exe --exact <name> --ignored`):
    //   crate::cli::workgroup::tests::cli_workgroup_lock_order_inversion_child
    // No-ops (no guard, no mutation) unless the child-mode action + per-spawn nonce
    // + control dir are all supplied; when driven, it calls the real private
    // `remove_hooked` (project-only) and drives the `after_project_acquired` barrier.
    #[test]
    #[ignore]
    fn cli_workgroup_lock_order_inversion_child() {
        use crate::cli::team::stage_d_lock_order_child as child;
        let Some(ctx) = child::child_context(child::WORKGROUP_ACTION) else {
            return;
        };
        let project = ctx.build_workgroup_fixture();
        let args = WorkgroupRemoveArgs {
            project,
            workgroup: "wg-1-dev-team".to_string(),
            force_dirty: false,
        };
        let result = remove_hooked(args, None, |_ac_root: &Path| ctx.report_and_wait());
        if let Err(error) = &result {
            println!("STAGE_D_LOCK_ORDER_ERROR {} workgroup {}", ctx.nonce, error);
        }
        println!(
            "STAGE_D_LOCK_ORDER_DONE {} workgroup ok={}",
            ctx.nonce,
            result.is_ok()
        );
    }

    // #1063: prove the driven project-only lock-order path in-process (no
    // `current_exe` parent - that machinery is Stage E). Env-guarded and `#[ignore]`
    // so the parallel `--lib` regression is untouched. Like the member driver it
    // no-ops unless `DRIVE_VAR` is set, so a bare `cargo test --lib -- --ignored` run
    // never drives it. Enable and isolate it deliberately (it pins the once-cached
    // config dir, so it must own the process):
    // `AC_STAGE_D_LOCK_ORDER_DRIVE=1 cargo test --lib -- --ignored --test-threads=1
    // --exact cli::workgroup::tests::cli_workgroup_lock_order_inversion_driver`.
    #[test]
    #[ignore]
    fn cli_workgroup_lock_order_inversion_driver() {
        use crate::cli::team::stage_d_lock_order_child::{
            driver_enabled, EnvGuard, ACTION_VAR, CONTROL_DIR_VAR, NONCE_VAR, WORKGROUP_ACTION,
        };
        if !driver_enabled() {
            return;
        }
        let control = tempfile::tempdir().expect("tempdir");
        let nonce = "driver-workgroup";
        let _guard = EnvGuard::capture(&[
            ACTION_VAR,
            NONCE_VAR,
            CONTROL_DIR_VAR,
            "AGENTSCOMMANDER_TEST_CONFIG_DIR",
        ]);
        std::env::set_var(ACTION_VAR, WORKGROUP_ACTION);
        std::env::set_var(NONCE_VAR, nonce);
        std::env::set_var(CONTROL_DIR_VAR, control.path());

        let reached = control.path().join(format!("reached-{nonce}"));
        let release = control.path().join(format!("release-{nonce}"));
        let releaser = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while std::time::Instant::now() < deadline && !reached.exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(
                reached.exists(),
                "child never reported reaching the barrier"
            );
            std::fs::write(&release, b"go").expect("write release");
        });

        cli_workgroup_lock_order_inversion_child();
        releaser.join().expect("join releaser");

        assert!(
            control.path().join(format!("reached-{nonce}")).exists(),
            "the barrier must have fired after the project gate was acquired"
        );
        let wg_dir = control
            .path()
            .join(format!("fixture-{nonce}"))
            .join("Project")
            .join(".ac")
            .join("wg-1-dev-team");
        assert!(
            !wg_dir.exists(),
            "the workgroup must be removed after the barrier releases"
        );
    }
}
