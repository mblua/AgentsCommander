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
mod windows_impl;
