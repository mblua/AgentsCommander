//! Disk operations for repository roles, prompt suites and capture catalogs.

use std::io;
use std::path::{Path, PathBuf};

pub(crate) fn read_repository_role(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn prompt_suite_is_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn read_prompt_suite(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn canonical_prompt_suite_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn capture_catalog_is_file(path: &Path) -> bool {
    path.is_file()
}

pub(crate) fn read_local_role_template(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn local_template_metadata(path: &Path) -> io::Result<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
}

pub(crate) fn local_template_has_entries(path: &Path) -> io::Result<bool> {
    std::fs::read_dir(path).map(|mut entries| entries.next().is_some())
}

pub(crate) fn read_configured_context(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn configured_context_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn visit_local_template_entries<B>(
    dir: &Path,
    mut visit: impl FnMut(
        io::Result<(PathBuf, std::ffi::OsString, io::Result<std::fs::Metadata>)>,
    ) -> std::ops::ControlFlow<B>,
) -> io::Result<std::ops::ControlFlow<B>> {
    for entry in std::fs::read_dir(dir)? {
        let item = entry.map(|entry| (entry.path(), entry.file_name(), entry.metadata()));
        if let std::ops::ControlFlow::Break(value) = visit(item) {
            return Ok(std::ops::ControlFlow::Break(value));
        }
    }
    Ok(std::ops::ControlFlow::Continue(()))
}

pub(crate) fn loop_prompt_metadata(path: &Path) -> io::Result<std::fs::Metadata> {
    std::fs::metadata(path)
}

pub(crate) fn read_loop_prompt(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn canonical_experiment_role(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub(crate) fn experiment_role_metadata(path: &Path) -> io::Result<std::fs::Metadata> {
    std::fs::metadata(path)
}

pub(crate) fn read_experiment_role(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn read_claude_resume_wrapper(path: &Path) -> Option<Vec<u8>> {
    // Cap read at 64 KiB; real wrappers are < 1 KiB. Refusing larger
    // files protects against accidentally treating an exe-renamed-as-cmd
    // as a wrapper.
    const MAX: u64 = 64 * 1024;
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX {
        return None;
    }
    std::fs::read(path).ok()
}

pub(crate) fn read_experiment_document(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn write_experiment_document(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

#[derive(Debug)]
pub(crate) enum ExperimentArtifactFailure {
    NoParent { path: PathBuf },
    PathInspection(super::directories::ExperimentPathFailure),
    CreateDirectory { path: PathBuf, error: io::Error },
    Create { path: PathBuf, error: io::Error },
    Write { path: PathBuf, error: io::Error },
    Flush { path: PathBuf, error: io::Error },
    Replace { path: PathBuf, error: io::Error },
}

pub(crate) fn create_experiment_artifact(
    path: &Path,
    bytes: &[u8],
) -> Result<(), ExperimentArtifactFailure> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| ExperimentArtifactFailure::Create {
            path: path.to_owned(),
            error,
        })?;
    file.write_all(bytes)
        .map_err(|error| ExperimentArtifactFailure::Write {
            path: path.to_owned(),
            error,
        })
}

#[cfg(test)]
fn replace_fault(path: &Path) -> io::Result<()> {
    super::tests::issue_3298_replace_fault(path)
}

#[cfg(not(test))]
fn replace_fault(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub(crate) fn replace_experiment_artifact(
    path: &Path,
    bytes: &[u8],
) -> Result<(), ExperimentArtifactFailure> {
    use super::directories::inspect_experiment_path_chain;
    use std::io::Write;
    use ExperimentArtifactFailure as Failure;
    inspect_experiment_path_chain(path).map_err(Failure::PathInspection)?;
    let parent = path.parent().ok_or_else(|| Failure::NoParent {
        path: path.to_owned(),
    })?;
    inspect_experiment_path_chain(parent).map_err(Failure::PathInspection)?;
    std::fs::create_dir_all(parent).map_err(|error| Failure::CreateDirectory {
        path: parent.to_owned(),
        error,
    })?;
    let tmp = parent.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|error| Failure::Create {
                path: tmp.clone(),
                error,
            })?;
        replace_fault(path)
            .and_then(|()| file.write_all(bytes))
            .map_err(|error| Failure::Write {
                path: tmp.clone(),
                error,
            })?;
        replace_fault(path)
            .and_then(|()| file.flush())
            .map_err(|error| Failure::Flush {
                path: tmp.clone(),
                error,
            })?;
    }
    match replace_fault(path).and_then(|()| std::fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            replace_fault(path)
                .and_then(|()| std::fs::remove_file(path))
                .map_err(|error| Failure::Replace {
                    path: path.to_owned(),
                    error,
                })?;
            replace_fault(path)
                .and_then(|()| std::fs::rename(&tmp, path))
                .map_err(|error| {
                    let _ = std::fs::remove_file(&tmp);
                    Failure::Replace {
                        path: path.to_owned(),
                        error,
                    }
                })
        }
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            Err(Failure::Replace {
                path: path.to_owned(),
                error,
            })
        }
    }
}

pub(crate) fn read_cli_outbox_text(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn write_cli_outbox_message(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

pub(crate) fn write_cli_session_document(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

pub(crate) fn read_cli_session_result(path: &Path) -> io::Result<String> {
    std::fs::read_to_string(path)
}

pub(crate) fn remove_cli_session_artifact(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path)
}

pub(crate) fn write_refresh_request_stage(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

pub(crate) fn finalize_refresh_request(tmp: &Path, dst: &Path) -> io::Result<()> {
    std::fs::rename(tmp, dst).inspect_err(|_| {
        let _ = std::fs::remove_file(tmp);
    })
}

pub(crate) fn append_harness_audit(path: &Path, line: &str) -> io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", line)
}

pub(crate) fn write_window_screenshot(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

pub(crate) fn read_cli_room_task(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}

pub(crate) fn read_cli_message_archive_source(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}
