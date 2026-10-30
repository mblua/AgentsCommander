use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

pub(super) fn sorted_labels(labels: &HashSet<String>) -> Vec<String> {
    let mut labels: Vec<String> = labels.iter().cloned().collect();
    labels.sort();
    labels
}

pub(crate) fn available_window_labels(app: &AppHandle) -> Vec<String> {
    let mut labels: Vec<String> = app.webview_windows().keys().cloned().collect();
    labels.sort();
    labels
}

pub(super) fn current_exe_path_string() -> String {
    std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| String::new())
}

pub(super) fn paths_equal_for_compare(a: &Path, b: &Path) -> bool {
    let a = canonical_for_compare(a);
    let b = canonical_for_compare(b);
    #[cfg(target_os = "windows")]
    {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    }
    #[cfg(not(target_os = "windows"))]
    {
        a == b
    }
}

pub(super) fn canonical_for_compare(path: &Path) -> PathBuf {
    crate::filesystem::directories::canonical_automation_executable(path)
}

pub(super) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
pub fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, FALSE, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if handle.is_null() {
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code: u32 = 0;
        let got_code = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        got_code != 0 && code == STILL_ACTIVE as u32
    }
}

#[cfg(unix)]
pub fn pid_is_alive(pid: u32) -> bool {
    // `kill` reads a negative pid as a process group and -1 as "every process
    // I may signal", so a blind `u32 as pid_t` cast would turn `u32::MAX` into
    // `kill(-1, 0)` and report a live process. Reject 0 and values a `pid_t`
    // cannot hold before converting.
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid == 0 {
        return false;
    }
    // SAFETY: `pid` is a positive `pid_t` (checked above) and signal 0 is
    // never delivered; `kill` performs only the existence and permission
    // checks, so this call cannot affect any process.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return !pid_is_zombie(pid);
    }
    // Mirrors the Windows branch: ACCESS_DENIED means "alive but not ours",
    // so EPERM is alive and every other errno (ESRCH et al.) is dead.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// #2382: Linux keeps a terminated child as a zombie until its parent waits.
/// `kill` reports a zombie as existing, but the automation session it names
/// can no longer answer, so treat it as dead - matching the Windows branch,
/// which does not report an exited process as alive either.
#[cfg(target_os = "linux")]
pub(super) fn pid_is_zombie(pid: libc::pid_t) -> bool {
    // `/proc/<pid>/stat` is `pid (comm) state ...` and `comm` may contain
    // spaces and parentheses, so read the state after the LAST ')'.
    crate::filesystem::processes::read_process_stat(pid)
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .map(|(_, rest)| rest.trim_start().starts_with('Z'))
        })
        .unwrap_or(false)
}

/// macOS and the BSDs have no `/proc`; plain `kill(pid, 0)` existence is the
/// whole answer there.
#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn pid_is_zombie(_pid: libc::pid_t) -> bool {
    false
}

#[cfg(not(any(unix, windows)))]
pub fn pid_is_alive(_pid: u32) -> bool {
    true
}

#[cfg(target_os = "windows")]
pub(super) fn process_path(pid: u32) -> Option<PathBuf> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len);
        let _ = CloseHandle(handle);
        if ok == 0 || len == 0 {
            return None;
        }
        Some(PathBuf::from(String::from_utf16_lossy(
            &buf[..len as usize],
        )))
    }
}
