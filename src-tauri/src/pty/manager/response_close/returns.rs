use crate::pty::manager::response_close::ResponseCloseError;
use crate::pty::manager::response_close::ResponseCloseInner;
use crate::pty::manager::response_close::ResponseClosePolicy;
use crate::pty::manager::response_close::ResponseCloseRoute;
use crate::pty::manager::response_close::ResponseCloseSettlementEvidence;
use crate::pty::manager::response_close::ResponseCloseSettlementOutcome;
use crate::pty::manager::response_close::ResponseCloseState;
use crate::pty::manager::response_close::ResponseCloseTicket;
use crate::pty::manager::response_close::ResponseDeliveryContext;
use crate::pty::manager::response_close::ResponseDeliveryProof;
use crate::pty::manager::response_close::ResponseRouteIdentity;
use crate::pty::manager::response_close::ReturnDeliveryProof;
use crate::pty::manager::response_close::ReturnObservationContext;
use crate::pty::manager::response_close::StaleOrInvalidSettlement;
use crate::pty::manager::response_close::DIAG_TARGET;
use std::collections::HashMap;
use uuid::Uuid;

impl ResponseCloseState {
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=admit_return site={} old_failed=false id={}",
                            line!(),
                            member.session_id
                        );
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
    pub(in crate::pty::manager) fn settle_return(
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
    pub(in crate::pty::manager) fn safe(
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
            if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
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
    pub(in crate::pty::manager) fn try_commit_response_close(
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
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag id=unknown site={} stage=commit \
                         outcome=safe_or_identity_epoch_group_rejected member={} actual_safe={} \
                         identity_epoch_group={}",
                        line!(),
                        ticket.session_id,
                        diagnostic_actual_safe,
                        if diagnostic_actual_safe { "false" } else { "not_evaluated" }
                    );
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
            if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
                log::debug!(
                    target: DIAG_TARGET,
                    "response_close_diag id=unknown site={} stage=commit outcome=committed \
                     member={}",
                    line!(),
                    ticket.session_id
                );
            }
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
            let route = inner.routes.get(&ticket.session_id).ok_or_else(|| {
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag id=unknown site={} stage=finish \
                         outcome=route_missing member={}",
                        line!(),
                        ticket.session_id
                    );
                }
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
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag id=unknown site={} stage=finish \
                         outcome=binding_or_current_settlement_group_rejected member={}",
                        line!(),
                        ticket.session_id
                    );
                }
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
                        if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
                            log::debug!(
                                target: DIAG_TARGET,
                                "response_close_diag id=unknown site={} stage=finish \
                                 outcome=dormant_row_group_rejected member={}",
                                line!(),
                                ticket.session_id
                            );
                        }
                        return Err(StaleOrInvalidSettlement);
                    }
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
                        if let Some(route) = inner.routes.get(&ticket.session_id) {
                            route.diagnostic_policy_loss("close_finished_route_removed", line!());
                        }
                    }
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag id=unknown site={} stage=finish outcome=dormant \
                             member={}",
                            line!(),
                            ticket.session_id
                        );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag id=unknown site={} stage=finish \
                             outcome=reopened_after_kill_failure member={}",
                            line!(),
                            ticket.session_id
                        );
                    }
                    route.closing = None;
                    route.closing_target = None;
                    route.committed_binding = None;
                    route.diagnostic_policy_loss("finish_response_close", line!());
                    route.policy = None;
                }
                _ => {
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag id=unknown site={} stage=finish \
                             outcome=settlement_outcome_group_rejected member={}",
                            line!(),
                            ticket.session_id
                        );
                    }
                    return Err(StaleOrInvalidSettlement);
                }
            }
            Ok(())
        })
        .map_err(|_| StaleOrInvalidSettlement)?
    }
}
