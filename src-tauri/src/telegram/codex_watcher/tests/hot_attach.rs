use super::*;

// ── #2232 phase 4: hot attach, re-anchor and the local preamble (tests 4,
// 9-codex and 18) ───────────────────────────────────────────────────────

/// Append one fresh `final_answer` line whose timestamp is `now - 1s`, so
/// both the §J grace window and the live read path accept it.
fn append_final(path: &Path, body: &str) -> std::io::Result<()> {
    let ts = (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
    let line = current_final_record_with_ts(body, Some(&ts));
    let mut f = fs::OpenOptions::new().append(true).open(path)?;
    writeln!(f, "{}", line)?;
    f.sync_all()
}

async fn recv_record(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Arc<CapturedRecord>>,
) -> Arc<CapturedRecord> {
    tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("a record must reach the sink")
        .expect("the sink stays open")
}

fn message_permits(network: &OutboundNetwork) -> usize {
    network
        .acquired_labels_for_tests()
        .iter()
        .filter(|label| **label == "telegram.send_message")
        .count()
}

async fn wait_for_message_permits(network: &OutboundNetwork, at_least: usize) {
    let deadline = Instant::now() + Duration::from_secs(25);
    while message_permits(network) < at_least {
        assert!(
            Instant::now() < deadline,
            "no telegram.send_message attempt within budget"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Test 4 (Codex half): the local start-indexed hot preamble returns starts
/// that walk back to the exact raw bytes, including a line containing a
/// multi-byte character and a line ending in CRLF. The same fixture shape
/// as the Claude half exercises both local variants.
#[test]
fn codex_preamble_line_starts_index_back_to_the_exact_raw_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout-preamble.jsonl");
    let now = Utc::now();
    let bodies = ["first", "caf\u{e9} \u{2705} second", "third"];
    let mut raw: Vec<u8> = Vec::new();
    let mut starts = Vec::new();
    let mut offset = 0u64;
    for (i, body) in bodies.iter().enumerate() {
        let ts = (now - chrono::Duration::milliseconds(100)).to_rfc3339();
        let line = current_final_record_with_ts(body, Some(&ts));
        let bytes = if i == 1 {
            format!("{line}\r\n")
        } else {
            format!("{line}\n")
        };
        starts.push(offset);
        offset += bytes.len() as u64;
        raw.extend_from_slice(bytes.as_bytes());
    }
    fs::write(&path, &raw).unwrap();

    let lines = read_preamble_with_starts(&path, now).expect("preamble scan");
    assert_eq!(lines.len(), bodies.len());
    for ((start, body), (expected_start, expected_body)) in
        lines.iter().zip(starts.iter().zip(bodies.iter()))
    {
        assert_eq!(start, expected_start, "body={body}");
        let tail = &raw[*start as usize..];
        let end = tail.iter().position(|&b| b == b'\n').expect("terminated");
        let re_read = String::from_utf8(tail[..end].to_vec())
            .unwrap()
            .trim_end_matches('\r')
            .to_string();
        assert_eq!(
            extract_assistant_final(&re_read).as_deref(),
            Some(*expected_body)
        );
        assert_eq!(body.as_str(), *expected_body);
    }
}

/// Test 18: **Codex hot attach/detach.** A room-only reader takes a bot
/// through the destination watch without restarting; the attached line is
/// sent exactly once; a detach stops Telegram while capture continues; a
/// pending buffer never flushes to a detached reader.
#[tokio::test]
async fn a_codex_hot_attach_sends_once_and_a_detach_stops_without_restarting() {
    // This test constructs the loggers on attach; keep the global log files
    // to itself while the room-only tests assert they are untouched.
    let _logs = lock_global_diagnostic_files().await;
    let work = tempfile::tempdir().unwrap();
    let cwd = work.path().to_string_lossy().replace('\\', "\\\\");
    let (fixture, path) = room_only_fixture(&cwd);
    let search_root = fixture.path().to_path_buf();

    let events = Arc::new(RecordingEvents::default());
    let network = OutboundNetwork::new_for_tests(1);
    let cancel = CancellationToken::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let (dest_tx, dest_rx) = tokio::sync::watch::channel(None);

    let task = spawn_watch_task(
        search_root,
        work.path().to_string_lossy().to_string(),
        Utc::now(),
        network.clone(),
        dest_rx,
        "hot-attach-session".to_string(),
        cancel.clone(),
        events.clone(),
        Some(tx),
    );

    // Room-only first bind: the tail record is captured, nothing is sent.
    let first = recv_record(&mut rx).await;
    assert_eq!(first.origin, RecordOrigin::Preamble);
    assert_eq!(message_permits(&network), 0);

    // Attach the bot over the running reader. No restart: the task stays up.
    dest_tx
        .send(Some(BotTarget {
            token: "test-token".into(),
            chat_id: 7,
        }))
        .expect("the reader watch stays open");
    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS * 2)).await;
    assert!(!task.is_finished(), "attach must not restart the reader");

    // A line appended after the attach is sent exactly once.
    append_final(&path, "live after attach").unwrap();
    let live = recv_record(&mut rx).await;
    assert_eq!(live.text, "live after attach");
    assert_eq!(live.origin, RecordOrigin::Live);
    wait_for_message_permits(&network, 1).await;

    // Detach while the reader keeps capturing. A record buffered around the
    // detach must never be flushed to the detached reader.
    append_final(&path, "buffered before detach").unwrap();
    let buffered = recv_record(&mut rx).await;
    assert_eq!(buffered.text, "buffered before detach");
    dest_tx.send(None).expect("the reader watch stays open");
    append_final(&path, "after detach").unwrap();
    let after = recv_record(&mut rx).await;
    assert_eq!(after.text, "after detach");

    tokio::time::sleep(Duration::from_millis(FLUSH_DELAY_MS * 2)).await;
    assert_eq!(
        message_permits(&network),
        1,
        "only the attached line may be sent; the pending buffer is discarded"
    );

    cancel.cancel();
    drop(dest_tx);
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}
