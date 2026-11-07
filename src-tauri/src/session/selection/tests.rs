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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
mod operation_turn;
mod payload;
mod response_registration;
mod restore;
mod shutdown_drain;
mod source_ownership;
mod timing;
mod user_switch;
mod worker;

#[cfg(test)]
pub(crate) struct SelectionPortTestState {
    pub(crate) cleanup_pty: Option<Arc<Mutex<PtyManager>>>,
    pub(crate) cleanup_backend_present: bool,
    pub(crate) receiver_present: bool,
    pub(crate) worker_present: bool,
    pub(crate) queue_capacity: usize,
    pub(crate) admission_available: usize,
    pub(crate) create_tickets_available: usize,
    pub(crate) expected_queue_capacity: usize,
    pub(crate) expected_admission_capacity: usize,
    pub(crate) expected_create_ticket_capacity: usize,
}

#[cfg(test)]
impl SelectionCoordinator {
    pub(crate) fn port_test_state(&self) -> SelectionPortTestState {
        SelectionPortTestState {
            cleanup_pty: self
                .inner
                .cleanup_pty
                .lock()
                .unwrap()
                .as_ref()
                .and_then(Weak::upgrade),
            cleanup_backend_present: self
                .inner
                .cleanup_container_backend
                .lock()
                .unwrap()
                .is_some(),
            receiver_present: self.inner.receiver.lock().unwrap().is_some(),
            worker_present: self.inner.worker.lock().unwrap().is_some(),
            queue_capacity: self.inner.sender.capacity(),
            admission_available: self.inner.admission.available_permits(),
            create_tickets_available: self.inner.create_tickets.available_permits(),
            expected_queue_capacity: COORDINATOR_QUEUE_CAPACITY,
            expected_admission_capacity: COORDINATOR_ADMISSION_CAPACITY,
            expected_create_ticket_capacity: CREATE_TICKET_CAPACITY,
        }
    }
}

#[cfg(test)]
impl<R: Runtime> SelectionTransaction<R> {
    pub(crate) fn port_test_binding_presence(&self) -> (bool, bool) {
        (self.lifecycle_entry.is_some(), self.restore_nonce.is_some())
    }

    pub(crate) fn port_test_executor(&self) -> Option<Arc<dyn LifecycleExecutor<Self>>> {
        self.lifecycle_executor()
    }
}

#[cfg(test)]
pub(crate) struct SelectionPortTestEntry {
    entry: Option<OwnedLifecycleEntry>,
    owner: Arc<replacement::ReplacementOwner>,
}

#[cfg(test)]
impl SelectionPortTestEntry {
    pub(crate) fn operation(&self) -> Uuid {
        self.owner.operation
    }

    pub(crate) fn nonce(&self) -> Uuid {
        self.entry.as_ref().unwrap().executor_nonce
    }

    pub(crate) fn sidecar_released(&self) -> bool {
        self.owner.state.lock().unwrap().sidecar_released
    }

    pub(crate) fn attach_lease(&mut self, lease: LifecycleLease) {
        let entry = self.entry.as_mut().unwrap();
        assert!(entry.authority.is_none());
        entry.authority = Some(lease);
    }

    pub(crate) fn lease(&self) -> &LifecycleLease {
        self.entry.as_ref().unwrap().authority.as_ref().unwrap()
    }

    pub(crate) fn enter<R: Runtime>(&self, t: &SelectionTransaction<R>) -> SelectionTransaction<R> {
        t.for_lifecycle_entry(self.entry.as_ref().unwrap())
    }

    pub(crate) fn observe_return(&self, result: &Result<SessionInfo, String>) {
        self.entry.as_ref().unwrap().observe_return(result);
    }

    pub(crate) async fn release<T>(&mut self, executor: &dyn LifecycleExecutor<T>) {
        self.entry.as_mut().unwrap().release_lease(executor).await;
    }

    pub(crate) fn drop_entry(&mut self) {
        drop(self.entry.take());
    }
}

#[cfg(test)]
pub(crate) enum SelectionPortTestExit {
    Drain,
    FailedEnqueue,
    ReceiverLoss,
}

#[cfg(test)]
impl SelectionCoordinator {
    pub(crate) fn port_test_active_restore_nonce(&self) -> Option<Uuid> {
        let retained = self.inner.retained_replacements.lock().unwrap();
        retained
            .restore_active
            .load(Ordering::Acquire)
            .then_some(retained.restore_nonce)
            .flatten()
    }
    pub(crate) fn port_test_retained_entry(&self, source: Uuid) -> Option<(Uuid, Uuid, String)> {
        self.inner
            .retained_replacements
            .lock()
            .unwrap()
            .entries
            .values()
            .find_map(|owner| {
                if !matches!(&owner.descriptor,
                replacement::OwnedReplacementDescriptor::EntryRestart(r) if r.session_id == source)
                {
                    return None;
                }
                let state = owner.state.lock().unwrap();
                Some((
                    owner.operation,
                    state.executor_nonce,
                    format!("{:?}", state.stage),
                ))
            })
    }
    pub(crate) fn port_test_running_unstarted(&self) {
        assert!(self.inner.worker.lock().unwrap().is_none());
        assert!(self.inner.receiver.lock().unwrap().is_some());
        self.inner
            .phase
            .compare_exchange(
                CoordinatorPhase::Bootstrapping as u8,
                CoordinatorPhase::Running as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .unwrap();
    }

    pub(crate) fn port_test_pending_entry(
        &self,
        request: RestartJobRequest,
    ) -> Result<SelectionPortTestEntry, String> {
        let (owner, executor_nonce) = Self::prepare_lifecycle_owner(
            replacement::OwnedReplacementDescriptor::EntryRestart(request),
        );
        self.insert_lifecycle_entry(&owner)?;
        owner.state.lock().unwrap().stage = ReplacementStage::Queued;
        Ok(SelectionPortTestEntry {
            entry: Some(OwnedLifecycleEntry {
                owner: owner.clone(),
                executor_nonce,
                authority: None,
            }),
            owner,
        })
    }

    pub(crate) async fn port_test_drive<R: Runtime>(
        &self,
        t: &SelectionTransaction<R>,
        mut entry: SelectionPortTestEntry,
        request: RestartJobRequest,
        exit: SelectionPortTestExit,
    ) -> SelectionPortTestEntry {
        if matches!(exit, SelectionPortTestExit::FailedEnqueue) {
            let mut receiver = self.inner.receiver.lock().unwrap().take().unwrap();
            receiver.close();
            drop(receiver);
            assert!(matches!(
                self.try_reserve_envelope(),
                Err(SelectionCoordinatorError::Unavailable)
            ));
            entry.drop_entry();
            return entry;
        }
        let (admission, slot) = self.try_reserve_envelope().unwrap();
        let (response, receiver) = oneshot::channel();
        let envelope = CoordinatorEnvelope {
            _admission: admission,
            _critical_admission: None,
            _create_ticket: None,
            job: CoordinatorJob::RestartLifecycle {
                entry: entry.entry.take().unwrap(),
                request,
                response,
            },
        };
        match exit {
            SelectionPortTestExit::Drain => {
                drop(slot.send(envelope));
                let mut queue = self.inner.receiver.lock().unwrap().take().unwrap();
                queue.close();
                SelectionTransaction::drain_after_shutdown(t, &mut queue).await;
                assert!(receiver.await.unwrap().is_err());
            }
            SelectionPortTestExit::ReceiverLoss => {
                drop(slot);
                drop(receiver);
                SelectionTransaction::execute_envelope(t, envelope).await;
            }
            SelectionPortTestExit::FailedEnqueue => unreachable!(),
        }
        entry
    }
}

#[cfg(test)]
impl<R: Runtime> SelectionTransaction<R> {
    pub(crate) fn port_test_restore_nonce(&self) -> Option<Uuid> {
        self.restore_nonce
    }
}

#[cfg(test)]
pub(crate) fn port_test_binding(session_id: Uuid) -> PendingCreateBinding {
    PendingCreateBinding::new(session_id, Uuid::new_v4())
}

#[cfg(test)]
impl LifecycleEffectGuard {
    pub(crate) fn port_test_identity(&self) -> (Uuid, Uuid) {
        (
            self.owner.operation,
            self.owner.state.lock().unwrap().executor_nonce,
        )
    }
}

#[test]
fn issue_3950_host_contract() {
    fn accepts_context<C: SelectionContext>(_: &C::Host) {}
    let sessions = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(
        LifecycleTestBackend::default(),
    ))));
    let detached = DetachedSessionsState::default();
    let settings = crate::config::settings::new_settings_state(Default::default());
    let response_close = crate::pty::manager::ResponseCloseState::default();
    let host = NeutralSelectionHost::new(
        Arc::clone(&sessions),
        Arc::clone(&pty),
        Arc::clone(&detached),
        Arc::clone(&settings),
        response_close,
    );
    accepts_context::<NeutralSelectionContext>(&host);
    assert!(std::ptr::eq(host.sessions_required(), &host.sessions));
    assert!(Arc::ptr_eq(host.sessions_required(), &sessions));
    assert!(std::ptr::eq(host.pty_required(), &host.pty));
    assert!(Arc::ptr_eq(host.pty_required(), &pty));
    assert!(std::ptr::eq(host.detached_required(), &host.detached));
    assert!(Arc::ptr_eq(host.detached_required(), &detached));
    assert!(std::ptr::eq(host.settings_required(), &host.settings));
    assert!(Arc::ptr_eq(host.settings_required(), &settings));
    assert!(std::ptr::eq(
        host.response_close_required(),
        &host.response_close
    ));
    assert!(host.pty_optional().is_none());
    assert!(host.response_close_optional().is_none());
    assert!(host.coordinator_optional().is_none());
    let mut present = host.clone();
    present.optional_present = true;
    present.coordinator = Some(SelectionCoordinator::new(
        sessions,
        CancellationToken::new(),
    ));
    assert!(std::ptr::eq(present.pty_optional().unwrap(), &present.pty));
    assert!(std::ptr::eq(
        present.response_close_optional().unwrap(),
        &present.response_close,
    ));
    assert!(std::ptr::eq(
        present.coordinator_optional().unwrap(),
        present.coordinator.as_ref().unwrap(),
    ));
    println!(
        "
ISSUE3950_CONTROL_OK case=issue_3950_host_contract"
    );
}
