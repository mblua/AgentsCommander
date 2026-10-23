use super::*;
use std::path::PathBuf;

struct FixtureRoot(PathBuf);
impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl FixtureRoot {
    fn new(prefix: &str) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::process::id().hash(&mut h);
        std::thread::current().id().hash(&mut h);
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            h.finish()
        ));
        std::fs::create_dir_all(&path).expect("fixture root");
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

fn make_wg_fixture(tmp: &Path) -> PathBuf {
    let agent_root = tmp
        .join("proj")
        .join(".ac")
        .join("wg-1-test")
        .join("__agent_alice");
    std::fs::create_dir_all(&agent_root).unwrap();
    agent_root
}

fn args_for(token: Option<String>, root: Option<String>, text: &str) -> TaskAppendBodyArgs {
    TaskAppendBodyArgs {
        token,
        root,
        text: text.to_string(),
    }
}

#[test]
fn status_input_validation_bounds_and_revision() {
    let mut args = TaskStatusSetArgs {
        token: "fixture".into(),
        root: "fixture".into(),
        expected_revision: "legacy:0".into(),
        request_id: uuid::Uuid::new_v4().to_string(),
        text: "Remaining 🦀\nFUP\r\tcontinuation".into(),
    };
    assert!(valid_status_args(&args));
    for text in ["", " \t\n", "bad\u{0}", "bad\u{7}", "bad\u{7f}"] {
        args.text = text.into();
        assert!(!valid_status_args(&args));
    }
    args.text = "🦀".repeat(12 * 1024);
    assert!(valid_status_args(&args));
    args.text.push('x');
    assert!(!valid_status_args(&args));
    args.text = "complete status".into();
    args.expected_revision = format!("{}:9007199254740991", uuid::Uuid::new_v4());
    assert!(valid_status_args(&args));
    args.expected_revision = format!("{}:9007199254740992", uuid::Uuid::new_v4());
    assert!(!valid_status_args(&args));
    args.expected_revision = format!("{}:01", uuid::Uuid::new_v4());
    assert!(!valid_status_args(&args));
    args.expected_revision = "legacy:0".into();
    args.request_id = "invalid".into();
    assert!(!valid_status_args(&args));
}

#[test]
fn set_body_clap_requires_text_accepts_empty_whitespace_and_multiline() {
    use clap::Parser;
    assert!(crate::cli::Cli::try_parse_from([
        "ac",
        "task-set-body",
        "--root",
        "fixture",
        "--token",
        "fixture"
    ])
    .is_err());
    for text in [
        "",
        " \t\n",
        "Unicode 🦀\r\nend\n\n",
        "---\ntitle: 'USER: forged'\n---\nliteral",
    ] {
        let cli = crate::cli::Cli::try_parse_from([
            "ac",
            "task-set-body",
            "--root",
            "fixture",
            "--token",
            "fixture",
            "--text",
            text,
        ])
        .unwrap();
        match cli.command {
            Some(crate::cli::Commands::TaskSetBody(args)) => assert_eq!(args.text, text),
            _ => panic!("wrong command"),
        }
    }
}
#[test]
fn set_body_help_documents_literal_clear_and_preservation() {
    use clap::CommandFactory;
    let mut command = crate::cli::Cli::command();
    assert!(command.render_help().to_string().contains("task-set-body"));
    let help = command
        .find_subcommand_mut("task-set-body")
        .unwrap()
        .render_long_help()
        .to_string();
    for literal in [
        "multi-line",
        "--text ''",
        "Whitespace",
        "title",
        "topic",
        "revision",
        "status",
        "empty frontmatter",
        "pending Clean",
        "backup",
        "5s timeout",
        "external-edit",
    ] {
        assert!(help.contains(literal), "missing {literal}: {help}");
    }
}
#[test]
fn set_body_rejects_invalid_token_noncoordinator_and_controls_without_writes() {
    for (token, text) in [
        (Some("not-a-uuid".to_owned()), "valid"),
        (None, "valid"),
        (Some(uuid::Uuid::new_v4().to_string()), "valid"),
        (Some(uuid::Uuid::new_v4().to_string()), ""),
        (Some(uuid::Uuid::new_v4().to_string()), "bad\u{0}"),
        (Some(uuid::Uuid::new_v4().to_string()), "bad\u{7}"),
        (Some(uuid::Uuid::new_v4().to_string()), "bad\u{7f}"),
    ] {
        let fixture = FixtureRoot::new("task-set-body-reject");
        let root = make_wg_fixture(fixture.path());
        let room = root.parent().unwrap();
        let original = "---\ntitle: 'Real'\n---\nkeep";
        std::fs::write(room.join("TASK.md"), original).unwrap();
        std::fs::write(room.join("TASK-status.jsonl"), "sentinel status").unwrap();
        assert_eq!(
            execute_set_body(TaskSetBodyArgs {
                token,
                root: Some(root.to_string_lossy().into_owned()),
                text: text.into()
            }),
            1
        );
        assert_eq!(
            std::fs::read_to_string(room.join("TASK.md")).unwrap(),
            original
        );
        assert_eq!(
            std::fs::read_to_string(room.join("TASK-status.jsonl")).unwrap(),
            "sentinel status"
        );
        assert_eq!(std::fs::read_dir(room).unwrap().count(), 3);
    }
    assert_eq!(
        execute_set_body(TaskSetBodyArgs {
            token: Some(uuid::Uuid::new_v4().to_string()),
            root: None,
            text: "valid".into()
        }),
        1
    );
}

// ── I4: non-coordinator rejected ────────────────────────────────────

#[test]
fn append_body_rejects_non_coordinator_with_uuid_token() {
    let fix = FixtureRoot::new("task-ai4");
    let agent_root = make_wg_fixture(fix.path());
    let token = uuid::Uuid::new_v4().to_string();
    let args = args_for(
        Some(token),
        Some(agent_root.to_string_lossy().into_owned()),
        "hello",
    );
    let code = execute(args);
    assert_eq!(code, 1);
    let wg_root = agent_root.parent().unwrap();
    assert!(!wg_root.join("TASK.md").exists());
}

// ── (token rejection) ───────────────────────────────────────────────
// Note: the substantive I19 guarantee ("--text preserves internal
// newlines after a successful append") is covered at the apply layer by
// the test in `task_ops::tests` named
// `apply_append_body_preserves_internal_body_line_endings_and_documents_trailing_loss`.
// Reaching the apply layer through `execute` would require stubbing
// team-config so the coordinator gate passes — the apply-layer test
// gives the same byte-level guarantee at much lower cost.

#[test]
fn append_body_rejects_invalid_token() {
    let fix = FixtureRoot::new("task-ai-token");
    let agent_root = make_wg_fixture(fix.path());
    let args = args_for(
        Some("not-a-uuid".into()),
        Some(agent_root.to_string_lossy().into_owned()),
        "hello",
    );
    let code = execute(args);
    assert_eq!(code, 1);
}

#[test]
fn append_body_rejects_nul_byte_in_text() {
    let fix = FixtureRoot::new("task-ai-nul");
    let agent_root = make_wg_fixture(fix.path());
    let token = uuid::Uuid::new_v4().to_string();
    let args = args_for(
        Some(token),
        Some(agent_root.to_string_lossy().into_owned()),
        "abc\u{0000}def",
    );
    let code = execute(args);
    assert_eq!(code, 1);
}

// ── I16: help text documents the verb ───────────────────────────────

#[test]
fn help_text_documents_append_body() {
    use clap::CommandFactory;
    let help = crate::cli::Cli::command().render_help().to_string();
    assert!(
        help.contains("task-append-body"),
        "help missing verb name: {}",
        help
    );
}
