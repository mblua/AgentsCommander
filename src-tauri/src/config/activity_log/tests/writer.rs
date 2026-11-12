use super::{build_busy, json_of, lines_of, parsed_lines, sample_session};
use crate::config::activity_log::{
    append, append_at, append_batch, build_app_alive, init_run, parse_record, run_id, sink_path,
    writer_lock, ActivityCoalescer, BusyReason, FILE_NAME,
};
use std::sync::{Arc, TryLockError};
use std::time::Duration;
use tempfile::TempDir;
use uuid::Uuid;

#[test]
fn a_poisoned_writer_mutex_does_not_panic_a_later_append() {
    let c = Arc::new(ActivityCoalescer::new());
    let poison_owner = Arc::clone(&c);
    let handle = std::thread::spawn(move || {
        let _guard = writer_lock(&poison_owner)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

#[test]
fn owner_clones_share_sink_with_preinit_positive_control() {
    let owner = std::sync::Arc::new(ActivityCoalescer::new());
    let clone = std::sync::Arc::clone(&owner);
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join(FILE_NAME);
    assert!(owner.sink.get().is_none());
    assert!(sink_path(&clone).is_none());
    assert!(run_id(&owner).is_empty());
    let before = build_app_alive(vec![], &owner);
    let before_json = json_of(&before);
    let (tx, rx) = std::sync::mpsc::channel();
    let worker_owner = std::sync::Arc::clone(&clone);
    let worker_record = before.clone();
    let guard = owner.writer.try_lock().expect("writer available");
    let worker = std::thread::spawn(move || {
        append(worker_record.clone(), &worker_owner);
        append_batch(&[worker_record], &worker_owner);
        tx.send(()).expect("report pre-init completion");
    });
    let completed = rx.recv_timeout(Duration::from_secs(5));
    drop(guard);
    completed.expect("pre-init appends must not acquire writer");
    worker.join().expect("pre-init worker");
    assert!(owner.sink.get().is_none());
    assert!(clone.sink.get().is_none());
    assert!(!path.exists());
    init_run(dir.path(), "first", true, &clone);
    assert_eq!(sink_path(&owner), Some(path.clone()));
    assert_eq!(sink_path(&clone), Some(path.clone()));
    assert_eq!(run_id(&owner), "first");
    assert_eq!(run_id(&clone), "first");
    append(before, &clone);
    let after = build_app_alive(vec![Uuid::new_v4()], &owner);
    let after_json = json_of(&after);
    append_batch(&[after], &clone);
    let lines = parsed_lines(&path);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["event"], "app_start");
    assert_eq!(lines[0]["runId"], "first");
    assert_eq!(lines[1], before_json);
    assert_eq!(lines[1]["runId"], "");
    assert_eq!(lines[2], after_json);
    assert_eq!(lines[2]["runId"], "first");
}

#[test]
fn repeated_init_preserves_first_sink_and_runs_startup_work() {
    let owner = std::sync::Arc::new(ActivityCoalescer::new());
    let clone = std::sync::Arc::clone(&owner);
    let other = ActivityCoalescer::new();
    let a = TempDir::new().expect("temp dir A");
    let b = TempDir::new().expect("temp dir B");
    init_run(a.path(), "first", true, &owner);
    init_run(b.path(), "second", true, &clone);
    init_run(b.path(), "disabled", false, &clone);
    assert_eq!(sink_path(&clone), Some(a.path().join(FILE_NAME)));
    assert_eq!(run_id(&owner), "first");
    assert_eq!(run_id(&clone), "first");
    assert!(sink_path(&other).is_none());
    assert!(run_id(&other).is_empty());
    let record = build_app_alive(vec![], &clone);
    let expected = json_of(&record);
    append(record, &clone);
    let a_lines = parsed_lines(&a.path().join(FILE_NAME));
    let b_lines = parsed_lines(&b.path().join(FILE_NAME));
    assert_eq!(a_lines.len(), 2);
    assert_eq!(a_lines[0]["event"], "app_start");
    assert_eq!(a_lines[0]["runId"], "first");
    assert_eq!(a_lines[1], expected);
    assert_eq!(a_lines[1]["runId"], "first");
    assert_eq!(b_lines.len(), 1);
    assert_eq!(b_lines[0]["event"], "app_start");
    assert_eq!(b_lines[0]["runId"], "second");
}
