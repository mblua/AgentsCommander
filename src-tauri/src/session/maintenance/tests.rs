use super::*;
use std::sync::{Arc, Barrier};

#[test]
fn legacy_deduplicates_without_replacing_the_owner() {
    let pending = PendingSelfClear::default();
    let sid = Uuid::new_v4();
    let lease = pending.reserve_legacy(sid).unwrap();
    assert_eq!(
        pending.reserve_legacy(sid),
        Err(MaintenanceError::AlreadyLegacy)
    );
    assert_eq!(pending.legacy_lease(sid), Some(lease.clone()));
    assert_eq!(pending.len(), 1);
    assert!(pending.release(&lease));
    assert_eq!(pending.legacy_lease(sid), None);
    assert!(pending.is_empty());
}

#[test]
fn aliases_preserve_ownership_and_old_release_cannot_delete_new_reservations() {
    let pending = PendingSelfClear::default();
    let original = Uuid::new_v4();
    let alias = Uuid::new_v4();
    let occupied = Uuid::new_v4();
    let old = pending.reserve_legacy(original).unwrap();
    let other = pending.reserve_legacy(occupied).unwrap();
    assert_eq!(
        pending.alias(&old, occupied),
        Err(MaintenanceError::AliasOccupied)
    );
    assert_eq!(pending.legacy_lease(occupied), Some(other.clone()));
    assert_eq!(pending.alias(&old, alias), Ok(()));
    assert_eq!(pending.legacy_lease(alias), Some(old.clone()));
    assert_eq!(pending.len(), 3);
    assert!(pending.contains(original));
    assert!(pending.contains(alias));
    assert!(pending.release(&old));
    assert!(!pending.contains(original));
    assert!(!pending.contains(alias));
    assert!(pending.contains(occupied));
    let newer = pending.reserve_legacy(original).unwrap();
    let reused_alias = pending.reserve_legacy(alias).unwrap();
    assert!(!pending.release(&old));
    assert_eq!(
        pending.alias(&old, Uuid::new_v4()),
        Err(MaintenanceError::MissingLease)
    );
    assert_eq!(pending.legacy_lease(original), Some(newer.clone()));
    assert_eq!(pending.legacy_lease(alias), Some(reused_alias.clone()));
    assert!(pending.release(&newer));
    assert!(pending.release(&reused_alias));
    assert!(pending.release(&other));
    assert!(pending.is_empty());
}

#[test]
fn concurrent_legacy_requests_have_one_owner_before_any_release() {
    let pending = Arc::new(PendingSelfClear::default());
    let barrier = Arc::new(Barrier::new(3));
    let sid = Uuid::new_v4();
    let mut threads = Vec::new();
    for _ in 0..2 {
        let pending = Arc::clone(&pending);
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            pending.reserve_legacy(sid)
        }));
    }
    barrier.wait();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| **r == Err(MaintenanceError::AlreadyLegacy))
            .count(),
        1
    );
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending.release(&winner));
    assert!(pending.is_empty());
}

#[test]
fn poison_recovery_keeps_the_owner_and_alias_cleanup() {
    let pending = PendingSelfClear::default();
    let sid = Uuid::new_v4();
    let lease = pending.reserve_legacy(sid).unwrap();
    let poisoned = std::panic::catch_unwind(|| {
        let _guard = pending.entries.lock().unwrap();
        panic!("poison the owner lock");
    });
    assert!(poisoned.is_err());
    assert_eq!(
        pending.reserve_legacy(sid),
        Err(MaintenanceError::AlreadyLegacy)
    );
    let alias = Uuid::new_v4();
    assert_eq!(pending.alias(&lease, alias), Ok(()));
    assert_eq!(pending.legacy_lease(alias), Some(lease.clone()));
    assert!(pending.release(&lease));
    assert!(pending.is_empty());
}

fn identity(sid: Uuid) -> PlainIdentity {
    PlainIdentity {
        session_id: sid,
        request_id: Uuid::new_v4(),
        action: "self-clear".into(),
        requested_by: "project:room/agent".to_string(),
    }
}

fn legacy(pending: &PendingSelfClear, sid: Uuid) -> Lease {
    match pending.reserve_legacy(sid) {
        Ok(lease) => lease,
        other => panic!("expected new legacy reservation, got {other:?}"),
    }
}

fn plain(pending: &PendingSelfClear, id: PlainIdentity) -> Lease {
    match pending.reserve_plain(id) {
        PlainReservation::New(lease) => lease,
        other => panic!("expected new plain reservation, got {other:?}"),
    }
}

#[test]
fn legacy_deduplicates_and_conflicts_with_plain_in_both_directions() {
    let pending = PendingSelfClear::default();
    let sid = Uuid::new_v4();
    let lease = legacy(&pending, sid);
    assert_eq!(
        pending.reserve_legacy(sid),
        Err(MaintenanceError::AlreadyLegacy)
    );
    assert_eq!(
        pending.reserve_plain(identity(sid)),
        PlainReservation::Conflict
    );
    assert_eq!(pending.legacy_lease(sid), Some(lease.clone()));
    assert!(pending.release(&lease));
    let lease = plain(&pending, identity(sid));
    assert_eq!(
        pending.reserve_legacy(sid),
        Err(MaintenanceError::ConflictPlain)
    );
    assert_eq!(pending.legacy_lease(sid), None);
    assert!(pending.release(&lease));
    assert!(pending.is_empty());
}

#[test]
fn plain_deduplicates_by_complete_identity_and_arms_only_its_owner() {
    let pending = PendingSelfClear::default();
    let sid = Uuid::new_v4();
    let id = identity(sid);
    let lease = plain(&pending, id.clone());
    assert_eq!(
        pending.reserve_plain(id.clone()),
        PlainReservation::SamePublishing
    );
    let mut other_request = id.clone();
    other_request.request_id = Uuid::new_v4();
    let mut other_sender = id.clone();
    other_sender.requested_by = "project:room/other".to_string();
    let mut other_action = id.clone();
    other_action.action = "self-handoff-and-clear".into();
    for changed in [other_request, other_sender, other_action] {
        assert_eq!(pending.reserve_plain(changed), PlainReservation::Conflict);
    }
    let foreign_owner = PendingSelfClear::default();
    let foreign_lease = plain(&foreign_owner, id.clone());
    assert_eq!(
        pending.arm_plain(&foreign_lease),
        Err(MaintenanceError::MissingLease)
    );
    assert!(!pending.release(&foreign_lease));
    assert_eq!(
        pending.reserve_plain(id.clone()),
        PlainReservation::SamePublishing
    );
    assert_eq!(pending.arm_plain(&lease), Ok(()));
    assert_eq!(
        pending.reserve_plain(id.clone()),
        PlainReservation::SameArmed
    );
    assert_eq!(
        pending.arm_plain(&lease),
        Err(MaintenanceError::NotPublishing)
    );
    assert_eq!(pending.len(), 1);
    assert!(pending.release(&lease));
    let retry = plain(&pending, id);
    assert_ne!(retry, lease);
    assert!(!pending.release(&lease));
    assert!(pending.release(&retry));
}

#[test]
fn same_request_in_another_session_has_an_independent_reservation() {
    let pending = PendingSelfClear::default();
    let first = identity(Uuid::new_v4());
    let mut second = first.clone();
    second.session_id = Uuid::new_v4();
    let first_lease = plain(&pending, first);
    let second_lease = plain(&pending, second);
    assert_ne!(first_lease, second_lease);
    assert_eq!(pending.len(), 2);
    assert!(pending.release(&first_lease));
    assert_eq!(pending.len(), 1);
    assert!(pending.release(&second_lease));
}

#[test]
fn legacy_arm_is_rejected_without_losing_the_reservation() {
    let pending = PendingSelfClear::default();
    let sid = Uuid::new_v4();
    let lease = legacy(&pending, sid);
    assert_eq!(
        pending.arm_plain(&lease),
        Err(MaintenanceError::NotPublishing)
    );
    assert_eq!(pending.legacy_lease(sid), Some(lease.clone()));
    assert!(pending.release(&lease));
    assert_eq!(
        pending.arm_plain(&lease),
        Err(MaintenanceError::MissingLease)
    );
}

#[test]
fn aliases_share_ownership_and_release_never_deletes_a_later_reservation() {
    let pending = PendingSelfClear::default();
    let original = Uuid::new_v4();
    let alias = Uuid::new_v4();
    let occupied = Uuid::new_v4();
    let old = legacy(&pending, original);
    let other = plain(&pending, identity(occupied));
    assert_eq!(
        pending.alias(&old, occupied),
        Err(MaintenanceError::AliasOccupied)
    );
    assert_eq!(pending.alias(&old, alias), Ok(()));
    assert_eq!(pending.legacy_lease(alias), Some(old.clone()));
    assert_eq!(pending.len(), 3);
    assert!(pending.contains(original));
    assert!(pending.contains(alias));
    assert!(pending.release(&old));
    assert!(!pending.contains(original));
    assert!(!pending.contains(alias));
    assert!(pending.contains(occupied));
    let newer = legacy(&pending, original);
    let reused_alias = legacy(&pending, alias);
    assert!(!pending.release(&old));
    assert_eq!(
        pending.alias(&old, Uuid::new_v4()),
        Err(MaintenanceError::MissingLease)
    );
    assert_eq!(pending.legacy_lease(original), Some(newer.clone()));
    assert_eq!(pending.legacy_lease(alias), Some(reused_alias.clone()));
    assert!(pending.release(&newer));
    assert!(pending.release(&reused_alias));
    assert!(pending.release(&other));
    assert!(pending.is_empty());
}

#[test]
fn legacy_and_plain_competition_has_one_owner_without_releasing_the_winner_early() {
    let pending = Arc::new(PendingSelfClear::default());
    let barrier = Arc::new(Barrier::new(3));
    let sid = Uuid::new_v4();
    let legacy_pending = Arc::clone(&pending);
    let legacy_barrier = Arc::clone(&barrier);
    let legacy_thread = std::thread::spawn(move || {
        legacy_barrier.wait();
        legacy_pending.reserve_legacy(sid)
    });
    let plain_pending = Arc::clone(&pending);
    let plain_barrier = Arc::clone(&barrier);
    let plain_thread = std::thread::spawn(move || {
        plain_barrier.wait();
        plain_pending.reserve_plain(identity(sid))
    });
    barrier.wait();
    let winner = match (legacy_thread.join().unwrap(), plain_thread.join().unwrap()) {
        (Ok(lease), PlainReservation::Conflict)
        | (Err(MaintenanceError::ConflictPlain), PlainReservation::New(lease)) => lease,
        other => panic!("expected exactly one owner and one conflict, got {other:?}"),
    };
    assert_eq!(pending.len(), 1);
    assert!(pending.release(&winner));
    assert!(pending.is_empty());
}

#[test]
fn poison_recovery_preserves_the_existing_owner_and_its_exclusion() {
    let pending = PendingSelfClear::default();
    let sid = Uuid::new_v4();
    let lease = legacy(&pending, sid);
    let poisoned = std::panic::catch_unwind(|| {
        let _guard = pending.entries.lock().unwrap();
        panic!("poison the owner lock");
    });
    assert!(poisoned.is_err());
    assert_eq!(
        pending.reserve_legacy(sid),
        Err(MaintenanceError::AlreadyLegacy)
    );
    assert_eq!(
        pending.reserve_plain(identity(sid)),
        PlainReservation::Conflict
    );
    let alias = Uuid::new_v4();
    assert_eq!(pending.alias(&lease, alias), Ok(()));
    assert_eq!(pending.legacy_lease(alias), Some(lease.clone()));
    assert!(pending.release(&lease));
    assert!(pending.is_empty());
}
