use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use thiserror::Error;
use tokio::sync::{mpsc, Notify, OwnedSemaphorePermit};
use uuid::Uuid;

use super::{remove_critical_key, CoordinatorEnvelope, CoordinatorInner};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CriticalAdmissionKind {
    RouteLoss,
    WatchdogKill,
    BackgroundCleanup,
    ResponseClose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CriticalAdmissionKey {
    pub(in crate::session) session_id: Uuid,
    pub(in crate::session) kind: CriticalAdmissionKind,
}

pub(super) struct CriticalAdmissionGuard {
    pub(super) inner: Weak<CoordinatorInner>,
    pub(super) key: CriticalAdmissionKey,
}

impl CriticalAdmissionGuard {
    pub(in crate::session) fn new(
        inner: &Arc<CoordinatorInner>,
        key: CriticalAdmissionKey,
    ) -> Self {
        Self {
            inner: Arc::downgrade(inner),
            key,
        }
    }
}

impl Drop for CriticalAdmissionGuard {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            remove_critical_key(&inner, self.key);
        }
    }
}

pub(super) struct CriticalAdmissionReservation {
    pub(super) admission: OwnedSemaphorePermit,
    pub(super) slot: mpsc::OwnedPermit<CoordinatorEnvelope>,
    pub(super) guard: CriticalAdmissionGuard,
}

pub use crate::pty::container_lifecycle::CriticalAdmissionOutcome;

/// #1151 - `watchdog_resource_kill` must distinguish two conditions that the shared
/// `CriticalAdmissionOutcome::AlreadyPending` conflates: a genuinely in-flight critical
/// admission for the same UUID, and a session that is no longer public or pending. Only
/// the second may fall back to a registry-only quarantine retry.
#[derive(Debug, Clone)]
pub(crate) enum WatchdogKillOutcome {
    Completed(crate::resource_monitor::ResourceKillResult),
    AlreadyInFlight,
    NoPublicSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CoordinatorPhase {
    Bootstrapping = 0,
    Restoring = 1,
    Running = 2,
    Closing = 3,
}

impl CoordinatorPhase {
    pub(in crate::session) fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Restoring,
            2 => Self::Running,
            3 => Self::Closing,
            _ => Self::Bootstrapping,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum BlockingSeedTaskError {
    #[error("selectionCoordinatorUnavailable")]
    Unavailable,
    #[error("blocking seed task canceled before start")]
    CanceledBeforeStart,
    #[error("blocking seed task join failed: {0}")]
    Join(String),
}

pub(super) enum BlockingSeedWorkState {
    Queued {
        abort: Option<tokio::task::AbortHandle>,
    },
    Started,
}

pub(super) struct BlockingSeedTrackerState {
    pub(in crate::session) accepting: bool,
    pub(in crate::session) next_id: u64,
    pub(in crate::session) work: BTreeMap<u64, BlockingSeedWorkState>,
}

pub(super) struct BlockingSeedWorkTracker {
    pub(in crate::session) state: Mutex<BlockingSeedTrackerState>,
    pub(super) notify: Notify,
}

impl BlockingSeedWorkTracker {
    pub(in crate::session) fn new() -> Self {
        Self {
            state: Mutex::new(BlockingSeedTrackerState {
                accepting: true,
                next_id: 1,
                work: BTreeMap::new(),
            }),
            notify: Notify::new(),
        }
    }

    pub(in crate::session) fn lock_state(
        &self,
    ) -> std::sync::MutexGuard<'_, BlockingSeedTrackerState> {
        self.state.lock().unwrap_or_else(|error| {
            log::error!("[selection] blocking seed tracker mutex poisoned; recovering");
            error.into_inner()
        })
    }

    pub(in crate::session) fn register(
        &self,
        phase: &AtomicU8,
    ) -> Result<u64, BlockingSeedTaskError> {
        let mut state = self.lock_state();
        if !state.accepting
            || CoordinatorPhase::from_u8(phase.load(Ordering::Acquire)) != CoordinatorPhase::Running
        {
            return Err(BlockingSeedTaskError::Unavailable);
        }
        let id = state.next_id;
        let Some(next_id) = id.checked_add(1) else {
            state.accepting = false;
            log::error!("[selection] blocking seed tracker id space exhausted");
            return Err(BlockingSeedTaskError::Unavailable);
        };
        state.next_id = next_id;
        state
            .work
            .insert(id, BlockingSeedWorkState::Queued { abort: None });
        Ok(id)
    }

    pub(in crate::session) fn attach_abort(&self, id: u64, abort: tokio::task::AbortHandle) {
        let abort_now = {
            let mut state = self.lock_state();
            match state.work.get_mut(&id) {
                Some(BlockingSeedWorkState::Queued { abort: slot }) => {
                    *slot = Some(abort.clone());
                    false
                }
                Some(BlockingSeedWorkState::Started) => false,
                None => true,
            }
        };
        if abort_now {
            abort.abort();
        }
    }

    pub(in crate::session) fn begin(&self, id: u64) -> bool {
        let mut state = self.lock_state();
        match state.work.get_mut(&id) {
            Some(slot @ BlockingSeedWorkState::Queued { .. }) => {
                *slot = BlockingSeedWorkState::Started;
                true
            }
            Some(BlockingSeedWorkState::Started) | None => false,
        }
    }

    fn cancel_waiter(&self, id: u64) -> Option<tokio::task::AbortHandle> {
        let (abort, empty) = {
            let mut state = self.lock_state();
            let abort = match state.work.get(&id) {
                Some(BlockingSeedWorkState::Queued { .. }) => match state.work.remove(&id) {
                    Some(BlockingSeedWorkState::Queued { abort }) => abort,
                    _ => None,
                },
                Some(BlockingSeedWorkState::Started) | None => None,
            };
            (abort, state.work.is_empty())
        };
        if empty {
            self.notify.notify_waiters();
        }
        abort
    }

    pub(in crate::session) fn complete_started(&self, id: u64) {
        let empty = {
            let mut state = self.lock_state();
            match state.work.remove(&id) {
                Some(BlockingSeedWorkState::Started) => {}
                Some(BlockingSeedWorkState::Queued { .. }) => {
                    log::error!("[selection] blocking seed tracker completed queued work id={id}");
                }
                None => {
                    log::error!("[selection] blocking seed tracker lost started work id={id}");
                }
            }
            state.work.is_empty()
        };
        if empty {
            self.notify.notify_waiters();
        }
    }

    pub(in crate::session) fn seal(&self) -> Vec<tokio::task::AbortHandle> {
        let (aborts, empty) = {
            let mut state = self.lock_state();
            state.accepting = false;
            let queued_ids = state
                .work
                .iter()
                .filter_map(|(id, work)| {
                    matches!(work, BlockingSeedWorkState::Queued { .. }).then_some(*id)
                })
                .collect::<Vec<_>>();
            let mut aborts = Vec::new();
            for id in queued_ids {
                if let Some(BlockingSeedWorkState::Queued { abort: Some(abort) }) =
                    state.work.remove(&id)
                {
                    aborts.push(abort);
                }
            }
            (aborts, state.work.is_empty())
        };
        if empty {
            self.notify.notify_waiters();
        }
        aborts
    }

    pub(in crate::session) async fn wait_quiescent_until(&self, deadline: Instant) -> bool {
        loop {
            let notified = self.notify.notified();
            if self.lock_state().work.is_empty() {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || tokio::time::timeout(remaining, notified).await.is_err() {
                return self.lock_state().work.is_empty();
            }
        }
    }
}

pub(super) struct BlockingSeedAwaitGuard {
    pub(super) tracker: Arc<BlockingSeedWorkTracker>,
    pub(super) id: u64,
    pub(super) armed: bool,
}

impl BlockingSeedAwaitGuard {
    pub(in crate::session) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BlockingSeedAwaitGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(abort) = self.tracker.cancel_waiter(self.id) {
            abort.abort();
        }
    }
}

pub(super) struct WorkerCompletionGuard {
    pub(super) tracker: Arc<BlockingSeedWorkTracker>,
    pub(super) id: u64,
}

impl Drop for WorkerCompletionGuard {
    fn drop(&mut self) {
        self.tracker.complete_started(self.id);
    }
}

pub(super) enum BlockingSeedWorkerResult<T> {
    Completed(T),
    CanceledBeforeStart,
}
