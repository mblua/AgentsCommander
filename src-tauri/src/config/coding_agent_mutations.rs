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
    static SOURCE_DURABLE_CUT: std::cell::RefCell<SourceDurableCutState> =
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
fn configure_source_durable_cut(cut: Option<(&'static str, usize)>) {
    SOURCE_DURABLE_CUT.with(|state| *state.borrow_mut() = (cut, Vec::new()));
}

#[cfg(test)]
thread_local! {
    // Test-only pause outside validation: an external writer changes a real
    // protected target between production's initial check and fence effect.
    static SOURCE_FENCE_EFFECT_BARRIER:
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
            "agent id '{id}' must match ^[a-z0-9][a-z0-9_-]{{0,63}}$ \
             (lowercase letters, digits, hyphen, underscore)"
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
mod tests;

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
