use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::session::SessionCommunicationKind;
use crate::voice::tracker::VoiceTrackingState;

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

    // (#630/#631) Re-arm resume intent on the FIRST real user message. This is
    // the unified user-input choke point (xterm/Telegram/web); injection and
    // auto-resume never call it, so a restarted-fresh session stays fresh until
    // the user actually engages. One-shot: persist only on the true->false flip.
    // MUST run before the coordinator-only early return below so non-coordinator
    // members re-arm too.
    {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let cleared = mgr
            .read()
            .await
            .clear_start_fresh_on_restore_if_set(session_id)
            .await;
        if cleared {
            let m = mgr.read().await;
            crate::config::sessions_persistence::persist_current_state(&m).await;
        }
    }

    let cleared_raise_hand = {
        let mgr = app.state::<Arc<tokio::sync::RwLock<crate::session::manager::SessionManager>>>();
        let manager = {
            let guard = mgr.read().await;
            guard.clone()
        };
        manager
            .clear_communication_if_kind(session_id, SessionCommunicationKind::RaiseHand)
            .await
    };
    if cleared_raise_hand {
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
