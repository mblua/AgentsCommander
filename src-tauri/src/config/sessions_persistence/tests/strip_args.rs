use super::*;

#[test]
fn strip_auto_injected_args_removes_direct_antigravity_continue() {
    let stripped = strip_auto_injected_args(
        "agy",
        &[
            "--continue".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ],
    );
    assert_eq!(stripped, vec!["-m".to_string(), "gpt-5".to_string()]);
}

#[test]
fn strip_auto_injected_args_removes_cmd_antigravity_continue() {
    let stripped = strip_auto_injected_args(
        "cmd.exe",
        &[
            "/C".to_string(),
            "agy".to_string(),
            "--continue".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ],
    );
    assert_eq!(
        stripped,
        vec![
            "/C".to_string(),
            "agy".to_string(),
            "-m".to_string(),
            "gpt-5".to_string()
        ]
    );
}

#[test]
fn strip_auto_injected_args_removes_embedded_cmd_antigravity_continue() {
    let stripped = strip_auto_injected_args(
        "cmd.exe",
        &[
            "/K".to_string(),
            "git pull && agy --continue -m gpt-5".to_string(),
        ],
    );
    assert_eq!(
        stripped,
        vec!["/K".to_string(), "git pull && agy -m gpt-5".to_string()]
    );
}

#[test]
fn strip_auto_injected_args_preserves_user_authored_antigravity_resume_forms() {
    // `-c` and `--conversation <ID>` / `--conversation=<ID>` are
    // user-authored resume forms: they must survive stripping verbatim.
    for (shell, args) in [
        (
            "agy",
            vec!["-c".to_string(), "-m".to_string(), "gpt-5".to_string()],
        ),
        (
            "agy",
            vec![
                "--conversation".to_string(),
                "abc123".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ],
        ),
        ("agy", vec!["--conversation=abc123".to_string()]),
        (
            "cmd.exe",
            vec![
                "/C".to_string(),
                "agy".to_string(),
                "--conversation".to_string(),
                "abc123".to_string(),
            ],
        ),
    ] {
        assert_eq!(
            strip_auto_injected_args(shell, &args),
            args,
            "shell={shell:?} args={args:?}"
        );
    }
}

#[test]
fn strip_auto_injected_args_antigravity_round_trip_continue() {
    // round-trip: strip(apply(cmd)) == cmd for the injected `--continue`.
    for (shell, injected) in [
        ("agy", vec!["-m".to_string(), "gpt-5".to_string()]),
        (
            "cmd.exe",
            vec![
                "/C".to_string(),
                "agy".to_string(),
                "-m".to_string(),
                "gpt-5".to_string(),
            ],
        ),
    ] {
        let mut applied = injected.clone();
        if shell == "agy" {
            applied.insert(0, "--continue".to_string());
        } else {
            applied.insert(2, "--continue".to_string());
        }
        assert_eq!(
            strip_auto_injected_args(shell, &applied),
            injected,
            "shell={shell:?}"
        );
    }
}

#[test]
fn strip_auto_injected_args_removes_direct_claude_continue() {
    let stripped = strip_auto_injected_args(
        "claude",
        &["--continue".to_string(), "--search".to_string()],
    );
    assert_eq!(stripped, vec!["--search".to_string()]);
}

#[test]
fn strip_auto_injected_args_removes_cmd_claude_continue() {
    let stripped = strip_auto_injected_args(
        "cmd.exe",
        &[
            "/C".to_string(),
            "claude".to_string(),
            "--continue".to_string(),
        ],
    );
    assert_eq!(stripped, vec!["/C".to_string(), "claude".to_string()]);
}

#[test]
fn strip_auto_injected_args_removes_direct_claude_session_id() {
    // (#756) Rider belt: the launcher-minted identity pair is stripped from
    // the saved recipe (a replayed stale --session-id hard-fails the spawn).
    let stripped = strip_auto_injected_args(
        "claude",
        &[
            "--dangerously-skip-permissions".to_string(),
            "--session-id".to_string(),
            "7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f".to_string(),
        ],
    );
    assert_eq!(stripped, vec!["--dangerously-skip-permissions".to_string()]);

    // The joined form is removed too.
    let joined = strip_auto_injected_args(
        "claude",
        &[
            "--session-id=7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f".to_string(),
            "--search".to_string(),
        ],
    );
    assert_eq!(joined, vec!["--search".to_string()]);
}

#[test]
fn strip_auto_injected_args_removes_embedded_cmd_claude_session_id() {
    // (#756) cmd-embedded tail loses BOTH injected flags (--continue and the
    // rider pair).
    let stripped = strip_auto_injected_args(
        "cmd.exe",
        &[
            "/C".to_string(),
            "npx claude --continue --session-id 7f9e4a10-2b3c-4d5e-8f90-1a2b3c4d5e6f".to_string(),
        ],
    );
    assert_eq!(stripped, vec!["/C".to_string(), "npx claude".to_string()]);
}

#[test]
fn strip_auto_injected_args_preserves_session_id_without_uuid_value() {
    // (#756) Pins the UUID gate: a user-authored --session-id with a
    // non-UUID value is never eaten.
    let args = vec!["--session-id".to_string(), "not-a-uuid".to_string()];
    assert_eq!(strip_auto_injected_args("claude", &args), args);
}

#[test]
fn strip_auto_injected_args_preserves_user_authored_direct_claude_prompt_file() {
    let stripped = strip_auto_injected_args(
        "claude",
        &[
            "--append-system-prompt-file".to_string(),
            "C:\\temp\\ctx.md".to_string(),
            "--search".to_string(),
        ],
    );
    assert_eq!(
        stripped,
        vec![
            "--append-system-prompt-file".to_string(),
            "C:\\temp\\ctx.md".to_string(),
            "--search".to_string()
        ]
    );
}

#[test]
fn strip_auto_injected_args_preserves_embedded_claude_prompt_file_with_spaces() {
    let stripped = strip_auto_injected_args(
        "cmd.exe",
        &[
            "/K".to_string(),
            "claude --continue --append-system-prompt-file \"C:\\Program Files\\ctx.md\" --search"
                .to_string(),
        ],
    );
    assert_eq!(
        stripped,
        vec![
            "/K".to_string(),
            "claude --append-system-prompt-file \"C:\\Program Files\\ctx.md\" --search".to_string(),
        ]
    );
}

#[test]
fn strip_auto_injected_args_removes_direct_codex_resume_last() {
    let stripped = strip_auto_injected_args(
        "codex",
        &[
            "resume".to_string(),
            "--last".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ],
    );
    assert_eq!(stripped, vec!["-m".to_string(), "gpt-5".to_string()]);
}

#[test]
fn strip_auto_injected_args_removes_cmd_codex_resume_last() {
    let stripped = strip_auto_injected_args(
        "cmd.exe",
        &[
            "/C".to_string(),
            "codex".to_string(),
            "resume".to_string(),
            "--last".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ],
    );
    assert_eq!(
        stripped,
        vec![
            "/C".to_string(),
            "codex".to_string(),
            "-m".to_string(),
            "gpt-5".to_string(),
        ]
    );
}

#[test]
fn strip_auto_injected_args_removes_embedded_cmd_codex_resume_last() {
    let stripped = strip_auto_injected_args(
        "cmd.exe",
        &[
            "/K".to_string(),
            "git pull && codex resume --last -m gpt-5".to_string(),
        ],
    );
    assert_eq!(
        stripped,
        vec!["/K".to_string(), "git pull && codex -m gpt-5".to_string(),]
    );
}

#[test]
fn strip_auto_injected_args_preserves_all_configured_pi_recipes() {
    let cases = [
        (
            "pi",
            vec![
                "--continue".to_string(),
                "--model".to_string(),
                "claude-sonnet".to_string(),
            ],
        ),
        (
            "cmd.exe",
            vec![
                "/C".to_string(),
                "pi".to_string(),
                "--continue".to_string(),
                "--model".to_string(),
                "codex-model".to_string(),
            ],
        ),
        (
            "cmd.exe",
            vec![
                "/K".to_string(),
                "\"C:\\Program Files\\Pi\\pi.cmd\" --continue --provider gemini".to_string(),
            ],
        ),
    ];

    for (shell, args) in cases {
        assert_eq!(
            strip_auto_injected_args(shell, &args),
            args,
            "shell={shell:?}"
        );
    }
}

#[test]
fn strip_auto_injected_args_pi_model_overlap_never_uses_other_stripper() {
    for args in [
        vec!["--model".to_string(), "claude-sonnet".to_string()],
        vec!["--model".to_string(), "codex-model".to_string()],
        vec!["--provider".to_string(), "gemini-pro".to_string()],
    ] {
        assert_eq!(strip_auto_injected_args("pi", &args), args);
    }
}

#[test]
fn strip_auto_injected_args_leaves_unrelated_commands_unchanged() {
    let args = vec!["-NoLogo".to_string()];
    assert_eq!(strip_auto_injected_args("powershell.exe", &args), args);
}

// ── Issue #186 — wrapper-basename Claude detection in the stripper ──

#[test]
fn strip_auto_injected_args_strips_continue_for_wrapper_basename() {
    // claude-mb invoked directly: `--continue` must be stripped from the
    // saved recipe even though the executable's stem is "claude-mb".
    let stripped = strip_auto_injected_args(
        "claude-mb",
        &[
            "--dangerously-skip-permissions".to_string(),
            "--effort".to_string(),
            "max".to_string(),
            "--continue".to_string(),
        ],
    );
    assert_eq!(
        stripped,
        vec![
            "--dangerously-skip-permissions".to_string(),
            "--effort".to_string(),
            "max".to_string(),
        ]
    );
}

#[test]
fn strip_auto_injected_args_strips_continue_for_cmd_wrapped_basename() {
    // cmd.exe /K claude-mb ... --continue → strip --continue.
    let stripped = strip_auto_injected_args(
        "cmd.exe",
        &[
            "/K".to_string(),
            "claude-mb".to_string(),
            "--effort".to_string(),
            "max".to_string(),
            "--continue".to_string(),
        ],
    );
    assert_eq!(
        stripped,
        vec![
            "/K".to_string(),
            "claude-mb".to_string(),
            "--effort".to_string(),
            "max".to_string(),
        ]
    );
}

#[test]
fn strip_auto_injected_args_strips_continue_for_embedded_cmd_wrapped_basename() {
    // cmd.exe /K "claude-mb --effort max --continue" → strip --continue.
    let stripped = strip_auto_injected_args(
        "cmd.exe",
        &[
            "/K".to_string(),
            "claude-mb --effort max --continue".to_string(),
        ],
    );
    assert_eq!(
        stripped,
        vec!["/K".to_string(), "claude-mb --effort max".to_string(),]
    );
}
