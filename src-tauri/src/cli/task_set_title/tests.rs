use super::*;
use std::path::PathBuf;

/// Auto-cleaned temp dir; mirrors `cli::task_ops::tests::FixtureRoot`.
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

/// Build a fake project layout so `workgroup_root` succeeds when --root
/// points at the agent replica.
fn make_wg_fixture(tmp: &Path) -> PathBuf {
    let agent_root = tmp
        .join("proj")
        .join(".ac")
        .join("wg-1-test")
        .join("__agent_alice");
    std::fs::create_dir_all(&agent_root).unwrap();
    agent_root
}

fn args_for(token: Option<String>, root: Option<String>, title: &str) -> TaskSetTitleArgs {
    TaskSetTitleArgs {
        token,
        root,
        title: title.to_string(),
    }
}

// Helpers for tests that need a settings root_token. We persist a settings.json
// with a known root_token, then point AC at it via the existing config path.
// For the test boundary, we construct a UUID, then the test path uses
// validate_cli_token's UUID branch by passing a non-root UUID.

// ── I3, I4: non-coordinator rejected with UUID token ────────────────

#[test]
fn set_title_rejects_non_coordinator_with_uuid_token() {
    let fix = FixtureRoot::new("task-i3");
    let agent_root = make_wg_fixture(fix.path());
    // No team config exists — discover_teams returns empty, so any caller
    // (root-token aside) is rejected as non-coordinator.
    let token = uuid::Uuid::new_v4().to_string();
    let args = args_for(
        Some(token),
        Some(agent_root.to_string_lossy().into_owned()),
        "X",
    );
    let code = execute(args);
    assert_eq!(code, 1);
    // TASK.md was not created.
    let wg_root = agent_root.parent().unwrap();
    assert!(!wg_root.join("TASK.md").exists());
}

// ── I5: invalid token rejected ──────────────────────────────────────

#[test]
fn set_title_rejects_invalid_token() {
    let fix = FixtureRoot::new("task-i5");
    let agent_root = make_wg_fixture(fix.path());
    let args = args_for(
        Some("notauuid".into()),
        Some(agent_root.to_string_lossy().into_owned()),
        "X",
    );
    let code = execute(args);
    assert_eq!(code, 1);
}

// ── I6: unresolvable root rejected ──────────────────────────────────

#[test]
fn set_title_rejects_unresolvable_root() {
    let fix = FixtureRoot::new("task-i6");
    // Path with no wg-<N>-* ancestor.
    let agent_root = fix.path().join("no-wg-here");
    std::fs::create_dir_all(&agent_root).unwrap();
    let token = uuid::Uuid::new_v4().to_string();
    let args = args_for(
        Some(token),
        Some(agent_root.to_string_lossy().into_owned()),
        "X",
    );
    let code = execute(args);
    assert_eq!(code, 1);
}

// ── I17: control-char rejection on --title ──────────────────────────

#[test]
fn set_title_rejects_embedded_newlines() {
    let fix = FixtureRoot::new("task-i17");
    let agent_root = make_wg_fixture(fix.path());
    let token = uuid::Uuid::new_v4().to_string();
    let args = args_for(
        Some(token),
        Some(agent_root.to_string_lossy().into_owned()),
        "a\nb",
    );
    let code = execute(args);
    assert_eq!(code, 1);
    // TASK.md untouched.
    let wg_root = agent_root.parent().unwrap();
    assert!(!wg_root.join("TASK.md").exists());
}

#[test]
fn set_title_rejects_nul_byte_in_title() {
    let fix = FixtureRoot::new("task-i17b");
    let agent_root = make_wg_fixture(fix.path());
    let token = uuid::Uuid::new_v4().to_string();
    let args = args_for(
        Some(token),
        Some(agent_root.to_string_lossy().into_owned()),
        "a\u{0000}b",
    );
    let code = execute(args);
    assert_eq!(code, 1);
}

// ── I18: --root at workgroup-root directly ──────────────────────────

#[test]
fn set_title_rejects_when_root_is_workgroup_root_directly() {
    let fix = FixtureRoot::new("task-i18");
    let _agent_root = make_wg_fixture(fix.path());
    let wg_root = fix.path().join("proj").join(".ac").join("wg-1-test");
    // Coordinator running directly from the WG dir (no __agent_*) — the
    // is_any_coordinator gate fails closed (sender is not a coordinator).
    let token = uuid::Uuid::new_v4().to_string();
    let args = args_for(
        Some(token),
        Some(wg_root.to_string_lossy().into_owned()),
        "X",
    );
    let code = execute(args);
    assert_eq!(code, 1);
}

// ── I16: help text documents the verb ───────────────────────────────

#[test]
fn help_text_documents_set_title() {
    use clap::CommandFactory;
    let help = crate::cli::Cli::command().render_help().to_string();
    assert!(
        help.contains("task-set-title"),
        "help missing verb name: {}",
        help
    );
}
