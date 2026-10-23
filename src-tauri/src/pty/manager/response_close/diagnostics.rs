use crate::pty::manager::response_close::ResponseCloseInner;
use crate::pty::manager::response_close::ResponseCloseRoute;
use crate::pty::manager::response_close::ResponseCloseState;
use crate::pty::manager::response_close::DIAG_TARGET;
use std::sync::TryLockError;
use uuid::Uuid;

impl ResponseCloseState {
    /// Observation only: never recover poison or invoke the authority wrapper.
    pub(crate) fn diagnostic_poll_health(&self) -> bool {
        if !log::log_enabled!(target: DIAG_TARGET, log::Level::Debug) {
            return false;
        }
        let mut inner = match self.inner.try_lock() {
            Ok(inner) => inner,
            Err(TryLockError::WouldBlock) => {
                log::debug!(
                    target: DIAG_TARGET,
                    "response_close_diag id=unknown site={} stage=poll_health \
                     scope=pre_reconcile_pre_selection outcome=busy health=unknown",
                    line!()
                );
                return false;
            }
            Err(TryLockError::Poisoned(_)) => {
                log::debug!(
                    target: DIAG_TARGET,
                    "response_close_diag id=unknown site={} stage=poll_health \
                     scope=pre_reconcile_pre_selection outcome=poisoned health=unknown",
                    line!()
                );
                return false;
            }
        };
        let emit = !inner.diagnostic_poll_initialized || inner.diagnostic_poll_modulo == 9;
        inner.diagnostic_poll_initialized = true;
        inner.diagnostic_poll_modulo = (inner.diagnostic_poll_modulo + 1) % 10;
        if emit {
            log::debug!(
                target: DIAG_TARGET,
                "response_close_diag id=unknown site={} outcome=observed stage=poll_health \
                 scope=pre_reconcile_pre_selection ready={} publishers={} bootstrap={} failed={} \
                 tracker_effective_enabled={} tracker_effective_seconds={} settings_epoch={} \
                 unknown_spawns={}",
                line!(),
                inner.ready,
                inner.publishers,
                inner.work_bootstrapped,
                inner.failed,
                inner.enabled,
                inner.seconds,
                inner.settings_epoch,
                inner.unknown_spawns
            );
        }
        emit
    }

    pub(in crate::pty::manager) fn diagnostic_select_observation(
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
            log::debug!(
                target: DIAG_TARGET,
                "response_close_diag id=unknown site={} outcome=observed stage=select \
                 scope=post_settings_apply_post_member_publish_post_coordinator_\
                 publish_pre_ticket_decision \
                 member={} actual_safe={} false_mask={} unknown_mask={} first_false={:?} \
                 first_unknown={:?} diagnostic_inconsistent={} ready={} publishers={} \
                 bootstrap={} failed={} tracker_effective_enabled={} tracker_effective_seconds={} \
                 settings_epoch={} unknown_spawns={}",
                line!(),
                id,
                actual_safe,
                false_mask,
                unknown_mask,
                first_false,
                first_unknown,
                actual_safe != (false_mask == 0 && unknown_mask == 0),
                inner.ready,
                inner.publishers,
                inner.work_bootstrapped,
                inner.failed,
                inner.enabled,
                inner.seconds,
                inner.settings_epoch,
                inner.unknown_spawns
            );
            log::debug!(
                target: DIAG_TARGET,
                "response_close_diag id=unknown site={} outcome=observed stage=select_route \
                 member={} generation={} tracker_mirror_version={} eligible={} \
                 manager_working={:?} detector_working={:?} menu_blocked={:?} hold_known={} \
                 hold_remaining_ms={:?} held_count={} writers={} work_epoch={} \
                 activity_version={} closing={} external_live_facts=unknown policy_present={}",
                line!(),
                id,
                route.generation,
                route.manager_version,
                route.eligible,
                route.manager_working,
                route.detector_working,
                route.menu_blocked,
                route.hold_known,
                route.hold_until.map(|end| end.saturating_duration_since(now).as_millis()),
                route.held.len(),
                route.writers,
                route.work_epoch,
                route.activity_version,
                route.closing.is_some(),
                policy.is_some()
            );
            Self::diagnostic_select_policy(inner, route, now, anchor);
        }
        if let Some(route) = inner.routes.get_mut(&id) {
            route.diagnostic_select = next_cache;
        }
    }

    pub(in crate::pty::manager) fn diagnostic_select_policy(
        inner: &ResponseCloseInner,
        route: &ResponseCloseRoute,
        now: std::time::Instant,
        anchor: Option<std::time::Instant>,
    ) {
        let Some(policy) = &route.policy else {
            log::debug!(
                target: DIAG_TARGET,
                "response_close_diag id=unknown site={} stage=select_policy outcome=absent \
                 captured_member=unknown captured_coordinator=unknown target_counts=unknown \
                 ages=unknown facts_match=unknown epoch_match=unknown",
                line!()
            );
            return;
        };
        let work = inner.work.get(&policy.member.target);
        let required = std::time::Duration::from_secs(u64::from(inner.seconds));
        let age = anchor.map(|anchor| now.saturating_duration_since(anchor));
        log::debug!(
            target: DIAG_TARGET,
            "response_close_diag id=unknown site={} outcome=observed stage=select_policy \
             captured_member={} captured_coordinator={} captured_member_generation={} \
             captured_coordinator_generation={} captured_member_carrier_version={} \
             captured_coordinator_carrier_version={} submitted={} registered={} success={} \
             pending_returns={} selected={} member_facts_match={} coordinator_facts_match={} \
             settings_epoch_match={} work_epoch_match={}",
            line!(),
            policy.member.session_id,
            policy.coordinator.session_id,
            policy.member.generation,
            policy.coordinator.generation,
            policy.member.carrier_version,
            policy.coordinator.carrier_version,
            policy.submitted,
            policy.registered_at.is_some(),
            policy.success_at.is_some(),
            policy.returns.len(),
            policy.selected.is_some(),
            route.facts.as_ref() == Some(&policy.member),
            inner
                .routes
                .get(&policy.coordinator.session_id)
                .is_some_and(|r| r.facts.as_ref() == Some(&policy.coordinator)),
            policy.settings_epoch == inner.settings_epoch,
            policy.work_epoch == route.work_epoch
        );
        log::debug!(
            target: DIAG_TARGET,
            "response_close_diag id=unknown site={} outcome=observed stage=select_work \
             captured_member={} provisional={:?} dispatch={:?} pending={:?} create_owners={:?} \
             registered_age_ms={:?} success_age_ms={:?} last_change_age_ms={} anchor_age_ms={:?} \
             required_ms={} remaining_ms={:?}",
            line!(),
            policy.member.session_id,
            route.facts.as_ref().map(|_| work.map_or(0, |w| w.provisional)),
            route.facts.as_ref().map(|_| work.map_or(0, |w| w.dispatch)),
            route.facts.as_ref().map(|_| work.map_or(0, |w| w.pending.len())),
            route.facts.as_ref().map(|_| {
                inner
                    .create_owners
                    .get(&policy.member.target)
                    .map_or(0, |owners| owners.len())
            }),
            policy.registered_at.map(|at| now.saturating_duration_since(at).as_millis()),
            policy.success_at.map(|at| now.saturating_duration_since(at).as_millis()),
            now.saturating_duration_since(route.last_change).as_millis(),
            age.map(|age| age.as_millis()),
            required.as_millis(),
            age.map(|age| required.saturating_sub(age).as_millis())
        );
    }
}
