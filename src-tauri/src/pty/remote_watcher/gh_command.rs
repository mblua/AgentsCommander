use std::path::{Path, PathBuf};

use super::CREATE_NO_WINDOW;

/// A `gh` invocation, fully described before any process exists.
///
/// Fields carry NO visibility modifier, so they are private to this module; the
/// in-file `#[cfg(test)] mod tests` reads them as a child module. The builder
/// validates its inputs by construction, which is what makes an abbreviated SHA
/// impossible to send rather than detectable afterwards.
pub(crate) struct GhCommandSpec {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) envs: Vec<(String, String)>,
    pub(crate) creation_flags: u32,
}

pub(crate) enum GhQuery<'a> {
    Ci {
        nwo: &'a str,
        sha40: &'a str,
    },
    CiRun {
        nwo: &'a str,
        run_id: u64,
    },
    CiAttemptJobs {
        nwo: &'a str,
        run_id: u64,
        attempt: u64,
    },
    Compare {
        nwo: &'a str,
        sha40: &'a str,
    },
    RepoInfo {
        nwo: &'a str,
    },
}

/// The only place a `gh` argument string is built. Every input is validated
/// here, so a malformed `nwo` or a short SHA yields `Err` and no spec at all.
pub(crate) fn build_gh_command_spec(
    gh: &Path,
    query: GhQuery<'_>,
) -> Result<GhCommandSpec, String> {
    let args = match query {
        GhQuery::Ci { nwo, sha40 } => {
            validate_nwo(nwo)?;
            validate_sha40(sha40)?;
            vec![
                "api".to_string(),
                format!("repos/{nwo}/actions/runs?head_sha={sha40}&per_page=100"),
            ]
        }
        GhQuery::CiRun { nwo, run_id } => {
            validate_nwo(nwo)?;
            if run_id == 0 {
                return Err("invalid run id".to_string());
            }
            vec![
                "api".to_string(),
                format!("repos/{nwo}/actions/runs/{run_id}"),
            ]
        }
        GhQuery::CiAttemptJobs {
            nwo,
            run_id,
            attempt,
        } => {
            validate_nwo(nwo)?;
            if run_id == 0 || attempt == 0 {
                return Err("invalid run identity".to_string());
            }
            vec![
                "api".to_string(),
                format!("repos/{nwo}/actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100"),
            ]
        }
        GhQuery::Compare { nwo, sha40 } => {
            validate_nwo(nwo)?;
            validate_sha40(sha40)?;
            vec![
                "api".to_string(),
                format!("repos/{nwo}/compare/HEAD...{sha40}"),
            ]
        }
        GhQuery::RepoInfo { nwo } => {
            validate_nwo(nwo)?;
            vec!["api".to_string(), format!("repos/{nwo}")]
        }
    };

    Ok(GhCommandSpec {
        program: gh.to_path_buf(),
        args,
        // `GH_PROMPT_DISABLED` is what makes an unauthenticated `gh` exit instead
        // of blocking a round on a prompt; `gh auth login` is never invoked.
        envs: vec![
            ("GH_PROMPT_DISABLED".to_string(), "1".to_string()),
            ("GH_NO_UPDATE_NOTIFIER".to_string(), "1".to_string()),
            ("GH_PAGER".to_string(), "cat".to_string()),
        ],
        creation_flags: {
            #[cfg(windows)]
            {
                CREATE_NO_WINDOW
            }
            #[cfg(not(windows))]
            {
                0
            }
        },
    })
}

pub(crate) fn validate_nwo(nwo: &str) -> Result<(), String> {
    let mut segments = nwo.split('/');
    let owner = segments.next().unwrap_or_default();
    let repo = segments.next().unwrap_or_default();
    if segments.next().is_some() || owner.is_empty() || repo.is_empty() {
        return Err(format!("invalid repository nwo: {nwo}"));
    }
    if !nwo
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
    {
        return Err(format!("invalid repository nwo: {nwo}"));
    }
    Ok(())
}

pub(crate) fn validate_sha40(sha: &str) -> Result<(), String> {
    if is_sha40(sha) {
        Ok(())
    } else {
        Err(format!(
            "sha must be 40 lowercase hex characters, got: {sha}"
        ))
    }
}

pub(crate) fn is_sha40(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The only place a `gh` `Command` is constructed. Pure: it spawns nothing, so a
/// test can inspect the environment and flags it would apply.
pub(crate) fn command_from_spec(spec: GhCommandSpec) -> tokio::process::Command {
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
    crate::pty::credentials::scrub_credentials_from_tokio_command(&mut command);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut command);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(command.as_std_mut(), creation_flags);
    // On non-Windows there is no API to apply creation flags; the read keeps the
    // field part of the spec's contract rather than dead on those targets.
    #[cfg(not(windows))]
    let _ = creation_flags;
    command
}

/// The only place a `git` `Command` is constructed. Pure, like
/// `command_from_spec`. Gate 2 of `detect_git_status` (`GIT_CEILING_DIRECTORIES`)
/// lives here so a repo whose `.git` exists but is corrupt cannot walk up and
/// answer with an unrelated ancestor's data. Gate 1 (the async `.git` metadata
/// check) runs once per path in the caller, before any command is built.
pub(crate) fn git_command(path: &str, args: &[String]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(path)
        .arg("--no-optional-locks")
        .args(args)
        .kill_on_drop(true);
    crate::pty::credentials::scrub_credentials_from_tokio_command(&mut command);
    crate::config::agent_path::apply_search_path_to_tokio_command(&mut command);

    if let Some(parent) = Path::new(path).parent() {
        if let Ok(ceiling) = std::env::join_paths(std::iter::once(parent)) {
            command.env("GIT_CEILING_DIRECTORIES", ceiling);
        }
    }

    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(command.as_std_mut(), CREATE_NO_WINDOW);
    command
}
