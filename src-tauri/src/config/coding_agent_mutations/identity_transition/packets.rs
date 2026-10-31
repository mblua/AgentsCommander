#[cfg(test)]
use super::IdentityTransitionTestControls;
use super::{source_before_fence_boundary, source_durable_boundary, SourceTransitionError};
use crate::config::coding_agent_profiles::prepare_identity_reference_patch;
use crate::config::settings::verify_completed_source_target;
use crate::config::settings::{
    recover_source_target_packet, source_transition_digest, IdentityReferenceError,
    IdentityReferenceRequest, SourceCoordinationError, SourceInventoryProof, SourceLocalProgress,
    SourcePacketRecord, SourceRevision, SourceWriteContext,
};

/// Packet preparation and original-packet verification under the source authority.
pub(super) struct TransitionPackets<'a> {
    pub(super) lease: &'a SourceWriteContext<'a>,
    pub(super) operation_id: &'a str,
    pub(super) authority: &'a SourceInventoryProof,
    pub(super) rediscover: &'a dyn Fn() -> Result<(), SourceTransitionError>,
    pub(super) validate: &'a dyn Fn() -> Result<(), IdentityReferenceError>,
    #[cfg(test)]
    pub(super) controls: &'a IdentityTransitionTestControls,
}

impl TransitionPackets<'_> {
    pub(super) fn prepare(
        &self,
        mut progress: SourceLocalProgress,
        persist: &dyn Fn(SourceLocalProgress) -> Result<SourceLocalProgress, SourceTransitionError>,
    ) -> Result<SourceLocalProgress, SourceTransitionError> {
        let Self {
            lease,
            operation_id,
            authority,
            rediscover,
            validate,
            #[cfg(test)]
            controls,
        } = *self;
        let inventory = authority.inventory();
        if !progress.packets_prepared {
            source_before_fence_boundary(
                #[cfg(test)]
                controls,
            )?;
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
                    validate,
                )? {
                    Some(packet) => packet,
                    None if indexed.is_some() => {
                        return Err(IdentityReferenceError::Conflict.into())
                    }
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
                        validate,
                    )?,
                };
                source_durable_boundary(
                    "packet",
                    #[cfg(test)]
                    controls,
                )?;
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
            source_durable_boundary(
                "allPackets",
                #[cfg(test)]
                controls,
            )?;
            progress.packets_prepared = true;
            progress = persist(progress)?;
        }
        Ok(progress)
    }

    pub(super) fn verify_original(
        &self,
        progress: &SourceLocalProgress,
    ) -> Result<(), SourceTransitionError> {
        let Self {
            lease,
            operation_id,
            authority,
            validate,
            ..
        } = *self;
        let inventory = authority.inventory();
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
                    validate,
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
        Ok(())
    }

    pub(super) fn verify_completed(
        &self,
        progress: &SourceLocalProgress,
    ) -> Result<(), SourceTransitionError> {
        let inventory = self.authority.inventory();
        let validate = self.validate;
        // Final target verification precedes the sole complete receipt.
        for row in &progress.packets {
            let target = inventory
                .targets
                .iter()
                .find(|t| t.decisions == row.target)
                .ok_or(SourceCoordinationError::Invalid)?;
            verify_completed_source_target(target, row, validate)?;
        }
        Ok(())
    }
}
