use super::{hand_over_held_size, resize_instance, send_size_to_conpty, PtyInstance, StartupGate};
use portable_pty::{native_pty_system, MasterPty, PtySize};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// A real ConPTY, with no child. `LocalProcessBackend` itself cannot be built here (its
/// `GitWatcher` needs a Tauri `AppHandle`), and it does not need to be: `PtyBackend::resize`
/// is a four-line wrapper - lock, delegate to `resize_instance`, broadcast - and
/// `resize_instance` is the whole of the decision.
fn conpty(cols: u16, rows: u16) -> (PtyInstance, Arc<Mutex<Box<dyn MasterPty + Send>>>) {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");
    drop(pair.slave);

    let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));
    let instance = PtyInstance {
        master: Arc::clone(&master),
        writer: Arc::new(Mutex::new(Box::new(std::io::sink()))),
        child: None,
        job: None,
        size: Mutex::new((cols, rows)),
        startup_gate: Mutex::new(StartupGate::Holding(None)),
        rendered: Arc::new(AtomicBool::new(false)),
    };
    (instance, master)
}

/// What the CHILD would see. This is the point of these tests: not what AC believes.
fn size_the_child_sees(master: &Arc<Mutex<Box<dyn MasterPty + Send>>>) -> (u16, u16) {
    let s = master.lock().unwrap().get_size().expect("get_size");
    (s.cols, s.rows)
}

/// #973 - THE COLD-START CASE. This is the one the user hits every morning.
///
/// `TerminalView` lives inside `<Show when={activeSessionId}>`, so when the app opens
/// with no active session there is no terminal host to measure, and the frontend cannot
/// supply a size. The PTY is opened at 120x30, the view then mounts, fits, and fires its
/// resize burst 300-500 ms later - straight into the startup window of the first coding
/// agent the user launches. **Option A cannot cover this. This is what covers it.**
///
/// Red without the gate: the resize reaches the ConPTY immediately, and the child is
/// resized while it is still coming up, which is what costs Codex its first content
/// render (measured: 8 blanks in 10).
#[test]
fn a_session_opened_without_a_view_size_holds_the_burst_until_the_child_renders() {
    let (instance, master) = conpty(120, 30);
    let id = Uuid::new_v4();

    // the view mounts and fits: 6 identical resizes inside a few milliseconds
    for _ in 0..6 {
        let sent = resize_instance(&instance, id, 74, 23).expect("resize");
        assert!(!sent, "nothing may reach a child that has not rendered yet");
    }

    assert_eq!(
        size_the_child_sees(&master),
        (120, 30),
        "the child has rendered nothing yet, so the ConPTY must NOT have been resized"
    );

    // the child paints
    let held = instance
        .startup_gate
        .lock()
        .unwrap()
        .open()
        .expect("the size the view asked for must have been kept");
    send_size_to_conpty(&instance, id, held.0, held.1).expect("apply");

    assert_eq!(
        size_the_child_sees(&master),
        (74, 23),
        "once the child has rendered, the size it should have had must be applied"
    );
}

/// The view fires 5-20 resizes per attach. Only the last is real; replaying all of them
/// at the child would be churn for nothing.
#[test]
fn only_the_last_held_size_reaches_the_child() {
    let (instance, master) = conpty(120, 30);
    let id = Uuid::new_v4();

    resize_instance(&instance, id, 74, 23).expect("resize");
    resize_instance(&instance, id, 90, 40).expect("resize");
    resize_instance(&instance, id, 74, 24).expect("resize");
    assert_eq!(size_the_child_sees(&master), (120, 30), "all of them held");

    let held = instance.startup_gate.lock().unwrap().open().expect("held");
    send_size_to_conpty(&instance, id, held.0, held.1).expect("apply");

    assert_eq!(
        size_the_child_sees(&master),
        (74, 24),
        "only the last size the view asked for should reach the child"
    );
}

/// Once the child is up, AC gets out of the way: every later resize - the user dragging
/// the window - goes straight through, forever. A gate that never reopened would leave
/// the terminal unable to follow its window.
#[test]
fn once_the_child_has_rendered_resizes_go_straight_through() {
    let (instance, master) = conpty(120, 30);
    let id = Uuid::new_v4();

    instance.startup_gate.lock().unwrap().open(); // the child rendered, nothing held

    assert!(resize_instance(&instance, id, 74, 23).expect("resize"));
    assert_eq!(size_the_child_sees(&master), (74, 23));

    assert!(resize_instance(&instance, id, 100, 50).expect("resize"));
    assert_eq!(
        size_the_child_sees(&master),
        (100, 50),
        "the gate must not close again"
    );
}

/// #973 (C) - a resize that changes nothing must not be sent. ConPTY hands even a
/// same-size resize to the child as a real event, and the view fires 5-20 of them.
#[test]
fn a_resize_that_changes_nothing_is_not_sent() {
    let (instance, id) = (conpty(74, 23).0, Uuid::new_v4());
    instance.startup_gate.lock().unwrap().open();

    assert!(
        !resize_instance(&instance, id, 74, 23).expect("resize"),
        "the ConPTY is already 74x23: nothing should be sent"
    );
    assert!(
        resize_instance(&instance, id, 74, 24).expect("resize"),
        "one row is a real resize and must be sent"
    );
}

/// #973 - a degenerate resize that lands WHILE THE GATE IS HOLDING must not evict the
/// real size the view is waiting to give the child.
///
/// This walks the path `pty_resize` actually walks - `resize_instance`, gate and all -
/// which is the entire point of it. The guard used to sit in `send_size_to_conpty`, PAST
/// the gate, so nothing checked a size on its way IN: the gate took the `0x0` last-wins
/// over the real `74x23`, the hand-over popped it, `send_size_to_conpty` refused it, and
/// the pending slot was consumed and gone. **Nothing retries.** The ConPTY then stays at
/// the size it was OPENED at, for good - on cold start, a 120x30 child behind a 74x23
/// terminal, until the user drags the window.
///
/// Red before the guard moved ahead of the gate:
/// `assertion failed: left: (120, 30)  right: (74, 23)`.
#[test]
fn a_degenerate_resize_cannot_evict_the_size_held_for_the_child() {
    let (instance, master) = conpty(120, 30);
    let id = Uuid::new_v4();

    // the view mounts and fits, while the child is still starting up: held, as it should be
    assert!(
        !resize_instance(&instance, id, 74, 23).expect("resize"),
        "nothing may reach a child that has not rendered yet"
    );

    // ...and now a zero dimension arrives, still inside the startup window. It comes off the
    // wire: `web/commands.rs` takes cols/rows straight from a JSON payload. (Not from xterm -
    // `fit()` clamps to 2x1 and cannot produce a zero.)
    assert!(
        !resize_instance(&instance, id, 0, 0).expect("must not error"),
        "a zero dimension must never be sent to the ConPTY"
    );

    // the child paints. This is the read loop's hand-over itself, not a copy of it.
    hand_over_held_size(&instance, id);

    assert_eq!(
        size_the_child_sees(&master),
        (74, 23),
        "the real held size must survive a degenerate resize"
    );
}

/// #973 - the guard INSIDE `send_size_to_conpty`, which is defence in depth: the last line
/// before `master.resize()`, and the one `hand_over_held_size` relies on when it applies a
/// size that has been sitting in the gate. The guard that keeps a `0x0` off the request
/// path is in `resize_instance`, ahead of the gate - see
/// `a_degenerate_resize_cannot_evict_the_size_held_for_the_child`, which is the one that
/// walks the path `pty_resize` walks. This test calls `send_size_to_conpty` DIRECTLY and
/// makes no claim about the gate.
///
/// It caught a real one. I had assumed portable-pty would reject `0x0`. **It does not**:
/// `master.resize(0x0)` returns Ok and the ConPTY is genuinely set to zero, so before the
/// guard this failed with `left: (0, 0)`.
///
/// The cache matters as much as the refusal: if a size the ConPTY never took were
/// cached, every retry would be skipped as a no-op and the terminal would be wedged at
/// the wrong size forever.
#[test]
fn a_degenerate_resize_is_refused_and_does_not_poison_the_cache() {
    let (instance, master) = conpty(120, 30);
    let id = Uuid::new_v4();
    instance.startup_gate.lock().unwrap().open();

    assert!(
        !send_size_to_conpty(&instance, id, 0, 0).expect("must not error"),
        "a zero dimension must never be sent to the ConPTY"
    );
    assert_eq!(
        size_the_child_sees(&master),
        (120, 30),
        "ConPTY accepts a 0x0 resize without complaint, so AC has to refuse it"
    );
    assert_eq!(
        *instance.size.lock().unwrap(),
        (120, 30),
        "and it must not be cached, or the next real resize is skipped as a no-op"
    );

    assert!(
        send_size_to_conpty(&instance, id, 74, 23).expect("resize"),
        "a real resize after a refused one must still go through"
    );
    assert_eq!(size_the_child_sees(&master), (74, 23));
}

/// The gate is a two-state machine. Pin it directly, including the ordering trap: the
/// hand-over happens exactly once, and after it the gate is open for good.
#[test]
fn the_gate_hands_over_exactly_once() {
    let mut gate = StartupGate::Holding(None);
    assert_eq!(gate.on_resize(74, 23), None, "held");
    assert_eq!(gate.on_resize(74, 24), None, "held, replacing the first");
    assert_eq!(
        gate.open(),
        Some((74, 24)),
        "the last held size, handed over once"
    );
    assert_eq!(gate.open(), None, "a second open hands over nothing");
    assert_eq!(
        gate.on_resize(80, 25),
        Some((80, 25)),
        "the gate is open now: resizes go straight through"
    );
}
