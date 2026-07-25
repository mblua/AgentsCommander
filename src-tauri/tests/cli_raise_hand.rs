//! Integration tests for the raise-hand CLI boolean stdout contract.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(not(target_os = "windows"))]
use std::sync::mpsc;
#[cfg(not(target_os = "windows"))]
use std::time::{Duration, Instant};

const VALID_TOKEN: &str = "11111111-1111-1111-1111-111111111111";

struct Tmp(PathBuf);

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Tmp {
    fn new(prefix: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("ac-{}-{}", prefix, uuid::Uuid::new_v4().simple()));
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
}

fn build_fixture(tmp: &Path) -> Fixture {
    let bin = copy_binary_into(tmp);
    let agent_root = tmp
        .join("proj")
        .join(".ac")
        .join("wg-1-test")
        .join("__agent_dev-rust");
    std::fs::create_dir_all(&agent_root).expect("create agent dir");
    Fixture { bin, agent_root }
}

#[cfg(target_os = "windows")]
const WINDOWS_SIMULATOR_PS1: &str = r#"
param(
  [Parameter(Mandatory=$true)][string]$OutboxDir,
  [Parameter(Mandatory=$true)][string]$ResponsesDir,
  [Parameter(Mandatory=$true)][string]$ResponseBodyPath,
  [Parameter(Mandatory=$true)][int]$TimeoutSec
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

    if ($message.action -ne 'raise-hand') {
      Write-Error "contract violation: action was '$($message.action)'"
      exit 13
    }
    if ($message.to -ne '') {
      Write-Error "contract violation: to was '$($message.to)'"
      exit 13
    }
    if ($message.mode -ne '') {
      Write-Error "contract violation: mode was '$($message.mode)'"
      exit 13
    }
    if ($message.getOutput -ne $false) {
      Write-Error "contract violation: getOutput was '$($message.getOutput)'"
      exit 13
    }

    $deliveredDir = Join-Path $OutboxDir 'delivered'
    New-Item -ItemType Directory -Force -Path $deliveredDir | Out-Null
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText((Join-Path $deliveredDir "$($message.id).json"), $messageBody, $utf8NoBom)
    Remove-Item -LiteralPath $messagePath -Force -ErrorAction SilentlyContinue

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
    let script = tmp.join("raise-hand-simulator.ps1");
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

fn spawn_daemon_simulator(
    tmp: &Path,
    outbox_dir: &Path,
    responses_dir: &Path,
    response_body: &str,
) -> SimulatorHandle {
    #[cfg(target_os = "windows")]
    {
        let response_body_path = tmp.join("raise-hand-response-body.json");
        std::fs::write(&response_body_path, response_body).expect("write response body");
        let script = write_windows_simulator_script(tmp);

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
                "-TimeoutSec",
                "20",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn PowerShell simulator");
        SimulatorHandle::Process(child)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = tmp;
        let outbox_for_thread = outbox_dir.to_path_buf();
        let responses_for_thread = responses_dir.to_path_buf();
        let response_owned = response_body.to_string();
        let (tx, rx) = mpsc::channel::<Result<String, String>>();
        std::thread::spawn(move || {
            let result = simulate_daemon_response(
                &outbox_for_thread,
                &responses_for_thread,
                &response_owned,
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

    validate_raise_hand_message(&msg).map_err(|e| format!("contract violation: {}", e))?;

    let delivered_dir = outbox_dir.join("delivered");
    std::fs::create_dir_all(&delivered_dir).map_err(|e| e.to_string())?;
    std::fs::write(delivered_dir.join(format!("{}.json", msg_id)), &body)
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&msg_path);

    std::fs::create_dir_all(responses_dir).map_err(|e| e.to_string())?;
    std::fs::write(
        responses_dir.join(format!("{}.json", request_id)),
        response_body,
    )
    .map_err(|e| e.to_string())?;

    Ok(msg_id)
}

#[cfg(not(target_os = "windows"))]
fn validate_raise_hand_message(msg: &serde_json::Value) -> Result<(), String> {
    let field = |name: &str| msg.get(name).ok_or_else(|| format!("missing {}", name));
    if field("action")?.as_str() != Some("raise-hand") {
        return Err(format!("action was {:?}", field("action")?));
    }
    if field("to")?.as_str() != Some("") {
        return Err(format!("to was {:?}", field("to")?));
    }
    if field("mode")?.as_str() != Some("") {
        return Err(format!("mode was {:?}", field("mode")?));
    }
    if field("body")?.as_str() != Some("") {
        return Err(format!("body was {:?}", field("body")?));
    }
    if field("getOutput")?.as_bool() != Some(false) {
        return Err(format!("getOutput was {:?}", field("getOutput")?));
    }
    if msg.get("requestId").and_then(|v| v.as_str()).is_none() {
        return Err("missing requestId".to_string());
    }
    if msg.get("target").is_some() {
        return Err(format!("target should be absent, got {:?}", msg["target"]));
    }
    Ok(())
}

fn run_raise_hand_with_simulator(
    tmp: &Path,
    fix: &Fixture,
    response_body: &str,
) -> (Option<i32>, String, String, String, String) {
    let stem = fix.bin.file_stem().unwrap().to_string_lossy().to_string();
    let ac_dir = fix.agent_root.join(format!(".{}", stem));
    let outbox_dir = ac_dir.join("outbox");
    let responses_dir = ac_dir.join("responses");
    std::fs::create_dir_all(&outbox_dir).unwrap();

    let simulator = spawn_daemon_simulator(tmp, &outbox_dir, &responses_dir, response_body);

    let out = Command::new(&fix.bin)
        .args([
            "raise-hand",
            "--token",
            VALID_TOKEN,
            "--root",
            &fix.agent_root.to_string_lossy(),
            "--timeout",
            "5",
        ])
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
fn raise_hand_true_response_exits_zero_with_exact_stdout() {
    let tmp = Tmp::new("raise-hand-true");
    let fix = build_fixture(tmp.path());

    let (code, stdout, stderr, sim_stdout, sim_stderr) =
        run_raise_hand_with_simulator(tmp.path(), &fix, r#"{"raised":true}"#);

    assert_eq!(
        code,
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}\nsim stdout: {sim_stdout}\nsim stderr: {sim_stderr}"
    );
    assert_eq!(stdout, "true\n");
}

#[test]
fn raise_hand_false_response_exits_zero_with_exact_stdout() {
    let tmp = Tmp::new("raise-hand-false");
    let fix = build_fixture(tmp.path());

    let (code, stdout, stderr, sim_stdout, sim_stderr) =
        run_raise_hand_with_simulator(tmp.path(), &fix, r#"{"raised":false}"#);

    assert_eq!(
        code,
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}\nsim stdout: {sim_stdout}\nsim stderr: {sim_stderr}"
    );
    assert_eq!(stdout, "false\n");
}

#[test]
fn raise_hand_malformed_response_exits_two_without_boolean_stdout() {
    let tmp = Tmp::new("raise-hand-malformed");
    let fix = build_fixture(tmp.path());

    let (code, stdout, stderr, sim_stdout, sim_stderr) =
        run_raise_hand_with_simulator(tmp.path(), &fix, r#"{"status":"raised"}"#);

    assert_eq!(
        code,
        Some(2),
        "stdout: {stdout}\nstderr: {stderr}\nsim stdout: {sim_stdout}\nsim stderr: {sim_stderr}"
    );
    assert!(stdout.trim().is_empty(), "stdout: {stdout}");
    assert!(stderr.contains("malformed"), "stderr: {stderr}");
}
