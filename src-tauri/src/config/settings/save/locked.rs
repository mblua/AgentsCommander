use super::super::super::projects;
use super::super::{
    agents_instance_path, apply_project_decode_to_value, capture_registered_profile_gate,
    captured_registered_profile_object, coding_agent_identity_with_physical_overlay,
    ensure_project_paths_migrated_at_locked_path, legacy_profiles_shape_present,
    physical_identity_violations, physical_profile_rows, prepare_registered_profile_owner,
    production_instance_base, project_physical_profiles, read_agents_instance_file,
    recheck_registered_profile_gate, refresh_agent_id_hashes, refresh_pair_id_hashes,
    registered_profiles_for_read, repair_loaded_response_close_settings,
    repair_loaded_room_idle_confirmation_settings, replace_project_projection,
    restore_agents_groups_for_redecode, restore_codex_home_carrier,
    restrict_split_profile_mutation_rows, retain_selected_legacy_profile_aliases,
    retain_unknown_registered_agent_fields, rotate_settings_backups, save_agents_instance_file,
    select_project_source_at_locked_path, selected_registered_profile_owner,
    validate_physical_identity_delta, validate_registered_overlay_profile_edits,
    validate_split_canonical_profile_edits, write_identity_rows, write_pre_384_v1_backup,
    AppSettings, MainWindowPlacementWriteMode, ProjectWriteMode, RegisteredProfileDocument,
    StartupAgentRows, TerminalSnapshotGateWriteMode,
};
use super::{
    apply_registered_main_disk_fields, preflight_duplicate_agents,
    prepare_registered_main_agent_groups, read_disk_object_and_contents_for_write_typed,
    read_disk_object_for_write_typed, write_value_atomic, SettingsSaveError,
    SettingsSaveLegacyOutward, SettingsSaveReason, SettingsSaveStage,
};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

pub(crate) fn save_settings_value_locked_with_startup(
    settings: &AppSettings,
    path: &Path,
    mode: ProjectWriteMode,
    terminal_snapshot_gate_mode: TerminalSnapshotGateWriteMode,
    main_window_placement_mode: MainWindowPlacementWriteMode,
    startup: Option<&StartupAgentRows>,
) -> Result<AppSettings, SettingsSaveError> {
    let disk_gate_stage = match &mode {
        ProjectWriteMode::Preserve => SettingsSaveStage::PreserveDiskGate,
        #[cfg(test)]
        ProjectWriteMode::Reconcile { .. } => SettingsSaveStage::ReconcileDiskGate,
    };
    let initial_disk = read_disk_object_for_write_typed(path, disk_gate_stage)?;
    let reject_profiles = |reason: String| {
        SettingsSaveError::semantic(
            disk_gate_stage,
            path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    };
    let profile_gate = capture_registered_profile_gate(path).map_err(&reject_profiles)?;
    let physical_sidecar =
        read_agents_instance_file(&agents_instance_path(path)).map_err(&reject_profiles)?;
    // Preparation and ownership use the original bytes checked by this gate,
    // rather than independently captured generations of the two files.
    let captured_main =
        captured_registered_profile_object(&profile_gate, path).map_err(&reject_profiles)?;
    let captured_sidecar =
        captured_registered_profile_object(&profile_gate, &agents_instance_path(path))
            .map_err(&reject_profiles)?;
    if initial_disk != captured_main || physical_sidecar != captured_sidecar {
        return Err(reject_profiles(format!(
            "staleProfileSource: {}",
            path.display()
        )));
    }
    let initial_disk = captured_main;
    let physical_sidecar = captured_sidecar;
    let profiles_in_sidecar = physical_sidecar.as_ref().is_some_and(|map| {
        map.contains_key("codingAgentProfiles")
            || (map.contains_key("agents")
                && initial_disk
                    .as_ref()
                    .is_none_or(|main| !main.contains_key("codingAgentProfiles")))
    });
    let profiles_unprotected = !settings
        .local_overlay_state
        .owns_top_level("codingAgentProfiles");
    let profile_before = selected_registered_profile_owner(
        &initial_disk.clone().unwrap_or_default(),
        physical_sidecar.as_ref(),
        profiles_unprotected,
    );
    let directional_split = profiles_in_sidecar
        && physical_sidecar
            .as_ref()
            .is_some_and(|map| !map.contains_key("agents"))
        && profile_before.pointer("/codingAgentProfiles/profileBindingVersion")
            == Some(&Value::from(1));
    let consolidate_legacy = profiles_unprotected
        && profile_before
            .get("codingAgentProfiles")
            .is_some_and(|profiles| profiles.get("profileBindingVersion").is_none())
        && physical_profile_rows(&profile_before, true).is_ok();
    let profiles_in_sidecar = profiles_in_sidecar
        || (profiles_unprotected
            && profile_before
                .get("codingAgentProfiles")
                .is_none_or(|profiles| profiles.get("profileBindingVersion").is_none()));
    // Prove whole-row correspondence before any ID-keyed copying or conversion.
    let prepared_duplicate_agents = if matches!(mode, ProjectWriteMode::Preserve)
        && !settings.local_overlay_state.owns_top_level("agents")
    {
        preflight_duplicate_agents(settings, path, initial_disk.as_ref(), startup)?
    } else {
        None
    };
    let prepared_profiles = if matches!(mode, ProjectWriteMode::Preserve) {
        let (profile_incoming, owner_has_active_agents, duplicate_rows) =
            prepare_registered_profile_incoming(
                (settings, path),
                &profile_before,
                &physical_sidecar,
                &initial_disk,
                &profile_gate,
                (
                    directional_split,
                    profiles_unprotected,
                    profiles_in_sidecar,
                    consolidate_legacy,
                ),
                prepared_duplicate_agents.as_ref(),
            )
            .map_err(&reject_profiles)?;
        let mut prepared_profiles = prepare_registered_profile_owner(
            settings,
            path,
            &profile_before,
            &profile_incoming,
            profiles_in_sidecar,
            startup,
            (owner_has_active_agents && prepared_duplicate_agents.is_some()) || duplicate_rows,
        )
        .map_err(&reject_profiles)?;
        // Original main bytes are backed up below before any cleanup publish.
        if !physical_sidecar
            .as_ref()
            .is_some_and(|map| map.contains_key("codingAgentProfiles"))
            && owner_has_active_agents
        {
            if let Some(profiles) = prepared_profiles
                .as_mut()
                .and_then(|prepared| prepared.get_mut("codingAgentProfiles"))
                .and_then(Value::as_object_mut)
            {
                for key in ["letters", "matrix", "agentDefaults"] {
                    profiles.remove(key);
                }
            }
        }
        if let Some(prepared) = &prepared_profiles {
            validate_physical_identity_delta(
                profile_before.as_object().unwrap(),
                prepared.as_object().unwrap(),
                !settings.local_overlay_state.owns_top_level("agents"),
                !directional_split,
            )
            .map_err(&reject_profiles)?;
            if directional_split {
                validate_directional_prepared_identity(&profile_before, prepared)
                    .map_err(&reject_profiles)?;
            }
        }
        if directional_split || settings.local_overlay_state.owns_top_level("agents") {
            if let Some(prepared) = prepared_profiles.as_mut().and_then(Value::as_object_mut) {
                prepared.remove("agents");
            }
        }
        prepared_profiles
    } else {
        None
    };
    // Validate the main-row packet as well before any migration, backup or
    // sidecar publisher. The real publishers below retain their existing order.
    prevalidate_registered_main_packet(
        (settings, path),
        &initial_disk,
        &physical_sidecar,
        prepared_profiles.as_ref(),
        prepared_duplicate_agents.as_ref(),
        (
            &mode,
            disk_gate_stage,
            terminal_snapshot_gate_mode,
            &main_window_placement_mode,
            directional_split,
            profiles_in_sidecar,
        ),
    )?;
    recheck_registered_profile_gate(&profile_gate).map_err(&reject_profiles)?;
    ensure_project_paths_migrated_at_locked_path(path, &mut |_, _| Ok(()))?;
    let selected_projection = select_project_source_at_locked_path(path)?.projection;
    let base = production_instance_base();
    let disk_gate_stage = match &mode {
        ProjectWriteMode::Preserve => SettingsSaveStage::PreserveDiskGate,
        #[cfg(test)]
        ProjectWriteMode::Reconcile { .. } => SettingsSaveStage::ReconcileDiskGate,
    };
    let disk_read = read_disk_object_and_contents_for_write_typed(path, disk_gate_stage)?;
    backup_legacy_profiles_before_write(
        path,
        disk_gate_stage,
        &mode,
        disk_read.as_ref().map(|(map, _)| map),
        disk_read.as_ref().map(|(_, contents)| contents.as_str()),
    )?;
    // #2058: the exact bytes this save is about to replace, captured before the
    // tuple is reduced to its object half.
    let previous_contents: Option<String> = disk_read.as_ref().map(|(_, c)| c.clone());
    let disk = disk_read.map(|(map, _)| map);
    let serialize_object = || -> Result<Map<String, Value>, SettingsSaveError> {
        match serde_json::to_value(settings).map_err(|source| {
            let outward = format!("Failed to serialize settings: {source}");
            SettingsSaveError::json(
                SettingsSaveStage::Serialize,
                path,
                None,
                source,
                SettingsSaveLegacyOutward::Serialize(outward),
            )
        })? {
            Value::Object(m) => Ok(m),
            _ => Err(SettingsSaveError::semantic(
                SettingsSaveStage::Serialize,
                path,
                None,
                SettingsSaveReason::SerializedJsonNotObject,
                SettingsSaveLegacyOutward::SerializedJsonNotObject(
                    "settings did not serialize to a JSON object".to_string(),
                ),
            )),
        }
    };

    // Base object: Preserve starts from the incoming settings (non-project edits
    // apply); Reconcile is project-only and starts from the fresh disk object
    // (or the live settings when the file is absent).
    let mut out: Map<String, Value> = match mode {
        ProjectWriteMode::Preserve => serialize_object()?,
        #[cfg(test)]
        ProjectWriteMode::Reconcile { .. } => match &disk {
            Some(m) => m.clone(),
            None => serialize_object()?,
        },
    };

    // E2432: only the dedicated preference publisher changes menu choices.
    out.remove("agentFileSourceByMenu");
    if let Some(preferences) = disk
        .as_ref()
        .and_then(|disk| disk.get("agentFileSourceByMenu"))
    {
        out.insert("agentFileSourceByMenu".into(), preferences.clone());
    }
    apply_registered_main_disk_fields(
        &mut out,
        disk.as_ref(),
        path,
        disk_gate_stage,
        terminal_snapshot_gate_mode,
        main_window_placement_mode,
    )?;

    // #1737: the effective view the caller must adopt for overlay-owned paths. `out`
    // is not a usable source: in Reconcile mode it was seeded from the disk object,
    // which already holds base values. Skipped entirely when no overlay is in force,
    // which is what keeps the no-overlay write path byte-identical.
    let effective: Option<Map<String, Value>> = if settings.local_overlay_state.is_empty() {
        None
    } else {
        Some(serialize_object()?)
    };
    // #1737: last writer over `out`, so no earlier stage can leak an overlay value
    // into the base file. Runs in both project write modes: Reconcile seeds `out`
    // from the live settings when the file is absent. The overlay-ineligible sets
    // (D7a, D7b) already exclude every key the #1077 and #1173 stages touch.
    if matches!(mode, ProjectWriteMode::Preserve) {
        write_identity_rows(&mut out, &settings.local_overlay_state);
    }
    settings.local_overlay_state.restore_base(&mut out);

    // #2716 (B3): the agents file is written first; only the keys it now holds
    // leave `out`, in both modes. A failure returns before the settings write.
    let agents_groups = save_agents_instance_file(
        settings,
        path,
        &mode,
        #[cfg(test)]
        disk.as_ref(),
        &out,
        disk_gate_stage,
        (
            prepared_duplicate_agents.as_ref(),
            prepared_profiles.as_ref().filter(|_| profiles_in_sidecar),
            Some(profiles_in_sidecar),
        ),
    )?;
    out = prepare_registered_main_agent_groups(
        out,
        settings,
        disk.as_ref(),
        &agents_groups,
        profiles_in_sidecar,
        (
            prepared_profiles.as_ref(),
            prepared_duplicate_agents.as_ref(),
            physical_sidecar.as_ref(),
            path,
        ),
        &mode,
    )
    .map_err(|reason| {
        SettingsSaveError::semantic(
            disk_gate_stage,
            path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    })?;
    let value = Value::Object(out);
    let written_bytes = write_value_atomic(&value, path)?;
    if let Some(previous) = previous_contents {
        if previous.as_bytes() != written_bytes.as_slice() {
            rotate_settings_backups(path, previous.as_bytes());
        }
    }

    let mut written_value = value;
    replace_project_projection(written_value.as_object_mut().unwrap(), &selected_projection);
    // #2716 (B3): the re-decode sees what a load would, the agents file's groups
    // in the base object, under the overlay reapplied below.
    let physical_groups = agents_groups.keys().cloned().collect::<Vec<_>>();
    let profile_owner = selected_registered_profile_owner(
        written_value.as_object().expect("written settings object"),
        Some(&agents_groups),
        !settings
            .local_overlay_state
            .owns_top_level("codingAgentProfiles"),
    );
    let profile_owner_view = registered_profiles_for_read(&profile_owner);
    let base_profiles = profile_owner.get("codingAgentProfiles").cloned();
    restore_agents_groups_for_redecode(&mut written_value, agents_groups);
    let identity_physical =
        coding_agent_identity_with_physical_overlay(written_value.clone(), path, &physical_groups);
    if let Some(profiles) = profile_owner_view.get("codingAgentProfiles") {
        written_value["codingAgentProfiles"] = profiles.clone();
    }
    // #1737: disk holds the base, memory holds the effective value. Without this the
    // caller would adopt base values for every overlay-owned key on the first save.
    if let (Some(effective), Value::Object(object)) = (&effective, &mut written_value) {
        settings.local_overlay_state.reapply_from(effective, object);
    }
    restore_codex_home_carrier(
        written_value.as_object_mut().unwrap(),
        base_profiles.as_ref(),
    );
    let fresh_state = apply_project_decode_to_value(
        &mut written_value,
        base.as_deref(),
        &projects::FsCandidateResolver,
    );
    let mut written: AppSettings = serde_json::from_value(written_value).map_err(|source| {
        let outward = format!("Failed to re-decode written settings: {source}");
        SettingsSaveError::json(
            SettingsSaveStage::ReDecode,
            path,
            None,
            source,
            SettingsSaveLegacyOutward::ReDecode(outward),
        )
    })?;
    refresh_agent_id_hashes(&mut written.agents);
    refresh_pair_id_hashes(&written.agents, &mut written.coding_agent_profiles);
    written.project_path_state = Arc::new(fresh_state);
    written.coding_agent_identity_physical = Some(Arc::new(identity_physical));
    written.local_overlay_state = settings.local_overlay_state.clone();
    repair_loaded_response_close_settings(&mut written, &path.display().to_string());
    repair_loaded_room_idle_confirmation_settings(&mut written, &path.display().to_string());
    Ok(written)
}

pub(crate) fn prevalidate_registered_main_packet(
    owner: (&AppSettings, &Path),
    initial_disk: &Option<Map<String, Value>>,
    physical_sidecar: &Option<Map<String, Value>>,
    prepared_profiles: Option<&Value>,
    prepared_duplicate_agents: Option<&Value>,
    controls: (
        &ProjectWriteMode,
        SettingsSaveStage,
        TerminalSnapshotGateWriteMode,
        &MainWindowPlacementWriteMode,
        bool,
        bool,
    ),
) -> Result<(), SettingsSaveError> {
    let (settings, path) = owner;
    let (
        mode,
        disk_gate_stage,
        terminal_snapshot_gate_mode,
        main_window_placement_mode,
        directional_split,
        profiles_in_sidecar,
    ) = controls;
    let reject_profiles = |reason: String| {
        SettingsSaveError::semantic(
            disk_gate_stage,
            path,
            None,
            SettingsSaveReason::DiskSettingsValidationRejected,
            SettingsSaveLegacyOutward::DiskValidation(reason),
        )
    };
    if prepared_profiles.is_some()
        && (directional_split || settings.local_overlay_state.owns_top_level("agents"))
    {
        let mut main_packet = serde_json::to_value(settings)
            .map_err(|error| reject_profiles(error.to_string()))?
            .as_object()
            .ok_or_else(|| reject_profiles("invalidSettingsObject".into()))?
            .clone();
        apply_registered_main_disk_fields(
            &mut main_packet,
            initial_disk.as_ref(),
            path,
            disk_gate_stage,
            terminal_snapshot_gate_mode,
            main_window_placement_mode.clone(),
        )?;
        settings.local_overlay_state.restore_base(&mut main_packet);
        let mut sidecar_packet = physical_sidecar.clone().unwrap_or_default();
        if let Some(profiles) =
            prepared_profiles.and_then(|prepared| prepared.get("codingAgentProfiles"))
        {
            sidecar_packet.insert("codingAgentProfiles".into(), profiles.clone());
        }
        let main_packet = prepare_registered_main_agent_groups(
            main_packet,
            settings,
            initial_disk.as_ref(),
            &sidecar_packet,
            profiles_in_sidecar,
            (
                prepared_profiles,
                prepared_duplicate_agents,
                physical_sidecar.as_ref(),
                path,
            ),
            mode,
        )
        .map_err(&reject_profiles)?;
        if !settings.local_overlay_state.owns_top_level("agents") {
            validate_physical_identity_delta(
                &initial_disk.clone().unwrap_or_default(),
                &main_packet,
                true,
                false,
            )
            .map_err(&reject_profiles)?;
        }
    }
    Ok(())
}

pub(crate) fn backup_legacy_profiles_before_write(
    path: &Path,
    disk_gate_stage: SettingsSaveStage,
    mode: &ProjectWriteMode,
    map: Option<&Map<String, Value>>,
    contents: Option<&str>,
) -> Result<(), SettingsSaveError> {
    // #2015: Preserve builds from the struct, which ignores legacy pre-v2
    // profile fields; keep the original once before any save drops them.
    if matches!(mode, ProjectWriteMode::Preserve) {
        if let (Some(map), Some(contents)) = (map, contents) {
            if legacy_profiles_shape_present(map) {
                write_pre_384_v1_backup(path, contents).map_err(|e| {
                    SettingsSaveError::io(
                        disk_gate_stage,
                        path,
                        None,
                        std::io::Error::other(e.clone()),
                        SettingsSaveLegacyOutward::DiskRead(format!(
                            "Refusing to overwrite {}: legacy settings backup failed ({e})",
                            path.display()
                        )),
                    )
                })?;
            }
        }
    }
    Ok(())
}

pub(crate) fn prepare_registered_profile_incoming(
    owner: (&AppSettings, &Path),
    profile_before: &Value,
    physical_sidecar: &Option<Map<String, Value>>,
    initial_disk: &Option<Map<String, Value>>,
    profile_gate: &[RegisteredProfileDocument],
    flags: (bool, bool, bool, bool),
    prepared_duplicate_agents: Option<&Value>,
) -> Result<(Value, bool, bool), String> {
    let (settings, path) = owner;
    let (directional_split, profiles_unprotected, profiles_in_sidecar, consolidate_legacy) = flags;
    let mut profile_incoming = serde_json::to_value(settings).map_err(|e| e.to_string())?;
    validate_registered_overlay_profile_edits(
        settings,
        profile_before,
        physical_sidecar,
        initial_disk,
        path,
        profile_gate,
    )?;
    settings
        .local_overlay_state
        .restore_base(profile_incoming.as_object_mut().unwrap());
    // The directional view and initial preparation use selected physical
    // rows. Their borrowed rows are stripped before sidecar publication.
    let owner_has_active_agents = if directional_split
        || (profiles_unprotected
            && profile_before.pointer("/codingAgentProfiles/profileBindingVersion")
                != Some(&Value::from(1)))
    {
        true
    } else if profiles_in_sidecar {
        physical_sidecar
            .as_ref()
            .is_some_and(|map| map.contains_key("agents"))
    } else {
        !physical_sidecar
            .as_ref()
            .is_some_and(|map| map.contains_key("agents"))
    };
    if !owner_has_active_agents {
        if profile_before.pointer("/codingAgentProfiles/profileBindingVersion")
            == Some(&Value::from(1))
        {
            let selected_rows = physical_sidecar
                .as_ref()
                .and_then(|map| map.get("agents"))
                .or_else(|| initial_disk.as_ref().and_then(|map| map.get("agents")));
            validate_split_canonical_profile_edits(settings, profile_before, selected_rows)?;
        }
        match profile_before.get("agents") {
            Some(rows) => {
                profile_incoming["agents"] = rows.clone();
            }
            None => {
                profile_incoming.as_object_mut().unwrap().remove("agents");
            }
        }
    }
    if settings.local_overlay_state.owns_top_level("agents") {
        match profile_before.get("agents") {
            Some(rows) => profile_incoming["agents"] = rows.clone(),
            None => {
                profile_incoming.as_object_mut().unwrap().remove("agents");
            }
        }
    }
    if consolidate_legacy {
        retain_selected_legacy_profile_aliases(
            profile_before,
            profile_before,
            &mut profile_incoming,
        )?;
    }
    if owner_has_active_agents {
        if let Some(rows) = prepared_duplicate_agents {
            profile_incoming["agents"] = rows.clone();
        } else {
            retain_unknown_registered_agent_fields(profile_before, &mut profile_incoming)?;
        }
    }
    let mut incoming_ids = HashSet::new();
    let duplicate_rows = profile_incoming
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("id").and_then(Value::as_str))
        .any(|id| !incoming_ids.insert(id));
    if directional_split {
        restrict_split_profile_mutation_rows(settings, profile_before, &mut profile_incoming)?;
    }
    Ok((profile_incoming, owner_has_active_agents, duplicate_rows))
}

pub(crate) fn validate_directional_prepared_identity(
    profile_before: &Value,
    prepared: &Value,
) -> Result<(), String> {
    // A BASE fork carries its old complete set, including historical
    // duplicate profiles. Compare profile edits under the proposed
    // row grouping; configuration admission above stays independent.
    let mut previous = project_physical_profiles(profile_before, true)?;
    let old_rows = physical_profile_rows(profile_before, true)?;
    let mut target_baseline = profile_before.clone();
    target_baseline["agents"] = prepared["agents"].clone();
    let target_baseline = project_physical_profiles(&target_baseline, true)?;
    for id in physical_profile_rows(prepared, true)?.keys() {
        if !old_rows.contains_key(id) {
            for key in ["profilesByAgent", "profileLabelsByAgent"] {
                previous["codingAgentProfiles"][key][id] =
                    target_baseline["codingAgentProfiles"][key][id].clone();
            }
        }
    }
    previous["agents"] = prepared["agents"].clone();
    let proposed = project_physical_profiles(prepared, true)?;
    let old = physical_identity_violations(previous.as_object().unwrap(), false, true)?;
    let new = physical_identity_violations(proposed.as_object().unwrap(), false, true)?;
    for (signature, (count, error)) in new {
        if count > old.get(&signature).map_or(0, |(count, _)| *count) {
            return Err(error.to_string());
        }
    }
    Ok(())
}
