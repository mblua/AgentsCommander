use super::*;

/// Test 15: **Room demand scope.** A session in a room whose flag is off
/// is not capturable and raises **no** Room demand, and no reader is
/// spawned for it; the orchestrator session of a room with the flag on is
/// capturable and raises one (#2756 U2a). This protects "rooms without the flag behave as
/// today".
#[tokio::test]
async fn only_a_capturable_session_raises_the_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), false);
    let h = harness(&fixture);
    let orchestrator = h.session_in(fixture.coordinator_path()).await;

    // Flag off: no demand, no reader, no capture entry.
    assert!(!raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), orchestrator).await);
    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(!tg.reader_is_running(orchestrator));
        assert!(tg.reader_demands(orchestrator).is_empty());
    }
    assert!(!h.captures.is_open(&orchestrator.to_string()));

    // Flag on: the orchestrator raises one; a member session never does.
    configure_room(fixture.room_path(), true);
    let member = h.session_in(fixture.member_path()).await;
    assert!(!raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), member).await);
    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), orchestrator).await);
    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(tg.reader_is_running(orchestrator));
        assert!(!tg.reader_is_running(member));
        assert_eq!(
            tg.reader_demands(orchestrator)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![ReaderConsumer::Room]
        );
    }
    // Acceptance criterion 8: the reader was spawned through the supervisor
    // with the live registry sender, so the session's endpoints are open.
    assert!(h.captures.is_open(&orchestrator.to_string()));
    assert!(h.captures.sender(&orchestrator.to_string()).is_some());

    let shutdown = {
        let tg = h.bridge().await;
        let mut tg = tg.lock().await;
        tg.reader_release_all(orchestrator)
    };
    if let Some(shutdown) = shutdown {
        shutdown.abort_now();
    }
}

/// Acceptance criterion 8: a reader spawned through the supervisor delivers
/// a record into `CaptureRegistry`'s **slot**, not merely into a channel.
/// This is the link that makes phases 1 and 3 reachable from production.
#[tokio::test]
async fn a_supervised_reader_puts_a_record_in_the_registry_slot() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let orchestrator = h.session_in(fixture.coordinator_path()).await;

    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), orchestrator).await);
    let slot = h
        .captures
        .slot(&orchestrator.to_string())
        .expect("the supervisor opened the session's endpoints");
    let before = slot.seq();

    // A transcript appears in the resolved projects dir and the reader
    // picks it up on its own poll.
    let transcript = h.projects_dir.join("session.jsonl");
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "message": {"content": [{"type": "text", "text": "supervised body"}]}
            })
        ),
    )
    .expect("write transcript");

    let mut rx = slot.subscribe();
    let published = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if rx.changed().await.is_err() {
                return None;
            }
            let state = rx.borrow_and_update().clone();
            if let crate::capture::sink::SlotValue::Valid(record) = state.value {
                return Some((state.seq, record));
            }
        }
    })
    .await
    .expect("a supervised reader must reach the slot");

    let (seq, record) = published.expect("the slot channel stays open");
    assert!(seq > before);
    assert_eq!(record.text, "supervised body");
    assert_eq!(record.session_id, orchestrator.to_string());

    let shutdown = {
        let tg = h.bridge().await;
        let mut tg = tg.lock().await;
        tg.reader_release_all(orchestrator)
    };
    if let Some(shutdown) = shutdown {
        shutdown.abort_now();
    }
}

/// Acceptance criterion 8 (Codex half): a Codex reader spawned through the
/// supervisor delivers a record into `CaptureRegistry`'s **slot** with the
/// production sender, proving the phase-1/phase-3 chain is reachable from
/// production for both watchers.
#[tokio::test]
async fn a_supervised_codex_reader_puts_a_record_in_the_registry_slot() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let home = fixture.temp_path().join("codex-home");
    std::fs::create_dir_all(&home).expect("codex home");
    let id = h.codex_session_in(fixture.coordinator_path(), &home).await;

    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), id).await);
    let slot = h.captures.slot(&id.to_string()).expect("endpoints open");
    let cwd = fixture.coordinator_path().to_string_lossy().into_owned();
    write_codex_rollout(
        &home,
        &cwd,
        "rollout-supervised.jsonl",
        "codex supervised body",
        chrono::Utc::now() - chrono::Duration::seconds(1),
    );
    let (_, record) = wait_for_slot_record(&slot, std::time::Duration::from_secs(10), |r| {
        r.text == "codex supervised body"
    })
    .await;
    assert_eq!(record.session_id, id.to_string());

    let shutdown = {
        let tg = h.bridge().await;
        let mut tg = tg.lock().await;
        tg.reader_release_all(id)
    };
    if let Some(shutdown) = shutdown {
        shutdown.abort_now();
    }
}

/// Test 21: a live detach whose `sessions.json` persistence fails still
/// releases the Bot demand (so the reader stops sending) and keeps capture
/// running under the surviving Room demand, with the error and event
/// unchanged.
#[tokio::test]
async fn a_detach_persistence_failure_releases_the_bot_demand_and_keeps_capture() {
    use crate::capture::record::RecordOrigin;
    use tauri::Listener;

    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let cwd = fixture.coordinator_path();
    let info = h.live_session_info(cwd, CodingAgentKind::Claude).await;
    let id = Uuid::parse_str(&info.id).expect("session id");
    let key = id.to_string();
    let minted = minted_transcript_id(&info).expect("the spawn mints a transcript id");

    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), id).await);
    h.attach_bot(id).await;
    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(tg.has_bridge(id));
        assert_eq!(
            tg.reader_demands(id),
            [ReaderConsumer::Bot, ReaderConsumer::Room]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
    }

    let (tx, rx) = std::sync::mpsc::channel();
    h.app.listen_any("telegram_bridge_error", move |event| {
        let _ = tx.send(event.payload().to_string());
    });

    crate::commands::telegram::detach_persistence_seam::arm(id);
    let error = crate::commands::telegram::detach_telegram_inner(h.app.handle(), id)
        .await
        .expect_err("the forced persistence failure must surface");
    assert!(error.contains("could not be persisted"), "{error}");
    let payload = rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("telegram_bridge_error is emitted");
    assert!(payload.contains("could not be persisted"), "{payload}");

    // Bot is gone; the Room demand keeps the reader and its slot open.
    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(!tg.has_bridge(id));
        assert!(tg.reader_is_running(id));
        assert_eq!(
            tg.reader_demands(id),
            [ReaderConsumer::Room]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
    }
    assert!(h.captures.is_open(&key));
    let slot = h.captures.slot(&key).expect("slot open");

    // Capture continues with no send: the appended line reaches the slot
    // and acquires no `telegram.send_message` permit. The reader pins its
    // first attach to the minted transcript (#2454), so "before detach"
    // lands `Live`; `sends_before` is read after it, so the count holds.
    let dir = h.claude_projects_dir(&h.claude_config_dir, cwd);
    std::fs::create_dir_all(&dir).expect("transcript dir");
    let path = dir.join(format!("{minted}.jsonl"));
    std::fs::write(
        &path,
        format!(
            "{}\n",
            claude_line(
                "before detach",
                chrono::Utc::now() - chrono::Duration::seconds(1)
            )
        ),
    )
    .expect("transcript");
    wait_for_slot_record(&slot, std::time::Duration::from_secs(10), |r| {
        r.text == "before detach"
    })
    .await;
    let sends_before = h.message_permits();
    append_claude(&path, "after detach");
    let (_, record) = wait_for_slot_record(&slot, std::time::Duration::from_secs(10), |r| {
        r.text == "after detach"
    })
    .await;
    assert_eq!(record.origin, RecordOrigin::Live);
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert_eq!(
        h.message_permits(),
        sends_before,
        "a detached reader must not send"
    );

    h.release_all(id).await;
    h.close().await;
}

/// Test 16: **live toggle through the production command.** With an
/// orchestrator session already running and no reader,
/// `co_managed_set_enabled(room, true)` must drive its live-session loop,
/// raise the Room demand and start a reader; `false` must release it and
/// stop the reader. A member session in the same room must never get one.
/// Epic section 3.2 is explicitly about already-running sessions; a
/// swapped command branch or a no-op live-session loop fails this test.
#[tokio::test]
async fn toggling_the_flag_on_a_live_session_starts_and_stops_the_reader() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), false);
    let h = harness(&fixture);
    let orchestrator = h.session_in(fixture.coordinator_path()).await;
    let member = h.session_in(fixture.member_path()).await;

    // Flag off and no reader anywhere: the command is the only trigger.
    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(!tg.reader_is_running(orchestrator));
        assert!(!tg.reader_is_running(member));
    }

    let room_root = fixture.room_path().to_string_lossy().into_owned();
    let config = crate::commands::co_managed::co_managed_set_enabled(
        h.app.handle().clone(),
        room_root.clone(),
        true,
    )
    .await
    .expect("enabling the flag on a live room succeeds");
    assert!(config.enabled, "the on-disk config is enabled");

    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(
            tg.reader_is_running(orchestrator),
            "the live orchestrator must get a reader from the toggle"
        );
        assert_eq!(
            tg.reader_demands(orchestrator)
                .into_iter()
                .collect::<Vec<_>>(),
            vec![ReaderConsumer::Room]
        );
        assert!(
            !tg.reader_is_running(member),
            "a member session never gets a reader from the toggle"
        );
    }
    assert!(h.captures.is_open(&orchestrator.to_string()));
    assert!(!h.captures.is_open(&member.to_string()));

    let config = crate::commands::co_managed::co_managed_set_enabled(
        h.app.handle().clone(),
        room_root,
        false,
    )
    .await
    .expect("disabling the flag succeeds");
    assert!(!config.enabled);

    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(!tg.reader_is_running(orchestrator));
        assert!(tg.reader_demands(orchestrator).is_empty());
    }
    assert!(!h.captures.is_open(&orchestrator.to_string()));
}

/// The production toggle canonicalises its `room_root`, while a session
/// stores the cwd it was created with. A session reached through an
/// unresolved ancestor (symlink on Unix, verbatim `\\?\` prefix on
/// Windows) must still be matched, or the toggle silently starts no
/// reader. Unix-only here because the Windows shape needs the Windows
/// path canonicaliser; the ordinary Windows CI run owns that side.
#[cfg(unix)]
#[tokio::test]
async fn toggling_matches_a_live_session_reached_through_an_unresolved_ancestor() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), false);

    // temp/project-link -> temp/project-a, so the room and its agents stay
    // reachable through an ancestor that canonicalises away.
    let link = fixture.temp_path().join("project-link");
    std::os::unix::fs::symlink(fixture.temp_path().join("project-a"), &link)
        .expect("symlink project");
    let linked_coordinator = link
        .join(".ac")
        .join("room-1-dev-team")
        .join("__agent_coordinator");

    let h = harness(&fixture);
    let orchestrator = h.session_in(&linked_coordinator).await;

    let config = crate::commands::co_managed::co_managed_set_enabled(
        h.app.handle().clone(),
        fixture.room_path().to_string_lossy().into_owned(),
        true,
    )
    .await
    .expect("enabling the flag on a live room succeeds");
    assert!(config.enabled, "the on-disk config is enabled");

    let tg = h.bridge().await;
    let tg = tg.lock().await;
    assert!(
        tg.reader_is_running(orchestrator),
        "a live session reached through an unresolved ancestor must still get a reader"
    );
}

/// B1 regression: the create-time Room raise must run **after** the
/// pending-create row is finalized and visible. Under the old ordering the
/// detached raise ran while the row was hidden, the gathering function
/// answered "session not found", and the Ready room-only session never got
/// a reader or a capture slot (and never retried).
#[tokio::test]
async fn a_created_session_raises_its_room_demand_after_finalization() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let cwd = fixture.coordinator_path();

    let id = h.live_session(cwd, CodingAgentKind::Claude).await;
    let key = id.to_string();
    assert!(
        h.manager.read().await.get_session(id).await.is_some(),
        "the create returns a finalized, visible row"
    );

    // The raise is detached, so poll: the bug is an absent reader, not a
    // slow one.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let running = {
            let tg = h.bridge().await;
            let tg = tg.lock().await;
            tg.reader_is_running(id)
        };
        if running && h.captures.is_open(&key) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "a Ready room-only create must get a reader and an open capture slot"
        );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert_eq!(
            tg.reader_demands(id).into_iter().collect::<Vec<_>>(),
            vec![ReaderConsumer::Room]
        );
    }
    assert!(h.captures.slot(&key).is_some());
    h.release_all(id).await;
    h.close().await;
}

#[tokio::test]
async fn response_close_bridge_detach_preserves_actual_resume_bot_assignment() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let harness = restart_harness(&fixture);
    let id = harness
        .live_session(fixture.coordinator_path(), CodingAgentKind::Claude)
        .await;
    harness.attach_bot(id).await;
    let before = harness
        .manager
        .read()
        .await
        .get_session(id)
        .await
        .unwrap()
        .telegram_bot_id;
    assert_eq!(before.as_deref(), Some(PHASE4_BOT_ID));
    crate::commands::session::detach_response_close_telegram(harness.app.handle(), id)
        .await
        .unwrap();
    {
        let telegram = harness.bridge().await;
        let telegram = telegram.lock().await;
        assert!(!telegram.has_bridge(id), "actual transport detached");
        assert_eq!(telegram.test_detach_count(id), 1);
    }
    crate::commands::session::detach_response_close_telegram(harness.app.handle(), id)
        .await
        .unwrap();
    {
        let telegram = harness.bridge().await;
        assert_eq!(
            telegram.lock().await.test_detach_count(id),
            1,
            "retry after completed detach does not repeat the bridge effect"
        );
    }
    assert_eq!(
        harness
            .manager
            .read()
            .await
            .get_session(id)
            .await
            .unwrap()
            .telegram_bot_id,
        before,
        "retained A keeps the exact bot assignment for its next resume"
    );
    harness.release_all(id).await;
    harness.close().await;
}

/// B3: a successful **dormant** restart takes the `!old_lost` branch: after
/// the commit the old bridge is detached, the old reader and slot are
/// released, and the replacement gets its own reader.
#[tokio::test]
async fn a_successful_dormant_restart_detaches_the_old_bridge_and_transfers_the_reader() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let cwd = fixture.coordinator_path();
    let old = h.live_session(cwd, CodingAgentKind::Claude).await;
    let old_key = old.to_string();

    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), old).await);
    h.attach_bot(old).await;
    h.backend.kill(old).expect("kill the dormant old");
    h.app
        .state::<crate::session::selection::SelectionCoordinator>()
        .container_lifecycle_sender()
        .route_lost(old, 17)
        .await
        .expect("reconcile the dormant route");
    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(
            tg.has_bridge(old),
            "the dormant old keeps its bridge until the post-commit branch"
        );
    }

    let restarted = h.restart(old).await.expect("dormant restart succeeds");
    let new = Uuid::parse_str(&restarted.id).expect("replacement id");
    assert_ne!(new, old);

    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(
            !tg.has_bridge(old),
            "the !old_lost branch detaches the old bridge"
        );
        assert_eq!(tg.test_detach_count(old), 1);
        assert!(!tg.reader_is_running(old));
        assert!(tg.reader_is_running(new));
    }
    assert!(!h.captures.is_open(&old_key));
    assert!(h.captures.is_open(&new.to_string()));
    assert!(h.manager.read().await.get_session(old).await.is_none());
    h.release_all(new).await;
    h.close().await;
}

/// B3: attach rollback at its production site. A persistence failure after
/// the bridge attach rolls the bridge back, clears the persisted bot id,
/// emits the error and leaves the Room reader capturing; the rollback must
/// not release a Bot demand this attach never raised.
#[tokio::test]
async fn an_attach_persistence_failure_rolls_back_without_touching_the_room_reader() {
    use tauri::Listener;

    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let cwd = fixture.coordinator_path();
    let id = h.live_session(cwd, CodingAgentKind::Claude).await;
    let key = id.to_string();

    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), id).await);

    let (tx, rx) = std::sync::mpsc::channel();
    h.app.listen_any("telegram_bridge_error", move |event| {
        let _ = tx.send(event.payload().to_string());
    });

    crate::commands::telegram::attach_persistence_seam::arm(id);
    let error =
        crate::commands::telegram::attach_telegram_bot_by_id(h.app.handle(), id, PHASE4_BOT_ID)
            .await
            .expect_err("the forced attach persistence failure must surface");
    assert!(error.contains("could not be persisted"), "{error}");
    let payload = rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("telegram_bridge_error is emitted");
    assert!(payload.contains("could not be persisted"), "{payload}");

    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(!tg.has_bridge(id), "the rolled-back bridge is detached");
        assert!(tg.reader_is_running(id));
        assert_eq!(
            tg.reader_demands(id).into_iter().collect::<Vec<_>>(),
            vec![ReaderConsumer::Room],
            "the rollback must not touch the Room demand"
        );
    }
    assert!(h.captures.is_open(&key));
    assert!(h
        .manager
        .read()
        .await
        .get_session(id)
        .await
        .expect("session row")
        .telegram_bot_id
        .is_none());

    h.release_all(id).await;
    h.close().await;
}

/// B3: an idempotent repeated attach that fails persistence must leave the
/// earlier attach's Bot demand alone; a rollback releases only a demand
/// *that attach raised*.
#[tokio::test]
async fn a_repeated_attach_rollback_keeps_the_previous_bot_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let cwd = fixture.coordinator_path();
    let id = h.live_session(cwd, CodingAgentKind::Claude).await;

    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), id).await);
    // First attach succeeds and raises the Bot demand.
    h.attach_bot(id).await;
    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(tg.reader_demands(id).contains(&ReaderConsumer::Bot));
    }

    crate::commands::telegram::attach_persistence_seam::arm(id);
    let error =
        crate::commands::telegram::attach_telegram_bot_by_id(h.app.handle(), id, PHASE4_BOT_ID)
            .await
            .expect_err("the repeated attach persistence failure must surface");
    assert!(error.contains("could not be persisted"), "{error}");

    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(
            tg.reader_demands(id).contains(&ReaderConsumer::Bot),
            "a repeated attach rollback must not release the earlier Bot demand"
        );
        assert!(tg.reader_demands(id).contains(&ReaderConsumer::Room));
        assert!(tg.reader_is_running(id));
    }
    assert!(h.captures.is_open(&id.to_string()));
    h.release_all(id).await;
    h.close().await;
}

mod interference;
mod prompt;
mod restart;
