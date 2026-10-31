use super::*;

fn frame(text: &str) -> Vec<String> {
    vec![text.to_string()]
}

fn samples(baseline: Option<&[String]>) -> (FailureSamples, Instant) {
    let mut state = FailureSamples::default();
    let entered = Instant::now();
    let _ = state.baseline(baseline);
    state.first_enter = Some(entered);
    (state, entered)
}

#[test]
fn issue_3371_late_failure_and_exact_reason() {
    for second in [31, 34, 45, 48] {
        let (mut state, entered) = samples(Some(&[]));
        assert_eq!(
            state.sample_at(entered + Duration::from_secs(30), Some(&[])),
            None
        );
        let outcome = state.sample_at(
            entered + Duration::from_secs(second),
            Some(&frame(FAILURE_PREFIX)),
        );
        assert_eq!(outcome, Some(ClearWatchOutcome::KnownFailure));
        assert_eq!(
            outcome.unwrap().into_result(),
            Err("codex_clear_failed: provider reported fresh-session startup failure".into())
        );
    }
}

#[test]
fn issue_3371_final_sample_required_and_empty_viewport_valid() {
    let (mut state, entered) = samples(Some(&[]));
    assert_eq!(state.duration, Duration::from_secs(45));
    assert_eq!(SAMPLE_INTERVAL, Duration::from_millis(250));
    assert_eq!(
        state.sample_at(entered + Duration::from_millis(44_999), Some(&[])),
        None
    );
    assert_eq!(
        state.outcome, None,
        "elapsed time alone cannot complete observation"
    );
    assert_eq!(
        state.sample_at(entered + Duration::from_secs(45), Some(&[])),
        Some(ClearWatchOutcome::CompleteNoNewFailure)
    );
}

#[test]
fn issue_3371_baseline_error_requires_observed_absence() {
    let error = frame(FAILURE_PREFIX);
    for disappears in [false, true] {
        let (mut state, entered) = samples(Some(&error));
        assert_eq!(
            state.sample_at(entered + Duration::from_secs(1), Some(&error)),
            None
        );
        if disappears {
            assert_eq!(
                state.sample_at(entered + Duration::from_secs(2), Some(&[])),
                None
            );
        }
        assert_eq!(
            state.sample_at(entered + Duration::from_secs(45), Some(&error)),
            Some(if disappears {
                ClearWatchOutcome::KnownFailure
            } else {
                ClearWatchOutcome::CompleteNoNewFailure
            })
        );
    }
    let (mut state, entered) = samples(Some(&error));
    assert_eq!(
        state.sample_at(entered + Duration::from_secs(1), Some(&[])),
        None
    );
    assert_eq!(
        state.sample_at(entered + Duration::from_secs(45), Some(&[])),
        Some(ClearWatchOutcome::CompleteNoNewFailure)
    );
}

#[test]
fn issue_3371_whitespace_word_wrap_and_optional_square_bullet() {
    for width in 1..=FAILURE_PREFIX.len() {
        let chars: Vec<char> = FAILURE_PREFIX.chars().collect();
        let mut rows: Vec<String> = chars
            .chunks(width)
            .map(|chunk| chunk.iter().collect())
            .collect();
        rows[0].insert_str(0, "  ■\u{2003}");
        assert_eq!(
            FailureSamples::failure_present(Some(&rows)),
            Some(true),
            "width {width}"
        );
    }
    for text in [
        "MCP timeout error",
        "failed to start a fresh session through the app server:",
        "Failed to start a fresh session through another server:",
    ] {
        assert_eq!(
            FailureSamples::failure_present(Some(&frame(text))),
            Some(false)
        );
    }
}

#[test]
fn issue_3371_unavailable_baseline_intermediate_or_final_is_execution_error() {
    let (mut baseline, entered) = samples(None);
    assert_eq!(
        baseline.sample_at(entered, Some(&[])),
        Some(ClearWatchOutcome::ExecutionError)
    );
    for second in [1, 45, 48] {
        let (mut state, entered) = samples(Some(&[]));
        assert_eq!(
            state.sample_at(entered + Duration::from_secs(second), None),
            Some(ClearWatchOutcome::ExecutionError)
        );
        assert_eq!(
            state.sample_at(entered + Duration::from_secs(50), Some(&[])),
            Some(ClearWatchOutcome::ExecutionError)
        );
    }
}

#[test]
fn issue_3371_oversized_unusable_capture_and_missing_enter_never_succeed() {
    for rows in [
        vec![String::new(); usize::from(terminal_snapshot_renderer::MAX_ROWS) + 1],
        frame(&"x".repeat(terminal_snapshot_renderer::MAX_CELLS * 4 + 1)),
        frame("\u{001b}[31m"),
    ] {
        let (mut state, entered) = samples(Some(&[]));
        assert_eq!(
            state.sample_at(entered + WATCH_DURATION, Some(&rows)),
            Some(ClearWatchOutcome::ExecutionError)
        );
    }
    let mut state = FailureSamples::default();
    state.baseline(Some(&[])).unwrap();
    assert_eq!(
        state.sample_at(Instant::now() + WATCH_DURATION, Some(&[])),
        Some(ClearWatchOutcome::ExecutionError)
    );
}

#[test]
fn issue_3371_unsubmitted_watch_drops_observation_callback() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    for submitted in [false, true] {
        let mut state = FailureSamples::default();
        if submitted {
            state.first_enter = Some(Instant::now());
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let callback_calls = Arc::clone(&calls);
        state.on_observation(Some(Box::new(move || {
            callback_calls.fetch_add(1, Ordering::SeqCst);
        })));
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(submitted));
        assert_eq!(
            Arc::strong_count(&calls),
            1,
            "callback owner was not released"
        );
    }
}
