use super::super::super::{
    configuration_identity, configuration_identity_debug, raw_entries, refresh_agent_id_hashes,
    refresh_pair_id_hashes, AppSettings, CodingAgentProfilesConfig, OVERLAY_DERIVED_ID_CLOSURES,
    OVERLAY_INELIGIBLE_DISK_KEYS, OVERLAY_INELIGIBLE_LEGACY_KEYS,
};
use super::super::{
    canonical_profile_source, comparable_agent, empty_profile_cell, finalize_agent_order,
    normalized_profile_cells, physical_profile_rows, physical_profile_sets,
    project_physical_profiles, registered_profiles_for_read, repair_coding_agent_profiles_config,
    retain_configuration_commands_with_startup, AgentConfig, ProfileCellConfig, ProfileSlotConfig,
    RegisteredProfileDocument, StartupAgentRows, CONFIGURATION_COMMAND_PAIRS,
};
use super::{agents_instance_path, AGENTS_INSTANCE_KEYS};
use crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME;
use crate::config::local_overlay::LocalSettingsOverlay;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// #2716 (B3) - puts the agents file's groups back into the just-written object
/// for the save's re-decode. Like the load merge, `[]` stands in when neither file
/// holds `agents`.
pub(crate) fn restore_agents_groups_for_redecode(written: &mut Value, groups: Map<String, Value>) {
    if let Value::Object(object) = written {
        object.extend(groups);
        object
            .entry(AGENTS_INSTANCE_KEYS[0])
            .or_insert_with(|| Value::Array(Vec::new()));
    }
}

/// Restore only the backend-owned carrier; ordinary profile edits still apply.
/// Absence in the physical base removes a forged client/overlay carrier.
pub(crate) fn restore_codex_home_carrier(
    out: &mut Map<String, Value>,
    base_profiles: Option<&Value>,
) {
    const KEY: &str = "codexHomeKeyByAgent";
    let carrier = base_profiles
        .and_then(Value::as_object)
        .and_then(|profiles| profiles.get(KEY));
    if carrier.is_some() {
        out.entry("codingAgentProfiles")
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if let Some(profiles) = out
        .get_mut("codingAgentProfiles")
        .and_then(Value::as_object_mut)
    {
        match carrier {
            Some(value) => {
                profiles.insert(KEY.to_string(), value.clone());
            }
            None => {
                profiles.remove(KEY);
            }
        }
    }
}

// A missing physical identity image is not an absence proof. This local
// reference is admitted only by the locked operation's original raw capture.
// It is never installed as identity state or used as a publication candidate.
pub(crate) fn absent_main_overlay_comparison(
    settings: &AppSettings,
    path: &Path,
    physical_sidecar: &Option<Map<String, Value>>,
    initial_disk: &Option<Map<String, Value>>,
    captured: &[RegisteredProfileDocument],
) -> Result<Value, String> {
    let unavailable = || "overlayProfileComparisonUnavailable".to_owned();
    if initial_disk.is_some() || settings.local_overlay_state.rejection().is_some() {
        return Err(unavailable());
    }
    let main = captured
        .iter()
        .find(|(document, _)| document == path)
        .ok_or_else(unavailable)?;
    if main.1.is_some() {
        return Err(unavailable());
    }
    let sidecar_path = agents_instance_path(path);
    let sidecar = captured
        .iter()
        .find(|(document, _)| document == &sidecar_path)
        .ok_or_else(unavailable)?;
    let captured_sidecar: Option<Map<String, Value>> = sidecar
        .1
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|_| unavailable())?;
    if &captured_sidecar != physical_sidecar {
        return Err(unavailable());
    }
    let overlay_path = path.with_file_name(SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
    let bytes = captured
        .iter()
        .find(|(document, _)| document == &overlay_path)
        .and_then(|(_, bytes)| bytes.as_deref())
        .ok_or_else(unavailable)?;
    let mut reference = serde_json::to_value(AppSettings::default()).map_err(|_| unavailable())?;
    let overlay = LocalSettingsOverlay::from_overlay_bytes_paired(
        &mut reference,
        bytes,
        OVERLAY_INELIGIBLE_DISK_KEYS,
        OVERLAY_INELIGIBLE_LEGACY_KEYS,
        OVERLAY_DERIVED_ID_CLOSURES,
        CONFIGURATION_COMMAND_PAIRS,
    );
    if overlay.rejection().is_some()
        || overlay.is_empty()
        || overlay.owned_paths() != settings.local_overlay_state.owned_paths()
        || overlay.dropped_keys() != settings.local_overlay_state.dropped_keys()
    {
        return Err(unavailable());
    }
    restore_codex_home_carrier(
        reference.as_object_mut().ok_or_else(unavailable)?,
        captured_sidecar
            .as_ref()
            .and_then(|sidecar| sidecar.get("codingAgentProfiles")),
    );
    serde_json::from_value::<AppSettings>(reference.clone()).map_err(|_| unavailable())?;
    Ok(reference)
}

pub(crate) fn validate_registered_overlay_profile_edits(
    settings: &AppSettings,
    profile_before: &Value,
    physical_sidecar: &Option<Map<String, Value>>,
    initial_disk: &Option<Map<String, Value>>,
    path: &Path,
    captured: &[RegisteredProfileDocument],
) -> Result<(), String> {
    let paths = settings.local_overlay_state.owned_paths();
    if !paths.iter().any(|path| {
        path.first()
            .is_some_and(|key| AGENTS_INSTANCE_KEYS.contains(&key.as_str()))
    }) {
        return Ok(());
    }
    let absence_reference = if settings.coding_agent_identity_physical.is_none() {
        Some(absent_main_overlay_comparison(
            settings,
            path,
            physical_sidecar,
            initial_disk,
            captured,
        )?)
    } else {
        None
    };
    let raw = settings
        .coding_agent_identity_physical
        .as_ref()
        .and_then(|raw| raw.as_object())
        .or_else(|| absence_reference.as_ref().and_then(Value::as_object))
        .ok_or("overlayProfileComparisonUnavailable")?;
    let mut previous = registered_profiles_for_read(profile_before);
    if let Some(rows) = physical_sidecar
        .as_ref()
        .and_then(|map| map.get("agents"))
        .or_else(|| initial_disk.as_ref().and_then(|map| map.get("agents")))
    {
        previous["agents"] = rows.clone();
    }
    settings
        .local_overlay_state
        .reapply_from(raw, previous.as_object_mut().ok_or("invalidSource")?);
    // These copies are comparison data only, never physical publication inputs.
    let normalize = |mut value: Value| -> Result<Value, String> {
        restore_codex_home_carrier(
            value.as_object_mut().ok_or("invalidSource")?,
            profile_before.get("codingAgentProfiles"),
        );
        let mut agents: Vec<AgentConfig> = value
            .get("agents")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        let mut profiles: CodingAgentProfilesConfig = value
            .get("codingAgentProfiles")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        finalize_agent_order(&mut agents);
        // Match only implicit empty-A repair; do not prune submitted values.
        profiles
            .profile_slots
            .entry("A".into())
            .or_insert_with(|| ProfileSlotConfig {
                label: String::new(),
            });
        for agent in &agents {
            profiles
                .profiles_by_agent
                .entry(agent.id.clone())
                .or_default()
                .entry("A".into())
                .or_insert_with(empty_profile_cell);
        }
        refresh_agent_id_hashes(&mut agents);
        refresh_pair_id_hashes(&agents, &mut profiles);
        for cell in profiles
            .profiles_by_agent
            .values_mut()
            .flat_map(|cells| cells.values_mut())
        {
            cell.id_hash = None;
            cell.id_hash_debug = None;
        }
        profiles.identity_by_agent.clear();
        let agents = agents
            .iter()
            .map(comparable_agent)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(serde_json::json!({"agents":agents,"codingAgentProfiles":profiles}))
    };
    let previous = normalize(previous)?;
    let incoming = normalize(serde_json::json!({
        "agents":&settings.agents,
        "codingAgentProfiles":&settings.coding_agent_profiles
    }))?;
    for path in paths {
        let Some(key) = path
            .first()
            .filter(|key| AGENTS_INSTANCE_KEYS.contains(&key.as_str()))
        else {
            continue;
        };
        let previous_value = path
            .iter()
            .try_fold(&previous, |value, segment| value.get(segment));
        let incoming_value = path
            .iter()
            .try_fold(&incoming, |value, segment| value.get(segment));
        if previous_value != incoming_value {
            return Err(format!("overlayOwnedProfileGroup: {key}"));
        }
    }
    Ok(())
}

pub(crate) fn retain_unknown_registered_agent_fields(
    profile_before: &Value,
    profile_incoming: &mut Value,
) -> Result<(), String> {
    // The raw physical row retains fields unknown to AgentConfig.
    let prior_rows = raw_entries(profile_before, "id");
    if let Some(rows) = profile_incoming
        .get_mut("agents")
        .and_then(Value::as_array_mut)
    {
        for row in rows {
            let Some(id) = row.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(prior) = prior_rows.get(id).and_then(Value::as_object) else {
                continue;
            };
            let typed: AgentConfig =
                serde_json::from_value(Value::Object(prior.clone())).map_err(|e| e.to_string())?;
            let known = serde_json::to_value(typed).map_err(|e| e.to_string())?;
            if let Some(object) = row.as_object_mut() {
                for (key, value) in prior {
                    if !known.as_object().unwrap().contains_key(key) && key != "command" {
                        object.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
            }
        }
    }
    Ok(())
}

// A legacy split may have projected no cells (or shadowed rows) on read. Keep
// the selected raw payload for untouched aliases; only actual draft edits win.
pub(crate) fn retain_selected_legacy_profile_aliases(
    selected: &Value,
    previous_owner: &Value,
    incoming: &mut Value,
) -> Result<(), String> {
    let previous = registered_profiles_for_read(previous_owner);
    let previous: CodingAgentProfilesConfig = previous
        .get("codingAgentProfiles")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let previous = serde_json::to_value(previous).map_err(|error| error.to_string())?;
    let projected = project_physical_profiles(selected, true)?;
    for id in physical_profile_rows(selected, true)?.keys() {
        let old_cells = previous.get("profilesByAgent").and_then(|map| map.get(id));
        let cells = incoming
            .pointer("/codingAgentProfiles/profilesByAgent")
            .and_then(|map| map.get(id));
        if normalized_profile_cells(old_cells)? == normalized_profile_cells(cells)? {
            incoming["codingAgentProfiles"]["profilesByAgent"][id] =
                projected["codingAgentProfiles"]["profilesByAgent"][id].clone();
        }
        let empty = serde_json::json!({});
        let old_labels = previous
            .get("profileLabelsByAgent")
            .and_then(|map| map.get(id))
            .unwrap_or(&empty);
        let labels = incoming
            .pointer("/codingAgentProfiles/profileLabelsByAgent")
            .and_then(|map| map.get(id))
            .unwrap_or(&empty);
        if old_labels == labels {
            incoming["codingAgentProfiles"]["profileLabelsByAgent"][id] =
                projected["codingAgentProfiles"]["profileLabelsByAgent"][id].clone();
        }
    }
    Ok(())
}

pub(crate) fn comparable_registered_profile_draft(
    mut profiles: CodingAgentProfilesConfig,
    agents: &[AgentConfig],
) -> CodingAgentProfilesConfig {
    repair_coding_agent_profiles_config(&mut profiles, agents);
    profiles.identity_by_agent.clear();
    for cell in profiles
        .profiles_by_agent
        .values_mut()
        .flat_map(|cells| cells.values_mut())
    {
        cell.id_hash = None;
        cell.id_hash_debug = None;
    }
    profiles
}

pub(crate) fn validate_split_canonical_profile_edits(
    settings: &AppSettings,
    original_owner: &Value,
    selected_rows: Option<&Value>,
) -> Result<(), String> {
    let previous = registered_profiles_for_read(original_owner);
    let previous = previous
        .get("codingAgentProfiles")
        .cloned()
        .map(serde_json::from_value::<CodingAgentProfilesConfig>)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    if comparable_registered_profile_draft(previous, &settings.agents)
        != comparable_registered_profile_draft(
            settings.coding_agent_profiles.clone(),
            &settings.agents,
        )
    {
        return Err("crossOwnerCanonicalProfileEdit".into());
    }
    let old_rows = physical_profile_rows(
        &serde_json::json!({
            "agents": selected_rows.cloned().unwrap_or_else(|| serde_json::json!([]))
        }),
        true,
    )?;
    if settings.agents.iter().any(|agent| {
        old_rows
            .get(&agent.id)
            .is_some_and(|hash| hash != &configuration_identity(&agent.command, &agent.envs))
    }) {
        return Err("crossOwnerCanonicalProfileEdit".into());
    }
    Ok(())
}

// Compare the full ordinary baseline before restricting mutation rows. A save
// of unrelated preferences must not bind an externally changed physical BASE.
pub(crate) fn restrict_split_profile_mutation_rows(
    settings: &AppSettings,
    before: &Value,
    incoming: &mut Value,
) -> Result<(), String> {
    let sets = physical_profile_sets(before, true)?;
    let previous = project_physical_profiles(before, true)?;
    let rows = physical_profile_rows(incoming, true)?;
    let empty = serde_json::json!({});
    let mut admitted = HashSet::new();
    for (id, hash) in rows {
        let old_hash = sets.rows.get(&id);
        // Both sides of intent comparison use the existing typed draft shape;
        // canonical_profile_source retains the complete raw unknown fields.
        let typed_cells = |source: &Value| -> Result<Value, String> {
            let cells = normalized_profile_cells(
                source
                    .pointer("/codingAgentProfiles/profilesByAgent")
                    .and_then(|map| map.get(&id)),
            )?;
            let cells: BTreeMap<String, ProfileCellConfig> =
                serde_json::from_value(cells).map_err(|error| error.to_string())?;
            normalized_profile_cells(Some(
                &serde_json::to_value(cells).map_err(|error| error.to_string())?,
            ))
        };
        let labels = incoming
            .pointer("/codingAgentProfiles/profileLabelsByAgent")
            .and_then(|map| map.get(&id))
            .unwrap_or(&empty);
        let old_labels = previous
            .pointer("/codingAgentProfiles/profileLabelsByAgent")
            .and_then(|map| map.get(&id))
            .unwrap_or(&empty);
        let profile_edit =
            typed_cells(incoming)? != typed_cells(&previous)? || labels != old_labels;
        let base_edit = old_hash != Some(&hash);
        let stored = old_hash.is_some_and(|hash| sets.cells.contains_key(hash));
        if settings.local_overlay_state.owns_top_level("agents") && !stored && profile_edit {
            return Err("crossOwnerCanonicalProfileEdit: protected missing binding".into());
        }
        if stored || base_edit || profile_edit {
            admitted.insert(id.clone());
            if old_hash.is_none() && !profile_edit {
                if let Some(cells) = sets.cells.get(&hash) {
                    incoming["codingAgentProfiles"]["profilesByAgent"][&id] = cells.clone();
                    incoming["codingAgentProfiles"]["profileLabelsByAgent"][&id] = sets
                        .labels
                        .get(&hash)
                        .cloned()
                        .unwrap_or_else(|| empty.clone());
                }
            }
        }
    }
    if let Some(rows) = incoming.get_mut("agents").and_then(Value::as_array_mut) {
        rows.retain(|row| {
            row.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| admitted.contains(id))
        });
    }
    Ok(())
}

pub(crate) fn prepare_registered_profile_owner(
    settings: &AppSettings,
    path: &Path,
    profile_before: &Value,
    profile_incoming: &Value,
    profiles_in_sidecar: bool,
    startup: Option<&StartupAgentRows>,
    duplicate_rows: bool,
) -> Result<Option<Value>, String> {
    let prepared_profiles = if settings
        .local_overlay_state
        .owns_top_level("codingAgentProfiles")
    {
        None
    } else {
        let owner_path = if profiles_in_sidecar {
            agents_instance_path(path)
        } else {
            path.to_path_buf()
        };
        let prepared = if duplicate_rows || physical_profile_rows(profile_before, true).is_err() {
            // Preserve the pre-existing lenient row view without converting an
            // ambiguous ID map. Profile edits still need an unambiguous BASE.
            let previous_view = registered_profiles_for_read(profile_before);
            let previous: CodingAgentProfilesConfig = previous_view
                .get("codingAgentProfiles")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            if comparable_registered_profile_draft(previous, &settings.agents)
                != comparable_registered_profile_draft(
                    settings.coding_agent_profiles.clone(),
                    &settings.agents,
                )
            {
                return Err("ambiguousProfileConversion".into());
            }
            let mut retained = profile_incoming.clone();
            retain_configuration_commands_with_startup(
                &mut retained,
                profile_before,
                "id",
                false,
                true,
                false,
                startup,
            )?;
            match profile_before.get("codingAgentProfiles") {
                Some(profiles) => retained["codingAgentProfiles"] = profiles.clone(),
                None => {
                    retained
                        .as_object_mut()
                        .unwrap()
                        .remove("codingAgentProfiles");
                }
            }
            retained
        } else if profile_before.get("agents").is_none()
            && profile_incoming.get("agents").is_none()
            && profile_before.pointer("/codingAgentProfiles/profileBindingVersion")
                == Some(&Value::from(1))
            && !["profilesByAgent", "profileLabelsByAgent", "identityByAgent"]
                .iter()
                .any(|key| {
                    profile_before
                        .get("codingAgentProfiles")
                        .is_some_and(|profiles| profiles.get(*key).is_some())
                })
        {
            // Canonical split owners do not acquire a new physical row donor
            // or rewrite their packet for an unrelated preference save.
            profile_before.clone()
        } else {
            let mut prepared = canonical_profile_source(profile_before, profile_incoming, true)
                .map_err(|reason| format!("{}: {reason}", owner_path.display()))?;
            // Typed slot edits own `label`; opaque siblings belong to the raw
            // owner, including obsolete `name` and extension values.
            if let Some(slots) = prepared
                .pointer_mut("/codingAgentProfiles/profileSlots")
                .and_then(Value::as_object_mut)
            {
                for (letter, slot) in slots {
                    if let (Some(slot), Some(raw)) = (
                        slot.as_object_mut(),
                        profile_before
                            .pointer("/codingAgentProfiles/profileSlots")
                            .and_then(|slots| slots.get(letter))
                            .and_then(Value::as_object),
                    ) {
                        for (key, value) in raw {
                            if key != "label" {
                                slot.insert(key.clone(), value.clone());
                            }
                        }
                    }
                }
            }
            retain_configuration_commands_with_startup(
                &mut prepared,
                profile_before,
                "id",
                false,
                true,
                false,
                startup,
            )?;
            prepared
        };
        Some(prepared)
    };
    Ok(prepared_profiles)
}

pub(crate) fn refresh_registered_agent_row_metadata(
    target: &mut Map<String, Value>,
) -> Result<(), String> {
    if let Some(rows) = target.get_mut("agents") {
        let agents: Vec<AgentConfig> =
            serde_json::from_value(rows.clone()).map_err(|error| error.to_string())?;
        for (raw, agent) in rows.as_array_mut().into_iter().flatten().zip(agents) {
            if let Some(object) = raw.as_object_mut() {
                object.insert(
                    "id_hash".to_string(),
                    Value::String(configuration_identity(&agent.command, &agent.envs)),
                );
                object.insert(
                    "id_hash_debug".into(),
                    serde_json::json!(configuration_identity_debug(&agent.command, &agent.envs)),
                );
            }
        }
    }
    Ok(())
}
