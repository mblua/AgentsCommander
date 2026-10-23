use super::*;

// --- #1873 Muse workspace-latest automatic resume --------------------------

/// Test-only Muse settings row appended to `test_settings()`. Production ships
/// no Muse row here; the #1860 catalog is the only built-in.
pub(super) fn muse_test_settings(command: &str) -> AppSettings {
    let mut settings = test_settings();
    settings.agents.push(AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: "muse".to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: "Muse Code".to_string(),
        command: command.to_string(),
        color: "#0668E1".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus: None,
        backend: Default::default(),
    });
    settings
}

pub(super) fn muse_spawn_for(
    settings: &AppSettings,
    cwd: &str,
) -> crate::config::agent_command::AgentSpawnCommand {
    super::super::build_configured_agent_spawn_for_cwd(settings, "muse", cwd, None)
        .expect("resolve the configured Muse spawn")
        .expect("muse is configured in muse_test_settings")
}

const MUSE_SUPPORTED_HOST: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// The plan's LITERAL workspace-latest selector. Deliberately NOT derived from
/// `MUSE_PROFILE.resume_tokens`: these tests must fail when the constant under
/// test carries a wrong token (Grinch mutation `--last` -> `--first`).
pub(super) fn muse_resume_tokens() -> Vec<String> {
    vec!["resume".to_string(), "--last".to_string()]
}

/// Effective argv the backend must see for a resume-intent Muse launch on this host.
pub(super) fn expected_muse_resume_argv() -> Vec<String> {
    if MUSE_SUPPORTED_HOST {
        muse_resume_tokens()
    } else {
        Vec::new()
    }
}

/// #1873 - the predicate is true only for the exact trusted recipe; every veto
/// is independent, and the injector fails closed without mutation on a wrong
/// kind, fresh intent, missing provenance, or a second call.
#[test]
fn muse_resume_eligibility_is_exact_and_fail_closed() {
    let resolve = |settings: &AppSettings| {
        crate::config::agent_command::resolve_agent_spawn_command(
            settings, "muse", None, None, false,
        )
        .expect("Muse test spawn should resolve without filesystem preparation")
    };
    let bare = resolve(&muse_test_settings("muse"));
    let absolute = resolve(&muse_test_settings("/opt/muse/bin/muse"));
    let no_args: Vec<String> = Vec::new();

    // Positives (host-gated: Windows compiles the same predicate to false).
    assert_eq!(
        super::super::trusted_muse_auto_resume_spawn(Some(&bare), "muse", &no_args),
        MUSE_SUPPORTED_HOST
    );
    assert_eq!(
        super::super::trusted_muse_auto_resume_spawn(
            Some(&absolute),
            "/opt/muse/bin/muse",
            &no_args
        ),
        MUSE_SUPPORTED_HOST
    );

    // Vetoes: ad-hoc (no provenance), shell mismatch, args mismatch.
    assert!(!super::super::trusted_muse_auto_resume_spawn(
        None, "muse", &no_args
    ));
    assert!(!super::super::trusted_muse_auto_resume_spawn(
        Some(&bare),
        "/usr/bin/muse",
        &no_args
    ));
    assert!(!super::super::trusted_muse_auto_resume_spawn(
        Some(&bare),
        "muse",
        &["--workspace".to_string(), "/srv/work".to_string()]
    ));

    // Any configured argument suppresses injection, even when the recipe matches.
    for command in [
        "muse resume --last",
        "muse resume 0f3d2a5c-9c1e-4b7e-8f6a-2b1c3d4e5f60",
        "muse --workspace /srv/work",
        "muse --root /srv/root",
        "muse --no-session-log",
        "muse --help",
        "muse --version",
        "muse \"summarize the open pull requests\"",
        "muse --future-flag",
    ] {
        let spawn = resolve(&muse_test_settings(command));
        assert!(
            !super::super::trusted_muse_auto_resume_spawn(
                Some(&spawn),
                &spawn.shell,
                &spawn.shell_args
            ),
            "command={command:?}"
        );
    }

    // Prefix names, mixed case and wrappers are never Muse identity; `muse.exe`
    // IS Muse kind (exact stem) but its file name is not `muse`, so only the
    // injection predicate rejects it.
    for command in [
        "muse-agent",
        "Muse",
        "muse.exe",
        "/opt/muse/bin/",
        "env muse",
        "cmd /C muse",
        "bash -lc muse",
    ] {
        let spawn = resolve(&muse_test_settings(command));
        assert!(
            !super::super::trusted_muse_auto_resume_spawn(
                Some(&spawn),
                &spawn.shell,
                &spawn.shell_args
            ),
            "command={command:?}"
        );
    }

    // Containers never qualify.
    let mut container = muse_test_settings("muse");
    container.agents.last_mut().unwrap().backend = crate::config::settings::AgentBackendConfig {
        kind: SessionBackendKind::ContainerTransport,
        image: Some("ghcr.io/example/muse:beta".to_string()),
    };
    let container_spawn = resolve(&container);
    assert_eq!(
        SessionBackendKind::from(&container_spawn.backend),
        SessionBackendKind::ContainerTransport
    );
    assert!(!super::super::trusted_muse_auto_resume_spawn(
        Some(&container_spawn),
        "muse",
        &no_args
    ));

    // Injector: resume intent + trusted recipe mutates exactly once.
    let mut args: Vec<String> = Vec::new();
    assert_eq!(
        super::super::maybe_inject_muse_resume(
            Some(CodingAgentKind::Muse),
            Some(&bare),
            false,
            "muse",
            &mut args
        ),
        MUSE_SUPPORTED_HOST
    );
    assert_eq!(args, expected_muse_resume_argv());
    // A second call finds a non-empty vector and fails closed without mutation.
    let before = args.clone();
    assert!(!super::super::maybe_inject_muse_resume(
        Some(CodingAgentKind::Muse),
        Some(&bare),
        false,
        "muse",
        &mut args
    ));
    assert_eq!(args, before);

    // Fresh intent, wrong/no kind, and missing provenance never mutate.
    for (kind, spawn, fresh) in [
        (Some(CodingAgentKind::Muse), Some(&bare), true),
        (Some(CodingAgentKind::Codex), Some(&bare), false),
        (Some(CodingAgentKind::Pi), Some(&bare), false),
        (None, Some(&bare), false),
        (Some(CodingAgentKind::Muse), None, false),
    ] {
        let mut args: Vec<String> = Vec::new();
        assert!(!super::super::maybe_inject_muse_resume(
            kind, spawn, fresh, "muse", &mut args
        ));
        assert!(args.is_empty());
    }
    // Configured arguments: the injector leaves them byte-identical.
    let manual = resolve(&muse_test_settings(
        "muse --workspace /tmp/codex resume --last",
    ));
    let mut manual_args = manual.shell_args.clone();
    assert!(!super::super::maybe_inject_muse_resume(
        Some(CodingAgentKind::Muse),
        Some(&manual),
        false,
        &manual.shell,
        &mut manual_args
    ));
    assert_eq!(manual_args, manual.shell_args);
}

/// #1873 - fresh (`true`), resume (`false`) and durable-fresh restore intent all
/// flow through the single launch seam; only resume intent yields `resume --last`,
/// and only in the effective vector.
#[tokio::test]
async fn muse_fresh_and_resume_intents_share_one_launch_seam() {
    let settings = muse_test_settings("muse");
    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );

    let launch = |cwd: String, skip_auto_resume: bool, name: &str| {
        let spawn = muse_spawn_for(&settings, &cwd);
        let name = name.to_string();
        let app = app.handle().clone();
        let session_mgr = Arc::clone(&session_mgr);
        let pty_mgr = Arc::clone(&pty_mgr);
        async move {
            super::super::create_session_inner(
                &app,
                &session_mgr,
                &pty_mgr,
                spawn.shell.clone(),
                spawn.shell_args.clone(),
                cwd,
                Some(name),
                Some(spawn.trusted_agent_id.clone()),
                Some(spawn.trusted_agent_label.clone()),
                false,
                Vec::new(),
                skip_auto_resume,
                Some(spawn),
                None,
                None,
                CreateSelectionIntent::User,
            )
            .await
            .expect("Muse launch succeeds")
        }
    };

    // Ordinary create / explicit Restart Session / root fresh / mailbox cold /
    // loop cold: skip_auto_resume == true -> configured plain muse.
    let fresh_dir = tempfile::tempdir().unwrap();
    let fresh = launch(
        fresh_dir.path().to_string_lossy().to_string(),
        true,
        "muse fresh",
    )
    .await;
    assert_eq!(fresh.agent_kind, Some(CodingAgentKind::Muse));
    assert!(fresh.shell_args.is_empty());
    assert_eq!(fresh.effective_shell_args.as_deref(), Some(&[][..]));
    {
        let specs = backend.specs.lock().unwrap();
        let spec = specs.last().expect("fresh launch reached the backend");
        assert_eq!(spec.cmd, "muse");
        assert!(
            spec.args.is_empty(),
            "fresh Muse must stay plain: {:?}",
            spec.args
        );
    }

    // Startup restore / closed-coordinator reopen / dormant root / mailbox and
    // loop known-state wake: skip_auto_resume == false -> muse resume --last.
    let resume_dir = tempfile::tempdir().unwrap();
    let resumed = launch(
        resume_dir.path().to_string_lossy().to_string(),
        false,
        "muse resume",
    )
    .await;
    assert_eq!(resumed.agent_kind, Some(CodingAgentKind::Muse));
    assert!(
        resumed.shell_args.is_empty(),
        "configured argv never receives AC tokens"
    );
    assert_eq!(
        resumed.effective_shell_args.clone().unwrap_or_default(),
        expected_muse_resume_argv()
    );
    {
        let specs = backend.specs.lock().unwrap();
        let spec = specs.last().expect("resume launch reached the backend");
        assert_eq!(spec.cmd, "muse");
        assert_eq!(spec.args, expected_muse_resume_argv());
    }

    // Durable fresh intent on a restore: `start_fresh_on_restore == true` maps to
    // `skip_auto_resume_for_restore(true) == true` -> plain muse, stamped fresh.
    let durable_dir = tempfile::tempdir().unwrap();
    let durable_cwd = durable_dir.path().to_string_lossy().to_string();
    let durable_spawn = muse_spawn_for(&settings, &durable_cwd);
    let restore = crate::session::selection::SelectionTransaction::for_test(app.handle().clone());
    let durable = super::super::create_session_inner_for_restore(
        &restore,
        &session_mgr,
        &pty_mgr,
        durable_spawn.shell.clone(),
        durable_spawn.shell_args.clone(),
        durable_cwd,
        Some("muse durable fresh".to_string()),
        Some("muse".to_string()),
        Some(durable_spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        crate::skip_auto_resume_for_restore(true),
        Some(durable_spawn),
        None,
        None,
        Some(true),
        None,
        None,
        None,
    )
    .await
    .expect("durable-fresh restore launches");
    assert!(durable.start_fresh_on_restore);
    assert!(durable.shell_args.is_empty());
    assert_eq!(durable.effective_shell_args.as_deref(), Some(&[][..]));
    {
        let specs = backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 3, "exactly one spawn per launch");
        let spec = specs.last().unwrap();
        assert_eq!(spec.cmd, "muse");
        assert!(
            spec.args.is_empty(),
            "durable fresh must stay plain: {:?}",
            spec.args
        );
    }
    assert_eq!(session_mgr.read().await.list_sessions().await.len(), 3);
    close_test_coordinator(&app).await;
}

/// #1873 - a directly configured Muse recipe with a Codex-looking value and
/// manual `resume --last` passes pre-spawn validation, is preserved byte-for-byte,
/// and suppresses injection all the way to the backend.
#[tokio::test]
async fn muse_manual_collision_args_pass_validation_and_reach_backend_unchanged() {
    let settings = muse_test_settings("muse --workspace /tmp/codex resume --last");
    crate::config::settings::validate_agent_commands(&settings)
        .expect("direct Muse arguments are user-authored and valid");
    let configured = vec![
        "--workspace".to_string(),
        "/tmp/codex".to_string(),
        "resume".to_string(),
        "--last".to_string(),
    ];

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let spawn = muse_spawn_for(&settings, &cwd);
    assert_eq!(spawn.shell, "muse");
    assert_eq!(spawn.shell_args, configured);
    assert_eq!(
        CodingAgentKind::detect(&spawn.shell, &spawn.shell_args),
        Some(CodingAgentKind::Muse),
        "direct Muse outranks the Codex-looking value"
    );

    // Resume intent: the configured arguments still win (no injection).
    let created = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("muse manual".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("manual Muse launch succeeds");
    assert_eq!(created.agent_kind, Some(CodingAgentKind::Muse));
    assert_eq!(created.shell_args, configured);
    assert_eq!(created.effective_shell_args.as_ref(), Some(&configured));
    {
        let specs = backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].cmd, "muse");
        assert_eq!(
            specs[0].args, configured,
            "argv reaches the backend unchanged"
        );
    }
    close_test_coordinator(&app).await;
}

/// #1873 - the backend positive control: on macOS/Linux the spec and effective
/// vectors carry exactly the profile tokens while the configured argv stays
/// empty; generic agent/profile env flows unchanged; the kind/wire value is
/// Muse/"muse". Unsupported hosts stay plain.
#[tokio::test]
async fn trusted_muse_resume_reaches_backend_effective_argv_only() {
    let mut settings = muse_test_settings("muse");
    settings
        .agents
        .last_mut()
        .unwrap()
        .envs
        .push(crate::config::settings::CodingAgentEnv {
            key: "MUSE_TEST_AGENT_ENV".to_string(),
            value: "agent-row".to_string(),
            source: Default::default(),
            enabled: true,
        });
    settings
        .coding_agent_profiles
        .profiles_by_agent
        .entry("muse".to_string())
        .or_default()
        .insert(
            "A".to_string(),
            crate::config::settings::ProfileCellConfig {
                id_hash: None,
                id_hash_debug: None,
                preflight: None,
                preflight_timeout_seconds: None,
                enabled: true,
                command: String::new(),
                env: BTreeMap::from([(
                    "MUSE_TEST_PROFILE_ENV".to_string(),
                    "profile-cell".to_string(),
                )]),
                notes: String::new(),
            },
        );
    settings
        .coding_agent_profiles
        .default_profile_by_agent
        .insert("muse".to_string(), "A".to_string());

    let session_mgr = Arc::new(tokio::sync::RwLock::new(SessionManager::new()));
    let backend = Arc::new(CapturingSpawnBackend::default());
    let pty_mgr = Arc::new(Mutex::new(crate::pty::manager::PtyManager::new_for_test(
        backend.clone(),
    )));
    let app = session_test_app(
        settings.clone(),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
    );
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().to_string_lossy().to_string();
    let spawn = muse_spawn_for(&settings, &cwd);
    assert_eq!(spawn.shell, "muse");
    assert!(spawn.shell_args.is_empty());
    let agent_row = ("MUSE_TEST_AGENT_ENV".to_string(), "agent-row".to_string());
    let profile_row = (
        "MUSE_TEST_PROFILE_ENV".to_string(),
        "profile-cell".to_string(),
    );
    assert!(
        spawn.child_env.contains(&agent_row),
        "{:?}",
        spawn.child_env
    );
    assert!(
        spawn.child_env.contains(&profile_row),
        "{:?}",
        spawn.child_env
    );

    let created = super::super::create_session_inner(
        app.handle(),
        &session_mgr,
        &pty_mgr,
        spawn.shell.clone(),
        spawn.shell_args.clone(),
        cwd,
        Some("muse positive control".to_string()),
        Some(spawn.trusted_agent_id.clone()),
        Some(spawn.trusted_agent_label.clone()),
        false,
        Vec::new(),
        false,
        Some(spawn),
        None,
        None,
        CreateSelectionIntent::User,
    )
    .await
    .expect("trusted Muse resume launch succeeds");

    assert_eq!(created.agent_kind, Some(CodingAgentKind::Muse));
    assert_eq!(created.shell, "muse");
    assert!(
        created.shell_args.is_empty(),
        "configured args stay empty: {:?}",
        created.shell_args
    );
    let effective = created.effective_shell_args.clone().unwrap_or_default();
    let wire = serde_json::to_value(&created).expect("SessionInfo serializes");
    assert_eq!(wire["agentKind"], serde_json::json!("muse"));
    assert_eq!(wire["shellArgs"], serde_json::json!([]));
    // PRIMARY: the literal plan tokens, never read back from the profile constant.
    if MUSE_SUPPORTED_HOST {
        assert_eq!(effective, ["resume", "--last"]);
        assert_eq!(
            wire["effectiveShellArgs"],
            serde_json::json!(["resume", "--last"])
        );
    } else {
        assert!(
            effective.is_empty(),
            "unsupported hosts stay plain: {effective:?}"
        );
        assert_eq!(wire["effectiveShellArgs"], serde_json::json!([]));
    }
    // Secondary cross-check: the profile constant equals the same literal.
    assert_eq!(
        CodingAgentKind::Muse.profile().resume_tokens,
        ["resume", "--last"]
    );
    assert_eq!(effective, expected_muse_resume_argv());

    {
        let specs = backend.specs.lock().unwrap();
        assert_eq!(specs.len(), 1, "exactly one process attempt");
        let spec = &specs[0];
        assert_eq!(spec.cmd, "muse");
        if MUSE_SUPPORTED_HOST {
            assert_eq!(spec.args, ["resume", "--last"]);
        } else {
            assert!(spec.args.is_empty(), "{:?}", spec.args);
        }
        assert_eq!(spec.args, expected_muse_resume_argv());
        assert_eq!(spec.coding_agent, Some(CodingAgentKind::Muse));
        assert_eq!(spec.agent_id.as_deref(), Some("muse"));
        assert!(
            spec.configured_env.contains(&agent_row),
            "agent env reaches the child unchanged: {:?}",
            spec.configured_env
        );
        assert!(
            spec.configured_env.contains(&profile_row),
            "profile env reaches the child unchanged: {:?}",
            spec.configured_env
        );
        assert!(
            !spec
                .configured_env
                .iter()
                .any(|(key, _)| key.to_ascii_uppercase().contains("MUSE_HOME")),
            "no Muse-specific env key is minted: {:?}",
            spec.configured_env
        );
    }
    close_test_coordinator(&app).await;
}
