use shutdown_cleanup::{
    begin_container_shutdown, cleanup_pending_creates_after_join, remove_critical_key,
    seal_and_drain_container_shutdown_work_after_join,
};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::atomic::AtomicU8;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
#[cfg(test)]
pub(crate) use turn::BENCH_CONTEXT;
#[cfg(test)]
use turn::TEST_SELECTION_TURN;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::pty::container_backend::ContainerTransportBackend;
use crate::pty::manager::PtyManager;
use crate::session::manager::{PendingCreateBinding, SessionManager};
use crate::session::session::SessionInfo;
#[cfg(test)]
use crate::session::session::SessionStatus;
use crate::session::warnings::SessionWarning;
use crate::web::broadcast::WsBroadcaster;
#[cfg(test)]
use crate::DetachedSessionsState;

fn response_close_registrations() -> &'static Mutex<BTreeMap<Uuid, ResponseCloseRegistration>> {
    static REGISTRATIONS: OnceLock<Mutex<BTreeMap<Uuid, ResponseCloseRegistration>>> =
        OnceLock::new();
    REGISTRATIONS.get_or_init(Default::default)
}

pub(crate) fn remember_response_close_registration(
    registration: ResponseCloseRegistration,
) -> bool {
    let Ok(mut registrations) = response_close_registrations().lock() else {
        return false;
    };
    registrations.insert(registration.member.session_id, registration);
    true
}

pub(crate) fn response_close_registration(id: Uuid) -> Option<ResponseCloseRegistration> {
    response_close_registrations()
        .lock()
        .ok()?
        .get(&id)
        .cloned()
}

pub(crate) fn forget_response_close_registration(id: Uuid) {
    if let Ok(mut registrations) = response_close_registrations().lock() {
        registrations.remove(&id);
    }
}

pub(crate) async fn poll_response_close<R: Runtime>(app: &AppHandle<R>) {
    let Some(tracker) = app.try_state::<crate::pty::manager::ResponseCloseState>() else {
        log::debug!(
            target: "agentscommander_lib::response_close_diag",
            "response_close_diag id=unknown site={} stage=poll_health \
             outcome=tracker_missing health=unknown",
            line!()
        );
        return;
    };
    let diagnostic_status = tracker.diagnostic_poll_health();
    crate::commands::session::reconcile_response_closes(app).await;
    let Some(coordinator) = app.try_state::<SelectionCoordinator>() else {
        if diagnostic_status {
            log::debug!(
                target: "agentscommander_lib::response_close_diag",
                "response_close_diag id=unknown site={} stage=poll_status \
                 outcome=coordinator_missing registrations=unknown",
                line!()
            );
        }
        return;
    };
    let registrations = response_close_registrations()
        .lock()
        .ok()
        .map(|registrations| registrations.values().cloned().collect::<Vec<_>>());
    let Some(registrations) = registrations else {
        if diagnostic_status {
            log::debug!(
                target: "agentscommander_lib::response_close_diag",
                "response_close_diag id=unknown site={} stage=poll_status \
                 outcome=registration_lock_error registrations=unknown",
                line!()
            );
        }
        return;
    };
    if diagnostic_status {
        log::debug!(
            target: "agentscommander_lib::response_close_diag",
            "response_close_diag id=unknown site={} outcome=observed stage=poll_status \
             registration_count={}",
            line!(),
            registrations.len()
        );
    }
    for registration in registrations {
        if !registration.tracker.same_state(&tracker) {
            registration.diagnostic_poll_outcome(1, "same_tracker_mismatch");
            continue;
        }
        if registration.member.backend != crate::pty::backend::SessionBackendKind::LocalProcess {
            registration.diagnostic_poll_outcome(2, "nonlocal_registration");
            continue;
        }
        let _ = coordinator.try_response_close(app, &registration).await;
    }
}

const COORDINATOR_QUEUE_CAPACITY: usize = 64;
const COORDINATOR_ADMISSION_CAPACITY: usize = 65;
const CREATE_TICKET_CAPACITY: usize = 16;
pub const SHUTDOWN_CLEANUP_BUDGET_SECS: u64 = 5;
const SHUTDOWN_FINALIZATION_RESERVE_MAX: Duration = Duration::from_millis(500);

/// #2475 - test-only timing instrumentation: recording sinks with their own
/// registration id, per-invocation correlation and per-run occupancy.
#[cfg(test)]
pub(crate) mod timing_probe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionRequest {
    UserSwitch { session_id: Uuid },
    Restore { persisted_target: Option<Uuid> },
}

#[derive(Debug, Clone)]
pub(crate) struct DormantRestoreRequest {
    pub persisted: crate::config::sessions_persistence::PersistedSession,
    pub working_directory: String,
    pub is_coordinator: bool,
    pub is_root_agent: bool,
}

impl SelectionRequest {
    pub fn user_switch(session_id: Uuid) -> Self {
        Self::UserSwitch { session_id }
    }

    pub(crate) fn restore(persisted_target: Option<Uuid>) -> Self {
        Self::Restore { persisted_target }
    }
}

#[derive(Debug)]
struct FinalizeCreateRequest {
    owned_replacement: Option<OwnedReplacementContinuation>,
    binding: PendingCreateBinding,
    cause: SelectionCause,
    auto_select_precondition: Option<(String, u64)>,
    warnings: Vec<SessionWarning>,
}

struct ConditionalResponseClose {
    ticket: crate::pty::manager::ResponseCloseTicket,
}

impl fmt::Debug for ConditionalResponseClose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConditionalResponseClose")
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
enum CoordinatorJob {
    #[cfg(test)]
    InlineReplacementFixture {
        input: crate::commands::session::FixtureReplacementInput,
        control: Arc<ReplacementFixtureControl>,
        response: oneshot::Sender<crate::commands::session::FixtureReplacementResult>,
    },
    Transition {
        request: SelectionRequest,
        response: oneshot::Sender<Result<Option<SessionSelection>, String>>,
    },
    Snapshot {
        response: oneshot::Sender<Result<SessionSelection, String>>,
    },
    FinalizeCreate {
        request: FinalizeCreateRequest,
        response: oneshot::Sender<Result<SessionInfo, String>>,
    },
    RollbackCreate {
        binding: PendingCreateBinding,
    },
    RouteLoss {
        session_id: Uuid,
        exit_code: i32,
        response: oneshot::Sender<Result<(), String>>,
    },
    ConditionalResponseClose {
        request: ConditionalResponseClose,
    },
    Destroy {
        request: crate::commands::session::DestroyRequest,
        response: oneshot::Sender<Result<crate::commands::session::DestroyOutcome, String>>,
    },
    RestartLifecycle {
        entry: OwnedLifecycleEntry,
        request: crate::commands::session::RestartJobRequest,
        response: oneshot::Sender<Result<SessionInfo, String>>,
    },
    RootLifecycle {
        entry: OwnedLifecycleEntry,
        request: crate::commands::session::RootJobRequest,
        response: oneshot::Sender<Result<SessionInfo, String>>,
    },
    ResourceKill {
        session_id: Uuid,
        intent: TrustedResourceIntent,
        response: oneshot::Sender<Result<crate::resource_monitor::ResourceKillResult, String>>,
    },
    Detach {
        session_id: Uuid,
        geometry: Option<crate::config::settings::WindowGeometry>,
        suppress_selection: bool,
        response: oneshot::Sender<Result<String, String>>,
    },
    Attach {
        session_id: Uuid,
        response: oneshot::Sender<Result<(), String>>,
    },
    RestoreBarrier {
        nonce: Uuid,
        started: oneshot::Sender<()>,
        release: oneshot::Receiver<()>,
    },
}

struct CoordinatorInner {
    retained_replacements: Mutex<RetainedReplacements>,
    sender: mpsc::Sender<CoordinatorEnvelope>,
    receiver: Mutex<Option<mpsc::Receiver<CoordinatorEnvelope>>>,
    admission: Arc<Semaphore>,
    create_tickets: Arc<Semaphore>,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    phase: AtomicU8,
    blocking_seed_work: Arc<BlockingSeedWorkTracker>,
    critical_keys: Mutex<HashSet<CriticalAdmissionKey>>,
    shutdown: CancellationToken,
    worker: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    retained_workers: Mutex<Vec<tauri::async_runtime::JoinHandle<()>>>,
    retained_cleanup_tasks:
        Mutex<Vec<tokio::task::JoinHandle<crate::pty::container_backend::ContainerShutdownReport>>>,
    retained_rollback_tasks: Mutex<Vec<tokio::task::JoinHandle<Vec<String>>>>,
    cleanup_pty: Mutex<Option<Weak<Mutex<PtyManager>>>>,
    cleanup_container_backend: Mutex<Option<Weak<ContainerTransportBackend>>>,
}

#[derive(Clone)]
pub struct SelectionCoordinator {
    inner: Arc<CoordinatorInner>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionShutdownReport {
    pub persistence_safe: bool,
    pub retained: Vec<String>,
}

fn publish_selection<R: Runtime>(app: &AppHandle<R>, payload: &SessionSelection) {
    publish_lifecycle_event(app, "session_switched", payload);
}

fn publish_lifecycle_event<R: Runtime, T: Serialize>(
    app: &AppHandle<R>,
    event: &'static str,
    payload: &T,
) {
    let value = match serde_json::to_value(payload) {
        Ok(value) => value,
        Err(error) => {
            log::error!(
                "[selection] failed to serialize event={} payload: {}",
                event,
                error
            );
            return;
        }
    };
    if let Err(error) = app.emit(event, value.clone()) {
        log::error!(
            "[selection] Tauri publication failed event={}: {}",
            event,
            error
        );
    }
    if let Some(broadcaster) = app.try_state::<WsBroadcaster>() {
        broadcaster.broadcast_event(event, &value);
    } else {
        log::debug!(
            "[selection] WebSocket broadcaster unavailable event={}",
            event
        );
    }
}

pub(crate) fn publish_session_communication<R: Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
    communication: Option<&crate::session::session::SessionCommunication>,
) {
    publish_lifecycle_event(
        app,
        "session_communication_changed",
        &serde_json::json!({
            "sessionId": session_id.to_string(),
            "communication": communication,
        }),
    );
}

mod admission;
mod response_close;
#[cfg(test)]
pub(crate) use admission::BlockingSeedTaskError;
pub(crate) use admission::WatchdogKillOutcome;
pub use admission::{CriticalAdmissionKind, CriticalAdmissionOutcome};
mod payload;
mod replacement;
#[cfg(test)]
pub(crate) use replacement::FixtureRootReservation;
pub(crate) use replacement::{EntryReaderDemandReceipt, LifecycleEffectGuard, LifecycleEffectSlot};
#[cfg(test)]
mod replacement_fixture;
pub(crate) use replacement::{
    OwnedReplacementContinuation, OwnedReplacementWork, OwnedSourceStop, PreparedOwnedReplacement,
    ReplacementAdmissionStep, ResolvedReplacement, SourceExecutorGuard,
};
#[cfg(test)]
pub(crate) use replacement_fixture::{
    await_fixture_native, FixtureContextNotification, NativeAwaitPanicOnce,
    ReplacementFixtureAction, ReplacementFixtureControl, ReplacementFixtureCut,
    ReplacementFixtureOwnerObservation, ReplacementFixtureOwnerWeak, ReplacementFixtureReceipt,
};
mod envelope;
use admission::{BlockingSeedWorkTracker, CriticalAdmissionKey};
#[cfg(test)]
use admission::{CoordinatorPhase, CriticalAdmissionGuard};
pub(super) use envelope::{FinalizedOwnedCarryCapability, FinalizedOwnedCarryOutcome};
pub(crate) use payload::{DormantRuntimeWitness, LiveRuntimeWitness, SelectionCause};
pub use payload::{
    RuntimeSelectionSnapshot, SelectionCoordinatorError, SelectionMode, SelectionSource,
    SessionSelection, TrustedCreateIntent, TrustedResourceIntent, TrustedRestartIntent,
};
#[cfg(test)]
use replacement::ReplacementStage;
use replacement::{OwnedLifecycleEntry, RetainedReplacements};
pub(crate) use response_close::ResponseCloseRegistration;
#[cfg(all(test, windows))]
use response_close::{response_registration_route, response_registration_row};

mod continuation_cleanup;
mod continuation_launch;
mod coordinator_lifecycle;
mod coordinator_requests;
mod tickets;
pub(crate) use tickets::AutoCloseBatchTicket;
pub use tickets::{ContainerLifecycleSender, CreateFinalizationTicket};
mod execute;
mod shutdown_cleanup;

mod worker;

mod transaction;
pub(super) use transaction::CommitCapability;
pub(crate) use transaction::SelectionTransaction;
mod coordinator_runtime;
pub use coordinator_runtime::RestoreBarrierGuard;
mod continuation_runtime;

mod turn;
use envelope::CoordinatorEnvelope;
#[cfg(test)]
pub(crate) use turn::acquire_selection_read_turn;
#[cfg(test)]
pub(crate) use turn::run_owned_selection_operation_timed;
use turn::IN_SELECTION_WORKER;
pub(crate) use turn::{
    acquire_selection_operation_turn, run_owned_selection_operation, run_owned_selection_read_timed,
};
#[cfg(test)]
use turn::{selection_operation_turn, selection_timing_line};

impl<R: Runtime> SelectionTransaction<R> {
    pub(crate) fn publish_selection(&self, payload: &SessionSelection) {
        publish_selection(&self.app, payload);
    }

    pub(crate) fn publish_created(&self, info: &SessionInfo) {
        publish_lifecycle_event(&self.app, "session_created", info);
    }

    pub(crate) fn publish_destroyed(&self, session_id: Uuid) {
        publish_lifecycle_event(
            &self.app,
            "session_destroyed",
            &serde_json::json!({ "id": session_id.to_string() }),
        );
    }

    pub(crate) fn publish_view_requested(&self, session_id: Uuid) {
        publish_lifecycle_event(
            &self.app,
            "session_view_requested",
            &serde_json::json!({ "id": session_id.to_string() }),
        );
    }

    pub(crate) fn publish_communication_cleared(&self, session_id: Uuid) {
        publish_lifecycle_event(
            &self.app,
            "session_communication_changed",
            &serde_json::json!({
                "sessionId": session_id.to_string(),
                "communication": null,
            }),
        );
    }
}

#[cfg(test)]
mod tests;
