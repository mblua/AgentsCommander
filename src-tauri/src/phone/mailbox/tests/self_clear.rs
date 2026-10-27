use super::*;

#[tokio::test]
async fn handle_self_switch_unknown_coding_agent_lists_configured_ids() {
    let fixture = make_self_switch_fixture();
    let app = app_handle(&fixture.app);
    {
        let settings = app.state::<SettingsState>();
        let mut cfg = settings.write().await;
        cfg.agents = vec![
            wake_agent("codex-main", "Codex Main", "codex"),
            wake_agent(
                "codex-research",
                "Codex Research",
                "codex --profile research",
            ),
        ];
    }
    let (_session_id, token) = seed_self_switch_session(
        &app,
        &fixture.replica,
        "codex",
        Some("codex-main"),
        None,
        Some("A"),
    )
    .await;
    seed_self_handoff(&fixture.replica);

    let (path, msg) = build_self_switch_message(
        &fixture.replica,
        "msg-ss-unknown",
        "rid-ss-unknown",
        Some(token.to_string()),
        Some("codex"),
        None,
        "proj-a:wg-1-dev-team/dev-rust",
    );
    let poller = MailboxPoller::default();
    poller
        .handle_self_handoff_switch(&app, &path, &msg, false)
        .await
        .unwrap();

    let reason = read_reject_reason(&fixture.replica, "msg-ss-unknown").unwrap();
    assert!(reason.contains("codex-main (Codex Main)"), "{reason}");
    assert!(
        reason.contains("codex-research (Codex Research)"),
        "{reason}"
    );
    assert_eq!(pending_self_clear_len(&app).await, 0);
}

#[tokio::test]
async fn self_clear_driver_pi_injects_new_then_handoff_in_boundary_order() {
    let session_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.seed_legacy(session_id);
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "Pi resume notes").unwrap();
    let injected = Arc::new(Mutex::new(Vec::<String>::new()));
    let injected_seen = Arc::clone(&injected);
    let boundaries = Arc::new(Mutex::new(Vec::<SelfClearBoundary>::new()));
    let boundaries_seen = Arc::clone(&boundaries);

    MailboxPoller::drive_self_clear_after_sustained_idle(
        session_id,
        "/new",
        temp.path().to_path_buf(),
        Arc::clone(&pending),
        None,
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_secs(1),
        |_session_id| async { (true, true) },
        move |_session_id, prompt| {
            let injected_seen = Arc::clone(&injected_seen);
            async move {
                injected_seen.lock().unwrap().push(prompt);
                Ok(())
            }
        },
        move |_session_id, boundary| {
            let boundaries_seen = Arc::clone(&boundaries_seen);
            async move {
                boundaries_seen.lock().unwrap().push(boundary);
            }
        },
    )
    .await;

    let injected = injected.lock().unwrap().clone();
    assert_eq!(injected.len(), 2);
    assert_eq!(injected[0], "/new");
    assert!(!injected.iter().any(|text| text == "/clear"));
    assert!(injected[1].contains("self-clear/"));
    assert_eq!(
        *boundaries.lock().unwrap(),
        vec![
            SelfClearBoundary::Cleared,
            SelfClearBoundary::ContentInjected
        ]
    );
    assert!(pending.is_empty());
}

#[tokio::test]
async fn self_clear_driver_waits_for_phase1_injector_before_boundary_or_phase2() {
    let session_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.seed_legacy(session_id);
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "barrier notes").unwrap();
    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let state_calls = Arc::new(AtomicU32::new(0));
    let entered = Arc::new(tokio::sync::Barrier::new(2));
    let release = Arc::new(tokio::sync::Barrier::new(2));

    let events_for_state = Arc::clone(&events);
    let calls_for_state = Arc::clone(&state_calls);
    let session_state = move |_session_id| {
        let events = Arc::clone(&events_for_state);
        let calls = Arc::clone(&calls_for_state);
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            events.lock().unwrap().push("poll");
            (true, true)
        }
    };
    let events_for_inject = Arc::clone(&events);
    let entered_for_inject = Arc::clone(&entered);
    let release_for_inject = Arc::clone(&release);
    let inject = move |_session_id, prompt: String| {
        let events = Arc::clone(&events_for_inject);
        let entered = Arc::clone(&entered_for_inject);
        let release = Arc::clone(&release_for_inject);
        async move {
            if prompt == "/new" {
                events.lock().unwrap().push("inject-start");
                entered.wait().await;
                release.wait().await;
                events.lock().unwrap().push("inject-end");
            } else {
                events.lock().unwrap().push("handoff");
            }
            Ok(())
        }
    };
    let events_for_boundary = Arc::clone(&events);
    let note_boundary = move |_session_id, boundary| {
        let events = Arc::clone(&events_for_boundary);
        async move {
            events.lock().unwrap().push(match boundary {
                SelfClearBoundary::Cleared => "cleared",
                SelfClearBoundary::ContentInjected => "content",
            });
        }
    };
    let root = temp.path().to_path_buf();
    let pending_for_driver = Arc::clone(&pending);
    let driver = tokio::spawn(async move {
        MailboxPoller::drive_self_clear_after_sustained_idle(
            session_id,
            "/new",
            root,
            pending_for_driver,
            None,
            Duration::ZERO,
            Duration::ZERO,
            Duration::from_secs(1),
            session_state,
            inject,
            note_boundary,
        )
        .await;
    });

    tokio::time::timeout(Duration::from_secs(1), entered.wait())
        .await
        .expect("phase-1 injector reached barrier");
    assert_eq!(state_calls.load(Ordering::SeqCst), 1);
    assert_eq!(*events.lock().unwrap(), vec!["poll", "inject-start"]);
    assert!(temp.path().join("SELF-HANDOFF.md").exists());
    release.wait().await;
    tokio::time::timeout(Duration::from_secs(1), driver)
        .await
        .expect("driver completes")
        .expect("driver task joins");

    let events = events.lock().unwrap().clone();
    let cleared = events.iter().position(|event| *event == "cleared").unwrap();
    let phase2_poll = events
        .iter()
        .enumerate()
        .skip(cleared + 1)
        .find(|(_, event)| **event == "poll")
        .map(|(index, _)| index)
        .unwrap();
    assert!(cleared < phase2_poll, "{events:?}");
    assert!(
        events
            .iter()
            .position(|event| *event == "inject-end")
            .unwrap()
            < cleared
    );
    assert!(events.iter().position(|event| *event == "handoff").unwrap() > phase2_poll);
    assert!(pending.is_empty());
}

#[test]
fn self_clear_handoff_busy_after_pi_new_restarts_fresh_idle() {
    let settle = Duration::from_secs(10);
    let max_defer = Duration::from_secs(100);
    let start = std::time::Instant::now();
    let mut state = SelfClearGateState::new(start);
    (state, _) = self_clear_gate_advance(state, true, true, start, settle, max_defer);
    let (next, action) =
        self_clear_gate_advance(state, true, true, start + settle, settle, max_defer);
    state = next;
    assert_eq!(action, SelfClearGateAction::InjectClear);
    assert_eq!(state.phase, SelfClearPhase::Handoff);

    (state, _) = self_clear_gate_advance(
        state,
        true,
        true,
        start + Duration::from_secs(11),
        settle,
        max_defer,
    );
    let (next, action) = self_clear_gate_advance(
        state,
        true,
        false,
        start + Duration::from_secs(15),
        settle,
        max_defer,
    );
    state = next;
    assert_eq!(action, SelfClearGateAction::Wait);
    assert_eq!(state.idle_since, None);
    (state, _) = self_clear_gate_advance(
        state,
        true,
        true,
        start + Duration::from_secs(20),
        settle,
        max_defer,
    );
    let (state, action) = self_clear_gate_advance(
        state,
        true,
        true,
        start + Duration::from_secs(30),
        settle,
        max_defer,
    );
    assert_eq!(state.phase, SelfClearPhase::Handoff);
    assert_eq!(action, SelfClearGateAction::InjectHandoff);
}

#[tokio::test]
async fn self_clear_driver_pi_phase1_failure_has_no_boundary_or_handoff() {
    let session_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.seed_legacy(session_id);
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "Pi resume notes").unwrap();
    let attempted = Arc::new(Mutex::new(Vec::<String>::new()));
    let attempted_seen = Arc::clone(&attempted);
    let boundaries = Arc::new(Mutex::new(Vec::<SelfClearBoundary>::new()));
    let boundaries_seen = Arc::clone(&boundaries);

    MailboxPoller::drive_self_clear_after_sustained_idle(
        session_id,
        "/new",
        temp.path().to_path_buf(),
        Arc::clone(&pending),
        None,
        Duration::ZERO,
        Duration::ZERO,
        Duration::from_secs(1),
        |_session_id| async { (true, true) },
        move |_session_id, prompt| {
            let attempted = Arc::clone(&attempted_seen);
            async move {
                attempted.lock().unwrap().push(prompt);
                Err("injection failed".to_string())
            }
        },
        move |_session_id, boundary| {
            let boundaries = Arc::clone(&boundaries_seen);
            async move {
                boundaries.lock().unwrap().push(boundary);
            }
        },
    )
    .await;

    assert_eq!(*attempted.lock().unwrap(), vec!["/new"]);
    assert!(boundaries.lock().unwrap().is_empty());
    assert!(temp.path().join("SELF-HANDOFF.md").exists());
    assert!(pending.is_empty());
}

#[tokio::test]
async fn self_clear_driver_injects_first_captured_summary_not_later_file() {
    let session_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.seed_legacy(session_id);

    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "resume notes").unwrap();
    std::fs::write(temp.path().join("SELF-FORGET.md"), "first queued summary").unwrap();
    let forgotten_summary = capture_self_forget_summary(temp.path());
    archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
        .unwrap()
        .expect("archive first forget");
    std::fs::write(temp.path().join("SELF-FORGET.md"), "second later summary").unwrap();

    let injected = Arc::new(Mutex::new(Vec::<String>::new()));

    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let injected_seen = injected.clone();
    let inject = move |_session_id: Uuid, prompt: String| {
        let injected_seen = injected_seen.clone();
        async move {
            injected_seen.lock().unwrap().push(prompt);
            Ok(())
        }
    };

    // (#756) F8: record the boundary events the driver surfaces.
    let boundary_events = Arc::new(Mutex::new(Vec::<(Uuid, SelfClearBoundary)>::new()));
    let events_seen = boundary_events.clone();
    let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
        let events_seen = events_seen.clone();
        async move {
            events_seen.lock().unwrap().push((session_id, boundary));
        }
    };

    MailboxPoller::drive_self_clear_after_sustained_idle(
        session_id,
        "/clear",
        temp.path().to_path_buf(),
        pending.clone(),
        forgotten_summary,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        session_state,
        inject,
        note_boundary,
    )
    .await;

    // (#756) happy path: exactly [Cleared, ContentInjected] in order, both
    // on the stable session id (the PTY and id survive /clear).
    assert_eq!(
        *boundary_events.lock().unwrap(),
        vec![
            (session_id, SelfClearBoundary::Cleared),
            (session_id, SelfClearBoundary::ContentInjected),
        ],
        "self-clear must stamp on phase 1 and drop on phase 2"
    );

    let injected = injected.lock().unwrap().clone();
    assert_eq!(injected.len(), 2);
    assert_eq!(injected[0], "/clear");
    assert!(injected[1].contains("first queued summary"));
    assert!(!injected[1].contains("second later summary"));
    assert!(injected[1].contains("closed background"));
    assert!(injected[1].contains("active core information"));
    assert!(!injected[1].contains('\n'));
    assert!(!injected[1].contains('\u{2014}'));
    // #749 - the handoff was archived at inject time and the prompt names the archived path.
    assert!(injected[1].contains("self-clear/"), "{}", injected[1]);
    assert!(injected[1].contains("_SELF-HANDOFF.md"), "{}", injected[1]);
    assert!(!temp.path().join("SELF-HANDOFF.md").exists());
    assert!(pending.is_empty());
}

/// #749 - the Phase-2 archive happens BEFORE the prompt inject, the prompt names the exact
/// archived relative path, and that path resolves to the file (original content) on disk.
#[tokio::test]
async fn self_clear_driver_archives_before_inject_and_prompt_names_exact_path() {
    let session_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.seed_legacy(session_id);

    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "resume notes").unwrap();

    let injected = Arc::new(Mutex::new(Vec::<String>::new()));
    let root_file_present_at_inject = Arc::new(Mutex::new(Vec::<bool>::new()));

    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let injected_seen = injected.clone();
    let presence_seen = root_file_present_at_inject.clone();
    let root_probe = temp.path().join("SELF-HANDOFF.md");
    let inject = move |_session_id: Uuid, prompt: String| {
        let injected_seen = injected_seen.clone();
        let presence_seen = presence_seen.clone();
        let root_probe = root_probe.clone();
        async move {
            presence_seen.lock().unwrap().push(root_probe.exists());
            injected_seen.lock().unwrap().push(prompt);
            Ok(())
        }
    };

    MailboxPoller::drive_self_clear_after_sustained_idle(
        session_id,
        "/clear",
        temp.path().to_path_buf(),
        pending.clone(),
        None,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        session_state,
        inject,
        |_session_id: Uuid, _boundary: SelfClearBoundary| async {},
    )
    .await;

    let injected = injected.lock().unwrap().clone();
    assert_eq!(injected.len(), 2);
    assert_eq!(injected[0], "/clear");
    assert_eq!(
        *root_file_present_at_inject.lock().unwrap(),
        vec![true, false],
        "root handoff still present at the /clear inject, already archived at the prompt inject"
    );
    // Extract the path the prompt names and verify it holds the original notes.
    let named = injected[1]
        .split("read the file ")
        .nth(1)
        .expect("prompt names a file")
        .split(' ')
        .next()
        .expect("path token");
    assert!(named.starts_with("self-clear/"), "{named}");
    assert!(named.ends_with("_SELF-HANDOFF.md"), "{named}");
    assert_eq!(
        std::fs::read_to_string(temp.path().join(named)).unwrap(),
        "resume notes",
        "the exact path named in the prompt must exist with the handoff content"
    );
    assert!(pending.is_empty());
}

/// #749 - inject failure after a successful pre-inject archive renames the handoff back to the
/// root so a re-issue finds it at the canonical name (retry semantics preserved).
#[tokio::test]
async fn self_clear_driver_inject_failure_renames_handoff_back() {
    let session_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.seed_legacy(session_id);

    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "resume notes").unwrap();

    let inject_calls = Arc::new(Mutex::new(0usize));

    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let calls = inject_calls.clone();
    // First call is the Phase-1 "/clear" (must succeed to reach Phase 2); the second is the
    // handoff prompt, which fails.
    let inject = move |_session_id: Uuid, _prompt: String| {
        let calls = calls.clone();
        async move {
            let n = {
                let mut guard = calls.lock().unwrap();
                *guard += 1;
                *guard
            };
            if n == 1 {
                Ok(())
            } else {
                Err("pty write failed".to_string())
            }
        }
    };

    // (#756) F8: a failed phase-2 inject must fire Cleared only (no drop).
    let boundary_events = Arc::new(Mutex::new(Vec::<(Uuid, SelfClearBoundary)>::new()));
    let events_seen = boundary_events.clone();
    let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
        let events_seen = events_seen.clone();
        async move {
            events_seen.lock().unwrap().push((session_id, boundary));
        }
    };

    MailboxPoller::drive_self_clear_after_sustained_idle(
        session_id,
        "/clear",
        temp.path().to_path_buf(),
        pending.clone(),
        None,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        session_state,
        inject,
        note_boundary,
    )
    .await;

    assert_eq!(
        *boundary_events.lock().unwrap(),
        vec![(session_id, SelfClearBoundary::Cleared)],
        "an un-injected handoff must not drop the fresh intent"
    );

    assert_eq!(*inject_calls.lock().unwrap(), 2);
    assert_eq!(
        std::fs::read_to_string(temp.path().join("SELF-HANDOFF.md")).unwrap(),
        "resume notes",
        "a failed prompt inject must rename the archived handoff back to the root"
    );
    let leftover_archives = std::fs::read_dir(temp.path().join("self-clear"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .ends_with("_SELF-HANDOFF.md")
                })
                .count()
        })
        .unwrap_or(0);
    assert_eq!(leftover_archives, 0, "no orphaned handoff archive remains");
    assert!(pending.is_empty());
}

/// (#756) F8: a failed phase-1 /clear inject abandons the flow and fires NO
/// boundary events (an un-injected /clear must not stamp).
#[tokio::test]
async fn self_clear_driver_phase1_inject_failure_fires_no_boundary_events() {
    let session_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.seed_legacy(session_id);

    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "resume notes").unwrap();

    let inject_calls = Arc::new(Mutex::new(0usize));
    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let calls = inject_calls.clone();
    let inject = move |_session_id: Uuid, _prompt: String| {
        let calls = calls.clone();
        async move {
            *calls.lock().unwrap() += 1;
            Err("pty write failed".to_string())
        }
    };

    let boundary_events = Arc::new(Mutex::new(Vec::<(Uuid, SelfClearBoundary)>::new()));
    let events_seen = boundary_events.clone();
    let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
        let events_seen = events_seen.clone();
        async move {
            events_seen.lock().unwrap().push((session_id, boundary));
        }
    };

    MailboxPoller::drive_self_clear_after_sustained_idle(
        session_id,
        "/clear",
        temp.path().to_path_buf(),
        pending.clone(),
        None,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        session_state,
        inject,
        note_boundary,
    )
    .await;

    assert_eq!(*inject_calls.lock().unwrap(), 1, "abandons after phase 1");
    assert!(
        boundary_events.lock().unwrap().is_empty(),
        "an un-injected /clear must not stamp the fresh intent"
    );
    assert!(pending.is_empty());
}

/// #749 - with no root handoff at Phase 2 (the agent moved its own notes), the prompt falls
/// back to the root name and its missing-or-empty clause; nothing is archived.
#[tokio::test]
async fn self_clear_driver_without_root_handoff_prompts_root_name() {
    let session_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.seed_legacy(session_id);

    let temp = tempfile::TempDir::new().unwrap();

    let injected = Arc::new(Mutex::new(Vec::<String>::new()));
    let session_state = move |_session_id: Uuid| async move { (true, true) };
    let injected_seen = injected.clone();
    let inject = move |_session_id: Uuid, prompt: String| {
        let injected_seen = injected_seen.clone();
        async move {
            injected_seen.lock().unwrap().push(prompt);
            Ok(())
        }
    };

    MailboxPoller::drive_self_clear_after_sustained_idle(
        session_id,
        "/clear",
        temp.path().to_path_buf(),
        pending.clone(),
        None,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        session_state,
        inject,
        |_session_id: Uuid, _boundary: SelfClearBoundary| async {},
    )
    .await;

    let injected = injected.lock().unwrap().clone();
    assert_eq!(injected.len(), 2);
    assert!(
        injected[1].contains("read the file SELF-HANDOFF.md relative to your own agent root"),
        "{}",
        injected[1]
    );
    assert!(!injected[1].contains("self-clear/"), "{}", injected[1]);
    assert!(pending.is_empty());
}

#[tokio::test]
async fn self_switch_driver_restarts_uses_new_id_and_cleans_pending() {
    let original_id = Uuid::new_v4();
    let new_id = Uuid::new_v4();
    let pending = Arc::new(crate::PendingSelfClear::default());
    pending.seed_legacy(original_id);
    let seen_states = Arc::new(Mutex::new(Vec::<Uuid>::new()));
    let persist_calls = Arc::new(Mutex::new(Vec::<(PathBuf, String, String)>::new()));
    let restart_calls = Arc::new(Mutex::new(Vec::<Uuid>::new()));
    let inject_calls = Arc::new(Mutex::new(Vec::<Uuid>::new()));
    let alias_seen_at_inject = Arc::new(Mutex::new(false));
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(temp.path().join("SELF-HANDOFF.md"), "switch resume notes").unwrap();
    std::fs::write(temp.path().join("SELF-FORGET.md"), "first switch summary").unwrap();
    let forgotten_summary = capture_self_forget_summary(temp.path());
    archive_root_md(temp.path(), "SELF-FORGET", "20260102_030405")
        .unwrap()
        .expect("archive first forget");
    std::fs::write(temp.path().join("SELF-FORGET.md"), "second switch summary").unwrap();

    let state_seen = seen_states.clone();
    let session_state = move |session_id: Uuid| {
        let state_seen = state_seen.clone();
        async move {
            state_seen.lock().unwrap().push(session_id);
            (true, true)
        }
    };

    let persist_seen = persist_calls.clone();
    let persist = move |cwd: PathBuf, agent: String, profile: String| {
        let persist_seen = persist_seen.clone();
        async move {
            persist_seen.lock().unwrap().push((cwd, agent, profile));
            Ok(())
        }
    };

    let restart_seen = restart_calls.clone();
    let restart = move |session_id: Uuid, _agent: String, _profile: String| {
        let restart_seen = restart_seen.clone();
        async move {
            restart_seen.lock().unwrap().push(session_id);
            Ok(new_id.to_string())
        }
    };

    let pending_for_inject = pending.clone();
    let inject_seen = inject_calls.clone();
    let alias_seen = alias_seen_at_inject.clone();
    let inject = move |session_id: Uuid, prompt: String| {
        let pending_for_inject = pending_for_inject.clone();
        let inject_seen = inject_seen.clone();
        let alias_seen = alias_seen.clone();
        async move {
            assert!(prompt.contains("first switch summary"));
            assert!(!prompt.contains("second switch summary"));
            assert!(prompt.contains("closed background"));
            assert!(prompt.contains("active core information"));
            assert!(!prompt.contains('\n'));
            assert!(!prompt.contains('\u{2014}'));
            // #749 - the prompt names the archived path.
            assert!(prompt.contains("self-clear/"), "{prompt}");
            assert!(prompt.contains("_SELF-HANDOFF.md"), "{prompt}");
            inject_seen.lock().unwrap().push(session_id);
            *alias_seen.lock().unwrap() =
                pending_for_inject.contains(original_id) && pending_for_inject.contains(session_id);
            Ok(())
        }
    };

    let cwd = temp.path().to_path_buf();

    // (#756) F8: the switch driver must fire ONLY ContentInjected, on the
    // NEW session id (phase 1 is a restart; C3 stamps it, never this driver).
    let boundary_events = Arc::new(Mutex::new(Vec::<(Uuid, SelfClearBoundary)>::new()));
    let events_seen = boundary_events.clone();
    let note_boundary = move |session_id: Uuid, boundary: SelfClearBoundary| {
        let events_seen = events_seen.clone();
        async move {
            events_seen.lock().unwrap().push((session_id, boundary));
        }
    };

    MailboxPoller::drive_self_switch_after_sustained_idle(
        original_id,
        cwd.clone(),
        "claude".into(),
        "B".into(),
        forgotten_summary,
        SELF_HANDOFF_FLAVOR_SWITCH,
        pending.clone(),
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        session_state,
        persist,
        restart,
        inject,
        note_boundary,
    )
    .await;

    assert_eq!(
        *boundary_events.lock().unwrap(),
        vec![(new_id, SelfClearBoundary::ContentInjected)],
        "switch fires exactly one ContentInjected on the post-switch id, never Cleared"
    );

    assert_eq!(*seen_states.lock().unwrap(), vec![original_id, new_id]);
    assert_eq!(
        *persist_calls.lock().unwrap(),
        vec![(cwd.clone(), "claude".into(), "B".into())]
    );
    assert_eq!(*restart_calls.lock().unwrap(), vec![original_id]);
    assert_eq!(*inject_calls.lock().unwrap(), vec![new_id]);
    // #749 - the handoff was archived (pre-inject) from the queue-time cwd.
    assert!(!temp.path().join("SELF-HANDOFF.md").exists());
    assert!(
        *alias_seen_at_inject.lock().unwrap(),
        "the new session id must be marked pending during Phase 2 injection"
    );
    assert!(pending.is_empty());
}

#[tokio::test]
async fn plain_self_clear_process_message_auth_binding_and_untouched_files() {
    for role in ["specialist", "coordinator", "root"] {
        for seeded in [false, true] {
            if role == "root" && seeded {
                continue;
            }
            let fixture = make_self_switch_fixture();
            let app = app_handle(&fixture.app);
            let root = if role == "root" {
                PathBuf::from(crate::config::root_agent::root_agent_dir().unwrap())
            } else if role == "coordinator" {
                let path = fixture
                    .replica
                    .parent()
                    .unwrap()
                    .join("__agent_team-coordinator");
                std::fs::create_dir_all(&path).unwrap();
                path
            } else {
                fixture.replica.clone()
            };
            let (sid, token) = if role == "coordinator" {
                seed_raise_hand_session(&app, &root, true, SessionStatus::Running).await
            } else {
                seed_self_switch_session(&app, &root, "claude", None, None, None).await
            };
            if role == "root" {
                app.state::<Arc<tokio::sync::RwLock<SessionManager>>>()
                    .read()
                    .await
                    .set_is_root_agent(sid, true)
                    .await;
            }
            if seeded {
                for name in ["SELF-HANDOFF.md", "SELF-FORGET.md"] {
                    std::fs::write(root.join(name), name).unwrap();
                }
            }
            let sender =
                sender_name_for_session_cwd_with_root_flag(&root.to_string_lossy(), role == "root");
            let mid = Uuid::new_v4().to_string();
            let rid = Uuid::new_v4().to_string();
            let (path, mut msg) = build_self_clear_message_with_from(
                &root,
                &mid,
                &rid,
                Some(token.to_string()),
                &sender,
            );
            msg.action = Some(super::super::PLAIN_SELF_CLEAR_ACTION.into());
            std::fs::write(&path, serde_json::to_vec_pretty(&msg).unwrap()).unwrap();
            MailboxPoller::default()
                .process_message(&app, &path, OutboxOrigin::Replica)
                .await
                .unwrap();
            let base = root.join(crate::config::agent_local_dir_name());
            let ack = base.join("responses").join(format!("{rid}.json"));
            let actual = std::fs::read(&ack).unwrap();
            let expected = format!(
                "{{\n  \"action\": \"self-clear\",\n  \"status\": \"accepted\",\n  \
                 \"session_id\": \"{sid}\",\n  \"request_id\": \"{rid}\",\n  \
                 \"requested_by\": \"{sender}\"\n}}"
            );
            assert_eq!(actual, expected.as_bytes());
            assert!(!path.exists());
            assert!(base
                .join("outbox/delivered")
                .join(format!("{mid}.json"))
                .is_file());
            if role != "root" {
                for name in ["SELF-HANDOFF.md", "SELF-FORGET.md"] {
                    if seeded {
                        assert_eq!(std::fs::read(root.join(name)).unwrap(), name.as_bytes());
                    } else {
                        assert!(!root.join(name).exists());
                    }
                }
                assert!(!root.join("self-clear").exists());
            }
            // Root cleanup owns only UUID leaves; shared handoff names are never inspected.
            std::fs::remove_file(ack).unwrap();
            std::fs::remove_file(base.join("outbox/delivered").join(format!("{mid}.json")))
                .unwrap();
        }
    }
}

#[tokio::test]
async fn plain_self_clear_process_message_rejects_invalid_self_requests() {
    for case in [
        "invalid",
        "stale",
        "sender",
        "root_sender",
        "target",
        "to",
        "shell",
        "request_id",
        "directory",
    ] {
        let fixture = make_self_switch_fixture();
        let app = app_handle(&fixture.app);
        let root = &fixture.replica;
        let shell = if case == "shell" { "cmd" } else { "claude" };
        let (sid, token) = seed_self_switch_session(&app, root, shell, None, None, None).await;
        let sender = sender_name_for_session_cwd_with_root_flag(&root.to_string_lossy(), false);
        let mid = Uuid::new_v4().to_string();
        let rid = Uuid::new_v4().to_string();
        let (path, mut msg) =
            build_self_clear_message_with_from(root, &mid, &rid, Some(token.to_string()), &sender);
        msg.action = Some(super::super::PLAIN_SELF_CLEAR_ACTION.into());
        match case {
            "invalid" => msg.token = Some("invalid".into()),
            "stale" => msg.token = Some(Uuid::new_v4().to_string()),
            "sender" => msg.from = "proj-a:wg-1-dev-team/other".into(),
            "root_sender" => msg.from = crate::config::root_agent::ROOT_AGENT_SENDER.into(),
            "target" => msg.target = Some(sender.clone()),
            "to" => msg.to = sender,
            "request_id" => msg.request_id = None,
            _ => {}
        }
        let ack = root
            .join(crate::config::agent_local_dir_name())
            .join("responses")
            .join(format!("{rid}.json"));
        if case == "directory" {
            std::fs::create_dir_all(&ack).unwrap();
        }
        std::fs::write(&path, serde_json::to_vec_pretty(&msg).unwrap()).unwrap();
        MailboxPoller::default()
            .process_message(&app, &path, OutboxOrigin::Replica)
            .await
            .unwrap();
        assert!(!ack.is_file(), "{case}");
        assert!(read_reject_reason(root, &mid).is_some(), "{case}");
        assert!(!pending_self_clear_contains(&app, sid).await);
    }
}
