use serde_json::{Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

fn local_config_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
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
struct ConfigFileWriteLock {
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
) -> Result<ConfigFileWriteLock, String> {
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

    match std::fs::symlink_metadata(&lock_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!(
                "Local config write lock '{}' must be a regular non-symlink file",
                lock_path.display()
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "Failed to inspect local config write lock '{}': {}",
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
        .open(&lock_path)
        .map_err(|e| {
            format!(
                "Failed to open local config write lock '{}': {}",
                lock_path.display(),
                e
            )
        })?;
    if !file
        .metadata()
        .map_err(|e| {
            format!(
                "Failed to inspect opened local config write lock '{}': {}",
                lock_path.display(),
                e
            )
        })?
        .is_file()
    {
        return Err(format!(
            "Local config write lock '{}' must be a regular file",
            lock_path.display()
        ));
    }

    let started = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() >= timeout => {
                return Err(format!(
                    "configLockTimeout: timed out after {} ms waiting for local config write lock '{}'",
                    timeout.as_millis(),
                    lock_path.display()
                ));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                let remaining = timeout.saturating_sub(started.elapsed());
                std::thread::sleep(CONFIG_LOCK_POLL_INTERVAL.min(remaining));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(format!(
                    "Failed to acquire local config write lock '{}': {}",
                    lock_path.display(),
                    e
                ));
            }
        }
    }

    Ok(ConfigFileWriteLock { _file: file })
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
    let _guard = local_config_write_lock()
        .lock()
        .map_err(|_| "Local config write lock is poisoned".to_string())?;

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
    let _guard = local_config_write_lock()
        .lock()
        .map_err(|_| "Local config write lock is poisoned".to_string())?;

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

    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    let path_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let tmp_wide: Vec<u16> = tmp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

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
        acquire_config_file_write_lock, config_lock_path, format_publish_error,
        is_transient_publish_error, update_config_json_object,
        update_config_json_object_with_publish, write_file_atomic_with_publish,
        CONFIG_LOCK_TIMEOUT,
    };
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
}
