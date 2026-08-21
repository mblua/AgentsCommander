use std::collections::HashMap;
use std::sync::{Arc, Mutex, TryLockError, Weak};

use uuid::Uuid;

use crate::errors::AppError;
use crate::pty::backend::{
    BackendSpawnSpec, PtyBackend, SessionBackendKind, TerminalScreenCopyRead, TerminalScreenRead,
};
use crate::pty::container_backend::ContainerTransportBackend;
use crate::pty::container_tokens::ContainerApiTokenManager;
use crate::pty::context_scrape::{ContextSessionLiveness, ScreenRowsRead};
use crate::pty::docker_runtime::DockerRuntime;
use crate::pty::git_watcher::GitWatcher;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::local_backend::LocalProcessBackend;
use crate::pty::output::{PtyScreenSnapshot, PtyTerminalOutputActivation};
use crate::telegram::manager::OutputSenderMap;

pub(crate) use crate::pty::output::{
    PtyTerminalReplaySnapshot, PtyTerminalSeedlessReason, TerminalOutputAttachError,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingSpawn {
    pub cwd: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PtyRouteRemovalError {
    Busy,
    LockPoisoned,
}

struct RouteEntry {
    kind: SessionBackendKind,
    generation: u64,
    input_gate: Arc<tokio::sync::Mutex<()>>,
    lifecycle_gate: Arc<std::sync::Mutex<()>>,
    canonical_cwd_identity: Option<crate::path_identity::VerifiedPathIdentity>,
    verified_replica_anchor: Option<crate::path_identity::VerifiedPathIdentity>,
}

#[derive(Default)]
struct SpawnRegistry {
    routes: HashMap<Uuid, RouteEntry>,
    pending: HashMap<u64, PendingSpawn>,
    next_seq: u64,
    next_route_generation: u64,
}

struct ViewOutputRegistry {
    next_document_epoch: u64,
    views: HashMap<String, ViewOutputState>,
}

impl Default for ViewOutputRegistry {
    fn default() -> Self {
        Self {
            next_document_epoch: 1,
            views: HashMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TerminalOutputObservationStage {
    PostWrite,
    PostFit,
    Settled,
    Aborted,
}

impl TerminalOutputObservationStage {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::PostWrite => "postWrite",
            Self::PostFit => "postFit",
            Self::Settled => "settled",
            Self::Aborted => "aborted",
        }
    }
}

struct ViewOutputState {
    document_epoch: u64,
    high_water_generation: u32,
    generation: Option<ViewOutputGenerationState>,
    observation: Option<ViewOutputObservationState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ViewOutputGenerationKey {
    session_id: Uuid,
    generation: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ViewOutputOwner {
    key: ViewOutputGenerationKey,
    backend_kind: SessionBackendKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewOutputGenerationState {
    Owned(ViewOutputOwner),
    Canceled(ViewOutputGenerationKey),
    Failed(ViewOutputGenerationKey),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ViewOutputObservationState {
    key: ViewOutputGenerationKey,
    last_stage: TerminalOutputObservationStage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminalOutputObservationError {
    RegistryPoisoned,
    DocumentUnavailable,
    StaleDocumentEpoch,
    StaleIdentity,
    StageOrderInvalid,
    CardinalityExceeded,
}

impl TerminalOutputObservationError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::RegistryPoisoned => "terminalOutputRegistryPoisoned",
            Self::DocumentUnavailable => "documentUnavailable",
            Self::StaleDocumentEpoch => "staleDocumentEpoch",
            Self::StaleIdentity => "staleObservationGeneration",
            Self::StageOrderInvalid => "observationStageOrderInvalid",
            Self::CardinalityExceeded => "observationCardinalityExceeded",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalOutputCoordinationError {
    DocumentEpochExhausted,
    DocumentUnavailable,
    StaleDocumentEpoch,
    StaleAttachGeneration,
    AttachGenerationZero,
    RouteUnavailable,
    BackendUnavailable,
    RegistryPoisoned,
    Backend(TerminalOutputAttachError),
}

impl TerminalOutputCoordinationError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::DocumentEpochExhausted => "documentEpochExhausted",
            Self::DocumentUnavailable => "documentUnavailable",
            Self::StaleDocumentEpoch => "staleDocumentEpoch",
            Self::StaleAttachGeneration => "staleAttachGeneration",
            Self::AttachGenerationZero => "attachGenerationZero",
            Self::RouteUnavailable => "sessionNotFound",
            Self::BackendUnavailable => "backendUnavailable",
            Self::RegistryPoisoned => "terminalOutputRegistryPoisoned",
            Self::Backend(error) => error.code(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct TerminalOutputCoordinator {
    state: Arc<Mutex<ViewOutputRegistry>>,
    routes: Arc<Mutex<SpawnRegistry>>,
    local_backend: Weak<dyn PtyBackend>,
    container_backend: Weak<dyn PtyBackend>,
}

fn log_coordinator_activation_decision(
    level: log::Level,
    label: &str,
    session_id: Uuid,
    document_epoch: u64,
    attach_generation: u32,
    reason: &str,
) {
    let message = format!(
        "[terminal-snapshot] event=terminal_attach_backend stage=activation_decision session={session_id} label={label:?} epoch={document_epoch} generation={attach_generation} reason={reason} sequence=0 rows=0 cols=0 active=none replay_stage=none replay_bytes=0 history_rows=0 pending_bytes=0"
    );
    match level {
        log::Level::Error => log::error!("{message}"),
        log::Level::Warn => log::warn!("{message}"),
        log::Level::Info => log::info!("{message}"),
        log::Level::Debug | log::Level::Trace => log::debug!("{message}"),
    }
}

impl TerminalOutputCoordinator {
    fn new(
        routes: Arc<Mutex<SpawnRegistry>>,
        local_backend: Arc<dyn PtyBackend>,
        container_backend: Arc<dyn PtyBackend>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(ViewOutputRegistry::default())),
            routes,
            local_backend: Arc::downgrade(&local_backend),
            container_backend: Arc::downgrade(&container_backend),
        }
    }

    fn backend_for_kind(&self, kind: SessionBackendKind) -> Option<Arc<dyn PtyBackend>> {
        match kind {
            SessionBackendKind::LocalProcess => self.local_backend.upgrade(),
            SessionBackendKind::ContainerTransport => self.container_backend.upgrade(),
        }
    }

    fn resolve_route(
        &self,
        session_id: Uuid,
    ) -> Result<(SessionBackendKind, Arc<dyn PtyBackend>), TerminalOutputCoordinationError> {
        let kind = self
            .routes
            .lock()
            .map_err(|_| TerminalOutputCoordinationError::RegistryPoisoned)?
            .routes
            .get(&session_id)
            .map(|entry| entry.kind)
            .ok_or(TerminalOutputCoordinationError::RouteUnavailable)?;
        let backend = self
            .backend_for_kind(kind)
            .ok_or(TerminalOutputCoordinationError::BackendUnavailable)?;
        Ok((kind, backend))
    }

    fn detach_owner(&self, label: &str, owner: ViewOutputOwner) {
        if let Some(backend) = self.backend_for_kind(owner.backend_kind) {
            backend.detach_terminal_output(owner.key.session_id, label);
        }
    }

    fn clear_residual_label(&self, label: &str) {
        if let Some(backend) = self.local_backend.upgrade() {
            backend.release_window_attachments(label);
        }
        if let Some(backend) = self.container_backend.upgrade() {
            backend.release_window_attachments(label);
        }
    }

    pub(crate) fn begin_document(
        &self,
        label: &str,
    ) -> Result<u64, TerminalOutputCoordinationError> {
        let mut registry = self
            .state
            .lock()
            .map_err(|_| TerminalOutputCoordinationError::RegistryPoisoned)?;
        if let Some(previous) = registry.views.remove(label) {
            if let Some(ViewOutputGenerationState::Owned(owner)) = previous.generation {
                self.detach_owner(label, owner);
            }
        }
        self.clear_residual_label(label);
        let epoch = registry.next_document_epoch;
        registry.next_document_epoch = epoch
            .checked_add(1)
            .ok_or(TerminalOutputCoordinationError::DocumentEpochExhausted)?;
        registry.views.insert(
            label.to_string(),
            ViewOutputState {
                document_epoch: epoch,
                high_water_generation: 0,
                generation: None,
                observation: None,
            },
        );
        Ok(epoch)
    }

    pub(crate) fn document_epoch(
        &self,
        label: &str,
    ) -> Result<u64, TerminalOutputCoordinationError> {
        self.state
            .lock()
            .map_err(|_| TerminalOutputCoordinationError::RegistryPoisoned)?
            .views
            .get(label)
            .map(|state| state.document_epoch)
            .ok_or(TerminalOutputCoordinationError::DocumentUnavailable)
    }

    pub(crate) fn accept_observation_stage(
        &self,
        label: &str,
        session_id: Uuid,
        document_epoch: u64,
        attach_generation: u32,
        stage: TerminalOutputObservationStage,
    ) -> Result<(), TerminalOutputObservationError> {
        let mut registry = self
            .state
            .lock()
            .map_err(|_| TerminalOutputObservationError::RegistryPoisoned)?;
        let view = registry
            .views
            .get_mut(label)
            .ok_or(TerminalOutputObservationError::DocumentUnavailable)?;
        if view.document_epoch != document_epoch {
            return Err(TerminalOutputObservationError::StaleDocumentEpoch);
        }

        let key = ViewOutputGenerationKey {
            session_id,
            generation: attach_generation,
        };
        let is_owner = match view.generation {
            Some(ViewOutputGenerationState::Owned(owner)) if owner.key == key => true,
            Some(ViewOutputGenerationState::Canceled(current)) if current == key => false,
            Some(ViewOutputGenerationState::Failed(current)) if current == key => false,
            _ => return Err(TerminalOutputObservationError::StaleIdentity),
        };

        if view
            .observation
            .is_some_and(|observation| observation.key != key)
        {
            view.observation = None;
        }
        let valid = match view.observation.map(|observation| observation.last_stage) {
            None if is_owner => matches!(
                stage,
                TerminalOutputObservationStage::PostWrite | TerminalOutputObservationStage::Aborted
            ),
            None => stage == TerminalOutputObservationStage::Aborted,
            Some(TerminalOutputObservationStage::PostWrite) if is_owner => matches!(
                stage,
                TerminalOutputObservationStage::PostFit | TerminalOutputObservationStage::Aborted
            ),
            Some(TerminalOutputObservationStage::PostWrite) => {
                stage == TerminalOutputObservationStage::Aborted
            }
            Some(TerminalOutputObservationStage::PostFit) if is_owner => matches!(
                stage,
                TerminalOutputObservationStage::Settled | TerminalOutputObservationStage::Aborted
            ),
            Some(TerminalOutputObservationStage::PostFit) => {
                stage == TerminalOutputObservationStage::Aborted
            }
            Some(
                TerminalOutputObservationStage::Settled | TerminalOutputObservationStage::Aborted,
            ) => return Err(TerminalOutputObservationError::CardinalityExceeded),
        };
        if !valid {
            return Err(TerminalOutputObservationError::StageOrderInvalid);
        }
        view.observation = Some(ViewOutputObservationState {
            key,
            last_stage: stage,
        });
        Ok(())
    }

    /// Atomically transfers one webview's output ownership under the view registry lock.
    /// Route resolution drops `SpawnRegistry` first. While `ViewOutputRegistry` is held, this
    /// method may detach the previous backend owner and activate the selected backend, whose
    /// nested order is parser state then attachment state. Backends must not synchronously
    /// re-enter this coordinator. Epoch acceptance, generation high-water advancement, and the
    /// final owned or failed state belong exclusively to this coordinator; the backend only
    /// carries the accepted epoch and generation as correlation metadata.
    pub(crate) fn activate(
        &self,
        label: &str,
        session_id: Uuid,
        document_epoch: u64,
        attach_generation: u32,
        include_history: bool,
    ) -> Result<PtyTerminalOutputActivation, TerminalOutputCoordinationError> {
        if attach_generation == 0 {
            log_coordinator_activation_decision(
                log::Level::Warn,
                label,
                session_id,
                document_epoch,
                attach_generation,
                "attachGenerationZero",
            );
            return Err(TerminalOutputCoordinationError::AttachGenerationZero);
        }
        let (backend_kind, backend) = match self.resolve_route(session_id) {
            Ok(route) => route,
            Err(error) => {
                log_coordinator_activation_decision(
                    log::Level::Error,
                    label,
                    session_id,
                    document_epoch,
                    attach_generation,
                    error.code(),
                );
                return Err(error);
            }
        };
        let mut registry = match self.state.lock() {
            Ok(registry) => registry,
            Err(_) => {
                log_coordinator_activation_decision(
                    log::Level::Error,
                    label,
                    session_id,
                    document_epoch,
                    attach_generation,
                    TerminalOutputCoordinationError::RegistryPoisoned.code(),
                );
                return Err(TerminalOutputCoordinationError::RegistryPoisoned);
            }
        };
        let Some(view) = registry.views.get_mut(label) else {
            drop(registry);
            log_coordinator_activation_decision(
                log::Level::Warn,
                label,
                session_id,
                document_epoch,
                attach_generation,
                TerminalOutputCoordinationError::DocumentUnavailable.code(),
            );
            return Err(TerminalOutputCoordinationError::DocumentUnavailable);
        };
        if view.document_epoch != document_epoch {
            drop(registry);
            log_coordinator_activation_decision(
                log::Level::Debug,
                label,
                session_id,
                document_epoch,
                attach_generation,
                TerminalOutputCoordinationError::StaleDocumentEpoch.code(),
            );
            return Err(TerminalOutputCoordinationError::StaleDocumentEpoch);
        }
        if attach_generation <= view.high_water_generation {
            drop(registry);
            log_coordinator_activation_decision(
                log::Level::Debug,
                label,
                session_id,
                document_epoch,
                attach_generation,
                TerminalOutputCoordinationError::StaleAttachGeneration.code(),
            );
            return Err(TerminalOutputCoordinationError::StaleAttachGeneration);
        }
        view.high_water_generation = attach_generation;
        if let Some(ViewOutputGenerationState::Owned(previous)) = view.generation.take() {
            self.detach_owner(label, previous);
        }
        view.observation = None;
        let key = ViewOutputGenerationKey {
            session_id,
            generation: attach_generation,
        };
        match backend.activate_terminal_output(
            session_id,
            label,
            include_history,
            document_epoch,
            attach_generation,
        ) {
            Ok(activation) => {
                view.generation = Some(ViewOutputGenerationState::Owned(ViewOutputOwner {
                    key,
                    backend_kind,
                }));
                Ok(activation)
            }
            Err(error) => {
                backend.detach_terminal_output(session_id, label);
                view.generation = Some(ViewOutputGenerationState::Failed(key));
                Err(TerminalOutputCoordinationError::Backend(error))
            }
        }
    }

    pub(crate) fn detach(
        &self,
        label: &str,
        session_id: Uuid,
        document_epoch: u64,
        attach_generation: u32,
    ) -> Result<(), TerminalOutputCoordinationError> {
        let mut registry = self
            .state
            .lock()
            .map_err(|_| TerminalOutputCoordinationError::RegistryPoisoned)?;
        let Some(view) = registry.views.get_mut(label) else {
            return Ok(());
        };
        if view.document_epoch != document_epoch {
            return Ok(());
        }
        let key = ViewOutputGenerationKey {
            session_id,
            generation: attach_generation,
        };
        if attach_generation != 0 {
            if let Some(ViewOutputGenerationState::Owned(owner)) = view.generation {
                if owner.key == key {
                    self.detach_owner(label, owner);
                    view.generation = Some(ViewOutputGenerationState::Canceled(key));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn cancel(
        &self,
        label: &str,
        session_id: Uuid,
        document_epoch: u64,
        attach_generation: u32,
    ) -> Result<(), TerminalOutputCoordinationError> {
        if attach_generation == 0 {
            return Err(TerminalOutputCoordinationError::AttachGenerationZero);
        }
        let mut registry = self
            .state
            .lock()
            .map_err(|_| TerminalOutputCoordinationError::RegistryPoisoned)?;
        let Some(view) = registry.views.get_mut(label) else {
            return Ok(());
        };
        if view.document_epoch != document_epoch {
            return Ok(());
        }
        let key = ViewOutputGenerationKey {
            session_id,
            generation: attach_generation,
        };
        if attach_generation > view.high_water_generation {
            view.high_water_generation = attach_generation;
            if let Some(ViewOutputGenerationState::Owned(owner)) = view.generation.take() {
                self.detach_owner(label, owner);
            }
            view.generation = Some(ViewOutputGenerationState::Canceled(key));
            view.observation = None;
            return Ok(());
        }
        if let Some(ViewOutputGenerationState::Owned(owner)) = view.generation {
            if owner.key == key {
                self.detach_owner(label, owner);
                view.generation = Some(ViewOutputGenerationState::Canceled(key));
            }
        }
        Ok(())
    }

    pub(crate) fn release_window(
        &self,
        label: &str,
    ) -> Result<(), TerminalOutputCoordinationError> {
        let mut registry = self
            .state
            .lock()
            .map_err(|_| TerminalOutputCoordinationError::RegistryPoisoned)?;
        if let Some(view) = registry.views.remove(label) {
            if let Some(ViewOutputGenerationState::Owned(owner)) = view.generation {
                self.detach_owner(label, owner);
            }
        }
        self.clear_residual_label(label);
        Ok(())
    }

    fn release_session(
        &self,
        session_id: Uuid,
        backend_kind: SessionBackendKind,
    ) -> Result<(), TerminalOutputCoordinationError> {
        let mut registry = self
            .state
            .lock()
            .map_err(|_| TerminalOutputCoordinationError::RegistryPoisoned)?;
        for (label, view) in &mut registry.views {
            match view.generation {
                Some(ViewOutputGenerationState::Owned(owner))
                    if owner.key.session_id == session_id && owner.backend_kind == backend_kind =>
                {
                    self.detach_owner(label, owner);
                    view.generation = None;
                    view.observation = None;
                }
                Some(ViewOutputGenerationState::Canceled(key)) if key.session_id == session_id => {
                    view.generation = None;
                    view.observation = None;
                }
                Some(ViewOutputGenerationState::Failed(key)) if key.session_id == session_id => {
                    view.generation = None;
                    view.observation = None;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Unforgeable safe-code authority for a raw backend PTY write. Only the
/// route-guard chokepoint can issue a production value, so adding an alias or
/// reformatting a call cannot bypass the permit inventory.
#[doc(hidden)]
pub struct BackendWriteAuthority {
    _private: (),
}

impl BackendWriteAuthority {
    fn for_route_guard() -> Self {
        Self { _private: () }
    }

    #[cfg(test)]
    pub(crate) fn for_backend_test() -> Self {
        Self { _private: () }
    }
}

/// Exclusive per-session input ownership. Holding this guard serializes a
/// complete multi-phase submission against every other production writer.
pub struct PtyInputPermit {
    session_id: Uuid,
    route_generation: u64,
    route_registry: Arc<std::sync::Mutex<SpawnRegistry>>,
    route_lifecycle: Arc<std::sync::Mutex<()>>,
    backend: Arc<dyn PtyBackend>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

/// Generation and lifecycle ownership for final sender-route authorization.
/// It carries no backend or credential and cannot write PTY bytes.
pub(crate) struct PtyAuthorityRouteProof {
    session_id: Uuid,
    route_generation: u64,
    route_registry: Arc<std::sync::Mutex<SpawnRegistry>>,
    route_lifecycle: Arc<std::sync::Mutex<()>>,
}

/// Non-writing proof for one frozen PTY route generation. It carries no input
/// gate and cannot mint a backend write authority.
pub(crate) struct PtySnapshotRouteProof {
    session_id: Uuid,
    route_generation: u64,
    route_kind: SessionBackendKind,
    route_registry: Arc<std::sync::Mutex<SpawnRegistry>>,
    route_lifecycle: Arc<std::sync::Mutex<()>>,
    backend: Arc<dyn PtyBackend>,
    saved_cwd: crate::path_identity::VerifiedPathIdentity,
    saved_replica: Option<crate::path_identity::VerifiedPathIdentity>,
}

/// A short-lived route lifecycle guard for one synchronous backend write.
pub struct PtyRouteWriteGuard<'a> {
    session_id: Uuid,
    backend: Arc<dyn PtyBackend>,
    _guard: std::sync::MutexGuard<'a, ()>,
    _authority_guard: Option<std::sync::MutexGuard<'a, ()>>,
    _settings_guard:
        Option<tokio::sync::OwnedRwLockReadGuard<crate::config::settings::AppSettings>>,
}

impl<'a> PtyRouteWriteGuard<'a> {
    pub fn write(&self, bytes: &[u8]) -> Result<(), AppError> {
        let authority = BackendWriteAuthority::for_route_guard();
        self.backend.write(&authority, self.session_id, bytes)
    }

    pub(crate) fn retain_authority_guard(
        &mut self,
        authority_guard: std::sync::MutexGuard<'a, ()>,
    ) {
        self._authority_guard = Some(authority_guard);
    }

    pub(crate) fn retain_settings_guard(
        &mut self,
        settings_guard: tokio::sync::OwnedRwLockReadGuard<crate::config::settings::AppSettings>,
    ) {
        self._settings_guard = Some(settings_guard);
    }
}

pub(crate) struct SpawnMark {
    registry: Arc<Mutex<SpawnRegistry>>,
    seq: u64,
}

impl Drop for SpawnMark {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry.pending.remove(&self.seq);
    }
}

pub struct PtyManager {
    registry: Arc<Mutex<SpawnRegistry>>,
    local_backend: Arc<dyn PtyBackend>,
    container_backend: Arc<ContainerTransportBackend>,
    terminal_output: TerminalOutputCoordinator,
}

impl PtyManager {
    pub fn new(
        output_senders: OutputSenderMap,
        idle_detector: Arc<IdleDetector>,
        git_watcher: Arc<GitWatcher>,
        ws_broadcaster: Option<crate::web::broadcast::WsBroadcaster>,
        lifecycle_sender: Option<crate::session::selection::ContainerLifecycleSender>,
    ) -> Self {
        let local_backend: Arc<dyn PtyBackend> = Arc::new(LocalProcessBackend::new(
            output_senders.clone(),
            idle_detector.clone(),
            git_watcher,
            ws_broadcaster.clone(),
        ));
        let container_backend = Arc::new(ContainerTransportBackend::with_runtime(
            output_senders,
            idle_detector,
            ws_broadcaster,
            lifecycle_sender,
            Arc::new(DockerRuntime::new()),
            ContainerApiTokenManager::at_config_dir(),
        ));
        debug_assert!(local_backend.as_any().is::<LocalProcessBackend>());
        debug_assert!(container_backend.as_any().is::<ContainerTransportBackend>());
        let registry = Arc::new(Mutex::new(SpawnRegistry::default()));
        let container_output_backend: Arc<dyn PtyBackend> = container_backend.clone();
        let terminal_output = TerminalOutputCoordinator::new(
            Arc::clone(&registry),
            Arc::clone(&local_backend),
            container_output_backend,
        );
        Self {
            registry,
            local_backend,
            container_backend,
            terminal_output,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(local_backend: Arc<dyn PtyBackend>) -> Self {
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let container_backend = Arc::new(ContainerTransportBackend::new(
            output_senders,
            idle_detector,
            None,
            None,
        ));
        Self::new_for_test_with_container_backend(local_backend, container_backend)
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_container_backend(
        local_backend: Arc<dyn PtyBackend>,
        container_backend: Arc<ContainerTransportBackend>,
    ) -> Self {
        let registry = Arc::new(Mutex::new(SpawnRegistry::default()));
        let container_output_backend: Arc<dyn PtyBackend> = container_backend.clone();
        let terminal_output = TerminalOutputCoordinator::new(
            Arc::clone(&registry),
            Arc::clone(&local_backend),
            container_output_backend,
        );
        Self {
            registry,
            local_backend,
            container_backend,
            terminal_output,
        }
    }

    pub(crate) fn backend_for_kind(&self, kind: SessionBackendKind) -> Arc<dyn PtyBackend> {
        match kind {
            SessionBackendKind::LocalProcess => self.local_backend.clone(),
            SessionBackendKind::ContainerTransport => self.container_backend.clone(),
        }
    }

    pub fn container_backend(&self) -> Arc<ContainerTransportBackend> {
        self.container_backend.clone()
    }

    pub(crate) fn terminal_output_coordinator(&self) -> TerminalOutputCoordinator {
        self.terminal_output.clone()
    }

    pub fn start_container_pending_reaper(&self, shutdown: crate::shutdown::ShutdownSignal) {
        self.container_backend.start_pending_reaper(shutdown);
    }

    pub fn cleanup_container_orphans_on_startup(&self) {
        self.container_backend.cleanup_labeled_orphans_on_startup();
    }

    pub fn stop_all_started_containers_blocking(
        &self,
        budget: std::time::Duration,
    ) -> super::container_backend::ContainerShutdownReport {
        self.container_backend
            .stop_all_started_containers_blocking(budget)
    }

    pub fn record_route(&self, id: Uuid, kind: SessionBackendKind) {
        if let Err(error) = self.try_record_route(id, kind) {
            log::warn!("[pty] route registration rejected session={id} code={error}");
        }
    }

    pub fn try_record_route(&self, id: Uuid, kind: SessionBackendKind) -> Result<(), AppError> {
        self.record_route_with_identities(id, kind, None, None)
    }

    pub(crate) fn record_route_with_identities(
        &self,
        id: Uuid,
        kind: SessionBackendKind,
        canonical_cwd_identity: Option<crate::path_identity::VerifiedPathIdentity>,
        verified_replica_anchor: Option<crate::path_identity::VerifiedPathIdentity>,
    ) -> Result<(), AppError> {
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        if registry.routes.contains_key(&id) {
            return Err(AppError::PtyError("duplicate_pty_route".to_string()));
        }
        let generation = registry
            .next_route_generation
            .checked_add(1)
            .ok_or_else(|| AppError::PtyError("route_generation_overflow".to_string()))?;
        registry.next_route_generation = generation;
        registry.routes.insert(
            id,
            RouteEntry {
                kind,
                generation,
                input_gate: Arc::new(tokio::sync::Mutex::new(())),
                lifecycle_gate: Arc::new(std::sync::Mutex::new(())),
                canonical_cwd_identity,
                verified_replica_anchor,
            },
        );
        Ok(())
    }

    pub fn remove_route_if_kind(&self, id: Uuid, kind: SessionBackendKind) {
        let route = {
            let registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            registry
                .routes
                .get(&id)
                .filter(|entry| entry.kind == kind)
                .map(|entry| (Arc::clone(&entry.lifecycle_gate), entry.generation))
        };
        let Some((lifecycle, generation)) = route else {
            return;
        };
        if let Err(error) = self.terminal_output.release_session(id, kind) {
            log::warn!(
                "[terminal-snapshot] event=session_owner_cleanup session={id} reason={}",
                error.code()
            );
        }
        let deferred_lifecycle = Arc::clone(&lifecycle);
        match lifecycle.try_lock() {
            Ok(_lifecycle_guard) => {
                let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
                if registry
                    .routes
                    .get(&id)
                    .is_some_and(|entry| entry.kind == kind && entry.generation == generation)
                {
                    registry.routes.remove(&id);
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                log::debug!("[pty] route removal deferred for busy session {id}");
                let registry = Arc::clone(&self.registry);
                std::thread::spawn(move || {
                    let _lifecycle_guard = deferred_lifecycle
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    let mut registry = registry.lock().unwrap_or_else(|error| error.into_inner());
                    if registry
                        .routes
                        .get(&id)
                        .is_some_and(|entry| entry.kind == kind && entry.generation == generation)
                    {
                        registry.routes.remove(&id);
                    }
                });
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                log::warn!("[pty] route removal failed session={id} code=route_lifecycle_poisoned");
            }
        };
    }

    pub(crate) fn try_remove_route_if_kind(
        &self,
        id: Uuid,
        kind: SessionBackendKind,
    ) -> Result<(), PtyRouteRemovalError> {
        let route = {
            let registry = match self.registry.try_lock() {
                Ok(registry) => registry,
                Err(TryLockError::Poisoned(_)) => return Err(PtyRouteRemovalError::LockPoisoned),
                Err(TryLockError::WouldBlock) => return Err(PtyRouteRemovalError::Busy),
            };
            registry
                .routes
                .get(&id)
                .filter(|entry| entry.kind == kind)
                .map(|entry| (Arc::clone(&entry.lifecycle_gate), entry.generation))
        };
        let Some((lifecycle, generation)) = route else {
            return Ok(());
        };
        if self.terminal_output.release_session(id, kind).is_err() {
            return Err(PtyRouteRemovalError::LockPoisoned);
        }
        let _lifecycle_guard = lifecycle
            .try_lock()
            .map_err(|_| PtyRouteRemovalError::Busy)?;
        let mut registry = match self.registry.try_lock() {
            Ok(registry) => registry,
            Err(TryLockError::Poisoned(_)) => return Err(PtyRouteRemovalError::LockPoisoned),
            Err(TryLockError::WouldBlock) => return Err(PtyRouteRemovalError::Busy),
        };
        if registry
            .routes
            .get(&id)
            .is_some_and(|entry| entry.kind == kind && entry.generation == generation)
        {
            registry.routes.remove(&id);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn poison_route_registry_for_test(&self) {
        let registry = Arc::clone(&self.registry);
        let result = std::panic::catch_unwind(move || {
            let _registry_guard = registry.lock().unwrap();
            panic!("poison the PTY route registry for deterministic test coverage");
        });
        assert!(result.is_err(), "route-registry poison fixture must panic");
    }

    #[cfg(test)]
    pub(crate) fn clear_route_registry_poison_for_test(&self) {
        self.registry.clear_poison();
    }

    fn kind_for_session(&self, id: Uuid) -> Result<SessionBackendKind, AppError> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .get(&id)
            .map(|entry| entry.kind)
            .ok_or_else(|| AppError::SessionNotFound(id.to_string()))
    }

    pub fn backend_kind(&self, id: Uuid) -> Option<SessionBackendKind> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .get(&id)
            .map(|entry| entry.kind)
    }

    pub(crate) fn route_identities(
        &self,
        id: Uuid,
    ) -> Option<(
        Option<crate::path_identity::VerifiedPathIdentity>,
        Option<crate::path_identity::VerifiedPathIdentity>,
        u64,
    )> {
        let registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        let entry = registry.routes.get(&id)?;
        Some((
            entry.canonical_cwd_identity.clone(),
            entry.verified_replica_anchor.clone(),
            entry.generation,
        ))
    }

    pub(crate) fn has_pending_spawn_for_replica(
        &self,
        replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> bool {
        let registry = self
            .registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.pending.values().any(|pending| {
            crate::path_identity::verify_directory(std::path::Path::new(&pending.cwd))
                .is_ok_and(|cwd| crate::path_identity::is_verified_descendant(&cwd, replica))
        })
    }

    pub(crate) fn mark_spawning(&self, cwd: &str, label: &str) -> SpawnMark {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        let seq = registry.next_seq;
        registry.next_seq = registry.next_seq.wrapping_add(1);
        registry.pending.insert(
            seq,
            PendingSpawn {
                cwd: cwd.to_string(),
                label: label.to_string(),
            },
        );
        SpawnMark {
            registry: Arc::clone(&self.registry),
            seq,
        }
    }

    pub(crate) fn archive_liveness(&self, ids: &[Uuid]) -> (Vec<PendingSpawn>, Vec<bool>) {
        let (pending, route_kinds) = {
            let registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            (
                registry.pending.values().cloned().collect::<Vec<_>>(),
                ids.iter()
                    .map(|id| registry.routes.get(id).map(|entry| entry.kind))
                    .collect::<Vec<_>>(),
            )
        };
        let live = ids
            .iter()
            .zip(route_kinds)
            .map(|(id, kind)| {
                kind.map(|kind| self.backend_for_kind(kind).has_session(*id))
                    .unwrap_or(false)
            })
            .collect();
        (pending, live)
    }

    pub(crate) async fn spawn(
        manager: &Arc<Mutex<Self>>,
        backend_kind: SessionBackendKind,
        spec: BackendSpawnSpec,
    ) -> Result<(), AppError> {
        let id = spec.id;
        let cwd = spec.cwd.clone();
        let backend = {
            let manager = manager
                .lock()
                .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
            manager.backend_for_kind(backend_kind)
        };
        backend.spawn(spec).await?;
        let cwd_identity = match crate::path_identity::verify_directory(std::path::Path::new(&cwd))
        {
            Ok(identity) => identity,
            Err(_) => {
                if backend.kill(id).is_err() {
                    log::warn!("[pty-route] unsafe route cleanup failed session={id}");
                }
                return Err(AppError::PtyError("unsafe_route_cwd".to_string()));
            }
        };
        let verified_replica_anchor =
            crate::config::teams::verify_pty_input_replica_cwd(std::path::Path::new(&cwd))
                .ok()
                .map(|identity| identity.replica_identity);
        let route_result = match manager.lock() {
            Ok(manager) => manager.record_route_with_identities(
                id,
                backend_kind,
                Some(cwd_identity),
                verified_replica_anchor,
            ),
            Err(_) => Err(AppError::PtyError("pty_manager_poisoned".to_string())),
        };
        if let Err(error) = route_result {
            if backend.kill(id).is_err() {
                log::warn!("[pty-route] failed spawn route cleanup session={id}");
            }
            return Err(error);
        }
        Ok(())
    }

    /// Returns true if this manager holds a backend route whose backend has a
    /// live session handle. A true result guarantees that the same id will not
    /// fail `write` with `AppError::SessionNotFound` because routing is missing.
    pub fn has_session(&self, id: Uuid) -> bool {
        let Ok(kind) = self.kind_for_session(id) else {
            return false;
        };
        self.backend_for_kind(kind).has_session(id)
    }

    pub fn context_session_liveness(&self, id: Uuid) -> ContextSessionLiveness {
        let Ok(kind) = self.kind_for_session(id) else {
            return ContextSessionLiveness::SessionOver;
        };
        self.backend_for_kind(kind).context_session_liveness(id)
    }

    pub fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
        let kind = self.kind_for_session(id)?;
        self.backend_for_kind(kind).resize(id, cols, rows)
    }

    pub fn kill(&self, id: Uuid) -> Result<(), AppError> {
        let kind = self
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .get(&id)
            .map(|entry| entry.kind)
            .unwrap_or(SessionBackendKind::LocalProcess);
        self.kill_for_kind(id, kind)
    }

    /// Clean a pending create through its manager-record backend kind even when
    /// route registration never completed. Cancellation and shutdown rollback
    /// use this for both local and container-backed sessions.
    pub(crate) fn kill_for_kind(&self, id: Uuid, kind: SessionBackendKind) -> Result<(), AppError> {
        if let Err(error) = self.terminal_output.release_session(id, kind) {
            log::warn!(
                "[terminal-snapshot] event=session_owner_cleanup session={id} reason={}",
                error.code()
            );
        }
        let backend = self.backend_for_kind(kind);
        let result = backend.kill(id);
        if result.is_ok() || !backend.has_session(id) {
            self.remove_route_if_kind(id, kind);
        }
        result
    }

    pub fn terminate_job_for_session(&self, id: Uuid) -> bool {
        let Ok(kind) = self.kind_for_session(id) else {
            return false;
        };
        self.backend_for_kind(kind).terminate_job_for_session(id)
    }

    /// #942 - publish the pre-stop witness for a session AC is about to kill outside the
    /// PTY layer (resource-monitor tree kill). No-op when the session has no route.
    pub fn publish_stop_witness(&self, id: Uuid, source: &str) {
        let Ok(kind) = self.kind_for_session(id) else {
            return;
        };
        self.backend_for_kind(kind).publish_stop_witness(id, source);
    }

    pub fn kill_all_jobs(&self) -> (usize, usize) {
        self.backend_for_kind(SessionBackendKind::LocalProcess)
            .kill_all_jobs()
    }

    pub fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
        let kind = self.kind_for_session(id).ok()?;
        self.backend_for_kind(kind).get_screen_snapshot(id)
    }

    /// #1388 - forwards to the routed backend. An unrouted id yields `true` ("no
    /// claim"), matching the trait default.
    ///
    /// **This deliberately inverts its two nearest siblings**, which answer
    /// conservatively for an unrouted id: `has_session` returns `false` (`:589-594`)
    /// and `context_session_liveness` returns `SessionOver` (`:596-601`). Here
    /// "cannot tell" must not gate, which is the rule `backend.rs:231-235` already
    /// states for `Unavailable`. A permissive answer on a vanished route leads to an
    /// injection that fails loudly with `AppError::SessionNotFound`; `false` would
    /// silently hold a live session to the 90s cap on a routing transient. Fail loud
    /// beats stall silent.
    pub fn has_rendered_visible_content(&self, id: Uuid) -> bool {
        let Ok(kind) = self.kind_for_session(id) else {
            return true;
        };
        self.backend_for_kind(kind).has_rendered_visible_content(id)
    }

    pub fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        let kind = self.kind_for_session(id).ok()?;
        self.backend_for_kind(kind).get_pty_size(id)
    }

    /// #1032 - forwards to the routed backend. A missing route is not "we could not read":
    /// every route removal is preceded by parser removal, so the session really is over.
    pub fn get_screen_rows(&self, id: Uuid) -> ScreenRowsRead {
        let Ok(kind) = self.kind_for_session(id) else {
            return ScreenRowsRead::SessionOver;
        };
        self.backend_for_kind(kind).get_screen_rows(id)
    }

    /// #1171 - forwards to the routed backend. A missing route is `Gone` for the same reason
    /// it is `SessionOver` above: every route removal is preceded by parser removal, so the
    /// session really is over.
    ///
    /// **The watcher engine does not call this on its tick.** It resolves the backend `Arc`
    /// once at registration through `backend_for_kind` and calls the backend directly, which
    /// is what keeps both this mutex and the `registry` mutex out of a 200 ms loop. This
    /// exists for completeness and for callers that hold no `Arc` of their own.
    pub fn screen_rows_since(
        &self,
        id: Uuid,
        seen: Option<crate::pty::watchers::FrameStamp>,
    ) -> crate::pty::watchers::ScreenRowsSince {
        let Ok(kind) = self.kind_for_session(id) else {
            return crate::pty::watchers::ScreenRowsSince::Gone;
        };
        self.backend_for_kind(kind).screen_rows_since(id, seen)
    }

    pub fn register_response_watcher(
        &self,
        session_id: Uuid,
        request_id: String,
        response_dir: std::path::PathBuf,
    ) {
        let kind = self
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .routes
            .get(&session_id)
            .map(|entry| entry.kind)
            .unwrap_or(SessionBackendKind::LocalProcess);
        self.backend_for_kind(kind)
            .register_response_watcher(session_id, request_id, response_dir);
    }

    pub async fn acquire_input_writer(
        manager: &Arc<std::sync::Mutex<PtyManager>>,
        session_id: Uuid,
    ) -> Result<PtyInputPermit, AppError> {
        acquire_input_writer(manager, session_id).await
    }

    pub(crate) fn authority_route_proof(
        manager: &Arc<std::sync::Mutex<PtyManager>>,
        session_id: Uuid,
    ) -> Result<PtyAuthorityRouteProof, AppError> {
        authority_route_proof(manager, session_id)
    }

    pub(crate) fn snapshot_route_proof(
        manager: &Arc<std::sync::Mutex<PtyManager>>,
        session_id: Uuid,
    ) -> Result<PtySnapshotRouteProof, AppError> {
        snapshot_route_proof(manager, session_id)
    }

    pub(crate) fn snapshot_route_proofs(
        manager: &Arc<std::sync::Mutex<PtyManager>>,
        session_ids: &[Uuid],
    ) -> Result<Vec<Option<PtySnapshotRouteProof>>, AppError> {
        let manager_guard = manager
            .lock()
            .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
        let registry = manager_guard
            .registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let mut proofs = Vec::new();
        proofs
            .try_reserve_exact(session_ids.len())
            .map_err(|_| AppError::PtyError("snapshot_route_capacity".to_string()))?;
        for session_id in session_ids {
            let proof = registry.routes.get(session_id).and_then(|entry| {
                Some(PtySnapshotRouteProof {
                    session_id: *session_id,
                    route_generation: entry.generation,
                    route_kind: entry.kind,
                    route_registry: Arc::clone(&manager_guard.registry),
                    route_lifecycle: Arc::clone(&entry.lifecycle_gate),
                    backend: manager_guard.backend_for_kind(entry.kind),
                    saved_cwd: entry.canonical_cwd_identity.clone()?,
                    saved_replica: entry.verified_replica_anchor.clone(),
                })
            });
            proofs.push(proof);
        }
        Ok(proofs)
    }

    pub fn lock_route_for_write(
        permit: &PtyInputPermit,
    ) -> Result<PtyRouteWriteGuard<'_>, AppError> {
        lock_route_for_write(permit)
    }

    pub(crate) fn lock_route_for_verified_write<'a>(
        permit: &'a PtyInputPermit,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> Result<PtyRouteWriteGuard<'a>, AppError> {
        lock_route_for_verified_write(permit, expected_kind, expected_cwd, expected_replica)
    }

    pub fn write_with_permit(permit: &PtyInputPermit, bytes: &[u8]) -> Result<(), AppError> {
        write_with_permit(permit, bytes)
    }
}

fn snapshot_route_proof(
    manager: &Arc<std::sync::Mutex<PtyManager>>,
    session_id: Uuid,
) -> Result<PtySnapshotRouteProof, AppError> {
    let manager_guard = manager
        .lock()
        .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
    let registry = manager_guard
        .registry
        .lock()
        .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
    let entry = registry
        .routes
        .get(&session_id)
        .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
    let saved_cwd = entry
        .canonical_cwd_identity
        .clone()
        .ok_or_else(|| AppError::PtyError("unsafe_route_cwd".to_string()))?;
    let saved_replica = entry.verified_replica_anchor.clone();
    Ok(PtySnapshotRouteProof {
        session_id,
        route_generation: entry.generation,
        route_kind: entry.kind,
        route_registry: Arc::clone(&manager_guard.registry),
        route_lifecycle: Arc::clone(&entry.lifecycle_gate),
        backend: manager_guard.backend_for_kind(entry.kind),
        saved_cwd,
        saved_replica,
    })
}

fn authority_route_proof(
    manager: &Arc<std::sync::Mutex<PtyManager>>,
    session_id: Uuid,
) -> Result<PtyAuthorityRouteProof, AppError> {
    let manager_guard = manager
        .lock()
        .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
    let registry = manager_guard
        .registry
        .lock()
        .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
    let entry = registry
        .routes
        .get(&session_id)
        .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
    Ok(PtyAuthorityRouteProof {
        session_id,
        route_generation: entry.generation,
        route_registry: Arc::clone(&manager_guard.registry),
        route_lifecycle: Arc::clone(&entry.lifecycle_gate),
    })
}

pub async fn acquire_input_writer(
    manager: &Arc<std::sync::Mutex<PtyManager>>,
    session_id: Uuid,
) -> Result<PtyInputPermit, AppError> {
    let (route_registry, input_gate, route_lifecycle, generation, backend) = {
        let manager_guard = manager
            .lock()
            .map_err(|_| AppError::PtyError("pty_manager_poisoned".to_string()))?;
        let registry = manager_guard
            .registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let entry = registry
            .routes
            .get(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        (
            Arc::clone(&manager_guard.registry),
            Arc::clone(&entry.input_gate),
            Arc::clone(&entry.lifecycle_gate),
            entry.generation,
            manager_guard.backend_for_kind(entry.kind),
        )
    };

    let guard = input_gate.lock_owned().await;
    {
        let registry = route_registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let current = registry
            .routes
            .get(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        if current.generation != generation {
            return Err(AppError::PtyError("stale_pty_route".to_string()));
        }
    }

    Ok(PtyInputPermit {
        session_id,
        route_generation: generation,
        route_registry,
        route_lifecycle,
        backend,
        _guard: guard,
    })
}

fn acquire_route_lifecycle(
    gate: &std::sync::Mutex<()>,
) -> Result<std::sync::MutexGuard<'_, ()>, AppError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
    loop {
        match gate.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(AppError::PtyError("pty_route_poisoned".to_string()))
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Err(AppError::PtyError("pty_route_busy".to_string()));
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}

impl PtySnapshotRouteProof {
    pub(crate) fn backend_kind(&self) -> SessionBackendKind {
        self.route_kind
    }

    pub(crate) fn liveness(&self) -> ContextSessionLiveness {
        self.backend.context_session_liveness(self.session_id)
    }

    pub(crate) fn saved_cwd(&self) -> &crate::path_identity::VerifiedPathIdentity {
        &self.saved_cwd
    }

    pub(crate) fn saved_replica(&self) -> Option<&crate::path_identity::VerifiedPathIdentity> {
        self.saved_replica.as_ref()
    }

    pub(crate) fn matches_requester_route(
        &self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: Option<&crate::path_identity::VerifiedPathIdentity>,
    ) -> bool {
        let Ok(registry) = self.route_registry.lock() else {
            return false;
        };
        registry
            .routes
            .get(&self.session_id)
            .is_some_and(|current| {
                let replica_matches = match (
                    current.verified_replica_anchor.as_ref(),
                    self.saved_replica.as_ref(),
                    expected_replica,
                ) {
                    (Some(current), Some(saved), Some(expected)) => {
                        crate::path_identity::same_object(current, saved)
                            && crate::path_identity::same_object(current, expected)
                    }
                    (None, None, None) => true,
                    _ => false,
                };
                current.generation == self.route_generation
                    && current.kind == self.route_kind
                    && current.kind == expected_kind
                    && current
                        .canonical_cwd_identity
                        .as_ref()
                        .is_some_and(|identity| {
                            crate::path_identity::same_object(identity, expected_cwd)
                                && crate::path_identity::same_object(identity, &self.saved_cwd)
                        })
                    && replica_matches
            })
    }

    pub(crate) fn capture_verified(
        &self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> TerminalScreenRead {
        let lifecycle_guard = match acquire_route_lifecycle(&self.route_lifecycle) {
            Ok(guard) => guard,
            Err(_) => return TerminalScreenRead::Unavailable,
        };
        {
            let registry = match self.route_registry.lock() {
                Ok(registry) => registry,
                Err(_) => return TerminalScreenRead::Unavailable,
            };
            let Some(current) = registry.routes.get(&self.session_id) else {
                return TerminalScreenRead::Unavailable;
            };
            let cwd_matches = current
                .canonical_cwd_identity
                .as_ref()
                .is_some_and(|identity| {
                    crate::path_identity::same_object(identity, expected_cwd)
                        && crate::path_identity::same_object(identity, &self.saved_cwd)
                });
            let replica_matches =
                current
                    .verified_replica_anchor
                    .as_ref()
                    .is_some_and(|identity| {
                        crate::path_identity::same_object(identity, expected_replica)
                            && self.saved_replica.as_ref().is_some_and(|saved| {
                                crate::path_identity::same_object(identity, saved)
                            })
                    });
            if current.generation != self.route_generation
                || current.kind != self.route_kind
                || current.kind != expected_kind
                || !cwd_matches
                || !replica_matches
            {
                return TerminalScreenRead::Unavailable;
            }
        }
        let copied = self.backend.copy_terminal_screen(self.session_id);
        drop(lifecycle_guard);
        match copied {
            TerminalScreenCopyRead::Copied(captured) => captured
                .into_model(self.session_id, self.route_kind)
                .map(TerminalScreenRead::Captured)
                .unwrap_or(TerminalScreenRead::Unavailable),
            TerminalScreenCopyRead::Unavailable => TerminalScreenRead::Unavailable,
            TerminalScreenCopyRead::TooLarge => TerminalScreenRead::TooLarge,
        }
    }

    pub(crate) fn matches_current(
        &self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: &crate::path_identity::VerifiedPathIdentity,
    ) -> bool {
        let Ok(registry) = self.route_registry.lock() else {
            return false;
        };
        registry
            .routes
            .get(&self.session_id)
            .is_some_and(|current| {
                current.generation == self.route_generation
                    && current.kind == self.route_kind
                    && current.kind == expected_kind
                    && current
                        .canonical_cwd_identity
                        .as_ref()
                        .is_some_and(|identity| {
                            crate::path_identity::same_object(identity, expected_cwd)
                                && crate::path_identity::same_object(identity, &self.saved_cwd)
                        })
                    && current
                        .verified_replica_anchor
                        .as_ref()
                        .is_some_and(|identity| {
                            crate::path_identity::same_object(identity, expected_replica)
                                && self.saved_replica.as_ref().is_some_and(|saved| {
                                    crate::path_identity::same_object(identity, saved)
                                })
                        })
            })
    }
}

impl PtyAuthorityRouteProof {
    pub(crate) fn lock_verified<'a>(
        &'a self,
        expected_kind: SessionBackendKind,
        expected_cwd: &crate::path_identity::VerifiedPathIdentity,
        expected_replica: Option<&crate::path_identity::VerifiedPathIdentity>,
    ) -> Result<std::sync::MutexGuard<'a, ()>, AppError> {
        let lifecycle_guard = acquire_route_lifecycle(&self.route_lifecycle)?;
        {
            let registry = self
                .route_registry
                .lock()
                .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
            let current = registry
                .routes
                .get(&self.session_id)
                .ok_or_else(|| AppError::SessionNotFound(self.session_id.to_string()))?;
            let cwd_matches = current
                .canonical_cwd_identity
                .as_ref()
                .is_some_and(|identity| crate::path_identity::same_object(identity, expected_cwd));
            let replica_matches = match (current.verified_replica_anchor.as_ref(), expected_replica)
            {
                (Some(current), Some(expected)) => {
                    crate::path_identity::same_object(current, expected)
                }
                (None, None) => true,
                _ => false,
            };
            if current.generation != self.route_generation
                || current.kind != expected_kind
                || !cwd_matches
                || !replica_matches
            {
                return Err(AppError::PtyError("stale_pty_route".to_string()));
            }
        }
        Ok(lifecycle_guard)
    }
}

pub fn lock_route_for_write(permit: &PtyInputPermit) -> Result<PtyRouteWriteGuard<'_>, AppError> {
    let lifecycle_guard = acquire_route_lifecycle(&permit.route_lifecycle)?;
    {
        let registry = permit
            .route_registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let current = registry
            .routes
            .get(&permit.session_id)
            .ok_or_else(|| AppError::SessionNotFound(permit.session_id.to_string()))?;
        if current.generation != permit.route_generation {
            return Err(AppError::PtyError("stale_pty_route".to_string()));
        }
    }
    Ok(PtyRouteWriteGuard {
        session_id: permit.session_id,
        backend: Arc::clone(&permit.backend),
        _guard: lifecycle_guard,
        _authority_guard: None,
        _settings_guard: None,
    })
}

pub(crate) fn lock_route_for_verified_write<'a>(
    permit: &'a PtyInputPermit,
    expected_kind: SessionBackendKind,
    expected_cwd: &crate::path_identity::VerifiedPathIdentity,
    expected_replica: &crate::path_identity::VerifiedPathIdentity,
) -> Result<PtyRouteWriteGuard<'a>, AppError> {
    let lifecycle_guard = acquire_route_lifecycle(&permit.route_lifecycle)?;
    {
        let registry = permit
            .route_registry
            .lock()
            .map_err(|_| AppError::PtyError("route_registry_poisoned".to_string()))?;
        let current = registry
            .routes
            .get(&permit.session_id)
            .ok_or_else(|| AppError::SessionNotFound(permit.session_id.to_string()))?;
        let cwd_matches = current
            .canonical_cwd_identity
            .as_ref()
            .is_some_and(|identity| crate::path_identity::same_object(identity, expected_cwd));
        let replica_matches = current
            .verified_replica_anchor
            .as_ref()
            .is_some_and(|identity| crate::path_identity::same_object(identity, expected_replica));
        if current.generation != permit.route_generation
            || current.kind != expected_kind
            || !cwd_matches
            || !replica_matches
        {
            return Err(AppError::PtyError("stale_pty_route".to_string()));
        }
    }
    Ok(PtyRouteWriteGuard {
        session_id: permit.session_id,
        backend: Arc::clone(&permit.backend),
        _guard: lifecycle_guard,
        _authority_guard: None,
        _settings_guard: None,
    })
}

pub fn write_with_permit(permit: &PtyInputPermit, bytes: &[u8]) -> Result<(), AppError> {
    lock_route_for_write(permit)?.write(bytes)
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        self.local_backend.shutdown_terminal_output();
        self.container_backend.shutdown_terminal_output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Write(Uuid, Vec<u8>),
        Resize(Uuid, u16, u16),
        Kill(Uuid),
        Has(Uuid),
        Snapshot(Uuid),
        Size(Uuid),
        Watcher(Uuid, String),
        TerminateJob(Uuid),
        Activate(Uuid, String, bool, u64, u32),
        Detach(Uuid, String),
        ReleaseWindow(String),
    }

    #[derive(Default)]
    struct RecordingBackend {
        calls: Mutex<Vec<Call>>,
        live: Mutex<std::collections::HashSet<Uuid>>,
        fail_next_activation: Mutex<bool>,
        activation_block:
            Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    }

    impl RecordingBackend {
        fn set_live(&self, id: Uuid) {
            self.live.lock().unwrap().insert(id);
        }

        fn calls(&self) -> Vec<Call> {
            let mut calls = self.calls.lock().unwrap();
            std::mem::take(&mut *calls)
        }

        fn fail_next_activation(&self) {
            *self.fail_next_activation.lock().unwrap() = true;
        }

        fn block_next_activation(
            &self,
            started: std::sync::mpsc::Sender<()>,
            release: std::sync::mpsc::Receiver<()>,
        ) {
            *self.activation_block.lock().unwrap() = Some((started, release));
        }
    }

    impl PtyBackend for RecordingBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
            Box::pin(async move {
                self.live.lock().unwrap().insert(spec.id);
                Ok(())
            })
        }

        fn write(
            &self,
            _authority: &BackendWriteAuthority,
            id: Uuid,
            data: &[u8],
        ) -> Result<(), AppError> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Write(id, data.to_vec()));
            Ok(())
        }

        fn resize(&self, id: Uuid, cols: u16, rows: u16) -> Result<(), AppError> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Resize(id, cols, rows));
            Ok(())
        }

        fn kill(&self, id: Uuid) -> Result<(), AppError> {
            self.calls.lock().unwrap().push(Call::Kill(id));
            self.live.lock().unwrap().remove(&id);
            Ok(())
        }

        fn has_session(&self, id: Uuid) -> bool {
            self.calls.lock().unwrap().push(Call::Has(id));
            self.live.lock().unwrap().contains(&id)
        }

        fn get_screen_snapshot(&self, id: Uuid) -> Option<PtyScreenSnapshot> {
            self.calls.lock().unwrap().push(Call::Snapshot(id));
            None
        }

        fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
            self.calls.lock().unwrap().push(Call::Size(id));
            Some((30, 120))
        }

        fn get_screen_rows(&self, _id: Uuid) -> ScreenRowsRead {
            ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            session_id: Uuid,
            request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Watcher(session_id, request_id));
        }

        fn terminate_job_for_session(&self, id: Uuid) -> bool {
            self.calls.lock().unwrap().push(Call::TerminateJob(id));
            true
        }

        fn activate_terminal_output(
            &self,
            id: Uuid,
            label: &str,
            include_history: bool,
            document_epoch: u64,
            attach_generation: u32,
        ) -> Result<PtyTerminalOutputActivation, TerminalOutputAttachError> {
            self.calls.lock().unwrap().push(Call::Activate(
                id,
                label.to_string(),
                include_history,
                document_epoch,
                attach_generation,
            ));
            if let Some((started, release)) = self.activation_block.lock().unwrap().take() {
                let _ = started.send(());
                let _ = release.recv();
            }
            if std::mem::take(&mut *self.fail_next_activation.lock().unwrap()) {
                return Err(TerminalOutputAttachError::SessionUnavailable);
            }
            Ok(PtyTerminalOutputActivation {
                snapshot: None,
                seedless_reason: Some(
                    crate::pty::output::PtyTerminalSeedlessReason::SeedlessParserUnavailable,
                ),
                attach_generation,
                document_epoch,
            })
        }

        fn detach_terminal_output(&self, id: Uuid, label: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Detach(id, label.to_string()));
        }

        fn release_window_attachments(&self, label: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(Call::ReleaseWindow(label.to_string()));
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (1, 2)
        }
    }

    fn output_coordinator_fixture() -> (
        TerminalOutputCoordinator,
        Arc<Mutex<SpawnRegistry>>,
        Arc<RecordingBackend>,
        Arc<RecordingBackend>,
    ) {
        let routes = Arc::new(Mutex::new(SpawnRegistry::default()));
        let local = Arc::new(RecordingBackend::default());
        let container = Arc::new(RecordingBackend::default());
        let local_backend: Arc<dyn PtyBackend> = local.clone();
        let container_backend: Arc<dyn PtyBackend> = container.clone();
        (
            TerminalOutputCoordinator::new(Arc::clone(&routes), local_backend, container_backend),
            routes,
            local,
            container,
        )
    }

    fn insert_output_route(routes: &Arc<Mutex<SpawnRegistry>>, id: Uuid, kind: SessionBackendKind) {
        let mut routes = routes.lock().unwrap();
        routes.next_route_generation += 1;
        let generation = routes.next_route_generation;
        routes.routes.insert(
            id,
            RouteEntry {
                kind,
                generation,
                input_gate: Arc::new(tokio::sync::Mutex::new(())),
                lifecycle_gate: Arc::new(std::sync::Mutex::new(())),
                canonical_cwd_identity: None,
                verified_replica_anchor: None,
            },
        );
    }

    #[test]
    fn terminal_output_document_ownership() {
        const LABEL: &str = "main";
        let (coordinator, routes, local, container) = output_coordinator_fixture();
        let local_a = Uuid::new_v4();
        let local_b = Uuid::new_v4();
        let container_c = Uuid::new_v4();
        insert_output_route(&routes, local_a, SessionBackendKind::LocalProcess);
        insert_output_route(&routes, local_b, SessionBackendKind::LocalProcess);
        insert_output_route(&routes, container_c, SessionBackendKind::ContainerTransport);

        let first_epoch = coordinator.begin_document(LABEL).expect("first document");
        let other_epoch = coordinator
            .begin_document("other")
            .expect("second document");
        assert_eq!(first_epoch, 1);
        assert_eq!(other_epoch, 2);
        assert_eq!(coordinator.document_epoch(LABEL), Ok(first_epoch));
        assert_eq!(
            coordinator.document_epoch("missing"),
            Err(TerminalOutputCoordinationError::DocumentUnavailable)
        );
        local.calls();
        container.calls();

        let local_activation = coordinator
            .activate(LABEL, local_a, first_epoch, 1, true)
            .expect("local activation");
        assert_eq!(local_activation.document_epoch, first_epoch);
        assert_eq!(local_activation.attach_generation, 1);
        assert_eq!(
            local.calls(),
            vec![Call::Activate(
                local_a,
                LABEL.to_string(),
                true,
                first_epoch,
                1,
            )]
        );
        assert!(container.calls().is_empty());

        let container_activation = coordinator
            .activate(LABEL, container_c, first_epoch, 2, false)
            .expect("container activation");
        assert_eq!(container_activation.document_epoch, first_epoch);
        assert_eq!(container_activation.attach_generation, 2);
        assert_eq!(
            local.calls(),
            vec![Call::Detach(local_a, LABEL.to_string())]
        );
        assert_eq!(
            container.calls(),
            vec![Call::Activate(
                container_c,
                LABEL.to_string(),
                false,
                first_epoch,
                2,
            )]
        );

        coordinator
            .detach(LABEL, local_a, first_epoch, 1)
            .expect("stale detach");
        assert!(local.calls().is_empty());
        assert!(container.calls().is_empty());
        assert!(matches!(
            coordinator.activate(LABEL, local_b, first_epoch, 2, true),
            Err(TerminalOutputCoordinationError::StaleAttachGeneration)
        ));
        assert!(local.calls().is_empty());
        coordinator
            .detach(LABEL, container_c, first_epoch, 2)
            .expect("exact detach");
        assert_eq!(
            container.calls(),
            vec![Call::Detach(container_c, LABEL.to_string())]
        );

        coordinator
            .cancel(LABEL, local_b, first_epoch, 5)
            .expect("pre-activation cancellation");
        assert!(matches!(
            coordinator.activate(LABEL, local_b, first_epoch, 5, true),
            Err(TerminalOutputCoordinationError::StaleAttachGeneration)
        ));
        assert!(local.calls().is_empty());
        coordinator
            .activate(LABEL, local_b, first_epoch, 6, true)
            .expect("post-tombstone activation");
        assert!(matches!(
            coordinator.activate(LABEL, local_a, other_epoch, 7, true),
            Err(TerminalOutputCoordinationError::StaleDocumentEpoch)
        ));
        assert_eq!(local.calls().len(), 1);
        coordinator
            .release_session(local_b, SessionBackendKind::LocalProcess)
            .expect("session owner release");
        assert_eq!(
            local.calls(),
            vec![Call::Detach(local_b, LABEL.to_string())]
        );

        let unknown = Uuid::new_v4();
        assert!(matches!(
            coordinator.activate(LABEL, unknown, first_epoch, 7, true),
            Err(TerminalOutputCoordinationError::RouteUnavailable)
        ));
        coordinator
            .activate(LABEL, local_a, first_epoch, 7, true)
            .expect("route error did not consume generation");
        local.calls();

        let rotated_epoch = coordinator
            .begin_document(LABEL)
            .expect("page-load rotation");
        assert!(rotated_epoch > other_epoch);
        assert!(matches!(
            coordinator.activate(LABEL, local_a, first_epoch, 8, true),
            Err(TerminalOutputCoordinationError::StaleDocumentEpoch)
        ));
        coordinator
            .activate(LABEL, local_a, rotated_epoch, 1, true)
            .expect("generation one in new document");
        local.calls();
        container.calls();

        coordinator
            .release_window(LABEL)
            .expect("destroyed window cleanup");
        assert_eq!(
            coordinator.document_epoch(LABEL),
            Err(TerminalOutputCoordinationError::DocumentUnavailable)
        );
        let reused_epoch = coordinator
            .begin_document(LABEL)
            .expect("destroyed label reuse");
        assert!(reused_epoch > rotated_epoch);
        coordinator
            .activate(LABEL, local_a, reused_epoch, 1, true)
            .expect("reused-label activation");
        local.calls();
        container.calls();

        local.fail_next_activation();
        assert!(matches!(
            coordinator.activate(LABEL, local_b, reused_epoch, 2, true),
            Err(TerminalOutputCoordinationError::Backend(
                TerminalOutputAttachError::SessionUnavailable
            ))
        ));
        assert_eq!(
            local.calls(),
            vec![
                Call::Detach(local_a, LABEL.to_string()),
                Call::Activate(local_b, LABEL.to_string(), true, reused_epoch, 2),
                Call::Detach(local_b, LABEL.to_string()),
            ]
        );
        let state = coordinator.state.lock().unwrap();
        let failed_view = state.views.get(LABEL).unwrap();
        assert_eq!(
            failed_view.generation,
            Some(ViewOutputGenerationState::Failed(ViewOutputGenerationKey {
                session_id: local_b,
                generation: 2,
            }))
        );
        assert_eq!(failed_view.high_water_generation, 2);
        drop(state);

        let race_epoch = coordinator.begin_document("race").expect("race document");
        local.calls();
        container.calls();
        let (activation_started_tx, activation_started_rx) = std::sync::mpsc::channel();
        let (activation_release_tx, activation_release_rx) = std::sync::mpsc::channel();
        local.block_next_activation(activation_started_tx, activation_release_rx);
        let activation_coordinator = coordinator.clone();
        let activation = std::thread::spawn(move || {
            activation_coordinator.activate("race", local_a, race_epoch, 1, true)
        });
        activation_started_rx
            .recv()
            .expect("activation entered backend");
        let (cancel_started_tx, cancel_started_rx) = std::sync::mpsc::channel();
        let cancel_coordinator = coordinator.clone();
        let cancel = std::thread::spawn(move || {
            cancel_started_tx.send(()).unwrap();
            cancel_coordinator.cancel("race", local_a, race_epoch, 1)
        });
        cancel_started_rx.recv().expect("cancel attempted");
        activation_release_tx.send(()).unwrap();
        activation
            .join()
            .expect("activation thread")
            .expect("activation result");
        cancel
            .join()
            .expect("cancel thread")
            .expect("cancel result");
        assert_eq!(
            local.calls(),
            vec![
                Call::Activate(local_a, "race".to_string(), true, race_epoch, 1),
                Call::Detach(local_a, "race".to_string()),
            ]
        );

        let page_epoch = coordinator
            .begin_document("page")
            .expect("in-flight page document");
        local.calls();
        container.calls();
        let (page_started_tx, page_started_rx) = std::sync::mpsc::channel();
        let (page_release_tx, page_release_rx) = std::sync::mpsc::channel();
        container.block_next_activation(page_started_tx, page_release_rx);
        let page_activation_coordinator = coordinator.clone();
        let page_activation = std::thread::spawn(move || {
            page_activation_coordinator.activate("page", container_c, page_epoch, 1, true)
        });
        page_started_rx.recv().expect("page activation started");
        let page_rotation_coordinator = coordinator.clone();
        let page_rotation =
            std::thread::spawn(move || page_rotation_coordinator.begin_document("page"));
        page_release_tx.send(()).unwrap();
        page_activation
            .join()
            .expect("page activation thread")
            .expect("page activation result");
        let next_page_epoch = page_rotation
            .join()
            .expect("page rotation thread")
            .expect("page rotation result");
        assert!(next_page_epoch > page_epoch);
        assert!(coordinator
            .state
            .lock()
            .unwrap()
            .views
            .get("page")
            .unwrap()
            .generation
            .is_none());
        coordinator
            .activate("page", container_c, next_page_epoch, 1, true)
            .expect("new-page generation one");

        let max_epoch = coordinator
            .begin_document("max-generation")
            .expect("max generation document");
        coordinator
            .activate("max-generation", local_a, max_epoch, u32::MAX, true)
            .expect("maximum generation");
        assert!(matches!(
            coordinator.activate("max-generation", local_b, max_epoch, u32::MAX, true,),
            Err(TerminalOutputCoordinationError::StaleAttachGeneration)
        ));
        assert!(coordinator.state.lock().unwrap().views.len() <= 6);

        {
            let mut registry = coordinator.state.lock().unwrap();
            registry.next_document_epoch = u64::MAX;
        }
        assert_eq!(
            coordinator.begin_document("epoch-overflow"),
            Err(TerminalOutputCoordinationError::DocumentEpochExhausted)
        );
        assert!(!coordinator
            .state
            .lock()
            .unwrap()
            .views
            .contains_key("epoch-overflow"));

        let manager_backend = Arc::new(RecordingBackend::default());
        let manager_backend_trait: Arc<dyn PtyBackend> = manager_backend.clone();
        let manager = PtyManager::new_for_test(manager_backend_trait);
        let removed = Uuid::new_v4();
        manager_backend.set_live(removed);
        manager.record_route(removed, SessionBackendKind::LocalProcess);
        let manager_coordinator = manager.terminal_output_coordinator();
        let manager_epoch = manager_coordinator
            .begin_document("removed")
            .expect("removed document");
        manager_coordinator
            .activate("removed", removed, manager_epoch, 1, true)
            .expect("removed activation");
        manager_backend.calls();
        manager
            .kill_for_kind(removed, SessionBackendKind::LocalProcess)
            .expect("session removal");
        assert_eq!(
            manager_backend.calls(),
            vec![
                Call::Detach(removed, "removed".to_string()),
                Call::Kill(removed),
            ]
        );
    }

    #[test]
    fn terminal_output_observation_ownership() {
        use TerminalOutputObservationError as ObservationError;
        use TerminalOutputObservationStage as Stage;

        assert_eq!(Stage::PostWrite.code(), "postWrite");
        assert_eq!(Stage::PostFit.code(), "postFit");
        assert_eq!(Stage::Settled.code(), "settled");
        assert_eq!(Stage::Aborted.code(), "aborted");
        assert_eq!(
            ObservationError::RegistryPoisoned.code(),
            "terminalOutputRegistryPoisoned"
        );
        assert_eq!(
            ObservationError::DocumentUnavailable.code(),
            "documentUnavailable"
        );
        assert_eq!(
            ObservationError::StaleDocumentEpoch.code(),
            "staleDocumentEpoch"
        );
        assert_eq!(
            ObservationError::StaleIdentity.code(),
            "staleObservationGeneration"
        );
        assert_eq!(
            ObservationError::StageOrderInvalid.code(),
            "observationStageOrderInvalid"
        );
        assert_eq!(
            ObservationError::CardinalityExceeded.code(),
            "observationCardinalityExceeded"
        );

        let (coordinator, routes, local, _container) = output_coordinator_fixture();
        let owner = Uuid::new_v4();
        let replacement = Uuid::new_v4();
        let failed = Uuid::new_v4();
        let removed = Uuid::new_v4();
        let race_old = Uuid::new_v4();
        let race_new = Uuid::new_v4();
        for session_id in [owner, replacement, failed, removed, race_old, race_new] {
            insert_output_route(&routes, session_id, SessionBackendKind::LocalProcess);
        }

        assert_eq!(
            coordinator.accept_observation_stage("missing", owner, 1, 1, Stage::PostWrite),
            Err(ObservationError::DocumentUnavailable)
        );

        let owner_label = "observation-owner";
        let owner_epoch = coordinator
            .begin_document(owner_label)
            .expect("owner document");
        coordinator
            .activate(owner_label, owner, owner_epoch, 1, true)
            .expect("owner activation");
        coordinator
            .accept_observation_stage(owner_label, owner, owner_epoch, 1, Stage::PostWrite)
            .expect("owner postWrite");
        coordinator
            .accept_observation_stage(owner_label, owner, owner_epoch, 1, Stage::PostFit)
            .expect("owner postFit");
        coordinator
            .accept_observation_stage(owner_label, owner, owner_epoch, 1, Stage::Settled)
            .expect("owner settled");
        assert_eq!(
            coordinator.accept_observation_stage(
                owner_label,
                owner,
                owner_epoch,
                1,
                Stage::Settled,
            ),
            Err(ObservationError::CardinalityExceeded)
        );
        assert_eq!(
            coordinator.accept_observation_stage(
                owner_label,
                owner,
                owner_epoch,
                1,
                Stage::Aborted,
            ),
            Err(ObservationError::CardinalityExceeded)
        );

        let early_label = "observation-early-abort";
        let early_epoch = coordinator
            .begin_document(early_label)
            .expect("early-abort document");
        coordinator
            .activate(early_label, owner, early_epoch, 1, true)
            .expect("early-abort activation");
        coordinator
            .accept_observation_stage(early_label, owner, early_epoch, 1, Stage::Aborted)
            .expect("early abort");
        assert_eq!(
            coordinator.accept_observation_stage(
                early_label,
                owner,
                early_epoch,
                1,
                Stage::PostWrite,
            ),
            Err(ObservationError::CardinalityExceeded)
        );

        let order_label = "observation-order";
        let order_epoch = coordinator
            .begin_document(order_label)
            .expect("order document");
        coordinator
            .activate(order_label, owner, order_epoch, 1, true)
            .expect("order activation");
        assert_eq!(
            coordinator.accept_observation_stage(
                order_label,
                owner,
                order_epoch,
                1,
                Stage::PostFit,
            ),
            Err(ObservationError::StageOrderInvalid)
        );
        assert_eq!(
            coordinator.accept_observation_stage(
                order_label,
                owner,
                order_epoch,
                1,
                Stage::Settled,
            ),
            Err(ObservationError::StageOrderInvalid)
        );
        coordinator
            .accept_observation_stage(order_label, owner, order_epoch, 1, Stage::PostWrite)
            .expect("ordered postWrite");
        assert_eq!(
            coordinator.accept_observation_stage(
                order_label,
                owner,
                order_epoch,
                1,
                Stage::PostWrite,
            ),
            Err(ObservationError::StageOrderInvalid)
        );
        assert_eq!(
            coordinator.accept_observation_stage(
                order_label,
                owner,
                order_epoch,
                1,
                Stage::Settled,
            ),
            Err(ObservationError::StageOrderInvalid)
        );
        coordinator
            .accept_observation_stage(order_label, owner, order_epoch, 1, Stage::PostFit)
            .expect("ordered postFit");
        assert_eq!(
            coordinator.accept_observation_stage(
                order_label,
                owner,
                order_epoch,
                1,
                Stage::PostFit,
            ),
            Err(ObservationError::StageOrderInvalid)
        );
        coordinator
            .accept_observation_stage(order_label, owner, order_epoch, 1, Stage::Settled)
            .expect("ordered settled");

        let identity_label = "observation-identity";
        let identity_epoch = coordinator
            .begin_document(identity_label)
            .expect("identity document");
        coordinator
            .activate(identity_label, owner, identity_epoch, 7, true)
            .expect("identity activation");
        assert_eq!(
            coordinator.accept_observation_stage(
                identity_label,
                owner,
                identity_epoch + 1,
                7,
                Stage::PostWrite,
            ),
            Err(ObservationError::StaleDocumentEpoch)
        );
        for (session_id, generation) in [(replacement, 7), (owner, 0), (owner, 6), (owner, 8)] {
            assert_eq!(
                coordinator.accept_observation_stage(
                    identity_label,
                    session_id,
                    identity_epoch,
                    generation,
                    Stage::PostWrite,
                ),
                Err(ObservationError::StaleIdentity)
            );
        }

        let bounded_label = "observation-bounded";
        let bounded_epoch = coordinator
            .begin_document(bounded_label)
            .expect("bounded document");
        for generation in 1..=4097_u32 {
            coordinator
                .cancel(bounded_label, owner, bounded_epoch, generation)
                .expect("sequential generation cancellation");
            coordinator
                .accept_observation_stage(
                    bounded_label,
                    owner,
                    bounded_epoch,
                    generation,
                    Stage::Aborted,
                )
                .expect("sequential generation observation");
        }
        {
            let registry = coordinator.state.lock().unwrap();
            let view = registry.views.get(bounded_label).unwrap();
            let key = ViewOutputGenerationKey {
                session_id: owner,
                generation: 4097,
            };
            assert_eq!(view.high_water_generation, 4097);
            assert_eq!(
                view.generation,
                Some(ViewOutputGenerationState::Canceled(key))
            );
            assert_eq!(
                view.observation,
                Some(ViewOutputObservationState {
                    key,
                    last_stage: Stage::Aborted,
                })
            );
        }

        let replacement_label = "observation-replacement";
        let replacement_epoch = coordinator
            .begin_document(replacement_label)
            .expect("replacement document");
        coordinator
            .activate(replacement_label, owner, replacement_epoch, 1, true)
            .expect("first replacement activation");
        coordinator
            .accept_observation_stage(
                replacement_label,
                owner,
                replacement_epoch,
                1,
                Stage::PostWrite,
            )
            .expect("first replacement postWrite");
        coordinator
            .activate(replacement_label, replacement, replacement_epoch, 2, true)
            .expect("terminal replacement activation");
        assert_eq!(
            coordinator.accept_observation_stage(
                replacement_label,
                owner,
                replacement_epoch,
                1,
                Stage::PostFit,
            ),
            Err(ObservationError::StaleIdentity)
        );
        coordinator
            .accept_observation_stage(
                replacement_label,
                replacement,
                replacement_epoch,
                2,
                Stage::Aborted,
            )
            .expect("replacement terminal observation");
        coordinator
            .activate(replacement_label, owner, replacement_epoch, 3, true)
            .expect("terminal progress replacement");
        coordinator
            .accept_observation_stage(
                replacement_label,
                owner,
                replacement_epoch,
                3,
                Stage::PostWrite,
            )
            .expect("nonterminal replacement postWrite");
        coordinator
            .accept_observation_stage(
                replacement_label,
                owner,
                replacement_epoch,
                3,
                Stage::PostFit,
            )
            .expect("nonterminal replacement postFit");
        coordinator
            .activate(replacement_label, replacement, replacement_epoch, 4, true)
            .expect("nonterminal progress replacement");
        assert_eq!(
            coordinator.accept_observation_stage(
                replacement_label,
                owner,
                replacement_epoch,
                3,
                Stage::Aborted,
            ),
            Err(ObservationError::StaleIdentity)
        );
        coordinator
            .accept_observation_stage(
                replacement_label,
                replacement,
                replacement_epoch,
                4,
                Stage::PostWrite,
            )
            .expect("new generation starts empty");

        let failed_label = "observation-failed";
        let failed_epoch = coordinator
            .begin_document(failed_label)
            .expect("failed document");
        local.fail_next_activation();
        assert!(matches!(
            coordinator.activate(failed_label, failed, failed_epoch, 1, true),
            Err(TerminalOutputCoordinationError::Backend(
                TerminalOutputAttachError::SessionUnavailable
            ))
        ));
        assert_eq!(
            coordinator.accept_observation_stage(
                failed_label,
                failed,
                failed_epoch,
                1,
                Stage::PostWrite,
            ),
            Err(ObservationError::StageOrderInvalid)
        );
        coordinator
            .accept_observation_stage(failed_label, failed, failed_epoch, 1, Stage::Aborted)
            .expect("failed activation abort");
        assert_eq!(
            coordinator.accept_observation_stage(
                failed_label,
                failed,
                failed_epoch,
                1,
                Stage::Aborted,
            ),
            Err(ObservationError::CardinalityExceeded)
        );
        coordinator
            .release_session(failed, SessionBackendKind::LocalProcess)
            .expect("failed session release");
        {
            let registry = coordinator.state.lock().unwrap();
            let view = registry.views.get(failed_label).unwrap();
            assert_eq!(view.high_water_generation, 1);
            assert!(view.generation.is_none());
            assert!(view.observation.is_none());
        }
        assert_eq!(
            coordinator.accept_observation_stage(
                failed_label,
                failed,
                failed_epoch,
                1,
                Stage::Aborted,
            ),
            Err(ObservationError::StaleIdentity)
        );

        let canceled_label = "observation-canceled-before-attach";
        let canceled_epoch = coordinator
            .begin_document(canceled_label)
            .expect("canceled document");
        coordinator
            .cancel(canceled_label, owner, canceled_epoch, 1)
            .expect("cancel before attach");
        assert_eq!(
            coordinator.accept_observation_stage(
                canceled_label,
                owner,
                canceled_epoch,
                1,
                Stage::PostWrite,
            ),
            Err(ObservationError::StageOrderInvalid)
        );
        coordinator
            .detach(canceled_label, owner, canceled_epoch, 1)
            .expect("exact canceled detach is a no-op");
        coordinator
            .cancel(canceled_label, owner, canceled_epoch, 1)
            .expect("exact canceled cancel is a no-op");
        coordinator
            .accept_observation_stage(canceled_label, owner, canceled_epoch, 1, Stage::Aborted)
            .expect("canceled abort");
        assert!(matches!(
            coordinator.activate(canceled_label, owner, canceled_epoch, 1, true),
            Err(TerminalOutputCoordinationError::StaleAttachGeneration)
        ));

        let detach_write_label = "observation-detach-post-write";
        let detach_write_epoch = coordinator
            .begin_document(detach_write_label)
            .expect("detach postWrite document");
        coordinator
            .activate(detach_write_label, owner, detach_write_epoch, 1, true)
            .expect("detach postWrite activation");
        coordinator
            .accept_observation_stage(
                detach_write_label,
                owner,
                detach_write_epoch,
                1,
                Stage::PostWrite,
            )
            .expect("detach postWrite observation");
        coordinator
            .detach(detach_write_label, owner, detach_write_epoch, 1)
            .expect("detach after postWrite");
        assert_eq!(
            coordinator.accept_observation_stage(
                detach_write_label,
                owner,
                detach_write_epoch,
                1,
                Stage::PostFit,
            ),
            Err(ObservationError::StageOrderInvalid)
        );
        coordinator
            .accept_observation_stage(
                detach_write_label,
                owner,
                detach_write_epoch,
                1,
                Stage::Aborted,
            )
            .expect("detach postWrite abort");

        let detach_fit_label = "observation-detach-post-fit";
        let detach_fit_epoch = coordinator
            .begin_document(detach_fit_label)
            .expect("detach postFit document");
        coordinator
            .activate(detach_fit_label, owner, detach_fit_epoch, 1, true)
            .expect("detach postFit activation");
        coordinator
            .accept_observation_stage(
                detach_fit_label,
                owner,
                detach_fit_epoch,
                1,
                Stage::PostWrite,
            )
            .expect("detach postFit postWrite");
        coordinator
            .accept_observation_stage(detach_fit_label, owner, detach_fit_epoch, 1, Stage::PostFit)
            .expect("detach postFit observation");
        coordinator
            .detach(detach_fit_label, owner, detach_fit_epoch, 1)
            .expect("detach after postFit");
        coordinator
            .accept_observation_stage(detach_fit_label, owner, detach_fit_epoch, 1, Stage::Aborted)
            .expect("detach postFit abort");

        let late_label = "observation-stale-lifecycle";
        let late_epoch = coordinator
            .begin_document(late_label)
            .expect("stale lifecycle document");
        coordinator
            .activate(late_label, owner, late_epoch, 1, true)
            .expect("stale lifecycle first activation");
        coordinator
            .activate(late_label, replacement, late_epoch, 2, true)
            .expect("stale lifecycle current activation");
        coordinator
            .accept_observation_stage(late_label, replacement, late_epoch, 2, Stage::PostWrite)
            .expect("current postWrite");
        coordinator
            .detach(late_label, owner, late_epoch, 1)
            .expect("stale late detach");
        coordinator
            .cancel(late_label, owner, late_epoch, 1)
            .expect("stale late cancel");
        coordinator
            .cancel(late_label, owner, late_epoch, 2)
            .expect("wrong-owner equal-generation cancel");
        coordinator
            .accept_observation_stage(late_label, replacement, late_epoch, 2, Stage::PostFit)
            .expect("current postFit survived stale lifecycle");
        coordinator
            .accept_observation_stage(late_label, replacement, late_epoch, 2, Stage::Settled)
            .expect("current settlement survived stale lifecycle");

        let removed_label = "observation-session-removal";
        let removed_epoch = coordinator
            .begin_document(removed_label)
            .expect("session-removal document");
        coordinator
            .activate(removed_label, removed, removed_epoch, 1, true)
            .expect("session-removal activation");
        coordinator
            .accept_observation_stage(removed_label, removed, removed_epoch, 1, Stage::PostWrite)
            .expect("session-removal postWrite");
        coordinator
            .release_session(removed, SessionBackendKind::LocalProcess)
            .expect("owned session release");
        {
            let registry = coordinator.state.lock().unwrap();
            let view = registry.views.get(removed_label).unwrap();
            assert_eq!(view.high_water_generation, 1);
            assert!(view.generation.is_none());
            assert!(view.observation.is_none());
        }
        assert_eq!(
            coordinator.accept_observation_stage(
                removed_label,
                removed,
                removed_epoch,
                1,
                Stage::PostFit,
            ),
            Err(ObservationError::StaleIdentity)
        );
        assert!(matches!(
            coordinator.activate(removed_label, removed, removed_epoch, 1, true),
            Err(TerminalOutputCoordinationError::StaleAttachGeneration)
        ));
        coordinator
            .activate(removed_label, removed, removed_epoch, 2, true)
            .expect("next generation after session-state release");

        let removed_canceled = Uuid::new_v4();
        let removed_canceled_label = "observation-canceled-session-removal";
        let removed_canceled_epoch = coordinator
            .begin_document(removed_canceled_label)
            .expect("canceled session-removal document");
        coordinator
            .cancel(
                removed_canceled_label,
                removed_canceled,
                removed_canceled_epoch,
                3,
            )
            .expect("canceled session tombstone");
        coordinator
            .accept_observation_stage(
                removed_canceled_label,
                removed_canceled,
                removed_canceled_epoch,
                3,
                Stage::Aborted,
            )
            .expect("canceled session abort");
        coordinator
            .release_session(removed_canceled, SessionBackendKind::LocalProcess)
            .expect("canceled session release");
        {
            let registry = coordinator.state.lock().unwrap();
            let view = registry.views.get(removed_canceled_label).unwrap();
            assert_eq!(view.high_water_generation, 3);
            assert!(view.generation.is_none());
            assert!(view.observation.is_none());
        }

        let page_label = "observation-page";
        let first_page_epoch = coordinator
            .begin_document(page_label)
            .expect("first page document");
        coordinator
            .activate(page_label, owner, first_page_epoch, 1, true)
            .expect("first page activation");
        coordinator
            .accept_observation_stage(page_label, owner, first_page_epoch, 1, Stage::PostWrite)
            .expect("first page postWrite");
        let second_page_epoch = coordinator
            .begin_document(page_label)
            .expect("page rotation");
        assert!(second_page_epoch > first_page_epoch);
        assert_eq!(
            coordinator.accept_observation_stage(
                page_label,
                owner,
                first_page_epoch,
                1,
                Stage::PostFit,
            ),
            Err(ObservationError::StaleDocumentEpoch)
        );
        assert_eq!(
            coordinator.accept_observation_stage(
                page_label,
                owner,
                second_page_epoch,
                1,
                Stage::PostWrite,
            ),
            Err(ObservationError::StaleIdentity)
        );
        coordinator
            .activate(page_label, owner, second_page_epoch, 1, true)
            .expect("second page activation");
        coordinator
            .accept_observation_stage(page_label, owner, second_page_epoch, 1, Stage::PostWrite)
            .expect("second page postWrite");
        coordinator
            .release_window(page_label)
            .expect("destroyed page cleanup");
        assert_eq!(
            coordinator.accept_observation_stage(
                page_label,
                owner,
                second_page_epoch,
                1,
                Stage::PostFit,
            ),
            Err(ObservationError::DocumentUnavailable)
        );
        let reused_page_epoch = coordinator
            .begin_document(page_label)
            .expect("same-label reuse");
        assert!(reused_page_epoch > second_page_epoch);
        coordinator
            .activate(page_label, owner, reused_page_epoch, 1, true)
            .expect("same-label reused activation");
        coordinator
            .accept_observation_stage(page_label, owner, reused_page_epoch, 1, Stage::Aborted)
            .expect("same-label reused observation");

        let race_label = "observation-race";
        let race_epoch = coordinator
            .begin_document(race_label)
            .expect("observation race document");
        coordinator
            .activate(race_label, race_old, race_epoch, 1, true)
            .expect("observation race old activation");
        coordinator
            .accept_observation_stage(race_label, race_old, race_epoch, 1, Stage::PostWrite)
            .expect("observation race old postWrite");
        let (activation_started_tx, activation_started_rx) = std::sync::mpsc::channel();
        let (activation_release_tx, activation_release_rx) = std::sync::mpsc::channel();
        local.block_next_activation(activation_started_tx, activation_release_rx);
        let activation_coordinator = coordinator.clone();
        let activation = std::thread::spawn(move || {
            activation_coordinator.activate(race_label, race_new, race_epoch, 2, true)
        });
        activation_started_rx
            .recv()
            .expect("newer activation entered backend under the registry lock");

        let (observation_attempted_tx, observation_attempted_rx) = std::sync::mpsc::channel();
        let (observation_result_tx, observation_result_rx) = std::sync::mpsc::channel();
        let observation_coordinator = coordinator.clone();
        let observation = std::thread::spawn(move || {
            observation_attempted_tx.send(()).unwrap();
            let result = observation_coordinator.accept_observation_stage(
                race_label,
                race_old,
                race_epoch,
                1,
                Stage::PostFit,
            );
            observation_result_tx.send(result).unwrap();
        });
        observation_attempted_rx
            .recv()
            .expect("old observation attempted during newer activation");
        assert!(matches!(
            observation_result_rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        activation_release_tx
            .send(())
            .expect("release newer activation");
        activation
            .join()
            .expect("newer activation thread")
            .expect("newer activation result");
        assert_eq!(
            observation_result_rx.recv().expect("observation result"),
            Err(ObservationError::StaleIdentity)
        );
        observation.join().expect("observation thread");
        coordinator
            .accept_observation_stage(race_label, race_new, race_epoch, 2, Stage::PostWrite)
            .expect("newer observation owns the atomic reservation");
    }

    struct DelayedSpawnBackend {
        live: Mutex<std::collections::HashSet<Uuid>>,
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl DelayedSpawnBackend {
        fn new(
            started: tokio::sync::oneshot::Sender<()>,
            release: tokio::sync::oneshot::Receiver<()>,
        ) -> Self {
            Self {
                live: Mutex::new(std::collections::HashSet::new()),
                started: Mutex::new(Some(started)),
                release: Mutex::new(Some(release)),
            }
        }
    }

    impl PtyBackend for DelayedSpawnBackend {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn spawn(
            &self,
            spec: BackendSpawnSpec,
        ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
            let started = self.started.lock().unwrap().take().expect("started sender");
            let release = self
                .release
                .lock()
                .unwrap()
                .take()
                .expect("release receiver");
            Box::pin(async move {
                let _ = started.send(());
                let _ = release.await;
                self.live.lock().unwrap().insert(spec.id);
                Ok(())
            })
        }

        fn write(
            &self,
            _authority: &BackendWriteAuthority,
            _id: Uuid,
            _data: &[u8],
        ) -> Result<(), AppError> {
            Ok(())
        }

        fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
            Ok(())
        }

        fn kill(&self, id: Uuid) -> Result<(), AppError> {
            self.live.lock().unwrap().remove(&id);
            Ok(())
        }

        fn has_session(&self, id: Uuid) -> bool {
            self.live.lock().unwrap().contains(&id)
        }

        fn get_screen_snapshot(&self, _id: Uuid) -> Option<PtyScreenSnapshot> {
            None
        }

        fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
            None
        }

        fn get_screen_rows(&self, _id: Uuid) -> ScreenRowsRead {
            ScreenRowsRead::SessionOver
        }

        fn register_response_watcher(
            &self,
            _session_id: Uuid,
            _request_id: String,
            _response_dir: std::path::PathBuf,
        ) {
        }

        fn terminate_job_for_session(&self, _id: Uuid) -> bool {
            false
        }

        fn kill_all_jobs(&self) -> (usize, usize) {
            (0, 0)
        }
    }

    fn test_spawn_spec(id: Uuid) -> BackendSpawnSpec {
        BackendSpawnSpec {
            id,
            agent_id: None,
            coding_agent: None,
            cmd: "cmd".to_string(),
            args: Vec::new(),
            resolved_agent_host_shell: None,
            cwd: ".".to_string(),
            selected_cwd: None,
            cols: 80,
            rows: 24,
            container_image: None,
            configured_env: Vec::new(),
            env_remove_keys: Vec::new(),
            env_unset: Vec::new(),
            extra_env: Vec::new(),
            idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
            output_target: crate::pty::output::PtyOutputTarget::noop(),
            resource_registration: None,
            logical_resource_slot: None,
            container_credential: None,
            container_repo_mounts: Vec::new(),
        }
    }

    #[tokio::test]
    async fn facade_delegates_local_route_by_session_id() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        backend.set_live(id);
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        manager
            .lock()
            .unwrap()
            .try_record_route(id, SessionBackendKind::LocalProcess)
            .unwrap();

        let permit = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        PtyManager::write_with_permit(&permit, b"abc").expect("write");
        drop(permit);
        manager.lock().unwrap().resize(id, 120, 30).expect("resize");
        assert!(manager.lock().unwrap().has_session(id));
        assert_eq!(manager.lock().unwrap().get_pty_size(id), Some((30, 120)));
        assert!(manager.lock().unwrap().terminate_job_for_session(id));
        manager.lock().unwrap().register_response_watcher(
            id,
            "r1".to_string(),
            std::path::PathBuf::from("x"),
        );
        manager.lock().unwrap().kill(id).expect("kill");

        assert_eq!(
            backend.calls(),
            vec![
                Call::Write(id, b"abc".to_vec()),
                Call::Resize(id, 120, 30),
                Call::Has(id),
                Call::Size(id),
                Call::TerminateJob(id),
                Call::Watcher(id, "r1".to_string()),
                Call::Kill(id),
            ]
        );
        assert!(!manager.lock().unwrap().has_session(id));
    }

    #[tokio::test]
    async fn facade_write_without_route_returns_session_not_found() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));

        let err = match PtyManager::acquire_input_writer(&manager, id).await {
            Ok(_) => panic!("missing route unexpectedly acquired"),
            Err(error) => error,
        };

        assert!(matches!(err, AppError::SessionNotFound(_)));
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn context_liveness_uses_backend_default_and_missing_route_is_over() {
        let live_id = Uuid::new_v4();
        let missing_id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        backend.set_live(live_id);
        let manager = PtyManager::new_for_test(backend);
        manager.record_route(live_id, SessionBackendKind::LocalProcess);

        assert_eq!(
            manager.context_session_liveness(live_id),
            ContextSessionLiveness::Live
        );
        assert_eq!(
            manager.context_session_liveness(missing_id),
            ContextSessionLiveness::SessionOver
        );
    }

    /// #1171, 9.1.9 - a session with no route is `Gone`, not `Missing`.
    ///
    /// Same reading `get_screen_rows` already gives a routeless id (`:573-580`): every route
    /// removal is preceded by parser removal, so the session really is over. The engine
    /// retires on `Gone`, which is exactly what should happen here; `Missing` would keep it
    /// sampling an id that can never come back.
    #[test]
    fn screen_rows_since_reports_gone_for_a_session_with_no_route() {
        let manager = PtyManager::new_for_test(Arc::new(RecordingBackend::default()));

        assert!(matches!(
            manager.screen_rows_since(Uuid::new_v4(), None),
            crate::pty::watchers::ScreenRowsSince::Gone
        ));
    }

    /// #1388, T4 - the trait default reads as "no claim, do not gate".
    ///
    /// Exercised on an existing fake that does not override it, which is the one path
    /// production never takes: both production backends override. Its only content is the
    /// literal `true` in `backend.rs`, so this is a guard against a future flip to `false`,
    /// which would strand every non-answering implementor at the wake settle's cap.
    #[test]
    fn pty_backend_default_reports_rendered() {
        let backend = RecordingBackend::default();

        assert!(backend.has_rendered_visible_content(Uuid::new_v4()));
    }

    /// #1171, 9.1.8 - a backend that never heard of the seam keeps working through the trait
    /// default: it reports a frame with NO stamp, and it never reports `Unchanged`, whatever
    /// stamp it is handed. That is the property that lets `stamp` be an `Option` instead of a
    /// fabricated value, and it is what keeps the two `PtyBackend` test fakes compiling.
    #[test]
    fn the_defaulted_seam_reports_no_stamp_and_never_reports_unchanged() {
        use crate::pty::watchers::{FrameStamp, ScreenRowsSince};

        struct DefaultSeamBackend;

        impl PtyBackend for DefaultSeamBackend {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn spawn(
                &self,
                _spec: BackendSpawnSpec,
            ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
                Box::pin(async { Ok(()) })
            }
            fn write(
                &self,
                _authority: &BackendWriteAuthority,
                _id: Uuid,
                _data: &[u8],
            ) -> Result<(), AppError> {
                Ok(())
            }
            fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
                Ok(())
            }
            fn kill(&self, _id: Uuid) -> Result<(), AppError> {
                Ok(())
            }
            fn has_session(&self, _id: Uuid) -> bool {
                true
            }
            fn get_screen_snapshot(&self, _id: Uuid) -> Option<PtyScreenSnapshot> {
                None
            }
            fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
                None
            }
            fn get_screen_rows(&self, _id: Uuid) -> ScreenRowsRead {
                ScreenRowsRead::Rows(vec!["only row".to_string()])
            }
            fn register_response_watcher(
                &self,
                _session_id: Uuid,
                _request_id: String,
                _response_dir: std::path::PathBuf,
            ) {
            }
            fn terminate_job_for_session(&self, _id: Uuid) -> bool {
                false
            }
            fn kill_all_jobs(&self) -> (usize, usize) {
                (0, 0)
            }
        }

        let id = Uuid::new_v4();
        let manager = PtyManager::new_for_test(Arc::new(DefaultSeamBackend));
        manager.record_route(id, SessionBackendKind::LocalProcess);

        let first = manager.screen_rows_since(id, None);
        let frame = first.frame().expect("the default must produce a frame");
        assert!(frame.stamp.is_none());
        assert_eq!(frame.rows, vec!["only row".to_string()]);
        assert_eq!(frame.wrapped, vec![false]);
        assert_eq!(frame.cursor_row, 0);

        // Handed a stamp that would match anything, it still refuses to claim "unchanged":
        // it has no sequence of its own to compare against.
        let seen = FrameStamp {
            sequence: 0,
            rows: 1,
            cols: 1,
        };
        assert!(matches!(
            manager.screen_rows_since(id, Some(seen)),
            ScreenRowsSince::Frame(_)
        ));
    }

    #[test]
    fn archive_liveness_reports_pending_spawn_until_mark_drops() {
        let backend = Arc::new(RecordingBackend::default());
        let manager = PtyManager::new_for_test(backend);
        let mark = manager.mark_spawning("C:/repo/.ac/wg-1/__agent_dev", "dev");

        let (pending, live) = manager.archive_liveness(&[]);

        assert_eq!(
            pending,
            vec![PendingSpawn {
                cwd: "C:/repo/.ac/wg-1/__agent_dev".to_string(),
                label: "dev".to_string(),
            }]
        );
        assert!(live.is_empty());
        drop(mark);

        let (pending, _) = manager.archive_liveness(&[]);
        assert!(pending.is_empty());
    }

    #[test]
    fn archive_liveness_reports_backend_live_route() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        backend.set_live(id);
        let manager = PtyManager::new_for_test(backend.clone());
        manager.record_route(id, SessionBackendKind::LocalProcess);

        let (pending, live) = manager.archive_liveness(&[id]);

        assert!(pending.is_empty());
        assert_eq!(live, vec![true]);
        assert_eq!(backend.calls(), vec![Call::Has(id)]);
    }

    #[tokio::test]
    async fn input_permits_serialize_writers_for_one_session() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        backend.set_live(id);
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        manager
            .lock()
            .unwrap()
            .try_record_route(id, SessionBackendKind::LocalProcess)
            .unwrap();
        let first = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        let waiting_manager = Arc::clone(&manager);
        let waiter = tokio::spawn(async move {
            let permit = PtyManager::acquire_input_writer(&waiting_manager, id)
                .await
                .unwrap();
            PtyManager::write_with_permit(&permit, b"second").unwrap();
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        PtyManager::write_with_permit(&first, b"first").unwrap();
        drop(first);
        waiter.await.unwrap();
        assert_eq!(
            backend.calls(),
            vec![
                Call::Write(id, b"first".to_vec()),
                Call::Write(id, b"second".to_vec()),
            ]
        );
    }

    #[tokio::test]
    async fn different_sessions_have_independent_input_gates() {
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));
        {
            let manager = manager.lock().unwrap();
            manager
                .try_record_route(first_id, SessionBackendKind::LocalProcess)
                .unwrap();
            manager
                .try_record_route(second_id, SessionBackendKind::LocalProcess)
                .unwrap();
        }
        let _first = PtyManager::acquire_input_writer(&manager, first_id)
            .await
            .unwrap();
        let second = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            PtyManager::acquire_input_writer(&manager, second_id),
        )
        .await
        .expect("another session must not wait on the first input gate")
        .unwrap();
        drop(second);
    }

    #[tokio::test]
    async fn removed_and_recreated_route_invalidates_an_old_permit() {
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        manager
            .lock()
            .unwrap()
            .try_record_route(id, SessionBackendKind::LocalProcess)
            .unwrap();
        let old = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        {
            let manager = manager.lock().unwrap();
            manager.remove_route_if_kind(id, SessionBackendKind::LocalProcess);
            manager
                .try_record_route(id, SessionBackendKind::LocalProcess)
                .unwrap();
        }
        assert!(PtyManager::write_with_permit(&old, b"stale").is_err());
        drop(old);
        let current = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        PtyManager::write_with_permit(&current, b"current").unwrap();
        assert_eq!(backend.calls(), vec![Call::Write(id, b"current".to_vec())]);
    }

    #[test]
    fn duplicate_route_and_generation_overflow_fail_without_replacement() {
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let manager = PtyManager::new_for_test(backend);
        manager
            .try_record_route(first_id, SessionBackendKind::LocalProcess)
            .unwrap();
        assert!(manager
            .try_record_route(first_id, SessionBackendKind::ContainerTransport)
            .is_err());
        assert_eq!(
            manager.backend_kind(first_id),
            Some(SessionBackendKind::LocalProcess)
        );
        manager.registry.lock().unwrap().next_route_generation = u64::MAX;
        assert!(manager
            .try_record_route(second_id, SessionBackendKind::LocalProcess)
            .is_err());
        assert_eq!(manager.backend_kind(second_id), None);
    }

    #[tokio::test]
    async fn same_spelling_directory_replacement_fails_verified_route_lock() {
        let id = Uuid::new_v4();
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("replica");
        let retired = temp.path().join("retired");
        std::fs::create_dir(&path).unwrap();
        let original = crate::path_identity::verify_directory(&path).unwrap();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));
        manager
            .lock()
            .unwrap()
            .record_route_with_identities(
                id,
                SessionBackendKind::LocalProcess,
                Some(original.clone()),
                Some(original),
            )
            .unwrap();
        let permit = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        std::fs::rename(&path, &retired).unwrap();
        std::fs::create_dir(&path).unwrap();
        let replacement = crate::path_identity::verify_directory(&path).unwrap();
        assert!(PtyManager::lock_route_for_verified_write(
            &permit,
            SessionBackendKind::LocalProcess,
            &replacement,
            &replacement,
        )
        .is_err());
    }

    #[tokio::test]
    async fn verified_write_guard_retains_authority_route_through_first_write() {
        let authority_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let temp = tempfile::TempDir::new().unwrap();
        let authority_path = temp.path().join("authority");
        let target_path = temp.path().join("target");
        std::fs::create_dir(&authority_path).unwrap();
        std::fs::create_dir(&target_path).unwrap();
        let authority_identity = crate::path_identity::verify_directory(&authority_path).unwrap();
        let target_identity = crate::path_identity::verify_directory(&target_path).unwrap();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));
        {
            let manager = manager.lock().unwrap();
            manager
                .record_route_with_identities(
                    authority_id,
                    SessionBackendKind::LocalProcess,
                    Some(authority_identity.clone()),
                    Some(authority_identity.clone()),
                )
                .unwrap();
            manager
                .record_route_with_identities(
                    target_id,
                    SessionBackendKind::LocalProcess,
                    Some(target_identity.clone()),
                    Some(target_identity.clone()),
                )
                .unwrap();
        }
        let authority = PtyManager::authority_route_proof(&manager, authority_id).unwrap();
        let permit = PtyManager::acquire_input_writer(&manager, target_id)
            .await
            .unwrap();
        let authority_guard = authority
            .lock_verified(
                SessionBackendKind::LocalProcess,
                &authority_identity,
                Some(&authority_identity),
            )
            .unwrap();
        let mut write_guard = PtyManager::lock_route_for_verified_write(
            &permit,
            SessionBackendKind::LocalProcess,
            &target_identity,
            &target_identity,
        )
        .unwrap();
        write_guard.retain_authority_guard(authority_guard);

        assert_eq!(
            manager
                .lock()
                .unwrap()
                .try_remove_route_if_kind(authority_id, SessionBackendKind::LocalProcess),
            Err(PtyRouteRemovalError::Busy)
        );
        drop(write_guard);
        assert_eq!(
            manager
                .lock()
                .unwrap()
                .try_remove_route_if_kind(authority_id, SessionBackendKind::LocalProcess),
            Ok(())
        );
    }

    #[tokio::test]
    async fn post_spawn_manager_poison_kills_the_unroutable_backend_session() {
        let id = Uuid::new_v4();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let backend = Arc::new(DelayedSpawnBackend::new(started_tx, release_rx));
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let spawn_task = {
            let manager = manager.clone();
            tokio::spawn(async move {
                PtyManager::spawn(
                    &manager,
                    SessionBackendKind::LocalProcess,
                    test_spawn_spec(id),
                )
                .await
            })
        };

        started_rx.await.expect("spawn started");
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = manager.lock().unwrap();
            panic!("poison manager between backend spawn and route publication");
        }));
        assert!(poisoned.is_err());
        release_tx.send(()).expect("release spawn");
        assert!(spawn_task.await.unwrap().is_err());
        assert!(!backend.has_session(id));
    }

    #[tokio::test]
    async fn spawn_does_not_hold_manager_mutex_while_backend_awaits() {
        let id = Uuid::new_v4();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let backend = Arc::new(DelayedSpawnBackend::new(started_tx, release_rx));
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend)));

        let spawn_task = {
            let manager = manager.clone();
            tokio::spawn(async move {
                PtyManager::spawn(
                    &manager,
                    SessionBackendKind::LocalProcess,
                    test_spawn_spec(id),
                )
                .await
            })
        };

        started_rx.await.expect("spawn started");
        {
            let guard = manager
                .try_lock()
                .expect("manager mutex should be free while backend spawn awaits");
            assert!(!guard.has_session(id));
        }

        release_tx.send(()).expect("release spawn");
        spawn_task.await.expect("spawn task").expect("spawn ok");
        assert!(manager.lock().unwrap().has_session(id));
    }
}
