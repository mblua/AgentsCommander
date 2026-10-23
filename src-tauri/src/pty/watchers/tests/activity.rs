use super::engine::{as_refs, occurrence_watcher, Harness, Painted};
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use ActivityObservation::{Absent, Present, Retired, Unavailable, Validate};

const WORKING: &str = "• Working (5s • esc to interrupt)";

type Observed = Arc<Mutex<Vec<(Uuid, ActivityObservation)>>>;

/// A factory whose consumers accept and record every observation.
fn recording_factory() -> (ActivityFactory, Observed) {
    let observed: Observed = Arc::default();
    let sink = Arc::clone(&observed);
    let factory: ActivityFactory = Arc::new(move |id: Uuid| {
        let sink = Arc::clone(&sink);
        let consumer: ActivityConsumer = Arc::new(move |observation: ActivityObservation| {
            sink.lock().unwrap().push((id, observation));
            true
        });
        Some(consumer)
    });
    (factory, observed)
}

fn observations(observed: &Observed, id: Uuid) -> Vec<ActivityObservation> {
    observed
        .lock()
        .unwrap()
        .iter()
        .filter(|(observed_id, _)| *observed_id == id)
        .map(|(_, observation)| *observation)
        .collect()
}

#[tokio::test]
async fn e2992_p51_unavailable_mid_tick_retirement() {
    let h = Harness::new();
    let id = Uuid::new_v4();
    h.engine.register_session(id, Some("a1".into()));
    h.patterns.1.store(true, Ordering::Relaxed);
    h.backends
        .liveness
        .lock()
        .unwrap()
        .insert(id, ContextSessionLiveness::SessionOver);
    h.engine.ticks.store(24, Ordering::Relaxed);
    let (entered, handshake) = tokio::sync::oneshot::channel();
    let entered = std::sync::Mutex::new(Some(entered));
    let (release, resume) = std::sync::mpsc::sync_channel(1);
    let resume = std::sync::Mutex::new(resume);
    *h.backends.on_liveness.lock().unwrap() = Some(Arc::new(move |_| {
        entered.lock().unwrap().take().unwrap().send(()).unwrap();
        resume
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
    }));
    let engine = Arc::clone(&h.engine);
    let tick = tokio::task::spawn_blocking(move || futures::executor::block_on(engine.tick()));
    tokio::time::timeout(Duration::from_secs(2), handshake)
        .await
        .unwrap()
        .unwrap();
    h.engine.retire_session(id);
    let replacement = Uuid::new_v4();
    h.engine.register_session(replacement, Some("a1".into()));
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), tick)
        .await
        .unwrap()
        .unwrap();
    *h.backends.on_liveness.lock().unwrap() = None;
    assert!(!h.engine.is_session_registered(id));
    assert!(h.engine.is_session_registered(replacement));
    assert_eq!(h.backend.reads.load(Ordering::Relaxed), 0);
    assert_eq!(h.batches(), 0);

    let shutdown = crate::shutdown::ShutdownSignal::new();
    let mut completed_ticks = 0;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.token().cancelled() => break,
                _ = h.engine.tick() => {
                    completed_ticks += 1;
                    if completed_ticks == 3 { shutdown.trigger(); }
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(completed_ticks, 3);
}

#[tokio::test]
async fn e2992_p51_watchers_unavailable_preserves() {
    let (factory, observed) = recording_factory();
    let h = Harness::with_activity_factory(factory);
    let id = Uuid::new_v4();
    h.engine.register_session(id, Some("a1".to_string()));
    h.configure(
        "a1",
        vec![super::engine::state_watcher("ctx", r"Context (\d+)%")],
    );
    h.backend.paint(id, &["Context 42%", WORKING]);
    h.engine.tick().await;
    assert_eq!(h.emitted().len(), 1);
    let history = serde_json::to_value(h.history.snapshot(id, None)).unwrap();
    let (stamp, seq) = {
        let registered = h.engine.registered.lock().unwrap();
        let session = registered.get(&id).unwrap();
        (session.stamp, session.seq)
    };
    h.patterns.1.store(true, Ordering::Relaxed);
    h.backend.paint(id, &["Context 70%"]);
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(1), h.engine.tick())
            .await
            .unwrap();
    }
    assert_eq!(h.emitted().len(), 1);
    assert_eq!(h.engine.compile_count(), 1);
    assert_eq!(
        serde_json::to_value(h.history.snapshot(id, None)).unwrap(),
        history
    );
    {
        let registered = h.engine.registered.lock().unwrap();
        let session = registered.get(&id).unwrap();
        assert_eq!((session.stamp, session.seq), (stamp, seq));
    }
    assert_eq!(observations(&observed, id).last(), Some(&Absent));
    h.patterns.0.lock().unwrap().clear();
    h.patterns.1.store(false, Ordering::Relaxed);
    h.engine.tick().await;
    assert!(h.history.snapshot(id, None).active_watchers.is_empty());
    h.configure(
        "a1",
        vec![super::engine::state_watcher("ctx", r"Context (\d+)%")],
    );
    h.backend.paint(id, &["Context 70%"]);
    h.engine.tick().await;
    assert_eq!(h.emitted().len(), 2);
}

#[tokio::test]
async fn e2992_p51_unavailable_progress_and_retirement() {
    let engine_cell: Arc<std::sync::OnceLock<std::sync::Weak<WatcherEngine>>> = Arc::default();
    let guards_free: Arc<Mutex<Vec<bool>>> = Arc::default();
    let (inner, observed) = recording_factory();
    let factory: ActivityFactory = {
        let (engine_cell, guards_free) = (Arc::clone(&engine_cell), Arc::clone(&guards_free));
        Arc::new(move |id: Uuid| {
            let inner = inner(id)?;
            let (engine_cell, guards_free) = (Arc::clone(&engine_cell), Arc::clone(&guards_free));
            let consumer: ActivityConsumer = Arc::new(move |observation| {
                if let Some(engine) = engine_cell.get().and_then(std::sync::Weak::upgrade) {
                    guards_free.lock().unwrap().push(
                        engine.sequence.try_lock().is_ok() && engine.registered.try_lock().is_ok(),
                    );
                }
                inner(observation)
            });
            Some(consumer)
        })
    };
    let h = Harness::with_activity_factory(factory);
    engine_cell.set(Arc::downgrade(&h.engine)).unwrap();
    let mixed = Uuid::new_v4();
    let internal = Uuid::new_v4();
    h.engine.register_session(mixed, Some("a1".to_string()));
    h.engine.register_session(internal, None);
    h.backend.paint(mixed, &[WORKING]);
    h.backend.paint(internal, &[WORKING]);
    h.patterns.1.store(true, Ordering::Relaxed);
    h.ticks(24).await;
    assert_eq!(h.backends.liveness_calls.load(Ordering::Relaxed), 0);
    h.backends
        .liveness
        .lock()
        .unwrap()
        .insert(mixed, ContextSessionLiveness::SessionOver);
    let reads = h.backend.reads.load(Ordering::Relaxed);
    h.engine.tick().await;
    assert_eq!(h.backends.liveness_calls.load(Ordering::Relaxed), 2);
    assert_eq!(h.backend.reads.load(Ordering::Relaxed), reads + 1);
    assert!(!h.engine.is_session_registered(mixed));
    assert!(h.engine.is_session_registered(internal));
    assert_eq!(observations(&observed, mixed).last(), Some(&Retired));
    assert!(guards_free.lock().unwrap().iter().all(|free| *free));
    assert_eq!(h.batches(), 0);
    assert!(!h.history.snapshot(internal, None).warmed_up);
    h.engine.retire_session(internal);
    h.engine.tick().await;
    assert!(!h.engine.is_session_registered(internal));

    // A registry without internal consumers is still probed on tick25,
    // but never reads rows or publishes unwatched history while unavailable.
    let h = Harness::new();
    let id = Uuid::new_v4();
    h.engine.register_session(id, Some("a1".to_string()));
    h.patterns.1.store(true, Ordering::Relaxed);
    h.backends
        .liveness
        .lock()
        .unwrap()
        .insert(id, ContextSessionLiveness::SessionOver);
    h.ticks(25).await;
    assert_eq!(h.backends.liveness_calls.load(Ordering::Relaxed), 1);
    assert_eq!(h.backend.reads.load(Ordering::Relaxed), 0);
    assert!(!h.engine.is_session_registered(id));
    assert!(!h.history.snapshot(id, None).warmed_up);
}

/// Sanitized from shared dev-live-snapshot.json, sequence 1342, 30x120.
/// SHA256: 243ffe6321aacf4c2dc821e9e12ad422109aef8fc62b3b691ca3167a0f17c6a9.
/// Wrap geometry and row 22 are preserved; unrelated transcript and Tip text are
/// substituted. This is not a byte-for-byte terminal replay.
fn i3059_sanitized_frame() -> ScreenFrame {
    let mut rows = vec![String::new(); 30];
    for row in &mut rows[..22] {
        *row = "generic transcript".to_string();
    }
    rows[22] = "Working (35s • esc to interrupt)".to_string();
    rows[23] = "  └ Tip: generic guidance".to_string();
    let mut wrapped = vec![false; 30];
    wrapped[..24].fill(true);
    ScreenFrame {
        rows,
        wrapped,
        cursor_row: 26,
        stamp: Some(FrameStamp {
            sequence: 1342,
            rows: 30,
            cols: 120,
        }),
    }
}

#[test]
fn i3059_fixture_preserves_snapshot_geometry() {
    let frame = i3059_sanitized_frame();
    let pattern = pattern::compile(CODEX_WORKING_PATTERN).expect("compiles");
    assert_eq!(frame.rows.len(), 30);
    assert_eq!(frame.stamp.unwrap().rows, 30);
    assert_eq!(frame.stamp.unwrap().cols, 120);
    assert_eq!(frame.wrapped, [vec![true; 24], vec![false; 6]].concat());
    assert_eq!(
        frame
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| pattern.regex().is_match(row))
            .map(|(index, _)| index)
            .collect::<Vec<_>>(),
        vec![22]
    );
    let logical = frame::logical_rows(&frame);
    assert_eq!(
        logical.iter().map(|row| row.start).collect::<Vec<_>>(),
        vec![25, 26, 27, 28, 29],
        "the shared policy still omits the top wrap group"
    );
    assert!(logical
        .iter()
        .all(|row| !pattern.regex().is_match(&row.text)));
}

#[tokio::test]
async fn i3059_top_wrapped_physical_indicator_then_worked() {
    let (factory, observed) = recording_factory();
    let harness = Harness::with_activity_factory(factory);
    let id = Uuid::new_v4();
    harness.engine.register_session(id, None);
    let mut frame = i3059_sanitized_frame();
    harness.backend.push(
        id,
        Painted::Frame(frame.rows.clone(), frame.wrapped.clone(), frame.cursor_row),
    );
    harness.ticks(1).await;
    assert_eq!(observations(&observed, id), vec![Validate, Present]);

    frame.rows[22] = "Worked (35s • esc to interrupt)".to_string();
    harness.backend.push(
        id,
        Painted::Frame(frame.rows, frame.wrapped, frame.cursor_row),
    );
    harness.ticks(1).await;
    assert_eq!(observations(&observed, id), vec![Validate, Present, Absent]);
    assert_eq!(
        *harness.backend.seen.lock().unwrap(),
        vec![
            None,
            Some(FrameStamp {
                sequence: 1,
                rows: 30,
                cols: 120,
            }),
        ],
        "the backend supplies fresh 30x120 stamps"
    );
}

#[tokio::test]
async fn i3059_non_top_indicator_joined_with_tip() {
    let (factory, observed) = recording_factory();
    let harness = Harness::with_activity_factory(factory);
    let id = Uuid::new_v4();
    harness.engine.register_session(id, None);
    let frame = ScreenFrame {
        rows: vec!["transcript".into(), WORKING.into(), "Tip: guidance".into()],
        wrapped: vec![false, true, false],
        cursor_row: 0,
        stamp: None,
    };
    let pattern = pattern::compile(CODEX_WORKING_PATTERN).expect("compiles");
    let logical = frame::logical_rows(&frame);
    assert_eq!((logical[1].start, logical[1].end), (1, 2));
    assert_eq!(logical[1].text, format!("{WORKING}Tip: guidance"));
    assert!(logical
        .iter()
        .all(|row| !pattern.regex().is_match(&row.text)));
    harness.backend.push(
        id,
        Painted::Frame(frame.rows, frame.wrapped, frame.cursor_row),
    );
    harness.ticks(1).await;
    assert_eq!(observations(&observed, id), vec![Validate, Present]);
}

#[tokio::test]
async fn i3059_split_indicator_keeps_logical_recognition() {
    let (factory, observed) = recording_factory();
    let harness = Harness::with_activity_factory(factory);
    let id = Uuid::new_v4();
    harness.engine.register_session(id, None);
    let frame = ScreenFrame {
        rows: vec![
            "transcript".into(),
            "Working (1m 0".into(),
            "5s • esc to interrupt)".into(),
        ],
        wrapped: vec![false, true, false],
        cursor_row: 0,
        stamp: None,
    };
    let pattern = pattern::compile(CODEX_WORKING_PATTERN).expect("compiles");
    assert!(frame.rows.iter().all(|row| !pattern.regex().is_match(row)));
    assert!(frame::logical_rows(&frame)
        .iter()
        .any(|row| pattern.regex().is_match(&row.text)));
    harness.backend.push(
        id,
        Painted::Frame(frame.rows, frame.wrapped, frame.cursor_row),
    );
    harness.ticks(1).await;
    assert_eq!(observations(&observed, id), vec![Validate, Present]);
}

#[tokio::test]
async fn i3059_anchored_negatives_deliver_absent() {
    let (factory, observed) = recording_factory();
    let harness = Harness::with_activity_factory(factory);
    let id = Uuid::new_v4();
    harness.engine.register_session(id, None);
    for negative in [
        "Worked (5s • esc to interrupt)",
        "› Working (5s • esc to interrupt)",
        "Working (5s • esc to",
        "Working (5s • esc to interrupt) unrelated suffix",
    ] {
        harness.backend.push(
            id,
            Painted::Frame(
                vec!["transcript".into(), negative.into()],
                vec![false; 2],
                0,
            ),
        );
    }
    harness.ticks(4).await;
    assert_eq!(
        observations(&observed, id),
        vec![Validate, Absent, Absent, Absent, Absent]
    );
}

/// The production pattern: both glyph prefixes or none, `s`/`m`/`h` durations, inline
/// context, every logical row of a multi-row frame; and the near misses it must refuse.
#[test]
fn i2879_feature_grammar() {
    let pattern = pattern::compile(CODEX_WORKING_PATTERN).expect("compiles");
    for row in [
        "Working (0s • esc to interrupt)",
        "• Working (12s • esc to interrupt)",
        "◦ Working (59s • esc to interrupt)",
        "  Working (1m 05s • esc to interrupt)",
        "\tWorking (59m 59s • esc to interrupt)  ",
        "Working (1h 00m 00s • esc to interrupt)",
        "Working  (3s  •  esc  to  interrupt)",
        "Working (12h 03m 09s • esc to interrupt) · Running hook",
        "• Working (3s • esc to interrupt) · Reading src/main.r…",
    ] {
        assert!(pattern.regex().is_match(row), "must match: {row:?}");
    }
    for row in [
        "Working",
        "Working (",
        "Worked for 1m 05s",
        "Worked (5s • esc to interrupt)",
        "⠋ Working (5s • esc to interrupt)",
        "* Working (5s • esc to interrupt)",
        "› Working (5s • esc to interrupt)",
        "•Working (5s • esc to interrupt)",
        "Working(5s • esc to interrupt)",
        "Working (5s • esc to interrupt",
        "Working (5s • esc to",
        "Working (5s)",
        "Working (5s • ctrl+c to interrupt)",
        "Working (5s • esc to interrupt) context",
        "Working (5s • esc to interrupt) · ",
        "Working (1m 60s • esc to interrupt)",
        "Working (1m 5s • esc to interrupt)",
        "Working (0m 05s • esc to interrupt)",
        "Working (01m 05s • esc to interrupt)",
        "Working (1h 60m 00s • esc to interrupt)",
        "Working (5 s • esc to interrupt)",
    ] {
        assert!(!pattern.regex().is_match(row), "must not match: {row:?}");
    }

    let matching = |rows: &[&str], wrapped: &[bool]| -> Vec<(usize, usize)> {
        let frame = ScreenFrame {
            rows: rows.iter().map(|row| row.to_string()).collect(),
            wrapped: wrapped.to_vec(),
            cursor_row: 0,
            stamp: None,
        };
        frame::logical_rows(&frame)
            .iter()
            .filter(|row| pattern.regex().is_match(&row.text))
            .map(|row| (row.start, row.end))
            .collect()
    };
    assert_eq!(
        matching(
            &[
                "› transcript",
                "Working (2h 03m 09s • esc to interrupt)",
                "",
                "› prompt"
            ],
            &[false; 4]
        ),
        vec![(1, 1)],
        "any row of the frame, not only the cursor's"
    );
    assert_eq!(
        matching(
            &["x", "◦ Working (1m 0", "5s • esc to interrupt) · ctx"],
            &[false, true, false]
        ),
        vec![(1, 2)],
        "a wrapped indicator is one logical row"
    );
}

/// After `Missing`, the consumer re-reads with `seen = None` - through an `Unchanged`
/// too - until a complete frame arrives; an identical frame then recovers `Present`
/// and a negative one releases it.
#[tokio::test]
async fn e2992_p51_unavailable_missing_next_full_read() {
    let (factory, observed) = recording_factory();
    let harness = Harness::with_activity_factory(factory);
    harness.patterns.1.store(true, Ordering::Relaxed);
    let id = Uuid::new_v4();
    harness.engine.register_session(id, None);
    harness.backend.paint(id, &["› run", WORKING]);
    harness.backend.push(id, Painted::Missing);
    harness.backend.push(id, Painted::Missing);
    harness.backend.push(id, Painted::Unchanged);
    harness.backend.push(id, Painted::Same);
    harness.backend.paint(id, &["› run", "Worked for 5s"]);

    harness.ticks(5).await;
    assert_eq!(harness.batches(), 0);
    assert!(!harness.history.snapshot(id, None).warmed_up);
    assert!(harness.engine.is_session_registered(id));

    let first = Some(FrameStamp {
        sequence: 1,
        rows: 2,
        cols: 120,
    });
    assert_eq!(
        *harness.backend.seen.lock().unwrap(),
        vec![None, first, first, None, None, first],
        "Missing (read and re-resolved) forces full reads until a frame is complete"
    );
    assert_eq!(
        observations(&observed, id),
        vec![Validate, Present, Unavailable, Present, Absent]
    );
}

#[tokio::test]
async fn i2879_feature_missing_recovery() {
    let (factory, observed) = recording_factory();
    let harness = Harness::with_activity_factory(factory);
    let id = Uuid::new_v4();
    harness.engine.register_session(id, None);
    harness.backend.paint(id, &["› run", WORKING]);
    harness.backend.push(id, Painted::Missing);
    harness.backend.push(id, Painted::Missing);
    harness.backend.push(id, Painted::Unchanged);
    harness.backend.push(id, Painted::Same);
    harness.backend.paint(id, &["› run", "Worked for 5s"]);

    harness.ticks(5).await;

    let first = Some(FrameStamp {
        sequence: 1,
        rows: 2,
        cols: 120,
    });
    assert_eq!(
        *harness.backend.seen.lock().unwrap(),
        vec![None, first, first, None, None, first],
        "Missing (read and re-resolved) forces full reads until a frame is complete"
    );
    assert_eq!(
        observations(&observed, id),
        vec![Validate, Present, Unavailable, Present, Absent]
    );
}

/// With no user watcher and no agent, an internal consumer still reads and observes,
/// with every engine guard released at each callback, and leaves sink and history
/// empty. Without a consumer, or when the factory declines, nothing is even read.
#[tokio::test]
async fn i2879_feature_internal_only() {
    let engine_cell: Arc<std::sync::OnceLock<std::sync::Weak<WatcherEngine>>> = Arc::default();
    let guards_free: Arc<Mutex<Vec<bool>>> = Arc::default();
    let (inner, observed) = recording_factory();
    let factory: ActivityFactory = {
        let (engine_cell, guards_free) = (Arc::clone(&engine_cell), Arc::clone(&guards_free));
        Arc::new(move |id: Uuid| {
            let inner = inner(id)?;
            let (engine_cell, guards_free) = (Arc::clone(&engine_cell), Arc::clone(&guards_free));
            let consumer: ActivityConsumer = Arc::new(move |observation: ActivityObservation| {
                if let Some(engine) = engine_cell.get().and_then(std::sync::Weak::upgrade) {
                    let free =
                        engine.sequence.try_lock().is_ok() && engine.registered.try_lock().is_ok();
                    guards_free.lock().unwrap().push(free);
                }
                inner(observation)
            });
            Some(consumer)
        })
    };
    let harness = Harness::with_activity_factory(factory);
    engine_cell
        .set(Arc::downgrade(&harness.engine))
        .expect("set once");
    let id = Uuid::new_v4();
    harness.engine.register_session(id, None);
    assert!(harness.engine.is_session_registered(id));
    harness.backend.paint(id, &["› run", WORKING]);

    harness.ticks(2).await;

    assert_eq!(harness.backend.reads.load(Ordering::Relaxed), 2);
    assert_eq!(
        observations(&observed, id),
        vec![Validate, Present, Present]
    );
    assert_eq!(harness.batches(), 0, "nothing reaches the sink");
    assert!(
        !harness.history.snapshot(id, None).warmed_up,
        "an internal-only session publishes no history"
    );

    // A confirmed Gone retires it and the consumer hears so, outside the guards.
    harness.backend.push(id, Painted::Gone);
    harness.backend.push(id, Painted::Gone);
    harness.ticks(1).await;
    assert!(!harness.engine.is_session_registered(id));
    assert_eq!(observations(&observed, id).last(), Some(&Retired));
    assert_eq!(*guards_free.lock().unwrap(), vec![true; 4]);

    // The control: no factory, or a factory that declines, and no agent - zero reads.
    let declining: ActivityFactory = Arc::new(|_: Uuid| -> Option<ActivityConsumer> { None });
    for harness in [Harness::new(), Harness::with_activity_factory(declining)] {
        let shell = Uuid::new_v4();
        harness.engine.register_session(shell, None);
        harness.backend.paint(shell, &[WORKING]);
        harness.ticks(3).await;
        assert!(!harness.engine.is_session_registered(shell));
        assert_eq!(harness.backend.reads.load(Ordering::Relaxed), 0);
    }
}

/// What the user watchers produced, for comparing an engine with an internal consumer
/// against one without: the events, the counters and the frame-diff status.
type UserView = (Vec<(u64, String, String)>, u64, Vec<(String, u64, bool)>);

fn user_view(harness: &Harness, id: Uuid) -> UserView {
    let snapshot = harness.history.snapshot(id, None);
    (
        harness
            .emitted()
            .into_iter()
            .map(|m| (m.seq, m.watcher_id, m.row))
            .collect(),
        snapshot.possibly_missed_frames,
        snapshot
            .active_watchers
            .iter()
            .map(|c| (c.watcher_id.clone(), c.count, c.degraded))
            .collect(),
    )
}

/// User caps and suspension never slow the internal consumer, and its `Missing`
/// recovery read never advances the user diff or duplicates a user match: the user
/// view is identical to an engine that has no consumer at all.
#[tokio::test]
async fn i2879_feature_user_isolation() {
    let (factory, observed) = recording_factory();
    let with = Harness::with_activity_factory(factory);
    let without = Harness::new();
    let id = Uuid::new_v4();
    let transcript: Vec<String> = (0..1000).map(|i| format!("row {i}")).collect();
    for harness in [&with, &without] {
        harness.engine.register_session(id, Some("a1".to_string()));
        harness.configure(
            "a1",
            vec![
                occurrence_watcher("flood", "row", WatcherDedupe::None, 2000),
                occurrence_watcher("keep", "keep", WatcherDedupe::None, 2000),
            ],
        );
        for tick in 0..40usize {
            let mut rows = transcript[tick * 10..tick * 10 + 29].to_vec();
            rows.push(WORKING.to_string());
            harness.backend.paint(id, &as_refs(&rows));
        }
        harness.ticks(40).await;
        let (degraded, suspended, _) = harness.engine.watcher_state(id, "flood").expect("state");
        assert!(
            degraded && suspended,
            "the user pair is capped and suspended"
        );
    }
    assert_eq!(
        observations(&observed, id),
        [vec![Validate], vec![Present; 40]].concat(),
        "a suspended, saturated user watcher never slows the internal consumer"
    );
    assert_eq!(user_view(&with, id), user_view(&without, id));

    for harness in [&with, &without] {
        harness.backend.paint(id, &["keep", WORKING]);
        harness.backend.push(id, Painted::Missing);
        harness.backend.push(id, Painted::Missing);
        harness.backend.push(id, Painted::Same);
        harness.ticks(3).await;
    }
    assert_eq!(
        observations(&observed, id)[41..],
        [Present, Unavailable, Present],
        "the full recovery read restores the internal verdict"
    );
    assert_ne!(
        with.backend.seen.lock().unwrap().last(),
        without.backend.seen.lock().unwrap().last(),
        "the control: only the engine with a consumer made the full recovery read"
    );
    assert_eq!(
        user_view(&with, id),
        user_view(&without, id),
        "the recovery read is the user's Unchanged"
    );
    for harness in [&with, &without] {
        harness.ticks(2).await;
    }
    assert_eq!(user_view(&with, id), user_view(&without, id));
}

/// Real detector tokens through the production composition: a delivery delayed past a
/// replacement is rejected, the old attach's retirement does not release the new one,
/// and an attach invalidated before its validation never starts sampling.
#[test]
fn i2879_feature_attach_race() {
    use crate::session::profile::{idle_tuning_for, CodingAgentKind};
    use std::sync::mpsc;

    let codex = idle_tuning_for(Some(CodingAgentKind::Codex));
    let detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let real = crate::codex_activity_factory(&detector);
    type Log = Arc<Mutex<Vec<(usize, ActivityObservation, bool)>>>;
    let log: Log = Arc::default();
    let attaches = Arc::new(AtomicUsize::new(0));
    let (arrived_tx, arrived_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let gate = Arc::new(Mutex::new(Some((arrived_tx, release_rx))));
    let invalidate: Arc<Mutex<Option<(usize, Uuid)>>> = Arc::default();
    let factory: ActivityFactory = {
        let (log, attaches, gate, invalidate, detector) = (
            Arc::clone(&log),
            Arc::clone(&attaches),
            Arc::clone(&gate),
            Arc::clone(&invalidate),
            Arc::clone(&detector),
        );
        Arc::new(move |id: Uuid| {
            let inner = real(id)?;
            let n = attaches.fetch_add(1, Ordering::SeqCst) + 1;
            if *invalidate.lock().unwrap() == Some((n, id)) {
                // A re-registration lands between this attach and its validation.
                detector.register_session(id, codex);
            }
            let (log, gate) = (Arc::clone(&log), Arc::clone(&gate));
            let consumer: ActivityConsumer = Arc::new(move |observation: ActivityObservation| {
                if n == 1 && observation == Present {
                    let parked = gate.lock().unwrap().take();
                    if let Some((arrived, release)) = parked {
                        arrived.send(()).unwrap();
                        release.recv().unwrap();
                    }
                }
                let accepted = inner(observation);
                log.lock().unwrap().push((n, observation, accepted));
                accepted
            });
            Some(consumer)
        })
    };
    let harness = Harness::with_activity_factory(factory);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let id = Uuid::new_v4();
    detector.register_session(id, codex);
    harness.engine.register_session(id, None);
    harness.backend.paint(id, &[WORKING]);
    let ticker = {
        let engine = Arc::clone(&harness.engine);
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(engine.tick());
        })
    };
    // The tick has released every engine guard and is delivering attach 1's Present.
    // Replace the registration while that delivery is held back; this would deadlock
    // if the tick still held the engine's locks.
    arrived_rx.recv().expect("delivery reached");
    harness.engine.register_session(id, None);
    release_tx.send(()).expect("release");
    ticker.join().expect("ticker");
    rt.block_on(harness.ticks(1));
    harness.engine.retire_session(id);
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            (1, Validate, true),
            (2, Validate, true),
            (1, Retired, false),
            (1, Present, false),
            (2, Present, true),
            (2, Retired, true),
        ],
        "the late delivery and the old retirement are both rejected; the new attach lives"
    );

    // Identity invalidated during attach: validation fails, nothing is registered or read.
    let other = Uuid::new_v4();
    detector.register_session(other, codex);
    *invalidate.lock().unwrap() = Some((3, other));
    let reads = harness.backend.reads.load(Ordering::Relaxed);
    harness.engine.register_session(other, None);
    assert!(!harness.engine.is_session_registered(other));
    assert_eq!(log.lock().unwrap().last(), Some(&(3, Validate, false)));
    harness.backend.paint(other, &[WORKING]);
    rt.block_on(harness.ticks(2));
    assert_eq!(harness.backend.reads.load(Ordering::Relaxed), reads);
    // The control: a fresh attach to the re-registered session is accepted.
    let fresh = detector.activity_consumer(other).expect("attach");
    assert!((fresh.observe)(None));
}
