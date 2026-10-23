use super::*;

#[test]
fn default_context_embeds_filename_only_warning() {
    // #923 D3: the two-step `--send` guidance is emitted for agents that can
    // actually send. A `MessagingContextMode::None` root cannot (cli/send.rs
    // rejects a `--root` with no `wg-<N>-*` ancestor), so it now renders the
    // no-messaging-directory arm instead. Assert against a real WG replica.
    let out = default_context(
        "C:/fake/wg-7-dev-team/__agent_architect",
        None,
        &no_skill_section(),
    );
    assert!(out.contains("filename ONLY"));
    assert!(out.contains("never a path"));
    assert!(out.contains("--send <filename> --mode wake"));
    assert!(out.contains("YYYYMMDD-HHMMSS-<roomN>-<you>-to-<roomN>-<peer>-<slug>.md"));
}

#[test]
fn default_context_embeds_send_receipt_rule() {
    // #1596: the recipe mandates a captured `Queued:` enqueue receipt before
    // reporting a message as sent; a missing receipt means NOT enqueued.
    let out = default_context(
        "C:/fake/wg-7-dev-team/__agent_architect",
        None,
        &no_skill_section(),
    );
    assert!(out.contains("Sent requires captured Queued: <message-id>"));
    assert!(out.contains("Queued: <message-id>"));
    assert!(out.contains("Wait for reply."));
    assert!(out.contains("Do NOT use --get-output (blocks; non-interactive only)."));
    assert!(out.contains("absent receipt means NOT enqueued"));
}

#[cfg(target_os = "windows")]
#[test]
fn default_context_embeds_windows_shell_routing_pointer() {
    // #1605: the #1596 Git Bash paragraph moved to the
    // `{{HOST_PLATFORM_RULES}}` block (single source of truth); messaging now
    // carries only a pointer to it.
    let out = default_context(
        "C:/fake/wg-7-dev-team/__agent_architect",
        None,
        &no_skill_section(),
    );
    assert!(out.contains("**Windows:** see **Host Platform Rules** above."));
    assert!(out.contains("## Host Platform Rules"));
    assert!(out.contains("bash.exe"));
    assert!(out.contains("2>&1 | Out-String"));
}

#[cfg(target_os = "windows")]
#[test]
fn default_context_embeds_host_platform_rules_block() {
    // #1605 T-1: a Windows host session carries the Git Bash routing rule in
    // the platform block plus the messaging pointer.
    let out = default_context(
        "C:/fake/wg-7-dev-team/__agent_architect",
        None,
        &no_skill_section(),
    );
    assert!(out.contains(concat!(
        r#"## Host Platform Rules

Windows host session: invoke `C:\Program Files\Git\bin\bash.exe` directly for all shell work and "#,
        r#"every AgentsCommander CLI invocation; avoid PowerShell whenever Git Bash is "#,
        r#"available. Only if PowerShell is unavoidable, wrap with `& 'C:\Program "#,
        r#"Files\Git\bin\bash.exe' -lc '...'` and capture CLI output with `2>&1 | "#,
        r#"Out-String`."#
    )));
    assert!(out.contains("bash.exe"));
    assert!(out.contains("2>&1 | Out-String"));
    assert!(out.contains("**Windows:** see **Host Platform Rules** above."));
}

#[cfg(not(target_os = "windows"))]
#[test]
fn default_context_non_windows_embeds_minimal_platform_block() {
    // #1605 T-2: Linux/macOS host sessions render only the minimal block.
    let out = default_context(
        "C:/fake/wg-7-dev-team/__agent_architect",
        None,
        &no_skill_section(),
    );
    assert!(out.contains("## Host Platform Rules"));
    assert!(!out.contains("bash.exe"));
    assert!(!out.contains("2>&1 | Out-String"));
}

#[test]
fn container_session_omits_host_platform_rules_block() {
    // #1605 T-3: a container (transport-api) session renders no host platform
    // rules block (the Windows messaging pointer may still name the block,
    // so assert on the heading) and no raw placeholder.
    let out = render_agent_context_template_inner(
        get_default_agent_template(),
        "C:/fake/wg-7-dev-team/__agent_architect",
        None,
        &no_skill_section(),
        Path::new("C:/fake/wg-7-dev-team/__agent_architect"),
        None,
        Some(&crate::pty::container_repos::RepoMountResolution::default()),
        false,
    );
    assert!(!out.contains("## Host Platform Rules"));
    assert!(!out.contains("bash.exe"));
    assert_no_raw_template_placeholders(&out);
}

#[cfg(target_os = "windows")]
#[test]
fn root_prologue_embeds_host_platform_rules_block() {
    // #1605 T-4: the Root runtime prologue renders the platform block as its
    // 10th block between CLI context and session credentials.
    let out = render_root_runtime_prologue_inner(
        "C:/fake/ac-root-agent",
        &no_skill_section(),
        Path::new("C:/fake/ac-root-agent"),
        None,
        None,
        true,
    );
    assert!(out.contains("## Host Platform Rules"));
    assert!(out.contains("bash.exe"));
    assert!(out.contains("2>&1 | Out-String"));
    let cli_pos = out.find("## CLI executable").expect("CLI block");
    let platform_pos = out.find("## Host Platform Rules").expect("platform block");
    let credentials_pos = out
        .find("## Session credentials")
        .expect("credentials block");
    assert!(
        cli_pos < platform_pos && platform_pos < credentials_pos,
        "the platform block must sit between CLI context and session credentials"
    );
}

#[test]
fn container_root_prologue_omits_host_platform_rules_block() {
    // #1605 T-5: a container root renders no host platform rules block.
    let out = render_root_runtime_prologue_inner(
        "C:/fake/ac-root-agent",
        &no_skill_section(),
        Path::new("C:/fake/ac-root-agent"),
        None,
        Some(&crate::pty::container_repos::RepoMountResolution::default()),
        true,
    );
    assert!(!out.contains("## Host Platform Rules"));
    assert!(!out.contains("bash.exe"));
    assert_no_raw_template_placeholders(&out);
}

#[test]
fn personalized_template_without_host_platform_rules_token_appends_fallback() {
    // #1605 T-6: a preserved personalized template lacking the token (and a
    // root-agent-shaped fixture, per the owner's `Context.root-agent.md`
    // probe) receives exactly one `## Host Platform Rules` section via the
    // append fallback.
    let no_token = get_default_agent_template().replace("{{HOST_PLATFORM_RULES}}", "");
    let root_shaped = "# Root Agent\n\nCustom prose without the platform token.\n".to_string();
    for template in [no_token, root_shaped] {
        let out = render_agent_context_template_inner(
            &template,
            "C:/fake/wg-7-dev-team/__agent_architect",
            None,
            &no_skill_section(),
            Path::new("C:/fake/wg-7-dev-team/__agent_architect"),
            None,
            None,
            false,
        );
        assert_eq!(
            count_context_occurrences(&out, "## Host Platform Rules"),
            1,
            "the fallback must append exactly one platform block"
        );
        assert_no_raw_template_placeholders(&out);
    }

    // Token present (both-tokens pathological shape): exactly one block.
    let with_token = get_default_agent_template();
    let out = render_agent_context_template_inner(
        with_token,
        "C:/fake/wg-7-dev-team/__agent_architect",
        None,
        &no_skill_section(),
        Path::new("C:/fake/wg-7-dev-team/__agent_architect"),
        None,
        None,
        false,
    );
    assert_eq!(count_context_occurrences(&out, "## Host Platform Rules"), 1);
    assert_no_raw_template_placeholders(&out);
}

#[test]
fn host_platform_rules_reads_project_file_each_materialization() {
    // #1605 T-7: every materialization re-reads `.ac/Context.platform.<os>.md`
    // (no cache, no rebuild): edit the file, render again, get the new text.
    let temp = tempfile::tempdir().expect("tempdir");
    let ac = temp.path().join(".ac");
    let replica = ac.join("wg-1-team").join("__agent_dev");
    std::fs::create_dir_all(&replica).expect("create replica layout");
    let filename = host_platform_rules_filename();

    let first = format!("## Host Platform Rules\n\nFIRST CUSTOM TEXT {filename}\n");
    std::fs::write(ac.join(filename), &first).expect("write first custom platform file");
    let out1 = render_agent_context_template_inner(
        get_default_agent_template(),
        &path_string(&replica),
        None,
        &no_skill_section(),
        &replica,
        None,
        None,
        false,
    );
    assert!(out1.contains("FIRST CUSTOM TEXT"));

    let second = "## Host Platform Rules\n\nSECOND CUSTOM TEXT\n";
    std::fs::write(ac.join(filename), second).expect("rewrite platform file");
    let out2 = render_agent_context_template_inner(
        get_default_agent_template(),
        &path_string(&replica),
        None,
        &no_skill_section(),
        &replica,
        None,
        None,
        false,
    );
    assert!(out2.contains("SECOND CUSTOM TEXT"));
    assert!(!out2.contains("FIRST CUSTOM TEXT"));
    assert_eq!(
        count_context_occurrences(&out2, "## Host Platform Rules"),
        1
    );
    assert_no_raw_template_placeholders(&out2);
}

#[test]
fn render_seeds_missing_platform_file_absent_only() {
    // #1625 T-1 (replaces the missing case of #1605 T-8): the render path
    // seeds the missing platform files absent-only through the seeder
    // lifecycle before reading: the host file and all three platform files
    // exist byte-equal to their embedded defaults, the state records
    // current platform versions with `lastSeededSha256 == hash(default)`, the
    // rendered block carries the default, and a second render is
    // idempotent (same output, same files, same state).
    use sha2::{Digest, Sha256};

    let temp = tempfile::tempdir().expect("tempdir");
    let ac = temp.path().join(".ac");
    let replica = ac.join("wg-1-team").join("__agent_dev");
    std::fs::create_dir_all(&replica).expect("create replica layout");
    let filename = host_platform_rules_filename();
    let default = host_platform_rules_default();
    let state_path =
        ac.join(crate::config::seeded_context_templates::SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);

    let render = |replica: &std::path::Path| {
        render_agent_context_template_inner(
            get_default_agent_template(),
            &path_string(replica),
            None,
            &no_skill_section(),
            replica,
            None,
            None,
            false,
        )
    };

    let out1 = render(&replica);
    assert!(out1.contains("## Host Platform Rules"));
    assert!(out1.contains(default));
    assert_eq!(
        count_context_occurrences(&out1, "## Host Platform Rules"),
        1
    );
    assert_no_raw_template_placeholders(&out1);

    // (a) host file exists byte-equal to its embedded default.
    assert_eq!(
        std::fs::read_to_string(ac.join(filename)).expect("read seeded host file"),
        default,
        "the host platform file must be seeded byte-equal to the default"
    );
    // (b) all three platform files exist byte-equal to their defaults.
    for (file, expected) in [
        (
            crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS,
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS,
        ),
        (
            crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_LINUX,
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
        ),
        (
            crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_MACOS,
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
        ),
    ] {
        assert_eq!(
            std::fs::read_to_string(ac.join(file)).expect("read seeded platform file"),
            expected,
            "{file} must be seeded byte-equal to its embedded default"
        );
    }
    // (c) state records current platform versions with lastSeededSha256 == hash(default).
    let state = std::fs::read_to_string(&state_path).expect("read seeded state");
    let parsed: serde_json::Value = serde_json::from_str(&state).expect("parse seeded state");
    for (id, expected) in [
        (
            "platform.windows",
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_WINDOWS,
        ),
        (
            "platform.linux",
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_LINUX,
        ),
        (
            "platform.macos",
            crate::config::session_context::DEFAULT_HOST_PLATFORM_RULES_MACOS,
        ),
    ] {
        assert_eq!(
            parsed["templates"][id]["currentVersion"],
            if id == "platform.windows" { 2 } else { 1 },
            "{id} state entry must carry the current version"
        );
        assert_eq!(
            parsed["templates"][id]["lastSeededSha256"],
            format!("{:x}", Sha256::digest(expected.as_bytes())),
            "{id} state entry must carry the default sha"
        );
    }

    // (e) second render idempotent: same output, files unchanged, state unchanged.
    let host_before = std::fs::read_to_string(ac.join(filename)).expect("read host file");
    let out2 = render(&replica);
    assert_eq!(out1, out2, "the second render must be byte-identical");
    assert_eq!(
        std::fs::read_to_string(ac.join(filename)).expect("read host file again"),
        host_before,
        "the seeded host file must not change on a second render"
    );
    assert_eq!(
        std::fs::read_to_string(&state_path).expect("read state again"),
        state,
        "the state must not change on a second render"
    );
}

#[test]
fn host_platform_rules_empty_file_falls_back_to_embedded_default() {
    // #1625 T-2 (replaces the empty case of #1605 T-8): an empty platform
    // file renders the embedded default (single block) and is preserved
    // as-is: it is never seeded over, and no state entry is created for it
    // (silent preservation of the unowned empty file via
    // `suppress_unknown_without_state`).
    let temp = tempfile::tempdir().expect("tempdir");
    let ac = temp.path().join(".ac");
    let replica = ac.join("wg-1-team").join("__agent_dev");
    std::fs::create_dir_all(&replica).expect("create replica layout");
    let filename = host_platform_rules_filename();
    let default = host_platform_rules_default();

    std::fs::write(ac.join(filename), "").expect("write empty platform file");
    let empty = render_agent_context_template_inner(
        get_default_agent_template(),
        &path_string(&replica),
        None,
        &no_skill_section(),
        &replica,
        None,
        None,
        false,
    );
    assert!(empty.contains("## Host Platform Rules"));
    assert!(empty.contains(default));
    assert_eq!(
        count_context_occurrences(&empty, "## Host Platform Rules"),
        1
    );
    assert_eq!(
        std::fs::read_to_string(ac.join(filename)).expect("read empty platform file"),
        "",
        "an empty pre-existing file must never be overwritten"
    );
    assert!(
        !ac.join(crate::config::seeded_context_templates::SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)
            .exists(),
        "an unowned empty file must not create a state entry"
    );
}

#[test]
fn render_with_container_mounts_never_seeds_platform_files() {
    // #1625 T-4: a container session renders no host platform rules block
    // and never seeds the platform files (the early return precedes the
    // seed guard); no state file is created either.
    let temp = tempfile::tempdir().expect("tempdir");
    let ac = temp.path().join(".ac");
    let replica = ac.join("wg-1-team").join("__agent_dev");
    std::fs::create_dir_all(&replica).expect("create replica layout");

    let out = render_agent_context_template_inner(
        get_default_agent_template(),
        &path_string(&replica),
        None,
        &no_skill_section(),
        &replica,
        None,
        Some(&crate::pty::container_repos::RepoMountResolution::default()),
        false,
    );
    assert!(!out.contains("## Host Platform Rules"));
    for file in [
        crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_WINDOWS,
        crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_LINUX,
        crate::config::session_context::HOST_PLATFORM_RULES_FILENAME_MACOS,
    ] {
        assert!(
            !ac.join(file).exists(),
            "{file} must never be seeded for a container session"
        );
    }
    assert!(
        !ac.join(crate::config::seeded_context_templates::SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME)
            .exists(),
        "no state file may be created for a container session"
    );
}

#[test]
fn deleted_platform_file_is_reseeded_absent_only() {
    // #1625 T-5: after a render-triggered seed, deleting the host platform
    // file re-seeds it absent-only on the next render (file byte-equal to
    // the default again) and the state keeps the current platform versions.
    let temp = tempfile::tempdir().expect("tempdir");
    let ac = temp.path().join(".ac");
    let replica = ac.join("wg-1-team").join("__agent_dev");
    std::fs::create_dir_all(&replica).expect("create replica layout");
    let filename = host_platform_rules_filename();
    let default = host_platform_rules_default();
    let state_path =
        ac.join(crate::config::seeded_context_templates::SEEDED_CONTEXT_TEMPLATE_STATE_FILENAME);

    let render = |replica: &std::path::Path| {
        render_agent_context_template_inner(
            get_default_agent_template(),
            &path_string(replica),
            None,
            &no_skill_section(),
            replica,
            None,
            None,
            false,
        )
    };

    let out1 = render(&replica);
    assert!(out1.contains(default));
    assert_eq!(
        std::fs::read_to_string(ac.join(filename)).expect("read seeded host file"),
        default
    );
    let state_before = std::fs::read_to_string(&state_path).expect("read state before deletion");

    std::fs::remove_file(ac.join(filename)).expect("delete host platform file");
    let out2 = render(&replica);
    assert!(out2.contains("## Host Platform Rules"));
    assert!(out2.contains(default));
    assert_eq!(
        std::fs::read_to_string(ac.join(filename)).expect("read reseeded host file"),
        default,
        "the deleted file must be re-seeded byte-equal to the default"
    );
    assert_eq!(
        std::fs::read_to_string(&state_path).expect("read state after reseed"),
        state_before,
        "the reseed must keep the existing state entry untouched"
    );
    let parsed: serde_json::Value = serde_json::from_str(&state_before).expect("parse state");
    assert_eq!(
        parsed["templates"]["platform.windows"]["currentVersion"], 2,
        "the Windows platform state entry must stay v2 seeded"
    );
}

#[test]
fn render_never_overwrites_edited_platform_file() {
    // #1625 T-6: an existing custom platform file renders as-is and is
    // never touched by the seed guard (no sync runs over existing files).
    let temp = tempfile::tempdir().expect("tempdir");
    let ac = temp.path().join(".ac");
    let replica = ac.join("wg-1-team").join("__agent_dev");
    std::fs::create_dir_all(&replica).expect("create replica layout");
    let filename = host_platform_rules_filename();
    let custom = format!("## Host Platform Rules\n\nMY CUSTOM RULES {filename}\n");

    std::fs::write(ac.join(filename), &custom).expect("write custom platform file");
    let out = render_agent_context_template_inner(
        get_default_agent_template(),
        &path_string(&replica),
        None,
        &no_skill_section(),
        &replica,
        None,
        None,
        false,
    );
    assert!(out.contains("MY CUSTOM RULES"));
    assert_eq!(count_context_occurrences(&out, "## Host Platform Rules"), 1);
    assert_eq!(
        std::fs::read_to_string(ac.join(filename)).expect("read custom platform file"),
        custom,
        "an existing custom file must never be overwritten"
    );
}

#[test]
fn coordinator_session_profile_embeds_host_platform_rules_once() {
    // #1605 T-9: the block reaches coordinator sessions through the global
    // render exactly once; the coordinator body adds no duplicate.
    let replica = default_context(
        "C:/fake/wg-7-dev-team/__agent_architect",
        None,
        &no_skill_section(),
    );
    let coordinator = format!(
        "{}\n\n---\n\n# Orchestrator Context\n\n{}",
        replica,
        get_default_coordinator_template()
    );
    assert_eq!(
        count_context_occurrences(&coordinator, "## Host Platform Rules"),
        1
    );
    assert!(!get_default_coordinator_template().contains("Host Platform Rules"));
}
