use super::super::poller::MailboxPoller;
#[cfg(test)]
use super::super::pty_claim::MailboxTestHooks;
use super::super::pty_claim::{
    await_pty_input_before_deadline, finish_pty_input_before_boundary,
    reacquire_pty_dispatch_target, reject_pty_input_before_boundary, PtyInputPreBoundaryStop,
    PtyReacquireGoal,
};
use super::super::settle::{
    finish_owned_pty_work, session_has_current_pty_submission_provenance, OwnedPtyWorkTransfer,
    PtySpawnStep,
};
use crate::api::message_store::{
    ClaimedPtyInputOperation, MessageStore, PreparationHeartbeatGuard, PtyInputStripeGuard,
    PtyInputTargetGate, PtyInputTargetGuard,
};
use crate::config::settings::SettingsState;
use crate::config::teams::VerifiedPtyInputIdentity;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Manager;
use uuid::Uuid;

/// Session chosen (or spawned) for a PTY input dispatch, with the verified
/// target identity and the managers the boundary write needs.
pub(in crate::phone::mailbox) type SelectedPtyTarget = (
    SessionInfo,
    bool,
    VerifiedPtyInputIdentity,
    Arc<tokio::sync::RwLock<SessionManager>>,
    Arc<Mutex<PtyManager>>,
);

/// Claimed operation facts the session selection reads but never changes.
pub(in crate::phone::mailbox) struct PtyDispatchContext<'a> {
    pub(in crate::phone::mailbox) store: &'a Arc<MessageStore>,
    pub(in crate::phone::mailbox) initial: &'a crate::phone::types::PtyInputResult,
    pub(in crate::phone::mailbox) claimed: &'a ClaimedPtyInputOperation,
    pub(in crate::phone::mailbox) injection_id: &'a str,
    pub(in crate::phone::mailbox) lease_owner: &'a str,
    pub(in crate::phone::mailbox) source_plane: crate::phone::types::PtyInputSourcePlane,
    pub(in crate::phone::mailbox) api_client_store:
        Option<&'a Arc<crate::api::auth::ApiClientStore>>,
    pub(in crate::phone::mailbox) target_gate: &'a Arc<PtyInputTargetGate>,
    pub(in crate::phone::mailbox) target: &'a String,
    pub(in crate::phone::mailbox) expires: chrono::DateTime<chrono::Utc>,
    pub(in crate::phone::mailbox) shutdown: &'a tokio_util::sync::CancellationToken,
}

/// Dispatch guards the session selection may release, reacquire or fill.
pub(in crate::phone::mailbox) struct PtyDispatchGuards<'a> {
    pub(in crate::phone::mailbox) target_guard: &'a mut Option<PtyInputTargetGuard>,
    pub(in crate::phone::mailbox) target_stripe: &'a mut Option<PtyInputStripeGuard>,
    pub(in crate::phone::mailbox) heartbeat: &'a mut PreparationHeartbeatGuard,
    pub(in crate::phone::mailbox) owned_work: &'a mut Option<OwnedPtyWorkTransfer>,
    pub(in crate::phone::mailbox) source_executor:
        &'a mut crate::session::selection::SourceExecutorGuard,
}

impl MailboxPoller {
    /// Picks the live target session, or spawns one, for a claimed PTY input.
    /// `Err(failed)` is the dispatch result when the operation stops here.
    pub(in crate::phone::mailbox) async fn select_pty_dispatch_session<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        context: &PtyDispatchContext<'_>,
        guards: PtyDispatchGuards<'_>,
    ) -> Result<SelectedPtyTarget, bool> {
        use crate::phone::types::PtyInputReasonCode as C;
        let PtyDispatchContext {
            store,
            initial,
            claimed,
            injection_id,
            lease_owner,
            source_plane,
            api_client_store,
            target_gate,
            target,
            expires,
            shutdown,
        } = *context;
        let PtyDispatchGuards {
            target_guard,
            target_stripe,
            heartbeat,
            owned_work,
            source_executor,
        } = guards;
        let mut prepared_replacement = None;
        loop {
            if app
                .try_state::<Arc<crate::RestoreInProgress>>()
                .is_some_and(|flag| flag.0.load(std::sync::atomic::Ordering::SeqCst))
            {
                let observed = finish_pty_input_before_boundary(
                    store,
                    heartbeat,
                    injection_id,
                    lease_owner,
                    C::RestoreInProgress,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                finish_owned_pty_work(app, owned_work, observed).await;
                return Err(false);
            }
            if app
                .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
                .is_some_and(|guard| guard.blocks_agent(&claimed.target_fqn))
            {
                let observed = finish_pty_input_before_boundary(
                    store,
                    heartbeat,
                    injection_id,
                    lease_owner,
                    C::PurgeInProgress,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                finish_owned_pty_work(app, owned_work, observed).await;
                return Err(false);
            }

            let verified_route =
                if source_plane == crate::phone::types::PtyInputSourcePlane::HostCli {
                    self.validate_claimed_host_authority(app, claimed).await
                } else {
                    match self
                        .validate_claimed_api_authority(app, claimed, api_client_store)
                        .await
                    {
                        Ok(route) => route,
                        Err(_) => {
                            let observed = finish_pty_input_before_boundary(
                                store,
                                heartbeat,
                                injection_id,
                                lease_owner,
                                C::StoreTransient,
                            )
                            .await;
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            finish_owned_pty_work(app, owned_work, observed).await;
                            return Err(false);
                        }
                    }
                };
            let Some(verified_route) = verified_route else {
                let code = if source_plane == crate::phone::types::PtyInputSourcePlane::HostCli {
                    C::AuthorityChanged
                } else {
                    C::ApiBindingMismatch
                };
                let observed =
                    reject_pty_input_before_boundary(store, heartbeat, injection_id, code).await;
                drop(target_guard.take());
                drop(target_stripe.take());
                finish_owned_pty_work(app, owned_work, observed).await;
                return Err(false);
            };
            if verified_route.target.canonical_fqn != *target {
                let observed = reject_pty_input_before_boundary(
                    store,
                    heartbeat,
                    injection_id,
                    C::AuthorityChanged,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                finish_owned_pty_work(app, owned_work, observed).await;
                return Err(false);
            }
            let verified_sender = verified_route.sender;
            let verified_target = verified_route.target;

            let session_manager = app
                .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                .inner()
                .clone();
            let pty_manager = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
            let sessions = {
                let guard = session_manager.read().await;
                guard.list_sessions().await
            };
            let mut matching: Vec<SessionInfo> = sessions
                .into_iter()
                .filter(|session| {
                    crate::config::teams::agent_fqn_from_path(&session.working_directory)
                        == claimed.target_fqn
                })
                .collect();
            matching.sort_by(|left, right| {
                fn rank(status: &SessionStatus) -> u8 {
                    match status {
                        SessionStatus::Active => 0,
                        SessionStatus::Running => 1,
                        SessionStatus::Idle => 2,
                        SessionStatus::Exited(_) => 3,
                    }
                }
                rank(&left.status)
                    .cmp(&rank(&right.status))
                    .then_with(|| right.created_at.cmp(&left.created_at))
                    .then_with(|| left.id.cmp(&right.id))
            });

            let pty_profile_settings = app.state::<SettingsState>().read().await.clone();
            let mut eligible = Vec::new();
            let mut saw_supported_busy = false;
            let mut saw_live_unsupported = false;
            let mut saw_live_inconsistent = false;
            let mut saw_live_temporary = false;
            for session in &matching {
                if matches!(session.status, SessionStatus::Exited(_)) {
                    continue;
                }
                if session
                    .name
                    .starts_with(crate::session::session::TEMP_SESSION_PREFIX)
                {
                    saw_live_temporary = true;
                    continue;
                }
                let Ok(id) = Uuid::parse_str(&session.id) else {
                    saw_live_unsupported = true;
                    continue;
                };
                let (live, route_backend, route_identities) = {
                    let manager = pty_manager
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    (
                        manager.has_session(id),
                        manager.backend_kind(id),
                        manager.route_identities(id),
                    )
                };
                let path_matches =
                    route_identities.is_some_and(|(cwd_identity, replica_anchor, _)| {
                        let current_cwd = crate::path_identity::verify_directory(Path::new(
                            &session.working_directory,
                        ));
                        let current_target = crate::config::teams::verify_pty_input_replica_cwd(
                            Path::new(&session.working_directory),
                        );
                        matches!(
                            (cwd_identity, current_cwd),
                            (Some(saved), Ok(current))
                                if crate::path_identity::same_object(&saved, &current)
                        ) && matches!(
                            (replica_anchor, current_target),
                            (Some(saved), Ok(current))
                                if current.canonical_fqn == verified_target.canonical_fqn
                                    && current.authority_fingerprint
                                        == verified_target.authority_fingerprint
                                    && crate::path_identity::same_object(
                                        &saved,
                                        &verified_target.replica_identity,
                                    )
                        )
                    });
                if !live || route_backend != Some(session.backend_kind) || !path_matches {
                    saw_live_inconsistent = true;
                    continue;
                }
                if !session_has_current_pty_submission_provenance(session, &pty_profile_settings) {
                    saw_live_unsupported = true;
                    continue;
                }
                if session.waiting_for_input {
                    eligible.push(session.clone());
                } else {
                    saw_supported_busy = true;
                }
            }
            let (selected, spawned) = if let Some(selected) = eligible.into_iter().next() {
                (selected, false)
            } else if saw_supported_busy {
                let observed =
                    reject_pty_input_before_boundary(store, heartbeat, injection_id, C::Busy).await;
                drop(target_guard.take());
                drop(target_stripe.take());
                finish_owned_pty_work(app, owned_work, observed).await;
                return Err(false);
            } else if saw_live_temporary {
                let observed = reject_pty_input_before_boundary(
                    store,
                    heartbeat,
                    injection_id,
                    C::NonpersistentLiveSession,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                finish_owned_pty_work(app, owned_work, observed).await;
                return Err(false);
            } else if saw_live_inconsistent {
                if claimed.attempt < 5 {
                    let observed = finish_pty_input_before_boundary(
                        store,
                        heartbeat,
                        injection_id,
                        lease_owner,
                        C::SessionRace,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, owned_work, observed).await;
                } else {
                    let observed = reject_pty_input_before_boundary(
                        store,
                        heartbeat,
                        injection_id,
                        C::InconsistentSession,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, owned_work, observed).await;
                }
                return Err(false);
            } else if saw_live_unsupported {
                let observed = reject_pty_input_before_boundary(
                    store,
                    heartbeat,
                    injection_id,
                    C::UnsupportedSession,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                finish_owned_pty_work(app, owned_work, observed).await;
                return Err(false);
            } else {
                #[cfg(test)]
                let fixture_control = self
                    .test_hooks
                    .as_ref()
                    .and_then(MailboxTestHooks::replacement_fixture_control)
                    .filter(|control| {
                        !control
                            .ticket_only
                            .load(std::sync::atomic::Ordering::Acquire)
                    });
                #[cfg(test)]
                let fixture_lease = if fixture_control.is_some() {
                    let (Some(exact), Some(stripe)) = (target_guard.take(), target_stripe.take())
                    else {
                        heartbeat.finish().await;
                        return Err(false);
                    };
                    Some(Arc::new(crate::commands::session::FixturePtyCutLease {
                        gate: target_gate.clone(),
                        target: target.clone(),
                        exact,
                        stripe,
                    }))
                } else {
                    None
                };
                let spawn = {
                    #[cfg(test)]
                    let (stripe, exact) = match fixture_lease.as_ref() {
                        Some(lease) => (Some(&lease.stripe), Some(&lease.exact)),
                        None => (target_stripe.as_ref(), target_guard.as_ref()),
                    };
                    #[cfg(not(test))]
                    let (stripe, exact) = (target_stripe.as_ref(), target_guard.as_ref());
                    let (Some(stripe), Some(exact)) = (stripe, exact) else {
                        heartbeat.finish().await;
                        return Err(false);
                    };
                    let target_ownership = match target_gate.target_ownership(target, stripe, exact)
                    {
                        Ok(proof) => proof,
                        Err(_) => {
                            heartbeat.finish().await;
                            return Err(false);
                        }
                    };
                    await_pty_input_before_deadline(
                        expires,
                        shutdown,
                        self.spawn_pty_input_target(
                            app,
                            claimed,
                            &matching,
                            &verified_sender,
                            &verified_target,
                            source_plane,
                            api_client_store,
                            &target_ownership,
                            initial,
                            prepared_replacement.take(),
                            source_executor,
                            #[cfg(test)]
                            fixture_lease.as_ref(),
                        ),
                    )
                    .await
                };
                #[cfg(test)]
                if let Some(lease) = fixture_lease {
                    let restored = Arc::try_unwrap(lease);
                    let worker_still_holds_clone = restored.is_err();
                    let receipt_missing = fixture_control.as_ref().is_some_and(|control| {
                        control.submitted.load(std::sync::atomic::Ordering::Acquire)
                            && !control.received.load(std::sync::atomic::Ordering::Acquire)
                    });
                    match restored {
                        Ok(lease) if !receipt_missing => {
                            *target_guard = Some(lease.exact);
                            *target_stripe = Some(lease.stripe);
                        }
                        other => {
                            drop(other);
                            if let Some(control) = fixture_control.as_ref() {
                                control.restoration_shared.store(
                                    worker_still_holds_clone,
                                    std::sync::atomic::Ordering::Release,
                                );
                                control
                                    .restoration_failed
                                    .store(true, std::sync::atomic::Ordering::Release);
                            }
                            // This cut precedes every Retained/timeout/error
                            // handoff, even if the owner already has a stop or
                            // rollback receipt. A submitted job without actual
                            // receive has no source-side work authority.
                            heartbeat.finish().await;
                            return Err(false);
                        }
                    }
                }
                match spawn {
                    Ok(Ok(PtySpawnStep::Created(session))) => (session, true),
                    Ok(Ok(PtySpawnStep::OwnedCreated(session, continuation))) => {
                        let Some(effect) = continuation.start_effect(
                            crate::session::selection::LifecycleEffectSlot::WorkTransfer,
                        ) else {
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            heartbeat.finish().await;
                            return Err(false);
                        };
                        *owned_work = Some((continuation, effect));
                        (session, true)
                    }
                    Ok(Ok(PtySpawnStep::Retained {
                        continuation,
                        failed,
                    })) => {
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        let stop = continuation.source_stop();
                        let rolled_back = continuation.precommit_rollback_completed();
                        if stop.is_none() && !rolled_back {
                            heartbeat.finish().await;
                            return Err(failed);
                        }
                        let effect = continuation.start_effect(
                            crate::session::selection::LifecycleEffectSlot::WorkTransfer,
                        );
                        use crate::session::selection::OwnedSourceStop as Stop;
                        let observed = match stop {
                            Some(Stop::Terminal) => {
                                heartbeat.finish().await;
                                true
                            }
                            reason => {
                                let code = match reason {
                                    Some(Stop::AuthorityChanged) => C::AuthorityChanged,
                                    Some(Stop::StoreTransient) => C::StoreTransient,
                                    Some(Stop::Expired) => C::Expired,
                                    Some(Stop::LeaseLost) => C::LeaseLost,
                                    Some(Stop::Terminal) => unreachable!(),
                                    None => C::SpawnFailedSafe,
                                };
                                finish_pty_input_before_boundary(
                                    store,
                                    heartbeat,
                                    injection_id,
                                    lease_owner,
                                    code,
                                )
                                .await
                            }
                        };
                        if let Some(effect) = effect {
                            let manager = app
                                .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                                .read()
                                .await
                                .clone();
                            let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
                            if rolled_back {
                                continuation
                                    .finish_rolled_back_work(&manager, &pty, effect, observed)
                                    .await;
                            } else {
                                continuation
                                    .finish_observed_work_transfer(&manager, &pty, effect, observed)
                                    .await;
                            }
                        }
                        return Err(failed);
                    }
                    Ok(Ok(PtySpawnStep::RetainedUnowned)) => {
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        heartbeat.finish().await;
                        return Err(false);
                    }
                    Ok(Ok(PtySpawnStep::NeedReacquire(prepared))) => {
                        prepared_replacement = prepared;
                        // The spawn block ended the proof borrow above.
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        let Some((stripe, exact)) = reacquire_pty_dispatch_target(
                            app,
                            (store, initial, claimed),
                            (target_gate, &verified_target, PtyReacquireGoal::Spawn),
                            (expires, shutdown),
                            heartbeat,
                        )
                        .await
                        else {
                            return Err(false);
                        };
                        *target_stripe = Some(stripe);
                        *target_guard = Some(exact);
                        continue;
                    }
                    Ok(Err(code)) => {
                        let observed = finish_pty_input_before_boundary(
                            store,
                            heartbeat,
                            injection_id,
                            lease_owner,
                            code,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        finish_owned_pty_work(app, owned_work, observed).await;
                        return Err(false);
                    }
                    Err(PtyInputPreBoundaryStop::Expired) => {
                        let observed = reject_pty_input_before_boundary(
                            store,
                            heartbeat,
                            injection_id,
                            C::Expired,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        finish_owned_pty_work(app, owned_work, observed).await;
                        return Err(false);
                    }
                    Err(PtyInputPreBoundaryStop::Shutdown) => {
                        heartbeat.finish().await;
                        return Err(false);
                    }
                }
            };
            let selected_id = Uuid::parse_str(&selected.id).ok();
            if owned_work.is_none()
                && selected_id.is_some_and(|id| {
                    crate::config::sessions_persistence::unconfirmed_replacement_work_waits(
                        &claimed.target_fqn,
                        id,
                    )
                })
            {
                drop(target_guard.take());
                drop(target_stripe.take());
                let Some(id) = selected_id else {
                    heartbeat.finish().await;
                    return Err(false);
                };
                let Some((stripe, exact)) = reacquire_pty_dispatch_target(
                    app,
                    (store, initial, claimed),
                    (
                        target_gate,
                        &verified_target,
                        PtyReacquireGoal::Published(id),
                    ),
                    (expires, shutdown),
                    heartbeat,
                )
                .await
                else {
                    return Err(false);
                };
                *target_stripe = Some(stripe);
                *target_guard = Some(exact);
                continue;
            }
            return Ok((
                selected,
                spawned,
                verified_target,
                session_manager,
                pty_manager,
            ));
        }
    }
}
