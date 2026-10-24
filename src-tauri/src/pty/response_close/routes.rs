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
