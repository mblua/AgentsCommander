use super::*;

mod burst_filter;

mod codex_working;

mod response_close_raw_output;

#[test]
fn register_session_seeds_activity_when_profile_opts_in() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT); // seed = true
    assert!(
        detector.activity.lock().unwrap().contains_key(&id),
        "register_session must seed activity[id] — the #260 fix"
    );
}

#[test]
fn register_session_does_not_seed_when_opted_out() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(
        id,
        IdleTuning {
            seed_initial_activity: false,
            ..IdleTuning::DEFAULT
        },
    );
    assert!(!detector.activity.lock().unwrap().contains_key(&id));
    // ...but the tuning is still recorded.
    assert!(detector.tuning.lock().unwrap().contains_key(&id));
}

/// Acceptance criterion #1 — the grinch stuck-session regression test.
/// A codex session whose entire visible output was suppressed (resize
/// grace) / escape-only (SKIPPED), so `record_activity_with_bytes` NEVER
/// ran. With the #260 seed it is still in `activity` and the watcher
/// transitions it busy→idle after `idle_threshold`. Revert the seed in
/// `register_session` and the `.expect(...)` below panics → this fails.
#[test]
fn seeded_silent_session_crosses_idle_threshold() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    let tuning = IdleTuning::DEFAULT;
    detector.register_session(id, tuning);

    let seeded_at = *detector
        .activity
        .lock()
        .unwrap()
        .get(&id)
        .expect("register_session must seed activity[id] — the #260 fix");

    let activity = detector.activity.lock().unwrap().clone();
    let idle_set: HashSet<Uuid> = HashSet::new();
    let mut tuning_map = HashMap::new();
    tuning_map.insert(id, tuning);

    // Before the threshold: no transition.
    let early = sessions_crossing_idle_threshold(
        seeded_at + tuning.idle_threshold - Duration::from_millis(100),
        &activity,
        &idle_set,
        &tuning_map,
    );
    assert!(
        early.is_empty(),
        "must not transition before idle_threshold"
    );

    // After idle_threshold of pure silence: transition fires even though
    // record_activity_with_bytes was NEVER called for this session.
    let crossed = sessions_crossing_idle_threshold(
        seeded_at + tuning.idle_threshold + Duration::from_millis(100),
        &activity,
        &idle_set,
        &tuning_map,
    );
    let ids: Vec<Uuid> = crossed.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![id], "seeded silent session must reach idle");
}

/// Documents the bug mechanism: WITHOUT a seed, an all-suppressed session
/// is absent from `activity` and the watcher never even evaluates it —
/// no matter how much time passes.
#[test]
fn unseeded_session_never_transitions() {
    let id = Uuid::new_v4();
    let activity: HashMap<Uuid, Instant> = HashMap::new(); // never seeded
    let idle_set: HashSet<Uuid> = HashSet::new();
    let mut tuning_map = HashMap::new();
    tuning_map.insert(id, IdleTuning::DEFAULT);

    let crossed = sessions_crossing_idle_threshold(
        Instant::now() + Duration::from_secs(3600),
        &activity,
        &idle_set,
        &tuning_map,
    );
    assert!(
        crossed.is_empty(),
        "an un-seeded session is invisible to the watcher — the #260 bug"
    );
}

/// The resize-grace suppression that contributes to the bug must not
/// clear the seed: output arriving inside the grace window is suppressed,
/// but the seeded `activity[id]` survives so the watcher can still act.
#[test]
fn resize_grace_suppression_preserves_the_seed() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    detector.record_resize(id);
    // "Initial output" arrives inside RESIZE_GRACE → suppressed.
    detector.record_activity_with_bytes(id, 500);
    assert!(
        detector.activity.lock().unwrap().contains_key(&id),
        "the seed must survive resize-grace suppression"
    );
}

/// dev-rust R1.5 — guards the `tuning.remove` line §6.1 adds to
/// `remove_session`; without it a future detector-map leak is silent.
#[test]
fn remove_session_clears_tuning() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    assert!(detector.tuning.lock().unwrap().contains_key(&id));
    detector.remove_session(id);
    assert!(
        !detector.tuning.lock().unwrap().contains_key(&id),
        "remove_session must drop the tuning entry"
    );
    assert!(!detector.activity.lock().unwrap().contains_key(&id));
}

/// #552: touch_silence updates only the `silence` map; it must NOT insert
/// into `activity`/`idle_set` or fire on_busy, or it would couple the
/// auto-close clock to the #260 idle dot.
#[test]
fn touch_silence_is_decoupled_from_idle_dot() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let busy_calls = Arc::new(AtomicUsize::new(0));
    let busy_calls_cb = Arc::clone(&busy_calls);
    let detector = IdleDetector::new(
        move |_| {},
        move |_| {
            busy_calls_cb.fetch_add(1, Ordering::SeqCst);
        },
    );
    let id = Uuid::new_v4();

    detector.touch_silence(id);

    assert!(
        detector.silence.lock().unwrap().contains_key(&id),
        "touch_silence must populate the silence map"
    );
    assert!(
        !detector.activity.lock().unwrap().contains_key(&id),
        "touch_silence must NOT touch activity (the #260 idle-dot signal)"
    );
    assert!(
        !detector.idle_set.lock().unwrap().contains(&id),
        "touch_silence must NOT touch idle_set"
    );
    assert_eq!(
        busy_calls.load(Ordering::SeqCst),
        0,
        "touch_silence must NOT fire on_busy"
    );
}

/// #552: silence_age grows over time and is None for an untracked id.
#[test]
fn silence_age_increases_and_is_none_when_untracked() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();

    assert!(
        detector.silence_age(id).is_none(),
        "an untracked session has no silence age"
    );

    detector.touch_silence(id);
    let first = detector.silence_age(id).expect("tracked now");
    std::thread::sleep(Duration::from_millis(10));
    let second = detector.silence_age(id).expect("still tracked");
    assert!(
        second >= first,
        "silence age must be monotonic non-decreasing ({second:?} >= {first:?})"
    );
}

/// #552: the silence seed in register_session is UNCONDITIONAL, unlike the
/// activity seed which is gated on `seed_initial_activity`. A live member
/// with no silence baseline would be treated as a spawn race and protected,
/// so the seed must fire even when the profile opts out of the activity seed.
#[test]
fn register_session_seeds_silence_even_when_activity_seed_disabled() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(
        id,
        IdleTuning {
            seed_initial_activity: false,
            ..IdleTuning::DEFAULT
        },
    );
    assert!(
        !detector.activity.lock().unwrap().contains_key(&id),
        "activity seed is gated off here"
    );
    assert!(
        detector.silence.lock().unwrap().contains_key(&id),
        "silence seed must be unconditional (#552)"
    );
    assert!(
        detector.silence_age(id).is_some(),
        "a registered session has a silence baseline age"
    );
}

/// #552: remove_session must clear the silence entry (no leak on destroy).
#[test]
fn remove_session_clears_silence() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    assert!(detector.silence.lock().unwrap().contains_key(&id));
    detector.remove_session(id);
    assert!(
        !detector.silence.lock().unwrap().contains_key(&id),
        "remove_session must drop the silence entry"
    );
}

/// (#580): register_session seeds registered_at; alive_age is Some after
/// register and None after remove_session (the WAKE_GRACE clock lifecycle).
#[test]
fn register_session_seeds_registered_at_and_alive_age() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();

    assert!(
        detector.alive_age(id).is_none(),
        "an unregistered session has no alive age"
    );

    detector.register_session(id, IdleTuning::DEFAULT);
    assert!(
        detector.registered_at.lock().unwrap().contains_key(&id),
        "register_session must seed registered_at (#580)"
    );
    assert!(
        detector.alive_age(id).is_some(),
        "a registered session has an alive age"
    );

    detector.remove_session(id);
    assert!(
        !detector.registered_at.lock().unwrap().contains_key(&id),
        "remove_session must drop the registered_at entry"
    );
    assert!(
        detector.alive_age(id).is_none(),
        "alive_age is None after remove_session"
    );
}

/// (#580): alive_age grows monotonically (non-decreasing).
#[test]
fn alive_age_is_monotonic() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);

    let first = detector.alive_age(id).expect("tracked after register");
    std::thread::sleep(Duration::from_millis(10));
    let second = detector.alive_age(id).expect("still tracked");
    assert!(
        second >= first,
        "alive age must be monotonic non-decreasing ({second:?} >= {first:?})"
    );
}

// ── (#885) purge_readiness tests ──

#[test]
fn purge_readiness_snapshot_is_consistent() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    detector.record_activity_with_bytes(id, 42);

    let readiness = detector.purge_readiness(&[id]);
    assert_eq!(readiness.len(), 1);
    let r = &readiness[0];
    assert_eq!(r.session_id, id);
    assert!(
        r.activity_age.is_some(),
        "activity_age must be Some after record_activity"
    );
    assert!(
        r.activity_age.unwrap() < Duration::from_secs(1),
        "activity_age must be small right after record_activity"
    );
    assert!(
        !r.watcher_idle,
        "watcher_idle must be false right after activity"
    );
}

#[test]
fn purge_readiness_untracked_is_none() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let untracked = Uuid::new_v4();

    let readiness = detector.purge_readiness(&[untracked]);
    assert_eq!(readiness.len(), 1);
    assert!(
        readiness[0].activity_age.is_none(),
        "activity_age must be None for an unregistered session"
    );
}

#[test]
fn has_printable_activity_since_compares_stamp_to_reference() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);

    // A reference that predates the recorded stamp: activity-since is true.
    let before = Instant::now() - Duration::from_millis(50);
    detector.record_activity_with_bytes(id, 5);
    assert!(
        detector.has_printable_activity_since(id, before),
        "a recorded stamp is strictly after an earlier reference"
    );

    // A reference strictly after the last stamp: no newer activity.
    let after = Instant::now() + Duration::from_millis(50);
    assert!(
        !detector.has_printable_activity_since(id, after),
        "no activity after a future reference"
    );

    // Untracked session: never any activity.
    assert!(!detector.has_printable_activity_since(Uuid::new_v4(), before));
}

#[test]
fn purge_readiness_reports_resize_age() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    detector.record_resize(id);

    let readiness = detector.purge_readiness(&[id]);
    let r = &readiness[0];
    assert!(
        r.last_resize_age.is_some(),
        "last_resize_age must be Some after record_resize"
    );
    assert!(
        r.last_resize_age.unwrap() < Duration::from_secs(1),
        "last_resize_age must be small right after record_resize"
    );
    assert_eq!(
        r.resize_grace,
        IdleTuning::DEFAULT.resize_grace,
        "resize_grace must be the session's tuning value"
    );
}

#[test]
fn pty_input_boundary_revalidates_idle_and_defers_busy_callback() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let busy_calls = Arc::new(AtomicUsize::new(0));
    let busy_calls_for_callback = Arc::clone(&busy_calls);
    let detector = IdleDetector::new(
        |_| {},
        move |_| {
            busy_calls_for_callback.fetch_add(1, Ordering::SeqCst);
        },
    );
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    detector.activity.lock().unwrap().insert(
        id,
        Instant::now() - IdleTuning::DEFAULT.idle_threshold - Duration::from_secs(3),
    );
    detector.idle_set.lock().unwrap().insert(id);
    let route_calls = AtomicUsize::new(0);
    let (marker, was_idle) = detector
        .prepare_pty_input_boundary(id, || {
            route_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, crate::errors::AppError>("route")
        })
        .unwrap();
    assert_eq!(marker, "route");
    assert!(was_idle);
    assert_eq!(route_calls.load(Ordering::SeqCst), 1);
    assert_eq!(busy_calls.load(Ordering::SeqCst), 0);
    assert!(!detector.idle_set.lock().unwrap().contains(&id));
    assert!(detector.purge_readiness(&[id])[0]
        .activity_age
        .is_some_and(|age| age < Duration::from_secs(1)));
    detector.notify_pty_input_busy(id, was_idle);
    assert_eq!(busy_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn pty_input_boundary_rejects_busy_and_unsettled_resize_before_route_lock() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    let route_calls = AtomicUsize::new(0);
    let busy = detector.prepare_pty_input_boundary(id, || {
        route_calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, crate::errors::AppError>(())
    });
    assert_eq!(busy.unwrap_err(), PtyInputBoundaryFailure::Busy);
    assert_eq!(route_calls.load(Ordering::SeqCst), 0);

    detector.activity.lock().unwrap().insert(
        id,
        Instant::now() - IdleTuning::DEFAULT.idle_threshold - Duration::from_secs(3),
    );
    detector.idle_set.lock().unwrap().insert(id);
    detector.record_resize(id);
    let resize = detector.prepare_pty_input_boundary(id, || {
        route_calls.fetch_add(1, Ordering::SeqCst);
        Ok::<_, crate::errors::AppError>(())
    });
    assert_eq!(
        resize.unwrap_err(),
        PtyInputBoundaryFailure::ResizeUnsettled
    );
    assert_eq!(route_calls.load(Ordering::SeqCst), 0);
}

/// (#885 F-1) The core bug: inside resize grace, `record_activity_with_bytes`
/// early-returns WITHOUT touching `activity`, so `activity_age` grows
/// without bound while the child is "printing". This test pins the bug so
/// nobody deletes the fourth gate leg.
#[test]
fn resize_grace_freezes_activity_age() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);

    // Record initial activity.
    detector.record_activity_with_bytes(id, 10);
    // Enter resize grace.
    detector.record_resize(id);
    // Sleep so a refresh would be detectable: if the early-return were
    // removed, the next record_activity_with_bytes would reset activity
    // to ~0ms, making age << 200ms.
    std::thread::sleep(Duration::from_millis(200));
    // Simulate the child printing during grace.
    detector.record_activity_with_bytes(id, 99);

    // activity must NOT have moved: the early-return at :159 suppressed it.
    // An un-suppressed refresh would reset age to ~0, so age would be < 200ms.
    let age = detector.purge_readiness(&[id])[0].activity_age.unwrap();
    assert!(
        age >= Duration::from_millis(200),
        "activity must be FROZEN during resize grace; an un-suppressed \
         refresh would reset age to ~0. age={age:?}. \
         If this fails, record_activity_with_bytes is no longer \
         early-returning during grace, and the F-1 fourth gate leg \
         may be unnecessary."
    );
}

/// #1682 - the executable form of the `idle_set` half of D1. Its exact
/// property is "this write found the session idle"; the first half is the
/// child-solicited device-status reply arriving mid-render, which must not
/// mark and so must not cancel the genuine turn that is under way.
#[test]
fn a_control_write_marks_only_when_it_finds_the_session_idle() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    assert!(
        detector.control_write_age(id).is_none(),
        "a freshly registered session carries no mark"
    );

    detector.record_control_write_activity(id, 3);
    assert!(
        detector.control_write_age(id).is_none(),
        "a control write into a BUSY session must not mark: this is the \
         assertion that goes red if the idle_set half is dropped"
    );

    // The in-test control: the same call on an idle session does mark.
    detector.set_pty_input_ready_for_test(id);
    detector.record_control_write_activity(id, 3);
    assert!(
        detector.control_write_age(id).is_some(),
        "a control write that FOUND the session idle must mark"
    );
}

/// #1682 - the refresh half of D1: a ten-second burst of arrow keys must not
/// age out mid-burst, even though only its first write finds the session idle.
#[test]
fn a_standing_control_write_mark_refreshes_while_the_session_is_busy() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    detector.set_pty_input_ready_for_test(id);
    detector.record_control_write_activity(id, 1);
    assert!(detector.control_write_age(id).is_some());

    detector.set_control_write_age_for_test(id, Duration::from_secs(30));
    assert!(
        detector.control_write_age(id).expect("the mark stands") >= Duration::from_secs(30),
        "the backdating helper must age the standing mark"
    );
    assert!(
        !detector.idle_set.lock().unwrap().contains(&id),
        "the first write made the session busy"
    );

    detector.record_control_write_activity(id, 1);
    assert!(
        detector
            .control_write_age(id)
            .expect("the mark still stands")
            < Duration::from_secs(1),
        "a later write must REFRESH the standing mark: this is the assertion \
         that goes red if the refresh half is dropped"
    );
}

/// #1682 - arming clears the mark, and the tail is the control: without it the
/// test passes on a `control_write_age` that always returns `None`.
#[test]
fn clear_control_write_cancels_the_mark_and_a_later_write_sets_it_again() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    detector.set_pty_input_ready_for_test(id);
    detector.record_control_write_activity(id, 1);
    assert!(detector.control_write_age(id).is_some());

    detector.clear_control_write(id);
    assert!(
        detector.control_write_age(id).is_none(),
        "clear_control_write must cancel the standing mark"
    );

    detector.set_pty_input_ready_for_test(id);
    detector.record_control_write_activity(id, 1);
    assert!(
        detector.control_write_age(id).is_some(),
        "a later control write must be able to set the mark again"
    );
}

/// #1682 - the regression guard for the #260 idle dot: apart from the mark,
/// `record_control_write_activity` must behave exactly as
/// `record_activity_with_bytes` does.
#[test]
fn record_control_write_activity_still_produces_the_busy_edge() {
    let busy: Arc<Mutex<Vec<Uuid>>> = Arc::new(Mutex::new(Vec::new()));
    let busy_cb = Arc::clone(&busy);
    let detector = IdleDetector::new(
        |_| {},
        move |id| {
            busy_cb.lock().unwrap().push(id);
        },
    );
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    detector.set_pty_input_ready_for_test(id);
    assert!(
        busy.lock().unwrap().is_empty(),
        "no busy edge before the write"
    );

    detector.record_control_write_activity(id, 7);

    assert_eq!(
        *busy.lock().unwrap(),
        vec![id],
        "the new method must still fire on_busy exactly once"
    );
    assert!(
        !detector.idle_set.lock().unwrap().contains(&id),
        "the session must have left idle_set"
    );
}

/// #1682 - teardown drops the mark, so a destroyed and recreated session id
/// cannot inherit one. Mirrors `remove_session_clears_tuning`.
#[test]
fn remove_session_clears_the_control_write_mark() {
    let detector = IdleDetector::new(|_| {}, |_| {});
    let id = Uuid::new_v4();
    detector.register_session(id, IdleTuning::DEFAULT);
    detector.set_pty_input_ready_for_test(id);
    detector.record_control_write_activity(id, 1);
    assert!(detector.control_write_age(id).is_some());

    detector.remove_session(id);
    assert!(
        detector.control_write_age(id).is_none(),
        "remove_session must drop the control-write mark"
    );
}
