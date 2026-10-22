use super::poller::MailboxPoller;
#[cfg(test)]
use super::pty_claim::MailboxTestHooks;
use super::pty_claim::{
    await_pty_input_before_deadline, finish_pty_input_before_boundary,
    pty_input_lease_failure_code, reacquire_pty_dispatch_target, refresh_pty_claim_for_resume,
    reject_pty_input_before_boundary, PtyClaimRefresh, PtyInputPreBoundaryStop, PtyReacquireGoal,
};
use super::settle::{
    finish_owned_pty_work, session_has_current_pty_submission_provenance, OwnedPtyWorkTransfer,
    PtySpawnStep,
};
use crate::config::settings::SettingsState;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
use crate::session::session::SessionStatus;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::Emitter;
use tauri::Manager;
use uuid::Uuid;

impl MailboxPoller {
    pub(crate) async fn dispatch_pty_input_operation_core<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        state: &crate::api::message_store::MessageStoreState,
        injection_id: &str,
        source_plane: crate::phone::types::PtyInputSourcePlane,
        api_client_store: Option<&Arc<crate::api::auth::ApiClientStore>>,
    ) {
        use crate::phone::types::{PtyInputPublicStatus as S, PtyInputReasonCode as C};
        let mut source_executor = crate::session::selection::SourceExecutorGuard::unbound();
        #[cfg(test)]
        if let Some(control) = self
            .test_hooks
            .as_ref()
            .and_then(MailboxTestHooks::replacement_fixture_control)
        {
            if !control.after_pty_core_accept(injection_id).await {
                return;
            }
        }
        // Borrow the lexical observer through the whole existing dispatch
        // body. This is polled inline, with no new task or accepted core.
        let failed = async {
            let store = match &state.store {
                Ok(store) => Arc::clone(store),
                Err(_) => return false,
            };
            let shutdown = app
                .try_state::<crate::shutdown::ShutdownSignal>()
                .map(|signal| signal.token().clone())
                .unwrap_or_else(tokio_util::sync::CancellationToken::new);
            let _operation_lock = match store.try_operation_lock(injection_id) {
                Ok(Some(lock)) => lock,
                _ => return false,
            };
            let _active = match state.active_operations.try_register(injection_id) {
                Some(active) => active,
                None => return false,
            };
            let initial = match store
                .query_pty_input_by_injection_offloaded(injection_id.to_string())
                .await
            {
                Ok(Some(result)) if !result.terminal => result,
                _ => return false,
            };
            let target = match initial.target.clone() {
                Some(target) => target,
                None => return false,
            };
            let _response_delivery = match store.begin_response_delivery(&target) {
                Ok(reservation) => reservation,
                Err(_) => return false,
            };
            let target_gate = match &state.target_gate {
                Ok(gate) => Arc::clone(gate),
                Err(_) => return false,
            };
            // Startup restore must finish before this dispatch owns target guards.
            // No claim or payload has been consumed at this point.
            if let Some(coordinator) =
                app.try_state::<crate::session::selection::SelectionCoordinator>()
            {
                if coordinator.wait_for_restore_release().await.is_err() {
                    return false;
                }
            }
            let mut target_stripe = Some(match target_gate.try_target_lock(&target) {
                Ok(Some(lock)) => lock,
                _ => return false,
            });
            let initial_expires =
                match initial.expires_at.as_deref().and_then(|value| {
                    crate::phone::types::parse_canonical_pty_timestamp(value).ok()
                }) {
                    Some(expires) => expires,
                    None => {
                        if store
                            .terminalize_pty_input_offloaded(
                                injection_id.to_string(),
                                S::Rejected,
                                Some(C::StoreCorrupt),
                                chrono::Utc::now(),
                            )
                            .await
                            .is_err()
                        {
                            log::warn!(
                        "[pty-input] invalid initial deadline id={} code=terminal_store_failed",
                        injection_id
                    );
                        }
                        return false;
                    }
                };
            let mut target_guard = Some(
                match await_pty_input_before_deadline(
                    initial_expires,
                    &shutdown,
                    target_gate.acquire_exact(&target),
                )
                .await
                {
                    Ok(guard) => guard,
                    Err(PtyInputPreBoundaryStop::Shutdown) => return false,
                    Err(PtyInputPreBoundaryStop::Expired) => {
                        if store
                            .terminalize_pty_input_offloaded(
                                injection_id.to_string(),
                                S::Rejected,
                                Some(C::Expired),
                                chrono::Utc::now(),
                            )
                            .await
                            .is_err()
                        {
                            log::warn!(
                        "[pty-input] target-wait expiry failed id={} code=terminal_store_failed",
                        injection_id
                    );
                        }
                        return false;
                    }
                },
            );
            {
                let (Some(stripe), Some(exact)) = (target_stripe.as_ref(), target_guard.as_ref())
                else {
                    return false;
                };
                if target_gate
                    .target_ownership(&target, stripe, exact)
                    .is_err()
                {
                    return false;
                }
            }
            let lease_owner = Uuid::new_v4().to_string();
            let claimed = match store
                .claim_pty_input_offloaded(
                    source_plane,
                    Some(injection_id.to_string()),
                    lease_owner.clone(),
                    chrono::Utc::now(),
                )
                .await
            {
                Ok(Some(claimed)) => claimed,
                _ => return false,
            };
            let expires =
                match crate::phone::types::parse_canonical_pty_timestamp(&claimed.expires_at) {
                    Ok(expires) => expires,
                    Err(_) => {
                        if store
                            .terminalize_pty_input_offloaded(
                                injection_id.to_string(),
                                S::Rejected,
                                Some(C::StoreCorrupt),
                                chrono::Utc::now(),
                            )
                            .await
                            .is_err()
                        {
                            log::warn!(
                                "[pty-input] corrupt-expiry rejection failed id={} \
                        code=terminal_store_failed",
                                injection_id
                            );
                        }
                        return false;
                    }
                };
            let mut heartbeat = crate::api::message_store::PreparationHeartbeatGuard::start(
                Arc::clone(&store),
                injection_id.to_string(),
                lease_owner.clone(),
                expires,
            );
            if expires <= chrono::Utc::now() {
                reject_pty_input_before_boundary(&store, &mut heartbeat, injection_id, C::Expired)
                    .await;
                return false;
            }
            match refresh_pty_claim_for_resume(&store, &initial, &claimed).await {
                Ok(PtyClaimRefresh::Current) => {}
                Ok(PtyClaimRefresh::Terminal) => {
                    heartbeat.finish().await;
                    return false;
                }
                Err(code) => {
                    finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        code,
                    )
                    .await;
                    return false;
                }
            }
            let mut prepared_replacement = None;
            let mut owned_work: Option<OwnedPtyWorkTransfer> = None;
            let (selected, spawned, verified_target, session_manager, pty_manager) = loop {
                if app
                    .try_state::<Arc<crate::RestoreInProgress>>()
                    .is_some_and(|flag| flag.0.load(std::sync::atomic::Ordering::SeqCst))
                {
                    let observed = finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        C::RestoreInProgress,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                if app
                    .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
                    .is_some_and(|guard| guard.blocks_agent(&claimed.target_fqn))
                {
                    let observed = finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        C::PurgeInProgress,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }

                let verified_route =
                    if source_plane == crate::phone::types::PtyInputSourcePlane::HostCli {
                        self.validate_claimed_host_authority(app, &claimed).await
                    } else {
                        match self
                            .validate_claimed_api_authority(app, &claimed, api_client_store)
                            .await
                        {
                            Ok(route) => route,
                            Err(_) => {
                                let observed = finish_pty_input_before_boundary(
                                    &store,
                                    &mut heartbeat,
                                    injection_id,
                                    &lease_owner,
                                    C::StoreTransient,
                                )
                                .await;
                                drop(target_guard.take());
                                drop(target_stripe.take());
                                finish_owned_pty_work(app, &mut owned_work, observed).await;
                                return false;
                            }
                        }
                    };
                let Some(verified_route) = verified_route else {
                    let code = if source_plane == crate::phone::types::PtyInputSourcePlane::HostCli
                    {
                        C::AuthorityChanged
                    } else {
                        C::ApiBindingMismatch
                    };
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        code,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                };
                if verified_route.target.canonical_fqn != target {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::AuthorityChanged,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
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
                    if !session_has_current_pty_submission_provenance(
                        session,
                        &pty_profile_settings,
                    ) {
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
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::Busy,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                } else if saw_live_temporary {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::NonpersistentLiveSession,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                } else if saw_live_inconsistent {
                    if claimed.attempt < 5 {
                        let observed = finish_pty_input_before_boundary(
                            &store,
                            &mut heartbeat,
                            injection_id,
                            &lease_owner,
                            C::SessionRace,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                    } else {
                        let observed = reject_pty_input_before_boundary(
                            &store,
                            &mut heartbeat,
                            injection_id,
                            C::InconsistentSession,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                    }
                    return false;
                } else if saw_live_unsupported {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::UnsupportedSession,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
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
                        let (Some(exact), Some(stripe)) =
                            (target_guard.take(), target_stripe.take())
                        else {
                            heartbeat.finish().await;
                            return false;
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
                            return false;
                        };
                        let target_ownership =
                            match target_gate.target_ownership(&target, stripe, exact) {
                                Ok(proof) => proof,
                                Err(_) => {
                                    heartbeat.finish().await;
                                    return false;
                                }
                            };
                        await_pty_input_before_deadline(
                            expires,
                            &shutdown,
                            self.spawn_pty_input_target(
                                app,
                                &claimed,
                                &matching,
                                &verified_sender,
                                &verified_target,
                                source_plane,
                                api_client_store,
                                &target_ownership,
                                &initial,
                                prepared_replacement.take(),
                                &mut source_executor,
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
                                target_guard = Some(lease.exact);
                                target_stripe = Some(lease.stripe);
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
                                return false;
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
                                return false;
                            };
                            owned_work = Some((continuation, effect));
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
                                return failed;
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
                                        &store,
                                        &mut heartbeat,
                                        injection_id,
                                        &lease_owner,
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
                                        .finish_observed_work_transfer(
                                            &manager, &pty, effect, observed,
                                        )
                                        .await;
                                }
                            }
                            return failed;
                        }
                        Ok(Ok(PtySpawnStep::RetainedUnowned)) => {
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            heartbeat.finish().await;
                            return false;
                        }
                        Ok(Ok(PtySpawnStep::NeedReacquire(prepared))) => {
                            prepared_replacement = prepared;
                            // The spawn block ended the proof borrow above.
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            let Some((stripe, exact)) = reacquire_pty_dispatch_target(
                                app,
                                (&store, &initial, &claimed),
                                (&target_gate, &verified_target, PtyReacquireGoal::Spawn),
                                (expires, &shutdown),
                                &mut heartbeat,
                            )
                            .await
                            else {
                                return false;
                            };
                            target_stripe = Some(stripe);
                            target_guard = Some(exact);
                            continue;
                        }
                        Ok(Err(code)) => {
                            let observed = finish_pty_input_before_boundary(
                                &store,
                                &mut heartbeat,
                                injection_id,
                                &lease_owner,
                                code,
                            )
                            .await;
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            finish_owned_pty_work(app, &mut owned_work, observed).await;
                            return false;
                        }
                        Err(PtyInputPreBoundaryStop::Expired) => {
                            let observed = reject_pty_input_before_boundary(
                                &store,
                                &mut heartbeat,
                                injection_id,
                                C::Expired,
                            )
                            .await;
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            finish_owned_pty_work(app, &mut owned_work, observed).await;
                            return false;
                        }
                        Err(PtyInputPreBoundaryStop::Shutdown) => {
                            heartbeat.finish().await;
                            return false;
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
                        return false;
                    };
                    let Some((stripe, exact)) = reacquire_pty_dispatch_target(
                        app,
                        (&store, &initial, &claimed),
                        (
                            &target_gate,
                            &verified_target,
                            PtyReacquireGoal::Published(id),
                        ),
                        (expires, &shutdown),
                        &mut heartbeat,
                    )
                    .await
                    else {
                        return false;
                    };
                    target_stripe = Some(stripe);
                    target_guard = Some(exact);
                    continue;
                }
                break (
                    selected,
                    spawned,
                    verified_target,
                    session_manager,
                    pty_manager,
                );
            };
            let selected_id = match Uuid::parse_str(&selected.id) {
                Ok(id) => id,
                Err(_) => {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::InconsistentSession,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
            };
            let readiness = await_pty_input_before_deadline(
                expires,
                &shutdown,
                self.wait_for_pty_input_ready(
                    app,
                    &claimed,
                    &verified_target,
                    selected_id,
                    spawned,
                    source_plane,
                    api_client_store,
                    &heartbeat,
                ),
            )
            .await;
            match readiness {
                Ok(Ok(())) => {}
                Ok(Err(code)) => {
                    let observed = finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        code,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Expired) => {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::Expired,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Shutdown) => {
                    heartbeat.finish().await;
                    return false;
                }
            }
            if !matches!(
                store
                    .renew_pty_input_lease_offloaded(
                        injection_id.to_string(),
                        lease_owner.clone(),
                        chrono::Utc::now(),
                    )
                    .await,
                Ok(true)
            ) {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    pty_input_lease_failure_code(expires),
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            let permit = match await_pty_input_before_deadline(
                expires,
                &shutdown,
                PtyManager::acquire_input_writer(&pty_manager, selected_id),
            )
            .await
            {
                Ok(Ok(permit)) => permit,
                Ok(Err(_)) => {
                    let observed = finish_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        &lease_owner,
                        C::SessionRace,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Expired) => {
                    let observed = reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::Expired,
                    )
                    .await;
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
                Err(PtyInputPreBoundaryStop::Shutdown) => {
                    heartbeat.finish().await;
                    return false;
                }
            };
            let final_api_guard =
                if source_plane == crate::phone::types::PtyInputSourcePlane::ContainerApi {
                    let Some(client_store) = api_client_store else {
                        let observed = reject_pty_input_before_boundary(
                            &store,
                            &mut heartbeat,
                            injection_id,
                            C::AuthorityChanged,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        return false;
                    };
                    let (Some(client_id), Some(generation)) = (
                        claimed.authority_client_id.as_deref(),
                        claimed.authority_client_generation.as_deref(),
                    ) else {
                        let observed = reject_pty_input_before_boundary(
                            &store,
                            &mut heartbeat,
                            injection_id,
                            C::AuthorityChanged,
                        )
                        .await;
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        return false;
                    };
                    match client_store
                        .load_active_binding_fresh_offloaded(
                            client_id.to_string(),
                            generation.to_string(),
                            crate::api::auth::SCOPE_PTY_INPUT,
                        )
                        .await
                    {
                        Ok(Some(guard)) => Some(guard),
                        Ok(None) => {
                            let observed = reject_pty_input_before_boundary(
                                &store,
                                &mut heartbeat,
                                injection_id,
                                C::AuthorityChanged,
                            )
                            .await;
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            drop(permit);
                            finish_owned_pty_work(app, &mut owned_work, observed).await;
                            return false;
                        }
                        Err(_) => {
                            let observed = finish_pty_input_before_boundary(
                                &store,
                                &mut heartbeat,
                                injection_id,
                                &lease_owner,
                                C::StoreTransient,
                            )
                            .await;
                            drop(target_guard.take());
                            drop(target_stripe.take());
                            drop(permit);
                            finish_owned_pty_work(app, &mut owned_work, observed).await;
                            return false;
                        }
                    }
                } else {
                    None
                };
            let final_authority = if let Some(fresh) = final_api_guard.as_ref() {
                self.validate_claimed_api_authority_with_fresh(app, &claimed, fresh)
                    .await
            } else {
                self.validate_claimed_host_authority(app, &claimed).await
            };
            let Some(final_authority) = final_authority else {
                let observed = reject_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    C::AuthorityChanged,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            };
            if final_authority.target.authority_fingerprint != verified_target.authority_fingerprint
                || !crate::path_identity::same_object(
                    &final_authority.target.replica_identity,
                    &verified_target.replica_identity,
                )
            {
                let observed = reject_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    C::AuthorityChanged,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if self
                .validate_selected_pty_target(
                    app,
                    &claimed,
                    &verified_target,
                    selected_id,
                    selected.backend_kind,
                )
                .await
                .is_none()
            {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    C::SessionRace,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if heartbeat.failed() {
                let code = if expires <= chrono::Utc::now() {
                    C::Expired
                } else {
                    C::LeaseLost
                };
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    code,
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if !matches!(
                store
                    .renew_pty_input_lease_offloaded(
                        injection_id.to_string(),
                        lease_owner.clone(),
                        chrono::Utc::now(),
                    )
                    .await,
                Ok(true)
            ) {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    pty_input_lease_failure_code(expires),
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            if !heartbeat.finish().await {
                let observed = finish_pty_input_before_boundary(
                    &store,
                    &mut heartbeat,
                    injection_id,
                    &lease_owner,
                    pty_input_lease_failure_code(expires),
                )
                .await;
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            let backend_name = match selected.backend_kind {
                SessionBackendKind::LocalProcess => "localProcess",
                SessionBackendKind::ContainerTransport => "containerTransport",
            };
            let payload = match store
                .begin_pty_actuating_offloaded(
                    injection_id.to_string(),
                    lease_owner.clone(),
                    selected.id.clone(),
                    backend_name.to_string(),
                    chrono::Utc::now(),
                )
                .await
            {
                Ok(payload) => payload,
                Err(crate::api::message_store::MessageStoreError::ActuationCommitAmbiguous) => {
                    let terminal = store
                        .terminalize_pty_input_offloaded(
                            injection_id.to_string(),
                            S::Indeterminate,
                            Some(C::TerminalStoreFailed),
                            chrono::Utc::now(),
                        )
                        .await;
                    let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                    drop(final_api_guard);
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    drop(permit);
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    if terminal.is_err() {
                        log::error!(
                            "[pty-input] ambiguous boundary reconciliation failed id={} \
                        code=terminal_store_failed",
                            injection_id
                        );
                    }
                    return false;
                }
                Err(_) => {
                    reject_pty_input_before_boundary(
                        &store,
                        &mut heartbeat,
                        injection_id,
                        C::StoreTransient,
                    )
                    .await;
                    return false;
                }
            };
            let post_authority = if let Some(fresh) = final_api_guard.as_ref() {
                self.validate_claimed_api_authority_with_fresh(app, &claimed, fresh)
                    .await
            } else {
                self.validate_claimed_host_authority(app, &claimed).await
            };
            let payload_matches = initial.payload_bytes == Some(payload.len() as u64)
                && initial.payload_sha256.as_deref()
                    == Some(crate::phone::types::sha256_hex(&payload).as_str());
            let post_valid = payload_matches
                && post_authority.is_some_and(|route| {
                    route.sender.authority_fingerprint
                        == final_authority.sender.authority_fingerprint
                        && route.target.authority_fingerprint
                            == verified_target.authority_fingerprint
                        && crate::path_identity::same_object(
                            &route.target.replica_identity,
                            &verified_target.replica_identity,
                        )
                })
                && self
                    .validate_selected_pty_target(
                        app,
                        &claimed,
                        &verified_target,
                        selected_id,
                        selected.backend_kind,
                    )
                    .await
                    .is_some();
            if !post_valid {
                let terminal = store
                    .terminalize_pty_input_offloaded(
                        injection_id.to_string(),
                        S::Indeterminate,
                        Some(C::FinalRevalidationFailed),
                        chrono::Utc::now(),
                    )
                    .await;
                let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                drop(final_api_guard);
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                if terminal.is_err() {
                    log::error!(
                        "[pty-input] post-boundary terminalization failed id={} \
                    code=terminal_store_failed",
                        injection_id
                    );
                }
                return false;
            }
            let manager = {
                let guard = session_manager.read().await;
                guard.clone()
            };
            let idle_detector = app
                .state::<Arc<crate::pty::idle_detector::IdleDetector>>()
                .inner()
                .clone();
            let authority_id =
                match crate::phone::types::parse_canonical_uuid_v4(&claimed.authority_session_id) {
                    Ok(id) => id,
                    Err(_) => {
                        let terminal = store
                            .terminalize_pty_input_offloaded(
                                injection_id.to_string(),
                                S::Indeterminate,
                                Some(C::FinalRevalidationFailed),
                                chrono::Utc::now(),
                            )
                            .await;
                        let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                        drop(final_api_guard);
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        if terminal.is_err() {
                            log::error!(
                                "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                                injection_id
                            );
                        }
                        return false;
                    }
                };
            let authority_backend = match source_plane {
                crate::phone::types::PtyInputSourcePlane::HostCli => {
                    SessionBackendKind::LocalProcess
                }
                crate::phone::types::PtyInputSourcePlane::ContainerApi => {
                    SessionBackendKind::ContainerTransport
                }
            };
            let authority_container_backend = pty_manager
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .container_backend();
            let authority_route =
                match PtyManager::authority_route_proof(&pty_manager, authority_id) {
                    Ok(proof) => proof,
                    Err(_) => {
                        let terminal = store.terminalize_pty_input(
                            injection_id,
                            S::Indeterminate,
                            Some(C::FinalRevalidationFailed),
                            chrono::Utc::now(),
                        );
                        let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                        drop(final_api_guard);
                        drop(target_guard.take());
                        drop(target_stripe.take());
                        drop(permit);
                        finish_owned_pty_work(app, &mut owned_work, observed).await;
                        if terminal.is_err() {
                            log::error!(
                                "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                                injection_id
                            );
                        }
                        return false;
                    }
                };
            let expected_api_binding = final_api_guard.as_ref().and_then(|fresh| {
                let root =
                    crate::path_identity::verify_directory(Path::new(&fresh.client.bound_root))
                        .ok()?;
                Some((
                    fresh.client.client_id.clone(),
                    fresh.client.credential_generation.clone()?,
                    fresh.client.bound_session_id.clone()?,
                    root.object_id,
                    fresh.presented_token_hash.clone(),
                ))
            });
            if source_plane == crate::phone::types::PtyInputSourcePlane::ContainerApi
                && expected_api_binding.is_none()
            {
                let terminal = store.terminalize_pty_input(
                    injection_id,
                    S::Indeterminate,
                    Some(C::FinalRevalidationFailed),
                    chrono::Utc::now(),
                );
                let observed = terminal.as_ref().is_ok_and(|result| result.terminal);
                drop(final_api_guard);
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                if terminal.is_err() {
                    log::error!(
                    "[pty-input] boundary terminalization failed id={} code=terminal_store_failed",
                    injection_id
                );
                }
                return false;
            }
            let boundary_settings = app.state::<SettingsState>().inner().clone();
            let owned_route = owned_work
                .as_ref()
                .and_then(|_| PtyManager::snapshot_route_proof(&pty_manager, selected_id).ok());
            let boundary_carrier = crate::session::manager::acquire_carrier_mutation_permit().await;
            let owned_token = owned_work
                .as_ref()
                .and_then(|(continuation, _)| continuation.launch_token());
            let owned_current = match owned_work.as_ref() {
                Some((continuation, _)) => {
                    manager
                        .with_current_carrier_snapshot(&boundary_carrier, |current| {
                            owned_token.is_some()
                                && continuation.current_completion_region(
                                    &boundary_carrier,
                                    current,
                                    &pty_manager,
                                )
                        })
                        .await
                }
                None => true,
            };
            if !owned_current {
                drop(boundary_carrier);
                let observed = store
                    .terminalize_pty_input_offloaded(
                        injection_id.to_string(),
                        S::Indeterminate,
                        Some(C::FinalRevalidationFailed),
                        chrono::Utc::now(),
                    )
                    .await
                    .is_ok_and(|result| result.terminal);
                drop(final_api_guard);
                drop(target_guard.take());
                drop(target_stripe.take());
                drop(permit);
                finish_owned_pty_work(app, &mut owned_work, observed).await;
                return false;
            }
            let boundary_write = match manager
                .prepare_pty_input_boundary_with_carrier_permit(
                    &boundary_carrier,
                    selected_id,
                    &verified_target,
                    selected.backend_kind,
                    authority_id,
                    &final_authority.sender,
                    authority_backend,
                    &authority_route,
                    &permit,
                    &idle_detector,
                    &boundary_settings,
                    |session, settings| {
                        owned_token.is_none_or(|token| session.token == token)
                            && owned_work.as_ref().is_none_or(|(continuation, _)| {
                                owned_route.as_ref().is_some_and(|proof| {
                                    continuation.matches_captured_launch_route(proof)
                                })
                            })
                            && session_has_current_pty_submission_provenance(
                                &SessionInfo::from(session),
                                settings,
                            )
                    },
                    || {
                        if expires <= chrono::Utc::now()
                            || app
                                .try_state::<Arc<crate::RestoreInProgress>>()
                                .is_some_and(|flag| {
                                    flag.0.load(std::sync::atomic::Ordering::SeqCst)
                                })
                            || app
                                .try_state::<Arc<crate::session::purge_guard::PurgeGuard>>()
                                .is_some_and(|guard| guard.blocks_agent(&claimed.target_fqn))
                        {
                            return false;
                        }
                        match (&source_plane, expected_api_binding.as_ref()) {
                            (crate::phone::types::PtyInputSourcePlane::HostCli, None) => true,
                            (
                                crate::phone::types::PtyInputSourcePlane::ContainerApi,
                                Some((
                                    client_id,
                                    generation,
                                    bound_session_id,
                                    root_object,
                                    token_hash,
                                )),
                            ) => {
                                let binding =
                                    authority_container_backend.credential_binding(authority_id);
                                binding.is_some_and(|binding| {
                                    binding.client_id == *client_id
                                        && binding.credential_generation == *generation
                                        && binding.bound_session_id == *bound_session_id
                                        && binding.bound_root_object_id == *root_object
                                        && crate::api::auth::constant_time_eq(
                                            &binding.credential_token_hash,
                                            token_hash,
                                        )
                                })
                            }
                            _ => false,
                        }
                    },
                )
                .await
            {
                Ok(guard) => {
                    drop(boundary_carrier);
                    Ok(crate::pty::inject::write_exact_agent_input_first(
                        guard, &payload,
                    ))
                }
                Err(_) => {
                    drop(boundary_carrier);
                    // The guard-bearing Result ends in this match. Only the
                    // observed terminal boolean survives to the work handoff await.
                    let terminal = store.terminalize_pty_input(
                        injection_id,
                        S::Indeterminate,
                        Some(C::FinalRevalidationFailed),
                        chrono::Utc::now(),
                    );
                    if terminal.is_err() {
                        log::error!(
                            "[pty-input] boundary terminalization failed id={} \
                        code=terminal_store_failed",
                            injection_id
                        );
                    }
                    Err(terminal.as_ref().is_ok_and(|result| result.terminal))
                }
            };
            let text_write_succeeded = match boundary_write {
                Ok(written) => written,
                Err(observed) => {
                    drop(final_api_guard);
                    drop(target_guard.take());
                    drop(target_stripe.take());
                    drop(permit);
                    finish_owned_pty_work(app, &mut owned_work, observed).await;
                    return false;
                }
            };
            // Keep the fresh registry lock through the synchronous first-write
            // boundary so a concurrent revocation linearizes before or after it,
            // never between the final authority check and the text write.
            drop(final_api_guard);
            let outcome = crate::pty::inject::submit_exact_agent_input_with_permit(
                &permit,
                text_write_succeeded,
            )
            .await;
            let (status, reason) = match outcome {
                crate::pty::inject::AgentSubmitOutcome::TextWriteFailed => {
                    (S::Indeterminate, Some(C::TextWriteFailed))
                }
                crate::pty::inject::AgentSubmitOutcome::RequiredEnterFailed => {
                    (S::Indeterminate, Some(C::RequiredEnterFailed))
                }
                crate::pty::inject::AgentSubmitOutcome::Submitted {
                    redundant_enter_failed,
                } => {
                    let metadata = if payload == b"/clear" {
                        crate::commands::pty::stamp_fresh_boundary_to_session(app, selected_id)
                            .await
                    } else if payload == b"/compact" {
                        crate::commands::pty::BoundaryMetadataOutcome::Unchanged
                    } else {
                        crate::commands::pty::note_post_boundary_content_to_session(
                            app,
                            selected_id,
                        )
                        .await
                    };
                    let reason =
                        if metadata == crate::commands::pty::BoundaryMetadataOutcome::Failed {
                            Some(C::BoundaryMetadataFailed)
                        } else {
                            redundant_enter_failed.then_some(C::RedundantEnterFailed)
                        };
                    (S::Injected, reason)
                }
            };
            let result = match store
                .terminalize_pty_input_offloaded(
                    injection_id.to_string(),
                    status,
                    reason,
                    chrono::Utc::now(),
                )
                .await
            {
                Ok(result) => result,
                Err(_) => {
                    log::error!(
                        "[pty-input] terminal persistence failed id={} code=terminal_store_failed",
                        injection_id
                    );
                    return false;
                }
            };
            crate::api::audit::record_pty_input_result("terminal", &result);
            drop(target_guard.take());
            drop(target_stripe.take());
            drop(permit);
            finish_owned_pty_work(app, &mut owned_work, result.terminal).await;
            if app.emit("pty_input_status", &result).is_err() {
                log::warn!(
                    "[pty-input] status event failed id={} code=boundary_metadata_failed",
                    injection_id
                );
            }
            false
        }
        .await;
        source_executor.returned(failed);
    }
}
