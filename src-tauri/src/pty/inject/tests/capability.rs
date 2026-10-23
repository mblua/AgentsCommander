use super::*;

fn supported_session() -> Session {
    Session {
        id: Uuid::new_v4(),
        name: "coordinator".to_string(),
        shell: "codex".to_string(),
        shell_args: Vec::new(),
        backend_kind: SessionBackendKind::LocalProcess,
        effective_shell_args: None,
        created_at: Utc::now(),
        working_directory: "C:/replica".to_string(),
        status: SessionStatus::Running,
        waiting_for_input: false,
        communication: None,
        pending_review: false,
        last_prompt: None,
        agent_id: Some("codex-profile".to_string()),
        agent_label: Some("Codex".to_string()),
        git_repos: Vec::new(),
        is_coordinator: true,
        is_root_agent: false,
        git_repos_gen: 0,
        agent_turn_armed: false,
        token: Uuid::new_v4(),
        agent_kind: None,
        requested_profile: None,
        effective_profile: None,
        profile_fallback_chain: Vec::new(),
        profile_fallback_applied: false,
        match_tier: None,
        original_profile_letter: None,
        effective_codex_home: None,
        resolved_claude_projects_dir: None,
        profile_content_hash: None,
        trusted_configured_spawn: false,
        telegram_bot_id: None,
        was_detached: false,
        detached_geometry: None,
        start_fresh_on_restore: false,
        context_percent: None,
    }
}

#[test]
fn supported_agent_final_snapshot_rejects_unsafe_recipient_records() {
    let valid = supported_session();
    assert!(validate_supported_agent_session(&valid, valid.id).is_ok());

    for shell in ["hermes", "opencode", "grok"] {
        let mut widened = supported_session();
        widened.shell = shell.to_string();
        assert!(
            validate_supported_agent_session(&widened, widened.id).is_ok(),
            "shell={shell:?}"
        );
    }

    let mut root = supported_session();
    root.is_root_agent = true;
    assert!(validate_supported_agent_session(&root, root.id).is_err());

    let mut exited = supported_session();
    exited.status = SessionStatus::Exited(0);
    assert!(validate_supported_agent_session(&exited, exited.id).is_err());

    let mut agentless = supported_session();
    agentless.agent_id = None;
    assert!(validate_supported_agent_session(&agentless, agentless.id).is_err());

    let mut shell = supported_session();
    shell.shell = "pwsh".to_string();
    assert!(validate_supported_agent_session(&shell, shell.id).is_err());
}

#[test]
fn direct_shell_capability_matrix() {
    let pi_positive = [
        "pi",
        "PI",
        "pi.exe",
        "Pi.CMD",
        "pi.ps1",
        r"C:\Tools\pi.exe",
        r"\\server\share\pi.cmd",
        r"\\?\C:\Tools\pi.exe",
        "/usr/local/bin/pi",
        "  pi  ",
    ];
    for shell in pi_positive {
        assert!(needs_explicit_enter(shell), "Pi positive: {shell:?}");
        assert_eq!(
            resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
            Some("/new"),
            "Pi clear mapping: {shell:?}"
        );
        assert_eq!(
            resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
            None,
            "Pi compact remains unsupported: {shell:?}"
        );
        assert!(supports_auto_self_maintenance(shell));
        assert!(
            supports_self_handoff_switch(shell),
            "Pi switch source: {shell:?}"
        );
    }

    let explicit_submit_positive = [
        "hermes",
        "Hermes",
        "hermes.exe",
        "HERMES.CMD",
        "hermes.ps1",
        "opencode",
        "OpenCode",
        "opencode.exe",
        "OpenCode.CMD",
        "opencode.ps1",
        "grok",
        "GROK",
        "grok.exe",
        "Grok.CMD",
        "grok.ps1",
        "  grok  ",
        "/usr/local/bin/hermes",
        "/usr/local/bin/opencode",
    ];
    for shell in explicit_submit_positive {
        assert!(
            needs_explicit_enter(shell),
            "ExplicitSubmit positive: {shell:?}"
        );
        assert_eq!(
            resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
            None,
            "ExplicitSubmit clear remains unsupported: {shell:?}"
        );
        assert_eq!(
            resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
            None,
            "ExplicitSubmit compact remains unsupported: {shell:?}"
        );
        assert!(
            !supports_auto_self_maintenance(shell),
            "ExplicitSubmit maintenance stays off: {shell:?}"
        );
        assert!(
            !supports_self_handoff_switch(shell),
            "ExplicitSubmit switch stays off: {shell:?}"
        );
    }

    let unsupported = [
        "pip",
        "pipx",
        "ping",
        "pixel",
        "pi-agent",
        "pi2",
        "pi-claude",
        "hermes-wrapper",
        "hermes-cli",
        "my-hermes",
        "opencode-proxy",
        "opencode2",
        "opencode-tui",
        "grok-build",
        "grokx",
        "grok-cli",
        r"C:\pi\runner.exe",
        "agy-proxy",
        "agyctl",
        "cmd.exe",
        "pwsh",
        "",
        "   ",
        "bash",
    ];
    for shell in unsupported {
        assert!(!needs_explicit_enter(shell), "unsupported: {shell:?}");
        assert_eq!(
            resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
            None
        );
        assert_eq!(
            resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
            None
        );
        assert!(!supports_auto_self_maintenance(shell));
        assert!(!supports_self_handoff_switch(shell));
    }

    for shell in [
        "claude",
        "claude-pi",
        "codex-wrapper.cmd",
        "codex-proxy.exe",
        "agy",
        "antigravity.exe",
    ] {
        assert!(needs_explicit_enter(shell));
        assert_eq!(
            resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
            Some("/clear")
        );
        assert_eq!(
            resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
            Some("/compact")
        );
        assert!(supports_auto_self_maintenance(shell));
        assert!(supports_self_handoff_switch(shell));
    }

    for shell in ["agent", "agent.exe", r"C:\Cursor\agent.cmd"] {
        assert!(needs_explicit_enter(shell));
        assert_eq!(
            resolve_logical_command_text(shell, LogicalPtyCommand::Clear),
            Some("/clear")
        );
        assert_eq!(
            resolve_logical_command_text(shell, LogicalPtyCommand::Compact),
            Some("/compact")
        );
        assert!(!supports_auto_self_maintenance(shell));
        assert!(supports_self_handoff_switch(shell));
    }
    assert!(!needs_explicit_enter("agentctl"));
    assert!(!needs_explicit_enter("agentic"));
}

#[test]
fn logical_command_parser_and_boundary_matrix() {
    assert_eq!(
        LogicalPtyCommand::from_wire_value("clear"),
        Some(LogicalPtyCommand::Clear)
    );
    assert_eq!(
        LogicalPtyCommand::from_wire_value("compact"),
        Some(LogicalPtyCommand::Compact)
    );
    for value in ["Clear", "COMPACT", "", "new"] {
        assert_eq!(LogicalPtyCommand::from_wire_value(value), None);
    }
    assert!(LogicalPtyCommand::Clear.creates_fresh_boundary());
    assert!(!LogicalPtyCommand::Compact.creates_fresh_boundary());
}
