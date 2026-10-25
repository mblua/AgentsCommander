//! Loop event delivery; includes transport and path resolution, not loop rules.

use tauri::Emitter;

use crate::filesystem::canonical_loop_event_path;
use crate::loops::events::{LoopChange, LoopEventPayload, LoopEvents};

impl<R: tauri::Runtime> LoopEvents for tauri::AppHandle<R> {
    fn loop_changed(&self, change: LoopChange<'_>) {
        let LoopChange {
            project_path,
            changed_path,
            loop_id,
            kind,
            summary,
            message,
        } = change;
        let project_path = canonical_loop_event_path(project_path);
        let changed_path = canonical_loop_event_path(changed_path);
        let project_path_string = project_path.to_string_lossy().to_string();
        let changed_path_string = changed_path.to_string_lossy().to_string();
        let _ = self.emit(
            "loop_event",
            LoopEventPayload {
                kind: kind.to_string(),
                project_path: project_path_string.clone(),
                loop_id: loop_id.to_string(),
                summary,
                message,
            },
        );
        let _ = self.emit(
            "ac_project_refresh_requested",
            serde_json::json!({
                "id": uuid::Uuid::new_v4().to_string(),
                "projectPath": project_path_string,
                "changedPath": changed_path_string,
                "changedName": loop_id,
                "reason": format!("loop{}", capitalize_reason(kind)),
            }),
        );
    }
}

fn capitalize_reason(kind: &str) -> String {
    let mut chars = kind.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Changed".to_string(),
    }
}

#[cfg(test)]
mod tests;
