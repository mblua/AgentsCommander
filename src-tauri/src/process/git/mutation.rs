//! Git clone/reset lifetimes and synchronous workgroup deletion probes.

use std::path::Path;
use std::process::Output;
use std::time::Duration;

const GIT_CLONE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const GIT_RESET_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub(crate) enum ResetCloneError {
    Spawn(std::io::Error),
    TimedOut { seconds: u64 },
}

#[derive(Debug)]
pub(crate) enum CloneRepositoryError {
    Spawn(std::io::Error),
    TimedOut { seconds: u64 },
}

impl std::fmt::Display for CloneRepositoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "Failed to spawn git clone: {error}"),
            Self::TimedOut { seconds } => {
                write!(f, "git clone timed out after {seconds} seconds")
            }
        }
    }
}

pub(crate) fn clone_repository(
    url: &str,
    target: &Path,
) -> impl std::future::Future<Output = Result<Output, CloneRepositoryError>> + Send {
    let mut cmd = clone_command(url, target);
    async move { classify_clone_result(tokio::time::timeout(GIT_CLONE_TIMEOUT, cmd.output()).await) }
}

fn classify_clone_result(
    result: Result<std::io::Result<Output>, tokio::time::error::Elapsed>,
) -> Result<Output, CloneRepositoryError> {
    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(CloneRepositoryError::Spawn(error)),
        Err(_) => Err(CloneRepositoryError::TimedOut {
            seconds: GIT_CLONE_TIMEOUT.as_secs(),
        }),
    }
}

pub(crate) fn reset_clone_index(
    target: &Path,
) -> impl std::future::Future<Output = Result<Output, ResetCloneError>> + Send {
    let mut cmd = reset_command(target);
    async move {
        classify_reset_result(
            tokio::time::timeout(GIT_RESET_TIMEOUT, cmd.output()).await,
            GIT_RESET_TIMEOUT,
        )
    }
}

fn classify_reset_result(
    result: Result<std::io::Result<Output>, tokio::time::error::Elapsed>,
    reset_timeout: Duration,
) -> Result<Output, ResetCloneError> {
    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(ResetCloneError::Spawn(error)),
        Err(_) => Err(ResetCloneError::TimedOut {
            seconds: reset_timeout.as_secs(),
        }),
    }
}

pub(crate) fn workgroup_status(path: &Path) -> std::io::Result<Output> {
    workgroup_status_command(path).output()
}

pub(crate) fn workgroup_unpushed(path: &Path) -> std::io::Result<Output> {
    workgroup_unpushed_command(path).output()
}

fn clone_command(url: &str, target: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("git");
    crate::process::host::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut cmd);
    cmd.args(["-c", "core.longpaths=true", "clone", "--depth", "1", url])
        .arg(target.as_os_str());
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd
}

fn reset_command(target: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("git");
    crate::process::host::credentials::scrub_credentials_from_tokio_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut cmd);
    cmd.args(["reset"]).current_dir(target);
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    cmd
}

fn workgroup_status_command(path: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    crate::process::host::credentials::scrub_credentials_from_std_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_std_command(&mut cmd);
    cmd.args(["status", "--porcelain"])
        .current_dir(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd
}

fn workgroup_unpushed_command(path: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    crate::process::host::credentials::scrub_credentials_from_std_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_std_command(&mut cmd);
    cmd.args(["log", "@{upstream}..HEAD", "--oneline"])
        .current_dir(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd
}

#[cfg(test)]
mod tests;
