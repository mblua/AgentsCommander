use super::super::projects;
use super::super::projects::ProjectPathPersistenceState;
#[cfg(test)]
use super::select_project_source_locked;
use super::{
    adopt_orphaned_profiles, agents_instance_path, agents_layer_state_at,
    agents_layer_state_at_labeled, apply_project_decode_to_value, canonical_profile_source,
    coding_agent_identity_physical_from_raw, collect_registered_profile_gate, default_zoom,
    duplicate_profile_document, empty_project_transaction,
    ensure_project_paths_migrated_at_labeled_path, export_agents_to_instance_file,
    export_blocking_menus_with_startup, finalize_agent_order, merge_agents_instance_groups,
    normalize_agent_backend_configs, normalize_agent_order, physical_profile_rows,
    production_instance_base, project_migration_error_labeled, project_physical_profiles,
    read_agents_instance_file, read_agents_instance_file_labeled, read_merged_top_level_key,
    read_raw_project_source_labeled_with_limit, recheck_registered_profile_gate,
    refresh_agent_id_hashes, refresh_pair_id_hashes, registered_profile_raw_gate,
    registered_profiles_for_read, repair_coding_agent_profiles_config,
    repair_loaded_response_close_settings, repair_loaded_room_idle_confirmation_settings,
    report_overlay_diagnostics, report_settings_save_error, restore_codex_home_carrier,
    save_settings_value_with_startup, select_project_source_at_labeled_path_with_limit,
    selected_registered_profile_owner, set_agents_layer_state, validate_agent_commands,
    validate_preflight_timeouts, AgentsLayerState, AppSettings, ProjectWriteMode,
    RegisteredProfileProblemKind, SettingsSaveError, SettingsSaveLegacyOutward, SettingsSaveReason,
    SettingsSaveReportSurface, SettingsSaveStage, StartupAgentRows, AGENTS_INSTANCE_KEYS,
    CONFIGURATION_COMMAND_PAIRS, FIELD_ARCHIVED, FIELD_ARCHIVED_REL, FIELD_PROJECT_PATH,
    FIELD_PROJECT_PATHS, FIELD_PROJECT_PATHS_REL, FIELD_PROJECT_PATH_REL,
    OVERLAY_DERIVED_ID_CLOSURES, OVERLAY_INELIGIBLE_DISK_KEYS, OVERLAY_INELIGIBLE_LEGACY_KEYS,
    OVERLAY_KEY_MAIN_ALWAYS_ON_TOP, OVERLAY_KEY_MAIN_GEOMETRY, OVERLAY_KEY_MAIN_ZOOM,
    OVERLAY_KEY_RESTORE_COORDINATOR_WAKE_STATE, RESPONSE_CLOSE_IDLE_SECONDS_MAX,
    RESPONSE_CLOSE_IDLE_SECONDS_MIN, ROOM_IDLE_CONFIRMATION_SECONDS_MAX,
    ROOM_IDLE_CONFIRMATION_SECONDS_MIN, TYPING_HOLD_SECONDS_MAX, TYPING_HOLD_SECONDS_MIN,
};
use crate::config::instance_artifacts::SETTINGS_FILE_NAME;
use crate::config::local_overlay::LocalSettingsOverlay;
use serde_json::{Map, Value};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod backup;
mod capture;
mod cli;
mod validate;
pub(crate) use backup::{
    rotate_settings_backups, rotate_settings_backups_labeled, write_pre_384_v1_backup,
};
#[cfg(test)]
pub(crate) use backup::{settings_backup_path, SETTINGS_BACKUP_KEEP};
pub(crate) use capture::{capture_project_settings, capture_project_settings_with_startup};
pub use cli::{load_settings_for_cli, load_settings_for_cli_strict};
#[cfg(test)]
pub(crate) use cli::{load_settings_for_cli_from_path, load_settings_for_cli_strict_from_path};
pub(crate) use validate::parse_api_server_socket_addr;
pub use validate::{
    config_seed_dest_os_token_stem, merge_protected_coding_agent_settings,
    validate_and_repair_settings, validate_api_server_settings, validate_config_seed_dest,
    validate_resource_settings, validate_room_number_mask, validate_screenshot_hotkey,
    validate_sidebar_compact_hotkey, validate_typing_hold_settings, CONFIG_SEED_OS_TOKENS,
    ROOM_NUMBER_MASK_MAX_WIDTH,
};

/// #1737 (D5): `settings_path` is `Some` on every production loader and `None` on
/// the in-module test call sites. `None` means "no overlay lookup".
#[cfg(test)]
pub(crate) fn parse_settings_json(
    contents: &str,
    source: &str,
    settings_path: Option<&Path>,
) -> Result<(AppSettings, bool), String> {
    match settings_path {
        None => parse_settings_json_with_projection(contents, source, None, None),
        Some(path) => {
            let transaction =
                empty_project_transaction(path, "parse_settings_json").map_err(|error| {
                    report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
                })?;
            let selected = select_project_source_locked(&transaction).map_err(|error| {
                report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings)
            })?;
            parse_settings_json_with_projection(
                contents,
                source,
                Some(path),
                Some(&selected.projection),
            )
        }
    }
}

#[cfg(test)]
pub(crate) fn parse_settings_json_with_projection(
    contents: &str,
    source: &str,
    settings_path: Option<&Path>,
    projection: Option<&Map<String, Value>>,
) -> Result<(AppSettings, bool), String> {
    parse_settings_json_with_projection_labeled(
        contents,
        source,
        settings_path,
        settings_path,
        projection,
    )
}

pub(crate) fn parse_settings_json_with_projection_labeled(
    contents: &str,
    source: &str,
    io_path: Option<&Path>,
    settings_path: Option<&Path>,
    projection: Option<&Map<String, Value>>,
) -> Result<(AppSettings, bool), String> {
    parse_settings_with_startup(contents, source, io_path, settings_path, projection, None)
}

pub(crate) fn parse_settings_with_startup(
    contents: &str,
    source: &str,
    io_path: Option<&Path>,
    settings_path: Option<&Path>,
    projection: Option<&Map<String, Value>>,
    startup: Option<&mut Option<StartupAgentRows>>,
) -> Result<(AppSettings, bool), String> {
    if let Some(reason) = duplicate_profile_document(
        contents.as_bytes(),
        settings_path.unwrap_or_else(|| Path::new(source)),
    ) {
        log::warn!("{reason}");
    }
    let value: Value = serde_json::from_str(contents)
        .map_err(|e| format!("Failed to parse settings file: {}", e))?;
    // One read of the agents file serves both the physical-group census and the
    // layer-30 merge; the merge label is the settings path, as before.
    let agents = io_path.map(|io| {
        read_agents_instance_file_labeled(
            &agents_instance_path(io),
            &agents_instance_path(settings_path.unwrap_or(io)),
        )
    });
    let (mut settings, legacy_profiles, agents_layer) = compose_settings_parts(
        value,
        source,
        settings_path,
        projection,
        agents,
        |value| match io_path {
            Some(p) => LocalSettingsOverlay::load_and_merge_paired(
                p,
                value,
                OVERLAY_INELIGIBLE_DISK_KEYS,
                OVERLAY_INELIGIBLE_LEGACY_KEYS,
                OVERLAY_DERIVED_ID_CLOSURES,
                CONFIGURATION_COMMAND_PAIRS,
            ),
            None => LocalSettingsOverlay::default(),
        },
        startup,
    )?;
    if let (Some(p), Some(state)) = (settings_path, agents_layer) {
        set_agents_layer_state(&mut settings, p, state);
    }
    repair_loaded_response_close_settings(&mut settings, source);
    repair_loaded_room_idle_confirmation_settings(&mut settings, source);
    Ok((settings, legacy_profiles))
}

/// One read of the agents file: `Ok(None)` is verified absence.
pub(crate) type AgentsInstanceRead = Result<Option<Map<String, Value>>, String>;

/// Settings, the legacy-profiles flag and the agents layer state, unregistered.
pub(crate) type ComposedSettings = (AppSettings, bool, Option<AgentsLayerState>);

/// The pure composition core of `parse_settings_with_startup`: projection, the
/// six-field decode, agents groups, carrier and physical identity, the overlay
/// over the same lists and pairs, serde, order and id hashes. It performs no IO
/// itself; the agents read and the overlay merge are supplied by the caller, so
/// the owner's single capture reuses this exact sequence over captured bytes.
/// `merge_label` is the settings path that enables the agents merge and the
/// undecodable-overlay fallback. The agents layer state is returned, not
/// registered, so the caller decides whether a process-wide registry changes.
pub(crate) fn compose_settings_parts(
    mut value: Value,
    source: &str,
    merge_label: Option<&Path>,
    projection: Option<&Map<String, Value>>,
    agents: Option<AgentsInstanceRead>,
    overlay_merge: impl FnOnce(&mut Value) -> LocalSettingsOverlay,
    mut startup: Option<&mut Option<StartupAgentRows>>,
) -> Result<ComposedSettings, String> {
    let settings_path = merge_label;
    if let (Some(object), Some(projection)) = (value.as_object_mut(), projection) {
        replace_project_projection(object, projection);
    }
    let legacy_profiles = value.as_object().is_some_and(legacy_profiles_shape_present);
    let main_profile_owner = value.as_object().expect("settings object").clone();
    let physical_sidecar = agents
        .as_ref()
        .and_then(|read| read.as_ref().ok())
        .and_then(Option::as_ref)
        .cloned();
    let physical_groups = agents
        .as_ref()
        .and_then(|read| read.as_ref().ok())
        .and_then(Option::as_ref)
        .map(|groups| {
            groups
                .keys()
                .filter(|key| AGENTS_INSTANCE_KEYS.contains(&key.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // #2716 (B3): the agents file is layer 30, under the layer-50 overlay, so it is
    // merged into the raw base object here: after the detection above, so
    // `legacy_profiles` stays a property of the settings file, and before the
    // `pre_merge` clone, so the undecodable fallback carries its groups.
    let agents_layer = settings_path.zip(agents).map(|(label, read)| {
        merge_agents_instance_groups(
            agents_instance_path(label),
            read,
            &mut value,
            startup.as_deref_mut(),
        )
    });
    // #1737 (D21): the clone must be taken BEFORE the merge, because the merge is
    // in place and the overlay's presence is not known until it returns. Gated on
    // `settings_path.is_some()`, which is true on all three production loaders and
    // false on the in-module test call sites. Cost: one Value clone per load.
    let pre_merge = settings_path.map(|_| value.clone());
    // #1737 (D5): the merge point, after the legacy-shape detection and before the
    // project decode. Merging after the detection keeps the `legacy_profiles` flag
    // a property of the base file alone and makes the B7 cross-key edge impossible.
    let physical_base = value.clone();
    let mut overlay = overlay_merge(&mut value);
    let profile_owner = selected_registered_profile_owner(
        &main_profile_owner,
        physical_sidecar.as_ref(),
        !overlay.owns_top_level("codingAgentProfiles"),
    );
    let profile_owner_view = registered_profiles_for_read(&profile_owner);
    if let (Some(object), Some(base)) = (value.as_object_mut(), &pre_merge) {
        restore_codex_home_carrier(object, base.get("codingAgentProfiles"));
    }
    let mut identity_physical =
        coding_agent_identity_physical_from_raw(&value, &physical_groups, &overlay);
    let unmerged = if overlay.is_empty() { None } else { pre_merge };
    // #1077: decode the six project fields and replace the three runtime fields
    // with the SELECTED canonical paths BEFORE deserializing the rest of
    // AppSettings, so a wrong-typed project field cannot fail unrelated settings
    // deserialization and unresolved/conflicting pairs survive in hidden state.
    let base = production_instance_base();
    // #2716 (B3, option C): with the agents file unreadable, the decode error also
    // names it, so the dialog does not name only the intact settings file.
    let unreadable_agents_file = agents_layer
        .as_ref()
        .and_then(AgentsLayerState::unreadable_path)
        .map(|p| format!("; the agents file {} could not be read", p.display()))
        .unwrap_or_default();
    let decode_error = |e: serde_json::Error| {
        format!("Failed to deserialize settings from {source}: {e}{unreadable_agents_file}")
    };
    let mut projected = physical_base.clone();
    if let Some(profiles) = profile_owner_view.get("codingAgentProfiles") {
        projected["codingAgentProfiles"] = profiles.clone();
    }
    if let (Some(effective), Some(base)) = (value.as_object(), projected.as_object_mut()) {
        overlay.reapply_from(effective, base);
    }
    value = projected;
    let state =
        apply_project_decode_to_value(&mut value, base.as_deref(), &projects::FsCandidateResolver);
    let decoded: Result<AppSettings, String> = serde_json::from_value(value).map_err(decode_error);
    // #1737 (D21): a merged value that fails to decode falls back to the BASE
    // value, never to defaults. One wrong-typed leaf in settings.local.json must
    // not replace a valid base configuration.
    let (mut settings, state) = match (decoded, unmerged) {
        (Ok(settings), _) => (settings, state),
        (Err(reason), Some(mut unmerged)) => {
            identity_physical["agents"] = unmerged.get("agents").cloned().unwrap_or(Value::Null);
            identity_physical["codingAgentProfiles"] = unmerged
                .get("codingAgentProfiles")
                .cloned()
                .unwrap_or(Value::Null);
            identity_physical["quotaSources"] =
                unmerged.get("quotaSources").cloned().unwrap_or(Value::Null);
            identity_physical["groupOwnership"]["overlayPaths"] = serde_json::json!([]);
            let base_state = apply_project_decode_to_value(
                &mut unmerged,
                base.as_deref(),
                &projects::FsCandidateResolver,
            );
            if let Some(profiles) = profile_owner_view.get("codingAgentProfiles") {
                unmerged["codingAgentProfiles"] = profiles.clone();
            }
            let settings: AppSettings = serde_json::from_value(unmerged).map_err(decode_error)?;
            overlay = overlay.into_undecodable(reason);
            (settings, base_state)
        }
        (Err(reason), None) => return Err(reason),
    };
    settings.project_path_state = Arc::new(state);
    settings.coding_agent_identity_physical = Some(Arc::new(identity_physical));
    // #2306 P1 - in-memory finalize over the merged view. It is deliberately not
    // part of `needs_save`: a legacy file with no ordinals must load without a
    // disk write or a backup rotation.
    finalize_agent_order(&mut settings.agents);
    refresh_agent_id_hashes(&mut settings.agents);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);
    report_overlay_diagnostics(source, &overlay);
    settings.local_overlay_state = Arc::new(overlay);
    if settings.local_overlay_state.owns_top_level("agents") {
        if let Some(startup) = startup {
            *startup = None;
        }
    }
    Ok((settings, legacy_profiles, agents_layer))
}

/// #2015: legacy pre-v2 codingAgentProfiles are no longer migrated. Detection only;
/// never mutates the value.
pub(crate) fn legacy_profiles_shape_present(root: &Map<String, Value>) -> bool {
    let Some(p) = root.get("codingAgentProfiles").and_then(Value::as_object) else {
        return false;
    };
    if ["letters", "matrix", "agentDefaults"]
        .iter()
        .any(|key| p.contains_key(*key))
    {
        return true;
    }
    if matches!(p.get("schemaVersio\
        n"), Some(Value::Number(n)) if n.as_f64().is_some_and(|v| v < 2.0))
    {
        return true;
    }
    if p.get("profileSlots")
        .and_then(Value::as_object)
        .is_some_and(|slots| {
            slots
                .values()
                .any(|slot| slot.as_object().is_some_and(|s| s.contains_key("name")))
        })
    {
        return true;
    }
    p.get("profilesByAgent")
        .and_then(Value::as_object)
        .is_some_and(|by_agent| {
            by_agent.values().filter_map(Value::as_object).any(|cells| {
                cells
                    .values()
                    .filter_map(Value::as_object)
                    .any(|cell| cell.contains_key("argv") || cell.contains_key("args"))
            })
        })
}

// A preview of this owner's profile property only. Retained opaque legacy
// leaves are diagnostics, not a reason to republish canonical data forever.
pub(crate) fn actionable_profile_legacy_work(
    owner: &Value,
    allow_main_cleanup: bool,
) -> Result<bool, String> {
    let Some(profiles) = owner.get("codingAgentProfiles").and_then(Value::as_object) else {
        return Ok(false);
    };
    let stale_active_maps = ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"]
        .iter()
        .any(|key| profiles.contains_key(*key));
    let removable_roots = allow_main_cleanup
        && ["letters", "matrix", "agentDefaults"]
            .iter()
            .any(|key| profiles.contains_key(*key));
    if profiles.get("profileBindingVersion") == Some(&Value::from(1))
        && !stale_active_maps
        && !removable_roots
    {
        return Ok(false);
    }
    let projected = project_physical_profiles(owner, true)?;
    let mut candidate = canonical_profile_source(owner, &projected, true)?;
    if let Some(profiles) = candidate
        .get_mut("codingAgentProfiles")
        .and_then(Value::as_object_mut)
        .filter(|_| allow_main_cleanup)
    {
        for key in ["letters", "matrix", "agentDefaults"] {
            profiles.remove(key);
        }
    }
    Ok(candidate.get("codingAgentProfiles") != owner.get("codingAgentProfiles"))
}

pub(crate) fn startup_profile_legacy_work(
    settings: &AppSettings,
    path: &Path,
    original: Option<&str>,
    main_legacy_detected: bool,
) -> Result<Option<bool>, String> {
    // O2 explicitly retains baseline scheduling for suppressed groups.
    if settings
        .local_overlay_state
        .owns_top_level("codingAgentProfiles")
    {
        return Ok(Some(main_legacy_detected));
    }
    let main: Value =
        serde_json::from_str(original.unwrap_or("{}")).map_err(|error| error.to_string())?;
    let sidecar = read_agents_instance_file(&agents_instance_path(path))?;
    let sidecar_profiles = sidecar
        .as_ref()
        .is_some_and(|groups| groups.contains_key("codingAgentProfiles"));
    let owner = selected_registered_profile_owner(
        main.as_object().ok_or("invalidSettingsObject")?,
        sidecar.as_ref(),
        true,
    );
    let encountered = owner
        .get("codingAgentProfiles")
        .and_then(Value::as_object)
        .is_some_and(|profiles| {
            ["profilesByAgent", "profileLabelsByAgent", "identityByAgent"]
                .iter()
                .any(|key| profiles.contains_key(*key))
        });
    // An admitted legacy transfer is ordinary publication work even when
    // ambiguous rows forbid conversion. The locked writer makes that decision.
    let main_transfer = AGENTS_INSTANCE_KEYS.iter().any(|key| {
        if settings.local_overlay_state.owns_top_level(key) {
            return false;
        }
        main.get(*key).is_some()
            && !sidecar
                .as_ref()
                .is_some_and(|groups| groups.contains_key(*key))
    }) && owner.pointer("/codingAgentProfiles/profileBindingVersion")
        != Some(&Value::from(1));
    if main_transfer {
        return Ok(Some(true));
    }
    if owner.get("codingAgentProfiles").is_none()
        && owner.get("agents").is_some()
        && physical_profile_rows(&owner, true).is_ok_and(|rows| !rows.is_empty())
    {
        return Ok(Some(true));
    }
    if !owner.as_object().is_some_and(legacy_profiles_shape_present) && !encountered {
        return Ok(Some(false));
    }
    // Ambiguous raw row paths do not grant new canonical cleanup authority.
    if physical_profile_rows(&owner, true).is_err() {
        return Ok(None);
    }
    actionable_profile_legacy_work(&owner, !sidecar_profiles).map(Some)
}

/// #1737 (D15) - the settings value when the base `settings.json` is absent,
/// unreadable or unparseable. A valid `settings.local.json` still applies, over the
/// serialized defaults, so the file the following `needs_save` write creates holds
/// exactly what a no-overlay fresh instance would have written and deleting the
/// local file restores it.
#[cfg(test)]
pub(crate) fn default_settings_with_overlay(settings_path: &Path, _source: &str) -> AppSettings {
    match capture_project_settings(settings_path, false) {
        Ok((settings, _, _)) => settings,
        Err(error) => unavailable_project_settings(AppSettings::default(), error),
    }
}

pub(crate) fn default_settings_with_overlay_captured(
    settings_path: &Path,
    source: &str,
    projection: &Map<String, Value>,
) -> AppSettings {
    // Total by construction: `AppSettings` is a struct with named fields, so serde
    // always produces an object. The arm exists so the function has no panic.
    let Ok(mut value @ Value::Object(_)) = serde_json::to_value(AppSettings::default()) else {
        return AppSettings::default();
    };
    let overlay = LocalSettingsOverlay::load_and_merge_paired(
        settings_path,
        &mut value,
        OVERLAY_INELIGIBLE_DISK_KEYS,
        OVERLAY_INELIGIBLE_LEGACY_KEYS,
        OVERLAY_DERIVED_ID_CLOSURES,
        CONFIGURATION_COMMAND_PAIRS,
    );
    let agents_base = read_agents_instance_file(&agents_instance_path(settings_path))
        .ok()
        .flatten();
    restore_codex_home_carrier(
        value.as_object_mut().unwrap(),
        agents_base
            .as_ref()
            .and_then(|base| base.get("codingAgentProfiles")),
    );
    // Unconditionally, and BEFORE the early return: `is_empty()` is also true for a
    // REJECTED overlay, so returning first would swallow the very diagnostics D8
    // mandates on a malformed local file.
    report_overlay_diagnostics(source, &overlay);
    replace_project_projection(value.as_object_mut().unwrap(), projection);
    let state = apply_project_decode_to_value(
        &mut value,
        production_instance_base().as_deref(),
        &projects::FsCandidateResolver,
    );
    match serde_json::from_value::<AppSettings>(value) {
        Ok(mut settings) => {
            settings.project_path_state = Arc::new(state);
            // #2306 P1 - same in-memory finalize as the normal parse, applied to
            // the defaults-plus-overlay view; still no write.
            finalize_agent_order(&mut settings.agents);
            refresh_agent_id_hashes(&mut settings.agents);
            refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);
            settings.local_overlay_state = Arc::new(overlay);
            repair_loaded_response_close_settings(&mut settings, source);
            repair_loaded_room_idle_confirmation_settings(&mut settings, source);
            settings
        }
        Err(e) => {
            // D21 with `AppSettings::default()` as the fallback. 4.2e step 5 says
            // "record `MergedValueUndecodable`, render THAT record", singular: the
            // ineligible-key drops were already rendered above and
            // `into_undecodable` preserves them, so rendering this overlay's whole
            // record list would emit every drop a second time. A default overlay
            // carries the new record alone, and the value returned is
            // `AppSettings::default()` either way, so the state the caller adopts
            // is unchanged.
            let undecodable = LocalSettingsOverlay::default().into_undecodable(e.to_string());
            report_overlay_diagnostics(source, &undecodable);
            AppSettings::default()
        }
    }
}

pub(crate) fn replace_project_projection(
    object: &mut Map<String, Value>,
    projection: &Map<String, Value>,
) {
    for key in [
        FIELD_PROJECT_PATH,
        FIELD_PROJECT_PATH_REL,
        FIELD_PROJECT_PATHS,
        FIELD_PROJECT_PATHS_REL,
        FIELD_ARCHIVED,
        FIELD_ARCHIVED_REL,
    ] {
        object.remove(key);
    }
    object.extend(projection.clone());
}

pub(crate) fn unavailable_project_settings(
    mut settings: AppSettings,
    error: String,
) -> AppSettings {
    settings.project_paths.clear();
    settings.project_path = None;
    settings.archived_project_paths.clear();
    settings.project_path_state = Arc::new(ProjectPathPersistenceState::default());
    settings.project_paths_persistence_error = Some(error);
    settings
}

#[cfg(test)]
pub(crate) type SettingsPathHook = Option<Box<dyn Fn(&Path)>>;

#[cfg(test)]
thread_local! {
    pub(crate) static PROJECT_SOURCE_CAPTURE_HOOK: std::cell::RefCell<SettingsPathHook> =
        std::cell::RefCell::new(None);
    pub(crate) static SETTINGS_PRESERVE_LOCK_HOOK: std::cell::RefCell<SettingsPathHook> =
        std::cell::RefCell::new(None);
}

pub(crate) fn settings_path() -> Option<PathBuf> {
    super::super::config_dir().map(|d| d.join(SETTINGS_FILE_NAME))
}

/// Load settings from the app config directory (see config_dir()), falling back to defaults.
/// Auto-generates a root_token if missing and persists it.
pub fn load_settings() -> AppSettings {
    let path = match settings_path() {
        Some(p) => p,
        None => {
            log::warn!("Could not determine home directory, using defaults");
            return AppSettings::default();
        }
    };

    load_settings_from_path(&path)
}

/// `pub(crate)` only so `commands::config`'s #1737 tests can seed a
/// `settings.json` plus `settings.local.json` pair in a tempdir and load it
/// through the real loader instead of reproducing the loader in a test. No
/// production caller outside this module.
pub(crate) fn load_settings_from_path(path: &Path) -> AppSettings {
    load_settings_from_path_with_blocking_menus_publish(path, None)
}

pub(crate) fn load_settings_from_path_with_blocking_menus_publish(
    path: &Path,
    publish: Option<crate::config::local_config_io::ConfigPublisher<'_>>,
) -> AppSettings {
    let mut startup = None;
    let (mut settings, legacy_profiles_detected, legacy_profiles_contents) =
        read_settings_for_startup_with_capture(path, Some(&mut startup));
    if settings.project_paths_persistence_error.is_some() {
        return settings;
    }
    apply_loaded_window_migrations(&mut settings);

    // The main-file diagnostic remains separate for its original-byte backup.
    // A sidecar-owned legacy cause must be reachable without that diagnostic.
    let (legacy_profile_work, profile_preview_rejected) = match startup_profile_legacy_work(
        &settings,
        path,
        legacy_profiles_contents.as_deref(),
        legacy_profiles_detected,
    ) {
        Ok(Some(actionable)) => (actionable, false),
        Ok(None) => {
            log::warn!("[profiles] legacy action awaits unambiguous owner routing");
            (legacy_profiles_detected, false)
        }
        Err(reason) => {
            log::warn!("[profiles] legacy action preview rejected: {reason}");
            (false, true)
        }
    };

    // #248 migration — translates legacy startOnlyCoordinators (if present).
    // Track whether the legacy field was on disk so we can fire a single save
    // at the end of the function (Grinch Z3 — without this, upgrade users
    // with an existing root_token never persist the migration and the legacy
    // key lingers in settings.json, spamming the migration log on every launch).
    let issue_248_migrated = settings.legacy_start_only_coordinators.is_some();
    apply_issue_248_migration(&mut settings);

    // Auto-generate root token if missing.
    let mut needs_save = issue_248_migrated || legacy_profile_work;
    if settings.root_token.is_none() {
        settings.root_token = Some(uuid::Uuid::new_v4().to_string());
        log::info!("Generated new root token");
        needs_save = true;
    }
    let mut startup_writable = if profile_preview_rejected {
        Err("profile migration preview rejected; no startup publication".to_owned())
    } else {
        registered_profile_raw_gate(path)
    };
    if legacy_profile_work
        && startup_writable.is_ok()
        && !settings
            .local_overlay_state
            .owns_top_level("codingAgentProfiles")
    {
        // Enter the admitted Preserve funnel before raw exports/adoption/repair.
        // Its complete source preflight precedes migration and backup publishers.
        match save_settings_value_with_startup(
            &settings,
            path,
            ProjectWriteMode::Preserve,
            startup.as_ref(),
        ) {
            Ok(written) => {
                settings = written;
                needs_save = false;
                // A successful transfer changes the physical row owner. Later
                // independent startup work must use a fresh correspondence image.
                if let Err(reason) =
                    capture_project_settings_with_startup(path, false, Some(&mut startup))
                {
                    startup_writable = Err(reason);
                }
            }
            Err(error) => {
                let reason =
                    report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings);
                startup_writable = Err(reason);
            }
        }
    }
    if let Err(reason) = &startup_writable {
        log::warn!("[profiles] startup publication skipped: {reason}");
    }
    let exported = startup_writable.is_ok()
        && export_blocking_menus_with_startup(
            &mut settings,
            path,
            publish.unwrap_or(&crate::config::local_config_io::publish_temp_config),
            startup.as_mut(),
        );
    if exported {
        log::info!("[settings-migration] #1905 - moved blockingMenus out of settings.json");
        needs_save = true;
    }
    if startup_writable.is_ok() && export_agents_to_instance_file(&mut settings, path) {
        needs_save = true;
    }
    if startup_writable.is_ok() && adopt_orphaned_profiles(&mut settings) {
        needs_save = true;
    }
    if startup_writable.is_ok()
        && repair_coding_agent_profiles_config(
            &mut settings.coding_agent_profiles,
            &settings.agents,
        )
    {
        log::info!("[settings-migration] repaired codingAgentProfiles invariants");
        needs_save = true;
    }
    if needs_save && startup_writable.is_ok() {
        persist_loaded_settings_migrations(
            &mut settings,
            path,
            legacy_profiles_detected,
            &legacy_profiles_contents,
            startup.as_ref(),
        );
    }

    // Startup migrations may move ownership or materialize profiles. Birth seeds
    // must describe the post-migration physical inputs, never the provisional load.
    settings.coding_agent_identity_physical = capture_project_settings(path, false)
        .ok()
        .and_then(|(captured, _, _)| captured.coding_agent_identity_physical);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);
    settings
}

#[cfg(test)]
pub(crate) fn read_settings_for_startup(path: &Path) -> (AppSettings, bool, Option<String>) {
    read_settings_for_startup_with_capture(path, None)
}

pub(crate) fn read_settings_for_startup_with_capture(
    path: &Path,
    startup: Option<&mut Option<StartupAgentRows>>,
) -> (AppSettings, bool, Option<String>) {
    match capture_project_settings_with_startup(path, true, startup) {
        Ok(captured) => captured,
        Err(error) => {
            let mut settings = AppSettings::default();
            set_agents_layer_state(&mut settings, path, agents_layer_state_at(path));
            (unavailable_project_settings(settings, error), false, None)
        }
    }
}

pub(crate) fn apply_loaded_window_migrations(settings: &mut AppSettings) {
    // 0.8.0 unified-window migration — seed main_* from legacy fields on first load
    // after upgrade. Runs BEFORE root_token auto-gen so the migrated values persist
    // via the same save. The deprecated `sidebar_geometry`/`terminal_geometry` fields
    // are automatically dropped from disk by `skip_serializing_if` on the next save.
    // #1737 (D7c): each of these three migrations writes its destination key from a
    // legacy source on the typed struct AFTER the merge, so an overlay that owns the
    // destination would have its override silently overwritten from the base file.
    // Owning the destination suppresses the migration; with no overlay in force
    // `owns_top_level` is false for every key and the behaviour is unchanged.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_GEOMETRY)
        && settings.main_geometry.is_none()
    {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
            log::info!("[settings-migration] seeded main_geometry from legacy terminal_geometry");
        }
    }
    // Seed main_zoom from sidebar_zoom on first boot. EPSILON guard: avoid clobbering
    // a user-set main_zoom=1.0 (which would equal default_zoom) with an effectively-unity
    // sidebar_zoom. See A3.10 / Arb-2.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ZOOM)
        && (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
        log::info!("[settings-migration] seeded main_zoom from legacy sidebar_zoom");
    }
    // Seed main_always_on_top from legacy sidebar_always_on_top.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ALWAYS_ON_TOP)
        && !settings.main_always_on_top
        && settings.sidebar_always_on_top
    {
        settings.main_always_on_top = true;
        log::info!(
            "[settings-migration] seeded main_always_on_top from legacy sidebar_always_on_top"
        );
    }
}

pub(crate) fn persist_loaded_settings_migrations(
    settings: &mut AppSettings,
    path: &Path,
    legacy_profiles_detected: bool,
    legacy_profiles_contents: &Option<String>,
    startup: Option<&StartupAgentRows>,
) {
    let backup_ok = if legacy_profiles_detected {
        match legacy_profiles_contents.as_deref() {
            Some(contents) => match write_pre_384_v1_backup(path, contents) {
                Ok(()) => true,
                Err(e) => {
                    log::error!(
                        "Failed to persist settings v2 migration backup; leaving settings.json \
                            untouched: {}",
                        e
                    );
                    false
                }
            },
            None => true,
        }
    } else {
        true
    };
    if backup_ok {
        // #2306 P1 - the migration write is authorized, so persist explicit
        // contiguous ordinals. Finalize already ran at load; this renumber is
        // the write-side guarantee and cannot re-sort or revive stale values.
        normalize_agent_order(&mut settings.agents);
        // #1077: route the startup root-token/migration write through PRESERVE
        // mode so it cannot erase project companions/conflicts before the
        // first snapshot, and so a present-but-invalid file is never
        // overwritten (the preserve writer's disk gate returns Err). On
        // success adopt the fresh-decoded settings so runtime/hidden agree.
        match save_settings_value_with_startup(settings, path, ProjectWriteMode::Preserve, startup)
        {
            Ok(written) => *settings = written,
            Err(error) => {
                let _ =
                    report_settings_save_error(error, SettingsSaveReportSurface::GeneralSettings);
            }
        }
    }
}

/// One-shot migration for issue #248: translate the legacy
/// `startOnlyCoordinators` field into the new state-sensitive
/// `restore_coordinator_wake_state`. Idempotent — once the legacy carrier is
/// cleared (`.take()`), subsequent calls see `None` and do nothing.
///
/// Translation rules:
///   - legacy `true`  → new `true`  (preserve "smart startup" intent under new semantics).
///   - legacy `false` → new `false` (legacy "wake everything" mode is removed; closest
///     equivalent under the new model is "wake nothing").
///
/// Conflict handling (Grinch Z4): if the user (or a third-party tool) wrote
/// BOTH keys and the new field already differs from the legacy intent, emit a
/// `warn!` and keep the new field's existing value — never silently overwrite
/// a deliberate `restoreCoordinatorWakeState` with a stale legacy value.
pub(crate) fn apply_issue_248_migration(settings: &mut AppSettings) {
    // #1737 (D7c): the overlay owns `restoreCoordinatorWakeState`, so this migration
    // would overwrite the operator's value from the base file's legacy key. Return
    // WITHOUT taking the legacy carrier: dropping it here would delete
    // `startOnlyCoordinators` from the base file while `restore_base` simultaneously
    // restored `restoreCoordinatorWakeState` to its (usually absent) base value, and
    // the migration would be lost rather than deferred. Leaving the carrier in place
    // costs one idempotent save per launch and makes the migration run correctly the
    // first time the overlay is removed. See plan D7c and evidence 2.11b row R4.
    if settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_RESTORE_COORDINATOR_WAKE_STATE)
    {
        return;
    }
    if let Some(legacy) = settings.legacy_start_only_coordinators.take() {
        if !settings.restore_coordinator_wake_state {
            settings.restore_coordinator_wake_state = legacy;
            log::info!(
                "[settings-migration] #248 — translated legacy startOnlyCoordinators={} → \
                    restoreCoordinatorWakeState={}",
                legacy,
                settings.restore_coordinator_wake_state
            );
        } else if legacy != settings.restore_coordinator_wake_state {
            log::warn!(
                "[settings-migration] #248 — conflicting state on disk: legacy \
                    startOnlyCoordinators={} but restoreCoordinatorWakeState={} already set; \
                    keeping the new value, dropping legacy",
                legacy,
                settings.restore_coordinator_wake_state
            );
        }
        // else: legacy and new agree → silent drop, no log.
    }
}

/// Read only the `logLevel` field from `settings.json` without triggering migrations,
/// auto-token-gen, or any in-memory mutation. Used by `lib.rs` at logger-init time so
/// the full `load_settings` flow can run post-init with log calls captured.
///
/// Returns `None` on missing file, missing field, malformed JSON, unreadable filesystem,
/// or any other read error — fully read-only and side-effect-free.
/// #1737 (D16): routed through the merged (base plus `settings.local.json`) view.
pub(crate) fn read_log_level_from_path(path: &std::path::Path) -> Option<String> {
    read_merged_top_level_key(path, "logLevel")?
        .as_str()
        .map(String::from)
}

/// See `read_log_level_from_path`. Resolves the canonical settings path and delegates.
pub fn read_log_level_only() -> Option<String> {
    read_log_level_from_path(&settings_path()?)
}

/// #1737 (D16): routed through the merged (base plus `settings.local.json`) view.
pub(crate) fn read_activity_log_enabled_from_path(path: &std::path::Path) -> bool {
    read_merged_top_level_key(path, "activityLogEnabled")
        .as_ref()
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub fn read_activity_log_enabled_only() -> bool {
    match settings_path() {
        Some(path) => read_activity_log_enabled_from_path(&path),
        None => false,
    }
}
