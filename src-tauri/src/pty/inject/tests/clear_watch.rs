use super::{clear_watch_concurrency, run_clear_watch, WatchCase};

const KNOWN: &str = "codex_clear_failed: provider reported fresh-session startup failure";
const INCOMPLETE: &str = "codex_clear_execution_error: observation incomplete";

#[tokio::test]
async fn issue_3371_real_injector_known_failure_before_success_tail() {
    for case in [WatchCase::FailureT1, WatchCase::FailureAfterT2] {
        let run = run_clear_watch(case).await;
        assert_eq!(run.result, Err(KNOWN.into()));
        if matches!(case, WatchCase::FailureT1) {
            assert!(!run.armed);
            assert_eq!(run.callbacks, 0);
            assert_eq!(run.writes, vec![b"/clear".to_vec(), vec![13]]);
        } else {
            assert!(
                run.armed,
                "submission stays armed after later observed failure"
            );
            assert_eq!(run.callbacks, 1);
        }
    }
}

#[tokio::test]
async fn issue_3371_incomplete_observation_never_passes_positive_control() {
    for case in [
        WatchCase::BaselineUnavailable,
        WatchCase::IntermediateUnavailable,
        WatchCase::FinalUnavailable,
    ] {
        let run = run_clear_watch(case).await;
        assert_eq!(run.result, Err(INCOMPLETE.into()));
        assert_ne!(run.result, Err(KNOWN.into()));
    }
}

#[tokio::test]
async fn issue_3371_required_and_redundant_enter_faults() {
    let required = run_clear_watch(WatchCase::RequiredEnterFault).await;
    assert!(required
        .result
        .unwrap_err()
        .starts_with("PTY Enter (1/2) write failed:"));
    assert!(!required.armed);
    assert_eq!(required.callbacks, 0);
    assert_eq!(required.writes, vec![b"/clear".to_vec(), vec![13]]);
    let redundant = run_clear_watch(WatchCase::RedundantEnterFault).await;
    redundant.result.unwrap();
    assert!(redundant.armed);
    assert_eq!(redundant.callbacks, 1);
    assert_eq!(
        redundant.writes,
        vec![b"/clear".to_vec(), vec![13], vec![13]]
    );
}

#[tokio::test]
async fn issue_3371_healthy_watch_and_legacy_facade_bytes() {
    for case in [WatchCase::Healthy, WatchCase::Legacy] {
        let run = run_clear_watch(case).await;
        run.result.unwrap();
        assert!(run.armed);
        assert_eq!(
            run.callbacks,
            usize::from(matches!(case, WatchCase::Healthy))
        );
        assert_eq!(run.writes, vec![b"/clear".to_vec(), vec![13], vec![13]]);
    }
    for case in [WatchCase::WrongProvider, WatchCase::WrongCommand] {
        let run = run_clear_watch(case).await;
        assert_eq!(run.result, Err(INCOMPLETE.into()));
        assert_eq!(run.callbacks, 0);
        assert!(run.writes.is_empty());
        assert!(!run.armed);
    }
}

#[tokio::test]
async fn issue_3371_writers_and_other_session_progress_before_watch_barrier() {
    clear_watch_concurrency(false, false, false).await.unwrap();
}

#[tokio::test]
async fn issue_3371_route_removal_poison_and_cancellation_during_watch() {
    for (remove, poison) in [(true, false), (false, true)] {
        assert_eq!(
            clear_watch_concurrency(remove, poison, false).await,
            Err(INCOMPLETE.into())
        );
    }
    assert_eq!(
        clear_watch_concurrency(false, false, true).await,
        Err("cancelled".into())
    );
}

#[test]
fn issue_3371_real_writer_caller_paths_share_admission_and_route_guard() {
    let desktop = include_str!("../../../commands/pty.rs");
    let web = include_str!("../../../web/commands.rs");
    let privileged = include_str!("../../../phone/mailbox/pty_dispatch.rs");
    let close = include_str!("../../../phone/mailbox/teardown.rs");
    for source in [desktop, web, close] {
        assert!(source.contains("PtyManager::acquire_input_writer"));
        assert!(source.contains("PtyManager::write_with_permit"));
    }
    assert!(privileged.contains("write_exact_agent_input_first("));
    let manager = include_str!("../../manager.rs");
    let routes = include_str!("../../input_route.rs");
    let forwarding = include_str!("../../manager/route_proof.rs");
    assert!(manager.contains("pub use route_proof::{acquire_input_writer"));
    assert!(forwarding.contains("crate::pty::input_route::write_with_permit"));
    assert!(privileged.contains("PtyManager::acquire_input_writer"));
    assert!(privileged.contains("submit_exact_agent_input_with_permit"));
    assert!(web.contains("PtyManager::write_with_permit"));
    assert!(routes.contains("lock_route_for_write(permit)?.write(bytes)"));
    assert!(routes.contains("reserve_writer(session_id, generation)"));
    assert!(routes.contains("permit._response_writer.is_closing()"));
    assert!(close.contains("command.as_bytes()"));
    assert!(close.contains("tokio::time::timeout"));
}

#[tokio::test]
async fn issue_3371_sampling_ignores_lifecycle_and_rejects_changed_capture_generation() {
    assert!(super::capture_watch_route(false, false, false).await);
    assert!(!super::capture_watch_route(true, false, false).await);
    assert!(!super::capture_watch_route(false, true, false).await);
    assert!(!super::capture_watch_route(false, false, true).await);
}
