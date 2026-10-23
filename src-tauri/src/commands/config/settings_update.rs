use std::collections::BTreeMap;
use std::path::Path;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::config::settings::{
    merge_protected_coding_agent_settings, normalize_agent_order, save_settings,
    validate_and_repair_settings, AgentConfig, AppSettings, CodingAgentEnv, SettingsState,
};
use crate::web::broadcast::WsBroadcaster;

use super::{apply_and_broadcast_log_level, SettingsDraftUpdateEvents};

#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    new_settings: AppSettings,
) -> Result<(), String> {
    update_settings_with_hotkey_registration(
        app,
        settings.inner(),
        new_settings,
        crate::screenshot::register_configured_hotkey,
    )
    .await
}

pub(crate) async fn update_settings_with_hotkey_registration(
    app: AppHandle,
    settings: &SettingsState,
    new_settings: AppSettings,
    register_hotkey: fn(&AppHandle, &str) -> Result<(), String>,
) -> Result<(), String> {
    let tracker = app
        .try_state::<crate::pty::manager::ResponseCloseState>()
        .map(|state| state.inner().clone());
    let (saved, changed) =
        persist_protected_settings_update_with_publication(settings, new_settings, |written| {
            if let Some(tracker) = &tracker {
                tracker.apply_settings(
                    written.response_close_enabled,
                    written.response_close_idle_seconds,
                    std::time::Instant::now(),
                );
            }
        })
        .await?;
    emit_room_idle_confirmation_updated(&app, changed);
    purge_sessions_after_settings_update(&saved).await;
    // #714 re-register the (possibly changed) hotkey. Syntax was already validated
    // before persistence; an OS-level registration conflict must NOT turn this
    // successfully-persisted save into an Err. Surface it as a visible event.
    if let Err(e) = register_hotkey(&app, &saved.screenshot_capture_hotkey) {
        let _ = app.emit(
            "screenshot_capture_failed",
            serde_json::json!({
                "message": format!("Screenshot hotkey was saved but could not be registered: {}", e)
            }),
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn save_settings_draft(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    draft: AppSettings,
) -> Result<(), String> {
    save_settings_draft_with_hotkey_registration(
        app,
        settings.inner(),
        draft,
        crate::screenshot::register_configured_hotkey,
    )
    .await
}

pub(crate) async fn save_settings_draft_with_hotkey_registration(
    app: AppHandle,
    settings: &SettingsState,
    draft: AppSettings,
    register_hotkey: fn(&AppHandle, &str) -> Result<(), String>,
) -> Result<(), String> {
    let tracker = app
        .try_state::<crate::pty::manager::ResponseCloseState>()
        .map(|state| state.inner().clone());
    let (saved, events) =
        persist_settings_draft_update_with_publication(settings, draft, |written| {
            if let Some(tracker) = &tracker {
                tracker.apply_settings(
                    written.response_close_enabled,
                    written.response_close_idle_seconds,
                    std::time::Instant::now(),
                );
            }
        })
        .await?;
    emit_room_idle_confirmation_updated(&app, events.room_idle_confirmation_changed);
    purge_sessions_after_settings_update(&saved).await;
    // #714 same non-failing hotkey re-registration as update_settings: persisted
    // save stays successful even if the OS refuses the hotkey.
    if let Err(e) = register_hotkey(&app, &saved.screenshot_capture_hotkey) {
        let _ = app.emit(
            "screenshot_capture_failed",
            serde_json::json!({
                "message": format!("Screenshot hotkey was saved but could not be registered: {}", e)
            }),
        );
    }
    emit_settings_draft_update_events(&app, &events);
    // #612 apply the (possibly changed) log level live + broadcast so every
    // webview re-applies its console gate. Idempotent and cheap; runs only on an
    // explicit Settings Save. `saved` is the binding already destructured from
    // the persist call above, so this does NOT re-persist.
    let level = crate::logging::normalize_log_level(saved.log_level.as_deref().unwrap_or("info"))
        .unwrap_or("info");
    apply_and_broadcast_log_level(&app, level);
    Ok(())
}

#[tauri::command]
pub async fn set_terminal_snapshots_enabled(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    expected: bool,
    enabled: bool,
) -> Result<(), String> {
    let tracker = app
        .try_state::<crate::pty::manager::ResponseCloseState>()
        .map(|state| state.inner().clone());
    set_terminal_snapshots_enabled_with_publication(
        settings.inner(),
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
    .await
}

/// Shared #1173 owner used by native Tauri and browser WebSocket transports.
/// Whole-settings writers preserve this field and cannot call this owner
/// without an explicit expected value.
// Compatibility entry point for tests and dormant callers without an app tracker.
#[allow(dead_code)]
pub(crate) async fn set_terminal_snapshots_enabled_inner(
    settings: &SettingsState,
    expected: bool,
    enabled: bool,
) -> Result<(), String> {
    set_terminal_snapshots_enabled_with_publication(settings, expected, enabled, |_| {}).await
}

pub(crate) async fn set_terminal_snapshots_enabled_with_publication(
    settings: &SettingsState,
    expected: bool,
    enabled: bool,
    mut publish: impl FnMut(&AppSettings),
) -> Result<(), String> {
    let mut guard = settings.write().await;
    let written = crate::config::settings::compare_and_set_terminal_snapshots_enabled(
        &guard, expected, enabled,
    )?;
    publish(&written);
    *guard = written;
    Ok(())
}

// Compatibility entry point for tests and dormant callers without an app tracker.
#[allow(dead_code)]
pub(crate) async fn persist_protected_settings_update(
    settings: &SettingsState,
    new_settings: AppSettings,
) -> Result<(AppSettings, bool), String> {
    persist_protected_settings_update_with_saver(settings, new_settings, save_settings).await
}

pub(crate) async fn persist_protected_settings_update_with_publication(
    settings: &SettingsState,
    new_settings: AppSettings,
    publish: impl FnMut(&AppSettings),
) -> Result<(AppSettings, bool), String> {
    persist_protected_settings_update_with_saver_and_publication(
        settings,
        new_settings,
        save_settings,
        publish,
    )
    .await
}

pub(crate) async fn persist_protected_settings_update_with_saver(
    settings: &SettingsState,
    new_settings: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(AppSettings, bool), String> {
    persist_protected_settings_update_with_saver_and_publication(
        settings,
        new_settings,
        save,
        |_| {},
    )
    .await
}

/// Publish the effective saved value before adoption under settings ownership.
/// The synchronous callback must not acquire the settings lock.
pub(crate) async fn persist_protected_settings_update_with_saver_and_publication(
    settings: &SettingsState,
    new_settings: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
    mut publish: impl FnMut(&AppSettings),
) -> Result<(AppSettings, bool), String> {
    let mut s = settings.write().await;
    let current = s.clone();
    let candidate = build_protected_settings_candidate(&current, new_settings)?;
    let written = save(&candidate)?;
    let changed = current.room_idle_confirmation_seconds != written.room_idle_confirmation_seconds;
    publish(&written);
    *s = written.clone();
    Ok((written, changed))
}

/// #2306 P2 - make the live effective order authoritative over a whole-settings
/// payload, without letting the payload's explicit ordinals reorder anything.
///
/// Both whole-settings writers call this after their protected-field restores
/// and before validation, event derivation and save. The live in-memory vector
/// is the effective order; the incoming payload may be a snapshot that predates
/// a move, so its `order` values carry no authority:
///
///   1. every incoming record whose exact id exists live survives, ordered by
///      its live position (records sharing a live rank keep incoming order);
///   2. incoming-only ids are appended in incoming order;
///   3. records and membership come from the incoming payload, so a user edit,
///      add or remove still applies;
///   4. `order` is finally renumbered by post-merge vector position.
///
/// A stale draft/update therefore cannot undo an ordered move for a surviving
/// id, and a forged incoming ordinal cannot sort the vector. Known limitation
/// (accepted, per the phase plan): membership is still the incoming payload's,
/// so a stale snapshot that predates an independent add omits that id here;
/// this merge does not resurrect it.
pub(crate) fn merge_live_agent_order(current: &AppSettings, incoming: &mut AppSettings) {
    let mut survivors: Vec<(usize, AgentConfig)> = Vec::new();
    let mut appended: Vec<AgentConfig> = Vec::new();
    for record in incoming.agents.drain(..) {
        match current.agents.iter().position(|live| live.id == record.id) {
            Some(live_rank) => survivors.push((live_rank, record)),
            None => appended.push(record),
        }
    }
    // Stable sort: records sharing a live rank (duplicate ids) keep incoming order.
    survivors.sort_by_key(|(live_rank, _)| *live_rank);
    let mut merged: Vec<AgentConfig> = survivors.into_iter().map(|(_, record)| record).collect();
    merged.extend(appended);
    incoming.agents = merged;
    normalize_agent_order(&mut incoming.agents);
}

pub(crate) fn build_protected_settings_candidate(
    current: &AppSettings,
    new_settings: AppSettings,
) -> Result<AppSettings, String> {
    let mut candidate = merge_protected_coding_agent_settings(current, new_settings);
    // Preserve existing root token. Frontend settings payloads cannot overwrite it.
    candidate.root_token = current.root_token.clone();
    // #881 A7 / R2-G1: project lists are disk-authoritative and are mutated
    // only by dedicated project commands. A settings payload from the GUI, CLI,
    // or API must never carry authority for them, so restore all three from
    // live memory before repair.
    //
    // This is not the prohibited disk re-read. The preserving writer still
    // overwrites all three from disk whenever disk has an opinion, so disk
    // authority is untouched. These copies only matter on the no-disk-truth
    // arms where a stale client would otherwise publish an empty list.
    candidate.project_paths = current.project_paths.clone();
    candidate.project_path = current.project_path.clone();
    candidate.archived_project_paths = current.archived_project_paths.clone();
    // #1077: restore the hidden project-path pair state too, so a returned
    // SettingsSnapshot echoed back as an update cannot erase or re-pair the
    // companion metadata. The disk serializer builds project fields only from
    // this protected state (or disk truth), never from the incoming payload.
    candidate.project_path_state = current.project_path_state.clone();
    // #1737: `local_overlay_state` is `#[serde(skip)]`, so a payload that arrived from
    // the webview or the WebSocket transport decodes it as an empty overlay. Restore
    // it from live memory, exactly like `project_path_state` above: without this the
    // save that follows would find nothing to restore, would write the overlay value
    // into settings.json, and `*s = written.clone()` would install the empty overlay
    // so every later save is unprotected too. General rule (plan D14): any AppSettings
    // that did not come out of `parse_settings_json` and is later saved must inherit
    // the live overlay state first.
    candidate.local_overlay_state = current.local_overlay_state.clone();
    // #965: rail collapse is mutated only by `set_rail_collapse`. A settings payload
    // from the GUI, CLI, or API must never carry authority for it, so restore both
    // fields from live memory. Same rule and same mechanism as the project lists
    // above; this is not the prohibited disk re-read.
    candidate.rail_collapsed_projects = current.rail_collapsed_projects.clone();
    candidate.rail_favorites_collapsed = current.rail_favorites_collapsed;
    // #1173: terminal snapshot disclosure is owned only by its dedicated CAS.
    candidate.terminal_snapshots_enabled = current.terminal_snapshots_enabled;
    merge_live_agent_order(current, &mut candidate);
    validate_and_repair_settings(&mut candidate)?;
    Ok(candidate)
}

// Compatibility entry point for tests and dormant callers without an app tracker.
#[allow(dead_code)]
pub(crate) async fn persist_settings_draft_update(
    settings: &SettingsState,
    draft: AppSettings,
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    persist_settings_draft_update_with_saver(settings, draft, save_settings).await
}

pub(crate) async fn persist_settings_draft_update_with_publication(
    settings: &SettingsState,
    draft: AppSettings,
    publish: impl FnMut(&AppSettings),
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    persist_settings_draft_update_with_saver_and_publication(
        settings,
        draft,
        save_settings,
        publish,
    )
    .await
}

pub(crate) async fn persist_settings_draft_update_with_saver(
    settings: &SettingsState,
    draft: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    persist_settings_draft_update_with_saver_and_publication(settings, draft, save, |_| {}).await
}

/// Publish the effective saved value before adoption under settings ownership.
/// The synchronous callback must not acquire the settings lock.
pub(crate) async fn persist_settings_draft_update_with_saver_and_publication(
    settings: &SettingsState,
    mut draft: AppSettings,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
    mut publish: impl FnMut(&AppSettings),
) -> Result<(AppSettings, SettingsDraftUpdateEvents), String> {
    let mut s = settings.write().await;
    let current = s.clone();
    draft.root_token = current.root_token.clone();
    // #881 A7 / R2-G1: same protected-list restore as the whole settings
    // publisher above. This is an in-memory copy from the held settings guard,
    // not a second disk read, and it only affects the no-disk-truth arms.
    draft.project_paths = current.project_paths.clone();
    draft.project_path = current.project_path.clone();
    draft.archived_project_paths = current.archived_project_paths.clone();
    // #1077: restore the hidden project-path pair state (see the protected
    // candidate builder above).
    draft.project_path_state = current.project_path_state.clone();
    // #1737: the same overlay-state carry-over as in
    // `build_protected_settings_candidate` above, and for the same reason (plan D14).
    draft.local_overlay_state = current.local_overlay_state.clone();
    draft.coding_agent_identity_physical = current.coding_agent_identity_physical.clone();
    // #965: same protect as `build_protected_settings_candidate`. The SettingsModal
    // Save path lands here, and so does every whole-object writer that reads
    // `settingsStore.current` first (window geometry, zoom, titlebar...). Rail
    // collapse is owned by `set_rail_collapse`; restore it from live memory.
    draft.rail_collapsed_projects = current.rail_collapsed_projects.clone();
    draft.rail_favorites_collapsed = current.rail_favorites_collapsed;
    // #1173: a stale whole-settings draft has no disclosure-gate authority.
    draft.terminal_snapshots_enabled = current.terminal_snapshots_enabled;
    merge_live_agent_order(&current, &mut draft);
    validate_and_repair_settings(&mut draft)?;
    let mut events = settings_draft_update_events(&current, &draft);
    let written = save(&draft)?;
    events.room_idle_confirmation_changed =
        current.room_idle_confirmation_seconds != written.room_idle_confirmation_seconds;
    publish(&written);
    *s = written.clone();
    Ok((written, events))
}

pub(crate) async fn purge_sessions_after_settings_update(saved: &AppSettings) {
    let Some(dir) = crate::config::config_dir() else {
        log::warn!("Could not determine home directory for session purge after settings update");
        return;
    };

    if let Err(e) = purge_sessions_after_settings_update_in_dir(saved, &dir).await {
        log::warn!(
            "[settings] Failed to purge sessions outside current projectPaths after settings \
                update: {}",
            e
        );
    }
}

pub(crate) async fn purge_sessions_after_settings_update_in_dir(
    saved: &AppSettings,
    dir: &Path,
) -> Result<(), String> {
    let retention_paths =
        crate::config::sessions_persistence::session_retention_project_paths(saved)?;
    crate::config::sessions_persistence::purge_sessions_outside_project_paths_in_dir(
        dir,
        &retention_paths,
    )
    .await
    .map(|_| ())
}

pub(crate) fn settings_draft_update_events(
    before: &AppSettings,
    after: &AppSettings,
) -> SettingsDraftUpdateEvents {
    let before_envs = agent_env_settings_by_id(before);
    let after_envs = agent_env_settings_by_id(after);
    let mut env_agent_ids = Vec::new();

    for agent_id in before_envs.keys().chain(after_envs.keys()) {
        if env_agent_ids.iter().any(|existing| existing == agent_id) {
            continue;
        }
        if before_envs.get(agent_id) != after_envs.get(agent_id) {
            env_agent_ids.push(agent_id.clone());
        }
    }

    let mut before_profiles = before.coding_agent_profiles.clone();
    let mut after_profiles = after.coding_agent_profiles.clone();
    for profiles in [&mut before_profiles, &mut after_profiles] {
        for cells in profiles.profiles_by_agent.values_mut() {
            for cell in cells.values_mut() {
                cell.id_hash = None;
                cell.id_hash_debug = None;
            }
        }
    }

    SettingsDraftUpdateEvents {
        room_idle_confirmation_changed: false,
        // #592/#597 - the drift content-hash fingerprints the EFFECTIVE command
        // (agent base command + cell command, via compose_effective_command), so a
        // bare agent-base-command edit (e.g. `claude` -> `claude-amp -c ...`) is real
        // drift even when the profile cells are untouched. It is neither a
        // coding_agent_profiles change nor an env change, so without folding it in
        // here the SettingsModal save path emits NO refresh event and the outdated
        // badge never updates. coding_agent_profiles_updated drives
        // refreshProfileOutdated in the sidebar, so route base-command edits through it.
        profiles_changed: before_profiles != after_profiles
            || agent_commands_by_id(before) != agent_commands_by_id(after),
        env_agent_ids,
    }
}

pub(crate) fn agent_env_settings_by_id(
    settings: &AppSettings,
) -> BTreeMap<String, (Vec<CodingAgentEnv>, bool)> {
    settings
        .agents
        .iter()
        .map(|agent| (agent.id.clone(), (agent.envs.clone(), agent.isolated_home)))
        .collect()
}

/// #592/#597 - per-agent base command, the other half of the drift hash input
/// (`compose_effective_command(agent.command, cell.command)`). A change here must
/// trigger a drift refresh just like a profile-cell edit.
pub(crate) fn agent_commands_by_id(settings: &AppSettings) -> BTreeMap<String, String> {
    settings
        .agents
        .iter()
        .map(|agent| (agent.id.clone(), agent.command.clone()))
        .collect()
}

pub(crate) fn emit_room_idle_confirmation_updated(app: &AppHandle, changed: bool) {
    if !changed {
        return;
    }
    let payload = serde_json::json!({});
    if let Err(error) = app.emit("room_idle_confirmation_updated", payload.clone()) {
        log::warn!("Failed to emit room idle confirmation update: {}", error);
    }
    if let Some(broadcaster) = app.try_state::<WsBroadcaster>() {
        broadcaster.broadcast_event("room_idle_confirmation_updated", &payload);
    }
}

pub(crate) fn emit_settings_draft_update_events(
    app: &AppHandle,
    events: &SettingsDraftUpdateEvents,
) {
    if events.profiles_changed {
        // #592/#597: the SettingsModal "Save" persists profile cells AND agent base
        // commands through save_settings_draft (NOT update_coding_agent_profiles), so
        // THIS is the emit point the user's normal edit flow actually reaches. Kept at
        // debug to confirm the refresh fired without spamming app.log.
        log::debug!(
            "[profile-hash] profile-save (save_settings_draft): coding-agent config changed; \
                emitting coding_agent_profiles_updated"
        );
        let _ = app.emit("coding_agent_profiles_updated", serde_json::json!({}));
    }

    for agent_id in &events.env_agent_ids {
        log::debug!(
            "[profile-hash] profile-save (save_settings_draft): agent env changed for {}; emitting \
                coding_agent_env_settings_updated",
            agent_id
        );
        let _ = app.emit(
            "coding_agent_env_settings_updated",
            serde_json::json!({ "agentId": agent_id }),
        );
    }
}
