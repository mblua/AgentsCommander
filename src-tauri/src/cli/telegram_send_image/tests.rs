use super::*;

fn bot(id: &str, label: &str) -> TelegramBotConfig {
    TelegramBotConfig {
        id: id.to_string(),
        label: label.to_string(),
        token: "test-token".to_string(),
        chat_id: 0,
        color: "#000000".to_string(),
    }
}

#[test]
fn resolve_bot_errors_when_no_bots_configured() {
    let err = resolve_bot(&[], None, None).unwrap_err();
    assert!(err.contains("No Telegram bots are configured"));
}

#[test]
fn resolve_bot_picks_single_bot_when_no_selector() {
    let bots = vec![bot("only-id", "OnlyBot")];
    let got = resolve_bot(&bots, None, None).unwrap();
    assert_eq!(got.id, "only-id");
}

#[test]
fn resolve_bot_requires_selector_when_multiple_bots() {
    let bots = vec![bot("a", "Alpha"), bot("b", "Beta")];
    let err = resolve_bot(&bots, None, None).unwrap_err();
    assert!(err.contains("--bot-id"));
    assert!(err.contains("--bot-label"));
    assert!(err.contains("Alpha"));
    assert!(err.contains("Beta"));
}

#[test]
fn resolve_bot_matches_by_id() {
    let bots = vec![bot("a", "Alpha"), bot("b", "Beta")];
    let got = resolve_bot(&bots, Some("b"), None).unwrap();
    assert_eq!(got.label, "Beta");
}

#[test]
fn resolve_bot_unknown_id_lists_available() {
    let bots = vec![bot("a", "Alpha"), bot("b", "Beta")];
    let err = resolve_bot(&bots, Some("zzz"), None).unwrap_err();
    assert!(err.contains("'zzz'"));
    assert!(err.contains("Alpha"));
    assert!(err.contains("Beta"));
}

#[test]
fn resolve_bot_matches_by_label() {
    let bots = vec![bot("a", "Alpha"), bot("b", "Beta")];
    let got = resolve_bot(&bots, None, Some("Beta")).unwrap();
    assert_eq!(got.id, "b");
}

#[test]
fn resolve_bot_label_ambiguous_lists_candidates() {
    let bots = vec![bot("a", "Same"), bot("b", "Same"), bot("c", "Other")];
    let err = resolve_bot(&bots, None, Some("Same")).unwrap_err();
    assert!(err.contains("Multiple"));
    assert!(err.contains("--bot-id"));
    // Both ambiguous candidates listed
    assert!(err.contains("id=a"));
    assert!(err.contains("id=b"));
}

#[test]
fn resolve_bot_unknown_label_lists_available() {
    let bots = vec![bot("a", "Alpha"), bot("b", "Beta")];
    let err = resolve_bot(&bots, None, Some("Gamma")).unwrap_err();
    assert!(err.contains("'Gamma'"));
    assert!(err.contains("Alpha"));
    assert!(err.contains("Beta"));
}

// Only covers the pre-settings empty-path early-exit (line 111 short-circuits
// before `load_settings_for_cli`). Do NOT extend with a non-empty path: a
// real path here would invoke `load_settings_for_cli()` and hit the
// developer's actual `settings.json` on disk. Path-validation belongs in
// tests of `perform_send_image` directly.
#[test]
fn execute_errors_on_empty_path() {
    let args = TelegramSendImageArgs {
        path: "   ".to_string(),
        caption: None,
        bot_id: None,
        bot_label: None,
    };
    let code = execute(args);
    assert_eq!(code, 1);
}

#[test]
fn help_text_documents_telegram_send_image() {
    use clap::CommandFactory;
    let help = crate::cli::Cli::command().render_help().to_string();
    assert!(
        help.contains("telegram-send-image"),
        "help missing verb: {}",
        help
    );
}
fn message_args(text: &str) -> TelegramSendMessageArgs {
    TelegramSendMessageArgs {
        text: text.into(),
        bot_id: None,
        bot_label: None,
    }
}

#[test]
fn message_rejects_before_loading_or_sending() {
    for text in [
        String::new(),
        " \n\t".into(),
        "a".repeat(4097),
        "😀".repeat(2049),
    ] {
        let result = send_message_with(
            message_args(&text),
            || panic!("loader called"),
            |_, _| panic!("sender called"),
        );
        assert!(result.unwrap_err().starts_with("--text must not"));
    }
}

#[test]
fn message_preserves_original_and_accepts_boundaries() {
    for text in [
        "  hello\nworld  ".into(),
        "a".repeat(4096),
        "😀".repeat(2048),
        "-value".into(),
    ] {
        let mut sent = false;
        let result = send_message_with(
            message_args(&text),
            || vec![bot("one", "Only")],
            |selected, original| {
                assert_eq!(selected.id, "one");
                assert_eq!(original, text);
                sent = true;
                Ok(())
            },
        );
        assert!(sent);
        assert_eq!(result.unwrap(), "Sent: bot=one");
    }
}

#[test]
fn message_failures_have_no_receipt_or_private_values() {
    for failure in [
        "could not initialize Telegram runtime",
        "could not initialize Telegram network",
        "Telegram send failed; delivery may be uncertain",
    ] {
        let result = send_message_with(
            message_args("BODY_SENTINEL"),
            || vec![bot("one", "Only")],
            |_, _| Err(failure),
        );
        let error = result.unwrap_err();
        assert_eq!(error, failure);
        for private in ["Sent:", "BODY_SENTINEL", "test-token"] {
            assert!(!error.contains(private));
        }
    }
}

#[test]
fn message_selector_errors_do_not_send_or_leak_token() {
    let mut args = message_args("BODY_SENTINEL");
    args.bot_id = Some("unknown".into());
    let error = send_message_with(
        args,
        || vec![bot("one", "Only")],
        |_, _| panic!("sender called"),
    )
    .unwrap_err();
    assert!(error.contains("id=one"));
    assert!(!error.contains("test-token"));
    assert!(!error.contains("BODY_SENTINEL"));
    let bots = vec![bot("same", "First"), bot("same", "Second")];
    assert_eq!(
        resolve_bot(&bots, Some("same"), None).unwrap().label,
        "First"
    );
}

#[test]
fn message_parses_selectors_and_leading_hyphen() {
    use clap::Parser;
    for selector in ["--bot-id", "--bot-label"] {
        let parsed = crate::cli::Cli::try_parse_from([
            "ac",
            "telegram-send-message",
            "--text=-value",
            selector,
            "one",
        ])
        .unwrap();
        let Some(crate::cli::Commands::TelegramSendMessage(args)) = parsed.command else {
            panic!("wrong command")
        };
        assert_eq!(args.text, "-value");
        assert_eq!(
            args.bot_id.as_deref().or(args.bot_label.as_deref()),
            Some("one")
        );
    }
    assert!(crate::cli::Cli::try_parse_from([
        "ac",
        "telegram-send-message",
        "--text",
        "hello",
        "--bot-id",
        "one",
        "--bot-label",
        "Only"
    ])
    .is_err());
}
