use std::path::{Path, PathBuf};

pub const CANONICAL_WORKSPACE_DIR: &str = ".ac";
pub const LEGACY_WORKSPACE_DIR: &str = ".ac-new";
pub const WORKSPACE_DIR_NAMES: [&str; 2] = [CANONICAL_WORKSPACE_DIR, LEGACY_WORKSPACE_DIR];

pub fn canonical_workspace_dir_label() -> &'static str {
    CANONICAL_WORKSPACE_DIR
}

pub fn workspace_dir_label() -> &'static str {
    ".ac or legacy .ac-new"
}

pub fn workspace_dir_for_project(project: &Path) -> PathBuf {
    project.join(CANONICAL_WORKSPACE_DIR)
}

pub fn legacy_workspace_dir_for_project(project: &Path) -> PathBuf {
    project.join(LEGACY_WORKSPACE_DIR)
}

pub fn existing_workspace_dir(project: &Path) -> Option<PathBuf> {
    let canonical = workspace_dir_for_project(project);
    if canonical.is_dir() {
        return Some(canonical);
    }

    let legacy = legacy_workspace_dir_for_project(project);
    if legacy.is_dir() {
        return Some(legacy);
    }

    None
}

pub fn has_workspace_dir(project: &Path) -> bool {
    existing_workspace_dir(project).is_some()
}

pub fn is_workspace_dir_name(name: &str) -> bool {
    WORKSPACE_DIR_NAMES
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

pub fn find_workspace_segment(parts: &[&str]) -> Option<usize> {
    parts
        .iter()
        .rposition(|part| is_workspace_dir_name(part))
}

pub fn find_workspace_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| {
            ancestor
                .file_name()
                .and_then(|name| name.to_str())
                .map(is_workspace_dir_name)
                .unwrap_or(false)
        })
        .map(Path::to_path_buf)
}
