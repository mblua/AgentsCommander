use super::*;

// Test 23: `open` is idempotent — two calls for one session id return the
// same slot — and one `close` drops it.
#[test]
fn open_is_idempotent_and_close_drops_the_entry() {
    let registry = CaptureRegistry::new();
    let (first, rx) = registry.open("session-a");
    assert!(rx.is_some(), "the creating call owns the receiver");
    let (second, rx_again) = registry.open("session-a");
    assert!(rx_again.is_none(), "a channel has exactly one receiver");

    // The same slot: a transition through one handle is visible on the other.
    first.slot.invalidate("probe");
    assert_eq!(first.slot.seq(), second.slot.seq());
    assert!(matches!(
        second.slot.snapshot().value,
        crate::capture::sink::SlotValue::Invalid(_)
    ));
    assert!(first.tx.same_channel(&second.tx));

    assert!(registry.slot("session-a").is_some());
    registry.close("session-a");
    assert!(!registry.is_open("session-a"));
    assert!(registry.slot("session-a").is_none());
}

#[test]
fn an_unknown_session_has_no_slot_and_no_sender() {
    let registry = CaptureRegistry::new();
    assert!(registry.slot("nobody").is_none());
    assert!(registry.sender("nobody").is_none());
    // `close` on an unknown session is a no-op, not a panic.
    registry.close("nobody");
}

#[test]
fn distinct_sessions_get_distinct_slots() {
    let registry = CaptureRegistry::new();
    let (a, _) = registry.open("a");
    let (b, _) = registry.open("b");
    a.slot.invalidate("only a");
    assert_eq!(b.slot.seq(), 0, "session b must be untouched");
}
