use tauri::State;

use crate::session::warnings::{SessionWarning, SessionWarningState};

#[tauri::command]
pub fn drain_session_warnings(
    state: State<'_, SessionWarningState>,
    session_id: Option<String>,
) -> Result<Vec<SessionWarning>, String> {
    let mut buffer = state
        .lock()
        .map_err(|err| format!("session warning buffer lock poisoned: {err}"))?;
    Ok(buffer.drain(session_id.as_deref()))
}
