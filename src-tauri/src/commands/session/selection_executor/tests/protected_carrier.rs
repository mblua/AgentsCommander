//! Protected carrier provisioning and retirement fixture.

use super::native_fixture::NativeFixture;
use std::sync::Arc;
use tauri::Manager;
use uuid::Uuid;

pub(crate) struct ProtectedFixture {
    pub(crate) native: NativeFixture,
    pub(crate) route: crate::config::teams::VerifiedPtyInputRoute,
    pub(crate) carrier: crate::config::sessions_persistence::PublishedCarrier,
    pub(crate) sender: Uuid,
    pub(crate) parent_source: Option<Uuid>,
}
impl ProtectedFixture {
    pub(crate) async fn new(with_parent: bool) -> Self {
        let name = std::env::var("AC_P17_CARRIER_CHILD").unwrap();
        isolated_identity(&name);
        let mut f = NativeFixture::build(false).await;
        let tracker = f.pty.lock().unwrap().response_close_state();
        *f.manager.write().await = crate::session::manager::SessionManager::with_response_close(
            tracker.clone(),
            f.app
                .state::<crate::app_state::AppOwners>()
                .activity
                .clone(),
        );
        assert!(f.app.manage(tracker));
        let project = f.cwd.path().join("proj-a");
        let ac = project.join(".ac");
        let room = ac.join("wg-1-dev-team");
        let sender = room.join("__agent_tech-lead");
        let target = room.join("__agent_dev-rust");
        let team = ac.join("_team_dev-team");
        for path in [
            &sender,
            &target,
            &team,
            &ac.join("_agent_tech-lead"),
            &ac.join("_agent_dev-rust"),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        let config = serde_json::json!({
            "agents": ["../_agent_dev-rust", "../_agent_tech-lead"],
            "coordinator": "../_agent_tech-lead"
        });
        std::fs::write(
            team.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        for (path, origin) in [(&sender, "tech-lead"), (&target, "dev-rust")] {
            let config = serde_json::json!({ "identity": format!("../../_agent_{origin}") });
            std::fs::write(
                path.join("config.json"),
                serde_json::to_vec(&config).unwrap(),
            )
            .unwrap();
        }
        f.coordinator.start(f.app.handle().clone()).unwrap();
        f.coordinator.submit_restore_first().await.unwrap().finish();
        f.coordinator.snapshot().await.unwrap();
        let sender_id = f.create_source_at(&sender).await;
        f.source = f.create_source_at(&target).await;
        let parent_source = if with_parent {
            Some(
                tokio::time::timeout(
                    std::time::Duration::from_secs(20),
                    f.create_source_at(target.parent().unwrap()),
                )
                .await
                .expect("unknown parent source creation must complete"),
            )
        } else {
            None
        };
        let project_paths = vec![f.cwd.path().to_string_lossy().into_owned()];
        let route = crate::config::teams::verify_pty_input_route(
            &sender,
            false,
            "proj-a:wg-1-dev-team/dev-rust",
            &project_paths,
        )
        .unwrap();
        let manager = f.manager.read().await.clone();
        manager
            .rename_session(sender_id, "coordinator".into())
            .await
            .unwrap();
        manager
            .rename_session(f.source, "member".into())
            .await
            .unwrap();
        manager.set_is_coordinator(sender_id, true).await;
        manager.mark_idle(f.source).await;
        manager
            .set_agent_kind(
                f.source,
                Some(crate::session::profile::CodingAgentKind::Codex),
            )
            .await;
        let settings = f
            .app
            .state::<crate::config::settings::SettingsState>()
            .read()
            .await
            .clone();
        crate::config::settings::save_settings_with_project_paths(&settings).unwrap();
        let written = crate::config::settings::save_settings(&settings).unwrap();
        *f.app
            .state::<crate::config::settings::SettingsState>()
            .write()
            .await = written;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let (token, version) = manager
            .with_current_carrier_snapshot(&permit, |current| {
                let row = current.rows.iter().find(|row| row.id == f.source).unwrap();
                (row.token, current.versions[&row.id])
            })
            .await;
        drop(permit);
        let carrier = crate::config::sessions_persistence::persist_initial_carrier(
            &manager,
            crate::config::config_dir().unwrap(),
            project_paths,
            crate::config::sessions_persistence::InitialCarrierIntent::new(
                route.target.canonical_fqn.clone(),
                f.source,
                token,
                version,
                route.target.replica_identity.canonical_path.clone(),
            ),
        )
        .await
        .unwrap()
        .expect("actual initial carrier");
        Self {
            native: f,
            route,
            carrier,
            sender: sender_id,
            parent_source,
        }
    }
}

impl ProtectedFixture {
    pub(crate) async fn retire(&self) {
        use crate::pty::manager::{ResponseClosePublisher as P, ResponseRouteIdentity};
        use std::time::Duration;
        let manager = self.native.manager.read().await.clone();
        let tracker = self.native.pty.lock().unwrap().response_close_state();
        let previous = self.native.source;
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        let (member, leader) = manager
            .with_current_carrier_snapshot(&permit, |current| {
                let identity = |id, target| {
                    let row = current.rows.iter().find(|row| row.id == id).unwrap();
                    ResponseRouteIdentity {
                        target,
                        session_id: id,
                        token: row.token,
                        generation: tracker.generation(id).unwrap(),
                        carrier_version: current.versions[&id],
                        backend: row.backend_kind,
                    }
                };
                (
                    identity(previous, self.route.target.canonical_fqn.clone()),
                    identity(self.sender, self.route.sender.canonical_fqn.clone()),
                )
            })
            .await;
        drop(permit);
        let key = format!("prepare:{}", Uuid::new_v4());
        let mut admission = tracker.reserve_work(&member.target, &key).unwrap();
        admission.accept_durable("prepare-body").unwrap();
        drop(admission);
        let past = std::time::Instant::now() - Duration::from_secs(120);
        for publisher in [
            P::Manager,
            P::Pty,
            P::RawOutput,
            P::Detector,
            P::Menu,
            P::Hold,
        ] {
            tracker.bind_publisher(publisher);
        }
        tracker.mark_work_bootstrapped();
        tracker.try_enable_readiness().unwrap();
        tracker.apply_settings(true, 30, past);
        // Re-publish the real route identities on the fake Instant timeline
        // before admitting this test's first policy. No retirement is forged.
        for (identity, eligible) in [(&member, true), (&leader, false)] {
            tracker.register_route(identity.session_id, identity.generation, past);
            tracker.publish_manager_row(
                identity.session_id,
                identity.token,
                identity.generation,
                identity.carrier_version,
                eligible,
                false,
                past,
            );
            tracker.publish_manager_facts(identity.clone(), eligible, false, past);
        }
        tracker.observe_detector(previous, false, past);
        tracker.publish_menu(previous, Some(false), past);
        tracker.publish_hold(previous, None, true, past);
        let context = tracker
            .reserve_policy_candidate(
                member.clone(),
                leader.clone(),
                key,
                "prepare-body".into(),
                past,
            )
            .unwrap();
        tracker.mark_request_submitted(&context, true);
        let permit = crate::session::manager::acquire_carrier_mutation_permit().await;
        manager
            .with_current_carrier_snapshot(&permit, |current| {
                crate::config::sessions_persistence::register_published_carrier(
                    &permit,
                    &self.carrier,
                    current,
                    |_| {
                        tracker
                            .register_request(
                                crate::pty::manager::ResponseDeliveryProof {
                                    context,
                                    member: member.clone(),
                                    coordinator: leader.clone(),
                                    published_carrier_version: self.carrier.version,
                                },
                                past,
                            )
                            .ok()
                    },
                )
            })
            .await
            .expect("real current published request registration");
        drop(permit);
        let context = tracker
            .admit_return(
                &member,
                &leader,
                format!("prepare-return-{}", Uuid::new_v4()),
                "prepare-return-body".into(),
                past,
            )
            .unwrap();
        tracker
            .observe_return_success(crate::pty::manager::ReturnDeliveryProof { context }, past)
            .unwrap();
        assert!(
            crate::session::selection::remember_response_close_registration(
                crate::session::selection::ResponseCloseRegistration {
                    diagnostic_last_poll_outcome: Arc::new(std::sync::atomic::AtomicU64::new(0)),
                    tracker: tracker.clone(),
                    member,
                    coordinator: leader,
                    carrier: self.carrier.clone()
                },
            )
        );
        crate::commands::session::selection_executor::poll_response_close(self.native.app.handle())
            .await;
        self.native.coordinator.snapshot().await.unwrap();
    }
}

fn isolated_identity(name: &str) -> (String, std::path::PathBuf) {
    assert_eq!(std::env::var("AC_P17_CARRIER_CHILD").unwrap(), name);
    let identity = std::env::var("AC_P17_CARRIER_IDENTITY").unwrap();
    let (id, exact) = identity.split_once('\n').unwrap();
    assert!(!Uuid::parse_str(id).unwrap().is_nil());
    assert_eq!(exact, name);
    let config =
        std::path::PathBuf::from(std::env::var("AGENTSCOMMANDER_TEST_CONFIG_DIR").unwrap());
    assert!(config.is_absolute());
    assert_eq!(
        std::path::PathBuf::from(std::env::var("AGENTSCOMMANDER_CONFIG_DIR").unwrap()),
        config
    );
    assert_eq!(crate::config::config_dir().unwrap(), config);
    (identity, config.parent().unwrap().join("completion.txt"))
}

pub(crate) fn complete_isolated(name: &str) {
    let (identity, receipt) = isolated_identity(name);
    assert!(!receipt.exists());
    std::fs::write(receipt, identity).unwrap();
}

struct CarrierChild {
    child: std::process::Child,
    directory: Option<tempfile::TempDir>,
    reaped: bool,
}

impl CarrierChild {
    fn retain_failure(&mut self, reason: &str) -> ! {
        let directory = self.directory.take().unwrap().keep();
        let stdout = std::fs::read_to_string(directory.join("stdout.txt")).unwrap_or_default();
        let stderr = std::fs::read_to_string(directory.join("stderr.txt")).unwrap_or_default();
        panic!(
            "{reason}; evidence={}\n{stdout}\n{stderr}",
            directory.display()
        );
    }
}

impl Drop for CarrierChild {
    fn drop(&mut self) {
        if !self.reaped {
            if let Err(error) = self.child.kill() {
                eprintln!("P17 child kill: {error}");
            }
            if let Err(error) = self.child.wait() {
                eprintln!("P17 child wait: {error}");
            }
        }
    }
}

pub(crate) async fn run_isolated(name: &str, body: impl std::future::Future<Output = ()>) {
    if let Ok(marker) = std::env::var("AC_P17_CARRIER_CHILD") {
        assert_eq!(marker, name);
        let (_, receipt) = isolated_identity(name);
        assert!(!receipt.exists());
        body.await;
        return;
    }
    assert!(std::env::var("AC_P17_CARRIER_IDENTITY").is_err());
    drop(body);
    let directory = tempfile::Builder::new()
        .prefix("p17-carrier-")
        .tempdir()
        .unwrap();
    let config = directory.path().join("config");
    let temporary = directory.path().join("tmp");
    std::fs::create_dir(&config).unwrap();
    std::fs::create_dir(&temporary).unwrap();
    let identity = format!("{}\n{name}", Uuid::new_v4());
    std::fs::write(directory.path().join("identity.txt"), &identity).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([name, "--exact", "--nocapture", "--test-threads=1"])
        .current_dir(std::env::current_dir().unwrap())
        .stdin(std::process::Stdio::null())
        .stdout(std::fs::File::create(directory.path().join("stdout.txt")).unwrap())
        .stderr(std::fs::File::create(directory.path().join("stderr.txt")).unwrap())
        .env("AGENTSCOMMANDER_CONFIG_DIR", &config)
        .env("AGENTSCOMMANDER_TEST_CONFIG_DIR", &config)
        .env("TEMP", &temporary)
        .env("TMP", &temporary)
        .env("TMPDIR", &temporary)
        .env("AC_P17_CARRIER_CHILD", name)
        .env("AC_P17_CARRIER_IDENTITY", &identity);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut owner = CarrierChild {
        child: command.spawn().unwrap(),
        directory: Some(directory),
        reaped: false,
    };
    let started = std::time::Instant::now();
    let status = loop {
        match owner.child.try_wait() {
            Ok(Some(status)) => {
                owner.reaped = true;
                break status;
            }
            Ok(None) if started.elapsed() < std::time::Duration::from_secs(120) => {
                tokio::task::yield_now().await;
            }
            Ok(None) => {
                let kill = owner.child.kill();
                let wait = owner.child.wait();
                owner.reaped = wait.is_ok();
                owner.retain_failure(&format!("child deadline; kill={kill:?}; wait={wait:?}"));
            }
            Err(error) => owner.retain_failure(&format!("child poll: {error}")),
        }
    };
    let directory = owner.directory.as_ref().unwrap().path();
    std::fs::write(
        directory.join("exit.txt"),
        format!("{status}; elapsed={:?}", started.elapsed()),
    )
    .unwrap();
    let receipt = std::fs::read_to_string(directory.join("completion.txt"));
    if !status.success() || !receipt.is_ok_and(|value| value == identity) {
        owner.retain_failure("child exit or completion receipt failed");
    }
    eprintln!(
        "P17 child {name}: config={} receipt={identity:?} exit={status} pid={} elapsed={:?}",
        config.display(),
        owner.child.id(),
        started.elapsed()
    );
}

use crate::session::selection::{
    DestroyOutcome, DestroyRequest, LifecycleAdmission, LifecycleEffectGuard,
    LifecycleEntryOutcome, LifecycleExecutor, LifecycleExecutorState, LifecycleFuture,
    LifecycleLease, OpaqueFixtureInput, OpaqueFixtureOutput, RestartJobRequest, RootJobRequest,
    SelectionCoordinator, SelectionTransaction,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::test::MockRuntime;
use tokio::sync::{oneshot, Notify};

type PollTransaction = SelectionTransaction<MockRuntime>;
type PollPort = Arc<dyn LifecycleExecutor<PollTransaction>>;
type PollLog = Arc<Mutex<Vec<String>>>;

struct PollLogger(PollLog);
impl log::Log for PollLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == "agentscommander_lib::response_close_diag"
            && metadata.level() <= log::Level::Debug
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.0.lock().unwrap().push(record.args().to_string());
        }
    }
    fn flush(&self) {}
}

struct PollExecutor {
    original: PollPort,
    log: PollLog,
    entered: Notify,
    resume: Mutex<Option<oneshot::Receiver<()>>>,
    started: Mutex<Option<Instant>>,
    coordinator: bool,
}
impl LifecycleExecutor<PollTransaction> for PollExecutor {
    fn prepare<'a>(
        &'a self,
        t: &'a PollTransaction,
        operation: Uuid,
        source: Option<Uuid>,
        root: bool,
        directory: Option<&'a str>,
    ) -> LifecycleFuture<'a, Result<LifecycleAdmission, String>> {
        self.original.prepare(t, operation, source, root, directory)
    }
    fn validate<'a>(
        &'a self,
        t: &'a PollTransaction,
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
        self.original.ensure_root_directory(effect)
    }
    fn destroy<'a>(
        &'a self,
        t: &'a PollTransaction,
        request: DestroyRequest,
    ) -> LifecycleFuture<'a, Result<DestroyOutcome, String>> {
        self.original.destroy(t, request)
    }
    fn restart<'a>(
        &'a self,
        t: &'a PollTransaction,
        request: RestartJobRequest,
        lease: &'a LifecycleLease,
    ) -> LifecycleFuture<'a, LifecycleEntryOutcome> {
        self.original.restart(t, request, lease)
    }
    fn root<'a>(
        &'a self,
        t: &'a PollTransaction,
        request: RootJobRequest,
        lease: &'a LifecycleLease,
    ) -> LifecycleFuture<'a, LifecycleEntryOutcome> {
        self.original.root(t, request, lease)
    }
    fn conditional_close<'a>(
        &'a self,
        t: &'a PollTransaction,
        ticket: crate::pty::manager::ResponseCloseTicket,
    ) -> LifecycleFuture<'a, Result<(), String>> {
        self.original.conditional_close(t, ticket)
    }
    fn complete_retirement<'a>(
        &'a self,
        t: &'a PollTransaction,
        id: Uuid,
    ) -> LifecycleFuture<'a, ()> {
        self.original.complete_retirement(t, id)
    }
    fn execute_fixture<'a>(
        &'a self,
        t: &'a PollTransaction,
        input: OpaqueFixtureInput,
    ) -> LifecycleFuture<'a, OpaqueFixtureOutput> {
        self.original.execute_fixture(t, input)
    }
    fn reconcile<'a>(&'a self, t: &'a PollTransaction) -> LifecycleFuture<'a, ()> {
        Box::pin(async move {
            assert_eq!(
                t.app().try_state::<SelectionCoordinator>().is_some(),
                self.coordinator
            );
            assert_eq!(t.port_test_binding_presence(), (false, false));
            let managed = t.app().state::<LifecycleExecutorState<PollTransaction>>();
            assert!(Arc::ptr_eq(&t.port_test_executor().unwrap(), managed.get()));
            if let Some(started) = *self.started.lock().unwrap() {
                assert!(
                    started.elapsed() >= Duration::from_secs(1),
                    "first poll elapsed < 1s"
                );
            }
            self.log.lock().unwrap().push("entry".into());
            self.entered.notify_one();
            let resume = self.resume.lock().unwrap().take();
            if let Some(resume) = resume {
                resume.await.unwrap();
            }
            self.original.reconcile(t).await;
            self.log.lock().unwrap().push("completed".into());
        })
    }
}

struct PollView {
    app: tauri::App<MockRuntime>,
    observer: Arc<PollExecutor>,
    outcome: Arc<AtomicU64>,
    sentinel: Uuid,
    log: PollLog,
}
impl PollView {
    fn new(f: &ProtectedFixture, log: PollLog, tracker: bool, bound: bool, coord: bool) -> Self {
        use crate::pty::manager::{PtyManager, ResponseRouteIdentity};
        let primary = PtyManager::new_for_test(f.native.backend.clone()).response_close_state();
        let native = f.native.pty.lock().unwrap().response_close_state();
        assert!(!primary.same_state(&native));
        let observer = Arc::new(PollExecutor {
            original: f.native.executor(),
            log: log.clone(),
            entered: Notify::new(),
            resume: Mutex::new(None),
            started: Mutex::new(None),
            coordinator: coord,
        });
        let app = tauri::test::mock_builder()
            .manage(
                f.native
                    .app
                    .state::<crate::app_state::AppOwners>()
                    .inner()
                    .clone(),
            )
            .manage(f.native.manager.clone())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        if tracker {
            assert!(app.manage(primary.clone()));
        }
        if coord {
            assert!(app.manage(f.native.coordinator.clone()));
        }
        if bound {
            let port: PollPort = observer.clone();
            assert!(app.manage(LifecycleExecutorState::new(port)));
        }
        assert_eq!(
            app.try_state::<crate::pty::manager::ResponseCloseState>()
                .is_some(),
            tracker
        );
        assert_eq!(app.try_state::<SelectionCoordinator>().is_some(), coord);
        assert_eq!(
            app.try_state::<LifecycleExecutorState<PollTransaction>>()
                .is_some(),
            bound
        );
        let identity = |target| ResponseRouteIdentity {
            target,
            session_id: Uuid::new_v4(),
            token: Uuid::new_v4(),
            generation: 1,
            carrier_version: f.carrier.version,
            backend: crate::pty::backend::SessionBackendKind::LocalProcess,
        };
        let member = identity(f.route.target.canonical_fqn.clone());
        let sentinel = member.session_id;
        let outcome = Arc::new(AtomicU64::new(0));
        assert!(
            crate::session::selection::remember_response_close_registration(
                crate::session::selection::ResponseCloseRegistration {
                    tracker: native.clone(),
                    member,
                    coordinator: identity(f.route.sender.canonical_fqn.clone()),
                    carrier: f.carrier.clone(),
                    diagnostic_last_poll_outcome: outcome.clone(),
                }
            )
        );
        let registration =
            crate::session::selection::response_close_registration(sentinel).unwrap();
        assert!(registration.tracker.same_state(&native));
        assert!(!registration.tracker.same_state(&primary));
        assert!(Arc::ptr_eq(
            &registration.diagnostic_last_poll_outcome,
            &outcome
        ));
        assert_eq!(outcome.load(Ordering::Acquire), 0);
        log.lock().unwrap().clear();
        Self {
            app,
            observer,
            outcome,
            sentinel,
            log,
        }
    }
    async fn poll(&self, direct: bool) {
        if direct {
            tokio::time::timeout(
                Duration::from_secs(5),
                crate::session::selection::poll_response_close(self.app.handle(), None),
            )
            .await
            .unwrap();
        } else {
            tokio::time::timeout(
                Duration::from_secs(5),
                crate::commands::session::selection_executor::poll_response_close(
                    self.app.handle(),
                ),
            )
            .await
            .unwrap();
        }
    }
    fn gate(&self) -> oneshot::Sender<()> {
        let (send, recv) = oneshot::channel();
        assert!(self.observer.resume.lock().unwrap().replace(recv).is_none());
        send
    }
    fn check(&self, tracker: bool, coord: bool, entries: usize, completed: usize) {
        let logs = self.log.lock().unwrap();
        let count = |text: &str| logs.iter().filter(|line| line.contains(text)).count();
        assert_eq!(count("entry"), entries, "{logs:?}");
        assert_eq!(count("completed"), completed, "{logs:?}");
        assert_eq!(count("stage=poll_health"), 1, "{logs:?}");
        if tracker {
            assert!(logs.iter().any(|l| l.contains("stage=poll_health")
                && l.contains("scope=pre_reconcile_pre_selection")
                && l.contains("outcome=observed")));
        } else {
            assert_eq!(count("outcome=tracker_missing"), 1);
        }
        assert_eq!(
            self.outcome.load(Ordering::Acquire),
            u64::from(tracker && coord)
        );
        if entries > 0 {
            let health = logs
                .iter()
                .position(|l| l.contains("stage=poll_health"))
                .unwrap();
            let entry = logs.iter().position(|l| l == "entry").unwrap();
            assert!(health < entry);
        }
        if tracker && !coord {
            let missing = logs
                .iter()
                .position(|l| l.contains("outcome=coordinator_missing"))
                .unwrap();
            if completed > 0 {
                assert!(logs.iter().position(|l| l == "completed").unwrap() < missing);
            }
        }
        if tracker && coord {
            assert_eq!(count("registration_count="), 1);
        }
    }
}
impl Drop for PollView {
    fn drop(&mut self) {
        crate::session::selection::forget_response_close_registration(self.sentinel);
    }
}

pub(crate) async fn response_close_case(case: u8) {
    let f = ProtectedFixture::new(false).await;
    let log: PollLog = Arc::new(Mutex::new(Vec::new()));
    log::set_logger(Box::leak(Box::new(PollLogger(log.clone())))).unwrap();
    log::set_max_level(log::LevelFilter::Debug);
    assert!(
        log::log_enabled!(target: "agentscommander_lib::response_close_diag", log::Level::Debug)
    );
    let (tracker, bound, coord) = match case {
        0 => (false, true, true),
        1 => (true, false, false),
        2 => (true, false, true),
        3 | 5 => (true, true, false),
        4 => (true, true, true),
        _ => unreachable!(),
    };
    let view = PollView::new(&f, log.clone(), tracker, bound, coord);
    assert!(Arc::ptr_eq(
        &view
            .app
            .state::<crate::app_state::AppOwners>()
            .response_effects,
        &f.native
            .app
            .state::<crate::app_state::AppOwners>()
            .response_effects,
    ));
    if case == 3 {
        let release = view.gate();
        let app = view.app.handle().clone();
        let task = tokio::spawn(async move {
            crate::commands::session::selection_executor::poll_response_close(&app).await;
        });
        tokio::time::timeout(Duration::from_secs(5), view.observer.entered.notified())
            .await
            .unwrap();
        assert!(!task.is_finished());
        assert_eq!(f.native.backend.kill_count(f.native.source), 0);
        assert!(f
            .native
            .app
            .state::<crate::app_state::AppOwners>()
            .response_effects
            .snapshot()
            .unwrap()
            .is_empty());
        let logs = log.lock().unwrap().clone();
        assert_eq!(logs.iter().filter(|l| *l == "completed").count(), 0);
        assert!(!logs
            .iter()
            .any(|l| l.contains("outcome=coordinator_missing")));
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        view.check(true, false, 1, 1);
    } else if case == 5 {
        let shutdown = crate::shutdown::ShutdownSignal::new();
        shutdown.trigger();
        let task = crate::commands::session::selection_executor::start_response_close_polling(
            view.app.handle().clone(),
            shutdown,
        );
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(log.lock().unwrap().is_empty());
        assert_eq!(view.outcome.load(Ordering::Acquire), 0);
        let shutdown = crate::shutdown::ShutdownSignal::new();
        let release = view.gate();
        *view.observer.started.lock().unwrap() = Some(Instant::now());
        let task = crate::commands::session::selection_executor::start_response_close_polling(
            view.app.handle().clone(),
            shutdown.clone(),
        );
        tokio::time::timeout(Duration::from_secs(5), view.observer.entered.notified())
            .await
            .unwrap();
        shutdown.trigger();
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        view.check(true, false, 1, 1);
    } else {
        view.poll(false).await;
        view.check(
            tracker,
            coord,
            usize::from(bound && tracker),
            usize::from(bound && tracker),
        );
    }
    if case == 1 || case == 2 {
        assert!(log
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("outcome=executor_missing")));
        drop(view);
        let direct = PollView::new(&f, log.clone(), true, false, coord);
        direct.poll(true).await;
        direct.check(true, coord, 0, 0);
        assert!(log
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("outcome=executor_missing")));
    } else {
        drop(view);
    }
    if case == 4 {
        tokio::time::timeout(Duration::from_secs(20), f.retire())
            .await
            .unwrap();
        let n = &f.native;
        let status = n
            .manager
            .read()
            .await
            .get_session(n.source)
            .await
            .unwrap()
            .status;
        assert_eq!(status, crate::session::session::SessionStatus::Exited(0));
        assert_eq!(n.backend.kill_count(n.source), 1);
        assert!(crate::session::selection::response_close_registration(n.source).is_none());
        assert!(n
            .app
            .state::<crate::app_state::AppOwners>()
            .response_effects
            .snapshot()
            .unwrap()
            .is_empty());
        crate::commands::session::selection_executor::poll_response_close(n.app.handle()).await;
        n.coordinator.snapshot().await.unwrap();
        assert_eq!(n.backend.kill_count(n.source), 1);
        let t = n.transaction();
        let port = n.executor();
        let LifecycleAdmission::Ready(lease) = port
            .prepare(&t, Uuid::new_v4(), Some(f.sender), false, None)
            .await
            .unwrap()
        else {
            panic!("sender lease required")
        };
        assert!(port.has_claim(&lease));
        let receipt = lease
            .downcast_ref::<super::super::NativeLifecycleLease>()
            .unwrap()
            .release_receipt
            .clone();
        assert!(port.release(lease));
        assert_eq!(receipt.load(Ordering::Acquire), 1);
    }
    f.native.coordinator.close_and_join().await;
}
