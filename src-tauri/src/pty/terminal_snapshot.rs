mod artifacts;
mod audit;
mod authority;
mod finalization;

use crate::config::settings::SettingsState;
use crate::config::teams::VerifiedTerminalSnapshotRoute;
#[cfg(test)]
use crate::pty::backend::SessionBackendKind;
use crate::pty::backend::TerminalScreenRead;
#[cfg(test)]
use crate::pty::context_scrape::ContextSessionLiveness;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
#[cfg(test)]
use crate::session::manager::TerminalSnapshotSessionFact;
#[cfg(test)]
use crate::session::session::{SessionStatus, TEMP_SESSION_PREFIX};
use artifacts::ArtifactRegistry;
pub(crate) use artifacts::TerminalSnapshotArtifactCleanupOutcome;
pub(crate) use audit::TerminalSnapshotAuditGuard;
#[cfg(test)]
use authority::status_rank;
use authority::{
    augment_coordinator_project, authority_key, build_payload, clone_session_manager,
    prove_requester, same_authority, select_target_session, RequesterProof, SelectedSession,
};
use finalization::{await_deadline, ensure_before_deadline, run_blocking_with_deadline};
#[cfg(test)]
use finalization::{final_revalidate_async, finalize_host_publication};
use limiter::{
    decrement_counter, prune_map, LimiterLeaseInner, RequesterSnapshotPermit, RollingState,
};
#[cfg(test)]
use std::collections::HashMap;
use std::collections::VecDeque;
#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
#[cfg(test)]
use terminal_snapshot_renderer::render_png;
use terminal_snapshot_renderer::{
    encode_api_json_success_from_model, encode_api_success_payload,
    encode_host_json_success_from_model, encode_host_success_payload,
    terminal_snapshot_json_bytes_from_model, terminal_snapshot_payload_bytes, TerminalScreenModel,
    TerminalSnapshotFormat, TerminalSnapshotPayload, TerminalSnapshotReasonCode,
};
#[cfg(all(test, unix))]
use test_support::TerminalSnapshotUnixCleanupControl;
#[cfg(test)]
pub(crate) use test_support::{
    TerminalSnapshotBlockingControl, TerminalSnapshotHostCancellationStage,
    TerminalSnapshotHostFinalizerControl, TerminalSnapshotHostFinalizerStage,
    TerminalSnapshotResponsePublicationFailure, TerminalSnapshotResponsePublicationStage,
    TerminalSnapshotTestLookupCounts,
};
#[cfg(test)]
use test_support::{TerminalSnapshotTestState, TerminalSnapshotTestTaskOutput};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

mod limiter;
#[cfg(test)]
mod test_support;

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
    pub restore: Arc<crate::app_state::RestoreInProgress>,
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

pub(crate) trait TerminalSnapshotArtifactCleanupTaskScope {
    fn spawn_owned(&self, future: futures::future::BoxFuture<'static, ()>);
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
}

impl TerminalSnapshotState {
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
        let memory_enabled = await_deadline(deadline, context.settings.current_read())
            .await?
            .map_err(|_| TerminalSnapshotReasonCode::TerminalSnapshotsDisabled)?
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
mod tests;
