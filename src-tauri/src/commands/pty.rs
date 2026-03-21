use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tauri::State;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::telegram::manager::TypingFlagMap;

#[tauri::command]
pub fn pty_write(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    typing_flags: State<'_, TypingFlagMap>,
    session_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

    // Signal typing state to the Telegram bridge (if any).
    // Enter (\r, \n) or control chars (Ctrl+C=0x03, Ctrl+D=0x04) clear the flag.
    // Regular keystrokes set it to suppress bridge output while typing.
    if let Ok(flags) = typing_flags.lock() {
        if let Some(flag) = flags.get(&uuid) {
            let has_submit = data.iter().any(|&b| b == b'\r' || b == b'\n' || b == 0x03 || b == 0x04);
            flag.store(!has_submit, Ordering::Relaxed);
        }
    }

    pty_mgr
        .lock()
        .unwrap()
        .write(uuid, &data)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_resize(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    pty_mgr
        .lock()
        .unwrap()
        .resize(uuid, cols, rows)
        .map_err(|e| e.to_string())
}
