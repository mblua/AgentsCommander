//! `telegram-send-image` CLI verb — sends a local image (or generic file) to
//! a configured Telegram bot from an external terminal, bypassing the Tauri
//! frontend. Issue #282.
//!
//! Reuses `commands::telegram::perform_send_image` so size/extension/symlink
//! validation and multipart-upload logic live in exactly one place.
//!
//! No `--token` requirement: the verb reads `settings.json` and posts to the
//! Telegram Bot API directly. Outbound HTTP from a process the user already
//! controls does not cross AC's per-session authorization boundary.

use clap::Args;

use crate::config::settings::load_settings_for_cli;
use crate::telegram::types::TelegramBotConfig;

#[derive(Args)]
#[command(after_help = "\
PURPOSE: Send a local image or file to a configured Telegram bot from the \
terminal. Files <=10 MB with extension jpg/jpeg/png/webp use sendPhoto; \
everything else (including GIF) falls back to sendDocument, capped at \
50 MB.\n\n\
BOT SELECTION:\n  \
  --bot-id <id>         Pick a bot by its persisted id (from settings.json).\n  \
  --bot-label <label>   Pick a bot by its human label (exact match).\n  \
  If exactly one bot is configured, both flags may be omitted.\n  \
  If multiple bots are configured and neither flag is provided, the verb \
errors with the list of available bots.\n\n\
CAPTION: Trimmed and clamped to Telegram's 1024 UTF-16-code-unit limit. \
Empty/whitespace-only captions are dropped.\n\n\
SYMLINKS: Symlinks and (on Windows) reparse points / junctions are rejected.")]
pub struct TelegramSendImageArgs {
    /// Absolute or relative path to the file to send (required)
    #[arg(long)]
    pub path: String,

    /// Optional caption to attach to the image / document
    #[arg(long)]
    pub caption: Option<String>,

    /// Bot id (from settings.json telegramBots[*].id). Mutually exclusive with --bot-label
    #[arg(long, conflicts_with = "bot_label")]
    pub bot_id: Option<String>,

    /// Bot label (exact match against telegramBots[*].label). Mutually exclusive with --bot-id
    #[arg(long)]
    pub bot_label: Option<String>,
}

/// Pick a bot from `bots` using the CLI selector flags.
///
/// Selection rules:
///   - `--bot-id` → match by `id`; error if none found.
///   - `--bot-label` → match by `label`; error if none found or multiple match.
///   - Neither → if exactly one bot is configured, use it; otherwise error
///     listing every available bot so the user knows which selector to pass.
fn resolve_bot<'a>(
    bots: &'a [TelegramBotConfig],
    bot_id: Option<&str>,
    bot_label: Option<&str>,
) -> Result<&'a TelegramBotConfig, String> {
    if bots.is_empty() {
        return Err("No Telegram bots are configured. Add one in the AgentsCommander GUI under Settings/Telegram first.".to_string());
    }

    if let Some(id) = bot_id {
        return bots.iter().find(|b| b.id == id).ok_or_else(|| {
            format!(
                "No Telegram bot with id '{}'. Available bots: {}",
                id,
                format_bot_list(bots)
            )
        });
    }

    if let Some(label) = bot_label {
        let matches: Vec<&TelegramBotConfig> = bots.iter().filter(|b| b.label == label).collect();
        return match matches.len() {
            0 => Err(format!(
                "No Telegram bot with label '{}'. Available bots: {}",
                label,
                format_bot_list(bots)
            )),
            1 => Ok(matches[0]),
            _ => Err(format!(
                "Multiple Telegram bots share label '{}'; disambiguate with --bot-id. Candidates: {}",
                label,
                format_bot_list(&matches.into_iter().cloned().collect::<Vec<_>>())
            )),
        };
    }

    if bots.len() == 1 {
        Ok(&bots[0])
    } else {
        Err(format!(
            "Multiple Telegram bots are configured; pass --bot-id <id> or --bot-label <label>. Available bots: {}",
            format_bot_list(bots)
        ))
    }
}

fn format_bot_list(bots: &[TelegramBotConfig]) -> String {
    bots.iter()
        .map(|b| format!("{} (id={})", b.label, b.id))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn execute(args: TelegramSendImageArgs) -> i32 {
    if args.path.trim().is_empty() {
        eprintln!("Error: --path must not be empty");
        return 1;
    }

    let settings = load_settings_for_cli();
    let bot = match resolve_bot(
        &settings.telegram_bots,
        args.bot_id.as_deref(),
        args.bot_label.as_deref(),
    ) {
        Ok(b) => b.clone(),
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Error: failed to build tokio runtime: {}", e);
            return 1;
        }
    };

    let result = rt.block_on(crate::commands::telegram::perform_send_image(
        &bot,
        &args.path,
        args.caption.as_deref(),
    ));

    match result {
        Ok(()) => {
            crate::cli_println!("Sent: bot={} path={}", bot.id, args.path);
            log::info!(
                "[cli] telegram-send-image: bot={} path={} caption_present={}",
                bot.id,
                args.path,
                args.caption.is_some()
            );
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

#[cfg(test)]
mod tests {
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
}
