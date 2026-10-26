use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub(crate) fn run_agency_git(
    current_dir: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    run_process("git", current_dir, args, timeout)
}

#[cfg(test)]
pub(crate) fn run_process_with_timeout(
    program: &str,
    current_dir: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    run_process(program, current_dir, args, timeout)
}

fn run_process(
    program: &str,
    current_dir: Option<&Path>,
    args: &[&str],
    timeout: Duration,
) -> Result<Output, String> {
    let mut cmd = Command::new(program);
    crate::process::host::credentials::scrub_credentials_from_std_command(&mut cmd);
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

#[cfg(test)]
mod tests;
