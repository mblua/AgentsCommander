mod publication;

pub(super) use self::publication::{
    all_carrier_protections, prepare_protected_publication, probe_publication_directory,
    publication_slot, publication_slots, published_initial_carrier, resolve_publication_slot,
    same_carrier_projection, CarrierProtection, ProtectionPhase, PublicationCheckpoint,
    PublicationDirectoryIdentity, PublicationSlot, PublicationState, RetiredLocalCarrierFact,
};
pub(crate) use self::publication::{
    CurrentCarrierCheckpoint, InitialCarrierIntent, PublishedCarrier,
};
use super::record::PersistedSession;
use crate::session::manager::{CarrierDisposition, CarrierMutationPermit};
use crate::session::session::SessionStatus;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Capture the retained current A key for an authenticated upper replacement.
/// No CWD, recipe, or UUID-only lookup creates replacement authority.
pub(crate) fn retained_carrier_for_replacement(
    _permit: &CarrierMutationPermit,
    directory: &Path,
    target: &str,
    session_id: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> Option<PublishedCarrier> {
    let row = current.rows.iter().find(|row| {
        row.id == session_id
            && matches!(row.status, SessionStatus::Exited(0))
            && crate::session::manager::CarrierSnapshot::is_local_interactive_carrier(row)
            && !current.pending_ids.contains(&row.id)
    })?;
    let version = *current.versions.get(&session_id)?;
    let scope = classify_create_carrier_scope(_permit, target, Path::new(&row.working_directory));
    let slot = publication_slots().lock().ok()?.get(directory)?.clone();
    let state = slot.lock().ok()?;
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed
            && checkpoint.epoch < u64::MAX
            && checkpoint.epoch == state.epoch
            && checkpoint.versions.get(&session_id) == Some(&version)
    })?;
    let mut matches = state.protections.iter().filter(|protection| {
        protection.session_id == session_id
            && protection.target == target
            && protection.token == row.token
            && protection.version == version
            && protection.epoch == checkpoint.epoch
            && protection.phase == ProtectionPhase::Protected
            && protection.lifetime.strong_count() != 0
    });
    let protection = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    let fact = protection.retired_local.as_ref()?;
    if fact.session_id != session_id
        || fact.token != row.token
        || fact.target != target
        || fact.protection_nonce != protection.nonce
        || fact.canonical_replica != protection.canonical_cwd
        || fact.committed_version > fact.settled_version
        || fact.settled_version > version
        || fact.publication_epoch > checkpoint.epoch
        || fact.publication_epoch == u64::MAX
        || (fact.publication_epoch == checkpoint.epoch
            && (fact.checkpoint_nonce != checkpoint.nonce || fact.settled_version != version))
        || !checkpoint
            .rows
            .iter()
            .any(|written| same_carrier_projection(written, &protection.row))
        || !matches!(scope,
            CreateCarrierScope::Protected { session_id: actual, token, version: actual_version,
                epoch, preparing_owner: None, .. }
            if actual == session_id && token == row.token && actual_version == version
                && epoch == checkpoint.epoch)
    {
        return None;
    }
    let lifetime = protection.lifetime.upgrade()?;
    Some(PublishedCarrier {
        target: target.to_owned(),
        session_id,
        token: row.token,
        nonce: protection.nonce,
        version,
        slot: slot.clone(),
        directory: directory.to_path_buf(),
        lifetime,
        retired_local: protection.retired_local.clone(),
    })
}

/// A strict current protection for an explicit manual discard. This proves no
/// route ownership; the transient upper executor must also own the tracker claim.
pub(crate) fn manual_discard_carrier(
    permit: &CarrierMutationPermit,
    canonical_source: &Path,
    target: &str,
    session_id: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> Option<PublishedCarrier> {
    let row = current
        .rows
        .iter()
        .find(|row| row.id == session_id && !current.pending_ids.contains(&row.id))?;
    let version = *current.versions.get(&session_id)?;
    if !matches!(classify_lifecycle_carrier_scope(permit, Some(target), canonical_source),
        CreateCarrierScope::Protected {
            session_id: id, token, version: actual, preparing_owner: None, ..
        }
        if id == session_id && token == row.token && actual == version)
    {
        return None;
    }
    let protections = all_carrier_protections()?;
    let mut own = protections.iter().filter(|p| p.session_id == session_id);
    let protection = own.next()?;
    if own.next().is_some()
        || protection.target != target
        || protection.token != row.token
        || protection.version != version
        || protection.phase != ProtectionPhase::Protected
    {
        return None;
    }
    let candidates = publication_slots()
        .lock()
        .ok()?
        .iter()
        .map(|(directory, slot)| (directory.clone(), slot.clone()))
        .collect::<Vec<_>>();
    let mut found: Option<PublishedCarrier> = None;
    for (directory, slot) in candidates {
        if found
            .as_ref()
            .is_some_and(|prior| std::sync::Arc::ptr_eq(&prior.slot, &slot))
        {
            continue;
        }
        let state = slot.lock().ok()?;
        if !state
            .protections
            .iter()
            .any(|p| p.nonce == protection.nonce && p.session_id == session_id)
        {
            continue;
        }
        if found.is_some() {
            return None;
        }
        state.checkpoint.as_ref().filter(|checkpoint| {
            checkpoint.confirmed
                && checkpoint.epoch < u64::MAX
                && checkpoint.epoch == state.epoch
                && checkpoint.epoch == protection.epoch
                && checkpoint.versions.get(&session_id) == Some(&version)
                && checkpoint
                    .rows
                    .iter()
                    .any(|written| same_carrier_projection(written, &protection.row))
        })?;
        found = Some(PublishedCarrier {
            target: target.to_owned(),
            session_id,
            token: row.token,
            nonce: protection.nonce,
            version,
            slot: slot.clone(),
            directory,
            lifetime: protection.lifetime.upgrade()?,
            retired_local: protection.retired_local.clone(),
        });
    }
    let carrier = found?;
    if carrier.retired_local.is_some() {
        // Historical evidence is accepted only through the full retired accessor.
        return retained_carrier_for_replacement(
            permit,
            &carrier.directory,
            target,
            session_id,
            current,
        );
    }
    Some(carrier)
}

pub(crate) fn retire_manual_discard_carrier(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    canonical_source: &Path,
    current: &crate::session::manager::CarrierSnapshot,
) -> bool {
    let Some(actual) = manual_discard_carrier(
        permit,
        canonical_source,
        &carrier.target,
        carrier.session_id,
        current,
    ) else {
        return false;
    };
    actual.nonce == carrier.nonce
        && actual.token == carrier.token
        && actual.version == carrier.version
        && std::sync::Arc::ptr_eq(&actual.slot, &carrier.slot)
        && retire_carrier_for_discard(permit, carrier.session_id)
}

pub(crate) fn begin_carrier_replacement(
    _permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    owner_nonce: Uuid,
    replacement: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> bool {
    if carrier.retired_local.is_none()
        || replacement == carrier.session_id
        || current.rows.iter().any(|row| row.id == replacement)
        || current.versions.get(&carrier.session_id) != Some(&carrier.version)
        || !current.rows.iter().any(|row| {
            row.id == carrier.session_id
                && row.token == carrier.token
                && matches!(row.status, SessionStatus::Exited(_))
                && !current.pending_ids.contains(&row.id)
        })
    {
        return false;
    }
    let Ok(mut state) = carrier.slot.lock() else {
        return false;
    };
    let Some(checkpoint) = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed
            && checkpoint.epoch < u64::MAX
            && checkpoint.epoch == state.epoch
            && checkpoint.versions.get(&carrier.session_id) == Some(&carrier.version)
    }) else {
        return false;
    };
    let epoch = checkpoint.epoch;
    let Some(protection) = state.protections.iter_mut().find(|protection| {
        protection.target == carrier.target
            && protection.session_id == carrier.session_id
            && protection.token == carrier.token
            && protection.nonce == carrier.nonce
            && protection.version == carrier.version
            && protection.epoch == epoch
            && protection.phase == ProtectionPhase::Protected
            && protection.retired_local == carrier.retired_local
    }) else {
        return false;
    };
    protection.phase = ProtectionPhase::Preparing {
        owner_nonce,
        previous: carrier.session_id,
        replacement,
    };
    true
}

/// Renew only the same operation's full Preparing relation. A historical key
/// never changes UUID; the returned current handle keeps its actual slot/nonce.
pub(crate) fn renew_carrier_replacement(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    replacement: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> Option<PublishedCarrier> {
    let row = current.rows.iter().find(|row| {
        row.id == carrier.session_id
            && row.token == carrier.token
            && !current.pending_ids.contains(&row.id)
    })?;
    if carrier.session_id == previous
        && (!matches!(row.status, SessionStatus::Exited(0))
            || !crate::session::manager::CarrierSnapshot::is_local_interactive_carrier(row))
    {
        return None;
    }
    let version = *current.versions.get(&row.id)?;
    if !matches!(classify_create_carrier_scope(permit, &carrier.target,
        Path::new(&row.working_directory)), CreateCarrierScope::Protected {
            session_id, token, version: actual, preparing_owner: Some(owner), ..
        } if session_id == row.id && token == row.token && actual == version
            && owner == owner_nonce)
    {
        return None;
    }
    let state = carrier.slot.lock().ok()?;
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed
            && checkpoint.epoch < u64::MAX
            && checkpoint.epoch == state.epoch
            && checkpoint.versions.get(&row.id) == Some(&version)
    })?;
    let protection = state.protections.iter().find(|protection| {
        protection.session_id == row.id
            && protection.token == row.token
            && protection.nonce == carrier.nonce
            && protection.target == carrier.target
            && protection.version == version
            && protection.epoch == checkpoint.epoch
            && protection.phase
                == ProtectionPhase::Preparing {
                    owner_nonce,
                    previous,
                    replacement,
                }
            && checkpoint
                .rows
                .iter()
                .any(|written| same_carrier_projection(written, &protection.row))
    })?;
    Some(PublishedCarrier {
        target: carrier.target.clone(),
        session_id: row.id,
        token: row.token,
        nonce: protection.nonce,
        version,
        slot: carrier.slot.clone(),
        directory: carrier.directory.clone(),
        lifetime: protection.lifetime.upgrade()?,
        retired_local: protection.retired_local.clone(),
    })
}

/// Current Protected-B completion proof; this reads the actual checkpoint and
/// never updates a handle to a version that has not been confirmed on disk.
pub(crate) fn current_protected_carrier(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    current: &crate::session::manager::CarrierSnapshot,
) -> Option<PublishedCarrier> {
    let row = current.rows.iter().find(|row| {
        row.id == carrier.session_id
            && row.token == carrier.token
            && !current.pending_ids.contains(&row.id)
    })?;
    let version = *current.versions.get(&row.id)?;
    if version == u64::MAX {
        return None;
    }
    if !matches!(classify_create_carrier_scope(permit, &carrier.target,
        Path::new(&row.working_directory)),
        CreateCarrierScope::Protected {
            session_id, token, version: actual, preparing_owner: None, ..
        }
        if session_id == row.id && token == row.token && actual == version)
    {
        return None;
    }
    let state = carrier.slot.lock().ok()?;
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed
            && checkpoint.epoch < u64::MAX
            && checkpoint.epoch == state.epoch
            && checkpoint.versions.get(&row.id) == Some(&version)
    })?;
    let protection = state.protections.iter().find(|protection| {
        protection.session_id == row.id
            && protection.nonce == carrier.nonce
            && protection.token == row.token
            && protection.version == version
            && protection.epoch == checkpoint.epoch
            && protection.phase == ProtectionPhase::Protected
            && checkpoint
                .rows
                .iter()
                .any(|written| same_carrier_projection(written, &protection.row))
    })?;
    Some(PublishedCarrier {
        target: carrier.target.clone(),
        session_id: row.id,
        token: row.token,
        nonce: protection.nonce,
        version,
        slot: carrier.slot.clone(),
        directory: carrier.directory.clone(),
        lifetime: protection.lifetime.upgrade()?,
        retired_local: None,
    })
}

/// Release the operation's Preparing only after B is strict Confirmed and A
/// has really been removed from the current manager snapshot under this same U.
pub(crate) fn finish_carrier_replacement(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> bool {
    if current.rows.iter().any(|row| row.id == previous) {
        return false;
    }
    let Some(actual) = renew_carrier_replacement(
        permit,
        carrier,
        owner_nonce,
        previous,
        carrier.session_id,
        current,
    ) else {
        return false;
    };
    let Ok(mut state) = carrier.slot.lock() else {
        return false;
    };
    let Some(protection) = state.protections.iter_mut().find(|protection| {
        protection.session_id == actual.session_id
            && protection.nonce == actual.nonce
            && protection.phase
                == ProtectionPhase::Preparing {
                    owner_nonce,
                    previous,
                    replacement: actual.session_id,
                }
    }) else {
        return false;
    };
    protection.phase = ProtectionPhase::Protected;
    true
}

/// Pre-rename rollback releases only this operation's A-side Preparing.
/// A current strict checkpoint must still prove A, and B must be absent.
pub(crate) fn abort_owned_carrier_replacement(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    owner_nonce: Uuid,
    replacement: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> bool {
    if current.rows.iter().any(|row| row.id == replacement) {
        return false;
    }
    let Some(actual) = renew_carrier_replacement(
        permit,
        carrier,
        owner_nonce,
        carrier.session_id,
        replacement,
        current,
    ) else {
        return false;
    };
    let Ok(mut state) = carrier.slot.lock() else {
        return false;
    };
    let Some(protection) = state.protections.iter_mut().find(|protection| {
        protection.session_id == actual.session_id
            && protection.nonce == actual.nonce
            && protection.token == actual.token
            && protection.version == actual.version
            && protection.phase
                == ProtectionPhase::Preparing {
                    owner_nonce,
                    previous: actual.session_id,
                    replacement,
                }
    }) else {
        return false;
    };
    protection.phase = ProtectionPhase::Protected;
    true
}

/// The generic callback runs only under the caller's U/current M region. It
/// supplies tracker primitives in the upper layer; no tracker dependency enters here.
pub(crate) fn register_published_carrier<T>(
    _permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    current: &crate::session::manager::CarrierSnapshot,
    register: impl FnOnce(CurrentCarrierCheckpoint) -> Option<T>,
) -> Option<T> {
    let mut state = carrier.slot.lock().ok()?;
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed && checkpoint.epoch < u64::MAX && checkpoint.epoch == state.epoch
    })?;
    if current.versions.get(&carrier.session_id) != Some(&carrier.version)
        || checkpoint.versions.get(&carrier.session_id) != Some(&carrier.version)
        || !current
            .rows
            .iter()
            .any(|row| row.id == carrier.session_id && row.token == carrier.token)
        || current.pending_ids.contains(&carrier.session_id)
    {
        return None;
    }
    let stamp = CurrentCarrierCheckpoint {
        nonce: checkpoint.nonce,
        epoch: checkpoint.epoch,
        version: carrier.version,
    };
    let protection = state.protections.iter_mut().find(|protection| {
        protection.session_id == carrier.session_id
            && protection.token == carrier.token
            && protection.nonce == carrier.nonce
            && protection.target == carrier.target
            && protection.version == carrier.version
            && protection.epoch == stamp.epoch
            && matches!(protection.phase, ProtectionPhase::Protected)
    })?;
    let result = register(stamp)?;
    protection.retained = Some(carrier.lifetime.clone());
    Some(result)
}

#[derive(Clone, Copy)]
pub(crate) enum CarrierCheckpointTransition<'a> {
    BeginClosing,
    FinishClosing,
    FinishDormant {
        generation: u64,
        committed_version: u64,
        replica: &'a crate::session::manager::CarrierPhysicalIdentity,
    },
}

/// Caller holds the current interior manager read under U. The opaque upper
/// transition runs while this same Confirmed slot remains locked; phase changes
/// occur only after an observed successful tracker transition.
pub(crate) fn with_confirmed_carrier<T>(
    _permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    current: &crate::session::manager::CarrierSnapshot,
    transition: CarrierCheckpointTransition<'_>,
    apply: impl FnOnce(CurrentCarrierCheckpoint) -> Option<T>,
) -> Option<T> {
    let mut state = carrier.slot.lock().ok()?;
    let version = *current.versions.get(&carrier.session_id)?;
    if version < carrier.version
        || current.pending_ids.contains(&carrier.session_id)
        || !current
            .rows
            .iter()
            .any(|row| row.id == carrier.session_id && row.token == carrier.token)
    {
        return None;
    }
    let checkpoint = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed
            && checkpoint.epoch == state.epoch
            && checkpoint.versions.get(&carrier.session_id) == Some(&version)
    })?;
    let stamp = CurrentCarrierCheckpoint {
        nonce: checkpoint.nonce,
        epoch: checkpoint.epoch,
        version,
    };
    let protection = state.protections.iter_mut().find(|protection| {
        protection.session_id == carrier.session_id
            && protection.token == carrier.token
            && protection.target == carrier.target
            && protection.nonce == carrier.nonce
            && protection.epoch == stamp.epoch
            && protection.version == stamp.version
    })?;
    let phase_matches = match transition {
        CarrierCheckpointTransition::BeginClosing => {
            stamp.epoch < u64::MAX
                && protection.phase == ProtectionPhase::Protected
                && version == carrier.version
        }
        CarrierCheckpointTransition::FinishClosing
        | CarrierCheckpointTransition::FinishDormant { .. } => {
            protection.phase == ProtectionPhase::Closing
        }
    };
    if !phase_matches {
        return None;
    }
    // Prepare owned data BEFORE the irreversible tracker transition. Afterwards
    // only infallible moves publish history and Protected under the same slot/U.
    let retirement = match transition {
        CarrierCheckpointTransition::FinishDormant {
            generation,
            committed_version,
            replica,
        } => {
            let row = current
                .rows
                .iter()
                .find(|row| row.id == carrier.session_id)?;
            if committed_version != carrier.version
                || committed_version > version
                || row.token != carrier.token
                || !crate::session::manager::CarrierSnapshot::is_local_interactive_carrier(row)
                || !matches!(row.status, SessionStatus::Exited(0))
                || current.selection.id() == Some(row.id)
                || replica.canonical_path() != protection.canonical_cwd
                || !replica.is_dir()
            {
                return None;
            }
            Some(RetiredLocalCarrierFact {
                session_id: carrier.session_id,
                token: carrier.token,
                target: carrier.target.clone(),
                protection_nonce: carrier.nonce,
                canonical_replica: protection.canonical_cwd.clone(),
                physical_replica: replica.clone(),
                generation,
                committed_version,
                settled_version: stamp.version,
                checkpoint_nonce: stamp.nonce,
                publication_epoch: stamp.epoch,
            })
        }
        _ => None,
    };
    let result = apply(stamp)?;
    protection.retired_local = retirement;
    protection.phase = match transition {
        CarrierCheckpointTransition::BeginClosing => ProtectionPhase::Closing,
        CarrierCheckpointTransition::FinishClosing
        | CarrierCheckpointTransition::FinishDormant { .. } => ProtectionPhase::Protected,
    };
    Some(result)
}

pub(crate) fn retire_carrier_for_discard(_permit: &CarrierMutationPermit, id: Uuid) -> bool {
    let slots = match publication_slots().lock() {
        Ok(slots) => slots.values().cloned().collect::<Vec<_>>(),
        Err(_) => return false,
    };
    for slot in &slots {
        let Ok(state) = slot.lock() else {
            return false;
        };
        if state
            .protections
            .iter()
            .any(|protection| match protection.phase {
                ProtectionPhase::Closing => protection.session_id == id,
                ProtectionPhase::Preparing {
                    previous,
                    replacement,
                    ..
                } => protection.session_id == id || previous == id || replacement == id,
                ProtectionPhase::Protected => false,
            })
        {
            return false;
        }
    }
    for slot in slots {
        let Ok(mut state) = slot.lock() else {
            return false;
        };
        state
            .protections
            .retain(|protection| protection.session_id != id);
    }
    true
}

/// Veto only: an unowned delivery cannot use an unconfirmed replacement B.
/// False supplies no Foreign, checkpoint, route or replacement authority.
pub(crate) fn unconfirmed_replacement_work_waits(target: &str, id: Uuid) -> bool {
    all_carrier_protections().is_none_or(|protections| {
        protections.iter().any(|protection| {
            protection.target == target
                && matches!(protection.phase,
            ProtectionPhase::Preparing { replacement, .. } if replacement == id)
        })
    })
}

/// Pure ALL-scopes startup precondition. None is unreadable ownership, never
/// evidence that no publication protection exists.
pub(crate) fn restore_carrier_scopes_are_empty(_permit: &CarrierMutationPermit) -> Option<bool> {
    Some(all_carrier_protections()?.is_empty())
}

pub(super) fn same_written_projection(
    expected: &[PersistedSession],
    actual: &[PersistedSession],
) -> bool {
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|(expected, actual)| {
            same_carrier_projection(expected, actual) && expected.status == actual.status
        })
}

/// Current ALL-scopes create relation. This is a publication classification,
/// never route authority; the upper owner validates current manager facts.
pub(crate) enum CreateCarrierScope {
    Foreign {
        publication_epoch: u64,
    },
    Protected {
        target: String,
        session_id: Uuid,
        token: Uuid,
        version: u64,
        epoch: u64,
        preparing_owner: Option<Uuid>,
    },
    Waiting,
}

pub(crate) fn classify_create_carrier_scope(
    permit: &CarrierMutationPermit,
    target: &str,
    cwd: &Path,
) -> CreateCarrierScope {
    // Preserve the literal zero-protection legacy path, including absent CWDs.
    let Some(protections) = all_carrier_protections() else {
        return CreateCarrierScope::Waiting;
    };
    if protections.is_empty() {
        return CreateCarrierScope::Foreign {
            publication_epoch: 0,
        };
    }
    let Ok(canonical) = std::fs::canonicalize(cwd) else {
        return CreateCarrierScope::Waiting;
    };
    classify_lifecycle_carrier_scope(permit, Some(target), &canonical)
}

pub(crate) fn classify_lifecycle_carrier_scope(
    _permit: &CarrierMutationPermit,
    target: Option<&str>,
    canonical: &Path,
) -> CreateCarrierScope {
    let Some(protections) = all_carrier_protections() else {
        return CreateCarrierScope::Waiting;
    };
    if protections.is_empty() {
        return CreateCarrierScope::Foreign {
            publication_epoch: 0,
        };
    }
    let publication_epoch = protections
        .iter()
        .map(|protection| protection.epoch)
        .max()
        .unwrap_or(0);
    let mut related = None;
    for protection in protections {
        let intersects = canonical.starts_with(&protection.canonical_cwd)
            || protection.canonical_cwd.starts_with(canonical);
        let same_target = target.is_some_and(|target| protection.target == target);
        if !same_target && !intersects {
            continue;
        }
        if target.is_some_and(|target| protection.target != target)
            || !canonical.starts_with(&protection.canonical_cwd)
            || related.is_some()
            || protection.phase == ProtectionPhase::Closing
        {
            return CreateCarrierScope::Waiting;
        }
        related = Some(protection);
    }
    match related {
        None => CreateCarrierScope::Foreign { publication_epoch },
        Some(protection) => CreateCarrierScope::Protected {
            target: protection.target,
            session_id: protection.session_id,
            token: protection.token,
            version: protection.version,
            epoch: protection.epoch,
            preparing_owner: match protection.phase {
                ProtectionPhase::Preparing { owner_nonce, .. } => Some(owner_nonce),
                _ => None,
            },
        },
    }
}

pub(super) struct PreparedCarrierRelation {
    pub(super) id: Option<Uuid>,
    pub(super) cwd: String,
    pub(super) canonical_cwd: Option<PathBuf>,
}

impl PreparedCarrierRelation {
    pub(super) fn capture(row: &PersistedSession, protections: &[CarrierProtection]) -> Self {
        Self {
            id: row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()),
            cwd: row.working_directory.clone(),
            canonical_cwd: (!protections.is_empty())
                .then(|| std::fs::canonicalize(&row.working_directory).ok())
                .flatten(),
        }
    }

    pub(super) fn capture_manager(
        row: &crate::session::session::Session,
        protections: &[CarrierProtection],
    ) -> Self {
        Self {
            id: Some(row.id),
            cwd: row.working_directory.clone(),
            canonical_cwd: (!protections.is_empty())
                .then(|| std::fs::canonicalize(&row.working_directory).ok())
                .flatten(),
        }
    }

    pub(super) fn classify(&self, protections: &[CarrierProtection]) -> CarrierDisposition {
        if protections.is_empty() {
            return CarrierDisposition::Foreign;
        }
        for protection in protections {
            if self.id == Some(protection.session_id) {
                return CarrierDisposition::Protected;
            }
            let Some(cwd) = self.canonical_cwd.as_ref() else {
                return CarrierDisposition::Unknown;
            };
            if self.cwd == protection.row.working_directory
                || cwd.starts_with(&protection.canonical_cwd)
                || protection.canonical_cwd.starts_with(cwd)
            {
                return CarrierDisposition::Unknown;
            }
        }
        CarrierDisposition::Foreign
    }
}
