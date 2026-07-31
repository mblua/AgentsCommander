//! #1171 - the per-session in-RAM ring the activity window reads, and the per-session status
//! the engine publishes beside it.
//!
//! Mould: `SessionWarningBuffer` (`session/warnings.rs:39-51`), with four deliberate
//! differences, each because this buffer is bigger, hotter and read rather than drained.
//!
//! **Nothing here is persisted.** The window never claims history across restarts, and the
//! engine's worst-case disk write is zero - a property worth keeping.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use super::{WatcherMatchPayload, WatcherMode};

/// Hard cap per session, oldest dropped first.
///
/// With `row` capped at 256 bytes the worst-case element is about 500 to 550 bytes, so twenty
/// sessions is about 3.5 MB typical and about 5.5 MB at the ceiling.
pub const MAX_ENTRIES_PER_SESSION: usize = 500;

/// One watcher's standing on one session, for the window's empty states and its degraded
/// marker. Present even when the count is 0, which is what lets the window say "configured and
/// waiting" instead of "nothing reaches this session".
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherActivityCounter {
    pub watcher_id: String,
    pub mode: WatcherMode,
    pub count: u64,
    /// True while this watcher is hitting a per-tick cap or is suspended.
    pub degraded: bool,
}

/// Everything `get_watcher_activity` answers with, in one synchronous read.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatcherActivitySnapshot {
    /// Oldest first. `limit` trims from the NEW end: `Some(n)` returns the n most recent, still
    /// ordered oldest to newest.
    pub matches: Vec<WatcherMatchPayload>,
    /// The highest `seq` ever inserted for this session. The merge fence for a window that
    /// subscribed before it fetched.
    pub last_seq: u64,
    /// The ring dropped at least one entry since the session started.
    pub truncated: bool,
    /// Monotonic since the session started. NOT a count of lost matches.
    pub possibly_missed_frames: u64,
    /// False until the engine has ticked this session at least once.
    ///
    /// Without it the window cannot tell "no watcher reaches this agent" from "the engine has
    /// not run yet", and would show the "Configure watchers" empty state for the first 200 ms
    /// of every session even when watchers ARE configured.
    pub warmed_up: bool,
    /// The watchers that reach this session's agent right now, with their count since the
    /// session started.
    pub active_watchers: Vec<WatcherActivityCounter>,
}

impl WatcherActivitySnapshot {
    /// What a session with no buffer answers with: an empty snapshot, never `None` and never
    /// an error. The window distinguishes its states from the VALUES here, with no nullability
    /// to reason about.
    pub fn empty() -> Self {
        Self {
            matches: Vec::new(),
            last_seq: 0,
            truncated: false,
            possibly_missed_frames: 0,
            warmed_up: false,
            active_watchers: Vec::new(),
        }
    }
}

/// What the engine publishes at the end of each tick, so the snapshot command can be
/// synchronous and take one per-session mutex instead of resolving settings and the session
/// manager itself.
#[derive(Clone, Debug, Default)]
pub struct SessionStatus {
    pub possibly_missed_frames: u64,
    pub active_watchers: Vec<WatcherActivityCounter>,
}

#[derive(Default)]
struct SessionHistory {
    matches: VecDeque<WatcherMatchPayload>,
    last_seq: u64,
    truncated: bool,
    status: SessionStatus,
}

/// The per-session rings, behind per-session locks.
///
/// The outer lock guards the MAP only and is released before any read or write of a session's
/// entry, so a snapshot never blocks the engine's writes to other sessions. The engine takes
/// this structure up to 250 times a second at fifty sessions; a single mutex would serialize
/// every snapshot against every write.
#[derive(Default)]
pub struct WatcherHistory {
    sessions: Mutex<HashMap<Uuid, Arc<Mutex<SessionHistory>>>>,
}

pub type WatcherHistoryState = Arc<WatcherHistory>;

impl WatcherHistory {
    /// Create or refresh a session's entry with what the engine knows at the end of its tick.
    ///
    /// **This is the only path that CREATES an entry**, and the engine calls it while still
    /// holding its registration lock. Together with `purge_session_side_state` retiring before
    /// it purges, that is what stops a tick in flight from resurrecting a session that was
    /// destroyed moments earlier.
    pub fn publish(&self, id: Uuid, status: SessionStatus) {
        let entry = {
            let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(sessions.entry(id).or_default())
        };
        let mut history = entry.lock().unwrap_or_else(|e| e.into_inner());
        history.status = status;
    }

    /// Append a tick's matches to an EXISTING entry.
    ///
    /// Creates nothing. A batch that arrives for a session purged between the tick and the
    /// emit is dropped, which is exactly what should happen to it.
    pub fn record(&self, id: Uuid, matches: &[WatcherMatchPayload]) {
        let Some(entry) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .map(Arc::clone)
        else {
            return;
        };
        let mut history = entry.lock().unwrap_or_else(|e| e.into_inner());
        for payload in matches {
            history.last_seq = history.last_seq.max(payload.seq);
            history.matches.push_back(payload.clone());
            // `VecDeque::pop_front` and not `Vec::remove(0)`: at the 32-entry cap of the
            // warning buffer that difference is irrelevant, at 500 under a burst it is not.
            while history.matches.len() > MAX_ENTRIES_PER_SESSION {
                history.matches.pop_front();
                history.truncated = true;
            }
        }
    }

    /// Read a session's activity WITHOUT consuming it.
    ///
    /// `SessionWarningBuffer::drain` consumes (`warnings.rs:53-63`), which here would mean
    /// closing and reopening the window shows nothing.
    pub fn snapshot(&self, id: Uuid, limit: Option<usize>) -> WatcherActivitySnapshot {
        let Some(entry) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .map(Arc::clone)
        else {
            return WatcherActivitySnapshot::empty();
        };
        let history = entry.lock().unwrap_or_else(|e| e.into_inner());

        let matches: Vec<WatcherMatchPayload> = match limit {
            // Trimmed from the NEW end and still ordered oldest to newest, so the window can
            // append to what it has instead of re-sorting.
            Some(limit) => history
                .matches
                .iter()
                .skip(history.matches.len().saturating_sub(limit))
                .cloned()
                .collect(),
            None => history.matches.iter().cloned().collect(),
        };

        WatcherActivitySnapshot {
            matches,
            last_seq: history.last_seq,
            truncated: history.truncated,
            possibly_missed_frames: history.status.possibly_missed_frames,
            // The entry exists at all only because the engine ticked this session.
            warmed_up: true,
            active_watchers: history.status.active_watchers.clone(),
        }
    }

    /// Drop a session's history. Idempotent.
    pub fn purge(&self, id: Uuid) {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
    }

    #[cfg(test)]
    pub(crate) fn tracked_sessions(&self) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

#[cfg(test)]
mod tests {
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
}
