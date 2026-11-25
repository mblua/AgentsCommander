//! Missing managed state: the Tauri port names the absent type, and `AutoClose::run`
//! skips that tick without side effects, then recovers on the same task.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::Manager;

const LEAD: &str = "myproj:wg-1-team/lead";

struct Handles {
    settings: SettingsState,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pty: Arc<Mutex<PtyManager>>,
    idle: Arc<IdleDetector>,
    coordinator: SelectionCoordinator,
}

fn handles() -> Handles {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(AutoCloseBackend::default());
    let shutdown = crate::shutdown::ShutdownSignal::new();
    Handles {
        settings: crate::config::settings::new_settings_state(Default::default()),
        coordinator: SelectionCoordinator::new(Arc::clone(&manager), shutdown.token().clone()),
        manager,
        pty: Arc::new(Mutex::new(PtyManager::new_for_test(backend))),
        idle: IdleDetector::new(|_| {}, |_| {}),
    }
}

/// A mock app managing every required handle except the one at `skip`.
fn app_without(h: &Handles, skip: usize) -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    manage_index(&app, h, (0..5).filter(|i| *i != skip));
    app
}

fn manage_index(
    app: &impl Manager<tauri::test::MockRuntime>,
    h: &Handles,
    which: impl Iterator<Item = usize>,
) {
    for i in which {
        match i {
            0 => app.manage(h.settings.clone()),
            1 => app.manage(Arc::clone(&h.manager)),
            2 => app.manage(Arc::clone(&h.pty)),
            3 => app.manage(Arc::clone(&h.idle)),
            _ => app.manage(h.coordinator.clone()),
        };
    }
}

/// All five returned handles are the managed ones. The coordinator has no identity
/// accessor, so its shared shutdown token stands in: cancelling the returned
/// coordinator's token is seen by the managed one and not by an unrelated decoy.
fn assert_same_handles(inputs: AutoCloseInputs, h: &Handles) {
    assert!(Arc::ptr_eq(&inputs.settings, &h.settings));
    assert!(Arc::ptr_eq(&inputs.session_manager, &h.manager));
    assert!(Arc::ptr_eq(&inputs.pty_manager, &h.pty));
    assert!(Arc::ptr_eq(&inputs.idle_detector, &h.idle));
    let decoy = handles().coordinator;
    assert!(!h.coordinator.shutdown_token().is_cancelled());
    inputs.selection_coordinator.shutdown_token().cancel();
    assert!(h.coordinator.shutdown_token().is_cancelled());
    assert!(!decoy.shutdown_token().is_cancelled());
}

#[test]
fn tauri_inputs_name_each_missing_type_and_return_the_managed_handles() {
    let names = [
        std::any::type_name::<SettingsState>(),
        std::any::type_name::<Arc<tokio::sync::RwLock<SessionManager>>>(),
        std::any::type_name::<Arc<Mutex<PtyManager>>>(),
        std::any::type_name::<Arc<IdleDetector>>(),
        std::any::type_name::<SelectionCoordinator>(),
    ];
    for (skip, name) in names.iter().enumerate() {
        for clock_present in [false, true] {
            let h = handles();
            let app = app_without(&h, skip);
            let handle = app.handle();
            let clocks: CoordinatorClocksState = Arc::new(Mutex::new(CoordinatorClocks::default()));
            if clock_present {
                app.manage(clocks.clone());
            }
            assert_eq!(AutoCloseServices::inputs(handle).err(), Some(*name));
            manage_index(&app, &h, std::iter::once(skip));
            let inputs = AutoCloseServices::inputs(handle).expect("all five managed");
            match handle.coordinator_clocks() {
                Some(managed) => assert!(clock_present && Arc::ptr_eq(&clocks, &managed)),
                None => assert!(!clock_present),
            }
            assert_same_handles(inputs, &h);
        }
    }
}

static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct AutoCloseLog;

impl log::Log for AutoCloseLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.target() == "auto-close"
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) && record.level() == log::Level::Warn {
            WARNINGS.lock().unwrap().push(record.args().to_string());
        }
    }
    fn flush(&self) {}
}

/// Delegates the five handles to a real mock `AppHandle`; badges and saves go to the
/// shared `RecordingServices` spy, and clock queries are counted.
struct Delegated {
    app: tauri::AppHandle<tauri::test::MockRuntime>,
    rec: RecordingServices,
    clock_queries: AtomicUsize,
}

impl AutoCloseServices for Arc<Delegated> {
    fn inputs(&self) -> Result<AutoCloseInputs, &'static str> {
        AutoCloseServices::inputs(&self.app)
    }
    fn coordinator_clocks(&self) -> Option<CoordinatorClocksState> {
        self.clock_queries.fetch_add(1, Ordering::SeqCst);
        self.rec.coordinator_clocks()
    }
    fn idle_badge(&self, cwd: &str, at: DateTime<Utc>) {
        self.rec.idle_badge(cwd, at);
    }
    fn auto_closed_badge(&self, cwd: &str, at: DateTime<Utc>) {
        self.rec.auto_closed_badge(cwd, at);
    }
    fn save_clocks(&self, map: &HashMap<String, ClockEntry>) -> Result<(), String> {
        self.rec.save_clocks(map)
    }
}

impl Delegated {
    fn counts(&self) -> (usize, usize, usize, usize) {
        (
            WARNINGS.lock().unwrap().len(),
            self.clock_queries.load(Ordering::SeqCst),
            self.rec.notices.lock().unwrap().len(),
            self.rec.saved.lock().unwrap().len(),
        )
    }
}

async fn settle_until(spy: &Delegated, done: impl Fn((usize, usize, usize, usize)) -> bool) {
    for _ in 0..10_000 {
        if done(spy.counts()) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("no observation: {:?}", spy.counts());
}

async fn yields(n: usize) {
    for _ in 0..n {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn run_skips_a_tick_with_missing_state_and_recovers_on_the_same_task() {
    use crate::pty::backend::{PtyBackend, SessionBackendKind};
    log::set_logger(&AutoCloseLog).unwrap();
    log::set_max_level(log::LevelFilter::Warn);

    let temp = tempfile::tempdir().unwrap();
    let team = temp.path().join("myproj/.ac/wg-1-team");
    let (lead_cwd, member_cwd) = (team.join("__agent_lead"), team.join("__agent_rust"));
    let h = handles();
    let lead = auto_close_session(&h.manager, &lead_cwd, None, true).await;
    let member = auto_close_session(&h.manager, &member_cwd, Some("codex".into()), false).await;
    let backend = Arc::new(AutoCloseBackend::default());
    *h.pty.lock().unwrap() = PtyManager::new_for_test(backend.clone());
    for id in [lead, member] {
        backend.set_live(id);
        h.pty
            .lock()
            .unwrap()
            .record_route(id, SessionBackendKind::LocalProcess);
    }
    // Disabled: badges only. Members are not tracked by the detector, so no activity
    // advance moves the anchor between ticks.
    h.settings.write().await.coordinator_auto_close_enabled = false;
    let anchor = ts(1_700_000_000);
    let mut seeded = CoordinatorClocks::default();
    assert!(seeded.note_user_message(LEAD, anchor));
    let before = seeded.snapshot();
    let app = app_without(&h, 4);
    let spy = Arc::new(Delegated {
        app: app.handle().clone(),
        rec: RecordingServices::new(
            h.settings.clone(),
            (
                Arc::clone(&h.manager),
                Arc::clone(&h.pty),
                Arc::clone(&h.idle),
            ),
            h.coordinator.clone(),
            Arc::new(Mutex::new(seeded)),
        ),
        clock_queries: AtomicUsize::new(0),
    });
    let shutdown = crate::shutdown::ShutdownSignal::new();
    let task = tokio::spawn(AutoClose::run(Arc::clone(&spy), shutdown.clone()));
    yields(50).await;

    tokio::time::advance(TICK).await;
    settle_until(&spy, |(warnings, ..)| warnings == 1).await;
    yields(50).await;
    assert_eq!(spy.counts(), (1, 0, 0, 0));
    let warning = WARNINGS.lock().unwrap()[0].clone();
    assert!(
        warning.contains(std::any::type_name::<SelectionCoordinator>()),
        "{warning}"
    );
    assert!(warning.contains("skipping tick"), "{warning}");
    assert_eq!(spy.rec.clocks.lock().unwrap().snapshot(), before);
    assert_eq!(h.manager.read().await.list_sessions().await.len(), 2);
    assert!(backend.has_session(lead) && backend.has_session(member));

    manage_index(&spy.app, &h, std::iter::once(4));
    tokio::time::advance(TICK).await;
    settle_until(&spy, |(.., saves)| saves == 1).await;
    yields(50).await;
    let cwd = lead_cwd.to_string_lossy().into_owned();
    assert_eq!(
        *spy.rec.notices.lock().unwrap(),
        vec![Badge::Idle(cwd, anchor)]
    );
    assert_eq!(spy.rec.saved.lock().unwrap()[0], before);
    assert_eq!(WARNINGS.lock().unwrap().len(), 1);

    let queries = spy.counts().1;
    tokio::time::advance(TICK).await;
    settle_until(&spy, |(_, q, ..)| q > queries).await;
    yields(50).await;
    assert_eq!(spy.counts().2, 1, "same anchor: no repeated badge");
    assert_eq!(spy.counts().3, 1, "clean map: no second save");

    shutdown.token().cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("run stops on cancellation")
        .unwrap();
    let after = spy.counts();
    tokio::time::advance(TICK * 3).await;
    yields(50).await;
    assert_eq!(spy.counts(), after);
    assert_eq!(h.manager.read().await.list_sessions().await.len(), 2);
}

#[tokio::test]
async fn tick_with_missing_state_keeps_dedup_sessions_markers_and_pty() {
    use crate::pty::backend::PtyBackend;
    let fixture = auto_close_fixture().await;
    let services = &fixture.services;
    services
        .settings
        .write()
        .await
        .coordinator_auto_close_enabled = false;
    let mut emitted = HashMap::new();
    assert_eq!(tick(services, &mut emitted).await, Ok(()));
    services
        .clocks
        .lock()
        .unwrap()
        .mark_auto_closed(LEAD, ts(1_700_000_500));
    let (dedup, clocks) = (emitted.clone(), services.clocks.lock().unwrap().snapshot());
    let sessions = services.manager.read().await.list_sessions().await.len();
    assert_eq!(sessions, 2);

    let missing = std::any::type_name::<Arc<IdleDetector>>();
    *services.missing.lock().unwrap() = Some(missing);
    assert_eq!(tick(services, &mut emitted).await, Err(missing));
    assert_eq!(emitted, dedup);
    assert_eq!(services.clocks.lock().unwrap().snapshot(), clocks);
    assert_eq!(
        services.manager.read().await.list_sessions().await.len(),
        sessions
    );
    assert!(fixture.backend.has_session(fixture.coordinator_id));
    assert!(fixture.backend.has_session(fixture.member_id));

    *services.missing.lock().unwrap() = None;
    assert_eq!(tick(services, &mut emitted).await, Ok(()));
    let first = DateTime::from_timestamp(fixture.anchor.timestamp(), 0).unwrap();
    assert_eq!(
        *services.notices.lock().unwrap(),
        vec![Badge::Idle(fixture.cwd.clone(), first)],
        "valid, missing, valid with the same anchor publishes once"
    );
    assert!(services.saved.lock().unwrap().is_empty());
    services.coordinator.close_and_join().await;
}
