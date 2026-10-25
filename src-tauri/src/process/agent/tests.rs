use super::*;
use std::path::Path;
use std::time::Duration;

use futures::FutureExt;

fn shell() -> &'static Path {
    Path::new(if cfg!(windows) { "cmd.exe" } else { "sh" })
}

fn args(script: &str) -> Vec<&str> {
    if cfg!(windows) {
        vec!["/D", "/C", script]
    } else {
        vec!["-c", script]
    }
}

fn immediate_script() -> &'static str {
    "echo 1.2.3"
}

fn blocked_script() -> &'static str {
    if cfg!(windows) {
        "ping -n 30 127.0.0.1 >NUL"
    } else {
        "sleep 30"
    }
}

#[tokio::test]
async fn callback_panic_retains_installed_owner_until_cleanup() {
    let (_cancel_tx, mut cancel) = tokio::sync::watch::channel(false);
    let mut owner = RetainedProbeOwner::default();
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    let argv = args(blocked_script());
    let result = std::panic::AssertUnwindSafe(probe_version_cancellable_retained(
        shell(),
        &argv,
        Duration::from_secs(30),
        &search_path,
        &mut cancel,
        &mut owner,
        || panic!("owner-installed callback"),
        || panic!("no cancellation requested"),
    ))
    .catch_unwind()
    .await;
    assert!(result.is_err());
    assert!(owner.is_some(), "callback ran after owner installation");
    let mut observed = false;
    let defect = tokio::time::timeout(
        Duration::from_secs(15),
        owner.settle_after_panic(&mut cancel, || observed = true),
    )
    .await
    .expect("retained child and readers settle");
    assert!(observed);
    assert_eq!(defect, None);
    assert!(
        owner.is_some(),
        "settlement does not clear retained storage"
    );
    owner.clear();
    assert!(!owner.is_some());
}

#[tokio::test]
async fn cancellation_settles_before_return_and_preserves_retained_slot() {
    let (cancel_tx, mut cancel) = tokio::sync::watch::channel(false);
    let mut owner = RetainedProbeOwner::default();
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    let argv = args(blocked_script());
    let mut installed = false;
    let mut observed = false;
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        probe_version_cancellable_retained(
            shell(),
            &argv,
            Duration::from_secs(30),
            &search_path,
            &mut cancel,
            &mut owner,
            || {
                installed = true;
                cancel_tx.send(true).expect("cancel installed owner");
            },
            || observed = true,
        ),
    )
    .await
    .expect("cancelled owner settles");
    assert_eq!(result, CancellableProbeOutcome::Cancelled);
    assert!(installed && observed);
    assert!(owner.is_some());
    owner.clear();
}

#[tokio::test]
async fn output_after_reader_cap_is_drained_but_not_parsed() {
    let control = if cfg!(windows) {
        "(for /L %i in (1,1,2) do @echo.) & echo 1.2.3"
    } else {
        "printf '%2s\n1.2.3\n' ''"
    };
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    assert_eq!(
        probe_version_in(
            shell(),
            &args(control),
            Duration::from_secs(10),
            &search_path
        )
        .await,
        ProbeOutcome::Version("1.2.3".into())
    );
    let script = if cfg!(windows) {
        "(for /L %i in (1,1,3000) do @echo.) & echo 1.2.3"
    } else {
        "printf '%5000s\n1.2.3\n' ''"
    };
    let result = probe_version_in(
        shell(),
        &args(script),
        Duration::from_secs(10),
        &std::env::var_os("PATH").unwrap_or_default(),
    )
    .await;
    assert_eq!(
        result,
        ProbeOutcome::Failed("no version in output: <empty>".into())
    );
}

#[tokio::test]
async fn timeout_kills_and_settles_owner() {
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        probe_version_in(
            shell(),
            &args(blocked_script()),
            Duration::from_millis(100),
            &std::env::var_os("PATH").unwrap_or_default(),
        ),
    )
    .await
    .expect("timeout settles native owner and readers");
    assert_eq!(
        result,
        ProbeOutcome::Failed("timed out after 0s (killed)".into())
    );
}

#[tokio::test]
async fn native_settlement_panic_can_resume_from_retained_owner() {
    let (_cancel_tx, mut cancel) = tokio::sync::watch::channel(false);
    let mut owner = RetainedProbeOwner::default();
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    let argv = args(immediate_script());
    let result = with_probe_settlement_hook(
        ProbeSettlementHook {
            before_native_proof: None,
            release_native_proof: None,
            settlement_window: None,
            native_observations: Default::default(),
            panic_after_native_proof_release: true,
            panic_after_native_query_defect: false,
            stdout_reader: None,
        },
        std::panic::AssertUnwindSafe(probe_version_cancellable_retained(
            shell(),
            &argv,
            Duration::from_secs(10),
            &search_path,
            &mut cancel,
            &mut owner,
            || {},
            || {},
        ))
        .catch_unwind(),
    )
    .await;
    assert!(result.is_err());
    assert!(owner.is_some());
    let defect = tokio::time::timeout(
        Duration::from_secs(15),
        owner.settle_after_panic(&mut cancel, || {}),
    )
    .await
    .expect("native proof resumes after panic");
    assert_eq!(defect, None);
    assert!(owner.is_some());
    owner.clear();
}
