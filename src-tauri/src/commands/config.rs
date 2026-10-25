use std::path::PathBuf;
use std::time::Duration;
use tauri::State;

use crate::network::OutboundNetwork;

mod diagnostics;
pub use diagnostics::{
    __cmd__append_rail_diagnostic, __cmd__drain_error_logs, __cmd__get_error_popup_mutes,
    __cmd__save_debug_logs, __cmd__set_error_popup_muted,
    __tauri_command_name_append_rail_diagnostic, __tauri_command_name_drain_error_logs,
    __tauri_command_name_get_error_popup_mutes, __tauri_command_name_save_debug_logs,
    __tauri_command_name_set_error_popup_muted, append_rail_diagnostic, drain_error_logs,
    get_error_popup_mutes, save_debug_logs, set_error_popup_muted,
};

mod settings_snapshot;
pub(crate) use settings_snapshot::settings_snapshot_helper;
pub use settings_snapshot::{
    __cmd__get_settings, __tauri_command_name_get_settings, get_settings, IssueSource,
    ProjectPathIssue, ProjectPathReconciliationError, ProjectPathResolution, RawJsonFieldState,
    RawStringFieldState, ReconciliationStage, SettingsSnapshot,
};

mod coding_agent_catalog;
pub use coding_agent_catalog::{
    __cmd__get_coding_agent_catalog, __cmd__get_coding_agent_catalog_report,
    __cmd__get_coding_agent_welcome_status, __cmd__install_coding_agent,
    __cmd__list_reseedable_agent_commands, __cmd__reseed_coding_agent_default,
    __tauri_command_name_get_coding_agent_catalog,
    __tauri_command_name_get_coding_agent_catalog_report,
    __tauri_command_name_get_coding_agent_welcome_status,
    __tauri_command_name_install_coding_agent, __tauri_command_name_list_reseedable_agent_commands,
    __tauri_command_name_reseed_coding_agent_default, coding_agent_catalog_inner,
    coding_agent_catalog_report_inner, coding_agent_welcome_status_inner, get_coding_agent_catalog,
    get_coding_agent_catalog_report, get_coding_agent_welcome_status, install_coding_agent,
    install_coding_agent_inner, list_reseedable_agent_commands, reseed_coding_agent_default,
};

mod settings_update;
pub use settings_update::{
    __cmd__save_settings_draft, __cmd__set_terminal_snapshots_enabled, __cmd__update_settings,
    __tauri_command_name_save_settings_draft, __tauri_command_name_set_terminal_snapshots_enabled,
    __tauri_command_name_update_settings, save_settings_draft, set_terminal_snapshots_enabled,
    update_settings,
};
pub(crate) use settings_update::{
    persist_protected_settings_update_with_publication,
    persist_settings_draft_update_with_publication, purge_sessions_after_settings_update,
    set_terminal_snapshots_enabled_with_publication,
};

mod coding_agent_profile;
pub use coding_agent_profile::{
    __cmd__resolve_coding_agent_profile, __cmd__set_agent_default_profile,
    __cmd__set_instance_profile_override, __cmd__update_coding_agent_env_settings,
    __cmd__update_coding_agent_profiles, __tauri_command_name_resolve_coding_agent_profile,
    __tauri_command_name_set_agent_default_profile,
    __tauri_command_name_set_instance_profile_override,
    __tauri_command_name_update_coding_agent_env_settings,
    __tauri_command_name_update_coding_agent_profiles, resolve_coding_agent_profile,
    set_agent_default_profile, set_instance_profile_override, update_coding_agent_env_settings,
    update_coding_agent_profiles,
};
pub(crate) use coding_agent_profile::{
    resolve_coding_agent_profile_inner, set_agent_default_profile_inner,
    set_instance_profile_override_inner,
};

mod profile_selection;
pub use profile_selection::{
    __cmd__apply_coding_agent_profile_selection, __cmd__apply_selection_lock_removal,
    __cmd__get_replica_selection_default, __cmd__preview_coding_agent_profile_selection,
    __cmd__preview_selection_lock_removal, __cmd__set_replica_selection_default,
    __tauri_command_name_apply_coding_agent_profile_selection,
    __tauri_command_name_apply_selection_lock_removal,
    __tauri_command_name_get_replica_selection_default,
    __tauri_command_name_preview_coding_agent_profile_selection,
    __tauri_command_name_preview_selection_lock_removal,
    __tauri_command_name_set_replica_selection_default, apply_coding_agent_profile_selection,
    apply_selection_lock_removal, get_replica_selection_default,
    preview_coding_agent_profile_selection, preview_selection_lock_removal,
    set_replica_selection_default, ApplyCodingAgentProfileSelectionRequest,
    ApplyCodingAgentProfileSelectionResult, ApplySelectionLockRemovalRequest,
    ApplySelectionLockRemovalResult, AssignmentMode, ConflictDecision,
    GetReplicaSelectionDefaultRequest, PreviewCodingAgentProfileSelectionRequest,
    PreviewCodingAgentProfileSelectionResult, PreviewSelectionLockRemovalRequest,
    PreviewSelectionLockRemovalResult, ProfileAssignmentError, ProfileAssignmentScope,
    ProfileAssignmentTarget, ReplicaSelectionDefaultResult, SavedPair, ScopeFault, ScopeFaultCode,
    SelectionDecisionProjection, SelectionDecisions, SelectionDefault, SelectionState,
    SetReplicaSelectionDefaultRequest,
};
pub(crate) use profile_selection::{
    apply_coding_agent_profile_selection_inner, apply_selection_lock_removal_inner,
    get_replica_selection_default_inner, preview_coding_agent_profile_selection_inner,
    preview_selection_lock_removal_inner, set_replica_selection_default_inner,
};

mod web_server;
pub(crate) use self::web_server::begin_web_server_start;
pub use self::web_server::{
    __cmd__api_server_status, __cmd__get_web_server_owned_status, __cmd__get_web_server_status,
    __cmd__list_web_server_interfaces, __cmd__mint_api_client, __cmd__open_web_remote,
    __cmd__start_api_server, __cmd__start_web_server, __cmd__stop_api_server,
    __cmd__stop_web_server, __tauri_command_name_api_server_status,
    __tauri_command_name_get_web_server_owned_status, __tauri_command_name_get_web_server_status,
    __tauri_command_name_list_web_server_interfaces, __tauri_command_name_mint_api_client,
    __tauri_command_name_open_web_remote, __tauri_command_name_start_api_server,
    __tauri_command_name_start_web_server, __tauri_command_name_stop_api_server,
    __tauri_command_name_stop_web_server, api_server_status, get_web_server_owned_status,
    get_web_server_status, list_web_server_interfaces, mint_api_client, open_web_remote,
    start_api_server, start_web_server, stop_api_server, stop_web_server, MintApiClientResponse,
    WebServerBindFailure, WebServerInterfaceInfo, WebServerOwnedStatus, WebServerOwnershipState,
};

mod preferences;
use preferences::apply_and_broadcast_log_level;
pub use preferences::{
    __cmd__get_instance_label, __cmd__move_coding_agent, __cmd__reorder_coding_agent,
    __cmd__set_log_level, __cmd__set_main_resource_monitor_attached, __cmd__set_rail_collapse,
    __cmd__set_sounds_enabled, __cmd__set_theme_light, __tauri_command_name_get_instance_label,
    __tauri_command_name_move_coding_agent, __tauri_command_name_reorder_coding_agent,
    __tauri_command_name_set_log_level, __tauri_command_name_set_main_resource_monitor_attached,
    __tauri_command_name_set_rail_collapse, __tauri_command_name_set_sounds_enabled,
    __tauri_command_name_set_theme_light, get_instance_label, move_coding_agent,
    reorder_coding_agent, set_log_level, set_main_resource_monitor_attached, set_rail_collapse,
    set_sounds_enabled, set_theme_light,
};
pub(crate) use preferences::{
    move_coding_agent_event_payload, move_coding_agent_inner_with_saver,
    persist_narrow_settings_update, reorder_coding_agent_inner_with_saver, set_rail_collapse_inner,
};

mod agent_update;
pub use agent_update::{
    __cmd__agent_update_answer, __cmd__agent_update_cancel, __cmd__agent_updates_cancel_all,
    __cmd__get_agent_update_overview, __cmd__get_agent_update_status, __cmd__get_update_status,
    __tauri_command_name_agent_update_answer, __tauri_command_name_agent_update_cancel,
    __tauri_command_name_agent_updates_cancel_all, __tauri_command_name_get_agent_update_overview,
    __tauri_command_name_get_agent_update_status, __tauri_command_name_get_update_status,
    agent_update_answer, agent_update_answer_inner, agent_update_cancel, agent_update_cancel_inner,
    agent_update_overview_inner, agent_update_status_inner, agent_updates_cancel_all,
    agent_updates_cancel_all_inner, get_agent_update_overview, get_agent_update_status,
    get_update_status,
};

mod agent_file_source;
#[cfg(test)]
use self::web_server::{
    api_server_probe_addr, build_web_server_owned_status, classify_virtual_interface,
    ensure_web_remote_open_allowed, is_tcp_listening, is_tcp_socket_listening,
    map_web_server_interfaces, mint_api_client_with_path, resolve_web_server_owned_status,
    select_current_bind_failure, stop_web_server_handle, web_remote_url, web_server_probe_addr,
    MINT_API_CLIENT_DEFAULT_TTL_HOURS, MINT_API_CLIENT_MAX_TTL_DAYS, MINT_API_CLIENT_NOTE,
};
#[cfg(test)]
use crate::config::instance_artifacts::SETTINGS_FILE_NAME;
#[cfg(test)]
use crate::web::broadcast::WsBroadcaster;
#[cfg(test)]
use agent_file_source::finish_remember_agent_file_report;
pub use agent_file_source::{
    __cmd__list_agent_file_sources, __cmd__read_agent_file_source,
    __cmd__remember_agent_file_source, __cmd__save_agent_file_source,
    __tauri_command_name_list_agent_file_sources, __tauri_command_name_read_agent_file_source,
    __tauri_command_name_remember_agent_file_source, __tauri_command_name_save_agent_file_source,
    list_agent_file_sources, read_agent_file_source, remember_agent_file_source,
    save_agent_file_source,
};
pub(crate) use agent_file_source::{
    emit_agent_file_save_events, list_agent_file_sources_inner, read_agent_file_source_inner,
    remember_agent_file_source_inner, save_agent_file_source_inner,
};
#[cfg(test)]
use coding_agent_catalog::{
    coding_agent_welcome_status_inner_with, install_coding_agent_inner_with, install_cwd_for,
};
#[cfg(test)]
use coding_agent_profile::{
    persist_coding_agent_env_settings_update, persist_coding_agent_env_settings_update_with_saver,
    persist_coding_agent_profiles_update,
};
#[cfg(test)]
pub(crate) use diagnostics::{append_rail_diagnostic_to, rail_diagnostic_line};
#[cfg(test)]
use diagnostics::{RailDiagnosticRecord, RAIL_DIAGNOSTIC_ERROR, RAIL_SAFE_INTEGER};
#[cfg(test)]
pub(crate) use preferences::{
    move_coding_agent_command_with_saver, reorder_coding_agent_command_with_saver,
    AGENT_ORDER_OVERLAY_PINNED,
};
#[cfg(test)]
use preferences::{persist_narrow_settings_update_with_saver, set_rail_collapse_inner_with_saver};
#[cfg(all(test, windows))]
use profile_selection::build_profile_assignment_target;
#[cfg(all(test, windows))]
pub(crate) use profile_selection::canonical_compare_key;
#[cfg(test)]
use profile_selection::{
    assignment_fingerprint, canonical_real_dir, classify_restart_failure,
    collect_kind_replica_dirs, collect_replica_dirs_in_workgroup,
    enumerate_profile_assignment_targets, profile_assignment_requires_explicit_confirmation,
    selection_default_fingerprint, selection_error_code, source_selection_fingerprint, target_pair,
    validate_profile_assignment_confirmation, CandidateDirs, ProfileCandidate,
    ProfileTargetEnumeration,
};
#[cfg(test)]
pub(crate) use settings_snapshot::settings_snapshot_from;
#[cfg(test)]
pub(crate) use settings_update::{
    persist_protected_settings_update, persist_settings_draft_update,
};
#[cfg(test)]
use settings_update::{
    persist_protected_settings_update_with_saver,
    persist_protected_settings_update_with_saver_and_publication,
    persist_settings_draft_update_with_saver,
    persist_settings_draft_update_with_saver_and_publication,
    purge_sessions_after_settings_update_in_dir, settings_draft_update_events,
};
#[cfg(all(test, windows))]
use settings_update::{
    save_settings_draft_with_hotkey_registration, update_settings_with_hotkey_registration,
};
#[cfg(test)]
use sha2::{Digest, Sha256};

const HOME_MARKDOWN_URL: &str =
    "https://raw.githubusercontent.com/mblua/AgentsCommander/main/docs/home-en.md";

const HOME_MARKDOWN_MAX_BYTES: usize = 256 * 1024; // 256 KB
const HOME_MARKDOWN_TIMEOUT_SECS: u64 = 5;
/// #2133 - the agent-help overlays the frontend layers over its embedded copy. The embedded
/// layer never travels over IPC (epic D6). `remote` stays `None` until P6 fills it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHelpOverlayPayload {
    pub local: Option<crate::config::settings::AgentHelpFile>,
    pub remote: Option<crate::config::settings::AgentHelpFile>,
    pub local_error: Option<String>,
}

/// #2133 - the path-injectable overlay reader. Tests inject `settings_path`;
/// production resolves it. Mirrors `settings_snapshot_helper`'s seam.
pub(crate) fn agent_help_overlay_payload(
    settings_path: Option<PathBuf>,
) -> AgentHelpOverlayPayload {
    let Some(path) = settings_path.or_else(crate::config::settings::settings_path) else {
        return AgentHelpOverlayPayload {
            local: None,
            remote: None,
            local_error: None,
        };
    };
    let (file, local_error) = crate::config::settings::load_local_agent_help_file(&path);
    let has_content =
        file.general.is_some() || !file.by_command.is_empty() || !file.by_agent.is_empty();
    // #2133 (P6) - the downloaded layer, served under the same non-empty rule as `local`.
    let remote = crate::config::settings::load_remote_agent_help_file(&path);
    let remote_has_content =
        remote.general.is_some() || !remote.by_command.is_empty() || !remote.by_agent.is_empty();
    AgentHelpOverlayPayload {
        local: has_content.then_some(file),
        remote: remote_has_content.then_some(remote),
        local_error,
    }
}

#[tauri::command]
pub fn get_agent_help() -> AgentHelpOverlayPayload {
    agent_help_overlay_payload(None)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentProfileResolutionResult {
    pub requested_profile: String,
    pub effective_profile: String,
    pub fallback_chain: Vec<String>,
    pub fallback_applied: bool,
    pub requested_profile_input: Option<String>,
    pub instance_profile_override: Option<String>,
    pub origin_default_profile: Option<String>,
    pub agent_default_profile: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsDraftUpdateEvents {
    pub room_idle_confirmation_changed: bool,
    pub profiles_changed: bool,
    pub env_agent_ids: Vec<String>,
}

/// Fetch the Home screen Markdown source from the public docs URL.
/// Returns the raw Markdown body as a String.
/// Errors are returned as user-facing strings; the frontend renders them in
/// the Home view's error state.
#[tauri::command]
pub async fn fetch_home_markdown(network: State<'_, OutboundNetwork>) -> Result<String, String> {
    let _permit = network.acquire("docs.fetch_home_markdown").await?;
    let resp = tokio::time::timeout(
        Duration::from_secs(HOME_MARKDOWN_TIMEOUT_SECS),
        network
            .general()
            .get(HOME_MARKDOWN_URL)
            .header(
                reqwest::header::USER_AGENT,
                concat!("agentscommander/", env!("CARGO_PKG_VERSION")),
            )
            .send(),
    )
    .await
    .map_err(|_| "Network error: request timed out".to_string())?
    .map_err(|e| format!("Network error: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Server returned status {}", resp.status().as_u16()));
    }

    // Use bytes() so we can length-check before allocating a String.
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    if bytes.is_empty() {
        return Err("Server returned empty response".to_string());
    }
    if bytes.len() > HOME_MARKDOWN_MAX_BYTES {
        return Err("Response too large".to_string());
    }

    // Strip a leading UTF-8 BOM if present so it does not render as an
    // invisible character at the top of the document (grinch §M, optional).
    let trimmed: &[u8] = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);

    String::from_utf8(trimmed.to_vec()).map_err(|_| "Response is not valid UTF-8".to_string())
}

#[cfg(test)]
mod tests;
