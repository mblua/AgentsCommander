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

#[derive(Clone)]
struct RecordingAutomationHost {
    state: UiAutomationState,
    calls: Arc<Mutex<Vec<&'static str>>>,
    expired_response: PathBuf,
    request_id: String,
}

impl UiAutomationEvents for RecordingAutomationHost {
    fn automation_request(
        &self,
        window: &str,
        request: &UiAutomationRequest,
    ) -> Result<(), String> {
        assert_eq!(window, "main");
        assert_eq!(request.request_id, self.request_id);
        assert!(self.state.is_pending(&self.request_id));
        assert!(self.state.inflight_path(&self.request_id).exists());
        self.calls.lock().unwrap().push("emit");
        Err("recorded emission failure".to_string())
    }
}

impl UiAutomationHost for RecordingAutomationHost {
    fn available_windows(&self) -> Vec<String> {
        let mut calls = self.calls.lock().unwrap();
        match calls.len() {
            0 => {
                assert!(!self.expired_response.exists());
                calls.push("sync");
                vec!["main".to_string()]
            }
            1 => {
                assert!(self.expired_response.exists());
                assert!(!self.state.is_pending(&self.request_id));
                calls.push("validate");
                vec!["main".to_string()]
            }
            3 => {
                assert!(!self.state.is_pending(&self.request_id));
                assert!(self.state.inflight_path(&self.request_id).exists());
                calls.push("failed_emit_lookup");
                vec!["sidebar".to_string()]
            }
            _ => panic!("unexpected window lookup: {calls:?}"),
        }
    }

    fn spawn_background(&self, _future: futures::future::BoxFuture<'static, ()>) {
        panic!("ordinary and stale requests must not spawn backend work");
    }

    fn backend_request<'a>(
        &'a self,
        _request: &'a UiAutomationRequest,
    ) -> futures::future::BoxFuture<'a, UiAutomationResponse> {
        panic!("ordinary and stale requests must not reach backend work");
    }
}

#[test]
fn issue_3265_automation_host() {
    let tmp = tempfile::tempdir().unwrap();
    let state = UiAutomationState::new(true, tmp.path().to_path_buf());
    state.initialize_files().unwrap();
    state.mark_frontend_ready("main", Some("main")).unwrap();
    let mut expired = sample_request(Uuid::new_v4().to_string(), "expired");
    expired.expires_at_unix_ms = Some(now_unix_ms() - 1);
    let expired_response = state.response_path(&expired.request_id);
    let expired_inflight = state.inflight_path(&expired.request_id);
    fs::write(&expired_inflight, "{}").unwrap();
    state.inner.pending.lock().unwrap().insert(
        expired.request_id.clone(),
        PendingRequest {
            request: expired,
            response_path: expired_response.clone(),
            inflight_path: expired_inflight.clone(),
        },
    );
    let mut request = sample_request(Uuid::new_v4().to_string(), "target");
    request.token = state.inner.token.clone();
    request.expires_at_unix_ms = Some(now_unix_ms() + 60_000);
    let request_path = state
        .requests_dir()
        .join(format!("{}.json", request.request_id));
    fs::write(&request_path, serde_json::to_vec(&request).unwrap()).unwrap();
    let host = RecordingAutomationHost {
        state: state.clone(),
        calls: Arc::new(Mutex::new(Vec::new())),
        expired_response,
        request_id: request.request_id.clone(),
    };
    state.poll_once_inner(&host).unwrap();
    assert_eq!(
        *host.calls.lock().unwrap(),
        vec!["sync", "validate", "emit", "failed_emit_lookup"]
    );
    let response_bytes = fs::read(state.response_path(&request.request_id)).unwrap();
    let response: UiAutomationResponse = serde_json::from_slice(&response_bytes).unwrap();
    assert_eq!(response.request_id, request.request_id);
    assert_eq!(response.window, request.window);
    assert_eq!(response.action, request.action);
    assert_eq!(response.selector, request.selector);
    assert_eq!(response.error.as_deref(), Some("webview_unavailable"));
    assert_eq!(
        response.available_windows,
        Some(vec!["sidebar".to_string()])
    );
    assert_eq!(
        response.diagnostics,
        Some(json!({ "message": "recorded emission failure" }))
    );
    assert!(state.inner.pending.lock().unwrap().is_empty());
    assert!(!request_path.exists());
    assert!(!state.inflight_path(&request.request_id).exists());
    assert!(!expired_inflight.exists());

    host.calls.lock().unwrap().clear();
    let mut stale = sample_request(Uuid::new_v4().to_string(), "target");
    stale.token = "invalid-current-session-token".to_string();
    stale.expires_at_unix_ms = Some(now_unix_ms() + 60_000);
    let stale_path = state
        .requests_dir()
        .join(format!("{}.json", stale.request_id));
    fs::write(&stale_path, serde_json::to_vec(&stale).unwrap()).unwrap();
    // The first lookup precedes pending expiry; a fresh absent marker observes that slot again.
    let stale_host = RecordingAutomationHost {
        expired_response: tmp.path().join("absent-expiry-marker"),
        request_id: stale.request_id.clone(),
        ..host
    };
    state.poll_once_inner(&stale_host).unwrap();
    assert_eq!(*stale_host.calls.lock().unwrap(), vec!["sync"]);
    let response: UiAutomationResponse =
        serde_json::from_slice(&fs::read(state.response_path(&stale.request_id)).unwrap()).unwrap();
    assert_eq!(response.request_id, stale.request_id);
    assert_eq!(response.error.as_deref(), Some("automation_session_stale"));
    assert!(!stale_path.exists());
    assert!(!state.inflight_path(&stale.request_id).exists());
    assert!(state.inner.pending.lock().unwrap().is_empty());
}
