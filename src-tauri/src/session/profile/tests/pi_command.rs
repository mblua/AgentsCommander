use super::*;

#[test]
fn locate_direct_pi_exact_allowlist_and_paths() {
    for shell in [
        "pi",
        "PI",
        "pi.exe",
        "Pi.CmD",
        "/usr/local/bin/pi",
        r"C:\tools\PI.EXE",
        r"\\server\share\pi.cmd",
        r"\\?\C:\Program Files\Pi\pi.exe",
    ] {
        let location = locate_pi_command(shell, &strings(&["--model", "claude-sonnet"]))
            .unwrap()
            .unwrap_or_else(|| panic!("expected Pi location for {shell:?}"));
        assert_eq!(
            location,
            PiCommandLocation {
                option_tokens: strings(&["--model", "claude-sonnet"]),
                insertion: PiInsertionPoint::Arg { index: 0 },
            },
            "shell={shell:?}"
        );
        assert_eq!(
            CodingAgentKind::detect(shell, &strings(&["--model", "claude-sonnet"])),
            Some(CodingAgentKind::Pi)
        );
    }
}

#[test]
fn direct_pi_args_remain_literal_process_arguments() {
    let args = strings(&["&&", "\"", "dangling^", "line\r\nvalue", "工具"]);
    assert_eq!(
        locate_pi_command("pi", &args),
        Ok(Some(PiCommandLocation {
            option_tokens: args,
            insertion: PiInsertionPoint::Arg { index: 0 },
        }))
    );
}

#[test]
fn runtime_shell_value_is_not_trimmed_or_unquoted() {
    for shell in [" pi ", "\"pi\"", r#""C:\tools\pi.cmd""#] {
        assert!(
            !matches!(locate_pi_command(shell, &[]), Ok(Some(_))),
            "shell={shell:?}"
        );
        assert_ne!(
            CodingAgentKind::detect(shell, &[]),
            Some(CodingAgentKind::Pi)
        );
    }

    let normalized = crate::config::agent_command::normalize_legacy_agent_command(
        r#""C:\Program Files\Pi\pi.cmd" --model claude-sonnet"#,
    )
    .unwrap();
    assert_eq!(normalized.shell, r"C:\Program Files\Pi\pi.cmd");
    assert_eq!(
        CodingAgentKind::detect(&normalized.shell, &normalized.shell_args),
        Some(CodingAgentKind::Pi)
    );
}

#[test]
fn reserved_pi_extensions_fail_closed() {
    for shell in ["pi.md", "PI.BAT", r"C:\tools\pi.ps1"] {
        let args = strings(&["--model", "claude-sonnet"]);
        assert_eq!(
            locate_pi_command(shell, &args),
            Err(PiCommandParseError::UnsupportedPiCommandShape)
        );
        assert_eq!(CodingAgentKind::detect(shell, &args), None);
    }
}

#[test]
fn locate_tokenized_cmd_pi_decodes_atoms_and_limits_first_segment() {
    let args = strings(&[
        "/C",
        r#""C:\Program Files\Pi\pi.cmd""#,
        "--model",
        "claude-sonnet",
        "&&",
        "echo",
        "done",
    ]);
    assert_eq!(
        locate_pi_command("CMD.EXE", &args),
        Ok(Some(PiCommandLocation {
            option_tokens: strings(&["--model", "claude-sonnet"]),
            insertion: PiInsertionPoint::Arg { index: 2 },
        }))
    );

    for head in ["pi", "p^i", "\"pi.cmd\""] {
        let args = strings(&["/K", head]);
        assert_eq!(
            locate_pi_command("cmd", &args),
            Ok(Some(PiCommandLocation {
                option_tokens: Vec::new(),
                insertion: PiInsertionPoint::Arg { index: 2 },
            }))
        );
    }

    let escaped = strings(&["/C", "pi", "\"--value&&literal\"", "x^&y", "&&", "echo"]);
    assert_eq!(
        locate_pi_command("cmd", &escaped)
            .unwrap()
            .unwrap()
            .option_tokens,
        strings(&["--value&&literal", "x&y"])
    );
}

#[test]
fn locate_embedded_cmd_pi_reports_exact_utf8_ranges() {
    let text = " \t\"C:\\工具\\pi.cmd\"  --model claude-sonnet  &&echo done";
    let args = vec!["/K".to_string(), text.to_string()];
    let location = locate_pi_command("cmd.exe", &args).unwrap().unwrap();
    assert_eq!(
        location.option_tokens,
        strings(&["--model", "claude-sonnet"])
    );
    let PiInsertionPoint::CmdText {
        arg_index,
        executable_range,
        segment_range,
    } = location.insertion
    else {
        panic!("expected embedded insertion");
    };
    assert_eq!(arg_index, 1);
    assert_eq!(&text[executable_range.clone()], "\"C:\\工具\\pi.cmd\"");
    assert_eq!(segment_range.start, executable_range.start);
    assert_eq!(segment_range.end, text.find("&&").unwrap());
    for endpoint in [
        executable_range.start,
        executable_range.end,
        segment_range.start,
        segment_range.end,
    ] {
        assert!(text.is_char_boundary(endpoint));
    }
    assert_eq!(&text[segment_range.end..], "&&echo done");
}

#[test]
fn embedded_quotes_carets_and_separators_decode_without_rewriting() {
    let text = r#"pi "--resume&&later" x^&y&&echo --continue"#;
    let location = locate_pi_command("cmd", &strings(&["/C", text]))
        .unwrap()
        .unwrap();
    assert_eq!(location.option_tokens, strings(&["--resume&&later", "x&y"]));
    let PiInsertionPoint::CmdText { segment_range, .. } = location.insertion else {
        panic!("expected embedded insertion");
    };
    assert_eq!(&text[segment_range.end..], "&&echo --continue");
}

#[test]
fn pi_identity_precedes_legacy_provider_values() {
    for args in [
        strings(&["--model", "claude-sonnet"]),
        strings(&["--model", "codex-model"]),
        strings(&["--provider", "gemini-pro"]),
    ] {
        assert_eq!(
            CodingAgentKind::detect("pi", &args),
            Some(CodingAgentKind::Pi)
        );
    }
    assert_eq!(
        CodingAgentKind::detect(
            "cmd.exe",
            &strings(&["/C", "pi", "--model", "claude-sonnet"])
        ),
        Some(CodingAgentKind::Pi)
    );
    assert_eq!(
        CodingAgentKind::detect(
            "cmd.exe",
            &strings(&["/C", "pi --provider codex --model gemini-pro"])
        ),
        Some(CodingAgentKind::Pi)
    );
}

#[test]
fn unsupported_pi_positions_and_syntax_fail_closed() {
    let cases = [
        strings(&["/C", "npx", "pi", "--model", "claude-sonnet"]),
        strings(&["/C", "call", "pi", "--provider", "codex"]),
        strings(&["/C", "start", "pi", "gemini-pro"]),
        strings(&["/S", "/C", "pi", "--model", "claude-sonnet"]),
        strings(&["/C", "echo before&&pi --model claude-sonnet"]),
        strings(&["/C", "(", "pi", ")", "--provider", "codex"]),
        strings(&["/C", "(pi) --provider codex"]),
        strings(&["/C", "pi>out --model claude-sonnet"]),
        strings(&["/C", "pi.bat --model claude-sonnet"]),
        strings(&["/C", "echo", "before", "&&", "pi", "gemini-pro"]),
        strings(&["/C", "pi", "--resume&&echo", "claude-sonnet"]),
        strings(&["/C", "pi&&echo", "--model", "claude-sonnet"]),
    ];
    for args in cases {
        assert_eq!(
            locate_pi_command("cmd.exe", &args),
            Err(PiCommandParseError::UnsupportedPiCommandShape),
            "args={args:?}"
        );
        assert_eq!(CodingAgentKind::detect("cmd.exe", &args), None);
    }

    let npx_args = strings(&["pi", "--model", "claude-sonnet"]);
    assert_eq!(
        locate_pi_command("npx.cmd", &npx_args),
        Err(PiCommandParseError::UnsupportedPiCommandShape)
    );
    assert_eq!(CodingAgentKind::detect("npx.cmd", &npx_args), None);
}

#[test]
fn tokenized_attached_later_pi_heads_fail_closed() {
    let providers = [
        ("--model", "claude-sonnet"),
        ("--provider", "codex-model"),
        ("--model", "gemini-pro"),
    ];

    for separator in ["&", "&&", "|", "||"] {
        for executable in ["pi", "pi.md", "pi.bat"] {
            for (provider_flag, provider_value) in providers {
                let cases = [
                    vec![
                        "/C".to_string(),
                        format!("echo{separator}{executable}"),
                        provider_flag.to_string(),
                        provider_value.to_string(),
                    ],
                    vec![
                        "/C".to_string(),
                        format!("echo{separator}"),
                        executable.to_string(),
                        provider_flag.to_string(),
                        provider_value.to_string(),
                    ],
                    vec![
                        "/C".to_string(),
                        "echo".to_string(),
                        format!("{separator}{executable}"),
                        provider_flag.to_string(),
                        provider_value.to_string(),
                    ],
                    vec![
                        "/C".to_string(),
                        "echo".to_string(),
                        format!("value{separator}{executable}"),
                        provider_flag.to_string(),
                        provider_value.to_string(),
                    ],
                ];

                for args in cases {
                    assert_eq!(
                        locate_pi_command("cmd.exe", &args),
                        Err(PiCommandParseError::UnsupportedPiCommandShape),
                        "separator={separator:?} executable={executable:?} args={args:?}"
                    );
                    assert_eq!(
                        CodingAgentKind::detect("cmd.exe", &args),
                        None,
                        "separator={separator:?} executable={executable:?} args={args:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn tokenized_grouped_attached_pi_boundaries_fail_closed() {
    let providers = [
        ("--model", "claude-sonnet"),
        ("--provider", "codex-model"),
        ("--model", "gemini-pro"),
    ];
    let wrappers = [
        "npx",
        "npx.exe",
        "npx.cmd",
        "call",
        "call.exe",
        "call.cmd",
        "start",
        "start.exe",
        "start.cmd",
    ];
    let mut cases = Vec::new();

    for separator in ["&", "&&", "|", "||"] {
        for groups in ["(", "((("] {
            for executable in ["pi", "pi.md", "pi.bat"] {
                for &(provider_flag, provider_value) in &providers {
                    cases.push(vec![
                        "/C".to_string(),
                        format!("echo{separator}{groups}"),
                        executable.to_string(),
                        provider_flag.to_string(),
                        provider_value.to_string(),
                    ]);
                    cases.push(vec![
                        "/C".to_string(),
                        format!("echo{separator}{groups}{executable}"),
                        provider_flag.to_string(),
                        provider_value.to_string(),
                    ]);
                    cases.push(vec![
                        "/C".to_string(),
                        format!("echo{separator}"),
                        groups.to_string(),
                        executable.to_string(),
                        provider_flag.to_string(),
                        provider_value.to_string(),
                    ]);

                    for wrapper in wrappers {
                        cases.push(vec![
                            "/C".to_string(),
                            format!("echo{separator}{groups}{wrapper}"),
                            executable.to_string(),
                            provider_flag.to_string(),
                            provider_value.to_string(),
                        ]);
                        cases.push(vec![
                            "/C".to_string(),
                            format!("echo{separator}{groups}"),
                            wrapper.to_string(),
                            executable.to_string(),
                            provider_flag.to_string(),
                            provider_value.to_string(),
                        ]);
                        cases.push(vec![
                            "/C".to_string(),
                            format!("echo{separator}"),
                            format!("{groups}{wrapper}"),
                            executable.to_string(),
                            provider_flag.to_string(),
                            provider_value.to_string(),
                        ]);
                    }
                }
            }
        }
    }

    for args in cases {
        assert_eq!(
            locate_pi_command("cmd.exe", &args),
            Err(PiCommandParseError::UnsupportedPiCommandShape),
            "args={args:?}"
        );
        assert_eq!(
            CodingAgentKind::detect("cmd.exe", &args),
            None,
            "args={args:?}"
        );
    }
}

#[test]
fn tokenized_attached_later_head_negative_controls_stay_non_pi() {
    let cases = [
        (
            strings(&["/C", "echo", "pi", "--model", "claude-sonnet"]),
            Some(CodingAgentKind::Claude),
        ),
        (
            strings(&["/C", "echo", "pi&&later", "--provider", "codex-model"]),
            Some(CodingAgentKind::Codex),
        ),
        (
            strings(&["/C", "echo", "value^&^&pi", "--model", "gemini-pro"]),
            None,
        ),
        (
            strings(&["/C", "echo&&other", "pi", "--model", "claude-sonnet"]),
            Some(CodingAgentKind::Claude),
        ),
        (
            strings(&["/C", "echo", "\"value&&pi\"", "--model", "claude-sonnet"]),
            Some(CodingAgentKind::Claude),
        ),
    ];

    for (args, expected_kind) in cases {
        assert_eq!(locate_pi_command("cmd.exe", &args), Ok(None));
        assert_eq!(CodingAgentKind::detect("cmd.exe", &args), expected_kind);
    }
}

#[test]
fn tokenized_redirection_targets_stay_out_of_pi_command_positions() {
    let legacy_cases = [
        (vec!["claude"], Some(CodingAgentKind::Claude)),
        (
            vec!["--model", "claude-sonnet"],
            Some(CodingAgentKind::Claude),
        ),
        (
            vec!["--provider", "codex-model"],
            Some(CodingAgentKind::Codex),
        ),
        // `--model gemini-pro` is an inert value now: no kind is detected.
        (vec!["--model", "gemini-pro"], None),
    ];

    for redirect in ["<", ">"] {
        for target in ["pi", "pi.md", "pi.bat"] {
            for (legacy_tokens, expected_kind) in &legacy_cases {
                let mut layouts = vec![
                    vec![redirect.to_string(), target.to_string()],
                    vec![redirect.repeat(2), target.to_string()],
                    vec![format!("2{redirect}"), target.to_string()],
                    vec!["npx".to_string(), redirect.to_string(), target.to_string()],
                    vec!["call".to_string(), redirect.to_string(), target.to_string()],
                    vec![
                        "start".to_string(),
                        redirect.to_string(),
                        target.to_string(),
                    ],
                ];
                for separator in ["&", "&&", "|", "||"] {
                    layouts.push(vec![
                        "echo".to_string(),
                        separator.to_string(),
                        redirect.to_string(),
                        target.to_string(),
                    ]);
                    layouts.push(vec![
                        format!("echo{separator}{redirect}"),
                        target.to_string(),
                    ]);
                }

                for mut layout in layouts {
                    let mut args = vec!["/C".to_string()];
                    args.append(&mut layout);
                    args.extend(legacy_tokens.iter().map(|token| (*token).to_string()));

                    assert_eq!(
                        locate_pi_command("cmd.exe", &args),
                        Ok(None),
                        "redirect={redirect:?} target={target:?} args={args:?}"
                    );
                    assert_eq!(
                        CodingAgentKind::detect("cmd.exe", &args),
                        *expected_kind,
                        "redirect={redirect:?} target={target:?} args={args:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn tokenized_group_and_control_position_transitions_stay_frozen() {
    let non_candidates = [
        (
            strings(&["/C", ")", "pi", "claude"]),
            Some(CodingAgentKind::Claude),
        ),
        (
            strings(&["/C", "><", "pi.md", "--provider", "codex-model"]),
            Some(CodingAgentKind::Codex),
        ),
        (
            strings(&["/C", "echo&&)", "pi.bat", "--model", "gemini-pro"]),
            None,
        ),
        (
            strings(&["/C", "npx", "()", "pi", "claude"]),
            Some(CodingAgentKind::Claude),
        ),
        (
            strings(&["/C", ">", "npx", "pi", "claude"]),
            Some(CodingAgentKind::Claude),
        ),
        (
            strings(&["/C", "echo&&>npx", "pi", "--provider", "codex-model"]),
            Some(CodingAgentKind::Codex),
        ),
        (
            strings(&["/C", "npx", ">", "start", "pi", "--model", "gemini-pro"]),
            None,
        ),
        (
            strings(&["/C", "echo(", "pi", "--model", "claude-sonnet"]),
            Some(CodingAgentKind::Claude),
        ),
        (
            strings(&["/C", "echo(npx", "pi", "--provider", "codex-model"]),
            Some(CodingAgentKind::Codex),
        ),
        (
            strings(&["/C", "echo^&^&(", "pi", "--model", "gemini-pro"]),
            None,
        ),
        (
            strings(&["/C", "echo&&^(", "pi", "--model", "claude-sonnet"]),
            Some(CodingAgentKind::Claude),
        ),
        (
            strings(&["/C", "echo&&\"(\"", "pi", "--provider", "codex-model"]),
            Some(CodingAgentKind::Codex),
        ),
        (
            strings(&["/C", "echo&&(()", "pi", "--model", "gemini-pro"]),
            None,
        ),
        (
            strings(&["/C", "echo&&(<", "pi", "--model", "claude-sonnet"]),
            Some(CodingAgentKind::Claude),
        ),
        (
            strings(&["/C", "echo", "(((", "pi", "--provider", "codex-model"]),
            Some(CodingAgentKind::Codex),
        ),
        (
            strings(&["/C", "npx>pi", "--model", "claude-sonnet"]),
            Some(CodingAgentKind::Claude),
        ),
        (
            strings(&["/C", "echo&&>npx(pi", "--provider", "codex-model"]),
            Some(CodingAgentKind::Codex),
        ),
        (
            strings(&["/C", "echo&&)start(pi", "--model", "gemini-pro"]),
            None,
        ),
    ];
    for (args, expected_kind) in non_candidates {
        assert_eq!(
            locate_pi_command("cmd.exe", &args),
            Ok(None),
            "args={args:?}"
        );
        assert_eq!(
            CodingAgentKind::detect("cmd.exe", &args),
            expected_kind,
            "args={args:?}"
        );
    }

    let candidates = [
        strings(&["/C", "echo&&", "(", "pi", "--model", "claude-sonnet"]),
        strings(&["/C", "npx", "(((", "pi.md", "--provider", "codex-model"]),
        strings(&["/C", ">", "out", "&&", "pi.bat", "--model", "gemini-pro"]),
        strings(&["/C", "echo)&", "pi", "--model", "claude-sonnet"]),
        strings(&["/C", "echo&&npx", "pi", "--provider", "codex-model"]),
    ];
    for args in candidates {
        assert_eq!(
            locate_pi_command("cmd.exe", &args),
            Err(PiCommandParseError::UnsupportedPiCommandShape),
            "args={args:?}"
        );
        assert_eq!(CodingAgentKind::detect("cmd.exe", &args), None);
    }
}

#[test]
fn malformed_cmd_syntax_fails_closed_after_full_validation() {
    let cases = [
        strings(&["/C", "pi", "\"unterminated", "--model", "claude-sonnet"]),
        strings(&["/C", "pi", "dangling^", "codex-model"]),
        strings(&["/C", "echo", "\"unterminated", "claude-sonnet"]),
        strings(&["/C", "pi \"unterminated --model claude-sonnet"]),
        strings(&["/C", "pi dangling^"]),
        vec![
            "/C".to_string(),
            "pi".to_string(),
            "bad\0".to_string(),
            "claude-sonnet".to_string(),
        ],
        vec!["/C".to_string(), "pi\0 --model claude-sonnet".to_string()],
        vec!["/C".to_string(), "pi\r --provider codex".to_string()],
        vec!["/C".to_string(), "pi\n gemini-pro".to_string()],
    ];
    for args in cases {
        assert_eq!(
            locate_pi_command("cmd.exe", &args),
            Err(PiCommandParseError::MalformedCmdSyntax),
            "args={args:?}"
        );
        assert_eq!(CodingAgentKind::detect("cmd.exe", &args), None);
    }
}

#[test]
fn genuine_non_pi_commands_retain_legacy_detection() {
    assert_eq!(
        CodingAgentKind::detect("claude", &strings(&["--model", "pi"])),
        Some(CodingAgentKind::Claude)
    );
    assert_eq!(
        CodingAgentKind::detect("cmd.exe", &strings(&["/C", "claude", "--model", "pi"])),
        Some(CodingAgentKind::Claude)
    );
    assert_eq!(
        CodingAgentKind::detect("cmd.exe", &strings(&["/C", "echo", "pi"])),
        None
    );
    for shell in ["my-pi", "pip", "pipx", "ping", "pixel"] {
        assert_eq!(CodingAgentKind::detect(shell, &[]), None, "shell={shell:?}");
    }
    assert_eq!(
        CodingAgentKind::detect("pip", &strings(&["--model", "claude-sonnet"])),
        Some(CodingAgentKind::Claude)
    );
}

#[test]
fn embedded_redirection_and_environment_indirection_follow_contract() {
    let text = "pi  > out &&echo done";
    let location = locate_pi_command("cmd", &strings(&["/C", text]))
        .unwrap()
        .unwrap();
    assert_eq!(location.option_tokens, strings(&[">", "out"]));
    assert_eq!(
        locate_pi_command("cmd", &strings(&["/C", "%PI_EXE% --model claude-sonnet"])),
        Ok(None)
    );
    assert_eq!(
        locate_pi_command("cmd", &strings(&["/C", "pi>out"])),
        Err(PiCommandParseError::UnsupportedPiCommandShape)
    );
}

#[test]
fn pi_first_compound_is_supported_and_later_selector_is_not_an_option() {
    for args in [
        strings(&["/C", "pi", "--model", "x", "&&", "echo", "--resume"]),
        strings(&["/C", "pi --model x&&echo --resume"]),
    ] {
        let location = locate_pi_command("cmd.exe", &args).unwrap().unwrap();
        assert_eq!(location.option_tokens, strings(&["--model", "x"]));
    }
}

#[test]
fn legacy_cmd_detection_and_precedence_remain_unchanged_after_ok_none() {
    assert_eq!(
        CodingAgentKind::detect("cmd.exe", &strings(&["/C", "codex"])),
        Some(CodingAgentKind::Codex)
    );
    // A user-defined `gemini` command now gets generic behavior (no kind).
    assert_eq!(
        CodingAgentKind::detect(
            "cmd.exe",
            &strings(&["/K", "git pull && gemini --resume latest"])
        ),
        None
    );
    assert_eq!(
        CodingAgentKind::detect("cmd.exe", &strings(&["/K", "codex && claude"])),
        Some(CodingAgentKind::Claude)
    );
}

#[test]
fn adversarial_cmd_corpus_never_panics_and_ranges_are_valid() {
    let fragments = [
        "",
        " ",
        "\t",
        "\"",
        "^",
        "^^",
        "&",
        "&&",
        "|",
        "||",
        "<",
        ">",
        "(",
        ")",
        "λ",
        "工具",
        "\0",
        "\r",
        "\n",
        "\"quoted\"",
    ];
    for left in fragments {
        for right in fragments {
            let text = format!("{left}pi{right} --model claude-sonnet&&echo λ");
            let args = vec!["/C".to_string(), text.clone()];
            if let Ok(Some(location)) = locate_pi_command("cmd.exe", &args) {
                if let PiInsertionPoint::CmdText {
                    executable_range,
                    segment_range,
                    ..
                } = location.insertion
                {
                    assert!(executable_range.start <= executable_range.end);
                    assert!(executable_range.end <= segment_range.end);
                    assert_eq!(segment_range.start, executable_range.start);
                    assert!(segment_range.end <= text.len());
                    for endpoint in [
                        executable_range.start,
                        executable_range.end,
                        segment_range.start,
                        segment_range.end,
                    ] {
                        assert!(text.is_char_boundary(endpoint));
                    }
                }
            }
        }
    }
}
