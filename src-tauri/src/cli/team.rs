use clap::{Args, Subcommand};
use serde::Serialize;

use crate::cli::workgroup::{
    build_new_team_config, clone_missing_for_config, push_unique, resolve_cli_project,
    resolve_cli_ac_root, write_refresh,
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
        validate_existing_name(&workgroup, "Workgroup")?;
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
    } else if let Ok(entries) = std::fs::read_dir(&ac_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let Some(team) = name.strip_prefix("_team_") else {
                continue;
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
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let guard = TeamConfigMutationGuard::acquire(&ac_root)?;
    let wg_dir = ac_root.join(&args.workgroup);
    if !wg_dir.is_dir() {
        return Err(format!("Workgroup '{}' not found", args.workgroup));
    }
    let team = parse_team_from_workgroup_name(&args.workgroup)?;
    let config = normalize_team_config_for_project(
        &ac_root,
        &read_team_config(&ac_root, &team)?,
    )?;
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
    validate_existing_name(&args.workgroup, "Workgroup")?;
    let project_path = resolve_cli_project(&args.project)?;
    let ac_root = resolve_cli_ac_root(&project_path)?;
    let wg_dir = ac_root.join(&args.workgroup);
    if !wg_dir.is_dir() {
        return Err(format!("Workgroup '{}' not found", args.workgroup));
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
    let config = normalize_team_config_for_project(
        &ac_root,
        &read_team_config(&ac_root, &team)?,
    )?;
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
pub(crate) mod stage_d_lock_order_child {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// Frozen env-var protocol a Stage E parent sets before spawning a child.
    pub(crate) const ACTION_VAR: &str = "AC_STAGE_D_LOCK_ORDER_ACTION";
    pub(crate) const NONCE_VAR: &str = "AC_STAGE_D_LOCK_ORDER_NONCE";
    pub(crate) const CONTROL_DIR_VAR: &str = "AC_STAGE_D_LOCK_ORDER_CONTROL_DIR";

    /// Frozen child-mode actions (one per helper).
    pub(crate) const MEMBER_ACTION: &str = "cli-member-lock-order";
    pub(crate) const WORKGROUP_ACTION: &str = "cli-workgroup-lock-order";

    /// Enable flag for the in-process driven-path PROOF tests
    /// (`*_lock_order_inversion_driver`). This is NOT part of the Stage E child-mode
    /// protocol above: a real Stage E parent spawns the frozen-named CHILD helpers,
    /// never the drivers. It exists so a broad `cargo test --lib -- --ignored` run
    /// does NOT execute the in-process drivers, which set process-global env and pin
    /// the once-cached config directory (see `config::instance_location`); doing that
    /// in a shared, multi-threaded ignored run would corrupt other tests or risk
    /// touching the real config dir. The drivers no-op unless this flag is present,
    /// so they run only when enabled deliberately and in isolation under
    /// `--test-threads=1`.
    pub(crate) const DRIVE_VAR: &str = "AC_STAGE_D_LOCK_ORDER_DRIVE";

    /// True only when the in-process driven-path proof is explicitly enabled via
    /// `DRIVE_VAR`; a bare `--ignored` run leaves it unset so the drivers no-op.
    pub(crate) fn driver_enabled() -> bool {
        std::env::var_os(DRIVE_VAR).is_some()
    }

    /// A validated child-mode context: present only when the frozen action, a
    /// non-empty nonce, and an existing control directory are all supplied.
    pub(crate) struct ChildContext {
        pub(crate) nonce: String,
        pub(crate) control_dir: PathBuf,
    }

    /// Pure tuple validation, unit-testable without touching env (only a control-dir
    /// existence probe). Returns `None` (no-op) unless all three inputs are present
    /// and valid and the action matches `expected_action`.
    pub(crate) fn context_from(
        expected_action: &str,
        action: Option<&str>,
        nonce: Option<&str>,
        control_dir: Option<&str>,
    ) -> Option<ChildContext> {
        if action? != expected_action {
            return None;
        }
        let nonce = nonce?.trim();
        if nonce.is_empty() {
            return None;
        }
        let control_dir = PathBuf::from(control_dir?);
        if !control_dir.is_dir() {
            return None;
        }
        Some(ChildContext {
            nonce: nonce.to_string(),
            control_dir,
        })
    }

    /// Read the child-mode tuple from the environment; `None` (no-op) when absent.
    pub(crate) fn child_context(expected_action: &str) -> Option<ChildContext> {
        let action = std::env::var(ACTION_VAR).ok();
        let nonce = std::env::var(NONCE_VAR).ok();
        let control_dir = std::env::var(CONTROL_DIR_VAR).ok();
        context_from(
            expected_action,
            action.as_deref(),
            nonce.as_deref(),
            control_dir.as_deref(),
        )
    }

    impl ChildContext {
        /// The lock-order barrier body: report project-gate acquisition to the parent
        /// over stdout and a `reached-<nonce>` marker, then wait a finite time for the
        /// parent's `release-<nonce>` marker before returning so the deletion proceeds
        /// to the team guard. The finite watchdog means a dead parent cannot hang it.
        pub(crate) fn report_and_wait(&self) {
            let _ = std::fs::write(
                self.control_dir.join(format!("reached-{}", self.nonce)),
                b"reached",
            );
            println!("STAGE_D_LOCK_ORDER_REACHED {}", self.nonce);
            let release = self.control_dir.join(format!("release-{}", self.nonce));
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                if release.exists() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }

        /// Build and register an owned workgroup fixture (settings + `.ac` + team +
        /// `wg-1-dev-team` + the `_agent_*` matrices and member replica) under the
        /// control dir, isolating the CLI config directory via the debug
        /// `AGENTSCOMMANDER_TEST_CONFIG_DIR` override. Returns the project folder name
        /// to pass as `--project`. Runs only inside a driven child, never on no-op.
        pub(crate) fn build_workgroup_fixture(&self) -> String {
            let root = self.control_dir.join(format!("fixture-{}", self.nonce));
            let config_dir = root.join("config");
            let ac_root = root.join("Project").join(".ac");
            for dir in [
                config_dir.as_path(),
                &ac_root.join("_agent_coordinator"),
                &ac_root.join("_agent_member"),
                &ac_root.join("wg-1-dev-team").join("__agent_coordinator"),
                &ac_root.join("wg-1-dev-team").join("__agent_member"),
            ] {
                std::fs::create_dir_all(dir).expect("fixture dir");
            }
            std::env::set_var("AGENTSCOMMANDER_TEST_CONFIG_DIR", &config_dir);
            let settings = serde_json::json!({
                "defaultShell": "powershell.exe",
                "defaultShellArgs": [],
                "agents": [],
                "projectPaths": [root.to_string_lossy().to_string()],
            });
            std::fs::write(
                config_dir.join("settings.json"),
                serde_json::to_string_pretty(&settings).expect("settings json"),
            )
            .expect("write settings");
            let team_config = crate::commands::entity_creation::TeamConfigResult {
                agents: vec![
                    "_agent_coordinator".to_string(),
                    "_agent_member".to_string(),
                ],
                coordinator: "_agent_coordinator".to_string(),
                repos: Vec::new(),
                context_alert_percentages: Vec::new(),
            };
            crate::commands::entity_creation::create_new_team_config_on_disk(
                &ac_root,
                "dev-team",
                &team_config,
            )
            .expect("team config");
            "Project".to_string()
        }
    }

    /// Save/restore a set of env vars so an in-process driven-path test never leaks
    /// into the parallel suite. Restores on drop, including on panic.
    pub(crate) struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        pub(crate) fn capture(keys: &[&'static str]) -> Self {
            let saved = keys
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect();
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
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

    // #1063 Stage D-owned exact ignored CHILD HELPER for the CLI team-member
    // cross-process lock-order inversion. Frozen fully-qualified name (a Stage E
    // parent spawns this verbatim via `current_exe --exact <name> --ignored`):
    //   crate::cli::team::tests::cli_member_lock_order_inversion_child
    // No-ops (no guard, no mutation) unless the child-mode action + per-spawn nonce
    // + control dir are all supplied; when driven, it calls the real private
    // `remove_member_hooked` and drives the `after_project_before_team` barrier to
    // report project-gate acquisition (before the team guard) and wait for release.
    #[test]
    #[ignore]
    fn cli_member_lock_order_inversion_child() {
        let Some(ctx) = super::stage_d_lock_order_child::child_context(
            super::stage_d_lock_order_child::MEMBER_ACTION,
        ) else {
            return;
        };
        let project = ctx.build_workgroup_fixture();
        let args = TeamRemoveMemberArgs {
            project,
            workgroup: "wg-1-dev-team".to_string(),
            agent: "member".to_string(),
        };
        let result = remove_member_hooked(args, None, |_ac_root: &std::path::Path| {
            ctx.report_and_wait()
        });
        println!(
            "STAGE_D_LOCK_ORDER_DONE {} member ok={}",
            ctx.nonce,
            result.is_ok()
        );
    }

    // Deterministic (non-ignored) proof that the child-mode tuple validation no-ops
    // without a complete, valid tuple - so a bare ignored helper cannot act.
    #[test]
    fn cli_lock_order_child_context_no_ops_without_tuple() {
        use super::stage_d_lock_order_child::{context_from, MEMBER_ACTION};
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().to_str().expect("utf8 dir");
        assert!(context_from(MEMBER_ACTION, None, None, None).is_none());
        assert!(context_from(MEMBER_ACTION, Some("other"), Some("n"), Some(dir)).is_none());
        assert!(context_from(MEMBER_ACTION, Some(MEMBER_ACTION), Some(" "), Some(dir)).is_none());
        assert!(context_from(MEMBER_ACTION, Some(MEMBER_ACTION), Some("n"), None).is_none());
        assert!(context_from(
            MEMBER_ACTION,
            Some(MEMBER_ACTION),
            Some("n"),
            Some("no-such-dir-xyz")
        )
        .is_none());
        assert!(context_from(MEMBER_ACTION, Some(MEMBER_ACTION), Some("n"), Some(dir)).is_some());
    }

    // #1063: prove the driven lock-order path in-process (no `current_exe` parent -
    // that machinery is Stage E). Sets the child-mode tuple, drives the real
    // `cli_member_lock_order_inversion_child`, releases the barrier from a thread,
    // and asserts the barrier fired (project gate acquired before team) and the
    // member replica was removed after release. `#[ignore]` + env-guarded so the
    // parallel `--lib` regression is untouched. It also no-ops unless `DRIVE_VAR` is
    // set, so a bare `cargo test --lib -- --ignored` run never drives it. Enable and
    // isolate it deliberately (it pins the once-cached config dir, so it must own the
    // process): `AC_STAGE_D_LOCK_ORDER_DRIVE=1 cargo test --lib -- --ignored
    // --test-threads=1 --exact cli::team::tests::cli_member_lock_order_inversion_driver`.
    #[test]
    #[ignore]
    fn cli_member_lock_order_inversion_driver() {
        use super::stage_d_lock_order_child::{
            driver_enabled, EnvGuard, ACTION_VAR, CONTROL_DIR_VAR, MEMBER_ACTION, NONCE_VAR,
        };
        if !driver_enabled() {
            return;
        }
        let control = tempfile::tempdir().expect("tempdir");
        let nonce = "driver-member";
        let _guard = EnvGuard::capture(&[
            ACTION_VAR,
            NONCE_VAR,
            CONTROL_DIR_VAR,
            "AGENTSCOMMANDER_TEST_CONFIG_DIR",
        ]);
        std::env::set_var(ACTION_VAR, MEMBER_ACTION);
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

        cli_member_lock_order_inversion_child();
        releaser.join().expect("join releaser");

        assert!(
            control.path().join(format!("reached-{nonce}")).exists(),
            "the barrier must have fired (project gate acquired before team)"
        );
        let replica = control
            .path()
            .join(format!("fixture-{nonce}"))
            .join("Project")
            .join(".ac")
            .join("wg-1-dev-team")
            .join("__agent_member");
        assert!(
            !replica.exists(),
            "the member replica must be removed after the barrier releases"
        );
    }
}
