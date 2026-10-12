use std::collections::HashMap;
use std::sync::{Arc, Mutex, TryLockError};

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
use crate::pty::output::PtyScreenSnapshot;
use crate::telegram::manager::OutputSenderMap;

pub(crate) use crate::pty::output::TerminalOutputAttachError;

// Runtime-only authority. No persisted lease, filesystem, settings or transport callouts.
#[derive(Clone, Default)]
pub(crate) struct ResponseCloseState {
    inner: Arc<Mutex<ResponseCloseInner>>,
}

#[derive(Clone, Copy)]
pub(crate) enum ResponseClosePublisher {
    Manager = 1,
    Pty = 2,
    RawOutput = 4,
    Detector = 8,
    Menu = 16,
    Hold = 32,
}

#[derive(Default)]
struct ResponseCloseInner {
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    ready: bool,
    publishers: u8,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    work_bootstrapped: bool,
    enabled: bool,
    seconds: u32,
    settings_epoch: u64,
    failed: bool,
    routes: HashMap<Uuid, ResponseCloseRoute>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    work: HashMap<String, ResponseCloseWork>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    create_owners: HashMap<String, HashMap<Uuid, bool>>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    next_return_sequence: u64,
    unknown_spawns: usize,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    terminal_returns: HashMap<(ResponseRouteIdentity, ResponseRouteIdentity, String), String>,
}

#[derive(Default)]
struct ResponseCloseWork {
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    provisional: usize,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    dispatch: usize,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pending: HashMap<String, String>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    terminal: HashMap<String, String>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct ResponseRouteIdentity {
    pub target: String,
    pub session_id: Uuid,
    pub token: Uuid,
    pub generation: u64,
    pub carrier_version: u64,
    pub backend: SessionBackendKind,
}

#[derive(Clone)]
struct ResponseClosePolicy {
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    nonce: Uuid,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    member: ResponseRouteIdentity,
    coordinator: ResponseRouteIdentity,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    work_epoch: u64,
    settings_epoch: u64,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    request_key: String,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    request_digest: String,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    opened_at: std::time::Instant,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    submitted: bool,
    registered_at: Option<std::time::Instant>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    success_at: Option<std::time::Instant>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    returns: HashMap<String, (String, u64)>,
    selected: Option<Uuid>,
}

struct ResponseCloseRoute {
    generation: u64,
    facts: Option<ResponseRouteIdentity>,
    eligible: bool,
    manager_token: Option<Uuid>,
    manager_version: u64,
    manager_working: Option<bool>,
    detector_working: Option<bool>,
    menu_blocked: Option<bool>,
    hold_until: Option<std::time::Instant>,
    hold_known: bool,
    hold_window: Option<std::time::Duration>,
    held: std::collections::HashSet<String>,
    writers: usize,
    work_epoch: u64,
    activity_version: u64,
    last_change: std::time::Instant,
    policy: Option<ResponseClosePolicy>,
    closing: Option<Uuid>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    closing_target: Option<String>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    committed_binding: Option<ResponseCloseCommittedBinding>,
}

impl ResponseCloseRoute {
    fn touch(&mut self, now: std::time::Instant) -> bool {
        self.last_change = self.last_change.max(now);
        if let Some(policy) = &mut self.policy {
            policy.selected = None;
        }
        if let Some(next) = self.activity_version.checked_add(1) {
            self.activity_version = next;
            true
        } else {
            self.policy = None;
            false
        }
    }
    fn invalidate(&mut self, now: std::time::Instant) -> bool {
        self.policy = None;
        if let Some(next) = self.work_epoch.checked_add(1) {
            self.work_epoch = next;
            self.touch(now)
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResponseCloseError {
    ClosingDeferred,
    Unknown,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    Conflict,
    Unavailable,
}

// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct WorkAdmission {
    state: ResponseCloseState,
    target: String,
    key: String,
    active: bool,
}

// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct WorkDeliveryReservation {
    state: ResponseCloseState,
    target: String,
}

pub(crate) struct WriterAdmission {
    counted: bool,
    state: ResponseCloseState,
    session_id: Uuid,
    generation: u64,
}

// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct CreateOwnerReservation {
    state: ResponseCloseState,
    target: String,
    nonce: Uuid,
    classification: CreateClassification,
}

/// P05 supplies current ALL-scopes classification under the publication permit.
/// A reservation records ownership; it does not prove protection or identity.
#[derive(Clone)]
// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) enum CreateClassification {
    Foreign {
        publication_epoch: u64,
    },
    Protected {
        identity: ResponseRouteIdentity,
        // Dormant until P05 integration; remove with real callers.
        #[allow(dead_code)]
        publication_epoch: u64,
    },
    Preparing {
        owner_nonce: Uuid,
        identity: ResponseRouteIdentity,
        // Dormant until P05 integration; remove with real callers.
        #[allow(dead_code)]
        publication_epoch: u64,
    },
    Unknown,
}

impl CreateOwnerReservation {
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn classification(&self) -> &CreateClassification {
        &self.classification
    }
}

// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) enum CreateAdmission {
    Reserved(CreateOwnerReservation),
    ClosingDeferred,
    WaitingIdentity,
}

#[derive(Clone)]
// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct ResponseDeliveryContext {
    session_id: Uuid,
    nonce: Uuid,
}

// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct ResponseDeliveryProof {
    pub context: ResponseDeliveryContext,
    pub member: ResponseRouteIdentity,
    pub coordinator: ResponseRouteIdentity,
    pub published_carrier_version: u64,
}

#[derive(Clone)]
// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct ReturnObservationContext {
    settings_epoch: u64,
    sequence: u64,
    session_id: Uuid,
    nonce: Uuid,
    key: String,
    digest: String,
    member: ResponseRouteIdentity,
    coordinator: ResponseRouteIdentity,
    work_epoch: u64,
}

// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct ReturnDeliveryProof {
    pub context: ReturnObservationContext,
}

#[derive(Clone)]
// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct ResponseCloseTicket {
    session_id: Uuid,
    generation: u64,
    nonce: Uuid,
    selection_nonce: Uuid,
    work_epoch: u64,
    activity_version: u64,
    carrier_version: u64,
    settings_epoch: u64,
    member: ResponseRouteIdentity,
}

/// Incarnation is the exact UUID/token/generation tuple; backend kind and the
/// checked generation identify the frozen backend route. Version stays at commit.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ResponseCloseCommittedBinding {
    pub member: ResponseRouteIdentity,
    pub close_nonce: Uuid,
}

impl ResponseCloseTicket {
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn committed_binding(&self) -> ResponseCloseCommittedBinding {
        ResponseCloseCommittedBinding {
            member: self.member.clone(),
            close_nonce: self.selection_nonce,
        }
    }
}

/// These are owned facts from P05's latest guarded manager/Confirmed checkpoint
/// validation. P03 never performs I/O or imports its publication/effect types.
// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct ResponseCloseCurrentSettlement {
    pub current_carrier_version: u64,
    pub confirmed_carrier_version: u64,
    pub checkpoint_nonce: Uuid,
    pub current_checkpoint_nonce: Uuid,
    pub publication_epoch: u64,
    pub current_publication_epoch: u64,
}

// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) enum ResponseCloseSettlementOutcome {
    Dormant {
        exact_backend_absent: bool,
        all_effects_completed: bool,
        retained_exited_confirmed: bool,
        selection_settled: bool,
    },
    ReopenAfterKillFailure {
        terminal_kill_failure: bool,
        all_effects_terminal: bool,
        original_backend_alive: bool,
        original_manager_identity_intact: bool,
    },
}

// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct ResponseCloseSettlementEvidence {
    pub committed_binding: ResponseCloseCommittedBinding,
    pub current_settlement: ResponseCloseCurrentSettlement,
    pub outcome: ResponseCloseSettlementOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct StaleOrInvalidSettlement;

impl ResponseCloseState {
    pub(crate) fn bind_publisher(&self, publisher: ResponseClosePublisher) {
        let _ = self.with_inner(|inner| inner.publishers |= publisher as u8);
    }
    pub(crate) fn publishers_bound(&self) -> bool {
        self.with_inner(|inner| inner.publishers == 63)
            .unwrap_or(false)
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn mark_work_bootstrapped(&self) {
        let _ = self.with_inner(|inner| inner.work_bootstrapped = true);
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn try_enable_readiness(&self) -> Result<(), ResponseCloseError> {
        self.with_inner(|inner| {
            if inner.publishers != 63 || !inner.work_bootstrapped {
                return Err(ResponseCloseError::Unavailable);
            }
            inner.ready = true;
            Ok(())
        })?
    }
    pub(crate) fn fail_closed(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.failed = true;
        for route in inner.routes.values_mut() {
            route.policy = None;
        }
    }
    // Publish primitive row facts under the manager guard, preserving the P03 API.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish_manager_row(
        &self,
        id: Uuid,
        token: Uuid,
        generation: u64,
        version: u64,
        eligible: bool,
        working: bool,
        now: std::time::Instant,
    ) {
        let _ = self.with_inner(|inner| {
            let Some(route) = inner
                .routes
                .get_mut(&id)
                .filter(|r| r.generation == generation)
            else {
                return;
            };
            if route.manager_token != Some(token)
                || route.manager_version != version
                || route.eligible != eligible
            {
                if !route.invalidate(now) {
                    inner.failed = true;
                }
                route.facts = None;
            } else if route.manager_working != Some(working) && !route.touch(now) {
                inner.failed = true;
            }
            route.manager_token = Some(token);
            route.manager_version = version;
            route.manager_working = Some(working);
            route.eligible = eligible;
        });
    }
    fn uuid_closing(&self, id: Uuid) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .routes
            .get(&id)
            .is_some_and(|route| route.closing.is_some())
    }
    fn begin_unresolved_spawn(&self) {
        let _ = self.with_inner(|inner| {
            let Some(next) = inner.unknown_spawns.checked_add(1) else {
                inner.failed = true;
                return;
            };
            inner.unknown_spawns = next;
            for route in inner.routes.values_mut() {
                if !route.touch(std::time::Instant::now()) {
                    inner.failed = true;
                }
            }
        });
    }
    fn end_unresolved_spawn(&self) {
        let _ = self.with_inner(|inner| {
            let Some(next) = inner.unknown_spawns.checked_sub(1) else {
                inner.failed = true;
                return;
            };
            inner.unknown_spawns = next;
            for route in inner.routes.values_mut() {
                if !route.touch(std::time::Instant::now()) {
                    inner.failed = true;
                }
            }
        });
    }
    pub(crate) fn wiring_identity(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }
    pub(crate) fn same_state(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
    fn with_inner<R>(
        &self,
        f: impl FnOnce(&mut ResponseCloseInner) -> R,
    ) -> Result<R, ResponseCloseError> {
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(poison) => {
                let mut inner = poison.into_inner();
                inner.failed = true;
                return Err(ResponseCloseError::Unavailable);
            }
        };
        if inner.failed {
            return Err(ResponseCloseError::Unavailable);
        }
        Ok(f(&mut inner))
    }
    pub(crate) fn apply_settings(&self, enabled: bool, seconds: u32, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            if inner.enabled == enabled && inner.seconds == seconds {
                return;
            }
            let Some(epoch) = inner.settings_epoch.checked_add(1) else {
                inner.failed = true;
                return;
            };
            inner.settings_epoch = epoch;
            inner.enabled = enabled && (1..=3600).contains(&seconds);
            inner.seconds = seconds;
            for route in inner.routes.values_mut() {
                if !inner.enabled {
                    route.policy = None;
                } else if let Some(policy) = &mut route.policy {
                    if policy.registered_at.is_some() {
                        policy.settings_epoch = epoch;
                    } else {
                        route.policy = None;
                    }
                }
                if !route.touch(now) {
                    inner.failed = true;
                }
            }
        });
    }
    pub(crate) fn register_route(&self, id: Uuid, generation: u64, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            // A route still Closing is never reopened by registration.
            if inner.routes.get(&id).is_some_and(|r| r.closing.is_some()) {
                inner.failed = true;
                return;
            }
            inner.routes.insert(
                id,
                ResponseCloseRoute {
                    generation,
                    facts: None,
                    eligible: false,
                    manager_token: None,
                    manager_version: 0,
                    manager_working: None,
                    detector_working: None,
                    menu_blocked: None,
                    hold_until: None,
                    hold_known: false,
                    hold_window: None,
                    held: Default::default(),
                    writers: 0,
                    work_epoch: 0,
                    activity_version: 0,
                    last_change: now,
                    policy: None,
                    closing: None,
                    closing_target: None,
                    committed_binding: None,
                },
            );
        });
    }
    pub(crate) fn remove_route(&self, id: Uuid, generation: u64) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner
                .routes
                .get_mut(&id)
                .filter(|r| r.generation == generation)
            {
                route.policy = None;
                route.facts = None;
                route.eligible = false;
                // Keep committed Closing until its effect owner settles it.
                if route.closing.is_none() {
                    inner.routes.remove(&id);
                }
            }
        });
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn publish_manager_facts(
        &self,
        identity: ResponseRouteIdentity,
        eligible: bool,
        working: bool,
        now: std::time::Instant,
    ) {
        let _ = self.with_inner(|inner| {
            let Some(route) = inner.routes.get_mut(&identity.session_id) else {
                return;
            };
            if route.generation != identity.generation
                || route.manager_token != Some(identity.token)
                || route.manager_version != identity.carrier_version
            {
                return;
            }
            let identity_changed = route.facts.as_ref() != Some(&identity);
            let changed_id = identity.session_id;
            if identity_changed || route.eligible != eligible {
                if !route.invalidate(now) {
                    inner.failed = true;
                }
            } else if route.manager_working != Some(working) && !route.touch(now) {
                inner.failed = true;
            }
            route.facts = Some(identity);
            route.eligible = eligible;
            route.manager_working = Some(working);
            if identity_changed {
                for dependent in inner.routes.values_mut().filter(|r| {
                    r.policy
                        .as_ref()
                        .is_some_and(|p| p.coordinator.session_id == changed_id)
                }) {
                    if !dependent.invalidate(now) {
                        inner.failed = true;
                    }
                }
            }
        });
    }
    pub(crate) fn invalidate_manager_row(&self, id: Uuid, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner.routes.get_mut(&id) {
                route.facts = None;
                route.eligible = false;
                if !route.invalidate(now) {
                    inner.failed = true;
                }
            }
            for route in inner.routes.values_mut() {
                if route
                    .policy
                    .as_ref()
                    .is_some_and(|p| p.coordinator.session_id == id)
                    && !route.invalidate(now)
                {
                    inner.failed = true;
                }
            }
        });
    }
    pub(crate) fn observe_raw_output(&self, id: Uuid, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner.routes.get_mut(&id) {
                if !route.touch(now) {
                    inner.failed = true;
                }
            }
        });
    }
    pub(crate) fn observe_detector(&self, id: Uuid, working: bool, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner.routes.get_mut(&id) {
                if route.detector_working != Some(working) && !route.touch(now) {
                    inner.failed = true;
                }
                route.detector_working = Some(working);
            }
        });
    }
    pub(crate) fn publish_menu(&self, id: Uuid, blocked: Option<bool>, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner.routes.get_mut(&id) {
                if route.menu_blocked != blocked && !route.touch(now) {
                    inner.failed = true;
                }
                route.menu_blocked = blocked;
            }
        });
    }
    pub(crate) fn publish_hold(
        &self,
        id: Uuid,
        until: Option<std::time::Instant>,
        known: bool,
        now: std::time::Instant,
    ) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner.routes.get_mut(&id) {
                if (route.hold_until, route.hold_known) != (until, known) && !route.touch(now) {
                    inner.failed = true;
                }
                route.hold_until = until;
                route.hold_known = known;
            }
        });
    }
    pub(crate) fn publish_held_key(
        &self,
        id: Uuid,
        key: &str,
        held: bool,
        now: std::time::Instant,
    ) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner.routes.get_mut(&id) {
                let changed = if held {
                    route.held.insert(key.to_owned())
                } else {
                    route.held.remove(key)
                };
                if changed && !route.touch(now) {
                    inner.failed = true;
                }
            }
        });
    }
    pub(crate) fn generation(&self, id: Uuid) -> Option<u64> {
        self.with_inner(|inner| inner.routes.get(&id).map(|r| r.generation))
            .ok()
            .flatten()
    }
    pub(crate) fn is_closing(&self, id: Uuid, generation: u64) -> Result<bool, ResponseCloseError> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        inner
            .routes
            .get(&id)
            .filter(|route| route.generation == generation)
            .map(|route| route.closing.is_some())
            .or_else(|| inner.failed.then_some(false))
            .ok_or(ResponseCloseError::Unknown)
    }
    pub(crate) fn publish_typing_key(
        &self,
        id: Uuid,
        window: Option<std::time::Duration>,
        now: std::time::Instant,
    ) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner.routes.get_mut(&id) {
                if !route.touch(now) {
                    inner.failed = true;
                }
                match window.and_then(|window| now.checked_add(window).map(|end| (window, end))) {
                    Some((window, end)) => {
                        let known =
                            route.hold_known && route.hold_window.is_none_or(|old| old == window);
                        route.hold_until = Some(route.hold_until.map_or(end, |old| old.max(end)));
                        route.hold_window = Some(window);
                        route.hold_known = known;
                    }
                    None => route.hold_known = false,
                }
            }
        });
    }
    pub(crate) fn refresh_hold_snapshot(
        &self,
        id: Uuid,
        window: std::time::Duration,
        closed: bool,
        now: std::time::Instant,
    ) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner.routes.get_mut(&id) {
                if !closed {
                    if !route.hold_known || route.hold_until.is_some() {
                        route.hold_until = None;
                        route.hold_known = true;
                        if !route.touch(now) {
                            inner.failed = true;
                        }
                    }
                } else if route.hold_window != Some(window)
                    || route.hold_until.is_none_or(|end| now > end)
                {
                    // A changed window or unseen manual episode has no timestamp
                    // authority here. Block until the upper snapshot observes open.
                    route.hold_known = false;
                    if !route.touch(now) {
                        inner.failed = true;
                    }
                }
                route.hold_window = Some(window);
            }
        });
    }
    pub(crate) fn reserve_hold_change(
        &self,
        id: Uuid,
    ) -> Result<Option<WriterAdmission>, ResponseCloseError> {
        let Some(generation) = self.generation(id) else {
            return Ok(None);
        };
        self.reserve_writer_inner(id, generation, false).map(Some)
    }
    fn reserve_writer_inner(
        &self,
        id: Uuid,
        generation: u64,
        invalidate: bool,
    ) -> Result<WriterAdmission, ResponseCloseError> {
        // Tracker failure disables feature closing, but does not break ordinary
        // input. A committed Closing is retained even when its mutex poisons.
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(poison) => {
                let mut inner = poison.into_inner();
                inner.failed = true;
                inner
            }
        };
        let failed = inner.failed;
        if failed
            && !inner
                .routes
                .get(&id)
                .is_some_and(|r| r.generation == generation && r.closing.is_some())
        {
            return Ok(WriterAdmission {
                state: self.clone(),
                session_id: id,
                generation,
                counted: false,
            });
        }
        let route = inner
            .routes
            .get_mut(&id)
            .filter(|r| r.generation == generation)
            .ok_or(ResponseCloseError::Unknown)?;
        if route.closing.is_some() {
            return Err(ResponseCloseError::ClosingDeferred);
        }
        if failed {
            return Ok(WriterAdmission {
                state: self.clone(),
                session_id: id,
                generation,
                counted: false,
            });
        }
        let Some(next) = route.writers.checked_add(1) else {
            inner.failed = true;
            return Ok(WriterAdmission {
                state: self.clone(),
                session_id: id,
                generation,
                counted: false,
            });
        };
        route.writers = next;
        let now = std::time::Instant::now();
        if !(if invalidate {
            route.invalidate(now)
        } else {
            route.touch(now)
        }) {
            inner.failed = true;
        }
        Ok(WriterAdmission {
            state: self.clone(),
            session_id: id,
            generation,
            counted: true,
        })
    }
    pub(crate) fn reserve_writer(
        &self,
        id: Uuid,
        generation: u64,
    ) -> Result<WriterAdmission, ResponseCloseError> {
        self.reserve_writer_inner(id, generation, true)
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn reserve_work(
        &self,
        target: &str,
        key: &str,
    ) -> Result<WorkAdmission, ResponseCloseError> {
        self.with_inner(|inner| {
            let work = inner.work.entry(target.to_owned()).or_default();
            if work.pending.contains_key(key) || work.terminal.contains_key(key) {
                return Ok(WorkAdmission {
                    state: self.clone(),
                    target: target.to_owned(),
                    key: key.to_owned(),
                    active: false,
                });
            }
            let Some(next) = work.provisional.checked_add(1) else {
                inner.failed = true;
                return Err(ResponseCloseError::Unavailable);
            };
            work.provisional = next;
            for route in inner
                .routes
                .values_mut()
                .filter(|r| r.facts.as_ref().is_some_and(|f| f.target == target))
            {
                if !route.touch(std::time::Instant::now()) {
                    inner.failed = true;
                }
            }
            Ok(WorkAdmission {
                state: self.clone(),
                target: target.to_owned(),
                key: key.to_owned(),
                active: true,
            })
        })?
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn settle_terminal(&self, target: &str, key: &str, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            let work = inner.work.entry(target.to_owned()).or_default();
            let Some(digest) = work.pending.remove(key) else {
                return;
            };
            work.terminal.insert(key.to_owned(), digest);
            for route in inner
                .routes
                .values_mut()
                .filter(|r| r.facts.as_ref().is_some_and(|f| f.target == target))
            {
                if !route.touch(now) {
                    inner.failed = true;
                }
            }
        });
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn begin_delivery(
        &self,
        target: &str,
    ) -> Result<WorkDeliveryReservation, ResponseCloseError> {
        self.with_inner(|inner| {
            if inner
                .routes
                .values()
                .any(|r| r.closing.is_some() && r.closing_target.as_deref() == Some(target))
            {
                return Err(ResponseCloseError::ClosingDeferred);
            }
            let work = inner.work.entry(target.to_owned()).or_default();
            let Some(next) = work.dispatch.checked_add(1) else {
                inner.failed = true;
                return Err(ResponseCloseError::Unavailable);
            };
            work.dispatch = next;
            Ok(WorkDeliveryReservation {
                state: self.clone(),
                target: target.to_owned(),
            })
        })?
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn reserve_create_owner(
        &self,
        target: &str,
        nonce: Uuid,
        classification: CreateClassification,
    ) -> Result<CreateAdmission, ResponseCloseError> {
        self.with_inner(|inner| {
            if matches!(&classification, CreateClassification::Unknown) {
                return Ok(CreateAdmission::WaitingIdentity);
            }
            if let CreateClassification::Protected { identity, .. }
            | CreateClassification::Preparing { identity, .. } = &classification
            {
                if identity.target != target {
                    return Err(ResponseCloseError::Conflict);
                }
            }
            if inner.routes.values().any(|route| {
                route.closing.is_some() && route.closing_target.as_deref() == Some(target)
            }) {
                return Ok(CreateAdmission::ClosingDeferred);
            }
            if matches!(
                &classification,
                CreateClassification::Preparing { owner_nonce, .. } if *owner_nonce != nonce
            ) {
                return Ok(CreateAdmission::ClosingDeferred);
            }
            let exclusive = !matches!(&classification, CreateClassification::Foreign { .. });
            let owners = inner.create_owners.entry(target.to_owned()).or_default();
            if owners.contains_key(&nonce) {
                return Err(ResponseCloseError::Conflict);
            }
            // Foreign ownership is non-exclusive, including a retained core
            // whose caller was cancelled. Never infer protection from owners.
            if exclusive && owners.values().any(|exclusive| *exclusive) {
                return Ok(CreateAdmission::ClosingDeferred);
            }
            owners.insert(nonce, exclusive);
            for route in inner
                .routes
                .values_mut()
                .filter(|r| r.facts.as_ref().is_some_and(|f| f.target == target))
            {
                if !route.touch(std::time::Instant::now()) {
                    inner.failed = true;
                }
            }
            Ok(CreateAdmission::Reserved(CreateOwnerReservation {
                state: self.clone(),
                target: target.to_owned(),
                nonce,
                classification,
            }))
        })?
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn reserve_policy_candidate(
        &self,
        member: ResponseRouteIdentity,
        coordinator: ResponseRouteIdentity,
        key: String,
        digest: String,
        now: std::time::Instant,
    ) -> Result<ResponseDeliveryContext, ResponseCloseError> {
        self.with_inner(|inner| {
            if !inner.ready || !inner.work_bootstrapped || !inner.enabled {
                return Err(ResponseCloseError::Unavailable);
            }
            if inner
                .work
                .get(&member.target)
                .is_some_and(|w| w.terminal.contains_key(&key))
            {
                return Err(ResponseCloseError::Conflict);
            }
            if !inner
                .work
                .get(&member.target)
                .is_some_and(|w| w.pending.get(&key) == Some(&digest))
            {
                return Err(ResponseCloseError::Unknown);
            }
            let route = inner
                .routes
                .get_mut(&member.session_id)
                .ok_or(ResponseCloseError::Unknown)?;
            if route.facts.as_ref() != Some(&member) || !route.eligible || route.closing.is_some() {
                return Err(ResponseCloseError::Unknown);
            }
            let nonce = Uuid::new_v4();
            route.policy = Some(ResponseClosePolicy {
                nonce,
                member: member.clone(),
                coordinator,
                work_epoch: route.work_epoch,
                settings_epoch: inner.settings_epoch,
                request_key: key,
                request_digest: digest,
                opened_at: now,
                submitted: false,
                registered_at: None,
                success_at: None,
                returns: HashMap::new(),
                selected: None,
            });
            Ok(ResponseDeliveryContext {
                session_id: member.session_id,
                nonce,
            })
        })?
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn mark_request_submitted(
        &self,
        context: &ResponseDeliveryContext,
        submitted: bool,
    ) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner.routes.get_mut(&context.session_id) {
                if route
                    .policy
                    .as_ref()
                    .is_some_and(|p| p.nonce == context.nonce)
                {
                    if submitted {
                        route.policy.as_mut().unwrap().submitted = true;
                    } else {
                        route.policy = None;
                    }
                }
            }
        });
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn register_request(
        &self,
        proof: ResponseDeliveryProof,
        now: std::time::Instant,
    ) -> Result<(), ResponseCloseError> {
        self.with_inner(|inner| {
            if !inner.ready || !inner.enabled {
                return Err(ResponseCloseError::Unavailable);
            }
            let route = inner
                .routes
                .get_mut(&proof.context.session_id)
                .ok_or(ResponseCloseError::Unknown)?;
            let policy = route
                .policy
                .as_mut()
                .filter(|p| p.nonce == proof.context.nonce)
                .ok_or(ResponseCloseError::Unknown)?;
            let work = inner.work.entry(policy.member.target.clone()).or_default();
            let valid = policy.submitted
                && policy.member == proof.member
                && policy.coordinator == proof.coordinator
                && policy.settings_epoch == inner.settings_epoch
                && policy.work_epoch == route.work_epoch
                && proof.published_carrier_version == policy.member.carrier_version
                && route.writers == 0
                && work.provisional == 0
                && work.pending.keys().all(|key| key == &policy.request_key);
            if !valid {
                route.policy = None;
                return Err(ResponseCloseError::Unknown);
            }
            work.pending.remove(&policy.request_key);
            work.terminal
                .insert(policy.request_key.clone(), policy.request_digest.clone());
            policy.registered_at = Some(now);
            route.last_change = route.last_change.max(now);
            Ok(())
        })?
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn admit_return(
        &self,
        member: &ResponseRouteIdentity,
        coordinator: &ResponseRouteIdentity,
        key: String,
        digest: String,
        now: std::time::Instant,
    ) -> Result<ReturnObservationContext, ResponseCloseError> {
        self.with_inner(|inner| {
            if !inner.ready || !inner.enabled {
                return Err(ResponseCloseError::Unavailable);
            }
            let terminal_key = (member.clone(), coordinator.clone(), key.clone());
            if let Some(previous) = inner.terminal_returns.get(&terminal_key) {
                return Err(if previous == &digest {
                    ResponseCloseError::Unknown
                } else {
                    ResponseCloseError::Conflict
                });
            }
            if !inner
                .routes
                .get(&member.session_id)
                .is_some_and(|r| r.facts.as_ref() == Some(member))
                || !inner
                    .routes
                    .get(&coordinator.session_id)
                    .is_some_and(|r| r.facts.as_ref() == Some(coordinator))
            {
                return Err(ResponseCloseError::Unknown);
            }
            let route = inner
                .routes
                .get_mut(&member.session_id)
                .ok_or(ResponseCloseError::Unknown)?;
            let policy = route.policy.as_mut().ok_or(ResponseCloseError::Unknown)?;
            if policy.member != *member
                || policy.coordinator != *coordinator
                || now < policy.opened_at
                || policy.settings_epoch != inner.settings_epoch
            {
                return Err(ResponseCloseError::Unknown);
            }
            let sequence = if let Some((old, sequence)) = policy.returns.get(&key) {
                if old != &digest {
                    return Err(ResponseCloseError::Conflict);
                }
                *sequence
            } else {
                let Some(sequence) = inner.next_return_sequence.checked_add(1) else {
                    inner.failed = true;
                    return Err(ResponseCloseError::Unavailable);
                };
                inner.next_return_sequence = sequence;
                policy
                    .returns
                    .insert(key.clone(), (digest.clone(), sequence));
                policy.selected = None;
                route.last_change = route.last_change.max(now);
                sequence
            };
            Ok(ReturnObservationContext {
                session_id: member.session_id,
                nonce: policy.nonce,
                key,
                digest,
                member: member.clone(),
                coordinator: coordinator.clone(),
                work_epoch: route.work_epoch,
                settings_epoch: inner.settings_epoch,
                sequence,
            })
        })?
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn observe_return_success(
        &self,
        proof: ReturnDeliveryProof,
        now: std::time::Instant,
    ) -> Result<(), ResponseCloseError> {
        self.settle_return(&proof.context, true, now)
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn settle_return_terminal(
        &self,
        context: &ReturnObservationContext,
        now: std::time::Instant,
    ) -> Result<(), ResponseCloseError> {
        self.settle_return(context, false, now)
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    fn settle_return(
        &self,
        context: &ReturnObservationContext,
        success: bool,
        now: std::time::Instant,
    ) -> Result<(), ResponseCloseError> {
        self.with_inner(|inner| {
            let terminal_key = (
                context.member.clone(),
                context.coordinator.clone(),
                context.key.clone(),
            );
            if let Some(digest) = inner.terminal_returns.get(&terminal_key) {
                return if digest == &context.digest {
                    Ok(())
                } else {
                    Err(ResponseCloseError::Conflict)
                };
            }
            // Terminal dedupe survives disarm or successor registration. A
            // stale context may settle its source but cannot arm that successor.
            inner
                .terminal_returns
                .insert(terminal_key, context.digest.clone());
            let route = inner
                .routes
                .get_mut(&context.session_id)
                .ok_or(ResponseCloseError::Unknown)?;
            let policy = route
                .policy
                .as_mut()
                .filter(|p| {
                    p.nonce == context.nonce
                        && p.member == context.member
                        && p.coordinator == context.coordinator
                        && p.work_epoch == context.work_epoch
                })
                .ok_or(ResponseCloseError::Unknown)?;
            let (digest, sequence) = policy
                .returns
                .get(&context.key)
                .ok_or(ResponseCloseError::Unknown)?;
            if digest != &context.digest || *sequence != context.sequence {
                return Err(ResponseCloseError::Conflict);
            }
            policy.returns.remove(&context.key);
            if success && context.settings_epoch == inner.settings_epoch {
                policy.success_at = Some(policy.success_at.map_or(now, |old| old.max(now)));
            }
            policy.selected = None;
            route.last_change = route.last_change.max(now);
            Ok(())
        })?
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    fn safe(
        inner: &ResponseCloseInner,
        route: &ResponseCloseRoute,
        now: std::time::Instant,
    ) -> bool {
        let Some(policy) = &route.policy else {
            return false;
        };
        let Some(registered) = policy.registered_at else {
            return false;
        };
        let Some(success) = policy.success_at else {
            return false;
        };
        let work_safe = inner
            .work
            .get(&policy.member.target)
            .is_none_or(|w| w.provisional == 0 && w.dispatch == 0 && w.pending.is_empty());
        inner.unknown_spawns == 0
            && inner.ready
            && inner.work_bootstrapped
            && inner.enabled
            && (1..=3600).contains(&inner.seconds)
            && !inner.failed
            && route.closing.is_none()
            && route.eligible
            && route.facts.as_ref() == Some(&policy.member)
            && inner
                .routes
                .get(&policy.coordinator.session_id)
                .is_some_and(|r| r.facts.as_ref() == Some(&policy.coordinator))
            && route.manager_working == Some(false)
            && route.detector_working == Some(false)
            && route.menu_blocked == Some(false)
            && route.hold_known
            && route.hold_until.is_none_or(|end| now > end)
            && route.held.is_empty()
            && route.writers == 0
            && work_safe
            && !inner.create_owners.contains_key(&policy.member.target)
            && policy.returns.is_empty()
            && policy.settings_epoch == inner.settings_epoch
            && policy.work_epoch == route.work_epoch
            && now.saturating_duration_since(
                registered
                    .max(success)
                    .max(route.last_change)
                    .max(route.hold_until.unwrap_or(registered)),
            ) >= std::time::Duration::from_secs(u64::from(inner.seconds))
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn select_ticket(
        &self,
        id: Uuid,
        now: std::time::Instant,
    ) -> Result<ResponseCloseTicket, ResponseCloseError> {
        self.with_inner(|inner| {
            let route = inner.routes.get(&id).ok_or(ResponseCloseError::Unknown)?;
            if !Self::safe(inner, route, now) {
                return Err(ResponseCloseError::Unknown);
            }
            let route = inner.routes.get_mut(&id).unwrap();
            let policy = route.policy.as_mut().unwrap();
            let selection_nonce = Uuid::new_v4();
            policy.selected = Some(selection_nonce);
            Ok(ResponseCloseTicket {
                session_id: id,
                generation: route.generation,
                nonce: policy.nonce,
                selection_nonce,
                work_epoch: route.work_epoch,
                activity_version: route.activity_version,
                carrier_version: policy.member.carrier_version,
                settings_epoch: inner.settings_epoch,
                member: policy.member.clone(),
            })
        })?
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    fn try_commit_response_close(
        &self,
        ticket: &ResponseCloseTicket,
        now: std::time::Instant,
    ) -> Result<Uuid, ResponseCloseError> {
        self.with_inner(|inner| {
            let route = inner
                .routes
                .get(&ticket.session_id)
                .ok_or(ResponseCloseError::Unknown)?;
            if !Self::safe(inner, route, now)
                || route.generation != ticket.generation
                || route.work_epoch != ticket.work_epoch
                || route.activity_version != ticket.activity_version
                || inner.settings_epoch != ticket.settings_epoch
                || !route.policy.as_ref().is_some_and(|p| {
                    p.nonce == ticket.nonce
                        && p.selected == Some(ticket.selection_nonce)
                        && p.member.carrier_version == ticket.carrier_version
                        && p.member == ticket.member
                })
            {
                return Err(ResponseCloseError::Unknown);
            }
            let route = inner.routes.get_mut(&ticket.session_id).unwrap();
            route.closing = Some(ticket.selection_nonce);
            route.closing_target = route
                .policy
                .as_ref()
                .map(|policy| policy.member.target.clone());
            route.committed_binding = Some(ticket.committed_binding());
            route.policy = None;
            Ok(ticket.selection_nonce)
        })?
    }
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn finish_response_close(
        &self,
        ticket: &ResponseCloseTicket,
        evidence: ResponseCloseSettlementEvidence,
    ) -> Result<(), StaleOrInvalidSettlement> {
        self.with_inner(|inner| {
            let route = inner
                .routes
                .get(&ticket.session_id)
                .ok_or(StaleOrInvalidSettlement)?;
            let binding = ticket.committed_binding();
            let current = &evidence.current_settlement;
            if evidence.committed_binding != binding
                || route.committed_binding.as_ref() != Some(&binding)
                || route.closing != Some(binding.close_nonce)
                || route.closing_target.as_deref() != Some(binding.member.target.as_str())
                || route.generation != binding.member.generation
                || route.manager_token != Some(binding.member.token)
                || route.manager_version != current.current_carrier_version
                || current.current_carrier_version < binding.member.carrier_version
                || current.confirmed_carrier_version != current.current_carrier_version
                || current.checkpoint_nonce != current.current_checkpoint_nonce
                || current.publication_epoch != current.current_publication_epoch
            {
                return Err(StaleOrInvalidSettlement);
            }
            match evidence.outcome {
                ResponseCloseSettlementOutcome::Dormant {
                    exact_backend_absent: true,
                    all_effects_completed: true,
                    retained_exited_confirmed: true,
                    selection_settled: true,
                } => {
                    if route.manager_working != Some(false) || route.eligible {
                        return Err(StaleOrInvalidSettlement);
                    }
                    inner.routes.remove(&ticket.session_id);
                }
                ResponseCloseSettlementOutcome::ReopenAfterKillFailure {
                    terminal_kill_failure: true,
                    all_effects_terminal: true,
                    original_backend_alive: true,
                    original_manager_identity_intact: true,
                } => {
                    let route = inner.routes.get_mut(&ticket.session_id).unwrap();
                    route.closing = None;
                    route.closing_target = None;
                    route.committed_binding = None;
                    route.policy = None;
                }
                _ => return Err(StaleOrInvalidSettlement),
            }
            Ok(())
        })
        .map_err(|_| StaleOrInvalidSettlement)?
    }
}

impl WorkAdmission {
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn accept_durable(&mut self, digest: &str) -> Result<bool, ResponseCloseError> {
        self.state.with_inner(|inner| {
            let work = inner
                .work
                .get_mut(&self.target)
                .ok_or(ResponseCloseError::Unknown)?;
            if let Some(old) = work
                .pending
                .get(&self.key)
                .or_else(|| work.terminal.get(&self.key))
            {
                if old != digest {
                    return Err(ResponseCloseError::Conflict);
                }
                return Ok(false);
            }
            work.pending.insert(self.key.clone(), digest.to_owned());
            for route in inner
                .routes
                .values_mut()
                .filter(|r| r.facts.as_ref().is_some_and(|f| f.target == self.target))
            {
                if !route.invalidate(std::time::Instant::now()) {
                    inner.failed = true;
                }
            }
            Ok(true)
        })?
    }
}

impl Drop for WorkAdmission {
    fn drop(&mut self) {
        if self.active {
            let _ = self.state.with_inner(|inner| {
                if let Some(work) = inner.work.get_mut(&self.target) {
                    work.provisional = work.provisional.saturating_sub(1);
                }
                for route in inner
                    .routes
                    .values_mut()
                    .filter(|r| r.facts.as_ref().is_some_and(|f| f.target == self.target))
                {
                    if !route.touch(std::time::Instant::now()) {
                        inner.failed = true;
                    }
                }
            });
        }
    }
}
impl Drop for WorkDeliveryReservation {
    fn drop(&mut self) {
        let _ = self.state.with_inner(|inner| {
            if let Some(work) = inner.work.get_mut(&self.target) {
                work.dispatch = work.dispatch.saturating_sub(1);
            }
            for route in inner
                .routes
                .values_mut()
                .filter(|r| r.facts.as_ref().is_some_and(|f| f.target == self.target))
            {
                if !route.touch(std::time::Instant::now()) {
                    inner.failed = true;
                }
            }
        });
    }
}
impl Drop for WriterAdmission {
    fn drop(&mut self) {
        if !self.counted {
            return;
        }
        let _ = self.state.with_inner(|inner| {
            if let Some(route) = inner
                .routes
                .get_mut(&self.session_id)
                .filter(|r| r.generation == self.generation)
            {
                route.writers = route.writers.saturating_sub(1);
                if !route.touch(std::time::Instant::now()) {
                    inner.failed = true;
                }
            }
        });
    }
}
impl Drop for CreateOwnerReservation {
    fn drop(&mut self) {
        let _ = self.state.with_inner(|inner| {
            let removed = if let Some(owners) = inner.create_owners.get_mut(&self.target) {
                let removed = owners.remove(&self.nonce).is_some();
                if owners.is_empty() {
                    inner.create_owners.remove(&self.target);
                }
                removed
            } else {
                false
            };
            if removed {
                for route in inner
                    .routes
                    .values_mut()
                    .filter(|r| r.facts.as_ref().is_some_and(|f| f.target == self.target))
                {
                    if !route.touch(std::time::Instant::now()) {
                        inner.failed = true;
                    }
                }
            }
        });
    }
}

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
    _response_writer: WriterAdmission,
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
    response_close: ResponseCloseState,
    registry: Arc<Mutex<SpawnRegistry>>,
    seq: u64,
}

impl Drop for SpawnMark {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry.pending.remove(&self.seq);
        self.response_close.end_unresolved_spawn();
    }
}

pub struct PtyManager {
    response_close: ResponseCloseState,
    registry: Arc<Mutex<SpawnRegistry>>,
    local_backend: Arc<dyn PtyBackend>,
    container_backend: Arc<ContainerTransportBackend>,
}

/// An owned shallow route selected while PtyManager is locked. All forwarding happens after the
/// caller has dropped that manager lock, so the output path never inherits it.
pub(crate) struct PtyTerminalOutputRoute {
    session_id: Uuid,
    backend: Arc<dyn PtyBackend>,
}

impl PtyTerminalOutputRoute {
    pub(crate) fn activate_terminal_output(
        &self,
        label: &str,
        include_history: bool,
    ) -> Result<Option<PtyScreenSnapshot>, TerminalOutputAttachError> {
        self.backend
            .activate_terminal_output(self.session_id, label, include_history)
    }

    pub(crate) fn detach_terminal_output(&self, label: &str) {
        self.backend.detach_terminal_output(self.session_id, label);
    }
}

impl PtyManager {
    pub(crate) fn bind_response_close(&mut self, state: ResponseCloseState) {
        state.bind_publisher(ResponseClosePublisher::Pty);
        self.response_close = state;
    }
    pub(crate) fn response_close_state(&self) -> ResponseCloseState {
        self.response_close.clone()
    }
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
        Self {
            response_close: ResponseCloseState::default(),
            registry: Arc::new(Mutex::new(SpawnRegistry::default())),
            local_backend,
            container_backend,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(local_backend: Arc<dyn PtyBackend>) -> Self {
        let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        Self {
            response_close: ResponseCloseState::default(),
            registry: Arc::new(Mutex::new(SpawnRegistry::default())),
            local_backend,
            container_backend: Arc::new(ContainerTransportBackend::new(
                output_senders,
                idle_detector,
                None,
                None,
            )),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_container_backend(
        local_backend: Arc<dyn PtyBackend>,
        container_backend: Arc<ContainerTransportBackend>,
    ) -> Self {
        Self {
            response_close: ResponseCloseState::default(),
            registry: Arc::new(Mutex::new(SpawnRegistry::default())),
            local_backend,
            container_backend,
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

    pub(crate) fn terminal_output_route(
        &self,
        session_id: Uuid,
    ) -> Result<PtyTerminalOutputRoute, AppError> {
        let kind = self.kind_for_session(session_id)?;
        Ok(PtyTerminalOutputRoute {
            session_id,
            backend: self.backend_for_kind(kind),
        })
    }

    /// #1363 - a destroyed window releases every terminal-output attachment it held, in both
    /// backends. The window that died is the only thing identified, so a session another window
    /// still watches keeps delivering.
    pub(crate) fn release_window_attachments(&self, label: &str) {
        self.local_backend.release_window_attachments(label);
        self.container_backend.release_window_attachments(label);
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
        if self.response_close.uuid_closing(id) {
            return Err(AppError::PtyError("response_close_deferred".to_string()));
        }
        if registry.routes.contains_key(&id) {
            return Err(AppError::PtyError("duplicate_pty_route".to_string()));
        }
        let generation = registry
            .next_route_generation
            .checked_add(1)
            .ok_or_else(|| AppError::PtyError("route_generation_overflow".to_string()))?;
        registry.next_route_generation = generation;
        self.response_close
            .register_route(id, generation, std::time::Instant::now());
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
        let deferred_lifecycle = Arc::clone(&lifecycle);
        match lifecycle.try_lock() {
            Ok(_lifecycle_guard) => {
                let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
                if registry
                    .routes
                    .get(&id)
                    .is_some_and(|entry| entry.kind == kind && entry.generation == generation)
                {
                    self.response_close.remove_route(id, generation);
                    registry.routes.remove(&id);
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                log::debug!("[pty] route removal deferred for busy session {id}");
                let registry = Arc::clone(&self.registry);
                let response_close = self.response_close.clone();
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
                        response_close.remove_route(id, generation);
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
            self.response_close.remove_route(id, generation);
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
        if registry.next_seq == u64::MAX {
            self.response_close.fail_closed();
        }
        self.response_close.begin_unresolved_spawn();
        registry.next_seq = registry.next_seq.wrapping_add(1);
        registry.pending.insert(
            seq,
            PendingSpawn {
                cwd: cwd.to_string(),
                label: label.to_string(),
            },
        );
        SpawnMark {
            response_close: self.response_close.clone(),
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

    /// #2586 - best-effort rows for an observation, not a gate. `Unavailable` and
    /// `SessionOver` both answer `None`: an observer must not branch on why.
    pub fn screen_rows_snapshot(&self, id: Uuid) -> Option<Vec<String>> {
        match self.get_screen_rows(id) {
            ScreenRowsRead::Rows(rows) => Some(rows),
            ScreenRowsRead::Unavailable | ScreenRowsRead::SessionOver => None,
        }
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
    let (route_registry, input_gate, route_lifecycle, generation, backend, response_close) = {
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
            manager_guard.response_close.clone(),
        )
    };

    let response_writer = response_close
        .reserve_writer(session_id, generation)
        .map_err(|_| AppError::PtyError("response_close_deferred".to_string()))?;
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
        _response_writer: response_writer,
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

impl PtyAuthorityRouteProof {
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(crate) fn try_commit_response_close(
        &self,
        state: &ResponseCloseState,
        ticket: &ResponseCloseTicket,
        now: std::time::Instant,
    ) -> Result<Uuid, ResponseCloseError> {
        if self.session_id != ticket.session_id || self.route_generation != ticket.generation {
            return Err(ResponseCloseError::Unknown);
        }
        let _lifecycle = self
            .route_lifecycle
            .try_lock()
            .map_err(|_| ResponseCloseError::Unavailable)?;
        let registry = self
            .route_registry
            .lock()
            .map_err(|_| ResponseCloseError::Unavailable)?;
        if !registry.routes.get(&self.session_id).is_some_and(|route| {
            route.generation == self.route_generation && route.kind == ticket.member.backend
        }) {
            return Err(ResponseCloseError::Unknown);
        }
        state.try_commit_response_close(ticket, now)
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
        if permit
            ._response_writer
            .state
            .is_closing(permit.session_id, permit.route_generation)
            .unwrap_or(true)
        {
            return Err(AppError::PtyError("response_close_deferred".to_string()));
        }
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
        if permit
            ._response_writer
            .state
            .is_closing(permit.session_id, permit.route_generation)
            .unwrap_or(true)
        {
            return Err(AppError::PtyError("response_close_deferred".to_string()));
        }
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
    }

    #[derive(Default)]
    struct RecordingBackend {
        calls: Mutex<Vec<Call>>,
        live: Mutex<std::collections::HashSet<Uuid>>,
    }

    impl RecordingBackend {
        fn set_live(&self, id: Uuid) {
            self.live.lock().unwrap().insert(id);
        }

        fn calls(&self) -> Vec<Call> {
            let mut calls = self.calls.lock().unwrap();
            std::mem::take(&mut *calls)
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

        fn kill_all_jobs(&self) -> (usize, usize) {
            (1, 2)
        }
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
            launch_witness: Default::default(),
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
    async fn response_close_waiting_writer_is_counted_and_cancellation_releases_only_its_admission()
    {
        use std::future::Future;
        let id = Uuid::new_v4();
        let backend = Arc::new(RecordingBackend::default());
        let manager = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        manager
            .lock()
            .unwrap()
            .try_record_route(id, SessionBackendKind::LocalProcess)
            .unwrap();
        let state = manager.lock().unwrap().response_close_state();
        let first = PtyManager::acquire_input_writer(&manager, id)
            .await
            .unwrap();
        let mut waiting = Box::pin(PtyManager::acquire_input_writer(&manager, id));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(waiting.as_mut().poll(&mut context).is_pending());
        assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 2);
        drop(waiting);
        assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 1);
        assert!(backend.calls().is_empty());
        PtyManager::write_with_permit(&first, b"first").unwrap();
        drop(first);
        assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 0);
        assert_eq!(backend.calls(), vec![Call::Write(id, b"first".to_vec())]);
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

#[cfg(test)]
mod response_close_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn response_close_readiness_requires_all_publishers_and_work_bootstrap() {
        let state = ResponseCloseState::default();
        assert_eq!(
            state.try_enable_readiness(),
            Err(ResponseCloseError::Unavailable)
        );
        for publisher in [
            ResponseClosePublisher::Manager,
            ResponseClosePublisher::Pty,
            ResponseClosePublisher::RawOutput,
            ResponseClosePublisher::Detector,
            ResponseClosePublisher::Menu,
            ResponseClosePublisher::Hold,
        ] {
            state.bind_publisher(publisher);
        }
        assert!(state.publishers_bound());
        assert_eq!(
            state.try_enable_readiness(),
            Err(ResponseCloseError::Unavailable)
        );
        state.mark_work_bootstrapped();
        assert_eq!(state.try_enable_readiness(), Ok(()));
        state.fail_closed();
        assert_eq!(
            state.try_enable_readiness(),
            Err(ResponseCloseError::Unavailable)
        );
        let other = ResponseCloseState::default();
        assert!(!state.same_state(&other));
        assert!(state.same_state(&state.clone()));
    }

    #[test]
    fn response_close_cancelled_delivery_keeps_durable_work_and_terminal_dedupe() {
        let state = ResponseCloseState::default();
        let mut admission = state.reserve_work("target", "request").unwrap();
        assert_eq!(admission.accept_durable("digest"), Ok(true));
        drop(admission);
        let delivery = state.begin_delivery("target").unwrap();
        drop(delivery);
        {
            let inner = state.inner.lock().unwrap();
            assert_eq!(
                inner.work["target"]
                    .pending
                    .get("request")
                    .map(String::as_str),
                Some("digest")
            );
            assert_eq!(inner.work["target"].dispatch, 0);
            assert_eq!(inner.work["target"].provisional, 0);
        }
        state.settle_terminal("target", "request", Instant::now());
        let mut duplicate = state.reserve_work("target", "request").unwrap();
        assert_eq!(duplicate.accept_durable("digest"), Ok(false));
        assert_eq!(
            duplicate.accept_durable("collision"),
            Err(ResponseCloseError::Conflict)
        );
        assert!(state.inner.lock().unwrap().work["target"]
            .pending
            .is_empty());
    }

    #[test]
    fn response_close_create_owner_drop_cannot_release_successor_nonce() {
        let now = Instant::now() + Duration::from_secs(3600);
        let (state, member, coordinator) = routes(now);
        arm(&state, &member, &coordinator, now);
        let first = Uuid::new_v4();
        let foreign = || CreateClassification::Foreign {
            publication_epoch: 1,
        };
        let protected_classification = || CreateClassification::Protected {
            identity: member.clone(),
            publication_epoch: 1,
        };
        let CreateAdmission::Reserved(owner) = state
            .reserve_create_owner(&member.target, first, foreign())
            .unwrap()
        else {
            panic!("Foreign ownership must progress");
        };
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(100))
            .is_err());
        let successor = Uuid::new_v4();
        let CreateAdmission::Reserved(successor_owner) = state
            .reserve_create_owner(&member.target, successor, foreign())
            .unwrap()
        else {
            panic!("retained Foreign core cannot defer another Foreign owner");
        };
        assert!(matches!(
            state.reserve_create_owner(
                &member.target,
                Uuid::new_v4(),
                CreateClassification::Unknown,
            ),
            Ok(CreateAdmission::WaitingIdentity)
        ));
        assert!(matches!(
            state.reserve_create_owner(
                &member.target,
                Uuid::new_v4(),
                CreateClassification::Preparing {
                    owner_nonce: first,
                    identity: member.clone(),
                    publication_epoch: 1,
                },
            ),
            Ok(CreateAdmission::ClosingDeferred)
        ));
        drop(owner);
        assert_eq!(
            state
                .inner
                .lock()
                .unwrap()
                .create_owners
                .get(&member.target)
                .and_then(|owners| owners.get(&successor)),
            Some(&false)
        );
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(100))
            .is_err());
        drop(successor_owner);
        let CreateAdmission::Reserved(protected) = state
            .reserve_create_owner(&member.target, first, protected_classification())
            .unwrap()
        else {
            panic!("first protected owner");
        };
        assert!(matches!(
            state.reserve_create_owner(&member.target, successor, protected_classification()),
            Ok(CreateAdmission::ClosingDeferred)
        ));
        drop(protected);
        assert!(!state
            .inner
            .lock()
            .unwrap()
            .create_owners
            .contains_key(&member.target));
        // Foreign does not demand a canonical FQN before legacy preflight.
        let CreateAdmission::Reserved(alias) = state
            .reserve_create_owner("legacy/alias", first, foreign())
            .unwrap()
        else {
            panic!("legacy alias Foreign ownership");
        };
        assert!(matches!(
            alias.classification(),
            CreateClassification::Foreign {
                publication_epoch: 1
            }
        ));
        drop(alias);
    }

    #[test]
    fn response_close_old_writer_drop_cannot_change_replacement_generation() {
        let now = Instant::now();
        let state = ResponseCloseState::default();
        let id = Uuid::new_v4();
        state.register_route(id, 1, now);
        let old = state.reserve_writer(id, 1).unwrap();
        state.remove_route(id, 1);
        state.register_route(id, 2, now);
        let new = state.reserve_writer(id, 2).unwrap();
        drop(old);
        assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 1);
        assert!(state.reserve_writer(id, 1).is_err());
        drop(new);
        assert_eq!(state.inner.lock().unwrap().routes[&id].writers, 0);
    }

    #[test]
    fn response_close_counter_overflow_disables_feature_but_preserves_ordinary_writer() {
        let state = ResponseCloseState::default();
        let id = Uuid::new_v4();
        state.register_route(id, 1, Instant::now());
        state
            .inner
            .lock()
            .unwrap()
            .work
            .entry("target".into())
            .or_default()
            .provisional = usize::MAX;
        assert!(matches!(
            state.reserve_work("target", "request"),
            Err(ResponseCloseError::Unavailable)
        ));
        assert!(state.inner.lock().unwrap().failed);
        assert!(state.reserve_writer(id, 1).is_ok());
    }

    fn routes(
        now: Instant,
    ) -> (
        ResponseCloseState,
        ResponseRouteIdentity,
        ResponseRouteIdentity,
    ) {
        let state = ResponseCloseState::default();
        state.apply_settings(true, 30, now);
        state.inner.lock().unwrap().ready = true;
        state.mark_work_bootstrapped();
        let member = ResponseRouteIdentity {
            target: "project:room/member".into(),
            session_id: Uuid::new_v4(),
            token: Uuid::new_v4(),
            generation: 1,
            carrier_version: 1,
            backend: SessionBackendKind::LocalProcess,
        };
        let coordinator = ResponseRouteIdentity {
            target: "project:room/coordinator".into(),
            session_id: Uuid::new_v4(),
            token: Uuid::new_v4(),
            generation: 2,
            carrier_version: 1,
            backend: SessionBackendKind::LocalProcess,
        };
        for (identity, eligible) in [(&member, true), (&coordinator, false)] {
            state.register_route(identity.session_id, identity.generation, now);
            state.publish_manager_row(
                identity.session_id,
                identity.token,
                identity.generation,
                identity.carrier_version,
                eligible,
                false,
                now,
            );
            state.publish_manager_facts(identity.clone(), eligible, false, now);
        }
        state.observe_detector(member.session_id, false, now);
        state.publish_menu(member.session_id, Some(false), now);
        state.publish_hold(member.session_id, None, true, now);
        (state, member, coordinator)
    }

    fn candidate(
        state: &ResponseCloseState,
        member: &ResponseRouteIdentity,
        coordinator: &ResponseRouteIdentity,
        key: &str,
        now: Instant,
    ) -> ResponseDeliveryContext {
        let mut work = state.reserve_work(&member.target, key).unwrap();
        work.accept_durable(key).unwrap();
        drop(work);
        let writer = state
            .reserve_writer(member.session_id, member.generation)
            .unwrap();
        let context = state
            .reserve_policy_candidate(
                member.clone(),
                coordinator.clone(),
                key.into(),
                key.into(),
                now,
            )
            .unwrap();
        drop(writer);
        context
    }

    fn register(
        state: &ResponseCloseState,
        member: &ResponseRouteIdentity,
        coordinator: &ResponseRouteIdentity,
        context: ResponseDeliveryContext,
        now: Instant,
    ) {
        state.mark_request_submitted(&context, true);
        assert!(state
            .register_request(
                ResponseDeliveryProof {
                    context,
                    member: member.clone(),
                    coordinator: coordinator.clone(),
                    published_carrier_version: member.carrier_version
                },
                now
            )
            .is_ok());
    }

    fn arm(
        state: &ResponseCloseState,
        member: &ResponseRouteIdentity,
        coordinator: &ResponseRouteIdentity,
        now: Instant,
    ) {
        let context = candidate(state, member, coordinator, "request", now);
        register(state, member, coordinator, context, now);
        let context = state
            .admit_return(member, coordinator, "ack".into(), "ack".into(), now)
            .unwrap();
        state
            .observe_return_success(ReturnDeliveryProof { context }, now)
            .unwrap();
    }

    #[test]
    fn response_close_readiness_is_disabled_even_with_enabled_settings() {
        let now = Instant::now();
        let (state, member, coordinator) = routes(now);
        state.inner.lock().unwrap().ready = false;
        assert!(state
            .reserve_policy_candidate(member, coordinator, "req".into(), "digest".into(), now)
            .is_err());
    }

    #[test]
    fn response_close_coordinator_identity_change_and_restore_never_resurrects_policy() {
        let now = Instant::now() + Duration::from_secs(3600);
        let (state, member, coordinator) = routes(now);
        arm(&state, &member, &coordinator, now);
        let mut changed = coordinator.clone();
        changed.target = "project:room/another-coordinator".into();
        state.publish_manager_facts(changed, false, false, now);
        state.publish_manager_facts(coordinator, false, false, now);
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(100))
            .is_err());
        assert!(state.inner.lock().unwrap().routes[&member.session_id]
            .policy
            .is_none());
    }

    #[test]
    fn response_close_ordinary_ack_requires_full_interval_and_raw_output_revokes_ticket() {
        let now = Instant::now() + Duration::from_secs(3600);
        let nonce = ordinary_ack_dormant_settlement(now);
        ordinary_ack_reopen_and_fresh_return(now, nonce);
    }

    fn ordinary_ack_dormant_settlement(now: Instant) -> Uuid {
        let (state, member, coordinator) = routes(now);
        arm(&state, &member, &coordinator, now);
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(29))
            .is_err());
        let ticket = state
            .select_ticket(member.session_id, now + Duration::from_secs(30))
            .unwrap();
        state.observe_raw_output(member.session_id, now + Duration::from_secs(30));
        assert!(state
            .try_commit_response_close(&ticket, now + Duration::from_secs(60))
            .is_err());
        assert!(!state
            .is_closing(member.session_id, member.generation)
            .unwrap());
        let ticket = state
            .select_ticket(member.session_id, now + Duration::from_secs(60))
            .unwrap();
        let nonce = state
            .try_commit_response_close(&ticket, now + Duration::from_secs(60))
            .unwrap();
        assert!(matches!(
            state.reserve_writer(member.session_id, member.generation),
            Err(ResponseCloseError::ClosingDeferred)
        ));
        let mut pending = state.reserve_work(&member.target, "after-commit").unwrap();
        assert_eq!(pending.accept_durable("retained-body"), Ok(true));
        drop(pending);
        assert!(matches!(
            state.begin_delivery(&member.target),
            Err(ResponseCloseError::ClosingDeferred)
        ));
        assert_eq!(
            state.inner.lock().unwrap().work[&member.target]
                .pending
                .get("after-commit")
                .map(String::as_str),
            Some("retained-body")
        );
        state.observe_detector(member.session_id, true, now + Duration::from_secs(61));
        state.apply_settings(false, 30, now + Duration::from_secs(62));
        assert!(state
            .is_closing(member.session_id, member.generation)
            .unwrap());
        assert_eq!(ticket.committed_binding().close_nonce, nonce);
        let dormant = || ResponseCloseSettlementOutcome::Dormant {
            exact_backend_absent: true,
            all_effects_completed: true,
            retained_exited_confirmed: true,
            selection_settled: true,
        };
        let evidence = |version, outcome| {
            let checkpoint_nonce = Uuid::new_v4();
            ResponseCloseSettlementEvidence {
                committed_binding: ticket.committed_binding(),
                current_settlement: ResponseCloseCurrentSettlement {
                    current_carrier_version: version,
                    confirmed_carrier_version: version,
                    checkpoint_nonce,
                    current_checkpoint_nonce: checkpoint_nonce,
                    publication_epoch: 2,
                    current_publication_epoch: 2,
                },
                outcome,
            }
        };
        // Same-UUID stable carrier changes advance current facts without
        // relabelling the committed ticket or reviving its consumed lease.
        state.publish_manager_row(
            member.session_id,
            member.token,
            member.generation,
            2,
            false,
            false,
            now + Duration::from_secs(63),
        );
        assert_eq!(ticket.committed_binding().member.carrier_version, 1);
        let mut stale = evidence(2, dormant());
        stale.current_settlement.confirmed_carrier_version = 1;
        assert_eq!(
            state.finish_response_close(&ticket, stale),
            Err(StaleOrInvalidSettlement)
        );
        assert_eq!(
            state.finish_response_close(&ticket, evidence(1, dormant())),
            Err(StaleOrInvalidSettlement)
        );
        let mut stale = evidence(2, dormant());
        stale.current_settlement.current_checkpoint_nonce = Uuid::new_v4();
        assert_eq!(
            state.finish_response_close(&ticket, stale),
            Err(StaleOrInvalidSettlement)
        );
        let mut stale = evidence(2, dormant());
        stale.current_settlement.current_publication_epoch += 1;
        assert_eq!(
            state.finish_response_close(&ticket, stale),
            Err(StaleOrInvalidSettlement)
        );
        let mut stale = evidence(2, dormant());
        stale.committed_binding.member.backend = SessionBackendKind::ContainerTransport;
        assert_eq!(
            state.finish_response_close(&ticket, stale),
            Err(StaleOrInvalidSettlement)
        );
        let mut stale = evidence(2, dormant());
        stale.committed_binding.member.generation += 1;
        assert_eq!(
            state.finish_response_close(&ticket, stale),
            Err(StaleOrInvalidSettlement)
        );
        // None facts are not proof of completion. Running, NotStarted or
        // Ambiguous must be represented by false, not Completed(Err).
        assert_eq!(
            state.finish_response_close(
                &ticket,
                evidence(
                    2,
                    ResponseCloseSettlementOutcome::Dormant {
                        exact_backend_absent: true,
                        all_effects_completed: false,
                        retained_exited_confirmed: true,
                        selection_settled: true,
                    },
                ),
            ),
            Err(StaleOrInvalidSettlement)
        );
        assert!(state
            .is_closing(member.session_id, member.generation)
            .unwrap());
        assert_eq!(
            state.finish_response_close(&ticket, evidence(2, dormant())),
            Ok(())
        );
        assert_eq!(
            state.inner.lock().unwrap().work[&member.target].pending["after-commit"],
            "retained-body"
        );
        nonce
    }

    fn ordinary_ack_reopen_and_fresh_return(now: Instant, nonce: Uuid) {
        // Contrast kill failure: latest LIVE Confirmed carrier, no Exited or
        // selection-clear claim. Every effect must still be terminal.
        let (state, member, coordinator) = routes(now);
        arm(&state, &member, &coordinator, now);
        let ticket = state
            .select_ticket(member.session_id, now + Duration::from_secs(30))
            .unwrap();
        state
            .try_commit_response_close(&ticket, now + Duration::from_secs(30))
            .unwrap();
        state.publish_manager_row(
            member.session_id,
            member.token,
            member.generation,
            2,
            true,
            false,
            now,
        );
        let reopen = |all_effects_terminal| ResponseCloseSettlementEvidence {
            committed_binding: ticket.committed_binding(),
            current_settlement: ResponseCloseCurrentSettlement {
                current_carrier_version: 2,
                confirmed_carrier_version: 2,
                checkpoint_nonce: nonce,
                current_checkpoint_nonce: nonce,
                publication_epoch: 3,
                current_publication_epoch: 3,
            },
            outcome: ResponseCloseSettlementOutcome::ReopenAfterKillFailure {
                terminal_kill_failure: true,
                all_effects_terminal,
                original_backend_alive: true,
                original_manager_identity_intact: true,
            },
        };
        assert_eq!(
            state.finish_response_close(&ticket, reopen(false)),
            Err(StaleOrInvalidSettlement)
        );
        assert!(state
            .is_closing(member.session_id, member.generation)
            .unwrap());
        assert_eq!(state.finish_response_close(&ticket, reopen(true)), Ok(()));
        assert!(!state
            .is_closing(member.session_id, member.generation)
            .unwrap());
        assert!(state.begin_delivery(&member.target).is_ok());
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(100))
            .is_err());
        assert!(state
            .try_commit_response_close(&ticket, now + Duration::from_secs(100))
            .is_err());

        let mut current_member = member.clone();
        current_member.carrier_version = 2;
        state.publish_manager_facts(current_member.clone(), true, false, now);
        // Pure terminal-ledger replay cannot mint another candidate. P05/P06
        // separately verify authenticated admission and on-disk receipt replay.
        assert!(matches!(
            state.reserve_policy_candidate(
                current_member.clone(),
                coordinator.clone(),
                "request".into(),
                "request".into(),
                now + Duration::from_secs(100),
            ),
            Err(ResponseCloseError::Conflict)
        ));

        let new_request = candidate(
            &state,
            &current_member,
            &coordinator,
            "new-request",
            now + Duration::from_secs(100),
        );
        register(
            &state,
            &current_member,
            &coordinator,
            new_request,
            now + Duration::from_secs(100),
        );
        // Registration alone cannot reuse the old ACK or its idle interval.
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(200))
            .is_err());
        let returned_at = now + Duration::from_secs(200);
        let fresh_return = state
            .admit_return(
                &current_member,
                &coordinator,
                "fresh-ack".into(),
                "fresh-ack".into(),
                returned_at,
            )
            .unwrap();
        state
            .observe_return_success(
                ReturnDeliveryProof {
                    context: fresh_return,
                },
                returned_at,
            )
            .unwrap();
        assert!(matches!(
            state.select_ticket(
                member.session_id,
                returned_at + Duration::from_secs(30) - Duration::from_nanos(1),
            ),
            Err(ResponseCloseError::Unknown)
        ));
        assert!(state
            .select_ticket(member.session_id, returned_at + Duration::from_secs(30))
            .is_ok());
    }
    #[test]
    fn response_close_early_return_waits_for_complete_submission_and_registration() {
        let now = Instant::now() + Duration::from_secs(3600);
        let (state, member, coordinator) = routes(now);
        let mut work = state.reserve_work(&member.target, "request").unwrap();
        work.accept_durable("request").unwrap();
        drop(work);
        let writer = state
            .reserve_writer(member.session_id, member.generation)
            .unwrap();
        let candidate = state
            .reserve_policy_candidate(
                member.clone(),
                coordinator.clone(),
                "request".into(),
                "request".into(),
                now,
            )
            .unwrap();
        let early = state
            .admit_return(
                &member,
                &coordinator,
                "early".into(),
                "early".into(),
                now + Duration::from_secs(1),
            )
            .unwrap();
        state
            .observe_return_success(
                ReturnDeliveryProof { context: early },
                now + Duration::from_secs(2),
            )
            .unwrap();
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(100))
            .is_err());
        drop(writer);
        register(
            &state,
            &member,
            &coordinator,
            candidate,
            now + Duration::from_secs(20),
        );
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(49))
            .is_err());
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(50))
            .is_ok());
    }

    #[test]
    fn response_close_stale_return_and_failed_submission_never_arm_successor() {
        let now = Instant::now() + Duration::from_secs(3600);
        let (state, member, coordinator) = routes(now);
        assert!(state
            .admit_return(&member, &coordinator, "pre".into(), "pre".into(), now)
            .is_err());
        let old = candidate(&state, &member, &coordinator, "old", now);
        let early = state
            .admit_return(&member, &coordinator, "reply".into(), "reply".into(), now)
            .unwrap();
        state.mark_request_submitted(&old, false);
        state.settle_terminal(&member.target, "old", now);
        let next = candidate(&state, &member, &coordinator, "next", now);
        register(&state, &member, &coordinator, next, now);
        assert!(state
            .observe_return_success(ReturnDeliveryProof { context: early }, now)
            .is_err());
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(100))
            .is_err());
    }

    #[test]
    fn response_close_pending_returns_dedupe_and_out_of_order_settlement_restart_clock() {
        let now = Instant::now() + Duration::from_secs(3600);
        let (state, member, coordinator) = routes(now);
        arm(&state, &member, &coordinator, now);
        let a = state
            .admit_return(
                &member,
                &coordinator,
                "a".into(),
                "a".into(),
                now + Duration::from_secs(10),
            )
            .unwrap();
        let b = state
            .admit_return(
                &member,
                &coordinator,
                "b".into(),
                "b".into(),
                now + Duration::from_secs(11),
            )
            .unwrap();
        let duplicate = state
            .admit_return(
                &member,
                &coordinator,
                "b".into(),
                "b".into(),
                now + Duration::from_secs(12),
            )
            .unwrap();
        assert_eq!(b.sequence, duplicate.sequence);
        assert!(matches!(
            state.admit_return(&member, &coordinator, "b".into(), "collision".into(), now),
            Err(ResponseCloseError::Conflict)
        ));
        state
            .observe_return_success(
                ReturnDeliveryProof { context: b.clone() },
                now + Duration::from_secs(20),
            )
            .unwrap();
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(100))
            .is_err());
        state
            .settle_return_terminal(&a, now + Duration::from_secs(25))
            .unwrap();
        state
            .observe_return_success(
                ReturnDeliveryProof { context: b },
                now + Duration::from_secs(40),
            )
            .unwrap();
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(54))
            .is_err());
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(55))
            .is_ok());
    }

    #[test]
    fn response_close_unknown_hold_held_keys_and_native_busy_veto_delayed_manager_idle() {
        let now = Instant::now() + Duration::from_secs(3600);
        let (state, member, coordinator) = routes(now);
        arm(&state, &member, &coordinator, now);
        state.publish_hold(member.session_id, None, false, now);
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(100))
            .is_err());
        state.publish_hold(
            member.session_id,
            Some(now + Duration::from_secs(40)),
            true,
            now,
        );
        state.publish_held_key(member.session_id, "held", true, now);
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(100))
            .is_err());
        state.publish_held_key(
            member.session_id,
            "held",
            false,
            now + Duration::from_secs(50),
        );
        state.observe_detector(member.session_id, true, now + Duration::from_secs(51));
        state.publish_manager_facts(member.clone(), true, false, now + Duration::from_secs(52));
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(100))
            .is_err());
        state.observe_detector(member.session_id, false, now + Duration::from_secs(60));
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(89))
            .is_err());
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(90))
            .is_ok());
    }

    #[test]
    fn response_close_setting_epoch_changes_revoke_tickets_and_old_return_success() {
        let now = Instant::now() + Duration::from_secs(3600);
        let (state, member, coordinator) = routes(now);
        arm(&state, &member, &coordinator, now);
        let old = state
            .select_ticket(member.session_id, now + Duration::from_secs(30))
            .unwrap();
        let late = state
            .admit_return(
                &member,
                &coordinator,
                "late".into(),
                "late".into(),
                now + Duration::from_secs(31),
            )
            .unwrap();
        state.apply_settings(true, 10, now + Duration::from_secs(32));
        state
            .observe_return_success(
                ReturnDeliveryProof { context: late },
                now + Duration::from_secs(33),
            )
            .unwrap();
        assert!(state
            .try_commit_response_close(&old, now + Duration::from_secs(100))
            .is_err());
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(42))
            .is_err());
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(43))
            .is_ok());
        state.apply_settings(true, 0, now + Duration::from_secs(44));
        assert!(state
            .select_ticket(member.session_id, now + Duration::from_secs(100))
            .is_err());
    }
}
