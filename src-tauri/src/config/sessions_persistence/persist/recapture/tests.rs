use super::*;
use tokio::time::Instant;

fn new_lines(before: usize, needle: &str) -> Vec<String> {
    crate::logging::test_tee_snapshot()[before..]
        .iter()
        .filter(|line| line.contains(needle))
        .cloned()
        .collect()
}

#[tokio::test(start_paused = true)]
async fn backoff_yields_without_sleeping_within_spin_budget() {
    crate::logging::test_install_logger();
    let before = crate::logging::test_tee_snapshot().len();
    let mut backoff = RecaptureBackoff::new("test_spin_4929");
    let start = Instant::now();
    for _ in 0..64 {
        backoff.wait("r").await;
    }
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert_eq!(backoff.attempts, 64);
    assert!(new_lines(before, "test_spin_4929").is_empty());
}

#[tokio::test(start_paused = true)]
async fn backoff_sleeps_with_capped_exponential_delay() {
    let mut backoff = RecaptureBackoff::new("test_delay_4929");
    for _ in 0..64 {
        backoff.wait("r").await;
    }
    for expected in [25, 50, 100, 200, 400, 800, 1000, 1000] {
        let start = Instant::now();
        backoff.wait("r").await;
        assert_eq!(start.elapsed(), Duration::from_millis(expected));
    }
}

#[tokio::test(start_paused = true)]
async fn backoff_warns_once_after_sustained_non_convergence() {
    crate::logging::test_install_logger();
    let before = crate::logging::test_tee_snapshot().len();
    let mut backoff = RecaptureBackoff::new("test_warn_4929");
    for _ in 0..64 + 5 {
        backoff.wait("r").await;
    }
    assert!(new_lines(before, "site=test_warn_4929").is_empty());
    backoff.wait("r").await;
    let lines = new_lines(before, "site=test_warn_4929");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("WARN "), "{lines:?}");
    assert!(lines[0].contains("reason=r"), "{lines:?}");
    assert!(lines[0].contains("attempts=70"), "{lines:?}");
    for _ in 0..20 {
        backoff.wait("r").await;
    }
    assert_eq!(new_lines(before, "site=test_warn_4929").len(), 1);
}

#[tokio::test(start_paused = true)]
async fn park_unreadable_logs_before_parking() {
    crate::logging::test_install_logger();
    let before = crate::logging::test_tee_snapshot().len();
    let parked = tokio::time::timeout(
        Duration::from_secs(1),
        park_unreadable::<()>("test_park_4929"),
    )
    .await;
    assert!(parked.is_err());
    let lines = new_lines(before, "site=test_park_4929");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("ERROR "), "{lines:?}");
}
