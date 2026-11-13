use super::{TerminalSnapshotAuditGuard, TerminalSnapshotSourcePlane};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

#[tokio::test]
async fn t1_ends_on_drop_without_time_passing() {
    let guard =
        TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::ContainerApi);
    let clone = guard.clone();
    let mut wait = std::pin::pin!(guard.wait_for_retained_owners(1));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(wait.as_mut().poll(&mut context), Poll::Pending));
    drop(clone);
    assert!(
        matches!(wait.as_mut().poll(&mut context), Poll::Ready(())),
        "T1: wait must be Ready right after the last extra guard drops"
    );
}

#[tokio::test]
async fn t2_waits_until_owner_drops() {
    let guard =
        TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::ContainerApi);
    let clone = guard.clone();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), guard.wait_for_retained_owners(1))
            .await
            .is_err()
    );
    drop(clone);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), guard.wait_for_retained_owners(1))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn t3_retains_two_owners() {
    let guard =
        TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::ContainerApi);
    let _retained = guard.clone();
    let extra = guard.clone();
    let mut wait = std::pin::pin!(guard.wait_for_retained_owners(2));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(wait.as_mut().poll(&mut context), Poll::Pending));
    drop(extra);
    assert!(matches!(wait.as_mut().poll(&mut context), Poll::Ready(())));
}

#[test]
fn t4_owner_count_matches_arc_count() {
    let guard =
        TerminalSnapshotAuditGuard::pre_admission(TerminalSnapshotSourcePlane::ContainerApi);
    let matches = || {
        assert_eq!(
            *guard.inner.owners.borrow(),
            Arc::strong_count(&guard.inner)
        );
    };
    matches();
    let first = guard.clone();
    matches();
    let second = first.clone();
    matches();
    drop(first);
    matches();
    drop(second);
    matches();
}
