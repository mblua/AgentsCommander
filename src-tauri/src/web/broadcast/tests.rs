use super::*;

#[test]
fn drops_slow_client_when_queue_is_full() {
    let broadcaster = WsBroadcaster::new();
    let _slow_rx = broadcaster.subscribe();
    assert_eq!(broadcaster.client_count(), 1);

    for n in 0..WS_CLIENT_QUEUE_CAP {
        broadcaster.broadcast_event("tick", &serde_json::json!({ "n": n }));
        assert_eq!(broadcaster.client_count(), 1);
    }

    broadcaster.broadcast_event("tick", &serde_json::json!({ "n": "overflow" }));
    assert_eq!(broadcaster.client_count(), 0);
}

#[test]
fn a_poisoned_senders_lock_does_not_cascade() {
    let broadcaster = WsBroadcaster::new();
    let poisoner = broadcaster.clone();
    let joined = std::thread::spawn(move || {
        let _guard = poisoner
            .senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        panic!("poison the senders mutex on purpose");
    })
    .join();
    assert!(joined.is_err(), "the helper thread must have panicked");
    assert!(broadcaster.senders.is_poisoned());

    let mut rx = broadcaster.subscribe();
    broadcaster.broadcast_event("tick", &serde_json::json!({ "n": 1 }));
    broadcaster.broadcast_pty_output("session", b"x");
    assert_eq!(broadcaster.client_count(), 1);
    assert!(matches!(rx.try_recv(), Ok(WsOutMsg::Text(_))));
    assert!(matches!(rx.try_recv(), Ok(WsOutMsg::Binary(_))));
}
