use super::apply_clean;
use super::parse_task;
use super::render;
use super::status::io_boundary;
use super::status::read_latest_status_locked;
use super::status::status_invalid;
use super::status::sync_file;
use super::status::write_failed;
use super::status::StatusRecord;
use super::status::JOURNAL_NAME;
use super::status::STATUS_LIMIT;
use super::status::STATUS_NAME;
use super::EditOutcome;
#[cfg(test)]
use super::LockGuard;
use super::TaskOpError;
#[cfg(test)]
use super::LOCK_STALE_AFTER_5M;
#[cfg(test)]
use super::LOCK_TIMEOUT_5S;
use chrono::DateTime;
use chrono::Utc;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;
use std::time::SystemTime;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CleanSide {
    pub(super) existed: bool,
    pub(super) original_hash: Option<String>,
    pub(super) target_hash: String,
    pub(super) backup: Option<String>,
    pub(super) stage: String,
}
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CleanJournal {
    pub(super) schema_version: u32,
    pub(super) tx_id: String,
    pub(super) stamp: String,
    pub(super) suffix: u32,
    pub(super) task: CleanSide,
    pub(super) status: CleanSide,
}
#[derive(PartialEq)]
struct SourceState {
    hash: Option<String>,
    len: u64,
    modified: Option<SystemTime>,
}

fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}
fn source_state(path: &Path) -> std::io::Result<SourceState> {
    use sha2::Digest;
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SourceState {
                hash: None,
                len: 0,
                modified: None,
            })
        }
        Err(e) => return Err(e),
    };
    let meta = file.metadata()?;
    let mut hash = sha2::Sha256::new();
    let mut buf = [0u8; 65536];
    let mut len = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        len += n as u64;
    }
    if len != meta.len() {
        return Err(std::io::Error::other("source changed while hashing"));
    }
    Ok(SourceState {
        hash: Some(format!("{:x}", hash.finalize())),
        len,
        modified: meta.modified().ok(),
    })
}

fn rename_retry(source: &Path, target: &Path) -> std::io::Result<()> {
    for attempt in 0..3 {
        match std::fs::rename(source, target) {
            Ok(()) => return Ok(()),
            Err(e)
                if attempt < 2
                    && (e.kind() == std::io::ErrorKind::PermissionDenied
                        || matches!(e.raw_os_error(), Some(5) | Some(32))) =>
            {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

fn clean_conflict(reason: impl Into<String>) -> TaskOpError {
    TaskOpError::CleanRecoveryConflict(reason.into())
}
fn clean_pending(e: std::io::Error) -> TaskOpError {
    TaskOpError::CleanRecoveryPending(e.to_string())
}

pub(super) fn backup_names(stamp: &str, suffix: u32) -> (String, String) {
    let tag = if suffix == 0 {
        stamp.into()
    } else {
        format!("{stamp}.{suffix}")
    };
    (
        format!("TASK.{tag}.bak.md"),
        format!("TASK-status.{tag}.bak.jsonl"),
    )
}

fn validate_journal(j: &CleanJournal) -> Result<(), TaskOpError> {
    let hash_ok = |h: &str| {
        h.len() == 64
            && h.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    };
    let stamp_ok = j.stamp.len() == 15
        && j.stamp.as_bytes()[8] == b'-'
        && j.stamp
            .bytes()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit())
        && chrono::NaiveDateTime::parse_from_str(&j.stamp, "%Y%m%d-%H%M%S").is_ok();
    if j.schema_version != 1
        || uuid::Uuid::parse_str(&j.tx_id).is_err()
        || !stamp_ok
        || j.suffix > 99
    {
        return Err(clean_conflict("invalid journal header"));
    }
    let names = backup_names(&j.stamp, j.suffix);
    let any = j.task.existed || j.status.existed;
    for (side, target, backup) in [
        (&j.task, "TASK.md", names.0),
        (&j.status, STATUS_NAME, names.1),
    ] {
        if side.stage != format!("{target}.tmp.{}", j.tx_id)
            || side.backup != any.then_some(backup)
            || side.existed != side.original_hash.is_some()
            || side.original_hash.as_deref().is_some_and(|h| !hash_ok(h))
            || !hash_ok(&side.target_hash)
        {
            return Err(clean_conflict("invalid journal side or basename"));
        }
    }
    Ok(())
}

fn cleanup_owned(path: &Path, hash: &str) {
    if source_state(path).ok().and_then(|s| s.hash).as_deref() == Some(hash) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
pub(super) fn recover_clean_pair(root: &Path) -> Result<(), TaskOpError> {
    let _lock = LockGuard::acquire(
        &root.join("TASK.md.lock"),
        LOCK_TIMEOUT_5S,
        LOCK_STALE_AFTER_5M,
    )?;
    recover_clean_pair_locked(root)
}

pub(super) fn recover_clean_pair_locked(root: &Path) -> Result<(), TaskOpError> {
    use std::io::Read;
    let journal_path = root.join(JOURNAL_NAME);
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(&journal_path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(clean_pending(e)),
    };
    let mut bytes = Vec::new();
    file.take(16_385)
        .read_to_end(&mut bytes)
        .map_err(clean_pending)?;
    if bytes.len() > 16_384 {
        return Err(clean_conflict("oversized journal"));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| clean_conflict(e.to_string()))?;
    for field in ["schemaVersion", "txId", "stamp", "suffix", "task", "status"] {
        if value.get(field).is_none() {
            return Err(clean_conflict("missing journal field"));
        }
    }
    for side in ["task", "status"] {
        for field in ["existed", "originalHash", "targetHash", "backup", "stage"] {
            if value[side].get(field).is_none() {
                return Err(clean_conflict("missing journal side field"));
            }
        }
    }
    let journal: CleanJournal =
        serde_json::from_value(value).map_err(|e| clean_conflict(e.to_string()))?;
    validate_journal(&journal)?;
    let journal_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&journal_path)
        .map_err(clean_pending)?;
    sync_file(&journal_file, "journal_sync").map_err(clean_pending)?;
    drop(journal_file);
    finish_clean_locked(root, &journal, &bytes)
}

fn finish_clean_locked(
    root: &Path,
    journal: &CleanJournal,
    journal_bytes: &[u8],
) -> Result<(), TaskOpError> {
    // Validate the entire pair before replacing either target.
    for (side, target) in [(&journal.task, "TASK.md"), (&journal.status, STATUS_NAME)] {
        let current = source_state(&root.join(target)).map_err(clean_pending)?;
        if current.hash.as_deref() != Some(&side.target_hash) && current.hash != side.original_hash
        {
            return Err(clean_conflict(format!("external edit: {target}")));
        }
        if let Some(backup) = &side.backup {
            let expected = side
                .original_hash
                .clone()
                .unwrap_or_else(|| hash_bytes(&[]));
            if source_state(&root.join(backup))
                .map_err(|e| clean_conflict(e.to_string()))?
                .hash
                .as_deref()
                != Some(&expected)
            {
                return Err(clean_conflict("missing or changed backup"));
            }
        }
        if current.hash.as_deref() != Some(&side.target_hash)
            && source_state(&root.join(&side.stage))
                .map_err(|e| clean_conflict(e.to_string()))?
                .hash
                .as_deref()
                != Some(&side.target_hash)
        {
            return Err(clean_conflict("missing or changed needed stage"));
        }
    }
    for (side, target, boundary) in [
        (&journal.task, "TASK.md", "task_rename"),
        (&journal.status, STATUS_NAME, "status_rename"),
    ] {
        let path = root.join(target);
        let current = source_state(&path).map_err(clean_pending)?;
        if current.hash.as_deref() == Some(&side.target_hash) {
            continue;
        }
        if current.hash != side.original_hash {
            return Err(clean_conflict(format!(
                "external edit before replacement: {target}"
            )));
        }
        io_boundary(boundary).map_err(clean_pending)?;
        if source_state(&path).map_err(clean_pending)?.hash != side.original_hash {
            return Err(clean_conflict(
                "external edit immediately before replacement",
            ));
        }
        if source_state(&root.join(&side.stage))
            .map_err(|e| clean_conflict(e.to_string()))?
            .hash
            .as_deref()
            != Some(&side.target_hash)
        {
            return Err(clean_conflict(
                "stage changed immediately before replacement",
            ));
        }
        rename_retry(&root.join(&side.stage), &path).map_err(clean_pending)?;
        io_boundary(&format!("after_{boundary}")).map_err(clean_pending)?;
    }
    // Even an already-new pair requires fresh sync attempts for BOTH targets.
    // Keep the first failure while still attempting the other target.
    let mut sync_error = None;
    for (side, target, boundary) in [
        (&journal.task, "TASK.md", "task_target_sync"),
        (&journal.status, STATUS_NAME, "status_target_sync"),
    ] {
        let path = root.join(target);
        if source_state(&path).map_err(clean_pending)?.hash.as_deref() != Some(&side.target_hash) {
            return Err(clean_conflict("target hash changed"));
        }
        let attempt = io_boundary(&format!("{boundary}_open")).and_then(|()| {
            let file = OpenOptions::new().read(true).write(true).open(path)?;
            sync_file(&file, boundary)
        });
        if let Err(e) = attempt {
            if sync_error.is_none() {
                sync_error = Some(e);
            }
        }
    }
    if let Some(e) = sync_error {
        return Err(clean_pending(e));
    }
    for (side, target) in [(&journal.task, "TASK.md"), (&journal.status, STATUS_NAME)] {
        if source_state(&root.join(target))
            .map_err(clean_pending)?
            .hash
            .as_deref()
            != Some(&side.target_hash)
        {
            return Err(clean_conflict("target changed after sync"));
        }
    }
    if std::fs::read(root.join(JOURNAL_NAME)).map_err(clean_pending)? != journal_bytes {
        return Err(clean_conflict("journal changed"));
    }
    io_boundary("journal_remove").map_err(clean_pending)?;
    std::fs::remove_file(root.join(JOURNAL_NAME)).map_err(clean_pending)?;
    for side in [&journal.task, &journal.status] {
        cleanup_owned(&root.join(&side.stage), &side.target_hash);
    }
    Ok(())
}

fn copy_backup(
    source: &Path,
    dest: &mut std::fs::File,
    exists: bool,
    boundary: &str,
    owned_hash: &mut String,
) -> std::io::Result<()> {
    use sha2::Digest;
    use std::io::Read;
    let mut hash = sha2::Sha256::new();
    io_boundary(&format!("{boundary}_copy"))?;
    if exists {
        let mut source = std::fs::File::open(source)?;
        let mut buffer = [0u8; 65536];
        loop {
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            let mut offset = 0;
            while offset < count {
                io_boundary(&format!("{boundary}_write"))?;
                let written = dest.write(&buffer[offset..count])?;
                if written == 0 {
                    return Err(std::io::ErrorKind::WriteZero.into());
                }
                hash.update(&buffer[offset..offset + written]);
                *owned_hash = format!("{:x}", hash.clone().finalize());
                offset += written;
                io_boundary(&format!("{boundary}_after_write"))?;
            }
        }
    }
    sync_file(dest, &format!("{boundary}_sync"))
}

pub(super) fn clean_pair_locked(
    root: &Path,
    existing: &str,
    now: DateTime<Utc>,
) -> Result<EditOutcome, TaskOpError> {
    let parsed = parse_task(existing);
    let cleaned = apply_clean(&parsed);
    // Corrupt logs are intentionally archivable; only inspect for NoOp when
    // the description already matches the canonical reset.
    if cleaned.frontmatter == parsed.frontmatter && cleaned.body == parsed.body {
        if let Ok(tail) = read_latest_status_locked(root) {
            if tail.partial.is_empty()
                && (tail.len == 0
                    || tail
                        .record
                        .as_ref()
                        .is_some_and(|r| r.kind == "topic_started")
                        && tail.complete_len == tail.bytes_read as u64
                        && tail.bytes_read <= STATUS_LIMIT
                        && {
                            let bytes =
                                std::fs::read(root.join(STATUS_NAME)).map_err(write_failed)?;
                            bytes.iter().filter(|b| **b == b'\n').count() == 1
                        })
            {
                return Ok(EditOutcome::NoOp {});
            }
        }
    }
    let task_path = root.join("TASK.md");
    let status_path = root.join(STATUS_NAME);
    let original_task = source_state(&task_path).map_err(write_failed)?;
    let original_status = source_state(&status_path).map_err(write_failed)?;
    // The caller's description must still be the description being archived.
    if original_task
        .hash
        .as_deref()
        .is_some_and(|h| h != hash_bytes(existing.as_bytes()))
    {
        return Err(clean_conflict("description changed before archive"));
    }
    let new_content = render(&cleaned);
    let seed = StatusRecord {
        schema_version: 1,
        kind: "topic_started".into(),
        topic_id: uuid::Uuid::new_v4().to_string(),
        sequence: 0,
        request_id: None,
        base_revision: None,
        recorded_at: now.to_rfc3339(),
        author: None,
        status: None,
    };
    let mut seed_bytes = serde_json::to_vec(&seed).map_err(|e| status_invalid(e.to_string()))?;
    seed_bytes.push(b'\n');
    let tx = uuid::Uuid::new_v4().to_string();
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let mut journal = CleanJournal {
        schema_version: 1,
        tx_id: tx.clone(),
        stamp: stamp.clone(),
        suffix: 0,
        task: CleanSide {
            existed: original_task.hash.is_some(),
            original_hash: original_task.hash.clone(),
            target_hash: hash_bytes(new_content.as_bytes()),
            backup: None,
            stage: format!("TASK.md.tmp.{tx}"),
        },
        status: CleanSide {
            existed: original_status.hash.is_some(),
            original_hash: original_status.hash.clone(),
            target_hash: hash_bytes(&seed_bytes),
            backup: None,
            stage: format!("TASK-status.jsonl.tmp.{tx}"),
        },
    };
    if journal.task.existed || journal.status.existed {
        let mut reserved = None;
        for n in 0..=99 {
            let (task_name, status_name) = backup_names(&stamp, n);
            io_boundary("task_backup_create").map_err(write_failed)?;
            let task_file = match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join(&task_name))
            {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(write_failed(e)),
            };
            let status_file = io_boundary("status_backup_create").and_then(|()| {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(root.join(&status_name))
            });
            match status_file {
                Ok(f) => {
                    reserved = Some((n, task_name, status_name, task_file, f));
                    break;
                }
                Err(e) => {
                    drop(task_file);
                    cleanup_owned(&root.join(&task_name), &hash_bytes(&[]));
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        continue;
                    }
                    return Err(write_failed(e));
                }
            }
        }
        let (n, task_name, status_name, mut task_file, mut status_file) =
            reserved.ok_or_else(|| {
                TaskOpError::BackupExhausted(root.join(format!("TASK.{stamp}.bak.md")))
            })?;
        journal.suffix = n;
        journal.task.backup = Some(task_name.clone());
        journal.status.backup = Some(status_name.clone());
        // Track bytes actually written, so failure cleanup cannot delete a
        // foreign replacement. Successfully synced archives stay immutable.
        let mut task_owned_hash = hash_bytes(&[]);
        let mut status_owned_hash = hash_bytes(&[]);
        let task_result = copy_backup(
            &task_path,
            &mut task_file,
            journal.task.existed,
            "task_backup",
            &mut task_owned_hash,
        );
        let task_finished = task_result.is_ok();
        let copied = task_result.and_then(|()| {
            copy_backup(
                &status_path,
                &mut status_file,
                journal.status.existed,
                "status_backup",
                &mut status_owned_hash,
            )
        });
        drop(task_file);
        drop(status_file);
        if let Err(e) = copied {
            if !task_finished {
                cleanup_owned(&root.join(&task_name), &task_owned_hash);
            }
            cleanup_owned(&root.join(&status_name), &status_owned_hash);
            return Err(write_failed(e));
        }
        for (side, expected) in [
            (&journal.task, &original_task),
            (&journal.status, &original_status),
        ] {
            let backup_hash = source_state(&root.join(side.backup.as_ref().unwrap()))
                .map_err(write_failed)?
                .hash;
            if backup_hash != Some(expected.hash.clone().unwrap_or_else(|| hash_bytes(&[]))) {
                return Err(clean_conflict("archive source changed"));
            }
        }
    }
    for (side, bytes, boundary) in [
        (&journal.task, new_content.as_bytes(), "task_stage"),
        (&journal.status, seed_bytes.as_slice(), "status_stage"),
    ] {
        io_boundary(&format!("{boundary}_create")).map_err(write_failed)?;
        let mut stage = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(&side.stage))
            .map_err(write_failed)?;
        io_boundary(&format!("{boundary}_write")).map_err(write_failed)?;
        stage.write_all(bytes).map_err(write_failed)?;
        sync_file(&stage, &format!("{boundary}_sync")).map_err(write_failed)?;
    }
    io_boundary("before_source_recheck").map_err(write_failed)?;
    if source_state(&task_path).map_err(write_failed)? != original_task
        || source_state(&status_path).map_err(write_failed)? != original_status
    {
        return Err(clean_conflict("source changed before journal"));
    }
    let journal_bytes = serde_json::to_vec(&journal).map_err(|e| clean_conflict(e.to_string()))?;
    let tmp = root.join(format!("{JOURNAL_NAME}.tmp.{tx}"));
    io_boundary("journal_create").map_err(write_failed)?;
    let mut journal_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(write_failed)?;
    io_boundary("journal_write").map_err(write_failed)?;
    journal_file
        .write_all(&journal_bytes)
        .map_err(write_failed)?;
    sync_file(&journal_file, "journal_stage_sync").map_err(write_failed)?;
    drop(journal_file);
    if root.join(JOURNAL_NAME).exists() {
        return Err(clean_conflict("foreign journal appeared"));
    }
    io_boundary("journal_publish").map_err(write_failed)?;
    rename_retry(&tmp, &root.join(JOURNAL_NAME)).map_err(write_failed)?;
    io_boundary("after_journal_publish").map_err(clean_pending)?;
    let published = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(JOURNAL_NAME))
        .map_err(clean_pending)?;
    sync_file(&published, "journal_sync").map_err(clean_pending)?;
    drop(published);
    finish_clean_locked(root, &journal, &journal_bytes)?;
    Ok(EditOutcome::Wrote {
        backup: journal.task.backup.map(|p| root.join(p)),
        #[cfg(test)]
        content: new_content,
    })
}

// ── Core flow (clock-injection seam, §G.1) ─────────────────────────────────
