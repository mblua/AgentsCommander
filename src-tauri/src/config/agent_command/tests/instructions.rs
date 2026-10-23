use super::*;

#[test]
fn default_instructions_filename_maps_by_detected_kind() {
    assert_eq!(
        default_instructions_filename_for_command("claude"),
        "CLAUDE.md"
    );
    assert_eq!(
        default_instructions_filename_for_command("claude-mb --effort max"),
        "CLAUDE.md"
    );
    assert_eq!(
        default_instructions_filename_for_command("cmd.exe /c claude"),
        "CLAUDE.md"
    );
    assert_eq!(
        default_instructions_filename_for_command("agy -m gpt-5"),
        "AGENTS.md"
    );
    assert_eq!(
        default_instructions_filename_for_command("codex"),
        "AGENTS.md"
    );
    assert_eq!(
        default_instructions_filename_for_command("pi --model claude-sonnet"),
        "AGENTS.md"
    );
    assert_eq!(
        default_instructions_filename_for_command("pi --model codex-model"),
        "AGENTS.md"
    );
    assert_eq!(
        default_instructions_filename_for_command("pi --provider gemini"),
        "AGENTS.md"
    );
    assert_eq!(
        default_instructions_filename_for_command("cmd /C pi --model claude-sonnet"),
        "AGENTS.md"
    );
    assert_eq!(
        default_instructions_filename_for_command("opencode"),
        "AGENTS.md"
    );
    assert_eq!(
        default_instructions_filename_for_command("my-custom-agent"),
        "AGENTS.md"
    );
    // Unparseable (unclosed quote) and empty both fall back to AGENTS.md.
    assert_eq!(
        default_instructions_filename_for_command("codex \"unterminated"),
        "AGENTS.md"
    );
    assert_eq!(default_instructions_filename_for_command(""), "AGENTS.md");
}

/// #1873 - direct/absolute Muse maps to AGENTS.md through the same detector
/// the launch path uses; a Codex-looking value does not change the target.
#[test]
fn muse_default_instructions_filename_is_agents_md() {
    for command in [
        "muse",
        "/opt/muse/bin/muse",
        "muse --workspace /tmp/codex resume --last",
        "muse --model claude-sonnet",
    ] {
        assert_eq!(
            default_instructions_filename_for_command(command),
            "AGENTS.md",
            "command={command:?}"
        );
        let normalized = normalize_legacy_agent_command(command).unwrap();
        assert_eq!(
            crate::session::profile::CodingAgentKind::detect(
                &normalized.shell,
                &normalized.shell_args
            ),
            Some(crate::session::profile::CodingAgentKind::Muse),
            "command={command:?}"
        );
    }
    let muse = agent("muse", "muse");
    assert_eq!(resolve_instructions_filename(&muse), "AGENTS.md");
    assert_eq!(
        resolve_target_filename(
            Some("muse"),
            &AppSettings {
                agents: vec![muse],
                ..AppSettings::default()
            },
            None,
        )
        .as_deref(),
        Some("AGENTS.md")
    );
}

/// #1873 - an explicit, safe filename wins over the Muse default.
#[test]
fn muse_explicit_instructions_filename_wins() {
    let mut muse = agent("muse", "/opt/muse/bin/muse");
    muse.instructions_filename = Some("  Squad.md  ".to_string());
    assert_eq!(resolve_instructions_filename(&muse), "Squad.md");
    let settings = AppSettings {
        agents: vec![muse.clone()],
        ..AppSettings::default()
    };
    assert_eq!(
        resolve_target_filename(Some("muse"), &settings, None).as_deref(),
        Some("Squad.md")
    );
    assert_eq!(
        managed_instructions_filenames(&settings),
        vec!["Squad.md".to_string()]
    );
    // Unsafe or blank explicit values fall back to the Muse default.
    muse.instructions_filename = Some("../escape.md".to_string());
    assert_eq!(resolve_instructions_filename(&muse), "AGENTS.md");
    muse.instructions_filename = Some("   ".to_string());
    assert_eq!(resolve_instructions_filename(&muse), "AGENTS.md");
}

#[test]
fn resolve_instructions_filename_prefers_valid_explicit() {
    let mut a = agent("x", "codex");
    // No explicit value -> command-derived default.
    assert_eq!(resolve_instructions_filename(&a), "AGENTS.md");
    // Valid explicit value wins.
    a.instructions_filename = Some("Squad.md".to_string());
    assert_eq!(resolve_instructions_filename(&a), "Squad.md");
    // Surrounding whitespace is trimmed.
    a.instructions_filename = Some("  Squad.md  ".to_string());
    assert_eq!(resolve_instructions_filename(&a), "Squad.md");
    // Whitespace-only -> command default.
    a.instructions_filename = Some("   ".to_string());
    assert_eq!(resolve_instructions_filename(&a), "AGENTS.md");
    // Unsafe value (path escape) silently falls back to the default; it never
    // reaches the writer.
    a.instructions_filename = Some("../escape.md".to_string());
    assert_eq!(resolve_instructions_filename(&a), "AGENTS.md");
    // Claude command default is CLAUDE.md when no explicit value is set.
    let claude = agent("c", "claude");
    assert_eq!(resolve_instructions_filename(&claude), "CLAUDE.md");
}

#[test]
fn managed_instructions_filenames_dedupes_across_agents() {
    let settings = AppSettings {
        agents: vec![
            agent("claude", "claude"),     // CLAUDE.md
            agent("codex", "codex"),       // AGENTS.md
            agent("agy", "agy"),           // AGENTS.md
            agent("opencode", "opencode"), // AGENTS.md (dup of codex)
        ],
        ..AppSettings::default()
    };
    let mut got = managed_instructions_filenames(&settings);
    got.sort();
    assert_eq!(got, vec!["AGENTS.md".to_string(), "CLAUDE.md".to_string()]);
}

#[test]
fn resolve_target_filename_covers_all_four_branches() {
    use crate::config::session_context::ManagedContextTarget;
    let mut configured = agent("opencode", "opencode");
    configured.instructions_filename = Some("Team.md".to_string());
    let settings = AppSettings {
        agents: vec![configured, agent("codex", "codex")],
        ..AppSettings::default()
    };

    // 1. configured agent with explicit valid filename wins.
    assert_eq!(
        resolve_target_filename(Some("opencode"), &settings, None).as_deref(),
        Some("Team.md")
    );
    // 2. configured agent without explicit -> command default (overrides detection).
    assert_eq!(
        resolve_target_filename(Some("codex"), &settings, Some(ManagedContextTarget::Claude))
            .as_deref(),
        Some("AGENTS.md")
    );
    // 3. unknown id -> detection fallback.
    assert_eq!(
        resolve_target_filename(
            Some("ghost"),
            &settings,
            Some(ManagedContextTarget::Antigravity)
        )
        .as_deref(),
        Some("AGENTS.md")
    );
    // 3b. no id -> detection fallback.
    assert_eq!(
        resolve_target_filename(None, &settings, Some(ManagedContextTarget::Codex)).as_deref(),
        Some("AGENTS.md")
    );
    // 4. neither configured agent nor detection -> None.
    assert_eq!(resolve_target_filename(None, &settings, None), None);
    assert_eq!(
        resolve_target_filename(Some("ghost"), &settings, None),
        None
    );
}

#[test]
fn is_safe_instructions_filename_accepts_bare_md_names() {
    for ok in [
        "AGENTS.md",
        "CLAUDE.md",
        "GEMINI.md",
        "MyTeam.md",
        "AGENTS.MD",
        "a.md",
        "my-notes.md",
        // COM0/LPT0 are NOT reserved devices.
        "COM0.md",
        "LPT0.md",
    ] {
        assert!(is_safe_instructions_filename(ok), "should accept {ok:?}");
    }
}

#[test]
fn is_safe_instructions_filename_rejects_unsafe_names() {
    let bad = [
        "",               // empty
        "   ",            // whitespace only
        ".md",            // empty stem
        "AGENTS.txt",     // wrong extension
        "AGENTS",         // no extension
        "a/b.md",         // forward separator
        "a\\b.md",        // backslash separator
        "..\\x.md",       // traversal + separator
        "../x.md",        // traversal
        "a..md",          // contains ..
        "C:x.md",         // drive prefix (colon)
        "AGENTS.md:evil", // NTFS Alternate Data Stream (colon)
        "AGENTS.md ",     // trailing space
        "AGENTS.md.",     // trailing dot
        "AGENTS .md",     // space immediately before extension
        "a\nb.md",        // control char
        "CON.md",         // reserved device
        "con.md",         // reserved device (case-insensitive)
        "PRN.md",
        "aux.md",
        "NUL.md",
        "nul.md",
        "COM1.md",
        "com9.md",
        "LPT1.md",
        "lpt9.md",
        "CON.foo.md",         // device name resolved by the segment before the first dot
        "nul.x.md",           // device name (case-insensitive) with extra dot segment
        "last_ac_context.md", // G9 internal sentinel
        "LAST_AC_CONTEXT.md", // G9 (case-insensitive)
    ];
    for name in bad {
        assert!(
            !is_safe_instructions_filename(name),
            "should reject {name:?}"
        );
    }
    // Length cap (> 128 chars).
    let long = format!("{}.md", "a".repeat(130));
    assert!(
        !is_safe_instructions_filename(&long),
        "should reject an over-long name"
    );
}

// #576 follow-up - auto-create OPENCODE_CONFIG_DIR before spawn.
