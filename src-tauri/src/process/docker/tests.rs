use super::*;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[test]
fn cloned_state_shares_retained_readers_and_retry_exclusion() {
    let state = DockerProcessState::default();
    let clone = state.clone();
    let readers = state.retain_blocked_command_readers_for_test(
        "owner-test",
        "docker-fixture",
        "readers blocked",
    );
    assert_eq!(clone.active_reader_count(), 2);
    let (entered, release_retry) = state.install_command_retry_gate_for_test();
    let worker = clone.clone();
    let retry = std::thread::spawn(move || {
        worker.retry_until(Instant::now() + Duration::from_secs(5));
    });
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let contexts = clone.retained_contexts();
    assert_eq!(contexts.len(), 1);
    let context = &contexts[0];
    assert_eq!(context.session_id, None);
    assert_eq!(context.reason, "owner-test");
    assert_eq!(context.program, "docker-fixture");
    assert_eq!(context.state, "inFlight");
    assert!(context.in_flight);
    assert_eq!(context.last_error.as_deref(), Some("readers blocked"));
    clone.retry_until(Instant::now() + Duration::from_millis(20));
    assert_eq!(clone.active_reader_count(), 2);
    readers.release();
    release_retry.send(()).unwrap();
    retry.join().unwrap();
    assert!(state.retained_contexts().is_empty());
    assert_eq!(state.active_reader_count(), 0);
}

#[test]
fn cloned_state_shares_script_order_and_recording() {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let state = DockerProcessState::with_scripted_commands(
        Arc::clone(&recorded),
        vec![
            ScriptedDockerCommandResult::SpawnedError("first".into()),
            ScriptedDockerCommandResult::ReaderError("second".into()),
        ],
    );
    let clone = state.clone();
    assert!(Arc::ptr_eq(
        state.recorded_commands.as_ref().unwrap(),
        clone.recorded_commands.as_ref().unwrap(),
    ));
    let first = state
        .scripted_results
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .pop_front();
    let second = clone
        .scripted_results
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .pop_front();
    assert!(matches!(first, Some(ScriptedDockerCommandResult::SpawnedError(e)) if e == "first"));
    assert!(matches!(second, Some(ScriptedDockerCommandResult::ReaderError(e)) if e == "second"));
    assert!(state
        .scripted_results
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .is_empty());
}
