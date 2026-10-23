use super::{ordinary_publish_result, PairTargetGuard, PreparedPairError};
use std::path::Path;

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
pub(super) const TRANSIENT_PUBLISH_OS_ERRORS: [i32; 4] = [1175, 32, 33, 5];

/// #537 - publish attempt budget and backoff schedule, duplicated from the
/// reviewed pattern in sessions_persistence::rename_with_retry (#280) per the
/// house "a little duplication over a premature abstraction" rule. Four
/// attempts so all three backoff entries are used; the terminal attempt has no
/// backoff. Worst-case added latency on a contended publish is the sum, 260 ms.
/// Like #280 this uses std::thread::sleep, which briefly blocks the calling
/// tokio worker; update_config_json_object is already fully synchronous, so
/// this only enlarges an existing blocking window rather than adding a new one.
pub(super) const PUBLISH_ATTEMPTS: u32 = 4;
pub(super) const PUBLISH_BACKOFFS_MS: [u64; 3] = [10, 50, 200];

/// #537 - true when a publish error looks like a transient lock collision we
/// should retry rather than a permanent failure. See TRANSIENT_PUBLISH_OS_ERRORS.
pub(super) fn is_transient_publish_error(err: &std::io::Error) -> bool {
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
pub(super) fn format_publish_error(path: &Path, tmp_path: &Path, err: &std::io::Error) -> String {
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
pub(in crate::config) fn publish_temp_config(tmp_path: &Path, path: &Path) -> Result<(), String> {
    ordinary_publish_result(publish_temp_config_guarded(
        tmp_path,
        path,
        &PairTargetGuard::new(&|| Ok(())),
    ))
}

#[cfg(not(windows))]
pub(super) fn publish_temp_config_guarded(
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
pub(super) fn publish_temp_config_retry_with_attempt(
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
                        "[config] publish succeeded after retry: path={} \
                         attempt={}/{} duration={:?}",
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
pub(super) fn publish_path_wide(path: &Path) -> Vec<u16> {
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
                let last = s.encode_wide().last();
                if matches!(last, Some(u) if u == u16::from(b'.') || u == u16::from(b' ')) {
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
pub(in crate::config) fn publish_temp_config(tmp_path: &Path, path: &Path) -> Result<(), String> {
    ordinary_publish_result(publish_temp_config_guarded(
        tmp_path,
        path,
        &PairTargetGuard::new(&|| Ok(())),
    ))
}

#[cfg(windows)]
pub(super) fn publish_temp_config_guarded(
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
pub(super) fn publish_temp_config_retry_with_attempt(
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
                    "[config] ReplaceFileW succeeded after retry: path={} \
                     attempt={}/{} duration={:?}",
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

#[cfg(windows)]
pub(super) fn replace_temp_config_once(
    path_wide: &[u16],
    tmp_wide: &[u16],
) -> Result<(), std::io::Error> {
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
pub(super) fn publish_temp_config_with_attempt(
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
