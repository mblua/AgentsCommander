use super::TaskOpError;
use chrono::DateTime;
use chrono::Utc;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::time::SystemTime;

// Status persistence shares the TASK lock; callers own authorization.
pub(super) const STATUS_LIMIT: usize = 65_536;
pub(super) const STATUS_WINDOW: u64 = 131_073;
pub(super) const TASK_LIMIT: u64 = 256 * 1024;
pub(super) const MAX_SEQUENCE: u64 = (1u64 << 53) - 1;
pub(super) const STATUS_NAME: &str = "TASK-status.jsonl";
pub(super) const JOURNAL_NAME: &str = "TASK-clean.pending.json";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatusRecord {
    pub schema_version: u32,
    pub kind: String,
    pub topic_id: String,
    pub sequence: u64,
    pub request_id: Option<String>,
    pub base_revision: Option<String>,
    pub recorded_at: String,
    pub author: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSnapshot {
    pub workgroup_root: String,
    pub task: Option<String>,
    pub task_title: Option<String>,
    pub description: String,
    pub status: Option<String>,
    pub revision: String,
    pub status_record: Option<StatusRecord>,
    pub tail_incomplete: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusReceipt {
    pub revision: String,
    pub record: StatusRecord,
    pub replayed: bool,
}

pub(super) struct StatusTail {
    pub(super) record: Option<StatusRecord>,
    pub(super) partial: Vec<u8>,
    pub(super) complete_len: u64,
    pub(super) len: u64,
    pub(super) modified: Option<SystemTime>,
    pub(super) bytes_read: usize,
}

impl StatusTail {
    pub(super) fn revision(&self) -> String {
        self.record.as_ref().map_or_else(
            || "legacy:0".into(),
            |r| format!("{}:{}", r.topic_id, r.sequence),
        )
    }
}

pub(super) fn status_invalid(reason: impl Into<String>) -> TaskOpError {
    TaskOpError::InvalidStatus(reason.into())
}

pub(super) fn validate_status_text(text: &str) -> Result<(), TaskOpError> {
    if text.trim().is_empty()
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(status_invalid(
            "empty status or forbidden control character",
        ));
    }
    Ok(())
}

pub(super) fn valid_author(author: &str) -> bool {
    !author.chars().any(|c| c.is_whitespace() || c.is_control())
        && author.split_once('/').is_some_and(|(project, agent)| {
            !project.is_empty() && !agent.is_empty() && !agent.contains('/')
        })
}

pub(super) fn valid_revision(revision: &str) -> bool {
    revision == "legacy:0"
        || revision.rsplit_once(':').is_some_and(|(topic, sequence)| {
            uuid::Uuid::parse_str(topic).is_ok()
                && sequence
                    .parse::<u64>()
                    .is_ok_and(|n| n <= MAX_SEQUENCE && n.to_string() == sequence)
        })
}

pub(super) fn validate_record(bytes: &[u8]) -> Result<StatusRecord, TaskOpError> {
    if bytes.len() + 1 > STATUS_LIMIT {
        return Err(status_invalid("completed row exceeds 65536 bytes"));
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| status_invalid(format!("invalid completed JSON: {e}")))?;
    for field in [
        "schemaVersion",
        "kind",
        "topicId",
        "sequence",
        "requestId",
        "baseRevision",
        "recordedAt",
        "author",
        "status",
    ] {
        if value.get(field).is_none() {
            return Err(status_invalid(format!("missing {field}")));
        }
    }
    let r: StatusRecord = serde_json::from_value(value)
        .map_err(|e| status_invalid(format!("invalid record shape: {e}")))?;
    if r.schema_version != 1
        || uuid::Uuid::parse_str(&r.topic_id).is_err()
        || r.sequence > MAX_SEQUENCE
        || DateTime::parse_from_rfc3339(&r.recorded_at)
            .map_or(true, |t| t.offset().local_minus_utc() != 0)
    {
        return Err(status_invalid(
            "unsupported schema, topic, sequence or UTC time",
        ));
    }
    match r.kind.as_str() {
        "topic_started"
            if r.sequence == 0
                && r.request_id.is_none()
                && r.base_revision.is_none()
                && r.author.is_none()
                && r.status.is_none() => {}
        "status"
            if r.sequence > 0
                && r.request_id
                    .as_deref()
                    .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
                && r.base_revision.as_deref().is_some_and(valid_revision)
                && r.author.as_deref().is_some_and(valid_author) =>
        {
            validate_status_text(
                r.status
                    .as_deref()
                    .ok_or_else(|| status_invalid("missing status"))?,
            )?;
            let base = r.base_revision.as_deref().unwrap();
            let expected = if r.sequence == 1 && base == "legacy:0" {
                "legacy:0".to_string()
            } else {
                format!("{}:{}", r.topic_id, r.sequence - 1)
            };
            if base != expected {
                return Err(status_invalid("invalid base sequence"));
            }
        }
        _ => return Err(status_invalid("invalid kind or record semantics")),
    }
    Ok(r)
}

pub(super) fn read_latest_status_locked(root: &Path) -> Result<StatusTail, TaskOpError> {
    use std::io::{Read, Seek, SeekFrom};
    let path = root.join(STATUS_NAME);
    let mut file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StatusTail {
                record: None,
                partial: Vec::new(),
                complete_len: 0,
                len: 0,
                modified: None,
                bytes_read: 0,
            })
        }
        Err(e) => return Err(TaskOpError::ReadFailed(path, e)),
    };
    let meta = file
        .metadata()
        .map_err(|e| TaskOpError::ReadFailed(path.clone(), e))?;
    let len = file
        .seek(SeekFrom::End(0))
        .map_err(|e| TaskOpError::ReadFailed(path.clone(), e))?;
    let start = len.saturating_sub(STATUS_WINDOW);
    file.seek(SeekFrom::Start(start))
        .map_err(|e| TaskOpError::ReadFailed(path.clone(), e))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.take(STATUS_WINDOW)
        .read_to_end(&mut bytes)
        .map_err(|e| TaskOpError::ReadFailed(path, e))?;
    let last_lf = bytes.iter().rposition(|b| *b == b'\n');
    let end = last_lf.map_or(0, |i| i + 1);
    let partial = bytes[end..].to_vec();
    if partial.len() >= STATUS_LIMIT || (last_lf.is_none() && start != 0) {
        return Err(status_invalid("partial suffix exceeds 65535 bytes"));
    }
    let record = if let Some(lf) = last_lf {
        let row_start = bytes[..lf]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1);
        if row_start == 0 && start != 0 {
            return Err(status_invalid("oversized completed row"));
        }
        Some(validate_record(&bytes[row_start..lf])?)
    } else {
        None
    };
    Ok(StatusTail {
        record,
        partial,
        complete_len: start + end as u64,
        len,
        modified: meta.modified().ok(),
        bytes_read: bytes.len(),
    })
}

pub(super) fn write_failed(e: std::io::Error) -> TaskOpError {
    TaskOpError::WriteFailed(e)
}

// Failures are injected only in fixtures, in this thread and this module.
#[cfg(test)]
thread_local! {
    pub(super) static IO_MUTATION: std::cell::RefCell<Option<(String, PathBuf, Vec<u8>)>> =
        const { std::cell::RefCell::new(None) };
    pub(super) static IO_FAULT: std::cell::RefCell<(Option<String>, Vec<String>)> =
        const { std::cell::RefCell::new((None, Vec::new())) };
}
pub(super) fn io_boundary(name: &str) -> std::io::Result<()> {
    #[cfg(test)]
    {
        if std::env::var("AC_TASK_OPS_CHILD_BOUNDARY").as_deref() == Ok(name)
            && std::env::var_os("AC_TASK_OPS_CHILD_FIXTURE").is_some()
        {
            std::process::exit(71);
        }
        IO_MUTATION.with(|v| {
            let mut v = v.borrow_mut();
            if v.as_ref().is_some_and(|(point, _, _)| point == name) {
                let (_, path, bytes) = v.take().unwrap();
                std::fs::write(path, bytes).unwrap();
            }
        });
        IO_FAULT.with(|f| {
            let mut f = f.borrow_mut();
            f.1.push(name.into());
            if f.0.as_deref() == Some(name) {
                Err(std::io::Error::other(format!("fixture: {name}")))
            } else {
                Ok(())
            }
        })
    }
    #[cfg(not(test))]
    {
        let _ = name;
        Ok(())
    }
}

pub(super) fn sync_file(file: &std::fs::File, boundary: &str) -> std::io::Result<()> {
    io_boundary(boundary)?;
    file.sync_all()
}

pub(super) fn repair_partial_locked(root: &Path, tail: &StatusTail) -> Result<(), TaskOpError> {
    if tail.partial.is_empty() {
        return Ok(());
    }
    let path = root.join(STATUS_NAME);
    let unchanged = |current: &StatusTail| {
        current.len == tail.len
            && current.modified == tail.modified
            && current.partial == tail.partial
            && current.complete_len == tail.complete_len
            && current.record == tail.record
    };
    if !unchanged(&read_latest_status_locked(root)?) {
        return Err(TaskOpError::StatusSourceChanged);
    }
    let backup = root.join(format!(
        "TASK-status.partial.{}.{}.bak",
        Utc::now().format("%Y%m%d-%H%M%S"),
        uuid::Uuid::new_v4()
    ));
    io_boundary("partial_backup_create").map_err(write_failed)?;
    let mut saved = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .map_err(write_failed)?;
    io_boundary("partial_backup_write").map_err(write_failed)?;
    saved.write_all(&tail.partial).map_err(write_failed)?;
    sync_file(&saved, "partial_backup_sync").map_err(write_failed)?;
    if !unchanged(&read_latest_status_locked(root)?) {
        return Err(TaskOpError::StatusSourceChanged);
    }
    io_boundary("partial_truncate").map_err(write_failed)?;
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(write_failed)?;
    file.set_len(tail.complete_len).map_err(write_failed)?;
    sync_file(&file, "partial_truncate_sync").map_err(write_failed)
}
