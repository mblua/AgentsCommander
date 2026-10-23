use super::{
    ContainerSessionState, ContainerShutdownReport, ContainerTransportBackend,
    CONTAINER_GLOBAL_SWEEP_RETRY_BACKOFF, CONTAINER_SHUTDOWN_POLL,
    CONTAINER_SHUTDOWN_QUEUE_CAPACITY,
};
use crate::pty::container_runtime::{
    ContainerRuntimeControl, RetainedContainerOwnerContext, CONTAINER_STOP_TIMEOUT,
};
use crate::pty::container_tokens::ContainerApiTokenManager;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, TryLockError};
use std::time::{Duration, Instant};

impl ContainerTransportBackend {
    /// #992 - Is the startup sweep load-bearing for THIS config dir, or a courtesy
    /// pass over containers another install left behind?
    ///
    /// SCOPED, and it is not a gate: the sweep runs either way. No container client
    /// on record proves only that no orphan of OURS can exist; the machine-wide
    /// label means another install's orphan still can, and we still clean it. This
    /// only decides how long we are willing to wait for the list and how loudly we
    /// complain when it fails.
    ///
    /// Fails LOAD-BEARING (no config dir, unreadable registry): an unknown answer
    /// must never downgrade the sweep.
    pub(super) fn sweep_is_load_bearing(token_manager: Option<&ContainerApiTokenManager>) -> bool {
        let Some(manager) = token_manager else {
            return true;
        };
        match manager.has_container_clients() {
            Ok(has_clients) => has_clients,
            Err(err) => {
                log::warn!(
                    "[container-transport] {err}; treating the startup orphan sweep as load-bearing"
                );
                true
            }
        }
    }

    pub fn cleanup_labeled_orphans_on_startup(&self) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let token_manager = self.token_manager.clone();
        std::thread::spawn(move || {
            // #992 - the registry read happens INSIDE the thread on purpose: the
            // caller is the Tauri setup hook and must not pay a file read.
            let load_bearing = Self::sweep_is_load_bearing(token_manager.as_ref());
            // #992 - one line per start, at info, so the posture is observable. A
            // design whose correct behavior is indistinguishable from its failure
            // mode in the log is not verifiable. Do not lower this to debug.
            log::info!(
                "[container-transport] startup orphan sweep: {}",
                if load_bearing {
                    "load-bearing (this install has created containers)"
                } else {
                    "opportunistic (no container clients on record)"
                }
            );

            match runtime.cleanup_labeled_orphans(&HashSet::new(), CONTAINER_STOP_TIMEOUT) {
                Ok(report) => {
                    if !report.stopped.is_empty() {
                        log::warn!(
                            "[container-transport] stopped {} labeled orphan container(s) on \
                                startup",
                            report.stopped.len()
                        );
                    }
                    if !report.invalid_labels.is_empty() {
                        log::warn!(
                            "[container-transport] ignored {} labeled container(s) with invalid \
                                session labels",
                            report.invalid_labels.len()
                        );
                    }
                }
                Err(err) => {
                    if load_bearing {
                        log::warn!(
                            "[container-transport] startup orphan cleanup failed: {}",
                            err
                        );
                    } else {
                        // #992 - this install never created a container, so this
                        // failure means only that a courtesy pass over someone
                        // else's leftovers did not happen. That is not the user's
                        // problem and must not warn on every start: it is exactly
                        // the log noise #992 was reported from.
                        log::debug!(
                            "[container-transport] opportunistic startup orphan sweep failed: {}",
                            err
                        );
                    }
                }
            }

            if let Some(manager) = token_manager {
                match manager.revoke_all_container_clients() {
                    Ok(count) if count > 0 => {
                        log::warn!(
                            "[container-transport] revoked {} container API client(s) on startup \
                                cleanup",
                            count
                        );
                    }
                    Ok(_) => {}
                    Err(err) => {
                        log::warn!(
                            "[container-transport] startup container token cleanup failed: {}",
                            err
                        );
                    }
                }
            }
        });
    }

    pub fn stop_all_started_containers_blocking(
        &self,
        budget: Duration,
    ) -> ContainerShutdownReport {
        let deadline = Instant::now() + budget;
        let mut retained_residue = Vec::new();
        let session_ids = loop {
            match self.sessions.try_lock() {
                Ok(sessions) => break sessions.keys().copied().collect::<Vec<_>>(),
                Err(TryLockError::Poisoned(_)) => {
                    retained_residue.push(
                        RetainedContainerOwnerContext {
                            owner: "installedContainerRegistry",
                            session_id: None,
                            reason: "global-shutdown".to_string(),
                            program: None,
                            runtime_handle: None,
                            state: "installed",
                            in_flight: false,
                            last_error: Some(
                                "container session registry lock poisoned".to_string(),
                            ),
                        }
                        .diagnostic(),
                    );
                    break Vec::new();
                }
                Err(TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        retained_residue.push(
                            RetainedContainerOwnerContext {
                                owner: "installedContainerRegistry",
                                session_id: None,
                                reason: "global-shutdown".to_string(),
                                program: None,
                                runtime_handle: None,
                                state: "installed",
                                in_flight: true,
                                last_error: Some(
                                    "container session registry deadline reached".to_string(),
                                ),
                            }
                            .diagnostic(),
                        );
                        break Vec::new();
                    }
                    std::thread::sleep(CONTAINER_SHUTDOWN_POLL.min(remaining));
                }
            }
        };
        let route_remover = if session_ids.is_empty() {
            Ok(None)
        } else {
            self.route_remover_until(deadline)
        };
        if let Err(error) = &route_remover {
            retained_residue.push(
                RetainedContainerOwnerContext {
                    owner: "routeRemoverRegistry",
                    session_id: None,
                    reason: "global-shutdown".to_string(),
                    program: None,
                    runtime_handle: None,
                    state: "retained",
                    in_flight: false,
                    last_error: Some(error.to_string()),
                }
                .diagnostic(),
            );
        }
        let control = ContainerRuntimeControl::default();
        control.request_shutdown(deadline);
        for session_id in session_ids {
            if Instant::now() >= deadline {
                log::warn!(
                    "[container-transport] global shutdown cleanup budget exhausted before \
                        ownership transfer session={} state=installed",
                    session_id
                );
                break;
            }
            let resources = match self.remove_session_state_until(session_id, deadline) {
                Ok(Some(resources)) => resources,
                Ok(None) => continue,
                Err(error) => {
                    retained_residue.push(
                        RetainedContainerOwnerContext {
                            owner: "installedContainer",
                            session_id: Some(session_id),
                            reason: "global-shutdown".to_string(),
                            program: None,
                            runtime_handle: None,
                            state: "installed",
                            in_flight: false,
                            last_error: Some(error.to_string()),
                        }
                        .diagnostic(),
                    );
                    break;
                }
            };
            let entry = self.retain_removed_resources(resources, "global-shutdown-sweep");
            entry.require_route_removal(route_remover.clone());
            log::debug!(
                "[container-transport] global shutdown transferred installed handle and \
                    route-removal ownership session={} reason={} state=retained",
                entry.session_id,
                entry.reason
            );
        }

        let mut work_report = ContainerShutdownReport {
            terminal: true,
            retained: Vec::new(),
        };
        loop {
            if Instant::now() >= deadline {
                work_report.terminal = false;
                work_report
                    .retained
                    .push("reason=global-sweep-deadline state=retained".to_string());
                break;
            }
            let entries = self
                .cleanup_ownership
                .entries_for_sweep(deadline, CONTAINER_SHUTDOWN_QUEUE_CAPACITY);
            let runtime_has_retained = self
                .runtime
                .as_ref()
                .is_some_and(|runtime| !runtime.retained_cleanup_contexts().is_empty());
            if entries.is_empty()
                && !runtime_has_retained
                && !self
                    .shutdown_work
                    .has_owned_work_until(deadline)
                    .unwrap_or(true)
            {
                break;
            }
            if !self.shutdown_work.start_global_sweep_epoch(deadline) {
                work_report.terminal = false;
                work_report
                    .retained
                    .push("reason=global-sweep-executor state=retained".to_string());
                break;
            }

            let epoch = self.cleanup_attempt_epoch.fetch_add(1, Ordering::Relaxed);
            let mut scheduled = 0_usize;
            if runtime_has_retained {
                if let Some(runtime) = self.runtime.clone() {
                    if self.shutdown_work.spawn_global_sweep_owned_with_control(
                        None,
                        "runtime-retained-cleanup",
                        control.clone(),
                        deadline,
                        move |control| runtime.retry_retained_cleanups(control),
                    ) {
                        scheduled += 1;
                    }
                }
            }
            for entry in entries {
                if Instant::now() >= deadline {
                    break;
                }
                let ownership = Arc::clone(&self.cleanup_ownership);
                let session_id = entry.session_id;
                let reason = entry.reason;
                if !self.shutdown_work.spawn_global_sweep_owned_with_control(
                    Some(session_id),
                    reason,
                    control.clone(),
                    deadline,
                    move |control| ownership.attempt(entry, epoch, control),
                ) {
                    break;
                }
                scheduled += 1;
            }

            work_report = self.shutdown_work.seal_and_drain_until(deadline);
            if !work_report.terminal || Instant::now() >= deadline {
                break;
            }
            let cleanup_remaining = !self
                .cleanup_ownership
                .is_empty_until(deadline)
                .unwrap_or(false);
            let runtime_remaining = self
                .runtime
                .as_ref()
                .is_some_and(|runtime| !runtime.retained_cleanup_contexts().is_empty());
            if !cleanup_remaining && !runtime_remaining {
                break;
            }
            if scheduled == 0 {
                log::warn!(
                    "[container-transport] global sweep epoch made no scheduling progress \
                        state=retained"
                );
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            std::thread::sleep(CONTAINER_GLOBAL_SWEEP_RETRY_BACKOFF.min(remaining));
        }
        let mut retained = retained_residue;
        retained.extend(work_report.retained.into_iter().map(|context| {
            if context.starts_with("owner=") {
                context
            } else {
                format!("owner=containerShutdownWork {context}")
            }
        }));
        match self.sessions.try_lock() {
            Ok(sessions) => retained.extend(sessions.iter().map(|(id, state)| {
                RetainedContainerOwnerContext {
                    owner: "installedContainer",
                    session_id: Some(*id),
                    reason: "global-shutdown".to_string(),
                    program: None,
                    runtime_handle: Some(match state {
                        ContainerSessionState::Pending(pending) => pending.runtime_handle.is_some(),
                        ContainerSessionState::Attaching(attaching) => {
                            attaching.runtime_handle.is_some()
                        }
                        ContainerSessionState::Active(active) => active.runtime_handle.is_some(),
                    }),
                    state: "installed",
                    in_flight: false,
                    last_error: None,
                }
                .diagnostic()
            })),
            Err(TryLockError::Poisoned(error)) => {
                let sessions = error.into_inner();
                retained.extend(sessions.iter().map(|(id, state)| {
                    RetainedContainerOwnerContext {
                        owner: "installedContainer",
                        session_id: Some(*id),
                        reason: "global-shutdown".to_string(),
                        program: None,
                        runtime_handle: Some(match state {
                            ContainerSessionState::Pending(pending) => {
                                pending.runtime_handle.is_some()
                            }
                            ContainerSessionState::Attaching(attaching) => {
                                attaching.runtime_handle.is_some()
                            }
                            ContainerSessionState::Active(active) => {
                                active.runtime_handle.is_some()
                            }
                        }),
                        state: "installed",
                        in_flight: false,
                        last_error: Some("container session registry lock poisoned".to_string()),
                    }
                    .diagnostic()
                }));
            }
            Err(TryLockError::WouldBlock) => retained.push(
                RetainedContainerOwnerContext {
                    owner: "installedContainerRegistry",
                    session_id: None,
                    reason: "global-shutdown".to_string(),
                    program: None,
                    runtime_handle: None,
                    state: "installed",
                    in_flight: true,
                    last_error: Some(
                        "container session registry diagnostic lock unavailable".to_string(),
                    ),
                }
                .diagnostic(),
            ),
        }
        retained.extend(
            self.cleanup_ownership
                .contexts()
                .into_iter()
                .map(|context| context.diagnostic()),
        );
        if let Some(runtime) = self.runtime.as_ref() {
            retained.extend(
                runtime
                    .retained_cleanup_contexts()
                    .into_iter()
                    .map(|context| context.diagnostic()),
            );
        }
        retained.sort();
        retained.dedup();
        let terminal = work_report.terminal && retained.is_empty();
        if !terminal {
            log::error!(
                "[container-transport] global shutdown cleanup retained unresolved ownership \
                    count={}",
                retained.len()
            );
        }
        ContainerShutdownReport { terminal, retained }
    }
}
