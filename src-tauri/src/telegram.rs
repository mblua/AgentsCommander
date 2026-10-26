pub(crate) mod api;
pub(crate) mod bridge;
mod claude_watcher;
mod codex_watcher;
mod jsonl_kernel;
pub mod manager;
pub mod output;
pub(crate) mod redact;
pub(crate) mod types;

pub(crate) fn existing_codex_sessions_root(home: &std::path::Path) -> Option<std::path::PathBuf> {
    let root = home.join(".codex").join("sessions");
    if crate::filesystem::directories::codex_sessions_root_is_dir(&root) {
        Some(root)
    } else {
        None
    }
}
