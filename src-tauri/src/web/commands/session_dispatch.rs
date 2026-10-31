use super::args::{require_str, str_or, str_vec_or, terminal_dimension};
use super::{WebSessionCreate, WsState};
use crate::web::event_broadcast::broadcast_all;
use serde_json::{json, Value};
use uuid::Uuid;

pub(super) async fn dispatch_session_command(
    state: &WsState,
    cmd: &str,
    args: &Value,
) -> Result<Value, String> {
    let operations = state.operations.as_ref();
    match cmd {
        "list_sessions" => {
            let mgr = state.session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            serde_json::to_value(sessions).map_err(|e| e.to_string())
        }

        "get_active_session" => {
            let selection = operations.selection().snapshot().await?;
            serde_json::to_value(selection).map_err(|error| error.to_string())
        }

        "drain_session_warnings" => {
            let session_id = args.get("sessionId").and_then(|v| v.as_str());
            let drained = {
                let warnings = operations.warnings();
                let mut buffer = warnings
                    .lock()
                    .map_err(|err| format!("session warning buffer lock poisoned: {err}"))?;
                buffer.drain(session_id)
            };
            serde_json::to_value(drained).map_err(|e| e.to_string())
        }

        "create_session" => create_session_command(state, args).await,

        "destroy_session" => {
            let id = require_str(args, "id")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
            operations.destroy_session(uuid).await?;
            Ok(json!(null))
        }

        "switch_session" => {
            let id = require_str(args, "id")?;
            let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
            operations
                .selection()
                .transition(crate::session::selection::SelectionRequest::user_switch(
                    uuid,
                ))
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
                state.desktop_events.as_ref(),
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
                state.desktop_events.as_ref(),
                &state.broadcaster,
                "last_prompt",
                &json!({ "sessionId": id, "text": text }),
            );
            Ok(json!(null))
        }

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
        _ => Err(format!("Unknown command: {}", cmd)),
    }
}

async fn create_session_command(state: &WsState, args: &Value) -> Result<Value, String> {
    let operations = state.operations.as_ref();
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
                Some(crate::pty::backend::ResolvedAgentHostShell {
                    program: cfg.default_shell.clone(),
                    args: cfg.default_shell_args.clone(),
                }),
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
    let info = operations
        .create_session(
            &state.session_mgr,
            &state.pty_mgr,
            WebSessionCreate {
                shell,
                shell_args,
                cwd,
                session_name,
                agent_id,
                agent_label,
                resolved_spawn,
                resolved_agent_host_shell,
            },
        )
        .await?;

    serde_json::to_value(info).map_err(|e| e.to_string())
}
