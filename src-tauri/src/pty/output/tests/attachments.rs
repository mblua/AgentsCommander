use super::*;

const SECOND_WINDOW: &str = "terminal-2";

fn new_sink() -> PtyOutputTestSink {
    Arc::new(Mutex::new(Vec::new()))
}

fn session_with_sink(fanout: &SessionIoFanout, sink: &PtyOutputTestSink) -> Uuid {
    let id = Uuid::new_v4();
    fanout
        .register_session(
            id,
            IdleTuning::DEFAULT,
            30,
            120,
            PtyOutputTarget::from_test_sink(Arc::clone(sink)),
        )
        .expect("register session with sink");
    id
}

fn attach(fanout: &SessionIoFanout, id: Uuid, label: &str) -> Option<PtyScreenSnapshot> {
    fanout
        .activate_terminal_output(id, label, true)
        .expect("attach")
}

/// Drives the flush by hand. Nothing on this path emits synchronously except the 64 KiB
/// threshold, and test builds hold the 16 ms timer, so every emit a test observes is the one
/// it asked for and no assertion races a task.
fn flush(fanout: &SessionIoFanout, id: Uuid) {
    fanout.flush_terminal_output_for_test(id);
}

fn events(sink: &PtyOutputTestSink) -> Vec<PtyOutputTestEvent> {
    sink.lock().expect("target sink").clone()
}

/// The non-UI consumers keep their order and their behaviour, and the UI is still served
/// last of all, which is what keeps a slow emit from delaying any of them. What changed is
/// that the UI step no longer EMITS inside the fanout: it appends to the session's 16 ms
/// batch, and the flush is what emits.
#[test]
fn fanout_characterization_preserves_raw_order_and_ui_is_last() {
    let id = Uuid::new_v4();
    let output_senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
    output_senders.lock().unwrap().insert(id, sender);
    let broadcaster = crate::web::broadcast::WsBroadcaster::new();
    let mut websocket_receiver = broadcaster.subscribe();
    let sink = new_sink();
    let fanout = SessionIoFanout::new(
        output_senders,
        IdleDetector::new(|_| {}, |_| {}),
        Some(broadcaster),
    );
    let token = fanout
        .register_session(
            id,
            IdleTuning::DEFAULT,
            30,
            120,
            PtyOutputTarget::from_test_sink(Arc::clone(&sink)),
        )
        .expect("register fanout");

    assert!(attach(&fanout, id, WINDOW).is_some());
    assert!(fanout.take_trace_for_test().is_empty());

    let bytes = b"characterized raw bytes".to_vec();
    fanout.handle_output(&token, &id.to_string(), bytes.clone());

    assert_eq!(
        fanout.take_trace_for_test(),
        vec![
            FanoutTraceEvent::TouchSilence,
            FanoutTraceEvent::PrintableActivity,
            FanoutTraceEvent::ResponseMarkers,
            FanoutTraceEvent::OutputSender,
            FanoutTraceEvent::ParserProcessed(1),
            FanoutTraceEvent::Websocket,
            FanoutTraceEvent::UiEmit,
        ]
    );
    assert_eq!(receiver.try_recv().expect("raw sender payload"), bytes);
    match websocket_receiver.try_recv().expect("websocket payload") {
        crate::web::broadcast::WsOutMsg::Binary(frame) => {
            assert_eq!(&frame[36..], bytes.as_slice());
        }
        other @ crate::web::broadcast::WsOutMsg::Text(_) => {
            panic!("expected websocket binary payload, got {other:?}")
        }
    }
    assert!(
        events(&sink).is_empty(),
        "the ingest coalesces; the flush is what emits"
    );

    flush(&fanout, id);
    let emitted = events(&sink);
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].1, bytes);
    assert_eq!(emitted[0].2, Some(1));
}

/// #2124 T12 - the real wiring through `handle_output`: the fanout's single
/// detector call reaches the burst filter, so a 64 B status-line chunk after
/// long silence neither resets the auto-close clock nor fires `on_busy`,
/// while 1024 B of real output does both.
#[test]
fn t12_handle_output_applies_the_burst_filter() {
    let busy_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let busy_calls_cb = Arc::clone(&busy_calls);
    let detector = IdleDetector::new(
        |_| {},
        move |_| {
            busy_calls_cb.fetch_add(1, Ordering::SeqCst);
        },
    );
    let fanout = SessionIoFanout::new(
        Arc::new(Mutex::new(HashMap::new())),
        Arc::clone(&detector),
        None,
    );
    let tuning = IdleTuning {
        burst_max_bytes: 1024,
        burst_window: Duration::from_secs(3),
        burst_prior_silence: Duration::from_secs(60),
        ..IdleTuning::DEFAULT
    };

    let id = Uuid::new_v4();
    let token = fanout
        .register_session(id, tuning, 30, 120, PtyOutputTarget::noop())
        .expect("register burst session");
    detector.set_auto_close_ages_for_test(
        id,
        Duration::from_secs(30 * 60),
        Duration::from_secs(31 * 60),
    );
    detector.set_pty_input_ready_for_test(id);

    fanout.handle_output(&token, &id.to_string(), vec![b'a'; 64]);
    assert_eq!(busy_calls.load(Ordering::SeqCst), 0);
    assert!(
        detector.silence_age(id).unwrap() >= Duration::from_secs(30 * 60),
        "the short chunk must not reset the auto-close clock"
    );

    // Control: the same path with real work resets the clock and flips the
    // idle dot.
    let control_id = Uuid::new_v4();
    let control_token = fanout
        .register_session(control_id, tuning, 30, 120, PtyOutputTarget::noop())
        .expect("register control session");
    detector.set_auto_close_ages_for_test(
        control_id,
        Duration::from_secs(30 * 60),
        Duration::from_secs(31 * 60),
    );
    detector.set_pty_input_ready_for_test(control_id);
    fanout.handle_output(&control_token, &control_id.to_string(), vec![b'a'; 1024]);
    assert_eq!(busy_calls.load(Ordering::SeqCst), 1);
    assert!(detector.silence_age(control_id).unwrap() < Duration::from_secs(1));
}

#[test]
fn output_targets_have_explicit_success_and_failure_results() {
    let payload = PtyOutputPayload {
        session_id: Uuid::new_v4().to_string(),
        data: b"target payload".to_vec(),
        sequence: Some(1),
    };
    let labels = vec![WINDOW.to_string()];
    assert_eq!(
        PtyOutputTarget::noop().emit_pty_output(&labels, payload.clone()),
        Ok(())
    );

    let sink = new_sink();
    let failing = PtyOutputTarget::failing_test_sink(Arc::clone(&sink));
    assert_eq!(
        failing.emit_pty_output(&labels, payload),
        Err(PtyOutputEmitError::Emit)
    );
    assert_eq!(events(&sink).len(), 1);
}

/// Section 5's review gate, executable.
///
/// Delivery gating is a map from session id to the SET of window labels watching it, so two
/// windows on one session hold two attachments and one window's detach cannot mute the
/// other. A single slot fails at the second attach, a single key or an uncounted set fails
/// at the first detach, and per-window bookkeeping without counts fails the same way.
#[test]
fn attachments_are_counted_by_window_label() {
    let fanout = fanout();
    let sink = new_sink();
    let id = session_with_sink(&fanout, &sink);
    let token = fanout.registration_token_for_test(id);

    attach(&fanout, id, WINDOW);
    attach(&fanout, id, SECOND_WINDOW);
    assert_eq!(
        fanout.attached_labels_for_test(id),
        vec![WINDOW.to_string(), SECOND_WINDOW.to_string()]
    );

    fanout.detach_terminal_output(id, SECOND_WINDOW);
    fanout.handle_output(&token, &id.to_string(), b"still watched".to_vec());
    flush(&fanout, id);
    let emitted = events(&sink);
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].1, b"still watched");

    fanout.detach_terminal_output(id, WINDOW);
    assert!(fanout.attached_labels_for_test(id).is_empty());
    fanout.handle_output(&token, &id.to_string(), b"unwatched".to_vec());
    flush(&fanout, id);
    assert_eq!(events(&sink).len(), 1);
}

/// Criterion M, and the case that IS #1363: two windows on two sessions, both delivering at
/// the same time with no reactivation. A single delivery slot passes every other test in
/// this family and fails this one, which is why it is here.
#[test]
fn two_windows_on_two_sessions_both_emit() {
    let fanout = fanout();
    let first = new_sink();
    let second = new_sink();
    let a = session_with_sink(&fanout, &first);
    let b = session_with_sink(&fanout, &second);
    let token_a = fanout.registration_token_for_test(a);
    let token_b = fanout.registration_token_for_test(b);

    attach(&fanout, a, WINDOW);
    attach(&fanout, b, SECOND_WINDOW);
    fanout.handle_output(&token_a, &a.to_string(), b"from a".to_vec());
    fanout.handle_output(&token_b, &b.to_string(), b"from b".to_vec());
    flush(&fanout, a);
    flush(&fanout, b);

    let from_a = events(&first);
    let from_b = events(&second);
    assert_eq!(from_a.len(), 1);
    assert_eq!(from_a[0].1, b"from a");
    assert_eq!(from_b.len(), 1);
    assert_eq!(from_b[0].1, b"from b");
}

/// The ingest emit, and its complement: an attached session's chunks produce payloads with
/// the right bytes and a monotonic sequence, and a session nobody attached produces nothing
/// and retains nothing.
#[test]
fn only_attached_sessions_emit_and_sequences_stay_monotonic() {
    let fanout = fanout();
    let watched_sink = new_sink();
    let hidden_sink = new_sink();
    let watched = session_with_sink(&fanout, &watched_sink);
    let hidden = session_with_sink(&fanout, &hidden_sink);
    let watched_token = fanout.registration_token_for_test(watched);
    let hidden_token = fanout.registration_token_for_test(hidden);

    attach(&fanout, watched, WINDOW);
    for chunk in [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()] {
        fanout.handle_output(&watched_token, &watched.to_string(), chunk.to_vec());
        flush(&fanout, watched);
    }
    let emitted = events(&watched_sink);
    assert_eq!(
        emitted
            .iter()
            .map(|event| (event.1.clone(), event.2))
            .collect::<Vec<_>>(),
        vec![
            (b"one".to_vec(), Some(1)),
            (b"two".to_vec(), Some(2)),
            (b"three".to_vec(), Some(3)),
        ]
    );

    fanout.handle_output(&hidden_token, &hidden.to_string(), b"unseen".to_vec());
    flush(&fanout, hidden);
    assert!(events(&hidden_sink).is_empty());
    assert_eq!(fanout.pending_output_bytes_for_test(hidden), None);
}

/// Criterion N. Detach is total and window local: an unattached session, a second detach
/// past zero and a destroyed session all succeed, and no window's detach can reach another
/// window's attachment.
#[test]
fn detach_is_idempotent_and_never_crosses_windows() {
    let fanout = fanout();
    let sink = new_sink();
    let a = session_with_sink(&fanout, &sink);
    let b = session_with_sink(&fanout, &sink);
    let token_a = fanout.registration_token_for_test(a);

    fanout.detach_terminal_output(a, WINDOW);
    attach(&fanout, a, WINDOW);
    attach(&fanout, b, SECOND_WINDOW);
    fanout.detach_terminal_output(b, SECOND_WINDOW);
    fanout.detach_terminal_output(b, SECOND_WINDOW);
    fanout.remove_session(b);
    fanout.detach_terminal_output(b, SECOND_WINDOW);

    assert_eq!(fanout.attached_labels_for_test(a), vec![WINDOW.to_string()]);
    fanout.handle_output(&token_a, &a.to_string(), b"a keeps delivering".to_vec());
    flush(&fanout, a);
    assert_eq!(events(&sink).len(), 1);
}

/// Criterion L. A session whose parser is gone still attaches and still emits, with no
/// sequence, and the client writes those bytes live: emitting nothing there is #955, the
/// permanently black terminal, and there is no recovery lane left to repair it. A rejected
/// attach, by contrast, changes nothing at all.
#[test]
fn an_unavailable_parser_still_attaches_and_emits_unsequenced() {
    let fanout = fanout();
    let sink = new_sink();
    let id = Uuid::new_v4();
    let token = fanout
        .register_session(
            id,
            IdleTuning::DEFAULT,
            1,
            1,
            PtyOutputTarget::from_test_sink(Arc::clone(&sink)),
        )
        .expect("register one-cell session");
    // A wide grapheme in a one-column grid removes this session's parser for good.
    fanout.handle_output(&token, &id.to_string(), "界".as_bytes().to_vec());

    assert!(matches!(
        fanout.activate_terminal_output(id, WINDOW, true),
        Ok(None)
    ));
    assert_eq!(
        fanout.attached_labels_for_test(id),
        vec![WINDOW.to_string()]
    );

    fanout.handle_output(&token, &id.to_string(), b"after the fault".to_vec());
    flush(&fanout, id);
    let emitted = events(&sink);
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].1, b"after the fault");
    assert_eq!(emitted[0].2, None);

    let absent = Uuid::new_v4();
    assert!(matches!(
        fanout.activate_terminal_output(absent, WINDOW, true),
        Err(TerminalOutputAttachError::SessionUnavailable)
    ));
    assert!(fanout.attached_labels_for_test(absent).is_empty());
}

/// Criterion O. Destroying the session releases its attachments and drops its pending
/// bytes in the backend, whatever the frontend did, and the ingest stops even with an
/// attachment outstanding.
#[test]
fn destroying_a_session_releases_its_attachments_and_pending_bytes() {
    let fanout = fanout();
    let sink = new_sink();
    let id = session_with_sink(&fanout, &sink);
    let token = fanout.registration_token_for_test(id);

    attach(&fanout, id, WINDOW);
    fanout.handle_output(&token, &id.to_string(), b"pending".to_vec());
    assert_eq!(
        fanout.pending_output_bytes_for_test(id),
        Some(b"pending".len())
    );

    fanout.remove_session(id);
    assert!(fanout.attached_labels_for_test(id).is_empty());
    assert_eq!(fanout.pending_output_bytes_for_test(id), None);
    flush(&fanout, id);
    fanout.handle_output(&token, &id.to_string(), b"after destroy".to_vec());
    assert!(events(&sink).is_empty());
}

/// Criterion O, the other release site. A destroyed window's attachments go with it, in the
/// backend, with no frontend call - and only that window's: this is what closes the leak
/// class rather than mitigating it, and why the frontend close hook need not block.
#[test]
fn destroying_a_window_releases_only_its_own_attachments() {
    let fanout = fanout();
    let shared_sink = new_sink();
    let solo_sink = new_sink();
    let shared = session_with_sink(&fanout, &shared_sink);
    let solo = session_with_sink(&fanout, &solo_sink);
    let shared_token = fanout.registration_token_for_test(shared);
    let solo_token = fanout.registration_token_for_test(solo);

    attach(&fanout, shared, WINDOW);
    attach(&fanout, shared, SECOND_WINDOW);
    attach(&fanout, solo, SECOND_WINDOW);

    fanout.release_window_attachments(SECOND_WINDOW);
    assert_eq!(
        fanout.attached_labels_for_test(shared),
        vec![WINDOW.to_string()]
    );
    assert!(fanout.attached_labels_for_test(solo).is_empty());

    fanout.handle_output(&shared_token, &shared.to_string(), b"survives".to_vec());
    fanout.handle_output(&solo_token, &solo.to_string(), b"silenced".to_vec());
    flush(&fanout, shared);
    flush(&fanout, solo);
    assert_eq!(events(&shared_sink).len(), 1);
    assert!(events(&solo_sink).is_empty());
}

/// Section 3.4.3 rule 4. A detach DRAINS rather than skipping: a flush that fires after it
/// emits nothing, and the bytes it would have carried are gone rather than waiting to
/// surface on a later re-attach, out of order and after that attach's reset.
#[test]
fn a_flush_after_detach_emits_nothing_and_drops_the_bytes() {
    let fanout = fanout();
    let sink = new_sink();
    let id = session_with_sink(&fanout, &sink);
    let token = fanout.registration_token_for_test(id);

    attach(&fanout, id, WINDOW);
    fanout.handle_output(&token, &id.to_string(), b"never delivered".to_vec());
    fanout.detach_terminal_output(id, WINDOW);
    assert_eq!(fanout.pending_output_bytes_for_test(id), None);

    flush(&fanout, id);
    attach(&fanout, id, WINDOW);
    flush(&fanout, id);
    assert!(events(&sink).is_empty());
}

/// Section 3.4.1. The attach cuts the batch exactly at the snapshot: what was pending goes
/// to the windows already watching, the snapshot carries every byte up to its own sequence,
/// and the ingest continues from there. No batch straddles the boundary, which is what makes
/// one scalar `sequence` sufficient to describe a coalesced batch.
#[test]
fn attach_cuts_the_batch_exactly_at_the_snapshot_boundary() {
    let fanout = fanout();
    let sink = new_sink();
    let id = session_with_sink(&fanout, &sink);
    let token = fanout.registration_token_for_test(id);

    attach(&fanout, id, WINDOW);
    fanout.handle_output(&token, &id.to_string(), b"before\r\n".to_vec());

    let snapshot = attach(&fanout, id, SECOND_WINDOW).expect("snapshot on attach");
    assert_eq!(snapshot.sequence, 1);
    assert!(String::from_utf8_lossy(&snapshot.data).contains("before"));
    assert_eq!(fanout.pending_output_bytes_for_test(id), Some(0));
    let seeded = events(&sink);
    assert_eq!(
        seeded.len(),
        1,
        "the window already watching keeps its bytes"
    );
    assert_eq!(seeded[0].1, b"before\r\n");
    assert_eq!(seeded[0].2, Some(1));

    fanout.handle_output(&token, &id.to_string(), b"after".to_vec());
    flush(&fanout, id);
    let emitted = events(&sink);
    assert_eq!(emitted.len(), 2);
    assert_eq!(emitted[1].1, b"after");
    assert_eq!(emitted[1].2, Some(2));
}

/// #1439. A parser grid that diverged from the grid the ConPTY last took must never seed
/// an attach: the attach returns no snapshot, converges the parser onto the RECORDED
/// grid, and the next attach seeds cleanly at that grid with the current sequence.
///
/// Grid constraints are load-bearing: registration R0=30x120 (from `session_with_sink`),
/// follow A=24x80, desync B=50x132 are pairwise distinct, each asymmetric, and no pair is
/// a transposition of another. With A != R0, an implementation that never writes
/// `conpty_size` converges onto R0 and the final grid assertion turns red; asymmetric,
/// non-transposed grids make a rows/cols argument swap fail loudly (the #973 bug class).
#[test]
fn a_grid_divergence_yields_no_seed_and_the_next_attach_seeds_clean() {
    let fanout = fanout();
    let sink = new_sink();
    let id = session_with_sink(&fanout, &sink);
    let token = fanout.registration_token_for_test(id);

    // One follow the ConPTY took: parser and record both move to A = 24 rows x 80 cols.
    fanout.resize_screen_and_broadcast(id, 80, 24);
    fanout.handle_output(&token, &id.to_string(), b"seed me\r\n".to_vec());

    // The divergence class: the parser grid moves, the record does not.
    fanout.desync_screen_size_for_test(id, 50, 132);

    let seed = attach(&fanout, id, WINDOW);
    assert!(seed.is_none(), "a diverged grid must not seed the attach");
    // No batch entry exists yet: the only chunk so far arrived unattached, and a
    // seedless attach creates none (same `None` the detach test observes).
    assert_eq!(fanout.pending_output_bytes_for_test(id), None);

    // The seedless window still attached: live bytes keep flowing to it, sequenced.
    fanout.handle_output(&token, &id.to_string(), b"after divergence".to_vec());
    flush(&fanout, id);
    let emitted = events(&sink);
    assert_eq!(
        emitted.len(),
        1,
        "the attached window keeps receiving its bytes"
    );
    assert_eq!(emitted[0].1, b"after divergence");
    assert_eq!(emitted[0].2, Some(2));

    let reseeded = attach(&fanout, id, SECOND_WINDOW).expect("the attach after convergence seeds");
    assert_eq!(
        (reseeded.rows, reseeded.cols),
        (24, 80),
        "the parser converged onto the recorded ConPTY grid"
    );
    assert_eq!(reseeded.sequence, 2);
    assert_eq!(fanout.pending_output_bytes_for_test(id), Some(0));
}

/// Section 3.4.3 rule 5. The batch is flushed at the `Available -> Unavailable` transition,
/// so none of them mixes sequenced and unsequenced bytes. Labelled with the last real
/// sequence a mixed batch would make the client watermark-drop bytes that were never
/// seeded; labelled with none, its sequenced prefix would escape reconciliation and
/// duplicate against the seed.
#[test]
fn a_parser_fault_flushes_at_the_transition_and_unsequences_everything_after() {
    let fanout = fanout();
    let sink = new_sink();
    let id = session_with_sink(&fanout, &sink);
    let token = fanout.registration_token_for_test(id);

    attach(&fanout, id, WINDOW);
    fanout.handle_output(&token, &id.to_string(), b"ok".to_vec());
    // One step from overflow, so the next chunk cannot be sequenced and the parser goes
    // `Unavailable` for good.
    fanout.exhaust_output_sequence_for_test(id);
    fanout.handle_output(&token, &id.to_string(), b"faulting".to_vec());
    flush(&fanout, id);
    fanout.handle_output(&token, &id.to_string(), b"later".to_vec());
    flush(&fanout, id);

    let emitted = events(&sink);
    assert_eq!(emitted.len(), 3);
    assert_eq!(
        (emitted[0].1.clone(), emitted[0].2),
        (b"ok".to_vec(), Some(1)),
        "the sequenced prefix is flushed at the transition"
    );
    assert_eq!(
        (emitted[1].1.clone(), emitted[1].2),
        (b"faulting".to_vec(), None)
    );
    assert_eq!(
        (emitted[2].1.clone(), emitted[2].2),
        (b"later".to_vec(), None)
    );
}

/// Criterion E'. The flush fires when the batch REACHES its ceiling, which happens after
/// appending the chunk that crossed it, so accumulation peaks at 64 KiB plus one ingest
/// chunk. It runs on the reader thread, and that is the only backpressure left on this path.
#[test]
fn the_batch_flushes_on_the_reader_thread_at_its_ceiling() {
    let fanout = fanout();
    let sink = new_sink();
    let id = session_with_sink(&fanout, &sink);
    let token = fanout.registration_token_for_test(id);
    attach(&fanout, id, WINDOW);

    let chunk = vec![b'.'; 4_096];
    for _ in 0..(UI_BATCH_LIMIT_BYTES / chunk.len()) {
        fanout.handle_output(&token, &id.to_string(), chunk.clone());
    }

    let emitted = events(&sink);
    assert_eq!(
        emitted.len(),
        1,
        "no timer, no explicit flush: the ceiling did it"
    );
    assert_eq!(emitted[0].1.len(), UI_BATCH_LIMIT_BYTES);
    assert_eq!(fanout.pending_output_bytes_for_test(id), Some(0));
}

/// 3.4.1's no-zombie invariant, at the one interleaving that broke it: an attach that
/// lands while `remove_session` is draining must not leave a label behind for a session
/// that is about to disappear. The reader lease is the synchronisation point and no sleep
/// is needed - `remove_session` parks in `wait_for_drain` until it is released, and that
/// is precisely the window the attach used to slip through.
#[test]
fn an_attach_racing_remove_session_leaves_no_label_behind() {
    let sink = new_sink();
    let fanout = fanout();
    let id = session_with_sink(&fanout, &sink);
    let token = fanout
        .registration_token_for_session(id)
        .expect("registration token");
    let lease = fanout.acquire_reader_lease(&token).expect("reader lease");

    std::thread::scope(|scope| {
        let destroyer = scope.spawn(|| fanout.remove_session(id));
        // The gate refusing a new lease IS `remove_session` past its first phase.
        while fanout.acquire_reader_lease(&token).is_some() {
            std::thread::yield_now();
        }
        attach(&fanout, id, WINDOW);
        drop(lease);
        destroyer.join().expect("remove_session thread");
    });

    assert!(fanout.attached_labels_for_test(id).is_empty());
    assert_eq!(fanout.pending_output_bytes_for_test(id), None);
    let raw_fanout = super::super::SessionIoFanout::new(
        Arc::new(Mutex::new(HashMap::new())),
        IdleDetector::new(|_| {}, |_| {}),
        None,
    );
    let raw_token = raw_fanout
        .register_session_for_test(id, IdleTuning::DEFAULT, 4, 40)
        .unwrap();
    let parsers = raw_fanout.screen_parsers.clone();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let resume_rx = Mutex::new(resume_rx);
    assert!(raw_fanout
        .idle_detector
        .bind_raw_output_observer(1, move |_, _| {
            assert!(
                parsers.try_lock().is_ok(),
                "raw policy holds no parser mutex"
            );
            entered_tx.send(()).unwrap();
            resume_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }));
    std::thread::scope(|scope| {
        let reader =
            scope.spawn(|| raw_fanout.observe_raw_read_for_reader(&raw_token, 1, Instant::now()));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (removed_tx, removed_rx) = std::sync::mpsc::channel();
        let raw_ref = &raw_fanout;
        let destroyer = scope.spawn(move || {
            raw_ref.remove_session(id);
            removed_tx.send(()).unwrap();
        });
        while raw_fanout.acquire_reader_lease(&raw_token).is_some() {
            std::thread::yield_now();
        }
        assert!(
            removed_rx.try_recv().is_err(),
            "admitted raw callback retains retirement lease"
        );
        assert!(!raw_fanout.observe_raw_read_for_reader(&raw_token, 1, Instant::now()));
        resume_tx.send(()).unwrap();
        assert!(reader.join().unwrap());
        destroyer.join().unwrap();
        removed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    });
}

#[test]
fn retired_reader_tokens_cannot_target_a_same_uuid_replacement() {
    let id = Uuid::new_v4();
    let old_sink = new_sink();
    let new_sink_events = new_sink();
    let fanout = fanout();
    let old_token = fanout
        .register_session(
            id,
            IdleTuning::DEFAULT,
            4,
            40,
            PtyOutputTarget::from_test_sink(Arc::clone(&old_sink)),
        )
        .expect("register old session");
    assert!(!fanout.observe_raw_read_for_reader(&old_token, 0, Instant::now()));
    assert!(fanout.observe_raw_read_for_reader(&old_token, 1, Instant::now()));
    let foreign = super::super::SessionIoFanout::new(
        Arc::new(Mutex::new(HashMap::new())),
        IdleDetector::new(|_| {}, |_| {}),
        None,
    );
    assert!(!foreign.observe_raw_read_for_reader(&old_token, 1, Instant::now()));
    fanout.remove_session(id);
    assert!(!fanout.observe_raw_read_for_reader(&old_token, 1, Instant::now()));
    let new_token = fanout
        .register_session(
            id,
            IdleTuning::DEFAULT,
            4,
            40,
            PtyOutputTarget::from_test_sink(Arc::clone(&new_sink_events)),
        )
        .expect("register replacement session");

    let notices = Arc::new(Mutex::new(Vec::new()));
    let captured = notices.clone();
    assert!(fanout
        .idle_detector
        .bind_raw_output_observer(1, move |id, at| captured.lock().unwrap().push((id, at))));
    assert!(!fanout.observe_raw_read_for_reader(&old_token, 1, Instant::now()));
    assert!(notices.lock().unwrap().is_empty());
    assert!(fanout.observe_raw_read_for_reader(&new_token, 1, Instant::now()));
    assert_eq!(notices.lock().unwrap().len(), 1);

    fanout.handle_output(&old_token, &id.to_string(), b"old output".to_vec());
    assert_eq!(
        notices.lock().unwrap().len(),
        1,
        "retired segment cannot notify successor policy"
    );
    let snapshot = fanout
        .get_screen_snapshot(id)
        .expect("replacement snapshot");
    assert_eq!(snapshot.sequence, 0);
    assert!(!String::from_utf8_lossy(&snapshot.data).contains("old output"));
    assert!(events(&old_sink).is_empty());

    attach(&fanout, id, WINDOW);
    fanout.handle_output(&new_token, &id.to_string(), b"replacement output".to_vec());
    flush(&fanout, id);
    let emitted = events(&new_sink_events);
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].1, b"replacement output");
}

#[test]
fn response_marker_capture_writes_trimmed_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_id = Uuid::new_v4();
    let watchers: ResponseWatcherMap = Arc::new(Mutex::new(HashMap::new()));
    watchers.lock().unwrap().insert(
        (session_id, "r1".to_string()),
        ResponseWatcher {
            response_dir: dir.path().to_path_buf(),
            buffer: None,
            capturing: false,
        },
    );

    scan_response_markers(
        session_id,
        "before %%AC_RESPONSE::r1::START%% {\"ok\": true} %%AC_RESPONSE::r1::END%% after",
        &watchers,
    );

    let json = std::fs::read_to_string(dir.path().join("r1.json")).expect("response json");
    assert!(json.contains("\"requestId\": \"r1\""));
    assert!(json.contains("\"content\": \"{\\\"ok\\\": true}\""));
    assert!(watchers.lock().unwrap().is_empty());
}
