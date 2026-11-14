//! Pure logic for the `task-set-title` and `task-append-body` CLI verbs.
//!
//! This facade coordinates TASK operations. Its document, operation and lock
//! children own text representation, typed contracts and kernel-lock lifetime;
//! clean and flow own recovery, publication and backup. It contains NO
//! clap surface and NO authorization — the per-verb modules
//! (`task_set_title`, `task_append_body`) handle those concerns and call
//! into [`perform`].
//!
//! Trust model: caller honestly reports their own `--root` and `--token`.
//! The same model is inherited from `send`/`close-session` and has a known
//! weakness (any well-formed UUID is accepted as a token, and `--root` is
//! unverified). See plan #137 §3a for the escalation analysis. A follow-up
//! issue is recommended to bind tokens to issued sessions, closing the hole
//! for all CLI verbs simultaneously.

#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

use std::path::Path;
use std::time::SystemTime;

#[cfg(test)]
use chrono::DateTime;
use chrono::Utc;

mod clean;
mod document;
mod flow;
mod initialization;
mod lock;
mod operation;
mod status;

use crate::filesystem::tasks::TASK_FILE_NAME;
use crate::filesystem::tasks::{
    description_sentinel, io_boundary, open_status_append, open_task_sync,
    read_description_snapshot,
};
#[cfg(test)]
use crate::filesystem::tasks::{IO_FAULT, IO_MUTATION};
#[cfg(test)]
use clean::recover_clean_pair;
use clean::recover_clean_pair_locked;
#[cfg(test)]
use clean::{backup_names, CleanJournal};
#[cfg(test)]
use document::{
    apply_append_body, apply_clean, apply_edit, apply_set_title, render, title_line,
    user_owned_title, ParsedTask,
};
pub(crate) use document::{parse_task, title_value_of};
use flow::{perform_inner, task_lock_path};
pub(crate) use initialization::{
    build_task_content, initialize_new_room, prepare_new_room_task_content, validate_task_title,
};
use lock::{LockGuard, LOCK_STALE_AFTER_5M, LOCK_TIMEOUT_5S};
pub use operation::{EditOutcome, TaskOp, TaskOpError};
pub use status::StatusReceipt;
pub use status::StatusRecord;
pub use status::TaskSnapshot;
use status::{
    read_latest_status_locked, repair_partial_locked, status_invalid, valid_author, valid_revision,
    validate_status_text, write_failed, JOURNAL_NAME, MAX_SEQUENCE, STATUS_LIMIT, STATUS_NAME,
    TASK_LIMIT,
};
#[cfg(test)]
use status::{validate_record, STATUS_WINDOW};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct StatSentinel {
    pub(crate) len: u64,
    pub(crate) mtime: Option<SystemTime>,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct TaskSentinels {
    pub(crate) task: Option<StatSentinel>,
    pub(crate) status: Option<StatSentinel>,
    pub(crate) journal: bool,
}

/// Observe task-owned metadata without locking or recovery.
/// Callers retain authorization; mutation and recovery use their existing locks.
pub(crate) fn task_sentinels(root: &Path) -> Result<TaskSentinels, String> {
    let stat = |name| match description_sentinel(&root.join(name)) {
        Ok((len, mtime)) => Ok(Some(StatSentinel { len, mtime })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("snapshot sentinel read failed: {error}")),
    };
    Ok(TaskSentinels {
        task: stat(TASK_FILE_NAME)?,
        status: stat(STATUS_NAME)?,
        journal: stat(JOURNAL_NAME)?.is_some(),
    })
}

/// Read the title without acquiring a mutation lock or recovering task state.
pub(crate) fn read_title(root: &Path) -> Result<Option<String>, String> {
    let task_path = root.join(TASK_FILE_NAME);
    let content = match crate::filesystem::tasks::read_task_title_text(&task_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read TASK.md: {e}")),
    };
    let parsed = parse_task(&content);
    Ok(title_value_of(&parsed))
}

// ── Public surface ──────────────────────────────────────────────────────────

/// Production entry point. Captures `chrono::Utc::now` for backup-name
/// timestamping and delegates to [`perform_inner`].
pub fn perform(wg_root: &Path, op: TaskOp) -> Result<EditOutcome, TaskOpError> {
    perform_inner(wg_root, op, Utc::now)
}

pub fn read_snapshot(root: &Path) -> Result<TaskSnapshot, TaskOpError> {
    let _lock = LockGuard::acquire(&task_lock_path(root), LOCK_TIMEOUT_5S, LOCK_STALE_AFTER_5M)?;
    recover_clean_pair_locked(root)?;
    let path = root.join(TASK_FILE_NAME);
    let task = match read_description_snapshot(&path, TASK_LIMIT) {
        Ok(Some(bytes)) => {
            if bytes.len() as u64 > TASK_LIMIT {
                return Err(TaskOpError::SnapshotTooLarge);
            }
            Some(String::from_utf8(bytes).map_err(|e| {
                TaskOpError::ReadFailed(
                    path.clone(),
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                )
            })?)
        }
        Ok(None) => None,
        Err(e) => return Err(TaskOpError::ReadFailed(path, e)),
    };
    let parsed = parse_task(task.as_deref().unwrap_or(""));
    let tail = read_latest_status_locked(root)?;
    Ok(TaskSnapshot {
        workgroup_root: root.to_string_lossy().into_owned(),
        task_title: title_value_of(&parsed),
        description: parsed.body,
        task,
        status: tail.record.as_ref().and_then(|r| r.status.clone()),
        revision: tail.revision(),
        tail_incomplete: !tail.partial.is_empty(),
        status_record: tail.record,
    })
}

pub fn append_status(
    root: &Path,
    expected_revision: &str,
    request_id: &str,
    text: &str,
    author: &str,
) -> Result<StatusReceipt, TaskOpError> {
    let _lock = LockGuard::acquire(&task_lock_path(root), LOCK_TIMEOUT_5S, LOCK_STALE_AFTER_5M)?;
    recover_clean_pair_locked(root)?;
    let tail = read_latest_status_locked(root)?;
    if let Some(last) = &tail.record {
        if last.request_id.as_deref() == Some(request_id) {
            if last.base_revision.as_deref() != Some(expected_revision)
                || last.author.as_deref() != Some(author)
                || last.status.as_deref() != Some(text)
            {
                return Err(TaskOpError::RequestIdConflict);
            }
            let file = open_task_sync(&root.join(STATUS_NAME)).map_err(write_failed)?;
            file.sync_at("append_sync").map_err(write_failed)?;
            return Ok(StatusReceipt {
                revision: tail.revision(),
                record: last.clone(),
                replayed: true,
            });
        }
    }
    validate_status_text(text)?;
    if uuid::Uuid::parse_str(request_id).is_err()
        || !valid_revision(expected_revision)
        || !valid_author(author)
    {
        return Err(status_invalid("invalid request, revision or author"));
    }
    let revision = tail.revision();
    if expected_revision != revision {
        return Err(TaskOpError::RevisionConflict {
            current_revision: revision,
        });
    }
    let (topic, sequence) = match &tail.record {
        Some(r) if r.sequence == MAX_SEQUENCE => return Err(TaskOpError::SequenceOverflow),
        Some(r) => (r.topic_id.clone(), r.sequence + 1),
        None => (uuid::Uuid::new_v4().to_string(), 1),
    };
    let record = StatusRecord {
        schema_version: 1,
        kind: "status".into(),
        topic_id: topic,
        sequence,
        request_id: Some(request_id.into()),
        base_revision: Some(expected_revision.into()),
        recorded_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        author: Some(author.into()),
        status: Some(text.into()),
    };
    let mut bytes = serde_json::to_vec(&record).map_err(|e| status_invalid(e.to_string()))?;
    bytes.push(b'\n');
    if bytes.len() > STATUS_LIMIT {
        return Err(TaskOpError::StatusTooLarge);
    }
    repair_partial_locked(root, &tail)?;
    io_boundary("append_open").map_err(write_failed)?;
    let mut file = open_status_append(&root.join(STATUS_NAME)).map_err(write_failed)?;
    file.append_status_bytes(&bytes).map_err(write_failed)?;
    Ok(StatusReceipt {
        revision: format!("{}:{}", record.topic_id, record.sequence),
        record,
        replayed: false,
    })
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
