use super::*;

const WINDOW: Duration = Duration::from_millis(4000);

#[test]
fn busy_at_inject_is_not_applicable_regardless_of_signal_or_clock() {
    // A mid-turn wake cannot attribute a flip; preserve write-receipt.
    for signal in [false, true] {
        for elapsed in [Duration::ZERO, WINDOW, WINDOW * 2] {
            assert_eq!(
                consumption_verdict(false, signal, elapsed, WINDOW),
                ConsumptionVerdict::NotApplicable
            );
        }
    }
}

#[test]
fn idle_with_signal_is_observed() {
    assert_eq!(
        consumption_verdict(true, true, Duration::ZERO, WINDOW),
        ConsumptionVerdict::Observed
    );
    // Even at/after the window, a signal this tick is Observed.
    assert_eq!(
        consumption_verdict(true, true, WINDOW, WINDOW),
        ConsumptionVerdict::Observed
    );
}

#[test]
fn idle_without_signal_is_pending_in_window_and_at_terminal() {
    // In-window: keep waiting.
    assert_eq!(
        consumption_verdict(true, false, Duration::from_millis(10), WINDOW),
        ConsumptionVerdict::Pending
    );
    // Terminal tick (elapsed >= window): still Pending; the driver converts
    // this to the not-consumed Result, not the pure fn.
    assert_eq!(
        consumption_verdict(true, false, WINDOW, WINDOW),
        ConsumptionVerdict::Pending
    );
}

#[test]
fn verdict_to_result_maps_success_and_failure() {
    assert!(verdict_to_result(ConsumptionVerdict::Observed).is_ok());
    assert!(verdict_to_result(ConsumptionVerdict::NotApplicable).is_ok());
    assert!(verdict_to_result(ConsumptionVerdict::Pending).is_err());
}

#[test]
fn terminal_pending_error_is_actionable() {
    // The Err text feeds the CLI/dispatcher failure surface; keep it
    // descriptive and free of the payload (no injected body echoed back).
    let err = verdict_to_result(ConsumptionVerdict::Pending).unwrap_err();
    assert!(err.contains("not observed"));
}
