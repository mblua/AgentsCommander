use super::*;

// ── extract_assistant_final through the kernel watch paths ────────────

#[test]
fn preamble_emits_fresh_final_answer_and_incremental_read_does_not_repeat() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rollout-preamble.jsonl");
    let now = Utc::now();
    // Real fixture bytes with the timestamp rewritten into the grace
    // window; the JSON shape and sanitized text are preserved.
    let fresh_ts = (now - chrono::Duration::seconds(1)).to_rfc3339();
    let fresh = REAL_CURRENT_CODEX_FINAL.replace("2026-09-13T18:28:25.938Z", &fresh_ts);
    assert!(
        fresh.contains(&fresh_ts),
        "fixture timestamp must be rewritten"
    );
    let mut f = fs::File::create(&path).unwrap();
    writeln!(f, "{}", fresh).unwrap();
    drop(f);

    let (bodies, ids, file_len) =
        read_preamble_for_race(&path, now, codex_preamble_extractor).unwrap();
    assert_eq!(bodies, vec!["sanitized assistant reply".to_string()]);
    assert_eq!(ids, vec![None]);
    assert_eq!(file_len, fs::metadata(&path).unwrap().len());

    // The first bind advances the consumer to EOF: the next incremental
    // read must not repeat the consumed record.
    let mut offset = file_len;
    let mut remainder = String::new();
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert!(lines.is_empty(), "consumed preamble must not replay");
}

#[test]
fn preamble_rejects_missing_invalid_and_out_of_grace_timestamps() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rollout-preamble-reject.jsonl");
    let now = Utc::now();

    let fresh_ts = (now - chrono::Duration::seconds(1)).to_rfc3339();
    let stale_ts = (now - chrono::Duration::seconds(30)).to_rfc3339();
    let fresh = current_final_record_with_ts("fresh body", Some(&fresh_ts));
    let stale = current_final_record_with_ts("stale body", Some(&stale_ts));
    let invalid = current_final_record_with_ts("invalid body", Some("not-a-timestamp"));
    let missing = current_final_record_with_ts("missing body", None);

    let mut f = fs::File::create(&path).unwrap();
    for line in [&stale, &invalid, &missing, &fresh] {
        writeln!(f, "{}", line).unwrap();
    }
    drop(f);

    let (bodies, _ids, file_len) =
        read_preamble_for_race(&path, now, codex_preamble_extractor).unwrap();
    assert_eq!(
        bodies,
        vec!["fresh body".to_string()],
        "only the in-grace record may be emitted"
    );
    assert_eq!(file_len, fs::metadata(&path).unwrap().len());

    // Offset moves to EOF, so the rejected records are not replayed later
    // either.
    let mut offset = file_len;
    let mut remainder = String::new();
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert!(
        lines.is_empty(),
        "rejected preamble records must not replay"
    );
}

#[test]
fn incremental_read_completes_a_split_json_line_once() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rollout-split.jsonl");
    let line = current_final_record(&["split body"]);
    let (head, tail) = line.split_at(line.len() / 2);

    let mut f = fs::File::create(&path).unwrap();
    f.write_all(head.as_bytes()).unwrap();
    drop(f);

    let mut offset = 0u64;
    let mut remainder = String::new();
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert!(lines.is_empty(), "partial JSON must not produce a record");
    assert_eq!(offset, head.len() as u64);

    // Complete the line; the record is extracted exactly once.
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    f.write_all(tail.as_bytes()).unwrap();
    f.write_all(b"\n").unwrap();
    drop(f);

    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(extract_bodies(&lines), vec!["split body".to_string()]);
    assert_eq!(offset, fs::metadata(&path).unwrap().len());

    // A further poll (the same path the final drain uses) finds nothing.
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert!(lines.is_empty(), "completed record must not repeat");
}

#[test]
fn final_drain_extracts_complete_appended_records_once() {
    // Mirrors the cancel path: the final poll drains the complete records
    // appended since the last incremental read.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rollout-drain.jsonl");
    let first = current_final_record(&["drain one"]);
    let second = current_final_record(&["drain two"]);

    let mut offset = 0u64;
    let mut remainder = String::new();
    let mut f = fs::File::create(&path).unwrap();
    write!(f, "{}\n{}\n", first, second).unwrap();
    drop(f);

    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert_eq!(
        extract_bodies(&lines),
        vec!["drain one".to_string(), "drain two".to_string()]
    );
    assert_eq!(offset, fs::metadata(&path).unwrap().len());

    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert!(lines.is_empty(), "drained records must not repeat");
}

#[test]
fn coexisting_legacy_and_current_records_emit_only_the_response_body() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rollout-coexist.jsonl");
    let legacy = |msg: &str| {
        serde_json::json!({
            "type": "event_msg",
            "payload": {"type": "agent_message", "message": msg, "phase": "final"}
        })
        .to_string()
    };
    let current = current_final_record(&["current body"]);
    let mut offset = 0u64;
    let mut remainder = String::new();

    // Order 1: legacy before current in the same read.
    let mut f = fs::File::create(&path).unwrap();
    write!(f, "{}\n{}\n", legacy("legacy first"), current).unwrap();
    drop(f);
    let mut buffer = extract_bodies(&read_new_lines(&path, &mut offset, &mut remainder).unwrap());
    assert_eq!(buffer, vec!["current body".to_string()]);

    // Simulated flush: the Telegram buffer was cleared, then more records
    // arrive. Nothing already consumed may be re-emitted.
    buffer.clear();

    // Order 2: current before legacy, appended after the flush.
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    write!(f, "{}\n{}\n", current, legacy("legacy second")).unwrap();
    drop(f);
    buffer.extend(extract_bodies(
        &read_new_lines(&path, &mut offset, &mut remainder).unwrap(),
    ));
    assert_eq!(buffer, vec!["current body".to_string()]);
}

#[test]
fn two_identical_final_records_remain_two_bodies() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rollout-dupe.jsonl");
    let record = current_final_record(&["identical body"]);
    let mut offset = 0u64;
    let mut remainder = String::new();

    let mut f = fs::File::create(&path).unwrap();
    write!(f, "{}\n{}\n", record, record).unwrap();
    drop(f);

    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert_eq!(
        extract_bodies(&lines),
        vec!["identical body".to_string(), "identical body".to_string()],
        "no text dedup may collapse two distinct records"
    );
}

/// Appends synthetic, never-valid filler records until the file reaches
/// `target_len` bytes, always ending on a line boundary.
fn write_synthetic_filler(f: &mut fs::File, target_len: u64) {
    const LINE: &str = "synthetic filler for issue 1997 geometry (not a codex record)\n";
    let mut written = f.metadata().unwrap().len();
    while written + LINE.len() as u64 <= target_len {
        f.write_all(LINE.as_bytes()).unwrap();
        written += LINE.len() as u64;
    }
    if written < target_len {
        let rem = (target_len - written) as usize;
        let mut last = "x".repeat(rem - 1);
        last.push('\n');
        f.write_all(last.as_bytes()).unwrap();
        written += rem as u64;
    }
    assert_eq!(written, target_len);
    f.flush().unwrap();
}

#[test]
fn replay_prefix_geometry_extracts_the_real_fixture_once() {
    // Real rollout geometry: the watcher binds at byte 22352 and the real
    // final answer lands at byte 87513. The filler is synthetic; the
    // record is the byte-exact real fixture.
    const BIND_OFFSET: u64 = 22_352;
    const FIXTURE_OFFSET: u64 = 87_513;

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rollout-geometry.jsonl");
    let mut f = fs::File::create(&path).unwrap();
    write_synthetic_filler(&mut f, BIND_OFFSET);

    // First bind: the preamble scan leaves the consumer at EOF.
    let mut offset = f.metadata().unwrap().len();
    assert_eq!(offset, BIND_OFFSET);
    let mut remainder = String::new();

    // Filler arrives up to the byte where the real answer starts.
    write_synthetic_filler(&mut f, FIXTURE_OFFSET);
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert!(
        extract_bodies(&lines).is_empty(),
        "synthetic filler must never be extracted"
    );
    assert_eq!(offset, FIXTURE_OFFSET, "offset advances with the filler");

    // The real final answer arrives.
    f.write_all(REAL_CURRENT_CODEX_FINAL.as_bytes()).unwrap();
    f.write_all(b"\n").unwrap();
    f.flush().unwrap();

    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert_eq!(
        extract_bodies(&lines),
        vec!["sanitized assistant reply".to_string()]
    );
    assert_eq!(
        offset,
        FIXTURE_OFFSET + REAL_CURRENT_CODEX_FINAL.len() as u64 + 1,
        "offset advances past the consumed record"
    );

    // Next poll: nothing new and nothing replayed.
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert!(
        extract_bodies(&lines).is_empty(),
        "record must not replay on the next poll"
    );
}

#[test]
fn rotation_reanchor_and_truncation_do_not_replay_consumed_data() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rollout-rotation.jsonl");
    let mut offset = 0u64;
    let mut remainder = String::new();

    let mut f = fs::File::create(&path).unwrap();
    writeln!(f, "{}", REAL_CURRENT_CODEX_FINAL).unwrap();
    drop(f);
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert_eq!(
        extract_bodies(&lines),
        vec!["sanitized assistant reply".to_string()]
    );

    // Rotation: the watcher re-anchors at the new file's EOF instead of
    // replaying it.
    let mut rotated_offset = fs::metadata(&path).unwrap().len();
    let lines = read_new_lines(&path, &mut rotated_offset, &mut remainder).unwrap();
    assert!(lines.is_empty(), "rotation re-anchor must not replay");

    // Truncation: the kernel re-anchors to the new EOF, skipping replay.
    let mut f = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    writeln!(f, "{{ truncated }}").unwrap();
    drop(f);
    let new_len = fs::metadata(&path).unwrap().len();
    assert!(new_len < offset, "truncation must shrink the file");
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert!(
        lines.is_empty(),
        "truncation must not replay consumed records"
    );
    assert_eq!(offset, new_len, "offset re-anchors to the new EOF");

    // A record appended after truncation is extracted normally.
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(f, "{}", REAL_CURRENT_CODEX_FINAL).unwrap();
    drop(f);
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert_eq!(
        extract_bodies(&lines),
        vec!["sanitized assistant reply".to_string()]
    );
}
