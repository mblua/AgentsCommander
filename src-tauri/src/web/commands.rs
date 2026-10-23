use std::sync::{Arc, Mutex};

use serde::Deserialize;
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
    GetProjectFileDifference,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeChangedPayload {
    light: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceMonitorAttachPayload {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenSettingsPayload {
    #[serde(default)]
    section: Option<String>,
}

fn validate_client_broadcast(event: &str, payload: &Value) -> Result<(), String> {
    if !payload.is_object() {
        return Err(format!("Invalid {event} payload: expected object"));
    }
    match event {
        "theme_changed" => {
            let decoded: ThemeChangedPayload = serde_json::from_value(payload.clone())
                .map_err(|error| format!("Invalid theme_changed payload: {error}"))?;
            let _ = decoded.light;
        }
        "resource_monitor_attach" => {
            serde_json::from_value::<ResourceMonitorAttachPayload>(payload.clone())
                .map_err(|error| format!("Invalid resource_monitor_attach payload: {error}"))?;
        }
        "open_settings" => {
            let decoded: OpenSettingsPayload = serde_json::from_value(payload.clone())
                .map_err(|error| format!("Invalid open_settings payload: {error}"))?;
            let _ = decoded.section;
        }
        _ => return Err(format!("Client broadcast event is not allowed: {event}")),
    }
    Ok(())
}

fn route_web_command(cmd: &str) -> WebCommandRoute {
    match cmd {
        "check_project_path" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::CheckProjectPath)
        }
        "discover_project" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::DiscoverProject)
        }
        "get_project_file_difference" => {
            WebCommandRoute::BrowserProject(BrowserProjectCommand::GetProjectFileDifference)
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

/// #1551 - agent-update surface parity. These arms read gate/catalog state or answer the
/// startup prompt; none touches sessions or the selection coordinator, and the prompt
/// phase runs WHILE restore is parked on the gate (`lib.rs`), so they must stay
/// reachable while `RestoreInProgress` is set: otherwise a browser client can never answer
/// the prompt and never observes the pass. Every other command keeps the guard.
async fn dispatch_agent_update_command(
    state: &WsState,
    cmd: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    match cmd {
        "get_agent_update_status" => Some(
            crate::commands::config::agent_update_status_inner(&state.app_handle).and_then(
                |status| {
                    serde_json::to_value(status)
                        .map_err(|e| format!("Failed to serialize agent update status: {e}"))
                },
            ),
        ),
        "agent_update_answer" => {
            let parsed = require_str(args, "command").and_then(|command| {
                require_json::<bool>(args, "enabled").map(|enabled| (command, enabled))
            });
            Some(match parsed {
                Ok((command, enabled)) => crate::commands::config::agent_update_answer_inner(
                    &state.app_handle,
                    &state.settings,
                    command,
                    enabled,
                )
                .await
                .map(|applied| json!(applied)),
                Err(e) => Err(e),
            })
        }
        "agent_update_cancel" => Some(match require_str(args, "command") {
            Ok(command) => {
                crate::commands::config::agent_update_cancel_inner(&state.app_handle, command)
                    .await
                    .and_then(|response| {
                        serde_json::to_value(response).map_err(|error| {
                            format!("Failed to serialize cancellation response: {error}")
                        })
                    })
            }
            Err(error) => Err(error),
        }),
        "agent_updates_cancel_all" => Some(
            crate::commands::config::agent_updates_cancel_all_inner(&state.app_handle)
                .await
                .and_then(|response| {
                    serde_json::to_value(response).map_err(|error| {
                        format!("Failed to serialize batch cancellation response: {error}")
                    })
                }),
        ),
        "get_agent_update_overview" => Some(
            match crate::commands::config::agent_update_overview_inner(
                &state.app_handle,
                &state.settings,
            )
            .await
            {
                Ok(rows) => serde_json::to_value(rows)
                    .map_err(|e| format!("Failed to serialize agent update overview: {e}")),
                Err(e) => Err(e),
            },
        ),
        "get_coding_agent_catalog" => {
            // #1967 P4 - the shared inner returns the persisted catalog or the
            // typed unavailability. Propagate that error as a transport error
            // (never serialize a Rust `Result` as the JSON payload).
            Some(
                match crate::commands::config::coding_agent_catalog_inner(&state.settings).await {
                    Ok(catalog) => serde_json::to_value(catalog)
                        .map_err(|e| format!("Failed to serialize coding agent catalog: {e}")),
                    Err(unavailable) => Err(unavailable),
                },
            )
        }
        "get_coding_agent_welcome_status" => Some(
            // #2736 - argument-free read: `args` is ignored, like the catalog arm.
            match crate::commands::config::coding_agent_welcome_status_inner(&state.settings).await
            {
                Ok(rows) => serde_json::to_value(rows)
                    .map_err(|e| format!("Failed to serialize coding agent welcome status: {e}")),
                Err(unavailable) => Err(unavailable),
            },
        ),
        "install_coding_agent" => Some(match require_str(args, "key") {
            // #2736 - returns once the install task is spawned; the outcome is
            // the `coding_agent_install_finished` event.
            Ok(key) => crate::commands::config::install_coding_agent_inner(
                &state.app_handle,
                &state.settings,
                key,
            )
            .await
            .map(|()| Value::Null),
            Err(error) => Err(error),
        }),
        "get_coding_agent_catalog_report" => {
            let report =
                crate::commands::config::coding_agent_catalog_report_inner(&state.settings).await;
            Some(
                serde_json::to_value(report)
                    .map_err(|e| format!("Failed to serialize coding agent catalog report: {e}")),
            )
        }
        "list_reseedable_agent_commands" => Some(
            serde_json::to_value(crate::commands::config::list_reseedable_agent_commands())
                .map_err(|e| format!("Failed to serialize reseedable commands: {e}")),
        ),
        _ => None,
    }
}

/// #2306 P2 - browser route body for `move_coding_agent`, factored so a test
/// can drive it with a temp-path saver (the real one resolves the process
/// config dir). Broadcasts `coding_agent_settings_updated` with the same payload
/// the Tauri command emits, after the inner releases the settings write guard.
async fn move_coding_agent_route_with_saver(
    state: &WsState,
    id: &str,
    neighbor_id: &str,
    direction: crate::config::coding_agent_mutations::AgentMoveDirection,
    save: impl FnOnce(
        &crate::config::settings::AppSettings,
    ) -> Result<crate::config::settings::AppSettings, String>,
) -> Result<Value, String> {
    let ids = crate::commands::config::move_coding_agent_inner_with_saver(
        &state.settings,
        id,
        neighbor_id,
        direction,
        save,
    )
    .await?;
    broadcast_all(
        &state.app_handle,
        &state.broadcaster,
        "coding_agent_settings_updated",
        &crate::commands::config::move_coding_agent_event_payload(id),
    );
    serde_json::to_value(ids).map_err(|e| e.to_string())
}

/// #2542 - browser route body for `reorder_coding_agent`, mirroring
/// `move_coding_agent_route_with_saver`: broadcasts the move's event payload
/// after the inner releases the settings write guard.
async fn reorder_coding_agent_route_with_saver(
    state: &WsState,
    id: &str,
    expected_ids: &[String],
    target_index: usize,
    save: impl FnOnce(
        &crate::config::settings::AppSettings,
    ) -> Result<crate::config::settings::AppSettings, String>,
) -> Result<Value, String> {
    let ids = crate::commands::config::reorder_coding_agent_inner_with_saver(
        &state.settings,
        id,
        expected_ids,
        target_index,
        save,
    )
    .await?;
    broadcast_all(
        &state.app_handle,
        &state.broadcaster,
        "coding_agent_settings_updated",
        &crate::commands::config::move_coding_agent_event_payload(id),
    );
    serde_json::to_value(ids).map_err(|e| e.to_string())
}

async fn dispatch_inner(state: &WsState, cmd: &str, args: &Value) -> Result<Value, String> {
    if let Some(result) = dispatch_agent_update_command(state, cmd, args).await {
        return result;
    }
    if state
        .app_handle
        .try_state::<Arc<crate::RestoreInProgress>>()
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

        // #2306 P2 - ordered move of a registered coding agent. The inner holds
        // the settings write guard; the broadcast runs after it returns, so the
        // guard is released before `broadcast_all`.
        "move_coding_agent" => {
            let id = require_str(args, "id")?;
            let neighbor_id = require_str(args, "neighborId")?;
            let direction: crate::config::coding_agent_mutations::AgentMoveDirection =
                require_json(args, "direction")?;
            move_coding_agent_route_with_saver(
                state,
                &id,
                &neighbor_id,
                direction,
                crate::config::settings::save_settings,
            )
            .await
        }

        // #2542 - one-step reorder; same guard/broadcast ordering as the move.
        "reorder_coding_agent" => {
            let id = require_str(args, "id")?;
            let expected_ids: Vec<String> = require_json(args, "expectedIds")?;
            let target_index: usize = require_json(args, "targetIndex")?;
            reorder_coding_agent_route_with_saver(
                state,
                &id,
                &expected_ids,
                target_index,
                crate::config::settings::save_settings,
            )
            .await
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
            let profile = args
                .get("profile")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            // #1940 - clone the handles/request into the owned selection
            // operation so the guarded write and its broadcast stay serialized
            // with the desktop paths and the self-switch persist/restart.
            let state = state.clone();
            crate::session::selection::run_owned_selection_operation(move || async move {
                let payload = crate::commands::config::set_instance_profile_override_inner(
                    &state.settings,
                    &agent_path,
                    profile.as_deref(),
                )
                .await?;
                broadcast_all(
                    &state.app_handle,
                    &state.broadcaster,
                    "coding_agent_profile_selection_updated",
                    &payload,
                );
                Ok(json!(null))
            })
            .await
        }

        "preview_coding_agent_profile_selection" => {
            let request: crate::commands::config::PreviewCodingAgentProfileSelectionRequest =
                require_json(args, "request")?;
            // #1941 - the preview takes the owned selection turn for a consistent
            // operation boundary, exactly like its sibling selection arms: a read
            // queued behind a mutation waits (and may time out at the transport
            // without canceling the mutation) instead of observing half-settled
            // state.
            let state = state.clone();
            crate::session::selection::run_owned_selection_read_timed(
                "preview_coding_agent_profile_selection",
                move || async move {
                    let result =
                        crate::commands::config::preview_coding_agent_profile_selection_inner(
                            &state.session_mgr,
                            &state.settings,
                            request,
                        )
                        .await?;
                    serde_json::to_value(result).map_err(|e| e.to_string())
                },
            )
            .await
        }

        "apply_coding_agent_profile_selection" => {
            let request: crate::commands::config::ApplyCodingAgentProfileSelectionRequest =
                require_json(args, "request")?;
            // #1940 - the whole apply runs under the owned selection operation
            // turn, and its broadcast happens before that turn is released.
            let state = state.clone();
            crate::session::selection::run_owned_selection_operation(move || async move {
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
            })
            .await
        }

        // #1941 - the scoped selection-lock API shares the desktop inner
        // implementation and the same owned selection-operation boundary: reads
        // acquire the turn for a consistent snapshot, mutations broadcast their
        // event payload before releasing it.
        "preview_selection_lock_removal" => {
            let request: crate::commands::config::PreviewSelectionLockRemovalRequest =
                require_json(args, "request")?;
            let state = state.clone();
            crate::session::selection::run_owned_selection_read_timed(
                "preview_selection_lock_removal",
                move || async move {
                    let result = crate::commands::config::preview_selection_lock_removal_inner(
                        &state.session_mgr,
                        &state.settings,
                        request,
                    )
                    .await?;
                    serde_json::to_value(result).map_err(|e| e.to_string())
                },
            )
            .await
        }

        "apply_selection_lock_removal" => {
            let request: crate::commands::config::ApplySelectionLockRemovalRequest =
                require_json(args, "request")?;
            let state = state.clone();
            crate::session::selection::run_owned_selection_operation(move || async move {
                let (result, payload) =
                    crate::commands::config::apply_selection_lock_removal_inner(
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
            })
            .await
        }

        "get_replica_selection_default" => {
            let request: crate::commands::config::GetReplicaSelectionDefaultRequest =
                require_json(args, "request")?;
            let state = state.clone();
            crate::session::selection::run_owned_selection_read_timed(
                "get_replica_selection_default",
                move || async move {
                    let result = crate::commands::config::get_replica_selection_default_inner(
                        &state.settings,
                        request,
                    )
                    .await?;
                    serde_json::to_value(result).map_err(|e| e.to_string())
                },
            )
            .await
        }

        "set_replica_selection_default" => {
            let request: crate::commands::config::SetReplicaSelectionDefaultRequest =
                require_json(args, "request")?;
            let state = state.clone();
            crate::session::selection::run_owned_selection_operation(move || async move {
                let (result, payload) =
                    crate::commands::config::set_replica_selection_default_inner(
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
            })
            .await
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
mod tests;
