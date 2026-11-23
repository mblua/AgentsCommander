//! Tauri command `set_replica_unleashed`, the UI-only writer of the Unleash flag.
//! Holds no state; persistence is delegated to `crate::config::unleashed_replicas`.
//! The write runs on a blocking task. Not exposed to the CLI or the web dispatcher.

// The only writer of the Unleash flag. UI-only by design: never add this command to
// the CLI (src-tauri/src/cli) or to the web dispatcher (src-tauri/src/web/commands.rs).
#[tauri::command]
pub async fn set_replica_unleashed(replica_path: String, unleashed: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::config::unleashed_replicas::set_unleashed(&replica_path, unleashed)
    })
    .await
    .map_err(|e| format!("unleashTaskFailed: {e}"))?
}
