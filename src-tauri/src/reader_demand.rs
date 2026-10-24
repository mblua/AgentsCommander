//! Room reader-demand re-raise tick (#2456).

#[cfg(test)]
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use crate::config::settings::SettingsState;
use crate::session::manager::SessionManager;
use tauri::Manager;

use crate::co_managed::CoManagedSupervisorHandle;
use crate::{capture, commands};

/// #2456 period of the Room reader-demand re-raise tick.
pub(crate) const READER_DEMAND_TICK_SECS: u64 = 5;

/// #2456 test-only count of the tick's calls into readiness resolution, per
/// session: a global counter would move under a concurrent test's tick.
#[cfg(test)]
pub(crate) static TICK_RAISE_CALLS: std::sync::LazyLock<Mutex<HashMap<uuid::Uuid, usize>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
pub(crate) fn tick_raise_calls(session_id: uuid::Uuid) -> usize {
    TICK_RAISE_CALLS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&session_id)
        .copied()
        .unwrap_or(0)
}

/// #2456 D5-j test-only count of the tick's post-raise rechecks, per session.
#[cfg(test)]
pub(crate) static RECHECK_CALLS: std::sync::LazyLock<Mutex<HashMap<uuid::Uuid, usize>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
pub(crate) fn tick_recheck_calls(session_id: uuid::Uuid) -> usize {
    RECHECK_CALLS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&session_id)
        .copied()
        .unwrap_or(0)
}

/// #2525 one reader-demand tick: sweep when Co-managed is unavailable
/// globally, otherwise the unchanged #2456 re-raise pass.
pub(crate) async fn reader_demand_pass<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if release_room_reader_demands_when_globally_off(app).await {
        return;
    }
    reraise_room_reader_demands(app).await;
}

/// #2525 release the Room demand of every live session when the global switch
/// is off; `true` iff it swept. #2756 U2a: an empty Jev API key no longer
/// sweeps, since the key is a `Ready` input, not a capture input. The release runs
/// for sessions holding nothing too: it still bumps the Room `DemandEpoch`,
/// which aborts a raise already in flight (D-e). Logs once per pass, and only
/// when a session actually held a Room demand, so a steady Off is silent.
pub(crate) async fn release_room_reader_demands_when_globally_off<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> bool {
    let Some(settings) = app.try_state::<SettingsState>() else {
        return false;
    };
    let enabled = match settings.current_read().await {
        Ok(guard) => guard.co_managed_enabled,
        Err(error) => {
            log::debug!("[co-managed] settings unavailable; reader demands retained: {error}");
            return false;
        }
    };
    if crate::config::co_managed::globally_capturable(enabled) {
        return false;
    }
    let session_ids: Vec<uuid::Uuid> = {
        let Some(manager) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() else {
            return false;
        };
        let guard = manager.read().await;
        guard
            .list_sessions()
            .await
            .into_iter()
            .filter_map(|s| uuid::Uuid::parse_str(&s.id).ok())
            .collect()
    };
    let mut held = 0usize;
    for session_id in session_ids {
        if commands::telegram::holds_room_reader_demand(app, session_id).await {
            held += 1;
        }
        commands::session::release_room_reader_demand(app, session_id).await;
    }
    if held > 0 {
        log::info!(
            "[co-managed] {held} reader demands released: Co-managed is unavailable globally"
        );
    }
    true
}

/// #2456 raise the Room reader demand for every live session that lacks one.
///
/// The create, restart and room-toggle raises each check readiness once, so a
/// room that becomes Ready later (key set, catalog created, `config.json`
/// edited) would keep no reader. The cheap in-memory gate runs first; only a
/// session without a Room demand pays for readiness resolution, which
/// `raise_room_reader_demand` owns. The tick releases only a demand it
/// installed in the same iteration and then found no longer Ready (D5-h). It
/// logs only for a session it raised and kept, so a steady state is silent.
/// The raise's `true` means "a reader is running afterwards", not "inserted".
pub(crate) async fn reraise_room_reader_demands<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let session_ids: Vec<uuid::Uuid> = {
        let Some(manager) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() else {
            return;
        };
        let guard = manager.read().await;
        guard
            .list_sessions()
            .await
            .into_iter()
            .filter_map(|s| uuid::Uuid::parse_str(&s.id).ok())
            .collect()
    };
    for session_id in session_ids {
        if commands::telegram::holds_room_reader_demand(app, session_id).await {
            continue;
        }
        #[cfg(test)]
        {
            *TICK_RAISE_CALLS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(session_id)
                .or_insert(0) += 1;
        }
        if !commands::session::raise_room_reader_demand(app, session_id).await {
            continue;
        }
        #[cfg(test)]
        {
            *RECHECK_CALLS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(session_id)
                .or_insert(0) += 1;
        }
        // D5-h: a disable landing while the raise resolved and installed has
        // already released, so undo the demand this iteration installed.
        if !room_still_capturable(app, session_id).await {
            commands::session::release_room_reader_demand(app, session_id).await;
            continue;
        }
        log::info!("[co-managed] reader demand re-raised [{session_id}]");
    }
}

/// #2456 D5-h re-resolve the session's room and capturability after a raise
/// (#2756 U2a: the same predicate the raise used).
pub(crate) async fn room_still_capturable<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: uuid::Uuid,
) -> bool {
    let session = {
        let manager = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let guard = manager.read().await;
        guard.get_session(session_id).await
    };
    let Some(session) = session else {
        return false;
    };
    let Some(room_root) =
        crate::config::co_managed::room_root_for_path(Path::new(&session.working_directory))
    else {
        return false;
    };
    matches!(
        crate::commands::session::co_managed_capturable_for_session(
            app,
            &room_root,
            &session_id.to_string()
        )
        .await,
        Ok(true)
    )
}

/// #2756 U2a: re-offer every candidate a not-ready trigger left pending, once
/// its room is `Ready`. Only marked sessions are visited. A mark whose slot is
/// gone, or whose `seq` or record no longer match the slot, is dropped; a
/// matching mark in a not-yet-Ready room waits for the next pass. The trigger
/// goes through the channel so this tick arm never runs a cycle inline, and it
/// is sent even to a busy session: the handler arms, and the next idle edge
/// runs the cycle.
pub(crate) async fn recovery_pass<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
) {
    let marked: Vec<String> = handle
        .lock_state()
        .pending_recovery
        .keys()
        .cloned()
        .collect();
    if marked.is_empty() {
        return;
    }
    let registry = app.try_state::<Arc<capture::registry::CaptureRegistry>>();
    for id in marked {
        let Some(slot) = registry.as_ref().and_then(|registry| registry.slot(&id)) else {
            handle.lock_state().pending_recovery.remove(&id);
            continue;
        };
        let current = slot.snapshot();
        let matches = {
            let state = handle.lock_state();
            let Some(mark) = state.pending_recovery.get(&id) else {
                continue;
            };
            mark.seq == current.seq
                && current
                    .value
                    .record()
                    .is_some_and(|record| Arc::ptr_eq(record, &mark.record))
        };
        let session_id = match uuid::Uuid::parse_str(&id) {
            Ok(session_id) if matches => session_id,
            _ => {
                handle.lock_state().pending_recovery.remove(&id);
                continue;
            }
        };
        let session = match app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() {
            Some(manager) => manager.read().await.get_session(session_id).await,
            None => None,
        };
        let Some(room_root) = session.and_then(|session| {
            crate::config::co_managed::room_root_for_path(Path::new(&session.working_directory))
        }) else {
            continue;
        };
        let ready = matches!(
            crate::commands::session::co_managed_effective_state_for_session(app, &room_root, &id)
                .await,
            Ok(crate::config::co_managed::CoManagedState::Ready)
        );
        if !ready {
            continue;
        }
        handle.lock_state().pending_recovery.remove(&id);
        log::info!("[co-managed] recovery emitted [{id}]: seq={}", current.seq);
        let _ = handle.notify_recovery(session_id);
    }
}

pub(crate) async fn watch_capture_slots<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
) {
    let Some(registry) = app.try_state::<Arc<capture::registry::CaptureRegistry>>() else {
        return;
    };
    let session_ids: Vec<uuid::Uuid> = {
        let Some(manager) = app.try_state::<Arc<tokio::sync::RwLock<SessionManager>>>() else {
            return;
        };
        let guard = manager.read().await;
        guard
            .list_sessions()
            .await
            .into_iter()
            .filter_map(|s| uuid::Uuid::parse_str(&s.id).ok())
            .collect()
    };
    for session_id in session_ids {
        let id = session_id.to_string();
        if handle.is_watching(&id) {
            continue;
        }
        let Some(slot) = registry.slot(&id) else {
            continue;
        };
        handle.mark_watching(&id);
        // Subscribe FIRST, then evaluate the slot's CURRENT value immediately:
        // `subscribe()` marks the existing state as seen, so without the
        // immediate trigger a candidate that arrived before discovery would
        // wait for the next transition. Subscribing first closes the race with
        // a record arriving between the two steps.
        let mut changes = slot.subscribe();
        let _ = handle.notify_slot_changed(session_id);
        let watcher_handle = handle.clone();
        tokio::spawn(async move {
            loop {
                if changes.changed().await.is_err() {
                    break;
                }
                if !watcher_handle.notify_slot_changed(session_id) {
                    break;
                }
            }
            watcher_handle.mark_unwatched(&id);
        });
    }
}
