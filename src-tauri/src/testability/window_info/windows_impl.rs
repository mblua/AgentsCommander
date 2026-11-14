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

    // SAFETY: EnumWindows invokes enum_windows_proc synchronously with this lparam;
    // state is an initialized EnumState that stays live and is accessed only by the callback.
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
    // SAFETY: EnumWindows passes a live hwnd and the &mut EnumState given as lparam.
    let state = unsafe { &mut *(lparam as *mut EnumState) };
    // SAFETY: EnumWindows passes a live hwnd and the &mut EnumState given as lparam.
    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return 1;
    }

    // SAFETY: EnumWindows passes a live hwnd and the &mut EnumState given as lparam.
    let title = match unsafe { window_title(hwnd) } {
        Some(title) => title,
        None => return 1,
    };
    if title != state.expected_title {
        return 1;
    }

    let mut pid = 0u32;
    // SAFETY: EnumWindows passes a live hwnd and the &mut EnumState given as lparam.
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
    }
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

    // SAFETY: EnumWindows passes a live hwnd and the &mut EnumState given as lparam.
    let Some(raw_rect) = (unsafe { physical_window_rect(hwnd) }) else {
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
        // SAFETY: EnumWindows passes a live hwnd and the &mut EnumState given as lparam.
        maximized: unsafe { IsZoomed(hwnd) } != 0,
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
    // SAFETY: the caller passes a live window handle (this unsafe fn's contract).
    let dwm_ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&mut raw_rect as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    } == 0;
    if dwm_ok {
        return Some(raw_rect);
    }

    // SAFETY: the caller passes a live window handle (this unsafe fn's contract).
    if unsafe { GetWindowRect(hwnd, &mut raw_rect) } == 0 {
        return None;
    }
    Some(raw_rect)
}

unsafe fn window_title(hwnd: HWND) -> Option<String> {
    // SAFETY: the caller passes a live window handle (this unsafe fn's contract).
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize + 1];
    // SAFETY: the caller passes a live window handle (this unsafe fn's contract).
    let copied = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if copied <= 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..copied as usize]))
}

fn process_path(pid: u32) -> Option<PathBuf> {
    // SAFETY: OpenProcess takes a numeric PID; its null result is rejected below.
    // QueryFullProcessImageNameW receives that live handle, a 32768-u16 buffer and its
    // capacity in len; success bounds len to that capacity. CloseHandle runs once after use.
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
    crate::filesystem::directories::canonical_automation_executable(path)
}
