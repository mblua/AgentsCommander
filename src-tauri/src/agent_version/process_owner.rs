//! #1551 - private owner of one version-probe process tree: spawn, wait,
//! settle readers and kill/reap the tree before any terminal publication.

use std::time::Duration;

use super::{
    record_probe_attempt_expiry, settle_probe_reader, spawn_capped_reader,
    wait_for_probe_cancellation, CancellableProbeOutcome, ProbeOutcome, ProbeSettlement, ProbeWait,
    RecoverableProbeReader, ACTIVE_SETTLEMENT_POLL, ACTIVE_SETTLEMENT_WINDOW,
};
#[cfg(test)]
use super::{ProbeNativeObservation, ProbeSettlementHook, PROBE_SETTLEMENT_HOOK};

pub(super) struct ProbeProcessOwner {
    child: Option<tokio::process::Child>,
    stdout: Option<RecoverableProbeReader>,
    stderr: Option<RecoverableProbeReader>,
    defects: Vec<String>,
    #[cfg(windows)]
    job: Option<crate::pty::job::JobObject>,
    #[cfg(unix)]
    pgid: i32,
    #[cfg(test)]
    settlement_hook: Option<ProbeSettlementHook>,
}

impl ProbeProcessOwner {
    pub(super) async fn spawn(
        command: &mut tokio::process::Command,
    ) -> Result<Self, CancellableProbeOutcome> {
        #[cfg(windows)]
        let (mut child, job) = match crate::pty::job::spawn_suspended_contained(command).await {
            Ok(pair) => pair,
            Err(crate::pty::job::ContainedSpawnError::Spawn(error)) => {
                return Err(CancellableProbeOutcome::Completed(ProbeOutcome::Failed(
                    format!("spawn failed: {error}"),
                )))
            }
            Err(crate::pty::job::ContainedSpawnError::Containment(reason)) => {
                log::warn!(
                    "[agent-version] contained probe launch rejected: \
                     process containment failed at {reason}"
                );
                return Err(CancellableProbeOutcome::Completed(ProbeOutcome::Failed(
                    "Version probe process-tree containment unavailable.".to_string(),
                )));
            }
            Err(crate::pty::job::ContainedSpawnError::Cleanup(detail)) => {
                log::warn!("[agent-version] contained probe launch cleanup defective: {detail}");
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

    pub(super) async fn wait(
        &mut self,
        timeout: Duration,
        cancel: &mut tokio::sync::watch::Receiver<bool>,
    ) -> ProbeWait {
        let child = self.child.as_mut().expect("probe child owner");
        tokio::select! {
            result = child.wait() => ProbeWait::Exited(result),
            _ = tokio::time::sleep(timeout) => ProbeWait::TimedOut,
            _ = wait_for_probe_cancellation(cancel) => ProbeWait::Cancelled,
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
        let mut attempt = crate::pty::job::SettlementAttempt::new(settlement_window);
        let defects = &mut self.defects;
        let cleanup_required = !matches!(wait, ProbeWait::Exited(Ok(_)));

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
            // SAFETY: this is the positive process-group id created by
            // `Command::process_group(0)` for this probe owner.
            let killed = unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
            if killed != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
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
                        record_probe_attempt_expiry(defects, "direct-child settlement");
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
                    tokio::time::sleep(ACTIVE_SETTLEMENT_POLL.min(attempt.remaining())).await;
                }
            }
        }

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
            // SAFETY: same owned positive group identity as above.
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
        }

        let stdout = settle_probe_reader(&mut self.stdout, defects, &mut attempt).await;
        let stderr = settle_probe_reader(&mut self.stderr, defects, &mut attempt).await;

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
                        ProbeNativeObservation::Empty => Ok(true),
                        ProbeNativeObservation::Active => Ok(false),
                        ProbeNativeObservation::Error(detail) => Err(detail.to_string()),
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
                            panic!("injected probe panic after native query defect");
                        }
                    }
                }
                if attempt.expired() {
                    record_probe_attempt_expiry(defects, "job accounting proof");
                    let _ = job.terminate_checked();
                    attempt.restart();
                }
                tokio::time::sleep(ACTIVE_SETTLEMENT_POLL.min(attempt.remaining())).await;
            }
        }

        #[cfg(unix)]
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
                        ProbeNativeObservation::Empty => Ok(true),
                        ProbeNativeObservation::Active => Ok(false),
                        ProbeNativeObservation::Error(detail) => Err(detail.to_string()),
                    },
                    None => {
                        // SAFETY: signal zero performs no mutation and checks the retained PGID.
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
                    // SAFETY: signal zero performs no mutation and checks the retained PGID.
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
                            panic!("injected probe panic after native query defect");
                        }
                    }
                }
                if attempt.expired() {
                    record_probe_attempt_expiry(defects, "process-group accounting proof");
                    // SAFETY: this remains the retained positive process group.
                    unsafe {
                        libc::kill(-self.pgid, libc::SIGKILL);
                    }
                    attempt.restart();
                }
                tokio::time::sleep(ACTIVE_SETTLEMENT_POLL.min(attempt.remaining())).await;
            }
        }

        #[cfg(windows)]
        drop(self.job.take());
        ProbeSettlement {
            stdout,
            stderr,
            defects: defects.clone(),
        }
    }

    pub(super) async fn settle_observing_cancellation<F>(
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
            _ = wait_for_probe_cancellation(cancel) => {
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
    pub(super) owner: Option<ProbeProcessOwner>,
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
