use crate::resource_monitor::registry::active_count;
use crate::resource_monitor::registry::ResourceAgentGroup;
use crate::resource_monitor::registry::ResourceMonitorInner;
use crate::resource_monitor::types::ProcessMemory;
use crate::resource_monitor::types::ResourceAgentGroupSnapshot;
use crate::resource_monitor::types::ResourceGroupState;
use crate::resource_monitor::types::ResourceGroupWarning;
use crate::resource_monitor::types::ResourceGroupWarningLevel;
use crate::resource_monitor::types::ResourceLimits;
use crate::resource_monitor::types::ResourceOverallState;
use crate::resource_monitor::types::ResourceProcessSnapshot;
use crate::resource_monitor::types::ResourceSnapshot;
use chrono::Utc;

pub(super) fn build_snapshot(
    inner: &ResourceMonitorInner,
    limits: ResourceLimits,
    app_memory: ProcessMemory,
    warnings: Vec<String>,
) -> ResourceSnapshot {
    let groups = inner
        .groups
        .values()
        .map(group_snapshot)
        .collect::<Vec<_>>();
    let mut group_warnings = Vec::new();
    for group in &groups {
        if let Some(bytes) = group.private_bytes {
            let level = if bytes >= limits.group_kill_private_bytes {
                Some((
                    ResourceGroupWarningLevel::Kill,
                    limits.group_kill_private_bytes,
                ))
            } else if bytes >= limits.group_warn_private_bytes {
                Some((
                    ResourceGroupWarningLevel::Warn,
                    limits.group_warn_private_bytes,
                ))
            } else {
                None
            };
            if let Some((level, limit_bytes)) = level {
                group_warnings.push(ResourceGroupWarning {
                    session_id: group.session_id.clone(),
                    level,
                    private_bytes: bytes,
                    limit_bytes,
                });
            }
        }
    }
    let active = active_count(inner);
    let has_quarantine = groups
        .iter()
        .any(|group| group.state == ResourceGroupState::Quarantined);
    let has_terminating = groups
        .iter()
        .any(|group| group.state == ResourceGroupState::Terminating);
    let has_critical = groups.iter().any(|group| {
        group
            .private_bytes
            .is_some_and(|bytes| bytes >= limits.group_kill_private_bytes)
            || group.processes.iter().any(|process| {
                process
                    .private_bytes
                    .is_some_and(|bytes| bytes >= limits.process_kill_private_bytes)
            })
    });
    let overall_state = if has_quarantine
        || has_terminating
        || active >= limits.max_concurrent_agent_processes as usize
    {
        ResourceOverallState::Enforcing
    } else if has_critical {
        ResourceOverallState::Critical
    } else if !warnings.is_empty() || !group_warnings.is_empty() {
        ResourceOverallState::Warn
    } else {
        ResourceOverallState::Ok
    };

    ResourceSnapshot {
        captured_at: Utc::now(),
        overall_state,
        monitor_enabled: limits.monitor_enabled,
        active_agent_groups: active,
        max_concurrent_agent_groups: limits.max_concurrent_agent_processes,
        app_private_bytes: app_memory.private_bytes,
        app_working_set_bytes: app_memory.working_set_bytes,
        groups,
        warnings,
        group_warnings,
    }
}

pub(super) fn group_snapshot(group: &ResourceAgentGroup) -> ResourceAgentGroupSnapshot {
    let mut processes = group
        .observed_processes
        .values()
        .map(ResourceProcessSnapshot::from)
        .collect::<Vec<_>>();
    processes.sort_by_key(|process| (process.name.clone(), process.pid));
    let private_bytes = sum_optional(processes.iter().map(|p| p.private_bytes));
    let working_set_bytes = sum_optional(processes.iter().map(|p| p.working_set_bytes));
    ResourceAgentGroupSnapshot {
        session_id: group.session_id.to_string(),
        name: group
            .agent_label
            .clone()
            .or_else(|| group.agent_id.clone())
            .unwrap_or_else(|| group.name.clone()),
        workgroup: group.workgroup.clone(),
        agent: group.agent.clone(),
        project: group.project.clone(),
        root_pid: group.root_identity.pid,
        root_identity: group.root_identity,
        state: group.state,
        descendants_observed: group.descendants_observed,
        process_count: processes.len(),
        private_bytes,
        working_set_bytes,
        cpu_percent: None,
        processes,
        kill_allowed: matches!(
            group.state,
            ResourceGroupState::Running | ResourceGroupState::Terminating
        ),
        last_error: group.last_error.clone(),
    }
}

pub(super) fn disabled_snapshot(limits: ResourceLimits) -> ResourceSnapshot {
    ResourceSnapshot {
        captured_at: Utc::now(),
        overall_state: ResourceOverallState::Unknown,
        monitor_enabled: false,
        active_agent_groups: 0,
        max_concurrent_agent_groups: limits.max_concurrent_agent_processes,
        app_private_bytes: None,
        app_working_set_bytes: None,
        groups: Vec::new(),
        warnings: Vec::new(),
        group_warnings: Vec::new(),
    }
}

pub(super) fn sum_optional(values: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    let mut any = false;
    let mut total = 0_u64;
    for value in values.flatten() {
        any = true;
        total = total.saturating_add(value);
    }
    any.then_some(total)
}
