use serde_json::{Map, Value};
use std::cell::Cell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

fn local_config_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// #2786 (C1) - take the process-wide write `Mutex`, recovering from poison.
/// The `Mutex<()>` guards no in-memory invariant, it only serializes writers:
/// the data lives on disk under the file sidecar and every reader re-reads it.
/// So a panic inside one mutate closure must not block every later config
/// write in the process until restart, which `map_err` on poison did.
fn lock_local_config_writes() -> MutexGuard<'static, ()> {
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
struct ConfigWriterActive;

impl ConfigWriterActive {
    fn enter(entry_point: &str) -> Result<Self, String> {
        if CONFIG_WRITER_ACTIVE.with(|active| active.replace(true)) {
            return Err(format!(
                "Nested config write: {} was called while a config writer is already active on this thread",
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
const CONFIG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// #1938 - a held sidecar lock. The `File` IS the lock: dropping this guard
/// closes the handle and releases the OS lock. The sidecar file itself is
/// deliberately left on disk.
#[derive(Debug)]
pub(crate) struct SidecarWriteLock {
    _file: std::fs::File,
}

/// #1938 - the physical sidecar for `config_path` once its parent has resolved
/// to the canonical directory, so raw, dot-segment, symlink and Windows case /
/// verbatim aliases of one directory all share one lock file.
fn config_lock_path(parent: &Path, config_path: &Path) -> Result<PathBuf, String> {
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
fn acquire_config_file_write_lock(
    config_path: &Path,
    timeout: Duration,
) -> Result<SidecarWriteLock, String> {
    let parent = config_path.parent().ok_or_else(|| {
        format!(
            "Local config {} has no parent directory",
            config_path.display()
        )
    })?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|e| {
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
    let mut chars = subject.chars();
    let subject_capitalized: String = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    match std::fs::symlink_metadata(lock_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!(
                "{} '{}' must be a regular non-symlink file",
                subject_capitalized,
                lock_path.display()
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to inspect {} '{}': {}",
                subject,
                lock_path.display(),
                e
            ));
        }
    }

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| {
            format!(
                "Failed to open {} '{}': {}",
                subject,
                lock_path.display(),
                e
            )
        })?;
    if !file
        .metadata()
        .map_err(|e| {
            format!(
                "Failed to inspect opened {} '{}': {}",
                subject,
                lock_path.display(),
                e
            )
        })?
        .is_file()
    {
        return Err(format!(
            "{} '{}' must be a regular file",
            subject_capitalized,
            lock_path.display()
        ));
    }

    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() >= timeout => {
                return Err(format!(
                    "{}: timed out after {} ms waiting for {} '{}'",
                    timeout_marker,
                    timeout.as_millis(),
                    subject,
                    lock_path.display()
                ));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                std::thread::sleep(CONFIG_LOCK_POLL_INTERVAL.min(remaining));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(format!(
                    "Failed to acquire {} '{}': {}",
                    subject,
                    lock_path.display(),
                    e
                ));
            }
        }
    }

    Ok(SidecarWriteLock { _file: file })
}

pub fn update_config_json_object<F>(
    path: &Path,
    allow_create: bool,
    mutate: F,
) -> Result<Value, String>
where
    F: FnOnce(&mut Map<String, Value>) -> Result<(), String>,
{
    update_config_json_object_with_publish(path, allow_create, mutate, publish_temp_config)
}

/// #1938 - the publish step is a parameter so tests can force a publish failure
/// without a second copy of the read-modify-write body (mirrors
/// `write_file_atomic_with_publish`). Production callers keep using
/// `update_config_json_object`, which passes the real atomic publisher.
fn update_config_json_object_with_publish<F, P>(
    path: &Path,
    allow_create: bool,
    mutate: F,
    publish: P,
) -> Result<Value, String>
where
    F: FnOnce(&mut Map<String, Value>) -> Result<(), String>,
    P: FnOnce(&Path, &Path) -> Result<(), String>,
{
    let _writer = ConfigWriterActive::enter("update_config_json_object")?;
    let _guard = lock_local_config_writes();

    // #1938 - the parent must exist before the sidecar can be resolved, and the
    // sidecar lock must be held before the existence check, the read, the
    // mutate closure, the temp write and the publish. The whole read-modify-
    // publish cycle therefore runs under one cross-process guard; moving any of
    // it above the lock reopens the lost-update window this phase closes.
    let parent = path
        .parent()
        .ok_or_else(|| format!("Local config {} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    let _file_lock = acquire_config_file_write_lock(path, CONFIG_LOCK_TIMEOUT)?;

    let mut root = if path.exists() {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let parsed: Value = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
        if !parsed.is_object() {
            return Err(format!(
                "Local config {} must be a JSON object",
                path.display()
            ));
        }
        parsed
    } else if allow_create {
        Value::Object(Map::new())
    } else {
        return Err(format!("Local config {} does not exist", path.display()));
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| format!("Local config {} must be a JSON object", path.display()))?;
    mutate(obj)?;

    let mut json = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("Failed to serialize {}: {}", path.display(), e))?;
    json.push('\n');

    let tmp_path = temp_config_path(path);

    let write_result = (|| -> Result<(), String> {
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temp config {}: {}", tmp_path.display(), e))?;
        file.write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write temp config {}: {}", tmp_path.display(), e))?;
        file.flush()
            .map_err(|e| format!("Failed to flush temp config {}: {}", tmp_path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync temp config {}: {}", tmp_path.display(), e))
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    if let Err(e) = publish(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    Ok(root)
}

pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_file_atomic_with_publish(path, bytes, publish_temp_config)
}

fn write_file_atomic_with_publish<P>(path: &Path, bytes: &[u8], publish: P) -> Result<(), String>
where
    P: FnOnce(&Path, &Path) -> Result<(), String>,
{
    let _writer = ConfigWriterActive::enter("write_file_atomic")?;
    let _guard = lock_local_config_writes();

    let parent = path
        .parent()
        .ok_or_else(|| format!("Local config {} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    let tmp_path = temp_config_path(path);

    let write_result = (|| -> Result<(), String> {
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temp config {}: {}", tmp_path.display(), e))?;
        file.write_all(bytes)
            .map_err(|e| format!("Failed to write temp config {}: {}", tmp_path.display(), e))?;
        file.flush()
            .map_err(|e| format!("Failed to flush temp config {}: {}", tmp_path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync temp config {}: {}", tmp_path.display(), e))
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    if let Err(e) = publish(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    Ok(())
}

/// #2786 (C1) - the caller-supplied cleanup sequence of `update_config_pair`:
/// it edits the decisions map and the state map before `mutate` runs.
pub(crate) type ConfigPairCleanup<'a> =
    &'a dyn Fn(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>;

/// #2786 (C1) - write the decisions file and the state file as one guarded
/// pair. `state` is the state file's path, `state_keys` the keys that live in
/// it, and `on_stage` a named pause point (a no-op in production); all three
/// are parameters so this module names no other module.
///
/// One critical section covers both sequences: the process `Mutex`, then the
/// decisions sidecar, then the state sidecar, each taken once for the whole
/// call. Inside it the optional `cleanup` runs first, then `mutate`, on the
/// same two maps read once. Each sequence publishes the state file before the
/// decisions file, and a side whose map the sequence did not change is
/// neither written nor created, so its stage does not fire. An absent state
/// file is an empty map; an unparseable one blocks the call before any write.
/// A `cleanup` error aborts before any write; a `mutate` error keeps whatever
/// the cleanup already published. The primitive moves, removes and marks no
/// key itself, deletes no file, and never rolls back or retries.
///
/// Neither closure may call a config writer: a nested call on the same thread
/// returns an error, and one waited on from another thread would deadlock.
#[cfg(test)]
fn update_config_pair<F>(
    decisions: &Path,
    state: &Path,
    state_keys: &[&str],
    cleanup: Option<ConfigPairCleanup<'_>>,
    on_stage: &dyn Fn(&str),
    mutate: F,
) -> Result<(), String>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    update_config_pair_inner(
        decisions,
        state,
        None,
        state_keys,
        cleanup,
        on_stage,
        mutate,
        &|_| Ok(()),
        None,
        None,
    )
}

/// The production PAIR boundary. Pending deltas are computed on private maps
/// before cleanup or the caller's split-marker policy can change either file.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_config_pair_guarded<F>(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    state_keys: &[&str],
    nonpending_preflight: Option<&dyn Fn() -> Result<(), String>>,
    cleanup: Option<ConfigPairCleanup<'_>>,
    on_stage: &dyn Fn(&str),
    mutate: F,
    finish: &dyn Fn(&mut Map<String, Value>) -> Result<(), String>,
    activity: Option<(&str, &Cell<bool>)>,
) -> Result<(), String>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    update_config_pair_inner(
        decisions,
        state,
        Some(reservation),
        state_keys,
        cleanup,
        on_stage,
        mutate,
        finish,
        activity,
        nonpending_preflight,
    )
}

#[allow(clippy::too_many_arguments)]
fn update_config_pair_inner<F>(
    decisions: &Path,
    state: &Path,
    reservation: Option<&Path>,
    state_keys: &[&str],
    cleanup: Option<ConfigPairCleanup<'_>>,
    on_stage: &dyn Fn(&str),
    mutate: F,
    finish: &dyn Fn(&mut Map<String, Value>) -> Result<(), String>,
    activity: Option<(&str, &Cell<bool>)>,
    nonpending_preflight: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<(), String>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    let _writer = ConfigWriterActive::enter("update_config_pair")?;
    let _guard = lock_local_config_writes();
    // C1 moves no key, so the list is carried for the cleanup policy of C2.
    let _ = state_keys;

    for path in [decisions, state] {
        let parent = path
            .parent()
            .ok_or_else(|| format!("Local config {} has no parent directory", path.display()))?;
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    if let Some(path) = reservation {
        validate_pair_paths(decisions, state, path).map_err(|e| e.to_string())?;
    }
    let _decisions_lock = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)?;
    // Every reservation publisher takes decisions first. Keep this lease while
    // classifying presence, so a packet cannot appear during ignore preflight.
    // Protect state/reservation artifacts before creating their sidecars, even
    // when a later read fails. Pending or unreadable packets never preflight.
    if let Some(path) = reservation {
        if read_pair_reservation(path)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            if let Some(preflight) = nonpending_preflight {
                preflight()?;
            }
        }
    }
    let _state_lock = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)?;

    let _reservation_lock = reservation
        .map(|path| acquire_config_file_write_lock(path, CONFIG_LOCK_TIMEOUT))
        .transpose()?;
    if let Some(path) = reservation {
        if let Some((mut packet, mut physical)) =
            read_pair_reservation(path).map_err(|e| e.to_string())?
        {
            validate_pair_binding(decisions, state, &packet).map_err(|e| e.to_string())?;
            if let Some((input, landed)) = activity {
                landed.set(
                    land_pair_activity(
                        decisions,
                        state,
                        path,
                        &mut packet,
                        &mut physical,
                        input,
                        &|_| Ok(()),
                    )
                    .map_err(|e| e.to_string())?,
                );
                return Ok(());
            }
            let current = read_pair_tuple(decisions, state).map_err(|e| e.to_string())?;
            packet.recognize(&current).map_err(|e| e.to_string())?;
            let mut d = current[0].map().map_err(|e| e.to_string())?;
            let mut s = current[1].map().map_err(|e| e.to_string())?;
            mutate(&mut d, &mut s)?;
            let proposed = [
                prepared_pair_image(
                    &current[0],
                    &current[0].map().map_err(|e| e.to_string())?,
                    &d,
                )
                .map_err(|e| e.to_string())?,
                prepared_pair_image(
                    &current[1],
                    &current[1].map().map_err(|e| e.to_string())?,
                    &s,
                )
                .map_err(|e| e.to_string())?,
            ];
            if proposed == current {
                return Ok(());
            }
            // Structural changes, history, selection and unknown fields all
            // remain frozen. Only the exact timestamp leaf may move forward.
            if proposed[0] != current[0]
                || protected_revision(&proposed[1], true).map_err(|e| e.to_string())?
                    != protected_revision(&current[1], true).map_err(|e| e.to_string())?
            {
                // Absent state permits only its timestamp-only overlay.
                if proposed[0] != current[0]
                    || current[1] != PhysicalState::Absent
                    || !timestamp_only_overlay(&proposed[1]).map_err(|e| e.to_string())?
                {
                    return Err("targetTransitionPending: PAIR is reserved".into());
                }
            }
            let input = timestamp_value(&proposed[1])
                .map_err(|e| e.to_string())?
                .ok_or_else(|| {
                    "targetTransitionPending: timestamp cannot be deleted".to_string()
                })?;
            if timestamp_value(&current[1])
                .map_err(|e| e.to_string())?
                .as_deref()
                .is_some_and(|old| timestamp_cmp(&input, old).is_ok_and(|order| order.is_lt()))
            {
                return Err("targetTransitionPending: timestamp cannot decrease".into());
            }
            land_pair_activity(
                decisions,
                state,
                path,
                &mut packet,
                &mut physical,
                &input,
                &|_| Ok(()),
            )
            .map_err(|e| e.to_string())?;
            return Ok(());
        }
    }

    let both = || format!("{} and {}", decisions.display(), state.display());
    let read = |path: &Path| {
        read_pair_side(path).map_err(|e| format!("Config pair {} blocked: {}", both(), e))
    };
    let mut decisions_map = read(decisions)?;
    let mut state_map = read(state)?;

    if let Some(cleanup) = cleanup {
        let decisions_before = decisions_map.clone();
        let state_before = state_map.clone();
        cleanup(&mut decisions_map, &mut state_map)
            .map_err(|e| format!("Config pair {} cleanup failed: {}", both(), e))?;
        publish_pair_sides(
            (decisions, &decisions_before, &decisions_map),
            (state, &state_before, &state_map),
            on_stage,
            ["after_cleanup_state_publish", "after_cleanup_tracked_write"],
        )?;
    }

    let decisions_before = decisions_map.clone();
    let state_before = state_map.clone();
    mutate(&mut decisions_map, &mut state_map)?;
    finish(&mut state_map)?;
    publish_pair_sides(
        (decisions, &decisions_before, &decisions_map),
        (state, &state_before, &state_map),
        on_stage,
        ["after_caller_state_publish", "after_caller_decisions_write"],
    )
}

/// Exact private disk image. Absence is distinct from an existing empty object.
/// Bytes are recovery authority; the digest is checked when a plan is executed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
pub(crate) enum PhysicalState {
    Absent,
    Bytes { sha256: String, bytes: Vec<u8> },
}

impl PhysicalState {
    #[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
    pub(crate) fn from_bytes(bytes: Vec<u8>) -> Self {
        use sha2::{Digest, Sha256};
        Self::Bytes {
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
    fn map(&self) -> Result<Map<String, Value>, PreparedPairError> {
        match self {
            Self::Absent => Ok(Map::new()),
            Self::Bytes { bytes, .. } => {
                if Self::from_bytes(bytes.clone()) != *self {
                    return Err(PreparedPairError::InvalidPlan("physical digest mismatch"));
                }
                match serde_json::from_slice(bytes) {
                    Ok(Value::Object(map)) => Ok(map),
                    _ => Err(PreparedPairError::InvalidPlan(
                        "physical image must be a JSON object",
                    )),
                }
            }
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
pub(crate) fn read_config_pair_physical(path: &Path) -> Result<PhysicalState, PreparedPairError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(PreparedPairError::Io(
                "PAIR target must be a regular non-symlink file".into(),
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(PhysicalState::Absent),
        Err(e) => return Err(PreparedPairError::Io(e.to_string())),
    }
    std::fs::read(path)
        .map(PhysicalState::from_bytes)
        .map_err(|e| PreparedPairError::Io(e.to_string()))
}

/// T0=(D0,S0), T1=(D0,Sc), T2=(Dc,Sc), T3=(Dc,Sf), T4=(Df,Sf).
/// No locator, callback or journal is persisted here. The authorized caller
/// supplies the target and persists this private plan in its own protocol.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
pub(crate) struct PreparedConfigPairPlan {
    stages: [[PhysicalState; 2]; 5],
}

impl PreparedConfigPairPlan {
    #[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
    pub(crate) fn stages(&self) -> &[[PhysicalState; 2]; 5] {
        &self.stages
    }

    #[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
    fn validate(&self) -> Result<(), PreparedPairError> {
        for tuple in &self.stages {
            for image in tuple {
                image.map()?;
            }
        }
        for (stage, changed_side) in [(1, 1), (2, 0), (3, 1), (4, 0)] {
            if self.stages[stage][1 - changed_side] != self.stages[stage - 1][1 - changed_side]
                || (self.stages[stage][changed_side] == PhysicalState::Absent
                    && self.stages[stage - 1][changed_side] != PhysicalState::Absent)
            {
                return Err(PreparedPairError::InvalidPlan(
                    "invalid ordered PAIR images",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
pub(crate) enum PreparedPairError {
    InvalidPlan(&'static str),
    Preparation(String),
    Conflict,
    Pending,
    Io(String),
}

impl std::fmt::Display for PreparedPairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPlan(reason) => write!(f, "invalidPreparedPair: {reason}"),
            Self::Preparation(reason) => write!(f, "preparedPairRejected: {reason}"),
            Self::Conflict => {
                f.write_str("preparedPairConflict: current tuple is not an authorized stage")
            }
            Self::Pending => {
                f.write_str("targetTransitionPending: reserved target is not finalized")
            }
            Self::Io(reason) => write!(f, "preparedPairIo: {reason}"),
        }
    }
}

impl std::error::Error for PreparedPairError {}

#[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
fn prepared_pair_image(
    previous: &PhysicalState,
    before: &Map<String, Value>,
    after: &Map<String, Value>,
) -> Result<PhysicalState, PreparedPairError> {
    if before == after {
        return Ok(previous.clone());
    }
    let mut bytes = serde_json::to_vec_pretty(after)
        .map_err(|e| PreparedPairError::Preparation(e.to_string()))?;
    bytes.push(b'\n');
    Ok(PhysicalState::from_bytes(bytes))
}

/// Pure preparation. Both closures finish before any publish. Reuses the
/// writer's map equality and pretty+newline serializer; untouched bytes survive.
/// Closures must only edit the supplied maps, never enter another writer.
#[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
pub(crate) fn prepare_config_pair_plan<F>(
    before: [PhysicalState; 2],
    cleanup: Option<ConfigPairCleanup<'_>>,
    mutate: F,
) -> Result<PreparedConfigPairPlan, PreparedPairError>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    let _writer = ConfigWriterActive::enter("prepare_config_pair_plan")
        .map_err(PreparedPairError::Preparation)?;
    prepare_config_pair_plan_inner(before, cleanup, mutate)
}

fn prepare_config_pair_plan_inner<F>(
    before: [PhysicalState; 2],
    cleanup: Option<ConfigPairCleanup<'_>>,
    mutate: F,
) -> Result<PreparedConfigPairPlan, PreparedPairError>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    let mut decisions = before[0].map()?;
    let mut state = before[1].map()?;
    let original_decisions = decisions.clone();
    let original_state = state.clone();
    if let Some(cleanup) = cleanup {
        cleanup(&mut decisions, &mut state).map_err(PreparedPairError::Preparation)?;
    }
    let dc = prepared_pair_image(&before[0], &original_decisions, &decisions)?;
    let sc = prepared_pair_image(&before[1], &original_state, &state)?;
    let clean_decisions = decisions.clone();
    let clean_state = state.clone();
    mutate(&mut decisions, &mut state).map_err(PreparedPairError::Preparation)?;
    let df = prepared_pair_image(&dc, &clean_decisions, &decisions)?;
    let sf = prepared_pair_image(&sc, &clean_state, &state)?;
    Ok(PreparedConfigPairPlan {
        stages: [
            before.clone(),
            [before[0].clone(), sc.clone()],
            [dc.clone(), sc],
            [dc, sf.clone()],
            [df, sf],
        ],
    })
}

#[cfg(test)]
pub(crate) fn execute_prepared_config_pair(
    decisions: &Path,
    state: &Path,
    plan: &PreparedConfigPairPlan,
) -> Result<(), PreparedPairError> {
    execute_prepared_config_pair_with_stage(decisions, state, plan, &|_| Ok(()))
}

/// One lock acquisition per side, for the whole execution. The stage seam is
/// private and exercises partial failure in the production execution body.
#[cfg(test)]
fn execute_prepared_config_pair_with_stage(
    decisions: &Path,
    state: &Path,
    plan: &PreparedConfigPairPlan,
    on_stage: &dyn Fn(usize) -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    execute_unreserved_config_pair_inner(decisions, state, None, plan, on_stage)
}

/// Retained full-physical P10 execution refuses a durable reservation under
/// the same PAIR locks; it cannot bypass the owner/protected-activity protocol.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn execute_unreserved_config_pair(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    plan: &PreparedConfigPairPlan,
) -> Result<(), PreparedPairError> {
    execute_unreserved_config_pair_inner(decisions, state, Some(reservation), plan, &|_| Ok(()))
}

fn execute_unreserved_config_pair_inner(
    decisions: &Path,
    state: &Path,
    reservation: Option<&Path>,
    plan: &PreparedConfigPairPlan,
    on_stage: &dyn Fn(usize) -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    let _writer = ConfigWriterActive::enter("execute_prepared_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    plan.validate()?;
    let resolve = |path: &Path| -> Result<PathBuf, PreparedPairError> {
        let parent = path
            .parent()
            .ok_or(PreparedPairError::InvalidPlan("PAIR target has no parent"))?;
        let name = path
            .file_name()
            .ok_or(PreparedPairError::InvalidPlan("PAIR target has no name"))?;
        let parent =
            std::fs::canonicalize(parent).map_err(|e| PreparedPairError::Io(e.to_string()))?;
        Ok(parent.join(name))
    };
    let decisions = resolve(decisions)?;
    let state = resolve(state)?;
    let same_target = decisions == state
        || (cfg!(windows)
            && decisions.to_string_lossy().to_lowercase()
                == state.to_string_lossy().to_lowercase());
    if same_target {
        return Err(PreparedPairError::InvalidPlan(
            "PAIR targets must be distinct",
        ));
    }
    if let Some(path) = reservation {
        validate_pair_paths(&decisions, &state, path)?;
    }
    let _guard = lock_local_config_writes();
    let _decisions_lock = acquire_config_file_write_lock(&decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _state_lock = acquire_config_file_write_lock(&state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _reservation_lock = reservation
        .map(|path| acquire_config_file_write_lock(path, CONFIG_LOCK_TIMEOUT))
        .transpose()
        .map_err(PreparedPairError::Io)?;
    if let Some(path) = reservation {
        if read_pair_reservation(path)?.is_some() {
            return Err(PreparedPairError::Pending);
        }
    }
    let read = || -> Result<[PhysicalState; 2], PreparedPairError> {
        Ok([
            read_config_pair_physical(&decisions)?,
            read_config_pair_physical(&state)?,
        ])
    };
    let current: [PhysicalState; 2] = read()?;
    let start = plan
        .stages
        .iter()
        .rposition(|tuple| *tuple == current)
        .ok_or(PreparedPairError::Conflict)?;
    for stage in start + 1..5 {
        if read()? != plan.stages[stage - 1] {
            return Err(PreparedPairError::Conflict);
        }
        let side = if stage == 1 || stage == 3 { 1 } else { 0 };
        if plan.stages[stage][side] != plan.stages[stage - 1][side] {
            let path = if side == 0 { &decisions } else { &state };
            publish_prepared_pair_image(
                path,
                &plan.stages[stage - 1][side],
                &plan.stages[stage][side],
            )?;
            if read()? != plan.stages[stage] {
                return Err(PreparedPairError::Conflict);
            }
            on_stage(stage)?;
        }
    }
    if read()? != plan.stages[4] {
        return Err(PreparedPairError::Conflict);
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))] // P10: inactive until P11 activation.
fn publish_prepared_pair_image(
    path: &Path,
    before: &PhysicalState,
    after: &PhysicalState,
) -> Result<(), PreparedPairError> {
    let PhysicalState::Bytes { bytes, .. } = after else {
        return Err(PreparedPairError::InvalidPlan("PAIR never deletes a side"));
    };
    let tmp = temp_config_path(path);
    let result = (|| {
        let mut file =
            std::fs::File::create(&tmp).map_err(|e| PreparedPairError::Io(e.to_string()))?;
        file.write_all(bytes)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        drop(file);
        if read_config_pair_physical(path)? != *before {
            return Err(PreparedPairError::Conflict);
        }
        if *before == PhysicalState::Absent {
            // Atomic create-if-absent; rename would clobber a racing Unix creator.
            std::fs::hard_link(&tmp, path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    PreparedPairError::Conflict
                } else {
                    PreparedPairError::Io(e.to_string())
                }
            })?;
            std::fs::remove_file(&tmp).map_err(|e| PreparedPairError::Io(e.to_string()))?;
        } else {
            publish_temp_config(&tmp, path).map_err(PreparedPairError::Io)?;
        }
        #[cfg(not(windows))]
        std::fs::File::open(
            path.parent()
                .ok_or(PreparedPairError::InvalidPlan("PAIR target has no parent"))?,
        )
        .and_then(|dir| dir.sync_all())
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        Ok(())
    })();
    if result.is_err() {
        if let Err(e) = std::fs::remove_file(&tmp) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("[prepared-pair] temporary cleanup failed: {e}");
            }
        }
    }
    result
}

/// Canonical JSON with structural presence retained. Only the state timestamp
/// is removed; decisions, nulls, empty objects and unknown values remain data.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ProtectedRevision {
    Absent,
    Json(Value),
}

fn protected_revision(
    image: &PhysicalState,
    state: bool,
) -> Result<ProtectedRevision, PreparedPairError> {
    if *image == PhysicalState::Absent {
        return Ok(ProtectedRevision::Absent);
    }
    let mut map = image.map()?;
    if state {
        timestamp_value(image)?;
        if let Some(tooling) = map.get_mut("tooling") {
            tooling
                .as_object_mut()
                .ok_or(PreparedPairError::Conflict)?
                .remove("lastAgentMessageAt");
        }
    }
    Ok(ProtectedRevision::Json(canonical_json(Value::Object(map))))
}

fn canonical_json(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut rows: Vec<_> = map.into_iter().collect();
            rows.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                rows.into_iter()
                    .map(|(key, value)| (key, canonical_json(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        value => value,
    }
}

fn changed_pair_sides(plan: &PreparedConfigPairPlan) -> [bool; 2] {
    [0, 1].map(|side| {
        plan.stages
            .windows(2)
            .any(|images| images[0][side] != images[1][side])
    })
}

fn timestamp_value(image: &PhysicalState) -> Result<Option<String>, PreparedPairError> {
    let map = image.map()?;
    let Some(tooling) = map.get("tooling") else {
        return Ok(None);
    };
    let tooling = tooling.as_object().ok_or(PreparedPairError::Conflict)?;
    let Some(value) = tooling.get("lastAgentMessageAt") else {
        return Ok(None);
    };
    let text = value.as_str().ok_or(PreparedPairError::Conflict)?;
    chrono::DateTime::parse_from_rfc3339(text).map_err(|_| PreparedPairError::Conflict)?;
    Ok(Some(text.to_string()))
}

fn timestamp_cmp(a: &str, b: &str) -> Result<std::cmp::Ordering, PreparedPairError> {
    let a = chrono::DateTime::parse_from_rfc3339(a).map_err(|_| PreparedPairError::Conflict)?;
    let b = chrono::DateTime::parse_from_rfc3339(b).map_err(|_| PreparedPairError::Conflict)?;
    Ok(a.cmp(&b))
}

pub(crate) fn merge_monotonic_activity(
    values: &[Option<String>],
) -> Result<Option<String>, PreparedPairError> {
    let mut maximum: Option<String> = None;
    for value in values.iter().flatten() {
        timestamp_cmp(value, value)?;
        if maximum
            .as_deref()
            .map(|old| timestamp_cmp(value, old))
            .transpose()?
            .is_none_or(|order| order.is_gt())
        {
            maximum = Some(value.clone());
        }
    }
    Ok(maximum)
}

fn timestamp_only_overlay(image: &PhysicalState) -> Result<bool, PreparedPairError> {
    let map = image.map()?;
    Ok(map.len() == 1
        && map
            .get("tooling")
            .and_then(Value::as_object)
            .is_some_and(|tooling| {
                tooling.len() == 1 && tooling.contains_key("lastAgentMessageAt")
            })
        && timestamp_value(image)?.is_some())
}

fn protected_matches(
    expected: &PhysicalState,
    observed: &PhysicalState,
    state: bool,
) -> Result<bool, PreparedPairError> {
    if state && *expected == PhysicalState::Absent && *observed != PhysicalState::Absent {
        return timestamp_only_overlay(observed);
    }
    Ok(protected_revision(expected, state)? == protected_revision(observed, state)?)
}

/// Private caller data; authorization and C/source proofs belong above IO.
/// Mappings are bound by their canonical digest, never copied source commands.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct PairReservationRequest {
    pub(crate) operation_id: String,
    pub(crate) source_physical_key: String,
    pub(crate) owner_instance_id: String,
    pub(crate) mappings_digest: String,
    pub(crate) before_protected: [ProtectedRevision; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "camelCase",
    deny_unknown_fields
)]
enum TimestampPhysical {
    AbsentState,
    AbsentField,
    Value(String),
}

fn timestamp_physical(image: &PhysicalState) -> Result<TimestampPhysical, PreparedPairError> {
    if *image == PhysicalState::Absent {
        return Ok(TimestampPhysical::AbsentState);
    }
    Ok(match timestamp_value(image)? {
        Some(value) => TimestampPhysical::Value(value),
        None => TimestampPhysical::AbsentField,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairActivityIntent {
    before_timestamp_physical: TimestampPhysical,
    after_timestamp: String,
    floor_before: Option<String>,
}

/// The single physical recovery authority: presence means complete images and
/// floor are durable. Owner and floor are deliberately outside planDigest.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PairReservation {
    version: u32,
    pub(crate) operation_id: String,
    pub(crate) source_physical_key: String,
    pub(crate) owner_instance_id: String,
    pub(crate) plan_digest: String,
    pub(crate) physical_target_identity: String,
    protected_policy_version: u32,
    mappings_digest: String,
    before_tuple: [PhysicalState; 2],
    protected_images: [[ProtectedRevision; 2]; 5],
    changed_sides: [bool; 2],
    timestamp_floor: Option<String>,
    activity_intent: Option<PairActivityIntent>,
    plan_images: PreparedConfigPairPlan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum PairReservationRole {
    Owner,
    Observer,
}

fn protected_plan(
    plan: &PreparedConfigPairPlan,
) -> Result<[[ProtectedRevision; 2]; 5], PreparedPairError> {
    let mut images = Vec::new();
    for tuple in plan.stages() {
        images.push([
            protected_revision(&tuple[0], false)?,
            protected_revision(&tuple[1], true)?,
        ]);
    }
    images
        .try_into()
        .map_err(|_| PreparedPairError::InvalidPlan("PAIR stage count"))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn protected_config_pair_revision(
    before: &[PhysicalState; 2],
) -> Result<[ProtectedRevision; 2], PreparedPairError> {
    Ok([
        protected_revision(&before[0], false)?,
        protected_revision(&before[1], true)?,
    ])
}

fn pair_plan_digest(
    plan: &PreparedConfigPairPlan,
    mappings_digest: &str,
) -> Result<String, PreparedPairError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(1u32, protected_plan(plan)?, mappings_digest))
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

impl PairReservation {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn protected_revision_digests(&self) -> Result<[String; 2], PreparedPairError> {
        use sha2::{Digest, Sha256};
        let digest = |tuple: &[ProtectedRevision; 2]| -> Result<String, PreparedPairError> {
            Ok(format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(tuple).map_err(|e| PreparedPairError::Io(e.to_string()))?
                )
            ))
        };
        Ok([
            digest(&self.protected_images[0])?,
            digest(&self.protected_images[4])?,
        ])
    }

    fn validate(&self) -> Result<(), PreparedPairError> {
        self.plan_images.validate()?;
        if self.version != 1
            || self.protected_policy_version != 1
            || self.operation_id.is_empty()
            || self.source_physical_key.is_empty()
            || self.owner_instance_id.is_empty()
            || self.physical_target_identity.is_empty()
            || self.mappings_digest.is_empty()
            || self.plan_digest != pair_plan_digest(&self.plan_images, &self.mappings_digest)?
            || self.protected_images != protected_plan(&self.plan_images)?
            || self.before_tuple != self.plan_images.stages[0]
            || self.changed_sides != changed_pair_sides(&self.plan_images)
        {
            return Err(PreparedPairError::InvalidPlan("invalid reservation packet"));
        }
        merge_monotonic_activity(&[self.timestamp_floor.clone()])?;
        if let Some(intent) = &self.activity_intent {
            if intent.floor_before != self.timestamp_floor
                || merge_monotonic_activity(&[
                    intent.floor_before.clone(),
                    Some(intent.after_timestamp.clone()),
                ])? != Some(intent.after_timestamp.clone())
            {
                return Err(PreparedPairError::Conflict);
            }
        }
        Ok(())
    }

    fn recognize(&self, current: &[PhysicalState; 2]) -> Result<usize, PreparedPairError> {
        let timestamp = timestamp_value(&current[1])?;
        if let Some(floor) = &self.timestamp_floor {
            if timestamp
                .as_deref()
                .map(|value| timestamp_cmp(value, floor))
                .transpose()?
                .is_none_or(|order| order.is_lt())
            {
                return Err(PreparedPairError::Conflict);
            }
        }
        for stage in (0..5).rev() {
            let expected = &self.plan_images.stages[stage];
            if protected_matches(&expected[0], &current[0], false)?
                && protected_matches(&expected[1], &current[1], true)?
            {
                let planned = if stage == 0 {
                    timestamp_value(&expected[1])?
                } else {
                    self.planned_activity_max()?
                };
                if let Some(planned) = planned {
                    if timestamp
                        .as_deref()
                        .map(|value| timestamp_cmp(value, &planned))
                        .transpose()?
                        .is_none_or(|order| order.is_lt())
                    {
                        continue;
                    }
                }
                return Ok(stage);
            }
        }
        Err(PreparedPairError::Conflict)
    }

    fn activity_max(
        &self,
        current: &[PhysicalState; 2],
        input: Option<String>,
    ) -> Result<Option<String>, PreparedPairError> {
        let mut values = vec![
            self.timestamp_floor.clone(),
            timestamp_value(&current[0])?,
            timestamp_value(&current[1])?,
            self.planned_activity_max()?,
            input,
        ];
        if let Some(intent) = &self.activity_intent {
            values.push(Some(intent.after_timestamp.clone()));
        }
        merge_monotonic_activity(&values)
    }

    fn planned_activity_max(&self) -> Result<Option<String>, PreparedPairError> {
        let mut values = vec![timestamp_value(&self.plan_images.stages[0][0])?];
        for tuple in &self.plan_images.stages {
            values.push(timestamp_value(&tuple[1])?);
        }
        merge_monotonic_activity(&values)
    }
}

fn read_pair_tuple(
    decisions: &Path,
    state: &Path,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    Ok([
        read_config_pair_physical(decisions)?,
        read_config_pair_physical(state)?,
    ])
}

fn pair_target_identity(decisions: &Path, state: &Path) -> Result<String, PreparedPairError> {
    let resolve = |path: &Path| -> Result<String, PreparedPairError> {
        let parent = path
            .parent()
            .ok_or(PreparedPairError::InvalidPlan("PAIR parent"))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(PreparedPairError::InvalidPlan("PAIR name"))?;
        let parent =
            std::fs::canonicalize(parent).map_err(|e| PreparedPairError::Io(e.to_string()))?;
        let path = parent.join(name).to_string_lossy().into_owned();
        Ok(if cfg!(windows) {
            path.to_lowercase()
        } else {
            path
        })
    };
    let d = resolve(decisions)?;
    let s = resolve(state)?;
    if d == s {
        return Err(PreparedPairError::InvalidPlan(
            "PAIR targets must be distinct",
        ));
    }
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(d, s)).map_err(|e| PreparedPairError::Io(e.to_string()))?
        )
    ))
}

fn validate_pair_binding(
    decisions: &Path,
    state: &Path,
    packet: &PairReservation,
) -> Result<(), PreparedPairError> {
    packet.validate()?;
    if packet.physical_target_identity != pair_target_identity(decisions, state)? {
        return Err(PreparedPairError::Conflict);
    }
    Ok(())
}

fn validate_pair_paths(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
) -> Result<(), PreparedPairError> {
    pair_target_identity(decisions, state)?;
    let resolve = |path: &Path| -> Result<PathBuf, PreparedPairError> {
        let parent = std::fs::canonicalize(
            path.parent()
                .ok_or(PreparedPairError::InvalidPlan("PAIR parent"))?,
        )
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        Ok(parent.join(
            path.file_name()
                .ok_or(PreparedPairError::InvalidPlan("PAIR name"))?,
        ))
    };
    let paths = [resolve(decisions)?, resolve(state)?, resolve(reservation)?];
    if paths.iter().any(|path| path.parent() != paths[0].parent())
        || paths[..2].iter().any(|path| {
            path == &paths[2]
                || (cfg!(windows)
                    && path.to_string_lossy().to_lowercase()
                        == paths[2].to_string_lossy().to_lowercase())
        })
    {
        return Err(PreparedPairError::InvalidPlan(
            "reservation must be a distinct PAIR sibling",
        ));
    }
    Ok(())
}

fn read_pair_reservation(
    path: &Path,
) -> Result<Option<(PairReservation, PhysicalState)>, PreparedPairError> {
    let physical = read_config_pair_physical(path)?;
    let PhysicalState::Bytes { bytes, .. } = &physical else {
        return Ok(None);
    };
    let packet: PairReservation = serde_json::from_slice(bytes)
        .map_err(|_| PreparedPairError::InvalidPlan("invalid reservation packet"))?;
    packet.validate()?;
    Ok(Some((packet, physical)))
}

fn publish_pair_reservation(
    path: &Path,
    physical: &mut PhysicalState,
    packet: &PairReservation,
) -> Result<(), PreparedPairError> {
    packet.validate()?;
    let mut bytes =
        serde_json::to_vec_pretty(packet).map_err(|e| PreparedPairError::Io(e.to_string()))?;
    bytes.push(b'\n');
    publish_coordination_bytes(path, physical, &bytes)?;
    *physical = PhysicalState::from_bytes(bytes);
    Ok(())
}

/// Source/C proofs and inventory are acquired by the caller before entry.
/// Snapshot, planning and the one complete packet publication never unlock.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn prepare_and_reserve_config_pair<F>(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    request: &PairReservationRequest,
    cleanup: Option<ConfigPairCleanup<'_>>,
    mutate: F,
) -> Result<(PairReservation, PairReservationRole), PreparedPairError>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    let _writer = ConfigWriterActive::enter("prepare_and_reserve_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let existing = read_pair_reservation(reservation)?;
    let current = read_pair_tuple(decisions, state)?;
    let before = existing
        .as_ref()
        .map(|(packet, _)| packet.plan_images.stages[0].clone())
        .unwrap_or(current.clone());
    if request.before_protected
        != [
            protected_revision(&before[0], false)?,
            protected_revision(&before[1], true)?,
        ]
    {
        return Err(PreparedPairError::Conflict);
    }
    let plan = prepare_config_pair_plan_inner(before, cleanup, mutate)?;
    let images = protected_plan(&plan)?;
    let candidate = PairReservation {
        version: 1,
        operation_id: request.operation_id.clone(),
        source_physical_key: request.source_physical_key.clone(),
        owner_instance_id: request.owner_instance_id.clone(),
        plan_digest: pair_plan_digest(&plan, &request.mappings_digest)?,
        physical_target_identity: pair_target_identity(decisions, state)?,
        protected_policy_version: 1,
        mappings_digest: request.mappings_digest.clone(),
        before_tuple: plan.stages[0].clone(),
        protected_images: images,
        changed_sides: changed_pair_sides(&plan),
        timestamp_floor: timestamp_value(&current[1])?,
        activity_intent: None,
        plan_images: plan,
    };
    candidate.validate()?;
    candidate.activity_max(&current, None)?;
    if let Some((mut packet, mut physical)) = existing {
        validate_pair_binding(decisions, state, &packet)?;
        if packet.operation_id != candidate.operation_id
            || packet.source_physical_key != candidate.source_physical_key
            || packet.plan_digest != candidate.plan_digest
            || packet.before_tuple != candidate.before_tuple
        {
            return Err(PreparedPairError::Conflict);
        }
        let role = if packet.owner_instance_id == request.owner_instance_id {
            PairReservationRole::Owner
        } else {
            PairReservationRole::Observer
        };
        if role == PairReservationRole::Owner {
            reconcile_pair_activity(decisions, state, reservation, &mut packet, &mut physical)?;
        }
        packet.recognize(&read_pair_tuple(decisions, state)?)?;
        return Ok((packet, role));
    }
    candidate.recognize(&current)?;
    let mut physical = PhysicalState::Absent;
    publish_pair_reservation(reservation, &mut physical, &candidate)?;
    Ok((candidate, PairReservationRole::Owner))
}

fn image_with_timestamp(
    image: &PhysicalState,
    timestamp: Option<String>,
) -> Result<PhysicalState, PreparedPairError> {
    let before = image.map()?;
    let mut after = before.clone();
    if let Some(timestamp) = timestamp {
        let tooling = after
            .entry("tooling")
            .or_insert_with(|| serde_json::json!({}));
        tooling
            .as_object_mut()
            .ok_or(PreparedPairError::Conflict)?
            .insert("lastAgentMessageAt".into(), Value::String(timestamp));
    }
    prepared_pair_image(image, &before, &after)
}

fn reconcile_pair_activity(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    packet: &mut PairReservation,
    physical: &mut PhysicalState,
) -> Result<(), PreparedPairError> {
    let Some(intent) = packet.activity_intent.clone() else {
        return Ok(());
    };
    let current = read_pair_tuple(decisions, state)?;
    packet.recognize(&current)?;
    let actual = timestamp_physical(&current[1])?;
    if actual == intent.before_timestamp_physical {
        let after = image_with_timestamp(&current[1], Some(intent.after_timestamp.clone()))?;
        publish_prepared_pair_image(state, &current[1], &after)?;
    } else if actual != TimestampPhysical::Value(intent.after_timestamp.clone()) {
        return Err(PreparedPairError::Conflict);
    }
    packet.timestamp_floor = Some(intent.after_timestamp);
    packet.activity_intent = None;
    packet.recognize(&read_pair_tuple(decisions, state)?)?;
    publish_pair_reservation(reservation, physical, packet)
}

fn land_pair_activity(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    packet: &mut PairReservation,
    physical: &mut PhysicalState,
    input: &str,
    on_stage: &dyn Fn(&str) -> Result<(), PreparedPairError>,
) -> Result<bool, PreparedPairError> {
    reconcile_pair_activity(decisions, state, reservation, packet, physical)?;
    let current = read_pair_tuple(decisions, state)?;
    packet.recognize(&current)?;
    let maximum = packet
        .activity_max(&current, Some(input.to_string()))?
        .ok_or(PreparedPairError::Conflict)?;
    if timestamp_value(&current[1])? == Some(maximum.clone()) {
        return Ok(false);
    }
    let after = image_with_timestamp(&current[1], Some(maximum.clone()))?;
    if !protected_matches(&current[1], &after, true)? {
        return Err(PreparedPairError::Conflict);
    }
    packet.activity_intent = Some(PairActivityIntent {
        before_timestamp_physical: timestamp_physical(&current[1])?,
        after_timestamp: maximum.clone(),
        floor_before: packet.timestamp_floor.clone(),
    });
    publish_pair_reservation(reservation, physical, packet)?;
    on_stage("activity_intent_durable")?;
    publish_prepared_pair_image(state, &current[1], &after)?;
    on_stage("activity_state_durable")?;
    reconcile_pair_activity(decisions, state, reservation, packet, physical)?;
    on_stage("activity_floor_durable")?;
    Ok(true)
}

/// Only the durable owner executes. Observer verification does not consolidate
/// the owner's activity intent, publish refs or take over a missing owner.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn execute_reserved_config_pair(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    instance_id: &str,
) -> Result<(), PreparedPairError> {
    execute_reserved_config_pair_with_stage(
        decisions,
        state,
        reservation,
        operation_id,
        instance_id,
        &|_| Ok(()),
    )
}

/// Observer acknowledgment is based on its own authorized target only. It
/// cannot finalize another owner's packet or recover an activity intent.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn verify_reserved_config_pair(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    plan_digest: &str,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    let _writer = ConfigWriterActive::enter("verify_reserved_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let (packet, _) = read_pair_reservation(reservation)?.ok_or(PreparedPairError::Conflict)?;
    validate_pair_binding(decisions, state, &packet)?;
    if packet.operation_id != operation_id || packet.plan_digest != plan_digest {
        return Err(PreparedPairError::Conflict);
    }
    let current = read_pair_tuple(decisions, state)?;
    if packet.recognize(&current)? != 4 || packet.activity_intent.is_some() {
        return Err(PreparedPairError::Pending);
    }
    Ok(current)
}

fn execute_reserved_config_pair_with_stage(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    instance_id: &str,
    on_stage: &dyn Fn(usize) -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    let _writer = ConfigWriterActive::enter("execute_reserved_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let (mut packet, mut physical) =
        read_pair_reservation(reservation)?.ok_or(PreparedPairError::Conflict)?;
    validate_pair_binding(decisions, state, &packet)?;
    if packet.operation_id != operation_id || packet.owner_instance_id != instance_id {
        return Err(PreparedPairError::Conflict);
    }
    reconcile_pair_activity(decisions, state, reservation, &mut packet, &mut physical)?;
    let start = packet.recognize(&read_pair_tuple(decisions, state)?)?;
    confirm_pair_activity_floor(state, reservation, &mut packet, &mut physical)?;
    for stage in start + 1..5 {
        let current = read_pair_tuple(decisions, state)?;
        let observed = packet.recognize(&current)?;
        if observed >= stage {
            continue;
        }
        if observed != stage - 1 {
            return Err(PreparedPairError::Conflict);
        }
        let side = if stage == 1 || stage == 3 { 1 } else { 0 };
        let planned = &packet.plan_images.stages[stage][side];
        let after = if side == 1 {
            image_with_timestamp(planned, packet.activity_max(&current, None)?)?
        } else {
            planned.clone()
        };
        if after != current[side] {
            let mut proposed = current.clone();
            proposed[side] = after.clone();
            if packet.recognize(&proposed)? < stage {
                return Err(PreparedPairError::Conflict);
            }
            let path = if side == 0 { decisions } else { state };
            // Absence plus an activity overlay is already the planned absent
            // side; it must never be removed to reproduce the original bytes.
            if after == PhysicalState::Absent {
                return Err(PreparedPairError::Conflict);
            }
            publish_prepared_pair_image(path, &current[side], &after)?;
            if packet.recognize(&read_pair_tuple(decisions, state)?)? < stage {
                return Err(PreparedPairError::Conflict);
            }
            confirm_pair_activity_floor(state, reservation, &mut packet, &mut physical)?;
            on_stage(stage)?;
        }
    }
    confirm_pair_activity_floor(state, reservation, &mut packet, &mut physical)?;
    if packet.recognize(&read_pair_tuple(decisions, state)?)? != 4 {
        return Err(PreparedPairError::Conflict);
    }
    Ok(())
}

fn confirm_pair_activity_floor(
    state: &Path,
    reservation: &Path,
    packet: &mut PairReservation,
    physical: &mut PhysicalState,
) -> Result<(), PreparedPairError> {
    let floor = merge_monotonic_activity(&[
        packet.timestamp_floor.clone(),
        timestamp_value(&read_config_pair_physical(state)?)?,
    ])?;
    if floor != packet.timestamp_floor {
        packet.timestamp_floor = floor;
        publish_pair_reservation(reservation, physical, packet)?;
    }
    Ok(())
}

/// IO consumes a proof assembled above PAIR from an authenticated, validated
/// shared ledger under the source lease. It never reads another instance's C.
#[derive(Clone, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct PairCompleteProof {
    operation_id: String,
    source_physical_key: String,
    target_id: String,
    plan_digest: String,
    owner_instance_id: String,
}

#[cfg_attr(not(test), allow(dead_code))]
impl PairCompleteProof {
    pub(crate) fn from_shared_complete(
        ledger: &Value,
        operation_id: &str,
        target_id: &str,
        current_source_revision: &Value,
    ) -> Result<Self, PreparedPairError> {
        let op = ledger
            .get("operations")
            .and_then(|ops| ops.get(operation_id))
            .ok_or(PreparedPairError::Conflict)?;
        let receipt = op
            .get("receipt")
            .filter(|value| value.is_object())
            .ok_or(PreparedPairError::Conflict)?;
        let participants = op
            .get("participantSet")
            .and_then(Value::as_object)
            .ok_or(PreparedPairError::Conflict)?;
        let commitments = op
            .get("commitments")
            .and_then(Value::as_object)
            .ok_or(PreparedPairError::Conflict)?;
        let acks = op
            .get("acks")
            .and_then(Value::as_object)
            .ok_or(PreparedPairError::Conflict)?;
        let receipt_ids = receipt
            .get("participantIds")
            .and_then(Value::as_array)
            .ok_or(PreparedPairError::Conflict)?;
        let after = op
            .get("afterSourceRevision")
            .filter(|revision| {
                revision.get("kind").and_then(Value::as_str) == Some("bytes")
                    && revision
                        .get("sha256")
                        .and_then(Value::as_str)
                        .is_some_and(|hash| {
                            hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
            })
            .ok_or(PreparedPairError::Conflict)?;
        if op.get("operationId").and_then(Value::as_str) != Some(operation_id)
            || op.get("phase").and_then(Value::as_str) != Some("complete")
            || receipt.get("operationId").and_then(Value::as_str) != Some(operation_id)
            || receipt.get("sourceRevision") != Some(after)
            || current_source_revision != after
            || participants.len() != commitments.len()
            || participants.len() != acks.len()
            || participants.len() != receipt_ids.len()
        {
            return Err(PreparedPairError::Conflict);
        }
        let mut target: Option<&Value> = None;
        for id in participants.keys() {
            if acks.get(id) != Some(after)
                || receipt_ids
                    .iter()
                    .filter(|value| value.as_str() == Some(id.as_str()))
                    .count()
                    != 1
            {
                return Err(PreparedPairError::Conflict);
            }
            let commitment = commitments.get(id).ok_or(PreparedPairError::Conflict)?;
            if commitment.get("participantId").and_then(Value::as_str) != Some(id.as_str()) {
                return Err(PreparedPairError::Conflict);
            }
            for row in commitment
                .get("targets")
                .and_then(Value::as_array)
                .ok_or(PreparedPairError::Conflict)?
            {
                if row.get("targetId").and_then(Value::as_str) == Some(target_id) {
                    if let Some(previous) = target {
                        if previous != row {
                            return Err(PreparedPairError::Conflict);
                        }
                    }
                    target = Some(row);
                }
            }
        }
        let target = target.ok_or(PreparedPairError::Conflict)?;
        let text = |value: &Value, field: &str| -> Result<String, PreparedPairError> {
            value
                .get(field)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .ok_or(PreparedPairError::Conflict)
        };
        let owner_instance_id = text(target, "ownerInstanceId")?;
        if !participants.contains_key(&owner_instance_id) {
            return Err(PreparedPairError::Conflict);
        }
        Ok(Self {
            operation_id: operation_id.to_string(),
            source_physical_key: text(ledger, "physicalSourceKey")?,
            target_id: target_id.to_string(),
            plan_digest: text(target, "planDigest")?,
            owner_instance_id,
        })
    }
}

/// Complete receipt is necessary but insufficient: the owner also verifies its
/// own final protected tuple and confirmed floor before removing this packet.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn release_reserved_config_pair(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    instance_id: &str,
    proof: &PairCompleteProof,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    let _writer = ConfigWriterActive::enter("release_reserved_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    if proof.owner_instance_id != instance_id
        || proof.target_id != pair_target_identity(decisions, state)?
    {
        return Err(PreparedPairError::Conflict);
    }
    let Some((mut packet, mut physical)) = read_pair_reservation(reservation)? else {
        return read_pair_tuple(decisions, state);
    };
    validate_pair_binding(decisions, state, &packet)?;
    if packet.operation_id != proof.operation_id
        || packet.source_physical_key != proof.source_physical_key
        || packet.owner_instance_id != proof.owner_instance_id
        || packet.plan_digest != proof.plan_digest
    {
        return Err(PreparedPairError::Conflict);
    }
    reconcile_pair_activity(decisions, state, reservation, &mut packet, &mut physical)?;
    let current = read_pair_tuple(decisions, state)?;
    if packet.recognize(&current)? != 4 {
        return Err(PreparedPairError::Conflict);
    }
    std::fs::remove_file(reservation).map_err(|e| PreparedPairError::Io(e.to_string()))?;
    #[cfg(not(windows))]
    std::fs::File::open(
        reservation
            .parent()
            .ok_or(PreparedPairError::InvalidPlan("reservation parent"))?,
    )
    .and_then(|directory| directory.sync_all())
    .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    Ok(current)
}

/// P14 single-file private/shared metadata publish. Caller holds the stable
/// coordination sidecar for this entire CAS/fsync segment; no nested writer.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn publish_coordination_bytes(
    path: &Path,
    before: &PhysicalState,
    bytes: &[u8],
) -> Result<(), PreparedPairError> {
    let after = PhysicalState::from_bytes(bytes.to_vec());
    after.map()?;
    if read_config_pair_physical(path)? != *before {
        return Err(PreparedPairError::Conflict);
    }
    if before == &after {
        return Ok(());
    }
    publish_prepared_pair_image(path, before, &after)
}

/// #2786 (C1) - one side of the pair as a map: absent is empty, anything that
/// is not a readable JSON object is an error.
fn read_pair_side(path: &Path) -> Result<Map<String, Value>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(format!("Failed to read {}: {}", path.display(), e)),
    };
    match serde_json::from_str::<Value>(&content) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!(
            "Local config {} must be a JSON object",
            path.display()
        )),
        Err(e) => Err(format!("Failed to parse {}: {}", path.display(), e)),
    }
}

/// #2786 (C1) - publish the changed sides of one sequence, state first, firing
/// each side's stage only after its write.
fn publish_pair_sides(
    decisions: (&Path, &Map<String, Value>, &Map<String, Value>),
    state: (&Path, &Map<String, Value>, &Map<String, Value>),
    on_stage: &dyn Fn(&str),
    [state_stage, decisions_stage]: [&str; 2],
) -> Result<(), String> {
    for ((path, before, after), stage) in [(state, state_stage), (decisions, decisions_stage)] {
        if before != after {
            publish_pair_side(path, after)?;
            on_stage(stage);
        }
    }
    Ok(())
}

/// #2786 (C1) - temp plus `publish_temp_config`, never `write_file_atomic`:
/// the pair already holds the process `Mutex`, which is not reentrant.
fn publish_pair_side(path: &Path, map: &Map<String, Value>) -> Result<(), String> {
    let mut json = serde_json::to_string_pretty(map)
        .map_err(|e| format!("Failed to serialize {}: {}", path.display(), e))?;
    json.push('\n');

    let tmp_path = temp_config_path(path);
    let write_result = (|| -> Result<(), String> {
        let mut file = std::fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temp config {}: {}", tmp_path.display(), e))?;
        file.write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write temp config {}: {}", tmp_path.display(), e))?;
        file.flush()
            .map_err(|e| format!("Failed to flush temp config {}: {}", tmp_path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync temp config {}: {}", tmp_path.display(), e))
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    if let Err(e) = publish_temp_config(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(())
}

fn temp_config_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");
    path.with_file_name(format!(".{}.{}.tmp", file_name, std::process::id()))
}

/// #537 - OS error codes that mean the destination config.json was only
/// briefly held by another handle (a concurrent in-process reader, a second
/// AgentsCommander instance, or antivirus / the Windows Search Indexer) at the
/// instant we tried to publish, rather than a permanent failure. A short
/// backoff and retry clears them; permanent failures (a bad path, a lasting
/// permission denial) are NOT in this set, so they surface immediately and we
/// never spin on an error that cannot clear.
///
/// These are Windows codes:
///   1175 ERROR_UNABLE_TO_REMOVE_REPLACED (ReplaceFileW could not delete dst)
///     32 ERROR_SHARING_VIOLATION
///     33 ERROR_LOCK_VIOLATION
///      5 ERROR_ACCESS_DENIED (commonly a transient AV lock on Windows)
const TRANSIENT_PUBLISH_OS_ERRORS: [i32; 4] = [1175, 32, 33, 5];

/// #537 - publish attempt budget and backoff schedule, duplicated from the
/// reviewed pattern in sessions_persistence::rename_with_retry (#280) per the
/// house "a little duplication over a premature abstraction" rule. Four
/// attempts so all three backoff entries are used; the terminal attempt has no
/// backoff. Worst-case added latency on a contended publish is the sum, 260 ms.
/// Like #280 this uses std::thread::sleep, which briefly blocks the calling
/// tokio worker; update_config_json_object is already fully synchronous, so
/// this only enlarges an existing blocking window rather than adding a new one.
const PUBLISH_ATTEMPTS: u32 = 4;
const PUBLISH_BACKOFFS_MS: [u64; 3] = [10, 50, 200];

/// #537 - true when a publish error looks like a transient lock collision we
/// should retry rather than a permanent failure. See TRANSIENT_PUBLISH_OS_ERRORS.
fn is_transient_publish_error(err: &std::io::Error) -> bool {
    if err
        .raw_os_error()
        .is_some_and(|code| TRANSIENT_PUBLISH_OS_ERRORS.contains(&code))
    {
        return true;
    }
    // Non-Windows analogue: POSIX rename(2) within a directory is atomic and
    // does not hit the Windows "destination briefly locked" class, but a
    // collision on a networked or watched mount can still surface as
    // PermissionDenied. Mirrors treating Windows code 5 as transient.
    cfg!(not(windows)) && err.kind() == std::io::ErrorKind::PermissionDenied
}

/// #537 facet (b) - turn a publish failure into a message that reads clearly in
/// both the "Assign to this replica" error row and the session-launch dialog. A
/// transient lock collision gets a plain-language, retry-suggesting sentence
/// instead of the cryptic raw "os error 1175"; the OS error code is still
/// appended for diagnostics. Permanent failures keep the precise low-level
/// detail an operator needs to fix them.
fn format_publish_error(path: &Path, tmp_path: &Path, err: &std::io::Error) -> String {
    if is_transient_publish_error(err) {
        format!(
            "Could not update {} because it was briefly locked by another program \
             (a second AgentsCommander instance, antivirus, or Windows Search). \
             Please try again. (os error {})",
            path.display(),
            err.raw_os_error().unwrap_or_default()
        )
    } else {
        format!(
            "Failed to replace {} with {}: {}",
            path.display(),
            tmp_path.display(),
            err
        )
    }
}

#[cfg(not(windows))]
fn publish_temp_config(tmp_path: &Path, path: &Path) -> Result<(), String> {
    let start = std::time::Instant::now();
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..PUBLISH_ATTEMPTS {
        match std::fs::rename(tmp_path, path) {
            Ok(()) => {
                if attempt > 0 {
                    log::info!(
                        "[config] publish succeeded after retry: path={} attempt={}/{} duration={:?}",
                        path.display(),
                        attempt + 1,
                        PUBLISH_ATTEMPTS,
                        start.elapsed()
                    );
                }
                return Ok(());
            }
            Err(e) => {
                if !is_transient_publish_error(&e) {
                    return Err(format_publish_error(path, tmp_path, &e));
                }
                log::debug!(
                    "[config] publish attempt {}/{} failed: path={} os_error={:?} kind={:?}",
                    attempt + 1,
                    PUBLISH_ATTEMPTS,
                    path.display(),
                    e.raw_os_error(),
                    e.kind()
                );
                last_err = Some(e);
                if let Some(backoff) = PUBLISH_BACKOFFS_MS.get(attempt as usize) {
                    std::thread::sleep(std::time::Duration::from_millis(*backoff));
                }
            }
        }
    }

    let e = last_err.expect("PUBLISH_ATTEMPTS >= 1, so the loop runs at least once");
    log::error!(
        "[config] publish exhausted {} attempts: path={} os_error={:?} duration={:?}",
        PUBLISH_ATTEMPTS,
        path.display(),
        e.raw_os_error(),
        start.elapsed()
    );
    Err(format_publish_error(path, tmp_path, &e))
}

/// #2378 - NUL-terminated UTF-16 for a `ReplaceFileW` argument. Raw Win32
/// paths are MAX_PATH-bound (the process has no `longPathAware` manifest), so
/// an absolute `Disk` or `UNC` path is respelled in verbatim (`\\?\`) form.
/// Every shape where the verbatim spelling could name a different object
/// (relative, drive-relative, `..`, trailing dot or space, already verbatim,
/// device namespace, interior NUL) keeps today's raw encoding. The verbatim
/// string is built from parsed prefix values, never from raw prefix text, so a
/// caller's `/` separators cannot leak into it.
#[cfg(windows)]
fn publish_path_wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Prefix};

    let raw = || -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };

    if path.as_os_str().encode_wide().any(|u| u == 0) || !path.is_absolute() {
        return raw();
    }
    for component in path.components() {
        match component {
            Component::ParentDir => return raw(),
            Component::Normal(s) => {
                if matches!(s.encode_wide().last(), Some(u) if u == u16::from(b'.') || u == u16::from(b' '))
                {
                    return raw();
                }
            }
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(_) | Prefix::UNC(_, _) => {}
                Prefix::Verbatim(_)
                | Prefix::VerbatimUNC(_, _)
                | Prefix::VerbatimDisk(_)
                | Prefix::DeviceNS(_) => return raw(),
            },
            Component::RootDir | Component::CurDir => {}
        }
    }

    let sep = u16::from(b'\\');
    let mut out: Vec<u16> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(d) => {
                    out.extend(r"\\?\".encode_utf16());
                    out.push(u16::from(d));
                    out.push(u16::from(b':'));
                }
                Prefix::UNC(server, share) => {
                    out.extend(r"\\?\UNC\".encode_utf16());
                    out.extend(server.encode_wide());
                    out.push(sep);
                    out.extend(share.encode_wide());
                }
                _ => return raw(),
            },
            Component::RootDir => out.push(sep),
            Component::CurDir => {}
            Component::Normal(s) => {
                if out.last() != Some(&sep) {
                    out.push(sep);
                }
                out.extend(s.encode_wide());
            }
            Component::ParentDir => return raw(),
        }
    }
    out.push(0);
    out
}

#[cfg(windows)]
fn publish_temp_config(tmp_path: &Path, path: &Path) -> Result<(), String> {
    if !path.exists() {
        return std::fs::rename(tmp_path, path).map_err(|e| {
            format!(
                "Failed to publish {} as {}: {}",
                tmp_path.display(),
                path.display(),
                e
            )
        });
    }

    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    let path_wide = publish_path_wide(path);
    let tmp_wide = publish_path_wide(tmp_path);

    // #537 - ReplaceFileW publishes the temp file over the existing
    // config.json. It returns ERROR_UNABLE_TO_REMOVE_REPLACED (1175) and
    // friends whenever another handle holds the destination for even an
    // instant, so a single collision used to fail the whole write. Retry the
    // publish with a bounded backoff, mirroring rename_with_retry (#280).
    let start = std::time::Instant::now();
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..PUBLISH_ATTEMPTS {
        let ok = unsafe {
            ReplaceFileW(
                path_wide.as_ptr(),
                tmp_wide.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ok != 0 {
            if attempt > 0 {
                log::info!(
                    "[config] ReplaceFileW succeeded after retry: path={} attempt={}/{} duration={:?}",
                    path.display(),
                    attempt + 1,
                    PUBLISH_ATTEMPTS,
                    start.elapsed()
                );
            }
            return Ok(());
        }

        let e = std::io::Error::last_os_error();
        if !is_transient_publish_error(&e) {
            return Err(format_publish_error(path, tmp_path, &e));
        }
        log::debug!(
            "[config] ReplaceFileW attempt {}/{} failed: path={} os_error={:?}",
            attempt + 1,
            PUBLISH_ATTEMPTS,
            path.display(),
            e.raw_os_error()
        );
        last_err = Some(e);
        if let Some(backoff) = PUBLISH_BACKOFFS_MS.get(attempt as usize) {
            std::thread::sleep(std::time::Duration::from_millis(*backoff));
        }
    }

    let e = last_err.expect("PUBLISH_ATTEMPTS >= 1, so the loop runs at least once");
    log::error!(
        "[config] ReplaceFileW exhausted {} attempts: path={} os_error={:?} duration={:?}",
        PUBLISH_ATTEMPTS,
        path.display(),
        e.raw_os_error(),
        start.elapsed()
    );
    Err(format_publish_error(path, tmp_path, &e))
}

#[cfg(test)]
mod tests {
    use super::{
        acquire_config_file_write_lock, config_lock_path, execute_prepared_config_pair,
        execute_prepared_config_pair_with_stage, format_publish_error, is_transient_publish_error,
        prepare_config_pair_plan, publish_prepared_pair_image, read_config_pair_physical,
        temp_config_path, update_config_json_object, update_config_json_object_with_publish,
        update_config_pair, write_file_atomic, write_file_atomic_with_publish, PhysicalState,
        PreparedConfigPairPlan, PreparedPairError, CONFIG_LOCK_TIMEOUT,
    };
    use serde_json::{json, Map, Value};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn transient_publish_errors_are_classified_for_retry() {
        for code in super::TRANSIENT_PUBLISH_OS_ERRORS {
            let err = std::io::Error::from_raw_os_error(code);
            assert!(
                is_transient_publish_error(&err),
                "os error {code} should be treated as transient"
            );
        }
        // A permanent code (ERROR_FILE_NOT_FOUND) must not be retried.
        let permanent = std::io::Error::from_raw_os_error(2);
        assert!(!is_transient_publish_error(&permanent));
    }

    #[test]
    fn transient_publish_error_message_is_human_readable() {
        let err = std::io::Error::from_raw_os_error(1175);
        let msg = format_publish_error(
            Path::new("C:/x/config.json"),
            Path::new("C:/x/.config.json.1.tmp"),
            &err,
        );
        assert!(msg.contains("briefly locked"), "{msg}");
        assert!(msg.contains("try again"), "{msg}");
        assert!(msg.contains("1175"), "{msg}");
        assert!(!msg.contains("Failed to replace"), "{msg}");
    }

    #[test]
    fn permanent_publish_error_message_keeps_low_level_detail() {
        let err = std::io::Error::from_raw_os_error(2);
        let msg = format_publish_error(Path::new("C:/x/config.json"), Path::new("C:/x/.tmp"), &err);
        assert!(msg.contains("Failed to replace"), "{msg}");
    }

    #[test]
    fn update_config_json_object_rejects_invalid_existing_json() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(&path, "{ invalid").unwrap();

        let err = update_config_json_object(&path, false, |_| Ok(())).unwrap_err();

        assert!(err.contains("Failed to parse"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ invalid");
    }

    #[test]
    fn write_file_atomic_keeps_original_when_publish_fails() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(&path, b"before").unwrap();

        let err = write_file_atomic_with_publish(&path, b"after", |_tmp, _path| {
            Err("forced publish failure".to_string())
        })
        .unwrap_err();

        assert!(err.contains("forced publish failure"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), b"before");
        let tmp_entries = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect::<Vec<_>>();
        assert!(tmp_entries.is_empty(), "temp files left behind");
    }

    #[test]
    fn update_config_json_object_preserves_unknown_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(&path, r#"{"identity":"../../_agent_a","tooling":{"x":1}}"#).unwrap();

        let value = update_config_json_object(&path, false, |obj| {
            obj.insert("context".to_string(), serde_json::json!(["Role.md"]));
            Ok(())
        })
        .unwrap();

        assert_eq!(value["identity"], "../../_agent_a");
        assert_eq!(value["tooling"]["x"], 1);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(saved["context"][0], "Role.md");
    }

    #[test]
    fn agent_replica_root_config_writes_go_through_shared_helper() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        // #1938 - the roots are every crate area that can hold a replica or
        // local config writer. The scan reports matches; it never whitelists
        // them, so an unexpected writer fails here and is fixed (or escalated
        // with evidence), never silenced.
        let roots = [
            manifest.join("src/config"),
            manifest.join("src/commands"),
            manifest.join("src/cli"),
            manifest.join("src/phone"),
            manifest.join("src/web"),
        ];
        let mut offenders = Vec::new();
        for root in roots {
            scan_dir(&root, &mut offenders);
        }

        assert!(
            offenders.is_empty(),
            "local config writers must use update_config_json_object:\n{}",
            offenders.join("\n")
        );
    }

    #[test]
    fn direct_write_guard_detects_async_and_shorthand_writes() {
        assert!(line_mentions_direct_write(
            "tokio::fs::write(&path, body).await?;"
        ));
        assert!(line_mentions_direct_write("fs::write(&path, body)?;"));
        assert!(!line_mentions_direct_write("safe_fs::write(&path, body)?;"));
    }

    #[test]
    fn direct_write_guard_detects_multiline_config_target_binding() {
        let source = r#"
pub fn bad(agent_dir: &Path) -> Result<(), String> {
    let target = agent_dir.join("config.json");
    fs::write(&target, "{}")?;
    Ok(())
}
"#;
        let mut offenders = Vec::new();
        scan_source(
            Path::new("src/config/agent_config.rs"),
            source,
            &mut offenders,
        );

        assert_eq!(offenders.len(), 1);
        assert!(offenders[0].contains("fs::write"), "{offenders:?}");
    }

    fn scan_dir(root: &Path, offenders: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                scan_dir(&path, offenders);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            scan_file(&path, offenders);
        }
    }

    fn scan_file(path: &Path, offenders: &mut Vec<String>) {
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        let stripped = strip_test_modules(&content);
        scan_source(path, &stripped, offenders);
    }

    fn scan_source(path: &Path, content: &str, offenders: &mut Vec<String>) {
        let mut config_target_vars = HashSet::new();
        let mut depth = 0;
        for (idx, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if depth == 0 {
                config_target_vars.clear();
            }
            if let Some(var) = local_config_target_binding(trimmed) {
                config_target_vars.insert(var);
            }
            let mentions_local_target = line_mentions_local_config_target(trimmed)
                || line_mentions_tracked_config_target(trimmed, &config_target_vars);
            if !line_mentions_direct_write(trimmed) || !mentions_local_target {
                depth += brace_delta(line);
                if depth <= 0 {
                    depth = 0;
                    config_target_vars.clear();
                }
                continue;
            }
            if is_allowed_line(path, trimmed) {
                depth += brace_delta(line);
                if depth <= 0 {
                    depth = 0;
                    config_target_vars.clear();
                }
                continue;
            }
            offenders.push(format!("{}:{}: {}", path.display(), idx + 1, trimmed));
            depth += brace_delta(line);
            if depth <= 0 {
                depth = 0;
                config_target_vars.clear();
            }
        }
    }

    fn line_mentions_direct_write(line: &str) -> bool {
        line_contains_call_path(line, "std::fs::write")
            || line_contains_call_path(line, "tokio::fs::write")
            || line_contains_call_path(line, "fs::write")
            || line_contains_call_path(line, "File::create")
            || line_contains_call_path(line, "OpenOptions::new")
            || line.contains(".write_all")
    }

    fn line_mentions_local_config_target(line: &str) -> bool {
        line.contains("config.json")
            || line.contains("join(\"config.json\")")
            || line.contains("config_path")
    }

    fn local_config_target_binding(line: &str) -> Option<String> {
        if !line_mentions_local_config_target(line) {
            return None;
        }
        let rest = line.strip_prefix("let ")?;
        let rest = rest.strip_prefix("mut ").unwrap_or(rest);
        let name = rest.split('=').next()?.trim();
        let name = name.split(':').next().unwrap_or(name).trim();
        if !is_rust_identifier(name) {
            return None;
        }
        Some(name.to_string())
    }

    fn line_mentions_tracked_config_target(line: &str, vars: &HashSet<String>) -> bool {
        vars.iter().any(|var| line_mentions_identifier(line, var))
    }

    fn line_contains_call_path(line: &str, needle: &str) -> bool {
        let mut rest = line;
        while let Some(idx) = rest.find(needle) {
            let before = rest[..idx].chars().next_back();
            let after = &rest[idx + needle.len()..];
            let before_ok = before.is_none_or(|ch| !is_rust_identifier_char(ch));
            let after_ok = after.chars().find(|ch| !ch.is_ascii_whitespace()) == Some('(');
            if before_ok && after_ok {
                return true;
            }
            rest = &rest[idx + needle.len()..];
        }
        false
    }

    fn line_mentions_identifier(line: &str, ident: &str) -> bool {
        let mut rest = line;
        while let Some(idx) = rest.find(ident) {
            let before = rest[..idx].chars().next_back();
            let after = rest[idx + ident.len()..].chars().next();
            let before_ok = before.is_none_or(|ch| !is_rust_identifier_char(ch));
            let after_ok = after.is_none_or(|ch| !is_rust_identifier_char(ch));
            if before_ok && after_ok {
                return true;
            }
            rest = &rest[idx + ident.len()..];
        }
        false
    }

    fn is_rust_identifier(value: &str) -> bool {
        let mut chars = value.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        (first == '_' || first.is_ascii_alphabetic()) && chars.all(is_rust_identifier_char)
    }

    fn is_rust_identifier_char(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphanumeric()
    }

    fn is_allowed_line(path: &Path, line: &str) -> bool {
        let normalized = path.to_string_lossy().replace('\\', "/");
        normalized.ends_with("src/config/local_config_io.rs")
            || (normalized.ends_with("src/commands/entity_creation.rs")
                && (line.contains("write_team_config")
                    || line.contains("create_new_team_config_on_disk")
                    || line.contains("team_dir.join(\"config.json\")")))
    }

    fn strip_test_modules(content: &str) -> String {
        let mut out = String::with_capacity(content.len());
        let mut lines = content.lines();
        while let Some(line) = lines.next() {
            if line.trim_start().starts_with("#[cfg(test)]") {
                out.push('\n');
                if let Some(next) = lines.next() {
                    if next.trim_start().starts_with("mod tests") {
                        out.push_str(&blank_braced_block(next, &mut lines));
                        continue;
                    }
                    out.push_str(next);
                    out.push('\n');
                }
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    fn blank_braced_block<'a>(
        first_line: &str,
        lines: &mut impl Iterator<Item = &'a str>,
    ) -> String {
        let mut blanked = String::new();
        let mut depth = brace_delta(first_line);
        blanked.push('\n');
        while depth > 0 {
            let Some(line) = lines.next() else {
                break;
            };
            depth += brace_delta(line);
            blanked.push('\n');
        }
        blanked
    }

    fn brace_delta(line: &str) -> i32 {
        let opens = line.chars().filter(|ch| *ch == '{').count() as i32;
        let closes = line.chars().filter(|ch| *ch == '}').count() as i32;
        opens - closes
    }

    fn non_utf8_file_name() -> std::ffi::OsString {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            // An unpaired surrogate: a legal OsString, never a legal `&str`.
            std::ffi::OsString::from_wide(&[0xD800])
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![0xff, 0xfe])
        }
        #[cfg(not(any(windows, unix)))]
        {
            std::ffi::OsString::from("config.json")
        }
    }

    /// #1209, instance-dir half: every temporary name this module can publish is
    /// inside the pattern the instance `.gitignore` emits for atomic-write
    /// temporaries.
    ///
    /// The assertion calls the registry's own predicate instead of restating the
    /// glob. A restatement does not follow the pattern when the pattern changes,
    /// which would leave the tie this test is named for untied; the predicate
    /// lives next to the pattern and a registry test pins their agreement.
    ///
    /// `temp_config_path` stays private and `write_file_atomic` is untouched:
    /// this reads what the writer would produce, it does not widen anything.
    #[test]
    fn atomic_temp_names_stay_inside_the_ignored_glob() {
        use crate::config::instance_artifacts::matches_atomic_write_tmp_glob;

        let mut targets: Vec<PathBuf> = ["settings.json", "sessions", "a.b.c.json"]
            .iter()
            .map(|name| Path::new("instance").join(name))
            .collect();
        // The `config.json` fallback branch of `temp_config_path`.
        targets.push(Path::new("instance").join(non_utf8_file_name()));

        for target in targets {
            let temp = super::temp_config_path(&target);
            let produced = temp
                .file_name()
                .and_then(|name| name.to_str())
                .expect("the temp name is composed by this module and is always UTF-8");
            assert!(
                matches_atomic_write_tmp_glob(produced),
                "temp_config_path produced {produced:?} for {target:?}, and the instance \
                 policy would leave it visible in git status"
            );
        }
    }

    // -----------------------------------------------------------------------
    // #1938 cross-process config write lock.
    //
    // The parent tests below re-execute this same lib test binary with
    // `--exact config::local_config_io::tests::issue_1937_config_lock_child`
    // and the child-only environment below. The child performs one action and
    // exits; parents bound the child lifetime and assert the named test pass,
    // not just the exit code. No app session, token, or real config directory
    // is used: every fixture is a fresh tempdir.
    // -----------------------------------------------------------------------

    const CHILD_ACTION_ENV: &str = "AC_1937_CONFIG_LOCK_CHILD_ACTION";
    const CHILD_DIR_ENV: &str = "AC_1937_CONFIG_LOCK_CHILD_DIR";
    const CHILD_TIMEOUT_MS_ENV: &str = "AC_1937_CONFIG_LOCK_CHILD_TIMEOUT_MS";
    const CHILD_KEY_ENV: &str = "AC_1937_CONFIG_LOCK_CHILD_KEY";
    const CHILD_TEST_FQN: &str = "config::local_config_io::tests::issue_1937_config_lock_child";
    const CHILD_READY_FILE: &str = "child-ready";
    const CHILD_RELEASE_FILE: &str = "child-release";

    fn lock_sidecar_path(config_path: &Path) -> PathBuf {
        let parent = config_path
            .parent()
            .expect("config path has a parent")
            .canonicalize()
            .expect("config parent canonicalizes");
        config_lock_path(&parent, config_path).expect("lock path")
    }

    fn wait_for_file(path: &Path, timeout: Duration, label: &str) {
        let started = Instant::now();
        while !path.exists() {
            assert!(
                started.elapsed() < timeout,
                "{label}: timed out waiting for {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn spawn_config_lock_child(action: &str, dir: &Path, extra_env: &[(&str, &str)]) -> Child {
        let exe = std::env::current_exe().expect("current test exe");
        let mut command = Command::new(exe);
        command
            .args(["--exact", CHILD_TEST_FQN, "--nocapture", "--test-threads=1"])
            .env(CHILD_ACTION_ENV, action)
            .env(CHILD_DIR_ENV, dir)
            .env_remove(CHILD_TIMEOUT_MS_ENV)
            .env_remove(CHILD_KEY_ENV)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        command.spawn().expect("spawn config lock child")
    }

    fn wait_bounded(
        mut child: Child,
        timeout: Duration,
        label: &str,
    ) -> (std::process::ExitStatus, String, String) {
        let started = Instant::now();
        loop {
            match child.try_wait().expect("poll child") {
                Some(_) => break,
                None if started.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                None => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{label} exceeded its {timeout:?} lifetime bound and was killed");
                }
            }
        }
        let output = child.wait_with_output().expect("collect child output");
        (
            output.status,
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    fn assert_child_success(
        label: &str,
        status: std::process::ExitStatus,
        stdout: &str,
        stderr: &str,
        marker: &str,
    ) {
        assert!(
            status.success(),
            "{label} child failed: status={status:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains(&format!("test {CHILD_TEST_FQN} ...")),
            "{label} child did not report {CHILD_TEST_FQN} running:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains("test result: ok. 1 passed; 0 failed"),
            "{label} child did not report exactly one passing test:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            stdout.contains(marker),
            "{label} child is missing marker {marker:?}:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    /// #1938 - the child half of the cross-process tests. Without the child-only
    /// action environment this test is a no-op, so a normal suite run (and the
    /// focused CI filter) passes it without touching any fixture.
    #[test]
    fn issue_1937_config_lock_child() {
        let Some(action) = std::env::var_os(CHILD_ACTION_ENV) else {
            return;
        };
        let action = action.to_string_lossy().into_owned();
        let dir = PathBuf::from(std::env::var_os(CHILD_DIR_ENV).expect("child dir env"));
        let config_path = dir.join("config.json");
        match action.as_str() {
            "hold" => {
                let _lock = acquire_config_file_write_lock(&config_path, CONFIG_LOCK_TIMEOUT)
                    .expect("child must acquire the lock");
                std::fs::write(dir.join(CHILD_READY_FILE), b"ready")
                    .expect("child must announce readiness");
                let deadline = Instant::now() + Duration::from_secs(60);
                while !dir.join(CHILD_RELEASE_FILE).exists() {
                    assert!(
                        Instant::now() < deadline,
                        "child hold exceeded its 60s bound"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            "assert_timeout" => {
                let timeout_ms: u64 = std::env::var(CHILD_TIMEOUT_MS_ENV)
                    .expect("child timeout env")
                    .parse()
                    .expect("child timeout ms");
                let error =
                    acquire_config_file_write_lock(&config_path, Duration::from_millis(timeout_ms))
                        .expect_err("child must not acquire a held lock");
                assert!(error.contains("configLockTimeout"), "child saw: {error}");
                assert!(error.contains(".config.json.lock"), "child saw: {error}");
                println!("AC_1937_CONFIG_LOCK_CHILD_TIMEOUT_OK");
            }
            "update" => {
                let key = std::env::var(CHILD_KEY_ENV).expect("child key env");
                for index in 0..25_u32 {
                    update_config_json_object(&config_path, true, |obj| {
                        obj.insert(key.clone(), serde_json::json!(index));
                        Ok(())
                    })
                    .expect("child update must succeed");
                }
                println!("AC_1937_CONFIG_LOCK_CHILD_UPDATE_OK key={key}");
            }
            other => panic!("unknown child action {other}"),
        }
        println!("AC_1937_CONFIG_LOCK_CHILD_DONE action={action}");
    }

    /// #1938 - the production deadline is 5 seconds and the injected short
    /// deadline is honored: a bounded wait with a distinct diagnostic naming
    /// the sidecar and the elapsed limit, then a clean reacquire.
    #[test]
    fn issue_1937_config_lock_timeout() {
        assert_eq!(CONFIG_LOCK_TIMEOUT, Duration::from_secs(5));
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        let held = acquire_config_file_write_lock(&path, CONFIG_LOCK_TIMEOUT).expect("hold lock");
        let started = Instant::now();
        let error = acquire_config_file_write_lock(&path, Duration::from_millis(150))
            .expect_err("a held lock must time out");
        let elapsed = started.elapsed();
        assert!(error.contains("configLockTimeout"), "{error}");
        assert!(error.contains(".config.json.lock"), "{error}");
        assert!(error.contains("150 ms"), "{error}");
        assert!(elapsed >= Duration::from_millis(150), "elapsed {elapsed:?}");
        assert!(
            elapsed < Duration::from_secs(2),
            "the wait must stay bounded, elapsed {elapsed:?}"
        );
        drop(held);
        let reacquired = acquire_config_file_write_lock(&path, Duration::from_millis(500))
            .expect("lock must be free after the holder drops");
        drop(reacquired);
        assert!(lock_sidecar_path(&path).is_file(), "sidecar must survive");
    }

    /// #1938 - exclusion across separate processes: a live holder in this
    /// process makes a separate test process time out.
    #[test]
    fn issue_1937_config_lock_process_exclusion() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        std::fs::write(&path, b"{}\n").expect("seed config");
        let _held = acquire_config_file_write_lock(&path, CONFIG_LOCK_TIMEOUT).expect("hold lock");

        let child = spawn_config_lock_child(
            "assert_timeout",
            temp.path(),
            &[(CHILD_TIMEOUT_MS_ENV, "400")],
        );
        let (status, stdout, stderr) =
            wait_bounded(child, Duration::from_secs(60), "process_exclusion child");
        assert_child_success(
            "process_exclusion",
            status,
            &stdout,
            &stderr,
            "AC_1937_CONFIG_LOCK_CHILD_TIMEOUT_OK",
        );
        assert!(
            stdout.contains("AC_1937_CONFIG_LOCK_CHILD_DONE action=assert_timeout"),
            "{stdout}"
        );
    }

    /// #1938 - normal child exit releases the lock: while the child holds it the
    /// parent times out, and after the child exits the parent reacquires.
    #[test]
    fn issue_1937_config_lock_process_release() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        std::fs::write(&path, b"{}\n").expect("seed config");

        let child = spawn_config_lock_child("hold", temp.path(), &[]);
        wait_for_file(
            &temp.path().join(CHILD_READY_FILE),
            Duration::from_secs(60),
            "child ready",
        );

        let error = acquire_config_file_write_lock(&path, Duration::from_millis(300))
            .expect_err("the child holds the lock");
        assert!(error.contains("configLockTimeout"), "{error}");

        std::fs::write(temp.path().join(CHILD_RELEASE_FILE), b"go").expect("release child");
        let (status, stdout, stderr) =
            wait_bounded(child, Duration::from_secs(60), "process_release child");
        assert_child_success(
            "process_release",
            status,
            &stdout,
            &stderr,
            "AC_1937_CONFIG_LOCK_CHILD_DONE action=hold",
        );

        let reacquired = acquire_config_file_write_lock(&path, Duration::from_secs(2))
            .expect("child exit must release the lock");
        drop(reacquired);
        assert!(lock_sidecar_path(&path).is_file(), "sidecar must survive");
    }

    /// #1938 - process death (kill, no unwind) also releases the OS lock in the
    /// kernel, so no sidecar deletion or stale-lock cleanup is needed.
    #[test]
    fn issue_1937_config_lock_process_death_release() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        std::fs::write(&path, b"{}\n").expect("seed config");

        let mut child = spawn_config_lock_child("hold", temp.path(), &[]);
        wait_for_file(
            &temp.path().join(CHILD_READY_FILE),
            Duration::from_secs(60),
            "child ready",
        );
        let held_error = acquire_config_file_write_lock(&path, Duration::from_millis(200))
            .expect_err("the child holds the lock");
        assert!(held_error.contains("configLockTimeout"), "{held_error}");

        child.kill().expect("kill lock holder");
        let status = child.wait().expect("reap killed child");
        assert!(
            !status.success(),
            "killed child must not report success: {status:?}"
        );

        let reacquired = acquire_config_file_write_lock(&path, Duration::from_secs(2))
            .expect("process death must release the OS lock");
        drop(reacquired);
        assert!(
            lock_sidecar_path(&path).is_file(),
            "sidecar must survive process death"
        );
    }

    /// #1938 - separate handles in one process contend exactly like separate
    /// processes, so the guarantee does not depend on one caller shape.
    #[test]
    fn issue_1937_config_lock_same_process_handles() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        let first =
            acquire_config_file_write_lock(&path, CONFIG_LOCK_TIMEOUT).expect("first handle");
        let error = acquire_config_file_write_lock(&path, Duration::from_millis(120))
            .expect_err("the second handle must contend");
        assert!(error.contains("configLockTimeout"), "{error}");
        drop(first);
        let second = acquire_config_file_write_lock(&path, Duration::from_millis(500))
            .expect("the first handle dropped");
        drop(second);
    }

    /// #1938 - canonicalization folds raw, dot-segment, symlink, and Windows
    /// case / verbatim aliases of one directory onto the single physical
    /// sidecar.
    #[test]
    fn issue_1937_config_lock_path_aliases_share_one_sidecar() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path();
        let path = dir.join("config.json");
        let held = acquire_config_file_write_lock(&path, CONFIG_LOCK_TIMEOUT).expect("hold lock");

        let nested = dir.join("nested");
        std::fs::create_dir_all(&nested).expect("nested dir");
        let mut aliases = vec![
            dir.join(".").join("config.json"),
            nested.join("..").join("config.json"),
        ];
        #[cfg(windows)]
        {
            // Windows paths are case-insensitive, so `.CONFIG.JSON.lock` and
            // `.config.json.lock` are the same physical file.
            aliases.push(dir.join("CONFIG.JSON"));
            aliases.push(PathBuf::from(format!(r"\\?\{}", dir.display())).join("config.json"));
        }
        #[cfg(unix)]
        {
            let link = dir.join("alias");
            std::os::unix::fs::symlink(dir, &link).expect("symlink alias");
            aliases.push(link.join("config.json"));
        }

        for alias in aliases {
            let error = acquire_config_file_write_lock(&alias, Duration::from_millis(120))
                .err()
                .unwrap_or_else(|| panic!("alias {alias:?} must share the held sidecar lock"));
            assert!(
                error.contains("configLockTimeout"),
                "alias {alias:?}: {error}"
            );
        }
        drop(held);
        assert!(lock_sidecar_path(&path).is_file());
    }

    /// #1938 - a malformed config is rejected under the lock and the original
    /// bytes stay untouched.
    #[test]
    fn issue_1937_config_lock_malformed_json_preserved() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        std::fs::write(&path, b"{ invalid").expect("seed malformed config");

        let error = update_config_json_object(&path, false, |_| Ok(()))
            .expect_err("malformed JSON must fail");
        assert!(error.contains("Failed to parse"), "{error}");
        assert_eq!(std::fs::read(&path).expect("read"), b"{ invalid");
        assert!(lock_sidecar_path(&path).is_file(), "sidecar must survive");
        let reacquired = acquire_config_file_write_lock(&path, Duration::from_millis(500))
            .expect("lock must be released after a parse failure");
        drop(reacquired);
    }

    /// #1938 - a failing mutation closure leaves the file untouched and releases
    /// the lock.
    #[test]
    fn issue_1937_config_lock_closure_failure_preserved() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        let before = br#"{"keep":true}"#.to_vec();
        std::fs::write(&path, &before).expect("seed config");

        let error = update_config_json_object(&path, false, |obj| {
            obj.insert("scratch".to_string(), serde_json::json!(1));
            Err("forced closure failure".to_string())
        })
        .expect_err("the closure failure must surface");
        assert!(error.contains("forced closure failure"), "{error}");
        assert_eq!(std::fs::read(&path).expect("read"), before);
        assert!(lock_sidecar_path(&path).is_file(), "sidecar must survive");
        let reacquired = acquire_config_file_write_lock(&path, Duration::from_millis(500))
            .expect("lock must be released after a closure failure");
        drop(reacquired);
    }

    /// #1938 - an atomic publish failure preserves the prior bytes, removes the
    /// temp file, releases the lock, and leaves the sidecar in place.
    #[test]
    fn issue_1937_config_lock_publish_failure_preserves_prior_bytes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        let before = br#"{"keep":true}"#.to_vec();
        std::fs::write(&path, &before).expect("seed config");

        let error = update_config_json_object_with_publish(
            &path,
            false,
            |obj| {
                obj.insert("scratch".to_string(), serde_json::json!(2));
                Ok(())
            },
            |_tmp, _target| Err("forced publish failure".to_string()),
        )
        .expect_err("the publish failure must surface");
        assert!(error.contains("forced publish failure"), "{error}");
        assert_eq!(std::fs::read(&path).expect("read"), before);
        let temp_files: Vec<PathBuf> = std::fs::read_dir(temp.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|entry| entry.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            temp_files.is_empty(),
            "temp files left behind: {temp_files:?}"
        );
        let lock_path = lock_sidecar_path(&path);
        assert!(
            lock_path.is_file(),
            "sidecar must survive a publish failure"
        );
        assert_eq!(
            std::fs::read(&lock_path).expect("read lock"),
            b"",
            "lock contents must never be written"
        );
        let reacquired = acquire_config_file_write_lock(&path, Duration::from_millis(500))
            .expect("lock must be released after a publish failure");
        drop(reacquired);
    }

    /// #1938 - the sidecar is never deleted, including error recovery, and it is
    /// never truncated or written to.
    #[test]
    fn issue_1937_config_lock_sidecar_survives_every_outcome() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        std::fs::write(&path, b"{}\n").expect("seed config");

        update_config_json_object(&path, false, |obj| {
            obj.insert("ok".to_string(), serde_json::json!(true));
            Ok(())
        })
        .expect("successful update");
        let lock_path = lock_sidecar_path(&path);
        assert!(lock_path.is_file(), "sidecar must exist after success");
        assert_eq!(std::fs::read(&lock_path).expect("read"), b"");

        let _ = update_config_json_object(&path, false, |_| Err("boom".to_string()));
        assert!(
            lock_path.is_file(),
            "sidecar must exist after a closure failure"
        );

        std::fs::write(&path, b"{ nope").expect("seed malformed config");
        let _ = update_config_json_object(&path, false, |_| Ok(()));
        assert!(
            lock_path.is_file(),
            "sidecar must exist after a parse failure"
        );

        std::fs::write(&path, b"{}\n").expect("reseed config");
        let _ = update_config_json_object_with_publish(
            &path,
            false,
            |_| Ok(()),
            |_, _| Err("forced".to_string()),
        );
        assert!(
            lock_path.is_file(),
            "sidecar must exist after a publish failure"
        );
        assert_eq!(std::fs::read(&lock_path).expect("read"), b"");
    }

    /// #1938 - two live processes update disjoint keys; the cross-process lock
    /// serializes their read-modify-publish cycles so both keys survive.
    #[test]
    fn issue_1937_config_lock_concurrent_disjoint_updates_both_survive() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        std::fs::write(&path, b"{}\n").expect("seed config");

        let alpha = spawn_config_lock_child("update", temp.path(), &[(CHILD_KEY_ENV, "alpha")]);
        let beta = spawn_config_lock_child("update", temp.path(), &[(CHILD_KEY_ENV, "beta")]);

        let (alpha_status, alpha_stdout, alpha_stderr) =
            wait_bounded(alpha, Duration::from_secs(120), "concurrent update alpha");
        let (beta_status, beta_stdout, beta_stderr) =
            wait_bounded(beta, Duration::from_secs(120), "concurrent update beta");

        assert_child_success(
            "concurrent alpha",
            alpha_status,
            &alpha_stdout,
            &alpha_stderr,
            "AC_1937_CONFIG_LOCK_CHILD_UPDATE_OK key=alpha",
        );
        assert_child_success(
            "concurrent beta",
            beta_status,
            &beta_stdout,
            &beta_stderr,
            "AC_1937_CONFIG_LOCK_CHILD_UPDATE_OK key=beta",
        );

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read saved config"))
                .expect("saved config is JSON");
        assert_eq!(saved["alpha"], serde_json::json!(24), "{saved}");
        assert_eq!(saved["beta"], serde_json::json!(24), "{saved}");
        assert!(lock_sidecar_path(&path).is_file(), "sidecar must survive");
    }

    #[cfg(windows)]
    fn wide_to_string(wide: &[u16]) -> String {
        assert_eq!(wide.last(), Some(&0), "output must be NUL-terminated");
        String::from_utf16(&wide[..wide.len() - 1]).expect("valid UTF-16")
    }

    #[cfg(windows)]
    #[test]
    fn publish_path_wide_prefixes_a_plain_drive_path() {
        let out = super::publish_path_wide(Path::new(r"C:\a\config.json"));
        assert_eq!(wide_to_string(&out), r"\\?\C:\a\config.json");
    }

    #[cfg(windows)]
    #[test]
    fn publish_path_wide_normalizes_forward_slashes_and_unc() {
        let cases = [
            (r"C:/a/./config.json", r"\\?\C:\a\config.json"),
            (
                r"\\server\share\config.json",
                r"\\?\UNC\server\share\config.json",
            ),
            // Round-2 finding: the raw prefix text `//server/share` must not
            // be copied, or the server would be named `server/share`.
            (
                r"//server/share/config.json",
                r"\\?\UNC\server\share\config.json",
            ),
        ];
        for (input, expected) in cases {
            let out = super::publish_path_wide(Path::new(input));
            assert_eq!(wide_to_string(&out), expected, "input {input}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn publish_path_wide_leaves_unconvertible_shapes_byte_identical() {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        let mut nul_units: Vec<u16> = r"C:\a\conf".encode_utf16().collect();
        nul_units.push(0);
        nul_units.extend("ig.json".encode_utf16());
        let interior_nul = PathBuf::from(OsString::from_wide(&nul_units));

        let cases: Vec<(&str, PathBuf)> = vec![
            ("relative", PathBuf::from(r"sub\config.json")),
            ("drive-relative", PathBuf::from(r"C:config.json")),
            ("bare drive", PathBuf::from(r"C:")),
            ("parent dir", PathBuf::from(r"C:\a\..\config.json")),
            (
                "trailing-dot file name",
                PathBuf::from(r"C:\a\config.json."),
            ),
            (
                "trailing-space directory",
                PathBuf::from(r"C:\a \config.json"),
            ),
            ("already verbatim", PathBuf::from(r"\\?\C:\a\config.json")),
            ("device namespace", PathBuf::from(r"\\.\PIPE\x")),
            ("interior NUL", interior_nul),
        ];
        for (name, p) in cases {
            let expected: Vec<u16> = p
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>();
            assert_eq!(super::publish_path_wide(&p), expected, "case {name}");
        }
    }

    /// Builds a non-verbatim directory over 300 wide units holding an existing
    /// `config.json` (old bytes) and a temp file (new bytes), and asserts the
    /// fixture rules of plan #2378 section 5.1 before the caller acts.
    #[cfg(windows)]
    fn long_path_publish_fixture(root: &Path) -> (PathBuf, PathBuf) {
        use std::os::windows::ffi::OsStrExt;
        use std::path::{Component, Prefix};

        let segment = "a".repeat(40);
        let mut dir = root.to_path_buf();
        while dir.as_os_str().encode_wide().count() <= 300 {
            dir.push(&segment);
        }
        std::fs::create_dir_all(&dir).expect("create long dir");
        let dest = dir.join("config.json");
        let tmp = dir.join(".config.json.1.tmp");
        std::fs::write(&dest, b"old content").expect("write dest");
        std::fs::write(&tmp, b"new content").expect("write tmp");

        for p in [&dest, &tmp] {
            match p.components().next() {
                Some(Component::Prefix(prefix)) => assert!(
                    matches!(prefix.kind(), Prefix::Disk(_)),
                    "fixture must be an ordinary Disk path, got {:?}",
                    prefix.kind()
                ),
                other => panic!("fixture must start with a prefix, got {other:?}"),
            }
            let wide_len = p.as_os_str().encode_wide().count();
            assert!(
                wide_len > 260,
                "fixture length {wide_len} must exceed MAX_PATH"
            );
            assert!(p.exists(), "fixture file must exist: {}", p.display());
        }
        (dest, tmp)
    }

    #[cfg(windows)]
    #[test]
    fn publish_over_a_destination_longer_than_max_path_succeeds() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (dest, tmp) = long_path_publish_fixture(temp.path());
        assert!(dest.exists());
        assert!(tmp.exists());

        super::publish_temp_config(&tmp, &dest).expect("long-path publish");

        assert_eq!(std::fs::read(&dest).expect("read dest"), b"new content");
        assert!(!tmp.exists(), "temp file must be consumed");
    }

    /// Inverse control for the long-path publish test: raw non-verbatim
    /// arguments still fail with ERROR_PATH_NOT_FOUND, so that test is not
    /// vacuous. If this ever goes red, the process gained long-path awareness
    /// (for example through a manifest); re-triage #2378 instead of relaxing
    /// this test.
    #[cfg(windows)]
    #[test]
    fn raw_non_verbatim_replacefilew_still_fails_over_max_path() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

        let temp = tempfile::tempdir().expect("tempdir");
        let (dest, tmp) = long_path_publish_fixture(temp.path());
        assert!(dest.exists());
        assert!(tmp.exists());

        let dest_wide: Vec<u16> = dest
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let tmp_wide: Vec<u16> = tmp
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let ok = unsafe {
            ReplaceFileW(
                dest_wide.as_ptr(),
                tmp_wide.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        let err = std::io::Error::last_os_error();
        assert_eq!(ok, 0, "raw ReplaceFileW must fail over MAX_PATH");
        assert_eq!(err.raw_os_error(), Some(3), "{err}");
    }

    // -----------------------------------------------------------------------
    // #2786 (C1) - the config pair primitive. The state path is a literal and
    // the stage hook is test-local: this module must name no other module.
    // -----------------------------------------------------------------------

    const PAIR_STATE_NAME: &str = "config.state.no-git.json";
    const PAIR_STATE_KEYS: [&str; 4] = [
        "lastCodingAgent",
        "codingAgents",
        "lastAgentMessageAt",
        "profileContentHash",
    ];

    fn pair_fixture(
        decisions: Option<&str>,
        state: Option<&str>,
    ) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().expect("tempdir");
        let decisions_path = temp.path().join("config.json");
        let state_path = temp.path().join(PAIR_STATE_NAME);
        if let Some(body) = decisions {
            std::fs::write(&decisions_path, body).expect("seed decisions");
        }
        if let Some(body) = state {
            std::fs::write(&state_path, body).expect("seed state");
        }
        (temp, decisions_path, state_path)
    }

    fn no_stage(_stage: &str) {}

    fn read_value(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read json"))
            .expect("parse json")
    }

    fn split_marker() -> Value {
        json!({"v": 1, "keys": PAIR_STATE_KEYS})
    }

    #[test]
    fn the_state_file_is_written_before_the_decisions_file() {
        let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), None);
        // The decisions write fails: its temp path is occupied by a directory.
        std::fs::create_dir(temp_config_path(&decisions)).expect("occupy decisions temp");

        let err = update_config_pair(
            &decisions,
            &state,
            &PAIR_STATE_KEYS,
            None,
            &no_stage,
            |d, s| {
                d.insert("a".to_string(), json!(2));
                s.insert("tooling".to_string(), json!({"lastCodingAgent": "claude"}));
                Ok(())
            },
        )
        .expect_err("the decisions write must fail");

        assert!(err.contains("temp config"), "{err}");
        assert!(
            state.is_file(),
            "the state file must already be written: {err}"
        );
        assert_eq!(read_value(&state)["tooling"]["lastCodingAgent"], "claude");
        assert_eq!(std::fs::read_to_string(&decisions).unwrap(), r#"{"a":1}"#);
    }

    #[test]
    fn an_absent_state_file_is_an_empty_tooling() {
        let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), None);
        let saw_empty = std::cell::Cell::new(false);

        update_config_pair(
            &decisions,
            &state,
            &PAIR_STATE_KEYS,
            None,
            &no_stage,
            |_d, s| {
                saw_empty.set(s.is_empty());
                Ok(())
            },
        )
        .expect("an absent state file is a valid state");

        assert!(saw_empty.get(), "the closure must see an empty state map");
        assert!(
            !state.exists(),
            "an untouched absent state file stays absent"
        );
    }

    #[test]
    fn an_untouched_side_is_not_written() {
        // Decisions only: no state file appears.
        let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), None);
        update_config_pair(
            &decisions,
            &state,
            &PAIR_STATE_KEYS,
            None,
            &no_stage,
            |d, _s| {
                d.insert("a".to_string(), json!(2));
                Ok(())
            },
        )
        .expect("decisions-only pair call");
        assert_eq!(read_value(&decisions)["a"], 2);
        assert!(
            !state.exists(),
            "a decisions-only call must not create a state file"
        );

        // State only: the decisions file stays byte-identical.
        let original = "{\"a\":1,  \"b\" : [1,2]}";
        let (_temp, decisions, state) = pair_fixture(Some(original), None);
        update_config_pair(
            &decisions,
            &state,
            &PAIR_STATE_KEYS,
            None,
            &no_stage,
            |_d, s| {
                s.insert(
                    "tooling".to_string(),
                    json!({"lastAgentMessageAt": "2026-09-29T00:00:00Z"}),
                );
                Ok(())
            },
        )
        .expect("state-only pair call");
        assert_eq!(std::fs::read_to_string(&decisions).unwrap(), original);
        assert_eq!(
            read_value(&state)["tooling"]["lastAgentMessageAt"],
            "2026-09-29T00:00:00Z"
        );
    }

    #[test]
    fn an_unparseable_state_file_blocks_the_pair() {
        let (_temp, decisions, state) = pair_fixture(Some(r#"{"a":1}"#), Some("{ not json"));
        let ran = std::cell::Cell::new(false);

        let err = update_config_pair(
            &decisions,
            &state,
            &PAIR_STATE_KEYS,
            None,
            &no_stage,
            |d, s| {
                ran.set(true);
                d.insert("a".to_string(), json!(2));
                s.insert("tooling".to_string(), json!({}));
                Ok(())
            },
        )
        .expect_err("an unparseable state file must block the pair");

        assert!(err.contains(&decisions.display().to_string()), "{err}");
        assert!(err.contains(&state.display().to_string()), "{err}");
        assert!(!ran.get(), "mutate must not run");
        assert_eq!(std::fs::read_to_string(&decisions).unwrap(), r#"{"a":1}"#);
        assert_eq!(std::fs::read_to_string(&state).unwrap(), "{ not json");
    }

    #[test]
    fn the_pair_preserves_unknown_keys_on_both_sides() {
        let decisions_seed =
            json!({"unknownDecision": {"x": [1, 2]}, "tooling": {"telegramBot": "b"}});
        let state_seed = json!({"unknownState": "keep", "split": split_marker(), "tooling": {}});
        let (_temp, decisions, state) = pair_fixture(
            Some(&decisions_seed.to_string()),
            Some(&state_seed.to_string()),
        );

        update_config_pair(
            &decisions,
            &state,
            &PAIR_STATE_KEYS,
            None,
            &no_stage,
            |d, s| {
                d.insert("context".to_string(), json!(["Role.md"]));
                s.insert("tooling".to_string(), json!({"lastCodingAgent": "claude"}));
                Ok(())
            },
        )
        .expect("pair call");

        let saved_decisions = read_value(&decisions);
        let saved_state = read_value(&state);
        assert_eq!(
            saved_decisions["unknownDecision"],
            decisions_seed["unknownDecision"]
        );
        assert_eq!(saved_decisions["tooling"], decisions_seed["tooling"]);
        assert_eq!(saved_state["unknownState"], state_seed["unknownState"]);
        assert_eq!(saved_state["split"], split_marker());
        assert_eq!(saved_decisions["context"][0], "Role.md");
        assert_eq!(saved_state["tooling"]["lastCodingAgent"], "claude");
    }

    #[test]
    fn the_guard_is_not_released_between_the_two_sequences() {
        let (_temp, decisions, state) = pair_fixture(
            Some(r#"{"tooling":{"lastCodingAgent":"claude","telegramBot":"b"}}"#),
            None,
        );
        let (paused_tx, paused_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();

        let worker = {
            let decisions = decisions.clone();
            let state = state.clone();
            std::thread::spawn(move || {
                let on_stage = |stage: &str| {
                    if stage == "after_cleanup_tracked_write" {
                        paused_tx.send(()).expect("announce pause");
                        release_rx
                            .recv_timeout(Duration::from_secs(60))
                            .expect("release the pause");
                    }
                };
                let cleanup = |d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
                    let moved = d
                        .get_mut("tooling")
                        .and_then(Value::as_object_mut)
                        .and_then(|tooling| tooling.remove("lastCodingAgent"))
                        .ok_or_else(|| "no key to move".to_string())?;
                    s.insert("tooling".to_string(), json!({"lastCodingAgent": moved}));
                    s.insert("split".to_string(), split_marker());
                    Ok(())
                };
                update_config_pair(
                    &decisions,
                    &state,
                    &PAIR_STATE_KEYS,
                    Some(&cleanup),
                    &on_stage,
                    |_d, _s| Ok(()),
                )
            })
        };

        paused_rx
            .recv_timeout(Duration::from_secs(60))
            .expect("the call must reach after_cleanup_tracked_write");
        let blocked = acquire_config_file_write_lock(&decisions, Duration::from_millis(300));
        let blocked_err = blocked
            .as_ref()
            .err()
            .cloned()
            .unwrap_or_else(|| "acquired".to_string());
        drop(blocked);
        release_tx.send(()).expect("release");
        worker.join().expect("worker").expect("pair call");

        assert!(
            blocked_err.contains("configLockTimeout"),
            "a second acquirer must fail while the pause holds, got: {blocked_err}"
        );
        drop(
            acquire_config_file_write_lock(&decisions, Duration::from_millis(500))
                .expect("the sidecar must be free once the call returns"),
        );
        let saved_state = read_value(&state);
        assert_eq!(saved_state["tooling"]["lastCodingAgent"], "claude");
        assert_eq!(saved_state["split"], split_marker());
        let saved_decisions = read_value(&decisions);
        assert!(saved_decisions["tooling"].get("lastCodingAgent").is_none());
        assert_eq!(saved_decisions["tooling"]["telegramBot"], "b");
    }

    #[test]
    fn the_cleanup_sequence_is_ordered_and_aborts_whole() {
        let decisions_seed = r#"{"tooling":{"lastCodingAgent":"claude"}}"#;
        let state_seed = r#"{"tooling":{"lastAgentMessageAt":"x"}}"#;

        // Leg 1: a failing cleanup writes nothing and mutate never runs.
        let (_temp, decisions, state) = pair_fixture(Some(decisions_seed), Some(state_seed));
        let ran = std::cell::Cell::new(false);
        let failing = |_d: &mut Map<String, Value>, _s: &mut Map<String, Value>| {
            Err("policy refused".to_string())
        };
        let err = update_config_pair(
            &decisions,
            &state,
            &PAIR_STATE_KEYS,
            Some(&failing),
            &no_stage,
            |d, s| {
                ran.set(true);
                d.insert("a".to_string(), json!(1));
                s.insert("a".to_string(), json!(1));
                Ok(())
            },
        )
        .expect_err("a failing cleanup aborts the call");
        assert!(!ran.get(), "mutate must never run after a failed cleanup");
        assert!(err.contains("policy refused"), "{err}");
        assert!(err.contains(&decisions.display().to_string()), "{err}");
        assert!(err.contains(&state.display().to_string()), "{err}");
        assert_eq!(std::fs::read_to_string(&decisions).unwrap(), decisions_seed);
        assert_eq!(std::fs::read_to_string(&state).unwrap(), state_seed);

        // Leg 2: a state-only cleanup, writing the marker, fires only its
        // state stage and leaves the decisions file byte-identical.
        let (_temp, decisions, state) = pair_fixture(Some(decisions_seed), None);
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let on_stage = |stage: &str| tx.send(stage.to_string()).expect("record stage");
        let state_only = |_d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
            s.insert("split".to_string(), split_marker());
            Ok(())
        };
        update_config_pair(
            &decisions,
            &state,
            &PAIR_STATE_KEYS,
            Some(&state_only),
            &on_stage,
            |_d, _s| Ok(()),
        )
        .expect("state-only cleanup");
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(200)).as_deref(),
            Ok("after_cleanup_state_publish")
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "after_cleanup_tracked_write must not fire for a state-only cleanup"
        );
        assert_eq!(std::fs::read_to_string(&decisions).unwrap(), decisions_seed);
        assert_eq!(read_value(&state)["split"], split_marker());

        // Leg 3: no cleanup behaves as a single-sequence call.
        let (_temp, decisions, state) = pair_fixture(Some(decisions_seed), None);
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let on_stage = |stage: &str| tx.send(stage.to_string()).expect("record stage");
        update_config_pair(
            &decisions,
            &state,
            &PAIR_STATE_KEYS,
            None,
            &on_stage,
            |d, _s| {
                d.insert("context".to_string(), json!([]));
                Ok(())
            },
        )
        .expect("single-sequence call");
        drop(tx);
        let stages: Vec<String> = rx.try_iter().collect();
        assert_eq!(stages, vec!["after_caller_decisions_write".to_string()]);
        assert!(
            !state.exists(),
            "a decisions-only mutate must not create a state file"
        );
    }

    fn prepared_fixture_plan() -> PreparedConfigPairPlan {
        let before = [
            PhysicalState::from_bytes(br#"{"d":0,"unknown":[1,2]}"#.to_vec()),
            PhysicalState::from_bytes(br#"{"s":0,"unknown":null}"#.to_vec()),
        ];
        let cleanup = |d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
            d.insert("d".into(), json!(1));
            s.insert("s".into(), json!(1));
            Ok(())
        };
        prepare_config_pair_plan(before, Some(&cleanup), |d, s| {
            d.insert("d".into(), json!(2));
            s.insert("s".into(), json!(2));
            Ok(())
        })
        .unwrap()
    }

    fn prepared_install(path: &Path, image: &PhysicalState) {
        match image {
            PhysicalState::Absent => {
                if path.exists() {
                    std::fs::remove_file(path).unwrap();
                }
            }
            PhysicalState::Bytes { bytes, .. } => std::fs::write(path, bytes).unwrap(),
        }
    }

    #[test]
    fn prepared_all_25_side_combinations_reachable_or_conflict() {
        let plan = prepared_fixture_plan();
        let d_steps = [0, 0, 1, 1, 2];
        let s_steps = [0, 1, 1, 2, 2];
        let mut reached = 0;
        let mut conflicts = 0;
        for (i, &d_step) in d_steps.iter().enumerate() {
            for (j, &s_step) in s_steps.iter().enumerate() {
                let (_temp, d, s) = pair_fixture(None, None);
                prepared_install(&d, &plan.stages[i][0]);
                prepared_install(&s, &plan.stages[j][1]);
                let before = [std::fs::read(&d).unwrap(), std::fs::read(&s).unwrap()];
                let reachable = s_step == d_step || s_step == d_step + 1;
                let result = execute_prepared_config_pair(&d, &s, &plan);
                if reachable {
                    result.unwrap();
                    reached += 1;
                    assert_eq!(
                        [
                            read_config_pair_physical(&d).unwrap(),
                            read_config_pair_physical(&s).unwrap()
                        ],
                        plan.stages[4]
                    );
                    assert_eq!(read_value(&d)["unknown"], json!([1, 2]));
                    assert_eq!(read_value(&s)["unknown"], Value::Null);
                } else {
                    assert_eq!(result, Err(PreparedPairError::Conflict));
                    conflicts += 1;
                    assert_eq!(
                        [std::fs::read(&d).unwrap(), std::fs::read(&s).unwrap()],
                        before
                    );
                }
            }
        }
        assert_eq!((reached, conflicts), (16, 9));
    }

    #[test]
    fn prepared_each_publish_cut_original_plan_resume_idempotent() {
        for cut in 1..=4 {
            let plan = prepared_fixture_plan();
            let (_temp, d, s) = pair_fixture(None, None);
            prepared_install(&d, &plan.stages[0][0]);
            prepared_install(&s, &plan.stages[0][1]);
            let stages = std::cell::RefCell::new(Vec::new());
            let result = execute_prepared_config_pair_with_stage(&d, &s, &plan, &|stage| {
                stages.borrow_mut().push(stage);
                assert!(write_file_atomic(&d, b"{}")
                    .unwrap_err()
                    .contains("Nested config write"));
                // Both sidecars remain held across cleanup and caller images.
                for path in [&d, &s] {
                    assert!(acquire_config_file_write_lock(path, Duration::ZERO)
                        .unwrap_err()
                        .contains("configLockTimeout"));
                }
                if stage == cut {
                    Err(PreparedPairError::Io("lost progress".into()))
                } else {
                    Ok(())
                }
            });
            assert_eq!(result, Err(PreparedPairError::Io("lost progress".into())));
            assert_eq!(*stages.borrow(), (1..=cut).collect::<Vec<_>>());
            assert_eq!(
                [
                    read_config_pair_physical(&d).unwrap(),
                    read_config_pair_physical(&s).unwrap()
                ],
                plan.stages[cut]
            );
            let restored: PreparedConfigPairPlan =
                serde_json::from_slice(&serde_json::to_vec(&plan).unwrap()).unwrap();
            execute_prepared_config_pair(&d, &s, &restored).unwrap();
            execute_prepared_config_pair_with_stage(&d, &s, &restored, &|_| {
                panic!("complete replay publishes nothing")
            })
            .unwrap();
            assert_eq!(
                [
                    read_config_pair_physical(&d).unwrap(),
                    read_config_pair_physical(&s).unwrap()
                ],
                plan.stages[4]
            );
        }
    }

    #[test]
    fn prepared_initial_absence_collapsed_stages_and_empty_object_distinct() {
        let (_temp, d, s) = pair_fixture(None, None);
        let plan = prepare_config_pair_plan(
            [PhysicalState::Absent, PhysicalState::Absent],
            None,
            |_, _| Ok(()),
        )
        .unwrap();
        execute_prepared_config_pair_with_stage(&d, &s, &plan, &|_| panic!("no target writes"))
            .unwrap();
        assert!(!d.exists() && !s.exists());
        std::fs::write(&s, b"{}").unwrap();
        assert_eq!(
            execute_prepared_config_pair(&d, &s, &plan),
            Err(PreparedPairError::Conflict)
        );
        let create = prepare_config_pair_plan(
            [
                PhysicalState::Absent,
                PhysicalState::from_bytes(b"{}".to_vec()),
            ],
            None,
            |d, _| {
                d.insert("owned".into(), json!(true));
                Ok(())
            },
        )
        .unwrap();
        execute_prepared_config_pair(&d, &s, &create).unwrap();
        assert_eq!(std::fs::read(&s).unwrap(), b"{}");
        assert_eq!(read_value(&d)["owned"], true);
    }

    #[test]
    fn prepared_external_bytes_disappearance_and_mid_stage_drift_no_clobber() {
        for external in [Some(b"{broken".as_slice()), Some(b"{}".as_slice()), None] {
            let plan = prepared_fixture_plan();
            let (_temp, d, s) = pair_fixture(None, None);
            prepared_install(&d, &plan.stages[0][0]);
            prepared_install(&s, &plan.stages[0][1]);
            if let Some(bytes) = external {
                std::fs::write(&s, bytes).unwrap();
            } else {
                std::fs::remove_file(&s).unwrap();
            }
            assert_eq!(
                execute_prepared_config_pair(&d, &s, &plan),
                Err(PreparedPairError::Conflict)
            );
            assert_eq!(read_config_pair_physical(&d).unwrap(), plan.stages[0][0]);
            assert_eq!(
                read_config_pair_physical(&s).unwrap(),
                external
                    .map(|b| PhysicalState::from_bytes(b.to_vec()))
                    .unwrap_or(PhysicalState::Absent)
            );
        }
        let plan = prepared_fixture_plan();
        let (_temp, d, s) = pair_fixture(None, None);
        prepared_install(&d, &plan.stages[0][0]);
        prepared_install(&s, &plan.stages[0][1]);
        let result = execute_prepared_config_pair_with_stage(&d, &s, &plan, &|stage| {
            assert_eq!(stage, 1);
            std::fs::write(&d, b"{\"external\":true}").unwrap();
            Ok(())
        });
        assert_eq!(result, Err(PreparedPairError::Conflict));
        assert_eq!(std::fs::read(&d).unwrap(), b"{\"external\":true}");
        assert_eq!(read_config_pair_physical(&s).unwrap(), plan.stages[1][1]);
    }

    #[test]
    fn prepared_rejects_invalid_plan_and_nested_writer_before_target_effects() {
        let (_temp, d, s) = pair_fixture(None, None);
        let nested = prepare_config_pair_plan(
            [PhysicalState::Absent, PhysicalState::Absent],
            None,
            |_, _| write_file_atomic(&d, b"{}"),
        );
        assert!(nested
            .unwrap_err()
            .to_string()
            .contains("Nested config write"));
        assert!(!d.exists() && !s.exists());
        let rejected = prepare_config_pair_plan(
            [PhysicalState::Absent, PhysicalState::Absent],
            None,
            |d, _| {
                d.insert("owned".into(), json!(1));
                Err("rejected".into())
            },
        );
        assert!(matches!(rejected, Err(PreparedPairError::Preparation(_))));
        assert!(!d.exists());
        for malformed in [b"null".as_slice(), b"[]".as_slice(), b"{broken".as_slice()] {
            assert!(prepare_config_pair_plan(
                [
                    PhysicalState::from_bytes(malformed.to_vec()),
                    PhysicalState::Absent
                ],
                None,
                |_, _| Ok(())
            )
            .is_err());
        }
        let mut plan = prepared_fixture_plan();
        if let PhysicalState::Bytes { sha256, .. } = &mut plan.stages[0][0] {
            *sha256 = "forged".into();
        }
        assert!(matches!(
            execute_prepared_config_pair(&d, &s, &plan),
            Err(PreparedPairError::InvalidPlan(_))
        ));
        let mut plan = prepared_fixture_plan();
        plan.stages[1][0] = plan.stages[4][0].clone();
        assert!(matches!(
            execute_prepared_config_pair(&d, &s, &plan),
            Err(PreparedPairError::InvalidPlan(_))
        ));
        assert!(!d.exists() && !s.exists());
        assert!(!lock_sidecar_path(&d).exists());
    }

    #[test]
    fn prepared_publish_failure_retains_reachable_stage_and_releases_locks() {
        let plan = prepared_fixture_plan();
        let (_temp, d, s) = pair_fixture(None, None);
        prepared_install(&d, &plan.stages[0][0]);
        prepared_install(&s, &plan.stages[0][1]);
        std::fs::create_dir(temp_config_path(&d)).unwrap();
        assert!(matches!(
            execute_prepared_config_pair(&d, &s, &plan),
            Err(PreparedPairError::Io(_))
        ));
        assert_eq!(
            [
                read_config_pair_physical(&d).unwrap(),
                read_config_pair_physical(&s).unwrap()
            ],
            plan.stages[1]
        );
        for path in [&d, &s] {
            drop(acquire_config_file_write_lock(path, Duration::ZERO).unwrap());
        }
        std::fs::remove_dir(temp_config_path(&d)).unwrap();
        execute_prepared_config_pair(&d, &s, &plan).unwrap();
    }

    #[test]
    fn prepared_expected_absent_publication_preserves_other_creator() {
        let (_temp, d, _s) = pair_fixture(Some("{\"other\":1}"), None);
        let after = PhysicalState::from_bytes(b"{\"owned\":1}\n".to_vec());
        assert_eq!(
            publish_prepared_pair_image(&d, &PhysicalState::Absent, &after),
            Err(PreparedPairError::Conflict)
        );
        assert_eq!(std::fs::read(&d).unwrap(), b"{\"other\":1}");
        assert!(!temp_config_path(&d).exists());
    }

    #[test]
    fn prepared_sidecar_timeout_and_process_death_release() {
        // Reuse the exact lock's bounded existing child-process coverage.
        issue_1937_config_lock_process_death_release();
        issue_1937_config_lock_same_process_handles();
    }

    const NESTED_CHILD_ACTION_ENV: &str = "AC_2786_NESTED_WRITER_CHILD_ACTION";
    const NESTED_CHILD_DIR_ENV: &str = "AC_2786_NESTED_WRITER_CHILD_DIR";
    const NESTED_CHILD_TEST_FQN: &str =
        "config::local_config_io::tests::a_nested_config_writer_child";

    /// #2786 (C1) - the child half of E2c. A no-op without the child-only
    /// environment, so a normal suite run passes it untouched.
    #[test]
    fn a_nested_config_writer_child() {
        let Some(action) = std::env::var_os(NESTED_CHILD_ACTION_ENV) else {
            return;
        };
        let action = action.to_string_lossy().into_owned();
        let dir = PathBuf::from(std::env::var_os(NESTED_CHILD_DIR_ENV).expect("child dir env"));
        let decisions = dir.join("config.json");
        let state = dir.join(PAIR_STATE_NAME);
        let other = dir.join("other.json");

        if action == "panic" {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                update_config_pair(
                    &decisions,
                    &state,
                    &PAIR_STATE_KEYS,
                    None,
                    &no_stage,
                    |_d, _s| panic!("forced panic inside mutate"),
                )
            }));
            assert!(
                outcome.is_err(),
                "the mutate panic must reach the test boundary"
            );
            update_config_json_object(&decisions, true, |obj| {
                obj.insert("sameThread".to_string(), json!(true));
                Ok(())
            })
            .expect("a normal write on the same thread must succeed after a panic");
            let other_thread = other.clone();
            std::thread::spawn(move || write_file_atomic(&other_thread, b"{}\n"))
                .join()
                .expect("writer thread")
                .expect("a normal write on another thread must succeed after a panic");
            println!("AC_2786_NESTED_WRITER_CHILD_OK action={action}");
            return;
        }

        let entry = match action.as_str() {
            "nest_pair" => "update_config_pair",
            "nest_json" => "update_config_json_object",
            "nest_atomic" => "write_file_atomic",
            other => panic!("unknown child action {other}"),
        };
        let err = update_config_pair(
            &decisions,
            &state,
            &PAIR_STATE_KEYS,
            None,
            &no_stage,
            |_d, _s| match entry {
                "update_config_pair" => update_config_pair(
                    &other,
                    &state,
                    &PAIR_STATE_KEYS,
                    None,
                    &no_stage,
                    |_d, _s| Ok(()),
                ),
                "update_config_json_object" => {
                    update_config_json_object(&other, true, |_obj| Ok(())).map(|_| ())
                }
                _ => write_file_atomic(&other, b"{}\n"),
            },
        )
        .expect_err("a nested config writer must return an error");
        assert!(
            err.contains(&format!("Nested config write: {entry}")),
            "the error must name the nesting: {err}"
        );
        update_config_json_object(&decisions, true, |obj| {
            obj.insert("afterNesting".to_string(), json!(true));
            Ok(())
        })
        .expect("the thread must stay writable after the nested error");
        println!("AC_2786_NESTED_WRITER_CHILD_OK action={action}");
    }

    /// #2786 (C1) E2c - every leg runs in a child process with a lifetime bound,
    /// because a hang cannot be asserted in-process and a poisoned `Mutex`
    /// must not leak into the rest of this suite.
    #[test]
    fn a_nested_config_writer_is_an_error_not_a_hang() {
        // One child at a time: `wait_bounded` kills and reaps the child it
        // waits on, so a timeout never leaves another child running.
        for action in ["nest_pair", "nest_json", "nest_atomic", "panic"] {
            let temp = tempfile::tempdir().expect("tempdir");
            let child = Command::new(std::env::current_exe().expect("current test exe"))
                .args([
                    "--exact",
                    NESTED_CHILD_TEST_FQN,
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(NESTED_CHILD_ACTION_ENV, action)
                .env(NESTED_CHILD_DIR_ENV, temp.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn nested writer child");
            let label = format!("nested writer child {action}");
            let (status, stdout, stderr) = wait_bounded(child, Duration::from_secs(30), &label);
            let report = format!(
                "status={status:?}
stdout:
{stdout}
stderr:
{stderr}"
            );
            assert!(status.success(), "{label} failed: {report}");
            assert!(
                stdout.contains("test result: ok. 1 passed; 0 failed"),
                "{label} did not run exactly one passing test: {report}"
            );
            assert!(
                stdout.contains(&format!("AC_2786_NESTED_WRITER_CHILD_OK action={action}")),
                "{label} is missing its marker: {report}"
            );
        }
    }
}

#[cfg(test)]
mod pair_activity_tests {
    use super::*;
    use serde_json::json;

    const T1: &str = "2026-10-08T01:00:00Z";
    const T2: &str = "2026-10-08T02:00:00Z";
    const T3: &str = "2026-10-08T03:00:00Z";

    struct Fixture {
        root: tempfile::TempDir,
    }

    impl Fixture {
        fn new(absent_state: bool) -> Self {
            let fixture = Self {
                root: tempfile::tempdir().unwrap(),
            };
            fixture.write(
                &fixture.d(),
                json!({"decision":0,"cleanup":0,
                "tooling":{"lastAgentMessageAt":T1},"opaque":null}),
            );
            if !absent_state {
                fixture.write(&fixture.s(), json!({"cleanup":0,
                    "tooling":{"lastAgentMessageAt":T1,"codingAgents":{"legacy":{"opaque":true}}},"opaque":[1,2]}));
            }
            fixture
        }
        fn d(&self) -> PathBuf {
            self.root.path().join("config.json")
        }
        fn s(&self) -> PathBuf {
            self.root.path().join("config.state.no-git.json")
        }
        fn r(&self) -> PathBuf {
            self.root
                .path()
                .join("config-identity-reservation.state.no-git.json")
        }
        fn write(&self, path: &Path, value: Value) {
            std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        }
        fn tuple(&self) -> [PhysicalState; 2] {
            read_pair_tuple(&self.d(), &self.s()).unwrap()
        }
        fn packet(&self) -> PairReservation {
            read_pair_reservation(&self.r()).unwrap().unwrap().0
        }
        fn request(&self, owner: &str) -> PairReservationRequest {
            PairReservationRequest {
                operation_id: "operation-1".into(),
                source_physical_key: "source-key".into(),
                owner_instance_id: owner.into(),
                mappings_digest: "a".repeat(64),
                before_protected: protected_config_pair_revision(&self.tuple()).unwrap(),
            }
        }
        fn reserve(
            &self,
            request: &PairReservationRequest,
        ) -> Result<(PairReservation, PairReservationRole), PreparedPairError> {
            let cleanup = |d: &mut Map<String, Value>, s: &mut Map<String, Value>| {
                d.remove("cleanup");
                s.insert("cleanup".into(), json!(1));
                let old = d
                    .get_mut("tooling")
                    .and_then(Value::as_object_mut)
                    .unwrap()
                    .remove("lastAgentMessageAt");
                let tooling = s
                    .entry("tooling")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .unwrap();
                if let Some(value) = old {
                    tooling.entry("lastAgentMessageAt").or_insert(value);
                }
                Ok(())
            };
            prepare_and_reserve_config_pair(
                &self.d(),
                &self.s(),
                &self.r(),
                request,
                Some(&cleanup),
                |d, s| {
                    d.insert("decision".into(), json!(1));
                    s.insert("caller".into(), json!(1));
                    Ok(())
                },
            )
        }
        fn activity(&self, input: &str) -> Result<bool, String> {
            let landed = Cell::new(false);
            update_config_pair_guarded(
                &self.d(),
                &self.s(),
                &self.r(),
                &[],
                None,
                Some(&|_, _| panic!("pending activity cannot cleanup")),
                &|_| panic!("no ordinary stages"),
                |_, _| panic!("pending activity uses the exact leaf primitive"),
                &|_| panic!("pending activity cannot stamp"),
                Some((input, &landed)),
            )?;
            Ok(landed.get())
        }
        fn interrupted_activity(&self, input: &str, stop: &str) -> Result<bool, PreparedPairError> {
            let _writer = ConfigWriterActive::enter("pair_activity_fixture").unwrap();
            let _guard = lock_local_config_writes();
            let _d = acquire_config_file_write_lock(&self.d(), CONFIG_LOCK_TIMEOUT).unwrap();
            let _s = acquire_config_file_write_lock(&self.s(), CONFIG_LOCK_TIMEOUT).unwrap();
            let _r = acquire_config_file_write_lock(&self.r(), CONFIG_LOCK_TIMEOUT).unwrap();
            let (mut packet, mut physical) = read_pair_reservation(&self.r())?.unwrap();
            land_pair_activity(
                &self.d(),
                &self.s(),
                &self.r(),
                &mut packet,
                &mut physical,
                input,
                &|stage| {
                    if stage == stop {
                        Err(PreparedPairError::Io("lost activity response".into()))
                    } else {
                        Ok(())
                    }
                },
            )
        }
        fn ledger(&self) -> Value {
            let packet = self.packet();
            let hashes = packet.protected_revision_digests().unwrap();
            let target = json!({"targetId":packet.physical_target_identity,
                "beforeProtectedRevision":{"kind":"bytes","sha256":hashes[0]},
                "afterProtectedRevision":{"kind":"bytes","sha256":hashes[1]},
                "planDigest":packet.plan_digest,"ownerInstanceId":"C1"});
            let revision = json!({"kind":"bytes","sha256":"b".repeat(64)});
            json!({"physicalSourceKey":"source-key","operations":{"operation-1":{
                "operationId":"operation-1","phase":"complete","afterSourceRevision":revision,
                "participantSet":{"C1":{},"C2":{}},
                "commitments":{"C1":{"participantId":"C1","targets":[target]},
                    "C2":{"participantId":"C2","targets":[target]}},
                "acks":{"C1":revision,"C2":revision},
                "receipt":{"operationId":"operation-1","sourceRevision":revision,"participantIds":["C1","C2"]}}}})
        }
    }

    #[test]
    fn pair_activity_atomic_packet_floor_and_locks_before_presence() {
        let f = Fixture::new(false);
        let request = f.request("C1");
        let before = f.tuple();
        let (packet, role) =
            prepare_and_reserve_config_pair(&f.d(), &f.s(), &f.r(), &request, None, |d, _| {
                assert!(!f.r().exists(), "no reservation before complete plan");
                for path in [f.d(), f.s(), f.r()] {
                    assert!(acquire_config_file_write_lock(&path, Duration::ZERO).is_err());
                }
                assert!(write_file_atomic(&f.d(), b"{}")
                    .unwrap_err()
                    .contains("Nested config write"));
                d.insert("decision".into(), json!(1));
                Ok(())
            })
            .unwrap();
        assert_eq!(role, PairReservationRole::Owner);
        assert_eq!(f.tuple(), before);
        assert_eq!(f.packet(), packet);
        assert_eq!(packet.timestamp_floor.as_deref(), Some(T1));
        assert_eq!(packet.plan_images.stages.len(), 5);
        assert!(packet.activity_intent.is_none());
    }

    #[test]
    fn pair_activity_packet_publication_failure_has_no_marker_or_target_write() {
        let f = Fixture::new(false);
        let before = f.tuple();
        std::fs::create_dir(temp_config_path(&f.r())).unwrap();
        assert!(matches!(
            f.reserve(&f.request("C1")),
            Err(PreparedPairError::Io(_))
        ));
        assert!(!f.r().exists());
        assert_eq!(f.tuple(), before);
        for path in [f.d(), f.s(), f.r()] {
            drop(acquire_config_file_write_lock(&path, Duration::ZERO).unwrap());
        }
    }

    #[test]
    fn pair_activity_first_owner_matching_observer_and_no_takeover() {
        let f = Fixture::new(false);
        let request = f.request("C1");
        let (owner, _) = f.reserve(&request).unwrap();
        let bytes = std::fs::read(f.r()).unwrap();
        let mut observer = request.clone();
        observer.owner_instance_id = "C2".into();
        assert_eq!(
            f.reserve(&observer).unwrap(),
            (owner.clone(), PairReservationRole::Observer)
        );
        assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
        assert_eq!(
            verify_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", &owner.plan_digest),
            Err(PreparedPairError::Pending)
        );
        assert_eq!(
            execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", "C2"),
            Err(PreparedPairError::Conflict)
        );
        let mut foreign = observer.clone();
        foreign.operation_id = "operation-2".into();
        assert_eq!(f.reserve(&foreign), Err(PreparedPairError::Conflict));
        let divergent =
            prepare_and_reserve_config_pair(&f.d(), &f.s(), &f.r(), &observer, None, |d, _| {
                d.insert("decision".into(), json!(9));
                Ok(())
            });
        assert_eq!(divergent, Err(PreparedPairError::Conflict));
        assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", "C1").unwrap();
        assert_eq!(
            verify_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", &owner.plan_digest)
                .unwrap(),
            f.tuple()
        );
    }

    #[test]
    fn pair_activity_generic_noop_zero_write_and_history_selection_unknown_rejected() {
        let f = Fixture::new(false);
        f.reserve(&f.request("C1")).unwrap();
        let before = f.tuple();
        let packet = std::fs::read(f.r()).unwrap();
        update_config_pair_guarded(
            &f.d(),
            &f.s(),
            &f.r(),
            &[],
            None,
            Some(&|_, _| panic!("no cleanup")),
            &|_| panic!("no publish"),
            |_, _| Ok(()),
            &|_| panic!("no stamp"),
            None,
        )
        .unwrap();
        for key in ["codingAgents", "configurationRef", "unknown"] {
            let error = update_config_pair_guarded(
                &f.d(),
                &f.s(),
                &f.r(),
                &[],
                None,
                None,
                &|_| {},
                |d, s| {
                    if key == "configurationRef" {
                        d.insert(key.into(), json!({"changed":true}));
                    } else {
                        s.insert(key.into(), json!({"changed":true}));
                    }
                    Ok(())
                },
                &|_| panic!("no stamp"),
                None,
            )
            .unwrap_err();
            assert!(error.contains("targetTransitionPending"), "{error}");
        }
        assert_eq!(f.tuple(), before);
        assert_eq!(std::fs::read(f.r()).unwrap(), packet);
        assert_eq!(
            execute_unreserved_config_pair(&f.d(), &f.s(), &f.r(), &f.packet().plan_images),
            Err(PreparedPairError::Pending)
        );
    }

    #[test]
    fn pair_activity_two_edges_and_older_edge_keep_digest_and_maximum() {
        let f = Fixture::new(false);
        let (original, _) = f.reserve(&f.request("C1")).unwrap();
        let d = read_config_pair_physical(&f.d()).unwrap();
        assert_eq!(f.activity(T2), Ok(true));
        assert_eq!(f.activity(T3), Ok(true));
        assert_eq!(f.activity(T1), Ok(false));
        let packet = f.packet();
        assert_eq!(packet.timestamp_floor.as_deref(), Some(T3));
        assert_eq!(packet.plan_digest, original.plan_digest);
        assert_eq!(
            packet.protected_revision_digests().unwrap(),
            original.protected_revision_digests().unwrap()
        );
        assert_eq!(read_config_pair_physical(&f.d()).unwrap(), d);
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", "C1").unwrap();
        assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
    }

    #[test]
    fn pair_activity_every_t0_t4_cut_recovers_with_monotonic_stamp() {
        for absent in [false, true] {
            for cut in 0..=4 {
                let f = Fixture::new(absent);
                f.reserve(&f.request("C1")).unwrap();
                if cut != 0 {
                    let result = execute_reserved_config_pair_with_stage(
                        &f.d(),
                        &f.s(),
                        &f.r(),
                        "operation-1",
                        "C1",
                        &|stage| {
                            if stage == cut {
                                Err(PreparedPairError::Io("lost progress".into()))
                            } else {
                                Ok(())
                            }
                        },
                    );
                    assert_eq!(result, Err(PreparedPairError::Io("lost progress".into())));
                }
                assert_eq!(f.activity(T2), Ok(true));
                assert_eq!(f.activity(T3), Ok(true));
                assert_eq!(f.activity(T1), Ok(false));
                // Re-read durable packet: no in-memory plan or progress authority.
                let packet = f.packet();
                assert_eq!(packet.timestamp_floor.as_deref(), Some(T3));
                execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", "C1").unwrap();
                assert_eq!(f.packet().recognize(&f.tuple()).unwrap(), 4);
                assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
                assert_eq!(f.tuple()[0].map().unwrap()["decision"], json!(1));
                assert_eq!(f.tuple()[0].map().unwrap()["opaque"], Value::Null);
                if !absent {
                    assert_eq!(f.tuple()[1].map().unwrap()["opaque"], json!([1, 2]));
                }
            }
        }
    }

    #[test]
    fn pair_activity_intent_state_floor_crashes_never_report_false_success() {
        for stop in [
            "activity_intent_durable",
            "activity_state_durable",
            "activity_floor_durable",
        ] {
            let f = Fixture::new(false);
            f.reserve(&f.request("C1")).unwrap();
            assert_eq!(
                f.interrupted_activity(T3, stop),
                Err(PreparedPairError::Io("lost activity response".into()))
            );
            let packet = f.packet();
            if stop == "activity_intent_durable" {
                assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T1));
                assert!(packet.activity_intent.is_some());
                // A generic no-op cannot consolidate a pending activity.
                let bytes = std::fs::read(f.r()).unwrap();
                let tuple = f.tuple();
                update_config_pair_guarded(
                    &f.d(),
                    &f.s(),
                    &f.r(),
                    &[],
                    None,
                    None,
                    &|_| panic!("no publish"),
                    |_, _| Ok(()),
                    &|_| panic!("no stamp"),
                    None,
                )
                .unwrap();
                assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
                assert_eq!(f.tuple(), tuple);
            } else {
                assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
            }
            assert_eq!(f.activity(T2), Ok(false));
            assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
            assert_eq!(f.packet().timestamp_floor.as_deref(), Some(T3));
            assert!(f.packet().activity_intent.is_none());
        }
    }

    #[test]
    fn pair_activity_absent_overlay_structural_presence_and_legacy_floor() {
        let f = Fixture::new(true);
        f.reserve(&f.request("C1")).unwrap();
        assert_eq!(f.activity(T2), Ok(true));
        let state = f.tuple()[1].map().unwrap();
        assert_eq!(
            state,
            json!({"tooling":{"lastAgentMessageAt":T2}})
                .as_object()
                .unwrap()
                .clone()
        );
        assert!(protected_matches(&PhysicalState::Absent, &f.tuple()[1], true).unwrap());
        let empty = PhysicalState::from_bytes(b"{}".to_vec());
        assert_ne!(
            protected_revision(&f.tuple()[1], true).unwrap(),
            protected_revision(&empty, true).unwrap()
        );
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", "C1").unwrap();
        assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T2));
        assert_eq!(f.tuple()[0].map().unwrap()["opaque"], Value::Null);
    }

    #[test]
    fn pair_activity_external_unknown_deleted_malformed_or_lower_timestamp_conflicts() {
        for kind in [
            "unknown",
            "deleted",
            "malformed",
            "lower",
            "tooling",
            "stateGone",
        ] {
            let f = Fixture::new(false);
            f.reserve(&f.request("C1")).unwrap();
            f.activity(T3).unwrap();
            let mut map = f.tuple()[1].map().unwrap();
            match kind {
                "unknown" => {
                    map.insert("outside".into(), json!(7));
                }
                "deleted" => {
                    map.get_mut("tooling")
                        .unwrap()
                        .as_object_mut()
                        .unwrap()
                        .remove("lastAgentMessageAt");
                }
                "malformed" => {
                    map.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!("bad");
                }
                "lower" => {
                    map.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!(T1);
                }
                "tooling" => {
                    map.insert("tooling".into(), Value::Null);
                }
                _ => {}
            }
            if kind == "stateGone" {
                std::fs::remove_file(f.s()).unwrap();
            } else {
                f.write(&f.s(), Value::Object(map));
            }
            let tuple = f.tuple();
            let packet = std::fs::read(f.r()).unwrap();
            assert!(f.activity(T2).is_err());
            assert_eq!(
                execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", "C1"),
                Err(PreparedPairError::Conflict)
            );
            assert_eq!(f.tuple(), tuple);
            assert_eq!(std::fs::read(f.r()).unwrap(), packet);
        }
    }

    #[test]
    fn pair_activity_corrupt_packet_wrong_target_and_before_cas_preserve_evidence() {
        let f = Fixture::new(false);
        let mut wrong = f.request("C1");
        wrong.before_protected[0] = ProtectedRevision::Absent;
        assert_eq!(f.reserve(&wrong), Err(PreparedPairError::Conflict));
        assert!(!f.r().exists());
        f.reserve(&f.request("C1")).unwrap();
        let other = Fixture::new(false);
        std::fs::write(other.r(), std::fs::read(f.r()).unwrap()).unwrap();
        assert!(other.activity(T2).is_err());
        let tuple = f.tuple();
        f.write(&f.r(), json!({"version":1,"operationId":"operation-1"}));
        let bytes = std::fs::read(f.r()).unwrap();
        assert!(f.activity(T2).is_err());
        assert_eq!(f.tuple(), tuple);
        assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
    }

    #[test]
    fn pair_activity_initial_null_parse_error_incompatible_tooling_zero_write() {
        for bytes in [
            b"null".as_slice(),
            b"[]",
            b"not json",
            br#"{"tooling":null}"#,
            br#"{"tooling":{"lastAgentMessageAt":null}}"#,
            br#"{"tooling":{"lastAgentMessageAt":"bad"}}"#,
        ] {
            let f = Fixture::new(false);
            let request = f.request("C1");
            std::fs::write(f.s(), bytes).unwrap();
            let d = std::fs::read(f.d()).unwrap();
            assert!(f.reserve(&request).is_err());
            assert!(!f.r().exists());
            assert_eq!(std::fs::read(f.s()).unwrap(), bytes);
            assert_eq!(std::fs::read(f.d()).unwrap(), d);
        }
    }

    #[test]
    fn pair_activity_collapsed_absent_stages_choose_greatest_and_keep_overlay() {
        let f = Fixture {
            root: tempfile::tempdir().unwrap(),
        };
        let request = f.request("C1");
        let (packet, _) =
            prepare_and_reserve_config_pair(&f.d(), &f.s(), &f.r(), &request, None, |_, _| Ok(()))
                .unwrap();
        assert_eq!(packet.recognize(&f.tuple()).unwrap(), 4);
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", "C1").unwrap();
        assert!(!f.d().exists());
        assert!(!f.s().exists());
        assert_eq!(f.activity(T2), Ok(true));
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", "C1").unwrap();
        assert!(!f.d().exists());
        assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T2));
        assert_eq!(f.packet().recognize(&f.tuple()).unwrap(), 4);
    }

    #[test]
    fn pair_activity_intent_unknown_timestamp_and_exact_generic_delta() {
        let f = Fixture::new(false);
        f.reserve(&f.request("C1")).unwrap();
        let tuple = f.tuple();
        update_config_pair_guarded(
            &f.d(),
            &f.s(),
            &f.r(),
            &[],
            None,
            None,
            &|_| panic!("no cleanup stages"),
            |_, s| {
                s.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!(T2);
                Ok(())
            },
            &|_| panic!("no split stamp"),
            None,
        )
        .unwrap();
        assert_eq!(f.tuple()[0], tuple[0]);
        assert_eq!(f.packet().timestamp_floor.as_deref(), Some(T2));
        let before = f.tuple();
        let packet = std::fs::read(f.r()).unwrap();
        let error = update_config_pair_guarded(
            &f.d(),
            &f.s(),
            &f.r(),
            &[],
            None,
            None,
            &|_| {},
            |_, s| {
                s.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!(T1);
                Ok(())
            },
            &|_| panic!("no stamp"),
            None,
        )
        .unwrap_err();
        assert!(error.contains("targetTransitionPending"));
        assert_eq!(f.tuple(), before);
        assert_eq!(std::fs::read(f.r()).unwrap(), packet);
        assert!(f
            .interrupted_activity(T3, "activity_intent_durable")
            .is_err());
        let mut state = f.tuple()[1].map().unwrap();
        state.get_mut("tooling").unwrap()["lastAgentMessageAt"] = json!("2026-10-08T02:30:00Z");
        f.write(&f.s(), Value::Object(state));
        let tuple = f.tuple();
        let packet = std::fs::read(f.r()).unwrap();
        assert!(f.activity(T3).is_err());
        assert_eq!(f.tuple(), tuple);
        assert_eq!(std::fs::read(f.r()).unwrap(), packet);
    }

    #[test]
    fn pair_activity_preflight_precedes_state_sidecars_and_holds_decisions_lease() {
        let f = Fixture::new(false);
        let called = Cell::new(false);
        let preflight = || {
            assert!(!lock_sidecar_path(&f.s()).exists());
            assert!(!lock_sidecar_path(&f.r()).exists());
            assert!(acquire_config_file_write_lock(&f.d(), Duration::from_millis(20)).is_err());
            called.set(true);
            Err("ignore preflight refused".to_string())
        };
        let before = f.tuple();
        let error = update_config_pair_guarded(
            &f.d(),
            &f.s(),
            &f.r(),
            &[],
            Some(&preflight),
            None,
            &|_| panic!("no publish"),
            |_, _| panic!("no mutation"),
            &|_| Ok(()),
            None,
        )
        .unwrap_err();
        assert_eq!(error, "ignore preflight refused");
        assert!(called.get());
        assert_eq!(f.tuple(), before);
        assert!(!f.r().exists());
        assert!(!lock_sidecar_path(&f.s()).exists());
        assert!(!lock_sidecar_path(&f.r()).exists());
        std::fs::write(f.r(), b"{invalid").unwrap();
        called.set(false);
        assert!(update_config_pair_guarded(
            &f.d(),
            &f.s(),
            &f.r(),
            &[],
            Some(&preflight),
            None,
            &|_| panic!("no publish"),
            |_, _| panic!("no mutation"),
            &|_| Ok(()),
            None
        )
        .is_err());
        assert!(!called.get());
        assert_eq!(std::fs::read(f.r()).unwrap(), b"{invalid");
        assert!(!lock_sidecar_path(&f.s()).exists());
        assert!(!lock_sidecar_path(&f.r()).exists());
    }

    #[test]
    fn pair_activity_release_requires_shared_complete_current_source_owner_and_final_target() {
        let f = Fixture::new(false);
        f.reserve(&f.request("C1")).unwrap();
        let target = f.packet().physical_target_identity;
        let ledger = f.ledger();
        let revision = &ledger["operations"]["operation-1"]["afterSourceRevision"];
        let proof =
            PairCompleteProof::from_shared_complete(&ledger, "operation-1", &target, revision)
                .unwrap();
        assert_eq!(
            release_reserved_config_pair(&f.d(), &f.s(), &f.r(), "C1", &proof),
            Err(PreparedPairError::Conflict)
        );
        assert!(PairCompleteProof::from_shared_complete(
            &ledger,
            "operation-1",
            &target,
            &json!({"kind":"absent"})
        )
        .is_err());
        for key in ["phase", "acks", "receipt"] {
            let mut invalid = ledger.clone();
            invalid["operations"]["operation-1"][key] = Value::Null;
            assert!(PairCompleteProof::from_shared_complete(
                &invalid,
                "operation-1",
                &target,
                revision
            )
            .is_err());
        }
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1", "C1").unwrap();
        f.activity(T3).unwrap();
        assert_eq!(
            release_reserved_config_pair(&f.d(), &f.s(), &f.r(), "C2", &proof),
            Err(PreparedPairError::Conflict)
        );
        let before = f.tuple();
        assert_eq!(
            release_reserved_config_pair(&f.d(), &f.s(), &f.r(), "C1", &proof).unwrap(),
            before
        );
        assert!(!f.r().exists());
        assert!(config_lock_path(f.root.path(), &f.r()).unwrap().exists());
        assert_eq!(
            release_reserved_config_pair(&f.d(), &f.s(), &f.r(), "C1", &proof).unwrap(),
            before
        );
        assert_eq!(timestamp_value(&before[1]).unwrap().as_deref(), Some(T3));
    }
}
