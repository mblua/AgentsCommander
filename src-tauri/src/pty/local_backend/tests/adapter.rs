use super::*;
use crate::errors::AppError;
use crate::pty::backend::ResolvedAgentHostShell;

fn host(program: &str, args: &[&str]) -> ResolvedAgentHostShell {
    ResolvedAgentHostShell {
        program: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
    }
}

fn launch_error(launch: Result<PreparedLaunch, AppError>) -> String {
    match launch {
        Ok(_) => panic!("expected adapter rejection"),
        Err(e) => e.to_string(),
    }
}

fn assert_error_shape(error: &str, host_program: &str, token: &str, category: &str) {
    assert!(
        error.contains(host_program),
        "error must name the configured host '{host_program}': {error}"
    );
    assert!(
        error.contains(token),
        "error must name the offending token '{token}': {error}"
    );
    assert!(
        error.contains(category),
        "error must state the category '{category}': {error}"
    );
    assert!(
        error.contains("agent adapter owns command execution"),
        "error must state that the agent adapter owns command execution: {error}"
    );
}

// --- Bare agent + configured PowerShell ---------------------------------

#[test]
fn configured_default_shell_powershell_hosts_bare_agent_with_managed_native_script() {
    let shell = host(
        r"C:\Program Files\WindowsPowerShell\v1.0\powershell.exe",
        &["-NoProfile", "-ExecutionPolicy", "Bypass"],
    );
    let launch = prepare_windows_resolved_agent_launch("claude", &[], &shell)
        .expect("valid configured PowerShell host");

    // The adapter program is the exact configured path, never cmd.exe.
    assert_eq!(
        launch.program,
        r"C:\Program Files\WindowsPowerShell\v1.0\powershell.exe"
    );
    assert_eq!(
        launch.args[..3],
        [
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string()
        ]
    );
    assert_eq!(launch.args[3], "-Command");
    let script = &launch.args[4];
    assert!(
        !script.contains("--%"),
        "generated script must never contain the stop-parsing token"
    );
    assert!(
        !script.contains("cmd.exe /C"),
        "generated script must not be a configured cmd fallback"
    );
    // Two-argument GetCommand application-only lookup, never `$true` third arg.
    assert!(script.contains("GetCommand('claude', $ac_kind)"));
    assert!(!script.contains("GetCommand('claude', $ac_kind, $true)"));
    assert!(script.contains(
        "[System.Management.Automation.CommandTypes]::Application -bor \
         [System.Management.Automation.CommandTypes]::ExternalScript"
    ));
    // Native branch: separate FileName + ProcessStartInfo with inherited handles.
    assert!(script.contains("$ac_start.FileName = $ac_command.Path"));
    assert!(script.contains("$ac_start.UseShellExecute = $false"));
    assert!(script.contains("$ac_start.RedirectStandardInput = $false"));
    assert!(script.contains("$ac_start.RedirectStandardOutput = $false"));
    assert!(script.contains("$ac_start.RedirectStandardError = $false"));
    assert!(script.contains("$ac_process.WaitForExit()"));
    assert!(script.contains("exit $ac_process.ExitCode"));
    // No logical args: Arguments is the empty ps-literal.
    assert!(script.contains("$ac_start.Arguments = '';"));
    // Provenance matches the launch exactly (one adapter result).
    let mut expected_argv = vec![shell.program.clone()];
    expected_argv.extend(shell.args.iter().cloned());
    expected_argv.push("-Command".to_string());
    expected_argv.push(script.clone());
    assert_eq!(launch.exec_argv(), expected_argv);
}

#[test]
fn configured_default_shell_pwsh_full_path_with_spaces_is_used_exactly() {
    let shell = host(r"C:\Program Files\PowerShell\7\pwsh.exe", &["-NoProfile"]);
    let args = vec!["agent's tool".to_string(), "a b".to_string()];
    let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
        .expect("valid configured pwsh host");

    assert_eq!(launch.program, r"C:\Program Files\PowerShell\7\pwsh.exe");
    assert_eq!(launch.args[0], "-NoProfile");
    assert_eq!(launch.args[1], "-Command");
    let script = &launch.args[2];
    // The lookup name retains spaces and apostrophes through ps-literal.
    assert!(script.contains("'claude'"), "bare lookup name: {script}");
    // The native Arguments value is the standard Windows encoding of the
    // logical argv through a PowerShell literal.
    assert!(script.contains("$ac_start.Arguments = '\"agent''s tool\" \"a b\"';"));
    assert!(!script.contains("--%"));
    // The only cmd.exe mention is the documented nested system-cmd child
    // for a resolved .cmd/.bat target, never the top-level host.
    assert!(script.contains("'cmd.exe'"));
}

#[test]
fn configured_default_shell_powershell_script_carries_batch_branch_and_raw_argv() {
    let shell = host("powershell.exe", &[]);
    let args = vec![
        "".to_string(),
        "a\"b".to_string(),
        "with space".to_string(),
        "o'clock".to_string(),
        "tail\\".to_string(),
        "a%z|p".to_string(),
        "a\\b\"c".to_string(),
    ];
    let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
        .expect("percent and pipe are data in the native branch");
    let script = &launch.args[1];
    assert!(script.contains("$ac_batch_unsupported_logical_arg = $true;"));
    // Batch sub-branch with the system-cmd child.
    assert!(script.contains("([System.IO.Path]::GetExtension($ac_command.Path) -ieq '.cmd')"));
    assert!(script
        .contains("[System.IO.Path]::Combine([System.Environment]::SystemDirectory, 'cmd.exe')"));
    assert!(script.contains("'/D /V:OFF /S /C \"' + $ac_batch_payload + '\"'"));
    assert!(script.contains("$ac_command.Path.Contains([char]34)"));
    // The native Arguments literal is windows_raw_argv of the logical args.
    assert!(script.contains(
        "$ac_start.Arguments = '\"\" \"a\\\"b\" \"with space\" \"o''clock\" \
         \"tail\\\\\" \"a%z|p\" \"a\\b\\\"c\"';"
    ));
    // ExternalScript branch keeps ps-literal invocation.
    assert!(script.contains(
        "& $ac_command.Path '' 'a\"b' 'with space' 'o''clock' 'tail\\' 'a%z|p' 'a\\b\"c';"
    ));
    assert!(script.contains("$ac_succeeded = $?; $ac_exit_code = $LASTEXITCODE;"));
    assert!(script.ends_with("if ($ac_succeeded) { exit 0 }; exit 1"));
    assert!(!script.ends_with('\\'), "trailing-run invariant");
}

#[test]
fn configured_default_shell_powershell_script_flags_batch_unsupported_arguments() {
    let shell = host("powershell.exe", &[]);
    let args = vec!["ok".to_string(), "bad%value".to_string()];
    let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
        .expect("native branch accepts percent");
    let script = &launch.args[1];
    assert!(script.contains("$ac_batch_unsupported_logical_arg = $true;"));
    assert!(script.contains("if ($ac_batch_unsupported_logical_arg -or"));
}

// --- Explicit batch targets through PowerShell --------------------------

#[test]
fn configured_default_shell_powershell_explicit_batch_rejects_percent_or_quote_args() {
    let shell = host("powershell.exe", &[]);
    let err = launch_error(prepare_windows_resolved_agent_launch(
        r"C:\tools\claude.cmd",
        &["a%b".to_string()],
        &shell,
    ));
    assert_error_shape(
        &err,
        "powershell.exe",
        "(batch target)",
        "unsupported explicit-batch",
    );
    let err = launch_error(prepare_windows_resolved_agent_launch(
        r"C:\tools\claude.bat",
        &["a\"b".to_string()],
        &shell,
    ));
    assert_error_shape(
        &err,
        "powershell.exe",
        "(batch target)",
        "unsupported explicit-batch",
    );
    // Clean explicit batch argv is accepted (nested system-cmd path).
    let launch = prepare_windows_resolved_agent_launch(
        r"C:\tools\claude.cmd",
        &["flag".to_string(), "bang!".to_string()],
        &shell,
    )
    .expect("explicit batch with clean args is supported");
    assert!(launch.args[1].contains("$ac_batch_unsupported_logical_arg = $false;"));
}

// --- cmd host -----------------------------------------------------------

#[test]
fn configured_default_shell_cmd_uses_unquoted_single_payload() {
    let shell = host(r"C:\Windows\System32\cmd.exe", &["/D", "/Q"]);
    let args = vec![
        "--flag".to_string(),
        "bang!".to_string(),
        r"a\b".to_string(),
    ];
    let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
        .expect("valid configured cmd host");
    assert_eq!(launch.program, r"C:\Windows\System32\cmd.exe");
    assert_eq!(launch.args[..2], ["/D".to_string(), "/Q".to_string()]);
    assert_eq!(
        launch.args[2..5],
        ["/V:OFF".to_string(), "/S".to_string(), "/C".to_string()]
    );
    // One single payload: program + args joined with ASCII spaces, no
    // per-token quoting, no outer quote pair of our own, no %% or caret.
    assert_eq!(launch.args[5], r"claude --flag bang! a\b");
    assert_eq!(launch.args.len(), 6);
    assert_eq!(
        launch.exec_argv(),
        vec![
            shell.program.clone(),
            "/D".to_string(),
            "/Q".to_string(),
            "/V:OFF".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            r"claude --flag bang! a\b".to_string(),
        ]
    );
}

#[test]
fn configured_default_shell_cmd_payload_rejects_out_of_domain_tokens() {
    let shell = host("cmd.exe", &[]);
    let rejected: &[&str] = &[
        "with space",
        "a\"b",
        "a&b",
        "a|b",
        "a^b",
        "a<b",
        "a>b",
        "a(b",
        "a)b",
        "a%b",
        "a=b",
        "a,b",
        "a;b",
        "",
        "a\\",
    ];
    for token in rejected {
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[token.to_string()],
            &shell,
        ));
        if token.ends_with('\\') {
            // The payload-final backslash rule reports the payload itself.
            assert_error_shape(
                &err,
                "cmd.exe",
                "(payload)",
                "unsupported cmd payload character",
            );
        } else {
            assert_error_shape(&err, "cmd.exe", token, "unsupported cmd payload character");
        }
    }
    // Leading '@' on the program token is rejected.
    let err = launch_error(prepare_windows_resolved_agent_launch(
        "@claude",
        &[],
        &shell,
    ));
    assert_error_shape(
        &err,
        "cmd.exe",
        "@claude",
        "unsupported cmd payload character",
    );
    // NUL/CR/LF remain forbidden (common rule).
    for bad in ["a\0b", "a\rb", "a\nb"] {
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[bad.to_string()],
            &shell,
        ));
        assert!(err.contains("forbidden line separator"), "{err}");
    }
}

#[test]
fn configured_default_shell_cmd_accepts_flag_style_bang_and_internal_backslashes() {
    let shell = host("cmd.exe", &[]);
    let args = vec![
        "--flag".to_string(),
        "-x".to_string(),
        "!bang!".to_string(),
        r"a\b\c".to_string(),
        "plain".to_string(),
    ];
    let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
        .expect("accepted cmd payload domain");
    assert_eq!(
        launch.args[launch.args.len() - 1],
        r"claude --flag -x !bang! a\b\c plain"
    );
}

// --- Custom POSIX-compatible shell --------------------------------------

#[test]
fn configured_default_shell_custom_posix_host_gets_exec_script() {
    let shell = host(r"C:\tools\bash.exe", &[]);
    let args = vec!["it's".to_string(), "a b".to_string()];
    let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
        .expect("custom posix host with empty args");
    assert_eq!(launch.program, r"C:\tools\bash.exe");
    assert_eq!(launch.args[0], "-c");
    assert_eq!(launch.args[1], "exec 'claude' 'it'\"'\"'s' 'a b'");
    assert_eq!(launch.args.len(), 2);
}

#[test]
fn configured_default_shell_custom_posix_host_rejects_any_configured_args() {
    let shell = host("bash.exe", &["--norc"]);
    let err = launch_error(prepare_windows_resolved_agent_launch("claude", &[], &shell));
    assert_error_shape(&err, "bash.exe", "--norc", "conflicting/terminal");
}

// --- #1498 - detached launchers and terminal-only POSIX args ------------

#[test]
fn reject_detached_launcher_matches_basename_case_insensitively() {
    for program in [
        "git-bash.exe",
        "GIT-BASH.EXE",
        r"C:\Program Files\Git\git-bash.exe",
        "mintty.exe",
        r"C:\tools\mintty.exe",
        r"\\server\share\MINTTY.EXE",
    ] {
        let error = reject_detached_launcher(program)
            .expect_err("launcher spelling must be rejected")
            .to_string();
        assert!(error.contains("detached launcher"), "{error}");
        assert!(error.contains(r"bin\bash.exe"), "{error}");
    }
    for program in [
        "bash.exe",
        r"C:\Program Files\Git\bin\bash.exe",
        "claude.exe",
        "cmd.exe",
        "powershell.exe",
        "git-bash",
        "mintty",
        "sh",
    ] {
        reject_detached_launcher(program).expect("non-launcher must pass");
    }
}

#[test]
fn bare_git_bash_terminal_args_pass_through_verbatim() {
    let launch = prepare_launch(
        r"C:\Program Files\Git\bin\bash.exe",
        &["--login".to_string(), "-i".to_string()],
        None,
    )
    .expect("bare Git Bash terminal must pass through verbatim");
    assert_eq!(launch.program, r"C:\Program Files\Git\bin\bash.exe");
    assert_eq!(launch.args, vec!["--login".to_string(), "-i".to_string()]);
}

#[test]
fn resolved_posix_host_drops_terminal_only_args_and_owns_c() {
    let shell = host("bash.exe", &["--login", "-i", "-NoLogo"]);
    let args = vec!["it's".to_string(), "a b".to_string()];
    let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
        .expect("terminal-only configured args must be dropped");
    assert_eq!(launch.program, "bash.exe");
    assert_eq!(launch.args.len(), 2);
    assert_eq!(launch.args[0], "-c");
    assert_eq!(launch.args[1], "exec 'claude' 'it'\"'\"'s' 'a b'");

    let shell = host("Bash.Exe", &["-L", "--LOGIN", "-NOlOgO"]);
    let args = vec!["it's".to_string(), "a b".to_string()];
    let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
        .expect("terminal-only configured args must be dropped case-insensitively");
    assert_eq!(launch.program, "Bash.Exe");
    assert_eq!(launch.args.len(), 2);
    assert_eq!(launch.args[0], "-c");
    assert_eq!(launch.args[1], "exec 'claude' 'it'\"'\"'s' 'a b'");
}

#[test]
fn resolved_posix_host_conflicting_args_still_rejected() {
    for (shell_args, offending) in [
        (&["-c"][..], "-c"),
        (&["--login", "-c"][..], "-c"),
        (&["--norc"][..], "--norc"),
        (&["--login", "--norc"][..], "--norc"),
    ] {
        let shell = host("bash.exe", shell_args);
        let err = launch_error(prepare_windows_resolved_agent_launch("claude", &[], &shell));
        assert_error_shape(&err, "bash.exe", offending, "conflicting/terminal");
    }
}

#[test]
fn resolved_posix_host_exec_payload_quotes_paths_with_spaces() {
    let shell = host(r"C:\tools\bash.exe", &[]);
    let program = r"C:\Program Files\Claude\claude.exe";
    let args = vec![
        "--config".to_string(),
        r"C:\Program Files\Claude\config.toml".to_string(),
    ];
    let launch = prepare_windows_resolved_agent_launch(program, &args, &shell).expect("posix host");
    assert_eq!(
        launch.args,
        vec![
            "-c".to_string(),
            "exec 'C:\\Program Files\\Claude\\claude.exe' '--config' 'C:\\Program \
                Files\\Claude\\config.toml'"
                .to_string(),
        ]
    );
}

#[test]
fn resolved_agent_rejects_detached_launcher_host() {
    for program in [
        "git-bash.exe",
        r"C:\Program Files\Git\git-bash.exe",
        "mintty.exe",
    ] {
        let shell = host(program, &["--login", "-i"]);
        let err = launch_error(prepare_windows_resolved_agent_launch("claude", &[], &shell));
        assert!(err.contains("detached launcher"), "{err}");
        assert!(err.contains(r"bin\bash.exe"), "{err}");
    }
    let shell = host("bash.exe", &["--login", "-i"]);
    let launch = prepare_windows_resolved_agent_launch("claude", &[], &shell)
        .expect("bash host with terminal-only args");
    assert_eq!(
        launch.args,
        vec!["-c".to_string(), "exec 'claude'".to_string()]
    );
}

#[cfg(windows)]
#[test]
fn detached_launcher_rejected_in_bare_and_direct_paths() {
    let error = prepare_launch(
        "git-bash.exe",
        &["--login".to_string(), "-i".to_string()],
        None,
    )
    .expect_err("bare git-bash terminal must be rejected on Windows")
    .to_string();
    assert!(error.contains("detached launcher"), "{error}");

    let error = prepare_launch("mintty.exe", &[], None)
        .expect_err("bare mintty terminal must be rejected on Windows")
        .to_string();
    assert!(error.contains("detached launcher"), "{error}");

    let error = prepare_launch(
        "git-bash.exe",
        &["--x".to_string()],
        Some(&host("bash.exe", &[])),
    )
    .expect_err("direct launcher agent must be rejected on Windows")
    .to_string();
    assert!(error.contains("detached launcher"), "{error}");

    let launch = prepare_launch(
        "claude.exe",
        &["--x".to_string()],
        Some(&host("git-bash.exe", &[])),
    )
    .expect("direct .exe agent must ignore its host shell");
    assert_eq!(launch.program, "claude.exe");
    assert_eq!(launch.args, vec!["--x".to_string()]);
}

// --- Direct .exe and host-name extraction --------------------------------

#[test]
fn direct_exe_agent_keeps_historical_shape_without_host_context() {
    let shell = host("powershell.exe", &["-NoProfile"]);
    for program in ["claude.exe", r"C:\tools\Claude.EXE", "agent.EXE"] {
        let launch = prepare_launch(program, &["--x".to_string()], Some(&shell))
            .expect("direct exe must stay direct");
        assert_eq!(launch.program, program);
        assert_eq!(launch.args, vec!["--x".to_string()]);
    }
}

#[test]
fn configured_shell_kind_extracts_final_path_component() {
    for (program, expected) in [
        ("powershell", WindowsHostShellKind::PowerShell),
        ("powershell.exe", WindowsHostShellKind::PowerShell),
        (
            r"C:\Program Files\WindowsPowerShell\v1.0\powershell.exe",
            WindowsHostShellKind::PowerShell,
        ),
        (
            r"\\server\share\POWERSHELL.EXE",
            WindowsHostShellKind::PowerShell,
        ),
        (r"\\?\C:\tools\pwsh.exe", WindowsHostShellKind::Pwsh),
        ("pwsh", WindowsHostShellKind::Pwsh),
        ("cmd", WindowsHostShellKind::Cmd),
        (
            r"C:\Program Files (x86)\thing\cmd.exe",
            WindowsHostShellKind::Cmd,
        ),
        ("cmd.exe", WindowsHostShellKind::Cmd),
        (
            r"C:\Program Files\Git\bin\bash.exe",
            WindowsHostShellKind::Posix,
        ),
        ("/usr/bin/sh", WindowsHostShellKind::Posix),
    ] {
        assert_eq!(
            configured_shell_kind(program),
            expected,
            "classification for {program}"
        );
    }
}

#[test]
fn is_direct_exe_predicate_is_case_insensitive_and_extension_based() {
    assert!(is_direct_exe("claude.exe"));
    assert!(is_direct_exe("CLAUDE.EXE"));
    assert!(is_direct_exe(r"C:\tools\Agent.Exe"));
    assert!(!is_direct_exe("claude"));
    assert!(!is_direct_exe("claude.cmd"));
    assert!(!is_direct_exe("claude.bat"));
}

// --- Encoders -----------------------------------------------------------

#[test]
fn windows_arg_encodes_with_standard_quote_and_backslash_rules() {
    assert_eq!(windows_arg(""), "\"\"");
    assert_eq!(windows_arg("a"), "\"a\"");
    assert_eq!(windows_arg("a\"b"), "\"a\\\"b\"");
    assert_eq!(windows_arg("with space"), "\"with space\"");
    assert_eq!(windows_arg("o'clock"), "\"o'clock\"");
    assert_eq!(windows_arg("tail\\"), "\"tail\\\\\"");
    assert_eq!(windows_arg("a%z|p"), "\"a%z|p\"");
    assert_eq!(windows_arg("a\\b\"c"), r#""a\b\"c""#);
    assert_eq!(windows_arg("a\\\""), r#""a\\\"""#);
    assert_eq!(windows_arg("a\\\\\"c"), r#""a\\\\\"c""#);
    assert_eq!(windows_arg("a\\"), "\"a\\\\\"");
    assert_eq!(windows_arg("\\\\server\\share"), r#""\\server\share""#);
}
#[test]
fn windows_raw_argv_joins_encoded_arguments_only() {
    assert_eq!(windows_raw_argv(&[]), "");
    assert_eq!(windows_raw_argv(&["".to_string()]), "\"\"");
    assert_eq!(
        windows_raw_argv(&["a b".to_string(), "".to_string()]),
        "\"a b\" \"\""
    );
}

#[test]
fn ps_and_posix_literals_follow_their_quoting_rules() {
    assert_eq!(ps_literal("plain"), "'plain'");
    assert_eq!(ps_literal("o'clock"), "'o''clock'");
    assert_eq!(ps_literal(""), "''");
    assert_eq!(ps_literal("a\"b"), "'a\"b'");
    assert_eq!(ps_literal("a\\"), r"'a\'");
    assert_eq!(posix_literal("plain"), "'plain'");
    assert_eq!(posix_literal("it's"), "'it'\"'\"'s'");
}

// --- Closed configured-host grammar: rejected forms ----------------------

#[test]
fn powershell_grammar_rejects_command_ownership_and_terminal_options() {
    // Every conflicting/terminal spelling, in both - and / prefix spellings.
    for prefix in ["-", "/"] {
        for spelling in [
            "Command",
            "c",
            "CommandWithArgs",
            "cwa",
            "File",
            "f",
            "EncodedCommand",
            "enc",
            "e",
            "NoExit",
            "noe",
            "Version",
            "v",
            "Help",
            "h",
            "?",
            "SSHServerMode",
            "ServerMode",
            "WindowsPowerShell",
            "Login",
            "Interactive",
        ] {
            for is_pwsh in [false, true] {
                let token = format!("{prefix}{spelling}");
                let err = launch_error(prepare_windows_resolved_agent_launch(
                    "claude",
                    &[],
                    &host(
                        if is_pwsh {
                            "pwsh.exe"
                        } else {
                            "powershell.exe"
                        },
                        &[token.as_str()],
                    ),
                ));
                assert_error_shape(
                    &err,
                    if is_pwsh {
                        "pwsh.exe"
                    } else {
                        "powershell.exe"
                    },
                    &token,
                    "conflicting/terminal configured option",
                );
            }
        }
    }
    // Parser terminators are literal tokens.
    for token in ["--", "--%"] {
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("powershell.exe", &[token]),
        ));
        assert_error_shape(&err, "powershell.exe", token, "conflicting/terminal");
    }
}

#[test]
fn pwsh_grammar_rejects_powershell_only_spellings_as_unknown() {
    for prefix in ["-", "/"] {
        for spelling in ["Sta", "Mta", "WindowStyle", "PSConsoleFile"] {
            let token = format!("{prefix}{spelling}");
            let err = launch_error(prepare_windows_resolved_agent_launch(
                "claude",
                &[],
                &host("pwsh.exe", &[token.as_str()]),
            ));
            assert_error_shape(&err, "pwsh.exe", &token, "unknown or ambiguous token");
        }
    }
    // The same spellings remain PERMITTED for powershell.exe.
    let launch = prepare_windows_resolved_agent_launch(
        "claude",
        &[],
        &host("powershell.exe", &["-Sta", "-WindowStyle", "Hidden"]),
    )
    .expect("powershell.exe accepts its own spellings");
    assert_eq!(launch.args[0], "-Sta");
}

#[test]
fn powershell_grammar_rejects_bare_unknown_and_abbreviated_tokens() {
    for token in [
        "foo", "echo", "-NoPro", "/NoPro", "-Bogus", "/Bogus", "-noex", "/E:O",
    ] {
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("powershell.exe", &[token]),
        ));
        assert_error_shape(&err, "powershell.exe", token, "unknown or ambiguous token");
    }
}

#[test]
fn powershell_grammar_rejects_missing_and_option_shaped_operands() {
    let err = launch_error(prepare_windows_resolved_agent_launch(
        "claude",
        &[],
        &host("powershell.exe", &["-ExecutionPolicy"]),
    ));
    assert_error_shape(
        &err,
        "powershell.exe",
        "ExecutionPolicy",
        "missing or option-shaped",
    );
    let err = launch_error(prepare_windows_resolved_agent_launch(
        "claude",
        &[],
        &host("powershell.exe", &["-ExecutionPolicy", "-NoProfile"]),
    ));
    assert_error_shape(
        &err,
        "powershell.exe",
        "-NoProfile",
        "option-shaped separated operand",
    );
    let err = launch_error(prepare_windows_resolved_agent_launch(
        "claude",
        &[],
        &host("powershell.exe", &["-ExecutionPolicy:"]),
    ));
    assert_error_shape(
        &err,
        "powershell.exe",
        "-ExecutionPolicy:",
        "missing or option-shaped",
    );
}

#[test]
fn powershell_grammar_rejects_duplicates() {
    for (args, offending) in [
        (vec!["-NoProfile", "-NoProfile"], "-NoProfile"),
        (vec!["-NoProfile", "/NoProfile"], "/NoProfile"),
        (
            vec![
                "-ExecutionPolicy:Bypass",
                "-ExecutionPolicy",
                "RemoteSigned",
            ],
            "RemoteSigned",
        ),
    ] {
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("powershell.exe", &args),
        ));
        assert_error_shape(
            &err,
            "powershell.exe",
            offending,
            "duplicate configured option",
        );
    }
}

#[test]
fn cmd_grammar_rejects_ownership_terminals_and_unknowns() {
    for token in [
        "/C", "/K", "/S", "/V:ON", "/V:", "/V:ZZ", "/?", "-D", "foo", "/help", "/E:O",
    ] {
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host("cmd.exe", &[token]),
        ));
        let category = if matches!(token, "/C" | "/K" | "/S" | "/V:ON" | "/V:" | "/V:ZZ" | "/?") {
            "conflicting/terminal"
        } else {
            "unknown or ambiguous"
        };
        assert_error_shape(&err, "cmd.exe", token, category);
    }
    let err = launch_error(prepare_windows_resolved_agent_launch(
        "claude",
        &[],
        &host("cmd.exe", &["/T:"]),
    ));
    assert_error_shape(&err, "cmd.exe", "/T:", "missing or option-shaped");
    let err = launch_error(prepare_windows_resolved_agent_launch(
        "claude",
        &[],
        &host("cmd.exe", &["/D", "/D"]),
    ));
    assert_error_shape(&err, "cmd.exe", "/D", "duplicate configured option");
}

// --- Closed configured-host grammar: accepted forms ----------------------

#[test]
fn powershell_grammar_accepts_permitted_spellings_with_operand_binding() {
    // Flags + one separated operand + one attached `:` operand, order kept.
    let launch = prepare_windows_resolved_agent_launch(
        "claude",
        &[],
        &host(
            "powershell.exe",
            &[
                "-NoProfile",
                "/NoLogo",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
            ],
        ),
    )
    .expect("accepted powershell spellings");
    // Order preserved, every operand bound exactly once, suffix appended.
    assert_eq!(
        launch.args[..7],
        [
            "-NoProfile".to_string(),
            "/NoLogo".to_string(),
            "-NonInteractive".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-WindowStyle".to_string(),
            "Hidden".to_string(),
        ]
    );
    assert_eq!(launch.args[7], "-Command");

    // Each attached binding form is a separate accepted spelling.
    for (token, operand) in [
        ("-ExecutionPolicy:Bypass", "-ExecutionPolicy:Bypass"),
        ("-ExecutionPolicy=Bypass", "-ExecutionPolicy=Bypass"),
        ("/NoProfile", "/NoProfile"),
    ] {
        let launch =
            prepare_windows_resolved_agent_launch("claude", &[], &host("powershell.exe", &[token]))
                .expect("accepted attached binding form");
        assert_eq!(launch.args[0], operand);
        assert_eq!(launch.args[1], "-Command");
    }

    let launch = prepare_windows_resolved_agent_launch(
        "claude",
        &[],
        &host(
            "pwsh.exe",
            &[
                "-WorkingDirectory",
                r"C:\Program Files",
                "-SettingsFile",
                "x.json",
            ],
        ),
    )
    .expect("accepted pwsh spellings");
    assert_eq!(
        launch.args[..4],
        [
            "-WorkingDirectory".to_string(),
            r"C:\Program Files".to_string(),
            "-SettingsFile".to_string(),
            "x.json".to_string(),
        ]
    );
    assert_eq!(launch.args[4], "-Command");
}

#[test]
fn cmd_grammar_accepts_permitted_spellings_in_order() {
    let launch = prepare_windows_resolved_agent_launch(
        "claude",
        &[],
        &host(
            "cmd.exe",
            &["/D", "/Q", "/E:ON", "/T:1F", "/V:OFF", "/A", "/F:OFF"],
        ),
    )
    .expect("accepted cmd spellings");
    assert_eq!(
        launch.args[..7],
        [
            "/D".to_string(),
            "/Q".to_string(),
            "/E:ON".to_string(),
            "/T:1F".to_string(),
            "/V:OFF".to_string(),
            "/A".to_string(),
            "/F:OFF".to_string(),
        ]
    );
    assert_eq!(launch.args[7], "/V:OFF"); // adapter suffix
    assert_eq!(launch.args[8], "/S");
    assert_eq!(launch.args[9], "/C");
}

// --- Common validation ---------------------------------------------------

#[test]
fn common_validation_rejects_blank_host_and_line_separators() {
    let err = launch_error(prepare_windows_resolved_agent_launch(
        "claude",
        &[],
        &host("   ", &[]),
    ));
    assert!(
        err.contains("blank configured default-shell program"),
        "{err}"
    );

    for bad in ["pow\0er", "pow\rer", "pow\ner"] {
        let err = launch_error(prepare_windows_resolved_agent_launch(
            "claude",
            &[],
            &host(bad, &[]),
        ));
        assert!(err.contains("forbidden line separator"), "{err}");
    }
    let err = launch_error(prepare_windows_resolved_agent_launch(
        "claude",
        &["a\nb".to_string()],
        &host("powershell.exe", &[]),
    ));
    assert!(err.contains("forbidden line separator"), "{err}");
    let err = launch_error(prepare_windows_resolved_agent_launch(
        "cla\0ude",
        &[],
        &host("powershell.exe", &[]),
    ));
    assert!(err.contains("forbidden line separator"), "{err}");
}

// --- No-resolved-agent fallback and cross-platform shape ------------------

#[test]
fn prepare_launch_keeps_cmd_exe_fallback_without_host_shell_and_direct_otherwise() {
    // No host shell + non-direct command: historical cmd.exe /C wrapper.
    let launch = prepare_launch("claude", &["--x".to_string()], None).expect("fallback launch");
    assert_eq!(launch.program, "cmd.exe");
    assert_eq!(
        launch.args,
        vec!["/C".to_string(), "claude".to_string(), "--x".to_string()]
    );
    // Direct exe: unchanged program + argv.
    let launch = prepare_launch("claude.exe", &["--x".to_string()], None).expect("direct launch");
    assert_eq!(launch.program, "claude.exe");
    assert_eq!(launch.args, vec!["--x".to_string()]);
}

#[test]
fn powershell_script_never_ends_with_backslash_across_awkward_values() {
    let shell = host("powershell.exe", &[]);
    for args in [
        vec![],
        vec!["tail\\".to_string()],
        vec!["a'b".to_string()],
        vec!["a\\b\"c".to_string(), "x".to_string()],
    ] {
        let launch = prepare_windows_resolved_agent_launch("claude", &args, &shell)
            .expect("accepted native values");
        let script = launch.args.last().unwrap();
        assert!(
            !script.ends_with('\\'),
            "trailing-run invariant for {args:?}"
        );
    }
}

#[test]
fn command_builder_and_exec_argv_share_one_adapter_result() {
    let shell = host("powershell.exe", &["-NoProfile"]);
    let launch = prepare_windows_resolved_agent_launch("claude", &["--x".to_string()], &shell)
        .expect("adapter launch");
    let builder = launch.command_builder();
    let _ = builder; // CommandBuilder has no public argv; exec_argv is the
                     // provenance, asserted equal to the adapter result above.
    assert_eq!(launch.exec_argv().len(), 4);
}
