use super::{
    ScannerTaskCleanup, ScannerTaskOwnership, SnapshotScannerShutdownOwner, SCANNER_TASK_ABORTABLE,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn cleanup_counter(counter: Arc<AtomicUsize>) -> ScannerTaskCleanup {
    Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })
}

#[test]
fn admission_seal_rejects_every_later_task() {
    let owner = SnapshotScannerShutdownOwner::default();
    owner.seal();
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_in_task = Arc::clone(&ran);
    let phase = Arc::new(std::sync::atomic::AtomicU8::new(SCANNER_TASK_ABORTABLE));
    let admitted = owner.spawn_tracked(
        None,
        phase,
        cleanup_counter(Arc::new(AtomicUsize::new(0))),
        async move {
            ran_in_task.fetch_add(1, Ordering::SeqCst);
        },
    );
    assert!(!admitted);
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    assert_eq!(owner.task_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn drain_abort_joins_work_before_service_ownership() {
    let owner = SnapshotScannerShutdownOwner::default();
    let cleanup = Arc::new(AtomicUsize::new(0));
    let ownership = ScannerTaskOwnership::new();
    assert!(owner.spawn_tracked(
        None,
        ownership.phase,
        cleanup_counter(Arc::clone(&cleanup)),
        std::future::pending(),
    ));

    let result = owner
        .seal_and_drain_until(tokio::time::Instant::now() + Duration::from_secs(1))
        .await;
    assert!(result.terminal);
    assert_eq!(result.joined, 1);
    assert_eq!(result.aborted, 1);
    assert!(result.retained.is_empty());
    assert_eq!(owner.task_count(), 0);
    assert_eq!(cleanup.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn blocking_service_owner_is_retained_then_joined_and_cleaned() {
    let owner = SnapshotScannerShutdownOwner::default();
    let cleanup = Arc::new(AtomicUsize::new(0));
    let ownership = ScannerTaskOwnership::new();
    assert!(ownership.mark_owned());
    let release = Arc::new(tokio::sync::Notify::new());
    let release_task = Arc::clone(&release);
    assert!(owner.spawn_tracked(
        None,
        ownership.phase,
        cleanup_counter(Arc::clone(&cleanup)),
        async move { release_task.notified().await },
    ));

    let retained = owner
        .seal_and_drain_until(tokio::time::Instant::now() + Duration::from_millis(30))
        .await;
    assert!(!retained.terminal);
    assert_eq!(retained.aborted, 0);
    assert_eq!(retained.retained.len(), 1);
    assert_eq!(owner.task_count(), 1);
    assert_eq!(cleanup.load(Ordering::SeqCst), 0);

    release.notify_waiters();
    let terminal = owner
        .seal_and_drain_until(tokio::time::Instant::now() + Duration::from_secs(1))
        .await;
    assert!(terminal.terminal);
    assert_eq!(terminal.joined, 1);
    assert_eq!(terminal.aborted, 0);
    assert_eq!(owner.task_count(), 0);
    assert_eq!(cleanup.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn finalizer_owner_is_never_aborted_and_is_reported_explicitly() {
    let owner = SnapshotScannerShutdownOwner::default();
    let cleanup = Arc::new(AtomicUsize::new(0));
    let ownership = ScannerTaskOwnership::new();
    assert!(ownership.mark_owned());
    ownership.mark_finalizer();
    let release = Arc::new(tokio::sync::Notify::new());
    let release_task = Arc::clone(&release);
    assert!(owner.spawn_tracked(
        None,
        ownership.phase,
        cleanup_counter(Arc::clone(&cleanup)),
        async move { release_task.notified().await },
    ));

    let retained = owner
        .seal_and_drain_until(tokio::time::Instant::now() + Duration::from_millis(30))
        .await;
    assert!(!retained.terminal);
    assert_eq!(retained.aborted, 0);
    assert_eq!(
        retained.retained,
        vec!["reason=terminal-snapshot-finalizer state=retained"]
    );
    assert_eq!(owner.task_count(), 1);
    assert_eq!(cleanup.load(Ordering::SeqCst), 0);

    release.notify_waiters();
    let terminal = owner
        .seal_and_drain_until(tokio::time::Instant::now() + Duration::from_secs(1))
        .await;
    assert!(terminal.terminal);
    assert_eq!(terminal.joined, 1);
    assert_eq!(owner.task_count(), 0);
    assert_eq!(cleanup.load(Ordering::SeqCst), 1);
}
