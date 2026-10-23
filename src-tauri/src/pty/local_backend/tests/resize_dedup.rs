use super::PtyInstance;
use std::sync::Mutex;

/// A `PtyInstance` carries real ConPTY handles, so the tests below drive the one piece
/// that #973 (C) actually changes: the size cache and the decision it drives.
fn cache(cols: u16, rows: u16) -> Mutex<(u16, u16)> {
    Mutex::new((cols, rows))
}

/// #973 (C) - the frontend calls resize unconditionally from a double
/// `requestAnimationFrame` and a `ResizeObserver`, so a single attach fires 5-20
/// identical resizes and ConPTY hands every one of them to the child as a real event.
/// A resize that changes nothing must not reach the child.
#[test]
fn a_resize_to_the_size_it_already_has_is_not_sent_to_the_child() {
    let size = cache(74, 23);
    assert!(
        !PtyInstance::size_changed_in(&size, 74, 23),
        "an identical resize must be skipped: today AC fires 5-20 of these per attach"
    );
}

/// The other half: a real resize must still get through, or the terminal would never
/// follow the window.
#[test]
fn a_resize_that_moves_the_size_is_sent() {
    let size = cache(74, 23);
    assert!(
        PtyInstance::size_changed_in(&size, 74, 24),
        "one row is a real resize"
    );
    assert!(
        PtyInstance::size_changed_in(&size, 120, 30),
        "a real resize"
    );
}

/// The trap in the dedup: if the cache were updated BEFORE the ConPTY accepted the new
/// size, a failed resize would leave the cache claiming a size the PTY never took, and
/// every retry would then be skipped as a no-op - the terminal would be wedged at the
/// wrong size forever. The cache is only written after `master.resize()` returns Ok.
#[test]
fn a_failed_resize_does_not_poison_the_cache_and_the_retry_still_fires() {
    let size = cache(74, 23);
    // resize to 100x40 "fails": remember_size is never reached, so the cache stays put
    assert!(PtyInstance::size_changed_in(&size, 100, 40));
    assert!(
        PtyInstance::size_changed_in(&size, 100, 40),
        "after a failed resize the retry must still be issued, not skipped as a no-op"
    );
    // now it succeeds
    PtyInstance::remember_size_in(&size, 100, 40);
    assert!(!PtyInstance::size_changed_in(&size, 100, 40));
}
