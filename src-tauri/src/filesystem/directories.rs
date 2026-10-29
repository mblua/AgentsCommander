//! Filesystem probes for transcript paths, peer directories and agent programs.

use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn canonical_transcript_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn canonical_peer_cwd(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn agent_program_is_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn registered_project_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn launch_root_metadata(path: &Path) -> io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn canonical_launch_root(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn canonical_automation_executable(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn canonical_session_guard_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn canonical_explorer_target(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn explorer_target_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn picker_default_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn codex_sessions_root_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn agent_parent_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn local_template_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn canonical_context_repo(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn context_repo_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn persisted_session_cwd_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn canonical_session_project_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn retained_prompt_cwd(path: &Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(path).ok()?;
    if !canonical.is_dir() {
        return None;
    }
    Some(canonical)
}

pub(crate) fn canonical_carrier_cwd(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn restore_cwd_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn agent_search_directory_exists(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn resume_host_directory_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn credential_source_is_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn canonical_container_mount(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn container_mount_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn ensure_experiment_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn create_experiment_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

pub(crate) fn experiment_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn experiment_path_is_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn experiment_path_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn visit_experiment_entries<B>(
    root: &Path,
    mut visit: impl FnMut(io::Result<(PathBuf, std::ffi::OsString)>) -> std::ops::ControlFlow<B>,
) -> io::Result<std::ops::ControlFlow<B>> {
    for entry in std::fs::read_dir(root)? {
        if let std::ops::ControlFlow::Break(value) =
            visit(entry.map(|entry| (entry.path(), entry.file_name())))
        {
            return Ok(std::ops::ControlFlow::Break(value));
        }
    }
    Ok(std::ops::ControlFlow::Continue(()))
}

pub(crate) fn collect_experiment_link_paths(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_experiment_link_paths_into(root, &mut out);
    out
}

fn collect_experiment_link_paths_into(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(meta) = std::fs::symlink_metadata(root) else {
        return;
    };
    if crate::path_identity::is_link_or_reparse(&meta) {
        out.push(root.to_path_buf());
        return;
    }
    if !meta.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        collect_experiment_link_paths_into(&entry.path(), out);
    }
}

#[derive(Debug)]
pub(crate) enum ExperimentPathFailure {
    Link { path: PathBuf },
    Inspect { path: PathBuf, error: io::Error },
}

pub(crate) fn inspect_experiment_path_chain(path: &Path) -> Result<(), ExperimentPathFailure> {
    use std::path::Component;
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(ExperimentPathFailure::Inspect {
                    path: current,
                    error,
                })
            }
        };
        if crate::path_identity::is_link_or_reparse(&metadata) {
            return Err(ExperimentPathFailure::Link { path: current });
        }
    }
    Ok(())
}

pub(crate) fn experiment_path_is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| crate::path_identity::is_link_or_reparse(&metadata))
        .unwrap_or(false)
}

pub(crate) fn ensure_cli_outbox(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn cli_outbox_artifact_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn cli_agent_root_is_dir(path: &Path) -> bool {
    path.is_dir()
}

pub(crate) fn ensure_cli_session_requests(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn ensure_project_refresh_requests(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn ensure_harness_logs(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn canonical_created_agent(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn visit_cli_discovery_entries<B>(
    root: &Path,
    mut visit: impl FnMut(io::Result<(PathBuf, std::ffi::OsString)>) -> std::ops::ControlFlow<B>,
) -> io::Result<std::ops::ControlFlow<B>> {
    for entry in std::fs::read_dir(root)? {
        if let std::ops::ControlFlow::Break(value) =
            visit(entry.map(|entry| (entry.path(), entry.file_name())))
        {
            return Ok(std::ops::ControlFlow::Break(value));
        }
    }
    Ok(std::ops::ControlFlow::Continue(()))
}

pub(crate) fn cli_discovery_is_dir(path: &Path) -> bool {
    path.is_dir()
}
pub(crate) fn cli_discovery_is_file(path: &Path) -> bool {
    path.is_file()
}
pub(crate) fn cli_discovery_exists(path: &Path) -> bool {
    path.exists()
}
pub(crate) fn canonical_cli_discovery_path(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}
pub(crate) fn ensure_cli_peer_mailbox(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn create_cli_outbox(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

pub(crate) fn ensure_container_credential_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(windows)]
pub(crate) fn ensure_git_guard_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn ensure_pty_response_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn test_reset_candidate_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn remove_test_reset_candidate(path: &Path) -> io::Result<()> {
    std::fs::remove_dir_all(path)
}

pub(crate) fn inspect_test_reset_candidate(path: &Path) -> io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn canonical_test_reset_path(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn ensure_ui_automation_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)
}

pub(crate) fn ui_automation_path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn remove_ui_automation_directory(path: &Path) -> io::Result<()> {
    std::fs::remove_dir_all(path)
}

pub(crate) fn retained_prompt_directory_metadata(
    path: &Path,
) -> std::io::Result<std::fs::Metadata> {
    std::fs::metadata(path)
}
