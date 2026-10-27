use crate::app_state::DetachedSessionsState;
use crate::commands::session::execute_destroy_transaction_common;
use crate::commands::session::reset_substantive_input;
use crate::commands::session::DestroyOutcome;
use crate::commands::session::DestroyRequest;
use crate::commands::session::DestructionSource;
use crate::config::settings::SettingsState;
use crate::pty::backend::SessionBackendKind;
use crate::pty::manager::PtyManager;
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::acquire_carrier_mutation_permit;
use crate::session::manager::SessionManager;
use crate::session::selection::response_close::ResponseEffectOwner;
use crate::session::selection::SelectionSource;
use crate::session::selection::SelectionTransaction;
use crate::session::session::SessionCommunicationKind;
use crate::session::session::SessionStatus;
use crate::telegram::manager::TelegramBridgeState;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use uuid::Uuid;

pub(crate) fn response_effect_owners<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Arc<crate::session::selection::response_close::ResponseEffectRegistry> {
    app.state::<crate::app_state::AppOwners>()
        .response_effects
        .clone()
}

fn diagnostic_value<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
}

pub(crate) fn log_discard_veto<R: tauri::Runtime>(
    app: &AppHandle<R>,
    permit: &crate::session::manager::CarrierMutationPermit,
    id: Uuid,
) {
    use crate::config::sessions_persistence::{observe_discard_veto, DiscardVetoReason};
    // This returns owned data and drops all lower locks before consulting owners.
    let observations: Vec<crate::config::sessions_persistence::DiscardVetoObservation> =
        observe_discard_veto(permit, id);
    let owners = app
        .try_state::<crate::app_state::AppOwners>()
        .and_then(|state| state.response_effects.snapshot());
    let tracker = app.try_state::<crate::pty::manager::ResponseCloseState>();
    for observed in observations {
        let mut owner_reason = "owner_unavailable";
        let mut close_nonce = None;
        let mut generation = None;
        let mut recorded_terminal = None;
        if observed.reason == DiscardVetoReason::ProtectionClosing {
            let owner = owners.as_ref().and_then(|owners| {
                owners.iter().find(|owner| {
                    Some(owner.registration().member.session_id) == observed.protected_session
                })
            });
            if let Some(owner) = owner {
                let registration = owner.registration();
                let binding = owner.ticket().committed_binding();
                let carrier = &registration.carrier;
                if binding.member == registration.member
                    && observed.protected_session == Some(carrier.session_id)
                    && observed.token == Some(binding.member.token)
                    && observed.token == Some(carrier.token)
                    && observed.target.as_ref() == Some(&binding.member.target)
                    && observed.target.as_ref() == Some(&carrier.target)
                    && observed.protection_nonce == Some(carrier.nonce)
                    && tracker
                        .as_ref()
                        .is_some_and(|t| registration.tracker.same_state(t))
                {
                    close_nonce = Some(binding.close_nonce);
                    generation = Some(binding.member.generation);
                    recorded_terminal = owner.diagnostic_terminal();
                    owner_reason = if recorded_terminal.is_some() {
                        "observed"
                    } else {
                        "owner_record_unreadable"
                    };
                } else {
                    owner_reason = "owner_identity_mismatch";
                }
            }
        }
        log::warn!(
            target: "agentscommander_lib::response_close_diag",
            "stage=discard_veto reason={} session={} protected_session={} target={} \
             phase={} protection_nonce={} token={} epoch={} version={} owner_reason={} \
             close_nonce={} generation={} recorded_terminal={} owner_nonce={} \
             previous={} replacement={}",
            observed.reason.code(), observed.session,
            diagnostic_value(observed.protected_session),
            diagnostic_value(observed.target), diagnostic_value(observed.phase),
            diagnostic_value(observed.protection_nonce), diagnostic_value(observed.token),
            diagnostic_value(observed.epoch), diagnostic_value(observed.version), owner_reason,
            diagnostic_value(close_nonce), diagnostic_value(generation),
            diagnostic_value(recorded_terminal), diagnostic_value(observed.owner_nonce),
            diagnostic_value(observed.previous), diagnostic_value(observed.replacement)
        );
    }
}

pub(crate) async fn response_group_effect(
    owner: &Arc<ResponseEffectOwner>,
    monitor: &Arc<ResourceMonitorState>,
) -> Option<()> {
    let id = owner.registration().member.session_id;
    if !monitor.has_registered_group(id) {
        return Some(());
    }
    owner.start_group(|| {
        let monitor = monitor.clone();
        owner.backend().publish_stop_witness(id, "session-destroy");
        tokio::task::spawn_blocking(move || {
            monitor.kill_group(
                id,
                crate::resource_monitor::ResourceKillReason::SessionDestroy,
            )
        })
    })?;
    std::future::poll_fn(|cx| owner.poll_group(cx, id)).await
}

pub(crate) async fn execute_response_close_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    ticket: crate::pty::manager::ResponseCloseTicket,
) -> Result<ResponseCloseDisposition, String> {
    let id = ticket.committed_binding().member.session_id;
    execute_destroy_transaction_common(
        transaction,
        DestroyRequest {
            ids: vec![id],
            source: DestructionSource::BackgroundCleanup,
            force_destroy_root: false,
        },
        Some(ticket),
    )
    .await
    .map(|completion| match completion {
        DestroyCompletion::Response(disposition) => disposition,
        DestroyCompletion::Legacy(_) => {
            unreachable!("conditional close selected the response core")
        }
    })
}

pub(crate) async fn execute_owned_response_close<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    ticket: crate::pty::manager::ResponseCloseTicket,
) -> Result<ResponseCloseDisposition, String> {
    let transaction = transaction.clone();
    crate::config::sessions_persistence::run_owned_persist_core(async move {
        execute_response_close_core(&transaction, ticket).await
    })
    .await
}

#[derive(Debug)]
pub(crate) enum ResponseCloseDisposition {
    Rejected,
    Dormant,
    Reopened,
    ReconcilePending,
}

pub(crate) enum DestroyCompletion {
    Legacy(DestroyOutcome),
    Response(ResponseCloseDisposition),
}

pub(crate) fn response_close_current_rows<'a>(
    current: &'a crate::session::manager::CarrierSnapshot,
    registration: &crate::session::selection::ResponseCloseRegistration,
) -> Option<(
    &'a crate::session::session::Session,
    &'a crate::session::session::Session,
)> {
    let find = |identity: &crate::pty::manager::ResponseRouteIdentity| {
        current.rows.iter().find(|row| {
            row.id == identity.session_id
                && row.token == identity.token
                && row.backend_kind == identity.backend
                && current.versions.get(&row.id) == Some(&identity.carrier_version)
                && !current.pending_ids.contains(&row.id)
                && !matches!(row.status, SessionStatus::Exited(_))
        })
    };
    let member = find(&registration.member)?;
    let coordinator = find(&registration.coordinator)?;
    if member.backend_kind != SessionBackendKind::LocalProcess
        || member.is_coordinator
        || member.is_root_agent
        || member.agent_kind.is_none()
        || !coordinator.is_coordinator
        || coordinator.is_root_agent
        || member
            .communication
            .as_ref()
            .is_some_and(|communication| communication.kind == SessionCommunicationKind::CoManaged)
    {
        return None;
    }
    Some((member, coordinator))
}

pub(crate) async fn commit_response_close<R: tauri::Runtime>(
    app: &AppHandle<R>,
    manager: &SessionManager,
    owner: &Arc<ResponseEffectOwner>,
) -> bool {
    let member = &owner.registration().member;
    let coordinator = &owner.registration().coordinator;
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let Ok(member_route) = PtyManager::snapshot_route_proof(&pty, member.session_id) else {
        return false;
    };
    let Ok(coordinator_route) = PtyManager::snapshot_route_proof(&pty, coordinator.session_id)
    else {
        return false;
    };
    let Ok(member_authority) = PtyManager::authority_route_proof(&pty, member.session_id) else {
        return false;
    };
    let Ok(coordinator_authority) = PtyManager::authority_route_proof(&pty, coordinator.session_id)
    else {
        return false;
    };
    if member_route.backend_kind() != SessionBackendKind::LocalProcess
        || coordinator_route.backend_kind() != coordinator.backend
    {
        return false;
    }
    let settings_state = app.state::<SettingsState>();
    let roots = settings_state.read().await.project_paths.clone();
    if crate::config::teams::verified_wg_coordinator_target(&coordinator.target, &roots).is_none() {
        return false;
    }
    let settings = settings_state.read().await;
    if settings.project_paths != roots {
        return false;
    }
    let tracker = app
        .state::<crate::pty::manager::ResponseCloseState>()
        .inner()
        .clone();
    let permit = acquire_carrier_mutation_permit().await;
    use crate::config::sessions_persistence::{
        with_confirmed_carrier, CarrierCheckpointTransition,
    };
    use crate::config::settings::try_with_agents_layer_publication_state;
    use crate::path_identity::{same_object, verify_directory};
    use crate::session::session::is_working;
    manager
        .with_current_carrier_snapshot(&permit, |current| {
            let Some((member_row, coordinator_row)) =
                response_close_current_rows(current, owner.registration())
            else {
                return false;
            };
            let member_cwd = Path::new(&member_row.working_directory);
            let coordinator_cwd = Path::new(&coordinator_row.working_directory);
            if !verify_directory(member_cwd)
                .is_ok_and(|actual| same_object(&actual, member_route.saved_cwd()))
                || !verify_directory(coordinator_cwd)
                    .is_ok_and(|actual| same_object(&actual, coordinator_route.saved_cwd()))
            {
                return false;
            }
            let now = std::time::Instant::now();
            tracker.apply_settings(
                settings.response_close_enabled,
                settings.response_close_idle_seconds,
                now,
            );
            tracker.publish_manager_facts(member.clone(), true, is_working(member_row), now);
            tracker.publish_manager_facts(
                coordinator.clone(),
                false,
                is_working(coordinator_row),
                now,
            );
            let carrier = &owner.registration().carrier;
            try_with_agents_layer_publication_state(&carrier.directory, |suppressed| {
                if suppressed {
                    return None;
                }
                let _coordinator_guard = coordinator_authority
                    .lock_verified(
                        coordinator.backend,
                        coordinator_route.saved_cwd(),
                        coordinator_route.saved_replica(),
                    )
                    .ok()?;
                if tracker.generation(coordinator.session_id) != Some(coordinator.generation) {
                    return None;
                }
                with_confirmed_carrier(
                    &permit,
                    carrier,
                    current,
                    CarrierCheckpointTransition::BeginClosing,
                    |_| {
                        member_authority
                            .try_commit_response_close(&tracker, owner.ticket(), now)
                            .ok()
                    },
                )
            })
            .ok()
            .flatten()
            .is_some()
        })
        .await
}

pub(crate) async fn execute_response_close_core<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    ticket: crate::pty::manager::ResponseCloseTicket,
) -> Result<ResponseCloseDisposition, String> {
    let binding = ticket.committed_binding();
    if binding.member.backend != SessionBackendKind::LocalProcess {
        return Ok(ResponseCloseDisposition::Rejected);
    }
    let Some(registration) =
        crate::session::selection::response_close_registration(binding.member.session_id)
    else {
        return Ok(ResponseCloseDisposition::Rejected);
    };
    if registration.member != binding.member {
        return Ok(ResponseCloseDisposition::Rejected);
    }
    let backend = {
        let pty = transaction.app().state::<Arc<Mutex<PtyManager>>>();
        let pty = pty.lock().map_err(|_| "pty_manager_poisoned".to_owned())?;
        pty.backend_for_kind(binding.member.backend)
    };
    let owner = Arc::new(ResponseEffectOwner::new(ticket, registration, backend));
    if !response_effect_owners(transaction.app())
        .insert_if_absent(binding.member.session_id, owner.clone())
        .map_err(|_| "response_effect_owner_poisoned".to_owned())?
    {
        return Ok(ResponseCloseDisposition::ReconcilePending);
    }
    let manager = transaction.manager().await;
    if !commit_response_close(transaction.app(), &manager, &owner).await {
        response_effect_owners(transaction.app())
            .remove(binding.member.session_id)
            .map_err(|_| "response_effect_owner_poisoned".to_owned())?;
        return Ok(ResponseCloseDisposition::Rejected);
    }
    let monitor = transaction
        .app()
        .state::<Arc<ResourceMonitorState>>()
        .inner()
        .clone();
    if response_group_effect(&owner, &monitor).await.is_none() {
        return Ok(ResponseCloseDisposition::ReconcilePending);
    }
    let Some(kill) = owner.start("kill") else {
        return Ok(ResponseCloseDisposition::ReconcilePending);
    };
    let kill_result = owner
        .backend()
        .kill(binding.member.session_id)
        .map_err(|error| error.to_string());
    let kill_error = kill_result.as_ref().err().cloned();
    if owner.complete(kill, kill_result).is_none() {
        return Ok(ResponseCloseDisposition::ReconcilePending);
    }
    if owner.backend().has_session(binding.member.session_id) {
        let _ = owner.seal();
        return Ok(settle_response_close(transaction.app(), &manager, &owner)
            .await
            .unwrap_or(ResponseCloseDisposition::ReconcilePending));
    }
    if let Some(error) = kill_error {
        log::warn!(
            "[session] teardown reported an error after PTY loss session={} \
             source=ResponseClose: {}",
            binding.member.session_id,
            error
        );
    }
    run_response_close_auxiliary(transaction, &manager, &owner).await;
    let _ = owner.seal();
    Ok(settle_response_close(transaction.app(), &manager, &owner)
        .await
        .unwrap_or(ResponseCloseDisposition::ReconcilePending))
}

pub(crate) async fn detach_response_close_telegram<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: Uuid,
) -> Result<(), String> {
    let bridge_shutdown = {
        let telegram = app.state::<TelegramBridgeState>();
        let mut telegram = telegram.lock().await;
        if telegram.has_bridge(id) {
            telegram
                .detach(id)
                .map(Some)
                .map_err(|error| error.to_string())
        } else {
            Ok(None)
        }
    };
    match bridge_shutdown {
        Ok(Some(shutdown)) => {
            // Retain the bot assignment on A as resume intent while the live
            // bridge shuts down; the next owned replacement carries it.
            let result = app
                .emit(
                    "telegram_bridge_detached",
                    serde_json::json!({ "sessionId": id.to_string() }),
                )
                .map_err(|error| error.to_string());
            shutdown.spawn_wait_or_abort();
            result
        }
        Ok(None) => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) async fn run_response_close_auxiliary<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    manager: &SessionManager,
    owner: &Arc<ResponseEffectOwner>,
) {
    let id = owner.registration().member.session_id;
    let Some(detach) = owner.start("detach") else {
        return;
    };
    let detached_result = transaction
        .app()
        .state::<DetachedSessionsState>()
        .lock()
        .map(|mut detached| {
            detached.remove(&id);
        })
        .map_err(|_| "detached_sessions_poisoned".to_owned());
    if owner.complete(detach, detached_result).is_none() {
        return;
    }

    let Some(bridge) = owner.start("telegram") else {
        return;
    };
    let bridge_result = detach_response_close_telegram(transaction.app(), id).await;
    if let Err(error) = &bridge_result {
        log::warn!(
            "[session] telegram detach publication failed session={} source=ResponseClose: {}",
            id,
            error
        );
    }
    if owner.complete(bridge, bridge_result).is_none() {
        return;
    }

    let Some(commit) = owner.start("manager") else {
        return;
    };
    let committed = transaction.commit_response_close_retained(id).await;
    let committed = match committed {
        Ok(committed) => {
            let _ = owner.complete(commit, Ok(()));
            committed
        }
        Err(error) => {
            let _ = owner.complete(commit, Err(error));
            return;
        }
    };
    let Some(persist) = owner.start("persist") else {
        return;
    };
    let persisted =
        crate::config::sessions_persistence::persist_current_state_result(manager).await;
    if let Err(error) = &persisted {
        log::warn!(
            "[selection] persistence failed source={} session={:?}: {}",
            SelectionSource::BackgroundCleanup,
            Some(id),
            error
        );
    }
    if owner.complete(persist, persisted).is_none() {
        return;
    }

    let Some(readers) = owner.start("readers") else {
        return;
    };
    crate::telegram::manager::release_all_reader_demands(transaction.app(), id).await;
    if owner.complete(readers, Ok(())).is_none() {
        return;
    }

    let Some(window) = owner.start("window") else {
        return;
    };
    let label = format!("terminal-{}", id.to_string().replace('-', ""));
    let closed = transaction
        .app()
        .get_webview_window(&label)
        .map_or(Ok(()), |window| {
            window.destroy().map_err(|error| error.to_string())
        });
    if let Err(error) = &closed {
        log::warn!(
            "[session] detached window destroy failed session={} source=ResponseClose: {}",
            id,
            error
        );
    }
    if owner.complete(window, closed).is_none() {
        return;
    }

    let Some(events) = owner.start("events") else {
        return;
    };
    transaction.publish_destroyed(id);
    reset_substantive_input(transaction.app(), id);
    for row in &committed.changed_rows {
        transaction.publish_created(row);
    }
    for id in &committed.cleared_raise_hand_ids {
        transaction.publish_communication_cleared(*id);
    }
    if let Some(selection) = &committed.selection {
        transaction.publish_selection(selection);
    }
    let _ = owner.complete(events, Ok(()));
}

#[derive(Clone, Copy)]
enum SettlementRejection {
    OwnerNotTerminal,
    RouteUnavailable,
    AuthorityUnavailable,
    NonlocalRoute,
    MemberRowUnavailable,
    CwdIdentityFailed,
    LifecycleUnavailable,
    GenerationMismatch,
    SettlementClassificationRejected,
    DetachedRegistryUnreadable,
    DetachedMember,
    ReplicaUnavailable,
    PublicationStateUnavailable,
    PublicationSuppressed,
    ConfirmedCarrierRejected,
    TrackerFinishRejected,
    PostfinishCleanup,
    #[cfg(test)]
    TestForcedSettlement,
}

impl SettlementRejection {
    fn code(self) -> &'static str {
        match self {
            Self::OwnerNotTerminal => "owner_not_terminal",
            Self::RouteUnavailable => "route_unavailable",
            Self::AuthorityUnavailable => "authority_unavailable",
            Self::NonlocalRoute => "nonlocal_route",
            Self::MemberRowUnavailable => "member_row_unavailable",
            Self::CwdIdentityFailed => "cwd_identity_failed",
            Self::LifecycleUnavailable => "lifecycle_unavailable",
            Self::GenerationMismatch => "generation_mismatch",
            Self::SettlementClassificationRejected => "settlement_classification_rejected",
            Self::DetachedRegistryUnreadable => "detached_registry_unreadable",
            Self::DetachedMember => "detached_member",
            Self::ReplicaUnavailable => "replica_unavailable",
            Self::PublicationStateUnavailable => "publication_state_unavailable",
            Self::PublicationSuppressed => "publication_suppressed",
            Self::ConfirmedCarrierRejected => "confirmed_carrier_rejected",
            Self::TrackerFinishRejected => "tracker_finish_rejected",
            Self::PostfinishCleanup => "postfinish_cleanup",
            #[cfg(test)]
            Self::TestForcedSettlement => "test_forced_settlement",
        }
    }
}

pub(crate) async fn settle_response_close<R: tauri::Runtime>(
    app: &AppHandle<R>,
    manager: &SessionManager,
    owner: &Arc<ResponseEffectOwner>,
) -> Option<ResponseCloseDisposition> {
    match settle_response_close_result(app, manager, owner).await {
        Ok(disposition) => Some(disposition),
        Err(reason) => {
            if !matches!(reason, SettlementRejection::PostfinishCleanup) {
                let binding = owner.ticket().committed_binding();
                log::warn!(
                    target: "agentscommander_lib::response_close_diag",
                    "stage=settlement reason={} session={} target={} token={} \
                     protection_nonce={} close_nonce={} generation={}",
                    reason.code(), binding.member.session_id, binding.member.target,
                    binding.member.token, owner.registration().carrier.nonce,
                    binding.close_nonce, binding.member.generation
                );
            }
            None
        }
    }
}

fn log_postfinish_cleanup(
    owner: &ResponseEffectOwner,
    disposition: &ResponseCloseDisposition,
    reason: &'static str,
) {
    let binding = owner.ticket().committed_binding();
    log::warn!(
        target: "agentscommander_lib::response_close_diag",
        "stage=postfinish_cleanup settled=true disposition={:?} reason={} session={} \
         close_nonce={} generation={}",
        disposition, reason, binding.member.session_id,
        binding.close_nonce, binding.member.generation
    );
}

async fn settle_response_close_result<R: tauri::Runtime>(
    app: &AppHandle<R>,
    manager: &SessionManager,
    owner: &Arc<ResponseEffectOwner>,
) -> Result<ResponseCloseDisposition, SettlementRejection> {
    if !owner.terminal() {
        return Err(SettlementRejection::OwnerNotTerminal);
    }
    #[cfg(test)]
    if owner.forced_settlement_for_test() {
        return Err(SettlementRejection::TestForcedSettlement);
    }
    let binding = owner.ticket().committed_binding();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let route = PtyManager::snapshot_route_proof(&pty, binding.member.session_id)
        .map_err(|_| SettlementRejection::RouteUnavailable)?;
    let authority = PtyManager::authority_route_proof(&pty, binding.member.session_id)
        .map_err(|_| SettlementRejection::AuthorityUnavailable)?;
    if route.backend_kind() != SessionBackendKind::LocalProcess {
        return Err(SettlementRejection::NonlocalRoute);
    }
    let tracker = app
        .state::<crate::pty::manager::ResponseCloseState>()
        .inner()
        .clone();
    let permit = acquire_carrier_mutation_permit().await;
    use crate::config::sessions_persistence::{
        with_confirmed_carrier, CarrierCheckpointTransition,
    };
    use crate::config::settings::try_with_agents_layer_publication_state;
    use crate::path_identity::{same_object, verify_directory};
    use crate::pty::context_scrape::ContextSessionLiveness;
    use crate::pty::manager::{
        ResponseCloseCurrentSettlement, ResponseCloseSettlementEvidence,
        ResponseCloseSettlementOutcome,
    };
    use crate::session::manager::CarrierPhysicalIdentity;
    use crate::session::session::is_working;
    let mut rejection = SettlementRejection::MemberRowUnavailable;
    let settled = manager
        .with_current_carrier_snapshot(&permit, |current| {
            let row = current.rows.iter().find(|row| {
                row.id == binding.member.session_id
                    && row.token == binding.member.token
                    && row.backend_kind == SessionBackendKind::LocalProcess
                    && !current.pending_ids.contains(&row.id)
            })?;
            rejection = SettlementRejection::CwdIdentityFailed;
            if !verify_directory(Path::new(&row.working_directory))
                .is_ok_and(|actual| same_object(&actual, route.saved_cwd()))
            {
                return None;
            }
            rejection = SettlementRejection::LifecycleUnavailable;
            let _lifecycle = authority
                .lock_verified(
                    SessionBackendKind::LocalProcess,
                    route.saved_cwd(),
                    route.saved_replica(),
                )
                .ok()?;
            rejection = SettlementRejection::GenerationMismatch;
            if tracker.generation(row.id) != Some(binding.member.generation) {
                return None;
            }
            let absent = !owner.backend().has_session(row.id);
            let dormant = absent
                && matches!(row.status, SessionStatus::Exited(0))
                && !row.was_detached
                && current.selection.id() != Some(row.id);
            let live = !absent
                && owner.kill_failed()
                && !matches!(row.status, SessionStatus::Exited(_))
                && current.versions.get(&row.id).is_some_and(|version| {
                    *version >= binding.member.carrier_version && *version < u64::MAX
                })
                && owner.backend().context_session_liveness(row.id) == ContextSessionLiveness::Live;
            rejection = SettlementRejection::SettlementClassificationRejected;
            if !dormant && !live {
                return None;
            }
            let detached = app.state::<DetachedSessionsState>();
            if dormant {
                rejection = SettlementRejection::DetachedRegistryUnreadable;
                if detached.lock().ok()?.contains(&row.id) {
                    rejection = SettlementRejection::DetachedMember;
                    return None;
                }
            }
            rejection = SettlementRejection::ReplicaUnavailable;
            let retired_replica = if dormant {
                Some(CarrierPhysicalIdentity::from(
                    route.saved_replica()?.clone(),
                ))
            } else {
                None
            };
            let carrier = &owner.registration().carrier;
            rejection = SettlementRejection::PublicationStateUnavailable;
            try_with_agents_layer_publication_state(&carrier.directory, |suppressed| {
                if suppressed {
                    rejection = SettlementRejection::PublicationSuppressed;
                    return None;
                }
                let transition = if dormant {
                    CarrierCheckpointTransition::FinishDormant {
                        generation: binding.member.generation,
                        committed_version: binding.member.carrier_version,
                        replica: retired_replica.as_ref()?,
                    }
                } else {
                    CarrierCheckpointTransition::FinishClosing
                };
                rejection = SettlementRejection::ConfirmedCarrierRejected;
                with_confirmed_carrier(&permit, carrier, current, transition, |stamp| {
                    rejection = SettlementRejection::TrackerFinishRejected;
                    let mut identity = binding.member.clone();
                    identity.carrier_version = stamp.version;
                    tracker.publish_manager_facts(
                        identity,
                        !dormant,
                        is_working(row),
                        std::time::Instant::now(),
                    );
                    let outcome = if dormant {
                        ResponseCloseSettlementOutcome::Dormant {
                            exact_backend_absent: true,
                            all_effects_completed: true,
                            retained_exited_confirmed: true,
                            selection_settled: true,
                        }
                    } else {
                        ResponseCloseSettlementOutcome::ReopenAfterKillFailure {
                            terminal_kill_failure: true,
                            all_effects_terminal: true,
                            original_backend_alive: true,
                            original_manager_identity_intact: true,
                        }
                    };
                    let current_settlement = ResponseCloseCurrentSettlement {
                        current_carrier_version: stamp.version,
                        confirmed_carrier_version: stamp.version,
                        checkpoint_nonce: stamp.nonce,
                        current_checkpoint_nonce: stamp.nonce,
                        publication_epoch: stamp.epoch,
                        current_publication_epoch: stamp.epoch,
                    };
                    tracker
                        .finish_response_close(
                            owner.ticket(),
                            ResponseCloseSettlementEvidence {
                                committed_binding: binding.clone(),
                                current_settlement,
                                outcome,
                            },
                        )
                        .ok()?;
                    Some(if dormant {
                        ResponseCloseDisposition::Dormant
                    } else {
                        ResponseCloseDisposition::Reopened
                    })
                })
            })
            .ok()
            .flatten()
        })
        .await;
    drop(permit);
    if let Some(disposition) = settled {
        response_effect_owners(app)
            .remove(binding.member.session_id)
            .map_err(|_| {
                log_postfinish_cleanup(owner, &disposition, "owner_remove_unreadable");
                SettlementRejection::PostfinishCleanup
            })?;
        crate::session::selection::forget_response_close_registration(binding.member.session_id);

        if matches!(disposition, ResponseCloseDisposition::Dormant) {
            pty.lock()
                .map_err(|_| {
                    log_postfinish_cleanup(owner, &disposition, "pty_route_lock_unreadable");
                    SettlementRejection::PostfinishCleanup
                })?
                .remove_route_if_kind(binding.member.session_id, binding.member.backend);
        }
        return Ok(disposition);
    }
    Err(rejection)
}

pub(crate) async fn reconcile_response_closes<R: tauri::Runtime>(app: &AppHandle<R>) {
    let Some(tracker) = app.try_state::<crate::pty::manager::ResponseCloseState>() else {
        return;
    };
    let owners = response_effect_owners(app).snapshot();
    let Some(owners) = owners.filter(|owners| !owners.is_empty()) else {
        return;
    };
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    for owner in owners {
        if !owner.registration().tracker.same_state(&tracker) || !owner.terminal() {
            continue;
        }
        // Reconcile only the latest carrier, without repeating an observed
        // kill, group operation, or cleanup after a save/readback failure.
        if crate::config::sessions_persistence::persist_current_state_result(&manager)
            .await
            .is_ok()
        {
            let _ = settle_response_close(app, &manager, &owner).await;
        }
    }
}
