use super::super::IdentityReferenceCompleteProof;
use super::super::IdentityReferenceError;
use super::super::IdentityReferencePhysicalState;
use super::coordination::coordination_parent;
use super::coordination::coordination_publication_error;
use super::coordination::coordination_read;
#[cfg(test)]
use super::coordination::count_source_transition_publication;
use super::coordination::own_source_journal_path;
use super::coordination::read_own_source_journal;
use super::coordination::IdentityWriteLease;
use super::coordination::SourcePrivateJournal;
use super::coordination::SourceStoredIntent;
use super::coordination::SourceWriteContext;
use super::identity::source_transition_digest;
use super::inventory::source_inventory_digest;
use super::read::SourceReadContext;
use super::read_identity_reference_physical;
use super::source_bytes_revision;
use super::SourceCoordinationError;
use super::SourceRevision;

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct SourcePublicationObservation {
    pub(crate) revision: SourceRevision,
}

/// The caller retains its original authority; exact after is recognized outside
/// the publisher and is never interpreted as a rejected, effect-free attempt.
#[allow(dead_code)]
pub(crate) fn publish_source_intent_with_validation(
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    intent: &SourceStoredIntent,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<SourcePublicationObservation, SourceCoordinationError> {
    if intent.source != lease.source
        || intent.editor_id != lease.identity.instance_id
        || intent.context_revision != lease.context_revision
    {
        return Err(SourceCoordinationError::Unauthorized);
    }
    let after = source_bytes_revision(&intent.serialized_after);
    let guard = || {
        lease
            .identity
            .parent
            .verify_current()
            .map_err(|_| IdentityReferenceError::Conflict)?;
        lease
            .revalidate(context)
            .map_err(|_| IdentityReferenceError::Conflict)?;
        let revision = lease
            .source_revision()
            .map_err(|_| IdentityReferenceError::Conflict)?;
        if revision != intent.expected_revision && revision != after {
            return Err(IdentityReferenceError::Conflict);
        }
        validate()
    };
    guard().map_err(coordination_publication_error)?;
    let observed = lease.source_revision()?;
    let before = match &intent.before_bytes {
        None => IdentityReferencePhysicalState::Absent,
        Some(bytes) => IdentityReferencePhysicalState::from_bytes(bytes.clone()),
    };
    let (mut private_revision, mut journal) = read_own_source_journal(context)?;
    if journal.intents.get(&intent.operation_id) != Some(intent) {
        return Err(SourceCoordinationError::Invalid);
    }
    let inventory = journal
        .inventories
        .get(&intent.operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    if inventory.source != intent.source
        || inventory.editor_id != intent.editor_id
        || inventory.context_revision != intent.context_revision
        || inventory.authorization_revision != intent.authorization_revision
        || inventory.physical_source_key != lease.physical_source_key
        || inventory.mappings != intent.mappings
    {
        return Err(SourceCoordinationError::Invalid);
    }
    let progress = journal
        .progress
        .get(&intent.operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    if !progress.packets_prepared {
        return Err(SourceCoordinationError::Pending);
    }
    let already_published = progress.source_published;
    let initial_proof = progress.source_publication.clone();
    let intent_digest = source_transition_digest(intent)?;
    let ownership_required = intent.before_bytes.is_none();
    if ownership_required && observed == after {
        let proof = initial_proof
            .as_ref()
            .ok_or(SourceCoordinationError::Stale)?;
        if !already_published {
            crate::config::local_config_io::verify_prepared_source_target(
                &lease.parent,
                &lease.source_path,
                &intent.serialized_after,
                proof,
                &guard,
            )
            .map_err(coordination_publication_error)?;
        }
    }
    if observed == after {
        if coordination_read(&lease.source_path)?.1.as_deref()
            != Some(intent.serialized_after.as_slice())
        {
            return Err(SourceCoordinationError::Stale);
        }
        guard().map_err(coordination_publication_error)?;
        return Ok(SourcePublicationObservation { revision: after });
    }
    if already_published {
        return Err(SourceCoordinationError::Stale);
    }
    if ownership_required {
        let mut capability = match initial_proof.as_ref() {
            Some(proof) => crate::config::local_config_io::reopen_source_publication(
                &lease.parent,
                &lease.source_path,
                &intent.serialized_after,
                proof,
                &guard,
            ),
            None => crate::config::local_config_io::prepare_source_publication(
                &lease.parent,
                &lease.source_path,
                &intent.serialized_after,
                &intent_digest,
                &guard,
            ),
        }
        .map_err(coordination_publication_error)?;
        if initial_proof.is_none() {
            let proof = capability.proof().clone();
            let mut planned = journal.clone();
            planned
                .progress
                .get_mut(&intent.operation_id)
                .ok_or(SourceCoordinationError::Invalid)?
                .source_publication = Some(proof.clone());
            let checkpoint_guard = || capability.check();
            let checkpoint = update_own_source_journal_for_publication(
                lease.identity,
                lease,
                context,
                &private_revision,
                |journal| {
                    journal
                        .progress
                        .get_mut(&intent.operation_id)
                        .ok_or(SourceCoordinationError::Invalid)?
                        .source_publication = Some(proof);
                    Ok(())
                },
                &checkpoint_guard,
                Some((&intent.operation_id, &capability)),
            );
            match checkpoint {
                Ok(revision) => {
                    capability.check().map_err(coordination_publication_error)?;
                    let (confirmed_revision, confirmed) = read_own_source_journal(context)?;
                    if confirmed_revision != revision || confirmed != planned {
                        return Err(SourceCoordinationError::Stale);
                    }
                    private_revision = confirmed_revision;
                    journal = confirmed;
                }
                Err(error) => {
                    // Pure Err-before/Err-landed/other classification. Every
                    // outcome retains the stage and returns the original error;
                    // no native, unlink or second CAS follows an Err.
                    if capability.check().is_ok() {
                        if let Ok((revision, current)) = read_own_source_journal(context) {
                            let _before_exact = revision == private_revision && current == journal;
                            let _after_exact = current == planned;
                        }
                    }
                    return Err(error);
                }
            }
        }
        let checkpoint_guard = || {
            guard()?;
            let (revision, current) =
                read_own_source_journal(context).map_err(|_| IdentityReferenceError::Conflict)?;
            if revision != private_revision || current != journal {
                return Err(IdentityReferenceError::Conflict);
            }
            Ok(())
        };
        #[cfg(test)]
        count_source_transition_publication(2);
        capability
            .commit(&checkpoint_guard)
            .map_err(coordination_publication_error)?;
        return Ok(SourcePublicationObservation { revision: after });
    }
    #[cfg(test)]
    count_source_transition_publication(2);
    let publication =
        crate::config::local_config_io::publish_source_coordination_bytes_with_validation(
            &lease.source_path,
            &before,
            &intent.serialized_after,
            &lease.parent,
            &guard,
        );
    publication.result.map_err(coordination_publication_error)?;
    // Validate authority again, then classify durable bytes even if the final
    // publication check failed. No callback advances local progress or events.
    guard().map_err(coordination_publication_error)?;
    match lease.source_revision()? {
        revision if revision == after => Ok(SourcePublicationObservation { revision }),
        revision if revision == intent.expected_revision => Err(SourceCoordinationError::Stale),
        _ => Err(SourceCoordinationError::Stale),
    }
}

/// Pure record changes are redecoded before guarded CAS publication; callbacks
/// borrow original proofs and never advance coordinator progress themselves.
#[allow(dead_code)]
pub(crate) fn update_own_source_journal_with_validation<F>(
    identity: &IdentityWriteLease,
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    expected: &SourceRevision,
    change: F,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<SourceRevision, SourceCoordinationError>
where
    F: FnOnce(&mut SourcePrivateJournal) -> Result<(), SourceCoordinationError>,
{
    update_own_source_journal_for_publication(
        identity, lease, context, expected, change, validate, None,
    )
}

pub(crate) fn update_own_source_journal_for_publication<F>(
    identity: &IdentityWriteLease,
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    expected: &SourceRevision,
    change: F,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
    prepared: Option<(
        &str,
        &crate::config::local_config_io::PreparedSourceCapability<'_>,
    )>,
) -> Result<SourceRevision, SourceCoordinationError>
where
    F: FnOnce(&mut SourcePrivateJournal) -> Result<(), SourceCoordinationError>,
{
    if !std::ptr::eq(identity, lease.identity) {
        return Err(SourceCoordinationError::Unauthorized);
    }
    let parent = coordination_parent(&context.settings_path)?;
    if !crate::path_identity::same_object(parent.identity(), identity.parent.identity()) {
        return Err(SourceCoordinationError::Unauthorized);
    }
    let guard = || {
        identity
            .parent
            .verify_current()
            .map_err(|_| IdentityReferenceError::Conflict)?;
        lease
            .revalidate(context)
            .map_err(|_| IdentityReferenceError::Conflict)?;
        validate()
    };
    guard().map_err(coordination_publication_error)?;
    let path = own_source_journal_path(context)?;
    let before = read_identity_reference_physical(&path).map_err(coordination_publication_error)?;
    let (revision, mut journal) = read_own_source_journal(context)?;
    if &revision != expected
        || before
            != read_identity_reference_physical(&path).map_err(coordination_publication_error)?
    {
        return Err(SourceCoordinationError::Stale);
    }
    let original = journal.clone();
    change(&mut journal)?;
    journal.validate()?;
    if original
        .intents
        .iter()
        .any(|(id, intent)| journal.intents.get(id) != Some(intent))
        || original
            .inventories
            .iter()
            .any(|(id, inventory)| journal.inventories.get(id) != Some(inventory))
        || original
            .progress
            .keys()
            .any(|id| !journal.progress.contains_key(id))
    {
        return Err(SourceCoordinationError::Invalid);
    }
    for (id, previous) in &original.progress {
        let next = journal
            .progress
            .get(id)
            .ok_or(SourceCoordinationError::Invalid)?;
        if !next.packets.starts_with(&previous.packets)
            || previous
                .source_publication
                .as_ref()
                .is_some_and(|proof| next.source_publication.as_ref() != Some(proof))
            || previous.packets_prepared && !next.packets_prepared
            || previous.source_published && !next.source_published
            || previous.refs_verified && !next.refs_verified
            || previous.reloaded && !next.reloaded
            || previous
                .receipt
                .as_ref()
                .is_some_and(|receipt| next.receipt.as_ref() != Some(receipt))
        {
            return Err(SourceCoordinationError::Invalid);
        }
    }
    for (id, next) in &journal.progress {
        if original
            .progress
            .get(id)
            .and_then(|p| p.source_publication.as_ref())
            .is_none()
            && next.source_publication.is_some()
        {
            let (prepared_id, capability) = prepared.ok_or(SourceCoordinationError::Invalid)?;
            let intent = journal
                .intents
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            if prepared_id != id
                || intent.source != lease.source
                || next.source_publication.as_ref() != Some(capability.proof())
                || !next.packets_prepared
                || intent.editor_id != identity.instance_id
                || intent.expected_revision != (SourceRevision::Absent {})
                || lease.source_revision()? != (SourceRevision::Absent {})
            {
                return Err(SourceCoordinationError::Invalid);
            }
            capability.check().map_err(coordination_publication_error)?;
        }
    }
    for (id, next) in &journal.progress {
        let previous = original.progress.get(id);
        let publishing = next.source_published && previous.is_none_or(|p| !p.source_published);
        let completing = next.receipt.is_some() && previous.is_none_or(|p| p.receipt.is_none());
        if publishing || completing {
            let intent = journal
                .intents
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            let inventory = journal
                .inventories
                .get(id)
                .ok_or(SourceCoordinationError::Invalid)?;
            if intent.source != lease.source
                || intent.editor_id != identity.instance_id
                || inventory.physical_source_key != lease.physical_source_key
                || coordination_read(&lease.source_path)?.1.as_deref()
                    != Some(intent.serialized_after.as_slice())
            {
                return Err(SourceCoordinationError::Stale);
            }
            if publishing && intent.before_bytes.is_none() {
                let proof = next
                    .source_publication
                    .as_ref()
                    .ok_or(SourceCoordinationError::Invalid)?;
                crate::config::local_config_io::verify_prepared_source_target(
                    &lease.parent,
                    &lease.source_path,
                    &intent.serialized_after,
                    proof,
                    &guard,
                )
                .map_err(coordination_publication_error)?;
            }
        }
    }
    for (id, intent) in &journal.intents {
        if original.intents.contains_key(id) {
            continue;
        }
        if intent.editor_id != identity.instance_id
            || intent.source != lease.source
            || intent.context_revision != lease.context_revision
            || lease.source_revision()? != intent.expected_revision
        {
            return Err(SourceCoordinationError::Stale);
        }
    }
    for (id, inventory) in &journal.inventories {
        if original.inventories.get(id) == Some(inventory) {
            continue;
        }
        if inventory.editor_id != identity.instance_id
            || inventory.source != lease.source
            || inventory.physical_source_key != lease.physical_source_key
            || inventory.context_revision != lease.context_revision
        {
            return Err(SourceCoordinationError::Unauthorized);
        }
        let intent = journal
            .intents
            .get(id)
            .ok_or(SourceCoordinationError::Invalid)?;
        let evidence = inventory
            .original_evidence
            .as_ref()
            .ok_or(SourceCoordinationError::Invalid)?;
        if intent.editor_id != identity.instance_id
            || intent.source != inventory.source
            || intent.mappings != inventory.mappings
            || intent.authorization_revision != inventory.authorization_revision
            || evidence.expected_source != intent.expected_revision
            || evidence.after_source != source_bytes_revision(&intent.serialized_after)
            || evidence.inventory_digest
                != source_inventory_digest(
                    &inventory.editor_id,
                    &inventory.source,
                    &inventory.physical_source_key,
                    &inventory.context_revision,
                    &evidence.topology,
                )?
            || lease.source_revision()? != evidence.expected_source
        {
            return Err(SourceCoordinationError::Unauthorized);
        }
    }
    if journal == original {
        guard().map_err(coordination_publication_error)?;
        return Ok(revision);
    }
    let mut bytes =
        serde_json::to_vec_pretty(&journal).map_err(|_| SourceCoordinationError::Invalid)?;
    bytes.push(b'\n');
    let decoded: SourcePrivateJournal =
        serde_json::from_slice(&bytes).map_err(|_| SourceCoordinationError::Invalid)?;
    decoded.validate()?;
    if decoded != journal {
        return Err(SourceCoordinationError::Invalid);
    }
    #[cfg(test)]
    count_source_transition_publication(1);
    crate::config::local_config_io::publish_source_coordination_bytes_with_validation(
        &path,
        &before,
        &bytes,
        &identity.parent,
        &guard,
    )
    .result
    .map_err(coordination_publication_error)?;
    Ok(source_bytes_revision(&bytes))
}

/// Lookup is restricted to the own C path derived from backend context. Status
/// can expose the original ID and CAS before a shared fence exists.
#[allow(dead_code)]
pub(crate) fn read_own_source_intent(
    context: &SourceReadContext,
    operation_id: &str,
) -> Result<(SourceRevision, Option<SourceStoredIntent>), SourceCoordinationError> {
    let (revision, journal) = read_own_source_journal(context)?;
    Ok((revision, journal.intents.get(operation_id).cloned()))
}

#[allow(dead_code)]
pub(crate) fn source_complete_proof(
    lease: &SourceWriteContext,
    context: &SourceReadContext,
    operation_id: &str,
    target_id: &str,
    validate: &dyn Fn() -> Result<(), IdentityReferenceError>,
) -> Result<IdentityReferenceCompleteProof, SourceCoordinationError> {
    validate().map_err(coordination_publication_error)?;
    lease.revalidate(context)?;
    let (_, journal) = read_own_source_journal(context)?;
    let intent = journal
        .intents
        .get(operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    let inventory = journal
        .inventories
        .get(operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    let progress = journal
        .progress
        .get(operation_id)
        .ok_or(SourceCoordinationError::Invalid)?;
    if intent.editor_id != lease.identity.instance_id
        || intent.source != lease.source
        || inventory.physical_source_key != lease.physical_source_key
        || coordination_read(&lease.source_path)?.1.as_deref()
            != Some(intent.serialized_after.as_slice())
    {
        return Err(SourceCoordinationError::Stale);
    }
    let projection = serde_json::json!({
        "schemaVersion":2, "operationId":operation_id, "phase":progress.phase(),
        "physicalSourceKey":inventory.physical_source_key,
        "afterSourceRevision":source_bytes_revision(&intent.serialized_after),
        "packetsPrepared":progress.packets_prepared, "sourcePublished":progress.source_published,
        "refsVerified":progress.refs_verified, "reloaded":progress.reloaded,
        "packets":progress.packets, "receipt":progress.receipt
    });
    let source = serde_json::to_value(lease.source_revision()?)
        .map_err(|_| SourceCoordinationError::Invalid)?;
    IdentityReferenceCompleteProof::from_single_editor_complete(
        &projection,
        operation_id,
        target_id,
        &source,
    )
    .map_err(coordination_publication_error)
}
