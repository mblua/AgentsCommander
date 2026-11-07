use super::*;
use crate::phone::mailbox::host::CoordinatorTicket;
use crate::phone::mailbox::pty_observation::WakeDeliveryState;
use crate::phone::terminal_snapshot::TerminalSnapshotHost;
use crate::session::warnings::SessionWarningEvents;
use std::sync::mpsc;
use tauri::Listener;

type Handle = tauri::AppHandle<tauri::test::MockRuntime>;

fn app() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(crate::app_state::AppOwners::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock native mailbox app")
}

#[test]
fn issue_3946_adapter_absent_state() {
    let app = app();
    let handle = app.handle();
    assert!(MailboxHost::selection_opt(handle).is_none());
    assert!(MailboxHost::clocks(handle).is_none());
    assert!(MailboxHost::shutdown_opt(handle).is_none());
    assert!(MailboxHost::begin_coordinator_create(handle, "absent").is_none());
    assert!(TerminalSnapshotHost::snapshot_state_opt(handle).is_none());
    assert!(TerminalSnapshotHost::session_manager_opt(handle).is_none());
    assert!(TerminalSnapshotHost::pty_manager_opt(handle).is_none());
    assert!(TerminalSnapshotHost::settings_opt(handle).is_none());
    assert!(TerminalSnapshotHost::restore_opt(handle).is_none());
    assert!(TerminalSnapshotHost::purge_opt(handle).is_none());
    let required: [fn(&Handle); 6] = [
        |host| {
            let _ = MailboxHost::master_required(host);
        },
        |host| {
            let _ = MailboxHost::restore_required(host);
        },
        |host| {
            let _ = MailboxHost::purge_required(host);
        },
        |host| {
            let _ = MailboxHost::pending_clear(host);
        },
        |host| {
            let _ = MailboxHost::outbox(host);
        },
        |host| {
            let _ = WakeDeliveryState::selection(host);
        },
    ];
    for getter in required {
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| getter(handle))).is_err());
    }
    fn warning_bound<T: SessionWarningEvents>(_: &T) {}
    warning_bound(handle);
}

#[test]
fn issue_3946_adapter_live_getters() {
    let app = app();
    let handle = app.handle();
    let restore = Arc::new(RestoreInProgress(std::sync::atomic::AtomicBool::new(false)));
    let purge = Arc::new(PurgeGuard::default());
    let pending = Arc::new(PendingSelfClear::default());
    let clocks: CoordinatorClocksState = Arc::new(Mutex::new(Default::default()));
    assert!(handle.manage(Arc::clone(&restore)));
    assert!(handle.manage(Arc::clone(&purge)));
    assert!(handle.manage(Arc::clone(&pending)));
    assert!(handle.manage(Arc::clone(&clocks)));
    assert!(Arc::ptr_eq(MailboxHost::restore_required(handle), &restore));
    assert!(Arc::ptr_eq(MailboxHost::purge_required(handle), &purge));
    assert!(Arc::ptr_eq(MailboxHost::pending_clear(handle), &pending));
    assert!(Arc::ptr_eq(MailboxHost::clocks(handle).unwrap(), &clocks));
    assert!(Arc::ptr_eq(
        TerminalSnapshotHost::restore_opt(handle).unwrap(),
        &restore
    ));
    assert!(Arc::ptr_eq(
        TerminalSnapshotHost::purge_opt(handle).unwrap(),
        &purge
    ));
    restore.0.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(MailboxHost::restore_required(handle)
        .0
        .load(std::sync::atomic::Ordering::SeqCst));
}

fn seed_markers(clocks: &CoordinatorClocksState, fqn: &str, time: chrono::DateTime<chrono::Utc>) {
    let mut clocks = clocks.lock().unwrap();
    assert!(clocks.mark_auto_closed(fqn, time));
    assert!(clocks.mark_manually_closed(fqn, time));
}

fn assert_markers(
    clocks: &CoordinatorClocksState,
    fqn: &str,
    time: Option<chrono::DateTime<chrono::Utc>>,
) {
    let clocks = clocks.lock().unwrap();
    assert_eq!(clocks.auto_closed_at(fqn), time);
    assert_eq!(clocks.manually_closed_at(fqn), time);
}

#[test]
fn issue_3946_adapter_real_ticket_settlement() {
    let app = app();
    let handle = app.handle();
    let clocks: CoordinatorClocksState = Arc::new(Mutex::new(Default::default()));
    assert!(handle.manage(Arc::clone(&clocks)));
    let cwd = "C:/ac-test/project/.ac/wg-3946-team/__agent_coordinator";
    let fqn = crate::config::teams::agent_fqn_from_path(cwd);
    let time = chrono::Utc::now();
    for outcome in [
        "success",
        "failure",
        "cancel",
        "fenced_failure",
        "fenced_cancel",
    ] {
        seed_markers(&clocks, &fqn, time);
        let mut ticket: crate::commands::session::CoordinatorCreateTicket<
            tauri::test::MockRuntime,
        > = MailboxHost::begin_coordinator_create(handle, cwd).expect("real ticket");
        assert_markers(&clocks, &fqn, None);
        ticket.session_may_exist = outcome.starts_with("fenced");
        match outcome {
            "success" => CoordinatorTicket::succeeded(ticket),
            "failure" | "fenced_failure" => CoordinatorTicket::failed(ticket),
            _ => drop(ticket),
        }
        let restored = matches!(outcome, "failure" | "cancel");
        assert_markers(&clocks, &fqn, restored.then_some(time));
        // Reset only the test's clocks between independent ticket scenarios.
        *clocks.lock().unwrap() = Default::default();
    }
}

fn record(
    app: &tauri::App<tauri::test::MockRuntime>,
    name: &'static str,
) -> mpsc::Receiver<(String, serde_json::Value)> {
    let (sender, receiver) = mpsc::sync_channel(4);
    app.listen_any(name, move |event| {
        sender
            .try_send((
                name.into(),
                serde_json::from_str(event.payload()).expect("native event JSON"),
            ))
            .expect("bounded listener");
    });
    receiver
}

#[test]
fn issue_3946_adapter_browser_forwards_and_events() {
    let app = app();
    let handle = app.handle();
    let states = record(&app, "web_browser_state");
    let opens = record(&app, "web_browser_open_requested");
    assert_eq!(
        BrowserHost::snapshot(handle),
        crate::commands::web_browser::snapshot(handle)
    );
    assert_eq!(
        BrowserHost::reload(handle),
        Err("web browser is not open".into())
    );
    assert_eq!(
        BrowserHost::reload(handle),
        crate::commands::web_browser::reload(handle)
    );
    let snapshot = BrowserHost::agent_navigate(handle, "https://example.com/p21").unwrap();
    assert_eq!(snapshot.url, "https://example.com/p21");
    assert_eq!(BrowserHost::snapshot(handle), snapshot);
    let expected = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(
        states.try_recv().unwrap(),
        ("web_browser_state".into(), expected.clone())
    );
    assert_eq!(
        opens.try_recv().unwrap(),
        ("web_browser_open_requested".into(), expected)
    );
    assert!(states.try_recv().is_err());
    assert!(opens.try_recv().is_err());
    let direct = crate::commands::web_browser::agent_navigate(handle, "https://example.com/p21");
    assert_eq!(direct, Ok(snapshot));
}

#[test]
fn issue_3946_adapter_request_event_names_and_payloads() {
    use crate::phone::events::MailboxRequestEvents;
    use crate::phone::mailbox::refresh::ProjectRefreshEventPayload;
    let app = app();
    let refreshes = record(&app, "ac_project_refresh_requested");
    let settings = record(&app, "coding_agent_settings_updated");
    let payload = ProjectRefreshEventPayload {
        id: "request".into(),
        project_path: "project".into(),
        changed_path: Some("changed".into()),
        changed_name: Some("name".into()),
        reason: "agent-created".into(),
    };
    assert!(app.handle().project_refresh(&payload));
    assert_eq!(
        refreshes.try_recv().unwrap(),
        (
            "ac_project_refresh_requested".into(),
            serde_json::to_value(&payload).unwrap(),
        )
    );
    let payload = serde_json::json!({"agentId": "agent", "enabled": false});
    assert!(app.handle().coding_agent_settings_updated(&payload));
    assert_eq!(
        settings.try_recv().unwrap(),
        ("coding_agent_settings_updated".into(), payload)
    );
    assert!(refreshes.try_recv().is_err());
    assert!(settings.try_recv().is_err());
}
