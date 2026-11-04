use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::config::config_seed::{
    CollectedSeedFiles, ConfigSeedFailure, ConfigSeedPublication, ConfigSeedRollbackFailure,
    ConfigSeedTier,
};
use crate::errors::AppError;
use crate::process::agent::{with_probe_settlement_hook, ProbeNativeObservation};
use crate::pty::backend::{PreflightLaunch, PtyBackend};
use uuid::Uuid;

const SUFFIX: &str = "The CLI will not start because the gate failed.";

/// Runs every preflight request as given, with the inherited env; logs each preparation.
#[derive(Default)]
struct EchoPreflightBackend {
    log: Arc<Mutex<Vec<&'static str>>>,
}

impl PtyBackend for EchoPreflightBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn spawn(
        &self,
        _spec: crate::pty::backend::BackendSpawnSpec,
    ) -> futures::future::BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }

    fn write(
        &self,
        _authority: &crate::pty::manager::BackendWriteAuthority,
        id: Uuid,
        _data: &[u8],
    ) -> Result<(), AppError> {
        Err(AppError::SessionNotFound(id.to_string()))
    }

    fn resize(&self, _id: Uuid, _cols: u16, _rows: u16) -> Result<(), AppError> {
        Ok(())
    }

    fn kill(&self, _id: Uuid) -> Result<(), AppError> {
        Ok(())
    }

    fn has_session(&self, _id: Uuid) -> bool {
        false
    }

    fn get_screen_snapshot(&self, _id: Uuid) -> Option<crate::pty::output::PtyScreenSnapshot> {
        None
    }

    fn get_pty_size(&self, _id: Uuid) -> Option<(u16, u16)> {
        None
    }

    fn get_screen_rows(&self, _id: Uuid) -> crate::pty::context_scrape::ScreenRowsRead {
        crate::pty::context_scrape::ScreenRowsRead::SessionOver
    }

    fn register_response_watcher(&self, _id: Uuid, _request_id: String, _dir: PathBuf) {}

    fn terminate_job_for_session(&self, _id: Uuid) -> bool {
        false
    }

    fn kill_all_jobs(&self) -> (usize, usize) {
        (0, 0)
    }

    fn prepare_preflight_launch(
        &self,
        request: PreflightLaunchRequest,
    ) -> Result<PreflightLaunch, AppError> {
        self.log.lock().unwrap().push("prepare");
        Ok(PreflightLaunch {
            program: request.program,
            args: request.args,
            windows_raw_args: None,
            cwd: request.cwd,
            env: std::env::vars().collect(),
        })
    }
}

fn manager() -> Arc<Mutex<PtyManager>> {
    logged_manager(Arc::default())
}

fn logged_manager(log: Arc<Mutex<Vec<&'static str>>>) -> Arc<Mutex<PtyManager>> {
    let backend = EchoPreflightBackend { log };
    Arc::new(Mutex::new(PtyManager::new_for_test(Arc::new(backend))))
}

fn stage(source: PreflightStageSource, script: &str, timeout_seconds: u32) -> PreflightStage {
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe",
        vec!["/D".to_string(), "/C".into(), script.into()],
    );
    #[cfg(not(windows))]
    let (program, args) = ("sh", vec!["-c".to_string(), script.into()]);
    let label = match source {
        PreflightStageSource::CodingAgent => "Coding Agent",
        PreflightStageSource::Profile => "Profile A",
    };
    PreflightStage {
        source,
        effective_profile: "A".to_string(),
        label: label.to_string(),
        command_text: script.to_string(),
        program: program.to_string(),
        args,
        timeout_seconds,
    }
}

fn ca(script: &str) -> PreflightStage {
    stage(PreflightStageSource::CodingAgent, script, 30)
}

fn profile(script: &str) -> PreflightStage {
    stage(PreflightStageSource::Profile, script, 30)
}

fn chain_env(cwd: &std::path::Path) -> PreflightChainEnv {
    PreflightChainEnv {
        backend_kind: SessionBackendKind::LocalProcess,
        cwd: cwd.to_string_lossy().to_string(),
        configured_env: Vec::new(),
        env_remove_keys: Vec::new(),
        extra_env: Vec::new(),
        resolved_agent_host_shell: None,
    }
}

async fn run_chain(stages: &[PreflightStage], cwd: &std::path::Path) -> PreflightChainOutcome {
    let (_tx, mut cancel) = tokio::sync::watch::channel(false);
    run_preflight_chain(&manager(), stages, &chain_env(cwd), &mut cancel, |_| {}).await
}

fn statuses(outcome: &PreflightChainOutcome) -> Vec<PreflightStageStatus> {
    outcome.reports.iter().map(|r| r.status).collect()
}

fn append_line(file: &str) -> String {
    if cfg!(windows) {
        format!("echo line>>{file}")
    } else {
        format!("echo line >> {file}")
    }
}

fn long_sleep() -> &'static str {
    if cfg!(windows) {
        "ping -n 30 127.0.0.1 >NUL"
    } else {
        "sleep 30"
    }
}

#[tokio::test]
async fn four_combinations_run_in_order() {
    let temp = tempfile::tempdir().unwrap();
    use PreflightStageSource::{CodingAgent, Profile};
    let combos: Vec<Vec<PreflightStage>> = vec![
        vec![],
        vec![ca("exit 0")],
        vec![profile("exit 0")],
        vec![ca("exit 0"), profile("exit 0")],
    ];
    let expected_sources = [
        vec![],
        vec![CodingAgent],
        vec![Profile],
        vec![CodingAgent, Profile],
    ];
    for (stages, sources) in combos.iter().zip(expected_sources) {
        let outcome = run_chain(stages, temp.path()).await;
        let got: Vec<_> = outcome.reports.iter().map(|r| r.source).collect();
        assert_eq!(got, sources);
        assert!(outcome
            .reports
            .iter()
            .all(|r| r.status == PreflightStageStatus::Passed));
        assert!(chain_passed(&outcome));
    }
}

#[tokio::test]
async fn first_failure_stops_chain() {
    let temp = tempfile::tempdir().unwrap();
    let marker = "profile-ran.txt";
    let outcome = run_chain(&[ca("exit 3"), profile(&append_line(marker))], temp.path()).await;
    assert_eq!(
        statuses(&outcome),
        [PreflightStageStatus::Failed, PreflightStageStatus::NotRun]
    );
    assert_eq!(outcome.reports[0].exit_code, Some(3));
    assert!(!temp.path().join(marker).exists());
    assert!(!chain_passed(&outcome));
}

#[tokio::test]
async fn second_failure_blocks() {
    let temp = tempfile::tempdir().unwrap();
    let outcome = run_chain(&[ca("exit 0"), profile("exit 5")], temp.path()).await;
    assert_eq!(
        statuses(&outcome),
        [PreflightStageStatus::Passed, PreflightStageStatus::Failed]
    );
    assert_eq!(outcome.reports[1].exit_code, Some(5));
    assert!(!chain_passed(&outcome));
}

#[tokio::test]
async fn same_command_runs_twice() {
    let temp = tempfile::tempdir().unwrap();
    let script = append_line("twice.txt");
    let outcome = run_chain(&[ca(&script), profile(&script)], temp.path()).await;
    assert!(chain_passed(&outcome));
    let text = std::fs::read_to_string(temp.path().join("twice.txt")).unwrap();
    assert_eq!(text.lines().count(), 2, "{text:?}");
}

#[tokio::test]
async fn no_export_between_stages() {
    let temp = tempfile::tempdir().unwrap();
    let (set, print) = if cfg!(windows) {
        (
            "set AC_PF_3516_X=1",
            "if defined AC_PF_3516_X (echo AC_PF_3516_X=%AC_PF_3516_X%)",
        )
    } else {
        ("export AC_PF_3516_X=1", "printf '%s' \"$AC_PF_3516_X\"")
    };
    let outcome = run_chain(&[ca(set), profile(print)], temp.path()).await;
    assert!(chain_passed(&outcome));
    assert_eq!(outcome.reports[1].stdout.trim(), "");
}

#[tokio::test]
async fn timeout_and_spawn_failure_map() {
    let temp = tempfile::tempdir().unwrap();
    let timed = stage(PreflightStageSource::CodingAgent, long_sleep(), 1);
    let outcome = run_chain(&[timed], temp.path()).await;
    assert_eq!(statuses(&outcome), [PreflightStageStatus::TimedOut]);

    let mut missing = ca("exit 0");
    missing.program = "ac-preflight-missing-3516".to_string();
    missing.args = Vec::new();
    let outcome = run_chain(&[missing, profile("exit 0")], temp.path()).await;
    assert_eq!(
        statuses(&outcome),
        [
            PreflightStageStatus::SpawnFailed,
            PreflightStageStatus::NotRun
        ]
    );
    assert!(outcome.reports[0]
        .detail
        .as_deref()
        .is_some_and(|d| !d.is_empty()));
}

#[tokio::test]
async fn cancel_marks_cancelled() {
    let temp = tempfile::tempdir().unwrap();
    let (tx, mut cancel) = tokio::sync::watch::channel(false);
    let stages = [ca(long_sleep()), profile("exit 0")];
    let env = chain_env(temp.path());
    let mgr = manager();
    let canceller = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        tx.send(true).expect("send cancel");
    };
    let (outcome, ()) = tokio::join!(
        run_preflight_chain(&mgr, &stages, &env, &mut cancel, |_| {}),
        canceller
    );
    assert_eq!(
        statuses(&outcome),
        [
            PreflightStageStatus::Cancelled,
            PreflightStageStatus::NotRun
        ]
    );
}

#[tokio::test]
async fn progress_is_reported() {
    let temp = tempfile::tempdir().unwrap();
    let (_tx, mut cancel) = tokio::sync::watch::channel(false);
    let mut seen: Vec<Vec<PreflightStageStatus>> = Vec::new();
    let stages = [ca("exit 0"), profile("exit 0")];
    run_preflight_chain(
        &manager(),
        &stages,
        &chain_env(temp.path()),
        &mut cancel,
        |r| seen.push(r.iter().map(|x| x.status).collect()),
    )
    .await;
    let first_running = seen
        .iter()
        .position(|s| s[0] == PreflightStageStatus::Running)
        .expect("stage 1 reported Running");
    let first_passed = seen
        .iter()
        .position(|s| s.contains(&PreflightStageStatus::Passed))
        .expect("a stage reported Passed");
    assert!(first_running < first_passed, "{seen:?}");
    assert_eq!(seen[first_running][1], PreflightStageStatus::Pending);
}

fn report(status: PreflightStageStatus) -> PreflightStageReport {
    let mut report = pending_report(&ca("exit 0"));
    report.status = status;
    report.timeout_seconds = 30;
    report
}

fn assert_clean(message: &str) {
    assert!(message.ends_with(SUFFIX), "{message}");
    assert!(!message.contains(".."), "{message}");
}

#[test]
fn messages_are_exact() {
    use PreflightStageStatus as S;
    let mut failed = report(S::Failed);
    failed.exit_code = Some(3);
    let mut spawn = report(S::SpawnFailed);
    spawn.detail = Some("program not found.".to_string());
    let mut defect = report(S::Passed);
    defect.cleanup_defect = Some("tree query failed.".to_string());
    let cases = [
        (
            failed.clone(),
            "Coding Agent preflight failed with exit code 3.",
        ),
        (
            report(S::TimedOut),
            "Coding Agent preflight exceeded its timeout of 30 seconds.",
        ),
        (
            spawn,
            "Coding Agent preflight could not be started: program not found.",
        ),
        (
            report(S::Signaled),
            "Coding Agent preflight was terminated by a signal.",
        ),
        (
            report(S::Cancelled),
            "Coding Agent preflight was cancelled.",
        ),
        (
            defect,
            "Coding Agent preflight cleanup reported an error: tree query failed.",
        ),
    ];
    for (report, head) in cases {
        let message = launch_failure_message(&report);
        assert_eq!(message, format!("{head} {SUFFIX}"));
        assert_clean(&message);
    }

    let mut denied = failed.clone();
    denied.stderr = "denied\nsecond line".to_string();
    let message = launch_failure_message(&denied);
    assert_eq!(
        message,
        format!("Coding Agent preflight failed with exit code 3. Output: \"denied\". {SUFFIX}")
    );
    assert_clean(&message);

    let mut long = failed.clone();
    long.stdout = "x".repeat(300);
    let message = launch_failure_message(&long);
    let cut = "x".repeat(crate::process::agent::DETAIL_MAX_CHARS);
    assert_eq!(
        message,
        format!(
            "Coding Agent preflight failed with exit code 3. Output: \"{cut}\u{2026}\". {SUFFIX}"
        )
    );
    assert_clean(&message);

    let mut ends = failed.clone();
    ends.stdout = "ends.".to_string();
    let message = launch_failure_message(&ends);
    assert_eq!(
        message,
        format!("Coding Agent preflight failed with exit code 3. Output: \"ends.\". {SUFFIX}")
    );
    assert_clean(&message);

    let mut both = failed;
    both.cleanup_defect = Some("tree query failed".to_string());
    let message = launch_failure_message(&both);
    assert_eq!(
        message,
        format!(
            "Coding Agent preflight failed with exit code 3. \
             Cleanup reported an error: tree query failed. {SUFFIX}"
        )
    );

    assert_eq!(
        resolution_failure_message("Invalid Coding Agent preflight command: x."),
        format!("Invalid Coding Agent preflight command: x. {SUFFIX}")
    );
    assert_eq!(
        refusal_message(HoldRefusal::Test),
        format!("A preflight test is running on this agent. {SUFFIX}")
    );
    assert_eq!(
        CONTAINER_REFUSAL,
        format!(
            "Preflights cannot run for container agents. Remove the preflight commands or \
             switch this Coding Agent to the local backend. {SUFFIX}"
        )
    );
    assert_eq!(
        GATE_UNAVAILABLE,
        format!("The preflight gate is unavailable. {SUFFIX}")
    );
}

#[test]
fn seed_table() {
    use ConfigSeedReport as R;
    use ConfigSeedSkipReason as K;
    let path = || PathBuf::from("C:/secret/path");
    let text = || "secret".to_string();
    let published = R::Published(ConfigSeedPublication {
        tier: ConfigSeedTier::MatrixBase,
        dest: path(),
        files: CollectedSeedFiles::Exact(Vec::new()),
        published_at: chrono::Utc::now(),
    });
    assert_eq!(seed_gate(None), Ok(()));
    assert_eq!(seed_gate(Some(&published)), Ok(()));
    assert_eq!(seed_gate(Some(&R::Skipped(K::NoSource))), Ok(()));
    let failures = [
        (
            R::Skipped(K::InvalidDestination {
                dest: path(),
                reason: text(),
            }),
            "the config seed destination is invalid",
        ),
        (
            R::Skipped(K::StaleReplica {
                replica_root: path(),
                reason: text(),
            }),
            "the agent replica is stale",
        ),
        (
            R::Skipped(K::DestinationInUse {
                dest: path(),
                error: text(),
            }),
            "the config seed destination is in use",
        ),
        (
            R::Skipped(K::GateUnavailable { reason: text() }),
            "the config seed gate is unavailable",
        ),
        (
            R::Failed(ConfigSeedFailure::Staging {
                source: path(),
                staging_path: path(),
                error: text(),
            }),
            "the config seed failed",
        ),
        (
            R::FailedAfterLogicalRemoval(ConfigSeedRollbackFailure::PreviousScopeStillStaged {
                scope: text(),
                trash_path: path(),
                install_error: text(),
                restore_error: text(),
            }),
            "the config seed failed after removing the previous configuration",
        ),
    ];
    for (report, reason) in failures {
        let message = seed_gate(Some(&report)).unwrap_err();
        assert_eq!(
            message,
            format!("Preparation gate failed: {reason}. {SUFFIX}")
        );
        assert!(!message.contains("secret"), "{message}");
    }
}

#[test]
fn shares_are_counted() {
    let state = Arc::new(PreflightState::default());
    let first = state.try_share("/tmp/a").expect("first share");
    let second = state.try_share("/tmp/a").expect("second share");
    assert_eq!(state.launch_count("/tmp/a"), 2);
    drop(first);
    assert_eq!(state.launch_count("/tmp/a"), 1);
    drop(second);
    assert_eq!(state.launch_count("/tmp/a"), 0);
    assert!(state.holds().is_empty(), "entry removed");
    if cfg!(windows) {
        assert_eq!(destination_key("C:\\A\\b"), destination_key("c:/a/B/"));
        let share = state.try_share("C:\\A\\b").expect("share");
        assert_eq!(state.launch_count("c:/a/B/"), 1);
        drop(share);
    }
}

#[tokio::test]
async fn defect_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let hook = crate::process::agent::ProbeSettlementHook {
        before_native_proof: None,
        release_native_proof: None,
        settlement_window: None,
        native_observations: [ProbeNativeObservation::Error("x")].into(),
        panic_after_native_proof_release: false,
        panic_after_native_query_defect: false,
        stdout_reader: None,
    };
    let outcome = with_probe_settlement_hook(hook, run_chain(&[ca("exit 0")], temp.path())).await;
    let report = &outcome.reports[0];
    assert_eq!(report.status, PreflightStageStatus::Passed);
    let defect = report.cleanup_defect.clone().expect("defect recorded");
    assert!(defect.contains('x'), "{defect}");
    assert!(!chain_passed(&outcome));
    let message = launch_failure_message(report);
    assert!(
        message.starts_with("Coding Agent preflight cleanup reported an error: "),
        "{message}"
    );
    assert_clean(&message);
}

#[tokio::test]
async fn superseded_replacement_runs_no_stage() {
    let cases: [(Result<bool, String>, &[&str]); 3] = [
        (Ok(false), &["revalidate"]),
        (Err("x".to_string()), &["revalidate"]),
        (Ok(true), &["revalidate", "prepare"]),
    ];
    for (verdict, expected_log) in cases {
        let temp = tempfile::tempdir().unwrap();
        let marker = "stage-ran.txt";
        let log: Arc<Mutex<Vec<&'static str>>> = Arc::default();
        let pty_mgr = logged_manager(log.clone());
        let stages = [ca(&format!("{} && exit 0", append_line(marker)))];
        let revalidate_log = log.clone();
        let current = verdict.clone();
        let revalidate = async move {
            revalidate_log.lock().unwrap().push("revalidate");
            current
        };
        let env = chain_env(temp.path());
        let result = run_chain_if_current(revalidate, &pty_mgr, &stages, &env).await;
        assert_eq!(log.lock().unwrap().as_slice(), expected_log);
        match (verdict, result) {
            (Ok(false), Ok(GatedChain::Superseded)) => {}
            (Err(expected), Err(got)) => assert_eq!(got, expected),
            (Ok(true), Ok(GatedChain::Ran(outcome))) => {
                assert_eq!(statuses(&outcome), [PreflightStageStatus::Passed]);
            }
            _ => panic!("unexpected gated chain result"),
        }
        let ran = temp.path().join(marker).exists();
        assert_eq!(ran, expected_log.len() == 2);
    }
}
