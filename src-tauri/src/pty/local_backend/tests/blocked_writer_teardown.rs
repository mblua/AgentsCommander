use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn ptys_with_nonreading_child() -> (Arc<Mutex<HashMap<Uuid, PtyInstance>>>, Uuid) {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open real ConPTY");
    let mut command = CommandBuilder::new("powershell.exe");
    command.arg("-NoProfile");
    command.arg("-NonInteractive");
    command.arg("-Command");
    command.arg("Start-Sleep -Seconds 30");
    let child = pair
        .slave
        .spawn_command(command)
        .expect("spawn non-reading child");
    drop(pair.slave);
    let writer = pair.master.take_writer().expect("take ConPTY writer");
    let id = Uuid::new_v4();
    let ptys = Arc::new(Mutex::new(HashMap::from([(
        id,
        PtyInstance {
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            child: Some(child),
            job: None,
            size: Mutex::new((120, 30)),
            startup_gate: Mutex::new(StartupGate::Holding(None)),
            rendered: Arc::new(AtomicBool::new(false)),
        },
    )])));
    (ptys, id)
}

// Real-ConPTY teardown evidence, kept out of the default parallel run.
//
// This spawns a genuine ConPTY child and relies on the OS pipe buffer filling so a
// 65,536-byte write stays blocked long enough to be observed within a ~250 ms detection
// window. That precondition (the `observed_block` assertion below) is parallel-load
// sensitive: it holds deterministically when the test runs in isolation, but under the
// full parallel `cargo test --lib` run the write can drain before detection, so the
// precondition flakes. The teardown/unblock behavior actually under test is sound; only
// the setup precondition is timing fragile, and it is a real platform boundary that
// cannot be reliably simulated under automated parallel load.
//
// The frozen plan therefore routes real-ConPTY behavior to manual Windows evidence
// (section 14 acceptance item 10: automated assertions everywhere, with ConPTY manual
// evidence only where the real platform boundary cannot be simulated; section 15 Windows
// manual ConPTY/API matrix). It stays `#[ignore]`d out of the default automated gate and
// is exercised on demand, in isolation, as part of that manual matrix:
//
// cargo test --lib real_blocked_conpty_writer_is_unblocked_by_session_teardown -- --ignored
// --test-threads=1
#[test]
#[ignore = "real ConPTY block precondition is parallel-load sensitive; run in isolation as manual \
    Windows matrix evidence (plan section 14 item 10 / section 15)"]
fn real_blocked_conpty_writer_is_unblocked_by_session_teardown() {
    let (ptys, id) = ptys_with_nonreading_child();
    let started = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let (done_tx, done_rx) = mpsc::channel();
    let writer_ptys = Arc::clone(&ptys);
    let writer_started = Arc::clone(&started);
    let writer_completed = Arc::clone(&completed);
    let writer = std::thread::spawn(move || {
        let chunk = vec![b'x'; crate::pty::backend::PTY_INPUT_MAX_BYTES];
        let outcome = (0..4096).try_for_each(|_| {
            writer_started.fetch_add(1, Ordering::SeqCst);
            write_to_local_pty(&writer_ptys, id, &chunk).map_err(|error| error.to_string())?;
            writer_completed.fetch_add(1, Ordering::SeqCst);
            Ok::<(), String>(())
        });
        done_tx.send(outcome).expect("publish writer completion");
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut observed_block = false;
    while Instant::now() < deadline {
        let before = (
            started.load(Ordering::SeqCst),
            completed.load(Ordering::SeqCst),
        );
        if before.0 > before.1 {
            std::thread::sleep(Duration::from_millis(250));
            let after = (
                started.load(Ordering::SeqCst),
                completed.load(Ordering::SeqCst),
            );
            if after == before {
                observed_block = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        observed_block,
        "the real ConPTY child must leave a writer blocked before teardown"
    );

    let kill_started = Instant::now();
    let mut instance = remove_local_pty(&ptys, id).expect("remove blocked PTY instance");
    if let Some(mut child) = instance.child.take() {
        child.kill().expect("terminate non-reading child");
    }
    drop(instance);
    assert!(
        kill_started.elapsed() < Duration::from_secs(2),
        "teardown must not wait on the blocked writer mutex"
    );
    let outcome = done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("child teardown must unblock the real ConPTY write");
    assert!(outcome.is_err(), "the broken PTY write must report failure");
    writer.join().expect("join blocked writer thread");
    assert!(!ptys.lock().unwrap().contains_key(&id));
}
