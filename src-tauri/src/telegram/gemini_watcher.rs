// Gemini CLI session-file watcher.
//
// P1 pivot: Telegram bridging targets new Gemini sessions only — the legacy
// `.json` (full-rewrite) format is out-of-scope; users on
// `@google/gemini-cli < 0.42.0` see a one-time `telegram_bridge_warning`.
// New Gemini sessions write append-only `session-*.jsonl` via `appendFileSync`
// (verified against `@google/gemini-cli@0.42.0` bundle line 248711), so this
// watcher reuses Kernel A (offset-based) just like Claude and Codex.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use tauri::Emitter;
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use crate::commands::gemini_resolver::lookup_chats_dir_for_cwd;
use crate::network::OutboundNetwork;
use crate::telegram::bridge::{flush_buffer, BridgeLogger, DiagLogger};
use crate::telegram::jsonl_kernel::{
    read_new_lines, read_preamble_for_race, POLL_INTERVAL_MS, ROTATION_STALE_SECS,
};

/// Buffer thresholds tuned for Gemini's whole-turn-at-once cadence.
const FLUSH_DELAY_MS: u64 = 250;
const FLUSH_BYTES: usize = 1000;

#[allow(clippy::too_many_arguments)]
pub fn spawn_watch_task<R: tauri::Runtime>(
    gemini_home: PathBuf,
    cwd: String,
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
            gemini_home,
            cwd,
            attach_time,
            network,
            bot_token,
            chat_id,
            session_id.clone(),
            cancel,
            app.clone(),
        )
        .await;
        log::info!(
            "[GEMINI_EXIT] Watcher task ended for session {}",
            session_id
        );
    })
}

/// Extractor for `read_preamble_for_race`: returns
/// `(timestamp, Some(id), body)` for each `type:"gemini"` line. The kernel
/// returns the surfaced ids alongside the bodies in a single read, which the
/// watcher uses to seed `emitted_ids` directly — closing the M1 TOCTOU where
/// a separate seeder pass re-read the file and could mark a line's id as
/// emitted whose body landed BETWEEN the two reads (silent permanent drop).
fn gemini_preamble_extractor(line: &str) -> Option<(DateTime<Utc>, Option<String>, String)> {
    let (id, body) = extract_gemini_message(line)?;
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let ts_str = v.get("timestamp")?.as_str()?;
    let ts = DateTime::parse_from_rfc3339(ts_str)
        .ok()?
        .with_timezone(&Utc);
    Some((ts, Some(id), body))
}

/// Parse a Gemini `.jsonl` line and extract `(id, content)` for `type:"gemini"`
/// records with non-empty string content. Returns `None` for any other record
/// kind (session header, `$set`, `$rewindTo`, `type:"user"`, `type:"info"`,
/// etc.) and for empty/whitespace-only content (in-progress turns that have
/// only `thoughts[]` so far).
fn extract_gemini_message(line: &str) -> Option<(String, String)> {
    // Fast-path: skip lines that can't be a "gemini" type record.
    if !line.contains("\"type\":\"gemini\"") && !line.contains("\"type\": \"gemini\"") {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "gemini" {
        return None;
    }
    let id = v.get("id")?.as_str()?.to_string();
    let content = v.get("content")?.as_str()?.trim();
    if content.is_empty() {
        return None;
    }
    Some((id, content.to_string()))
}

/// Non-recursive scan of `chats_dir`: returns the newest file whose name
/// starts with `session-` and ends with `.jsonl`. Subagent files live at
/// `chats/<parentId>/<sessionId>.jsonl` (different naming and a different
/// depth), so they're excluded by both the prefix filter and the
/// non-recursive walk.
fn find_newest_session_jsonl(chats_dir: &Path) -> Option<PathBuf> {
    let read = std::fs::read_dir(chats_dir).ok()?;
    let mut best: Option<(PathBuf, SystemTime)> = None;
    for entry in read.flatten() {
        let path = entry.path();
        let fname = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !fname.starts_with("session-") || !fname.ends_with(".jsonl") {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                match &best {
                    Some((_, best_t)) if mtime > *best_t => best = Some((path, mtime)),
                    None => best = Some((path, mtime)),
                    _ => {}
                }
            }
        }
    }
    best.map(|(p, _)| p)
}

/// Returns true if `chats_dir` contains any file matching the given extension.
fn dir_has_extension(chats_dir: &Path, ext_without_dot: &str) -> bool {
    let Ok(read) = std::fs::read_dir(chats_dir) else {
        return false;
    };
    for entry in read.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with("session-") {
            continue;
        }
        let extension = path.extension().and_then(|e| e.to_str());
        if extension == Some(ext_without_dot) {
            return true;
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
async fn watch_loop<R: tauri::Runtime>(
    gemini_home: PathBuf,
    cwd: String,
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

    let mut chats_dir: Option<PathBuf> = None;
    let mut current_file: Option<PathBuf> = None;
    let mut current_file_mtime: Option<SystemTime> = None;
    let mut last_mtime_advance: Instant = Instant::now();
    let mut file_offset: u64 = 0;
    let mut line_remainder = String::new();
    let mut emitted_ids: HashSet<String> = HashSet::new();
    let mut chats_dir_warned = false;
    let mut chats_empty_warned = false;
    let mut legacy_json_warned = false;

    logger.log(
        "GEMINI_INIT",
        &session_id,
        &format!("gemini_home={} cwd={}", gemini_home.display(), cwd),
    );

    let mut poll_interval = tokio::time::interval(Duration::from_millis(POLL_INTERVAL_MS));
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = poll_interval.tick() => {
                // Step 1 (H1): resolve chats_dir lazily until projects.json has the cwd.
                if chats_dir.is_none() {
                    match lookup_chats_dir_for_cwd(&gemini_home, &cwd) {
                        Some(cd) => {
                            logger.log("GEMINI_DIR", &session_id,
                                &format!("resolved chats_dir={}", cd.display()));
                            chats_dir = Some(cd);
                        }
                        None => {
                            if !chats_dir_warned {
                                logger.log("GEMINI_WAIT_PROJECTS", &session_id,
                                    "cwd not yet in projects.json");
                                let _ = app.emit(
                                    "telegram_bridge_warning",
                                    serde_json::json!({
                                        "sessionId": session_id,
                                        "warning": "Gemini has not yet recorded this working directory. Telegram will start delivering messages once Gemini finishes its startup phase and writes projects.json.",
                                    }),
                                );
                                chats_dir_warned = true;
                            }
                            continue;
                        }
                    }
                }
                let chats_dir_ref = chats_dir.as_ref().expect("chats_dir bound above");

                // Step 2: chats_dir resolved but the directory may not exist on disk yet.
                if !chats_dir_ref.is_dir() {
                    if !chats_empty_warned {
                        logger.log("GEMINI_WAIT_CHATS", &session_id,
                            "chats dir does not exist yet");
                        let _ = app.emit(
                            "telegram_bridge_warning",
                            serde_json::json!({
                                "sessionId": session_id,
                                "warning": "Gemini's chats directory does not exist yet. Telegram will start delivering messages once Gemini writes its first session file.",
                            }),
                        );
                        chats_empty_warned = true;
                    }
                    continue;
                }

                // Step 3 (H3): warn once if only legacy .json files are present.
                let has_jsonl = dir_has_extension(chats_dir_ref, "jsonl");
                let has_json = dir_has_extension(chats_dir_ref, "json");
                if !has_jsonl && has_json && !legacy_json_warned {
                    logger.log("GEMINI_LEGACY_JSON", &session_id,
                        ".json only, no .jsonl (upgrade gemini-cli)");
                    let _ = app.emit(
                        "telegram_bridge_warning",
                        serde_json::json!({
                            "sessionId": session_id,
                            "warning": "Gemini is writing the legacy .json session format. Upgrade @google/gemini-cli to >= 0.42.0 for Telegram bridging support.",
                        }),
                    );
                    legacy_json_warned = true;
                    continue;
                }
                if !has_jsonl {
                    continue;
                }

                // Step 4: Kernel A discovery — pick newest session-*.jsonl by mtime.
                // M5: rescan only when we don't have a current file, the file
                // was unlinked, or its mtime has not advanced for
                // ROTATION_STALE_SECS wall-clock seconds. `last_mtime_advance`
                // is updated below whenever we observe the file's mtime grow.
                let need_rescan = match &current_file {
                    None => true,
                    Some(p) if !p.exists() => true,
                    Some(_) => last_mtime_advance.elapsed().as_secs() >= ROTATION_STALE_SECS,
                };

                if need_rescan {
                    if let Some(found) = find_newest_session_jsonl(chats_dir_ref) {
                        if Some(&found) != current_file.as_ref() {
                            let first_bind = current_file.is_none();
                            line_remainder.clear();
                            if first_bind {
                                // §J preamble scan on first bind. Re-emit recent
                                // assistant lines from the file's tail (timestamp
                                // >= attach_time - 5s). The kernel returns
                                // (bodies, ids, file_len) from a SINGLE read, so
                                // seeding emitted_ids from `ids` (no second read)
                                // is race-free: an id can't be marked emitted
                                // unless its body was surfaced in this same pass.
                                match read_preamble_for_race(&found, attach_time, gemini_preamble_extractor) {
                                    Ok((bodies, ids, file_len)) => {
                                        for id in ids.into_iter().flatten() {
                                            emitted_ids.insert(id);
                                        }
                                        for text in bodies {
                                            logger.log("GEMINI_PREAMBLE", &session_id, &text);
                                            buffer.push_str(&text);
                                            buffer.push('\n');
                                            last_buffer_add = Instant::now();
                                        }
                                        file_offset = file_len;
                                        logger.log("GEMINI_FILE", &session_id,
                                            &format!("bound to {}, preamble done, offset={}",
                                                found.display(), file_offset));
                                    }
                                    Err(e) => {
                                        logger.log("GEMINI_ERR", &session_id,
                                            &format!("preamble scan failed: {}", e));
                                        file_offset = std::fs::metadata(&found).ok().map(|m| m.len()).unwrap_or(0);
                                    }
                                }
                            } else {
                                // Rotation (mid-session /new). Clear dedup and re-anchor at EOF.
                                emitted_ids.clear();
                                file_offset = std::fs::metadata(&found).ok().map(|m| m.len()).unwrap_or(0);
                                logger.log("GEMINI_ROTATE", &session_id,
                                    &format!("rotated to {}", found.display()));
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
                    }
                }

                if let Some(ref path) = current_file {
                    match read_new_lines(path, &mut file_offset, &mut line_remainder) {
                        Ok(new_lines) => {
                            for line in new_lines {
                                if let Some((id, content)) = extract_gemini_message(&line) {
                                    if emitted_ids.insert(id) {
                                        logger.log("GEMINI_EXTRACT", &session_id, &content);
                                        buffer.push_str(&content);
                                        buffer.push('\n');
                                        last_buffer_add = Instant::now();
                                    }
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
                            logger.log("GEMINI_ERR", &session_id, &e.to_string());
                            log::error!("[GEMINI_ERR] Read error for session {}: {}", session_id, e);
                            let _ = app.emit(
                                "telegram_bridge_error",
                                serde_json::json!({
                                    "sessionId": session_id,
                                    "error": format!("Gemini JSONL read error: {}", e),
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
                if let Some((id, content)) = extract_gemini_message(&line) {
                    if emitted_ids.insert(id) {
                        buffer.push_str(&content);
                        buffer.push('\n');
                    }
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

    // ── extract_gemini_message ────────────────────────────────────────────

    #[test]
    fn extract_gemini_message_returns_id_and_content() {
        let line = r#"{"type":"gemini","id":"abc","content":"Hello","thoughts":[]}"#;
        assert_eq!(
            extract_gemini_message(line),
            Some(("abc".into(), "Hello".into()))
        );
    }

    #[test]
    fn extract_gemini_message_skips_empty_content() {
        let line = r#"{"type":"gemini","id":"abc","content":"","thoughts":[]}"#;
        assert_eq!(extract_gemini_message(line), None);
    }

    #[test]
    fn extract_gemini_message_skips_whitespace_only_content() {
        let line = r#"{"type":"gemini","id":"abc","content":"   ","thoughts":[]}"#;
        assert_eq!(extract_gemini_message(line), None);
    }

    #[test]
    fn extract_gemini_message_skips_user_records() {
        let line = r#"{"type":"user","id":"u1","content":[{"text":"hi"}]}"#;
        assert_eq!(extract_gemini_message(line), None);
    }

    #[test]
    fn extract_gemini_message_skips_set_record() {
        let line = r#"{"$set":{"updatedAt":"2026-05-19T00:00:00Z"}}"#;
        assert_eq!(extract_gemini_message(line), None);
    }

    #[test]
    fn extract_gemini_message_skips_rewind_record() {
        let line = r#"{"$rewindTo":"abc","timestamp":"2026-05-19T00:00:00Z"}"#;
        assert_eq!(extract_gemini_message(line), None);
    }

    #[test]
    fn extract_gemini_message_skips_session_header() {
        let line = r#"{"sessionId":"abc","projectHash":"deadbeef","startTime":"2026-05-19T00:00:00Z","kind":"main"}"#;
        assert_eq!(extract_gemini_message(line), None);
    }

    #[test]
    fn extract_gemini_message_skips_info_record() {
        let line = r#"{"type":"info","content":"some info"}"#;
        assert_eq!(extract_gemini_message(line), None);
    }

    // ── find_newest_session_jsonl ─────────────────────────────────────────

    #[test]
    fn find_newest_session_jsonl_picks_latest_mtime() {
        let tmp = tempfile::tempdir().unwrap();
        let chats = tmp.path();

        let a = chats.join("session-2026-05-01T00-00-aaaaaaaa.jsonl");
        fs::write(&a, b"{}").unwrap();
        // Windows NTFS mtime resolution can be as coarse as 100 ms; sleep
        // long enough that the two files definitely have distinct mtimes.
        std::thread::sleep(std::time::Duration::from_millis(150));
        let b = chats.join("session-2026-05-02T00-00-bbbbbbbb.jsonl");
        fs::write(&b, b"{}").unwrap();

        let found = find_newest_session_jsonl(chats);
        assert_eq!(found, Some(b));
    }

    #[test]
    fn find_newest_session_jsonl_skips_subagent_files() {
        // Subagent files live one level deeper under <parentId>/<sessionId>.jsonl.
        // The non-recursive scan must not descend.
        let tmp = tempfile::tempdir().unwrap();
        let chats = tmp.path();

        let main = chats.join("session-2026-05-02T00-00-main.jsonl");
        fs::write(&main, b"{}").unwrap();

        let subdir = chats.join("parent-uuid");
        fs::create_dir_all(&subdir).unwrap();
        let sub = subdir.join("sub-uuid.jsonl");
        fs::write(&sub, b"{}").unwrap();

        let found = find_newest_session_jsonl(chats);
        assert_eq!(found, Some(main));
    }

    #[test]
    fn find_newest_session_jsonl_skips_non_session_files() {
        let tmp = tempfile::tempdir().unwrap();
        let chats = tmp.path();
        fs::write(chats.join("not-a-session.jsonl"), b"{}").unwrap();
        fs::write(chats.join("session-2026-05-02T00-00-x.json"), b"{}").unwrap(); // .json not .jsonl
        let found = find_newest_session_jsonl(chats);
        assert!(found.is_none());
    }

    #[test]
    fn find_newest_session_jsonl_returns_none_when_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let found = find_newest_session_jsonl(tmp.path());
        assert!(found.is_none());
    }

    // ── dir_has_extension ─────────────────────────────────────────────────

    #[test]
    fn dir_has_extension_distinguishes_json_and_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let chats = tmp.path();
        fs::write(chats.join("session-old.json"), b"{}").unwrap();
        assert!(dir_has_extension(chats, "json"));
        assert!(!dir_has_extension(chats, "jsonl"));

        fs::write(chats.join("session-new.jsonl"), b"{}").unwrap();
        assert!(dir_has_extension(chats, "jsonl"));
    }

    #[test]
    fn dir_has_extension_ignores_non_session_files() {
        let tmp = tempfile::tempdir().unwrap();
        let chats = tmp.path();
        // A `.jsonl` file but NOT a session file shouldn't count.
        fs::write(chats.join("something.jsonl"), b"{}").unwrap();
        assert!(!dir_has_extension(chats, "jsonl"));
    }

    // ── dedup behavior (integration-ish via direct buffer/HashSet) ────────

    #[test]
    fn dedup_set_emits_each_id_once_across_reappends() {
        let mut emitted: HashSet<String> = HashSet::new();
        let mut out: Vec<String> = Vec::new();

        // First emit: non-empty content, new id.
        let l1 = r#"{"type":"gemini","id":"a","content":"first"}"#;
        if let Some((id, c)) = extract_gemini_message(l1) {
            if emitted.insert(id) {
                out.push(c);
            }
        }
        // Re-append with same id, same content — skip.
        if let Some((id, c)) = extract_gemini_message(l1) {
            if emitted.insert(id) {
                out.push(c);
            }
        }
        // In-progress empty-content pattern — never emits anyway.
        let l2 = r#"{"type":"gemini","id":"b","content":"","thoughts":[]}"#;
        if let Some((id, c)) = extract_gemini_message(l2) {
            if emitted.insert(id) {
                out.push(c);
            }
        }
        // Non-empty content for new id — emits once.
        let l3 = r#"{"type":"gemini","id":"c","content":"new"}"#;
        if let Some((id, c)) = extract_gemini_message(l3) {
            if emitted.insert(id) {
                out.push(c);
            }
        }

        assert_eq!(out, vec!["first".to_string(), "new".to_string()]);
    }

    // ── multi-line file integration via read_new_lines + extractor ────────

    #[test]
    fn extracts_only_non_empty_gemini_messages_from_real_fixture() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session-2026-05-19T00-00-fixture.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        // Session header (no `type`).
        writeln!(f, r#"{{"sessionId":"abc","projectHash":"x","startTime":"2026-05-19T00:00:00Z","kind":"main"}}"#).unwrap();
        // user message.
        writeln!(f, r#"{{"id":"u1","timestamp":"2026-05-19T00:00:01Z","type":"user","content":[{{"text":"hi"}}]}}"#).unwrap();
        // $set metadata record.
        writeln!(f, r#"{{"$set":{{"lastUpdated":"2026-05-19T00:00:02Z"}}}}"#).unwrap();
        // gemini in-progress (empty content, thoughts only).
        writeln!(f, r#"{{"id":"g1","timestamp":"2026-05-19T00:00:03Z","type":"gemini","content":"","thoughts":[{{"subject":"x"}}]}}"#).unwrap();
        // gemini final.
        writeln!(f, r#"{{"id":"g1","timestamp":"2026-05-19T00:00:04Z","type":"gemini","content":"Hello world"}}"#).unwrap();
        // gemini re-appended (same id) — must be deduped.
        writeln!(f, r#"{{"id":"g1","timestamp":"2026-05-19T00:00:05Z","type":"gemini","content":"Hello world"}}"#).unwrap();
        // $rewindTo record.
        writeln!(
            f,
            r#"{{"$rewindTo":"g1","timestamp":"2026-05-19T00:00:06Z"}}"#
        )
        .unwrap();
        drop(f);

        let mut offset: u64 = 0;
        let mut remainder = String::new();
        let lines = read_new_lines(&path, &mut offset, &mut remainder).unwrap();

        let mut emitted: HashSet<String> = HashSet::new();
        let mut out: Vec<String> = Vec::new();
        for line in lines {
            if let Some((id, c)) = extract_gemini_message(&line) {
                if emitted.insert(id) {
                    out.push(c);
                }
            }
        }
        assert_eq!(out, vec!["Hello world".to_string()]);
    }

    // ── M1 TOCTOU regression ──────────────────────────────────────────────

    #[test]
    fn gemini_first_attach_does_not_silently_drop_concurrent_append() {
        // M1 regression test. Old code had a TOCTOU on first attach:
        //   1. read_preamble_for_race read the file → surfaced bodies for X, Y.
        //   2. line Z appended to file.
        //   3. seed_emitted_ids_from_preamble re-read the file → marked X, Y,
        //      AND Z as already-emitted (Z's body was never surfaced).
        //   4. next read_new_lines saw Z → dedup said "emitted" → silent drop.
        //
        // Fix: the kernel returns (bodies, ids, file_len) from a SINGLE read,
        // so seeding emitted_ids from `ids` (no second file read) means an id
        // can only be marked emitted if its body was surfaced in the same pass.
        //
        // This test exercises the fixed flow: scan + seed, simulate an append
        // happening AFTER the kernel returned (i.e. what the old double-read
        // would have racily incorporated into the seeder), then verify the
        // appended line surfaces via the next read_new_lines.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session-2026-05-19T00-00-race.jsonl");
        let now = Utc::now();
        let fresh = now - chrono::Duration::seconds(1);

        // Initial state: 2 fresh gemini lines.
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"id":"a","timestamp":"{}","type":"gemini","content":"first"}}"#,
            fresh.to_rfc3339()
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"id":"b","timestamp":"{}","type":"gemini","content":"second"}}"#,
            fresh.to_rfc3339()
        )
        .unwrap();
        drop(f);

        // Single-read preamble scan: surfaces bodies AND ids in one pass.
        let (bodies, ids, file_len) =
            read_preamble_for_race(&path, now, gemini_preamble_extractor).unwrap();
        assert_eq!(
            bodies.len(),
            ids.len(),
            "kernel must return parallel bodies/ids arrays"
        );
        assert_eq!(bodies, vec!["first".to_string(), "second".to_string()]);

        // Seed emitted_ids ONLY from kernel's returned ids — no second file read.
        let mut emitted_ids: HashSet<String> = HashSet::new();
        for id in ids.into_iter().flatten() {
            emitted_ids.insert(id);
        }
        assert!(emitted_ids.contains("a"));
        assert!(emitted_ids.contains("b"));

        // Race: a third line lands AFTER the kernel returned. In the old
        // double-read this would have happened BETWEEN the two reads and the
        // seeder would have pre-marked "c" as emitted — silently dropping its
        // body on the next poll. With the single-read fix, "c" is invisible
        // to the seed step and must surface here.
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            f,
            r#"{{"id":"c","timestamp":"{}","type":"gemini","content":"third"}}"#,
            fresh.to_rfc3339()
        )
        .unwrap();
        drop(f);

        assert!(
            !emitted_ids.contains("c"),
            "M1 regression: id 'c' must NOT be pre-seeded after a single-read scan"
        );

        // Next poll picks up the new line cleanly.
        let mut file_offset = file_len;
        let mut line_remainder = String::new();
        let new_lines = read_new_lines(&path, &mut file_offset, &mut line_remainder).unwrap();
        let mut surfaced: Vec<String> = Vec::new();
        for line in new_lines {
            if let Some((id, c)) = extract_gemini_message(&line) {
                if emitted_ids.insert(id) {
                    surfaced.push(c);
                }
            }
        }
        assert_eq!(
            surfaced,
            vec!["third".to_string()],
            "concurrently-appended line must surface, not be silently dropped"
        );
    }
}
