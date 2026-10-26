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

struct CommandControl {
    canceled: bool,
    deadline: Option<Instant>,
}

impl DockerCommandControl for CommandControl {
    fn shutdown_requested(&self) -> bool {
        self.canceled
    }

    fn shutdown_deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

#[test]
fn owner_rejects_cancellation_and_expired_deadline_before_spawn() {
    let state = DockerProcessState::default();
    for control in [
        CommandControl {
            canceled: true,
            deadline: None,
        },
        CommandControl {
            canceled: false,
            deadline: Some(Instant::now()),
        },
    ] {
        let error = state
            .execute(
                DockerCommandSpec {
                    program: "nonexistent-docker-owner-fixture".into(),
                    args: Vec::new(),
                    secret_env: Default::default(),
                    redacted_values: Vec::new(),
                },
                &control,
                DockerCommandShutdownBehavior::CancelOnShutdown,
                None,
                "owner-cancellation",
                Instant::now() + Duration::from_secs(2),
            )
            .expect_err("control must reject before spawning");
        assert!(!error.spawned);
        let message = error.source.to_string();
        if control.canceled {
            assert!(message.contains("canceled by shutdown before spawn"));
        } else {
            assert!(message.contains("skipped after shutdown deadline"));
        }
        assert_eq!(state.active_child_count(), 0);
        assert_eq!(state.active_reader_count(), 0);
        assert!(state.retained_contexts().is_empty());
    }
}

#[test]
fn owner_execution_preserves_scripted_spawned_failures_and_shared_order() {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let state = DockerProcessState::with_scripted_commands(
        Arc::clone(&recorded),
        vec![
            ScriptedDockerCommandResult::SpawnedError("first".into()),
            ScriptedDockerCommandResult::ReaderError("second".into()),
        ],
    );
    let clone = state.clone();
    let control = CommandControl {
        canceled: false,
        deadline: None,
    };
    for (owner, expected) in [(&state, "first"), (&clone, "second")] {
        let error = owner
            .execute(
                DockerCommandSpec {
                    program: expected.into(),
                    args: Vec::new(),
                    secret_env: Default::default(),
                    redacted_values: Vec::new(),
                },
                &control,
                DockerCommandShutdownBehavior::ContinueUntilDeadline,
                None,
                "owner-script",
                Instant::now() + Duration::from_secs(2),
            )
            .expect_err("scripted failure");
        assert!(error.spawned);
        assert!(error.source.to_string().contains(expected));
    }
    let commands = recorded.lock().unwrap();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].program, "first");
    assert_eq!(commands[1].program, "second");
}

#[test]
fn owner_retry_reaps_panicked_reader_without_losing_context_or_ownership() {
    let state = DockerProcessState::default();
    let (entered, release) = state.install_command_retry_gate_for_test();
    let owner = DockerCommandOwner {
        child: None,
        stdout: Some(std::thread::spawn(|| panic!("reader fixture panic"))),
        stderr: Some(std::thread::spawn(|| Ok(CappedCommandStream::default()))),
        session_id: Some(uuid::Uuid::new_v4()),
        reason: "reader-panic",
        program: "docker-fixture".into(),
        active_child: None,
    };
    let session_id = owner.session_id;
    state.retain(owner, Some("reader pending".into()));
    let worker = state.clone();
    let retry = std::thread::spawn(move || {
        worker.retry_until(Instant::now() + Duration::from_secs(5));
    });
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    let contexts = state.retained_contexts();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].session_id, session_id);
    assert_eq!(contexts[0].reason, "reader-panic");
    assert_eq!(contexts[0].program, "docker-fixture");
    assert!(contexts[0].in_flight);
    release.send(()).unwrap();
    retry.join().expect("reader panic must not escape retry");
    assert!(state.retained_contexts().is_empty());
}
