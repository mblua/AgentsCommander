//! #3516 - the preflight test: runs the SAVED chain on a stopped agent folder without a CLI
//! or a session. Describe, start, snapshot and cancel; the UI polls snapshots. Tauri-free.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    chain_passed, destination_key, refusal_message, run_preflight_chain, HoldRefusal,
    PreflightChainEnv, PreflightStageReport, PreflightStageStatus, PreflightState, TestHold,
};
use crate::commands::session::create::{
    build_configured_agent_spawn_for_cwd, resolve_create_session_cwd,
};
use crate::config::agent_command::{PreflightStage, PreflightStageSource};
use crate::config::settings::{AppSettings, SettingsState};
use crate::pty::backend::{ResolvedAgentHostShell, SessionBackendKind};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

const MAX_STORED_RUNS: usize = 16;
const UNKNOWN_RUN: &str = "Unknown preflight test run.";
const STOP_FIRST: &str = "Stop this agent before testing its preflights.";
const NOT_FOUND: &str = "Coding Agent not found.";

/// The saved inputs of one agent's chain: first 16 hex chars of their SHA-256.
pub(crate) fn preflight_revision(cfg: &AppSettings, agent_id: &str) -> String {
    let agent_row = cfg.agents.iter().find(|agent| agent.id == agent_id);
    let inputs = (
        agent_row,
        cfg.coding_agent_profiles.profiles_by_agent.get(agent_id),
        &cfg.default_shell,
        &cfg.default_shell_args,
    );
    let bytes = serde_json::to_vec(&inputs).unwrap_or_default();
    let digest = format!("{:x}", Sha256::digest(&bytes));
    digest[..16].to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreflightStageSummary {
    pub source: PreflightStageSource,
    pub label: String,
    pub command_text: String,
    pub timeout_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreflightTargetDescription {
    pub agent_id: String,
    pub replica_path: String,
    pub cwd: String,
    pub requested_profile: String,
    pub effective_profile: String,
    pub fallback_applied: bool,
    pub stopped: bool,
    pub stages: Vec<PreflightStageSummary>,
    pub settings_revision: String,
    pub active_run_id: Option<String>,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PreflightTestPhase {
    Preparing,
    Running,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PreflightTestOutcome {
    Passed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreflightTestSnapshot {
    pub run_id: String,
    pub agent_id: String,
    pub replica_path: String,
    pub effective_profile: String,
    pub settings_revision: String,
    pub phase: PreflightTestPhase,
    pub outcome: Option<PreflightTestOutcome>,
    pub message: Option<String>,
    pub stages: Vec<PreflightStageReport>,
}

pub(crate) struct PreflightTestDeps {
    pub settings: SettingsState,
    pub session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pub pty_mgr: Arc<std::sync::Mutex<PtyManager>>,
    pub state: Arc<PreflightState>,
}

pub(crate) type PreflightTestTask =
    std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;

struct StoredRun {
    snapshot: PreflightTestSnapshot,
    cancel: tokio::sync::watch::Sender<bool>,
    key: String,
}

/// The stored test runs, oldest first.
#[derive(Default)]
pub(crate) struct PreflightRuns {
    runs: HashMap<String, StoredRun>,
    order: Vec<String>,
}

impl PreflightRuns {
    fn insert(&mut self, run_id: String, run: StoredRun) {
        self.runs.insert(run_id.clone(), run);
        self.order.push(run_id);
        while self.runs.len() > MAX_STORED_RUNS {
            let finished = self.order.iter().position(|id| {
                self.runs
                    .get(id)
                    .is_some_and(|run| run.snapshot.phase == PreflightTestPhase::Finished)
            });
            let Some(index) = finished else { break };
            let id = self.order.remove(index);
            self.runs.remove(&id);
        }
    }

    fn active_for(&self, key: &str) -> Option<String> {
        self.order.iter().find_map(|id| {
            let run = self.runs.get(id)?;
            let active = run.key == key && run.snapshot.phase != PreflightTestPhase::Finished;
            active.then(|| id.clone())
        })
    }
}

impl PreflightState {
    fn runs(&self) -> std::sync::MutexGuard<'_, PreflightRuns> {
        self.runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn update_run(&self, run_id: &str, edit: impl FnOnce(&mut PreflightTestSnapshot)) {
        if let Some(run) = self.runs().runs.get_mut(run_id) {
            edit(&mut run.snapshot);
        }
    }
}

/// Everything a start needs from describe, beyond the wire description.
struct Resolution {
    description: PreflightTargetDescription,
    stages: Vec<PreflightStage>,
    env: Option<PreflightChainEnv>,
}

fn unavailable(description: &mut PreflightTargetDescription, reason: impl Into<String>) {
    description.stages.clear();
    description.unavailable_reason = Some(reason.into());
}

fn summary(stage: &PreflightStage) -> PreflightStageSummary {
    PreflightStageSummary {
        source: stage.source,
        label: stage.label.clone(),
        command_text: stage.command_text.clone(),
        timeout_seconds: stage.timeout_seconds,
    }
}

async fn session_at(deps: &PreflightTestDeps, key: &str) -> bool {
    let dirs = deps
        .session_mgr
        .read()
        .await
        .working_dirs_including_pending()
        .await;
    dirs.iter().any(|dir| destination_key(dir) == key)
}

/// Fills `description` from the saved agent; `None` when the target is unavailable.
fn resolve_target(
    cfg: &AppSettings,
    agent_id: &str,
    profile_letter: Option<&str>,
    description: &mut PreflightTargetDescription,
) -> Option<(Vec<PreflightStage>, PreflightChainEnv)> {
    let Some(agent) = cfg.agents.iter().find(|agent| agent.id == agent_id) else {
        unavailable(description, NOT_FOUND);
        return None;
    };
    if agent.backend.kind == SessionBackendKind::ContainerTransport {
        unavailable(description, "Preflights cannot run for container agents.");
        return None;
    }
    let cwd = description.cwd.clone();
    let is_agent = crate::config::teams::verify_pty_input_replica_cwd(Path::new(&cwd)).is_ok()
        || crate::config::root_agent::is_root_agent_path(&cwd);
    if !is_agent {
        unavailable(description, "This folder is not an AgentsCommander agent.");
        return None;
    }
    let spawn = match build_configured_agent_spawn_for_cwd(cfg, agent_id, &cwd, None) {
        Err(e) => {
            let reason = format!("Test unavailable: the launch context could not be prepared. {e}");
            unavailable(description, reason);
            return None;
        }
        Ok(None) => {
            unavailable(description, NOT_FOUND);
            return None;
        }
        Ok(Some(spawn)) => spawn,
    };
    let resolution = &spawn.profile_resolution;
    description.requested_profile = resolution.requested_profile.clone();
    description.effective_profile = resolution.effective_profile.clone();
    description.fallback_applied = resolution.fallback_applied;
    let stages = match &spawn.preflight_stages {
        Err(e) => {
            unavailable(description, e.clone());
            return None;
        }
        Ok(stages) => stages.clone(),
    };
    if let Some(letter) = profile_letter.filter(|l| *l != resolution.effective_profile) {
        let effective = &resolution.effective_profile;
        let reason = format!("This agent uses Profile {effective}, not Profile {letter}.");
        unavailable(description, reason);
        return None;
    }
    description.stages = stages.iter().map(summary).collect();
    let env = PreflightChainEnv {
        backend_kind: SessionBackendKind::LocalProcess,
        cwd: cwd.clone(),
        configured_env: spawn.child_env.clone(),
        env_remove_keys: spawn.env_remove_keys.clone(),
        extra_env: crate::pty::credentials::build_credentials_env(&Uuid::new_v4(), &cwd),
        resolved_agent_host_shell: Some(ResolvedAgentHostShell {
            program: cfg.default_shell.clone(),
            args: cfg.default_shell_args.clone(),
        }),
    };
    Some((stages, env))
}

async fn resolve(
    deps: &PreflightTestDeps,
    agent_id: String,
    profile_letter: Option<String>,
    replica_path: String,
) -> Result<Resolution, String> {
    let cfg = deps
        .settings
        .current_snapshot()
        .await
        .map_err(|e| format!("Settings are unavailable: {e}"))?;
    let cwd = resolve_create_session_cwd(Some(replica_path.clone()));
    let mut description = PreflightTargetDescription {
        settings_revision: preflight_revision(&cfg, &agent_id),
        agent_id,
        replica_path,
        cwd,
        requested_profile: String::new(),
        effective_profile: String::new(),
        fallback_applied: false,
        stopped: false,
        stages: Vec::new(),
        active_run_id: None,
        unavailable_reason: None,
    };
    let resolved = resolve_target(
        &cfg,
        &description.agent_id.clone(),
        profile_letter.as_deref(),
        &mut description,
    );
    let key = destination_key(&description.cwd);
    description.stopped = !session_at(deps, &key).await && deps.state.is_free(&description.cwd);
    description.active_run_id = deps.state.runs().active_for(&key);
    let (stages, env) = match resolved {
        Some((stages, env)) => (stages, Some(env)),
        None => (Vec::new(), None),
    };
    Ok(Resolution {
        description,
        stages,
        env,
    })
}

pub(crate) async fn describe_preflight_target_inner(
    deps: &PreflightTestDeps,
    agent_id: String,
    profile_letter: Option<String>,
    replica_path: String,
) -> Result<PreflightTargetDescription, String> {
    Ok(resolve(deps, agent_id, profile_letter, replica_path)
        .await?
        .description)
}

fn pending_snapshot(
    run_id: &str,
    description: &PreflightTargetDescription,
) -> PreflightTestSnapshot {
    PreflightTestSnapshot {
        run_id: run_id.to_string(),
        agent_id: description.agent_id.clone(),
        replica_path: description.replica_path.clone(),
        effective_profile: description.effective_profile.clone(),
        settings_revision: description.settings_revision.clone(),
        phase: PreflightTestPhase::Preparing,
        outcome: None,
        message: Some("Preparing test context\u{2026}".to_string()),
        stages: Vec::new(),
    }
}

fn take_test_hold(state: &Arc<PreflightState>, cwd: &str) -> Result<TestHold, String> {
    state.try_test(cwd).map_err(|refusal| match refusal {
        HoldRefusal::Test => "A preflight test is already running on this agent.".to_string(),
        HoldRefusal::Launching => refusal_message(refusal).to_string(),
    })
}

/// Validates and registers a run; returns its id and the not-yet-spawned task.
pub(crate) async fn prepare_preflight_test(
    deps: PreflightTestDeps,
    agent_id: String,
    profile_letter: Option<String>,
    replica_path: String,
    settings_revision: String,
) -> Result<(String, PreflightTestTask), String> {
    let resolution = resolve(&deps, agent_id, profile_letter, replica_path).await?;
    let Resolution {
        description,
        stages,
        env,
    } = resolution;
    if description.settings_revision != settings_revision {
        return Err("Save your changes before testing preflights.".to_string());
    }
    if let Some(reason) = description.unavailable_reason.clone() {
        return Err(reason);
    }
    let env = env.ok_or_else(|| NOT_FOUND.to_string())?;
    if stages.is_empty() {
        return Err("No preflight commands configured. Nothing to test.".to_string());
    }
    let key = destination_key(&description.cwd);
    if session_at(&deps, &key).await {
        return Err(STOP_FIRST.to_string());
    }
    let hold = take_test_hold(&deps.state, &description.cwd)?;
    // A launch pre-creates its pending session before its share; this re-check after the
    // hold means either the test sees the session or the launch sees the hold.
    if session_at(&deps, &key).await {
        drop(hold);
        return Err(STOP_FIRST.to_string());
    }
    let run_id = Uuid::new_v4().to_string();
    let mut snapshot = pending_snapshot(&run_id, &description);
    snapshot.stages = stages.iter().map(super::pending_report).collect();
    let (cancel, cancel_rx) = tokio::sync::watch::channel(false);
    deps.state.runs().insert(
        run_id.clone(),
        StoredRun {
            snapshot,
            cancel,
            key,
        },
    );
    let task = run_task(TestRun {
        deps,
        run_id: run_id.clone(),
        hold,
        stages,
        env,
        cancel: cancel_rx,
    });
    Ok((run_id, Box::pin(task)))
}

struct TestRun {
    deps: PreflightTestDeps,
    run_id: String,
    hold: TestHold,
    stages: Vec<PreflightStage>,
    env: PreflightChainEnv,
    cancel: tokio::sync::watch::Receiver<bool>,
}

fn finish(snapshot: &mut PreflightTestSnapshot, outcome: PreflightTestOutcome) {
    let message = match outcome {
        PreflightTestOutcome::Passed => "Test passed. Preflights will run again when you launch.",
        PreflightTestOutcome::Cancelled => "Test cancelled. No passing result.",
        PreflightTestOutcome::Failed => "Test failed. Launch would be blocked.",
    };
    snapshot.phase = PreflightTestPhase::Finished;
    snapshot.outcome = Some(outcome);
    snapshot.message = Some(message.to_string());
}

async fn run_task(run: TestRun) {
    let TestRun {
        deps,
        run_id,
        hold,
        stages,
        env,
        mut cancel,
    } = run;
    let state = &deps.state;
    if *cancel.borrow() {
        state.update_run(&run_id, |s| finish(s, PreflightTestOutcome::Cancelled));
        drop(hold);
        return;
    }
    state.update_run(&run_id, |s| {
        s.phase = PreflightTestPhase::Running;
        if s.message.as_deref() != Some("Cancelling\u{2026}") {
            s.message = None;
        }
    });
    let outcome = run_preflight_chain(&deps.pty_mgr, &stages, &env, &mut cancel, |reports| {
        state.update_run(&run_id, |s| s.stages = reports.to_vec());
    })
    .await;
    let cancelled = outcome
        .reports
        .iter()
        .any(|report| report.status == PreflightStageStatus::Cancelled);
    let result = if chain_passed(&outcome) {
        PreflightTestOutcome::Passed
    } else if cancelled {
        PreflightTestOutcome::Cancelled
    } else {
        PreflightTestOutcome::Failed
    };
    state.update_run(&run_id, |s| {
        s.stages = outcome.reports.clone();
        finish(s, result);
    });
    drop(hold);
}

pub(crate) async fn start_preflight_test_inner(
    deps: PreflightTestDeps,
    agent_id: String,
    profile_letter: Option<String>,
    replica_path: String,
    settings_revision: String,
) -> Result<String, String> {
    let (run_id, task) = prepare_preflight_test(
        deps,
        agent_id,
        profile_letter,
        replica_path,
        settings_revision,
    )
    .await?;
    tokio::spawn(task);
    Ok(run_id)
}

pub(crate) fn get_preflight_test_inner(
    state: &PreflightState,
    run_id: &str,
) -> Result<PreflightTestSnapshot, String> {
    let runs = state.runs();
    let run = runs
        .runs
        .get(run_id)
        .ok_or_else(|| UNKNOWN_RUN.to_string())?;
    Ok(run.snapshot.clone())
}

pub(crate) fn cancel_preflight_test_inner(
    state: &PreflightState,
    run_id: &str,
) -> Result<PreflightTestSnapshot, String> {
    let mut runs = state.runs();
    let run = runs
        .runs
        .get_mut(run_id)
        .ok_or_else(|| UNKNOWN_RUN.to_string())?;
    if run.snapshot.phase != PreflightTestPhase::Finished {
        let _ = run.cancel.send(true);
        run.snapshot.message = Some("Cancelling\u{2026}".to_string());
    }
    Ok(run.snapshot.clone())
}

#[cfg(test)]
mod tests;
