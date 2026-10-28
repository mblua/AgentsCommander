use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::purge_guard::PurgeGuard;
use crate::session::selection::{CriticalAdmissionOutcome, SelectionCoordinator};
use crate::session::session::SessionStatus;
use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(crate) const OBSERVATION_ALLOWANCE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TeardownOutcome {
    Closed,
    AlreadyGone,
    Failed(String),
    Uncertain {
        reason: String,
        may_complete_later: bool,
    },
}

impl TeardownOutcome {
    pub(crate) fn confirmed(&self) -> bool {
        matches!(self, Self::Closed | Self::AlreadyGone)
    }

    pub(crate) fn reason(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) | Self::Uncertain { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

pub(crate) fn observation_budget(force: bool, timeout_secs: u32) -> Option<Duration> {
    if force {
        Some(OBSERVATION_ALLOWANCE)
    } else {
        Duration::from_secs(u64::from(timeout_secs)).checked_add(OBSERVATION_ALLOWANCE)
    }
}

pub(crate) async fn observe_close_attempt<F>(
    budget: Duration,
    accepted: Arc<AtomicBool>,
    attempt: F,
) -> TeardownOutcome
where
    F: Future<Output = TeardownOutcome>,
{
    match tokio::time::timeout(budget, attempt).await {
        Ok(outcome) => outcome,
        Err(_) => TeardownOutcome::Uncertain {
            reason: "observer_timeout".into(),
            may_complete_later: accepted.load(Ordering::Acquire),
        },
    }
}

pub(crate) async fn wait_background_destroy<P, C, F>(mut pending: P, mut contains: C) -> bool
where
    P: FnMut() -> Result<bool, String>,
    C: FnMut() -> F,
    F: Future<Output = bool>,
{
    loop {
        match pending() {
            Ok(true) => tokio::time::sleep(Duration::from_millis(100)).await,
            Ok(false) => return !contains().await,
            Err(_) => return std::future::pending::<bool>().await,
        }
    }
}

pub(crate) async fn force_close(
    manager: SessionManager,
    coordinator: SelectionCoordinator,
    sid: Uuid,
    accepted: Arc<AtomicBool>,
) -> TeardownOutcome {
    // Once admission is in flight, accepted or already-pending work cannot be excluded.
    accepted.store(true, Ordering::Release);
    match coordinator.background_destroy(sid).await {
        Ok(CriticalAdmissionOutcome::Completed(result)) => match result.into_single_result(sid) {
            Ok(()) => TeardownOutcome::Closed,
            Err(reason) => {
                accepted.store(false, Ordering::Release);
                // This reprobe is part of the owned attempt, never an unbounded parent tail.
                if manager.contains_public_or_pending(sid).await {
                    TeardownOutcome::Failed(format!("destroy_failed: {reason}"))
                } else {
                    TeardownOutcome::AlreadyGone
                }
            }
        },
        Ok(CriticalAdmissionOutcome::AlreadyPending) => {
            let absent = wait_background_destroy(
                move || coordinator.background_destroy_is_pending(sid),
                move || {
                    let manager = manager.clone();
                    async move { manager.contains_public_or_pending(sid).await }
                },
            )
            .await;
            accepted.store(false, Ordering::Release);
            if absent {
                TeardownOutcome::AlreadyGone
            } else {
                TeardownOutcome::Failed("duplicate_unconfirmed".into())
            }
        }
        Err(reason) => {
            // Receiver loss can follow enqueue. Establish final absence under the same witness.
            let absent = wait_background_destroy(
                move || coordinator.background_destroy_is_pending(sid),
                move || {
                    let manager = manager.clone();
                    async move { manager.contains_public_or_pending(sid).await }
                },
            )
            .await;
            accepted.store(false, Ordering::Release);
            if absent {
                TeardownOutcome::AlreadyGone
            } else {
                TeardownOutcome::Failed(format!("destroy_failed: {reason}"))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn graceful_close<E, B, F>(
    manager: SessionManager,
    coordinator: SelectionCoordinator,
    pty: Arc<Mutex<PtyManager>>,
    sid: Uuid,
    timeout_secs: u32,
    accepted: Arc<AtomicBool>,
    exit_command: E,
    mut mark_busy: B,
) -> TeardownOutcome
where
    E: Fn(&str, &[String]) -> String,
    B: FnMut(Uuid, usize) -> F,
    F: Future<Output = ()>,
{
    let sessions = manager.list_sessions().await;
    let Some(session) = sessions
        .iter()
        .find(|session| session.id == sid.to_string())
    else {
        return force_close(manager, coordinator, sid, accepted).await;
    };
    let command = exit_command(&session.shell, &session.shell_args);
    let injected = match PtyManager::acquire_input_writer(&pty, sid).await {
        Ok(permit) => {
            let result = PtyManager::write_with_permit(&permit, command.as_bytes());
            if result.is_ok() {
                accepted.store(true, Ordering::Release);
                mark_busy(sid, command.len()).await;
            }
            result.is_ok()
        }
        Err(_) => false,
    };
    if injected {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(u64::from(timeout_secs)) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let sessions = manager.list_sessions().await;
            match sessions
                .iter()
                .find(|session| session.id == sid.to_string())
            {
                Some(session) if matches!(session.status, SessionStatus::Exited(_)) => break,
                None => return TeardownOutcome::AlreadyGone,
                _ => {}
            }
        }
    }
    force_close(manager, coordinator, sid, accepted).await
}

pub(crate) struct PurgeTargets {
    pub(crate) ids: Vec<Uuid>,
    pub(crate) session_ids: HashSet<Uuid>,
    pub(crate) fqns: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PurgePhase {
    Acquiring,
    AcquiredAwaitGrant,
    Started(Uuid),
    Settling,
    Stopped,
}

#[derive(Clone, Debug)]
pub(crate) struct PurgeSnapshot {
    pub(crate) phase: PurgePhase,
    pub(crate) results: Vec<(Uuid, TeardownOutcome)>,
    pub(crate) ids: Vec<Uuid>,
    pub(crate) stop: bool,
    pub(crate) granted: bool,
    pub(crate) started: bool,
    pub(crate) deadline: Instant,
    pub(crate) reason: Option<String>,
}

pub(crate) struct PurgeProgress {
    state: Mutex<PurgeSnapshot>,
    changed: Notify,
    grant: Notify,
    stop: CancellationToken,
}

impl PurgeProgress {
    pub(crate) fn new(ids: Vec<Uuid>) -> Self {
        Self {
            state: Mutex::new(PurgeSnapshot {
                phase: PurgePhase::Acquiring,
                results: Vec::new(),
                ids,
                stop: false,
                granted: false,
                started: false,
                deadline: Instant::now() + OBSERVATION_ALLOWANCE,
                reason: None,
            }),
            changed: Notify::new(),
            grant: Notify::new(),
            stop: CancellationToken::new(),
        }
    }

    pub(crate) fn snapshot(&self) -> Result<PurgeSnapshot, String> {
        self.state
            .lock()
            .map(|state| state.clone())
            .map_err(|_| "purge state poisoned".into())
    }

    fn freeze(&self, reason: &str) -> Result<PurgeSnapshot, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "purge state poisoned".to_string())?;
        state.stop = true;
        state.reason = Some(reason.into());
        let snapshot = state.clone();
        drop(state);
        self.stop.cancel();
        Ok(snapshot)
    }
}

struct StopOnDrop(Arc<PurgeProgress>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if !matches!(self.0.snapshot(), Ok(state) if state.phase == PurgePhase::Stopped) {
            let _ = self.0.freeze("parent_lost");
            self.0.stop.cancel();
        }
    }
}

pub(crate) async fn run_purge_batch<A, F>(
    guard: Arc<PurgeGuard>,
    targets: PurgeTargets,
    progress: Arc<PurgeProgress>,
    attempt_budget: Duration,
    mut attempt: A,
) where
    A: FnMut(Uuid) -> F + Send + 'static,
    F: Future<Output = TeardownOutcome> + Send,
{
    let lease = tokio::select! {
        biased;
        _ = progress.stop.cancelled() => return,
        lease = guard.acquire(targets.session_ids, targets.fqns) => lease,
    };
    {
        let Ok(mut state) = progress.state.lock() else {
            return;
        };
        if state.stop {
            return;
        }
        state.phase = PurgePhase::AcquiredAwaitGrant;
    }
    progress.changed.notify_one();
    loop {
        let notified = progress.grant.notified();
        {
            let Ok(state) = progress.state.lock() else {
                return;
            };
            if state.stop {
                return;
            }
            if state.granted {
                break;
            }
        }
        tokio::select! {
            biased;
            _ = progress.stop.cancelled() => return,
            _ = notified => {}
        }
    }
    for sid in targets.ids {
        {
            let Ok(mut state) = progress.state.lock() else {
                return;
            };
            if state.stop {
                break;
            }
            state.phase = PurgePhase::Started(sid);
            state.started = true;
            state.deadline = Instant::now() + attempt_budget;
        }
        progress.changed.notify_one();
        // Accepted work is never selected against Stop or the observer deadline.
        let outcome = attempt(sid).await;
        let uncertain = matches!(outcome, TeardownOutcome::Uncertain { .. });
        {
            let Ok(mut state) = progress.state.lock() else {
                return;
            };
            state.results.push((sid, outcome));
            if Instant::now() >= state.deadline {
                state.reason = Some("observer_timeout: settlement_unconfirmed".into());
                state.stop = true;
            }
            if uncertain {
                state.stop = true;
            }
        }
        progress.changed.notify_one();
    }
    {
        let Ok(mut state) = progress.state.lock() else {
            return;
        };
        state.phase = PurgePhase::Settling;
    }
    drop(lease);
    {
        let Ok(mut state) = progress.state.lock() else {
            return;
        };
        state.phase = PurgePhase::Stopped;
    }
    progress.changed.notify_one();
}

pub(crate) fn observe_purge(
    progress: Arc<PurgeProgress>,
    mut worker: tokio::task::JoinHandle<()>,
) -> impl Future<Output = Result<PurgeSnapshot, String>> {
    let stop_on_drop = StopOnDrop(progress.clone());
    async move {
        let _stop_on_drop = stop_on_drop;
        loop {
            let changed = progress.changed.notified();
            let snapshot = {
                let mut state = progress
                    .state
                    .lock()
                    .map_err(|_| "purge state poisoned".to_string())?;
                if state.phase == PurgePhase::Stopped {
                    return Ok(state.clone());
                }
                if Instant::now() >= state.deadline {
                    // Stop and Started share this linearization point.
                    state.stop = true;
                    state.reason = Some("observer_timeout: settlement_unconfirmed".into());
                } else if state.phase == PurgePhase::AcquiredAwaitGrant && !state.granted {
                    state.granted = true;
                    progress.grant.notify_one();
                }
                state.clone()
            };
            if snapshot.stop {
                progress.stop.cancel();
                if !snapshot.started {
                    // Stop won before Started: only cancelled acquire/grant remains.
                    let _ = worker.await;
                }
                return Ok(snapshot);
            }
            tokio::select! {
                _ = changed => {},
                _ = tokio::time::sleep_until(snapshot.deadline) => {},
                result = &mut worker => {
                    let state = progress.snapshot()?;
                    if result.is_ok() && state.phase == PurgePhase::Stopped {
                        return Ok(state);
                    }
                    return progress.freeze("worker_lost: outcome_unknown");
                }
            }
        }
    }
}
