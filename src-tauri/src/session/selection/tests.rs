use super::*;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};

fn response_close_unbound_ticket() -> super::CreateFinalizationTicket {
    super::CreateFinalizationTicket {
        owned_replacement: None,
        slot: None,
        admission: None,
        create_ticket: None,
        binding: None,
        cause: super::SelectionCause::SessionCreated(super::TrustedCreateIntent::Background),
        auto_select_precondition: None,
        completed: false,
    }
}

#[tokio::test]
async fn response_close_prepared_ticket_rejects_both_duplicate_bind_orders() {
    let mut prepared_first = response_close_unbound_ticket();
    let binding = super::PendingCreateBinding::new(uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    assert_eq!(prepared_first.bind_prepared(binding).unwrap(), binding);
    assert!(prepared_first.bind(uuid::Uuid::new_v4()).is_err());
    assert!(prepared_first.bind_prepared(binding).is_err());
    assert_eq!(prepared_first.binding(), Some(binding));
    // No manager row was created by this pure preparation fixture.
    prepared_first.completed = true;

    let mut default_first = response_close_unbound_ticket();
    let first = default_first.bind(uuid::Uuid::new_v4()).unwrap();
    assert!(default_first.bind_prepared(binding).is_err());
    assert_eq!(default_first.binding(), Some(first));
    default_first.completed = true;
    assert!(default_first.bind_prepared(binding).is_err());

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager.clone(), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(manager)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    coordinator.start(app.handle().clone()).unwrap();
    coordinator.submit_restore_first().await.unwrap().finish();
    coordinator.snapshot().await.unwrap();
    let mut waiting = coordinator
        .reserve_create(TrustedCreateIntent::Background)
        .await
        .unwrap();
    let original_selection = waiting.auto_select_precondition.clone();
    assert!(original_selection.is_some());
    assert!(waiting.release_capacity_for_wait());
    let mut occupied = Vec::new();
    for _ in 0..CREATE_TICKET_CAPACITY {
        occupied.push(coordinator.reserve_suppressed_create().await.unwrap());
    }
    assert!(
        matches!(
            waiting.resume_unbound_capacity(&coordinator),
            Err(SelectionCoordinatorError::Busy)
        ),
        "a real second probe reports actual saturation; it does not await capacity"
    );
    assert_eq!(waiting.auto_select_precondition, original_selection);
    drop(occupied);
    waiting.resume_unbound_capacity(&coordinator).unwrap();
    assert_eq!(
        waiting.auto_select_precondition, original_selection,
        "reacquisition preserves option1's original None-selection condition"
    );
    drop(waiting);
    coordinator.close_and_join().await;
}

async fn await_state_eq<T, F, D>(
    label: &str,
    budget: Duration,
    mut probe: F,
    expected: T,
    diagnose: D,
) -> T
where
    T: PartialEq + std::fmt::Debug,
    F: FnMut() -> T,
    D: FnOnce() -> String,
{
    let mut last = probe();
    if tokio::time::timeout(budget, async {
        while last != expected {
            tokio::time::sleep(Duration::from_millis(5)).await;
            last = probe();
        }
    })
    .await
    .is_err()
    {
        panic!(
            "{label}: expected {expected:?} within {budget:?}, last observed {last:?}; {}",
            diagnose()
        );
    }
    last
}

#[tokio::test]
#[should_panic(expected = "AC2-DIAGNOSE-MARKER")]
async fn await_state_eq_fails_when_the_state_never_arrives() {
    await_state_eq(
        "await_state_eq positive control",
        Duration::from_millis(50),
        || 0usize,
        1usize,
        || "AC2-DIAGNOSE-MARKER".to_string(),
    )
    .await;
}

#[tokio::test]
async fn await_state_eq_does_not_evaluate_diagnose_when_the_state_arrives() {
    let diagnose_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&diagnose_calls);
    await_state_eq(
        "await_state_eq lazy diagnose control",
        Duration::from_millis(50),
        || 7usize,
        7usize,
        || {
            calls.fetch_add(1, Ordering::SeqCst);
            String::new()
        },
    )
    .await;
    assert_eq!(diagnose_calls.load(Ordering::SeqCst), 0);
}

#[derive(Default)]
struct LifecycleTestBackend {
    live: Mutex<HashSet<Uuid>>,
    kills: Mutex<std::collections::HashMap<Uuid, usize>>,
}

impl LifecycleTestBackend {
    fn set_live(&self, session_id: Uuid, live: bool) {
        let mut sessions = self.live.lock().unwrap();
        if live {
            sessions.insert(session_id);
        } else {
            sessions.remove(&session_id);
        }
    }

    fn kill_count(&self, session_id: Uuid) -> usize {
        self.kills
            .lock()
            .unwrap()
            .get(&session_id)
            .copied()
            .unwrap_or_default()
    }
}

struct GatedStopRuntime {
    stop_started: Mutex<Option<oneshot::Sender<()>>>,
    stop_calls: AtomicUsize,
    stop_hold: Duration,
    active_stops: AtomicUsize,
    deadline_seen: AtomicBool,
}

struct GatedStartStopRuntime {
    start_started: Mutex<Option<oneshot::Sender<()>>>,
    stop_started: Mutex<Option<oneshot::Sender<()>>>,
    start_calls: AtomicUsize,
    stop_calls: AtomicUsize,
    stop_hold: Duration,
    active_starts: AtomicUsize,
    active_stops: AtomicUsize,
    deadline_seen: AtomicBool,
    stop_outcomes: Mutex<std::collections::VecDeque<CanceledStartStopOutcome>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanceledStartStopOutcome {
    Success,
    Error,
    Panic,
}

struct AtomicActivityGuard<'a> {
    active: &'a AtomicUsize,
}

impl<'a> AtomicActivityGuard<'a> {
    fn enter(active: &'a AtomicUsize) -> Self {
        active.fetch_add(1, Ordering::SeqCst);
        Self { active }
    }
}

impl Drop for AtomicActivityGuard<'_> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn hold_runtime_stop_until_budget(
    control: &crate::pty::container_runtime::ContainerRuntimeControl,
    requested_hold: Duration,
) {
    if requested_hold.is_zero() {
        return;
    }
    let _ = control.wait_for_shutdown();
    let remaining = control.remaining().unwrap_or(requested_hold);
    let hold = requested_hold.min(remaining.saturating_sub(Duration::from_millis(20)));
    if !hold.is_zero() {
        std::thread::sleep(hold);
    }
}

impl crate::pty::container_runtime::ContainerRuntime for GatedStopRuntime {
    fn start(
        &self,
        request: crate::pty::container_runtime::ContainerStartRequest,
        _control: &crate::pty::container_runtime::ContainerRuntimeControl,
    ) -> Result<crate::pty::container_runtime::ContainerRuntimeHandle, crate::errors::AppError>
    {
        Ok(crate::pty::container_runtime::ContainerRuntimeHandle {
            session_id: request.session_id,
            container_id: format!("container-{}", request.session_id),
        })
    }

    fn stop(
        &self,
        _handle: &crate::pty::container_runtime::ContainerRuntimeHandle,
        _timeout: Duration,
        control: &crate::pty::container_runtime::ContainerRuntimeControl,
    ) -> Result<(), crate::errors::AppError> {
        let _active = AtomicActivityGuard::enter(&self.active_stops);
        self.stop_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(started) = self
            .stop_started
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = started.send(());
        }
        hold_runtime_stop_until_budget(control, self.stop_hold);
        self.deadline_seen
            .store(control.shutdown_deadline().is_some(), Ordering::SeqCst);
        Ok(())
    }

    fn cleanup_labeled_orphans(
        &self,
        _live_sessions: &HashSet<Uuid>,
        _timeout: Duration,
    ) -> Result<crate::pty::container_runtime::ContainerCleanupReport, crate::errors::AppError>
    {
        Ok(crate::pty::container_runtime::ContainerCleanupReport::default())
    }
}

impl crate::pty::container_runtime::ContainerRuntime for GatedStartStopRuntime {
    fn start(
        &self,
        request: crate::pty::container_runtime::ContainerStartRequest,
        control: &crate::pty::container_runtime::ContainerRuntimeControl,
    ) -> Result<crate::pty::container_runtime::ContainerRuntimeHandle, crate::errors::AppError>
    {
        let _active = AtomicActivityGuard::enter(&self.active_starts);
        self.start_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(started) = self
            .start_started
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = started.send(());
        }
        let _ = control.wait_for_shutdown();
        Ok(crate::pty::container_runtime::ContainerRuntimeHandle {
            session_id: request.session_id,
            container_id: format!("container-{}", request.session_id),
        })
    }

    fn stop(
        &self,
        _handle: &crate::pty::container_runtime::ContainerRuntimeHandle,
        _timeout: Duration,
        control: &crate::pty::container_runtime::ContainerRuntimeControl,
    ) -> Result<(), crate::errors::AppError> {
        let _active = AtomicActivityGuard::enter(&self.active_stops);
        let call = self.stop_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(started) = self
            .stop_started
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = started.send(());
        }
        if call == 1 {
            hold_runtime_stop_until_budget(control, self.stop_hold);
        }
        self.deadline_seen
            .store(control.shutdown_deadline().is_some(), Ordering::SeqCst);
        match self
            .stop_outcomes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pop_front()
            .unwrap_or(CanceledStartStopOutcome::Success)
        {
            CanceledStartStopOutcome::Success => Ok(()),
            CanceledStartStopOutcome::Error => Err(crate::errors::AppError::Other(
                "injected canceled-start stop failure".to_string(),
            )),
            CanceledStartStopOutcome::Panic => {
                panic!("injected canceled-start stop panic")
            }
        }
    }

    fn cleanup_labeled_orphans(
        &self,
        _live_sessions: &HashSet<Uuid>,
        _timeout: Duration,
    ) -> Result<crate::pty::container_runtime::ContainerCleanupReport, crate::errors::AppError>
    {
        Ok(crate::pty::container_runtime::ContainerCleanupReport::default())
    }
}

fn real_container_spawn_spec(
    session_id: Uuid,
    cwd: String,
) -> crate::pty::backend::BackendSpawnSpec {
    crate::pty::backend::BackendSpawnSpec {
        id: session_id,
        agent_id: None,
        coding_agent: None,
        cmd: "container".to_string(),
        args: Vec::new(),
        resolved_agent_host_shell: None,
        cwd,
        selected_cwd: None,
        cols: 120,
        rows: 30,
        container_image: Some("agentscommander/test:latest".to_string()),
        configured_env: Vec::new(),
        env_remove_keys: Vec::new(),
        env_unset: Vec::new(),
        extra_env: Vec::new(),
        idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
        output_target: crate::pty::output::PtyOutputTarget::noop(),
        resource_registration: None,
        logical_resource_slot: None,
        container_credential: None,
        container_repo_mounts: Vec::new(),
        launch_witness: Default::default(),
    }
}

impl crate::pty::backend::PtyBackend for LifecycleTestBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            self.set_live(spec.id, true);
            Ok(())
        })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        _data: &[u8],
    ) -> Result<(), crate::errors::AppError> {
        self.live
            .lock()
            .unwrap()
            .contains(&id)
            .then_some(())
            .ok_or_else(|| crate::errors::AppError::SessionNotFound(id.to_string()))
    }

    fn resize(&self, id: Uuid, _cols: u16, _rows: u16) -> Result<(), crate::errors::AppError> {
        self.write(
            &crate::pty::manager::BackendWriteAuthority::for_backend_test(),
            id,
            &[],
        )
    }

    fn kill(&self, id: Uuid) -> Result<(), crate::errors::AppError> {
        *self.kills.lock().unwrap().entry(id).or_default() += 1;
        self.set_live(id, false);
        Ok(())
    }

    fn has_session(&self, id: Uuid) -> bool {
        self.live.lock().unwrap().contains(&id)
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, id: Uuid) -> Option<(u16, u16)> {
        self.has_session(id).then_some((30, 120))
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(
        &self,
        _session_id: Uuid,
        _request_id: String,
        _response_dir: PathBuf,
    ) {
    }

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }
}

mod admission;
mod blocking_seed;
mod container_shutdown;
mod critical_key;
mod executor_port;
mod operation_turn;
mod payload;
mod response_registration;
mod restore;
mod shutdown_drain;
mod source_ownership;
mod timing;
mod user_switch;
mod worker;
