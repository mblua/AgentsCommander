use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use tokio_util::sync::CancellationToken;

use crate::session::manager::SessionManager;

type RouteResult = Result<CriticalAdmissionOutcome<()>, String>;

struct RouteDrop(Arc<AtomicBool>);

impl Drop for RouteDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct RecordingLifecycle {
    calls: Mutex<Vec<(Uuid, i32)>>,
    response: Mutex<Option<oneshot::Receiver<RouteResult>>>,
    dropped: Arc<AtomicBool>,
}

impl ContainerLifecycle for RecordingLifecycle {
    fn route_lost(
        &self,
        id: Uuid,
        exit_code: i32,
    ) -> Pin<Box<dyn Future<Output = RouteResult> + Send + '_>> {
        Box::pin(async move {
            let _drop = RouteDrop(self.dropped.clone());
            self.calls.lock().expect("calls lock").push((id, exit_code));
            let response = self.response.lock().expect("response lock").take();
            response.expect("one route").await.expect("response sender")
        })
    }
}

#[tokio::test]
async fn idle_sender_clones_and_unpolled_future_retain_no_coordinator() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
    let weak = Arc::downgrade(&coordinator.inner);
    let sender = coordinator.container_lifecycle_sender();
    let clone = sender.clone();
    let future = clone.route_lost(Uuid::new_v4(), 17);
    drop(coordinator);
    assert!(weak.upgrade().is_none());
    assert_eq!(
        future.await,
        Err(SelectionCoordinatorError::Unavailable.to_string())
    );
    assert!(weak.upgrade().is_none());
    drop(sender);
}

#[tokio::test]
async fn route_lost_lazily_forwards_inputs_and_every_outcome() {
    for expected in [
        Ok(CriticalAdmissionOutcome::Completed(())),
        Ok(CriticalAdmissionOutcome::AlreadyPending),
        Err("unchanged route error".to_string()),
    ] {
        let (response, receiver) = oneshot::channel();
        let lifecycle = Arc::new(RecordingLifecycle {
            calls: Mutex::new(Vec::new()),
            response: Mutex::new(Some(receiver)),
            dropped: Arc::new(AtomicBool::new(false)),
        });
        let sender = ContainerLifecycleSender::new(lifecycle.clone());
        let id = Uuid::new_v4();
        let mut future = Box::pin(sender.route_lost(id, -17));
        assert!(lifecycle.calls.lock().expect("calls lock").is_empty());
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
        assert_eq!(*lifecycle.calls.lock().expect("calls lock"), [(id, -17)]);
        assert!(!lifecycle.dropped.load(Ordering::SeqCst));
        response.send(expected.clone()).expect("waiting route");
        assert_eq!(future.await, expected);
    }
}

#[test]
fn dropping_polled_route_immediately_drops_delegated_future() {
    let (_response, receiver) = oneshot::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let lifecycle = Arc::new(RecordingLifecycle {
        calls: Mutex::new(Vec::new()),
        response: Mutex::new(Some(receiver)),
        dropped: dropped.clone(),
    });
    let sender = ContainerLifecycleSender::new(lifecycle.clone());
    let id = Uuid::new_v4();
    let mut future = Box::pin(sender.route_lost(id, 42));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
    assert_eq!(*lifecycle.calls.lock().expect("calls lock"), [(id, 42)]);
    assert!(!dropped.load(Ordering::SeqCst));
    drop(future);
    assert!(dropped.load(Ordering::SeqCst));
}
