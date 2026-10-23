use super::*;

// ── #2232 phase 1: capture records ────────────────────────────────────

/// A current-format final record carrying the nested turn id the real
/// fixture shows, so grouping has something to match.
fn current_final_record_with_turn(text: &str, turn_id: &str) -> String {
    let mut value =
        serde_json::from_str::<serde_json::Value>(&current_final_record(&[text])).unwrap();
    value["payload"]["internal_chat_message_metadata_passthrough"] =
        serde_json::json!({"turn_id": turn_id});
    value.to_string()
}

/// A `task_complete` closure line with the flat turn id (#2232 phase 5).
fn task_complete_record(turn_id: &str, last_agent_message: Option<&str>) -> String {
    let mut payload = serde_json::json!({"type": "task_complete", "turn_id": turn_id});
    if let Some(last) = last_agent_message {
        payload["last_agent_message"] = serde_json::json!(last);
    }
    serde_json::json!({"type": "event_msg", "payload": payload}).to_string()
}

/// A `turn_complete` closure line: the alias of `task_complete` (#2356).
fn turn_complete_record(turn_id: &str, last_agent_message: Option<&str>) -> String {
    let mut payload = serde_json::json!({"type": "turn_complete", "turn_id": turn_id});
    if let Some(last) = last_agent_message {
        payload["last_agent_message"] = serde_json::json!(last);
    }
    serde_json::json!({"type": "event_msg", "payload": payload}).to_string()
}

/// A `task_started` closure line with the flat turn id.
fn task_started_record(turn_id: &str) -> String {
    serde_json::json!({
        "type": "event_msg",
        "payload": {"type": "task_started", "turn_id": turn_id}
    })
    .to_string()
}

/// #2356 item 1: a `turn_complete` closes the turn like `task_complete`.
#[test]
fn turn_complete_alias_emits_exactly_one_record() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut reader_seq = 0u64;
    let mut turns = TurnAccumulator::new();
    let records = capture_live_lines(
        vec![
            (100, current_final_record_with_turn("body", "turn-a")),
            (200, turn_complete_record("turn-a", Some("body"))),
        ],
        "codex-session",
        Path::new("rollout.jsonl"),
        &mut reader_seq,
        Some(&tx),
        RecordOrigin::Live,
        &ReaderAttachment::default(),
        &mut turns,
    );
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].text, "body");
    assert_eq!(records[0].record_start, Some(100));
    assert_eq!(rx.try_recv().unwrap().text, "body");
    assert!(rx.try_recv().is_err());
}

/// #2356 item 2: a superseded turn never reaches the sink, even when a late
/// fragment and a matching closure follow; a healthy turn still does.
#[test]
fn a_superseded_turn_never_reaches_the_sink() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut reader_seq = 0u64;
    let mut turns = TurnAccumulator::new();
    let records = capture_live_lines(
        vec![
            (0, task_complete_record("turn-s", Some("late"))),
            (10, task_started_record("turn-n")),
            (20, current_final_record_with_turn("late", "turn-s")),
            (30, task_complete_record("turn-s", Some("late"))),
            (40, current_final_record_with_turn("ok", "turn-h")),
            (50, task_complete_record("turn-h", Some("ok"))),
        ],
        "codex-session",
        Path::new("rollout.jsonl"),
        &mut reader_seq,
        Some(&tx),
        RecordOrigin::Live,
        &ReaderAttachment::default(),
        &mut turns,
    );
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].text, "ok");
    assert_eq!(rx.try_recv().unwrap().text, "ok");
    assert!(rx.try_recv().is_err());
    assert_eq!(turns.counters().late_after_superseded, 1);
    assert_eq!(turns.counters().emitted, 1);
}

/// Tests 1 and 9 (#2232 phase 5): the real fixture's nested turn id groups
/// with the flat id of a matching `task_complete`, and the assembled
/// record carries the first contributing offset and the assembled digest.
#[test]
fn real_fixture_carries_the_nested_turn_id_and_final_bits() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut reader_seq = 0u64;
    let mut turns = TurnAccumulator::new();
    let records = capture_live_lines(
        vec![
            (17, REAL_CURRENT_CODEX_FINAL.to_string()),
            (
                REAL_CURRENT_CODEX_FINAL.len() as u64 + 18,
                task_complete_record(
                    "01a09c07-0ad4-7710-9c1d-ce3f8dd0aaac",
                    Some("sanitized assistant reply"),
                ),
            ),
        ],
        "codex-session",
        Path::new("rollout-real.jsonl"),
        &mut reader_seq,
        Some(&tx),
        RecordOrigin::Live,
        &ReaderAttachment::default(),
        &mut turns,
    );
    assert_eq!(records.len(), 1, "one whole turn is one candidate");
    let record = &records[0];
    assert_eq!(
        record.turn_id.as_deref(),
        Some("01a09c07-0ad4-7710-9c1d-ce3f8dd0aaac")
    );
    assert!(record.provider_final);
    assert!(record.turn_identified);
    assert_eq!(record.text, "sanitized assistant reply");
    assert_eq!(record.provider, CaptureProvider::Codex);
    assert_eq!(record.origin, RecordOrigin::Live);
    // The record start belongs to the first contributing record, not the
    // closure line, and the digest covers the assembled text.
    assert_eq!(record.record_start, Some(17));
    let expected: [u8; 32] = <Sha256 as Digest>::digest(record.text.as_bytes()).into();
    assert_eq!(record.text_sha256, expected);
    assert_eq!(record.reader_seq, 0);
    // The record reached the sink, exactly once.
    assert_eq!(rx.try_recv().unwrap().text, "sanitized assistant reply");
    assert!(rx.try_recv().is_err());
}

/// Test 8: a `final_answer` with no turn identifier keeps phase 1's bits,
/// is emitted immediately and is still routable (it reaches the sink).
#[test]
fn final_answer_without_turn_metadata_is_final_but_not_identified() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut reader_seq = 0u64;
    let mut turns = TurnAccumulator::new();
    let records = capture_live_lines(
        vec![(0, current_final_record(&["no metadata"]))],
        "codex-session",
        Path::new("rollout.jsonl"),
        &mut reader_seq,
        Some(&tx),
        RecordOrigin::Live,
        &ReaderAttachment::default(),
        &mut turns,
    );
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].turn_id, None);
    assert!(records[0].provider_final);
    assert!(!records[0].turn_identified);
    assert_eq!(rx.try_recv().unwrap().text, "no metadata");
    assert_eq!(turns.open_turns(), 0, "nothing is held without a turn id");
}

/// Test 2 (#2232 phase 5): the extractor still rejects `task_complete`, and
/// the new closure path consumes it without emitting when it matches no
/// accumulated final answer.
#[test]
fn task_complete_emits_no_capture_record() {
    let line = task_complete_record("01a09c07-0ad4-7710-9c1d-ce3f8dd0aaac", None);
    assert_eq!(extract_assistant_final(&line), None);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut reader_seq = 0u64;
    let mut turns = TurnAccumulator::new();
    let records = capture_live_lines(
        vec![(0, line)],
        "codex-session",
        Path::new("rollout.jsonl"),
        &mut reader_seq,
        Some(&tx),
        RecordOrigin::Live,
        &ReaderAttachment::default(),
        &mut turns,
    );
    assert!(records.is_empty());
    assert!(rx.try_recv().is_err(), "nothing may reach the sink");
    assert_eq!(reader_seq, 0, "a rejected line must not advance reader_seq");
    assert_eq!(turns.counters().abstained, 1, "abstain with a reason");
    assert_eq!(turns.counters().emitted, 0);
}

/// Test 3 (#2232 phase 5): two records of one turn assemble in file order
/// and exactly one `CapturedRecord` reaches the sink.
#[test]
fn two_final_answers_of_one_turn_emit_exactly_one_record() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut reader_seq = 0u64;
    let mut turns = TurnAccumulator::new();
    let records = capture_live_lines(
        vec![
            (100, current_final_record_with_turn("first part", "turn-9")),
            (200, current_final_record_with_turn("second part", "turn-9")),
            (
                300,
                task_complete_record("turn-9", Some("first part\nsecond part")),
            ),
        ],
        "codex-session",
        Path::new("rollout.jsonl"),
        &mut reader_seq,
        Some(&tx),
        RecordOrigin::Live,
        &ReaderAttachment::default(),
        &mut turns,
    );
    assert_eq!(
        records.len(),
        1,
        "not merely the right text: the count is 1"
    );
    assert_eq!(records[0].text, "first part\nsecond part");
    assert_eq!(records[0].record_start, Some(100));
    let delivered = rx.try_recv().expect("the one record reaches the sink");
    assert_eq!(delivered.text, "first part\nsecond part");
    assert!(
        rx.try_recv().is_err(),
        "no fragment may also reach the sink"
    );
}

/// Test 7 (#2232 phase 5): the flat id belongs to `task_complete` and the
/// nested one to `final_answer`; swapping them matches nothing. The final
/// answer is then an ungrouped immediate candidate and the closure is a
/// counted malformed skip.
#[test]
fn swapped_turn_id_locations_match_nothing() {
    // Final answer with the id in the *flat* (closure) location: the
    // extractor reads only the nested one, so this is no turn id at all.
    let mut flat_final = valid_final_value();
    flat_final["payload"]["content"] =
        serde_json::json!([{"type": "output_text", "text": "flat body"}]);
    flat_final["payload"]["turn_id"] = serde_json::json!("swapped");
    // Completion with the id in the *nested* (final answer) location: the
    // closure parser reads only the flat one, so this is malformed.
    let nested_completion = serde_json::json!({
        "type": "event_msg",
        "payload": {
            "type": "task_complete",
            "last_agent_message": "flat body",
            "internal_chat_message_metadata_passthrough": {"turn_id": "swapped"}
        }
    })
    .to_string();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut reader_seq = 0u64;
    let mut turns = TurnAccumulator::new();
    let records = capture_live_lines(
        vec![(11, flat_final.to_string()), (22, nested_completion)],
        "codex-session",
        Path::new("rollout.jsonl"),
        &mut reader_seq,
        Some(&tx),
        RecordOrigin::Live,
        &ReaderAttachment::default(),
        &mut turns,
    );
    // Nothing was grouped: the prose left as an ungrouped candidate.
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].text, "flat body");
    assert!(!records[0].turn_identified);
    assert!(rx.try_recv().is_ok());
    assert_eq!(turns.counters().emitted, 0, "no grouped turn may emit");
    assert_eq!(turns.counters().malformed_closures, 1);
}

#[test]
fn capture_without_a_sender_leaves_the_telegram_bytes_unchanged() {
    // Test 8: a full watcher read pass with the sender left `None` appends
    // exactly the bytes the pre-change code appended.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rollout-parity.jsonl");
    let mut f = fs::File::create(&path).unwrap();
    writeln!(f, "{}", current_final_record(&["first body"])).unwrap();
    writeln!(
        f,
        r#"{{"type":"event_msg","payload":{{"type":"task_complete"}}}}"#
    )
    .unwrap();
    writeln!(f, "{}", current_final_record(&["second body"])).unwrap();
    drop(f);

    let mut offset = 0u64;
    let mut remainder = String::new();
    let new_lines = read_new_lines_with_starts(&path, &mut offset, &mut remainder).unwrap();

    // Pre-change append path: extractor text, one trailing newline each.
    let mut expected = String::new();
    for (_, line) in &new_lines {
        if let Some(text) = extract_assistant_final(line) {
            expected.push_str(&text);
            expected.push('\n');
        }
    }

    // Watcher path: capture with no sink attached, then buffer the text.
    let mut reader_seq = 0u64;
    let mut turns = TurnAccumulator::new();
    let mut observed = String::new();
    for record in capture_live_lines(
        new_lines,
        "codex-session",
        &path,
        &mut reader_seq,
        None,
        RecordOrigin::Live,
        &ReaderAttachment::default(),
        &mut turns,
    ) {
        observed.push_str(&record.text);
        observed.push('\n');
    }

    assert_eq!(expected, observed);
    assert_eq!(reader_seq, 2);
}
