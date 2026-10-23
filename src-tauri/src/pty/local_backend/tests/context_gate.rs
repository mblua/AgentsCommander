use super::{probe_child_in, screen_rows_if_child_alive, ChildLiveness, PtyInstance, StartupGate};
use crate::pty::context_scrape::ScreenRowsRead;
use crate::pty::idle_detector::IdleDetector;
use crate::pty::output::SessionIoFanout;
use crate::session::profile::IdleTuning;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// The statusline the agent painted, and the one that stays on the grid after it dies.
const ROW: &str = "  Context \u{2591}\u{2591}\u{2588} 42%";

/// A real ConPTY with a REAL CHILD on the far end, plus the real fanout.
///
/// The child is the entire point. This gate's whole job is to answer a question the grid
/// cannot - is the child alive? - so a fake child cannot be asked it wrongly, which means
/// it cannot be asked at all. `startup_gate_tests::conpty` already opens the real ConPTY
/// and builds the real `PtyInstance`; this fills in its `child: None`.
fn conpty_with_child(cols: u16, rows: u16) -> (Mutex<HashMap<Uuid, PtyInstance>>, Uuid) {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    // A child that sits and waits, the way an agent CLI does.
    let shell = if cfg!(windows) { "cmd.exe" } else { "sh" };
    let child = pair
        .slave
        .spawn_command(CommandBuilder::new(shell))
        .expect("spawn a real child");
    drop(pair.slave);

    let writer = pair.master.take_writer().expect("take_writer");
    let instance = PtyInstance {
        master: Arc::new(Mutex::new(pair.master)),
        writer: Arc::new(Mutex::new(writer)),
        child: Some(child),
        job: None,
        size: Mutex::new((cols, rows)),
        startup_gate: Mutex::new(StartupGate::Holding(None)),
        rendered: Arc::new(AtomicBool::new(false)),
    };

    let id = Uuid::new_v4();
    let mut map = HashMap::new();
    map.insert(id, instance);
    (Mutex::new(map), id)
}

fn fanout() -> SessionIoFanout {
    SessionIoFanout::new(
        Arc::new(Mutex::new(HashMap::new())),
        IdleDetector::new(|_| {}, |_| {}),
        None,
    )
}

fn paint(fanout: &SessionIoFanout, id: Uuid) {
    let token = fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 30, 120)
        .expect("register test session");
    fanout.handle_output(&token, &id.to_string(), ROW.as_bytes().to_vec());
}

/// Kill the child and wait until it is REALLY gone - which is NOT what
/// `portable_pty::Child::wait()` waits for, and finding that out cost this test a failing
/// run.
///
/// `TerminateProcess` is asynchronous, and `wait()` short-circuits on `try_wait()` ->
/// `is_complete()` -> `GetExitCodeProcess`. The exit code is readable ~14 ms (measured
/// here) BEFORE the kernel signals the process object, so `wait()` returns `Ok(1)` while
/// `WaitForSingleObject(h, 0)` still answers WAIT_TIMEOUT. That is #942 arriving from the
/// other side: `is_complete` is exactly the accessor AC's oracle refuses to trust, and
/// the oracle asks the stronger question on purpose. Production never notices the gap -
/// 14 ms against a 5 s tick - but a test that kills and reads in the same breath lands
/// inside it every time.
fn kill_and_await_real_exit(ptys: &Mutex<HashMap<Uuid, PtyInstance>>, id: Uuid) {
    {
        let mut map = ptys.lock().unwrap();
        let child = map
            .get_mut(&id)
            .expect("instance")
            .child
            .as_mut()
            .expect("child");
        child.kill().expect("kill the child");
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if !matches!(probe_child_in(ptys, id), ChildLiveness::Alive) {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("the oracle answered Alive for 10s after TerminateProcess: it cannot see deaths");
}

/// The live case: a real child, a real grid with a real statusline on it, and the gate
/// hands the rows straight through.
#[test]
fn rows_are_readable_while_the_child_is_alive() {
    let (ptys, id) = conpty_with_child(120, 30);
    let fanout = fanout();
    paint(&fanout, id);

    match screen_rows_if_child_alive(&ptys, &fanout, id) {
        ScreenRowsRead::Rows(rows) => assert_eq!(rows[0], ROW),
        ScreenRowsRead::Unavailable => panic!("a live child with a live parser must read"),
        ScreenRowsRead::SessionOver => panic!("the child is alive; the session is not over"),
    }
}

/// The case the whole gate exists for, and it is RED if the probe call is deleted.
///
/// The parser is never touched here, so the row stays on the frozen grid verbatim,
/// exactly as it does in production: a killed process cannot repaint, and no EOF ever
/// arrives to notice. Without the probe this reads a perfectly well-formed `42%` off a
/// dead session forever, and no rule expressible in the user pattern can tell.
#[test]
fn session_over_once_the_child_actually_exits() {
    let (ptys, id) = conpty_with_child(120, 30);
    let fanout = fanout();
    paint(&fanout, id);

    kill_and_await_real_exit(&ptys, id);

    assert!(
        matches!(
            screen_rows_if_child_alive(&ptys, &fanout, id),
            ScreenRowsRead::SessionOver
        ),
        "the child is gone, so the session is over - whatever the frozen grid still says"
    );

    // ...and the row really is still sitting there. This is the fact that makes the probe
    // load-bearing rather than belt-and-braces.
    assert_eq!(
        fanout.get_screen_rows(id).expect("the parser is untouched")[0],
        ROW,
        "if this goes red the grid started self-correcting and the gate premise changed"
    );
}

/// No instance at all is the other definite answer: `Gone`, so the session is over.
#[test]
fn session_over_when_there_is_no_pty_instance() {
    let (ptys, _id) = conpty_with_child(120, 30);
    let fanout = fanout();
    assert!(matches!(
        screen_rows_if_child_alive(&ptys, &fanout, Uuid::new_v4()),
        ScreenRowsRead::SessionOver
    ));
}

/// A live child whose parser is missing is a DESYNC, not a dead session, and the caller
/// must keep sampling it. This is the two-state collapse `ScreenRowsRead` exists to
/// prevent, pinned at the one seam where the real oracle is involved.
#[test]
fn a_live_child_with_no_parser_is_unavailable_not_over() {
    let (ptys, id) = conpty_with_child(120, 30);
    let fanout = fanout();
    // deliberately never registered with the fanout

    assert!(
        matches!(
            screen_rows_if_child_alive(&ptys, &fanout, id),
            ScreenRowsRead::Unavailable
        ),
        "the child is alive, so nothing here may claim the session is over"
    );
}

/// #1171, 9.1.4 (local half) - an unknown id is `Missing` here, where the container
/// backend answers `Gone` to the identical question.
///
/// Asserted against the fanout call the `screen_rows_since` override delegates to, and not
/// against a `LocalProcessBackend`, because that type cannot be built in a unit test: its
/// `GitWatcher` needs a Tauri `AppHandle` (`:95-96`), which is the same reason
/// `screen_rows_if_child_alive` above is a free function. The override adds nothing to
/// this call on purpose - **no child liveness probe** - so this is the whole of its
/// behavior for an unknown id.
#[test]
fn the_watcher_seam_reports_missing_for_an_unknown_id() {
    let fanout = fanout();

    assert!(
        matches!(
            fanout.get_screen_rows_since(Uuid::new_v4(), None),
            crate::pty::watchers::ScreenRowsSince::Missing
        ),
        "for a local process, parser-absence is a desync, never a statement about the child"
    );
}
