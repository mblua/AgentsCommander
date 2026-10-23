use super::*;

/// 9.4.45 (the engine half) - **the cursor-row rule, end to end.**
///
/// A path is written in two chunks across a tick boundary. Without the rule this produces
/// two events - one with `C:/repo/very/long/pa` and one with the whole path - and no
/// dedupe setting can merge them, because both the row AND the captures differ. With it,
/// exactly one event carries the complete capture.
#[tokio::test]
async fn a_row_written_in_two_chunks_produces_one_event_with_the_complete_capture() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher(
            "read",
            r"Read \((.+)\)",
            WatcherDedupe::None,
            2000,
        )],
    );

    // Seed, then scroll the half-written row in UNDER THE CURSOR, then finish it, then
    // hold still. The cursor on the bottom row is what a terminal that just printed a
    // newline really looks like.
    harness.backend.paint_with_cursor(id, &["a", "b", "c"], 2);
    harness
        .backend
        .paint_with_cursor(id, &["b", "c", "Read (C:/repo/very/long/pa"], 2);
    harness.backend.paint_with_cursor(
        id,
        &["b", "c", "Read (C:/repo/very/long/path/to/main.rs)"],
        2,
    );
    harness.backend.paint_with_cursor(
        id,
        &["b", "c", "Read (C:/repo/very/long/path/to/main.rs)"],
        2,
    );

    harness.ticks(4).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 1, "got {emitted:#?}");
    assert_eq!(
        emitted[0].captures[0].as_deref(),
        Some("C:/repo/very/long/path/to/main.rs")
    );
}

/// 9.4.50 - the same text at two positions at two times counts TWICE, and the two events
/// are told apart by `seq`. Layer 1 must not confuse them for each other; `at` cannot tell
/// them apart, which is why `seq` exists.
#[tokio::test]
async fn the_same_text_arriving_twice_counts_twice_with_different_seq() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher(
            "done",
            "Done",
            WatcherDedupe::None,
            2000,
        )],
    );
    harness.backend.paint(id, &["a", "b", "c"]);
    harness.backend.paint(id, &["b", "c", "Done"]);
    harness.backend.paint(id, &["c", "Done", "Done"]);

    harness.ticks(3).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 2);
    assert_eq!(emitted[0].row, emitted[1].row);
    assert_ne!(emitted[0].seq, emitted[1].seq);
}

/// 9.4.52 - the three dedupe keys, on the residual case layer 1 cannot separate: the same
/// path printed twice, truncated differently by the terminal.
#[tokio::test]
async fn the_capture_key_collapses_differently_truncated_rows_and_none_lets_everything_through() {
    for (dedupe, expected) in [(WatcherDedupe::Capture, 1), (WatcherDedupe::None, 2)] {
        let harness = Harness::new();
        let id = Uuid::new_v4();
        harness.engine.register_session(id, Some("a1".to_string()));
        harness.configure(
            "a1",
            vec![occurrence_watcher("read", r"Read (\S+)", dedupe, 60_000)],
        );
        harness.backend.paint(id, &["a", "b", "c"]);
        harness
            .backend
            .paint(id, &["b", "c", "Read C:/repo/main.rs"]);
        harness.backend.paint(
            id,
            &["c", "Read C:/repo/main.rs", "Read C:/repo/main.rs (2)"],
        );

        harness.ticks(3).await;

        assert_eq!(
            harness.emitted().len(),
            expected,
            "dedupe={dedupe:?} should have produced {expected}"
        );
    }
}

/// ...and the window is a DEDUPLICATION window, not a mute button: the same key comes back
/// once it has passed.
#[tokio::test]
async fn a_repeated_key_is_admitted_again_once_the_dedupe_window_has_passed() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher("done", "Done", WatcherDedupe::Row, 1)],
    );
    harness.backend.paint(id, &["a", "b", "c"]);
    harness.backend.paint(id, &["b", "c", "Done"]);
    harness.ticks(2).await;
    assert_eq!(harness.emitted().len(), 1);

    std::thread::sleep(Duration::from_millis(5));
    harness.backend.paint(id, &["c", "Done", "Done"]);
    harness.ticks(1).await;

    assert_eq!(harness.emitted().len(), 2);
}

/// 9.4.54 and 9.4.55 - **the caps, and where the overflow goes: nowhere.**
///
/// A pattern that matches every row, against a full-screen repaint that stabilizes. Three
/// watchers would produce 90 matches; the per-watcher cap holds each to 8 and the
/// per-session cap holds the tick to 16, and every match beyond them reaches neither the
/// event nor anything else.
#[tokio::test]
async fn the_per_tick_caps_bound_the_batch_and_mark_the_watchers_degraded() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![
            occurrence_watcher("w1", "row", WatcherDedupe::None, 2000),
            occurrence_watcher("w2", "row", WatcherDedupe::None, 2000),
            occurrence_watcher("w3", "row", WatcherDedupe::None, 2000),
        ],
    );

    let seed: Vec<String> = (0..30).map(|i| format!("seed {i}")).collect();
    let painted: Vec<String> = (0..30).map(|i| format!("row {i}")).collect();
    harness.backend.paint(id, &as_refs(&seed));
    harness.backend.paint(id, &as_refs(&painted));
    harness.backend.paint(id, &as_refs(&painted));

    harness.ticks(3).await;

    let emitted = harness.emitted();
    assert_eq!(
        emitted.len(),
        MATCHES_PER_SESSION_PER_TICK,
        "the session cap is the outer bound"
    );
    let w1 = emitted.iter().filter(|m| m.watcher_id == "w1").count();
    assert_eq!(w1, MATCHES_PER_WATCHER_PER_TICK, "the per-watcher cap");
    assert_eq!(
        emitted.iter().filter(|m| m.watcher_id == "w3").count(),
        0,
        "the session budget was already spent when w3 was reached"
    );

    for watcher in ["w1", "w2", "w3"] {
        let (degraded, _, _) = harness.engine.watcher_state(id, watcher).expect("state");
        assert!(degraded, "{watcher} must be marked degraded");
    }
}

/// **The regression that made a permission prompt vanish forever.**
///
/// Two saturated `occurrence` watchers ahead of a `state` one in `BTreeMap` order spend the
/// whole 16-per-session budget, so the `state` watcher is handed `budget == 0` and its match
/// is dropped. If the gate were committed while evaluating, the next tick would find the
/// candidate equal to the gate and return nothing: the activation would never reappear
/// while the text and the generation held - and silently, because `degraded` clears on the
/// tick after.
///
/// The plan says "emits nothing further for that key THIS TICK". This is the test that the
/// word "tick" is honoured.
#[tokio::test]
async fn a_state_match_dropped_by_the_session_cap_still_emits_on_the_next_tick() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![
            occurrence_watcher("a-reads", "row", WatcherDedupe::None, 2000),
            occurrence_watcher("b-tools", "row", WatcherDedupe::None, 2000),
            state_watcher("z-permission", "Permission required"),
        ],
    );

    // Seed, then a full repaint that settles nothing, then the tick where 29 rows become
    // evaluable AT THE SAME TIME as the prompt appears on the bottom row. That coincidence
    // is the whole scenario: the prompt has to arrive while the budget is already gone.
    let seed: Vec<String> = (0..30).map(|i| format!("seed {i}")).collect();
    let burst: Vec<String> = (0..30).map(|i| format!("row {i}")).collect();
    let mut prompted = burst.clone();
    prompted[29] = "Permission required".to_string();

    harness.backend.paint(id, &as_refs(&seed));
    harness.backend.paint(id, &as_refs(&burst));
    harness.backend.paint(id, &as_refs(&prompted));

    harness.ticks(3).await;

    let during = harness.emitted();
    assert_eq!(
        during.len(),
        MATCHES_PER_SESSION_PER_TICK,
        "the two occurrence watchers must have spent the whole session budget"
    );
    assert!(
        during.iter().all(|m| m.watcher_id != "z-permission"),
        "the prompt was dropped by the cap on this tick, which is what the plan allows"
    );
    let (degraded, _, _) = harness
        .engine
        .watcher_state(id, "z-permission")
        .expect("state");
    assert!(degraded, "and it must say so");

    // The screen holds still. The occurrence watchers take no budget - the only row that
    // stabilizes is the prompt, which their pattern does not match - and the prompt is
    // still on screen, so it MUST come through.
    harness.backend.paint(id, &as_refs(&prompted));
    harness.ticks(1).await;

    let after: Vec<_> = harness
        .emitted()
        .into_iter()
        .filter(|m| m.watcher_id == "z-permission")
        .collect();
    assert_eq!(
        after.len(),
        1,
        "the activation the cap dropped has to reappear, or it is lost forever"
    );
    assert_eq!(after[0].row, "Permission required");
}

/// **P1-2's regression: the day-one empty state has to be reachable.**
///
/// A registered agent session that no watcher reaches must answer `warmedUp: true` with
/// `activeWatchers: []`. That pair IS the window's "no watcher reaches this agent, here is
/// the Configure button" state, and it is what everybody sees on day one. Publishing
/// nothing left both this case and "the engine has not run yet" answering `warmedUp: false`
/// forever - byte-identical snapshots for the two cases the field exists to tell apart.
#[tokio::test]
async fn a_session_no_watcher_reaches_is_warmed_up_with_no_active_watchers() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.backend.paint(id, &["idle"]);

    // Before the first tick: the engine really has not run.
    assert!(!harness.history.snapshot(id, None).warmed_up);

    // Day one: nothing configured at all, so the tick takes the early exit.
    harness.ticks(1).await;

    let snapshot = harness.history.snapshot(id, None);
    assert!(snapshot.warmed_up);
    assert!(snapshot.active_watchers.is_empty());
    assert!(snapshot.matches.is_empty());
    assert_eq!(
        harness.backend.reads.load(Ordering::Relaxed),
        0,
        "and it must still not have touched the session: no read, no rows"
    );
    assert_eq!(
        harness.backends.liveness_calls.load(Ordering::Relaxed),
        0,
        "nor probed a child nobody is watching"
    );

    // Watchers configured, but for a DIFFERENT agent: same answer, same reason.
    harness.configure("other-agent", vec![state_watcher("w", "x")]);
    harness.ticks(1).await;

    let snapshot = harness.history.snapshot(id, None);
    assert!(snapshot.warmed_up);
    assert!(snapshot.active_watchers.is_empty());
    assert_eq!(harness.backend.reads.load(Ordering::Relaxed), 0);
}

/// 9.4.56 - the caps bound the RATE but not the duration, so a pair that stays saturated is
/// suspended, stays marked degraded while it is, and is retried afterwards.
#[tokio::test]
async fn a_pair_saturated_for_twenty_five_ticks_is_suspended_and_then_retried() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher(
            "flood",
            "row",
            WatcherDedupe::None,
            2000,
        )],
    );

    // A transcript scrolling ten rows a tick: every tick brings ten new matching rows, so
    // every tick is over the eight-per-watcher cap. A repaint would only degrade every
    // other tick and the counter would reset in between, which is the whole point of
    // requiring the ticks to be CONSECUTIVE.
    let transcript: Vec<String> = (0..1000).map(|i| format!("row {i}")).collect();
    for tick in 0..60usize {
        let start = tick * 10;
        harness
            .backend
            .paint(id, &as_refs(&transcript[start..start + 30]));
    }

    harness.ticks(40).await;

    let (degraded, suspended, _) = harness.engine.watcher_state(id, "flood").expect("state");
    assert!(suspended, "25 degraded ticks must suspend the pair");
    assert!(degraded, "suspension must not read as recovery");

    let before = harness.emitted().len();
    harness.ticks(1).await;
    assert_eq!(
        harness.emitted().len(),
        before,
        "a suspended pair emits nothing at all"
    );

    harness.engine.expire_suspension_for_test(id, "flood");
    harness.ticks(1).await;
    assert!(
        harness.emitted().len() > before,
        "and it is retried once the suspension passes"
    );
}

/// Acceptance criterion A5 - a transcript of more than a screenful delivered in ONE burst
/// reports its matches AND a non-zero `possiblyMissedFrames`, never a silent zero.
///
/// A jump larger than the screen has no alignment that explains it, so the honest answer
/// is "the sampler could not follow" plus whatever the next stable frame turns out to hold.
#[tokio::test]
async fn a_burst_larger_than_the_screen_reports_matches_and_admits_it_may_have_missed_some() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher("row", "row", WatcherDedupe::None, 2000)],
    );

    let seed: Vec<String> = (0..30).map(|i| format!("seed {i}")).collect();
    // 60 rows arrived between two samples: only the last 30 are still on the screen.
    let after: Vec<String> = (30..60).map(|i| format!("row {i}")).collect();
    harness.backend.paint(id, &as_refs(&seed));
    harness.backend.paint(id, &as_refs(&after));
    harness.backend.paint(id, &as_refs(&after));

    harness.ticks(3).await;

    assert!(!harness.emitted().is_empty(), "the burst must still report");
    let snapshot = harness.history.snapshot(id, None);
    assert!(
        snapshot.possibly_missed_frames > 0,
        "a jump larger than the screen must never report a silent zero"
    );
}

/// An occurrence watcher never fires on the tick a row is merely painted: the row has to
/// have arrived from below, or held still for a tick. That is layer 1 doing the work.
#[tokio::test]
async fn an_occurrence_watcher_fires_only_when_the_diff_declares_a_row_evaluable() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![occurrence_watcher("hit", "hit", WatcherDedupe::None, 2000)],
    );
    // The first tick seeds and evaluates nothing, even though the row is right there.
    harness.backend.paint(id, &["hit", "hit", "hit"]);
    harness.ticks(1).await;

    assert!(harness.emitted().is_empty());
}
