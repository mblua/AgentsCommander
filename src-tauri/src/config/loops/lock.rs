#[cfg(test)]
use super::pause;
use super::{
    apply_loop_update_patch, baseline_loop_state, loop_dir, read_loop_config, read_loop_state,
    validate_loop_config, validate_loop_id, write_loop_config, write_loop_state_atomic,
    LoopConfigToml, LoopState, LoopUpdatePatch, LEGACY_LOOP_STATE_RENAME, LOOP_DIR_PREFIX,
};
use crate::config::local_config_io::{acquire_sidecar_write_lock, SidecarWriteLock};
use crate::config::naming_migration;
use crate::filesystem::loop_files;
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// #2682 - production deadline for [`acquire_loop_lock`].
pub const LOOP_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// #2682 - a held Loop lock; dropping it releases the OS lock.
pub(crate) type LoopDirLock = SidecarWriteLock;

/// #2682 - cross-process guard over one Loop's `config.toml` + `state.json` +
/// `audit.jsonl`, and over the existence of the Loop directory itself.
///
/// The sidecar is `<canonical ac_root>/._loop_<id>.lock`: outside the Loop
/// directory, so it resolves before `create` makes the directory and survives
/// `remove_dir_all`. Rule: in every section that reads, checks or mutates a
/// Loop, this is the first statement once the AC root and Loop id are known;
/// nothing that observes or mutates the Loop runs above it. The leaf writers
/// (`write_loop_config`, `write_loop_state_atomic`, `append_loop_audit_once`)
/// never take it: the OS lock excludes per handle, so a self-locking leaf
/// would deadlock against its own section. Lock order: `scan_lock` ->
/// `io_lock` -> this. Never held across an `.await`.
pub(crate) fn acquire_loop_lock(
    ac_root: &Path,
    loop_id: &str,
    timeout: Duration,
) -> Result<LoopDirLock, String> {
    acquire_loop_lock_in(
        ac_root,
        loop_id,
        timeout,
        crate::config::config_dir().as_deref(),
    )
}

/// #2718 (B4b) - [`acquire_loop_lock`] with an explicit journal directory. The
/// Loop state migration runs here, under the lock just taken, before the guard
/// is returned, so every Loop section passes through it.
pub(super) fn acquire_loop_lock_in(
    ac_root: &Path,
    loop_id: &str,
    timeout: Duration,
    journal_dir: Option<&Path>,
) -> Result<LoopDirLock, String> {
    validate_loop_id(loop_id)?;
    let canonical_root = loop_files::canonical_loop_root(ac_root).map_err(|e| {
        format!(
            "Failed to resolve AC root '{}' for Loop lock: {}",
            ac_root.display(),
            e
        )
    })?;
    let lock_path = canonical_root.join(format!(".{}{}.lock", LOOP_DIR_PREFIX, loop_id));
    let guard =
        acquire_sidecar_write_lock(&lock_path, timeout, "loopLockTimeout", "Loop write lock")?;
    migrate_loop_state_name(&canonical_root, loop_id, journal_dir);
    Ok(guard)
}

/// #2682 - [`acquire_loop_lock`] for a caller that holds only the Loop
/// directory path (`<ac_root>/_loop_<id>`), such as the scheduler.
pub(crate) fn acquire_loop_lock_for_dir(
    dir: &Path,
    timeout: Duration,
) -> Result<LoopDirLock, String> {
    acquire_loop_lock_for_dir_in(dir, timeout, crate::config::config_dir().as_deref())
}

/// #2718 (B4b) - [`acquire_loop_lock_for_dir`] with an explicit journal directory.
pub(super) fn acquire_loop_lock_for_dir_in(
    dir: &Path,
    timeout: Duration,
    journal_dir: Option<&Path>,
) -> Result<LoopDirLock, String> {
    let ac_root = dir
        .parent()
        .ok_or_else(|| format!("Loop directory {} has no parent", dir.display()))?;
    let loop_id = dir
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(LOOP_DIR_PREFIX))
        .ok_or_else(|| format!("{} is not a Loop directory", dir.display()))?;
    acquire_loop_lock_in(ac_root, loop_id, timeout, journal_dir)
}

/// #2718 (B4b) - one naming-migration scope per canonical Loop directory.
pub(super) fn loop_state_scope_key(loop_dir: &Path) -> String {
    format!("loop-state:{}", loop_dir.display())
}

/// #2718 (B4b) - moves `_loop_<id>/state.json` to `LOOP_STATE_FILE`, journalled
/// in `journal_dir`. It runs under the Loop lock the caller already holds and
/// takes **no** lock of its own: `lock_scope` would open that sidecar on a
/// second handle and deadlock against the caller (see [`acquire_loop_lock`]),
/// so `lock_scope_under_held(.., None, ..)` is used only for the proof token.
/// Fail-soft: a refusal leaves the Loop on its old name, which
/// `resolved_loop_state_path` still reads, and the next lock retries.
pub(super) fn migrate_loop_state_name(
    canonical_root: &Path,
    loop_id: &str,
    journal_dir: Option<&Path>,
) {
    let loop_dir = canonical_root.join(format!("{LOOP_DIR_PREFIX}{loop_id}"));
    if !loop_files::loop_path_is_directory(&loop_dir) {
        return;
    }
    let key = loop_state_scope_key(&loop_dir);
    let warn = |what: &str, refusal: naming_migration::Refusal| {
        let reason = match refusal {
            naming_migration::Refusal::LockUnavailable => "lock unavailable".to_string(),
            naming_migration::Refusal::Io(message) => message,
        };
        log::warn!(
            "[loops] #2718 - {what} for {}: {reason}; the Loop keeps its old state name until the \
                next lock",
            loop_dir.display()
        );
    };
    // `scope_is_settled`, never `is_complete`: a `Complete` the disk
    // contradicts is re-run.
    match naming_migration::read_journal(journal_dir) {
        Ok(journal) if naming_migration::scope_is_settled(journal.as_ref(), &key) => return,
        Ok(_) => {}
        Err(refusal) => return warn("the naming-migration journal is unreadable", refusal),
    }
    let held = match naming_migration::lock_scope_under_held(
        &loop_dir,
        None,
        naming_migration::MIGRATION_LOCK_BUDGET,
    ) {
        Ok(held) => held,
        Err(refusal) => return warn("the scope token failed", refusal),
    };
    // #2718 4.5: the new names are ignored before anything takes them.
    if let Err(refusal) = ensure_loop_state_ignore_rows(canonical_root) {
        return warn("the .ac/.gitignore sweep failed", refusal);
    }
    if let naming_migration::Outcome::Refused(refusal) = naming_migration::rename_step(
        &loop_dir,
        &LEGACY_LOOP_STATE_RENAME,
        &key,
        journal_dir,
        &held,
    ) {
        return warn("the state rename was refused", refusal);
    }
    if let Err(refusal) = naming_migration::update_journal(journal_dir, |j| {
        j.set_status(&key, naming_migration::ScopeStatus::Complete)
    }) {
        warn("the scope completion was not recorded", refusal);
    }
}

#[cfg(not(test))]
#[inline(always)]
pub(super) fn sweep_pause_hook(_stage: &str, _dir: &Path) {}

/// Test-only pause point between the sweep's read and its append. Inert unless
/// a test arms it.
#[cfg(test)]
pub(super) fn sweep_pause_hook(stage: &str, dir: &Path) {
    naming_migration::pause::hook(stage, dir);
}

/// #2718 (B4b) 4.5 - appends to `<ac_root>/.gitignore` the Loop state rows it
/// lacks. An appending write, never a rewrite, so no line another writer added
/// can be lost; retirement of the old row stays with the writer at registration.
pub(super) fn ensure_loop_state_ignore_rows(
    ac_root: &Path,
) -> Result<(), naming_migration::Refusal> {
    let path = ac_root.join(".gitignore");
    let io = |what: &str, e: std::io::Error| {
        naming_migration::Refusal::Io(format!("failed to {what} {}: {e}", path.display()))
    };
    let content = match loop_files::read_loop_document(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(io("read", e)),
    };
    let blocks = naming_migration::missing_ignore_rows(
        &content,
        &naming_migration::loop_state_ignore_rows(),
    );
    if blocks.is_empty() {
        return Ok(());
    }
    sweep_pause_hook("before_gitignore_append", ac_root);
    let separator = if content.is_empty() || content.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    loop_files::append_loop_ignore_rows(&path, &format!("{separator}{blocks}"))
        .map_err(|e| io("append to", e))
}

/// #2682 - the create section shared by the app command and the CLI: lock,
/// existence check, validation, then the config+state pair as one unit.
pub fn create_loop_files(
    project_dir: &Path,
    ac_root: &Path,
    config: &LoopConfigToml,
) -> Result<(PathBuf, LoopState), String> {
    let _lock = acquire_loop_lock(ac_root, &config.loop_def.id, LOOP_LOCK_TIMEOUT)?;
    if loop_files::loop_path_exists(&loop_dir(ac_root, &config.loop_def.id)) {
        return Err(format!("Loop '{}' already exists", config.loop_def.id));
    }
    validate_loop_config(project_dir, config)?;
    let dir = write_loop_config(ac_root, config)?;
    loop_write_pause_hook("between_config_and_state", &dir);
    let state = baseline_loop_state(config, Utc::now())?;
    write_loop_state_atomic(&dir, &state)?;
    Ok((dir, state))
}

/// #2682 - the read-modify-write section shared by the app `update_loop` /
/// `toggle_loop` and the CLI `update` / `enable` / `disable`.
pub fn update_loop_files(
    project_dir: &Path,
    ac_root: &Path,
    loop_id: &str,
    patch: LoopUpdatePatch,
) -> Result<(PathBuf, LoopConfigToml, LoopState), String> {
    let _lock = acquire_loop_lock(ac_root, loop_id, LOOP_LOCK_TIMEOUT)?;
    let dir = loop_dir(ac_root, loop_id);
    if !loop_files::loop_path_is_directory(&dir) {
        return Err(format!("Loop '{}' not found", loop_id));
    }
    let mut config = read_loop_config(&dir)?;
    let reset_schedule = apply_loop_update_patch(&mut config, patch)?;
    validate_loop_config(project_dir, &config)?;
    let dir = write_loop_config(ac_root, &config)?;
    loop_write_pause_hook("between_config_and_state", &dir);
    let state = if reset_schedule {
        baseline_loop_state(&config, Utc::now())?
    } else {
        read_loop_state(&dir).unwrap_or_default()
    };
    if reset_schedule {
        write_loop_state_atomic(&dir, &state)?;
    }
    Ok((dir, config, state))
}

/// #2682 - the delete section shared by the app `delete_loop` and the CLI
/// `remove`. The sidecar lives outside the directory it deletes.
pub fn remove_loop_files(ac_root: &Path, loop_id: &str) -> Result<PathBuf, String> {
    let _lock = acquire_loop_lock(ac_root, loop_id, LOOP_LOCK_TIMEOUT)?;
    let dir = loop_dir(ac_root, loop_id);
    if !loop_files::loop_path_is_directory(&dir) {
        return Err(format!("Loop '{}' not found", loop_id));
    }
    loop_files::remove_loop_directory(&dir)
        .map_err(|e| format!("Failed to remove Loop directory: {}", e))?;
    Ok(dir)
}

#[cfg(not(test))]
#[inline(always)]
pub(super) fn loop_write_pause_hook(_stage: &str, _dir: &Path) {}

/// #2682 - test-only pause point. Inert unless a test arms it.
#[cfg(test)]
pub(super) fn loop_write_pause_hook(stage: &str, dir: &Path) {
    pause::hook(stage, dir);
}
