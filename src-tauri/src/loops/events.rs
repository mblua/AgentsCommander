//! Loop change events, and the payload the frontend listens for.
//!
//! #1252: this module exists so `loops::scheduler` never has to reach up into the
//! Tauri command layer to announce a Loop transition. The command surface and the
//! scheduler both depend downward on this module, which owns the emitter, so neither
//! depends on the other to emit. It lives under `loops` rather than in
//! `config::loops` because `config::loops` is TOML persistence and is itself inside
//! the crate's 89 module knot: putting an IPC emitter there would trade one layering
//! inversion for another and move code into the cycle instead of out of one.

use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::config::loops::AcLoopSummary;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopEventPayload {
    pub kind: String,
    pub project_path: String,
    pub loop_id: String,
    pub summary: Option<AcLoopSummary>,
    pub message: Option<String>,
}

pub fn emit_loop_change(
    app: &AppHandle,
    project_path: &Path,
    changed_path: &Path,
    loop_id: &str,
    kind: &str,
    summary: Option<AcLoopSummary>,
    message: Option<String>,
) {
    let project_path = std::fs::canonicalize(project_path).unwrap_or_else(|_| project_path.into());
    let changed_path = std::fs::canonicalize(changed_path).unwrap_or_else(|_| changed_path.into());
    let project_path_string = project_path.to_string_lossy().to_string();
    let changed_path_string = changed_path.to_string_lossy().to_string();
    let _ = app.emit(
        "loop_event",
        LoopEventPayload {
            kind: kind.to_string(),
            project_path: project_path_string.clone(),
            loop_id: loop_id.to_string(),
            summary,
            message,
        },
    );
    let _ = app.emit(
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

fn capitalize_reason(kind: &str) -> String {
    let mut chars = kind.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Changed".to_string(),
    }
}
