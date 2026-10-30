use super::*;
use crate::telegram::output::RecordingEvents;

// The extractor tests drive the kernel directly; production now reads
// through `read_new_lines_with_starts` (`#2232` phase 1), so the wrapper
// is imported here instead of by the module's production import list.
use crate::telegram::jsonl_kernel::read_new_lines;
use std::fs;
use std::io::Write;

mod capture;
mod extract;
mod find_session;
mod hot_attach;
mod kernel_paths;

const SESSION_META_TEMPLATE: &str = concat!(
    r#"{"timestamp":"{ts}","type":"session_meta","#,
    r#""payload":{"id":"{id}","cwd":"{cwd}","timestamp":"{ts}"}}"#,
);

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

/// Byte-exact sanitized copy of the real Codex 0.154.0 `final_answer`
/// record captured at rollout byte offset 87513 (room-shared
/// `codex-real-assistant-sanitized.jsonl`, SHA256
/// 76d5a755f02e62242c6b8ce7804a5ebf9322e66d14785d0109dba45dd6f70498).
const REAL_CURRENT_CODEX_FINAL: &str = concat!(
    r#"{"timestamp":"2026-09-13T18:28:25.938Z","ordinal":12,"type":"response_item","#,
    r#""payload":{"type":"message","#,
    r#""id":"msg_07770767cb7cb624016aa6eb4a72d487d2bcf644a579754670","role":"assistant","#,
    r#""content":[{"type":"output_text","text":"sanitized assistant reply"}],"#,
    r#""phase":"final_answer","internal_chat_message_metadata_passthrough":{"#,
    r#""turn_id":"01a09c07-0ad4-7710-9c1d-ce3f8dd0aaac","create_time":1789324104.823481,"#,
    r#""content_item_kinds":["unknown"]}}}"#,
);

// ── #2232 phase 4: the room-only reader (tests 14 and 17) ────────────

/// A `search_root` holding today's partition with one rollout whose
/// `session_meta.cwd` matches, plus one fresh `final_answer` record.
fn room_only_fixture(cwd: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let now = Utc::now();
    let today = tmp
        .path()
        .join(format!("{:04}", now.format("%Y")))
        .join(format!("{:02}", now.format("%m")))
        .join(format!("{:02}", now.format("%d")));
    let ts = now.to_rfc3339();
    let path = write_rollout(&today, "rollout-room-only.jsonl", cwd, &ts);
    let fresh_ts = (now - chrono::Duration::seconds(1)).to_rfc3339();
    let fresh = REAL_CURRENT_CODEX_FINAL.replace("2026-09-13T18:28:25.938Z", &fresh_ts);
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(f, "{}", fresh).unwrap();
    f.sync_all().unwrap();
    (tmp, path)
}

/// The three global diagnostic files `BridgeLogger::new` and
/// `DiagLogger::new` truncate (`telegram/output.rs:140`). `None` when this
/// process has no config dir, in which case the constructors write nothing
/// at all and the size check is trivially satisfied.
fn global_log_sizes() -> Vec<(PathBuf, u64)> {
    let Some(dir) = crate::config::config_dir() else {
        return Vec::new();
    };
    ["telegram-bridge.log", "diag-raw.log", "diag-sent.log"]
        .into_iter()
        .map(|name| {
            let path = dir.join(name);
            let len = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            (path, len)
        })
        .collect()
}

/// Test 14: **room-only Codex reader, no bot anywhere.** Records reach the
/// sink, no `BridgeLogger` is constructed, no HTTP request is attempted and
/// the global log files' sizes are unchanged. This is the executable form
/// of "never requires a bot".
#[tokio::test]
async fn a_room_only_codex_reader_emits_records_with_no_bot_anywhere() {
    // Serialized against every other test that touches the global logs.
    let _logs = lock_global_diagnostic_files().await;
    let work = tempfile::tempdir().unwrap();
    let cwd = work.path().to_string_lossy().replace('\\', "\\\\");
    let (_fixture, _path) = room_only_fixture(&cwd);
    let search_root = _fixture.path().to_path_buf();
    let before = global_log_sizes();

    let events = Arc::new(RecordingEvents::default());
    let network = OutboundNetwork::new_for_tests(1);
    let cancel = CancellationToken::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    // The supervisor always holds this sender open for the reader's
    // lifetime; a dropped sender means the reader is going away.
    let (dest_tx, dest_rx) = tokio::sync::watch::channel(None);

    let task = spawn_watch_task(
        search_root,
        work.path().to_string_lossy().to_string(),
        Utc::now(),
        network.clone(),
        dest_rx,
        "room-only-session".to_string(),
        cancel.clone(),
        events.clone(),
        Some(tx),
    );

    let record = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("a room-only reader must still deliver records")
        .expect("the sink stays open while the reader runs");
    assert_eq!(record.text, "sanitized assistant reply");
    assert_eq!(record.provider, CaptureProvider::Codex);
    assert_eq!(record.session_id, "room-only-session");

    cancel.cancel();
    drop(dest_tx);
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;

    assert!(
        network.acquired_labels_for_tests().is_empty(),
        "no HTTP request may be attempted without a bot target"
    );
    for (path, len) in before {
        let now = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        assert_eq!(
            now,
            len,
            "{} must not be truncated by a room-only reader",
            path.display()
        );
    }
}

/// Test 17: **no per-poll cost for a room without the flag.** A watcher run
/// with `sink == None` performs zero `.co-managed/` filesystem operations —
/// no lock acquisition, no `state.json` open — because the observation is
/// computed only when a sink is attached. This pins phase 3 section 6.1
/// from the watcher side.
#[tokio::test]
async fn a_watcher_without_a_sink_touches_no_co_managed_state() {
    // Serialized against every other test that touches the global logs.
    let _logs = lock_global_diagnostic_files().await;
    let work = tempfile::tempdir().unwrap();
    let cwd = work.path().to_string_lossy().replace('\\', "\\\\");
    let (_fixture, _path) = room_only_fixture(&cwd);
    let search_root = _fixture.path().to_path_buf();

    let events = Arc::new(RecordingEvents::default());
    let network = OutboundNetwork::new_for_tests(1);
    let cancel = CancellationToken::new();
    let (dest_tx, dest_rx) = tokio::sync::watch::channel(None);

    let task = spawn_watch_task(
        search_root,
        work.path().to_string_lossy().to_string(),
        Utc::now(),
        network.clone(),
        dest_rx,
        "no-sink-session".to_string(),
        cancel.clone(),
        events.clone(),
        None, // no sink: a room without the flag
    );

    // Several poll intervals, so the file is bound and read repeatedly.
    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS * 4)).await;
    cancel.cancel();
    drop(dest_tx);
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;

    for root in [work.path(), _fixture.path()] {
        let co_managed = root.join(".co-managed");
        assert!(
            !co_managed.exists(),
            "{} must not be created by a watcher with no sink",
            co_managed.display()
        );
        assert!(!co_managed.join("state.json").exists());
        assert!(!co_managed.join("state.lock").exists());
    }
    assert!(
        network.acquired_labels_for_tests().is_empty(),
        "no bot target, so no send is attempted"
    );
}

/// Payload of a valid current-format record.
fn valid_final_payload() -> serde_json::Value {
    serde_json::json!({
        "type": "message",
        "role": "assistant",
        "phase": "final_answer",
        "content": [{"type": "output_text", "text": "body"}]
    })
}

/// A valid current-format record with every field the extractor requires.
fn valid_final_value() -> serde_json::Value {
    serde_json::json!({
        "type": "response_item",
        "payload": valid_final_payload()
    })
}

/// Synthetic current-format record carrying the given `output_text` texts.
fn current_final_record(texts: &[&str]) -> String {
    let content: Vec<serde_json::Value> = texts
        .iter()
        .map(|t| serde_json::json!({"type": "output_text", "text": t}))
        .collect();
    let mut v = valid_final_value();
    v["payload"]["content"] = serde_json::Value::Array(content);
    v.to_string()
}

/// Synthetic current-format record with an explicit top-level `timestamp`;
/// `None` omits the field.
fn current_final_record_with_ts(text: &str, ts: Option<&str>) -> String {
    let mut v = valid_final_value();
    v["payload"]["content"] = serde_json::json!([{"type": "output_text", "text": text}]);
    if let Some(ts) = ts {
        v["timestamp"] = serde_json::json!(ts);
    }
    v.to_string()
}

/// Extract the assistant bodies of already-read JSONL lines, exactly as
/// the watcher's incremental and final-drain call sites do.
fn extract_bodies(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|l| extract_assistant_final(l.as_str()))
        .collect()
}

#[tokio::test]
async fn forced_read_error_preserves_session_text_and_absent_kind() {
    let work = tempfile::tempdir().unwrap();
    let cwd = work.path().to_string_lossy().replace('\\', "\\\\");
    let (fixture, path) = room_only_fixture(&cwd);
    let events = Arc::new(RecordingEvents::default());
    let cancel = CancellationToken::new();
    let (dest_tx, dest_rx) = tokio::sync::watch::channel(None);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let task = spawn_watch_task(
        fixture.path().to_path_buf(),
        work.path().to_string_lossy().to_string(),
        Utc::now(),
        OutboundNetwork::new_for_tests(1),
        dest_rx,
        "read-error-codex".to_owned(),
        cancel.clone(),
        events.clone(),
        Some(tx),
    );
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("reader attaches")
        .expect("capture proves selected transcript");
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[0xff, b'\n'])
        .unwrap();
    let expected_error = read_new_lines_with_starts(&path, &mut 0, &mut String::new()).unwrap_err();
    let observed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let errors = events.0.lock().unwrap().clone();
            if !errors.is_empty() {
                break errors;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    cancel.cancel();
    drop(dest_tx);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("reader cancels")
        .expect("reader joins");
    let expected = (
        "read-error-codex".to_owned(),
        format!("Codex JSONL read error: {}", expected_error),
        None,
    );
    assert!(observed
        .expect("real read branch emits error")
        .iter()
        .all(|e| e == &expected));
}
