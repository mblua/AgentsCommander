use super::*;
use std::error::Error;

const SENTINEL: &str = "test.lock";

#[test]
fn issue_3294_lock_timing_positive_control() {
    let root = tempfile::tempdir().expect("root");
    let poll = Duration::from_millis(50);
    let timeout = Duration::from_millis(5000);
    let holder = acquire(root.path(), SENTINEL, poll, timeout).expect("holder");
    DELAYS.with_borrow_mut(|items| *items = Some(Vec::new()));
    let started = Instant::now();
    let failure = acquire(root.path(), SENTINEL, poll, timeout).expect_err("held lock");
    assert!(started.elapsed() >= timeout);
    assert_eq!(failure.step, LockStep::Acquire);
    assert!(matches!(
        failure.cause,
        LockCause::Timeout { milliseconds: 5000 }
    ));
    let delays = DELAYS.with_borrow_mut(|items| items.take().expect("observations"));
    assert!(delays.iter().filter(|(delay, _)| *delay == poll).count() > 1);
    for (delay, remaining) in &delays {
        assert_eq!(*delay, poll.min(*remaining));
    }
    let (last, remaining) = delays.last().expect("final delay");
    assert_eq!(*last, poll.min(*remaining));
    drop(holder);
    let reacquired = acquire(root.path(), SENTINEL, poll, timeout).expect("reacquire");
    assert!(reacquired.lock_path().is_file());
    assert_eq!(
        reacquired.canonical_ac_root(),
        crate::path_utils::normalize_windows_verbatim_path_buf(
            &std::fs::canonicalize(root.path()).expect("canonical root")
        )
    );
}

#[test]
fn issue_3294_lock_short_timeout_and_yield_positive_control() {
    let root = tempfile::tempdir().expect("root");
    let holder = acquire(root.path(), SENTINEL, Duration::ZERO, Duration::ZERO).unwrap();
    DELAYS.with_borrow_mut(|items| *items = Some(Vec::new()));
    let timeout = Duration::from_millis(10);
    let started = Instant::now();
    let failure = acquire(root.path(), SENTINEL, Duration::ZERO, timeout).unwrap_err();
    assert!(started.elapsed() >= timeout);
    assert!(matches!(
        failure.cause,
        LockCause::Timeout { milliseconds: 10 }
    ));
    let delays = DELAYS.with_borrow_mut(|items| items.take().unwrap());
    assert!(!delays.is_empty());
    assert!(delays.iter().all(|(delay, _)| delay.is_zero()));
    DELAYS.with_borrow_mut(|items| *items = Some(Vec::new()));
    let poll = Duration::from_millis(50);
    let failure = acquire(root.path(), SENTINEL, poll, timeout).unwrap_err();
    assert!(matches!(
        failure.cause,
        LockCause::Timeout { milliseconds: 10 }
    ));
    let delays = DELAYS.with_borrow_mut(|items| items.take().unwrap());
    assert!(!delays.is_empty());
    for (delay, remaining) in delays {
        assert_eq!(delay, poll.min(remaining));
        assert!(delay < poll);
    }
    drop(holder);
    acquire(root.path(), SENTINEL, Duration::ZERO, Duration::ZERO).unwrap();
}

#[test]
fn issue_3294_lock_failure_preserves_io_and_steps() {
    let root = tempfile::tempdir().expect("root");
    let missing = root.path().join("missing");
    let error = acquire(&missing, SENTINEL, Duration::ZERO, Duration::ZERO).unwrap_err();
    assert_eq!(error.step, LockStep::ResolveWorkspace);
    let LockCause::Io(io) = &error.cause else {
        panic!("I/O cause")
    };
    assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
    assert!(io.raw_os_error().is_some());
    assert!(error.source().is_some());
    // Semantic failures retain their distinct operation without fabricating an I/O cause.
    std::fs::create_dir(root.path().join(SENTINEL)).unwrap();
    let error = acquire(root.path(), SENTINEL, Duration::ZERO, Duration::ZERO).unwrap_err();
    assert_eq!(error.step, LockStep::InspectSentinel);
    assert!(matches!(error.cause, LockCause::NonRegularOrLink));
    std::fs::remove_dir(root.path().join(SENTINEL)).unwrap();
    let holder = acquire(root.path(), SENTINEL, Duration::ZERO, Duration::ZERO).unwrap();
    let error = holder.require_workspace(&missing).unwrap_err();
    assert_eq!(error.step, LockStep::VerifyWorkspace);
    let LockCause::Io(io) = &error.cause else {
        panic!("I/O cause")
    };
    assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
    assert!(io.raw_os_error().is_some());
    // Check the original OS error also remains downcastable in the source chain.
    let source = error.source().unwrap();
    assert!(source.downcast_ref::<LockCause>().is_some());
    let raw = std::io::Error::from_raw_os_error(5);
    let failure = LockFailure::new(
        LockStep::OpenExisting,
        holder.lock_path(),
        LockCause::Io(raw),
    );
    let LockCause::Io(io) = &failure.cause else {
        panic!("I/O cause")
    };
    assert_eq!(io.raw_os_error(), Some(5));
    let source = failure.source().unwrap().source().unwrap();
    let source_io = source
        .downcast_ref::<std::io::Error>()
        .expect("original I/O source");
    assert_eq!(source_io.raw_os_error(), Some(5));
    assert_eq!(io.kind(), std::io::Error::from_raw_os_error(5).kind());
}
