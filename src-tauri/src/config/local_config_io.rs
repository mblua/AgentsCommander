use serde_json::{Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn local_config_write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub fn update_config_json_object<F>(
    path: &Path,
    allow_create: bool,
    mutate: F,
) -> Result<Value, String>
where
    F: FnOnce(&mut Map<String, Value>) -> Result<(), String>,
{
    let _guard = local_config_write_lock()
        .lock()
        .map_err(|_| "Local config write lock is poisoned".to_string())?;

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

    let parent = path
        .parent()
        .ok_or_else(|| format!("Local config {} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
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

    Ok(root)
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
    use super::{format_publish_error, is_transient_publish_error, update_config_json_object};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

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
        let roots = [manifest.join("src/config"), manifest.join("src/commands")];
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
}
