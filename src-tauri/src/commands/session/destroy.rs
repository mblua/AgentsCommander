use crate::commands::session::publish_legacy_destroy_outcome;
#[cfg(test)]
use crate::commands::session::restart_reader_seam;
use crate::pty::manager::PtyManager;
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::acquire_carrier_mutation_permit;
use crate::session::manager::CommitDecision;
use crate::session::manager::LifecycleMutations;
use crate::session::selection::CriticalAdmissionOutcome;
use crate::session::selection::SelectionCoordinator;
use crate::session::selection::SelectionTransaction;
use crate::session::session::SessionStatus;
use crate::telegram::manager::TelegramBridgeState;
use crate::DetachedSessionsState;
use std::sync::Arc;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use uuid::Uuid;
mod response_close;
#[cfg(test)]
pub(crate) use response_close::detach_response_close_telegram;
pub(crate) use response_close::execute_owned_response_close;
pub(crate) use response_close::execute_response_close_transaction;
pub(crate) use response_close::reconcile_response_closes;
pub(crate) use response_close::DestroyCompletion;

pub(crate) async fn destroy_one_legacy_session<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: &DestroyRequest,
    before: &crate::session::manager::ManagerAggregateSnapshot,
    session_id: uuid::Uuid,
    outcome: &mut DestroyOutcome,
    mutations: &mut LifecycleMutations,
    retained_marked: &mut std::collections::HashSet<uuid::Uuid>,
) {
    let Some(existing) = before
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
    else {
        outcome
            .failed
            .push((session_id, "Session not found".to_string()));
        return;
    };
    {
        let tracker = transaction
            .app()
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .response_close_state();
        let permit = acquire_carrier_mutation_permit().await;
        tracker.invalidate_manager_row(session_id, std::time::Instant::now());
        if !crate::config::sessions_persistence::retire_carrier_for_discard(&permit, session_id) {
            outcome
                .failed
                .push((session_id, "response_close_deferred".to_owned()));
            return;
        }
        crate::session::selection::forget_response_close_registration(session_id);
    }
    let is_root_agent = existing.is_root_agent
        || crate::config::root_agent::is_root_agent_path(&existing.working_directory);

    if let Err(error) =
        teardown_legacy_destroy_runtime(transaction, session_id, &request.source).await
    {
        outcome.failed.push((session_id, error));
        return;
    }

    #[cfg(test)]
    restart_reader_seam::hit_after_old_release(&format!("destroy:{session_id}")).await;

    if is_root_agent && !request.force_destroy_root {
        mutations.set_detached_intent(session_id, false);
        if !matches!(existing.status, SessionStatus::Exited(_)) {
            mutations.mark_exited(session_id, 0);
            retained_marked.insert(session_id);
        }
        outcome.retained_exited_ids.push(session_id);
    } else {
        mutations.remove(session_id);
        outcome.destroyed_ids.push(session_id);
    }
}

pub(crate) async fn teardown_legacy_destroy_runtime<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: uuid::Uuid,
    source: &DestructionSource,
) -> Result<(), String> {
    {
        let detached = transaction.app().state::<DetachedSessionsState>();
        detached
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
    }

    let bridge_shutdown = {
        let telegram = transaction.app().state::<TelegramBridgeState>();
        let mut telegram = telegram.lock().await;
        if telegram.has_bridge(session_id) {
            telegram.detach(session_id).ok()
        } else {
            None
        }
    };
    if let Some(shutdown) = bridge_shutdown {
        transaction
            .manager()
            .await
            .set_telegram_bot_id(session_id, None)
            .await;
        if let Err(error) = transaction.app().emit(
            "telegram_bridge_detached",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        ) {
            log::warn!(
                "[session] telegram detach publication failed session={} source={:?}: {}",
                session_id,
                source,
                error
            );
        }
        shutdown.spawn_wait_or_abort();
    }

    let resource_monitor = transaction
        .app()
        .state::<Arc<ResourceMonitorState>>()
        .inner()
        .clone();
    if resource_monitor.has_registered_group(session_id) {
        {
            let pty = transaction.app().state::<Arc<Mutex<PtyManager>>>();
            pty.lock()
                .unwrap_or_else(|error| error.into_inner())
                .publish_stop_witness(session_id, "session-destroy");
        }
        let monitor = Arc::clone(&resource_monitor);
        match tokio::task::spawn_blocking(move || {
            monitor.kill_group(
                session_id,
                crate::resource_monitor::ResourceKillReason::SessionDestroy,
            )
        })
        .await
        {
            Ok(Ok(result)) => {
                if result.quarantined {
                    log::warn!(
                        "[session] resource cleanup quarantined session={} source={:?}: {}",
                        session_id,
                        source,
                        result.message
                    );
                }
            }
            Ok(Err(error)) => log::warn!(
                "[session] resource cleanup failed session={} source={:?}: {}",
                session_id,
                source,
                error
            ),
            Err(error) => log::warn!(
                "[session] resource cleanup task failed session={} source={:?}: {}",
                session_id,
                source,
                error
            ),
        }
    }

    // The std-Mutex guard from this `let` initializer is dropped at the `;`
    // before `runtime_snapshot` re-locks the same pty Mutex below. Moving this
    // into an `if let`/`match` scrutinee or an inner blocking block would hold
    // the guard across that re-lock and re-introduce the re-entrant std-Mutex
    // deadlock fixed in resource_monitor (§1295).
    let kill_result = transaction
        .app()
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .kill(session_id);
    let still_has_pty = transaction.runtime_snapshot(session_id).has_pty;
    if let Err(error) = kill_result {
        if still_has_pty {
            return Err(error.to_string());
        }
        log::warn!(
            "[session] teardown reported an error after PTY loss session={} source={:?}: {}",
            session_id,
            source,
            error
        );
    }
    Ok(())
}

pub use crate::session::selection::DestructionSource;
pub(crate) use crate::session::selection::{DestroyOutcome, DestroyRequest};

pub async fn destroy_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    uuid: Uuid,
) -> Result<(), String> {
    destroy_sessions_with_source(app, vec![uuid], DestructionSource::ManualClose, false)
        .await?
        .into_single_result(uuid)
}

pub(crate) async fn background_destroy_session_inner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    uuid: Uuid,
) -> Result<(), String> {
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    match coordinator.background_destroy(uuid).await? {
        CriticalAdmissionOutcome::Completed(outcome) => outcome.into_single_result(uuid),
        CriticalAdmissionOutcome::AlreadyPending => Ok(()),
    }
}

pub(crate) async fn destroy_sessions_with_source<R: tauri::Runtime>(
    app: &AppHandle<R>,
    ids: Vec<Uuid>,
    source: DestructionSource,
    force_destroy_root: bool,
) -> Result<DestroyOutcome, String> {
    let coordinator = app
        .try_state::<SelectionCoordinator>()
        .ok_or_else(|| "selectionCoordinatorUnavailable".to_string())?;
    coordinator
        .destroy(DestroyRequest {
            ids,
            source,
            force_destroy_root,
        })
        .await
}

pub(crate) async fn execute_destroy_transaction<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: DestroyRequest,
) -> Result<DestroyOutcome, String> {
    match execute_destroy_transaction_common(transaction, request, None).await? {
        DestroyCompletion::Legacy(outcome) => Ok(outcome),
        DestroyCompletion::Response(_) => unreachable!("legacy destroy selected its original core"),
    }
}

pub(crate) async fn execute_destroy_transaction_common<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    request: DestroyRequest,
    response_ticket: Option<crate::pty::manager::ResponseCloseTicket>,
) -> Result<DestroyCompletion, String> {
    if let Some(ticket) = response_ticket {
        return execute_owned_response_close(transaction, ticket)
            .await
            .map(DestroyCompletion::Response);
    }
    let planned = request
        .ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let before = transaction.aggregate_snapshot().await;
    let selected_id = before.selection.id();
    let mut outcome = DestroyOutcome::default();
    let mut mutations = LifecycleMutations::default();
    let mut retained_marked = std::collections::HashSet::new();

    for session_id in request.ids.iter().copied() {
        destroy_one_legacy_session(
            transaction,
            &request,
            &before,
            session_id,
            &mut outcome,
            &mut mutations,
            &mut retained_marked,
        )
        .await;
    }

    let selected_was_torn_down = selected_id.is_some_and(|selected| outcome.succeeded(selected));
    let decision = if !selected_was_torn_down {
        CommitDecision::Keep
    } else {
        match request.source {
            DestructionSource::ManualClose => {
                let final_snapshot = transaction.aggregate_snapshot().await;
                let mut fallback = None;
                for candidate_id in &final_snapshot.order {
                    let Some(candidate) = final_snapshot
                        .sessions
                        .iter()
                        .find(|candidate| candidate.id == *candidate_id)
                    else {
                        continue;
                    };
                    if planned.contains(&candidate.id) {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty=false \
                             detached=false excluded=true reason=plannedForDestruction",
                            candidate.id,
                            candidate.status
                        );
                        continue;
                    }
                    if matches!(candidate.status, SessionStatus::Exited(_)) {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty=false \
                             detached=false excluded=false reason=exited",
                            candidate.id,
                            candidate.status
                        );
                        continue;
                    }
                    let runtime = transaction.runtime_snapshot(candidate.id);
                    if runtime.detached {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty={} detached=true \
                             excluded=false reason=detached",
                            candidate.id,
                            candidate.status,
                            runtime.has_pty
                        );
                        continue;
                    }
                    if !runtime.has_pty {
                        log::info!(
                            "[selection] fallback candidate={} status={:?} hasPty=false \
                             detached=false excluded=false reason=missingPty",
                            candidate.id,
                            candidate.status
                        );
                        continue;
                    }
                    fallback = transaction.live_decision(candidate.id);
                    if fallback.is_some() {
                        break;
                    }
                }
                fallback.unwrap_or(CommitDecision::Clear)
            }
            DestructionSource::AutoClose
            | DestructionSource::SpawnRollback
            | DestructionSource::BackgroundCleanup => CommitDecision::Clear,
        }
    };

    let committed = transaction
        .commit(decision, request.source.cause(), mutations)
        .await?;
    if !outcome.destroyed_ids.is_empty() || !outcome.retained_exited_ids.is_empty() {
        transaction
            .persist(request.source.selection_source(), selected_id)
            .await;
    }

    publish_legacy_destroy_outcome(
        transaction,
        &request.source,
        &outcome,
        &committed,
        &retained_marked,
    )
    .await;

    Ok(DestroyCompletion::Legacy(outcome))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoordinatorCloseOutcome {
    /// true if the close was performed; false if confirmation is required first.
    pub closed: bool,
    /// number of live, working (not waiting_for_input) team members; only
    /// meaningful when `closed == false`.
    pub working_count: usize,
}

/// #588 Count LIVE team members that are working (not waiting_for_input). Pure so
/// the working-gate rule is unit-testable without a live app, mirroring how
/// `auto_close.rs` extracts `team_is_closeable`. A member counts only when it is
/// both live (PTY-backed) and not waiting for input.
pub(crate) fn count_working_members(
    members: &[(bool /*live*/, bool /*waiting_for_input*/)],
) -> usize {
    members
        .iter()
        .filter(|(live, waiting)| *live && !*waiting)
        .count()
}

pub(crate) async fn execute_manual_coordinator_destroy<R: tauri::Runtime>(
    app: &AppHandle<R>,
    coordinator_id: Uuid,
    member_ids: Vec<Uuid>,
    cascade: bool,
) -> Result<DestroyOutcome, String> {
    let mut planned_ids = if cascade { member_ids } else { Vec::new() };
    planned_ids.push(coordinator_id);
    let outcome =
        destroy_sessions_with_source(app, planned_ids, DestructionSource::ManualClose, false)
            .await?;
    for (failed_id, error) in &outcome.failed {
        if *failed_id != coordinator_id {
            log::warn!(
                "[manual-close] member destroy {} failed: {}",
                &failed_id.to_string()[..8],
                error
            );
        }
    }
    outcome.clone().into_single_result(coordinator_id)?;
    Ok(outcome)
}
