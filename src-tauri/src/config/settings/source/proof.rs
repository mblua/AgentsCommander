use super::super::IdentityReferenceError;
use super::super::IdentityReferencePhysicalState;
use super::coordination::SourceDiscoveryWitness;
use super::coordination::SourceInventoryTarget;
use super::coordination::SourceLocalInventory;
use super::coordination::SourcePrivateJournal;
use super::identity::source_transition_digest;
use super::identity_reference_protected_revision;
use super::inventory::source_inventory_digest;
use super::inventory::SourceDiscoveredTargetEvidence;
use super::inventory::SourceInventoryCapture;
use super::inventory::SourceInventoryInputs;
use super::inventory::SourceInventoryProof;
use super::inventory::SourceOriginalSlotParser;
use super::read::SourceReadContext;
use super::read_identity_reference_physical;
use super::source_bytes_revision;
use super::SourceCoordinationError;
use super::SourcePreparationError;
use super::SourceRevision;
use std::path::PathBuf;

impl SourceInventoryCapture {
    pub(crate) fn same_original_capture(&self, other: &Self) -> bool {
        self.discovery == other.discovery
            && self.objects == other.objects
            && self.discovered == other.discovered
            && self.topology == other.topology
    }

    pub(crate) fn original_targets(&self) -> &[SourceDiscoveredTargetEvidence] {
        &self.discovered
    }

    pub(crate) fn classify(
        &mut self,
        classifications: Vec<(PathBuf, bool)>,
    ) -> Result<(), SourcePreparationError> {
        if classifications.len() != self.discovered.len() {
            return Err(SourcePreparationError::LegacyOriginUnresolved);
        }
        for (evidence, (target, affected)) in self.discovered.iter_mut().zip(classifications) {
            if evidence.target.decisions != target {
                return Err(SourcePreparationError::LegacyOriginUnresolved);
            }
            evidence.affected = affected;
        }
        self.classified = true;
        Ok(())
    }
}

impl SourceLocalInventory {
    pub(crate) fn validate_original_evidence(&self) -> Result<(), SourceCoordinationError> {
        let Some(original) = &self.original_evidence else {
            return Ok(());
        };
        let invalid = || SourceCoordinationError::Invalid;
        let mut unsigned = self.clone();
        unsigned.authorization_revision.clear();
        if source_transition_digest(&unsigned)? != self.authorization_revision
            || original.objects != self.authorized_roots
            || original.topology.directories != original.objects
            || original.topology.registry != original.discovery.registry
            || original.topology.slots != original.discovery.slots
            || original.topology.membership != original.discovery.membership
            || self.origin_binding.is_none()
        {
            return Err(invalid());
        }
        let before = match &original.source_before {
            IdentityReferencePhysicalState::Absent => SourceRevision::Absent {},
            IdentityReferencePhysicalState::Bytes { bytes, .. } => {
                if IdentityReferencePhysicalState::from_bytes(bytes.clone())
                    != original.source_before
                {
                    return Err(invalid());
                }
                source_bytes_revision(bytes)
            }
        };
        if before != original.expected_source
            || source_inventory_digest(
                &self.editor_id,
                &self.source,
                &self.physical_source_key,
                &self.context_revision,
                &original.topology,
            )? != original.inventory_digest
            || original
                .discovered
                .windows(2)
                .any(|pair| pair[0].target.decisions >= pair[1].target.decisions)
        {
            return Err(invalid());
        }
        let mut selected = Vec::new();
        let mut targets = Vec::new();
        let mut links = Vec::new();
        for evidence in &original.discovered {
            let target = &evidence.target;
            let physical = target.original_physical.as_ref().ok_or_else(invalid)?;
            for image in physical {
                if let IdentityReferencePhysicalState::Bytes { bytes, .. } = image {
                    if &IdentityReferencePhysicalState::from_bytes(bytes.clone()) != image {
                        return Err(invalid());
                    }
                }
            }
            if identity_reference_protected_revision(physical).map_err(|_| invalid())?
                != target.before_protected
                || !original
                    .objects
                    .iter()
                    .any(|(path, object)| path == &target.root && object == &target.root_object)
                || target.original_selections
                    != evidence
                        .slots
                        .iter()
                        .map(|(_, selection)| selection.clone())
                        .collect::<Vec<_>>()
            {
                return Err(invalid());
            }
            targets.push((
                target.root.clone(),
                target.root_object,
                target.decisions.clone(),
                target.state.clone(),
                target.reservation.clone(),
            ));
            for (slot, selection) in &evidence.slots {
                if selection.source == self.source {
                    links.push((
                        target.root.clone(),
                        slot.coordinate(),
                        selection.source.clone(),
                        self.physical_source_key.clone(),
                    ));
                }
            }
            if evidence.affected {
                let mut target = target.clone();
                target.original_physical = None;
                selected.push(target);
            }
        }
        if targets != original.topology.targets
            || links != original.topology.links
            || selected != self.targets
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub(crate) fn verify_original_slots(
        &self,
        parse: &SourceOriginalSlotParser<'_>,
    ) -> Result<(), IdentityReferenceError> {
        let original = self
            .original_evidence
            .as_ref()
            .ok_or(IdentityReferenceError::Conflict)?;
        for evidence in &original.discovered {
            let slots = parse(&evidence.target)?;
            if slots != evidence.slots
                || slots
                    .iter()
                    .map(|(_, reference)| reference.clone())
                    .collect::<Vec<_>>()
                    != evidence.target.original_selections
            {
                return Err(IdentityReferenceError::Conflict);
            }
        }
        Ok(())
    }

    /// Shared three-outcome reader: Ok(true) matches, Ok(false) is definite
    /// protected mismatch, Err is uncertainty/invalid authority, never stale.
    pub(crate) fn affected_original_protected_matches(
        &self,
    ) -> Result<bool, SourceCoordinationError> {
        self.validate_original_evidence()?;
        let original = self
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        let mut matches = true;
        for evidence in original
            .discovered
            .iter()
            .filter(|evidence| evidence.affected)
        {
            let target = &evidence.target;
            let identity = crate::path_identity::verify_directory(&target.root)
                .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
            if (identity.object_id.volume, identity.object_id.file) != target.root_object {
                return Err(SourceCoordinationError::IdentityUncertain);
            }
            let physical = [
                read_identity_reference_physical(&target.decisions)
                    .map_err(|_| SourceCoordinationError::IdentityUncertain)?,
                read_identity_reference_physical(&target.state)
                    .map_err(|_| SourceCoordinationError::IdentityUncertain)?,
            ];
            let protected = identity_reference_protected_revision(&physical)
                .map_err(|_| SourceCoordinationError::IdentityUncertain)?;
            matches &= protected == target.before_protected;
        }
        Ok(matches)
    }
}

impl SourceInventoryProof {
    pub(crate) fn for_resume(
        inputs: SourceInventoryInputs<'_, '_>,
        operation_id: &str,
        journal: &SourcePrivateJournal,
        parse_slots: &SourceOriginalSlotParser<'_>,
    ) -> Result<Self, SourceCoordinationError> {
        let SourceInventoryInputs {
            settings,
            context,
            identity,
            lease,
        } = inputs;
        let inventory = journal
            .inventories
            .get(operation_id)
            .ok_or(SourceCoordinationError::Invalid)?
            .clone();
        // Recompute from original physical images BEFORE restoring any proof.
        inventory
            .verify_original_slots(parse_slots)
            .map_err(|_| SourceCoordinationError::Invalid)?;
        let original = inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if original.discovery.registry != settings.project_paths
            || SourceReadContext::from_settings(settings, &context.settings_path)
                .map_err(|_| SourceCoordinationError::Unauthorized)?
                != *context
        {
            return Err(SourceCoordinationError::Unauthorized);
        }
        let mut unsigned = inventory.clone();
        unsigned.authorization_revision.clear();
        if source_transition_digest(&unsigned)? != inventory.authorization_revision {
            return Err(SourceCoordinationError::Invalid);
        }
        let intent = journal
            .intents
            .get(operation_id)
            .ok_or(SourceCoordinationError::Invalid)?;
        if intent.editor_id != identity.instance_id()
            || intent.authorization_revision != inventory.authorization_revision
            || intent.expected_revision != original.expected_source
            || source_bytes_revision(&intent.serialized_after) != original.after_source
            || intent.source != inventory.source
            || intent.mappings != inventory.mappings
        {
            return Err(SourceCoordinationError::Invalid);
        }
        let proof = Self {
            operation_id: operation_id.into(),
            inventory,
        };
        proof.inventory_digest()?;
        proof
            .validate(identity, lease, context)
            .map_err(|_| SourceCoordinationError::Stale)?;
        proof.restore_original_links(context)?;
        Ok(proof)
    }

    /// Replay the original pure patch over every discovered before image.
    /// The callback is supplied by the orchestration owner, outside PAIR guards.
    pub(crate) fn verify_original_classification(
        &self,
        classify: &dyn Fn(&SourceInventoryTarget) -> Result<bool, IdentityReferenceError>,
    ) -> Result<(), IdentityReferenceError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(IdentityReferenceError::Conflict)?;
        let mut selected = Vec::new();
        for evidence in &original.discovered {
            if classify(&evidence.target)? != evidence.affected {
                return Err(IdentityReferenceError::Conflict);
            }
            if evidence.affected {
                let mut target = evidence.target.clone();
                target.original_physical = None;
                selected.push(target);
            }
        }
        if selected != self.inventory.targets {
            return Err(IdentityReferenceError::Conflict);
        }
        Ok(())
    }

    /// Fence-only invariant; later PAIR progress legitimately changes revisions.
    pub(crate) fn verify_affected_before_fence(&self) -> Result<(), SourceCoordinationError> {
        if self.inventory.affected_original_protected_matches()? {
            Ok(())
        } else {
            Err(SourceCoordinationError::Stale)
        }
    }

    pub(crate) fn verify_complete_discovery(
        &self,
        witness: &SourceDiscoveryWitness,
        current: &[SourceDiscoveredTargetEvidence],
    ) -> Result<(), SourceCoordinationError> {
        let original = self
            .inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if witness != &original.discovery || current.len() != original.discovered.len() {
            return Err(SourceCoordinationError::Stale);
        }
        let mut links = Vec::new();
        for (now_evidence, before_evidence) in current.iter().zip(&original.discovered) {
            if !before_evidence.affected
                && now_evidence.target.before_protected != before_evidence.target.before_protected
            {
                return Err(SourceCoordinationError::Stale);
            }
        }
        for (now, before) in current.iter().zip(&original.discovered) {
            let now = &now.target;
            let before = &before.target;
            if now.root != before.root
                || now.root_object != before.root_object
                || now.decisions != before.decisions
                || now.state != before.state
                || now.reservation != before.reservation
            {
                return Err(SourceCoordinationError::Stale);
            }
        }
        for evidence in current {
            for (slot, selection) in &evidence.slots {
                if selection.source == self.inventory.source {
                    links.push((
                        evidence.target.root.clone(),
                        slot.coordinate(),
                        selection.source.clone(),
                        self.inventory.physical_source_key.clone(),
                    ));
                }
            }
        }
        let mut topology = original.topology.clone();
        topology.links = links;
        if source_inventory_digest(
            &self.inventory.editor_id,
            &self.inventory.source,
            &self.inventory.physical_source_key,
            &self.inventory.context_revision,
            &topology,
        )? != original.inventory_digest
        {
            return Err(SourceCoordinationError::Stale);
        }
        Ok(())
    }
}
