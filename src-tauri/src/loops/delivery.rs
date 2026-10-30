use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::config::agent_command::{build_agent_spawn_command, AgentSpawnCommand};
use crate::config::loops::{
    loop_dir, resolve_loop_target, revalidate_loop_current, BusyCoordinatorPolicy, LoopAuditKind,
    LoopConfigRevalidation, LoopConfigToml, LoopSessionStart,
};
use crate::config::settings::{AppSettings, SettingsState};
use crate::filesystem::loop_files;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::{SessionInfo, SessionStatus};

pub(crate) trait LoopPromptEvents {
    fn last_prompt(&self, session_id: Uuid, text: &str);
}

pub(crate) struct LoopSessionCreate {
    pub(crate) shell: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) cwd: String,
    pub(crate) session_name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) skip_auto_resume: bool,
    pub(crate) resolved_spawn: Option<AgentSpawnCommand>,
    pub(crate) resolved_agent_host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>,
}

pub(crate) trait LoopDeliveryHost: LoopPromptEvents + Send + Sync {
    fn settings_owner(&self) -> &SettingsState;
    fn sessions(&self) -> &Arc<tokio::sync::RwLock<SessionManager>>;
    fn ptys(&self) -> &Arc<Mutex<PtyManager>>;
    fn purge_guard(&self) -> Option<&Arc<crate::session::purge_guard::PurgeGuard>>;
    fn background_destroy(
        &self,
        id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;
    fn restart_fresh(
        &self,
        id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<SessionInfo, String>> + Send + '_>>;
    fn create(
        &self,
        request: LoopSessionCreate,
    ) -> Pin<Box<dyn Future<Output = Result<SessionInfo, String>> + Send + '_>>;
    fn inject<'a>(
        &'a self,
        id: Uuid,
        text: &'a str,
        pre_write: Box<dyn FnOnce() -> Result<(), String> + Send + 'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
    fn note_post_boundary(&self, id: Uuid) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

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
pub(crate) struct ResolvedLoopAgentCommand {
    shell: String,
    shell_args: Vec<String>,
    agent_id: Option<String>,
    agent_label: Option<String>,
    resolved_spawn: Option<AgentSpawnCommand>,
    /// #1271 - the configured host shell paired with the resolved agent, built
    /// from the same settings snapshot that produced the spawn.
    resolved_agent_host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>,
}

pub async fn deliver_loop_prompt<H: LoopDeliveryHost + ?Sized>(
    app: &H,
    project_dir: &Path,
    config: &LoopConfigToml,
    _run_id: Uuid,
    _due_at: DateTime<Utc>,
) -> LoopDeliveryReport {
    let target = match resolve_loop_target(project_dir, config) {
        Ok(target) => target,
        Err(e) => return target_unresolved_report(e),
    };
    let target_fqn = target.target_fqn.clone();

    // (#885 J1/J2) A purge is destroying this agent right now.
    // `deliver_loop_prompt` both injects into a live session and SPAWNS one
    // when none exists (`spawn_coordinator_session`), so an unguarded loop
    // tick can resurrect a peer the purge already destroyed. Keyed on the
    // FQN, not a session id, because the J2 case is precisely the one where
    // the session record is already gone.
    if let Some(g) = app.purge_guard() {
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
    let session_start = config.policy.session_start;
    let prompt = config.prompt.body.clone();

    let lookup = match find_coordinator_session(app, &target.coordinator_replica_dir).await {
        Ok(lookup) => lookup,
        Err(e) => return failed_report(Some(target_fqn), None, e),
    };

    let had_live_session = lookup.live.is_some();
    let session = match lookup.live {
        Some(session) => session,
        None => {
            for stale_id in lookup.stale_session_ids {
                if let Err(e) = app.background_destroy(stale_id).await {
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
                session_start,
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

    let session_id = if session_start == LoopSessionStart::Fresh && had_live_session {
        // Fresh + live: the stale check and the busy gate run before any
        // teardown, then at most one restart, an explicit wait, and the
        // post-restart outcome. This replaces the busy block below on this path.
        match restart_delivery_session(
            app,
            &loop_storage_dir,
            config,
            &target_fqn,
            session_id,
            &policy,
        )
        .await
        {
            Ok(id) => id,
            Err(report) => return *report,
        }
    } else {
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
        session_id
    };

    match app
        .inject(
            session_id,
            &prompt,
            Box::new(|| {
                stale_delivery_error_if_needed(&loop_storage_dir, config, &target_fqn, session_id)
            }),
        )
        .await
    {
        Ok(()) => delivered_report(app, target_fqn, session_id, prompt).await,
        Err(e) => failed_report(Some(target_fqn), Some(session_id), e),
    }
}

async fn delivered_report<H: LoopDeliveryHost + ?Sized>(
    app: &H,
    target_fqn: String,
    session_id: Uuid,
    prompt: String,
) -> LoopDeliveryReport {
    // (#756) loop prompt = AC-injected post-boundary content: drop any
    // pending fresh intent (record + mirror).
    app.note_post_boundary(session_id).await;
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

async fn restart_delivery_session<H: LoopDeliveryHost + ?Sized>(
    app: &H,
    loop_storage_dir: &Path,
    config: &LoopConfigToml,
    target_fqn: &str,
    session_id: Uuid,
    policy: &BusyCoordinatorPolicy,
) -> Result<Uuid, Box<LoopDeliveryReport>> {
    let target_fqn = target_fqn.to_string();
    if let Some(report) =
        stale_delivery_report_if_needed(loop_storage_dir, config, &target_fqn, session_id)
    {
        return Err(Box::new(report));
    }
    let pre_restart_idle = match final_busy_check(app, session_id).await {
        Ok(idle) => idle,
        Err(e) => {
            return Err(Box::new(failed_report(
                Some(target_fqn),
                Some(session_id),
                e,
            )))
        }
    };
    match pre_restart_decision(pre_restart_idle, policy) {
        PreRestartDecision::Restart => {}
        PreRestartDecision::Pending => {
            return Err(Box::new(LoopDeliveryReport {
                kind: LoopAuditKind::PendingBusy,
                message: "Orchestrator is busy; delivery will run when idle".to_string(),
                target: Some(target_fqn),
                session_id: Some(session_id),
                error: None,
                prompt_snapshot: None,
                completed_at: None,
            }));
        }
        PreRestartDecision::Skip => {
            return Err(Box::new(LoopDeliveryReport {
                kind: LoopAuditKind::SkippedBusy,
                message: "Orchestrator is busy; delivery skipped".to_string(),
                target: Some(target_fqn),
                session_id: Some(session_id),
                error: None,
                prompt_snapshot: None,
                completed_at: Some(Utc::now()),
            }));
        }
    }

    let restarted = match app.restart_fresh(session_id).await {
        Ok(info) => info,
        Err(e) => {
            return Err(Box::new(failed_report(
                Some(target_fqn),
                Some(session_id),
                e,
            )))
        }
    };

    let post_restart_id = match Uuid::parse_str(&restarted.id) {
        Ok(id) => id,
        Err(e) => {
            return Err(Box::new(failed_report(
                Some(target_fqn),
                Some(session_id),
                format!("Failed to parse session id '{}': {}", restarted.id, e),
            )));
        }
    };

    if let Err(e) = wait_for_session_idle(app, post_restart_id).await {
        return Err(Box::new(failed_report(
            Some(target_fqn),
            Some(post_restart_id),
            e,
        )));
    }

    let post_restart_idle = match final_busy_check(app, post_restart_id).await {
        Ok(idle) => idle,
        Err(e) => {
            return Err(Box::new(failed_report(
                Some(target_fqn),
                Some(post_restart_id),
                e,
            )))
        }
    };
    match post_restart_decision(post_restart_idle, policy) {
        PostRestartDecision::Inject => {}
        PostRestartDecision::Skip => {
            return Err(Box::new(LoopDeliveryReport {
                kind: LoopAuditKind::SkippedBusy,
                message: "Orchestrator is busy; delivery skipped".to_string(),
                target: Some(target_fqn),
                session_id: Some(post_restart_id),
                error: None,
                prompt_snapshot: None,
                completed_at: Some(Utc::now()),
            }));
        }
        PostRestartDecision::Failed => {
            return Err(Box::new(failed_report(
                Some(target_fqn),
                Some(post_restart_id),
                "coordinator did not become idle within 90s after the fresh restart".to_string(),
            )));
        }
    }
    Ok(post_restart_id)
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

/// #2733 - the target room/orchestrator does not resolve on this machine.
/// Its own kind so the scheduler notifies once per transition, not per tick.
fn target_unresolved_report(error: String) -> LoopDeliveryReport {
    LoopDeliveryReport {
        kind: LoopAuditKind::TargetUnresolved,
        message: error.clone(),
        target: None,
        session_id: None,
        error: Some(error),
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

async fn find_coordinator_session<H: LoopDeliveryHost + ?Sized>(
    app: &H,
    coordinator_replica_dir: &Path,
) -> Result<CoordinatorSessionLookup, String> {
    let target_key = path_compare_key(coordinator_replica_dir);
    let session_mgr = app.sessions();
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
                let pty_mgr = app.ptys();
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

async fn spawn_coordinator_session<H: LoopDeliveryHost + ?Sized>(
    app: &H,
    target: &crate::config::loops::ResolvedLoopTarget,
    had_existing_match: bool,
    persisted_agent: Option<PersistedCoordinatorAgent>,
    session_start: LoopSessionStart,
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
    let cwd = crate::path_utils::path_to_string_without_windows_verbatim_prefix(
        &target.coordinator_replica_dir,
    );
    let skip_auto_resume = spawn_skip_auto_resume(session_start, had_existing_match, &command);
    let info = app
        .create(LoopSessionCreate {
            shell: command.shell,
            shell_args: command.shell_args,
            cwd,
            session_name: Some(session_name),
            agent_id: command.agent_id,
            agent_label: command.agent_label,
            skip_auto_resume,
            resolved_spawn: command.resolved_spawn,
            resolved_agent_host_shell: command.resolved_agent_host_shell,
        })
        .await?;
    let session_id = Uuid::parse_str(&info.id)
        .map_err(|e| format!("Failed to parse spawned session id: {}", e))?;
    wait_for_session_idle(app, session_id).await?;
    let session_mgr = app.sessions();
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

/// Epic §4.5 step 2 - the pre-restart busy gate. Only an idle live session, or
/// an explicit `ForceInject`, may cost the live session; a busy `WaitUntilIdle`
/// keeps the run pending without restarting and a busy `Skip` drops it.
pub(crate) enum PreRestartDecision {
    Restart,
    Pending,
    Skip,
}

pub(crate) fn pre_restart_decision(
    idle: bool,
    policy: &BusyCoordinatorPolicy,
) -> PreRestartDecision {
    if idle {
        return PreRestartDecision::Restart;
    }
    match policy {
        BusyCoordinatorPolicy::WaitUntilIdle => PreRestartDecision::Pending,
        BusyCoordinatorPolicy::ForceInject => PreRestartDecision::Restart,
        BusyCoordinatorPolicy::Skip => PreRestartDecision::Skip,
    }
}

/// Epic §4.5 step 5 - the post-restart outcome. Deliberately has no `Pending`:
/// `PendingBusy` is the only kind that keeps the retry alive, so returning it
/// here would restart the same session on every retry forever (hazard H2).
pub(crate) enum PostRestartDecision {
    Inject,
    Skip,
    Failed,
}

pub(crate) fn post_restart_decision(
    idle: bool,
    policy: &BusyCoordinatorPolicy,
) -> PostRestartDecision {
    if idle {
        return PostRestartDecision::Inject;
    }
    match policy {
        BusyCoordinatorPolicy::WaitUntilIdle => PostRestartDecision::Failed,
        BusyCoordinatorPolicy::ForceInject => PostRestartDecision::Inject,
        BusyCoordinatorPolicy::Skip => PostRestartDecision::Skip,
    }
}

/// The `sessionStart` choice applied to a cold spawn: `Fresh` always suppresses
/// provider auto-resume; `Accumulate` keeps the historical cold/wake rule.
pub(crate) fn spawn_skip_auto_resume(
    session_start: LoopSessionStart,
    had_existing_match: bool,
    command: &ResolvedLoopAgentCommand,
) -> bool {
    match session_start {
        LoopSessionStart::Fresh => true,
        LoopSessionStart::Accumulate => loop_spawn_skip_auto_resume(had_existing_match, command),
    }
}

async fn wait_for_session_idle<H: LoopDeliveryHost + ?Sized>(
    app: &H,
    session_id: Uuid,
) -> Result<(), String> {
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

async fn final_busy_check<H: LoopDeliveryHost + ?Sized>(
    app: &H,
    session_id: Uuid,
) -> Result<bool, String> {
    let session_mgr = app.sessions();
    let mgr = session_mgr.read().await;
    let session = mgr
        .list_sessions()
        .await
        .into_iter()
        .find(|session| session.id == session_id.to_string())
        .ok_or_else(|| format!("Session {} was destroyed before Loop delivery", session_id))?;
    Ok(session.waiting_for_input)
}

async fn set_last_prompt<H: LoopDeliveryHost + ?Sized>(
    app: &H,
    session_id: Uuid,
    prompt: String,
) -> Result<(), String> {
    let session_mgr = app.sessions();
    let mgr = session_mgr.read().await;
    mgr.set_last_prompt(session_id, prompt.clone()).await;
    crate::config::sessions_persistence::persist_current_state(&mgr).await;
    app.last_prompt(session_id, &prompt);
    Ok(())
}

async fn resolve_loop_agent_command<H: LoopDeliveryHost + ?Sized>(
    app: &H,
    replica_dir: &Path,
    persisted_agent: Option<&PersistedCoordinatorAgent>,
) -> Result<ResolvedLoopAgentCommand, String> {
    let replica_dir = crate::path_utils::normalize_windows_verbatim_path_buf(replica_dir);
    let replica_dir = replica_dir.as_path();
    let settings = {
        let settings_state = app.settings_owner();
        let settings = settings_state
            .current_snapshot()
            .await
            .map_err(|error| error.to_string())?;
        settings
    };

    resolve_loop_agent_command_from_settings(&settings, replica_dir, persisted_agent)
}

/// #2176 - the whole Loop wake agent decision, kept free of transport so it
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
            "[loops] Replica '{}' is pinned to coding agent '{}', which is not configured; \
             refusing to wake with a different agent",
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
        "[loops] No pinned coding agent for '{}': tooling.currentCodingAgent, \
         the persisted session agentId and tooling.lastCodingAgent are all absent \
         or not configured",
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
    crate::config::agent_config::read_agent_local_config(replica_dir)?
        .tooling
        .last_coding_agent
}

fn path_compare_key(path: &Path) -> String {
    let resolved: PathBuf =
        loop_files::canonical_loop_root(path).unwrap_or_else(|_| path.to_path_buf());
    let value = crate::path_utils::path_to_string_without_windows_verbatim_prefix(&resolved);
    let value = value.replace('\\', "/").trim_end_matches('/').to_string();
    if cfg!(windows) {
        value.to_lowercase()
    } else {
        value
    }
}

#[cfg(test)]
mod tests;
