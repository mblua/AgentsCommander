//! Shared native lifecycle fixtures and forwarding observer.

use crate::commands::session::selection_executor::NativeLifecycleLease;
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::selection::{
    DestroyOutcome, DestroyRequest, LifecycleAdmission, LifecycleEffectGuard,
    LifecycleEntryOutcome, LifecycleExecutor, LifecycleExecutorState, LifecycleFuture,
    LifecycleLease, OpaqueFixtureInput, OpaqueFixtureOutput, RestartJobRequest, RootJobRequest,
    SelectionCoordinator, SelectionTransaction, TrustedCreateIntent, TrustedRestartIntent,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use tauri::test::MockRuntime;
use tauri::Manager;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct PortTestBackend {
    spawn_pause: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<Uuid>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    live: Mutex<HashSet<Uuid>>,
    kills: Mutex<std::collections::HashMap<Uuid, usize>>,
}

impl PortTestBackend {
    pub(crate) fn gate_spawn(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<Uuid>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (reached, entered) = tokio::sync::oneshot::channel();
        let (resume, wait) = tokio::sync::oneshot::channel();
        assert!(self
            .spawn_pause
            .lock()
            .unwrap()
            .replace((reached, wait))
            .is_none());
        (entered, resume)
    }
    pub(crate) fn set_live(&self, session_id: Uuid, live: bool) {
        let mut sessions = self.live.lock().unwrap();
        if live {
            sessions.insert(session_id);
        } else {
            sessions.remove(&session_id);
        }
    }

    pub(crate) fn kill_count(&self, session_id: Uuid) -> usize {
        self.kills
            .lock()
            .unwrap()
            .get(&session_id)
            .copied()
            .unwrap_or_default()
    }
}

impl crate::pty::backend::PtyBackend for PortTestBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), crate::errors::AppError>> {
        Box::pin(async move {
            let gate = self.spawn_pause.lock().unwrap().take();
            if let Some((reached, resume)) = gate {
                let _ = reached.send(spec.id);
                let _ = resume.await;
            }
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

#[derive(Clone)]
pub(crate) struct NativeReceiptRecord {
    pub(crate) trace: crate::session::selection::SelectionPortTestTrace,
    pub(crate) operation: Uuid,
    pub(crate) source: Option<Uuid>,
    pub(crate) root: bool,
    pub(crate) receipt: Arc<AtomicU8>,
    pub(crate) initial: u8,
    pub(crate) claimed: bool,
    pub(crate) outcome: Option<(bool, bool)>,
}

struct NativePause {
    reached: tokio::sync::oneshot::Sender<(bool, bool, u8)>,
    resume: tokio::sync::oneshot::Receiver<()>,
}
#[derive(Default)]
pub(crate) struct NativeSchedule {
    before: Mutex<Option<NativePause>>,
    after: Mutex<Option<NativePause>>,
    root_outcomes: Mutex<Vec<(Uuid, Uuid, Result<String, String>)>>,
    traces:
        Mutex<std::collections::HashMap<Uuid, crate::session::selection::SelectionPortTestTrace>>,
    waiting: tokio::sync::Notify,
}
impl NativeSchedule {
    fn trace(&self, operation: Uuid) -> crate::session::selection::SelectionPortTestTrace {
        self.traces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(operation)
            .or_insert_with(|| Arc::new(Mutex::new(Vec::new())))
            .clone()
    }
    pub(crate) async fn waiting(&self) {
        self.waiting.notified().await;
    }
    pub(crate) fn root_outcomes(&self) -> Vec<(Uuid, Uuid, Result<String, String>)> {
        self.root_outcomes.lock().unwrap().clone()
    }
    pub(crate) fn gate(
        &self,
        after: bool,
    ) -> (
        tokio::sync::oneshot::Receiver<(bool, bool, u8)>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (reached, observation) = tokio::sync::oneshot::channel();
        let (resume, wait) = tokio::sync::oneshot::channel();
        let gate = NativePause {
            reached,
            resume: wait,
        };
        let slot = if after { &self.after } else { &self.before };
        assert!(slot.lock().unwrap().replace(gate).is_none());
        (observation, resume)
    }
}
struct NativeReceiptObserver {
    original: Arc<dyn LifecycleExecutor<SelectionTransaction<MockRuntime>>>,
    records: Arc<Mutex<Vec<NativeReceiptRecord>>>,
    schedule: Arc<NativeSchedule>,
}

impl NativeReceiptObserver {
    async fn pause(
        &self,
        after: bool,
        t: &SelectionTransaction<MockRuntime>,
        lease: &LifecycleLease,
    ) {
        let slot = if after {
            &self.schedule.after
        } else {
            &self.schedule.before
        };
        let gate = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(gate) = gate {
            let receipt = lease
                .downcast_ref::<NativeLifecycleLease>()
                .map(|n| n.release_receipt.load(Ordering::Acquire))
                .unwrap_or(255);
            let _ = gate.reached.send((
                t.has_accepted_lifecycle_core(),
                self.original.has_claim(lease),
                receipt,
            ));
            let _ = gate.resume.await;
        }
    }
    fn observe(&self, lease: &LifecycleLease, outcome: &LifecycleEntryOutcome) {
        let Some(native) = lease.downcast_ref::<NativeLifecycleLease>() else {
            return;
        };
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(record) = records
            .iter_mut()
            .find(|record| Arc::ptr_eq(&record.receipt, &native.release_receipt))
        {
            record.outcome = Some(match outcome {
                LifecycleEntryOutcome::Interrupted => (false, false),
                LifecycleEntryOutcome::Returned(result) => (true, result.is_ok()),
            });
        }
    }
}

impl LifecycleExecutor<SelectionTransaction<MockRuntime>> for NativeReceiptObserver {
    fn prepare<'a>(
        &'a self,
        t: &'a SelectionTransaction<MockRuntime>,
        operation: Uuid,
        source: Option<Uuid>,
        root: bool,
        directory: Option<&'a str>,
    ) -> LifecycleFuture<'a, Result<LifecycleAdmission, String>> {
        Box::pin(async move {
            let mut admission = self
                .original
                .prepare(t, operation, source, root, directory)
                .await?;
            if let LifecycleAdmission::Ready(lease) = &mut admission {
                if let Some(native) = lease.downcast_ref::<NativeLifecycleLease>() {
                    let record = NativeReceiptRecord {
                        operation,
                        source,
                        root,
                        receipt: native.release_receipt.clone(),
                        initial: native.release_receipt.load(Ordering::Acquire),
                        claimed: native
                            .authority
                            .as_ref()
                            .is_some_and(|a| a.has_owned_claim()),
                        outcome: None,
                        trace: self.schedule.trace(operation),
                    };
                    lease.port_test_attach_trace(record.trace.clone());
                    self.records
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(record);
                }
            }
            if matches!(admission, LifecycleAdmission::Waiting) {
                self.schedule.waiting.notify_one();
            }
            Ok(admission)
        })
    }

    fn validate<'a>(
        &'a self,
        t: &'a SelectionTransaction<MockRuntime>,
        lease: &'a mut LifecycleLease,
    ) -> LifecycleFuture<'a, bool> {
        self.original.validate(t, lease)
    }

    fn has_claim(&self, lease: &LifecycleLease) -> bool {
        self.original.has_claim(lease)
    }

    fn release(&self, lease: LifecycleLease) -> bool {
        self.original.release(lease)
    }

    fn ensure_root_directory(&self, effect: LifecycleEffectGuard) -> Result<String, String> {
        let identity = effect.port_test_identity();
        let result = self.original.ensure_root_directory(effect);
        self.schedule
            .trace(identity.0)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((
                identity.0,
                identity.1,
                crate::session::selection::SelectionPortTestEvent::RootDirectoryComplete(
                    result.is_ok(),
                ),
            ));
        self.schedule
            .root_outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((identity.0, identity.1, result.clone()));
        result
    }

    fn destroy<'a>(
        &'a self,
        t: &'a SelectionTransaction<MockRuntime>,
        request: DestroyRequest,
    ) -> LifecycleFuture<'a, Result<DestroyOutcome, String>> {
        self.original.destroy(t, request)
    }

    fn restart<'a>(
        &'a self,
        t: &'a SelectionTransaction<MockRuntime>,
        request: RestartJobRequest,
        lease: &'a LifecycleLease,
    ) -> LifecycleFuture<'a, LifecycleEntryOutcome> {
        Box::pin(async move {
            self.pause(false, t, lease).await;
            let outcome = self.original.restart(t, request, lease).await;
            self.observe(lease, &outcome);
            self.pause(true, t, lease).await;
            outcome
        })
    }

    fn root<'a>(
        &'a self,
        t: &'a SelectionTransaction<MockRuntime>,
        request: RootJobRequest,
        lease: &'a LifecycleLease,
    ) -> LifecycleFuture<'a, LifecycleEntryOutcome> {
        Box::pin(async move {
            self.pause(false, t, lease).await;
            let outcome = self.original.root(t, request, lease).await;
            self.observe(lease, &outcome);
            self.pause(true, t, lease).await;
            outcome
        })
    }

    fn conditional_close<'a>(
        &'a self,
        t: &'a SelectionTransaction<MockRuntime>,
        ticket: crate::pty::manager::ResponseCloseTicket,
    ) -> LifecycleFuture<'a, Result<(), String>> {
        self.original.conditional_close(t, ticket)
    }

    fn reconcile<'a>(
        &'a self,
        t: &'a SelectionTransaction<MockRuntime>,
    ) -> LifecycleFuture<'a, ()> {
        self.original.reconcile(t)
    }

    fn complete_retirement<'a>(
        &'a self,
        t: &'a SelectionTransaction<MockRuntime>,
        id: Uuid,
    ) -> LifecycleFuture<'a, ()> {
        self.original.complete_retirement(t, id)
    }

    fn execute_fixture<'a>(
        &'a self,
        t: &'a SelectionTransaction<MockRuntime>,
        input: OpaqueFixtureInput,
    ) -> LifecycleFuture<'a, OpaqueFixtureOutput> {
        self.original.execute_fixture(t, input)
    }
}

pub(crate) struct NativeFixture {
    pub(crate) app: tauri::App<MockRuntime>,
    pub(crate) coordinator: SelectionCoordinator,
    pub(crate) manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(crate) pty: Arc<Mutex<PtyManager>>,
    pub(crate) source: Uuid,
    pub(crate) cwd: tempfile::TempDir,
    records: Arc<Mutex<Vec<NativeReceiptRecord>>>,
    pub(crate) schedule: Arc<NativeSchedule>,
    pub(crate) backend: Arc<PortTestBackend>,
}

impl NativeFixture {
    pub(crate) fn receipt_records(&self) -> Vec<NativeReceiptRecord> {
        self.records.lock().unwrap().clone()
    }
    pub(crate) async fn new() -> Self {
        Self::build(true).await
    }
    pub(crate) async fn build(archived: bool) -> Self {
        let marker = std::env::var("AC_P17_CARRIER_CHILD").unwrap();
        let identity = std::env::var("AC_P17_CARRIER_IDENTITY").unwrap();
        let (id, name) = identity.split_once('\n').unwrap();
        assert_eq!(name, marker);
        assert!(!Uuid::parse_str(id).unwrap().is_nil());
        let configured = std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR")
            .expect("native fixtures require designated test configuration");
        assert_eq!(
            crate::config::config_dir().unwrap(),
            PathBuf::from(configured)
        );
        let cwd = tempfile::tempdir().unwrap();
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let source = if archived {
            manager
                .read()
                .await
                .create_session(
                    "shell".into(),
                    Vec::new(),
                    cwd.path().to_string_lossy().into_owned(),
                    None,
                    None,
                    Vec::new(),
                    false,
                    crate::pty::backend::SessionBackendKind::LocalProcess,
                )
                .await
                .unwrap()
                .id
        } else {
            Uuid::nil()
        };
        let backend = Arc::new(PortTestBackend::default());
        let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
        let coordinator = SelectionCoordinator::new(manager.clone(), CancellationToken::new());
        let settings = crate::config::settings::new_settings_state(
            crate::config::settings::AppSettings::default(),
        );
        settings.write().await.agents.clear();
        settings.write().await.project_paths = vec![cwd.path().to_string_lossy().into_owned()];
        if archived {
            settings.write().await.archived_project_paths =
                vec![cwd.path().to_string_lossy().into_owned()];
        }
        let schedule = Arc::new(NativeSchedule::default());
        let records = Arc::new(Mutex::new(Vec::new()));
        let original = crate::commands::session::selection_executor::state::<MockRuntime>()
            .get()
            .clone();
        let observer: Arc<dyn LifecycleExecutor<SelectionTransaction<MockRuntime>>> =
            Arc::new(NativeReceiptObserver {
                original,
                records: records.clone(),
                schedule: schedule.clone(),
            });
        let captures = Arc::new(crate::capture::registry::CaptureRegistry::new());
        let telegram: crate::telegram::manager::TelegramBridgeState =
            Arc::new(tokio::sync::Mutex::new(
                crate::telegram::manager::TelegramBridgeManager::with_captures(
                    Arc::new(Mutex::new(std::collections::HashMap::new())),
                    captures.clone(),
                ),
            ));
        let store_dir = tempfile::tempdir().unwrap();
        let store_root = crate::path_utils::normalize_windows_verbatim_path_buf(
            &std::fs::canonicalize(store_dir.path()).unwrap(),
        );
        let gate = crate::api::message_store::PtyInputTargetGateState::for_root(store_root.clone());
        let store = Arc::new(
            crate::api::message_store::MessageStore::open(
                store_root.join(crate::api::message_store::DB_FILENAME),
            )
            .unwrap(),
        );
        let app = tauri::test::mock_builder()
            .manage(manager.clone())
            .manage(pty.clone())
            .manage(coordinator.clone())
            .manage(settings)
            .manage(Arc::new(
                crate::resource_monitor::ResourceMonitorState::new(),
            ))
            .manage(crate::DetachedSessionsState::default())
            .manage(telegram)
            .manage(captures)
            .manage(crate::network::OutboundNetwork::new_for_tests(1))
            .manage(crate::session::warnings::new_session_warning_state())
            .manage(Arc::new(crate::pty::menu_guard::MenuGuard::new()))
            .manage(gate.clone())
            .manage(
                crate::api::message_store::MessageStoreState::with_store_and_target_gate(
                    Ok(store),
                    gate.gate.clone(),
                ),
            )
            .manage(store_dir)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        assert!(app.manage(LifecycleExecutorState::new(observer.clone())));
        assert!(Arc::ptr_eq(
            app.state::<LifecycleExecutorState<SelectionTransaction<MockRuntime>>>()
                .get(),
            &observer,
        ));
        Self {
            app,
            coordinator,
            manager,
            pty,
            source,
            cwd,
            records,
            schedule,
            backend,
        }
    }

    pub(crate) async fn running() -> Self {
        let mut f = Self::build(false).await;
        f.coordinator.start(f.app.handle().clone()).unwrap();
        f.coordinator.submit_restore_first().await.unwrap().finish();
        f.coordinator.snapshot().await.unwrap();
        f.source = f.create_source().await;
        f
    }
    pub(crate) async fn restore_source(
        &self,
        transaction: &SelectionTransaction<MockRuntime>,
    ) -> Uuid {
        let info = crate::commands::session::create_session_inner_for_restore(
            transaction,
            &self.manager,
            &self.pty,
            "test-shell".into(),
            Vec::new(),
            self.cwd.path().to_string_lossy().into_owned(),
            Some("restore source".into()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        Uuid::parse_str(&info.id).unwrap()
    }
    pub(crate) async fn create_source(&self) -> Uuid {
        self.create_source_at(self.cwd.path()).await
    }
    pub(crate) async fn create_source_at(&self, cwd: &std::path::Path) -> Uuid {
        let info = crate::commands::session::create_session_inner(
            self.app.handle(),
            &self.manager,
            &self.pty,
            "test-shell".into(),
            Vec::new(),
            cwd.to_string_lossy().into_owned(),
            Some("native port source".into()),
            None,
            None,
            true,
            Vec::new(),
            true,
            None,
            None,
            None,
            crate::commands::session::CreateSelectionIntent::User,
        )
        .await
        .unwrap();
        Uuid::parse_str(&info.id).unwrap()
    }
    pub(crate) fn executor(&self) -> Arc<dyn LifecycleExecutor<SelectionTransaction<MockRuntime>>> {
        self.app
            .state::<LifecycleExecutorState<SelectionTransaction<MockRuntime>>>()
            .get()
            .clone()
    }

    pub(crate) fn transaction(&self) -> SelectionTransaction<MockRuntime> {
        self.coordinator
            .port_test_transaction(self.app.handle().clone())
            .with_lifecycle_executor(self.executor())
    }

    pub(crate) fn request(&self) -> RestartJobRequest {
        RestartJobRequest {
            session_id: self.source,
            ..restart_request()
        }
    }

    pub(crate) async fn prepare(&self, operation: Uuid) -> LifecycleLease {
        let t = self.transaction();
        let LifecycleAdmission::Ready(lease) = self
            .executor()
            .prepare(&t, operation, Some(self.source), false, None)
            .await
            .unwrap()
        else {
            panic!("real original target claim required")
        };
        let records = self.records.lock().unwrap();
        let matched: Vec<_> = records
            .iter()
            .filter(|r| r.operation == operation)
            .collect();
        assert_eq!(matched.len(), 1);
        let record = matched[0];
        assert_eq!(
            (record.source, record.root, record.initial),
            (Some(self.source), false, 0)
        );
        assert!(record.claimed);
        assert!(Arc::ptr_eq(&record.receipt, &native_receipt(&lease)));
        lease
    }
}

pub(crate) fn native_receipt(lease: &LifecycleLease) -> Arc<AtomicU8> {
    lease
        .downcast_ref::<NativeLifecycleLease>()
        .unwrap()
        .release_receipt
        .clone()
}

pub(crate) fn restart_request() -> RestartJobRequest {
    RestartJobRequest {
        session_id: Uuid::new_v4(),
        agent_id: None,
        requested_profile: None,
        skip_auto_resume: None,
        activate_after: false,
        intent: TrustedRestartIntent::User,
        communication_override: None,
        enforcement: crate::config::sessions_persistence::CreationGateEnforcement::Enforce,
    }
}

pub(crate) fn root_request() -> RootJobRequest {
    RootJobRequest {
        requested_agent_id: None,
        requested_profile: None,
        skip_auto_resume_for_new_session: false,
        intent: TrustedCreateIntent::User,
        select_after: false,
    }
}
