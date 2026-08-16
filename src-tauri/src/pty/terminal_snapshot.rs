use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use terminal_snapshot_renderer::{
    encode_api_json_success_from_model, encode_api_success_payload,
    encode_host_json_success_from_model, encode_host_success_payload, render_png,
    terminal_snapshot_json_bytes_from_model, terminal_snapshot_payload_bytes, TerminalScreenModel,
    TerminalSnapshotFormat, TerminalSnapshotPayload, TerminalSnapshotReasonCode,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::config::teams::{VerifiedPtyInputIdentity, VerifiedTerminalSnapshotRoute};
use crate::pty::backend::{SessionBackendKind, TerminalScreenRead};
use crate::pty::context_scrape::ContextSessionLiveness;
use crate::pty::manager::{PtyManager, PtySnapshotRouteProof};
use crate::session::manager::{
    SessionManager, TerminalSnapshotRequesterFact, TerminalSnapshotSessionFact,
};
use crate::session::session::{SessionStatus, TEMP_SESSION_PREFIX};

pub(crate) const SNAPSHOT_SERVER_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const SNAPSHOT_INGRESS_LIMIT: usize = 8;
pub(crate) const SNAPSHOT_REQUESTER_RATE: usize = 6;
pub(crate) const SNAPSHOT_TARGET_RATE: usize = 12;
pub(crate) const SNAPSHOT_INGRESS_RATE: usize = 30;
pub(crate) const SNAPSHOT_RATE_WINDOW: Duration = Duration::from_secs(60);
pub(crate) const SNAPSHOT_LIMITER_KEY_CAP: usize = 4_096;
pub(crate) const SNAPSHOT_GLOBAL_IN_FLIGHT: usize = 2;
pub(crate) const SNAPSHOT_ARTIFACT_DIRECTORY_CAP: usize = 4_096;
pub(crate) const SNAPSHOT_ARTIFACT_FILE_CAP: usize = 8_192;
pub(crate) const SNAPSHOT_ARTIFACT_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TerminalSnapshotBlockingStage {
    RouteVerification,
    RouteProofs,
    SessionSelection,
    Capture,
    JsonPayload,
    PngPayload,
    ApiEnvelope,
    HostEnvelope,
    FinalVerification,
    #[cfg(test)]
    TestResourceRetention,
}

#[cfg(test)]
type TerminalSnapshotTestHook = Box<dyn FnOnce() + Send + 'static>;

#[cfg(test)]
type TerminalSnapshotResponsePublicationHook = Box<dyn FnOnce(&Path, &Path) + Send + 'static>;

#[cfg(all(test, unix))]
type TerminalSnapshotUnixCleanupHook =
    Box<dyn FnMut(crate::path_identity::UnixTrackedCleanupStage, &Path, &Path) + Send + 'static>;

#[cfg(all(test, unix))]
struct TerminalSnapshotUnixCleanupControl {
    claim_leaf: Option<std::ffi::OsString>,
    hook: TerminalSnapshotUnixCleanupHook,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TerminalSnapshotResponsePublicationStage {
    BeforeDirectoryRetention,
    BeforeTemporaryCreate,
    BeforeTemporaryCommit,
    AfterTemporaryCommit,
    AfterTemporaryWrite,
    BeforeAtomicRename,
    AfterAtomicRename,
    AfterFinalVerification,
    AfterRegistryCommit,
    AfterParentSync,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSnapshotResponsePublicationFailure {
    FileSync,
    ParentSync,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TerminalSnapshotHostCancellationStage {
    Processing,
    ResponseBytesReady,
    BeforePublish,
}

#[cfg(test)]
#[derive(Default)]
struct TerminalSnapshotBlockingControlState {
    entered: bool,
    released: bool,
    deadline_expired: bool,
    completed: bool,
    retained_payload_bytes: usize,
}

#[cfg(test)]
pub(crate) struct TerminalSnapshotBlockingControl {
    state: Mutex<TerminalSnapshotBlockingControlState>,
    changed: std::sync::Condvar,
    deadline_changed: tokio::sync::Notify,
    panic_after_release: Option<String>,
}

#[cfg(test)]
impl TerminalSnapshotBlockingControl {
    pub(crate) fn new(panic_after_release: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(TerminalSnapshotBlockingControlState::default()),
            changed: std::sync::Condvar::new(),
            deadline_changed: tokio::sync::Notify::new(),
            panic_after_release,
        })
    }

    fn wait_for(&self, completed: bool, label: &'static str) {
        let limit = Instant::now() + Duration::from_secs(60);
        let mut state = self.state.lock().expect("blocking control state");
        while if completed {
            !state.completed
        } else {
            !state.entered
        } {
            let remaining = limit.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "{label}");
            let (next, timeout) = self
                .changed
                .wait_timeout(state, remaining)
                .expect("blocking control wait");
            state = next;
            let reached = if completed {
                state.completed
            } else {
                state.entered
            };
            assert!(!timeout.timed_out() || reached, "{label}");
        }
    }

    pub(crate) fn wait_until_entered(&self) {
        self.wait_for(
            false,
            "blocking worker did not reach its deterministic barrier",
        );
    }

    pub(crate) fn wait_until_completed(&self) {
        self.wait_for(true, "detached blocking worker did not complete");
    }

    pub(crate) fn expire_deadline(&self) {
        let mut state = self.state.lock().expect("blocking control state");
        state.deadline_expired = true;
        drop(state);
        self.deadline_changed.notify_waiters();
    }

    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("blocking control state");
        state.released = true;
        self.changed.notify_all();
    }

    pub(crate) fn retained_payload_bytes(&self) -> usize {
        self.state
            .lock()
            .expect("blocking control state")
            .retained_payload_bytes
    }

    fn set_retained_payload_bytes(&self, bytes: usize) {
        self.state
            .lock()
            .expect("blocking control state")
            .retained_payload_bytes = bytes;
    }

    fn enter_worker(&self) {
        let mut state = self.state.lock().expect("blocking control state");
        state.entered = true;
        self.changed.notify_all();
        while !state.released {
            state = self
                .changed
                .wait(state)
                .expect("blocking control release wait");
        }
        drop(state);
        if let Some(payload) = self.panic_after_release.clone() {
            std::panic::panic_any(payload);
        }
    }

    fn complete_worker(&self) {
        let mut state = self.state.lock().expect("blocking control state");
        state.completed = true;
        self.changed.notify_all();
    }

    async fn deadline_expired(&self) {
        loop {
            let changed = self.deadline_changed.notified();
            if self
                .state
                .lock()
                .expect("blocking control state")
                .deadline_expired
            {
                return;
            }
            changed.await;
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSnapshotHostFinalizerStage {
    RevalidationEntry,
    RouteVerification,
    FinalDeadline,
}

#[cfg(test)]
#[derive(Default)]
struct TerminalSnapshotHostFinalizerControlState {
    entered: bool,
    released: bool,
    deadline_expired: bool,
    retained_response_bytes: usize,
}

#[cfg(test)]
pub(crate) struct TerminalSnapshotHostFinalizerControl {
    stage: TerminalSnapshotHostFinalizerStage,
    state: Mutex<TerminalSnapshotHostFinalizerControlState>,
    changed: std::sync::Condvar,
    panic_after_release: Option<String>,
}

#[cfg(test)]
impl TerminalSnapshotHostFinalizerControl {
    pub(crate) fn new(
        stage: TerminalSnapshotHostFinalizerStage,
        panic_after_release: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            stage,
            state: Mutex::new(TerminalSnapshotHostFinalizerControlState::default()),
            changed: std::sync::Condvar::new(),
            panic_after_release,
        })
    }

    pub(crate) fn wait_until_entered(&self) {
        let limit = Instant::now() + Duration::from_secs(60);
        let mut state = self.state.lock().expect("host finalizer control state");
        while !state.entered {
            let remaining = limit.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "host finalizer did not reach its deterministic barrier"
            );
            let (next, timeout) = self
                .changed
                .wait_timeout(state, remaining)
                .expect("host finalizer control wait");
            state = next;
            assert!(
                !timeout.timed_out() || state.entered,
                "host finalizer did not reach its deterministic barrier"
            );
        }
    }

    pub(crate) fn expire_deadline(&self) {
        self.state
            .lock()
            .expect("host finalizer control state")
            .deadline_expired = true;
    }

    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("host finalizer control state");
        state.released = true;
        self.changed.notify_all();
    }

    pub(crate) fn retained_response_bytes(&self) -> usize {
        self.state
            .lock()
            .expect("host finalizer control state")
            .retained_response_bytes
    }

    fn set_retained_response_bytes(&self, bytes: usize) {
        self.state
            .lock()
            .expect("host finalizer control state")
            .retained_response_bytes = bytes;
    }

    fn deadline_expired(&self) -> bool {
        self.state
            .lock()
            .expect("host finalizer control state")
            .deadline_expired
    }

    fn enter_stage(&self, stage: TerminalSnapshotHostFinalizerStage) {
        if self.stage != stage {
            return;
        }
        let mut state = self.state.lock().expect("host finalizer control state");
        state.entered = true;
        self.changed.notify_all();
        while !state.released {
            state = self
                .changed
                .wait(state)
                .expect("host finalizer control release wait");
        }
        drop(state);
        if let Some(payload) = self.panic_after_release.clone() {
            std::panic::panic_any(payload);
        }
    }
}

#[cfg(test)]
struct TerminalSnapshotTestTaskOutput<T> {
    outcome: Option<Result<T, crate::logging::PayloadPanic>>,
    control: Option<Arc<TerminalSnapshotBlockingControl>>,
}

#[cfg(test)]
impl<T> TerminalSnapshotTestTaskOutput<T> {
    fn into_outcome(mut self) -> Result<T, crate::logging::PayloadPanic> {
        let outcome = self
            .outcome
            .take()
            .expect("blocking task output consumed exactly once");
        if let Some(control) = self.control.take() {
            control.complete_worker();
        }
        outcome
    }
}

#[cfg(test)]
impl<T> Drop for TerminalSnapshotTestTaskOutput<T> {
    fn drop(&mut self) {
        drop(self.outcome.take());
        if let Some(control) = self.control.take() {
            control.complete_worker();
        }
    }
}

#[cfg(test)]
#[derive(Default)]
struct TerminalSnapshotTestState {
    target_session_lookups: std::sync::atomic::AtomicUsize,
    target_route_lookups: std::sync::atomic::AtomicUsize,
    api_success_handoffs: std::sync::atomic::AtomicUsize,
    host_before_final_revalidation: Mutex<Option<TerminalSnapshotTestHook>>,
    host_cancellation_hooks:
        Mutex<HashMap<TerminalSnapshotHostCancellationStage, VecDeque<TerminalSnapshotTestHook>>>,
    host_finalizer_controls: Mutex<VecDeque<Arc<TerminalSnapshotHostFinalizerControl>>>,
    api_before_capture: Mutex<Option<TerminalSnapshotTestHook>>,
    api_after_response_bytes: Mutex<Option<TerminalSnapshotTestHook>>,
    api_before_final_binding: Mutex<Option<TerminalSnapshotTestHook>>,
    response_publication_hooks: Mutex<
        HashMap<
            TerminalSnapshotResponsePublicationStage,
            VecDeque<TerminalSnapshotResponsePublicationHook>,
        >,
    >,
    response_publication_failures: Mutex<VecDeque<TerminalSnapshotResponsePublicationFailure>>,
    #[cfg(unix)]
    unix_cleanup_controls: Mutex<VecDeque<TerminalSnapshotUnixCleanupControl>>,
    blocking_controls: Mutex<
        HashMap<TerminalSnapshotBlockingStage, VecDeque<Arc<TerminalSnapshotBlockingControl>>>,
    >,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalSnapshotTestLookupCounts {
    pub target_session_lookups: usize,
    pub target_route_lookups: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSnapshotSourcePlane {
    HostCli,
    ContainerApi,
}

impl TerminalSnapshotSourcePlane {
    fn as_str(self) -> &'static str {
        match self {
            Self::HostCli => "host_cli",
            Self::ContainerApi => "container_api",
        }
    }
}

pub(crate) struct TerminalSnapshotServiceRequest {
    pub request_id: Uuid,
    pub target: String,
    pub format: TerminalSnapshotFormat,
    pub source_plane: TerminalSnapshotSourcePlane,
    pub host_authorization_deadline: Option<(Instant, chrono::DateTime<chrono::Utc>)>,
}

impl std::fmt::Debug for TerminalSnapshotServiceRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotServiceRequest")
            .field("format", &self.format)
            .field("source_plane", &self.source_plane)
            .field(
                "has_host_authorization_deadline",
                &self.host_authorization_deadline.is_some(),
            )
            .finish_non_exhaustive()
    }
}

pub(crate) enum TerminalSnapshotRequesterSelector {
    Host {
        token: Uuid,
        expected_root: crate::path_identity::VerifiedPathIdentity,
        claimed_from: String,
    },
    ApiSession(Uuid),
}

impl std::fmt::Debug for TerminalSnapshotRequesterSelector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Host { .. } => "TerminalSnapshotRequesterSelector::Host",
            Self::ApiSession(_) => "TerminalSnapshotRequesterSelector::ApiSession",
        })
    }
}

#[derive(Clone)]
pub(crate) struct TerminalSnapshotServiceContext {
    pub session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pub pty_manager: Arc<std::sync::Mutex<PtyManager>>,
    pub settings: SettingsState,
    pub restore: Arc<crate::RestoreInProgress>,
    pub purge: Arc<crate::session::purge_guard::PurgeGuard>,
}

pub(crate) enum PreparedSnapshotPayload {
    Json {
        request_id: String,
        requester: String,
        target: String,
        model: Arc<TerminalScreenModel>,
    },
    Png(Box<TerminalSnapshotPayload>),
}

impl std::fmt::Debug for PreparedSnapshotPayload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json { model, .. } => formatter
                .debug_struct("PreparedSnapshotPayload::Json")
                .field("model", model)
                .finish(),
            Self::Png(payload) => formatter
                .debug_tuple("PreparedSnapshotPayload::Png")
                .field(payload)
                .finish(),
        }
    }
}

impl PreparedSnapshotPayload {
    #[cfg(test)]
    fn retained_content_bytes(&self) -> usize {
        match self {
            Self::Json { model, .. } => model
                .screen
                .lines
                .iter()
                .map(|line| line.cells.len())
                .sum::<usize>()
                .saturating_mul(std::mem::size_of::<terminal_snapshot_renderer::TerminalCell>()),
            Self::Png(payload) => match &**payload {
                TerminalSnapshotPayload::Png { png, .. } => png.len(),
                TerminalSnapshotPayload::Json { .. } => 0,
            },
        }
    }

    fn payload_bytes(&self) -> Result<u64, terminal_snapshot_renderer::ProtocolError> {
        match self {
            Self::Json {
                request_id,
                requester,
                target,
                model,
            } => terminal_snapshot_json_bytes_from_model(request_id, requester, target, model),
            Self::Png(payload) => terminal_snapshot_payload_bytes(payload),
        }
    }

    fn encode_api(&self) -> Result<Vec<u8>, terminal_snapshot_renderer::ProtocolError> {
        match self {
            Self::Json {
                request_id,
                requester,
                target,
                model,
            } => encode_api_json_success_from_model(request_id, requester, target, model),
            Self::Png(payload) => encode_api_success_payload(payload),
        }
    }

    fn encode_host(
        &self,
        request_id: &str,
        confirmation_tag: &str,
        expires_at: &str,
    ) -> Result<Vec<u8>, terminal_snapshot_renderer::ProtocolError> {
        match self {
            Self::Json {
                request_id: payload_request_id,
                requester,
                target,
                model,
            } => {
                if payload_request_id != request_id {
                    return Err(terminal_snapshot_renderer::ProtocolError::Invalid);
                }
                encode_host_json_success_from_model(
                    request_id,
                    confirmation_tag,
                    expires_at,
                    requester,
                    target,
                    model,
                )
            }
            Self::Png(payload) => {
                encode_host_success_payload(request_id, confirmation_tag, expires_at, payload)
            }
        }
    }
}

pub(crate) struct TerminalSnapshotPrepared {
    payload: PreparedSnapshotPayload,
    finalization: TerminalSnapshotFinalization,
}

impl std::fmt::Debug for TerminalSnapshotPrepared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotPrepared")
            .field("payload", &self.payload)
            .finish_non_exhaustive()
    }
}

impl TerminalSnapshotPrepared {
    pub(crate) fn into_parts(self) -> (PreparedSnapshotPayload, TerminalSnapshotFinalization) {
        (self.payload, self.finalization)
    }
}

pub(crate) struct TerminalSnapshotPreAdmission {
    state: Arc<TerminalSnapshotState>,
    context: TerminalSnapshotServiceContext,
    manager: SessionManager,
    requester: RequesterProof,
    permit: RequesterSnapshotPermit,
    audit: TerminalSnapshotAuditGuard,
    deadline: Instant,
    host_wall_deadline: Option<chrono::DateTime<chrono::Utc>>,
    source_plane: TerminalSnapshotSourcePlane,
}

impl TerminalSnapshotPreAdmission {
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
}

pub(crate) struct TerminalSnapshotFinalization {
    state: Arc<TerminalSnapshotState>,
    context: TerminalSnapshotServiceContext,
    manager: SessionManager,
    requester: RequesterProof,
    route: VerifiedTerminalSnapshotRoute,
    selected: SelectedSession,
    permit: RequesterSnapshotPermit,
    audit: TerminalSnapshotAuditGuard,
    deadline: Instant,
    host_wall_deadline: Option<chrono::DateTime<chrono::Utc>>,
    #[cfg(test)]
    host_finalizer_control: Option<Arc<TerminalSnapshotHostFinalizerControl>>,
}

pub(crate) struct TerminalSnapshotDisclosure {
    state: Arc<TerminalSnapshotState>,
    permit: RequesterSnapshotPermit,
    audit: TerminalSnapshotAuditGuard,
    deadline: Instant,
}

impl TerminalSnapshotDisclosure {
    pub(crate) fn remaining(&self) -> Result<Duration, TerminalSnapshotReasonCode> {
        ensure_before_deadline(self.deadline, &self.state.shutdown)?;
        Ok(self.deadline.saturating_duration_since(Instant::now()))
    }

    pub(crate) fn ensure_handoff(&self) -> Result<(), TerminalSnapshotReasonCode> {
        ensure_before_deadline(self.deadline, &self.state.shutdown)
    }

    pub(crate) fn finalize_success(self) {
        self.audit.finalize("succeeded", None);
        drop(self.permit);
    }

    pub(crate) fn finalize_failure(self, reason: TerminalSnapshotReasonCode) {
        self.audit.finalize_failure(reason);
        drop(self.permit);
    }
}

#[derive(Default)]
struct RollingState {
    ingress: HashMap<String, VecDeque<Instant>>,
    requester: HashMap<String, VecDeque<Instant>>,
    target: HashMap<String, VecDeque<Instant>>,
    requester_in_flight: HashMap<String, usize>,
    target_in_flight: HashMap<String, usize>,
    global_in_flight: usize,
}

struct LimiterLeaseInner {
    limiter: Arc<Mutex<RollingState>>,
    requester_key: String,
    target_key: Mutex<Option<String>>,
}

impl Drop for LimiterLeaseInner {
    fn drop(&mut self) {
        let Ok(mut limiter) = self.limiter.lock() else {
            return;
        };
        decrement_counter(&mut limiter.requester_in_flight, &self.requester_key);
        if let Ok(target_key) = self.target_key.lock() {
            if let Some(target_key) = target_key.as_ref() {
                decrement_counter(&mut limiter.target_in_flight, target_key);
            }
        }
        limiter.global_in_flight = limiter.global_in_flight.saturating_sub(1);
    }
}

#[derive(Clone)]
struct RequesterSnapshotPermit {
    inner: Arc<LimiterLeaseInner>,
}

impl RequesterSnapshotPermit {
    fn promote_target(&self, key: String) -> Result<(), TerminalSnapshotReasonCode> {
        let now = Instant::now();
        let mut target_slot = self
            .inner
            .target_key
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        if target_slot.is_some() {
            return Err(TerminalSnapshotReasonCode::Internal);
        }
        let mut limiter = self
            .inner
            .limiter
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        prune_map(&mut limiter.target, now);
        if !limiter.target.contains_key(&key) && limiter.target.len() >= SNAPSHOT_LIMITER_KEY_CAP {
            return Err(TerminalSnapshotReasonCode::RateLimited);
        }
        let in_flight = limiter.target_in_flight.get(&key).copied().unwrap_or(0);
        let attempts = limiter.target.get(&key).map(VecDeque::len).unwrap_or(0);
        if attempts >= SNAPSHOT_TARGET_RATE || in_flight >= 1 {
            return Err(TerminalSnapshotReasonCode::RateLimited);
        }
        limiter
            .target
            .entry(key.clone())
            .or_default()
            .push_back(now);
        limiter.target_in_flight.insert(key.clone(), 1);
        *target_slot = Some(key);
        Ok(())
    }
}

fn decrement_counter<K: Eq + std::hash::Hash + Clone>(map: &mut HashMap<K, usize>, key: &K) {
    if let Some(value) = map.get_mut(key) {
        *value = value.saturating_sub(1);
        if *value == 0 {
            map.remove(key);
        }
    }
}

fn prune_window(window: &mut VecDeque<Instant>, now: Instant) {
    while window
        .front()
        .is_some_and(|accepted| now.saturating_duration_since(*accepted) >= SNAPSHOT_RATE_WINDOW)
    {
        window.pop_front();
    }
}

fn prune_map(map: &mut HashMap<String, VecDeque<Instant>>, now: Instant) {
    map.retain(|_, window| {
        prune_window(window, now);
        !window.is_empty()
    });
}

#[derive(Clone)]
struct TrackedArtifactDirectory {
    identity: crate::path_identity::VerifiedPathIdentity,
    retained: crate::path_identity::RetainedDirectory,
}

#[derive(Clone)]
struct TrackedArtifactFile {
    directory: crate::path_identity::FileObjectId,
    path: PathBuf,
    identity: crate::path_identity::VerifiedPathIdentity,
    expires_at: Instant,
    #[cfg(unix)]
    witness: crate::path_identity::RetainedUnixFileWitness,
    #[cfg(unix)]
    operation_generation: u64,
    #[cfg(unix)]
    operation_owner: Option<u64>,
}

#[cfg(unix)]
struct UnixArtifactOperation {
    object: crate::path_identity::FileObjectId,
    token: u64,
    directory: TrackedArtifactDirectory,
    path: PathBuf,
    identity: crate::path_identity::VerifiedPathIdentity,
    witness: crate::path_identity::RetainedUnixFileWitness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSnapshotArtifactCleanupOutcome {
    Removed,
    AlreadyAbsent,
    SourceRetained,
    #[cfg(unix)]
    PrivateClaimRetained,
    #[cfg(unix)]
    Busy,
    Conflict,
    #[cfg(unix)]
    Uncertain,
}

impl TerminalSnapshotArtifactCleanupOutcome {
    #[cfg(unix)]
    pub(crate) fn confirmed_absent(self) -> bool {
        matches!(self, Self::Removed | Self::AlreadyAbsent)
    }
}

#[derive(Default)]
struct ArtifactRegistry {
    directories: HashMap<crate::path_identity::FileObjectId, TrackedArtifactDirectory>,
    files: HashMap<crate::path_identity::FileObjectId, TrackedArtifactFile>,
    reservations: usize,
    directory_reservations: HashMap<crate::path_identity::FileObjectId, usize>,
}

pub(crate) struct TerminalSnapshotArtifactReservation {
    registry: Arc<Mutex<ArtifactRegistry>>,
    directory: TrackedArtifactDirectory,
    active: bool,
}

impl TerminalSnapshotArtifactReservation {
    pub(crate) fn commit(
        self,
        path: PathBuf,
        identity: crate::path_identity::VerifiedPathIdentity,
    ) -> Result<(), TerminalSnapshotReasonCode> {
        self.commit_with_ttl(path, identity, SNAPSHOT_ARTIFACT_TTL)
    }

    pub(crate) fn commit_with_ttl(
        mut self,
        path: PathBuf,
        identity: crate::path_identity::VerifiedPathIdentity,
        ttl: Duration,
    ) -> Result<(), TerminalSnapshotReasonCode> {
        self.directory
            .retained
            .verify_current()
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        let current_file = self
            .directory
            .retained
            .verify_regular_file(&path)
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        if !crate::path_identity::same_object(&current_file, &identity)
            || !crate::path_identity::is_verified_descendant(
                &current_file,
                &self.directory.identity,
            )
        {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        #[cfg(unix)]
        let witness = self
            .directory
            .retained
            .retain_unix_file_witness(&path, &identity)
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        let expires_at = Instant::now()
            .checked_add(ttl)
            .ok_or(TerminalSnapshotReasonCode::Internal)?;
        if let Some(existing) = registry.files.get_mut(&identity.object_id) {
            #[cfg(unix)]
            if existing.operation_owner.is_some() {
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            }
            #[cfg(unix)]
            let next_generation = existing
                .operation_generation
                .checked_add(1)
                .ok_or(TerminalSnapshotReasonCode::Internal)?;
            existing.directory = self.directory.identity.object_id;
            existing.path = path;
            existing.identity = identity;
            existing.expires_at = expires_at;
            #[cfg(unix)]
            {
                existing.witness = witness;
                existing.operation_generation = next_generation;
            }
        } else {
            registry.files.insert(
                identity.object_id,
                TrackedArtifactFile {
                    directory: self.directory.identity.object_id,
                    path,
                    identity,
                    expires_at,
                    #[cfg(unix)]
                    witness,
                    #[cfg(unix)]
                    operation_generation: 0,
                    #[cfg(unix)]
                    operation_owner: None,
                },
            );
        }
        release_artifact_reservation(&mut registry, self.directory.identity.object_id);
        self.active = false;
        Ok(())
    }
}

impl Drop for TerminalSnapshotArtifactReservation {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Ok(mut registry) = self.registry.lock() {
            release_artifact_reservation(&mut registry, self.directory.identity.object_id);
        }
    }
}

fn release_artifact_reservation(
    registry: &mut ArtifactRegistry,
    directory: crate::path_identity::FileObjectId,
) {
    registry.reservations = registry.reservations.saturating_sub(1);
    decrement_counter(&mut registry.directory_reservations, &directory);
    remove_idle_artifact_directory(registry, directory);
}

fn remove_idle_artifact_directory(
    registry: &mut ArtifactRegistry,
    directory: crate::path_identity::FileObjectId,
) {
    if !registry.directory_reservations.contains_key(&directory)
        && !registry
            .files
            .values()
            .any(|file| file.directory == directory)
    {
        registry.directories.remove(&directory);
    }
}

pub(crate) struct TerminalSnapshotState {
    ingress: Arc<Semaphore>,
    limiter: Arc<Mutex<RollingState>>,
    artifacts: Arc<Mutex<ArtifactRegistry>>,
    shutdown: crate::shutdown::ShutdownSignal,
    #[cfg(test)]
    test_state: TerminalSnapshotTestState,
}

impl TerminalSnapshotState {
    pub(crate) fn new(shutdown: crate::shutdown::ShutdownSignal) -> Arc<Self> {
        Arc::new(Self {
            ingress: Arc::new(Semaphore::new(SNAPSHOT_INGRESS_LIMIT)),
            limiter: Arc::new(Mutex::new(RollingState::default())),
            artifacts: Arc::new(Mutex::new(ArtifactRegistry::default())),
            shutdown,
            #[cfg(test)]
            test_state: TerminalSnapshotTestState::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn test_artifact_counts(&self) -> (usize, usize, usize) {
        let registry = self.artifacts.lock().expect("artifact registry");
        (
            registry.directories.len(),
            registry.files.len(),
            registry.reservations,
        )
    }

    #[cfg(test)]
    pub(crate) fn sweep_artifacts_for_test(&self, force: bool) {
        self.sweep_artifacts(force);
    }

    #[cfg(test)]
    pub(crate) fn reset_test_target_lookup_counts(&self) {
        self.test_state
            .target_session_lookups
            .store(0, Ordering::SeqCst);
        self.test_state
            .target_route_lookups
            .store(0, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn test_target_lookup_counts(&self) -> TerminalSnapshotTestLookupCounts {
        TerminalSnapshotTestLookupCounts {
            target_session_lookups: self
                .test_state
                .target_session_lookups
                .load(Ordering::SeqCst),
            target_route_lookups: self.test_state.target_route_lookups.load(Ordering::SeqCst),
        }
    }

    #[cfg(test)]
    fn install_blocking_control(
        &self,
        stage: TerminalSnapshotBlockingStage,
        control: Arc<TerminalSnapshotBlockingControl>,
    ) {
        self.test_state
            .blocking_controls
            .lock()
            .expect("blocking control queue")
            .entry(stage)
            .or_default()
            .push_back(control);
    }

    #[cfg(test)]
    fn take_blocking_control(
        &self,
        stage: TerminalSnapshotBlockingStage,
    ) -> Option<Arc<TerminalSnapshotBlockingControl>> {
        let mut controls = self
            .test_state
            .blocking_controls
            .lock()
            .expect("blocking control queue");
        let control = controls.get_mut(&stage).and_then(VecDeque::pop_front);
        if controls.get(&stage).is_some_and(VecDeque::is_empty) {
            controls.remove(&stage);
        }
        control
    }

    #[cfg(test)]
    fn mark_blocking_payload_retention(&self, stage: TerminalSnapshotBlockingStage, bytes: usize) {
        if let Some(control) = self
            .test_state
            .blocking_controls
            .lock()
            .expect("blocking control queue")
            .get(&stage)
            .and_then(VecDeque::front)
        {
            control.set_retained_payload_bytes(bytes);
        }
    }

    #[cfg(test)]
    fn has_blocking_controls(&self) -> bool {
        !self
            .test_state
            .blocking_controls
            .lock()
            .expect("blocking control queue")
            .is_empty()
            || !self
                .test_state
                .host_cancellation_hooks
                .lock()
                .expect("host cancellation hook queue")
                .is_empty()
            || !self
                .test_state
                .host_finalizer_controls
                .lock()
                .expect("host finalizer control queue")
                .is_empty()
            || !self
                .test_state
                .response_publication_hooks
                .lock()
                .expect("response publication hook queue")
                .is_empty()
            || !self
                .test_state
                .response_publication_failures
                .lock()
                .expect("response publication failure queue")
                .is_empty()
    }

    #[cfg(test)]
    pub(crate) fn install_host_cancellation_hook(
        &self,
        stage: TerminalSnapshotHostCancellationStage,
        hook: impl FnOnce() + Send + 'static,
    ) {
        self.test_state
            .host_cancellation_hooks
            .lock()
            .expect("host cancellation hook queue")
            .entry(stage)
            .or_default()
            .push_back(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn install_next_host_finalizer_control(
        &self,
        control: Arc<TerminalSnapshotHostFinalizerControl>,
    ) {
        self.test_state
            .host_finalizer_controls
            .lock()
            .expect("host finalizer control queue")
            .push_back(control);
    }

    #[cfg(test)]
    pub(crate) fn install_host_final_handoff_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *self
            .test_state
            .host_before_final_revalidation
            .lock()
            .expect("host final-handoff test hook lock") = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn install_api_before_capture_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *self
            .test_state
            .api_before_capture
            .lock()
            .expect("API before-capture test hook lock") = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn install_api_after_response_bytes_hook(
        &self,
        hook: impl FnOnce() + Send + 'static,
    ) {
        *self
            .test_state
            .api_after_response_bytes
            .lock()
            .expect("API response-bytes test hook lock") = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn install_api_final_handoff_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *self
            .test_state
            .api_before_final_binding
            .lock()
            .expect("API final-handoff test hook lock") = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn test_api_success_handoffs(&self) -> usize {
        self.test_state.api_success_handoffs.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn record_api_success_handoff(&self) {
        self.test_state
            .api_success_handoffs
            .fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn install_response_publication_hook(
        &self,
        stage: TerminalSnapshotResponsePublicationStage,
        hook: impl FnOnce(&Path, &Path) + Send + 'static,
    ) {
        self.test_state
            .response_publication_hooks
            .lock()
            .expect("response publication hook queue")
            .entry(stage)
            .or_default()
            .push_back(Box::new(hook));
    }

    #[cfg(all(test, unix))]
    pub(crate) fn install_unix_cleanup_hook(
        &self,
        claim_leaf: Option<std::ffi::OsString>,
        hook: impl FnMut(crate::path_identity::UnixTrackedCleanupStage, &Path, &Path) + Send + 'static,
    ) {
        self.test_state
            .unix_cleanup_controls
            .lock()
            .expect("Unix cleanup control queue")
            .push_back(TerminalSnapshotUnixCleanupControl {
                claim_leaf,
                hook: Box::new(hook),
            });
    }

    #[cfg(all(test, unix))]
    fn take_unix_cleanup_control(&self) -> Option<TerminalSnapshotUnixCleanupControl> {
        self.test_state
            .unix_cleanup_controls
            .lock()
            .expect("Unix cleanup control queue")
            .pop_front()
    }

    #[cfg(test)]
    pub(crate) fn install_response_after_publish_hook(&self, hook: impl FnOnce() + Send + 'static) {
        self.install_response_publication_hook(
            TerminalSnapshotResponsePublicationStage::AfterAtomicRename,
            move |_, _| hook(),
        );
    }

    #[cfg(test)]
    pub(crate) fn install_response_publication_failure(
        &self,
        failure: TerminalSnapshotResponsePublicationFailure,
    ) {
        self.test_state
            .response_publication_failures
            .lock()
            .expect("response publication failure queue")
            .push_back(failure);
    }

    #[cfg(test)]
    pub(crate) fn take_response_publication_failure(
        &self,
        expected: TerminalSnapshotResponsePublicationFailure,
    ) -> bool {
        let mut failures = self
            .test_state
            .response_publication_failures
            .lock()
            .expect("response publication failure queue");
        if failures.front() == Some(&expected) {
            failures.pop_front();
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    fn run_host_final_handoff_hook(&self) {
        let hook = self
            .test_state
            .host_before_final_revalidation
            .lock()
            .expect("host final-handoff test hook lock")
            .take();
        if let Some(hook) = hook {
            hook();
        }
    }

    #[cfg(test)]
    pub(crate) fn run_host_cancellation_hook(&self, stage: TerminalSnapshotHostCancellationStage) {
        let hook = {
            let mut hooks = self
                .test_state
                .host_cancellation_hooks
                .lock()
                .expect("host cancellation hook queue");
            let hook = hooks.get_mut(&stage).and_then(VecDeque::pop_front);
            if hooks.get(&stage).is_some_and(VecDeque::is_empty) {
                hooks.remove(&stage);
            }
            hook
        };
        if let Some(hook) = hook {
            hook();
        }
    }

    #[cfg(test)]
    fn take_next_host_finalizer_control(
        &self,
    ) -> Option<Arc<TerminalSnapshotHostFinalizerControl>> {
        self.test_state
            .host_finalizer_controls
            .lock()
            .expect("host finalizer control queue")
            .pop_front()
    }

    #[cfg(test)]
    async fn run_api_before_capture_hook(&self) {
        let hook = self
            .test_state
            .api_before_capture
            .lock()
            .expect("API before-capture test hook lock")
            .take();
        if let Some(hook) = hook {
            hook();
            tokio::task::yield_now().await;
        }
    }

    #[cfg(test)]
    pub(crate) async fn run_api_after_response_bytes_hook(&self) {
        let hook = self
            .test_state
            .api_after_response_bytes
            .lock()
            .expect("API response-bytes test hook lock")
            .take();
        if let Some(hook) = hook {
            hook();
            tokio::task::yield_now().await;
        }
    }

    #[cfg(test)]
    pub(crate) async fn run_api_final_handoff_hook(&self) {
        let hook = self
            .test_state
            .api_before_final_binding
            .lock()
            .expect("API final-handoff test hook lock")
            .take();
        if let Some(hook) = hook {
            hook();
            tokio::task::yield_now().await;
        }
    }

    #[cfg(test)]
    pub(crate) fn run_response_publication_hook(
        &self,
        stage: TerminalSnapshotResponsePublicationStage,
        temporary: &Path,
        destination: &Path,
    ) {
        let hook = {
            let mut hooks = self
                .test_state
                .response_publication_hooks
                .lock()
                .expect("response publication hook queue");
            let hook = hooks.get_mut(&stage).and_then(VecDeque::pop_front);
            if hooks.get(&stage).is_some_and(VecDeque::is_empty) {
                hooks.remove(&stage);
            }
            hook
        };
        if let Some(hook) = hook {
            hook(temporary, destination);
        }
    }

    pub(crate) fn start_artifact_cleanup(self: &Arc<Self>) {
        let state = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = state.shutdown.token().cancelled() => {
                        let cleanup = Arc::clone(&state);
                        let _ = tokio::task::spawn_blocking(move || cleanup.sweep_artifacts(true)).await;
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        let cleanup = Arc::clone(&state);
                        let _ = tokio::task::spawn_blocking(move || cleanup.sweep_artifacts(false)).await;
                    }
                }
            }
        });
    }

    pub(crate) fn reserve_artifact(
        &self,
        directory_path: &Path,
        directory_identity: &crate::path_identity::VerifiedPathIdentity,
    ) -> Result<TerminalSnapshotArtifactReservation, TerminalSnapshotReasonCode> {
        self.reserve_artifact_inner(directory_path, directory_identity, None)
    }

    pub(crate) fn reserve_existing_artifact(
        &self,
        directory_path: &Path,
        directory_identity: &crate::path_identity::VerifiedPathIdentity,
        object: crate::path_identity::FileObjectId,
    ) -> Result<TerminalSnapshotArtifactReservation, TerminalSnapshotReasonCode> {
        self.reserve_artifact_inner(directory_path, directory_identity, Some(object))
    }

    fn reserve_artifact_inner(
        &self,
        directory_path: &Path,
        directory_identity: &crate::path_identity::VerifiedPathIdentity,
        existing_object: Option<crate::path_identity::FileObjectId>,
    ) -> Result<TerminalSnapshotArtifactReservation, TerminalSnapshotReasonCode> {
        let retained = crate::path_identity::retain_directory(directory_path)
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        if !crate::path_identity::same_object(retained.identity(), directory_identity) {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        let mut registry = self
            .artifacts
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        if !registry
            .directories
            .contains_key(&directory_identity.object_id)
            && registry.directories.len() >= SNAPSHOT_ARTIFACT_DIRECTORY_CAP
        {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        if registry.files.len().saturating_add(registry.reservations) >= SNAPSHOT_ARTIFACT_FILE_CAP
            && !existing_object.is_some_and(|object| registry.files.contains_key(&object))
        {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        let directory = TrackedArtifactDirectory {
            identity: directory_identity.clone(),
            retained,
        };
        registry
            .directories
            .entry(directory_identity.object_id)
            .or_insert_with(|| directory.clone());
        registry.reservations += 1;
        *registry
            .directory_reservations
            .entry(directory_identity.object_id)
            .or_default() += 1;
        drop(registry);
        Ok(TerminalSnapshotArtifactReservation {
            registry: Arc::clone(&self.artifacts),
            directory,
            active: true,
        })
    }

    #[cfg(not(unix))]
    pub(crate) fn untrack_artifact(&self, identity: &crate::path_identity::VerifiedPathIdentity) {
        if let Ok(mut registry) = self.artifacts.lock() {
            if let Some(removed) = registry.files.remove(&identity.object_id) {
                remove_idle_artifact_directory(&mut registry, removed.directory);
            }
        }
    }

    #[cfg(unix)]
    fn begin_unix_artifact_operation(
        &self,
        expected: &crate::path_identity::VerifiedPathIdentity,
    ) -> Result<UnixArtifactOperation, TerminalSnapshotArtifactCleanupOutcome> {
        let mut registry = self
            .artifacts
            .lock()
            .map_err(|_| TerminalSnapshotArtifactCleanupOutcome::Uncertain)?;
        let tracked = registry
            .files
            .get(&expected.object_id)
            .ok_or(TerminalSnapshotArtifactCleanupOutcome::Conflict)?;
        if tracked.identity.object_id != expected.object_id {
            return Err(TerminalSnapshotArtifactCleanupOutcome::Conflict);
        }
        if tracked.operation_owner.is_some() {
            return Err(TerminalSnapshotArtifactCleanupOutcome::Busy);
        }
        let token = tracked
            .operation_generation
            .checked_add(1)
            .ok_or(TerminalSnapshotArtifactCleanupOutcome::Uncertain)?;
        let directory = registry
            .directories
            .get(&tracked.directory)
            .cloned()
            .ok_or(TerminalSnapshotArtifactCleanupOutcome::Conflict)?;
        let operation = UnixArtifactOperation {
            object: expected.object_id,
            token,
            directory,
            path: tracked.path.clone(),
            identity: tracked.identity.clone(),
            witness: tracked.witness.clone(),
        };
        let tracked = registry
            .files
            .get_mut(&expected.object_id)
            .ok_or(TerminalSnapshotArtifactCleanupOutcome::Conflict)?;
        tracked.operation_generation = token;
        tracked.operation_owner = Some(token);
        Ok(operation)
    }

    #[cfg(unix)]
    fn clear_unix_artifact_operation(
        &self,
        operation: &UnixArtifactOperation,
        outcome: TerminalSnapshotArtifactCleanupOutcome,
    ) -> TerminalSnapshotArtifactCleanupOutcome {
        let Ok(mut registry) = self.artifacts.lock() else {
            return TerminalSnapshotArtifactCleanupOutcome::Uncertain;
        };
        let Some(tracked) = registry.files.get_mut(&operation.object) else {
            return TerminalSnapshotArtifactCleanupOutcome::Conflict;
        };
        if tracked.operation_generation != operation.token
            || tracked.operation_owner != Some(operation.token)
        {
            return TerminalSnapshotArtifactCleanupOutcome::Conflict;
        }
        tracked.operation_owner = None;
        outcome
    }

    #[cfg(unix)]
    fn finish_unix_artifact_cleanup(
        &self,
        operation: UnixArtifactOperation,
        outcome: crate::path_identity::UnixTrackedCleanupOutcome,
    ) -> TerminalSnapshotArtifactCleanupOutcome {
        match outcome {
            crate::path_identity::UnixTrackedCleanupOutcome::Removed
            | crate::path_identity::UnixTrackedCleanupOutcome::AlreadyAbsent => {
                let public_outcome = if matches!(
                    outcome,
                    crate::path_identity::UnixTrackedCleanupOutcome::Removed
                ) {
                    TerminalSnapshotArtifactCleanupOutcome::Removed
                } else {
                    TerminalSnapshotArtifactCleanupOutcome::AlreadyAbsent
                };
                let Ok(mut registry) = self.artifacts.lock() else {
                    return TerminalSnapshotArtifactCleanupOutcome::Uncertain;
                };
                let valid_owner = registry
                    .files
                    .get(&operation.object)
                    .is_some_and(|tracked| {
                        tracked.operation_generation == operation.token
                            && tracked.operation_owner == Some(operation.token)
                    });
                if !valid_owner {
                    return TerminalSnapshotArtifactCleanupOutcome::Uncertain;
                }
                let Some(removed) = registry.files.remove(&operation.object) else {
                    return TerminalSnapshotArtifactCleanupOutcome::Uncertain;
                };
                remove_idle_artifact_directory(&mut registry, removed.directory);
                public_outcome
            }
            crate::path_identity::UnixTrackedCleanupOutcome::ClaimRetained { path, identity } => {
                if !operation.witness.matches(&operation.identity, &identity)
                    || !crate::path_identity::is_verified_descendant(
                        &identity,
                        &operation.directory.identity,
                    )
                {
                    return self.clear_unix_artifact_operation(
                        &operation,
                        TerminalSnapshotArtifactCleanupOutcome::Uncertain,
                    );
                }
                let Ok(mut registry) = self.artifacts.lock() else {
                    return TerminalSnapshotArtifactCleanupOutcome::Uncertain;
                };
                let Some(tracked) = registry.files.get_mut(&operation.object) else {
                    return TerminalSnapshotArtifactCleanupOutcome::Conflict;
                };
                if tracked.operation_generation != operation.token
                    || tracked.operation_owner != Some(operation.token)
                    || tracked.directory != operation.directory.identity.object_id
                {
                    return TerminalSnapshotArtifactCleanupOutcome::Conflict;
                }
                tracked.path = path;
                tracked.identity = identity;
                tracked.operation_owner = None;
                TerminalSnapshotArtifactCleanupOutcome::PrivateClaimRetained
            }
            crate::path_identity::UnixTrackedCleanupOutcome::SourceRetained => self
                .clear_unix_artifact_operation(
                    &operation,
                    TerminalSnapshotArtifactCleanupOutcome::SourceRetained,
                ),
            crate::path_identity::UnixTrackedCleanupOutcome::Uncertain => self
                .clear_unix_artifact_operation(
                    &operation,
                    TerminalSnapshotArtifactCleanupOutcome::Uncertain,
                ),
        }
    }

    #[cfg(unix)]
    fn cleanup_artifact_unix(
        &self,
        expected: &crate::path_identity::VerifiedPathIdentity,
    ) -> TerminalSnapshotArtifactCleanupOutcome {
        let operation = match self.begin_unix_artifact_operation(expected) {
            Ok(operation) => operation,
            Err(outcome) => return outcome,
        };
        #[cfg(test)]
        let outcome = if let Some(control) = self.take_unix_cleanup_control() {
            let TerminalSnapshotUnixCleanupControl {
                claim_leaf,
                mut hook,
            } = control;
            operation
                .directory
                .retained
                .cleanup_unix_tracked_file_with_hook(
                    &operation.path,
                    &operation.identity,
                    &operation.witness,
                    claim_leaf.as_deref(),
                    move |stage, source, claim| hook(stage, source, claim),
                )
        } else {
            operation.directory.retained.cleanup_unix_tracked_file(
                &operation.path,
                &operation.identity,
                &operation.witness,
            )
        };
        #[cfg(not(test))]
        let outcome = operation.directory.retained.cleanup_unix_tracked_file(
            &operation.path,
            &operation.identity,
            &operation.witness,
        );
        self.finish_unix_artifact_cleanup(operation, outcome)
    }

    pub(crate) fn cleanup_artifact(
        &self,
        path: &Path,
        expected: &crate::path_identity::VerifiedPathIdentity,
    ) -> TerminalSnapshotArtifactCleanupOutcome {
        #[cfg(unix)]
        {
            let _ = path;
            return self.cleanup_artifact_unix(expected);
        }
        #[cfg(not(unix))]
        {
            let retained = match self.artifacts.lock() {
                Ok(registry) => registry
                    .files
                    .get(&expected.object_id)
                    .and_then(|tracked| registry.directories.get(&tracked.directory))
                    .map(|directory| directory.retained.clone()),
                Err(_) => None,
            };
            let Some(retained) = retained else {
                return TerminalSnapshotArtifactCleanupOutcome::Conflict;
            };
            if retained.remove_regular_file_if_same(path, expected) {
                self.untrack_artifact(expected);
                return TerminalSnapshotArtifactCleanupOutcome::Removed;
            }
            match retained.verify_regular_file(path) {
                Ok(current) if crate::path_identity::same_object(expected, &current) => {
                    TerminalSnapshotArtifactCleanupOutcome::SourceRetained
                }
                _ => {
                    self.untrack_artifact(expected);
                    TerminalSnapshotArtifactCleanupOutcome::AlreadyAbsent
                }
            }
        }
    }

    #[cfg(unix)]
    fn relocate_artifact_unix(
        &self,
        expected: &crate::path_identity::VerifiedPathIdentity,
        path: &Path,
    ) -> Result<Option<crate::path_identity::VerifiedPathIdentity>, TerminalSnapshotReasonCode>
    {
        let operation = self
            .begin_unix_artifact_operation(expected)
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        if operation.witness.state(&operation.identity)
            == crate::path_identity::UnixFileWitnessState::Unlinked
        {
            let outcome = self.finish_unix_artifact_cleanup(
                operation,
                crate::path_identity::UnixTrackedCleanupOutcome::AlreadyAbsent,
            );
            return if outcome.confirmed_absent() {
                Ok(None)
            } else {
                Err(TerminalSnapshotReasonCode::ResponseUnavailable)
            };
        }
        if operation.witness.state(&operation.identity)
            != crate::path_identity::UnixFileWitnessState::Linked
        {
            self.clear_unix_artifact_operation(
                &operation,
                TerminalSnapshotArtifactCleanupOutcome::Uncertain,
            );
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        let current = match operation.directory.retained.verify_regular_file(path) {
            Ok(current)
                if operation.witness.matches(&operation.identity, &current)
                    && crate::path_identity::is_verified_descendant(
                        &current,
                        &operation.directory.identity,
                    ) =>
            {
                current
            }
            _ => {
                if operation.witness.state(&operation.identity)
                    == crate::path_identity::UnixFileWitnessState::Unlinked
                {
                    let outcome = self.finish_unix_artifact_cleanup(
                        operation,
                        crate::path_identity::UnixTrackedCleanupOutcome::AlreadyAbsent,
                    );
                    return if outcome.confirmed_absent() {
                        Ok(None)
                    } else {
                        Err(TerminalSnapshotReasonCode::ResponseUnavailable)
                    };
                }
                self.clear_unix_artifact_operation(
                    &operation,
                    TerminalSnapshotArtifactCleanupOutcome::SourceRetained,
                );
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            }
        };
        let mut registry = self
            .artifacts
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        let Some(tracked) = registry.files.get_mut(&operation.object) else {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        };
        if tracked.operation_generation != operation.token
            || tracked.operation_owner != Some(operation.token)
            || tracked.directory != operation.directory.identity.object_id
        {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        tracked.path = path.to_path_buf();
        tracked.identity = current.clone();
        tracked.operation_owner = None;
        Ok(Some(current))
    }

    pub(crate) fn relocate_artifact(
        &self,
        expected: &crate::path_identity::VerifiedPathIdentity,
        path: &Path,
    ) -> Result<Option<crate::path_identity::VerifiedPathIdentity>, TerminalSnapshotReasonCode>
    {
        #[cfg(unix)]
        {
            return self.relocate_artifact_unix(expected, path);
        }
        #[cfg(not(unix))]
        {
            let directory = {
                let registry = self
                    .artifacts
                    .lock()
                    .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
                let directory_object = registry
                    .files
                    .get(&expected.object_id)
                    .map(|tracked| tracked.directory)
                    .ok_or(TerminalSnapshotReasonCode::ResponseUnavailable)?;
                registry
                    .directories
                    .get(&directory_object)
                    .cloned()
                    .ok_or(TerminalSnapshotReasonCode::ResponseUnavailable)?
            };
            directory
                .retained
                .verify_current()
                .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
            let current = match directory.retained.verify_regular_file(path) {
                Ok(current) if crate::path_identity::same_object(expected, &current) => current,
                Err(_) if directory.retained.child_is_absent(path) => {
                    self.untrack_artifact(expected);
                    return Ok(None);
                }
                _ => return Err(TerminalSnapshotReasonCode::ResponseUnavailable),
            };
            if !crate::path_identity::is_verified_descendant(&current, &directory.identity) {
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            }
            let mut registry = self
                .artifacts
                .lock()
                .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
            let Some(tracked) = registry.files.get_mut(&expected.object_id) else {
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            };
            if tracked.directory != directory.identity.object_id {
                return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
            }
            tracked.path = path.to_path_buf();
            tracked.identity = current.clone();
            Ok(Some(current))
        }
    }

    fn sweep_artifacts(&self, force: bool) {
        #[cfg(unix)]
        {
            let files = match self.artifacts.lock() {
                Ok(registry) => registry
                    .files
                    .values()
                    .filter(|tracked| force || Instant::now() >= tracked.expires_at)
                    .map(|tracked| (tracked.path.clone(), tracked.identity.clone()))
                    .collect::<Vec<_>>(),
                Err(_) => return,
            };
            for (path, identity) in files {
                let _ = self.cleanup_artifact(&path, &identity);
            }
            return;
        }
        #[cfg(not(unix))]
        {
            let (files, directories) = match self.artifacts.lock() {
                Ok(registry) => (
                    registry.files.values().cloned().collect::<Vec<_>>(),
                    registry.directories.values().cloned().collect::<Vec<_>>(),
                ),
                Err(_) => return,
            };
            let now = Instant::now();
            let mut absent_files = Vec::new();
            for tracked in files {
                if !force && now < tracked.expires_at {
                    continue;
                }
                let Some(directory) = directories
                    .iter()
                    .find(|directory| directory.identity.object_id == tracked.directory)
                else {
                    absent_files.push(tracked.identity.object_id);
                    continue;
                };
                match directory.retained.verify_regular_file(&tracked.path) {
                    Ok(current)
                        if crate::path_identity::same_object(&current, &tracked.identity) =>
                    {
                        if directory
                            .retained
                            .remove_regular_file_if_same(&tracked.path, &tracked.identity)
                        {
                            absent_files.push(tracked.identity.object_id);
                        }
                    }
                    Ok(_) => {
                        absent_files.push(tracked.identity.object_id);
                    }
                    Err(_) if directory.retained.child_is_absent(&tracked.path) => {
                        absent_files.push(tracked.identity.object_id);
                    }
                    _ => {}
                }
            }
            let mut registry = match self.artifacts.lock() {
                Ok(registry) => registry,
                Err(_) => return,
            };
            for object in absent_files {
                registry.files.remove(&object);
            }
            let live_directories: std::collections::HashSet<_> = registry
                .files
                .values()
                .map(|file| file.directory)
                .chain(registry.directory_reservations.keys().copied())
                .collect();
            for directory in directories {
                if !live_directories.contains(&directory.identity.object_id) {
                    registry.directories.remove(&directory.identity.object_id);
                }
            }
        }
    }

    pub(crate) fn try_admit_ingress(
        &self,
        source_key: String,
    ) -> Result<OwnedSemaphorePermit, TerminalSnapshotReasonCode> {
        let now = Instant::now();
        {
            let mut limiter = self
                .limiter
                .lock()
                .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
            prune_map(&mut limiter.ingress, now);
            if !limiter.ingress.contains_key(&source_key)
                && limiter.ingress.len() >= SNAPSHOT_LIMITER_KEY_CAP
            {
                return Err(TerminalSnapshotReasonCode::RateLimited);
            }
            let window = limiter.ingress.entry(source_key).or_default();
            if window.len() >= SNAPSHOT_INGRESS_RATE {
                return Err(TerminalSnapshotReasonCode::RateLimited);
            }
            window.push_back(now);
        }
        Arc::clone(&self.ingress)
            .try_acquire_owned()
            .map_err(|_| TerminalSnapshotReasonCode::RateLimited)
    }

    fn admit_requester(
        &self,
        key: String,
    ) -> Result<RequesterSnapshotPermit, TerminalSnapshotReasonCode> {
        let now = Instant::now();
        let mut limiter = self
            .limiter
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        prune_map(&mut limiter.requester, now);
        if !limiter.requester.contains_key(&key)
            && limiter.requester.len() >= SNAPSHOT_LIMITER_KEY_CAP
        {
            return Err(TerminalSnapshotReasonCode::RateLimited);
        }
        let in_flight = limiter.requester_in_flight.get(&key).copied().unwrap_or(0);
        let attempts = limiter.requester.get(&key).map(VecDeque::len).unwrap_or(0);
        if attempts >= SNAPSHOT_REQUESTER_RATE
            || in_flight >= 1
            || limiter.global_in_flight >= SNAPSHOT_GLOBAL_IN_FLIGHT
        {
            return Err(TerminalSnapshotReasonCode::RateLimited);
        }
        limiter
            .requester
            .entry(key.clone())
            .or_default()
            .push_back(now);
        limiter.requester_in_flight.insert(key.clone(), 1);
        limiter.global_in_flight += 1;
        drop(limiter);
        Ok(RequesterSnapshotPermit {
            inner: Arc::new(LimiterLeaseInner {
                limiter: Arc::clone(&self.limiter),
                requester_key: key,
                target_key: Mutex::new(None),
            }),
        })
    }

    pub(crate) async fn pre_admit_requester(
        self: &Arc<Self>,
        context: &TerminalSnapshotServiceContext,
        requester_selector: TerminalSnapshotRequesterSelector,
        source_plane: TerminalSnapshotSourcePlane,
        host_authorization_deadline: Option<(Instant, chrono::DateTime<chrono::Utc>)>,
        audit: TerminalSnapshotAuditGuard,
    ) -> Result<TerminalSnapshotPreAdmission, TerminalSnapshotReasonCode> {
        self.pre_admit_requester_inner(
            context,
            requester_selector,
            source_plane,
            host_authorization_deadline,
            audit,
        )
        .await
    }

    async fn pre_admit_requester_inner(
        self: &Arc<Self>,
        context: &TerminalSnapshotServiceContext,
        requester_selector: TerminalSnapshotRequesterSelector,
        source_plane: TerminalSnapshotSourcePlane,
        host_authorization_deadline: Option<(Instant, chrono::DateTime<chrono::Utc>)>,
        audit: TerminalSnapshotAuditGuard,
    ) -> Result<TerminalSnapshotPreAdmission, TerminalSnapshotReasonCode> {
        if self.shutdown.is_cancelled() {
            return Err(TerminalSnapshotReasonCode::ServiceUnavailable);
        }
        let manager = clone_session_manager(&context.session_manager).await;
        let requester = prove_requester(
            &manager,
            &context.pty_manager,
            requester_selector,
            source_plane,
        )
        .await?;
        audit.accept_requester(&requester.identity.canonical_fqn);
        let accepted_at = Instant::now();
        let server_deadline = accepted_at
            .checked_add(SNAPSHOT_SERVER_TIMEOUT)
            .ok_or(TerminalSnapshotReasonCode::Internal)?;
        let (deadline, host_wall_deadline) = host_authorization_deadline.map_or(
            (server_deadline, None),
            |(host_deadline, wall_deadline)| {
                (server_deadline.min(host_deadline), Some(wall_deadline))
            },
        );
        let permit = self.admit_requester(authority_key(&requester.identity))?;
        ensure_before_deadline(deadline, &self.shutdown)?;
        Ok(TerminalSnapshotPreAdmission {
            state: Arc::clone(self),
            context: context.clone(),
            manager,
            requester,
            permit,
            audit,
            deadline,
            host_wall_deadline,
            source_plane,
        })
    }

    pub(crate) async fn prepare_with_admission(
        self: &Arc<Self>,
        admission: TerminalSnapshotPreAdmission,
        request: TerminalSnapshotServiceRequest,
    ) -> Result<TerminalSnapshotPrepared, TerminalSnapshotReasonCode> {
        self.prepare_inner(admission, request).await
    }

    async fn prepare_inner(
        self: &Arc<Self>,
        admission: TerminalSnapshotPreAdmission,
        request: TerminalSnapshotServiceRequest,
    ) -> Result<TerminalSnapshotPrepared, TerminalSnapshotReasonCode> {
        let TerminalSnapshotPreAdmission {
            state,
            context,
            manager,
            requester,
            permit,
            audit,
            deadline,
            host_wall_deadline,
            source_plane,
        } = admission;
        if !Arc::ptr_eq(self, &state) || source_plane != request.source_plane {
            return Err(TerminalSnapshotReasonCode::Internal);
        }
        terminal_snapshot_renderer::validate_uuid(&request.request_id.to_string(), Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        crate::config::teams::validate_terminal_snapshot_target_syntax(&request.target)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        audit.accept_request(&request);
        ensure_before_deadline(deadline, &self.shutdown)?;
        let security = await_deadline(
            deadline,
            crate::config::settings::read_terminal_snapshot_security_settings_strict_offloaded(),
        )
        .await
        .map_err(|reason| match reason {
            TerminalSnapshotReasonCode::SnapshotTimeout => reason,
            _ => TerminalSnapshotReasonCode::TerminalSnapshotsDisabled,
        })?
        .map_err(|_| TerminalSnapshotReasonCode::TerminalSnapshotsDisabled)?;
        let memory_enabled = await_deadline(deadline, context.settings.read())
            .await?
            .terminal_snapshots_enabled;
        if !security.terminal_snapshots_enabled || !memory_enabled {
            return Err(TerminalSnapshotReasonCode::TerminalSnapshotsDisabled);
        }

        let sender_cwd = requester.fact.working_directory.clone();
        let target = request.target.clone();
        let mut project_paths = security.project_paths;
        let sender_is_root = requester.fact.is_root_agent;
        if !sender_is_root {
            augment_coordinator_project(&mut project_paths, &requester.identity)?;
        }
        let route = run_blocking_with_deadline(
            self,
            TerminalSnapshotBlockingStage::RouteVerification,
            deadline,
            &permit,
            &audit,
            move || {
                crate::config::teams::verify_terminal_snapshot_route(
                    std::path::Path::new(&sender_cwd),
                    sender_is_root,
                    &target,
                    &project_paths,
                )
            },
        )
        .await?
        .map_err(|_| TerminalSnapshotReasonCode::NotAuthorized)?;
        if !same_authority(&requester.identity, &route.sender) {
            return Err(TerminalSnapshotReasonCode::NotAuthorized);
        }
        audit.accept_route(&route);
        permit.promote_target(authority_key(&route.target))?;

        if context.restore.0.load(Ordering::SeqCst)
            || context.purge.blocks_agent(&route.target.canonical_fqn)
        {
            return Err(TerminalSnapshotReasonCode::SnapshotUnavailable);
        }

        #[cfg(test)]
        self.test_state
            .target_session_lookups
            .fetch_add(1, Ordering::SeqCst);
        let facts = await_deadline(deadline, manager.terminal_snapshot_session_facts()).await??;
        let ids: Vec<Uuid> = facts.iter().map(|fact| fact.id).collect();
        let pty_manager = Arc::clone(&context.pty_manager);
        #[cfg(test)]
        self.test_state
            .target_route_lookups
            .fetch_add(1, Ordering::SeqCst);
        let proofs = run_blocking_with_deadline(
            self,
            TerminalSnapshotBlockingStage::RouteProofs,
            deadline,
            &permit,
            &audit,
            move || PtyManager::snapshot_route_proofs(&pty_manager, &ids),
        )
        .await??;
        let target_root = route.target.replica_root.clone();
        let target_identity = route.target.replica_identity.clone();
        let selected = run_blocking_with_deadline(
            self,
            TerminalSnapshotBlockingStage::SessionSelection,
            deadline,
            &permit,
            &audit,
            move || select_target_session(facts, proofs, &target_root, &target_identity),
        )
        .await??;
        if context.purge.blocks_session(selected.fact.id) {
            return Err(TerminalSnapshotReasonCode::SnapshotUnavailable);
        }
        audit.accept_selected(&selected.fact);
        #[cfg(test)]
        if source_plane == TerminalSnapshotSourcePlane::ContainerApi {
            self.run_api_before_capture_hook().await;
        }

        let capture_kind = selected.fact.backend_kind;
        let capture_cwd = selected.cwd_identity.clone();
        let capture_replica = route.target.replica_identity.clone();
        let selected = run_blocking_with_deadline(
            self,
            TerminalSnapshotBlockingStage::Capture,
            deadline,
            &permit,
            &audit,
            move || {
                let read =
                    selected
                        .proof
                        .capture_verified(capture_kind, &capture_cwd, &capture_replica);
                (selected, read)
            },
        )
        .await?;
        let (selected, model) = match selected {
            (selected, TerminalScreenRead::Captured(model)) => (selected, model),
            (_, TerminalScreenRead::TooLarge) => {
                return Err(TerminalSnapshotReasonCode::SnapshotTooLarge)
            }
            (_, TerminalScreenRead::Unavailable) => {
                return Err(TerminalSnapshotReasonCode::SnapshotUnavailable)
            }
        };
        audit.accept_model(&model);

        let requester_fqn = route.sender.canonical_fqn.clone();
        let target_fqn = route.target.canonical_fqn.clone();
        let request_id = request.request_id.to_string();
        let format = request.format;
        let model_for_build = Arc::clone(&model);
        let payload_stage = match format {
            TerminalSnapshotFormat::Json => TerminalSnapshotBlockingStage::JsonPayload,
            TerminalSnapshotFormat::Png => TerminalSnapshotBlockingStage::PngPayload,
        };
        let (payload, payload_bytes) =
            run_blocking_with_deadline(self, payload_stage, deadline, &permit, &audit, move || {
                let payload = build_payload(
                    format,
                    request_id,
                    requester_fqn,
                    target_fqn,
                    model_for_build,
                )?;
                let payload_bytes = payload.payload_bytes().map_err(|error| match error {
                    terminal_snapshot_renderer::ProtocolError::TooLarge => {
                        TerminalSnapshotReasonCode::SnapshotTooLarge
                    }
                    _ => TerminalSnapshotReasonCode::Internal,
                })?;
                Ok::<_, TerminalSnapshotReasonCode>((payload, payload_bytes))
            })
            .await??;
        audit.accept_payload(payload_bytes);
        if host_wall_deadline.is_some_and(|wall_deadline| chrono::Utc::now() >= wall_deadline) {
            return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
        }

        Ok(TerminalSnapshotPrepared {
            payload,
            finalization: TerminalSnapshotFinalization {
                state,
                context,
                manager,
                requester,
                route,
                selected,
                permit,
                audit,
                deadline,
                host_wall_deadline,
                #[cfg(test)]
                host_finalizer_control: if source_plane == TerminalSnapshotSourcePlane::HostCli {
                    self.take_next_host_finalizer_control()
                } else {
                    None
                },
            },
        })
    }
}

async fn clone_session_manager(state: &Arc<tokio::sync::RwLock<SessionManager>>) -> SessionManager {
    let guard = state.read().await;
    guard.clone()
}

struct RequesterProof {
    fact: TerminalSnapshotRequesterFact,
    identity: VerifiedPtyInputIdentity,
    cwd_identity: crate::path_identity::VerifiedPathIdentity,
    route: PtySnapshotRouteProof,
    host_token: Option<Uuid>,
}

async fn prove_requester(
    manager: &SessionManager,
    pty_manager: &Arc<std::sync::Mutex<PtyManager>>,
    selector: TerminalSnapshotRequesterSelector,
    source_plane: TerminalSnapshotSourcePlane,
) -> Result<RequesterProof, TerminalSnapshotReasonCode> {
    let (fact, host_confinement, host_token) = match selector {
        TerminalSnapshotRequesterSelector::Host {
            token,
            expected_root,
            claimed_from,
        } => (
            manager
                .find_unique_live_snapshot_requester_by_token(token)
                .await
                .map_err(|_| TerminalSnapshotReasonCode::RequesterUnavailable)?,
            Some((expected_root, claimed_from)),
            Some(token),
        ),
        TerminalSnapshotRequesterSelector::ApiSession(id) => (
            manager
                .live_snapshot_requester_by_id(id)
                .await
                .ok_or(TerminalSnapshotReasonCode::RequesterUnavailable)?,
            None,
            None,
        ),
    };
    let expected_backend = match source_plane {
        TerminalSnapshotSourcePlane::HostCli => SessionBackendKind::LocalProcess,
        TerminalSnapshotSourcePlane::ContainerApi => SessionBackendKind::ContainerTransport,
    };
    if fact.backend_kind != expected_backend {
        return Err(TerminalSnapshotReasonCode::RequesterUnavailable);
    }
    if (!fact.is_root_agent && !fact.is_coordinator)
        || (source_plane == TerminalSnapshotSourcePlane::ContainerApi && fact.is_root_agent)
    {
        return Err(TerminalSnapshotReasonCode::NotAuthorized);
    }
    let cwd = fact.working_directory.clone();
    let is_root = fact.is_root_agent;
    let identity_task = tokio::task::spawn_blocking(move || {
        crate::logging::catch_payload_unwind(move || {
            let cwd_identity = crate::path_identity::verify_directory(std::path::Path::new(&cwd))?;
            let identity = if is_root {
                let identity = crate::config::teams::verify_terminal_snapshot_root_identity(
                    std::path::Path::new(&cwd),
                )?;
                if !crate::path_identity::same_object(&identity.replica_identity, &cwd_identity) {
                    return Err("requester_identity_invalid".to_string());
                }
                identity
            } else {
                let identity = crate::config::teams::verify_pty_input_coordinator_root(
                    std::path::Path::new(&cwd),
                )?;
                if !crate::path_identity::same_object(&identity.replica_identity, &cwd_identity) {
                    return Err("requester_identity_invalid".to_string());
                }
                identity
            };
            Ok::<_, String>((identity, cwd_identity))
        })
    })
    .await;
    let (identity, cwd_identity) = match crate::logging::collapse_payload_task(identity_task) {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => return Err(TerminalSnapshotReasonCode::RequesterUnavailable),
        Err(_) => {
            log::error!("[terminal-snapshot] stage=requester_task code=internal");
            return Err(TerminalSnapshotReasonCode::ServiceUnavailable);
        }
    };
    let route = PtyManager::snapshot_route_proof(pty_manager, fact.id)
        .map_err(|_| TerminalSnapshotReasonCode::RequesterUnavailable)?;
    if let Some((expected_root, claimed_from)) = host_confinement {
        if !crate::path_identity::same_object(&expected_root, &identity.replica_identity)
            || claimed_from != identity.canonical_fqn
        {
            return Err(TerminalSnapshotReasonCode::RequesterUnavailable);
        }
    }
    let expected_replica = if fact.is_root_agent {
        None
    } else {
        Some(&identity.replica_identity)
    };
    if route.backend_kind() != expected_backend
        || route.liveness() != ContextSessionLiveness::Live
        || !route.matches_requester_route(expected_backend, &cwd_identity, expected_replica)
    {
        return Err(TerminalSnapshotReasonCode::RequesterUnavailable);
    }
    Ok(RequesterProof {
        fact,
        identity,
        cwd_identity,
        route,
        host_token,
    })
}

fn augment_coordinator_project(
    project_paths: &mut Vec<String>,
    requester: &VerifiedPtyInputIdentity,
) -> Result<(), TerminalSnapshotReasonCode> {
    let project = requester
        .ac_root_identity
        .canonical_path
        .parent()
        .and_then(Path::to_str)
        .map(crate::path_utils::normalize_windows_verbatim_path)
        .ok_or(TerminalSnapshotReasonCode::ServiceUnavailable)?;
    if !project_paths.contains(&project) {
        if project_paths.len() >= 4_096 {
            return Err(TerminalSnapshotReasonCode::ServiceUnavailable);
        }
        project_paths.push(project);
    }
    Ok(())
}

fn same_authority(left: &VerifiedPtyInputIdentity, right: &VerifiedPtyInputIdentity) -> bool {
    left.canonical_fqn == right.canonical_fqn
        && crate::path_identity::same_object(&left.replica_identity, &right.replica_identity)
        && left.authority_fingerprint == right.authority_fingerprint
}

fn authority_key(identity: &VerifiedPtyInputIdentity) -> String {
    format!(
        "{:016x}:{:016x}:{}",
        identity.replica_identity.object_id.volume,
        identity.replica_identity.object_id.file,
        identity.incarnation_fingerprint
    )
}

struct SelectedSession {
    fact: TerminalSnapshotSessionFact,
    cwd_identity: crate::path_identity::VerifiedPathIdentity,
    proof: PtySnapshotRouteProof,
}

fn select_target_session(
    facts: Vec<TerminalSnapshotSessionFact>,
    proofs: Vec<Option<PtySnapshotRouteProof>>,
    target_root: &Path,
    target_identity: &crate::path_identity::VerifiedPathIdentity,
) -> Result<SelectedSession, TerminalSnapshotReasonCode> {
    if facts.len() != proofs.len() {
        return Err(TerminalSnapshotReasonCode::Internal);
    }
    let mut eligible = Vec::new();
    let mut unavailable = false;
    for (fact, proof) in facts.into_iter().zip(proofs) {
        if matches!(fact.status, SessionStatus::Exited(_))
            || fact.name.starts_with(TEMP_SESSION_PREFIX)
        {
            continue;
        }
        let lexical_target = std::path::Path::new(&fact.working_directory).starts_with(target_root);
        let cwd_identity = match crate::path_identity::verify_directory(std::path::Path::new(
            &fact.working_directory,
        )) {
            Ok(identity)
                if crate::path_identity::is_verified_descendant(&identity, target_identity) =>
            {
                identity
            }
            Ok(_) => continue,
            Err(_) => {
                if lexical_target {
                    unavailable = true;
                }
                continue;
            }
        };
        let Some(proof) = proof else {
            unavailable = true;
            continue;
        };
        let route_matches = proof.backend_kind() == fact.backend_kind
            && crate::path_identity::same_object(proof.saved_cwd(), &cwd_identity)
            && proof
                .saved_replica()
                .is_some_and(|replica| crate::path_identity::same_object(replica, target_identity));
        if !route_matches || proof.liveness() != ContextSessionLiveness::Live {
            unavailable = true;
            continue;
        }
        eligible.push(SelectedSession {
            fact,
            cwd_identity,
            proof,
        });
    }
    eligible.sort_by(|left, right| {
        status_rank(&left.fact.status)
            .cmp(&status_rank(&right.fact.status))
            .then_with(|| right.fact.created_at.cmp(&left.fact.created_at))
            .then_with(|| left.fact.id.as_bytes().cmp(right.fact.id.as_bytes()))
    });
    if let Some(selected) = eligible.into_iter().next() {
        Ok(selected)
    } else if unavailable {
        Err(TerminalSnapshotReasonCode::SnapshotUnavailable)
    } else {
        Err(TerminalSnapshotReasonCode::TargetUnavailable)
    }
}

fn status_rank(status: &SessionStatus) -> u8 {
    match status {
        SessionStatus::Active => 0,
        SessionStatus::Running => 1,
        SessionStatus::Idle => 2,
        SessionStatus::Exited(_) => 3,
    }
}

fn build_payload(
    format: TerminalSnapshotFormat,
    request_id: String,
    requester: String,
    target: String,
    model: Arc<TerminalScreenModel>,
) -> Result<PreparedSnapshotPayload, TerminalSnapshotReasonCode> {
    match format {
        TerminalSnapshotFormat::Json => {
            terminal_snapshot_json_bytes_from_model(&request_id, &requester, &target, &model)
                .map_err(|error| match error {
                    terminal_snapshot_renderer::ProtocolError::TooLarge => {
                        TerminalSnapshotReasonCode::SnapshotTooLarge
                    }
                    _ => TerminalSnapshotReasonCode::Internal,
                })?;
            Ok(PreparedSnapshotPayload::Json {
                request_id,
                requester,
                target,
                model,
            })
        }
        TerminalSnapshotFormat::Png => {
            let rendered = render_png(&model).map_err(|error| match error {
                terminal_snapshot_renderer::RenderError::TooLarge => {
                    TerminalSnapshotReasonCode::SnapshotTooLarge
                }
                _ => TerminalSnapshotReasonCode::RenderFailed,
            })?;
            let metadata = rendered.metadata(request_id, requester, target, &model);
            Ok(PreparedSnapshotPayload::Png(Box::new(
                TerminalSnapshotPayload::Png {
                    metadata,
                    png: rendered.bytes,
                },
            )))
        }
    }
}

impl TerminalSnapshotFinalization {
    #[cfg(test)]
    pub(crate) fn install_host_finalizer_control(
        &mut self,
        control: Arc<TerminalSnapshotHostFinalizerControl>,
    ) {
        self.host_finalizer_control = Some(control);
    }

    #[cfg(test)]
    fn run_host_finalizer_control(&self, stage: TerminalSnapshotHostFinalizerStage) {
        if let Some(control) = self.host_finalizer_control.as_ref() {
            control.enter_stage(stage);
        }
    }

    pub(crate) async fn build_api_response(
        &self,
        payload: PreparedSnapshotPayload,
    ) -> Result<Vec<u8>, TerminalSnapshotReasonCode> {
        #[cfg(test)]
        self.state.mark_blocking_payload_retention(
            TerminalSnapshotBlockingStage::ApiEnvelope,
            payload.retained_content_bytes(),
        );
        run_blocking_with_deadline(
            &self.state,
            TerminalSnapshotBlockingStage::ApiEnvelope,
            self.deadline,
            &self.permit,
            &self.audit,
            move || payload.encode_api().map_err(map_envelope_error),
        )
        .await?
    }

    pub(crate) async fn build_host_response(
        &self,
        payload: PreparedSnapshotPayload,
        request_id: String,
        confirmation_tag: String,
        expires_at: String,
    ) -> Result<Vec<u8>, TerminalSnapshotReasonCode> {
        #[cfg(test)]
        self.state.mark_blocking_payload_retention(
            TerminalSnapshotBlockingStage::HostEnvelope,
            payload.retained_content_bytes(),
        );
        run_blocking_with_deadline(
            &self.state,
            TerminalSnapshotBlockingStage::HostEnvelope,
            self.deadline,
            &self.permit,
            &self.audit,
            move || {
                payload
                    .encode_host(&request_id, &confirmation_tag, &expires_at)
                    .map_err(map_envelope_error)
            },
        )
        .await?
    }

    pub(crate) async fn revalidate_api(
        self,
    ) -> Result<TerminalSnapshotDisclosure, TerminalSnapshotReasonCode> {
        if self.host_wall_deadline.is_some() {
            self.audit
                .finalize_failure(TerminalSnapshotReasonCode::Internal);
            return Err(TerminalSnapshotReasonCode::Internal);
        }
        if let Err(reason) = final_revalidate_async(&self).await {
            self.audit.finalize_failure(reason);
            return Err(reason);
        }
        Ok(TerminalSnapshotDisclosure {
            state: self.state,
            permit: self.permit,
            audit: self.audit,
            deadline: self.deadline,
        })
    }

    pub(crate) fn fail_host<F>(
        self,
        reason: TerminalSnapshotReasonCode,
        publish: F,
    ) -> Result<(), TerminalSnapshotReasonCode>
    where
        F: FnOnce(TerminalSnapshotReasonCode) -> Result<(), TerminalSnapshotReasonCode>,
    {
        let result = match publish(reason) {
            Ok(()) => Err(reason),
            Err(_) => Err(TerminalSnapshotReasonCode::ResponseUnavailable),
        };
        if let Err(final_reason) = result {
            self.audit.finalize_failure(final_reason);
        }
        drop(self.permit);
        result
    }

    pub(crate) fn finalize_host<F>(
        self,
        success_bytes: Vec<u8>,
        publish: F,
    ) -> Result<(), TerminalSnapshotReasonCode>
    where
        F: FnOnce(
            Result<Vec<u8>, TerminalSnapshotReasonCode>,
        ) -> Result<(), TerminalSnapshotReasonCode>,
    {
        #[cfg(test)]
        if let Some(control) = self.host_finalizer_control.as_ref() {
            control.set_retained_response_bytes(success_bytes.len());
        }
        #[cfg(test)]
        self.state.run_host_final_handoff_hook();
        #[cfg(test)]
        self.run_host_finalizer_control(TerminalSnapshotHostFinalizerStage::RevalidationEntry);
        let result =
            finalize_host_publication(success_bytes, final_revalidate_blocking(&self), publish);
        match result {
            Ok(()) => self.audit.finalize("succeeded", None),
            Err(reason) => self.audit.finalize_failure(reason),
        }
        drop(self.permit);
        result
    }
}

fn finalize_host_publication<F>(
    success_bytes: Vec<u8>,
    authority: Result<(), TerminalSnapshotReasonCode>,
    publish: F,
) -> Result<(), TerminalSnapshotReasonCode>
where
    F: FnOnce(
        Result<Vec<u8>, TerminalSnapshotReasonCode>,
    ) -> Result<(), TerminalSnapshotReasonCode>,
{
    match authority {
        Ok(()) => publish(Ok(success_bytes)),
        Err(reason) => match publish(Err(reason)) {
            Ok(()) => Err(reason),
            Err(_) => Err(TerminalSnapshotReasonCode::ResponseUnavailable),
        },
    }
}

async fn final_revalidate_async(
    finalization: &TerminalSnapshotFinalization,
) -> Result<(), TerminalSnapshotReasonCode> {
    let state = &finalization.state;
    let context = &finalization.context;
    let manager = &finalization.manager;
    let deadline = finalization.deadline;
    let permit = &finalization.permit;
    let audit = &finalization.audit;
    let requester = &finalization.requester;
    let route = &finalization.route;
    let selected = &finalization.selected;
    ensure_before_deadline(deadline, &state.shutdown)?;
    let security = await_deadline(
        deadline,
        crate::config::settings::read_terminal_snapshot_security_settings_strict_offloaded(),
    )
    .await?
    .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    let memory_enabled = await_deadline(deadline, context.settings.read())
        .await?
        .terminal_snapshots_enabled;
    if !security.terminal_snapshots_enabled
        || !memory_enabled
        || context.restore.0.load(Ordering::SeqCst)
        || context.purge.blocks_agent(&route.target.canonical_fqn)
        || context.purge.blocks_session(selected.fact.id)
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    let sender_cwd = requester.fact.working_directory.clone();
    let target_fqn = route.target.canonical_fqn.clone();
    let mut project_paths = security.project_paths;
    let sender_is_root = requester.fact.is_root_agent;
    if !sender_is_root {
        augment_coordinator_project(&mut project_paths, &requester.identity)
            .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    }
    let fresh_route = run_blocking_with_deadline(
        state,
        TerminalSnapshotBlockingStage::FinalVerification,
        deadline,
        permit,
        audit,
        move || {
            crate::config::teams::verify_terminal_snapshot_route(
                std::path::Path::new(&sender_cwd),
                sender_is_root,
                &target_fqn,
                &project_paths,
            )
        },
    )
    .await?
    .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    if !same_authority(&requester.identity, &fresh_route.sender)
        || !same_authority(&route.target, &fresh_route.target)
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    if let Some(token) = requester.host_token {
        let token_fact = await_deadline(
            deadline,
            manager.find_unique_live_snapshot_requester_by_token(token),
        )
        .await?
        .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
        if token_fact.id != requester.fact.id || token_fact.created_at != requester.fact.created_at
        {
            return Err(TerminalSnapshotReasonCode::AuthorityChanged);
        }
    }
    verify_current_requester_async(deadline, manager, requester).await?;
    verify_current_selected_async(deadline, manager, route, selected).await?;
    ensure_before_deadline(deadline, &state.shutdown)
}

fn final_revalidate_blocking(
    finalization: &TerminalSnapshotFinalization,
) -> Result<(), TerminalSnapshotReasonCode> {
    let context = &finalization.context;
    let manager = &finalization.manager;
    let requester = &finalization.requester;
    let route = &finalization.route;
    let selected = &finalization.selected;
    ensure_host_finalizer_before_deadline(finalization)?;
    let security = crate::config::settings::read_terminal_snapshot_security_settings_strict()
        .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    let memory_enabled = context.settings.blocking_read().terminal_snapshots_enabled;
    if !security.terminal_snapshots_enabled
        || !memory_enabled
        || context.restore.0.load(Ordering::SeqCst)
        || context.purge.blocks_agent(&route.target.canonical_fqn)
        || context.purge.blocks_session(selected.fact.id)
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    let mut project_paths = security.project_paths;
    if !requester.fact.is_root_agent {
        augment_coordinator_project(&mut project_paths, &requester.identity)
            .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    }
    #[cfg(test)]
    finalization.run_host_finalizer_control(TerminalSnapshotHostFinalizerStage::RouteVerification);
    let fresh_route = crate::config::teams::verify_terminal_snapshot_route(
        Path::new(&requester.fact.working_directory),
        requester.fact.is_root_agent,
        &route.target.canonical_fqn,
        &project_paths,
    )
    .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
    if !same_authority(&requester.identity, &fresh_route.sender)
        || !same_authority(&route.target, &fresh_route.target)
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    if let Some(token) = requester.host_token {
        let token_fact = manager
            .find_unique_live_snapshot_requester_by_token_blocking(token)
            .map_err(|_| TerminalSnapshotReasonCode::AuthorityChanged)?;
        if token_fact.id != requester.fact.id || token_fact.created_at != requester.fact.created_at
        {
            return Err(TerminalSnapshotReasonCode::AuthorityChanged);
        }
    }
    verify_current_requester_blocking(manager, requester)?;
    verify_current_selected_blocking(manager, route, selected)?;
    if finalization
        .host_wall_deadline
        .is_some_and(|wall_deadline| chrono::Utc::now() >= wall_deadline)
    {
        return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
    }
    #[cfg(test)]
    finalization.run_host_finalizer_control(TerminalSnapshotHostFinalizerStage::FinalDeadline);
    ensure_host_finalizer_before_deadline(finalization)
}

fn ensure_host_finalizer_before_deadline(
    finalization: &TerminalSnapshotFinalization,
) -> Result<(), TerminalSnapshotReasonCode> {
    #[cfg(test)]
    if finalization
        .host_finalizer_control
        .as_ref()
        .is_some_and(|control| control.deadline_expired())
    {
        return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
    }
    ensure_before_deadline(finalization.deadline, &finalization.state.shutdown)
}

fn map_envelope_error(
    error: terminal_snapshot_renderer::ProtocolError,
) -> TerminalSnapshotReasonCode {
    match error {
        terminal_snapshot_renderer::ProtocolError::TooLarge => {
            TerminalSnapshotReasonCode::SnapshotTooLarge
        }
        _ => TerminalSnapshotReasonCode::Internal,
    }
}

async fn verify_current_requester_async(
    deadline: Instant,
    manager: &SessionManager,
    requester: &RequesterProof,
) -> Result<(), TerminalSnapshotReasonCode> {
    let current = await_deadline(
        deadline,
        manager.live_snapshot_requester_by_id(requester.fact.id),
    )
    .await?
    .ok_or(TerminalSnapshotReasonCode::AuthorityChanged)?;
    verify_requester_fact(requester, &current)
}

fn verify_current_requester_blocking(
    manager: &SessionManager,
    requester: &RequesterProof,
) -> Result<(), TerminalSnapshotReasonCode> {
    let current = manager
        .live_snapshot_requester_by_id_blocking(requester.fact.id)
        .ok_or(TerminalSnapshotReasonCode::AuthorityChanged)?;
    verify_requester_fact(requester, &current)
}

fn verify_requester_fact(
    requester: &RequesterProof,
    current: &TerminalSnapshotRequesterFact,
) -> Result<(), TerminalSnapshotReasonCode> {
    if current.created_at != requester.fact.created_at
        || current.working_directory != requester.fact.working_directory
        || current.backend_kind != requester.fact.backend_kind
        || current.is_root_agent != requester.fact.is_root_agent
        || current.is_coordinator != requester.fact.is_coordinator
        || requester.route.liveness() != ContextSessionLiveness::Live
        || !requester.route.matches_requester_route(
            requester.fact.backend_kind,
            &requester.cwd_identity,
            if requester.fact.is_root_agent {
                None
            } else {
                Some(&requester.identity.replica_identity)
            },
        )
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    Ok(())
}

async fn verify_current_selected_async(
    deadline: Instant,
    manager: &SessionManager,
    route: &VerifiedTerminalSnapshotRoute,
    selected: &SelectedSession,
) -> Result<(), TerminalSnapshotReasonCode> {
    let current = await_deadline(
        deadline,
        manager.terminal_snapshot_session_fact_by_id(selected.fact.id),
    )
    .await?
    .ok_or(TerminalSnapshotReasonCode::AuthorityChanged)?;
    verify_selected_fact(route, selected, &current)
}

fn verify_current_selected_blocking(
    manager: &SessionManager,
    route: &VerifiedTerminalSnapshotRoute,
    selected: &SelectedSession,
) -> Result<(), TerminalSnapshotReasonCode> {
    let current = manager
        .terminal_snapshot_session_fact_by_id_blocking(selected.fact.id)
        .ok_or(TerminalSnapshotReasonCode::AuthorityChanged)?;
    verify_selected_fact(route, selected, &current)
}

fn verify_selected_fact(
    route: &VerifiedTerminalSnapshotRoute,
    selected: &SelectedSession,
    current: &TerminalSnapshotSessionFact,
) -> Result<(), TerminalSnapshotReasonCode> {
    if current.created_at != selected.fact.created_at
        || current.working_directory != selected.fact.working_directory
        || current.backend_kind != selected.fact.backend_kind
        || current.name != selected.fact.name
        || current.name.starts_with(TEMP_SESSION_PREFIX)
        || matches!(current.status, SessionStatus::Exited(_))
        || selected.proof.liveness() != ContextSessionLiveness::Live
        || !selected.proof.matches_current(
            selected.fact.backend_kind,
            &selected.cwd_identity,
            &route.target.replica_identity,
        )
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    Ok(())
}

fn ensure_before_deadline(
    deadline: Instant,
    shutdown: &crate::shutdown::ShutdownSignal,
) -> Result<(), TerminalSnapshotReasonCode> {
    if shutdown.is_cancelled() {
        return Err(TerminalSnapshotReasonCode::ServiceUnavailable);
    }
    if Instant::now() >= deadline {
        return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
    }
    Ok(())
}

async fn await_deadline<F, T>(deadline: Instant, future: F) -> Result<T, TerminalSnapshotReasonCode>
where
    F: std::future::Future<Output = T>,
{
    let remaining = deadline.saturating_duration_since(Instant::now());
    tokio::time::timeout(remaining, future)
        .await
        .map_err(|_| TerminalSnapshotReasonCode::SnapshotTimeout)
}

async fn run_blocking_with_deadline<F, T>(
    state: &TerminalSnapshotState,
    stage: TerminalSnapshotBlockingStage,
    deadline: Instant,
    permit: &RequesterSnapshotPermit,
    audit: &TerminalSnapshotAuditGuard,
    operation: F,
) -> Result<T, TerminalSnapshotReasonCode>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    #[cfg(test)]
    let control = state.take_blocking_control(stage);
    #[cfg(not(test))]
    let _ = (state, stage);

    let permit = permit.clone();
    let audit = audit.clone();
    #[cfg(test)]
    let worker_control = control.clone();
    #[cfg(test)]
    let completion_control = control.clone();
    #[cfg(test)]
    let mut handle = tokio::task::spawn_blocking(move || {
        let outcome = crate::logging::catch_payload_unwind(move || {
            let _permit = permit;
            let _audit = audit;
            if let Some(control) = worker_control {
                control.enter_worker();
            }
            operation()
        });
        TerminalSnapshotTestTaskOutput {
            outcome: Some(outcome),
            control: completion_control,
        }
    });
    #[cfg(not(test))]
    let mut handle = tokio::task::spawn_blocking(move || {
        crate::logging::catch_payload_unwind(move || {
            let _permit = permit;
            let _audit = audit;
            operation()
        })
    });

    #[cfg(test)]
    let joined = if let Some(control) = control {
        let remaining = deadline.saturating_duration_since(Instant::now());
        tokio::select! {
            joined = &mut handle => joined,
            _ = tokio::time::sleep(remaining) => {
                return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
            }
            _ = control.deadline_expired() => {
                return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
            }
        }
    } else {
        await_deadline(deadline, &mut handle).await?
    };
    #[cfg(not(test))]
    let joined = await_deadline(deadline, &mut handle).await?;
    #[cfg(test)]
    let joined = joined.map(TerminalSnapshotTestTaskOutput::into_outcome);

    match crate::logging::collapse_payload_task(joined) {
        Ok(value) => Ok(value),
        Err(_) => {
            log::error!("[terminal-snapshot] stage=blocking_task code=internal");
            Err(TerminalSnapshotReasonCode::Internal)
        }
    }
}

struct AuditInner {
    finalized: AtomicBool,
    metadata: Mutex<crate::api::audit::TerminalSnapshotAuditMetadata>,
}

impl Drop for AuditInner {
    fn drop(&mut self) {
        if self.finalized.swap(true, Ordering::SeqCst) {
            return;
        }
        let Ok(mut metadata) = self.metadata.lock() else {
            return;
        };
        metadata.completed_at = terminal_snapshot_renderer::canonical_timestamp(chrono::Utc::now());
        metadata.status = "failed".to_string();
        metadata.reason_code = Some("internal".to_string());
        crate::api::audit::record_terminal_snapshot(&metadata);
    }
}

#[derive(Clone)]
pub(crate) struct TerminalSnapshotAuditGuard {
    inner: Arc<AuditInner>,
}

impl TerminalSnapshotAuditGuard {
    pub(crate) fn pre_admission(source_plane: TerminalSnapshotSourcePlane) -> Self {
        Self {
            inner: Arc::new(AuditInner {
                finalized: AtomicBool::new(false),
                metadata: Mutex::new(crate::api::audit::TerminalSnapshotAuditMetadata {
                    event: "terminal_snapshot".to_string(),
                    request_id: None,
                    requester_fqn: None,
                    target_fqn: None,
                    source_plane: source_plane.as_str().to_string(),
                    format: None,
                    selected_session_id: None,
                    selected_backend: None,
                    rows: None,
                    columns: None,
                    sequence: None,
                    captured_at: None,
                    payload_bytes: None,
                    accepted_at: None,
                    completed_at: String::new(),
                    status: "failed".to_string(),
                    reason_code: None,
                }),
            }),
        }
    }

    pub(crate) fn accept_request(&self, request: &TerminalSnapshotServiceRequest) {
        self.update(|metadata| {
            metadata.request_id = Some(request.request_id.to_string());
            metadata.format = Some(request.format.to_string());
        });
    }

    fn update(&self, update: impl FnOnce(&mut crate::api::audit::TerminalSnapshotAuditMetadata)) {
        if let Ok(mut metadata) = self.inner.metadata.lock() {
            update(&mut metadata);
        }
    }

    fn accept_requester(&self, requester: &str) {
        self.update(|metadata| {
            metadata.requester_fqn = Some(requester.to_string());
            metadata.accepted_at = Some(terminal_snapshot_renderer::canonical_timestamp(
                chrono::Utc::now(),
            ));
        });
    }

    fn accept_route(&self, route: &VerifiedTerminalSnapshotRoute) {
        self.update(|metadata| metadata.target_fqn = Some(route.target.canonical_fqn.clone()));
    }

    fn accept_selected(&self, fact: &TerminalSnapshotSessionFact) {
        self.update(|metadata| {
            metadata.selected_session_id = Some(fact.id.to_string());
            metadata.selected_backend = Some(match fact.backend_kind {
                SessionBackendKind::LocalProcess => "localProcess".to_string(),
                SessionBackendKind::ContainerTransport => "containerTransport".to_string(),
            });
        });
    }

    fn accept_model(&self, model: &TerminalScreenModel) {
        self.update(|metadata| {
            metadata.rows = Some(model.screen.dimensions.rows);
            metadata.columns = Some(model.screen.dimensions.columns);
            metadata.sequence = Some(model.screen.sequence);
            metadata.captured_at = Some(model.captured_at.clone());
        });
    }

    fn accept_payload(&self, payload_bytes: u64) {
        self.update(|metadata| metadata.payload_bytes = Some(payload_bytes));
    }

    pub(crate) async fn wait_for_retained_owners(&self, expected_owners: usize) {
        debug_assert!(expected_owners > 0);
        while Arc::strong_count(&self.inner) > expected_owners {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    pub(crate) fn finalize_failure(&self, reason: TerminalSnapshotReasonCode) {
        let status = match reason {
            TerminalSnapshotReasonCode::TerminalSnapshotsDisabled
            | TerminalSnapshotReasonCode::NotAuthorized
            | TerminalSnapshotReasonCode::InvalidRequest
            | TerminalSnapshotReasonCode::RequesterUnavailable => "rejected",
            _ => "failed",
        };
        self.finalize(status, Some(reason.as_str()));
    }

    fn finalize(&self, status: &str, reason: Option<&str>) {
        if self.inner.finalized.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Ok(mut metadata) = self.inner.metadata.lock() {
            metadata.completed_at =
                terminal_snapshot_renderer::canonical_timestamp(chrono::Utc::now());
            metadata.status = status.to_string();
            metadata.reason_code = reason.map(str::to_string);
            crate::api::audit::record_terminal_snapshot(&metadata);
        }
    }
}

impl From<crate::session::manager::TerminalSnapshotFactsError> for TerminalSnapshotReasonCode {
    fn from(_: crate::session::manager::TerminalSnapshotFactsError) -> Self {
        TerminalSnapshotReasonCode::SnapshotUnavailable
    }
}

impl From<crate::errors::AppError> for TerminalSnapshotReasonCode {
    fn from(_: crate::errors::AppError) -> Self {
        TerminalSnapshotReasonCode::SnapshotUnavailable
    }
}

#[cfg(test)]
mod acceptance_tests;
#[cfg(test)]
mod resource_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_request_selector_and_prepared_payload_debug_are_structural_only() {
        const CELL_CANARY: &str = "CELL_1173_SVC_F4R6";
        const PNG_CANARY: &str = "PNG_1173_SVC_F4R6";
        const AUTH_CANARY: &str = "AUTH_1173_SVC_F4R6";
        const PATH_CANARY: &str = r"C:\PATH_1173_SVC_F4R6\snapshot.png";
        let request_id = Uuid::parse_str("11730000-0000-4000-8000-00000000f406").unwrap();
        let request = TerminalSnapshotServiceRequest {
            request_id,
            target: PATH_CANARY.to_string(),
            format: TerminalSnapshotFormat::Json,
            source_plane: TerminalSnapshotSourcePlane::HostCli,
            host_authorization_deadline: Some((Instant::now(), chrono::Utc::now())),
        };
        let temporary = tempfile::TempDir::new().unwrap();
        let expected_root = crate::path_identity::verify_directory(temporary.path()).unwrap();
        let selector = TerminalSnapshotRequesterSelector::Host {
            token: request_id,
            expected_root,
            claimed_from: AUTH_CANARY.to_string(),
        };
        let api_selector = TerminalSnapshotRequesterSelector::ApiSession(request_id);

        let mut model: TerminalScreenModel = terminal_snapshot_renderer::decode_bounded(
            include_bytes!(
                "../../../crates/terminal-snapshot-renderer/tests/fixtures/blank-cursor-model.json"
            ),
            terminal_snapshot_renderer::MAX_JSON_BYTES,
        )
        .unwrap();
        model.screen.lines[0].cells[0].text = CELL_CANARY.to_string();
        let json = PreparedSnapshotPayload::Json {
            request_id: AUTH_CANARY.to_string(),
            requester: AUTH_CANARY.to_string(),
            target: PATH_CANARY.to_string(),
            model: Arc::new(model),
        };
        let png_model: TerminalScreenModel = terminal_snapshot_renderer::decode_bounded(
            include_bytes!(
                "../../../crates/terminal-snapshot-renderer/tests/fixtures/blank-cursor-model.json"
            ),
            terminal_snapshot_renderer::MAX_JSON_BYTES,
        )
        .unwrap();
        let rendered = render_png(&png_model).unwrap();
        let png = PreparedSnapshotPayload::Png(Box::new(TerminalSnapshotPayload::Png {
            metadata: rendered.metadata(
                AUTH_CANARY.to_string(),
                AUTH_CANARY.to_string(),
                PATH_CANARY.to_string(),
                &png_model,
            ),
            png: PNG_CANARY.as_bytes().to_vec(),
        }));

        let diagnostic = format!("{request:?}\n{selector:?}\n{api_selector:?}\n{json:?}\n{png:?}");
        let request_id_text = request_id.to_string();
        let temporary_path = temporary.path().to_string_lossy().into_owned();
        for forbidden in [
            CELL_CANARY,
            PNG_CANARY,
            AUTH_CANARY,
            PATH_CANARY,
            request_id_text.as_str(),
            temporary_path.as_str(),
        ] {
            assert!(!diagnostic.contains(forbidden));
        }
        for structural in [
            "source_plane: HostCli",
            "has_host_authorization_deadline: true",
            "TerminalSnapshotRequesterSelector::Host",
            "TerminalSnapshotRequesterSelector::ApiSession",
            "PreparedSnapshotPayload::Json",
            "PreparedSnapshotPayload::Png",
            "decoded_bytes",
        ] {
            assert!(diagnostic.contains(structural));
        }
    }

    #[test]
    fn status_order_is_exact() {
        assert!(status_rank(&SessionStatus::Active) < status_rank(&SessionStatus::Running));
        assert!(status_rank(&SessionStatus::Running) < status_rank(&SessionStatus::Idle));
        assert!(status_rank(&SessionStatus::Idle) < status_rank(&SessionStatus::Exited(0)));
    }

    #[test]
    fn temporary_only_target_is_unavailable_even_when_its_cwd_vanished() {
        let target = tempfile::TempDir::new().unwrap();
        let target_identity = crate::path_identity::verify_directory(target.path()).unwrap();
        let fact = TerminalSnapshotSessionFact {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            name: format!("{TEMP_SESSION_PREFIX}only"),
            status: SessionStatus::Running,
            working_directory: target.path().join("missing").to_string_lossy().to_string(),
            backend_kind: SessionBackendKind::LocalProcess,
        };
        let result = select_target_session(vec![fact], vec![None], target.path(), &target_identity);
        assert!(matches!(
            result,
            Err(TerminalSnapshotReasonCode::TargetUnavailable)
        ));
    }

    #[test]
    fn artifact_registry_removes_only_the_tracked_object() {
        let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
        let directory = tempfile::TempDir::new().unwrap();
        let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
        let reservation = state
            .reserve_artifact(directory.path(), &directory_identity)
            .unwrap();
        let path = directory.path().join(format!("{}.json", Uuid::new_v4()));
        std::fs::write(&path, b"secret").unwrap();
        let identity = crate::path_identity::verify_regular_file(&path).unwrap();
        reservation.commit(path.clone(), identity.clone()).unwrap();
        {
            let mut registry = state.artifacts.lock().unwrap();
            registry
                .files
                .get_mut(&identity.object_id)
                .unwrap()
                .expires_at = Instant::now();
        }
        state.sweep_artifacts(false);
        assert!(!path.exists());
        assert!(!state
            .artifacts
            .lock()
            .unwrap()
            .files
            .contains_key(&identity.object_id));
    }

    #[cfg(not(unix))]
    #[test]
    fn artifact_registry_releases_a_displaced_object_without_removing_its_replacement() {
        let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
        let directory = tempfile::TempDir::new().unwrap();
        let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
        let reservation = state
            .reserve_artifact(directory.path(), &directory_identity)
            .unwrap();
        let path = directory.path().join(format!("{}.json", Uuid::new_v4()));
        let displaced = directory.path().join("displaced-tracked-response");
        std::fs::write(&path, b"tracked response").unwrap();
        let identity = crate::path_identity::verify_regular_file(&path).unwrap();
        reservation.commit(path.clone(), identity.clone()).unwrap();
        std::fs::rename(&path, &displaced).unwrap();
        std::fs::write(&path, b"replacement response").unwrap();
        {
            let mut registry = state.artifacts.lock().unwrap();
            registry
                .files
                .get_mut(&identity.object_id)
                .unwrap()
                .expires_at = Instant::now();
        }

        state.sweep_artifacts(false);
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement response");
        assert_eq!(std::fs::read(&displaced).unwrap(), b"tracked response");
        let registry = state.artifacts.lock().unwrap();
        assert!(!registry.files.contains_key(&identity.object_id));
        assert!(registry.directories.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn artifact_registry_retains_displaced_object_until_witness_proves_absence() {
        let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
        let directory = tempfile::TempDir::new().unwrap();
        let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
        let reservation = state
            .reserve_artifact(directory.path(), &directory_identity)
            .unwrap();
        let path = directory.path().join(format!("{}.json", Uuid::new_v4()));
        let displaced = directory.path().join("displaced-tracked-response");
        std::fs::write(&path, b"tracked response").unwrap();
        let identity = crate::path_identity::verify_regular_file(&path).unwrap();
        reservation.commit(path.clone(), identity.clone()).unwrap();
        std::fs::rename(&path, &displaced).unwrap();
        std::fs::write(&path, b"replacement response").unwrap();

        state.sweep_artifacts(true);
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement response");
        assert_eq!(std::fs::read(&displaced).unwrap(), b"tracked response");
        assert_eq!(state.test_artifact_counts(), (1, 1, 0));

        std::fs::remove_file(&displaced).unwrap();
        state.sweep_artifacts(true);
        assert_eq!(std::fs::read(path).unwrap(), b"replacement response");
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[cfg(unix)]
    #[test]
    fn artifact_cleanup_reclaims_once_only_after_witness_confirmed_removal() {
        let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
        let directory = tempfile::TempDir::new().unwrap();
        let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
        let reservation = state
            .reserve_artifact(directory.path(), &directory_identity)
            .unwrap();
        let path = directory.path().join("tracked.json");
        std::fs::write(&path, b"tracked response").unwrap();
        let identity = crate::path_identity::verify_regular_file(&path).unwrap();
        reservation.commit(path.clone(), identity.clone()).unwrap();

        assert_eq!(
            state.cleanup_artifact(&path, &identity),
            TerminalSnapshotArtifactCleanupOutcome::Removed
        );
        assert!(!path.exists());
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
        assert_eq!(
            state.cleanup_artifact(&path, &identity),
            TerminalSnapshotArtifactCleanupOutcome::Conflict
        );
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[cfg(unix)]
    #[test]
    fn artifact_cleanup_claim_collision_retains_entry_and_capacity() {
        let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
        let directory = tempfile::TempDir::new().unwrap();
        let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
        let reservation = state
            .reserve_artifact(directory.path(), &directory_identity)
            .unwrap();
        let path = directory.path().join("tracked.json");
        let claim = directory.path().join(".registry-claim-collision");
        std::fs::write(&path, b"tracked response").unwrap();
        std::fs::write(&claim, b"collision response").unwrap();
        let identity = crate::path_identity::verify_regular_file(&path).unwrap();
        reservation.commit(path.clone(), identity.clone()).unwrap();
        state.install_unix_cleanup_hook(
            claim.file_name().map(std::ffi::OsStr::to_os_string),
            |_, _, _| {},
        );

        assert_eq!(
            state.cleanup_artifact(&path, &identity),
            TerminalSnapshotArtifactCleanupOutcome::SourceRetained
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"tracked response");
        assert_eq!(std::fs::read(&claim).unwrap(), b"collision response");
        assert_eq!(state.test_artifact_counts(), (1, 1, 0));

        std::fs::remove_file(claim).unwrap();
        assert_eq!(
            state.cleanup_artifact(&path, &identity),
            TerminalSnapshotArtifactCleanupOutcome::Removed
        );
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[cfg(unix)]
    #[test]
    fn artifact_cleanup_token_serializes_relocation_and_second_cleanup() {
        let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
        let directory = tempfile::TempDir::new().unwrap();
        let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
        let reservation = state
            .reserve_artifact(directory.path(), &directory_identity)
            .unwrap();
        let path = directory.path().join("tracked.json");
        std::fs::write(&path, b"tracked response").unwrap();
        let identity = crate::path_identity::verify_regular_file(&path).unwrap();
        reservation.commit(path.clone(), identity.clone()).unwrap();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let entered_hook = Arc::clone(&entered);
        let release_hook = Arc::clone(&release);
        state.install_unix_cleanup_hook(None, move |stage, _, _| {
            if stage == crate::path_identity::UnixTrackedCleanupStage::BeforeClaimUnlink {
                entered_hook.wait();
                release_hook.wait();
            }
        });
        let cleanup_state = Arc::clone(&state);
        let cleanup_path = path.clone();
        let cleanup_identity = identity.clone();
        let cleanup = std::thread::spawn(move || {
            cleanup_state.cleanup_artifact(&cleanup_path, &cleanup_identity)
        });

        entered.wait();
        assert_eq!(state.test_artifact_counts(), (1, 1, 0));
        assert_eq!(
            state.cleanup_artifact(&path, &identity),
            TerminalSnapshotArtifactCleanupOutcome::Busy
        );
        assert!(state.relocate_artifact(&identity, &path).is_err());
        release.wait();
        assert_eq!(
            cleanup.join().unwrap(),
            TerminalSnapshotArtifactCleanupOutcome::Removed
        );
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[cfg(unix)]
    #[test]
    fn artifact_relocation_finds_namespace_move_that_wins_cleanup_race() {
        let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
        let directory = tempfile::TempDir::new().unwrap();
        let directory_identity = crate::path_identity::verify_directory(directory.path()).unwrap();
        let reservation = state
            .reserve_artifact(directory.path(), &directory_identity)
            .unwrap();
        let path = directory.path().join("temporary.json");
        let destination = directory.path().join("published.json");
        std::fs::write(&path, b"tracked response").unwrap();
        let identity = crate::path_identity::verify_regular_file(&path).unwrap();
        reservation.commit(path.clone(), identity.clone()).unwrap();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let entered_hook = Arc::clone(&entered);
        let release_hook = Arc::clone(&release);
        state.install_unix_cleanup_hook(None, move |stage, _, _| {
            if stage == crate::path_identity::UnixTrackedCleanupStage::BeforeClaimRename {
                entered_hook.wait();
                release_hook.wait();
            }
        });
        let cleanup_state = Arc::clone(&state);
        let cleanup_path = path.clone();
        let cleanup_identity = identity.clone();
        let cleanup = std::thread::spawn(move || {
            cleanup_state.cleanup_artifact(&cleanup_path, &cleanup_identity)
        });

        entered.wait();
        std::fs::rename(&path, &destination).unwrap();
        assert!(state.relocate_artifact(&identity, &destination).is_err());
        release.wait();
        assert_eq!(
            cleanup.join().unwrap(),
            TerminalSnapshotArtifactCleanupOutcome::Uncertain
        );
        assert!(state
            .relocate_artifact(&identity, &destination)
            .unwrap()
            .is_some());
        assert_eq!(state.test_artifact_counts(), (1, 1, 0));
        assert_eq!(
            state.cleanup_artifact(&destination, &identity),
            TerminalSnapshotArtifactCleanupOutcome::Removed
        );
        assert_eq!(state.test_artifact_counts(), (0, 0, 0));
    }

    #[test]
    fn host_final_handoff_discards_success_bytes_after_authority_change() {
        let secret = b"terminal-content-sentinel".to_vec();
        let result = finalize_host_publication(
            secret,
            Err(TerminalSnapshotReasonCode::AuthorityChanged),
            |outcome| {
                assert!(matches!(
                    outcome,
                    Err(TerminalSnapshotReasonCode::AuthorityChanged)
                ));
                Ok(())
            },
        );
        assert!(matches!(
            result,
            Err(TerminalSnapshotReasonCode::AuthorityChanged)
        ));
    }

    #[test]
    fn host_final_handoff_reports_the_actual_publication_failure() {
        let result = finalize_host_publication(b"safe-envelope".to_vec(), Ok(()), |outcome| {
            assert!(outcome.is_ok());
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        });
        assert!(matches!(
            result,
            Err(TerminalSnapshotReasonCode::ResponseUnavailable)
        ));
    }

    #[test]
    fn artifact_reservation_release_immediately_reclaims_an_idle_directory_record() {
        let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
        let directory = tempfile::TempDir::new().unwrap();
        let identity = crate::path_identity::verify_directory(directory.path()).unwrap();
        drop(state.reserve_artifact(directory.path(), &identity).unwrap());
        assert!(state.artifacts.lock().unwrap().directories.is_empty());
        assert!(state.reserve_artifact(directory.path(), &identity).is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timed_out_blocking_work_retains_the_requester_and_global_permit() {
        let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
        let permit = state.admit_requester("requester".to_string()).unwrap();
        let audit =
            TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::ContainerApi);
        let control = TerminalSnapshotBlockingControl::new(None);
        state.install_blocking_control(
            TerminalSnapshotBlockingStage::TestResourceRetention,
            Arc::clone(&control),
        );
        let task_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            let result = run_blocking_with_deadline(
                &task_state,
                TerminalSnapshotBlockingStage::TestResourceRetention,
                Instant::now() + Duration::from_secs(60),
                &permit,
                &audit,
                || (),
            )
            .await;
            drop(permit);
            result
        });
        control.wait_until_entered();
        control.expire_deadline();
        assert!(matches!(
            task.await.expect("controlled blocking waiter"),
            Err(TerminalSnapshotReasonCode::SnapshotTimeout)
        ));
        assert!(state.admit_requester("requester".to_string()).is_err());
        control.release();
        control.wait_until_completed();
        assert!(state.admit_requester("requester".to_string()).is_ok());
    }

    #[test]
    fn limiter_records_requester_before_target_promotion() {
        let state = TerminalSnapshotState::new(crate::shutdown::ShutdownSignal::new());
        let permit = state.admit_requester("requester".to_string()).unwrap();
        permit.promote_target("target".to_string()).unwrap();
        assert!(state.admit_requester("requester".to_string()).is_err());
        drop(permit);
        assert!(state.admit_requester("requester".to_string()).is_ok());
    }
}
