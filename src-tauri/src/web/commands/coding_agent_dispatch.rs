use serde_json::{json, Value};

use crate::web::event_broadcast::broadcast_all;

use super::args::{require_json, require_str};
use super::WsState;

/// Coding-agent order and profile arms of `dispatch_inner`, reached behind the same
/// restore guard and routing as every other arm.
pub(super) async fn dispatch_coding_agent_command(
    state: &WsState,
    cmd: &str,
    args: &Value,
) -> Result<Value, String> {
    match cmd {
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
                state.desktop_events.as_ref(),
                &state.broadcaster,
                "coding_agent_profile_selection_updated",
                &payload,
            );
            Ok(json!(null))
        }

        "set_instance_profile_override" => set_instance_profile_override_command(state, args).await,

        "preview_coding_agent_profile_selection" => {
            preview_coding_agent_profile_selection_command(state, args).await
        }

        "apply_coding_agent_profile_selection" => {
            apply_coding_agent_profile_selection_command(state, args).await
        }

        // #1941 - the scoped selection-lock API shares the desktop inner
        // implementation and the same owned selection-operation boundary: reads
        // acquire the turn for a consistent snapshot, mutations broadcast their
        // event payload before releasing it.
        "preview_selection_lock_removal" => {
            preview_selection_lock_removal_command(state, args).await
        }

        "apply_selection_lock_removal" => apply_selection_lock_removal_command(state, args).await,

        "get_replica_selection_default" => get_replica_selection_default_command(state, args).await,

        "set_replica_selection_default" => set_replica_selection_default_command(state, args).await,

        _ => Err(format!("Unknown command: {}", cmd)),
    }
}

async fn get_replica_selection_default_command(
    state: &WsState,
    args: &Value,
) -> Result<Value, String> {
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

async fn set_replica_selection_default_command(
    state: &WsState,
    args: &Value,
) -> Result<Value, String> {
    let request: crate::commands::config::SetReplicaSelectionDefaultRequest =
        require_json(args, "request")?;
    let state = state.clone();
    crate::session::selection::run_owned_selection_operation(move || async move {
        let (result, payload) =
            crate::commands::config::set_replica_selection_default_inner(&state.settings, request)
                .await?;
        broadcast_all(
            state.desktop_events.as_ref(),
            &state.broadcaster,
            "coding_agent_profile_selection_updated",
            &payload,
        );
        serde_json::to_value(result).map_err(|e| e.to_string())
    })
    .await
}

/// #2306 P2 - browser route body for `move_coding_agent`, factored so a test
/// can drive it with a temp-path saver (the real one resolves the process
/// config dir). Broadcasts `coding_agent_settings_updated` with the same payload
/// the Tauri command emits, after the inner releases the settings write guard.
pub(super) async fn move_coding_agent_route_with_saver(
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
        state.desktop_events.as_ref(),
        &state.broadcaster,
        "coding_agent_settings_updated",
        &crate::commands::config::move_coding_agent_event_payload(id),
    );
    serde_json::to_value(ids).map_err(|e| e.to_string())
}

/// #2542 - browser route body for `reorder_coding_agent`, mirroring
/// `move_coding_agent_route_with_saver`: broadcasts the move's event payload
/// after the inner releases the settings write guard.
pub(super) async fn reorder_coding_agent_route_with_saver(
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
        state.desktop_events.as_ref(),
        &state.broadcaster,
        "coding_agent_settings_updated",
        &crate::commands::config::move_coding_agent_event_payload(id),
    );
    serde_json::to_value(ids).map_err(|e| e.to_string())
}

async fn set_instance_profile_override_command(
    state: &WsState,
    args: &Value,
) -> Result<Value, String> {
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
            state.desktop_events.as_ref(),
            &state.broadcaster,
            "coding_agent_profile_selection_updated",
            &payload,
        );
        Ok(json!(null))
    })
    .await
}

async fn preview_coding_agent_profile_selection_command(
    state: &WsState,
    args: &Value,
) -> Result<Value, String> {
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
            let result = crate::commands::config::preview_coding_agent_profile_selection_inner(
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

async fn apply_coding_agent_profile_selection_command(
    state: &WsState,
    args: &Value,
) -> Result<Value, String> {
    let request: crate::commands::config::ApplyCodingAgentProfileSelectionRequest =
        require_json(args, "request")?;
    // #1940 - the whole apply runs under the owned selection operation
    // turn, and its broadcast happens before that turn is released.
    let state = state.clone();
    crate::session::selection::run_owned_selection_operation(move || async move {
        let (result, payload) = state
            .operations
            .apply_coding_agent_profile_selection(
                &state.session_mgr,
                &state.pty_mgr,
                &state.settings,
                request,
            )
            .await?;
        broadcast_all(
            state.desktop_events.as_ref(),
            &state.broadcaster,
            "coding_agent_profile_selection_updated",
            &payload,
        );
        serde_json::to_value(result).map_err(|e| e.to_string())
    })
    .await
}

async fn preview_selection_lock_removal_command(
    state: &WsState,
    args: &Value,
) -> Result<Value, String> {
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

async fn apply_selection_lock_removal_command(
    state: &WsState,
    args: &Value,
) -> Result<Value, String> {
    let request: crate::commands::config::ApplySelectionLockRemovalRequest =
        require_json(args, "request")?;
    let state = state.clone();
    crate::session::selection::run_owned_selection_operation(move || async move {
        let (result, payload) =
            crate::commands::config::apply_selection_lock_removal_inner(&state.settings, request)
                .await?;
        broadcast_all(
            state.desktop_events.as_ref(),
            &state.broadcaster,
            "coding_agent_profile_selection_updated",
            &payload,
        );
        serde_json::to_value(result).map_err(|e| e.to_string())
    })
    .await
}
