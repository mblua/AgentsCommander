use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration as StdDuration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_blocking_spawn_cleans_up_after_worker_finishes() {
    let id = Uuid::new_v4();
    let registered = Arc::new(AtomicBool::new(false));
    let cleanup_count = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let (cleanup_tx, cleanup_rx) = mpsc::channel();

    let registered_for_work = Arc::clone(&registered);
    let work = move || {
        started_tx.send(()).expect("send started");
        release_rx.recv().expect("wait for release");
        registered_for_work.store(true, Ordering::SeqCst);
        done_tx.send(()).expect("send done");
        Ok(())
    };

    let registered_for_cleanup = Arc::clone(&registered);
    let cleanup_count_for_cleanup = Arc::clone(&cleanup_count);
    let expected_id = id;
    let cleanup = move |cleanup_id| {
        assert_eq!(cleanup_id, expected_id);
        registered_for_cleanup.store(false, Ordering::SeqCst);
        cleanup_count_for_cleanup.fetch_add(1, Ordering::SeqCst);
        let _ = cleanup_tx.send(());
    };

    let task = tokio::spawn(spawn_blocking_cancel_safe(
        id,
        work,
        cleanup,
        "test spawn failed",
    ));
    started_rx
        .recv_timeout(StdDuration::from_secs(2))
        .expect("blocking worker should start");

    task.abort();
    cleanup_rx
        .recv_timeout(StdDuration::from_secs(2))
        .expect("dropping waiter should request cleanup");
    release_tx.send(()).expect("release worker");
    done_rx
        .recv_timeout(StdDuration::from_secs(2))
        .expect("blocking worker should finish work");
    cleanup_rx
        .recv_timeout(StdDuration::from_secs(2))
        .expect("cancelled worker should clean up registered state");

    assert!(
        !registered.load(Ordering::SeqCst),
        "registered state should be cleaned up after cancellation"
    );
    assert_eq!(cleanup_count.load(Ordering::SeqCst), 2);
}
