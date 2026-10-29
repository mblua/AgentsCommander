use super::{AdmissionKey, AdmissionKind, TrackerProofError, TrackerReceiptHandle};
use super::{ResponseTerminalKey, ResponseTerminalReceipt, TrackerReleaseReceipt};
use crate::pty::response_close::CreateAdmission;
use crate::pty::response_close::CreateClassification;
use crate::pty::response_close::CreateOwnerReservation;
use crate::pty::response_close::ResponseCloseError;
use crate::pty::response_close::ResponseCloseState;
use crate::pty::response_close::WorkAdmission;
use crate::pty::response_close::WorkDeliveryReservation;
use crate::pty::response_close::WriterAdmission;
use crate::pty::response_close::DIAG_TARGET;
use uuid::Uuid;

impl ResponseCloseState {
    pub(in crate::pty) fn reserve_writer_inner(
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
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag outcome=observed stage=failed_transition \
                         operation=reserve_writer_inner site={} old_failed=false id={}",
                        line!(),
                        id
                    );
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
            if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                log::debug!(
                    target: DIAG_TARGET,
                    "response_close_diag outcome=observed stage=failed_transition \
                     operation=reserve_writer_inner site={} old_failed=false id={}",
                    line!(),
                    id
                );
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
            if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                log::debug!(
                    target: DIAG_TARGET,
                    "response_close_diag outcome=observed stage=failed_transition \
                     operation=reserve_writer_inner site={} old_failed=false id={}",
                    line!(),
                    id
                );
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
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag outcome=observed stage=failed_transition \
                         operation=reserve_work site={} old_failed=false id=unknown",
                        line!()
                    );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=reserve_work site={} old_failed=false id=unknown",
                            line!()
                        );
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=settle_terminal site={} old_failed=false id=unknown",
                            line!()
                        );
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
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag outcome=observed stage=failed_transition \
                         operation=target_is_closing site={} old_failed=false id=unknown",
                        line!()
                    );
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
        self.begin_delivery_inner(target, None)
    }
    fn begin_delivery_inner(
        &self,
        target: &str,
        operation: Option<Uuid>,
    ) -> Result<WorkDeliveryReservation, ResponseCloseError> {
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(poison) => {
                let mut inner = poison.into_inner();
                if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                    log::debug!(
                        target: DIAG_TARGET,
                        "response_close_diag outcome=observed stage=failed_transition \
                         operation=begin_delivery site={} old_failed=false id=unknown",
                        line!()
                    );
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
            if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                log::debug!(
                    target: DIAG_TARGET,
                    "response_close_diag outcome=observed stage=failed_transition \
                     operation=begin_delivery site={} old_failed=false id=unknown",
                    line!()
                );
            }
            inner.failed = true;
            return Err(ResponseCloseError::Unavailable);
        };
        work.dispatch = next;
        let receipt = operation.map(|operation| AdmissionKey {
            target: target.to_owned(),
            operation,
            kind: AdmissionKind::Delivery,
            nonce: Uuid::new_v4(),
        });
        if let Some(key) = &receipt {
            inner
                .register_admission(key)
                .map_err(|_| ResponseCloseError::Unknown)?;
        }
        drop(inner);
        Ok(WorkDeliveryReservation {
            receipt,
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
        self.reserve_create_owner_inner(target, nonce, classification, None)
    }
    fn reserve_create_owner_inner(
        &self,
        target: &str,
        nonce: Uuid,
        classification: CreateClassification,
        operation: Option<Uuid>,
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=reserve_create_owner site={} old_failed=false id=unknown",
                            line!()
                        );
                    }
                    inner.failed = true;
                }
            }
            let receipt = operation.map(|operation| AdmissionKey {
                target: target.to_owned(),
                operation,
                kind: AdmissionKind::CreateOwner,
                nonce,
            });
            if let Some(key) = &receipt {
                inner
                    .register_admission(key)
                    .map_err(|_| ResponseCloseError::Unknown)?;
                if inner.failed {
                    return Err(ResponseCloseError::Unknown);
                }
            }
            Ok(CreateAdmission::Reserved(CreateOwnerReservation {
                receipt,
                state: self.clone(),
                target: target.to_owned(),
                nonce,
                classification,
            }))
        })?
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=accept_durable site={} old_failed=false id=unknown",
                            line!()
                        );
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
                        if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug)
                            && !inner.failed
                        {
                            log::debug!(
                                target: DIAG_TARGET,
                                "response_close_diag outcome=observed stage=failed_transition \
                                 operation=drop site={} old_failed=false id=unknown",
                                line!()
                            );
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
        if let Some(key) = &self.receipt {
            let _ = self.state.release_observed_key(key);
            return;
        }
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=drop site={} old_failed=false id=unknown",
                            line!()
                        );
                    }
                    inner.failed = true;
                }
            }
        });
    }
}
impl WriterAdmission {
    pub(in crate::pty) fn is_closing(&self) -> Result<bool, ResponseCloseError> {
        self.state.is_closing(self.session_id, self.generation)
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
                    if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) && !inner.failed {
                        log::debug!(
                            target: DIAG_TARGET,
                            "response_close_diag outcome=observed stage=failed_transition \
                             operation=drop site={} old_failed=false id=unknown",
                            line!()
                        );
                    }
                    inner.failed = true;
                }
            }
        });
    }
}
impl Drop for CreateOwnerReservation {
    fn drop(&mut self) {
        if let Some(key) = &self.receipt {
            let _ = self.state.release_observed_key(key);
            return;
        }
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
                        if log::log_enabled!(target: DIAG_TARGET, log::Level::Debug)
                            && !inner.failed
                        {
                            log::debug!(
                                target: DIAG_TARGET,
                                "response_close_diag outcome=observed stage=failed_transition \
                                 operation=drop site={} old_failed=false id=unknown",
                                line!()
                            );
                        }
                        inner.failed = true;
                    }
                }
            }
        });
    }
}

impl ResponseCloseState {
    #[allow(dead_code)]
    pub(crate) fn begin_delivery_observed(
        &self,
        target: &str,
        operation: Uuid,
    ) -> Result<(WorkDeliveryReservation, TrackerReceiptHandle), TrackerProofError> {
        let reservation = self
            .begin_delivery_inner(target, Some(operation))
            .map_err(TrackerProofError::from)?;
        let handle = TrackerReceiptHandle {
            state: self.clone(),
            key: reservation
                .receipt
                .clone()
                .ok_or(TrackerProofError::Unknown)?,
        };
        Ok((reservation, handle))
    }
    #[allow(dead_code)]
    pub(crate) fn reserve_create_owner_observed(
        &self,
        target: &str,
        nonce: Uuid,
        classification: CreateClassification,
        operation: Uuid,
    ) -> Result<(CreateOwnerReservation, TrackerReceiptHandle), TrackerProofError> {
        match self
            .reserve_create_owner_inner(target, nonce, classification, Some(operation))
            .map_err(TrackerProofError::from)?
        {
            CreateAdmission::Reserved(reservation) => {
                let key = reservation
                    .receipt
                    .clone()
                    .ok_or(TrackerProofError::Unknown)?;
                Ok((
                    reservation,
                    TrackerReceiptHandle {
                        state: self.clone(),
                        key,
                    },
                ))
            }
            CreateAdmission::ClosingDeferred => Err(TrackerProofError::ClosingDeferred),
            CreateAdmission::WaitingIdentity => Err(TrackerProofError::Unknown),
        }
    }
    fn release_observed_key(
        &self,
        key: &AdmissionKey,
    ) -> Result<TrackerReleaseReceipt, TrackerProofError> {
        self.with_proof(|inner| {
            let Some(admission) = inner.admissions.get(key) else {
                return inner.proof_conflict();
            };
            if let Some(receipt) = &admission.release {
                return receipt.clone();
            }
            let generation = admission.generation.clone();
            let routes = admission.routes.clone();
            if !inner.admission_routes_match(key) {
                return inner.proof_conflict();
            }
            if generation != inner.target_generations(&key.target) {
                return inner.proof_conflict();
            }
            inner.admissions.get_mut(key).ok_or(TrackerProofError::Unknown)?.release =
                Some(Err(TrackerProofError::Unknown));
            let released = match key.kind {
                AdmissionKind::Delivery => inner
                    .work
                    .get_mut(&key.target)
                    .and_then(|work| {
                        work.dispatch
                            .checked_sub(1)
                            .map(|next| work.dispatch = next)
                    })
                    .is_some(),
                AdmissionKind::CreateOwner => {
                    let removed = inner
                        .create_owners
                        .get_mut(&key.target)
                        .is_some_and(|owners| owners.remove(&key.nonce).is_some());
                    if inner
                        .create_owners
                        .get(&key.target)
                        .is_some_and(|owners| owners.is_empty())
                    {
                        inner.create_owners.remove(&key.target);
                    }
                    removed
                }
            };
            if !released {
                return inner.proof_conflict();
            }
            inner.admissions.get_mut(key).ok_or(TrackerProofError::Unknown)?.debited = true;
            inner.touch_target(&key.target)?;
            let receipt = TrackerReleaseReceipt {
                key: key.clone(),
                epoch: inner.next_proof_epoch()?,
                generation,
                routes,
                result: super::TrackerReleaseResult::Released,
            };
            inner
                .admissions
                .get_mut(key)
                .ok_or(TrackerProofError::Unknown)?
                .release = Some(Ok(receipt.clone()));
            Ok(receipt)
        })
    }
}
impl From<ResponseCloseError> for TrackerProofError {
    fn from(error: ResponseCloseError) -> Self {
        match error {
            ResponseCloseError::Conflict => Self::Conflict,
            ResponseCloseError::ClosingDeferred => Self::ClosingDeferred,
            _ => Self::Unknown,
        }
    }
}
impl WorkDeliveryReservation {
    #[allow(dead_code)]
    pub(crate) fn release_observed(self) -> Result<TrackerReleaseReceipt, TrackerProofError> {
        self.state
            .release_observed_key(self.receipt.as_ref().ok_or(TrackerProofError::Unknown)?)
    }
}
impl CreateOwnerReservation {
    #[allow(dead_code)]
    pub(crate) fn release_observed(self) -> Result<TrackerReleaseReceipt, TrackerProofError> {
        self.state
            .release_observed_key(self.receipt.as_ref().ok_or(TrackerProofError::Unknown)?)
    }
}
impl TrackerReceiptHandle {
    #[allow(dead_code)]
    pub(crate) fn observe(&self) -> Result<Option<TrackerReleaseReceipt>, TrackerProofError> {
        self.state.with_proof(|inner| {
            inner
                .admissions
                .get(&self.key)
                .map(|a| a.release.clone().transpose())
                .ok_or(TrackerProofError::Unknown)?
        })
    }
    #[allow(dead_code)]
    pub(crate) fn settle_terminal_observed(
        &self,
        key: &ResponseTerminalKey,
    ) -> Result<ResponseTerminalReceipt, TrackerProofError> {
        self.state.with_proof(|inner| {
            if key.target != self.key.target || !inner.admission_routes_match(&self.key) {
                return inner.proof_conflict();
            }
            if let Some(old) = inner.terminal_receipts.get(key) {
                if old.admission != self.key || old.return_context.is_some()
                    || old.result != super::TrackerTerminalResult::Ordinary {
                    return inner.proof_conflict();
                }
                let mut receipt = old.clone();
                receipt.already_terminal = true;

                return Ok(receipt);
            }
            if inner.admissions.get(&self.key).is_none_or(|a| a.terminal_started) {
                return inner.proof_conflict();
            }
            let Some(work) = inner.work.get_mut(&key.target) else {
                return inner.proof_conflict();
            };
            if work.pending.get(&key.source_key) != Some(&key.digest)
                || work.terminal.contains_key(&key.source_key)
            {
                return inner.proof_conflict();
            }
            inner.admissions.get_mut(&self.key).ok_or(TrackerProofError::Unknown)?
                .terminal_started = true;
            work.pending.remove(&key.source_key);
            work.terminal
                .insert(key.source_key.clone(), key.digest.clone());
            inner.touch_target(&key.target)?;
            let receipt = ResponseTerminalReceipt {
                key: key.clone(),
                admission: self.key.clone(),
                epoch: inner.next_proof_epoch()?,
                already_terminal: false,
                return_context: None,
                result: super::TrackerTerminalResult::Ordinary,
            };
            inner.terminal_receipts.insert(key.clone(), receipt.clone());
            Ok(receipt)
        })
    }
}
