use std::path::{Path, PathBuf};

pub const CANONICAL_WORKSPACE_DIR: &str = ".ac";
pub const WORKSPACE_DIR_NAMES: [&str; 1] = [CANONICAL_WORKSPACE_DIR];

pub fn canonical_workspace_dir_label() -> &'static str {
    CANONICAL_WORKSPACE_DIR
}

pub fn workspace_dir_label() -> &'static str {
    ".ac"
}

pub fn workspace_dir_for_project(project: &Path) -> PathBuf {
    project.join(CANONICAL_WORKSPACE_DIR)
}

pub fn existing_workspace_dir(project: &Path) -> Option<PathBuf> {
    let canonical = workspace_dir_for_project(project);
    if canonical.is_dir() {
        return Some(canonical);
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
    parts.iter().rposition(|part| is_workspace_dir_name(part))
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

fn same_existing_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

pub fn ensure_authoritative_workspace_dir(workspace_dir: &Path) -> Result<(), String> {
    let workspace_name = workspace_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "Project AC Root path '{}' has no valid directory name",
                workspace_dir.display()
            )
        })?;
    if !is_workspace_dir_name(workspace_name) {
        return Err(format!(
            "Project AC Root path '{}' is not a Project AC Root directory",
            workspace_dir.display()
        ));
    }

    let project_dir = workspace_dir.parent().ok_or_else(|| {
        format!(
            "Project AC Root path '{}' has no parent project directory",
            workspace_dir.display()
        )
    })?;
    let Some(authoritative) = existing_workspace_dir(project_dir) else {
        return Err(format!(
            "project '{}' has no Project AC Root directory",
            project_dir.display()
        ));
    };

    if same_existing_path(workspace_dir, &authoritative) {
        Ok(())
    } else {
        Err(format!(
            "Project AC Root '{}' rejected because authoritative Project AC Root '{}' exists",
            workspace_dir.display(),
            authoritative.display()
        ))
    }
}
