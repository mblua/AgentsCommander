use super::*;

#[test]
fn is_root_agent_target_recognizes_canonical_uri() {
    assert!(is_root_agent_target(ROOT_AGENT_SENDER));
    assert!(is_root_agent_target("agentscommander://root-agent"));
}

#[test]
fn is_root_agent_target_rejects_partial_or_wrong_uris() {
    assert!(!is_root_agent_target(""));
    assert!(!is_root_agent_target("agentscommander://root"));
    assert!(!is_root_agent_target("root-agent"));
    assert!(!is_root_agent_target("agentscommander/root-agent"));
    assert!(!is_root_agent_target("agentscommander://ROOT-AGENT"));
}

/// #2816 (C3) - raw JSON of the state file beside the tracked `config`.
fn state_beside(config: &Path) -> Value {
    let path = config.with_file_name(crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME);
    serde_json::from_str(&std::fs::read_to_string(path).expect("read state file"))
        .expect("parse state file")
}

#[test]
fn merge_root_agent_config_preserves_tooling_and_unknown_fields() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("config.json");
    std::fs::write(
        &config_path,
        r#"{
  "tooling": {
"lastCodingAgent": "codex",
"codingAgents": {"codex": {"app": "Codex"}},
"telegramBot": "ops"
  },
  "unknown": {"keep": true},
  "context": []
}"#,
    )
    .expect("write config");

    merge_root_agent_config(&config_path).expect("merge config");

    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read config"))
            .expect("parse config");
    let state = state_beside(&config_path);
    assert_eq!(state["tooling"]["lastCodingAgent"], "codex");
    assert_eq!(state["tooling"]["codingAgents"]["codex"]["app"], "Codex");
    assert!(config["tooling"].get("lastCodingAgent").is_none());
    assert!(config["tooling"].get("codingAgents").is_none());
    assert_eq!(config["tooling"]["telegramBot"], "ops");
    assert_eq!(config["unknown"]["keep"], true);
    assert_eq!(
        config["context"],
        serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT)
    );
}

#[test]
fn merge_root_agent_config_migrates_old_default_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("config.json");
    std::fs::write(
        &config_path,
        r#"{"tooling":{"lastCodingAgent":"codex"},"context":["$AGENTSCOMMANDER_CONTEXT","Role.md"]}"#,
    )
    .expect("write config");

    merge_root_agent_config(&config_path).expect("merge config");

    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read config"))
            .expect("parse config");
    assert_eq!(
        state_beside(&config_path)["tooling"]["lastCodingAgent"],
        "codex"
    );
    assert!(config["tooling"].get("lastCodingAgent").is_none());
    assert_eq!(
        config["context"],
        serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT)
    );
}

#[test]
fn merge_root_agent_config_strips_the_token_and_preserves_custom_context() {
    // #979: the sentinel is removed from a custom array; every other entry keeps
    // its value and its position.
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("config.json");
    std::fs::write(
        &config_path,
        r#"{"context":["$AGENTSCOMMANDER_CONTEXT","custom.md","Role.md"]}"#,
    )
    .expect("write config");

    merge_root_agent_config(&config_path).expect("merge config");

    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).expect("read config"))
            .expect("parse config");
    assert_eq!(
        config["context"],
        serde_json::json!(["custom.md", "Role.md"])
    );
}

#[test]
fn merge_root_agent_config_migration_table() {
    // #979 4.2.C. Every row: no exact token survives, other values and their order
    // survive, `tooling` and unknown fields survive, and a SECOND merge yields an
    // identical parsed value AND identical bytes (semantic + byte-stable
    // idempotence; `update_agent_config` publishes only a changed side, and mtime
    // and inode are still deliberately not asserted).
    let token = crate::config::session_context::CONTEXT_TOKEN_GLOBAL;
    let default_context = serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT);
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "missing context",
            serde_json::json!({"tooling":{"lastCodingAgent":"codex"},"unknown":{"keep":true}}),
            default_context.clone(),
        ),
        (
            "empty context",
            serde_json::json!({"context":[],"unknown":{"keep":true}}),
            default_context.clone(),
        ),
        (
            "null context",
            serde_json::json!({"context":Value::Null,"unknown":{"keep":true}}),
            default_context.clone(),
        ),
        (
            "object context",
            serde_json::json!({"context":{"nope":1},"unknown":{"keep":true}}),
            default_context.clone(),
        ),
        (
            "pre-979 default",
            serde_json::json!({"context":[token,"../Context.root-agent.md","Role.md"],"unknown":{"keep":true}}),
            default_context.clone(),
        ),
        (
            "older default without the supplement",
            serde_json::json!({"context":[token,"Role.md"],"unknown":{"keep":true}}),
            default_context.clone(),
        ),
        (
            "token at the beginning",
            serde_json::json!({"context":[token,"a.md","b.md"],"unknown":{"keep":true}}),
            serde_json::json!(["a.md", "b.md"]),
        ),
        (
            "token in the middle",
            serde_json::json!({"context":["a.md",token,"b.md"],"unknown":{"keep":true}}),
            serde_json::json!(["a.md", "b.md"]),
        ),
        (
            "token at the end",
            serde_json::json!({"context":["a.md","b.md",token],"unknown":{"keep":true}}),
            serde_json::json!(["a.md", "b.md"]),
        ),
        (
            "repeated token",
            serde_json::json!({"context":[token,"a.md",token,"b.md",token],"unknown":{"keep":true}}),
            serde_json::json!(["a.md", "b.md"]),
        ),
        (
            "non-string values interleaved",
            serde_json::json!({"context":["a.md",42,token,Value::Null,{"o":1},"b.md"],"unknown":{"keep":true}}),
            serde_json::json!(["a.md",42,Value::Null,{"o":1},"b.md"]),
        ),
        (
            "custom array with no token",
            serde_json::json!({"context":["a.md","b.md"],"unknown":{"keep":true}}),
            serde_json::json!(["a.md", "b.md"]),
        ),
        (
            "near-token strings stay custom entries",
            serde_json::json!({"context":["$AGENTSCOMMANDER_CONTEXT ","$agentscommander_context","Role.md"],"unknown":{"keep":true}}),
            serde_json::json!([
                "$AGENTSCOMMANDER_CONTEXT ",
                "$agentscommander_context",
                "Role.md"
            ]),
        ),
        (
            "sentinel-only array",
            serde_json::json!({"context":[token],"unknown":{"keep":true}}),
            default_context.clone(),
        ),
        (
            "repeated-sentinel-only array",
            serde_json::json!({"context":[token,token],"unknown":{"keep":true}}),
            default_context.clone(),
        ),
    ];

    for (label, input, expected) in cases {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("config.json");
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&input).expect("serialize input"),
        )
        .expect("write config");

        merge_root_agent_config(&config_path).expect("merge config");
        let first_bytes = std::fs::read(&config_path).expect("read config");
        let first: Value = serde_json::from_slice(&first_bytes).expect("parse config");

        assert_eq!(first["context"], expected, "case: {}", label);
        assert!(
            !String::from_utf8_lossy(&first_bytes).contains(&format!("\"{}\"", token)),
            "case: {}: no exact token may survive",
            label
        );
        assert!(first["tooling"].is_object(), "case: {}", label);
        if input.get("unknown").is_some() {
            assert_eq!(first["unknown"]["keep"], true, "case: {}", label);
        }
        if let Some(last) = input.get("tooling").and_then(|t| t.get("lastCodingAgent")) {
            assert_eq!(
                &state_beside(&config_path)["tooling"]["lastCodingAgent"],
                last,
                "case: {}",
                label
            );
            assert!(
                first["tooling"].get("lastCodingAgent").is_none(),
                "case: {}",
                label
            );
        }

        merge_root_agent_config(&config_path).expect("merge config again");
        let second_bytes = std::fs::read(&config_path).expect("read config");
        let second: Value = serde_json::from_slice(&second_bytes).expect("parse config");
        assert_eq!(
            second, first,
            "case: {}: parsed value must be idempotent",
            label
        );
        assert_eq!(
            second_bytes, first_bytes,
            "case: {}: bytes must be stable after the first canonical rewrite",
            label
        );
    }
}

#[test]
fn malformed_config_returns_error_without_rewriting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("config.json");
    std::fs::write(&config_path, "{not json").expect("write config");

    let err = merge_root_agent_config(&config_path).expect_err("must fail");

    assert!(err.contains("Failed to parse"));
    assert_eq!(
        std::fs::read_to_string(&config_path).expect("read config"),
        "{not json"
    );
}

#[test]
fn set_last_coding_agent_preserves_root_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    ensure_root_agent_dir_at(&root).expect("ensure root");

    crate::config::agent_config::set_last_coding_agent(
        &root.to_string_lossy(),
        "codex",
        "Codex",
        Some("session-1"),
        None,
    )
    .expect("set last coding agent");

    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("config.json")).expect("read config"),
    )
    .expect("parse config");
    assert_eq!(
        state_beside(&root.join("config.json"))["tooling"]["lastCodingAgent"],
        "codex"
    );
    assert!(config["tooling"].get("lastCodingAgent").is_none());
    assert_eq!(
        config["context"],
        serde_json::json!(ROOT_AGENT_DEFAULT_CONTEXT)
    );
}

#[test]
fn read_last_coding_agent_reads_tooling_field_and_tolerates_bad_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join(ROOT_AGENT_DIR_NAME);
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(
        root.join("config.json"),
        r#"{"tooling":{"lastCodingAgent":"claude"}}"#,
    )
    .expect("write config");

    assert_eq!(
        read_last_coding_agent(&root.to_string_lossy()).as_deref(),
        Some("claude")
    );

    std::fs::write(root.join("config.json"), "{not json").expect("write bad config");
    assert!(read_last_coding_agent(&root.to_string_lossy()).is_none());
    assert!(read_last_coding_agent(&temp.path().join("missing").to_string_lossy()).is_none());
}

#[test]
fn root_dir_name_detection_is_case_insensitive() {
    assert!(is_root_agent_dir_name("C:/tmp/AC-ROOT-AGENT"));
    assert!(!is_root_agent_dir_name("C:/tmp/not-root"));
}

/// #1005 S5 / G3: expected values captured from the BASE-COMMIT blob
/// (`git show 409b7f90:...role-skill-boundary-audit/SKILL.md`), never from
/// this const. LF-folded first (G8) so the pin is checkout-invariant.
#[test]
fn role_skill_boundary_audit_before_token_minimization_snapshot_is_byte_exact() {
    use sha2::{Digest, Sha256};
    let lf = ROLE_SKILL_BOUNDARY_AUDIT_BEFORE_TOKEN_MINIMIZATION.replace("\r\n", "\n");
    assert_eq!(
        lf.len(),
        3199,
        "snapshot must be the 409b7f90 blob, byte for byte"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(lf.as_bytes())),
        "b3237843e2a6e9ac3cb014735ab398ee552711b0c017f8858758c619b3344c3f",
        "the frozen audit snapshot changed; it must stay byte-identical to what shipped"
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(
                normalize_role_text(ROLE_SKILL_BOUNDARY_AUDIT_BEFORE_TOKEN_MINIMIZATION).as_bytes()
            )
        ),
        "b27e1a9ae8aa8ce6febbdee89b95a07b99be18114cb627d6337356fd566607c3",
        "the normalized audit snapshot changed; it is what on-disk copies hash to"
    );
}
