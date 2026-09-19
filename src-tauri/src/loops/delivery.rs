use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::config::agent_command::{build_agent_spawn_command, AgentSpawnCommand};
use crate::config::agent_config::AgentLocalConfig;
use crate::config::loops::{
    loop_dir, resolve_loop_target, revalidate_loop_current, BusyCoordinatorPolicy, LoopAuditKind,
    LoopConfigRevalidation, LoopConfigToml,
};
use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::{SessionInfo, SessionRepo, SessionStatus};

#[derive(Debug, Clone)]
pub struct LoopDeliveryReport {
    pub kind: LoopAuditKind,
    pub message: String,
    pub target: Option<String>,
    pub session_id: Option<Uuid>,
    pub error: Option<String>,
    pub prompt_snapshot: Option<String>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct ResolvedLoopAgentCommand {
    shell: String,
    shell_args: Vec<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    resolved_spawn: Option<AgentSpawnCommand>,
    /// #1271 - the configured host shell paired with the resolved agent, built
    /// from the same settings snapshot that produced the spawn.
    resolved_agent_host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>,
}

pub async fn deliver_loop_prompt(
    app: &AppHandle,
    project_dir: &Path,
    config: &LoopConfigToml,
    _run_id: Uuid,
    _due_at: DateTime<Utc>,
) -> LoopDeliveryReport {
    let target = match resolve_loop_target(project_dir, config) {
        Ok(target) => target,
        Err(e) => {
            return failed_report(None, None, e);
        }
    };
    let target_fqn = target.target_fqn.clone();

    // (#885 J1/J2) A purge is destroying this agent right now.
    // `deliver_loop_prompt` both injects into a live session and SPAWNS one
    // when none exists (`spawn_coordinator_session`), so an unguarded loop
    // tick can resurrect a peer the purge already destroyed. Keyed on the
    // FQN, not a session id, because the J2 case is precisely the one where
    // the session record is already gone.
    if let Some(g) = app.try_state::<std::sync::Arc<crate::session::purge_guard::PurgeGuard>>() {
        if g.blocks_agent(&target_fqn) {
            return LoopDeliveryReport {
                kind: LoopAuditKind::SkippedBusy,
                message: format!(
                    "purge-room in progress for '{}'; loop delivery skipped",
                    target_fqn
                ),
                target: Some(target_fqn),
                session_id: None,
                error: None,
                prompt_snapshot: None,
                completed_at: Some(Utc::now()),
            };
        }
    }

    let loop_storage_dir = loop_dir(&target.ac_root, &config.loop_def.id);
    let policy = config.policy.busy_coordinator.clone();
    let prompt = config.prompt.body.clone();

    let lookup = match find_coordinator_session(app, &target.coordinator_replica_dir).await {
        Ok(lookup) => lookup,
        Err(e) => return failed_report(Some(target_fqn), None, e),
    };

    let session = match lookup.live {
        Some(session) => session,
        None => {
            for stale_id in lookup.stale_session_ids {
                if let Err(e) =
                    crate::commands::session::background_destroy_session_inner(app, stale_id).await
                {
                    log::warn!(
                        "[loops] Failed to clear stale coordinator session {} before wake: {}",
                        stale_id,
                        e
                    );
                }
            }
            match spawn_coordinator_session(
                app,
                &target,
                lookup.had_any_match,
                lookup.persisted_agent.clone(),
            )
            .await
            {
                Ok(session) => session,
                Err(e) => return failed_report(Some(target_fqn), None, e),
            }
        }
    };

    let session_id = match Uuid::parse_str(&session.id) {
        Ok(id) => id,
        Err(e) => {
            return failed_report(
                Some(target_fqn),
                None,
                format!("Failed to parse session id '{}': {}", session.id, e),
            );
        }
    };

    match final_busy_check(app, session_id).await {
        Ok(true) => {}
        Ok(false) => match policy {
            BusyCoordinatorPolicy::ForceInject => {}
            BusyCoordinatorPolicy::WaitUntilIdle => {
                return LoopDeliveryReport {
                    kind: LoopAuditKind::PendingBusy,
                    message: "Orchestrator is busy; delivery will run when idle".to_string(),
                    target: Some(target_fqn),
                    session_id: Some(session_id),
                    error: None,
                    prompt_snapshot: None,
                    completed_at: None,
                };
            }
            BusyCoordinatorPolicy::Skip => {
                return LoopDeliveryReport {
                    kind: LoopAuditKind::SkippedBusy,
                    message: "Orchestrator is busy; delivery skipped".to_string(),
                    target: Some(target_fqn),
                    session_id: Some(session_id),
                    error: None,
                    prompt_snapshot: None,
                    completed_at: Some(Utc::now()),
                };
            }
        },
        Err(e) => return failed_report(Some(target_fqn), Some(session_id), e),
    }

    match crate::pty::inject::inject_text_into_session_with_pre_write_check(
        app,
        session_id,
        &prompt,
        || stale_delivery_error_if_needed(&loop_storage_dir, config, &target_fqn, session_id),
    )
    .await
    {
        Ok(()) => {
            // (#756) loop prompt = AC-injected post-boundary content: drop any
            // pending fresh intent (record + mirror).
            crate::commands::pty::note_post_boundary_content_to_session(app, session_id).await;
            if let Err(e) = set_last_prompt(app, session_id, prompt.clone()).await {
                log::warn!(
                    "[loops] Failed to update last_prompt after Loop delivery to {}: {}",
                    session_id,
                    e
                );
            }
            LoopDeliveryReport {
                kind: LoopAuditKind::Delivered,
                message: "Loop prompt delivered".to_string(),
                target: Some(target_fqn),
                session_id: Some(session_id),
                error: None,
                prompt_snapshot: Some(prompt),
                completed_at: Some(Utc::now()),
            }
        }
        Err(e) => failed_report(Some(target_fqn), Some(session_id), e),
    }
}

fn stale_delivery_error_if_needed(
    loop_storage_dir: &Path,
    config: &LoopConfigToml,
    target_fqn: &str,
    session_id: Uuid,
) -> Result<(), String> {
    match stale_delivery_report_if_needed(loop_storage_dir, config, target_fqn, session_id) {
        Some(report) => Err(report.error.unwrap_or(report.message)),
        None => Ok(()),
    }
}

fn stale_delivery_report_if_needed(
    loop_storage_dir: &Path,
    config: &LoopConfigToml,
    target_fqn: &str,
    session_id: Uuid,
) -> Option<LoopDeliveryReport> {
    match revalidate_loop_current(loop_storage_dir, config) {
        Ok(LoopConfigRevalidation::Current) => None,
        Ok(LoopConfigRevalidation::Gone) => Some(stale_delivery_report(
            loop_storage_dir,
            target_fqn,
            session_id,
        )),
        Ok(LoopConfigRevalidation::Disabled) => Some(stale_delivery_report(
            loop_storage_dir,
            target_fqn,
            session_id,
        )),
        Ok(LoopConfigRevalidation::Changed) => Some(stale_delivery_report(
            loop_storage_dir,
            target_fqn,
            session_id,
        )),
        Err(e) => Some(failed_report(
            Some(target_fqn.to_string()),
            Some(session_id),
            e,
        )),
    }
}

fn stale_delivery_report(
    loop_storage_dir: &Path,
    target_fqn: &str,
    session_id: Uuid,
) -> LoopDeliveryReport {
    let message = format!(
        "Loop config changed before prompt injection; skipped stale delivery for {}",
        loop_storage_dir.display()
    );
    log::warn!("[loops] {}", message);
    LoopDeliveryReport {
        kind: LoopAuditKind::DeliveryFailed,
        message: message.clone(),
        target: Some(target_fqn.to_string()),
        session_id: Some(session_id),
        error: Some(message),
        prompt_snapshot: None,
        completed_at: Some(Utc::now()),
    }
}

fn failed_report(
    target: Option<String>,
    session_id: Option<Uuid>,
    error: String,
) -> LoopDeliveryReport {
    LoopDeliveryReport {
        kind: LoopAuditKind::DeliveryFailed,
        message: error.clone(),
        target,
        session_id,
        error: Some(error),
        prompt_snapshot: None,
        completed_at: Some(Utc::now()),
    }
}

/// #2176 - the agent pin carried by the replica's persisted session record:
/// the `agentId` the session was launched with and its stored
/// `requestedProfile` (always from the same record).
#[derive(Debug, Clone, PartialEq)]
struct PersistedCoordinatorAgent {
    agent_id: String,
    requested_profile: Option<String>,
}

#[derive(Debug)]
struct CoordinatorSessionLookup {
    live: Option<SessionInfo>,
    stale_session_ids: Vec<Uuid>,
    had_any_match: bool,
    persisted_agent: Option<PersistedCoordinatorAgent>,
}

async fn find_coordinator_session(
    app: &AppHandle,
    coordinator_replica_dir: &Path,
) -> Result<CoordinatorSessionLookup, String> {
    let target_key = path_compare_key(coordinator_replica_dir);
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    let sessions = mgr.list_sessions().await;
    // #2176 - the persisted agent pin is extracted from the same unfiltered
    // snapshot the live/stale logic uses. The helper repeats the filter and
    // status ordering on purpose, so the extraction is one tested unit.
    let persisted_agent = persisted_agent_for_replica(&sessions, coordinator_replica_dir);
    let mut matches = sessions
        .into_iter()
        .filter(|session| path_compare_key(Path::new(&session.working_directory)) == target_key)
        .collect::<Vec<_>>();
    drop(mgr);

    matches.sort_by_key(|session| match session.status {
        SessionStatus::Active | SessionStatus::Running => 0u8,
        SessionStatus::Idle => 1,
        SessionStatus::Exited(_) => 2,
    });

    let had_any_match = !matches.is_empty();
    let mut stale_session_ids = Vec::new();
    for session in matches {
        let Ok(id) = Uuid::parse_str(&session.id) else {
            continue;
        };
        let has_pty = {
            if matches!(session.status, SessionStatus::Exited(_)) {
                false
            } else {
                let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
                let has_pty = pty_mgr
                    .lock()
                    .map_err(|_| "PtyManager lock poisoned".to_string())?
                    .has_session(id);
                has_pty
            }
        };
        if loop_candidate_is_live(&session.status, has_pty) {
            return Ok(CoordinatorSessionLookup {
                live: Some(session),
                stale_session_ids,
                had_any_match,
                persisted_agent,
            });
        }
        if loop_candidate_should_respawn(&session.status, has_pty) {
            if !matches!(session.status, SessionStatus::Exited(_)) {
                log::warn!(
                    "[loops] Skipping desync coordinator session {} with no PTY",
                    id
                );
            }
            stale_session_ids.push(id);
        }
    }

    Ok(CoordinatorSessionLookup {
        live: None,
        stale_session_ids,
        had_any_match,
        persisted_agent,
    })
}

/// #2176 - pure extraction of the replica's pinned agent from the unfiltered
/// `list_sessions()` output. It reproduces, in order, what
/// `find_coordinator_session` does to its own candidates: the same
/// working-directory filter, then the same status ordering (`sort_by_key` is
/// stable, so within one status rank the registry order is preserved). The
/// first remaining entry with an `agentId` wins; an entry without one is
/// skipped, not treated as a stop. The profile is always taken from the same
/// record as the agent id.
fn persisted_agent_for_replica(
    sessions: &[SessionInfo],
    replica_dir: &Path,
) -> Option<PersistedCoordinatorAgent> {
    let target_key = path_compare_key(replica_dir);
    let mut matches = sessions
        .iter()
        .filter(|session| path_compare_key(Path::new(&session.working_directory)) == target_key)
        .collect::<Vec<_>>();
    matches.sort_by_key(|session| match session.status {
        SessionStatus::Active | SessionStatus::Running => 0u8,
        SessionStatus::Idle => 1,
        SessionStatus::Exited(_) => 2,
    });
    matches.into_iter().find_map(|session| {
        session
            .agent_id
            .as_ref()
            .map(|agent_id| PersistedCoordinatorAgent {
                agent_id: agent_id.clone(),
                requested_profile: session.requested_profile.clone(),
            })
    })
}

fn loop_candidate_is_live(status: &SessionStatus, has_pty: bool) -> bool {
    match status {
        SessionStatus::Active | SessionStatus::Running | SessionStatus::Idle => has_pty,
        SessionStatus::Exited(_) => false,
    }
}

fn loop_candidate_should_respawn(status: &SessionStatus, has_pty: bool) -> bool {
    match status {
        SessionStatus::Exited(_) => true,
        SessionStatus::Active | SessionStatus::Running | SessionStatus::Idle => !has_pty,
    }
}

async fn spawn_coordinator_session(
    app: &AppHandle,
    target: &crate::config::loops::ResolvedLoopTarget,
    had_existing_match: bool,
    persisted_agent: Option<PersistedCoordinatorAgent>,
) -> Result<SessionInfo, String> {
    let command = resolve_loop_agent_command(
        app,
        &target.coordinator_replica_dir,
        persisted_agent.as_ref(),
    )
    .await?;
    let session_name = format!(
        "{}/{}",
        target
            .wg_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workgroup"),
        target.coordinator_agent_name
    );
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
    let cwd = crate::path_utils::path_to_string_without_windows_verbatim_prefix(
        &target.coordinator_replica_dir,
    );
    // #1873 - decided BEFORE the command's fields move into the create call.
    let skip_auto_resume = loop_spawn_skip_auto_resume(had_existing_match, &command);
    let info = crate::commands::session::create_session_inner(
        app,
        session_mgr.inner(),
        pty_mgr.inner(),
        command.shell,
        command.shell_args,
        cwd,
        Some(session_name),
        command.agent_id,
        command.agent_label,
        false,
        Vec::<SessionRepo>::new(),
        skip_auto_resume,
        command.resolved_spawn,
        // #1271 - the configured host shell paired with the resolved agent,
        // carried through ResolvedLoopAgentCommand from the same settings
        // snapshot that built the spawn.
        command.resolved_agent_host_shell,
        // #973 - headless caller: no terminal to measure, keep 120x30.
        None,
        crate::commands::session::CreateSelectionIntent::Background,
    )
    .await?;
    let session_id = Uuid::parse_str(&info.id)
        .map_err(|e| format!("Failed to parse spawned session id: {}", e))?;
    wait_for_session_idle(app, session_id).await?;
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    mgr.list_sessions()
        .await
        .into_iter()
        .find(|session| session.id == info.id)
        .ok_or_else(|| format!("Spawned session {} was not found", info.id))
}

/// Provider auto-resume policy for a loop spawn. Every prior provider keeps its
/// historical `false` (cold creation and known-state wake both allow the
/// provider's own resume logic). #1873: a cold Muse creation (`had_existing_match
/// == false`) on a trusted, exact, empty-argv local recipe must launch plain
/// `muse`; a Muse exited/missing-PTY wake (`true`) resumes with `resume --last`.
fn loop_spawn_skip_auto_resume(
    had_existing_match: bool,
    command: &ResolvedLoopAgentCommand,
) -> bool {
    !had_existing_match
        && crate::commands::session::trusted_muse_auto_resume_spawn(
            command.resolved_spawn.as_ref(),
            &command.shell,
            &command.shell_args,
        )
}

async fn wait_for_session_idle(app: &AppHandle, session_id: Uuid) -> Result<(), String> {
    let start = Instant::now();
    let max_wait = Duration::from_secs(90);
    let poll = Duration::from_millis(500);
    loop {
        if start.elapsed() >= max_wait {
            log::warn!(
                "[loops] Timeout waiting for spawned coordinator session {} to become idle",
                session_id
            );
            return Ok(());
        }
        tokio::time::sleep(poll).await;
        match final_busy_check(app, session_id).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) => return Err(e),
        }
    }
}

async fn final_busy_check(app: &AppHandle, session_id: Uuid) -> Result<bool, String> {
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    let session = mgr
        .list_sessions()
        .await
        .into_iter()
        .find(|session| session.id == session_id.to_string())
        .ok_or_else(|| format!("Session {} was destroyed before Loop delivery", session_id))?;
    Ok(session.waiting_for_input)
}

async fn set_last_prompt(app: &AppHandle, session_id: Uuid, prompt: String) -> Result<(), String> {
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    mgr.set_last_prompt(session_id, prompt.clone()).await;
    crate::config::sessions_persistence::persist_current_state(&mgr).await;
    let _ = tauri::Emitter::emit(
        app,
        "last_prompt",
        serde_json::json!({ "sessionId": session_id.to_string(), "text": prompt }),
    );
    Ok(())
}

async fn resolve_loop_agent_command(
    app: &AppHandle,
    replica_dir: &Path,
    persisted_agent: Option<&PersistedCoordinatorAgent>,
) -> Result<ResolvedLoopAgentCommand, String> {
    let replica_dir = crate::path_utils::normalize_windows_verbatim_path_buf(replica_dir);
    let replica_dir = replica_dir.as_path();
    let settings = {
        let settings_state = app.state::<SettingsState>();
        let settings = settings_state.read().await.clone();
        settings
    };

    resolve_loop_agent_command_from_settings(&settings, replica_dir, persisted_agent)
}

/// #2176 - the whole Loop wake agent decision, kept free of `AppHandle` so it
/// is directly unit-testable.
///
/// Rank 1 (`tooling.currentCodingAgent`, validated) and rank 2 (the persisted
/// session's `agentId`) both come through
/// `resolve_restart_selected_agent_id`, the manual restart path's own
/// function. Rank 3 (`tooling.lastCodingAgent`) is a deliberate legacy
/// allowance kept only for a replica that has neither of the first two; it is
/// not claimed as manual-path parity. `settings.agents.first()` is gone: a
/// replica with no pin fails closed instead of silently waking a different
/// agent.
fn resolve_loop_agent_command_from_settings(
    settings: &AppSettings,
    replica_dir: &Path,
    persisted_agent: Option<&PersistedCoordinatorAgent>,
) -> Result<ResolvedLoopAgentCommand, String> {
    let replica_dir_string =
        crate::path_utils::path_to_string_without_windows_verbatim_prefix(replica_dir);
    let selected = crate::commands::session::resolve_restart_selected_agent_id(
        settings,
        &replica_dir_string,
        None,
        persisted_agent.map(|p| p.agent_id.as_str()),
    );
    let requested_profile = persisted_agent.and_then(|p| p.requested_profile.clone());

    if let Some(selected_id) = selected.as_deref() {
        if let Some(command) = command_for_agent(
            settings,
            selected_id,
            replica_dir,
            requested_profile.as_deref(),
        )? {
            return Ok(command);
        }
        // `resolve_restart_selected_agent_id` validates rank 1 against
        // `settings.agents`, so an unconfigured `Some` can only be the
        // persisted `agentId`. That pin wins over `lastCodingAgent`, so a
        // deleted agent fails closed here and rank 3 is never consulted.
        let message = format!(
            "[loops] Replica '{}' is pinned to coding agent '{}', which is not configured; refusing to wake with a different agent",
            replica_dir_string, selected_id
        );
        log::warn!("{}", message);
        return Err(message);
    }

    if let Some(agent_id) = read_last_coding_agent(replica_dir) {
        if let Some(command) = command_for_agent(
            settings,
            &agent_id,
            replica_dir,
            requested_profile.as_deref(),
        )? {
            return Ok(command);
        }
        log::warn!(
            "[loops] lastCodingAgent '{}' is no longer configured; falling back",
            agent_id
        );
    }

    let message = format!(
        "[loops] No pinned coding agent for '{}': tooling.currentCodingAgent, the persisted session agentId and tooling.lastCodingAgent are all absent or not configured",
        replica_dir_string
    );
    log::warn!("{}", message);
    Err(message)
}

fn resolved_loop_command_from_spawn(
    spawn: AgentSpawnCommand,
    settings: &AppSettings,
) -> ResolvedLoopAgentCommand {
    ResolvedLoopAgentCommand {
        shell: spawn.shell.clone(),
        shell_args: spawn.shell_args.clone(),
        agent_id: Some(spawn.trusted_agent_id.clone()),
        agent_label: Some(spawn.trusted_agent_label.clone()),
        resolved_spawn: Some(spawn),
        // #1271 - same-snapshot host shell: `settings` is the exact snapshot
        // the caller used to build the spawn.
        resolved_agent_host_shell: Some(crate::pty::backend::ResolvedAgentHostShell {
            program: settings.default_shell.clone(),
            args: settings.default_shell_args.clone(),
        }),
    }
}

fn command_for_agent(
    settings: &AppSettings,
    agent_id: &str,
    replica_dir: &Path,
    requested_profile: Option<&str>,
) -> Result<Option<ResolvedLoopAgentCommand>, String> {
    let Some(agent) = settings.agents.iter().find(|agent| agent.id == agent_id) else {
        return Ok(None);
    };
    let spawn =
        build_agent_spawn_command(settings, &agent.id, Some(replica_dir), requested_profile)?;
    Ok(Some(resolved_loop_command_from_spawn(spawn, settings)))
}

fn read_last_coding_agent(replica_dir: &Path) -> Option<String> {
    let config_path = replica_dir.join("config.json");
    let content = std::fs::read_to_string(config_path).ok()?;
    let config = serde_json::from_str::<AgentLocalConfig>(&content).ok()?;
    config.tooling.last_coding_agent
}

fn path_compare_key(path: &Path) -> String {
    let resolved: PathBuf = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let value = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&resolved);
    let value = value.replace('\\', "/").trim_end_matches('/').to_string();
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::loops::{
        write_loop_config, LoopDef, LoopPolicy, LoopPrompt, LoopTarget, LoopTargetKind,
        LoopTrigger, LoopTriggerKind, LOOP_TIMEZONE_LOCAL,
    };
    use crate::config::settings::{AgentConfig, ProfileCellConfig};
    use std::collections::BTreeMap;

    #[test]
    #[cfg(windows)]
    fn path_compare_key_converts_verbatim_unc() {
        assert_eq!(
            path_compare_key(Path::new(r"\\?\UNC\server\share\repo")),
            "//server/share/repo"
        );
    }

    fn sample_config() -> LoopConfigToml {
        LoopConfigToml {
            loop_def: LoopDef {
                id: "daily-sync".to_string(),
                name: "Daily sync".to_string(),
                enabled: true,
            },
            trigger: LoopTrigger {
                kind: LoopTriggerKind::Cron,
                expr: "0 9 * * *".to_string(),
                timezone: LOOP_TIMEZONE_LOCAL.to_string(),
            },
            target: LoopTarget {
                kind: LoopTargetKind::WorkgroupCoordinator,
                workgroup: "wg-1-dev-team".to_string(),
            },
            prompt: LoopPrompt {
                body: "Send status".to_string(),
            },
            policy: LoopPolicy {
                busy_coordinator: BusyCoordinatorPolicy::WaitUntilIdle,
                ..LoopPolicy::default()
            },
        }
    }

    fn make_inject_test_app(session_mgr: Arc<tokio::sync::RwLock<SessionManager>>) -> tauri::App {
        let app = crate::test_support::test_builder()
            .manage(Arc::clone(&session_mgr))
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build inject test app");
        let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
        let git_watcher = crate::pty::git_watcher::GitWatcher::new(
            Arc::clone(&session_mgr),
            app.handle().clone(),
        );
        let pty_manager = Arc::new(Mutex::new(PtyManager::new(
            Arc::new(Mutex::new(std::collections::HashMap::new())),
            idle_detector,
            git_watcher,
            None,
            None,
        )));
        app.manage(pty_manager);
        app
    }

    fn loop_command_for(
        settings: &crate::config::settings::AppSettings,
        agent_id: &str,
    ) -> ResolvedLoopAgentCommand {
        let spawn = crate::config::agent_command::resolve_agent_spawn_command(
            settings, agent_id, None, None, false,
        )
        .expect("loop test spawn should resolve without filesystem preparation");
        ResolvedLoopAgentCommand {
            shell: spawn.shell.clone(),
            shell_args: spawn.shell_args.clone(),
            agent_id: Some(spawn.trusted_agent_id.clone()),
            agent_label: Some(spawn.trusted_agent_label.clone()),
            resolved_spawn: Some(spawn),
            resolved_agent_host_shell: None,
        }
    }

    /// #1873 - cold Muse is fresh, known-state Muse resumes, and every prior
    /// provider (plus ad-hoc, argument-bearing and recipe-mismatched Muse) keeps
    /// the historical `false` on both branches.
    #[test]
    fn muse_loop_cold_is_fresh_and_known_state_resumes_without_provider_drift() {
        use crate::config::settings::{AgentConfig, AppSettings};
        let agent = |id: &str, command: &str| AgentConfig {
            id: id.to_string(),
            label: id.to_string(),
            command: command.to_string(),
            color: "#000000".to_string(),
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        };
        let settings = AppSettings {
            agents: vec![
                agent("muse", "muse"),
                agent("muse-abs", "/opt/muse/bin/muse"),
                agent("muse-args", "muse --workspace /srv/work"),
                agent("muse-manual", "muse resume --last"),
                agent("claude", "claude"),
                agent("codex", "codex"),
                agent("agy", "agy"),
                agent("pi", "pi"),
            ],
            ..AppSettings::default()
        };

        let expected_muse_cold = cfg!(any(target_os = "macos", target_os = "linux"));
        for id in ["muse", "muse-abs"] {
            let command = loop_command_for(&settings, id);
            assert_eq!(
                loop_spawn_skip_auto_resume(false, &command),
                expected_muse_cold,
                "cold Muse creation is fresh on supported hosts, id={id}"
            );
            assert!(
                !loop_spawn_skip_auto_resume(true, &command),
                "known-state Muse wake resumes, id={id}"
            );
        }

        // Configured arguments, prior providers: unchanged `false` on both branches.
        for id in ["muse-args", "muse-manual", "claude", "codex", "agy", "pi"] {
            let command = loop_command_for(&settings, id);
            assert!(!loop_spawn_skip_auto_resume(false, &command), "id={id}");
            assert!(!loop_spawn_skip_auto_resume(true, &command), "id={id}");
        }

        // Ad-hoc Muse (no resolved spawn) and a recipe mismatch never qualify.
        let mut adhoc = loop_command_for(&settings, "muse");
        adhoc.resolved_spawn = None;
        assert!(!loop_spawn_skip_auto_resume(false, &adhoc));
        assert!(!loop_spawn_skip_auto_resume(true, &adhoc));
        let mut mismatch = loop_command_for(&settings, "muse");
        mismatch.shell_args = vec!["--workspace".to_string(), "/srv/work".to_string()];
        assert!(!loop_spawn_skip_auto_resume(false, &mismatch));
        assert!(!loop_spawn_skip_auto_resume(true, &mismatch));
    }

    #[test]
    fn loop_candidate_rules_preserve_exited_and_phantom_respawn_paths() {
        assert!(loop_candidate_should_respawn(
            &SessionStatus::Exited(0),
            false
        ));
        assert!(loop_candidate_should_respawn(&SessionStatus::Idle, false));
        assert!(!loop_candidate_should_respawn(&SessionStatus::Idle, true));
        assert!(loop_candidate_is_live(&SessionStatus::Idle, true));
        assert!(!loop_candidate_is_live(&SessionStatus::Exited(0), false));
    }

    #[test]
    fn final_revalidation_blocks_prompt_change_before_inject() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = sample_config();
        let dir = write_loop_config(tmp.path(), &config).expect("write config");
        let session_id = Uuid::new_v4();

        assert!(stale_delivery_report_if_needed(
            &dir,
            &config,
            "project:wg-1-dev-team/tech-lead",
            session_id
        )
        .is_none());

        let mut changed = config.clone();
        changed.prompt.body = "New prompt".to_string();
        write_loop_config(tmp.path(), &changed).expect("write changed config");

        let report = stale_delivery_report_if_needed(
            &dir,
            &config,
            "project:wg-1-dev-team/tech-lead",
            session_id,
        )
        .expect("stale report");

        assert_eq!(report.kind, LoopAuditKind::DeliveryFailed);
        assert_eq!(
            report.target.as_deref(),
            Some("project:wg-1-dev-team/tech-lead")
        );
        assert_eq!(report.session_id, Some(session_id));
        assert!(report.prompt_snapshot.is_none());
        assert!(report.message.contains("skipped stale delivery"));
    }

    #[tokio::test]
    async fn pre_write_revalidation_blocks_stale_loop_after_inject_shell_lookup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = sample_config();
        let dir = write_loop_config(tmp.path(), &config).expect("write config");

        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let app = make_inject_test_app(session_mgr.clone());
        let session = {
            let mgr = session_mgr.read().await;
            mgr.create_session(
                "codex".to_string(),
                Vec::new(),
                tmp.path().to_string_lossy().to_string(),
                None,
                None,
                Vec::<SessionRepo>::new(),
                false,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            )
            .await
            .expect("create session")
        };
        let session_id = session.id;
        app.state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap()
            .record_route(
                session_id,
                crate::pty::backend::SessionBackendKind::LocalProcess,
            );

        let mut changed = config.clone();
        changed.prompt.body = "New prompt".to_string();
        write_loop_config(tmp.path(), &changed).expect("write changed config");

        let result = crate::pty::inject::inject_text_into_session_with_pre_write_check(
            &app.handle().clone(),
            session_id,
            &config.prompt.body,
            || {
                stale_delivery_error_if_needed(
                    &dir,
                    &config,
                    "project:wg-1-dev-team/tech-lead",
                    session_id,
                )
            },
        )
        .await;

        let err = result.expect_err("stale loop should block before PTY write");
        assert!(err.contains("skipped stale delivery"));
    }

    #[test]
    fn final_revalidation_blocks_policy_change_and_deleted_config_before_inject() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config = sample_config();
        let dir = write_loop_config(tmp.path(), &config).expect("write config");
        let session_id = Uuid::new_v4();

        let mut changed = config.clone();
        changed.policy.busy_coordinator = BusyCoordinatorPolicy::Skip;
        write_loop_config(tmp.path(), &changed).expect("write changed config");
        assert!(stale_delivery_report_if_needed(
            &dir,
            &config,
            "project:wg-1-dev-team/tech-lead",
            session_id
        )
        .is_some());

        std::fs::remove_dir_all(&dir).expect("remove loop dir");
        assert!(stale_delivery_report_if_needed(
            &dir,
            &config,
            "project:wg-1-dev-team/tech-lead",
            session_id
        )
        .is_some());
        assert!(!dir.exists());
    }

    // ── #2176 - Loop wake agent resolution ──

    fn loop_test_agent(id: &str, command: &str) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            label: id.to_string(),
            command: command.to_string(),
            color: "#000000".to_string(),
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }
    }

    /// Two agents with codex first, plus one enabled profile cell carrying a
    /// distinctive env row. As in `config/agent_command.rs`'s fixtures, codex
    /// being first is what makes any surviving `settings.agents.first()`
    /// behavior fail the assertions.
    fn loop_test_settings_with_cell(
        agent_id: &str,
        letter: &str,
        env_key: &str,
        env_value: &str,
    ) -> AppSettings {
        let mut settings = AppSettings {
            agents: vec![
                loop_test_agent("codex", "codex"),
                loop_test_agent("claude", "claude"),
            ],
            ..AppSettings::default()
        };
        settings
            .coding_agent_profiles
            .profiles_by_agent
            .entry(agent_id.to_string())
            .or_default()
            .insert(
                letter.to_string(),
                ProfileCellConfig {
                    enabled: true,
                    command: String::new(),
                    env: BTreeMap::from([(env_key.to_string(), env_value.to_string())]),
                    notes: String::new(),
                },
            );
        settings
    }

    /// A real replica directory so `path_compare_key`'s `canonicalize`
    /// succeeds, laid out as `<tmp>/.ac/wg-7-dev-team/__agent_dev-rust`.
    fn loop_test_replica_dir(tmp: &tempfile::TempDir) -> PathBuf {
        let replica = tmp
            .path()
            .join(".ac")
            .join("wg-7-dev-team")
            .join("__agent_dev-rust");
        std::fs::create_dir_all(&replica).expect("create replica dir");
        replica
    }

    fn write_replica_config(replica: &Path, tooling: &str) {
        std::fs::write(
            replica.join("config.json"),
            format!(r#"{{"tooling":{}}}"#, tooling),
        )
        .expect("write replica config.json");
    }

    fn session_info_for_test(
        id: &str,
        cwd: &str,
        status: SessionStatus,
        agent_id: Option<&str>,
        requested_profile: Option<&str>,
    ) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            name: id.to_string(),
            shell: "claude".to_string(),
            shell_args: Vec::new(),
            backend_kind: crate::pty::backend::SessionBackendKind::LocalProcess,
            effective_shell_args: None,
            created_at: "2026-05-16T00:00:00Z".to_string(),
            working_directory: cwd.to_string(),
            status,
            waiting_for_input: false,
            communication: None,
            pending_review: false,
            last_prompt: None,
            agent_id: agent_id.map(str::to_string),
            agent_label: agent_id.map(str::to_string),
            git_repos: Vec::new(),
            workgroup_task: None,
            is_coordinator: true,
            is_root_agent: false,
            token: "t".to_string(),
            agent_kind: None,
            requested_profile: requested_profile.map(str::to_string),
            effective_profile: None,
            profile_fallback_chain: Vec::new(),
            profile_fallback_applied: false,
            effective_codex_home: None,
            profile_content_hash: None,
            trusted_configured_spawn: false,
            profile_outdated: false,
            telegram_bot_id: None,
            was_detached: false,
            detached_geometry: None,
            start_fresh_on_restore: false,
            context_percent: None,
        }
    }

    /// Issue acceptance criterion 4: no `lastCodingAgent`, no
    /// `currentCodingAgent`, still resolves the persisted session's agent and
    /// its stored profile letter, and the profile cell is actually applied.
    #[test]
    fn loop_resolves_persisted_agent_and_profile_when_last_coding_agent_is_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let replica = loop_test_replica_dir(&tmp);
        write_replica_config(&replica, "{}");
        let settings =
            loop_test_settings_with_cell("claude", "E", "LOOP_TEST_PROFILE_E", "applied");
        let persisted = PersistedCoordinatorAgent {
            agent_id: "claude".to_string(),
            requested_profile: Some("E".to_string()),
        };

        let command =
            resolve_loop_agent_command_from_settings(&settings, &replica, Some(&persisted))
                .expect("persisted agent should resolve");

        assert_eq!(command.agent_id.as_deref(), Some("claude"));
        let spawn = command.resolved_spawn.expect("resolved spawn");
        assert_eq!(spawn.trusted_agent_id, "claude");
        assert_eq!(spawn.profile_resolution.requested_profile, "E");
        assert!(
            spawn
                .child_env
                .iter()
                .any(|(key, value)| key == "LOOP_TEST_PROFILE_E" && value == "applied"),
            "profile cell env row must be applied, child_env={:?}",
            spawn.child_env
        );
    }

    /// Rank 1 (`currentCodingAgent`) outranks rank 2 (the persisted agentId).
    #[test]
    fn loop_prefers_current_coding_agent_over_persisted_agent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let replica = loop_test_replica_dir(&tmp);
        write_replica_config(&replica, r#"{"currentCodingAgent":"claude"}"#);
        let settings =
            loop_test_settings_with_cell("claude", "E", "LOOP_TEST_PROFILE_E", "applied");
        let persisted = PersistedCoordinatorAgent {
            agent_id: "codex".to_string(),
            requested_profile: None,
        };

        let command =
            resolve_loop_agent_command_from_settings(&settings, &replica, Some(&persisted))
                .expect("currentCodingAgent should resolve");

        assert_eq!(command.agent_id.as_deref(), Some("claude"));
    }

    /// Rank 3 is the explicit legacy tail: no persisted record and no
    /// `currentCodingAgent`, but a configured `lastCodingAgent`.
    #[test]
    fn loop_falls_back_to_last_coding_agent_when_no_session_record() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let replica = loop_test_replica_dir(&tmp);
        write_replica_config(&replica, r#"{"lastCodingAgent":"claude"}"#);
        let settings =
            loop_test_settings_with_cell("claude", "E", "LOOP_TEST_PROFILE_E", "applied");

        let command = resolve_loop_agent_command_from_settings(&settings, &replica, None)
            .expect("legacy lastCodingAgent tail should resolve");

        assert_eq!(command.agent_id.as_deref(), Some("claude"));
    }

    /// The deleted `settings.agents.first()` fallback: with no pin at all the
    /// resolver must fail closed instead of waking codex just because it is
    /// first in `settings.agents`.
    #[test]
    fn loop_resolution_fails_closed_without_any_pinned_agent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let replica = loop_test_replica_dir(&tmp);
        write_replica_config(&replica, "{}");
        let settings =
            loop_test_settings_with_cell("claude", "E", "LOOP_TEST_PROFILE_E", "applied");

        let err = resolve_loop_agent_command_from_settings(&settings, &replica, None)
            .expect_err("no pin must fail closed");

        assert!(
            err.contains("No pinned coding agent"),
            "unexpected error: {err}"
        );
    }

    /// Decision 2.1: a persisted record naming a deleted agent is the pin, so
    /// resolution errors instead of falling back to `lastCodingAgent`.
    #[test]
    fn loop_fails_closed_when_persisted_agent_is_not_configured() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let replica = loop_test_replica_dir(&tmp);
        write_replica_config(&replica, r#"{"lastCodingAgent":"codex"}"#);
        let settings =
            loop_test_settings_with_cell("claude", "E", "LOOP_TEST_PROFILE_E", "applied");
        let persisted = PersistedCoordinatorAgent {
            agent_id: "ghost".to_string(),
            requested_profile: Some("E".to_string()),
        };

        let err = resolve_loop_agent_command_from_settings(&settings, &replica, Some(&persisted))
            .expect_err("an unconfigured pin must fail closed");

        assert!(err.contains("ghost"), "unexpected error: {err}");
        assert!(err.contains("not configured"), "unexpected error: {err}");
    }

    /// Most-live status first, then registry insertion order; an agent-less
    /// record is skipped and its profile is never borrowed.
    #[test]
    fn persisted_agent_skips_records_without_an_agent_and_pins_the_tie_break() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let replica = loop_test_replica_dir(&tmp);
        let sibling = tmp.path().join("sibling");
        std::fs::create_dir_all(&sibling).expect("create sibling dir");
        let replica_cwd = replica.to_string_lossy().to_string();
        let sibling_cwd = sibling.to_string_lossy().to_string();

        let rows = vec![
            session_info_for_test(
                "sibling",
                &sibling_cwd,
                SessionStatus::Idle,
                Some("codex"),
                Some("Z"),
            ),
            session_info_for_test(
                "no-agent",
                &replica_cwd,
                SessionStatus::Idle,
                None,
                Some("Z"),
            ),
            session_info_for_test(
                "claude",
                &replica_cwd,
                SessionStatus::Idle,
                Some("claude"),
                Some("E"),
            ),
            session_info_for_test(
                "codex",
                &replica_cwd,
                SessionStatus::Idle,
                Some("codex"),
                Some("B"),
            ),
        ];

        assert_eq!(
            persisted_agent_for_replica(&rows, &replica),
            Some(PersistedCoordinatorAgent {
                agent_id: "claude".to_string(),
                requested_profile: Some("E".to_string()),
            })
        );
    }

    #[test]
    fn persisted_agent_prefers_the_most_live_status() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let replica = loop_test_replica_dir(&tmp);
        let replica_cwd = replica.to_string_lossy().to_string();

        let rows = vec![
            session_info_for_test(
                "codex",
                &replica_cwd,
                SessionStatus::Exited(0),
                Some("codex"),
                Some("B"),
            ),
            session_info_for_test(
                "claude",
                &replica_cwd,
                SessionStatus::Active,
                Some("claude"),
                Some("E"),
            ),
        ];

        assert_eq!(
            persisted_agent_for_replica(&rows, &replica),
            Some(PersistedCoordinatorAgent {
                agent_id: "claude".to_string(),
                requested_profile: Some("E".to_string()),
            })
        );
        assert_eq!(persisted_agent_for_replica(&[], &replica), None);
    }
}
