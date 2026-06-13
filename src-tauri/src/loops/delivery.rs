use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::config::agent_config::AgentLocalConfig;
use crate::config::loops::{
    resolve_loop_target, BusyCoordinatorPolicy, LoopAuditKind, LoopConfigToml,
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
                if let Err(e) = crate::commands::session::destroy_session_inner(app, stale_id).await
                {
                    log::warn!(
                        "[loops] Failed to clear stale coordinator session {} before wake: {}",
                        stale_id,
                        e
                    );
                }
            }
            match spawn_coordinator_session(app, &target, lookup.had_any_match).await {
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
                    message: "Coordinator is busy; delivery will run when idle".to_string(),
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
                    message: "Coordinator is busy; delivery skipped".to_string(),
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

    match crate::pty::inject::inject_text_into_session(app, session_id, &prompt).await {
        Ok(()) => {
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

#[derive(Debug)]
struct CoordinatorSessionLookup {
    live: Option<SessionInfo>,
    stale_session_ids: Vec<Uuid>,
    had_any_match: bool,
}

async fn find_coordinator_session(
    app: &AppHandle,
    coordinator_replica_dir: &Path,
) -> Result<CoordinatorSessionLookup, String> {
    let target_key = path_compare_key(coordinator_replica_dir);
    let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    let mgr = session_mgr.read().await;
    let mut matches = mgr
        .list_sessions()
        .await
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
) -> Result<SessionInfo, String> {
    let command = resolve_loop_agent_command(app, &target.coordinator_replica_dir).await?;
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
    let info = crate::commands::session::create_session_inner(
        app,
        session_mgr.inner(),
        pty_mgr.inner(),
        command.shell,
        command.shell_args,
        target.coordinator_replica_dir.to_string_lossy().to_string(),
        Some(session_name),
        command.agent_id,
        command.agent_label,
        false,
        Vec::<SessionRepo>::new(),
        loop_spawn_skip_auto_resume(had_existing_match),
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

fn loop_spawn_skip_auto_resume(_had_existing_match: bool) -> bool {
    false
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
) -> Result<ResolvedLoopAgentCommand, String> {
    let settings = {
        let settings_state = app.state::<SettingsState>();
        let settings = settings_state.read().await.clone();
        settings
    };

    let local_last = read_last_coding_agent(replica_dir);
    if let Some(agent_id) = local_last.as_deref() {
        if let Some(command) = command_for_agent(&settings, agent_id)? {
            return Ok(command);
        }
        log::warn!(
            "[loops] lastCodingAgent '{}' is no longer configured; falling back",
            agent_id
        );
    }

    if let Some(agent) = settings.agents.first() {
        let normalized = crate::config::agent_command::normalize_legacy_agent_command(
            &agent.command,
        )
        .map_err(|e| {
            format!(
                "Invalid agent command for configured agent '{}': {}",
                agent.id, e
            )
        })?;
        return Ok(ResolvedLoopAgentCommand {
            shell: normalized.shell,
            shell_args: normalized.shell_args,
            agent_id: Some(agent.id.clone()),
            agent_label: Some(agent.label.clone()),
        });
    }

    Err("No coding agent is configured for Loop coordinator wake".to_string())
}

fn command_for_agent(
    settings: &AppSettings,
    agent_id: &str,
) -> Result<Option<ResolvedLoopAgentCommand>, String> {
    let Some(agent) = settings.agents.iter().find(|agent| agent.id == agent_id) else {
        return Ok(None);
    };
    let normalized = crate::config::agent_command::normalize_legacy_agent_command(&agent.command)
        .map_err(|e| {
        format!(
            "Invalid agent command for configured agent '{}': {}",
            agent.id, e
        )
    })?;
    Ok(Some(ResolvedLoopAgentCommand {
        shell: normalized.shell,
        shell_args: normalized.shell_args,
        agent_id: Some(agent.id.clone()),
        agent_label: Some(agent.label.clone()),
    }))
}

fn read_last_coding_agent(replica_dir: &Path) -> Option<String> {
    let config_path = replica_dir.join("config.json");
    let content = std::fs::read_to_string(config_path).ok()?;
    let config = serde_json::from_str::<AgentLocalConfig>(&content).ok()?;
    config.tooling.last_coding_agent
}

fn path_compare_key(path: &Path) -> String {
    let resolved: PathBuf = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let value = resolved.to_string_lossy().to_string();
    let value = value.strip_prefix(r"\\?\").unwrap_or(&value).to_string();
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

    #[test]
    fn loop_spawn_allows_provider_resume_for_cold_and_known_state_wakes() {
        assert!(!loop_spawn_skip_auto_resume(false));
        assert!(!loop_spawn_skip_auto_resume(true));
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
}
