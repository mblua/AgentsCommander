//! Tauri commands `ipc_blackbox_report` (harvests renderer IPC black-box records) and
//! `ipc_blackbox_enabled` (whether the renderer instrument runs this process).
//! Holds only the boot snapshot `IpcBlackBoxActivation`; the report command forwards
//! records to the managed `IpcObserver` from `crate::loops::ipc_observer` and returns the
//! localStorage keys the caller must delete.
//! Parsing, logging into `app.log` and run filtering are done by `IpcObserver`, not here.

use std::sync::Arc;

use tauri::State;

use crate::loops::ipc_observer::{IpcObserver, StoredRecord};

/// Harvest the previous run's renderer black boxes into `app.log`.
///
/// Returns the localStorage keys the caller must delete: every key whose record
/// was from a previous run (and has now been logged) plus every key that could
/// not be parsed. Keys belonging to the CURRENT run are never returned, which is
/// what keeps a sibling window that is still writing from being harvested.
#[tauri::command]
pub async fn ipc_blackbox_report(
    observer: State<'_, Arc<IpcObserver>>,
    records: Vec<StoredRecord>,
) -> Result<Vec<String>, String> {
    Ok(observer.ingest_records(records))
}

/// The `ipcBlackBoxEnabled` value read once at boot. Never updated: a
/// settings save takes effect on the next start only.
pub struct IpcBlackBoxActivation {
    pub enabled: bool,
}

/// Whether this process runs the renderer black box. Every window reads the
/// same boot value, including windows opened after a settings save.
#[tauri::command]
pub fn ipc_blackbox_enabled(activation: State<'_, IpcBlackBoxActivation>) -> bool {
    activation.enabled
}
