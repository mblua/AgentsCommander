#![cfg(target_os = "windows")]
//! Integration test for issue #129. Reproduces the PS-NonInteractive `&` direct-
//! call failure mode and asserts that the fix (conditional AttachConsole) lets
//! stdout flow through the inherited pipe.
//!
//! IMPORTANT: marked #[ignore] because the bug only reproduces in release mode
//! (`windows_subsystem = "windows"` is gated on `not(debug_assertions)`). To run:
//!     cargo test --release --test cli_powershell_capture -- --ignored
//!
//! CI coverage for shipped/testable release executable names lives in:
//!     npm run build:prod
//!     npm run smoke:cli-release-windows

use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_agentscommander-new");

fn ps_command(shell: &str, args: &str) -> Option<Command> {
    if Command::new(shell)
        .arg("-Help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return None;
    }
    let mut c = Command::new(shell);
    c.args([
        "-NonInteractive",
        "-NoProfile",
        "-Command",
        &format!("& '{}' {}", BIN.replace('\'', "''"), args),
    ]);
    c.stdout(Stdio::piped());
    c.stderr(Stdio::piped());
    Some(c)
}

fn run_ps(shell: &str, args: &str) -> Option<(i32, String, String)> {
    let mut cmd = ps_command(shell, args)?;
    let out = cmd.spawn().ok()?.wait_with_output().ok()?;
    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    Some((code, stdout, stderr))
}

/// `run_ps` variant for a caller-supplied PS script (no `& '<BIN>'` prefix)
/// that ends with `; exit $LASTEXITCODE` so the outer powershell.exe exit code
/// reflects the inner process's. Only meaningful for console-subsystem carriers
/// (bash.exe); the GUI-subsystem AC binary under bare `&` leaves
/// `$LASTEXITCODE` unset, which is why the four direct-capture tests above
/// intentionally never assert exit codes.
fn run_ps_script_with_exit_propagation(shell: &str, script: &str) -> Option<(i32, String, String)> {
    if Command::new(shell)
        .arg("-Help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        return None;
    }
    let mut c = Command::new(shell);
    c.args(["-NonInteractive", "-NoProfile", "-Command", script]);
    c.stdout(Stdio::piped());
    c.stderr(Stdio::piped());
    let out = c.spawn().ok()?.wait_with_output().ok()?;
    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    Some((code, stdout, stderr))
}

#[allow(clippy::assertions_on_constants)] // Plan R2.6 step 3: intentional runtime guard.
fn debug_guard() {
    assert!(
        !cfg!(debug_assertions),
        "This test only validates issue #129 in release mode. \
         Run with `cargo test --release --test cli_powershell_capture -- --ignored`."
    );
}

// Exit-code assertions are intentionally absent on all four tests (R5.4 — applies
// R4.1's NEW-3 logic symmetrically to R2.6). PS-NonInteractive bare `&` does not
// propagate the AC binary's $LASTEXITCODE for GUI-subsystem children (PE
// Subsystem=2); the outer powershell.exe always exits 0 regardless. The bug-
// relevant signals are stdout/stderr presence — those alone distinguish fixed
// from unfixed binaries. The exit-code carrier is the bash-routed test below
// (`..._via_git_bash`): bash.exe is console-subsystem, so $LASTEXITCODE
// propagates through it and out of the outer powershell.exe.

#[test]
#[ignore = "Manual developer check; CI uses smoke:cli-release-windows against shipped/testable exe names"]
fn list_peers_outputs_valid_json_under_powershell_noninteractive() {
    debug_guard();

    let tmp = std::env::temp_dir().join(format!(
        "ac-test-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let safe_root = tmp.to_string_lossy().replace('\'', "''");
    let token = "00000000-0000-0000-0000-000000000000";

    let (_code, stdout, stderr) = run_ps(
        "powershell.exe",
        &format!("list-peers --token {} --root '{}'", token, safe_root),
    )
    .expect("powershell.exe must be available on Windows CI/dev");

    assert!(
        !stdout.trim().is_empty(),
        "stdout should contain the JSON payload (post-fix). stderr=[{}]",
        stderr
    );
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should parse as JSON");
    assert!(parsed.is_array(), "expected JSON array");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
#[ignore = "Manual developer check; CI uses smoke:cli-release-windows against shipped/testable exe names"]
fn send_help_outputs_under_powershell_noninteractive() {
    debug_guard();

    let (_code, stdout, stderr) =
        run_ps("powershell.exe", "send --help").expect("powershell.exe must be available");

    assert!(
        !stdout.trim().is_empty(),
        "stdout should contain help text. stderr=[{}]",
        stderr
    );
    assert!(
        stdout.contains("--to") || stdout.contains("DELIVERY MODES"),
        "help text missing expected content; got: {}",
        stdout
    );
}

#[test]
#[ignore = "Manual developer check; CI uses smoke:cli-release-windows against shipped/testable exe names"]
fn send_unknown_flag_emits_stderr_under_powershell_noninteractive() {
    debug_guard();

    let (_code, _stdout, stderr) = run_ps("powershell.exe", "send --bogus-flag-xyz")
        .expect("powershell.exe must be available");
    assert!(
        !stderr.trim().is_empty(),
        "stderr must contain a usage error"
    );
}

// G8: parallel pwsh.exe tests. Skip if pwsh not installed.
#[test]
#[ignore = "Manual developer check; CI uses smoke:cli-release-windows against shipped/testable exe names when pwsh.exe is installed"]
fn list_peers_outputs_valid_json_under_pwsh_noninteractive() {
    debug_guard();

    let tmp = std::env::temp_dir().join(format!(
        "ac-test-pwsh-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let safe_root = tmp.to_string_lossy().replace('\'', "''");
    let token = "00000000-0000-0000-0000-000000000000";

    let result = run_ps(
        "pwsh.exe",
        &format!("list-peers --token {} --root '{}'", token, safe_root),
    );
    let (_code, stdout, _stderr) = match result {
        Some(t) => t,
        None => {
            eprintln!("skip: pwsh.exe not available");
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }
    };

    assert!(
        !stdout.trim().is_empty(),
        "stdout should contain the JSON payload"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert!(parsed.is_array());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
#[ignore = "Manual developer check; CI uses smoke:cli-release-windows against shipped/testable exe names"]
fn list_peers_outputs_valid_json_under_powershell_noninteractive_via_git_bash() {
    debug_guard();

    // #1596: Git Bash is the required carrier on Windows — bash.exe is
    // console-subsystem, so PowerShell captures its stdout AND propagates the
    // AC binary's exit code through it (`; exit $LASTEXITCODE`). Skip (not a
    // failure) when bash.exe is not on PATH, e.g. a non-Git-Bash dev machine.
    if Command::new("bash.exe")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("skip: bash.exe not available");
        return;
    }

    let tmp = std::env::temp_dir().join(format!(
        "ac-test-bash-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let safe_root = tmp.to_string_lossy().replace('\'', "''");
    let token = "00000000-0000-0000-0000-000000000000";

    // Inner bash command (single-quote escaping identical to `ps_command`):
    //   'BIN' list-peers --token <token> --root '<root>'
    // wrapped as  & bash.exe -lc '<inner>'; exit $LASTEXITCODE  at the PS level.
    let bash_inner = format!(
        "'{}' list-peers --token {} --root '{}'",
        BIN.replace('\'', "''"),
        token,
        safe_root
    );
    let ps_script = format!(
        "& bash.exe -lc '{}'; exit $LASTEXITCODE",
        bash_inner.replace('\'', "''")
    );
    let (code, stdout, stderr) = run_ps_script_with_exit_propagation("powershell.exe", &ps_script)
        .expect("powershell.exe must be available on Windows CI/dev");

    assert!(
        !stdout.trim().is_empty(),
        "stdout should contain the JSON payload via the Git Bash carrier. stderr=[{}]",
        stderr
    );
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stdout should parse as JSON");
    assert!(parsed.is_array(), "expected JSON array");
    assert_eq!(
        code, 0,
        "outer powershell.exe must exit 0 (bash.exe propagates the AC binary's \
         $LASTEXITCODE through `; exit $LASTEXITCODE`); stderr=[{}]",
        stderr
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
