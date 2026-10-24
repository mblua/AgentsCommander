use super::*;

fn obs(len: u64, prefix: &[u8]) -> FileObservation {
    FileObservation::new(len, prefix.to_vec())
}

// Test 5: append of 10 bytes to a 100-byte file does not advance the epoch.
#[test]
fn append_does_not_advance_the_epoch() {
    let head = vec![b'a'; 100];
    let stored = obs(100, &head);
    let mut grown = head.clone();
    grown.extend(std::iter::repeat_n(b'b', 10));
    let current = obs(110, &grown);
    assert_eq!(
        classify_observation(&stored, &current),
        ObservationVerdict::Append
    );
    assert!(!ObservationVerdict::Append.advances_epoch());
}

// Test 6: a file shorter than N that grows does not advance the epoch, and
// its stored prefix is extended rather than treated as a replacement.
#[test]
fn a_short_file_that_grows_is_not_a_replacement() {
    let mut stored = obs(4, b"head");
    let current = obs(9, b"head-tail");
    assert_eq!(
        classify_observation(&stored, &current),
        ObservationVerdict::Append
    );
    extend_observation(&mut stored, &current);
    assert_eq!(stored.len, 9);
    assert_eq!(stored.prefix, b"head-tail".to_vec());
    assert_eq!(stored.prefix_len, 9);
}

#[test]
fn truncation_is_detected_by_length() {
    let stored = obs(100, b"head");
    let current = obs(4, b"head");
    assert_eq!(
        classify_observation(&stored, &current),
        ObservationVerdict::Truncated
    );
}

// Test 8: prefix replacement at the same length advances the epoch.
#[test]
fn same_length_prefix_replacement_advances_the_epoch() {
    let stored = obs(10, b"aaaaaaaaaa");
    let current = obs(10, b"bbbbbbbbbb");
    let verdict = classify_observation(&stored, &current);
    assert_eq!(verdict, ObservationVerdict::Replaced);
    assert!(verdict.advances_epoch());
}

#[test]
fn a_missing_prefix_leaves_length_to_decide() {
    // A live read that started past the head carries no prefix evidence.
    let stored = obs(100, b"head-bytes");
    let current = FileObservation::new(140, Vec::new());
    assert_eq!(
        classify_observation(&stored, &current),
        ObservationVerdict::Append
    );
}

#[test]
fn a_shorter_prefix_is_not_lost_when_extending() {
    let mut stored = obs(100, b"head-bytes");
    let current = FileObservation::new(140, Vec::new());
    extend_observation(&mut stored, &current);
    assert_eq!(stored.len, 140);
    assert_eq!(stored.prefix, b"head-bytes".to_vec());
}
