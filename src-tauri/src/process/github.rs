//! GitHub CLI construction and bounded output lifetime.

use std::path::PathBuf;
use std::process::Output;
use std::time::Duration;

/// A GitHub invocation described before any process exists.
pub(crate) struct GhCommandSpec {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) envs: Vec<(String, String)>,
    pub(crate) creation_flags: u32,
}

#[derive(Debug)]
pub(crate) enum QueryError {
    Spawn(std::io::Error),
    Timeout,
}

pub(crate) async fn query(spec: GhCommandSpec, timeout: Duration) -> Result<Output, QueryError> {
    let mut command = command_from_spec(spec);
    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(QueryError::Spawn(error)),
        Err(_) => Err(QueryError::Timeout),
    }
}

/// The only place a `gh` `Command` is constructed. Pure: it spawns nothing, so a
/// test can inspect the environment and flags it would apply.
fn command_from_spec(spec: GhCommandSpec) -> tokio::process::Command {
    let GhCommandSpec {
        program,
        args,
        envs,
        creation_flags,
    } = spec;
    let mut command = tokio::process::Command::new(program);
    command.args(&args);
    for (key, value) in &envs {
        command.env(key, value);
    }
    command.kill_on_drop(true);
    crate::process::host::credentials::scrub_credentials_from_tokio_command(&mut command);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut command);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(command.as_std_mut(), creation_flags);
    // On non-Windows there is no API to apply creation flags; the read keeps the
    // field part of the spec's contract rather than dead on those targets.
    #[cfg(not(windows))]
    let _ = creation_flags;
    command
}

#[cfg(test)]
mod tests;
