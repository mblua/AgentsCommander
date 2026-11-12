//! #3516 - the launch gate: folder holds (launch share vs. preflight test), the preflight
//! chain run before a CLI spawn, its user-facing messages and the strict seed rule.
//! Tauri-free; the create path wires it in `create/core/backend.rs`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::agent_command::{PreflightStage, PreflightStageSource};
use crate::config::config_seed::{ConfigSeedReport, ConfigSeedSkipReason};
use crate::process::agent::{
    run_contained, sanitize_detail, ContainedRunEnd, ContainedRunSpec, PROBE_OUTPUT_CAP,
};
use crate::pty::backend::{PreflightLaunchRequest, ResolvedAgentHostShell, SessionBackendKind};
use crate::pty::manager::PtyManager;

const GATE_SUFFIX: &str = "The CLI will not start because the gate failed.";

pub(crate) const CONTAINER_REFUSAL: &str = "Preflights cannot run for container agents. \
     Remove the preflight commands or switch this Coding Agent to the local backend. \
     The CLI will not start because the gate failed.";
pub(crate) const GATE_UNAVAILABLE: &str =
    "The preflight gate is unavailable. The CLI will not start because the gate failed.";

/// Per-folder holds: launches share a folder; a preflight test (P04) holds it alone.
#[derive(Default)]
pub(crate) struct PreflightState {
    holds: Mutex<HashMap<String, Hold>>,
    runs: Mutex<test_run::PreflightRuns>,
}

#[derive(Default)]
struct Hold {
    launches: usize,
    test: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoldRefusal {
    Test,
    Launching,
}

/// One launch's shared hold, released on drop.
pub(crate) struct LaunchShare {
    state: Arc<PreflightState>,
    key: String,
}

/// A preflight test's exclusive hold, released on drop.
pub(crate) struct TestHold {
    state: Arc<PreflightState>,
    key: String,
}

impl PreflightState {
    fn holds(&self) -> std::sync::MutexGuard<'_, HashMap<String, Hold>> {
        self.holds
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn try_share(self: &Arc<Self>, cwd: &str) -> Result<LaunchShare, HoldRefusal> {
        let key = destination_key(cwd);
        let mut holds = self.holds();
        let hold = holds.entry(key.clone()).or_default();
        if hold.test {
            return Err(HoldRefusal::Test);
        }
        hold.launches += 1;
        Ok(LaunchShare {
            state: Arc::clone(self),
            key,
        })
    }

    /// Takes the folder alone for a preflight test; never waits.
    pub(crate) fn try_test(self: &Arc<Self>, cwd: &str) -> Result<TestHold, HoldRefusal> {
        let key = destination_key(cwd);
        let mut holds = self.holds();
        let hold = holds.entry(key.clone()).or_default();
        if hold.test {
            return Err(HoldRefusal::Test);
        }
        if hold.launches > 0 {
            return Err(HoldRefusal::Launching);
        }
        hold.test = true;
        Ok(TestHold {
            state: Arc::clone(self),
            key,
        })
    }

    /// No launch share and no test hold on the folder of `cwd`.
    pub(crate) fn is_free(&self, cwd: &str) -> bool {
        !self.holds().contains_key(&destination_key(cwd))
    }

    /// The number of launch shares on the folder of `cwd` (0 when it has no entry).
    #[cfg(test)]
    pub(crate) fn launch_count(&self, cwd: &str) -> usize {
        self.holds()
            .get(&destination_key(cwd))
            .map_or(0, |hold| hold.launches)
    }

    fn release_launch(&self, key: &str) {
        let mut holds = self.holds();
        if let Some(hold) = holds.get_mut(key) {
            hold.launches = hold.launches.saturating_sub(1);
            if hold.launches == 0 && !hold.test {
                holds.remove(key);
            }
        }
    }
}

impl Drop for TestHold {
    fn drop(&mut self) {
        let mut holds = self.state.holds();
        if let Some(hold) = holds.get_mut(&self.key) {
            hold.test = false;
            if hold.launches == 0 {
                holds.remove(&self.key);
            }
        }
    }
}

impl Drop for LaunchShare {
    fn drop(&mut self) {
        self.state.release_launch(&self.key);
    }
}

/// One folder identity: no verbatim prefix, `/` separators, no trailing `/`; on Windows
/// ASCII-lowercased.
pub(crate) fn destination_key(cwd: &str) -> String {
    let normalized = crate::path_utils::normalize_windows_verbatim_path(cwd).replace('\\', "/");
    let trimmed = normalized.trim_end_matches('/');
    if cfg!(windows) {
        trimmed.to_ascii_lowercase()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PreflightStageStatus {
    Pending,
    Running,
    Passed,
    Failed,
    TimedOut,
    SpawnFailed,
    Signaled,
    Cancelled,
    NotRun,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreflightStageReport {
    pub source: PreflightStageSource,
    pub label: String,
    pub effective_profile: String,
    pub command_text: String,
    pub timeout_seconds: u32,
    pub status: PreflightStageStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u64>,
    pub stdout: String,
    pub stderr: String,
    /// "May be truncated": output of exactly `PROBE_OUTPUT_CAP` bytes is flagged too.
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub detail: Option<String>,
    pub cleanup_defect: Option<String>,
}

/// The CLI's final local env inputs and cwd, shared by every stage.
pub(crate) struct PreflightChainEnv {
    pub backend_kind: SessionBackendKind,
    pub cwd: String,
    pub configured_env: Vec<(String, String)>,
    pub env_remove_keys: Vec<String>,
    pub extra_env: Vec<(String, String)>,
    pub resolved_agent_host_shell: Option<ResolvedAgentHostShell>,
}

pub(crate) struct PreflightChainOutcome {
    pub reports: Vec<PreflightStageReport>,
}

fn pending_report(stage: &PreflightStage) -> PreflightStageReport {
    PreflightStageReport {
        source: stage.source,
        label: stage.label.clone(),
        effective_profile: stage.effective_profile.clone(),
        command_text: stage.command_text.clone(),
        timeout_seconds: stage.timeout_seconds,
        status: PreflightStageStatus::Pending,
        exit_code: None,
        duration_ms: None,
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        detail: None,
        cleanup_defect: None,
    }
}

fn launch_request(stage: &PreflightStage, env: &PreflightChainEnv) -> PreflightLaunchRequest {
    PreflightLaunchRequest {
        program: stage.program.clone(),
        args: stage.args.clone(),
        cwd: env.cwd.clone(),
        configured_env: env.configured_env.clone(),
        env_remove_keys: env.env_remove_keys.clone(),
        extra_env: env.extra_env.clone(),
        resolved_agent_host_shell: env.resolved_agent_host_shell.clone(),
    }
}

/// Runs one stage to its end and fills its report.
async fn run_stage(
    pty_mgr: &Arc<Mutex<PtyManager>>,
    stage: &PreflightStage,
    env: &PreflightChainEnv,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    report: &mut PreflightStageReport,
) {
    let backend = pty_mgr
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .backend_for_kind(env.backend_kind);
    let launch = match backend.prepare_preflight_launch(launch_request(stage, env)) {
        Ok(launch) => launch,
        Err(error) => {
            report.status = PreflightStageStatus::SpawnFailed;
            report.detail = Some(error.to_string());
            return;
        }
    };
    let run = run_contained(
        ContainedRunSpec {
            program: launch.program,
            args: launch.args,
            windows_raw_args: launch.windows_raw_args,
            cwd: launch.cwd,
            env: launch.env,
            timeout: Duration::from_secs(u64::from(stage.timeout_seconds)),
        },
        cancel,
    )
    .await;
    report.status = match run.end {
        ContainedRunEnd::Exited(0) => PreflightStageStatus::Passed,
        ContainedRunEnd::Exited(code) => {
            report.exit_code = Some(code);
            PreflightStageStatus::Failed
        }
        ContainedRunEnd::TimedOut => PreflightStageStatus::TimedOut,
        ContainedRunEnd::Signaled => PreflightStageStatus::Signaled,
        ContainedRunEnd::Cancelled => PreflightStageStatus::Cancelled,
        ContainedRunEnd::SpawnFailed(detail) => {
            report.detail = Some(detail);
            PreflightStageStatus::SpawnFailed
        }
    };
    report.cleanup_defect = run.cleanup_defect;
    report.duration_ms = Some(u64::try_from(run.duration.as_millis()).unwrap_or(u64::MAX));
    report.stdout_truncated = run.stdout.len() >= PROBE_OUTPUT_CAP;
    report.stderr_truncated = run.stderr.len() >= PROBE_OUTPUT_CAP;
    report.stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    report.stderr = String::from_utf8_lossy(&run.stderr).into_owned();
}

fn report_passed(report: &PreflightStageReport) -> bool {
    report.status == PreflightStageStatus::Passed && report.cleanup_defect.is_none()
}

/// Runs the stages in order, each in the CLI's env and cwd, stopping at the first stage that
/// did not pass. Returns only after every started stage's process tree is proven gone.
pub(crate) async fn run_preflight_chain<F: FnMut(&[PreflightStageReport])>(
    pty_mgr: &Arc<Mutex<PtyManager>>,
    stages: &[PreflightStage],
    env: &PreflightChainEnv,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    mut on_progress: F,
) -> PreflightChainOutcome {
    let mut reports: Vec<PreflightStageReport> = stages.iter().map(pending_report).collect();
    for (index, stage) in stages.iter().enumerate() {
        reports[index].status = PreflightStageStatus::Running;
        on_progress(&reports);
        run_stage(pty_mgr, stage, env, cancel, &mut reports[index]).await;
        on_progress(&reports);
        if !report_passed(&reports[index]) {
            for later in &mut reports[index + 1..] {
                later.status = PreflightStageStatus::NotRun;
            }
            on_progress(&reports);
            break;
        }
    }
    PreflightChainOutcome { reports }
}

/// Result of a revalidated chain: the launch was superseded, or the chain ran.
pub(crate) enum GatedChain {
    Superseded,
    Ran(PreflightChainOutcome),
}

/// Awaits `revalidate` first; only a current launch (`Ok(true)`) prepares and runs stages.
pub(crate) async fn run_chain_if_current(
    revalidate: impl std::future::Future<Output = Result<bool, String>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    stages: &[PreflightStage],
    env: &PreflightChainEnv,
) -> Result<GatedChain, String> {
    if !revalidate.await? {
        return Ok(GatedChain::Superseded);
    }
    let (_cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    let outcome = run_preflight_chain(pty_mgr, stages, env, &mut cancel_rx, |_| {}).await;
    Ok(GatedChain::Ran(outcome))
}

/// Every stage passed and no cleanup reported a defect.
pub(crate) fn chain_passed(outcome: &PreflightChainOutcome) -> bool {
    outcome.reports.iter().all(report_passed)
}

fn without_trailing_dots(text: &str) -> &str {
    text.trim_end_matches('.')
}

fn output_part(report: &PreflightStageReport) -> String {
    let stderr = sanitize_detail(report.stderr.as_bytes());
    let line = if stderr.is_empty() {
        sanitize_detail(report.stdout.as_bytes())
    } else {
        stderr
    };
    if line.is_empty() {
        return String::new();
    }
    let line = match line.strip_suffix("...") {
        Some(cut) => format!("{cut}\u{2026}"),
        None => line,
    };
    format!(" Output: \"{line}\".")
}

/// The toast text for the first stage that did not pass.
pub(crate) fn launch_failure_message(report: &PreflightStageReport) -> String {
    let detail = without_trailing_dots(report.detail.as_deref().unwrap_or_default());
    let defect = report.cleanup_defect.as_deref().map(without_trailing_dots);
    let cause = match report.status {
        PreflightStageStatus::Failed => {
            format!(
                "failed with exit code {}",
                report.exit_code.unwrap_or_default()
            )
        }
        PreflightStageStatus::TimedOut => {
            format!("exceeded its timeout of {} seconds", report.timeout_seconds)
        }
        PreflightStageStatus::SpawnFailed => format!("could not be started: {detail}"),
        PreflightStageStatus::Signaled => "was terminated by a signal".to_string(),
        PreflightStageStatus::Cancelled => "was cancelled".to_string(),
        PreflightStageStatus::Passed => {
            format!("cleanup reported an error: {}", defect.unwrap_or_default())
        }
        PreflightStageStatus::Pending
        | PreflightStageStatus::Running
        | PreflightStageStatus::NotRun => "did not run".to_string(),
    };
    let mut message = format!("{} preflight {cause}.", report.label);
    if let Some(defect) = defect.filter(|_| report.status != PreflightStageStatus::Passed) {
        message.push_str(&format!(" Cleanup reported an error: {defect}."));
    }
    message.push_str(&output_part(report));
    message.push(' ');
    message.push_str(GATE_SUFFIX);
    message
}

/// The toast text when the configured stages could not be resolved.
pub(crate) fn resolution_failure_message(error: &str) -> String {
    format!("{}. {GATE_SUFFIX}", without_trailing_dots(error.trim()))
}

pub(crate) fn refusal_message(refusal: HoldRefusal) -> &'static str {
    match refusal {
        HoldRefusal::Test => {
            "A preflight test is running on this agent. \
             The CLI will not start because the gate failed."
        }
        HoldRefusal::Launching => "This agent is starting. Try again when it is stopped.",
    }
}

/// The strict seed rule, applied only when a preflight chain is configured.
pub(crate) fn seed_gate(report: Option<&ConfigSeedReport>) -> Result<(), String> {
    let reason = match report {
        None | Some(ConfigSeedReport::Published(_)) => return Ok(()),
        Some(ConfigSeedReport::Skipped(skip)) => match skip {
            ConfigSeedSkipReason::NoSource => return Ok(()),
            ConfigSeedSkipReason::InvalidDestination { .. } => {
                "the config seed destination is invalid"
            }
            ConfigSeedSkipReason::StaleReplica { .. } => "the agent replica is stale",
            ConfigSeedSkipReason::DestinationInUse { .. } => {
                "the config seed destination is in use"
            }
            ConfigSeedSkipReason::GateUnavailable { .. } => "the config seed gate is unavailable",
        },
        Some(ConfigSeedReport::Failed(_)) => "the config seed failed",
        Some(ConfigSeedReport::FailedAfterLogicalRemoval(_)) => {
            "the config seed failed after removing the previous configuration"
        }
    };
    Err(format!("Preparation gate failed: {reason}. {GATE_SUFFIX}"))
}

pub(crate) mod test_run;

#[cfg(test)]
mod tests;
