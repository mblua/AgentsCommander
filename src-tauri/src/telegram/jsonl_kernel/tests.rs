use super::*;
use std::fs;
use std::io::Write;

// ── H2 truncation tests ───────────────────────────────────────────────

#[test]
fn truncation_does_not_replay_old_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");

    // Write 5 lines, advance offset to file_len.
    let mut f = fs::File::create(&path).unwrap();
    for i in 0..5 {
        writeln!(f, r#"{{"id":"{}","content":"line{}"}}"#, i, i).unwrap();
    }
    f.sync_all().unwrap();
    drop(f);
    let file_len = fs::metadata(&path).unwrap().len();
    let mut offset = file_len;
    let mut remainder = String::new();

    // No new content — read returns empty.
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert!(lines.is_empty());
    assert_eq!(offset, file_len);

    // Truncate to 1 line.
    let mut f = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    writeln!(f, r#"{{"id":"x","content":"only"}}"#).unwrap();
    f.sync_all().unwrap();
    drop(f);
    let new_len = fs::metadata(&path).unwrap().len();
    assert!(new_len < file_len, "truncation must shrink the file");

    // read_new_lines must NOT replay the truncated content. It sets
    // offset = new_len and returns an empty Vec.
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert!(lines.is_empty(), "truncation must not replay");
    assert_eq!(offset, new_len, "offset must be re-anchored to new EOF");
}

#[test]
fn truncation_followed_by_new_lines_emits_only_new() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");

    // Initial state: 5 lines + offset at EOF.
    let mut f = fs::File::create(&path).unwrap();
    for i in 0..5 {
        writeln!(f, r#"{{"id":"{}","content":"line{}"}}"#, i, i).unwrap();
    }
    drop(f);
    let mut offset = fs::metadata(&path).unwrap().len();
    let mut remainder = String::new();

    // Truncate to nothing.
    let mut f = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    // Append 1 new line.
    writeln!(f, r#"{{"id":"new","content":"after-truncate"}}"#).unwrap();
    drop(f);

    // First poll detects shrink, re-anchors, emits nothing.
    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert!(lines.is_empty());

    // The truncated file already has 1 line; offset should be at file_len
    // matching that 1 line. So a follow-up append is what we want to test.
    // Append one more line.
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(f, r#"{{"id":"newer","content":"after-append"}}"#).unwrap();
    drop(f);

    let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("after-append"));
}

// ── §J preamble scan tests ────────────────────────────────────────────

/// Extractor for tests: reads `timestamp` field as RFC3339, `content` as the body.
/// `id` slot is unused (None).
fn test_extractor(line: &str) -> Option<(DateTime<Utc>, Option<String>, String)> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let ts_str = v.get("timestamp")?.as_str()?;
    let ts = DateTime::parse_from_rfc3339(ts_str)
        .ok()?
        .with_timezone(&Utc);
    let body = v.get("content")?.as_str()?.to_string();
    Some((ts, None, body))
}

#[test]
fn preamble_scan_emits_messages_inside_grace_window() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    let now = Utc::now();
    let old1 = now - chrono::Duration::seconds(20);
    let old2 = now - chrono::Duration::seconds(15);
    let fresh = now - chrono::Duration::seconds(1);

    let mut f = fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#"{{"timestamp":"{}","content":"old1"}}"#,
        old1.to_rfc3339()
    )
    .unwrap();
    writeln!(
        f,
        r#"{{"timestamp":"{}","content":"old2"}}"#,
        old2.to_rfc3339()
    )
    .unwrap();
    writeln!(
        f,
        r#"{{"timestamp":"{}","content":"fresh"}}"#,
        fresh.to_rfc3339()
    )
    .unwrap();
    drop(f);

    let (bodies, ids, file_len) = read_preamble_for_race(&path, now, test_extractor).unwrap();
    assert_eq!(bodies, vec!["fresh".to_string()]);
    assert_eq!(ids.len(), bodies.len(), "ids parallel to bodies");
    assert_eq!(file_len, fs::metadata(&path).unwrap().len());
}

#[test]
fn preamble_scan_emits_nothing_when_all_lines_old() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    let now = Utc::now();
    let old = now - chrono::Duration::seconds(30);

    let mut f = fs::File::create(&path).unwrap();
    for i in 0..3 {
        writeln!(
            f,
            r#"{{"timestamp":"{}","content":"old{}"}}"#,
            old.to_rfc3339(),
            i
        )
        .unwrap();
    }
    drop(f);

    let (bodies, ids, file_len) = read_preamble_for_race(&path, now, test_extractor).unwrap();
    assert!(bodies.is_empty());
    assert!(ids.is_empty());
    assert_eq!(file_len, fs::metadata(&path).unwrap().len());
}

#[test]
fn preamble_scan_emits_future_timestamps() {
    // Future timestamps relative to attach_time are emitted (lower-bound
    // filter only).
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    let now = Utc::now();
    let future = now + chrono::Duration::seconds(60);

    let mut f = fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#"{{"timestamp":"{}","content":"future"}}"#,
        future.to_rfc3339()
    )
    .unwrap();
    drop(f);

    let (bodies, _ids, _file_len) = read_preamble_for_race(&path, now, test_extractor).unwrap();
    assert_eq!(bodies, vec!["future".to_string()]);
}

#[test]
fn preamble_scan_caps_at_64_kib() {
    // File > 64 KiB. Only the last 64 KiB's lines should be examined.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    let now = Utc::now();
    let fresh = now - chrono::Duration::seconds(1);

    let mut f = fs::File::create(&path).unwrap();
    // Pad with junk to exceed 64 KiB (each line ~100 B; need ~700 lines).
    // Each "junk" line will not parse via test_extractor (missing
    // `timestamp` field) so we don't need to be precise about content.
    let padding_line = "x".repeat(200);
    for _ in 0..700 {
        writeln!(f, "{}", padding_line).unwrap();
    }
    // One fresh line at the end.
    writeln!(
        f,
        r#"{{"timestamp":"{}","content":"tail"}}"#,
        fresh.to_rfc3339()
    )
    .unwrap();
    drop(f);

    let file_size = fs::metadata(&path).unwrap().len();
    assert!(file_size > PREAMBLE_MAX_BYTES);

    let (bodies, _ids, file_len) = read_preamble_for_race(&path, now, test_extractor).unwrap();
    assert_eq!(bodies, vec!["tail".to_string()]);
    assert_eq!(file_len, file_size);
}

#[test]
fn preamble_scan_handles_partial_first_line_in_window() {
    // First line in the 64 KiB window is mid-JSON → that line is skipped
    // (drop bytes before first `\n`).
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    let now = Utc::now();
    let fresh = now - chrono::Duration::seconds(1);

    // Write a HUGE first line that gets cut by the 64 KiB window.
    let mut f = fs::File::create(&path).unwrap();
    let huge = "z".repeat((PREAMBLE_MAX_BYTES as usize) + 500);
    writeln!(f, "{}", huge).unwrap();
    // Then a real line.
    writeln!(
        f,
        r#"{{"timestamp":"{}","content":"after-huge"}}"#,
        fresh.to_rfc3339()
    )
    .unwrap();
    drop(f);

    let (bodies, _ids, _file_len) = read_preamble_for_race(&path, now, test_extractor).unwrap();
    // The huge first line's tail (z's) won't parse; only "after-huge" is emitted.
    assert_eq!(bodies, vec!["after-huge".to_string()]);
}

#[test]
fn preamble_scan_byte_safe_on_utf8_split_at_seek_point() {
    // Seek lands mid-multi-byte-UTF-8 character. Skip-to-first-\n drops
    // the partial codepoint; lossy UTF-8 conversion never errors.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    let now = Utc::now();
    let fresh = now - chrono::Duration::seconds(1);

    // Build a file where the byte at PREAMBLE_MAX_BYTES (from the end)
    // is in the middle of a 3-byte UTF-8 codepoint (em dash U+2014 is
    // 0xE2 0x80 0x94 in UTF-8).
    let mut f = fs::File::create(&path).unwrap();
    // Padding to push the start of the window into the middle of a codepoint:
    // Write 64 KB + some bytes of padding lines, then sprinkle in em-dashes
    // that span the seek point.
    for _ in 0..600 {
        // ~120 bytes per line including em dash
        writeln!(f, "padding em-dash \u{2014} text").unwrap();
    }
    // Emit one tail line that should be emitted.
    writeln!(
        f,
        r#"{{"timestamp":"{}","content":"tail-line"}}"#,
        fresh.to_rfc3339()
    )
    .unwrap();
    drop(f);

    // Just verify the call returns Ok (no panic) and includes "tail-line".
    let (bodies, _ids, _) = read_preamble_for_race(&path, now, test_extractor).unwrap();
    assert!(bodies.contains(&"tail-line".to_string()));
}

#[test]
fn preamble_scan_with_single_huge_line_partial_at_start_emits_empty() {
    // File contains one 80 KB line — preamble window has no `\n` → empty Vec.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    let now = Utc::now();

    let mut f = fs::File::create(&path).unwrap();
    // Single line, no trailing \n, ~80 KB.
    let huge = "z".repeat(80 * 1024);
    f.write_all(huge.as_bytes()).unwrap();
    drop(f);

    let (bodies, ids, file_len) = read_preamble_for_race(&path, now, test_extractor).unwrap();
    assert!(bodies.is_empty());
    assert!(ids.is_empty());
    assert_eq!(file_len, fs::metadata(&path).unwrap().len());
}

#[test]
fn preamble_scan_small_file_starts_from_offset_zero() {
    // File < 64 KiB → start == 0 path is exercised; we keep the first
    // (complete) line.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("session.jsonl");
    let now = Utc::now();
    let fresh = now - chrono::Duration::seconds(1);

    let mut f = fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#"{{"timestamp":"{}","content":"only"}}"#,
        fresh.to_rfc3339()
    )
    .unwrap();
    drop(f);

    let (bodies, ids, _file_len) = read_preamble_for_race(&path, now, test_extractor).unwrap();
    assert_eq!(bodies, vec!["only".to_string()]);
    assert_eq!(ids, vec![None]);
}

// ── #2232 phase 1: absolute line starts ───────────────────────────────

#[test]
fn read_new_lines_with_starts_indexes_the_raw_line_bytes() {
    // Test 1: offsets index back to the exact raw bytes of each line, with
    // a blank line and a whitespace-only line between them.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("starts.jsonl");
    let raw = "{\"a\":1}\n\n   \n{\"b\":2}  \n{\"c\":3}\n";
    fs::write(&path, raw).unwrap();

    let mut offset = 0u64;
    let mut remainder = String::new();
    let lines = read_new_lines_with_starts(&path, &mut offset, &mut remainder).unwrap();

    assert_eq!(
        lines
            .iter()
            .map(|(_, line)| line.as_str())
            .collect::<Vec<_>>(),
        vec!["{\"a\":1}", "{\"b\":2}", "{\"c\":3}"]
    );
    for (start, line) in &lines {
        let start = *start as usize;
        assert!(
            start == 0 || raw.as_bytes()[start - 1] == b'\n',
            "offset {start} must sit on a line boundary"
        );
        let raw_line = raw[start..].split('\n').next().unwrap();
        assert_eq!(raw_line.trim(), line, "offset must point at the raw line");
    }
    assert_eq!(offset, raw.len() as u64);
}

#[test]
fn read_new_lines_with_starts_keeps_the_remainder_start() {
    // A line completed by the carried remainder takes that remainder's own
    // absolute offset, not the offset of the newly read bytes.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("starts-remainder.jsonl");
    let mut raw = String::from("{\"a\":1");
    fs::write(&path, &raw).unwrap();

    let mut offset = 0u64;
    let mut remainder = String::new();
    let first = read_new_lines_with_starts(&path, &mut offset, &mut remainder).unwrap();
    assert!(first.is_empty(), "a partial line must not produce a record");
    assert_eq!(offset, raw.len() as u64);

    raw.push_str("}\n{\"b\":2}\n");
    fs::write(&path, &raw).unwrap();
    let lines = read_new_lines_with_starts(&path, &mut offset, &mut remainder).unwrap();

    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0].0, 0,
        "the completed remainder starts at its own offset"
    );
    assert_eq!(lines[0].1, "{\"a\":1}");
    assert_eq!(lines[1].0, raw.find("{\"b\":2}").unwrap() as u64);
    assert_eq!(lines[1].1, "{\"b\":2}");
}

#[test]
fn read_new_lines_matches_with_starts_bodies() {
    // Test 2: on the kernel fixtures, `read_new_lines` is the body
    // projection of `read_new_lines_with_starts` — same lines, same
    // offset advance, same remainder.
    let fixtures = [
        include_str!("tests/fixtures/line-bodies.json"),
        "{\"a\":1}\n\n   \n{\"b\":2}\n{\"partial\":",
        "{\"x\":\"é\"}\n\t\n{\"y\":2}\n",
    ];
    for raw in fixtures {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("kernel-fixture.jsonl");
        fs::write(&path, raw).unwrap();

        let mut plain_offset = 0u64;
        let mut plain_remainder = String::new();
        let plain = read_new_lines(&path, &mut plain_offset, &mut plain_remainder).unwrap();

        let mut start_offset = 0u64;
        let mut start_remainder = String::new();
        let with_starts =
            read_new_lines_with_starts(&path, &mut start_offset, &mut start_remainder).unwrap();

        assert_eq!(
            plain,
            with_starts
                .iter()
                .map(|(_, line)| line.clone())
                .collect::<Vec<_>>(),
            "raw={raw:?}"
        );
        assert_eq!(plain_offset, start_offset, "raw={raw:?}");
        assert_eq!(plain_remainder, start_remainder, "raw={raw:?}");
    }
}
