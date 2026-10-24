use super::*;

// Test 22: `is_armed` is false for an unknown session, never panics, and
// returns false rather than blocking while a writer holds the map.
#[test]
fn is_armed_is_false_for_an_unknown_session() {
    let flags = ArmedFlags::new();
    assert!(!flags.is_armed("nobody"));
    // And asking did not create an entry that a later `arm` would collide with.
    flags.arm("nobody");
    assert!(flags.is_armed("nobody"));
}

#[test]
fn is_armed_does_not_block_while_a_writer_holds_the_map() {
    let flags = Arc::new(ArmedFlags::new());
    flags.arm("held");
    assert!(flags.is_armed("held"));

    let guard = flags.flags.write().expect("writer");
    let probe = {
        let flags = Arc::clone(&flags);
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let armed = flags.is_armed("held");
            (armed, started.elapsed())
        })
    };
    let (armed, elapsed) = probe.join().expect("probe thread");
    drop(guard);

    assert!(!armed, "a contended map must fail closed");
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "is_armed blocked for {elapsed:?}"
    );
    // Once the writer is gone the real value is visible again.
    assert!(flags.is_armed("held"));
}

#[test]
fn arm_disarm_and_remove_round_trip() {
    let flags = ArmedFlags::new();
    let handle = flags.handle("s");
    assert!(!handle.load(Ordering::Acquire));
    flags.arm("s");
    assert!(handle.load(Ordering::Acquire));
    flags.disarm("s");
    assert!(!flags.is_armed("s"));
    flags.arm("s");
    flags.remove("s");
    assert!(!flags.is_armed("s"));
    // The handle held by an earlier caller is cleared, not left stale.
    assert!(!handle.load(Ordering::Acquire));
}
