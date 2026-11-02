use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::*;
use crate::process::agent::{
    with_probe_settlement_hook, ProbeNativeObservation, ProbeSettlementHook,
};

fn spec_for(script: &str, timeout: Duration) -> ContainedRunSpec {
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe",
        vec!["/D".to_string(), "/C".into(), script.into()],
    );
    #[cfg(not(windows))]
    let (program, args) = ("sh", vec!["-c".to_string(), script.into()]);
    ContainedRunSpec {
        program: program.to_string(),
        args,
        windows_raw_args: None,
        cwd: std::env::temp_dir().to_string_lossy().to_string(),
        env: std::env::vars().collect(),
        timeout,
    }
}

fn long_sleep() -> &'static str {
    if cfg!(windows) {
        "ping -n 30 127.0.0.1 >NUL"
    } else {
        "sleep 30"
    }
}

async fn run(spec: ContainedRunSpec) -> ContainedRun {
    let (_tx, mut cancel) = tokio::sync::watch::channel(false);
    run_contained(spec, &mut cancel).await
}

fn hook() -> ProbeSettlementHook {
    ProbeSettlementHook {
        before_native_proof: None,
        release_native_proof: None,
        settlement_window: None,
        native_observations: Default::default(),
        panic_after_native_proof_release: false,
        panic_after_native_query_defect: false,
        stdout_reader: None,
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

#[tokio::test]
async fn exit_zero_captures_stdout() {
    let result = run(spec_for("echo ok", Duration::from_secs(30))).await;
    assert_eq!(result.end, ContainedRunEnd::Exited(0));
    assert!(text(&result.stdout).contains("ok"));
    assert_eq!(result.cleanup_defect, None);
}

#[tokio::test]
async fn nonzero_exit_code_propagates() {
    let result = run(spec_for("exit 7", Duration::from_secs(30))).await;
    assert_eq!(result.end, ContainedRunEnd::Exited(7));
}

#[tokio::test]
async fn timeout_kills_tree() {
    let result = run(spec_for(long_sleep(), Duration::from_secs(1))).await;
    assert_eq!(result.end, ContainedRunEnd::TimedOut);
    assert!(
        result.duration < Duration::from_secs(15),
        "{:?}",
        result.duration
    );
}

#[tokio::test]
async fn descendant_holding_pipe_does_not_hang() {
    let script = if cfg!(windows) {
        "start /b ping -n 30 127.0.0.1 & echo done"
    } else {
        "sleep 30 & echo done"
    };
    let result = run(spec_for(script, Duration::from_secs(60))).await;
    assert_eq!(result.end, ContainedRunEnd::Exited(0));
    assert!(
        result.duration < Duration::from_secs(15),
        "{:?}",
        result.duration
    );
}

#[tokio::test]
async fn cancel_returns_cancelled() {
    let (tx, mut cancel) = tokio::sync::watch::channel(false);
    let canceller = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        tx.send(true).expect("send cancel");
    };
    let spec = spec_for(long_sleep(), Duration::from_secs(60));
    let (result, ()) = tokio::join!(run_contained(spec, &mut cancel), canceller);
    assert_eq!(result.end, ContainedRunEnd::Cancelled);
    assert!(
        result.duration < Duration::from_secs(15),
        "{:?}",
        result.duration
    );
}

#[tokio::test]
async fn missing_program_is_spawn_failed() {
    let mut spec = spec_for("exit 0", Duration::from_secs(30));
    spec.program = "ac-preflight-missing-3516".to_string();
    let result = run(spec).await;
    assert!(
        matches!(result.end, ContainedRunEnd::SpawnFailed(_)),
        "{:?}",
        result.end
    );
}

#[tokio::test]
async fn env_is_exactly_the_given_env() {
    let script = if cfg!(windows) {
        "echo [%AC_PF_PROBE%]"
    } else {
        "echo [$AC_PF_PROBE]"
    };
    let mut spec = spec_for(script, Duration::from_secs(30));
    spec.env.push(("AC_PF_PROBE".into(), "v1".into()));
    let result = run(spec).await;
    assert!(
        text(&result.stdout).contains("[v1]"),
        "{}",
        text(&result.stdout)
    );

    // Leg 2 drops a key the test process really has: only `env_clear`
    // keeps the inherited value away from the child.
    let inherited = std::env::vars()
        .find(|(k, _)| k == "CARGO_PKG_NAME")
        .map(|(_, v)| v)
        .expect("cargo test sets CARGO_PKG_NAME (positive control)");
    let script = if cfg!(windows) {
        "echo [%CARGO_PKG_NAME%]"
    } else {
        "echo [$CARGO_PKG_NAME]"
    };
    let mut spec = spec_for(script, Duration::from_secs(30));
    spec.env
        .retain(|(k, _)| !k.eq_ignore_ascii_case("CARGO_PKG_NAME"));
    let result = run(spec).await;
    let out = text(&result.stdout);
    assert!(out.contains('['), "child ran: {out}");
    assert!(!out.contains(&format!("[{inherited}]")), "{out}");
}

#[cfg(windows)]
#[tokio::test]
async fn windows_raw_args_replace_args() {
    let mut spec = spec_for("echo from-args", Duration::from_secs(30));
    spec.windows_raw_args = Some("/D /C echo \"x y\" raw-line".into());
    let result = run(spec).await;
    let out = text(&result.stdout);
    assert_eq!(result.end, ContainedRunEnd::Exited(0), "{out}");
    assert!(out.contains("\"x y\" raw-line"), "{out}");
    assert!(!out.contains("from-args"), "{out}");
}

#[tokio::test]
async fn output_is_bounded() {
    let script = if cfg!(windows) {
        "for /L %i in (1,1,1000) do @echo 012345678"
    } else {
        "i=0; while [ $i -lt 1000 ]; do echo 012345678; i=$((i+1)); done"
    };
    let result = run(spec_for(script, Duration::from_secs(30))).await;
    assert_eq!(result.end, ContainedRunEnd::Exited(0));
    assert_eq!(result.stdout.len(), 4096);
}

#[tokio::test]
async fn cwd_is_applied() {
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("pf cwd 3516");
    std::fs::create_dir(&dir).expect("create spaced dir");
    let script = if cfg!(windows) { "cd" } else { "pwd" };
    let mut spec = spec_for(script, Duration::from_secs(30));
    spec.cwd = dir.to_string_lossy().to_string();
    let result = run(spec).await;
    assert!(
        text(&result.stdout).contains("pf cwd 3516"),
        "{}",
        text(&result.stdout)
    );
}

#[tokio::test]
async fn cleanup_defect_keeps_primary_end_and_owner() {
    let mut hook = hook();
    hook.native_observations = [ProbeNativeObservation::Error("injected")].into();
    let spec = spec_for("exit 7", Duration::from_secs(30));
    let result = with_probe_settlement_hook(hook, run(spec)).await;
    assert_eq!(result.end, ContainedRunEnd::Exited(7));
    let defect = result.cleanup_defect.expect("defect recorded");
    assert!(defect.contains("injected"), "{defect}");
}

#[tokio::test]
async fn pending_cleanup_holds_the_return() {
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let mut hook = hook();
    hook.before_native_proof = Some(ready_tx);
    hook.release_native_proof = Some(release_rx);
    let returned = AtomicBool::new(false);
    let runner = with_probe_settlement_hook(hook, async {
        let result = run(spec_for("exit 0", Duration::from_secs(30))).await;
        returned.store(true, Ordering::SeqCst);
        result
    });
    let checker = async {
        ready_rx.await.expect("native proof reached");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !returned.load(Ordering::SeqCst),
            "returned before the native proof"
        );
        release_tx.send(()).expect("release native proof");
    };
    let (result, ()) = tokio::join!(runner, checker);
    assert_eq!(result.end, ContainedRunEnd::Exited(0));
}

#[test]
fn containment_text_is_preflight_wording() {
    assert_eq!(
        preflight_text("Version probe process-tree containment unavailable.".into()),
        "process-tree containment is unavailable"
    );
    assert_eq!(preflight_text("spawn failed: x".into()), "spawn failed: x");
}
