use super::*;

#[derive(Default)]
struct UnsupportedAutomationBackend {
    observe_tree_calls: AtomicUsize,
    observe_identity_calls: AtomicUsize,
    terminate_verified_calls: AtomicUsize,
    current_process_memory_calls: AtomicUsize,
}

impl ProcessTreeBackend for UnsupportedAutomationBackend {
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

/// #1151 - supported process-tree backend for the automation tests. `observe_tree`
/// synthesises a root plus one child from the requested root identity, so every
/// registered group gets distinct identities and a private-bytes total above the
/// kill thresholds these tests configure. A pid marked stubborn fails
/// `terminate_verified`, which is what parks a group in `Quarantined`; marking it
/// gone afterwards is what lets a later retry verify the cleanup.
#[derive(Default)]
struct AutomationProcessBackend {
    stubborn: Mutex<HashSet<u32>>,
    gone: Mutex<HashSet<u32>>,
    observe_tree_calls: AtomicUsize,
    observe_identity_calls: AtomicUsize,
    terminate_verified_calls: AtomicUsize,
}

impl AutomationProcessBackend {
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
            private_bytes: Some(5_000),
            working_set_bytes: Some(5_000),
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
}

impl ProcessTreeBackend for AutomationProcessBackend {
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

fn automation_settings() -> AppSettings {
    AppSettings {
        resource_monitor_enabled: true,
        resource_watchdog_action: ResourceWatchdogAction::KillGroup,
        max_concurrent_agent_processes: 4,
        agent_group_warn_private_bytes: 100,
        agent_group_kill_private_bytes: 200,
        agent_process_kill_private_bytes: 300,
        ..AppSettings::default()
    }
}

fn register_automation_group(
    monitor: &crate::resource_monitor::ResourceMonitorState,
    limits: crate::resource_monitor::ResourceLimits,
    session_id: Uuid,
    root_pid: u32,
) -> ProcessIdentity {
    let permit = monitor.try_reserve_agent_slot(limits).unwrap().unwrap();
    let mut registration = ResourceLaunchRegistration::new(
        monitor.clone(),
        permit,
        ResourceLaunchMetadata {
            session_id,
            name: "agent".to_string(),
            agent_id: None,
            agent_label: None,
            workgroup: None,
            agent: None,
            project: None,
        },
    );
    registration
        .register_root_pid(root_pid)
        .expect("register automation root pid")
        .expect("supported backend observes the root identity")
}

fn watchdog_backend_request(mode: &str) -> UiAutomationRequest {
    UiAutomationRequest {
        request_id: Uuid::new_v4().to_string(),
        token: "token".to_string(),
        window: BACKEND_AUTOMATION_WINDOW.to_string(),
        action: UiAutomationAction::Backend,
        selector: RESOURCE_WATCHDOG_BACKEND_SELECTOR.to_string(),
        value: Some(mode.to_string()),
        expires_at_unix_ms: Some(now_unix_ms() + 1_000),
    }
}

#[tokio::test]
async fn unsupported_backend_reports_disabled_and_never_kills() {
    let backend = Arc::new(UnsupportedAutomationBackend::default());
    let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
        backend.clone() as Arc<dyn ProcessTreeBackend>,
    ));
    let app = tauri::test::mock_builder()
        .manage(monitor)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build unsupported resource-watchdog automation app");
    let cfg = AppSettings {
        resource_monitor_enabled: true,
        resource_watchdog_action: ResourceWatchdogAction::KillGroup,
        max_concurrent_agent_processes: 7,
        agent_group_warn_private_bytes: 101,
        agent_group_kill_private_bytes: 202,
        agent_process_kill_private_bytes: 303,
        ..AppSettings::default()
    };

    for mode in ["sample", "warn", "killGroup", "tick", "quarantineRetry"] {
        let request = UiAutomationRequest {
            request_id: Uuid::new_v4().to_string(),
            token: "token".to_string(),
            window: BACKEND_AUTOMATION_WINDOW.to_string(),
            action: UiAutomationAction::Backend,
            selector: RESOURCE_WATCHDOG_BACKEND_SELECTOR.to_string(),
            value: Some(mode.to_string()),
            expires_at_unix_ms: Some(now_unix_ms() + 1_000),
        };

        let response =
            handle_resource_watchdog_backend_request_with_config(app.handle(), &request, &cfg)
                .await;

        assert!(response.ok);
        assert_eq!(
            response.target,
            Some(json!({
                "testId": RESOURCE_WATCHDOG_BACKEND_SELECTOR,
                "role": "backend",
                "state": "disabled",
                "tag": "backend",
                "text": format!(
                    "resource monitor watchdog {mode}: 0 group(s), 0 warn match(es), 0 kill match(es)"
                ),
                "visible": true,
                "disabled": false,
            }))
        );
        assert_eq!(
            response.diagnostics,
            Some(json!({
                "mode": mode,
                "configuredAction": cfg.resource_watchdog_action,
                "resourceMonitorEnabled": false,
                "killApplied": false,
                "limits": {
                    "maxConcurrentAgentGroups": 7,
                    "groupWarnPrivateBytes": 101,
                    "groupKillPrivateBytes": 202,
                    "processKillPrivateBytes": 303,
                },
                "snapshot": {
                    "overallState": "unknown",
                    "activeAgentGroups": 0,
                    "appPrivateBytes": null,
                    "warnings": [],
                },
                "decisions": [],
                "killResults": [],
                "quarantineRetries": [],
            }))
        );
    }

    assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.terminate_verified_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        backend.current_process_memory_calls.load(Ordering::SeqCst),
        0
    );
}

// #1151 (A3) - the automation `killResults` contract for the two split outcomes.
// `alreadyPending` keeps its value and type byte-identical so no existing automation
// breaks; `reason` is the additive discriminator that makes the split observable.
#[tokio::test]
async fn watchdog_kill_results_preserve_already_pending_contract() {
    use crate::pty::backend::SessionBackendKind;

    let backend = Arc::new(AutomationProcessBackend::default());
    let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
        backend.clone() as Arc<dyn ProcessTreeBackend>,
    ));
    let cfg = automation_settings();
    let limits = crate::resource_monitor::ResourceLimits::from(&cfg);

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let live = manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            "C:/automation-kill-results".to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap();

    // One group whose public row exists (the in-flight case) and one whose row never
    // did (the orphan case). Both are Running with private bytes over the kill limit,
    // so the hook produces a kill decision for each.
    let orphan_id = Uuid::new_v4();
    register_automation_group(&monitor, limits, live.id, 5_100);
    register_automation_group(&monitor, limits, orphan_id, 5_200);

    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(Arc::clone(&monitor))
        .manage(coordinator.clone())
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build kill-results automation app");
    coordinator.start(app.handle().clone()).unwrap();
    let restore = coordinator.submit_restore_first().await.unwrap();

    // Fill the queue so the first watchdog kill parks holding ONLY its critical key,
    // without ever enqueueing a job.
    let mut reservations = Vec::new();
    while let Ok(reservation) = coordinator.reserve_auto_close() {
        reservations.push(reservation);
    }
    let waiter = {
        let coordinator = coordinator.clone();
        tokio::spawn(async move { coordinator.watchdog_resource_kill(live.id).await })
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        while !coordinator
            .critical_key_registered_for_test(live.id, CriticalAdmissionKind::WatchdogKill)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the parked watchdog kill registers its critical key");

    assert!(matches!(
        coordinator.watchdog_resource_kill(live.id).await.unwrap(),
        WatchdogKillOutcome::AlreadyInFlight
    ));
    assert!(matches!(
        coordinator.watchdog_resource_kill(orphan_id).await.unwrap(),
        WatchdogKillOutcome::NoPublicSession
    ));

    let response = handle_resource_watchdog_backend_request_with_config(
        app.handle(),
        &watchdog_backend_request("killGroup"),
        &cfg,
    )
    .await;
    assert!(response.ok);
    let diagnostics = response.diagnostics.expect("watchdog diagnostics");
    let kill_results = diagnostics["killResults"]
        .as_array()
        .expect("killResults array")
        .clone();
    assert_eq!(kill_results.len(), 2);
    let entry_for = |session_id: Uuid| {
        kill_results
            .iter()
            .find(|entry| entry["sessionId"] == json!(session_id))
            .cloned()
            .unwrap_or_else(|| panic!("kill result for session {session_id}"))
    };
    assert_eq!(
        entry_for(live.id),
        json!({
            "ok": true,
            "sessionId": live.id,
            "alreadyPending": true,
            "reason": "alreadyInFlight",
        })
    );
    assert_eq!(
        entry_for(orphan_id),
        json!({
            "ok": true,
            "sessionId": orphan_id,
            "alreadyPending": true,
            "reason": "noPublicSession",
        })
    );

    restore.finish();
    coordinator.close_and_join().await;
    drop(reservations);
    let _ = waiter.await;
}

/// #1151 - registers a group, quarantines its cleanup on a stubborn child, and leaves
/// the retry backoff already satisfied. No public session row is ever created, which
/// is precisely what makes the group an orphan.
fn orphan_automation_group(
    monitor: &crate::resource_monitor::ResourceMonitorState,
    backend: &AutomationProcessBackend,
    limits: crate::resource_monitor::ResourceLimits,
    session_id: Uuid,
    root_pid: u32,
) -> ProcessIdentity {
    backend.mark_stubborn(AutomationProcessBackend::child_pid(root_pid));
    let root = register_automation_group(monitor, limits, session_id, root_pid);
    let quarantine = monitor
        .kill_group(
            session_id,
            crate::resource_monitor::types::ResourceKillReason::SessionDestroy,
        )
        .expect("first cleanup runs");
    assert!(quarantine.quarantined);
    monitor.test_backdate_quarantine_retry(session_id);
    root
}

async fn automation_app_with_coordinator(
    monitor: Arc<crate::resource_monitor::ResourceMonitorState>,
) -> (tauri::App<tauri::test::MockRuntime>, SelectionCoordinator) {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let coordinator = SelectionCoordinator::new(Arc::clone(&manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&manager))
        .manage(monitor)
        .manage(coordinator.clone())
        .manage(DetachedSessionsState::default())
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build quarantine-retry automation app");
    coordinator
        .start(app.handle().clone())
        .expect("start quarantine-retry coordinator");
    coordinator
        .submit_restore_first()
        .await
        .expect("restore-first admitted")
        .finish();
    (app, coordinator)
}

// #1151 (A1) - the deterministic native artifact for a successful reclaim. Everything
// is read from quarantineRetries[0], NEVER from diagnostics.snapshot, which is captured
// before the action loop and would still read activeAgentGroups = 1 on a correct build.
#[tokio::test]
async fn quarantine_retry_mode_reports_completed_orphan_cleanup() {
    let backend = Arc::new(AutomationProcessBackend::default());
    let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
        backend.clone() as Arc<dyn ProcessTreeBackend>,
    ));
    let cfg = automation_settings();
    let limits = crate::resource_monitor::ResourceLimits::from(&cfg);

    let session_id = Uuid::new_v4();
    let root = orphan_automation_group(&monitor, &backend, limits, session_id, 6_100);
    assert_eq!(monitor.active_agent_groups(), 1);
    // The blocker finally exits, which is the only thing a retry can discover.
    backend.mark_gone(AutomationProcessBackend::child_pid(6_100));

    let (app, coordinator) = automation_app_with_coordinator(Arc::clone(&monitor)).await;
    let response = handle_resource_watchdog_backend_request_with_config(
        app.handle(),
        &watchdog_backend_request("quarantineRetry"),
        &cfg,
    )
    .await;

    assert!(response.ok);
    let diagnostics = response.diagnostics.expect("watchdog diagnostics");
    let retries = diagnostics["quarantineRetries"]
        .as_array()
        .expect("quarantineRetries array");
    assert_eq!(retries.len(), 1);
    assert_eq!(retries[0]["sessionId"], json!(session_id));
    assert_eq!(retries[0]["path"], json!("orphan"));
    assert_eq!(retries[0]["rootPid"], json!(root.pid));
    assert_eq!(retries[0]["quarantined"], json!(false));
    assert_eq!(retries[0]["stillCountsTowardAdmission"], json!(false));
    assert_eq!(retries[0]["activeAgentGroups"], json!(0));
    // The pre-action snapshot deliberately still shows the group counted.
    assert_eq!(diagnostics["snapshot"]["activeAgentGroups"], json!(1));
    coordinator.close_and_join().await;
}

// #1151 (A2) - the safety artifact. An owned descendant that is still unverifiable
// keeps blocking capacity, and the report says so instead of claiming a reclaim.
#[tokio::test]
async fn quarantine_retry_mode_reports_still_blocked_group() {
    let backend = Arc::new(AutomationProcessBackend::default());
    let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
        backend.clone() as Arc<dyn ProcessTreeBackend>,
    ));
    let cfg = automation_settings();
    let limits = crate::resource_monitor::ResourceLimits::from(&cfg);

    let session_id = Uuid::new_v4();
    // The blocker is deliberately NOT marked gone.
    orphan_automation_group(&monitor, &backend, limits, session_id, 6_200);

    let (app, coordinator) = automation_app_with_coordinator(Arc::clone(&monitor)).await;
    let response = handle_resource_watchdog_backend_request_with_config(
        app.handle(),
        &watchdog_backend_request("quarantineRetry"),
        &cfg,
    )
    .await;

    let diagnostics = response.diagnostics.expect("watchdog diagnostics");
    let retries = diagnostics["quarantineRetries"]
        .as_array()
        .expect("quarantineRetries array");
    assert_eq!(retries.len(), 1);
    assert_eq!(retries[0]["path"], json!("orphan"));
    assert_eq!(retries[0]["quarantined"], json!(true));
    assert_eq!(retries[0]["stillCountsTowardAdmission"], json!(true));
    assert_eq!(retries[0]["activeAgentGroups"], json!(1));
    assert_eq!(monitor.active_agent_groups(), 1);
    coordinator.close_and_join().await;
}

// #1151 (A4) - an unsupported backend stays at zero admission and zero enforcement
// calls in the new mode too.
#[tokio::test]
async fn unsupported_backend_quarantine_retry_is_disabled() {
    let backend = Arc::new(UnsupportedAutomationBackend::default());
    let monitor = Arc::new(crate::resource_monitor::ResourceMonitorState::with_backend(
        backend.clone() as Arc<dyn ProcessTreeBackend>,
    ));
    let cfg = automation_settings();

    let (app, coordinator) = automation_app_with_coordinator(Arc::clone(&monitor)).await;
    let response = handle_resource_watchdog_backend_request_with_config(
        app.handle(),
        &watchdog_backend_request("quarantineRetry"),
        &cfg,
    )
    .await;

    assert!(response.ok);
    assert_eq!(response.target.expect("target")["state"], json!("disabled"));
    let diagnostics = response.diagnostics.expect("watchdog diagnostics");
    assert_eq!(diagnostics["quarantineRetries"], json!([]));
    assert_eq!(backend.observe_tree_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.observe_identity_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.terminate_verified_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        backend.current_process_memory_calls.load(Ordering::SeqCst),
        0
    );
    coordinator.close_and_join().await;
}
