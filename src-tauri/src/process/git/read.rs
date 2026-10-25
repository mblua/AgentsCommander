//! Read-only Git child lifetimes for discovery and repository inspection.

use std::path::PathBuf;
use std::time::Duration;

/// #1298 - bound for the one-shot branch detection below. Deliberately its own
/// constant and not the sweeper's `DETECT_TIMEOUT`: this is a discovery-time
/// one-shot on the async command body, not the poll loop's per-round bound, and
/// the two must be able to move independently.
pub(crate) const BRANCH_DETECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Detect the git branch of a directory without blocking a runtime worker.
///
/// #1298 - the previous synchronous version spawned a `std::process::Command`
/// straight on the async body of `discover_ac_agents` / `discover_project` and
/// held its Tokio worker for as long as `git` took, with no bound. Now
/// `tokio::process` under `BRANCH_DETECT_TIMEOUT`, with `kill_on_drop(true)` so
/// the timeout's drop is what terminates `git.exe` (same mechanism as
/// `git_watcher.rs`'s detection).
///
/// Named `detect_repo_branch`, not `detect_git_branch`, because
/// `config/session_context.rs` already owns a private `detect_git_branch` that
/// runs a DIFFERENT command (`git -C <dir> branch --show-current`); two
/// same-named private detectors would break argv-based triage of `git.exe` peaks.
///
/// No `--no-optional-locks` here, unlike `detect_git_status` in the watcher
/// owner where it IS load-bearing: `rev-parse` neither takes `.git/index.lock`
/// nor rewrites `.git/index`, so the flag would be cargo-culted noise.
pub(crate) async fn detect_repo_branch(dir: &str) -> Option<String> {
    let mut cmd = branch_command(dir);

    match tokio::time::timeout(BRANCH_DETECT_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if branch.is_empty() || branch == "HEAD" {
                None
            } else {
                Some(branch)
            }
        }
        Err(_) => {
            log::debug!(
                "[discovery] branch detection timed out after {BRANCH_DETECT_TIMEOUT:?}: {dir}"
            );
            None
        }
        _ => None,
    }
}

/// Inherited Git controls that would point the child at another repository or index.
pub(crate) const PROJECT_GIT_REDIRECT_ENV: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

pub(crate) async fn run_project_git(cwd: PathBuf, args: Vec<String>) -> Result<Vec<u8>, String> {
    let mut cmd = project_command(&cwd, &args);

    // args[3] is the subcommand after the shared prefix; stderr stays out of the result.
    let subcommand = args.get(3).map(String::as_str).unwrap_or("?");
    let out = cmd
        .output()
        .await
        .map_err(|e| format!("spawn git {subcommand}: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {subcommand} exited with {}", out.status));
    }
    Ok(out.stdout)
}

pub(crate) async fn remote_origin_output(
    working_dir: &str,
) -> std::io::Result<std::process::Output> {
    let mut cmd = remote_origin_command(working_dir);

    cmd.output().await
}

fn branch_command(dir: &str) -> tokio::process::Command {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = tokio::process::Command::new("git");
    crate::process::host::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut cmd);
    cmd.args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .kill_on_drop(true);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd
}

fn project_command(cwd: &std::path::Path, args: &[String]) -> tokio::process::Command {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = tokio::process::Command::new("git");
    crate::process::host::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut cmd);
    for var in PROJECT_GIT_REDIRECT_ENV {
        cmd.env_remove(var);
    }
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd
}

fn remote_origin_command(working_dir: &str) -> tokio::process::Command {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = tokio::process::Command::new("git");
    crate::process::host::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut cmd);
    cmd.args(["remote", "get-url", "origin"])
        .current_dir(working_dir)
        .kill_on_drop(true);

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd
}

#[cfg(test)]
mod tests;
