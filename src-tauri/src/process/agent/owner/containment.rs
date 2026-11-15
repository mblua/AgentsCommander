use std::time::Duration;

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;

use super::outcome::{CancellableProbeOutcome, ProbeOutcome};

/// Bytes retained per stream (the rest is drained and dropped).
pub const PROBE_OUTPUT_CAP: usize = 4 * 1024;

/// One cleanup attempt may spend this long proving the native tree empty. A
/// missed window records a defect but does not permit a terminal return; the
/// owned reaper starts another attempt and remains nonterminal until proof.
const ACTIVE_SETTLEMENT_WINDOW: Duration = Duration::from_secs(10);

const ACTIVE_SETTLEMENT_POLL: Duration = Duration::from_millis(25);

struct RecoverableProbeReader {
    completion: Shared<BoxFuture<'static, Result<Vec<u8>, String>>>,
    abort: tokio::task::AbortHandle,
    abort_requested: bool,
    #[cfg(test)]
    settlement_hook: Option<ProbeReaderSettlementHook>,
}

#[cfg(test)]
pub(crate) struct ProbeReaderSettlementHook {
    pub(crate) hold_completion: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(crate) before_panic: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) release_panic: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(crate) retry_started: Option<tokio::sync::oneshot::Sender<()>>,
}

/// Drain a pipe to EOF but retain only the first `PROBE_OUTPUT_CAP` bytes.
fn spawn_capped_reader<R>(
    mut reader: R,
    #[cfg(test)] mut settlement_hook: Option<ProbeReaderSettlementHook>,
) -> RecoverableProbeReader
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
        let mut kept: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match tokio::io::AsyncReadExt::read(&mut reader, &mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if kept.len() < PROBE_OUTPUT_CAP {
                        let room = PROBE_OUTPUT_CAP - kept.len();
                        kept.extend_from_slice(&chunk[..n.min(room)]);
                    }
                }
            }
        }
        kept
    });
    let abort = reader.abort_handle();
    let completion = async move { reader.await.map_err(|error| error.to_string()) }
        .boxed()
        .shared();
    RecoverableProbeReader {
        completion,
        abort,
        abort_requested: false,
        #[cfg(test)]
        settlement_hook,
    }
}

fn record_probe_attempt_expiry(defects: &mut Vec<String>, stage: &str) {
    let detail = format!("settlement attempt deadline exceeded during {stage}");
    if !defects.iter().any(|defect| defect == &detail) {
        defects.push(detail);
    }
}

async fn settle_probe_reader(
    reader: &mut Option<RecoverableProbeReader>,
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
            panic!("injected probe panic during reader settlement");
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
        record_probe_attempt_expiry(defects, "reader settlement");
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
            record_probe_attempt_expiry(defects, "reader settlement");
            reader.abort_requested = true;
            reader.abort.abort();
            let _ = completion.await;
            attempt.restart();
            Vec::new()
        }
    }
}

pub(crate) enum ProbeWait {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    Cancelled,
}

pub(crate) struct ProbeSettlement {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) defects: Vec<String>,
}

#[cfg(test)]
pub(crate) struct ProbeSettlementHook {
    pub(crate) before_native_proof: Option<tokio::sync::oneshot::Sender<()>>,
    pub(crate) release_native_proof: Option<tokio::sync::oneshot::Receiver<()>>,
    pub(crate) settlement_window: Option<Duration>,
    pub(crate) native_observations: std::collections::VecDeque<ProbeNativeObservation>,
    pub(crate) panic_after_native_proof_release: bool,
    pub(crate) panic_after_native_query_defect: bool,
    pub(crate) stdout_reader: Option<ProbeReaderSettlementHook>,
}

#[cfg(test)]
pub(crate) enum ProbeNativeObservation {
    Empty,
    Active,
    Error(&'static str),
}

#[cfg(test)]
impl crate::process::agent::native_proof::NativeProofHooks for ProbeSettlementHook {
    fn next_observation(&mut self) -> Option<Result<bool, String>> {
        self.native_observations
            .pop_front()
            .map(|observation| match observation {
                ProbeNativeObservation::Empty => Ok(true),
                ProbeNativeObservation::Active => Ok(false),
                ProbeNativeObservation::Error(detail) => Err(detail.to_string()),
            })
    }

    fn after_query_defect(&mut self) {
        if std::mem::take(&mut self.panic_after_native_query_defect) {
            panic!("injected probe panic after native query defect");
        }
    }
}

#[cfg(test)]
struct ProbeSettlementHookScope {
    remaining_spawns: usize,
    hook: Option<ProbeSettlementHook>,
}

#[cfg(test)]
tokio::task_local! {
    static PROBE_SETTLEMENT_HOOK: std::cell::RefCell<ProbeSettlementHookScope>;
}

#[cfg(test)]
pub(crate) async fn with_probe_settlement_hook<F>(hook: ProbeSettlementHook, future: F) -> F::Output
where
    F: std::future::Future,
{
    PROBE_SETTLEMENT_HOOK
        .scope(
            std::cell::RefCell::new(ProbeSettlementHookScope {
                remaining_spawns: 0,
                hook: Some(hook),
            }),
            future,
        )
        .await
}

#[cfg(test)]
pub(crate) async fn with_probe_settlement_hook_after_spawns<F>(
    remaining_spawns: usize,
    hook: ProbeSettlementHook,
    future: F,
) -> F::Output
where
    F: std::future::Future,
{
    PROBE_SETTLEMENT_HOOK
        .scope(
            std::cell::RefCell::new(ProbeSettlementHookScope {
                remaining_spawns,
                hook: Some(hook),
            }),
            future,
        )
        .await
}

async fn wait_for_probe_cancellation(cancel: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

pub(crate) struct ProbeProcessOwner {
    child: Option<tokio::process::Child>,
    stdout: Option<RecoverableProbeReader>,
    stderr: Option<RecoverableProbeReader>,
    defects: Vec<String>,
    #[cfg(windows)]
    job: Option<crate::process::host::job::JobObject>,
    #[cfg(unix)]
    pgid: i32,
    #[cfg(test)]
    settlement_hook: Option<ProbeSettlementHook>,
}

impl ProbeProcessOwner {
    pub(crate) async fn spawn(
        command: &mut tokio::process::Command,
    ) -> Result<Self, CancellableProbeOutcome> {
        #[cfg(windows)]
        let (mut child, job) =
            match crate::process::host::job::spawn_suspended_contained(command).await {
                Ok(pair) => pair,
                Err(crate::process::host::job::ContainedSpawnError::Spawn(error)) => {
                    return Err(CancellableProbeOutcome::Completed(ProbeOutcome::Failed(
                        format!("spawn failed: {error}"),
                    )))
                }
                Err(crate::process::host::job::ContainedSpawnError::Containment(reason)) => {
                    log::warn!(
                        target: "agentscommander_lib::process::agent::owner",
                        "[agent-version] contained probe launch rejected: \
                         process containment failed at {reason}"
                    );
                    return Err(CancellableProbeOutcome::Completed(ProbeOutcome::Failed(
                        "Version probe process-tree containment unavailable.".to_string(),
                    )));
                }
                Err(crate::process::host::job::ContainedSpawnError::Cleanup(detail)) => {
                    log::warn!(
                    target: "agentscommander_lib::process::agent::owner",
                    "[agent-version] contained probe launch cleanup defective: {detail}");
                    return Err(CancellableProbeOutcome::CleanupFailed(detail));
                }
            };

        #[cfg(not(windows))]
        let mut child = command.spawn().map_err(|error| {
            CancellableProbeOutcome::Completed(ProbeOutcome::Failed(format!(
                "spawn failed: {error}"
            )))
        })?;

        #[cfg(unix)]
        let pgid = child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .unwrap_or(0);
        #[cfg(test)]
        let mut settlement_hook = PROBE_SETTLEMENT_HOOK
            .try_with(|scope| {
                let mut scope = scope.borrow_mut();
                if scope.remaining_spawns == 0 {
                    scope.hook.take()
                } else {
                    scope.remaining_spawns -= 1;
                    None
                }
            })
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
            .map(move |reader| spawn_capped_reader(reader, stdout_reader_hook));
        #[cfg(test)]
        let stderr = child
            .stderr
            .take()
            .map(|reader| spawn_capped_reader(reader, None));
        #[cfg(not(test))]
        let stdout = child.stdout.take().map(spawn_capped_reader);
        #[cfg(not(test))]
        let stderr = child.stderr.take().map(spawn_capped_reader);
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
    ) -> ProbeWait {
        let child = self.child.as_mut().expect("probe child owner");
        tokio::select! {
            result = child.wait() => ProbeWait::Exited(result),
            () = tokio::time::sleep(timeout) => ProbeWait::TimedOut,
            () = wait_for_probe_cancellation(cancel) => ProbeWait::Cancelled,
        }
    }

    async fn settle_child(
        &mut self,
        cleanup_required: bool,
        attempt: &mut crate::process::host::job::SettlementAttempt,
    ) {
        let defects = &mut self.defects;
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
                        record_probe_attempt_expiry(defects, "direct-child settlement");
                        #[cfg(windows)]
                        if let Some(job) = self.job.as_ref() {
                            let _ = job.terminate_checked();
                        }
                        #[cfg(unix)]
                        if self.pgid > 0 {
                            // SAFETY: self.pgid > 0 makes its negation a representable group
                            // selector. kill accepts SIGKILL and has no caller-memory arguments.
                            unsafe {
                                libc::kill(-self.pgid, libc::SIGKILL);
                            }
                        }
                        let _ = child.start_kill();
                        attempt.restart();
                    }
                    tokio::time::sleep(ACTIVE_SETTLEMENT_POLL.min(attempt.remaining())).await;
                }
            }
        }
    }

    fn start_forced_cleanup(&mut self) {
        let defects = &mut self.defects;
        #[cfg(windows)]
        if let Some(job) = self.job.as_ref() {
            if let Err(error) = job.terminate_checked() {
                defects.push(format!("job termination failed: {error}"));
            }
        }
        #[cfg(unix)]
        if self.pgid > 0 {
            // SAFETY: spawn records pgid using i32::try_from(child.id()); the > 0 guard
            // makes its negation representable. kill accepts this group selector and SIGKILL
            // without accessing caller memory.
            let killed = unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
            if killed != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                defects.push(format!(
                    "process-group termination failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        if let Some(child) = self.child.as_mut() {
            if let Err(error) = child.start_kill() {
                defects.push(format!("direct child termination failed: {error}"));
            }
        }
    }

    async fn settle(&mut self, wait: &ProbeWait) -> ProbeSettlement {
        #[cfg(test)]
        let settlement_window = self
            .settlement_hook
            .as_ref()
            .and_then(|hook| hook.settlement_window)
            .unwrap_or(ACTIVE_SETTLEMENT_WINDOW);
        #[cfg(not(test))]
        let settlement_window = ACTIVE_SETTLEMENT_WINDOW;
        let mut attempt = crate::process::host::job::SettlementAttempt::new(settlement_window);
        let cleanup_required = !matches!(wait, ProbeWait::Exited(Ok(_)));
        if cleanup_required {
            self.start_forced_cleanup();
        }

        self.settle_child(cleanup_required, &mut attempt).await;

        // Release Tokio's direct process handle before the first native
        // emptiness query that is eligible to prove settlement.
        drop(self.child.take());

        #[cfg(windows)]
        if !cleanup_required {
            if let Some(job) = self.job.as_ref() {
                job.terminate();
            }
        }
        #[cfg(unix)]
        if !cleanup_required && self.pgid > 0 {
            // SAFETY: self.pgid > 0 above makes its negation a representable group
            // selector. kill accepts SIGKILL and takes no pointers into Rust memory.
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
        }

        let defects = &mut self.defects;
        let stdout = settle_probe_reader(&mut self.stdout, defects, &mut attempt).await;
        let stderr = settle_probe_reader(&mut self.stderr, defects, &mut attempt).await;

        #[cfg(test)]
        {
            let (ready, mut release) = self.settlement_hook.as_mut().map_or((None, None), |hook| {
                (
                    hook.before_native_proof.take(),
                    hook.release_native_proof.take(),
                )
            });
            if let Some(ready) = ready {
                let _ = ready.send(());
            }
            if let Some(release) = release.as_mut() {
                loop {
                    if attempt.expired() {
                        record_probe_attempt_expiry(defects, "native-proof barrier");
                        attempt.restart();
                    }
                    match tokio::time::timeout(attempt.remaining(), &mut *release).await {
                        Ok(_) => break,
                        Err(_) => {
                            record_probe_attempt_expiry(defects, "native-proof barrier");
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
                panic!("injected probe panic during native settlement");
            }
        }

        #[cfg(windows)]
        self.settle_job(&mut attempt).await;
        #[cfg(unix)]
        self.settle_process_group(&mut attempt).await;

        #[cfg(windows)]
        drop(self.job.take());
        ProbeSettlement {
            stdout,
            stderr,
            defects: self.defects.clone(),
        }
    }

    #[cfg(windows)]
    async fn settle_job(&mut self, attempt: &mut crate::process::host::job::SettlementAttempt) {
        crate::process::agent::native_proof::settle_job(
            self.job.as_ref(),
            &mut self.defects,
            attempt,
            record_probe_attempt_expiry,
            #[cfg(test)]
            self.settlement_hook
                .as_mut()
                .map(|hook| hook as &mut dyn crate::process::agent::native_proof::NativeProofHooks),
        )
        .await;
    }

    #[cfg(unix)]
    async fn settle_process_group(
        &mut self,
        attempt: &mut crate::process::host::job::SettlementAttempt,
    ) {
        crate::process::agent::native_proof::settle_process_group(
            self.pgid,
            &mut self.defects,
            attempt,
            record_probe_attempt_expiry,
            #[cfg(test)]
            self.settlement_hook
                .as_mut()
                .map(|hook| hook as &mut dyn crate::process::agent::native_proof::NativeProofHooks),
        )
        .await;
    }

    pub(crate) async fn settle_observing_cancellation<F>(
        &mut self,
        wait: &ProbeWait,
        cancel: &mut tokio::sync::watch::Receiver<bool>,
        observed: F,
    ) -> (ProbeSettlement, bool)
    where
        F: FnOnce(),
    {
        if matches!(wait, ProbeWait::Cancelled) {
            observed();
            return (self.settle(wait).await, true);
        }
        let settlement = self.settle(wait);
        tokio::pin!(settlement);
        tokio::select! {
            biased;
            () = wait_for_probe_cancellation(cancel) => {
                observed();
                (settlement.await, true)
            }
            settled = &mut settlement => (settled, false),
        }
    }
}

/// The startup-update target keeps this slot outside its caught future. The
/// private process owner therefore survives a panic while wait/settlement is
/// being polled and can resume cleanup before any terminal publication.
#[derive(Default)]
pub(crate) struct RetainedProbeOwner {
    pub(crate) owner: Option<ProbeProcessOwner>,
}

impl RetainedProbeOwner {
    pub(crate) fn is_some(&self) -> bool {
        self.owner.is_some()
    }

    pub(crate) fn clear(&mut self) {
        drop(self.owner.take());
    }

    pub(crate) async fn settle_after_panic<F>(
        &mut self,
        cancel: &mut tokio::sync::watch::Receiver<bool>,
        observed: F,
    ) -> Option<String>
    where
        F: FnOnce(),
    {
        let owner = self.owner.as_mut()?;
        let (settlement, _) = owner
            .settle_observing_cancellation(&ProbeWait::Cancelled, cancel, observed)
            .await;
        (!settlement.defects.is_empty()).then(|| settlement.defects.join("; "))
    }
}
