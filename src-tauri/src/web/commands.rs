use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::Manager;
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
            let active = mgr.get_active().await;
            let active = if let Some(active_id) = active {
                let is_detached = {
                    let detached = state.app_handle.state::<crate::DetachedSessionsState>();
                    let set = detached.lock().unwrap();
                    set.contains(&active_id)
                };
                if is_detached {
                    mgr.clear_active_if(active_id).await;
                    None
                } else {
                    Some(active_id.to_string())
                }
            } else {
                None
            };
            Ok(json!(active))
        }

        "create_session" => {
            let cfg = state.settings.read().await;
            let cwd = str_or(
                args,
                "cwd",
                &dirs::home_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "C:\\".to_string()),
            );
            let cwd = crate::path_utils::normalize_windows_verbatim_path(&cwd);
            let session_name = args
                .get("sessionName")
                .and_then(|v| v.as_str())
                .map(String::from);
            let agent_id = args
                .get("agentId")
                .and_then(|v| v.as_str())
                .map(String::from);
            let requested_profile = args
                .get("requestedProfile")
                .and_then(|v| v.as_str())
                .map(String::from);
            let resolved_spawn = if let Some(aid) = agent_id.as_deref() {
                crate::commands::session::build_configured_agent_spawn_for_cwd(
                    &cfg,
                    aid,
                    &cwd,
                    requested_profile.as_deref(),
                )?
            } else {
                None
            };
            let (shell, shell_args, agent_label) = if let Some(spawn) = resolved_spawn.as_ref() {
                (
                    spawn.shell.clone(),
                    spawn.shell_args.clone(),
                    Some(spawn.trusted_agent_label.clone()),
                )
            } else {
                (
                    str_or(args, "shell", &cfg.default_shell),
                    str_vec_or(args, "shellArgs", &cfg.default_shell_args),
                    None,
                )
            };
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
                agent_label, // agent_label (auto-detected for legacy custom shell)
                false,       // skip_tooling_save
                Vec::new(),  // git_repos
                true,        // skip_auto_resume = true → fresh create, no `--continue` injection
                resolved_spawn,
            )
            .await?;

            serde_json::to_value(info).map_err(|e| e.to_string())
        }

        "destroy_session" => {
            let id = require_str(args, "id")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

            crate::commands::session::destroy_session_inner(&state.app_handle, uuid).await?;
            state
                .broadcaster
                .broadcast_event("session_destroyed", &json!({ "id": id }));

            let active = {
                let mgr = state.session_mgr.read().await;
                if let Some(active_id) = mgr.get_active().await {
                    let is_detached = {
                        let detached = state.app_handle.state::<crate::DetachedSessionsState>();
                        let set = detached.lock().unwrap();
                        set.contains(&active_id)
                    };
                    if is_detached {
                        mgr.clear_active_if(active_id).await;
                        None
                    } else {
                        Some(active_id.to_string())
                    }
                } else {
                    None
                }
            };
            if let Some(active_id) = active {
                state
                    .broadcaster
                    .broadcast_event("session_switched", &json!({ "id": active_id }));
            } else {
                state
                    .broadcaster
                    .broadcast_event("session_switched", &json!({ "id": Value::Null }));
            }

            Ok(json!(null))
        }

        "switch_session" => {
            let id = require_str(args, "id")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

            let is_detached = {
                let detached = state.app_handle.state::<crate::DetachedSessionsState>();
                let set = detached.lock().unwrap();
                set.contains(&uuid)
            };
            if is_detached {
                let mgr = state.session_mgr.read().await;
                mgr.clear_active_if(uuid).await;
                let label = format!("terminal-{}", id.replace('-', ""));
                if let Some(win) = state.app_handle.get_webview_window(&label) {
                    win.set_focus().map_err(|e| e.to_string())?;
                }
                return Ok(json!(null));
            }

            let mgr = state.session_mgr.read().await;
            mgr.switch_session(uuid).await.map_err(|e| e.to_string())?;

            broadcast_all(
                &state.app_handle,
                &state.broadcaster,
                "session_switched",
                &json!({ "id": id, "userInitiated": true }),
            );

            Ok(json!(null))
        }

        "rename_session" => {
            let id = require_str(args, "id")?;
            let name = require_str(args, "name")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

            let mgr = state.session_mgr.read().await;
            mgr.rename_session(uuid, name.clone())
                .await
                .map_err(|e| e.to_string())?;

            broadcast_all(
                &state.app_handle,
                &state.broadcaster,
                "session_renamed",
                &json!({ "id": id, "name": name }),
            );

            Ok(json!(null))
        }

        "set_last_prompt" => {
            let id = require_str(args, "id")?;
            let text = require_str(args, "text")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

            let mgr = state.session_mgr.read().await;
            mgr.set_last_prompt(uuid, text.clone()).await;

            broadcast_all(
                &state.app_handle,
                &state.broadcaster,
                "last_prompt",
                &json!({ "sessionId": id, "text": text }),
            );

            Ok(json!(null))
        }

        // --- PTY commands ---
        "pty_resize" => {
            let session_id = require_str(args, "sessionId")?;
            let cols = args.get("cols").and_then(|v| v.as_u64()).unwrap_or(120) as u16;
            let rows = args.get("rows").and_then(|v| v.as_u64()).unwrap_or(30) as u16;
            let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

            state
                .pty_mgr
                .lock()
                .unwrap()
                .resize(uuid, cols, rows)
                .map_err(|e| e.to_string())?;

            Ok(json!(null))
        }

        // pty_write is handled via binary frames, not JSON commands
        "pty_write" => {
            let session_id = require_str(args, "sessionId")?;
            let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
            let data: Vec<u8> = args
                .get("data")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_u64().map(|n| n as u8))
                        .collect()
                })
                .unwrap_or_default();

            state
                .pty_mgr
                .lock()
                .unwrap()
                .write(uuid, &data)
                .map_err(|e| e.to_string())?;

            // #552 web UI input is a real user message (resets badge + silence).
            crate::commands::pty::note_user_message_to_session(&state.app_handle, uuid).await;

            Ok(json!(null))
        }

        // --- Settings ---
        "get_settings" => {
            let cfg = state.settings.read().await;
            serde_json::to_value(&*cfg).map_err(|e| e.to_string())
        }

        "update_settings" => {
            let new_settings: crate::config::settings::AppSettings =
                serde_json::from_value(args.get("newSettings").cloned().unwrap_or(args.clone()))
                    .map_err(|e| e.to_string())?;

            let saved = crate::commands::config::persist_protected_settings_update(
                &state.settings,
                new_settings,
            )
            .await?;
            crate::commands::config::purge_sessions_after_settings_update(&saved).await;

            Ok(json!(null))
        }

        "save_settings_draft" => {
            let draft: crate::config::settings::AppSettings =
                serde_json::from_value(args.get("draft").cloned().unwrap_or(args.clone()))
                    .map_err(|e| e.to_string())?;

            let (saved, events) =
                crate::commands::config::persist_settings_draft_update(&state.settings, draft)
                    .await?;
            crate::commands::config::purge_sessions_after_settings_update(&saved).await;
            if events.profiles_changed {
                broadcast_all(
                    &state.app_handle,
                    &state.broadcaster,
                    "coding_agent_profiles_updated",
                    &json!({}),
                );
            }
            for agent_id in events.env_agent_ids {
                broadcast_all(
                    &state.app_handle,
                    &state.broadcaster,
                    "coding_agent_env_settings_updated",
                    &json!({ "agentId": agent_id }),
                );
            }

            Ok(json!(null))
        }

        // --- Role templates ---
        "list_role_templates" => {
            let snapshot = state.settings.read().await.clone();
            match crate::config::config_dir() {
                Some(config_dir) => serde_json::to_value(
                    crate::commands::role_templates::collect_role_templates(&snapshot, &config_dir),
                )
                .map_err(|e| e.to_string()),
                None => {
                    log::warn!(
                        "[role-templates] could not resolve config dir; returning no role templates"
                    );
                    Ok(json!([]))
                }
            }
        }

        "get_agency_templates_status" => {
            let status =
                tauri::async_runtime::spawn_blocking(crate::cli::agency_templates::status_cache)
                    .await
                    .map_err(|e| format!("Agency template status task failed: {}", e))??;
            serde_json::to_value(status).map_err(|e| e.to_string())
        }

        "update_agency_templates" => {
            let result = tauri::async_runtime::spawn_blocking(|| {
                crate::cli::agency_templates::update_cache(
                    crate::cli::agency_templates::AgencyTemplatesUpdateArgs::default_ui_update(),
                )
            })
            .await
            .map_err(|e| format!("Agency template update task failed: {}", e))??;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        // --- Screen replay for late-joining clients ---
        "subscribe_session" => {
            let session_id = require_str(args, "sessionId")?;
            let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

            let pty_mgr = state.pty_mgr.lock().unwrap();
            let snapshot = pty_mgr.get_screen_snapshot(uuid);
            let size = snapshot
                .as_ref()
                .map(|snapshot| (snapshot.rows, snapshot.cols))
                .or_else(|| pty_mgr.get_pty_size(uuid));
            drop(pty_mgr);

            if let Some(snapshot) = snapshot {
                state
                    .broadcaster
                    .broadcast_pty_output(&session_id, &snapshot.data);
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

        // --- Cross-window event broadcast (theme sync, etc.) ---
        "broadcast_event" => {
            let event = require_str(args, "event")?;
            let payload = args.get("payload").cloned().unwrap_or(json!(null));
            broadcast_all(&state.app_handle, &state.broadcaster, &event, &payload);
            Ok(json!(null))
        }

        "list_detached_sessions" => {
            let detached = state.app_handle.state::<crate::DetachedSessionsState>();
            let set = detached.lock().unwrap();
            Ok(json!(set.iter().map(|u| u.to_string()).collect::<Vec<_>>()))
        }

        // --- Window commands (no-ops for web clients) ---
        // Browser-remote clients don't have Tauri windows; these all return null.
        // 0.8.0: `close_detached_terminal` removed; `ensure_terminal_window`
        // renamed to `focus_main_window`; `attach_terminal`, `list_detached_sessions`,
        // `set_detached_geometry` added (plan §R.5 / §A2.10).
        "detach_terminal"
        | "attach_terminal"
        | "set_detached_geometry"
        | "open_in_explorer"
        | "focus_main_window"
        | "open_guide_window"
        | "open_external_url" => Ok(json!(null)),

        // Home screen Markdown fetch is Tauri-only for v1; browser mode is
        // out of scope (issue #164 §Constraints). The frontend renders this
        // error in the Home view's error state.
        "fetch_home_markdown" => Err("Home is not available in browser mode".to_string()),

        // --- Repos ---
        "search_repos" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cfg = state.settings.read().await;
            let repo_paths = cfg.project_paths.clone();
            drop(cfg);

            // Re-use the Tauri command via invoke on the app handle
            // Since search_repos needs State<>, we call it through the repo scanning logic directly
            let query_lower = query.to_lowercase();
            let mut results: Vec<crate::commands::repos::RepoMatch> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for base_path in &repo_paths {
                let base = std::path::Path::new(base_path);
                if !base.is_dir() {
                    continue;
                }
                crate::commands::repos::try_add_repo(base, &query_lower, &mut seen, &mut results);
                if let Ok(entries) = std::fs::read_dir(base) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                if !name.starts_with('.') {
                                    crate::commands::repos::try_add_repo(
                                        &path,
                                        &query_lower,
                                        &mut seen,
                                        &mut results,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            results.sort_by_key(|a| a.name.to_lowercase());
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
pub fn broadcast_all(
    app: &tauri::AppHandle,
    broadcaster: &WsBroadcaster,
    event: &str,
    payload: &Value,
) {
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
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| default.to_vec())
}
