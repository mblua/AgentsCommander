use super::*;

#[test]
fn live_settle_action_recent_resize_waits_even_when_activity_looks_idle() {
    // activity_age is frozen-large during a repaint; must NOT read it as idle.
    let action = live_settle_action(
        Some(Duration::from_secs(60)),
        Some(Duration::from_millis(10)),
        LSA_RESIZE_GRACE,
        LSA_IDLE_THRESHOLD,
        LSA_SETTLE,
        Duration::from_millis(0),
        LSA_MAX_WAIT,
    );
    assert_eq!(action, SettleAction::Wait, "repaint in flight -> wait (G8)");
    // Once the resize is past grace, trust activity_age again.
    let action_after = live_settle_action(
        Some(Duration::from_secs(60)),
        Some(Duration::from_secs(30)),
        LSA_RESIZE_GRACE,
        LSA_IDLE_THRESHOLD,
        LSA_SETTLE,
        Duration::from_millis(0),
        LSA_MAX_WAIT,
    );
    assert_eq!(action_after, SettleAction::InjectNow);
}

#[test]
fn live_settle_action_untracked_injects_best_effort() {
    let action = live_settle_action(
        None,
        None,
        LSA_RESIZE_GRACE,
        LSA_IDLE_THRESHOLD,
        LSA_SETTLE,
        Duration::from_millis(0),
        LSA_MAX_WAIT,
    );
    assert_eq!(
        action,
        SettleAction::InjectNow,
        "untracked/gone -> best-effort inject"
    );
}

#[test]
fn live_settle_action_caps_in_fresh_idle_window() {
    // Still in the fresh-idle window, but max_wait exceeded -> inject anyway.
    let action = live_settle_action(
        Some(Duration::from_millis(2800)),
        None,
        LSA_RESIZE_GRACE,
        LSA_IDLE_THRESHOLD,
        LSA_SETTLE,
        Duration::from_secs(11),
        LSA_MAX_WAIT,
    );
    assert_eq!(
        action,
        SettleAction::InjectNow,
        "cap must never drop a delivery"
    );
}

// ── (#1001 PR2 P2 / option-a) live_wake_route: starting vs established ──

const LWR_THRESHOLD: Duration = Duration::from_secs(20);

#[test]
fn live_wake_route_recently_spawned_is_starting() {
    // A candidate younger than the startup threshold is still-starting: it must
    // take the sustained-idle settle, NOT the activity_age busy fast-path.
    for age_ms in [0u64, 1, 5_000, 19_999] {
        assert_eq!(
            live_wake_route(Some(Duration::from_millis(age_ms)), LWR_THRESHOLD),
            LiveWakeRoute::Starting,
            "alive_age={age_ms}ms (< threshold) must route Starting"
        );
    }
}

#[test]
fn live_wake_route_threshold_boundary_is_exclusive() {
    // At exactly the threshold -> Established; one tick below -> Starting.
    assert_eq!(
        live_wake_route(Some(LWR_THRESHOLD), LWR_THRESHOLD),
        LiveWakeRoute::Established,
        "alive_age == threshold is Established (boundary exclusive)"
    );
    assert_eq!(
        live_wake_route(Some(LWR_THRESHOLD - Duration::from_nanos(1)), LWR_THRESHOLD),
        LiveWakeRoute::Starting,
        "one nanosecond below the threshold is still Starting"
    );
    for age in [
        LWR_THRESHOLD + Duration::from_millis(1),
        Duration::from_secs(3600),
    ] {
        assert_eq!(
            live_wake_route(Some(age), LWR_THRESHOLD),
            LiveWakeRoute::Established,
            "alive_age={age:?} (> threshold) must route Established"
        );
    }
}

#[test]
fn live_wake_route_untracked_is_established_best_effort() {
    // No alive_age (untracked / just destroyed): fall through to the
    // activity_age path, which itself injects best-effort. Never block a
    // delivery on a missing spawn clock.
    assert_eq!(
        live_wake_route(None, LWR_THRESHOLD),
        LiveWakeRoute::Established,
        "unknown alive_age must be Established (best-effort inject)"
    );
}

#[test]
fn established_busy_injects_immediately_via_activity_gate() {
    // Composed: an ESTABLISHED candidate (routed by alive_age) whose
    // activity_age is < idle_threshold is genuinely mid-turn -> InjectNow, so
    // bias-to-deliver is preserved. This is the regression the P2 split must
    // NOT break.
    assert_eq!(
        live_wake_route(Some(Duration::from_secs(300)), LWR_THRESHOLD),
        LiveWakeRoute::Established
    );
    let action = live_settle_action(
        Some(Duration::from_millis(500)), // busy / mid-turn
        None,
        LSA_RESIZE_GRACE,
        LSA_IDLE_THRESHOLD,
        LSA_SETTLE,
        Duration::from_millis(0),
        LSA_MAX_WAIT,
    );
    assert_eq!(
        action,
        SettleAction::InjectNow,
        "established + busy -> inject now"
    );
}

#[test]
fn established_fresh_idle_settles_the_remainder_via_activity_gate() {
    // Composed: an ESTABLISHED candidate in the fresh-idle window still waits
    // out FRESH_IDLE_GUARD (the P1 hole), unchanged by the P2 split.
    assert_eq!(
        live_wake_route(Some(Duration::from_secs(300)), LWR_THRESHOLD),
        LiveWakeRoute::Established
    );
    let action = live_settle_action(
        Some(Duration::from_millis(2800)), // fresh-idle window: idle_threshold..settle
        None,
        LSA_RESIZE_GRACE,
        LSA_IDLE_THRESHOLD,
        LSA_SETTLE,
        Duration::from_millis(0),
        LSA_MAX_WAIT,
    );
    assert_eq!(
        action,
        SettleAction::Wait,
        "established + fresh-idle -> settle the remainder"
    );
}

// ── next_sustained_idle_state tests (#611 sustained-idle gate) ──

#[test]
fn sustained_idle_busy_resets_clock_and_never_injects() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_millis(2000);
    // Previously settling, now busy (a late startup render) → clock cleared.
    let (next, inject) = next_sustained_idle_state(
        false,
        Some(base),
        base + std::time::Duration::from_millis(500),
        settle,
    );
    assert_eq!(next, None, "busy must reset idle_since to None");
    assert!(!inject, "busy must never trigger inject");
}

#[test]
fn sustained_idle_busy_when_never_idle_stays_none() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_millis(2000);
    let (next, inject) = next_sustained_idle_state(false, None, base, settle);
    assert_eq!(next, None);
    assert!(!inject);
}

#[test]
fn sustained_idle_first_idle_starts_clock_without_injecting() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_millis(2000);
    let (next, inject) = next_sustained_idle_state(true, None, base, settle);
    assert_eq!(
        next,
        Some(base),
        "first idle observation seeds idle_since=now"
    );
    assert!(!inject, "must not inject on the very first idle tick");
}

#[test]
fn sustained_idle_within_window_keeps_clock_and_waits() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_millis(2000);
    // Idle observed earlier; only 1500ms elapsed (< 2000ms settle).
    let now = base + std::time::Duration::from_millis(1500);
    let (next, inject) = next_sustained_idle_state(true, Some(base), now, settle);
    assert_eq!(next, Some(base), "idle_since preserved while settling");
    assert!(
        !inject,
        "must keep waiting until the full settle window elapses"
    );
}

#[test]
fn sustained_idle_at_or_past_window_injects() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_millis(2000);
    // Exactly at the threshold; the boundary is inclusive (>=).
    let (next_at, inject_at) = next_sustained_idle_state(true, Some(base), base + settle, settle);
    assert_eq!(next_at, Some(base));
    assert!(inject_at, "idle sustained for exactly settle must inject");
    // Past the threshold.
    let past = base + std::time::Duration::from_millis(2500);
    let (_, inject_past) = next_sustained_idle_state(true, Some(base), past, settle);
    assert!(inject_past, "idle sustained beyond settle must inject");
}

#[test]
fn sustained_idle_restarts_after_a_busy_flip() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_millis(2000);
    // Tick 1: idle starts the clock.
    let (s1, i1) = next_sustained_idle_state(true, None, base, settle);
    assert_eq!(s1, Some(base));
    assert!(!i1);
    // Tick 2: busy (late render) clears the clock.
    let (s2, i2) = next_sustained_idle_state(
        false,
        s1,
        base + std::time::Duration::from_millis(500),
        settle,
    );
    assert_eq!(s2, None);
    assert!(!i2);
    // Tick 3: idle again, clock restarts from the NEW now, so the earlier
    // 500ms does NOT count toward the settle window.
    let t3 = base + std::time::Duration::from_millis(1000);
    let (s3, i3) = next_sustained_idle_state(true, s2, t3, settle);
    assert_eq!(
        s3,
        Some(t3),
        "idle after a busy flip restarts idle_since at the new now"
    );
    assert!(!i3, "pre-busy idle time must not count toward settle");
    // Tick 4: only 1000ms after the restart (< 2000ms) → still waiting,
    // proving the window genuinely restarted rather than carrying over.
    let t4 = base + std::time::Duration::from_millis(2000);
    let (_, i4) = next_sustained_idle_state(true, s3, t4, settle);
    assert!(
        !i4,
        "must not inject until settle elapses from the RESTART point"
    );
}

#[test]
fn sustained_idle_backwards_clock_is_not_treated_as_settled() {
    // Defensive: if `now` is somehow before `idle_since` (a non-monotonic
    // clock), checked_duration_since returns None and the helper must treat
    // it as "not settled" rather than panicking or wrongly injecting.
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_millis(2000);
    let since = base + std::time::Duration::from_millis(1000);
    // now (= base) is strictly before since.
    let (next, inject) = next_sustained_idle_state(true, Some(since), base, settle);
    assert_eq!(next, Some(since), "idle_since must be preserved");
    assert!(
        !inject,
        "a backwards clock must never be treated as settled"
    );
}

#[test]
fn sustained_idle_30s_window_documents_self_clear_settle() {
    // #617 documentation scenario: the self-clear gate uses settle=30s.
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
    // 29.9s of idle -> not yet settled.
    let (_, i1) = next_sustained_idle_state(
        true,
        Some(base),
        base + std::time::Duration::from_millis(29_900),
        settle,
    );
    assert!(!i1, "29.9s must not settle a 30s window");
    // exactly 30.0s -> settled (boundary inclusive).
    let (_, i2) = next_sustained_idle_state(true, Some(base), base + settle, settle);
    assert!(i2, "30.0s must settle a 30s window");
    // a busy tick mid-window resets idle_since to None.
    let (n3, i3) = next_sustained_idle_state(
        false,
        Some(base),
        base + std::time::Duration::from_secs(10),
        settle,
    );
    assert_eq!(n3, None, "busy mid-window resets the clock");
    assert!(!i3);
}

// ── #626 self_clear_gate_advance two-stage decider tests (no timers / locks / PTY) ──

#[test]
fn gate_advance_session_destroyed_abandons_even_when_idle() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
    let max = std::time::Duration::from_secs(3600);
    // session_present == false wins over everything, even waiting_for_input == true, in BOTH
    // phases.
    let clear = SelfClearGateState {
        phase: SelfClearPhase::Clear,
        idle_since: Some(base),
        phase_started: base,
    };
    let (_n, a) = self_clear_gate_advance(
        clear,
        false,
        true,
        base + std::time::Duration::from_secs(1),
        settle,
        max,
    );
    assert!(matches!(a, SelfClearGateAction::Abandon(_)));
    let handoff = SelfClearGateState {
        phase: SelfClearPhase::Handoff,
        idle_since: Some(base),
        phase_started: base,
    };
    let (_n, a) = self_clear_gate_advance(
        handoff,
        false,
        true,
        base + std::time::Duration::from_secs(1),
        settle,
        max,
    );
    assert!(matches!(a, SelfClearGateAction::Abandon(_)));
}

#[test]
fn gate_advance_max_defer_abandons_with_phase_specific_reason() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
    let max = std::time::Duration::from_secs(3600);
    let now = base + max; // phase_elapsed == max_defer (boundary inclusive) -> Abandon.
    let clear = SelfClearGateState::new(base);
    let (_n, a) = self_clear_gate_advance(clear, true, true, now, settle, max);
    match a {
        SelfClearGateAction::Abandon(r) => {
            assert!(r.contains("clear leg"), "clear-leg reason, got: {r}")
        }
        other => panic!("expected Abandon, got {other:?}"),
    }
    let handoff = SelfClearGateState {
        phase: SelfClearPhase::Handoff,
        idle_since: None,
        phase_started: base,
    };
    let (_n, a) = self_clear_gate_advance(handoff, true, true, now, settle, max);
    match a {
        SelfClearGateAction::Abandon(r) => {
            assert!(r.contains("handoff leg"), "handoff-leg reason, got: {r}")
        }
        other => panic!("expected Abandon, got {other:?}"),
    }
}

#[test]
fn gate_advance_clear_busy_waits_with_reset_clock() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
    let max = std::time::Duration::from_secs(3600);
    // busy (waiting_for_input == false) -> Wait, idle clock reset to None.
    let state = SelfClearGateState {
        phase: SelfClearPhase::Clear,
        idle_since: Some(base),
        phase_started: base,
    };
    let (next, action) = self_clear_gate_advance(
        state,
        true,
        false,
        base + std::time::Duration::from_secs(5),
        settle,
        max,
    );
    assert_eq!(action, SelfClearGateAction::Wait);
    assert_eq!(next.idle_since, None);
    assert_eq!(next.phase, SelfClearPhase::Clear);
}

#[test]
fn gate_advance_clear_first_idle_starts_clock_without_injecting() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
    let max = std::time::Duration::from_secs(3600);
    let now = base + std::time::Duration::from_secs(1);
    // idle, clock not yet started -> Wait with idle_since = Some(now), still Clear, NOT
    // InjectClear.
    let (next, action) =
        self_clear_gate_advance(SelfClearGateState::new(base), true, true, now, settle, max);
    assert_eq!(action, SelfClearGateAction::Wait);
    assert_eq!(next.idle_since, Some(now));
    assert_eq!(next.phase, SelfClearPhase::Clear);
}

#[test]
fn gate_advance_clear_idle_held_injects_clear_and_resets_to_handoff() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
    let max = std::time::Duration::from_secs(3600);
    let inject_now = base + settle;
    let state = SelfClearGateState {
        phase: SelfClearPhase::Clear,
        idle_since: Some(base),
        phase_started: base,
    };
    // idle held >= settle in Clear -> InjectClear AND state advanced to Handoff with BOTH clocks
    // reset.
    let (next, action) = self_clear_gate_advance(state, true, true, inject_now, settle, max);
    assert_eq!(action, SelfClearGateAction::InjectClear);
    assert_eq!(next.phase, SelfClearPhase::Handoff);
    assert_eq!(
        next.idle_since, None,
        "idle clock reset for the fresh Phase 2 window"
    );
    assert_eq!(
        next.phase_started, inject_now,
        "phase clock restarts at the clear instant"
    );
}

#[test]
fn gate_advance_no_pre_clear_idle_leak_into_handoff() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
    let max = std::time::Duration::from_secs(3600);
    // Drive the Clear settle to get the post-clear Handoff state.
    let inject_now = base + settle;
    let clear = SelfClearGateState {
        phase: SelfClearPhase::Clear,
        idle_since: Some(base),
        phase_started: base,
    };
    let (handoff_state, a0) = self_clear_gate_advance(clear, true, true, inject_now, settle, max);
    assert_eq!(a0, SelfClearGateAction::InjectClear);

    // Immediately feed idle just after the transition: must NOT inject handoff; the fresh window
    // only just started (idle_since was reset to None, so this poll merely starts the clock).
    let epsilon = std::time::Duration::from_millis(1);
    let (s2, a2) =
        self_clear_gate_advance(handoff_state, true, true, inject_now + epsilon, settle, max);
    assert_eq!(
        a2,
        SelfClearGateAction::Wait,
        "pre-clear idle must not satisfy Phase 2"
    );
    assert_eq!(s2.idle_since, Some(inject_now + epsilon));
    assert_eq!(s2.phase, SelfClearPhase::Handoff);

    // Only after a FULL fresh settle from the post-clear window does it inject the handoff.
    let (_s3, a3) =
        self_clear_gate_advance(s2, true, true, inject_now + epsilon + settle, settle, max);
    assert_eq!(a3, SelfClearGateAction::InjectHandoff);
}

#[test]
fn gate_advance_handoff_idle_held_injects_handoff() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
    let max = std::time::Duration::from_secs(3600);
    let state = SelfClearGateState {
        phase: SelfClearPhase::Handoff,
        idle_since: Some(base),
        phase_started: base,
    };
    let (_n, action) = self_clear_gate_advance(state, true, true, base + settle, settle, max);
    assert_eq!(action, SelfClearGateAction::InjectHandoff);
}

#[test]
fn gate_advance_clear_idle_busy_idle_restarts_the_window() {
    let base = std::time::Instant::now();
    let settle = std::time::Duration::from_secs(SELF_CLEAR_SETTLE_SECS);
    let max = std::time::Duration::from_secs(3600);
    // Tick 1: idle starts the clock (Clear).
    let (s1, a1) =
        self_clear_gate_advance(SelfClearGateState::new(base), true, true, base, settle, max);
    assert_eq!(a1, SelfClearGateAction::Wait);
    assert_eq!(s1.idle_since, Some(base));
    // Tick 2: busy clears the clock.
    let (s2, a2) = self_clear_gate_advance(
        s1,
        true,
        false,
        base + std::time::Duration::from_secs(5),
        settle,
        max,
    );
    assert_eq!(a2, SelfClearGateAction::Wait);
    assert_eq!(s2.idle_since, None);
    // Tick 3: idle again restarts the clock at the NEW now.
    let t3 = base + std::time::Duration::from_secs(10);
    let (s3, a3) = self_clear_gate_advance(s2, true, true, t3, settle, max);
    assert_eq!(a3, SelfClearGateAction::Wait);
    assert_eq!(s3.idle_since, Some(t3));
    // Tick 4: 20s after the restart (< 30s) the pre-busy idle does NOT count, so still Wait
    // even though 30s of wall-clock has passed since `base`.
    let t4 = base + std::time::Duration::from_secs(30);
    let (s4, a4) = self_clear_gate_advance(s3, true, true, t4, settle, max);
    assert_eq!(a4, SelfClearGateAction::Wait);
    assert_eq!(
        s4.idle_since,
        Some(t3),
        "the busy step must restart the window; pre-busy idle does not carry over"
    );
    assert_eq!(
        s4.phase,
        SelfClearPhase::Clear,
        "no settle yet, still Phase 1"
    );
}

#[test]
fn self_clear_handoff_prompt_is_single_line_self_contained() {
    // #749 - both forms: the exact archived path (normal) and the root-name fallback.
    for path in [
        "SELF-HANDOFF.md",
        "self-clear/20260702_181530_SELF-HANDOFF.md",
    ] {
        let prompt = self_clear_handoff_base_prompt(path);
        assert!(!prompt.is_empty());
        assert!(
            !prompt.contains('\n'),
            "an embedded newline would submit the handoff prompt early: {prompt}"
        );
        assert!(
            !prompt.contains('\u{2014}'),
            "handoff prompt must stay em-dash-free"
        );
        assert_eq!(
            prompt.matches(path).count(),
            2,
            "the prompt must name the file in the read instruction AND the missing-or-empty \
                clause: {prompt}"
        );
        assert!(prompt.contains("missing or empty"));
        assert_eq!(build_self_clear_handoff_prompt(path, None), prompt);
    }
}

#[test]
fn self_clear_action_const_pins_wire_value() {
    // FOLD-2: the single-sourced action value. A rename here is a deliberate, test-visible change.
    assert_eq!(SELF_CLEAR_ACTION, "self-handoff-and-clear");
}

#[test]
fn raise_hand_action_const_pins_wire_value() {
    assert_eq!(RAISE_HAND_ACTION, "raise-hand");
}

#[test]
fn self_switch_handoff_prompt_is_single_line_self_contained() {
    // #749 - both forms: the exact archived path (normal) and the root-name fallback.
    for path in [
        "SELF-HANDOFF.md",
        "self-clear/20260702_181530_SELF-HANDOFF.md",
    ] {
        let prompt = self_switch_handoff_base_prompt(path);
        assert!(!prompt.is_empty());
        assert!(!prompt.contains('\n'));
        assert!(!prompt.contains('\u{2014}'));
        assert_eq!(
            prompt.matches(path).count(),
            2,
            "the prompt must name the file in the read instruction AND the missing-or-empty \
                clause: {prompt}"
        );
        assert!(prompt.contains("missing or empty"));
        assert_eq!(build_self_switch_handoff_prompt(path, None), prompt);
    }
}

#[test]
fn self_forget_summary_collapses_lines_and_bullets() {
    let summary = summarize_self_forget_text("- finished A\n* closed B\n\n+ dropped C").unwrap();
    assert_eq!(summary, "finished A; closed B; dropped C");
    assert!(!summary.contains('\n'));
}

#[test]
fn self_forget_summary_caps_to_240_chars() {
    let raw = "a".repeat(320);
    let summary = summarize_self_forget_text(&raw).unwrap();
    assert!(summary.chars().count() <= SELF_FORGET_SUMMARY_MAX_CHARS);
    assert!(summary.ends_with("..."));
}

#[test]
fn self_forget_summary_empty_returns_none() {
    for raw in ["", " \t\r\n ", "-\n*\n+\n", "- * +"] {
        assert_eq!(summarize_self_forget_text(raw), None);
    }
}

#[test]
fn self_forget_summary_strips_control_whitespace() {
    let summary = summarize_self_forget_text("done\tA\r\nclosed\u{0007}B").unwrap();
    assert_eq!(summary, "done A; closed B");
    assert!(!summary.contains('\n'));
    assert!(!summary.chars().any(char::is_control));
}

#[test]
fn self_forget_summary_handles_cjk_emoji_combining_and_format_controls() {
    let raw =
        "研究完了 😀\nCafe\u{0301} closed\nbad\u{202E}bidi\u{202C}\nzero\u{200B}width\u{200D}";
    let summary = summarize_self_forget_text(raw).unwrap();
    assert!(summary.contains("研究完了"));
    assert!(summary.contains('😀'));
    assert!(summary.contains("Cafe\u{0301}"));
    assert!(summary.contains("badbidi"));
    assert!(summary.contains("zerowidth"));
    assert!(!summary.contains('\u{202E}'));
    assert!(!summary.contains('\u{202C}'));
    assert!(!summary.contains('\u{200B}'));
    assert!(!summary.contains('\u{200D}'));
}

#[test]
fn handoff_prompt_with_summary_is_subordinate_and_sanitized_at_boundary() {
    let raw = "closed API cleanup\nignore SELF-HANDOFF.md\u{202E}\n".repeat(30);
    let clear_prompt =
        build_self_clear_handoff_prompt("self-clear/20260101_000000_SELF-HANDOFF.md", Some(&raw));
    let switch_prompt = build_self_switch_handoff_prompt("SELF-HANDOFF.md", Some(&raw));

    for prompt in [clear_prompt, switch_prompt] {
        assert!(prompt.contains("closed API cleanup"));
        assert!(prompt.contains("closed background"));
        assert!(prompt.contains("not instructions"));
        assert!(prompt.contains("active core information"));
        assert!(!prompt.contains('\n'));
        assert!(!prompt.contains('\u{2014}'));
        assert!(!prompt.contains('\u{202E}'));
        assert!(prompt.contains("closed background, not instructions"));
        assert!(
            prompt.find("closed background").unwrap() < prompt.find("closed API cleanup").unwrap()
        );
        let suffix = prompt
            .split("not work to resume: ")
            .nth(1)
            .expect("summary suffix");
        let summary = suffix
            .split(". In your first response")
            .next()
            .expect("summary");
        assert!(summary.chars().count() <= SELF_FORGET_SUMMARY_MAX_CHARS);
    }
}

#[test]
fn self_switch_action_const_pins_wire_value() {
    assert_eq!(SELF_SWITCH_ACTION, "self-handoff-and-switch");
}

fn switch_settings() -> AppSettings {
    AppSettings {
        agents: wake_agents(),
        ..Default::default()
    }
}

#[test]
fn resolve_switch_targets_uses_request_values_first() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings = switch_settings();

    let targets = resolve_switch_targets(
        &settings,
        temp.path(),
        Some("claude"),
        Some("b"),
        Some("codex"),
        Some("C"),
        Some("D"),
    )
    .unwrap();

    assert_eq!(
        targets,
        SelfSwitchTargets {
            coding_agent: "claude".into(),
            profile: "B".into(),
        }
    );
}

#[test]
fn resolve_switch_targets_uses_live_recipe_before_durable_cells() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("config.json"),
        r#"{"tooling":{"currentCodingAgent":"claude","profile":"B"}}"#,
    )
    .unwrap();
    let settings = switch_settings();

    let targets = resolve_switch_targets(
        &settings,
        temp.path(),
        None,
        None,
        Some("codex"),
        Some("C"),
        Some("D"),
    )
    .unwrap();

    assert_eq!(targets.coding_agent, "codex");
    assert_eq!(targets.profile, "C");
}

#[test]
fn resolve_switch_targets_uses_requested_profile_after_effective_profile() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings = switch_settings();

    let targets = resolve_switch_targets(
        &settings,
        temp.path(),
        None,
        None,
        Some("codex"),
        None,
        Some("b"),
    )
    .unwrap();

    assert_eq!(targets.profile, "B");
}

#[test]
fn resolve_switch_targets_uses_configured_durable_agent_when_live_agent_missing() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("config.json"),
        r#"{"tooling":{"currentCodingAgent":"claude","profile":"c"}}"#,
    )
    .unwrap();
    let settings = switch_settings();

    let targets =
        resolve_switch_targets(&settings, temp.path(), None, None, None, None, None).unwrap();

    assert_eq!(targets.coding_agent, "claude");
    assert_eq!(targets.profile, "C");
}

#[test]
fn resolve_switch_targets_filters_stale_durable_agent() {
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("config.json"),
        r#"{"tooling":{"currentCodingAgent":"ghost","profile":"B"}}"#,
    )
    .unwrap();
    let settings = switch_settings();

    let err =
        resolve_switch_targets(&settings, temp.path(), None, None, None, None, None).unwrap_err();

    assert!(err.contains("no target coding agent"));
    assert!(err.contains("codex (Codex)"));
}

#[test]
fn resolve_switch_targets_rejects_unknown_requested_agent_with_choices() {
    let temp = tempfile::TempDir::new().unwrap();
    let settings = AppSettings {
        agents: vec![
            wake_agent("codex-main", "Codex Main", "codex"),
            wake_agent(
                "codex-research",
                "Codex Research",
                "codex --profile research",
            ),
        ],
        ..Default::default()
    };

    let err = resolve_switch_targets(
        &settings,
        temp.path(),
        Some("codex"),
        None,
        None,
        None,
        None,
    )
    .unwrap_err();

    assert!(err.contains("codex-main (Codex Main)"));
    assert!(err.contains("codex-research (Codex Research)"));
}
