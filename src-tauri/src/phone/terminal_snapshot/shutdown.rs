use super::failure::record_host_ingress_failure;
use std::collections::HashMap;
use terminal_snapshot_renderer::TerminalSnapshotReasonCode;
use uuid::Uuid;

const SCANNER_TASK_ABORTABLE: u8 = 0;
const SCANNER_TASK_OWNED: u8 = 1;
const SCANNER_TASK_ABORTING: u8 = 2;
const SCANNER_TASK_TERMINAL: u8 = 3;
const SCANNER_TASK_FINALIZER: u8 = 4;
const SCANNER_TASK_CAP: usize = 8_192;
const SCANNER_ABORT_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

pub(super) type ScannerTaskCleanup = std::sync::Arc<dyn Fn() + Send + Sync + 'static>;

#[cfg(test)]
pub(crate) struct SnapshotScannerTaskStartControl {
    entered: std::sync::Mutex<bool>,
    changed: std::sync::Condvar,
    release: tokio::sync::Semaphore,
}

#[cfg(test)]
impl SnapshotScannerTaskStartControl {
    pub(crate) fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            entered: std::sync::Mutex::new(false),
            changed: std::sync::Condvar::new(),
            release: tokio::sync::Semaphore::new(0),
        })
    }

    pub(crate) fn wait_until_entered(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut entered = self.entered.lock().expect("scanner start control state");
        while !*entered {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "scanner task did not reach its pre-service barrier"
            );
            let (next, timeout) = self
                .changed
                .wait_timeout(entered, remaining)
                .expect("scanner start control wait");
            entered = next;
            assert!(
                !timeout.timed_out() || *entered,
                "scanner task did not reach its pre-service barrier"
            );
        }
    }

    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }

    pub(super) async fn enter(&self) {
        {
            let mut entered = self.entered.lock().expect("scanner start control state");
            *entered = true;
            self.changed.notify_all();
        }
        if let Ok(permit) = self.release.acquire().await {
            permit.forget();
        }
    }
}

#[derive(Clone)]
pub(super) struct ScannerTaskOwnership {
    pub(super) phase: std::sync::Arc<std::sync::atomic::AtomicU8>,
}

impl ScannerTaskOwnership {
    pub(super) fn new() -> Self {
        Self {
            phase: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(SCANNER_TASK_ABORTABLE)),
        }
    }

    pub(super) fn mark_owned(&self) -> bool {
        self.phase
            .compare_exchange(
                SCANNER_TASK_ABORTABLE,
                SCANNER_TASK_OWNED,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
    }

    pub(super) fn mark_finalizer(&self) {
        self.phase
            .store(SCANNER_TASK_FINALIZER, std::sync::atomic::Ordering::Release);
    }
}

struct ScannerTaskRecord {
    audit_request_id: Option<String>,
    phase: std::sync::Arc<std::sync::atomic::AtomicU8>,
    handle: tauri::async_runtime::JoinHandle<()>,
    cleanup: ScannerTaskCleanup,
}

#[derive(Default)]
struct ScannerShutdownState {
    sealed: bool,
    tasks: HashMap<Uuid, ScannerTaskRecord>,
    #[cfg(test)]
    task_start_controls:
        std::collections::VecDeque<std::sync::Arc<SnapshotScannerTaskStartControl>>,
}

#[derive(Clone)]
pub(crate) struct SnapshotScannerShutdownOwner {
    inner: std::sync::Arc<std::sync::Mutex<ScannerShutdownState>>,
}

static ACTIVE_SCANNER_SHUTDOWN_OWNER: std::sync::OnceLock<
    std::sync::Mutex<Option<SnapshotScannerShutdownOwner>>,
> = std::sync::OnceLock::new();

impl Default for SnapshotScannerShutdownOwner {
    fn default() -> Self {
        let owner = Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(ScannerShutdownState::default())),
        };
        let active = ACTIVE_SCANNER_SHUTDOWN_OWNER.get_or_init(|| std::sync::Mutex::new(None));
        match active.lock() {
            Ok(mut slot) => *slot = Some(owner.clone()),
            Err(poisoned) => *poisoned.into_inner() = Some(owner.clone()),
        }
        owner
    }
}

#[derive(Default)]
pub(crate) struct SnapshotScannerDrainResult {
    pub(crate) terminal: bool,
    pub(crate) joined: usize,
    pub(crate) aborted: usize,
    pub(crate) retained: Vec<String>,
}

impl SnapshotScannerShutdownOwner {
    pub(crate) fn active() -> Option<Self> {
        let active = ACTIVE_SCANNER_SHUTDOWN_OWNER.get()?;
        match active.lock() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// (#1399) An owner that does NOT claim `ACTIVE_SCANNER_SHUTDOWN_OWNER`.
    /// For scanners that will never run a task and must never displace the
    /// long-lived poller's owner on the shutdown path.
    pub(crate) fn unregistered() -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(ScannerShutdownState::default())),
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ScannerShutdownState> {
        match self.inner.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub(crate) fn seal(&self) {
        self.lock_state().sealed = true;
    }

    pub(crate) fn is_sealed(&self) -> bool {
        self.lock_state().sealed
    }

    #[cfg(test)]
    pub(crate) fn task_count(&self) -> usize {
        self.lock_state().tasks.len()
    }

    #[cfg(test)]
    pub(crate) fn install_next_task_start_control(
        &self,
        control: std::sync::Arc<SnapshotScannerTaskStartControl>,
    ) {
        self.lock_state().task_start_controls.push_back(control);
    }

    #[cfg(test)]
    pub(super) fn take_next_task_start_control(
        &self,
    ) -> Option<std::sync::Arc<SnapshotScannerTaskStartControl>> {
        self.lock_state().task_start_controls.pop_front()
    }

    #[cfg(test)]
    pub(super) async fn join_all_for_test(&self) {
        let tasks = {
            let mut state = self.lock_state();
            std::mem::take(&mut state.tasks)
        };
        for (_, task) in tasks {
            task.handle.await.expect("terminal snapshot mailbox task");
            (task.cleanup)();
        }
    }

    pub(super) fn spawn_tracked<F>(
        &self,
        audit_request_id: Option<String>,
        phase: std::sync::Arc<std::sync::atomic::AtomicU8>,
        cleanup: ScannerTaskCleanup,
        future: F,
    ) -> bool
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut state = self.lock_state();
        if state.sealed || state.tasks.len() >= SCANNER_TASK_CAP {
            return false;
        }
        let task_phase = phase.clone();
        let handle = tauri::async_runtime::spawn(async move {
            future.await;
            task_phase.store(SCANNER_TASK_TERMINAL, std::sync::atomic::Ordering::Release);
        });
        state.tasks.insert(
            Uuid::new_v4(),
            ScannerTaskRecord {
                audit_request_id,
                phase,
                handle,
                cleanup,
            },
        );
        true
    }

    pub(crate) async fn seal_and_drain_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> SnapshotScannerDrainResult {
        self.seal();
        let tasks = {
            let mut state = self.lock_state();
            std::mem::take(&mut state.tasks)
        };
        let mut result = SnapshotScannerDrainResult::default();
        let mut retained = HashMap::new();

        for (task_id, mut task) in tasks {
            let grace_deadline =
                std::cmp::min(deadline, tokio::time::Instant::now() + SCANNER_ABORT_GRACE);
            if let Ok(joined) = tokio::time::timeout_at(grace_deadline, &mut task.handle).await {
                if joined.is_err() {
                    log::error!("[terminal-snapshot] stage=host_task code=internal");
                }
                (task.cleanup)();
                result.joined += 1;
                continue;
            }

            if task
                .phase
                .compare_exchange(
                    SCANNER_TASK_ABORTABLE,
                    SCANNER_TASK_ABORTING,
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                )
                .is_ok()
            {
                if let Some(request_id) = &task.audit_request_id {
                    record_host_ingress_failure(
                        Some(request_id.clone()),
                        None,
                        TerminalSnapshotReasonCode::AuthorityChanged,
                    );
                }
                task.handle.abort();
                result.aborted += 1;
                match tokio::time::timeout_at(deadline, &mut task.handle).await {
                    Ok(_) => {
                        task.phase
                            .store(SCANNER_TASK_TERMINAL, std::sync::atomic::Ordering::Release);
                        (task.cleanup)();
                        result.joined += 1;
                    }
                    Err(_) => {
                        result
                            .retained
                            .push("reason=terminal-snapshot-aborting state=retained".to_string());
                        retained.insert(task_id, task);
                    }
                }
                continue;
            }

            match tokio::time::timeout_at(deadline, &mut task.handle).await {
                Ok(joined) => {
                    if joined.is_err() {
                        log::error!("[terminal-snapshot] stage=host_task code=internal");
                    }
                    (task.cleanup)();
                    result.joined += 1;
                }
                Err(_) => {
                    let reason = if task.phase.load(std::sync::atomic::Ordering::Acquire)
                        == SCANNER_TASK_FINALIZER
                    {
                        "terminal-snapshot-finalizer"
                    } else {
                        "terminal-snapshot-service"
                    };
                    result
                        .retained
                        .push(format!("reason={reason} state=retained"));
                    retained.insert(task_id, task);
                }
            }
        }

        result.terminal = retained.is_empty();
        if !retained.is_empty() {
            self.lock_state().tasks.extend(retained);
        }
        result
    }
}

#[cfg(test)]
mod tests;
