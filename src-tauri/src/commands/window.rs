use tauri::{AppHandle, Emitter, Manager};

/// Detach a session into its own terminal window.
/// Creates a new WebviewWindow locked to a specific session.
#[tauri::command]
pub async fn detach_terminal(app: AppHandle, session_id: String) -> Result<String, String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

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

    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/icon.png"))
        .expect("Failed to load app icon");

    WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
        .title(format!("Terminal [detached]"))
        .icon(icon)
        .map_err(|e| e.to_string())?
        .inner_size(900.0, 600.0)
        .min_inner_size(400.0, 300.0)
        .decorations(false)
        .build()
        .map_err(|e| e.to_string())?;

    let _ = app.emit(
        "terminal_detached",
        serde_json::json!({ "sessionId": session_id, "windowLabel": label }),
    );

    Ok(label)
}

/// Close a detached terminal window.
#[tauri::command]
pub async fn close_detached_terminal(app: AppHandle, session_id: String) -> Result<(), String> {
    let label = format!("terminal-{}", session_id.replace('-', ""));
    if let Some(window) = app.get_webview_window(&label) {
        window.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}
