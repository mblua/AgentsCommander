use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use crate::config::role_templates::AGENCY_TEMPLATES_DIR;

const GIT_LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_FETCH_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const GIT_CHECKOUT_TIMEOUT: Duration = Duration::from_secs(30);

fn run_git_with_timeout(
    current_dir: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    crate::process::git::cache::run_agency_git(current_dir, args, timeout)
        .map_err(|e| format!("Failed to run git {:?}: {}", args, e))
}

#[cfg(test)]
pub(super) fn run_process_with_timeout(
    program: &str,
    current_dir: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    crate::process::git::cache::run_process_with_timeout(program, current_dir, args, timeout)
}

pub(super) fn resolve_commit_with_git(repo: &str, reference: &str) -> Result<String, String> {
    #[cfg(test)]
    if let Ok(commit) = std::env::var("AGENTSCOMMANDER_AGENCY_TEST_COMMIT") {
        let _ = (repo, reference);
        return Ok(commit);
    }

    let output = run_git_with_timeout(None, &["ls-remote", repo, reference], GIT_LS_REMOTE_TIMEOUT)
        .map_err(|e| {
            format!(
                "Failed to run git. Install git or use an environment where git is on PATH: {}",
                e
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "Failed to resolve Agency repository ref with git ls-remote: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sha = stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .find(|candidate| candidate.len() == 40 && candidate.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| "git ls-remote did not return a 40-character commit sha".to_string())?;
    if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("git ls-remote returned an invalid sha".into());
    }
    Ok(sha.to_string())
}

pub(super) fn fetch_repo_with_git(
    repo: &str,
    commit: &str,
    config_dir: &Path,
) -> Result<PathBuf, String> {
    let temp = config_dir.join(format!(
        "{}.download-{}",
        AGENCY_TEMPLATES_DIR,
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&temp).map_err(|e| {
        format!(
            "Failed to create Agency download dir {}: {}",
            temp.display(),
            e
        )
    })?;
    let run = |args: &[&str], timeout: Duration| -> Result<(), String> {
        let output = run_git_with_timeout(Some(&temp), args, timeout)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    };
    run(&["init", "--quiet"], GIT_CHECKOUT_TIMEOUT)?;
    run(&["remote", "add", "origin", repo], GIT_CHECKOUT_TIMEOUT)?;
    run(
        &["fetch", "--depth", "1", "origin", commit],
        GIT_FETCH_TIMEOUT,
    )?;
    run(
        &["checkout", "--quiet", "--detach", commit],
        GIT_CHECKOUT_TIMEOUT,
    )?;
    Ok(temp)
}
