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
    /// per-session authorization happens at the daemon mailbox.
    /// See `--help` TOKEN VALIDATION MODEL.
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

    let wg_root = match super::task_append_body::task_edit_room(
        is_root,
        args.token.as_deref(),
        &sender,
        &root,
    ) {
        Ok(room) => room,
        Err(code) => return code,
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
mod tests;
