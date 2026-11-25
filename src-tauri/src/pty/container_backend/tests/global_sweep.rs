use super::*;
use super::{
    assert_runtime_owner_retained, backend_with_runtime, insert_runtime_handle,
    production_route_remover_fixture, SweepDriver,
};
#[test]
fn blocked_global_sweep_route_remover_returns_at_deadline_with_retained_owner() {
    let runtime = Arc::new(RecordingRuntime::default());
    let backend = Arc::new(backend_with_runtime(runtime.clone()));
    let session_id = Uuid::new_v4();
    let _receiver = insert_runtime_handle(&backend, session_id);

    let route_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&route_calls);
    let (route_entered, route_entered_rx) = std::sync::mpsc::channel();
    let (route_release, route_release_rx) = std::sync::mpsc::channel();
    let route_release_rx = Arc::new(Mutex::new(route_release_rx));
    let release = Arc::clone(&route_release_rx);
    backend.set_route_remover(Arc::new(move |removed_session, _| {
        assert_eq!(removed_session, session_id);
        calls.fetch_add(1, Ordering::SeqCst);
        route_entered
            .send(())
            .expect("publish blocked route-remover entry");
        release
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recv()
            .ok();
        Ok(())
    }));

    let driver = SweepDriver::new(&["epochOpened", "drainPoll"]);
    let _rescue = Rescue(Arc::clone(&driver));
    let report = sweep_at_deadline(&driver, &backend, |_| {
        route_entered_rx
            .recv_timeout(WATCHDOG)
            .expect("global sweep enters the deterministic blocked route remover");
    });

    assert!(!report.terminal, "retained={:?}", report.retained);
    assert_runtime_owner_retained(&backend, &runtime, session_id);
    assert_eq!(route_calls.load(Ordering::SeqCst), 1);
    assert!(
        report.retained.iter().any(|context| {
            context.contains("owner=containerCleanup")
                && context.contains(&format!("session={session_id}"))
                && context.contains("runtimeHandle=true")
                && context.contains("state=inFlight")
                && context.contains("inFlight=true")
        }),
        "retained={:?}",
        report.retained
    );
    assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

    route_release.send(()).expect("release route remover owner");
    let cleanup_deadline = Instant::now() + Duration::from_secs(1);
    while runtime.stopped() != vec![session_id] {
        assert!(
            Instant::now() < cleanup_deadline,
            "retained cleanup did not finish after route-remover release"
        );
        std::thread::yield_now();
    }
    while !backend
        .retained_runtime_cleanup_sessions_for_test()
        .is_empty()
    {
        assert!(
            Instant::now() < cleanup_deadline,
            "retained ownership did not become terminal after release"
        );
        std::thread::yield_now();
    }
    assert_eq!(route_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn poisoned_global_sweep_route_removal_is_structured_residue() {
    let runtime = Arc::new(RecordingRuntime::default());
    let backend = backend_with_runtime(runtime.clone());
    let session_id = Uuid::new_v4();
    let _receiver = insert_runtime_handle(&backend, session_id);
    backend.set_route_remover(Arc::new(|_, _| {
        Err(RouteRemovalError::LockPoisoned("ptyManager"))
    }));

    // 50 ms could end before the remover ran once (#1253); 1 s is the run time.
    let report = backend.stop_all_started_containers_blocking(Duration::from_secs(1));

    assert!(!report.terminal, "retained={:?}", report.retained);
    assert_runtime_owner_retained(&backend, &runtime, session_id);
    assert!(
        report.retained.iter().any(|context| {
            context.contains("owner=containerCleanup")
                && context.contains(&format!("session={session_id}"))
                && context.contains("runtimeHandle=true")
                && context.contains("route removal lock poisoned owner=ptyManager")
        }),
        "retained={:?}",
        report.retained
    );
    assert!(!crate::shutdown_persistence_allowed(true, report.terminal));
}

#[test]
fn docker_command_owner_stays_visible_during_real_global_sweep_retry() {
    let driver = SweepDriver::new(&["epochOpened", "drainPoll"]);
    let _rescue = Rescue(Arc::clone(&driver));
    let mut docker = crate::pty::docker_runtime::DockerRuntime::new();
    let (now, wait) = (Arc::clone(&driver), Arc::clone(&driver));
    docker.set_retry_clock_for_test(
        Arc::new(move || now.now()),
        Arc::new(move |_| wait.wait("dockerRetry")),
    );
    let runtime = Arc::new(docker);
    let readers = runtime.retain_blocked_command_readers_for_test(
        "global-sweep-reader",
        "docker-reader-fixture",
        "reader cleanup failed OPENAI_API_KEY=sk-proj-global-sweep-secret\nnested failure",
    );
    assert_eq!(runtime.active_reader_count(), 2);
    let retained_before = runtime.retained_cleanup_contexts();
    assert_eq!(
        retained_before.len(),
        1,
        "one command owner before the sweep"
    );
    let (retry_entered, retry_release) = runtime.install_command_retry_gate_for_test();
    let backend = Arc::new(backend_with_runtime(runtime.clone()));
    let report = sweep_at_deadline(&driver, &backend, |_| {
        retry_entered
            .recv_timeout(WATCHDOG)
            .expect("production runtime retry publishes its in-flight transition");
    });

    assert!(!report.terminal, "retained={:?}", report.retained);
    let expected = "owner=dockerCommand session=none reason=global-sweep-reader \
        program=docker-reader-fixture runtimeHandle=none state=inFlight inFlight=true \
        lastError=reader cleanup failed OPENAI_API_KEY=[REDACTED]\\nnested failure";
    assert_eq!(
        report
            .retained
            .iter()
            .filter(|context| context.as_str() == expected)
            .count(),
        1,
        "retained={:?}",
        report.retained
    );
    assert!(report.retained.iter().all(|context| {
        !context.contains("sk-proj-global-sweep-secret")
            && !context.contains("00000000-0000-0000-0000-000000000000")
    }));
    let live_contexts = runtime.retained_cleanup_contexts();
    assert_eq!(live_contexts.len(), 1);
    assert_eq!(live_contexts[0].owner, "dockerCommand");
    assert_eq!(live_contexts[0].session_id, None);
    assert_eq!(live_contexts[0].reason, "global-sweep-reader");
    assert_eq!(
        live_contexts[0].program.as_deref(),
        Some("docker-reader-fixture")
    );
    assert_eq!(live_contexts[0].state, "inFlight");
    assert!(live_contexts[0].in_flight);
    assert_eq!(runtime.active_reader_count(), 2);
    assert!(!crate::shutdown_persistence_allowed(true, report.terminal));

    assert_combined_with_selection_fixtures(expected, &report.retained);

    retry_release
        .send(())
        .expect("release the deterministic in-flight retry gate");
    wait_for_docker_owner_retained(&runtime);
    let retained_after = runtime.retained_cleanup_contexts();
    assert_eq!(retained_after[0].last_error, retained_before[0].last_error);
    assert_eq!(runtime.active_reader_count(), 2);
    readers.release();
    let readers_limit = Instant::now() + WATCHDOG;
    while runtime.active_reader_count() != 0 {
        assert!(
            Instant::now() < readers_limit,
            "released readers did not exit"
        );
        std::thread::yield_now();
    }
    let retry_control = ContainerRuntimeControl::default();
    retry_control.request_shutdown(driver.now() + Duration::from_secs(1));
    runtime.retry_retained_cleanups(&retry_control);
    assert!(runtime.retained_cleanup_contexts().is_empty());
    assert_eq!(runtime.active_reader_count(), 0);

    let terminal = backend.stop_all_started_containers_blocking(Duration::from_secs(1));
    assert!(terminal.terminal, "retained={:?}", terminal.retained);
    assert!(terminal.retained.is_empty());
    assert!(crate::shutdown_persistence_allowed(true, terminal.terminal));
}

fn assert_combined_with_selection_fixtures(expected: &str, report_retained: &[String]) {
    let mut selection_retained = (0..300)
        .map(|index| {
            format!(
                "owner=selectionFixture session=none reason=fixture-{index:03} program=none \
                    runtimeHandle=none state=retained inFlight=false"
            )
        })
        .collect::<Vec<_>>();
    selection_retained.push(expected.to_string());
    let mut uncapped = selection_retained
        .iter()
        .map(|context| {
            crate::pty::container_runtime::normalize_retained_owner_diagnostic("selection", context)
        })
        .chain(report_retained.iter().map(|context| {
            crate::pty::container_runtime::normalize_retained_owner_diagnostic(
                "containerShutdown",
                context,
            )
        }))
        .collect::<Vec<_>>();
    uncapped.sort();
    uncapped.dedup();
    let combined =
        crate::combined_shutdown_retained_diagnostics(selection_retained, report_retained.to_vec());
    let expected_omitted = uncapped.len() - (RETAINED_OWNER_REPORT_CAPACITY - 1);
    assert_eq!(combined.len(), RETAINED_OWNER_REPORT_CAPACITY);
    assert_eq!(
        combined
            .iter()
            .filter(|context| context.as_str() == expected)
            .count(),
        1,
        "combined={combined:?}"
    );
    assert_eq!(
        combined.last().expect("global omitted-count sentinel"),
        &format!(
            "owner=diagnosticSummary session=none reason=retained-owner-report-truncated \
                program=none runtimeHandle=none state=retained inFlight=false \
                omittedCount={expected_omitted}"
        )
    );
}

fn wait_for_docker_owner_retained(runtime: &crate::pty::docker_runtime::DockerRuntime) {
    let transition_deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let contexts = runtime.retained_cleanup_contexts();
        if contexts
            .first()
            .is_some_and(|context| context.state == "retained" && !context.in_flight)
        {
            break;
        }
        assert!(
            Instant::now() < transition_deadline,
            "Docker command owner did not return to retained state after retry release: \
                {contexts:?}"
        );
        std::thread::yield_now();
    }
}

fn high_cardinality_runtime_contexts(runtime_entries: usize) -> Vec<RetainedContainerOwnerContext> {
    (0..runtime_entries)
        .map(|index| {
            if index % 2 == 0 {
                RetainedContainerOwnerContext {
                    owner: "ambiguousStartCleanup",
                    session_id: Some(Uuid::from_u128(10_000 + index as u128)),
                    reason: "ambiguous-start".to_string(),
                    program: None,
                    runtime_handle: Some(true),
                    state: "retained",
                    in_flight: false,
                    last_error: None,
                }
            } else {
                RetainedContainerOwnerContext {
                    owner: "dockerCommand",
                    session_id: None,
                    reason: "docker-command".to_string(),
                    program: Some(format!("docker-reader-{index}")),
                    runtime_handle: None,
                    state: "retained",
                    in_flight: false,
                    last_error: None,
                }
            }
        })
        .collect()
}

const INSTALLED_ENTRIES: usize = 180;
const RUNTIME_ENTRIES: usize = 180;

fn high_cardinality_backend() -> ContainerTransportBackend {
    let runtime_contexts = high_cardinality_runtime_contexts(RUNTIME_ENTRIES);
    let runtime = Arc::new(RetainedContextRuntime {
        contexts: runtime_contexts,
    });
    let backend = backend_with_runtime(runtime);
    for index in 0..INSTALLED_ENTRIES {
        let session_id = Uuid::from_u128(20_000 + index as u128);
        let _receiver = insert_runtime_handle(&backend, session_id);
    }
    backend
}

#[test]
fn high_cardinality_retained_scheduling_obeys_the_advertised_deadline() {
    let backend = high_cardinality_backend();
    let budget = Duration::ZERO;
    let started_at = Instant::now();
    let report = backend.stop_all_started_containers_blocking(budget);
    let elapsed = started_at.elapsed();

    assert!(
        elapsed <= Duration::from_millis(250),
        "high-cardinality scheduling exceeded its advertised deadline: {elapsed:?}"
    );
    assert_high_cardinality_report(report);
}

/// Causal companion: at a fixed `D = origin` the transfer and drain cuts fire before any
/// ownership transfer, epoch start, work or wait.
#[test]
fn high_cardinality_retained_scheduling_cuts_at_the_logical_deadline() {
    let backend = high_cardinality_backend();
    let driver = SweepDriver::new(&[]);
    let clock = driver.clock();
    let report = backend.stop_all_started_containers_blocking_with_clock(Duration::ZERO, &clock);

    assert_eq!(driver.probes(SweepProbe::TransferCut), 1);
    let probes = driver.probes.lock().unwrap().clone();
    let transfers = probes
        .iter()
        .filter(|mark| matches!(mark, SweepProbe::TransferAttempt(_)));
    assert_eq!(transfers.count(), 0, "{probes:?}");
    assert_eq!(driver.probes(SweepProbe::DrainCut), 1);
    assert_eq!(driver.probes(SweepProbe::EpochStartEntered), 0);
    assert_eq!(driver.waits.load(Ordering::SeqCst), 0);
    assert_eq!(backend.shutdown_work.observation(), (false, 0, 0, 0));
    assert!(backend.cleanup_ownership.entries().is_empty());
    assert_high_cardinality_report(report);
}

fn assert_high_cardinality_report(report: ContainerShutdownReport) {
    assert!(!report.terminal);
    assert_eq!(
        report
            .retained
            .iter()
            .filter(|context| context.contains("owner=installedContainer "))
            .count(),
        INSTALLED_ENTRIES
    );
    assert_eq!(
        report
            .retained
            .iter()
            .filter(|context| {
                context.contains("owner=ambiguousStartCleanup ")
                    || context.contains("owner=dockerCommand ")
            })
            .count(),
        RUNTIME_ENTRIES
    );

    let selection_retained = vec![
        "reason=selection-coordinator state=retained".to_string(),
        "owner=pendingCreate session=none reason=create state=retained".to_string(),
    ];
    let mut uncapped = selection_retained
        .iter()
        .map(|context| {
            crate::pty::container_runtime::normalize_retained_owner_diagnostic("selection", context)
        })
        .chain(report.retained.iter().map(|context| {
            crate::pty::container_runtime::normalize_retained_owner_diagnostic(
                "containerShutdown",
                context,
            )
        }))
        .collect::<Vec<_>>();
    uncapped.sort();
    uncapped.dedup();
    assert!(uncapped
        .iter()
        .any(|context| context.starts_with("owner=selection ")));
    assert!(uncapped
        .iter()
        .any(|context| context.starts_with("owner=installedContainer ")));
    assert!(uncapped
        .iter()
        .any(|context| context.starts_with("owner=dockerCommand ")));
    assert!(uncapped
        .iter()
        .any(|context| context.starts_with("owner=ambiguousStartCleanup ")));

    let retained =
        crate::combined_shutdown_retained_diagnostics(selection_retained, report.retained);
    let expected_omitted = uncapped.len() - (RETAINED_OWNER_REPORT_CAPACITY - 1);
    assert_eq!(retained.len(), RETAINED_OWNER_REPORT_CAPACITY);
    assert_eq!(
        retained.last().expect("omitted-count sentinel"),
        &format!(
            "owner=diagnosticSummary session=none reason=retained-owner-report-truncated \
                program=none runtimeHandle=none state=retained inFlight=false \
                omittedCount={expected_omitted}"
        )
    );
}

type RetainedEntry = Arc<super::super::retained_cleanup::RetainedContainerCleanup>;

/// Real poisons of the outer PTY mutex and of the route registry, with their exact text.
const REAL_POISONS: [(bool, &str); 2] = [
    (
        false,
        "route removal failed: route removal lock poisoned owner=ptyManager",
    ),
    (
        true,
        "route removal failed: route removal lock poisoned owner=ptyRouteRegistry",
    ),
];

struct ClockedSweepFixture {
    backend: Arc<ContainerTransportBackend>,
    pty_manager: Arc<Mutex<PtyManager>>,
    session_id: Uuid,
    runtime: Arc<RecordingRuntime>,
    entry: RetainedEntry,
    control: ContainerRuntimeControl,
    deadline: Instant,
    _receiver: mpsc::Receiver<HostToBridgeFrame>,
}

impl ClockedSweepFixture {
    /// Retains a real session that requires the installed production route remover.
    fn new() -> Self {
        let runtime = Arc::new(RecordingRuntime::default());
        let (backend, pty_manager, session_id, receiver) =
            production_route_remover_fixture(Arc::clone(&runtime));
        let resources = backend
            .remove_session_state(session_id)
            .expect("installed session resources");
        let entry = backend.retain_removed_resources(resources, "global-shutdown-sweep");
        let remover = backend.route_remover_until(Instant::now() + Duration::from_secs(1));
        assert!(
            matches!(remover, Ok(Some(_))),
            "production remover is installed"
        );
        entry.require_route_removal(remover);
        let control = ContainerRuntimeControl::default();
        let deadline = Instant::now() + Duration::from_secs(60);
        control.request_shutdown(deadline);
        Self {
            backend,
            pty_manager,
            session_id,
            runtime,
            entry,
            control,
            deadline,
            _receiver: receiver,
        }
    }

    fn attempt(&self, epoch: u64, now: &impl Fn() -> Instant) {
        self.backend.cleanup_ownership.attempt_with_clock(
            Arc::clone(&self.entry),
            epoch,
            &self.control,
            now,
        );
    }

    fn before_deadline(&self) -> Instant {
        self.deadline - Duration::from_nanos(1)
    }

    /// Records the real poison through the production remover; never seeded.
    fn record_real_poison(&self, epoch: u64, (registry, error): (bool, &str)) -> String {
        if registry {
            self.pty_manager
                .lock()
                .unwrap()
                .poison_route_registry_for_test();
        } else {
            let poison_manager = Arc::clone(&self.pty_manager);
            let poison = std::thread::spawn(move || {
                let _manager_guard = poison_manager.lock().unwrap();
                panic!("poison the real outer PTY manager mutex");
            });
            assert!(
                poison.join().is_err(),
                "outer mutex poison fixture must panic"
            );
        }
        let before = self.before_deadline();
        self.attempt(epoch, &|| before);
        assert_eq!(self.last_error().as_deref(), Some(error));
        let context = self.context();
        assert!(
            context.ends_with(&format!("lastError={error}")),
            "{context}"
        );
        context
    }

    fn last_error(&self) -> Option<String> {
        self.entry.attempt.lock().unwrap().last_error.clone()
    }

    fn attempt_state(&self) -> (bool, Option<u64>) {
        let attempt = self.entry.attempt.lock().unwrap();
        (attempt.in_flight, attempt.last_epoch)
    }

    fn context(&self) -> String {
        let contexts = self.backend.retained_cleanup_contexts_for_test();
        assert_eq!(contexts.len(), 1, "{contexts:?}");
        contexts[0].clone()
    }

    fn assert_retained(&self, epoch: u64) {
        assert_eq!(self.attempt_state(), (false, Some(epoch)));
        assert_eq!(
            self.backend.retained_runtime_cleanup_sessions_for_test(),
            vec![self.session_id]
        );
        assert!(self.entry.non_runtime.lock().unwrap().is_some());
        assert!(self.runtime.stopped().is_empty());
        let manager = self
            .pty_manager
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            manager.backend_kind(self.session_id),
            Some(SessionBackendKind::ContainerTransport)
        );
    }

    fn assert_cleaned_after_retry(&self, epoch: u64) {
        self.pty_manager.clear_poison();
        let manager = self.pty_manager.lock().unwrap();
        manager.clear_route_registry_poison_for_test();
        drop(manager);
        let before = self.before_deadline();
        self.attempt(epoch, &|| before);
        assert_eq!(self.runtime.stopped(), vec![self.session_id]);
        assert_eq!(self.last_error(), None);
        assert!(self.backend.retained_cleanup_sessions_for_test().is_empty());
        assert_eq!(
            self.pty_manager
                .lock()
                .unwrap()
                .backend_kind(self.session_id),
            None
        );
    }

    /// Replaces the pending remover with a fake that returns `error` and records its calls.
    fn install_counting_remover(
        &self,
        error: RouteRemovalError,
    ) -> Arc<Mutex<Vec<(Uuid, Instant)>>> {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        self.entry
            .require_route_removal(Ok(Some(Arc::new(move |session_id, deadline| {
                recorded.lock().unwrap().push((session_id, deadline));
                Err(error.clone())
            }))));
        calls
    }
}

type ClockObservation = (bool, Option<u64>, Instant);

/// Clock that records each call and the attempt state seen at that moment.
fn observing_clock<'a>(
    entry: &'a RetainedEntry,
    returns: Instant,
    observed: &'a Mutex<Vec<ClockObservation>>,
) -> impl Fn() -> Instant + 'a {
    move || {
        let attempt = entry.attempt.lock().unwrap();
        observed
            .lock()
            .unwrap()
            .push((attempt.in_flight, attempt.last_epoch, returns));
        returns
    }
}

#[test]
fn global_sweep_pre_remover_expiry_preserves_real_poison_error() {
    for poison in REAL_POISONS {
        pre_remover_expiry_preserves(poison);
    }
}

fn pre_remover_expiry_preserves(poison: (bool, &str)) {
    let fixture = ClockedSweepFixture::new();
    let context = fixture.record_real_poison(1, poison);
    let admission = fixture.before_deadline();
    assert!(admission < fixture.deadline);
    let observed = Mutex::new(Vec::new());
    fixture.attempt(
        2,
        &observing_clock(&fixture.entry, fixture.deadline, &observed),
    );
    let observed = observed.into_inner().unwrap();
    assert_eq!(observed, vec![(true, Some(2), fixture.deadline)]);
    assert_eq!(
        [admission, observed[0].2],
        [fixture.before_deadline(), fixture.deadline]
    );
    assert_eq!(fixture.last_error().as_deref(), Some(poison.1));
    assert_eq!(fixture.context(), context);
    fixture.assert_retained(2);

    fixture.assert_cleaned_after_retry(3);
}

#[test]
fn global_sweep_invoked_deadline_preserves_real_poison_error() {
    for poison in REAL_POISONS {
        invoked_deadline_preserves(poison);
    }
}

fn invoked_deadline_preserves(poison: (bool, &str)) {
    let fixture = ClockedSweepFixture::new();
    let context = fixture.record_real_poison(1, poison);
    let production = fixture
        .backend
        .route_remover_until(Instant::now() + Duration::from_secs(1));

    let calls = fixture.install_counting_remover(RouteRemovalError::Deadline("ptyManager"));
    let before = fixture.before_deadline();
    fixture.attempt(2, &|| before);

    assert_eq!(
        *calls.lock().unwrap(),
        vec![(fixture.session_id, fixture.deadline)]
    );
    assert_eq!(fixture.last_error().as_deref(), Some(poison.1));
    assert_eq!(fixture.context(), context);
    fixture.assert_retained(2);

    fixture.entry.require_route_removal(production);
    fixture.assert_cleaned_after_retry(3);
}

#[test]
fn global_sweep_deadline_history_fallbacks_and_replacements() {
    // Pre-remover expiry without history: remover never runs, deadline text is fallback.
    let fixture = ClockedSweepFixture::new();
    let calls = fixture.install_counting_remover(RouteRemovalError::Deadline("ptyManager"));
    let expired = fixture.deadline;
    fixture.attempt(1, &|| expired);
    let pre_remover = "route removal deadline reached owner=globalSweep";
    assert_eq!(fixture.last_error().as_deref(), Some(pre_remover));
    assert!(calls.lock().unwrap().is_empty());
    // A later expired attempt keeps its own deadline history too.
    fixture.attempt(2, &|| expired);
    assert_eq!(fixture.last_error().as_deref(), Some(pre_remover));
    assert!(calls.lock().unwrap().is_empty());
    fixture.assert_retained(2);

    // Invoked deadline without history: wrapped remover text is fallback.
    let fixture = ClockedSweepFixture::new();
    let calls = fixture.install_counting_remover(RouteRemovalError::Deadline("ptyManager"));
    let before = fixture.before_deadline();
    fixture.attempt(1, &|| before);
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.last_error().as_deref(),
        Some("route removal failed: route removal deadline reached owner=ptyManager")
    );

    // Invoked non-deadline remover errors replace history.
    fixture.install_counting_remover(RouteRemovalError::LockPoisoned("ptyRouteRegistry"));
    fixture.attempt(2, &|| before);
    assert_eq!(
        fixture.last_error().as_deref(),
        Some("route removal failed: route removal lock poisoned owner=ptyRouteRegistry")
    );

    // Ownership failures precede the clock and replace history.
    let clock_calls = AtomicUsize::new(0);
    let counted = || {
        clock_calls.fetch_add(1, Ordering::SeqCst);
        expired
    };
    fixture
        .entry
        .require_route_removal(Err(RouteRemovalError::LockPoisoned("routeRemoverRegistry")));
    fixture.attempt(3, &counted);
    assert_eq!(
        fixture.last_error().as_deref(),
        Some("route removal unavailable: route removal lock poisoned owner=routeRemoverRegistry")
    );
    assert_eq!(clock_calls.load(Ordering::SeqCst), 0);
    fixture.assert_retained(3);
}

#[test]
fn global_sweep_rejected_or_routeless_attempts_skip_the_clock() {
    // Missing global-sweep deadline is a real failure reported before the clock.
    let fixture = ClockedSweepFixture::new();
    let clock_calls = AtomicUsize::new(0);
    let expired = fixture.deadline;
    let counted = || {
        clock_calls.fetch_add(1, Ordering::SeqCst);
        expired
    };
    let no_deadline = ContainerRuntimeControl::default();
    fixture.backend.cleanup_ownership.attempt_with_clock(
        Arc::clone(&fixture.entry),
        1,
        &no_deadline,
        &counted,
    );
    assert_eq!(
        fixture.last_error().as_deref(),
        Some("route removal missing global-sweep deadline")
    );

    // Duplicate epoch and in-flight attempts are rejected before the clock.
    fixture.attempt(1, &counted);
    fixture.entry.attempt.lock().unwrap().in_flight = true;
    fixture.attempt(2, &counted);
    assert_eq!(fixture.attempt_state(), (true, Some(1)));
    fixture.entry.attempt.lock().unwrap().in_flight = false;
    assert_eq!(clock_calls.load(Ordering::SeqCst), 0);

    // NotRequired bypasses the clock and finishes cleanup.
    fixture.entry.require_route_removal(Ok(None));
    fixture.attempt(3, &counted);
    assert_eq!(clock_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.runtime.stopped(), vec![fixture.session_id]);
    assert!(fixture
        .backend
        .retained_cleanup_sessions_for_test()
        .is_empty());
}
