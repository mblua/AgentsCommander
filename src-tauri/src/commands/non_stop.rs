//! #777 Non-stop watchdog IPC command.
//!
//! The frontend pushes a full snapshot (one entry per project with an ACTIVE
//! Non-stop group) whenever session state, the groups config, or the project
//! list changes, plus a light keepalive. The backend `NonStopWatchdogState`
//! reconciles the snapshot; the `non_stop_watchdog` loop times + actuates.

use crate::loops::non_stop_watchdog::{NonStopReport, NonStopWatchdogState};
use tauri::State;

#[tauri::command]
pub async fn non_stop_report(
    state: State<'_, NonStopWatchdogState>,
    reports: Vec<NonStopReport>,
) -> Result<(), String> {
    state.ingest(reports).await;
    Ok(())
}
