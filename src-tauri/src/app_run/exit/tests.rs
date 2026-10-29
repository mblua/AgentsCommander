use super::*;
use futures::FutureExt;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Default)]
struct RecordingHost {
    operations: Arc<Mutex<Vec<String>>>,
    pty: Option<Arc<Mutex<PtyManager>>>,
}

impl RecordingHost {
    fn with_empty_pty() -> Self {
        let backend = Arc::new(
            crate::pty::container_backend::ContainerTransportBackend::new(
                Arc::new(Mutex::new(HashMap::new())),
                crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {}),
                None,
                None,
            ),
        );
        Self {
            operations: Arc::new(Mutex::new(Vec::new())),
            pty: Some(Arc::new(Mutex::new(PtyManager::new_for_test(backend)))),
        }
    }

    fn record(&self, operation: String) {
        self.operations.lock().unwrap().push(operation);
    }

    fn operations(&self) -> Vec<String> {
        self.operations.lock().unwrap().clone()
    }
}

impl ExitEvents for RecordingHost {
    fn terminal_attached(&self, session_id: String) {
        self.record(format!("attached:{session_id}"));
    }
}

impl ExitHost for RecordingHost {
    fn quit_window_gone(&self, label: &str) {
        self.record(format!("quit:{label}"));
    }

    fn optional_pty(&self) -> Option<Arc<Mutex<PtyManager>>> {
        self.record("pty-lookup".to_string());
        self.pty.clone()
    }

    fn required_pty(&self) -> Arc<Mutex<PtyManager>> {
        panic!("WindowDestroyed must not resolve the shutdown PTY owner");
    }

    fn context_alert_monitor(&self) -> Option<Arc<ContextAlertMonitor>> {
        panic!("WindowDestroyed must not resolve the shutdown monitor");
    }

    fn destroy_main_dependents(&self) {
        self.record("main-dependents".to_string());
    }

    fn overlay_destroyed(&self, label: String) {
        self.record(format!("overlay-spawn:{label}"));
    }

    fn spawn_background<F: Future<Output = ()> + Send + 'static>(&self, future: F) {
        self.record("spawn".to_string());
        future
            .now_or_never()
            .expect("fake work must finish in one poll");
    }

    fn block_on<F: Future>(&self, _future: F) -> F::Output {
        panic!("WindowDestroyed must not block on shutdown work");
    }
}

fn detached() -> DetachedSessionsState {
    Arc::new(Mutex::new(HashSet::new()))
}

async fn close_memory(owner: Arc<AtomicUsize>) {
    owner.fetch_add(1, Ordering::SeqCst);
}

#[test]
fn synchronous_quit_precedes_attachment_spawn() {
    let host = RecordingHost::with_empty_pty();
    let owner = Arc::new(AtomicUsize::new(0));
    on_window_destroyed(
        &host,
        "other".to_string(),
        &detached(),
        &owner,
        &close_memory,
    );
    assert_eq!(host.operations(), ["quit:other", "pty-lookup", "spawn"]);
    assert_eq!(owner.load(Ordering::SeqCst), 0);
}

#[test]
fn absent_optional_pty_still_closes_spec_after_synchronous_quit() {
    let host = RecordingHost::default();
    let owner = Arc::new(AtomicUsize::new(0));
    on_window_destroyed(
        &host,
        "spec-board".to_string(),
        &detached(),
        &owner,
        &close_memory,
    );
    assert_eq!(
        host.operations(),
        ["quit:spec-board", "pty-lookup", "spawn"]
    );
    assert_eq!(owner.load(Ordering::SeqCst), 1);
}

#[test]
fn synchronous_quit_precedes_overlay_spawn_without_optional_pty() {
    let host = RecordingHost::default();
    let owner = Arc::new(AtomicUsize::new(0));
    on_window_destroyed(
        &host,
        "screenshot-overlay-test".to_string(),
        &detached(),
        &owner,
        &close_memory,
    );
    assert_eq!(
        host.operations(),
        [
            "quit:screenshot-overlay-test",
            "pty-lookup",
            "overlay-spawn:screenshot-overlay-test"
        ]
    );
    assert_eq!(owner.load(Ordering::SeqCst), 0);
}

#[test]
fn terminal_destroy_clears_only_live_tracking_and_emits_canonical_uuid() {
    let host = RecordingHost::default();
    let owner = Arc::new(AtomicUsize::new(0));
    let id = uuid::Uuid::from_u128(1);
    let other = uuid::Uuid::from_u128(2);
    let set = detached();
    set.lock().unwrap().extend([id, other]);
    let label = format!("terminal-{}", id.simple());
    on_window_destroyed(&host, label.clone(), &set, &owner, &close_memory);
    assert_eq!(*set.lock().unwrap(), HashSet::from([other]));
    assert_eq!(
        host.operations(),
        [
            format!("quit:{label}"),
            "pty-lookup".to_string(),
            format!("attached:{id}")
        ]
    );
    assert_eq!(owner.load(Ordering::SeqCst), 0);
}

#[test]
fn invalid_terminal_label_never_clears_tracking_or_emits() {
    let host = RecordingHost::default();
    let owner = Arc::new(AtomicUsize::new(0));
    let id = uuid::Uuid::from_u128(1);
    let set = detached();
    set.lock().unwrap().insert(id);
    let label = "terminal-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".to_string();
    on_window_destroyed(&host, label.clone(), &set, &owner, &close_memory);
    assert_eq!(*set.lock().unwrap(), HashSet::from([id]));
    assert_eq!(
        host.operations(),
        [format!("quit:{label}"), "pty-lookup".to_string()]
    );
    assert_eq!(owner.load(Ordering::SeqCst), 0);
}

#[test]
fn main_dependents_follow_quit_and_attachment_spawn() {
    let host = RecordingHost::with_empty_pty();
    let owner = Arc::new(AtomicUsize::new(0));
    on_window_destroyed(
        &host,
        "main".to_string(),
        &detached(),
        &owner,
        &close_memory,
    );
    assert_eq!(
        host.operations(),
        ["quit:main", "pty-lookup", "spawn", "main-dependents"]
    );
    assert_eq!(owner.load(Ordering::SeqCst), 0);
}
