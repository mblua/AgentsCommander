use super::lock::loop_write_pause_hook;
use super::{
    validate_loop_id, LoopAuditEntry, LoopConfigToml, LoopState, LOOP_AUDIT_FILE, LOOP_CONFIG_FILE,
    LOOP_DIR_PREFIX, LOOP_STATE_FILE,
};
use crate::config::naming_migration;
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub fn loop_dir(ac_root: &Path, id: &str) -> PathBuf {
    ac_root.join(format!("{}{}", LOOP_DIR_PREFIX, id))
}

pub fn read_loop_config(loop_dir: &Path) -> Result<LoopConfigToml, String> {
    let config_path = loop_dir.join(LOOP_CONFIG_FILE);
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;
    parse_loop_config(&config_path, &content)
}

/// Like `read_loop_config`, but a missing Loop directory or `config.toml`
/// is `Ok(None)` instead of a read error. A malformed file is still an error.
pub fn read_loop_config_if_present(loop_dir: &Path) -> Result<Option<LoopConfigToml>, String> {
    let config_path = loop_dir.join(LOOP_CONFIG_FILE);
    match std::fs::read_to_string(&config_path) {
        Ok(content) => parse_loop_config(&config_path, &content).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Failed to read {}: {}", config_path.display(), e)),
    }
}

pub(super) fn parse_loop_config(
    config_path: &Path,
    content: &str,
) -> Result<LoopConfigToml, String> {
    toml::from_str(content).map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopConfigRevalidation {
    Current,
    Gone,
    Disabled,
    Changed,
}

pub fn revalidate_loop_current(
    loop_dir: &Path,
    expected: &LoopConfigToml,
) -> Result<LoopConfigRevalidation, String> {
    if !loop_dir.is_dir() || !loop_dir.join(LOOP_CONFIG_FILE).is_file() {
        return Ok(LoopConfigRevalidation::Gone);
    }
    let current = read_loop_config(loop_dir)?;
    if !current.loop_def.enabled {
        return Ok(LoopConfigRevalidation::Disabled);
    }
    if loop_delivery_config_matches(&current, expected) {
        Ok(LoopConfigRevalidation::Current)
    } else {
        Ok(LoopConfigRevalidation::Changed)
    }
}

pub fn loop_delivery_config_matches(current: &LoopConfigToml, expected: &LoopConfigToml) -> bool {
    current.loop_def.id == expected.loop_def.id
        && current.loop_def.enabled == expected.loop_def.enabled
        && current.trigger.kind == expected.trigger.kind
        && current.trigger.expr == expected.trigger.expr
        && current.trigger.timezone == expected.trigger.timezone
        && current.target.kind == expected.target.kind
        && current.target.workgroup == expected.target.workgroup
        && current.prompt.body == expected.prompt.body
        && current.policy.missed_while_closed == expected.policy.missed_while_closed
        && current.policy.busy_coordinator == expected.policy.busy_coordinator
        && current.policy.session_start == expected.policy.session_start
}

pub fn write_loop_config(ac_root: &Path, config: &LoopConfigToml) -> Result<PathBuf, String> {
    validate_loop_id(&config.loop_def.id)?;
    let dir = loop_dir(ac_root, &config.loop_def.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create Loop directory: {}", e))?;
    let content = toml::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize Loop config: {}", e))?;
    // Write a sibling tmp file and replace, so no reader in any process
    // (including the CLI) ever sees a torn config.toml.
    let config_path = dir.join(LOOP_CONFIG_FILE);
    let tmp_path = dir.join(format!("{}.{}.tmp", LOOP_CONFIG_FILE, Uuid::new_v4()));
    write_config_via_tmp(&tmp_path, &config_path, &content)?;
    Ok(dir)
}

/// Writes `content` to `tmp_path` and replaces `config_path` with it. On any
/// failure, including a partial tmp write, the tmp file is removed.
pub(super) fn write_config_via_tmp(
    tmp_path: &Path,
    config_path: &Path,
    content: &str,
) -> Result<(), String> {
    std::fs::write(tmp_path, content)
        .and_then(|()| replace_file_with_retry(tmp_path, config_path))
        .map_err(|e| {
            let _ = std::fs::remove_file(tmp_path);
            format!("Failed to write Loop config: {}", e)
        })
}

pub fn read_loop_state(loop_dir: &Path) -> Result<LoopState, String> {
    read_loop_state_with_raw(loop_dir).map(|(state, _)| state)
}

/// Reads `state.json` and also returns its raw text (`None` when absent), the
/// expectation `write_loop_state_if_unchanged` compares against.
pub fn read_loop_state_with_raw(loop_dir: &Path) -> Result<(LoopState, Option<String>), String> {
    let state_path = resolved_loop_state_path(loop_dir);
    if !state_path.exists() {
        return Ok((LoopState::default(), None));
    }
    let content = std::fs::read_to_string(&state_path)
        .map_err(|e| format!("Failed to read {}: {}", state_path.display(), e))?;
    let state = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {}", state_path.display(), e))?;
    Ok((state, Some(content)))
}

/// #2718 (B4b) - the pre-migration state name, moved to `LOOP_STATE_FILE`.
pub(super) const LEGACY_LOOP_STATE_RENAME: naming_migration::Rename = naming_migration::Rename {
    from: "state.json",
    to: LOOP_STATE_FILE,
};

/// #2718 (B4b) - the state file an unlocked reader should read: the new name
/// when it exists, else the old name when that exists, else the new name. A
/// read fallback for a Loop whose lock has not been taken since the upgrade;
/// writers never use it, and the compare-and-swap does not either.
pub(crate) fn resolved_loop_state_path(loop_dir: &Path) -> PathBuf {
    let new = loop_dir.join(LOOP_STATE_FILE);
    if new.exists() {
        return new;
    }
    let old = loop_dir.join(LEGACY_LOOP_STATE_RENAME.from);
    if old.exists() {
        old
    } else {
        new
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopStateWrite {
    Written,
    Stale,
}

/// Compare-and-swap on `state.json`: writes only when the on-disk raw bytes
/// still equal `expected_raw` (absent included). Not a lock by itself; it is
/// atomic only for a caller that holds a lock across the read and this call.
pub fn write_loop_state_if_unchanged(
    loop_dir: &Path,
    state: &LoopState,
    expected_raw: Option<&str>,
) -> Result<LoopStateWrite, String> {
    // #2718 4.5 point 5: resolved once; the compare and the write share it.
    let state_path = resolved_loop_state_path(loop_dir);
    let current_raw = match std::fs::read_to_string(&state_path) {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("Failed to read {}: {}", state_path.display(), e)),
    };
    if current_raw.as_deref() != expected_raw {
        return Ok(LoopStateWrite::Stale);
    }
    loop_write_pause_hook("after_cas_compare", loop_dir);
    write_loop_state_atomic_at(loop_dir, &state_path, state)?;
    Ok(LoopStateWrite::Written)
}

/// #2718 4.5 point 4: resolves, so an unmigrated Loop keeps writing its old
/// name until a sweep and a rename have both succeeded.
pub fn write_loop_state_atomic(loop_dir: &Path, state: &LoopState) -> Result<(), String> {
    write_loop_state_atomic_at(loop_dir, &resolved_loop_state_path(loop_dir), state)
}

/// The atomic write onto an already resolved `state_path`; the temporary is
/// named after that file.
pub(super) fn write_loop_state_atomic_at(
    loop_dir: &Path,
    state_path: &Path,
    state: &LoopState,
) -> Result<(), String> {
    ensure_loop_config_present(loop_dir)?;
    let state_name = state_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(LOOP_STATE_FILE);
    let tmp_path = loop_dir.join(format!("{}.{}.tmp", state_name, Uuid::new_v4()));
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Failed to serialize Loop state: {}", e))?;
    std::fs::write(&tmp_path, content)
        .map_err(|e| format!("Failed to write temporary Loop state: {}", e))?;
    replace_file_with_retry(&tmp_path, state_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        format!("Failed to finalize Loop state: {}", e)
    })
}

/// Sleeps between replace attempts: 5 attempts, at most 150 ms added.
pub(super) const REPLACE_RETRY_DELAYS_MS: [u64; 4] = [10, 20, 40, 80];

/// `replace_file`, retried while the destination is transiently held open
/// (any open handle, even a fully shared reader, fails `MoveFileExW`).
/// Blocking `std::thread::sleep` on purpose: the callers are synchronous and
/// the CLI uses them too; 150 ms on an already-failing path is cheaper than
/// making the write path async.
pub(super) fn replace_file_with_retry(src: &Path, dst: &Path) -> std::io::Result<()> {
    let mut delays = REPLACE_RETRY_DELAYS_MS.iter();
    loop {
        match replace_file(src, dst) {
            Err(e) if is_transient_replace_error(&e) => match delays.next() {
                Some(ms) => std::thread::sleep(std::time::Duration::from_millis(*ms)),
                None => return Err(e),
            },
            result => return result,
        }
    }
}

#[cfg(windows)]
pub(super) fn is_transient_replace_error(e: &std::io::Error) -> bool {
    // ERROR_ACCESS_DENIED (what a held destination produces) and
    // ERROR_SHARING_VIOLATION.
    matches!(e.raw_os_error(), Some(5) | Some(32))
}

#[cfg(not(windows))]
pub(super) fn is_transient_replace_error(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::PermissionDenied
}

pub fn append_loop_audit_once(loop_dir: &Path, entry: &LoopAuditEntry) -> Result<(), String> {
    ensure_loop_config_present(loop_dir)?;
    let audit_path = loop_dir.join(LOOP_AUDIT_FILE);
    if audit_path.exists() {
        let content = std::fs::read_to_string(&audit_path)
            .map_err(|e| format!("Failed to read {}: {}", audit_path.display(), e))?;
        for line in content.lines().filter(|line| !line.trim().is_empty()) {
            match serde_json::from_str::<LoopAuditEntry>(line) {
                Ok(existing)
                    if existing.loop_id == entry.loop_id
                        && existing.run_id == entry.run_id
                        && existing.kind == entry.kind
                        && existing.due_at == entry.due_at =>
                {
                    return Ok(());
                }
                Ok(_) => {}
                Err(e) => log::warn!(
                    "[loops] Ignoring malformed audit line in {}: {}",
                    audit_path.display(),
                    e
                ),
            }
        }
    }

    let line = serde_json::to_string(entry)
        .map_err(|e| format!("Failed to serialize Loop audit entry: {}", e))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&audit_path)
        .map_err(|e| format!("Failed to open {}: {}", audit_path.display(), e))?;
    writeln!(file, "{}", line)
        .map_err(|e| format!("Failed to append {}: {}", audit_path.display(), e))
}

pub(super) fn ensure_loop_config_present(loop_dir: &Path) -> Result<(), String> {
    let config_path = loop_dir.join(LOOP_CONFIG_FILE);
    if config_path.is_file() {
        Ok(())
    } else {
        Err(format!(
            "Loop config no longer exists at {}",
            config_path.display()
        ))
    }
}

#[cfg(windows)]
pub(super) fn replace_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let src_wide: Vec<u16> = src.as_os_str().encode_wide().chain(Some(0)).collect();
    let dst_wide: Vec<u16> = dst.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        MoveFileExW(
            src_wide.as_ptr(),
            dst_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub(super) fn replace_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::rename(src, dst)
}
