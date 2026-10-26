use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

pub(crate) const PID_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(75);

pub(crate) fn write_child_script(path: &Path) {
    let script = r#"param([string]$PidFile)
$ErrorActionPreference = 'Stop'
$pid | Set-Content -LiteralPath $PidFile -Encoding ASCII
[Console]::Out.WriteLine('AC_READY')
[Console]::Out.Flush()
while ($true) {
    $line = [Console]::In.ReadLine()
    if ($null -eq $line) { break }
    if ($line -eq 'AC_EXIT') {
        [Console]::Out.WriteLine('AC_EXITING')
        [Console]::Out.Flush()
        exit 0
    }
    if ($line.StartsWith('AC_ECHO ')) {
        [Console]::Out.WriteLine('AC_ECHOED ' + $line.Substring(8))
        [Console]::Out.Flush()
    }
}
"#;
    std::fs::write(path, script).expect("write pty child script");
}

pub(crate) fn script_args(script_path: &Path, pid_file: &Path) -> Vec<String> {
    vec![
        "-NoLogo".to_string(),
        "-NoProfile".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        path_to_string(script_path),
        "-PidFile".to_string(),
        path_to_string(pid_file),
    ]
}

pub(crate) fn run_powershell_preflight() {
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Write-Output AC_PS_OK",
        ])
        .output()
        .expect("run PowerShell preflight");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("AC_PS_OK"),
        "PowerShell preflight failed: status={:?}\nstdout={stdout}\nstderr={stderr}",
        output.status.code()
    );
}

pub(crate) async fn wait_for_pid_file(path: &Path, timeout: Duration) -> Result<u32, String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                return Ok(pid);
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!("timeout waiting for pid file {}", path.display()))
}

pub(crate) async fn wait_for_pid_change(
    path: &Path,
    old_pid: u32,
    timeout: Duration,
) -> Result<u32, String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                if pid != old_pid {
                    return Ok(pid);
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!(
        "timeout waiting for pid file {} to change from {old_pid}",
        path.display()
    ))
}

pub(crate) async fn wait_for_process_exit(pid: u32, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if !process_exists(pid) {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!("process {pid} still alive after {timeout:?}"))
}

pub(crate) fn process_exists(pid: u32) -> bool {
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) \
                 {{ exit 0 }} else {{ exit 1 }}"
            ),
        ])
        .status();
    matches!(status, Ok(status) if status.success())
}

pub(crate) fn stop_process(pid: u32) -> std::io::Result<()> {
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("Stop-Process -Id {pid} -Force -ErrorAction SilentlyContinue"),
        ])
        .status()
        .map(|_| ())
}

pub(crate) fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}
