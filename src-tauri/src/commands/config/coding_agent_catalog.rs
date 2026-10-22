use std::path::PathBuf;
use tauri::{AppHandle, State};

use crate::config::settings::{AppSettings, SettingsState};

/// #769 Phase 1 + #1318 - return the externalized coding-agent catalog for the onboarding
/// and Settings pick lists. Resolves against the PRIMARY registered project's
/// `.ac/coding-agents` catalog; with no registered project it falls back to the
/// legacy `<config_dir>/coding-agents` catalog when one exists (pre-migration
/// installs keep today's read behavior).
/// #1967 P4 contract: this is a persisted-only read. `Ok(vec![])` is honored
/// verbatim when the user removed every built-in, and `Err` (code/path/reason)
/// is returned when no readable persisted catalog exists - a missing or
/// unparseable `agents.json` is NEVER replaced with the embedded default, which
/// is seed material only. Warnings are logged here and carried structurally by
/// `get_coding_agent_catalog_report`. #1912: a `false` row in
/// `BUILTIN_AGENT_SUPPORT` (`src-tauri/src/config/coding_agents_catalog.rs`) is
/// already dropped via the persisted read gate.
#[tauri::command]
pub async fn get_coding_agent_catalog(
    settings: State<'_, SettingsState>,
) -> Result<Vec<crate::config::coding_agents_catalog::CodingAgentDefinition>, String> {
    coding_agent_catalog_inner(settings.inner()).await
}

/// #1551 - shared by the Tauri command and the WebSocket router. #1967 P4 -
/// resolves through the persisted-only report: a readable catalog is `Ok(Vec)`
/// (a valid empty list included) and an unavailable persisted source is `Err`
/// carrying the diagnostic's code/path/reason. The embedded default is NEVER
/// substituted on this path. Takes one settings snapshot and releases the async
/// settings lock BEFORE any filesystem work (mirrors
/// `coding_agent_catalog_report_inner`).
pub async fn coding_agent_catalog_inner(
    settings: &SettingsState,
) -> Result<Vec<crate::config::coding_agents_catalog::CodingAgentDefinition>, String> {
    let snapshot = settings.read().await.clone();
    crate::config::coding_agents_catalog::load_catalog_for_settings(&snapshot)
        .map_err(|unavailable| unavailable.to_string())
}

/// #2736 - per catalog entry: installed (PATH only, no process spawn), the
/// code-only tested level and this host's install command. Read-only; an
/// unavailable persisted catalog is `Err` exactly as `get_coding_agent_catalog`.
#[tauri::command]
pub async fn get_coding_agent_welcome_status(
    settings: State<'_, SettingsState>,
) -> Result<Vec<crate::config::coding_agents_catalog::CodingAgentWelcomeStatus>, String> {
    coding_agent_welcome_status_inner(settings.inner()).await
}

/// #2736 - shared by the Tauri command and the WebSocket router. Takes one
/// settings snapshot and releases the async settings lock BEFORE any filesystem
/// work (mirrors `coding_agent_catalog_inner`). The embedded default is NEVER
/// substituted; never seeds, refreshes or writes.
pub async fn coding_agent_welcome_status_inner(
    settings: &SettingsState,
) -> Result<Vec<crate::config::coding_agents_catalog::CodingAgentWelcomeStatus>, String> {
    coding_agent_welcome_status_inner_with(settings, |snapshot| {
        crate::config::coding_agents_catalog::load_catalog_for_settings(snapshot)
            .map_err(|unavailable| unavailable.to_string())
    })
    .await
}

/// Testable twin: `load` carries ALL filesystem work, so a test can observe
/// the settings lock state at that step.
pub(crate) async fn coding_agent_welcome_status_inner_with(
    settings: &SettingsState,
    load: impl FnOnce(
        &AppSettings,
    ) -> Result<
        Vec<crate::config::coding_agents_catalog::CodingAgentDefinition>,
        String,
    >,
) -> Result<Vec<crate::config::coding_agents_catalog::CodingAgentWelcomeStatus>, String> {
    let snapshot = settings.read().await.clone();
    let catalog = load(&snapshot)?;
    Ok(crate::config::coding_agents_catalog::welcome_status_for(
        &catalog,
    ))
}

/// #2736 - run the catalog entry's install command for THIS host with no
/// visible window. Returns as soon as the install task is spawned; the outcome
/// arrives as the `coding_agent_install_finished` event.
#[tauri::command]
pub async fn install_coding_agent(
    app: AppHandle,
    settings: State<'_, SettingsState>,
    key: String,
) -> Result<(), String> {
    install_coding_agent_inner(&app, settings.inner(), key).await
}

/// #2736 - shared by the Tauri command and the WebSocket router. Never awaits
/// the install itself.
pub async fn install_coding_agent_inner(
    app: &AppHandle,
    settings: &SettingsState,
    key: String,
) -> Result<(), String> {
    install_coding_agent_inner_with(app, settings, key, |snapshot| {
        crate::config::coding_agents_catalog::load_catalog_for_settings(snapshot)
            .map_err(|unavailable| unavailable.to_string())
    })
    .await
}

/// Testable twin: `load` carries the catalog read, so a test can supply a
/// fixture catalog without writing one to disk.
pub(crate) async fn install_coding_agent_inner_with(
    app: &AppHandle,
    settings: &SettingsState,
    key: String,
    load: impl FnOnce(
        &AppSettings,
    ) -> Result<
        Vec<crate::config::coding_agents_catalog::CodingAgentDefinition>,
        String,
    >,
) -> Result<(), String> {
    let snapshot = settings.read().await.clone();
    let catalog = load(&snapshot)?;
    let entry = catalog
        .iter()
        .find(|entry| entry.key == key)
        .ok_or_else(|| format!("unknown coding agent '{key}'"))?;
    let command = entry
        .install_commands
        .as_ref()
        .map(crate::config::coding_agents_catalog::resolve_install_command)
        .ok_or_else(|| format!("no install command is available for '{key}' on this platform"))?
        .to_string();
    let cwd = install_cwd_for(&snapshot);
    crate::agent_update::start_agent_install(app, key, command, cwd)
}

/// #2736 - the install cwd, same precedence the updater uses. Pure over the
/// snapshot so test 15 can assert the precedence without spawning anything.
pub(crate) fn install_cwd_for(snapshot: &AppSettings) -> PathBuf {
    crate::config::coding_agents_catalog::primary_project_root(snapshot)
        .or_else(crate::config::config_dir)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// #1963 (P1) - read-only persisted-catalog report for the pre-migration UI.
/// Additive IPC: since P4 the array endpoint above resolves through the SAME
/// resolver, so both agree about availability and the persisted selection.
/// Always `Ok`; ordinary filesystem failure is carried by the report's
/// `unavailable` field, and `Err` is reserved for a transport fault.
#[tauri::command]
pub async fn get_coding_agent_catalog_report(
    settings: State<'_, SettingsState>,
) -> Result<crate::config::coding_agents_catalog::CatalogReport, String> {
    Ok(coding_agent_catalog_report_inner(settings.inner()).await)
}

/// #1963 (P1) - shared by the Tauri command and the WebSocket router. Takes one
/// settings snapshot and releases the async settings lock BEFORE any filesystem
/// work, so the report resolver never holds the settings read guard across I/O.
/// Read-only: never seeds, refreshes or writes.
pub async fn coding_agent_catalog_report_inner(
    settings: &SettingsState,
) -> crate::config::coding_agents_catalog::CatalogReport {
    let snapshot = settings.read().await.clone();
    crate::config::coding_agents_catalog::load_catalog_report_for_settings(&snapshot)
}

/// #769 Phase 2 - the coding-agent command executable basenames that ship a
/// re-seedable default config-folder master (`claude`, `codex`, `opencode`). The
/// frontend enables the "Re-seed default configuration" button only for a catalog
/// def whose command reduces (exact basename) to one of these; the reseed command
/// re-checks server-side.
#[tauri::command]
pub fn list_reseedable_agent_commands() -> Vec<String> {
    crate::config::coding_agents_catalog::reseedable_command_basenames()
}

/// #769 Phase 2 + #1318 - restore a built-in's shipped default config-folder master (the
/// Settings "Re-seed default configuration" button). Resolves the master from the
/// PRIMARY registered project's `.ac/coding-agents/_seed/<dest>`; with NO primary
/// root it falls back to the legacy `<config_dir>/coding-agents/_seed/<dest>`
/// masters (pre-migration installs keep the button working). Gating is re-checked
/// server-side (exact executable basename, never `starts_with`, so `pi`/`agent`
/// cannot false-match): `command` must map to a built-in that ships a master, else
/// `Err`. On success the current master is `.bak`ed first, then atomically
/// replaced with the embedded default; it takes effect on NEW sessions via the
/// absent-only fill. Running replicas and their live config are untouched.
#[tauri::command]
pub async fn reseed_coding_agent_default(
    settings: State<'_, SettingsState>,
    command: String,
) -> Result<crate::config::coding_agents_catalog::ReseedResult, String> {
    let settings = settings.read().await;
    let ac_dir = crate::config::coding_agents_catalog::primary_project_root(&settings)
        .map(|root| root.join(".ac"));
    match ac_dir {
        Some(ac_dir) => {
            crate::config::coding_agents_catalog::reseed_master_for_command(&ac_dir, &command)
        }
        None => {
            let config_dir = crate::config::config_dir().ok_or("No config dir")?;
            crate::config::coding_agents_catalog::reseed_master_for_command(&config_dir, &command)
        }
    }
}
