use super::*;

// ── (#630/#631) restart_skip_auto_resume_with_intent: the call site uses this
//    exact composition to decide resume-vs-fresh on a restart. ──

#[test]
fn restart_intent_explicit_restart_button_is_fresh() {
    // "Restart Session" (None) on a session with no stored intent => fresh.
    assert!(super::super::restart_skip_auto_resume_with_intent(
        false, None
    ));
}

#[test]
fn restart_intent_normal_member_reopen_resumes() {
    // Branch A reopen of a NORMAL member (no stored intent, Some(false))
    // => resume, unchanged from today.
    assert!(!super::super::restart_skip_auto_resume_with_intent(
        false,
        Some(false)
    ));
}

#[test]
fn restart_intent_deferred_fresh_member_reopen_stays_fresh() {
    // #631 closure: a "Restart Session"-then-app-restart member defers, then
    // reopens via Branch A (Some(false)), but its persisted fresh intent wins.
    assert!(super::super::restart_skip_auto_resume_with_intent(
        true,
        Some(false)
    ));
}

#[test]
fn restart_intent_stored_fresh_with_restart_button_is_fresh() {
    // Stored fresh AND an explicit restart => still fresh (no regression).
    assert!(super::super::restart_skip_auto_resume_with_intent(
        true, None
    ));
}

// ── (#747) carry_communication_for_restart: shared decision for the
//    Branch-A reopen (restart_session_inner_with_activation) AND the
//    startup wake arm (lib.rs restore loop), which reuses this helper with
//    the persisted intent. This matrix pins the fresh gate for BOTH. ──

#[test]
fn carry_communication_for_restart_matrix() {
    let visible_hand = || {
        Some(crate::session::session::SessionCommunication {
            kind: crate::session::session::SessionCommunicationKind::RaiseHand,
            visible: true,
            updated_at: "2026-06-30T11:00:00+00:00".to_string(),
            message: None,
        })
    };
    let hidden_hand = || {
        Some(crate::session::session::SessionCommunication {
            kind: crate::session::session::SessionCommunicationKind::RaiseHand,
            visible: false,
            updated_at: "2026-06-30T11:00:00+00:00".to_string(),
            message: None,
        })
    };

    // (visible raise, fresh=false): the reopen resumes the conversation,
    // so the pending user-attention marker carries.
    let carried = super::super::carry_communication_for_restart(visible_hand(), false)
        .expect("visible hand must carry on a resuming reopen");
    assert!(carried.visible);
    assert_eq!(carried.updated_at, "2026-06-30T11:00:00+00:00");

    // (visible raise, fresh=true): a fresh restart abandons the old
    // conversation; the raise belonged to it and drops.
    assert!(super::super::carry_communication_for_restart(visible_hand(), true).is_none());

    // (hidden raise, fresh=false): non-visible payloads never carry.
    assert!(super::super::carry_communication_for_restart(hidden_hand(), false).is_none());

    // (None, fresh=false): nothing to carry.
    assert!(super::super::carry_communication_for_restart(None, false).is_none());
}

// ── #599 R1 effective_create_skip_auto_resume tests ──

#[test]
fn effective_create_skip_auto_resume_defaults_to_true_for_none() {
    // No explicit value: legacy fresh-create default used by create-in-place,
    // new-agent, open-agent, CLI, and web call sites (no --continue).
    assert!(super::super::effective_create_skip_auto_resume(None));
}

#[test]
fn effective_create_skip_auto_resume_respects_explicit_false() {
    // #599 R1: reopening a coordinator destroyed by auto-close (#552/#580)
    // or manual close (#588) opts in to resume; --continue is then injected
    // (still subject to the claude_project_exists disk gate downstream).
    assert!(!super::super::effective_create_skip_auto_resume(Some(
        false
    )));
}

#[test]
fn effective_create_skip_auto_resume_respects_explicit_true() {
    // Explicit true still skips resume (parity with the restart helper).
    assert!(super::super::effective_create_skip_auto_resume(Some(true)));
}

// ── #599 R2 claude_projects_dir_for_config_dir tests ──

#[test]
fn claude_projects_dir_for_config_dir_joins_projects_and_mangled() {
    // An explicit CLAUDE_CONFIG_DIR (already AC-expanded) maps to
    // <base>/projects/<mangled-cwd>, mirroring the wrapper resolver output.
    let base = if cfg!(windows) {
        "C:\\Users\\maria\\.claude-env"
    } else {
        "/home/maria/.claude-env"
    };
    let cwd = if cfg!(windows) { "C:\\x" } else { "/home/x" };
    let resolved = super::super::claude_projects_dir_for_config_dir(base, cwd);
    let expected = std::path::PathBuf::from(base)
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, expected);
}

#[test]
fn claude_projects_dir_for_config_dir_expands_percent_envvar_in_base() {
    // Directly covers the lifted expand_env_var_refs: a %VAR% ref in the
    // config dir resolves against the process env before the projects join.
    let var = "AC_TEST_599_CONFIG_DIR";
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-env");
    // SAFETY: env state is process-global; unique name avoids cross-test races.
    std::env::set_var(var, custom_base.to_str().unwrap());
    let resolved = super::super::claude_projects_dir_for_config_dir(&format!("%{}%", var), "C:\\x");
    std::env::remove_var(var);
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, expected);
}

#[test]
fn local_resume_probe_env_layer_uses_config_dir_override() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-env");
    let cwd = "C:\\Users\\Test\\repo";

    let resolved =
        super::super::claude_projects_dir_for_config_dir(custom_base.to_str().unwrap(), cwd);

    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, expected);
}

#[test]
fn local_resume_probe_env_layer_expands_powershell_envvar() {
    let var = "AC_TEST_894_CONFIG_DIR";
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-env");
    std::env::set_var(var, custom_base.to_str().unwrap());

    let resolved = super::super::claude_projects_dir_for_config_dir(
        &format!("$env:{}\\.claude", var),
        "C:\\x",
    );

    std::env::remove_var(var);
    let expected = PathBuf::from(format!("{}\\.claude", custom_base.to_string_lossy()))
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, expected);
}

// ── should_inject_continue tests (issue #82, plan §8.1) ──

#[test]
fn should_inject_continue_returns_false_when_not_claude() {
    assert!(!should_inject_continue(false, false, true, "codex"));
}

#[test]
fn should_inject_continue_returns_false_when_skip_overrides_existing_dir() {
    // G4 strengthening: lock the predicate against future refactors that
    // re-order early-return clauses. Explicit fixture, not "all permissive".
    assert!(!should_inject_continue(true, true, true, "claude"));
}

#[test]
fn should_inject_continue_returns_false_when_dir_missing() {
    assert!(!should_inject_continue(true, false, false, "claude"));
}

#[test]
fn should_inject_continue_returns_false_when_continue_already_present() {
    assert!(!should_inject_continue(
        true,
        false,
        true,
        "claude --continue"
    ));
}

#[test]
fn should_inject_continue_returns_true_for_canonical_resume_case() {
    assert!(should_inject_continue(true, false, true, "claude"));
}

#[test]
fn should_inject_continue_returns_false_when_continue_with_value_present() {
    // R2.4 / G2: the GNU long-option-with-value form must also suppress
    // re-injection.
    assert!(!should_inject_continue(
        true,
        false,
        true,
        "claude --continue=somevalue"
    ));
}

#[test]
fn should_inject_continue_returns_false_when_uppercase_continue_present() {
    // D4 #6: case-insensitivity regression fence.
    assert!(!should_inject_continue(
        true,
        false,
        true,
        "claude --CONTINUE"
    ));
}

#[test]
fn should_inject_continue_returns_false_when_short_form_present() {
    // D4 #7: -c short-form regression fence.
    assert!(!should_inject_continue(true, false, true, "claude -c"));
}

#[test]
fn should_inject_continue_returns_false_when_continue_in_cmd_wrapper() {
    // D4 #8: token-level scan, not arg-index scan.
    assert!(!should_inject_continue(
        true,
        false,
        true,
        "cmd /C claude --continue"
    ));
}

#[test]
fn should_inject_continue_returns_true_when_unrelated_continue_substring() {
    // D4 #9: token-equality fence — `--continued-mode` is NOT `--continue`.
    // Guards against a future regression to substring matching.
    assert!(should_inject_continue(
        true,
        false,
        true,
        "claude --continued-mode something"
    ));
}

// ── (#756) mirror_forces_fresh truth table ──

#[test]
fn mirror_forces_fresh_truth_table() {
    // Forces fresh ONLY when: resume requested + coordinator + mirror pending.
    assert!(super::super::mirror_forces_fresh(false, true, true));
    // Any other combination must not force.
    assert!(!super::super::mirror_forces_fresh(false, true, false));
    assert!(!super::super::mirror_forces_fresh(false, false, true));
    assert!(!super::super::mirror_forces_fresh(false, false, false));
    assert!(!super::super::mirror_forces_fresh(true, true, true));
    assert!(!super::super::mirror_forces_fresh(true, true, false));
    assert!(!super::super::mirror_forces_fresh(true, false, true));
    assert!(!super::super::mirror_forces_fresh(true, false, false));
}

// ── (#756) should_inject_fresh_session_id decision table ──

#[test]
fn should_inject_fresh_session_id_true_for_claude_skip_clean_argv() {
    assert!(super::super::should_inject_fresh_session_id(
        true,
        true,
        "claude --dangerously-skip-permissions"
    ));
}

#[test]
fn should_inject_fresh_session_id_false_when_not_skipping_resume() {
    assert!(!super::super::should_inject_fresh_session_id(
        true,
        false,
        "claude --dangerously-skip-permissions"
    ));
}

#[test]
fn should_inject_fresh_session_id_false_when_not_claude() {
    assert!(!super::super::should_inject_fresh_session_id(
        false,
        true,
        "codex --dangerously-bypass-approvals-and-sandbox"
    ));
}

#[test]
fn should_inject_fresh_session_id_false_for_each_identity_veto_token() {
    // User-configured identity flags win; the rider must never stack onto
    // them (stacking is a hard CLI error, Q2-verified).
    for cmd in [
        "claude --session-id 7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f",
        "claude --session-id=7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f",
        "claude --resume",
        "claude --resume=abc",
        "claude -r",
        "claude --continue",
        "claude --continue=abc",
        "claude -c",
        "claude --fork-session",
        "claude --SESSION-ID x", // case-insensitive token match
        "claude --Continue",
    ] {
        assert!(
            !super::super::should_inject_fresh_session_id(true, true, cmd),
            "identity flag in {:?} must veto the rider",
            cmd
        );
    }
}

#[test]
fn should_inject_fresh_session_id_true_when_unrelated_flag_substring() {
    // Token-equality fence: `--session-identity` is NOT `--session-id`,
    // `--recover` is NOT `-r`.
    assert!(super::super::should_inject_fresh_session_id(
        true,
        true,
        "claude --session-identity thing --recover"
    ));
}

// ── (#2454) transcript_id_from_args ──

const TRANSCRIPT_UUID: &str = "7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f";

fn transcript_id_of(args: &[&str]) -> Option<String> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    super::super::transcript_id_from_args(&args)
}

// Test 1.
#[test]
fn transcript_id_reads_the_spaced_session_id() {
    assert_eq!(
        transcript_id_of(&["--session-id", TRANSCRIPT_UUID]).as_deref(),
        Some(TRANSCRIPT_UUID)
    );
}

// Test 2.
#[test]
fn transcript_id_reads_the_equals_session_id() {
    let arg = format!("--session-id={TRANSCRIPT_UUID}");
    assert_eq!(transcript_id_of(&[&arg]).as_deref(), Some(TRANSCRIPT_UUID));
}

// Test 3: the `cmd` spawn shape keeps the flag inside the last element; an
// argv scan that does not split on whitespace misses it.
#[test]
fn transcript_id_reads_the_cmd_shape_inside_one_argument() {
    let arg = format!("C:/Tools/claude --session-id {TRANSCRIPT_UUID}");
    assert_eq!(
        transcript_id_of(&["/C", &arg]).as_deref(),
        Some(TRANSCRIPT_UUID)
    );
}

// Test 4.
#[test]
fn transcript_id_reads_resume_in_both_forms() {
    let equals = format!("--resume={TRANSCRIPT_UUID}");
    assert_eq!(
        transcript_id_of(&["--resume", TRANSCRIPT_UUID]).as_deref(),
        Some(TRANSCRIPT_UUID)
    );
    assert_eq!(
        transcript_id_of(&[&equals]).as_deref(),
        Some(TRANSCRIPT_UUID)
    );
}

// Test 5 (guard): `-r` carries a real UUID on purpose, so treating `-r` as
// `--resume` would return it and turn this red.
#[test]
fn transcript_id_is_none_for_flags_that_name_no_file() {
    assert_eq!(transcript_id_of(&["--continue"]), None);
    assert_eq!(transcript_id_of(&["-c"]), None);
    assert_eq!(transcript_id_of(&["-r", TRANSCRIPT_UUID]), None);
    assert_eq!(transcript_id_of(&[]), None);
}

// Test 6 (guard).
#[test]
fn transcript_id_is_none_for_a_non_uuid_value() {
    assert_eq!(transcript_id_of(&["--session-id", "not-a-uuid"]), None);
    assert_eq!(transcript_id_of(&["--resume=abc"]), None);
}

// Test 7 (guard).
#[test]
fn transcript_id_matches_the_flag_case_insensitively() {
    assert_eq!(
        transcript_id_of(&["--SESSION-ID", TRANSCRIPT_UUID]).as_deref(),
        Some(TRANSCRIPT_UUID)
    );
}

// Test 8 (guard): a trailing flag with no value yields `None`, no panic.
#[test]
fn transcript_id_is_none_for_a_trailing_flag_without_value() {
    assert_eq!(transcript_id_of(&["claude", "--session-id"]), None);
}

// ── Issue #107 Round 5 §R5.8.6 — build_title_prompt_appendage idempotence ──
//
// Tempdir naming starts with `wg-` so `find_workgroup_task_path_for_cwd`'s
// ancestor walk finds the cwd itself as the wg ancestor. The three tests
// pin gates (3), (4), and the happy path. Path-walk gate (1) failure is
// exercised by the existing `find_workgroup_task_path_for_cwd` tests in
// `session/session.rs`. Read-failure gate (2) requires fault-injecting
// `std::fs::read_to_string`, which is not worth the harness for a thin
// orchestrator.

// ── Issue #186 — resolve_claude_projects_dir ──

pub(super) fn write_wrapper(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn resolve_claude_projects_dir_uses_home_for_bare_claude() {
    // Default install → fall back to <home>/.claude/projects/<mangled>.
    let cwd = "C:\\Users\\Test\\repo";
    let resolved = super::super::resolve_claude_projects_dir("claude", &[], cwd);
    // Skip if the test host has no home dir (CI sandboxes sometimes don't).
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let expected = home
        .join(".claude")
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_uses_home_for_direct_claude_exe_path() {
    // Direct executable path with file_stem == "claude" → still default base.
    let cwd = "C:\\Users\\Test\\repo";
    let resolved = super::super::resolve_claude_projects_dir("C:\\Tools\\claude.exe", &[], cwd);
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let expected = home
        .join(".claude")
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_returns_none_when_no_claude_token() {
    let resolved = super::super::resolve_claude_projects_dir(
        "powershell.exe",
        &["-NoLogo".to_string()],
        "C:\\x",
    );
    assert!(resolved.is_none());
}

#[test]
fn resolve_claude_projects_dir_parses_wrapper_with_set_directive() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let cwd = "C:\\Users\\Test\\repo";
    let resolved = super::super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], cwd);
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_strips_quotes_around_value() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join("Path With Spaces").join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR=\"{}\"\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let resolved =
        super::super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_falls_back_when_wrapper_lacks_directive() {
    let tmp = tempfile::tempdir().unwrap();
    let wrapper = write_wrapper(tmp.path(), "claude-mb.cmd", "@echo off\r\nclaude %*\r\n");
    let resolved =
        super::super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let expected = home
        .join(".claude")
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_falls_back_when_wrapper_missing() {
    let resolved = super::super::resolve_claude_projects_dir(
        "C:\\definitely\\not\\there\\claude-mb.cmd",
        &[],
        "C:\\x",
    );
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let expected = home
        .join(".claude")
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_finds_claude_token_in_cmd_wrapper_args() {
    // shell=cmd.exe, args=["/K", "<abs path to claude-mb.cmd>", "--effort", "max"]
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let resolved = super::super::resolve_claude_projects_dir(
        "cmd.exe",
        &[
            "/K".to_string(),
            wrapper.to_str().unwrap().to_string(),
            "--effort".to_string(),
            "max".to_string(),
        ],
        "C:\\x",
    );
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_finds_claude_token_in_embedded_cmd_string() {
    // Embedded form — per-arg whitespace split must surface claude-mb.cmd.
    // Skip on hosts where the temp dir contains spaces (split would
    // fragment the wrapper path); the cmd-wrapped-args test above
    // already covers spaced paths via direct token form.
    let tmp = tempfile::tempdir().unwrap();
    if tmp.path().to_string_lossy().contains(' ') {
        return;
    }
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let combined = format!("git pull && {} --effort max", wrapper.display());
    let resolved = super::super::resolve_claude_projects_dir(
        "cmd.exe",
        &["/K".to_string(), combined],
        "C:\\x",
    );
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_ignores_oversized_wrapper() {
    // Large file (> 64 KiB cap) → treated as not-a-wrapper, fall back.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("claude-mb.cmd");
    let mut body = String::with_capacity(80 * 1024);
    body.push_str("set CLAUDE_CONFIG_DIR=C:\\should-not-be-read\r\n");
    body.push_str(&"x".repeat(80 * 1024));
    std::fs::write(&path, body).unwrap();
    let resolved = super::super::resolve_claude_projects_dir(path.to_str().unwrap(), &[], "C:\\x");
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let expected = home
        .join(".claude")
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_parses_cmd_quoted_whole_assignment() {
    // `set "CLAUDE_CONFIG_DIR=<path with spaces>"` — canonical cmd idiom
    // when the value contains spaces or shell metacharacters.
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join("Path With Spaces").join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset \"CLAUDE_CONFIG_DIR={}\"\r\nclaude %*\r\n",
            custom_base.display()
        ),
    );
    let resolved =
        super::super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_expands_percent_envvar_value() {
    let var = "AC_TEST_186_BASE_PCT";
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    // SAFETY: env state is process-global; unique name avoids cross-test races.
    std::env::set_var(var, custom_base.to_str().unwrap());
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        &format!(
            "@echo off\r\nset CLAUDE_CONFIG_DIR=%{}%\r\nclaude %*\r\n",
            var
        ),
    );
    let resolved =
        super::super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    std::env::remove_var(var);
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_expands_powershell_envvar_value() {
    let var = "AC_TEST_186_BASE_PS";
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    std::env::set_var(var, custom_base.to_str().unwrap());
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.ps1",
        &format!(
            "$env:CLAUDE_CONFIG_DIR = \"$env:{}\"\r\nclaude @args\r\n",
            var
        ),
    );
    let resolved =
        super::super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    std::env::remove_var(var);
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
    assert_eq!(resolved, Some(expected));
}

#[test]
fn resolve_claude_projects_dir_preserves_unknown_envvar_literal() {
    // Unknown var → literal preserved → resulting path is_dir() will be
    // false at the call site, but parse must succeed (return Some).
    let tmp = tempfile::tempdir().unwrap();
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.cmd",
        "@echo off\r\nset \
             CLAUDE_CONFIG_DIR=%AC_TEST_186_DEFINITELY_UNSET%\\\\.claude-mb\r\nclaude %*\r\n",
    );
    let resolved =
        super::super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], "C:\\x");
    let resolved = resolved.expect("parser must return Some even when var is unset");
    let s = resolved.to_string_lossy();
    assert!(
        s.contains("%AC_TEST_186_DEFINITELY_UNSET%"),
        "expected literal preservation, got {s}"
    );
}

#[test]
fn resolve_claude_projects_dir_parses_wrapper_with_export_directive() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        tmp.path(),
        "claude-mb.sh",
        &format!(
            "#!/usr/bin/env bash\nexport CLAUDE_CONFIG_DIR={}\nexec claude \"$@\"\n",
            custom_base.display()
        ),
    );
    let resolved = super::super::resolve_claude_projects_dir(
        wrapper.to_str().unwrap(),
        &[],
        "/home/test/repo",
    );
    let expected =
        custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(
                "/home/test/repo",
            ));
    assert_eq!(resolved, Some(expected));
}

#[cfg(unix)]
fn agent_path_2589_wrapper_fixture(name: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("user-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let custom_base = tmp.path().join(".claude-mb");
    let wrapper = write_wrapper(
        &bin,
        name,
        &format!(
            "#!/usr/bin/env bash
export CLAUDE_CONFIG_DIR={}
exec claude \"$@\"
",
            custom_base.display()
        ),
    );
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    (tmp, bin, custom_base)
}

#[cfg(unix)]
fn agent_path_2589_search_path(extra: Option<&std::path::Path>) -> std::ffi::OsString {
    crate::config::agent_path::compose(&crate::config::agent_path::SearchPathInputs {
        inherited: Some(std::ffi::OsString::from("/bin:/usr/bin")),
        home: None,
        login_shell_path: extra.map(|dir| dir.as_os_str().to_os_string()),
    })
}

#[cfg(unix)]
fn agent_path_2589_default_base(cwd: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|home| {
        home.join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(cwd))
    })
}

#[cfg(unix)]
#[test]
fn agent_path_2589_claude_wrapper_in_a_user_bin_dir_is_parsed() {
    let (_tmp, bin, custom_base) = agent_path_2589_wrapper_fixture("claude-mb.sh");
    let cwd = "/home/test/repo";
    let resolved = super::super::resume_probe::resolve_claude_projects_dir_in(
        "claude-mb.sh",
        &[],
        cwd,
        &agent_path_2589_search_path(Some(&bin)),
    );
    let expected = custom_base
        .join("projects")
        .join(crate::session::session::mangle_cwd_for_claude(cwd));
    assert_eq!(resolved, Some(expected));
}

#[cfg(unix)]
#[test]
fn agent_path_2589_claude_wrapper_without_the_added_dir_falls_back() {
    let (_tmp, _bin, custom_base) = agent_path_2589_wrapper_fixture("claude-mb.sh");
    let cwd = "/home/test/repo";
    let resolved = super::super::resume_probe::resolve_claude_projects_dir_in(
        "claude-mb.sh",
        &[],
        cwd,
        &agent_path_2589_search_path(None),
    );
    assert_eq!(resolved, agent_path_2589_default_base(cwd));
    assert_ne!(
        resolved,
        Some(
            custom_base
                .join("projects")
                .join(crate::session::session::mangle_cwd_for_claude(cwd))
        )
    );
}

#[cfg(unix)]
#[test]
fn agent_path_2589_claude_wrapper_without_extension_uses_the_default_base() {
    let (_tmp, bin, _custom_base) = agent_path_2589_wrapper_fixture("claude-mb");
    let search_path = agent_path_2589_search_path(Some(&bin));
    assert!(
        crate::config::agent_command::resolve_program_in("claude-mb", &search_path).is_some(),
        "the extensionless wrapper now resolves through the added dir"
    );
    let cwd = "/home/test/repo";
    let resolved = super::super::resume_probe::resolve_claude_projects_dir_in(
        "claude-mb",
        &[],
        cwd,
        &search_path,
    );
    assert_eq!(resolved, agent_path_2589_default_base(cwd));
}

#[test]
fn p19_wrapper_bom() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-bom");
    let wrapper = tmp.path().join("claude-bom.cmd");
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend_from_slice(
        format!("set CLAUDE_CONFIG_DIR={}\r\n", custom_base.display()).as_bytes(),
    );
    std::fs::write(&wrapper, bytes).unwrap();
    let cwd = r"C:\Users\Test\repo";
    let resolved = super::super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], cwd);
    assert_eq!(
        resolved,
        Some(
            custom_base
                .join("projects")
                .join(crate::session::session::mangle_cwd_for_claude(cwd))
        )
    );
}

#[test]
fn p19_wrapper_non_utf8() {
    let tmp = tempfile::tempdir().unwrap();
    let custom_base = tmp.path().join(".claude-non-utf8");
    let wrapper = tmp.path().join("claude-non-utf8.cmd");
    let mut bytes = vec![0xff, 0x0a];
    bytes.extend_from_slice(
        format!("set CLAUDE_CONFIG_DIR={}\r\n", custom_base.display()).as_bytes(),
    );
    std::fs::write(&wrapper, bytes).unwrap();
    let cwd = r"C:\Users\Test\repo";
    let resolved = super::super::resolve_claude_projects_dir(wrapper.to_str().unwrap(), &[], cwd);
    assert_eq!(
        resolved,
        Some(
            custom_base
                .join("projects")
                .join(crate::session::session::mangle_cwd_for_claude(cwd))
        )
    );
}
