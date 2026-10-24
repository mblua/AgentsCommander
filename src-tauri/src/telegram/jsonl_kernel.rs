// Shared scaffold for append-only JSONL session-file readers.
// Hosts the primitives reused by claude_watcher and codex_watcher:
// directory scan, offset-based incremental read,
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
/// a recent assistant turn for both Claude and Codex: Claude assistant lines are
/// 1-4 KiB, Codex `event_msg` lines are 200-800 B (worst-observed turn ≈47 KiB).
pub(crate) const PREAMBLE_MAX_BYTES: u64 = 64 * 1024;

/// Grace window for the §J timestamp filter. Lines with embedded timestamp
/// ≥ attach_time - RACE_GRACE_SECS are emitted (strict lower bound — future
/// timestamps from the live agent are always emitted).
pub(crate) const RACE_GRACE_SECS: i64 = 5;

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

/// How long the Claude reader waits for the transcript AC minted to appear
/// before it falls back to the newest file by mtime (#2454).
pub(crate) const PINNED_ATTACH_WAIT_SECS: u64 = 30;

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
/// [`read_new_lines`] discards them.
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

/// Read new bytes from a file starting at the given byte offset.
/// Returns parsed complete lines and updates the offset by actual bytes read.
/// Partial lines are accumulated in `remainder` for the next poll.
///
/// Thin wrapper over [`read_new_lines_with_starts`] that drops the offsets.
///
/// Called only from test code: both watchers read through
/// `read_new_lines_with_starts` so they can attach `record_start` to each
/// capture record (#2232 phase 1), and the kernel and extractor tests pin this
/// signature.
#[allow(dead_code)] // called only from test code
pub(crate) fn read_new_lines(
    path: &Path,
    offset: &mut u64,
    remainder: &mut String,
) -> std::io::Result<Vec<String>> {
    Ok(read_new_lines_with_starts(path, offset, remainder)?
        .into_iter()
        .map(|(_, line)| line)
        .collect())
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
            // Single huge line, can't reason about it.
            None => return Ok((vec![], vec![], new_offset)),
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
mod tests;
