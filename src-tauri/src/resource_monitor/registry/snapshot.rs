use crate::resource_monitor::types::ProcessMemory;
use crate::resource_monitor::types::ResourceAgentGroupSnapshot;
use crate::resource_monitor::types::ResourceGroupState;
use crate::resource_monitor::types::ResourceGroupWarning;
use crate::resource_monitor::types::ResourceGroupWarningLevel;
use crate::resource_monitor::types::ResourceLimits;
use crate::resource_monitor::types::ResourceOverallState;
use crate::resource_monitor::types::ResourceSnapshot;
use chrono::Utc;

pub(super) fn build_snapshot(
    groups: Vec<ResourceAgentGroupSnapshot>,
    active: usize,
    limits: ResourceLimits,
    app_memory: ProcessMemory,
    warnings: Vec<String>,
) -> ResourceSnapshot {
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
