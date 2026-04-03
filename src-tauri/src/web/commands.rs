use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

use super::broadcast::WsBroadcaster;

/// Shared state passed to the WS command dispatcher.
#[derive(Clone)]
pub struct WsState {
    pub session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pub pty_mgr: Arc<Mutex<PtyManager>>,
    pub settings: SettingsState,
    pub broadcaster: WsBroadcaster,
    pub app_handle: tauri::AppHandle,
}

/// Dispatch a WebSocket JSON command and return the result as JSON.
/// Format: { "id": N, "cmd": "command_name", "args": { ... } }
/// Returns: { "id": N, "result": ... } or { "id": N, "error": "..." }
pub async fn dispatch(state: &WsState, id: u64, cmd: &str, args: &Value) -> Value {
    match dispatch_inner(state, cmd, args).await {
        Ok(result) => json!({ "id": id, "result": result }),
        Err(e) => json!({ "id": id, "error": e }),
    }
}

async fn dispatch_inner(state: &WsState, cmd: &str, args: &Value) -> Result<Value, String> {
    match cmd {
        // --- Session commands ---
        "list_sessions" => {
            let mgr = state.session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            serde_json::to_value(sessions).map_err(|e| e.to_string())
        }

        "get_active_session" => {
            let mgr = state.session_mgr.read().await;
            let active = mgr.get_active().await.map(|id| id.to_string());
            Ok(json!(active))
        }

        "create_session" => {
            let cfg = state.settings.read().await;
            let shell = str_or(args, "shell", &cfg.default_shell);
            let shell_args = str_vec_or(args, "shellArgs", &cfg.default_shell_args);
            let cwd = str_or(args, "cwd", &dirs::home_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "C:\\".to_string()));
            let session_name = args.get("sessionName").and_then(|v| v.as_str()).map(String::from);
            let agent_id = args.get("agentId").and_then(|v| v.as_str()).map(String::from);
            drop(cfg);

            let info = crate::commands::session::create_session_inner(
                &state.app_handle,
                &state.session_mgr,
                &state.pty_mgr,
                shell,
                shell_args,
                cwd,
                session_name,
                agent_id,
                None, // agent_label (auto-detected)
                false, // skip_tooling_save
                false, // not a restore
            ).await?;

            serde_json::to_value(info).map_err(|e| e.to_string())
        }

        "destroy_session" => {
            let id = require_str(args, "id")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

            state.pty_mgr.lock().unwrap().kill(uuid).map_err(|e| e.to_string())?;

            let mgr = state.session_mgr.read().await;
            let new_active = mgr.destroy_session(uuid).await.map_err(|e| e.to_string())?;

            broadcast_all(&state.app_handle, &state.broadcaster, "session_destroyed", &json!({ "id": id }));

            if let Some(new_id) = new_active {
                broadcast_all(&state.app_handle, &state.broadcaster, "session_switched", &json!({ "id": new_id.to_string() }));
            }

            Ok(json!(null))
        }

        "switch_session" => {
            let id = require_str(args, "id")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

            let mgr = state.session_mgr.read().await;
            mgr.switch_session(uuid).await.map_err(|e| e.to_string())?;

            broadcast_all(&state.app_handle, &state.broadcaster, "session_switched", &json!({ "id": id }));

            Ok(json!(null))
        }

        "rename_session" => {
            let id = require_str(args, "id")?;
            let name = require_str(args, "name")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

            let mgr = state.session_mgr.read().await;
            mgr.rename_session(uuid, name.clone()).await.map_err(|e| e.to_string())?;

            broadcast_all(&state.app_handle, &state.broadcaster, "session_renamed", &json!({ "id": id, "name": name }));

            Ok(json!(null))
        }

        "set_last_prompt" => {
            let id = require_str(args, "id")?;
            let text = require_str(args, "text")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

            let mgr = state.session_mgr.read().await;
            mgr.set_last_prompt(uuid, text.clone()).await;

            broadcast_all(&state.app_handle, &state.broadcaster, "last_prompt", &json!({ "sessionId": id, "text": text }));

            Ok(json!(null))
        }

        // --- PTY commands ---
        "pty_resize" => {
            let session_id = require_str(args, "sessionId")?;
            let cols = args.get("cols").and_then(|v| v.as_u64()).unwrap_or(120) as u16;
            let rows = args.get("rows").and_then(|v| v.as_u64()).unwrap_or(30) as u16;
            let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

            state.pty_mgr.lock().unwrap().resize(uuid, cols, rows).map_err(|e| e.to_string())?;

            Ok(json!(null))
        }

        // pty_write is handled via binary frames, not JSON commands
        "pty_write" => {
            let session_id = require_str(args, "sessionId")?;
            let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
            let data: Vec<u8> = args.get("data")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect())
                .unwrap_or_default();

            state.pty_mgr.lock().unwrap().write(uuid, &data).map_err(|e| e.to_string())?;

            Ok(json!(null))
        }

        // --- Settings ---
        "get_settings" => {
            let cfg = state.settings.read().await;
            serde_json::to_value(&*cfg).map_err(|e| e.to_string())
        }

        "update_settings" => {
            let new_settings: crate::config::settings::AppSettings = serde_json::from_value(
                args.get("newSettings").cloned().unwrap_or(args.clone())
            ).map_err(|e| e.to_string())?;

            let mut cfg = state.settings.write().await;
            *cfg = new_settings.clone();
            drop(cfg);
            crate::config::settings::save_settings(&new_settings).map_err(|e| e.to_string())?;

            Ok(json!(null))
        }

        // --- Screen replay for late-joining clients ---
        "subscribe_session" => {
            let session_id = require_str(args, "sessionId")?;
            let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

            let pty_mgr = state.pty_mgr.lock().unwrap();
            let snapshot = pty_mgr.get_screen_snapshot(uuid);
            let size = pty_mgr.get_pty_size(uuid);
            drop(pty_mgr);

            if let Some(data) = snapshot {
                state.broadcaster.broadcast_pty_output(&session_id, &data);
            }

            match size {
                Some((rows, cols)) => Ok(json!({ "rows": rows, "cols": cols })),
                None => Ok(json!(null)),
            }
        }

        "get_pty_size" => {
            let session_id = require_str(args, "sessionId")?;
            let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

            let size = state.pty_mgr.lock().unwrap().get_pty_size(uuid);
            match size {
                Some((rows, cols)) => Ok(json!({ "rows": rows, "cols": cols })),
                None => Err(format!("Session not found: {}", session_id)),
            }
        }

        // --- Window commands (no-ops for web clients) ---
        "detach_terminal" | "close_detached_terminal" | "open_in_explorer"
        | "ensure_terminal_window" | "open_guide_window" => {
            Ok(json!(null))
        }

        // --- Repos ---
        "search_repos" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let cfg = state.settings.read().await;
            let repo_paths = cfg.repo_paths.clone();
            drop(cfg);

            // Re-use the Tauri command via invoke on the app handle
            // Since search_repos needs State<>, we call it through the repo scanning logic directly
            let query_lower = query.to_lowercase();
            let mut results: Vec<crate::commands::repos::RepoMatch> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for base_path in &repo_paths {
                let base = std::path::Path::new(base_path);
                if !base.is_dir() { continue; }
                crate::commands::repos::try_add_repo(base, &query_lower, &mut seen, &mut results);
                if let Ok(entries) = std::fs::read_dir(base) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                if !name.starts_with('.') {
                                    crate::commands::repos::try_add_repo(&path, &query_lower, &mut seen, &mut results);
                                }
                            }
                        }
                    }
                }
            }
            results.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            serde_json::to_value(results).map_err(|e| e.to_string())
        }

        // --- Debug ---
        "save_debug_logs" => {
            let content = require_str(args, "content")?;
            let path = crate::config::config_dir()
                .ok_or("No config dir")?
                .join("debug-logs.txt");
            tokio::fs::write(&path, &content)
                .await
                .map_err(|e| format!("Failed to write logs: {}", e))?;
            Ok(json!(null))
        }

        _ => Err(format!("Unknown command: {}", cmd)),
    }
}

/// Emit event to both Tauri windows and WebSocket clients.
pub fn broadcast_all(app: &tauri::AppHandle, broadcaster: &WsBroadcaster, event: &str, payload: &Value) {
    let _ = tauri::Emitter::emit(app, event, payload.clone());
    broadcaster.broadcast_event(event, payload);
}

// --- Arg helpers ---

fn require_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| format!("Missing required field: {}", key))
}

fn str_or(args: &Value, key: &str, default: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| default.to_string())
}

fn str_vec_or(args: &Value, key: &str, default: &[String]) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_else(|| default.to_vec())
}
