mod identity;
mod registered;

use super::super::{
    configuration_identity, configuration_identity_debug, default_profiles_schema_version,
    default_true, refresh_agent_id_hashes, refresh_pair_id_hashes, CodingAgentProfilesConfig,
};
use super::{
    deserialize_agent_id_hash, deserialize_agent_id_hash_debug, raw_configuration_command,
    validate_agent_command_text, AgentConfig, CodingAgentEnv,
};
pub use identity::agent_profile_identity;
pub(crate) use identity::{
    adopt_orphaned_profiles, bootstrap_binding_revision, coding_agent_identity_physical_from_raw,
    coding_agent_identity_with_physical_overlay, refresh_coding_agent_identity_at_locked_path,
    write_identity_rows,
};
#[cfg(test)]
pub(crate) use identity::{bootstrap_identity_revision, is_profile_gap};
pub(crate) use registered::{
    capture_registered_profile_gate, captured_registered_profile_object,
    classify_registered_profile_inputs, collect_registered_profile_gate,
    recheck_registered_profile_gate, registered_profile_raw_gate, registered_profiles_for_read,
    selected_registered_profile_owner, RegisteredProfileCapture, RegisteredProfileDocument,
    RegisteredProfileProblemKind,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSlotConfig {
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCellConfig {
    #[serde(default, rename = "id_hash", skip_serializing_if = "Option::is_none")]
    pub id_hash: Option<String>,
    #[serde(
        default,
        rename = "id_hash_debug",
        skip_serializing_if = "Option::is_none"
    )]
    pub id_hash_debug: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    #[serde(rename = "additionalArguments")]
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_timeout_seconds: Option<u32>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProfileCellConfigWire {
    #[serde(
        default,
        rename = "id_hash",
        deserialize_with = "deserialize_agent_id_hash"
    )]
    pub id_hash: Option<String>,
    #[serde(
        default,
        rename = "id_hash_debug",
        deserialize_with = "deserialize_agent_id_hash_debug"
    )]
    pub id_hash_debug: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(
        default,
        deserialize_with = "crate::config::settings::deserialize_present_command"
    )]
    pub command: Option<String>,
    #[serde(
        default,
        rename = "additionalArguments",
        deserialize_with = "crate::config::settings::deserialize_present_command"
    )]
    pub canonical_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_timeout_seconds: Option<u32>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: String,
}

impl<'de> Deserialize<'de> for ProfileCellConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ProfileCellConfigWire::deserialize(deserializer)?;
        let command = crate::config::settings::select_configuration_command(
            wire.canonical_command.as_deref(),
            wire.command.as_deref(),
        )
        .unwrap_or_default()
        .to_owned();
        Ok(Self {
            id_hash: wire.id_hash,
            id_hash_debug: wire.id_hash_debug,
            enabled: wire.enabled,
            command,
            preflight: wire.preflight,
            preflight_timeout_seconds: wire.preflight_timeout_seconds,
            env: wire.env,
            notes: wire.notes,
        })
    }
}

// E2432: raw physical storage is authoritative. ID maps below are transient
// compatibility projections only; quarantine never participates in projection.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PhysicalProfileSets {
    pub(crate) rows: BTreeMap<String, String>,
    pub(crate) cells: Map<String, Value>,
    pub(crate) labels: Map<String, Value>,
    pub(crate) orphans: Option<Map<String, Value>>,
}

pub(crate) fn profile_object(
    value: Option<&Value>,
    path: &str,
) -> Result<Map<String, Value>, String> {
    match value {
        None => Ok(Map::new()),
        Some(Value::Object(map)) => Ok(map.clone()),
        Some(_) => Err(format!("invalidProfileMap: {path}")),
    }
}

pub(crate) fn profile_container(source: &Value, registered: bool) -> Option<&Value> {
    if registered {
        source.get("codingAgentProfiles")
    } else {
        Some(source)
    }
}

pub(crate) fn validate_profile_orphans(
    value: Option<&Value>,
) -> Result<Option<Map<String, Value>>, String> {
    let Some(value) = value else { return Ok(None) };
    let invalid = |path: &str| format!("invalidLegacyProfileOrphans: {path}");
    let object = value
        .as_object()
        .ok_or_else(|| invalid("legacyProfileOrphans"))?;
    for (id, records) in object {
        let path = format!("legacyProfileOrphans[{id:?}]");
        let records = records
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| invalid(&path))?;
        for (index, record) in records.iter().enumerate() {
            let record = record
                .as_object()
                .ok_or_else(|| invalid(&format!("{path}[{index}]")))?;
            if !["cells", "labels", "identity"]
                .iter()
                .any(|key| record.contains_key(*key))
            {
                return Err(invalid(&format!("{path}[{index}]")));
            }
        }
    }
    Ok(Some(object.clone()))
}

pub(crate) fn normalized_profile_cells(value: Option<&Value>) -> Result<Value, String> {
    let mut cells = profile_object(value, "cells")?;
    for (letter, raw) in &mut cells {
        if !is_valid_profile_letter(letter) {
            return Err(format!("invalidProfileLetter: {letter}"));
        }
        let cell: ProfileCellConfig =
            serde_json::from_value(raw.clone()).map_err(|e| e.to_string())?;
        let mut normalized = raw.as_object().cloned().ok_or("invalidProfileCell")?;
        normalized.remove("id_hash");
        normalized.remove("id_hash_debug");
        let mut typed = serde_json::to_value(cell).map_err(|e| e.to_string())?;
        let typed = typed.as_object_mut().expect("cell object");
        typed.remove("id_hash");
        typed.remove("id_hash_debug");
        normalized.extend(typed.clone());
        // Optional serde-default fields compare equally whether omitted or null.
        for key in ["preflight", "preflightTimeoutSeconds"] {
            if normalized.get(key).is_some_and(Value::is_null) {
                normalized.remove(key);
            }
        }
        *raw = Value::Object(normalized);
    }
    if !cells.contains_key("A") {
        let mut empty = serde_json::to_value(empty_profile_cell()).map_err(|e| e.to_string())?;
        empty.as_object_mut().unwrap().remove("id_hash");
        empty.as_object_mut().unwrap().remove("id_hash_debug");
        cells.insert("A".into(), empty);
    }
    Ok(Value::Object(cells))
}

pub(crate) fn physical_profile_rows(
    source: &Value,
    registered: bool,
) -> Result<BTreeMap<String, String>, String> {
    let mut rows = BTreeMap::new();
    let mut seen = HashSet::new();
    let empty = Vec::new();
    let agents = match source.get("agents") {
        None => &empty,
        Some(Value::Array(rows)) => rows,
        Some(_) => return Err("invalidAgentsArray".into()),
    };
    for row in agents {
        let object = row.as_object().ok_or("invalidAgentRow")?;
        let id = object
            .get(if registered { "id" } else { "key" })
            .and_then(Value::as_str)
            .ok_or("invalidAgentKey")?;
        if id.is_empty() {
            return Err("invalidAgentKey".into());
        }
        if !registered {
            crate::config::coding_agents_catalog::validate_catalog_key(id)?;
        }
        if !seen.insert(id.to_owned()) {
            return Err(format!("duplicateAgentKey: {id}"));
        }
        if object.get("remove") == Some(&Value::Bool(true)) {
            continue;
        }
        let command = raw_configuration_command(object, "baseCommand")?;
        let Some(command) = command.filter(|command| !command.trim().is_empty()) else {
            if registered {
                return Err("invalidRegisteredCommand".into());
            }
            continue;
        };
        if let Err(reason) = validate_agent_command_text("physical profile row", command) {
            if registered {
                return Err(reason);
            }
            continue;
        }
        let envs: Vec<CodingAgentEnv> = object
            .get("envs")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        rows.insert(id.to_owned(), configuration_identity(command, &envs));
    }
    Ok(rows)
}

pub(crate) fn physical_profile_sets(
    source: &Value,
    registered: bool,
) -> Result<PhysicalProfileSets, String> {
    let rows = physical_profile_rows(source, registered)?;
    let empty = Value::Object(Map::new());
    let profiles = profile_container(source, registered).unwrap_or(&empty);
    if !profiles.is_object() {
        return Err("invalidProfileContainer".into());
    }
    let version = profiles.get("profileBindingVersion");
    if version.is_some() && version != Some(&Value::from(1)) {
        return Err("unsupportedProfileBindingVersion".into());
    }
    let canonical = version.is_some();
    let legacy_cells_key = if registered {
        "profilesByAgent"
    } else {
        "codingAgentProfiles"
    };
    let legacy_labels_key = if registered {
        "profileLabelsByAgent"
    } else {
        "profileLabels"
    };
    let legacy_cells = profile_object(profiles.get(legacy_cells_key), legacy_cells_key)?;
    let legacy_labels = profile_object(profiles.get(legacy_labels_key), legacy_labels_key)?;
    let identities = if registered {
        profile_object(profiles.get("identityByAgent"), "identityByAgent")?
    } else {
        Map::new()
    };
    let mut result = PhysicalProfileSets {
        rows,
        cells: profile_object(profiles.get("profilesByBaseHash"), "profilesByBaseHash")?,
        labels: profile_object(
            profiles.get("profileLabelsByBaseHash"),
            "profileLabelsByBaseHash",
        )?,
        orphans: validate_profile_orphans(profiles.get("legacyProfileOrphans"))?,
    };
    for labels in legacy_labels
        .iter()
        .filter(|(id, _)| result.rows.contains_key(*id))
        .filter_map(|(_, value)| value.as_object())
        .filter(|_| !canonical)
    {
        if labels.keys().any(|letter| !is_valid_profile_letter(letter)) {
            return Err("invalidProfileLetter".into());
        }
    }
    for cells in result.cells.values_mut() {
        *cells = normalized_profile_cells(Some(cells))?;
    }
    for labels in result.labels.values() {
        if labels
            .as_object()
            .is_some_and(|labels| labels.keys().any(|letter| !is_valid_profile_letter(letter)))
        {
            return Err("invalidProfileLetter".into());
        }
        serde_json::from_value::<BTreeMap<String, String>>(labels.clone())
            .map_err(|e| e.to_string())?;
    }
    if !canonical {
        for (id, hash) in &result.rows {
            let cells = normalized_profile_cells(legacy_cells.get(id))?;
            let labels = Value::Object(profile_object(legacy_labels.get(id), "labels")?);
            serde_json::from_value::<BTreeMap<String, String>>(labels.clone())
                .map_err(|e| e.to_string())?;
            if result
                .cells
                .get(hash)
                .is_some_and(|previous| previous != &cells)
                || result
                    .labels
                    .get(hash)
                    .is_some_and(|previous| previous != &labels)
            {
                return Err(format!("sourceProfileConflict: {hash}; row {id}"));
            }
            result.cells.insert(hash.clone(), cells);
            result.labels.insert(hash.clone(), labels);
        }
    }
    // Capture raw values before typed decoding. Existing arrays (including equal
    // records) keep their order. Only newly appended equal records are suppressed.
    let ids: std::collections::BTreeSet<_> = legacy_cells
        .keys()
        .chain(legacy_labels.keys())
        .chain(identities.keys())
        .collect();
    for id in ids {
        if result.rows.contains_key(id) {
            continue;
        }
        let mut record = Map::new();
        for (key, map) in [
            ("cells", &legacy_cells),
            ("labels", &legacy_labels),
            ("identity", &identities),
        ] {
            if let Some(value) = map.get(id) {
                record.insert(key.into(), value.clone());
            }
        }
        let record = Value::Object(record);
        let records = result
            .orphans
            .get_or_insert_with(Map::new)
            .entry(id.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
        let records = records.as_array_mut().expect("validated quarantine array");
        if !records.contains(&record) {
            records.push(record);
        }
    }
    Ok(result)
}

/// A read-only compatibility view. Never serialize this projection as storage.
pub(crate) fn project_physical_profiles(source: &Value, registered: bool) -> Result<Value, String> {
    let sets = physical_profile_sets(source, registered)?;
    let mut projected = source.clone();
    if registered && projected.get("codingAgentProfiles").is_none() {
        projected["codingAgentProfiles"] = serde_json::json!({});
    }
    let profiles = if registered {
        projected.get_mut("codingAgentProfiles").unwrap()
    } else {
        &mut projected
    };
    let profiles = profiles.as_object_mut().ok_or("invalidProfileContainer")?;
    let mut cells = Map::new();
    let mut labels = Map::new();
    for (id, hash) in &sets.rows {
        cells.insert(
            id.clone(),
            sets.cells
                .get(hash)
                .cloned()
                .unwrap_or(normalized_profile_cells(None)?),
        );
        labels.insert(
            id.clone(),
            sets.labels
                .get(hash)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        );
    }
    profiles.insert(
        if registered {
            "profilesByAgent"
        } else {
            "codingAgentProfiles"
        }
        .into(),
        Value::Object(cells),
    );
    profiles.insert(
        if registered {
            "profileLabelsByAgent"
        } else {
            "profileLabels"
        }
        .into(),
        Value::Object(labels),
    );
    if registered {
        profiles.remove("identityByAgent");
    }
    // A compatibility draft is an ID projection, never a second canonical
    // authority. This also distinguishes an explicit alias edit from stale
    // legacy maps still present in a raw version1 physical document.
    for key in [
        "profileBindingVersion",
        "profilesByBaseHash",
        "profileLabelsByBaseHash",
    ] {
        profiles.remove(key);
    }
    Ok(projected)
}

/// Regroup an explicit ID-view edit against the previous physical projection.
/// Unchanged aliases never compete with a changed alias. Raw extensions survive.
pub(crate) fn canonical_profile_source(
    before: &Value,
    incoming: &Value,
    registered: bool,
) -> Result<Value, String> {
    if !registered && before.get("managed").is_some() {
        return Err("sourceReadOnly".into());
    }
    let mut sets = physical_profile_sets(before, registered)?;
    let next_rows = physical_profile_rows(incoming, registered)?;
    let empty = serde_json::json!({});
    let incoming_profiles = profile_container(incoming, registered).unwrap_or(&empty);
    let compatible_draft =
        if incoming_profiles.get("profileBindingVersion") == Some(&Value::from(1)) {
            project_physical_profiles(before, registered)?
        } else {
            incoming.clone()
        };
    let submitted = profile_container(&compatible_draft, registered).unwrap_or(&empty);
    let old_projection = project_physical_profiles(before, registered)?;
    let old = profile_container(&old_projection, registered).unwrap_or(&empty);
    let cells_key = if registered {
        "profilesByAgent"
    } else {
        "codingAgentProfiles"
    };
    let labels_key = if registered {
        "profileLabelsByAgent"
    } else {
        "profileLabels"
    };
    let submitted_cells = profile_object(submitted.get(cells_key), cells_key)?;
    let submitted_labels = profile_object(submitted.get(labels_key), labels_key)?;
    let mut changes = BTreeMap::<String, (Value, Value)>::new();
    let mut forks = BTreeMap::<String, (Value, Value)>::new();
    for (id, hash) in &next_rows {
        let old_hash = sets.rows.get(id);
        let previous_cells = old.get(cells_key).and_then(|map| map.get(id));
        let previous_labels = old
            .get(labels_key)
            .and_then(|map| map.get(id))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let mut cells = normalized_profile_cells(submitted_cells.get(id))?;
        let labels = Value::Object(profile_object(submitted_labels.get(id), labels_key)?);
        let old_cells = normalized_profile_cells(previous_cells)?;
        if let (Some(new_cells), Some(previous)) = (cells.as_object_mut(), old_cells.as_object()) {
            for (letter, cell) in new_cells {
                if let (Some(cell), Some(previous)) = (
                    cell.as_object_mut(),
                    previous.get(letter).and_then(Value::as_object),
                ) {
                    let typed: ProfileCellConfig =
                        serde_json::from_value(Value::Object(previous.clone()))
                            .map_err(|e| e.to_string())?;
                    let known = serde_json::to_value(typed).map_err(|e| e.to_string())?;
                    for (key, value) in previous {
                        if !known.as_object().unwrap().contains_key(key)
                            && !["id_hash", "id_hash_debug"].contains(&key.as_str())
                        {
                            cell.entry(key.clone()).or_insert_with(|| value.clone());
                        }
                    }
                }
            }
        }
        let changed = cells != old_cells || labels != previous_labels;
        if changed {
            let value = (cells, labels);
            if changes.get(hash).is_some_and(|existing| existing != &value) {
                return Err(format!("sourceProfileConflict: {hash}; row {id}"));
            }
            changes.insert(hash.clone(), value);
        } else if old_hash.is_some_and(|old_hash| old_hash != hash) {
            let value = (old_cells, previous_labels);
            if forks.get(hash).is_some_and(|existing| existing != &value) {
                return Err(format!("sourceProfileConflict: {hash}; row {id}"));
            }
            forks.insert(hash.clone(), value);
        } else if !sets.cells.contains_key(hash) {
            forks.insert(hash.clone(), (cells, labels));
        }
    }
    for (hash, (cells, labels)) in forks {
        if sets
            .cells
            .get(&hash)
            .is_some_and(|existing| existing != &cells)
            || sets
                .labels
                .get(&hash)
                .is_some_and(|existing| existing != &labels)
        {
            return Err(format!("sourceProfileConflict: {hash}"));
        }
        sets.cells.entry(hash.clone()).or_insert(cells);
        sets.labels.entry(hash).or_insert(labels);
    }
    for (hash, (cells, labels)) in changes {
        if next_rows
            .iter()
            .any(|(id, h)| h == &hash && sets.rows.get(id).is_none_or(|old| old != h))
            && (sets.cells.get(&hash).is_some_and(|v| v != &cells)
                || sets.labels.get(&hash).is_some_and(|v| v != &labels))
        {
            return Err(format!("sourceProfileConflict: {hash}"));
        }
        sets.cells.insert(hash.clone(), cells);
        sets.labels.insert(hash, labels);
    }
    let mut result = before.clone();
    let result_root = result.as_object_mut().ok_or("invalidSource")?;
    let incoming_root = incoming.as_object().ok_or("invalidSource")?;
    if let Some(rows) = incoming_root.get("agents") {
        result_root.insert("agents".into(), rows.clone());
    }
    if registered && !result_root.contains_key("codingAgentProfiles") {
        result_root.insert("codingAgentProfiles".into(), empty.clone());
    }
    let target = if registered {
        result_root
            .get_mut("codingAgentProfiles")
            .unwrap()
            .as_object_mut()
            .ok_or("invalidProfileContainer")?
    } else {
        result_root
    };
    if let Some(fields) = incoming_profiles.as_object() {
        for key in [
            "schemaVersion",
            "profileSlots",
            "defaultProfileByAgent",
            "codexHomeKeyByAgent",
        ] {
            if let Some(value) = fields.get(key) {
                target.insert(key.into(), value.clone());
            }
        }
    }
    if !registered {
        target
            .entry("profileSlots".to_owned())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(order) = incoming_root.get("order") {
            target.insert("order".into(), order.clone());
        } else {
            target.remove("order");
        }
    }
    target.insert("profileBindingVersion".into(), Value::from(1));
    target.insert("profilesByBaseHash".into(), Value::Object(sets.cells));
    target.insert("profileLabelsByBaseHash".into(), Value::Object(sets.labels));
    if let Some(orphans) = sets.orphans {
        target.insert("legacyProfileOrphans".into(), Value::Object(orphans));
    }
    target.remove(cells_key);
    target.remove(labels_key);
    if registered {
        target.remove("identityByAgent");
    }
    refresh_physical_source_rows(&mut result, registered)?;
    Ok(result)
}

pub(crate) fn refresh_physical_source_rows(
    source: &mut Value,
    registered: bool,
) -> Result<(), String> {
    let hashes = physical_profile_rows(source, registered)?;
    for row in source
        .get_mut("agents")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        let object = row.as_object_mut().ok_or("invalidAgentRow")?;
        let key = object
            .get(if registered { "id" } else { "key" })
            .and_then(Value::as_str)
            .ok_or("invalidAgentKey")?;
        if let Some(hash) = hashes.get(key).filter(|_| registered) {
            let command = raw_configuration_command(object, "baseCommand")?.unwrap_or_default();
            let envs: Vec<CodingAgentEnv> = object
                .get("envs")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            let debug = serde_json::json!(configuration_identity_debug(command, &envs));
            object.insert("id_hash".into(), Value::String(hash.clone()));
            object.insert("id_hash_debug".into(), debug);
        } else {
            object.remove("id_hash");
            object.remove("id_hash_debug");
        }
    }
    Ok(())
}

/// Catalog keys become transient IDs; physical definitions remain untouched.
pub(crate) fn decode_physical_catalog(
    value: &Value,
) -> Result<(Vec<AgentConfig>, CodingAgentProfilesConfig), String> {
    if !matches!(
        value.get("schemaVersion").and_then(Value::as_u64),
        Some(1 | 2)
    ) {
        return Err("unsupportedCatalogSchema".into());
    }
    if value.get("schemaVersion").and_then(Value::as_u64) == Some(2) {
        crate::config::coding_agents_catalog::parse_catalog_schema2(
            &serde_json::to_vec(value).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    let sets = physical_profile_sets(value, false)?;
    let projected = project_physical_profiles(value, false)?;
    let mut agents = Vec::new();
    for raw in value
        .get("agents")
        .and_then(Value::as_array)
        .ok_or("invalidAgentsArray")?
    {
        let key = raw
            .get("key")
            .and_then(Value::as_str)
            .ok_or("invalidAgentKey")?;
        if !sets.rows.contains_key(key) {
            continue;
        }
        crate::config::coding_agents_catalog::validate_catalog_key(key)?;
        let mut row = raw.as_object().cloned().ok_or("invalidAgentRow")?;
        row.insert("id".into(), Value::String(key.to_owned()));
        row.entry("label".to_owned())
            .or_insert_with(|| Value::String(String::new()));
        row.entry("color".to_owned())
            .or_insert_with(|| Value::String(String::new()));
        agents.push(serde_json::from_value(Value::Object(row)).map_err(|e| e.to_string())?);
    }
    let profiles = serde_json::json!({
        "profilesByAgent":projected.get("codingAgentProfiles"),
        "profileLabelsByAgent":projected.get("profileLabels"),
        "profileSlots":value.get("profileSlots").cloned().unwrap_or_else(|| serde_json::json!({}))
    });
    let mut profiles = serde_json::from_value(profiles).map_err(|e| e.to_string())?;
    refresh_agent_id_hashes(&mut agents);
    refresh_pair_id_hashes(&agents, &mut profiles);
    Ok((agents, profiles))
}

pub(crate) fn duplicate_profile_document(bytes: &[u8], path: &Path) -> Option<String> {
    serde_json::from_slice::<Value>(bytes).ok()?;
    crate::config::coding_agents_catalog::parse_strict_json(bytes)
        .err()
        .map(|reason| {
            format!(
                "dupl\
        icateJsonMember: {}: {reason}; no files changed; correct the ambiguous file \
        externally",
                path.display()
            )
        })
}

pub fn is_valid_profile_letter(letter: &str) -> bool {
    letter.len() == 1 && letter.as_bytes()[0].is_ascii_uppercase()
}

pub fn normalize_profile_letter(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() == 1 {
        let upper = trimmed.to_ascii_uppercase();
        if is_valid_profile_letter(&upper) {
            return Some(upper);
        }
    }
    None
}

pub fn empty_profile_cell() -> ProfileCellConfig {
    ProfileCellConfig {
        id_hash: None,
        id_hash_debug: None,
        preflight: None,
        preflight_timeout_seconds: None,
        enabled: true,
        command: String::new(),
        env: BTreeMap::new(),
        notes: String::new(),
    }
}

pub fn repair_coding_agent_profiles_config(
    profiles: &mut CodingAgentProfilesConfig,
    agents: &[AgentConfig],
) -> bool {
    let mut changed = false;

    if profiles.schema_version < 2 {
        profiles.schema_version = default_profiles_schema_version();
        changed = true;
    }

    let original_letters_len = profiles.profile_slots.len();
    profiles
        .profile_slots
        .retain(|letter, _| is_valid_profile_letter(letter));
    changed |= profiles.profile_slots.len() != original_letters_len;

    if !profiles.profile_slots.contains_key("A") {
        profiles.profile_slots.insert(
            "A".to_string(),
            ProfileSlotConfig {
                label: String::new(),
            },
        );
        changed = true;
    }

    let original_defaults_len = profiles.default_profile_by_agent.len();
    profiles
        .default_profile_by_agent
        .retain(|_, letter| is_valid_profile_letter(letter));
    changed |= profiles.default_profile_by_agent.len() != original_defaults_len;

    for cells in profiles.profiles_by_agent.values_mut() {
        let original_cells_len = cells.len();
        cells.retain(|letter, _| is_valid_profile_letter(letter));
        changed |= cells.len() != original_cells_len;
    }

    // #548: prune invalid (non A..Z) letters from each agent's label-override map,
    // mirroring the cell-letter prune above. Never creates override entries and never
    // prunes by agent id (an override for a not-yet-loaded agent is harmless and may
    // precede its agent in load order, per plan 3.7). `retain` keeps every valid key,
    // so a clean config leaves `changed` false and is not rewritten on load.
    for labels in profiles.profile_labels_by_agent.values_mut() {
        let original_len = labels.len();
        labels.retain(|letter, _| is_valid_profile_letter(letter));
        changed |= labels.len() != original_len;
    }

    for agent in agents {
        let cells = profiles
            .profiles_by_agent
            .entry(agent.id.clone())
            .or_default();
        if !cells.contains_key("A") {
            cells.insert("A".to_string(), empty_profile_cell());
            changed = true;
        }
    }

    changed
}
