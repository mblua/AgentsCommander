use super::*;

/// 9.3.25 and 9.3.26 - **the zero-cost promise, and the lock promise, together.**
///
/// With a session registered and no watcher reaching its agent, the tick returns before
/// touching the session: no read, so no `screen_parsers` lock and no row allocation. And
/// the backend `Arc` is resolved exactly ONCE, at registration, so the tick path never
/// goes through `PtyManager` or the route registry however many times it runs.
#[tokio::test]
async fn an_unconfigured_app_touches_no_session_and_resolves_no_backend_per_tick() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));

    harness.ticks(10).await;

    assert_eq!(harness.backend.reads.load(Ordering::Relaxed), 0);
    assert_eq!(
        harness.backends.resolutions.load(Ordering::Relaxed),
        1,
        "registration resolves the backend; no tick may resolve it again"
    );

    // ...and with a watcher configured, the reads happen and the resolution count is still 1.
    harness.configure("a1", vec![state_watcher("w", "never")]);
    harness.ticks(5).await;
    assert_eq!(harness.backend.reads.load(Ordering::Relaxed), 5);
    assert_eq!(harness.backends.resolutions.load(Ordering::Relaxed), 1);
}

/// 9.3.27 and acceptance criterion A4 - a state reading is idempotent. Twenty-five ticks -
/// five seconds of a screen being repainted with the same content, which is what a live
/// spinner does - produce ONE event.
#[tokio::test]
async fn a_state_watcher_emits_once_for_a_value_that_does_not_change() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("ctx", r"Context (\d+)%")]);
    harness.backend.paint(id, &["idle", "Context 42%"]);

    harness.ticks(25).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].captures, vec![Some("42".to_string())]);
    assert_eq!(emitted[0].row, "Context 42%");
    assert_eq!(emitted[0].mode, WatcherMode::State);
    assert_eq!(emitted[0].seq, 1);
}

/// 9.3.28 - ...and it emits again the moment the reading changes.
#[tokio::test]
async fn a_state_watcher_emits_again_when_the_matched_row_changes() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("ctx", r"Context (\d+)%")]);
    harness.backend.paint(id, &["Context 42%"]);
    harness.backend.paint(id, &["Context 42%"]);
    harness.backend.paint(id, &["Context 39%"]);

    harness.ticks(3).await;

    let captures: Vec<_> = harness
        .emitted()
        .iter()
        .map(|m| m.captures[0].clone().unwrap())
        .collect();
    assert_eq!(captures, vec!["42", "39"]);
}

/// 9.3.29 - a state watcher that stops matching emits NOTHING. "The prompt disappeared" is
/// not a log entry, and clearing the gate is exactly what lets an identical re-appearance
/// emit again.
#[tokio::test]
async fn a_state_watcher_that_stops_matching_emits_nothing_and_re_emits_on_reappearance() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness.backend.paint(id, &["Permission required"]);
    harness.backend.paint(id, &["gone"]);
    harness.backend.paint(id, &["Permission required"]);

    harness.ticks(3).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 2, "appearance, silence, appearance");
    assert_eq!(emitted[0].row, "Permission required");
    assert_eq!(emitted[1].row, "Permission required");
    assert_ne!(emitted[0].seq, emitted[1].seq);
}

/// 9.3.30 - **the case a `(captures, row)` gate cannot express, and the strongest argument
/// for this engine existing.** A second permission prompt appears while the first is still
/// on screen: the lowest match reads the same text, so only the generation can tell them
/// apart.
#[tokio::test]
async fn a_second_identical_match_appearing_above_an_existing_one_emits() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness.backend.paint(id, &["Permission required", "idle"]);
    harness
        .backend
        .paint(id, &["Permission required", "Permission required"]);

    harness.ticks(2).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 2);
    assert_eq!(
        emitted[0].row, emitted[1].row,
        "identical text: a (captures, row) gate would have suppressed the second"
    );
}

/// 9.3.31 - a scroll that leaves the match count unchanged emits nothing. This is what
/// "incrementing the generation only on a RISE" buys: the count of a persistent condition
/// does not change as the screen moves under it.
#[tokio::test]
async fn a_scroll_that_leaves_the_match_count_unchanged_emits_nothing() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness
        .backend
        .paint(id, &["Permission required", "a", "b"]);
    harness
        .backend
        .paint(id, &["a", "Permission required", "b"]);
    harness
        .backend
        .paint(id, &["a", "b", "Permission required"]);

    harness.ticks(3).await;

    assert_eq!(harness.emitted().len(), 1);
}

/// 9.3.32 - the LOWEST match wins, mirroring `rows::extract`, because a statusline always
/// sits below the transcript.
#[tokio::test]
async fn the_lowest_matching_logical_row_wins() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("ctx", r"Context (\d+)%")]);
    harness
        .backend
        .paint(id, &["Context 99%", "middle", "Context 42%"]);

    harness.ticks(1).await;

    assert_eq!(harness.emitted()[0].captures[0].as_deref(), Some("42"));
}

/// 9.3.33 - a pattern that does not compile is compiled ONCE and logged once, however many
/// ticks pass. The compile site is the log site, so the compile count IS the log count.
#[tokio::test]
async fn an_uncompilable_pattern_is_compiled_once_across_ten_ticks() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("broken", r"Read \((.+")]);
    harness.backend.paint(id, &["anything"]);

    harness.ticks(10).await;

    assert_eq!(harness.engine.compile_count(), 1);
    assert!(harness.emitted().is_empty());
}

/// A watcher whose pattern is edited recompiles, and its gate starts again - so the first
/// match under the new pattern is an event even if the old one had already emitted.
#[tokio::test]
async fn editing_a_pattern_recompiles_and_clears_that_watchers_gate() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "alpha")]);
    harness.backend.paint(id, &["alpha beta"]);
    harness.ticks(3).await;
    assert_eq!(harness.engine.compile_count(), 1);
    assert_eq!(harness.emitted().len(), 1);

    harness.configure("a1", vec![state_watcher("w", "beta")]);
    harness.ticks(3).await;

    assert_eq!(harness.engine.compile_count(), 2);
    assert_eq!(
        harness.emitted().len(),
        2,
        "the row is the same, but the question is not: the gate must not suppress it"
    );
}

/// 9.3.34 (first half) - `Gone` retires the session on that tick, with no waiting for the
/// 5 s probe.
#[tokio::test]
async fn a_session_reported_gone_is_retired_on_that_tick() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "x")]);
    harness.backend.push(id, Painted::Gone);
    harness.backend.push(id, Painted::Gone);

    harness.ticks(1).await;

    assert!(!harness.engine.is_session_registered(id));
}

/// `Missing` says NOTHING about the session, so it is kept and sampled again. Retiring on
/// it would strand a live session whose parser was momentarily unreadable.
#[tokio::test]
async fn a_session_reported_missing_is_kept() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "x")]);
    harness.backend.push(id, Painted::Missing);
    harness.backend.push(id, Painted::Missing);

    harness.ticks(1).await;

    assert!(harness.engine.is_session_registered(id));
}

/// 9.3.34 (second half) and 9.3.35 - liveness is probed on tick 25 and on no tick before
/// it, which is once per 5 s per session: exactly the rate the 5 s scraper probes at, and
/// the reason this engine is cheaper than the one it sits beside rather than 25x dearer.
#[tokio::test]
async fn liveness_is_probed_on_the_twenty_fifth_tick_and_retires_a_dead_session() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "never")]);
    harness.backend.paint(id, &["idle"]);
    harness
        .backends
        .liveness
        .lock()
        .unwrap()
        .insert(id, ContextSessionLiveness::SessionOver);

    harness.ticks(24).await;
    assert_eq!(harness.backends.liveness_calls.load(Ordering::Relaxed), 0);
    assert!(harness.engine.is_session_registered(id));

    harness.ticks(1).await;
    assert_eq!(harness.backends.liveness_calls.load(Ordering::Relaxed), 1);
    assert!(!harness.engine.is_session_registered(id));
}

/// 9.3.37 - one event per `(session, tick)`, carrying every match that tick produced. The
/// coalescing is what keeps the IPC load off the per-tick caps.
#[tokio::test]
async fn one_event_carries_all_of_a_ticks_matches_for_a_session() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![
            state_watcher("perm", "Permission required"),
            state_watcher("ctx", r"Context (\d+)%"),
        ],
    );
    harness
        .backend
        .paint(id, &["Permission required", "Context 42%"]);

    harness.ticks(1).await;

    assert_eq!(harness.batches(), 1);
    let batch = &harness.sink.0.lock().unwrap()[0];
    assert_eq!(batch.session_id, id.to_string());
    assert_eq!(batch.matches.len(), 2);
    assert_eq!(batch.matches[0].seq, 1);
    assert_eq!(batch.matches[1].seq, 2);
}

/// An `Unchanged` read evaluates nothing: the screen the caller last saw cannot have
/// started or stopped matching.
#[tokio::test]
async fn an_unchanged_frame_produces_no_event() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "match me")]);
    harness.backend.push(id, Painted::Unchanged);
    harness.backend.push(id, Painted::Unchanged);

    harness.ticks(2).await;

    assert!(harness.emitted().is_empty());
}

/// A session whose agent no watcher reaches is skipped, even while another session on a
/// configured agent is being sampled.
#[tokio::test]
async fn a_session_whose_agent_has_no_watcher_is_skipped() {
    let harness = Harness::new();
    let watched = Uuid::new_v4();
    let ignored = Uuid::new_v4();
    harness
        .engine
        .register_session(watched, Some("a1".to_string()));
    harness
        .engine
        .register_session(ignored, Some("a2".to_string()));
    harness.configure("a1", vec![state_watcher("w", "hit")]);
    harness.backend.paint(watched, &["hit"]);
    harness.backend.paint(ignored, &["hit"]);

    harness.ticks(1).await;

    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].session_id, watched.to_string());
}

/// A wrapped path is matched as ONE logical row, which is the whole reason evaluation is
/// not on physical rows.
#[tokio::test]
async fn a_state_watcher_matches_across_a_wrapped_row() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("read", r"Read \((.+)\)")]);
    harness.backend.push(
        id,
        Painted::Frame(
            vec![
                "idle".to_string(),
                "Read (C:/repo/very/long/pa".to_string(),
                "th/to/main.rs)".to_string(),
            ],
            vec![false, true, false],
            0,
        ),
    );

    harness.ticks(1).await;

    assert_eq!(
        harness.emitted()[0].captures[0].as_deref(),
        Some("C:/repo/very/long/path/to/main.rs")
    );
}

/// 9.6.38 - a row past the byte cap is truncated on a char boundary and SAYS so, because
/// `row.length >= 256` cannot answer that question in TypeScript.
#[tokio::test]
async fn an_over_long_row_is_truncated_on_a_char_boundary_and_flagged() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("w", "start")]);
    // 3-byte chars, so the cap lands mid-character unless the boundary is respected.
    let long = format!("start{}", "\u{4e2d}".repeat(200));
    harness.backend.paint(id, &[&long]);

    harness.ticks(1).await;

    let emitted = harness.emitted();
    assert!(emitted[0].row_truncated);
    assert!(emitted[0].row.len() <= MAX_ROW_BYTES);
    assert!(long.starts_with(&emitted[0].row));
}
