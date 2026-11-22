//! Tauri command `drain_session_warnings`: drains buffered warnings, for one session or all.
//! The buffer is owned by `crate::session::warnings::SessionWarningState`, not by this module.
//! It does not create warnings; it only locks the buffer, drains it and reports a poisoned lock.

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
