use super::super::super::{
    apply_project_decode_to_value, production_instance_base, read_disk_object_for_write_typed,
    AppSettings, CodingAgentProfilesConfig, SettingsSaveError, SettingsSaveStage,
    OVERLAY_DERIVED_ID_CLOSURES, OVERLAY_INELIGIBLE_DISK_KEYS, OVERLAY_INELIGIBLE_LEGACY_KEYS,
};
use super::super::{
    agents_instance_groups, agents_instance_path, read_agents_instance_file,
    restore_codex_home_carrier, set_agents_layer_state, AgentConfig, AgentsLayerState,
    CONFIGURATION_COMMAND_PAIRS,
};
use super::empty_profile_cell;
use crate::config::local_overlay::LocalSettingsOverlay;
use crate::config::settings::project_path as projects;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;

/// #2450: stable, local-only identity of an agent, used to re-bind orphaned
/// profiles. NOT the per-cell digest: a cell's own hash changes whenever that cell
/// is edited, so keying a cell's home by it would move the home on every edit.
/// The case-folded label is mixed in so two agents sharing a command and env stay
/// distinct. Never leaves the machine.
pub fn agent_profile_identity(agent: &AgentConfig) -> String {
    use sha2::{Digest, Sha256};
    let env: BTreeMap<String, String> = agent
        .envs
        .iter()
        .filter(|row| row.enabled)
        .map(|row| (row.key.clone(), row.value.clone()))
        .collect();
    let mut buf = crate::config::agent_command::profile_hash_preimage(&agent.command, &env);
    buf.push_str("label\u{0}");
    buf.push_str(&agent.label.to_ascii_lowercase());
    buf.push('\u{0}');
    let digest = format!("{:x}", Sha256::digest(buf.as_bytes()));
    digest[..16].to_string()
}

pub(crate) fn is_identity_shape(identity: &str) -> bool {
    identity.len() == 16 && identity.bytes().all(|b| b.is_ascii_hexdigit())
}

/// #2450: an agent's profile entry holds nothing real: absent, empty, or exactly
/// the empty `A` that repair creates, with no label overrides.
pub(crate) fn is_profile_gap(profiles: &CodingAgentProfilesConfig, id: &str) -> bool {
    let no_labels = profiles
        .profile_labels_by_agent
        .get(id)
        .is_none_or(|labels| labels.is_empty());
    let no_cells = match profiles.profiles_by_agent.get(id) {
        None => true,
        Some(cells) => {
            cells.is_empty()
                || (cells.len() == 1
                    && cells.get("A").is_some_and(|cell| {
                        let mut content = cell.clone();
                        content.id_hash = None;
                        content.id_hash_debug = None;
                        content == empty_profile_cell()
                    }))
        }
    };
    no_labels && no_cells
}

/// #2450: re-bind an orphaned profile set to the agent that now carries its
/// identity. Returns true when anything moved. NO-OP while any overlay is in
/// force. Runs before every `repair_coding_agent_profiles_config` call, because
/// repair fills a gap with an empty `A` that would otherwise hide it.
pub(crate) fn adopt_orphaned_profiles(settings: &mut AppSettings) -> bool {
    if settings
        .coding_agent_identity_physical
        .as_ref()
        .is_some_and(|raw| raw.get("codingAgentProfiles").is_some())
    {
        return false;
    }
    if settings.coding_agent_profiles.identity_by_agent.is_empty() {
        return false;
    }
    if !settings.local_overlay_state.is_empty() {
        log::info!(
            "[profiles] #2450 - orphan adoption skipped: a settings.local.json overlay is in force"
        );
        return false;
    }
    let current: HashSet<&str> = settings.agents.iter().map(|a| a.id.as_str()).collect();
    let mut orphans: Vec<(String, String)> = settings
        .coding_agent_profiles
        .identity_by_agent
        .iter()
        .filter(|(id, identity)| !current.contains(id.as_str()) && is_identity_shape(identity))
        .map(|(id, identity)| (id.clone(), identity.clone()))
        .collect();
    // Earliest agent in `settings.agents` order owns an identity; a later
    // colliding agent keeps its own state and never adopts.
    let mut claimed: BTreeMap<String, String> = BTreeMap::new();
    let mut moved = false;
    for agent in &settings.agents {
        let identity = agent_profile_identity(agent);
        if let Some(winner) = claimed.get(&identity) {
            log::warn!(
                "[profiles] #2450 - agents '{}' and '{}' share identity {}; only '{}' may adopt",
                winner,
                agent.id,
                identity,
                winner
            );
            continue;
        }
        claimed.insert(identity.clone(), agent.id.clone());
        if !is_profile_gap(&settings.coding_agent_profiles, &agent.id) {
            continue;
        }
        let Some(pos) = orphans.iter().position(|(_, o)| *o == identity) else {
            continue;
        };
        let (orphan_id, _) = orphans.remove(pos);
        let profiles = &mut settings.coding_agent_profiles;
        match profiles.profiles_by_agent.remove(&orphan_id) {
            Some(cells) => {
                profiles.profiles_by_agent.insert(agent.id.clone(), cells);
            }
            None => {
                profiles.profiles_by_agent.remove(&agent.id);
            }
        }
        if let Some(labels) = profiles.profile_labels_by_agent.remove(&orphan_id) {
            profiles
                .profile_labels_by_agent
                .insert(agent.id.clone(), labels);
        }
        profiles.identity_by_agent.remove(&orphan_id);
        log::info!(
            "[profiles] #2450 - agent '{}' adopted the orphaned profiles of '{}'",
            agent.id,
            orphan_id
        );
        moved = true;
    }
    moved
}

/// Pure carrier construction from raw merged inputs, before typed repair.
pub(crate) fn coding_agent_identity_physical_from_raw(
    value: &Value,
    physical_groups: &[String],
    overlay: &LocalSettingsOverlay,
) -> Value {
    serde_json::json!({
        "agents": value.get("agents"),
        "codingAgentProfiles": value.get("codingAgentProfiles"),
        "quotaSources": value.get("quotaSources"),
        "groupOwnership": { "instanceGroups": physical_groups, "overlayPath\
            s": overlay.owned_paths().iter().filter(|path| path.first().is_some_and(|key| ["agent\
            s", "codingAgentProfiles", "quotaSource\
            s"].contains(&key.as_str()))).collect::<Vec<_>>() },
    })
}

/// Borrow the caller's settings file lock; never reenter a locking loader.
/// Physical overlay bytes retain identity fields unknown to the typed view.
pub(crate) fn coding_agent_identity_with_physical_overlay(
    mut value: Value,
    path: &Path,
    physical_groups: &[String],
) -> Value {
    let unmerged = value.clone();
    let base_profiles = value.get("codingAgentProfiles").cloned();
    let overlay = LocalSettingsOverlay::load_and_merge_paired(
        path,
        &mut value,
        OVERLAY_INELIGIBLE_DISK_KEYS,
        OVERLAY_INELIGIBLE_LEGACY_KEYS,
        OVERLAY_DERIVED_ID_CLOSURES,
        CONFIGURATION_COMMAND_PAIRS,
    );
    restore_codex_home_carrier(value.as_object_mut().unwrap(), base_profiles.as_ref());
    // Match the loader's undecodable-overlay rule without using its repaired
    // typed value as identity authority. The fallback is the raw physical base.
    if !overlay.is_empty() {
        let mut decode_value = value.clone();
        apply_project_decode_to_value(
            &mut decode_value,
            production_instance_base().as_deref(),
            &projects::FsCandidateResolver,
        );
        if serde_json::from_value::<AppSettings>(decode_value).is_err() {
            return coding_agent_identity_physical_from_raw(
                &unmerged,
                physical_groups,
                &LocalSettingsOverlay::default(),
            );
        }
    }
    coding_agent_identity_physical_from_raw(&value, physical_groups, &overlay)
}

/// Refresh clone-based state replacements while borrowing their file lock.
/// Unavailable raw inputs clear transition evidence rather than retaining a stale seed source.
pub(crate) fn refresh_coding_agent_identity_at_locked_path(
    settings: &mut AppSettings,
    path: &Path,
) {
    let captured = (|| {
        let Some(object) =
            read_disk_object_for_write_typed(path, SettingsSaveStage::PreserveDiskGate)?
        else {
            return Ok(None);
        };
        let groups = read_agents_instance_file(&agents_instance_path(path));
        let layer = match &groups {
            Ok(_) => AgentsLayerState::Readable,
            Err(reason) => AgentsLayerState::Unreadable {
                path: agents_instance_path(path),
                reason: reason.clone(),
            },
        };
        let mut value = Value::Object(object);
        let groups = groups
            .ok()
            .flatten()
            .map(|groups| agents_instance_groups(&groups))
            .unwrap_or_default();
        let physical_groups = groups.keys().cloned().collect::<Vec<_>>();
        value.as_object_mut().unwrap().extend(groups);
        let raw = coding_agent_identity_with_physical_overlay(value, path, &physical_groups);
        Ok::<_, SettingsSaveError>(Some((raw, layer)))
    })();
    match captured {
        Ok(Some((raw, layer))) => {
            settings.coding_agent_identity_physical = Some(Arc::new(raw));
            set_agents_layer_state(settings, path, layer);
        }
        Ok(None) => settings.coding_agent_identity_physical = None,
        Err(error) => {
            log::warn!("[settings] binding physical refresh unavailable: {error:?}");
            settings.coding_agent_identity_physical = None;
        }
    }
}

/// Canonical identity-only projection of backend physical inputs. Never infer
/// authority from a client draft or from repaired/defaulted runtime rows.
pub(crate) fn coding_agent_identity_projection_hash(
    settings: &AppSettings,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    if settings.agents_layer.unreadable_path().is_some() {
        return Err("coding_agent_identity_physical_unreadable".to_string());
    }
    let raw = settings
        .coding_agent_identity_physical
        .as_ref()
        .ok_or_else(|| "coding_agent_identity_physical_missing".to_string())?;
    let mut projection = raw.as_ref().clone();
    let agents = projection
        .get_mut("agents")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "coding_agent_identity_agents_missing".to_string())?;
    let mut ids = HashSet::new();
    for agent in agents.iter_mut() {
        let object = agent
            .as_object_mut()
            .ok_or_else(|| "coding_agent_identity_invalid".to_string())?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "coding_agent_identity_invalid".to_string())?;
        if !ids.insert(id.to_string()) {
            return Err("coding_agent_identity_duplicate".to_string());
        }
        for key in ["order", "name", "label", "color", "id_hash_debug"] {
            object.remove(key);
        }
    }
    agents.sort_by(|a, b| {
        a.get("id")
            .and_then(Value::as_str)
            .cmp(&b.get("id").and_then(Value::as_str))
    });
    if let Some(slots) = projection
        .get_mut("codingAgentProfiles")
        .and_then(|profiles| profiles.get_mut("profileSlots"))
        .and_then(Value::as_object_mut)
    {
        for slot in slots.values_mut() {
            if let Some(slot) = slot.as_object_mut() {
                slot.remove("label");
            }
        }
    }
    if let Some(profiles) = projection
        .get_mut("codingAgentProfiles")
        .and_then(|profiles| profiles.get_mut("profilesByAgent"))
        .and_then(Value::as_object_mut)
    {
        for cells in profiles.values_mut().filter_map(Value::as_object_mut) {
            for cell in cells.values_mut().filter_map(Value::as_object_mut) {
                cell.remove("id_hash_debug");
            }
        }
    }
    // serde_json's Map is sorted (no preserve_order feature); recurse explicitly
    // so the revision contract remains canonical if that feature changes later.
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let sorted: BTreeMap<_, _> = object
                    .iter()
                    .map(|(key, value)| (key.clone(), canonical(value)))
                    .collect();
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
            _ => value.clone(),
        }
    }
    let bytes = serde_json::to_vec(&canonical(&projection)).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn bootstrap_identity_revision(settings: &AppSettings) -> Result<String, String> {
    coding_agent_identity_projection_hash(settings).map(|hash| format!("r1:0:{hash}"))
}

pub(crate) fn bootstrap_binding_revision(
    settings: &AppSettings,
    original_agent_id: &str,
) -> Result<String, String> {
    let revision = bootstrap_identity_revision(settings)?;
    let rows = settings
        .coding_agent_identity_physical
        .as_ref()
        .and_then(|raw| raw.get("agents"))
        .and_then(Value::as_array)
        .ok_or_else(|| "coding_agent_identity_agents_missing".to_string())?;
    if rows
        .iter()
        .filter(|row| row.get("id").and_then(Value::as_str) == Some(original_agent_id))
        .count()
        != 1
    {
        return Err("coding_agent_binding_entry_missing_or_ambiguous".to_string());
    }
    Ok(revision)
}

/// #2450: record `identityByAgent[id]` for every agent actually written to the
/// base file. Under an overlay those are the agents `restore_base` leaves, so a
/// clone is restored first and an overlay's effective definition is never
/// recorded; `restore_base` itself still runs last over `out`. Rows for other
/// ids are kept verbatim (orphans).
pub(crate) fn write_identity_rows(out: &mut Map<String, Value>, overlay: &LocalSettingsOverlay) {
    let base_agents = if overlay.is_empty() {
        out.get("agents").cloned()
    } else {
        let mut restored = out.clone();
        overlay.restore_base(&mut restored);
        restored.remove("agents")
    };
    let Some(Value::Array(agents)) = base_agents else {
        return;
    };
    let Some(Value::Object(profiles)) = out.get_mut("codingAgentProfiles") else {
        return;
    };
    let Value::Object(rows) = profiles
        .entry("identityByAgent")
        .or_insert_with(|| Value::Object(Map::new()))
    else {
        return;
    };
    for agent in agents {
        match serde_json::from_value::<AgentConfig>(agent) {
            Ok(agent) => {
                rows.insert(
                    agent.id.clone(),
                    Value::String(agent_profile_identity(&agent)),
                );
            }
            Err(e) => log::warn!("[profiles] #2450 - identity row skipped: {e}"),
        }
    }
}
