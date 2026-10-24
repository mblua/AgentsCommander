use super::*;

/// Test 9 (Claude, changed path): the restart mints a new session UUID and
/// transfers reader ownership to it. The old id's reader, pump and slot are
/// gone before the replacement is delivered; the replacement gets a fresh
/// reader, one demand per consumer, a fresh cut and its own slot; an old
/// slot clone cannot reach the new slot; and only the appended post-restart
/// line is sent, exactly once.
#[tokio::test]
async fn a_live_restart_transfers_the_reader_to_the_new_uuid() {
    use crate::capture::record::RecordOrigin;
    use crate::capture::sink::SlotValue;

    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let cwd = fixture.coordinator_path();
    let old_info = h.live_session_info(cwd, CodingAgentKind::Claude).await;
    let old = Uuid::parse_str(&old_info.id).expect("session id");
    let old_key = old.to_string();
    let minted_old = minted_transcript_id(&old_info).expect("the old spawn mints a transcript id");

    // Room demand first, then a hot Bot attach over the live reader.
    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), old).await);
    let old_dir = h.claude_projects_dir(&h.claude_config_dir, cwd);
    std::fs::create_dir_all(&old_dir).expect("old transcript dir");
    let old_path = old_dir.join(format!("{minted_old}.jsonl"));
    std::fs::write(
        &old_path,
        format!(
            "{}\n",
            claude_line(
                "old preamble",
                chrono::Utc::now() - chrono::Duration::seconds(1)
            )
        ),
    )
    .expect("old transcript");
    let old_slot = h.captures.slot(&old_key).expect("old endpoints open");
    // The cold bind offers the recent tail to the sink: that proves the
    // reader is bound before the append below can race the scan.
    wait_for_slot_record(&old_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "old preamble"
    })
    .await;
    h.attach_bot(old).await;
    append_claude(&old_path, "old live");
    wait_for_slot_record(&old_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "old live"
    })
    .await;
    // Let the pending Telegram buffer flush, so the final old release has
    // nothing left to send.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    let (old_reader_id, old_demands) = h.snapshot(old).await;
    assert_eq!(
        old_demands,
        [ReaderConsumer::Bot, ReaderConsumer::Room]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
    );

    // The replacement resolves a different transcript directory: a >64 KiB
    // Claude file whose only final answer sits above the 64 KiB tail
    // window, so cold-start rules never replay it. It is written AFTER
    // the restart, named from the replacement's minted id (#2454); the
    // pre-restart `session.jsonl` is a backdated decoy that is never the
    // newest, so only the pin can choose the minted file.
    let new_dir = fixture.temp_path().join("claude-home-b");
    std::fs::create_dir_all(&new_dir).expect("new config dir");
    h.set_claude_config_dir(&new_dir).await;
    let new_projects = h.claude_projects_dir(&new_dir, cwd);
    std::fs::create_dir_all(&new_projects).expect("new transcript dir");
    let decoy = new_projects.join("session.jsonl");
    large_claude_transcript(&decoy, "decoy above the tail window");
    backdate(&decoy, 120);

    let restarted = h.restart_fresh(old).await.expect("restart succeeds");
    let new = Uuid::parse_str(&restarted.id).expect("replacement id");
    assert_ne!(new, old, "the restart mints a new session UUID");
    let minted_new =
        minted_transcript_id(&restarted).expect("the replacement mints a transcript id");
    assert_ne!(
        minted_new, minted_old,
        "the replacement mints its own transcript id"
    );
    let new_path = new_projects.join(format!("{minted_new}.jsonl"));
    // Written under a non-`.jsonl` name and renamed in: a waiting reader
    // pins the file the moment it appears, and a half-written file would
    // still hold the first line inside the tail window.
    let staging = new_projects.join(format!("{minted_new}.jsonl.tmp"));
    let new_len = large_claude_transcript(&staging, "above the tail window");
    std::fs::rename(&staging, &new_path).expect("publish the new transcript");

    // Old id: reader, demands, pump and slot are gone.
    let (gone_reader, gone_demands) = h.snapshot(old).await;
    assert_eq!(gone_reader, None);
    assert!(gone_demands.is_empty());
    assert!(!h.captures.is_open(&old_key), "the old slot is closed");
    assert!(h.captures.slot(&old_key).is_none());
    // A stale old-id slot clone cannot reach the replacement's slot: the
    // replacement got a fresh slot object.
    old_slot.set_cut(Cut {
        path: old_path.clone(),
        epoch: 9,
        len: 9,
        reader_seq: Some(9),
    });

    // New id: exactly one reader, one demand per consumer, fresh cut, slot.
    let (new_reader_id, new_demands) = h.snapshot(new).await;
    assert!(new_reader_id.is_some());
    assert_ne!(new_reader_id, old_reader_id, "a new reader identity");
    assert_eq!(
        new_demands,
        [ReaderConsumer::Bot, ReaderConsumer::Room]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
    );
    let new_key = new.to_string();
    let new_slot = h.captures.slot(&new_key).expect("new endpoints open");
    // A fresh slot has no cut at all; if a demand join recorded one, its
    // sequence part is superseded. Either way the old reader's cut cannot
    // be the replacement's.
    if let Some(cut) = new_slot.cut() {
        assert_eq!(
            cut.reader_seq, None,
            "the replacement's cut sequence must never be the old reader's"
        );
    }
    // Give the room-only reader one poll to cold-bind the new file and one
    // tick for the hot Bot attach; neither may offer a record.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert!(matches!(new_slot.snapshot().value, SlotValue::Empty));
    let seq_before = new_slot.seq();
    let sends_before = h.message_permits();

    append_claude(&new_path, "post restart");
    let (seq, record) = wait_for_slot_record(&new_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "post restart"
    })
    .await;
    assert_eq!(record.origin, RecordOrigin::Live);
    assert_eq!(record.observed_path, new_path);
    assert!(
        record.record_start.expect("live record") >= new_len,
        "only appended bytes may be read after the replacement's cold bind"
    );
    assert_eq!(
        seq,
        seq_before + 1,
        "exactly one capture offer for the post-restart line"
    );
    wait_for_message_permits(&h, sends_before + 1).await;
    // Exactly one send for the appended line: the old reader is gone and
    // its pending buffer was empty at the final release.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert_eq!(h.message_permits(), sends_before + 1);
    assert!(h.captures.is_open(&new_key));
    h.release_all(new).await;
    h.close().await;
}

/// Test 9 (Claude, same directory, fresh restart; #2454 p3b test 23a): a
/// fresh restart mints a NEW transcript id, so the replacement never
/// appends to the previous spawn's file. It pins its own minted file and
/// reads only that file's appended bytes; nothing the old spawn wrote ever
/// reaches the new slot.
#[tokio::test]
async fn a_fresh_restart_reads_only_its_own_minted_transcript() {
    use crate::capture::record::RecordOrigin;
    use crate::capture::sink::SlotValue;

    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let cwd = fixture.coordinator_path();
    let old_info = h.live_session_info(cwd, CodingAgentKind::Claude).await;
    let old = Uuid::parse_str(&old_info.id).expect("session id");
    let minted_old = minted_transcript_id(&old_info).expect("the old spawn mints a transcript id");

    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), old).await);
    let dir = h.claude_projects_dir(&h.claude_config_dir, cwd);
    std::fs::create_dir_all(&dir).expect("transcript dir");
    let old_path = dir.join(format!("{minted_old}.jsonl"));
    std::fs::write(
        &old_path,
        format!(
            "{}\n",
            claude_line(
                "cold preamble",
                chrono::Utc::now() - chrono::Duration::seconds(1)
            )
        ),
    )
    .expect("old transcript");
    let old_slot = h
        .captures
        .slot(&old.to_string())
        .expect("old endpoints open");
    wait_for_slot_record(&old_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "cold preamble"
    })
    .await;
    append_claude(&old_path, "old live");
    wait_for_slot_record(&old_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "old live"
    })
    .await;
    // Not backdated and never touched again: right after the restart it is
    // the only, hence the newest, `.jsonl` in the directory.

    let restarted = h.restart_fresh(old).await.expect("restart succeeds");
    let new = Uuid::parse_str(&restarted.id).expect("replacement id");
    assert_ne!(new, old);
    assert!(h.snapshot(old).await.0.is_none());
    // Step 1: the replacement minted its own transcript id.
    let minted_new =
        minted_transcript_id(&restarted).expect("the fresh restart mints a transcript id");
    assert_ne!(
        minted_new, minted_old,
        "a fresh restart mints a new transcript id"
    );

    let new_slot = h
        .captures
        .slot(&new.to_string())
        .expect("new endpoints open");
    // Step 2, the guard: more than two 500 ms polls with ONLY the old file
    // present. A pinned reader waits; a reader attaching by mtime would
    // already have offered the old file's tail, both lines of which sit
    // inside the 5 s window.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let step2 = new_slot.snapshot().value;
    assert!(
        matches!(step2, SlotValue::Empty),
        "the replacement attached to the old spawn's transcript: {:?}",
        match &step2 {
            SlotValue::Valid(record) => Some(record.text.clone()),
            _ => None,
        }
    );

    // Step 3: the minted file appears, published by rename, holding one
    // completed reply outside the 5 s window, so the pinned attach binds
    // it and offers nothing.
    let new_path = dir.join(format!("{minted_new}.jsonl"));
    let staging = dir.join(format!("{minted_new}.jsonl.tmp"));
    std::fs::write(
        &staging,
        format!(
            "{}\n",
            claude_line(
                "new history",
                chrono::Utc::now() - chrono::Duration::seconds(60)
            )
        ),
    )
    .expect("new transcript");
    std::fs::rename(&staging, &new_path).expect("publish the new transcript");
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert!(matches!(new_slot.snapshot().value, SlotValue::Empty));
    let seq_before = new_slot.seq();
    let len_before = std::fs::metadata(&new_path)
        .expect("transcript metadata")
        .len();

    // Step 4: exactly the appended line follows the attach.
    append_claude(&new_path, "new live");
    let (seq, record) = wait_for_slot_record(&new_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "new live"
    })
    .await;
    assert_eq!(record.origin, RecordOrigin::Live);
    assert_eq!(record.observed_path, new_path);
    assert_eq!(
        record.record_start.expect("live record"),
        len_before,
        "only the appended bytes of the minted file are read"
    );
    assert_eq!(seq, seq_before + 1, "exactly one offer follows the attach");
    h.release_all(new).await;
    h.close().await;
}

/// Test 9 (Claude, same path, resumed restart; #2454 p3b test 23b): the
/// replacement cold-binds the same transcript file. That is reachable
/// only when no transcript id is minted, forced here by a configured
/// `--continue`. Normal cold-start rules apply: the recent tail is
/// re-offered as `Preamble` (that is what a cold attach does today), the
/// offset starts at the file length, and only the appended line arrives
/// `Live` above the pre-restart length.
#[tokio::test]
async fn a_resumed_restart_reads_only_appended_bytes_of_the_same_transcript() {
    use crate::capture::record::RecordOrigin;

    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let mut h = restart_harness(&fixture);
    // A configured identity flag vetoes the `--session-id` injection; it
    // must live in the configured agent because a restart re-resolves argv
    // from settings.
    h.set_claude_command("claude --continue").await;
    let cwd = fixture.coordinator_path();
    let old = h.live_session(cwd, CodingAgentKind::Claude).await;

    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), old).await);
    let dir = h.claude_projects_dir(&h.claude_config_dir, cwd);
    std::fs::create_dir_all(&dir).expect("transcript dir");
    let path = dir.join("session.jsonl");
    std::fs::write(
        &path,
        format!(
            "{}\n",
            claude_line(
                "cold preamble",
                chrono::Utc::now() - chrono::Duration::seconds(1)
            )
        ),
    )
    .expect("transcript");
    let old_slot = h
        .captures
        .slot(&old.to_string())
        .expect("old endpoints open");
    wait_for_slot_record(&old_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "cold preamble"
    })
    .await;
    append_claude(&path, "old live");
    wait_for_slot_record(&old_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "old live"
    })
    .await;
    let len_before = std::fs::metadata(&path).expect("transcript metadata").len();

    let restarted = h.restart(old).await.expect("restart succeeds");
    let new = Uuid::parse_str(&restarted.id).expect("replacement id");
    assert_ne!(new, old);
    // Non-vacuity: an id minted here would pin a different file, and this
    // test must fail rather than pass on a shape it does not describe.
    assert_eq!(
        minted_transcript_id(&restarted),
        None,
        "a resumed restart mints no transcript id: {:?}",
        restarted.effective_shell_args
    );
    assert!(h.snapshot(old).await.0.is_none());
    assert!(!h.captures.is_open(&old.to_string()));

    let new_slot = h
        .captures
        .slot(&new.to_string())
        .expect("new endpoints open");
    // The cold scan re-offers the recent tail as Preamble; waiting for the
    // last tail line proves the replacement has bound the file (and set
    // its offset to the length) before the append below.
    wait_for_slot_record(&new_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "old live" && r.origin == RecordOrigin::Preamble
    })
    .await;
    let seq_after_scan = new_slot.seq();

    append_claude(&path, "new live");
    let (seq, record) = wait_for_slot_record(&new_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "new live"
    })
    .await;
    assert_eq!(record.origin, RecordOrigin::Live);
    assert_eq!(
        record.record_start.expect("live record"),
        len_before,
        "the same path must not replay consumed bytes"
    );
    assert_eq!(
        seq,
        seq_after_scan + 1,
        "only the appended line may follow the cold scan"
    );
    h.release_all(new).await;
    h.close().await;
}

/// Test 9 (Codex, changed path): the same UUID transfer, driven for the
/// Codex watcher with a fresh `CODEX_HOME`, a real slot record and one
/// Telegram send for the appended post-restart line.
#[tokio::test]
async fn a_codex_live_restart_transfers_the_reader_to_the_new_uuid() {
    use crate::capture::record::RecordOrigin;
    use crate::capture::sink::SlotValue;

    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let cwd = fixture.coordinator_path();
    let cwd_str = cwd.to_string_lossy().into_owned();
    let old = h.live_session(cwd, CodingAgentKind::Codex).await;
    let old_key = old.to_string();

    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), old).await);
    let old_path = write_codex_rollout(
        &h.codex_home,
        &cwd_str,
        "rollout-old.jsonl",
        "codex preamble",
        chrono::Utc::now() - chrono::Duration::seconds(1),
    );
    let old_slot = h.captures.slot(&old_key).expect("old endpoints open");
    wait_for_slot_record(&old_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "codex preamble"
    })
    .await;
    h.attach_bot(old).await;
    append_codex(&old_path, "codex live");
    wait_for_slot_record(&old_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "codex live"
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;

    let new_home = fixture.temp_path().join("codex-home-b");
    std::fs::create_dir_all(&new_home).expect("new codex home");
    h.set_codex_home(&new_home).await;
    let new_path = write_codex_rollout(
        &new_home,
        &cwd_str,
        "rollout-new.jsonl",
        "codex old",
        chrono::Utc::now() - chrono::Duration::seconds(30),
    );

    let restarted = h.restart(old).await.expect("restart succeeds");
    let new = Uuid::parse_str(&restarted.id).expect("replacement id");
    assert_ne!(new, old);
    assert!(h.snapshot(old).await.0.is_none());
    assert!(!h.captures.is_open(&old_key));
    assert!(h.captures.slot(&old_key).is_none());

    let new_key = new.to_string();
    let new_slot = h.captures.slot(&new_key).expect("new endpoints open");
    if let Some(cut) = new_slot.cut() {
        assert_eq!(cut.reader_seq, None, "the replacement's cut is fresh");
    }
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert!(matches!(new_slot.snapshot().value, SlotValue::Empty));
    let seq_before = new_slot.seq();
    let sends_before = h.message_permits();

    append_codex(&new_path, "codex post restart");
    let (seq, record) = wait_for_slot_record(&new_slot, std::time::Duration::from_secs(10), |r| {
        r.text == "codex post restart"
    })
    .await;
    assert_eq!(record.origin, RecordOrigin::Live);
    assert_eq!(record.observed_path, new_path);
    assert_eq!(seq, seq_before + 1);
    wait_for_message_permits(&h, sends_before + 1).await;
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert_eq!(h.message_permits(), sends_before + 1);
    h.release_all(new).await;
    h.close().await;
}
