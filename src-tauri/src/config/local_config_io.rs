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
pub(super) fn check_configuration_json(path: &Path, bytes: &[u8]) -> Result<(), String> {
    crate::path_identity::validate_ac_configuration_json(path, bytes)
}

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
        check_configuration_json(path, content.as_bytes())?;
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
#[allow(dead_code)] // P52: retain legacy wrapper during checked migration.
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
    update_config_pair_guarded_classified(
        decisions,
        state,
        reservation,
        state_keys,
        nonpending_preflight,
        cleanup,
        on_stage,
        mutate,
        finish,
        activity,
    )
    .map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_config_pair_guarded_classified<F>(
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
) -> Result<(), ConfigWriteError>
where
    F: FnOnce(&mut Map<String, Value>, &mut Map<String, Value>) -> Result<(), String>,
{
    update_config_pair_inner_classified(
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
#[allow(dead_code)] // P52: retain legacy wrapper during checked migration.
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
    update_config_pair_inner_classified(
        decisions,
        state,
        reservation,
        state_keys,
        cleanup,
        on_stage,
        mutate,
        finish,
        activity,
        nonpending_preflight,
        publish,
    )
    .map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_config_pair_inner_classified<F>(
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
) -> Result<(), ConfigWriteError>
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
        validate_pair_paths(decisions, state, path).map_err(ConfigWriteError::from)?;
    }
    let _decisions_lock = acquire_config_file_write_lock(decisions, CONFIG_LOCK_TIMEOUT)?;
    // Every reservation publisher takes decisions first. Keep this lease while
    // classifying presence, so a packet cannot appear during ignore preflight.
    // Protect state/reservation artifacts before creating their sidecars, even
    // when a later read fails. Pending or unreadable packets never preflight.
    if let Some(path) = reservation {
        if read_pair_reservation([decisions, state], path)
            .map_err(ConfigWriteError::from)?
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
            read_pair_reservation([decisions, state], path).map_err(ConfigWriteError::from)?
        {
            validate_pair_binding(decisions, state, &packet).map_err(ConfigWriteError::from)?;
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
                    .map_err(ConfigWriteError::from)?,
                );
                return Ok(());
            }
            let current = read_pair_tuple(decisions, state).map_err(ConfigWriteError::from)?;
            packet.recognize(&current).map_err(ConfigWriteError::from)?;
            let mut d = current[0].map().map_err(ConfigWriteError::from)?;
            let mut s = current[1].map().map_err(ConfigWriteError::from)?;
            mutate(&mut d, &mut s)?;
            let proposed = [
                prepared_pair_image(
                    &current[0],
                    &current[0].map().map_err(ConfigWriteError::from)?,
                    &d,
                )
                .map_err(ConfigWriteError::from)?,
                prepared_pair_image(
                    &current[1],
                    &current[1].map().map_err(ConfigWriteError::from)?,
                    &s,
                )
                .map_err(ConfigWriteError::from)?,
            ];
            if proposed == current {
                return Ok(());
            }
            // Structural changes, history, selection and unknown fields all
            // remain frozen. Only the exact timestamp leaf may move forward.
            if proposed[0] != current[0]
                || protected_revision(&proposed[1], true).map_err(ConfigWriteError::from)?
                    != protected_revision(&current[1], true).map_err(ConfigWriteError::from)?
            {
                // Absent state permits only its timestamp-only overlay.
                if proposed[0] != current[0]
                    || current[1] != PhysicalState::Absent
                    || !timestamp_only_overlay(&proposed[1]).map_err(ConfigWriteError::from)?
                {
                    return Err(ConfigWriteError::Ordinary(
                        "targetTransitionPending: PAIR is reserved".to_string(),
                    ));
                }
            }
            let input = timestamp_value(&proposed[1])
                .map_err(ConfigWriteError::from)?
                .ok_or_else(|| {
                    "targetTransitionPending: timestamp cannot be deleted".to_string()
                })?;
            if timestamp_value(&current[1])
                .map_err(ConfigWriteError::from)?
                .as_deref()
                .is_some_and(|old| timestamp_cmp(&input, old).is_ok_and(|order| order.is_lt()))
            {
                return Err(ConfigWriteError::Ordinary(
                    "targetTransitionPending: timestamp cannot decrease".to_string(),
                ));
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
            .map_err(ConfigWriteError::from)?;
            return Ok(());
        }
    }

    let both = || format!("{} and {}", decisions.display(), state.display());
    let read = |path: &Path| {
        read_pair_side(path).map_err(|e| {
            e.contextualize(|message| format!("Config pair {} blocked: {}", both(), message))
        })
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
    .map_err(ConfigWriteError::Ordinary)
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

#[cfg(test)]
thread_local! {
    static E2992_CANARY_MAP_CALLS: Cell<usize> = const { Cell::new(0) };
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
        #[cfg(test)]
        if let Self::Bytes { bytes, .. } = self {
            const CANARY: &[u8] = br#", "opaque":"e2992-parse-canary""#;
            if bytes.windows(CANARY.len()).any(|part| part == CANARY) {
                E2992_CANARY_MAP_CALLS.with(|count| count.set(count.get() + 1));
            }
        }
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
    DuplicateJsonMember(String),
    Conflict,
    Pending,
    Io(String),
}

impl std::fmt::Display for PreparedPairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPlan(reason) => write!(f, "invalidPreparedPair: {reason}"),
            Self::Preparation(reason) | Self::DuplicateJsonMember(reason) => {
                write!(f, "preparedPairRejected: {reason}")
            }
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

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ConfigWriteError {
    DuplicateJsonMember(String),
    Ordinary(String),
}

impl std::fmt::Display for ConfigWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateJsonMember(message) | Self::Ordinary(message) => f.write_str(message),
        }
    }
}

impl From<String> for ConfigWriteError {
    fn from(message: String) -> Self {
        Self::Ordinary(message)
    }
}

impl ConfigWriteError {
    fn contextualize(self, f: impl FnOnce(String) -> String) -> Self {
        match self {
            Self::DuplicateJsonMember(message) => Self::DuplicateJsonMember(f(message)),
            Self::Ordinary(message) => Self::Ordinary(f(message)),
        }
    }
}

impl From<PreparedPairError> for ConfigWriteError {
    fn from(error: PreparedPairError) -> Self {
        match error {
            PreparedPairError::DuplicateJsonMember(reason) => Self::DuplicateJsonMember(
                PreparedPairError::DuplicateJsonMember(reason).to_string(),
            ),
            ordinary @ (PreparedPairError::InvalidPlan(_)
            | PreparedPairError::Preparation(_)
            | PreparedPairError::Conflict
            | PreparedPairError::Pending
            | PreparedPairError::Io(_)) => Self::Ordinary(ordinary.to_string()),
        }
    }
}

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
    check_configuration_pair_images([decisions, state], plan)?;
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
        if read_pair_reservation([decisions.as_path(), state.as_path()], path)?.is_some() {
            return Err(PreparedPairError::Pending);
        }
    }
    let read = || read_pair_tuple(&decisions, &state);
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

    fn validate(&self, paths: [&Path; 2]) -> Result<(), PreparedPairError> {
        for (path, image) in paths.into_iter().zip(&self.before_tuple) {
            check_configuration_physical(path, image)?;
        }
        check_configuration_pair_images(paths, &self.plan_images)?;
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

fn check_configuration_physical(
    path: &Path,
    image: &PhysicalState,
) -> Result<(), PreparedPairError> {
    if let PhysicalState::Bytes { bytes, .. } = image {
        if PhysicalState::from_bytes(bytes.clone()) != *image {
            return Err(PreparedPairError::InvalidPlan("physical digest mismatch"));
        }
        check_configuration_json(path, bytes).map_err(PreparedPairError::DuplicateJsonMember)?;
    }
    Ok(())
}

fn check_configuration_pair_images(
    paths: [&Path; 2],
    plan: &PreparedConfigPairPlan,
) -> Result<(), PreparedPairError> {
    for tuple in plan.stages() {
        for side in 0..2 {
            check_configuration_physical(paths[side], &tuple[side])?;
        }
    }
    Ok(())
}

fn read_pair_tuple(
    decisions: &Path,
    state: &Path,
) -> Result<[PhysicalState; 2], PreparedPairError> {
    let tuple = [
        read_config_pair_physical(decisions)?,
        read_config_pair_physical(state)?,
    ];
    check_configuration_physical(decisions, &tuple[0])?;
    check_configuration_physical(state, &tuple[1])?;
    Ok(tuple)
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
    packet.validate([decisions, state])?;
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
    paths: [&Path; 2],
    path: &Path,
) -> Result<Option<(PairReservation, PhysicalState)>, PreparedPairError> {
    let physical = read_config_pair_physical(path)?;
    let PhysicalState::Bytes { bytes, .. } = &physical else {
        return Ok(None);
    };
    let packet: PairReservation = serde_json::from_slice(bytes)
        .map_err(|_| PreparedPairError::InvalidPlan("invalid reservation packet"))?;
    packet.validate(paths)?;
    Ok(Some((packet, physical)))
}

fn publish_pair_reservation(
    paths: [&Path; 2],
    path: &Path,
    physical: &mut PhysicalState,
    packet: &PairReservation,
) -> Result<(), PreparedPairError> {
    publish_pair_reservation_guarded(
        paths,
        path,
        physical,
        packet,
        &PairTargetGuard::new(&|| Ok(())),
    )
}

#[cfg_attr(not(test), allow(dead_code))]
fn publish_pair_reservation_guarded(
    paths: [&Path; 2],
    path: &Path,
    physical: &mut PhysicalState,
    packet: &PairReservation,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    packet.validate(paths)?;
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
    let existing = read_pair_reservation([decisions, state], reservation)?;
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
    check_configuration_pair_images([decisions, state], &plan)?;
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
    candidate.validate([decisions, state])?;
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
    publish_pair_reservation_guarded(
        [decisions, state],
        reservation,
        &mut physical,
        &candidate,
        &guard,
    )?;
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
    packet.validate([decisions, state])?;
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
    publish_pair_reservation_guarded([decisions, state], reservation, physical, packet, guard)
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
    publish_pair_reservation([decisions, state], reservation, physical, packet)?;
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
    let (packet, _) = read_pair_reservation([decisions, state], reservation)?
        .ok_or(PreparedPairError::Conflict)?;
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
    let (mut packet, mut physical) = read_pair_reservation([decisions, state], reservation)?
        .ok_or(PreparedPairError::Conflict)?;
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
    confirm_pair_activity_floor_guarded(
        decisions,
        state,
        reservation,
        &mut packet,
        &mut physical,
        &guard,
    )?;
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
                decisions,
                state,
                reservation,
                &mut packet,
                &mut physical,
                &guard,
            )?;
            on_stage(stage)?;
        }
    }
    confirm_pair_activity_floor_guarded(
        decisions,
        state,
        reservation,
        &mut packet,
        &mut physical,
        &guard,
    )?;
    guard.check()?;
    if packet.recognize(&read_pair_tuple(decisions, state)?)? != 4 {
        return Err(PreparedPairError::Conflict);
    }
    guard.check()?;
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn confirm_pair_activity_floor_guarded(
    decisions: &Path,
    state: &Path,
    reservation: &Path,
    packet: &mut PairReservation,
    physical: &mut PhysicalState,
    guard: &PairTargetGuard<'_>,
) -> Result<(), PreparedPairError> {
    guard.check()?;
    let captured = read_config_pair_physical(state)?;
    check_configuration_physical(state, &captured)?;
    let floor =
        merge_monotonic_activity(&[packet.timestamp_floor.clone(), timestamp_value(&captured)?])?;
    if floor != packet.timestamp_floor {
        packet.timestamp_floor = floor;
        publish_pair_reservation_guarded([decisions, state], reservation, physical, packet, guard)?;
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
    let Some((mut packet, mut physical)) = read_pair_reservation([decisions, state], reservation)?
    else {
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
fn read_pair_side(path: &Path) -> Result<Map<String, Value>, ConfigWriteError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(format!("Failed to read {}: {}", path.display(), e).into()),
    };
    check_configuration_json(path, content.as_bytes())
        .map_err(ConfigWriteError::DuplicateJsonMember)?;
    match serde_json::from_str::<Value>(&content) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("Local config {} must be a JSON object", path.display()).into()),
        Err(e) => Err(format!("Failed to parse {}: {}", path.display(), e).into()),
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
    /// The private allocator actually creates the stage; this fixture
    /// verifies the returned name against the existing artifact policy.
    #[test]
    fn atomic_temp_names_stay_inside_the_ignored_glob() {
        use crate::config::instance_artifacts::matches_atomic_write_tmp_glob;
        let root = tempfile::tempdir().unwrap();
        let mut targets: Vec<PathBuf> = [
            "settings.json",
            "sessions",
            "a.b.c.json",
            crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME,
            crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME,
        ]
        .iter()
        .map(|name| root.path().join(name))
        .collect();
        targets.push(root.path().join(non_utf8_file_name()));
        for target in targets {
            let guard = super::PairTargetGuard::new(&|| Ok(()));
            let (temp, file) = super::acquire_fresh_config_stage(&target, &guard).unwrap();
            drop(file);
            let produced = temp.file_name().unwrap().to_str().unwrap();
            assert!(
                matches_atomic_write_tmp_glob(produced),
                "{produced:?} for {target:?}"
            );
            if target.file_name().unwrap()
                == crate::config::instance_artifacts::CONFIG_STATE_TARGET_NAME
                || target.file_name().unwrap()
                    == crate::config::instance_artifacts::CONFIG_IDENTITY_RESERVATION_NAME
            {
                let prefix = format!(".{}.", target.file_name().unwrap().to_str().unwrap());
                assert!(produced.starts_with(&prefix) && produced.ends_with(".tmp"));
            }
            assert!(temp.is_file());
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
        use std::io::Write;
        std::fs::write(&dest, b"old content").expect("write dest");
        let guard = super::PairTargetGuard::new(&|| Ok(()));
        let (tmp, mut file) = super::acquire_fresh_config_stage(&dest, &guard).unwrap();
        file.write_all(b"new content").unwrap();
        file.flush().unwrap();
        file.sync_all().unwrap();
        drop(file);

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
        let debris = temp_config_path(&decisions);
        std::fs::create_dir(&debris).unwrap();
        let stages = std::cell::RefCell::new(Vec::new());
        update_config_pair(
            &decisions,
            &state,
            &PAIR_STATE_KEYS,
            None,
            &|stage| stages.borrow_mut().push(stage.to_string()),
            |d, s| {
                d.insert("a".to_string(), json!(2));
                s.insert("tooling".to_string(), json!({"lastCodingAgent": "claude"}));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(read_value(&state)["tooling"]["lastCodingAgent"], "claude");
        assert_eq!(read_value(&decisions)["a"], 2);
        assert_eq!(
            stages.into_inner(),
            vec!["after_caller_state_publish", "after_caller_decisions_write"]
        );
        assert!(debris.is_dir());
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
    fn prepared_occupied_legacy_stage_does_not_block_and_releases_locks() {
        // Occupied legacy staging is debris, no longer an injected IO failure.
        let plan = prepared_fixture_plan();
        let (_temp, d, s) = pair_fixture(None, None);
        prepared_install(&d, &plan.stages[0][0]);
        prepared_install(&s, &plan.stages[0][1]);
        let debris = temp_config_path(&d);
        std::fs::create_dir(&debris).unwrap();
        execute_prepared_config_pair(&d, &s, &plan).unwrap();
        assert_eq!(
            [
                read_config_pair_physical(&d).unwrap(),
                read_config_pair_physical(&s).unwrap()
            ],
            plan.stages[4]
        );
        assert!(debris.is_dir());
        for path in [&d, &s] {
            drop(acquire_config_file_write_lock(path, Duration::ZERO).unwrap());
        }
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
        assert!(std::fs::read_dir(d.parent().unwrap())
            .unwrap()
            .all(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_none_or(|extension| extension != "tmp")));
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
    // Each fixture owns its temporary directory. Production never scans debris.
    fn p22_stages(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut stages: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "tmp") && path.is_file()
            })
            .map(|path| {
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect();
        stages.sort_by(|left, right| left.0.cmp(&right.0));
        stages
    }

    // A fresh positive-control fixture per call; no state is shared between tests.
    fn calibrate_source_native_b(after: &[u8]) -> (usize, usize) {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let parent = crate::path_identity::retain_directory(root.path()).unwrap();
        let target = parent.identity().canonical_path.join("native-target.json");
        let calls = Cell::new(0usize);
        let pre = Cell::new(0usize);
        let post = Cell::new(0usize);
        let stage = std::cell::RefCell::new(None);
        let observe = || {
            let ordinal = calls.get() + 1;
            calls.set(ordinal);
            let stages = p22_stages(root.path());
            if !target.exists() {
                if stages.len() == 1 && stages[0].1 == after {
                    pre.set(ordinal);
                    *stage.borrow_mut() = Some((
                        stages[0].0.clone(),
                        parent.verify_regular_file(&stages[0].0).unwrap(),
                    ));
                }
            } else if post.get() == 0 {
                assert_eq!(std::fs::read(&target).unwrap(), after);
                assert!(stages.is_empty());
                post.set(ordinal);
                let staged = stage.borrow();
                let (path, identity) = staged.as_ref().unwrap();
                assert!(!path.exists());
                assert!(crate::path_identity::same_object(
                    identity,
                    &parent.verify_regular_file(&target).unwrap()
                ));
            }
            Ok(())
        };
        let published = publish_source_coordination_bytes_with_validation(
            &target,
            &PhysicalState::Absent,
            after,
            &parent,
            &observe,
        );
        assert_eq!(published.result, Ok(()));
        assert!(!published.guard_rejected);
        let landed = parent.verify_regular_file(&target).unwrap();
        assert!(crate::path_identity::same_object(
            published.published_identity.as_ref().unwrap(),
            &landed
        ));
        assert_eq!(std::fs::read(&target).unwrap(), after);
        assert!(p22_stages(root.path()).is_empty());
        assert!(pre.get() > 0);
        assert_eq!(post.get(), pre.get() + 1);
        (pre.get(), post.get())
    }

    #[test]
    fn source_native_b_collision_and_foreign_identical_preserve_target_and_cleanup_owned_stage() {
        let after: &[u8] = br#"{"native":true}"#;
        let (pre_move, post_move) = calibrate_source_native_b(after);
        for foreign in [br#"{"foreign":true}"#.as_slice(), after] {
            let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
            let sentinel = root.path().join("foreign-sentinel.tmp");
            let parent = crate::path_identity::retain_directory(root.path()).unwrap();
            let target = parent.identity().canonical_path.join("native-target.json");
            let calls = Cell::new(0usize);
            let phases = std::cell::RefCell::new(Vec::new());
            let owned = std::cell::RefCell::new(None);
            let foreign_identity = std::cell::RefCell::new(None);
            let sentinel_identity = std::cell::RefCell::new(None);
            let interfere = || {
                let ordinal = calls.get() + 1;
                calls.set(ordinal);
                if ordinal == pre_move {
                    phases.borrow_mut().push("preMove");
                    assert!(!target.exists());
                    let stages = p22_stages(root.path());
                    assert_eq!(stages.len(), 1);
                    assert_eq!(stages[0].1, after);
                    let identity = parent.verify_regular_file(&stages[0].0).unwrap();
                    *owned.borrow_mut() = Some((stages[0].0.clone(), identity.clone()));
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&target)
                        .unwrap();
                    file.write_all(foreign).unwrap();
                    file.sync_all().unwrap();
                    drop(file);
                    let foreign_object = parent.verify_regular_file(&target).unwrap();
                    assert!(!crate::path_identity::same_object(
                        &identity,
                        &foreign_object
                    ));
                    *foreign_identity.borrow_mut() = Some(foreign_object);
                } else if ordinal == post_move {
                    phases.borrow_mut().push("postMove");
                    assert_eq!(std::fs::read(&target).unwrap(), foreign);
                    assert_eq!(
                        parent.verify_regular_file(&target).unwrap(),
                        *foreign_identity.borrow().as_ref().unwrap()
                    );
                    let stage = owned.borrow();
                    let (path, identity) = stage.as_ref().unwrap();
                    assert_eq!(std::fs::read(path).unwrap(), after);
                    assert_eq!(parent.verify_regular_file(path).unwrap(), *identity);
                } else if ordinal == post_move + 1 {
                    phases.borrow_mut().push("cleanupAuthority");
                    assert!(owned.borrow().as_ref().unwrap().0.exists());
                } else if ordinal == post_move + 2 {
                    phases.borrow_mut().push("cleanupOwnedIdentity");
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&sentinel)
                        .unwrap();
                    file.write_all(b"foreign sentinel").unwrap();
                    file.sync_all().unwrap();
                    drop(file);
                    *sentinel_identity.borrow_mut() =
                        Some(parent.verify_regular_file(&sentinel).unwrap());
                } else {
                    assert!(ordinal < pre_move, "unexpected callback after cleanup");
                    assert!(!target.exists());
                }
                Ok(())
            };
            let published = publish_source_coordination_bytes_with_validation(
                &target,
                &PhysicalState::Absent,
                after,
                &parent,
                &interfere,
            );
            assert_eq!(published.result, Err(PreparedPairError::Conflict));
            assert!(!published.guard_rejected);
            assert!(published.published_identity.is_none());
            assert_eq!(
                *phases.borrow(),
                [
                    "preMove",
                    "postMove",
                    "cleanupAuthority",
                    "cleanupOwnedIdentity"
                ]
            );
            assert_eq!(calls.get(), post_move + 2);
            assert_eq!(std::fs::read(&target).unwrap(), foreign);
            assert_eq!(
                parent.verify_regular_file(&target).unwrap(),
                *foreign_identity.borrow().as_ref().unwrap()
            );
            assert!(!owned.borrow().as_ref().unwrap().0.exists());
            assert_eq!(
                parent.verify_regular_file(&sentinel).unwrap(),
                *sentinel_identity.borrow().as_ref().unwrap()
            );
            assert_eq!(
                p22_stages(root.path()),
                vec![(sentinel, b"foreign sentinel".to_vec())]
            );
        }
    }

    #[test]
    fn source_native_b_postmove_guard_refusal_preserves_after_without_ownership_or_cleanup() {
        let after: &[u8] = br#"{"native":true}"#;
        let (pre_move, post_move) = calibrate_source_native_b(after);
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let sentinel = root.path().join("foreign-sentinel.tmp");
        let parent = crate::path_identity::retain_directory(root.path()).unwrap();
        let target = parent.identity().canonical_path.join("native-target.json");
        let calls = Cell::new(0usize);
        let phases = std::cell::RefCell::new(Vec::new());
        let owned = std::cell::RefCell::new(None);
        let snapshot = std::cell::RefCell::new(None);
        let refuse = || {
            let ordinal = calls.get() + 1;
            calls.set(ordinal);
            if ordinal == pre_move {
                phases.borrow_mut().push("preMove");
                assert!(!target.exists());
                let stages = p22_stages(root.path());
                assert_eq!(stages.len(), 1);
                assert_eq!(stages[0].1, after);
                *owned.borrow_mut() = Some((
                    stages[0].0.clone(),
                    parent.verify_regular_file(&stages[0].0).unwrap(),
                ));
            } else if ordinal == post_move {
                phases.borrow_mut().push("postMoveRefusal");
                let landed = parent.verify_regular_file(&target).unwrap();
                let stage = owned.borrow();
                let (path, identity) = stage.as_ref().unwrap();
                assert!(!path.exists());
                assert!(crate::path_identity::same_object(identity, &landed));
                assert_eq!(std::fs::read(&target).unwrap(), after);
                assert!(p22_stages(root.path()).is_empty());
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&sentinel)
                    .unwrap();
                file.write_all(b"foreign sentinel").unwrap();
                file.sync_all().unwrap();
                drop(file);
                *snapshot.borrow_mut() = Some((
                    landed,
                    std::fs::read(&target).unwrap(),
                    p22_stages(root.path()),
                    parent.verify_regular_file(&sentinel).unwrap(),
                ));
                return Err(PreparedPairError::Conflict);
            } else {
                assert!(
                    ordinal < pre_move,
                    "unexpected callback after authority refusal"
                );
                assert!(!target.exists());
            }
            Ok(())
        };
        let published = publish_source_coordination_bytes_with_validation(
            &target,
            &PhysicalState::Absent,
            after,
            &parent,
            &refuse,
        );
        assert_eq!(published.result, Err(PreparedPairError::Conflict));
        assert!(published.guard_rejected);
        assert!(published.published_identity.is_none());
        assert_eq!(*phases.borrow(), ["preMove", "postMoveRefusal"]);
        assert_eq!(calls.get(), post_move);
        assert_eq!(
            (
                parent.verify_regular_file(&target).unwrap(),
                std::fs::read(&target).unwrap(),
                p22_stages(root.path()),
                parent.verify_regular_file(&sentinel).unwrap()
            ),
            snapshot.into_inner().unwrap()
        );
        assert!(!owned.borrow().as_ref().unwrap().0.exists());
    }

    #[test]
    fn source_native_b_premove_guard_refusal_retains_owned_stage_without_effect() {
        let after: &[u8] = br#"{"native":true}"#;
        let (pre_move, post_move) = calibrate_source_native_b(after);
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let sentinel = root.path().join("foreign-sentinel.tmp");
        let parent = crate::path_identity::retain_directory(root.path()).unwrap();
        let target = parent.identity().canonical_path.join("native-target.json");
        let calls = Cell::new(0usize);
        let phases = std::cell::RefCell::new(Vec::new());
        let snapshot = std::cell::RefCell::new(None);
        let refuse = || {
            let ordinal = calls.get() + 1;
            calls.set(ordinal);
            if ordinal == pre_move {
                phases.borrow_mut().push("preMoveRefusal");
                assert!(!target.exists());
                let stages = p22_stages(root.path());
                assert_eq!(stages.len(), 1);
                assert_eq!(stages[0].1, after);
                let identity = parent.verify_regular_file(&stages[0].0).unwrap();
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&sentinel)
                    .unwrap();
                file.write_all(b"foreign sentinel").unwrap();
                file.sync_all().unwrap();
                drop(file);
                *snapshot.borrow_mut() = Some((
                    stages[0].0.clone(),
                    identity,
                    p22_stages(root.path()),
                    parent.verify_regular_file(&sentinel).unwrap(),
                ));
                return Err(PreparedPairError::Conflict);
            }
            assert!(
                ordinal < pre_move,
                "unexpected callback after authority refusal"
            );
            assert!(!target.exists());
            Ok(())
        };
        let published = publish_source_coordination_bytes_with_validation(
            &target,
            &PhysicalState::Absent,
            after,
            &parent,
            &refuse,
        );
        assert_eq!(published.result, Err(PreparedPairError::Conflict));
        assert!(published.guard_rejected);
        assert!(published.published_identity.is_none());
        assert_eq!(*phases.borrow(), ["preMoveRefusal"]);
        assert_eq!(calls.get(), pre_move);
        assert!(pre_move < post_move);
        assert!(!target.exists());
        let (stage, identity, stages, sentinel_object) = snapshot.into_inner().unwrap();
        assert_eq!(parent.verify_regular_file(&stage).unwrap(), identity);
        assert_eq!(std::fs::read(&stage).unwrap(), after);
        assert_eq!(p22_stages(root.path()), stages);
        assert_eq!(
            parent.verify_regular_file(&sentinel).unwrap(),
            sentinel_object
        );
        // Retained diagnostic stage is NOT cleanup or product GC proof.
        // Production never scans this debris; fixture TempDir Drop is fixture-only.
    }

    fn p22_filled_stage(target: &Path, bytes: &[u8]) -> PathBuf {
        let guard = PairTargetGuard::new(&|| Ok(()));
        let (stage, mut file) = acquire_fresh_config_stage(target, &guard).unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        file.sync_all().unwrap();
        drop(file);
        stage
    }

    fn p22_next_writer(mode: usize, target: &Path) {
        match mode {
            0 => {
                update_config_json_object(target, true, |map| {
                    map.insert("value".into(), json!(2));
                    Ok(())
                })
                .unwrap();
            }
            1 => write_file_atomic(target, b"{\"value\":2}").unwrap(),
            2 => publish_pair_side(target, json!({"value":2}).as_object().unwrap()).unwrap(),
            3 => publish_prepared_pair_image(
                target,
                &read_config_pair_physical(target).unwrap(),
                &PhysicalState::from_bytes(b"{\"value\":2}".to_vec()),
            )
            .unwrap(),
            _ => unreachable!(),
        }
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(target).unwrap()).unwrap(),
            json!({"value":2})
        );
    }

    #[test]
    fn p22_all_four_next_writers_leave_detached_legacy_debris_unchanged() {
        for mode in 0..4 {
            let root = tempfile::tempdir().unwrap();
            let target = root.path().join("config.json");
            let debris = temp_config_path(&target);
            std::fs::write(&debris, b"detached stale bytes").unwrap();
            std::fs::write(&target, b"{\"value\":1}").unwrap();
            p22_next_writer(mode, &target);
            assert_eq!(std::fs::read(&debris).unwrap(), b"detached stale bytes");
            assert_eq!(
                p22_stages(root.path()),
                vec![(debris, b"detached stale bytes".to_vec())]
            );
        }
    }

    #[test]
    fn p22_all_four_next_writers_preserve_refused_absent_publish_hardlink_alias() {
        for mode in 0..4 {
            let root = tempfile::tempdir().unwrap();
            let target = root.path().join("config.json");
            let validate = || {
                if target.exists() {
                    Err(PreparedPairError::Conflict)
                } else {
                    Ok(())
                }
            };
            let guard = PairTargetGuard::new(&validate);
            let first = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
            assert_eq!(
                publish_prepared_pair_image_guarded(
                    &target,
                    &PhysicalState::Absent,
                    &first,
                    &guard
                ),
                Err(PreparedPairError::Conflict)
            );
            let stages = p22_stages(root.path());
            assert_eq!(stages.len(), 1);
            let alias = stages[0].0.clone();
            assert_eq!(std::fs::read(&target).unwrap(), stages[0].1);
            p22_next_writer(mode, &target);
            assert_eq!(std::fs::read(&alias).unwrap(), b"{\"value\":1}");
            assert_eq!(p22_stages(root.path()), stages);
        }
    }

    #[test]
    fn p22_atomic_candidate_collisions_regular_directory_hardlink_and_race() {
        for kind in 0..4 {
            let root = tempfile::tempdir().unwrap();
            let target = root.path().join("config.json");
            let occupied = fresh_config_stage_path(&target, 0);
            let linked = root.path().join("linked-original");
            match kind {
                0 => std::fs::write(&occupied, b"occupied").unwrap(),
                1 => std::fs::create_dir(&occupied).unwrap(),
                2 => {
                    std::fs::write(&linked, b"occupied").unwrap();
                    std::fs::hard_link(&linked, &occupied).unwrap();
                }
                3 => {}
                _ => unreachable!(),
            }
            let mut calls = 0;
            let mut next = || {
                let value = calls;
                calls += 1;
                if kind == 3 && value == 0 {
                    // Competitor creates the candidate before our atomic open.
                    std::fs::write(&occupied, b"occupied").unwrap();
                }
                Ok(value)
            };
            let guard = PairTargetGuard::new(&|| Ok(()));
            match acquire_fresh_config_stage_with_sequence(&target, &guard, &mut next) {
                Ok((stage, mut file)) => {
                    assert_eq!(stage, fresh_config_stage_path(&target, 1));
                    assert_eq!(calls, 2);
                    file.write_all(b"owned").unwrap();
                    drop(file);
                }
                Err(PreparedPairError::Io(reason)) if kind == 1 => {
                    // Some hosts report a directory as a non-AlreadyExists
                    // open error; that precise error must surface immediately.
                    assert!(reason.contains(&occupied.display().to_string()));
                    assert_eq!(calls, 1);
                }
                other => panic!("unexpected collision result: {other:?}"),
            }
            if kind == 1 {
                assert!(occupied.is_dir());
            } else {
                assert_eq!(std::fs::read(&occupied).unwrap(), b"occupied");
            }
            if kind == 2 {
                assert_eq!(std::fs::read(linked).unwrap(), b"occupied");
            }
        }
    }

    #[test]
    fn p22_exact_64_attempt_exhaustion_never_cleans_unacquired_candidates() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.json");
        for sequence in 0..MAX_STAGING_ATTEMPTS as u64 {
            std::fs::write(fresh_config_stage_path(&target, sequence), b"occupied").unwrap();
        }
        let original = p22_stages(root.path());
        let mut calls = 0;
        let mut next = || {
            let value = calls;
            calls += 1;
            Ok(value)
        };
        let guard = PairTargetGuard::new(&|| Ok(()));
        let result = publish_prepared_pair_image_with_io(
            &target,
            &PhysicalState::Absent,
            &PhysicalState::from_bytes(b"{}".to_vec()),
            &guard,
            &|_, _| panic!("no cleanup diagnostic without acquisition"),
            &mut next,
            &|_, _| panic!("no write without acquisition"),
        );
        assert!(
            matches!(result, Err(PreparedPairError::Io(reason)) if reason.contains("64 attempts"))
        );
        assert_eq!(calls, 64);
        assert_eq!(p22_stages(root.path()), original);
        assert!(!target.exists());
    }

    #[test]
    fn p22_sequence_overflow_never_wraps_or_creates_a_candidate() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let sequence = AtomicU64::new(u64::MAX - 1);
        assert_eq!(next_staging_sequence(&sequence), Ok(u64::MAX - 1));
        assert_eq!(sequence.load(Ordering::Relaxed), u64::MAX);
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("config.json");
        let guard = PairTargetGuard::new(&|| Ok(()));
        assert!(matches!(
            acquire_fresh_config_stage_with_sequence(&target, &guard, &mut || {
                next_staging_sequence(&sequence)
            }),
            Err(PreparedPairError::Io(_))
        ));
        assert_eq!(sequence.load(Ordering::Relaxed), u64::MAX);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn p22_allocator_refusal_before_each_candidate_is_sticky() {
        for cut in 1..=4 {
            let root = tempfile::tempdir().unwrap();
            let target = root.path().join("config.json");
            let occupied = fresh_config_stage_path(&target, 0);
            std::fs::write(&occupied, b"occupied").unwrap();
            let calls = Cell::new(0);
            let validate = || {
                calls.set(calls.get() + 1);
                if calls.get() == cut {
                    Err(PreparedPairError::Conflict)
                } else {
                    Ok(())
                }
            };
            let guard = PairTargetGuard::new(&validate);
            let mut sequence = 0;
            let mut next = || {
                let value = sequence;
                sequence += 1;
                Ok(value)
            };
            assert!(matches!(
                acquire_fresh_config_stage_with_sequence(&target, &guard, &mut next),
                Err(PreparedPairError::Conflict)
            ));
            assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
            assert_eq!(calls.get(), cut);
            assert_eq!(
                p22_stages(root.path()),
                vec![(occupied, b"occupied".to_vec())]
            );
        }
    }

    #[test]
    fn p22_fresh_staging_long_paths_all_four_writers() {
        let root = tempfile::tempdir().unwrap();
        let mut directory = root.path().to_path_buf();
        while directory.as_os_str().to_string_lossy().len() <= 300 {
            directory.push("long-path-component-0123456789");
        }
        std::fs::create_dir_all(&directory).unwrap();
        for mode in 0..4 {
            let target = directory.join(format!("config-{mode}.json"));
            std::fs::write(&target, b"{\"value\":1}").unwrap();
            p22_next_writer(mode, &target);
        }
        assert!(p22_stages(&directory).is_empty());
    }

    #[test]
    fn p22_noncollision_acquisition_error_is_immediate_and_precise() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("missing-parent").join("config.json");
        let guard = PairTargetGuard::new(&|| Ok(()));
        let mut calls = 0;
        let mut next = || {
            calls += 1;
            Ok(0)
        };
        let result = publish_prepared_pair_image_with_io(
            &target,
            &PhysicalState::Absent,
            &PhysicalState::from_bytes(b"{}".to_vec()),
            &guard,
            &|_, _| panic!("no cleanup without acquisition"),
            &mut next,
            &|_, _| panic!("no write without acquisition"),
        );
        assert!(matches!(result, Err(PreparedPairError::Io(reason))
            if reason.contains(&fresh_config_stage_path(&target, 0).display().to_string())));
        assert_eq!(calls, 1);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn p22_observed_directory_witness_change_after_acquisition_preserves_stage() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("parent");
        std::fs::create_dir(&directory).unwrap();
        let witness = directory.join("fixture-directory-witness");
        std::fs::write(&witness, b"original witness").unwrap();
        let target = directory.join("config.json");
        std::fs::write(&target, b"{\"value\":0}").unwrap();
        let before = read_config_pair_physical(&target).unwrap();
        let validate = || {
            if std::fs::read(&witness).unwrap() == b"original witness" {
                Ok(())
            } else {
                Err(PreparedPairError::Conflict)
            }
        };
        let guard = PairTargetGuard::new(&validate);
        let mut next = || next_staging_sequence(&STAGING_SEQUENCE);
        let write = |file: &mut std::fs::File, bytes: &[u8]| {
            file.write_all(bytes).unwrap();
            // A fixture marker models an observed directory witness. It is
            // neither production identity proof nor a hostile-host claim.
            std::fs::write(&witness, b"changed witness").unwrap();
            Ok(())
        };
        assert_eq!(
            publish_prepared_pair_image_with_io(
                &target,
                &before,
                &PhysicalState::from_bytes(b"{\"value\":1}".to_vec()),
                &guard,
                &|_, _| panic!("sticky refusal skips cleanup"),
                &mut next,
                &write
            ),
            Err(PreparedPairError::Conflict)
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"value\":0}");
        let stages = p22_stages(&directory);
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].1, b"{\"value\":1}");
        assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
        assert_eq!(p22_stages(&directory), stages);
    }

    #[test]
    fn e2992_duplicate_json_pair_original_and_embedded_images_before_parse() {
        for side in 0..2 {
            for slot in 0usize..7 {
                assert_pair_image_duplicate_rejected(side, slot);
            }
        }
    }

    fn assert_pair_image_duplicate_rejected(side: usize, slot: usize) {
        let fixture = Fixture::new(false);
        for path in [fixture.d(), fixture.s()] {
            let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            value["opaque"] = json!("e2992-parse-canary");
            fixture.write(&path, value);
        }
        let request = fixture.request();
        let mut packet = fixture.reserve(&request).unwrap();
        let d = fixture.d();
        let st = fixture.s();
        let paths = [d.as_path(), st.as_path()];
        let stage = slot.saturating_sub(2);
        let valid = match &packet.plan_images.stages[stage][side] {
            PhysicalState::Bytes { bytes, .. } => bytes,
            PhysicalState::Absent => panic!("present fixture"),
        };
        let closing = valid.iter().rposition(|byte| *byte == b'}').unwrap();
        let mut duplicate = valid[..closing].to_vec();
        duplicate.extend_from_slice(br#", "opaque":"e2992-parse-canary"}"#);
        assert_eq!(
            serde_json::from_slice::<Value>(&duplicate).unwrap(),
            serde_json::from_slice::<Value>(valid).unwrap()
        );
        let image = PhysicalState::from_bytes(duplicate.clone());
        match slot {
            0 => std::fs::write(paths[side], &duplicate).unwrap(),
            1 => packet.before_tuple[side] = image,
            _ => packet.plan_images.stages[slot - 2][side] = image,
        }
        let envelope = serde_json::to_vec(&packet).unwrap();
        std::fs::write(fixture.r(), &envelope).unwrap();
        let originals = [std::fs::read(&d).unwrap(), std::fs::read(&st).unwrap()];
        E2992_CANARY_MAP_CALLS.with(|count| count.set(0));
        let error =
            execute_reserved_config_pair(&d, &st, &fixture.r(), &request.operation_id).unwrap_err();
        assert!(
            matches!(&error, PreparedPairError::DuplicateJsonMember(message) if message.contains("duplicateJsonMember:") && message.contains(paths[side].to_str().unwrap()) && message.contains("repeated JSON object members"))
        );
        assert!(!error.to_string().contains("canary"));
        let called = Cell::new(false);
        let classified = update_config_pair_guarded_classified(
            &d,
            &st,
            &fixture.r(),
            &[],
            None,
            Some(&|_, _| {
                called.set(true);
                Ok(())
            }),
            &|_| {
                called.set(true);
            },
            |_, _| {
                called.set(true);
                Ok(())
            },
            &|_| {
                called.set(true);
                Ok(())
            },
            None,
        )
        .unwrap_err();
        assert!(
            matches!(&classified, ConfigWriteError::DuplicateJsonMember(message)
            if message.contains(paths[side].to_str().unwrap()))
        );
        assert!(!called.get());
        assert_eq!(E2992_CANARY_MAP_CALLS.with(Cell::get), 0);
        assert_eq!(std::fs::read(&d).unwrap(), originals[0]);
        assert_eq!(std::fs::read(&st).unwrap(), originals[1]);
        assert_eq!(std::fs::read(fixture.r()).unwrap(), envelope);
        if slot != 0 {
            let mut physical = PhysicalState::from_bytes(envelope.clone());
            let before = packet.clone();
            let error = reconcile_pair_activity_guarded(
                &d,
                &st,
                &fixture.r(),
                &mut packet,
                &mut physical,
                &PairTargetGuard::new(&|| Ok(())),
            )
            .unwrap_err();
            assert!(matches!(error, PreparedPairError::DuplicateJsonMember(_)));
            assert_eq!(packet, before);
            assert_eq!(std::fs::read(&d).unwrap(), originals[0]);
            assert_eq!(std::fs::read(&st).unwrap(), originals[1]);
            assert_eq!(E2992_CANARY_MAP_CALLS.with(Cell::get), 0);
            assert_eq!(std::fs::read(fixture.r()).unwrap(), envelope);
        }
    }

    #[test]
    fn e2992_duplicate_json_pair_ordinary_callbacks_and_envelope_controls() {
        let fixture = Fixture::new(false);
        let request = fixture.request();
        let mut packet = fixture.reserve(&request).unwrap();
        let mut physical = PhysicalState::from_bytes(std::fs::read(fixture.r()).unwrap());
        reconcile_pair_activity_guarded(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            &mut packet,
            &mut physical,
            &PairTargetGuard::new(&|| Ok(())),
        )
        .unwrap();
        execute_reserved_config_pair(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            &request.operation_id,
        )
        .unwrap();
        for invalid in 0..6 {
            let fixture = Fixture::new(false);
            let request = fixture.request();
            let packet = fixture.reserve(&request).unwrap();
            let mut envelope = serde_json::to_value(&packet).unwrap();
            match invalid {
                0 => envelope["unknown"] = json!(true),
                1 => envelope["version"] = json!(1),
                2 => envelope["planDigest"] = json!("wrong"),
                3 => envelope["protectedImages"] = json!([]),
                4 => envelope["timestampFloor"] = json!("invalid timestamp"),
                _ => envelope["activityIntent"] = json!({"invalid": true}),
            }
            let bytes = serde_json::to_vec(&envelope).unwrap();
            std::fs::write(fixture.r(), &bytes).unwrap();
            let before = fixture.tuple();
            let error =
                read_pair_reservation([&fixture.d(), &fixture.s()], &fixture.r()).unwrap_err();
            assert!(!matches!(error, PreparedPairError::DuplicateJsonMember(_)));
            assert_eq!(fixture.tuple(), before);
            assert_eq!(std::fs::read(fixture.r()).unwrap(), bytes);
        }
        for side in 0..2 {
            let fixture = Fixture::new(false);
            let d = fixture.d();
            let st = fixture.s();
            let paths = [&d, &st];
            std::fs::write(
                paths[side],
                br#"{"unknown-canary":"secret-canary","unknown-canary":0}"#,
            )
            .unwrap();
            let original = [std::fs::read(&d).unwrap(), std::fs::read(&st).unwrap()];
            let called = Cell::new(false);
            let error = update_config_pair_inner_classified(
                &d,
                &st,
                None,
                &[],
                Some(&|_, _| {
                    called.set(true);
                    Ok(())
                }),
                &|_| {
                    called.set(true);
                },
                |_, _| {
                    called.set(true);
                    Ok(())
                },
                &|_| {
                    called.set(true);
                    Ok(())
                },
                None,
                None,
                &|_, _| {
                    called.set(true);
                    Ok(())
                },
            )
            .unwrap_err();
            assert!(
                matches!(&error, ConfigWriteError::DuplicateJsonMember(message)
                if message.contains(paths[side].to_str().unwrap()) && message.contains("repeated JSON object members"))
            );
            assert!(!error.to_string().contains("canary"));
            assert!(!called.get());
            assert_eq!(std::fs::read(&d).unwrap(), original[0]);
            assert_eq!(std::fs::read(&st).unwrap(), original[1]);
        }
        let fixture = Fixture::new(false);
        let error = prepare_config_pair_plan_inner(fixture.tuple(), None, |_, _| {
            Err("duplicateJsonMember: callback remains ordinary".into())
        })
        .unwrap_err();
        assert!(matches!(error, PreparedPairError::Preparation(_)));
        let error = update_config_pair_inner_classified(
            &fixture.d(),
            &fixture.s(),
            None,
            &[],
            None,
            &|_| {},
            |_, _| Err("duplicateJsonMember: ordinary callback".into()),
            &|_| Ok(()),
            None,
            None,
            &publish_temp_config,
        )
        .unwrap_err();
        assert!(matches!(error, ConfigWriteError::Ordinary(_)));
    }

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
            read_pair_reservation([&self.d(), &self.s()], &self.r())
                .unwrap()
                .unwrap()
                .0
        }
        fn request(&self) -> PairReservationRequest {
            PairReservationRequest {
                operation_id: "operation-1".into(),
                source_physical_key: "source-key".into(),
                mappings_digest: "a".repeat(64),
                before_protected: protected_config_pair_revision(&self.tuple()).unwrap(),
            }
        }
        fn reserve(
            &self,
            request: &PairReservationRequest,
        ) -> Result<PairReservation, PreparedPairError> {
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
            let (mut packet, mut physical) =
                read_pair_reservation([&self.d(), &self.s()], &self.r())?.unwrap();
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
        fn complete_projection(&self) -> Value {
            let packet = self.packet();
            let hashes = packet.protected_revision_digests().unwrap();
            let target = json!({"targetId":packet.physical_target_identity,
                "beforeProtectedRevision":{"kind":"bytes","sha256":hashes[0]},
                "afterProtectedRevision":{"kind":"bytes","sha256":hashes[1]},
                "planDigest":packet.plan_digest});
            let revision = json!({"kind":"bytes","sha256":"b".repeat(64)});
            json!({"schemaVersion":2,"physicalSourceKey":"source-key","operationId":"operation-1",
                "phase":"complete","afterSourceRevision":revision,"packetsPrepared":true,
                "sourcePublished":true,"refsVerified":true,"reloaded":true,"packets":[target],
                "receipt":{"operationId":"operation-1","sourceRevision":revision}})
        }
    }

    #[test]
    fn pair_activity_atomic_packet_floor_and_locks_before_presence() {
        let f = Fixture::new(false);
        let request = f.request();
        let before = f.tuple();
        let packet =
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
        assert_eq!(f.tuple(), before);
        assert_eq!(f.packet(), packet);
        assert_eq!(packet.timestamp_floor.as_deref(), Some(T1));
        assert_eq!(packet.plan_images.stages.len(), 5);
        assert!(packet.activity_intent.is_none());
    }

    #[test]
    fn pair_activity_occupied_legacy_stage_allows_packet_without_target_write() {
        let f = Fixture::new(false);
        let before = f.tuple();
        let debris = temp_config_path(&f.r());
        std::fs::create_dir(&debris).unwrap();
        let _packet = f.reserve(&f.request()).unwrap();
        assert!(f.r().is_file());
        assert!(debris.is_dir());
        assert_eq!(f.tuple(), before);
        for path in [f.d(), f.s(), f.r()] {
            drop(acquire_config_file_write_lock(&path, Duration::ZERO).unwrap());
        }
    }

    #[test]
    fn single_editor_pair_activity_same_operation_original_and_incompatible_refusal() {
        let f = Fixture::new(false);
        let request = f.request();
        let original = f.reserve(&request).unwrap();
        let bytes = std::fs::read(f.r()).unwrap();
        assert_eq!(f.reserve(&request).unwrap(), original);
        assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
        assert_eq!(
            verify_reserved_config_pair(
                &f.d(),
                &f.s(),
                &f.r(),
                "operation-1",
                &original.plan_digest
            ),
            Err(PreparedPairError::Pending)
        );
        let mut foreign = request.clone();
        foreign.operation_id = "operation-2".into();
        assert_eq!(f.reserve(&foreign), Err(PreparedPairError::Conflict));
        let divergent =
            prepare_and_reserve_config_pair(&f.d(), &f.s(), &f.r(), &request, None, |d, _| {
                d.insert("decision".into(), json!(9));
                Ok(())
            });
        assert_eq!(divergent, Err(PreparedPairError::Conflict));
        assert_eq!(std::fs::read(f.r()).unwrap(), bytes);
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
        assert_eq!(
            verify_reserved_config_pair(
                &f.d(),
                &f.s(),
                &f.r(),
                "operation-1",
                &original.plan_digest
            )
            .unwrap(),
            f.tuple()
        );
    }

    #[test]
    fn pair_activity_generic_noop_zero_write_and_history_selection_unknown_rejected() {
        let f = Fixture::new(false);
        f.reserve(&f.request()).unwrap();
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
        let original = f.reserve(&f.request()).unwrap();
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
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
        assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T3));
    }

    #[test]
    fn pair_activity_every_t0_t4_cut_recovers_with_monotonic_stamp() {
        for absent in [false, true] {
            for cut in 0..=4 {
                let f = Fixture::new(absent);
                f.reserve(&f.request()).unwrap();
                if cut != 0 {
                    let result = execute_reserved_config_pair_with_stage(
                        &f.d(),
                        &f.s(),
                        &f.r(),
                        "operation-1",
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
                execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
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
            f.reserve(&f.request()).unwrap();
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
        f.reserve(&f.request()).unwrap();
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
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
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
            f.reserve(&f.request()).unwrap();
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
                execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1"),
                Err(PreparedPairError::Conflict)
            );
            assert_eq!(f.tuple(), tuple);
            assert_eq!(std::fs::read(f.r()).unwrap(), packet);
        }
    }

    #[test]
    fn pair_activity_corrupt_packet_wrong_target_and_before_cas_preserve_evidence() {
        let f = Fixture::new(false);
        let mut wrong = f.request();
        wrong.before_protected[0] = ProtectedRevision::Absent;
        assert_eq!(f.reserve(&wrong), Err(PreparedPairError::Conflict));
        assert!(!f.r().exists());
        f.reserve(&f.request()).unwrap();
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
            let request = f.request();
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
        let request = f.request();
        let packet =
            prepare_and_reserve_config_pair(&f.d(), &f.s(), &f.r(), &request, None, |_, _| Ok(()))
                .unwrap();
        assert_eq!(packet.recognize(&f.tuple()).unwrap(), 4);
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
        assert!(!f.d().exists());
        assert!(!f.s().exists());
        assert_eq!(f.activity(T2), Ok(true));
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
        assert!(!f.d().exists());
        assert_eq!(timestamp_value(&f.tuple()[1]).unwrap().as_deref(), Some(T2));
        assert_eq!(f.packet().recognize(&f.tuple()).unwrap(), 4);
    }

    #[test]
    fn pair_activity_intent_unknown_timestamp_and_exact_generic_delta() {
        let f = Fixture::new(false);
        f.reserve(&f.request()).unwrap();
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
        let parent = f.root.path().canonicalize().unwrap();
        let state_lock = config_lock_path(&parent, &f.s()).unwrap();
        let reservation_lock = config_lock_path(&parent, &f.r()).unwrap();
        let called = Cell::new(false);
        let preflight = || {
            assert!(!state_lock.exists());
            assert!(!reservation_lock.exists());
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
        assert!(!state_lock.exists());
        assert!(!reservation_lock.exists());
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
        assert!(!state_lock.exists());
        assert!(!reservation_lock.exists());
    }

    #[test]
    fn single_editor_pair_activity_release_requires_local_receipt_source_and_final_target() {
        let f = Fixture::new(false);
        f.reserve(&f.request()).unwrap();
        let target = f.packet().physical_target_identity;
        let ledger = f.complete_projection();
        let revision = &ledger["afterSourceRevision"];
        let proof = PairCompleteProof::from_single_editor_complete(
            &ledger,
            "operation-1",
            &target,
            revision,
        )
        .unwrap();
        assert_eq!(
            release_reserved_config_pair(&f.d(), &f.s(), &f.r(), &proof),
            Err(PreparedPairError::Conflict)
        );
        assert!(PairCompleteProof::from_single_editor_complete(
            &ledger,
            "operation-1",
            &target,
            &json!({"kind":"absent"})
        )
        .is_err());
        for key in ["phase", "sourcePublished", "receipt"] {
            let mut invalid = ledger.clone();
            invalid[key] = Value::Null;
            assert!(PairCompleteProof::from_single_editor_complete(
                &invalid,
                "operation-1",
                &target,
                revision
            )
            .is_err());
        }
        execute_reserved_config_pair(&f.d(), &f.s(), &f.r(), "operation-1").unwrap();
        f.activity(T3).unwrap();
        let before = f.tuple();
        assert_eq!(
            release_reserved_config_pair(&f.d(), &f.s(), &f.r(), &proof).unwrap(),
            before
        );
        assert!(!f.r().exists());
        assert!(config_lock_path(f.root.path(), &f.r()).unwrap().exists());
        assert_eq!(
            release_reserved_config_pair(&f.d(), &f.s(), &f.r(), &proof).unwrap(),
            before
        );
        assert_eq!(timestamp_value(&before[1]).unwrap().as_deref(), Some(T3));
    }
    // P22: deterministic failure at each reached validation boundary.
    fn p22_prepare(
        fixture: &Fixture,
        request: &PairReservationRequest,
        validate: &dyn Fn() -> Result<(), PreparedPairError>,
    ) -> Result<PairReservation, PreparedPairError> {
        let cleanup = |decisions: &mut Map<String, Value>, state: &mut Map<String, Value>| {
            decisions.insert("cleanup".into(), json!(1));
            state.insert("cleanup".into(), json!(1));
            Ok(())
        };
        prepare_and_reserve_config_pair_guarded(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            request,
            Some(&cleanup),
            |decisions, state| {
                decisions.insert("decision".into(), json!(2));
                state.insert("caller".into(), json!(2));
                Ok(())
            },
            validate,
        )
    }

    type P22Artifacts = (Vec<u8>, Vec<u8>, Option<Vec<u8>>, Vec<(String, Vec<u8>)>);

    fn p22_artifacts(fixture: &Fixture) -> P22Artifacts {
        let mut stages = std::fs::read_dir(fixture.root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "tmp"))
            .map(|path| {
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    std::fs::read(path).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        stages.sort();
        (
            std::fs::read(fixture.d()).unwrap(),
            std::fs::read(fixture.s()).unwrap(),
            std::fs::read(fixture.r()).ok(),
            stages,
        )
    }

    fn p22_case(mode: usize, stop: Option<usize>) -> (usize, Result<(), PreparedPairError>) {
        let fixture = Fixture::new(false);
        let original_request = fixture.request();
        let mut proof = None;
        if mode != 0 {
            p22_prepare(&fixture, &original_request, &|| Ok(())).unwrap();
        }
        if [2, 4, 9, 10].contains(&mode) {
            assert!(fixture
                .interrupted_activity(T2, "activity_intent_durable")
                .is_err());
        }
        if mode == 5 {
            let mut state: Value =
                serde_json::from_slice(&std::fs::read(fixture.s()).unwrap()).unwrap();
            state["tooling"]["lastAgentMessageAt"] = json!(T3);
            fixture.write(&fixture.s(), state);
        }
        if [6, 7, 8].contains(&mode) {
            execute_reserved_config_pair(&fixture.d(), &fixture.s(), &fixture.r(), "operation-1")
                .unwrap();
            if mode == 7 {
                assert!(fixture
                    .interrupted_activity(T3, "activity_intent_durable")
                    .is_err());
            }
            proof = Some(
                PairCompleteProof::from_single_editor_complete(
                    &fixture.complete_projection(),
                    "operation-1",
                    &fixture.packet().physical_target_identity,
                    &json!({"kind":"bytes", "sha256":"b".repeat(64)}),
                )
                .unwrap(),
            );
            if mode == 8 {
                release_reserved_config_pair(
                    &fixture.d(),
                    &fixture.s(),
                    &fixture.r(),
                    proof.as_ref().unwrap(),
                )
                .unwrap();
            }
        }
        let calls = Cell::new(0);
        let at_failure = std::cell::RefCell::new(None);
        let validate = || {
            calls.set(calls.get() + 1);
            if stop == Some(calls.get()) {
                *at_failure.borrow_mut() = Some(p22_artifacts(&fixture));
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        };
        let result = match mode {
            0..=3 | 10 => p22_prepare(&fixture, &original_request, &validate).map(|_| ()),
            4..=5 => execute_reserved_config_pair_guarded(
                &fixture.d(),
                &fixture.s(),
                &fixture.r(),
                "operation-1",
                &validate,
            ),
            6 | 9 => verify_reserved_config_pair_guarded(
                &fixture.d(),
                &fixture.s(),
                &fixture.r(),
                "operation-1",
                &fixture.packet().plan_digest,
                &validate,
            )
            .map(|_| ()),
            7..=8 => release_reserved_config_pair_guarded(
                &fixture.d(),
                &fixture.s(),
                &fixture.r(),
                proof.as_ref().unwrap(),
                &validate,
            )
            .map(|_| ()),
            _ => unreachable!(),
        };
        if let Some(expected) = at_failure.borrow().as_ref() {
            assert_eq!(
                &p22_artifacts(&fixture),
                expected,
                "no target, packet or staging mutation after failed validation"
            );
            assert_eq!(result, Err(PreparedPairError::Conflict));
            assert_eq!(Some(calls.get()), stop, "failed guard remains failed");
        }
        (calls.get(), result)
    }

    #[test]
    fn single_editor_pair_activity_every_reached_reservation_execution_verify_release_cut() {
        let mut total_cuts = 0;
        for mode in 0..11 {
            let (count, result) = p22_case(mode, None);
            if mode == 9 {
                assert_eq!(result, Err(PreparedPairError::Pending));
            } else {
                result.unwrap();
            }
            assert!(count > 0);
            for cut in 1..=count {
                assert_eq!(
                    p22_case(mode, Some(cut)).1,
                    Err(PreparedPairError::Conflict)
                );
                total_cuts += 1;
            }
        }
        assert!(
            total_cuts >= 11,
            "all eleven independently prepared paths ran"
        );
    }

    #[test]
    fn p22_validation_after_mutation_before_first_packet_preserves_original_tuple() {
        let fixture = Fixture::new(false);
        let request = fixture.request();
        let before = fixture.tuple();
        let mutated = Cell::new(false);
        let result = prepare_and_reserve_config_pair_guarded(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            &request,
            None,
            |decisions, _| {
                decisions.insert("decision".into(), json!(2));
                mutated.set(true);
                Ok(())
            },
            &|| {
                if mutated.get() {
                    Err(PreparedPairError::Conflict)
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result, Err(PreparedPairError::Conflict));
        assert_eq!(fixture.tuple(), before);
        assert!(!fixture.r().exists());
    }

    // Insert inside pair_activity_tests; existing source_guard cut fixture is retained.
    #[test]
    fn source_guard_postpublish_failure_preserves_exact_after_without_false_rollback() {
        for absent in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let target = root.path().join("source-guard-postpublish.json");
            if !absent {
                std::fs::write(&target, b"{\"value\":0}").unwrap();
            }
            let before = read_config_pair_physical(&target).unwrap();
            let after = b"{\"value\":1}";
            let rejected = Cell::new(false);
            let evidence = std::cell::RefCell::new(None);
            let result =
                publish_coordination_bytes_with_validation(&target, &before, after, &|| {
                    if std::fs::read(&target).ok().as_deref() == Some(after.as_slice()) {
                        rejected.set(true);
                        *evidence.borrow_mut() =
                            Some((std::fs::read(&target).unwrap(), p22_stages(root.path())));
                        Err(PreparedPairError::Conflict)
                    } else {
                        Ok(())
                    }
                });
            assert_eq!(result, Err(PreparedPairError::Conflict));
            assert!(
                rejected.get(),
                "must reach a validation boundary after publication"
            );
            assert_eq!(
                (std::fs::read(&target).unwrap(), p22_stages(root.path())),
                evidence.borrow_mut().take().unwrap()
            );
            assert_eq!(std::fs::read(&target).unwrap(), after);
            // This fixture proves after-evidence retention at the adapter.
            // The driver must separately classify pending/conflict and forbid
            // complete until durable refs/reload/acks/receipt all succeed.
        }
    }

    #[test]
    fn source_guard_callback_adapter_noop_absent_replace_and_every_failure_cut() {
        for mode in 0..3 {
            let setup = |root: &Path| {
                let target = root.join("source-guard.json");
                if mode != 1 {
                    std::fs::write(&target, b"{\"value\":0}").unwrap();
                }
                let before = read_config_pair_physical(&target).unwrap();
                let after = if mode == 0 {
                    b"{\"value\":0}".as_slice()
                } else {
                    b"{\"value\":1}".as_slice()
                };
                (target, before, after)
            };
            let root = tempfile::tempdir().unwrap();
            let (target, before, after) = setup(root.path());
            let calls = Cell::new(0);
            publish_coordination_bytes_with_validation(&target, &before, after, &|| {
                calls.set(calls.get() + 1);
                Ok(())
            })
            .unwrap();
            assert!(calls.get() > 0);
            assert_eq!(std::fs::read(&target).unwrap(), after);
            assert!(p22_stages(root.path()).is_empty());
            for cut in 1..=calls.get() {
                let root = tempfile::tempdir().unwrap();
                let (target, before, after) = setup(root.path());
                let reached = Cell::new(0);
                let observed = std::cell::RefCell::new(None);
                let result =
                    publish_coordination_bytes_with_validation(&target, &before, after, &|| {
                        reached.set(reached.get() + 1);
                        if reached.get() == cut {
                            *observed.borrow_mut() =
                                Some((std::fs::read(&target).ok(), p22_stages(root.path())));
                            Err(PreparedPairError::Conflict)
                        } else {
                            Ok(())
                        }
                    });
                assert_eq!(result, Err(PreparedPairError::Conflict));
                assert_eq!(reached.get(), cut);
                assert_eq!(
                    (std::fs::read(&target).ok(), p22_stages(root.path())),
                    observed.borrow_mut().take().unwrap(),
                    "a rejected callback must forbid later publication and cleanup",
                );
            }
        }
    }

    #[test]
    fn p22_publisher_failure_cuts_preserve_staging_and_no_false_rollback() {
        for absent in [false, true] {
            let setup = |root: &Path| {
                let target = root.join("pair-side.json");
                if !absent {
                    std::fs::write(&target, b"{\"value\":0}").unwrap();
                }
                let before = read_config_pair_physical(&target).unwrap();
                (target, before)
            };
            let root = tempfile::tempdir().unwrap();
            let (target, before) = setup(root.path());
            let after = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
            let count = Cell::new(0);
            let allow = || {
                count.set(count.get() + 1);
                Ok(())
            };
            publish_prepared_pair_image_guarded(
                &target,
                &before,
                &after,
                &PairTargetGuard::new(&allow),
            )
            .unwrap();
            for cut in 1..=count.get() {
                let root = tempfile::tempdir().unwrap();
                let (target, before) = setup(root.path());
                let calls = Cell::new(0);
                let observed = std::cell::RefCell::new(None);
                let reject = || {
                    calls.set(calls.get() + 1);
                    if calls.get() == cut {
                        *observed.borrow_mut() =
                            Some((std::fs::read(&target).ok(), p22_stages(root.path())));
                        Err(PreparedPairError::Conflict)
                    } else {
                        Ok(())
                    }
                };
                let guard = PairTargetGuard::new(&reject);
                assert_eq!(
                    publish_prepared_pair_image_guarded(&target, &before, &after, &guard),
                    Err(PreparedPairError::Conflict)
                );
                assert_eq!(
                    (std::fs::read(&target).ok(), p22_stages(root.path())),
                    observed.borrow_mut().take().unwrap()
                );
                assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
                assert_eq!(calls.get(), cut);
            }
        }
    }

    #[test]
    fn p22_cleanup_rejection_retains_diagnostic_stage_and_observed_drift() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("pair-side.json");
        std::fs::write(&target, b"{\"value\":0}").unwrap();
        let before = read_config_pair_physical(&target).unwrap();
        let after = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
        let drifted = Cell::new(false);
        let validate = || {
            if drifted.get() {
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        };
        let guard = PairTargetGuard::new(&validate);
        let mut next = || next_staging_sequence(&STAGING_SEQUENCE);
        let write = |file: &mut std::fs::File, bytes: &[u8]| {
            file.write_all(bytes).unwrap();
            std::fs::write(&target, b"{\"value\":2}").unwrap();
            drifted.set(true);
            Ok(())
        };
        assert_eq!(
            publish_prepared_pair_image_with_io(
                &target,
                &before,
                &after,
                &guard,
                &|_, _| panic!("validation failure is already sticky"),
                &mut next,
                &write
            ),
            Err(PreparedPairError::Conflict)
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"value\":2}");
        assert_eq!(p22_stages(root.path()).len(), 1);
        assert_eq!(p22_stages(root.path())[0].1, b"{\"value\":1}");
    }

    #[test]
    fn p22_allowing_guard_ordinary_adapter_packet_and_floor_parity() {
        // Same physical target avoids comparing path-dependent packet identities.
        let fixture = Fixture::new(false);
        let request = fixture.request();
        let first = p22_prepare(&fixture, &request, &|| Ok(())).unwrap();
        let observed = p22_prepare(&fixture, &request, &|| Ok(())).unwrap();
        assert_eq!(first, observed);
        execute_reserved_config_pair_guarded(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            "operation-1",
            &|| Ok(()),
        )
        .unwrap();
        let guarded_after = fixture.tuple();
        execute_reserved_config_pair(&fixture.d(), &fixture.s(), &fixture.r(), "operation-1")
            .unwrap();
        assert_eq!(fixture.tuple(), guarded_after);
        assert_eq!(fixture.packet().timestamp_floor.as_deref(), Some(T1));
    }

    #[test]
    fn p22_ordinary_and_guarded_reservation_same_packet_and_tuples() {
        let fixture = Fixture::new(false);
        let request = fixture.request();
        let ordinary = prepare_and_reserve_config_pair(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            &request,
            None,
            |decisions, _| {
                decisions.insert("decision".into(), json!(1));
                Ok(())
            },
        )
        .unwrap();
        let packet_bytes = std::fs::read(fixture.r()).unwrap();
        let before = fixture.tuple();
        let guarded = prepare_and_reserve_config_pair_guarded(
            &fixture.d(),
            &fixture.s(),
            &fixture.r(),
            &request,
            None,
            |decisions, _| {
                decisions.insert("decision".into(), json!(1));
                Ok(())
            },
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(ordinary, guarded);
        assert_eq!(std::fs::read(fixture.r()).unwrap(), packet_bytes);
        assert_eq!(fixture.tuple(), before);
    }

    #[test]
    fn p22_primary_io_diagnostic_and_cleanup_refusal_both_retained() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("pair-side.json");
        std::fs::write(&target, b"{\"value\":0}").unwrap();
        let before = read_config_pair_physical(&target).unwrap();
        let after = PhysicalState::from_bytes(b"{\"value\":1}".to_vec());
        let io_failed = Cell::new(false);
        let validations = Cell::new(0);
        let validate = || {
            validations.set(validations.get() + 1);
            if io_failed.get() {
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        };
        let guard = PairTargetGuard::new(&validate);
        let diagnostics = std::cell::RefCell::new(Vec::new());
        let owned = std::cell::RefCell::new(None);
        let mut next = || next_staging_sequence(&STAGING_SEQUENCE);
        let write = |_: &mut std::fs::File, _: &[u8]| {
            let stages = p22_stages(root.path());
            assert_eq!(stages.len(), 1);
            *owned.borrow_mut() = Some(stages[0].0.clone());
            io_failed.set(true);
            Err(PreparedPairError::Io(
                "injected write failure after exclusive acquisition".into(),
            ))
        };
        let report = |primary: &PreparedPairError, refusal: &PreparedPairError| {
            assert!(matches!(primary, PreparedPairError::Io(_)));
            assert_eq!(refusal, &PreparedPairError::Conflict);
            diagnostics
                .borrow_mut()
                .push((primary.to_string(), refusal.to_string()));
        };
        assert_eq!(
            publish_prepared_pair_image_with_io(
                &target, &before, &after, &guard, &report, &mut next, &write
            ),
            Err(PreparedPairError::Conflict)
        );
        let calls = validations.get();
        assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
        assert_eq!(validations.get(), calls);
        let observed = diagnostics.into_inner();
        assert_eq!(observed.len(), 1);
        assert!(observed[0].0.starts_with("preparedPairIo:"));
        assert!(observed[0].1.starts_with("preparedPairConflict:"));
        assert_eq!(std::fs::read(&target).unwrap(), b"{\"value\":0}");
        assert_eq!(std::fs::read(owned.into_inner().unwrap()).unwrap(), b"");
    }

    #[cfg(not(windows))]
    fn p22_os_publish_attempt(stage: &Path, target: &Path) -> Result<(), std::io::Error> {
        std::fs::rename(stage, target)
    }

    #[cfg(windows)]
    fn p22_os_publish_attempt(stage: &Path, target: &Path) -> Result<(), std::io::Error> {
        replace_temp_config_once(&publish_path_wide(target), &publish_path_wide(stage))
    }

    #[test]
    fn p22_transient_error_then_guard_refusal_has_no_second_os_attempt() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("pair-side.json");
        std::fs::write(&target, b"before").unwrap();
        let stage = p22_filled_stage(&target, b"after");
        let attempts = Cell::new(0);
        let validations = Cell::new(0);
        let delays = std::cell::RefCell::new(Vec::new());
        let validate = || {
            validations.set(validations.get() + 1);
            if attempts.get() > 0 {
                Err(PreparedPairError::Conflict)
            } else {
                Ok(())
            }
        };
        let guard = PairTargetGuard::new(&validate);
        let mut attempt = || {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                Err(std::io::Error::from_raw_os_error(
                    TRANSIENT_PUBLISH_OS_ERRORS[1],
                ))
            } else {
                p22_os_publish_attempt(&stage, &target)
            }
        };
        let backoff = |duration| delays.borrow_mut().push(duration);
        assert_eq!(
            publish_temp_config_retry_with_attempt(&stage, &target, &guard, &mut attempt, &backoff,),
            Err(PreparedPairError::Conflict)
        );
        assert_eq!(attempts.get(), 1, "second OS primitive forbidden");
        assert_eq!(validations.get(), 3);
        assert_eq!(guard.check(), Err(PreparedPairError::Conflict));
        assert_eq!(validations.get(), 3);
        assert_eq!(
            delays.into_inner(),
            vec![std::time::Duration::from_millis(PUBLISH_BACKOFFS_MS[0])]
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"before");
        assert_eq!(std::fs::read(&stage).unwrap(), b"after");
    }

    #[test]
    fn p22_accepting_retry_and_ordinary_adapter_have_exact_success_parity() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("pair-side.json");
        for ordinary in [false, true] {
            std::fs::write(&target, b"before").unwrap();
            let stage = p22_filled_stage(&target, b"after");
            let attempts = Cell::new(0);
            let delays = std::cell::RefCell::new(Vec::new());
            let mut attempt = || {
                attempts.set(attempts.get() + 1);
                if attempts.get() == 1 {
                    Err(std::io::Error::from_raw_os_error(
                        TRANSIENT_PUBLISH_OS_ERRORS[1],
                    ))
                } else {
                    p22_os_publish_attempt(&stage, &target)
                }
            };
            let backoff = |duration| delays.borrow_mut().push(duration);
            if ordinary {
                assert_eq!(
                    publish_temp_config_with_attempt(&stage, &target, &mut attempt, &backoff,),
                    Ok(())
                );
            } else {
                assert_eq!(
                    publish_temp_config_retry_with_attempt(
                        &stage,
                        &target,
                        &PairTargetGuard::new(&|| Ok(())),
                        &mut attempt,
                        &backoff,
                    ),
                    Ok(())
                );
            }
            assert_eq!(attempts.get(), 2);
            assert_eq!(
                delays.into_inner(),
                vec![std::time::Duration::from_millis(PUBLISH_BACKOFFS_MS[0])]
            );
            assert_eq!(std::fs::read(&target).unwrap(), b"after");
            assert!(!stage.exists());
        }
        // Ordinary production adapter still chooses the same host OS primitive.
        std::fs::write(&target, b"before").unwrap();
        let stage = p22_filled_stage(&target, b"after");
        assert_eq!(publish_temp_config(&stage, &target), Ok(()));
        assert_eq!(std::fs::read(&target).unwrap(), b"after");
        assert!(!stage.exists());
    }
}

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
