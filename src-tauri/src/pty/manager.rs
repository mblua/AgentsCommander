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
    diagnostic_poll_modulo: u8,
    diagnostic_poll_initialized: bool,
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
    diagnostic_select: Option<(u64, u64, u8)>,
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
    fn diagnostic_policy_loss(&self, outcome: &'static str, site: u32) {
        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
        {
            if let Some(policy) = &self.policy {
                log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown stage=policy_loss outcome={} site={} member={} coordinator={} route_generation={} tracker_mirror_version={} external_after_version=unknown work_epoch={} activity_version={}", outcome, site, policy.member.session_id, policy.coordinator.session_id, self.generation, self.manager_version, self.work_epoch, self.activity_version);
            }
        }
    }

    #[track_caller]
    fn touch(&mut self, now: std::time::Instant) -> bool {
        self.last_change = self.last_change.max(now);
        if let Some(policy) = &mut self.policy {
            policy.selected = None;
        }
        if let Some(next) = self.activity_version.checked_add(1) {
            self.activity_version = next;
            true
        } else {
            if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
            {
                self.diagnostic_policy_loss(
                    "activity_overflow",
                    std::panic::Location::caller().line(),
                );
            }
            self.policy = None;
            false
        }
    }
    #[track_caller]
    fn invalidate(&mut self, now: std::time::Instant) -> bool {
        let diagnostic_before = if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
        {
            self.policy.as_ref().map(|policy| {
                (
                    policy.member.session_id,
                    policy.coordinator.session_id,
                    self.work_epoch,
                )
            })
        } else {
            None
        };
        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
        {
            self.diagnostic_policy_loss("invalidate", std::panic::Location::caller().line());
        }
        self.policy = None;
        if let Some(next) = self.work_epoch.checked_add(1) {
            self.work_epoch = next;
            if let Some((member, coordinator, before)) = diagnostic_before {
                log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown outcome=observed stage=invalidate member={} coordinator={} before_work_epoch={} after_work_epoch={} external_after_version=unknown site={}", member, coordinator, before, self.work_epoch, std::panic::Location::caller().line());
            }
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

impl ResponseCloseInner {
    fn target_is_closing(&self, target: &str) -> bool {
        self.routes
            .values()
            .any(|route| route.closing.is_some() && route.closing_target.as_deref() == Some(target))
    }
}

impl ResponseCloseState {
    /// Observation only: never recover poison or invoke the authority wrapper.
    pub(crate) fn diagnostic_poll_health(&self) -> bool {
        if !log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
        {
            return false;
        }
        let mut inner = match self.inner.try_lock() {
            Ok(inner) => inner,
            Err(TryLockError::WouldBlock) => {
                log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=poll_health scope=pre_reconcile_pre_selection outcome=busy health=unknown", line!());
                return false;
            }
            Err(TryLockError::Poisoned(_)) => {
                log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=poll_health scope=pre_reconcile_pre_selection outcome=poisoned health=unknown", line!());
                return false;
            }
        };
        let emit = !inner.diagnostic_poll_initialized || inner.diagnostic_poll_modulo == 9;
        inner.diagnostic_poll_initialized = true;
        inner.diagnostic_poll_modulo = (inner.diagnostic_poll_modulo + 1) % 10;
        if emit {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} outcome=observed stage=poll_health scope=pre_reconcile_pre_selection ready={} publishers={} bootstrap={} failed={} tracker_effective_enabled={} tracker_effective_seconds={} settings_epoch={} unknown_spawns={}", line!(), inner.ready, inner.publishers, inner.work_bootstrapped, inner.failed, inner.enabled, inner.seconds, inner.settings_epoch, inner.unknown_spawns);
        }
        emit
    }

    fn diagnostic_select_observation(
        inner: &mut ResponseCloseInner,
        id: Uuid,
        now: std::time::Instant,
        actual_safe: bool,
    ) {
        let Some(route) = inner.routes.get(&id) else {
            return;
        };
        let policy = route.policy.as_ref();
        let registered = policy.and_then(|p| p.registered_at);
        let success = policy.and_then(|p| p.success_at);
        let work = policy.and_then(|p| {
            route
                .facts
                .as_ref()
                .map(|_| inner.work.get(&p.member.target))
        });
        let anchor = registered.zip(success).map(|(registered, success)| {
            registered
                .max(success)
                .max(route.last_change)
                .max(route.hold_until.unwrap_or(registered))
        });
        // One bit per safe predicate, including its three early-return requirements.
        let predicates = [
            Some(policy.is_some()),
            {
                let registered_present = registered.is_some();
                policy.is_some().then_some(registered_present)
            },
            {
                let success_present = success.is_some();
                policy.is_some().then_some(success_present)
            },
            Some(inner.unknown_spawns == 0),
            Some(inner.ready),
            Some(inner.work_bootstrapped),
            Some(inner.enabled),
            Some((1..=3600).contains(&inner.seconds)),
            Some(!inner.failed),
            Some(route.closing.is_none()),
            Some(route.eligible),
            policy.map(|p| route.facts.as_ref() == Some(&p.member)),
            policy.map(|p| {
                inner
                    .routes
                    .get(&p.coordinator.session_id)
                    .is_some_and(|r| r.facts.as_ref() == Some(&p.coordinator))
            }),
            route.manager_working.map(|working| !working),
            route.detector_working.map(|working| !working),
            route.menu_blocked.map(|blocked| !blocked),
            Some(route.hold_known),
            {
                let hold_elapsed = route.hold_until.is_none_or(|end| now > end);
                route.hold_known.then_some(hold_elapsed)
            },
            Some(route.held.is_empty()),
            Some(route.writers == 0),
            work.map(|w| {
                w.is_none_or(|w| w.provisional == 0 && w.dispatch == 0 && w.pending.is_empty())
            }),
            policy.and_then(|p| {
                route
                    .facts
                    .as_ref()
                    .map(|_| !inner.create_owners.contains_key(&p.member.target))
            }),
            policy.map(|p| p.returns.is_empty()),
            policy.map(|p| p.settings_epoch == inner.settings_epoch),
            policy.map(|p| p.work_epoch == route.work_epoch),
            anchor.map(|anchor| {
                now.saturating_duration_since(anchor)
                    >= std::time::Duration::from_secs(u64::from(inner.seconds))
            }),
        ];
        let mut false_mask = 0_u64;
        let mut unknown_mask = 0_u64;
        for (bit, predicate) in predicates.iter().enumerate() {
            match predicate {
                Some(false) => false_mask |= 1 << bit,
                None => unknown_mask |= 1 << bit,
                Some(true) => {}
            }
        }
        let old = route.diagnostic_select;
        let modulo = old.map_or(0, |(_, _, modulo)| modulo);
        let emit = old.is_none_or(|(old_false, old_unknown, _)| {
            old_false != false_mask || old_unknown != unknown_mask
        }) || modulo == 9;
        let next_cache = Some((false_mask, unknown_mask, (modulo + 1) % 10));
        if emit {
            let first_false = (false_mask != 0).then(|| false_mask.trailing_zeros());
            let first_unknown = (unknown_mask != 0).then(|| unknown_mask.trailing_zeros());
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} outcome=observed stage=select scope=post_settings_apply_post_member_publish_post_coordinator_publish_pre_ticket_decision member={} actual_safe={} false_mask={} unknown_mask={} first_false={:?} first_unknown={:?} diagnostic_inconsistent={} ready={} publishers={} bootstrap={} failed={} tracker_effective_enabled={} tracker_effective_seconds={} settings_epoch={} unknown_spawns={}", line!(), id, actual_safe, false_mask, unknown_mask, first_false, first_unknown, actual_safe != (false_mask == 0 && unknown_mask == 0), inner.ready, inner.publishers, inner.work_bootstrapped, inner.failed, inner.enabled, inner.seconds, inner.settings_epoch, inner.unknown_spawns);
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} outcome=observed stage=select_route member={} generation={} tracker_mirror_version={} eligible={} manager_working={:?} detector_working={:?} menu_blocked={:?} hold_known={} hold_remaining_ms={:?} held_count={} writers={} work_epoch={} activity_version={} closing={} external_live_facts=unknown policy_present={}", line!(), id, route.generation, route.manager_version, route.eligible, route.manager_working, route.detector_working, route.menu_blocked, route.hold_known, route.hold_until.map(|end| end.saturating_duration_since(now).as_millis()), route.held.len(), route.writers, route.work_epoch, route.activity_version, route.closing.is_some(), policy.is_some());
            Self::diagnostic_select_policy(inner, route, now, anchor);
        }
        if let Some(route) = inner.routes.get_mut(&id) {
            route.diagnostic_select = next_cache;
        }
    }

    fn diagnostic_select_policy(
        inner: &ResponseCloseInner,
        route: &ResponseCloseRoute,
        now: std::time::Instant,
        anchor: Option<std::time::Instant>,
    ) {
        let Some(policy) = &route.policy else {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=select_policy outcome=absent captured_member=unknown captured_coordinator=unknown target_counts=unknown ages=unknown facts_match=unknown epoch_match=unknown", line!());
            return;
        };
        let work = inner.work.get(&policy.member.target);
        let required = std::time::Duration::from_secs(u64::from(inner.seconds));
        let age = anchor.map(|anchor| now.saturating_duration_since(anchor));
        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} outcome=observed stage=select_policy captured_member={} captured_coordinator={} captured_member_generation={} captured_coordinator_generation={} captured_member_carrier_version={} captured_coordinator_carrier_version={} submitted={} registered={} success={} pending_returns={} selected={} member_facts_match={} coordinator_facts_match={} settings_epoch_match={} work_epoch_match={}", line!(), policy.member.session_id, policy.coordinator.session_id, policy.member.generation, policy.coordinator.generation, policy.member.carrier_version, policy.coordinator.carrier_version, policy.submitted, policy.registered_at.is_some(), policy.success_at.is_some(), policy.returns.len(), policy.selected.is_some(), route.facts.as_ref() == Some(&policy.member), inner.routes.get(&policy.coordinator.session_id).is_some_and(|r| r.facts.as_ref() == Some(&policy.coordinator)), policy.settings_epoch == inner.settings_epoch, policy.work_epoch == route.work_epoch);
        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} outcome=observed stage=select_work captured_member={} provisional={:?} dispatch={:?} pending={:?} create_owners={:?} registered_age_ms={:?} success_age_ms={:?} last_change_age_ms={} anchor_age_ms={:?} required_ms={} remaining_ms={:?}", line!(), policy.member.session_id, route.facts.as_ref().map(|_| work.map_or(0, |w| w.provisional)), route.facts.as_ref().map(|_| work.map_or(0, |w| w.dispatch)), route.facts.as_ref().map(|_| work.map_or(0, |w| w.pending.len())), route.facts.as_ref().map(|_| inner.create_owners.get(&policy.member.target).map_or(0, |owners| owners.len())), policy.registered_at.map(|at| now.saturating_duration_since(at).as_millis()), policy.success_at.map(|at| now.saturating_duration_since(at).as_millis()), now.saturating_duration_since(route.last_change).as_millis(), age.map(|age| age.as_millis()), required.as_millis(), age.map(|age| required.saturating_sub(age).as_millis()));
    }
}

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
        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
            && !inner.failed
        {
            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=fail_closed site={} old_failed=false id=unknown", line!());
        }
        inner.failed = true;
        for route in inner.routes.values_mut() {
            route.diagnostic_policy_loss("fail_closed", line!());
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=publish_manager_row site={} old_failed=false id={}", line!(), id);
                    }
                    inner.failed = true;
                }
                route.facts = None;
            } else if route.manager_working != Some(working) && !route.touch(now) {
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=publish_manager_row site={} old_failed=false id={}", line!(), id);
                }
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
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=begin_unresolved_spawn site={} old_failed=false id=unknown", line!());
                }
                inner.failed = true;
                return;
            };
            inner.unknown_spawns = next;
            for route in inner.routes.values_mut() {
                if !route.touch(std::time::Instant::now()) {
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=begin_unresolved_spawn site={} old_failed=false id=unknown", line!());
                    }
                    inner.failed = true;
                }
            }
        });
    }
    fn end_unresolved_spawn(&self) {
        let _ = self.with_inner(|inner| {
            let Some(next) = inner.unknown_spawns.checked_sub(1) else {
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=end_unresolved_spawn site={} old_failed=false id=unknown", line!());
                }
                inner.failed = true;
                return;
            };
            inner.unknown_spawns = next;
            for route in inner.routes.values_mut() {
                if !route.touch(std::time::Instant::now()) {
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=end_unresolved_spawn site={} old_failed=false id=unknown", line!());
                    }
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
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
                    && !inner.failed
                {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=with_inner site={} old_failed=false id=unknown", line!());
                }
                inner.failed = true;
                return Err(ResponseCloseError::Unavailable);
            }
        };
        if inner.failed {
            return Err(ResponseCloseError::Unavailable);
        }
        Ok(f(&mut inner))
    }
    pub(crate) fn is_healthy(&self) -> bool {
        self.with_inner(|_| ()).is_ok()
    }
    pub(crate) fn apply_settings(&self, enabled: bool, seconds: u32, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            if inner.enabled == enabled && inner.seconds == seconds {
                return;
            }
            let Some(epoch) = inner.settings_epoch.checked_add(1) else {
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=apply_settings site={} old_failed=false id=unknown", line!());
                }
                inner.failed = true;
                return;
            };
            inner.settings_epoch = epoch;
            inner.enabled = enabled && (1..=3600).contains(&seconds);
            inner.seconds = seconds;
            for route in inner.routes.values_mut() {
                if !inner.enabled {
                    route.diagnostic_policy_loss("apply_settings", line!());
                    route.policy = None;
                } else if let Some(policy) = &mut route.policy {
                    if policy.registered_at.is_some() {
                        policy.settings_epoch = epoch;
                    } else {
                        route.diagnostic_policy_loss("apply_settings", line!());
                        route.policy = None;
                    }
                }
                if !route.touch(now) {
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=apply_settings site={} old_failed=false id=unknown", line!());
                    }
                    inner.failed = true;
                }
            }
        });
    }
    pub(crate) fn register_route(&self, id: Uuid, generation: u64, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            // A route still Closing is never reopened by registration.
            if inner.routes.get(&id).is_some_and(|r| r.closing.is_some()) {
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=register_route site={} old_failed=false id={}", line!(), id);
                }
                inner.failed = true;
                return;
            }
            if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { if let Some(route) = inner.routes.get(&id) {
                route.diagnostic_policy_loss("route_replaced", line!());
            } }
            inner.routes.insert(
                id,
                ResponseCloseRoute {
                    diagnostic_select: None,
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
                route.diagnostic_policy_loss("remove_route", line!());
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=publish_manager_facts site={} old_failed=false id={}", line!(), changed_id);
                    }
                    inner.failed = true;
                }
            } else if route.manager_working != Some(working) && !route.touch(now) {
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=publish_manager_facts site={} old_failed=false id={}", line!(), changed_id);
                }
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
                        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=publish_manager_facts site={} old_failed=false id={}", line!(), changed_id);
                        }
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=invalidate_manager_row site={} old_failed=false id={}", line!(), id);
                    }
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=invalidate_manager_row site={} old_failed=false id={}", line!(), id);
                    }
                    inner.failed = true;
                }
            }
        });
    }
    pub(crate) fn observe_raw_output(&self, id: Uuid, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner.routes.get_mut(&id) {
                if !route.touch(now) {
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=observe_raw_output site={} old_failed=false id={}", line!(), id);
                    }
                    inner.failed = true;
                }
            }
        });
    }
    pub(crate) fn observe_detector(&self, id: Uuid, working: bool, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            if let Some(route) = inner.routes.get_mut(&id) {
                if route.detector_working != Some(working) && !route.touch(now) {
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=observe_detector site={} old_failed=false id={}", line!(), id);
                    }
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=publish_menu site={} old_failed=false id={}", line!(), id);
                    }
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=publish_hold site={} old_failed=false id={}", line!(), id);
                    }
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=publish_held_key site={} old_failed=false id={}", line!(), id);
                    }
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=publish_typing_key site={} old_failed=false id={}", line!(), id);
                    }
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
                            if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                                log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=refresh_hold_snapshot site={} old_failed=false id={}", line!(), id);
                            }
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
                        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=refresh_hold_snapshot site={} old_failed=false id={}", line!(), id);
                        }
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
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
                    && !inner.failed
                {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=reserve_writer_inner site={} old_failed=false id={}", line!(), id);
                }
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
            if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
                && !inner.failed
            {
                log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=reserve_writer_inner site={} old_failed=false id={}", line!(), id);
            }
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
            if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
                && !inner.failed
            {
                log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=reserve_writer_inner site={} old_failed=false id={}", line!(), id);
            }
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
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=reserve_work site={} old_failed=false id=unknown", line!());
                }
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=reserve_work site={} old_failed=false id=unknown", line!());
                    }
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=settle_terminal site={} old_failed=false id=unknown", line!());
                    }
                    inner.failed = true;
                }
            }
        });
    }
    pub(crate) fn target_is_closing(&self, target: &str) -> bool {
        let inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(poison) => {
                let mut inner = poison.into_inner();
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
                    && !inner.failed
                {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=target_is_closing site={} old_failed=false id=unknown", line!());
                }
                inner.failed = true;
                inner
            }
        };
        inner.target_is_closing(target)
    }

    pub(crate) fn begin_delivery(
        &self,
        target: &str,
    ) -> Result<WorkDeliveryReservation, ResponseCloseError> {
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(poison) => {
                let mut inner = poison.into_inner();
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
                    && !inner.failed
                {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=begin_delivery site={} old_failed=false id=unknown", line!());
                }
                inner.failed = true;
                inner
            }
        };
        if inner.target_is_closing(target) {
            return Err(ResponseCloseError::ClosingDeferred);
        }
        if inner.failed {
            return Err(ResponseCloseError::Unavailable);
        }
        let work = inner.work.entry(target.to_owned()).or_default();
        let Some(next) = work.dispatch.checked_add(1) else {
            if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
                && !inner.failed
            {
                log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=begin_delivery site={} old_failed=false id=unknown", line!());
            }
            inner.failed = true;
            return Err(ResponseCloseError::Unavailable);
        };
        work.dispatch = next;
        drop(inner);
        Ok(WorkDeliveryReservation {
            state: self.clone(),
            target: target.to_owned(),
        })
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=reserve_create_owner site={} old_failed=false id=unknown", line!());
                    }
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
            route.diagnostic_policy_loss("candidate_replaced", line!());
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
                        route.diagnostic_policy_loss("mark_request_submitted", line!());
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
                route.diagnostic_policy_loss("register_request", line!());
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=admit_return site={} old_failed=false id={}", line!(), member.session_id);
                    }
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
            let actual_safe = Self::safe(inner, route, now);
            if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) {
                Self::diagnostic_select_observation(inner, id, now, actual_safe);
            }
            if !actual_safe {
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
            let diagnostic_actual_safe = Self::safe(inner, route, now);
            if !diagnostic_actual_safe
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
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) {
                    log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=commit outcome=safe_or_identity_epoch_group_rejected member={} actual_safe={} identity_epoch_group={}", line!(), ticket.session_id, diagnostic_actual_safe, if diagnostic_actual_safe { "false" } else { "not_evaluated" });
                }
                return Err(ResponseCloseError::Unknown);
            }
            let route = inner.routes.get_mut(&ticket.session_id).unwrap();
            route.closing = Some(ticket.selection_nonce);
            route.closing_target = route
                .policy
                .as_ref()
                .map(|policy| policy.member.target.clone());
            route.committed_binding = Some(ticket.committed_binding());
            route.diagnostic_policy_loss("try_commit_response_close", line!());
            route.policy = None;
            if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=commit outcome=committed member={}", line!(), ticket.session_id); }
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
                .ok_or_else(|| {
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=finish outcome=route_missing member={}", line!(), ticket.session_id); }
                    StaleOrInvalidSettlement
                })?;
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
                if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=finish outcome=binding_or_current_settlement_group_rejected member={}", line!(), ticket.session_id); }
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
                        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=finish outcome=dormant_row_group_rejected member={}", line!(), ticket.session_id); }
                return Err(StaleOrInvalidSettlement);
                    }
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { if let Some(route) = inner.routes.get(&ticket.session_id) {
                        route.diagnostic_policy_loss("close_finished_route_removed", line!());
                    } }
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=finish outcome=dormant member={}", line!(), ticket.session_id); }
                    inner.routes.remove(&ticket.session_id);
                }
                ResponseCloseSettlementOutcome::ReopenAfterKillFailure {
                    terminal_kill_failure: true,
                    all_effects_terminal: true,
                    original_backend_alive: true,
                    original_manager_identity_intact: true,
                } => {
                    let route = inner.routes.get_mut(&ticket.session_id).unwrap();
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) { log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=finish outcome=reopened_after_kill_failure member={}", line!(), ticket.session_id); }
                    route.closing = None;
                    route.closing_target = None;
                    route.committed_binding = None;
                    route.diagnostic_policy_loss("finish_response_close", line!());
                    route.policy = None;
                }
                _ => {
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag id=unknown site={} stage=finish outcome=settlement_outcome_group_rejected member={}", line!(), ticket.session_id);
                    }
                    return Err(StaleOrInvalidSettlement);
                },
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=accept_durable site={} old_failed=false id=unknown", line!());
                    }
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
                        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=drop site={} old_failed=false id=unknown", line!());
                        }
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=drop site={} old_failed=false id=unknown", line!());
                    }
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
                    if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                        log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=drop site={} old_failed=false id=unknown", line!());
                    }
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
                        if log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug) && !inner.failed {
                            log::debug!(target: "agentscommander_lib::response_close_diag", "response_close_diag outcome=observed stage=failed_transition operation=drop site={} old_failed=false id=unknown", line!());
                        }
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
    _settings_guard: Option<crate::config::settings::SettingsReadPermit>,
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
        settings_guard: crate::config::settings::SettingsReadPermit,
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

    pub(crate) fn saved_cwd_matches_path(&self, path: &std::path::Path) -> bool {
        crate::path_identity::verify_directory(path)
            .is_ok_and(|actual| crate::path_identity::same_object(&actual, &self.saved_cwd))
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
mod tests;
