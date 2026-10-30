use super::{lines_of, sample_session};
use crate::config::activity_log::{
    append_at, build_busy, parse_record, writer_lock, ActivityCoalescer, BusyReason, FILE_NAME,
};
use std::sync::{Arc, TryLockError};
use tempfile::TempDir;
use uuid::Uuid;

#[test]
fn a_poisoned_writer_mutex_does_not_panic_a_later_append() {
    let c = Arc::new(ActivityCoalescer::new());
    let poison_owner = Arc::clone(&c);
    let handle = std::thread::spawn(move || {
        let _guard = writer_lock(&poison_owner)
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        panic!("poison the writer mutex on purpose");
    });
    assert!(
        handle.join().is_err(),
        "the helper thread must have panicked"
    );

    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(FILE_NAME);
    let s = sample_session(Uuid::new_v4(), None);
    append_at(&path, &[build_busy(s.id, &s, BusyReason::MarkBusy, &c)], &c);
    let lines = lines_of(&path);
    assert_eq!(lines.len(), 1);
    assert!(parse_record(&lines[0]).is_some());
}

#[test]
fn concurrent_appends_never_interleave_a_line() {
    let c = Arc::new(ActivityCoalescer::new());
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(FILE_NAME);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let path = path.as_path();
            let c = Arc::clone(&c);
            scope.spawn(move || {
                for _ in 0..100 {
                    let s = sample_session(Uuid::new_v4(), None);
                    append_at(path, &[build_busy(s.id, &s, BusyReason::MarkBusy, &c)], &c);
                }
            });
        }
    });
    let lines = lines_of(&path);
    assert_eq!(lines.len(), 800);
    for line in lines {
        assert!(
            parse_record(&line).is_some(),
            "a line was torn by a concurrent append: {line}"
        );
    }
}

#[test]
fn writer_identity_is_shared_only_by_the_same_owner() {
    let a = Arc::new(ActivityCoalescer::new());
    let shared = Arc::clone(&a);
    let b = ActivityCoalescer::new();
    let _guard = writer_lock(&a).lock().unwrap();
    assert!(matches!(
        writer_lock(&shared).try_lock(),
        Err(TryLockError::WouldBlock)
    ));
    assert!(writer_lock(&b).try_lock().is_ok());
}
