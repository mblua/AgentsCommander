use crate::pty::backend_kind::SessionBackendKind;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use uuid::Uuid;

mod admission;
mod diagnostics;
mod returns;
mod routes;

const DIAG_TARGET: &str = "agentscommander_lib::response_close_diag";

// Runtime-only authority. No persisted lease, filesystem, settings or transport callouts.
#[derive(Clone, Default)]
pub(crate) struct ResponseCloseState {
    pub(in crate::pty) inner: Arc<Mutex<ResponseCloseInner>>,
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
pub(super) struct ResponseCloseInner {
    pub(in crate::pty) diagnostic_poll_modulo: u8,
    pub(in crate::pty) diagnostic_poll_initialized: bool,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) ready: bool,
    pub(in crate::pty) publishers: u8,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) work_bootstrapped: bool,
    pub(in crate::pty) enabled: bool,
    pub(in crate::pty) seconds: u32,
    pub(in crate::pty) settings_epoch: u64,
    pub(in crate::pty) failed: bool,
    pub(in crate::pty) routes: HashMap<Uuid, ResponseCloseRoute>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) work: HashMap<String, ResponseCloseWork>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) create_owners: HashMap<String, HashMap<Uuid, bool>>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) next_return_sequence: u64,
    pub(in crate::pty) unknown_spawns: usize,
    proof_epoch: u64,
    admissions: HashMap<AdmissionKey, ObservedAdmission>,
    terminal_receipts: HashMap<ResponseTerminalKey, ResponseTerminalReceipt>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) terminal_returns:
        HashMap<(ResponseRouteIdentity, ResponseRouteIdentity, String), String>,
}

#[derive(Default)]
pub(super) struct ResponseCloseWork {
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) provisional: usize,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) dispatch: usize,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) pending: HashMap<String, String>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) terminal: HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ResponseRouteIdentity {
    pub target: String,
    pub session_id: Uuid,
    pub token: Uuid,
    pub generation: u64,
    pub carrier_version: u64,
    pub backend: SessionBackendKind,
}

#[derive(Clone)]
pub(super) struct ResponseClosePolicy {
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) nonce: Uuid,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) member: ResponseRouteIdentity,
    pub(in crate::pty) coordinator: ResponseRouteIdentity,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) work_epoch: u64,
    pub(in crate::pty) settings_epoch: u64,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) request_key: String,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) request_digest: String,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) opened_at: std::time::Instant,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) submitted: bool,
    pub(in crate::pty) registered_at: Option<std::time::Instant>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) success_at: Option<std::time::Instant>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) returns: HashMap<String, (String, u64)>,
    pub(in crate::pty) selected: Option<Uuid>,
}

pub(super) struct ResponseCloseRoute {
    pub(in crate::pty) diagnostic_select: Option<(u64, u64, u8)>,
    pub(in crate::pty) generation: u64,
    pub(in crate::pty) facts: Option<ResponseRouteIdentity>,
    pub(in crate::pty) eligible: bool,
    pub(in crate::pty) manager_token: Option<Uuid>,
    pub(in crate::pty) manager_version: u64,
    pub(in crate::pty) manager_working: Option<bool>,
    pub(in crate::pty) detector_working: Option<bool>,
    pub(in crate::pty) menu_blocked: Option<bool>,
    pub(in crate::pty) hold_until: Option<std::time::Instant>,
    pub(in crate::pty) hold_known: bool,
    pub(in crate::pty) hold_window: Option<std::time::Duration>,
    pub(in crate::pty) held: std::collections::HashSet<String>,
    pub(in crate::pty) writers: usize,
    pub(in crate::pty) work_epoch: u64,
    pub(in crate::pty) activity_version: u64,
    pub(in crate::pty) last_change: std::time::Instant,
    pub(in crate::pty) policy: Option<ResponseClosePolicy>,
    pub(in crate::pty) closing: Option<Uuid>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) closing_target: Option<String>,
    // Dormant until P05 integration; remove with real callers.
    #[allow(dead_code)]
    pub(in crate::pty) committed_binding: Option<ResponseCloseCommittedBinding>,
}

impl ResponseCloseRoute {
    pub(in crate::pty) fn diagnostic_policy_loss(&self, outcome: &'static str, site: u32) {
        if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
            if let Some(policy) = &self.policy {
                log::debug!(
                    target: DIAG_TARGET,
                    "response_close_diag id=unknown stage=policy_loss outcome={} site={} \
                     member={} coordinator={} route_generation={} tracker_mirror_version={} \
                     external_after_version=unknown work_epoch={} activity_version={}",
                    outcome,
                    site,
                    policy.member.session_id,
                    policy.coordinator.session_id,
                    self.generation,
                    self.manager_version,
                    self.work_epoch,
                    self.activity_version
                );
            }
        }
    }

    #[track_caller]
    pub(in crate::pty) fn touch(&mut self, now: std::time::Instant) -> bool {
        self.last_change = self.last_change.max(now);
        if let Some(policy) = &mut self.policy {
            policy.selected = None;
        }
        if let Some(next) = self.activity_version.checked_add(1) {
            self.activity_version = next;
            true
        } else {
            if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
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
    pub(in crate::pty) fn invalidate(&mut self, now: std::time::Instant) -> bool {
        let diagnostic_before = if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
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
        if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
            self.diagnostic_policy_loss("invalidate", std::panic::Location::caller().line());
        }
        self.policy = None;
        if let Some(next) = self.work_epoch.checked_add(1) {
            self.work_epoch = next;
            if let Some((member, coordinator, before)) = diagnostic_before {
                log::debug!(
                    target: DIAG_TARGET,
                    "response_close_diag id=unknown outcome=observed stage=invalidate member={} \
                     coordinator={} before_work_epoch={} after_work_epoch={} \
                     external_after_version=unknown site={}",
                    member,
                    coordinator,
                    before,
                    self.work_epoch,
                    std::panic::Location::caller().line()
                );
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
    pub(in crate::pty) state: ResponseCloseState,
    pub(in crate::pty) target: String,
    pub(in crate::pty) key: String,
    pub(in crate::pty) active: bool,
}

// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct WorkDeliveryReservation {
    receipt: Option<AdmissionKey>,
    pub(in crate::pty) state: ResponseCloseState,
    pub(in crate::pty) target: String,
}

pub(crate) struct WriterAdmission {
    pub(in crate::pty) counted: bool,
    pub(in crate::pty) state: ResponseCloseState,
    pub(in crate::pty) session_id: Uuid,
    pub(in crate::pty) generation: u64,
}

// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct CreateOwnerReservation {
    receipt: Option<AdmissionKey>,
    pub(in crate::pty) state: ResponseCloseState,
    pub(in crate::pty) target: String,
    pub(in crate::pty) nonce: Uuid,
    pub(in crate::pty) classification: CreateClassification,
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
    pub(in crate::pty) session_id: Uuid,
    pub(in crate::pty) nonce: Uuid,
}

// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct ResponseDeliveryProof {
    pub context: ResponseDeliveryContext,
    pub member: ResponseRouteIdentity,
    pub coordinator: ResponseRouteIdentity,
    pub published_carrier_version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
// Dormant until P05 integration; remove with real callers.
#[allow(dead_code)]
pub(crate) struct ReturnObservationContext {
    pub(in crate::pty) settings_epoch: u64,
    pub(in crate::pty) sequence: u64,
    pub(in crate::pty) session_id: Uuid,
    pub(in crate::pty) nonce: Uuid,
    pub(in crate::pty) key: String,
    pub(in crate::pty) digest: String,
    pub(in crate::pty) member: ResponseRouteIdentity,
    pub(in crate::pty) coordinator: ResponseRouteIdentity,
    pub(in crate::pty) work_epoch: u64,
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
    pub(in crate::pty) session_id: Uuid,
    pub(in crate::pty) generation: u64,
    pub(in crate::pty) nonce: Uuid,
    pub(in crate::pty) selection_nonce: Uuid,
    pub(in crate::pty) work_epoch: u64,
    pub(in crate::pty) activity_version: u64,
    pub(in crate::pty) carrier_version: u64,
    pub(in crate::pty) settings_epoch: u64,
    pub(in crate::pty) member: ResponseRouteIdentity,
}

/// Incarnation is the exact UUID/token/generation tuple; backend kind and the
/// checked generation identify the frozen backend route. Version stays at commit.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ResponseCloseCommittedBinding {
    pub member: ResponseRouteIdentity,
    pub close_nonce: Uuid,
}

impl ResponseCloseTicket {
    pub(in crate::pty) fn route_identity(&self) -> (Uuid, u64, SessionBackendKind) {
        (self.session_id, self.generation, self.member.backend)
    }

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
    pub(in crate::pty) fn target_is_closing(&self, target: &str) -> bool {
        self.routes
            .values()
            .any(|route| route.closing.is_some() && route.closing_target.as_deref() == Some(target))
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
        if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
            log::debug!(
                target: DIAG_TARGET,
                "response_close_diag outcome=observed stage=failed_transition \
                 operation=fail_closed site={} old_failed=false id=unknown",
                line!()
            );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=publish_manager_row site={} old_failed=false id={}",
                            line!(),
                            id
                        );
                    }
                    inner.failed = true;
                }
                route.facts = None;
            } else if route.manager_working != Some(working) && !route.touch(now) {
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag outcome=observed stage=failed_transition \
                         operation=publish_manager_row site={} old_failed=false id={}",
                        line!(),
                        id
                    );
                }
                inner.failed = true;
            }
            route.manager_token = Some(token);
            route.manager_version = version;
            route.manager_working = Some(working);
            route.eligible = eligible;
        });
    }
    pub(in crate::pty) fn uuid_closing(&self, id: Uuid) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .routes
            .get(&id)
            .is_some_and(|route| route.closing.is_some())
    }
    pub(in crate::pty) fn begin_unresolved_spawn(&self) {
        let _ = self.with_inner(|inner| {
            let Some(next) = inner.unknown_spawns.checked_add(1) else {
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag outcome=observed stage=failed_transition \
                         operation=begin_unresolved_spawn site={} old_failed=false id=unknown",
                        line!()
                    );
                }
                inner.failed = true;
                return;
            };
            inner.unknown_spawns = next;
            for route in inner.routes.values_mut() {
                if !route.touch(std::time::Instant::now()) {
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=begin_unresolved_spawn site={} old_failed=false id=unknown",
                            line!()
                        );
                    }
                    inner.failed = true;
                }
            }
        });
    }
    pub(in crate::pty) fn end_unresolved_spawn(&self) {
        let _ = self.with_inner(|inner| {
            let Some(next) = inner.unknown_spawns.checked_sub(1) else {
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag outcome=observed stage=failed_transition \
                         operation=end_unresolved_spawn site={} old_failed=false id=unknown",
                        line!()
                    );
                }
                inner.failed = true;
                return;
            };
            inner.unknown_spawns = next;
            for route in inner.routes.values_mut() {
                if !route.touch(std::time::Instant::now()) {
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=end_unresolved_spawn site={} old_failed=false id=unknown",
                            line!()
                        );
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
    pub(in crate::pty) fn with_inner<R>(
        &self,
        f: impl FnOnce(&mut ResponseCloseInner) -> R,
    ) -> Result<R, ResponseCloseError> {
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(poison) => {
                let mut inner = poison.into_inner();
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag outcome=observed stage=failed_transition \
                         operation=with_inner site={} old_failed=false id=unknown",
                        line!()
                    );
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
}

// Owned APIs stay additive until the upper settlement path is activated.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AdmissionKind {
    CreateOwner,
    Delivery,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct AdmissionKey {
    target: String,
    operation: Uuid,
    kind: AdmissionKind,
    nonce: Uuid,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TrackerProofError {
    Unknown,
    Conflict,
    ClosingDeferred,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackerReleaseReceipt {
    pub key: AdmissionKey,
    pub epoch: u64,
    pub generation: Vec<(Uuid, u64)>,
    pub routes: Vec<ResponseRouteIdentity>,
    pub result: TrackerReleaseResult,
}
struct ObservedAdmission {
    generation: Vec<(Uuid, u64)>,
    routes: Vec<ResponseRouteIdentity>,
    release: Option<Result<TrackerReleaseReceipt, TrackerProofError>>,
    debited: bool,
    terminal_started: bool,
}
#[derive(Clone)]
pub(crate) struct TrackerReceiptHandle {
    state: ResponseCloseState,
    key: AdmissionKey,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ResponseTerminalKey {
    pub target: String,
    pub source_key: String,
    pub digest: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResponseTerminalReceipt {
    pub key: ResponseTerminalKey,
    admission: AdmissionKey,
    pub epoch: u64,
    pub already_terminal: bool,
    return_context: Option<ReturnObservationContext>,
    pub result: TrackerTerminalResult,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TrackerPublicationReceipt {
    pub row: Uuid,
    pub token: Option<Uuid>,
    pub generation: Option<u64>,
    pub version: u64,
    pub epoch: u64,
    pub invalidated: bool,
    pub expectation: TrackerRowExpectation,
    pub kind: TrackerPublicationKind,
    pub mirror_before: Option<TrackerMirror>,
    pub mirror_after: Option<TrackerMirror>,
}
impl ResponseCloseInner {
    fn next_proof_epoch(&mut self) -> Result<u64, TrackerProofError> {
        match self.proof_epoch.checked_add(1) {
            Some(next) => {
                self.proof_epoch = next;
                Ok(next)
            }
            None => {
                self.failed = true;
                Err(TrackerProofError::Unknown)
            }
        }
    }
    fn proof_conflict<T>(&mut self) -> Result<T, TrackerProofError> {
        self.failed = true;
        Err(TrackerProofError::Conflict)
    }
    fn target_generations(&self, target: &str) -> Vec<(Uuid, u64)> {
        let mut rows: Vec<_> = self
            .routes
            .iter()
            .filter_map(|(id, route)| {
                route
                    .facts
                    .as_ref()
                    .filter(|f| f.target == target)
                    .map(|_| (*id, route.generation))
            })
            .collect();
        rows.sort_unstable();
        rows
    }
    fn touch_target(&mut self, target: &str) -> Result<(), TrackerProofError> {
        for route in self
            .routes
            .values_mut()
            .filter(|r| r.facts.as_ref().is_some_and(|f| f.target == target))
        {
            if !route.touch(std::time::Instant::now()) {
                self.failed = true;
                return Err(TrackerProofError::Unknown);
            }
        }
        Ok(())
    }
    fn register_admission(&mut self, key: &AdmissionKey) -> Result<(), TrackerProofError> {
        if self.admissions.contains_key(key) {
            return self.proof_conflict();
        }
        let generation = self.target_generations(&key.target);
        let routes = self.routes.values().filter_map(|r| r.facts.clone())
            .filter(|r| r.target == key.target).collect();
        self.admissions.insert(
            key.clone(),
            ObservedAdmission {
                generation,
                routes,
                release: None,
                debited: false,
                terminal_started: false,
            },
        );
        self.next_proof_epoch()?;
        Ok(())
    }
}
impl ResponseCloseState {
    fn with_proof<T>(
        &self,
        f: impl FnOnce(&mut ResponseCloseInner) -> Result<T, TrackerProofError>,
    ) -> Result<T, TrackerProofError> {
        self.with_inner(f).map_err(|_| TrackerProofError::Unknown)?
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackerAdmissionFact {
    NotTracked,
    Bound { wiring_identity: usize, rows: [RowAdmission; 2] },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackerMirror {
    Unpublished,
    Published { token: Uuid, version: u64 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowEnrollment {
    Unenrolled,
    Enrolled { generation: u64, mirror: TrackerMirror },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RowAdmission {
    pub row: Uuid,
    pub enrollment: RowEnrollment,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackerVersionRule {
    Pinned,
    SameTokenAdvance,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackerPublicationKind {
    Invalidated,
    Published,
    Removed,
    Unenrolled,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TrackerRowExpectation {
    pub wiring_identity: usize,
    pub original: RowAdmission,
    pub version_rule: TrackerVersionRule,
    pub manager_before: Option<(Uuid, u64)>,
    pub manager_final: Option<(Uuid, u64)>,
    pub tombstone: Option<u64>,
    pub kind: TrackerPublicationKind,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TrackerReleaseResult {
    Released,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TrackerTerminalResult {
    Ordinary,
    ResponseReturn,
}

impl ResponseCloseInner {
    fn admission_routes_match(&self, key: &AdmissionKey) -> bool {
        self.admissions.get(key).is_some_and(|a| {
            a.generation == self.target_generations(&key.target)
                && a.routes.iter().all(|original| {
                    self.routes.get(&original.session_id).is_some_and(|r| {
                        r.generation == original.generation && r.facts.as_ref() == Some(original)
                    })
                })
        })
    }
}

impl TrackerRowExpectation {
    pub(crate) fn new(
        wiring_identity: usize, original: RowAdmission, version_rule: TrackerVersionRule,
        manager_before: Option<(Uuid, u64)>,
    ) -> Self {
        Self {
            wiring_identity, original, version_rule, manager_before,
            manager_final: None, tombstone: None, kind: TrackerPublicationKind::Invalidated,
        }
    }
    pub(crate) fn stage(
        mut self, kind: TrackerPublicationKind,
        manager_final: Option<(Uuid, u64)>, tombstone: Option<u64>,
    ) -> Self {
        self.kind = kind;
        self.manager_final = manager_final;
        self.tombstone = tombstone;
        self
    }
}
