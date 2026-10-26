// Shared scaffold for append-only JSONL session-file readers.
// Hosts the primitives reused by claude_watcher and codex_watcher:
// directory scan, offset-based incremental read,
// truncation detection, rotation-flicker constants, and the §J first-attach
// preamble scan.

use std::path::{Path, PathBuf};

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
    crate::filesystem::transcripts::find_latest_jsonl(project_dir)
}

pub(crate) fn transcript_is_dir(path: &Path) -> bool {
    crate::filesystem::transcripts::transcript_is_dir(path)
}

pub(crate) fn transcript_exists(path: &Path) -> bool {
    crate::filesystem::transcripts::transcript_exists(path)
}

pub(crate) fn transcript_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    crate::filesystem::transcripts::transcript_metadata(path)
}

pub(crate) fn read_transcript_header(path: &Path) -> Option<String> {
    crate::filesystem::transcripts::read_transcript_header(path)
}

pub(crate) fn read_live_preamble_window(
    path: &Path,
    max_bytes: u64,
) -> std::io::Result<(Vec<u8>, u64)> {
    crate::filesystem::transcripts::read_live_preamble_window(path, max_bytes)
}

pub(crate) fn visit_codex_rollout_candidates(
    day: &Path,
    visit: impl FnMut(PathBuf, std::time::SystemTime),
) {
    crate::filesystem::transcripts::visit_codex_rollout_candidates(day, visit)
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
    crate::filesystem::transcripts::find_pinned_jsonl(project_dir, transcript_id)
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
    crate::filesystem::transcripts::read_new_lines_with_starts(path, offset, remainder)
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
    crate::filesystem::transcripts::read_preamble_for_race(
        path,
        attach_time,
        extractor,
        PREAMBLE_MAX_BYTES,
        RACE_GRACE_SECS,
    )
}

#[cfg(test)]
mod tests;
