use super::apply_edit;
use super::clean::clean_pair_locked;
use super::clean::recover_clean_pair_locked;
use super::is_user_owned_title;
use super::parse_task;
use super::render;
#[cfg(test)]
use super::status::io_boundary;
use super::title_value_of;
use super::EditOutcome;
use super::LockGuard;
use super::TaskOp;
use super::TaskOpError;
use super::LOCK_STALE_AFTER_5M;
use super::LOCK_TIMEOUT_5S;
use chrono::DateTime;
use chrono::Utc;
use std::fs::OpenOptions;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;

pub(crate) fn perform_inner<F>(
    wg_root: &Path,
    op: TaskOp,
    now: F,
) -> Result<EditOutcome, TaskOpError>
where
    F: FnOnce() -> DateTime<Utc>,
{
    let task_path = wg_root.join("TASK.md");
    let lock_path = wg_root.join("TASK.md.lock");
    // Per-PID tmp suffix eliminates the tmp-collision race during stale-lock
    // recovery (HIGH-2).
    let tmp_path = wg_root.join(format!("TASK.md.tmp.{}", std::process::id()));

    let _lock = LockGuard::acquire(&lock_path, LOCK_TIMEOUT_5S, LOCK_STALE_AFTER_5M)?;

    recover_clean_pair_locked(wg_root)?;

    // ── 2. Read existing content ───────────────────────────────────────────
    let (existing, file_existed) = match std::fs::read_to_string(&task_path) {
        Ok(s) => (s, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), false),
        Err(e) => return Err(TaskOpError::ReadFailed(task_path, e)),
    };

    if matches!(op, TaskOp::Clean) {
        return clean_pair_locked(wg_root, &existing, now());
    }

    // ── 2a. Capture pre-edit sentinel (HIGH-4) ────────────────────────────
    // Snapshot is taken AFTER the read, so an external write that lands in the
    // read→metadata window (~µs) is reflected in the captured snapshot rather
    // than detected at step 7a. Do NOT "tighten" by moving this BEFORE the
    // read: that would introduce an unbounded write-between-snapshot-and-read
    // window. The sentinel covers the realistic editor-save case (seconds apart).
    let pre_sentinel: Option<(u64, Option<SystemTime>)> = if file_existed {
        match std::fs::metadata(&task_path) {
            Ok(m) => Some((m.len(), m.modified().ok())),
            Err(_) => None,
        }
    } else {
        None
    };

    // ── 3-4. Parse + apply edit ───────────────────────────────────────────
    let parsed = parse_task(&existing);
    let current_title = title_value_of(&parsed);
    let current_is_user_owned = current_title.as_deref().is_some_and(is_user_owned_title);

    // #738 user-lock: a coordinator SetTitle must never overwrite a human-owned
    // (`USER:`) title. This is a successful outcome (exit 0): no write, no backup.
    // Ordered before the reserved-prefix check so a coordinator re-sending the
    // same `USER: X` is rejected here (not treated as invalid input).
    if matches!(op, TaskOp::SetTitle(_)) && current_is_user_owned {
        return Ok(EditOutcome::RejectedUserTitle {});
    }

    // #738 reserved-prefix: a coordinator SetTitle whose own input starts with
    // `USER:` is invalid input (it would forge a human lock) unless an existing
    // user-owned title already blocked the write above. Hard error, exit 1, no
    // write, no backup. SetUserTitle/creation are unaffected (human-owned paths).
    if let TaskOp::SetTitle(title) = &op {
        if is_user_owned_title(title) {
            return Err(TaskOpError::ReservedUserTitlePrefix);
        }
    }

    let new_parsed = apply_edit(&parsed, &op);

    // ── 5. Idempotence short-circuit ──────────────────────────────────────
    // SetTitle: semantic — short-circuit when YAML-decoded title value is
    //   unchanged (re-quoting/escaping never produces a NoOp).
    // Clean:    structural — short-circuit when post-edit frontmatter and
    //   body byte-match the pre-edit shape (covers repeated clean clicks).
    // AppendBody: never NoOp.
    let is_noop = match op {
        TaskOp::SetTitle(_) | TaskOp::SetUserTitle(_) => {
            title_value_of(&new_parsed) == current_title
        }
        TaskOp::Clean => {
            new_parsed.frontmatter == parsed.frontmatter && new_parsed.body == parsed.body
        }
        TaskOp::AppendBody(_) => false,
        TaskOp::SetBody(ref text) => *text == parsed.body,
    };
    if is_noop {
        return Ok(EditOutcome::NoOp {});
    }

    // ── 5b. Render to bytes for the upcoming write ────────────────────────
    let new_content = render(&new_parsed);

    // ── 6. Backup with collision-suffix loop (only if file existed) ───────
    let backup_path: Option<PathBuf> = if file_existed {
        // NOTE: backup filenames sort by wall-clock; an NTP backward correction
        // can break chronological ordering. Acceptable per spec; see plan #137 LOW-2.
        let ts = now().format("%Y%m%d-%H%M%S").to_string();
        let mut chosen: Option<PathBuf> = None;
        for n in 0..=99u32 {
            let candidate = if n == 0 {
                wg_root.join(format!("TASK.{}.bak.md", ts))
            } else {
                wg_root.join(format!("TASK.{}.{}.bak.md", ts, n))
            };
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(file) => {
                    drop(file);
                    chosen = Some(candidate);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(TaskOpError::BackupFailed(candidate, e)),
            }
        }
        let bp = chosen.ok_or_else(|| {
            TaskOpError::BackupExhausted(wg_root.join(format!("TASK.{}.bak.md", ts)))
        })?;
        match std::fs::copy(&task_path, &bp) {
            Ok(_) => Some(bp),
            Err(copy_err) => {
                // §C.1: fs::copy makes NO guarantee of partial-file cleanup.
                let _ = std::fs::remove_file(&bp);
                return Err(TaskOpError::BackupFailed(bp, copy_err));
            }
        }
    } else {
        None
    };

    // ── 7. Atomic write: tmp + sentinel-check + rename ────────────────────
    let write_tmp = || {
        #[cfg(test)]
        io_boundary("nonclean_tmp_write")?;
        std::fs::write(&tmp_path, &new_content)
    };
    if let Err(e) = write_tmp() {
        // MED-6 cleanup
        let _ = std::fs::remove_file(&tmp_path);
        return Err(TaskOpError::TmpWriteFailed(tmp_path, e));
    }

    // 7a. Sentinel check — see HIGH-4. Realistic editor-save case caught;
    // sub-millisecond TOCTOU at the read→metadata window remains theoretically open.
    // FAT32 mtime granularity is 2 s — for typical AC layouts (NTFS / EXT4 / APFS,
    // sub-second), this is not a concern.
    #[cfg(test)]
    io_boundary("nonclean_before_recheck").expect("mutation-only fixture boundary");
    if let Some((pre_len, pre_mtime)) = pre_sentinel {
        match std::fs::metadata(&task_path) {
            Ok(now_meta) => {
                let now_mtime = now_meta.modified().ok();
                let len_changed = now_meta.len() != pre_len;
                let mtime_changed = match (pre_mtime, now_mtime) {
                    (Some(a), Some(b)) => a != b,
                    _ => false,
                };
                if len_changed || mtime_changed {
                    let _ = std::fs::remove_file(&tmp_path);
                    let bp = backup_path
                        .clone()
                        .expect("file_existed ⇒ backup_path is Some");
                    return Err(TaskOpError::ExternalWrite(bp));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // External delete between read and rename. Without this branch,
                // rename to a vanished destination silently re-creates the file
                // (normal create on both Windows MoveFileExW(MOVEFILE_REPLACE_EXISTING)
                // and Unix), undoing the external delete. Treat as ExternalWrite.
                let _ = std::fs::remove_file(&tmp_path);
                let bp = backup_path
                    .clone()
                    .expect("file_existed ⇒ backup_path is Some");
                return Err(TaskOpError::ExternalWrite(bp));
            }
            Err(_) => { /* other transient FS error — let rename surface the real error */ }
        }
    }

    // 7b. Rename with retry on Windows AV/Explorer transient holds (MED-4).
    let do_rename = || -> Result<(), std::io::Error> {
        for attempt in 0..=2u32 {
            let rename = || {
                #[cfg(test)]
                io_boundary("nonclean_rename")?;
                std::fs::rename(&tmp_path, &task_path)
            };
            match rename() {
                Ok(_) => return Ok(()),
                Err(e) => {
                    let retry = e.kind() == std::io::ErrorKind::PermissionDenied
                        || e.raw_os_error() == Some(32) // ERROR_SHARING_VIOLATION
                        || e.raw_os_error() == Some(5); // ERROR_ACCESS_DENIED
                    if attempt < 2 && retry {
                        std::thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        unreachable!("loop body always returns")
    };

    if let Err(e) = do_rename() {
        // MED-1 cleanup — keeps I20's "no TASK.md.tmp.* litter" assertion holding.
        let _ = std::fs::remove_file(&tmp_path);
        return Err(TaskOpError::RenameFailed(e, backup_path));
    }

    Ok(EditOutcome::Wrote {
        backup: backup_path,
        #[cfg(test)]
        content: new_content,
    })
}
