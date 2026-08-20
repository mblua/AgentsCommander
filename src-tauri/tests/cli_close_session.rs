//! Integration tests for the close-session CLI exit-code contract.
//!
//! Strategy: spawn the real CLI binary in a subprocess with master-token
//! bypass, then simulate the daemon side by writing the expected delivery
//! marker and response file. On Windows the simulator is a fresh
//! `powershell.exe` process because the long-lived Rust test runner can miss
//! outbox files written by the CLI subprocess. On non-Windows the simulator
//! remains an in-process Rust helper.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(not(target_os = "windows"))]
use std::sync::mpsc;
#[cfg(not(target_os = "windows"))]
use std::time::{Duration, Instant};

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl Tmp {
    fn new(prefix: &str) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::process::id().hash(&mut h);
        std::thread::current().id().hash(&mut h);
        let path = std::env::temp_dir().join(format!(
            "ac-{}-{}-{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            h.finish()
        ));
        std::fs::create_dir_all(&path).expect("create tmp dir");
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

fn copy_binary_into(tmp: &Path) -> PathBuf {
    let src = Path::new(env!("CARGO_BIN_EXE_agentscommander-new"));
    let dst = tmp.join(src.file_name().expect("binary file name"));
    std::fs::copy(src, &dst).expect("copy binary");
    dst
}

struct Fixture {
    bin: PathBuf,
    agent_root: PathBuf,
    master: String,
}

fn build_fixture(tmp: &Path, agent: &str) -> Fixture {
    let bin = copy_binary_into(tmp);
    let stem = bin
        .file_stem()
        .expect("bin stem")
        .to_string_lossy()
        .to_string();
    let cfg_dir = tmp.join(format!(".{}", stem));
    std::fs::create_dir_all(&cfg_dir).expect("create config dir");

    let master = "test-master-token-224".to_string();
    std::fs::write(cfg_dir.join("master-token.txt"), &master).expect("write master token");

    let settings = serde_json::json!({
        "defaultShell": "powershell.exe",
        "defaultShellArgs": [],
        "agents": [],
        "projectPaths": [tmp.to_string_lossy().to_string()],
    });
    std::fs::write(
        cfg_dir.join("settings.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .expect("write settings.json");

    let agent_root = tmp
        .join("proj")
        .join(".ac")
        .join("wg-1-test")
        .join(format!("__agent_{}", agent));
    std::fs::create_dir_all(&agent_root).expect("create agent dir");

    Fixture {
        bin,
        agent_root,
        master,
    }
}

fn close_response(
    status: &str,
    sessions_closed: u64,
    session_ids: &[&str],
    target: &str,
) -> String {
    serde_json::json!({
        "action": "close-session",
        "target": target,
        "status": status,
        "sessions_closed": sessions_closed,
        "session_ids": session_ids,
        "requested_by": "tester",
    })
    .to_string()
}

#[cfg(target_os = "windows")]
const WINDOWS_SIMULATOR_PS1: &str = r#"
param(
  [Parameter(Mandatory=$true)][string]$OutboxDir,
  [Parameter(Mandatory=$true)][string]$ResponsesDir,
  [Parameter(Mandatory=$true)][string]$ResponseBodyPath,
  [Parameter(Mandatory=$true)][string]$ExpectedTarget,
  [Parameter(Mandatory=$true)][string]$ExpectedTo,
  [Parameter(Mandatory=$true)][int]$ExpectedTimeoutSec,
  [Parameter(Mandatory=$true)][int]$TimeoutSec,
  [string]$Mode = 'respond'
)

$deadline = (Get-Date).AddSeconds($TimeoutSec)
$lastReadinessError = $null

while ((Get-Date) -lt $deadline) {
  $messageFile = Get-ChildItem -LiteralPath $OutboxDir -Filter '*.json' -File -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTimeUtc, Name |
    Select-Object -First 1

  if ($null -ne $messageFile) {
    $messagePath = $messageFile.FullName

    try {
      $messageBody = Get-Content -LiteralPath $messagePath -Raw -ErrorAction Stop
      $message = $messageBody | ConvertFrom-Json -ErrorAction Stop
    } catch {
      $lastReadinessError = "message visible but not parseable yet at ${messagePath}: $($_.Exception.Message)"
      Start-Sleep -Milliseconds 50
      continue
    }

    if (-not $message.id) {
      $lastReadinessError = "message visible but missing id at ${messagePath}"
      Start-Sleep -Milliseconds 50
      continue
    }
    if (-not $message.requestId) {
      $lastReadinessError = "message visible but missing requestId at ${messagePath}"
      Start-Sleep -Milliseconds 50
      continue
    }

    if ($Mode -eq 'reject') {
      # 1440: el daemon rechaza antes de validar force/timeout y escribe la
      # reason ANTES del JSON espejado (mailbox.rs reject_message).
      $rejectedDir = Join-Path $OutboxDir 'rejected'
      New-Item -ItemType Directory -Force -Path $rejectedDir | Out-Null
      $utf8NoBomReject = New-Object System.Text.UTF8Encoding($false)
      $reasonBody = Get-Content -LiteralPath $ResponseBodyPath -Raw -ErrorAction Stop
      [System.IO.File]::WriteAllText((Join-Path $rejectedDir "$($message.id).reason.txt"), $reasonBody, $utf8NoBomReject)
      [System.IO.File]::WriteAllText((Join-Path $rejectedDir "$($message.id).json"), $messageBody, $utf8NoBomReject)
      Remove-Item -LiteralPath $messagePath -Force -ErrorAction SilentlyContinue
      Write-Output $message.id
      exit 0
    }

    if ($message.action -ne 'close-session') {
      Write-Error "contract violation: action was '$($message.action)'"
      exit 13
    }
    if ($message.target -ne $ExpectedTarget) {
      Write-Error "contract violation: target was '$($message.target)', expected '$ExpectedTarget'"
      exit 13
    }
    if ($message.to -ne $ExpectedTo) {
      Write-Error "contract violation: to was '$($message.to)', expected '$ExpectedTo'"
      exit 13
    }
    $forceType = if ($null -eq $message.force) { '<null>' } else { $message.force.GetType().FullName }
    if (-not ($message.force -is [bool]) -or $message.force -ne $true) {
      Write-Error "contract violation: force was '$($message.force)' with type '$forceType'"
      exit 13
    }
    $timeoutType = if ($null -eq $message.timeoutSecs) { '<null>' } else { $message.timeoutSecs.GetType().FullName }
    $timeoutIsInteger = $message.timeoutSecs -is [byte] -or
      $message.timeoutSecs -is [sbyte] -or
      $message.timeoutSecs -is [int16] -or
      $message.timeoutSecs -is [uint16] -or
      $message.timeoutSecs -is [int] -or
      $message.timeoutSecs -is [uint32] -or
      $message.timeoutSecs -is [long] -or
      $message.timeoutSecs -is [uint64]
    if (-not $timeoutIsInteger -or [long]$message.timeoutSecs -ne [long]$ExpectedTimeoutSec) {
      Write-Error "contract violation: timeoutSecs was '$($message.timeoutSecs)' with type '$timeoutType', expected integer '$ExpectedTimeoutSec'"
      exit 13
    }

    $deliveredDir = Join-Path $OutboxDir 'delivered'
    New-Item -ItemType Directory -Force -Path $deliveredDir | Out-Null
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText((Join-Path $deliveredDir "$($message.id).json"), $messageBody, $utf8NoBom)
    Remove-Item -LiteralPath $messagePath -Force -ErrorAction SilentlyContinue

    if ($Mode -eq 'respond-staged') {
      # 1440 F1: el daemon escribe el response sin rename atomico y dos veces
      # (dual-write 224 A.6), asi que el archivo existe truncado durante una
      # ventana. Aca la ventana es determinista: 600ms (mas de dos ticks del
      # poll de 250ms del CLI) con un prefijo que no parsea.
      New-Item -ItemType Directory -Force -Path $ResponsesDir | Out-Null
      Set-Content -LiteralPath (Join-Path $ResponsesDir "$($message.requestId).json") -Value '{"' -NoNewline -Encoding ascii
      Start-Sleep -Milliseconds 600
    }

    New-Item -ItemType Directory -Force -Path $ResponsesDir | Out-Null
    $responseBody = Get-Content -LiteralPath $ResponseBodyPath -Raw -ErrorAction Stop
    [System.IO.File]::WriteAllText((Join-Path $ResponsesDir "$($message.requestId).json"), $responseBody, $utf8NoBom)

    Write-Output $message.id
    exit 0
  }

  Start-Sleep -Milliseconds 50
}

if ($lastReadinessError) {
  Write-Error "timeout waiting for ready CLI outbox message at $OutboxDir; last readiness error: $lastReadinessError"
} else {
  Write-Error "timeout waiting for CLI outbox write at $OutboxDir"
}
exit 12
"#;

#[cfg(target_os = "windows")]
fn write_windows_simulator_script(tmp: &Path) -> PathBuf {
    let script = tmp.join("close-session-simulator.ps1");
    std::fs::write(&script, WINDOWS_SIMULATOR_PS1).expect("write simulator script");
    script
}

struct SimulatorOutput {
    stdout: String,
    stderr: String,
}

#[cfg(target_os = "windows")]
enum SimulatorHandle {
    Process(std::process::Child),
}

#[cfg(not(target_os = "windows"))]
enum SimulatorHandle {
    Thread(mpsc::Receiver<Result<String, String>>),
}

impl SimulatorHandle {
    fn wait(self) -> Result<SimulatorOutput, String> {
        match self {
            #[cfg(target_os = "windows")]
            SimulatorHandle::Process(child) => {
                let out = child
                    .wait_with_output()
                    .map_err(|e| format!("wait for simulator process: {}", e))?;
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                if !out.status.success() {
                    return Err(format!(
                        "simulator exited {:?}\nstdout: {}\nstderr: {}",
                        out.status.code(),
                        stdout,
                        stderr
                    ));
                }
                Ok(SimulatorOutput { stdout, stderr })
            }
            #[cfg(not(target_os = "windows"))]
            SimulatorHandle::Thread(rx) => {
                let msg_id = rx
                    .recv_timeout(Duration::from_secs(25))
                    .map_err(|e| format!("simulator thread did not finish: {}", e))?
                    .map_err(|e| format!("simulator failed: {}", e))?;
                Ok(SimulatorOutput {
                    stdout: msg_id,
                    stderr: String::new(),
                })
            }
        }
    }
}

/// §1440: `mode` is "respond" (write delivered/ + responses/) or "reject"
/// (write rejected/<msg_id>.reason.txt, no contract validation, no response),
/// mirroring the daemon's two terminal outcomes. "respond-staged" is
/// "respond" with the response left torn for 600ms first (§1440 F1).
fn spawn_daemon_simulator(
    _tmp: &Path,
    outbox_dir: &Path,
    responses_dir: &Path,
    response_body: &str,
    expected_target: &str,
    expected_timeout_secs: &str,
    mode: &str,
) -> SimulatorHandle {
    #[cfg(target_os = "windows")]
    {
        let response_body_path = _tmp.join("response-body.json");
        std::fs::write(&response_body_path, response_body).expect("write response body");
        let script = write_windows_simulator_script(_tmp);

        let child = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                script.to_string_lossy().as_ref(),
                "-OutboxDir",
                outbox_dir.to_string_lossy().as_ref(),
                "-ResponsesDir",
                responses_dir.to_string_lossy().as_ref(),
                "-ResponseBodyPath",
                response_body_path.to_string_lossy().as_ref(),
                "-ExpectedTarget",
                expected_target,
                "-ExpectedTo",
                expected_target,
                "-ExpectedTimeoutSec",
                expected_timeout_secs,
                "-TimeoutSec",
                "20",
                "-Mode",
                mode,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn PowerShell simulator");
        SimulatorHandle::Process(child)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let outbox_for_thread = outbox_dir.to_path_buf();
        let responses_for_thread = responses_dir.to_path_buf();
        let response_owned = response_body.to_string();
        let target_owned = expected_target.to_string();
        let mode_owned = mode.to_string();
        let timeout_secs = expected_timeout_secs
            .parse::<u32>()
            .expect("test timeout secs must parse");
        let (tx, rx) = mpsc::channel::<Result<String, String>>();
        std::thread::spawn(move || {
            let result = simulate_daemon_response(
                &outbox_for_thread,
                &responses_for_thread,
                &response_owned,
                &target_owned,
                timeout_secs,
                &mode_owned,
                Duration::from_secs(20),
            );
            let _ = tx.send(result);
        });
        SimulatorHandle::Thread(rx)
    }
}

#[cfg(not(target_os = "windows"))]
fn simulate_daemon_response(
    outbox_dir: &Path,
    responses_dir: &Path,
    response_body: &str,
    expected_target: &str,
    expected_timeout_secs: u32,
    mode: &str,
    overall_timeout: Duration,
) -> Result<String, String> {
    let start = Instant::now();
    let poll = Duration::from_millis(50);
    let mut last_readiness_error = None::<String>;

    let (msg_path, body, msg, msg_id, request_id) = loop {
        if start.elapsed() >= overall_timeout {
            return Err(match last_readiness_error {
                Some(e) => format!(
                    "timeout waiting for ready CLI outbox message at {:?}; last readiness error: {}",
                    outbox_dir, e
                ),
                None => format!("timeout waiting for CLI outbox write at {:?}", outbox_dir),
            });
        }

        let Some(path) = std::fs::read_dir(outbox_dir).ok().and_then(|rd| {
            let mut files: Vec<_> = rd
                .flatten()
                .map(|entry| entry.path())
                .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("json"))
                .collect();
            files.sort();
            files.into_iter().next()
        }) else {
            std::thread::sleep(poll);
            continue;
        };

        let body = match std::fs::read_to_string(&path) {
            Ok(body) => body,
            Err(e) => {
                last_readiness_error = Some(format!(
                    "message visible but not readable at {:?}: {}",
                    path, e
                ));
                std::thread::sleep(poll);
                continue;
            }
        };
        let msg: serde_json::Value = match serde_json::from_str(&body) {
            Ok(msg) => msg,
            Err(e) => {
                last_readiness_error = Some(format!(
                    "message visible but not parseable at {:?}: {}",
                    path, e
                ));
                std::thread::sleep(poll);
                continue;
            }
        };
        let Some(msg_id) = msg
            .get("id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            last_readiness_error = Some(format!("message visible but missing id at {:?}", path));
            std::thread::sleep(poll);
            continue;
        };
        let Some(request_id) = msg
            .get("requestId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            last_readiness_error = Some(format!(
                "message visible but missing requestId at {:?}",
                path
            ));
            std::thread::sleep(poll);
            continue;
        };

        let msg_id = msg_id.to_string();
        let request_id = request_id.to_string();

        break (path, body, msg, msg_id, request_id);
    };

    if mode == "reject" {
        // §1440: el daemon rechaza antes de validar el contrato y antes de
        // tocar sesiones; escribe la reason primero y nunca un response.
        let rejected_dir = outbox_dir.join("rejected");
        std::fs::create_dir_all(&rejected_dir).map_err(|e| e.to_string())?;
        std::fs::write(
            rejected_dir.join(format!("{}.reason.txt", msg_id)),
            response_body,
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(rejected_dir.join(format!("{}.json", msg_id)), &body)
            .map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&msg_path);
        return Ok(msg_id);
    }

    validate_close_session_message(&msg, expected_target, expected_timeout_secs)
        .map_err(|e| format!("contract violation in {:?}: {}", msg_path, e))?;

    let delivered_dir = outbox_dir.join("delivered");
    std::fs::create_dir_all(&delivered_dir).map_err(|e| e.to_string())?;
    std::fs::write(delivered_dir.join(format!("{}.json", msg_id)), &body)
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&msg_path);

    if mode == "respond-staged" {
        // §1440 F1: el daemon escribe el response sin rename atomico y dos
        // veces (dual-write §224 A.6), asi que el archivo existe truncado
        // durante una ventana. Aca la ventana es determinista: 600ms (mas de
        // dos ticks del poll de 250ms del CLI) con un prefijo que no parsea.
        std::fs::create_dir_all(responses_dir).map_err(|e| e.to_string())?;
        std::fs::write(responses_dir.join(format!("{}.json", request_id)), "{\"")
            .map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_millis(600));
    }

    std::fs::create_dir_all(responses_dir).map_err(|e| e.to_string())?;
    std::fs::write(
        responses_dir.join(format!("{}.json", request_id)),
        response_body,
    )
    .map_err(|e| e.to_string())?;

    Ok(msg_id)
}

#[cfg(not(target_os = "windows"))]
fn validate_close_session_message(
    msg: &serde_json::Value,
    expected_target: &str,
    expected_timeout_secs: u32,
) -> Result<(), String> {
    let field = |name: &str| msg.get(name).ok_or_else(|| format!("missing {}", name));
    if field("action")?.as_str() != Some("close-session") {
        return Err(format!("action was {:?}", field("action")?));
    }
    if field("target")?.as_str() != Some(expected_target) {
        return Err(format!("target was {:?}", field("target")?));
    }
    if field("to")?.as_str() != Some(expected_target) {
        return Err(format!("to was {:?}", field("to")?));
    }
    if field("force")?.as_bool() != Some(true) {
        return Err(format!("force was {:?}", field("force")?));
    }
    if field("timeoutSecs")?.as_u64() != Some(u64::from(expected_timeout_secs)) {
        return Err(format!("timeoutSecs was {:?}", field("timeoutSecs")?));
    }
    Ok(())
}

fn run_close_session_with_simulator(
    tmp: &Path,
    fix: &Fixture,
    response_body: String,
    target: &str,
    timeout_secs: &str,
) -> (Option<i32>, String, String, String, String) {
    run_close_session_in_mode(tmp, fix, response_body, target, timeout_secs, "respond")
}

/// §1440: same run, but the simulator rejects the message instead of
/// responding; `reason` is written to rejected/<msg_id>.reason.txt.
fn run_close_session_expecting_rejection(
    tmp: &Path,
    fix: &Fixture,
    reason: String,
    target: &str,
    timeout_secs: &str,
) -> (Option<i32>, String, String, String, String) {
    run_close_session_in_mode(tmp, fix, reason, target, timeout_secs, "reject")
}

fn run_close_session_in_mode(
    tmp: &Path,
    fix: &Fixture,
    response_body: String,
    target: &str,
    timeout_secs: &str,
    mode: &str,
) -> (Option<i32>, String, String, String, String) {
    let stem = fix.bin.file_stem().unwrap().to_string_lossy().to_string();
    let ac_dir = fix.agent_root.join(format!(".{}", stem));
    let outbox_dir = ac_dir.join("outbox");
    let responses_dir = ac_dir.join("responses");
    std::fs::create_dir_all(&outbox_dir).unwrap();

    let simulator = spawn_daemon_simulator(
        tmp,
        &outbox_dir,
        &responses_dir,
        &response_body,
        target,
        timeout_secs,
        mode,
    );

    let out = Command::new(&fix.bin)
        .args([
            "close-session",
            "--token",
            &fix.master,
            "--root",
            &fix.agent_root.to_string_lossy(),
            "--target",
            target,
            "--force",
            "--timeout",
            timeout_secs,
        ])
        .env("RUST_LOG", "agentscommander=info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn binary");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let sim_output = simulator.wait().unwrap_or_else(|e| {
        panic!(
            "simulator failed: {}\nCLI exit code: {:?}\nCLI stdout: {}\nCLI stderr: {}",
            e,
            out.status.code(),
            stdout,
            stderr,
        )
    });

    (
        out.status.code(),
        stdout,
        stderr,
        sim_output.stdout,
        sim_output.stderr,
    )
}

#[test]
fn close_session_no_match_exits_zero_with_prose() {
    let tmp = Tmp::new("close-no-match");
    let fix = build_fixture(tmp.path(), "bob-not-running");
    let target = "proj:wg-1-test/bob-not-running";

    let (code, stdout, stderr, sim_stdout, sim_stderr) = run_close_session_with_simulator(
        tmp.path(),
        &fix,
        close_response("no_match", 0, &[], target),
        target,
        "5",
    );

    assert_eq!(
        code,
        Some(0),
        "no_match must exit 0.\nstdout: {}\nstderr: {}\nsim stdout: {}\nsim stderr: {}",
        stdout,
        stderr,
        sim_stdout,
        sim_stderr
    );
    assert!(
        stdout.contains("\"status\": \"no_match\"") || stdout.contains("\"status\":\"no_match\""),
        "stdout must contain the no_match JSON response; got: {}",
        stdout
    );
    assert!(
        stdout.contains("No sessions matched") && stdout.contains("nothing to close"),
        "stdout must contain no_match prose; got: {}",
        stdout
    );
}

#[test]
fn close_session_restore_in_progress_exits_zero_with_retry_prose() {
    let tmp = Tmp::new("close-restore");
    let fix = build_fixture(tmp.path(), "carol-mid-restore");
    let target = "proj:wg-1-test/carol-mid-restore";

    let (code, stdout, stderr, sim_stdout, sim_stderr) = run_close_session_with_simulator(
        tmp.path(),
        &fix,
        close_response("restore_in_progress", 0, &[], target),
        target,
        "5",
    );

    assert_eq!(
        code,
        Some(0),
        "restore_in_progress must exit 0.\nstdout: {}\nstderr: {}\nsim stdout: {}\nsim stderr: {}",
        stdout,
        stderr,
        sim_stdout,
        sim_stderr
    );
    assert!(
        stdout.contains("Daemon is still restoring sessions"),
        "stdout must contain the restore-in-progress retry prose; got: {}",
        stdout
    );
    assert!(
        stdout.contains("Retry in a few seconds"),
        "stdout must hint at retry; got: {}",
        stdout
    );
}

#[test]
fn close_session_already_closed_exits_zero_with_prose() {
    let tmp = Tmp::new("close-already");
    let fix = build_fixture(tmp.path(), "dan-raced");
    let target = "proj:wg-1-test/dan-raced";

    let (code, stdout, stderr, sim_stdout, sim_stderr) = run_close_session_with_simulator(
        tmp.path(),
        &fix,
        close_response("already_closed", 0, &[], target),
        target,
        "5",
    );

    assert_eq!(
        code,
        Some(0),
        "already_closed must exit 0.\nstdout: {}\nstderr: {}\nsim stdout: {}\nsim stderr: {}",
        stdout,
        stderr,
        sim_stdout,
        sim_stderr
    );
    assert!(
        stdout.contains("already closed"),
        "stdout must contain already_closed prose; got: {}",
        stdout
    );
}

#[test]
fn close_session_closed_exits_zero_silent_prose() {
    let tmp = Tmp::new("close-closed");
    let fix = build_fixture(tmp.path(), "eve-actually-running");
    let target = "proj:wg-1-test/eve-actually-running";

    let (code, stdout, stderr, sim_stdout, sim_stderr) = run_close_session_with_simulator(
        tmp.path(),
        &fix,
        close_response(
            "closed",
            1,
            &["00000000-0000-0000-0000-000000000001"],
            target,
        ),
        target,
        "5",
    );

    assert_eq!(
        code,
        Some(0),
        "closed must exit 0.\nstdout: {}\nstderr: {}\nsim stdout: {}\nsim stderr: {}",
        stdout,
        stderr,
        sim_stdout,
        sim_stderr
    );
    assert!(
        stdout.contains("\"status\": \"closed\"") || stdout.contains("\"status\":\"closed\""),
        "stdout must contain closed JSON; got: {}",
        stdout
    );
    assert!(
        !stdout.contains("No sessions matched") && !stdout.contains("already closed"),
        "closed status should not emit no_match/already_closed prose; got: {}",
        stdout
    );
}

#[test]
fn close_session_response_via_outbox_relative_path_only() {
    let tmp = Tmp::new("close-outbox-rel");
    let fix = build_fixture(tmp.path(), "frank-rel-only");
    let target = "proj:wg-1-test/frank-rel-only";

    let stem = fix.bin.file_stem().unwrap().to_string_lossy().to_string();
    let responses_dir = fix.agent_root.join(format!(".{}", stem)).join("responses");

    let (code, stdout, stderr, sim_stdout, sim_stderr) = run_close_session_with_simulator(
        tmp.path(),
        &fix,
        close_response("no_match", 0, &[], target),
        target,
        "5",
    );

    assert_eq!(
        code,
        Some(0),
        "outbox-relative response must exit 0.\nstdout: {}\nstderr: {}\nsim stdout: {}\nsim stderr: {}",
        stdout,
        stderr,
        sim_stdout,
        sim_stderr
    );
    assert!(
        stdout.contains("No sessions matched"),
        "prose must appear; got: {}",
        stdout
    );
    let response_files: Vec<_> = std::fs::read_dir(&responses_dir)
        .map(|rd| rd.flatten().collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(
        !response_files.is_empty(),
        "responses dir at {:?} must contain at least one response file",
        responses_dir
    );
}

#[test]
fn close_session_incoherent_response_exits_two() {
    let tmp = Tmp::new("close-bad-response");
    let fix = build_fixture(tmp.path(), "gina-bad-response");
    let target = "proj:wg-1-test/gina-bad-response";

    let (code, stdout, stderr, sim_stdout, sim_stderr) = run_close_session_with_simulator(
        tmp.path(),
        &fix,
        r#"{"status":"new_unrecognized_status","target":"proj:wg-1-test/gina-bad-response"}"#
            .to_string(),
        target,
        "5",
    );

    assert_eq!(
        code,
        Some(2),
        "unknown response status must exit 2.\nstdout: {}\nstderr: {}\nsim stdout: {}\nsim stderr: {}",
        stdout,
        stderr,
        sim_stdout,
        sim_stderr
    );
    assert!(
        stdout.contains("new_unrecognized_status"),
        "stdout must include daemon JSON; got: {}",
        stdout
    );
}

#[test]
fn close_session_rejected_exits_one_with_reason() {
    let tmp = Tmp::new("close-rejected");
    let fix = build_fixture(tmp.path(), "hank-unauthorized");
    let target = "proj:wg-1-test/hank-unauthorized";

    let (code, stdout, stderr, _sim_out, _sim_err) = run_close_session_expecting_rejection(
        tmp.path(),
        &fix,
        "close-session target unresolvable: nope".to_string(),
        target,
        "5",
    );

    assert_eq!(
        code,
        Some(1),
        "a rejected close-session must exit 1.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(
        stderr.contains("close-session rejected"),
        "stderr must carry the rejection prefix; got: {}",
        stderr
    );
    assert!(
        stderr.contains("target unresolvable: nope"),
        "stderr must carry the daemon reason; got: {}",
        stderr
    );
    assert!(
        !stdout.contains("\"status\""),
        "no response JSON must reach stdout on rejection; got: {}",
        stdout
    );
}

// §1440 F1: el daemon escribe el response sin atomicidad y (en el caso
// comun) dos veces; respond-staged materializa la ventana: el archivo
// existe 600ms (mas de dos ticks de poll de 250ms) con un prefijo torn
// antes de tener el JSON completo. Sin el parse-gate de 5.1.8, un tick
// lee el torn, imprime basura en stdout y sale 2 sobre un cierre exitoso.
#[test]
fn close_session_staged_response_write_still_exits_zero() {
    let tmp = Tmp::new("close-staged");
    let fix = build_fixture(tmp.path(), "hank-staged");
    let target = "proj:wg-1-test/hank-staged";
    let body = format!(
        "{{\"action\":\"close-session\",\"target\":\"{}\",\"status\":\"closed\",\"sessions_closed\":1,\"session_ids\":[\"7\"],\"requested_by\":\"tester\"}}",
        target
    );

    let (code, stdout, stderr, _sim_out, _sim_err) = run_close_session_in_mode(
        tmp.path(),
        &fix,
        body.clone(),
        target,
        "5",
        "respond-staged",
    );

    assert_eq!(
        code,
        Some(0),
        "a staged (non-atomic) response write must still exit 0.
stdout: {}
stderr: {}",
        stdout,
        stderr
    );
    assert!(
        stdout.starts_with(&body),
        "stdout must begin with the complete response JSON (no torn prefix may reach stdout); got: {}",
        stdout
    );
}
