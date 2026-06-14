use std::time::Duration;

use crate::config::settings::{ResourceWatchdogAction, SettingsState};
use crate::resource_monitor::types::{ResourceGroupState, ResourceKillReason, ResourceLimits};
use crate::resource_monitor::ResourceMonitorState;
use crate::shutdown::ShutdownSignal;

pub fn start(monitor: ResourceMonitorState, settings: SettingsState, shutdown: ShutdownSignal) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.token().cancelled() => break,
                _ = tokio::time::sleep(next_delay(&monitor, &settings).await) => {
                    run_tick(&monitor, &settings).await;
                }
            }
        }
    });
}

async fn next_delay(monitor: &ResourceMonitorState, settings: &SettingsState) -> Duration {
    let cfg = settings.read().await;
    if cfg.resource_backoff_polling && monitor.active_agent_groups() == 0 {
        Duration::from_secs(10)
    } else {
        Duration::from_secs(2)
    }
}

async fn run_tick(monitor: &ResourceMonitorState, settings: &SettingsState) {
    let cfg = settings.read().await.clone();
    if !cfg.resource_monitor_enabled {
        return;
    }
    let limits = ResourceLimits::from(&cfg);
    let groups = monitor.sample_for_watchdog(limits);
    if cfg.resource_watchdog_action != ResourceWatchdogAction::KillGroup {
        return;
    }
    for (session_id, group) in groups {
        if group.state != ResourceGroupState::Running {
            continue;
        }
        let group_over = group
            .private_bytes
            .is_some_and(|bytes| bytes >= limits.group_kill_private_bytes);
        let process_over = group.processes.iter().any(|process| {
            process
                .private_bytes
                .is_some_and(|bytes| bytes >= limits.process_kill_private_bytes)
        });
        if group_over || process_over {
            let monitor = monitor.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let _ = monitor.kill_group(session_id, ResourceKillReason::Watchdog);
            });
        }
    }
}
