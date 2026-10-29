use super::updater::{
    output_tail, TargetProcessOwner, TargetSettlement, TargetWait, UpdaterStepOutcome,
};
#[cfg(test)]
use super::updater::{with_target_settlement_hook, TARGET_SETTLEMENT_HOOK};
use super::{
    emit_all, AgentUpdateTasks, ManagedBroadcastEvents, OUTPUT_TAIL_BYTES, UPDATE_STEP_TIMEOUT,
};
use crate::config::agent_path::effective_search_path;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// #2736 - resolved install commands with a running install task. Keyed by the
/// command string, so two catalog keys sharing one command never run it twice.
pub struct AgentInstallState {
    commands: Mutex<HashSet<String>>,
}

impl AgentInstallState {
    pub(crate) fn new() -> Self {
        Self {
            commands: Mutex::new(HashSet::new()),
        }
    }
}

#[cfg(test)]
pub(crate) fn contains_install_command(owner: &AgentInstallState, command: &str) -> bool {
    owner
        .commands
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(command)
}

/// #2736 - RAII entry in `AgentInstallState`; dropping it releases the command.
pub(crate) struct InstallInFlight {
    command: String,
    owner: Arc<AgentInstallState>,
}

impl InstallInFlight {
    pub(crate) fn acquire(owner: Arc<AgentInstallState>, command: &str) -> Result<Self, String> {
        let mut in_flight = owner.commands.lock().unwrap_or_else(|e| e.into_inner());
        if !in_flight.insert(command.to_string()) {
            return Err("an install is already running for that command".to_string());
        }
        Ok(Self {
            command: command.to_string(),
            owner: owner.clone(),
        })
    }
}

impl Drop for InstallInFlight {
    fn drop(&mut self) {
        self.owner
            .commands
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.command);
    }
}

/// #2736 - run ONE install command for a coding agent with no visible window.
/// Reuses the updater step primitives; deliberately does NOT touch
/// `AgentUpdateGate`, the `agent_update_*` events, or the update overlay, so a
/// first-run install stays silent.
pub(crate) fn start_agent_install<E>(
    app: &E,
    owner: Arc<AgentInstallState>,
    key: String,
    command: String,
    cwd: PathBuf,
) -> Result<(), String>
where
    E: ManagedBroadcastEvents + AgentUpdateTasks + Clone + 'static,
{
    start_agent_install_with_timeout(app, owner, key, command, cwd, UPDATE_STEP_TIMEOUT)
}

/// #2736 - `start_agent_install` with the step timeout injected. Production
/// always passes `UPDATE_STEP_TIMEOUT`; only tests pass a shorter one.
pub(crate) fn start_agent_install_with_timeout<E>(
    app: &E,
    owner: Arc<AgentInstallState>,
    key: String,
    command: String,
    cwd: PathBuf,
    timeout: Duration,
) -> Result<(), String>
where
    E: ManagedBroadcastEvents + AgentUpdateTasks + Clone + 'static,
{
    let in_flight = InstallInFlight::acquire(owner, &command)?;
    let app = app.clone();
    let spawner = app.clone();
    let task = async move {
        log::info!("[agent-install] running '{command}' for {key}");
        let run = run_agent_install(&app, &key, &command, &cwd, timeout).await;
        drop(in_flight);
        emit_all(
            &app,
            "coding_agent_install_finished",
            install_finished_payload(&key, &command, &run),
        );
    };
    // Test-only: carry a caller's settlement hook into the detached task, so a
    // test can inject a cleanup defect and still observe the emitted frame.
    #[cfg(test)]
    let task = {
        let hook = TARGET_SETTLEMENT_HOOK
            .try_with(|hook| hook.borrow_mut().take())
            .ok()
            .flatten();
        async move {
            match hook {
                Some(hook) => with_target_settlement_hook(hook, task).await,
                None => task.await,
            }
        }
    };
    spawner.spawn_update_task(Box::pin(task));
    Ok(())
}

/// #2736 - the outcome of one install run. `ok` is true only for a successful
/// exit with a clean process-tree settlement.
pub(crate) struct AgentInstallRun {
    pub(crate) ok: bool,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    /// The child exit code when a status was actually observed, else `None`
    /// (timeout, spawn failure, wait error, or a unix signal, where
    /// `ExitStatus::code()` is itself `None`).
    pub(crate) exit_code: Option<i32>,
    /// The single-line outcome reason written to the log: `ok`, `exit code N`,
    /// `timed out after 300s (killed)`, a spawn error, a containment failure,
    /// or the joined cleanup defects.
    pub(crate) detail: String,
}

/// #2736 - the AWAITABLE body. This is the test seam: `start_agent_install`
/// takes the in-flight guard, spawns a detached task and returns; the task
/// awaits this, then releases the guard and emits. Tests await it directly and
/// read the outcome instead of racing a detached task.
pub(crate) async fn run_agent_install(
    _app: &impl ManagedBroadcastEvents,
    key: &str,
    command: &str,
    cwd: &Path,
    timeout: Duration,
) -> AgentInstallRun {
    let run = match TargetProcessOwner::spawn_step(command, cwd, &effective_search_path()).await {
        Ok(mut owner) => {
            // This feature has no cancel verb: the sender is never set.
            let (_cancel_tx, mut cancel) = tokio::sync::watch::channel(false);
            let waited = owner.wait(timeout, &mut cancel).await;
            let settlement = owner
                .settle_observing_cancellation(&waited, &mut cancel, || {})
                .await;
            install_run_from_settlement(waited, settlement, timeout)
        }
        Err(outcome) => install_run_from_spawn_failure(outcome),
    };
    log_agent_install_run(key, command, &run);
    run
}

pub(crate) fn install_run_from_spawn_failure(outcome: UpdaterStepOutcome) -> AgentInstallRun {
    let detail = match outcome {
        UpdaterStepOutcome::Failed(error) => error,
        UpdaterStepOutcome::ContainmentFailed => "containment failed".to_string(),
        UpdaterStepOutcome::CleanupFailed(detail) => format!("launch cleanup defective: {detail}"),
        UpdaterStepOutcome::Succeeded | UpdaterStepOutcome::Cancelled => {
            "unexpected spawn outcome".to_string()
        }
    };
    AgentInstallRun {
        ok: false,
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit_code: None,
        detail,
    }
}

/// #2736 - section 2b of the plan. Defects are checked FIRST, as the updater
/// does, so a zero exit with a defective cleanup is never a success.
pub(crate) fn install_run_from_settlement(
    waited: TargetWait,
    settlement: TargetSettlement,
    timeout: Duration,
) -> AgentInstallRun {
    let TargetSettlement {
        stdout,
        stderr,
        defects,
    } = settlement;
    let (ok, exit_code, detail) = if !defects.is_empty() {
        // A status observed before the defective cleanup is still reported.
        let observed = match &waited {
            TargetWait::Exited(Ok(status)) => status.code(),
            _ => None,
        };
        (
            false,
            observed,
            format!("process cleanup defective: {}", defects.join("; ")),
        )
    } else {
        match waited {
            TargetWait::TimedOut => (
                false,
                None,
                format!("timed out after {}s (killed)", timeout.as_secs()),
            ),
            TargetWait::Exited(Err(error)) => (false, None, format!("wait error: {error}")),
            TargetWait::Exited(Ok(status)) if status.success() => {
                (true, status.code(), "ok".to_string())
            }
            TargetWait::Exited(Ok(status)) => (
                false,
                status.code(),
                format!("exit code {}", status.code().unwrap_or(-1)),
            ),
            TargetWait::Cancelled => (false, None, "unexpected cancellation".to_string()),
        }
    };
    AgentInstallRun {
        ok,
        stdout,
        stderr,
        exit_code,
        detail,
    }
}

/// #2736 - the COMPLETE captured output goes to the log, never `output_tail`.
pub(crate) fn log_agent_install_run(key: &str, command: &str, run: &AgentInstallRun) {
    for (level, line) in agent_install_log_lines(key, command, run) {
        log::log!(level, "{line}");
    }
}

/// #2736 - the exact lines `log_agent_install_run` writes, pure so tests can
/// read them without installing a process-wide logger.
pub(crate) fn agent_install_log_lines(
    key: &str,
    command: &str,
    run: &AgentInstallRun,
) -> Vec<(log::Level, String)> {
    let mut lines = vec![(
        log::Level::Info,
        format!(
            "[agent-install] stdout for {key} ({command}): {}",
            String::from_utf8_lossy(&run.stdout)
        ),
    )];
    if !run.stderr.is_empty() {
        lines.push((
            log::Level::Info,
            format!(
                "[agent-install] stderr for {key} ({command}): {}",
                String::from_utf8_lossy(&run.stderr)
            ),
        ));
    }
    lines.push(if run.ok {
        (
            log::Level::Info,
            format!("[agent-install] {key} ({command}): {}", run.detail),
        )
    } else {
        (
            log::Level::Warn,
            format!("[agent-install] {key} ({command}) FAILED: {}", run.detail),
        )
    });
    lines
}

/// #2736 - one stream for the event: the last `OUTPUT_TAIL_BYTES`, behind one
/// marker line when the stream was cut. The log keeps the full output.
pub(crate) fn event_output(bytes: &[u8]) -> String {
    let tail = output_tail(bytes);
    if bytes.len() > OUTPUT_TAIL_BYTES {
        format!(
            "[... truncated; showing the last {OUTPUT_TAIL_BYTES} bytes. The full output is in \
            the app log.]\n{tail}"
        )
    } else {
        tail
    }
}

/// #2736 - the ONLY builder of the `coding_agent_install_finished` frame. A
/// successful run carries no output.
pub(crate) fn install_finished_payload(key: &str, command: &str, run: &AgentInstallRun) -> Value {
    let (stdout, stderr) = if run.ok {
        (String::new(), String::new())
    } else {
        (event_output(&run.stdout), event_output(&run.stderr))
    };
    json!({
        "key": key,
        "command": command,
        "ok": run.ok,
        "detail": run.detail,
        "exitCode": run.exit_code,
        "stdout": stdout,
        "stderr": stderr,
    })
}
