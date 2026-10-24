use super::*;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const KIND: SessionBackendKind = SessionBackendKind::LocalProcess;
const BOUND: Duration = Duration::from_secs(5);

#[derive(Default)]
struct CountedWriter {
    writes: AtomicUsize,
}

impl RawPtyWriter for CountedWriter {
    fn write(&self, _: &BackendWriteAuthority, _: Uuid, _: &[u8]) -> Result<(), AppError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn fixture() -> (RouteRegistry, ResponseCloseState, Uuid, Arc<CountedWriter>) {
    let registry = RouteRegistry::default();
    let tracker = ResponseCloseState::default();
    let id = Uuid::new_v4();
    let writer = Arc::new(CountedWriter::default());
    registry
        .publish(id, KIND, None, None, writer.clone(), &tracker)
        .unwrap();
    (registry, tracker, id, writer)
}

fn poll_pending<F: Future>(future: std::pin::Pin<&mut F>) {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(future.poll(&mut context).is_pending());
}

fn writers(state: &ResponseCloseState, id: Uuid) -> usize {
    state.inner.lock().unwrap().routes[&id].writers
}

fn error<T>(result: Result<T, AppError>, expected: &str) {
    match result {
        Ok(_) => panic!("expected {expected}"),
        Err(actual) => assert!(actual.to_string().contains(expected), "{actual}"),
    }
}

#[tokio::test]
async fn generation_replaced_during_input_wait() {
    let (registry, tracker, id, _) = fixture();
    let first = registry
        .prepare(id, &tracker)
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let mut queued = Box::pin(registry.prepare(id, &tracker).unwrap().acquire());
    poll_pending(queued.as_mut());
    assert_eq!(writers(&tracker, id), 2);
    registry.remove_route_if_kind(id, KIND, &tracker);
    registry
        .publish(
            id,
            KIND,
            None,
            None,
            Arc::new(CountedWriter::default()),
            &tracker,
        )
        .unwrap();
    drop(first);
    error(
        tokio::time::timeout(BOUND, queued).await.unwrap(),
        "stale_pty_route",
    );
    assert_eq!(writers(&tracker, id), 0);
}

#[tokio::test]
async fn close_started_during_input_wait() {
    let (registry, tracker, id, writer) = fixture();
    let first = registry
        .prepare(id, &tracker)
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let mut queued = Box::pin(registry.prepare(id, &tracker).unwrap().acquire());
    poll_pending(queued.as_mut());
    assert_eq!(writers(&tracker, id), 2);
    tracker
        .inner
        .lock()
        .unwrap()
        .routes
        .get_mut(&id)
        .unwrap()
        .closing = Some(Uuid::new_v4());
    drop(first);
    let permit = tokio::time::timeout(BOUND, queued).await.unwrap().unwrap();
    error(lock_route_for_write(&permit), "response_close_deferred");
    assert_eq!(writer.writes.load(Ordering::SeqCst), 0);
    drop(permit);
    assert_eq!(writers(&tracker, id), 0);
}

#[tokio::test]
async fn writer_bound_to_route_generation() {
    let (registry, tracker, id, a) = fixture();
    let old = registry
        .prepare(id, &tracker)
        .unwrap()
        .acquire()
        .await
        .unwrap();
    registry.remove_route_if_kind(id, KIND, &tracker);
    let b = Arc::new(CountedWriter::default());
    registry
        .publish(id, KIND, None, None, b.clone(), &tracker)
        .unwrap();
    assert!(lock_route_for_write(&old).is_err());
    let new = registry
        .prepare(id, &tracker)
        .unwrap()
        .acquire()
        .await
        .unwrap();
    write_with_permit(&new, b"only B").unwrap();
    assert_eq!(a.writes.load(Ordering::SeqCst), 0);
    assert_eq!(b.writes.load(Ordering::SeqCst), 1);
}

struct BlockingWriter {
    entered: std::sync::mpsc::Sender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
    writes: AtomicUsize,
}
impl RawPtyWriter for BlockingWriter {
    fn write(&self, _: &BackendWriteAuthority, _: Uuid, _: &[u8]) -> Result<(), AppError> {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv_timeout(BOUND).unwrap();
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn removal_waits_for_active_guard() {
    let (registry, tracker, id, _) = fixture();
    registry.remove_route_if_kind(id, KIND, &tracker);
    let (entered, started) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let writer = Arc::new(BlockingWriter {
        entered,
        release: Mutex::new(released),
        writes: AtomicUsize::new(0),
    });
    registry
        .publish(id, KIND, None, None, writer.clone(), &tracker)
        .unwrap();
    let permit = registry
        .prepare(id, &tracker)
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let (finished, joined) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let guard = lock_route_for_write(&permit).unwrap();
        guard.write(b"blocked write").unwrap();
        drop(guard);
        finished.send(()).unwrap();
    });
    started.recv_timeout(BOUND).unwrap();
    assert_eq!(
        registry.try_remove_route_if_kind(id, KIND, &tracker),
        Err(PtyRouteRemovalError::Busy)
    );
    let (done, received) = std::sync::mpsc::channel();
    registry.state.lock().unwrap().removal_completed = Some(done);
    registry.remove_route_if_kind(id, KIND, &tracker);
    assert_eq!(registry.backend_kind(id), Some(KIND));
    assert_eq!(writer.writes.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    joined.recv_timeout(BOUND).unwrap();
    thread.join().unwrap();
    received.recv_timeout(BOUND).unwrap();
    assert_eq!(writer.writes.load(Ordering::SeqCst), 1);
    assert_eq!(registry.backend_kind(id), None);
    assert!(!tracker.inner.lock().unwrap().routes.contains_key(&id));
}

#[tokio::test]
async fn poisoned_registry_and_lifecycle_fail_closed() {
    let (registry, tracker, id, writer) = fixture();
    let permit = registry
        .prepare(id, &tracker)
        .unwrap()
        .acquire()
        .await
        .unwrap();
    registry.poison_route_registry_for_test();
    error(registry.prepare(id, &tracker), "route_registry_poisoned");
    error(lock_route_for_write(&permit), "route_registry_poisoned");
    error(
        registry.authority_route_proof(id),
        "route_registry_poisoned",
    );
    error(
        registry.batch_snapshot_proofs(&[id]),
        "route_registry_poisoned",
    );
    registry.clear_route_registry_poison_for_test();
    poison_lifecycle(&permit.route_lifecycle);
    error(lock_route_for_write(&permit), "pty_route_poisoned");
    assert_eq!(writer.writes.load(Ordering::SeqCst), 0);
}

fn poison_lifecycle(lifecycle: &Arc<Mutex<()>>) {
    let lifecycle = lifecycle.clone();
    assert!(std::thread::spawn(move || {
        let _guard = lifecycle.lock().unwrap();
        panic!("intentional lifecycle poison");
    })
    .join()
    .is_err());
}

#[tokio::test]
async fn cancellation_and_drop_release_admission() {
    let (registry, tracker, id, _) = fixture();
    let first = registry
        .prepare(id, &tracker)
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let mut queued = Box::pin(registry.prepare(id, &tracker).unwrap().acquire());
    poll_pending(queued.as_mut());
    assert_eq!(writers(&tracker, id), 2);
    drop(queued);
    assert_eq!(writers(&tracker, id), 1);
    let preparation = registry.prepare(id, &tracker).unwrap();
    let task = tokio::spawn(preparation.acquire());
    // A barrier after polling proves that admission has happened before abort.
    let (started, notified) = tokio::sync::oneshot::channel();
    let preparation = registry.prepare(id, &tracker).unwrap();
    let abortable = tokio::spawn(async move {
        let mut future = Box::pin(preparation.acquire());
        poll_pending(future.as_mut());
        started.send(()).unwrap();
        future.await
    });
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    abortable.abort();
    assert!(tokio::time::timeout(BOUND, abortable)
        .await
        .unwrap()
        .err()
        .unwrap()
        .is_cancelled());
    task.abort();
    assert!(tokio::time::timeout(BOUND, task)
        .await
        .unwrap()
        .err()
        .unwrap()
        .is_cancelled());
    assert_eq!(writers(&tracker, id), 1);
    drop(first);
    assert_eq!(writers(&tracker, id), 0);
    tracker.fail_closed();
    let noncounted = registry
        .prepare(id, &tracker)
        .unwrap()
        .acquire()
        .await
        .unwrap();
    assert_eq!(writers(&tracker, id), 0);
    drop(noncounted);
    assert_eq!(writers(&tracker, id), 0);
}

#[test]
fn pending_drop_under_registry_poison_removes_and_ends_spawn() {
    let (registry, original, _, _) = fixture();
    let rebound = ResponseCloseState::default();
    let pending = registry.mark_spawning("cwd", "label", &original);
    let other = registry.mark_spawning("other", "other", &rebound);
    assert_eq!(original.inner.lock().unwrap().unknown_spawns, 1);
    registry.poison_route_registry_for_test();
    drop(pending);
    assert_eq!(original.inner.lock().unwrap().unknown_spawns, 0);
    assert_eq!(rebound.inner.lock().unwrap().unknown_spawns, 1);
    let (rows, _) = registry.joint_archive_snapshot(&[]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cwd, "other");
    drop(other);
    assert!(registry.joint_archive_snapshot(&[]).0.is_empty());
    assert_eq!(rebound.inner.lock().unwrap().unknown_spawns, 0);
}

#[test]
fn pending_drop_preserves_tracker_and_joint_snapshot() {
    let (registry, tracker, id, _) = fixture();
    let directory = tempfile::TempDir::new().unwrap();
    let identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    registry.state.lock().unwrap().next_seq = u64::MAX;
    let pending = registry.mark_spawning(directory.path().to_str().unwrap(), "pending", &tracker);
    assert!(tracker.inner.lock().unwrap().failed);
    assert_eq!(registry.state.lock().unwrap().next_seq, 0);
    registry.poison_route_registry_for_test();
    assert!(registry.has_pending_spawn_for_replica(&identity));
    assert_eq!(registry.kind_for_session(id).unwrap(), KIND);
    assert_eq!(registry.backend_kind(id), Some(KIND));
    assert!(registry.route_identities(id).is_some());
    let (rows, kinds) = registry.joint_archive_snapshot(&[id, Uuid::new_v4()]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "pending");
    assert_eq!(kinds, vec![Some(KIND), None]);
    drop(pending);
    registry.clear_route_registry_poison_for_test();
    registry.set_generation_for_test(u64::MAX);
    error(
        registry.publish(
            Uuid::new_v4(),
            KIND,
            None,
            None,
            Arc::new(CountedWriter::default()),
            &tracker,
        ),
        "route_generation_overflow",
    );
}

#[tokio::test]
async fn ordinary_remove_recovers_registry_poison() {
    let (registry, tracker, id, _) = fixture();
    registry.poison_route_registry_for_test();
    registry.remove_route_if_kind(id, SessionBackendKind::ContainerTransport, &tracker);
    assert_eq!(registry.backend_kind(id), Some(KIND));
    registry.remove_route_if_kind(id, KIND, &tracker);
    assert_eq!(registry.backend_kind(id), None);
    assert!(!tracker.inner.lock().unwrap().routes.contains_key(&id));
    registry.clear_route_registry_poison_for_test();
    registry
        .publish(
            id,
            KIND,
            None,
            None,
            Arc::new(CountedWriter::default()),
            &tracker,
        )
        .unwrap();
    let permit = registry
        .prepare(id, &tracker)
        .unwrap()
        .acquire()
        .await
        .unwrap();
    let guard = lock_route_for_write(&permit).unwrap();
    let (done, received) = std::sync::mpsc::channel();
    registry.state.lock().unwrap().removal_completed = Some(done);
    registry.poison_route_registry_for_test();
    registry.remove_route_if_kind(id, KIND, &tracker);
    // Simulate a replacement generation while the captured lifecycle is busy.
    // The deferred remover must compare its captured generation before deletion.
    registry
        .state
        .lock()
        .err()
        .unwrap()
        .into_inner()
        .routes
        .get_mut(&id)
        .unwrap()
        .generation += 1;
    drop(guard);
    received.recv_timeout(BOUND).unwrap();
    assert_eq!(registry.backend_kind(id), Some(KIND));
    // Restore only the fixture's generation to verify deferred poison recovery.
    registry
        .state
        .lock()
        .err()
        .unwrap()
        .into_inner()
        .routes
        .get_mut(&id)
        .unwrap()
        .generation -= 1;
    let lifecycle = permit.route_lifecycle.lock().unwrap();
    registry.remove_route_if_kind(id, KIND, &tracker);
    drop(lifecycle);
    received.recv_timeout(BOUND).unwrap();
    assert_eq!(registry.backend_kind(id), None);
    assert!(!tracker.inner.lock().unwrap().routes.contains_key(&id));
}

#[test]
fn try_remove_lifecycle_poison_is_busy() {
    let (registry, tracker, id, _) = fixture();
    let lifecycle = registry.state.lock().unwrap().routes[&id]
        .lifecycle_gate
        .clone();
    poison_lifecycle(&lifecycle);
    assert_eq!(
        registry.try_remove_route_if_kind(id, KIND, &tracker),
        Err(PtyRouteRemovalError::Busy)
    );
    registry.remove_route_if_kind(id, KIND, &tracker);
    assert_eq!(registry.backend_kind(id), Some(KIND));
    registry.poison_route_registry_for_test();
    assert_eq!(
        registry.try_remove_route_if_kind(id, KIND, &tracker),
        Err(PtyRouteRemovalError::LockPoisoned)
    );
    // Contention schedules the deferred path; poisoning on release must still recover.
    lifecycle.clear_poison();
    let lifecycle_guard = lifecycle.lock().unwrap();
    let (done, received) = std::sync::mpsc::channel();
    registry
        .state
        .lock()
        .err()
        .unwrap()
        .into_inner()
        .removal_completed = Some(done);
    registry.remove_route_if_kind(id, KIND, &tracker);
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = lifecycle_guard;
            panic!("poison lifecycle after deferred removal is scheduled");
        }))
        .is_err()
    );
    received.recv_timeout(BOUND).unwrap();
    assert!(lifecycle.is_poisoned());
    assert_eq!(registry.backend_kind(id), None);
    assert!(!tracker.inner.lock().unwrap().routes.contains_key(&id));
}

#[test]
fn snapshot_copy_holds_lifecycle_without_writing() {
    let (registry, tracker, id, writer) = fixture();
    registry.remove_route_if_kind(id, KIND, &tracker);
    let directory = tempfile::TempDir::new().unwrap();
    let identity = crate::path_identity::verify_directory(directory.path()).unwrap();
    registry
        .publish(
            id,
            KIND,
            Some(identity.clone()),
            Some(identity.clone()),
            writer.clone(),
            &tracker,
        )
        .unwrap();
    let proofs = registry
        .batch_snapshot_proofs(&[id, Uuid::new_v4(), id])
        .unwrap();
    assert!(proofs[1].is_none());
    assert_eq!(
        proofs[0].as_ref().unwrap().route_generation,
        proofs[2].as_ref().unwrap().route_generation
    );
    let proof = proofs[0].as_ref().unwrap();
    let copy_guard = proof.lock_verified(KIND, &identity, &identity).unwrap();
    let (done, received) = std::sync::mpsc::channel();
    registry.state.lock().unwrap().removal_completed = Some(done);
    registry.remove_route_if_kind(id, KIND, &tracker);
    assert_eq!(registry.backend_kind(id), Some(KIND));
    assert_eq!(writer.writes.load(Ordering::SeqCst), 0);
    drop(copy_guard);
    received.recv_timeout(BOUND).unwrap();
    assert_eq!(registry.backend_kind(id), None);
    assert!(!proof.matches_current(KIND, &identity, &identity));
}
