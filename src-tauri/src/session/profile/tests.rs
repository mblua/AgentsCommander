use super::*;

mod pi_command;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

#[test]
fn detect_direct_claude_and_wrapper_basename() {
    assert_eq!(
        CodingAgentKind::detect("claude", &[]),
        Some(CodingAgentKind::Claude)
    );
    assert_eq!(
        CodingAgentKind::detect("claude-mb", &["--effort".into(), "max".into()]),
        Some(CodingAgentKind::Claude)
    );
}

#[test]
fn detect_codex_and_antigravity_direct() {
    assert_eq!(
        CodingAgentKind::detect("codex", &[]),
        Some(CodingAgentKind::Codex)
    );
    assert_eq!(
        CodingAgentKind::detect("agy", &[]),
        Some(CodingAgentKind::Antigravity)
    );
    assert_eq!(
        CodingAgentKind::detect("antigravity", &[]),
        Some(CodingAgentKind::Antigravity)
    );
    assert_eq!(
        CodingAgentKind::detect("agy.exe", &[]),
        Some(CodingAgentKind::Antigravity)
    );
    assert_eq!(
        CodingAgentKind::detect("agy.cmd", &["-m".into(), "gpt-5".into()]),
        Some(CodingAgentKind::Antigravity)
    );
}

#[test]
fn detect_antigravity_stems_and_wrapper_exclusions() {
    for shell in [
        "agy",
        "agy.exe",
        "agy.cmd",
        "antigravity",
        r"C:\tools\agy.exe",
    ] {
        assert_eq!(
            CodingAgentKind::detect(shell, &[]),
            Some(CodingAgentKind::Antigravity),
            "shell={shell:?}"
        );
    }
    // Tokenized cmd form.
    assert_eq!(
        CodingAgentKind::detect("cmd.exe", &strings(&["/C", "agy", "--effort", "high"])),
        Some(CodingAgentKind::Antigravity)
    );
    // Exact-stem only: prefix wrappers are NOT inferred.
    for shell in [
        "agy-proxy",
        "my-agy",
        "antigravity-pro",
        "agyctl",
        "antigravityctl",
    ] {
        assert_eq!(CodingAgentKind::detect(shell, &[]), None, "shell={shell:?}");
    }
    // `--model antigravity` as a VALUE for claude still detects Claude
    // (prefix precedence wins); a gemini* executable is no longer a kind.
    assert_eq!(
        CodingAgentKind::detect("claude", &strings(&["--model", "antigravity"])),
        Some(CodingAgentKind::Claude)
    );
    for shell in ["pip", "pixel"] {
        assert_eq!(CodingAgentKind::detect(shell, &[]), None, "shell={shell:?}");
    }
    for shell in ["gemini", "gemini-bar"] {
        assert_eq!(
            CodingAgentKind::detect(shell, &["-m".into(), "gpt-5".into()]),
            None,
            "shell={shell:?} now behaves as a plain unknown command"
        );
    }
}

#[test]
fn antigravity_serde_profile_contract() {
    assert_eq!(
        serde_json::to_string(&CodingAgentKind::Antigravity).unwrap(),
        "\"antigravity\""
    );
    assert_eq!(CodingAgentKind::Antigravity.as_str(), "agy");
    let profile = CodingAgentKind::Antigravity.profile();
    assert_eq!(profile.kind, CodingAgentKind::Antigravity);
    assert_eq!(profile.idle, IdleTuning::AGENT_DEFAULT);
    assert_eq!(profile.resume_tokens, ["--continue"]);
    assert!(profile.container_credential.is_none());
    assert!(profile.auto_self_clear_supported);
}

#[test]
fn pi_serde_and_profile_contract_are_stable() {
    assert_eq!(
        serde_json::to_string(&CodingAgentKind::Pi).unwrap(),
        "\"pi\""
    );
    assert_eq!(CodingAgentKind::Pi.as_str(), "pi");
    let profile = CodingAgentKind::Pi.profile();
    assert_eq!(profile.kind, CodingAgentKind::Pi);
    assert_eq!(profile.idle, IdleTuning::AGENT_DEFAULT);
    assert_eq!(profile.resume_tokens, ["--continue"]);
    assert!(profile.container_credential.is_none());
    assert!(!profile.auto_self_clear_supported);
    for kind in [
        CodingAgentKind::Claude,
        CodingAgentKind::Codex,
        CodingAgentKind::Antigravity,
    ] {
        assert!(kind.profile().auto_self_clear_supported);
    }
}

/// #1873 - Muse is a serde-stable first-class kind whose identity is a direct
/// shell with the exact stem `muse`; it never becomes a logical PTY
/// submission agent and its profile carries the exact workspace-latest tokens.
#[test]
fn muse_serde_profile_detection_and_submission_boundary_are_stable() {
    // Serde/as_str wire value.
    assert_eq!(
        serde_json::to_string(&CodingAgentKind::Muse).unwrap(),
        "\"muse\""
    );
    assert_eq!(
        serde_json::from_str::<CodingAgentKind>("\"muse\"").unwrap(),
        CodingAgentKind::Muse
    );
    assert_eq!(CodingAgentKind::Muse.as_str(), "muse");

    // Exact profile.
    let profile = CodingAgentKind::Muse.profile();
    assert_eq!(profile.kind, CodingAgentKind::Muse);
    assert_eq!(profile.idle, IdleTuning::AGENT_DEFAULT);
    assert_eq!(profile.resume_tokens, ["resume", "--last"]);
    assert!(profile.container_credential.is_none());
    assert!(!profile.auto_self_clear_supported);
    assert_eq!(
        idle_tuning_for(Some(CodingAgentKind::Muse)),
        IdleTuning::AGENT_DEFAULT
    );

    // Direct positives: bare, absolute (both separators), and an exe stem.
    let codex_looking = vec![
        "--workspace".to_string(),
        "/tmp/codex".to_string(),
        "resume".to_string(),
        "--last".to_string(),
    ];
    for shell in ["muse", "/opt/muse/bin/muse", "/usr/local/bin/muse"] {
        assert_eq!(
            CodingAgentKind::detect(shell, &[]),
            Some(CodingAgentKind::Muse),
            "shell={shell:?}"
        );
        assert_eq!(
            CodingAgentKind::detect(shell, &codex_looking),
            Some(CodingAgentKind::Muse),
            "direct Muse outranks a Codex-looking argument value, shell={shell:?}"
        );
        assert_eq!(
            CodingAgentKind::detect(shell, &["--provider".into(), "claude".into()]),
            Some(CodingAgentKind::Muse),
            "shell={shell:?}"
        );
    }
    assert_eq!(
        CodingAgentKind::detect("muse.exe", &[]),
        Some(CodingAgentKind::Muse)
    );

    // Negatives: prefix names, mixed case, and argument-only identity inside
    // env/shell/PowerShell/cmd/compound wrappers.
    for shell in ["muse-agent", "musee", "Muse", "MUSE", "amuse", "muse2"] {
        assert_ne!(
            CodingAgentKind::detect(shell, &[]),
            Some(CodingAgentKind::Muse),
            "shell={shell:?}"
        );
    }
    let muse_args = vec![
        "muse".to_string(),
        "resume".to_string(),
        "--last".to_string(),
    ];
    for shell in ["env", "bash", "sh", "pwsh", "powershell.exe", "zsh"] {
        assert_eq!(
            CodingAgentKind::detect(shell, &muse_args),
            None,
            "wrapper shell={shell:?}"
        );
    }
    assert_eq!(
        CodingAgentKind::detect(
            "bash",
            &["-lc".to_string(), "muse resume --last".to_string()]
        ),
        None
    );
    assert_eq!(
        CodingAgentKind::detect(
            "cmd.exe",
            &["/C".to_string(), "muse".to_string(), "resume".to_string()]
        ),
        None
    );
    assert_eq!(
        CodingAgentKind::detect(
            "cmd",
            &[
                "/K".to_string(),
                "git pull && muse resume --last".to_string()
            ]
        ),
        None
    );
    // A wrapper with a Codex-looking value keeps the legacy Codex detection,
    // never Muse.
    assert_eq!(
        CodingAgentKind::detect(
            "env",
            &[
                "muse".to_string(),
                "--workspace".to_string(),
                "/tmp/codex".to_string(),
                "resume".to_string(),
                "--last".to_string(),
            ]
        ),
        Some(CodingAgentKind::Codex)
    );

    // Prior-provider precedence is untouched.
    assert_eq!(
        CodingAgentKind::detect("claude", &["muse".into()]),
        Some(CodingAgentKind::Claude)
    );
    assert_eq!(
        CodingAgentKind::detect("pi", &["--provider".into(), "muse".into()]),
        Some(CodingAgentKind::Pi)
    );

    // Muse is outside logical/privileged PTY submission on every shape.
    for shell in ["muse", "/opt/muse/bin/muse"] {
        for configured_wrapper in [false, true] {
            assert_eq!(
                detect_pty_submission_agent(shell, &[], Some(CodingAgentKind::Muse)),
                None,
                "shell={shell:?}"
            );
            assert_eq!(
                detect_pty_submission_agent_with_provenance(
                    shell,
                    &[],
                    Some(CodingAgentKind::Muse),
                    configured_wrapper
                ),
                None,
                "shell={shell:?} configured_wrapper={configured_wrapper}"
            );
        }
    }
    assert_eq!(
        detect_pty_submission_agent("claude", &[], Some(CodingAgentKind::Muse)),
        None,
        "a Muse hint never agrees with another provider's executable"
    );
}

#[test]
fn detect_plain_shell_and_literal_space_path_are_none() {
    assert_eq!(
        CodingAgentKind::detect("powershell.exe", &["-NoLogo".into()]),
        None
    );
    assert_eq!(CodingAgentKind::detect("cmd.exe", &[]), None);
}

#[test]
fn privileged_detector_requires_executable_position() {
    assert_eq!(
        detect_pty_submission_agent("claude.exe", &[], Some(CodingAgentKind::Claude)),
        Some(PtySubmissionAgent::Claude)
    );
    assert_eq!(
        detect_pty_submission_agent(
            "cmd.exe",
            &["/C".into(), "codex".into(), "--quiet".into()],
            Some(CodingAgentKind::Codex)
        ),
        Some(PtySubmissionAgent::Codex)
    );
    assert_eq!(
        detect_pty_submission_agent("agent.exe", &[], None),
        Some(PtySubmissionAgent::CursorAgent)
    );
    assert_eq!(detect_pty_submission_agent("agentctl", &[], None), None);
    assert_eq!(
        detect_pty_submission_agent(
            "bash",
            &["-c".into(), "echo claude".into()],
            Some(CodingAgentKind::Claude)
        ),
        None
    );
}

#[test]
fn privileged_detector_antigravity_exact_stems() {
    assert_eq!(
        detect_pty_submission_agent("agy", &[], Some(CodingAgentKind::Antigravity)),
        Some(PtySubmissionAgent::Antigravity)
    );
    assert_eq!(
        detect_pty_submission_agent("antigravity.exe", &[], Some(CodingAgentKind::Antigravity)),
        Some(PtySubmissionAgent::Antigravity)
    );
    assert_eq!(
        detect_pty_submission_agent(
            "cmd.exe",
            &["/C".into(), "agy".into(), "--effort".into(), "high".into()],
            Some(CodingAgentKind::Antigravity)
        ),
        Some(PtySubmissionAgent::Antigravity)
    );
    // Exact stems only — no prefix wrappers, and /K is rejected.
    assert_eq!(
        detect_pty_submission_agent("agyctl", &[], Some(CodingAgentKind::Antigravity)),
        None
    );
    assert_eq!(
        detect_pty_submission_agent(
            "cmd.exe",
            &["/K".into(), "agy".into()],
            Some(CodingAgentKind::Antigravity)
        ),
        None
    );
}

#[test]
fn privileged_detector_rejects_unconfigured_prefix_named_plain_shells() {
    assert_eq!(
        detect_pty_submission_agent("claudette.exe", &[], Some(CodingAgentKind::Claude),),
        None
    );
    let wrapper_args = ["/C".into(), "codex-shell.exe".into()];
    assert_eq!(
        detect_pty_submission_agent("cmd.exe", &wrapper_args, Some(CodingAgentKind::Codex),),
        None
    );
    assert_eq!(
        detect_configured_pty_submission_agent(
            "cmd.exe",
            &wrapper_args,
            Some(CodingAgentKind::Codex),
        ),
        Some(PtySubmissionAgent::Codex)
    );
}

#[test]
fn privileged_detector_rejects_persistent_or_compound_cmd() {
    for args in [
        vec!["/K".into(), "codex".into()],
        vec!["/C".into(), "codex && whoami".into()],
        vec!["/C".into(), "CALL codex".into()],
        vec!["/C".into(), "codex %EXTRA%".into()],
    ] {
        assert_eq!(
            detect_pty_submission_agent("cmd.exe", &args, Some(CodingAgentKind::Codex)),
            None,
            "args={args:?}"
        );
    }
}

#[test]
fn detect_strips_known_exe_extension() {
    // `file_stem` drops the `.exe` suffix the basename match relies on
    // (dev-rust R1.5).
    assert_eq!(
        CodingAgentKind::detect("claude.exe", &[]),
        Some(CodingAgentKind::Claude)
    );
}

#[test]
fn detect_space_in_shell_path_treats_shell_as_one_token() {
    // #260 G3 — `detect` treats `shell` as a SINGLE token; it does NOT
    // whitespace-split it the way pre-#260 `create_session_inner` split
    // the joined command string. A space-containing shell path whose real
    // executable is not an agent therefore resolves to `None` (the more
    // correct result — the executable here is `runner.exe`).
    assert_eq!(
        CodingAgentKind::detect("C:\\codex tools\\runner.exe", &[]),
        None
    );
}

#[test]
fn detect_strips_known_legacy_exe_extension() {
    assert_eq!(
        CodingAgentKind::detect("claude.exe", &[]),
        Some(CodingAgentKind::Claude)
    );
}

#[test]
fn idle_tuning_for_launch_resolves_burst_subfields() {
    // #2156 - unrecognised-agent OFF contract.
    assert_eq!(idle_tuning_for_launch(None, None), IdleTuning::DEFAULT);
    assert_eq!(idle_tuning_for_launch(None, None).burst_max_bytes, 0);
    assert_eq!(
        idle_tuning_for_launch(Some(CodingAgentKind::Claude), None),
        IdleTuning::AGENT_DEFAULT,
        "#2156 - no catalog object means no override, the filter stays ON"
    );
    assert_eq!(
        idle_tuning_for_launch(Some(CodingAgentKind::Claude), None).burst_max_bytes,
        1024
    );

    let defaults = idle_tuning_for_launch(Some(CodingAgentKind::Claude), Some((None, None, None)));
    assert_eq!(defaults.burst_max_bytes, 1024);
    assert_eq!(defaults.burst_window, Duration::from_secs(3));
    assert_eq!(defaults.burst_prior_silence, Duration::from_secs(60));

    let unkinded = idle_tuning_for_launch(None, Some((None, None, None)));
    assert_eq!(unkinded.burst_max_bytes, 1024);
    assert_eq!(unkinded.idle_threshold, IdleTuning::DEFAULT.idle_threshold);

    let non_finite_and_negative = idle_tuning_for_launch(
        Some(CodingAgentKind::Codex),
        Some((None, Some(-1.0), Some(f64::NAN))),
    );
    assert_eq!(non_finite_and_negative.burst_window, Duration::from_secs(3));
    assert_eq!(
        non_finite_and_negative.burst_prior_silence,
        Duration::from_secs(60)
    );

    let disabled =
        idle_tuning_for_launch(Some(CodingAgentKind::Claude), Some((Some(0), None, None)));
    assert_eq!(disabled.burst_max_bytes, 0);

    let zero_window =
        idle_tuning_for_launch(Some(CodingAgentKind::Claude), Some((None, Some(0.0), None)));
    assert_eq!(zero_window.burst_window, Duration::ZERO);

    let capped = idle_tuning_for_launch(
        Some(CodingAgentKind::Claude),
        Some((None, Some(1e9), Some(1e9))),
    );
    assert_eq!(capped.burst_window, Duration::from_secs(600));
    assert_eq!(capped.burst_prior_silence, Duration::from_secs(86_400));

    let kind = Some(CodingAgentKind::Pi);
    let resolved = idle_tuning_for_launch(kind, Some((Some(2048), Some(2.0), Some(30.0))));
    assert_eq!(
        resolved.idle_threshold,
        idle_tuning_for(kind).idle_threshold
    );
    assert_eq!(resolved.resize_grace, idle_tuning_for(kind).resize_grace);
    assert_eq!(
        resolved.seed_initial_activity,
        idle_tuning_for(kind).seed_initial_activity
    );
}

#[test]
fn idle_profiles_keep_existing_defaults() {
    assert_eq!(idle_tuning_for(None), IdleTuning::DEFAULT);
    for kind in [
        CodingAgentKind::Claude,
        CodingAgentKind::Codex,
        CodingAgentKind::Antigravity,
        CodingAgentKind::Pi,
        CodingAgentKind::Muse,
    ] {
        assert!(kind.profile().idle.seed_initial_activity);
        // #2879 - Codex differs from AGENT_DEFAULT only in its detector selector.
        let idle = IdleTuning {
            activity_detector: ActivityDetectorKind::Timed,
            ..kind.profile().idle
        };
        assert_eq!(idle, IdleTuning::AGENT_DEFAULT);
    }
    assert!(idle_tuning_for(None).seed_initial_activity);
    assert_eq!(
        IdleTuning::AGENT_DEFAULT.idle_threshold,
        IdleTuning::DEFAULT.idle_threshold
    );
    assert_eq!(
        IdleTuning::AGENT_DEFAULT.resize_grace,
        IdleTuning::DEFAULT.resize_grace
    );
    assert_eq!(
        IdleTuning::AGENT_DEFAULT.seed_initial_activity,
        IdleTuning::DEFAULT.seed_initial_activity
    );
}

/// #2156 - guards the const-literal durations against the
/// `IDLE_BURST_DEFAULT_*` constants they duplicate.
#[test]
fn agent_default_burst_matches_the_documented_constants() {
    assert_eq!(
        IdleTuning::AGENT_DEFAULT.burst_max_bytes,
        IDLE_BURST_DEFAULT_MAX_BYTES
    );
    assert_eq!(
        IdleTuning::AGENT_DEFAULT.burst_window,
        Duration::from_secs_f64(IDLE_BURST_DEFAULT_MAX_SECS)
    );
    assert_eq!(
        IdleTuning::AGENT_DEFAULT.burst_prior_silence,
        Duration::from_secs_f64(IDLE_BURST_DEFAULT_PRIOR_SILENCE_SECS)
    );
}

/// #2879 - only an effective Codex launch selects `CodexWorking`; every other agent, a
/// plain shell and `DEFAULT` stay `Timed`, and burst launch overrides keep the selector.
#[test]
fn i2879_feature_selection() {
    assert_eq!(
        CodingAgentKind::Codex.profile().idle.activity_detector,
        ActivityDetectorKind::CodexWorking
    );
    assert_eq!(
        idle_tuning_for(Some(CodingAgentKind::Codex)).activity_detector,
        ActivityDetectorKind::CodexWorking
    );
    for kind in [
        CodingAgentKind::Claude,
        CodingAgentKind::Antigravity,
        CodingAgentKind::Pi,
        CodingAgentKind::Muse,
    ] {
        assert_eq!(
            idle_tuning_for(Some(kind)).activity_detector,
            ActivityDetectorKind::Timed,
            "{kind:?}"
        );
        assert_eq!(
            idle_tuning_for_launch(Some(kind), Some((Some(7), Some(1.0), Some(2.0))))
                .activity_detector,
            ActivityDetectorKind::Timed,
            "{kind:?}"
        );
    }
    assert_eq!(
        idle_tuning_for(None).activity_detector,
        ActivityDetectorKind::Timed
    );
    assert_eq!(
        IdleTuning::DEFAULT.activity_detector,
        ActivityDetectorKind::Timed
    );
    assert_eq!(
        IdleTuning::AGENT_DEFAULT.activity_detector,
        ActivityDetectorKind::Timed
    );
    // Every launch-override shape keeps the Codex selector AND still applies the burst
    // override it was given (the control: the override is not silently ignored).
    for burst in [
        None,
        Some((None, None, None)),
        Some((Some(0), Some(0.0), Some(0.0))),
        Some((Some(2048), Some(2.0), Some(30.0))),
    ] {
        let resolved = idle_tuning_for_launch(Some(CodingAgentKind::Codex), burst);
        assert_eq!(
            resolved.activity_detector,
            ActivityDetectorKind::CodexWorking,
            "{burst:?}"
        );
        if let Some((Some(bytes), _, _)) = burst {
            assert_eq!(resolved.burst_max_bytes, bytes, "{burst:?}");
        }
    }

    // A Timed selection never yields a semantic consumer; the Codex one does.
    let detector = crate::pty::idle_detector::IdleDetector::new(|_| {}, |_| {});
    let (claude, shell, codex) = (
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
    );
    detector.register_session(claude, idle_tuning_for(Some(CodingAgentKind::Claude)));
    detector.register_session(shell, idle_tuning_for(None));
    detector.register_session(
        codex,
        idle_tuning_for_launch(Some(CodingAgentKind::Codex), Some((None, None, None))),
    );
    assert!(detector.activity_consumer(claude).is_none());
    assert!(detector.activity_consumer(shell).is_none());
    assert!(detector.activity_consumer(codex).is_some());
}
