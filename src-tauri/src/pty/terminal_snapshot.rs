use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use terminal_snapshot_renderer::{
    encode_canonical_base64, render_png, to_ascii_json, TerminalScreenModel,
    TerminalSnapshotDocument, TerminalSnapshotFormat, TerminalSnapshotReasonCode,
    TerminalSnapshotResult, MAX_TRANSPORT_BYTES,
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
            .field("request_id", &self.request_id)
            .field("format", &self.format)
            .finish()
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

pub(crate) struct TerminalSnapshotServiceContext {
    pub session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pub pty_manager: Arc<std::sync::Mutex<PtyManager>>,
    pub settings: SettingsState,
    pub restore: Arc<crate::RestoreInProgress>,
    pub purge: Arc<crate::session::purge_guard::PurgeGuard>,
}

pub(crate) struct TerminalSnapshotServiceSuccess {
    pub result: TerminalSnapshotResult,
    pub payload_bytes: u64,
}

impl std::fmt::Debug for TerminalSnapshotServiceSuccess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSnapshotServiceSuccess")
            .field("format", &self.result.format())
            .field("payload_bytes", &self.payload_bytes)
            .finish()
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
    path: PathBuf,
    identity: crate::path_identity::VerifiedPathIdentity,
}

#[derive(Clone)]
struct TrackedArtifactFile {
    directory: crate::path_identity::FileObjectId,
    path: PathBuf,
    identity: crate::path_identity::VerifiedPathIdentity,
    expires_at: Instant,
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
        let current_directory = crate::path_identity::verify_directory(&self.directory.path)
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        if !crate::path_identity::same_object(&current_directory, &self.directory.identity) {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        let current_file = crate::path_identity::verify_regular_file(&path)
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        if !crate::path_identity::same_object(&current_file, &identity)
            || !crate::path_identity::is_verified_descendant(
                &current_file,
                &self.directory.identity,
            )
        {
            return Err(TerminalSnapshotReasonCode::ResponseUnavailable);
        }
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?;
        let expires_at = Instant::now()
            .checked_add(ttl)
            .ok_or(TerminalSnapshotReasonCode::Internal)?;
        if let Some(existing) = registry.files.get_mut(&identity.object_id) {
            existing.directory = self.directory.identity.object_id;
            existing.path = path;
            existing.identity = identity;
            existing.expires_at = expires_at;
        } else {
            registry.files.insert(
                identity.object_id,
                TrackedArtifactFile {
                    directory: self.directory.identity.object_id,
                    path,
                    identity,
                    expires_at,
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
}

pub(crate) struct TerminalSnapshotState {
    ingress: Arc<Semaphore>,
    limiter: Arc<Mutex<RollingState>>,
    artifacts: Arc<Mutex<ArtifactRegistry>>,
    shutdown: crate::shutdown::ShutdownSignal,
}

impl TerminalSnapshotState {
    pub(crate) fn new(shutdown: crate::shutdown::ShutdownSignal) -> Arc<Self> {
        Arc::new(Self {
            ingress: Arc::new(Semaphore::new(SNAPSHOT_INGRESS_LIMIT)),
            limiter: Arc::new(Mutex::new(RollingState::default())),
            artifacts: Arc::new(Mutex::new(ArtifactRegistry::default())),
            shutdown,
        })
    }

    pub(crate) fn start_artifact_cleanup(self: &Arc<Self>) {
        let state = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = state.shutdown.token().cancelled() => {
                        state.sweep_artifacts(true);
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {
                        state.sweep_artifacts(false);
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
        let current = crate::path_identity::verify_directory(directory_path)
            .map_err(|_| TerminalSnapshotReasonCode::ResponseUnavailable)?;
        if !crate::path_identity::same_object(&current, directory_identity) {
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
            path: directory_path.to_path_buf(),
            identity: directory_identity.clone(),
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

    pub(crate) fn untrack_artifact(&self, identity: &crate::path_identity::VerifiedPathIdentity) {
        if let Ok(mut registry) = self.artifacts.lock() {
            registry.files.remove(&identity.object_id);
        }
    }

    fn sweep_artifacts(&self, force: bool) {
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
            match crate::path_identity::verify_regular_file(&tracked.path) {
                Ok(current) if crate::path_identity::same_object(&current, &tracked.identity) => {
                    if std::fs::remove_file(&tracked.path).is_ok() {
                        absent_files.push(tracked.identity.object_id);
                    }
                }
                Err(_) if !tracked.path.exists() => {
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
            if live_directories.contains(&directory.identity.object_id) {
                continue;
            }
            let vanished = crate::path_identity::verify_directory(&directory.path)
                .map(|current| !crate::path_identity::same_object(&current, &directory.identity))
                .unwrap_or(true);
            if vanished {
                registry.directories.remove(&directory.identity.object_id);
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

    pub(crate) async fn execute(
        &self,
        context: &TerminalSnapshotServiceContext,
        request: TerminalSnapshotServiceRequest,
        requester_selector: TerminalSnapshotRequesterSelector,
    ) -> Result<TerminalSnapshotServiceSuccess, TerminalSnapshotReasonCode> {
        let audit = TerminalSnapshotAuditGuard::new(&request);
        let result = self
            .execute_inner(context, &request, requester_selector, &audit)
            .await;
        match &result {
            Ok(success) => audit.finalize_success(success),
            Err(reason) => audit.finalize_failure(*reason),
        }
        result
    }

    pub(crate) async fn execute_with_deferred_success_audit(
        &self,
        context: &TerminalSnapshotServiceContext,
        request: TerminalSnapshotServiceRequest,
        requester_selector: TerminalSnapshotRequesterSelector,
        audit: TerminalSnapshotAuditGuard,
    ) -> Result<
        (TerminalSnapshotServiceSuccess, PendingTerminalSnapshotAudit),
        TerminalSnapshotReasonCode,
    > {
        audit.accept_request(&request);
        match self
            .execute_inner(context, &request, requester_selector, &audit)
            .await
        {
            Ok(success) => Ok((success, PendingTerminalSnapshotAudit(audit))),
            Err(reason) => {
                audit.finalize_failure(reason);
                Err(reason)
            }
        }
    }

    async fn execute_inner(
        &self,
        context: &TerminalSnapshotServiceContext,
        request: &TerminalSnapshotServiceRequest,
        requester_selector: TerminalSnapshotRequesterSelector,
        audit: &TerminalSnapshotAuditGuard,
    ) -> Result<TerminalSnapshotServiceSuccess, TerminalSnapshotReasonCode> {
        if self.shutdown.is_cancelled() {
            return Err(TerminalSnapshotReasonCode::ServiceUnavailable);
        }
        terminal_snapshot_renderer::validate_uuid(&request.request_id.to_string(), Some(4))
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;
        crate::config::teams::validate_terminal_snapshot_target_syntax(&request.target)
            .map_err(|_| TerminalSnapshotReasonCode::InvalidRequest)?;

        let manager = clone_session_manager(&context.session_manager).await;
        let requester = prove_requester(
            &manager,
            &context.pty_manager,
            requester_selector,
            request.source_plane,
        )
        .await?;
        audit.accept_requester(&requester.identity.canonical_fqn);
        let accepted_at = Instant::now();
        let server_deadline = accepted_at
            .checked_add(SNAPSHOT_SERVER_TIMEOUT)
            .ok_or(TerminalSnapshotReasonCode::Internal)?;
        let deadline = request
            .host_authorization_deadline
            .as_ref()
            .map_or(server_deadline, |(host_deadline, _)| {
                server_deadline.min(*host_deadline)
            });
        let requester_key = authority_key(&requester.identity);
        let permit = self.admit_requester(requester_key)?;

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
        let mut project_paths = security.project_paths.clone();
        let sender_is_root = requester.fact.is_root_agent;
        if !sender_is_root {
            augment_coordinator_project(&mut project_paths, &requester.identity)?;
        }
        let route = run_blocking_with_deadline(deadline, permit.clone(), move || {
            crate::config::teams::verify_terminal_snapshot_route(
                std::path::Path::new(&sender_cwd),
                sender_is_root,
                &target,
                &project_paths,
            )
        })
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

        let facts = await_deadline(deadline, manager.terminal_snapshot_session_facts()).await??;
        let ids: Vec<Uuid> = facts.iter().map(|fact| fact.id).collect();
        let pty_manager = Arc::clone(&context.pty_manager);
        let proofs = run_blocking_with_deadline(deadline, permit.clone(), move || {
            PtyManager::snapshot_route_proofs(&pty_manager, &ids)
        })
        .await??;
        let target_identity = route.target.clone();
        let selected = run_blocking_with_deadline(deadline, permit.clone(), move || {
            select_target_session(facts, proofs, &target_identity)
        })
        .await??;
        if context.purge.blocks_session(selected.fact.id) {
            return Err(TerminalSnapshotReasonCode::SnapshotUnavailable);
        }
        audit.accept_selected(&selected.fact);

        let capture_kind = selected.fact.backend_kind;
        let capture_cwd = selected.cwd_identity.clone();
        let capture_replica = route.target.replica_identity.clone();
        let selected = run_blocking_with_deadline(deadline, permit.clone(), move || {
            let read =
                selected
                    .proof
                    .capture_verified(capture_kind, &capture_cwd, &capture_replica);
            (selected, read)
        })
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
        let built = run_blocking_with_deadline(deadline, permit.clone(), move || {
            build_result(
                format,
                request_id,
                requester_fqn,
                target_fqn,
                &model_for_build,
            )
        })
        .await??;
        audit.accept_payload(built.payload_bytes);
        if request
            .host_authorization_deadline
            .as_ref()
            .is_some_and(|(_, wall_deadline)| chrono::Utc::now() >= *wall_deadline)
        {
            return Err(TerminalSnapshotReasonCode::SnapshotTimeout);
        }

        final_revalidate(
            self, context, &manager, deadline, permit, &requester, &route, &selected,
        )
        .await?;
        Ok(built)
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
    let (identity, cwd_identity) = tokio::task::spawn_blocking(move || {
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
    .await
    .map_err(|_| TerminalSnapshotReasonCode::ServiceUnavailable)?
    .map_err(|_| TerminalSnapshotReasonCode::RequesterUnavailable)?;
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
        .workspace_identity
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
    target: &VerifiedPtyInputIdentity,
) -> Result<SelectedSession, TerminalSnapshotReasonCode> {
    if facts.len() != proofs.len() {
        return Err(TerminalSnapshotReasonCode::Internal);
    }
    let mut eligible = Vec::new();
    let mut unavailable = false;
    for (fact, proof) in facts.into_iter().zip(proofs) {
        let lexical_target =
            std::path::Path::new(&fact.working_directory).starts_with(&target.replica_root);
        let cwd_identity = match crate::path_identity::verify_directory(std::path::Path::new(
            &fact.working_directory,
        )) {
            Ok(identity)
                if crate::path_identity::is_verified_descendant(
                    &identity,
                    &target.replica_identity,
                ) =>
            {
                identity
            }
            Ok(_) => continue,
            Err(_) => {
                if lexical_target && !matches!(fact.status, SessionStatus::Exited(_)) {
                    unavailable = true;
                }
                continue;
            }
        };
        if matches!(fact.status, SessionStatus::Exited(_))
            || fact.name.starts_with(TEMP_SESSION_PREFIX)
        {
            continue;
        }
        let Some(proof) = proof else {
            unavailable = true;
            continue;
        };
        let route_matches = proof.backend_kind() == fact.backend_kind
            && crate::path_identity::same_object(proof.saved_cwd(), &cwd_identity)
            && proof.saved_replica().is_some_and(|replica| {
                crate::path_identity::same_object(replica, &target.replica_identity)
            });
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

fn build_result(
    format: TerminalSnapshotFormat,
    request_id: String,
    requester: String,
    target: String,
    model: &TerminalScreenModel,
) -> Result<TerminalSnapshotServiceSuccess, TerminalSnapshotReasonCode> {
    match format {
        TerminalSnapshotFormat::Json => {
            let document =
                TerminalSnapshotDocument::from_model(request_id, requester, target, model);
            document
                .validate()
                .map_err(|_| TerminalSnapshotReasonCode::Internal)?;
            let document_bytes =
                to_ascii_json(&document, terminal_snapshot_renderer::MAX_JSON_BYTES).map_err(
                    |error| match error {
                        terminal_snapshot_renderer::ProtocolError::TooLarge => {
                            TerminalSnapshotReasonCode::SnapshotTooLarge
                        }
                        _ => TerminalSnapshotReasonCode::Internal,
                    },
                )?;
            let result = TerminalSnapshotResult::Json { snapshot: document };
            to_ascii_json(&result, MAX_TRANSPORT_BYTES)
                .map_err(|_| TerminalSnapshotReasonCode::SnapshotTooLarge)?;
            Ok(TerminalSnapshotServiceSuccess {
                result,
                payload_bytes: document_bytes.len() as u64,
            })
        }
        TerminalSnapshotFormat::Png => {
            let rendered = render_png(model).map_err(|error| match error {
                terminal_snapshot_renderer::RenderError::TooLarge => {
                    TerminalSnapshotReasonCode::SnapshotTooLarge
                }
                _ => TerminalSnapshotReasonCode::RenderFailed,
            })?;
            let metadata = rendered.metadata(request_id, requester, target, model);
            let png_base64 = encode_canonical_base64(&rendered.bytes)
                .map_err(|_| TerminalSnapshotReasonCode::SnapshotTooLarge)?;
            let payload_bytes = rendered.bytes.len() as u64;
            let result = TerminalSnapshotResult::Png {
                metadata,
                png_base64,
            };
            to_ascii_json(&result, MAX_TRANSPORT_BYTES)
                .map_err(|_| TerminalSnapshotReasonCode::SnapshotTooLarge)?;
            Ok(TerminalSnapshotServiceSuccess {
                result,
                payload_bytes,
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn final_revalidate(
    state: &TerminalSnapshotState,
    context: &TerminalSnapshotServiceContext,
    manager: &SessionManager,
    deadline: Instant,
    permit: RequesterSnapshotPermit,
    requester: &RequesterProof,
    route: &VerifiedTerminalSnapshotRoute,
    selected: &SelectedSession,
) -> Result<(), TerminalSnapshotReasonCode> {
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
    let fresh_route = run_blocking_with_deadline(deadline, permit, move || {
        crate::config::teams::verify_terminal_snapshot_route(
            std::path::Path::new(&sender_cwd),
            sender_is_root,
            &target_fqn,
            &project_paths,
        )
    })
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
    let current_requester = await_deadline(
        deadline,
        manager.live_snapshot_requester_by_id(requester.fact.id),
    )
    .await?
    .ok_or(TerminalSnapshotReasonCode::AuthorityChanged)?;
    if current_requester.created_at != requester.fact.created_at
        || current_requester.working_directory != requester.fact.working_directory
        || current_requester.backend_kind != requester.fact.backend_kind
        || current_requester.is_root_agent != requester.fact.is_root_agent
        || current_requester.is_coordinator != requester.fact.is_coordinator
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
    let current_selected = await_deadline(
        deadline,
        manager.terminal_snapshot_session_fact_by_id(selected.fact.id),
    )
    .await?
    .ok_or(TerminalSnapshotReasonCode::AuthorityChanged)?;
    if current_selected.created_at != selected.fact.created_at
        || current_selected.working_directory != selected.fact.working_directory
        || current_selected.backend_kind != selected.fact.backend_kind
        || current_selected.name != selected.fact.name
        || current_selected.name.starts_with(TEMP_SESSION_PREFIX)
        || matches!(current_selected.status, SessionStatus::Exited(_))
        || selected.proof.liveness() != ContextSessionLiveness::Live
        || !selected.proof.matches_current(
            selected.fact.backend_kind,
            &selected.cwd_identity,
            &route.target.replica_identity,
        )
    {
        return Err(TerminalSnapshotReasonCode::AuthorityChanged);
    }
    ensure_before_deadline(deadline, &state.shutdown)
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
    deadline: Instant,
    permit: RequesterSnapshotPermit,
    operation: F,
) -> Result<T, TerminalSnapshotReasonCode>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        crate::logging::catch_payload_unwind(operation)
    });
    let joined = await_deadline(deadline, handle).await?;
    match joined {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) | Err(_) => Err(TerminalSnapshotReasonCode::Internal),
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

pub(crate) struct PendingTerminalSnapshotAudit(TerminalSnapshotAuditGuard);

impl PendingTerminalSnapshotAudit {
    pub(crate) fn finalize_success(self) {
        self.0.finalize("succeeded", None);
    }

    pub(crate) fn finalize_failure(self, reason: TerminalSnapshotReasonCode) {
        self.0.finalize_failure(reason);
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

    fn new(request: &TerminalSnapshotServiceRequest) -> Self {
        let audit = Self::pre_admission(request.source_plane);
        audit.accept_request(request);
        audit
    }

    fn accept_request(&self, request: &TerminalSnapshotServiceRequest) {
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

    fn finalize_success(&self, success: &TerminalSnapshotServiceSuccess) {
        self.accept_payload(success.payload_bytes);
        self.finalize("succeeded", None);
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
mod tests {
    use super::*;

    #[test]
    fn status_order_is_exact() {
        assert!(status_rank(&SessionStatus::Active) < status_rank(&SessionStatus::Running));
        assert!(status_rank(&SessionStatus::Running) < status_rank(&SessionStatus::Idle));
        assert!(status_rank(&SessionStatus::Idle) < status_rank(&SessionStatus::Exited(0)));
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
