//! #3516 - preflight launch preparation: the same adapter, cwd and env as the CLI spawn.

use super::{LocalProcessBackend, SpawnEnvironment};
use crate::errors::AppError;
use crate::pty::backend::{PreflightLaunch, PreflightLaunchRequest};

/// Turns one stage argv into the exact program, args, Windows command line, cwd and full
/// env the CLI would get from `prepare_launch` + `prepare_spawn_command`.
pub(super) fn prepare_preflight_launch(
    backend: &LocalProcessBackend,
    request: PreflightLaunchRequest,
) -> Result<PreflightLaunch, AppError> {
    let launch = super::launch::prepare_launch(
        &request.program,
        &request.args,
        request.resolved_agent_host_shell.as_ref(),
    )?;
    let cwd = crate::path_utils::normalize_windows_verbatim_path(&request.cwd);
    let built = backend.prepare_spawn_command(
        &launch,
        &cwd,
        uuid::Uuid::new_v4(),
        SpawnEnvironment {
            configured_env: &request.configured_env,
            env_remove_keys: &request.env_remove_keys,
            extra_env: &request.extra_env,
        },
    )?;
    let windows_raw_args = cfg!(windows).then(|| {
        launch
            .args
            .iter()
            .map(|a| pty_quoted(a))
            .collect::<Vec<_>>()
            .join(" ")
    });
    Ok(PreflightLaunch {
        program: launch.program,
        args: launch.args,
        windows_raw_args,
        cwd,
        env: built
            .command
            .iter_full_env_as_str()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    })
}

/// Copy of portable-pty 0.9.0 `cmdbuilder.rs::append_quoted`, so a preflight gets the
/// same Windows command line as the PTY child. A non-empty argument with no space, tab,
/// `\n`, `\v` or `"` is copied verbatim; otherwise it is wrapped in `"`, a backslash run
/// before `"` is doubled plus one (escaping the `"`), a run at the end is doubled, and
/// other runs are copied.
pub(super) fn pty_quoted(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{0b}', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut chars = arg.chars().peekable();
    loop {
        let mut backslashes = 0;
        while chars.peek() == Some(&'\\') {
            chars.next();
            backslashes += 1;
        }
        match chars.next() {
            None => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2));
                break;
            }
            Some('"') => {
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
            }
            Some(c) => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
            }
        }
    }
    out.push('"');
    out
}
