//! Discovery and incremental reads of foreign JSONL transcripts.

use std::io::{Read as IoRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};

/// Find the most recently modified .jsonl file in a directory (non-recursive).
/// Used by the Claude watcher; Codex has its own per-poll discovery.
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

/// Find `<transcript_id>.jsonl` in a directory (non-recursive, #2454).
///
/// The stem is compared case-insensitively and whole paths are never compared:
/// a path may be in Windows verbatim form. `find_latest_jsonl` is left as it
/// is because the Codex watcher shares it.
pub(crate) fn find_pinned_jsonl(project_dir: &Path, transcript_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(project_dir).ok()?;
    entries.flatten().map(|entry| entry.path()).find(|path| {
        path.extension().and_then(|e| e.to_str()) == Some("jsonl")
            && path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|stem| stem.eq_ignore_ascii_case(transcript_id))
            && path.is_file()
    })
}

/// Read new lines from a file starting at the given byte offset, keeping each
/// line's absolute start offset.
///
/// Starts are absolute file byte offsets of the **raw** line, computed before
/// trimming, so `raw[start..]` walks back to the exact bytes in the file. A
/// line completed by a carried-over remainder takes the absolute offset of that
/// remainder. Empty (or whitespace-only) lines are still discarded, exactly as
/// the `read_new_lines` wrapper discards them.
///
/// **H2 truncation reset (commit 5):** on file shrink, set `offset = file_len`
/// (silent skip to current EOF, NO replay) and clear `remainder`. This
/// supersedes the previous reset-to-0 behavior which replayed the full file
/// to Telegram after operational truncates (logrotate, manual `truncate`,
/// replacing snapshot, partial-write race). The §J preamble scan is
/// first-attach-only and does NOT re-apply on truncation.
pub(crate) fn read_new_lines_with_starts(
    path: &Path,
    offset: &mut u64,
    remainder: &mut String,
) -> std::io::Result<Vec<(u64, String)>> {
    let mut file = std::fs::File::open(path)?;
    // Use metadata on the open handle (avoids TOCTOU with path-based metadata)
    let file_len = file.metadata()?.len();

    // H2: File truncation/shrink — silent skip to current EOF, no replay.
    if file_len < *offset {
        log::warn!(
            target: "agentscommander_lib::telegram::jsonl_kernel",
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
    #[expect(
        clippy::verbose_file_reads,
        reason = "Read from saved cursor and advance by actual bytes"
    )]
    file.read_to_string(&mut buf)?;

    // G2: Track offset by actual bytes read, not reported file length
    *offset += buf.len() as u64;

    // Prepend any partial line from previous read. The remainder occupies the
    // bytes immediately before the range just read, so the absolute offset of
    // the combined buffer is `read_start - remainder.len()`.
    let mut base = *offset - buf.len() as u64;
    if !remainder.is_empty() {
        let mut combined = std::mem::take(remainder);
        base -= combined.len() as u64;
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
                lines.push((base + last_newline as u64, trimmed.to_string()));
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
/// The `id` slot lets backends that dedupe by id seed their dedup set
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
    preamble_max_bytes: u64,
    race_grace_secs: i64,
) -> std::io::Result<(Vec<String>, Vec<Option<String>>, u64)> {
    let initial_len = std::fs::metadata(path)?.len();
    let start = initial_len.saturating_sub(preamble_max_bytes);
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut buf: Vec<u8> = Vec::with_capacity((initial_len - start) as usize);
    #[expect(
        clippy::verbose_file_reads,
        reason = "Read sought preamble handle and retain actual replay offset"
    )]
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
            // Single huge line, can't reason about it.
            None => return Ok((vec![], vec![], new_offset)),
        }
    };
    let text = String::from_utf8_lossy(bytes);

    let cutoff = attach_time - chrono::Duration::seconds(race_grace_secs);
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

/// Probe a transcript directory at the caller's original observation point.
pub(crate) fn transcript_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn transcript_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn transcript_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    std::fs::metadata(path)
}

/// Read once, up to 64 KiB, and decode the first header line lossily.
pub(crate) fn read_transcript_header(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 64 * 1024];
    let n = f.read(&mut buf).ok()?;
    let bytes = &buf[..n];
    let end = bytes
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(bytes.len());
    let line = String::from_utf8_lossy(&bytes[..end]).into_owned();
    Some(line)
}

/// Read the live window without decoding or resynchronizing record boundaries.
pub(crate) fn read_live_preamble_window(
    path: &Path,
    max_bytes: u64,
) -> std::io::Result<(Vec<u8>, u64)> {
    let initial_len = std::fs::metadata(path)?.len();
    let window_start = initial_len.saturating_sub(max_bytes);
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(window_start))?;
    let mut buf: Vec<u8> = Vec::new();
    #[expect(
        clippy::verbose_file_reads,
        reason = "Read live preamble from the selected window cursor"
    )]
    file.read_to_end(&mut buf)?;
    Ok((buf, window_start))
}

/// Visit eligible names and mtimes serially, before advancing to the next entry.
pub(crate) fn visit_codex_rollout_candidates(
    day: &Path,
    mut visit: impl FnMut(PathBuf, SystemTime),
) {
    let read = match std::fs::read_dir(day) {
        Ok(r) => r,
        Err(_) => return, // partition missing — normal for days with no codex usage
    };
    for entry in read.flatten() {
        let path = entry.path();
        let fname = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !fname.starts_with("rollout-") || !fname.ends_with(".jsonl") {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime = match meta.modified() {
            Ok(m) => m,
            Err(_) => continue, // can't compare without mtime
        };
        visit(path, mtime);
    }
}
