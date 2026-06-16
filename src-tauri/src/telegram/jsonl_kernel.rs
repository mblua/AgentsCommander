// Shared scaffold for append-only JSONL session-file readers.
// Hosts the primitives reused by claude_watcher, codex_watcher, and
// gemini_watcher: directory scan, offset-based incremental read,
// truncation detection, rotation-flicker constants, and the §J first-attach
// preamble scan.

use std::io::{Read as IoRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};

pub(crate) const POLL_INTERVAL_MS: u64 = 500;
/// Duration a tracked file must be stale before switching to a newer one (file rotation guard)
pub(crate) const ROTATION_STALE_SECS: u64 = 3;

/// Window size for the §J first-attach preamble scan. 64 KiB comfortably holds
/// a recent assistant turn for all three backends: Claude assistant lines are
/// 1-4 KiB, Codex `event_msg` lines are 200-800 B (worst-observed turn ≈47 KiB),
/// Gemini turns are usually one large line.
pub(crate) const PREAMBLE_MAX_BYTES: u64 = 64 * 1024;

/// Grace window for the §J timestamp filter. Lines with embedded timestamp
/// ≥ attach_time - RACE_GRACE_SECS are emitted (strict lower bound — future
/// timestamps from the live agent are always emitted).
pub(crate) const RACE_GRACE_SECS: i64 = 5;

/// Find the most recently modified .jsonl file in a directory (non-recursive).
/// Used by the Claude watcher; Codex and Gemini have their own per-poll discovery.
pub(crate) fn find_latest_jsonl(project_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(project_dir).ok()?;
    let mut best: Option<(PathBuf, SystemTime)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                match &best {
                    Some((_, best_time)) if modified > *best_time => {
                        best = Some((path, modified));
                    }
                    None => {
                        best = Some((path, modified));
                    }
                    _ => {}
                }
            }
        }
    }

    best.map(|(p, _)| p)
}

/// Read new bytes from a file starting at the given byte offset.
/// Returns parsed complete lines and updates the offset by actual bytes read.
/// Partial lines are accumulated in `remainder` for the next poll.
///
/// **H2 truncation reset (commit 5):** on file shrink, set `offset = file_len`
/// (silent skip to current EOF, NO replay) and clear `remainder`. This
/// supersedes the previous reset-to-0 behavior which replayed the full file
/// to Telegram after operational truncates (logrotate, manual `truncate`,
/// replacing snapshot, partial-write race). The §J preamble scan is
/// first-attach-only and does NOT re-apply on truncation.
pub(crate) fn read_new_lines(
    path: &Path,
    offset: &mut u64,
    remainder: &mut String,
) -> std::io::Result<Vec<String>> {
    let mut file = std::fs::File::open(path)?;
    // Use metadata on the open handle (avoids TOCTOU with path-based metadata)
    let file_len = file.metadata()?.len();

    // H2: File truncation/shrink — silent skip to current EOF, no replay.
    if file_len < *offset {
        log::warn!(
            "[JSONL_TRUNCATE] File shrunk from {} to {}, skipping past replay",
            *offset,
            file_len
        );
        *offset = file_len;
        remainder.clear();
        return Ok(vec![]);
    }

    if file_len <= *offset {
        return Ok(vec![]);
    }

    file.seek(SeekFrom::Start(*offset))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;

    // G2: Track offset by actual bytes read, not reported file length
    *offset += buf.len() as u64;

    // Prepend any partial line from previous read
    if !remainder.is_empty() {
        let mut combined = std::mem::take(remainder);
        combined.push_str(&buf);
        buf = combined;
    }

    let mut lines = Vec::new();
    let mut last_newline = 0;

    for (i, ch) in buf.char_indices() {
        if ch == '\n' {
            let line = &buf[last_newline..i];
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
            }
            last_newline = i + 1;
        }
    }

    // Keep unterminated tail in remainder for next poll
    if last_newline < buf.len() {
        *remainder = buf[last_newline..].to_string();
    }

    Ok(lines)
}

/// §J first-message-race fix. Instead of skipping to EOF on first attach,
/// read the last `PREAMBLE_MAX_BYTES` of the file and emit only lines whose
/// embedded timestamp is ≥ `attach_time - RACE_GRACE_SECS` (strict lower
/// bound). Then set `offset = file_len` so the watch loop continues from
/// the current EOF.
///
/// **Byte-safe at the seek point (M1):** reading from `len - 64 KiB` may land
/// mid-UTF-8 codepoint, so we (1) read raw bytes, (2) drop everything before
/// the first `\n` unless the window starts at offset 0, (3) only then convert
/// to UTF-8 lossily.
///
/// `extractor` is the per-backend "given a line, return its
/// `(timestamp, optional_id, body)` if it's an emission candidate, else None".
/// The `id` slot lets backends that dedupe by id (Gemini) seed their dedup set
/// directly from this single read — eliminating the M1 TOCTOU where a parallel
/// `seed_emitted_ids_from_preamble` pass re-read the file and could mark an
/// id as emitted whose body was appended between the two reads. Backends that
/// don't dedupe (Claude, Codex) pass `None` for the id slot.
///
/// Returns `(bodies, ids, file_len)` where `bodies` and `ids` are parallel
/// arrays: `ids[i]` is the optional id of `bodies[i]`. Callers should set
/// `offset = file_len` so the subsequent `read_new_lines` polls start from
/// current EOF.
pub(crate) fn read_preamble_for_race(
    path: &Path,
    attach_time: DateTime<Utc>,
    extractor: impl Fn(&str) -> Option<(DateTime<Utc>, Option<String>, String)>,
) -> std::io::Result<(Vec<String>, Vec<Option<String>>, u64)> {
    let initial_len = std::fs::metadata(path)?.len();
    let start = initial_len.saturating_sub(PREAMBLE_MAX_BYTES);
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut buf: Vec<u8> = Vec::with_capacity((initial_len - start) as usize);
    f.read_to_end(&mut buf)?;
    // Return the offset that matches what we ACTUALLY read (start + bytes read),
    // not the stale path-level metadata `initial_len`. Concurrent writers can
    // grow the file between the metadata call and the read; using the stale
    // value would leave a gap that `read_new_lines` then re-reads, causing
    // duplicate sends to Telegram.
    let new_offset = start + buf.len() as u64;

    // Drop everything before the first `\n` UNLESS we read from offset 0.
    // The seek point at `len - 64 KiB` almost certainly lands mid-line.
    let bytes: &[u8] = if start == 0 {
        &buf
    } else {
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => &buf[i + 1..],
            None => return Ok((vec![], vec![], new_offset)), // single huge line, can't reason about it
        }
    };
    let text = String::from_utf8_lossy(bytes);

    let cutoff = attach_time - chrono::Duration::seconds(RACE_GRACE_SECS);
    let mut bodies = Vec::new();
    let mut ids = Vec::new();
    for line in text.lines() {
        if let Some((ts, id, body)) = extractor(line) {
            if ts >= cutoff {
                bodies.push(body);
                ids.push(id);
            }
        }
    }
    Ok((bodies, ids, new_offset))
}

#[cfg(test)]
mod tests {
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
    /// `id` slot is unused (None) — backends that dedupe by id are covered in
    /// gemini_watcher tests.
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
}
