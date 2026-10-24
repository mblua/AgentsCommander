mod agent;
mod app_defaults;
mod field;
mod load;
mod owner;
pub(crate) mod project_path;
mod save;
mod source;

#[cfg(test)]
use super::projects;
#[cfg(test)]
use crate::config::instance_artifacts::{
    AGENTS_INSTANCE_FILE_NAME, BLOCKING_MENUS_REMOTE_FILE_NAME, SETTINGS_FILE_NAME,
    SETTINGS_LOCAL_OVERRIDE_FILE_NAME, SETTINGS_LOCK_FILE_NAME, SETTINGS_OWNER_LOCK_FILE_NAME,
};
use crate::config::local_overlay::LocalSettingsOverlay;
#[cfg(test)]
use crate::pty::backend::SessionBackendKind;
use crate::telegram::types::TelegramBotConfig;
#[cfg(test)]
use agent::{
    absent_main_overlay_comparison, agents_unreadable_dirs, classify_registered_profile_inputs,
    codex_hooks_review_menu, comparable_agent, is_profile_gap, known_agent_field,
    normalized_profile_cells, refresh_shipped_agent_help_file_with_publish,
    try_with_agents_layer_publication_state_in, validate_profile_orphans, AGENT_FILE_CHECKPOINT,
    EMBEDDED_AGENT_HELP_JSON, EMBEDDED_BLOCKING_MENUS_JSON,
};
pub(crate) use agent::{
    accept_remote_agent_help_response, accept_remote_blocking_menus_response,
    adopt_orphaned_profiles, agent_file_projection, agents_layer_unreadable_for_dir,
    agents_unreadable_cli_error, agents_unreadable_session_error, bootstrap_binding_revision,
    canonical_profile_source, command_token_basename, deserialize_present_command,
    export_agents_to_instance_file, load_local_agent_help_file, load_remote_agent_help_file,
    persist_agent_file_edits, physical_profile_sets, project_physical_profiles,
    read_remote_agent_help_check_stamp, read_remote_blocking_menus_check_stamp,
    remember_agent_file_locked, retain_configuration_commands, select_configuration_command,
    try_with_agents_layer_publication_state, validate_agent_command_text, validate_env_rows,
    write_remote_agent_help_cache, write_remote_agent_help_check_stamp,
    write_remote_blocking_menus_cache, write_remote_blocking_menus_check_stamp, AgentFileReadReply,
    AgentFileReadRequest, AgentsLayerState, CatalogSourceOperationGuard,
    REMOTE_AGENT_HELP_MAX_BYTES, REMOTE_BLOCKING_MENUS_MAX_BYTES,
};
#[cfg(test)]
pub(crate) use agent::{
    agent_file_read, agent_help_local_path, agent_help_remote_check_path, agent_help_remote_path,
    agent_help_shipped_path, agents_unreadable_startup_notice, blocking_menus_local_path,
    blocking_menus_remote_check_path, blocking_menus_remote_path, blocking_menus_shipped_path,
    bootstrap_identity_revision, export_blocking_menus_to_local_file, list_agent_files,
    load_local_blocking_menus_file, load_remote_blocking_menus_file, parse_agent_help_file,
    parse_blocking_menus_file, prepare_agent_file_edits, pretty_json_bytes,
    read_agent_file_snapshot, refresh_shipped_agent_help_file, refresh_shipped_blocking_menus_file,
    save_agent_file_locked, validate_remote_agent_help_file, validate_remote_blocking_menus_file,
    AgentsPublicationLockError, CODEX_HOOKS_REVIEW_PATTERN, REMOTE_AGENT_HELP_SOURCE_REF,
    REMOTE_BLOCKING_MENUS_SOURCE_REF,
};
pub use agent::{
    agent_profile_identity, apply_issue_1757_migration, default_blocking_menus_for_command,
    empty_profile_cell, finalize_agent_order, is_codex_home_key, is_opencode_config_dir_key,
    is_reserved_env_key, is_valid_profile_letter, normalize_agent_order,
    normalize_container_image_input, normalize_env_key_for_platform, normalize_profile_letter,
    refresh_shipped_agent_help_from_config_dir, repair_coding_agent_profiles_config,
    shipped_agent_help, shipped_blocking_menus, validate_agent_commands,
    validate_codex_home_template_value, validate_expanded_codex_home_value,
    validate_preflight_timeout_seconds, validate_preflight_timeouts, validate_user_env_key,
    AgentBackendConfig, AgentConfig, AgentFileEdits, AgentFileProfileEdit, AgentFileRow,
    AgentFileRowAction, AgentFileRowEdit, AgentFileRowKind, AgentFileSaveOutcome,
    AgentFileSaveReply, AgentFileSourceFormat, AgentFileSourceMenu, AgentFileSourceOption,
    AgentFileSourcePreferences, AgentFileSources, AgentHelpEntry, AgentHelpFile, AgentHelpLink,
    AgentHelpTip, BlockingMenuConfig, BlockingMenuEntry, BlockingMenusFile, BlockingMenusStore,
    CodingAgentEnv, CodingAgentEnvSource, ProfileCellConfig, ProfileSlotConfig,
    SelectedAgentFileSnapshot, AGENT_HELP_SCHEMA_VERSION, BLOCKING_MENUS_SCHEMA_VERSION,
};
use agent::{
    agents_instance_groups, agents_instance_path, agents_layer_state_at,
    agents_layer_state_at_labeled, capture_registered_profile_gate,
    captured_registered_profile_object, coding_agent_identity_physical_from_raw,
    coding_agent_identity_with_physical_overlay, collect_registered_profile_gate,
    decode_physical_catalog, deserialize_agent_file_source_preferences, duplicate_profile_document,
    export_blocking_menus_with_startup, merge_agents_instance_file, merge_agents_instance_groups,
    normalize_agent_backend_configs, parse_agents_instance_contents, physical_profile_rows,
    prepare_registered_profile_owner, raw_configuration_command, read_agents_instance_file,
    read_agents_instance_file_labeled, recheck_registered_profile_gate,
    refresh_coding_agent_identity_at_locked_path, refresh_physical_source_rows,
    registered_profile_raw_gate, registered_profiles_for_read, restore_agents_groups_for_redecode,
    restore_codex_home_carrier, restrict_split_profile_mutation_rows,
    retain_configuration_commands_with_startup, retain_selected_legacy_profile_aliases,
    retain_unknown_registered_agent_fields, save_agents_instance_file,
    selected_registered_profile_owner, set_agents_layer_state,
    validate_registered_overlay_profile_edits, validate_split_canonical_profile_edits,
    write_identity_rows, RegisteredProfileDocument, RegisteredProfileProblemKind, StartupAgentRows,
    AGENTS_INSTANCE_KEYS, CONFIGURATION_COMMAND_PAIRS,
};
pub(crate) use field::default_room_number_mask;
use field::{
    default_agent_group_kill_private_bytes, default_agent_group_warn_private_bytes,
    default_agent_process_kill_private_bytes, default_api_bind, default_api_port,
    default_branch_staleness_enabled, default_branch_staleness_interval_secs,
    default_branch_staleness_notify_orchestrator, default_ci_activity_enabled,
    default_ci_activity_notify_orchestrator, default_ci_sweep_min_interval_secs,
    default_coord_auto_close_minutes, default_coord_badge_red_minutes,
    default_coord_badge_yellow_minutes, default_gemini_model, default_git_sweep_concurrency,
    default_git_sweep_min_interval_secs, default_main_sidebar_width,
    default_max_concurrent_agent_processes, default_profile_slots, default_profiles_schema_version,
    default_resource_backoff_polling, default_resource_keep_last_snapshot,
    default_resource_monitor_enabled, default_resource_watchdog_action,
    default_response_close_idle_seconds, default_restart_resume_agent_prompt,
    default_restart_resume_orchestrator_prompt, default_room_idle_confirmation_seconds,
    default_screenshot_capture_hotkey, default_selected_row_rail_width,
    default_sidebar_compact_hotkey, default_sidebar_style, default_terminal_snapshots_enabled,
    default_true, default_typing_hold_seconds, default_voice_delay, default_web_bind,
    default_web_port, default_zoom, repair_loaded_response_close_settings,
    repair_loaded_room_idle_confirmation_settings, RESPONSE_CLOSE_IDLE_SECONDS_MAX,
    RESPONSE_CLOSE_IDLE_SECONDS_MIN, ROOM_IDLE_CONFIRMATION_SECONDS_MAX,
    ROOM_IDLE_CONFIRMATION_SECONDS_MIN,
};
pub use field::{
    resolve_auto_self_clear, ConfigSeedConfig, MainSidebarSide, MainWindowDisplayState,
    QuotaSourceConfig, QuotaSourceEntry, ResourceWatchdogAction, TelegramNetworkPollErrorLogging,
    TelegramPollFailureLogLevel, TelegramPollRecoveryLogLevel, WatcherConfig, WatcherDedupe,
    WatcherEntry, WatcherMode, WindowGeometry, DEFAULT_TYPING_HOLD_SECONDS,
    TYPING_HOLD_SECONDS_MAX, TYPING_HOLD_SECONDS_MIN,
};
#[cfg(test)]
pub(crate) use load::load_settings_from_path;
#[cfg(test)]
use load::{
    actionable_profile_legacy_work, apply_issue_248_migration, capture_project_settings,
    capture_project_settings_with_startup, default_settings_with_overlay,
    default_settings_with_overlay_captured, load_settings_for_cli_from_path,
    load_settings_for_cli_strict_from_path, load_settings_from_path_with_blocking_menus_publish,
    parse_settings_json, parse_settings_json_with_projection_labeled,
    read_activity_log_enabled_from_path, read_log_level_from_path, read_settings_for_startup,
    settings_backup_path, startup_profile_legacy_work, PROJECT_SOURCE_CAPTURE_HOOK,
    SETTINGS_BACKUP_KEEP, SETTINGS_PRESERVE_LOCK_HOOK,
};
use load::{
    compose_settings_parts, legacy_profiles_shape_present, replace_project_projection,
    rotate_settings_backups, rotate_settings_backups_labeled, write_pre_384_v1_backup,
    AgentsInstanceRead,
};
pub use load::{
    config_seed_dest_os_token_stem, load_settings, load_settings_for_cli,
    load_settings_for_cli_strict, merge_protected_coding_agent_settings,
    read_activity_log_enabled_only, read_log_level_only, validate_and_repair_settings,
    validate_api_server_settings, validate_config_seed_dest, validate_resource_settings,
    validate_room_number_mask, validate_screenshot_hotkey, validate_sidebar_compact_hotkey,
    validate_typing_hold_settings, CONFIG_SEED_OS_TOKENS, ROOM_NUMBER_MASK_MAX_WIDTH,
};
pub(crate) use load::{parse_api_server_socket_addr, settings_path};
pub(crate) use owner::CapturedSource;
#[cfg(test)]
use owner::OWNER_CAPTURE_PANIC;
use owner::{capture_source_file, SETTINGS_CAPTURE_LIMIT};
pub use owner::{
    new_settings_state, CapturePolicy, OperationEffect, OperationOutcome, SettingsAvailability,
    SettingsOperationGuard, SettingsOperationReport, SettingsOwner, SettingsOwnerError,
    SettingsOwnerIdentity, SettingsPublisher, SettingsReadGuard, SettingsReadPermit,
    SettingsRetention, SettingsState,
};
#[cfg(all(test, windows))]
use owner::{SettingsOwnerLeaseVerifyFaultGuard, SETTINGS_OWNER_LEASE_VERIFY_FAULT};
#[cfg(test)]
use project_path::{
    absent_side, ensure_project_paths_migrated_locked, ensure_project_paths_migrated_with,
    project_migration_error, raw_project_projection, read_raw_project_source,
    select_project_source_at_path, OVERLAY_PREMIGRATION_UNSAFE_KEYS,
};
pub(crate) use project_path::{
    apply_project_decode_to_value, begin_project_paths_transaction_at_path, decode_project_state,
    repair_fresh_project_paths_at_path, resync_project_state_from_runtime, ProjectPathsRepairStage,
    ProjectPathsWriteTransaction, ProjectWriteMode, OVERLAY_DERIVED_ID_CLOSURES,
    OVERLAY_INELIGIBLE_DISK_KEYS, OVERLAY_INELIGIBLE_LEGACY_KEYS, OVERLAY_KEY_AGENTS,
    OVERLAY_KEY_MAIN_ALWAYS_ON_TOP, OVERLAY_KEY_MAIN_DISPLAY_STATE, OVERLAY_KEY_MAIN_GEOMETRY,
    OVERLAY_KEY_MAIN_ZOOM, OVERLAY_KEY_RESTORE_COORDINATOR_WAKE_STATE,
};
use project_path::{
    apply_selected_project_state, empty_project_transaction,
    ensure_project_paths_migrated_at_labeled_path, ensure_project_paths_migrated_at_locked_path,
    production_instance_base, project_migration_error_labeled, project_paths_path,
    raw_project_source_from_bytes, read_merged_top_level_key,
    read_raw_project_source_labeled_with_limit, report_overlay_diagnostics,
    select_project_source_at_labeled_path_with_limit, select_project_source_at_locked_path,
    select_project_source_at_locked_path_with_limit, select_project_source_from_raw,
    select_project_source_locked, validate_non_project_settings, MainWindowPlacementWriteMode,
    TerminalSnapshotGateWriteMode, FIELD_ARCHIVED, FIELD_ARCHIVED_REL, FIELD_MAIN_DISPLAY_STATE,
    FIELD_MAIN_GEOMETRY, FIELD_PROJECT_PATH, FIELD_PROJECT_PATHS, FIELD_PROJECT_PATHS_REL,
    FIELD_PROJECT_PATH_REL, FIELD_TERMINAL_SNAPSHOTS_ENABLED,
};
#[cfg(test)]
pub(crate) use project_path::{
    reconcile_project_state_to_path, OVERLAY_MIGRATION_DESTINATION_KEYS,
};
use save::replace_settings_file_atomic;
#[cfg(all(test, windows))]
use save::replace_settings_file_atomic_with_retry;
#[cfg(test)]
pub use save::save_settings_with_project_paths;
pub(crate) use save::{
    compare_and_set_terminal_snapshots_enabled, project_state_has_structural,
    read_pty_input_project_paths_strict, read_pty_input_project_paths_strict_offloaded,
    read_terminal_snapshot_project_paths_strict, read_terminal_snapshot_security_settings_strict,
    read_terminal_snapshot_security_settings_strict_offloaded, set_main_window_placement,
};
#[cfg(test)]
use save::{
    compare_and_set_terminal_snapshots_enabled_at_path, preserve_main_window_placement_keys,
    project_atomic_checkpoint, read_project_paths_from_disk,
    read_pty_input_project_paths_strict_from_path,
    read_terminal_snapshot_security_settings_strict_from_path, render_diagnostic_path,
    render_settings_save_diagnostic, save_settings_to_path,
    save_settings_to_path_preserving_project_paths_typed, save_settings_value,
    save_settings_value_locked, settings_lock_debug_lost_count, settings_lock_debug_path,
    settings_lock_debug_sender, settings_lock_debug_set_attempt, settings_lock_debug_transport,
    settings_lock_debug_transport_initialized, SettingsLockDebugAttempt, SettingsLockDebugContext,
    SettingsLockDebugTestHooks, SettingsSaveDiagnosticCapture, PROJECT_MIGRATION_ATOMIC_FAULT,
    SETTINGS_LOCK_DEBUG_CAPACITY, SETTINGS_LOCK_DEBUG_PATH_BYTES, SETTINGS_LOCK_DEBUG_TEST_HOOKS,
    SETTINGS_LOCK_DEBUG_TEST_UNLOCK_ERROR, SETTINGS_LOCK_UNAVAILABLE_CODE,
    SETTINGS_SAVE_DIAGNOSTIC_CAPTURE,
};
use save::{
    io_error_kind_as_str, json_error_category_as_str, next_settings_save_op_id,
    project_lists_from_projection, read_disk_object_for_write_typed, report_settings_save_error,
    save_settings_value_with_startup, serialize_settings_component, write_value_atomic,
    write_value_atomic_labeled, SettingsFileLock, SettingsSaveCause, SettingsSaveError,
    SettingsSaveLegacyOutward, SettingsSaveReason, SettingsSaveReportSurface, SettingsSaveStage,
};
#[cfg(test)]
pub(crate) use save::{
    refresh_and_decode_project_paths_from_path, refresh_project_paths_from_path,
    save_settings_to_path_preserving_project_paths, save_settings_with_project_paths_to_path,
    set_main_window_placement_at_path, TerminalSnapshotSecuritySettings,
};
pub use save::{refresh_project_paths_from_disk, save_settings};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::Map;
use serde_json::Value;
pub use source::{
    configuration_identity, identity_persisted_name, profile_identity, refresh_agent_id_hashes,
    validate_identity_snapshot, ConfigurationRef, IdentitySnapshotEntry, IdentityValidationCode,
    IdentityValidationError, SourceAvailability, SourceCapabilities, SourceDiagnostic,
    SourceFamily, SourceKind, SourceRef, SourceRevision, SourceRole, SourceScope,
    SourceSelectionDefault, SourceSelectionExpectation, SourceSelectionState,
    SourceSelectionTicket, SourceSnapshot,
};
use source::{
    configuration_identity_debug, decode_registered_raw, physical_identity_violations, raw_entries,
    refresh_pair_id_hashes, refresh_physical_pair_id_hashes, source_bytes_revision,
    source_diagnostic, source_general_field_names, validate_physical_identity_delta,
};
#[cfg(test)]
pub(crate) use source::{
    configure_source_factory_guard_cut, prepare_settings_without_source_write,
    prepare_source_candidate, read_own_source_intent, source_factory_guard_calls,
    source_factory_guard_cut_armed, source_transition_effect_counts,
    source_transition_publication_counts, source_transition_test_locator,
    source_transition_test_root, LegacyOriginUnit, OriginPhysicalInput, PreparedSourceCandidate,
    SourceLocalInventory, SourceOriginalProofAggregate,
};
pub(crate) use source::{
    execute_identity_reference_pair_guarded, identity_reference_pair_paths, list_active_sources,
    pair_identity, pair_identity_debug, prepare_source_candidate_with_original_links,
    publish_source_intent_with_validation, read_identity_reference_physical,
    read_own_source_journal, read_source_snapshot, read_source_transition,
    recover_source_target_packet, release_identity_reference_pair_guarded, source_complete_proof,
    source_inventory_target, source_original_mappings, source_protocol_active,
    source_transition_digest, source_transition_digest_bytes,
    update_own_source_journal_with_validation, verify_completed_source_target,
    verify_identity_reference_pair_guarded, IdentityWriteLease, LegacyCompatibilityOrigin,
    OriginInputs, RawSourceSnapshot, SourceCoordinationError, SourceDiscoveredTargetEvidence,
    SourceDiscoveryWitness, SourceIdentityMapping, SourceInventoryCapture, SourceInventoryInputs,
    SourceInventoryProof, SourceInventoryTarget, SourceLocalProgress, SourceOriginalSlot,
    SourcePacketRecord, SourcePreparationError, SourceReadContext, SourceReceipt,
    SourceTransitionPhase, SourceTransitionStatus, SourceWriteContext,
};
#[cfg(test)]
use source::{
    identity_digest, require_source_original_units, source_inventory_digest,
    source_inventory_projection, validate_identity_snapshot_with_digest, SourceMembershipTopology,
    SOURCE_GENERAL_PROTECTED,
};
use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::HashSet;
#[cfg(test)]
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentProfilesConfig {
    #[serde(default = "default_profiles_schema_version")]
    pub schema_version: u32,
    #[serde(default = "default_profile_slots")]
    pub profile_slots: BTreeMap<String, ProfileSlotConfig>,
    #[serde(default)]
    pub default_profile_by_agent: BTreeMap<String, String>,
    #[serde(default)]
    pub profiles_by_agent: BTreeMap<String, BTreeMap<String, ProfileCellConfig>>,
    /// #548: per-(agent, letter) label override. Resolution is own label ->
    /// legacy profile_slots[letter].label -> bare letter; there is no first-agent
    /// inheritance. Always serializes (no skip_serializing_if) like
    /// profiles_by_agent, so the key is always present on disk.
    #[serde(default)]
    pub profile_labels_by_agent: BTreeMap<String, BTreeMap<String, String>>,
    /// #2450: agent id -> `agent_profile_identity` as of the last save. Rows for
    /// ids that are no longer agents are kept: they are the orphans a recreated
    /// agent adopts (`adopt_orphaned_profiles`). Absent in an older file.
    #[serde(default)]
    pub identity_by_agent: BTreeMap<String, String>,
    /// Backend-owned isolated Codex HOME leaves, retained independently of IDs.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub codex_home_key_by_agent: BTreeMap<String, String>,
}

impl Default for CodingAgentProfilesConfig {
    fn default() -> Self {
        Self {
            schema_version: default_profiles_schema_version(),
            profile_slots: default_profile_slots(),
            default_profile_by_agent: BTreeMap::new(),
            profiles_by_agent: BTreeMap::new(),
            profile_labels_by_agent: BTreeMap::new(),
            identity_by_agent: BTreeMap::new(),
            codex_home_key_by_agent: BTreeMap::new(),
        }
    }
}

// Keep lower IO carriers behind the existing settings facade.
#[allow(unused_imports)]
pub(crate) use crate::config::local_config_io::{
    PairCompleteProof as IdentityReferenceCompleteProof,
    PairReservation as IdentityReferencePacket, PairReservationRequest as IdentityReferenceRequest,
    PhysicalState as IdentityReferencePhysicalState, PreparedPairError as IdentityReferenceError,
    ProtectedRevision as IdentityReferenceProtectedRevision,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub default_shell: String,
    pub default_shell_args: Vec<String>,
    /// Available coding agents
    pub agents: Vec<AgentConfig>,
    /// Reusable coding-agent profile letters, defaults, and per-agent cells.
    #[serde(default)]
    pub coding_agent_profiles: CodingAgentProfilesConfig,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_agent_file_source_preferences"
    )]
    pub agent_file_source_by_menu: Option<AgentFileSourcePreferences>,
    /// Configured Telegram bots for bridge
    #[serde(default)]
    pub telegram_bots: Vec<TelegramBotConfig>,
    /// Controls log severity for transient and sustained Telegram getUpdates
    /// network failures. Non-network poll failures still log at ERROR.
    #[serde(default)]
    pub telegram_network_poll_error_logging: TelegramNetworkPollErrorLogging,
    /// When true, on app start, wake coordinators whose PTY was awake at shutdown.
    /// Coordinators that were asleep at shutdown remain asleep. Non-coordinator
    /// team members are never auto-woken on startup (the user must click their
    /// replica in the sidebar to wake them). Issue #248.
    #[serde(default)]
    pub restore_coordinator_wake_state: bool,
    /// Migration carrier: legacy field name from before issue #248. Read on
    /// deserialization, then translated into `restore_coordinator_wake_state` by
    /// `apply_issue_248_migration`. `skip_serializing_if = "Option::is_none"`
    /// elides it on the next save once the migration has run.
    ///
    /// One-shot migration semantics:
    ///   - legacy `startOnlyCoordinators: true`  → `restoreCoordinatorWakeState: true`
    ///   - legacy `startOnlyCoordinators: false` → `restoreCoordinatorWakeState: false`
    ///
    /// In both cases the legacy value is dropped from disk on next save.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "startOnlyCoordinators"
    )]
    pub legacy_start_only_coordinators: Option<bool>,
    /// (#1793) Typed into an ORCHESTRATOR whose persisted row was working when
    /// the app went down. Empty means inject nothing. No effect unless
    /// `restore_coordinator_wake_state` is on: a deferred orchestrator has no PTY.
    #[serde(default = "default_restart_resume_orchestrator_prompt")]
    pub restart_resume_orchestrator_prompt: String,
    /// (#1793) Typed into a NON-COORDINATOR replica whose persisted row was
    /// working. Empty means inject nothing. No effect unless
    /// `restart_resume_wake_working_agents` is on: a deferred replica has no PTY.
    #[serde(default = "default_restart_resume_agent_prompt")]
    pub restart_resume_agent_prompt: String,
    /// (#1793) Wake a non-coordinator replica whose persisted row was working.
    /// Off by default so the #248 policy (non-coordinators are never auto-woken)
    /// is unchanged unless the user opts in. This wakes; the prompt above types.
    #[serde(default)]
    pub restart_resume_wake_working_agents: bool,
    /// Keep sidebar window always on top
    #[serde(default)]
    pub sidebar_always_on_top: bool,
    /// When true, play a short beep when an entire team transitions from
    /// busy→all-idle. The transition is computed in the FE from `waitingForInput`.
    #[serde(default = "default_true")]
    pub team_idle_beep_enabled: bool,
    /// Master switch for all app-emitted sounds. When false, every current and
    /// future app-generated playback path must stay silent. Per-feature toggles
    /// (e.g. `team_idle_beep_enabled`) act as additional gates underneath this one.
    #[serde(default = "default_true")]
    pub sounds_enabled: bool,
    /// Raise terminal window when sidebar is clicked
    #[serde(default = "default_true")]
    pub raise_terminal_on_click: bool,
    /// #714 Native global hotkey for screenshot capture, e.g. "Ctrl+Q".
    #[serde(default = "default_screenshot_capture_hotkey")]
    pub screenshot_capture_hotkey: String,
    /// #2281 Sidebar compact toggle hotkey, e.g. "Ctrl+Shift+E". Stored
    /// byte-identical to what the user typed; an invalid value blocks the save.
    #[serde(default = "default_sidebar_compact_hotkey")]
    pub sidebar_compact_hotkey: String,
    /// #2507 One `#` per zero-padded digit of a NEWLY created room's number. Default
    /// `"#"` reproduces today's names (`room-1-<team>`); `"##"` gives `room-01-<team>`.
    /// A wider number is never truncated (`"##"` + 100 -> `room-100-<team>`). Existing
    /// rooms are never renamed. An invalid value blocks the save; it is not repaired.
    #[serde(default = "default_room_number_mask")]
    pub room_number_mask: String,
    /// Enable voice-to-text microphone button on session items
    #[serde(default)]
    pub voice_to_text_enabled: bool,
    /// Gemini API key for voice transcription
    #[serde(default)]
    pub gemini_api_key: String,
    /// Gemini model for voice transcription
    #[serde(default = "default_gemini_model")]
    pub gemini_model: String,
    /// #2265 Co-managed: Jev (TypeSafe System One) API key. `""` means not configured.
    #[serde(default)]
    pub jev_api_key: String,
    /// #2265 Co-managed: Jev model. Pinned to the version the 0.70/0.15
    /// thresholds were measured on; a newer model is a deliberate, visible
    /// change that invalidates those numbers.
    #[serde(default = "default_jev_model")]
    pub jev_model: String,
    /// #2265 Co-managed: Jev endpoint.
    #[serde(default = "default_jev_endpoint")]
    pub jev_endpoint: String,
    /// #2265 Co-managed: Jev request timeout, in seconds.
    #[serde(default = "default_jev_timeout_secs")]
    pub jev_timeout_secs: u64,
    /// #2265 Co-managed: judgment threshold.
    #[serde(default = "default_jev_threshold")]
    pub jev_threshold: f32,
    /// #2265 Co-managed: minimum margin between the top two judgments.
    #[serde(default = "default_jev_margin")]
    pub jev_margin: f32,
    /// Global Co-managed switch. **`false` by default: the feature is in
    /// development.** While `false` the feature is off app-wide: it masks every
    /// room `config.json` and hides the menu item. Only turned on by editing
    /// `settings.json` by hand; there is no UI control. Takes effect on restart.
    #[serde(default)]
    pub co_managed_enabled: bool,
    /// Show coding-agent installation actions only after a manual opt-in.
    /// Fresh settings and older files without this key keep the actions hidden.
    #[serde(default)]
    pub coding_agent_install_enabled: bool,
    /// Show container UI options only after a manual settings.json opt-in.
    /// Takes effect after reloading settings or restarting the app.
    #[serde(default)]
    pub container_options_enabled: bool,
    /// Auto-execute (send Enter) after voice transcription
    #[serde(default = "default_true")]
    pub voice_auto_execute: bool,
    /// Delay in seconds before auto-executing after transcription
    #[serde(default = "default_voice_delay")]
    pub voice_auto_execute_delay: u32,
    /// Zoom level for the sidebar window (1.0 = 100%). DEPRECATED in 0.8.0 — see `main_zoom`.
    /// Retained for one version for downgrade safety; seeded into `main_zoom` on first load.
    #[serde(default = "default_zoom")]
    pub sidebar_zoom: f64,
    /// Zoom level for the terminal window (1.0 = 100%). Still used by detached windows in 0.8.0.
    #[serde(default = "default_zoom")]
    pub terminal_zoom: f64,
    /// Zoom level for the unified main window (1.0 = 100%). Introduced in 0.8.0.
    #[serde(default = "default_zoom")]
    pub main_zoom: f64,
    /// Legacy: zoom level for the removed guide window. Kept for backwards-compat reads.
    #[serde(default = "default_zoom")]
    pub guide_zoom: f64,
    /// Legacy: zoom level for the removed dark factory window. Kept for backwards-compat reads.
    #[serde(default = "default_zoom")]
    pub darkfactory_zoom: f64,
    /// DEPRECATED in 0.8.0 — previously held the sidebar window geometry under the
    /// two-window model. `skip_serializing_if` drops it on next save. See `main_geometry`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_geometry: Option<WindowGeometry>,
    /// DEPRECATED in 0.8.0 — previously held the terminal window geometry. Seeded into
    /// `main_geometry` by the first-boot migration. `skip_serializing_if` drops it on next save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_geometry: Option<WindowGeometry>,
    /// Saved geometry for the unified main window. Introduced in 0.8.0.
    #[serde(default)]
    pub main_geometry: Option<WindowGeometry>,
    /// #2348 - saved display state for the unified main window. Defaulted to
    /// `Normal` so a pre-#2348 file loads unchanged. Whole-settings writers
    /// preserve a present on-disk value; only the narrow placement command
    /// (`commands::window::set_main_window_placement`) may change it.
    #[serde(default)]
    pub main_window_display_state: MainWindowDisplayState,
    /// Width of the sidebar pane inside the main window, in logical pixels.
    /// Clamped to [200, 600] at drag time and on load.
    #[serde(default = "default_main_sidebar_width")]
    pub main_sidebar_width: f64,
    /// Side of the unified main window where the sidebar is placed.
    #[serde(default)]
    pub main_sidebar_side: MainSidebarSide,
    /// Keep the unified main window always on top.
    #[serde(default)]
    pub main_always_on_top: bool,
    /// #587 - whether the Resource Monitor occupies the main central pane (vs the
    /// terminal). Restored on startup; default false (terminal).
    #[serde(default)]
    pub main_resource_monitor_attached: bool,
    /// Enable the embedded web server for remote browser access
    #[serde(default)]
    pub web_server_enabled: bool,
    /// Port for the web server
    #[serde(default = "default_web_port")]
    pub web_server_port: u16,
    /// Bind address: "127.0.0.1" (local only) or "0.0.0.0" (all interfaces)
    #[serde(default = "default_web_bind")]
    pub web_server_bind: String,
    /// #791 - enable the in-daemon control-plane API server (Docker/distributed
    /// agents). Default false: no new listening socket unless the operator opts in.
    #[serde(default)]
    pub api_server_enabled: bool,
    /// #791 - control-plane API port. Profile-aware default so dev/prod builds
    /// do not collide on one host.
    #[serde(default = "default_api_port")]
    pub api_server_port: u16,
    /// #791 - control-plane API bind address. Default "127.0.0.1" (safe, loopback).
    /// Reaching a Linux container requires a deliberate operator widening; any
    /// non-loopback bind logs a loud startup warning.
    #[serde(default = "default_api_bind")]
    pub api_server_bind: String,
    /// #1173 - disclosure gate for authorized backend terminal snapshots.
    /// Whole-settings writers preserve an explicit on-disk value; a legacy
    /// settings object whose key is absent is materialized as true (the
    /// default-on phase deliberately reversed the absent-key rule), and a
    /// malformed value fails closed. Only the dedicated compare-and-set
    /// command may change an explicit value.
    #[serde(default = "default_terminal_snapshots_enabled")]
    pub terminal_snapshots_enabled: bool,
    /// Currently loaded project path (legacy single-project, kept for backward compat)
    #[serde(default)]
    pub project_path: Option<String>,
    /// Currently loaded project paths (multi-project support)
    #[serde(default)]
    pub project_paths: Vec<String>,
    /// #881 - projects hidden from the sidebar but still registered. Disk-authoritative
    /// under the same Design S rules as `project_paths`: the default `save_settings`
    /// preserves the on-disk copy and only dedicated list commands mutate it.
    #[serde(default)]
    pub archived_project_paths: Vec<String>,
    /// #1077 - hidden persistence state for the portable dual project paths. Holds
    /// the resolved raw/selected pairs, their instance-relative companions,
    /// outcomes, and reconcile-eligibility bits so the codec can rebuild the six
    /// disk fields without re-reading disk. Never serialized directly (the codec
    /// owns the six on-disk fields); behind an `Arc` so the ubiquitous
    /// `AppSettings::clone()` stays cheap and copy-on-write mutation is explicit.
    #[serde(skip, default)]
    pub(crate) project_path_state: Arc<project_path::ProjectPathPersistenceState>,
    /// An unavailable source must never become an empty retention list.
    #[serde(skip, default)]
    pub(crate) project_paths_persistence_error: Option<String>,
    /// #1737 - the `settings.local.json` overlay in force for this load, and the base
    /// values it displaces. Never serialized; behind an `Arc` so `AppSettings::clone()`
    /// stays cheap. Like `project_path_state`, a value that arrives from the renderer
    /// or the WebSocket transport carries `Arc::default()` here, so every whole-object
    /// writer must restore it from live memory first (plan D14).
    #[serde(skip, default)]
    pub(crate) local_overlay_state: Arc<LocalSettingsOverlay>,
    /// #2716 (B3, option C) - whether `agents.30.instance.no-git.json` could be read.
    /// Never serialized, for the same D14 reason as `local_overlay_state`: a client
    /// draft can never carry or forge it. Set only through `set_agents_layer_state`.
    #[serde(skip, default)]
    pub(crate) agents_layer: AgentsLayerState,
    /// Backend physical identity inputs captured before typed repair/defaulting.
    #[serde(skip, default)]
    pub(crate) coding_agent_identity_physical: Option<Arc<Value>>,
    /// Sidebar visual style: "noir-minimal", "card-sections", "command-center", "deep-space",
    /// "arctic-ops", "obsidian-mesh", "neon-circuit"
    #[serde(default = "default_sidebar_style")]
    pub sidebar_style: String,
    /// #1796 - selected-row rail width. A whole number from 1 to 14, optionally
    /// suffixed `px`: `9px` and `12` are both accepted; `0`, `15`, `2em` and
    /// `40px` are not. The bound is `RAIL_WIDTH_MAX` in
    /// `src/sidebar/selected-row-rail.ts`. Rust does not validate this field. That
    /// one predicate runs at startup and on every keystroke in the settings modal,
    /// which is the only place it shows a warning; an unusable value is never
    /// published to the DOM and the stylesheet falls back to its `:root` default.
    #[serde(default = "default_selected_row_rail_width")]
    pub selected_row_rail_width: String,
    /// #1796 - selected-row rail colour, `#rrggbb` exactly; the 3- and 8-digit hex
    /// forms are not accepted. Same soft-warning contract as
    /// `selected_row_rail_width`.
    #[serde(default = "default_selected_row_rail_color")]
    pub selected_row_rail_color: String,
    /// Root token that bypasses all routing checks in the send command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_token: Option<String>,
    /// Whether the user has dismissed the first-run onboarding wizard
    #[serde(default)]
    pub onboarding_dismissed: bool,
    /// When true, sort the Coordinator Quick-Access list by most-recent-activity descending.
    /// Activity = busy→idle transition (IdleDetector emits session_idle).
    /// Per-session timestamps live in the frontend store and are NOT persisted.
    #[serde(default)]
    pub coord_sort_by_activity: bool,
    #[serde(default = "default_true")]
    pub always_show_selected_workgroup: bool,
    /// Optional logger filter expression. Applied at startup if `RUST_LOG` is unset.
    /// Uses standard `env_logger` filter syntax
    /// (e.g. `info,agentscommander_lib::config::teams=trace`).
    /// Phase 1 of #93 — settings-level control with `RUST_LOG` env override (backwards-compat).
    /// Phase 2 (UI dropdown) and Phase 3 (live reload) are deferred per the issue.
    #[serde(default)]
    pub log_level: Option<String>,
    #[serde(default)]
    pub activity_log_enabled: bool,
    /// When true, on Coordinator session spawn AC injects a prompt asking the
    /// agent to add a YAML frontmatter `title:` line to its workgroup
    /// `TASK.md` (only if the brief is non-empty and has no `title:` yet).
    /// See plan `_plans/107-auto-brief-title.md`.
    #[serde(default = "default_true")]
    pub auto_generate_task_title: bool,
    /// Optional override for the local agent-templates directory used by the
    /// New Agent role-template picker (#271). Empty/missing ⇒ default
    /// `<config_dir>/agent-templates/`. Relative ⇒ resolved against
    /// `<config_dir>/`. Absolute ⇒ used as-is.
    #[serde(default)]
    pub agent_templates_path: Option<String>,
    /// When true, the app uses the light theme; when false, the dark theme.
    /// Defaults to `false` so fresh and missing values use dark mode.
    #[serde(default)]
    pub theme_light: bool,
    /// #965 - rail project sections the user has explicitly collapsed by clicking
    /// their header. This is the RAIL's own state; it is deliberately NOT the
    /// ProjectPanel's collapse (which stays session-only and auto-focus driven).
    /// Entries are FRONTEND-NORMALIZED project paths (lowercase, forward slashes,
    /// no trailing slash: `normalizeProjectPathForCompare` in
    /// `src/sidebar/stores/project-refresh.ts`).
    /// WRITTEN ONLY BY `set_rail_collapse`. A whole-object settings payload from the
    /// GUI/CLI/API carries no authority for this field: both whole-object writers
    /// restore it from live memory (see `build_protected_settings_candidate`).
    #[serde(default)]
    pub rail_collapsed_projects: Vec<String>,
    /// #965 - collapsed state of the rail's cross-project `Favorites` section.
    /// Belongs to no project, which is why collapse cannot live in project-settings.json.
    /// Same protection as `rail_collapsed_projects`.
    #[serde(default)]
    pub rail_favorites_collapsed: bool,
    /// When true, show the Spec Board toolbar button. Defaults off because the
    /// board is an opt-in feature enabled manually from settings.json.
    #[serde(default)]
    pub spec_board_enabled: bool,
    /// #1298 - how many repositories the global git sweeper detects at once.
    /// Default 1, i.e. strictly sequential, which is the property the sweeper
    /// exists for. Read once per round and clamped to 1..=4 at the read site, which
    /// keeps the app from rewriting the user's hand-edited settings.json; changing
    /// it needs a RESTART, since the sweeper reads the in-memory settings state and
    /// nothing reloads that from disk. Raising it to 2 halves worst-case
    /// head-of-line blocking (see INV-1) at the cost of reintroducing bounded
    /// concurrency. No UI: same manual-opt-in shape as `spec_board_enabled`;
    /// documented in docs/reference/settings.md.
    #[serde(default = "default_git_sweep_concurrency")]
    pub git_sweep_concurrency: u8,
    /// #1298 - lower bound, in seconds, on one sweeper round. GUARD, not an
    /// optimization: with an empty work list a round takes ~0ms and an unfloored
    /// loop spins a core. Clamped to 1..=3600 at the read site; 0 is rejected by the
    /// clamp for that reason. The effective period is max(this, round duration), so
    /// on a large workgroup set the round duration dominates and this never fires.
    #[serde(default = "default_git_sweep_min_interval_secs")]
    pub git_sweep_min_interval_secs: u64,
    /// #2064 - whether the remote activity sweeper asks GitHub whether CI is
    /// running on each room repo's exact HEAD. Default on. Read at startup, to
    /// decide whether the sweeper thread exists at all, and once per round
    /// otherwise. Changing it needs a RESTART, because nothing reloads the
    /// in-memory settings state from disk. No UI: hand-edited settings.json.
    #[serde(default = "default_ci_activity_enabled")]
    pub ci_activity_enabled: bool,
    /// #2064 - whether a CI state transition may inject a notice into the room's
    /// orchestrator (Phase B). Subordinate to `ci_activity_enabled`: colour
    /// without interruption must be expressible, because the chip is passive and
    /// an injected notice consumes an agent's context mid-task. Read once per
    /// round; changing it needs a RESTART. No UI.
    #[serde(default = "default_ci_activity_notify_orchestrator")]
    pub ci_activity_notify_orchestrator: bool,
    /// #2064 - whether the sweeper asks whether the repository's default branch
    /// contains commits this checkout does not. A SIBLING of
    /// `ci_activity_enabled`, not a child: two questions, two cadences, two
    /// failure modes, and a user who wants one without the other. Read once per
    /// round; changing it needs a RESTART. No UI.
    #[serde(default = "default_branch_staleness_enabled")]
    pub branch_staleness_enabled: bool,
    /// #2064 - whether a `BranchStale` transition may inject a notice into the
    /// room's orchestrator (Phase B). Subordinate to
    /// `branch_staleness_enabled`, for the same context-budget reason as
    /// `ci_activity_notify_orchestrator`. Read once per round; changing it needs
    /// a RESTART. No UI.
    #[serde(default = "default_branch_staleness_notify_orchestrator")]
    pub branch_staleness_notify_orchestrator: bool,
    /// #2064 - seconds between CI questions for a key that is not currently
    /// running. Read once per round and clamped to 10..=3600 at the read site,
    /// which keeps the app from rewriting the user's hand-edited settings.json;
    /// changing it needs a RESTART. Keys whose last confirmed state is `Running`
    /// use the fixed 10s cadence instead: `Finished` is the transition that
    /// actually unblocks an agent. No UI.
    #[serde(default = "default_ci_sweep_min_interval_secs")]
    pub ci_sweep_min_interval_secs: u64,
    /// #2064 - seconds between staleness questions. Read once per round and
    /// clamped to 10..=3600 at the read site; changing it needs a RESTART. No UI.
    #[serde(default = "default_branch_staleness_interval_secs")]
    pub branch_staleness_interval_secs: u64,
    #[serde(default = "default_resource_monitor_enabled")]
    pub resource_monitor_enabled: bool,
    #[serde(default = "default_max_concurrent_agent_processes")]
    pub max_concurrent_agent_processes: u32,
    #[serde(default = "default_resource_watchdog_action")]
    pub resource_watchdog_action: ResourceWatchdogAction,
    #[serde(default = "default_agent_group_warn_private_bytes")]
    pub agent_group_warn_private_bytes: u64,
    #[serde(default = "default_agent_group_kill_private_bytes")]
    pub agent_group_kill_private_bytes: u64,
    #[serde(default = "default_agent_process_kill_private_bytes")]
    pub agent_process_kill_private_bytes: u64,
    #[serde(default = "default_resource_keep_last_snapshot")]
    pub resource_keep_last_snapshot: bool,
    #[serde(default = "default_resource_backoff_polling")]
    pub resource_backoff_polling: bool,
    /// #552 badge color thresholds (minutes). green < yellow <= value < red <= value.
    #[serde(default = "default_coord_badge_yellow_minutes")]
    pub coordinator_idle_badge_yellow_minutes: u32,
    #[serde(default = "default_coord_badge_red_minutes")]
    pub coordinator_idle_badge_red_minutes: u32,
    /// #552 auto-close lifecycle clock.
    #[serde(default = "default_true")]
    pub coordinator_auto_close_enabled: bool,
    #[serde(default = "default_coord_auto_close_minutes")]
    pub coordinator_auto_close_minutes: u32,
    /// #817 When true, auto-close skips sessions with a persisted Telegram bot
    /// assignment. Default false preserves legacy auto-close behavior.
    #[serde(default)]
    pub coordinator_auto_close_skip_telegram_assigned: bool,
    /// #588 When true, manually closing a coordinator also closes its team
    /// agents (cascade). When false, only the coordinator closes. Default true.
    #[serde(default = "default_true")]
    pub coordinator_cascade_close_enabled: bool,
    /// #609 When true, check npm on startup (<=1x/24h) and notify in-app when
    /// a newer published version is available. Default true.
    #[serde(default = "default_true")]
    pub npm_update_notifications_enabled: bool,
    /// #1925 When true, download the remote blocking-menu patterns on startup
    /// (<=1x/24h); a downloaded file applies at the next start. Default true.
    #[serde(default = "default_true")]
    pub remote_blocking_menus_enabled: bool,
    /// #2133 When true, download the remote per-agent help on startup
    /// (<=1x/24h); a downloaded file applies at the next start. Default true.
    #[serde(default = "default_true")]
    pub remote_agent_help_enabled: bool,
    /// #640 Global master for auto self-handoff-and-clear. Absolute kill switch:
    /// false => off for every agent. When true, the class-aware default applies
    /// (ON for coordinator/Root, OFF for specialists), subject to per-agent
    /// overrides in `auto_self_clear_by_agent`.
    #[serde(default = "default_true")]
    pub auto_self_clear_enabled: bool,
    /// #640 Per-agent override of the class default, keyed by agent name (same
    /// key as `coding_agent_profiles.default_profile_by_agent`). Applies only
    /// while the global master is on; absent = use the class default.
    #[serde(default)]
    pub auto_self_clear_by_agent: std::collections::BTreeMap<String, bool>,
    /// #1327 - per-command auto-update policy for the startup update run, keyed by
    /// the catalog COMMAND string (not the agent id: several profiles can share one
    /// command; the software is the update unit). Absent = never asked (the startup
    /// dialog asks once, default No); present = last answer (true = run this
    /// command's updateCommands at startup, false = never ask again). Replaces the
    /// two inert #1318 agent-id-keyed maps. Read and
    /// written by the #1327 startup update flow only.
    #[serde(default)]
    pub agent_auto_update_by_command: std::collections::BTreeMap<String, bool>,
    /// #930 - when true (default), container coding-agent sessions copy the host
    /// user's credential file for that agent into the replica config dir at spawn
    /// and delete it on teardown. When false, the user supplies credentials
    /// themselves (e.g. a CLAUDE_CODE_OAUTH_TOKEN env row).
    #[serde(default = "default_true")]
    pub container_credentials_from_host: bool,
    /// #1171 - root-level watcher patterns, keyed by watcher id.
    ///
    /// Root-level and not a field on `AgentConfig`, so the 20-plus struct-construction sites
    /// that already had to write `context_regex: None` are untouched, AND so a pattern can
    /// apply to every agent - which is exactly what the per-agent shape cannot express. Same
    /// shape as `auto_self_clear_by_agent` (`:530-531`). `BTreeMap` for a stable on-disk
    /// order and clean diffs, and because the 8-watcher budget resolves in key order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub watchers: BTreeMap<String, WatcherEntry>,
    /// #1171 - geometry of the watcher activity window.
    ///
    /// `skip_serializing_if` and deliberately NOT a copy of `main_geometry` (`:367-369`),
    /// which lacks it: without the skip, `"watchersGeometry": null` would appear in every
    /// user's file on the next save, so configuring nothing would still leave a trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watchers_geometry: Option<WindowGeometry>,
    /// #2482 - where each agent's weekly (7-day) quota reading comes from, keyed by
    /// agent id. Root-level, not a field on `AgentConfig`, for the reason `watchers`
    /// is: the 34 literal `AgentConfig` construction sites stay untouched. Absent or
    /// empty = off for every agent: no reading, no compile, no event.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quota_sources: BTreeMap<String, QuotaSourceEntry>,
    /// #1646 / #1647 - master kill switch for proactive detection of terminal blocking menus.
    #[serde(default = "default_true")]
    pub menu_guard_enabled: bool,
    /// #2336 - seconds the automatic typing hold stays active after the last
    /// qualifying keystroke in a session. Default 30; accepted range 1..=3600,
    /// rejected outside it by `validate_and_repair_settings` so a Settings update
    /// cannot persist a value that would make the hold nonsensical. Read live at
    /// each hold evaluation, so an edit takes effect without a restart.
    #[serde(default = "default_typing_hold_seconds")]
    pub typing_hold_seconds: u32,
    /// User preference; runtime readiness is controlled separately.
    #[serde(default = "default_true")]
    pub response_close_enabled: bool,
    /// Continuous safe idle required after an eligible response, in seconds.
    #[serde(default = "default_response_close_idle_seconds")]
    pub response_close_idle_seconds: u32,
    /// Continuous room inactivity required before confirmation, in seconds.
    #[serde(default = "default_room_idle_confirmation_seconds")]
    pub room_idle_confirmation_seconds: u32,
}

fn default_jev_model() -> String {
    "jev-1.13.0".to_string()
}

fn default_jev_endpoint() -> String {
    "https://api.typesafe.ai/v1/systemone".to_string()
}

fn default_jev_timeout_secs() -> u64 {
    20
}

fn default_jev_threshold() -> f32 {
    0.70
}

fn default_jev_margin() -> f32 {
    0.15
}

fn default_selected_row_rail_color() -> String {
    "#FFFFFF".to_string()
}

/// #1925 (D2) - the published file: one pinned value built with concat! of literals,
/// source ref `main` and a
/// `v1` segment that must equal `BLOCKING_MENUS_SCHEMA_VERSION`; a test pins both.
pub(crate) const REMOTE_BLOCKING_MENUS_URL: &str = concat!(
    "https://raw.githubusercontent.com/mblua/AgentsCommander/main/",
    "remote-resources/blocking-menus/v1/settings-blocking-menus.json",
);

/// #2133 - the published file: one pinned value built with concat! of literals whose ref must equal
/// `REMOTE_AGENT_HELP_SOURCE_REF` (P6's) and whose `v1` segment must equal
/// `AGENT_HELP_SCHEMA_VERSION`; T1 pins both.
pub(crate) const REMOTE_AGENT_HELP_URL: &str = concat!(
    "https://raw.githubusercontent.com/mblua/AgentsCommander/main/",
    "remote-resources/agent-help/v1/agent-help.json",
);

#[cfg(test)]
mod tests;
