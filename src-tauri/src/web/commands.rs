use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
#[cfg(test)]
use tauri::Manager;
use uuid::Uuid;

use crate::agent_update::{
    AgentUpdateCancelAllResponse, AgentUpdateCancelResponse, AgentUpdateOverviewRow,
    AgentUpdateStatus,
};
use crate::commands::ac_discovery::AcDiscoveryResult;
use crate::commands::config::{
    ApplyCodingAgentProfileSelectionRequest, ApplyCodingAgentProfileSelectionResult,
};
use crate::config::projects::ProjectRegistration;
use crate::config::settings::SettingsState;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;
// #1265: keep private. A `pub use` here would re-expose the emitter under the
// dispatcher's own path and let the deleted arc come back as an import.
use crate::web::event_broadcast::{broadcast_all, DesktopBroadcastEvents, ManagedBroadcastEvents};

use super::broadcast::WsBroadcaster;

mod agent_update_dispatch;
mod args;
mod coding_agent_dispatch;
mod protocol;
mod session_dispatch;
mod settings_dispatch;

use agent_update_dispatch::dispatch_agent_update_command;
#[cfg(test)]
use args::terminal_dimension;
use args::{require_json, require_str};

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
    pub(crate) operations: Arc<dyn WebOperations>,
    pub(crate) desktop_events: Arc<dyn DesktopBroadcastEvents>,
}

pub(crate) struct WebSessionCreate {
    pub(crate) shell: String,
    pub(crate) shell_args: Vec<String>,
    pub(crate) cwd: String,
    pub(crate) session_name: Option<String>,
    pub(crate) agent_id: Option<String>,
    pub(crate) agent_label: Option<String>,
    pub(crate) resolved_spawn: Option<crate::config::agent_command::AgentSpawnCommand>,
    pub(crate) resolved_agent_host_shell: Option<crate::pty::backend::ResolvedAgentHostShell>,
}

pub(crate) trait AgentUpdateOperations: Send + Sync {
    fn agent_update_status(&self) -> Result<AgentUpdateStatus, String>;
    fn agent_update_answer<'a>(
        &'a self,
        settings: &'a SettingsState,
        command: String,
        enabled: bool,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + 'a>>;
    fn agent_update_cancel<'a>(
        &'a self,
        command: String,
    ) -> Pin<Box<dyn Future<Output = Result<AgentUpdateCancelResponse, String>> + Send + 'a>>;
    fn agent_updates_cancel_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<AgentUpdateCancelAllResponse, String>> + Send + 'a>>;
    fn agent_update_overview<'a>(
        &'a self,
        settings: &'a SettingsState,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AgentUpdateOverviewRow>, String>> + Send + 'a>>;
    fn install_coding_agent<'a>(
        &'a self,
        settings: &'a SettingsState,
        key: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
}

pub(crate) trait WebOperations: AgentUpdateOperations + Send + Sync {
    fn selection(&self) -> &crate::session::selection::SelectionCoordinator;
    fn warnings(&self) -> &crate::session::warnings::SessionWarningState;
    fn purge_guard(&self) -> Option<&Arc<crate::session::purge_guard::PurgeGuard>>;
    fn response_close(&self) -> Option<&crate::pty::manager::ResponseCloseState>;
    fn detached(&self) -> &crate::app_state::DetachedSessionsState;
    fn restore(&self) -> Option<&Arc<crate::app_state::RestoreInProgress>>;
    fn emit_agent_file_save_events(&self, reply: &crate::config::settings::AgentFileSaveReply);
    fn create_session<'a>(
        &'a self,
        session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: &'a Arc<Mutex<PtyManager>>,
        request: WebSessionCreate,
    ) -> Pin<Box<dyn Future<Output = Result<SessionInfo, String>> + Send + 'a>>;
    fn destroy_session<'a>(
        &'a self,
        uuid: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
    fn mark_successful_pty_write_busy<'a>(
        &'a self,
        uuid: Uuid,
        byte_count: usize,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
    fn note_user_message_to_session<'a>(
        &'a self,
        uuid: Uuid,
        data: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
    fn apply_coding_agent_profile_selection<'a>(
        &'a self,
        session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: &'a Arc<Mutex<PtyManager>>,
        settings: &'a SettingsState,
        request: ApplyCodingAgentProfileSelectionRequest,
    ) -> futures::future::BoxFuture<
        'a,
        Result<(ApplyCodingAgentProfileSelectionResult, Value), String>,
    >;
    fn discover_project<'a>(
        &'a self,
        session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        path: &'a str,
        settings: &'a SettingsState,
    ) -> Pin<Box<dyn Future<Output = Result<AcDiscoveryResult, String>> + Send + 'a>>;
    fn open_project<'a>(
        &'a self,
        settings: &'a SettingsState,
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<ProjectRegistration, String>> + Send + 'a>>;
    fn remove_project<'a>(
        &'a self,
        settings: &'a SettingsState,
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
    fn archive_project<'a>(
        &'a self,
        settings: &'a SettingsState,
        session_mgr: &'a Arc<tokio::sync::RwLock<SessionManager>>,
        pty_mgr: &'a Arc<Mutex<PtyManager>>,
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
    fn unarchive_project<'a>(
        &'a self,
        settings: &'a SettingsState,
        path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<ProjectRegistration, String>> + Send + 'a>>;
    fn agency_status<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>>;
    fn agency_update<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>>;
}

pub(crate) struct WebCommandsDispatcher {
    state: WsState,
}

impl WebCommandsDispatcher {
    pub(crate) fn new(state: WsState) -> Self {
        Self { state }
    }
}

impl super::command_port::WebCommandDispatcher for WebCommandsDispatcher {
    fn dispatch<'a>(
        &'a self,
        id: u64,
        cmd: &'a str,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(dispatch(&self.state, id, cmd, args))
    }

    fn binary<'a>(&'a self, data: &'a [u8]) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(handle_binary_message(&self.state, data))
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
    let operations = state.operations.as_ref();
    if let Some(result) = dispatch_agent_update_command(state, cmd, args).await {
        return result;
    }
    if operations
        .restore()
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
        "list_sessions"
        | "get_active_session"
        | "drain_session_warnings"
        | "create_session"
        | "destroy_session"
        | "switch_session"
        | "rename_session"
        | "set_last_prompt"
        | "pty_resize"
        | "subscribe_session"
        | "get_pty_size" => {
            session_dispatch::dispatch_session_command(state, cmd, args).await
        }

        // --- PTY commands ---

        // pty_write is handled via binary frames, not JSON commands
        "pty_write" => pty_write_command(state, args).await,

        // --- Settings ---
        // #2133 - the server owns the config dir; a browser client is a remote view of it.

        // #965 - the rail is live in the browser client (`src/browser/App.tsx`
        // mounts `<SidebarApp embedded>`), so its collapse setter must be routed
        // or every rail header click hits the `Unknown command` fallback at the
        // bottom of this match. Same omission #859 had to fix for the profile
        // commands below. Symptom if unrouted: SILENT non-persistence, not a
        // console error - the frontend catch swallows the rejection.
        "set_rail_collapse"
        | "get_agent_help"
        | "list_agent_file_sources"
        | "read_agent_file_source"
        | "save_agent_file_source"
        | "remember_agent_file_source"
        | "get_settings"
        | "update_settings"
        | "save_settings_draft"
        | "set_terminal_snapshots_enabled" => {
            settings_dispatch::dispatch_settings_command(state, cmd, args).await
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

        "get_agency_templates_status" => operations.agency_status().await,
        "update_agency_templates" => operations.agency_update().await,

        // --- Screen replay for late-joining clients ---

        // --- Cross-window event broadcast (theme sync, etc.) ---
        "broadcast_event" => {
            let event = require_str(args, "event")?;
            let payload = args.get("payload").cloned().unwrap_or(json!(null));
            validate_client_broadcast(&event, &payload)?;
            broadcast_all(state.desktop_events.as_ref(), &state.broadcaster, &event, &payload);
            Ok(json!(null))
        }

        "list_detached_sessions" => {
            let detached = operations.detached();
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

            let results = crate::repos::search_repositories(&repo_paths, &query);
            serde_json::to_value(results).map_err(|e| e.to_string())
        }

        // --- Debug ---
        "append_rail_diagnostic" => {
            crate::commands::config::append_rail_diagnostic(args.get("record").cloned())?;
            Ok(Value::Null)
        }
        "save_debug_logs" => {
            let content = require_str(args, "content")?;
            crate::debug_logs::save_debug_logs(crate::config::config_dir(), &content).await?;
            Ok(json!(null))
        }

        _ => Err(format!("Unknown command: {}", cmd)),
    }
}

async fn pty_write_command(state: &WsState, args: &Value) -> Result<Value, String> {
    let operations = state.operations.as_ref();
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
    let permit = crate::pty::manager::PtyManager::acquire_input_writer(&state.pty_mgr, uuid)
        .await
        .map_err(|error| error.to_string())?;
    if operations
        .purge_guard()
        .is_some_and(|guard| guard.blocks_session(uuid))
    {
        return Err("purge-room in progress for this session; input rejected".to_string());
    }
    crate::pty::manager::PtyManager::write_with_permit(&permit, &data)
        .map_err(|error| error.to_string())?;
    operations
        .mark_successful_pty_write_busy(uuid, data.len())
        .await;
    drop(permit);
    // #552 web UI input is a real user message (resets badge + silence).
    operations.note_user_message_to_session(uuid, &data).await;
    Ok(json!(null))
}

async fn dispatch_browser_project_command(
    state: &WsState,
    cmd: BrowserProjectCommand,
    args: &Value,
) -> Result<Value, String> {
    let operations = state.operations.as_ref();
    match cmd {
        BrowserProjectCommand::CheckProjectPath => {
            let path = require_str(args, "path")?;
            Ok(json!(
                crate::commands::ac_discovery::check_project_path_inner(&path)
            ))
        }

        BrowserProjectCommand::DiscoverProject => {
            let path = require_str(args, "path")?;
            let result = operations
                .discover_project(&state.session_mgr, &path, &state.settings)
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
                state.desktop_events.as_ref(),
                &state.broadcaster,
                crate::commands::project_settings::PROJECT_GROUPS_UPDATED_EVENT,
                &payload,
            );
            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        BrowserProjectCommand::OpenProject => {
            let path = require_str(args, "path")?;
            let result = operations.open_project(&state.settings, &path).await?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }

        BrowserProjectCommand::RemoveProject => {
            let path = require_str(args, "path")?;
            operations.remove_project(&state.settings, &path).await?;
            Ok(json!(null))
        }

        BrowserProjectCommand::ArchiveProject => {
            let path = require_str(args, "path")?;
            operations
                .archive_project(&state.settings, &state.session_mgr, &state.pty_mgr, &path)
                .await?;
            Ok(json!(null))
        }

        BrowserProjectCommand::UnarchiveProject => {
            let path = require_str(args, "path")?;
            let result = operations.unarchive_project(&state.settings, &path).await?;
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

fn broadcast_all_to_managed<E: ManagedBroadcastEvents + ?Sized>(
    app: &E,
    event: &str,
    payload: &Value,
) -> bool {
    app.managed_event(event, payload)
}

/// Emit event to both Tauri windows and managed WebSocket clients.
pub fn broadcast_all_r<E: ManagedBroadcastEvents + ?Sized>(app: &E, event: &str, payload: &Value) {
    let _ = broadcast_all_to_managed(app, event, payload);
}

// --- Arg helpers ---

/// Same helper as the Tauri command; a failed read is `{count: null}`, not an error.
async fn project_file_difference_value(args: &Value) -> Result<Value, String> {
    let path = require_str(args, "path")?;
    let result = crate::commands::ac_discovery::get_project_file_difference_inner(&path).await;
    serde_json::to_value(result).map_err(|e| e.to_string())
}

/// Handle a binary PTY write from a WS client.
/// Format: [36 bytes UUID ASCII][raw PTY input bytes]
async fn handle_binary_message(state: &WsState, data: &[u8]) {
    let operations = state.operations.as_ref();
    if data.len() < 36 {
        return;
    }

    let session_id_str = match std::str::from_utf8(&data[..36]) {
        Ok(s) => s.trim(),
        Err(_) => return,
    };

    let uuid = match uuid::Uuid::parse_str(session_id_str) {
        Ok(u) => u,
        Err(_) => return,
    };

    let pty_data = &data[36..];
    let write_result =
        match crate::pty::manager::PtyManager::acquire_input_writer(&state.pty_mgr, uuid).await {
            Ok(permit) => {
                if operations
                    .purge_guard()
                    .is_some_and(|guard| guard.blocks_session(uuid))
                {
                    return;
                }
                let result = crate::pty::manager::PtyManager::write_with_permit(&permit, pty_data);
                if result.is_ok() {
                    operations
                        .mark_successful_pty_write_busy(uuid, pty_data.len())
                        .await;
                }
                result
            }
            Err(error) => Err(error),
        };
    if !binary_pty_write_succeeded(write_result, uuid) {
        return;
    }

    // #552 web UI keystrokes (binary frame) are the real web input path and a
    // genuine user message: reset the badge clock + auto-close silence.
    operations
        .note_user_message_to_session(uuid, pty_data)
        .await;
}

pub(crate) fn binary_pty_write_succeeded(
    result: Result<(), crate::errors::AppError>,
    uuid: uuid::Uuid,
) -> bool {
    match result {
        Ok(()) => true,
        Err(err) => {
            log::warn!(
                "[web-server] PTY binary write failed for session {}: {}",
                uuid,
                err
            );
            false
        }
    }
}

#[cfg(test)]
mod tests;
