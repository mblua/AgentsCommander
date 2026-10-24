//! #777 Non-stop watchdog IPC command.
//!
//! The frontend pushes a full snapshot (one entry per project with an ACTIVE
//! Non-stop group) whenever session state, the groups config, or the project
//! list changes, plus a light keepalive. The backend `NonStopWatchdogState`
//! reconciles the snapshot; the `non_stop_watchdog` loop times + actuates.

use crate::loops::non_stop_watchdog::{
    emit_non_stop_alarm, NonStopAlarmPayload, NonStopReport, NonStopWatchdogState,
};
use tauri::State;

#[tauri::command]
pub async fn non_stop_report<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, NonStopWatchdogState>,
    reports: Vec<NonStopReport>,
) -> Result<(), String> {
    for path in state.ingest(reports).await {
        emit_non_stop_alarm(
            &app,
            NonStopAlarmPayload {
                project_path: path,
                group_name: String::new(),
                seconds: 0,
                action: "stop".to_string(),
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
