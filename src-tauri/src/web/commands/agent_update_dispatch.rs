use serde_json::{json, Value};

use super::args::{require_json, require_str};
use super::WsState;

/// #1551 - agent-update surface parity. These arms read gate/catalog state or answer the
/// startup prompt; none touches sessions or the selection coordinator, and the prompt
/// phase runs WHILE restore is parked on the gate (`lib.rs`), so they must stay
/// reachable while `RestoreInProgress` is set: otherwise a browser client can never answer
/// the prompt and never observes the pass. Every other command keeps the guard.
pub(super) async fn dispatch_agent_update_command(
    state: &WsState,
    cmd: &str,
    args: &Value,
) -> Option<Result<Value, String>> {
    let operations = state.operations.as_ref();
    match cmd {
        "get_agent_update_status" => Some(operations.agent_update_status().and_then(|status| {
            serde_json::to_value(status)
                .map_err(|e| format!("Failed to serialize agent update status: {e}"))
        })),
        "agent_update_answer" => {
            let parsed = require_str(args, "command").and_then(|command| {
                require_json::<bool>(args, "enabled").map(|enabled| (command, enabled))
            });
            Some(match parsed {
                Ok((command, enabled)) => operations
                    .agent_update_answer(&state.settings, command, enabled)
                    .await
                    .map(|applied| json!(applied)),
                Err(e) => Err(e),
            })
        }
        "agent_update_cancel" => Some(match require_str(args, "command") {
            Ok(command) => operations
                .agent_update_cancel(command)
                .await
                .and_then(|response| {
                    serde_json::to_value(response).map_err(|error| {
                        format!("Failed to serialize cancellation response: {error}")
                    })
                }),
            Err(error) => Err(error),
        }),
        "agent_updates_cancel_all" => Some(operations.agent_updates_cancel_all().await.and_then(
            |response| {
                serde_json::to_value(response).map_err(|error| {
                    format!("Failed to serialize batch cancellation response: {error}")
                })
            },
        )),
        "get_agent_update_overview" => Some(
            match operations.agent_update_overview(&state.settings).await {
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
            Ok(key) => operations
                .install_coding_agent(&state.settings, key)
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
