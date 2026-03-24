use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::session::manager::SessionManager;
use crate::DetachedSessionsState;

/// Detach a session into its own terminal window.
/// Creates a new WebviewWindow locked to a specific session,
/// and switches the main terminal to a different session.
#[tauri::command]
pub async fn detach_terminal(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    detached: State<'_, DetachedSessionsState>,
    session_id: String,
) -> Result<String, String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let label = format!("terminal-{}", session_id.replace('-', ""));
    let url = format!(
        "index.html?window=terminal&sessionId={}&detached=true",
        session_id
    );

    // If window already exists, focus it instead of creating a new one
    if let Some(existing) = app.get_webview_window(&label) {
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(label);
    }

    // Register as detached
    {
        let mut detached_set = detached.lock().unwrap();
        detached_set.insert(uuid);
    }

    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/icon.png"))
        .expect("Failed to load app icon");

    WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(format!("Terminal [detached]"))
        .icon(icon)
        .map_err(|e| e.to_string())?
        .inner_size(900.0, 600.0)
        .min_inner_size(400.0, 300.0)
        .decorations(false)
        .zoom_hotkeys_enabled(true)
        .build()
        .map_err(|e| e.to_string())?;

    let _ = app.emit(
        "terminal_detached",
        serde_json::json!({ "sessionId": session_id, "windowLabel": label }),
    );

    // Switch main terminal away from the detached session.
    // Find another non-detached session to activate.
    let mgr = session_mgr.read().await;
    let sessions = mgr.list_sessions().await;

    let next_id = {
        let detached_set = detached.lock().unwrap();
        sessions
            .iter()
            .find(|s| {
                let sid = Uuid::parse_str(&s.id).ok();
                sid.map_or(false, |u| !detached_set.contains(&u))
            })
            .map(|s| s.id.clone())
    }; // MutexGuard dropped here

    if let Some(next_id) = next_id {
        let next_uuid = Uuid::parse_str(&next_id).map_err(|e| e.to_string())?;
        mgr.switch_session(next_uuid)
            .await
            .map_err(|e| e.to_string())?;
        let _ = app.emit(
            "session_switched",
            serde_json::json!({ "id": next_id }),
        );
    } else {
        // No other non-detached sessions - clear the main terminal
        let _ = app.emit(
            "session_switched",
            serde_json::json!({ "id": serde_json::Value::Null }),
        );
    }

    Ok(label)
}

/// Close a detached terminal window and return the session to the main terminal.
#[tauri::command]
pub async fn close_detached_terminal(
    app: AppHandle,
    detached: State<'_, DetachedSessionsState>,
    session_id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

    // Remove from detached set
    {
        let mut detached_set = detached.lock().unwrap();
        detached_set.remove(&uuid);
    }

    let label = format!("terminal-{}", session_id.replace('-', ""));
    if let Some(window) = app.get_webview_window(&label) {
        window.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}
