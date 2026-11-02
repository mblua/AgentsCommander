use super::*;

#[test]
fn initial_payload_matches_contract() {
    let epoch = Uuid::new_v4();
    let payload = SessionSelection::initial(epoch);
    let value = serde_json::to_value(payload).unwrap();
    assert_eq!(value["epoch"], epoch.to_string());
    assert_eq!(value["source"], "initialHydration");
    assert_eq!(value["userInitiated"], false);
    assert_eq!(value["revision"], 0);
    assert_eq!(value["mode"], "none");
    assert!(value["id"].is_null());
    assert!(value["status"].is_null());
    assert_eq!(value["hasPty"], false);
    assert_eq!(value["detached"], false);
    assert_eq!(value["displayable"], false);
}

#[test]
fn payload_mode_invariants_and_epoch_are_stable() {
    let epoch = Uuid::new_v4();
    let id = Uuid::new_v4();
    let none = SessionSelection::none(epoch, 1, SelectionCause::AutoClose);
    let live = SessionSelection::live(epoch, 2, SelectionCause::UserSwitch, id);
    let dormant =
        SessionSelection::dormant(epoch, 3, SelectionCause::LivenessReconcile, id, 19, false);

    assert_eq!(none.epoch(), live.epoch());
    assert_eq!(live.epoch(), dormant.epoch());
    assert_eq!(none.mode(), SelectionMode::None);
    assert_eq!(none.id(), None);
    assert_eq!(none.status(), None);
    assert!(!none.has_pty() && !none.detached() && !none.displayable());
    assert_eq!(live.mode(), SelectionMode::Live);
    assert_eq!(live.id(), Some(id));
    assert_eq!(live.status(), Some(&SessionStatus::Active));
    assert!(live.has_pty() && !live.detached() && live.displayable());
    assert_eq!(dormant.mode(), SelectionMode::Dormant);
    assert_eq!(dormant.id(), Some(id));
    assert_eq!(dormant.status(), Some(&SessionStatus::Exited(19)));
    assert!(!dormant.has_pty() && !dormant.detached() && !dormant.displayable());

    let serialized = serde_json::to_value(dormant).expect("serialize dormant payload");
    assert_eq!(serialized["source"], "livenessReconcile");
    assert_eq!(serialized["userInitiated"], false);
    assert_eq!(serialized["mode"], "dormant");
}

#[test]
fn authoritative_publisher_sends_byte_equivalent_tauri_and_websocket_payloads() {
    use crate::web::broadcast::WsOutMsg;
    use tauri::Listener;

    let broadcaster = WsBroadcaster::new();
    let mut web = broadcaster.subscribe();
    let app = tauri::test::mock_builder()
        .manage(broadcaster)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build publisher test app");
    let (native_sender, native_receiver) = std::sync::mpsc::channel();
    app.listen_any("session_switched", move |event| {
        let _ = native_sender.send(event.payload().to_string());
    });

    let payload = SessionSelection::live(
        Uuid::new_v4(),
        41,
        SelectionCause::UserSwitch,
        Uuid::new_v4(),
    );
    publish_selection(app.handle(), &payload);
    let native: serde_json::Value = serde_json::from_str(
        &native_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("native selection event"),
    )
    .expect("parse native selection");
    let websocket = match web.try_recv().expect("websocket selection event") {
        WsOutMsg::Text(text) => {
            serde_json::from_str::<serde_json::Value>(&text).expect("parse websocket selection")
        }
        other @ WsOutMsg::Binary(_) => panic!("expected websocket text event, got {other:?}"),
    };
    assert_eq!(websocket["event"], "session_switched");
    assert_eq!(websocket["payload"], native);
    assert_eq!(native, serde_json::to_value(payload).unwrap());
}

#[test]
fn sealed_causes_pin_source_and_user_intent() {
    let cases = [
        (
            SelectionCause::UserSwitch,
            SelectionSource::UserSwitch,
            true,
        ),
        (
            SelectionCause::ManualClose,
            SelectionSource::ManualClose,
            true,
        ),
        (SelectionCause::AutoClose, SelectionSource::AutoClose, false),
        (SelectionCause::Restore, SelectionSource::Restore, false),
        (SelectionCause::Detach, SelectionSource::Detach, true),
        (SelectionCause::Attach, SelectionSource::Attach, true),
        (
            SelectionCause::SpawnRollback,
            SelectionSource::SpawnRollback,
            false,
        ),
        (
            SelectionCause::BackgroundCleanup,
            SelectionSource::BackgroundCleanup,
            false,
        ),
        (
            SelectionCause::LivenessReconcile,
            SelectionSource::LivenessReconcile,
            false,
        ),
    ];
    for (cause, source, user_initiated) in cases {
        assert_eq!(cause.source(), source);
        assert_eq!(cause.user_initiated(), user_initiated);
    }
    for (cause, source, user_initiated) in [
        (
            SelectionCause::SessionCreated(TrustedCreateIntent::User),
            SelectionSource::SessionCreated,
            true,
        ),
        (
            SelectionCause::SessionCreated(TrustedCreateIntent::Background),
            SelectionSource::SessionCreated,
            false,
        ),
        (
            SelectionCause::Restart(TrustedRestartIntent::User),
            SelectionSource::Restart,
            true,
        ),
        (
            SelectionCause::Restart(TrustedRestartIntent::Background),
            SelectionSource::Restart,
            false,
        ),
        (
            SelectionCause::ResourceMonitor(TrustedResourceIntent::User),
            SelectionSource::ResourceMonitor,
            true,
        ),
        (
            SelectionCause::ResourceMonitor(TrustedResourceIntent::Watchdog),
            SelectionSource::ResourceMonitor,
            false,
        ),
    ] {
        assert_eq!(cause.source(), source);
        assert_eq!(cause.user_initiated(), user_initiated);
    }
}
