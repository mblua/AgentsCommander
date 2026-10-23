use super::*;
use crate::capture::record::{CapturedRecord, RecordOrigin};
use std::sync::Arc;

fn room(temp: &tempfile::TempDir) -> PathBuf {
    let room = temp.path().join("room-1-dev-team");
    std::fs::create_dir_all(&room).expect("room dir");
    forget_room_for_tests(&room);
    room
}

fn obs(len: u64, prefix: &[u8]) -> FileObservation {
    FileObservation::new(len, prefix.to_vec())
}

fn record(file: &Path, text: &str, start: Option<u64>, seq: u64) -> Arc<CapturedRecord> {
    record_with_origin(file, text, start, seq, RecordOrigin::Live)
}

fn record_with_origin(
    file: &Path,
    text: &str,
    start: Option<u64>,
    seq: u64,
    origin: RecordOrigin,
) -> Arc<CapturedRecord> {
    let text_sha256: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes()).into();
    Arc::new(CapturedRecord {
        session_id: "session-a".to_owned(),
        text: text.to_owned(),
        file: file.to_path_buf(),
        epoch: 0,
        record_start: start,
        reader_seq: seq,
        text_sha256,
        turn_id: None,
        provider: CaptureProvider::Claude,
        provider_final: false,
        turn_identified: false,
        origin,
        observed_path: file.to_path_buf(),
        observed_len: 0,
        observed_prefix: Vec::new(),
    })
}

fn preconditions(kind: EffectKind) -> EffectPreconditions {
    EffectPreconditions {
        session_alive: true,
        session_id: "session-a".to_owned(),
        anchor: "agent".to_owned(),
        provider: CaptureProvider::Claude,
        unique_live_session_for_cwd: true,
        no_pending_user_input: true,
        effective_ready: true,
        observed_at: Instant::now(),
        kind,
    }
}

/// Publish `record` and return `(seq, key)` for a commit.
fn arm(slot: &CaptureSlot, record: Arc<CapturedRecord>, epoch: u64) -> (u64, ConsumptionKey) {
    slot.offer(Arc::clone(&record), epoch);
    (slot.seq(), ConsumptionKey::from_record(&record, epoch))
}

// Acceptance criterion 10, in a test as well as in the const assertion.
#[test]
fn the_lock_wait_is_strictly_below_the_staleness_bound() {
    assert!(LOCK_WAIT_BUDGET < STALE_PRECONDITIONS);
    assert_eq!(LOCK_WAIT_BUDGET, Duration::from_millis(100));
    assert_eq!(STALE_PRECONDITIONS, Duration::from_millis(250));
}

// Test 7: truncation advances the epoch exactly once; a second observer of
// the same truncation does not advance it again.
#[test]
fn truncation_advances_the_epoch_exactly_once() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");

    assert_eq!(observe(&root, &file, obs(100, b"head-aaaa")), 0);
    assert_eq!(observe(&root, &file, obs(10, b"head-aaaa")), 1);
    // A second observer sees the same truncation already recorded.
    assert_eq!(observe(&root, &file, obs(10, b"head-aaaa")), 1);
    assert_eq!(epoch_of(&root, &file), 1);
}

#[test]
fn appending_never_advances_the_epoch() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    assert_eq!(observe(&root, &file, obs(100, b"aaaa")), 0);
    assert_eq!(observe(&root, &file, obs(110, b"aaaa")), 0);
    assert_eq!(observe(&root, &file, obs(4096, b"aaaa")), 0);
}

// Test 10: reading file B and returning to file A recovers A's watermark,
// and A's epoch is unchanged.
#[test]
fn returning_to_a_file_recovers_its_watermark_and_epoch() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let a = temp.path().join("a.jsonl");
    let b = temp.path().join("b.jsonl");
    std::fs::write(&a, b"a").expect("write a");
    std::fs::write(&b, b"b").expect("write b");

    observe(&root, &a, obs(100, b"aaaa"));
    observe(&root, &a, obs(40, b"aaaa")); // truncation: epoch 1
    let epoch_a = epoch_of(&root, &a);
    assert_eq!(epoch_a, 1);

    let slot = CaptureSlot::new();
    let record_a = record(&a, "from a", Some(30), 0);
    let (seq, key_a) = arm(&slot, record_a, epoch_a);
    commit_effect(
        &root,
        &slot,
        seq,
        &key_a,
        &preconditions(EffectKind::Automatic),
    )
    .expect("committed");
    assert_eq!(watermark(&root, &a), Some(30));

    // Read B in between; it neither moves A's epoch nor its watermark.
    observe(&root, &b, obs(10, b"bbbb"));
    observe(&root, &b, obs(1, b"bbbb"));
    assert_eq!(epoch_of(&root, &a), epoch_a);
    assert_eq!(watermark(&root, &a), Some(30));

    // Back to A: an appending observation keeps the epoch.
    assert_eq!(observe(&root, &a, obs(50, b"aaaa")), epoch_a);
}

// Test 9: watermark `None` plus a record at offset 0 acts.
#[test]
fn an_unconsumed_file_acts_on_offset_zero() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let key = ConsumptionKey::from_record(&record(&file, "first", Some(0), 0), 0);
    assert_eq!(watermark(&root, &file), None);
    assert!(is_actionable(&root, "session-a", &key, Some(0)));
}

#[test]
fn a_record_at_or_below_the_watermark_and_out_of_the_recent_set_abstains() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let slot = CaptureSlot::new();
    let (seq, key) = arm(&slot, record(&file, "first", Some(120), 0), 0);
    commit_effect(
        &root,
        &slot,
        seq,
        &key,
        &preconditions(EffectKind::Automatic),
    )
    .expect("committed");

    // In the recent set: known-consumed, not actionable.
    assert!(!is_actionable(&root, "session-a", &key, Some(120)));
    // A different record below the watermark: consumed and pruned, abstain.
    let older = ConsumptionKey::from_record(&record(&file, "older", Some(30), 1), 0);
    assert!(!is_actionable(&root, "session-a", &older, Some(30)));
    // Above the watermark: new, act.
    let newer = ConsumptionKey::from_record(&record(&file, "newer", Some(400), 2), 0);
    assert!(is_actionable(&root, "session-a", &newer, Some(400)));
}

#[test]
fn only_missing_paths_are_pruned() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let alive = temp.path().join("alive.jsonl");
    let gone = temp.path().join("gone.jsonl");
    std::fs::write(&alive, b"x").expect("write");
    std::fs::write(&gone, b"x").expect("write");
    observe(&root, &alive, obs(1, b"x"));
    observe(&root, &gone, obs(1, b"x"));
    std::fs::remove_file(&gone).expect("remove");

    prune_missing_paths(&root);
    assert_eq!(epoch_of(&root, &alive), 0);
    assert!(with_room(&root, |room| room
        .epochs
        .keys()
        .all(|path| path != gone.as_path())));
}

// Test 12 (state half): an advanced epoch drops the recorded cut.
#[test]
fn an_advanced_epoch_supersedes_the_recorded_cut() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    std::fs::write(&file, b"x").expect("write");
    let slot = CaptureSlot::new();
    observe(&root, &file, obs(500, b"aaaa"));
    register_cut(
        &root,
        &slot,
        Cut {
            path: crate::capture::key::normalise_path(&file),
            epoch: 0,
            len: 500,
            reader_seq: Some(9),
        },
    );
    assert!(cut(&root).is_some());
    observe(&root, &file, obs(10, b"aaaa"));
    assert!(cut(&root).is_none(), "truncation must supersede the cut");
}

// Test 15: three automatic actions with no human input leave the fourth
// refused.
#[test]
fn the_fourth_automatic_action_is_refused() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let slot = CaptureSlot::new();
    for i in 0..3u64 {
        let (seq, key) = arm(
            &slot,
            record(&file, &format!("turn {i}"), Some(i * 10), i),
            0,
        );
        let committed = commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic),
        )
        .expect("committed");
        assert_eq!(committed.budget_remaining, 2 - u32::try_from(i).unwrap());
        slot.try_consume(seq, &key, 0);
    }
    let (seq, key) = arm(&slot, record(&file, "turn 3", Some(40), 3), 0);
    assert_eq!(
        commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic)
        ),
        Err(AbstainReason::BudgetExhausted)
    );
    assert_eq!(budget(&root), 0);

    // Substantive human input recharges to the cap.
    recharge_in_memory(&root);
    assert_eq!(budget(&root), BUDGET_CAP);
    let (seq, key) = arm(&slot, record(&file, "turn 4", Some(50), 4), 0);
    assert!(commit_effect(
        &root,
        &slot,
        seq,
        &key,
        &preconditions(EffectKind::Automatic)
    )
    .is_ok());
}

// Test 16: a text-to-user action does not decrement.
#[test]
fn a_text_to_user_action_does_not_decrement() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let slot = CaptureSlot::new();
    for i in 0..5u64 {
        let (seq, key) = arm(
            &slot,
            record(&file, &format!("note {i}"), Some(i * 10), i),
            0,
        );
        let committed = commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::TextToUser),
        )
        .expect("committed");
        assert_eq!(committed.budget_remaining, BUDGET_CAP);
        slot.try_consume(seq, &key, 0);
    }
    assert_eq!(budget(&root), BUDGET_CAP);
}

// Test 18: flipping `enabled` to false between the snapshot and the commit
// yields zero effects.
#[test]
fn a_room_turned_off_after_the_snapshot_commits_nothing() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let slot = CaptureSlot::new();
    let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);

    let mut pre = preconditions(EffectKind::Automatic);
    pre.effective_ready = false;
    assert_eq!(
        commit_effect(&root, &slot, seq, &key, &pre),
        Err(AbstainReason::PreconditionsRejected)
    );
    assert_eq!(budget(&root), BUDGET_CAP);
    assert_eq!(watermark(&root, &file), None);
    assert!(matches!(
        slot.snapshot().value,
        crate::capture::sink::SlotValue::Valid(_)
    ));
}

// Test 19: an aged snapshot abstains with `PreconditionsStale`, mutates
// nothing, and creates no lock file — which proves the check ran before the
// acquisition.
#[test]
fn an_aged_snapshot_abstains_before_touching_the_lock() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let slot = CaptureSlot::new();
    let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);

    let mut pre = preconditions(EffectKind::Automatic);
    pre.observed_at = Instant::now() - STALE_PRECONDITIONS - Duration::from_millis(10);
    assert_eq!(
        commit_effect(&root, &slot, seq, &key, &pre),
        Err(AbstainReason::PreconditionsStale)
    );
    assert_eq!(budget(&root), BUDGET_CAP);
    assert_eq!(watermark(&root, &file), None);
    assert!(
        !lock_path(&root).exists(),
        "the staleness check must run before the lock is opened"
    );
    assert!(!co_managed_dir(&root).exists());
}

// Test 20: a crash between step 5 and step 7 leaves the key marked consumed
// and the budget decremented: at-most-once, no double action.
#[test]
fn a_crash_after_the_commit_cannot_double_act() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let slot = CaptureSlot::new();
    let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);
    let committed = commit_effect(
        &root,
        &slot,
        seq,
        &key,
        &preconditions(EffectKind::Automatic),
    )
    .expect("committed");
    assert_eq!(committed.budget_remaining, BUDGET_CAP - 1);

    // The supervisor dies here: the effect never happens. Simulate the
    // restart by dropping the in-memory room and reloading `state.json`.
    forget_room_for_tests(&root);
    assert_eq!(budget(&root), BUDGET_CAP - 1);
    assert!(!is_actionable(&root, "session-a", &key, Some(10)));
    assert_eq!(
        commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic)
        ),
        Err(AbstainReason::AlreadyConsumed),
        "the orphan is reported, never retried into a second action"
    );
}

// Test 21: `commit_effect` holds no `await`. It is not `async`, and a
// current-thread runtime with no reactor work drives it to completion.
#[test]
fn commit_effect_completes_on_a_current_thread_runtime() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let slot = CaptureSlot::new();
        let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);
        let outcome: Result<CommittedEffect, AbstainReason> = commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic),
        );
        assert!(outcome.is_ok());
    });
}

/// Hold `.co-managed/lock` for `hold`, signalling once it is really held.
/// The join handle returns how long the lock was actually held: a loaded
/// runner can deschedule the holder past its sleep, and a caller must be
/// able to tell that scheduler overshoot from a code defect.
fn hold_lock(
    root: &Path,
    hold: Duration,
) -> (
    std::sync::mpsc::Receiver<()>,
    std::thread::JoinHandle<Duration>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    let root = root.to_path_buf();
    let handle = std::thread::spawn(move || {
        let LockAttempt::Held(lock) = acquire_lock(&root, Duration::from_secs(5)) else {
            panic!("the holder must get the lock");
        };
        let acquired = Instant::now();
        tx.send(()).expect("signal");
        std::thread::sleep(hold);
        drop(lock);
        acquired.elapsed()
    });
    (rx, handle)
}

// Test 24: contention is `LockBusy`, not `PreconditionsStale`, and it is
// bounded. This is the test round 2's staleness test could not have
// written, because it never took contention.
#[test]
fn contention_is_lock_busy_and_bounded() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let slot = CaptureSlot::new();
    let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);

    let (ready, holder) = hold_lock(&root, Duration::from_millis(300));
    ready.recv().expect("holder ready");

    let pre = preconditions(EffectKind::Automatic); // a fresh snapshot
    let started = Instant::now();
    let outcome = commit_effect(&root, &slot, seq, &key, &pre);
    let elapsed = started.elapsed();
    let snapshot_age = pre.observed_at.elapsed();
    holder.join().expect("holder");

    assert_eq!(outcome, Err(AbstainReason::LockBusy));
    assert!(
        elapsed <= LOCK_WAIT_BUDGET + Duration::from_millis(150),
        "gave up after {elapsed:?}, snapshot age {snapshot_age:?}"
    );
    assert!(
        snapshot_age < STALE_PRECONDITIONS + Duration::from_millis(150),
        "the snapshot was still fresh when the call gave up: \
         age {snapshot_age:?}, elapsed {elapsed:?}"
    );
    assert_eq!(budget(&root), BUDGET_CAP);
    assert_eq!(watermark(&root, &file), None);
}

// Test 25: the staleness window is not consumed by the lock wait. Together
// with test 24 this pins the ordering of the two bounds, not their values
// in isolation. The plan allows scheduler slack: a holder the runner
// deschedules past LOCK_WAIT_BUDGET legitimately yields LockBusy, which is
// not the same result as the staleness check consuming the lock wait.
#[test]
fn a_short_lock_wait_still_leaves_the_snapshot_fresh() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let slot = CaptureSlot::new();
    let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);

    let (ready, holder) = hold_lock(&root, Duration::from_millis(25));
    ready.recv().expect("holder ready");

    let mut pre = preconditions(EffectKind::Automatic);
    pre.observed_at = Instant::now() - Duration::from_millis(75);
    let outcome = commit_effect(&root, &slot, seq, &key, &pre);
    let held = holder.join().expect("holder");

    match outcome {
        Ok(_) => assert_eq!(watermark(&root, &file), Some(10)),
        // Slop is the runner's, not the code's: accept LockBusy only when
        // the measured hold really did outlive the budget.
        Err(AbstainReason::LockBusy) => assert!(
            held > LOCK_WAIT_BUDGET,
            "LockBusy although the holder released after only {held:?}, \
             inside the {LOCK_WAIT_BUDGET:?} budget"
        ),
        Err(other) => panic!(
            "a short lock wait must not consume the staleness window: {other:?} \
             (held {held:?})"
        ),
    }
}

// Test 26: a held lock bounds the debounce flush, leaves the dirty stamp
// intact, and a later flush after release writes the recharged budget.
#[test]
fn a_held_lock_bounds_the_flush_and_the_next_one_writes() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    recharge_in_memory(&root);
    with_room(&root, |room| {
        room.spends_since_recharge = 2;
        room.dirty = true;
    });
    recharge_in_memory(&root);

    let (ready, holder) = hold_lock(&root, Duration::from_millis(300));
    ready.recv().expect("holder ready");
    let started = Instant::now();
    let outcome = flush_room(&root);
    let elapsed = started.elapsed();

    assert_eq!(outcome, FlushOutcome::LockBusy);
    assert!(
        elapsed <= LOCK_WAIT_BUDGET + Duration::from_millis(150),
        "the flush blocked for {elapsed:?}"
    );
    assert!(
        with_room(&root, |room| room.dirty),
        "a busy lock must leave the room dirty for the next flush"
    );
    assert!(!state_path(&root).exists());

    holder.join().expect("holder");
    assert_eq!(flush_room(&root), FlushOutcome::Written);
    assert!(!with_room(&root, |room| room.dirty));
    let persisted = read_persisted(&root);
    assert_eq!(persisted.budget, Some(BUDGET_CAP));
}

#[test]
fn a_recharge_survives_a_state_file_written_by_another_instance() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");

    // Another instance leaves an exhausted budget on disk.
    std::fs::create_dir_all(co_managed_dir(&root)).expect("dir");
    write_state_atomic(
        &root,
        &PersistedRoomState {
            budget: Some(0),
            recharge_stamp: 0,
            ..PersistedRoomState::default()
        },
    )
    .expect("seed");

    // This instance observes substantive human input.
    assert_eq!(budget(&root), 0);
    recharge_in_memory(&root);

    let slot = CaptureSlot::new();
    let (seq, key) = arm(&slot, record(&file, "turn", Some(10), 0), 0);
    let committed = commit_effect(
        &root,
        &slot,
        seq,
        &key,
        &preconditions(EffectKind::Automatic),
    )
    .expect("the recharge must not be lost under the lock");
    assert_eq!(committed.budget_remaining, BUDGET_CAP - 1);
}

#[test]
fn the_recent_set_is_bounded_at_its_capacity() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    for i in 0..(RECENT_CAPACITY as u64 + 10) {
        let key = ConsumptionKey::from_record(&record(&file, &format!("t{i}"), Some(i), i), 0);
        with_room(&root, |room| room.remember("session-a", key));
    }
    let held = with_room(&root, |room| room.recent["session-a"].len());
    assert_eq!(held, RECENT_CAPACITY);
}

// Test 13: preamble and rotation-backfill records are consumed and never
// marked routable. Without this a rotated Claude transcript's first sweep
// would be acted on automatically, which is the case section 8.3 forbids.
#[test]
fn a_baseline_is_consumed_but_never_routed() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let slot = CaptureSlot::new();

    for (origin, start) in [
        (RecordOrigin::Preamble, None),
        (RecordOrigin::RotationBackfill, Some(0)),
    ] {
        let record = record_with_origin(&file, &format!("{origin:?}"), start, 0, origin);
        let (seq, key) = arm(&slot, Arc::clone(&record), 0);

        // A starved runner can age `pre` past STALE_PRECONDITIONS before the
        // check (#2389). Retry only when that is measurably what happened;
        // a stale abstain mutates nothing, and test 19 pins the staleness path.
        let mut attempts = 0;
        let committed = loop {
            attempts += 1;
            let pre = preconditions(EffectKind::Automatic);
            match commit_effect(&root, &slot, seq, &key, &pre) {
                Err(AbstainReason::PreconditionsStale)
                    if attempts < 5 && pre.observed_at.elapsed() > STALE_PRECONDITIONS => {}
                result => {
                    break result.expect("a baseline commits, so it can never be offered again")
                }
            }
        };

        assert!(
            !committed.routable,
            "{origin:?} must never be routed (section 8)"
        );
        assert_eq!(
            committed.budget_remaining, BUDGET_CAP,
            "{origin:?} must not spend budget"
        );
        // Consumed: the key is remembered, so a re-delivery abstains.
        assert!(!is_actionable(&root, "session-a", &key, start));
        slot.try_consume(seq, &key, 0);
    }

    assert_eq!(budget(&root), BUDGET_CAP);
    assert!(is_baseline(&record_with_origin(
        &file,
        "p",
        None,
        0,
        RecordOrigin::Preamble
    )));
    assert!(is_baseline(&record_with_origin(
        &file,
        "r",
        Some(0),
        0,
        RecordOrigin::RotationBackfill
    )));
    assert!(!is_baseline(&record(&file, "live", Some(0), 0)));
}

/// A live record in the same room still routes and still spends, so test 13
/// pins the gate rather than a blanket refusal.
#[test]
fn a_live_record_is_still_routable_after_a_baseline() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    let slot = CaptureSlot::new();

    let backfill = record_with_origin(&file, "history", Some(0), 0, RecordOrigin::RotationBackfill);
    let (seq, key) = arm(&slot, backfill, 0);
    commit_effect(
        &root,
        &slot,
        seq,
        &key,
        &preconditions(EffectKind::Automatic),
    )
    .expect("baseline committed");
    slot.try_consume(seq, &key, 0);

    let (seq, key) = arm(&slot, record(&file, "a real turn", Some(500), 1), 0);
    let committed = commit_effect(
        &root,
        &slot,
        seq,
        &key,
        &preconditions(EffectKind::Automatic),
    )
    .expect("committed");
    assert!(committed.routable);
    assert_eq!(committed.budget_remaining, BUDGET_CAP - 1);
}

/// Grinch finding: the authoritative epoch is the one [`observe`] returns,
/// and `CapturedRecord.epoch` is the reader's advisory counter. After a
/// restart the two disagree, so a key built from the record's own field
/// would miss the recent set and the record would be acted on twice.
/// [`consumption_key`] is the only correct constructor and phase 7 must use
/// it.
#[test]
fn the_authoritative_epoch_is_the_persisted_one_not_the_records_field() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");

    // The room has already seen this file truncated once.
    observe(&root, &file, obs(100, b"head"));
    observe(&root, &file, obs(10, b"head"));
    assert_eq!(epoch_of(&root, &file), 1);

    // A reader that started after that restart stamps its own epoch 0.
    let mut fresh = record(&file, "turn", Some(20), 0);
    Arc::get_mut(&mut fresh).expect("sole owner").epoch = 0;
    Arc::get_mut(&mut fresh).expect("sole owner").observed_len = 20;
    Arc::get_mut(&mut fresh)
        .expect("sole owner")
        .observed_prefix = b"head".to_vec();

    let authoritative = consumption_key(&root, &fresh);
    assert_eq!(
        authoritative.epoch, 1,
        "the persisted epoch survives the reader that produced the record"
    );
    assert_ne!(
        authoritative.epoch, fresh.epoch,
        "the record's own field is advisory and disagrees after a restart"
    );

    // Acting through the authoritative key marks THAT key consumed.
    let slot = CaptureSlot::new();
    slot.offer(Arc::clone(&fresh), authoritative.epoch);
    let seq = slot.seq();
    commit_effect(
        &root,
        &slot,
        seq,
        &authoritative,
        &preconditions(EffectKind::Automatic),
    )
    .expect("committed");

    // The advisory key is a different key: proof that using it would let the
    // same record through a second time.
    let advisory = ConsumptionKey::from_record(&fresh, fresh.epoch);
    assert_ne!(advisory, authoritative);
    assert!(!is_actionable(&root, "session-a", &authoritative, Some(20)));
}

/// Grinch finding: a cut restored from `state.json` must not keep the
/// sequence tie-break of the reader that is gone. A fresh reader starts at
/// `reader_seq == 0`, so a surviving comparison would hide everything it
/// produces — exactly the bug section 7 paragraph 2 closes.
#[test]
fn a_cut_restored_from_disk_loses_its_sequence_tie_break() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");
    std::fs::write(&file, b"x").expect("write");
    let slot = CaptureSlot::new();

    register_cut(
        &root,
        &slot,
        Cut {
            path: crate::capture::key::normalise_path(&file),
            epoch: 0,
            len: 0,
            reader_seq: Some(42),
        },
    );
    assert_eq!(
        flush_room(&root),
        FlushOutcome::Written,
        "the cut has to reach disk for this test to mean anything"
    );
    assert_eq!(
        cut(&root).and_then(|c| c.reader_seq),
        Some(42),
        "the live cut keeps its tie-break within one reader lifetime"
    );

    // Restart.
    forget_room_for_tests(&root);
    let restored = cut(&root).expect("the cut is persisted");
    assert_eq!(restored.path, crate::capture::key::normalise_path(&file));
    assert_eq!(restored.len, 0);
    assert_eq!(
        restored.reader_seq, None,
        "the sequence tie-break must not survive the reader"
    );

    // And a fresh reader's first record is not hidden by it.
    let fresh = CaptureSlot::new();
    fresh.set_cut(restored);
    assert_eq!(
        fresh.offer(record(&file, "first after restart", Some(0), 0), 0),
        crate::capture::sink::OfferOutcome::Published
    );
}

/// Grinch finding: a decrement another instance made inside the same
/// recharge generation must survive reconciliation. The budget is merged
/// through the spend count, so neither side's work is discarded.
#[test]
fn a_second_instances_decrement_is_not_lost_by_reconciliation() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    let file = temp.path().join("a.jsonl");

    // This instance recharges, so generation 1 is current, and spends one.
    recharge_in_memory(&root);
    let slot = CaptureSlot::new();
    let (seq, key) = arm(&slot, record(&file, "mine", Some(10), 0), 0);
    commit_effect(
        &root,
        &slot,
        seq,
        &key,
        &preconditions(EffectKind::Automatic),
    )
    .expect("committed");
    slot.try_consume(seq, &key, 0);
    assert_eq!(budget(&root), BUDGET_CAP - 1);

    // A second instance, in the SAME generation, spends two more.
    let mut disk = read_persisted(&root);
    disk.spends_since_recharge = Some(3);
    disk.budget = Some(0);
    write_state_atomic(&root, &disk).expect("seed the other instance's file");

    // This instance reconciles: three spends in generation 1, not one.
    let (seq, key) = arm(&slot, record(&file, "fourth", Some(20), 1), 0);
    assert_eq!(
        commit_effect(
            &root,
            &slot,
            seq,
            &key,
            &preconditions(EffectKind::Automatic)
        ),
        Err(AbstainReason::BudgetExhausted),
        "the other instance's decrements must not vanish"
    );
    assert_eq!(budget(&root), 0);
}

/// The other direction, already covered by
/// `a_recharge_survives_a_state_file_written_by_another_instance`: a
/// superseded generation on disk does not undo this instance's recharge.
#[test]
fn a_superseded_generation_on_disk_does_not_undo_a_recharge() {
    let temp = tempfile::tempdir().expect("temp");
    let root = room(&temp);
    std::fs::create_dir_all(co_managed_dir(&root)).expect("dir");
    write_state_atomic(
        &root,
        &PersistedRoomState {
            budget: Some(0),
            spends_since_recharge: Some(3),
            recharge_stamp: 0,
            ..PersistedRoomState::default()
        },
    )
    .expect("seed");
    assert_eq!(budget(&root), 0);

    recharge_in_memory(&root);
    let mut room_snapshot = RoomState {
        recharge_stamp: 1,
        ..Default::default()
    };
    reconcile(&mut room_snapshot, read_persisted(&root));
    assert_eq!(room_snapshot.budget(), BUDGET_CAP);
}

/// Grinch finding: the reconstruction in [`head_from_lines`] is the only
/// permitted prefix source. This test names that rule and shows what
/// happens if it is broken.
#[test]
fn prefix_source_is_the_reconstruction_only() {
    let lines = vec![
        (0u64, "{\"a\":1}".to_string()),
        (8u64, "{\"b\":2}".to_string()),
    ];
    let reconstructed = head_from_lines(&lines, 0);
    assert_eq!(reconstructed, b"{\"a\":1}\n{\"b\":2}\n".to_vec());

    // A read that did not start at offset 0 carries no head evidence.
    assert!(head_from_lines(&lines, 4096).is_empty());

    // Self-consistent: the same lines reconstruct identically, so the
    // verdict is `Append` and the epoch holds.
    let stored = FileObservation::new(18, reconstructed.clone());
    let again = FileObservation::new(18, head_from_lines(&lines, 0));
    assert_eq!(
        classify_observation(&stored, &again),
        crate::capture::key::ObservationVerdict::Append
    );

    // The hazard, made executable: true file bytes with CRLF endings are a
    // DIFFERENT byte string, so mixing the two sources flips the verdict to
    // `Replaced`, advances the epoch spuriously and invalidates every
    // watermark for that path. This is why the reconstruction is pinned.
    let true_bytes = FileObservation::new(20, b"{\"a\":1}\r\n{\"b\":2}\r\n".to_vec());
    assert_eq!(
        classify_observation(&stored, &true_bytes),
        crate::capture::key::ObservationVerdict::Replaced,
        "mixing a true-bytes prefix with the reconstruction is forbidden"
    );
}
