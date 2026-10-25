use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tauri::Manager;
use uuid::Uuid;

use crate::config::instance_artifacts::DEBUG_LOGS_FILE_NAME;
use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
// #1265: keep private. A `pub use` here would re-expose the emitter under the
// dispatcher's own path and let the deleted arc come back as an import.
use crate::web::event_broadcast::broadcast_all;

use super::broadcast::WsBroadcaster;

mod agent_update_dispatch;
mod args;
mod coding_agent_dispatch;
mod protocol;

use agent_update_dispatch::dispatch_agent_update_command;
use args::{require_json, require_str, str_or, str_vec_or, terminal_dimension};

use protocol::{
    route_web_command, validate_client_broadcast, BrowserProjectCommand, WebCommandRoute,
};

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
    if let Some(result) = dispatch_agent_update_command(state, cmd, args).await {
        return result;
    }
    if state
        .app_handle
        .try_state::<Arc<crate::app_state::RestoreInProgress>>()
        .is_some_and(|restore| restore.0.load(std::sync::atomic::Ordering::SeqCst))
    {
        return Err("selectionCoordinatorBusy".to_string());
    }
    if let WebCommandRoute::BrowserProject(project_cmd) = route_web_command(cmd) {
        return dispatch_browser_project_command(state, project_cmd, args).await;
    }

    match cmd {
        "task_get_snapshot" => {
            let id = require_str(args, "sessionId")?;
            crate::commands::ac_discovery::task_snapshot_inner(
                &state.session_mgr,
                &state.settings,
                Some(&id),
                None,
            )
            .await
        }
        "task_get_snapshot_at" => {
            let root = require_str(args, "workgroupRoot")?;
            crate::commands::ac_discovery::task_snapshot_inner(
                &state.session_mgr,
                &state.settings,
                None,
                Some(&root),
            )
            .await
        }
        // --- Session commands ---
        "list_sessions" => {
            let mgr = state.session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            serde_json::to_value(sessions).map_err(|e| e.to_string())
        }

        "get_active_session" => {
            let selection = state
                .app_handle
                .state::<crate::session::selection::SelectionCoordinator>()
                .snapshot()
                .await?;
            serde_json::to_value(selection).map_err(|error| error.to_string())
        }

        "drain_session_warnings" => {
            let session_id = args.get("sessionId").and_then(|v| v.as_str());
            let drained = {
                let warnings = state
                    .app_handle
                    .state::<crate::session::warnings::SessionWarningState>();
                let mut buffer = warnings
                    .lock()
                    .map_err(|err| format!("session warning buffer lock poisoned: {err}"))?;
                buffer.drain(session_id)
            };
            serde_json::to_value(drained).map_err(|e| e.to_string())
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
            let (shell, shell_args, agent_label, resolved_agent_host_shell) =
                if let Some(spawn) = resolved_spawn.as_ref() {
                    // #1271 - same snapshot rule as the native path: the resolved
                    // agent stays the command-to-run; the configured default host
                    // shell (copied from this same config guard) travels separately
                    // to the backend.
                    (
                        spawn.shell.clone(),
                        spawn.shell_args.clone(),
                        Some(spawn.trusted_agent_label.clone()),
                        Some(
                            crate::pty::backend::ResolvedAgentHostShell {
                                program: cfg.default_shell.clone(),
                                args: cfg.default_shell_args.clone(),
                            },
                        ),
                    )
                } else {
                    (
                        str_or(args, "shell", &cfg.default_shell),
                        str_vec_or(args, "shellArgs", &cfg.default_shell_args),
                        None,
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
                resolved_agent_host_shell,
                // #973 - browser-mode create. The browser client pushes its fitted size after
                // attach, not at create time, so this keeps AC's 120x30 for now.
                None,
                crate::commands::session::CreateSelectionIntent::User,
            )
            .await?;

            serde_json::to_value(info).map_err(|e| e.to_string())
        }

        "destroy_session" => {
            let id = require_str(args, "id")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

            crate::commands::session::destroy_session_inner(&state.app_handle, uuid).await?;

            Ok(json!(null))
        }

        "switch_session" => {
            let id = require_str(args, "id")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

            state
                .app_handle
                .state::<crate::session::selection::SelectionCoordinator>()
                .transition(crate::session::selection::SelectionRequest::user_switch(uuid))
                .await?;

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
            let cols = terminal_dimension(args, "cols", 120)?;
            let rows = terminal_dimension(args, "rows", 30)?;
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

            let permit = crate::pty::manager::PtyManager::acquire_input_writer(
                &state.pty_mgr,
                uuid,
            )
            .await
            .map_err(|error| error.to_string())?;
            if state
                .app_handle
                .try_state::<std::sync::Arc<crate::session::purge_guard::PurgeGuard>>()
                .is_some_and(|guard| guard.blocks_session(uuid))
            {
                return Err("purge-room in progress for this session; input rejected".to_string());
            }
            crate::pty::manager::PtyManager::write_with_permit(&permit, &data)
                .map_err(|error| error.to_string())?;
            crate::commands::pty::mark_successful_pty_write_busy(
                &state.app_handle,
                uuid,
                data.len(),
            )
            .await;
            drop(permit);

            // #552 web UI input is a real user message (resets badge + silence).
            crate::commands::pty::note_user_message_to_session(
                &state.app_handle,
                uuid,
                crate::commands::pty::UserInputSource::Web(&data),
            )
            .await;

            Ok(json!(null))
        }

        // --- Settings ---
        // #2133 - the server owns the config dir; a browser client is a remote view of it.
        "get_agent_help" => Ok(
            serde_json::to_value(crate::commands::config::get_agent_help())
                .map_err(|e| e.to_string())?,
        ),
        "list_agent_file_sources" => serde_json::to_value(
            crate::commands::config::list_agent_file_sources_inner(&state.settings).await?,
        )
        .map_err(|e| e.to_string()),
        "read_agent_file_source" => {
            let source =
                serde_json::from_value(args.get("source").cloned().ok_or("missing source")?)
                    .map_err(|e| e.to_string())?;
            serde_json::to_value(
                crate::commands::config::read_agent_file_source_inner(&state.settings, source)
                    .await?,
            )
            .map_err(|e| e.to_string())
        }
        "save_agent_file_source" => {
            let source =
                serde_json::from_value(args.get("source").cloned().ok_or("missing source")?)
                    .map_err(|e| e.to_string())?;
            let expected = serde_json::from_value(
                args.get("expectedRevision")
                    .cloned()
                    .ok_or("missing expectedRevision")?,
            )
            .map_err(|e| e.to_string())?;
            let context = args
                .get("contextRevision")
                .and_then(Value::as_str)
                .ok_or("missing contextRevision")?
                .to_owned();
            let edits =
                serde_json::from_value(args.get("edits").cloned().ok_or("missing edits")?)
                    .map_err(|e| e.to_string())?;
            let reply = crate::commands::config::save_agent_file_source_inner(
                &state.settings,
                source,
                expected,
                context,
                edits,
            )
            .await?;
            crate::commands::config::emit_agent_file_save_events(&state.app_handle, &reply);
            serde_json::to_value(reply).map_err(|e| e.to_string())
        }
        "remember_agent_file_source" => {
            let menu = serde_json::from_value(args.get("menu").cloned().ok_or("missing menu")?)
                .map_err(|e| e.to_string())?;
            let source =
                serde_json::from_value(args.get("source").cloned().ok_or("missing source")?)
                    .map_err(|e| e.to_string())?;
            serde_json::to_value(
                crate::commands::config::remember_agent_file_source_inner(
                    &state.settings,
                    menu,
                    source,
                )
                .await?,
            )
            .map_err(|e| e.to_string())
        }

        "get_settings" => {
            // #1077: route through the exact shared snapshot helper so browser
            // and native clients receive the identical resolution report and
            // `rootToken` is omitted from both transports.
            let snapshot =
                crate::commands::config::settings_snapshot_helper(&state.settings, None).await;
            serde_json::to_value(&snapshot).map_err(|e| e.to_string())
        }

        "update_settings" => {
            let new_settings: crate::config::settings::AppSettings =
                serde_json::from_value(args.get("newSettings").cloned().unwrap_or(args.clone()))
                    .map_err(|e| e.to_string())?;

            let tracker = state
                .app_handle
                .try_state::<crate::pty::manager::ResponseCloseState>()
                .map(|state| state.inner().clone());
            let (saved, changed) =
                crate::commands::config::persist_protected_settings_update_with_publication(
                    &state.settings,
                    new_settings,
                    |written| {
                        if let Some(tracker) = &tracker {
                            tracker.apply_settings(
                                written.response_close_enabled,
                                written.response_close_idle_seconds,
                                std::time::Instant::now(),
                            );
                        }
                    },
                )
                .await?;
            if changed {
                broadcast_all(
                    &state.app_handle,
                    &state.broadcaster,
                    "room_idle_confirmation_updated",
                    &json!({}),
                );
            }
            crate::commands::config::purge_sessions_after_settings_update(&saved).await;

            Ok(json!(null))
        }

        "save_settings_draft" => {
            let draft: crate::config::settings::AppSettings =
                serde_json::from_value(args.get("draft").cloned().unwrap_or(args.clone()))
                    .map_err(|e| e.to_string())?;

            let tracker = state
                .app_handle
                .try_state::<crate::pty::manager::ResponseCloseState>()
                .map(|state| state.inner().clone());
            let (saved, events) =
                crate::commands::config::persist_settings_draft_update_with_publication(
                    &state.settings,
                    draft,
                    |written| {
                        if let Some(tracker) = &tracker {
                            tracker.apply_settings(
                                written.response_close_enabled,
                                written.response_close_idle_seconds,
                                std::time::Instant::now(),
                            );
                        }
                    },
                )
                .await?;
            if events.room_idle_confirmation_changed {
                broadcast_all(
                    &state.app_handle,
                    &state.broadcaster,
                    "room_idle_confirmation_updated",
                    &json!({}),
                );
            }
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

        "set_terminal_snapshots_enabled" => {
            let expected: bool = require_json(args, "expected")?;
            let enabled: bool = require_json(args, "enabled")?;
            let tracker = state
                .app_handle
                .try_state::<crate::pty::manager::ResponseCloseState>()
                .map(|state| state.inner().clone());
            crate::commands::config::set_terminal_snapshots_enabled_with_publication(
                &state.settings,
                expected,
                enabled,
                |written| {
                    if let Some(tracker) = &tracker {
                        tracker.apply_settings(
                            written.response_close_enabled,
                            written.response_close_idle_seconds,
                            std::time::Instant::now(),
                        );
                    }
                },
            )
            .await?;
            Ok(json!(null))
        }

        // #965 - the rail is live in the browser client (`src/browser/App.tsx`
        // mounts `<SidebarApp embedded>`), so its collapse setter must be routed
        // or every rail header click hits the `Unknown command` fallback at the
        // bottom of this match. Same omission #859 had to fix for the profile
        // commands below. Symptom if unrouted: SILENT non-persistence, not a
        // console error - the frontend catch swallows the rejection.
        "set_rail_collapse" => {
            // (#965) STRICT parse. Deliberately NOT `unwrap_or_default()` / `unwrap_or(false)`:
            // this setter writes a FULL snapshot, so a payload that is missing or malforms
            // either half is not a partial update, it is a WIPE. A defaulted
            // `collapsedProjects` erases every collapsed section; a defaulted
            // `favoritesCollapsed` re-expands a section the user explicitly folded. BOTH args
            // are therefore required: the desktop path takes them as non-Option typed args and
            // errors when one is absent, and the two transports must not disagree about the
            // same payload. `require_json` additionally rejects a non-string element instead of
            // silently dropping it, which the previous `filter_map` did.
            let collapsed_projects: Vec<String> = require_json(args, "collapsedProjects")?;
            let favorites_collapsed: bool = require_json(args, "favoritesCollapsed")?;
            crate::commands::config::set_rail_collapse_inner(
                &state.settings,
                collapsed_projects,
                favorites_collapsed,
            )
            .await?;
            Ok(json!(null))
        }

        "move_coding_agent"
        | "reorder_coding_agent"
        | "resolve_coding_agent_profile"
        | "set_agent_default_profile"
        | "set_instance_profile_override"
        | "preview_coding_agent_profile_selection"
        | "apply_coding_agent_profile_selection"
        | "preview_selection_lock_removal"
        | "apply_selection_lock_removal"
        | "get_replica_selection_default"
        | "set_replica_selection_default" => {
            coding_agent_dispatch::dispatch_coding_agent_command(state, cmd, args).await
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
            validate_client_broadcast(&event, &payload)?;
            broadcast_all(&state.app_handle, &state.broadcaster, &event, &payload);
            Ok(json!(null))
        }

        "list_detached_sessions" => {
            let detached = state.app_handle.state::<crate::app_state::DetachedSessionsState>();
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
        | "open_external_url"
        // #943 - Browse is desktop-only: `open_external_url` above is already a
        // no-op here, so the submenu is hidden in web mode (`browseSupported()`
        // = isTauri) and this command has no consumer. A real arm would hand a
        // network client (a) an arbitrary-path existence oracle plus a `git.exe`
        // spawn per request, and (b) the repo's `origin`. Not worth it for a
        // feature that cannot run here. See §9 (Module W) for what it costs to
        // turn Browse on for web.
        | "git_remote_url" => Ok(json!(null)),

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
        "append_rail_diagnostic" => {
            crate::commands::config::append_rail_diagnostic(args.get("record").cloned())?;
            Ok(Value::Null)
        }
        "save_debug_logs" => {
            let content = require_str(args, "content")?;
            let path = crate::config::config_dir()
                .ok_or("No config dir")?
                .join(DEBUG_LOGS_FILE_NAME);
            tokio::fs::write(&path, &content)
                .await
                .map_err(|e| format!("Failed to write logs: {}", e))?;
            Ok(json!(null))
        }

        _ => Err(format!("Unknown command: {}", cmd)),
    }
}

async fn dispatch_browser_project_command(
    state: &WsState,
    cmd: BrowserProjectCommand,
    args: &Value,
) -> Result<Value, String> {
    match cmd {
        BrowserProjectCommand::CheckProjectPath => {
            let path = require_str(args, "path")?;
            Ok(json!(
                crate::commands::ac_discovery::check_project_path_inner(&path)
            ))
        }

        BrowserProjectCommand::DiscoverProject => {
            let path = require_str(args, "path")?;
            let branch_watcher = state
                .app_handle
                .state::<Arc<crate::commands::ac_discovery::DiscoveryBranchWatcher>>();
            let coordinator_clocks = state
                .app_handle
                .state::<crate::config::coordinator_clocks::CoordinatorClocksState>(
            );

            let result = crate::commands::ac_discovery::discover_project_inner(
                &state.app_handle,
                &state.session_mgr,
                &path,
                &state.settings,
                branch_watcher.inner(),
                coordinator_clocks.inner(),
            )
            .await?;

            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        BrowserProjectCommand::GetProjectFileDifference => {
            project_file_difference_value(args).await
        }

        BrowserProjectCommand::GetProjectGroups => {
            let path = require_str(args, "path")?;
            let result = crate::commands::project_settings::get_project_groups_inner(&path)?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        BrowserProjectCommand::UpdateProjectGroups => {
            let path = require_str(args, "path")?;
            let config: crate::config::project_settings::WorkgroupGroupsConfig =
                require_json(args, "config")?;
            let result =
                crate::commands::project_settings::update_project_groups_inner(&path, config)?;
            let payload =
                crate::commands::project_settings::project_groups_updated_payload(&path, &result);
            broadcast_all(
                &state.app_handle,
                &state.broadcaster,
                crate::commands::project_settings::PROJECT_GROUPS_UPDATED_EVENT,
                &payload,
            );
            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        BrowserProjectCommand::OpenProject => {
            let path = require_str(args, "path")?;
            let result = crate::commands::ac_discovery::open_project_inner(
                &state.app_handle,
                &state.settings,
                &path,
            )
            .await?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        BrowserProjectCommand::RemoveProject => {
            let path = require_str(args, "path")?;
            crate::commands::ac_discovery::remove_project_inner(
                &state.app_handle,
                &state.settings,
                &path,
            )
            .await?;
            Ok(json!(null))
        }

        BrowserProjectCommand::ArchiveProject => {
            let path = require_str(args, "path")?;
            crate::commands::ac_discovery::archive_project_inner(
                &state.app_handle,
                &state.settings,
                &state.session_mgr,
                &state.pty_mgr,
                &path,
            )
            .await?;
            Ok(json!(null))
        }

        BrowserProjectCommand::UnarchiveProject => {
            let path = require_str(args, "path")?;
            let result = crate::commands::ac_discovery::unarchive_project_inner(
                &state.app_handle,
                &state.settings,
                &path,
            )
            .await?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        BrowserProjectCommand::ListArchivedProjects => {
            let result =
                crate::commands::ac_discovery::list_archived_projects_inner(&state.settings)
                    .await?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
    }
}

fn broadcast_all_to_managed<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    event: &str,
    payload: &Value,
) -> bool {
    let _ = tauri::Emitter::emit(app, event, payload.clone());
    if let Some(bc) = app.try_state::<WsBroadcaster>() {
        bc.broadcast_event(event, payload);
        true
    } else {
        false
    }
}

/// Emit event to both Tauri windows and managed WebSocket clients.
pub fn broadcast_all_r<R: tauri::Runtime>(app: &tauri::AppHandle<R>, event: &str, payload: &Value) {
    let _ = broadcast_all_to_managed(app, event, payload);
}

// --- Arg helpers ---

/// Same helper as the Tauri command; a failed read is `{count: null}`, not an error.
async fn project_file_difference_value(args: &Value) -> Result<Value, String> {
    let path = require_str(args, "path")?;
    let result = crate::commands::ac_discovery::get_project_file_difference_inner(&path).await;
    serde_json::to_value(result).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
