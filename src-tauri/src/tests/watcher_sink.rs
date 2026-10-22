use super::*;
use crate::pty::watchers::WatcherMatchBatch;
use std::sync::Mutex as StdMutex;

use crate::pty::watchers::history::{SessionStatus, WatcherHistory};
use crate::pty::watchers::{WatcherMatchPayload, WatcherMode};

type Recorded = (
    WatcherSink,
    Arc<StdMutex<Vec<WatcherMatchBatch>>>,
    crate::pty::watchers::history::WatcherHistoryState,
);

fn recording(present: bool) -> Recorded {
    let delivered = Arc::new(StdMutex::new(Vec::new()));
    let sink_delivered = Arc::clone(&delivered);
    let history: crate::pty::watchers::history::WatcherHistoryState =
        Arc::new(WatcherHistory::default());
    (
        WatcherSink {
            history: Arc::clone(&history),
            delivery: WatcherDelivery {
                window_present: Arc::new(move || present),
                emit: Arc::new(move |batch| sink_delivered.lock().unwrap().push(batch)),
            },
        },
        delivered,
        history,
    )
}

fn batch(session_id: uuid::Uuid) -> WatcherMatchBatch {
    WatcherMatchBatch {
        session_id: session_id.to_string(),
        matches: vec![WatcherMatchPayload {
            session_id: session_id.to_string(),
            seq: 1,
            watcher_id: "w".to_string(),
            mode: WatcherMode::Occurrence,
            at: chrono::Utc::now(),
            captures: Vec::new(),
            row: "hit".to_string(),
            row_truncated: false,
        }],
    }
}

/// #1171, 9.3.36 - with the activity window closed, NOTHING is emitted - not a broadcast
/// nobody reads, not an emit to a label that does not exist - **and the ring still
/// records**, so opening the window later shows the history.
#[test]
fn no_event_is_emitted_when_the_window_is_closed_and_the_ring_still_records() {
    let id = uuid::Uuid::new_v4();
    let (sink, delivered, history) = recording(false);
    history.publish(id, SessionStatus::default());

    sink.emit(batch(id));

    assert!(delivered.lock().unwrap().is_empty());
    assert_eq!(history.snapshot(id, None).matches.len(), 1);
}

#[test]
fn the_batch_is_delivered_and_recorded_when_the_window_is_open() {
    let id = uuid::Uuid::new_v4();
    let (sink, delivered, history) = recording(true);
    history.publish(id, SessionStatus::default());

    sink.emit(batch(id));

    assert_eq!(delivered.lock().unwrap().len(), 1);
    assert_eq!(history.snapshot(id, None).matches.len(), 1);
}
