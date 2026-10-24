use super::*;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

#[test]
fn ok_when_work_finishes_in_budget() {
    assert_eq!(run_time_boxed(Duration::from_secs(5), || 42), Ok(42));
}

#[test]
fn timeout_when_work_exceeds_budget() {
    let r = run_time_boxed(Duration::from_millis(50), || {
        std::thread::sleep(Duration::from_millis(500));
        42
    });
    assert_eq!(r, Err(RecvTimeoutError::Timeout));
}

#[test]
fn disconnected_when_closure_panics() {
    let r: Result<i32, _> = run_time_boxed(Duration::from_secs(5), || panic!("boom"));
    assert_eq!(r, Err(RecvTimeoutError::Disconnected));
}
