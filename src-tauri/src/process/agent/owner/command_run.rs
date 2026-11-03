//! #3516 - one contained, finite, bounded-output preflight process. Reuses the version-probe
//! owner, so the run returns only after the native proof that the process tree is gone.

use std::process::Stdio;
use std::time::{Duration, Instant};

use super::{CancellableProbeOutcome, ProbeOutcome, ProbeProcessOwner, ProbeWait};

const CONTAINMENT_PROBE_TEXT: &str = "Version probe process-tree containment unavailable.";

/// What to run: the exact program, argv (or Windows command line), cwd and complete env.
pub(crate) struct ContainedRunSpec {
    pub program: String,
    pub args: Vec<String>,
    /// Some on Windows: the exact encoded arg line, passed raw instead of `args`.
    pub windows_raw_args: Option<String>,
    pub cwd: String,
    /// The complete child env; nothing is inherited.
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
}

/// The primary end of a run. Cleanup trouble never replaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContainedRunEnd {
    Exited(i32),
    Signaled,
    SpawnFailed(String),
    TimedOut,
    Cancelled,
}

pub(crate) struct ContainedRun {
    pub end: ContainedRunEnd,
    /// Some when the finished settlement recorded defects.
    pub cleanup_defect: Option<String>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration: Duration,
}

fn preflight_text(detail: String) -> String {
    if detail == CONTAINMENT_PROBE_TEXT {
        "process-tree containment is unavailable".to_string()
    } else {
        detail
    }
}

fn command_for(spec: &ContainedRunSpec) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&spec.program);
    match spec.windows_raw_args.as_deref() {
        #[cfg(windows)]
        Some(line) => {
            std::os::windows::process::CommandExt::raw_arg(command.as_std_mut(), line);
        }
        _ => {
            command.args(&spec.args);
        }
    }
    command.current_dir(&spec.cwd);
    command.env_clear();
    command.envs(spec.env.iter().map(|(k, v)| (k, v)));
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    command
}

fn start_failure(start: Instant, end: ContainedRunEnd, defect: Option<String>) -> ContainedRun {
    ContainedRun {
        end,
        cleanup_defect: defect,
        stdout: Vec::new(),
        stderr: Vec::new(),
        duration: start.elapsed(),
    }
}

/// Runs one contained process to its end. Returns only after settlement proved the tree
/// empty; a cleanup that cannot finish keeps this future pending.
pub(crate) async fn run_contained(
    spec: ContainedRunSpec,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
) -> ContainedRun {
    let start = Instant::now();
    let mut command = command_for(&spec);
    let mut owner = match ProbeProcessOwner::spawn(&mut command).await {
        Ok(owner) => owner,
        Err(outcome) => {
            let (end, defect, detail) = match outcome {
                CancellableProbeOutcome::Completed(ProbeOutcome::Failed(detail))
                | CancellableProbeOutcome::Completed(ProbeOutcome::Version(detail)) => {
                    let text = preflight_text(detail);
                    (ContainedRunEnd::SpawnFailed(text.clone()), None, text)
                }
                CancellableProbeOutcome::CleanupFailed(detail) => (
                    ContainedRunEnd::SpawnFailed("process-tree cleanup failed during start".into()),
                    Some(detail.clone()),
                    detail,
                ),
                CancellableProbeOutcome::Cancelled => {
                    (ContainedRunEnd::Cancelled, None, "cancelled".to_string())
                }
            };
            log::warn!("[preflight] contained start failed: {detail}");
            return start_failure(start, end, defect);
        }
    };
    let waited = owner.wait(spec.timeout, cancel).await;
    let (settlement, cancellation_observed) = owner
        .settle_observing_cancellation(&waited, cancel, || {})
        .await;
    drop(owner);
    let end = if cancellation_observed {
        ContainedRunEnd::Cancelled
    } else {
        match waited {
            ProbeWait::Cancelled => ContainedRunEnd::Cancelled,
            ProbeWait::TimedOut => ContainedRunEnd::TimedOut,
            ProbeWait::Exited(Err(error)) => {
                ContainedRunEnd::SpawnFailed(format!("wait failed: {error}"))
            }
            ProbeWait::Exited(Ok(status)) => status
                .code()
                .map(ContainedRunEnd::Exited)
                .unwrap_or(ContainedRunEnd::Signaled),
        }
    };
    let cleanup_defect = (!settlement.defects.is_empty()).then(|| settlement.defects.join("; "));
    ContainedRun {
        end,
        cleanup_defect,
        stdout: settlement.stdout,
        stderr: settlement.stderr,
        duration: start.elapsed(),
    }
}

#[cfg(test)]
mod tests;
