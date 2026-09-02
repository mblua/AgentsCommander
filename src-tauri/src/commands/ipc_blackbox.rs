use std::sync::Arc;

use tauri::State;

use crate::loops::ipc_observer::{IpcObserver, StoredRecord};

/// #1652 - harvest the previous run's renderer black boxes into `app.log`.
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
