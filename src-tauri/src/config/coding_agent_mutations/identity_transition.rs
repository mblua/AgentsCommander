use crate::config::settings::AppSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum IdentityTransitionReadState {
    NotFound,
    IntentStored,
    Prepared,
    RefPending,
    Conflict,
    Complete,
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct IdentityTransitionRead {
    pub(crate) state: IdentityTransitionReadState,
    pub(crate) operation_id: Option<String>,
    pub(crate) status: crate::config::settings::SourceTransitionStatus,
    pub(crate) active: bool,
}

/// Read-only stable observation; never consolidates an equivalent source after.
#[allow(dead_code)]
pub(crate) fn get_identity_transition(
    context: &crate::config::settings::SourceReadContext,
    source: &crate::config::settings::SourceRef,
    operation_id: &str,
) -> Result<IdentityTransitionRead, crate::config::settings::SourceCoordinationError> {
    use crate::config::settings::*;
    let status = read_source_transition(context, source, operation_id)?;
    let state = match &status.summary {
        None => IdentityTransitionReadState::NotFound,
        Some(summary) => {
            let matches = match summary.phase {
                SourceTransitionPhase::IntentStored | SourceTransitionPhase::Prepared => {
                    status.source_revision_observed == summary.expected_source_revision
                        || status.source_revision_observed == summary.after_source_revision
                }
                SourceTransitionPhase::SourcePublished | SourceTransitionPhase::Complete => {
                    status.source_revision_observed == summary.after_source_revision
                }
            };
            if !matches {
                IdentityTransitionReadState::Conflict
            } else {
                match summary.phase {
                    SourceTransitionPhase::IntentStored => {
                        IdentityTransitionReadState::IntentStored
                    }
                    SourceTransitionPhase::Prepared => IdentityTransitionReadState::Prepared,
                    SourceTransitionPhase::SourcePublished => {
                        IdentityTransitionReadState::RefPending
                    }
                    SourceTransitionPhase::Complete => IdentityTransitionReadState::Complete,
                }
            }
        }
    };
    Ok(IdentityTransitionRead {
        state,
        operation_id: status.summary.as_ref().map(|v| v.operation_id.clone()),
        status,
        active: source_protocol_active(),
    })
}

#[cfg(test)]
thread_local! {
    pub(super) static SOURCE_TRANSITION_UUIDS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) type SourceDurableCutState = (Option<(&'static str, usize)>, Vec<&'static str>);

#[cfg(test)]
thread_local! {
    pub(super) static SOURCE_DURABLE_CUT: std::cell::RefCell<SourceDurableCutState> =
        const { std::cell::RefCell::new((None, Vec::new())) };
}

/// Fail-only seam after real durability, outside every publisher/PAIR guard.
fn source_durable_boundary(
    _stage: &'static str,
) -> Result<(), crate::config::settings::SourceCoordinationError> {
    #[cfg(test)]
    return SOURCE_DURABLE_CUT.with(|state| {
        let mut state = state.borrow_mut();
        state.1.push(_stage);
        if let Some((stage, ordinal)) = state.0 {
            if stage == _stage && state.1.iter().filter(|seen| **seen == stage).count() == ordinal {
                state.0 = None;
                return Err(crate::config::settings::SourceCoordinationError::Io(
                    format!("durable cut: {_stage}"),
                ));
            }
        }
        Ok(())
    });
    #[cfg(not(test))]
    Ok(())
}

#[cfg(test)]
pub(super) fn configure_source_durable_cut(cut: Option<(&'static str, usize)>) {
    SOURCE_DURABLE_CUT.with(|state| *state.borrow_mut() = (cut, Vec::new()));
}

#[cfg(test)]
thread_local! {
    // Test-only pause outside validation: an external writer changes a real
    // protected target between production's initial check and fence effect.
    pub(super) static SOURCE_FENCE_EFFECT_BARRIER:
        std::cell::RefCell<Option<std::sync::Arc<std::sync::Barrier>>> =
        const { std::cell::RefCell::new(None) };
}

fn source_before_fence_boundary() -> Result<(), crate::config::settings::SourceCoordinationError> {
    source_durable_boundary("beforeFence")?;
    #[cfg(test)]
    if let Some(barrier) = SOURCE_FENCE_EFFECT_BARRIER.with(|slot| slot.borrow_mut().take()) {
        barrier.wait();
        barrier.wait();
    }
    Ok(())
}

pub(super) fn new_source_transition_id() -> String {
    #[cfg(test)]
    SOURCE_TRANSITION_UUIDS.with(|count| count.set(count.get() + 1));
    uuid::Uuid::new_v4().to_string()
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct SourceResumeExpectation {
    pub(crate) source: crate::config::settings::SourceRevision,
    pub(crate) journal: crate::config::settings::SourceRevision,
}

/// Inactive gate precedes candidate parsing, UUID, write leases and sidecars.
/// Reload is synchronous backend-local work only. It must not wait for IPC or
/// frontend acknowledgement, await, dispatch a worker, or reacquire coordination
/// leases. Any future activated caller must be reviewed against this contract.
#[allow(dead_code)]
pub(crate) fn commit_source_candidate(
    settings: &AppSettings,
    context: &crate::config::settings::SourceReadContext,
    source: &crate::config::settings::SourceRef,
    request_id: &str,
    expected: &crate::config::settings::SourceRevision,
    candidate: &serde_json::Value,
    reload: &dyn Fn() -> Result<(), crate::config::settings::SourceCoordinationError>,
) -> Result<IdentityTransitionRead, SourceTransitionError> {
    if !crate::config::settings::source_protocol_active() {
        return Err(SourceTransitionError::ActivationRequired);
    }
    commit_source_candidate_internal(
        settings, context, source, request_id, expected, candidate, reload,
    )
}

#[allow(dead_code)]
pub(super) fn commit_source_candidate_internal(
    settings: &AppSettings,
    context: &crate::config::settings::SourceReadContext,
    source: &crate::config::settings::SourceRef,
    request_id: &str,
    expected: &crate::config::settings::SourceRevision,
    candidate: &serde_json::Value,
    reload: &dyn Fn() -> Result<(), crate::config::settings::SourceCoordinationError>,
) -> Result<IdentityTransitionRead, SourceTransitionError> {
    use crate::config::settings::*;
    let identity = IdentityWriteLease::acquire(context, std::time::Duration::ZERO)?;
    let lease = SourceWriteContext::acquire(context, source, &identity)?;
    let payload_digest = source_transition_digest(&(source, expected, candidate))?;
    let (local_revision, journal) = read_own_source_journal(context)?;
    // Request lookup precedes UUID creation and every effect.
    if let Some(original) = journal
        .intents
        .values()
        .find(|intent| intent.request_id == request_id)
    {
        if original.source != *source || original.payload_digest != payload_digest {
            return Err(SourceCoordinationError::Invalid.into());
        }
        return Ok(get_identity_transition(
            context,
            source,
            &original.operation_id,
        )?);
    }
    // One incomplete operation fences this physical source, including source drift.
    if let Some((id, _)) = journal.inventories.iter().find(|(id, inventory)| {
        inventory.physical_source_key == lease.physical_source_key()
            && journal
                .progress
                .get(*id)
                .is_none_or(|p| p.receipt.is_none())
    }) {
        return Err(SourceTransitionError::Interrupted {
            operation_id: id.clone(),
            cause: Box::new(SourceCoordinationError::Pending.into()),
        });
    }
    let snapshot = read_source_snapshot(context, source)?;
    let origin_inputs = OriginInputs::capture(context)?;
    let origin = LegacyCompatibilityOrigin::classify(&origin_inputs);
    let mut capture =
        capture_source_inventory(settings, &snapshot, &origin, lease.physical_source_key())?;
    let prepared = prepare_source_candidate_with_original_links(
        &snapshot,
        expected,
        candidate,
        Some(&origin),
        Some(capture.original_links()),
    )?;
    let mappings = source_original_mappings(&snapshot, &prepared)?;
    let mut classifications = Vec::new();
    for evidence in capture.original_targets() {
        let target = &evidence.target;
        let original = crate::config::coding_agent_profiles::SourceSelectionTarget {
            root: target.root.clone(),
            decisions: target.decisions.clone(),
            state: target.state.clone(),
            reservation: target.reservation.clone(),
            physical_revision: target
                .original_physical
                .clone()
                .ok_or(SourceCoordinationError::Invalid)?,
        };
        let affected = crate::config::coding_agent_profiles::source_target_is_affected(
            &original, source, &mappings,
        )?;
        classifications.push((target.decisions.clone(), affected));
    }
    capture.classify(classifications)?;
    prepared.revalidate_before_write(context)?;
    let operation_id = new_source_transition_id();
    // settings alone constructs original authorization, mapping and binding.
    let authority = SourceInventoryProof::for_candidate(
        crate::config::settings::SourceInventoryInputs {
            settings,
            context,
            identity: &identity,
            lease: &lease,
        },
        &snapshot,
        &prepared,
        &origin_inputs,
        capture,
        &operation_id,
    )?;
    let (witness, targets) = source_discovery_evidence(settings)?;
    authority.verify_complete_discovery(&witness, &targets)?;
    authority.verify_original_classification(&|target| {
        let original = crate::config::coding_agent_profiles::SourceSelectionTarget {
            root: target.root.clone(),
            decisions: target.decisions.clone(),
            state: target.state.clone(),
            reservation: target.reservation.clone(),
            physical_revision: target
                .original_physical
                .clone()
                .ok_or(IdentityReferenceError::Conflict)?,
        };
        crate::config::coding_agent_profiles::source_target_is_affected(
            &original,
            source,
            &authority.inventory().mappings,
        )
    })?;
    let rediscover = || -> Result<(), SourceTransitionError> {
        let (witness, targets) = source_discovery_evidence(settings)?;
        authority.verify_complete_discovery(&witness, &targets)?;
        Ok(())
    };
    let validate = || authority.validate(&identity, &lease, context);
    source_durable_boundary("beforeIntent")?;
    let intent = authority.stored_intent(&prepared, &operation_id, request_id, &payload_digest)?;
    let journal_publication = update_own_source_journal_with_validation(
        &identity,
        &lease,
        context,
        &local_revision,
        |journal| {
            journal.intents.insert(operation_id.clone(), intent.clone());
            journal
                .inventories
                .insert(operation_id.clone(), authority.inventory().clone());
            journal
                .progress
                .insert(operation_id.clone(), SourceLocalProgress::default());
            Ok(())
        },
        &validate,
    );
    if let Err(error) = journal_publication {
        return match read_own_source_journal(context) {
            Ok((_, observed))
                if observed.intents.get(&operation_id) == Some(&intent)
                    && observed.inventories.get(&operation_id) == Some(authority.inventory()) =>
            {
                Err(SourceTransitionError::Interrupted {
                    operation_id,
                    cause: Box::new(error.into()),
                })
            }
            Ok(_) => Err(error.into()),
            Err(uncertain) => Err(uncertain.into()),
        };
    }
    let continuation = (|| -> Result<IdentityTransitionRead, SourceTransitionError> {
        source_durable_boundary("intentInventory")?;
        rediscover()?;
        authority.verify_affected_before_fence()?;
        source_before_fence_boundary()?;
        validate()?;
        authority.verify_affected_before_fence()?;
        drive_identity_transition(
            &identity,
            &lease,
            context,
            &operation_id,
            &authority,
            &rediscover,
            reload,
        )
    })();
    continuation.map_err(|error| SourceTransitionError::Interrupted {
        operation_id,
        cause: Box::new(error),
    })
}

/// Explicit recovery accepts only operation identity and physical CAS from a
/// fresh observation. It never receives new paths, candidate bytes or roles.
/// Reload obeys the same synchronous backend-local, nonblocking contract as Save;
/// the Fn signature itself does not prove that an arbitrary callback is safe.
#[allow(dead_code)]
pub(crate) fn resume_identity_transition(
    settings: &AppSettings,
    context: &crate::config::settings::SourceReadContext,
    source: &crate::config::settings::SourceRef,
    operation_id: &str,
    expected: &SourceResumeExpectation,
    reload: &dyn Fn() -> Result<(), crate::config::settings::SourceCoordinationError>,
) -> Result<IdentityTransitionRead, SourceTransitionError> {
    if !crate::config::settings::source_protocol_active() {
        return Err(SourceTransitionError::ActivationRequired);
    }
    resume_identity_transition_internal(settings, context, source, operation_id, expected, reload)
}

#[allow(dead_code)]
pub(super) fn resume_identity_transition_internal(
    settings: &AppSettings,
    context: &crate::config::settings::SourceReadContext,
    source: &crate::config::settings::SourceRef,
    operation_id: &str,
    expected: &SourceResumeExpectation,
    reload: &dyn Fn() -> Result<(), crate::config::settings::SourceCoordinationError>,
) -> Result<IdentityTransitionRead, SourceTransitionError> {
    use crate::config::settings::*;
    let identity = IdentityWriteLease::acquire(context, std::time::Duration::ZERO)?;
    let lease = SourceWriteContext::acquire(context, source, &identity)?;
    let (local_revision, journal) = read_own_source_journal(context)?;
    if expected.source != lease.source_revision()? || expected.journal != local_revision {
        return Err(SourceCoordinationError::Stale.into());
    }
    if !journal.intents.contains_key(operation_id) {
        return Err(SourceCoordinationError::NotFound.into());
    }
    let authority = SourceInventoryProof::for_resume(
        SourceInventoryInputs {
            settings,
            context,
            identity: &identity,
            lease: &lease,
        },
        operation_id,
        &journal,
        &source_original_inventory_slots,
    )?;
    authority.verify_original_classification(&|target| {
        let original = crate::config::coding_agent_profiles::SourceSelectionTarget {
            root: target.root.clone(),
            decisions: target.decisions.clone(),
            state: target.state.clone(),
            reservation: target.reservation.clone(),
            physical_revision: target
                .original_physical
                .clone()
                .ok_or(IdentityReferenceError::Conflict)?,
        };
        crate::config::coding_agent_profiles::source_target_is_affected(
            &original,
            source,
            &authority.inventory().mappings,
        )
    })?;
    let rediscover = || -> Result<(), SourceTransitionError> {
        let (witness, targets) = source_discovery_evidence(settings)?;
        authority.verify_complete_discovery(&witness, &targets)?;
        Ok(())
    };
    drive_identity_transition(
        &identity,
        &lease,
        context,
        operation_id,
        &authority,
        &rediscover,
        reload,
    )
    .map_err(|error| SourceTransitionError::Interrupted {
        operation_id: operation_id.into(),
        cause: Box::new(error),
    })
}

pub(super) fn source_original_inventory_slots(
    target: &crate::config::settings::SourceInventoryTarget,
) -> Result<
    Vec<(
        crate::config::settings::SourceOriginalSlot,
        crate::config::settings::ConfigurationRef,
    )>,
    crate::config::settings::IdentityReferenceError,
> {
    let original = crate::config::coding_agent_profiles::SourceSelectionTarget {
        root: target.root.clone(),
        decisions: target.decisions.clone(),
        state: target.state.clone(),
        reservation: target.reservation.clone(),
        physical_revision: target
            .original_physical
            .clone()
            .ok_or(crate::config::settings::IdentityReferenceError::Conflict)?,
    };
    crate::config::coding_agent_profiles::source_original_slots(&original)
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub(crate) enum SourceTransitionError {
    #[error("sourceActivationRequired")]
    ActivationRequired,
    #[error("sourceTransitionPending: {operation_id}: {cause}")]
    Interrupted {
        operation_id: String,
        cause: Box<SourceTransitionError>,
    },
    #[error(transparent)]
    Coordination(#[from] crate::config::settings::SourceCoordinationError),
    #[error(transparent)]
    Pair(#[from] crate::config::settings::IdentityReferenceError),
    #[error(transparent)]
    Preparation(#[from] crate::config::settings::SourcePreparationError),
}

/// Progress never contains replacement images. PAIR remains recovery authority.
/// Called only with settings-issued original inventory authority under C/source.
#[allow(dead_code)]
pub(super) fn drive_identity_transition(
    identity: &crate::config::settings::IdentityWriteLease,
    lease: &crate::config::settings::SourceWriteContext,
    context: &crate::config::settings::SourceReadContext,
    operation_id: &str,
    authority: &crate::config::settings::SourceInventoryProof,
    rediscover: &dyn Fn() -> Result<(), SourceTransitionError>,
    reload: &dyn Fn() -> Result<(), crate::config::settings::SourceCoordinationError>,
) -> Result<IdentityTransitionRead, SourceTransitionError> {
    use crate::config::coding_agent_profiles::{
        apply_identity_reference_patch, prepare_identity_reference_patch,
    };
    use crate::config::settings::*;
    let validate = || authority.validate(identity, lease, context);
    rediscover()?;
    validate()?;
    let inventory = authority.inventory();
    let (_, journal) = read_own_source_journal(context)?;
    let intent = journal
        .intents
        .get(operation_id)
        .ok_or(SourceCoordinationError::Invalid)?
        .clone();
    if journal.inventories.get(operation_id) != Some(inventory)
        || inventory.editor_id != identity.instance_id()
        || inventory.source != *lease.source()
        || inventory.physical_source_key != lease.physical_source_key()
        || inventory.context_revision != lease.context_revision()
        || inventory.mappings != intent.mappings
    {
        return Err(SourceCoordinationError::Unauthorized.into());
    }
    authority.inventory_digest()?;
    let after = source_transition_digest_bytes(&intent.serialized_after);
    let mut progress = journal
        .progress
        .get(operation_id)
        .ok_or(SourceCoordinationError::Invalid)?
        .clone();
    let persist =
        |next: SourceLocalProgress| -> Result<SourceLocalProgress, SourceTransitionError> {
            let (revision, original) = read_own_source_journal(context)?;
            let prior = original
                .progress
                .get(operation_id)
                .ok_or(SourceCoordinationError::Invalid)?
                .clone();
            update_own_source_journal_with_validation(
                identity,
                lease,
                context,
                &revision,
                |journal| {
                    if journal.progress.get(operation_id) != Some(&prior) {
                        return Err(SourceCoordinationError::Stale);
                    }
                    journal.progress.insert(operation_id.into(), next.clone());
                    Ok(())
                },
                &validate,
            )?;
            let (_, current) = read_own_source_journal(context)?;
            let confirmed = current
                .progress
                .get(operation_id)
                .ok_or(SourceCoordinationError::Invalid)?;
            if confirmed != &next {
                return Err(SourceCoordinationError::Stale.into());
            }
            Ok(confirmed.clone())
        };
    if progress.source_published && lease.source_revision()? != after {
        return Err(SourceCoordinationError::Stale.into());
    }
    if !progress.packets_prepared {
        source_before_fence_boundary()?;
        authority.verify_affected_before_fence()?;
        let mappings_digest = source_transition_digest(&inventory.mappings)?;
        for target in &inventory.targets {
            rediscover()?;
            validate()?;
            let indexed = progress
                .packets
                .iter()
                .find(|row| row.target == target.decisions);
            let packet = match recover_source_target_packet(
                target,
                operation_id,
                lease.physical_source_key(),
                &mappings_digest,
                &validate,
            )? {
                Some(packet) => packet,
                None if indexed.is_some() => return Err(IdentityReferenceError::Conflict.into()),
                None => prepare_identity_reference_patch(
                    &target.decisions,
                    &IdentityReferenceRequest {
                        operation_id: operation_id.into(),
                        source_physical_key: lease.physical_source_key().into(),
                        mappings_digest: mappings_digest.clone(),
                        before_protected: target.before_protected.clone(),
                    },
                    &inventory.source,
                    &inventory.mappings,
                    &validate,
                )?,
            };
            source_durable_boundary("packet")?;
            let hashes = packet.protected_revision_digests()?;
            let record = SourcePacketRecord {
                target: target.decisions.clone(),
                target_id: packet.physical_target_identity.clone(),
                before_protected_revision: SourceRevision::Bytes {
                    sha256: hashes[0].clone(),
                },
                after_protected_revision: SourceRevision::Bytes {
                    sha256: hashes[1].clone(),
                },
                plan_digest: packet.plan_digest.clone(),
            };
            if let Some(indexed) = indexed {
                if indexed != &record {
                    return Err(IdentityReferenceError::Conflict.into());
                }
            } else {
                progress.packets.push(record);
                progress = persist(progress)?;
            }
        }
        source_durable_boundary("allPackets")?;
        progress.packets_prepared = true;
        progress = persist(progress)?;
    }
    source_durable_boundary("prepared")?;
    // Verify every ORIGINAL packet before recognizing/publishing source.
    if progress.receipt.is_none() {
        let mappings_digest = source_transition_digest(&inventory.mappings)?;
        for row in &progress.packets {
            let target = inventory
                .targets
                .iter()
                .find(|t| t.decisions == row.target)
                .ok_or(SourceCoordinationError::Invalid)?;
            let packet = recover_source_target_packet(
                target,
                operation_id,
                lease.physical_source_key(),
                &mappings_digest,
                &validate,
            )?
            .ok_or(IdentityReferenceError::Conflict)?;
            let hashes = packet.protected_revision_digests()?;
            if packet.plan_digest != row.plan_digest
                || packet.physical_target_identity != row.target_id
                || row.before_protected_revision
                    != (SourceRevision::Bytes {
                        sha256: hashes[0].clone(),
                    })
                || row.after_protected_revision
                    != (SourceRevision::Bytes {
                        sha256: hashes[1].clone(),
                    })
            {
                return Err(IdentityReferenceError::Conflict.into());
            }
        }
    }
    if !progress.source_published {
        rediscover()?;
        let publication =
            publish_source_intent_with_validation(lease, context, &intent, &validate)?;
        if publication.revision != after {
            return Err(SourceCoordinationError::Stale.into());
        }
        let (_, current) = read_own_source_journal(context)?;
        progress = current
            .progress
            .get(operation_id)
            .ok_or(SourceCoordinationError::Invalid)?
            .clone();
        source_durable_boundary("sourceAfter")?;
        validate()?;
        if lease.source_revision()? != after {
            return Err(SourceCoordinationError::Stale.into());
        }
        progress.source_published = true;
        progress = persist(progress)?;
    }
    source_durable_boundary("sourcePublished")?;
    rediscover()?;
    for row in &progress.packets {
        let target = inventory
            .targets
            .iter()
            .find(|t| t.decisions == row.target)
            .ok_or(SourceCoordinationError::Invalid)?;
        if progress.receipt.is_some() {
            verify_completed_source_target(target, row, &validate)?;
        } else {
            apply_identity_reference_patch(
                &target.decisions,
                &target.state,
                &target.reservation,
                operation_id,
                &row.plan_digest,
                &validate,
            )?;
        }
        source_durable_boundary("ref")?;
    }
    if !progress.refs_verified {
        progress.refs_verified = true;
        progress = persist(progress)?;
    }
    source_durable_boundary("refsVerified")?;
    validate()?;
    reload()?;
    validate()?;
    if !progress.reloaded {
        progress.reloaded = true;
        progress = persist(progress)?;
    }
    source_durable_boundary("reloaded")?;
    rediscover()?;
    if lease.source_revision()? != after {
        return Err(SourceCoordinationError::Stale.into());
    }
    if progress.receipt.is_none() {
        // Final target verification precedes the sole complete receipt.
        for row in &progress.packets {
            let target = inventory
                .targets
                .iter()
                .find(|t| t.decisions == row.target)
                .ok_or(SourceCoordinationError::Invalid)?;
            verify_completed_source_target(target, row, &validate)?;
        }
        progress.receipt = Some(SourceReceipt {
            operation_id: operation_id.into(),
            source_revision: after,
        });
        progress = persist(progress)?;
    }
    source_durable_boundary("receipt")?;
    for row in &progress.packets {
        rediscover()?;
        let target = inventory
            .targets
            .iter()
            .find(|t| t.decisions == row.target)
            .ok_or(SourceCoordinationError::Invalid)?;
        verify_completed_source_target(target, row, &validate)?;
        let proof = source_complete_proof(lease, context, operation_id, &row.target_id, &validate)?;
        release_identity_reference_pair_guarded(
            &target.decisions,
            &target.state,
            &target.reservation,
            &proof,
            &validate,
        )?;
        source_durable_boundary("release")?;
    }
    Ok(get_identity_transition(
        context,
        lease.source(),
        operation_id,
    )?)
}

pub(super) fn source_discovery_evidence(
    settings: &AppSettings,
) -> Result<
    (
        crate::config::settings::SourceDiscoveryWitness,
        Vec<crate::config::settings::SourceDiscoveredTargetEvidence>,
    ),
    SourceTransitionError,
> {
    use crate::config::coding_agent_profiles::{discover_source_inventory, source_original_slots};
    use crate::config::settings::{source_inventory_target, SourceDiscoveredTargetEvidence};
    let (witness, targets) = discover_source_inventory(settings)?;
    let mut evidence = Vec::new();
    for target in targets {
        let slots = source_original_slots(&target)?;
        let inventory = source_inventory_target(
            target.root,
            target.decisions,
            target.state,
            target.reservation,
            target.physical_revision,
            slots
                .iter()
                .map(|(_, reference)| reference.clone())
                .collect(),
        )?;
        evidence.push(SourceDiscoveredTargetEvidence {
            target: inventory,
            slots,
            affected: false,
        });
    }
    Ok((witness, evidence))
}

pub(super) fn capture_source_inventory(
    settings: &AppSettings,
    snapshot: &crate::config::settings::RawSourceSnapshot,
    origin: &crate::config::settings::LegacyCompatibilityOrigin,
    physical_source_key: &str,
) -> Result<crate::config::settings::SourceInventoryCapture, SourceTransitionError> {
    use crate::config::settings::SourceInventoryCapture;
    for _ in 0..=2 {
        let (first_witness, first_targets) = source_discovery_evidence(settings)?;
        let first = SourceInventoryCapture::capture(
            first_witness.clone(),
            first_targets.clone(),
            snapshot,
            origin,
            physical_source_key,
        )?;
        let (second_witness, second_targets) = source_discovery_evidence(settings)?;
        let second = SourceInventoryCapture::capture(
            second_witness.clone(),
            second_targets.clone(),
            snapshot,
            origin,
            physical_source_key,
        )?;
        if first_witness == second_witness
            && first_targets == second_targets
            && first.same_original_capture(&second)
        {
            return Ok(first);
        }
    }
    Err(crate::config::settings::SourcePreparationError::StaleRevision.into())
}
