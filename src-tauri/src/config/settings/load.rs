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
use crate::config::instance_artifacts::{
    SETTINGS_BACKUP_PREFIX, SETTINGS_BACKUP_SUFFIX, SETTINGS_FILE_NAME,
};
use crate::config::local_overlay::LocalSettingsOverlay;
use serde_json::{Map, Value};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

pub(crate) fn capture_project_settings(
    path: &Path,
    migrate: bool,
) -> Result<(AppSettings, bool, Option<String>), String> {
    capture_project_settings_with_startup(path, migrate, None)
}

pub(crate) fn capture_project_settings_with_startup(
    path: &Path,
    migrate: bool,
    mut startup: Option<&mut Option<StartupAgentRows>>,
) -> Result<(AppSettings, bool, Option<String>), String> {
    let lock_error = |reason| {
        SettingsSaveError::semantic(
            SettingsSaveStage::LockAcquire,
            path,
            None,
            reason,
            SettingsSaveLegacyOutward::SettingsLockUnavailable,
        )
    };
    let report = |error: SettingsSaveError| {
        report_settings_save_error(
            error.with_capture_labels(path),
            SettingsSaveReportSurface::GeneralSettings,
        )
    };
    let parent = path
        .parent()
        .ok_or_else(|| report(lock_error(SettingsSaveReason::MissingParentDirectory)))?;
    std::fs::create_dir_all(parent).map_err(|source| {
        report(SettingsSaveError::io(
            SettingsSaveStage::LockAcquire,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::SettingsLockUnavailable,
        ))
    })?;
    match crate::path_identity::verify_component_chain_reason(parent) {
        Ok(()) | Err(crate::path_identity::PathRule::ComponentIsLink) => {}
        Err(_) => return Err(report(lock_error(SettingsSaveReason::LockIdentityRejected))),
    }
    let physical_parent = std::fs::canonicalize(parent)
        .map_err(|_| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    // Windows retains a no-share-delete handle. Unix detects persistent
    // replacement, not transient A -> B -> A absolute-path substitution.
    let retained = crate::path_identity::retain_directory(&physical_parent)
        .map_err(|_| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    let leaf = path
        .file_name()
        .ok_or_else(|| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    let physical = retained.identity().canonical_path.join(leaf);
    let transaction = empty_project_transaction(&physical, "capture_project_settings")
        .map_err(|error| report(error.with_capture_labels(path)))?;
    retained
        .verify_current()
        .map_err(|_| report(lock_error(SettingsSaveReason::LockIdentityRejected)))?;
    #[cfg(test)]
    PROJECT_SOURCE_CAPTURE_HOOK.with(|hook| {
        if let Some(callback) = hook.borrow().as_ref() {
            callback(path);
        }
    });
    let raw_profile_capture = collect_registered_profile_gate(&transaction.path);
    if let Some(reason) = raw_profile_capture.ordinary_error() {
        log::warn!("[profiles] {reason}");
    }
    let migration_error = if migrate
        && raw_profile_capture.admits_project_migration(&transaction.path)
    {
        let mut checked = false;
        ensure_project_paths_migrated_at_labeled_path(&transaction.path, path, &mut |stage, _| {
            if !checked
                && matches!(
                    stage,
                    "backup_create" | "destination_before_write" | "source_cleanup"
                )
            {
                recheck_registered_profile_gate(&raw_profile_capture.participant_documents())
                    .map_err(|reason| {
                        project_migration_error_labeled(
                            &transaction.path,
                            path,
                            SettingsSaveStage::ProjectPathsRefreshDiskGate,
                            &reason,
                        )
                    })?;
                checked = true;
            }
            Ok(())
        })
        .err()
    } else {
        None
    };
    let selected = match migration_error {
        Some(error) => Err(error),
        None => select_project_source_at_labeled_path_with_limit(
            &transaction.path,
            path,
            16 * 1024 * 1024,
        ),
    };
    let captured = if raw_profile_capture
        .problems
        .iter()
        .any(|problem| problem.kind == RegisteredProfileProblemKind::DuplicateMember)
        && selected
            .as_ref()
            .err()
            .is_none_or(|error| error.target_path.as_path() == transaction.path.as_path())
    {
        // The ordinary reader remains lenient. The strict project selector must
        // not turn an unrelated duplicate into project persistence failure.
        let contents = std::fs::read_to_string(&transaction.path).unwrap_or_else(|_| {
            serde_json::to_string(&AppSettings::default()).expect("default settings")
        });
        let legacy: Value =
            serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}));
        let mut probe = legacy.clone();
        apply_project_decode_to_value(
            &mut probe,
            production_instance_base().as_deref(),
            &projects::FsCandidateResolver,
        );
        let projection = selected
            .as_ref()
            .ok()
            .map(|selected| selected.projection.clone())
            .unwrap_or_else(|| {
                probe
                    .as_object()
                    .map(|map| {
                        let mut projection = Map::new();
                        for key in [
                            FIELD_PROJECT_PATH,
                            FIELD_PROJECT_PATHS,
                            FIELD_ARCHIVED,
                            FIELD_PROJECT_PATH_REL,
                            FIELD_PROJECT_PATHS_REL,
                            FIELD_ARCHIVED_REL,
                        ] {
                            if let Some(value) = map.get(key) {
                                projection.insert(key.to_owned(), value.clone());
                            }
                        }
                        projection
                    })
                    .unwrap_or_default()
            });
        let parsed = parse_settings_with_startup(
            &contents,
            &path.to_string_lossy(),
            Some(&transaction.path),
            Some(path),
            Some(&projection),
            startup.as_deref_mut(),
        );
        parsed.map(|(settings, legacy)| (settings, legacy, Some(contents)))
    } else {
        (|| {
            match selected {
                Ok(selected) => match selected.legacy {
                    Some(legacy) => {
                        let contents =
                            String::from_utf8(legacy.bytes).map_err(|error| error.to_string())?;
                        match parse_settings_with_startup(
                            &contents,
                            &path.to_string_lossy(),
                            Some(&transaction.path),
                            Some(path),
                            Some(&selected.projection),
                            startup.as_deref_mut(),
                        ) {
                            Ok((settings, legacy_profiles)) => {
                                Ok((settings, legacy_profiles, Some(contents)))
                            }
                            Err(error) => {
                                if let Some(startup) = startup {
                                    *startup = None;
                                }
                                log::error!("{error}");
                                // Selection succeeded; unrelated decode failure must not
                                // mark the authoritative project source unavailable.
                                let mut settings = default_settings_with_overlay_captured(
                                    &transaction.path,
                                    &path.to_string_lossy(),
                                    &selected.projection,
                                );
                                set_agents_layer_state(
                                    &mut settings,
                                    path,
                                    agents_layer_state_at_labeled(&transaction.path, path),
                                );
                                Ok((settings, false, None))
                            }
                        }
                    }
                    None => Ok((
                        default_settings_with_overlay_captured(
                            &transaction.path,
                            &path.to_string_lossy(),
                            &selected.projection,
                        ),
                        false,
                        None,
                    )),
                },
                Err(error) => {
                    let message = report_settings_save_error(
                        error.with_capture_labels(path),
                        SettingsSaveReportSurface::GeneralSettings,
                    );
                    // Preserve readable unrelated values while exposing no project authorization.
                    let mut settings = read_raw_project_source_labeled_with_limit(
                        &transaction.path,
                        path,
                        16 * 1024 * 1024,
                        false,
                    )
                    .ok()
                    .flatten()
                    .and_then(|legacy| {
                        let contents = String::from_utf8(legacy.bytes).ok()?;
                        parse_settings_json_with_projection_labeled(
                            &contents,
                            &path.to_string_lossy(),
                            Some(&transaction.path),
                            Some(path),
                            Some(&Map::new()),
                        )
                        .ok()
                        .map(|pair| pair.0)
                    })
                    .unwrap_or_else(|| {
                        default_settings_with_overlay_captured(
                            &transaction.path,
                            &path.to_string_lossy(),
                            &Map::new(),
                        )
                    });
                    set_agents_layer_state(
                        &mut settings,
                        path,
                        agents_layer_state_at_labeled(&transaction.path, path),
                    );
                    Ok((unavailable_project_settings(settings, message), false, None))
                }
            }
        })()
    };
    // Every fallback passes this check while the same lock is still held.
    if retained.verify_current().is_err() {
        let message = report(lock_error(SettingsSaveReason::LockIdentityRejected));
        return match captured {
            Ok((settings, _, _)) => {
                Ok((unavailable_project_settings(settings, message), false, None))
            }
            Err(_) => Err(message),
        };
    }
    captured
}

pub fn validate_and_repair_settings(settings: &mut AppSettings) -> Result<(), String> {
    if !(RESPONSE_CLOSE_IDLE_SECONDS_MIN..=RESPONSE_CLOSE_IDLE_SECONDS_MAX)
        .contains(&settings.response_close_idle_seconds)
    {
        return Err("responseCloseIdleSeconds must be between 1 and 3600 seconds".to_string());
    }
    if !(ROOM_IDLE_CONFIRMATION_SECONDS_MIN..=ROOM_IDLE_CONFIRMATION_SECONDS_MAX)
        .contains(&settings.room_idle_confirmation_seconds)
    {
        return Err("roomIdleConfirmationSeconds must be between 1 and 3600 seconds".to_string());
    }
    validate_preflight_timeouts(settings)?;
    adopt_orphaned_profiles(settings);
    normalize_agent_backend_configs(settings)?;
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);
    validate_agent_commands(settings)?;
    refresh_agent_id_hashes(&mut settings.agents);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);
    validate_screenshot_hotkey(&settings.screenshot_capture_hotkey)?;
    validate_sidebar_compact_hotkey(&settings.sidebar_compact_hotkey)?;
    validate_room_number_mask(&settings.room_number_mask)?;
    validate_api_server_settings(settings)?;
    validate_typing_hold_settings(settings)?;
    validate_resource_settings(settings)
}

pub const ROOM_NUMBER_MASK_MAX_WIDTH: usize = 9;

/// #2507 Returns the pad width. `Err` blocks the settings save.
pub fn validate_room_number_mask(value: &str) -> Result<usize, String> {
    let reject = |reason: &str| Err(format!("Room number mask: {reason}"));
    if value.is_empty() {
        return reject("must be at least one '#'");
    }
    if !value.chars().all(|c| c == '#') {
        return reject("must contain only '#' characters, e.g. ## or ###");
    }
    if value.len() > ROOM_NUMBER_MASK_MAX_WIDTH {
        return reject(&format!(
            "must be at most {ROOM_NUMBER_MASK_MAX_WIDTH} '#' characters"
        ));
    }
    Ok(value.len())
}

/// #2336 - reject a settings update whose typing-hold window is outside the
/// accepted range. Called on every save path, so the invalid candidate is
/// discarded before `save_settings` and the persisted (and live) value stays
/// untouched.
pub fn validate_typing_hold_settings(settings: &AppSettings) -> Result<(), String> {
    let seconds = settings.typing_hold_seconds;
    if !(TYPING_HOLD_SECONDS_MIN..=TYPING_HOLD_SECONDS_MAX).contains(&seconds) {
        return Err(format!(
            "typingHoldSeconds must be between {} and {} (got {})",
            TYPING_HOLD_SECONDS_MIN, TYPING_HOLD_SECONDS_MAX, seconds
        ));
    }
    Ok(())
}

pub(crate) fn parse_api_server_socket_addr(bind: &str, port: u16) -> Result<SocketAddr, String> {
    let bind = bind.trim();
    if bind.is_empty() {
        return Err("apiServerBind must not be empty".to_string());
    }
    if port == 0 {
        return Err("apiServerPort must be between 1 and 65535".to_string());
    }
    let ip: IpAddr = bind.parse().map_err(|e| {
        format!("apiServerBind must be an IP address such as 127.0.0.1, 0.0.0.0, ::1, or :: ({e})")
    })?;
    Ok(SocketAddr::new(ip, port))
}

pub fn validate_api_server_settings(settings: &mut AppSettings) -> Result<(), String> {
    let bind = settings.api_server_bind.trim().to_string();
    parse_api_server_socket_addr(&bind, settings.api_server_port)?;
    settings.api_server_bind = bind;
    Ok(())
}

/// #714 Reject a screenshot hotkey that the native parser cannot accept. Syntax
/// errors block a settings save; OS-level registration conflicts do not (they are
/// surfaced as runtime status by `screenshot::register_configured_hotkey`).
pub fn validate_screenshot_hotkey(value: &str) -> Result<(), String> {
    crate::screenshot::parse_screenshot_hotkey(value)
        .map(|_| ())
        .map_err(|e| format!("Screenshot hotkey: {}", e))
}

/// #2281 Reject a sidebar compact hotkey that the settings contract cannot
/// accept. Unlike `validate_screenshot_hotkey` this is not an OS registration:
/// it only enforces the shape `Ctrl|Control + Shift + <A-Z>` minus the reserved
/// letters `W`, `R`, `C`, `V` (case-insensitive parts). Digits are rejected:
/// `Ctrl+Shift+2` reaches xterm as `key="@"`, which Keyboard.ts maps to NUL and
/// writes to the PTY. Syntax errors block a settings save; the value is judged
/// byte-identical and never repaired, so a lower-case spelling is stored as-is.
pub fn validate_sidebar_compact_hotkey(value: &str) -> Result<(), String> {
    let reject = |reason: &str| Err(format!("Sidebar compact hotkey: {reason}"));
    let parts: Vec<&str> = value.trim().split('+').map(|p| p.trim()).collect();
    if parts.len() != 3 {
        return reject("expected exactly three '+' separated parts, e.g. Ctrl+Shift+E");
    }
    if !parts[0].eq_ignore_ascii_case("ctrl") && !parts[0].eq_ignore_ascii_case("control") {
        return reject("first part must be Ctrl or Control");
    }
    if !parts[1].eq_ignore_ascii_case("shift") {
        return reject("second part must be Shift");
    }
    let mut chars = parts[2].chars();
    let key = match (chars.next(), chars.next()) {
        (Some(c), None) => c,
        _ => return reject("third part must be exactly one character"),
    };
    if !key.is_ascii_alphabetic() {
        return reject("third part must be an ASCII letter");
    }
    if matches!(key.to_ascii_lowercase(), 'w' | 'r' | 'c' | 'v') {
        return reject("third part must not be one of the reserved letters W, R, C, V");
    }
    Ok(())
}

pub fn merge_protected_coding_agent_settings(
    current: &AppSettings,
    mut incoming: AppSettings,
) -> AppSettings {
    incoming.coding_agent_profiles = current.coding_agent_profiles.clone();

    for incoming_agent in &mut incoming.agents {
        if let Some(current_agent) = current.agents.iter().find(|a| a.id == incoming_agent.id) {
            incoming_agent.envs = current_agent.envs.clone();
            incoming_agent.isolated_home = current_agent.isolated_home;
        }
    }

    incoming
}

/// #2162 - the OS suffix tokens a config-seed template folder may carry.
pub const CONFIG_SEED_OS_TOKENS: [&str; 3] = ["linux", "windows", "macos"];

/// #2162 - `Some(stem)` when `dest` ends with an OS token, where `stem` is the
/// dest it collides with. `stem` is empty when `dest` is exactly `".<token>"`.
pub fn config_seed_dest_os_token_stem(dest: &str) -> Option<String> {
    let lower = dest.trim().to_ascii_lowercase();
    CONFIG_SEED_OS_TOKENS
        .iter()
        .find(|t| lower.ends_with(&format!(".{t}")))
        .map(|t| lower[..lower.len() - t.len() - 1].to_string())
}

/// A config-seed `dest` must be a single, relative folder NAME under the replica
/// root. The convention prefixes (`default`, `default_profile_<letter>`) are
/// concatenated onto this name for the workspace-root tiers, so it must be a
/// single path segment and a legal Windows directory name.
pub fn validate_config_seed_dest(dest: &str) -> Result<(), String> {
    let d = dest.trim();
    if d.is_empty() {
        return Err("config seed destination must not be empty".to_string());
    }
    if d.contains('/') || d.contains('\\') {
        return Err(
            "config seed destination must be a single folder name (no path separators)".to_string(),
        );
    }
    if d == "." || d == ".." || d.contains("..") {
        return Err("config seed destination must not contain '..'".to_string());
    }
    if Path::new(d).is_absolute() {
        return Err("config seed destination must be relative".to_string());
    }
    // M5: ':' makes "C:foo" drive-relative (is_absolute()==false) or "x:y" an ADS.
    if d.contains(':') {
        return Err("config seed destination must not contain ':'".to_string());
    }
    if d.contains('%') || d.contains('$') {
        return Err("config seed destination must not contain placeholder markers".to_string());
    }
    // M5: reject a trailing dot (Windows strips it, causing target drift). A
    // trailing space cannot reach here: `d` was trimmed above.
    if d.ends_with('.') {
        return Err("config seed destination must not end with a dot".to_string());
    }
    // M5: reject Windows reserved device names (case-insensitive, before any extension).
    let stem = d.split('.').next().unwrap_or(d).to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!(
            "config seed destination '{}' is a reserved Windows device name",
            d
        ));
    }
    Ok(())
}

pub fn validate_resource_settings(settings: &AppSettings) -> Result<(), String> {
    // Floor at 1 (a value of 0 would block every agent launch); no upper ceiling.
    // The user sets this deliberately and is responsible for sizing it to their machine.
    if settings.max_concurrent_agent_processes == 0 {
        return Err("maxConcurrentAgentProcesses must be at least 1".to_string());
    }
    if settings.agent_group_warn_private_bytes > settings.agent_group_kill_private_bytes {
        return Err(
            "agentGroupWarnPrivateBytes must be less than or equal to agentGroupKillPrivateBytes"
                .to_string(),
        );
    }
    if settings.agent_group_kill_private_bytes == 0 {
        return Err("agentGroupKillPrivateBytes must be greater than 0".to_string());
    }
    if settings.agent_process_kill_private_bytes == 0 {
        return Err("agentProcessKillPrivateBytes must be greater than 0".to_string());
    }
    Ok(())
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

pub(crate) fn write_pre_384_v1_backup(settings_path: &Path, contents: &str) -> Result<(), String> {
    let backup_path = settings_path
        .with_file_name(crate::config::instance_artifacts::SETTINGS_MIGRATION_BACKUP_384_V1_NAME);
    if backup_path.exists() {
        return Ok(());
    }
    std::fs::write(&backup_path, contents)
        .map_err(|e| format!("Failed to write {}: {}", backup_path.display(), e))?;
    log::info!(
        "[settings-migration] wrote pre-384 v1 settings backup to {:?}",
        backup_path
    );
    Ok(())
}

/// #2058 - number of previous `settings.json` generations kept beside the live
/// file. Slot 1 is the version the most recent save replaced, slot
/// `SETTINGS_BACKUP_KEEP` the oldest kept; the next rotation drops it.
pub(crate) const SETTINGS_BACKUP_KEEP: u32 = 5;

pub(crate) fn settings_backup_path(settings_path: &Path, index: u32) -> PathBuf {
    settings_path.with_file_name(format!(
        "{SETTINGS_BACKUP_PREFIX}{index}{SETTINGS_BACKUP_SUFFIX}"
    ))
}

/// #2058 - archive `previous` (the bytes a just-completed save replaced) into
/// slot 1, shifting the older slots down and dropping slot
/// `SETTINGS_BACKUP_KEEP`.
///
/// Best effort by design: the caller's save has already succeeded on disk, so a
/// rotation failure is logged and never propagated. Runs with
/// `settings.json.lock` held by the caller, and never touches
/// `settings.pre-*.json`, `settings.local.json`, the lock file or a write
/// temporary. Deliberate local mirror of
/// `sessions_persistence::rotate_orphan_archive`.
pub(crate) fn rotate_settings_backups(settings_path: &Path, previous: &[u8]) {
    rotate_settings_backups_labeled(settings_path, settings_path, previous)
}

pub(crate) fn rotate_settings_backups_labeled(settings_path: &Path, label: &Path, previous: &[u8]) {
    use std::io::Write as _;
    // D8: a non-regular slot is a user-placed symlink or directory; never follow
    // or clobber it.
    for index in 1..=SETTINGS_BACKUP_KEEP {
        let slot = settings_backup_path(settings_path, index);
        match std::fs::symlink_metadata(&slot) {
            Ok(metadata) => {
                if !metadata.is_file() {
                    log::warn!(
                        "[settings] #2058 aborting backup rotation: slot {:?} is not a regular \
                            file",
                        settings_backup_path(label, index)
                    );
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                log::warn!(
                    "[settings] #2058 aborting backup rotation: stat of slot {:?} failed: {}",
                    settings_backup_path(label, index),
                    error
                );
                return;
            }
        }
    }

    // Shift the older slots down; the last rename drops the oldest generation.
    for index in (1..SETTINGS_BACKUP_KEEP).rev() {
        let from = settings_backup_path(settings_path, index);
        let to = settings_backup_path(settings_path, index + 1);
        match std::fs::symlink_metadata(&from) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                log::warn!(
                    "[settings] #2058 aborting backup rotation: stat of slot {:?} failed: {}",
                    settings_backup_path(label, index),
                    error
                );
                return;
            }
        }
        if let Err(error) = std::fs::rename(&from, &to) {
            log::warn!(
                "[settings] #2058 aborting backup rotation: rename {:?} -> {:?} failed: {}",
                settings_backup_path(label, index),
                settings_backup_path(label, index + 1),
                error
            );
            return;
        }
    }

    // Write the just-replaced bytes into slot 1.
    let slot_one = settings_backup_path(settings_path, 1);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    match options.open(&slot_one) {
        Ok(mut file) => {
            if let Err(error) = file.write_all(previous) {
                log::warn!(
                    "[settings] #2058 aborting backup rotation: write to slot {:?} failed: {}",
                    settings_backup_path(label, 1),
                    error
                );
                return;
            }
        }
        Err(error) => {
            log::warn!(
                "[settings] #2058 aborting backup rotation: open of slot {:?} failed: {}",
                settings_backup_path(label, 1),
                error
            );
            return;
        }
    }

    log::info!(
        "[settings] #2058 archived the replaced settings to {:?}",
        settings_backup_path(label, 1)
    );
}

/// CLI-only variant of `load_settings`. Reads disk and applies the same
/// in-memory migrations as `load_settings`, but does NOT auto-generate or
/// persist a `root_token`. Used by CLI verbs that mutate settings
/// (`open-project`, `new-project`) so error paths and pre-validation reads
/// do NOT silently rewrite `settings.json` (Round-1 G5 in #191's plan).
///
/// The CLI verbs do not consume the root_token; if a future verb needs it,
/// `settings.root_token == None` on a brand-new install is fine — the CLI
/// is read-only with respect to it. The GUI still owns root_token
/// generation via the next `load_settings()` call when it boots.
///
/// **Migration duplication is intentional for this PR.** Extracting an
/// `apply_in_memory_migrations(&mut AppSettings)` helper that both loaders
/// share is a clean follow-up, but pulls in scope outside #191 (touches
/// `load_settings`'s control flow). Keep both copies in lockstep until
/// then; if you add a new in-memory migration to `load_settings`, mirror
/// it here too.
pub fn load_settings_for_cli() -> AppSettings {
    match settings_path() {
        Some(path) => load_settings_for_cli_from_path(&path),
        None => {
            log::warn!("[cli] Could not determine home directory, using defaults");
            AppSettings::default()
        }
    }
}

/// #2716 (B3) - `load_settings_for_cli` at an explicit path, so a test can drive
/// the CLI loader over a tempdir.
pub(crate) fn load_settings_for_cli_from_path(path: &Path) -> AppSettings {
    let mut settings = match capture_project_settings(path, false) {
        Ok((settings, _, _)) => settings,
        Err(error) => unavailable_project_settings(AppSettings::default(), error),
    };

    // 0.8.0 unified-window migration — must mirror `load_settings` exactly,
    // EXCEPT for the root_token auto-gen + save_settings call.
    // #1737 (D7c): the same three destination-ownership suppressions as the GUI loader.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_GEOMETRY)
        && settings.main_geometry.is_none()
    {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
        }
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ZOOM)
        && (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ALWAYS_ON_TOP)
        && !settings.main_always_on_top
        && settings.sidebar_always_on_top
    {
        settings.main_always_on_top = true;
    }

    // #248 migration — translate in-memory only. The CLI loader is forbidden
    // from writing settings.json per the §463 contract (load_settings_for_cli
    // is the read-only variant used by mutating verbs like `open-project` and
    // `new-project`; it must not race with the GUI's settings writes). The
    // next GUI launch finalizes the migration to disk via load_settings.
    apply_issue_248_migration(&mut settings);
    adopt_orphaned_profiles(&mut settings);
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);

    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);

    // NO root_token auto-gen, NO save_settings call.
    settings
}

/// #786 R1: strict CLI loader for MUTATING paths and `list`/`show`. Identical to
/// `load_settings_for_cli` EXCEPT it distinguishes an ABSENT settings.json (fine:
/// start from default) from a PRESENT-but-unparseable one (Err: refuse to touch
/// it). This closes the silent-wipe hole: `load_settings_for_cli` returns a
/// default on a strict parse error, and a subsequent `save_settings` (which
/// preserves only `project_paths`, via a LENIENT `Value` read) would rewrite
/// settings.json to defaults, destroying every agent/profile/hotkey. NEVER call
/// the silent-default `load_settings_for_cli` for a write.
///
/// Keep the in-memory migration tail in lockstep with `load_settings_for_cli`
/// (see the note above that function).
pub fn load_settings_for_cli_strict() -> Result<AppSettings, String> {
    load_settings_for_cli_strict_from_path(settings_path().as_deref())
}

/// #2716 (B3) - `load_settings_for_cli_strict` at an explicit path, so a test can
/// drive the strict CLI loader over a tempdir.
pub(crate) fn load_settings_for_cli_strict_from_path(
    path: Option<&Path>,
) -> Result<AppSettings, String> {
    let mut settings = match path {
        None => AppSettings::default(),
        Some(path) => {
            let (settings, _, _) = capture_project_settings(path, false).map_err(|error| {
                format!(
                    "s\
                ettings source could not be parsed or selected ({error}); refusing to modify it"
                )
            })?;
            if let Some(error) = &settings.project_paths_persistence_error {
                return Err(format!(
                    "settings source could not be parsed or selected ({error}); \
                    refusing to modify it"
                ));
            }
            settings
        }
    };

    // Mirror `load_settings_for_cli`'s in-memory migrations (no disk write).
    // #1737 (D7c): the same three destination-ownership suppressions.
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_GEOMETRY)
        && settings.main_geometry.is_none()
    {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
        }
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ZOOM)
        && (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
    }
    if !settings
        .local_overlay_state
        .owns_top_level(OVERLAY_KEY_MAIN_ALWAYS_ON_TOP)
        && !settings.main_always_on_top
        && settings.sidebar_always_on_top
    {
        settings.main_always_on_top = true;
    }
    apply_issue_248_migration(&mut settings);
    adopt_orphaned_profiles(&mut settings);
    repair_coding_agent_profiles_config(&mut settings.coding_agent_profiles, &settings.agents);
    refresh_pair_id_hashes(&settings.agents, &mut settings.coding_agent_profiles);

    Ok(settings)
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
