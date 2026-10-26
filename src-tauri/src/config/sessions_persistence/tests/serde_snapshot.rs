use super::*;

/// §224 D.2 — the strip drops every runtime field but preserves the recipe
/// fields needed for the next-startup restore attempt. Since #747 a raised
/// hand counts as durable intent, not runtime state: `communication`
/// survives the strip so the retry can restore it.
#[test]
fn sanitize_failed_recoverable_drops_runtime_fields_keeps_raise_hand() {
    let ps = PersistedSession {
        last_prompt: None,
        name: "alice".into(),
        shell: "claude".into(),
        shell_args: vec!["--continue".into()],
        working_directory: r"C:\proj\.ac\wg-1-devs\__agent_alice".into(),
        was_active: false,
        git_repos: vec![],
        is_coordinator: false,
        is_root_agent: false,
        agent_id: Some("aid-1".into()),
        agent_label: Some("Claude Code".into()),
        requested_profile: None,
        telegram_bot_id: Some("bot-1".into()),
        was_detached: false,
        detached_geometry: None,
        git_branch_source: None,
        git_branch_prefix: None,
        // Stale runtime fields from a prior run:
        id: Some("uuid-prior-run".into()),
        status: Some(SessionStatus::Idle),
        waiting_for_input: Some(true),
        communication: Some(SessionCommunication {
            kind: SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: "2026-06-30T11:00:00+00:00".into(),
            message: None,
        }),
        created_at: Some("2026-05-15T00:00:00Z".into()),
        ..Default::default()
    };

    let clean = sanitize_failed_recoverable(&ps);

    // Runtime fields cleared:
    assert!(clean.id.is_none(), "id must be cleared");
    assert!(clean.status.is_none(), "status must be cleared");
    assert!(
        clean.waiting_for_input.is_none(),
        "waiting_for_input must be cleared"
    );
    assert!(clean.created_at.is_none(), "created_at must be cleared");

    // Recipe fields preserved (so next-run restore can retry):
    let kept = clean
        .communication
        .as_ref()
        .expect("#747: the raised hand must survive the strip");
    assert_eq!(kept.kind, SessionCommunicationKind::RaiseHand);
    assert!(kept.visible, "visibility must survive the strip");
    assert_eq!(clean.name, "alice");
    assert_eq!(clean.shell, "claude");
    assert_eq!(clean.shell_args, vec!["--continue".to_string()]);
    assert_eq!(clean.working_directory, ps.working_directory);
    assert_eq!(clean.agent_id.as_deref(), Some("aid-1"));
    assert_eq!(clean.agent_label.as_deref(), Some("Claude Code"));
    assert_eq!(clean.telegram_bot_id.as_deref(), Some("bot-1"));
    assert!(!clean.was_active);
    assert!(!clean.was_detached);
}

/// §224 D.2 — idempotence: stripping an entry that already has None
/// runtime fields is a no-op (does not flip recipe fields).
#[test]
fn sanitize_failed_recoverable_is_idempotent() {
    let ps = PersistedSession {
        last_prompt: None,
        name: "bob".into(),
        shell: "cmd".into(),
        shell_args: vec![],
        working_directory: "C:/x".into(),
        was_active: false,
        git_repos: vec![],
        is_coordinator: false,
        is_root_agent: false,
        agent_id: None,
        agent_label: None,
        requested_profile: None,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        git_branch_source: None,
        git_branch_prefix: None,
        id: None,
        status: None,
        waiting_for_input: None,
        created_at: None,
        ..Default::default()
    };
    let once = sanitize_failed_recoverable(&ps);
    let twice = sanitize_failed_recoverable(&once);
    assert!(twice.id.is_none());
    assert!(twice.status.is_none());
    assert!(twice.waiting_for_input.is_none());
    assert!(twice.created_at.is_none());
    assert_eq!(twice.name, "bob");
}

#[test]
fn telegram_bot_id_defaults_none_for_legacy_json() {
    let json = r#"{
        "name": "legacy",
        "shell": "cmd",
        "shellArgs": [],
        "workingDirectory": "C:/x"
    }"#;

    let back: PersistedSession = serde_json::from_str(json).expect("deserialize");
    assert!(back.telegram_bot_id.is_none());
}

#[test]
fn communication_defaults_none_for_legacy_json() {
    let json = r#"{
        "name": "legacy",
        "shell": "cmd",
        "shellArgs": [],
        "workingDirectory": "C:/x"
    }"#;

    let back: PersistedSession = serde_json::from_str(json).expect("deserialize");
    assert!(back.communication.is_none());
}

/// Issue 698: a malformed/future typed `communication` field invalidates the
/// entire raw load (serde fails the whole array, `unwrap_or_default` yields
/// `[]`), exactly like other malformed typed persisted fields. This is the
/// accepted behavior for #698; documented here so it is intentional, not a
/// silent regression. Revisit in a separate persistence-resilience issue.
#[test]
fn load_sessions_raw_returns_empty_for_malformed_communication() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions = serde_json::json!([
        {
            "name": "coord-x",
            "shell": "codex",
            "shellArgs": [],
            "workingDirectory": "C:/proj/.ac/wg-1-dev-team/__agent_tech-lead",
            "id": "11111111-1111-1111-1111-111111111111",
            "status": "running",
            "waitingForInput": false,
            "isCoordinator": true,
            "communication": {
                "kind": "futureKind",
                "visible": true,
                "updatedAt": "2026-06-30T11:00:00+00:00"
            },
            "createdAt": "2026-06-30T10:00:00+00:00"
        }
    ]);
    std::fs::write(
        temp.path().join("sessions.json"),
        serde_json::to_string_pretty(&sessions).expect("sessions json"),
    )
    .expect("write sessions");

    let rows = load_sessions_raw_from_dir_for_test(temp.path());

    assert!(
        rows.is_empty(),
        "malformed typed communication intentionally invalidates the raw load in issue 698"
    );
}

#[test]
fn telegram_bot_id_round_trips_when_present() {
    let ps = PersistedSession {
        last_prompt: None,
        name: "telegram-on".into(),
        shell: "codex".into(),
        shell_args: vec![],
        working_directory: "C:/x".into(),
        telegram_bot_id: Some("bot-1".into()),
        ..Default::default()
    };

    let json = serde_json::to_value(&ps).expect("serialize");
    assert_eq!(json["telegramBotId"], "bot-1");
    let back: PersistedSession = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back.telegram_bot_id.as_deref(), Some("bot-1"));
}

#[test]
fn persisted_session_round_trips_effective_profile() {
    let ps = PersistedSession {
        last_prompt: None,
        name: "agent-session".into(),
        shell: "codex".into(),
        shell_args: vec![],
        working_directory: "C:/x".into(),
        agent_id: Some("codex".into()),
        effective_profile: Some("B".into()),
        ..Default::default()
    };

    let json = serde_json::to_value(&ps).expect("serialize");
    assert_eq!(json["effectiveProfile"], "B");
    let back: PersistedSession = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back.effective_profile.as_deref(), Some("B"));

    // None omits the key entirely (additive schema: rows written before
    // this phase serialize back without the field).
    let plain = PersistedSession {
        last_prompt: None,
        name: "plain-shell".into(),
        shell: "cmd".into(),
        shell_args: vec![],
        working_directory: "C:/x".into(),
        ..Default::default()
    };
    let json = serde_json::to_value(&plain).expect("serialize");
    assert!(
        json.get("effectiveProfile").is_none(),
        "None must omit effectiveProfile: {json}"
    );
    let back: PersistedSession = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back.effective_profile, None);
}

// (#630 fleet-safety) A pre-existing sessions.json record that predates this
// field must deserialize to `false` = resume, so no existing user is silently
// flipped to "start fresh" on upgrade. Polarity guard for §3.1.
#[test]
fn start_fresh_on_restore_defaults_false_for_legacy_json() {
    let json = r#"{
        "name": "legacy",
        "shell": "cmd",
        "shellArgs": [],
        "workingDirectory": "C:/x"
    }"#;

    let back: PersistedSession = serde_json::from_str(json).expect("deserialize");
    assert!(!back.start_fresh_on_restore);
}

// (#631) The durable fresh intent survives a serialize -> deserialize round-trip
// in both polarities, so "Restart Session" persists across an app restart. The
// field has no `skip_serializing_if`, so it is always written explicitly.
#[test]
fn start_fresh_on_restore_round_trips() {
    for fresh in [true, false] {
        let ps = PersistedSession {
            name: "round-trip".into(),
            shell: "claude".into(),
            shell_args: vec![],
            working_directory: "C:/x".into(),
            start_fresh_on_restore: fresh,
            ..Default::default()
        };
        let json = serde_json::to_value(&ps).expect("serialize");
        assert_eq!(json["startFreshOnRestore"], fresh);
        let back: PersistedSession = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back.start_fresh_on_restore, fresh);
    }
}

#[tokio::test]
async fn snapshot_sessions_preserves_telegram_bot_id() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "powershell.exe".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");

    mgr.set_telegram_bot_id(session.id, Some("bot-1".into()))
        .await;

    let snapshot = snapshot_sessions(&mgr).await;

    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].telegram_bot_id.as_deref(), Some("bot-1"));
}

// ── #1088: context_percent rides Session -> SessionInfo -> snapshot into
//    PersistedSession, and serializes additively (0 explicit, None absent). ──

#[tokio::test]
async fn snapshot_sessions_preserves_context_percent() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "powershell.exe".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");

    mgr.set_context_percent(session.id, Some(37)).await;

    let snapshot = snapshot_sessions(&mgr).await;

    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].context_percent, Some(37));
}

#[test]
fn context_percent_serializes_zero_and_omits_none() {
    // Some(0) -> explicit "contextPercent": 0 (0 is a valid reading).
    let ps = PersistedSession {
        name: "ctx".into(),
        shell: "codex".into(),
        shell_args: vec![],
        working_directory: "C:/x".into(),
        context_percent: Some(0),
        ..Default::default()
    };
    let json = serde_json::to_value(&ps).expect("serialize");
    assert_eq!(json["contextPercent"], 0);
    let back: PersistedSession = serde_json::from_value(json).expect("deserialize");
    assert_eq!(back.context_percent, Some(0));

    // None -> key absent (skip_serializing_if), so old files stay byte-identical.
    let ps_none = PersistedSession {
        name: "ctx".into(),
        shell: "codex".into(),
        shell_args: vec![],
        working_directory: "C:/x".into(),
        context_percent: None,
        ..Default::default()
    };
    let json_none = serde_json::to_value(&ps_none).expect("serialize");
    assert!(json_none.get("contextPercent").is_none());
}

#[test]
fn context_percent_defaults_none_for_legacy_json() {
    let json = r#"{
        "name": "legacy",
        "shell": "cmd",
        "shellArgs": [],
        "workingDirectory": "C:/x"
    }"#;
    let back: PersistedSession = serde_json::from_str(json).expect("deserialize");
    assert_eq!(back.context_percent, None);
}

#[tokio::test]
async fn snapshot_pi_persists_configured_args_not_effective_resume() {
    let mgr = SessionManager::new();
    let configured = vec!["--model".to_string(), "claude-sonnet".to_string()];
    let session = mgr
        .create_session(
            "pi".to_string(),
            configured.clone(),
            "C:\\tmp".to_string(),
            Some("pi".to_string()),
            Some("Pi".to_string()),
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    mgr.set_effective_shell_args(
        session.id,
        vec![
            "--continue".to_string(),
            "--model".to_string(),
            "claude-sonnet".to_string(),
        ],
    )
    .await;

    let snapshot = snapshot_sessions(&mgr).await;

    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].shell_args, configured);
}

/// #1873 - every configured Muse recipe survives stripping byte-for-byte, and a
/// snapshot persists the configured argv, never the effective `resume --last`.
#[tokio::test]
async fn muse_recipes_and_snapshots_preserve_configured_args_not_effective_resume() {
    let s = |tokens: &[&str]| tokens.iter().map(|t| t.to_string()).collect::<Vec<_>>();
    let recipes: Vec<(&str, Vec<String>)> = vec![
        ("muse", s(&[])),
        ("muse", s(&["--workspace", "/srv/work"])),
        ("muse", s(&["resume", "--last"])),
        (
            "muse",
            s(&["resume", "0f3d2a5c-9c1e-4b7e-8f6a-2b1c3d4e5f60"]),
        ),
        ("muse", s(&["--no-session-log"])),
        ("muse", s(&["Summarize the open pull requests"])),
        (
            "muse",
            s(&["--workspace", "/tmp/codex", "resume", "--last"]),
        ),
        ("muse", s(&["--root", "/srv/root", "--continue", "-c"])),
        (
            "/opt/muse/bin/muse",
            s(&["resume", "--last", "--no-session-log"]),
        ),
        ("env", s(&["muse", "resume", "--last"])),
        ("bash", s(&["-lc", "muse resume --last"])),
        ("cmd.exe", s(&["/C", "muse", "resume", "--last"])),
        ("Muse", s(&["resume", "--last"])),
        ("MUSE", s(&["--continue"])),
    ];
    for (shell, args) in &recipes {
        assert_eq!(
            &strip_auto_injected_args(shell, args),
            args,
            "shell={shell:?} args={args:?}"
        );
    }

    let mgr = SessionManager::new();
    let configured: Vec<String> = Vec::new();
    let session = mgr
        .create_session(
            "muse".to_string(),
            configured.clone(),
            "/tmp/work".to_string(),
            Some("muse".to_string()),
            Some("Muse".to_string()),
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    mgr.set_effective_shell_args(session.id, s(&["resume", "--last"]))
        .await;

    let snapshot = snapshot_sessions(&mgr).await;
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].shell, "muse");
    assert_eq!(snapshot[0].shell_args, configured);
    assert!(
        !snapshot[0]
            .shell_args
            .iter()
            .any(|a| a == "resume" || a == "--last"),
        "effective resume tokens must never persist"
    );

    // Round trip through the persisted JSON shape keeps the configured argv.
    let json = serde_json::to_string(&snapshot[0]).expect("serialize");
    let back: PersistedSession = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.shell, "muse");
    assert_eq!(back.shell_args, configured);
    assert_eq!(
        strip_auto_injected_args(&back.shell, &back.shell_args),
        configured
    );
}

#[tokio::test]
async fn snapshot_sessions_preserves_raise_hand_communication() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "powershell.exe".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            true,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");
    let now = chrono::DateTime::parse_from_rfc3339("2026-06-30T11:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let (_, expected) = mgr.raise_hand(session.id, now).await.unwrap();

    let snapshot = snapshot_sessions(&mgr).await;

    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].communication, Some(expected));
}

// (#630/#631) The durable fresh intent flows Session -> SessionInfo carrier ->
// PersistedSession through the real snapshot path, so a restart-fresh session's
// intent reaches disk (and is not lost at the SessionInfo boundary).
#[tokio::test]
async fn snapshot_sessions_preserves_start_fresh_on_restore() {
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "claude-mb".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");

    // Default snapshots as resume (false).
    let before = snapshot_sessions(&mgr).await;
    assert_eq!(before.len(), 1);
    assert!(!before[0].start_fresh_on_restore);

    mgr.set_start_fresh_on_restore(session.id, true).await;

    let after = snapshot_sessions(&mgr).await;
    assert_eq!(after.len(), 1);
    assert!(after[0].start_fresh_on_restore);
}

#[tokio::test]
async fn persist_current_state_captures_snapshot_inside_save_lock() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mgr = SessionManager::new();
    let session = mgr
        .create_session(
            "powershell.exe".to_string(),
            Vec::new(),
            "C:\\tmp".to_string(),
            None,
            None,
            Vec::new(),
            false,
            crate::pty::backend::SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create_session should succeed");

    let stale_snapshot = snapshot_sessions(&mgr).await;
    assert!(stale_snapshot[0].telegram_bot_id.is_none());

    let guard = super::super::acquire_carrier_mutation_permit().await;
    let mut persist = Box::pin(persist_current_state_to_dir_result(&mgr, temp.path()));
    let timed_out = tokio::time::timeout(Duration::from_millis(25), &mut persist)
        .await
        .is_err();
    assert!(timed_out, "persistence should wait for the save lock");

    mgr.set_telegram_bot_id_with_carrier_permit(&guard, session.id, Some("bot-1".into()))
        .await;
    drop(guard);
    persist.await.expect("persist current state");

    let saved =
        std::fs::read_to_string(temp.path().join("sessions.json")).expect("read sessions.json");
    let rows: Vec<PersistedSession> = serde_json::from_str(&saved).expect("deserialize");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].telegram_bot_id.as_deref(), Some("bot-1"));
}

#[test]
fn save_sessions_to_dir_returns_create_dir_error_for_file_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let file_path = temp.path().join("not-a-dir");
    std::fs::write(&file_path, "already a file").expect("write file target");

    let err = save_sessions_to_dir(&file_path, &[]).expect_err("file target should fail");

    assert!(
        err.contains("Failed to create config directory"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn persist_current_state_result_succeeds_for_simple_manager() {
    let mgr = SessionManager::new();
    mgr.create_session(
        "powershell.exe".to_string(),
        Vec::new(),
        "C:\\tmp".to_string(),
        None,
        None,
        Vec::new(),
        false,
        crate::pty::backend::SessionBackendKind::LocalProcess,
    )
    .await
    .expect("create_session should succeed");

    persist_current_state_result(&mgr)
        .await
        .expect("persist_current_state_result should succeed");
}

#[test]
fn characterize_3254_p03_loader_errors() {
    crate::logging::test_install_logger();
    let temp = tempfile::tempdir().unwrap();
    let tag = uuid::Uuid::new_v4().to_string();
    let missing = temp.path().join(format!("missing-{tag}.json"));
    let malformed = temp.path().join(format!("malformed-{tag}.json"));
    let directory = temp.path().join(format!("directory-{tag}"));
    let input = format!(
        r#"[{{"name":"{tag}","shell":"cmd","shellArgs":[],"workingDirectory":"C:/x",
        "communication":{{"kind":"malformed-{tag}","visible":true,"updatedAt":"x"}}}}]"#
    );
    std::fs::write(&malformed, &input).unwrap();
    std::fs::create_dir(&directory).unwrap();
    let parse_error = serde_json::from_str::<Vec<PersistedSession>>(&input).unwrap_err();
    let read_error = std::fs::read_to_string(&directory).unwrap_err();
    let parse_message = format!("Failed to parse sessions file: {parse_error}");
    let read_message = format!("Failed to read sessions file: {read_error}");
    for (path, expected) in [
        (&missing, None),
        (&malformed, Some(parse_message.as_str())),
        (&directory, Some(read_message.as_str())),
    ] {
        let before = crate::logging::test_tee_snapshot().len();
        assert!(super::super::load_sessions_from_path_with_settings(path, None).is_empty());
        let logs = crate::logging::test_tee_snapshot();
        let entries: Vec<_> = logs[before..]
            .iter()
            .map(|line| {
                let mut parts = line.splitn(3, ' ');
                let level = parts.next().unwrap();
                let _target = parts.next().unwrap();
                (level, parts.next().unwrap())
            })
            .collect();
        if let Some(message) = expected {
            assert!(
                entries.contains(&("ERROR", message)),
                "missing exact error: {message}"
            );
        } else {
            assert!(!entries.iter().any(|(_, message)| message.contains(&tag)));
        }
        let before = crate::logging::test_tee_snapshot().len();
        assert!(super::super::load_sessions_raw_from_path(path).is_empty());
        let logs = crate::logging::test_tee_snapshot();
        assert!(!logs[before..].iter().any(|line| line.contains(&tag)));
    }
}
