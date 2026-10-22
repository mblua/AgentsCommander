use super::*;
use crate::capture::sink::SlotState;
use crate::commands::session::co_managed_tests::{configure_room, room_fixture};
use crate::commands::session::reader_demand_tests::harness;
use crate::commands::telegram::holds_room_reader_demand;
use crate::config::settings::SettingsState;
use crate::telegram::manager::{
    OutputSenderMap, ReaderConsumer, TelegramBridgeManager, TelegramBridgeState,
};
use tauri::Manager;

fn tee_len() -> usize {
    crate::logging::test_tee_snapshot().len()
}

/// The single cycle measure (plan 3.5): `cycle start` lines naming
/// `session_id` added since `before`.
fn u2a_cycle_starts(before: usize, session_id: uuid::Uuid) -> Vec<String> {
    let id = session_id.to_string();
    crate::logging::test_tee_snapshot()
        .into_iter()
        .skip(before)
        .filter(|line| line.contains("[co-managed] cycle start") && line.contains(&id))
        .collect()
}

/// Plan 3.5: (i) drain the channel without processing, (ii) handle
/// the drained triggers in order. Triggers born during (ii) stay
/// queued for the next call.
async fn u2a_drain_and_handle<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    handle: &CoManagedSupervisorHandle,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<CoManagedTrigger>,
) -> Vec<CoManagedTrigger> {
    let mut drained = Vec::new();
    while let Ok(trigger) = rx.try_recv() {
        drained.push(trigger);
    }
    for trigger in drained.iter().copied() {
        super::super::handle_co_managed_trigger(app, handle, trigger).await;
    }
    drained
}

fn recoveries(drained: &[CoManagedTrigger], session_id: uuid::Uuid) -> usize {
    drained
        .iter()
        .filter(|t| **t == CoManagedTrigger::Recovery(session_id))
        .count()
}

fn mark_of(
    handle: &CoManagedSupervisorHandle,
    id: uuid::Uuid,
) -> Option<(u64, Arc<CapturedRecord>)> {
    handle
        .lock_state()
        .pending_recovery
        .get(&id.to_string())
        .map(|mark| (mark.seq, Arc::clone(&mark.record)))
}

fn assert_mark(
    handle: &CoManagedSupervisorHandle,
    id: uuid::Uuid,
    seq: u64,
    record: &Arc<CapturedRecord>,
) {
    let (mark_seq, mark_record) = mark_of(handle, id).expect("the mount left a mark");
    assert_eq!(mark_seq, seq, "the mark keeps the discarded seq");
    assert!(
        Arc::ptr_eq(&mark_record, record),
        "the mark keeps the discarded record"
    );
}

async fn set_key<R: tauri::Runtime>(app: &tauri::AppHandle<R>, key: &str) {
    let settings = app.state::<SettingsState>();
    settings.write().await.jev_api_key = key.to_string();
}

const U2A_SCORES: [(&str, f32); 4] = [
    ("to-peer", 0.9),
    ("to-user", 0.1),
    ("to-root", 0.0),
    ("to-reply", 0.0),
];

struct Mounted {
    fixture: CoManagedFixture,
    app: tauri::App<tauri::test::MockRuntime>,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    registry: Arc<CaptureRegistry>,
    handle: CoManagedSupervisorHandle,
    rx: tokio::sync::mpsc::UnboundedReceiver<CoManagedTrigger>,
    id: uuid::Uuid,
    slot: CaptureSlot,
    record: Arc<CapturedRecord>,
    seq: u64,
}

/// A Co-managed room with the flag on and no key: one `SlotChanged`
/// is discarded as not ready, which leaves the mark. Nothing is
/// processed after this returns except what the test does.
async fn mount(status: SessionStatus) -> Mounted {
    crate::logging::test_install_logger();
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(U2A_SCORES.to_vec()).await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    let id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        status,
        &fixture.projects_dir,
    )
    .await;
    let (slot, record) = install_candidate(&registry, id, "candidate text");
    set_key(app.handle(), "").await;
    let (handle, rx) = CoManagedSupervisorHandle::new();
    super::super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::SlotChanged(id),
    )
    .await;
    let seq = slot.snapshot().seq;
    assert_mark(&handle, id, seq, &record);
    Mounted {
        fixture,
        app,
        manager,
        registry,
        handle,
        rx,
        id,
        slot,
        record,
        seq,
    }
}

async fn pass_and_drain(m: &mut Mounted) -> Vec<CoManagedTrigger> {
    super::super::recovery_pass(m.app.handle(), &m.handle).await;
    u2a_drain_and_handle(m.app.handle(), &m.handle, &mut m.rx).await
}

fn same_candidate(state: &SlotState, record: &Arc<CapturedRecord>) -> bool {
    state
        .value
        .record()
        .is_some_and(|held| Arc::ptr_eq(held, record))
}

enum Degrade {
    NoKey,
    NoCatalog,
    UnreadableCatalog,
}

/// Tests 1 to 3: the session starts with no Room and no Bot demand.
async fn keeps_slot_and_candidate(degrade: Degrade) {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    let catalog = fixture.room_path().join("catalog.json");
    match degrade {
        Degrade::NoKey => h.set_global_co_managed(true, "   ").await,
        Degrade::NoCatalog => std::fs::remove_file(&catalog).expect("remove catalog"),
        Degrade::UnreadableCatalog => std::fs::write(&catalog, "not json").expect("catalog"),
    }
    assert!(!holds_room_reader_demand(h.app.handle(), id).await);
    assert!(!h.captures.is_open(&id.to_string()));

    crate::reader_demand_pass(h.app.handle()).await;
    assert!(
        holds_room_reader_demand(h.app.handle(), id).await,
        "tick 1 installs the Room demand"
    );
    let slot = h
        .captures
        .slot(&id.to_string())
        .expect("tick 1 opens the slot");
    let record = co_managed_candidate(&id.to_string(), "candidate text");
    slot.offer(Arc::clone(&record), 0);
    let before = slot.snapshot();
    assert!(same_candidate(&before, &record));

    crate::reader_demand_pass(h.app.handle()).await;
    assert!(
        holds_room_reader_demand(h.app.handle(), id).await,
        "tick 2 keeps the Room demand"
    );
    assert!(h.captures.is_open(&id.to_string()), "the slot survives");
    let slot = h.captures.slot(&id.to_string()).expect("slot");
    let after = slot.snapshot();
    assert_eq!(after.seq, before.seq, "the candidate keeps its seq");
    assert!(same_candidate(&after, &record), "the candidate survives");
}

/// Test 1.
#[tokio::test]
async fn u2a_capturable_room_without_key_keeps_slot_and_candidate() {
    keeps_slot_and_candidate(Degrade::NoKey).await;
}

/// Test 2.
#[tokio::test]
async fn u2a_capturable_room_without_catalog_keeps_slot_and_candidate() {
    keeps_slot_and_candidate(Degrade::NoCatalog).await;
}

/// Test 3.
#[tokio::test]
async fn u2a_capturable_room_with_unreadable_catalog_keeps_slot_and_candidate() {
    keeps_slot_and_candidate(Degrade::UnreadableCatalog).await;
}

/// Test 4.
#[tokio::test]
async fn u2a_global_switch_off_releases_demand_and_closes_slot() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    crate::reader_demand_pass(h.app.handle()).await;
    assert!(holds_room_reader_demand(h.app.handle(), id).await);
    assert!(h.captures.is_open(&id.to_string()));

    h.set_global_co_managed(false, "test-key").await;
    crate::reader_demand_pass(h.app.handle()).await;

    assert!(!holds_room_reader_demand(h.app.handle(), id).await);
    assert!(!h.captures.is_open(&id.to_string()), "the slot closes");
}

/// Test 5.
#[tokio::test]
async fn u2a_non_orchestrator_never_raises_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let member = h.session_in(fixture.member_path()).await;
    h.set_global_co_managed(true, "   ").await;
    for _ in 0..3 {
        crate::reader_demand_pass(h.app.handle()).await;
    }
    h.set_global_co_managed(true, "test-key").await;
    for _ in 0..3 {
        crate::reader_demand_pass(h.app.handle()).await;
    }
    assert!(!holds_room_reader_demand(h.app.handle(), member).await);
    assert!(!h.captures.is_open(&member.to_string()));
}

/// Test 6, and the positive control of the cycle counter.
#[tokio::test]
async fn u2a_recovery_runs_one_cycle_when_the_key_appears() {
    let mut m = mount(SessionStatus::Idle).await;
    set_key(m.app.handle(), "test-key").await;
    let before = tee_len();
    let drained = pass_and_drain(&mut m).await;
    assert_eq!(recoveries(&drained, m.id), 1, "{drained:?}");
    let cycles = u2a_cycle_starts(before, m.id);
    assert_eq!(cycles.len(), 1, "{cycles:?}");
    assert!(cycles[0].contains("trigger=Recovery"), "{cycles:?}");
    assert!(
        mark_of(&m.handle, m.id).is_none(),
        "emitting clears the mark"
    );
}

/// Test 7.
#[tokio::test]
async fn u2a_recovery_without_key_runs_no_cycle() {
    let mut m = mount(SessionStatus::Idle).await;
    let before = tee_len();
    let drained = pass_and_drain(&mut m).await;
    assert_eq!(recoveries(&drained, m.id), 0, "{drained:?}");
    assert!(u2a_cycle_starts(before, m.id).is_empty());
    assert_mark(&m.handle, m.id, m.seq, &m.record);
}

/// Test 8.
#[tokio::test]
async fn u2a_recovery_without_candidate_runs_no_cycle() {
    let mut m = mount(SessionStatus::Idle).await;
    m.slot.clear();
    set_key(m.app.handle(), "test-key").await;
    let before = tee_len();
    let drained = pass_and_drain(&mut m).await;
    assert_eq!(recoveries(&drained, m.id), 0, "{drained:?}");
    assert!(u2a_cycle_starts(before, m.id).is_empty());
    assert!(mark_of(&m.handle, m.id).is_none());
}

/// Test 9.
#[tokio::test]
async fn u2a_recovery_twice_in_a_row_runs_one_cycle() {
    let mut m = mount(SessionStatus::Idle).await;
    set_key(m.app.handle(), "test-key").await;
    let before = tee_len();
    let drained = pass_and_drain(&mut m).await;
    assert_eq!(recoveries(&drained, m.id), 1, "{drained:?}");
    assert_eq!(u2a_cycle_starts(before, m.id).len(), 1);

    let before = tee_len();
    let drained = pass_and_drain(&mut m).await;
    assert_eq!(recoveries(&drained, m.id), 0, "{drained:?}");
    assert!(u2a_cycle_starts(before, m.id).is_empty());
}

/// Test 10: startup with the key, in the fixed order of plan 9.3.
#[tokio::test]
async fn u2a_startup_with_key_runs_no_recovery_cycle() {
    crate::logging::test_install_logger();
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(U2A_SCORES.to_vec()).await;
    let (app, manager, registry) = co_managed_app(&fixture, endpoint, true);
    let id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Idle,
        &fixture.projects_dir,
    )
    .await;
    install_candidate(&registry, id, "candidate text");
    let (handle, mut rx) = CoManagedSupervisorHandle::new();

    // (a)
    super::super::watch_capture_slots(app.handle(), &handle).await;
    // (b)
    let trigger = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the discovery trigger is already queued")
        .expect("channel open");
    assert_eq!(trigger, CoManagedTrigger::SlotChanged(id));
    let before = tee_len();
    super::super::handle_co_managed_trigger(app.handle(), &handle, trigger).await;
    // (c)
    assert_eq!(u2a_cycle_starts(before, id).len(), 1);
    // (d)
    assert!(
        mark_of(&handle, id).is_none(),
        "a Ready room leaves no mark"
    );
    // (e)
    let before = tee_len();
    super::super::recovery_pass(app.handle(), &handle).await;
    let drained = u2a_drain_and_handle(app.handle(), &handle, &mut rx).await;
    // (f)
    assert!(u2a_cycle_starts(before, id).is_empty());
    assert_eq!(recoveries(&drained, id), 0, "{drained:?}");
}

/// Test 11: `PreconditionsRejected` is forced by a second live session
/// in the same directory (`unique_live_session_for_cwd == false`),
/// which is not retryable, unlike `LockBusy`.
#[tokio::test]
async fn u2a_rejected_recovery_retries_on_the_next_idle_edge() {
    let mut m = mount(SessionStatus::Idle).await;
    add_claude_session(
        &m.manager,
        &m.fixture.coordinator_cwd,
        SessionStatus::Running,
        &m.fixture.projects_dir,
    )
    .await;
    set_key(m.app.handle(), "test-key").await;

    let before = tee_len();
    let drained = pass_and_drain(&mut m).await;
    assert_eq!(recoveries(&drained, m.id), 1, "{drained:?}");
    assert_eq!(u2a_cycle_starts(before, m.id).len(), 1);
    let id = m.id.to_string();
    let rejected = crate::logging::test_tee_snapshot()
        .into_iter()
        .skip(before)
        .any(|line| {
            line.contains("cycle end")
                && line.contains(&id)
                && line.contains("PreconditionsRejected")
        });
    assert!(rejected, "the cycle ends Rejected(PreconditionsRejected)");

    let before = tee_len();
    let drained = pass_and_drain(&mut m).await;
    assert_eq!(recoveries(&drained, m.id), 0, "{drained:?}");
    assert!(u2a_cycle_starts(before, m.id).is_empty());

    let before = tee_len();
    emit_session_idle_edge(m.app.handle(), &m.handle.armed, Some(&m.handle), m.id);
    let drained = u2a_drain_and_handle(m.app.handle(), &m.handle, &mut m.rx).await;
    assert_eq!(drained, vec![CoManagedTrigger::IdleEdge(m.id)]);
    assert_eq!(u2a_cycle_starts(before, m.id).len(), 1);
}

/// Test 12: no idle gate in the pass (R1).
#[tokio::test]
async fn u2a_recovery_on_a_busy_session_arms_and_defers() {
    let mut m = mount(SessionStatus::Running).await;
    set_key(m.app.handle(), "test-key").await;

    let before = tee_len();
    let drained = pass_and_drain(&mut m).await;
    assert_eq!(recoveries(&drained, m.id), 1, "{drained:?}");
    assert!(u2a_cycle_starts(before, m.id).is_empty());
    assert!(m.handle.armed.is_armed(&m.id.to_string()), "Recovery arms");

    let before = tee_len();
    let comanaged = emit_session_idle_edge(m.app.handle(), &m.handle.armed, Some(&m.handle), m.id);
    assert!(comanaged, "session_idle.comanaged must be true");
    let drained = u2a_drain_and_handle(m.app.handle(), &m.handle, &mut m.rx).await;
    assert_eq!(drained, vec![CoManagedTrigger::IdleEdge(m.id)]);
    assert_eq!(u2a_cycle_starts(before, m.id).len(), 1);
}

/// Test 13: the identity leg.
#[tokio::test]
async fn u2a_reopened_slot_with_the_same_seq_runs_no_cycle() {
    let mut m = mount(SessionStatus::Idle).await;
    let id = m.id.to_string();
    m.registry.close(&id);
    let (capture, _rx) = m.registry.open(&id);
    let other = co_managed_candidate(&id, "another candidate");
    capture.slot.offer(Arc::clone(&other), 0);
    let state = capture.slot.snapshot();
    assert_eq!(state.seq, m.seq, "the reopened slot repeats the seq");
    assert!(same_candidate(&state, &other));
    assert_mark(&m.handle, m.id, m.seq, &m.record);
    set_key(m.app.handle(), "test-key").await;

    let before = tee_len();
    let drained = pass_and_drain(&mut m).await;
    assert_eq!(recoveries(&drained, m.id), 0, "{drained:?}");
    assert!(u2a_cycle_starts(before, m.id).is_empty());
}

/// Test 14: the `seq` leg. The record is `Live`, never `Preamble`, so
/// `offer` publishes it again instead of deduplicating it. A slot API
/// construction, not a production scenario.
#[tokio::test]
async fn u2a_same_record_at_a_newer_seq_runs_no_cycle() {
    let mut m = mount(SessionStatus::Idle).await;
    assert_ne!(m.record.origin, RecordOrigin::Preamble);
    m.slot.offer(Arc::clone(&m.record), 0);
    let state = m.slot.snapshot();
    assert_eq!(state.seq, m.seq + 1, "the same Arc at a newer seq");
    assert!(same_candidate(&state, &m.record));
    assert_mark(&m.handle, m.id, m.seq, &m.record);
    set_key(m.app.handle(), "test-key").await;

    let before = tee_len();
    let drained = pass_and_drain(&mut m).await;
    assert_eq!(recoveries(&drained, m.id), 0, "{drained:?}");
    assert!(u2a_cycle_starts(before, m.id).is_empty());
}

/// Test 15: a Bot demand keeps the slot while the room flag is off;
/// the candidate it kept is recovered once the flag is on.
#[tokio::test]
async fn u2a_bot_demand_keeps_the_slot_while_the_room_flag_is_off() {
    crate::logging::test_install_logger();
    let fixture = make_co_managed_fixture();
    let (endpoint, _hits) = spawn_jev_listener(U2A_SCORES.to_vec()).await;
    write_co_managed_room_config(&fixture.room_root, false, Some("catalog.json"));
    write_co_managed_catalog(&fixture.room_root);
    let settings = AppSettings {
        project_paths: vec![fixture.project.to_string_lossy().to_string()],
        jev_api_key: "test-key".to_string(),
        jev_model: "jev-1.13.0".to_string(),
        jev_endpoint: endpoint,
        jev_timeout_secs: 5,
        jev_threshold: 0.70,
        jev_margin: 0.15,
        co_managed_enabled: true,
        ..AppSettings::default()
    };
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let registry = Arc::new(CaptureRegistry::new());
    let senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let bridge: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::with_captures(senders, Arc::clone(&registry)),
    ));
    let idle_detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let app = tauri::test::mock_builder()
        .manage(crate::config::settings::new_settings_state(settings))
        .manage(manager.clone())
        .manage(crate::network::OutboundNetwork::new().expect("shared outbound network"))
        .manage(registry.clone())
        .manage(bridge)
        .manage(crate::pty::input_activity::new_state())
        .manage(idle_detector)
        .manage(Arc::new(crate::session::purge_guard::PurgeGuard::default()))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build U2a Bot test app");
    let id = add_claude_session(
        &manager,
        &fixture.coordinator_cwd,
        SessionStatus::Idle,
        &fixture.projects_dir,
    )
    .await;
    assert!(
        crate::commands::telegram::raise_reader_demand(
            app.handle(),
            id,
            ReaderConsumer::Bot,
            None,
        )
        .await,
        "the Bot demand opens the slot"
    );
    let key = id.to_string();
    let slot = registry.slot(&key).expect("slot");
    let record = co_managed_candidate(&key, "candidate text");
    slot.offer(Arc::clone(&record), 0);
    let (handle, mut rx) = CoManagedSupervisorHandle::new();
    super::super::handle_co_managed_trigger(
        app.handle(),
        &handle,
        CoManagedTrigger::SlotChanged(id),
    )
    .await;
    assert_mark(&handle, id, slot.snapshot().seq, &record);

    crate::reader_demand_pass(app.handle()).await;
    assert!(registry.is_open(&key), "the Bot demand keeps the slot");
    assert!(!holds_room_reader_demand(app.handle(), id).await);
    assert_mark(&handle, id, slot.snapshot().seq, &record);

    write_co_managed_room_config(&fixture.room_root, true, Some("catalog.json"));
    let before = tee_len();
    super::super::recovery_pass(app.handle(), &handle).await;
    let drained = u2a_drain_and_handle(app.handle(), &handle, &mut rx).await;
    assert_eq!(recoveries(&drained, id), 1, "{drained:?}");
    assert_eq!(u2a_cycle_starts(before, id).len(), 1);
}
