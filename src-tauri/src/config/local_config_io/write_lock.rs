use crate::filesystem::local_config;
use std::cell::Cell;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::time::Duration;

pub(crate) use crate::filesystem::local_config::SidecarWriteLock;

pub(super) fn local_config_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// #2786 (C1) - take the process-wide write `Mutex`, recovering from poison.
/// The `Mutex<()>` guards no in-memory invariant, it only serializes writers:
/// the data lives on disk under the file sidecar and every reader re-reads it.
/// So a panic inside one mutate closure must not block every later config
/// write in the process until restart, which `map_err` on poison did.
pub(super) fn lock_local_config_writes() -> MutexGuard<'static, ()> {
    local_config_write_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

thread_local! {
    /// #2786 (C1) - "a config writer is active on this thread".
    static CONFIG_WRITER_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// #2786 (C1) - the re-entrancy flag, held for a writer's whole call. The
/// process `Mutex` is not reentrant, so a config writer called from inside a
/// mutate closure or a stage hook on the same thread would deadlock silently;
/// each entry point checks this flag BEFORE touching the `Mutex` and returns
/// an error instead. `Drop` clears the flag, so an early `?` return and a panic
/// both leave the thread usable. Same-thread recursion only: a callback that
/// blocks on a writer running on another thread is not detected.
pub(super) struct ConfigWriterActive;

impl ConfigWriterActive {
    pub(super) fn enter(entry_point: &str) -> Result<Self, String> {
        if CONFIG_WRITER_ACTIVE.with(|active| active.replace(true)) {
            return Err(format!(
                "Nested config write: {} was called while a config writer \
                 is already active on this thread",
                entry_point
            ));
        }
        Ok(Self)
    }
}

impl Drop for ConfigWriterActive {
    fn drop(&mut self) {
        CONFIG_WRITER_ACTIVE.with(|active| active.set(false));
    }
}

/// #1938 - cooperative cross-process serialization for local config writes.
///
/// The config file itself is replaced by every publish (rename on Unix,
/// ReplaceFileW on Windows), so locking it would guard an inode that is about
/// to disappear. The lock lives on a stable sidecar instead, `.<name>.lock`
/// next to the config file: created once, never deleted - including on error
/// paths - so every process locks the same file. Closing the handle (normal
/// drop, panic, or process death) releases the OS lock.
pub(super) const CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// #1938 - the physical sidecar for `config_path` once its parent has resolved
/// to the canonical directory, so raw, dot-segment, symlink and Windows case /
/// verbatim aliases of one directory all share one lock file.
pub(super) fn config_lock_path(parent: &Path, config_path: &Path) -> Result<PathBuf, String> {
    let file_name = config_path
        .file_name()
        .ok_or_else(|| format!("Local config {} has no file name", config_path.display()))?;
    let mut lock_name = std::ffi::OsString::from(".");
    lock_name.push(file_name);
    lock_name.push(".lock");
    Ok(parent.join(lock_name))
}

/// #1938 - open (creating once) and acquire the sidecar lock for `config_path`.
///
/// `timeout` is the whole acquire deadline: production passes
/// [`CONFIG_LOCK_TIMEOUT`]; tests pass a short value so the timeout path is
/// exercised without a five-second wait. A live holder that never releases
/// returns a distinct `configLockTimeout` error; an unrecoverable OS error stops
/// immediately (a network mount without compatible lock support fails visibly
/// instead of silently degrading to process-only exclusion); a crashed holder
/// has already released the lock in the kernel.
pub(super) fn acquire_config_file_write_lock(
    config_path: &Path,
    timeout: Duration,
) -> Result<SidecarWriteLock, String> {
    let parent = config_path.parent().ok_or_else(|| {
        format!(
            "Local config {} has no parent directory",
            config_path.display()
        )
    })?;
    let canonical_parent = local_config::canonical_local_config_parent(parent).map_err(|e| {
        format!(
            "Failed to resolve local config directory '{}' for write lock: {}",
            parent.display(),
            e
        )
    })?;
    let lock_path = config_lock_path(&canonical_parent, config_path)?;
    acquire_sidecar_write_lock(
        &lock_path,
        timeout,
        "configLockTimeout",
        "local config write lock",
    )
}

/// #1938 - open (creating once) and acquire the sidecar lock at `lock_path`.
/// The caller resolves the path; `timeout_marker` prefixes the deadline error
/// so each caller's timeout stays distinguishable. Shared by the local-config
/// lock and the Loop lock (#2682); this is the one `try_lock` poll loop.
pub(crate) fn acquire_sidecar_write_lock(
    lock_path: &Path,
    timeout: Duration,
    timeout_marker: &str,
    subject: &str,
) -> Result<SidecarWriteLock, String> {
    local_config::acquire_local_config_sidecar_lock(lock_path, timeout, timeout_marker, subject)
}
