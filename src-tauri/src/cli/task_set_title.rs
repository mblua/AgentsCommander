//! `task-set-title` CLI verb — set the YAML-frontmatter `title:` field of
//! the workgroup TASK.md.
//!
//! Trust model: caller honestly reports their own `--root` and `--token`.
//! The same model is inherited from `send`/`close-session` and has a known
//! weakness (any well-formed UUID is accepted as a token, and `--root` is
//! unverified). See plan #137 §3a for the escalation analysis. A follow-up
//! issue is recommended to bind tokens to issued sessions, closing the hole
//! for all CLI verbs simultaneously.

use clap::Args;
use std::path::Path;

use super::send::agent_name_from_root;
use super::task_ops::{self, EditOutcome, TaskOp};

#[derive(Args)]
#[command(after_help = "\
AUTHORIZATION: Only orchestrators of any team in the caller's project can edit TASK.md. \
The master/root token bypasses this check. The verb writes ONLY to \
<room-root>/TASK.md and its *.bak.md siblings.\n\n\
INVARIANTS: A timestamped backup is created on every successful write that had a \
prior file. Concurrent writes are serialized via an advisory lockfile (5s timeout). \
External edits between our read and our write are detected and the verb aborts.\n\n\
TITLE INPUT: --title is a single-line string. Embedded \\n / \\r / NUL / other \
control characters (except tab) are rejected. An orchestrator --title also cannot \
start with the reserved USER: prefix; that marker is reserved for human-set \
titles applied through the in-app title editor.")]
pub struct TaskSetTitleArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated in the CLI;
    /// per-session authorization happens at the daemon mailbox. See `--help` TOKEN VALIDATION MODEL.
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory — used to derive your agent name
    #[arg(long)]
    pub root: Option<String>,

    /// New title text (single line, no embedded newlines or control chars)
    #[arg(long)]
    pub title: String,
}

pub fn execute(args: TaskSetTitleArgs) -> i32 {
    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };

    let is_root = match crate::cli::validate_cli_token(&args.token) {
        Ok((_token, root)) => root,
        Err(msg) => {
            eprintln!("{}", msg);
            return 1;
        }
    };

    let sender = agent_name_from_root(&root);

    // Operation-specific validation: --title must be a non-empty single line
    // of printable characters (control characters other than tab are rejected).
    if args.title.trim().is_empty() {
        eprintln!("Error: --title cannot be empty.");
        return 1;
    }
    if args.title.chars().any(|c| c.is_control() && c != '\t') {
        eprintln!(
            "Error: --title must be a single line of printable characters \
             (control characters other than tab are not allowed)."
        );
        return 1;
    }

    // Coordinator gate (skipped for root/master token; mirrors close_session.rs:89-101).
    let is_master = is_root || {
        if let Some(ref token_str) = args.token {
            crate::config::config_dir()
                .map(|d| d.join("master-token.txt"))
                .and_then(|p| std::fs::read_to_string(&p).ok())
                .map(|m| m.trim() == token_str)
                .unwrap_or(false)
        } else {
            false
        }
    };

    if !is_master {
        let teams = crate::config::teams::discover_teams();
        if teams.is_empty() || !crate::config::teams::is_any_coordinator(&sender, &teams) {
            eprintln!(
                "Error: authorization denied — '{}' is not an orchestrator of any team. \
                 Only orchestrators can edit TASK.md.",
                sender
            );
            return 1;
        }
    }

    // Locate workgroup root.
    let wg_root = match crate::phone::messaging::workgroup_root(Path::new(&root)) {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "Error: --root is not under a `room-*` or legacy `wg-*` Room directory; \
                 cannot locate the room TASK.md."
            );
            return 1;
        }
    };

    // Hand off to task_ops::perform.
    // NIT-2: include `pid={}` so an auditor can cross-reference the AC process
    // tree. `sender=` and `wg=` are both caller-derived (--root) and a forged
    // --root produces a forged-but-consistent line; pid disambiguates.
    match task_ops::perform(&wg_root, TaskOp::SetTitle(args.title.clone())) {
        Ok(EditOutcome::Wrote { backup, .. }) => {
            if let Some(bp) = backup {
                log::info!(
                    "[task] set-title: sender={} wg={} pid={} backup={}",
                    sender,
                    wg_root.display(),
                    std::process::id(),
                    bp.display()
                );
            } else {
                log::info!(
                    "[task] set-title: sender={} wg={} pid={} backup=<no prior file>",
                    sender,
                    wg_root.display(),
                    std::process::id()
                );
            }
            crate::cli_println!("Updated");
            0
        }
        Ok(EditOutcome::NoOp { .. }) => {
            log::info!(
                "[task] set-title (no-op): sender={} wg={} pid={} (title value already matches)",
                sender,
                wg_root.display(),
                std::process::id()
            );
            crate::cli_println!("Updated");
            0
        }
        Ok(EditOutcome::RejectedUserTitle { .. }) => {
            log::info!(
                "[task] set-title rejected user-owned title: sender={} wg={} pid={}",
                sender,
                wg_root.display(),
                std::process::id()
            );
            crate::cli_println!("Rejected: title set by user");
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

#[cfg(test)]
mod tests {
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
}
