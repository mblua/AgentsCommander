//! `task-append-body` CLI verb - append a body paragraph to the workgroup
//! TASK.md without touching the YAML frontmatter.
//!
//! Trust model: caller honestly reports their own `--root` and `--token`.
//! The same model is inherited from `send`/`close-session` and has a known
//! weakness (any well-formed UUID is accepted as a token, and `--root` is
//! unverified). See plan #137 §3a for the escalation analysis. A follow-up
//! issue is recommended to bind tokens to issued sessions, closing the hole
//! for all CLI verbs simultaneously.

use clap::Args;
use std::path::Path;

use super::task_ops::{self, TaskOp, EditOutcome};
use super::send::agent_name_from_root;

#[derive(Args)]
#[command(after_help = "\
AUTHORIZATION: Only coordinators of any team in the caller's project can edit TASK.md. \
The master/root token bypasses this check. The verb writes ONLY to \
<workgroup-root>/TASK.md and its *.bak.md siblings.\n\n\
INVARIANTS: A timestamped backup is created on every successful write that had a \
prior file. Concurrent writes are serialized via an advisory lockfile (5s timeout). \
External edits between our read and our write are detected and the verb aborts. \
Frontmatter is never modified by this verb.\n\n\
TEXT INPUT: --text accepts multi-line content. Newline (\\n), carriage return (\\r), \
and tab (\\t) are permitted. NUL and other control characters are rejected.")]
pub struct TaskAppendBodyArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated in the CLI;
    /// per-session authorization happens at the daemon mailbox. See `--help` TOKEN VALIDATION MODEL.
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory - used to derive your agent name
    #[arg(long)]
    pub root: Option<String>,

    /// Body text to append. Multi-paragraph supported (preserves internal newlines)
    #[arg(long)]
    pub text: String,
}

pub fn execute(args: TaskAppendBodyArgs) -> i32 {
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

    // Validation: --text must be non-empty after trim, and must not contain
    // invisible-byte control chars (NUL, \x01-\x08, \x0b-\x0c, \x0e-\x1f).
    if args.text.trim().is_empty() {
        eprintln!("Error: --text cannot be empty.");
        return 1;
    }
    if args
        .text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        eprintln!(
            "Error: --text contains a control character that is not allowed \
             (only newline, carriage return, and tab are permitted)."
        );
        return 1;
    }

    // Coordinator gate (skipped for root/master token).
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
                "Error: authorization denied - '{}' is not a coordinator of any team. \
                 Only coordinators can edit TASK.md.",
                sender
            );
            return 1;
        }
    }

    let wg_root = match crate::phone::messaging::workgroup_root(Path::new(&root)) {
        Ok(p) => p,
        Err(_) => {
            eprintln!(
                "Error: --root is not under a wg-<N>-* ancestor; \
                 cannot locate the workgroup TASK.md."
            );
            return 1;
        }
    };

    // NIT-2: include `pid={}` so an auditor can cross-reference the AC process
    // tree. `sender=` and `wg=` are both caller-derived (--root) and a forged
    // --root produces a forged-but-consistent line; pid disambiguates.
    match task_ops::perform(&wg_root, TaskOp::AppendBody(args.text.clone())) {
        Ok(EditOutcome::Wrote { backup: Some(bp) }) => {
            log::info!(
                "[task] append-body: sender={} wg={} pid={} backup={}",
                sender,
                wg_root.display(),
                std::process::id(),
                bp.display()
            );
            crate::cli_println!("TASK.md body appended; backup: {}", bp.display());
            0
        }
        Ok(EditOutcome::Wrote { backup: None }) => {
            log::info!(
                "[task] append-body: sender={} wg={} pid={} backup=<no prior file>",
                sender,
                wg_root.display(),
                std::process::id()
            );
            crate::cli_println!("TASK.md created; no prior content to back up");
            0
        }
        Ok(EditOutcome::NoOp) => {
            // append-body never produces NoOp (an append always changes the file).
            // Defensive: surface the same success line as a Wrote{None} would.
            crate::cli_println!("TASK.md unchanged");
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
            .join(".ac-new")
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
    // `task_ops::tests::apply_append_body_preserves_internal_body_line_endings_and_documents_trailing_loss`.
    // Reaching the apply layer through `execute` would require stubbing
    // team-config so the coordinator gate passes - the apply-layer test
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
}
