use super::creation_trace::{Outcome, Reason, Stage, StageGuard};
use crate::process::git::mutation::{self, CloneRepositoryError, ResetCloneError};
use std::path::{Path, PathBuf};

/// Async git clone with CREATE_NO_WINDOW on Windows.
pub(super) async fn git_clone_async(
    url: &str,
    target: &Path,
    parent: Option<&StageGuard<'_>>,
) -> Result<(), String> {
    let git_target = git_cli_path(target);
    let clone_work = mutation::clone_repository(url, &git_target);
    let mut clone_stage = parent.map(|p| p.child(Stage::CloneProcess));
    let output = clone_work.await.map_err(|error| {
        if let Some(stage) = &mut clone_stage {
            match &error {
                CloneRepositoryError::Spawn(_) => {
                    stage.finish(Outcome::Error, Some(Reason::Failed));
                }
                CloneRepositoryError::TimedOut { .. } => {
                    stage.finish(Outcome::Timeout, Some(Reason::CloneTimeout));
                }
            }
        }
        error.to_string()
    })?;

    if !output.status.success() {
        if let Some(stage) = &mut clone_stage {
            stage.finish(Outcome::Error, Some(Reason::Failed));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        // Cap error message length to avoid sending huge progress output to frontend
        let capped = if trimmed.len() > 512 {
            &trimmed[..512]
        } else {
            trimmed
        };
        return Err(format!("git clone failed: {}", capped));
    }

    if let Some(stage) = &mut clone_stage {
        stage.finish(Outcome::Ok, None);
    }

    if !crate::repos::clone_index_exists(&target.join(".git").join("index")) {
        log::warn!(
            "[entity_creation] .git/index missing after clone for {}, running fallback git reset",
            url
        );
        let reset_work = mutation::reset_clone_index(&git_target);
        let mut reset_stage = parent.map(|p| p.child(Stage::ResetProcess));
        match reset_work.await {
            Ok(output) if output.status.success() => {
                if let Some(stage) = &mut reset_stage {
                    stage.finish(Outcome::Ok, None);
                }
            }
            Ok(output) => {
                if let Some(stage) = &mut reset_stage {
                    stage.finish(Outcome::Error, Some(Reason::ResetFailed));
                }
                log::warn!(
                    "[entity_creation] fallback git reset failed for {}: {}",
                    url,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Err(ResetCloneError::Spawn(e)) => {
                if let Some(stage) = &mut reset_stage {
                    stage.finish(Outcome::Error, Some(Reason::ResetFailed));
                }
                log::warn!(
                    "[entity_creation] failed to spawn fallback git reset for {}: {}",
                    url,
                    e
                );
            }
            Err(ResetCloneError::TimedOut { seconds }) => {
                if let Some(stage) = &mut reset_stage {
                    stage.finish(Outcome::Timeout, Some(Reason::ResetTimeout));
                }
                log::warn!(
                    "[entity_creation] fallback git reset timed out after {} seconds for {}",
                    seconds,
                    url
                );
            }
        }
    }

    Ok(())
}

#[cfg(windows)]
pub(super) fn git_cli_path(path: &Path) -> PathBuf {
    crate::path_utils::normalize_windows_verbatim_path_buf(path)
}

#[cfg(not(windows))]
pub(super) fn git_cli_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}
