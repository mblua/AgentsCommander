use super::{reraise_room_reader_demands, tick_raise_calls, tick_recheck_calls};
use crate::commands::session::co_managed_tests::{configure_room, room_fixture};
use crate::commands::session::reader_demand_tests::harness;
use crate::commands::session::release_room_reader_demand;
use crate::commands::telegram::{holds_room_reader_demand, reader_demand_seam};

fn tee_len() -> usize {
    crate::logging::test_tee_snapshot().len()
}

/// Re-raise lines naming `session_id` added since `before`.
fn lines_since(before: usize, session_id: uuid::Uuid) -> Vec<String> {
    let id = session_id.to_string();
    crate::logging::test_tee_snapshot()
        .into_iter()
        .skip(before)
        .filter(|line| line.contains("reader demand re-raised") && line.contains(&id))
        .collect()
}

/// Test 1 (reproduction): a Ready room's orchestrator with no Room demand
/// gains one after a single tick.
#[tokio::test]
async fn p5_ready_session_without_a_demand_gains_one_in_one_tick() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    assert!(!holds_room_reader_demand(h.app.handle(), id).await);

    reraise_room_reader_demands(h.app.handle()).await;

    assert!(holds_room_reader_demand(h.app.handle(), id).await);
}

/// Test 2 (reproduction): readiness acquired after session create. Not
/// Ready for several ticks, then Ready: the next tick raises exactly one
/// demand and logs exactly one line.
#[tokio::test]
async fn p5_readiness_acquired_late_is_picked_up_by_the_next_tick() {
    crate::logging::test_install_logger();
    let fixture = room_fixture();
    configure_room(fixture.room_path(), false);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let before = tee_len();
    for _ in 0..5 {
        reraise_room_reader_demands(h.app.handle()).await;
    }
    assert!(!holds_room_reader_demand(h.app.handle(), id).await);
    let lines = lines_since(before, id);
    assert!(lines.is_empty(), "no line while Off: {lines:?}");

    configure_room(fixture.room_path(), true);
    let before = tee_len();
    reraise_room_reader_demands(h.app.handle()).await;
    assert!(holds_room_reader_demand(h.app.handle(), id).await);
    let lines = lines_since(before, id);
    assert_eq!(lines.len(), 1, "exactly one line for the raise: {lines:?}");
    assert!(lines[0].starts_with("INFO "), "{lines:?}");
}

/// Test 3 (guard): a session already holding a Room demand is untouched and
/// costs no readiness resolution. Mutation: remove the cheap gate; the set
/// assertion stays green while the counter assertion turns red.
#[tokio::test]
async fn p5_held_demand_is_skipped_without_resolving_readiness() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    reraise_room_reader_demands(h.app.handle()).await;
    assert!(holds_room_reader_demand(h.app.handle(), id).await);
    let calls = tick_raise_calls(id);

    for _ in 0..10 {
        reraise_room_reader_demands(h.app.handle()).await;
    }

    assert!(
        holds_room_reader_demand(h.app.handle(), id).await,
        "the demand set is unchanged"
    );
    assert_eq!(
        tick_raise_calls(id),
        calls,
        "a held demand must not reach readiness resolution"
    );
}

/// Test 4: a non-orchestrator in a Ready room never gains a demand
/// (delegation contract), the tick logs nothing for it, and the refused raise
/// never pays for the post-raise recheck (D5-j cost guard; mutation: drop the
/// raise-result check).
#[tokio::test]
async fn p5_non_orchestrator_never_gains_a_demand_and_logs_nothing() {
    crate::logging::test_install_logger();
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let member = h.session_in(fixture.member_path()).await;

    let before = tee_len();
    for _ in 0..10 {
        reraise_room_reader_demands(h.app.handle()).await;
    }

    assert!(!holds_room_reader_demand(h.app.handle(), member).await);
    let lines = lines_since(before, member);
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(
        tick_recheck_calls(member),
        0,
        "a refused raise must not resolve readiness a second time"
    );
}

/// Test 5: a session whose directory is in no room never gains a demand,
/// does not panic, logs nothing and never reaches the recheck (same split as
/// test 4).
#[tokio::test]
async fn p5_session_outside_any_room_never_gains_a_demand_and_logs_nothing() {
    crate::logging::test_install_logger();
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let outside = h.session_in(fixture.temp_path()).await;

    let before = tee_len();
    for _ in 0..10 {
        reraise_room_reader_demands(h.app.handle()).await;
    }

    assert!(!holds_room_reader_demand(h.app.handle(), outside).await);
    let lines = lines_since(before, outside);
    assert!(lines.is_empty(), "{lines:?}");
    assert_eq!(
        tick_recheck_calls(outside),
        0,
        "a refused raise must not resolve readiness a second time"
    );
}

/// Test 6 (guard): the zero cases. No live session at all, and a list where
/// every entry already holds a demand, both complete with no raise and no
/// line. Mutation: assume a non-empty list.
#[tokio::test]
async fn p5_zero_cases_complete_cleanly() {
    crate::logging::test_install_logger();
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);

    // No live session: the tick is a no-op, not an error.
    reraise_room_reader_demands(h.app.handle()).await;

    let id = h.session_in(fixture.coordinator_path()).await;
    reraise_room_reader_demands(h.app.handle()).await;
    assert!(holds_room_reader_demand(h.app.handle(), id).await);
    let calls = tick_raise_calls(id);
    let before = tee_len();

    reraise_room_reader_demands(h.app.handle()).await;

    assert_eq!(tick_raise_calls(id), calls, "no raise");
    let lines = lines_since(before, id);
    assert!(lines.is_empty(), "no line: {lines:?}");
}

/// Test 7 (guard): release is not fought, and the tick MAY raise again
/// after it. Decided behavior for a still-Ready room: a released demand is
/// re-raised by the next tick. Mutation: suppress the raise after a release.
#[tokio::test]
async fn p5_tick_raises_again_after_an_explicit_release() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    reraise_room_reader_demands(h.app.handle()).await;
    assert!(holds_room_reader_demand(h.app.handle(), id).await);

    release_room_reader_demand(h.app.handle(), id).await;
    assert!(
        !holds_room_reader_demand(h.app.handle(), id).await,
        "the release itself is honoured"
    );

    reraise_room_reader_demands(h.app.handle()).await;
    assert!(
        holds_room_reader_demand(h.app.handle(), id).await,
        "a still-Ready room is re-raised after a release"
    );
}

/// Test 8 (guard): exactly one line per actual raise, zero lines across ten
/// steady-state ticks. The steady state keeps a live session in an `Off`
/// room for all ten ticks: it fails the cheap gate every tick and reaches
/// the refused raise. Mutation (combined, D5-j): drop the raise-result check
/// AND the recheck; either one alone still guards the log.
#[tokio::test]
async fn p5_steady_state_logs_nothing_across_ten_ticks() {
    crate::logging::test_install_logger();
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let off_fixture = room_fixture();
    configure_room(off_fixture.room_path(), false);
    let h = harness(&fixture);
    let ready = h.session_in(fixture.coordinator_path()).await;
    let off = h.session_in(off_fixture.coordinator_path()).await;

    let before = tee_len();
    reraise_room_reader_demands(h.app.handle()).await;
    let lines = lines_since(before, ready);
    assert_eq!(lines.len(), 1, "one line for the one raise: {lines:?}");
    let lines = lines_since(before, off);
    assert!(lines.is_empty(), "{lines:?}");

    let off_calls = tick_raise_calls(off);
    let before = tee_len();
    for _ in 0..10 {
        reraise_room_reader_demands(h.app.handle()).await;
    }

    assert_eq!(
        tick_raise_calls(off) - off_calls,
        10,
        "all ten ticks ran and visited the Off session"
    );
    assert!(holds_room_reader_demand(h.app.handle(), ready).await);
    assert!(!holds_room_reader_demand(h.app.handle(), off).await);
    let ready_lines = lines_since(before, ready);
    let off_lines = lines_since(before, off);
    assert!(ready_lines.is_empty(), "{ready_lines:?}");
    assert!(off_lines.is_empty(), "{off_lines:?}");
}

/// Test 9 (reproduction, D5-h): readiness is lost DURING the tick's own
/// raise. The room is disabled while the raise is paused before install;
/// when the tick returns the session holds no Room demand and nothing was
/// logged. Mutation: delete the post-raise recheck.
#[tokio::test]
async fn p5_readiness_lost_during_the_raise_leaves_no_demand() {
    crate::logging::test_install_logger();
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    let barrier = reader_demand_seam::install_before_install(&id.to_string());

    let before = tee_len();
    let app = h.app.handle().clone();
    let tick = tokio::spawn(async move { reraise_room_reader_demands(&app).await });
    barrier.reached.notified().await;
    // The same file `co_managed_set_enabled` writes.
    configure_room(fixture.room_path(), false);
    barrier.release.notify_one();
    tick.await.expect("tick task");

    assert!(
        !holds_room_reader_demand(h.app.handle(), id).await,
        "a raise that lost the readiness race must be undone"
    );
    let lines = lines_since(before, id);
    assert!(lines.is_empty(), "{lines:?}");
}

/// Test 10 (guard, D5-h): the recheck is not a blanket release. A demand
/// already held when the room is disabled survives many ticks, because the
/// cheap gate skips it before the recheck. Mutation: release on a non-Ready
/// recheck outside the "this tick raised it" branch.
#[tokio::test]
async fn p5_recheck_never_releases_a_demand_it_did_not_raise() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    reraise_room_reader_demands(h.app.handle()).await;
    assert!(holds_room_reader_demand(h.app.handle(), id).await);

    configure_room(fixture.room_path(), false);
    for _ in 0..10 {
        reraise_room_reader_demands(h.app.handle()).await;
    }

    assert!(
        holds_room_reader_demand(h.app.handle(), id).await,
        "the tick only releases what it installed in the same iteration"
    );
}

/// #2525 T-r1 (reproduction, global switch): switching Co-managed off
/// globally releases a running Room demand within one pass.
#[tokio::test]
async fn p2525_switch_off_releases_a_running_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    super::reader_demand_pass(h.app.handle()).await;
    assert!(holds_room_reader_demand(h.app.handle(), id).await);

    h.set_global_co_managed(false, "test-key").await;
    super::reader_demand_pass(h.app.handle()).await;

    assert!(
        !holds_room_reader_demand(h.app.handle(), id).await,
        "the global switch off must release the Room demand"
    );
    crate::commands::session::reader_demand_tests::assert_no_room_reader_state_for(&h, id).await;
}

/// #2525 T-r2, inverted by #2756 U2a: a whitespace-only key is a `Ready`
/// input, not a capture input, so a running Room demand survives the pass
/// and the slot stays open. Mutation: sweep on the key again.
#[tokio::test]
async fn p2525_cleared_api_key_keeps_a_running_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    super::reader_demand_pass(h.app.handle()).await;
    assert!(holds_room_reader_demand(h.app.handle(), id).await);

    h.set_global_co_managed(true, "   ").await;
    super::reader_demand_pass(h.app.handle()).await;

    assert!(
        holds_room_reader_demand(h.app.handle(), id).await,
        "a cleared API key must keep the Room demand"
    );
    assert!(
        h.captures.is_open(&id.to_string()),
        "a cleared API key must keep the capture slot"
    );
}

/// #2525 T-r3 (guard): while available globally the pass keeps running the
/// ordinary re-raise. Mutations: an inverted predicate, or a sweep that
/// answers `true` while available.
#[tokio::test]
async fn p2525_available_globally_keeps_re_raising() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    for pass in 0..10 {
        release_room_reader_demand(h.app.handle(), id).await;
        super::reader_demand_pass(h.app.handle()).await;
        assert!(
            holds_room_reader_demand(h.app.handle(), id).await,
            "pass {pass} re-raises the demand"
        );
        assert_eq!(tick_raise_calls(id), pass + 1, "pass {pass}");
    }
}

/// #2525 T-r5 (the early return): a swept pass never resolves readiness.
/// Mutation: drop the `return` in `reader_demand_pass`.
#[tokio::test]
async fn p2525_a_swept_pass_does_not_resolve_readiness() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    h.set_global_co_managed(false, "test-key").await;
    let calls = tick_raise_calls(id);

    for _ in 0..5 {
        super::reader_demand_pass(h.app.handle()).await;
    }

    assert_eq!(tick_raise_calls(id), calls, "no readiness resolution");
    assert!(!holds_room_reader_demand(h.app.handle(), id).await);
}

/// #2525 T-r6 (the unconditional release): the sweep moves the Room epoch
/// of a session that holds nothing. Mutation: guard the release with
/// `holds_room_reader_demand`.
#[tokio::test]
async fn p2525_sweep_moves_the_epoch_of_a_session_with_no_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    let before = crate::commands::telegram::room_demand_epoch(h.app.handle(), id).await;

    h.set_global_co_managed(false, "test-key").await;
    super::reader_demand_pass(h.app.handle()).await;

    assert_ne!(
        crate::commands::telegram::room_demand_epoch(h.app.handle(), id).await,
        before,
        "the sweep must bump the epoch of a session with no demand"
    );
}

/// #2525 T-r7 (the stale-raise interleaving): a raise that resolved Ready
/// and parks before the install, overtaken by a globally-off sweep,
/// installs nothing. Mutation: a holders-only sweep.
#[tokio::test]
async fn p2525_a_raise_in_flight_when_the_sweep_runs_installs_nothing() {
    use crate::commands::session::reader_demand_tests::{
        assert_no_room_reader_state, race_room_raise,
    };
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let barrier = crate::commands::session::room_raise_seam::install_before_add(&id.to_string());
    let raised = race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        async {
            h.set_global_co_managed(false, "test-key").await;
            super::reader_demand_pass(h.app.handle()).await;
        },
    )
    .await;
    assert_no_room_reader_state(&h, raised, id).await;
}
