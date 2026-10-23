//! `task-append-body` CLI verb — append a body paragraph to the workgroup
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

use super::send::agent_name_from_root;
use super::task_ops::{self, EditOutcome, TaskOp};

#[derive(Args)]
#[command(after_help = "\
AUTHORIZATION: Only orchestrators of any team in the caller's project can edit TASK.md. \
The master/root token bypasses this check. The verb writes ONLY to \
<room-root>/TASK.md and its *.bak.md siblings.\n\n\
INVARIANTS: A timestamped backup is created on every successful write that had a \
prior file. Concurrent writes are serialized via an advisory lockfile (5s timeout). \
External edits between our read and our write are detected and the verb aborts. \
Frontmatter is never modified by this verb.\n\n\
TEXT INPUT: --text accepts multi-line content. Newline (\\n), carriage return (\\r), \
and tab (\\t) are permitted. NUL and other control characters are rejected.")]
pub struct TaskAppendBodyArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN. Shape-validated in the CLI;
    /// per-session authorization happens at the daemon mailbox.
    /// See `--help` TOKEN VALIDATION MODEL.
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory — used to derive your agent name
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
        let teams = crate::config::teams::discover_teams_for_cli();
        if teams.is_empty() || !crate::config::teams::is_any_coordinator(&sender, &teams) {
            eprintln!(
                "Error: authorization denied — '{}' is not an orchestrator of any team. \
                 Only orchestrators can edit TASK.md.",
                sender
            );
            return 1;
        }
    }

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

    // NIT-2: include `pid={}` so an auditor can cross-reference the AC process
    // tree. `sender=` and `wg=` are both caller-derived (--root) and a forged
    // --root produces a forged-but-consistent line; pid disambiguates.
    match task_ops::perform(&wg_root, TaskOp::AppendBody(args.text.clone())) {
        Ok(EditOutcome::Wrote {
            backup: Some(bp), ..
        }) => {
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
        Ok(EditOutcome::Wrote { backup: None, .. }) => {
            log::info!(
                "[task] append-body: sender={} wg={} pid={} backup=<no prior file>",
                sender,
                wg_root.display(),
                std::process::id()
            );
            crate::cli_println!("TASK.md created; no prior content to back up");
            0
        }
        Ok(EditOutcome::NoOp { .. }) => {
            // append-body never produces NoOp (an append always changes the file).
            // Defensive: surface the same success line as a Wrote{None} would.
            crate::cli_println!("TASK.md unchanged");
            0
        }
        Ok(EditOutcome::RejectedUserTitle { .. }) => {
            // Unreachable for AppendBody (the #738 user-lock guard only fires on
            // TaskOp::SetTitle); defensive arm so the match stays exhaustive.
            crate::cli_println!("TASK.md unchanged");
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

#[derive(Args)]
#[command(
    after_help = "AUTHORIZATION: Only project team orchestrators can edit TASK.md; the master/root \
        token bypasses this check.

TEXT INPUT: --text replaces the complete body literally, including multi-line content, Unicode \
        and trailing newlines. Use --text '' to clear the body. Whitespace is preserved and \
        does not mean empty. Newline, carriage return and tab are permitted; NUL and other \
        controls are rejected.

INVARIANTS: Preserves the title (including USER:), topic, revision and status. An effective \
        change to legacy content without closed frontmatter adds an empty frontmatter block \
        without a title. Identical body text leaves the original bytes unchanged without a \
        backup. Effective changes to an existing file create a timestamped exact backup, with \
        an advisory lock (5s timeout) and external-edit detection. An already pending Clean \
        operation is recovered first; these guarantees apply to that recovered baseline. \
        Frontmatter delimiters/line endings may be normalized by the existing renderer."
)]
pub struct TaskSetBodyArgs {
    /// Session token from AGENTSCOMMANDER_TOKEN (existing trusted-session model)
    #[arg(long)]
    pub token: Option<String>,
    /// Agent root directory (required)
    #[arg(long)]
    pub root: Option<String>,
    /// Complete replacement body; an explicit empty value clears it
    #[arg(long, allow_hyphen_values = true)]
    pub text: String,
}

pub fn execute_set_body(args: TaskSetBodyArgs) -> i32 {
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
        let teams = crate::config::teams::discover_teams_for_cli();
        if teams.is_empty() || !crate::config::teams::is_any_coordinator(&sender, &teams) {
            eprintln!(
                "Error: authorization denied — '{}' is not an orchestrator of any team. \
                 Only orchestrators can edit TASK.md.",
                sender
            );
            return 1;
        }
    }

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

    // NIT-2: include `pid={}` so an auditor can cross-reference the AC process
    // tree. `sender=` and `wg=` are both caller-derived (--root) and a forged
    // --root produces a forged-but-consistent line; pid disambiguates.
    match task_ops::perform(&wg_root, TaskOp::SetBody(args.text.clone())) {
        Ok(EditOutcome::Wrote {
            backup: Some(bp), ..
        }) => {
            log::info!(
                "[task] set-body: sender={} wg={} pid={} result={} backup={}",
                sender,
                wg_root.display(),
                std::process::id(),
                if args.text.is_empty() {
                    "cleared"
                } else {
                    "replaced"
                },
                bp.display()
            );
            crate::cli_println!(
                "TASK.md body {}; backup: {}",
                if args.text.is_empty() {
                    "cleared"
                } else {
                    "replaced"
                },
                bp.display()
            );
            0
        }
        Ok(EditOutcome::Wrote { backup: None, .. }) => {
            log::info!(
                "[task] set-body: sender={} wg={} pid={} result=created backup=<no prior file>",
                sender,
                wg_root.display(),
                std::process::id()
            );
            crate::cli_println!("TASK.md created; no prior content to back up");
            0
        }
        Ok(EditOutcome::NoOp { .. }) => {
            log::info!(
                "[task] set-body: sender={} wg={} pid={} result=unchanged",
                sender,
                wg_root.display(),
                std::process::id()
            );
            crate::cli_println!("TASK.md unchanged");
            0
        }
        Ok(EditOutcome::RejectedUserTitle { .. }) => {
            // Defensive: SetBody does not apply the title ownership guard.
            crate::cli_println!("TASK.md unchanged");
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

/// Credentials are explicit flags, preserving the existing trusted-local-session model.
#[derive(Args)]
pub struct TaskGetArgs {
    #[arg(long)]
    pub token: String,
    #[arg(long)]
    pub root: String,
}

#[derive(Args)]
pub struct TaskStatusSetArgs {
    #[arg(long)]
    pub token: String,
    #[arg(long)]
    pub root: String,
    #[arg(long)]
    pub expected_revision: String,
    #[arg(long)]
    pub request_id: String,
    /// Complete replacement status, including remaining tickets and continuation.
    #[arg(long)]
    pub text: String,
}

fn task_error(code: &str, message: &str, current_revision: Option<&str>) -> i32 {
    let mut value = serde_json::json!({"error": code, "message": message});
    if let Some(revision) = current_revision {
        value["currentRevision"] = revision.into();
    }
    eprintln!("{value}");
    if code == "revision_conflict" {
        2
    } else {
        1
    }
}

fn task_authorization(token: &str, root: &str) -> Result<(std::path::PathBuf, String), i32> {
    let denied = || {
        task_error(
            "authorization_denied",
            "Room orchestrator authorization required",
            None,
        )
    };
    let (_, is_master) =
        crate::cli::validate_cli_token(&Some(token.to_owned())).map_err(|_| denied())?;
    let sender = agent_name_from_root(root);
    if !is_master {
        let teams = crate::config::teams::discover_teams_for_cli();
        if teams.is_empty() || !crate::config::teams::is_any_coordinator(&sender, &teams) {
            return Err(denied());
        }
    }
    let room = crate::phone::messaging::workgroup_root(Path::new(root))
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("room-") || n.starts_with("wg-"))
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|n| n == ".ac")
        })
        .ok_or_else(denied)?;
    Ok((room, sender))
}

fn task_storage_error(error: task_ops::TaskOpError) -> i32 {
    use task_ops::TaskOpError;
    let code = match &error {
        TaskOpError::RevisionConflict { current_revision } => {
            return task_error(
                "revision_conflict",
                "Status revision changed",
                Some(current_revision),
            );
        }
        TaskOpError::RequestIdConflict => "request_id_conflict",
        TaskOpError::InvalidStatus(_) => "status_corrupt",
        TaskOpError::StatusTooLarge => "invalid_input",
        TaskOpError::SequenceOverflow => "status_corrupt",
        TaskOpError::SnapshotTooLarge | TaskOpError::ReadFailed(..) => "read_failed",
        TaskOpError::LockTimeout => "lock_timeout",
        TaskOpError::CleanRecoveryPending(_) => "clean_recovery_pending",
        TaskOpError::CleanRecoveryConflict(_) => "clean_recovery_conflict",
        _ => "write_failed",
    };
    // Do not expose storage diagnostics containing caller-supplied text or paths.
    task_error(code, "Room task operation failed", None)
}

pub fn execute_get(args: TaskGetArgs) -> i32 {
    let (room, sender) = match task_authorization(&args.token, &args.root) {
        Ok(value) => value,
        Err(code) => return code,
    };
    match task_ops::read_snapshot(&room) {
        Ok(snapshot) => {
            log::info!(
                "[task] get: sender={} wg={} pid={} revision={}",
                sender,
                room.display(),
                std::process::id(),
                snapshot.revision
            );
            crate::cli_println!("{}", serde_json::json!(snapshot));
            0
        }
        Err(error) => task_storage_error(error),
    }
}

fn valid_status_args(args: &TaskStatusSetArgs) -> bool {
    let revision_valid = args.expected_revision == "legacy:0"
        || args
            .expected_revision
            .rsplit_once(':')
            .is_some_and(|(topic, sequence)| {
                uuid::Uuid::parse_str(topic).is_ok()
                    && sequence
                        .parse::<u64>()
                        .is_ok_and(|n| n <= 9_007_199_254_740_991 && n.to_string() == sequence)
            });
    revision_valid
        && uuid::Uuid::parse_str(&args.request_id).is_ok()
        && !args.text.trim().is_empty()
        && args.text.len() <= 48 * 1024
        && !args
            .text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}

pub fn execute_status_set(args: TaskStatusSetArgs) -> i32 {
    let (room, sender) = match task_authorization(&args.token, &args.root) {
        Ok(value) => value,
        Err(code) => return code,
    };
    if !valid_status_args(&args) {
        return task_error(
            "invalid_input",
            "Invalid status, revision or request UUID",
            None,
        );
    }
    match task_ops::append_status(
        &room,
        &args.expected_revision,
        &args.request_id,
        &args.text,
        &sender,
    ) {
        Ok(receipt) => {
            log::info!(
                "[task] status-set: sender={} wg={} pid={} revision={} requestId={} replayed={}",
                sender,
                room.display(),
                std::process::id(),
                receipt.revision,
                args.request_id,
                receipt.replayed
            );
            crate::cli_println!(
                "{}",
                serde_json::json!({
                    "workgroupRoot": room.to_string_lossy(),
                    "revision": receipt.revision,
                    "requestId": receipt.record.request_id,
                    "recordedAt": receipt.record.recorded_at,
                    "status": receipt.record.status,
                    "replayed": receipt.replayed,
                })
            );
            0
        }
        Err(error) => task_storage_error(error),
    }
}

#[cfg(test)]
mod tests;
