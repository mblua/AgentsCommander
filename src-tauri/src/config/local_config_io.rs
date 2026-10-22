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

    let guard = PairTargetGuard::new(&|| Ok(()));
    let (tmp_path, mut file) =
        acquire_fresh_config_stage(path, &guard).map_err(ordinary_staging_error)?;

    let write_result = (|| -> Result<(), String> {
        file.write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write temp config {}: {}", tmp_path.display(), e))?;
        file.flush()
            .map_err(|e| format!("Failed to flush temp config {}: {}", tmp_path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync temp config {}: {}", tmp_path.display(), e))
    })();
    drop(file);
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

pub(super) fn write_file_atomic_with_publish<P>(
    path: &Path,
    bytes: &[u8],
    publish: P,
) -> Result<(), String>
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
    let guard = PairTargetGuard::new(&|| Ok(()));
    let (tmp_path, mut file) =
        acquire_fresh_config_stage(path, &guard).map_err(ordinary_staging_error)?;

    let write_result = (|| -> Result<(), String> {
        file.write_all(bytes)
            .map_err(|e| format!("Failed to write temp config {}: {}", tmp_path.display(), e))?;
        file.flush()
            .map_err(|e| format!("Failed to flush temp config {}: {}", tmp_path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync temp config {}: {}", tmp_path.display(), e))
    })();
    drop(file);
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

pub(super) type ConfigPublisher<'a> = &'a dyn Fn(&Path, &Path) -> Result<(), String>;

pub(crate) type ConfigPairFinish<'a> = &'a dyn Fn(&mut Map<String, Value>) -> Result<(), String>;

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
        &publish_temp_config,
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
    finish: ConfigPairFinish<'_>,
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
        &publish_temp_config,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_config_pair_inner<F>(
    decisions: &Path,
    state: &Path,
    reservation: Option<&Path>,
    state_keys: &[&str],
    cleanup: Option<ConfigPairCleanup<'_>>,
    on_stage: &dyn Fn(&str),
    mutate: F,
    finish: ConfigPairFinish<'_>,
    activity: Option<(&str, &Cell<bool>)>,
    nonpending_preflight: Option<&dyn Fn() -> Result<(), String>>,
    publish: ConfigPublisher<'_>,
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
            publish,
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
        publish,
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
/// the same PAIR locks; it cannot bypass the original packet/protected-activity protocol.
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
    publish_prepared_pair_image_guarded(path, before, after, &PairTargetGuard::new(&|| Ok(())))
}

#[cfg_attr(not(test), allow(dead_code))]
fn publish_prepared_pair_image_guarded(
    path: &Path,
    before: &PhysicalState,
    after: &PhysicalState,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    publish_prepared_pair_image_with_report(
        path,
        before,
        after,
        guard,
        &log_unverified_cleanup_refusal,
    )
}

fn publish_prepared_pair_image_with_report(
    path: &Path,
    before: &PhysicalState,
    after: &PhysicalState,
    guard: &PairTargetGuard<'_>,
    report_cleanup_refusal: &dyn Fn(&PreparedPairError, &PreparedPairError),
) -> Result<(), PreparedPairError> {
    publish_prepared_pair_image_with_io(
        path,
        before,
        after,
        guard,
        report_cleanup_refusal,
        &mut || next_staging_sequence(&STAGING_SEQUENCE),
        &|file, bytes| {
            file.write_all(bytes)
                .map_err(|e| PreparedPairError::Io(e.to_string()))
        },
    )
}

type PreparedPairStageWrite<'a> =
    dyn Fn(&mut std::fs::File, &[u8]) -> Result<(), PreparedPairError> + 'a;

fn publish_prepared_pair_image_with_io(
    path: &Path,
    before: &PhysicalState,
    after: &PhysicalState,
    guard: &PairTargetGuard<'_>,
    report_cleanup_refusal: &dyn Fn(&PreparedPairError, &PreparedPairError),
    next: &mut dyn FnMut() -> Result<u64, PreparedPairError>,
    write: &PreparedPairStageWrite<'_>,
) -> Result<(), PreparedPairError> {
    publish_prepared_pair_image_with_source_parent(
        path,
        before,
        after,
        guard,
        report_cleanup_refusal,
        (next, write),
        None,
    )
}

struct SourceNativePublication<'a> {
    parent: &'a crate::path_identity::RetainedDirectory,
    moved: std::cell::Cell<bool>,
    after_owned: std::cell::RefCell<Option<crate::path_identity::VerifiedPathIdentity>>,
}

fn publish_prepared_pair_image_with_source_parent(
    path: &Path,
    before: &PhysicalState,
    after: &PhysicalState,
    guard: &PairTargetGuard<'_>,
    report_cleanup_refusal: &dyn Fn(&PreparedPairError, &PreparedPairError),
    stage_io: (
        &mut dyn FnMut() -> Result<u64, PreparedPairError>,
        &PreparedPairStageWrite<'_>,
    ),
    source: Option<&SourceNativePublication<'_>>,
) -> Result<(), PreparedPairError> {
    let (next, write) = stage_io;
    let PhysicalState::Bytes { bytes, .. } = after else {
        return Err(PreparedPairError::InvalidPlan("PAIR never deletes a side"));
    };
    guard.check()?;
    if let Some(source) = source {
        guard.check()?;
        validate_source_publication_children(source.parent, path, path)?;
    }
    // Cleanup ownership begins only after exclusive acquisition succeeds.
    let (tmp, mut file) = acquire_fresh_config_stage_with_sequence(path, guard, next)?;
    let mut stage_identity = None;
    let result = (|| {
        if let Some(source) = source {
            let parent = source.parent;
            guard.check()?;
            validate_source_publication_children(parent, &tmp, path)?;
            stage_identity = Some(
                parent
                    .verify_opened_regular_file(&tmp, &file, true)
                    .map_err(PreparedPairError::Io)?,
            );
        }
        guard.check()?;
        write(&mut file, bytes)?;
        guard.check()?;
        file.flush()
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        guard.check()?;
        file.sync_all()
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        Ok(())
    })();
    drop(file);
    let result = result.and_then(|_| {
        guard.check()?;
        if read_config_pair_physical(path)? != *before {
            return Err(PreparedPairError::Conflict);
        }
        if *before == PhysicalState::Absent {
            if let Some(source) = source {
                let parent = source.parent;
                guard.check()?;
                validate_source_publication_children(parent, &tmp, path)?;
                let staged = parent
                    .verify_regular_file(&tmp)
                    .map_err(PreparedPairError::Io)?;
                if stage_identity
                    .as_ref()
                    .is_none_or(|owned| !crate::path_identity::same_object(owned, &staged))
                {
                    return Err(PreparedPairError::Conflict);
                }
                guard.check()?;
                // Windows remains path-based after verification; post-move checks
                // detect uncertainty rather than promising handle-based exclusion.
                let publication = parent.publish_new_file_atomic(&tmp, path);
                source.moved.set(publication.is_ok());
                guard.check()?;
                validate_source_publication_children(parent, &tmp, path)?;
                if let Err(error) = publication {
                    // Opaque native errors never identify AlreadyExists or ownership.
                    return match read_config_pair_physical(path) {
                        Ok(PhysicalState::Bytes { .. })
                            if parent.verify_regular_file(path).is_ok_and(|foreign| {
                                !crate::path_identity::same_object(&staged, &foreign)
                            }) =>
                        {
                            Err(PreparedPairError::Conflict)
                        }
                        _ => Err(PreparedPairError::Io(error)),
                    };
                }
                let landed = parent
                    .verify_regular_file(path)
                    .map_err(PreparedPairError::Io)?;
                if !crate::path_identity::same_object(&staged, &landed)
                    || read_config_pair_physical(path)? != *after
                {
                    return Err(PreparedPairError::Conflict);
                }
                *source.after_owned.borrow_mut() = Some(landed);
            } else {
                // Atomic create-if-absent; rename would clobber a racing Unix creator.
                guard.check()?;
                std::fs::hard_link(&tmp, path).map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        PreparedPairError::Conflict
                    } else {
                        PreparedPairError::Io(e.to_string())
                    }
                })?;
                guard.check()?;
                std::fs::remove_file(&tmp).map_err(|e| PreparedPairError::Io(e.to_string()))?;
            }
        } else {
            publish_temp_config_guarded(&tmp, path, guard)?;
        }
        guard.check()?;
        #[cfg(not(windows))]
        std::fs::File::open(
            path.parent()
                .ok_or(PreparedPairError::InvalidPlan("PAIR target has no parent"))?,
        )
        .and_then(|dir| dir.sync_all())
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        Ok(())
    });
    if let Err(primary) = &result {
        if !guard.failed.get() {
            if let Err(refusal) = guard.check() {
                report_cleanup_refusal(primary, &refusal);
                return Err(refusal);
            }
            if let Some(source) = source {
                let parent = source.parent;
                if !source.moved.get() {
                    validate_source_publication_children(parent, &tmp, path)?;
                    if let Some(owned) = &stage_identity {
                        // Cleanup owns only this stage, never the published target.
                        guard.check()?;
                        parent.remove_regular_file_if_same(&tmp, owned);
                    }
                }
            } else if let Err(e) = std::fs::remove_file(&tmp) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("[prepared-pair] temporary cleanup failed: {e}");
                }
            }
        }
    }
    if result.is_ok() {
        guard.check()?;
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
        if self.version != 2
            || self.protected_policy_version != 1
            || self.operation_id.is_empty()
            || self.source_physical_key.is_empty()
            || self.physical_target_identity.is_empty()
            || self.mappings_digest.is_empty()
            || self.plan_digest != pair_plan_digest(&self.plan_images, &self.mappings_digest)?
            || self.protected_images != protected_plan(&self.plan_images)?
            || self.before_tuple != self.plan_images.stages[0]
            || self.changed_sides != changed_pair_sides(&self.plan_images)
        {
            return Err(PreparedPairError::InvalidPlan("invalid reservation packet"));
        }
        merge_monotonic_activity(std::slice::from_ref(&self.timestamp_floor))?;
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
    publish_pair_reservation_guarded(path, physical, packet, &PairTargetGuard::new(&|| Ok(())))
}

#[cfg_attr(not(test), allow(dead_code))]
fn publish_pair_reservation_guarded(
    path: &Path,
    physical: &mut PhysicalState,
    packet: &PairReservation,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    packet.validate()?;
    let mut bytes =
        serde_json::to_vec_pretty(packet).map_err(|e| PreparedPairError::Io(e.to_string()))?;
    bytes.push(b'\n');
    publish_coordination_bytes_guarded(path, physical, &bytes, guard)?;
    *physical = PhysicalState::from_bytes(bytes);
    guard.check()?;
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
) -> Result<PairReservation, PreparedPairError>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    prepare_and_reserve_config_pair_guarded(
        decisions,
        state,
        reservation,
        request,
        cleanup,
        mutate,
        &|| Ok(()),
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn prepare_and_reserve_config_pair_guarded<F>(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    request: &PairReservationRequest,
    cleanup: Option<ConfigPairCleanup<'_>>,
    mutate: F,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<PairReservation, PreparedPairError>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    let _writer = ConfigWriterActive::enter("prepare_and_reserve_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    guard.check()?;
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let existing = read_pair_reservation(reservation)?;
    guard.check()?;
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
    guard.check()?;
    let images = protected_plan(&plan)?;
    let candidate = PairReservation {
        version: 2,
        operation_id: request.operation_id.clone(),
        source_physical_key: request.source_physical_key.clone(),
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
        reconcile_pair_activity_guarded(
            decisions,
            state,
            reservation,
            &mut packet,
            &mut physical,
            &guard,
        )?;
        guard.check()?;
        packet.recognize(&read_pair_tuple(decisions, state)?)?;
        guard.check()?;
        return Ok(packet);
    }
    candidate.recognize(&current)?;
    let mut physical = PhysicalState::Absent;
    publish_pair_reservation_guarded(reservation, &mut physical, &candidate, &guard)?;
    guard.check()?;
    Ok(candidate)
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
    reconcile_pair_activity_guarded(
        decisions,
        state,
        reservation,
        packet,
        physical,
        &PairTargetGuard::new(&|| Ok(())),
    )
}

#[cfg_attr(not(test), allow(dead_code))]
fn reconcile_pair_activity_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    packet: &mut PairReservation,
    physical: &mut PhysicalState,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    let Some(intent) = packet.activity_intent.clone() else {
        return Ok(());
    };
    let current = read_pair_tuple(decisions, state)?;
    packet.recognize(&current)?;
    let actual = timestamp_physical(&current[1])?;
    if actual == intent.before_timestamp_physical {
        let after = image_with_timestamp(&current[1], Some(intent.after_timestamp.clone()))?;
        publish_prepared_pair_image_guarded(state, &current[1], &after, guard)?;
    } else if actual != TimestampPhysical::Value(intent.after_timestamp.clone()) {
        return Err(PreparedPairError::Conflict);
    }
    packet.timestamp_floor = Some(intent.after_timestamp);
    packet.activity_intent = None;
    guard.check()?;
    packet.recognize(&read_pair_tuple(decisions, state)?)?;
    publish_pair_reservation_guarded(reservation, physical, packet, guard)
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

/// Execute pending stages of the original operation under current target authority.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn execute_reserved_config_pair(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
) -> Result<(), PreparedPairError> {
    execute_reserved_config_pair_with_stage(
        decisions,
        state,
        reservation,
        operation_id,
        &|_| Ok(()),
    )
}

/// Verify the original target without applying stages or consolidating activity.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn verify_reserved_config_pair(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    plan_digest: &str,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    verify_reserved_config_pair_guarded(
        decisions,
        state,
        reservation,
        operation_id,
        plan_digest,
        &|| Ok(()),
    )
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn verify_reserved_config_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    plan_digest: &str,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    let _writer = ConfigWriterActive::enter("verify_reserved_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    guard.check()?;
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let (packet, _) = read_pair_reservation(reservation)?.ok_or(PreparedPairError::Conflict)?;
    validate_pair_binding(decisions, state, &packet)?;
    if packet.operation_id != operation_id || packet.plan_digest != plan_digest {
        return Err(PreparedPairError::Conflict);
    }
    guard.check()?;
    let current = read_pair_tuple(decisions, state)?;
    if packet.recognize(&current)? != 4 || packet.activity_intent.is_some() {
        return Err(PreparedPairError::Pending);
    }
    guard.check()?;
    Ok(current)
}

fn execute_reserved_config_pair_with_stage(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    on_stage: &dyn Fn(usize) -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    execute_reserved_config_pair_with_validation(
        decisions,
        state,
        reservation,
        operation_id,
        on_stage,
        &|| Ok(()),
    )
}

#[cfg_attr(not(test), allow(dead_code))]
fn execute_reserved_config_pair_with_validation(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    on_stage: &dyn Fn(usize) -> Result<(), PreparedPairError>,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    let _writer = ConfigWriterActive::enter("execute_reserved_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    guard.check()?;
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let (mut packet, mut physical) =
        read_pair_reservation(reservation)?.ok_or(PreparedPairError::Conflict)?;
    validate_pair_binding(decisions, state, &packet)?;
    if packet.operation_id != operation_id {
        return Err(PreparedPairError::Conflict);
    }
    reconcile_pair_activity_guarded(
        decisions,
        state,
        reservation,
        &mut packet,
        &mut physical,
        &guard,
    )?;
    guard.check()?;
    let start = packet.recognize(&read_pair_tuple(decisions, state)?)?;
    confirm_pair_activity_floor_guarded(state, reservation, &mut packet, &mut physical, &guard)?;
    for stage in start + 1..5 {
        guard.check()?;
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
            publish_prepared_pair_image_guarded(path, &current[side], &after, &guard)?;
            guard.check()?;
            if packet.recognize(&read_pair_tuple(decisions, state)?)? < stage {
                return Err(PreparedPairError::Conflict);
            }
            confirm_pair_activity_floor_guarded(
                state,
                reservation,
                &mut packet,
                &mut physical,
                &guard,
            )?;
            on_stage(stage)?;
        }
    }
    confirm_pair_activity_floor_guarded(state, reservation, &mut packet, &mut physical, &guard)?;
    guard.check()?;
    if packet.recognize(&read_pair_tuple(decisions, state)?)? != 4 {
        return Err(PreparedPairError::Conflict);
    }
    guard.check()?;
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn confirm_pair_activity_floor_guarded(
    state: &Path,
    reservation: &Path,
    packet: &mut PairReservation,
    physical: &mut PhysicalState,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    let floor = merge_monotonic_activity(&[
        packet.timestamp_floor.clone(),
        timestamp_value(&read_config_pair_physical(state)?)?,
    ])?;
    if floor != packet.timestamp_floor {
        packet.timestamp_floor = floor;
        publish_pair_reservation_guarded(reservation, physical, packet, guard)?;
    }
    Ok(())
}

/// IO consumes a proof assembled above PAIR from an authenticated, validated
/// local complete receipt under C. It never reads another config directory.
#[derive(Clone, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct PairCompleteProof {
    operation_id: String,
    source_physical_key: String,
    target_id: String,
    plan_digest: String,
}

#[cfg_attr(not(test), allow(dead_code))]
impl PairCompleteProof {
    pub(crate) fn from_single_editor_complete(
        projection: &Value,
        operation_id: &str,
        target_id: &str,
        current_source_revision: &Value,
    ) -> Result<Self, PreparedPairError> {
        let receipt = projection
            .get("receipt")
            .filter(|v| v.is_object())
            .ok_or(PreparedPairError::Conflict)?;
        let after = projection
            .get("afterSourceRevision")
            .filter(|v| {
                v.get("kind").and_then(Value::as_str) == Some("bytes")
                    && v.get("sha256").and_then(Value::as_str).is_some_and(|h| {
                        h.len() == 64
                            && h.bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
            })
            .ok_or(PreparedPairError::Conflict)?;
        if projection.get("schemaVersion").and_then(Value::as_u64) != Some(2)
            || projection.get("operationId").and_then(Value::as_str) != Some(operation_id)
            || projection.get("phase").and_then(Value::as_str) != Some("complete")
            || [
                "packetsPrepared",
                "sourcePublished",
                "refsVerified",
                "reloaded",
            ]
            .iter()
            .any(|key| projection.get(key).and_then(Value::as_bool) != Some(true))
            || receipt.get("operationId").and_then(Value::as_str) != Some(operation_id)
            || receipt.get("sourceRevision") != Some(after)
            || current_source_revision != after
        {
            return Err(PreparedPairError::Conflict);
        }
        let rows = projection
            .get("packets")
            .and_then(Value::as_array)
            .ok_or(PreparedPairError::Conflict)?;
        let mut matches = rows
            .iter()
            .filter(|row| row.get("targetId").and_then(Value::as_str) == Some(target_id));
        let target = matches.next().ok_or(PreparedPairError::Conflict)?;
        if matches.next().is_some() {
            return Err(PreparedPairError::Conflict);
        }
        let text = |value: &Value, key: &str| -> Result<String, PreparedPairError> {
            value
                .get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .ok_or(PreparedPairError::Conflict)
        };
        Ok(Self {
            operation_id: operation_id.into(),
            target_id: target_id.into(),
            source_physical_key: text(projection, "physicalSourceKey")?,
            plan_digest: text(target, "planDigest")?,
        })
    }
}

/// Complete receipt is necessary but insufficient: release also verifies the
/// own final protected tuple and confirmed floor before removing this packet.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn release_reserved_config_pair(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    proof: &PairCompleteProof,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    release_reserved_config_pair_guarded(decisions, state, reservation, proof, &|| Ok(()))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn release_reserved_config_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    proof: &PairCompleteProof,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    let _writer = ConfigWriterActive::enter("release_reserved_config_pair")
        .map_err(PreparedPairError::Preparation)?;
    validate_pair_paths(decisions, state, reservation)?;
    let _guard = lock_local_config_writes();
    guard.check()?;
    let _decisions = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _state = acquire_config_file_write_lock(state, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    let _reservation = acquire_config_file_write_lock(reservation, CONFIG_LOCK_TIMEOUT)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    if proof.target_id != pair_target_identity(decisions, state)? {
        return Err(PreparedPairError::Conflict);
    }
    let Some((mut packet, mut physical)) = read_pair_reservation(reservation)? else {
        let current = read_pair_tuple(decisions, state)?;
        guard.check()?;
        return Ok(current);
    };
    validate_pair_binding(decisions, state, &packet)?;
    if packet.operation_id != proof.operation_id
        || packet.source_physical_key != proof.source_physical_key
        || packet.plan_digest != proof.plan_digest
    {
        return Err(PreparedPairError::Conflict);
    }
    reconcile_pair_activity_guarded(
        decisions,
        state,
        reservation,
        &mut packet,
        &mut physical,
        &guard,
    )?;
    guard.check()?;
    let current = read_pair_tuple(decisions, state)?;
    if packet.recognize(&current)? != 4 {
        return Err(PreparedPairError::Conflict);
    }
    guard.check()?;
    std::fs::remove_file(reservation).map_err(|e| PreparedPairError::Io(e.to_string()))?;
    guard.check()?;
    #[cfg(not(windows))]
    std::fs::File::open(
        reservation
            .parent()
            .ok_or(PreparedPairError::InvalidPlan("reservation parent"))?,
    )
    .and_then(|directory| directory.sync_all())
    .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    guard.check()?;
    Ok(current)
}

#[cfg(test)]
thread_local! {
    static SOURCE_GUARDED_PUBLICATION_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
/// Counts invocations on this calling thread only. Driver fixtures are synchronous;
/// worker tests return their own per-thread observations explicitly.
pub(crate) fn source_guard_publication_calls() -> usize {
    SOURCE_GUARDED_PUBLICATION_CALLS.with(std::cell::Cell::get)
}

/// P03 forwards borrowed coordination authority through every existing P22 cut.
/// The callback only validates; it must not acquire locks or advance a journal.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn publish_coordination_bytes_with_validation(
    path: &Path,
    before: &PhysicalState,
    bytes: &[u8],
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    #[cfg(test)]
    SOURCE_GUARDED_PUBLICATION_CALLS.with(|calls| calls.set(calls.get() + 1));
    publish_coordination_bytes_guarded(path, before, bytes, &PairTargetGuard::new(validate))
}

/// Durable original-object witness; no paths or authority supplied by a client.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum PreparedSourcePublication {
    Prepared {
        #[serde(rename = "stageObject")]
        stage_object: (u64, u64),
        #[serde(rename = "parentObject")]
        parent_object: (u64, u64),
        #[serde(rename = "stageLeaf")]
        stage_leaf: String,
        #[serde(rename = "intentDigest")]
        intent_digest: String,
    },
}

impl PreparedSourcePublication {
    pub(crate) fn validate_record(&self, digest: &str) -> bool {
        let Self::Prepared {
            stage_leaf,
            intent_digest,
            ..
        } = self;
        let mut parts = Path::new(stage_leaf).components();
        intent_digest == digest
            && matches!(parts.next(), Some(std::path::Component::Normal(_)))
            && parts.next().is_none()
            && !stage_leaf.contains(['/', '\\', ':'])
    }

    fn stage_path(&self, parent: &crate::path_identity::RetainedDirectory) -> PathBuf {
        let Self::Prepared { stage_leaf, .. } = self;
        parent.identity().canonical_path.join(stage_leaf)
    }

    fn matches_parent(&self, parent: &crate::path_identity::RetainedDirectory) -> bool {
        let Self::Prepared { parent_object, .. } = self;
        let id = parent.identity().object_id;
        *parent_object == (id.volume, id.file)
    }

    fn matches_object(&self, object: &crate::path_identity::VerifiedPathIdentity) -> bool {
        let Self::Prepared { stage_object, .. } = self;
        *stage_object == (object.object_id.volume, object.object_id.file)
    }
}

/// Non-clonable borrowed capability. Drop only closes the handle: even an
/// uncertain checkpoint may be durable, so this path never unlinks staging.
pub(crate) struct PreparedSourceCapability<'a> {
    parent: &'a crate::path_identity::RetainedDirectory,
    target: &'a Path,
    bytes: &'a [u8],
    stage: PathBuf,
    file: std::fs::File,
    proof: PreparedSourcePublication,
    guard: PairTargetGuard<'a>,
}

impl PreparedSourceCapability<'_> {
    pub(crate) fn proof(&self) -> &PreparedSourcePublication {
        &self.proof
    }

    pub(crate) fn check(&self) -> Result<(), PreparedPairError> {
        let result = (|| {
            self.guard.check()?;
            validate_source_publication_children(self.parent, &self.stage, self.target)?;
            let current = self
                .parent
                .verify_opened_regular_file(&self.stage, &self.file, false)
                .map_err(PreparedPairError::Io)?;
            if !self.proof.matches_parent(self.parent)
                || !self.proof.matches_object(&current)
                || read_config_pair_physical(&self.stage)?
                    != PhysicalState::from_bytes(self.bytes.to_vec())
            {
                return Err(PreparedPairError::Conflict);
            }
            self.guard.check()
        })();
        if result.is_err() {
            self.guard.failed.set(true);
        }
        result
    }

    pub(crate) fn commit(
        &mut self,
        validate_checkpoint: &dyn Fn() -> Result<(), PreparedPairError>,
    ) -> Result<(), PreparedPairError> {
        let result = (|| {
            self.guard.check()?;
            validate_checkpoint()?;
            validate_source_publication_children(self.parent, &self.stage, self.target)?;
            self.verify_stage()?;
            self.guard.check()?;
            if read_config_pair_physical(self.target)? != PhysicalState::Absent {
                return Err(PreparedPairError::Conflict);
            }
            // The complete journal/revision and original authority are read-only
            // checks immediately before this invocation's single native attempt.
            validate_checkpoint()?;
            self.guard.check()?;
            #[cfg(test)]
            SOURCE_CHECKPOINT_NATIVE_CALLS.with(|n| n.set(n.get() + 1));
            #[cfg(test)]
            SOURCE_GUARDED_PUBLICATION_CALLS.with(|n| n.set(n.get() + 1));
            let native = self
                .parent
                .publish_new_file_atomic(&self.stage, self.target);
            self.guard.check()?;
            native.map_err(PreparedPairError::Io)?;
            verify_prepared_source_target(
                self.parent,
                self.target,
                self.bytes,
                &self.proof,
                &|| self.guard.check(),
            )?;
            #[cfg(not(windows))]
            std::fs::File::open(self.target.parent().ok_or(PreparedPairError::Conflict)?)
                .and_then(|dir| dir.sync_all())
                .map_err(|e| PreparedPairError::Io(e.to_string()))?;
            self.guard.check()
        })();
        if result.is_err() {
            self.guard.failed.set(true);
        }
        result
    }

    fn verify_stage(&mut self) -> Result<(), PreparedPairError> {
        use std::io::{Read, Seek, SeekFrom};
        let opened = self
            .parent
            .verify_opened_regular_file(&self.stage, &self.file, false)
            .map_err(PreparedPairError::Io)?;
        if !self.proof.matches_parent(self.parent) || !self.proof.matches_object(&opened) {
            return Err(PreparedPairError::Conflict);
        }
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        let mut current = Vec::new();
        self.file
            .read_to_end(&mut current)
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        if current != self.bytes {
            return Err(PreparedPairError::Conflict);
        }
        self.guard.check()?;
        self.file
            .flush()
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        self.file
            .sync_all()
            .map_err(|e| PreparedPairError::Io(e.to_string()))?;
        let current = self
            .parent
            .verify_opened_regular_file(&self.stage, &self.file, false)
            .map_err(PreparedPairError::Io)?;
        if !self.proof.matches_object(&current) {
            return Err(PreparedPairError::Conflict);
        }
        self.guard.check()
    }
}

pub(crate) fn prepare_source_publication<'a>(
    parent: &'a crate::path_identity::RetainedDirectory,
    target: &'a Path,
    bytes: &'a [u8],
    intent_digest: &str,
    validate: &'a dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<PreparedSourceCapability<'a>, PreparedPairError> {
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    validate_source_publication_children(parent, target, target)?;
    PhysicalState::from_bytes(bytes.to_vec()).map()?;
    if read_config_pair_physical(target)? != PhysicalState::Absent {
        return Err(PreparedPairError::Conflict);
    }
    let (stage, mut file) = acquire_fresh_config_stage(target, &guard)?;
    let own = parent
        .verify_opened_regular_file(&stage, &file, true)
        .map_err(PreparedPairError::Io)?;
    guard.check()?;
    file.write_all(bytes)
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    guard.check()?;
    file.flush()
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    file.sync_all()
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    let id = parent.identity().object_id;
    let proof = PreparedSourcePublication::Prepared {
        stage_object: (own.object_id.volume, own.object_id.file),
        parent_object: (id.volume, id.file),
        stage_leaf: stage
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(PreparedPairError::Conflict)?
            .into(),
        intent_digest: intent_digest.into(),
    };
    drop(file);
    let mut cap = reopen_source_publication(parent, target, bytes, &proof, validate)?;
    // Keep this attempt's original sticky guard, never a reset after refusal.
    cap.guard = guard;
    cap.verify_stage()?;
    Ok(cap)
}

pub(crate) fn reopen_source_publication<'a>(
    parent: &'a crate::path_identity::RetainedDirectory,
    target: &'a Path,
    bytes: &'a [u8],
    proof: &PreparedSourcePublication,
    validate: &'a dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<PreparedSourceCapability<'a>, PreparedPairError> {
    let guard = PairTargetGuard::new(validate);
    guard.check()?;
    if !proof.matches_parent(parent) {
        return Err(PreparedPairError::Conflict);
    }
    let stage = proof.stage_path(parent);
    validate_source_publication_children(parent, &stage, target)?;
    // Identity is checked through the retained parent around the opened handle;
    // opening alone never proves ownership or authorizes a new stage.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&stage)
        .map_err(|e| PreparedPairError::Io(e.to_string()))?;
    let mut cap = PreparedSourceCapability {
        parent,
        target,
        bytes,
        stage,
        file,
        proof: proof.clone(),
        guard,
    };
    cap.verify_stage()?;
    Ok(cap)
}

pub(crate) fn verify_prepared_source_target(
    parent: &crate::path_identity::RetainedDirectory,
    target: &Path,
    bytes: &[u8],
    proof: &PreparedSourcePublication,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    validate()?;
    validate_source_publication_children(parent, target, target)?;
    let before = parent
        .verify_regular_file(target)
        .map_err(PreparedPairError::Io)?;
    if !proof.matches_parent(parent)
        || !proof.matches_object(&before)
        || read_config_pair_physical(target)? != PhysicalState::from_bytes(bytes.to_vec())
    {
        return Err(PreparedPairError::Conflict);
    }
    let after = parent
        .verify_regular_file(target)
        .map_err(PreparedPairError::Io)?;
    if !crate::path_identity::same_object(&before, &after) {
        return Err(PreparedPairError::Conflict);
    }
    validate()
}

#[cfg(test)]
thread_local! {
    static SOURCE_CHECKPOINT_NATIVE_CALLS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn source_checkpoint_native_calls() -> usize {
    SOURCE_CHECKPOINT_NATIVE_CALLS.with(Cell::get)
}

/// Internal evidence distinguishes verified own publication from identical foreign bytes.
pub(crate) struct SourceCoordinationPublication {
    pub(crate) result: Result<(), PreparedPairError>,
    #[cfg(test)]
    pub(crate) published_identity: Option<crate::path_identity::VerifiedPathIdentity>,
    #[cfg(test)]
    pub(crate) guard_rejected: bool,
}

/// Only the new source/ledger/journal path borrows this native capability.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn publish_source_coordination_bytes_with_validation(
    path: &Path,
    before: &PhysicalState,
    bytes: &[u8],
    parent: &crate::path_identity::RetainedDirectory,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> SourceCoordinationPublication {
    if *before != PhysicalState::Absent {
        return SourceCoordinationPublication {
            result: publish_coordination_bytes_with_validation(path, before, bytes, validate),
            #[cfg(test)]
            published_identity: None,
            #[cfg(test)]
            guard_rejected: false,
        };
    }
    #[cfg(test)]
    SOURCE_GUARDED_PUBLICATION_CALLS.with(|calls| calls.set(calls.get() + 1));
    let guard = PairTargetGuard::new(validate);
    let source = SourceNativePublication {
        parent,
        moved: std::cell::Cell::new(false),
        after_owned: std::cell::RefCell::new(None),
    };
    let result = (|| {
        guard.check()?;
        let after = PhysicalState::from_bytes(bytes.to_vec());
        after.map()?;
        if read_config_pair_physical(path)? != *before {
            return Err(PreparedPairError::Conflict);
        }
        publish_prepared_pair_image_with_source_parent(
            path,
            before,
            &after,
            &guard,
            &log_unverified_cleanup_refusal,
            (
                &mut || next_staging_sequence(&STAGING_SEQUENCE),
                &|file, bytes| {
                    file.write_all(bytes)
                        .map_err(|e| PreparedPairError::Io(e.to_string()))
                },
            ),
            Some(&source),
        )
    })();
    SourceCoordinationPublication {
        result,
        #[cfg(test)]
        published_identity: source.after_owned.into_inner(),
        #[cfg(test)]
        guard_rejected: guard.failed.get(),
    }
}

fn validate_source_publication_children(
    parent: &crate::path_identity::RetainedDirectory,
    stage: &Path,
    target: &Path,
) -> Result<(), PreparedPairError> {
    parent.verify_current().map_err(PreparedPairError::Io)?;
    for child in [stage, target] {
        let directory = child
            .parent()
            .ok_or(PreparedPairError::InvalidPlan("source target parent"))?;
        let identity =
            crate::path_identity::verify_directory(directory).map_err(PreparedPairError::Io)?;
        if child.file_name().is_none()
            || !crate::path_identity::same_object(parent.identity(), &identity)
            || !crate::path_identity::paths_equivalent(directory, &parent.identity().canonical_path)
        {
            return Err(PreparedPairError::Conflict);
        }
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn publish_coordination_bytes_guarded(
    path: &Path,
    before: &PhysicalState,
    bytes: &[u8],
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    let after = PhysicalState::from_bytes(bytes.to_vec());
    after.map()?;
    if read_config_pair_physical(path)? != *before {
        return Err(PreparedPairError::Conflict);
    }
    if before == &after {
        return Ok(());
    }
    publish_prepared_pair_image_guarded(path, before, &after, guard)
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
    publish: ConfigPublisher<'_>,
) -> Result<(), String> {
    for ((path, before, after), stage) in [(state, state_stage), (decisions, decisions_stage)] {
        if before != after {
            publish_pair_side_with_publish(path, after, publish)?;
            on_stage(stage);
        }
    }
    Ok(())
}

/// #2786 (C1) - temp plus `publish_temp_config`, never `write_file_atomic`:
/// the pair already holds the process `Mutex`, which is not reentrant.
#[cfg(test)]
fn publish_pair_side(path: &Path, map: &Map<String, Value>) -> Result<(), String> {
    publish_pair_side_with_publish(path, map, &publish_temp_config)
}

fn publish_pair_side_with_publish(
    path: &Path,
    map: &Map<String, Value>,
    publish: ConfigPublisher<'_>,
) -> Result<(), String> {
    let mut json = serde_json::to_string_pretty(map)
        .map_err(|e| format!("Failed to serialize {}: {}", path.display(), e))?;
    json.push('\n');

    let guard = PairTargetGuard::new(&|| Ok(()));
    let (tmp_path, mut file) =
        acquire_fresh_config_stage(path, &guard).map_err(ordinary_staging_error)?;
    let write_result = (|| -> Result<(), String> {
        file.write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write temp config {}: {}", tmp_path.display(), e))?;
        file.flush()
            .map_err(|e| format!("Failed to flush temp config {}: {}", tmp_path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to sync temp config {}: {}", tmp_path.display(), e))
    })();
    drop(file);
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

const MAX_STAGING_ATTEMPTS: usize = 64;
static STAGING_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_staging_sequence(
    sequence: &std::sync::atomic::AtomicU64,
) -> Result<u64, PreparedPairError> {
    sequence
        .fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |value| value.checked_add(1),
        )
        .map_err(|_| PreparedPairError::Io("temporary staging sequence exhausted".into()))
}

fn fresh_config_stage_path(path: &Path, sequence: u64) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.json");
    path.with_file_name(format!(".{}.{}.{}.tmp", name, std::process::id(), sequence))
}

fn acquire_fresh_config_stage(
    path: &Path,
    guard: &PairTargetGuard<'_>,
) -> Result<(PathBuf, std::fs::File), PreparedPairError> {
    acquire_fresh_config_stage_with_sequence(path, guard, &mut || {
        next_staging_sequence(&STAGING_SEQUENCE)
    })
}

// The private seam selects only a sequence; it cannot supply another path or
// relax validation, exclusivity, collision handling or the attempt bound.
fn acquire_fresh_config_stage_with_sequence(
    path: &Path,
    guard: &PairTargetGuard<'_>,
    next: &mut dyn FnMut() -> Result<u64, PreparedPairError>,
) -> Result<(PathBuf, std::fs::File), PreparedPairError> {
    for _ in 0..MAX_STAGING_ATTEMPTS {
        guard.check()?;
        let candidate = fresh_config_stage_path(path, next()?);
        guard.check()?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(PreparedPairError::Io(format!(
                    "Failed to acquire fresh temp config {}: {}",
                    candidate.display(),
                    error,
                )))
            }
        }
    }
    Err(PreparedPairError::Io(format!(
        "Fresh temp config allocation exhausted {} attempts for {}",
        MAX_STAGING_ATTEMPTS,
        path.display(),
    )))
}

fn ordinary_staging_error(error: PreparedPairError) -> String {
    match error {
        PreparedPairError::Io(reason) => reason,
        other => other.to_string(),
    }
}

// Legacy names exist only as detached-debris regression fixture inputs.
#[cfg(test)]
pub(super) fn temp_config_path(path: &Path) -> PathBuf {
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
pub(super) fn publish_temp_config(tmp_path: &Path, path: &Path) -> Result<(), String> {
    ordinary_publish_result(publish_temp_config_guarded(
        tmp_path,
        path,
        &PairTargetGuard::new(&|| Ok(())),
    ))
}

#[cfg(not(windows))]
fn publish_temp_config_guarded(
    tmp_path: &Path,
    path: &Path,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    publish_temp_config_retry_with_attempt(
        tmp_path,
        path,
        guard,
        &mut || std::fs::rename(tmp_path, path),
        &|duration| std::thread::sleep(duration),
    )
}

#[cfg(not(windows))]
fn publish_temp_config_retry_with_attempt(
    tmp_path: &Path,
    path: &Path,
    guard: &PairTargetGuard<'_>,
    publish_attempt: &mut dyn FnMut() -> Result<(), std::io::Error>,
    backoff: &dyn Fn(std::time::Duration),
) -> Result<(), PreparedPairError> {
    guard.check()?;
    let start = std::time::Instant::now();
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..PUBLISH_ATTEMPTS {
        guard.check()?;
        match publish_attempt() {
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
                    return Err(PreparedPairError::Io(format_publish_error(
                        path, tmp_path, &e,
                    )));
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
                if let Some(delay) = PUBLISH_BACKOFFS_MS.get(attempt as usize) {
                    backoff(std::time::Duration::from_millis(*delay));
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
    Err(PreparedPairError::Io(format_publish_error(
        path, tmp_path, &e,
    )))
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
pub(super) fn publish_temp_config(tmp_path: &Path, path: &Path) -> Result<(), String> {
    ordinary_publish_result(publish_temp_config_guarded(
        tmp_path,
        path,
        &PairTargetGuard::new(&|| Ok(())),
    ))
}

#[cfg(windows)]
fn publish_temp_config_guarded(
    tmp_path: &Path,
    path: &Path,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    if !path.exists() {
        guard.check()?;
        return std::fs::rename(tmp_path, path)
            .map_err(|e| {
                format!(
                    "Failed to publish {} as {}: {}",
                    tmp_path.display(),
                    path.display(),
                    e
                )
            })
            .map_err(PreparedPairError::Io);
    }

    let path_wide = publish_path_wide(path);
    let tmp_wide = publish_path_wide(tmp_path);

    publish_temp_config_retry_with_attempt(
        tmp_path,
        path,
        guard,
        &mut || replace_temp_config_once(&path_wide, &tmp_wide),
        &|duration| std::thread::sleep(duration),
    )
}

#[cfg(windows)]
fn publish_temp_config_retry_with_attempt(
    tmp_path: &Path,
    path: &Path,
    guard: &PairTargetGuard<'_>,
    publish_attempt: &mut dyn FnMut() -> Result<(), std::io::Error>,
    backoff: &dyn Fn(std::time::Duration),
) -> Result<(), PreparedPairError> {
    guard.check()?;
    // #537 - ReplaceFileW publishes the temp file over the existing
    // config.json. It returns ERROR_UNABLE_TO_REMOVE_REPLACED (1175) and
    // friends whenever another handle holds the destination for even an
    // instant, so a single collision used to fail the whole write. Retry the
    // publish with a bounded backoff, mirroring rename_with_retry (#280).
    let start = std::time::Instant::now();
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..PUBLISH_ATTEMPTS {
        guard.check()?;
        let outcome = publish_attempt();
        if outcome.is_ok() {
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

        let e = outcome.expect_err("successful attempt returned above");
        if !is_transient_publish_error(&e) {
            return Err(PreparedPairError::Io(format_publish_error(
                path, tmp_path, &e,
            )));
        }
        log::debug!(
            "[config] ReplaceFileW attempt {}/{} failed: path={} os_error={:?}",
            attempt + 1,
            PUBLISH_ATTEMPTS,
            path.display(),
            e.raw_os_error()
        );
        last_err = Some(e);
        if let Some(delay) = PUBLISH_BACKOFFS_MS.get(attempt as usize) {
            backoff(std::time::Duration::from_millis(*delay));
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
    Err(PreparedPairError::Io(format_publish_error(
        path, tmp_path, &e,
    )))
}

#[cfg(test)]
mod tests;

// Synchronous borrowed validation only; no filesystem authority is constructed here.
struct PairTargetGuard<'a> {
    validate: &'a dyn Fn() -> Result<(), PreparedPairError>,
    failed: Cell<bool>,
}

impl<'a> PairTargetGuard<'a> {
    fn new(validate: &'a dyn Fn() -> Result<(), PreparedPairError>) -> Self {
        Self {
            validate,
            failed: Cell::new(false),
        }
    }

    fn check(&self) -> Result<(), PreparedPairError> {
        if self.failed.get() {
            return Err(PreparedPairError::Conflict);
        }
        match (self.validate)() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.failed.set(true);
                Err(error)
            }
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn execute_reserved_config_pair_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    operation_id: &str,
    validate: &dyn Fn() -> Result<(), PreparedPairError>,
) -> Result<(), PreparedPairError> {
    execute_reserved_config_pair_with_validation(
        decisions,
        state,
        reservation,
        operation_id,
        &|_| Ok(()),
        validate,
    )
}

fn log_unverified_cleanup_refusal(primary: &PreparedPairError, refusal: &PreparedPairError) {
    log::warn!(
        "[prepared-pair] cleanup refused; primary={primary}; refusal={refusal}; staging retained"
    );
}

fn ordinary_publish_result(result: Result<(), PreparedPairError>) -> Result<(), String> {
    result.map_err(|error| match error {
        PreparedPairError::Io(reason) => reason,
        other => other.to_string(),
    })
}

#[cfg(windows)]
fn replace_temp_config_once(path_wide: &[u16], tmp_wide: &[u16]) -> Result<(), std::io::Error> {
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};
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
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
fn publish_temp_config_with_attempt(
    tmp_path: &Path,
    path: &Path,
    publish_attempt: &mut dyn FnMut() -> Result<(), std::io::Error>,
    backoff: &dyn Fn(std::time::Duration),
) -> Result<(), String> {
    ordinary_publish_result(publish_temp_config_retry_with_attempt(
        tmp_path,
        path,
        &PairTargetGuard::new(&|| Ok(())),
        publish_attempt,
        backoff,
    ))
}
