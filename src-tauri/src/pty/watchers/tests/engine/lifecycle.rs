use super::*;

/// 9.3.24 - **a session with no agent is never registered**, so a plain shell costs this
/// engine nothing, ever: no entry, no read, no allocation.
///
/// Asserted against the real registration site, which is one helper shared with the #1032
/// scraper, rather than against a re-implementation of its condition.
#[test]
fn a_session_with_no_agent_is_never_registered() {
    let harness = Harness::new();
    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&harness.engine))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app");

    let shell = Uuid::new_v4();
    crate::commands::session::register_session_samplers(app.handle(), shell, None);
    assert!(!harness.engine.is_session_registered(shell));

    let agent = Uuid::new_v4();
    crate::commands::session::register_session_samplers(
        app.handle(),
        agent,
        Some("a1".to_string()),
    );
    assert!(harness.engine.is_session_registered(agent));
}

/// 9.5.62 and 9.5.66 - the engine publishes each session's STATUS at the end of every
/// tick, so the snapshot command can be synchronous, and the `seq` in the ring is the same
/// value the event carried. That shared identity is what lets the window merge a snapshot
/// with a live stream instead of guessing.
#[tokio::test]
async fn the_engine_publishes_status_every_tick_and_the_ring_shares_the_events_seq() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure(
        "a1",
        vec![
            state_watcher("perm", "Permission required"),
            occurrence_watcher("read", "Read", WatcherDedupe::None, 2000),
        ],
    );

    // One tick with no match at all: the status is still published.
    harness.backend.paint(id, &["idle"]);
    harness.ticks(1).await;

    let snapshot = harness.history.snapshot(id, None);
    assert!(snapshot.warmed_up);
    assert!(snapshot.matches.is_empty());
    let counters: Vec<(&str, u64)> = snapshot
        .active_watchers
        .iter()
        .map(|counter| (counter.watcher_id.as_str(), counter.count))
        .collect();
    assert_eq!(counters, vec![("perm", 0), ("read", 0)]);

    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(1).await;

    // The engine hands the batch to the sink; the ring is the SINK's job, so mirror what
    // the production sink does and assert the identity that makes the merge possible.
    let emitted = harness.emitted();
    assert_eq!(emitted.len(), 1);
    harness.history.record(id, &emitted);
    let snapshot = harness.history.snapshot(id, None);
    assert_eq!(snapshot.matches[0].seq, emitted[0].seq);
    assert_eq!(snapshot.last_seq, emitted[0].seq);
    assert_eq!(
        snapshot
            .active_watchers
            .iter()
            .find(|counter| counter.watcher_id == "perm")
            .expect("counter")
            .count,
        1
    );
}

/// 9.5.72 - **a session whose child exits on its own KEEPS its buffer.** It is retired from
/// sampling and nothing else: an API error or a CLI crash is exactly when the evidence is
/// worth keeping, and it is the only thing left to look at.
#[tokio::test]
async fn a_session_that_dies_on_its_own_is_retired_but_keeps_its_history() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(1).await;
    harness.history.record(id, &harness.emitted());

    // Twice: a read that says `Gone` makes the engine re-resolve the backend once and ask
    // again, which is what covers a session that changed route.
    harness.backend.push(id, Painted::Gone);
    harness.backend.push(id, Painted::Gone);
    harness.ticks(1).await;

    assert!(!harness.engine.is_session_registered(id));
    let snapshot = harness.history.snapshot(id, None);
    assert_eq!(snapshot.matches.len(), 1, "the evidence must survive");
    assert!(snapshot.warmed_up);
}

/// 9.5.68 and 9.5.70 - `purge_session_side_state` retires FIRST and purges second, and the
/// entry cannot come back: only the engine creates entries, and only while holding its
/// registration lock.
#[tokio::test]
async fn purging_a_destroyed_session_retires_it_first_and_the_entry_never_returns() {
    let harness = Harness::new();
    let id = Uuid::new_v4();
    harness.engine.register_session(id, Some("a1".to_string()));
    harness.configure("a1", vec![state_watcher("perm", "Permission required")]);
    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(1).await;
    harness.history.record(id, &harness.emitted());
    assert!(harness.history.snapshot(id, None).warmed_up);

    let app = tauri::test::mock_builder()
        .manage(Arc::clone(&harness.engine))
        .manage(Arc::clone(&harness.history))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build a mock app");
    crate::commands::session::purge_session_side_state(app.handle(), id);

    assert!(!harness.engine.is_session_registered(id));
    assert!(!harness.history.snapshot(id, None).warmed_up);

    // Whatever else ticks, the entry stays gone: the session is no longer registered, so
    // no tick can publish it back.
    harness.backend.paint(id, &["Permission required"]);
    harness.ticks(5).await;
    assert!(!harness.history.snapshot(id, None).warmed_up);
}

/// 9.5.68 and 9.5.69 (call-site guard) - **the finding revision 1 of the plan missed.**
///
/// There are three production exits from `SessionManager`, not one, and the post-commit
/// cleanup is two parallel copies. A restart is a normal flow - the invoke handler, the
/// mailbox and a bulk settings save all reach it - so a purge wired into only the destroy
/// path would orphan up to 500 ring entries on every restart, under an id already gone
/// from the session list and therefore unreachable from the UI.
///
/// This pins BOTH call sites against the source, so removing either one fails here rather
/// than leaking silently.
#[test]
fn both_post_commit_cleanups_purge_the_per_session_side_state() {
    const SOURCE: &str = include_str!("../../../../commands/session.rs");

    let restart = SOURCE
        .split("fn publish_restart_destroyed")
        .nth(1)
        .expect("publish_restart_destroyed must exist");
    // Delimited by the closing brace at column 0, which is where a top-level function ends
    // whatever comes after it. Splitting on the NEXT declaration would let an unrelated
    // function inserted in between extend this "body" downwards, and the guard would then
    // pass with the purge living somewhere else entirely.
    let restart_body = restart
        .split("\n}")
        .next()
        .expect("its body must end somewhere");
    assert!(
        restart_body.contains("purge_session_side_state("),
        "the restart cleanup must purge; both restart paths publish through it"
    );

    let destroy = SOURCE
        .split("for session_id in outcome.destroyed_ids.iter().copied()")
        .nth(1)
        .expect("the destroy transaction must purge over destroyed_ids ONLY");
    let loop_body = destroy
        .split("\n    }")
        .next()
        .expect("its body must end somewhere");
    assert!(
        loop_body.contains("purge_session_side_state("),
        "the destroyed-only loop must be the one that purges"
    );
    // 9.5.71 - and it must stay destroyed-ONLY. A root-agent session retained as `Exited`
    // stays in the manager and its row is still in the list, so its post-mortem view still
    // has a place to be shown and its buffer must survive.
    assert!(
        !loop_body.contains("retained_exited_ids"),
        "retained-exited sessions keep their buffer; the purge loop must not reach them"
    );
}
