// Codex CLI session-file watcher.
//
// Polls `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` files for `event_msg`
// records with `payload.type == "agent_message"` and sends the assistant prose
// to Telegram. Uses Kernel A (offset-based append-only JSONL) from
// `jsonl_kernel.rs` once `find_session_file` selects the right rollout.

use std::io::Read as IoRead;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use tauri::Emitter;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use crate::commands::codex_resolver::canonicalize_cwd_for_codex;
use crate::network::OutboundNetwork;
use crate::telegram::jsonl_kernel::{
    read_new_lines, read_preamble_for_race, POLL_INTERVAL_MS, ROTATION_STALE_SECS,
};
use crate::telegram::output::{flush_buffer, BridgeLogger, DiagLogger};

/// Buffer thresholds tuned for Codex's commentary cadence.
/// `event_msg + agent_message` events average 200-400 B and arrive 3-8 per
/// turn within 800-2500 ms. Coalesce a whole turn into one Telegram message.
const FLUSH_DELAY_MS: u64 = 1500;
const FLUSH_BYTES: usize = 3000;

/// Grace window for the M6 day-walk: files older than this aren't considered.
const FILE_MTIME_GRACE_SECS: i64 = 5 * 60;

/// M6 empirical (Codex 0.130.0): `codex resume --last` APPENDS to the prior
/// rollout file. The walk therefore covers today + last 7 UTC days to catch
/// resumes from week-old sessions. See plan §15 §A for the empirical record.
const DAY_WALK_DEPTH: i64 = 7;

#[allow(clippy::too_many_arguments)]
pub fn spawn_watch_task<R: tauri::Runtime>(
    search_root: PathBuf,
    expected_cwd: String,
    attach_time: DateTime<Utc>,
    network: OutboundNetwork,
    bot_token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        watch_loop(
            search_root,
            expected_cwd,
            attach_time,
            network,
            bot_token,
            chat_id,
            session_id.clone(),
            cancel,
            app.clone(),
        )
        .await;
        log::info!("[CODEX_EXIT] Watcher task ended for session {}", session_id);
    })
}

/// Extractor for `read_preamble_for_race`: pairs each emitted body with the
/// line's top-level `timestamp` field so the kernel can apply its grace-window
/// filter. Codex agent_message events have no stable per-line id we need for
/// dedup, so the id slot is always `None`.
fn codex_preamble_extractor(line: &str) -> Option<(DateTime<Utc>, Option<String>, String)> {
    let body = extract_agent_message(line)?;
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let ts_str = v.get("timestamp")?.as_str()?;
    let ts = DateTime::parse_from_rfc3339(ts_str)
        .ok()?
        .with_timezone(&Utc);
    Some((ts, None, body))
}

/// Parse a single Codex rollout JSONL line and extract the `agent_message`
/// body, if any. Returns `None` for any other event/payload type, or for
/// empty/whitespace-only messages.
fn extract_agent_message(line: &str) -> Option<String> {
    // Fast-path: skip lines that can't be agent_message events. Codex rollouts
    // contain many tool_call / response_item lines per assistant turn.
    if !line.contains("\"type\":\"event_msg\"") && !line.contains("\"type\": \"event_msg\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "event_msg" {
        return None;
    }
    let payload = v.get("payload")?;
    if payload.get("type")?.as_str()? != "agent_message" {
        return None;
    }
    let msg = payload.get("message")?.as_str()?;
    let trimmed = msg.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Open `path`, read up to 64 KiB from the start, return the first complete
/// JSON line. Used by `find_session_file` to inspect each candidate's
/// `session_meta` header without slurping the full rollout (a 2 MB file would
/// be ~30 ms of blocking I/O per candidate otherwise).
fn read_first_line(path: &Path) -> Option<String> {
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

/// Walk today + last 7 UTC days of `search_root`, open each candidate
/// `rollout-*.jsonl` whose mtime is recent enough, parse its first
/// `session_meta` line, and return the path whose `payload.cwd` (after
/// canonicalization) equals the canonicalized `expected_cwd`, preferring the
/// candidate with the newest **file mtime**.
///
/// **mtime, not `payload.timestamp`**: `session_meta.payload.timestamp` is the
/// session CREATION time and is NEVER updated on resume. M6 empirical (Codex
/// 0.130.0) shows `codex resume --last` appends to the original file from the
/// resumed session's creation day — so a 4-day-old file's `payload.timestamp`
/// is 4 days old even when it was just appended to. mtime tracks the
/// most-recently-written file, which is the live one Codex is appending to.
///
/// Returns `None` if no candidate matches; the watcher polls again next tick.
fn find_session_file(
    search_root: &Path,
    expected_cwd: &str,
    attach_time: DateTime<Utc>,
) -> Option<PathBuf> {
    let normalized_expected = canonicalize_cwd_for_codex(expected_cwd);
    let mtime_cutoff = attach_time - chrono::Duration::seconds(FILE_MTIME_GRACE_SECS);
    let mtime_cutoff_st: SystemTime = mtime_cutoff.into();

    let mut best: Option<(PathBuf, SystemTime)> = None;

    for offset in 0..=DAY_WALK_DEPTH {
        let day = attach_time.date_naive() - chrono::Duration::days(offset);
        let dir = search_root
            .join(format!("{:04}", day.format("%Y")))
            .join(format!("{:02}", day.format("%m")))
            .join(format!("{:02}", day.format("%d")));
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue, // partition missing — normal for days with no codex usage
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
            // Skip files modified more than 5 min before attach (stale).
            if mtime < mtime_cutoff_st {
                continue;
            }
            let first = match read_first_line(&path) {
                Some(l) => l,
                None => continue,
            };
            let v: serde_json::Value = match serde_json::from_str(&first) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let payload = match v.get("payload") {
                Some(p) => p,
                None => continue,
            };
            let candidate_cwd = match payload.get("cwd").and_then(|c| c.as_str()) {
                Some(c) => c,
                None => continue,
            };
            if canonicalize_cwd_for_codex(candidate_cwd) != normalized_expected {
                continue;
            }
            match &best {
                Some((_, best_mtime)) if mtime > *best_mtime => best = Some((path, mtime)),
                None => best = Some((path, mtime)),
                _ => {}
            }
        }
    }

    best.map(|(p, _)| p)
}

#[allow(clippy::too_many_arguments)]
async fn watch_loop<R: tauri::Runtime>(
    search_root: PathBuf,
    expected_cwd: String,
    attach_time: DateTime<Utc>,
    network: OutboundNetwork,
    token: String,
    chat_id: i64,
    session_id: String,
    cancel: CancellationToken,
    app: tauri::AppHandle<R>,
) {
    let mut logger = BridgeLogger::new(&session_id);
    let mut diag = DiagLogger::new();
    let mut buffer = String::new();
    let mut last_buffer_add = Instant::now();
    let flush_delay = Duration::from_millis(FLUSH_DELAY_MS);

    let mut current_file: Option<PathBuf> = None;
    let mut current_file_mtime: Option<SystemTime> = None;
    let mut last_mtime_advance: Instant = Instant::now();
    let mut file_offset: u64 = 0;
    let mut line_remainder = String::new();
    let mut search_warned = false;

    logger.log(
        "CODEX_INIT",
        &session_id,
        &format!(
            "search_root={} expected_cwd={}",
            search_root.display(),
            expected_cwd
        ),
    );

    let mut poll_interval = tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = poll_interval.tick() => {
                // M5: re-scan only when we don't have a current file, the tracked
                // file has been unlinked, or the file's mtime has not advanced
                // for ROTATION_STALE_SECS wall-clock seconds (i.e. it might have
                // been rotated out from under us). `last_mtime_advance` is
                // updated below when we observe the file's current mtime grow.
                let need_rescan = match &current_file {
                    None => true,
                    Some(p) if !p.exists() => true,
                    Some(_) => last_mtime_advance.elapsed().as_secs() >= ROTATION_STALE_SECS,
                };

                if need_rescan {
                    if let Some(found) = find_session_file(&search_root, &expected_cwd, attach_time) {
                        if Some(&found) != current_file.as_ref() {
                            // First bind OR rotation. On first bind, run the §J
                            // preamble scan to emit any agent_message from the file's
                            // tail with timestamp >= attach_time - 5s. Then set
                            // offset = file_len.
                            let first_bind = current_file.is_none();
                            line_remainder.clear();
                            if first_bind {
                                match read_preamble_for_race(&found, attach_time, codex_preamble_extractor) {
                                    Ok((bodies, _ids, file_len)) => {
                                        for text in bodies {
                                            logger.log("CODEX_PREAMBLE", &session_id, &text);
                                            buffer.push_str(&text);
                                            buffer.push('\n');
                                            last_buffer_add = Instant::now();
                                        }
                                        file_offset = file_len;
                                        logger.log("CODEX_FILE", &session_id,
                                            &format!("bound to {}, preamble done, offset={}", found.display(), file_offset));
                                    }
                                    Err(e) => {
                                        logger.log("CODEX_ERR", &session_id,
                                            &format!("preamble scan failed: {}", e));
                                        file_offset = std::fs::metadata(&found).ok().map(|m| m.len()).unwrap_or(0);
                                    }
                                }
                            } else {
                                // Rotation. Re-anchor at the new file's current EOF.
                                file_offset = std::fs::metadata(&found).ok().map(|m| m.len()).unwrap_or(0);
                                logger.log("CODEX_ROTATE", &session_id,
                                    &format!("rotated to {}, offset={}", found.display(), file_offset));
                            }
                            current_file = Some(found);
                        }
                        let new_mtime = current_file.as_ref()
                            .and_then(|p| std::fs::metadata(p).ok())
                            .and_then(|m| m.modified().ok());
                        if new_mtime != current_file_mtime {
                            last_mtime_advance = Instant::now();
                        }
                        current_file_mtime = new_mtime;
                    } else if !search_warned {
                        logger.log("CODEX_WAIT", &session_id,
                            "no rollout matching cwd found yet");
                        search_warned = true;
                    }
                }

                if let Some(ref path) = current_file {
                    match read_new_lines(path, &mut file_offset, &mut line_remainder) {
                        Ok(new_lines) => {
                            for line in new_lines {
                                if let Some(text) = extract_agent_message(&line) {
                                    logger.log("CODEX_EXTRACT", &session_id, &text);
                                    buffer.push_str(&text);
                                    buffer.push('\n');
                                    last_buffer_add = Instant::now();
                                }
                            }
                            let new_mtime = std::fs::metadata(path).ok()
                                .and_then(|m| m.modified().ok());
                            if new_mtime != current_file_mtime {
                                last_mtime_advance = Instant::now();
                            }
                            current_file_mtime = new_mtime;
                        }
                        Err(e) => {
                            logger.log("CODEX_ERR", &session_id, &e.to_string());
                            log::error!("[CODEX_ERR] Read error for session {}: {}", session_id, e);
                            let _ = app.emit(
                                "telegram_bridge_error",
                                serde_json::json!({
                                    "sessionId": session_id,
                                    "error": format!("Codex JSONL read error: {}", e),
                                }),
                            );
                        }
                    }
                }

                if !buffer.is_empty() {
                    let elapsed = last_buffer_add.elapsed();
                    if elapsed >= flush_delay || buffer.len() > FLUSH_BYTES {
                        flush_buffer(
                            &mut buffer, &network, &token, chat_id,
                            &session_id, &app, &mut logger, &mut diag,
                            true,
                        ).await;
                    }
                }
            }
        }
    }

    // Final poll + flush after cancel.
    if let Some(ref path) = current_file {
        if let Ok(new_lines) = read_new_lines(path, &mut file_offset, &mut line_remainder) {
            for line in new_lines {
                if let Some(text) = extract_agent_message(&line) {
                    buffer.push_str(&text);
                    buffer.push('\n');
                }
            }
        }
    }
    if !buffer.is_empty() {
        flush_buffer(
            &mut buffer,
            &network,
            &token,
            chat_id,
            &session_id,
            &app,
            &mut logger,
            &mut diag,
            true,
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    const SESSION_META_TEMPLATE: &str = r#"{"timestamp":"{ts}","type":"session_meta","payload":{"id":"{id}","cwd":"{cwd}","timestamp":"{ts}"}}"#;

    fn write_rollout(dir: &Path, name: &str, cwd: &str, ts: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        let line = SESSION_META_TEMPLATE
            .replace("{ts}", ts)
            .replace("{cwd}", cwd)
            .replace("{id}", "test-uuid");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "{}", line).unwrap();
        path
    }

    // ── extract_agent_message ─────────────────────────────────────────────

    #[test]
    fn extract_agent_message_from_real_event() {
        let line = r#"{"timestamp":"2026-05-19T05:00:00Z","type":"event_msg","payload":{"type":"agent_message","message":"  Hello from Codex.  ","phase":"commentary"}}"#;
        assert_eq!(
            extract_agent_message(line),
            Some("Hello from Codex.".into())
        );
    }

    #[test]
    fn extract_agent_message_skips_non_event_msg_types() {
        for kind in [
            "session_meta",
            "turn_context",
            "task_started",
            "response_item",
            "token_count",
        ] {
            let line = format!(r#"{{"type":"{}","payload":{{}}}}"#, kind);
            assert_eq!(extract_agent_message(&line), None, "kind={}", kind);
        }
    }

    #[test]
    fn extract_agent_message_skips_other_payload_types() {
        for ptype in [
            "reasoning",
            "function_call",
            "function_call_output",
            "input_text",
            "output_text",
        ] {
            let line = format!(
                r#"{{"type":"event_msg","payload":{{"type":"{}","message":"x"}}}}"#,
                ptype
            );
            assert_eq!(extract_agent_message(&line), None, "ptype={}", ptype);
        }
    }

    #[test]
    fn extract_agent_message_skips_empty_or_whitespace() {
        for msg in ["", "   ", "\n", "\t  \t"] {
            let line = format!(
                r#"{{"type":"event_msg","payload":{{"type":"agent_message","message":"{}"}}}}"#,
                msg.replace('\t', "\\t").replace('\n', "\\n")
            );
            assert_eq!(extract_agent_message(&line), None, "msg={:?}", msg);
        }
    }

    #[test]
    fn extract_agent_message_fast_path_rejects_unrelated_lines() {
        // Line shouldn't even contain `"type":"event_msg"` substring.
        assert_eq!(extract_agent_message(r#"{"type":"response_item"}"#), None);
        assert_eq!(extract_agent_message("not json at all"), None);
    }

    #[test]
    fn extract_agent_message_handles_phase_variants() {
        let commentary = r#"{"type":"event_msg","payload":{"type":"agent_message","message":"a","phase":"commentary"}}"#;
        let final_ = r#"{"type":"event_msg","payload":{"type":"agent_message","message":"b","phase":"final"}}"#;
        assert_eq!(extract_agent_message(commentary), Some("a".into()));
        assert_eq!(extract_agent_message(final_), Some("b".into()));
    }

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
        let expected_new =
            write_rollout(&today, "rollout-new.jsonl", &cwd.replace('\\', "\\\\"), &ts);

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
}
