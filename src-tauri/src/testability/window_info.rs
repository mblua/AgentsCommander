use clap::Args;
#[cfg(target_os = "windows")]
use serde::Serialize;

#[derive(Debug, Args)]
pub struct WindowInfoArgs {}

#[cfg(target_os = "windows")]
pub fn execute(_args: WindowInfoArgs) -> i32 {
    match windows_impl::collect_window_info() {
        Ok(output) => {
            crate::cli_println!(
                "{}",
                serde_json::to_string(&output)
                    .unwrap_or_else(|_| { "{\"supported\":true,\"windows\":[]}".to_string() })
            );
            0
        }
        Err(e) => {
            eprintln!(
                "{}",
                serde_json::json!({
                    "supported": true,
                    "ok": false,
                    "error": "window_info_failed",
                    "message": e,
                })
            );
            1
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn execute(_args: WindowInfoArgs) -> i32 {
    crate::cli_println!(
        "{}",
        serde_json::json!({
            "supported": false,
            "error": "window_info_unsupported_on_platform",
        })
    );
    1
}

#[cfg(target_os = "windows")]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowInfoOutput {
    ok: bool,
    supported: bool,
    expected_title: String,
    current_exe: String,
    windows: Vec<WindowSnapshot>,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowSnapshot {
    title: String,
    pid: u32,
    process_path: String,
    hwnd: String,
    rect: WindowRect,
    maximized: bool,
}

#[cfg(target_os = "windows")]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    width: i32,
    height: i32,
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::{WindowInfoOutput, WindowRect, WindowSnapshot};
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible, IsZoomed,
    };

    pub fn collect_window_info() -> Result<WindowInfoOutput, String> {
        let expected_title = crate::config::profile::app_title().to_string();
        let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let canonical_current = canonical_for_compare(&current_exe);
        let mut state = EnumState {
            expected_title: expected_title.clone(),
            canonical_current,
            windows: Vec::new(),
        };

        let ok = unsafe {
            EnumWindows(
                Some(enum_windows_proc),
                (&mut state as *mut EnumState) as LPARAM,
            )
        };
        if ok == 0 {
            return Err("EnumWindows returned failure".to_string());
        }

        Ok(WindowInfoOutput {
            ok: true,
            supported: true,
            expected_title,
            current_exe: current_exe.to_string_lossy().into_owned(),
            windows: state.windows,
        })
    }

    struct EnumState {
        expected_title: String,
        canonical_current: PathBuf,
        windows: Vec<WindowSnapshot>,
    }

    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
        let state = &mut *(lparam as *mut EnumState);
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }

        let title = match window_title(hwnd) {
            Some(title) => title,
            None => return 1,
        };
        if title != state.expected_title {
            return 1;
        }

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return 1;
        }

        let Some(process_path) = process_path(pid) else {
            return 1;
        };
        let canonical_process = canonical_for_compare(&process_path);
        if canonical_process != state.canonical_current {
            return 1;
        }

        let Some(raw_rect) = physical_window_rect(hwnd) else {
            return 1;
        };

        state.windows.push(WindowSnapshot {
            title,
            pid,
            process_path: process_path.to_string_lossy().into_owned(),
            hwnd: format!("0x{:016X}", hwnd as usize),
            rect: WindowRect {
                left: raw_rect.left,
                top: raw_rect.top,
                right: raw_rect.right,
                bottom: raw_rect.bottom,
                width: raw_rect.right - raw_rect.left,
                height: raw_rect.bottom - raw_rect.top,
            },
            maximized: IsZoomed(hwnd) != 0,
        });

        1
    }

    unsafe fn physical_window_rect(hwnd: HWND) -> Option<RECT> {
        let mut raw_rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let dwm_ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut raw_rect as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        ) == 0;
        if dwm_ok {
            return Some(raw_rect);
        }

        if GetWindowRect(hwnd, &mut raw_rect) == 0 {
            return None;
        }
        Some(raw_rect)
    }

    unsafe fn window_title(hwnd: HWND) -> Option<String> {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if copied <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..copied as usize]))
    }

    fn process_path(pid: u32) -> Option<PathBuf> {
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

    fn canonical_for_compare(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }
}
