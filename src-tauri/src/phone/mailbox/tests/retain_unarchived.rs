use super::*;

#[test]
fn resolve_remote_pty_command_rejects_pi_compact_and_lookalikes() {
    for (shell, command) in [("pi", "compact"), ("pip", "clear"), ("cmd.exe", "clear")] {
        let error = resolve_remote_pty_command(shell, command).unwrap_err();
        assert_eq!(
            error,
            format!(
                "Cannot execute logical remote command '{command}': session shell '{shell}' \
                    has no verified mapping. Claude / Codex / Antigravity / Cursor agent direct \
                    shells use /clear and /compact; exact Pi uses /new for clear only. cmd / pwsh \
                    outer wrappers and Pi compact are unsupported."
            )
        );
    }
}

#[test]
fn resolve_remote_pty_command_preserves_established_and_cursor_controls() {
    for shell in ["claude-wrapper", "codex.exe", "agy.cmd", "agent.exe"] {
        assert_eq!(
            resolve_remote_pty_command(shell, "clear").unwrap().text,
            "/clear"
        );
        assert_eq!(
            resolve_remote_pty_command(shell, "compact").unwrap().text,
            "/compact"
        );
    }
}

#[test]
fn is_permanent_delivery_error_classifies_only_terminal_shapes() {
    assert!(is_permanent_delivery_error(
        "Unsupported logical remote command 'Clear'. Allowed values: clear, compact"
    ));
    assert!(is_permanent_delivery_error(
        "Cannot execute logical remote command 'compact': session shell 'pi' has no verified \
            mapping."
    ));
    assert!(is_permanent_delivery_error(
        "Could not resolve inbox for agent 'missing'"
    ));
    // #1635-2 D6: the invalid-dispatch-profile marker is terminal.
    assert!(is_permanent_delivery_error(ERR_INVALID_DISPATCH_PROFILE));
    assert!(is_permanent_delivery_error(
        "send: requestedProfile must be a single letter A through Z (hand-written outbox)"
    ));
    for transient in [
        "Cannot execute remote command 'clear': agent is busy (not idle)",
        "Session not found: 00000000-0000-0000-0000-000000000000",
        "Failed to spawn session",
        "PTY write failed",
    ] {
        assert!(!is_permanent_delivery_error(transient), "{transient}");
    }
}

#[test]
fn retain_unarchived_session_dirs_drops_dirs_under_archived_projects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let archived = temp.path().join("archived");
    let archived_agent = archived.join(".ac").join("wg-1").join("__agent_archived");
    let active_agent = temp
        .path()
        .join("active")
        .join(".ac")
        .join("wg-1")
        .join("__agent_active");
    std::fs::create_dir_all(&archived_agent).expect("archived agent");
    std::fs::create_dir_all(&active_agent).expect("active agent");
    let roots = crate::config::sessions_persistence::normalize_project_roots(&[archived
        .to_string_lossy()
        .to_string()]);
    let archived_id = Uuid::new_v4();
    let active_id = Uuid::new_v4();

    let filtered = retain_unarchived_session_dirs(
        vec![
            (archived_id, archived_agent.to_string_lossy().to_string()),
            (active_id, active_agent.to_string_lossy().to_string()),
        ],
        &roots,
    );

    assert_eq!(
        filtered,
        vec![(active_id, active_agent.to_string_lossy().to_string())]
    );
}

#[test]
fn retain_unarchived_session_dirs_keeps_dirs_outside_every_project_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let archived = temp.path().join("archived");
    let ad_hoc = temp.path().join("scratch").join("__agent_ad_hoc");
    std::fs::create_dir_all(&archived).expect("archived");
    std::fs::create_dir_all(&ad_hoc).expect("ad hoc");
    let roots = crate::config::sessions_persistence::normalize_project_roots(&[archived
        .to_string_lossy()
        .to_string()]);
    let id = Uuid::new_v4();

    let filtered =
        retain_unarchived_session_dirs(vec![(id, ad_hoc.to_string_lossy().to_string())], &roots);

    assert_eq!(filtered, vec![(id, ad_hoc.to_string_lossy().to_string())]);
}

#[test]
fn retain_unarchived_session_dirs_returns_input_unchanged_when_archived_list_is_empty() {
    let id = Uuid::new_v4();
    let input = vec![(id, "Z:/does/not/exist".to_string())];
    let ptr = input.as_ptr();
    let capacity = input.capacity();

    let filtered = retain_unarchived_session_dirs(input, &[]);

    assert_eq!(filtered, vec![(id, "Z:/does/not/exist".to_string())]);
    assert_eq!(
        filtered.as_ptr(),
        ptr,
        "empty archived roots must skip the into_iter/collect round trip"
    );
    assert_eq!(filtered.capacity(), capacity);
}

#[test]
fn mailbox_poll_bypasses_session_dir_filter_when_archived_list_is_empty() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/phone/mailbox/wake_workers.rs"
    ))
    .expect("read mailbox/wake_workers.rs");
    let production = source
        .split("#[cfg(test)]\nmod tests")
        .next()
        .expect("production mailbox source");
    let bypass = production
        .find("let session_dirs = if archived.is_empty()")
        .expect("archived-empty bypass");
    let filter = production
        .find("retain_unarchived_session_dirs(session_dirs, &roots)")
        .expect("archived session filter call");

    assert!(
        bypass < filter,
        "MailboxPoller::poll must bypass filtering before calling the archived-session filter"
    );
}

#[tokio::test]
async fn resolve_repo_path_filters_archived_session_dirs() {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("proj-a");
    let target = project
        .join(".ac")
        .join("wg-1-dev-team")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&target).expect("create target");
    let app = make_mailbox_app(temp.path());
    let app_handle = app_handle(&app);
    {
        let settings = app_handle.state::<SettingsState>();
        let mut cfg = settings.write().await;
        cfg.project_paths.clear();
        cfg.archived_project_paths = vec![project.to_string_lossy().to_string()];
    }
    let session_mgr = app_handle.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "shell".to_string(),
            Vec::new(),
            target.to_string_lossy().to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create archived session");
    }
    let poller = MailboxPoller::default();

    let resolved = poller
        .resolve_repo_path(CANONICAL_WAKE_TO, &app_handle)
        .await;

    assert_eq!(resolved, None);
}

#[tokio::test]
async fn resolve_wg_path_from_sessions_filters_archived_session_dirs() {
    let temp = tempfile::TempDir::new().unwrap();
    let project = temp.path().join("proj-a");
    let wg_root = project.join(".ac").join("wg-1-dev-team");
    let sender = wg_root.join("__agent_tech-lead");
    let target = wg_root.join("__agent_dev-rust");
    std::fs::create_dir_all(&sender).expect("create sender");
    std::fs::create_dir_all(&target).expect("create target");
    let app = make_mailbox_app(temp.path());
    let app_handle = app_handle(&app);
    {
        let settings = app_handle.state::<SettingsState>();
        let mut cfg = settings.write().await;
        cfg.archived_project_paths = vec![project.to_string_lossy().to_string()];
    }
    let session_mgr = app_handle.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
    {
        let mgr = session_mgr.read().await;
        mgr.create_session(
            "shell".to_string(),
            Vec::new(),
            sender.to_string_lossy().to_string(),
            None,
            None,
            Vec::new(),
            false,
            SessionBackendKind::LocalProcess,
        )
        .await
        .expect("create archived sibling session");
    }
    let poller = MailboxPoller::default();

    let resolved = poller
        .resolve_wg_path_from_sessions(&app_handle, LOCAL_WAKE_TO)
        .await;

    assert_eq!(resolved, None);
}

// ── §224 D.5a — wait_for_restore_or_session unit tests ──

#[test]
fn container_file_notification_adapter_reads_inline_content() {
    let temp = tempfile::TempDir::new().unwrap();
    let wg_root = temp.path().join("proj").join(".ac").join("wg-1-team");
    let sender_root = wg_root.join("__agent_sender");
    let messaging = wg_root.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    std::fs::create_dir_all(&sender_root).unwrap();
    std::fs::create_dir_all(&messaging).unwrap();
    let filename = "20260704-000000-wg1-a-to-wg1-b-hello.md";
    let path = messaging.join(filename);
    std::fs::write(&path, "inline body").unwrap();
    let notification = crate::phone::messaging::format_file_notification(&path.to_string_lossy());

    let got = inline_body_from_file_notification(&notification, &sender_root).unwrap();

    assert_eq!(got.as_deref(), Some("inline body"));
}

#[test]
fn container_file_notification_adapter_rejects_oversize_content() {
    let temp = tempfile::TempDir::new().unwrap();
    let wg_root = temp.path().join("proj").join(".ac").join("wg-1-team");
    let sender_root = wg_root.join("__agent_sender");
    let messaging = wg_root.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    std::fs::create_dir_all(&sender_root).unwrap();
    std::fs::create_dir_all(&messaging).unwrap();
    let filename = "20260704-000000-wg1-a-to-wg1-b-large.md";
    let path = messaging.join(filename);
    std::fs::write(
        &path,
        vec![b'x'; crate::api::message_store::INLINE_BODY_MAX_BYTES + 1],
    )
    .unwrap();
    let notification = crate::phone::messaging::format_file_notification(&path.to_string_lossy());

    let err = inline_body_from_file_notification(&notification, &sender_root).unwrap_err();

    assert!(err.contains("inline cap"));
}

#[test]
fn container_file_notification_adapter_ignores_plain_body() {
    assert_eq!(
        inline_body_from_file_notification("plain body", Path::new("unused")).unwrap(),
        None
    );
}

#[test]
fn db_origin_container_adapter_does_not_read_crafted_file_notification() {
    let temp = tempfile::TempDir::new().unwrap();
    let sender_wg = temp.path().join("proj").join(".ac").join("wg-1-team");
    let sender_root = sender_wg.join("__agent_sender");
    let victim_wg = temp.path().join("proj").join(".ac").join("wg-2-team");
    let victim_messaging = victim_wg.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    std::fs::create_dir_all(&sender_root).unwrap();
    std::fs::create_dir_all(&victim_messaging).unwrap();
    let filename = "20260704-000000-wg2-a-to-wg2-b-secret.md";
    let victim_file = victim_messaging.join(filename);
    std::fs::write(&victim_file, "victim secret").unwrap();
    let crafted = crate::phone::messaging::format_file_notification(&victim_file.to_string_lossy());

    let got = container_body_override_for_delivery(
        WakeDeliveryOrigin::DbQueue,
        &crafted,
        Some(&sender_root),
    )
    .unwrap();

    assert_eq!(got, None);
    assert!(!crafted.contains("victim secret"));
}

#[test]
fn filesystem_origin_container_adapter_rejects_cross_workgroup_notification() {
    let temp = tempfile::TempDir::new().unwrap();
    let sender_wg = temp.path().join("proj").join(".ac").join("wg-1-team");
    let sender_root = sender_wg.join("__agent_sender");
    let victim_wg = temp.path().join("proj").join(".ac").join("wg-2-team");
    let victim_messaging = victim_wg.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    std::fs::create_dir_all(&sender_root).unwrap();
    std::fs::create_dir_all(&victim_messaging).unwrap();
    let filename = "20260704-000000-wg2-a-to-wg2-b-secret.md";
    let victim_file = victim_messaging.join(filename);
    std::fs::write(&victim_file, "victim secret").unwrap();
    let crafted = crate::phone::messaging::format_file_notification(&victim_file.to_string_lossy());

    let err = container_body_override_for_delivery(
        WakeDeliveryOrigin::FilesystemPoller,
        &crafted,
        Some(&sender_root),
    )
    .unwrap_err();

    assert!(err.contains("outside sender messaging directory"));
}

#[test]
fn exited_temporary_sessions_are_not_persistent_restart_candidates() {
    let mut temp = make_session_info(
        "temp",
        "[temp] legacy wake",
        "C:/target",
        SessionStatus::Exited(0),
        false,
    );
    temp.created_at = "2026-07-20T01:00:00Z".to_string();
    let mut persistent = make_session_info(
        "persistent",
        "member",
        "C:/target",
        SessionStatus::Exited(0),
        false,
    );
    persistent.created_at = "2026-07-19T01:00:00Z".to_string();
    assert!(!is_persistent_exited_pty_candidate(&temp));
    assert!(is_persistent_exited_pty_candidate(&persistent));
    assert!(select_persistent_exited_pty_candidate(&[temp.clone()], |_| true).is_none());
    let rows = vec![persistent, temp];
    assert_eq!(
        select_persistent_exited_pty_candidate(&rows, |_| true).map(|session| session.id.as_str()),
        Some("persistent"),
        "a newer exited temp must not hide the older persistent restart candidate"
    );
}

#[test]
fn configured_prefix_wrapper_requires_retained_and_current_spawn_provenance() {
    let cwd = tempfile::tempdir().unwrap();
    let mut settings = AppSettings {
        agents: vec![wake_agent(
            "codex",
            "Codex wrapper",
            "codex-shell.exe --yolo",
        )],
        ..Default::default()
    };
    let spawn = crate::commands::session::build_configured_agent_spawn_for_cwd(
        &settings,
        "codex",
        &cwd.path().to_string_lossy(),
        None,
    )
    .unwrap()
    .unwrap();
    let mut session = make_session_info(
        "wrapper",
        "member",
        &cwd.path().to_string_lossy(),
        SessionStatus::Idle,
        true,
    );
    session.shell = spawn.shell.clone();
    session.shell_args = spawn.shell_args.clone();
    session.agent_id = Some(spawn.trusted_agent_id.clone());
    session.agent_kind = Some(crate::session::profile::CodingAgentKind::Codex);
    session.profile_content_hash = Some(spawn.profile_content_hash.clone());
    session.trusted_configured_spawn = true;
    assert!(session_has_current_pty_submission_provenance(
        &session, &settings
    ));

    let mut untrusted = session.clone();
    untrusted.trusted_configured_spawn = false;
    assert!(!session_has_current_pty_submission_provenance(
        &untrusted, &settings
    ));

    settings.agents[0].command = "codex-other.exe --yolo".to_string();
    assert!(!session_has_current_pty_submission_provenance(
        &session, &settings
    ));
}
