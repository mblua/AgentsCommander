use super::*;
use std::time::Duration;

fn probe() -> u8 {
    panic!("#4929 ambiguous probe")
}

#[tokio::test(start_paused = true)]
async fn owned_persist_waiter_reports_ambiguous_end() {
    crate::logging::test_install_logger();
    let before = crate::logging::test_tee_snapshot().len();
    let waited = tokio::time::timeout(
        Duration::from_secs(1),
        run_owned_persist_core(async { probe() }),
    )
    .await;
    assert!(waited.is_err());
    let reported = crate::logging::test_tee_snapshot()[before..]
        .iter()
        .filter(|line| {
            line.starts_with("ERROR ") && line.contains("persist operation ended without a result")
        })
        .count();
    assert!(reported >= 1);
}
