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
