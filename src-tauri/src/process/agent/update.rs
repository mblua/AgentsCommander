//! Native update-step processes; admission and update policy stay with the caller.

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use std::ffi::OsStr;
#[cfg(test)]
use std::future::Future;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

/// Captured output tail kept for logging and install event payloads.
pub(crate) const OUTPUT_TAIL_BYTES: usize = 8 * 1024;
const PROCESS_SETTLEMENT_WINDOW: Duration = Duration::from_secs(10);
const PROCESS_SETTLEMENT_POLL: Duration = Duration::from_millis(25);

/// Last `OUTPUT_TAIL_BYTES` of a captured pipe, lossy-decoded for logging.
pub(crate) fn output_tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(OUTPUT_TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).to_string()
}

pub(crate) enum TargetWait {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    Cancelled,
}

pub(crate) struct TargetSettlement {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) defects: Vec<String>,
}

pub(crate) enum UpdaterStepOutcome {
    Succeeded,
    Failed(String),
    Cancelled,
    CleanupFailed(String),
    /// Windows only: pre-execution containment is a Win32 Job Object primitive, so
    /// `spawn_suspended_contained` is exported only there and this is constructed only on
    /// the `ContainedSpawnError::Containment` arm. On Unix `TargetProcessOwner::spawn` uses
    /// a plain spawn, which can fail but has no containment step to fail, so the variant is
    /// never produced and `dead_code` is correct to say so.
    #[cfg_attr(not(windows), allow(dead_code))]
    ContainmentFailed,
}

pub(crate) struct RecoverableUpdateReader {
    pub(crate) completion: Shared<BoxFuture<'static, Result<Vec<u8>, String>>>,
    pub(crate) abort: tokio::task::AbortHandle,
    pub(crate) abort_requested: bool,
    #[cfg(test)]
    pub(crate) settlement_hook: Option<TargetReaderSettlementHook>,
}

#[cfg(test)]
pub(crate) struct TargetReaderSettlementHook {
    pub(crate) hold_completion: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(crate) before_panic: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) release_panic: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(crate) retry_started: Option<tokio::sync::oneshot::Sender<()>>,
}

pub(crate) struct TargetProcessOwner {
    pub(crate) child: Option<tokio::process::Child>,
    pub(crate) stdout: Option<RecoverableUpdateReader>,
    pub(crate) stderr: Option<RecoverableUpdateReader>,
    pub(crate) defects: Vec<String>,
    #[cfg(windows)]
    pub(crate) job: Option<crate::process::host::job::JobObject>,
    #[cfg(unix)]
    pub(crate) pgid: i32,
    #[cfg(test)]
    pub(crate) settlement_hook: Option<TargetSettlementHook>,
}

#[cfg(test)]
pub(crate) struct TargetSettlementHook {
    pub(crate) before_native_proof: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) release_native_proof: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(crate) settlement_window: Option<Duration>,
    pub(crate) native_observations: std::collections::VecDeque<TargetNativeObservation>,
    pub(crate) panic_after_native_proof_release: bool,
    pub(crate) panic_after_native_query_defect: bool,
    pub(crate) stdout_reader: Option<TargetReaderSettlementHook>,
}

#[cfg(test)]
pub(crate) enum TargetNativeObservation {
    Empty,
    Active,
    Error(&'static str),
}

#[cfg(test)]
tokio::task_local! {
    pub(crate) static TARGET_SETTLEMENT_HOOK: std::cell::RefCell<Option<TargetSettlementHook>>;
}

#[cfg(test)]
pub(crate) async fn with_target_settlement_hook<F>(
    hook: TargetSettlementHook,
    future: F,
) -> F::Output
where
    F: Future,
{
    TARGET_SETTLEMENT_HOOK
        .scope(std::cell::RefCell::new(Some(hook)), future)
        .await
}

impl TargetProcessOwner {
    pub(crate) async fn spawn_step(
        cmd: &str,
        cwd: &Path,
        search_path: &OsStr,
    ) -> Result<Self, UpdaterStepOutcome> {
        let mut command = build_update_step_command(cmd, cwd, search_path);
        Self::spawn_native(&mut command).await
    }

    #[cfg(test)]
    pub(crate) async fn spawn(
        command: &mut tokio::process::Command,
    ) -> Result<Self, UpdaterStepOutcome> {
        Self::spawn_native(command).await
    }

    async fn spawn_native(
        command: &mut tokio::process::Command,
    ) -> Result<Self, UpdaterStepOutcome> {
        #[cfg(windows)]
        let (mut child, job) = match crate::process::host::job::spawn_suspended_contained(command)
            .await
        {
            Ok(pair) => pair,
            Err(crate::process::host::job::ContainedSpawnError::Spawn(error)) => {
                return Err(UpdaterStepOutcome::Failed(error.to_string()))
            }
            Err(crate::process::host::job::ContainedSpawnError::Containment(reason)) => {
                log::warn!(
                    "[agent-update] contained updater launch rejected: process containment \
                    failed at {reason}"
                );
                return Err(UpdaterStepOutcome::ContainmentFailed);
            }
            Err(crate::process::host::job::ContainedSpawnError::Cleanup(detail)) => {
                log::warn!("[agent-update] contained updater launch cleanup defective: {detail}");
                return Err(UpdaterStepOutcome::CleanupFailed(detail));
            }
        };
        #[cfg(not(windows))]
        let mut child = command
            .spawn()
            .map_err(|error| UpdaterStepOutcome::Failed(error.to_string()))?;

        #[cfg(unix)]
        let pgid = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .unwrap_or(0);
        #[cfg(test)]
        let mut settlement_hook = TARGET_SETTLEMENT_HOOK
            .try_with(|hook| hook.borrow_mut().take())
            .ok()
            .flatten();
        #[cfg(test)]
        let stdout_reader_hook = settlement_hook
            .as_mut()
            .and_then(|hook| hook.stdout_reader.take());
        #[cfg(test)]
        let stdout = child
            .stdout
            .take()
            .map(move |reader| spawn_update_reader(reader, stdout_reader_hook));
        #[cfg(test)]
        let stderr = child
            .stderr
            .take()
            .map(|reader| spawn_update_reader(reader, None));
        #[cfg(not(test))]
        let stdout = child.stdout.take().map(spawn_update_reader);
        #[cfg(not(test))]
        let stderr = child.stderr.take().map(spawn_update_reader);
        Ok(Self {
            child: Some(child),
            stdout,
            stderr,
            defects: Vec::new(),
            #[cfg(windows)]
            job: Some(job),
            #[cfg(unix)]
            pgid,
            #[cfg(test)]
            settlement_hook,
        })
    }

    pub(crate) async fn wait(
        &mut self,
        timeout: Duration,
        cancel: &mut tokio::sync::watch::Receiver<bool>,
    ) -> TargetWait {
        let child = self.child.as_mut().expect("target process owner");
        tokio::select! {
            result = child.wait() => TargetWait::Exited(result),
            _ = tokio::time::sleep(timeout) => TargetWait::TimedOut,
            _ = wait_for_target_cancellation(cancel) => TargetWait::Cancelled,
        }
    }

    pub(crate) async fn settle(&mut self, wait: &TargetWait) -> TargetSettlement {
        #[cfg(test)]
        let settlement_window = self
            .settlement_hook
            .as_ref()
            .and_then(|hook| hook.settlement_window)
            .unwrap_or(PROCESS_SETTLEMENT_WINDOW);
        #[cfg(not(test))]
        let settlement_window = PROCESS_SETTLEMENT_WINDOW;
        let mut attempt = crate::process::host::job::SettlementAttempt::new(settlement_window);
        let cleanup_required = !matches!(wait, TargetWait::Exited(Ok(_)));
        self.settle_direct_child(cleanup_required, &mut attempt)
            .await;
        let defects = &mut self.defects;

        #[cfg(windows)]
        if !cleanup_required {
            if let Some(job) = self.job.as_ref() {
                job.terminate();
            }
        }
        #[cfg(unix)]
        if !cleanup_required && self.pgid > 0 {
            // SAFETY: the retained positive group belongs to this step.
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
        }

        let stdout = settle_update_reader(&mut self.stdout, defects, &mut attempt).await;
        let stderr = settle_update_reader(&mut self.stderr, defects, &mut attempt).await;

        #[cfg(test)]
        {
            let (ready, mut release) = self
                .settlement_hook
                .as_mut()
                .map(|hook| {
                    (
                        hook.before_native_proof.take(),
                        hook.release_native_proof.take(),
                    )
                })
                .unwrap_or((None, None));
            if let Some(ready) = ready {
                let _ = ready.send(());
            }
            if let Some(release) = release.as_mut() {
                loop {
                    if attempt.expired() {
                        record_update_attempt_expiry(defects, "native-proof barrier");
                        attempt.restart();
                    }
                    match tokio::time::timeout(attempt.remaining(), &mut *release).await {
                        Ok(_) => break,
                        Err(_) => {
                            record_update_attempt_expiry(defects, "native-proof barrier");
                            attempt.restart();
                        }
                    }
                }
            }
            let panic_after_release = self
                .settlement_hook
                .as_mut()
                .is_some_and(|hook| std::mem::take(&mut hook.panic_after_native_proof_release));
            if panic_after_release {
                panic!("injected updater panic during native settlement");
            }
        }

        self.settle_native_proof(&mut attempt).await;
        #[cfg(windows)]
        drop(self.job.take());
        TargetSettlement {
            stdout,
            stderr,
            defects: self.defects.clone(),
        }
    }

    async fn settle_direct_child(
        &mut self,
        cleanup_required: bool,
        attempt: &mut crate::process::host::job::SettlementAttempt,
    ) {
        let defects = &mut self.defects;

        #[cfg(windows)]
        if cleanup_required {
            if let Some(job) = self.job.as_ref() {
                if let Err(error) = job.terminate_checked() {
                    defects.push(format!("job termination failed: {error}"));
                }
            }
        }
        #[cfg(unix)]
        if cleanup_required && self.pgid > 0 {
            // SAFETY: the group was created for this updater step.
            let result = unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                defects.push(format!(
                    "process-group termination failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        if cleanup_required {
            if let Some(child) = self.child.as_mut() {
                if let Err(error) = child.start_kill() {
                    defects.push(format!("direct child termination failed: {error}"));
                }
            }
        }
        if cleanup_required {
            if let Some(child) = self.child.as_mut() {
                let mut wait_defect_recorded = false;
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) => {}
                        Err(error) => {
                            if !wait_defect_recorded {
                                defects.push(format!("direct child wait failed: {error}"));
                                wait_defect_recorded = true;
                            }
                        }
                    }
                    if attempt.expired() {
                        record_update_attempt_expiry(defects, "direct-child settlement");
                        #[cfg(windows)]
                        if let Some(job) = self.job.as_ref() {
                            let _ = job.terminate_checked();
                        }
                        #[cfg(unix)]
                        if self.pgid > 0 {
                            // SAFETY: this remains the retained group identity.
                            unsafe {
                                libc::kill(-self.pgid, libc::SIGKILL);
                            }
                        }
                        let _ = child.start_kill();
                        attempt.restart();
                    }
                    tokio::time::sleep(PROCESS_SETTLEMENT_POLL.min(attempt.remaining())).await;
                }
            }
        }
        drop(self.child.take());
    }

    async fn settle_native_proof(
        &mut self,
        attempt: &mut crate::process::host::job::SettlementAttempt,
    ) {
        #[cfg(windows)]
        self.settle_job_proof(attempt).await;
        #[cfg(unix)]
        self.settle_process_group_proof(attempt).await;
    }

    #[cfg(windows)]
    async fn settle_job_proof(
        &mut self,
        attempt: &mut crate::process::host::job::SettlementAttempt,
    ) {
        let defects = &mut self.defects;
        if let Some(job) = self.job.as_ref() {
            let mut query_defect_recorded = false;
            loop {
                #[cfg(test)]
                let proof = match self
                    .settlement_hook
                    .as_mut()
                    .and_then(|hook| hook.native_observations.pop_front())
                {
                    Some(observation) => match observation {
                        TargetNativeObservation::Empty => Ok(true),
                        TargetNativeObservation::Active => Ok(false),
                        TargetNativeObservation::Error(detail) => Err(detail.to_string()),
                    },
                    None => job
                        .active_processes()
                        .map(|active| active == 0)
                        .map_err(|error| error.to_string()),
                };
                #[cfg(not(test))]
                let proof = job
                    .active_processes()
                    .map(|active| active == 0)
                    .map_err(|error| error.to_string());
                match proof {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(error) => {
                        if !query_defect_recorded {
                            defects.push(format!("job accounting failed: {error}"));
                            query_defect_recorded = true;
                        }
                        #[cfg(test)]
                        if self.settlement_hook.as_mut().is_some_and(|hook| {
                            std::mem::take(&mut hook.panic_after_native_query_defect)
                        }) {
                            panic!("injected updater panic after native query defect");
                        }
                    }
                }
                if attempt.expired() {
                    record_update_attempt_expiry(defects, "job accounting proof");
                    let _ = job.terminate_checked();
                    attempt.restart();
                }
                tokio::time::sleep(PROCESS_SETTLEMENT_POLL.min(attempt.remaining())).await;
            }
        }
    }

    #[cfg(unix)]
    async fn settle_process_group_proof(
        &mut self,
        attempt: &mut crate::process::host::job::SettlementAttempt,
    ) {
        let defects = &mut self.defects;
        if self.pgid > 0 {
            let mut errno_defect_recorded = false;
            loop {
                #[cfg(test)]
                let proof = match self
                    .settlement_hook
                    .as_mut()
                    .and_then(|hook| hook.native_observations.pop_front())
                {
                    Some(observation) => match observation {
                        TargetNativeObservation::Empty => Ok(true),
                        TargetNativeObservation::Active => Ok(false),
                        TargetNativeObservation::Error(detail) => Err(detail.to_string()),
                    },
                    None => {
                        // SAFETY: signal zero is a read-only group existence query.
                        let result = unsafe { libc::kill(-self.pgid, 0) };
                        if result == -1 {
                            let error = std::io::Error::last_os_error();
                            if error.raw_os_error() == Some(libc::ESRCH) {
                                Ok(true)
                            } else {
                                Err(error.to_string())
                            }
                        } else {
                            Ok(false)
                        }
                    }
                };
                #[cfg(not(test))]
                let proof = {
                    // SAFETY: signal zero is a read-only group existence query.
                    let result = unsafe { libc::kill(-self.pgid, 0) };
                    if result == -1 {
                        let error = std::io::Error::last_os_error();
                        if error.raw_os_error() == Some(libc::ESRCH) {
                            Ok(true)
                        } else {
                            Err(error.to_string())
                        }
                    } else {
                        Ok(false)
                    }
                };
                match proof {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(error) => {
                        if !errno_defect_recorded {
                            defects.push(format!("process-group accounting failed: {error}"));
                            errno_defect_recorded = true;
                        }
                        #[cfg(test)]
                        if self.settlement_hook.as_mut().is_some_and(|hook| {
                            std::mem::take(&mut hook.panic_after_native_query_defect)
                        }) {
                            panic!("injected updater panic after native query defect");
                        }
                    }
                }
                if attempt.expired() {
                    record_update_attempt_expiry(defects, "process-group accounting proof");
                    // SAFETY: this remains the retained group identity.
                    unsafe {
                        libc::kill(-self.pgid, libc::SIGKILL);
                    }
                    attempt.restart();
                }
                tokio::time::sleep(PROCESS_SETTLEMENT_POLL.min(attempt.remaining())).await;
            }
        }
    }

    pub(crate) async fn settle_observing_cancellation<F>(
        &mut self,
        wait: &TargetWait,
        cancel: &mut tokio::sync::watch::Receiver<bool>,
        observed: F,
    ) -> TargetSettlement
    where
        F: FnOnce(),
    {
        if matches!(wait, TargetWait::Cancelled) {
            observed();
            return self.settle(wait).await;
        }
        let settlement = self.settle(wait);
        tokio::pin!(settlement);
        tokio::select! {
            biased;
            _ = wait_for_target_cancellation(cancel) => {
                observed();
                settlement.await
            }
            settled = &mut settlement => settled,
        }
    }
}

pub(crate) fn spawn_update_reader<R>(
    mut reader: R,
    #[cfg(test)] mut settlement_hook: Option<TargetReaderSettlementHook>,
) -> RecoverableUpdateReader
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    #[cfg(test)]
    let hold_completion = settlement_hook
        .as_mut()
        .and_then(|hook| hook.hold_completion.take());
    let reader = tokio::spawn(async move {
        #[cfg(test)]
        if let Some(hold_completion) = hold_completion {
            let _ = hold_completion.await;
        }
        let mut bytes = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut bytes).await;
        bytes
    });
    let abort = reader.abort_handle();
    let completion = async move { reader.await.map_err(|error| error.to_string()) }
        .boxed()
        .shared();
    RecoverableUpdateReader {
        completion,
        abort,
        abort_requested: false,
        #[cfg(test)]
        settlement_hook,
    }
}

pub(crate) fn record_update_attempt_expiry(defects: &mut Vec<String>, stage: &str) {
    let detail = format!("settlement attempt deadline exceeded during {stage}");
    if !defects.iter().any(|defect| defect == &detail) {
        defects.push(detail);
    }
}

pub(crate) async fn settle_update_reader(
    reader: &mut Option<RecoverableUpdateReader>,
    defects: &mut Vec<String>,
    attempt: &mut crate::process::host::job::SettlementAttempt,
) -> Vec<u8> {
    let Some(reader) = reader.as_mut() else {
        return Vec::new();
    };
    let completion = reader.completion.clone();
    #[cfg(test)]
    if let Some(hook) = reader.settlement_hook.as_mut() {
        if let Some(release_panic) = hook.release_panic.take() {
            if let Some(before_panic) = hook.before_panic.take() {
                let _ = before_panic.send(());
            }
            let _ = release_panic.await;
            panic!("injected updater panic during reader settlement");
        }
        if let Some(retry_started) = hook.retry_started.take() {
            let _ = retry_started.send(());
        }
    }
    if reader.abort_requested {
        reader.abort.abort();
        let _ = completion.await;
        return Vec::new();
    }
    if attempt.expired() {
        record_update_attempt_expiry(defects, "reader settlement");
        attempt.restart();
    }
    match tokio::time::timeout(attempt.remaining(), completion.clone()).await {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            let detail = format!("reader join failed: {error}");
            if !defects.iter().any(|defect| defect == &detail) {
                defects.push(detail);
            }
            Vec::new()
        }
        Err(_) => {
            record_update_attempt_expiry(defects, "reader settlement");
            reader.abort_requested = true;
            reader.abort.abort();
            let _ = completion.await;
            attempt.restart();
            Vec::new()
        }
    }
}

pub(crate) async fn wait_for_target_cancellation(cancel: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Execute one command's update sequence IN ORDER, fail-fast: the first failed
/// step stops the chain. Each step is a full shell string run through
/// `cmd.exe /C` (Windows) or `sh -c` (elsewhere), cwd = the target's directory,
/// no stdin, output tail-captured concurrently with the wait (a chatty command
/// must never block on the 64KB pipe buffer). Per-step timeout kills the WHOLE
/// tree (Windows JobObject, Unix process-group kill).
/// #2589 - one updater step's process, with `search_path` as its PATH so a
/// GUI-launched app finds agents installed in the user's bin dirs.
fn build_update_step_command(
    cmd: &str,
    cwd: &Path,
    search_path: &OsStr,
) -> tokio::process::Command {
    let mut command = if cfg!(windows) {
        let mut command = tokio::process::Command::new("cmd.exe");
        command.arg("/C").arg(cmd);
        command
    } else {
        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(cmd);
        command
    };
    command.current_dir(cwd);
    command.env("PATH", search_path);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command
}

#[cfg(test)]
pub(crate) fn build_update_step_command_for_test(
    cmd: &str,
    cwd: &Path,
    search_path: &OsStr,
) -> tokio::process::Command {
    build_update_step_command(cmd, cwd, search_path)
}
