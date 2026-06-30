use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::session::SessionCommunicationKind;
use crate::voice::tracker::VoiceTrackingState;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyScreenSnapshotPayload {
    pub session_id: String,
    pub data: Vec<u8>,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
    pub sequence: u64,
}

#[tauri::command]
pub async fn pty_write(
    app: AppHandle,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    voice_tracker: State<'_, VoiceTrackingState>,
    session_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

    // Flag if user typed while voice recording is active for this session.
    // Lock scope closes before pty_mgr lock to avoid holding both.
    {
        let mut tracker = voice_tracker.lock().unwrap();
        if tracker.is_recording(uuid) {
            tracker.mark_typed(uuid);
        }
    }

    // Existing behavior: write keystrokes to the PTY FIRST (no await held here)
    // so input latency is unchanged; the #552 bookkeeping is additive and after.
    pty_mgr
        .lock()
        .unwrap()
        .write(uuid, &data)
        .map_err(|e| e.to_string())?;

    // #552 user input -> silence touch (+ badge reset if coordinator). Resolves
    // all state from `app`, so the same helper serves Telegram and web.
    note_user_message_to_session(&app, uuid).await;

    Ok(())
}

/// #552 Record a real user message to `session_id`: always reset the auto-close
/// silence clock; if the session is a coordinator, reset its badge clock and
/// emit `coordinator_clock_updated` (and clear any "auto-closed" marker).
/// Resolves all state from `app`, so every user-input surface (xterm `pty_write`,
/// Telegram inbound, web UI) can call it with just (app, uuid). Injection /
/// auto-resume MUST NOT call this (they are not user messages).
///
/// Generic over the Tauri runtime so callers holding either a concrete
/// `AppHandle` or a generic `AppHandle<R>` (e.g. the Telegram bridge) can reuse it.
pub(crate) async fn note_user_message_to_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Uuid,
) {
    // (a) auto-close silence: any user message keeps the team alive.
    if let Some(idle) = app.try_state::<Arc<crate::pty::idle_detector::IdleDetector>>() {
        idle.touch_silence(session_id);
    }

    // (#630/#631 + #698) Apply both user-input state transitions, then persist
    // once. On the FIRST real user message we re-arm the resume intent (so a
    // restarted-fresh session stays fresh until the user actually engages) and
    // clear any visible raise-hand communication. This is the unified user-input
    // choke point (xterm/Telegram/web); injection and auto-resume never call it,
    // and it runs before the coordinator-only early return below so non-
    // coordinator members re-arm too. Both mutations run before the single
    // persistence attempt so a snapshot can never capture a half-applied
    // transition (e.g. a still-raised hand). The clear event is emitted only
    // after persistence succeeds, so `list-sessions` and the UI agree.
    let (cleared_start_fresh, cleared_raise_hand, manager) = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let manager = {
            let guard = mgr.read().await;
            guard.clone()
        };
        let cleared_start_fresh = manager
            .clear_start_fresh_on_restore_if_set(session_id)
            .await;
        let cleared_raise_hand = manager
            .clear_communication_if_kind(session_id, SessionCommunicationKind::RaiseHand)
            .await;
        (cleared_start_fresh, cleared_raise_hand, manager)
    };

    let persisted_transitions = if cleared_start_fresh || cleared_raise_hand {
        match crate::config::sessions_persistence::persist_current_state_result(&manager).await {
            Ok(()) => true,
            Err(e) => {
                log::error!(
                    "Failed to persist user-input session state transitions: {}",
                    e
                );
                false
            }
        }
    } else {
        true
    };

    if cleared_raise_hand && persisted_transitions {
        let _ = app.emit(
            "session_communication_changed",
            serde_json::json!({ "sessionId": session_id.to_string(), "communication": null }),
        );
    }

    // (b) badge: reset only when the typed-to session is a coordinator.
    let cwd = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let cwd = mgr.read().await.coordinator_cwd(session_id).await;
        cwd
    };
    let Some(cwd) = cwd else { return };
    let Some(clocks) = app.try_state::<crate::config::coordinator_clocks::CoordinatorClocksState>()
    else {
        return;
    };

    // agent_fqn_from_path returns String (teams.rs:80), not Option.
    let fqn = crate::config::teams::agent_fqn_from_path(&cwd);
    let now = chrono::Utc::now();
    let (changed, cleared) = {
        let mut guard = clocks.lock().unwrap_or_else(|e| e.into_inner());
        let changed = guard.note_user_message(&fqn, now);
        // #552 a real user message reopens the coordinator -> clear any
        // "auto-closed" marker (idempotent; no-op if not marked).
        let cleared = guard.clear_auto_closed(&fqn);
        (changed, cleared)
    };
    if changed {
        let _ = app.emit(
            "coordinator_clock_updated",
            serde_json::json!({ "replicaPath": cwd, "lastUserMessageAt": now.to_rfc3339() }),
        );
    }
    if cleared {
        let _ = app.emit(
            "coordinator_auto_close_changed",
            serde_json::json!({ "replicaPath": cwd, "autoClosedAt": null }),
        );
    }
}

#[tauri::command]
pub fn pty_resize(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    pty_mgr
        .lock()
        .unwrap()
        .resize(uuid, cols, rows)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_screen_snapshot(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    session_id: String,
) -> Result<Option<PtyScreenSnapshotPayload>, String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

    let snapshot = {
        let pty_mgr = pty_mgr
            .lock()
            .map_err(|_| "PtyManager lock poisoned".to_string())?;
        pty_mgr.get_screen_snapshot(uuid)
    };

    let Some(snapshot) = snapshot else {
        return Ok(None);
    };

    Ok(Some(PtyScreenSnapshotPayload {
        session_id,
        data: snapshot.data,
        rows: Some(snapshot.rows),
        cols: Some(snapshot.cols),
        sequence: snapshot.sequence,
    }))
}
