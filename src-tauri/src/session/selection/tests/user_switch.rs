use super::*;
use crate::pty::backend::SessionBackendKind;
use crate::session::session::Session;
use tauri::Listener;

type MockApp = tauri::App<tauri::test::MockRuntime>;

async fn create_shell_session(
    manager: &Arc<tokio::sync::RwLock<SessionManager>>,
    working_directory: &str,
) -> Session {
    manager
        .read()
        .await
        .create_session(
            "shell".to_string(),
            Vec::new(),
            working_directory.to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .unwrap()
}

/// Marks every session live, then routes each one through a local process.
fn live_routed_pty(
    backend: &Arc<LifecycleTestBackend>,
    session_ids: &[uuid::Uuid],
) -> Arc<Mutex<PtyManager>> {
    for session_id in session_ids {
        backend.set_live(*session_id, true);
    }
    let pty = Arc::new(Mutex::new(PtyManager::new_for_test(backend.clone())));
    for session_id in session_ids {
        pty.lock()
            .unwrap()
            .record_route(*session_id, SessionBackendKind::LocalProcess);
    }
    pty
}

/// Builds the mock app, starts the coordinator on it and finishes restore-first.
async fn start_switch_app(
    manager: &Arc<tokio::sync::RwLock<SessionManager>>,
    pty: &Arc<Mutex<PtyManager>>,
    detached: DetachedSessionsState,
    label: &str,
) -> (SelectionCoordinator, MockApp) {
    let coordinator = SelectionCoordinator::new(Arc::clone(manager), CancellationToken::new());
    let app = tauri::test::mock_builder()
        .manage(crate::commands::session::selection_executor::state::<
            tauri::test::MockRuntime,
        >())
        .manage(Arc::clone(manager))
        .manage(Arc::clone(pty))
        .manage(detached)
        .manage(WsBroadcaster::new())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap_or_else(|error| panic!("build {label} app: {error:?}"));
    coordinator
        .start(app.handle().clone())
        .unwrap_or_else(|error| panic!("start {label} coordinator: {error:?}"));
    coordinator.submit_restore_first().await.unwrap().finish();
    (coordinator, app)
}

fn listen_events(
    app: &MockApp,
    event_names: &[&'static str],
) -> std::sync::mpsc::Receiver<(&'static str, String)> {
    let (events_tx, events_rx) = std::sync::mpsc::channel();
    for &event_name in event_names {
        let events_tx = events_tx.clone();
        app.listen_any(event_name, move |event| {
            let _ = events_tx.send((event_name, event.payload().to_string()));
        });
    }
    events_rx
}

#[tokio::test]
async fn user_switch_to_already_selected_session_publishes_view_requested() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    // The filler absorbs the manager's automatic first-session selection, so
    // the first user switch below is a real selection change.
    create_shell_session(&manager, "C:/filler").await;
    let session = create_shell_session(&manager, "C:/already-selected").await;
    let backend = Arc::new(LifecycleTestBackend::default());
    let pty = live_routed_pty(&backend, &[session.id]);
    let (coordinator, app) = start_switch_app(
        &manager,
        &pty,
        DetachedSessionsState::default(),
        "already-selected",
    )
    .await;
    let events_rx = listen_events(&app, &["session_switched", "session_view_requested"]);

    coordinator
        .transition(SelectionRequest::user_switch(session.id))
        .await
        .unwrap()
        .expect("the first switch changes the selection");
    assert_eq!(
        events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
        "session_view_requested"
    );
    assert_eq!(
        events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
        "session_switched"
    );
    let revision = manager.read().await.selection_payload().await.revision();

    let committed = coordinator
        .transition(SelectionRequest::user_switch(session.id))
        .await
        .unwrap();
    assert!(
        committed.is_none(),
        "an already-selected target commits no selection payload"
    );
    let (event_name, payload) = events_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("an already-selected switch publishes the view request");
    assert_eq!(event_name, "session_view_requested");
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["id"], session.id.to_string());
    assert!(
        events_rx.try_recv().is_err(),
        "an unchanged selection publishes no session_switched"
    );
    assert_eq!(
        manager.read().await.selection_payload().await.revision(),
        revision
    );
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn user_switch_to_detached_target_publishes_no_view_requested() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let detached_session = create_shell_session(&manager, "C:/detached-target").await;
    let live_session = create_shell_session(&manager, "C:/live-detached-control").await;
    let backend = Arc::new(LifecycleTestBackend::default());
    let pty = live_routed_pty(&backend, &[detached_session.id, live_session.id]);
    let detached = DetachedSessionsState::default();
    let (coordinator, app) =
        start_switch_app(&manager, &pty, Arc::clone(&detached), "detached-target").await;
    let events_rx = listen_events(&app, &["session_switched", "session_view_requested"]);

    assert_eq!(
        manager.read().await.selection_payload().await.id(),
        Some(detached_session.id),
        "the created session is the current selection"
    );
    detached.lock().unwrap().insert(detached_session.id);

    assert!(
        coordinator
            .transition(SelectionRequest::user_switch(detached_session.id))
            .await
            .unwrap()
            .is_none(),
        "a detached target exits before commit"
    );
    coordinator
        .transition(SelectionRequest::user_switch(live_session.id))
        .await
        .unwrap()
        .expect("the control switch reaches the live session");
    // The channel was never drained, so the first event arriving after both
    // switches proves the detector was live and the detached switch emitted nothing.
    let (event_name, payload) = events_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("the control switch keeps the detector live");
    assert_eq!(event_name, "session_view_requested");
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["id"], live_session.id.to_string());
    assert_eq!(
        events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0,
        "session_switched"
    );
    coordinator.close_and_join().await;
}

#[tokio::test]
async fn user_switch_to_session_without_live_pty_publishes_no_view_requested() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let dead_session = create_shell_session(&manager, "C:/no-live-pty").await;
    let live_session = create_shell_session(&manager, "C:/live-after-repair").await;
    let backend = Arc::new(LifecycleTestBackend::default());
    let pty = live_routed_pty(&backend, &[dead_session.id, live_session.id]);
    let (coordinator, app) = start_switch_app(
        &manager,
        &pty,
        DetachedSessionsState::default(),
        "no-live-pty",
    )
    .await;
    let events_rx = listen_events(&app, &["session_switched", "session_view_requested"]);

    // Precondition: the session that will lose its PTY is the current selection.
    assert_eq!(
        manager.read().await.selection_payload().await.id(),
        Some(dead_session.id),
        "the created session is the current selection"
    );
    backend.set_live(dead_session.id, false);
    pty.lock()
        .unwrap()
        .remove_route_if_kind(dead_session.id, SessionBackendKind::LocalProcess);

    assert_eq!(
        coordinator
            .transition(SelectionRequest::user_switch(dead_session.id))
            .await
            .unwrap_err(),
        "Session has no live PTY"
    );
    coordinator
        .transition(SelectionRequest::user_switch(live_session.id))
        .await
        .unwrap()
        .expect("the control switch reaches the live session");

    assert_repair_then_live_switch_events(&events_rx, live_session.id);
    coordinator.close_and_join().await;
}

fn assert_repair_then_live_switch_events(
    events_rx: &std::sync::mpsc::Receiver<(&str, String)>,
    live_session_id: uuid::Uuid,
) {
    // The channel was never drained: the repair event is the in-test positive
    // control that proves the dead switch emitted no view request.
    let (repair_name, repair_payload) = events_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("the liveness repair publishes a clearing selection event");
    assert_eq!(repair_name, "session_switched");
    let repair_payload: serde_json::Value = serde_json::from_str(&repair_payload).unwrap();
    assert!(repair_payload["id"].is_null());
    assert_eq!(repair_payload["source"], "livenessReconcile");

    let (view_name, view_payload) = events_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(view_name, "session_view_requested");
    let view_payload: serde_json::Value = serde_json::from_str(&view_payload).unwrap();
    assert_eq!(view_payload["id"], live_session_id.to_string());

    let (switched_name, switched_payload) = events_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(switched_name, "session_switched");
    let switched_payload: serde_json::Value = serde_json::from_str(&switched_payload).unwrap();
    assert_eq!(switched_payload["id"], live_session_id.to_string());

    assert!(events_rx.try_recv().is_err());
}

#[tokio::test]
async fn route_loss_publishes_destroyed_exited_row_and_one_dormant_selection() {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let session = create_shell_session(&manager, "C:/work").await;
    let backend = Arc::new(LifecycleTestBackend::default());
    let pty = live_routed_pty(&backend, &[session.id]);
    let (coordinator, app) = start_switch_app(
        &manager,
        &pty,
        DetachedSessionsState::default(),
        "route-loss",
    )
    .await;
    let events_rx = listen_events(
        &app,
        &["session_destroyed", "session_created", "session_switched"],
    );

    backend.set_live(session.id, false);
    pty.lock()
        .unwrap()
        .remove_route_if_kind(session.id, SessionBackendKind::LocalProcess);
    let sender = coordinator.container_lifecycle_sender();
    assert_eq!(
        sender.route_lost(session.id, 44).await.unwrap(),
        CriticalAdmissionOutcome::Completed(())
    );
    let row = manager.read().await.get_session(session.id).await.unwrap();
    assert_eq!(row.status, SessionStatus::Exited(44));
    let selection = manager.read().await.selection_payload().await;
    assert_eq!(selection.id(), Some(session.id));
    assert_eq!(selection.mode(), SelectionMode::Dormant);
    assert_eq!(selection.status(), Some(&SessionStatus::Exited(44)));
    assert_eq!(selection.source(), SelectionSource::LivenessReconcile);
    let observed = (0..3)
        .map(|_| events_rx.recv_timeout(Duration::from_secs(1)).unwrap().0)
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec!["session_destroyed", "session_created", "session_switched"]
    );
    let revision = selection.revision();
    assert_eq!(
        sender.route_lost(session.id, 99).await.unwrap(),
        CriticalAdmissionOutcome::Completed(())
    );
    assert_eq!(
        manager.read().await.selection_payload().await.revision(),
        revision
    );
    assert!(events_rx.try_recv().is_err());
    coordinator.close_and_join().await;
}
