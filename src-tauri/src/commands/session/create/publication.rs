use super::admission::{CoordinatorCreateTicket, OwnedCreateAdmission, OwnedCreateTargetLease};
use super::contracts::CreateCompletion;
use crate::commands::session::lifecycle::LifecycleEntryAuthority;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::selection::SelectionTransaction;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

/// #1032 + #1171 - start sampling a freshly spawned session, with both engines.
///
/// **Sessions with no agent are never registered** with the scraper or the quota engine, and
/// that rule lives HERE, once: a plain shell costs neither anything, ever. #2879 - the watcher
/// engine is asked BEFORE that rule, with the optional agent, because its internal activity
/// consumer may observe a Codex session that has no configured agent; it registers nothing
/// when there is neither an agent nor a consumer. `try_state` and not `state`, because
/// `state` panics when unmanaged and a test app manages neither engine - an absent engine is
/// simply the feature being off.
///
/// There is no race with the screen parser: `PtyManager::spawn` has already returned `Ok` at
/// the call site and the parser is registered during the spawn. The scraper's first sample is
/// 5 s away and the first watcher tick is 200 ms away, and neither can arrive before the
/// parser exists.
pub(crate) fn register_session_samplers<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: Uuid,
    agent_id: Option<String>,
) {
    if let Some(watchers) = app.try_state::<Arc<crate::pty::watchers::WatcherEngine>>() {
        watchers.register_session(id, agent_id.clone());
    }
    let Some(agent_id) = agent_id else {
        return;
    };
    if let Some(scraper) = app.try_state::<Arc<crate::pty::context_scrape::ContextScraper>>() {
        scraper.register_session(id, agent_id.clone());
    }
    if let Some(quota) = app.try_state::<Arc<crate::pty::agent_quota::AgentQuotaEngine>>() {
        quota.register_session(id, agent_id);
    }
}

/// #1171 - purge every per-session side structure a DESTROYED session leaves behind.
///
/// One helper with two call sites, because the per-session side-state purge is already
/// duplicated in this file: the destroy loop and `publish_restart_destroyed` are parallel
/// copies of each other, and adding a third thing to purge to only one of them is how a leak
/// gets written.
///
/// **The order is load-bearing.** The engine is retired FIRST, so no tick still in flight can
/// republish this session's status and recreate the entry step 2 removes. `WatcherHistory`
/// creates entries only from the engine's publish, and the engine publishes only while holding
/// its registration lock, so once `retire_session` returns nothing can bring the entry back.
///
/// **Destroyed sessions only.** A session that exits on its own is NOT destroyed: the engine
/// stops sampling it and its buffer stays, which is the case that matters - an API error or a
/// CLI crash is exactly when the evidence is worth keeping. Root-agent sessions retained as
/// `Exited` keep theirs too, because their row is still in the list.
pub(crate) fn purge_session_side_state<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    if let Some(quota) = app.try_state::<Arc<crate::pty::agent_quota::AgentQuotaEngine>>() {
        quota.retire_session(session_id);
    }
    if let Some(watchers) = app.try_state::<Arc<crate::pty::watchers::WatcherEngine>>() {
        watchers.retire_session(session_id);
    }
    if let Some(history) = app.try_state::<crate::pty::watchers::history::WatcherHistoryState>() {
        history.purge(session_id);
    }
    reset_substantive_input(app, session_id);
}

/// #871 - clear a session's substantive-input marker. (#2336) Also clears the
/// session's typing-hold entry, so the destroy and restart teardown paths drop
/// the manual padlock, the natural clock and every counted held ID with the
/// tracker they already reset.
///
/// Extracted by #1171 so it has ONE definition rather than the two inline copies the destroy
/// and restart cleanups each carried. It is a map removal, so calling it twice for the same
/// session is a no-op and not a second effect - which is what lets the destroy path keep
/// resetting every id it publishes, retained-exited ones included, while the destroyed-only
/// purge below also resets the ones it owns.
pub(crate) fn reset_substantive_input<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Uuid) {
    if let Some(activity) = app.try_state::<crate::pty::input_activity::SubstantiveInputState>() {
        activity
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .reset(session_id);
    }
    if let Some(hold) = app.try_state::<crate::pty::input_activity::TypingHoldState>() {
        hold.lock()
            .unwrap_or_else(|error| error.into_inner())
            .reset(session_id);
    }
}

pub(crate) async fn rollback_pre_created_session<R: tauri::Runtime>(
    _app: &AppHandle<R>,
    _session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    _pty_mgr: &Arc<Mutex<PtyManager>>,
    id: Uuid,
    reason: &str,
) {
    log::warn!(
        "[session] Rolling back pre-created session {} after setup failure: {}",
        id,
        reason
    );
    // The reserved create ticket or inline transaction owns the one common
    // backend-kind-aware rollback after this setup body returns. Killing here
    // would race that finalizer and perform the same teardown twice.
}

pub(crate) fn resolve_create_destination<R: tauri::Runtime>(
    cwd: &str,
    lifecycle_domain: Option<&LifecycleEntryAuthority>,
    preheld_target: Option<&crate::api::message_store::PtyInputTargetOwnership<'_>>,
    inline_transaction: Option<&SelectionTransaction<R>>,
) -> Result<
    (
        Option<String>,
        Option<crate::path_identity::VerifiedPathIdentity>,
    ),
    String,
> {
    let destination_proof = (|| {
        let cwd_path = std::path::Path::new(cwd);
        let create_target_key = crate::config::teams::pty_input_create_gate_key_from_cwd(cwd_path)
            .map_err(|_| "targetCreateGateUnavailable".to_string())?;
        let create_target_replica_identity = if create_target_key.is_some() {
            let anchor = crate::config::teams::strict_wg_replica_anchor_from_cwd(cwd_path)
                .map_err(|_| "targetCreateGateUnavailable".to_string())?
                .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
            Some(
                crate::path_identity::verify_directory(&anchor)
                    .map_err(|_| "targetCreateGateUnavailable".to_string())?,
            )
        } else {
            None
        };
        if let Some(domain) = lifecycle_domain {
            if !domain.validates_destination(cwd, create_target_key.as_deref()) {
                return Err("invalidTargetCreateOwnership".to_string());
            }
        }
        if let Some(preheld) = preheld_target {
            let target_key = create_target_key
                .as_deref()
                .ok_or_else(|| "invalidTargetCreateOwnership".to_string())?;
            if !preheld.proves(target_key) {
                return Err("invalidTargetCreateOwnership".to_string());
            }
        }
        Ok::<_, String>((create_target_key, create_target_replica_identity))
    })();
    match destination_proof {
        Ok(proof) => Ok(proof),
        Err(error) => {
            if let Some(domain) = lifecycle_domain {
                domain
                    .interrupted
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            if let Some(transaction) = inline_transaction {
                transaction.invalidate_restore_ownership();
            }
            Err(error)
        }
    }
}

pub(crate) async fn acquire_create_target_lease<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    target_key: Option<&str>,
    replica: Option<&crate::path_identity::VerifiedPathIdentity>,
    has_preheld_target: bool,
    has_lifecycle_domain: bool,
) -> Result<Option<OwnedCreateTargetLease>, String> {
    Ok(if !has_preheld_target && !has_lifecycle_domain {
        if let Some(target_key) = target_key {
            let gate = app
                .try_state::<crate::api::message_store::PtyInputTargetGateState>()
                .and_then(|state| state.gate.as_ref().ok().map(Arc::clone))
                .or_else(|| {
                    app.try_state::<crate::api::message_store::MessageStoreState>()
                        .and_then(|state| state.target_gate.as_ref().ok().map(Arc::clone))
                })
                .ok_or_else(|| "targetCreateGateUnavailable".to_string())?;
            let stripe = gate
                .acquire_target_lock(target_key)
                .await
                .map_err(|_| "targetCreateGateUnavailable".to_string())?;
            let exact = gate.acquire_exact(target_key).await;
            Some(OwnedCreateTargetLease {
                key: target_key.to_owned(),
                gate,
                replica: replica
                    .cloned()
                    .ok_or_else(|| "targetCreateGateUnavailable".to_string())?,
                guards: Some((exact, stripe)),
            })
        } else {
            None
        }
    } else {
        None
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "P05 retains distinct result, marker, admission and original cleanup ownership \
              without a context bundle."
)]
pub(crate) async fn finish_create_core_result<R: tauri::Runtime>(
    marker: Option<CoordinatorCreateTicket<R>>,
    mut result: Result<CreateCompletion, String>,
    owned_replacement: Option<&crate::session::selection::OwnedReplacementContinuation>,
    create_owner: &Arc<Mutex<OwnedCreateAdmission>>,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: &Arc<Mutex<PtyManager>>,
    has_preheld_target: bool,
    inline_pending_binding: &Mutex<Option<crate::session::manager::PendingCreateBinding>>,
    inline_transaction: Option<&SelectionTransaction<R>>,
) -> Result<CreateCompletion, String> {
    if let Some(t) = marker {
        if result.is_err() {
            t.failed();
        } else if matches!(
            &result,
            Ok(CreateCompletion::Finalized(_)) | Ok(CreateCompletion::Deferred(_))
        ) {
            t.succeeded();
        }
    }
    if let (Err(error), Some(continuation)) = (&result, owned_replacement) {
        let waiting = continuation.source_stop().is_none()
            && create_owner
                .lock()
                .ok()
                .is_some_and(|owner| owner.blocked_bound);
        if !waiting {
            let manager = session_mgr.read().await.clone();
            continuation
                .rollback_failed_create(&manager, pty_mgr, error)
                .await;
        }
    }
    if let Some(continuation) = owned_replacement {
        if continuation.source_stop().is_some() {
            // Validation failure is not a setup Err or whole-source stopped
            // proof. Retain the real pending owner and its work.
            result = Ok(CreateCompletion::Retained);
        } else if create_owner
            .lock()
            .ok()
            .is_some_and(|owner| owner.blocked_bound)
        {
            result = Ok(CreateCompletion::Retained);
        }
    }
    if has_preheld_target
        && create_owner
            .lock()
            .ok()
            .is_some_and(|owner| owner.blocked_bound)
    {
        result = Ok(CreateCompletion::Retained);
    }
    if result.is_err() && owned_replacement.is_none() {
        let binding = {
            inline_pending_binding
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
        };
        if let (Some(transaction), Some(binding)) = (inline_transaction, binding) {
            transaction.rollback_inline_create(binding).await;
        }
    }
    result
}
