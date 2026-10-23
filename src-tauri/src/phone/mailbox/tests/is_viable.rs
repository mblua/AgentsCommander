use super::*;

#[test]
fn wake_agent_command_uses_sender_agent_after_missing_last_coding_agent() {
    let agents = wake_agents();

    let resolved = resolve_wake_agent_command_from_sources(
        &agents,
        "auto",
        None,
        Some("missing"),
        None,
        Some("claude"),
    )
    .unwrap()
    .unwrap();

    assert_eq!(resolved.shell, "claude");
    assert!(resolved.shell_args.is_empty());
    assert_eq!(resolved.agent_id.as_deref(), Some("claude"));
    assert!(resolved.source.contains("senderAgent 'claude'"));
}

#[test]
fn wake_agent_command_uses_first_configured_agent_as_last_resort() {
    let agents = wake_agents();

    let resolved = resolve_wake_agent_command_from_sources(
        &agents,
        "auto",
        None,
        Some("missing"),
        None,
        Some("also-missing"),
    )
    .unwrap()
    .unwrap();

    assert_eq!(resolved.shell, "codex");
    assert_eq!(resolved.shell_args, vec!["--yolo"]);
    assert_eq!(resolved.agent_id.as_deref(), Some("codex"));
    assert!(resolved.source.contains("first configured agent 'codex'"));
}

#[test]
fn wake_agent_command_prefers_current_coding_agent_over_last() {
    let agents = wake_agents();

    let resolved = resolve_wake_agent_command_from_sources(
        &agents,
        "auto",
        Some("claude"),
        Some("codex"),
        None,
        None,
    )
    .unwrap()
    .unwrap();

    assert_eq!(resolved.shell, "claude");
    assert_eq!(resolved.agent_id.as_deref(), Some("claude"));
    assert!(resolved.source.contains("currentCodingAgent 'claude'"));
}

#[test]
fn wake_agent_command_uses_current_coding_agent_when_last_missing() {
    // The "Entire Workgroup" assignment writes currentCodingAgent to a
    // freshly-spawned member that has no lastCodingAgent yet. Without it the
    // resolver would fall through to senderAgent / first configured agent.
    let agents = wake_agents();

    let resolved = resolve_wake_agent_command_from_sources(
        &agents,
        "auto",
        Some("codex"),
        None,
        None,
        Some("claude"),
    )
    .unwrap()
    .unwrap();

    assert_eq!(resolved.shell, "codex");
    assert_eq!(resolved.agent_id.as_deref(), Some("codex"));
    assert!(resolved.source.contains("currentCodingAgent 'codex'"));
}

#[test]
fn wake_agent_command_rejects_invalid_quoted_command_with_source() {
    let agents = vec![wake_agent("codex", "Codex", "codex \"unterminated")];

    let err = resolve_wake_agent_command_from_sources(&agents, "codex", None, None, None, None)
        .unwrap_err();

    assert!(err.contains("preferredAgent 'codex'"));
    assert!(err.contains("agent id 'codex'"));
    assert!(err.contains("label 'Codex'"));
    assert!(err.contains("command=\"codex \\\"unterminated\""));
    assert!(err.contains("unclosed double quote"));
}

#[test]
fn wake_agent_command_selected_agent_preserves_args() {
    let agents = wake_agents();

    let resolved =
        resolve_wake_agent_command_from_sources(&agents, "codex", None, None, None, Some("claude"))
            .unwrap()
            .unwrap();

    assert_eq!(resolved.shell, "codex");
    assert_eq!(resolved.shell_args, vec!["--yolo"]);
    assert_eq!(resolved.agent_id.as_deref(), Some("codex"));
    assert_eq!(resolved.agent_label.as_deref(), Some("Codex"));
}

#[test]
fn sender_name_for_session_cwd_with_root_flag_uses_root_sender() {
    assert_eq!(
        sender_name_for_session_cwd_with_root_flag("C:/tmp/ac-root-agent", true),
        crate::config::root_agent::ROOT_AGENT_SENDER
    );
    assert_eq!(
        sender_name_for_session_cwd_with_root_flag(
            "C:/tmp/proj-a/.ac/wg-1-dev-team/__agent_tech-lead",
            false
        ),
        "proj-a:wg-1-dev-team/tech-lead"
    );
}

#[test]
fn root_agent_claim_accepts_live_root_uuid_to_verified_wg_coordinator() {
    let (_temp, paths) = make_root_route_fixture(false);

    assert_eq!(
        validate_root_sender_route("proj-a:wg-1-dev-team/tech-lead", &paths, false, true, true,),
        Ok(())
    );
}

#[test]
fn root_agent_claim_rejects_non_root_uuid_claiming_root_sender() {
    let (_temp, paths) = make_root_route_fixture(false);

    assert_eq!(
        validate_root_sender_route("proj-a:wg-1-dev-team/tech-lead", &paths, false, true, false,),
        Err("Root Agent sender requires the live root session token")
    );
}

#[test]
fn root_agent_claim_rejects_origin_coordinator_target() {
    let (_temp, paths) = make_root_route_fixture(false);

    assert_eq!(
        validate_root_sender_route("proj-a/tech-lead", &paths, false, true, true),
        Err("Root Agent can only message verified Room orchestrator replicas")
    );
}

#[test]
fn root_agent_claim_rejects_spoofed_wg_coordinator_dir_name() {
    let (_temp, paths) = make_root_route_fixture(true);

    assert_eq!(
        validate_root_sender_route("proj-a:wg-1-dev-team/tech-lead", &paths, false, true, true,),
        Err("Root Agent can only message verified Room orchestrator replicas")
    );
}

#[tokio::test]
async fn ordinary_message_stale_uuid_still_rejected() {
    let mgr = SessionManager::new();

    assert!(mgr.find_by_token(Uuid::new_v4()).await.is_none());
}

#[test]
fn ordinary_message_malformed_uuid_still_rejected() {
    assert!(Uuid::parse_str("not-a-session-uuid").is_err());
}

#[test]
fn master_root_sender_still_restricted_to_verified_wg_coordinator() {
    let (_temp, paths) = make_root_route_fixture(false);

    assert_eq!(
        validate_root_sender_route("proj-a/tech-lead", &paths, true, false, false),
        Err("Root Agent can only message verified Room orchestrator replicas")
    );
}

#[test]
fn root_sender_payload_rejects_hand_written_command_json() {
    let raw = serde_json::json!({
        "id": "msg-root-command",
        "from": crate::config::root_agent::ROOT_AGENT_SENDER,
        "to": "proj-a:wg-1-dev-team/tech-lead",
        "body": "",
        "mode": "wake",
        "timestamp": "2026-05-24T00:00:00Z",
        "command": "compact"
    });
    let msg: OutboxMessage = serde_json::from_value(raw).unwrap();

    assert_eq!(
        validate_root_sender_payload_with_root_dir(&msg, Path::new("unused-root")),
        Err("Root Agent messages must use --send; remote commands are not allowed".into())
    );
}

#[test]
fn root_sender_payload_rejects_hand_written_non_file_body_json() {
    let raw = serde_json::json!({
        "id": "msg-root-body",
        "from": crate::config::root_agent::ROOT_AGENT_SENDER,
        "to": "proj-a:wg-1-dev-team/tech-lead",
        "body": "please do this directly",
        "mode": "wake",
        "timestamp": "2026-05-24T00:00:00Z"
    });
    let msg: OutboxMessage = serde_json::from_value(raw).unwrap();

    assert_eq!(
        validate_root_sender_payload_with_root_dir(&msg, Path::new("unused-root")),
        Err("Root Agent messages must be canonical file notifications".into())
    );
}

#[test]
fn root_sender_payload_accepts_valid_root_file_notification() {
    let temp = tempfile::TempDir::new().unwrap();
    let root_dir = temp
        .path()
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    let messaging_dir = root_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    std::fs::create_dir_all(&messaging_dir).unwrap();
    let filename = "20260524-040000-root-to-wg1-tech-lead-smoke.md";
    let message_file = messaging_dir.join(filename);
    std::fs::write(&message_file, "root message").unwrap();
    let body = crate::phone::messaging::format_file_notification(&message_file.to_string_lossy());
    let msg = root_outbox_message(body, None);

    assert_eq!(
        validate_root_sender_payload_with_root_dir(&msg, &root_dir),
        Ok(())
    );
}

#[test]
fn root_sender_payload_accepts_legacy_file_notification() {
    let temp = tempfile::TempDir::new().unwrap();
    let root_dir = temp
        .path()
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    let messaging_dir = root_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    std::fs::create_dir_all(&messaging_dir).unwrap();
    let filename = "20260524-040000-root-to-wg1-tech-lead-smoke.md";
    let message_file = messaging_dir.join(filename);
    std::fs::write(&message_file, "root message").unwrap();
    let body = format!(
        "New message: {}. Read this file.",
        message_file.to_string_lossy()
    );
    let msg = root_outbox_message(body, None);

    assert_eq!(
        validate_root_sender_payload_with_root_dir(&msg, &root_dir),
        Ok(())
    );
}

#[test]
fn root_sender_payload_rejects_existing_non_root_message_file() {
    let temp = tempfile::TempDir::new().unwrap();
    let root_dir = temp
        .path()
        .join(crate::config::root_agent::ROOT_AGENT_DIR_NAME);
    let messaging_dir = root_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    std::fs::create_dir_all(&messaging_dir).unwrap();
    let filename = "20260524-040000-wg1-dev-rust-to-wg1-tech-lead-smoke.md";
    let message_file = messaging_dir.join(filename);
    std::fs::write(&message_file, "not root-shaped").unwrap();
    let body = crate::phone::messaging::format_file_notification(&message_file.to_string_lossy());
    let msg = root_outbox_message(body, None);

    assert!(validate_root_sender_payload_with_root_dir(&msg, &root_dir)
        .unwrap_err()
        .contains("Root Agent file notification is invalid"));
}

#[test]
fn validate_coordinator_to_root_route_accepts_verified_coordinator() {
    let (_temp, paths) = make_root_route_fixture(false);
    assert_eq!(
        validate_coordinator_to_root_route("proj-a:wg-1-dev-team/tech-lead", &paths),
        Ok(())
    );
}

#[test]
fn validate_coordinator_to_root_route_rejects_non_coordinator_replica() {
    let (_temp, paths) = make_root_route_fixture(false);
    assert_eq!(
        validate_coordinator_to_root_route("proj-a:wg-1-dev-team/dev-rust", &paths),
        Err("Only verified Room orchestrator replicas may message the Root Agent")
    );
}

#[test]
fn validate_coordinator_to_root_route_rejects_spoofed_identity() {
    let (_temp, paths) = make_root_route_fixture(true);
    assert_eq!(
        validate_coordinator_to_root_route("proj-a:wg-1-dev-team/tech-lead", &paths),
        Err("Only verified Room orchestrator replicas may message the Root Agent")
    );
}

#[test]
fn is_viable_root_recipient_rejects_exited_regardless_of_pty() {
    // §12.1 regression: Exited(_) must never be a viable candidate, with
    // or without PTY. If this fails, `find_root_session_candidate` would
    // return an Exited candidate, the deferred-destroy block at
    // mailbox.rs:982-998 would fire, and the user's Root Agent session
    // record would be silently destroyed.
    assert!(!is_viable_root_recipient(
        &crate::session::session::SessionStatus::Exited(0),
        true
    ));
    assert!(!is_viable_root_recipient(
        &crate::session::session::SessionStatus::Exited(0),
        false
    ));
    assert!(!is_viable_root_recipient(
        &crate::session::session::SessionStatus::Exited(127),
        true
    ));
}

#[test]
fn is_viable_root_recipient_requires_pty_for_live_states() {
    use crate::session::session::SessionStatus;
    for status in [
        SessionStatus::Active,
        SessionStatus::Running,
        SessionStatus::Idle,
    ] {
        assert!(
            is_viable_root_recipient(&status, true),
            "{:?}+pty should be viable",
            status
        );
        assert!(
            !is_viable_root_recipient(&status, false),
            "{:?}+no-pty must be a phantom",
            status
        );
    }
}

/// #293 regression: a coordinator sends to `ROOT_AGENT_SENDER`; the
/// mailbox accepts the route and the recipient lookup finds the
/// session marked `is_root_agent`. No auto-spawn, no can_communicate
/// fallback.
#[test]
fn coordinator_to_root_uri_route_accepts_and_locates_session_by_flag() {
    let (_temp, paths) = make_root_route_fixture(false);

    // Sender route: coordinator → root.
    assert_eq!(
        validate_coordinator_to_root_route("proj-a:wg-1-dev-team/tech-lead", &paths),
        Ok(())
    );

    // Recipient lookup predicate: any session with `is_root_agent==true`
    // matches, regardless of CWD or FQN. Use the same predicate that
    // `find_root_session_candidate` applies.
    let pool = vec![make_session_info(
        "root-uuid",
        "root",
        "C:/cfg/ac-root-agent",
        crate::session::session::SessionStatus::Idle,
        true,
    )];
    let mut root_session = pool[0].clone();
    root_session.is_root_agent = true;
    let predicate = |s: &crate::session::session::SessionInfo| {
        s.is_root_agent || crate::config::root_agent::is_root_agent_path(&s.working_directory)
    };
    assert!(predicate(&root_session));
    assert_eq!(
        filter_sessions_by_fqn(&pool, "proj-a:wg-1-dev-team/tech-lead").len(),
        0,
        "agent_fqn_from_path must NOT resolve the root URI — the lookup is by `is_root_agent` flag"
    );
}

/// §224 regression: an idle session with `waiting_for_input=true` (the bug
/// user's exact state) must still pass the predicate. The predicate is
/// FQN-only by design.
#[test]
fn filter_sessions_by_fqn_includes_idle_waiting_session() {
    let target = "proj:wg-1-devs/alice";
    let s = make_session_info(
        "uuid-1",
        "alice",
        r"C:\proj\.ac\wg-1-devs\__agent_alice",
        crate::session::session::SessionStatus::Idle,
        true,
    );
    let pool = vec![s];
    let hits = filter_sessions_by_fqn(&pool, target);
    assert_eq!(
        hits.len(),
        1,
        "idle/waiting session must match FQN-only predicate"
    );
}

#[test]
fn filter_sessions_by_fqn_rejects_non_matching_cwd() {
    let target = "proj:wg-1-devs/alice";
    let s = make_session_info(
        "uuid-1",
        "bob",
        r"C:\proj\.ac\wg-1-devs\__agent_bob",
        crate::session::session::SessionStatus::Active,
        false,
    );
    let pool = vec![s];
    let hits = filter_sessions_by_fqn(&pool, target);
    assert!(hits.is_empty(), "bob's session must not match alice's FQN");
}

#[test]
fn filter_sessions_by_fqn_matches_regardless_of_was_detached_or_status() {
    // §224 — the active vs detached vs waiting-for-input mix should not
    // matter; only the FQN of working_directory.
    let target = "proj:wg-1-devs/alice";
    let mut active = make_session_info(
        "uuid-a",
        "alice-active",
        r"C:\proj\.ac\wg-1-devs\__agent_alice",
        crate::session::session::SessionStatus::Active,
        false,
    );
    active.was_detached = true;
    let idle = make_session_info(
        "uuid-b",
        "alice-idle",
        r"C:\proj\.ac\wg-1-devs\__agent_alice",
        crate::session::session::SessionStatus::Idle,
        true,
    );
    let exited = make_session_info(
        "uuid-c",
        "alice-exited",
        r"C:\proj\.ac\wg-1-devs\__agent_alice",
        crate::session::session::SessionStatus::Exited(0),
        false,
    );
    let pool = vec![active, idle, exited];
    let hits = filter_sessions_by_fqn(&pool, target);
    assert_eq!(hits.len(), 3, "all three must match the same FQN");
}

#[tokio::test]
async fn wait_returns_session_when_probe_succeeds_mid_wait() {
    let flag = AtomicBool::new(true);
    let counter = std::sync::Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let result = wait_for_restore_or_session(
        &flag,
        || {
            let counter = counter_clone.clone();
            async move {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                if n >= 2 {
                    vec![Uuid::nil()]
                } else {
                    vec![]
                }
            }
        },
        deadline,
        std::time::Duration::from_millis(50),
    )
    .await;
    assert!(result.is_ok(), "probe should have produced a hit");
    assert_eq!(result.unwrap().len(), 1);
}

#[tokio::test]
async fn wait_returns_still_in_progress_when_flag_never_clears() {
    let flag = AtomicBool::new(true);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    let result = wait_for_restore_or_session(
        &flag,
        || async { vec![] },
        deadline,
        std::time::Duration::from_millis(50),
    )
    .await;
    assert_eq!(result, Err(RestoreWaitOutcome::StillInProgress));
}

#[tokio::test]
async fn wait_returns_no_match_when_flag_clears_with_empty_result() {
    let flag = std::sync::Arc::new(AtomicBool::new(true));
    let flag_clone = flag.clone();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        flag_clone.store(false, Ordering::SeqCst);
    });
    let result = wait_for_restore_or_session(
        &flag,
        || async { vec![] },
        deadline,
        std::time::Duration::from_millis(50),
    )
    .await;
    assert_eq!(result, Err(RestoreWaitOutcome::NoMatch));
}

#[test]
fn wake_action_running_injects() {
    assert_eq!(wake_action_for(&SessionStatus::Running), WakeAction::Inject);
}

// ── wake_spawn_skip_auto_resume tests (issue #82, plan §8.2) ──

#[test]
fn wake_spawn_skip_auto_resume_skips_when_cold() {
    // Cold wake (no prior session, or race fallthrough) — suppress resume.
    assert!(wake_spawn_skip_auto_resume(false));
}

#[test]
fn wake_spawn_skip_auto_resume_allows_when_known_state() {
    // Known-state wake (RespawnExited match) — allow `--continue` /
    // codex `resume --last` / antigravity `--continue`.
    assert!(!wake_spawn_skip_auto_resume(true));
}

#[test]
fn wake_action_active_injects() {
    assert_eq!(wake_action_for(&SessionStatus::Active), WakeAction::Inject);
}

#[test]
fn wake_action_idle_injects() {
    assert_eq!(wake_action_for(&SessionStatus::Idle), WakeAction::Inject);
}

#[test]
fn wake_action_exited_respawns() {
    assert_eq!(
        wake_action_for(&SessionStatus::Exited(0)),
        WakeAction::RespawnExited
    );
    // Non-zero exit codes take the same path.
    assert_eq!(
        wake_action_for(&SessionStatus::Exited(1)),
        WakeAction::RespawnExited
    );
    assert_eq!(
        wake_action_for(&SessionStatus::Exited(-1)),
        WakeAction::RespawnExited
    );
}

// ── is_viable_wake_candidate tests (issue #223) ──

#[test]
fn is_viable_wake_candidate_keeps_live_running() {
    assert!(is_viable_wake_candidate(&SessionStatus::Running, true));
}

#[test]
fn is_viable_wake_candidate_keeps_live_idle() {
    assert!(is_viable_wake_candidate(&SessionStatus::Idle, true));
}

#[test]
fn is_viable_wake_candidate_keeps_live_active() {
    assert!(is_viable_wake_candidate(&SessionStatus::Active, true));
}

/// Issue #223 — primary regression guard. A SessionManager record with
/// status=Running but no PtyManager entry (the phantom in the log) MUST be
/// skipped — otherwise inject_into_pty fails with `Session not found:` and
/// the router has no fallback.
#[test]
fn is_viable_wake_candidate_skips_phantom_running_no_pty() {
    assert!(!is_viable_wake_candidate(&SessionStatus::Running, false));
}

#[test]
fn is_viable_wake_candidate_skips_phantom_idle_no_pty() {
    assert!(!is_viable_wake_candidate(&SessionStatus::Idle, false));
}

#[test]
fn is_viable_wake_candidate_skips_phantom_active_no_pty() {
    assert!(!is_viable_wake_candidate(&SessionStatus::Active, false));
}

/// AC5 regression guard. Deferred-non-coord sessions have status=Exited(0)
/// and NO PTY (see lib.rs:614). They MUST remain candidates so the
/// documented respawn path runs and `--continue` is injected on the new
/// session.
#[test]
fn is_viable_wake_candidate_keeps_exited_without_pty_for_respawn() {
    assert!(is_viable_wake_candidate(&SessionStatus::Exited(0), false));
    assert!(is_viable_wake_candidate(&SessionStatus::Exited(1), false));
    assert!(is_viable_wake_candidate(&SessionStatus::Exited(-1), false));
}

/// Stale-PTY-instance edge case: status=Exited with a leftover PtyInstance
/// entry. Still a candidate — RespawnExited will destroy the leftover.
#[test]
fn is_viable_wake_candidate_keeps_exited_with_stale_pty() {
    assert!(is_viable_wake_candidate(&SessionStatus::Exited(0), true));
}

/// A PtyManager entry that EXISTS but whose underlying child has exited is
/// STILL treated as viable by this layer (we cannot detect child liveness
/// without surfacing portable_pty::Child::wait()). Documented here so a
/// future PR closing the AC3 gap (PTY-exit hook in `#223-fu1`) knows this
/// test must be updated to assert the new contract. (dev-rust R1.E3.)
#[test]
fn is_viable_wake_candidate_accepts_mapped_pty_regardless_of_child_state() {
    // status=Running + has_pty=true → viable, even if the child is secretly dead.
    // The router relies on `#223-fu1` to keep status in sync with reality.
    assert!(is_viable_wake_candidate(&SessionStatus::Running, true));
}

// ── (#1001 PR2 / B) cold-spawn settle_tick + live_settle_action (grinch P1) ──

#[test]
fn settle_tick_cold_spawn_busy_keeps_waiting_and_resets_clock() {
    let now = std::time::Instant::now();
    let (idle_since, action) = settle_tick(
        false,
        Some(now),
        now,
        Duration::from_millis(2000),
        Duration::from_millis(0),
        Duration::from_secs(90),
    );
    assert_eq!(action, SettleAction::Wait);
    assert_eq!(
        idle_since, None,
        "busy resets the settle clock (cold-spawn)"
    );
}

#[test]
fn settle_tick_sustained_idle_injects() {
    let now = std::time::Instant::now();
    let since = now.checked_sub(Duration::from_millis(2000)).unwrap();
    let (_, action) = settle_tick(
        true,
        Some(since),
        now,
        Duration::from_millis(2000),
        Duration::from_millis(0),
        Duration::from_secs(90),
    );
    assert_eq!(
        action,
        SettleAction::InjectNow,
        "idle held >= settle -> inject"
    );
}

#[test]
fn settle_tick_caps_and_injects_anyway() {
    let now = std::time::Instant::now();
    // Not yet settled, but max_wait exceeded -> inject anyway (never drop).
    let (_, action) = settle_tick(
        true,
        Some(now),
        now,
        Duration::from_millis(2000),
        Duration::from_secs(91),
        Duration::from_secs(90),
    );
    assert_eq!(
        action,
        SettleAction::InjectNow,
        "cap must never drop a delivery"
    );
}

/// #1388, T1 - the composed readiness input requires BOTH conditions.
///
/// The suite's only genuinely new assertion. It does not verify the wiring: the
/// composition happens here, in the test, and `settle_tick` is a free function that
/// cannot observe what `settle_until_ready` passes it. Criteria C1 and C2 carry that.
#[test]
fn wake_settle_ready_requires_both() {
    assert!(wake_settle_ready(true, true));
    assert!(!wake_settle_ready(true, false), "idle but nothing painted");
    assert!(!wake_settle_ready(false, true), "painted but still busy");
    assert!(!wake_settle_ready(false, false));
}

/// #1388, T3 - a `settle_tick` regression test for one uncovered input combination
/// (`ready == false` with `elapsed >= max_wait`), NOT evidence this change works.
///
/// The existing pair covers `true`/over-cap and `false`/zero-elapsed. Decision 4.1 is
/// a decision NOT to change `settle_tick`, already pinned by
/// `settle_tick_caps_and_injects_anyway`, which must stay green unmodified.
#[test]
fn settle_tick_caps_when_not_ready() {
    let now = std::time::Instant::now();
    let (_, action) = settle_tick(
        false,
        Some(now),
        now,
        Duration::from_millis(2000),
        Duration::from_secs(91),
        Duration::from_secs(90),
    );
    assert_eq!(
        action,
        SettleAction::InjectNow,
        "cap must never drop a delivery, not even for a never-rendered session"
    );
}

#[test]
fn live_settle_action_busy_mid_turn_injects_immediately() {
    // activity_age < idle_threshold: the agent is actively producing output.
    let action = live_settle_action(
        Some(Duration::from_millis(1000)),
        None,
        LSA_RESIZE_GRACE,
        LSA_IDLE_THRESHOLD,
        LSA_SETTLE,
        Duration::from_millis(0),
        LSA_MAX_WAIT,
    );
    assert_eq!(
        action,
        SettleAction::InjectNow,
        "busy/mid-turn injects at once"
    );
}

#[test]
fn live_settle_action_fresh_idle_window_waits() {
    // The exact lagged-flag hole (grinch P1): activity_age past idle_threshold
    // but < settle. waiting_for_input may still be false here; we must WAIT.
    for age_ms in [2501u64, 2800, 3499] {
        let action = live_settle_action(
            Some(Duration::from_millis(age_ms)),
            None,
            LSA_RESIZE_GRACE,
            LSA_IDLE_THRESHOLD,
            LSA_SETTLE,
            Duration::from_millis(0),
            LSA_MAX_WAIT,
        );
        assert_eq!(
            action,
            SettleAction::Wait,
            "fresh-idle window must settle (age={age_ms})"
        );
    }
}

#[test]
fn live_settle_action_long_idle_injects_without_waiting() {
    let action = live_settle_action(
        Some(Duration::from_secs(60)),
        None,
        LSA_RESIZE_GRACE,
        LSA_IDLE_THRESHOLD,
        LSA_SETTLE,
        Duration::from_millis(0),
        LSA_MAX_WAIT,
    );
    assert_eq!(
        action,
        SettleAction::InjectNow,
        "long-idle (ready) injects at once"
    );
}
