use super::co_managed_tests::*;
use super::*;
use crate::capture::key::Cut;
use crate::capture::registry::CaptureRegistry;
use crate::network::OutboundNetwork;
use crate::pty::backend::PtyBackend;
use crate::telegram::manager::{
    OutputSenderMap, ReaderConsumer, TelegramBridgeManager, TelegramBridgeState,
};
use std::collections::HashMap;

pub(crate) struct Harness {
    pub(crate) app: tauri::App<tauri::test::MockRuntime>,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pub(crate) captures: Arc<CaptureRegistry>,
    projects_dir: PathBuf,
}

pub(crate) fn harness(fixture: &RoomFixture) -> Harness {
    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let captures = Arc::new(CaptureRegistry::new());
    let senders: OutputSenderMap = Arc::new(Mutex::new(HashMap::new()));
    let bridge_state: TelegramBridgeState = Arc::new(tokio::sync::Mutex::new(
        TelegramBridgeManager::with_captures(senders, Arc::clone(&captures)),
    ));
    let projects_dir = fixture.temp_path().join("claude-projects");
    std::fs::create_dir_all(&projects_dir).expect("projects dir");
    // The production `co_managed_set_enabled` command canonicalises the
    // room root and proves it belongs to a registered project, so the
    // phase-2 fixture root is registered exactly as a project row would.
    let mut settings = settings_with_key();
    settings.project_paths = vec![fixture
        .temp_path()
        .join("project-a")
        .to_string_lossy()
        .into_owned()];
    let app = tauri::test::mock_builder()
        .manage(crate::config::settings::new_settings_state(settings))
        .manage(Arc::clone(&manager))
        .manage(bridge_state)
        .manage(OutboundNetwork::new_for_tests(1))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("build phase 4 supervisor test app");
    Harness {
        app,
        manager,
        captures,
        projects_dir,
    }
}

impl Harness {
    pub(crate) async fn session_in(&self, cwd: &std::path::Path) -> Uuid {
        let id = add_session_for_tests(
            &self.manager,
            cwd,
            SessionBackendKind::LocalProcess,
            CodingAgentKind::Claude,
        )
        .await;
        self.manager
            .read()
            .await
            .set_resolved_claude_projects_dir(id, Some(self.projects_dir.clone()))
            .await;
        id
    }

    async fn codex_session_in(&self, cwd: &std::path::Path, home: &std::path::Path) -> Uuid {
        let id = add_session_for_tests(
            &self.manager,
            cwd,
            SessionBackendKind::LocalProcess,
            CodingAgentKind::Codex,
        )
        .await;
        self.manager
            .read()
            .await
            .set_profile_metadata(
                id,
                None,
                None,
                Vec::new(),
                false,
                None,
                None,
                Some(home.to_string_lossy().into_owned()),
                None,
            )
            .await;
        id
    }

    async fn bridge(&self) -> tauri::State<'_, TelegramBridgeState> {
        self.app.handle().state::<TelegramBridgeState>()
    }

    /// #2525 set the two global Co-managed inputs in the live settings.
    pub(crate) async fn set_global_co_managed(&self, enabled: bool, key: &str) {
        let settings = self.app.handle().state::<SettingsState>();
        let mut guard = settings.write().await;
        guard.co_managed_enabled = enabled;
        guard.jev_api_key = key.to_string();
        drop(guard);
    }
}

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
    super::detach_response_close_telegram(harness.app.handle(), id)
        .await
        .unwrap();
    {
        let telegram = harness.bridge().await;
        let telegram = telegram.lock().await;
        assert!(!telegram.has_bridge(id), "actual transport detached");
        assert_eq!(telegram.test_detach_count(id), 1);
    }
    super::detach_response_close_telegram(harness.app.handle(), id)
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

// ── Test 9: restart transfers reader ownership to the new session UUID ──

#[tokio::test]
async fn issue_3145_restart_barrier_publishes_latest_prompt_in_first_event() {
    use tauri::Listener;
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let old = h
        .live_session(fixture.coordinator_path(), CodingAgentKind::Codex)
        .await;
    let manager = h.manager.read().await.clone();
    manager.set_last_prompt(old, "A".into()).await;
    let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let captured = events.clone();
    let listener = h.app.listen("session_created", move |event| {
        captured
            .lock()
            .unwrap()
            .push(serde_json::from_str(event.payload()).unwrap());
    });
    let barrier = restart_reader_seam::install_after_old_release(&old.to_string());
    let app = h.app.handle().clone();
    let sessions = h.manager.clone();
    let pty = h.pty.clone();
    let settings = h
        .app
        .state::<crate::config::settings::SettingsState>()
        .inner()
        .clone();
    let restart = tokio::spawn(async move {
        restart_session_inner_with_intent(
            &app,
            &sessions,
            &pty,
            &settings,
            old,
            None,
            None,
            Some(false),
            true,
            TrustedRestartIntent::User,
            None,
            crate::config::sessions_persistence::default_creation_gate_enforcement(),
        )
        .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        barrier.reached.notified(),
    )
    .await
    .unwrap();
    manager.set_last_prompt(old, "B".into()).await;
    barrier.release.notify_one();
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), restart)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.last_prompt.as_deref(), Some("B"));
    assert_eq!(
        manager.list_sessions().await[0].last_prompt.as_deref(),
        Some("B")
    );
    assert_eq!(
        crate::config::sessions_persistence::snapshot_sessions(&manager).await[0]
            .last_prompt
            .as_deref(),
        Some("B")
    );
    let observed = events.lock().unwrap().clone();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0]["id"], result.id);
    assert_eq!(observed[0]["lastPrompt"], "B");
    h.app.unlisten(listener);
    h.release_all(Uuid::parse_str(&result.id).unwrap()).await;
    h.close().await;
}

#[tokio::test]
async fn issue_3145_restart_fresh_durable_and_failed_spawn_never_inherit() {
    for mode in [0, 1, 2] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let h = restart_harness(&fixture);
        let old = h
            .live_session(fixture.coordinator_path(), CodingAgentKind::Codex)
            .await;
        let manager = h.manager.read().await.clone();
        manager.set_last_prompt(old, "A".into()).await;
        if mode == 1 {
            manager.set_start_fresh_on_restore(old, true).await;
        }
        if mode == 2 {
            h.backend.fail_spawn(2);
        }
        let result = h
            .restart_with(old, if mode == 0 { Some(true) } else { Some(false) })
            .await;
        if mode == 2 {
            assert!(result.is_err());
            assert!(manager.list_sessions().await.is_empty());
        } else {
            let result = result.unwrap();
            assert_eq!(result.last_prompt, None);
            h.release_all(Uuid::parse_str(&result.id).unwrap()).await;
        }
        h.close().await;
    }
}

#[tokio::test]
async fn issue_3145_restart_changed_agent_requested_or_effective_profile_abstains() {
    for mode in [0, 1, 2] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let h = restart_harness(&fixture);
        let old = h
            .live_session(fixture.coordinator_path(), CodingAgentKind::Codex)
            .await;
        let manager = h.manager.read().await.clone();
        manager.set_last_prompt(old, "A".into()).await;
        let row = manager.get_session(old).await.unwrap();
        if mode != 0 {
            manager
                .set_profile_metadata(
                    old,
                    if mode == 1 {
                        None
                    } else {
                        row.requested_profile.clone()
                    },
                    if mode == 2 {
                        Some("previous-effective".into())
                    } else {
                        row.effective_profile.clone()
                    },
                    Vec::new(),
                    false,
                    None,
                    None,
                    None,
                    None,
                )
                .await;
        }
        let settings = h.app.state::<crate::config::settings::SettingsState>();
        let result = restart_session_inner_with_intent(
            h.app.handle(),
            &h.manager,
            &h.pty,
            settings.inner(),
            old,
            (mode == 0).then(|| "claude".to_string()),
            row.requested_profile,
            Some(false),
            true,
            TrustedRestartIntent::User,
            None,
            crate::config::sessions_persistence::default_creation_gate_enforcement(),
        )
        .await
        .unwrap();
        assert_eq!(result.last_prompt, None, "incompatible domain {mode}");
        h.release_all(Uuid::parse_str(&result.id).unwrap()).await;
        h.close().await;
    }
}

#[tokio::test]
async fn issue_3145_restart_mirror_forced_fresh_never_inherits() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let old = h
        .live_session(fixture.coordinator_path(), CodingAgentKind::Codex)
        .await;
    h.manager
        .read()
        .await
        .set_last_prompt(old, "A".into())
        .await;
    let cwd = crate::path_utils::normalize_windows_verbatim_path(
        &fixture.coordinator_path().to_string_lossy(),
    );
    let clocks: crate::config::coordinator_clocks::CoordinatorClocksState =
        Arc::new(Mutex::new(Default::default()));
    clocks.lock().unwrap().mark_start_fresh(
        &crate::config::teams::agent_fqn_from_path(&cwd),
        chrono::Utc::now(),
    );
    assert!(h.app.manage(clocks));
    coordinator_create_seam::force(&cwd);
    let result = h.restart(old).await.unwrap();
    coordinator_create_seam::unforce(&cwd);
    assert_eq!(result.last_prompt, None);
    assert!(
        h.manager
            .read()
            .await
            .get_session(Uuid::parse_str(&result.id).unwrap())
            .await
            .unwrap()
            .start_fresh_on_restore
    );
    h.release_all(Uuid::parse_str(&result.id).unwrap()).await;
    h.close().await;
}

#[tokio::test]
async fn issue_3145_manual_auto_close_reload_retains_input_during_teardown() {
    for source in [DestructionSource::ManualClose, DestructionSource::AutoClose] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let h = restart_harness(&fixture);
        let old = h
            .live_session(fixture.coordinator_path(), CodingAgentKind::Codex)
            .await;
        let manager = h.manager.read().await.clone();
        manager.set_last_prompt(old, "A".into()).await;
        let barrier = restart_reader_seam::install_after_old_release(&format!("destroy:{old}"));
        let app = h.app.handle().clone();
        let destroy = tokio::spawn(async move {
            destroy_sessions_with_source(&app, vec![old], source, false).await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            barrier.reached.notified(),
        )
        .await
        .unwrap();
        manager.set_last_prompt(old, "B".into()).await;
        barrier.release.notify_one();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), destroy)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(outcome.succeeded(old));
        assert!(manager.list_sessions().await.is_empty());
        let checkpoint = tempfile::tempdir().unwrap();
        crate::config::sessions_persistence::persist_current_state_to_dir_result(
            &manager,
            checkpoint.path(),
        )
        .await
        .unwrap();
        let next = restart_harness(&fixture);
        let next_manager = next.manager.read().await.clone();
        crate::config::sessions_persistence::initialize_prompt_retention_in_dir(
            &next_manager,
            checkpoint.path(),
        )
        .await;
        assert!(next_manager.list_sessions().await.is_empty());
        let cwd = fixture.coordinator_path().to_string_lossy().into_owned();
        let spawn = build_configured_agent_spawn_for_cwd(&next.settings, "codex", &cwd, None)
            .unwrap()
            .unwrap();
        let resumed = create_session_inner(
            next.app.handle(),
            &next.manager,
            &next.pty,
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            cwd,
            Some("resumed".into()),
            Some("codex".into()),
            None,
            true,
            Vec::new(),
            false,
            Some(spawn),
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        .unwrap();
        assert_eq!(resumed.last_prompt.as_deref(), Some("B"));
        next.release_all(Uuid::parse_str(&resumed.id).unwrap())
            .await;
        h.close().await;
        next.close().await;
    }
}

async fn issue_3145_open_claude_without_forced_fresh(
    harness: &RestartHarness,
    cwd: &std::path::Path,
) -> SessionInfo {
    let cwd = cwd.to_string_lossy().into_owned();
    let spawn = build_configured_agent_spawn_for_cwd(&harness.settings, "claude", &cwd, None)
        .unwrap()
        .unwrap();
    create_session_inner(
        harness.app.handle(),
        &harness.manager,
        &harness.pty,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("initial conversation".into()),
        Some("claude".into()),
        None,
        true,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn issue_3145_initial_claude_launch_retains_input_after_close_reload_resume() {
    for accepted in ["A", ""] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let first = restart_harness(&fixture);
        let projects =
            first.claude_projects_dir(&first.claude_config_dir, fixture.coordinator_path());
        assert!(
            !projects.exists(),
            "first conversation has no resume probe directory"
        );
        let initial =
            issue_3145_open_claude_without_forced_fresh(&first, fixture.coordinator_path()).await;
        let initial_args = initial
            .effective_shell_args
            .as_deref()
            .unwrap_or(&initial.shell_args);
        assert!(!initial_args
            .iter()
            .flat_map(|arg| arg.split_whitespace())
            .map(|arg| arg.trim_matches('"'))
            .any(|arg| arg == "--continue" || arg == "-c"));
        let id = Uuid::parse_str(&initial.id).unwrap();
        let manager = first.manager.read().await.clone();
        let retained = manager.retained_prompt_snapshot().await.unwrap();
        assert_eq!(retained.len(), 1);
        assert!(
            !retained.values().next().unwrap().blocked,
            "AC-3145-006: natural first launch must not poison retention"
        );
        manager.set_last_prompt(id, accepted.into()).await;
        destroy_session_inner(first.app.handle(), id).await.unwrap();
        assert!(manager.list_sessions().await.is_empty());
        let checkpoint = tempfile::tempdir().unwrap();
        crate::config::sessions_persistence::persist_current_state_to_dir_result(
            &manager,
            checkpoint.path(),
        )
        .await
        .unwrap();
        let next = restart_harness(&fixture);
        let next_manager = next.manager.read().await.clone();
        crate::config::sessions_persistence::initialize_prompt_retention_in_dir(
            &next_manager,
            checkpoint.path(),
        )
        .await;
        assert!(next_manager.list_sessions().await.is_empty());
        // Only the provider's pre-existing launch probe is mocked. The PTY,
        // create, close, sidecar reload and publication paths are production.
        std::fs::create_dir_all(
            next.claude_projects_dir(&next.claude_config_dir, fixture.coordinator_path()),
        )
        .unwrap();
        let resumed =
            issue_3145_open_claude_without_forced_fresh(&next, fixture.coordinator_path()).await;
        let resume_args = resumed
            .effective_shell_args
            .as_deref()
            .unwrap_or(&resumed.shell_args);
        assert!(resume_args
            .iter()
            .flat_map(|arg| arg.split_whitespace())
            .any(|arg| arg.trim_matches('"') == "--continue"));
        assert_eq!(
            resumed.last_prompt.as_deref(),
            Some(accepted),
            "AC-3145-006: input survives close and reload into effective resume"
        );
        next.release_all(Uuid::parse_str(&resumed.id).unwrap())
            .await;
        first.close().await;
        next.close().await;
    }
}

#[tokio::test]
async fn issue_3145_configured_external_resume_stays_blocked_after_close_reload() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let mut first = restart_harness(&fixture);
    first.set_claude_command("claude --continue").await;
    let initial =
        issue_3145_open_claude_without_forced_fresh(&first, fixture.coordinator_path()).await;
    let id = Uuid::parse_str(&initial.id).unwrap();
    let manager = first.manager.read().await.clone();
    manager.set_last_prompt(id, "unattributed".into()).await;
    assert!(
        manager
            .retained_prompt_snapshot()
            .await
            .unwrap()
            .values()
            .next()
            .unwrap()
            .blocked
    );
    destroy_session_inner(first.app.handle(), id).await.unwrap();
    let checkpoint = tempfile::tempdir().unwrap();
    crate::config::sessions_persistence::persist_current_state_to_dir_result(
        &manager,
        checkpoint.path(),
    )
    .await
    .unwrap();
    let next = restart_harness(&fixture);
    let next_manager = next.manager.read().await.clone();
    crate::config::sessions_persistence::initialize_prompt_retention_in_dir(
        &next_manager,
        checkpoint.path(),
    )
    .await;
    std::fs::create_dir_all(
        next.claude_projects_dir(&next.claude_config_dir, fixture.coordinator_path()),
    )
    .unwrap();
    let resumed =
        issue_3145_open_claude_without_forced_fresh(&next, fixture.coordinator_path()).await;
    assert_eq!(resumed.last_prompt, None);
    next.release_all(Uuid::parse_str(&resumed.id).unwrap())
        .await;
    first.close().await;
    next.close().await;
}

const PHASE4_BOT_ID: &str = "phase4-bot";

/// The heavy harness test 9 needs: the scripted-session app, extended with
/// the single capture registry, the outbound network and the phase-2 room
/// fixture paths the replacement's Room raise resolves against.
///
/// Both configured agents carry the environment row production reads for
/// the transcript memo (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`), so the old
/// session and the replacement resolve their reader paths from settings
/// exactly as a real restart does.
struct RestartHarness {
    app: tauri::App<tauri::test::MockRuntime>,
    manager: Arc<tokio::sync::RwLock<SessionManager>>,
    pty: Arc<Mutex<crate::pty::manager::PtyManager>>,
    backend: Arc<crate::commands::session::tests::ScriptedSpawnBackend>,
    captures: Arc<CaptureRegistry>,
    network: OutboundNetwork,
    settings: AppSettings,
    claude_config_dir: PathBuf,
    codex_home: PathBuf,
}

fn claude_env_row(dir: &std::path::Path) -> crate::config::settings::CodingAgentEnv {
    crate::config::settings::CodingAgentEnv {
        key: "CLAUDE_CONFIG_DIR".to_string(),
        value: dir.to_string_lossy().into_owned(),
        source: Default::default(),
        enabled: true,
    }
}

fn codex_env_row(dir: &std::path::Path) -> crate::config::settings::CodingAgentEnv {
    crate::config::settings::CodingAgentEnv {
        key: "CODEX_HOME".to_string(),
        value: dir.to_string_lossy().into_owned(),
        source: Default::default(),
        enabled: true,
    }
}

fn restart_harness(fixture: &RoomFixture) -> RestartHarness {
    let claude_config_dir = fixture.temp_path().join("claude-home");
    let codex_home = fixture.temp_path().join("codex-home");
    std::fs::create_dir_all(&claude_config_dir).expect("claude config dir");
    std::fs::create_dir_all(&codex_home).expect("codex home");

    let mut settings = super::tests::test_settings();
    settings.jev_api_key = "test-key".to_string();
    settings.co_managed_enabled = true;
    settings.agents[0].envs = vec![claude_env_row(&claude_config_dir)];
    settings.agents[1].envs = vec![codex_env_row(&codex_home)];
    settings.telegram_bots = vec![crate::telegram::types::TelegramBotConfig {
        id: PHASE4_BOT_ID.to_string(),
        label: "Phase 4 bot".to_string(),
        token: "test-token".to_string(),
        chat_id: 7,
        color: "#229ED9".to_string(),
    }];
    // The creation gate must accept the orchestrator replica cwd: the
    // phase-2 fixture lives under `<temp>/project-a`, so registering that
    // root is what a real AC project row contributes.
    settings.project_paths = vec![fixture
        .temp_path()
        .join("project-a")
        .to_string_lossy()
        .into_owned()];

    let manager = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(crate::commands::session::tests::ScriptedSpawnBackend::default());
    let pty = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = crate::commands::session::tests::session_test_app_with_store(
        settings.clone(),
        Arc::clone(&manager),
        Arc::clone(&pty),
        crate::commands::session::tests::SessionTestStoreState::Ready,
    );
    let captures = app.state::<Arc<CaptureRegistry>>().inner().clone();
    let network = app.state::<OutboundNetwork>().inner().clone();
    RestartHarness {
        app,
        manager,
        pty,
        backend,
        captures,
        network,
        settings,
        claude_config_dir,
        codex_home,
    }
}

impl RestartHarness {
    /// A live scripted session for `kind` whose transcript memo comes from
    /// the configured agent environment, exactly as production resolves it.
    async fn live_session(&self, cwd: &std::path::Path, kind: CodingAgentKind) -> Uuid {
        let info = self.live_session_info(cwd, kind).await;
        Uuid::parse_str(&info.id).expect("session id")
    }

    /// [`Self::live_session`], keeping the created `SessionInfo`: its
    /// `effective_shell_args` carry the transcript id the spawn minted
    /// (#2454), which is NOT the session UUID.
    async fn live_session_info(&self, cwd: &std::path::Path, kind: CodingAgentKind) -> SessionInfo {
        let (agent_id, label) = match kind {
            CodingAgentKind::Claude => ("claude", "Claude Code"),
            CodingAgentKind::Codex => ("codex", "Codex"),
            _ => panic!("phase 4 restart fixture supports Claude and Codex only"),
        };
        let cwd_str = cwd.to_string_lossy().into_owned();
        let spawn = crate::commands::session::build_configured_agent_spawn_for_cwd(
            &self.settings,
            agent_id,
            &cwd_str,
            None,
        )
        .expect("build configured agent spawn")
        .expect("configured agent exists in test settings");
        crate::commands::session::create_session_inner(
            self.app.handle(),
            &self.manager,
            &self.pty,
            spawn.shell.clone(),
            spawn.shell_args.clone(),
            cwd_str,
            Some("phase 4 restart fixture".to_string()),
            Some(agent_id.to_string()),
            Some(label.to_string()),
            true,
            Vec::new(),
            true,
            Some(spawn),
            None,
            None,
            CreateSelectionIntent::User,
        )
        .await
        .expect("create phase 4 scripted session")
    }

    async fn bridge(&self) -> tauri::State<'_, TelegramBridgeState> {
        self.app.handle().state::<TelegramBridgeState>()
    }

    /// Drive the production restart entry point (the innermost of the
    /// three, so all three inherit it) and return its error, if any.
    async fn restart(&self, id: Uuid) -> Result<SessionInfo, String> {
        self.restart_with(id, Some(false)).await
    }

    /// The restart button's default: `skip_auto_resume` unset resolves to
    /// FRESH, so the replacement mints a new transcript id (#2454).
    async fn restart_fresh(&self, id: Uuid) -> Result<SessionInfo, String> {
        self.restart_with(id, None).await
    }

    async fn restart_with(
        &self,
        id: Uuid,
        skip_auto_resume: Option<bool>,
    ) -> Result<SessionInfo, String> {
        let settings = self.app.state::<crate::config::settings::SettingsState>();
        crate::commands::session::restart_session_inner_with_intent(
            self.app.handle(),
            &self.manager,
            &self.pty,
            settings.inner(),
            id,
            None,
            None,
            skip_auto_resume,
            true,
            TrustedRestartIntent::User,
            None,
            crate::config::sessions_persistence::default_creation_gate_enforcement(),
        )
        .await
    }

    async fn snapshot(
        &self,
        id: Uuid,
    ) -> (Option<u64>, std::collections::BTreeSet<ReaderConsumer>) {
        let tg = self.bridge().await;
        let tg = tg.lock().await;
        (tg.reader_id(id), tg.reader_demands(id))
    }

    async fn release_all(&self, id: Uuid) {
        let shutdown = {
            let tg = self.bridge().await;
            let mut tg = tg.lock().await;
            tg.reader_release_all(id)
        };
        if let Some(shutdown) = shutdown {
            shutdown.abort_now();
        }
    }

    /// Point the configured Claude agent at `dir`: the next restart builds
    /// its replacement spawn from the live settings, so the replacement's
    /// transcript memo moves with it (a "changed path" restart).
    async fn set_claude_config_dir(&self, dir: &std::path::Path) {
        let settings = self.app.state::<crate::config::settings::SettingsState>();
        settings.write().await.agents[0].envs = vec![claude_env_row(dir)];
    }

    /// Configure the Claude agent's command line, in the harness copy that
    /// creates sessions AND in the live settings a restart re-resolves
    /// argv from.
    async fn set_claude_command(&mut self, command: &str) {
        self.settings.agents[0].command = command.to_string();
        let settings = self.app.state::<crate::config::settings::SettingsState>();
        settings.write().await.agents[0].command = command.to_string();
    }

    async fn set_codex_home(&self, dir: &std::path::Path) {
        let settings = self.app.state::<crate::config::settings::SettingsState>();
        settings.write().await.agents[1].envs = vec![codex_env_row(dir)];
    }

    /// The Claude transcript directory the configured memo resolves to.
    fn claude_projects_dir(&self, config_dir: &std::path::Path, cwd: &std::path::Path) -> PathBuf {
        config_dir
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(
                &cwd.to_string_lossy(),
            ))
    }

    fn message_permits(&self) -> usize {
        self.network
            .acquired_labels_for_tests()
            .iter()
            .filter(|label| **label == "telegram.send_message")
            .count()
    }

    async fn attach_bot(&self, id: Uuid) -> crate::telegram::types::BridgeInfo {
        crate::commands::telegram::attach_telegram_bot_by_id(self.app.handle(), id, PHASE4_BOT_ID)
            .await
            .expect("attach the configured bot")
    }

    async fn close(self) {
        crate::commands::session::tests::close_test_coordinator(&self.app).await;
    }
}

/// The transcript id a spawn minted (#2454), read back from its effective
/// argv; `None` when no `--session-id` was injected.
fn minted_transcript_id(info: &SessionInfo) -> Option<String> {
    crate::commands::session::transcript_id_from_args(
        info.effective_shell_args.as_deref().unwrap_or(&[]),
    )
}

/// Move a file's mtime `secs` into the past, so it is never the newest.
fn backdate(path: &std::path::Path, secs: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open for backdate")
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(secs))
        .expect("backdate mtime");
}

/// One Claude assistant line with an explicit timestamp.
fn claude_line(body: &str, ts: chrono::DateTime<chrono::Utc>) -> String {
    serde_json::json!({
        "type": "assistant",
        "timestamp": ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "message": {"content": [{"type": "text", "text": body}]}
    })
    .to_string()
}

fn append_claude(path: &std::path::Path, body: &str) {
    use std::io::Write as _;
    let line = claude_line(body, chrono::Utc::now() - chrono::Duration::seconds(1));
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("append transcript");
    writeln!(f, "{line}").expect("write transcript line");
    f.sync_all().expect("sync transcript");
}

/// A >64 KiB Claude transcript whose only assistant record is at the very
/// start, far above the 64 KiB tail window (test 9's backfill probe).
/// Returns the byte length written.
fn large_claude_transcript(path: &std::path::Path, body: &str) -> u64 {
    use std::io::Write as _;
    let first = claude_line(body, chrono::Utc::now() - chrono::Duration::seconds(1));
    let mut f = std::fs::File::create(path).expect("create transcript");
    writeln!(f, "{first}").expect("first line");
    let filler = r#"{"type":"user","message":{"content":[{"type":"text","text":"filler"}]}}"#;
    let mut written = first.len() as u64 + 1;
    while written < 70 * 1024 {
        writeln!(f, "{filler}").expect("filler line");
        written += filler.len() as u64 + 1;
    }
    f.sync_all().expect("sync transcript");
    written
}

fn append_codex(path: &std::path::Path, body: &str) {
    use std::io::Write as _;
    let line = serde_json::json!({
        "timestamp": (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "assistant",
            "phase": "final_answer",
            "content": [{"type": "output_text", "text": body}]
        }
    })
    .to_string();
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("append rollout");
    writeln!(f, "{line}").expect("write rollout line");
    f.sync_all().expect("sync rollout");
}

/// Poll the slot until it holds a record the predicate accepts.
async fn wait_for_slot_record(
    slot: &crate::capture::sink::CaptureSlot,
    timeout: std::time::Duration,
    mut accept: impl FnMut(&Arc<crate::capture::record::CapturedRecord>) -> bool,
) -> (u64, Arc<crate::capture::record::CapturedRecord>) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let state = slot.snapshot();
        if let crate::capture::sink::SlotValue::Valid(record) = state.value {
            if accept(&record) {
                return (state.seq, record);
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no matching slot record within the budget"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Write one Codex rollout with a `session_meta` line and a single
/// `final_answer` record carrying `body` at `ts`.
fn write_codex_rollout(
    home: &std::path::Path,
    cwd: &str,
    name: &str,
    body: &str,
    ts: chrono::DateTime<chrono::Utc>,
) -> std::path::PathBuf {
    use std::io::Write as _;
    let now = chrono::Utc::now();
    let day = home
        .join("sessions")
        .join(format!("{:04}", now.format("%Y")))
        .join(format!("{:02}", now.format("%m")))
        .join(format!("{:02}", now.format("%d")));
    std::fs::create_dir_all(&day).expect("codex day dir");
    let path = day.join(name);
    let meta = serde_json::json!({
        "timestamp": now.to_rfc3339(),
        "type": "session_meta",
        "payload": {"id": name, "cwd": cwd, "timestamp": now.to_rfc3339()}
    });
    let record = serde_json::json!({
        "timestamp": ts.to_rfc3339(),
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "assistant",
            "phase": "final_answer",
            "content": [{"type": "output_text", "text": body}]
        }
    });
    let mut f = std::fs::File::create(&path).expect("rollout");
    writeln!(f, "{meta}").expect("meta line");
    writeln!(f, "{record}").expect("record line");
    f.sync_all().expect("sync rollout");
    path
}

async fn wait_for_message_permits(h: &RestartHarness, at_least: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while h.message_permits() < at_least {
        assert!(
            std::time::Instant::now() < deadline,
            "no telegram.send_message attempt within the budget"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

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

/// Test 9 (old-id raise, timing variant 2): a raise paused across the
/// restart and resumed **after** the old row is gone installs for a dead
/// id; its own post-install liveness recheck self-cleans it.
#[tokio::test]
async fn an_old_id_raise_resumed_after_old_row_removal_self_cleans() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let old = h
        .live_session(fixture.coordinator_path(), CodingAgentKind::Claude)
        .await;
    let old_key = old.to_string();

    let barrier = crate::commands::telegram::reader_demand_seam::install_before_install(&old_key);
    let app = h.app.handle().clone();
    let raise = tokio::spawn(async move {
        crate::commands::telegram::raise_reader_demand(&app, old, ReaderConsumer::Room, None).await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        barrier.reached.notified(),
    )
    .await
    .expect("the old-id raise reaches the pre-install seam");

    // The restart runs to completion while the raise is still paused.
    let restarted = h.restart(old).await.expect("restart succeeds");
    let new = Uuid::parse_str(&restarted.id).expect("replacement id");
    assert_ne!(new, old);
    assert!(h.snapshot(old).await.0.is_none());
    assert!(!h.captures.is_open(&old_key));

    // Resumed after old-row removal: the raise installs, then the liveness
    // recheck releases the reader it just created.
    barrier.release.notify_one();
    let raised = tokio::time::timeout(std::time::Duration::from_secs(10), raise)
        .await
        .expect("the raise resumes")
        .expect("the raise task joins");
    assert!(!raised, "a raise resumed after removal must self-clean");
    let (reader, demands) = h.snapshot(old).await;
    assert_eq!(reader, None);
    assert!(demands.is_empty());
    assert!(!h.captures.is_open(&old_key));
    assert!(h.captures.slot(&old_key).is_none());

    // Exactly one reader exists, and it is the replacement's.
    assert!(h.snapshot(new).await.0.is_some());
    assert!(h.captures.is_open(&new.to_string()));
    h.release_all(new).await;
    h.close().await;
}

/// Test 9 (old-id raise, timing variant 1): a raise **installed after the
/// early release but before the commit** is swept by the post-commit
/// release. This is the deterministic proof that the success path cleans
/// the old id even when a demand lands in the window.
#[tokio::test]
async fn an_old_id_raise_installed_before_commit_is_swept_after_it() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let old = h
        .live_session(fixture.coordinator_path(), CodingAgentKind::Claude)
        .await;
    let old_key = old.to_string();

    let raise_barrier =
        crate::commands::telegram::reader_demand_seam::install_before_install(&old_key);
    let restart_barrier =
        crate::commands::session::restart_reader_seam::install_after_old_release(&old_key);

    let app = h.app.handle().clone();
    let raise = tokio::spawn(async move {
        crate::commands::telegram::raise_reader_demand(&app, old, ReaderConsumer::Room, None).await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        raise_barrier.reached.notified(),
    )
    .await
    .expect("the old-id raise reaches the pre-install seam");

    let restart = {
        let app = h.app.handle().clone();
        let manager = Arc::clone(&h.manager);
        let pty = Arc::clone(&h.pty);
        let settings = h
            .app
            .state::<crate::config::settings::SettingsState>()
            .inner()
            .clone();
        tokio::spawn(async move {
            crate::commands::session::restart_session_inner_with_intent(
                &app,
                &manager,
                &pty,
                &settings,
                old,
                None,
                None,
                Some(false),
                true,
                TrustedRestartIntent::User,
                None,
                crate::config::sessions_persistence::default_creation_gate_enforcement(),
            )
            .await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        restart_barrier.reached.notified(),
    )
    .await
    .expect("the restart reaches the post-release seam");

    // Install inside the window: the old row is still live, so the raise's
    // liveness recheck passes and the reader stays installed.
    raise_barrier.release.notify_one();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.snapshot(old).await.0.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the paused raise must install before the commit"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(h.captures.is_open(&old_key));

    restart_barrier.release.notify_one();
    let restarted = restart
        .await
        .expect("join restart")
        .expect("restart succeeds");
    let new = Uuid::parse_str(&restarted.id).expect("replacement id");
    let raised = tokio::time::timeout(std::time::Duration::from_secs(10), raise)
        .await
        .expect("the raise resumes")
        .expect("the raise task joins");
    assert!(
        raised,
        "installed before the commit, the raise returns true"
    );

    // The post-commit release swept the old id.
    let (reader, demands) = h.snapshot(old).await;
    assert_eq!(reader, None, "the post-commit release sweeps the old id");
    assert!(demands.is_empty());
    assert!(!h.captures.is_open(&old_key));
    assert!(h.captures.slot(&old_key).is_none());
    // Exactly one reader, on the new id.
    assert!(h.snapshot(new).await.0.is_some());
    assert!(h.captures.is_open(&new.to_string()));
    h.release_all(new).await;
    h.close().await;
}

/// Test 9 (failure after runtime loss): when the replacement spawn fails
/// after the old PTY is gone, the old reader and its slot must close.
#[tokio::test]
async fn a_spawn_failure_after_runtime_loss_closes_the_old_reader() {
    for kind in [CodingAgentKind::Claude, CodingAgentKind::Codex] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let h = restart_harness(&fixture);
        let old = h.live_session(fixture.coordinator_path(), kind).await;
        assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), old).await);
        let old_key = old.to_string();
        assert!(h.captures.is_open(&old_key));

        // Session create was spawn #1; fail the replacement spawn.
        h.backend.fail_spawn(2);
        let error = h
            .restart(old)
            .await
            .expect_err("replacement spawn must fail");
        assert!(
            error.contains("synthetic scripted spawn failure"),
            "{error}"
        );

        assert!(h.snapshot(old).await.0.is_none());
        assert!(!h.captures.is_open(&old_key));
        assert!(h.captures.slot(&old_key).is_none());
        assert!(h.manager.read().await.get_session(old).await.is_none());
        h.close().await;
    }
}

/// Test 9 (dormant failure): a failed replacement for a dormant old session
/// retains the old row, reader and slot for retry.
#[tokio::test]
async fn a_dormant_restart_failure_keeps_the_old_reader_for_retry() {
    for kind in [CodingAgentKind::Claude, CodingAgentKind::Codex] {
        let fixture = room_fixture();
        configure_room(fixture.room_path(), true);
        let h = restart_harness(&fixture);
        let old = h.live_session(fixture.coordinator_path(), kind).await;
        h.backend.kill(old).expect("kill the dormant old");
        h.app
            .state::<crate::session::selection::SelectionCoordinator>()
            .container_lifecycle_sender()
            .route_lost(old, 23)
            .await
            .expect("reconcile the dormant route");
        assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), old).await);
        let old_key = old.to_string();

        h.backend.fail_spawn(2);
        let error = h
            .restart(old)
            .await
            .expect_err("dormant replacement must fail");
        assert!(
            error.contains("synthetic scripted spawn failure"),
            "{error}"
        );

        let retained = h
            .manager
            .read()
            .await
            .get_session(old)
            .await
            .expect("the dormant row is retained");
        assert!(matches!(retained.status, SessionStatus::Exited(_)));
        assert!(h.snapshot(old).await.0.is_some(), "the old reader survives");
        assert!(h.captures.is_open(&old_key), "the old slot survives");
        h.release_all(old).await;
        h.close().await;
    }
}

/// Test 9 (preflight): a failure before teardown keeps the old runtime, so
/// its reader, demand set and slot survive untouched.
#[tokio::test]
async fn a_preflight_failure_keeps_the_old_reader_running() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = restart_harness(&fixture);
    let cwd = fixture.coordinator_path();
    let old = h.live_session(cwd, CodingAgentKind::Claude).await;
    assert!(raise_room_reader_demand_in(h.app.handle(), fixture.room_path(), old).await);
    let old_key = old.to_string();
    let before = h.snapshot(old).await;

    h.app
        .state::<crate::config::settings::SettingsState>()
        .write()
        .await
        .archived_project_paths = vec![cwd.to_string_lossy().into_owned()];
    let error = h
        .restart(old)
        .await
        .expect_err("archived project blocks restart");
    assert!(
        error.contains("Cannot start a session in archived project"),
        "{error}"
    );

    assert_eq!(h.snapshot(old).await, before, "the old reader is untouched");
    assert!(h.captures.is_open(&old_key));
    assert!(h.captures.slot(&old_key).is_some());
    h.release_all(old).await;
    h.close().await;
}

/// Test 19: **create/destroy race.** The detached create-time Room raise is
/// paused after eligibility resolution; destroying the session during that
/// pause leaves no reader task, no demand and no open capture slot.
#[tokio::test]
async fn a_destroy_during_the_detached_room_raise_leaves_no_reader_state() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    let key = id.to_string();

    let barrier = crate::commands::telegram::reader_demand_seam::install_before_install(&key);
    let app = h.app.handle().clone();
    let room = fixture.room_path().to_path_buf();
    let raise = tokio::spawn(async move { raise_room_reader_demand_in(&app, &room, id).await });

    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        barrier.reached.notified(),
    )
    .await
    .expect("the detached raise reaches the pause seam");

    // Destroy wins the race while the raise is paused, exactly as the
    // production destroy path does: remove the row, then release the
    // session's demands.
    h.manager
        .read()
        .await
        .destroy_session(id)
        .await
        .expect("destroy session");
    crate::commands::telegram::release_all_reader_demands(h.app.handle(), id).await;

    barrier.release.notify_one();
    let raised = tokio::time::timeout(std::time::Duration::from_secs(10), raise)
        .await
        .expect("the raise resumes")
        .expect("the raise task joins");
    assert!(
        !raised,
        "a raise whose session died must not leave a running reader"
    );

    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(!tg.reader_is_running(id), "the reader is discarded");
        assert!(tg.reader_demands(id).is_empty(), "no demand remains");
    }
    assert!(!h.captures.is_open(&key), "the capture slot is closed");
    assert!(h.captures.slot(&key).is_none());
}

// ── #2516: the raise/disable race on the Room demand ─────────────────

/// Spawn the Room raise for `id`, wait until it parks at the armed seam
/// (`reached`), run `interfere`, resume it (`release`) and return what the
/// raise answered.
pub(crate) async fn race_room_raise<Fut>(
    h: &Harness,
    room: &std::path::Path,
    id: Uuid,
    reached: &tokio::sync::Notify,
    release: &tokio::sync::Notify,
    interfere: Fut,
) -> bool
where
    Fut: std::future::Future<Output = ()>,
{
    let app = h.app.handle().clone();
    let room = room.to_path_buf();
    let raise = tokio::spawn(async move { raise_room_reader_demand_in(&app, &room, id).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), reached.notified())
        .await
        .expect("the Room raise reaches the armed seam");
    interfere.await;
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(10), raise)
        .await
        .expect("the Room raise resumes")
        .expect("the Room raise task joins")
}

/// The production disable toggle for the fixture's room.
async fn disable_room(h: &Harness, fixture: &RoomFixture) {
    let config = crate::commands::co_managed::co_managed_set_enabled(
        h.app.handle().clone(),
        fixture.room_path().to_string_lossy().into_owned(),
        false,
    )
    .await
    .expect("disabling the flag succeeds");
    assert!(!config.enabled);
}

/// No Room demand, no reader and no capture slot for `id`.
pub(crate) async fn assert_no_room_reader_state_for(h: &Harness, id: Uuid) {
    {
        let tg = h.bridge().await;
        let tg = tg.lock().await;
        assert!(
            tg.reader_demands(id).is_empty(),
            "no Room demand may be installed after the release"
        );
        assert!(!tg.reader_is_running(id), "no reader may be spawned");
    }
    assert!(
        !h.captures.is_open(&id.to_string()),
        "no capture slot may be opened"
    );
}

/// T1's assertions: the aborted raise left no demand, no reader, no slot.
pub(crate) async fn assert_no_room_reader_state(h: &Harness, raised: bool, id: Uuid) {
    assert!(!raised, "a raise overtaken by a release must answer false");
    assert_no_room_reader_state_for(h, id).await;
}

/// #2516 T1, the positive control: a disable landing between readiness
/// and install (slow path, no reader anywhere) installs no Room demand.
#[tokio::test]
async fn disabling_between_readiness_and_install_installs_no_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let barrier = room_raise_seam::install_before_add(&id.to_string());
    let raised = race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        disable_room(&h, &fixture),
    )
    .await;
    assert_no_room_reader_state(&h, raised, id).await;
}

/// #2516 T2 (fast path): the session already runs a reader for its Bot
/// demand, so the disable removes **nothing**. The raise must still see it
/// and leave the reader Room-free.
#[tokio::test]
async fn disabling_between_readiness_and_add_leaves_a_bot_reader_room_free() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;
    assert!(
        crate::commands::telegram::raise_reader_demand(
            h.app.handle(),
            id,
            ReaderConsumer::Bot,
            None,
        )
        .await,
        "the Bot demand starts a reader"
    );

    let barrier = room_raise_seam::install_before_add(&id.to_string());
    race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        disable_room(&h, &fixture),
    )
    .await;

    let tg = h.bridge().await;
    let tg = tg.lock().await;
    assert_eq!(
        tg.reader_demands(id).into_iter().collect::<Vec<_>>(),
        vec![ReaderConsumer::Bot],
        "the disable must keep the Room demand off a Bot reader"
    );
    assert!(tg.reader_is_running(id), "the Bot reader keeps running");
}

/// #2516 T3, the negative control: the same pause with **no**
/// interference still installs the Room demand.
#[tokio::test]
async fn a_raise_with_no_interference_still_installs_the_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let barrier = room_raise_seam::install_before_add(&id.to_string());
    let raised = race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        async {},
    )
    .await;
    assert!(raised, "an undisturbed raise answers true");
    let tg = h.bridge().await;
    let tg = tg.lock().await;
    assert!(
        tg.reader_demands(id).contains(&ReaderConsumer::Room),
        "an undisturbed raise installs the Room demand"
    );
}

/// #2516 T4: a destroy-style `release_all` landing between readiness and
/// install installs no Room demand.
#[tokio::test]
async fn destroying_between_readiness_and_install_installs_no_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let barrier = room_raise_seam::install_before_add(&id.to_string());
    let raised = race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        crate::commands::telegram::release_all_reader_demands(h.app.handle(), id),
    )
    .await;
    assert_no_room_reader_state(&h, raised, id).await;
}

/// #2516 T6: the raise clears the first bridge lock with an unmoved epoch
/// and parks at the existing `reader_demand_seam`, between the two locks;
/// a disable there must be caught by the second guard.
#[tokio::test]
async fn disabling_between_the_two_bridge_locks_installs_no_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let barrier =
        crate::commands::telegram::reader_demand_seam::install_before_install(&id.to_string());
    let raised = race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        disable_room(&h, &fixture),
    )
    .await;
    assert_no_room_reader_state(&h, raised, id).await;
}

/// #2516 T7: a release landing after the epoch sample and before the
/// readiness read. `release_all` leaves the room config alone, so
/// readiness still resolves `Ready` and the raise really reaches a guard.
#[tokio::test]
async fn destroying_before_the_readiness_read_installs_no_room_demand() {
    let fixture = room_fixture();
    configure_room(fixture.room_path(), true);
    let h = harness(&fixture);
    let id = h.session_in(fixture.coordinator_path()).await;

    let barrier = room_raise_seam::install_before_readiness(&id.to_string());
    let raised = race_room_raise(
        &h,
        fixture.room_path(),
        id,
        &barrier.reached,
        &barrier.release,
        crate::commands::telegram::release_all_reader_demands(h.app.handle(), id),
    )
    .await;
    assert_no_room_reader_state(&h, raised, id).await;
}
