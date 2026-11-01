use std::time::Duration;

use crate::config::settings::{ResourceWatchdogAction, SettingsState};
use crate::resource_monitor::registry::{QuarantineRetryOutcome, QuarantineRetrySkip};
use crate::resource_monitor::types::{ProcessIdentity, ResourceGroupState, ResourceLimits};
use crate::resource_monitor::ResourceMonitorState;
use crate::session::selection::{SelectionCoordinator, WatchdogKillOutcome};
#[cfg(test)]
use crate::shutdown::ShutdownSignal;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
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

    fn error(session_id: Uuid, root_pid: u32, message: String) -> Self {
        let mut report = Self::new(session_id, root_pid, QuarantineRetryPath::Error);
        report.message = Some(message);
        report
    }
}

/// #1151 - only an explicit `NoPublicSession` may fall through to the registry-only
/// retry. `AlreadyInFlight` means another critical admission owns the outcome, and
/// treating it as an orphan is the exact mistake the issue rules out. Extracted so that
/// rule is directly unit-testable.
fn should_run_orphan_fallback(outcome: &WatchdogKillOutcome) -> bool {
    matches!(outcome, WatchdogKillOutcome::NoPublicSession)
}

pub(crate) trait ResourceWatchdogTaskScope: Clone + Send + Sync + 'static {
    fn token(&self) -> &CancellationToken;
    fn spawn_owned(&self, future: futures::future::BoxFuture<'static, ()>);
}

pub(crate) fn start<S: ResourceWatchdogTaskScope>(
    monitor: ResourceMonitorState,
    settings: SettingsState,
    coordinator: SelectionCoordinator,
    shutdown: S,
) {
    if !watchdog_eligible(&monitor) {
        return;
    }
    let task_scope = shutdown.clone();
    shutdown.spawn_owned(Box::pin(async move {
        loop {
            tokio::select! {
                _ = task_scope.token().cancelled() => break,
                _ = tokio::time::sleep(next_delay(&monitor, &settings).await) => {
                    run_tick(&monitor, &settings, &coordinator).await;
                }
            }
        }
    }));
}

fn watchdog_eligible(monitor: &ResourceMonitorState) -> bool {
    monitor.supports_process_tree_enforcement()
}

async fn next_delay(monitor: &ResourceMonitorState, settings: &SettingsState) -> Duration {
    let cfg = match settings.current_read().await {
        Ok(cfg) => cfg,
        Err(_) => {
            log::debug!("[ResourceWatchdog] settings unavailable; retrying");
            return Duration::from_secs(2);
        }
    };
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
    let cfg = match settings.current_read().await {
        Ok(cfg) => cfg.clone(),
        Err(_) => {
            log::debug!("[ResourceWatchdog] settings unavailable; skipping enforcement");
            return;
        }
    };
    if !cfg.resource_monitor_enabled {
        return;
    }
    let limits = ResourceLimits::from(&cfg);
    let groups = monitor.sample_for_watchdog(limits);

    // #1151 - the kill_required loop is evaluated FIRST. Before this fix an orphan
    // quarantine retry returned immediately, so the quarantine loop was effectively free;
    // it now performs a real blocking kill_group per orphan, and running it first would
    // delay every memory-driven kill decision on a KillGroup install. The starvation is
    // introduced by this change, so mitigating it belongs to this change.
    //
    // #559 (H2) - the quarantine retry runs on EVERY configured action, including the
    // default Warn. The action gate is therefore SCOPED to the kill_required loop; it must
    // never be an early return from run_tick, which would delete the whole fix on a
    // default install while the app kept quarantining and kept leaking.
    if cfg.resource_watchdog_action == ResourceWatchdogAction::KillGroup {
        for decision in evaluate_watchdog_groups(&groups, limits) {
            if decision.kill_required {
                submit_watchdog_kill(coordinator, decision.session_id).await;
            }
        }
    }

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
    // Reordering is behavior-neutral on which groups are dispatched: evaluate_watchdog_groups
    // is Running-only and this loop is Quarantined-only, over the same immutable sample.
    //
    // run_tick discards the reports and logs nothing: retry_quarantined_group owns all
    // logging, or every orphan cleanup would emit its line twice.
    let _ = run_quarantine_retries(monitor, coordinator, &groups).await;
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
            return QuarantineRetryReport::error(session_id, root_identity.pid, error);
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

    retry_orphaned_group(monitor, session_id, root_identity).await
}

async fn retry_orphaned_group(
    monitor: &ResourceMonitorState,
    session_id: Uuid,
    root_identity: ProcessIdentity,
) -> QuarantineRetryReport {
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
            QuarantineRetryReport::error(session_id, root_identity.pid, join_error.to_string())
        }
        Ok(Err(message)) => {
            log::warn!("[resource-watchdog] orphan retry failed session={session_id}: {message}");
            QuarantineRetryReport::error(session_id, root_identity.pid, message)
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
                    "[resource-watchdog] orphan cleanup completed session={session_id} root_pid={} \
                     state=Terminated still_counted=false active_groups={active_groups}",
                    root_identity.pid
                );
            } else if result.quarantined {
                // Deliberately warn: this carries the per-PID blocker text a native run
                // needs before any AV or EDR attribution.
                log::warn!(
                    "[resource-watchdog] orphan cleanup still blocked session={session_id} \
                     root_pid={} still_counted={still_counted}: {}",
                    root_identity.pid,
                    result.message
                );
            } else {
                log::debug!(
                    "[resource-watchdog] orphan cleanup no-op session={session_id} state={:?} \
                     still_counted={still_counted}: {}",
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
mod tests;
