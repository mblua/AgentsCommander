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
            Ok(CreateAdmission::Reserved(CreateOwnerReservation {
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
