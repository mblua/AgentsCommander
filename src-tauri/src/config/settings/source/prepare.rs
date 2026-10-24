use super::super::agents_instance_groups;
use super::super::apply_project_decode_to_value;
use super::super::canonical_profile_source;
use super::super::decode_physical_catalog;
use super::super::production_instance_base;
use super::super::retain_configuration_commands;
use super::super::AppSettings;
use super::super::CONFIGURATION_COMMAND_PAIRS;
use super::super::OVERLAY_DERIVED_ID_CLOSURES;
use super::super::OVERLAY_INELIGIBLE_DISK_KEYS;
use super::super::OVERLAY_INELIGIBLE_LEGACY_KEYS;
use super::identity::configuration_identity;
use super::identity::profile_identity;
use super::identity::IdentityValidationCode;
use super::identity::IdentityValidationError;
use super::read::decode_registered_raw;
use super::read::origin_paths_overlap;
use super::read::LegacyCompatibilityOrigin;
use super::read::LegacyOriginLinkProof;
use super::read::LegacyOriginUnit;
use super::read::OriginBinding;
use super::read::OriginInputs;
use super::read::RawSourceSnapshot;
use super::read::SourceReadContext;
use super::OriginPhysicalInput;
use super::SourceKind;
use super::SourcePreparationError;
use super::SourceRef;
use super::SourceRevision;
use crate::config::local_overlay::LocalSettingsOverlay;
use crate::config::settings::project_path as projects;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct PreparedSourceCandidate {
    pub(crate) source: SourceRef,
    pub(crate) expected_revision: SourceRevision,
    pub(crate) context_revision: String,
    pub(crate) before_bytes: Option<Vec<u8>>,
    pub(crate) serialized_after: Vec<u8>,
    pub(crate) persisted_names: Vec<(String, String)>,
    pub(crate) origin_binding: Option<OriginBinding>,
}

#[allow(dead_code)] // The later writer calls this while holding its proofs.
impl PreparedSourceCandidate {
    pub(crate) fn revalidate_before_write(
        &self,
        context: &SourceReadContext,
    ) -> Result<(), SourcePreparationError> {
        if self.context_revision != context.revision {
            return Err(SourcePreparationError::ContextChanged);
        }
        let observed = OriginPhysicalInput::read(&context.locator(&self.source)?)
            .revision()
            .ok_or(SourcePreparationError::OriginUnavailable)?;
        if observed != self.expected_revision {
            return Err(SourcePreparationError::StaleRevision);
        }
        if let Some(binding) = &self.origin_binding {
            if binding.registered.is_none()
                || binding.settings.is_none()
                || binding.overlay.is_none()
            {
                return Err(SourcePreparationError::OriginUnavailable);
            }
            let observed = OriginInputs::read(context);
            if &observed.binding != binding {
                return Err(SourcePreparationError::StaleRevision);
            }
        }
        Ok(())
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn raw_entries(value: &Value, key_name: &str) -> BTreeMap<String, Value> {
    value
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .get(key_name)
                .and_then(Value::as_str)
                .map(|key| (key.into(), entry.clone()))
        })
        .collect()
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn changed_origin_units(
    before: &Value,
    after: &Value,
    origin: &LegacyCompatibilityOrigin,
) -> std::collections::BTreeSet<LegacyOriginUnit> {
    let mut affected = std::collections::BTreeSet::new();
    let old = raw_entries(before, "id");
    let new = raw_entries(after, "id");
    for key in old.keys().chain(new.keys()) {
        if old.get(key) != new.get(key) {
            affected.insert(LegacyOriginUnit::Entry(key.clone()));
            affected.extend(
                origin
                    .represented
                    .iter()
                    .filter(|unit| matches!(unit, LegacyOriginUnit::Profile(k, _) if k == key))
                    .cloned(),
            );
        }
    }
    // Include complete raw units, including removals, unknown fields within a
    // cell and absent/null changes. Labels stay a separate presentation domain.
    for map_name in [
        "profilesByAgent",
        "profileLabelsByAgent",
        "identityByAgent",
        "defaultProfileByAgent",
        "profileSlots",
    ] {
        let root = |v: &Value| {
            v.get("codingAgentProfiles")
                .and_then(|v| v.get(map_name))
                .cloned()
        };
        let old = root(before);
        let new = root(after);
        if old == new {
            continue;
        }
        let keys: std::collections::BTreeSet<_> = old
            .as_ref()
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| m.keys())
            .chain(
                new.as_ref()
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|m| m.keys()),
            )
            .cloned()
            .collect();
        for key in keys {
            let a = old.as_ref().and_then(|v| v.get(&key));
            let b = new.as_ref().and_then(|v| v.get(&key));
            if a == b {
                continue;
            }
            match map_name {
                "defaultProfileByAgent" => {
                    affected.insert(LegacyOriginUnit::Default(key));
                }
                "profileSlots" => {
                    affected.insert(LegacyOriginUnit::Slot(key));
                }
                "identityByAgent" => {
                    affected.extend(
                        origin
                            .represented
                            .iter()
                            .filter(|u| matches!(u, LegacyOriginUnit::Profile(k, _) if k == &key))
                            .cloned(),
                    );
                    // A missing/orphan profile container still needs its own
                    // identity ownership checked, not equality of the stamp.
                    if origin.applied_paths.iter().any(|p| {
                        origin_paths_overlap(
                            p,
                            &[
                                "codingAgentProfiles".into(),
                                "identityByAgent".into(),
                                key.clone(),
                            ],
                        )
                    }) {
                        affected.insert(LegacyOriginUnit::Profile(key, String::new()));
                    }
                }
                _ => {
                    let letters: std::collections::BTreeSet<_> = a
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|m| m.keys())
                        .chain(
                            b.and_then(Value::as_object)
                                .into_iter()
                                .flat_map(|m| m.keys()),
                        )
                        .cloned()
                        .collect();
                    for letter in letters {
                        if a.and_then(|v| v.get(&letter)) != b.and_then(|v| v.get(&letter)) {
                            affected.insert(if map_name == "profilesByAgent" {
                                LegacyOriginUnit::Profile(key.clone(), letter)
                            } else {
                                LegacyOriginUnit::Presentation(key.clone(), letter)
                            });
                        }
                    }
                }
            }
        }
    }
    affected
}

#[allow(dead_code)] // Dependency inventory is owned by the later coordinator.
pub(crate) fn unresolved_default_dependency(
    origin: &LegacyCompatibilityOrigin,
    units: &std::collections::BTreeSet<LegacyOriginUnit>,
) -> bool {
    units.iter().any(|unit| {
        matches!(
            unit,
            LegacyOriginUnit::Entry(_) | LegacyOriginUnit::Profile(_, _)
        )
    }) && origin.represented.iter().any(|unit| {
        matches!(unit, LegacyOriginUnit::Default(_))
            && origin.require_disjoint([unit.clone()]).is_err()
    })
}

/// Unknown persisted fields are not discarded by the typed validation view.
/// Schema keys derive from the concrete model; raw absence/null remains raw.
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn preserve_unknown_fields(before: &Value, after: &Value, model: &Value) -> bool {
    match (before, after, model) {
        (Value::Object(old), Value::Object(new), Value::Object(known)) => {
            old.iter().all(|(key, value)| match known.get(key) {
                Some(shape) => new
                    .get(key)
                    .is_none_or(|next| preserve_unknown_fields(value, next, shape)),
                None => new.get(key) == Some(value),
            })
        }
        (Value::Array(old), Value::Array(new), Value::Array(known)) => {
            let key = if old.first().is_some_and(|v| v.get("id").is_some()) {
                "id"
            } else {
                "key"
            };
            old.iter().all(|entry| {
                let id = entry.get(key);
                match id {
                    Some(id) => match new.iter().find(|entry| entry.get(key) == Some(id)) {
                        Some(next) => known
                            .iter()
                            .find(|entry| entry.get(key) == Some(id))
                            .is_some_and(|shape| preserve_unknown_fields(entry, next, shape)),
                        None => true, // Explicit removal is checked by OperationAffected.
                    },
                    None => true,
                }
            })
        }
        _ => true,
    }
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn validate_source_identity_groups(
    before: &Value,
    after: &Value,
    registered: bool,
) -> Result<(), SourcePreparationError> {
    let groups = |value: &Value| -> Result<
        BTreeMap<String, std::collections::BTreeSet<String>>,
        SourcePreparationError,
    > {
        let mut groups = BTreeMap::<String, std::collections::BTreeSet<String>>::new();
        let (agents, profiles) = if registered {
            let (agents, profiles) = decode_registered_raw(value)?;
            (
                agents
                    .into_iter()
                    .map(|a| (a.id, a.command, a.envs))
                    .collect::<Vec<_>>(),
                profiles.profiles_by_agent,
            )
        } else {
            let (agents, profiles) = decode_physical_catalog(value)
                .map_err(|_| SourcePreparationError::InvalidCandidate)?;
            (
                agents
                    .into_iter()
                    .map(|a| (a.id, a.command, a.envs))
                    .collect::<Vec<_>>(),
                profiles.profiles_by_agent,
            )
        };
        for (key, command, envs) in agents {
            groups
                .entry(configuration_identity(&command, &envs))
                .or_default()
                .insert(key.clone());
            for (letter, profile) in profiles
                .get(&key)
                .into_iter()
                .flat_map(|cells| cells.iter())
            {
                groups
                    .entry(format!(
                        "{}/{}",
                        configuration_identity(&command, &envs),
                        profile_identity(profile)
                    ))
                    .or_default()
                    .insert(letter.clone());
            }
        }
        Ok(groups)
    };
    let old = groups(before)?;
    let new = groups(after)?;
    if let Some((id, members)) = new.iter().find(|(id, members)| {
        id.contains("/pid1:")
            && members.len() > 1
            && old.get(*id).is_none_or(|prior| !members.is_subset(prior))
    }) {
        let entries = raw_entries(after, if registered { "id" } else { "key" });
        let profile_key = id.rsplit_once("/pid1:").map(|(key, _)| key);
        let incumbent = profile_key.unwrap_or_else(|| {
            old.get(id)
                .and_then(|prior| prior.iter().find(|key| members.contains(*key)))
                .or_else(|| members.first())
                .expect("duplicate group")
                .as_str()
        });
        return Err(SourcePreparationError::Identity(IdentityValidationError {
            code: if profile_key.is_some() {
                IdentityValidationCode::DuplicateProfileIdentity
            } else {
                IdentityValidationCode::DuplicateConfigurationIdentity
            },
            existing_name: entries
                .get(incumbent)
                .and_then(|entry| entry.get("label"))
                .and_then(Value::as_str)
                .unwrap_or(incumbent)
                .into(),
            entry_hint: incumbent.into(),
            requested_profile: profile_key.and_then(|_| members.last().cloned()),
        }));
    }
    Ok(())
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
/// Grouping preserves each original selection's Entry/Profile/Default relation.
/// Only the complete backend inventory factory may construct this aggregate.
#[derive(Debug)]
pub(crate) struct SourceOriginalProofAggregate {
    pub(crate) originals: Vec<LegacyOriginLinkProof>,
}

pub(crate) fn require_source_original_units(
    origin: &LegacyCompatibilityOrigin,
    units: &std::collections::BTreeSet<LegacyOriginUnit>,
    aggregate: Option<&SourceOriginalProofAggregate>,
) -> Result<(), SourcePreparationError> {
    let Some(aggregate) = aggregate else {
        if unresolved_default_dependency(origin, units) {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        return origin.require_disjoint(units.iter().cloned());
    };
    if aggregate
        .originals
        .iter()
        .any(|proof| proof.binding != origin.binding)
    {
        return Err(SourcePreparationError::StaleRevision);
    }
    for unit in units {
        if origin.require_disjoint([unit.clone()]).is_ok() {
            continue;
        }
        let mut covered = false;
        for proof in aggregate
            .originals
            .iter()
            .filter(|proof| proof.units.contains(unit))
        {
            // This model records binding and unit membership only; the per-unit
            // loop rechecks those facts. Same-Default relation coherence is
            // enforced below; distinct default/profile groups remain separate.
            origin.require_legacy_units([unit.clone()], Some(proof))?;
            covered = true;
        }
        if !covered {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
    }
    let entry_or_profile = units.iter().any(|unit| {
        matches!(
            unit,
            LegacyOriginUnit::Entry(_) | LegacyOriginUnit::Profile(_, _)
        )
    });
    if !entry_or_profile {
        return Ok(());
    }
    for default in origin
        .represented
        .iter()
        .filter(|unit| matches!(unit, LegacyOriginUnit::Default(_)))
    {
        if origin.require_disjoint([default.clone()]).is_ok() {
            continue;
        }
        let mut groups = aggregate
            .originals
            .iter()
            .filter(|proof| proof.units.contains(default));
        let group = groups
            .next()
            .ok_or(SourcePreparationError::LegacyOriginUnresolved)?;
        if groups.any(|other| other.units != group.units) {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let keys: Vec<_> = group
            .units
            .iter()
            .filter_map(|unit| match unit {
                LegacyOriginUnit::Entry(key) => Some(key),
                _ => None,
            })
            .collect();
        if keys.len() != 1 {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let key = keys[0];
        let profiles: Vec<_> = group
            .units
            .iter()
            .filter_map(|unit| match unit {
                LegacyOriginUnit::Profile(profile_key, letter) if profile_key == key => {
                    Some(letter)
                }
                _ => None,
            })
            .collect();
        if profiles.len() > 1 {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let changes_profile = units.iter().any(|unit| {
            matches!(unit,
            LegacyOriginUnit::Profile(profile_key, _) if profile_key == key)
        });
        // Missing original selected profile cannot prove disjunction for K/L.
        if changes_profile && profiles.is_empty() {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        let linked_change = units.contains(&LegacyOriginUnit::Entry(key.clone()))
            || profiles.iter().any(|letter| {
                units.contains(&LegacyOriginUnit::Profile(key.clone(), (*letter).clone()))
            })
            || units.contains(default);
        if linked_change {
            origin.require_legacy_units([default.clone()], Some(group))?;
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn prepare_source_candidate(
    snapshot: &RawSourceSnapshot,
    expected: &SourceRevision,
    candidate: &Value,
    origin: Option<&LegacyCompatibilityOrigin>,
) -> Result<PreparedSourceCandidate, SourcePreparationError> {
    prepare_source_candidate_with_original_links(snapshot, expected, candidate, origin, None)
}

pub(crate) fn prepare_source_candidate_with_original_links(
    snapshot: &RawSourceSnapshot,
    expected: &SourceRevision,
    candidate: &Value,
    origin: Option<&LegacyCompatibilityOrigin>,
    proofs: Option<&SourceOriginalProofAggregate>,
) -> Result<PreparedSourceCandidate, SourcePreparationError> {
    if expected != &snapshot.snapshot.revision {
        return Err(SourcePreparationError::StaleRevision);
    }
    if !snapshot.snapshot.capabilities.editable_save
        && !snapshot.snapshot.capabilities.editable_create
    {
        return Err(SourcePreparationError::ReadOnly);
    }
    let registered = snapshot.snapshot.source.kind == SourceKind::RegisteredInstance;
    if registered {
        decode_registered_raw(candidate)?;
    }
    let before = snapshot
        .raw
        .value()
        .or_else(|| snapshot.candidate_template.clone())
        .ok_or(SourcePreparationError::InvalidCandidate)?;
    let mut retained_candidate = candidate.clone();
    retain_configuration_commands(
        &mut retained_candidate,
        &before,
        if registered { "id" } else { "key" },
        !registered,
        true,
        true,
    )
    .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let candidate = &retained_candidate;
    let model = if registered {
        let (agents, profiles) = decode_registered_raw(candidate)?;
        serde_json::json!({"agents":agents, "codingAgentProfiles":profiles})
    } else {
        decode_physical_catalog(candidate).map_err(|_| SourcePreparationError::InvalidCandidate)?;
        candidate.clone()
    };
    // Binding fields are validated and preserved by canonical_profile_source,
    // including unresolved raw legacy entries omitted from the editor's ID view.
    let mut unknown_before = before.clone();
    let binding = if registered {
        unknown_before.get_mut("codingAgentProfiles")
    } else {
        Some(&mut unknown_before)
    };
    if let Some(binding) = binding.and_then(Value::as_object_mut) {
        for key in [
            "profileBindingVersion",
            "profilesByBaseHash",
            "profileLabelsByBaseHash",
            if registered {
                "profilesByAgent"
            } else {
                "codingAgentProfiles"
            },
            if registered {
                "profileLabelsByAgent"
            } else {
                "profileLabels"
            },
        ] {
            binding.remove(key);
        }
        if registered {
            binding.remove("identityByAgent");
        }
    }
    // Catalog slots default to an empty object. Compare an omitted default
    // without discarding any actual before/candidate field or opaque data.
    if !registered
        && before
            .get("profileSlots")
            .and_then(Value::as_object)
            .is_some_and(Map::is_empty)
        && candidate.get("profileSlots").is_none()
    {
        unknown_before
            .as_object_mut()
            .expect("catalog object")
            .remove("profileSlots");
    }
    if !preserve_unknown_fields(&unknown_before, candidate, &model) {
        return Err(SourcePreparationError::InvalidCandidate);
    }
    validate_source_identity_groups(&before, candidate, registered)?;
    if registered {
        let origin = origin.ok_or(SourcePreparationError::OriginUnavailable)?;
        if origin.unknown_paths.iter().any(Vec::is_empty) {
            return Err(SourcePreparationError::OriginUnavailable);
        }
        if origin.binding.registered.is_none()
            || origin.binding.settings.is_none()
            || origin.binding.overlay.is_none()
        {
            return Err(SourcePreparationError::OriginUnavailable);
        }
        if origin.binding.context != snapshot.context
            || origin.binding.registered.as_ref() != Some(expected)
        {
            return Err(SourcePreparationError::StaleRevision);
        }
        let units = changed_origin_units(&before, candidate, origin);
        require_source_original_units(origin, &units, proofs)?;
    }
    let mut candidate = candidate.clone();
    let key_name = if registered { "id" } else { "key" };
    let label_name = "label";
    let incumbents = raw_entries(&before, key_name);
    let mut used = std::collections::BTreeSet::new();
    let mut names = Vec::new();
    if let Some(agents) = candidate.get_mut("agents").and_then(Value::as_array_mut) {
        // Retained unchanged names are the incumbents even when a newly added
        // row appears earlier in the candidate's order.
        for agent in agents.iter() {
            let key = agent
                .get(key_name)
                .and_then(Value::as_str)
                .unwrap_or_default();
            if incumbents.get(key).and_then(|a| a.get(label_name)) == agent.get(label_name) {
                if let Some(label) = agent.get(label_name).and_then(Value::as_str) {
                    used.insert(label.trim().to_lowercase());
                }
            }
        }
        for agent in agents {
            let key = agent
                .get(key_name)
                .and_then(Value::as_str)
                .unwrap_or_default();
            let label = agent
                .get(label_name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if incumbents.get(key).and_then(|a| a.get(label_name)) == agent.get(label_name) {
                continue;
            }
            let mut persisted = label.clone();
            let mut suffix = 1;
            while !used.insert(persisted.trim().to_lowercase()) {
                persisted = format!("{label} ({suffix})");
                suffix += 1;
            }
            if persisted != label {
                names.push((label, persisted.clone()));
                agent
                    .as_object_mut()
                    .ok_or(SourcePreparationError::InvalidCandidate)?
                    .insert(label_name.into(), Value::String(persisted));
            }
        }
    }
    // Naming is also part of the full diff. Recheck it before serialization.
    if registered {
        let origin = origin.ok_or(SourcePreparationError::OriginUnavailable)?;
        let units = changed_origin_units(&before, &candidate, origin);
        require_source_original_units(origin, &units, proofs)?;
    }
    let candidate = canonical_profile_source(&before, &candidate, registered)
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let mut serialized_after = serde_json::to_vec_pretty(&candidate)
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    serialized_after.push(b'\n');
    Ok(PreparedSourceCandidate {
        source: snapshot.snapshot.source.clone(),
        expected_revision: expected.clone(),
        context_revision: snapshot.context.revision.clone(),
        before_bytes: match &snapshot.raw {
            OriginPhysicalInput::Bytes(bytes) => Some(bytes.clone()),
            _ => None,
        },
        serialized_after,
        persisted_names: names,
        origin_binding: origin.map(|origin| origin.binding.clone()),
    })
}

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) const SOURCE_GENERAL_PROTECTED: &[&str] = &[
    "agents",
    "codingAgentProfiles",
    "profileLabels",
    "rootToken",
    "projectPaths",
    "projectPath",
    "archivedProjectPaths",
    "railCollapsedProjects",
    "railFavoritesCollapsed",
    "terminalSnapshotsEnabled",
    "mainGeometry",
    "mainWindowDisplayState",
    "projectPathState",
    "localOverlayState",
];

#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) struct PreparedSettingsWithoutSourceWrite {
    pub(crate) serialized_after: Vec<u8>,
    pub(crate) redecoded_settings: AppSettings,
}

#[allow(dead_code)] // Serde's field table also includes omitted optional fields.
pub(crate) fn source_general_field_names() -> Vec<&'static str> {
    struct Fields<'a>(&'a mut Vec<&'static str>);
    impl<'de> serde::Deserializer<'de> for Fields<'_> {
        type Error = serde::de::value::Error;
        fn deserialize_any<V: serde::de::Visitor<'de>>(
            self,
            _: V,
        ) -> Result<V::Value, Self::Error> {
            Err(serde::de::Error::custom("field table only"))
        }
        fn deserialize_struct<V: serde::de::Visitor<'de>>(
            self,
            _: &'static str,
            fields: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, Self::Error> {
            self.0.extend_from_slice(fields);
            Err(serde::de::Error::custom("field table captured"))
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
            map enum identifier ignored_any
        }
    }
    let mut names = Vec::new();
    let _ = AppSettings::deserialize(Fields(&mut names));
    names
}

/// General-only pure preparation over the captured settings base. Never calls
/// a saver, migration, export, order/adoption/repair helper, or source writer.
#[allow(dead_code)] // Dormant until the later source protocol cuts.
pub(crate) fn prepare_settings_without_source_write(
    inputs: &OriginInputs,
    candidate_general: &Value,
) -> Result<PreparedSettingsWithoutSourceWrite, SourcePreparationError> {
    let origin = LegacyCompatibilityOrigin::classify(inputs);
    let mut out = match &inputs.settings {
        OriginPhysicalInput::Absent => Value::Object(Map::new()),
        OriginPhysicalInput::Bytes(_) => inputs
            .settings
            .value()
            .ok_or(SourcePreparationError::OriginUnavailable)?,
        OriginPhysicalInput::Unknown => return Err(SourcePreparationError::OriginUnavailable),
    };
    // Capture disk restoration before applying the draft. In particular, an
    // overlay-owned general key must retain its raw absent/null/value state,
    // rather than capturing the effective value echoed by the caller.
    let mut restoration_base = out.clone();
    let restoration = match &inputs.overlay {
        OriginPhysicalInput::Bytes(bytes) => LocalSettingsOverlay::from_overlay_bytes_paired(
            &mut restoration_base,
            bytes,
            OVERLAY_INELIGIBLE_DISK_KEYS,
            OVERLAY_INELIGIBLE_LEGACY_KEYS,
            OVERLAY_DERIVED_ID_CLOSURES,
            CONFIGURATION_COMMAND_PAIRS,
        ),
        _ => LocalSettingsOverlay::default(),
    };
    let map = out
        .as_object_mut()
        .ok_or(SourcePreparationError::InvalidCandidate)?;
    let general = candidate_general
        .as_object()
        .ok_or(SourcePreparationError::InvalidCandidate)?;
    let public = serde_json::to_value(AppSettings::default())
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let fields = source_general_field_names();
    if general.keys().any(|key| {
        SOURCE_GENERAL_PROTECTED.contains(&key.as_str()) || !fields.contains(&key.as_str())
    }) {
        return Err(SourcePreparationError::ProtectedSettingsField);
    }
    if public
        .as_object()
        .ok_or(SourcePreparationError::InvalidCandidate)?
        .keys()
        .any(|key| !SOURCE_GENERAL_PROTECTED.contains(&key.as_str()) && !general.contains_key(key))
    {
        return Err(SourcePreparationError::InvalidCandidate);
    }
    // Only caller-owned general keys are updated. Unknown/source/placement keys
    // retain exact raw absence, null or value, even when the typed view differs.
    for key in fields {
        if !SOURCE_GENERAL_PROTECTED.contains(&key) && !general.contains_key(key) {
            map.remove(key);
        }
    }
    for (key, value) in general {
        map.insert(key.clone(), value.clone());
    }
    let mut effective = out.clone();
    if let (Some(dst), Some(raw)) = (
        effective.as_object_mut(),
        inputs
            .registered
            .value()
            .filter(|raw| decode_registered_raw(raw).is_ok())
            .and_then(|v| v.as_object().cloned()),
    ) {
        dst.extend(agents_instance_groups(&raw));
    }
    let unmerged = effective.clone();
    if let OriginPhysicalInput::Bytes(bytes) = &inputs.overlay {
        let overlay = LocalSettingsOverlay::from_overlay_bytes_paired(
            &mut effective,
            bytes,
            OVERLAY_INELIGIBLE_DISK_KEYS,
            OVERLAY_INELIGIBLE_LEGACY_KEYS,
            OVERLAY_DERIVED_ID_CLOSURES,
            CONFIGURATION_COMMAND_PAIRS,
        );
        let mut probe = effective.clone();
        if let Some(root) = probe.as_object_mut() {
            root.entry("agents")
                .or_insert_with(|| Value::Array(Vec::new()));
        }
        if !overlay.is_empty() && serde_json::from_value::<AppSettings>(probe).is_err() {
            effective = unmerged;
        } else {
            restoration.restore_base(out.as_object_mut().expect("object checked"));
        }
    }
    if origin.unknown_paths.iter().any(Vec::is_empty) {
        return Err(SourcePreparationError::OriginUnavailable);
    }
    if let Some(root) = effective.as_object_mut() {
        root.entry("agents")
            .or_insert_with(|| Value::Array(Vec::new()));
    }
    let base = production_instance_base();
    apply_project_decode_to_value(
        &mut effective,
        base.as_deref(),
        &projects::FsCandidateResolver,
    );
    let redecoded_settings =
        serde_json::from_value(effective).map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let mut serialized_after =
        serde_json::to_vec_pretty(&out).map_err(|_| SourcePreparationError::InvalidCandidate)?;
    serialized_after.push(b'\n');
    Ok(PreparedSettingsWithoutSourceWrite {
        serialized_after,
        redecoded_settings,
    })
}
