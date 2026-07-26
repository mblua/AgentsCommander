use std::time::Duration;

use crate::config::settings::{ResourceWatchdogAction, SettingsState};
use crate::resource_monitor::registry::{QuarantineRetryOutcome, QuarantineRetrySkip};
use crate::resource_monitor::types::{ProcessIdentity, ResourceGroupState, ResourceLimits};
use crate::resource_monitor::ResourceMonitorState;
use crate::session::selection::{SelectionCoordinator, WatchdogKillOutcome};
use crate::shutdown::ShutdownSignal;
use serde::Serialize;
use uuid::Uuid;

use super::types::ResourceAgentGroupSnapshot;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceWatchdogDecision {
    pub session_id: Uuid,
    pub name: String,
    pub state: ResourceGroupState,
    pub group_private_bytes: Option<u64>,
    pub group_warn: bool,
    pub group_kill: bool,
    pub process_kill: bool,
    pub process_kill_pids: Vec<u32>,
    pub warn_required: bool,
    pub kill_required: bool,
}

/// #1151 - which branch handled one quarantine retry. `Coordinator` means a live public
/// session took the normal coordinator plus Job Object path; `Orphan` means the session
/// row was gone and the registry-only fallback ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum QuarantineRetryPath {
    Coordinator,
    Orphan,
    AlreadyInFlight,
    Skipped,
    Error,
}

/// #1151 - structured, NON-CLAIMING report of one quarantine retry.
/// `still_counts_toward_admission` is read back from the registry after the call using
/// the admission predicate itself, so nothing here asserts "we reclaimed a slot"; it
/// states the observable fact and leaves authorship out of it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuarantineRetryReport {
    pub session_id: Uuid,
    pub root_pid: u32,
    pub path: QuarantineRetryPath,
    pub state: Option<ResourceGroupState>,
    pub quarantined: bool,
    pub still_counts_toward_admission: bool,
    pub active_agent_groups: usize,
    pub skipped: Option<&'static str>,
    pub message: Option<String>,
}

impl QuarantineRetryReport {
    fn new(session_id: Uuid, root_pid: u32, path: QuarantineRetryPath) -> Self {
        Self {
            session_id,
            root_pid,
            path,
            state: None,
            quarantined: false,
            still_counts_toward_admission: false,
            active_agent_groups: 0,
            skipped: None,
            message: None,
        }
    }
}

/// #1151 - only an explicit `NoPublicSession` may fall through to the registry-only
/// retry. `AlreadyInFlight` means another critical admission owns the outcome, and
/// treating it as an orphan is the exact mistake the issue rules out. Extracted so that
/// rule is directly unit-testable.
fn should_run_orphan_fallback(outcome: &WatchdogKillOutcome) -> bool {
    matches!(outcome, WatchdogKillOutcome::NoPublicSession)
}

pub fn start(
    monitor: ResourceMonitorState,
    settings: SettingsState,
    coordinator: SelectionCoordinator,
    shutdown: ShutdownSignal,
) {
    if !watchdog_eligible(&monitor) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.token().cancelled() => break,
                _ = tokio::time::sleep(next_delay(&monitor, &settings).await) => {
                    run_tick(&monitor, &settings, &coordinator).await;
                }
            }
        }
    });
}

fn watchdog_eligible(monitor: &ResourceMonitorState) -> bool {
    monitor.supports_process_tree_enforcement()
}

async fn next_delay(monitor: &ResourceMonitorState, settings: &SettingsState) -> Duration {
    let cfg = settings.read().await;
    if cfg.resource_backoff_polling && monitor.active_agent_groups() == 0 {
        Duration::from_secs(10)
    } else {
        Duration::from_secs(2)
    }
}

async fn run_tick(
    monitor: &ResourceMonitorState,
    settings: &SettingsState,
    coordinator: &SelectionCoordinator,
) {
    if !watchdog_eligible(monitor) {
        return;
    }
    let cfg = settings.read().await.clone();
    if !cfg.resource_monitor_enabled {
        return;
    }
    let limits = ResourceLimits::from(&cfg);
    let groups = monitor.sample_for_watchdog(limits);

    // #559 (H2) - retry cleanup for quarantined groups regardless of the configured
    // watchdog action; a leaked slot must be reclaimed even in Warn mode. kill_group
    // already re-observes and releases if the process is now gone. A double-dispatch
    // (a still-Quarantined group whose prior retry is mid-flight) is a no-op via
    // kill_group's Terminating/Terminated idempotency guard, so this relies on that
    // guard staying intact.
    //
    // #1151 - that guard now has TWO callers. The orphan fallback below reaches
    // kill_group_inner directly, WITHOUT the coordinator's WatchdogKill dedup key, and
    // relies on the same Terminating/Terminated transition for mutual exclusion. So
    // weakening kill_group_inner's state guard now breaks two callers, not one.
    //
    // run_tick discards the reports and logs nothing: retry_quarantined_group owns all
    // logging, or every orphan cleanup would emit its line twice.
    let _ = run_quarantine_retries(monitor, coordinator, &groups).await;

    if cfg.resource_watchdog_action != ResourceWatchdogAction::KillGroup {
        return;
    }
    for decision in evaluate_watchdog_groups(&groups, limits) {
        if decision.kill_required {
            submit_watchdog_kill(coordinator, decision.session_id).await;
        }
    }
}

/// #1151 - owns the quarantine-retry GATE and the loop, so no caller reimplements the
/// dispatch decision. `run_tick` calls it, and so does the automation hook, which is what
/// makes the automation evidence about shipped dispatch logic rather than a mirrored copy.
pub(crate) async fn run_quarantine_retries(
    monitor: &ResourceMonitorState,
    coordinator: &SelectionCoordinator,
    groups: &[(Uuid, ResourceAgentGroupSnapshot)],
) -> Vec<QuarantineRetryReport> {
    let mut reports = Vec::new();
    for (session_id, group) in groups {
        if group.state == ResourceGroupState::Quarantined
            && monitor.quarantine_retry_due(*session_id)
        {
            reports.push(
                retry_quarantined_group(monitor, coordinator, *session_id, group.root_identity)
                    .await,
            );
        }
    }
    reports
}

/// #1151 - the per-group decision. The coordinator is asked FIRST, so a live public
/// session always takes the coordinator plus Job Object path exactly as before. Only an
/// explicit `NoPublicSession` falls through to the registry-only retry.
pub(crate) async fn retry_quarantined_group(
    monitor: &ResourceMonitorState,
    coordinator: &SelectionCoordinator,
    session_id: Uuid,
    root_identity: ProcessIdentity,
) -> QuarantineRetryReport {
    let outcome = match coordinator.watchdog_resource_kill(session_id).await {
        Ok(outcome) => outcome,
        Err(error) => {
            // Busy or Unavailable means the app is bootstrapping, restoring or closing.
            // Acting on the registry then is out of policy and buys nothing: the next
            // tick retries.
            log::warn!("[resource-watchdog] coordinator kill failed session={session_id}: {error}");
            let mut report = QuarantineRetryReport::new(
                session_id,
                root_identity.pid,
                QuarantineRetryPath::Error,
            );
            report.message = Some(error);
            return report;
        }
    };

    if !should_run_orphan_fallback(&outcome) {
        return match outcome {
            WatchdogKillOutcome::Completed(result) => {
                log::info!(
                    "[resource-watchdog] finalized session={} state={:?} finalized={}",
                    session_id,
                    result.state,
                    result.finalized
                );
                let mut report = QuarantineRetryReport::new(
                    session_id,
                    root_identity.pid,
                    QuarantineRetryPath::Coordinator,
                );
                report.state = Some(result.state);
                report.quarantined = result.quarantined;
                report.still_counts_toward_admission =
                    monitor.group_counts_toward_admission(session_id);
                report.active_agent_groups = monitor.active_agent_groups();
                report.message = Some(result.message);
                report
            }
            WatchdogKillOutcome::AlreadyInFlight => {
                log::debug!("[resource-watchdog] kill already in flight session={session_id}");
                QuarantineRetryReport::new(
                    session_id,
                    root_identity.pid,
                    QuarantineRetryPath::AlreadyInFlight,
                )
            }
            // Unreachable: should_run_orphan_fallback is exactly this variant.
            WatchdogKillOutcome::NoPublicSession => QuarantineRetryReport::new(
                session_id,
                root_identity.pid,
                QuarantineRetryPath::Orphan,
            ),
        };
    }

    let monitor_c = monitor.clone();
    // #1151 - this spawn_blocking is the ONLY production invocation of
    // retry_orphaned_quarantine. kill_group can cost ~2s per stubborn PID
    // (windows.rs WaitForSingleObject), so inlining it here would stall a Tokio worker.
    // Awaited rather than detached: it keeps the loop sequential, keeps the log ordering,
    // and stops a later tick starting a second retry while the first is still running.
    let outcome = tokio::task::spawn_blocking(move || {
        monitor_c.retry_orphaned_quarantine(session_id, root_identity)
    })
    .await;

    match outcome {
        Err(join_error) => {
            log::warn!(
                "[resource-watchdog] orphan retry failed session={session_id}: {join_error}"
            );
            let mut report = QuarantineRetryReport::new(
                session_id,
                root_identity.pid,
                QuarantineRetryPath::Error,
            );
            report.message = Some(join_error.to_string());
            report
        }
        Ok(Err(message)) => {
            log::warn!("[resource-watchdog] orphan retry failed session={session_id}: {message}");
            let mut report = QuarantineRetryReport::new(
                session_id,
                root_identity.pid,
                QuarantineRetryPath::Error,
            );
            report.message = Some(message);
            report
        }
        Ok(Ok(QuarantineRetryOutcome::Skipped(reason))) => {
            log::debug!(
                "[resource-watchdog] orphan retry skipped session={} reason={}",
                session_id,
                reason.as_reason()
            );
            let mut report = QuarantineRetryReport::new(
                session_id,
                root_identity.pid,
                QuarantineRetryPath::Skipped,
            );
            if let QuarantineRetrySkip::NotQuarantined(state) = reason {
                report.state = Some(state);
            }
            report.skipped = Some(reason.as_reason());
            report.still_counts_toward_admission =
                monitor.group_counts_toward_admission(session_id);
            report.active_agent_groups = monitor.active_agent_groups();
            report
        }
        Ok(Ok(QuarantineRetryOutcome::Completed(result))) => {
            let still_counted = monitor.group_counts_toward_admission(session_id);
            let active_groups = monitor.active_agent_groups();
            if result.state == ResourceGroupState::Terminated
                && !result.quarantined
                && !still_counted
            {
                log::info!(
                    "[resource-watchdog] orphan cleanup completed session={session_id} root_pid={} state=Terminated still_counted=false active_groups={active_groups}",
                    root_identity.pid
                );
            } else if result.quarantined {
                // Deliberately warn: this carries the per-PID blocker text a native run
                // needs before any AV or EDR attribution.
                log::warn!(
                    "[resource-watchdog] orphan cleanup still blocked session={session_id} root_pid={} still_counted={still_counted}: {}",
                    root_identity.pid,
                    result.message
                );
            } else {
                log::debug!(
                    "[resource-watchdog] orphan cleanup no-op session={session_id} state={:?} still_counted={still_counted}: {}",
                    result.state,
                    result.message
                );
            }
            let mut report = QuarantineRetryReport::new(
                session_id,
                root_identity.pid,
                QuarantineRetryPath::Orphan,
            );
            report.state = Some(result.state);
            report.quarantined = result.quarantined;
            report.still_counts_toward_admission = still_counted;
            report.active_agent_groups = active_groups;
            report.message = Some(result.message);
            report
        }
    }
}

async fn submit_watchdog_kill(coordinator: &SelectionCoordinator, session_id: Uuid) {
    match coordinator.watchdog_resource_kill(session_id).await {
        Ok(WatchdogKillOutcome::Completed(result)) => {
            log::info!(
                "[resource-watchdog] finalized session={} state={:?} finalized={}",
                session_id,
                result.state,
                result.finalized
            );
        }
        // #1151 - the two conditions the old conflated line merged are now distinct, so a
        // log reader can tell an in-flight critical admission from a vanished session row.
        Ok(WatchdogKillOutcome::AlreadyInFlight) => {
            log::debug!(
                "[resource-watchdog] kill already in flight session={}",
                session_id
            );
        }
        Ok(WatchdogKillOutcome::NoPublicSession) => {
            log::debug!(
                "[resource-watchdog] session no longer public session={}",
                session_id
            );
        }
        Err(error) => {
            log::warn!(
                "[resource-watchdog] coordinator kill failed session={}: {}",
                session_id,
                error
            );
        }
    }
}

pub fn evaluate_watchdog_groups(
    groups: &[(Uuid, ResourceAgentGroupSnapshot)],
    limits: ResourceLimits,
) -> Vec<ResourceWatchdogDecision> {
    groups
        .iter()
        .filter_map(|(session_id, group)| {
            if group.state != ResourceGroupState::Running {
                return None;
            }
            let group_warn = group
                .private_bytes
                .is_some_and(|bytes| bytes >= limits.group_warn_private_bytes);
            let group_kill = group
                .private_bytes
                .is_some_and(|bytes| bytes >= limits.group_kill_private_bytes);
            let process_kill_pids = group
                .processes
                .iter()
                .filter(|process| {
                    process
                        .private_bytes
                        .is_some_and(|bytes| bytes >= limits.process_kill_private_bytes)
                })
                .map(|process| process.pid)
                .collect::<Vec<_>>();
            let process_kill = !process_kill_pids.is_empty();
            let kill_required = group_kill || process_kill;
            let warn_required = group_warn || kill_required;

            Some(ResourceWatchdogDecision {
                session_id: *session_id,
                name: group.name.clone(),
                state: group.state,
                group_private_bytes: group.private_bytes,
                group_warn,
                group_kill,
                process_kill,
                process_kill_pids,
                warn_required,
                kill_required,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::AppSettings;
    use crate::resource_monitor::registry::{
        ProcessTreeBackend, ResourceError, ResourceLaunchRegistration,
    };
    use crate::resource_monitor::types::{
        ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory,
        ResourceAgentGroupSnapshot, ResourceKillReason, ResourceKillResult, ResourceLaunchMetadata,
        ResourceNetworkState, ResourceProcessSnapshot, TerminateOutcome,
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

        fn observe_tree(
            &self,
            _root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
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
            network_state: ResourceNetworkState::Unknown,
            network_summary: "Socket attribution unavailable".to_string(),
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
        let settings = Arc::new(tokio::sync::RwLock::new(cfg));
        let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
        let coordinator = SelectionCoordinator::new(manager, CancellationToken::new());
        let shutdown = ShutdownSignal::new();

        let start_fn: fn(
            ResourceMonitorState,
            SettingsState,
            SelectionCoordinator,
            ShutdownSignal,
        ) = start;
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
        fn observe_tree(
            &self,
            root: ProcessIdentity,
        ) -> Result<ObservedProcessTree, ResourceError> {
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
    fn orphan_one_group(
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
        monitor.test_backdate_quarantine_retry(session_id);
        root
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
        let settings = Arc::new(tokio::sync::RwLock::new(cfg));

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

        let (_app, coordinator) =
            running_coordinator(Arc::clone(&manager), Arc::clone(&monitor)).await;
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
        let settings = Arc::new(tokio::sync::RwLock::new(cfg));

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
}
