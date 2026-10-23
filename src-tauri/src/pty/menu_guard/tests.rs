use super::*;
use crate::config::settings::default_blocking_menus_for_command;
use crate::config::settings::BlockingMenusStore;
// #1757 - T11 builds a real ScreenFrame. Test-module only: widening the
// production use statement at the top of this file would remove a production line.
use crate::pty::watchers::ScreenFrame;

#[test]
fn test_menu_guard_pattern_matching() {
    let guard = MenuGuard::new();
    let session_id = Uuid::new_v4();

    let pi_entries = default_blocking_menus_for_command("pi");
    let codex_entries = default_blocking_menus_for_command("codex");

    let pi_row = vec![LogicalRow {
        start: 0,
        end: 0,
        text: "  Trust project folder? (y/n)".to_string(),
    }];
    let eval_pi = guard.evaluate_logical_rows(session_id, &pi_row, &pi_entries);
    assert!(eval_pi.is_blocked);
    assert!(eval_pi.should_notify);
    assert!(guard.is_blocked(session_id));
    assert_eq!(
        eval_pi.matched_notification.as_deref(),
        Some("pi is waiting for you to answer the folder-trust menu in this terminal")
    );

    let codex_session_id = Uuid::new_v4();
    let codex_row = vec![LogicalRow {
        start: 0,
        end: 0,
        text: "Do you trust the contents of this directory? [Y/n]".to_string(),
    }];
    let eval_codex = guard.evaluate_logical_rows(codex_session_id, &codex_row, &codex_entries);
    assert!(eval_codex.is_blocked);
    assert!(eval_codex.should_notify);
    assert!(guard.is_blocked(codex_session_id));
    assert_eq!(
        eval_codex.matched_notification.as_deref(),
        Some("codex is waiting for you to answer the folder-trust menu in this terminal")
    );
}

#[test]
fn test_menu_guard_episode_suppression_and_rearm() {
    let guard = MenuGuard::new();
    let session_id = Uuid::new_v4();
    let entries = default_blocking_menus_for_command("pi");

    let menu_rows = vec![LogicalRow {
        start: 0,
        end: 0,
        text: "Trust project folder?".to_string(),
    }];
    let clean_rows = vec![LogicalRow {
        start: 0,
        end: 0,
        text: "Normal output".to_string(),
    }];

    // 1. First appearance -> blocked
    let eval1 = guard.evaluate_logical_rows(session_id, &menu_rows, &entries);
    assert!(eval1.is_blocked);
    assert!(eval1.should_notify);
    assert!(guard.is_blocked(session_id));
    let episode_1 = eval1.episode_id;

    // 2. User resolves the episode
    let resolved = guard.resolve_current_episode(session_id);
    assert!(resolved);
    assert!(!guard.is_blocked(session_id));

    // 3. Same menu still visible on next tick -> suppressed, not blocked
    let eval2 = guard.evaluate_logical_rows(session_id, &menu_rows, &entries);
    assert!(!eval2.is_blocked);
    assert!(!eval2.should_notify);
    assert!(!guard.is_blocked(session_id));
    assert_eq!(eval2.episode_id, episode_1);

    // 4. Menu disappears -> clears state, returns should_clear_notification
    let eval3 = guard.evaluate_logical_rows(session_id, &clean_rows, &entries);
    assert!(!eval3.is_blocked);
    assert!(eval3.should_clear_notification);
    assert!(!guard.is_blocked(session_id));

    // 5. Menu reappears -> new episode, re-armed and blocked!
    let eval4 = guard.evaluate_logical_rows(session_id, &menu_rows, &entries);
    assert!(eval4.is_blocked);
    assert!(eval4.should_notify);
    assert!(guard.is_blocked(session_id));
    assert_ne!(eval4.episode_id, episode_1);
}

#[test]
fn test_menu_guard_invalid_regex_tolerance() {
    let guard = MenuGuard::new();
    let session_id = Uuid::new_v4();

    let entries = vec![
        BlockingMenuEntry::Valid(BlockingMenuConfig {
            pattern: "[unclosed regex".to_string(),
            notification: "bad regex".to_string(),
            enabled: true,
            captured_against: None,
        }),
        BlockingMenuEntry::Valid(BlockingMenuConfig {
            pattern: "valid match".to_string(),
            notification: "good regex".to_string(),
            enabled: true,
            captured_against: None,
        }),
    ];

    let rows = vec![LogicalRow {
        start: 0,
        end: 0,
        text: "some valid match text".to_string(),
    }];

    // Should skip invalid regex without panic and match the valid one
    let eval = guard.evaluate_logical_rows(session_id, &rows, &entries);
    assert!(eval.is_blocked);
    assert_eq!(eval.matched_notification.as_deref(), Some("good regex"));
}

/// #1757 G2 - the Codex "Hooks need review" dialog as a REPLAYED terminal
/// mirror, not as hand-typed row text. Every content row is inset by two
/// columns, carries SGR attributes, and ends CRLF; U+203A is the selection
/// marker Codex actually renders.
const CODEX_HOOKS_REVIEW_FRAME: &str = concat!(
    "  \x1b[1mHooks need review\x1b[0m\r\n",
    "  \x1b[33m1 hook is new or changed.\x1b[0m\r\n",
    "  \x1b[2mHooks can run outside the sandbox after you trust them.\x1b[0m\r\n",
    "\r\n",
    "  \u{203a} 1. Review hooks\r\n",
    "    2. Trust all and continue\r\n",
    "    3. Continue without trusting (hooks won't run)\r\n",
    "\r\n",
    "  \x1b[2mPress enter to confirm or esc to go back\x1b[0m\r\n",
);

/// #1757 T11. Drives the product's own frame-building and row-joining code
/// over a replayed vt100 mirror, so the two-column inset, the trailing-blank
/// trim and the wrap join are what the shipped pattern is measured against.
#[test]
fn codex_hooks_review_matches_the_mirror_of_a_styled_inset_dialog() {
    let guard = MenuGuard::new();
    let entries = default_blocking_menus_for_command("codex");

    let mut parser = vt100::Parser::new(30, 120, 0);
    parser.process(CODEX_HOOKS_REVIEW_FRAME.as_bytes());

    // Built exactly as PtyManager::get_screen_rows_since builds it.
    let screen = parser.screen();
    let rows: Vec<String> = screen.rows(0, 120).collect();
    let wrapped: Vec<bool> = (0..rows.len() as u16)
        .map(|row| screen.row_wrapped(row))
        .collect();
    let frame = ScreenFrame {
        rows,
        wrapped,
        cursor_row: screen.cursor_position().0,
        stamp: None,
    };
    let rows = logical_rows(&frame);

    let eval = guard.evaluate_logical_rows(Uuid::new_v4(), &rows, &entries);
    assert!(
        eval.is_blocked,
        "the shipped codex entries did not mark the replayed hooks-review dialog blocked"
    );
    assert_eq!(
        eval.matched_notification.as_deref(),
        Some("codex is waiting for you to answer the hooks-review menu in this terminal")
    );

    // 7.3 - the deliberately-broken control, executed and not merely described.
    let broken = vec![BlockingMenuEntry::Valid(BlockingMenuConfig {
        pattern: r"^Hooks need review$".to_string(),
        notification: "control".to_string(),
        enabled: true,
        captured_against: None,
    })];
    let control = guard.evaluate_logical_rows(Uuid::new_v4(), &rows, &broken);
    assert!(
        !control.is_blocked,
        "control pattern matched: the replayed rows are neither inset nor decorated, \
         so the shipped \
         pattern's tolerance for a gutter is untested and the main assertion proves nothing"
    );

    // 7.4 - negative corpus, on a fresh session id per row so no earlier
    // episode masks the result.
    for text in [
        "1 hook is new or changed.",
        "Hooks can run outside the sandbox after you trust them.",
        "Press enter to confirm or esc to go back",
        "The docs explain why Hooks need review is shown.",
        "Hooks need reviewers",
        "hooks need review",
    ] {
        let row = vec![LogicalRow {
            start: 0,
            end: 0,
            text: text.to_string(),
        }];
        let negative = guard.evaluate_logical_rows(Uuid::new_v4(), &row, &entries);
        assert!(
            !negative.is_blocked,
            "negative corpus row must not match: {text:?}"
        );
    }
}

// #1905 phase 2 fixtures. `agent` builds an `AgentConfig` with the eleven fields exactly
// as the loader tests in `settings.rs` do; `CUSTOM` is the docs page's own example entry
// (`docs/features/menu-guard.md`), test-only and independent of the shipped file.
fn agent(id: &str, command: &str, blocking_menus: Option<Vec<BlockingMenuEntry>>) -> AgentConfig {
    AgentConfig {
        preflight: None,
        preflight_timeout_seconds: None,
        id: id.to_string(),
        id_hash: None,
        id_hash_debug: None,
        label: id.to_string(),
        command: command.to_string(),
        color: "#000000".to_string(),
        order: None,
        envs: Vec::new(),
        isolated_home: false,
        instructions_filename: None,
        config_seed: None,
        context_regex: None,
        blocking_menus,
        backend: Default::default(),
    }
}

const CUSTOM_NOTIFICATION: &str =
    "claude is waiting for you to answer the folder-trust menu in this terminal";

fn custom(enabled: bool) -> BlockingMenuEntry {
    BlockingMenuEntry::Valid(BlockingMenuConfig {
        pattern: r"^\s*Do you trust the files in this folder\?".to_string(),
        notification: CUSTOM_NOTIFICATION.to_string(),
        enabled,
        captured_against: Some("claude 2.1 / Windows".to_string()),
    })
}

fn claude_row() -> Vec<LogicalRow> {
    vec![LogicalRow {
        start: 0,
        end: 0,
        text: "  Do you trust the files in this folder? (y/n)".to_string(),
    }]
}

fn pi_row() -> Vec<LogicalRow> {
    vec![LogicalRow {
        start: 0,
        end: 0,
        text: "Trust project folder?".to_string(),
    }]
}

/// The production loader against a tempdir whose `.local` file carries `byAgent` rows.
fn production_store(dir: &std::path::Path) -> BlockingMenusStore {
    let local = serde_json::json!({
        "schemaVersion": 1,
        "byAgent": {
            "claude-1": [custom(true)],
            "claude-off": [custom(false)],
            "pi-off": [],
        }
    });
    std::fs::write(
        dir.join("blocking-menus.50.personal.no-git.json"),
        serde_json::to_vec_pretty(&local).unwrap(),
    )
    .unwrap();
    let store = BlockingMenusStore::load_from_settings_path(&dir.join("settings.json"));
    assert!(
        dir.join("blocking-menus.10.default.no-git.json").exists(),
        "the production loader writes the shipped file"
    );
    store
}

#[test]
fn a_custom_pattern_on_disk_reaches_the_evaluator_through_the_production_store() {
    let temp = tempfile::TempDir::new().unwrap();
    let guard = MenuGuard::with_store(production_store(temp.path()));
    let control = MenuGuard::new();

    // 1. byAgent["claude-1"] = [CUSTOM]: the on-disk pattern blocks the row.
    let claude_1 = agent("claude-1", "claude", None);
    let entries = guard.entries_for(&claude_1);
    assert_eq!(entries.len(), 1);
    let eval = guard.evaluate_logical_rows(Uuid::new_v4(), &claude_row(), &entries);
    assert!(eval.is_blocked);
    assert_eq!(
        eval.matched_notification.as_deref(),
        Some(CUSTOM_NOTIFICATION)
    );
    // Control: the shipped-only guard has nothing for claude, so a store that always
    // returned `[]` would fail the assertions above, not pass them by accident.
    let control_entries = control.entries_for(&claude_1);
    assert!(control_entries.is_empty());
    let control_eval =
        control.evaluate_logical_rows(Uuid::new_v4(), &claude_row(), &control_entries);
    assert!(!control_eval.is_blocked);

    // 2. byAgent["claude-off"] = [CUSTOM disabled]: present, but does not block.
    let entries = guard.entries_for(&agent("claude-off", "claude", None));
    assert_eq!(entries.len(), 1);
    let eval = guard.evaluate_logical_rows(Uuid::new_v4(), &claude_row(), &entries);
    assert!(!eval.is_blocked);

    // 3. byAgent["pi-off"] = []: an explicit empty array switches the shipped set off.
    let pi_off = agent("pi-off", "pi", None);
    let entries = guard.entries_for(&pi_off);
    assert!(entries.is_empty());
    let eval = guard.evaluate_logical_rows(Uuid::new_v4(), &pi_row(), &entries);
    assert!(!eval.is_blocked);
    let control_entries = control.entries_for(&pi_off);
    let control_eval = control.evaluate_logical_rows(Uuid::new_v4(), &pi_row(), &control_entries);
    assert!(control_eval.is_blocked);
}

#[test]
fn a_remote_pattern_in_the_cache_reaches_the_evaluator_1925() {
    let temp = tempfile::TempDir::new().unwrap();
    let remote = serde_json::json!({
        "schemaVersion": 1,
        "byCommand": {
            "grok": [{
                "pattern": "^\\s*Grok test menu\\?",
                "notification": "grok is waiting for you to answer the test menu in this terminal",
                "enabled": true,
            }],
        },
        "byAgent": {},
    });
    std::fs::write(
        temp.path().join("blocking-menus.20.remote.no-git.json"),
        serde_json::to_vec_pretty(&remote).unwrap(),
    )
    .unwrap();

    let guard = MenuGuard::with_store(BlockingMenusStore::load_from_settings_path(
        &temp.path().join("settings.json"),
    ));
    let grok = agent("grok-1", "grok", None);
    let entries = guard.entries_for(&grok);
    assert_eq!(entries.len(), 1);
    let row = vec![LogicalRow {
        start: 0,
        end: 0,
        text: "Grok test menu?".to_string(),
    }];
    let eval = guard.evaluate_logical_rows(Uuid::new_v4(), &row, &entries);
    assert!(eval.is_blocked);
    assert_eq!(
        eval.matched_notification.as_deref(),
        Some("grok is waiting for you to answer the test menu in this terminal")
    );

    // Control: the shipped-only guard has nothing for grok, so a store that always
    // returned the cache would fail the assertions above, not pass them by accident.
    let control = MenuGuard::new();
    let control_entries = control.entries_for(&grok);
    assert!(control_entries.is_empty());
    let control_eval = control.evaluate_logical_rows(Uuid::new_v4(), &row, &control_entries);
    assert!(!control_eval.is_blocked);
}

// #2251 - the three captures transcribed as the rows the scanner evaluates.
// claude (room-shared/2250-claude-menu.png): the footer line of the numbered selection
// menu, its words on one row. opencode permission
// (room-shared/2250-opencode-permission-menu.png): the title line of the
// external-directory permission dialog. opencode Always allow
// (room-shared/2251-opencode-always-allow-menu.png, SHA-256 D5FD25FB...): the
// confirmation sentence of the nested Always allow dialog. The captures' leading cells
// are not reproduced; the patterns' `[^A-Za-z0-9]*` prefix absorbs them.
const CAPTURED_2251_CLAUDE_FOOTER: &str =
    "Enter to select \u{00b7} \u{2191}/\u{2193} to navigate \u{00b7} Esc to cancel";
const CAPTURED_2251_OPENCODE_PERMISSION: &str = "Permission required";
const CAPTURED_2251_OPENCODE_ALWAYS: &str =
    "This will allow the following patterns until OpenCode is restarted";

// #2251 - the exact published patterns under proof; must equal the file's bytes.
const CLAUDE_2251_PATTERN: &str = r"^[^A-Za-z0-9]*Enter to select\b.{1,64}Esc to cancel\b";
const OPENCODE_PERMISSION_2251_PATTERN: &str = r"^[^A-Za-z0-9]*Permission required\b";
const OPENCODE_ALWAYS_2251_PATTERN: &str =
    r"^[^A-Za-z0-9]*This will allow the following patterns until OpenCode is restarted\b";

// #2251 - per pattern, the captured rows it must NOT match. They include the other
// states' rows, the one-end truncations of its own captured row, and rows that other
// entries legitimately own (for example `Esc to cancel · Tab to amend`, the existing
// #2248 claude footer), so they are negative controls for THIS pattern, not "non-menu"
// rows in general. The bullet row is the reviewer's false-positive test for a title
// anchor.
const CAPTURED_2251_CLAUDE_NEGATIVES: &[&str] = &[
    "Enter to select",
    "Esc to cancel \u{00b7} Tab to amend",
    "6. Chat about this",
    "Access external directory",
    "Always allow",
    "- Always allow the user to pick",
    "This will allow the following patterns",
];
const CAPTURED_2251_OPENCODE_PERMISSION_NEGATIVES: &[&str] = &[
    "Permission",
    "Access external directory",
    "Always allow",
    "- Always allow the user to pick",
    "This will allow the following patterns",
    "Allow always",
];
const CAPTURED_2251_OPENCODE_ALWAYS_NEGATIVES: &[&str] = &[
    "Always allow",
    "- Always allow the user to pick",
    "This will allow the following patterns",
    "Confirm",
    "Cancel",
    "Allow once",
    "Allow always",
    "Reject",
    "Access external directory",
    "Build",
    "GLM-5.2",
];

#[test]
fn the_published_2251_entries_block_the_captured_rows_and_ignore_their_neighbours() {
    let published =
        include_str!("../../../../remote-resources/blocking-menus/v1/settings-blocking-menus.json");
    let file = crate::config::settings::validate_remote_blocking_menus_file(published)
        .expect("the published remote file passes the validator");

    let cases: [(&str, &str, &str, &str, &[&str]); 3] = [
        (
            "claude",
            CLAUDE_2251_PATTERN,
            CAPTURED_2251_CLAUDE_FOOTER,
            "claude is waiting for you to answer a selection menu in this terminal",
            CAPTURED_2251_CLAUDE_NEGATIVES,
        ),
        (
            "opencode",
            OPENCODE_PERMISSION_2251_PATTERN,
            CAPTURED_2251_OPENCODE_PERMISSION,
            "opencode is waiting for you to answer a permission prompt in this terminal",
            CAPTURED_2251_OPENCODE_PERMISSION_NEGATIVES,
        ),
        (
            "opencode",
            OPENCODE_ALWAYS_2251_PATTERN,
            CAPTURED_2251_OPENCODE_ALWAYS,
            "opencode is waiting for you to confirm the Always allow menu in this terminal",
            CAPTURED_2251_OPENCODE_ALWAYS_NEGATIVES,
        ),
    ];

    for (stem, pattern, captured, notification, negatives) in cases {
        let config = file.by_command[stem]
            .iter()
            .filter_map(|entry| entry.valid())
            .find(|config| config.pattern == pattern)
            .unwrap_or_else(|| panic!("the published {stem} array carries the #2251 pattern"));
        assert_eq!(config.notification, notification);
        let regex = regex::Regex::new(&config.pattern).expect("the pattern compiles");
        assert!(
            regex.is_match(captured),
            "{stem}: the captured row must match"
        );
        for row in negatives {
            assert!(
                !regex.is_match(row),
                "{stem}: a negative row must not match: {row}"
            );
        }
    }

    // End to end through the scanner: the published bytes installed as the remote cache
    // block each captured row for its stem and resolve the published notification. The
    // two opencode cases share the stem and differ only in the row under test.
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("blocking-menus.20.remote.no-git.json"),
        published,
    )
    .unwrap();
    let guard = MenuGuard::with_store(BlockingMenusStore::load_from_settings_path(
        &temp.path().join("settings.json"),
    ));
    for (stem, _, captured, notification, negatives) in cases {
        let entries = guard.entries_for(&agent(&format!("{stem}-2251"), stem, None));
        let blocked = guard.evaluate_logical_rows(
            Uuid::new_v4(),
            &[LogicalRow {
                start: 0,
                end: 0,
                text: captured.to_string(),
            }],
            &entries,
        );
        assert!(blocked.is_blocked, "{stem}: the captured row blocks");
        assert_eq!(blocked.matched_notification.as_deref(), Some(notification));
        let clean = guard.evaluate_logical_rows(
            Uuid::new_v4(),
            &[LogicalRow {
                start: 0,
                end: 0,
                text: negatives[0].to_string(),
            }],
            &entries,
        );
        assert!(!clean.is_blocked, "{stem}: a negative row stays clean");
    }
}

// #2292 - the published claude mid-response disconnect entry. The captured row
// (room-shared/2292-claude-midresponse-api-error.png) is a single terminal row; the
// pattern's `[^A-Za-z0-9]*` prefix absorbs any leading gutter the capture trimmed.
const CLAUDE_2292_PATTERN: &str = r"^[^A-Za-z0-9]*API Error: Connection lost mid-response\b";
const CAPTURED_2292_NOTICE: &str =
    "API Error: Connection lost mid-response. The response above may be incomplete.";
const CLAUDE_2292_NOTIFICATION: &str =
    "claude lost the API connection mid-response; its response may be incomplete; \
     check this terminal";

// #2292 - the six published claude entries that must survive the addition byte-identical.
const CLAUDE_PATTERNS_BEFORE_2292: [&str; 6] = [
    r"API Error: Can['’]t reach the API server",
    r"^\s*Claude in Chrome extension detected",
    r"^[^A-Za-z0-9]*API Error: \d{3}\b",
    r"^[^A-Za-z0-9]*Dangerous rm operation\b",
    r"^\s*Esc to cancel\b.{1,5}Tab to amend\b",
    r"^[^A-Za-z0-9]*Enter to select\b.{1,64}Esc to cancel\b",
];

// #2292 - rows the new entry must NOT trigger: the two existing API-error rows, one-end
// truncations of its own row, the message without its prefix, and an alphanumeric-prefixed
// quote.
const CAPTURED_2292_NEGATIVES: &[&str] = &[
    "API Error: Can['’]t reach the API server",
    "API Error: 500 Internal Server Error",
    "API Error: Connection lost",
    "Connection lost mid-response",
    "The response above may be incomplete.",
    "Note: API Error: Connection lost mid-response.",
];

#[test]
fn the_published_2292_entry_blocks_the_captured_notice_and_ignores_its_neighbours() {
    let published =
        include_str!("../../../../remote-resources/blocking-menus/v1/settings-blocking-menus.json");
    let file = crate::config::settings::validate_remote_blocking_menus_file(published)
        .expect("the published remote file passes the validator");

    let claude = &file.by_command["claude"];
    for pattern in CLAUDE_PATTERNS_BEFORE_2292 {
        assert!(
            claude
                .iter()
                .any(|entry| entry.valid().is_some_and(|c| c.pattern == pattern)),
            "the new entry must not replace an existing claude pattern: {pattern}"
        );
    }

    let config = claude
        .iter()
        .filter_map(|entry| entry.valid())
        .find(|config| config.pattern == CLAUDE_2292_PATTERN)
        .expect("the published claude array carries the #2292 pattern");
    assert_eq!(config.notification, CLAUDE_2292_NOTIFICATION);

    let regex = regex::Regex::new(&config.pattern).expect("the pattern compiles");
    assert!(
        regex.is_match(CAPTURED_2292_NOTICE),
        "the captured row must match"
    );
    for gutter in [
        format!("\u{25CF} {}", CAPTURED_2292_NOTICE),
        format!("\u{2502} {}", CAPTURED_2292_NOTICE),
    ] {
        assert!(
            regex.is_match(&gutter),
            "a guttered row must match: {gutter:?}"
        );
    }
    for row in CAPTURED_2292_NEGATIVES {
        assert!(
            !regex.is_match(row),
            "a negative row must not match: {row:?}"
        );
    }

    // End to end through the scanner: published bytes installed as the remote cache block
    // the captured row for the claude stem, while the shipped-only guard has no claude
    // entries at all (control).
    let temp = tempfile::TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("blocking-menus.20.remote.no-git.json"),
        published,
    )
    .unwrap();
    let guard = MenuGuard::with_store(BlockingMenusStore::load_from_settings_path(
        &temp.path().join("settings.json"),
    ));
    let entries = guard.entries_for(&agent("claude-2292", "claude", None));
    let blocked = guard.evaluate_logical_rows(
        Uuid::new_v4(),
        &[LogicalRow {
            start: 0,
            end: 0,
            text: CAPTURED_2292_NOTICE.to_string(),
        }],
        &entries,
    );
    assert!(blocked.is_blocked, "the captured row blocks");
    assert_eq!(
        blocked.matched_notification.as_deref(),
        Some(CLAUDE_2292_NOTIFICATION)
    );

    let clean = guard.evaluate_logical_rows(
        Uuid::new_v4(),
        &[LogicalRow {
            start: 0,
            end: 0,
            text: CAPTURED_2292_NEGATIVES[0].to_string(),
        }],
        &entries,
    );
    assert!(!clean.is_blocked, "a negative row stays clean");

    let control = MenuGuard::new();
    let control_entries = control.entries_for(&agent("claude-2292-control", "claude", None));
    assert!(
        control_entries.is_empty(),
        "the shipped baseline has no claude entries"
    );
    let control_eval = control.evaluate_logical_rows(
        Uuid::new_v4(),
        &[LogicalRow {
            start: 0,
            end: 0,
            text: CAPTURED_2292_NOTICE.to_string(),
        }],
        &control_entries,
    );
    assert!(!control_eval.is_blocked);
}

#[test]
fn a_legacy_array_on_the_agent_wins_over_both_files() {
    let temp = tempfile::TempDir::new().unwrap();
    let guard = MenuGuard::with_store(production_store(temp.path()));

    // Some(vec![]) on the agent beats byAgent["claude-1"] = [CUSTOM].
    let entries = guard.entries_for(&agent("claude-1", "claude", Some(vec![])));
    assert!(entries.is_empty());
    let eval = guard.evaluate_logical_rows(Uuid::new_v4(), &claude_row(), &entries);
    assert!(!eval.is_blocked);

    // Some([CUSTOM]) on the agent beats byAgent["pi-off"] = [].
    let entries = guard.entries_for(&agent("pi-off", "pi", Some(vec![custom(true)])));
    assert_eq!(entries.len(), 1);
    let eval = guard.evaluate_logical_rows(Uuid::new_v4(), &claude_row(), &entries);
    assert!(eval.is_blocked);

    // The materialized state every agent is in until phase 3.
    let entries = guard.entries_for(&agent(
        "pi-2",
        "pi",
        Some(default_blocking_menus_for_command("pi")),
    ));
    let eval = guard.evaluate_logical_rows(Uuid::new_v4(), &pi_row(), &entries);
    assert!(eval.is_blocked);
}

mod response_close_menu;
