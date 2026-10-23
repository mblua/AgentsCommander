use tauri::{AppHandle, Emitter, State};

use crate::config::coding_agent_mutations::{
    move_registered_agent, reorder_registered_agent, AgentMoveDirection,
};
use crate::config::settings::{save_settings, AppSettings, SettingsState, OVERLAY_KEY_AGENTS};

/// Returns the runtime instance label for the titlebar badge.
/// E.g. "STAGE", "STANDALONE", or "" for prod (no badge).
#[tauri::command]
pub fn get_instance_label() -> String {
    crate::config::profile::instance_label().to_string()
}

/// `pub(crate)` since #1171: `set_watchers_geometry` lives in `commands/window.rs` beside the
/// window it belongs to, and must write its one field through this same candidate-save-publish
/// path rather than a second one of its own.
pub(crate) async fn persist_narrow_settings_update(
    settings: &SettingsState,
    mutate_candidate: impl FnOnce(&mut AppSettings),
) -> Result<(), String> {
    persist_narrow_settings_update_with_saver(settings, mutate_candidate, save_settings).await
}

pub(crate) async fn persist_narrow_settings_update_with_saver(
    settings: &SettingsState,
    mutate_candidate: impl FnOnce(&mut AppSettings),
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(), String> {
    let mut s = settings.write().await;
    let mut candidate = s.clone();
    mutate_candidate(&mut candidate);
    let written = save(&candidate)?;
    *s = written;
    Ok(())
}

/// #2306 P2 - the ordered-move rejection while the local overlay owns the whole
/// top-level `agents` key. Stable token shared by the Tauri command and the
/// browser route (transport parity).
pub(crate) const AGENT_ORDER_OVERLAY_PINNED: &str =
    "agent order is controlled by the local settings overlay (settings.local.json)";

/// #2306 P2 - fallible sibling of `persist_narrow_settings_update_with_saver`,
/// used by the ordered move.
///
/// Same candidate-save-publish pattern with two differences:
///
///   * the mutation is fallible, so a rejected request (boundary move, unknown
///     or non-adjacent neighbor) returns before the saver runs and leaves live
///     settings untouched;
///   * while the local overlay owns the top-level `agents` key the request is
///     rejected before the clone. `save_settings_value_locked` restores the
///     overlay's base array to `settings.json` and reapplies the overlay-
///     effective array only to memory, so no move could survive a restart and a
///     "successful" move would publish a transient order. The check runs under
///     the write guard, so an overlay appearing concurrently cannot slip
///     between the check and the save.
///
/// On success it installs the written snapshot and returns it, so the caller
/// derives the authoritative ids (and ordinals) from what was persisted.
pub(crate) async fn persist_narrow_settings_update_fallible_with_saver(
    settings: &SettingsState,
    mutate_candidate: impl FnOnce(&mut AppSettings) -> Result<(), String>,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<AppSettings, String> {
    let mut s = settings.write().await;
    if s.local_overlay_state.owns_top_level(OVERLAY_KEY_AGENTS) {
        return Err(AGENT_ORDER_OVERLAY_PINNED.to_string());
    }
    let mut candidate = s.clone();
    mutate_candidate(&mut candidate)?;
    let written = save(&candidate)?;
    *s = written.clone();
    Ok(written)
}

/// Narrow setter for `sounds_enabled`. Same candidate-save-publish
/// pattern as the other narrow setters (issue #158). Replaces the toolbar's
/// previous full-object `update_settings(next)` call, which could clobber
/// unrelated fields from a stale `settingsStore.current` snapshot.
#[tauri::command]
pub async fn set_sounds_enabled(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.sounds_enabled = value;
    })
    .await
}

/// Narrow setter for `theme_light`. Same candidate-save-publish
/// pattern as `set_sounds_enabled` (issue #289). Lets the UI persist the
/// user's light/dark mode choice without going through `update_settings`,
/// which could clobber unrelated fields from a stale snapshot.
#[tauri::command]
pub async fn set_theme_light(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.theme_light = value;
    })
    .await
}

/// Narrow setter for `main_resource_monitor_attached` (#587). Same
/// candidate-save-publish pattern as `set_theme_light`; lets the central-view
/// toggle persist without a get+update round-trip racing SettingsModal.
#[tauri::command]
pub async fn set_main_resource_monitor_attached(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.main_resource_monitor_attached = value;
    })
    .await
}

/// #965 narrow setter for the rail collapse snapshot. Same candidate-save-publish
/// pattern as `set_theme_light`: holds the `SettingsState` write lock across
/// `save_settings`, so it cannot lose an interleaved `update_settings`.
/// ONE command for both fields: the rail always persists its whole snapshot, so a
/// per-field setter would just double the writes. Called once per explicit header
/// click (no auto-focus path writes here), so it is cold by construction.
///
/// The `_inner` split exists so the web/WS route (`web/commands.rs`) reaches the
/// same read-modify-save instead of duplicating it; the browser client mounts the
/// rail too. Precedent: `set_agent_default_profile_inner`.
pub(crate) async fn set_rail_collapse_inner(
    settings: &SettingsState,
    collapsed_projects: Vec<String>,
    favorites_collapsed: bool,
) -> Result<(), String> {
    set_rail_collapse_inner_with_saver(
        settings,
        collapsed_projects,
        favorites_collapsed,
        save_settings,
    )
    .await
}

/// Saver seam for `set_rail_collapse_inner`, mirroring the three existing
/// `*_with_saver` pairs in this module. It exists so a unit test can drive the REAL
/// mutation without reaching `save_settings` -> `config::config_dir()`, a `OnceLock`
/// that no test redirects and that would overwrite the developer's own settings.json.
pub(crate) async fn set_rail_collapse_inner_with_saver(
    settings: &SettingsState,
    collapsed_projects: Vec<String>,
    favorites_collapsed: bool,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<(), String> {
    persist_narrow_settings_update_with_saver(
        settings,
        |candidate| {
            candidate.rail_collapsed_projects = collapsed_projects;
            candidate.rail_favorites_collapsed = favorites_collapsed;
        },
        save,
    )
    .await
}

#[tauri::command]
pub async fn set_rail_collapse(
    settings: State<'_, SettingsState>,
    collapsed_projects: Vec<String>,
    favorites_collapsed: bool,
) -> Result<(), String> {
    set_rail_collapse_inner(settings.inner(), collapsed_projects, favorites_collapsed).await
}

/// #2306 P2 - narrow owner of the registered-agent move. Mutates only the
/// `agents` vector under the settings write guard, persists through the
/// preserving `save_settings`, and returns the authoritative ordered ids.
#[tauri::command]
pub async fn move_coding_agent<R: tauri::Runtime>(
    app: AppHandle<R>,
    settings: State<'_, SettingsState>,
    id: String,
    neighbor_id: String,
    direction: AgentMoveDirection,
) -> Result<Vec<String>, String> {
    move_coding_agent_command_with_saver(
        &app,
        settings.inner(),
        id,
        neighbor_id,
        direction,
        save_settings,
    )
    .await
}

/// Shared Tauri-command body. The settings write guard is released when the
/// inner returns, so `coding_agent_settings_updated` is emitted after unlock.
/// Split so a test can drive the real emit with a temp-path saver.
pub(crate) async fn move_coding_agent_command_with_saver<R: tauri::Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsState,
    id: String,
    neighbor_id: String,
    direction: AgentMoveDirection,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<Vec<String>, String> {
    let ids =
        move_coding_agent_inner_with_saver(settings, &id, &neighbor_id, direction, save).await?;
    let _ = app.emit(
        "coding_agent_settings_updated",
        move_coding_agent_event_payload(&id),
    );
    Ok(ids)
}

/// The shared inner owner of the move, used by the Tauri command and the
/// browser WebSocket route.
pub(crate) async fn move_coding_agent_inner_with_saver(
    settings: &SettingsState,
    id: &str,
    neighbor_id: &str,
    direction: AgentMoveDirection,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<Vec<String>, String> {
    let written = persist_narrow_settings_update_fallible_with_saver(
        settings,
        |candidate| move_registered_agent(candidate, id, neighbor_id, direction).map(|_| ()),
        save,
    )
    .await?;
    Ok(written
        .agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect())
}

/// #2542 - narrow owner of the one-step reorder: moves `id` to `target_index`
/// when `expected_ids` still matches the effective order. Mutates only the
/// `agents` vector under the settings write guard and persists through the
/// preserving `save_settings`.
#[tauri::command]
pub async fn reorder_coding_agent<R: tauri::Runtime>(
    app: AppHandle<R>,
    settings: State<'_, SettingsState>,
    id: String,
    expected_ids: Vec<String>,
    target_index: usize,
) -> Result<Vec<String>, String> {
    reorder_coding_agent_command_with_saver(
        &app,
        settings.inner(),
        id,
        expected_ids,
        target_index,
        save_settings,
    )
    .await
}

/// Shared Tauri-command body for the reorder; emits the move's event payload
/// after the inner has released the settings write guard.
pub(crate) async fn reorder_coding_agent_command_with_saver<R: tauri::Runtime>(
    app: &AppHandle<R>,
    settings: &SettingsState,
    id: String,
    expected_ids: Vec<String>,
    target_index: usize,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<Vec<String>, String> {
    let ids =
        reorder_coding_agent_inner_with_saver(settings, &id, &expected_ids, target_index, save)
            .await?;
    let _ = app.emit(
        "coding_agent_settings_updated",
        move_coding_agent_event_payload(&id),
    );
    Ok(ids)
}

/// The shared inner owner of the reorder, used by the Tauri command and the
/// browser WebSocket route.
pub(crate) async fn reorder_coding_agent_inner_with_saver(
    settings: &SettingsState,
    id: &str,
    expected_ids: &[String],
    target_index: usize,
    save: impl FnOnce(&AppSettings) -> Result<AppSettings, String>,
) -> Result<Vec<String>, String> {
    let written = persist_narrow_settings_update_fallible_with_saver(
        settings,
        |candidate| reorder_registered_agent(candidate, id, expected_ids, target_index).map(|_| ()),
        save,
    )
    .await?;
    Ok(written
        .agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect())
}

/// One payload shape for the move's `coding_agent_settings_updated` event on
/// both transports (native `app.emit` and browser `broadcast_all`).
pub(crate) fn move_coding_agent_event_payload(id: &str) -> serde_json::Value {
    serde_json::json!({ "op": "move", "agentId": id })
}

/// #612 apply the runtime log level (no-op under RUST_LOG) and broadcast so
/// every webview re-applies its own console gate. Single point that both the
/// dedicated `set_log_level` command and the SettingsModal Save path reuse.
pub(crate) fn apply_and_broadcast_log_level(app: &AppHandle, level: &str) {
    // set_runtime_log_level returns false when the gate is not installed
    // (RUST_LOG dev-override active): the backend stays frozen and only the
    // frontend console gate moves. Emit one debug line so an operator reading
    // app.log isn't confused why the backend verbosity didn't change.
    if !crate::logging::set_runtime_log_level(level) {
        log::debug!(
            "[log-level] RUST_LOG override active; backend frozen, applying '{level}' to frontend \
                only"
        );
    }
    let _ = app.emit("log_level_changed", serde_json::json!({ "level": level }));
}

/// #612 canonical command to change ONLY the log level: validates, persists
/// `logLevel`, applies live, broadcasts. Used by programmatic callers / future
/// quick-switch; the SettingsModal uses the form Save path (`save_settings_draft`).
#[tauri::command]
pub async fn set_log_level(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    level: String,
) -> Result<(), String> {
    let normalized = crate::logging::normalize_log_level(&level)
        .ok_or_else(|| format!("Invalid log level: {level}"))?;
    persist_narrow_settings_update(settings.inner(), |candidate| {
        candidate.log_level = Some(normalized.to_string());
    })
    .await?;
    apply_and_broadcast_log_level(&app, normalized);
    Ok(())
}
