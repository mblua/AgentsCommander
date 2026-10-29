use super::ReturnObservationContext;
use super::{ResponseTerminalKey, ResponseTerminalReceipt, TrackerReceiptHandle};
use super::{TrackerAdmissionFact, TrackerProofError, TrackerPublicationReceipt};
use crate::pty::response_close::ResponseCloseError;
use crate::pty::response_close::ResponseCloseRoute;
use crate::pty::response_close::ResponseCloseState;
use crate::pty::response_close::ResponseRouteIdentity;
use crate::pty::response_close::WriterAdmission;
use crate::pty::response_close::DIAG_TARGET;
use uuid::Uuid;

impl ResponseCloseState {
    pub(crate) fn apply_settings(&self, enabled: bool, seconds: u32, now: std::time::Instant) {
        let _ = self.with_inner(|inner| {
            if inner.enabled == enabled && inner.seconds == seconds {
                return;
            }
            let Some(epoch) = inner.settings_epoch.checked_add(1) else {
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag outcome=observed stage=failed_transition \
                         operation=apply_settings site={} old_failed=false id=unknown",
                        line!()
                    );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=apply_settings site={} old_failed=false id=unknown",
                            line!()
                        );
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
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag outcome=observed stage=failed_transition \
                         operation=register_route site={} old_failed=false id={}",
                        line!(),
                        id
                    );
                }
                inner.failed = true;
                return;
            }
            if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
                if let Some(route) = inner.routes.get(&id) {
                    route.diagnostic_policy_loss("route_replaced", line!());
                }
            }
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=publish_manager_facts site={} old_failed=false id={}",
                            line!(),
                            changed_id
                        );
                    }
                    inner.failed = true;
                }
            } else if route.manager_working != Some(working) && !route.touch(now) {
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag outcome=observed stage=failed_transition \
                         operation=publish_manager_facts site={} old_failed=false id={}",
                        line!(),
                        changed_id
                    );
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
                        if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug)
                            && !inner.failed
                        {
                            log::debug!(
                                target: DIAG_TARGET,
                                "response_close_diag outcome=observed stage=failed_transition \
                                 operation=publish_manager_facts site={} old_failed=false id={}",
                                line!(),
                                changed_id
                            );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=invalidate_manager_row site={} old_failed=false id={}",
                            line!(),
                            id
                        );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=invalidate_manager_row site={} old_failed=false id={}",
                            line!(),
                            id
                        );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=observe_raw_output site={} old_failed=false id={}",
                            line!(),
                            id
                        );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=observe_detector site={} old_failed=false id={}",
                            line!(),
                            id
                        );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=publish_menu site={} old_failed=false id={}",
                            line!(),
                            id
                        );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=publish_hold site={} old_failed=false id={}",
                            line!(),
                            id
                        );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=publish_held_key site={} old_failed=false id={}",
                            line!(),
                            id
                        );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=publish_typing_key site={} old_failed=false id={}",
                            line!(),
                            id
                        );
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
                            if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug)
                                && !inner.failed
                            {
                                log::debug!(
                                    target: DIAG_TARGET,
                                    "response_close_diag outcome=observed stage=failed_transition \
                                     operation=refresh_hold_snapshot site={} old_failed=false \
                                     id={}",
                                    line!(),
                                    id
                                );
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
                        if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug)
                            && !inner.failed
                        {
                            log::debug!(
                                target: DIAG_TARGET,
                                "response_close_diag outcome=observed stage=failed_transition \
                                 operation=refresh_hold_snapshot site={} old_failed=false id={}",
                                line!(),
                                id
                            );
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
}

impl ResponseCloseRoute {
    fn mirror(&self) -> Result<super::TrackerMirror, TrackerProofError> {
        use super::TrackerMirror;
        match (self.manager_token, self.manager_version) {
            (None, 0) => Ok(TrackerMirror::Unpublished),
            (Some(token), version) => Ok(TrackerMirror::Published { token, version }),
            _ => Err(TrackerProofError::Conflict),
        }
    }
}
impl super::TrackerRowExpectation {
    fn compatible(&self, current: Option<super::TrackerMirror>) -> bool {
        use super::{RowEnrollment, TrackerMirror, TrackerVersionRule};
        match (self.original.enrollment, current, self.manager_before) {
            (RowEnrollment::Unenrolled, None, _) => true,
            (RowEnrollment::Enrolled { mirror, .. }, Some(current), Some((token, version))) => {
                let expected = TrackerMirror::Published { token, version };
                match mirror {
                    TrackerMirror::Unpublished => {
                        current == TrackerMirror::Unpublished || current == expected
                    }
                    TrackerMirror::Published { token: captured, version: old } => {
                        captured == token && current == expected
                            && match self.version_rule {
                                TrackerVersionRule::Pinned => old == version,
                                TrackerVersionRule::SameTokenAdvance => old <= version,
                            }
                    }
                }
            }
            _ => false,
        }
    }
    fn validate(
        &self,
        inner: &super::ResponseCloseInner,
        wiring: usize,
    ) -> Result<Option<super::TrackerMirror>, TrackerProofError> {
        use super::RowEnrollment;
        if self.wiring_identity != wiring {
            return Err(TrackerProofError::Conflict);
        }
        let current = match (self.original.enrollment, inner.routes.get(&self.original.row)) {
            (RowEnrollment::Unenrolled, None) => None,
            (RowEnrollment::Enrolled { generation, .. }, Some(route))
                if route.generation == generation => Some(route.mirror()?),
            _ => return Err(TrackerProofError::Conflict),
        };
        if !self.compatible(current) {
            return Err(TrackerProofError::Conflict);
        }
        Ok(current)
    }
}
impl TrackerPublicationReceipt {
    pub(crate) fn covers(&self, expected: &super::TrackerRowExpectation) -> bool {
        use super::{RowEnrollment, TrackerMirror, TrackerPublicationKind};
        let generation = match expected.original.enrollment {
            RowEnrollment::Unenrolled => None,
            RowEnrollment::Enrolled { generation, .. } => Some(generation),
        };
        let final_mirror = match expected.kind {
            TrackerPublicationKind::Invalidated => self.mirror_before,
            TrackerPublicationKind::Unenrolled => None,
            _ => expected.manager_final.or_else(|| {
                expected.manager_before.zip(expected.tombstone).map(|((t, _), v)| (t, v))
            }).map(|(token, version)| TrackerMirror::Published { token, version }),
        };
        self.expectation == *expected && self.row == expected.original.row
            && self.generation == generation && self.kind == expected.kind && self.epoch > 0
            && expected.compatible(self.mirror_before) && self.mirror_after == final_mirror
    }
}
impl ResponseCloseState {
    pub(crate) fn capture_tracking_admission(
        &self,
        rows: [Uuid; 2],
    ) -> Result<TrackerAdmissionFact, TrackerProofError> {
        self.with_proof(|inner| {
            if inner.publishers & 1 == 0 {
                return Ok(TrackerAdmissionFact::NotTracked);
            }
            let capture = |row| -> Result<super::RowAdmission, TrackerProofError> {
                let enrollment = match inner.routes.get(&row) {
                    None => super::RowEnrollment::Unenrolled,
                    Some(route) => super::RowEnrollment::Enrolled {
                        generation: route.generation, mirror: route.mirror()?,
                    },
                };
                Ok(super::RowAdmission { row, enrollment })
            };
            Ok(TrackerAdmissionFact::Bound {
                wiring_identity: self.wiring_identity(),
                rows: [capture(rows[0])?, capture(rows[1])?],
            })
        })
    }
    pub(crate) fn validate_tracking_rows(
        &self,
        rows: &[super::TrackerRowExpectation],
    ) -> Result<(), TrackerProofError> {
        self.with_proof(|inner| {
            for row in rows {
                row.validate(inner, self.wiring_identity())?;
            }
            Ok(())
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish_manager_row_dispatch(
        &self, id: Uuid, token: Uuid, generation: u64, version: u64,
        eligible: bool, working: bool, now: std::time::Instant,
        observed: Option<&super::TrackerRowExpectation>,
    ) -> Option<Result<TrackerPublicationReceipt, TrackerProofError>> {
        match observed {
            Some(expected) => Some(self.publish_manager_row_observed(
                id, token, generation, version, eligible, working, now, expected,
            )),
            None => {
                self.publish_manager_row(id, token, generation, version, eligible, working, now);
                None
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn publish_manager_row_observed(
        &self, id: Uuid, token: Uuid, generation: u64, version: u64,
        eligible: bool, working: bool, now: std::time::Instant,
        expected: &super::TrackerRowExpectation,
    ) -> Result<TrackerPublicationReceipt, TrackerProofError> {
        self.with_proof(|inner| {
            let before = expected.validate(inner, self.wiring_identity())?;
            if expected.original.row != id
                || expected.kind != super::TrackerPublicationKind::Published
                || expected.manager_final != Some((token, version)) || expected.tombstone.is_some()
            {
                return Err(TrackerProofError::Conflict);
            }
            let Some(route) = inner.routes.get_mut(&id).filter(|r| r.generation == generation) else {
                return Err(TrackerProofError::Conflict);
            };
            let changed = route.manager_token != Some(token)
                || route.manager_version != version || route.eligible != eligible;
            let healthy = if changed {
                route.facts = None;
                route.invalidate(now)
            } else {
                route.manager_working == Some(working) || route.touch(now)
            };
            route.manager_token = Some(token);
            route.manager_version = version;
            route.manager_working = Some(working);
            route.eligible = eligible;
            if !healthy {
                inner.failed = true;
                return Err(TrackerProofError::Unknown);
            }
            let after = route.mirror()?;
            Self::row_receipt(inner, expected, before, Some(after))
        })
    }
    pub(crate) fn invalidate_manager_row_observed(
        &self, expected: &super::TrackerRowExpectation, now: std::time::Instant,
    ) -> Result<TrackerPublicationReceipt, TrackerProofError> {
        self.with_proof(|inner| {
            let before = expected.validate(inner, self.wiring_identity())?;
            if before.is_none() || expected.kind != super::TrackerPublicationKind::Invalidated {
                return Err(TrackerProofError::Conflict);
            }
            let id = expected.original.row;
            for (row, route) in &mut inner.routes {
                if *row == id {
                    route.facts = None;
                    route.eligible = false;
                }
                if (*row == id || route.policy.as_ref().is_some_and(|p| p.coordinator.session_id == id))
                    && !route.invalidate(now)
                {
                    inner.failed = true;
                    return Err(TrackerProofError::Unknown);
                }
            }
            Self::row_receipt(inner, expected, before, before)
        })
    }
    pub(crate) fn observe_manager_absence(
        &self, expected: &super::TrackerRowExpectation,
    ) -> Result<TrackerPublicationReceipt, TrackerProofError> {
        self.with_proof(|inner| {
            let before = expected.validate(inner, self.wiring_identity())?;
            let after = match expected.kind {
                super::TrackerPublicationKind::Unenrolled if before.is_none() => None,
                super::TrackerPublicationKind::Removed if expected.manager_final.is_none() => {
                    let token = expected.manager_before.ok_or(TrackerProofError::Conflict)?.0;
                    let version = expected.tombstone.ok_or(TrackerProofError::Conflict)?;
                    let route = inner.routes.get_mut(&expected.original.row)
                        .ok_or(TrackerProofError::Conflict)?;
                    route.manager_token = Some(token);
                    route.manager_version = version;
                    route.eligible = false;
                    route.facts = None;
                    Some(super::TrackerMirror::Published { token, version })
                }
                _ => return Err(TrackerProofError::Conflict),
            };
            Self::row_receipt(inner, expected, before, after)
        })
    }
    fn row_receipt(
        inner: &mut super::ResponseCloseInner, expected: &super::TrackerRowExpectation,
        before: Option<super::TrackerMirror>, after: Option<super::TrackerMirror>,
    ) -> Result<TrackerPublicationReceipt, TrackerProofError> {
        let generation = match expected.original.enrollment {
            super::RowEnrollment::Unenrolled => None,
            super::RowEnrollment::Enrolled { generation, .. } => Some(generation),
        };
        let (token, version) = match after {
            Some(super::TrackerMirror::Published { token, version }) => (Some(token), version),
            _ => (None, 0),
        };
        Ok(TrackerPublicationReceipt {
            row: expected.original.row, token, generation, version,
            epoch: inner.next_proof_epoch()?, invalidated:
                expected.kind == super::TrackerPublicationKind::Invalidated,
            expectation: *expected, kind: expected.kind, mirror_before: before, mirror_after: after,
        })
    }
}
impl TrackerReceiptHandle {
    #[allow(dead_code)]
    pub(crate) fn settle_return_terminal_observed(
        &self,
        context: &ReturnObservationContext,
    ) -> Result<ResponseTerminalReceipt, TrackerProofError> {
        self.state.with_proof(|inner| {
            let key = ResponseTerminalKey {
                target: context.member.target.clone(),
                source_key: context.key.clone(),
                digest: context.digest.clone(),
            };
            if key.target != self.key.target || !inner.admission_routes_match(&self.key)
                || !context.current_authority(inner) {
                return inner.proof_conflict();
            }
            if let Some(old) = inner.terminal_receipts.get(&key) {
                if old.admission != self.key || old.return_context.as_ref() != Some(context)
                    || old.result != super::TrackerTerminalResult::ResponseReturn {
                    return inner.proof_conflict();
                }
                let mut receipt = old.clone();
                receipt.already_terminal = true;
                return Ok(receipt);
            }
            if inner.admissions.get(&self.key).is_none_or(|a| a.terminal_started) {
                return inner.proof_conflict();
            }
            let terminal_key = (
                context.member.clone(),
                context.coordinator.clone(),
                context.key.clone(),
            );
            if inner.terminal_returns.contains_key(&terminal_key) {
                return inner.proof_conflict();
            }
            let Some(route) = inner.routes.get_mut(&context.session_id).filter(|r| {
                r.generation == context.member.generation && r.work_epoch == context.work_epoch
            }) else {
                return inner.proof_conflict();
            };
            let Some(policy) = route.policy.as_mut().filter(|p| {
                p.nonce == context.nonce
                    && p.member == context.member
                    && p.coordinator == context.coordinator
                    && p.work_epoch == context.work_epoch
            }) else {
                return inner.proof_conflict();
            };
            if policy.returns.get(&context.key) != Some(&(context.digest.clone(), context.sequence))
            {
                return inner.proof_conflict();
            }
            inner.admissions.get_mut(&self.key).ok_or(TrackerProofError::Unknown)?
                .terminal_started = true;
            policy.returns.remove(&context.key);
            policy.selected = None;
            if !route.touch(std::time::Instant::now()) {
                inner.failed = true;
                return Err(TrackerProofError::Unknown);
            }
            inner
                .terminal_returns
                .insert(terminal_key, context.digest.clone());
            let receipt = ResponseTerminalReceipt {
                key: key.clone(),
                admission: self.key.clone(),
                epoch: inner.next_proof_epoch()?,
                already_terminal: false,
                return_context: Some(context.clone()),
                result: super::TrackerTerminalResult::ResponseReturn,
            };
            inner.terminal_receipts.insert(key, receipt.clone());
            Ok(receipt)
        })
    }
}

impl ReturnObservationContext {
    fn current_authority(&self, inner: &super::ResponseCloseInner) -> bool {
        self.session_id == self.member.session_id
            && [&self.member, &self.coordinator].into_iter().all(|identity| {
                inner.routes.get(&identity.session_id).is_some_and(|r| {
                    r.generation == identity.generation && r.facts.as_ref() == Some(identity)
                })
            })
            && inner.routes.get(&self.session_id).is_some_and(|r| {
                r.work_epoch == self.work_epoch && r.policy.as_ref().is_some_and(|p| {
                    p.nonce == self.nonce && p.member == self.member
                        && p.coordinator == self.coordinator && p.work_epoch == self.work_epoch
                        && p.settings_epoch == self.settings_epoch
                })
            })
    }
}
