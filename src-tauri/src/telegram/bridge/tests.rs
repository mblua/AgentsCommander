use super::*;

#[test]
fn poll_backoff_uses_plan_bounds_and_session_salt() {
    let mut first = new_poll_backoff("session-a");
    let mut second = new_poll_backoff("session-b");

    let first_delay = first.next_delay();
    let second_delay = second.next_delay();

    assert!(first_delay >= Duration::from_secs(3));
    assert!(second_delay >= Duration::from_secs(3));
    assert!(first_delay <= Duration::from_secs(60));
    assert!(second_delay <= Duration::from_secs(60));
    assert_ne!(first_delay, second_delay);
}

#[test]
fn network_policy_first_failure_uses_warn_default() {
    let mut state = PollNetworkErrorState::new();
    let policy = TelegramNetworkPollErrorLogging::default();

    assert_eq!(
        state.record_network_failure(&policy),
        PollNetworkLogAction::Failure {
            level: log::Level::Warn,
            suppressed_since_last_emit: 0,
            sustained: false,
        }
    );
}

#[test]
fn network_policy_repeated_transient_uses_debug_default() {
    let mut state = PollNetworkErrorState::new();
    let policy = TelegramNetworkPollErrorLogging::default();

    let _ = state.record_network_failure(&policy);
    assert_eq!(
        state.record_network_failure(&policy),
        PollNetworkLogAction::Suppressed {
            level: log::Level::Debug
        }
    );
}

#[test]
fn network_policy_recovery_emits_once_after_failure() {
    let mut state = PollNetworkErrorState::new();
    let policy = TelegramNetworkPollErrorLogging::default();

    let _ = state.record_network_failure(&policy);
    let action = state.record_success(&policy);
    assert!(matches!(
        action,
        PollNetworkLogAction::Recovery {
            level: log::Level::Info,
            ..
        }
    ));
    assert_eq!(state.record_success(&policy), PollNetworkLogAction::None);
}

#[test]
fn network_policy_recovery_reports_suppressed_total_once() {
    let mut state = PollNetworkErrorState::new();
    let policy = TelegramNetworkPollErrorLogging::default();

    let _ = state.record_network_failure(&policy);
    let _ = state.record_network_failure(&policy);
    let _ = state.record_network_failure(&policy);

    assert!(matches!(
        state.record_success(&policy),
        PollNetworkLogAction::Recovery {
            suppressed_total: 2,
            ..
        }
    ));
}

#[test]
fn network_policy_zero_sustained_threshold_escalates_first_failure() {
    let mut state = PollNetworkErrorState::new();
    let policy = TelegramNetworkPollErrorLogging {
        sustained_after_seconds: 0,
        ..TelegramNetworkPollErrorLogging::default()
    };

    assert_eq!(
        state.record_network_failure(&policy),
        PollNetworkLogAction::Failure {
            level: log::Level::Error,
            suppressed_since_last_emit: 0,
            sustained: true,
        }
    );
}

#[test]
fn network_policy_zero_sustained_threshold_recovers_as_sustained() {
    let mut state = PollNetworkErrorState::new();
    let policy = TelegramNetworkPollErrorLogging {
        sustained_after_seconds: 0,
        ..TelegramNetworkPollErrorLogging::default()
    };

    let _ = state.record_network_failure(&policy);

    assert!(matches!(
        state.record_success(&policy),
        PollNetworkLogAction::Recovery {
            sustained: true,
            ..
        }
    ));
}

#[test]
fn network_policy_zero_threshold_and_zero_repeat_logs_every_failure_at_sustained_level() {
    let mut state = PollNetworkErrorState::new();
    let policy = TelegramNetworkPollErrorLogging {
        sustained_after_seconds: 0,
        sustained_repeat_seconds: 0,
        ..TelegramNetworkPollErrorLogging::default()
    };

    assert!(matches!(
        state.record_network_failure(&policy),
        PollNetworkLogAction::Failure {
            level: log::Level::Error,
            sustained: true,
            ..
        }
    ));
    assert!(matches!(
        state.record_network_failure(&policy),
        PollNetworkLogAction::Failure {
            level: log::Level::Error,
            sustained: true,
            ..
        }
    ));
}

#[test]
fn network_policy_initial_failure_flow_recovers_on_first_success() {
    let mut state = PollNetworkErrorState::new();
    let policy = TelegramNetworkPollErrorLogging::default();

    let _initial = state.record_network_failure(&policy);

    assert!(matches!(
        state.record_success(&policy),
        PollNetworkLogAction::Recovery {
            suppressed_total: 0,
            sustained: false,
            ..
        }
    ));
}

#[test]
fn network_policy_reset_makes_next_network_failure_fresh() {
    let mut state = PollNetworkErrorState::new();
    let policy = TelegramNetworkPollErrorLogging::default();

    let _ = state.record_network_failure(&policy);
    state.reset_without_recovery();

    assert_eq!(
        state.record_network_failure(&policy),
        PollNetworkLogAction::Failure {
            level: log::Level::Warn,
            suppressed_since_last_emit: 0,
            sustained: false,
        }
    );
}

#[test]
fn network_policy_reset_prevents_stale_recovery() {
    let mut state = PollNetworkErrorState::new();
    let policy = TelegramNetworkPollErrorLogging::default();

    let _ = state.record_network_failure(&policy);
    state.reset_without_recovery();

    assert_eq!(state.record_success(&policy), PollNetworkLogAction::None);
}

// ── AgyFilter unit tests (#1549) ────────────────────────────
//
// Fixtures are the exact rows observed in diag-sent.log (session
// 19de56d2-…, frames 03:14-03:23 UTC 2026-08-25). DROPPED = chrome,
// KEPT = real content / documented residual.

#[test]
fn agy_filter_keep_line_drops_logo_rows() {
    let filter = AgyFilter;

    // Horizontal logo, frame 03:14:29.703 (all rows).
    let horizontal = [
        "▄▀▀▄        Antigravity CLI 1.1.19",
        "     ▀▀▀▀▀▀       mariano.blua@gmail.com",
        "    ▀▀▀▀▀▀▀▀      Gemini 3.7 Flash (High)",
        "   ▄▀▀    ▀▀▄     \
        D:/0_repos/AgentsCommander_iac/.ac/wg-21-ac-dev-team-v3/__agent_ac-cli-tester-v3",
    ];
    for line in horizontal {
        assert!(
            !filter.keep_line(line),
            "logo row should be dropped: {line:?}"
        );
    }

    // Vertical logo text rows, frame 03:15:27.898 (Blocker C: no block
    // glyph, 2-space indent — dropped by patterns, not by glyph rules).
    let vertical = [
        "  Antigravity CLI 1.1.19",
        "  mariano.blua@gmail.com (Google AI Pro)",
        "  Gemini 3.7 Flash (High)",
    ];
    for line in vertical {
        assert!(
            !filter.keep_line(line),
            "vertical logo row should be dropped: {line:?}"
        );
    }
}

#[test]
fn agy_filter_keep_line_pins_frame_031527_rows() {
    let filter = AgyFilter;

    // The 4 exact rows of the vertical-logo response frame (L19-33),
    // with explicit outcomes.
    // DROPPED: account row by `Google AI Pro` pattern
    assert!(!filter.keep_line("  mariano.blua@gmail.com (Google AI Pro)"));
    // DROPPED: model row by ` (High)` pattern
    assert!(!filter.keep_line("  Gemini 3.7 Flash (High)"));
    assert!(filter.keep_line(
        "  D:/0_repos/AgentsCommander_iac/.ac/wg-21-ac-dev-team-v3/__agent_ac-cli-tester-v3"
    )); // KEPT: path row — documented residual (plan §10.4)
    assert!(filter
        .keep_line("  ¡Hola! Soy ac-cli-tester-v3. Estoy listo para ayudarte con la validación"));
    // KEPT: real content
}

#[test]
fn agy_filter_keep_line_drops_hint_rows() {
    let filter = AgyFilter;

    assert!(!filter.keep_line(concat!(
        "? for shortcuts                                                             ",
        "                                                             Gemini 3.7 Flash · high",
    ))); // frame 03:14:29.703
    assert!(!filter.keep_line(concat!(
        "esc to cancel                                                              ",
        "                                                              Gemini 3.7 Flash · high",
    ))); // frame 03:14:47.504
}

#[test]
fn agy_filter_keep_line_drops_status_line() {
    let filter = AgyFilter;

    // Bare status lines (middot spelling).
    assert!(!filter.keep_line("Gemini 3.7 Flash · high"));
    assert!(!filter.keep_line("Gemini 3.7 Flash · medium"));
    assert!(!filter.keep_line("Gemini 3.7 Flash · low"));

    // Blocker A: counter variant, bare and fused (frame 03:23:42.495, L208
    // — exact content, 15 spaces).
    assert!(!filter.keep_line("Gemini 3.7 Flash · high · 1 task(s) · /tasks"));
    assert!(!filter
        .keep_line("esc to cancel               Gemini 3.7 Flash · high · 1 task(s) · /tasks"));
}

#[test]
fn agy_filter_keep_line_drops_thought_lines() {
    let filter = AgyFilter;

    // L96 / L~122 / L140 and the fused row L~201.
    assert!(!filter.keep_line("▸ Thought for 2s, 236 tokens"));
    assert!(!filter.keep_line("▸ Thought for 14s, 2.8k tokens"));
    assert!(!filter.keep_line("▸ Thought for 4s, 1.8k tokens"));
    assert!(!filter.keep_line("▸ ThougUse /feedback to share your experience with the team."));
}

#[test]
fn agy_filter_keep_line_drops_single_glyph() {
    let filter = AgyFilter;

    assert!(!filter.keep_line("●")); // frame 03:22:47.105
    assert!(!filter.keep_line("○")); // frame 03:23:37.897
}

#[test]
fn agy_filter_keep_line_drops_survey() {
    let filter = AgyFilter;

    // L240/L241 and the standalone feedback hint.
    assert!(!filter.keep_line("How's the CLI experience so far? Help us improve:"));
    assert!(!filter.keep_line("[1] Good  [2] Fine  [3] Bad  [0] Skip"));
    assert!(!filter.keep_line("Use /feedback to share your experience with the team."));
}

#[test]
fn agy_filter_keep_line_drops_input_echo() {
    let filter = AgyFilter;

    assert!(!filter.keep_line("> Hola")); // frame 03:14:45.492
    assert!(!filter.keep_line(">"));
}

#[test]
fn agy_filter_keep_line_keeps_content() {
    let filter = AgyFilter;

    // Real response content, frame 03:15:27 and 03:23:46.
    assert!(filter
        .keep_line("  ¡Hola! Soy ac-cli-tester-v3. Estoy listo para ayudarte con la validación"));
    assert!(filter.keep_line(
        "He ejecutado la validación en vivo de las 5 filas PENDING \
    asignadas"
    ));
    // A normal code/tool line.
    assert!(filter.keep_line("  let info = BridgeInfo { bot_id: bot.id.clone() };"));
    // Tool-call activity rows pass intentionally (informative, not chrome).
    assert!(filter.keep_line(
        "● Read(D:/0_repos/AgentsCommander_iac...t-compatibility-antigravity.md) (ctrl+o to expand)"
    ));
}

#[test]
fn agy_filter_keep_line_passes_step_labels() {
    let filter = AgyFilter;

    // Step labels are a documented residual (plan §10.4): task-dependent
    // vocabulary, no generalizable structural marker. KEPT deliberately.
    for line in [
        "  Analyzing Shell Arguments",
        "  Clarifying Authorization Boundaries",
        "  Investigating System Behavior",
        "  Observing System Activity",
    ] {
        assert!(
            filter.keep_line(line),
            "step label should be kept: {line:?}"
        );
    }
}

#[test]
fn agy_filter_keep_line_tradeoff_drops_midline_effort() {
    let filter = AgyFilter;

    // Contains-based tradeoff of Blockers A and C (plan §10.7): real
    // content carrying the effort marker anywhere is dropped, incl. the
    // substring false positives ` · higher` / ` · highly`.
    assert!(!filter.keep_line("El modelo rinde · high en esta prueba"));
    assert!(!filter.keep_line("El modo (High) es el más potente"));
    assert!(!filter.keep_line("Rendimiento · higher que antes"));
}

#[test]
fn filter_for_agent_maps_antigravity() {
    let filter = filter_for_agent(Some(CodingAgentKind::Antigravity));
    assert_eq!(filter.name(), "antigravity");
}

#[test]
fn filter_for_agent_maps_everything_else_to_claude() {
    // Regression pin: every non-agy session keeps ClaudeCodeFilter
    // bit-identically (Claude/Codex/Pi/None).
    assert_eq!(
        filter_for_agent(Some(CodingAgentKind::Claude)).name(),
        "claude-code"
    );
    assert_eq!(
        filter_for_agent(Some(CodingAgentKind::Codex)).name(),
        "claude-code"
    );
    assert_eq!(
        filter_for_agent(Some(CodingAgentKind::Pi)).name(),
        "claude-code"
    );
    assert_eq!(filter_for_agent(None).name(), "claude-code");
}

#[test]
fn agy_filter_shared_predicates_identical_to_claude_filter() {
    // Same decoration inputs behave identically through both filters.
    let agy = AgyFilter;
    let claude = ClaudeCodeFilter;

    for line in [
        "────────────────────", // box-drawing separator (U+2500)
        "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏",           // braille spinner row (U+2800..U+28FF)
        "▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪▪",     // low-alnum decorative line
    ] {
        assert!(!agy.keep_line(line), "agy should drop: {line:?}");
        assert!(!claude.keep_line(line), "claude should drop: {line:?}");
    }

    // Normal content passes both.
    for line in ["Hello world", "He ejecutado la validación"] {
        assert!(agy.keep_line(line), "agy should keep: {line:?}");
        assert!(claude.keep_line(line), "claude should keep: {line:?}");
    }
}
