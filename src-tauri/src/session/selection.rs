use shutdown_cleanup::{
    begin_container_shutdown, cleanup_pending_creates_after_join, remove_critical_key,
    seal_and_drain_container_shutdown_work_after_join,
};
use std::any::Any;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
#[cfg(test)]
pub(crate) use turn::BENCH_CONTEXT;
#[cfg(test)]
pub(crate) use turn::TEST_SELECTION_TURN;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[cfg(test)]
use crate::app_state::DetachedSessionsState;
use crate::pty::container_backend::ContainerTransportBackend;
use crate::pty::manager::PtyManager;
use crate::session::manager::{PendingCreateBinding, SessionManager};
#[cfg(test)]
use crate::session::session::SessionStatus;
use crate::session::session::{SessionCommunication, SessionInfo};
use crate::session::warnings::SessionWarning;
use crate::web::broadcast::WsBroadcaster;

/// Selection's lifecycle operations; transport remains on `SelectionTransaction`
/// until later phases.
pub(crate) trait LifecycleExecutor<T>: Send + Sync {
    fn prepare<'a>(
        &'a self,
        transaction: &'a T,
        operation: Uuid,
        source: Option<Uuid>,
        root: bool,
        directory: Option<&'a str>,
    ) -> LifecycleFuture<'a, Result<LifecycleAdmission, String>>;
    fn validate<'a>(
        &'a self,
        transaction: &'a T,
        lease: &'a mut LifecycleLease,
    ) -> LifecycleFuture<'a, bool>;
    fn has_claim(&self, lease: &LifecycleLease) -> bool;
    fn release(&self, lease: LifecycleLease) -> bool;
    fn ensure_root_directory(&self, effect: LifecycleEffectGuard) -> Result<String, String>;
    fn destroy<'a>(
        &'a self,
        transaction: &'a T,
        request: DestroyRequest,
    ) -> LifecycleFuture<'a, Result<DestroyOutcome, String>>;
    fn restart<'a>(
        &'a self,
        transaction: &'a T,
        request: RestartJobRequest,
        lease: &'a LifecycleLease,
    ) -> LifecycleFuture<'a, LifecycleEntryOutcome>;
    fn root<'a>(
        &'a self,
        transaction: &'a T,
        request: RootJobRequest,
        lease: &'a LifecycleLease,
    ) -> LifecycleFuture<'a, LifecycleEntryOutcome>;
    fn conditional_close<'a>(
        &'a self,
        transaction: &'a T,
        ticket: crate::pty::manager::ResponseCloseTicket,
    ) -> LifecycleFuture<'a, Result<(), String>>;
    fn reconcile<'a>(&'a self, transaction: &'a T) -> LifecycleFuture<'a, ()>;
    fn complete_retirement<'a>(&'a self, transaction: &'a T, id: Uuid) -> LifecycleFuture<'a, ()>;
    #[cfg(test)]
    fn execute_fixture<'a>(
        &'a self,
        transaction: &'a T,
        input: OpaqueFixtureInput,
    ) -> LifecycleFuture<'a, OpaqueFixtureOutput>;
}

#[cfg(test)]
pub(crate) struct OpaqueFixtureInput {
    pub(crate) continuation: OwnedReplacementContinuation,
    pub(crate) payload: Box<dyn Any + Send + Sync>,
}

#[cfg(test)]
pub(crate) struct OpaqueFixtureOutput {
    pub(crate) outcome: replacement_fixture::ReplacementFixtureOutcome,
    pub(crate) payload: Box<dyn Any + Send + Sync>,
}

#[cfg(test)]
impl fmt::Debug for OpaqueFixtureOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueFixtureOutput")
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl fmt::Debug for OpaqueFixtureInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueFixtureInput")
            .field("continuation", &self.continuation)
            .finish_non_exhaustive()
    }
}

/// Synchronous publication; selection retains event names and serialization policy.
pub(crate) trait SelectionEvents {
    // First used by the transaction cut in P08.
    #[allow(dead_code)]
    fn selection_event(&self, event: &'static str, value: serde_json::Value);
}

pub(crate) trait SelectionContext: Sized + 'static {
    type Host: SelectionHost<Self>;
}

pub(crate) trait SelectionHost<C: SelectionContext>:
    Clone + Send + Sync + 'static + SelectionEvents + crate::session::warnings::SessionWarningEvents
{
    // First required by P08.
    #[allow(dead_code)]
    fn sessions_required(&self) -> &Arc<tokio::sync::RwLock<SessionManager>>;
    // First required by P08.
    #[allow(dead_code)]
    fn pty_required(&self) -> &Arc<Mutex<PtyManager>>;
    // First required by P10.
    #[allow(dead_code)]
    fn pty_optional(&self) -> Option<&Arc<Mutex<PtyManager>>>;
    // First required by P08.
    #[allow(dead_code)]
    fn detached_required(&self) -> &crate::app_state::DetachedSessionsState;
    // First required by P10.
    #[allow(dead_code)]
    fn settings_required(&self) -> &crate::config::settings::SettingsState;
    // First required by P13.
    #[allow(dead_code)]
    fn response_close_required(&self) -> &crate::pty::manager::ResponseCloseState;
    // First required by P13.
    #[allow(dead_code)]
    fn response_close_optional(&self) -> Option<&crate::pty::manager::ResponseCloseState>;
    // First required by P13.
    #[allow(dead_code)]
    fn coordinator_optional(&self) -> Option<&SelectionCoordinator>;
    // First required by P12.
    #[allow(dead_code)]
    fn tasks(&self) -> &dyn task::SelectionTasks;
    // First required by P10.
    #[allow(dead_code)]
    fn focus_window(&self, label: &str) -> Option<Result<(), String>>;
    // First required by P10.
    #[allow(dead_code)]
    fn destroy_window(&self, label: &str) -> Option<Result<(), String>>;
}

#[cfg(test)]
struct NeutralSelectionContext {
    _local: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(test)]
impl SelectionContext for NeutralSelectionContext {
    type Host = NeutralSelectionHost;
}

#[cfg(test)]
#[derive(Clone)]
struct NeutralSelectionHost {
    sessions: Arc<tokio::sync::RwLock<SessionManager>>,
    pty: Arc<Mutex<PtyManager>>,
    detached: crate::app_state::DetachedSessionsState,
    settings: crate::config::settings::SettingsState,
    response_close: crate::pty::manager::ResponseCloseState,
    coordinator: Option<SelectionCoordinator>,
    optional_present: bool,
}

#[cfg(test)]
impl NeutralSelectionHost {
    fn new(
        sessions: Arc<tokio::sync::RwLock<SessionManager>>,
        pty: Arc<Mutex<PtyManager>>,
        detached: crate::app_state::DetachedSessionsState,
        settings: crate::config::settings::SettingsState,
        response_close: crate::pty::manager::ResponseCloseState,
    ) -> Self {
        Self {
            sessions,
            pty,
            detached,
            settings,
            response_close,
            coordinator: None,
            optional_present: false,
        }
    }
}

#[cfg(test)]
impl SelectionEvents for NeutralSelectionHost {
    fn selection_event(&self, _event: &'static str, _value: serde_json::Value) {}
}

#[cfg(test)]
impl crate::session::warnings::SessionWarningEvents for NeutralSelectionHost {
    fn session_warning(&self, _warning: SessionWarning) {}
}

#[cfg(test)]
struct NeutralSelectionTasks;

#[cfg(test)]
impl task::SelectionTasks for NeutralSelectionTasks {
    fn spawn(
        &self,
        _future: futures::future::BoxFuture<'static, ()>,
    ) -> Pin<Box<dyn task::SelectionTask>> {
        panic!("host contract fixture does not spawn tasks")
    }
}

#[cfg(test)]
impl SelectionHost<NeutralSelectionContext> for NeutralSelectionHost {
    fn sessions_required(&self) -> &Arc<tokio::sync::RwLock<SessionManager>> {
        &self.sessions
    }

    fn pty_required(&self) -> &Arc<Mutex<PtyManager>> {
        &self.pty
    }

    fn pty_optional(&self) -> Option<&Arc<Mutex<PtyManager>>> {
        self.optional_present.then_some(&self.pty)
    }

    fn detached_required(&self) -> &crate::app_state::DetachedSessionsState {
        &self.detached
    }

    fn settings_required(&self) -> &crate::config::settings::SettingsState {
        &self.settings
    }

    fn response_close_required(&self) -> &crate::pty::manager::ResponseCloseState {
        &self.response_close
    }

    fn response_close_optional(&self) -> Option<&crate::pty::manager::ResponseCloseState> {
        self.optional_present.then_some(&self.response_close)
    }

    fn coordinator_optional(&self) -> Option<&SelectionCoordinator> {
        self.coordinator.as_ref()
    }

    fn tasks(&self) -> &dyn task::SelectionTasks {
        &NeutralSelectionTasks
    }

    fn focus_window(&self, _label: &str) -> Option<Result<(), String>> {
        None
    }

    fn destroy_window(&self, _label: &str) -> Option<Result<(), String>> {
        None
    }
}

pub(crate) type LifecycleFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub(crate) enum LifecycleAdmission {
    Ready(LifecycleLease),
    Waiting,
    NeedsRootDirectory,
}

pub(crate) struct LifecycleExecutorState<T> {
    executor: Arc<dyn LifecycleExecutor<T>>,
}

impl<T> LifecycleExecutorState<T> {
    pub(crate) fn new(executor: Arc<dyn LifecycleExecutor<T>>) -> Self {
        Self { executor }
    }

    pub(crate) fn get(&self) -> &Arc<dyn LifecycleExecutor<T>> {
        &self.executor
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionPortTestEvent {
    RootDirectoryComplete(bool),
    Validate(bool),
    Accepted(bool),
    NativeEntered,
    NativeOutcome(bool, bool),
    ReleaseAwaitBefore,
    ReleaseAwaitAfter,
    TransactionDropped,
    EntryDropped,
    HoldReleased,
    ResponseDropped,
    ResponseAttempted,
}
#[cfg(test)]
pub(crate) type SelectionPortTestTrace = Arc<Mutex<Vec<(Uuid, Uuid, SelectionPortTestEvent)>>>;

/// Non-Clone native sidecar, erased without retaining an executor or application handle.
pub(crate) struct LifecycleLease {
    #[cfg(test)]
    port_test_trace: Option<SelectionPortTestTrace>,
    payload: Box<dyn Any + Send + Sync>,
    release_receipt: Arc<AtomicU8>,
}

impl fmt::Debug for LifecycleLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LifecycleLease").finish_non_exhaustive()
    }
}

impl LifecycleLease {
    #[cfg(test)]
    pub(crate) fn port_test_attach_trace(&mut self, trace: SelectionPortTestTrace) {
        self.port_test_trace = Some(trace);
    }
    #[cfg(test)]
    pub(crate) fn port_test_trace(&self) -> Option<SelectionPortTestTrace> {
        self.port_test_trace.clone()
    }

    pub(crate) fn new(payload: Box<dyn Any + Send + Sync>, release_receipt: Arc<AtomicU8>) -> Self {
        Self {
            payload,
            release_receipt,
            #[cfg(test)]
            port_test_trace: None,
        }
    }

    pub(crate) fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.payload.downcast_ref()
    }

    pub(crate) fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.payload.downcast_mut()
    }

    pub(crate) fn release(self) -> bool {
        let Self {
            payload,
            release_receipt,
            ..
        } = self;
        drop(payload);
        release_receipt.load(Ordering::Acquire) == 1
    }
}

/// Core session destruction logic shared by the Tauri command and the `MailboxPoller`.
/// Kills PTY, detaches Telegram bridge, removes from `SessionManager`, persists, and emits events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestructionSource {
    ManualClose,
    AutoClose,
    SpawnRollback,
    BackgroundCleanup,
}

impl DestructionSource {
    pub(crate) fn cause(self) -> SelectionCause {
        match self {
            Self::ManualClose => SelectionCause::ManualClose,
            Self::AutoClose => SelectionCause::AutoClose,
            Self::SpawnRollback => SelectionCause::SpawnRollback,
            Self::BackgroundCleanup => SelectionCause::BackgroundCleanup,
        }
    }

    pub(crate) fn selection_source(self) -> SelectionSource {
        match self {
            Self::ManualClose => SelectionSource::ManualClose,
            Self::AutoClose => SelectionSource::AutoClose,
            Self::SpawnRollback => SelectionSource::SpawnRollback,
            Self::BackgroundCleanup => SelectionSource::BackgroundCleanup,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DestroyRequest {
    pub ids: Vec<Uuid>,
    pub source: DestructionSource,
    pub force_destroy_root: bool,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DestroyOutcome {
    pub destroyed_ids: Vec<Uuid>,
    pub retained_exited_ids: Vec<Uuid>,
    pub failed: Vec<(Uuid, String)>,
}

impl DestroyOutcome {
    pub fn succeeded(&self, id: Uuid) -> bool {
        self.destroyed_ids.contains(&id) || self.retained_exited_ids.contains(&id)
    }

    pub(crate) fn into_single_result(self, id: Uuid) -> Result<(), String> {
        if self.succeeded(id) {
            return Ok(());
        }
        let message = self
            .failed
            .into_iter()
            .find(|(failed_id, _)| *failed_id == id)
            .map_or_else(|| "Session not found".to_string(), |(_, message)| message);
        Err(message)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RestartJobRequest {
    pub session_id: Uuid,
    pub agent_id: Option<String>,
    pub requested_profile: Option<String>,
    pub skip_auto_resume: Option<bool>,
    pub activate_after: bool,
    pub intent: TrustedRestartIntent,
    pub communication_override: Option<SessionCommunication>,
    /// §1295 5.1b — creation-gate enforcement for the restart replacement.
    /// Root-agent and archived-root cwds are exempt by the gate itself, so
    /// callers pass the build default; the mailbox wake-restart passes
    /// `Enforce` explicitly (§1295 6.6 / mailbox.rs:9796).
    pub enforcement: crate::config::sessions_persistence::CreationGateEnforcement,
}

#[expect(
    clippy::large_enum_variant,
    reason = "Preserve lifecycle result and Interrupted dispatch ownership without new allocation"
)]
pub(crate) enum LifecycleEntryOutcome {
    Returned(Result<SessionInfo, String>),
    Interrupted,
}

#[derive(Debug, Clone)]
pub(crate) struct RootJobRequest {
    pub requested_agent_id: Option<String>,
    pub requested_profile: Option<String>,
    pub skip_auto_resume_for_new_session: bool,
    pub intent: TrustedCreateIntent,
    pub select_after: bool,
}

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

pub(crate) async fn poll_response_close<R: Runtime>(
    app: &AppHandle<R>,
    executor: Option<Arc<dyn LifecycleExecutor<SelectionTransaction<R>>>>,
) {
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
    if let Some(executor) = executor {
        let transaction = SelectionTransaction::new(app.clone(), Weak::new())
            .with_lifecycle_executor(Arc::clone(&executor));
        executor.reconcile(&transaction).await;
    } else {
        log::debug!(
            target: "agentscommander_lib::response_close_diag",
            "response_close_diag stage=poll_reconcile outcome=executor_missing"
        );
    }
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
        input: OpaqueFixtureInput,
        control: Arc<ReplacementFixtureControl>,
        response: oneshot::Sender<OpaqueFixtureOutput>,
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
        request: DestroyRequest,
        response: oneshot::Sender<Result<DestroyOutcome, String>>,
    },
    RestartLifecycle {
        entry: OwnedLifecycleEntry,
        request: RestartJobRequest,
        response: oneshot::Sender<Result<SessionInfo, String>>,
    },
    RootLifecycle {
        entry: OwnedLifecycleEntry,
        request: RootJobRequest,
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
            log::error!("[selection] failed to serialize event={event} payload: {error}");
            return;
        }
    };
    if let Err(error) = app.emit(event, value.clone()) {
        log::error!("[selection] Tauri publication failed event={event}: {error}");
    }
    if let Some(broadcaster) = app.try_state::<WsBroadcaster>() {
        broadcaster.broadcast_event(event, &value);
    } else {
        log::debug!("[selection] WebSocket broadcaster unavailable event={event}");
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
pub(crate) mod authority;
pub(crate) mod response_close;
mod seal;
#[cfg(test)]
pub(crate) use admission::BlockingSeedTaskError;
pub(crate) use admission::WatchdogKillOutcome;
pub use admission::{CriticalAdmissionKind, CriticalAdmissionOutcome};
pub(crate) mod payload;
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
    ReplacementFixtureOutcome, ReplacementFixtureOwnerObservation, ReplacementFixtureOwnerWeak,
    ReplacementFixtureReceipt,
};
mod envelope;
use admission::{BlockingSeedWorkTracker, CriticalAdmissionKey};
#[cfg(test)]
use admission::{CoordinatorPhase, CriticalAdmissionGuard};
pub(crate) use payload::SelectionCause;
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
pub(crate) use transaction::SelectionTransaction;
mod coordinator_runtime;
pub use coordinator_runtime::RestoreBarrierGuard;
mod continuation_runtime;

pub(crate) mod task;
mod turn;
use envelope::CoordinatorEnvelope;
#[cfg(test)]
pub(crate) use turn::acquire_selection_read_turn;
#[cfg(test)]
pub(crate) use turn::run_owned_selection_operation_timed_with_tasks;
pub(crate) use turn::IN_SELECTION_WORKER;
pub(crate) use turn::{
    acquire_selection_operation_turn, run_owned_selection_operation_with_tasks,
    run_owned_selection_read_timed_with_tasks,
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
impl SelectionCoordinator {
    pub(crate) fn port_test_transaction<R: Runtime>(
        &self,
        app: AppHandle<R>,
    ) -> SelectionTransaction<R> {
        SelectionTransaction::new(app, Arc::downgrade(&self.inner))
    }
}

#[cfg(test)]
pub(crate) use tests::{port_test_binding, SelectionPortTestExit};

#[cfg(test)]
mod tests;
