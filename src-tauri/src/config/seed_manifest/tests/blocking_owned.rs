use super::*;

#[tokio::test]
async fn blocking_owned_drop_before_first_poll_schedules_nothing() {
    struct DropProbe(Arc<AtomicBool>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, AtomicOrdering::Release);
        }
    }

    let ran = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let drop_probe = DropProbe(Arc::clone(&dropped));
    let future = run_blocking_owned({
        let ran = Arc::clone(&ran);
        move || {
            let _drop_probe = drop_probe;
            ran.store(true, AtomicOrdering::Release);
        }
    });
    drop(future);
    tokio::task::yield_now().await;
    assert!(!ran.load(AtomicOrdering::Acquire));
    assert!(dropped.load(AtomicOrdering::Acquire));
}

#[test]
fn blocking_owned_drop_while_queued_suppresses_user_work() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let (blocker_started_tx, blocker_started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            blocker_started_tx.send(()).expect("signal blocker");
            release_rx.recv().expect("release blocker");
        });
        blocker_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking pool occupied");

        let ran = Arc::new(AtomicBool::new(false));
        let waiter = tokio::spawn(run_blocking_owned({
            let ran = Arc::clone(&ran);
            move || ran.store(true, AtomicOrdering::Release)
        }));
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        waiter.abort();
        let _ = waiter.await;
        release_tx.send(()).expect("release blocking pool");
        blocker.await.expect("join blocker");
        tokio::task::yield_now().await;
        assert!(!ran.load(AtomicOrdering::Acquire));
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_owned_started_work_finishes_after_waiter_drop() {
    let started = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicBool::new(false));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let waiter = tokio::spawn(run_blocking_owned({
        let started = Arc::clone(&started);
        let completed = Arc::clone(&completed);
        move || {
            started.store(true, AtomicOrdering::Release);
            release_rx.recv().expect("release started work");
            completed.store(true, AtomicOrdering::Release);
        }
    }));
    while !started.load(AtomicOrdering::Acquire) {
        tokio::task::yield_now().await;
    }
    waiter.abort();
    let _ = waiter.await;
    release_tx.send(()).expect("release worker");
    tokio::time::timeout(Duration::from_secs(1), async {
        while !completed.load(AtomicOrdering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("started work must remain self-owned");
}

#[tokio::test]
async fn blocking_owned_surfaces_worker_panic_as_join_error() {
    let error = run_blocking_owned(|| -> () { panic!("injected blocking panic") })
        .await
        .expect_err("panic must surface as a join error");
    let BlockingTaskError::Join(join_error) = &error else {
        panic!("expected typed join error, got {error:?}");
    };
    assert!(join_error.is_panic());
    let source = std::error::Error::source(&error).expect("join error remains the source");
    assert!(source.downcast_ref::<tokio::task::JoinError>().is_some());
}
