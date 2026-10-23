use super::*;

// -----------------------------------------------------------------
// #2879 - the Codex Working detector, driven through `evaluate_tick`
// with injected instants, so no test sleeps.
// -----------------------------------------------------------------

type I2879Edges = Arc<Mutex<Vec<(&'static str, Uuid, usize)>>>;

fn i2879_codex() -> IdleTuning {
    IdleTuning {
        activity_detector: ActivityDetectorKind::CodexWorking,
        ..IdleTuning::DEFAULT
    }
}

/// A detector whose callbacks record `(edge, id, global order)`.
fn i2879_detector() -> (
    Arc<IdleDetector>,
    I2879Edges,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let edges: I2879Edges = Arc::new(Mutex::new(Vec::new()));
    let order = Arc::new(AtomicUsize::new(0));
    let (idle_edges, busy_edges) = (Arc::clone(&edges), Arc::clone(&edges));
    let (idle_order, busy_order) = (Arc::clone(&order), Arc::clone(&order));
    let detector = IdleDetector::new(
        move |id| {
            let n = idle_order.fetch_add(1, Ordering::SeqCst);
            idle_edges.lock().unwrap().push(("idle", id, n));
        },
        move |id| {
            let n = busy_order.fetch_add(1, Ordering::SeqCst);
            busy_edges.lock().unwrap().push(("busy", id, n));
        },
    );
    (detector, edges, order)
}

fn i2879_edges(edges: &I2879Edges) -> Vec<(&'static str, Uuid)> {
    edges
        .lock()
        .unwrap()
        .iter()
        .map(|(edge, id, _)| (*edge, *id))
        .collect()
}

fn i2879_stamp(detector: &IdleDetector, id: Uuid) -> Instant {
    *detector.activity.lock().unwrap().get(&id).expect("seeded")
}

/// `(kind, attached, last_valid)` of the current registration.
fn i2879_registration(
    detector: &IdleDetector,
    id: Uuid,
) -> Option<(ActivityDetectorKind, bool, Option<bool>)> {
    detector
        .activity
        .lock()
        .unwrap()
        .semantic
        .get(&id)
        .map(|r| (r.kind, r.observation.is_some(), r.last_valid))
}

/// A visible indicator keeps a Codex session busy long after the threshold, with no
/// bytes and no refresh of any clock; the Timed control on the same instants goes idle.
#[test]
fn i2879_feature_frozen() {
    let (detector, edges, _) = i2879_detector();
    let codex = Uuid::new_v4();
    let timed = Uuid::new_v4();
    detector.register_session(codex, i2879_codex());
    detector.register_session(timed, IdleTuning::DEFAULT);
    let t0 = i2879_stamp(&detector, codex);
    detector.activity.lock().unwrap().insert(timed, t0);
    let silence = *detector.silence.lock().unwrap().get(&codex).unwrap();

    let handle = detector.activity_consumer(codex).expect("Codex attaches");
    assert!((handle.observe)(Some(true)));

    let threshold = i2879_codex().idle_threshold;
    for after in [
        threshold + Duration::from_millis(1),
        threshold * 4,
        Duration::from_secs(10 * 60 * 60),
    ] {
        assert!(detector.evaluate_tick(Some(t0 + after), false).is_none());
        // An Unchanged frame or a validation delivers nothing new and keeps the evidence.
        assert!((handle.observe)(None));
    }

    assert_eq!(
        i2879_edges(&edges),
        vec![("idle", timed)],
        "the Timed control idles once; the visible indicator never lets Codex idle"
    );
    assert!(!detector.idle_set.lock().unwrap().contains(&codex));
    assert_eq!(i2879_stamp(&detector, codex), t0, "no activity refresh");
    assert_eq!(
        *detector.silence.lock().unwrap().get(&codex).unwrap(),
        silence,
        "no silence refresh"
    );
    assert_eq!(
        i2879_registration(&detector, codex),
        Some((ActivityDetectorKind::CodexWorking, true, Some(true)))
    );
}

/// Without positive evidence - none yet, an initial Unavailable, or a valid negative -
/// Codex is exactly Timed on both sides of the threshold; a positive followed by a
/// negative never ends the turn before the timed clock does.
#[test]
fn i2879_feature_fallback() {
    let threshold = i2879_codex().idle_threshold;
    for case in ["none", "unavailable", "negative", "positive-then-negative"] {
        let (detector, edges, _) = i2879_detector();
        let id = Uuid::new_v4();
        detector.register_session(id, i2879_codex());
        let t0 = i2879_stamp(&detector, id);
        let handle = detector.activity_consumer(id).expect("Codex attaches");
        match case {
            "none" => {}
            "unavailable" => assert!((handle.observe)(None)),
            "negative" => assert!((handle.observe)(Some(false))),
            _ => {
                assert!((handle.observe)(Some(true)));
                detector.evaluate_tick(Some(t0 + Duration::from_millis(1)), false);
                assert!((handle.observe)(Some(false)));
            }
        }
        detector.evaluate_tick(Some(t0 + threshold), false);
        assert!(
            i2879_edges(&edges).is_empty(),
            "{case}: not past the threshold"
        );
        detector.evaluate_tick(Some(t0 + threshold + Duration::from_millis(1)), false);
        detector.evaluate_tick(Some(t0 + Duration::from_secs(3600)), false);
        assert_eq!(i2879_edges(&edges), vec![("idle", id)], "{case}");
    }

    // The control: the same instants with positive evidence never idle.
    let (detector, edges, _) = i2879_detector();
    let id = Uuid::new_v4();
    detector.register_session(id, i2879_codex());
    let t0 = i2879_stamp(&detector, id);
    let handle = detector.activity_consumer(id).expect("Codex attaches");
    assert!((handle.observe)(Some(true)));
    detector.evaluate_tick(Some(t0 + threshold + Duration::from_millis(1)), false);
    assert!(i2879_edges(&edges).is_empty());
}

/// From idle, a positive under a resize grace and an unconfirmed burst - output that
/// moves no clock - emits `on_busy` exactly once through the real commit, and the
/// observation itself touches no output or silence clock.
#[test]
fn i2879_feature_suppressed_output() {
    let (detector, edges, _) = i2879_detector();
    let id = Uuid::new_v4();
    detector.register_session(
        id,
        IdleTuning {
            activity_detector: ActivityDetectorKind::CodexWorking,
            ..IdleTuning::AGENT_DEFAULT
        },
    );
    detector.set_auto_close_ages_for_test(
        id,
        Duration::from_secs(30 * 60),
        Duration::from_secs(31 * 60),
    );
    detector.set_pty_input_ready_for_test(id);
    let stamp = i2879_stamp(&detector, id);
    let silence = *detector.silence.lock().unwrap().get(&id).unwrap();

    detector.record_resize(id);
    detector.record_activity_with_bytes(id, 500);
    detector.record_output_at(id, 64, true, Instant::now());
    assert!(
        detector.burst.lock().unwrap().contains_key(&id),
        "burst pending"
    );
    detector.evaluate_tick(Some(stamp + Duration::from_secs(3600)), false);
    assert!(
        i2879_edges(&edges).is_empty(),
        "suppressed output and no evidence: still idle, no edge"
    );

    let handle = detector.activity_consumer(id).expect("Codex attaches");
    assert!((handle.observe)(Some(true)));
    assert!(
        i2879_edges(&edges).is_empty(),
        "observing alone emits nothing"
    );
    detector.evaluate_tick(Some(stamp + Duration::from_secs(3600)), false);
    detector.evaluate_tick(Some(stamp + Duration::from_secs(7200)), false);
    assert_eq!(i2879_edges(&edges), vec![("busy", id)], "exactly once");
    assert!(!detector.idle_set.lock().unwrap().contains(&id));
    assert_eq!(
        i2879_stamp(&detector, id),
        stamp,
        "activity clock untouched"
    );
    assert_eq!(
        *detector.silence.lock().unwrap().get(&id).unwrap(),
        silence,
        "silence clock untouched"
    );
}

/// Token identity: a new attach, a retire, `remove_session` and a re-registration each
/// reject every older handle without mutating anything, and nothing positive is
/// inherited by a re-registration or a new UUID. Timed registrations never attach.
#[test]
fn i2879_feature_retire() {
    let (detector, edges, _) = i2879_detector();
    let id = Uuid::new_v4();
    detector.register_session(id, i2879_codex());

    let first = detector.activity_consumer(id).expect("Codex attaches");
    assert!((first.observe)(Some(true)));
    let second = detector.activity_consumer(id).expect("a new attach");
    assert!(
        !(first.observe)(Some(false)),
        "a replaced attach is rejected"
    );
    assert!(
        !(first.retire)(),
        "a late retire of a replaced attach is rejected"
    );
    assert_eq!(
        i2879_registration(&detector, id),
        Some((ActivityDetectorKind::CodexWorking, true, Some(true))),
        "the rejected calls mutated nothing and the new attach kept the evidence"
    );
    assert!((second.observe)(None));

    assert!((second.retire)(), "the current attach retires");
    assert_eq!(
        i2879_registration(&detector, id),
        Some((ActivityDetectorKind::CodexWorking, false, None)),
        "retire releases the evidence and keeps the registration"
    );
    assert!(
        !(second.observe)(Some(true)),
        "a retired attach is rejected"
    );
    assert!(!(second.retire)());

    // Removal (a confirmed Gone or a destroy) invalidates the current attach.
    let third = detector.activity_consumer(id).expect("re-attach");
    assert!((third.observe)(Some(true)));
    detector.remove_session(id);
    assert!(!(third.observe)(Some(true)));
    assert!(!(third.retire)());
    assert!(i2879_registration(&detector, id).is_none());
    assert!(
        detector.activity_consumer(id).is_none(),
        "nothing to attach to"
    );

    // Re-registration of the same UUID starts clean and rejects the old handle.
    detector.register_session(id, i2879_codex());
    assert!(
        !(third.observe)(Some(true)),
        "late callback after re-registration"
    );
    assert_eq!(
        i2879_registration(&detector, id),
        Some((ActivityDetectorKind::CodexWorking, false, None))
    );
    let fourth = detector.activity_consumer(id).expect("attach");
    assert!((fourth.observe)(Some(true)));
    detector.register_session(id, i2879_codex());
    assert!(!(fourth.observe)(Some(true)));
    assert!(!(fourth.retire)());
    assert_eq!(
        i2879_registration(&detector, id),
        Some((ActivityDetectorKind::CodexWorking, false, None)),
        "no positive is inherited"
    );

    // A new UUID never sees another session's evidence; Timed never attaches.
    let other = Uuid::new_v4();
    detector.register_session(other, i2879_codex());
    assert_eq!(
        i2879_registration(&detector, other),
        Some((ActivityDetectorKind::CodexWorking, false, None))
    );
    let timed = Uuid::new_v4();
    detector.register_session(timed, IdleTuning::DEFAULT);
    assert!(detector.activity_consumer(timed).is_none());
    assert_eq!(
        i2879_registration(&detector, timed),
        Some((ActivityDetectorKind::Timed, false, None)),
        "a refused attach mutates nothing"
    );

    // With all evidence gone, the re-registered session follows the timed clock.
    let t0 = i2879_stamp(&detector, id);
    detector.evaluate_tick(Some(t0 + Duration::from_secs(3600)), false);
    assert!(i2879_edges(&edges).contains(&("idle", id)));
}

/// The commit order: every edge is emitted under `activity -> idle_set`, so an edge
/// recorded after an observation returned saw that observation. Deterministic phases
/// first, then a real concurrent interleaving of observation and ticks.
#[test]
fn i2879_feature_commit_order() {
    use std::sync::atomic::Ordering;
    let (detector, edges, order) = i2879_detector();
    let id = Uuid::new_v4();
    detector.register_session(id, i2879_codex());
    let t0 = i2879_stamp(&detector, id);
    let threshold = i2879_codex().idle_threshold;
    let handle = detector.activity_consumer(id).expect("Codex attaches");
    let far = || Instant::now() + Duration::from_secs(3600);

    detector.evaluate_tick(Some(t0 + threshold + Duration::from_millis(1)), false);
    detector.record_activity_with_bytes(id, 3); // input re-opens the turn
    assert!((handle.observe)(Some(true)));
    detector.evaluate_tick(Some(far()), false); // positive vetoes idle
    assert!((handle.observe)(Some(false)));
    detector.evaluate_tick(Some(far()), false); // negative: timed idle
    assert!((handle.observe)(Some(true)));
    detector.evaluate_tick(Some(far()), false); // positive re-opens, once
    detector.evaluate_tick(Some(far()), false);
    detector.record_activity_with_bytes(id, 3); // already busy: no edge
    assert_eq!(
        i2879_edges(&edges),
        vec![("idle", id), ("busy", id), ("idle", id), ("busy", id)]
    );

    // Concurrent: a tick thread races one observation. No idle may be ordered after
    // the observation's commit returned.
    let racing = Uuid::new_v4();
    detector.register_session(racing, i2879_codex());
    let racing_handle = detector.activity_consumer(racing).expect("attach");
    edges.lock().unwrap().clear();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let ticker = {
        let detector = Arc::clone(&detector);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..500 {
                detector.evaluate_tick(Some(Instant::now() + Duration::from_secs(3600)), false);
            }
        })
    };
    barrier.wait();
    assert!((racing_handle.observe)(Some(true)));
    let committed = order.fetch_add(1, Ordering::SeqCst);
    ticker.join().expect("ticker");
    detector.evaluate_tick(Some(far()), false);

    let racing_edges: Vec<(&str, usize)> = edges
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, edge_id, _)| *edge_id == racing)
        .map(|(edge, _, n)| (*edge, *n))
        .collect();
    assert!(
        racing_edges
            .iter()
            .all(|(edge, n)| *edge != "idle" || *n < committed),
        "an idle edge after a committed positive: {racing_edges:?} committed={committed}"
    );
    assert!(racing_edges.iter().filter(|(e, _)| *e == "idle").count() <= 1);
    if racing_edges.iter().any(|(e, _)| *e == "idle") {
        assert_eq!(racing_edges.last().map(|(e, _)| *e), Some("busy"));
    }
    assert!(!detector.idle_set.lock().unwrap().contains(&racing));
}
