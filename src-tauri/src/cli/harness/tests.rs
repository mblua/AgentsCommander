use super::*;

#[test]
fn denies_destructive_unix_root_removal() {
    let input = PolicyInput {
        raw_display: display_argv(&["rm".into(), "-rf".into(), "/".into()]),
        argv: vec!["rm".into(), "-rf".into(), "/".into()],
        raw_shell: false,
    };
    assert!(matches!(evaluate_policy(&input), PolicyDecision::Deny(_)));
}

#[test]
fn denies_destructive_windows_forms() {
    let inputs = [
        vec!["Remove-Item", "-Recurse", "-Force", "C:\\"],
        vec!["rd", "/s", "/q", "C:\\"],
        vec!["del", "/s", "/q", "C:\\"],
    ];
    for argv in inputs {
        let argv: Vec<String> = argv.into_iter().map(str::to_string).collect();
        let input = PolicyInput {
            raw_display: display_argv(&argv),
            argv,
            raw_shell: false,
        };
        assert!(matches!(evaluate_policy(&input), PolicyDecision::Deny(_)));
    }
}

#[test]
fn raw_chain_destructive_command_is_denied() {
    let input = PolicyInput {
        raw_display: "echo ok && rm -rf /".to_string(),
        argv: Vec::new(),
        raw_shell: true,
    };
    assert!(matches!(evaluate_policy(&input), PolicyDecision::Deny(_)));
}

#[test]
fn raw_destructive_unix_flag_variants_are_denied() {
    for raw in ["rm -fr /", "rm -r -f /"] {
        let input = PolicyInput {
            raw_display: raw.to_string(),
            argv: Vec::new(),
            raw_shell: true,
        };
        assert!(matches!(evaluate_policy(&input), PolicyDecision::Deny(_)));
    }
}

#[test]
fn raw_destructive_windows_flag_variants_are_denied() {
    for raw in ["rd /q /s C:\\", "Remove-Item -r -fo C:\\"] {
        let input = PolicyInput {
            raw_display: raw.to_string(),
            argv: Vec::new(),
            raw_shell: true,
        };
        assert!(matches!(evaluate_policy(&input), PolicyDecision::Deny(_)));
    }
}

#[test]
fn raw_destructive_chained_variants_are_denied() {
    for raw in [
        "echo ok && rm -fr /",
        "echo ok; rd /q /s C:\\",
        "echo ok\nrm -fr /",
    ] {
        let input = PolicyInput {
            raw_display: raw.to_string(),
            argv: Vec::new(),
            raw_shell: true,
        };
        assert!(matches!(evaluate_policy(&input), PolicyDecision::Deny(_)));
    }
}

#[test]
fn branch_guardrail_warns_without_denying() {
    let input = PolicyInput {
        raw_display: "git checkout -b bad branch".to_string(),
        argv: vec![
            "git".into(),
            "checkout".into(),
            "-b".into(),
            "bad branch".into(),
        ],
        raw_shell: false,
    };
    assert!(matches!(evaluate_policy(&input), PolicyDecision::Warn(_)));
}

#[test]
fn nested_shell_warns() {
    let input = PolicyInput {
        raw_display: "pwsh -Command echo hi".to_string(),
        argv: vec!["pwsh".into(), "-Command".into(), "echo hi".into()],
        raw_shell: false,
    };
    assert!(matches!(evaluate_policy(&input), PolicyDecision::Warn(_)));
}

#[test]
fn argv_display_preserves_argument_boundaries() {
    let argv = vec!["echo".to_string(), "a b".to_string(), "c\"d".to_string()];
    assert_eq!(display_argv(&argv), "echo\u{1f}a b\u{1f}c\"d");
}

#[test]
fn redacts_secret_patterns() {
    let redacted = redact_secrets(
        "curl -H Authorization=Bearer abc --token secret _authToken=def password=hunter2",
    );
    assert!(redacted.contains("Authorization=[REDACTED]"));
    assert!(redacted.contains("--token [REDACTED]"));
    assert!(redacted.contains("_authToken=[REDACTED]"));
    assert!(redacted.contains("password=[REDACTED]"));
    assert!(!redacted.contains("abc"));
    assert!(!redacted.contains("hunter2"));
    assert!(!redacted.contains("secret _authToken"));
}

#[test]
fn redacts_argv_secret_flag_value_before_display_join() {
    let input = PolicyInput {
        raw_display: display_argv(&[
            "echo".to_string(),
            "--token".to_string(),
            "secret-leak-check".to_string(),
        ]),
        argv: vec![
            "echo".to_string(),
            "--token".to_string(),
            "secret-leak-check".to_string(),
        ],
        raw_shell: false,
    };
    let redacted = redacted_display(&input);
    assert!(redacted.contains("--token\u{1f}[REDACTED]"));
    assert!(!redacted.contains("secret-leak-check"));
}

#[test]
fn audit_append_preserves_two_records_with_one_newline_each() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs").join("nested");
    crate::filesystem::directories::ensure_harness_logs(&logs).unwrap();
    let path = logs.join("harness.log");
    crate::filesystem::documents::append_harness_audit(&path, "first record").unwrap();
    crate::filesystem::documents::append_harness_audit(&path, "second record").unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"first record\nsecond record\n"
    );
}
