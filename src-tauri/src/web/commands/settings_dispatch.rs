use super::args::require_json;
use super::WsState;
use crate::web::event_broadcast::broadcast_all;
use serde_json::{json, Value};

pub(super) async fn dispatch_settings_command(
    state: &WsState,
    cmd: &str,
    args: &Value,
) -> Result<Value, String> {
    let operations = state.operations.as_ref();
    match cmd {
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

        "save_agent_file_source" => save_agent_file_source_command(state, args).await,

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

        "update_settings" => update_settings_command(state, args).await,

        "save_settings_draft" => save_settings_draft_command(state, args).await,

        "set_terminal_snapshots_enabled" => {
            let expected: bool = require_json(args, "expected")?;
            let enabled: bool = require_json(args, "enabled")?;
            let tracker = operations.response_close().cloned();
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
        _ => Err(format!("Unknown command: {}", cmd)),
    }
}

async fn update_settings_command(state: &WsState, args: &Value) -> Result<Value, String> {
    let operations = state.operations.as_ref();
    let new_settings: crate::config::settings::AppSettings =
        serde_json::from_value(args.get("newSettings").cloned().unwrap_or(args.clone()))
            .map_err(|e| e.to_string())?;
    let tracker = operations.response_close().cloned();
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
            state.desktop_events.as_ref(),
            &state.broadcaster,
            "room_idle_confirmation_updated",
            &json!({}),
        );
    }
    crate::commands::config::purge_sessions_after_settings_update(&saved).await;
    Ok(json!(null))
}

async fn save_settings_draft_command(state: &WsState, args: &Value) -> Result<Value, String> {
    let operations = state.operations.as_ref();
    let draft: crate::config::settings::AppSettings =
        serde_json::from_value(args.get("draft").cloned().unwrap_or(args.clone()))
            .map_err(|e| e.to_string())?;
    let tracker = operations.response_close().cloned();
    let (saved, events) = crate::commands::config::persist_settings_draft_update_with_publication(
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
            state.desktop_events.as_ref(),
            &state.broadcaster,
            "room_idle_confirmation_updated",
            &json!({}),
        );
    }
    crate::commands::config::purge_sessions_after_settings_update(&saved).await;
    if events.profiles_changed {
        broadcast_all(
            state.desktop_events.as_ref(),
            &state.broadcaster,
            "coding_agent_profiles_updated",
            &json!({}),
        );
    }
    for agent_id in events.env_agent_ids {
        broadcast_all(
            state.desktop_events.as_ref(),
            &state.broadcaster,
            "coding_agent_env_settings_updated",
            &json!({ "agentId": agent_id }),
        );
    }

    Ok(json!(null))
}

async fn save_agent_file_source_command(state: &WsState, args: &Value) -> Result<Value, String> {
    let operations = state.operations.as_ref();
    let source = serde_json::from_value(args.get("source").cloned().ok_or("missing source")?)
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
    let edits = serde_json::from_value(args.get("edits").cloned().ok_or("missing edits")?)
        .map_err(|e| e.to_string())?;
    let reply = crate::commands::config::save_agent_file_source_inner(
        &state.settings,
        source,
        expected,
        context,
        edits,
    )
    .await?;
    operations.emit_agent_file_save_events(&reply);
    serde_json::to_value(reply).map_err(|e| e.to_string())
}
