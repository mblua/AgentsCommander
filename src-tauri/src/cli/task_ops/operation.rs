use std::path::PathBuf;

/// Edit operation requested by a verb.
#[derive(Debug, Clone)]
pub enum TaskOp {
    /// Coordinator/agent title write. Writes the value verbatim (no `USER:`
    /// marker) and is subject to the user-lock + reserved-prefix guards.
    SetTitle(String),
    /// Human title write (GUI editor + workgroup creation). Prefixes the value
    /// with `USER:` (unless already present) and bypasses the coordinator
    /// user-lock guard so a human can always edit a prior human title.
    SetUserTitle(String),
    /// Append a body paragraph (frontmatter untouched).
    AppendBody(String),
    /// Replace the complete body literally, preserving title and status.
    SetBody(String),
    /// Replace BOTH frontmatter title AND body with the canonical Clean form
    /// (title: 'Clean', empty body). Preserves the
    /// file's existing BOM and frontmatter line ending; body has no bytes. `NoOp` when the
    /// file is already in canonical Clean form.
    Clean,
}

/// Outcome of a successful [`crate::cli::task_ops::perform`] call.
/// The CLI translates this into the verb-specific stdout line.
#[derive(Debug, Clone)]
pub enum EditOutcome {
    /// File was written. `backup` is `None` when the file did not exist before.
    Wrote {
        backup: Option<PathBuf>,
        #[cfg(test)]
        content: String,
    },
    /// Set-title found the existing value already matched; no write performed.
    NoOp {},
    /// Coordinator set-title rejected because the current title is user-owned
    /// (`USER:`). NOT an error: exit code 0, no write, no backup. The GUI/human
    /// path (`SetUserTitle`) never produces this.
    RejectedUserTitle {},
}

/// Errors emitted by [`crate::cli::task_ops::perform`].
/// `Display` impls match the §3 error matrix
/// of plan #137 verbatim.
#[derive(Debug, thiserror::Error)]
pub enum TaskOpError {
    /// Coordinator `--title` input itself started with the reserved `USER:`
    /// prefix while the current title was not already user-owned (#738). Invalid
    /// input: exit 1, no stdout, TASK.md unchanged, no backup.
    #[error("--title cannot start with reserved USER: prefix")]
    ReservedUserTitlePrefix,
    #[error("invalid_status: {0}")]
    InvalidStatus(String),
    #[error("snapshot_too_large: TASK.md exceeds 256 KiB")]
    SnapshotTooLarge,
    #[error("status_too_large: encoded record exceeds 65536 bytes")]
    StatusTooLarge,
    #[error("sequence_overflow")]
    SequenceOverflow,
    #[error("request_id_conflict")]
    RequestIdConflict,
    #[error("revision_conflict: currentRevision={current_revision}")]
    RevisionConflict { current_revision: String },
    #[error("write_failed: {0}")]
    WriteFailed(std::io::Error),
    #[error("write_failed: status source changed; partial evidence preserved")]
    StatusSourceChanged,
    #[error("clean_recovery_pending: {0}")]
    CleanRecoveryPending(String),
    #[error("clean_recovery_conflict: {0}")]
    CleanRecoveryConflict(String),
    #[error("TASK.md is locked by another writer (5s timeout). Try again.")]
    LockTimeout,
    #[error(
        "failed to acquire TASK.md lock at {}: {}. Aborting; TASK.md left unchanged.",
        .0.display(),
        .1
    )]
    LockIo(PathBuf, std::io::Error),
    #[error("failed to read TASK.md at {}: {}", .0.display(), .1)]
    ReadFailed(PathBuf, std::io::Error),
    #[error(
        "failed to write backup at {}: {}. Aborting; TASK.md left unchanged.",
        .0.display(),
        .1
    )]
    BackupFailed(PathBuf, std::io::Error),
    #[error(
        "failed to write backup at {}: 100 collision retries exhausted in the same second. \
         Aborting; TASK.md left unchanged.",
        .0.display()
    )]
    BackupExhausted(PathBuf),
    #[error("failed to write {}: {}. Aborting; TASK.md left unchanged.", .0.display(), .1)]
    TmpWriteFailed(PathBuf, std::io::Error),
    #[error(
        "TASK.md was modified externally between read and write; aborting. \
         Backup at {} retains the externally-modified state.",
        .0.display()
    )]
    ExternalWrite(PathBuf),
    /// Custom Display below: `Some(p)` → "Backup at <p> retains the prior state.";
    /// `None` → "No backup (TASK.md did not exist before)." (§H.4 / NIT-2 in plan).
    #[error("{}", format_rename_failed(.0, .1))]
    RenameFailed(std::io::Error, Option<PathBuf>),
}

fn format_rename_failed(io_err: &std::io::Error, backup: &Option<PathBuf>) -> String {
    match backup {
        Some(p) => format!(
            "failed to publish TASK.md (rename): {}. Backup at {} retains the prior state.",
            io_err,
            p.display()
        ),
        None => format!(
            "failed to publish TASK.md (rename): {io_err}. \
             No backup (TASK.md did not exist before)."
        ),
    }
}
