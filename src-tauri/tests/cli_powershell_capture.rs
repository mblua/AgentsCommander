#![cfg(target_os = "windows")]
//! Integration test for issue #129. Reproduces the PS-NonInteractive `&` direct-
//! call failure mode and asserts that the fix (conditional AttachConsole) lets
//! stdout flow through the inherited pipe.
//!
//! IMPORTANT: marked #[ignore] because the bug only reproduces in release mode
//! (`windows_subsystem = "windows"` is gated on `not(debug_assertions)`). To run:
//!     cargo test --release --test cli_powershell_capture -- --ignored
//!
//! CI coverage for shipped/testable release executable names lives in:
//!     npm run build:prod
//!     npm run smoke:cli-release-windows

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_agentscommander-new");

struct TempConfigRoot {
    path: PathBuf,
}

impl TempConfigRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ac-powershell-config-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).expect("create owned config root");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempConfigRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn ps_command(shell: &str, args: &str, guard: &TempConfigRoot) -> Option<Command> {
    if Command::new(shell)
        .arg("-Help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return None;
    }
    let mut c = Command::new(shell);
    c.env("AGENTSCOMMANDER_CONFIG_DIR", guard.path());
    c.args([
        "-NonInteractive",
        "-NoProfile",
        "-Command",
        &format!("& '{}' {}", BIN.replace('\'', "''"), args),
    ]);
    c.stdout(Stdio::piped());
    c.stderr(Stdio::piped());
    Some(c)
}

fn run_ps(shell: &str, args: &str, guard: &TempConfigRoot) -> Option<(i32, String, String)> {
    let mut cmd = ps_command(shell, args, guard)?;
    let out = cmd.spawn().ok()?.wait_with_output().ok()?;
    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    Some((code, stdout, stderr))
}

fn ps_script_command(shell: &str, script: &str, guard: &TempConfigRoot) -> Command {
    let mut c = Command::new(shell);
    c.env("AGENTSCOMMANDER_CONFIG_DIR", guard.path());
    c.args(["-NonInteractive", "-NoProfile", "-Command", script]);
    c.stdout(Stdio::piped());
    c.stderr(Stdio::piped());
    c
}

/// `run_ps` variant for a caller-supplied PS script (no `& '<BIN>'` prefix)
/// that ends with `; exit $LASTEXITCODE` so the outer powershell.exe exit code
/// reflects the inner process's. Only meaningful for console-subsystem carriers
/// (bash.exe); the GUI-subsystem AC binary under bare `&` leaves
/// `$LASTEXITCODE` unset, which is why the four direct-capture tests above
/// intentionally never assert exit codes.
fn run_ps_script_with_exit_propagation(
    shell: &str,
    script: &str,
    guard: &TempConfigRoot,
) -> Option<(i32, String, String)> {
    if Command::new(shell)
        .arg("-Help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return None;
    }
    let mut c = ps_script_command(shell, script, guard);
    let out = c.spawn().ok()?.wait_with_output().ok()?;
    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    Some((code, stdout, stderr))
}

#[allow(clippy::assertions_on_constants)] // Plan R2.6 step 3: intentional runtime guard.
fn debug_guard() {
    assert!(
        !cfg!(debug_assertions),
        "This test only validates issue #129 in release mode. \
         Run with `cargo test --release --test cli_powershell_capture -- --ignored`."
    );
}

// Exit-code assertions are intentionally absent on all four tests (R5.4 — applies
// R4.1's NEW-3 logic symmetrically to R2.6). PS-NonInteractive bare `&` does not
// propagate the AC binary's $LASTEXITCODE for GUI-subsystem children (PE
// Subsystem=2); the outer powershell.exe always exits 0 regardless. The bug-
// relevant signals are stdout/stderr presence — those alone distinguish fixed
// from unfixed binaries. The exit-code carrier is the bash-routed test below
// (`..._via_git_bash`): bash.exe is console-subsystem, so $LASTEXITCODE
// propagates through it and out of the outer powershell.exe.

#[test]
#[ignore = "Manual developer check; CI uses smoke:cli-release-windows against shipped/testable exe names"]
fn list_peers_outputs_valid_json_under_powershell_noninteractive() {
    debug_guard();
    let guard = TempConfigRoot::new();

    let tmp = std::env::temp_dir().join(format!(
        "ac-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let safe_root = tmp.to_string_lossy().replace('\'', "''");
    let token = "00000000-0000-0000-0000-000000000000";

    let (_code, stdout, stderr) = run_ps(
        "powershell.exe",
        &format!("list-peers --token {} --root '{}'", token, safe_root),
        &guard,
    )
    .expect("powershell.exe must be available on Windows CI/dev");

    assert!(
        !stdout.trim().is_empty(),
        "stdout should contain the JSON payload (post-fix). stderr=[{}]",
        stderr
    );
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should parse as JSON");
    assert!(parsed.is_array(), "expected JSON array");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
#[ignore = "Manual developer check; CI uses smoke:cli-release-windows against shipped/testable exe names"]
fn send_help_outputs_under_powershell_noninteractive() {
    debug_guard();
    let guard = TempConfigRoot::new();

    let (_code, stdout, stderr) =
        run_ps("powershell.exe", "send --help", &guard).expect("powershell.exe must be available");

    assert!(
        !stdout.trim().is_empty(),
        "stdout should contain help text. stderr=[{}]",
        stderr
    );
    assert!(
        stdout.contains("--to") || stdout.contains("DELIVERY MODES"),
        "help text missing expected content; got: {}",
        stdout
    );
}

#[test]
#[ignore = "Manual developer check; CI uses smoke:cli-release-windows against shipped/testable exe names"]
fn send_unknown_flag_emits_stderr_under_powershell_noninteractive() {
    debug_guard();
    let guard = TempConfigRoot::new();

    let (_code, _stdout, stderr) = run_ps("powershell.exe", "send --bogus-flag-xyz", &guard)
        .expect("powershell.exe must be available");
    assert!(
        !stderr.trim().is_empty(),
        "stderr must contain a usage error"
    );
}

// G8: parallel pwsh.exe tests. Skip if pwsh not installed.
#[test]
#[ignore = "Manual developer check; CI uses smoke:cli-release-windows against shipped/testable exe names when pwsh.exe is installed"]
fn list_peers_outputs_valid_json_under_pwsh_noninteractive() {
    debug_guard();
    let guard = TempConfigRoot::new();

    let tmp = std::env::temp_dir().join(format!(
        "ac-test-pwsh-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let safe_root = tmp.to_string_lossy().replace('\'', "''");
    let token = "00000000-0000-0000-0000-000000000000";

    let result = run_ps(
        "pwsh.exe",
        &format!("list-peers --token {} --root '{}'", token, safe_root),
        &guard,
    );
    let (_code, stdout, _stderr) = match result {
        Some(t) => t,
        None => {
            eprintln!("skip: pwsh.exe not available");
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }
    };

    assert!(
        !stdout.trim().is_empty(),
        "stdout should contain the JSON payload"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(parsed.is_array());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
#[ignore = "Manual developer check; CI uses smoke:cli-release-windows against shipped/testable exe names"]
fn list_peers_outputs_valid_json_under_powershell_noninteractive_via_git_bash() {
    debug_guard();
    let guard = TempConfigRoot::new();

    // #1596: Git Bash is the required carrier on Windows — bash.exe is
    // console-subsystem, so PowerShell captures its stdout AND propagates the
    // AC binary's exit code through it (`; exit $LASTEXITCODE`). Skip (not a
    // failure) when bash.exe is not on PATH, e.g. a non-Git-Bash dev machine.
    if Command::new("bash.exe")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("skip: bash.exe not available");
        return;
    }

    let tmp = std::env::temp_dir().join(format!(
        "ac-test-bash-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let safe_root = tmp.to_string_lossy().replace('\'', "''");
    let token = "00000000-0000-0000-0000-000000000000";

    // Inner bash command (single-quote escaping identical to `ps_command`):
    //   'BIN' list-peers --token <token> --root '<root>'
    // wrapped as  & bash.exe -lc '<inner>'; exit $LASTEXITCODE  at the PS level.
    let bash_inner = format!(
        "'{}' list-peers --token {} --root '{}'",
        BIN.replace('\'', "''"),
        token,
        safe_root
    );
    let ps_script = format!(
        "& bash.exe -lc '{}'; exit $LASTEXITCODE",
        bash_inner.replace('\'', "''")
    );
    let (code, stdout, stderr) =
        run_ps_script_with_exit_propagation("powershell.exe", &ps_script, &guard)
            .expect("powershell.exe must be available on Windows CI/dev");

    assert!(
        !stdout.trim().is_empty(),
        "stdout should contain the JSON payload via the Git Bash carrier. stderr=[{}]",
        stderr
    );
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should parse as JSON");
    assert!(parsed.is_array(), "expected JSON array");
    assert_eq!(
        code, 0,
        "outer powershell.exe must exit 0 (bash.exe propagates the AC binary's \
         $LASTEXITCODE through `; exit $LASTEXITCODE`); stderr=[{}]",
        stderr
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn issue_1867_powershell_builders_bind_distinct_owned_roots() {
    let before = std::env::var_os("AGENTSCOMMANDER_CONFIG_DIR");
    let first = TempConfigRoot::new();
    let second = TempConfigRoot::new();
    assert_ne!(first.path(), second.path());
    for guard in [&first, &second] {
        assert!(guard.path().is_absolute());
        assert!(guard.path().is_dir());
        let direct = ps_command("powershell.exe", "send --help", guard)
            .expect("powershell.exe must be available on Windows");
        let script = ps_script_command("powershell.exe", "exit 0", guard);
        for command in [direct, script] {
            let value = command
                .get_envs()
                .find(|(key, _)| *key == "AGENTSCOMMANDER_CONFIG_DIR")
                .and_then(|(_, value)| value);
            assert_eq!(value, Some(guard.path().as_os_str()));
        }
    }
    assert_eq!(std::env::var_os("AGENTSCOMMANDER_CONFIG_DIR"), before);
    let first_path = first.path().to_path_buf();
    drop(first);
    assert!(!first_path.exists());
    assert!(second.path().is_dir());
}

// Small lexical scanner: literals and comments cannot contribute braces or calls.
// Keep literal tokens intact so environment keys and command arguments are checked.
fn isolation_tokens(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while depth > 0 {
                assert!(i < bytes.len(), "unterminated block comment");
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        let start = i;
        if bytes[i] == b'r' {
            let mut quote = i + 1;
            while quote < bytes.len() && bytes[quote] == b'#' {
                quote += 1;
            }
            if quote < bytes.len() && bytes[quote] == b'"' {
                let end = format!("\"{}", "#".repeat(quote - i - 1));
                i = quote + 1 + source[quote + 1..].find(&end).expect("raw string end") + end.len();
                tokens.push(source[start..i].to_owned());
                continue;
            }
        }
        if bytes[i] == b'"'
            || (bytes[i] == b'\''
                && (bytes.get(i + 1) == Some(&b'\\') || bytes.get(i + 2) == Some(&b'\'')))
        {
            let quote = bytes[i];
            i += 1;
            loop {
                assert!(i < bytes.len(), "unterminated literal");
                if bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
        } else {
            i += source[i..].chars().next().unwrap().len_utf8();
        }
        tokens.push(source[start..i].to_owned());
    }
    tokens
}

fn isolation_function<'a>(tokens: &'a [String], name: &str) -> &'a [String] {
    let starts: Vec<_> = tokens
        .windows(3)
        .enumerate()
        .filter(|(_, words)| words[0] == "fn" && words[1] == name && words[2] == "(")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(starts.len(), 1, "unique function {name}");
    let start = starts[0];
    let open = start + tokens[start..].iter().position(|word| word == "{").unwrap();
    let mut depth = 0;
    for (i, word) in tokens.iter().enumerate().skip(open) {
        match word.as_str() {
            "{" => depth += 1,
            "}" => {
                depth -= 1;
                if depth == 0 {
                    return &tokens[start..=i];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced function {name}");
}

fn isolation_occurrences(tokens: &[String], pattern: &str) -> usize {
    let expected = isolation_tokens(pattern);
    tokens
        .windows(expected.len())
        .filter(|window| *window == expected)
        .count()
}

#[test]
fn issue_1867_isolation_source_contract() {
    // Frozen per-file and per-function routes. A bypass cannot hide in another helper.
    type Counts = &'static [(&'static str, usize)];
    type Entry = (&'static str, &'static str, usize, Counts, Counts);
    let inventory: &[Entry] = &[
        (
            "cli_agency_templates.rs",
            include_str!("cli_agency_templates.rs"),
            6,
            &[
                (
                    "agency_templates_unknown_subcommand_exits_one_with_usage",
                    1,
                ),
                ("agency_templates_update_prints_json_on_success_and_noop", 1),
                ("agency_templates_status_missing_cache_returns_json", 1),
                ("agency_templates_list_missing_cache_returns_empty_array", 1),
                ("agency_templates_list_pretty_returns_cached_metadata", 1),
                ("agency_templates_status_reports_locked_cache", 1),
            ],
            &[("Command::new(\"git\")", 1)],
        ),
        (
            "cli_behavior_contract.rs",
            include_str!("cli_behavior_contract.rs"),
            2,
            &[("run", 1), ("run_task_title", 1)],
            &[],
        ),
        (
            "cli_close_session.rs",
            include_str!("cli_close_session.rs"),
            1,
            &[("run_close_session_in_mode", 1)],
            &[("Command::new(\"powershell.exe\")", 1)],
        ),
        (
            "cli_create_agent_matrix.rs",
            include_str!("cli_create_agent_matrix.rs"),
            18,
            &[
                (
                    "create_agent_matrix_success_prints_json_and_writes_layout",
                    1,
                ),
                (
                    "create_agent_matrix_resolves_project_name_from_settings_for_unrelated_root",
                    1,
                ),
                (
                    "create_agent_matrix_rejects_ambiguous_project_name_without_writing",
                    1,
                ),
                ("create_agent_project_mode_requires_description", 1),
                ("create_agent_rejects_parent_argument", 1),
                ("create_agent_rejects_project_path_input_without_writing", 1),
                (
                    "create_agent_matrix_rejects_project_path_input_without_writing",
                    1,
                ),
                (
                    "create_agent_matrix_success_writes_project_refresh_request",
                    1,
                ),
                (
                    "create_agent_matrix_project_name_resolves_from_settings_not_cwd",
                    1,
                ),
                ("create_agent_project_mode_delegates_to_matrix_creation", 1),
                (
                    "create_agent_matrix_local_template_writes_role_and_skills",
                    1,
                ),
                (
                    "create_agent_matrix_invalid_template_exits_1_without_target_dir",
                    1,
                ),
                (
                    "create_agent_matrix_from_cached_agency_template_seeds_role_without_skills",
                    1,
                ),
                (
                    "create_agent_matrix_launch_reports_launched_when_app_confirms",
                    1,
                ),
                ("create_agent_matrix_launch_rejection_is_reported", 1),
                (
                    "create_agent_matrix_whitespace_launch_command_warns_without_request",
                    1,
                ),
                (
                    "create_agent_matrix_empty_launch_request_warns_without_request",
                    1,
                ),
                (
                    "create_agent_project_mode_blank_launch_command_warns_without_request",
                    1,
                ),
            ],
            &[],
        ),
        (
            "cli_harness.rs",
            include_str!("cli_harness.rs"),
            1,
            &[("run", 1)],
            &[],
        ),
        (
            "cli_loop.rs",
            include_str!("cli_loop.rs"),
            3,
            &[("run_json", 1), ("run_stdout", 1), ("run_fail", 1)],
            &[],
        ),
        (
            "cli_project_registration.rs",
            include_str!("cli_project_registration.rs"),
            3,
            &[("run_success", 1), ("run_json", 1), ("run_failure", 1)],
            &[],
        ),
        (
            "cli_raise_hand.rs",
            include_str!("cli_raise_hand.rs"),
            1,
            &[("run_raise_hand_with_simulator", 1)],
            &[("Command::new(\"powershell.exe\")", 1)],
        ),
        (
            "cli_role_experiment.rs",
            include_str!("cli_role_experiment.rs"),
            2,
            &[("run", 1), ("run_fake", 1)],
            &[],
        ),
        (
            "cli_task_logger.rs",
            include_str!("cli_task_logger.rs"),
            2,
            &[
                ("task_set_title_audit_line_reaches_file_sink", 1),
                (
                    "task_append_body_audit_line_reaches_file_sink_and_preserves_title",
                    1,
                ),
            ],
            &[],
        ),
        (
            "cli_workgroup_team.rs",
            include_str!("cli_workgroup_team.rs"),
            10,
            &[
                ("run_json", 1),
                ("run_json_machine", 1),
                ("run_fail", 1),
                ("run_fail_output", 1),
                ("run_stdout", 1),
                ("team_add_member_creates_replica_and_peer_is_reachable", 1),
                ("issue_1937_repeat_member_preserves_config", 1),
                (
                    "list_peers_surfaces_context_percent_for_matching_live_session",
                    1,
                ),
                (
                    "workgroup_add_legacy_missing_team_still_bootstraps_with_warning",
                    1,
                ),
                (
                    "purge_room_and_purge_wg_produce_identical_outbox_messages",
                    1,
                ),
            ],
            &[("Command::new(\"git\")", 2), ("Command::new(\"cmd\")", 1)],
        ),
        (
            "cli_ui_automation.rs",
            include_str!("cli_ui_automation.rs"),
            4,
            &[
                ("run", 1),
                ("run_with_env", 1),
                ("run_without_draining_output_until_exit", 1),
                ("reap_on_drop_kills_the_child_when_the_caller_panics", 1),
            ],
            &[],
        ),
        (
            "cli_window_placement.rs",
            include_str!("cli_window_placement.rs"),
            2,
            &[("run", 1), ("run_with_env", 1)],
            &[],
        ),
        (
            "terminal_snapshot_host.rs",
            include_str!("terminal_snapshot_host.rs"),
            2,
            &[("run", 1), ("run_with_closed_stdout", 1)],
            &[],
        ),
    ];
    for &(file, source, count, bindings, exclusions) in inventory {
        let tokens = isolation_tokens(source);
        for forbidden in ["std::env::set_var", "std::env::remove_var", "BUILD_PROFILE"] {
            assert_eq!(
                isolation_occurrences(&tokens, forbidden),
                0,
                "{file}: {forbidden}"
            );
        }
        let helper = isolation_function(&tokens, "command_for_binary");
        let config = if isolation_occurrences(&tokens, "fn config_dir_for(") == 1 {
            "config_dir_for"
        } else {
            "config_dir_for_bin"
        };
        let expected = format!(
            r#"fn command_for_binary(bin: &Path) -> Command {{
            let mut command = Command::new(bin);
            let stem = bin.file_stem().expect("bin stem").to_string_lossy();
            if !stem.contains('_') {{
                command.env("AGENTSCOMMANDER_CONFIG_DIR", {config}(bin));
            }}
            command
        }}"#
        );
        assert_eq!(
            helper,
            isolation_tokens(&expected),
            "{file}: suffix and override contract"
        );
        // Freeze the complete construction: counting parent/join calls alone lets
        // a ../ prefix escape the fixture and share state between sibling tests.
        let expected_path_helper = match file {
            "cli_agency_templates.rs"
            | "cli_behavior_contract.rs"
            | "cli_create_agent_matrix.rs"
            | "cli_loop.rs"
            | "cli_project_registration.rs"
            | "cli_role_experiment.rs"
            | "cli_workgroup_team.rs" => {
                r#"fn config_dir_for_bin(bin: &Path) -> PathBuf {
                let stem = bin.file_stem().expect("bin stem").to_string_lossy().to_string();
                bin.parent().expect("bin parent").join(format!(".{}", stem))
            }"#
            }
            "cli_close_session.rs"
            | "cli_raise_hand.rs"
            | "cli_window_placement.rs"
            | "terminal_snapshot_host.rs" => {
                r#"fn config_dir_for_bin(bin: &Path) -> PathBuf {
                let stem = bin.file_stem().expect("bin stem").to_string_lossy();
                bin.parent().expect("bin parent").join(format!(".{stem}"))
            }"#
            }
            "cli_harness.rs" => {
                r#"fn config_dir_for(bin: &Path) -> PathBuf {
                let stem = bin.file_stem().unwrap().to_string_lossy().to_string();
                bin.parent().unwrap().join(format!(".{}", stem))
            }"#
            }
            "cli_task_logger.rs" => {
                r#"fn config_dir_for_bin(bin: &Path) -> PathBuf {
                let stem = bin.file_stem().expect("bin has stem").to_string_lossy().to_string();
                bin.parent().expect("bin parent").join(format!(".{}", stem))
            }"#
            }
            "cli_ui_automation.rs" => {
                r#"fn config_dir_for(bin: &Path) -> PathBuf {
                let stem = bin.file_stem().unwrap().to_string_lossy();
                bin.parent().unwrap().join(format!(".{stem}"))
            }"#
            }
            _ => panic!("unrecognized isolation fixture: {file}"),
        };
        assert_eq!(
            isolation_function(&tokens, config),
            isolation_tokens(expected_path_helper),
            "{file}: config path must stay inside its owned fixture"
        );
        assert_eq!(
            isolation_occurrences(&tokens, "command_for_binary("),
            count + 1,
            "{file}: routes"
        );
        for &(name, calls) in bindings {
            assert_eq!(
                isolation_occurrences(isolation_function(&tokens, name), "command_for_binary("),
                calls,
                "{file}: {name}"
            );
        }
        assert_eq!(
            isolation_occurrences(&tokens, "Command::new("),
            1 + exclusions.iter().map(|(_, n)| n).sum::<usize>(),
            "{file}: exclusions"
        );
        for &(command, calls) in exclusions {
            assert_eq!(
                isolation_occurrences(&tokens, command),
                calls,
                "{file}: {command}"
            );
        }
    }
    let source = include_str!("cli_powershell_capture.rs");
    let tokens = isolation_tokens(source);
    assert_eq!(
        isolation_occurrences(&tokens, "Command::new("),
        5,
        "two builders and three probes"
    );
    assert_eq!(
        isolation_occurrences(&tokens, ".env("),
        2,
        "only the two builder overrides"
    );
    let direct = isolation_function(&tokens, "ps_command");
    let script = isolation_function(&tokens, "ps_script_command");
    for (name, builder) in [("ps_command", direct), ("ps_script_command", script)] {
        assert_eq!(
            isolation_occurrences(builder, "guard: &TempConfigRoot"),
            1,
            "{name}: guard parameter"
        );
        assert_eq!(
            isolation_occurrences(
                builder,
                "c.env(\"AGENTSCOMMANDER_CONFIG_DIR\", guard.path());"
            ),
            1,
            "{name}: guard binding"
        );
        assert_eq!(
            isolation_occurrences(builder, ".env("),
            1,
            "{name}: only owned override"
        );
    }
    let constructor = isolation_function(&tokens, "new");
    assert_eq!(
        constructor,
        isolation_tokens(
            r#"fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ac-powershell-config-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&path).expect("create owned config root");
        Self { path }
    }"#
        ),
        "unique owned root construction"
    );
    assert_eq!(
        isolation_function(&tokens, "drop"),
        isolation_tokens("fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.path); }"),
        "owned cleanup"
    );
    assert_eq!(
        isolation_function(&tokens, "path"),
        isolation_tokens("fn path(&self) -> &Path { &self.path }"),
        "owned root accessor"
    );
    for (function, call) in [
        ("run_ps", "ps_command(shell, args, guard)"),
        (
            "run_ps_script_with_exit_propagation",
            "ps_script_command(shell, script, guard)",
        ),
    ] {
        let body = isolation_function(&tokens, function);
        assert_eq!(
            isolation_occurrences(body, "guard: &TempConfigRoot"),
            1,
            "{function}"
        );
        assert_eq!(
            isolation_occurrences(body, call),
            1,
            "{function}: forwarding"
        );
    }
    // The original three availability probes remain separate from the builders.
    for function in ["ps_command", "run_ps_script_with_exit_propagation"] {
        assert_eq!(
            isolation_occurrences(
                isolation_function(&tokens, function),
                r#"Command::new(shell).arg("-Help").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err()"#
            ),
            1,
            "{function}: probe"
        );
    }
    let ignored = [
        "list_peers_outputs_valid_json_under_powershell_noninteractive",
        "send_help_outputs_under_powershell_noninteractive",
        "send_unknown_flag_emits_stderr_under_powershell_noninteractive",
        "list_peers_outputs_valid_json_under_pwsh_noninteractive",
        "list_peers_outputs_valid_json_under_powershell_noninteractive_via_git_bash",
    ];
    for name in ignored {
        let body = isolation_function(&tokens, name);
        assert_eq!(
            isolation_occurrences(body, "let guard = TempConfigRoot::new();"),
            1,
            "{name}: local guard"
        );
        assert_eq!(
            isolation_occurrences(body, "&guard"),
            1,
            "{name}: guard passed"
        );
    }
    assert_eq!(
        isolation_occurrences(
            isolation_function(&tokens, ignored[4]),
            r#"Command::new("bash.exe").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err()"#
        ),
        1,
        "Bash probe"
    );
    for forbidden in ["std::env::set_var", "std::env::remove_var", "BUILD_PROFILE"] {
        assert_eq!(
            isolation_occurrences(&tokens, forbidden),
            0,
            "PowerShell: {forbidden}"
        );
    }
}
