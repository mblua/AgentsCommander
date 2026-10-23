use super::{
    UiAutomationAction, UiAutomationRequest, UiAutomationResponse,
    RESOURCE_WATCHDOG_BACKEND_SELECTOR,
};
use serde_json::json;
use std::sync::Arc;
use tauri::AppHandle;
use tauri::Manager;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BackendWatchdogMode {
    Sample,
    Warn,
    KillGroup,
    Tick,
    /// #1151 - runs the production quarantine-retry gate and loop, so native verification
    /// of the orphan path is a deterministic, assertable artifact rather than log scraping
    /// behind a 15 second backoff.
    QuarantineRetry,
}

impl BackendWatchdogMode {
    pub(super) fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("sample") {
            "sample" => Ok(Self::Sample),
            "warn" => Ok(Self::Warn),
            "killGroup" | "kill-group" => Ok(Self::KillGroup),
            "tick" => Ok(Self::Tick),
            "quarantineRetry" | "quarantine-retry" => Ok(Self::QuarantineRetry),
            other => Err(format!(
                "Unsupported resource monitor watchdog mode '{other}'. Expected sample, warn, \
                    killGroup, tick, or quarantineRetry."
            )),
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Sample => "sample",
            Self::Warn => "warn",
            Self::KillGroup => "killGroup",
            Self::Tick => "tick",
            Self::QuarantineRetry => "quarantineRetry",
        }
    }
}

pub(super) async fn handle_backend_request(
    app: &AppHandle,
    request: &UiAutomationRequest,
) -> UiAutomationResponse {
    if request.action != UiAutomationAction::Backend {
        return UiAutomationResponse::error_for_request(
            request,
            "unsupported_action",
            "Backend automation window only supports backend requests.",
        );
    }

    match request.selector.as_str() {
        RESOURCE_WATCHDOG_BACKEND_SELECTOR => {
            handle_resource_watchdog_backend_request(app, request).await
        }
        _ => UiAutomationResponse::error_for_request(
            request,
            "unsupported_selector",
            "Backend automation selector is not supported.",
        ),
    }
}

pub(super) async fn handle_resource_watchdog_backend_request(
    app: &AppHandle,
    request: &UiAutomationRequest,
) -> UiAutomationResponse {
    let cfg = crate::config::settings::load_settings();
    handle_resource_watchdog_backend_request_with_config(app, request, &cfg).await
}

pub(super) async fn handle_resource_watchdog_backend_request_with_config<R: tauri::Runtime>(
    app: &AppHandle<R>,
    request: &UiAutomationRequest,
    cfg: &crate::config::settings::AppSettings,
) -> UiAutomationResponse {
    let mode = match BackendWatchdogMode::parse(request.value.as_deref()) {
        Ok(mode) => mode,
        Err(message) => {
            return UiAutomationResponse::error_for_request(request, "invalid_value", &message);
        }
    };

    let Some(monitor) = app.try_state::<Arc<crate::resource_monitor::ResourceMonitorState>>()
    else {
        return UiAutomationResponse::error_for_request(
            request,
            "backend_state_missing",
            "Resource monitor state is not registered.",
        );
    };

    let limits = crate::resource_monitor::ResourceLimits::from(cfg);
    let resource_monitor_enabled = monitor.is_effectively_enabled(cfg.resource_monitor_enabled);
    let snapshot = monitor.snapshot(limits);
    let groups = snapshot
        .groups
        .iter()
        .filter_map(|group| {
            Uuid::parse_str(&group.session_id)
                .ok()
                .map(|session_id| (session_id, group.clone()))
        })
        .collect::<Vec<_>>();
    let decisions = crate::resource_monitor::watchdog::evaluate_watchdog_groups(&groups, limits);
    let warn_matches = decisions
        .iter()
        .filter(|decision| decision.warn_required)
        .count();
    let kill_matches = decisions
        .iter()
        .filter(|decision| decision.kill_required)
        .count();

    let kill_enabled = resource_monitor_enabled
        && (matches!(mode, BackendWatchdogMode::KillGroup)
            || (matches!(mode, BackendWatchdogMode::Tick)
                && cfg.resource_watchdog_action
                    == crate::config::settings::ResourceWatchdogAction::KillGroup));

    let mut kill_results = Vec::new();
    if kill_enabled {
        for decision in decisions.iter().filter(|decision| decision.kill_required) {
            let Some(coordinator) =
                app.try_state::<crate::session::selection::SelectionCoordinator>()
            else {
                kill_results.push(json!({
                    "ok": false,
                    "sessionId": decision.session_id,
                    "message": "selectionCoordinatorUnavailable",
                }));
                continue;
            };
            match coordinator
                .watchdog_resource_kill(decision.session_id)
                .await
            {
                Ok(crate::session::selection::WatchdogKillOutcome::Completed(result)) => {
                    kill_results.push(json!({
                        "ok": true,
                        "result": result,
                    }))
                }
                // #1151 - `alreadyPending` keeps its value and type byte-identical so every
                // existing automation assertion still passes; `reason` is the additive
                // discriminator that makes the split observable from outside the process.
                Ok(crate::session::selection::WatchdogKillOutcome::AlreadyInFlight) => kill_results
                    .push(json!({
                        "ok": true,
                        "sessionId": decision.session_id,
                        "alreadyPending": true,
                        "reason": "alreadyInFlight",
                    })),
                Ok(crate::session::selection::WatchdogKillOutcome::NoPublicSession) => kill_results
                    .push(json!({
                        "ok": true,
                        "sessionId": decision.session_id,
                        "alreadyPending": true,
                        "reason": "noPublicSession",
                    })),
                Err(message) => kill_results.push(json!({
                    "ok": false,
                    "sessionId": decision.session_id,
                    "message": message,
                })),
            }
        }
    }

    // #1151 - run the PRODUCTION quarantine-retry gate and loop, not a mirrored copy, so
    // these reports are evidence about shipped dispatch logic. The coordinator is resolved
    // ONCE before the call, because the loop takes it by reference for every group.
    let mut quarantine_retries = Vec::new();
    if resource_monitor_enabled
        && matches!(
            mode,
            BackendWatchdogMode::QuarantineRetry | BackendWatchdogMode::Tick
        )
    {
        match app.try_state::<crate::session::selection::SelectionCoordinator>() {
            Some(coordinator) => {
                for report in crate::resource_monitor::watchdog::run_quarantine_retries(
                    &monitor,
                    coordinator.inner(),
                    &groups,
                )
                .await
                {
                    quarantine_retries.push(serde_json::to_value(&report).unwrap_or_else(
                        |error| json!({ "ok": false, "message": error.to_string() }),
                    ));
                }
            }
            None => quarantine_retries.push(json!({
                "ok": false,
                "message": "selectionCoordinatorUnavailable",
            })),
        }
    }

    let state = if !resource_monitor_enabled {
        "disabled"
    } else if !kill_results.is_empty() {
        "enforcing"
    } else if kill_matches > 0 {
        "critical"
    } else if warn_matches > 0 {
        "warn"
    } else {
        "ok"
    };

    let snapshot_diagnostics = if monitor.supports_process_tree_enforcement() {
        json!({
            "capturedAt": snapshot.captured_at,
            "overallState": snapshot.overall_state,
            "activeAgentGroups": snapshot.active_agent_groups,
            "appPrivateBytes": snapshot.app_private_bytes,
            "warnings": snapshot.warnings,
        })
    } else {
        json!({
            "overallState": snapshot.overall_state,
            "activeAgentGroups": snapshot.active_agent_groups,
            "appPrivateBytes": snapshot.app_private_bytes,
            "warnings": snapshot.warnings,
        })
    };

    UiAutomationResponse {
        ok: true,
        request_id: request.request_id.clone(),
        window: request.window.clone(),
        action: request.action,
        selector: request.selector.clone(),
        target: Some(json!({
            "testId": request.selector,
            "role": "backend",
            "state": state,
            "tag": "backend",
            "text": format!(
                "resource monitor watchdog {}: {} group(s), {} warn match(es), {} kill match(es)",
                mode.as_str(),
                groups.len(),
                warn_matches,
                kill_matches
            ),
            "visible": true,
            "disabled": false,
        })),
        error: None,
        message: None,
        available: None,
        diagnostics: Some(json!({
            "mode": mode.as_str(),
            "configuredAction": cfg.resource_watchdog_action,
            "resourceMonitorEnabled": resource_monitor_enabled,
            "killApplied": kill_enabled,
            "limits": {
                "maxConcurrentAgentGroups": limits.max_concurrent_agent_processes,
                "groupWarnPrivateBytes": limits.group_warn_private_bytes,
                "groupKillPrivateBytes": limits.group_kill_private_bytes,
                "processKillPrivateBytes": limits.process_kill_private_bytes,
            },
            // #1151 - `snapshot` is captured BEFORE the action loop and is deliberately
            // left that way. Every post-retry assertion reads `quarantineRetries[i]`
            // instead; reading `snapshot.activeAgentGroups` after a successful reclaim
            // still shows the pre-action figure and produces a false FAIL.
            "snapshot": snapshot_diagnostics,
            "decisions": decisions,
            "killResults": kill_results,
            "quarantineRetries": quarantine_retries,
        })),
        available_windows: None,
        timeout_ms: None,
        phase: None,
    }
}
