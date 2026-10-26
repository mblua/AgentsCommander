use super::*;
use crate::config::settings::AppSettings;
use crate::resource_monitor::registry::{
    ProcessTreeBackend, ResourceError, ResourceLaunchRegistration,
};
use crate::resource_monitor::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory,
    ResourceAgentGroupSnapshot, ResourceKillReason, ResourceKillResult, ResourceLaunchMetadata,
    ResourceProcessSnapshot, TerminateOutcome,
};
use crate::session::manager::SessionManager;
use crate::web::broadcast::WsBroadcaster;
use crate::DetachedSessionsState;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct UnsupportedWatchdogBackend {
    observe_tree_calls: AtomicUsize,
    observe_identity_calls: AtomicUsize,
    terminate_verified_calls: AtomicUsize,
    current_process_memory_calls: AtomicUsize,
}

impl ProcessTreeBackend for UnsupportedWatchdogBackend {
    fn supports_process_tree_enforcement(&self) -> bool {
        false
    }

    fn observe_tree(&self, _root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
        self.observe_tree_calls.fetch_add(1, Ordering::SeqCst);
        Ok(ObservedProcessTree::default())
    }

    fn observe_identity(&self, _pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        self.observe_identity_calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }

    fn terminate_verified(
        &self,
        _process: &ObservedProcess,
    ) -> Result<TerminateOutcome, ResourceError> {
        self.terminate_verified_calls.fetch_add(1, Ordering::SeqCst);
        Ok(TerminateOutcome::AlreadyGone)
    }

    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
        self.current_process_memory_calls
            .fetch_add(1, Ordering::SeqCst);
        Ok(ProcessMemory::default())
    }
}

fn limits() -> ResourceLimits {
    ResourceLimits {
        monitor_enabled: true,
        max_concurrent_agent_processes: 5,
        group_warn_private_bytes: 100,
        group_kill_private_bytes: 200,
        process_kill_private_bytes: 300,
    }
}

fn identity(pid: u32) -> ProcessIdentity {
    ProcessIdentity {
        pid,
        creation_time_100ns: u64::from(pid),
    }
}

fn process(pid: u32, private_bytes: Option<u64>) -> ResourceProcessSnapshot {
    ResourceProcessSnapshot {
        identity: identity(pid),
        pid,
        parent_pid: None,
        name: format!("p{pid}"),
        exe_name: format!("p{pid}.exe"),
        private_bytes,
        working_set_bytes: None,
        cpu_percent: None,
        owned: true,
        kill_allowed: true,
        depth: 0,
    }
}

fn group(
    private_bytes: Option<u64>,
    processes: Vec<ResourceProcessSnapshot>,
) -> ResourceAgentGroupSnapshot {
    ResourceAgentGroupSnapshot {
        session_id: Uuid::new_v4().to_string(),
        name: "agent".to_string(),
        workgroup: None,
        agent: None,
        project: None,
        root_pid: 1,
        root_identity: identity(1),
        state: ResourceGroupState::Running,
        descendants_observed: true,
        process_count: processes.len(),
        private_bytes,
        working_set_bytes: None,
        cpu_percent: None,
        processes,
        kill_allowed: true,
        last_error: None,
    }
}

#[tokio::test]
async fn unsupported_backend_is_ineligible_for_start_and_tick() {
    let backend = Arc::new(UnsupportedWatchdogBackend::default());
    let monitor =
        ResourceMonitorState::with_backend(backend.clone() as Arc<dyn ProcessTreeBackend>);
    assert!(!watchdog_eligible(&monitor));

    let cfg = AppSettings {
        resource_monitor_enabled: true,
        ..AppSettings::default()
    };
    let settings = crate::config::settings::new_settings_state(cfg);
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
    let shutdown = ShutdownSignal::new();

    let start_fn: fn(ResourceMonitorState, SettingsState, SelectionCoordinator, ShutdownSignal) =
        start;
    start_fn(
        monitor.clone(),
        Arc::clone(&settings),
        coordinator.clone(),
        shutdown.clone(),
    );
    tokio::task::yield_now().await;

    let settings_guard = settings.write().await;
    tokio::time::timeout(
        Duration::from_millis(50),
        run_tick(&monitor, &settings, &coordinator),
    )
    .await
    .expect("unsupported tick returns before reading settings");
    shutdown.trigger();
    drop(settings_guard);
    tokio::task::yield_now().await;

    assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.terminate_verified_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        backend.current_process_memory_calls.load(Ordering::SeqCst),
        0
    );
}

#[test]
fn evaluates_group_warn_without_kill() {
    let id = Uuid::new_v4();
    let groups = vec![(id, group(Some(150), vec![process(10, Some(50))]))];

    let decisions = evaluate_watchdog_groups(&groups, limits());

    assert_eq!(decisions.len(), 1);
    assert!(decisions[0].warn_required);
    assert!(decisions[0].group_warn);
    assert!(!decisions[0].kill_required);
}

#[test]
fn evaluates_group_and_process_kill() {
    let id = Uuid::new_v4();
    let groups = vec![(id, group(Some(250), vec![process(10, Some(350))]))];

    let decisions = evaluate_watchdog_groups(&groups, limits());

    assert_eq!(decisions.len(), 1);
    assert!(decisions[0].group_kill);
    assert!(decisions[0].process_kill);
    assert_eq!(decisions[0].process_kill_pids, vec![10]);
    assert!(decisions[0].kill_required);
}

#[test]
fn ignores_non_running_groups() {
    let id = Uuid::new_v4();
    let mut group = group(Some(250), vec![process(10, Some(350))]);
    group.state = ResourceGroupState::Terminated;
    let groups = vec![(id, group)];

    assert!(evaluate_watchdog_groups(&groups, limits()).is_empty());
}

/// #1151 - supported backend with the shape every orphan takes: a root plus one owned
/// descendant, the descendant refusing to verify gone until it is marked gone. That
/// refusal is exactly what parks a group in `Quarantined` with its permit retained.
#[derive(Default)]
struct QuarantineWatchdogBackend {
    gone: Mutex<HashSet<u32>>,
    stubborn: Mutex<HashSet<u32>>,
    observe_tree_calls: AtomicUsize,
    observe_identity_calls: AtomicUsize,
    terminate_verified_calls: AtomicUsize,
}

impl QuarantineWatchdogBackend {
    fn identity(pid: u32) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            creation_time_100ns: u64::from(pid),
        }
    }

    fn child_pid(root_pid: u32) -> u32 {
        root_pid + 1
    }

    fn observed(pid: u32, depth: u32, parent_pid: Option<u32>) -> ObservedProcess {
        ObservedProcess {
            identity: Self::identity(pid),
            parent_pid,
            parent_identity: parent_pid.map(Self::identity),
            exe_name: format!("p{pid}"),
            depth,
            private_bytes: Some(10),
            working_set_bytes: Some(10),
            cpu_percent: None,
            kill_allowed: true,
        }
    }

    fn mark_stubborn(&self, pid: u32) {
        self.stubborn.lock().unwrap().insert(pid);
    }

    fn mark_gone(&self, pid: u32) {
        self.stubborn.lock().unwrap().remove(&pid);
        self.gone.lock().unwrap().insert(pid);
    }

    /// (observe_tree, observe_identity, terminate_verified) call counts.
    fn call_counts(&self) -> (usize, usize, usize) {
        (
            self.observe_tree_calls.load(Ordering::SeqCst),
            self.observe_identity_calls.load(Ordering::SeqCst),
            self.terminate_verified_calls.load(Ordering::SeqCst),
        )
    }
}

impl ProcessTreeBackend for QuarantineWatchdogBackend {
    fn observe_tree(&self, root: ProcessIdentity) -> Result<ObservedProcessTree, ResourceError> {
        self.observe_tree_calls.fetch_add(1, Ordering::SeqCst);
        let gone = self.gone.lock().unwrap();
        let child_pid = Self::child_pid(root.pid);
        let mut processes = Vec::new();
        if !gone.contains(&root.pid) {
            processes.push(Self::observed(root.pid, 0, None));
        }
        if !gone.contains(&child_pid) {
            processes.push(Self::observed(child_pid, 1, Some(root.pid)));
        }
        let errors = if gone.contains(&root.pid) {
            vec![format!("root pid {} was not in process snapshot", root.pid)]
        } else {
            Vec::new()
        };
        Ok(ObservedProcessTree { processes, errors })
    }

    fn observe_identity(&self, pid: u32) -> Result<Option<ProcessIdentity>, ResourceError> {
        self.observe_identity_calls.fetch_add(1, Ordering::SeqCst);
        if self.gone.lock().unwrap().contains(&pid) {
            Ok(None)
        } else {
            Ok(Some(Self::identity(pid)))
        }
    }

    fn terminate_verified(
        &self,
        process: &ObservedProcess,
    ) -> Result<TerminateOutcome, ResourceError> {
        self.terminate_verified_calls.fetch_add(1, Ordering::SeqCst);
        let pid = process.identity.pid;
        if self.gone.lock().unwrap().contains(&pid) {
            return Ok(TerminateOutcome::AlreadyGone);
        }
        if self.stubborn.lock().unwrap().contains(&pid) {
            return Err(ResourceError::Message(format!(
                "pid {pid}: process still alive after terminate"
            )));
        }
        self.gone.lock().unwrap().insert(pid);
        Ok(TerminateOutcome::Terminated)
    }

    fn current_process_memory(&self) -> Result<ProcessMemory, ResourceError> {
        Ok(ProcessMemory::default())
    }
}

/// #1151 - the default watchdog action is `Warn` (config/settings.rs), and the
/// quarantine retry must run there too. Every orphan test uses the default on purpose.
fn orphan_settings(max_groups: u32) -> AppSettings {
    AppSettings {
        resource_monitor_enabled: true,
        max_concurrent_agent_processes: max_groups,
        ..AppSettings::default()
    }
}

fn register_agent_group(
    monitor: &ResourceMonitorState,
    limits: ResourceLimits,
    session_id: Uuid,
    root_pid: u32,
    identity_triple: Option<(&str, &str, &str)>,
) -> ProcessIdentity {
    let permit = monitor
        .try_reserve_agent_slot(limits)
        .expect("reservation admitted")
        .expect("supported backend issues a permit");
    let (workgroup, agent, project) = match identity_triple {
        Some((workgroup, agent, project)) => (
            Some(workgroup.to_string()),
            Some(agent.to_string()),
            Some(project.to_string()),
        ),
        None => (None, None, None),
    };
    let mut registration = ResourceLaunchRegistration::new(
        monitor.clone(),
        permit,
        ResourceLaunchMetadata {
            session_id,
            name: "agent".to_string(),
            agent_id: None,
            agent_label: None,
            workgroup,
            agent,
            project,
        },
    );
    registration
        .register_root_pid(root_pid)
        .expect("register root pid")
        .expect("supported backend observes the root identity")
}

/// #1151 - the coordinator must be at `Running` or `watchdog_resource_kill` answers
/// `Err(Busy)` at its phase gate, before the absent-session check. `SelectionCoordinator::new`
/// starts at `Bootstrapping` and `CoordinatorPhase` is private, so the supported route
/// is `submit_restore_first`, which needs a started worker and therefore an AppHandle.
async fn running_coordinator(
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    monitor: Arc<ResourceMonitorState>,
) -> (tauri::App<tauri::test::MockRuntime>, SelectionCoordinator) {
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(Arc::clone(&manager))
        .manage(monitor)
        .manage(coordinator.clone())
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build orphan-retry watchdog app");
    coordinator
        .start(app.handle().clone())
        .expect("start orphan-retry coordinator");
    coordinator
        .submit_restore_first()
        .await
        .expect("restore-first admitted")
        .finish();
    (app, coordinator)
}

/// #1151 - drives one group from launch to an orphaned quarantine: register, quarantine
/// on the stubborn child, then let the blocker disappear. The public session row never
/// exists, which is what makes it an orphan.
///
/// #1160 - the retry backoff is deliberately NOT satisfied here. Callers that want a
/// DUE group use `orphan_one_group`, which is this plus the backdate; the not-yet-due
/// shape is the only one that can observe the `quarantine_retry_due` gate at all.
fn orphan_one_group_not_yet_due(
    monitor: &ResourceMonitorState,
    backend: &QuarantineWatchdogBackend,
    limits: ResourceLimits,
    session_id: Uuid,
    root_pid: u32,
    identity_triple: Option<(&str, &str, &str)>,
) -> ProcessIdentity {
    let child_pid = QuarantineWatchdogBackend::child_pid(root_pid);
    backend.mark_stubborn(child_pid);
    let root = register_agent_group(monitor, limits, session_id, root_pid, identity_triple);
    let quarantine = monitor
        .kill_group(session_id, ResourceKillReason::SessionDestroy)
        .expect("first cleanup runs");
    assert!(quarantine.quarantined);
    assert_eq!(quarantine.state, ResourceGroupState::Quarantined);
    backend.mark_gone(child_pid);
    root
}

/// #1151 - the same orphaned quarantine with its retry backoff already satisfied, which
/// is what every test that needs the retry to DISPATCH depends on. Behavior is identical
/// to the pre-#1160 helper of this name, so its existing callers are unaffected.
fn orphan_one_group(
    monitor: &ResourceMonitorState,
    backend: &QuarantineWatchdogBackend,
    limits: ResourceLimits,
    session_id: Uuid,
    root_pid: u32,
    identity_triple: Option<(&str, &str, &str)>,
) -> ProcessIdentity {
    let root = orphan_one_group_not_yet_due(
        monitor,
        backend,
        limits,
        session_id,
        root_pid,
        identity_triple,
    );
    monitor.test_backdate_quarantine_retry(session_id);
    root
}

#[tokio::test]
async fn e2992_p44_invalid_no_enforcement() {
    use crate::config::settings::{
        OperationOutcome, SettingsAvailability, SettingsOwner, SettingsOwnerError,
        SettingsPublisher,
    };

    let evidence = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/2992-evidence/P44");
    std::fs::create_dir_all(&evidence).unwrap();
    for invalid in [true, false] {
        let temp = tempfile::Builder::new()
            .prefix("watchdog-")
            .tempdir_in(&evidence)
            .unwrap();
        let path = temp.path().join("settings.30.instance.no-git.json");
        let cfg = orphan_settings(1);
        std::fs::write(&path, serde_json::to_vec(&cfg).unwrap()).unwrap();
        let owner = tokio::task::spawn_blocking(move || {
            SettingsOwner::open_active(&path, Duration::from_secs(5)).unwrap()
        })
        .await
        .unwrap();
        assert_eq!(owner.availability(), SettingsAvailability::Ready);
        assert!(owner.current_read().await.unwrap().resource_monitor_enabled);
        let backend = Arc::new(QuarantineWatchdogBackend::default());
        let monitor = Arc::new(ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>
        ));
        assert!(watchdog_eligible(&monitor));
        let limits = ResourceLimits::from(&cfg);
        let id = Uuid::new_v4();
        orphan_one_group(&monitor, &backend, limits, id, 440, None);
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (_app, coordinator) = running_coordinator(manager, Arc::clone(&monitor)).await;
        if invalid {
            let report = owner
                .with_operation("p44-watchdog-invalid", |_, guard| {
                    guard.publish(SettingsPublisher::SettingsSave, |path| {
                        let mut value: serde_json::Value = serde_json::from_slice(
                            &std::fs::read(path).map_err(|error| error.to_string())?,
                        )
                        .map_err(|error| error.to_string())?;
                        value["defaultShell"] = serde_json::json!(42);
                        std::fs::write(path, serde_json::to_vec(&value).unwrap())
                            .map_err(|error| error.to_string())
                    })
                })
                .await
                .unwrap();
            assert_eq!(report.outcome, OperationOutcome::PartialOrUnknown);
            assert_eq!(report.availability, SettingsAvailability::Invalid);
            assert!(matches!(
                owner.current_read().await,
                Err(SettingsOwnerError::Unavailable(
                    SettingsAvailability::Invalid
                ))
            ));
        }
        let before = backend.call_counts();
        run_tick(&monitor, &owner, &coordinator).await;
        if invalid {
            assert_eq!(backend.call_counts(), before);
            assert!(monitor.group_counts_toward_admission(id));
            assert_eq!(monitor.active_agent_groups(), 1);
            assert_eq!(next_delay(&monitor, &owner).await, Duration::from_secs(2));
            assert_eq!(owner.availability(), SettingsAvailability::Invalid);
        } else {
            assert!(backend.call_counts().0 > before.0);
            assert!(!monitor.group_counts_toward_admission(id));
            assert_eq!(monitor.active_agent_groups(), 0);
            let permit = monitor.try_reserve_agent_slot(limits).unwrap().unwrap();
            monitor.release_unregistered_permit(permit);
        }
        coordinator.close_and_join().await;
        tokio::task::spawn_blocking(move || {
            owner.drain_blocking(Duration::from_secs(5));
            assert_eq!(owner.availability(), SettingsAvailability::Closed);
        })
        .await
        .unwrap();
    }
}

// #1151 (W1) - the issue's minimum supported-backend regression, end to end through
// the REAL run_tick: an orphaned quarantined group whose blocker cleared is reclaimed,
// the cap recovers, and the next launch is admitted. Runs on the DEFAULT Warn action,
// which is what makes a mis-scoped action gate fail here rather than ship silently.
#[tokio::test]
async fn orphaned_quarantine_recovers_through_real_tick() {
    let backend = Arc::new(QuarantineWatchdogBackend::default());
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    let cfg = orphan_settings(1);
    assert_eq!(cfg.resource_watchdog_action, ResourceWatchdogAction::Warn);
    let group_limits = ResourceLimits::from(&cfg);

    let session_id = Uuid::new_v4();
    orphan_one_group(&monitor, &backend, group_limits, session_id, 200, None);
    assert_eq!(monitor.active_agent_groups(), 1);
    assert!(monitor.try_reserve_agent_slot(group_limits).is_err());

    // The public row never existed, so the coordinator can only answer NoPublicSession.
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (_app, coordinator) = running_coordinator(manager, Arc::clone(&monitor)).await;
    let settings = crate::config::settings::new_settings_state(cfg);

    run_tick(&monitor, &settings, &coordinator).await;

    assert!(!monitor.group_counts_toward_admission(session_id));
    assert_eq!(monitor.active_agent_groups(), 0);
    let permit = monitor
        .try_reserve_agent_slot(group_limits)
        .expect("the reclaimed slot admits the next launch")
        .expect("supported backend issues a permit");
    monitor.release_unregistered_permit(permit);
    coordinator.close_and_join().await;
}

// #1160 - the pin for the 15s quarantine-retry backoff gate: the
// `&& monitor.quarantine_retry_due(*session_id)` clause in run_quarantine_retries.
// Phase 1 is the half no other test can express, because every other orphan test
// backdates the group DUE before ticking: an orphan whose blocker has already cleared
// is still not retried while its backoff is unelapsed. Phase 2 re-ticks the same group
// with only the backoff satisfied, so this fails if the gate is inverted as well as if
// it is deleted.
//
// observe_tree is deliberately NOT pinned: sample_for_watchdog re-observes Quarantined
// groups on every snapshot by design (#647 C), so it moves even on a correctly skipped
// tick. Pinning it would fail on correct code.
#[tokio::test]
async fn not_yet_due_quarantine_is_skipped_then_retried_after_backoff() {
    let backend = Arc::new(QuarantineWatchdogBackend::default());
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    let cfg = orphan_settings(1);
    let group_limits = ResourceLimits::from(&cfg);

    let session_id = Uuid::new_v4();
    orphan_one_group_not_yet_due(&monitor, &backend, group_limits, session_id, 260, None);
    assert_eq!(monitor.active_agent_groups(), 1);

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (_app, coordinator) = running_coordinator(manager, Arc::clone(&monitor)).await;
    let settings = crate::config::settings::new_settings_state(cfg);

    // Phase 1: kill_started_at was set microseconds ago by the setup's kill_group, so
    // the 15s backoff cannot have elapsed and the gate must skip this group.
    let (_, identity_before, terminate_before) = backend.call_counts();
    run_tick(&monitor, &settings, &coordinator).await;
    let (_, identity_after, terminate_after) = backend.call_counts();

    assert_eq!(
        terminate_after, terminate_before,
        "a not-yet-due quarantine must not reach kill_group"
    );
    assert_eq!(
        identity_after, identity_before,
        "a skipped tick performs no identity syscall either"
    );
    assert!(
        monitor.group_counts_toward_admission(session_id),
        "the skipped group keeps holding its admission slot"
    );
    assert_eq!(monitor.active_agent_groups(), 1);
    assert!(
        monitor.try_reserve_agent_slot(group_limits).is_err(),
        "the permit is still held at cap 1"
    );
    let state = monitor
        .sample_for_watchdog(group_limits)
        .into_iter()
        .find(|(id, _)| *id == session_id)
        .map(|(_, group)| group.state);
    assert_eq!(state, Some(ResourceGroupState::Quarantined));

    // Phase 2: same group, same tick, only the backoff satisfied.
    monitor.test_backdate_quarantine_retry(session_id);
    run_tick(&monitor, &settings, &coordinator).await;

    assert!(
        !monitor.group_counts_toward_admission(session_id),
        "the due retry reclaims the slot"
    );
    assert_eq!(monitor.active_agent_groups(), 0);
    let (_, _, terminate_final) = backend.call_counts();
    assert!(
        terminate_final > terminate_after,
        "the due retry actually ran a cleanup pass"
    );
    let permit = monitor
        .try_reserve_agent_slot(group_limits)
        .expect("the reclaimed slot admits the next launch")
        .expect("supported backend issues a permit");
    monitor.release_unregistered_permit(permit);
    coordinator.close_and_join().await;
}

// #1151 (W7) - the named pin for `#559 (H2)`, which has been unpinned since it was
// written: the quarantine retry runs on EVERY configured action, starting with the
// DEFAULT Warn. This is what fails if the run_tick action gate is ever restored to an
// early return instead of a scoped `if` around the kill_required loop.
#[tokio::test]
async fn quarantine_retry_runs_in_warn_mode() {
    for (index, action) in [
        ResourceWatchdogAction::Warn,
        ResourceWatchdogAction::KillGroup,
    ]
    .into_iter()
    .enumerate()
    {
        let backend = Arc::new(QuarantineWatchdogBackend::default());
        let monitor = Arc::new(ResourceMonitorState::with_backend(
            backend.clone() as Arc<dyn ProcessTreeBackend>
        ));
        let cfg = AppSettings {
            resource_watchdog_action: action,
            ..orphan_settings(1)
        };
        let group_limits = ResourceLimits::from(&cfg);

        let session_id = Uuid::new_v4();
        orphan_one_group(
            &monitor,
            &backend,
            group_limits,
            session_id,
            240 + (index as u32) * 10,
            None,
        );
        assert_eq!(monitor.active_agent_groups(), 1);

        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let (_app, coordinator) = running_coordinator(manager, Arc::clone(&monitor)).await;
        let settings = crate::config::settings::new_settings_state(cfg);

        run_tick(&monitor, &settings, &coordinator).await;

        assert!(
            !monitor.group_counts_toward_admission(session_id),
            "{action:?}: the quarantine retry must run on every configured action"
        );
        assert_eq!(monitor.active_agent_groups(), 0, "{action:?}");
        coordinator.close_and_join().await;
    }
}

// #1151 (W3a) - the rule the issue states explicitly: only NoPublicSession may fall
// through to the registry-only retry. Treating AlreadyInFlight as an orphan would
// re-enter a cleanup another caller already owns.
#[test]
fn already_in_flight_is_not_an_orphan_fallback() {
    let completed = WatchdogKillOutcome::Completed(ResourceKillResult {
        session_id: Uuid::new_v4().to_string(),
        state: ResourceGroupState::Terminated,
        killed_processes: Vec::new(),
        quarantined: false,
        message: "resource group terminated and verified".to_string(),
        blocked_by_security: false,
        finalized: true,
    });
    assert!(!should_run_orphan_fallback(&completed));
    assert!(!should_run_orphan_fallback(
        &WatchdogKillOutcome::AlreadyInFlight
    ));
    assert!(should_run_orphan_fallback(
        &WatchdogKillOutcome::NoPublicSession
    ));
}

// #1151 (W3b) - and the same rule proved through retry_quarantined_group itself: the
// AlreadyInFlight arm touches the process-tree backend zero times.
#[tokio::test]
async fn already_in_flight_performs_zero_registry_calls() {
    use crate::pty::backend::SessionBackendKind;
    use crate::session::selection::CriticalAdmissionKind;

    let backend = Arc::new(QuarantineWatchdogBackend::default());
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    let cfg = orphan_settings(1);
    let group_limits = ResourceLimits::from(&cfg);

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/watchdog-in-flight".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();
    let root = orphan_one_group(&monitor, &backend, group_limits, session.id, 210, None);

    let (_app, coordinator) = running_coordinator(Arc::clone(&manager), Arc::clone(&monitor)).await;
    // Fill the queue so the first watchdog kill parks holding ONLY its critical key.
    let mut reservations = Vec::new();
    while let Ok(reservation) = coordinator.reserve_auto_close() {
        reservations.push(reservation);
    }
    let waiter = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.watchdog_resource_kill(session.id).await })
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        while !coordinator
            .critical_key_registered_for_test(session.id, CriticalAdmissionKind::WatchdogKill)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the parked watchdog kill registers its critical key");

    let before = backend.call_counts();
    let report = retry_quarantined_group(&monitor, &coordinator, session.id, root).await;
    assert_eq!(report.path, QuarantineRetryPath::AlreadyInFlight);
    assert_eq!(backend.call_counts(), before);
    assert!(monitor.group_counts_toward_admission(session.id));

    coordinator.close_and_join().await;
    drop(reservations);
    let _ = waiter.await;
}

// #1151 (W4) - retry_quarantined_group forwards the SAMPLED root identity verbatim, so
// a stale sample is refused by the registry guard instead of acting on whatever row
// now holds the uuid.
#[tokio::test]
async fn stale_sample_root_cannot_touch_replacement() {
    let backend = Arc::new(QuarantineWatchdogBackend::default());
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    let cfg = orphan_settings(1);
    let group_limits = ResourceLimits::from(&cfg);

    let session_id = Uuid::new_v4();
    orphan_one_group(&monitor, &backend, group_limits, session_id, 220, None);

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (_app, coordinator) = running_coordinator(manager, Arc::clone(&monitor)).await;

    let stale_root = ProcessIdentity {
        pid: 220,
        creation_time_100ns: 999_999,
    };
    let report = retry_quarantined_group(&monitor, &coordinator, session_id, stale_root).await;

    assert_eq!(report.path, QuarantineRetryPath::Skipped);
    assert_eq!(report.skipped, Some("rootMismatch"));
    assert_eq!(report.root_pid, 220);
    assert!(report.still_counts_toward_admission);
    assert_eq!(monitor.active_agent_groups(), 1);
    coordinator.close_and_join().await;
}

// #1151 (W8) - the restart shape end to end. Three consecutive cycles, each a DISTINCT
// uuid sharing one workgroup/agent/project at cap 1, which is what "repetition
// exhausts the cap" actually means. Rows must not accumulate across cycles.
#[tokio::test]
async fn three_restart_shaped_cycles_do_not_accumulate_rows() {
    let backend = Arc::new(QuarantineWatchdogBackend::default());
    let monitor = Arc::new(ResourceMonitorState::with_backend(
        backend.clone() as Arc<dyn ProcessTreeBackend>
    ));
    let cfg = orphan_settings(1);
    let group_limits = ResourceLimits::from(&cfg);
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let (_app, coordinator) = running_coordinator(manager, Arc::clone(&monitor)).await;
    let settings = crate::config::settings::new_settings_state(cfg);

    for cycle in 0..3u32 {
        let session_id = Uuid::new_v4();
        orphan_one_group(
            &monitor,
            &backend,
            group_limits,
            session_id,
            230 + cycle * 10,
            Some(("wg-1", "dev-rust", "ac")),
        );
        assert_eq!(
            monitor.active_agent_groups(),
            1,
            "cycle {cycle}: the quarantined row holds the only slot"
        );

        run_tick(&monitor, &settings, &coordinator).await;

        assert!(
            !monitor.group_counts_toward_admission(session_id),
            "cycle {cycle}: the reclaimed row no longer counts"
        );
        assert_eq!(
            monitor.active_agent_groups(),
            0,
            "cycle {cycle}: rows must not accumulate"
        );
        let permit = monitor
            .try_reserve_agent_slot(group_limits)
            .unwrap_or_else(|error| panic!("cycle {cycle}: relaunch rejected: {error}"))
            .expect("supported backend issues a permit");
        monitor.release_unregistered_permit(permit);
    }
    coordinator.close_and_join().await;
}
