use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::config::role_templates::AGENCY_TEMPLATES_DIR;

const GIT_LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_FETCH_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const GIT_CHECKOUT_TIMEOUT: Duration = Duration::from_secs(30);

fn run_git_with_timeout(
    current_dir: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    run_process_with_timeout("git", current_dir, args, timeout)
        .map_err(|e| format!("Failed to run git {:?}: {}", args, e))
}

pub(super) fn run_process_with_timeout(
    program: &str,
    current_dir: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    let mut cmd = Command::new(program);
    crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
    crate::config::agent_path::apply_search_path_to_std_command(&mut cmd);
    if let Some(dir) = current_dir {
        cmd.current_dir(dir);
    }
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn failed for {} {:?}: {}", program, args, e))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture stdout".to_string())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture stderr".to_string())?;

    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.read_to_end(&mut bytes);
        (result, bytes)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stderr.read_to_end(&mut bytes);
        (result, bytes)
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("wait failed for {} {:?}: {}", program, args, e))?
        {
            break status;
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = join_reader(stdout_reader, "stdout")?;
            let stderr = join_reader(stderr_reader, "stderr")?;
            let stderr_text = String::from_utf8_lossy(&stderr);
            return Err(format!(
                "{} {:?} timed out after {} seconds; killed and reaped. stderr: {} stdout_bytes={}",
                program,
                args,
                timeout.as_secs(),
                stderr_text.trim(),
                stdout.len()
            ));
        }

        std::thread::sleep(Duration::from_millis(50));
    };

    let stdout = join_reader(stdout_reader, "stdout")?;
    let stderr = join_reader(stderr_reader, "stderr")?;

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn join_reader(
    reader: std::thread::JoinHandle<(std::io::Result<usize>, Vec<u8>)>,
    stream_name: &str,
) -> Result<Vec<u8>, String> {
    let (result, bytes) = reader
        .join()
        .map_err(|_| format!("{stream_name} reader thread panicked"))?;
    result.map_err(|e| format!("{stream_name} read failed: {e}"))?;
    Ok(bytes)
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
