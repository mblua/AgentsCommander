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
use crate::session::selection::SelectionSource;
use crate::session::selection::SelectionTransaction;
use crate::session::session::SessionCommunicationKind;
use crate::session::session::SessionStatus;
use crate::telegram::manager::TelegramBridgeState;
use crate::DetachedSessionsState;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) enum ResponseEffectState<T> {
    NotStarted,
    Running,
    Completed(Result<T, String>),
    Ambiguous,
}

pub(crate) struct ResponseEffectRecord {
    stages: Vec<(&'static str, ResponseEffectState<()>)>,
    group: Option<ResponseEffectState<crate::resource_monitor::ResourceKillResult>>,
    group_worker: Option<
        tokio::task::JoinHandle<Result<crate::resource_monitor::ResourceKillResult, String>>,
    >,
    sealed: bool,
}

pub(crate) struct ResponseEffectOwner {
    ticket: crate::pty::manager::ResponseCloseTicket,
    registration: crate::session::selection::ResponseCloseRegistration,
    backend: Arc<dyn crate::pty::backend::PtyBackend>,
    record: Mutex<ResponseEffectRecord>,
}

pub(crate) fn response_effect_owners(
) -> &'static Mutex<std::collections::HashMap<Uuid, Arc<ResponseEffectOwner>>> {
    static OWNERS: std::sync::OnceLock<
        Mutex<std::collections::HashMap<Uuid, Arc<ResponseEffectOwner>>>,
    > = std::sync::OnceLock::new();
    OWNERS.get_or_init(Default::default)
}

impl ResponseEffectOwner {
    fn start(&self, name: &'static str) -> Option<usize> {
        let mut record = self.record.lock().ok()?;
        if record.sealed || record.stages.len() == record.stages.capacity() {
            return None;
        }
        let index = record.stages.len();
        record.stages.push((name, ResponseEffectState::NotStarted));
        record.stages[index].1 = ResponseEffectState::Running;
        Some(index)
    }

    fn complete(&self, index: usize, outcome: Result<(), String>) -> Option<()> {
        let mut record = self.record.lock().ok()?;
        let (_, stage) = record.stages.get_mut(index)?;
        if !matches!(stage, ResponseEffectState::Running) {
            return None;
        }
        *stage = ResponseEffectState::Completed(outcome);
        Some(())
    }

    fn terminal(&self) -> bool {
        let Ok(record) = self.record.lock() else {
            return false;
        };
        record.sealed
            && record
                .stages
                .iter()
                .all(|(_, stage)| matches!(stage, ResponseEffectState::Completed(_)))
            && record
                .group
                .as_ref()
                .is_none_or(|stage| matches!(stage, ResponseEffectState::Completed(_)))
    }

    fn kill_failed(&self) -> bool {
        self.record.lock().ok().is_some_and(|record| {
            record.stages.iter().any(|(name, stage)| {
                *name == "kill" && matches!(stage, ResponseEffectState::Completed(Err(_)))
            })
        })
    }

    fn seal(&self) -> Option<()> {
        self.record.lock().ok()?.sealed = true;
        Some(())
    }
}

pub(crate) async fn response_group_effect(
    owner: &Arc<ResponseEffectOwner>,
    monitor: &Arc<ResourceMonitorState>,
) -> Option<()> {
    let id = owner.registration.member.session_id;
    if !monitor.has_registered_group(id) {
        return Some(());
    }
    {
        let mut record = owner.record.lock().ok()?;
        record.group = Some(ResponseEffectState::Running);
        let monitor = monitor.clone();
        owner.backend.publish_stop_witness(id, "session-destroy");
        record.group_worker = Some(tokio::task::spawn_blocking(move || {
            monitor.kill_group(
                id,
                crate::resource_monitor::ResourceKillReason::SessionDestroy,
            )
        }));
    }
    // The JoinHandle stays in the retained owner while it is polled. Losing
    // this waiter does not drop the only handle or fabricate a group outcome.
    std::future::poll_fn(|cx| {
        use std::future::Future;
        let Ok(mut record) = owner.record.lock() else {
            return std::task::Poll::Pending;
        };
        let Some(worker) = record.group_worker.as_mut() else {
            return std::task::Poll::Pending;
        };
        match std::pin::Pin::new(worker).poll(cx) {
            std::task::Poll::Pending => std::task::Poll::Pending,
            std::task::Poll::Ready(Ok(outcome)) => {
                if let Err(error) = &outcome {
                    log::warn!(
                        "[session] resource cleanup failed session={} source=ResponseClose: {}",
                        id,
                        error
                    );
                }
                record.group = Some(ResponseEffectState::Completed(outcome));
                let _ = record.group_worker.take();
                std::task::Poll::Ready(Some(()))
            }
            std::task::Poll::Ready(Err(error)) => {
                record.group = Some(ResponseEffectState::Ambiguous);
                let _ = record.group_worker.take();
                log::warn!(
                    "[session] resource cleanup task failed session={} source=ResponseClose: {}",
                    id,
                    error
                );
                std::task::Poll::Ready(None)
            }
        }
    })
    .await
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
    let member = &owner.registration.member;
    let coordinator = &owner.registration.coordinator;
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
                response_close_current_rows(current, &owner.registration)
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
            let carrier = &owner.registration.carrier;
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
                            .try_commit_response_close(&tracker, &owner.ticket, now)
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
    let owner = Arc::new(ResponseEffectOwner {
        ticket,
        registration,
        backend,
        record: Mutex::new(ResponseEffectRecord {
            stages: Vec::with_capacity(16),
            group: None,
            group_worker: None,
            sealed: false,
        }),
    });
    {
        let mut owners = response_effect_owners()
            .lock()
            .map_err(|_| "response_effect_owner_poisoned".to_owned())?;
        if owners.contains_key(&binding.member.session_id) {
            return Ok(ResponseCloseDisposition::ReconcilePending);
        }
        owners.insert(binding.member.session_id, owner.clone());
    }
    let manager = transaction.manager().await;
    if !commit_response_close(transaction.app(), &manager, &owner).await {
        response_effect_owners()
            .lock()
            .map_err(|_| "response_effect_owner_poisoned".to_owned())?
            .remove(&binding.member.session_id);
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
        .backend
        .kill(binding.member.session_id)
        .map_err(|error| error.to_string());
    let kill_error = kill_result.as_ref().err().cloned();
    if owner.complete(kill, kill_result).is_none() {
        return Ok(ResponseCloseDisposition::ReconcilePending);
    }
    if owner.backend.has_session(binding.member.session_id) {
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
    let id = owner.registration.member.session_id;
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

pub(crate) async fn settle_response_close<R: tauri::Runtime>(
    app: &AppHandle<R>,
    manager: &SessionManager,
    owner: &Arc<ResponseEffectOwner>,
) -> Option<ResponseCloseDisposition> {
    if !owner.terminal() {
        return None;
    }
    let binding = owner.ticket.committed_binding();
    let pty = app.state::<Arc<Mutex<PtyManager>>>().inner().clone();
    let route = PtyManager::snapshot_route_proof(&pty, binding.member.session_id).ok()?;
    let authority = PtyManager::authority_route_proof(&pty, binding.member.session_id).ok()?;
    if route.backend_kind() != SessionBackendKind::LocalProcess {
        return None;
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
    let settled = manager
        .with_current_carrier_snapshot(&permit, |current| {
            let row = current.rows.iter().find(|row| {
                row.id == binding.member.session_id
                    && row.token == binding.member.token
                    && row.backend_kind == SessionBackendKind::LocalProcess
                    && !current.pending_ids.contains(&row.id)
            })?;
            if !verify_directory(Path::new(&row.working_directory))
                .is_ok_and(|actual| same_object(&actual, route.saved_cwd()))
            {
                return None;
            }
            let _lifecycle = authority
                .lock_verified(
                    SessionBackendKind::LocalProcess,
                    route.saved_cwd(),
                    route.saved_replica(),
                )
                .ok()?;
            if tracker.generation(row.id) != Some(binding.member.generation) {
                return None;
            }
            let absent = !owner.backend.has_session(row.id);
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
                && owner.backend.context_session_liveness(row.id) == ContextSessionLiveness::Live;
            if !dormant && !live {
                return None;
            }
            let detached = app.state::<DetachedSessionsState>();
            if dormant && detached.lock().ok()?.contains(&row.id) {
                return None;
            }
            let retired_replica = if dormant {
                Some(CarrierPhysicalIdentity::from(
                    route.saved_replica()?.clone(),
                ))
            } else {
                None
            };
            let carrier = &owner.registration.carrier;
            try_with_agents_layer_publication_state(&carrier.directory, |suppressed| {
                if suppressed {
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
                with_confirmed_carrier(&permit, carrier, current, transition, |stamp| {
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
                            &owner.ticket,
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
        response_effect_owners()
            .lock()
            .ok()?
            .remove(&binding.member.session_id);
        crate::session::selection::forget_response_close_registration(binding.member.session_id);

        if matches!(disposition, ResponseCloseDisposition::Dormant) {
            pty.lock()
                .ok()?
                .remove_route_if_kind(binding.member.session_id, binding.member.backend);
        }
        return Some(disposition);
    }
    None
}

pub(crate) async fn reconcile_response_closes<R: tauri::Runtime>(app: &AppHandle<R>) {
    let Some(tracker) = app.try_state::<crate::pty::manager::ResponseCloseState>() else {
        return;
    };
    let owners = response_effect_owners()
        .lock()
        .ok()
        .map(|owners| owners.values().cloned().collect::<Vec<_>>());
    let Some(owners) = owners.filter(|owners| !owners.is_empty()) else {
        return;
    };
    let manager = app
        .state::<Arc<tokio::sync::RwLock<SessionManager>>>()
        .read()
        .await
        .clone();
    for owner in owners {
        if !owner.registration.tracker.same_state(&tracker) || !owner.terminal() {
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
