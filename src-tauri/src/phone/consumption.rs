//! (#1001 PR1) The wake-consumption oracle: pure decision core.
//!
//! Two dead oracles (dev-rust's bare `waiting_for_input` flip, §14.1's
//! post-submit activity gate) both observed the PTY OUTPUT stream, and the echo
//! of a pasted body IS output on that stream produced WITHOUT consuming the turn
//! (grinch G1/G2, architect §16.1). PR1 therefore does not pick a signal on
//! paper: it ships a GENERIC decision table and an `#[ignore]` harness that
//! measures which candidate signal actually predicts consumption against an
//! echo-immune, out-of-band ground truth. This module is that generic core plus
//! the shared verdict->Result conversion.
//!
//! Kept in a sibling module (allowed by plan §12) so the 12k-line `mailbox.rs`
//! is not the home for a pure, timer-free, PTY-free decision the harness and A
//! (PR3) both import unchanged.

use std::time::Duration;

/// The verdict of the consumption oracle for one observation tick.
///
/// Signal-agnostic on purpose (grinch G1 / §16.1): the driver supplies an opaque
/// `consumed_signal` computed from whichever candidate the PR1 harness selects;
/// this type never names a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsumptionVerdict {
    /// Idle at inject AND the chosen consumed-signal was observed within the
    /// window: consumption confirmed.
    Observed,
    /// Idle at inject, no consumed-signal yet. Returned every tick the session
    /// stays idle-without-signal; the driver converts a `Pending` that survives
    /// to `elapsed >= window` into the not-consumed result.
    Pending,
    /// Not idle at inject (busy / mid-turn). Consumption cannot be attributed to
    /// this wake, so the write-receipt (bias-to-deliver) semantics are preserved
    /// (§F2). `wake_action_for` injects into a busy session regardless.
    NotApplicable,
}

/// Pure per-tick decision for the consumption oracle. GENERIC: it must not
/// reference `activity_after_submit` or any specific signal (§16.1) - the driver
/// passes an opaque `consumed_signal`. Timer-free and PTY-free, so the whole
/// decision table is unit-testable.
///
/// `elapsed` and `window` are part of the signature mandated by §16.1 so this
/// single fn is the driver's loop primitive and A (PR3) imports it unchanged.
/// The fn itself does not branch on them: `idle + !signal` is `Pending` whether
/// or not the window is spent, and the DRIVER (which owns the clock) maps a
/// terminal `Pending` (survived to `elapsed >= window`) to not-consumed via
/// [`verdict_to_result`]. Keeping the clock in the driver, not here, is what
/// keeps the table signal-agnostic and trivially testable.
// PR1 lands this generic table and its enum ahead of the sole production
// caller (the A/PR3 oracle driver). It is exercised now by the unit tests
// below and by the `#[ignore]` measurement harness. Remove when A wires it.
#[allow(dead_code)]
pub(crate) fn consumption_verdict(
    idle_at_inject: bool,
    consumed_signal: bool,
    elapsed: Duration,
    window: Duration,
) -> ConsumptionVerdict {
    // Threaded for the mandated call-site shape; the driver, not this fn, owns
    // the terminal test against these (see doc above).
    let _ = (elapsed, window);
    if !idle_at_inject {
        ConsumptionVerdict::NotApplicable
    } else if consumed_signal {
        ConsumptionVerdict::Observed
    } else {
        ConsumptionVerdict::Pending
    }
}

/// The SINGLE shared verdict->Result conversion (grinch G6). Called by BOTH the
/// production path in `inject_wake_into_pty` and the `consumption_results` test
/// hook, so the deterministic AC3 test exercises the real conversion rather than
/// a hook-local copy. Pure, so it is unit-testable in isolation.
pub(crate) fn verdict_to_result(verdict: ConsumptionVerdict) -> Result<(), String> {
    match verdict {
        // Observed: consumption confirmed. NotApplicable: busy-at-inject, keep the
        // existing write-receipt semantics (bias-to-deliver). Both are success.
        ConsumptionVerdict::Observed | ConsumptionVerdict::NotApplicable => Ok(()),
        // A `Pending` that reached the driver's terminal tick means the window
        // elapsed with no consumed signal: surface an actionable failure so the
        // FS/DB retry machinery redelivers (A/PR3). In PR1 the production path
        // never reaches this arm - the verdict is always `NotApplicable` until A
        // wires a real oracle - but the hook path and A both rely on it.
        ConsumptionVerdict::Pending => {
            Err("wake injected but consumption was not observed within the window".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
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
}
