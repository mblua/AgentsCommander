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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserProjectCommand {
    CheckProjectPath,
    DiscoverProject,
    GetProjectGroups,
    UpdateProjectGroups,
    OpenProject,
    RemoveProject,
    ArchiveProject,
    UnarchiveProject,
    ListArchivedProjects,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WebCommandRoute {
    BrowserProject(BrowserProjectCommand),
    Other,
}

fn route_web_command(cmd: &str) -> WebCommandRoute {
    match cmd {
        "check_project_path" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::CheckProjectPath)
        }
        "discover_project" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::DiscoverProject)
        }
        "get_project_groups" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::GetProjectGroups)
        }
        "update_project_groups" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::UpdateProjectGroups)
        }
        "open_project" => WebCommandRoute::BrowserProject(BrowserProjectCommand::OpenProject),
        "remove_project" => WebCommandRoute::BrowserProject(BrowserProjectCommand::RemoveProject),
        "archive_project" => WebCommandRoute::BrowserProject(BrowserProjectCommand::ArchiveProject),
        "unarchive_project" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::UnarchiveProject)
        }
        "list_archived_projects" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::ListArchivedProjects)
        }
        _ => WebCommandRoute::Other,
    }
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
    if let WebCommandRoute::BrowserProject(project_cmd) = route_web_command(cmd) {
        return dispatch_browser_project_command(state, project_cmd, args).await;
    }

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
                // #973 - browser-mode create. The browser client pushes its fitted size after
                // attach, not at create time, so this keeps AC's 120x30 for now.
                None,
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

            state
                .pty_mgr
                .lock()
                .unwrap()
                .write(uuid, &data)
                .map_err(|e| e.to_string())?;

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

        // --- Coding-agent profiles (#859 web transport parity) ---
        // Desktop registers these in the Tauri invoke_handler, but the browser
        // websocket router did not route them, so the CODING AGENT modal hit the
        // `Unknown command` fallback (e.g. when a closed coordinator falls
        // through to the launch/profile path). The apply/set commands emit
        // `coding_agent_profile_selection_updated`; we `broadcast_all` it so web
        // clients also receive it, mirroring the `save_settings_draft` broadcast
        // of `coding_agent_profiles_updated`.
        "resolve_coding_agent_profile" => {
            let agent_path = args.get("agentPath").and_then(|v| v.as_str());
            let agent_id = require_str(args, "agentId")?;
            let requested_profile = args.get("requestedProfile").and_then(|v| v.as_str());
            let result = crate::commands::config::resolve_coding_agent_profile_inner(
                &state.settings,
                agent_path,
                &agent_id,
                requested_profile,
            )
            .await?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        "set_agent_default_profile" => {
            let agent_path = require_str(args, "agentPath")?;
            let profile = require_str(args, "profile")?;
            let payload = crate::commands::config::set_agent_default_profile_inner(
                &state.settings,
                &agent_path,
                &profile,
            )
            .await?;
            broadcast_all(
                &state.app_handle,
                &state.broadcaster,
                "coding_agent_profile_selection_updated",
                &payload,
            );
            Ok(json!(null))
        }

        "set_instance_profile_override" => {
            let agent_path = require_str(args, "agentPath")?;
            // `profile` is `string | null`; a null clears the override.
            let profile = args.get("profile").and_then(|v| v.as_str());
            let payload = crate::commands::config::set_instance_profile_override_inner(
                &state.settings,
                &agent_path,
                profile,
            )
            .await?;
            broadcast_all(
                &state.app_handle,
                &state.broadcaster,
                "coding_agent_profile_selection_updated",
                &payload,
            );
            Ok(json!(null))
        }

        "preview_coding_agent_profile_selection" => {
            let request: crate::commands::config::PreviewCodingAgentProfileSelectionRequest =
                require_json(args, "request")?;
            let result = crate::commands::config::preview_coding_agent_profile_selection_inner(
                &state.session_mgr,
                &state.settings,
                request,
            )
            .await?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        "apply_coding_agent_profile_selection" => {
            let request: crate::commands::config::ApplyCodingAgentProfileSelectionRequest =
                require_json(args, "request")?;
            let (result, payload) =
                crate::commands::config::apply_coding_agent_profile_selection_inner(
                    &state.app_handle,
                    &state.session_mgr,
                    &state.pty_mgr,
                    &state.settings,
                    request,
                )
                .await?;
            broadcast_all(
                &state.app_handle,
                &state.broadcaster,
                "coding_agent_profile_selection_updated",
                &payload,
            );
            serde_json::to_value(result).map_err(|e| e.to_string())
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

/// #973 - a terminal dimension off the wire, for `pty_resize`.
///
/// This is a network-facing input path, and `as u16` SILENTLY TRUNCATES: `cols: 65536` arrives
/// at the PTY as **0**, and `cols: 65537` as **1**. A zero-column ConPTY is precisely what #973
/// exists to keep out, and a one-column one is not much better - both from a number that looked
/// perfectly plausible on the wire.
///
/// `u16` is exactly what the Tauri `pty_resize` command takes, where serde rejects an
/// out-of-range number for us. The web transport has no such boundary, so this is it: same
/// contract, both transports. A value that is absent stays absent (the caller's default); only
/// one that is PRESENT and cannot be a terminal dimension is an error.
///
/// A zero is deliberately NOT an error here, and the reason is NOT that a legitimate client sends
/// one - it cannot. xterm's `fit()` clamps to MINIMUM_COLS = 2 / MINIMUM_ROWS = 1, so a zero on
/// this path means a client that is not xterm, or is broken, or is hostile. This function is what
/// stands between such a client and the PTY.
///
/// It passes the zero down anyway, because refusing it HERE would be the second place that
/// refuses it. `resize_instance` already does, once, for every transport, with a warn. Erroring
/// here as well would make the same payload behave differently on the two transports - the Tauri
/// command takes a typed `u16`, where serde accepts a 0 and passes it down exactly like this -
/// and would put one rule in two places that can drift apart. One refusal, in the backend, shared.
/// What this boundary is for is the value that cannot be a dimension at all, which is the one the
/// old `as u16` cast turned into a plausible-looking lie.
fn terminal_dimension(args: &Value, key: &str, default: u16) -> Result<u16, String> {
    let Some(value) = args.get(key) else {
        return Ok(default);
    };
    if value.is_null() {
        return Ok(default);
    }
    value
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| format!("Invalid field {}: not a terminal dimension: {}", key, value))
}

fn require_json<T>(args: &Value, key: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    let value = args
        .get(key)
        .cloned()
        .ok_or_else(|| format!("Missing required field: {}", key))?;
    serde_json::from_value(value).map_err(|e| format!("Invalid field {}: {}", key, e))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::project_settings::{
        get_project_groups_inner, PROJECT_GROUPS_UPDATED_EVENT,
    };
    use crate::config::project_settings::{WorkgroupGroup, WorkgroupGroupsConfig};
    use crate::config::settings::AppSettings;
    use crate::pty::git_watcher::GitWatcher;
    use crate::pty::idle_detector::IdleDetector;
    use crate::pty::manager::PtyManager;
    use crate::session::manager::SessionManager;
    use crate::web::broadcast::{WsBroadcaster, WsOutMsg};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// #973 - `pty_resize` is network-facing, and the old `as u16` cast SILENTLY TRUNCATED.
    /// `cols: 65536` reached the PTY as **0** and `cols: 65537` as **1**: a zero-column ConPTY
    /// (the very thing #973 exists to keep out) conjured from a number that looked fine on the
    /// wire. Rejected at the boundary now, exactly as serde rejects it for the Tauri command.
    #[test]
    fn a_terminal_dimension_off_the_wire_is_never_truncated_into_a_degenerate_one() {
        let dim = |v: serde_json::Value| terminal_dimension(&json!({ "cols": v }), "cols", 120);

        assert_eq!(dim(json!(74)).expect("a plain size"), 74);
        assert_eq!(dim(json!(65535)).expect("the largest there is"), 65535);

        // 65536 -> 0 and 65537 -> 1 under `as u16`. This is the whole point.
        assert!(
            dim(json!(65536)).is_err(),
            "65536 must not become a 0-column terminal"
        );
        assert!(
            dim(json!(65537)).is_err(),
            "65537 must not become a 1-column terminal"
        );
        assert!(
            dim(json!(-1)).is_err(),
            "a negative is not a terminal dimension"
        );
        assert!(
            dim(json!("74")).is_err(),
            "a string is not a terminal dimension"
        );
        assert!(
            dim(json!(74.5)).is_err(),
            "a float is not a terminal dimension"
        );

        // absent stays absent: the caller's default, unchanged
        assert_eq!(
            terminal_dimension(&json!({}), "cols", 120).expect("absent"),
            120
        );
        assert_eq!(
            terminal_dimension(&json!({ "cols": null }), "cols", 120).expect("null"),
            120
        );

        // ...and a zero passes through to the ONE guard that refuses it, in `resize_instance`,
        // shared by both transports. Not because a zero is legitimate - xterm cannot produce one -
        // but because refusing it twice, in two places that can drift apart, is worse than
        // refusing it once where both transports meet.
        assert_eq!(
            dim(json!(0)).expect("a zero is refused downstream, not here"),
            0
        );
    }

    #[test]
    fn browser_project_commands_route_before_unknown_fallback() {
        let expected = [
            ("open_project", BrowserProjectCommand::OpenProject),
            ("discover_project", BrowserProjectCommand::DiscoverProject),
            (
                "check_project_path",
                BrowserProjectCommand::CheckProjectPath,
            ),
            ("remove_project", BrowserProjectCommand::RemoveProject),
            ("archive_project", BrowserProjectCommand::ArchiveProject),
            ("unarchive_project", BrowserProjectCommand::UnarchiveProject),
            (
                "list_archived_projects",
                BrowserProjectCommand::ListArchivedProjects,
            ),
            (
                "get_project_groups",
                BrowserProjectCommand::GetProjectGroups,
            ),
            (
                "update_project_groups",
                BrowserProjectCommand::UpdateProjectGroups,
            ),
        ];

        for (command, route) in expected {
            assert_eq!(
                route_web_command(command),
                WebCommandRoute::BrowserProject(route),
                "{command} should be routed before Unknown command"
            );
        }
    }

    #[test]
    fn deferred_and_cli_project_commands_are_not_web_commands() {
        assert_eq!(
            route_web_command("create_ac_project"),
            WebCommandRoute::Other
        );
        assert_eq!(route_web_command("new_project"), WebCommandRoute::Other);
        assert_eq!(route_web_command("open-project"), WebCommandRoute::Other);
        assert_eq!(route_web_command("new-project"), WebCommandRoute::Other);
    }

    #[test]
    fn broadcast_all_r_sends_to_managed_websocket_broadcaster() {
        let broadcaster = WsBroadcaster::new();
        let mut receiver = broadcaster.subscribe();
        let app = tauri::Builder::default()
            .any_thread()
            .manage(broadcaster)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");
        let payload = json!({ "path": "C:/project", "archived": true });

        broadcast_all_r(app.handle(), "project_archive_changed", &payload);

        let event = match receiver.try_recv().expect("broadcast event") {
            WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
            other => panic!("expected text event, got {other:?}"),
        };
        assert_eq!(event["event"], json!("project_archive_changed"));
        assert_eq!(event["payload"], payload);
    }

    #[test]
    fn broadcast_all_sends_to_explicit_websocket_broadcaster() {
        let managed = WsBroadcaster::new();
        let explicit = WsBroadcaster::new();
        let mut receiver = explicit.subscribe();
        let app = tauri::Builder::default()
            .any_thread()
            .manage(managed)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");
        let payload = json!({ "path": "C:/project", "archived": false });

        broadcast_all(app.handle(), &explicit, "project_archive_changed", &payload);

        let event = match receiver.try_recv().expect("broadcast event") {
            WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
            other => panic!("expected text event, got {other:?}"),
        };
        assert_eq!(event["event"], json!("project_archive_changed"));
        assert_eq!(event["payload"], payload);
    }

    #[tokio::test]
    async fn update_project_groups_web_dispatch_broadcasts_saved_config() {
        let app = tauri::Builder::default()
            .any_thread()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");
        let app_handle = app.handle().clone();
        let broadcaster = WsBroadcaster::new();
        let mut receiver = broadcaster.subscribe();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let git_watcher = GitWatcher::new(Arc::clone(&session_mgr), app_handle.clone());
        let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
            Arc::new(Mutex::new(HashMap::new())),
            idle_detector,
            git_watcher,
            Some(broadcaster.clone()),
            Arc::clone(&session_mgr),
        )));
        let settings = Arc::new(tokio::sync::RwLock::new(AppSettings::default()));
        let state = WsState {
            session_mgr,
            pty_mgr,
            settings,
            broadcaster: broadcaster.clone(),
            app_handle,
        };

        let project = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(project.path().join(".ac")).expect("create .ac");
        let path = project.path().to_string_lossy().to_string();
        let config = WorkgroupGroupsConfig {
            groups: vec![WorkgroupGroup {
                id: "core".to_string(),
                name: "Core".to_string(),
                regex: "^wg-14$".to_string(),
            }],
            show_all: false,
            show_ungrouped: true,
            non_stop: None,
        };

        let response = dispatch(
            &state,
            7,
            "update_project_groups",
            &json!({ "path": path, "config": config.clone() }),
        )
        .await;

        assert_eq!(response["id"], json!(7));
        assert_eq!(
            response["result"],
            serde_json::to_value(&config).expect("serialize config")
        );
        assert_eq!(
            get_project_groups_inner(&path).expect("load saved groups"),
            config
        );

        let event = match receiver.try_recv().expect("broadcast event") {
            WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
            other => panic!("expected text event, got {other:?}"),
        };
        assert_eq!(event["event"], json!(PROJECT_GROUPS_UPDATED_EVENT));
        assert_eq!(event["payload"]["projectPath"], json!(path));
        assert_eq!(
            event["payload"]["config"],
            serde_json::to_value(&config).expect("serialize config")
        );
    }

    /// Build a minimal `WsState` backed by `settings`, plus a broadcast receiver
    /// for asserting web events emitted during dispatch.
    fn ws_state_for(settings: AppSettings) -> (WsState, tokio::sync::mpsc::Receiver<WsOutMsg>) {
        let app = tauri::Builder::default()
            .any_thread()
            .manage(crate::session::warnings::new_session_warning_state())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build test app");
        let app_handle = app.handle().clone();
        let broadcaster = WsBroadcaster::new();
        let receiver = broadcaster.subscribe();
        let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let idle_detector = IdleDetector::new(|_| {}, |_| {});
        let git_watcher = GitWatcher::new(Arc::clone(&session_mgr), app_handle.clone());
        let pty_mgr = Arc::new(Mutex::new(PtyManager::new(
            Arc::new(Mutex::new(HashMap::new())),
            idle_detector,
            git_watcher,
            Some(broadcaster.clone()),
            Arc::clone(&session_mgr),
        )));
        let settings = Arc::new(tokio::sync::RwLock::new(settings));
        let state = WsState {
            session_mgr,
            pty_mgr,
            settings,
            broadcaster,
            app_handle,
        };
        (state, receiver)
    }

    fn test_agent(id: &str) -> crate::config::settings::AgentConfig {
        crate::config::settings::AgentConfig {
            id: id.to_string(),
            label: id.to_string(),
            command: id.to_string(),
            color: "#10b981".to_string(),
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            backend: Default::default(),
        }
    }

    #[tokio::test]
    async fn drain_session_warnings_web_dispatch_drains_buffer() {
        let (state, _rx) = ws_state_for(AppSettings::default());
        let session_id = Uuid::new_v4();
        {
            let warnings = state
                .app_handle
                .state::<crate::session::warnings::SessionWarningState>();
            let mut buffer = warnings.lock().expect("warning buffer lock");
            buffer.push(crate::session::warnings::SessionWarning::new(
                session_id,
                "CLAUDE_CONFIG_DIR",
                "outside-mount",
                "warning text",
            ));
        }

        let response = dispatch(
            &state,
            9,
            "drain_session_warnings",
            &json!({ "sessionId": session_id.to_string() }),
        )
        .await;

        assert_eq!(response["id"], json!(9));
        assert_eq!(
            response["result"][0]["sessionId"],
            json!(session_id.to_string())
        );
        assert_eq!(response["result"][0]["key"], json!("CLAUDE_CONFIG_DIR"));

        let response = dispatch(
            &state,
            10,
            "drain_session_warnings",
            &json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        assert_eq!(response["id"], json!(10));
        assert_eq!(response["result"], json!([]));
    }

    #[tokio::test]
    async fn coding_agent_profile_commands_route_past_unknown_command() {
        // #859 regression: before web-router parity these five commands hit the
        // `Unknown command` fallback. Empty args make each inner fail arg
        // validation instead, which proves the route now exists.
        let (state, _rx) = ws_state_for(AppSettings::default());
        for cmd in [
            "resolve_coding_agent_profile",
            "preview_coding_agent_profile_selection",
            "apply_coding_agent_profile_selection",
            "set_agent_default_profile",
            "set_instance_profile_override",
        ] {
            let response = dispatch(&state, 1, cmd, &json!({})).await;
            let error = response
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            assert!(
                !error.contains("Unknown command"),
                "{cmd} should be routed, got error: {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn resolve_coding_agent_profile_web_dispatch_returns_resolution() {
        let settings = AppSettings {
            agents: vec![test_agent("codex")],
            ..AppSettings::default()
        };
        let (state, _rx) = ws_state_for(settings);

        let response = dispatch(
            &state,
            2,
            "resolve_coding_agent_profile",
            &json!({ "agentId": "codex", "requestedProfile": "A" }),
        )
        .await;

        assert_eq!(response["id"], json!(2));
        assert!(
            response.get("error").is_none(),
            "resolve should succeed, got {response:?}"
        );
        assert!(
            response["result"]["effectiveProfile"].is_string(),
            "expected a resolved profile, got {response:?}"
        );
    }

    #[tokio::test]
    async fn apply_coding_agent_profile_selection_web_dispatch_broadcasts() {
        // Real WG replica layout so enumeration + config write succeed under the
        // replica scope (no confirmation fingerprint required).
        let temp = tempfile::tempdir().expect("tempdir");
        let project = temp.path().join("project");
        let workspace = project.join(".ac");
        let matrix = workspace.join("_agent_codex");
        let replica = workspace.join("wg-1-team").join("__agent_codex");
        std::fs::create_dir_all(&matrix).expect("create matrix");
        std::fs::create_dir_all(&replica).expect("create replica");
        std::fs::write(matrix.join("Role.md"), "# Codex\n").expect("write Role.md");
        std::fs::write(
            replica.join("config.json"),
            serde_json::to_string(&json!({ "identity": "../../_agent_codex" }))
                .expect("serialize replica config"),
        )
        .expect("write replica config");

        let settings = AppSettings {
            agents: vec![test_agent("codex")],
            project_paths: vec![project.to_string_lossy().to_string()],
            ..AppSettings::default()
        };
        let (state, mut rx) = ws_state_for(settings);

        let request = json!({
            "targetReplicaPath": replica.to_string_lossy(),
            "codingAgentId": "codex",
            "profile": "B",
            "scope": "replica",
            "restartSessions": false,
            "confirmedTargetFingerprint": null,
            "typedConfirmation": null,
        });

        let response = dispatch(
            &state,
            3,
            "apply_coding_agent_profile_selection",
            &json!({ "request": request }),
        )
        .await;

        assert_eq!(response["id"], json!(3));
        assert!(
            response.get("error").is_none(),
            "apply should succeed, got {response:?}"
        );
        assert_eq!(response["result"]["updatedCount"], json!(1));
        assert_eq!(response["result"]["scope"], json!("replica"));

        // The web dispatcher must broadcast the selection-updated event so
        // browser clients refresh, mirroring the save_settings_draft pattern.
        let event = match rx.try_recv().expect("broadcast event") {
            WsOutMsg::Text(text) => serde_json::from_str::<Value>(&text).expect("parse event"),
            other => panic!("expected text event, got {other:?}"),
        };
        assert_eq!(
            event["event"],
            json!("coding_agent_profile_selection_updated")
        );
        assert_eq!(event["payload"]["scope"], json!("replica"));
        assert_eq!(event["payload"]["codingAgentId"], json!("codex"));
        assert_eq!(event["payload"]["profile"], json!("B"));
        assert_eq!(event["payload"]["updatedCount"], json!(1));
    }
}
