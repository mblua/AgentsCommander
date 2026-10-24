use super::*;

// Each test pins one row of the 5.2 table byte for byte. Counter values are
// distinct on purpose so a swapped argument changes the sentence.

#[test]
fn row_a_found_with_a_complete_scan_has_no_message() {
    assert_eq!(status_for(1, 10, 0, 0), (STATUS_FOUND, None));
}

#[test]
fn row_b_found_with_skipped_handles_stays_found_and_says_others_may_exist() {
    let (status, message) = status_for(2, 3113, 192, 1190);
    assert_eq!(status, STATUS_FOUND);
    assert_eq!(
        message.as_deref(),
        Some(
            "Found 2 holder(s), but 1190 of 3113 mutant handle(s) were not compared (192 \
              process(es) refused PROCESS_DUP_HANDLE); other holders may exist."
        )
    );
}

#[test]
fn row_c_not_found_is_claimed_only_when_every_handle_was_compared() {
    let (status, message) = status_for(0, 10, 0, 0);
    assert_eq!(status, STATUS_NOT_FOUND);
    assert_eq!(
        message.as_deref(),
        Some(
            "All 10 mutant handle(s) on the system were compared and none matched; this scan \
              could not see the holder. Re-run test-reset."
        )
    );
}

#[test]
fn row_d_skipped_handles_without_a_match_is_incomplete() {
    let (status, message) = status_for(0, 3113, 192, 1190);
    assert_eq!(status, STATUS_INCOMPLETE);
    assert_eq!(
        message.as_deref(),
        Some(
            "No compared handle matched, and 1190 of 3113 mutant handle(s) were not compared \
              (192 process(es) refused PROCESS_DUP_HANDLE), so the holder may be one of them. \
              Re-run test-reset; an elevated shell can inspect processes that refused \
              PROCESS_DUP_HANDLE."
        )
    );
}

#[test]
fn row_d_failed_duplication_alone_is_still_incomplete_not_not_found() {
    let (status, message) = status_for(0, 10, 0, 2);
    assert_eq!(status, STATUS_INCOMPLETE);
    assert_eq!(
        message.as_deref(),
        Some(
            "No compared handle matched, and 2 of 10 mutant handle(s) were not compared (0 \
              process(es) refused PROCESS_DUP_HANDLE), so the holder may be one of them. Re-run \
              test-reset; an elevated shell can inspect processes that refused PROCESS_DUP_HANDLE."
        )
    );
}

#[test]
fn no_row_asserts_a_cause_it_did_not_observe() {
    for (holders, uninspectable, skipped) in [(1, 0, 0), (1, 5, 7), (0, 0, 0), (0, 5, 7), (0, 0, 7)]
    {
        let (_, message) = status_for(holders, 20, uninspectable, skipped);
        let message = message.unwrap_or_default();
        for forbidden in ["exited", "protected", "probably"] {
            assert!(!message.contains(forbidden), "{forbidden:?} in {message:?}");
        }
    }
}
