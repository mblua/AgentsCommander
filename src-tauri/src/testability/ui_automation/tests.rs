use super::*;
use crate::config::settings::{AppSettings, ResourceWatchdogAction};
use crate::resource_monitor::registry::{
    ProcessTreeBackend, ResourceError, ResourceLaunchRegistration,
};
use crate::resource_monitor::types::{
    ObservedProcess, ObservedProcessTree, ProcessIdentity, ProcessMemory, ResourceLaunchMetadata,
    TerminateOutcome,
};
use crate::session::manager::SessionManager;
use crate::session::selection::{CriticalAdmissionKind, SelectionCoordinator, WatchdogKillOutcome};
use crate::web::broadcast::WsBroadcaster;
use crate::DetachedSessionsState;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_util::sync::CancellationToken;

mod cli_grammar;
mod pid;
mod requests;
mod watchdog;

fn sample_request(request_id: String, selector: &str) -> UiAutomationRequest {
    UiAutomationRequest {
        request_id,
        token: "token".to_string(),
        window: "main".to_string(),
        action: UiAutomationAction::Query,
        selector: selector.to_string(),
        value: None,
        expires_at_unix_ms: Some(now_unix_ms() + 1_000),
    }
}

fn action_wire_name(action: UiAutomationAction) -> &'static str {
    match action {
        UiAutomationAction::Query => "query",
        UiAutomationAction::Click => "click",
        UiAutomationAction::ContextClick => "contextClick",
        UiAutomationAction::Hover => "hover",
        UiAutomationAction::SetValue => "setValue",
        UiAutomationAction::TypeText => "typeText",
        UiAutomationAction::Backend => "backend",
        UiAutomationAction::Terminal => "terminal",
        UiAutomationAction::Pointer => "pointer",
        UiAutomationAction::Key => "key",
    }
}
