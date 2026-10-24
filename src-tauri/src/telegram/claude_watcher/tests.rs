use super::*;

use std::io::Write;

/// A Claude fixture line with a timestamp, as the preamble scan needs.
fn stamped_line(text: &str, ts: DateTime<Utc>) -> String {
    serde_json::json!({
        "type": "assistant",
        "timestamp": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "message": {"content": [{"type": "text", "text": text}]}
    })
    .to_string()
}

struct Transcript {
    _dir: tempfile::TempDir,
    path: PathBuf,
    /// Absolute start offset of each written line, in order.
    starts: Vec<u64>,
    bodies: Vec<String>,
}

/// Five recent assistant turns, one with a multi-byte character and one
/// terminated by CRLF, so the byte arithmetic is exercised (test 4).
fn transcript(now: DateTime<Utc>) -> Transcript {
    let dir = tempfile::tempdir().expect("fixture dir");
    let path = dir.path().join("session.jsonl");
    let bodies = vec![
        "first".to_string(),
        "caf\u{e9} \u{2705} second".to_string(),
        "third".to_string(),
        "fourth".to_string(),
        "fifth".to_string(),
    ];
    let mut file = std::fs::File::create(&path).expect("fixture file");
    let mut starts = Vec::new();
    let mut offset = 0u64;
    for (i, body) in bodies.iter().enumerate() {
        let line = stamped_line(body, now - chrono::Duration::milliseconds(100));
        // Row 2 ends with CRLF; the rest with LF.
        let raw = if i == 1 {
            format!("{line}\r\n")
        } else {
            format!("{line}\n")
        };
        starts.push(offset);
        offset += raw.len() as u64;
        file.write_all(raw.as_bytes()).expect("write fixture line");
    }
    file.sync_all().expect("sync fixture");
    Transcript {
        _dir: dir,
        path,
        starts,
        bodies,
    }
}

/// What the normal poll loop sends to Telegram from `offset` onward.
fn loop_bodies_from(path: &Path, offset: u64) -> Vec<String> {
    let mut cursor = offset;
    let mut remainder = String::new();
    read_new_lines_with_starts(path, &mut cursor, &mut remainder)
        .expect("read fixture")
        .into_iter()
        .filter_map(|(_, line)| extract_assistant_text(&line))
        .collect()
}

/// Test 11 (Claude half; the Codex half lives in `codex_watcher`): with no
/// Bot demand no `JSONL_EXTRACT` line is written and **no global log file
/// is truncated**. Each file's size is asserted unchanged, not merely that
/// no line matched — `BridgeLogger::new` truncates on construction
/// (`telegram/output.rs:140`), so "not constructed" is the only safe state.
#[tokio::test]
async fn a_room_only_claude_reader_truncates_no_global_log_and_still_emits() {
    // Serialized against the Codex hot-attach test, which constructs the
    // loggers and writes to the same global files.
    let _logs = crate::telegram::codex_watcher::lock_global_diagnostic_files().await;
    let dir = tempfile::tempdir().expect("projects dir");
    let now = Utc::now();
    std::fs::write(
        dir.path().join("session.jsonl"),
        format!("{}\n", stamped_line("room-only body", now)),
    )
    .expect("write transcript");

    let before: Vec<(PathBuf, u64)> = match crate::config::config_dir() {
        Some(config) => ["telegram-bridge.log", "diag-raw.log", "diag-sent.log"]
            .into_iter()
            .map(|name| {
                let path = config.join(name);
                let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                (path, len)
            })
            .collect(),
        None => Vec::new(),
    };

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build claude watcher test app");
    let network = crate::network::OutboundNetwork::new_for_tests(1);
    let cancel = CancellationToken::new();
    let (dest_tx, dest_rx) = tokio::sync::watch::channel(None);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let task = spawn_watch_task(
        dir.path().to_path_buf(),
        None,
        network.clone(),
        dest_rx,
        "room-only-claude".to_string(),
        cancel.clone(),
        app.handle().clone(),
        Some(tx),
    );

    let record = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("a room-only reader must still deliver records")
        .expect("the sink stays open while the reader runs");
    assert_eq!(record.text, "room-only body");

    cancel.cancel();
    drop(dest_tx);
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;

    assert!(
        network.acquired_labels_for_tests().is_empty(),
        "no HTTP request may be attempted without a bot demand"
    );
    for (path, len) in before {
        let after = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        assert_eq!(
            after,
            len,
            "{} must not be truncated by a room-only reader",
            path.display()
        );
    }
}

/// Test 4: preamble line starts index back to the exact raw bytes,
/// including a line containing a multi-byte character and a line ending in
/// CRLF. Starts are computed on the raw bytes plus the window offset, never
/// on the decoded text.
#[test]
fn preamble_line_starts_index_back_to_the_exact_raw_bytes() {
    let now = Utc::now();
    let fixture = transcript(now);
    let raw = std::fs::read(&fixture.path).expect("read fixture bytes");

    let lines = read_preamble_with_starts(&fixture.path, now).expect("preamble scan");

    assert_eq!(lines.len(), fixture.bodies.len());
    for ((start, body), (expected_start, expected_body)) in lines
        .iter()
        .zip(fixture.starts.iter().zip(fixture.bodies.iter()))
    {
        assert_eq!(start, expected_start, "body={body}");
        // The start walks back to the exact bytes in the file.
        let tail = &raw[*start as usize..];
        let end = tail
            .iter()
            .position(|&b| b == b'\n')
            .expect("every fixture line is terminated");
        let re_read = String::from_utf8(tail[..end].to_vec())
            .expect("fixture lines are valid UTF-8")
            .trim_end_matches('\r')
            .to_string();
        assert_eq!(
            extract_assistant_text(&re_read).as_deref(),
            Some(expected_body.as_str())
        );
        assert_eq!(body, expected_body);
    }
}

/// Tests 1 and 2: attaching over a **live** reader and a **cold** attach
/// produce the same chat content over the same fixture transcript, and no
/// duplicate line reaches Telegram when the preamble runs over a live
/// reader. This is the parity hypothesis of `epic.md` 9.6.
#[test]
fn a_live_attach_and_a_cold_attach_send_the_same_chat_content_exactly_once() {
    let now = Utc::now();
    let fixture = transcript(now);

    // Cold attach: the §J scan emits the whole recent tail, then the reader
    // continues from EOF, which has nothing more to send.
    let (cold_preamble, _ids, cold_offset) =
        read_preamble_for_race(&fixture.path, now, claude_preamble_extractor)
            .expect("cold preamble");
    let cold_chat: Vec<String> = cold_preamble
        .into_iter()
        .chain(loop_bodies_from(&fixture.path, cold_offset))
        .collect();

    // Live attach: the reader is already running and has consumed the first
    // three turns, so it sits at the start of the fourth.
    let reader_offset = fixture.starts[3];
    let mut buffer = String::new();
    let mut current_dest = None;
    let live_preamble = apply_destination_change(
        &mut buffer,
        &mut current_dest,
        Some(BotTarget {
            token: "token".into(),
            chat_id: 7,
        }),
        Some(&fixture.path),
        reader_offset,
        now,
    );
    let live_chat: Vec<String> = live_preamble
        .clone()
        .into_iter()
        .chain(loop_bodies_from(&fixture.path, reader_offset))
        .collect();

    assert_eq!(cold_chat, fixture.bodies, "the fixture pins the cold case");
    assert_eq!(live_chat, cold_chat, "a live attach must match a cold one");

    // No duplicate: the preamble stops exactly where the reader stands.
    assert_eq!(live_preamble, fixture.bodies[..3].to_vec());
    let mut seen = live_chat.clone();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), live_chat.len(), "no line is sent twice");
}

/// Test 3: the pending buffer is **not** flushed to the new destination
/// when the destination switches — it is discarded first, before the
/// destination is switched and before the preamble runs.
#[test]
fn the_pending_buffer_is_discarded_and_never_reaches_the_new_destination() {
    let now = Utc::now();
    let fixture = transcript(now);
    let mut buffer = String::from("pre-attach text that must never be sent\n");
    let mut current_dest = None;

    let preamble = apply_destination_change(
        &mut buffer,
        &mut current_dest,
        Some(BotTarget {
            token: "token".into(),
            chat_id: 7,
        }),
        Some(&fixture.path),
        fixture.starts[3],
        now,
    );

    assert!(buffer.is_empty(), "the pending buffer must be discarded");
    assert!(
        !preamble.iter().any(|body| body.contains("pre-attach")),
        "pre-attach text must not reappear through the preamble"
    );
    assert_eq!(current_dest.map(|d| d.chat_id), Some(7));
}

/// Test 13: a live-attach preamble delivers **zero** records to the sink; a
/// cold-attach preamble delivers them marked `Preamble`.
#[test]
fn a_live_attach_preamble_reaches_telegram_only_and_a_cold_one_marks_the_sink() {
    let now = Utc::now();
    let fixture = transcript(now);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // Live attach: the transition function has no sink parameter at all, so
    // no record can reach one.
    let mut buffer = String::new();
    let mut current_dest = None;
    let live = apply_destination_change(
        &mut buffer,
        &mut current_dest,
        Some(BotTarget {
            token: "token".into(),
            chat_id: 7,
        }),
        Some(&fixture.path),
        fixture.starts[3],
        now,
    );
    assert!(!live.is_empty(), "the live attach does emit to Telegram");
    assert!(
        rx.try_recv().is_err(),
        "a live-attach preamble delivers zero records to the sink"
    );

    // Cold attach: the §J bodies are delivered, marked `Preamble`.
    let (cold_bodies, _ids, _len) =
        read_preamble_for_race(&fixture.path, now, claude_preamble_extractor)
            .expect("cold preamble");
    let mut reader_seq = 0u64;
    let records = capture_preamble_bodies(
        cold_bodies,
        "session",
        &fixture.path,
        &mut reader_seq,
        Some(&tx),
        &ReaderAttachment::default(),
        RecordOrigin::Preamble,
    );
    assert_eq!(records.len(), fixture.bodies.len());
    for _ in 0..records.len() {
        let record = rx.try_recv().expect("cold preamble reaches the sink");
        assert_eq!(record.origin, RecordOrigin::Preamble);
    }
}

/// A Claude fixture line carrying one assistant `text` block.
fn assistant_line(text: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "message": {"content": [{"type": "text", "text": text}]}
    })
    .to_string()
}

fn capture_one(line: &str) -> Vec<Arc<CapturedRecord>> {
    let mut reader_seq = 0u64;
    capture_live_lines(
        vec![(0, line.to_string())],
        "claude-session",
        Path::new("session.jsonl"),
        &mut reader_seq,
        None,
        RecordOrigin::Live,
        &ReaderAttachment::default(),
    )
}

#[test]
fn assistant_records_carry_claudes_permanent_bits() {
    // Test 6: Claude yields `provider_final == false`, `turn_identified ==
    // false`, `turn_id == None`. Those bits are the permanent shape
    // (`epic.md` 3.2/3.3); no phase raises them.
    let records = capture_one(&assistant_line("hello"));
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.text, "hello");
    assert_eq!(record.turn_id, None);
    assert!(!record.provider_final);
    assert!(!record.turn_identified);
    assert_eq!(record.provider, CaptureProvider::Claude);
    assert_eq!(record.origin, RecordOrigin::Live);
    assert_eq!(record.record_start, Some(0));
    assert_eq!(record.reader_seq, 0);
    assert_eq!(record.epoch, 0);
    assert_eq!(record.file, PathBuf::from("session.jsonl"));
    assert_eq!(record.session_id, "claude-session");
}

#[test]
fn tool_use_and_thinking_blocks_still_yield_nothing() {
    // Test 6: the extractor's whitelist is unchanged by the capture layer.
    let tool_use = serde_json::json!({
        "type": "assistant",
        "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {}}]}
    })
    .to_string();
    let thinking = serde_json::json!({
        "type": "assistant",
        "message": {"content": [{"type": "thinking", "thinking": "why"}]}
    })
    .to_string();
    for line in [tool_use, thinking] {
        assert!(capture_one(&line).is_empty(), "line={line}");
    }
}

#[test]
fn text_sha256_covers_the_utf8_bytes() {
    // Test 7: the digest is over `text.as_bytes()`, not over characters.
    let text = "café ✅";
    let records = capture_one(&assistant_line(text));
    assert_eq!(records.len(), 1);
    let expected: [u8; 32] = Sha256::digest(text.as_bytes()).into();
    assert_eq!(records[0].text_sha256, expected);
}

#[test]
fn sidechain_assistant_records_are_captured_today() {
    // Test 9 — sub-agent residual, pinned not fixed (`epic.md` 9.7):
    // `claude_watcher` has no `isSidechain` filter (0 hits) and follows
    // `find_latest_jsonl`, so a sidechain assistant record is captured.
    // Narrowing this is not in #2264 scope; this test makes a later
    // narrowing a deliberate, visible change.
    let line = serde_json::json!({
        "type": "assistant",
        "isSidechain": true,
        "message": {"content": [{"type": "text", "text": "sidechain body"}]}
    })
    .to_string();
    let records = capture_one(&line);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].text, "sidechain body");
}

// ── #2454: the pinned first attach ──

const PIN_ID: &str = "16045f91-7d59-4c75-acdb-11ebfcb9aa68";

fn inputs<'a>(
    transcript_id: Option<&'a str>,
    pinned: Option<&'a Path>,
    newest: Option<&'a Path>,
    first_poll: bool,
    deadline_expired: bool,
) -> AttachInputs<'a> {
    AttachInputs {
        transcript_id,
        pinned,
        newest,
        first_poll,
        deadline_expired,
    }
}

// Test 10.
#[test]
fn decide_attach_waits_when_the_pinned_file_is_absent_at_the_first_poll() {
    let old = Path::new("old.jsonl");
    assert_eq!(
        decide_attach(inputs(Some(PIN_ID), None, Some(old), true, false)),
        AttachDecision::Wait
    );
    assert_eq!(
        decide_attach(inputs(Some(PIN_ID), None, Some(old), false, false)),
        AttachDecision::Wait
    );
}

// Test 11.
#[test]
fn decide_attach_pins_when_the_awaited_file_appears() {
    let pinned = PathBuf::from(format!("{PIN_ID}.jsonl"));
    for newest in [Some(pinned.as_path()), Some(Path::new("old.jsonl"))] {
        assert_eq!(
            decide_attach(inputs(Some(PIN_ID), Some(&pinned), newest, false, false)),
            AttachDecision::Pinned(pinned.clone())
        );
    }
}

// Test 12: late or resumed history, the pinned file already newest.
#[test]
fn decide_attach_takes_mtime_when_the_pinned_file_already_exists_and_is_newest() {
    let pinned = PathBuf::from(format!("{PIN_ID}.jsonl"));
    assert_eq!(
        decide_attach(inputs(
            Some(PIN_ID),
            Some(&pinned),
            Some(&pinned),
            true,
            false
        )),
        AttachDecision::Mtime(pinned.clone())
    );
}

// Test 13: the post-`/clear` late attach, the pinned file not newest.
#[test]
fn decide_attach_takes_mtime_when_the_pinned_file_already_exists_and_is_not_newest() {
    let pinned = PathBuf::from(format!("{PIN_ID}.jsonl"));
    let newer = Path::new("after-clear.jsonl");
    assert_eq!(
        decide_attach(inputs(
            Some(PIN_ID),
            Some(&pinned),
            Some(newer),
            true,
            false
        )),
        AttachDecision::Mtime(newer.to_path_buf())
    );
}

// Test 14.
#[test]
fn decide_attach_without_an_id_is_mtime_for_every_input() {
    let pinned = PathBuf::from(format!("{PIN_ID}.jsonl"));
    let newest = Path::new("newest.jsonl");
    for pinned in [None, Some(pinned.as_path())] {
        for first_poll in [true, false] {
            for deadline_expired in [true, false] {
                assert_eq!(
                    decide_attach(inputs(
                        None,
                        pinned,
                        Some(newest),
                        first_poll,
                        deadline_expired
                    )),
                    AttachDecision::Mtime(newest.to_path_buf())
                );
                assert_eq!(
                    decide_attach(inputs(None, pinned, None, first_poll, deadline_expired)),
                    AttachDecision::Nothing
                );
            }
        }
    }
}

// Test 15.
#[test]
fn decide_attach_falls_back_to_mtime_when_the_deadline_expires() {
    let old = Path::new("old.jsonl");
    assert_eq!(
        decide_attach(inputs(Some(PIN_ID), None, Some(old), false, true)),
        AttachDecision::MtimeFallback(old.to_path_buf())
    );
    assert_eq!(
        decide_attach(inputs(Some(PIN_ID), None, None, false, true)),
        AttachDecision::Nothing
    );
}

/// A reader over a fixture projects dir, with a live sink.
struct Reader {
    _app: tauri::App<tauri::test::MockRuntime>,
    _dest_tx: tokio::sync::watch::Sender<Option<BotTarget>>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    rx: tokio::sync::mpsc::UnboundedReceiver<Arc<CapturedRecord>>,
}

impl Reader {
    fn start(project_dir: &Path, transcript_id: Option<String>) -> Self {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("build claude watcher test app");
        let (dest_tx, dest_rx) = tokio::sync::watch::channel(None);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let task = spawn_watch_task(
            project_dir.to_path_buf(),
            transcript_id,
            crate::network::OutboundNetwork::new_for_tests(1),
            dest_rx,
            "pin-session".to_string(),
            cancel.clone(),
            app.handle().clone(),
            Some(tx),
        );
        Self {
            _app: app,
            _dest_tx: dest_tx,
            cancel,
            task,
            rx,
        }
    }

    async fn next(&mut self, within: Duration) -> Option<Arc<CapturedRecord>> {
        tokio::time::timeout(within, self.rx.recv())
            .await
            .ok()
            .flatten()
    }

    async fn stop(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.task).await;
    }
}

/// Write `content` to `path` whole, through a rename, so the reader never
/// sees a half-written first line.
fn write_atomically(path: &Path, content: &str) {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content).expect("write fixture");
    std::fs::rename(&tmp, path).expect("publish fixture");
}

fn backdate(path: &Path, secs: u64) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open fixture");
    file.set_modified(SystemTime::now() - std::time::Duration::from_secs(secs))
        .expect("backdate fixture");
}

/// Commit `record` as an `Automatic` effect and return `routable`, the
/// routing answer of `capture::state::commit_effect`.
fn routable(room: &Path, record: &Arc<CapturedRecord>) -> bool {
    commit_automatic(room, record)
        .expect("the commit takes the record")
        .routable
}

/// [`routable`] for a record that may be refused outright: a refusal such
/// as `AlreadyConsumed` routes nothing.
fn routes(room: &Path, record: &Arc<CapturedRecord>) -> bool {
    commit_automatic(room, record).is_ok_and(|effect| effect.routable)
}

fn commit_automatic(
    room: &Path,
    record: &Arc<CapturedRecord>,
) -> Result<crate::capture::state::CommittedEffect, crate::capture::state::AbstainReason> {
    use crate::capture::key::ConsumptionKey;
    use crate::capture::state::{commit_effect, EffectKind, EffectPreconditions};
    let slot = crate::capture::sink::CaptureSlot::new();
    slot.offer(Arc::clone(record), 0);
    let key = ConsumptionKey::from_record(record, 0);
    let pre = EffectPreconditions {
        session_alive: true,
        session_id: record.session_id.clone(),
        unique_live_session_for_cwd: true,
        no_pending_user_input: true,
        effective_ready: true,
        observed_at: std::time::Instant::now(),
        kind: EffectKind::Automatic,
    };
    commit_effect(room, &slot, slot.seq(), &key, &pre)
}

fn room(temp: &tempfile::TempDir) -> PathBuf {
    let room = temp.path().join("room-1-dev-team");
    std::fs::create_dir_all(&room).expect("room dir");
    crate::capture::state::forget_room_for_tests(&room);
    room
}

/// Test 16, the reported defect: `<id>.jsonl` is absent at the first poll,
/// so the reader waits; when it appears its first reply is `Live` and is
/// actually routed.
#[tokio::test]
async fn a_fresh_spawn_first_reply_is_live_and_routed() {
    let temp = tempfile::tempdir().expect("temp");
    let projects = temp.path().join("projects");
    std::fs::create_dir_all(&projects).expect("projects dir");
    let old = projects.join("old-session.jsonl");
    std::fs::write(&old, format!("{}\n", stamped_line("old reply", Utc::now())))
        .expect("old transcript");

    let mut reader = Reader::start(&projects, Some(PIN_ID.to_string()));
    // The reader waits: nothing from the old transcript reaches the sink.
    assert!(reader.next(Duration::from_millis(1200)).await.is_none());

    let pinned = projects.join(format!("{PIN_ID}.jsonl"));
    write_atomically(
        &pinned,
        &format!("{}\n", stamped_line("first reply", Utc::now())),
    );
    let record = reader
        .next(Duration::from_secs(10))
        .await
        .expect("the first reply reaches the sink");
    reader.stop().await;

    assert_eq!(record.text, "first reply");
    assert_eq!(record.file, pinned);
    assert_eq!(record.origin, RecordOrigin::Live);
    assert!(
        !crate::capture::state::is_baseline(&record),
        "supporting evidence only"
    );
    assert!(
        routable(&room(&temp), &record),
        "the first reply must be routed"
    );
}

/// Test 16b (guard): `<id>.jsonl` already exists and is the newest, and
/// ends with a reply inside the 5 s window that a previous reader already
/// consumed. A new reader stamps it `Preamble` and never routes it again.
#[tokio::test]
async fn an_existing_pinned_file_reply_is_not_routed_twice() {
    let temp = tempfile::tempdir().expect("temp");
    let room = room(&temp);
    let projects = temp.path().join("projects");
    std::fs::create_dir_all(&projects).expect("projects dir");
    let old = projects.join("old-session.jsonl");
    std::fs::write(&old, format!("{}\n", stamped_line("old reply", Utc::now())))
        .expect("old transcript");
    backdate(&old, 60);
    let pinned = projects.join(format!("{PIN_ID}.jsonl"));
    let line = stamped_line("consumed reply", Utc::now());
    std::fs::write(&pinned, format!("{line}\n")).expect("pinned transcript");

    // A previous reader consumed the reply through an incremental read.
    let mut reader_seq = 0u64;
    let earlier = capture_live_lines(
        vec![(0, line)],
        "pin-session",
        &pinned,
        &mut reader_seq,
        None,
        RecordOrigin::Live,
        &ReaderAttachment::default(),
    );
    assert!(routable(&room, &earlier[0]), "the first delivery is routed");

    let mut reader = Reader::start(&projects, Some(PIN_ID.to_string()));
    let record = reader
        .next(Duration::from_secs(10))
        .await
        .expect("the preamble reaches the sink");
    reader.stop().await;

    assert_eq!(record.text, "consumed reply");
    assert_eq!(record.file, pinned);
    assert_eq!(record.origin, RecordOrigin::Preamble);
    assert!(crate::capture::state::is_baseline(&record));
    assert!(
        !routable(&room, &record),
        "a consumed reply must not route twice"
    );
}

/// Test 17 (guard): an attach by mtime over genuine history still stamps
/// `Preamble` and is still suppressed.
#[tokio::test]
async fn an_mtime_first_attach_over_history_is_still_suppressed() {
    let temp = tempfile::tempdir().expect("temp");
    let projects = temp.path().join("projects");
    std::fs::create_dir_all(&projects).expect("projects dir");
    let history = projects.join("history.jsonl");
    std::fs::write(
        &history,
        format!("{}\n", stamped_line("history reply", Utc::now())),
    )
    .expect("history transcript");

    let mut reader = Reader::start(&projects, None);
    let record = reader
        .next(Duration::from_secs(10))
        .await
        .expect("the preamble reaches the sink");
    reader.stop().await;

    assert_eq!(record.file, history);
    assert_eq!(record.origin, RecordOrigin::Preamble);
    assert!(
        !routable(&room(&temp), &record),
        "history must stay suppressed"
    );
}

/// Test 18 (guard): an unextractable id changes nothing. Asserted as
/// absolute values for each shape: the newest file by mtime, `Preamble`
/// stamps, and the offset at that file's length.
#[tokio::test]
async fn an_unextractable_id_attaches_by_mtime_and_reads_to_the_end() {
    for shape in [vec!["--continue".to_string()], Vec::new()] {
        let transcript_id = crate::commands::session::transcript_id_from_args(&shape);
        let temp = tempfile::tempdir().expect("temp");
        let projects = temp.path().join("projects");
        std::fs::create_dir_all(&projects).expect("projects dir");
        let older = projects.join("older.jsonl");
        std::fs::write(
            &older,
            format!("{}\n", stamped_line("older reply", Utc::now())),
        )
        .expect("older transcript");
        backdate(&older, 60);
        let newest = projects.join("newest.jsonl");
        std::fs::write(
            &newest,
            format!(
                "{}\n{}\n",
                stamped_line("newest one", Utc::now()),
                stamped_line("newest two", Utc::now())
            ),
        )
        .expect("newest transcript");
        let newest_len = std::fs::metadata(&newest).expect("newest len").len();

        let mut reader = Reader::start(&projects, transcript_id);
        let mut records = Vec::new();
        while records.len() < 2 {
            match reader.next(Duration::from_secs(10)).await {
                Some(record) => records.push(record),
                None => break,
            }
        }
        reader.stop().await;

        assert_eq!(records.len(), 2, "shape={shape:?}");
        for record in records {
            assert_eq!(record.file, newest, "shape={shape:?}");
            assert_eq!(record.origin, RecordOrigin::Preamble, "shape={shape:?}");
            assert_eq!(record.observed_len, newest_len, "shape={shape:?}");
        }
    }
}

/// Test 19 (guard): after the pinned first attach, a `/clear` rotation is
/// still picked up by newest mtime.
#[tokio::test]
async fn a_clear_after_the_pinned_attach_is_still_a_rotation() {
    let temp = tempfile::tempdir().expect("temp");
    let projects = temp.path().join("projects");
    std::fs::create_dir_all(&projects).expect("projects dir");

    let mut reader = Reader::start(&projects, Some(PIN_ID.to_string()));
    tokio::time::sleep(Duration::from_millis(700)).await;
    let pinned = projects.join(format!("{PIN_ID}.jsonl"));
    write_atomically(
        &pinned,
        &format!("{}\n", stamped_line("first reply", Utc::now())),
    );
    let first = reader
        .next(Duration::from_secs(10))
        .await
        .expect("the pinned reply reaches the sink");
    assert_eq!(first.file, pinned);

    // The rotation stale guard reads wall-clock mtimes.
    backdate(&pinned, 60);
    tokio::time::sleep(Duration::from_millis(700)).await;
    let cleared = projects.join("after-clear.jsonl");
    write_atomically(
        &cleared,
        &format!("{}\n", stamped_line("after clear", Utc::now())),
    );
    let rotated = reader
        .next(Duration::from_secs(10))
        .await
        .expect("the rotated transcript is read");
    reader.stop().await;

    assert_eq!(rotated.file, cleared);
    assert_eq!(rotated.origin, RecordOrigin::RotationBackfill);
}

/// Test 24, pure (#2454 p3b requirement 4): the decision runs only while
/// the first-attach latch is clear and no file is attached.
#[test]
fn runs_first_attach_decision_only_before_the_first_attach() {
    assert!(runs_first_attach_decision(false, false));
    assert!(!runs_first_attach_decision(false, true));
    assert!(!runs_first_attach_decision(true, false));
    assert!(!runs_first_attach_decision(true, true));
}

/// Test 24 (#2454 p3b requirement 4): the pinned transcript vanishes after
/// the first attach and comes back with the same bytes. The reader must
/// not re-run the pin decision: nothing restored is offered `Live` and
/// nothing restored routes a second time.
#[tokio::test]
async fn a_vanished_pinned_transcript_is_not_pinned_again() {
    let temp = tempfile::tempdir().expect("temp");
    let room = room(&temp);
    let projects = temp.path().join("projects");
    std::fs::create_dir_all(&projects).expect("projects dir");

    let mut reader = Reader::start(&projects, Some(PIN_ID.to_string()));
    tokio::time::sleep(Duration::from_millis(700)).await;
    let pinned = projects.join(format!("{PIN_ID}.jsonl"));
    let content = format!("{}\n", stamped_line("first reply", Utc::now()));
    write_atomically(&pinned, &content);
    let first = reader
        .next(Duration::from_secs(10))
        .await
        .expect("the pinned reply reaches the sink");
    assert_eq!(first.file, pinned);
    assert_eq!(first.origin, RecordOrigin::Live);
    assert!(routable(&room, &first), "the first delivery is routed");

    // One poll picks up the backdated mtime, so the file is stale.
    backdate(&pinned, 60);
    tokio::time::sleep(Duration::from_millis(700)).await;
    // One poll with no `.jsonl` at all clears the current file.
    std::fs::remove_file(&pinned).expect("hide the transcript");
    tokio::time::sleep(Duration::from_millis(700)).await;
    write_atomically(&pinned, &content);

    let mut restored = Vec::new();
    while let Some(record) = reader.next(Duration::from_millis(1500)).await {
        restored.push(record);
    }
    reader.stop().await;

    // Positive control: the reader re-attached and re-offered the tail,
    // so the checks below cannot pass on a dead reader.
    assert!(
        !restored.is_empty(),
        "the restored transcript is re-attached"
    );
    for record in &restored {
        assert_ne!(
            record.origin,
            RecordOrigin::Live,
            "a restored transcript must not be pinned again: {:?}",
            record.text
        );
        assert!(
            !routes(&room, record),
            "a restored reply must not route twice: {:?}",
            record.text
        );
    }
}

// Test 20: the fallback fires only AFTER the deadline. Same inputs, the
// deadline live then expired.
#[test]
fn decide_attach_falls_back_only_after_the_deadline() {
    let old = Path::new("old.jsonl");
    assert_eq!(
        decide_attach(inputs(Some(PIN_ID), None, Some(old), false, false)),
        AttachDecision::Wait
    );
    assert_eq!(
        decide_attach(inputs(Some(PIN_ID), None, Some(old), false, true)),
        AttachDecision::MtimeFallback(old.to_path_buf())
    );
}
