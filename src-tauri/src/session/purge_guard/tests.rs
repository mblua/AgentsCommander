use super::*;
use std::sync::Arc;

#[tokio::test]
async fn purge_guard_blocks_only_targets() {
    let guard = PurgeGuard::default();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let fqn_a = "proj:wg-1-devs/alice".to_string();
    let fqn_b = "proj:wg-1-devs/bob".to_string();

    let mut sids = HashSet::new();
    sids.insert(a);
    let mut fqns = HashSet::new();
    fqns.insert(fqn_a.clone());

    let lease = guard.acquire(sids, fqns).await;

    assert!(guard.blocks_session(a));
    assert!(!guard.blocks_session(b));
    assert!(guard.blocks_agent(&fqn_a));
    assert!(!guard.blocks_agent(&fqn_b));

    drop(lease);
    assert!(!guard.blocks_session(a));
    assert!(!guard.blocks_agent(&fqn_a));
}

#[tokio::test]
async fn purge_guard_serializes_concurrent_purges() {
    let guard = Arc::new(PurgeGuard::default());
    let g1 = Arc::clone(&guard);
    let g2 = Arc::clone(&guard);

    let h1 = tokio::spawn(async move {
        let _lease = g1.acquire(HashSet::new(), HashSet::new()).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    });
    let h2 = tokio::spawn(async move {
        // This must wait for h1's lease to drop.
        let _lease = g2.acquire(HashSet::new(), HashSet::new()).await;
    });

    let (r1, r2) = tokio::join!(h1, h2);
    assert!(r1.is_ok());
    assert!(r2.is_ok());
    // After both complete, no lease is held.
    assert!(!guard.is_active());
}
