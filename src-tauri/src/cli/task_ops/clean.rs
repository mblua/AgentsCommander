use super::apply_clean;
use super::initialization::TASK_FILE_NAME;
use super::parse_task;
use super::render;
use super::status::read_latest_status_locked;
use super::status::status_invalid;
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
use crate::filesystem::tasks::{
    create_task_artifact, io_boundary, open_task_sync, read_clean_journal, read_task_artifact,
    remove_task_artifact, rename_task_artifact_with_retry, task_artifact_exists, task_source_state,
};
use chrono::DateTime;
use chrono::Utc;
use std::path::Path;

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
fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
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
        (&j.task, TASK_FILE_NAME, names.0),
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
    if task_source_state(path).ok().and_then(|s| s.hash).as_deref() == Some(hash) {
        let _ = remove_task_artifact(path);
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
    let journal_path = root.join(JOURNAL_NAME);
    let bytes = match read_clean_journal(&journal_path) {
        Ok(None) => return Ok(()),
        Ok(Some(bytes)) => bytes,
        Err(e) => return Err(clean_pending(e)),
    };
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
    let journal_file = open_task_sync(&journal_path).map_err(clean_pending)?;
    journal_file
        .sync_at("journal_sync")
        .map_err(clean_pending)?;
    drop(journal_file);
    finish_clean_locked(root, &journal, &bytes)
}

fn finish_clean_locked(
    root: &Path,
    journal: &CleanJournal,
    journal_bytes: &[u8],
) -> Result<(), TaskOpError> {
    // Validate the entire pair before replacing either target.
    for (side, target) in [
        (&journal.task, TASK_FILE_NAME),
        (&journal.status, STATUS_NAME),
    ] {
        let current = task_source_state(&root.join(target)).map_err(clean_pending)?;
        if current.hash.as_deref() != Some(&side.target_hash) && current.hash != side.original_hash
        {
            return Err(clean_conflict(format!("external edit: {target}")));
        }
        if let Some(backup) = &side.backup {
            let expected = side
                .original_hash
                .clone()
                .unwrap_or_else(|| hash_bytes(&[]));
            if task_source_state(&root.join(backup))
                .map_err(|e| clean_conflict(e.to_string()))?
                .hash
                .as_deref()
                != Some(&expected)
            {
                return Err(clean_conflict("missing or changed backup"));
            }
        }
        if current.hash.as_deref() != Some(&side.target_hash)
            && task_source_state(&root.join(&side.stage))
                .map_err(|e| clean_conflict(e.to_string()))?
                .hash
                .as_deref()
                != Some(&side.target_hash)
        {
            return Err(clean_conflict("missing or changed needed stage"));
        }
    }
    for (side, target, boundary) in [
        (&journal.task, TASK_FILE_NAME, "task_rename"),
        (&journal.status, STATUS_NAME, "status_rename"),
    ] {
        let path = root.join(target);
        let current = task_source_state(&path).map_err(clean_pending)?;
        if current.hash.as_deref() == Some(&side.target_hash) {
            continue;
        }
        if current.hash != side.original_hash {
            return Err(clean_conflict(format!(
                "external edit before replacement: {target}"
            )));
        }
        io_boundary(boundary).map_err(clean_pending)?;
        if task_source_state(&path).map_err(clean_pending)?.hash != side.original_hash {
            return Err(clean_conflict(
                "external edit immediately before replacement",
            ));
        }
        if task_source_state(&root.join(&side.stage))
            .map_err(|e| clean_conflict(e.to_string()))?
            .hash
            .as_deref()
            != Some(&side.target_hash)
        {
            return Err(clean_conflict(
                "stage changed immediately before replacement",
            ));
        }
        rename_task_artifact_with_retry(&root.join(&side.stage), &path).map_err(clean_pending)?;
        io_boundary(&format!("after_{boundary}")).map_err(clean_pending)?;
    }
    // Even an already-new pair requires fresh sync attempts for BOTH targets.
    // Keep the first failure while still attempting the other target.
    let mut sync_error = None;
    for (side, target, boundary) in [
        (&journal.task, TASK_FILE_NAME, "task_target_sync"),
        (&journal.status, STATUS_NAME, "status_target_sync"),
    ] {
        let path = root.join(target);
        if task_source_state(&path)
            .map_err(clean_pending)?
            .hash
            .as_deref()
            != Some(&side.target_hash)
        {
            return Err(clean_conflict("target hash changed"));
        }
        let attempt = io_boundary(&format!("{boundary}_open")).and_then(|()| {
            let file = open_task_sync(&path)?;
            file.sync_at(boundary)
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
    for (side, target) in [
        (&journal.task, TASK_FILE_NAME),
        (&journal.status, STATUS_NAME),
    ] {
        if task_source_state(&root.join(target))
            .map_err(clean_pending)?
            .hash
            .as_deref()
            != Some(&side.target_hash)
        {
            return Err(clean_conflict("target changed after sync"));
        }
    }
    if read_task_artifact(&root.join(JOURNAL_NAME)).map_err(clean_pending)? != journal_bytes {
        return Err(clean_conflict("journal changed"));
    }
    io_boundary("journal_remove").map_err(clean_pending)?;
    remove_task_artifact(&root.join(JOURNAL_NAME)).map_err(clean_pending)?;
    for side in [&journal.task, &journal.status] {
        cleanup_owned(&root.join(&side.stage), &side.target_hash);
    }
    Ok(())
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
                            let bytes = read_task_artifact(&root.join(STATUS_NAME))
                                .map_err(write_failed)?;
                            bytes.iter().filter(|b| **b == b'\n').count() == 1
                        })
            {
                return Ok(EditOutcome::NoOp {});
            }
        }
    }
    let task_path = root.join(TASK_FILE_NAME);
    let status_path = root.join(STATUS_NAME);
    let original_task = task_source_state(&task_path).map_err(write_failed)?;
    let original_status = task_source_state(&status_path).map_err(write_failed)?;
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
            let task_file = match create_task_artifact(&root.join(&task_name)) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(write_failed(e)),
            };
            let status_file = io_boundary("status_backup_create")
                .and_then(|()| create_task_artifact(&root.join(&status_name)));
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
        let task_result = task_file.copy_archive_from(
            &task_path,
            journal.task.existed,
            "task_backup",
            &mut task_owned_hash,
        );
        let task_finished = task_result.is_ok();
        let copied = task_result.and_then(|()| {
            status_file.copy_archive_from(
                &status_path,
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
            let backup_hash = task_source_state(&root.join(side.backup.as_ref().unwrap()))
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
        let mut stage = create_task_artifact(&root.join(&side.stage)).map_err(write_failed)?;
        io_boundary(&format!("{boundary}_write")).map_err(write_failed)?;
        stage.write_bytes(bytes).map_err(write_failed)?;
        stage
            .sync_at(&format!("{boundary}_sync"))
            .map_err(write_failed)?;
    }
    io_boundary("before_source_recheck").map_err(write_failed)?;
    if task_source_state(&task_path).map_err(write_failed)? != original_task
        || task_source_state(&status_path).map_err(write_failed)? != original_status
    {
        return Err(clean_conflict("source changed before journal"));
    }
    let journal_bytes = serde_json::to_vec(&journal).map_err(|e| clean_conflict(e.to_string()))?;
    let tmp = root.join(format!("{JOURNAL_NAME}.tmp.{tx}"));
    io_boundary("journal_create").map_err(write_failed)?;
    let mut journal_file = create_task_artifact(&tmp).map_err(write_failed)?;
    io_boundary("journal_write").map_err(write_failed)?;
    journal_file
        .write_bytes(&journal_bytes)
        .map_err(write_failed)?;
    journal_file
        .sync_at("journal_stage_sync")
        .map_err(write_failed)?;
    drop(journal_file);
    if task_artifact_exists(&root.join(JOURNAL_NAME)) {
        return Err(clean_conflict("foreign journal appeared"));
    }
    io_boundary("journal_publish").map_err(write_failed)?;
    rename_task_artifact_with_retry(&tmp, &root.join(JOURNAL_NAME)).map_err(write_failed)?;
    io_boundary("after_journal_publish").map_err(clean_pending)?;
    let published = open_task_sync(&root.join(JOURNAL_NAME)).map_err(clean_pending)?;
    published.sync_at("journal_sync").map_err(clean_pending)?;
    drop(published);
    finish_clean_locked(root, &journal, &journal_bytes)?;
    Ok(EditOutcome::Wrote {
        backup: journal.task.backup.map(|p| root.join(p)),
        #[cfg(test)]
        content: new_content,
    })
}

// ── Core flow (clock-injection seam, §G.1) ─────────────────────────────────
