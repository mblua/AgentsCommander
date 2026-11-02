use super::super::co_managed::raise_room_reader_demand;
use super::super::create::{purge_session_side_state, CreateCompletion, DeferredCreateOutput};
use super::super::telegram::{
    attach_local_config_telegram_if_any, attach_persisted_telegram_if_configured,
};
use super::context::LifecycleExecutionContext;
use crate::app_state::DetachedSessionsState;
use crate::pty::manager::PtyManager;
use crate::resource_monitor::ResourceMonitorState;
use crate::session::manager::{CommitDecision, LifecycleMutations};
use crate::session::selection::{
    SelectionCause, SelectionSource, SelectionTransaction, TrustedRestartIntent,
};
use crate::session::session::SessionInfo;
use crate::telegram::manager::TelegramBridgeState;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use uuid::Uuid;

pub(crate) async fn accept_restart_deferred_completion<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    completion: Result<CreateCompletion, String>,
    context: &LifecycleExecutionContext<'_>,
    uuid: uuid::Uuid,
    old_lost: bool,
    old_selected: bool,
    intent: TrustedRestartIntent,
) -> Result<DeferredCreateOutput, String> {
    Ok(match completion {
        Ok(CreateCompletion::Deferred(deferred)) => deferred,
        Ok(CreateCompletion::Retained) => {
            if let Some(authority) = context.authority() {
                authority
                    .interrupted
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            return Err(
                crate::session::selection::SelectionCoordinatorError::Unavailable.to_string(),
            );
        }
        Ok(CreateCompletion::Finalized(_)) => {
            let error = "restart replacement was published before atomic finalization".to_string();
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(error);
        }
        Err(error) => {
            if context.authority().is_some_and(|authority| {
                authority
                    .interrupted
                    .load(std::sync::atomic::Ordering::Acquire)
            }) {
                return Err(error);
            }
            if old_lost {
                if let Err(finalize_error) =
                    finalize_failed_restart(transaction, uuid, old_selected, intent).await
                {
                    return Err(format!(
                        "{}; restart failure finalization failed: {}",
                        error, finalize_error
                    ));
                }
            }
            return Err(error);
        }
    })
}

#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "Preserve restart settlement inputs for rollback, selection and prompt transfer"
)]
pub(crate) async fn finalize_restart_deferred<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    deferred: &DeferredCreateOutput,
    uuid: uuid::Uuid,
    old_lost: bool,
    old_selected: bool,
    activate_after: bool,
    intent: TrustedRestartIntent,
    restart_start_fresh: bool,
) -> Result<
    (
        uuid::Uuid,
        crate::session::manager::CommitResult,
        SessionInfo,
    ),
    String,
> {
    let new_uuid = match Uuid::parse_str(&deferred.info.id) {
        Ok(id) => id,
        Err(error) => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(format!(
                "restart replacement returned an invalid id: {error}"
            ));
        }
    };
    let live = match transaction.live_decision(new_uuid) {
        Some(CommitDecision::Live(live)) => live,
        _ => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err("restart replacement is not displayable".to_string());
        }
    };

    // 5. Finalize the replacement, remove the old row, and decide selection in
    // one manager commit. Neither lifecycle row was public before this point.
    let mut mutations = LifecycleMutations::default();
    mutations.finalize_live(deferred.binding, live);
    mutations.remove(uuid);
    if deferred.effective_continuation && !restart_start_fresh {
        let manager = transaction.manager().await;
        if let (Some(source), Some(target)) = (
            manager.get_session(uuid).await,
            manager.get_pending_session(deferred.binding).await,
        ) {
            if source.agent_id.is_some()
                && source.agent_id == target.agent_id
                && source.requested_profile == target.requested_profile
                && source.effective_profile == target.effective_profile
                && source.working_directory == target.working_directory
                && !source.start_fresh_on_restore
                && !target.start_fresh_on_restore
            {
                mutations.transfer_prompt(uuid, deferred.binding);
            }
        }
    }
    let decision = if old_selected || activate_after {
        CommitDecision::Live(live)
    } else {
        CommitDecision::Keep
    };
    let committed = match transaction
        .commit_entry_create(
            deferred.binding,
            decision,
            SelectionCause::Restart(intent),
            mutations,
        )
        .await
    {
        Ok(committed) => committed,
        Err(error) => {
            transaction.rollback_inline_create(deferred.binding).await;
            if old_lost {
                finalize_failed_restart(transaction, uuid, old_selected, intent).await?;
            }
            return Err(error);
        }
    };
    let session_info = committed
        .finalized_rows
        .iter()
        .find(|row| row.id == new_uuid.to_string())
        .cloned()
        .ok_or_else(|| "restart finalization did not produce the replacement row".to_string())?;
    Ok((new_uuid, committed, session_info))
}

pub(crate) async fn publish_restart_before_warnings<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    uuid: uuid::Uuid,
    new_uuid: uuid::Uuid,
    binding: crate::session::manager::PendingCreateBinding,
    committed: &crate::session::manager::CommitResult,
    session_info: &SessionInfo,
    old_lost: bool,
) -> Result<Option<crate::session::selection::LifecycleEffectGuard>, String> {
    let app = transaction.app();
    let publication = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::Publication);
    if transaction.has_lifecycle_entry() && publication.is_none() {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    // Section 5.2: the commit removed the old row. Detach its bridge (dormant
    // old) and sweep every old-id reader demand raised since teardown before
    // the replacement is published or attached; then evaluate the new uuid's
    // own Room eligibility, because the create-time raise was suppressed for
    // `SelectionCause::Restart(_)`. Exactly one demand per consumer, one reader
    // and one slot may exist.
    if !old_lost {
        detach_restart_telegram(transaction, uuid).await;
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&uuid);
    }
    crate::telegram::manager::release_all_reader_demands(app, uuid).await;
    raise_room_reader_demand(app, new_uuid).await;
    transaction.observe_entry_reader_demand(
        binding,
        crate::session::selection::EntryReaderDemandReceipt::Returned,
    );
    transaction
        .persist(SelectionSource::Restart, Some(new_uuid))
        .await;
    transaction.publish_created(session_info);
    publish_restart_destroyed(transaction, uuid);
    if let Some(selection) = committed.selection.as_ref() {
        transaction.publish_selection(selection);
    }
    Ok(publication)
}

pub(crate) async fn publish_restart_after_warnings<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    new_uuid: uuid::Uuid,
    session_info: SessionInfo,
    restart_start_fresh: bool,
    telegram_bot_id: Option<&str>,
    cwd: &str,
    publication: Option<crate::session::selection::LifecycleEffectGuard>,
) -> Result<SessionInfo, String> {
    let app = transaction.app();
    // (#756) C3: mirror the honored fresh intent into the coordinator clocks so
    // it survives record destruction (idle auto-close, manual close destroy the
    // record that carries start_fresh_on_restore). Self-gates on coordinators
    // inside the helper; root agents are never coordinators, so this is a no-op
    // for them. Self-handoff-and-switch (#668) respawns through this function
    // and inherits the mirror automatically.
    if restart_start_fresh {
        crate::commands::pty::write_start_fresh_mirror_for_session(app, new_uuid, true).await;
    }

    // 6. Re-attach Telegram bridge from live persisted intent, or fall back to repo config.
    if telegram_bot_id.is_some() {
        attach_persisted_telegram_if_configured(app, new_uuid, telegram_bot_id).await;
    } else {
        attach_local_config_telegram_if_any(app, new_uuid, cwd).await;
    }

    if let Some(publication) = publication {
        publication.complete(Ok(Some(session_info.id.clone())));
    }
    Ok(session_info)
}

pub(crate) struct RestartTeardownError {
    pub(crate) message: String,
    pub(crate) old_lost: bool,
}

pub(crate) async fn complete_owned_replacement_retirement<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) {
    crate::session::selection::forget_response_close_registration(session_id);
    detach_restart_telegram(transaction, session_id).await;
    crate::telegram::manager::release_all_reader_demands(transaction.app(), session_id).await;
    transaction
        .app()
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&session_id);
}

pub(crate) async fn detach_restart_telegram<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) {
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
        if let Err(error) = transaction.app().emit(
            "telegram_bridge_detached",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        ) {
            log::warn!(
                "[restart] telegram detach publication failed session={}: {}",
                session_id,
                error
            );
        }
        shutdown.spawn_wait_or_abort();
    }
}

pub(crate) async fn teardown_old_for_restart<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    old_was_dormant: bool,
) -> Result<bool, RestartTeardownError> {
    if !transaction.runtime_snapshot(session_id).has_pty {
        if old_was_dormant {
            return Ok(false);
        }
        transaction
            .app()
            .state::<DetachedSessionsState>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&session_id);
        detach_restart_telegram(transaction, session_id).await;
        // Section 5.2: the old runtime is confirmed gone, so its reader
        // demands are released before the replacement is created. A failure to
        // create the replacement therefore leaves no old reader behind.
        crate::telegram::manager::release_all_reader_demands(transaction.app(), session_id).await;
        return Ok(true);
    }

    let resource_monitor = transaction
        .app()
        .state::<Arc<ResourceMonitorState>>()
        .inner()
        .clone();
    if resource_monitor.has_registered_group(session_id) {
        transaction
            .app()
            .state::<Arc<Mutex<PtyManager>>>()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .publish_stop_witness(session_id, "session-restart");
        let monitor = Arc::clone(&resource_monitor);
        match tokio::task::spawn_blocking(move || {
            monitor.kill_group(
                session_id,
                crate::resource_monitor::ResourceKillReason::SessionDestroy,
            )
        })
        .await
        {
            Ok(Ok(result)) if result.quarantined => log::warn!(
                "[restart] resource cleanup quarantined session={}: {}",
                session_id,
                result.message
            ),
            Ok(Ok(_)) => {}
            Ok(Err(error)) => log::warn!(
                "[restart] resource cleanup failed session={}: {}",
                session_id,
                error
            ),
            Err(error) => log::warn!(
                "[restart] resource cleanup task failed session={}: {}",
                session_id,
                error
            ),
        }
    }

    // The std-Mutex guard from this `let` initializer is dropped at the `;` before
    // `runtime_snapshot` re-locks the same pty Mutex below. Moving this into an
    // `if let`/`match` scrutinee or an inner blocking block would hold the guard
    // across that re-lock and re-introduce the re-entrant std-Mutex deadlock fixed
    // in resource_monitor (§1295).
    let kill_result = transaction
        .app()
        .state::<Arc<Mutex<PtyManager>>>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .kill(session_id);
    let old_lost = !transaction.runtime_snapshot(session_id).has_pty;
    if !old_lost {
        let message = kill_result
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| "old PTY remained live after restart teardown".to_string());
        return Err(RestartTeardownError {
            message,
            old_lost: false,
        });
    }
    if let Err(error) = kill_result {
        log::warn!(
            "[restart] teardown reported an error after PTY loss session={}: {}",
            session_id,
            error
        );
    }
    transaction
        .app()
        .state::<DetachedSessionsState>()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&session_id);
    detach_restart_telegram(transaction, session_id).await;
    // Section 5.2: old runtime loss is confirmed, so the old id's reader
    // demands are released before the replacement is created.
    crate::telegram::manager::release_all_reader_demands(transaction.app(), session_id).await;
    Ok(true)
}

pub(crate) async fn finalize_failed_restart<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
    selected: bool,
    intent: TrustedRestartIntent,
) -> Result<(), String> {
    let publication = transaction
        .start_lifecycle_effect(crate::session::selection::LifecycleEffectSlot::Publication);
    if transaction.has_lifecycle_entry() && publication.is_none() {
        return Err(crate::session::selection::SelectionCoordinatorError::Unavailable.to_string());
    }
    let result = async {
        detach_restart_telegram(transaction, session_id).await;
        let mut mutations = LifecycleMutations::default();
        mutations.remove(session_id);
        let committed = transaction
            .commit(
                if selected {
                    CommitDecision::Clear
                } else {
                    CommitDecision::Keep
                },
                SelectionCause::Restart(intent),
                mutations,
            )
            .await?;
        // Section 5.2: the old row is gone, so any old-id reader demand that
        // survived teardown (or was raised while it was in flight) is released and
        // its capture slot closed before the destroyed publication.
        crate::telegram::manager::release_all_reader_demands(transaction.app(), session_id).await;
        transaction
            .persist(SelectionSource::Restart, Some(session_id))
            .await;
        publish_restart_destroyed(transaction, session_id);
        if let Some(selection) = committed.selection.as_ref() {
            transaction.publish_selection(selection);
        }
        Ok(())
    }
    .await;
    if let Some(publication) = publication {
        publication.complete(result.as_ref().map(|_| None).map_err(Clone::clone));
    }
    result
}

pub(crate) async fn accept_restart_teardown<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    teardown: Result<bool, RestartTeardownError>,
    uuid: uuid::Uuid,
    old_selected: bool,
    intent: TrustedRestartIntent,
) -> Result<bool, String> {
    match teardown {
        Ok(old_lost) => Ok(old_lost),
        Err(error) => {
            if error.old_lost {
                // Section 5.2: the old PTY is already gone, so its demands must
                // not outlive the failed restart.
                crate::telegram::manager::release_all_reader_demands(transaction.app(), uuid).await;
                if let Err(finalize_error) =
                    finalize_failed_restart(transaction, uuid, old_selected, intent).await
                {
                    return Err(format!(
                        "{}; restart failure finalization failed: {}",
                        error.message, finalize_error
                    ));
                }
            }
            Err(error.message)
        }
    }
}

/// #1171 - this covers BOTH restart paths: `execute_restart_transaction`'s success path and
/// `finalize_failed_restart`. A restart is a normal flow - it is in the invoke handler, the
/// mailbox calls it, and a bulk settings save can restart sessions - so without the purge here
/// every restart would orphan up to 500 ring entries under an id that is already gone from the
/// session list, unreachable from the UI.
fn publish_restart_destroyed<R: tauri::Runtime>(
    transaction: &SelectionTransaction<R>,
    session_id: Uuid,
) {
    transaction.publish_destroyed(session_id);
    let label = format!("terminal-{}", session_id.to_string().replace('-', ""));
    if let Some(window) = transaction.app().get_webview_window(&label) {
        if let Err(error) = window.destroy() {
            log::warn!(
                "[restart] detached window destroy failed session={}: {}",
                session_id,
                error
            );
        }
    }
    purge_session_side_state(transaction.app(), session_id);
}
