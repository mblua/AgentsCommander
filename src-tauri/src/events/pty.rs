//! PTY event transport and task scheduling at the Tauri edge.

use crate::pty::output::{PtyOutputEmitError, PtyOutputTarget, PtyOutputTask};
use tauri::{AppHandle, Emitter};

/// Emits once per attached label and schedules flushes on the Tauri-owned runtime.
pub(crate) fn output_target<R: tauri::Runtime>(app: AppHandle<R>) -> PtyOutputTarget {
    PtyOutputTarget::new(
        move |labels, payload| {
            let mut result = Ok(());
            for label in labels {
                if app
                    .emit_to(label.as_str(), "pty_output", payload.clone())
                    .is_err()
                {
                    result = Err(PtyOutputEmitError::Emit);
                }
            }
            result
        },
        spawn_output_task,
    )
}

fn spawn_output_task(task: PtyOutputTask) {
    tauri::async_runtime::spawn(task);
}

#[cfg(test)]
mod tests;
