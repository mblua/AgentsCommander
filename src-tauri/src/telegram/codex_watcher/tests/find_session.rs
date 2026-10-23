use super::*;

// ── find_session_file ─────────────────────────────────────────────────

fn day_dir(root: &Path, day: chrono::NaiveDate) -> PathBuf {
    root.join(format!("{:04}", day.format("%Y")))
        .join(format!("{:02}", day.format("%m")))
        .join(format!("{:02}", day.format("%d")))
}

#[test]
fn find_session_file_matches_by_cwd_canonicalization() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let today = day_dir(root, now.date_naive());

    let cwd_a = r"C:\Users\foo\bar";
    let cwd_b = r"C:\Users\foo\baz";

    let expected_a = write_rollout(
        &today,
        "rollout-a.jsonl",
        &cwd_a.replace('\\', "\\\\"),
        &now.to_rfc3339(),
    );
    let _b = write_rollout(
        &today,
        "rollout-b.jsonl",
        &cwd_b.replace('\\', "\\\\"),
        &now.to_rfc3339(),
    );

    let found = find_session_file(root, "c:\\users\\foo\\bar", now);
    assert_eq!(found, Some(expected_a));
}

#[test]
fn find_session_file_matches_forward_slash_ac_input_against_backslash_codex_input() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let today = day_dir(root, now.date_naive());

    // Codex writes backslashes in cwd; AC passes forward slashes.
    let codex_cwd = r"C:\Users\foo\bar";
    let ac_cwd = "C:/Users/foo/bar";

    let expected = write_rollout(
        &today,
        "rollout-fwd.jsonl",
        &codex_cwd.replace('\\', "\\\\"),
        &now.to_rfc3339(),
    );

    let found = find_session_file(root, ac_cwd, now);
    assert_eq!(found, Some(expected));
}

#[test]
fn find_session_file_picks_newest_mtime_when_multiple_match() {
    // M6: tiebreaker is file mtime, not payload.timestamp — see
    // find_session_file doc comment for why.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let today = day_dir(root, now.date_naive());

    let cwd = r"C:\Users\foo\bar";
    // Both files reference the same cwd; payload.timestamp values are not
    // used for tie-breaking. Sleep 150 ms between writes so the two
    // candidates have definitely-distinct mtimes on Windows NTFS.
    let ts = now.to_rfc3339();
    let _old = write_rollout(&today, "rollout-old.jsonl", &cwd.replace('\\', "\\\\"), &ts);
    std::thread::sleep(std::time::Duration::from_millis(150));
    let expected_new = write_rollout(&today, "rollout-new.jsonl", &cwd.replace('\\', "\\\\"), &ts);

    let found = find_session_file(root, "c:\\users\\foo\\bar", now);
    assert_eq!(found, Some(expected_new));
}

#[test]
fn find_session_file_picks_resumed_file_even_with_old_payload_timestamp() {
    // M6 use case: codex exec resume --last appends to a 4-day-old file.
    // The candidate has payload.timestamp = 4 days ago but mtime = now.
    // Another (older session, abandoned) candidate exists with both
    // payload.timestamp and mtime "today (earlier)".
    // The mtime tiebreaker should pick the resumed file (current mtime),
    // NOT the abandoned same-day file.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let cwd = r"C:\Users\foo\bar";

    // Abandoned same-day session — its payload.timestamp is "today" but
    // it was last touched a couple of minutes ago.
    let today = day_dir(root, now.date_naive());
    let abandoned_today = write_rollout(
        &today,
        "rollout-abandoned.jsonl",
        &cwd.replace('\\', "\\\\"),
        &(now - chrono::Duration::minutes(2)).to_rfc3339(),
    );
    std::thread::sleep(std::time::Duration::from_millis(150));

    // Resumed file lives 4 days back — its payload.timestamp is 4 days ago
    // but its mtime is "now" (we just wrote it = the resume just appended).
    let four_days_ago = now - chrono::Duration::days(4);
    let dir4 = day_dir(root, four_days_ago.date_naive());
    let resumed = write_rollout(
        &dir4,
        "rollout-resumed-4d.jsonl",
        &cwd.replace('\\', "\\\\"),
        &four_days_ago.to_rfc3339(),
    );

    let found = find_session_file(root, "c:\\users\\foo\\bar", now);
    assert_eq!(found, Some(resumed));
    // The abandoned file should NOT have been chosen.
    assert_ne!(found, Some(abandoned_today));
}

#[test]
fn find_session_file_accepts_fresh_mtime_files() {
    // A file created "now" must NOT be skipped by the mtime grace window.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let today = day_dir(root, now.date_naive());

    let cwd = r"C:\Users\foo\bar";
    let expected = write_rollout(
        &today,
        "rollout-fresh.jsonl",
        &cwd.replace('\\', "\\\\"),
        &now.to_rfc3339(),
    );
    let found = find_session_file(root, "c:\\users\\foo\\bar", now);
    assert_eq!(found, Some(expected));
}

#[test]
fn find_session_file_skips_files_without_rollout_prefix() {
    // Files whose name doesn't match `rollout-*.jsonl` must be ignored
    // (defensive against stray files in the partition).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let today = day_dir(root, now.date_naive());

    let cwd = r"C:\Users\foo\bar";
    // No `rollout-` prefix.
    let _stray = write_rollout(
        &today,
        "session.jsonl",
        &cwd.replace('\\', "\\\\"),
        &now.to_rfc3339(),
    );
    let found = find_session_file(root, "c:\\users\\foo\\bar", now);
    assert!(found.is_none());
}

#[test]
fn find_session_file_returns_none_when_no_match() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let today = day_dir(root, now.date_naive());

    let other = r"C:\Other\Path";
    let _file = write_rollout(
        &today,
        "rollout.jsonl",
        &other.replace('\\', "\\\\"),
        &now.to_rfc3339(),
    );

    let found = find_session_file(root, "c:\\users\\foo\\bar", now);
    assert!(found.is_none());
}

#[test]
fn find_session_file_handles_missing_date_partition() {
    // Empty root — no partitions at all. Function returns None without panic.
    let tmp = tempfile::tempdir().unwrap();
    let now = Utc::now();
    let found = find_session_file(tmp.path(), "c:\\users\\foo\\bar", now);
    assert!(found.is_none());
}

#[test]
fn find_session_file_skips_unparseable_first_line() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let today = day_dir(root, now.date_naive());
    fs::create_dir_all(&today).unwrap();

    let bad = today.join("rollout-bad.jsonl");
    let mut f = fs::File::create(&bad).unwrap();
    writeln!(f, "{{ this is not valid JSON").unwrap();

    let found = find_session_file(root, "c:\\users\\foo\\bar", now);
    assert!(found.is_none());
}

#[test]
fn find_session_file_walks_past_seven_days() {
    // M6: the day-walk covers today + last 7 UTC days. A file 4 days
    // ago should be found.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let now = Utc::now();
    let four_days_ago = now - chrono::Duration::days(4);
    let dir = day_dir(root, four_days_ago.date_naive());

    let cwd = r"C:\Users\foo\bar";
    // The mtime of the freshly-written file will be now (well within the
    // 5 min mtime grace), simulating an APPEND on a resumed session.
    let expected = write_rollout(
        &dir,
        "rollout-old-session.jsonl",
        &cwd.replace('\\', "\\\\"),
        &four_days_ago.to_rfc3339(),
    );

    let found = find_session_file(root, "c:\\users\\foo\\bar", now);
    assert_eq!(found, Some(expected));
}
