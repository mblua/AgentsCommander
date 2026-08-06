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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// The file's source with everything from the first `#[cfg(test)]` removed, so a
    /// test that names the forbidden path (this one) cannot report itself.
    fn production_source(path: &Path) -> String {
        let text = std::fs::read_to_string(path).expect("read Rust source");
        match text.find("#[cfg(test)]") {
            Some(cut) => text[..cut].to_string(),
            None => text,
        }
    }

    fn rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read Rust source directory") {
            let entry = entry.expect("read Rust source entry");
            let path = entry.path();
            if path.is_dir() {
                rust_sources(&path, files);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }

    /// #1252: `loops::scheduler` used to call `crate::commands::loops::emit_loop_change`,
    /// which made the domain depend on the Tauri command surface and put the two modules
    /// in a 2 member cycle. The emitter moved to this module so both sides depend
    /// downward. The match is on `commands::loops` rather than on the fully qualified
    /// path so that `super::super::` and bare `use` spellings are caught too: an arc that
    /// is invisible to the detector is still a dependency.
    ///
    /// The cost of matching a bare substring is that it also fires on prose. Nothing in
    /// production source under `src/loops/` may spell that path, comments included, which
    /// is why this module's own header describes the Tauri command layer in words. Say it
    /// here instead: everything below `#[cfg(test)]` is cut before the scan.
    #[test]
    fn loops_production_sources_do_not_reference_the_loops_command_module() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/loops");
        let mut files = Vec::new();
        rust_sources(&root, &mut files);
        assert!(
            !files.is_empty(),
            "no Rust sources found under src/loops; the scan proves nothing"
        );

        let offenders: Vec<String> = files
            .iter()
            .filter(|path| production_source(path).contains("commands::loops"))
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect();

        assert!(
            offenders.is_empty(),
            "src/loops must not reference commands::loops.\n\
             \n\
             WHY: `loops` is domain logic and `commands` is the Tauri IPC surface. \
             The domain must not depend on the surface it is announced through. \
             Issue #1252 removed the one call that did, \
             `crate::commands::loops::emit_loop_change` in loops/scheduler.rs, \
             because it put those two modules in a dependency cycle: \
             commands::loops needs LoopScheduler, so the scheduler must not need \
             commands::loops back. Any reference from here rebuilds that cycle.\n\
             \n\
             INSTEAD: emit Loop events through \
             `crate::loops::events::emit_loop_change`, which the command layer and \
             the scheduler both depend on downward. If you need something from \
             commands::loops that is not an event, it belongs in a module below \
             both of them, never above.\n\
             \n\
             OFFENDING FILES: {}",
            offenders.join(", ")
        );
    }
}
