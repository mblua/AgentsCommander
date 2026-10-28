use super::paths::{
    canonical_existing_dir, same_canonical_path, validate_profile_selection_agent_path,
};
use super::selection_state::ReplicaSelectionPair;
use super::{source_discovery_cut, source_original_slots};
use crate::config::ac_root::{
    ensure_authoritative_ac_root, is_ac_root_name, CANONICAL_AC_ROOT_DIR,
};
use crate::config::settings::{normalize_profile_letter, AppSettings};
use crate::filesystem::coding_agent_profiles::{
    canonical_profile_path, open_profile_directory, profile_path_symlink_metadata,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(crate) struct SourceSelectionTarget {
    pub(crate) root: PathBuf,
    pub(crate) decisions: PathBuf,
    pub(crate) state: PathBuf,
    pub(crate) reservation: PathBuf,
    pub(crate) physical_revision: [crate::config::settings::IdentityReferencePhysicalState; 2],
}

/// One backend-derived locator tuple serves read, inventory and PAIR recovery.
/// No caller supplies a new path when the original operation resumes.
pub(crate) fn read_source_selection_target(
    settings: &AppSettings,
    agent_path: &Path,
) -> Result<SourceSelectionTarget, crate::config::settings::IdentityReferenceError> {
    use crate::config::settings::{read_identity_reference_physical, IdentityReferenceError};
    let validated_launch_path = validate_profile_selection_agent_path(settings, agent_path)
        .map_err(IdentityReferenceError::Preparation)?
        .launch_path;
    let root = canonical_profile_path(&validated_launch_path)
        .map_err(|error| IdentityReferenceError::Preparation(error.to_string()))?;
    let (decisions, state, reservation) =
        crate::config::settings::identity_reference_pair_paths(&root);
    let physical_revision = [
        read_identity_reference_physical(&decisions)?,
        read_identity_reference_physical(&state)?,
    ];
    Ok(SourceSelectionTarget {
        root,
        decisions,
        state,
        reservation,
        physical_revision,
    })
}

/// Strict finite source discovery. The ordinary best-effort helper is unchanged.
#[allow(dead_code)]
pub(crate) fn discover_source_inventory(
    settings: &AppSettings,
) -> Result<
    (
        crate::config::settings::SourceDiscoveryWitness,
        Vec<SourceSelectionTarget>,
    ),
    crate::config::settings::IdentityReferenceError,
> {
    use crate::config::settings::{IdentityReferenceError, SourceDiscoveryWitness};
    let fail = |error: std::io::Error| IdentityReferenceError::Preparation(error.to_string());
    let mut witness = SourceDiscoveryWitness {
        registry: settings.project_paths.clone(),
        parents: Vec::new(),
        slots: Vec::new(),
        membership: Vec::new(),
    };
    let mut candidates = Vec::new();
    for project in &settings.project_paths {
        let base = PathBuf::from(project);
        if !crate::filesystem::projects::project_path_symlink_metadata(&base)
            .map_err(&fail)?
            .is_dir()
        {
            return Err(IdentityReferenceError::Conflict);
        }
        witness.parents.push(base.clone());
        if base
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_ac_root_name)
        {
            candidates.push(base.clone());
        }
        candidates.push(base.join(CANONICAL_AC_ROOT_DIR));
        let mut children = Vec::new();
        crate::filesystem::projects::visit_source_inventory_children(
            &base,
            source_discovery_cut,
            |path, kind| {
                if kind.is_dir() {
                    witness.parents.push(path.clone());
                    children.push(path.clone());
                    candidates.push(path.join(CANONICAL_AC_ROOT_DIR));
                }
            },
        )
        .map_err(&fail)?;
        children.sort();
        witness.membership.push((base, children));
    }
    candidates.sort();
    candidates.dedup();
    let mut ac_roots: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        match profile_path_symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                witness.slots.push((candidate, false))
            }
            Err(error) => return Err(fail(error)),
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(IdentityReferenceError::Conflict);
                }
                ensure_authoritative_ac_root(&candidate)
                    .map_err(IdentityReferenceError::Preparation)?;
                let canonical = canonical_existing_dir(&candidate, "Source AC Root")
                    .map_err(IdentityReferenceError::Preparation)?;
                witness.slots.push((candidate, true));
                if !ac_roots
                    .iter()
                    .any(|root| same_canonical_path(root, &canonical))
                {
                    ac_roots.push(canonical);
                }
            }
        }
    }
    ac_roots.sort();
    let mut targets = Vec::new();
    for root in ac_roots {
        witness.parents.push(root.clone());
        let mut selected = Vec::new();
        source_discovery_cut("readDir").map_err(&fail)?;
        for item in open_profile_directory(&root).map_err(&fail)? {
            source_discovery_cut("entry").map_err(&fail)?;
            let item = item.map_err(&fail)?;
            source_discovery_cut("fileType").map_err(&fail)?;
            let kind = item.file_type().map_err(&fail)?;
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| IdentityReferenceError::Conflict)?;
            if name.starts_with("_agent_") {
                if !kind.is_dir() {
                    return Err(IdentityReferenceError::Conflict);
                }
                selected.push(item.path());
                targets.push(read_source_selection_target(settings, &item.path())?);
            } else if crate::config::entity_prefix::has_entity_prefix(&name) {
                if !kind.is_dir() {
                    return Err(IdentityReferenceError::Conflict);
                }
                selected.push(item.path());
                witness.parents.push(item.path());
                let mut replicas = Vec::new();
                source_discovery_cut("readDir").map_err(&fail)?;
                for replica in open_profile_directory(&item.path()).map_err(&fail)? {
                    source_discovery_cut("entry").map_err(&fail)?;
                    let replica = replica.map_err(&fail)?;
                    source_discovery_cut("fileType").map_err(&fail)?;
                    let kind = replica.file_type().map_err(&fail)?;
                    let name = replica
                        .file_name()
                        .into_string()
                        .map_err(|_| IdentityReferenceError::Conflict)?;
                    if !name.starts_with("__agent_") {
                        continue;
                    }
                    if !kind.is_dir() {
                        return Err(IdentityReferenceError::Conflict);
                    }
                    replicas.push(replica.path());
                    targets.push(read_source_selection_target(settings, &replica.path())?);
                }
                replicas.sort();
                witness.membership.push((item.path(), replicas));
            }
        }
        selected.sort();
        witness.membership.push((root, selected));
    }
    witness
        .parents
        .extend(targets.iter().map(|target| target.root.clone()));
    witness.parents.sort();
    witness.parents.dedup();
    witness.slots.sort();
    witness.membership.sort();
    targets.sort_by(|left, right| left.decisions.cmp(&right.decisions));
    if targets
        .windows(2)
        .any(|pair| same_canonical_path(&pair[0].decisions, &pair[1].decisions))
    {
        return Err(IdentityReferenceError::Conflict);
    }
    Ok((witness, targets))
}

/// Source-aware original linkage is retained independently of the requested diff.
/// Unknown fields remain untouched; inspect the same semantic domains as patch.
#[allow(dead_code)]
pub(crate) fn original_source_selections(
    target: &SourceSelectionTarget,
) -> Result<
    Vec<crate::config::settings::ConfigurationRef>,
    crate::config::settings::IdentityReferenceError,
> {
    Ok(source_original_slots(target)?
        .into_iter()
        .map(|(_, reference)| reference)
        .collect())
}

// #2897 P03: read-only source companions, without enrollment or repair.
#[allow(dead_code)]
pub(crate) fn read_source_selection_state(
    settings: &AppSettings,
    agent_path: &Path,
) -> Result<
    crate::config::settings::SourceSelectionState<
        Option<ReplicaSelectionPair>,
        [crate::config::settings::IdentityReferencePhysicalState; 2],
    >,
    crate::config::settings::IdentityReferenceError,
> {
    use crate::config::settings::{IdentityReferenceError, SourceSelectionState};
    let target = read_source_selection_target(settings, agent_path)?;
    let physical_revision = target.physical_revision;
    let raw = identity_reference_value(&physical_revision[0])?;
    let tooling = raw.get("tooling");
    if tooling.is_some_and(|value| !value.is_object()) {
        return Err(IdentityReferenceError::Conflict);
    }
    let configuration_ref = tooling
        .and_then(|value| value.get("configurationRef"))
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|_| IdentityReferenceError::Conflict)?;
    let legacy_pair = match tooling.and_then(|value| value.get("currentCodingAgent")) {
        None => None,
        Some(Value::String(id)) if !id.trim().is_empty() => {
            let profile = match tooling.and_then(|value| value.get("profile")) {
                None => "A".into(),
                Some(Value::String(profile)) => {
                    normalize_profile_letter(profile).ok_or(IdentityReferenceError::Conflict)?
                }
                _ => return Err(IdentityReferenceError::Conflict),
            };
            Some(ReplicaSelectionPair {
                coding_agent_id: id.clone(),
                requested_profile: profile,
            })
        }
        _ => return Err(IdentityReferenceError::Conflict),
    };
    Ok(SourceSelectionState {
        legacy_pair,
        configuration_ref,
        physical_revision,
    })
}

pub(super) fn identity_reference_value(
    physical: &crate::config::settings::IdentityReferencePhysicalState,
) -> Result<Value, crate::config::settings::IdentityReferenceError> {
    use crate::config::settings::{IdentityReferenceError, IdentityReferencePhysicalState};
    match physical {
        IdentityReferencePhysicalState::Absent => Ok(serde_json::json!({})),
        IdentityReferencePhysicalState::Bytes { bytes, .. } => {
            let value: Value =
                serde_json::from_slice(bytes).map_err(|_| IdentityReferenceError::Conflict)?;
            if !value.is_object()
                || &IdentityReferencePhysicalState::from_bytes(bytes.clone()) != physical
            {
                return Err(IdentityReferenceError::Conflict);
            }
            Ok(value)
        }
    }
}

/// Pure caller images; the durable PAIR packet remains the execution authority.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct IdentityReferencePatch {
    pub(crate) before: [crate::config::settings::IdentityReferencePhysicalState; 2],
    pub(crate) after: [Value; 2],
}

pub(super) fn patch_identity_companion(
    value: &mut Value,
    source: &crate::config::settings::SourceRef,
    mappings: &[crate::config::settings::SourceIdentityMapping],
) -> Result<(), crate::config::settings::IdentityReferenceError> {
    use crate::config::settings::{ConfigurationRef, IdentityReferenceError};
    let Some(raw) = value.get_mut("configurationRef") else {
        return Ok(());
    };
    let mut reference: ConfigurationRef =
        serde_json::from_value(raw.clone()).map_err(|_| IdentityReferenceError::Conflict)?;
    if reference.version != 1 {
        return Err(IdentityReferenceError::Conflict);
    }
    if &reference.source != source {
        return Ok(());
    }
    // The existing local hint narrows an explicit source reference. It never
    // manufactures source authority for a legacy selection or neighboring row.
    let matches: Vec<_> = mappings
        .iter()
        .filter(|mapping| {
            mapping.old_calculated_id.as_ref() == Some(&reference.calculated_id)
                && reference
                    .legacy_entry_hint
                    .as_ref()
                    .is_none_or(|hint| hint == &mapping.entry_hint)
        })
        .collect();
    if matches.len() > 1 {
        return Err(IdentityReferenceError::Conflict);
    }
    let Some(mapping) = matches.first() else {
        return Ok(());
    };
    // Keep removed/disabled references, requested letters and locks unchanged.
    if mapping.unavailable || mapping.new_calculated_id.is_none() {
        return Ok(());
    }
    if let Some(letter) = reference.requested_profile.as_ref() {
        if let Some((before, after)) = mapping.profiles.get(letter) {
            if reference.profile_calculated_id.as_ref() != before.as_ref() {
                return Err(IdentityReferenceError::Conflict);
            }
            let Some(after) = after else {
                return Ok(());
            };
            reference.profile_calculated_id = Some(after.clone());
        }
    }
    reference.calculated_id = mapping
        .new_calculated_id
        .clone()
        .ok_or(IdentityReferenceError::Conflict)?;
    let encoded = serde_json::to_value(reference).map_err(|_| IdentityReferenceError::Conflict)?;
    let object = raw
        .as_object_mut()
        .ok_or(IdentityReferenceError::Conflict)?;
    for key in ["calculatedId", "profileCalculatedId"] {
        match encoded.get(key) {
            Some(value) => {
                object.insert(key.into(), value.clone());
            }
            None => {
                object.remove(key);
            }
        }
    }
    Ok(())
}

/// Prepares exact raw companions in memory, without enrollment or disk writes.
#[allow(dead_code)]
pub(crate) fn plan_identity_reference_patch(
    before: [crate::config::settings::IdentityReferencePhysicalState; 2],
    source: &crate::config::settings::SourceRef,
    mappings: &[crate::config::settings::SourceIdentityMapping],
) -> Result<IdentityReferencePatch, crate::config::settings::IdentityReferenceError> {
    use crate::config::settings::IdentityReferenceError;
    let mut after = [
        identity_reference_value(&before[0])?,
        identity_reference_value(&before[1])?,
    ];
    for (side_index, side) in after.iter_mut().enumerate() {
        let Some(tooling) = side.get_mut("tooling") else {
            continue;
        };
        if !tooling.is_object() {
            return Err(IdentityReferenceError::Conflict);
        }
        if side_index == 0 {
            patch_identity_companion(tooling, source, mappings)?;
            if let Some(default) = tooling.get_mut("replicaSelectionDefault") {
                if !default.is_object() {
                    return Err(IdentityReferenceError::Conflict);
                }
                patch_identity_companion(default, source, mappings)?;
            }
        } else {
            if let Some(history) = tooling.get_mut("codingAgents") {
                let history = history
                    .as_object_mut()
                    .ok_or(IdentityReferenceError::Conflict)?;
                for entry in history.values_mut() {
                    if !entry.is_object() {
                        return Err(IdentityReferenceError::Conflict);
                    }
                    patch_identity_companion(entry, source, mappings)?;
                }
            }
            if let Some(history) = tooling.get_mut("sourceReferences") {
                let history = history
                    .as_array_mut()
                    .ok_or(IdentityReferenceError::Conflict)?;
                for entry in history {
                    if !entry.is_object() {
                        return Err(IdentityReferenceError::Conflict);
                    }
                    patch_identity_companion(entry, source, mappings)?;
                }
            }
        }
    }
    Ok(IdentityReferencePatch { before, after })
}

/// The caller has durably recorded the authorized inventory before this call.
/// PAIR owns cleanup, timestamp protection and the original recovery images.
#[allow(dead_code)]
pub(crate) fn prepare_identity_reference_patch(
    decisions: &Path,
    request: &crate::config::settings::IdentityReferenceRequest,
    source: &crate::config::settings::SourceRef,
    mappings: &[crate::config::settings::SourceIdentityMapping],
    validate: &dyn Fn() -> Result<(), crate::config::settings::IdentityReferenceError>,
) -> Result<
    crate::config::settings::IdentityReferencePacket,
    crate::config::settings::IdentityReferenceError,
> {
    use crate::config::settings::IdentityReferencePhysicalState;
    crate::config::agent_config::prepare_and_reserve_identity_reference_pair_guarded(
        decisions,
        request,
        |decisions, state| {
            // Cleanup has already run under the same PAIR locks. Patch only
            // companions in these maps; never overwrite the loader's cleanup
            // with snapshots taken before lock acquisition.
            let physical = [decisions.clone(), state.clone()].map(|map| {
                serde_json::to_vec(&map).map(IdentityReferencePhysicalState::from_bytes)
            });
            let [decisions_before, state_before] = physical;
            let patch = plan_identity_reference_patch(
                [
                    decisions_before.map_err(|e| e.to_string())?,
                    state_before.map_err(|e| e.to_string())?,
                ],
                source,
                mappings,
            )
            .map_err(|e| e.to_string())?;
            let [decisions_after, state_after] = patch.after;
            *decisions = decisions_after
                .as_object()
                .ok_or("invalid decisions object")?
                .clone();
            *state = state_after
                .as_object()
                .ok_or("invalid state object")?
                .clone();
            Ok(())
        },
        validate,
    )
}

/// Resume executes the original packet; it never prepares new caller images.
/// The pure validation callback borrows existing C/source/target authority.
#[allow(dead_code, clippy::too_many_arguments)]
pub(crate) fn apply_identity_reference_patch(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    plan_digest: &str,
    validate: &dyn Fn() -> Result<(), crate::config::settings::IdentityReferenceError>,
) -> Result<
    [crate::config::settings::IdentityReferencePhysicalState; 2],
    crate::config::settings::IdentityReferenceError,
> {
    use crate::config::settings::{
        execute_identity_reference_pair_guarded, verify_identity_reference_pair_guarded,
    };
    execute_identity_reference_pair_guarded(decisions, state, reservation, operation_id, validate)?;
    verify_identity_reference_pair_guarded(
        decisions,
        state,
        reservation,
        operation_id,
        plan_digest,
        validate,
    )
}
