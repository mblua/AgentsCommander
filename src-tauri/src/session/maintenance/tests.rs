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
