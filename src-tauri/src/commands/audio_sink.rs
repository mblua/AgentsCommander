//! Sound sink check behind the sidebar sound button.

use crate::process::AudioSinkStatus;

/// Whether WebKitGTK can find its sound sink; always `"ok"` off Linux.
#[tauri::command]
pub(crate) async fn get_audio_sink_status() -> AudioSinkStatus {
    crate::process::audio_sink_status(crate::config::environment::in_appimage()).await
}

/// Route `get_audio_sink_status` to its handler; every other command goes to
/// `fallback` exactly once and keeps its result. The caller already observed the invoke.
pub(crate) fn dispatch<R, F>(invoke: tauri::ipc::Invoke<R>, fallback: &F) -> bool
where
    R: tauri::Runtime,
    F: Fn(tauri::ipc::Invoke<R>) -> bool,
{
    let audio_sink: fn(tauri::ipc::Invoke<R>) -> bool =
        tauri::generate_handler![get_audio_sink_status];
    match invoke.message.command() {
        "get_audio_sink_status" => audio_sink(invoke),
        _ => fallback(invoke),
    }
}

#[cfg(test)]
mod tests;
