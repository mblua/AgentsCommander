use super::carrier::{
    all_carrier_protections, prepare_protected_publication, renew_carrier_replacement,
    same_carrier_projection, CarrierProtection, ProtectionPhase, PublicationCheckpoint,
    PublicationState, PublishedCarrier,
};
use super::orphan::prepare_persist_snapshot;
use super::persist::{same_protection_set, snapshot_aggregate};
use super::store::PreparedSessionsWrite;
use super::write::{
    confirm_publication_readback, current_capture_matches, rename_and_publish,
    settle_rename_attempt, snapshot_store, RenamePublicationAttempt, SettledRenameAttempt,
};
#[cfg(test)]
use super::write::{FixturePublicationAction, FixturePublicationActionKind};
use crate::session::manager::{
    acquire_carrier_mutation_permit, CarrierMutationPermit, SessionManager,
};
use crate::session::session::SessionStatus;
use std::collections::HashSet;
#[cfg(test)]
use std::path::Path;
use uuid::Uuid;

/// Transient prepared native publication. The owner retains destination_key
/// before publishing; this value itself never enters the retained root.
pub(crate) struct PreparedOwnedReplacementPublication {
    #[cfg(test)]
    pub(super) fixture_action: Option<FixturePublicationAction>,
    pub(super) source: PublishedCarrier,
    pub(super) destination_key: PublishedCarrier,
    pub(super) owner_nonce: Uuid,
    pub(super) previous: Uuid,
    pub(super) captured: crate::session::manager::CarrierSnapshot,
    pub(super) protections: Vec<CarrierProtection>,
    pub(super) expected_epoch: u64,
    pub(super) next: Option<PublicationState>,
    pub(super) write: PreparedSessionsWrite,
    pub(super) nonce: Uuid,
    pub(super) epoch: u64,
    pub(super) rows: usize,
}

impl PreparedOwnedReplacementPublication {
    #[cfg(test)]
    pub(crate) fn attach_fixture_action(
        &mut self,
        operation: Uuid,
        directory: &Path,
        kind: FixturePublicationActionKind,
    ) {
        assert_eq!(
            operation, self.owner_nonce,
            "fixture actual publication owner"
        );
        assert_eq!(
            directory,
            self.source.directory.as_path(),
            "fixture prepared directory"
        );
        assert!(
            self.fixture_action.is_none(),
            "fixture action consumed once"
        );
        self.fixture_action = Some(FixturePublicationAction {
            operation,
            nonce: self.nonce,
            directory: directory.to_path_buf(),
            kind,
        });
    }
    pub(crate) fn destination_key(&self) -> PublishedCarrier {
        self.destination_key.clone()
    }
}

pub(crate) enum OwnedReplacementPublicationOutcome {
    Confirmed(PublishedCarrier),
    Pending(PublishedCarrier),
    Recapture,
    Failed(String),
}

/// A current A still needs strict Confirmed retired authority. After a real
/// Confirmed B rename, its exact retained key also admits a fresh-manager
/// checkpoint of newer B, without relabeling the old proof as current.
pub(super) fn replacement_publication_authorized(
    permit: &CarrierMutationPermit,
    source: &PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    replacement: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> bool {
    if source.session_id == previous {
        return renew_carrier_replacement(
            permit,
            source,
            owner_nonce,
            previous,
            replacement,
            current,
        )
        .is_some();
    }
    if source.session_id != replacement
        || current.pending_ids.contains(&replacement)
        || !current
            .rows
            .iter()
            .any(|row| row.id == replacement && row.token == source.token)
        || !current
            .versions
            .get(&replacement)
            .is_some_and(|version| *version >= source.version && *version < u64::MAX)
    {
        return false;
    }
    let Ok(state) = source.slot.lock() else {
        return false;
    };
    let Some(checkpoint) = state.checkpoint.as_ref().filter(|checkpoint| {
        checkpoint.confirmed && checkpoint.epoch == state.epoch && checkpoint.epoch < u64::MAX
    }) else {
        return false;
    };
    state.protections.iter().any(|protection| {
        protection.session_id == replacement
            && protection.token == source.token
            && protection.nonce == source.nonce
            && protection.target == source.target
            && protection.epoch == checkpoint.epoch
            && protection.version >= source.version
            && checkpoint.versions.get(&replacement) == Some(&protection.version)
            && protection.phase
                == ProtectionPhase::Preparing {
                    owner_nonce,
                    previous,
                    replacement,
                }
            && checkpoint
                .rows
                .iter()
                .any(|row| same_carrier_projection(row, &protection.row))
    })
}

struct ReplacementPublicationCapture {
    captured: crate::session::manager::CarrierSnapshot,
    aggregate: crate::session::manager::ManagerAggregateSnapshot,
    expected_epoch: u64,
    next: PublicationState,
    protections: Vec<CarrierProtection>,
}

async fn capture_replacement_publication(
    mgr: &SessionManager,
    source: &PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    replacement: Uuid,
) -> Option<ReplacementPublicationCapture> {
    let permit = acquire_carrier_mutation_permit().await;
    let captured = mgr
        .with_current_carrier_snapshot(&permit, Clone::clone)
        .await;
    if !replacement_publication_authorized(
        &permit,
        source,
        owner_nonce,
        previous,
        replacement,
        &captured,
    ) {
        return None;
    }
    let protections = all_carrier_protections()?;
    let state = source.slot.lock().ok()?.clone();
    let aggregate = mgr.aggregate_snapshot().await;
    Some(ReplacementPublicationCapture {
        captured,
        aggregate,
        expected_epoch: state.epoch,
        next: state,
        protections,
    })
}

fn replacement_publication_row(
    rows: &[super::record::PersistedSession],
    previous: Uuid,
    replacement: Uuid,
) -> Option<super::record::PersistedSession> {
    // A filtered/deduplicated B never authorizes a target write. Protected
    // unrelated rows must also survive the ordinary preparation unchanged.
    let mut matches = rows.iter().filter(|row| {
        row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) == Some(replacement)
    });
    let b_row = matches.next().cloned()?;
    if matches.next().is_some()
        || rows
            .iter()
            .any(|row| row.id.as_deref().and_then(|id| Uuid::parse_str(id).ok()) == Some(previous))
    {
        return None;
    }
    Some(b_row)
}

/// Capture under U, then release it for filtering, physical probes, serialization
/// and temporary-file preparation. Only A is omitted before normal deduplication.
pub(crate) async fn prepare_owned_replacement_publication<V, VF>(
    mgr: &SessionManager,
    source: &PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    replacement: Uuid,
    project_paths: &[String],
    validate_source: V,
) -> Result<Option<PreparedOwnedReplacementPublication>, String>
where
    V: Fn() -> VF,
    VF: std::future::Future<Output = bool>,
{
    let Some(ReplacementPublicationCapture {
        captured,
        aggregate,
        expected_epoch,
        mut next,
        protections,
    }) = capture_replacement_publication(mgr, source, owner_nonce, previous, replacement).await
    else {
        return Ok(None);
    };
    let Some(b) = captured.rows.iter().find(|row| {
        row.id == replacement
            && crate::session::manager::CarrierSnapshot::is_local_interactive_carrier(row)
            && !matches!(row.status, SessionStatus::Exited(_))
            && !captured.pending_ids.contains(&replacement)
    }) else {
        return Ok(None);
    };
    let Some(version) = captured.versions.get(&replacement).copied() else {
        return Ok(None);
    };
    let omitted = HashSet::from([previous]);
    let rows = snapshot_aggregate(&aggregate, &omitted);
    let prep = prepare_persist_snapshot(rows, project_paths).await?;
    let Some(b_row) = replacement_publication_row(&prep.kept, previous, replacement) else {
        return Ok(None);
    };
    let Some(index) = next.protections.iter().position(|protection| {
        protection.session_id == source.session_id
            && protection.nonce == source.nonce
            && protection.phase
                == ProtectionPhase::Preparing {
                    owner_nonce,
                    previous,
                    replacement,
                }
    }) else {
        return Ok(None);
    };
    let old = next.protections.remove(index);
    let Some(epoch) = expected_epoch.checked_add(1) else {
        return Ok(None);
    };
    next.epoch = epoch;
    if !prepare_protected_publication(&mut next, &prep.kept, &captured, None) {
        return Ok(None);
    }
    let lifetime = std::sync::Arc::new(());
    let destination_key = PublishedCarrier {
        target: source.target.clone(),
        session_id: replacement,
        token: b.token,
        nonce: Uuid::new_v4(),
        version,
        slot: source.slot.clone(),
        directory: source.directory.clone(),
        lifetime: lifetime.clone(),
        retired_local: None,
    };
    next.protections.push(CarrierProtection {
        target: source.target.clone(),
        session_id: replacement,
        token: b.token,
        nonce: destination_key.nonce,
        epoch,
        version,
        row: b_row,
        canonical_cwd: old.canonical_cwd.clone(),
        phase: ProtectionPhase::Preparing {
            owner_nonce,
            previous,
            replacement,
        },
        retired_local: None,
        lifetime: std::sync::Arc::downgrade(&lifetime),
        retained: None,
        pending_lifetime: Some(lifetime),
        active: None,
    });
    let nonce = Uuid::new_v4();
    next.checkpoint = Some(PublicationCheckpoint {
        nonce,
        epoch,
        rows: prep.kept.clone(),
        versions: captured.versions.clone(),
        confirmed: false,
    });
    if super::super::settings::agents_layer_unreadable_for_dir(&source.directory) {
        return Ok(None);
    }
    if !validate_source().await {
        return Ok(None);
    }
    let permit = acquire_carrier_mutation_permit().await;
    let valid = mgr
        .with_current_carrier_snapshot(&permit, |current| {
            current_capture_matches(current, &captured)
                && replacement_publication_authorized(
                    &permit,
                    source,
                    owner_nonce,
                    previous,
                    replacement,
                    current,
                )
        })
        .await;
    if !valid
        || !all_carrier_protections()
            .is_some_and(|actual| same_protection_set(&actual, &protections))
    {
        return Ok(None);
    }
    let write = snapshot_store().prepare(&source.directory, &prep.kept)?;
    drop(permit);
    Ok(Some(PreparedOwnedReplacementPublication {
        #[cfg(test)]
        fixture_action: None,
        source: source.clone(),
        destination_key,
        owner_nonce,
        previous,
        captured,
        protections,
        expected_epoch,
        next: Some(next),
        write,
        nonce,
        epoch,
        rows: prep.kept.len(),
    }))
}

/// One actual rename attempt. The caller releases U before any backoff. M and
/// suppression are gone before readback; the exact native leaf remains held.
pub(crate) async fn publish_owned_replacement(
    mgr: &SessionManager,
    permit: &CarrierMutationPermit,
    prepared: &mut PreparedOwnedReplacementPublication,
    validate_current: impl Fn(&crate::session::manager::CarrierSnapshot) -> bool,
) -> OwnedReplacementPublicationOutcome {
    let protections_current = all_carrier_protections()
        .is_some_and(|current| same_protection_set(&current, &prepared.protections));
    if !protections_current {
        return OwnedReplacementPublicationOutcome::Recapture;
    }
    let authority_current = mgr
        .with_current_carrier_snapshot(permit, |current| {
            validate_current(current)
                && current_capture_matches(current, &prepared.captured)
                && replacement_publication_authorized(
                    permit,
                    &prepared.source,
                    prepared.owner_nonce,
                    prepared.previous,
                    prepared.destination_key.session_id,
                    current,
                )
        })
        .await;
    if !authority_current {
        return OwnedReplacementPublicationOutcome::Recapture;
    }
    let attempt = mgr
        .with_current_carrier_snapshot(permit, |current| {
            if !validate_current(current) || !current_capture_matches(current, &prepared.captured) {
                return RenamePublicationAttempt::Stale;
            }
            rename_and_publish(
                &prepared.source.directory,
                &prepared.source.slot,
                prepared.expected_epoch,
                &mut prepared.next,
                &prepared.write,
            )
        })
        .await;
    #[cfg(test)]
    let fixture = if matches!(&attempt, RenamePublicationAttempt::Renamed { .. }) {
        prepared.fixture_action.take()
    } else {
        None
    };
    let result = settle_rename_attempt(
        attempt,
        &prepared.source.slot,
        &prepared.write.path,
        prepared.nonce,
        prepared.epoch,
        prepared.rows,
        #[cfg(test)]
        fixture,
    );
    match result {
        SettledRenameAttempt::Completed(true) => {
            match renew_carrier_replacement(
                permit,
                &prepared.destination_key,
                prepared.owner_nonce,
                prepared.previous,
                prepared.destination_key.session_id,
                &prepared.captured,
            ) {
                Some(carrier) => OwnedReplacementPublicationOutcome::Confirmed(carrier),
                None => {
                    OwnedReplacementPublicationOutcome::Pending(prepared.destination_key.clone())
                }
            }
        }
        SettledRenameAttempt::Completed(false) | SettledRenameAttempt::Recapture => {
            OwnedReplacementPublicationOutcome::Recapture
        }
        SettledRenameAttempt::Failed(error) => {
            OwnedReplacementPublicationOutcome::Failed(prepared.write.terminal_error(&error))
        }
    }
}

/// Post-rename recovery only reads back the same leaf. It never prepares A or
/// performs another rename after an observed successful publication.
pub(crate) fn reconcile_owned_replacement_readback(
    permit: &CarrierMutationPermit,
    carrier: &PublishedCarrier,
    owner_nonce: Uuid,
    previous: Uuid,
    current: &crate::session::manager::CarrierSnapshot,
) -> Option<PublishedCarrier> {
    let (nonce, epoch) = {
        let state = carrier.slot.lock().ok()?;
        let checkpoint = state.checkpoint.as_ref()?;
        let protection = state.protections.iter().find(|protection| {
            protection.session_id == carrier.session_id
                && protection.nonce == carrier.nonce
                && protection.token == carrier.token
                && protection.phase
                    == ProtectionPhase::Preparing {
                        owner_nonce,
                        previous,
                        replacement: carrier.session_id,
                    }
        })?;
        if checkpoint.epoch != state.epoch || protection.epoch != checkpoint.epoch {
            return None;
        }
        (checkpoint.nonce, checkpoint.epoch)
    };
    let leaf = snapshot_store().try_lock_snapshot()?;
    confirm_publication_readback(
        &carrier.slot,
        &super::store::snapshot_path(&carrier.directory),
        nonce,
        epoch,
    );
    drop(leaf);
    renew_carrier_replacement(
        permit,
        carrier,
        owner_nonce,
        previous,
        carrier.session_id,
        current,
    )
}
