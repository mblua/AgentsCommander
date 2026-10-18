//! #786 - shared op layer for scriptable Coding Agent configuration.
//!
//! Single entry point (`apply_coding_agent_op`) used by BOTH the direct CLI
//! write path (GUI closed) and the daemon-routed poller (GUI running), so the
//! two paths behave identically by construction. The op layer is PURE: it
//! mutates an in-memory `AppSettings` and runs the shared validator suite, but
//! it NEVER touches disk. Callers own persistence (save + in-memory swap).
//!
//! The NEW rules this feature adds (agent id format/uniqueness, color hex,
//! non-empty label) live ONLY here, never in `validate_and_repair_settings`, so
//! legacy settings.json files with arbitrary ids/colors keep loading and saving.
//! Command/env/instructions/config-seed validation is delegated to the existing
//! `validate_and_repair_settings` -> `validate_agent_commands` suite that every
//! other writer already runs.
//!
//! Queue transport (daemon path): the CLI drops a `CodingAgentRequest` JSON into
//! `<config_dir>/coding-agent-requests/`; the `MailboxPoller` claims it with an
//! atomic rename to `.processing` (#786 R2, closes the cancel false-negative
//! race), applies it, writes a `CodingAgentResult` into `results/`, and deletes
//! the `.processing` file LAST. The per-request handler is a pure,
//! path-parameterized free function so it unit-tests against a tempdir with no
//! Tauri handle (modeled on `mailbox::collect_project_refresh_requests`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::settings::{
    finalize_agent_order, normalize_agent_order, normalize_container_image_input,
    validate_and_repair_settings, AgentConfig, AppSettings, CodingAgentEnv, ConfigSeedConfig,
};
use crate::pty::backend::SessionBackendKind;

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
    static SOURCE_TRANSITION_UUIDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
type SourceDurableCutState = (Option<(&'static str, usize)>, Vec<&'static str>);

#[cfg(test)]
thread_local! {
    static SOURCE_DURABLE_CUT: std::cell::RefCell<SourceDurableCutState> = const { std::cell::RefCell::new((None, Vec::new())) };
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
fn configure_source_durable_cut(cut: Option<(&'static str, usize)>) {
    SOURCE_DURABLE_CUT.with(|state| *state.borrow_mut() = (cut, Vec::new()));
}

#[cfg(test)]
thread_local! {
    // Test-only pause outside validation: an external writer changes a real
    // protected target between production's initial check and fence effect.
    static SOURCE_FENCE_EFFECT_BARRIER: std::cell::RefCell<Option<std::sync::Arc<std::sync::Barrier>>> = const { std::cell::RefCell::new(None) };
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

fn new_source_transition_id() -> String {
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
fn commit_source_candidate_internal(
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
fn resume_identity_transition_internal(
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

fn source_original_inventory_slots(
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
fn drive_identity_transition(
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

/// Subdirectory of the config dir holding pending coding-agent mutation requests.
pub const CODING_AGENT_REQUESTS_DIR: &str =
    crate::config::instance_artifacts::CODING_AGENT_REQUESTS_DIR_NAME;
/// Results subdirectory (under the requests dir) the CLI polls for outcomes.
pub const RESULTS_SUBDIR: &str = "results";
/// A request older than this (poller `now` minus the CLI `createdAtMs`) is
/// rejected as expired, so a stale request cannot apply on a much later GUI boot.
pub const REQUEST_EXPIRY_MS: u64 = 300_000;
/// Default custom-agent color when none is supplied (matches OnboardingModal).
pub const DEFAULT_CUSTOM_COLOR: &str = "#6366f1";

/// A settings.agents mutation. Serialized (camelCase, internally tagged on
/// `kind`) into the request file: `{ "kind": "add", "agent": { ... } }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CodingAgentOp {
    /// Add a new agent. `id` is already minted and syntactically valid CLI-side.
    Add { agent: AgentConfig },
    /// Patch selected fields of the agent with this exact `id`.
    Update { id: String, patch: AgentPatch },
    /// Remove the agent with this exact `id`.
    Remove { id: String },
}

/// Partial update for `coding-agent update`. Every field is optional; only the
/// provided ones change. `envs: Some(vec)` replaces the WHOLE env list
/// (`Some(empty)` clears it, matching `update_coding_agent_env_settings`).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "baseCommand")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envs: Option<Vec<CodingAgentEnv>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolated_home: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_filename: Option<String>,
    #[serde(default)]
    pub clear_instructions_filename: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed_dest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed_enabled: Option<bool>,
    #[serde(default)]
    pub clear_config_seed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_kind: Option<SessionBackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_image: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_container_image: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentPatchWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::config::settings::deserialize_present_command"
    )]
    pub command: Option<String>,
    #[serde(
        default,
        rename = "baseCommand",
        deserialize_with = "crate::config::settings::deserialize_present_command"
    )]
    pub canonical_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envs: Option<Vec<CodingAgentEnv>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolated_home: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_filename: Option<String>,
    #[serde(default)]
    pub clear_instructions_filename: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed_dest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_seed_enabled: Option<bool>,
    #[serde(default)]
    pub clear_config_seed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_kind: Option<SessionBackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_image: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_container_image: bool,
}

impl<'de> Deserialize<'de> for AgentPatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AgentPatchWire::deserialize(deserializer)?;
        let command = crate::config::settings::select_configuration_command(
            wire.canonical_command.as_deref(),
            wire.command.as_deref(),
        )
        .map(str::to_owned);
        Ok(Self {
            label: wire.label,
            command,
            color: wire.color,
            envs: wire.envs,
            isolated_home: wire.isolated_home,
            instructions_filename: wire.instructions_filename,
            clear_instructions_filename: wire.clear_instructions_filename,
            config_seed_dest: wire.config_seed_dest,
            config_seed_enabled: wire.config_seed_enabled,
            clear_config_seed: wire.clear_config_seed,
            backend_kind: wire.backend_kind,
            container_image: wire.container_image,
            clear_container_image: wire.clear_container_image,
        })
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Result of a successful `apply_coding_agent_op`. `op` is `"add"|"update"|"remove"`.
#[derive(Debug, Clone)]
pub struct CodingAgentOpOutcome {
    pub op: &'static str,
    pub agent: Option<AgentConfig>,
    pub removed_id: Option<String>,
}

/// The request file the CLI drops for the poller (daemon path).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentRequest {
    pub request_id: String,
    pub created_at_ms: u64,
    pub op: CodingAgentOp,
}

/// The result file the poller writes and the CLI polls for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAgentResult {
    pub request_id: String,
    pub ok: bool,
    /// #786: `"add"|"update"|"remove"` on success. Mirrored into the daemon-path
    /// stdout so the JSON shape matches the direct (GUI-closed) path exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_id: Option<String>,
}

/// Disposition of a single processed request, returned by the pure handler so
/// the poller knows whether to swap in-memory state and emit an event.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestDisposition {
    /// The request file was gone before we could claim it (a timed-out CLI
    /// cancelled it). Nothing applied, no result written; caller must NOT swap.
    Skipped,
    /// A result file was written but settings were NOT mutated/persisted
    /// (expired, malformed, validation error, or save failure). Do NOT swap.
    Rejected,
    /// Settings were mutated AND saved; a success result was written. The caller
    /// SHOULD swap in-memory state and emit `coding_agent_settings_updated`.
    Applied {
        op: &'static str,
        agent_id: Option<String>,
    },
}

/// Validate a user-supplied custom agent id: `^[a-z0-9][a-z0-9_-]{0,63}$`.
/// Deliberately distinct from (not a superset of) the catalog key rule
/// (`^[a-z0-9-]+$`): first-char alnum + a 64-char cap, with `_` allowed so
/// FE-minted `agent_<ms>_<n>` ids conform. Lowercase-only because
/// `find_launch_agent` resolves ids case-insensitively.
pub fn validate_custom_agent_id(id: &str) -> Result<(), String> {
    let bytes = id.as_bytes();
    if bytes.is_empty() {
        return Err("agent id must not be empty".to_string());
    }
    if bytes.len() > 64 {
        return Err(format!("agent id '{id}' must be at most 64 characters"));
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(format!(
            "agent id '{id}' must start with a lowercase letter or digit"
        ));
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
    {
        return Err(format!(
            "agent id '{id}' must match ^[a-z0-9][a-z0-9_-]{{0,63}}$ (lowercase letters, digits, hyphen, underscore)"
        ));
    }
    Ok(())
}

/// Validate an explicit `--color` flag value: strict `^#[0-9a-fA-F]{6}$`.
pub fn validate_agent_color(color: &str) -> Result<(), String> {
    let bytes = color.as_bytes();
    let ok = bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit);
    if ok {
        Ok(())
    } else {
        Err(format!(
            "color '{color}' must be a 6-digit hex value like #RRGGBB"
        ))
    }
}

/// Mint an agent id in the FE-compatible shape `agent_<unix_ms>_<6 hex>`.
/// Collision-safe for scripted loops (the uuid suffix disambiguates same-ms
/// mints); conforms to `validate_custom_agent_id`.
pub fn mint_agent_id() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let uuid = uuid::Uuid::new_v4().simple().to_string();
    format!("agent_{}_{}", ms, &uuid[..6])
}

/// Error message for an unknown selector id that lists the available ids.
pub fn unknown_agent_id_error(id: &str, agents: &[AgentConfig]) -> String {
    let available: Vec<&str> = agents.iter().map(|a| a.id.as_str()).collect();
    let list = if available.is_empty() {
        "(none)".to_string()
    } else {
        available.join(", ")
    };
    format!("agent id '{id}' not found. Available ids: {list}")
}

/// #2306 P1 - direction of a registered-agent move request. Serializes as the
/// `"up"`/`"down"` strings the picker sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMoveDirection {
    Up,
    Down,
}

/// #786 R8: PURE op application. Mutates `settings` in memory and runs the shared
/// validator/repair suite; NEVER saves to disk. Both callers own persistence.
pub fn apply_coding_agent_op(
    settings: &mut AppSettings,
    op: &CodingAgentOp,
) -> Result<CodingAgentOpOutcome, String> {
    let outcome = match op {
        CodingAgentOp::Add { agent } => {
            validate_custom_agent_id(&agent.id)?;
            if settings
                .agents
                .iter()
                .any(|a| a.id.eq_ignore_ascii_case(&agent.id))
            {
                return Err(format!("agent id '{}' already exists", agent.id));
            }
            if agent.label.trim().is_empty() {
                return Err("agent label must not be empty".to_string());
            }
            ensure_seed_consistent(&agent.config_seed)?;
            warn_on_duplicate_label(settings, agent.label.trim(), None);
            let mut new_agent = agent.clone();
            new_agent.label = new_agent.label.trim().to_string();
            ensure_backend_consistent(&mut new_agent)?;
            // #2306 P1 - Add appends at position n; a caller-supplied `order` is
            // ignored on purpose because only a move may choose a position.
            new_agent.order = Some(settings.agents.len() as u32);
            settings.agents.push(new_agent.clone());
            CodingAgentOpOutcome {
                op: "add",
                agent: Some(new_agent),
                removed_id: None,
            }
        }
        CodingAgentOp::Update { id, patch } => {
            let idx = settings
                .agents
                .iter()
                .position(|a| a.id == *id)
                .ok_or_else(|| unknown_agent_id_error(id, &settings.agents))?;
            let mut updated = settings.agents[idx].clone();
            apply_patch(&mut updated, patch)?;
            // #2306 P1 - Update/relabel keeps the current position; `AgentPatch` has
            // no order field, so a patch can never move a tool.
            updated.order = Some(idx as u32);
            warn_on_duplicate_label(settings, updated.label.trim(), Some(idx));
            settings.agents[idx] = updated.clone();
            CodingAgentOpOutcome {
                op: "update",
                agent: Some(updated),
                removed_id: None,
            }
        }
        CodingAgentOp::Remove { id } => {
            let idx = settings
                .agents
                .iter()
                .position(|a| a.id == *id)
                .ok_or_else(|| unknown_agent_id_error(id, &settings.agents))?;
            settings.agents.remove(idx);
            // Orphan `profilesByAgent[id]` / label entries are intentionally left
            // in place: `repair_coding_agent_profiles_config` never prunes by
            // agent id, and the GUI leaves them too. A same-id re-add resurrects
            // the old cells (documented, accepted).
            CodingAgentOpOutcome {
                op: "remove",
                agent: None,
                removed_id: Some(id.clone()),
            }
        }
    };

    // #2306 P1 - keep vector and ordinals aligned after every op: Add appended at
    // n, Remove compacted and Update kept its slot. Renumbering in current vector
    // order (never by stored ordinals) is the effective-view transition.
    normalize_agent_order(&mut settings.agents);

    // Authoritative shared suite: command bans, env rows, instructions filename,
    // config-seed dest, plus profile repair (A-cell bookkeeping for a new agent).
    // This is exactly what every existing writer enforces.
    validate_and_repair_settings(settings)?;
    outcome_from_written(outcome, settings)
}

/// Select the final row by its stable ID after validation or persistence.
/// Update intentionally retains the historical first-exact-ID selection.
pub fn outcome_from_written(
    mut outcome: CodingAgentOpOutcome,
    written: &AppSettings,
) -> Result<CodingAgentOpOutcome, String> {
    if matches!(outcome.op, "add" | "update") {
        let id = outcome
            .agent
            .as_ref()
            .ok_or_else(|| "coding-agent outcome is missing its required row".to_string())?
            .id
            .clone();
        outcome.agent = Some(
            written
                .agents
                .iter()
                .find(|agent| agent.id == id)
                .ok_or_else(|| format!("written settings are missing agent '{id}'"))?
                .clone(),
        );
    }
    Ok(outcome)
}

/// #2306 P1 - pure "swap with the adjacent tool" transition over the effective
/// order.
///
/// Effective order is derived on a temporary candidate first, so a rejected
/// request (or a caller that supplied an unnormalized vector) can never reorder
/// or renumber the live settings. `id` and `neighbor_id` must each occur exactly
/// once by exact, case-sensitive id, and `neighbor_id` must be the immediate
/// `direction` neighbor of `id` in that effective order: a boundary move, a
/// stale/nonadjacent neighbor, an unknown id and a duplicated id all fail with
/// `Err` and leave `settings` untouched. On success the live vector is replaced
/// by the swapped, renumbered candidate and the authoritative ordered id list is
/// returned.
pub fn move_registered_agent(
    settings: &mut AppSettings,
    id: &str,
    neighbor_id: &str,
    direction: AgentMoveDirection,
) -> Result<Vec<String>, String> {
    let mut candidate = settings.agents.clone();
    finalize_agent_order(&mut candidate);

    let from = unique_agent_index(&candidate, id)?;
    let neighbor = unique_agent_index(&candidate, neighbor_id)?;
    let target = match direction {
        AgentMoveDirection::Up => from.checked_sub(1),
        AgentMoveDirection::Down => from.checked_add(1).filter(|index| *index < candidate.len()),
    };
    match target {
        Some(index) if index == neighbor => {}
        _ => {
            let boundary = match direction {
                AgentMoveDirection::Up => from == 0,
                AgentMoveDirection::Down => from + 1 >= candidate.len(),
            };
            return Err(if boundary {
                let edge = match direction {
                    AgentMoveDirection::Up => "top",
                    AgentMoveDirection::Down => "bottom",
                };
                format!("agent '{id}' is already at the {edge} of the list")
            } else {
                format!(
                    "agent '{neighbor_id}' is not adjacent to '{id}' in the requested direction"
                )
            });
        }
    }

    candidate.swap(from, neighbor);
    normalize_agent_order(&mut candidate);
    settings.agents = candidate;
    Ok(settings
        .agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect())
}

/// #2542 - pure "move one tool to `target_index`" transition over the
/// effective order, in one step.
///
/// Like `move_registered_agent`, all validation runs on a finalized candidate,
/// so a rejected request can never reorder or renumber the live settings. The
/// candidate's ordered ids must equal `expected_ids` element-wise (a stale view
/// is rejected), `id` must occur exactly once, `target_index` must be in range
/// and must differ from the current index (a no-op is rejected so nothing is
/// written or emitted). The move is remove-then-insert-at-index; on success the
/// live vector is replaced by the renumbered candidate and the authoritative
/// ordered id list is returned.
pub fn reorder_registered_agent(
    settings: &mut AppSettings,
    id: &str,
    expected_ids: &[String],
    target_index: usize,
) -> Result<Vec<String>, String> {
    let mut candidate = settings.agents.clone();
    finalize_agent_order(&mut candidate);

    let current: Vec<&str> = candidate.iter().map(|agent| agent.id.as_str()).collect();
    if current.len() != expected_ids.len()
        || current
            .iter()
            .zip(expected_ids)
            .any(|(actual, expected)| *actual != expected.as_str())
    {
        return Err(
            "the agent order changed since it was read; refresh and retry the reorder".to_string(),
        );
    }

    let from = unique_agent_index(&candidate, id)?;
    if target_index >= candidate.len() {
        return Err(format!(
            "target index {target_index} is out of range for {} agents",
            candidate.len()
        ));
    }
    if target_index == from {
        return Err(format!(
            "agent '{id}' is already at index {target_index}; nothing to reorder"
        ));
    }

    let agent = candidate.remove(from);
    candidate.insert(target_index, agent);
    normalize_agent_order(&mut candidate);
    settings.agents = candidate;
    Ok(settings
        .agents
        .iter()
        .map(|agent| agent.id.clone())
        .collect())
}

/// #2306 P1 - the single index of `id` in `agents`, rejecting an unknown or
/// duplicated id. Comparison is exact: ids differing only by case are distinct
/// records here, matching the move contract.
fn unique_agent_index(agents: &[AgentConfig], id: &str) -> Result<usize, String> {
    let mut found: Option<usize> = None;
    for (index, agent) in agents.iter().enumerate() {
        if agent.id == id {
            if found.is_some() {
                return Err(format!(
                    "agent id '{id}' appears more than once; reordering requires unique ids"
                ));
            }
            found = Some(index);
        }
    }
    found.ok_or_else(|| unknown_agent_id_error(id, agents))
}

fn apply_patch(agent: &mut AgentConfig, patch: &AgentPatch) -> Result<(), String> {
    if let Some(label) = &patch.label {
        if label.trim().is_empty() {
            return Err("agent label must not be empty".to_string());
        }
        agent.label = label.trim().to_string();
    }
    if let Some(command) = &patch.command {
        agent.command = command.clone();
    }
    if let Some(color) = &patch.color {
        agent.color = color.clone();
    }
    if let Some(envs) = &patch.envs {
        agent.envs = envs.clone();
    }
    if let Some(isolated) = patch.isolated_home {
        agent.isolated_home = isolated;
    }
    if patch.clear_instructions_filename {
        agent.instructions_filename = None;
    } else if let Some(name) = &patch.instructions_filename {
        agent.instructions_filename = Some(name.clone());
    }
    if patch.clear_config_seed {
        agent.config_seed = None;
    } else if patch.config_seed_dest.is_some() || patch.config_seed_enabled.is_some() {
        let mut seed = agent.config_seed.clone().unwrap_or(ConfigSeedConfig {
            enabled: true,
            dest: String::new(),
        });
        if let Some(dest) = &patch.config_seed_dest {
            seed.dest = dest.clone();
        }
        if let Some(enabled) = patch.config_seed_enabled {
            seed.enabled = enabled;
        }
        ensure_seed_consistent(&Some(seed.clone()))?;
        agent.config_seed = Some(seed);
    }
    if let Some(kind) = patch.backend_kind {
        agent.backend.kind = kind;
    }
    if patch.clear_container_image {
        agent.backend.image = None;
    } else if let Some(image) = &patch.container_image {
        agent.backend.image = Some(normalize_container_image_input(image, "container image")?);
    }
    ensure_backend_consistent(agent)?;
    Ok(())
}

fn ensure_backend_consistent(agent: &mut AgentConfig) -> Result<(), String> {
    if let Some(image) = agent.backend.image.take() {
        let trimmed = image.trim();
        if trimmed.is_empty() {
            agent.backend.image = None;
        } else {
            agent.backend.image = Some(normalize_container_image_input(
                trimmed,
                &format!("Agent \"{}\" container image", agent.label),
            )?);
        }
    }
    if agent.backend.kind == SessionBackendKind::LocalProcess {
        agent.backend.image = None;
    }
    Ok(())
}

/// Reject an enabled seed with no destination (the GUI picker never produces
/// this; enabling requires a dest). An inactive seed (disabled, any dest) is fine.
fn ensure_seed_consistent(seed: &Option<ConfigSeedConfig>) -> Result<(), String> {
    if let Some(s) = seed {
        if s.enabled && s.dest.trim().is_empty() {
            return Err(
                "config seed is enabled but has no destination; pass --config-seed-dest"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn warn_on_duplicate_label(settings: &AppSettings, label: &str, skip_idx: Option<usize>) {
    let dup = settings
        .agents
        .iter()
        .enumerate()
        .any(|(i, a)| Some(i) != skip_idx && a.label.trim().eq_ignore_ascii_case(label));
    if dup {
        log::warn!(
            "[coding-agent] duplicate label '{label}'; launch-by-label resolution may be ambiguous"
        );
    }
}

// ---------------------------------------------------------------------------
// Queue transport helpers (daemon path). All writes are atomic temp+rename.
// ---------------------------------------------------------------------------

/// Atomically write a request into `dir` as `<requestId>.json`. Returns the
/// final path (used by the CLI to cancel on timeout).
pub fn write_coding_agent_request(dir: &Path, req: &CodingAgentRequest) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create coding-agent-requests dir: {e}"))?;
    let final_path = dir.join(format!("{}.json", req.request_id));
    let tmp_path = dir.join(format!("{}.json.tmp", req.request_id));
    let json = serde_json::to_string_pretty(req)
        .map_err(|e| format!("Failed to serialize request: {e}"))?;
    std::fs::write(&tmp_path, json)
        .map_err(|e| format!("Failed to write request temp file: {e}"))?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to finalize request file: {e}")
    })?;
    Ok(final_path)
}

/// #786 R4: the result writer OWNS the results dir (`create_dir_all` first) so
/// the very first result on a fresh config dir cannot fail for a missing dir.
pub fn write_coding_agent_result(
    results_dir: &Path,
    res: &CodingAgentResult,
) -> Result<(), String> {
    std::fs::create_dir_all(results_dir)
        .map_err(|e| format!("Failed to create results dir: {e}"))?;
    let final_path = results_dir.join(format!("{}.json", res.request_id));
    let tmp_path = results_dir.join(format!("{}.json.tmp", res.request_id));
    let json = serde_json::to_string_pretty(res)
        .map_err(|e| format!("Failed to serialize result: {e}"))?;
    std::fs::write(&tmp_path, json)
        .map_err(|e| format!("Failed to write result temp file: {e}"))?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to finalize result file: {e}")
    })?;
    Ok(())
}

/// Read a result file if present (used by the CLI while polling).
pub fn read_coding_agent_result(results_dir: &Path, request_id: &str) -> Option<CodingAgentResult> {
    let path = results_dir.join(format!("{request_id}.json"));
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// #786 R5: expiry with a saturating subtraction. A `created_at_ms` in the
/// future (wall clock stepped back between the CLI write and the poller read)
/// yields age 0, so a fresh request is never wrongly rejected as expired.
pub fn is_expired(created_at_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(created_at_ms) > REQUEST_EXPIRY_MS
}

fn processing_path_for(request_path: &Path) -> PathBuf {
    let mut s = request_path.as_os_str().to_os_string();
    s.push(".processing");
    PathBuf::from(s)
}

/// #786 R8: process ONE request file end to end. PURE of Tauri:
/// `settings` is the candidate to mutate and `save` persists it. Order (R2):
/// claim (atomic rename to `.processing`) -> parse -> expiry -> apply -> write
/// result -> delete `.processing` LAST. Path-parameterized for tempdir tests.
pub fn process_coding_agent_request(
    request_path: &Path,
    results_dir: &Path,
    now_ms: u64,
    settings: &mut AppSettings,
    save: &mut dyn FnMut(&AppSettings) -> Result<AppSettings, String>,
) -> RequestDisposition {
    // Claim: an atomic rename on the same volume. ENOENT means a timed-out CLI
    // already removed the request; never apply it.
    let processing_path = processing_path_for(request_path);
    if std::fs::rename(request_path, &processing_path).is_err() {
        return RequestDisposition::Skipped;
    }
    // The CLI polls `results/<stem>.json`, where `<stem>` is the request-file
    // stem (also the requestId it generated).
    let request_id = request_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let disposition = process_claimed(
        &processing_path,
        results_dir,
        &request_id,
        now_ms,
        settings,
        save,
    );
    // Delete the claim file LAST, after the result is durably written.
    let _ = std::fs::remove_file(&processing_path);
    disposition
}

fn process_claimed(
    processing_path: &Path,
    results_dir: &Path,
    request_id: &str,
    now_ms: u64,
    settings: &mut AppSettings,
    save: &mut dyn FnMut(&AppSettings) -> Result<AppSettings, String>,
) -> RequestDisposition {
    let content = match std::fs::read_to_string(processing_path) {
        Ok(c) => c,
        Err(e) => {
            write_reject(
                results_dir,
                request_id,
                format!("failed to read request: {e}"),
            );
            return RequestDisposition::Rejected;
        }
    };
    let request: CodingAgentRequest = match serde_json::from_str(&content) {
        Ok(r) => r,
        Err(e) => {
            write_reject(results_dir, request_id, format!("malformed request: {e}"));
            return RequestDisposition::Rejected;
        }
    };
    if is_expired(request.created_at_ms, now_ms) {
        write_reject(results_dir, request_id, "request expired".to_string());
        return RequestDisposition::Rejected;
    }
    match apply_coding_agent_op(settings, &request.op) {
        Ok(outcome) => {
            let written = match save(settings) {
                Ok(written) => written,
                Err(e) => {
                    write_reject(
                        results_dir,
                        request_id,
                        format!("failed to persist settings: {e}"),
                    );
                    return RequestDisposition::Rejected;
                }
            };
            let outcome = match outcome_from_written(outcome, &written) {
                Ok(outcome) => outcome,
                Err(error) => {
                    write_reject(results_dir, request_id, error);
                    return RequestDisposition::Rejected;
                }
            };
            *settings = written;
            let agent_id = outcome
                .agent
                .as_ref()
                .map(|a| a.id.clone())
                .or_else(|| outcome.removed_id.clone());
            let _ = write_coding_agent_result(
                results_dir,
                &CodingAgentResult {
                    request_id: request_id.to_string(),
                    ok: true,
                    op: Some(outcome.op.to_string()),
                    error: None,
                    agent: outcome.agent,
                    removed_id: outcome.removed_id,
                },
            );
            RequestDisposition::Applied {
                op: outcome.op,
                agent_id,
            }
        }
        Err(e) => {
            write_reject(results_dir, request_id, e);
            RequestDisposition::Rejected
        }
    }
}

fn write_reject(results_dir: &Path, request_id: &str, error: String) {
    let _ = write_coding_agent_result(
        results_dir,
        &CodingAgentResult {
            request_id: request_id.to_string(),
            ok: false,
            op: None,
            error: Some(error),
            agent: None,
            removed_id: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::CodingAgentEnvSource;
    use crate::pty::backend::SessionBackendKind;

    #[test]
    fn single_editor_inactive_gate() {
        use crate::config::settings::{
            list_active_sources, source_transition_effect_counts, SourceKind, SourceReadContext,
            SourceRevision,
        };
        let root = crate::config::settings::source_transition_test_root();
        let project = root.path().join("project");
        let config = root.path().join("C");
        std::fs::create_dir_all(project.join(".ac/coding-agents")).unwrap();
        std::fs::create_dir(&config).unwrap();
        let settings = AppSettings {
            project_paths: vec![project.to_string_lossy().into_owned()],
            ..AppSettings::default()
        };
        let context =
            SourceReadContext::from_settings(&settings, &config.join("settings.json")).unwrap();
        let source = list_active_sources(&context)
            .into_iter()
            .find(|source| source.kind == SourceKind::CatalogPersonal)
            .unwrap();
        let before = source_transition_effect_counts();
        let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
        let reloads = std::cell::Cell::new(0);
        let reload = || {
            reloads.set(reloads.get() + 1);
            Ok(())
        };
        for candidate in [
            serde_json::json!({"schemaVersion":2,"agents":[]}),
            serde_json::Value::Null,
        ] {
            assert!(matches!(
                commit_source_candidate(
                    &settings,
                    &context,
                    &source,
                    "request-1",
                    &SourceRevision::Absent {},
                    &candidate,
                    &reload
                ),
                Err(SourceTransitionError::ActivationRequired)
            ));
        }
        let expected = SourceResumeExpectation {
            source: SourceRevision::Absent {},
            journal: SourceRevision::Absent {},
        };
        for operation in ["operation-1", "unknown", ""] {
            assert!(matches!(
                resume_identity_transition(
                    &settings, &context, &source, operation, &expected, &reload
                ),
                Err(SourceTransitionError::ActivationRequired)
            ));
        }
        let status = get_identity_transition(&context, &source, "unknown").unwrap();
        assert_eq!(status.state, IdentityTransitionReadState::NotFound);
        assert_eq!(source_transition_effect_counts(), before);
        assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
        assert_eq!(reloads.get(), 0);
        assert_eq!(std::fs::read_dir(&config).unwrap().count(), 0);
        assert_eq!(
            std::fs::read_dir(project.join(".ac/coding-agents"))
                .unwrap()
                .count(),
            0
        );
        let mut ordinary = settings.clone();
        let agent: AgentConfig = serde_json::from_value(serde_json::json!({
            "id":"custom-ordinary","label":"Ordinary","color":"#123456","command":"ordinary-command"})).unwrap();
        apply_coding_agent_op(&mut ordinary, &CodingAgentOp::Add { agent }).unwrap();
        let saved = crate::config::settings::save_settings_to_path_preserving_project_paths(
            &ordinary,
            &config.join("settings.json"),
        )
        .unwrap();
        assert!(saved
            .agents
            .iter()
            .any(|agent| agent.id == "custom-ordinary"));
        let registered: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                config.join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(registered["agents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|agent| agent["id"] == "custom-ordinary"));
        assert_eq!(source_transition_effect_counts(), before);
        assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
        assert!(crate::config::settings::read_own_source_journal(&context)
            .unwrap()
            .1
            .intents
            .is_empty());
    }

    #[test]
    fn single_editor_status_unknown_inactive_zero_write_without_sidecars() {
        use crate::config::settings::{SourceKind, SourceReadContext};
        let root = crate::config::settings::source_transition_test_root();
        let settings = AppSettings::default();
        let context =
            SourceReadContext::from_settings(&settings, &root.path().join("settings.json"))
                .unwrap();
        let source = crate::config::settings::list_active_sources(&context)
            .into_iter()
            .find(|source| source.kind == SourceKind::RegisteredInstance)
            .unwrap();
        for _ in 0..3 {
            let read = get_identity_transition(&context, &source, "unknown-operation").unwrap();
            assert_eq!(read.state, IdentityTransitionReadState::NotFound);
            assert!(read.operation_id.is_none());
            assert!(!read.active);
            assert!(read.status.summary.is_none());
            assert_eq!(
                read.status.journal_revision,
                crate::config::settings::SourceRevision::Absent {}
            );
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn single_editor_status_invalid_present_journal_preserved_not_empty() {
        use crate::config::settings::{SourceCoordinationError, SourceKind, SourceReadContext};
        let root = crate::config::settings::source_transition_test_root();
        let settings = AppSettings::default();
        let context =
            SourceReadContext::from_settings(&settings, &root.path().join("settings.json"))
                .unwrap();
        let source = crate::config::settings::list_active_sources(&context)
            .into_iter()
            .find(|source| source.kind == SourceKind::RegisteredInstance)
            .unwrap();
        let journal = root
            .path()
            .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
        std::fs::write(&journal, b"{}").unwrap();
        assert!(matches!(
            get_identity_transition(&context, &source, "unknown-operation"),
            Err(SourceCoordinationError::Invalid)
        ));
        assert_eq!(std::fs::read(&journal).unwrap(), b"{}");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    fn agent(id: &str, label: &str, command: &str) -> AgentConfig {
        AgentConfig {
            preflight: None,
            preflight_timeout_seconds: None,
            id: id.to_string(),
            id_hash: None,
            id_hash_debug: None,
            label: label.to_string(),
            command: command.to_string(),
            color: "#6366f1".to_string(),
            order: None,
            envs: Vec::new(),
            isolated_home: false,
            instructions_filename: None,
            config_seed: None,
            context_regex: None,
            blocking_menus: None,
            backend: Default::default(),
        }
    }

    fn add(agent: AgentConfig) -> CodingAgentOp {
        CodingAgentOp::Add { agent }
    }

    #[test]
    fn issue_2859_preflight_add_and_patch_preserve_supplied_and_dormant_fields() {
        let mut settings = AppSettings::default();
        let mut row = agent("first", "First", "codex");
        row.preflight = Some(" raw  command ".into());
        row.preflight_timeout_seconds = Some(123);
        apply_coding_agent_op(&mut settings, &add(row.clone())).unwrap();
        row.id = "second".into();
        row.label = "Second".into();
        row.preflight = None;
        row.preflight_timeout_seconds = Some(60);
        apply_coding_agent_op(&mut settings, &add(row)).unwrap();
        apply_coding_agent_op(
            &mut settings,
            &CodingAgentOp::Update {
                id: "first".into(),
                patch: AgentPatch {
                    label: Some("Renamed".into()),
                    ..Default::default()
                },
            },
        )
        .unwrap();
        assert_eq!(
            settings.agents[0].preflight.as_deref(),
            Some(" raw  command ")
        );
        assert_eq!(settings.agents[0].preflight_timeout_seconds, Some(123));
        assert!(settings.agents[1].preflight.is_none());
        assert_eq!(settings.agents[1].preflight_timeout_seconds, Some(60));
    }

    #[test]
    fn issue_2859_preflight_request_save_path_rejects_zero_and_persists_valid_values() {
        for (timeout, accepted) in [(0, false), (60, true), (u32::MAX, true)] {
            let dir = crate::config::settings::source_transition_test_root();
            let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
            let results = requests.join(RESULTS_SUBDIR);
            let persisted = dir.path().join("saved.json");
            let mut row = agent("row", "Row", "codex");
            row.preflight = Some(" raw  command ".into());
            row.preflight_timeout_seconds = Some(timeout);
            let path = write_request_file(&requests, "preflight", 100, add(row));
            let mut settings = AppSettings::default();
            let mut save = |settings: &AppSettings| {
                std::fs::write(&persisted, serde_json::to_vec(settings).unwrap())
                    .map_err(|error| error.to_string())?;
                Ok(settings.clone())
            };
            let disposition =
                process_coding_agent_request(&path, &results, 200, &mut settings, &mut save);
            assert_eq!(
                matches!(disposition, RequestDisposition::Applied { .. }),
                accepted
            );
            assert_eq!(persisted.exists(), accepted);
            let result = read_coding_agent_result(&results, "preflight").unwrap();
            assert_eq!(result.ok, accepted);
            if accepted {
                let saved: AppSettings =
                    serde_json::from_slice(&std::fs::read(&persisted).unwrap()).unwrap();
                assert_eq!(saved.agents[0].preflight.as_deref(), Some(" raw  command "));
                assert_eq!(saved.agents[0].preflight_timeout_seconds, Some(timeout));
            } else {
                let error = result.error.unwrap();
                assert!(error.contains("row") && error.contains("preflightTimeoutSeconds"));
                assert!(!error.contains("raw  command"));
            }
        }
    }

    #[test]
    fn issue_2859_preflight_save_path_validates_dormant_profile_cells_before_save() {
        let dir = crate::config::settings::source_transition_test_root();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let mut settings = AppSettings::default();
        apply_coding_agent_op(&mut settings, &add(agent("row", "Row", "codex"))).unwrap();
        let cell = settings
            .coding_agent_profiles
            .profiles_by_agent
            .get_mut("row")
            .unwrap()
            .get_mut("A")
            .unwrap();
        cell.enabled = false;
        cell.preflight_timeout_seconds = Some(0);
        let path = write_request_file(
            &requests,
            "profile-zero",
            100,
            CodingAgentOp::Update {
                id: "row".into(),
                patch: AgentPatch::default(),
            },
        );
        let mut saves = 0;
        let disposition =
            process_coding_agent_request(&path, &results, 200, &mut settings, &mut |st| {
                saves += 1;
                Ok(st.clone())
            });
        assert_eq!(disposition, RequestDisposition::Rejected);
        assert_eq!(saves, 0);
        let error = read_coding_agent_result(&results, "profile-zero")
            .unwrap()
            .error
            .unwrap();
        assert!(
            error.contains("row")
                && error.contains("profile 'A'")
                && error.contains("preflightTimeoutSeconds")
        );
    }

    #[test]
    fn id_hash_activation_apply_uses_final_command_env_and_metadata_exclusions() {
        let mut settings = AppSettings::default();
        let mut row = agent("stable", "Stable", "codex");
        row.id_hash = Some("forged".into());
        row.backend.image = Some("discard-local-image".into());
        let added = apply_coding_agent_op(&mut settings, &add(row))
            .unwrap()
            .agent
            .unwrap();
        assert_eq!(
            serde_json::to_value(&added).unwrap(),
            serde_json::to_value(&settings.agents[0]).unwrap()
        );
        assert!(added.backend.image.is_none());
        let original_hash = added.id_hash.clone();
        let metadata = AgentPatch {
            label: Some("Renamed".into()),
            color: Some("#abc123".into()),
            isolated_home: Some(true),
            ..Default::default()
        };
        let changed = apply_coding_agent_op(
            &mut settings,
            &CodingAgentOp::Update {
                id: "stable".into(),
                patch: metadata,
            },
        )
        .unwrap()
        .agent
        .unwrap();
        assert_eq!(changed.id_hash, original_hash);
        let disabled = CodingAgentEnv {
            key: "MODE".into(),
            value: "one".into(),
            source: CodingAgentEnvSource::User,
            enabled: false,
        };
        let updated = apply_coding_agent_op(
            &mut settings,
            &CodingAgentOp::Update {
                id: "stable".into(),
                patch: AgentPatch {
                    command: Some("codex x".into()),
                    envs: Some(vec![disabled]),
                    ..Default::default()
                },
            },
        )
        .unwrap()
        .agent
        .unwrap();
        assert_eq!(updated.id, "stable");
        assert_eq!(
            updated.id_hash,
            Some(crate::config::settings::configuration_identity(
                &updated.command,
                &updated.envs
            ))
        );
        assert_ne!(updated.id_hash, original_hash);
        let disabled_hash = updated.id_hash;
        settings.agents[0].envs[0].value = "two".into();
        let updated = apply_coding_agent_op(
            &mut settings,
            &CodingAgentOp::Update {
                id: "stable".into(),
                patch: AgentPatch::default(),
            },
        )
        .unwrap()
        .agent
        .unwrap();
        assert_ne!(updated.id_hash, disabled_hash);
        let cleared = apply_coding_agent_op(
            &mut settings,
            &CodingAgentOp::Update {
                id: "stable".into(),
                patch: AgentPatch {
                    envs: Some(vec![]),
                    ..Default::default()
                },
            },
        )
        .unwrap()
        .agent
        .unwrap();
        assert!(cleared.envs.is_empty());
        assert_eq!(
            cleared.id_hash,
            Some(crate::config::settings::configuration_identity(
                "codex x",
                &[]
            ))
        );
    }

    #[test]
    fn id_hash_activation_queued_real_saver_result_tracks_written_and_reload() {
        let dir = crate::config::settings::source_transition_test_root();
        let settings_path = dir.path().join("settings.json");
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let mut settings = AppSettings::default();
        let mut input = agent("stable", "Stable", "codex");
        input.id_hash = Some("forged-input".into());
        for (request_id, op) in [
            ("add", add(input)),
            (
                "clear",
                CodingAgentOp::Update {
                    id: "stable".into(),
                    patch: AgentPatch {
                        envs: Some(vec![]),
                        ..Default::default()
                    },
                },
            ),
            (
                "remove",
                CodingAgentOp::Remove {
                    id: "stable".into(),
                },
            ),
        ] {
            let request = write_request_file(&requests, request_id, 100, op);
            let mut captured = None;
            let disposition = process_coding_agent_request(
                &request,
                &results,
                200,
                &mut settings,
                &mut |candidate| {
                    let mut physical = candidate.clone();
                    if let Some(row) = physical.agents.first_mut() {
                        // A real saver must be the authority even if its final inputs differ.
                        row.command = "final-saver-command".into();
                        row.id_hash = Some("forged-before-save".into());
                    }
                    let written =
                        crate::config::settings::save_settings_to_path_preserving_project_paths(
                            &physical,
                            &settings_path,
                        )?;
                    captured = Some(written.clone());
                    Ok(written)
                },
            );
            assert!(matches!(disposition, RequestDisposition::Applied { .. }));
            let result = read_coding_agent_result(&results, request_id).unwrap();
            assert!(result.ok);
            let written = captured.unwrap();
            let reload = crate::config::settings::load_settings_from_path(&settings_path);
            assert_eq!(
                serde_json::to_value(&settings.agents).unwrap(),
                serde_json::to_value(&written.agents).unwrap()
            );
            assert_eq!(
                serde_json::to_value(&reload.agents).unwrap(),
                serde_json::to_value(&written.agents).unwrap()
            );
            if request_id == "remove" {
                assert!(result.agent.is_none());
                assert_eq!(result.removed_id.as_deref(), Some("stable"));
            } else {
                assert_eq!(
                    serde_json::to_value(result.agent.as_ref()).unwrap(),
                    serde_json::to_value(written.agents.first()).unwrap()
                );
                assert_eq!(result.agent.unwrap().command, "final-saver-command");
                let disk: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(
                        dir.path()
                            .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(
                    disk["agents"][0]["id_hash"],
                    written.agents[0].id_hash.clone().unwrap()
                );
            }
            assert!(!request.exists() && !processing_path_for(&request).exists());
        }
    }

    #[test]
    fn id_hash_activation_queued_errors_never_publish_success() {
        for failure in [
            "invalid-command",
            "invalid-env",
            "bad-disk",
            "missing-written-row",
        ] {
            let dir = crate::config::settings::source_transition_test_root();
            let path = dir.path().join("settings.json");
            let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
            let results = requests.join(RESULTS_SUBDIR);
            let mut row = agent("stable", "Stable", "codex");
            if failure == "invalid-command" {
                row.command = "codex resume --last".into();
            }
            if failure == "invalid-env" {
                row.envs = vec![CodingAgentEnv {
                    key: "AGENTSCOMMANDER_TOKEN".into(),
                    value: "bad".into(),
                    source: CodingAgentEnvSource::User,
                    enabled: true,
                }];
            }
            let sidecar = dir
                .path()
                .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME);
            if failure == "bad-disk" {
                std::fs::write(&sidecar, b"{ malformed").unwrap();
            }
            let request = write_request_file(&requests, "rejected", 100, add(row));
            let mut candidate = AppSettings::default();
            let disposition = process_coding_agent_request(
                &request,
                &results,
                200,
                &mut candidate,
                &mut |settings| {
                    let mut physical = settings.clone();
                    if failure == "missing-written-row" {
                        physical.agents.clear();
                    }
                    crate::config::settings::save_settings_to_path_preserving_project_paths(
                        &physical, &path,
                    )
                },
            );
            assert_eq!(disposition, RequestDisposition::Rejected, "{failure}");
            let result = read_coding_agent_result(&results, "rejected").unwrap();
            assert!(!result.ok && result.agent.is_none(), "{failure}");
            assert!(result.error.is_some());
            if failure == "bad-disk" {
                assert_eq!(std::fs::read(&sidecar).unwrap(), b"{ malformed");
            }
            if failure.starts_with("invalid-") {
                assert!(!path.exists() && !sidecar.exists());
            }
            assert!(!processing_path_for(&request).exists());
        }
    }

    fn noop_save() -> impl FnMut(&AppSettings) -> Result<AppSettings, String> {
        |s: &AppSettings| Ok(s.clone())
    }

    // ---- id / color validators ---------------------------------------------

    #[test]
    fn minted_id_conforms_and_is_lowercase() {
        let id = mint_agent_id();
        assert!(validate_custom_agent_id(&id).is_ok(), "minted id: {id}");
        assert_eq!(id, id.to_lowercase());
    }

    #[test]
    fn custom_id_rules() {
        assert!(validate_custom_agent_id("claude-1").is_ok());
        assert!(validate_custom_agent_id("agent_123_ab12cd").is_ok());
        assert!(validate_custom_agent_id("").is_err());
        assert!(
            validate_custom_agent_id("Claude").is_err(),
            "uppercase rejected"
        );
        assert!(
            validate_custom_agent_id("-foo").is_err(),
            "leading hyphen rejected"
        );
        assert!(validate_custom_agent_id("has space").is_err());
        assert!(
            validate_custom_agent_id(&"a".repeat(65)).is_err(),
            "over 64 rejected"
        );
        assert!(
            validate_custom_agent_id(&"a".repeat(64)).is_ok(),
            "64 allowed"
        );
    }

    #[test]
    fn color_rules() {
        assert!(validate_agent_color("#6366f1").is_ok());
        assert!(validate_agent_color("#FFFFFF").is_ok());
        assert!(validate_agent_color("#fff").is_err());
        assert!(validate_agent_color("#6366f1ff").is_err());
        assert!(validate_agent_color("6366f1").is_err());
        assert!(validate_agent_color("red").is_err());
    }

    // ---- add ---------------------------------------------------------------

    #[test]
    fn add_appends_and_repairs_a_cell() {
        let mut s = AppSettings::default();
        let out = apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        assert_eq!(out.op, "add");
        assert_eq!(s.agents.len(), 1);
        // repair ensures an 'A' profile cell exists for the new agent.
        assert!(s
            .coding_agent_profiles
            .profiles_by_agent
            .get("claude")
            .map(|cells| cells.contains_key("A"))
            .unwrap_or(false));
    }

    #[test]
    fn add_rejects_duplicate_id() {
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        let err =
            apply_coding_agent_op(&mut s, &add(agent("claude", "Other", "codex"))).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn add_rejects_case_insensitive_dup_against_legacy_mixedcase_id() {
        let mut s = AppSettings::default();
        // A legacy agent with a mixed-case id predates the new lowercase-only
        // rule; insert it directly to bypass validate_custom_agent_id. Adding a
        // valid lowercase id that collides case-insensitively must be rejected
        // (find_launch_agent resolves ids case-insensitively).
        s.agents.push(agent("Claude", "Legacy", "claude"));
        let err = apply_coding_agent_op(&mut s, &add(agent("claude", "New", "codex"))).unwrap_err();
        assert!(err.contains("already exists"), "{err}");
    }

    #[test]
    fn add_rejects_bad_id_and_empty_label() {
        let mut s = AppSettings::default();
        assert!(apply_coding_agent_op(&mut s, &add(agent("Bad Id", "L", "claude"))).is_err());
        assert!(apply_coding_agent_op(&mut s, &add(agent("ok", "  ", "claude"))).is_err());
    }

    #[test]
    fn add_rejects_banned_continue_and_dup_env() {
        let mut s = AppSettings::default();
        let err =
            apply_coding_agent_op(&mut s, &add(agent("c", "C", "claude --continue"))).unwrap_err();
        assert!(err.contains("continue"), "{err}");

        let mut dup = agent("d", "D", "claude");
        dup.envs = vec![
            CodingAgentEnv {
                key: "FOO".into(),
                value: "1".into(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
            CodingAgentEnv {
                key: "FOO".into(),
                value: "2".into(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            },
        ];
        let mut s2 = AppSettings::default();
        assert!(apply_coding_agent_op(&mut s2, &add(dup)).is_err());
    }

    #[test]
    fn add_rejects_enabled_seed_without_dest() {
        let mut s = AppSettings::default();
        let mut a = agent("s", "S", "claude");
        a.config_seed = Some(ConfigSeedConfig {
            enabled: true,
            dest: String::new(),
        });
        assert!(apply_coding_agent_op(&mut s, &add(a)).is_err());
    }

    #[test]
    fn add_normalizes_backend_image() {
        let mut s = AppSettings::default();
        let mut a = agent("container", "Container", "claude");
        a.backend.kind = SessionBackendKind::ContainerTransport;
        a.backend.image = Some(" image:tag ".to_string());

        apply_coding_agent_op(&mut s, &add(a)).unwrap();

        assert_eq!(
            s.agents[0].backend.kind,
            SessionBackendKind::ContainerTransport
        );
        assert_eq!(s.agents[0].backend.image.as_deref(), Some("image:tag"));
    }

    #[test]
    fn add_clears_local_backend_image_and_rejects_leading_dash() {
        let mut local_settings = AppSettings::default();
        let mut local = agent("local", "Local", "claude");
        local.backend.image = Some(" image:tag ".to_string());
        apply_coding_agent_op(&mut local_settings, &add(local)).unwrap();
        assert!(local_settings.agents[0].backend.image.is_none());

        let mut bad_settings = AppSettings::default();
        let mut bad = agent("bad", "Bad", "claude");
        bad.backend.kind = SessionBackendKind::ContainerTransport;
        bad.backend.image = Some("--privileged".to_string());
        let err = apply_coding_agent_op(&mut bad_settings, &add(bad)).unwrap_err();
        assert!(err.contains("must not start with"), "{err}");
    }

    #[test]
    fn add_accepts_catalog_seeded_non_hex_color() {
        // §14.4 R6 carve-out: the op layer NEVER validates color (only the
        // explicit `--color` CLI flag does, in cmd_add). A catalog-seeded agent
        // whose color is a shorthand like "#fff" must be accepted as-is so that
        // `add --from-catalog <key>` never rejects a trusted catalog color.
        // Guards against a future regression that adds a color check here.
        let mut s = AppSettings::default();
        let mut a = agent("cat", "Catalog Agent", "claude");
        a.color = "#fff".to_string();
        let out = apply_coding_agent_op(&mut s, &add(a)).unwrap();
        assert_eq!(out.op, "add");
        assert_eq!(s.agents[0].color, "#fff", "catalog color persisted as-is");
    }

    // ---- update ------------------------------------------------------------

    #[test]
    fn update_partial_keeps_other_fields() {
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        let patch = AgentPatch {
            label: Some("Renamed".into()),
            ..Default::default()
        };
        apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch,
            },
        )
        .unwrap();
        assert_eq!(s.agents[0].label, "Renamed");
        assert_eq!(s.agents[0].command, "claude");
    }

    #[test]
    fn update_env_replace_all_and_clear() {
        let mut s = AppSettings::default();
        let mut a = agent("claude", "Claude", "claude");
        a.envs = vec![CodingAgentEnv {
            key: "OLD".into(),
            value: "1".into(),
            source: CodingAgentEnvSource::User,
            enabled: true,
        }];
        apply_coding_agent_op(&mut s, &add(a)).unwrap();

        let patch = AgentPatch {
            envs: Some(vec![CodingAgentEnv {
                key: "NEW".into(),
                value: "2".into(),
                source: CodingAgentEnvSource::User,
                enabled: true,
            }]),
            ..Default::default()
        };
        apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch,
            },
        )
        .unwrap();
        assert_eq!(s.agents[0].envs.len(), 1);
        assert_eq!(s.agents[0].envs[0].key, "NEW");

        let clear = AgentPatch {
            envs: Some(Vec::new()),
            ..Default::default()
        };
        apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch: clear,
            },
        )
        .unwrap();
        assert!(s.agents[0].envs.is_empty());
    }

    #[test]
    fn update_unknown_id_lists_available() {
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        let err = apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "nope".into(),
                patch: AgentPatch::default(),
            },
        )
        .unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains("claude"), "{err}");
    }

    #[test]
    fn update_clear_instructions_and_seed() {
        let mut s = AppSettings::default();
        let mut a = agent("claude", "Claude", "claude");
        a.instructions_filename = Some("CLAUDE.md".into());
        a.config_seed = Some(ConfigSeedConfig {
            enabled: true,
            dest: ".claude".into(),
        });
        apply_coding_agent_op(&mut s, &add(a)).unwrap();

        let patch = AgentPatch {
            clear_instructions_filename: true,
            clear_config_seed: true,
            ..Default::default()
        };
        apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch,
            },
        )
        .unwrap();
        assert!(s.agents[0].instructions_filename.is_none());
        assert!(s.agents[0].config_seed.is_none());
    }

    #[test]
    fn update_sets_and_clears_container_image() {
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();

        let patch = AgentPatch {
            backend_kind: Some(SessionBackendKind::ContainerTransport),
            container_image: Some(" image:tag ".into()),
            ..Default::default()
        };
        apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch,
            },
        )
        .unwrap();
        assert_eq!(
            s.agents[0].backend.kind,
            SessionBackendKind::ContainerTransport
        );
        assert_eq!(s.agents[0].backend.image.as_deref(), Some("image:tag"));

        let clear = AgentPatch {
            clear_container_image: true,
            ..Default::default()
        };
        apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch: clear,
            },
        )
        .unwrap();
        assert!(s.agents[0].backend.image.is_none());

        let local = AgentPatch {
            backend_kind: Some(SessionBackendKind::LocalProcess),
            container_image: Some("remembered:tag".into()),
            ..Default::default()
        };
        apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch: local,
            },
        )
        .unwrap();
        assert_eq!(s.agents[0].backend.kind, SessionBackendKind::LocalProcess);
        assert!(s.agents[0].backend.image.is_none());
    }

    #[test]
    fn update_rejects_leading_dash_container_image() {
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();

        let patch = AgentPatch {
            backend_kind: Some(SessionBackendKind::ContainerTransport),
            container_image: Some("--volume=/:/host".into()),
            ..Default::default()
        };
        let err = apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Update {
                id: "claude".into(),
                patch,
            },
        )
        .unwrap_err();
        assert!(err.contains("must not start with"), "{err}");
    }

    // ---- remove (orphan-preserve) ------------------------------------------

    #[test]
    fn remove_preserves_profile_orphans() {
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        // repair created profiles_by_agent["claude"]; removing the agent must
        // leave that orphan in place (matches repair/GUI semantics).
        assert!(s
            .coding_agent_profiles
            .profiles_by_agent
            .contains_key("claude"));
        let out = apply_coding_agent_op(
            &mut s,
            &CodingAgentOp::Remove {
                id: "claude".into(),
            },
        )
        .unwrap();
        assert_eq!(out.removed_id.as_deref(), Some("claude"));
        assert!(s.agents.is_empty());
        assert!(
            s.coding_agent_profiles
                .profiles_by_agent
                .contains_key("claude"),
            "profile orphan must survive remove"
        );
    }

    #[test]
    fn remove_unknown_id_errors() {
        let mut s = AppSettings::default();
        assert!(
            apply_coding_agent_op(&mut s, &CodingAgentOp::Remove { id: "ghost".into() }).is_err()
        );
    }

    // ---- purity ------------------------------------------------------------

    #[test]
    fn apply_is_pure_no_disk() {
        // A bare AppSettings with no config dir override: apply must not touch
        // disk (it takes &mut and validates only). If it saved, tests would need
        // a config dir; they do not.
        let mut s = AppSettings::default();
        apply_coding_agent_op(&mut s, &add(agent("claude", "Claude", "claude"))).unwrap();
        assert_eq!(s.agents.len(), 1);
    }

    // ---- queue helpers -----------------------------------------------------

    #[test]
    fn is_expired_backward_clock_and_old() {
        // future createdAt (backward clock) -> not expired
        assert!(!is_expired(1_000_000, 900_000));
        // fresh -> not expired
        assert!(!is_expired(1_000_000, 1_000_100));
        // genuinely old -> expired
        assert!(is_expired(0, REQUEST_EXPIRY_MS + 1));
    }

    #[test]
    fn request_result_roundtrip_atomic() {
        let dir = crate::config::settings::source_transition_test_root();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let req = CodingAgentRequest {
            request_id: "req1".into(),
            created_at_ms: 123,
            op: add(agent("claude", "Claude", "claude")),
        };
        let path = write_coding_agent_request(&requests, &req).unwrap();
        assert!(path.exists());
        // no leftover temp file
        assert!(std::fs::read_dir(&requests).unwrap().all(|e| {
            let p = e.unwrap().path();
            p.extension().and_then(|s| s.to_str()) == Some("json")
        }));

        let results = requests.join(RESULTS_SUBDIR);
        write_coding_agent_result(
            &results,
            &CodingAgentResult {
                request_id: "req1".into(),
                ok: true,
                op: Some("add".into()),
                error: None,
                agent: None,
                removed_id: None,
            },
        )
        .unwrap();
        let read = read_coding_agent_result(&results, "req1").unwrap();
        assert!(read.ok);
        assert_eq!(read.op.as_deref(), Some("add"), "op round-trips");
    }

    #[test]
    fn first_request_autocreates_results_dir() {
        // R4: brand-new tempdir, results/ does not exist; write_coding_agent_result
        // must create it.
        let dir = crate::config::settings::source_transition_test_root();
        let results = dir
            .path()
            .join(CODING_AGENT_REQUESTS_DIR)
            .join(RESULTS_SUBDIR);
        assert!(!results.exists());
        write_coding_agent_result(
            &results,
            &CodingAgentResult {
                request_id: "x".into(),
                ok: true,
                op: None,
                error: None,
                agent: None,
                removed_id: None,
            },
        )
        .unwrap();
        assert!(results.join("x.json").exists());
    }

    // ---- per-request handler ----------------------------------------------

    fn write_request_file(
        requests: &Path,
        id: &str,
        created_at_ms: u64,
        op: CodingAgentOp,
    ) -> PathBuf {
        std::fs::create_dir_all(requests).unwrap();
        let req = CodingAgentRequest {
            request_id: id.to_string(),
            created_at_ms,
            op,
        };
        let path = requests.join(format!("{id}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(&req).unwrap()).unwrap();
        path
    }

    #[test]
    fn handler_applies_and_writes_result_and_deletes_processing() {
        let dir = crate::config::settings::source_transition_test_root();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let path = write_request_file(
            &requests,
            "r1",
            100,
            add(agent("claude", "Claude", "claude")),
        );

        let mut settings = AppSettings::default();
        let mut saved = false;
        let disp = {
            let mut save = |_s: &AppSettings| {
                saved = true;
                Ok(_s.clone())
            };
            process_coding_agent_request(&path, &results, 200, &mut settings, &mut save)
        };
        assert!(matches!(
            disp,
            RequestDisposition::Applied { op: "add", .. }
        ));
        assert!(saved, "save callback invoked");
        assert_eq!(settings.agents.len(), 1);
        // result written, request + processing gone
        assert!(read_coding_agent_result(&results, "r1").unwrap().ok);
        assert!(!path.exists());
        assert!(!processing_path_for(&path).exists());
    }

    #[test]
    fn handler_applies_backend_image_patch_from_request_json() {
        let dir = crate::config::settings::source_transition_test_root();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let mut settings = AppSettings::default();
        apply_coding_agent_op(&mut settings, &add(agent("claude", "Claude", "claude"))).unwrap();
        let patch = AgentPatch {
            backend_kind: Some(SessionBackendKind::ContainerTransport),
            container_image: Some(" image:tag ".into()),
            ..Default::default()
        };
        let path = write_request_file(
            &requests,
            "patch",
            100,
            CodingAgentOp::Update {
                id: "claude".into(),
                patch,
            },
        );

        let disp =
            process_coding_agent_request(&path, &results, 200, &mut settings, &mut noop_save());

        assert!(matches!(disp, RequestDisposition::Applied { .. }));
        assert_eq!(
            settings.agents[0].backend.kind,
            SessionBackendKind::ContainerTransport
        );
        assert_eq!(
            settings.agents[0].backend.image.as_deref(),
            Some("image:tag")
        );
        assert!(read_coding_agent_result(&results, "patch").unwrap().ok);
    }

    #[test]
    fn handler_skips_when_request_already_removed() {
        let dir = crate::config::settings::source_transition_test_root();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        std::fs::create_dir_all(&requests).unwrap();
        let missing = requests.join("gone.json");
        let mut settings = AppSettings::default();
        let disp =
            process_coding_agent_request(&missing, &results, 200, &mut settings, &mut noop_save());
        assert_eq!(disp, RequestDisposition::Skipped);
        assert!(settings.agents.is_empty());
    }

    #[test]
    fn handler_rejects_expired_without_applying() {
        let dir = crate::config::settings::source_transition_test_root();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let path = write_request_file(
            &requests,
            "old",
            0,
            add(agent("claude", "Claude", "claude")),
        );
        let mut settings = AppSettings::default();
        let disp = process_coding_agent_request(
            &path,
            &results,
            REQUEST_EXPIRY_MS + 10,
            &mut settings,
            &mut noop_save(),
        );
        assert_eq!(disp, RequestDisposition::Rejected);
        assert!(settings.agents.is_empty());
        let res = read_coding_agent_result(&results, "old").unwrap();
        assert!(!res.ok);
        assert!(res.error.unwrap().contains("expired"));
    }

    #[test]
    fn handler_rejects_validation_error() {
        let dir = crate::config::settings::source_transition_test_root();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let path = write_request_file(&requests, "bad", 100, add(agent("BadId", "L", "claude")));
        let mut settings = AppSettings::default();
        let disp =
            process_coding_agent_request(&path, &results, 150, &mut settings, &mut noop_save());
        assert_eq!(disp, RequestDisposition::Rejected);
        assert!(settings.agents.is_empty());
        assert!(!read_coding_agent_result(&results, "bad").unwrap().ok);
    }

    // ---- T10: daemon-path launch consumption (acceptance criterion 2) ------

    #[test]
    fn daemon_applied_agent_is_launch_resolvable() {
        // Apply an Add through the extracted per-request handler (the daemon-path
        // seam), then confirm the new agent is resolvable to a subsequent
        // `--launch` by id AND by label, and that a spawn command builds for it.
        let dir = crate::config::settings::source_transition_test_root();
        let requests = dir.path().join(CODING_AGENT_REQUESTS_DIR);
        let results = requests.join(RESULTS_SUBDIR);
        let path = write_request_file(
            &requests,
            "r",
            100,
            add(agent("claude", "Claude Code", "claude")),
        );

        let mut settings = AppSettings::default();
        let disp =
            process_coding_agent_request(&path, &results, 200, &mut settings, &mut noop_save());
        assert!(matches!(disp, RequestDisposition::Applied { .. }));

        // find_launch_agent (create_agent.rs) resolves by exact id and by label.
        assert!(crate::cli::create_agent::find_launch_agent(&settings, "claude").is_some());
        assert!(crate::cli::create_agent::find_launch_agent(&settings, "Claude Code").is_some());

        // A spawn command builds against the same state (session.rs). Some(_)
        // means the agent was found and the command composed.
        let cwd = dir.path().to_string_lossy();
        let spawn = crate::commands::session::build_configured_agent_spawn_for_cwd(
            &settings, "claude", &cwd, None,
        );
        assert!(spawn.is_ok(), "spawn build failed: {:?}", spawn.err());
        assert!(
            spawn.unwrap().is_some(),
            "expected a spawn command for the applied agent"
        );
    }

    // ---- #2306 P1: explicit registered-agent order -------------------------

    /// `a,b,c` with contiguous ordinals, the shape the loaders leave behind.
    fn ordered_2306() -> AppSettings {
        let mut settings = AppSettings::default();
        for id in ["a", "b", "c"] {
            apply_coding_agent_op(&mut settings, &add(agent(id, &id.to_uppercase(), "claude")))
                .unwrap();
        }
        settings
    }

    fn ids_2306(settings: &AppSettings) -> Vec<&str> {
        settings.agents.iter().map(|a| a.id.as_str()).collect()
    }

    fn positions_2306(settings: &AppSettings) -> Vec<Option<u32>> {
        settings.agents.iter().map(|a| a.order).collect()
    }

    #[test]
    fn add_appends_at_the_next_position_and_ignores_a_caller_supplied_order() {
        let mut settings = ordered_2306();
        let mut incoming = agent("d", "D", "claude");
        incoming.order = Some(0); // forged/stale: must have no authority
        let outcome = apply_coding_agent_op(&mut settings, &add(incoming)).unwrap();
        assert_eq!(ids_2306(&settings), ["a", "b", "c", "d"]);
        assert_eq!(
            positions_2306(&settings),
            [Some(0), Some(1), Some(2), Some(3)]
        );
        assert_eq!(outcome.agent.unwrap().order, Some(3));
    }

    #[test]
    fn remove_compacts_positions_and_update_retains_the_current_position() {
        let mut settings = ordered_2306();
        apply_coding_agent_op(&mut settings, &CodingAgentOp::Remove { id: "b".into() }).unwrap();
        assert_eq!(ids_2306(&settings), ["a", "c"]);
        assert_eq!(positions_2306(&settings), [Some(0), Some(1)]);

        // A patch carrying an unknown `order` key cannot move the tool.
        let patch: AgentPatch = serde_json::from_str(r#"{"label":"Renamed","order":0}"#).unwrap();
        let outcome = apply_coding_agent_op(
            &mut settings,
            &CodingAgentOp::Update {
                id: "c".into(),
                patch,
            },
        )
        .unwrap();
        assert_eq!(ids_2306(&settings), ["a", "c"]);
        assert_eq!(positions_2306(&settings), [Some(0), Some(1)]);
        assert_eq!(outcome.agent.unwrap().order, Some(1));
        assert_eq!(settings.agents[1].label, "Renamed");
    }

    #[test]
    fn move_swaps_both_directions_and_returns_the_authoritative_order() {
        let mut settings = ordered_2306();
        let ids = move_registered_agent(&mut settings, "c", "b", AgentMoveDirection::Up).unwrap();
        assert_eq!(ids, ["a", "c", "b"]);
        assert_eq!(positions_2306(&settings), [Some(0), Some(1), Some(2)]);

        let ids = move_registered_agent(&mut settings, "c", "b", AgentMoveDirection::Down).unwrap();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(positions_2306(&settings), [Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn move_derives_the_effective_order_before_the_adjacency_check() {
        // Vector slots are [b, a] but the stored ordinals put a first; the move must
        // act on the effective order [a, b], and normalize the result.
        let mut settings = AppSettings::default();
        apply_coding_agent_op(&mut settings, &add(agent("a", "A", "claude"))).unwrap();
        apply_coding_agent_op(&mut settings, &add(agent("b", "B", "claude"))).unwrap();
        settings.agents.swap(0, 1);
        settings.agents[0].order = Some(1);
        settings.agents[1].order = Some(0);

        let ids = move_registered_agent(&mut settings, "b", "a", AgentMoveDirection::Up).unwrap();
        assert_eq!(ids, ["b", "a"]);
        assert_eq!(positions_2306(&settings), [Some(0), Some(1)]);
    }

    #[test]
    fn rejected_moves_leave_the_live_settings_untouched() {
        let cases = [
            ("a", "b", AgentMoveDirection::Up, "top"),
            ("c", "b", AgentMoveDirection::Down, "bottom"),
            ("b", "a", AgentMoveDirection::Down, "not adjacent"),
            ("missing", "a", AgentMoveDirection::Up, "not found"),
            ("a", "missing", AgentMoveDirection::Down, "not found"),
        ];
        for (id, neighbor, direction, expected) in cases {
            let mut settings = ordered_2306();
            let ids_before: Vec<String> = settings.agents.iter().map(|a| a.id.clone()).collect();
            let positions_before = positions_2306(&settings);
            let error = move_registered_agent(&mut settings, id, neighbor, direction).unwrap_err();
            assert!(
                error.contains(expected),
                "case ({id},{neighbor}): error {error:?} must mention {expected:?}"
            );
            let ids_after: Vec<String> = settings.agents.iter().map(|a| a.id.clone()).collect();
            assert_eq!(ids_after, ids_before, "case ({id},{neighbor})");
            assert_eq!(
                positions_2306(&settings),
                positions_before,
                "case ({id},{neighbor})"
            );
        }
    }

    #[test]
    fn move_rejects_duplicate_ids_while_case_distinct_ids_stay_distinct() {
        let mut duplicated = AppSettings {
            agents: vec![agent("dup", "D1", "claude"), agent("dup", "D2", "claude")],
            ..AppSettings::default()
        };
        let error = move_registered_agent(&mut duplicated, "dup", "dup", AgentMoveDirection::Up)
            .unwrap_err();
        assert!(error.contains("more than once"), "{error}");

        // Case-distinct ids are distinct records for ordering (Add keeps rejecting
        // case-insensitive duplicates, so this legacy pair is inserted directly).
        let mut upper = agent("A", "Upper", "claude");
        upper.order = Some(0);
        let mut lower = agent("a", "Lower", "claude");
        lower.order = Some(1);
        let mut cased = AppSettings {
            agents: vec![upper, lower],
            ..AppSettings::default()
        };
        let ids = move_registered_agent(&mut cased, "a", "A", AgentMoveDirection::Up).unwrap();
        assert_eq!(ids, ["a", "A"]);
    }

    fn reorder_fixture_2542() -> AppSettings {
        let mut settings = AppSettings::default();
        for id in ["a", "b", "c", "d", "e"] {
            apply_coding_agent_op(&mut settings, &add(agent(id, &id.to_uppercase(), "claude")))
                .unwrap();
        }
        settings
    }

    fn owned_ids_2542(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn reorder_moves_multiple_rows_down_and_up_in_one_step() {
        let mut settings = reorder_fixture_2542();
        let expected = owned_ids_2542(&["a", "b", "c", "d", "e"]);
        let ids = reorder_registered_agent(&mut settings, "b", &expected, 3).unwrap();
        assert_eq!(ids, ["a", "c", "d", "b", "e"]);
        assert_eq!(ids_2306(&settings), ["a", "c", "d", "b", "e"]);

        let ids = reorder_registered_agent(&mut settings, "e", &ids, 1).unwrap();
        assert_eq!(ids, ["a", "e", "c", "d", "b"]);
        assert_eq!(
            positions_2306(&settings),
            [Some(0), Some(1), Some(2), Some(3), Some(4)]
        );
    }

    #[test]
    fn reorder_reaches_both_ends_with_contiguous_positions() {
        let mut settings = reorder_fixture_2542();
        let expected = owned_ids_2542(&["a", "b", "c", "d", "e"]);
        let ids = reorder_registered_agent(&mut settings, "d", &expected, 0).unwrap();
        assert_eq!(ids, ["d", "a", "b", "c", "e"]);
        assert_eq!(
            positions_2306(&settings),
            [Some(0), Some(1), Some(2), Some(3), Some(4)]
        );

        let ids = reorder_registered_agent(&mut settings, "a", &ids, 4).unwrap();
        assert_eq!(ids, ["d", "b", "c", "e", "a"]);
        assert_eq!(
            positions_2306(&settings),
            [Some(0), Some(1), Some(2), Some(3), Some(4)]
        );
    }

    #[test]
    fn reorder_rejections_leave_the_live_settings_byte_identical() {
        let fresh = ["a", "b", "c", "d", "e"];
        let cases: [(&str, &[&str], usize, &str); 5] = [
            ("b", &["a", "x", "c", "d", "e"], 3, "changed since"),
            ("b", &["b", "a", "c", "d", "e"], 3, "changed since"),
            ("missing", &fresh, 1, "not found"),
            ("b", &fresh, 5, "out of range"),
            ("b", &fresh, 1, "nothing to reorder"),
        ];
        for (id, expected_ids, target, needle) in cases {
            let mut settings = reorder_fixture_2542();
            let before = serde_json::to_string(&settings.agents).unwrap();
            let error =
                reorder_registered_agent(&mut settings, id, &owned_ids_2542(expected_ids), target)
                    .unwrap_err();
            assert!(
                error.contains(needle),
                "case ({id},{target}): error {error:?} must mention {needle:?}"
            );
            assert_eq!(
                serde_json::to_string(&settings.agents).unwrap(),
                before,
                "case ({id},{target})"
            );
        }

        let mut duplicated = AppSettings {
            agents: vec![agent("dup", "D1", "claude"), agent("dup", "D2", "claude")],
            ..AppSettings::default()
        };
        let before = serde_json::to_string(&duplicated.agents).unwrap();
        let error =
            reorder_registered_agent(&mut duplicated, "dup", &owned_ids_2542(&["dup", "dup"]), 1)
                .unwrap_err();
        assert!(error.contains("more than once"), "{error}");
        assert_eq!(serde_json::to_string(&duplicated.agents).unwrap(), before);
    }

    #[test]
    fn reorder_derives_the_effective_order_from_unnormalized_input() {
        // Vector slots are [a, b, c] but the stored ordinals say [c, a, b]; the
        // caller's view and the move act on that effective order.
        let mut settings = ordered_2306();
        settings.agents[0].order = Some(1);
        settings.agents[1].order = Some(2);
        settings.agents[2].order = Some(0);

        let ids =
            reorder_registered_agent(&mut settings, "c", &owned_ids_2542(&["c", "a", "b"]), 2)
                .unwrap();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(positions_2306(&settings), [Some(0), Some(1), Some(2)]);
    }
}

fn source_discovery_evidence(
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

fn capture_source_inventory(
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

#[cfg(test)]
mod tests_source_transition_driver {
    use super::*;
    use crate::config::settings::*;

    fn r3_ready_fixture() -> (DriverFixture, String) {
        let fixture = DriverFixture::new();
        configure_source_durable_cut(Some(("prepared", 1)));
        let result = commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "r3-original",
            &SourceRevision::Absent {},
            &DriverFixture::candidate(),
            &|| Ok(()),
        );
        assert!(
            matches!(result, Err(SourceTransitionError::Interrupted { .. })),
            "r3 readiness result: {result:?}"
        );
        SOURCE_DURABLE_CUT.with(|cut| assert!(cut.borrow().0.is_none()));
        configure_source_durable_cut(None);
        let operation = fixture.operation();
        assert_eq!(
            get_identity_transition(&fixture.context, &fixture.source, &operation)
                .unwrap()
                .state,
            IdentityTransitionReadState::Prepared
        );
        (fixture, operation)
    }

    fn r3_prepared_fixture() -> (DriverFixture, String, std::path::PathBuf) {
        let (fixture, operation) = r3_ready_fixture();
        let native = crate::config::local_config_io::source_checkpoint_native_calls();
        {
            let identity =
                IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
            let lease =
                SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
            let intent = read_own_source_intent(&fixture.context, &operation)
                .unwrap()
                .1
                .unwrap();
            let cut = || {
                if read_own_source_journal(&fixture.context)
                    .unwrap()
                    .1
                    .progress[&operation]
                    .source_publication
                    .is_some()
                {
                    Err(IdentityReferenceError::Conflict)
                } else {
                    Ok(())
                }
            };
            assert!(
                publish_source_intent_with_validation(&lease, &fixture.context, &intent, &cut)
                    .is_err()
            );
            assert_eq!(lease.source_revision().unwrap(), SourceRevision::Absent {});
        }
        assert_eq!(
            crate::config::local_config_io::source_checkpoint_native_calls(),
            native
        );
        let journal = read_own_source_journal(&fixture.context).unwrap().1;
        let proof = journal.progress[&operation]
            .source_publication
            .as_ref()
            .unwrap();
        let value = serde_json::to_value(proof).unwrap();
        let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        let stage = target
            .parent()
            .unwrap()
            .join(value["stageLeaf"].as_str().unwrap());
        assert_eq!(
            std::fs::read(&stage).unwrap(),
            journal.intents[&operation].serialized_after
        );
        (fixture, operation, stage)
    }

    fn r3_assert_pending(fixture: &DriverFixture, operation: &str) {
        let journal = read_own_source_journal(&fixture.context).unwrap().1;
        let progress = &journal.progress[operation];
        assert!(progress.packets_prepared && !progress.source_published);
        assert!(!progress.refs_verified && !progress.reloaded && progress.receipt.is_none());
    }

    #[test]
    fn single_editor_absent_foreign_identical_refusal() {
        // Calibrate the last premove validation on this same synchronous path.
        let (control, control_op, _) = r3_prepared_fixture();
        let premove = std::cell::Cell::new(0);
        let calls = std::cell::Cell::new(0);
        {
            let identity =
                IdentityWriteLease::acquire(&control.context, std::time::Duration::ZERO).unwrap();
            let lease =
                SourceWriteContext::acquire(&control.context, &control.source, &identity).unwrap();
            let intent = read_own_source_intent(&control.context, &control_op)
                .unwrap()
                .1
                .unwrap();
            let observe = || {
                calls.set(calls.get() + 1);
                if lease.source_revision().unwrap() == (SourceRevision::Absent {}) {
                    premove.set(calls.get());
                }
                Ok(())
            };
            publish_source_intent_with_validation(&lease, &control.context, &intent, &observe)
                .unwrap();
        }
        assert!(premove.get() > 0);
        let (fixture, operation, stage) = r3_prepared_fixture();
        let parent = crate::path_identity::retain_directory(stage.parent().unwrap()).unwrap();
        let own_object = parent.verify_regular_file(&stage).unwrap();
        let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let after = original.intents[&operation].serialized_after.clone();
        let native = crate::config::local_config_io::source_checkpoint_native_calls();
        {
            let identity =
                IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
            let lease =
                SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
            let calls = std::cell::Cell::new(0);
            let interfere = || {
                calls.set(calls.get() + 1);
                if calls.get() == premove.get() {
                    std::fs::write(&target, &after).unwrap();
                }
                Ok(())
            };
            assert!(publish_source_intent_with_validation(
                &lease,
                &fixture.context,
                &original.intents[&operation],
                &interfere
            )
            .is_err());
            assert_eq!(
                crate::config::local_config_io::source_checkpoint_native_calls(),
                native + 1
            );
            let foreign = parent.verify_regular_file(&target).unwrap();
            assert!(!crate::path_identity::same_object(&own_object, &foreign));
            assert!(publish_source_intent_with_validation(
                &lease,
                &fixture.context,
                &original.intents[&operation],
                &|| Ok(())
            )
            .is_err());
            assert_eq!(parent.verify_regular_file(&target).unwrap(), foreign);
        }
        for _ in 0..2 {
            let reloads = std::cell::Cell::new(0);
            assert!(resume_identity_transition_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                &operation,
                &fixture.expectation(),
                &|| {
                    reloads.set(reloads.get() + 1);
                    Ok(())
                }
            )
            .is_err());
            assert_eq!(reloads.get(), 0);
            r3_assert_pending(&fixture, &operation);
        }
        assert_eq!(
            crate::config::local_config_io::source_checkpoint_native_calls(),
            native + 1
        );
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            original
        );
        assert_eq!(std::fs::read(&target).unwrap(), after);
        assert_eq!(parent.verify_regular_file(&stage).unwrap(), own_object);
        assert_eq!(std::fs::read(&stage).unwrap(), after);
    }

    #[test]
    fn single_editor_absent_ownmove_restart() {
        let (fixture, operation, stage) = r3_prepared_fixture();
        let parent = crate::path_identity::retain_directory(stage.parent().unwrap()).unwrap();
        let staged = parent.verify_regular_file(&stage).unwrap();
        let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let native = crate::config::local_config_io::source_checkpoint_native_calls();
        {
            let identity =
                IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
            let lease =
                SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
            let cut = || {
                if target.exists() {
                    Err(IdentityReferenceError::Conflict)
                } else {
                    Ok(())
                }
            };
            assert!(publish_source_intent_with_validation(
                &lease,
                &fixture.context,
                &original.intents[&operation],
                &cut
            )
            .is_err());
        }
        assert!(!stage.exists());
        assert!(crate::path_identity::same_object(
            &staged,
            &parent.verify_regular_file(&target).unwrap()
        ));
        r3_assert_pending(&fixture, &operation);
        fixture.resume_at(&fixture.context, &operation).unwrap();
        let complete = read_own_source_journal(&fixture.context).unwrap().1;
        assert!(complete.progress[&operation].receipt.is_some());
        assert_eq!(complete.intents[&operation], original.intents[&operation]);
        assert_eq!(
            complete.progress[&operation].source_publication,
            original.progress[&operation].source_publication
        );
        assert_eq!(
            crate::config::local_config_io::source_checkpoint_native_calls(),
            native + 1
        );
    }

    #[test]
    fn single_editor_r3_checkpoint_same_stage() {
        let (fixture, operation, stage) = r3_prepared_fixture();
        let parent = crate::path_identity::retain_directory(stage.parent().unwrap()).unwrap();
        let staged = parent.verify_regular_file(&stage).unwrap();
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let bytes = &original.intents[&operation].serialized_after;
        let native = crate::config::local_config_io::source_checkpoint_native_calls();
        std::fs::write(&stage, b"{\"modified\":true}").unwrap();
        assert!(fixture.resume_at(&fixture.context, &operation).is_err());
        r3_assert_pending(&fixture, &operation);
        assert_eq!(
            crate::config::local_config_io::source_checkpoint_native_calls(),
            native
        );
        std::fs::write(&stage, bytes).unwrap();
        assert!(crate::path_identity::same_object(
            &staged,
            &parent.verify_regular_file(&stage).unwrap()
        ));
        let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
        fixture.resume_at(&fixture.context, &operation).unwrap();
        let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        assert!(!stage.exists());
        assert!(crate::path_identity::same_object(
            &staged,
            &parent.verify_regular_file(&target).unwrap()
        ));
        let complete = read_own_source_journal(&fixture.context).unwrap().1;
        assert!(complete.progress[&operation].receipt.is_some());
        assert_eq!(
            complete.progress[&operation].source_publication,
            original.progress[&operation].source_publication
        );
        assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
        assert_eq!(
            crate::config::local_config_io::source_checkpoint_native_calls(),
            native + 1
        );
    }

    #[test]
    fn single_editor_r3_stage_missing_replaced_modified() {
        for mode in ["missing", "replaced", "modified"] {
            let (fixture, operation, stage) = r3_prepared_fixture();
            let journal = read_own_source_journal(&fixture.context).unwrap().1;
            let parent = crate::path_identity::retain_directory(stage.parent().unwrap()).unwrap();
            let own = parent.verify_regular_file(&stage).unwrap();
            if mode == "modified" {
                std::fs::write(&stage, b"{\"external\":true}").unwrap();
            } else {
                std::fs::rename(&stage, stage.with_extension("external-diagnostic")).unwrap();
                if mode == "replaced" {
                    std::fs::write(&stage, &journal.intents[&operation].serialized_after).unwrap();
                    assert!(!crate::path_identity::same_object(
                        &own,
                        &parent.verify_regular_file(&stage).unwrap()
                    ));
                }
            }
            let before = std::fs::read(&stage).ok();
            let native = crate::config::local_config_io::source_checkpoint_native_calls();
            let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
            for _ in 0..2 {
                assert!(fixture.resume_at(&fixture.context, &operation).is_err());
            }
            assert_eq!(
                read_own_source_journal(&fixture.context).unwrap().1,
                journal
            );
            assert_eq!(std::fs::read(&stage).ok(), before);
            assert_eq!(
                crate::config::local_config_io::source_checkpoint_native_calls(),
                native
            );
            assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
            r3_assert_pending(&fixture, &operation);
        }
    }

    #[test]
    fn single_editor_r3_cas_error_classification() {
        // Real Err-landed checkpoint is created by the existing validation cut.
        let (after_fixture, after_operation, after_stage) = r3_prepared_fixture();
        r3_assert_pending(&after_fixture, &after_operation);
        assert!(after_stage.exists());
        for mode in ["before", "other", "sticky"] {
            let (fixture, operation) = r3_ready_fixture();
            let before = read_own_source_journal(&fixture.context).unwrap();
            let native = crate::config::local_config_io::source_checkpoint_native_calls();
            let reached = std::cell::Cell::new(false);
            let private_path = fixture
                ._root
                .path()
                .join("C")
                .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
            {
                let identity =
                    IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO)
                        .unwrap();
                let lease =
                    SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity)
                        .unwrap();
                let cut = || {
                    let metadata_stage = std::fs::read_dir(private_path.parent().unwrap())
                        .unwrap()
                        .filter_map(Result::ok)
                        .map(|e| e.path())
                        .any(|p| {
                            p.extension().is_some_and(|e| e == "tmp")
                                && std::fs::read(&p).ok().is_some_and(|b| {
                                    serde_json::from_slice::<serde_json::Value>(&b)
                                        .ok()
                                        .is_some_and(|v| {
                                            v["progress"][&operation]["sourcePublication"]
                                                .is_object()
                                        })
                                })
                        });
                    if metadata_stage && !reached.replace(true) {
                        if mode == "other" {
                            std::fs::write(&private_path, b"{broken").unwrap();
                        }
                        return Err(IdentityReferenceError::Conflict);
                    }
                    if mode == "sticky" && reached.get() {
                        panic!("validation re-entered after sticky refusal");
                    }
                    Ok(())
                };
                assert!(publish_source_intent_with_validation(
                    &lease,
                    &fixture.context,
                    &before.1.intents[&operation],
                    &cut
                )
                .is_err());
                assert!(reached.get());
            }
            assert_eq!(
                crate::config::local_config_io::source_checkpoint_native_calls(),
                native
            );
            if mode == "other" {
                assert_eq!(std::fs::read(&private_path).unwrap(), b"{broken");
            } else {
                assert_eq!(read_own_source_journal(&fixture.context).unwrap(), before);
                r3_assert_pending(&fixture, &operation);
            }
        }
    }

    #[test]
    fn single_editor_r3_checkpointed_stage_never_unlinked() {
        let (fixture, operation, stage) = r3_prepared_fixture();
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let parent = crate::path_identity::retain_directory(stage.parent().unwrap()).unwrap();
        let target = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        let owned = parent.verify_regular_file(&stage).unwrap();
        let proof = original.progress[&operation]
            .source_publication
            .as_ref()
            .unwrap();
        let refusal = std::cell::Cell::new(false);
        let validate = || {
            if refusal.get() {
                Err(IdentityReferenceError::Conflict)
            } else {
                Ok(())
            }
        };
        let native = crate::config::local_config_io::source_checkpoint_native_calls();
        {
            let mut capability = crate::config::local_config_io::reopen_source_publication(
                &parent,
                &target,
                &original.intents[&operation].serialized_after,
                proof,
                &validate,
            )
            .unwrap();
            refusal.set(true);
            assert!(capability.commit(&|| Ok(())).is_err());
            refusal.set(false);
            assert!(capability.commit(&|| Ok(())).is_err());
        }
        assert_eq!(parent.verify_regular_file(&stage).unwrap(), owned);
        assert_eq!(
            std::fs::read(&stage).unwrap(),
            original.intents[&operation].serialized_after
        );
        assert!(!target.exists());
        assert_eq!(
            crate::config::local_config_io::source_checkpoint_native_calls(),
            native
        );
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            original
        );
    }

    #[test]
    fn single_editor_r3_stale_progress_preserves_proof() {
        let (fixture, operation, _) = r3_prepared_fixture();
        let identity =
            IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
        let lease =
            SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
        let (stale, original) = read_own_source_journal(&fixture.context).unwrap();
        let private_path = fixture
            ._root
            .path()
            .join("C")
            .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
        let mut bytes = std::fs::read(&private_path).unwrap();
        bytes.push(10);
        std::fs::write(&private_path, bytes).unwrap();
        let fresh = read_own_source_journal(&fixture.context).unwrap().0;
        for field in ["packets", "aggregate", "refs", "reload", "receipt"] {
            assert_eq!(
                update_own_source_journal_with_validation(
                    &identity,
                    &lease,
                    &fixture.context,
                    &stale,
                    |journal| {
                        let progress = journal.progress.get_mut(&operation).unwrap();
                        match field {
                            "packets" => progress.packets.clear(),
                            "aggregate" => progress.packets_prepared = false,
                            "refs" => progress.refs_verified = false,
                            "reload" => progress.reloaded = true,
                            _ => progress.receipt = None,
                        }
                        Ok(())
                    },
                    &|| Ok(())
                ),
                Err(SourceCoordinationError::Stale)
            );
        }
        assert_eq!(
            update_own_source_journal_with_validation(
                &identity,
                &lease,
                &fixture.context,
                &fresh,
                |journal| {
                    journal
                        .progress
                        .get_mut(&operation)
                        .unwrap()
                        .source_publication = None;
                    Ok(())
                },
                &|| Ok(())
            ),
            Err(SourceCoordinationError::Invalid)
        );
        let current = read_own_source_journal(&fixture.context).unwrap().1;
        assert_eq!(
            current.progress[&operation].source_publication,
            original.progress[&operation].source_publication
        );
        assert_eq!(current.intents, original.intents);

        // The private journal spans sources; this lease's ledger does not.
        // An unchanged proof belonging to another source remains valid.
        let registered_path = fixture
            ._root
            .path()
            .join("C")
            .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME);
        std::fs::write(&registered_path, b"{\"agents\":[]}").unwrap();
        let other = list_active_sources(&fixture.context)
            .into_iter()
            .find(|source| source.kind == SourceKind::RegisteredInstance)
            .unwrap();
        assert_ne!(other, fixture.source);
        let other_lease = SourceWriteContext::acquire(&fixture.context, &other, &identity).unwrap();
        assert_eq!(
            update_own_source_journal_with_validation(
                &identity,
                &other_lease,
                &fixture.context,
                &fresh,
                |_| Ok(()),
                &|| Ok(())
            ),
            Ok(fresh)
        );
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            current
        );
    }

    #[test]
    fn single_editor_r3_legacy_none_before_after() {
        for foreign_after in [false, true] {
            let (fixture, operation) = r3_ready_fixture();
            let journal = read_own_source_journal(&fixture.context).unwrap().1;
            assert!(journal.progress[&operation].source_publication.is_none());
            let legacy = serde_json::to_value(&journal.progress[&operation]).unwrap();
            assert!(legacy.get("sourcePublication").is_none());
            let native = crate::config::local_config_io::source_checkpoint_native_calls();
            if foreign_after {
                let target =
                    source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
                std::fs::write(&target, &journal.intents[&operation].serialized_after).unwrap();
                assert!(fixture.resume_at(&fixture.context, &operation).is_err());
                assert_eq!(
                    read_own_source_journal(&fixture.context).unwrap().1,
                    journal
                );
                assert_eq!(
                    crate::config::local_config_io::source_checkpoint_native_calls(),
                    native
                );
                r3_assert_pending(&fixture, &operation);
            } else {
                fixture.resume_at(&fixture.context, &operation).unwrap();
                let complete = read_own_source_journal(&fixture.context).unwrap().1;
                assert!(complete.progress[&operation].source_publication.is_some());
                assert!(complete.progress[&operation].receipt.is_some());
                assert_eq!(
                    crate::config::local_config_io::source_checkpoint_native_calls(),
                    native + 1
                );
            }
        }
    }

    struct DriverFixture {
        _root: tempfile::TempDir,
        settings: AppSettings,
        context: SourceReadContext,
        source: SourceRef,
        project: std::path::PathBuf,
    }

    impl DriverFixture {
        fn new() -> Self {
            let root = crate::config::settings::source_transition_test_root();
            let project = root.path().join("project");
            let config = root.path().join("C");
            std::fs::create_dir_all(project.join(".ac/coding-agents")).unwrap();
            std::fs::create_dir(&config).unwrap();
            let settings = AppSettings {
                project_paths: vec![project.to_string_lossy().into_owned()],
                ..AppSettings::default()
            };
            let context =
                SourceReadContext::from_settings(&settings, &config.join("settings.json")).unwrap();
            let source = list_active_sources(&context)
                .into_iter()
                .find(|source| source.kind == SourceKind::CatalogPersonal)
                .unwrap();
            Self {
                _root: root,
                settings,
                context,
                source,
                project,
            }
        }
        fn candidate() -> serde_json::Value {
            serde_json::json!({"schemaVersion": 2, "agents": []})
        }
        fn start(&self, request: &str) -> IdentityTransitionRead {
            let revision = read_source_snapshot(&self.context, &self.source)
                .unwrap()
                .snapshot
                .revision;
            commit_source_candidate_internal(
                &self.settings,
                &self.context,
                &self.source,
                request,
                &revision,
                &Self::candidate(),
                &|| Ok(()),
            )
            .unwrap()
        }
        fn expectation(&self) -> SourceResumeExpectation {
            let identity =
                IdentityWriteLease::acquire(&self.context, std::time::Duration::ZERO).unwrap();
            let lease =
                SourceWriteContext::acquire(&self.context, &self.source, &identity).unwrap();
            SourceResumeExpectation {
                source: lease.source_revision().unwrap(),
                journal: read_own_source_journal(&self.context).unwrap().0,
            }
        }
        fn operation(&self) -> String {
            read_own_source_journal(&self.context)
                .unwrap()
                .1
                .intents
                .keys()
                .next()
                .unwrap()
                .clone()
        }
    }

    #[test]
    fn single_editor_complete_without_participants() {
        let fixture = DriverFixture::new();
        let reloads = std::cell::Cell::new(0);
        let result = commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "request-empty",
            &SourceRevision::Absent {},
            &DriverFixture::candidate(),
            &|| {
                reloads.set(reloads.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        let operation = fixture.operation();
        let journal = read_own_source_journal(&fixture.context).unwrap().1;
        assert!(journal.inventories[&operation].targets.is_empty());
        let progress = &journal.progress[&operation];
        assert!(progress.packets.is_empty());
        assert!(progress.refs_verified && progress.reloaded && progress.receipt.is_some());
        assert_eq!(reloads.get(), 1);
        assert_ne!(result.state, IdentityTransitionReadState::NotFound);
        // Real completed Resume must recognize after without calling the publisher.
        let publishers = source_transition_effect_counts().1;
        let expected = fixture.expectation();
        resume_identity_transition_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            &operation,
            &expected,
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(source_transition_effect_counts().1, publishers);
    }

    #[test]
    fn single_editor_driver_request_lookup_before_uuid_and_new_click_has_new_identity() {
        let fixture = DriverFixture::new();
        fixture.start("request-1");
        let operation = fixture.operation();
        let count = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
        commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "request-1",
            &SourceRevision::Absent {},
            &DriverFixture::candidate(),
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), count);
        assert!(matches!(
            commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "request-1",
                &SourceRevision::Absent {},
                &serde_json::Value::Null,
                &|| Ok(())
            ),
            Err(SourceTransitionError::Coordination(
                SourceCoordinationError::Invalid
            ))
        ));
        fixture.start("request-2");
        let journal = read_own_source_journal(&fixture.context).unwrap().1;
        assert_eq!(journal.intents.len(), 2);
        assert!(journal.intents.contains_key(&operation));
        assert_eq!(
            SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get),
            count + 1
        );
    }

    #[test]
    fn single_editor_driver_complete_resume_rejects_source_z_and_root_replacement() {
        let fixture = DriverFixture::new();
        fixture.start("request-z");
        let operation = fixture.operation();
        let source_path =
            source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        std::fs::write(&source_path, b"{\"unrelated\":\"Z\"}").unwrap();
        let expected = fixture.expectation();
        let count = source_transition_effect_counts().1;
        assert!(resume_identity_transition_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            &operation,
            &expected,
            &|| Ok(())
        )
        .is_err());
        assert_eq!(
            std::fs::read(&source_path).unwrap(),
            b"{\"unrelated\":\"Z\"}"
        );
        assert_eq!(source_transition_effect_counts().1, count);
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        assert!(original.progress[&operation].receipt.is_some());
    }
    #[test]
    fn single_editor_driver_actual_guard_cuts_preserve_original_and_resume_without_new_uuid() {
        configure_source_factory_guard_cut(None);
        let baseline = DriverFixture::new();
        baseline.start("baseline");
        let cuts = source_factory_guard_calls();
        assert!(cuts > 0);
        for cut in 1..=cuts {
            let fixture = DriverFixture::new();
            configure_source_factory_guard_cut(Some(cut));
            let result = commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "request-cut",
                &SourceRevision::Absent {},
                &DriverFixture::candidate(),
                &|| Ok(()),
            );
            assert!(
                !source_factory_guard_cut_armed(),
                "actual guard cut {cut} was not reached"
            );
            assert!(result.is_err(), "guard refusal was swallowed at cut {cut}");
            configure_source_factory_guard_cut(None);
            let (_, original) = read_own_source_journal(&fixture.context).unwrap();
            if let Some((operation, intent)) = original.intents.iter().next() {
                let operation = operation.clone();
                let intent = intent.clone();
                let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
                let expected = fixture.expectation();
                resume_identity_transition_internal(
                    &fixture.settings,
                    &fixture.context,
                    &fixture.source,
                    &operation,
                    &expected,
                    &|| Ok(()),
                )
                .unwrap();
                let recovered = read_own_source_journal(&fixture.context).unwrap().1;
                assert_eq!(recovered.intents[&operation], intent);
                assert_eq!(
                    recovered.inventories[&operation],
                    original.inventories[&operation]
                );
                assert!(recovered.progress[&operation].receipt.is_some());
                assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
                let publisher_calls = source_transition_effect_counts().1;
                let expected = fixture.expectation();
                resume_identity_transition_internal(
                    &fixture.settings,
                    &fixture.context,
                    &fixture.source,
                    &operation,
                    &expected,
                    &|| Ok(()),
                )
                .unwrap();
                assert_eq!(
                    source_transition_effect_counts().1,
                    publisher_calls,
                    "completed after recognition republished at cut {cut}"
                );
            } else {
                assert!(result.is_err());
                // An admission may exist, but no operation/source may be invented.
                assert_eq!(
                    read_source_snapshot(&fixture.context, &fixture.source)
                        .unwrap()
                        .snapshot
                        .revision,
                    SourceRevision::Absent {}
                );
                assert!(original.inventories.is_empty() && original.progress.is_empty());
            }
        }
    }

    #[test]
    fn single_editor_driver_rename_allowed_but_replaced_discovery_object_rejected() {
        let fixture = DriverFixture::new();
        fixture.start("request-root");
        let operation = fixture.operation();
        let new_source = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        let canonical_project = std::fs::canonicalize(&fixture.project).unwrap();
        let canonical_source = std::fs::canonicalize(&new_source).unwrap();
        let relative_source = canonical_source
            .strip_prefix(&canonical_project)
            .unwrap()
            .to_path_buf();
        let old = fixture.project.with_file_name("original-project");
        std::fs::rename(&fixture.project, &old).unwrap();
        std::fs::create_dir_all(fixture.project.join(".ac/coding-agents")).unwrap();
        let old_source = old.join(relative_source);
        std::fs::copy(&old_source, &new_source).unwrap();
        let expected = fixture.expectation();
        let before = source_transition_effect_counts().1;
        assert!(resume_identity_transition_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            &operation,
            &expected,
            &|| Ok(())
        )
        .is_err());
        assert_eq!(source_transition_effect_counts().1, before);
        std::fs::remove_file(&new_source).unwrap();
    }

    impl DriverFixture {
        fn with_reference_targets() -> (
            Self,
            serde_json::Value,
            std::path::PathBuf,
            std::path::PathBuf,
        ) {
            let fixture = Self::new();
            let before = serde_json::json!({"schemaVersion":2,"agents":[{
                "key":"custom-k","label":"Custom K","description":"Owned","color":"#123456",
                "command":"original-command","envs":[]}],
                "codingAgentProfiles":{"custom-k":{"A":{"command":"--profile", "env":{}}}}});
            let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
            std::fs::write(path, serde_json::to_vec(&before).unwrap()).unwrap();
            let snapshot = read_source_snapshot(&fixture.context, &fixture.source).unwrap();
            let selection = ConfigurationRef {
                version: 1,
                source: fixture.source.clone(),
                calculated_id: configuration_identity("original-command", &[]),
                requested_profile: Some("A".into()),
                profile_calculated_id: Some(profile_identity(
                    &snapshot.snapshot.profiles["custom-k"]["A"],
                )),
                legacy_entry_hint: Some("custom-k".into()),
            };
            let target = fixture.project.join(".ac/_agent_N");
            let disjoint = fixture.project.join(".ac/_agent_U");
            std::fs::create_dir(&target).unwrap();
            std::fs::create_dir(&disjoint).unwrap();
            std::fs::write(
                target.join("config.json"),
                serde_json::to_vec(&serde_json::json!({"unknown":null,
                "tooling":{"configurationRef":selection.clone(), "unknown":{"keep":[null,false]}}}))
                .unwrap(),
            )
            .unwrap();
            std::fs::write(
                target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
                serde_json::to_vec(
                    &serde_json::json!({"unknown":[null],"tooling":{"sourceReferences":[{
                    "configurationRef":selection.clone(), "unknown":true}]}}),
                )
                .unwrap(),
            )
            .unwrap();
            let mut foreign = selection;
            foreign.source = list_active_sources(&fixture.context)
                .into_iter()
                .find(|source| source.kind == SourceKind::CatalogBase)
                .unwrap();
            std::fs::write(
                disjoint.join("config.json"),
                serde_json::to_vec(&serde_json::json!({"unknown":true,
                "tooling":{"configurationRef":foreign}}))
                .unwrap(),
            )
            .unwrap();
            let mut candidate = before;
            candidate["agents"][0]["command"] = serde_json::json!("updated-command");
            (fixture, candidate, target, disjoint)
        }
    }

    #[test]
    fn single_editor_driver_actual_packet_changes_only_affected_target_then_releases_original() {
        let (fixture, candidate, target, disjoint) = DriverFixture::with_reference_targets();
        let untouched = std::fs::read(disjoint.join("config.json")).unwrap();
        let revision = read_source_snapshot(&fixture.context, &fixture.source)
            .unwrap()
            .snapshot
            .revision;
        let reloads = std::cell::Cell::new(0);
        commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "request-packet",
            &revision,
            &candidate,
            &|| {
                reloads.set(reloads.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        let operation = fixture.operation();
        let journal = read_own_source_journal(&fixture.context).unwrap().1;
        assert_eq!(journal.inventories[&operation].targets.len(), 1);
        assert_eq!(journal.progress[&operation].packets.len(), 1);
        assert_eq!(journal.progress[&operation].packets.len(), 1);
        assert!(journal.progress[&operation].receipt.is_some());
        assert_eq!(reloads.get(), 1);
        assert_eq!(
            std::fs::read(disjoint.join("config.json")).unwrap(),
            untouched
        );
        assert!(!disjoint
            .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
            .exists());
        let decisions: serde_json::Value =
            serde_json::from_slice(&std::fs::read(target.join("config.json")).unwrap()).unwrap();
        let state: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
            )
            .unwrap(),
        )
        .unwrap();
        let calculated = configuration_identity("updated-command", &[]);
        assert_eq!(
            decisions
                .pointer("/tooling/configurationRef/calculatedId")
                .and_then(serde_json::Value::as_str),
            Some(calculated.as_str())
        );
        assert_eq!(
            state
                .pointer("/tooling/sourceReferences/0/configurationRef/calculatedId")
                .and_then(serde_json::Value::as_str),
            Some(calculated.as_str())
        );
        assert_eq!(decisions["unknown"], serde_json::Value::Null);
        assert_eq!(state["unknown"], serde_json::json!([null]));
        assert!(!target
            .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
            .exists());
        let publishers = source_transition_effect_counts().1;
        let expected = fixture.expectation();
        resume_identity_transition_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            &operation,
            &expected,
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(source_transition_effect_counts().1, publishers);
    }

    #[test]
    fn single_editor_driver_nonempty_actual_pair_guard_cuts_resume_original_packets() {
        configure_source_factory_guard_cut(None);
        let (baseline, candidate, _, _) = DriverFixture::with_reference_targets();
        let revision = read_source_snapshot(&baseline.context, &baseline.source)
            .unwrap()
            .snapshot
            .revision;
        commit_source_candidate_internal(
            &baseline.settings,
            &baseline.context,
            &baseline.source,
            "baseline-packet",
            &revision,
            &candidate,
            &|| Ok(()),
        )
        .unwrap();
        let cuts = source_factory_guard_calls();
        assert!(cuts > 0);
        for cut in 1..=cuts {
            let (fixture, candidate, target, disjoint) = DriverFixture::with_reference_targets();
            let source_before = read_source_snapshot(&fixture.context, &fixture.source)
                .unwrap()
                .snapshot
                .revision;
            let unrelated = std::fs::read(disjoint.join("config.json")).unwrap();
            configure_source_factory_guard_cut(Some(cut));
            let failed = commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "request-packet-cut",
                &source_before,
                &candidate,
                &|| Ok(()),
            );
            assert!(
                !source_factory_guard_cut_armed(),
                "nonempty guard cut {cut} was not reached"
            );
            assert!(
                failed.is_err(),
                "nonempty guard refusal was swallowed at cut {cut}"
            );
            configure_source_factory_guard_cut(None);
            let original = read_own_source_journal(&fixture.context)
                .unwrap_or_else(|error| {
                    let journal_path =
                        fixture._root.path().join("C").join(
                            crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME,
                        );
                    let raw =
                        crate::path_identity::read_bounded_regular(&journal_path, 16 * 1024 * 1024)
                            .map(|_| ());
                    panic!(
                        "nonempty guard cut {cut}: journal read {error:?}; bounded read {raw:?}"
                    );
                })
                .1;
            if let Some((operation, intent)) = original.intents.iter().next() {
                let operation = operation.clone();
                let intent = intent.clone();
                let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
                let expected = fixture.expectation();
                resume_identity_transition_internal(
                    &fixture.settings,
                    &fixture.context,
                    &fixture.source,
                    &operation,
                    &expected,
                    &|| Ok(()),
                )
                .unwrap();
                let recovered = read_own_source_journal(&fixture.context).unwrap().1;
                assert_eq!(recovered.intents[&operation], intent);
                assert_eq!(
                    recovered.inventories[&operation],
                    original.inventories[&operation]
                );
                assert!(recovered.progress[&operation].receipt.is_some());
                if let Some(packet) = original.progress[&operation].packets.first() {
                    assert_eq!(&recovered.progress[&operation].packets[0], packet);
                }
                assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
                assert!(!target
                    .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
                    .exists());
                let calls = source_transition_effect_counts().1;
                let expected = fixture.expectation();
                resume_identity_transition_internal(
                    &fixture.settings,
                    &fixture.context,
                    &fixture.source,
                    &operation,
                    &expected,
                    &|| Ok(()),
                )
                .unwrap();
                assert_eq!(source_transition_effect_counts().1, calls);
            }
            assert_eq!(
                std::fs::read(disjoint.join("config.json")).unwrap(),
                unrelated
            );
        }
    }

    #[test]
    fn single_editor_driver_failed_reload_retains_pending_and_activity_max_through_release() {
        let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
        let revision = read_source_snapshot(&fixture.context, &fixture.source)
            .unwrap()
            .snapshot
            .revision;
        let state_path = target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME);
        let activity = "2026-10-11T06:00:00Z";
        let failure = commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "request-reload",
            &revision,
            &candidate,
            &|| {
                // Real physical interleaving, outside PAIR and the validation callback.
                let mut state: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
                state["tooling"]["lastAgentMessageAt"] = serde_json::json!(activity);
                std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
                Err(SourceCoordinationError::Io("reload interrupted".into()))
            },
        );
        assert!(matches!(
            failure,
            Err(SourceTransitionError::Interrupted { .. })
        ));
        let operation = fixture.operation();
        let before = read_own_source_journal(&fixture.context).unwrap().1;
        assert!(before.progress[&operation].refs_verified);
        assert!(!before.progress[&operation].reloaded);
        assert!(before.progress[&operation].receipt.is_none());
        assert!(target
            .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
            .exists());
        let expected = fixture.expectation();
        resume_identity_transition_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            &operation,
            &expected,
            &|| Ok(()),
        )
        .unwrap();
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
        assert_eq!(
            state
                .pointer("/tooling/lastAgentMessageAt")
                .and_then(serde_json::Value::as_str),
            Some(activity)
        );
        assert!(read_own_source_journal(&fixture.context)
            .unwrap()
            .1
            .progress[&operation]
            .receipt
            .is_some());
        assert!(!target
            .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
            .exists());
    }

    #[test]
    fn single_editor_original_factory_real_matrix_default_group_and_history_no_default() {
        for matrix in ["N", "K"] {
            let mut fixture = DriverFixture::new();
            let config = fixture._root.path().join("C");
            let target = fixture.project.join(".ac").join(format!("_agent_{matrix}"));
            std::fs::create_dir(&target).unwrap();
            let mut registered = serde_json::json!({"agents":[
                {"id":"K","label":"Own","color":"#123456","command":"own-base"},
                {"id":"J","label":"Other","color":"#123456","command":"other-base"}],
                "codingAgentProfiles":{"schemaVersion":2,"profilesByAgent":{"K":{
                    "A":{"command":"--alpha","env":{}},"B":{"command":"--beta","env":{}}},
                    "J":{"A":{"command":"--other","env":{}}}},"defaultProfileByAgent":{},
                    "identityByAgent":{"K":"legacy-k","J":"legacy-j"}}});
            registered["codingAgentProfiles"]["defaultProfileByAgent"][matrix] =
                serde_json::json!("B");
            let mut settings_value = serde_json::to_value(&fixture.settings).unwrap();
            settings_value["codingAgentProfiles"]["defaultProfileByAgent"][matrix] =
                serde_json::json!("B");
            fixture.settings = serde_json::from_value(settings_value.clone()).unwrap();
            std::fs::write(
                config.join("settings.json"),
                serde_json::to_vec(&settings_value).unwrap(),
            )
            .unwrap();
            std::fs::write(
                config.join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
                serde_json::to_vec(&registered).unwrap(),
            )
            .unwrap();
            std::fs::write(config.join(crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
                serde_json::to_vec(&serde_json::json!({"codingAgentProfiles":{"defaultProfileByAgent":{(matrix):"B"},
                    "profilesByAgent":{"K":{"B":{"command":"overlay-beta","env":{}}}}}})).unwrap()).unwrap();
            fixture.context =
                SourceReadContext::from_settings(&fixture.settings, &config.join("settings.json"))
                    .unwrap();
            fixture.source = list_active_sources(&fixture.context)
                .into_iter()
                .find(|source| source.kind == SourceKind::RegisteredInstance)
                .unwrap();
            let snapshot = read_source_snapshot(&fixture.context, &fixture.source).unwrap();
            let inputs = OriginInputs::capture(&fixture.context).unwrap();
            let origin = LegacyCompatibilityOrigin::classify(&inputs);
            let identity =
                IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
            let lease =
                SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
            let selection = ConfigurationRef {
                version: 1,
                source: fixture.source.clone(),
                calculated_id: configuration_identity("own-base", &[]),
                requested_profile: Some("B".into()),
                profile_calculated_id: Some(profile_identity(
                    &snapshot.snapshot.profiles["K"]["B"],
                )),
                legacy_entry_hint: Some("K".into()),
            };
            let mut candidate = registered.clone();
            candidate["codingAgentProfiles"]["profilesByAgent"]["K"]["B"]["command"] =
                serde_json::json!("new-beta");
            assert!(prepare_source_candidate(
                &snapshot,
                &snapshot.snapshot.revision,
                &candidate,
                Some(&origin)
            )
            .is_err());
            std::fs::write(target.join("config.json"), serde_json::to_vec(&serde_json::json!({"tooling":{
                "replicaSelectionDefault":{"codingAgentId":"K","profile":"B","configurationRef":selection.clone()}}})).unwrap()).unwrap();
            let capture = capture_source_inventory(
                &fixture.settings,
                &snapshot,
                &origin,
                lease.physical_source_key(),
            )
            .unwrap();
            prepare_source_candidate_with_original_links(
                &snapshot,
                &snapshot.snapshot.revision,
                &candidate,
                Some(&origin),
                Some(capture.original_links()),
            )
            .unwrap();
            let mut disjoint_j = registered.clone();
            disjoint_j["agents"][1]["command"] = serde_json::json!("other-new-base");
            prepare_source_candidate_with_original_links(
                &snapshot,
                &snapshot.snapshot.revision,
                &disjoint_j,
                Some(&origin),
                Some(capture.original_links()),
            )
            .unwrap();
            let mut direct_default = registered.clone();
            direct_default["codingAgentProfiles"]["defaultProfileByAgent"][matrix] =
                serde_json::json!("A");
            prepare_source_candidate_with_original_links(
                &snapshot,
                &snapshot.snapshot.revision,
                &direct_default,
                Some(&origin),
                Some(capture.original_links()),
            )
            .unwrap();

            let overlay_path =
                config.join(crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME);
            let overlay_before = std::fs::read(&overlay_path).unwrap();
            let mut drift: serde_json::Value = serde_json::from_slice(&overlay_before).unwrap();
            drift["logLevel"] = serde_json::json!("debug");
            std::fs::write(&overlay_path, serde_json::to_vec(&drift).unwrap()).unwrap();
            let changed_inputs = OriginInputs::capture(&fixture.context).unwrap();
            let changed_origin = LegacyCompatibilityOrigin::classify(&changed_inputs);
            assert!(prepare_source_candidate_with_original_links(
                &snapshot,
                &snapshot.snapshot.revision,
                &candidate,
                Some(&changed_origin),
                Some(capture.original_links())
            )
            .is_err());
            std::fs::write(&overlay_path, &overlay_before).unwrap();
            let mut missing_letter = selection.clone();
            missing_letter.requested_profile = None;
            missing_letter.profile_calculated_id = None;
            std::fs::write(target.join("config.json"), serde_json::to_vec(&serde_json::json!({"tooling":{
                "replicaSelectionDefault":{"codingAgentId":"K","configurationRef":missing_letter}}})).unwrap()).unwrap();
            let missing = capture_source_inventory(
                &fixture.settings,
                &snapshot,
                &origin,
                lease.physical_source_key(),
            )
            .unwrap();
            assert!(prepare_source_candidate_with_original_links(
                &snapshot,
                &snapshot.snapshot.revision,
                &candidate,
                Some(&origin),
                Some(missing.original_links())
            )
            .is_err());
            let mut both: serde_json::Value = serde_json::from_slice(&overlay_before).unwrap();
            both["codingAgentProfiles"]["profilesByAgent"]["K"]["A"] =
                serde_json::json!({"command":"overlay-alpha","env":{}});
            std::fs::write(&overlay_path, serde_json::to_vec(&both).unwrap()).unwrap();
            let both_inputs = OriginInputs::capture(&fixture.context).unwrap();
            let both_origin = LegacyCompatibilityOrigin::classify(&both_inputs);
            let mut alpha = selection.clone();
            alpha.requested_profile = Some("A".into());
            alpha.profile_calculated_id =
                Some(profile_identity(&snapshot.snapshot.profiles["K"]["A"]));
            std::fs::write(target.join("config.json"), serde_json::to_vec(&serde_json::json!({"tooling":{
                "replicaSelectionDefault":{"codingAgentId":"K","profile":"A","configurationRef":alpha}}})).unwrap()).unwrap();
            let alpha_capture = capture_source_inventory(
                &fixture.settings,
                &snapshot,
                &both_origin,
                lease.physical_source_key(),
            )
            .unwrap();
            assert!(
                prepare_source_candidate_with_original_links(
                    &snapshot,
                    &snapshot.snapshot.revision,
                    &candidate,
                    Some(&both_origin),
                    Some(alpha_capture.original_links())
                )
                .is_err(),
                "A proof covered changed non-disjoint B"
            );
            std::fs::write(&overlay_path, &overlay_before).unwrap();
            std::fs::write(target.join("config.json"), b"{}").unwrap();
            std::fs::write(target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
                serde_json::to_vec(&serde_json::json!({"tooling":{"sourceReferences":[{"configurationRef":selection}]}})).unwrap()).unwrap();
            let history = capture_source_inventory(
                &fixture.settings,
                &snapshot,
                &origin,
                lease.physical_source_key(),
            )
            .unwrap();
            assert!(prepare_source_candidate_with_original_links(
                &snapshot,
                &snapshot.snapshot.revision,
                &candidate,
                Some(&origin),
                Some(history.original_links())
            )
            .is_err());
            std::fs::write(
                target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME),
                b"{}",
            )
            .unwrap();
            let replica = fixture
                .project
                .join(".ac/wg-7-proof")
                .join(format!("__agent_{matrix}"));
            std::fs::create_dir_all(&replica).unwrap();
            let replica_selection = ConfigurationRef {
                version: 1,
                source: fixture.source.clone(),
                calculated_id: configuration_identity("own-base", &[]),
                requested_profile: Some("B".into()),
                profile_calculated_id: Some(profile_identity(
                    &snapshot.snapshot.profiles["K"]["B"],
                )),
                legacy_entry_hint: Some("K".into()),
            };
            std::fs::write(
                replica.join("config.json"),
                serde_json::to_vec(&serde_json::json!({
                "identity":format!("../../_agent_{matrix}"),"tooling":{"replicaSelectionDefault":{
                    "codingAgentId":"K","profile":"B","configurationRef":replica_selection}}}))
                .unwrap(),
            )
            .unwrap();
            let replica_capture = capture_source_inventory(
                &fixture.settings,
                &snapshot,
                &origin,
                lease.physical_source_key(),
            )
            .unwrap();
            assert!(
                prepare_source_candidate_with_original_links(
                    &snapshot,
                    &snapshot.snapshot.revision,
                    &candidate,
                    Some(&origin),
                    Some(replica_capture.original_links())
                )
                .is_err(),
                "replica minted Matrix Default proof"
            );
        }
    }

    impl DriverFixture {
        fn resume_at(
            &self,
            context: &SourceReadContext,
            operation: &str,
        ) -> Result<IdentityTransitionRead, SourceTransitionError> {
            let expected = {
                let identity =
                    IdentityWriteLease::acquire(context, std::time::Duration::ZERO).unwrap();
                let lease = SourceWriteContext::acquire(context, &self.source, &identity).unwrap();
                SourceResumeExpectation {
                    source: lease.source_revision().unwrap(),
                    journal: read_own_source_journal(context).unwrap().0,
                }
            };
            resume_identity_transition_internal(
                &self.settings,
                context,
                &self.source,
                operation,
                &expected,
                &|| Ok(()),
            )
        }
    }

    #[test]
    fn single_editor_crash_resume_original() {
        let cuts = [
            ("beforeIntent", 1),
            ("intentInventory", 1),
            ("beforeFence", 1),
            ("packet", 1),
            ("packet", 2),
            ("allPackets", 1),
            ("prepared", 1),
            ("sourcePublished", 1),
            ("ref", 1),
            ("sourceAfter", 1),
            ("refsVerified", 1),
            ("reloaded", 1),
            ("receipt", 1),
            ("release", 1),
        ];
        let mut reached = 0;
        for cut in cuts {
            assert_source_durable_cut(cut);
            reached += 1;
        }
        assert_eq!(reached, cuts.len(), "not all durable cuts completed");
    }

    fn assert_source_durable_cut(cut: (&'static str, usize)) {
        let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
        let second = target.parent().unwrap().join("_agent_second");
        std::fs::create_dir(&second).unwrap();
        for name in [
            "config.json",
            crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME,
        ] {
            std::fs::copy(target.join(name), second.join(name)).unwrap();
        }
        let revision = read_source_snapshot(&fixture.context, &fixture.source)
            .unwrap()
            .snapshot
            .revision;
        configure_source_durable_cut(Some(cut));
        let result = commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "durable-boundary",
            &revision,
            &candidate,
            &|| Ok(()),
        );
        assert!(result.is_err(), "unreached cut: {cut:?}");
        SOURCE_DURABLE_CUT
            .with(|state| assert!(state.borrow().0.is_none(), "cut was not reached: {cut:?}"));
        configure_source_durable_cut(None);
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        if cut.0 == "beforeIntent" {
            assert!(original.intents.is_empty());
            assert!(!matches!(
                result,
                Err(SourceTransitionError::Interrupted { .. })
            ));
            assert_eq!(
                read_source_snapshot(&fixture.context, &fixture.source)
                    .unwrap()
                    .snapshot
                    .revision,
                revision
            );
            let fresh_revision = read_source_snapshot(&fixture.context, &fixture.source)
                .unwrap()
                .snapshot
                .revision;
            commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "new-after-before-intent",
                &fresh_revision,
                &candidate,
                &|| Ok(()),
            )
            .unwrap();
            return;
        }
        let operation = fixture.operation();
        assert!(
            matches!(result, Err(SourceTransitionError::Interrupted { operation_id, .. }) if operation_id == operation)
        );
        assert_eq!(original.inventories[&operation].targets.len(), 2);
        let observation =
            get_identity_transition(&fixture.context, &fixture.source, &operation).unwrap();
        assert_ne!(observation.state, IdentityTransitionReadState::NotFound);
        assert_original_durable_packets(&[target.as_path(), second.as_path()], &operation);
        if matches!(
            cut.0,
            "intentInventory" | "beforeFence" | "packet" | "allPackets" | "prepared"
        ) {
            assert_eq!(
                read_source_snapshot(&fixture.context, &fixture.source)
                    .unwrap()
                    .snapshot
                    .revision,
                revision
            );
        }
        let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
        fixture.resume_at(&fixture.context, &operation).unwrap();
        let recovered = read_own_source_journal(&fixture.context).unwrap().1;
        assert_eq!(recovered.intents[&operation], original.intents[&operation]);
        assert_eq!(
            recovered.inventories[&operation],
            original.inventories[&operation]
        );
        assert_eq!(recovered.progress[&operation].packets.len(), 2);
        assert!(recovered.progress[&operation].receipt.is_some());
        assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
        for root in [&target, &second] {
            assert!(!root
                .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
                .exists());
        }
    }

    fn assert_original_durable_packets(roots: &[&std::path::Path], operation: &str) {
        for root in roots {
            let reservation =
                root.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME);
            if reservation.exists() {
                let packet: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(reservation).unwrap()).unwrap();
                assert_eq!(packet["planImages"]["stages"].as_array().unwrap().len(), 5);
                assert_eq!(packet["protectedImages"].as_array().unwrap().len(), 5);
                assert!(packet.get("timestampFloor").is_some());
                assert_eq!(packet["operationId"].as_str(), Some(operation));
            }
        }
    }

    #[test]
    fn single_editor_driver_metadata_only_keeps_refs_bytes_and_runs_full_barrier() {
        let (fixture, _, target, _) = DriverFixture::with_reference_targets();
        let source_path =
            source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        let mut candidate: serde_json::Value =
            serde_json::from_slice(&std::fs::read(source_path).unwrap()).unwrap();
        candidate["agents"][0]["name"] = serde_json::json!("metadata renamed");
        let decisions = std::fs::read(target.join("config.json")).unwrap();
        let state =
            std::fs::read(target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME))
                .unwrap();
        let before = read_source_snapshot(&fixture.context, &fixture.source)
            .unwrap()
            .snapshot
            .revision;
        let reloads = std::cell::Cell::new(0);
        commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "metadata-only",
            &before,
            &candidate,
            &|| {
                reloads.set(reloads.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        let journal = read_own_source_journal(&fixture.context).unwrap().1;
        let operation = fixture.operation();
        assert!(journal.inventories[&operation].targets.is_empty());
        assert!(journal.progress[&operation].packets_prepared);
        assert!(journal.progress[&operation].packets.is_empty());
        assert!(journal.progress[&operation].receipt.is_some());
        assert_eq!(reloads.get(), 1);
        assert_eq!(
            std::fs::read(target.join("config.json")).unwrap(),
            decisions
        );
        assert_eq!(
            std::fs::read(target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME))
                .unwrap(),
            state
        );
    }

    #[test]
    fn single_editor_all_authority_context_source_target_cas_failures() {
        for drift in [
            "protected",
            "packet",
            "history-index",
            "source-Z",
            "settings-binding",
            "overlay-binding",
        ] {
            let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
            let bound_settings_path = fixture
                ._root
                .path()
                .join("C")
                .join(crate::config::instance_artifacts::SETTINGS_FILE_NAME);
            let settings_before = if drift == "settings-binding" {
                let before = read_identity_reference_physical(&bound_settings_path).unwrap();
                assert_eq!(
                    before,
                    IdentityReferencePhysicalState::Absent,
                    "bound settings must be physically absent before capture"
                );
                Some(before)
            } else {
                None
            };
            let revision = read_source_snapshot(&fixture.context, &fixture.source)
                .unwrap()
                .snapshot
                .revision;
            configure_source_durable_cut(Some(("packet", 1)));
            assert!(commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "authority-drift",
                &revision,
                &candidate,
                &|| Ok(())
            )
            .is_err());
            configure_source_durable_cut(None);
            let operation = fixture.operation();
            let original = read_own_source_journal(&fixture.context).unwrap().1;
            let path = match drift {
                "protected" => target.join("config.json"),
                "packet" => {
                    target.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
                }
                "history-index" => {
                    target.join(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME)
                }
                "source-Z" => {
                    source_transition_test_locator(&fixture.context, &fixture.source).unwrap()
                }
                "settings-binding" => bound_settings_path,
                _ => fixture
                    ._root
                    .path()
                    .join("C")
                    .join(crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
            };
            let changed = match drift {
                "packet" => b"{corrupt packet".to_vec(),
                "history-index" => {
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    value["tooling"]["sourceReferences"]
                        .as_array_mut()
                        .unwrap()
                        .insert(0, serde_json::json!({"unknown":"new slot"}));
                    serde_json::to_vec(&value).unwrap()
                }
                "protected" => {
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                    value["unknown"] = serde_json::json!("external protected drift");
                    serde_json::to_vec(&value).unwrap()
                }
                "source-Z" => {
                    serde_json::to_vec(&serde_json::json!({"schemaVersion":2,"agents":[]})).unwrap()
                }
                _ => br#"{"logLevel":"debug"}"#.to_vec(),
            };
            if let Some(before) = &settings_before {
                assert_eq!(
                    read_identity_reference_physical(&path).unwrap(),
                    *before,
                    "bound settings changed before external drift"
                );
                assert_ne!(
                    IdentityReferencePhysicalState::from_bytes(changed.clone()),
                    *before,
                    "settings drift must change physical state"
                );
            }
            std::fs::write(&path, &changed).unwrap();
            if settings_before.is_some() {
                assert_eq!(
                    read_identity_reference_physical(&path).unwrap(),
                    IdentityReferencePhysicalState::from_bytes(changed.clone()),
                    "bound settings drift was not persisted exactly"
                );
            }
            let publishers = source_transition_effect_counts().1;
            assert!(
                fixture.resume_at(&fixture.context, &operation).is_err(),
                "drift accepted: {drift}"
            );
            assert_eq!(source_transition_effect_counts().1, publishers);
            assert_eq!(std::fs::read(path).unwrap(), changed);
            assert_eq!(
                read_own_source_journal(&fixture.context).unwrap().1,
                original
            );
        }
    }

    #[test]
    fn single_editor_resume_each_stale_physical_cas_rejects_before_effect() {
        for stale in ["source", "journal"] {
            let fixture = DriverFixture::new();
            configure_source_durable_cut(Some(("intentInventory", 1)));
            assert!(commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "prefence-CAS",
                &SourceRevision::Absent {},
                &DriverFixture::candidate(),
                &|| Ok(())
            )
            .is_err());
            configure_source_durable_cut(None);
            let operation = fixture.operation();
            let journal = read_own_source_journal(&fixture.context).unwrap().1;
            let uuid_count = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
            assert!(matches!(
                commit_source_candidate_internal(
                    &fixture.settings,
                    &fixture.context,
                    &fixture.source,
                    "different-prefence-request",
                    &SourceRevision::Absent {},
                    &DriverFixture::candidate(),
                    &|| Ok(())
                ),
                Err(SourceTransitionError::Interrupted { .. })
            ));
            assert_eq!(
                SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get),
                uuid_count
            );
            let mut expected = fixture.expectation();
            let wrong = SourceRevision::Bytes {
                sha256: "0".repeat(64),
            };
            match stale {
                "source" => expected.source = wrong,
                "journal" => expected.journal = wrong,
                _ => expected.journal = wrong,
            }
            let publishers = source_transition_effect_counts().1;
            assert!(matches!(
                resume_identity_transition_internal(
                    &fixture.settings,
                    &fixture.context,
                    &fixture.source,
                    &operation,
                    &expected,
                    &|| Ok(())
                ),
                Err(SourceTransitionError::Coordination(
                    SourceCoordinationError::Stale
                ))
            ));
            assert_eq!(source_transition_effect_counts().1, publishers);
            assert_eq!(
                read_own_source_journal(&fixture.context).unwrap().1,
                journal
            );
        }
    }

    #[test]
    fn single_editor_registered_source_after_restores_original_groups_two_targets() {
        for matrix in ["N", "K"] {
            let mut fixture = DriverFixture::new();
            let config = fixture._root.path().join("C");
            let target = fixture.project.join(".ac").join(format!("_agent_{matrix}"));
            std::fs::create_dir(&target).unwrap();
            let mut registered = serde_json::json!({"agents":[
                {"id":"K","label":"Own","color":"#123456","command":"own-base"},
                {"id":"J","label":"Other","color":"#123456","command":"other-base"}],
                "codingAgentProfiles":{"schemaVersion":2,"profilesByAgent":{"K":{
                    "A":{"command":"--alpha","env":{}},"B":{"command":"--beta","env":{}}},
                    "J":{"A":{"command":"--other","env":{}}}},"defaultProfileByAgent":{},
                    "identityByAgent":{"K":"legacy-k","J":"legacy-j"}}});
            registered["codingAgentProfiles"]["defaultProfileByAgent"][matrix] =
                serde_json::json!("B");
            let mut settings_value = serde_json::to_value(&fixture.settings).unwrap();
            settings_value["codingAgentProfiles"]["defaultProfileByAgent"][matrix] =
                serde_json::json!("B");
            fixture.settings = serde_json::from_value(settings_value.clone()).unwrap();
            std::fs::write(
                config.join("settings.json"),
                serde_json::to_vec(&settings_value).unwrap(),
            )
            .unwrap();
            std::fs::write(
                config.join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME),
                serde_json::to_vec(&registered).unwrap(),
            )
            .unwrap();
            std::fs::write(config.join(crate::config::instance_artifacts::SETTINGS_LOCAL_OVERRIDE_FILE_NAME),
                serde_json::to_vec(&serde_json::json!({"codingAgentProfiles":{"defaultProfileByAgent":{(matrix):"B"},
                    "profilesByAgent":{"K":{"B":{"command":"overlay-beta","env":{}}}}}})).unwrap()).unwrap();
            fixture.context =
                SourceReadContext::from_settings(&fixture.settings, &config.join("settings.json"))
                    .unwrap();
            fixture.source = list_active_sources(&fixture.context)
                .into_iter()
                .find(|source| source.kind == SourceKind::RegisteredInstance)
                .unwrap();
            let snapshot = read_source_snapshot(&fixture.context, &fixture.source).unwrap();
            let identity =
                IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
            let lease =
                SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
            let selection = ConfigurationRef {
                version: 1,
                source: fixture.source.clone(),
                calculated_id: configuration_identity("own-base", &[]),
                requested_profile: Some("B".into()),
                profile_calculated_id: Some(profile_identity(
                    &snapshot.snapshot.profiles["K"]["B"],
                )),
                legacy_entry_hint: Some("K".into()),
            };
            let mut candidate = registered;
            candidate["codingAgentProfiles"]["profilesByAgent"]["K"]["B"]["command"] =
                serde_json::json!("new-beta");
            std::fs::write(target.join("config.json"), serde_json::to_vec(&serde_json::json!({"tooling":{
                "replicaSelectionDefault":{"codingAgentId":"K","profile":"B","configurationRef":selection.clone()}}})).unwrap()).unwrap();

            let second = fixture.project.join(".ac/_agent_second");
            std::fs::create_dir(&second).unwrap();
            std::fs::write(
                second.join("config.json"),
                serde_json::to_vec(&serde_json::json!({"tooling":{
                "codingAgentId":"K","configurationRef":selection},"unknown":{"keep":true}}))
                .unwrap(),
            )
            .unwrap();
            drop(lease);
            drop(identity);
            configure_source_durable_cut(Some(("sourceAfter", 1)));
            assert!(commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "registered-source-after",
                &snapshot.snapshot.revision,
                &candidate,
                &|| Ok(())
            )
            .is_err());
            configure_source_durable_cut(None);
            let operation = fixture.operation();
            let original = read_own_source_journal(&fixture.context).unwrap().1;
            assert_eq!(original.inventories[&operation].targets.len(), 2);
            assert_ne!(
                read_source_snapshot(&fixture.context, &fixture.source)
                    .unwrap()
                    .snapshot
                    .revision,
                snapshot.snapshot.revision
            );
            let publishers = source_transition_effect_counts().1;
            let source_publications = source_transition_publication_counts()[2];
            fixture.resume_at(&fixture.context, &operation).unwrap();
            assert_eq!(
                source_transition_publication_counts()[2],
                source_publications
            );
            let recovered = read_own_source_journal(&fixture.context).unwrap().1;
            assert_eq!(recovered.intents[&operation], original.intents[&operation]);
            assert_eq!(
                recovered.inventories[&operation],
                original.inventories[&operation]
            );
            assert!(recovered.progress[&operation].receipt.is_some());
            for root in [&target, &second] {
                assert!(!root
                    .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
                    .exists());
            }
            let calls = source_transition_effect_counts().1;
            assert!(calls > publishers); // Progress/ack/receipt have real metadata publications.
            fixture.resume_at(&fixture.context, &operation).unwrap();
            assert_eq!(source_transition_effect_counts().1, calls);
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(second.join("config.json")).unwrap())
                    .unwrap();
            assert_eq!(value["unknown"]["keep"], serde_json::json!(true));
        }
    }

    #[test]
    fn single_editor_missing_corrupt_original_evidence_never_reconstructs_candidate() {
        for corrupt in [false, true] {
            let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
            let before = read_source_snapshot(&fixture.context, &fixture.source)
                .unwrap()
                .snapshot
                .revision;
            configure_source_durable_cut(Some(("intentInventory", 1)));
            assert!(commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "missing-original",
                &before,
                &candidate,
                &|| Ok(())
            )
            .is_err());
            configure_source_durable_cut(None);
            let operation = fixture.operation();
            let path = fixture
                ._root
                .path()
                .join("C")
                .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
            if corrupt {
                std::fs::write(&path, b"{corrupt original").unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            let identity =
                IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
            let lease =
                SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
            let local = if corrupt {
                SourceRevision::Bytes {
                    sha256: "0".repeat(64),
                }
            } else {
                SourceRevision::Absent {}
            };
            let expected = SourceResumeExpectation {
                source: lease.source_revision().unwrap(),
                journal: local,
            };
            drop(lease);
            drop(identity);
            let publications = source_transition_effect_counts().1;
            assert!(resume_identity_transition_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                &operation,
                &expected,
                &|| Ok(())
            )
            .is_err());
            assert_eq!(source_transition_effect_counts().1, publications);
            assert_eq!(
                read_source_snapshot(&fixture.context, &fixture.source)
                    .unwrap()
                    .snapshot
                    .revision,
                before
            );
            assert!(!target
                .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
                .exists());
            if corrupt {
                assert_eq!(std::fs::read(path).unwrap(), b"{corrupt original");
            } else {
                assert!(!path.exists());
            }
        }
    }

    #[test]
    fn single_editor_thread_oracles_require_worker_owned_observation() {
        let parent = source_transition_effect_counts();
        let worker = std::thread::spawn(|| {
            let fixture = DriverFixture::new();
            let before = source_transition_effect_counts();
            let owning_thread = std::thread::current().id();
            commit_source_candidate_internal(
                &fixture.settings,
                &fixture.context,
                &fixture.source,
                "worker-local",
                &SourceRevision::Absent {},
                &DriverFixture::candidate(),
                &|| {
                    assert_eq!(std::thread::current().id(), owning_thread);
                    Ok(())
                },
            )
            .unwrap();
            let after = source_transition_effect_counts();
            (before, after)
        })
        .join()
        .unwrap();
        assert_eq!(source_transition_effect_counts(), parent);
        assert!(worker.1 .0[0] > worker.0 .0[0]);
        assert_eq!(worker.1 .0[1], worker.0 .0[1], "no source lock acquisition");
        assert!(worker.1 .1 > worker.0 .1);
    }

    #[test]
    fn single_editor_original_slot_parser_rejects_omitted_extra_and_changed_saved_slots() {
        let (fixture, candidate, _, _) = DriverFixture::with_reference_targets();
        let revision = fixture.expectation().source;
        configure_source_durable_cut(Some(("intentInventory", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "slot-completeness",
            &revision,
            &candidate,
            &|| Ok(())
        )
        .is_err());
        configure_source_durable_cut(None);
        let operation = fixture.operation();
        let inventory = read_own_source_journal(&fixture.context)
            .unwrap()
            .1
            .inventories[&operation]
            .clone();
        inventory
            .verify_original_slots(&source_original_inventory_slots)
            .unwrap();
        for corruption in ["omitted", "extra", "changed"] {
            let mut value = serde_json::to_value(&inventory).unwrap();
            let target = value["originalEvidence"]["discovered"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|evidence| !evidence["slots"].as_array().unwrap().is_empty())
                .unwrap();
            let slots = target["slots"].as_array_mut().unwrap();
            match corruption {
                "omitted" => {
                    slots.remove(0);
                }
                "extra" => {
                    slots.push(slots[0].clone());
                }
                "changed" => {
                    slots[0][1]["calculatedId"] = serde_json::json!("changed-id");
                }
                _ => unreachable!(),
            }
            let corrupt: SourceLocalInventory = serde_json::from_value(value).unwrap();
            assert!(corrupt
                .verify_original_slots(&source_original_inventory_slots)
                .is_err());
        }
    }

    #[test]
    fn single_editor_production_save_and_resume_effect_guard_reject_actual_external_drift() {
        for uncertain_target in [false, true] {
            let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
            let path = target.join("config.json");
            let before = std::fs::read(&path).unwrap();
            let mut value: serde_json::Value = serde_json::from_slice(&before).unwrap();
            value["unknown"] = serde_json::json!("actual external fence-time drift");
            let changed = if uncertain_target {
                b"not JSON".to_vec()
            } else {
                serde_json::to_vec(&value).unwrap()
            };
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let worker_barrier = barrier.clone();
            let worker = std::thread::spawn(move || {
                let revision = fixture.expectation().source;
                SOURCE_FENCE_EFFECT_BARRIER.with(|slot| *slot.borrow_mut() = Some(worker_barrier));
                let result = commit_source_candidate_internal(
                    &fixture.settings,
                    &fixture.context,
                    &fixture.source,
                    "real-fence-drift",
                    &revision,
                    &candidate,
                    &|| Ok(()),
                );
                let error = result.unwrap_err().to_string();
                if uncertain_target {
                    assert!(error.contains("sourceIdentityUncertain"), "{error}");
                }
                (fixture, candidate)
            });
            barrier.wait();
            std::fs::write(&path, &changed).unwrap();
            barrier.wait();
            let (fixture, _) = worker.join().unwrap();
            let operation = fixture.operation();
            let original = read_own_source_journal(&fixture.context).unwrap().1;
            let assert_no_fence = |fixture: &DriverFixture| {
                let identity =
                    IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO)
                        .unwrap();
                let lease =
                    SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity)
                        .unwrap();
                assert_eq!(
                    lease.source_revision().unwrap(),
                    original.intents[&operation].expected_revision
                );
                assert!(
                    !read_own_source_journal(&fixture.context)
                        .unwrap()
                        .1
                        .progress[&operation]
                        .packets_prepared
                );
            };
            assert_no_fence(&fixture);
            assert_eq!(std::fs::read(&path).unwrap(), changed);
            std::fs::write(&path, &before).unwrap();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let worker_barrier = barrier.clone();
            let resume_operation = operation.clone();
            let worker = std::thread::spawn(move || {
                SOURCE_FENCE_EFFECT_BARRIER.with(|slot| *slot.borrow_mut() = Some(worker_barrier));
                let error = fixture
                    .resume_at(&fixture.context, &resume_operation)
                    .unwrap_err()
                    .to_string();
                if uncertain_target {
                    assert!(error.contains("sourceIdentityUncertain"), "{error}");
                }
                fixture
            });
            barrier.wait();
            std::fs::write(&path, &changed).unwrap();
            barrier.wait();
            let fixture = worker.join().unwrap();
            assert_no_fence(&fixture);
            assert_eq!(
                read_own_source_journal(&fixture.context).unwrap().1,
                original
            );
            assert_eq!(std::fs::read(&path).unwrap(), changed);
            std::fs::write(&path, before).unwrap();
            fixture.resume_at(&fixture.context, &operation).unwrap();
            assert!(read_own_source_journal(&fixture.context)
                .unwrap()
                .1
                .progress[&operation]
                .receipt
                .is_some());
        }
    }

    #[test]
    fn single_editor_production_save_resume_before_fence_drift_preserves_original() {
        let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
        let revision = fixture.expectation().source;
        configure_source_durable_cut(Some(("beforeFence", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "production-before-fence",
            &revision,
            &candidate,
            &|| Ok(())
        )
        .is_err());
        SOURCE_DURABLE_CUT.with(|state| assert!(state.borrow().0.is_none()));
        configure_source_durable_cut(None);
        let operation = fixture.operation();
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let path = target.join("config.json");
        let before = std::fs::read(&path).unwrap();
        let mut changed: serde_json::Value = serde_json::from_slice(&before).unwrap();
        changed["unknown"] = serde_json::json!("drift after actual Save initial fence check");
        let bytes = serde_json::to_vec(&changed).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let publishers = source_transition_effect_counts().1;
        assert!(fixture.resume_at(&fixture.context, &operation).is_err());
        assert_eq!(source_transition_effect_counts().1, publishers);
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            original
        );
        std::fs::write(&path, &before).unwrap();
        // Actual Resume reaches its own production beforeFence boundary.
        configure_source_durable_cut(Some(("beforeFence", 1)));
        assert!(fixture.resume_at(&fixture.context, &operation).is_err());
        SOURCE_DURABLE_CUT.with(|state| assert!(state.borrow().0.is_none()));
        configure_source_durable_cut(None);
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            original
        );
        fixture.resume_at(&fixture.context, &operation).unwrap();
        assert!(read_own_source_journal(&fixture.context)
            .unwrap()
            .1
            .progress[&operation]
            .receipt
            .is_some());
    }

    #[test]
    fn single_editor_backend_primary_context_drift_preserves_original_no_next_effect() {
        let mut fixture = DriverFixture::new();
        configure_source_durable_cut(Some(("intentInventory", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "primary-context",
            &SourceRevision::Absent {},
            &DriverFixture::candidate(),
            &|| Ok(())
        )
        .is_err());
        configure_source_durable_cut(None);
        let operation = fixture.operation();
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let expected = fixture.expectation();
        let other = fixture._root.path().join("other-project");
        std::fs::create_dir_all(other.join(".ac/coding-agents")).unwrap();
        fixture
            .settings
            .project_paths
            .insert(0, other.to_string_lossy().into_owned());
        let changed = SourceReadContext::from_settings(
            &fixture.settings,
            &fixture._root.path().join("C/settings.json"),
        )
        .unwrap();
        let publishers = source_transition_effect_counts().1;
        assert!(resume_identity_transition_internal(
            &fixture.settings,
            &changed,
            &fixture.source,
            &operation,
            &expected,
            &|| Ok(())
        )
        .is_err());
        assert_eq!(source_transition_effect_counts().1, publishers);
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            original
        );
    }

    fn existing_prepared(cut: &'static str) -> (DriverFixture, String, std::path::PathBuf) {
        let (fixture, candidate, target, _) = DriverFixture::with_reference_targets();
        let before = fixture.expectation().source;
        configure_source_durable_cut(Some((cut, 1)));
        let result = commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "existing-original",
            &before,
            &candidate,
            &|| Ok(()),
        );
        assert!(
            matches!(result, Err(SourceTransitionError::Interrupted { .. })),
            "{result:?}"
        );
        SOURCE_DURABLE_CUT.with(|state| assert!(state.borrow().0.is_none()));
        configure_source_durable_cut(None);
        let operation = fixture.operation();
        assert!(
            read_own_source_journal(&fixture.context).unwrap().1.intents[&operation]
                .before_bytes
                .is_some()
        );
        (fixture, operation, target)
    }

    #[test]
    fn single_editor_existing_after_equivalent_resume() {
        for distinct_object in [false, true] {
            let (fixture, operation, target) = existing_prepared("sourceAfter");
            let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
            let original = read_own_source_journal(&fixture.context).unwrap().1;
            let after = original.intents[&operation].serialized_after.clone();
            let parent = crate::path_identity::retain_directory(path.parent().unwrap()).unwrap();
            let published = parent.verify_regular_file(&path).unwrap();
            if distinct_object {
                std::fs::rename(&path, path.with_extension("original-diagnostic")).unwrap();
                std::fs::write(&path, &after).unwrap();
                assert!(!crate::path_identity::same_object(
                    &published,
                    &parent.verify_regular_file(&path).unwrap()
                ));
            }
            let before = source_transition_publication_counts()[2];
            let uuids = SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get);
            let fresh = SourceReadContext::from_settings(
                &fixture.settings,
                &fixture._root.path().join("C/settings.json"),
            )
            .unwrap();
            assert_eq!(
                get_identity_transition(&fresh, &fixture.source, &operation)
                    .unwrap()
                    .state,
                IdentityTransitionReadState::Prepared
            );
            assert_eq!(
                read_own_source_journal(&fresh).unwrap().1,
                original,
                "status never consolidates"
            );
            fixture.resume_at(&fresh, &operation).unwrap();
            let complete = read_own_source_journal(&fresh).unwrap().1;
            assert_eq!(complete.intents[&operation], original.intents[&operation]);
            assert_eq!(
                complete.inventories[&operation],
                original.inventories[&operation]
            );
            assert_eq!(
                complete.progress[&operation].packets,
                original.progress[&operation].packets
            );
            let progress = &complete.progress[&operation];
            assert!(
                progress.source_published
                    && progress.refs_verified
                    && progress.reloaded
                    && progress.receipt.is_some()
            );
            assert!(
                progress.source_publication.is_none(),
                "existing equivalence has no ownership witness"
            );
            assert_eq!(
                source_transition_publication_counts()[2],
                before,
                "zero second source publication"
            );
            assert_eq!(SOURCE_TRANSITION_UUIDS.with(std::cell::Cell::get), uuids);
            assert_eq!(std::fs::read(path).unwrap(), after);
            assert!(!target
                .join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME)
                .exists());
        }
    }

    #[test]
    fn single_editor_existing_after_divergent_refusal() {
        for drift in ["bytes", "context", "parent", "irregular"] {
            let (fixture, operation, target) = existing_prepared("sourceAfter");
            let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
            let before = read_own_source_journal(&fixture.context).unwrap().1;
            let decisions = std::fs::read(target.join("config.json")).unwrap();
            let mut context = fixture.context.clone();
            match drift {
                "bytes" => std::fs::write(&path, b"{\"external\":\"Z\"}").unwrap(),
                "context" => {
                    let other = fixture._root.path().join("changed-project");
                    std::fs::create_dir_all(other.join(".ac/coding-agents")).unwrap();
                    let mut settings = fixture.settings.clone();
                    settings
                        .project_paths
                        .insert(0, other.to_string_lossy().into_owned());
                    context = SourceReadContext::from_settings(
                        &settings,
                        &fixture._root.path().join("C/settings.json"),
                    )
                    .unwrap();
                }
                "parent" => {
                    let parent = path.parent().unwrap();
                    std::fs::rename(parent, parent.with_extension("original-parent-diagnostic"))
                        .unwrap();
                    std::fs::create_dir(parent).unwrap();
                    std::fs::write(&path, &before.intents[&operation].serialized_after).unwrap();
                }
                _ => {
                    std::fs::remove_file(&path).unwrap();
                    std::fs::create_dir(&path).unwrap();
                }
            }
            let source = std::fs::read(&path)
                .map(|bytes| source_transition_digest_bytes(&bytes))
                .unwrap_or_else(|_| {
                    source_transition_digest_bytes(&before.intents[&operation].serialized_after)
                });
            let expected = SourceResumeExpectation {
                source,
                journal: read_own_source_journal(&context).unwrap().0,
            };
            let reloads = std::cell::Cell::new(0);
            let effects = source_transition_publication_counts();
            assert!(resume_identity_transition_internal(
                &fixture.settings,
                &context,
                &fixture.source,
                &operation,
                &expected,
                &|| {
                    reloads.set(reloads.get() + 1);
                    Ok(())
                }
            )
            .is_err());
            assert_eq!(source_transition_publication_counts(), effects);
            assert_eq!(reloads.get(), 0);
            assert_eq!(read_own_source_journal(&fixture.context).unwrap().1, before);
            assert_eq!(
                std::fs::read(target.join("config.json")).unwrap(),
                decisions
            );
        }
    }

    #[test]
    fn single_editor_source_changed_refusal() {
        for cut in [
            "intentInventory",
            "prepared",
            "sourcePublished",
            "ref",
            "reloaded",
            "receipt",
        ] {
            let (fixture, operation, target) = existing_prepared(cut);
            let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
            let original = read_own_source_journal(&fixture.context).unwrap().1;
            let before_target = std::fs::read(target.join("config.json")).unwrap();
            let bytes = if original.progress[&operation].source_published {
                original.intents[&operation].before_bytes.clone().unwrap()
            } else {
                b"{\"external\":\"Z\"}".to_vec()
            };
            std::fs::write(&path, bytes).unwrap();
            let effects = source_transition_publication_counts();
            assert!(fixture.resume_at(&fixture.context, &operation).is_err());
            assert_eq!(
                source_transition_publication_counts(),
                effects,
                "never republish after source_published"
            );
            assert_eq!(
                read_own_source_journal(&fixture.context).unwrap().1,
                original
            );
            assert_eq!(
                std::fs::read(target.join("config.json")).unwrap(),
                before_target
            );
        }
    }

    #[test]
    fn single_editor_status_compatibility() {
        let fixture = DriverFixture::new();
        let private = fixture
            ._root
            .path()
            .join("C")
            .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
        for bytes in [
            b"{broken".as_slice(),
            b"{}",
            b"{\"schemaVersion\":1,\"intents\":{},\"inventories\":{},\"progress\":{}}",
            b"{\"schemaVersion\":3,\"intents\":{},\"inventories\":{},\"progress\":{}}",
        ] {
            std::fs::write(&private, bytes).unwrap();
            let effects = source_transition_effect_counts();
            assert!(get_identity_transition(&fixture.context, &fixture.source, "unknown").is_err());
            assert_eq!(std::fs::read(&private).unwrap(), bytes);
            assert_eq!(source_transition_effect_counts(), effects);
        }
        std::fs::remove_file(&private).unwrap();
        let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        let legacy = path
            .parent()
            .unwrap()
            .join(crate::config::instance_artifacts::SOURCE_CATALOG_PERSONAL_LEDGER_NAME);
        std::fs::write(&legacy, b"legacy-relevant-diagnostic").unwrap();
        let effects = source_transition_effect_counts();
        assert!(get_identity_transition(&fixture.context, &fixture.source, "unknown").is_err());
        assert_eq!(
            std::fs::read(&legacy).unwrap(),
            b"legacy-relevant-diagnostic"
        );
        assert_eq!(source_transition_effect_counts(), effects);
        std::fs::remove_file(&legacy).unwrap();
        let absent = get_identity_transition(&fixture.context, &fixture.source, "unknown").unwrap();
        assert_eq!(absent.state, IdentityTransitionReadState::NotFound);
        assert_eq!(absent.status.journal_revision, SourceRevision::Absent {});
        let other_source = list_active_sources(&fixture.context)
            .into_iter()
            .find(|s| s.kind == SourceKind::CatalogProject)
            .unwrap();
        let unrelated_path =
            source_transition_test_locator(&fixture.context, &other_source).unwrap();
        // If both kinds share a parent, the source-specific legacy leaf still isolates diagnosis.
        let unrelated = unrelated_path
            .parent()
            .unwrap()
            .join(crate::config::instance_artifacts::SOURCE_CATALOG_PROJECT_LEDGER_NAME);
        std::fs::write(&unrelated, b"unrelated-legacy").unwrap();
        assert_eq!(
            get_identity_transition(&fixture.context, &fixture.source, "unknown")
                .unwrap()
                .state,
            IdentityTransitionReadState::NotFound
        );
        assert_eq!(std::fs::read(unrelated).unwrap(), b"unrelated-legacy");
    }

    #[test]
    fn single_editor_original_local_journal_immutable_and_missing_inventory_refusal() {
        let (fixture, operation, target) = existing_prepared("sourcePublished");
        let identity =
            IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
        let lease =
            SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
        let (revision, original) = read_own_source_journal(&fixture.context).unwrap();
        for field in [
            "intent",
            "inventory",
            "packet",
            "prepared",
            "published",
            "receipt",
        ] {
            let result = update_own_source_journal_with_validation(
                &identity,
                &lease,
                &fixture.context,
                &revision,
                |journal| {
                    match field {
                        "intent" => {
                            journal
                                .intents
                                .get_mut(&operation)
                                .unwrap()
                                .serialized_after
                                .push(b' ');
                        }
                        "inventory" => {
                            journal.inventories.remove(&operation);
                        }
                        "packet" => {
                            journal
                                .progress
                                .get_mut(&operation)
                                .unwrap()
                                .packets
                                .clear();
                        }
                        "prepared" => {
                            journal
                                .progress
                                .get_mut(&operation)
                                .unwrap()
                                .packets_prepared = false
                        }
                        "published" => {
                            journal
                                .progress
                                .get_mut(&operation)
                                .unwrap()
                                .source_published = false
                        }
                        _ => {
                            journal.progress.get_mut(&operation).unwrap().receipt =
                                Some(SourceReceipt {
                                    operation_id: operation.clone(),
                                    source_revision: revision.clone(),
                                })
                        }
                    }
                    Ok(())
                },
                &|| Ok(()),
            );
            assert!(result.is_err(), "mutation accepted: {field}");
            assert_eq!(
                read_own_source_journal(&fixture.context).unwrap().1,
                original
            );
        }
        drop(lease);
        drop(identity);
        let private = fixture
            ._root
            .path()
            .join("C")
            .join(crate::config::instance_artifacts::IDENTITY_TRANSITION_STATE_NAME);
        let mut damaged = serde_json::to_value(&original).unwrap();
        damaged["inventories"]
            .as_object_mut()
            .unwrap()
            .remove(&operation);
        let bytes = serde_json::to_vec(&damaged).unwrap();
        std::fs::write(&private, &bytes).unwrap();
        let expected = SourceResumeExpectation {
            source: source_transition_digest_bytes(&original.intents[&operation].serialized_after),
            journal: source_transition_digest_bytes(&bytes),
        };
        let decision = std::fs::read(target.join("config.json")).unwrap();
        assert!(resume_identity_transition_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            &operation,
            &expected,
            &|| Ok(())
        )
        .is_err());
        assert_eq!(std::fs::read(private).unwrap(), bytes);
        assert_eq!(std::fs::read(target.join("config.json")).unwrap(), decision);
    }
    #[test]
    fn single_editor_original_inventory_topology_and_local_fence() {
        let (fixture, operation, target) = existing_prepared("prepared");
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let packet_path =
            target.join(crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME);
        let packet = std::fs::read(&packet_path).unwrap();
        let late = fixture.project.join(".ac/_agent_late");
        std::fs::create_dir(&late).unwrap();
        std::fs::write(late.join("config.json"), b"{}").unwrap();
        let effects = source_transition_publication_counts();
        assert!(fixture.resume_at(&fixture.context, &operation).is_err());
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            original
        );
        assert_eq!(std::fs::read(&packet_path).unwrap(), packet);
        assert_eq!(source_transition_publication_counts(), effects);
        let result = commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "fresh-request-blocked",
            &fixture.expectation().source,
            &DriverFixture::candidate(),
            &|| Ok(()),
        );
        assert!(
            matches!(result, Err(SourceTransitionError::Interrupted { operation_id, .. }) if operation_id == operation)
        );
        assert_eq!(
            read_own_source_journal(&fixture.context).unwrap().1,
            original
        );
        std::fs::remove_file(late.join("config.json")).unwrap();
        std::fs::remove_dir(&late).unwrap();
        let complete = fixture.resume_at(&fixture.context, &operation).unwrap();
        assert_eq!(complete.state, IdentityTransitionReadState::Complete);
        let journal = read_own_source_journal(&fixture.context).unwrap().1;
        assert_eq!(journal.intents[&operation], original.intents[&operation]);
        assert_eq!(
            journal.inventories[&operation],
            original.inventories[&operation]
        );
        assert_eq!(
            journal.progress[&operation].packets,
            original.progress[&operation].packets
        );
    }

    #[test]
    fn single_editor_source_context_physical_key_and_uncertainty() {
        let fixture = DriverFixture::new();
        let path = source_transition_test_locator(&fixture.context, &fixture.source).unwrap();
        let identity =
            IdentityWriteLease::acquire(&fixture.context, std::time::Duration::ZERO).unwrap();
        let counts = source_transition_effect_counts().0;
        let lease =
            SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
        let key = lease.physical_source_key().to_owned();
        assert_eq!(source_transition_effect_counts().0[1], counts[1]);
        std::fs::write(&path, b"{}").unwrap();
        lease.revalidate(&fixture.context).unwrap();
        let replacement = path.with_extension("replacement");
        std::fs::write(&replacement, b"{\"replacement\":true}").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        let fresh =
            SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).unwrap();
        assert_eq!(fresh.physical_source_key(), key);
        let alias_settings = fixture._root.path().join("C/./settings.json");
        let alias = SourceReadContext::from_settings(&fixture.settings, &alias_settings).unwrap();
        assert_eq!(
            SourceWriteContext::acquire(&alias, &fixture.source, &identity)
                .unwrap()
                .physical_source_key(),
            key
        );
        let foreign = fixture._root.path().join("foreign-C");
        std::fs::create_dir(&foreign).unwrap();
        let foreign_context =
            SourceReadContext::from_settings(&fixture.settings, &foreign.join("settings.json"))
                .unwrap();
        assert!(matches!(
            SourceWriteContext::acquire(&foreign_context, &fixture.source, &identity),
            Err(SourceCoordinationError::Unauthorized)
        ));
        let hardlink = path.with_extension("hardlink");
        std::fs::hard_link(&path, &hardlink).unwrap();
        assert!(SourceWriteContext::acquire(&fixture.context, &fixture.source, &identity).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"replacement\":true}");
    }

    #[test]
    fn single_editor_pending_physical_source_isolation() {
        let fixture = DriverFixture::new();
        let registered = serde_json::json!({"agents":[
            {"id":"K","label":"Own","color":"#123456","command":"own-base"},
            {"id":"J","label":"Other","color":"#123456","command":"other-base"}],
            "codingAgentProfiles":{"schemaVersion":2,"profilesByAgent":{"K":{
                "A":{"command":"--alpha","env":{}},"B":{"command":"--beta","env":{}}},
                "J":{"A":{"command":"--other","env":{}}}},"defaultProfileByAgent":{},
                "identityByAgent":{"K":"legacy-k","J":"legacy-j"}}});
        let registered_path = fixture
            ._root
            .path()
            .join("C")
            .join(crate::config::instance_artifacts::AGENTS_INSTANCE_FILE_NAME);
        std::fs::write(&registered_path, serde_json::to_vec(&registered).unwrap()).unwrap();
        let other = list_active_sources(&fixture.context)
            .into_iter()
            .find(|source| source.kind == SourceKind::RegisteredInstance)
            .unwrap();
        assert_ne!(other, fixture.source);
        let snapshot = read_source_snapshot(&fixture.context, &other).unwrap();
        assert!(snapshot.snapshot.capabilities.editable_save);
        let revision = snapshot.snapshot.revision;
        let mut candidate_b: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&registered_path).unwrap()).unwrap();
        candidate_b["agents"][0]["command"] = serde_json::json!("updated-own-base");
        configure_source_durable_cut(Some(("intentInventory", 1)));
        assert!(commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &fixture.source,
            "personal-prefence",
            &SourceRevision::Absent {},
            &DriverFixture::candidate(),
            &|| Ok(())
        )
        .is_err());
        configure_source_durable_cut(None);
        let original = read_own_source_journal(&fixture.context).unwrap().1;
        let original_a_bytes: Vec<_> = original
            .intents
            .iter()
            .map(|(id, intent)| {
                (
                    id.clone(),
                    serde_json::to_vec(intent).unwrap(),
                    serde_json::to_vec(&original.inventories[id]).unwrap(),
                )
            })
            .collect();
        commit_source_candidate_internal(
            &fixture.settings,
            &fixture.context,
            &other,
            "base-independent",
            &revision,
            &candidate_b,
            &|| Ok(()),
        )
        .unwrap();
        let after = read_own_source_journal(&fixture.context).unwrap().1;
        for (id, intent) in &original.intents {
            assert_eq!(after.intents.get(id), Some(intent));
            assert_eq!(after.inventories.get(id), original.inventories.get(id));
        }
        for (id, intent_bytes, inventory_bytes) in original_a_bytes {
            assert_eq!(
                serde_json::to_vec(&after.intents[&id]).unwrap(),
                intent_bytes
            );
            assert_eq!(
                serde_json::to_vec(&after.inventories[&id]).unwrap(),
                inventory_bytes
            );
        }
        assert_eq!(after.intents.len(), 2);
        assert!(after
            .intents
            .values()
            .any(|intent| intent.source == fixture.source));
        assert!(after.intents.values().any(|intent| intent.source == other));
    }
}

#[cfg(test)]
mod p02_patch_schema_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn p02_patch_presence_types_and_queue_canonical_wire() {
        assert!(serde_json::from_value::<AgentPatch>(json!({}))
            .unwrap()
            .command
            .is_none());
        let patch: AgentPatch =
            serde_json::from_value(json!({"baseCommand":"","command":"\t"})).unwrap();
        assert_eq!(patch.command.as_deref(), Some(""));
        let op = CodingAgentOp::Update {
            id: "own".into(),
            patch,
        };
        let wire = serde_json::to_value(&op).unwrap();
        assert_eq!(wire["patch"]["baseCommand"], "");
        assert!(wire["patch"].get("command").is_none());
        let decoded: CodingAgentOp = serde_json::from_value(wire).unwrap();
        match decoded {
            CodingAgentOp::Update { patch, .. } => assert_eq!(patch.command.as_deref(), Some("")),
            _ => panic!("update"),
        }
        for bad in [json!(null), json!(false), json!(9), json!([]), json!({})] {
            for key in ["command", "baseCommand"] {
                let mut pair = json!({"baseCommand":"new","command":"old"});
                pair[key] = bad.clone();
                assert!(serde_json::from_value::<AgentPatch>(pair).is_err());
            }
        }
    }
}
