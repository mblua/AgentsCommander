use super::*;

fn payload(seq: u64) -> WatcherMatchPayload {
    WatcherMatchPayload {
        session_id: "s".to_string(),
        seq,
        watcher_id: "w".to_string(),
        mode: WatcherMode::Occurrence,
        at: chrono::DateTime::parse_from_rfc3339("2026-07-30T00:00:00Z")
            .expect("fixed instant")
            .with_timezone(&chrono::Utc),
        captures: Vec::new(),
        row: format!("row {seq}"),
        row_truncated: false,
    }
}

fn history_with(id: Uuid, count: u64) -> WatcherHistory {
    let history = WatcherHistory::default();
    history.publish(id, SessionStatus::default());
    for seq in 1..=count {
        history.record(id, &[payload(seq)]);
    }
    history
}

/// 9.5.60 - reading is not draining. Closing and reopening the window must show the same
/// thing, which is the whole difference from the warning buffer this is modelled on.
#[test]
fn a_snapshot_does_not_consume() {
    let id = Uuid::new_v4();
    let history = history_with(id, 3);

    let first = history.snapshot(id, None);
    let second = history.snapshot(id, None);

    assert_eq!(first.matches.len(), 3);
    assert_eq!(
        first.matches.iter().map(|m| m.seq).collect::<Vec<_>>(),
        second.matches.iter().map(|m| m.seq).collect::<Vec<_>>()
    );
}

/// 9.5.61 - the 501st entry drops the oldest and says so. `truncated` is EXACT knowledge
/// that something was lost, which is why the window shows it separately from the weaker
/// "some screen output was not sampled" line.
#[test]
fn the_ring_drops_the_oldest_entry_and_marks_itself_truncated() {
    let id = Uuid::new_v4();
    let history = history_with(id, MAX_ENTRIES_PER_SESSION as u64);
    assert!(!history.snapshot(id, None).truncated);

    history.record(id, &[payload(MAX_ENTRIES_PER_SESSION as u64 + 1)]);

    let snapshot = history.snapshot(id, None);
    assert!(snapshot.truncated);
    assert_eq!(snapshot.matches.len(), MAX_ENTRIES_PER_SESSION);
    assert_eq!(
        snapshot.matches[0].seq, 2,
        "the oldest went, not the newest"
    );
    assert_eq!(
        snapshot.last_seq,
        MAX_ENTRIES_PER_SESSION as u64 + 1,
        "lastSeq counts what was INSERTED, not what is still held"
    );
}

/// 9.5.63 - `limit` trims from the new end and keeps the order, so the window can append.
#[test]
fn a_limit_returns_the_most_recent_entries_still_oldest_first() {
    let id = Uuid::new_v4();
    let history = history_with(id, 10);

    let limited = history.snapshot(id, Some(3));
    assert_eq!(
        limited.matches.iter().map(|m| m.seq).collect::<Vec<_>>(),
        vec![8, 9, 10]
    );

    assert_eq!(history.snapshot(id, None).matches.len(), 10);
    assert_eq!(
        history.snapshot(id, Some(1000)).matches.len(),
        10,
        "a limit past the ring is the ring"
    );
}

/// 9.5.64 - a session with no buffer is an EMPTY snapshot, never `None` and never an
/// error, and `warmedUp` is what tells the window it is looking at a session the engine
/// has not reached yet.
#[test]
fn a_session_with_no_buffer_returns_the_empty_snapshot_and_is_not_warmed_up() {
    let history = WatcherHistory::default();

    let snapshot = history.snapshot(Uuid::new_v4(), None);

    assert!(snapshot.matches.is_empty());
    assert_eq!(snapshot.last_seq, 0);
    assert!(!snapshot.truncated);
    assert_eq!(snapshot.possibly_missed_frames, 0);
    assert!(!snapshot.warmed_up);
    assert!(snapshot.active_watchers.is_empty());
}

/// 9.5.66 and 9.5.67 - the counters are there before any match, with their mode, and
/// `warmedUp` flips on the first published tick. That is the difference between "nothing
/// reaches this session" and "configured and waiting".
#[test]
fn the_counters_are_published_before_any_match_and_warm_the_session_up() {
    let id = Uuid::new_v4();
    let history = WatcherHistory::default();
    assert!(!history.snapshot(id, None).warmed_up);

    history.publish(
        id,
        SessionStatus {
            possibly_missed_frames: 0,
            active_watchers: vec![WatcherActivityCounter {
                watcher_id: "perm".to_string(),
                mode: WatcherMode::State,
                count: 0,
                degraded: false,
            }],
        },
    );

    let snapshot = history.snapshot(id, None);
    assert!(snapshot.warmed_up);
    assert!(snapshot.matches.is_empty());
    assert_eq!(snapshot.active_watchers.len(), 1);
    assert_eq!(snapshot.active_watchers[0].mode, WatcherMode::State);
    assert_eq!(snapshot.active_watchers[0].count, 0);
}

/// 9.5.70 - **the resurrection this ordering exists to prevent.** A batch that arrives for
/// a purged session creates nothing: only the engine, holding its registration lock,
/// creates entries.
#[test]
fn recording_into_a_purged_session_does_not_recreate_it() {
    let id = Uuid::new_v4();
    let history = history_with(id, 2);
    assert_eq!(history.tracked_sessions(), 1);

    history.purge(id);
    history.record(id, &[payload(3)]);

    assert_eq!(history.tracked_sessions(), 0);
    assert!(!history.snapshot(id, None).warmed_up);
}

/// 9.5.73 - the outer lock guards the map only, so a snapshot of one session cannot block
/// the engine writing another. Asserted by HOLDING one session's entry and reading the
/// other, which would deadlock under a single global mutex.
#[test]
fn a_snapshot_of_one_session_does_not_block_another() {
    let held = Uuid::new_v4();
    let other = Uuid::new_v4();
    let history = WatcherHistory::default();
    history.publish(held, SessionStatus::default());
    history.publish(other, SessionStatus::default());

    let entry = {
        let sessions = history.sessions.lock().unwrap();
        Arc::clone(&sessions[&held])
    };
    let _guard = entry.lock().unwrap();

    history.record(other, &[payload(1)]);
    assert_eq!(history.snapshot(other, None).matches.len(), 1);
}

/// Purging is idempotent, so every caller can just call it.
#[test]
fn purging_twice_is_harmless() {
    let id = Uuid::new_v4();
    let history = history_with(id, 1);
    history.purge(id);
    history.purge(id);
    assert_eq!(history.tracked_sessions(), 0);
}
