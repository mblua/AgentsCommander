use super::super::raw_configuration_command;
use super::super::AppSettings;
use super::super::CodingAgentEnv;
use super::super::IdentityReferenceError;
use super::super::IdentityReferencePacket;
use super::super::IdentityReferencePhysicalState;
use super::coordination::read_own_source_journal;
use super::coordination::IdentityWriteLease;
use super::coordination::SourceDiscoveryWitness;
use super::coordination::SourceIdentityMapping;
use super::coordination::SourceInventoryTarget;
use super::coordination::SourceLocalInventory;
use super::coordination::SourcePacketRecord;
use super::coordination::SourceStoredIntent;
use super::coordination::SourceWriteContext;
use super::identity::configuration_identity;
use super::identity::profile_identity;
use super::identity::source_transition_digest;
use super::identity_reference_protected_revision;
use super::prepare::PreparedSourceCandidate;
use super::prepare::SourceOriginalProofAggregate;
use super::read::decode_registered_raw;
use super::read::LegacyCompatibilityOrigin;
use super::read::LegacyOriginUnit;
use super::read::OriginInputs;
use super::read::RawSourceSnapshot;
use super::read::SourceReadContext;
use super::read_identity_reference_physical;
use super::source_bytes_revision;
#[cfg(test)]
use super::source_factory_guard_cut;
use super::verify_identity_reference_pair_guarded;
use super::ConfigurationRef;
use super::SourceCoordinationError;
use super::SourceFamily;
use super::SourceKind;
use super::SourceOriginalInventoryEvidence;
use super::SourceOriginalSlot;
use super::SourcePreparationError;
use super::SourceRef;
use super::SourceRevision;
use super::SourceRole;
use super::SourceScope;
use crate::filesystem::settings_sources;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Validate the retained original packet with existing guarded PAIR verification.
/// Pending is admissible here only after the primitive validates its reachable
/// stage; it never licenses preparation of a replacement packet.
#[allow(dead_code)]
pub(crate) fn recover_source_target_packet(
    target: &SourceInventoryTarget,
    operation_id: &str,
    source_key: &str,
    mappings_digest: &str,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<Option<IdentityReferencePacket>, IdentityReferenceError> {
    for _ in 0..=2 {
        validate()?;
        let physical = read_identity_reference_physical(&target.reservation)?;
        let bytes = match &physical {
            IdentityReferencePhysicalState::Absent => return Ok(None),
            IdentityReferencePhysicalState::Bytes { bytes, .. } => bytes,
        };
        let packet: IdentityReferencePacket =
            serde_json::from_slice(bytes).map_err(|_| IdentityReferenceError::Conflict)?;
        let value: Value =
            serde_json::from_slice(bytes).map_err(|_| IdentityReferenceError::Conflict)?;
        if packet.operation_id != operation_id
            || packet.source_physical_key != source_key
            || value.get("mappingsDigest").and_then(Value::as_str) != Some(mappings_digest)
        {
            return Err(IdentityReferenceError::Conflict);
        }
        let before_bytes = serde_json::to_vec(&target.before_protected)
            .map_err(|error| IdentityReferenceError::Io(error.to_string()))?;
        let before = source_bytes_revision(&before_bytes);
        let hashes = packet.protected_revision_digests()?;
        if before
            != (SourceRevision::Bytes {
                sha256: hashes[0].clone(),
            })
        {
            return Err(IdentityReferenceError::Conflict);
        }
        match verify_identity_reference_pair_guarded(
            &target.decisions,
            &target.state,
            &target.reservation,
            operation_id,
            &packet.plan_digest,
            validate,
        ) {
            Ok(_) | Err(IdentityReferenceError::Pending) => {}
            Err(error) => return Err(error),
        }
        validate()?;
        if read_identity_reference_physical(&target.reservation)? == physical {
            return Ok(Some(packet));
        }
    }
    Err(IdentityReferenceError::Conflict)
}

#[allow(dead_code)]
pub(crate) fn verify_completed_source_target(
    target: &SourceInventoryTarget,
    commitment: &SourcePacketRecord,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<(), IdentityReferenceError> {
    validate()?;
    let original_before = serde_json::to_vec(&target.before_protected)
        .map_err(|error| IdentityReferenceError::Io(error.to_string()))?;
    if source_bytes_revision(&original_before) != commitment.before_protected_revision {
        return Err(IdentityReferenceError::Conflict);
    }
    let before = [
        read_identity_reference_physical(&target.decisions)?,
        read_identity_reference_physical(&target.state)?,
    ];
    let protected = identity_reference_protected_revision(&before)?;
    let digest =
        source_transition_digest(&protected).map_err(|_| IdentityReferenceError::Conflict)?;
    if commitment.after_protected_revision != (SourceRevision::Bytes { sha256: digest }) {
        return Err(IdentityReferenceError::Conflict);
    }
    validate()?;
    if before
        != [
            read_identity_reference_physical(&target.decisions)?,
            read_identity_reference_physical(&target.state)?,
        ]
    {
        return Err(IdentityReferenceError::Conflict);
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn source_inventory_target(
    root: PathBuf,
    decisions: PathBuf,
    state: PathBuf,
    reservation: PathBuf,
    physical: [IdentityReferencePhysicalState; 2],
    originals: Vec<ConfigurationRef>,
) -> Result<SourceInventoryTarget, IdentityReferenceError> {
    let identity = crate::path_identity::verify_directory(&root)
        .map_err(IdentityReferenceError::Preparation)?;
    if identity.canonical_path != root
        || decisions.parent() != Some(root.as_path())
        || state.parent() != Some(root.as_path())
        || reservation.parent() != Some(root.as_path())
    {
        return Err(IdentityReferenceError::Conflict);
    }
    let before_protected = identity_reference_protected_revision(&physical)?;
    Ok(SourceInventoryTarget {
        root,
        root_object: (identity.object_id.volume, identity.object_id.file),
        decisions,
        state,
        reservation,
        before_protected,
        original_selections: originals,
        original_physical: Some(physical),
    })
}

pub(crate) fn source_original_mappings(
    snapshot: &RawSourceSnapshot,
    prepared: &PreparedSourceCandidate,
) -> Result<Vec<SourceIdentityMapping>, SourcePreparationError> {
    let registered = snapshot.snapshot.source.kind == SourceKind::RegisteredInstance;
    let after: Value = serde_json::from_slice(&prepared.serialized_after)
        .map_err(|_| SourcePreparationError::InvalidCandidate)?;
    let (after_agents, after_profiles) = if registered {
        let (agents, profiles) = decode_registered_raw(&after)?;
        (
            agents
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| SourcePreparationError::InvalidCandidate)?,
            profiles.profiles_by_agent,
        )
    } else {
        let catalog =
            crate::config::coding_agents_catalog::parse_catalog_schema2(&prepared.serialized_after)
                .map_err(|_| SourcePreparationError::InvalidCandidate)?;
        (
            catalog
                .agents
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| SourcePreparationError::InvalidCandidate)?,
            catalog.coding_agent_profiles,
        )
    };
    let key_field = if registered { "id" } else { "key" };
    let old: BTreeMap<_, _> = snapshot
        .snapshot
        .agents
        .iter()
        .map(|agent| {
            agent
                .get(key_field)
                .and_then(Value::as_str)
                .map(|key| (key.to_owned(), agent))
                .ok_or(SourcePreparationError::InvalidCandidate)
        })
        .collect::<Result<_, _>>()?;
    let new: BTreeMap<_, _> = after_agents
        .iter()
        .map(|agent| {
            agent
                .get(key_field)
                .and_then(Value::as_str)
                .map(|key| (key.to_owned(), agent))
                .ok_or(SourcePreparationError::InvalidCandidate)
        })
        .collect::<Result<_, _>>()?;
    let calculated = |value: &&Value| -> Result<String, SourcePreparationError> {
        let command = raw_configuration_command(
            value
                .as_object()
                .ok_or(SourcePreparationError::InvalidCandidate)?,
            "baseCommand",
        )
        .map_err(|_| SourcePreparationError::InvalidCandidate)?
        .ok_or(SourcePreparationError::InvalidCandidate)?;
        let envs: Vec<CodingAgentEnv> = value
            .get("envs")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| SourcePreparationError::InvalidCandidate)?
            .unwrap_or_default();
        Ok(configuration_identity(command, &envs))
    };
    let keys: std::collections::BTreeSet<_> = old.keys().chain(new.keys()).cloned().collect();
    let mut mappings = Vec::new();
    for key in keys {
        let old_calculated_id = old.get(&key).map(&calculated).transpose()?;
        let new_calculated_id = new.get(&key).map(&calculated).transpose()?;
        let old_cells = snapshot.snapshot.profiles.get(&key);
        let new_cells = after_profiles.get(&key);
        let letters: std::collections::BTreeSet<_> = old_cells
            .into_iter()
            .flat_map(|cells| cells.keys())
            .chain(new_cells.into_iter().flat_map(|cells| cells.keys()))
            .cloned()
            .collect();
        let profiles: BTreeMap<_, _> = letters
            .into_iter()
            .filter_map(|letter| {
                let before = old_cells
                    .and_then(|cells| cells.get(&letter))
                    .map(profile_identity);
                let after = new_cells
                    .and_then(|cells| cells.get(&letter))
                    .map(profile_identity);
                (before != after).then_some((letter, (before, after)))
            })
            .collect();
        if old_calculated_id != new_calculated_id || !profiles.is_empty() {
            let unavailable = new_calculated_id.is_none()
                || !registered
                    && crate::config::coding_agents_catalog::BUILTIN_AGENT_SUPPORT
                        .iter()
                        .any(|(builtin, supported)| *builtin == key && !supported);
            mappings.push(SourceIdentityMapping {
                entry_hint: key,
                old_calculated_id,
                new_calculated_id,
                profiles,
                unavailable,
            });
        }
    }
    Ok(mappings)
}

pub(crate) type SourceMembershipTarget = (PathBuf, (u64, u64), PathBuf, PathBuf, PathBuf);

/// Private, backend-validated topology; never a deserialized authority token.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceMembershipTopology {
    pub(crate) registry: Vec<String>,
    pub(crate) directories: Vec<(PathBuf, (u64, u64))>,
    pub(crate) slots: Vec<(PathBuf, bool)>,
    pub(crate) membership: Vec<(PathBuf, Vec<PathBuf>)>,
    pub(crate) targets: Vec<SourceMembershipTarget>,
    // Slot coordinate is structural: mutable selection values are excluded.
    pub(crate) links: Vec<(PathBuf, String, SourceRef, String)>,
}

pub(crate) fn source_ref_tuple(
    source: &SourceRef,
) -> (SourceFamily, SourceRole, SourceScope, SourceKind, &str) {
    (
        source.family,
        source.role,
        source.scope,
        source.kind,
        &source.context_root,
    )
}

pub(crate) fn source_inventory_projection(
    editor: &str,
    source: &SourceRef,
    physical_source_key: &str,
    context_revision: &str,
    original: &SourceMembershipTopology,
) -> Result<Vec<u8>, SourceCoordinationError> {
    if editor.is_empty() || physical_source_key.is_empty() || context_revision.is_empty() {
        return Err(SourceCoordinationError::Invalid);
    }
    // Registry order is policy-significant. Everything below is set-valued.
    let mut topology = original.clone();
    topology.directories.sort();
    topology.directories.dedup();
    topology.slots.sort();
    topology.slots.dedup();
    for (_, members) in &mut topology.membership {
        members.sort();
        members.dedup();
    }
    topology.membership.sort();
    topology.membership.dedup();
    topology.targets.sort();
    topology.targets.dedup();
    for pair in topology.directories.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for pair in topology.slots.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for pair in topology.membership.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for pair in topology.targets.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    let mut links = topology
        .links
        .iter()
        .map(|(root, slot, linked, key)| {
            if slot.is_empty() || linked != source || key != physical_source_key {
                return Err(SourceCoordinationError::Invalid);
            }
            // Encode enum spellings via serde, not Debug or platform formatting.
            serde_json::to_vec(&(root, slot, source_ref_tuple(linked), key))
                .map(|bytes| ((root.clone(), slot.clone()), bytes))
                .map_err(|_| SourceCoordinationError::Invalid)
        })
        .collect::<Result<Vec<_>, _>>()?;
    links.sort();
    links.dedup();
    for pair in links.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    let links = links
        .into_iter()
        .map(|(_, bytes)| {
            serde_json::from_slice::<Value>(&bytes).map_err(|_| SourceCoordinationError::Invalid)
        })
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_vec(&(
        "p03-member-inventory-v1",
        editor,
        1u32,
        source_ref_tuple(source),
        physical_source_key,
        context_revision,
        topology.registry,
        topology.directories,
        topology.slots,
        topology.membership,
        topology.targets,
        links,
    ))
    .map_err(|_| SourceCoordinationError::Invalid)
}

pub(crate) fn source_inventory_digest(
    participant: &str,
    source: &SourceRef,
    physical_source_key: &str,
    context_revision: &str,
    topology: &SourceMembershipTopology,
) -> Result<String, SourceCoordinationError> {
    let bytes = source_inventory_projection(
        participant,
        source,
        physical_source_key,
        context_revision,
        topology,
    )?;
    match source_bytes_revision(&bytes) {
        SourceRevision::Bytes { sha256 } => Ok(sha256),
        SourceRevision::Absent {} => Err(SourceCoordinationError::Invalid),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceDiscoveredTargetEvidence {
    pub(crate) target: SourceInventoryTarget,
    pub(crate) slots: Vec<(SourceOriginalSlot, ConfigurationRef)>,
    pub(crate) affected: bool,
}

pub(crate) struct SourceInventoryCapture {
    pub(crate) discovery: SourceDiscoveryWitness,
    pub(crate) objects: Vec<(PathBuf, (u64, u64))>,
    pub(crate) discovered: Vec<SourceDiscoveredTargetEvidence>,
    pub(crate) topology: SourceMembershipTopology,
    pub(crate) original_links: SourceOriginalProofAggregate,
    pub(crate) classified: bool,
}

impl SourceInventoryCapture {
    pub(crate) fn capture(
        discovery: SourceDiscoveryWitness,
        mut discovered: Vec<SourceDiscoveredTargetEvidence>,
        snapshot: &RawSourceSnapshot,
        origin: &LegacyCompatibilityOrigin,
        physical_source_key: &str,
    ) -> Result<Self, SourcePreparationError> {
        let invalid = || SourcePreparationError::LegacyOriginUnresolved;
        let mut paths = discovery.parents.clone();
        for (root, members) in &discovery.membership {
            paths.push(root.clone());
            paths.extend(members.iter().cloned());
        }
        paths.extend(
            discovered
                .iter()
                .map(|evidence| evidence.target.root.clone()),
        );
        paths.sort();
        paths.dedup();
        let mut objects = Vec::new();
        for path in paths {
            let verified = crate::path_identity::verify_directory(&path).map_err(|_| invalid())?;
            objects.push((
                verified.canonical_path,
                (verified.object_id.volume, verified.object_id.file),
            ));
        }
        objects.sort();
        objects.dedup();
        if objects.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(invalid());
        }
        for (slot, present) in &discovery.slots {
            if *present {
                let verified =
                    crate::path_identity::verify_directory(slot).map_err(|_| invalid())?;
                if !objects.iter().any(|(path, object)| {
                    path == &verified.canonical_path
                        && *object == (verified.object_id.volume, verified.object_id.file)
                }) {
                    return Err(invalid());
                }
            } else {
                let parent = slot.parent().ok_or_else(invalid)?;
                crate::path_identity::verify_directory(parent).map_err(|_| invalid())?;
                match settings_sources::probe(slot) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return Err(invalid()),
                }
            }
        }
        discovered.sort_by(|a, b| a.target.decisions.cmp(&b.target.decisions));
        if discovered
            .windows(2)
            .any(|pair| pair[0].target.root == pair[1].target.root)
        {
            return Err(invalid());
        }
        let mut links = Vec::new();
        let mut originals = Vec::new();
        let mut defaults: BTreeMap<
            String,
            (PathBuf, std::collections::BTreeSet<LegacyOriginUnit>),
        > = BTreeMap::new();
        for evidence in &discovered {
            let target = &evidence.target;
            let physical = target.original_physical.as_ref().ok_or_else(invalid)?;
            if identity_reference_protected_revision(physical).map_err(|_| invalid())?
                != target.before_protected
                || !objects
                    .iter()
                    .any(|(root, object)| root == &target.root && object == &target.root_object)
            {
                return Err(invalid());
            }
            for (slot, selection) in &evidence.slots {
                if selection.version != 1 {
                    return Err(invalid());
                }
                if matches!(
                    slot,
                    SourceOriginalSlot::MatrixDefault | SourceOriginalSlot::ReplicaDefault
                ) {
                    let name = target
                        .root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(invalid)?;
                    let matrix = name.starts_with("_agent_");
                    let replica = name.starts_with("__agent_");
                    if matrix != matches!(slot, SourceOriginalSlot::MatrixDefault)
                        || replica != matches!(slot, SourceOriginalSlot::ReplicaDefault)
                    {
                        return Err(invalid());
                    }
                }
                let side = match slot {
                    SourceOriginalSlot::Decision
                    | SourceOriginalSlot::MatrixDefault
                    | SourceOriginalSlot::ReplicaDefault => 0,
                    _ => 1,
                };
                let bytes = match &physical[side] {
                    IdentityReferencePhysicalState::Bytes { bytes, .. } => bytes,
                    _ => return Err(invalid()),
                };
                let raw: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
                let stored: ConfigurationRef =
                    serde_json::from_value(slot.stored(&raw).cloned().ok_or_else(invalid)?)
                        .map_err(|_| invalid())?;
                if stored != *selection {
                    return Err(invalid());
                }
                if selection.source != snapshot.snapshot.source {
                    continue;
                }
                links.push((
                    target.root.clone(),
                    slot.coordinate(),
                    selection.source.clone(),
                    physical_source_key.into(),
                ));
                if snapshot.snapshot.source.kind != SourceKind::RegisteredInstance {
                    continue;
                }
                let matrix = if matches!(slot, SourceOriginalSlot::MatrixDefault) {
                    // Only a real origin Matrix can own N. Replica/history never mint Default.
                    let name = target
                        .root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(invalid)?;
                    Some(
                        name.strip_prefix("_agent_")
                            .filter(|name| !name.is_empty())
                            .ok_or_else(invalid)?,
                    )
                } else {
                    None
                };
                let proof = origin.bind_original_selection_at(
                    snapshot,
                    selection,
                    &snapshot.snapshot.revision,
                    &snapshot.snapshot.context_revision,
                    bytes,
                    bytes,
                    matrix,
                    slot,
                )?;
                if let Some(matrix) = matrix {
                    let provenance = (target.root.clone(), proof.units.clone());
                    if defaults
                        .get(matrix)
                        .is_some_and(|original| original != &provenance)
                    {
                        return Err(invalid());
                    }
                    defaults.insert(matrix.into(), provenance);
                }
                originals.push(proof);
            }
        }
        let topology = SourceMembershipTopology {
            registry: discovery.registry.clone(),
            directories: objects.clone(),
            slots: discovery.slots.clone(),
            membership: discovery.membership.clone(),
            targets: discovered
                .iter()
                .map(|e| {
                    let t = &e.target;
                    (
                        t.root.clone(),
                        t.root_object,
                        t.decisions.clone(),
                        t.state.clone(),
                        t.reservation.clone(),
                    )
                })
                .collect(),
            links,
        };
        Ok(Self {
            discovery,
            objects,
            discovered,
            topology,
            original_links: SourceOriginalProofAggregate { originals },
            classified: false,
        })
    }

    pub(crate) fn original_links(&self) -> &SourceOriginalProofAggregate {
        &self.original_links
    }
}

pub(crate) type SourceOriginalSlotParser<'a> = dyn Fn(
        &SourceInventoryTarget,
    ) -> Result<Vec<(SourceOriginalSlot, ConfigurationRef)>, IdentityReferenceError>
    + 'a;

pub(crate) struct SourceInventoryInputs<'borrow, 'identity> {
    pub(crate) settings: &'borrow AppSettings,
    pub(crate) context: &'borrow SourceReadContext,
    pub(crate) identity: &'borrow IdentityWriteLease,
    pub(crate) lease: &'borrow SourceWriteContext<'identity>,
}

/// Backend-only original operation authority. No retained discovery handles.
pub(crate) struct SourceInventoryProof {
    pub(crate) operation_id: String,
    pub(crate) inventory: SourceLocalInventory,
}

impl SourceInventoryProof {
    pub(crate) fn for_candidate(
        inputs: SourceInventoryInputs<'_, '_>,
        snapshot: &RawSourceSnapshot,
        prepared: &PreparedSourceCandidate,
        origin: &OriginInputs,
        capture: SourceInventoryCapture,
        operation_id: &str,
    ) -> Result<Self, SourcePreparationError> {
        let SourceInventoryInputs {
            settings,
            context,
            identity,
            lease,
        } = inputs;
        if !capture.classified
            || SourceReadContext::from_settings(settings, &context.settings_path)? != *context
            || capture.discovery.registry != settings.project_paths
            || prepared.source != snapshot.snapshot.source
            || prepared.source != *lease.source()
        {
            return Err(SourcePreparationError::ContextChanged);
        }
        prepared.revalidate_before_write(context)?;
        origin.revalidate(context)?;
        let inventory_digest = source_inventory_digest(
            identity.instance_id(),
            lease.source(),
            lease.physical_source_key(),
            lease.context_revision(),
            &capture.topology,
        )
        .map_err(|_| SourcePreparationError::ContextChanged)?;
        let mappings = source_original_mappings(snapshot, prepared)?;
        let source_before = match &prepared.before_bytes {
            Some(bytes) => IdentityReferencePhysicalState::from_bytes(bytes.clone()),
            None => IdentityReferencePhysicalState::Absent,
        };
        let evidence = SourceOriginalInventoryEvidence {
            discovery: capture.discovery,
            objects: capture.objects.clone(),
            source_before,
            expected_source: prepared.expected_revision.clone(),
            after_source: source_bytes_revision(&prepared.serialized_after),
            discovered: capture.discovered.clone(),
            topology: capture.topology,
            inventory_digest,
        };
        let mut targets = Vec::new();
        for classified in &capture.discovered {
            if classified.affected {
                let mut target = classified.target.clone();
                // Original images have one durable location, in full evidence.
                target.original_physical = None;
                targets.push(target);
            }
        }
        let mut inventory = SourceLocalInventory {
            editor_id: identity.instance_id().into(),
            source: prepared.source.clone(),
            physical_source_key: lease.physical_source_key().into(),
            context_revision: context.revision.clone(),
            authorization_revision: String::new(),
            authorized_roots: capture.objects,
            targets,
            mappings,
            origin_binding: Some(origin.binding.clone()),
            original_evidence: Some(evidence),
        };
        inventory.authorization_revision = source_transition_digest(&inventory)
            .map_err(|_| SourcePreparationError::ContextChanged)?;
        let proof = Self {
            operation_id: operation_id.into(),
            inventory,
        };
        proof
            .validate(identity, lease, context)
            .map_err(|_| SourcePreparationError::ContextChanged)?;
        Ok(proof)
    }

    pub(crate) fn inventory(&self) -> &SourceLocalInventory {
        &self.inventory
    }

    pub(crate) fn inventory_digest(&self) -> Result<&str, SourceCoordinationError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        let actual = source_inventory_digest(
            &self.inventory.editor_id,
            &self.inventory.source,
            &self.inventory.physical_source_key,
            &self.inventory.context_revision,
            &original.topology,
        )?;
        if actual != original.inventory_digest {
            return Err(SourceCoordinationError::Invalid);
        }
        Ok(&original.inventory_digest)
    }

    pub(crate) fn stored_intent(
        &self,
        prepared: &PreparedSourceCandidate,
        operation_id: &str,
        request_id: &str,
        payload_digest: &str,
    ) -> Result<SourceStoredIntent, SourceCoordinationError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if operation_id != self.operation_id
            || prepared.source != self.inventory.source
            || prepared.expected_revision != original.expected_source
            || source_bytes_revision(&prepared.serialized_after) != original.after_source
        {
            return Err(SourceCoordinationError::Invalid);
        }
        Ok(SourceStoredIntent {
            schema_version: 2,
            operation_id: operation_id.into(),
            request_id: request_id.into(),
            payload_digest: payload_digest.into(),
            source: self.inventory.source.clone(),
            expected_revision: prepared.expected_revision.clone(),
            before_bytes: prepared.before_bytes.clone(),
            serialized_after: prepared.serialized_after.clone(),
            mappings: self.inventory.mappings.clone(),
            editor_id: self.inventory.editor_id.clone(),
            context_revision: self.inventory.context_revision.clone(),
            authorization_revision: self.inventory.authorization_revision.clone(),
        })
    }

    /// Pure segment guard: directory/object and original origin checks only.
    /// Full enumeration is orchestrated outside PAIR by mutations.
    pub(crate) fn validate(
        &self,
        identity: &IdentityWriteLease,
        lease: &SourceWriteContext,
        context: &SourceReadContext,
    ) -> Result<(), IdentityReferenceError> {
        #[cfg(test)]
        source_factory_guard_cut()?;
        let conflict = || IdentityReferenceError::Conflict;
        let inventory = &self.inventory;
        let original = inventory.original_evidence.as_ref().ok_or_else(conflict)?;
        if identity.instance_id() != inventory.editor_id
            || lease.source() != &inventory.source
            || lease.physical_source_key() != inventory.physical_source_key
            || context.revision != inventory.context_revision
        {
            return Err(conflict());
        }
        lease.revalidate(context).map_err(|_| conflict())?;
        for (path, object) in &original.objects {
            let current = crate::path_identity::verify_directory(path).map_err(|_| conflict())?;
            if current.canonical_path != *path
                || (current.object_id.volume, current.object_id.file) != *object
            {
                return Err(conflict());
            }
        }
        for (slot, present) in &original.discovery.slots {
            let exists = match settings_sources::probe(slot) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                _ => return Err(conflict()),
            };
            if exists != *present {
                return Err(conflict());
            }
        }
        let observed = OriginInputs::read(context);
        let binding = inventory.origin_binding.as_ref().ok_or_else(conflict)?;
        if observed.binding.context != binding.context
            || observed.binding.settings != binding.settings
            || observed.binding.overlay != binding.overlay
        {
            return Err(conflict());
        }
        let current_source = lease.source_revision().map_err(|_| conflict())?;
        if current_source != original.expected_source && current_source != original.after_source {
            return Err(conflict());
        }
        if current_source != original.expected_source {
            let (_, journal) = read_own_source_journal(context).map_err(|_| conflict())?;
            let intent = journal
                .intents
                .get(&self.operation_id)
                .ok_or_else(conflict)?;
            let progress = journal
                .progress
                .get(&self.operation_id)
                .ok_or_else(conflict)?;
            if !progress.packets_prepared
                || intent.source != inventory.source
                || intent.expected_revision != original.expected_source
                || source_bytes_revision(&intent.serialized_after) != original.after_source
                || intent.mappings != inventory.mappings
                || intent.editor_id != identity.instance_id()
            {
                return Err(conflict());
            }
        }
        if observed.binding.registered != binding.registered {
            if inventory.source.kind != SourceKind::RegisteredInstance
                || current_source != original.after_source
                || observed.binding.registered.as_ref() != Some(&original.after_source)
            {
                return Err(conflict());
            }
            let (_, journal) = read_own_source_journal(context).map_err(|_| conflict())?;
            let intent = journal
                .intents
                .get(&self.operation_id)
                .ok_or_else(conflict)?;
            if intent.source != inventory.source
                || intent.expected_revision != original.expected_source
                || source_bytes_revision(&intent.serialized_after) != original.after_source
                || intent.mappings != inventory.mappings
                || intent.editor_id != identity.instance_id()
            {
                return Err(conflict());
            }
        }
        Ok(())
    }
}
