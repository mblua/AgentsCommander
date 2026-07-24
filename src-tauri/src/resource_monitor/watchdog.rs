use std::time::Duration;

use crate::config::settings::{ResourceWatchdogAction, SettingsState};
use crate::resource_monitor::types::{ResourceGroupState, ResourceLimits};
use crate::resource_monitor::ResourceMonitorState;
use crate::session::selection::{CriticalAdmissionOutcome, SelectionCoordinator};
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

pub fn start(
    monitor: ResourceMonitorState,
    settings: SettingsState,
    coordinator: SelectionCoordinator,
    shutdown: ShutdownSignal,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        if !shutdown.wait_for_startup_commit().await {
            return;
        }
        loop {
            tokio::select! {
                _ = shutdown.token().cancelled() => break,
                _ = tokio::time::sleep(next_delay(&monitor, &settings).await) => {
                    run_tick(&monitor, &settings, &coordinator).await;
                }
            }
        }
    })
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
    for (session_id, group) in &groups {
        if group.state == ResourceGroupState::Quarantined
            && monitor.quarantine_retry_due(*session_id)
        {
            submit_watchdog_kill(coordinator, *session_id).await;
        }
    }

    if cfg.resource_watchdog_action != ResourceWatchdogAction::KillGroup {
        return;
    }
    for decision in evaluate_watchdog_groups(&groups, limits) {
        if decision.kill_required {
            submit_watchdog_kill(coordinator, decision.session_id).await;
        }
    }
}

async fn submit_watchdog_kill(coordinator: &SelectionCoordinator, session_id: Uuid) {
    match coordinator.watchdog_resource_kill(session_id).await {
        Ok(CriticalAdmissionOutcome::Completed(result)) => {
            log::info!(
                "[resource-watchdog] finalized session={} state={:?} finalized={}",
                session_id,
                result.state,
                result.finalized
            );
        }
        Ok(CriticalAdmissionOutcome::AlreadyPending) => {
            log::debug!(
                "[resource-watchdog] kill already pending or session no longer public session={}",
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
    use crate::resource_monitor::types::{
        ProcessIdentity, ResourceAgentGroupSnapshot, ResourceNetworkState, ResourceProcessSnapshot,
    };

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
}
