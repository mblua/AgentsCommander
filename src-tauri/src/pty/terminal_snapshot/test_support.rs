use super::{TerminalSnapshotBlockingStage, TerminalSnapshotState};
#[cfg(unix)]
use crate::path_identity::UnixTrackedCleanupStage;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

impl TerminalSnapshotState {
    #[cfg(test)]
    pub(crate) fn test_artifact_counts(&self) -> (usize, usize, usize) {
        let registry = self.artifacts.lock().expect("artifact registry");
        (
            registry.directories.len(),
            registry.files.len(),
            registry.reservations,
        )
    }

    #[cfg(test)]
    pub(crate) fn sweep_artifacts_for_test(&self, force: bool) {
        self.sweep_artifacts(force);
    }

    #[cfg(test)]
    pub(crate) fn reset_test_target_lookup_counts(&self) {
        self.test_state
            .target_session_lookups
            .store(0, Ordering::SeqCst);
        self.test_state
            .target_route_lookups
            .store(0, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn test_target_lookup_counts(&self) -> TerminalSnapshotTestLookupCounts {
        TerminalSnapshotTestLookupCounts {
            target_session_lookups: self
                .test_state
                .target_session_lookups
                .load(Ordering::SeqCst),
            target_route_lookups: self.test_state.target_route_lookups.load(Ordering::SeqCst),
        }
    }

    #[cfg(test)]
    pub(super) fn install_blocking_control(
        &self,
        stage: TerminalSnapshotBlockingStage,
        control: Arc<TerminalSnapshotBlockingControl>,
    ) {
        self.test_state
            .blocking_controls
            .lock()
            .expect("blocking control queue")
            .entry(stage)
            .or_default()
            .push_back(control);
    }

    #[cfg(test)]
    pub(super) fn take_blocking_control(
        &self,
        stage: TerminalSnapshotBlockingStage,
    ) -> Option<Arc<TerminalSnapshotBlockingControl>> {
        let mut controls = self
            .test_state
            .blocking_controls
            .lock()
            .expect("blocking control queue");
        let control = controls.get_mut(&stage).and_then(VecDeque::pop_front);
        if controls.get(&stage).is_some_and(VecDeque::is_empty) {
            controls.remove(&stage);
        }
        control
    }

    #[cfg(test)]
    pub(super) fn mark_blocking_payload_retention(
        &self,
        stage: TerminalSnapshotBlockingStage,
        bytes: usize,
    ) {
        if let Some(control) = self
            .test_state
            .blocking_controls
            .lock()
            .expect("blocking control queue")
            .get(&stage)
            .and_then(VecDeque::front)
        {
            control.set_retained_payload_bytes(bytes);
        }
    }

    #[cfg(test)]
    pub(super) fn has_blocking_controls(&self) -> bool {
        !self
            .test_state
            .blocking_controls
            .lock()
            .expect("blocking control queue")
            .is_empty()
            || !self
                .test_state
                .host_cancellation_hooks
                .lock()
                .expect("host cancellation hook queue")
                .is_empty()
            || !self
                .test_state
                .host_finalizer_controls
                .lock()
                .expect("host finalizer control queue")
                .is_empty()
            || !self
                .test_state
                .response_publication_hooks
                .lock()
                .expect("response publication hook queue")
                .is_empty()
            || !self
                .test_state
                .response_publication_failures
                .lock()
                .expect("response publication failure queue")
                .is_empty()
    }

    #[cfg(test)]
    pub(crate) fn install_host_cancellation_hook(
        &self,
        stage: TerminalSnapshotHostCancellationStage,
        hook: impl FnOnce() + Send + 'static,
    ) {
        self.test_state
            .host_cancellation_hooks
            .lock()
            .expect("host cancellation hook queue")
            .entry(stage)
            .or_default()
            .push_back(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn install_next_host_finalizer_control(
        &self,
        control: Arc<TerminalSnapshotHostFinalizerControl>,
    ) {
        self.test_state
            .host_finalizer_controls
            .lock()
            .expect("host finalizer control queue")
            .push_back(control);
    }

    #[cfg(test)]
    pub(crate) fn install_host_final_handoff_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *self
            .test_state
            .host_before_final_revalidation
            .lock()
            .expect("host final-handoff test hook lock") = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn install_api_before_capture_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *self
            .test_state
            .api_before_capture
            .lock()
            .expect("API before-capture test hook lock") = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn install_api_after_response_bytes_hook(
        &self,
        hook: impl FnOnce() + Send + 'static,
    ) {
        *self
            .test_state
            .api_after_response_bytes
            .lock()
            .expect("API response-bytes test hook lock") = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn install_api_final_handoff_hook(&self, hook: impl FnOnce() + Send + 'static) {
        *self
            .test_state
            .api_before_final_binding
            .lock()
            .expect("API final-handoff test hook lock") = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub(crate) fn test_api_success_handoffs(&self) -> usize {
        self.test_state.api_success_handoffs.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn record_api_success_handoff(&self) {
        self.test_state
            .api_success_handoffs
            .fetch_add(1, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn install_response_publication_hook(
        &self,
        stage: TerminalSnapshotResponsePublicationStage,
        hook: impl FnOnce(&Path, &Path) + Send + 'static,
    ) {
        self.test_state
            .response_publication_hooks
            .lock()
            .expect("response publication hook queue")
            .entry(stage)
            .or_default()
            .push_back(Box::new(hook));
    }

    #[cfg(all(test, unix))]
    pub(crate) fn install_unix_cleanup_hook(
        &self,
        claim_leaf: Option<std::ffi::OsString>,
        hook: impl FnMut(UnixTrackedCleanupStage, &Path, &Path) + Send + 'static,
    ) {
        self.test_state
            .unix_cleanup_controls
            .lock()
            .expect("Unix cleanup control queue")
            .push_back(TerminalSnapshotUnixCleanupControl {
                claim_leaf,
                hook: Box::new(hook),
            });
    }

    #[cfg(all(test, unix))]
    pub(super) fn take_unix_cleanup_control(&self) -> Option<TerminalSnapshotUnixCleanupControl> {
        self.test_state
            .unix_cleanup_controls
            .lock()
            .expect("Unix cleanup control queue")
            .pop_front()
    }

    #[cfg(test)]
    pub(crate) fn install_response_after_publish_hook(&self, hook: impl FnOnce() + Send + 'static) {
        self.install_response_publication_hook(
            TerminalSnapshotResponsePublicationStage::AfterAtomicRename,
            move |_, _| hook(),
        );
    }

    #[cfg(test)]
    pub(crate) fn install_response_publication_failure(
        &self,
        failure: TerminalSnapshotResponsePublicationFailure,
    ) {
        self.test_state
            .response_publication_failures
            .lock()
            .expect("response publication failure queue")
            .push_back(failure);
    }

    #[cfg(test)]
    pub(crate) fn take_response_publication_failure(
        &self,
        expected: TerminalSnapshotResponsePublicationFailure,
    ) -> bool {
        let mut failures = self
            .test_state
            .response_publication_failures
            .lock()
            .expect("response publication failure queue");
        if failures.front() == Some(&expected) {
            failures.pop_front();
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    pub(super) fn run_host_final_handoff_hook(&self) {
        let hook = self
            .test_state
            .host_before_final_revalidation
            .lock()
            .expect("host final-handoff test hook lock")
            .take();
        if let Some(hook) = hook {
            hook();
        }
    }

    #[cfg(test)]
    pub(crate) fn run_host_cancellation_hook(&self, stage: TerminalSnapshotHostCancellationStage) {
        let hook = {
            let mut hooks = self
                .test_state
                .host_cancellation_hooks
                .lock()
                .expect("host cancellation hook queue");
            let hook = hooks.get_mut(&stage).and_then(VecDeque::pop_front);
            if hooks.get(&stage).is_some_and(VecDeque::is_empty) {
                hooks.remove(&stage);
            }
            hook
        };
        if let Some(hook) = hook {
            hook();
        }
    }

    #[cfg(test)]
    pub(super) fn take_next_host_finalizer_control(
        &self,
    ) -> Option<Arc<TerminalSnapshotHostFinalizerControl>> {
        self.test_state
            .host_finalizer_controls
            .lock()
            .expect("host finalizer control queue")
            .pop_front()
    }

    #[cfg(test)]
    pub(super) async fn run_api_before_capture_hook(&self) {
        let hook = self
            .test_state
            .api_before_capture
            .lock()
            .expect("API before-capture test hook lock")
            .take();
        if let Some(hook) = hook {
            hook();
            tokio::task::yield_now().await;
        }
    }

    #[cfg(test)]
    pub(crate) async fn run_api_after_response_bytes_hook(&self) {
        let hook = self
            .test_state
            .api_after_response_bytes
            .lock()
            .expect("API response-bytes test hook lock")
            .take();
        if let Some(hook) = hook {
            hook();
            tokio::task::yield_now().await;
        }
    }

    #[cfg(test)]
    pub(crate) async fn run_api_final_handoff_hook(&self) {
        let hook = self
            .test_state
            .api_before_final_binding
            .lock()
            .expect("API final-handoff test hook lock")
            .take();
        if let Some(hook) = hook {
            hook();
            tokio::task::yield_now().await;
        }
    }

    #[cfg(test)]
    pub(crate) fn run_response_publication_hook(
        &self,
        stage: TerminalSnapshotResponsePublicationStage,
        temporary: &Path,
        destination: &Path,
    ) {
        let hook = {
            let mut hooks = self
                .test_state
                .response_publication_hooks
                .lock()
                .expect("response publication hook queue");
            let hook = hooks.get_mut(&stage).and_then(VecDeque::pop_front);
            if hooks.get(&stage).is_some_and(VecDeque::is_empty) {
                hooks.remove(&stage);
            }
            hook
        };
        if let Some(hook) = hook {
            hook(temporary, destination);
        }
    }
}

#[cfg(test)]
pub(crate) type TerminalSnapshotTestHook = Box<dyn FnOnce() + Send + 'static>;

#[cfg(test)]
pub(crate) type TerminalSnapshotResponsePublicationHook =
    Box<dyn FnOnce(&Path, &Path) + Send + 'static>;

#[cfg(all(test, unix))]
pub(crate) type TerminalSnapshotUnixCleanupHook =
    Box<dyn FnMut(UnixTrackedCleanupStage, &Path, &Path) + Send + 'static>;

#[cfg(all(test, unix))]
pub(crate) struct TerminalSnapshotUnixCleanupControl {
    pub(crate) claim_leaf: Option<std::ffi::OsString>,
    pub(crate) hook: TerminalSnapshotUnixCleanupHook,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TerminalSnapshotResponsePublicationStage {
    BeforeDirectoryRetention,
    BeforeTemporaryCreate,
    BeforeTemporaryCommit,
    AfterTemporaryCommit,
    AfterTemporaryWrite,
    BeforeAtomicRename,
    AfterAtomicRename,
    AfterFinalVerification,
    AfterRegistryCommit,
    AfterParentSync,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSnapshotResponsePublicationFailure {
    FileSync,
    ParentSync,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TerminalSnapshotHostCancellationStage {
    Processing,
    ResponseBytesReady,
    BeforePublish,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct TerminalSnapshotBlockingControlState {
    pub(crate) entered: bool,
    pub(crate) released: bool,
    pub(crate) deadline_expired: bool,
    pub(crate) completed: bool,
    pub(crate) retained_payload_bytes: usize,
}

#[cfg(test)]
pub(crate) struct TerminalSnapshotBlockingControl {
    pub(crate) state: Mutex<TerminalSnapshotBlockingControlState>,
    pub(crate) changed: std::sync::Condvar,
    pub(crate) deadline_changed: tokio::sync::Notify,
    pub(crate) panic_after_release: Option<String>,
}

#[cfg(test)]
impl TerminalSnapshotBlockingControl {
    pub(crate) fn new(panic_after_release: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(TerminalSnapshotBlockingControlState::default()),
            changed: std::sync::Condvar::new(),
            deadline_changed: tokio::sync::Notify::new(),
            panic_after_release,
        })
    }

    pub(super) fn wait_for(&self, completed: bool, label: &'static str) {
        let limit = Instant::now() + Duration::from_secs(60);
        let mut state = self.state.lock().expect("blocking control state");
        while if completed {
            !state.completed
        } else {
            !state.entered
        } {
            let remaining = limit.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "{label}");
            let (next, timeout) = self
                .changed
                .wait_timeout(state, remaining)
                .expect("blocking control wait");
            state = next;
            let reached = if completed {
                state.completed
            } else {
                state.entered
            };
            assert!(!timeout.timed_out() || reached, "{label}");
        }
    }

    pub(crate) fn wait_until_entered(&self) {
        self.wait_for(
            false,
            "blocking worker did not reach its deterministic barrier",
        );
    }

    pub(crate) fn wait_until_completed(&self) {
        self.wait_for(true, "detached blocking worker did not complete");
    }

    pub(crate) fn expire_deadline(&self) {
        let mut state = self.state.lock().expect("blocking control state");
        state.deadline_expired = true;
        drop(state);
        self.deadline_changed.notify_waiters();
    }

    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("blocking control state");
        state.released = true;
        self.changed.notify_all();
    }

    pub(crate) fn retained_payload_bytes(&self) -> usize {
        self.state
            .lock()
            .expect("blocking control state")
            .retained_payload_bytes
    }

    pub(super) fn set_retained_payload_bytes(&self, bytes: usize) {
        self.state
            .lock()
            .expect("blocking control state")
            .retained_payload_bytes = bytes;
    }

    pub(super) fn enter_worker(&self) {
        let mut state = self.state.lock().expect("blocking control state");
        state.entered = true;
        self.changed.notify_all();
        while !state.released {
            state = self
                .changed
                .wait(state)
                .expect("blocking control release wait");
        }
        drop(state);
        if let Some(payload) = self.panic_after_release.clone() {
            std::panic::panic_any(payload);
        }
    }

    pub(super) fn complete_worker(&self) {
        let mut state = self.state.lock().expect("blocking control state");
        state.completed = true;
        self.changed.notify_all();
    }

    pub(super) async fn deadline_expired(&self) {
        loop {
            let changed = self.deadline_changed.notified();
            if self
                .state
                .lock()
                .expect("blocking control state")
                .deadline_expired
            {
                return;
            }
            changed.await;
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSnapshotHostFinalizerStage {
    RevalidationEntry,
    RouteVerification,
    FinalDeadline,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct TerminalSnapshotHostFinalizerControlState {
    pub(crate) entered: bool,
    pub(crate) released: bool,
    pub(crate) deadline_expired: bool,
    pub(crate) retained_response_bytes: usize,
}

#[cfg(test)]
pub(crate) struct TerminalSnapshotHostFinalizerControl {
    pub(crate) stage: TerminalSnapshotHostFinalizerStage,
    pub(crate) state: Mutex<TerminalSnapshotHostFinalizerControlState>,
    pub(crate) changed: std::sync::Condvar,
    pub(crate) panic_after_release: Option<String>,
}

#[cfg(test)]
impl TerminalSnapshotHostFinalizerControl {
    pub(crate) fn new(
        stage: TerminalSnapshotHostFinalizerStage,
        panic_after_release: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            stage,
            state: Mutex::new(TerminalSnapshotHostFinalizerControlState::default()),
            changed: std::sync::Condvar::new(),
            panic_after_release,
        })
    }

    pub(crate) fn wait_until_entered(&self) {
        let limit = Instant::now() + Duration::from_secs(60);
        let mut state = self.state.lock().expect("host finalizer control state");
        while !state.entered {
            let remaining = limit.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "host finalizer did not reach its deterministic barrier"
            );
            let (next, timeout) = self
                .changed
                .wait_timeout(state, remaining)
                .expect("host finalizer control wait");
            state = next;
            assert!(
                !timeout.timed_out() || state.entered,
                "host finalizer did not reach its deterministic barrier"
            );
        }
    }

    pub(crate) fn expire_deadline(&self) {
        self.state
            .lock()
            .expect("host finalizer control state")
            .deadline_expired = true;
    }

    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("host finalizer control state");
        state.released = true;
        self.changed.notify_all();
    }

    pub(crate) fn retained_response_bytes(&self) -> usize {
        self.state
            .lock()
            .expect("host finalizer control state")
            .retained_response_bytes
    }

    pub(super) fn set_retained_response_bytes(&self, bytes: usize) {
        self.state
            .lock()
            .expect("host finalizer control state")
            .retained_response_bytes = bytes;
    }

    pub(super) fn deadline_expired(&self) -> bool {
        self.state
            .lock()
            .expect("host finalizer control state")
            .deadline_expired
    }

    pub(super) fn enter_stage(&self, stage: TerminalSnapshotHostFinalizerStage) {
        if self.stage != stage {
            return;
        }
        let mut state = self.state.lock().expect("host finalizer control state");
        state.entered = true;
        self.changed.notify_all();
        while !state.released {
            state = self
                .changed
                .wait(state)
                .expect("host finalizer control release wait");
        }
        drop(state);
        if let Some(payload) = self.panic_after_release.clone() {
            std::panic::panic_any(payload);
        }
    }
}

#[cfg(test)]
pub(crate) struct TerminalSnapshotTestTaskOutput<T> {
    pub(crate) outcome: Option<Result<T, crate::logging::PayloadPanic>>,
    pub(crate) control: Option<Arc<TerminalSnapshotBlockingControl>>,
}

#[cfg(test)]
impl<T> TerminalSnapshotTestTaskOutput<T> {
    pub(super) fn into_outcome(mut self) -> Result<T, crate::logging::PayloadPanic> {
        let outcome = self
            .outcome
            .take()
            .expect("blocking task output consumed exactly once");
        if let Some(control) = self.control.take() {
            control.complete_worker();
        }
        outcome
    }
}

#[cfg(test)]
impl<T> Drop for TerminalSnapshotTestTaskOutput<T> {
    fn drop(&mut self) {
        drop(self.outcome.take());
        if let Some(control) = self.control.take() {
            control.complete_worker();
        }
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct TerminalSnapshotTestState {
    pub(crate) target_session_lookups: std::sync::atomic::AtomicUsize,
    pub(crate) target_route_lookups: std::sync::atomic::AtomicUsize,
    pub(crate) api_success_handoffs: std::sync::atomic::AtomicUsize,
    pub(crate) host_before_final_revalidation: Mutex<Option<TerminalSnapshotTestHook>>,
    pub(crate) host_cancellation_hooks:
        Mutex<HashMap<TerminalSnapshotHostCancellationStage, VecDeque<TerminalSnapshotTestHook>>>,
    pub(crate) host_finalizer_controls: Mutex<VecDeque<Arc<TerminalSnapshotHostFinalizerControl>>>,
    pub(crate) api_before_capture: Mutex<Option<TerminalSnapshotTestHook>>,
    pub(crate) api_after_response_bytes: Mutex<Option<TerminalSnapshotTestHook>>,
    pub(crate) api_before_final_binding: Mutex<Option<TerminalSnapshotTestHook>>,
    pub(crate) response_publication_hooks: Mutex<
        HashMap<
            TerminalSnapshotResponsePublicationStage,
            VecDeque<TerminalSnapshotResponsePublicationHook>,
        >,
    >,
    pub(crate) response_publication_failures:
        Mutex<VecDeque<TerminalSnapshotResponsePublicationFailure>>,
    #[cfg(unix)]
    pub(crate) unix_cleanup_controls: Mutex<VecDeque<TerminalSnapshotUnixCleanupControl>>,
    pub(crate) blocking_controls: Mutex<
        HashMap<TerminalSnapshotBlockingStage, VecDeque<Arc<TerminalSnapshotBlockingControl>>>,
    >,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalSnapshotTestLookupCounts {
    pub target_session_lookups: usize,
    pub target_route_lookups: usize,
}
