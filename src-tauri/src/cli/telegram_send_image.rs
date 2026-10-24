//! `telegram-send-message` sends original plain text without an attachment.
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

/// Send plain text through a bot configured in this executable's instance.
#[derive(Args)]
#[command(
    after_help = "Text is sent unchanged. Empty text and more than 4096 UTF-16 code units \
        are rejected locally. \
        Select a bot by exact id or unique label; omit selectors only with one configured bot. \
        No room, session, bridge or GUI is required. \
        Use --text=-value for leading hyphens."
)]
pub struct TelegramSendMessageArgs {
    /// Original plain text to send (required)
    #[arg(long)]
    pub text: String,
    /// Configured bot id; mutually exclusive with --bot-label
    #[arg(long, conflicts_with = "bot_label")]
    pub bot_id: Option<String>,
    /// Configured bot label (exact, unique match)
    #[arg(long)]
    pub bot_label: Option<String>,
}

// The loader and sender are one-shot closures so tests never need real settings or network.
fn send_message_with(
    args: TelegramSendMessageArgs,
    load_bots: impl FnOnce() -> Vec<TelegramBotConfig>,
    send: impl FnOnce(&TelegramBotConfig, &str) -> Result<(), &'static str>,
) -> Result<String, String> {
    if args.text.trim().is_empty() {
        return Err("--text must not be empty".into());
    }
    if args.text.encode_utf16().count() > 4096 {
        return Err("--text must not exceed 4096 UTF-16 code units".into());
    }
    let bots = load_bots();
    let bot = resolve_bot(&bots, args.bot_id.as_deref(), args.bot_label.as_deref())?;
    send(bot, &args.text).map_err(str::to_owned)?;
    Ok(format!("Sent: bot={}", bot.id))
}

fn send_message(bot: &TelegramBotConfig, text: &str) -> Result<(), &'static str> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "could not initialize Telegram runtime")?;
    let network = crate::network::OutboundNetwork::new()
        .map_err(|_| "could not initialize Telegram network")?;
    rt.block_on(crate::commands::telegram::perform_send_message(
        &network, bot, text,
    ))
}

pub fn execute_message(args: TelegramSendMessageArgs) -> i32 {
    let result = send_message_with(args, || load_settings_for_cli().telegram_bots, send_message);
    match result {
        Ok(receipt) => {
            crate::cli_println!("{}", receipt);
            0
        }
        Err(error) => {
            eprintln!("Error: {}", error);
            1
        }
    }
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
        return Err("No Telegram bots are configured. \
            Add one in the AgentsCommander GUI under Settings/Telegram first."
            .to_string());
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
                "Multiple Telegram bots share label '{}'; disambiguate with \
                 --bot-id. Candidates: {}",
                label,
                format_bot_list(matches.iter().copied())
            )),
        };
    }

    if bots.len() == 1 {
        Ok(&bots[0])
    } else {
        Err(format!(
            "Multiple Telegram bots are configured; pass --bot-id <id> or --bot-label <label>. \
             Available bots: {}",
            format_bot_list(bots)
        ))
    }
}

fn format_bot_list<'a>(bots: impl IntoIterator<Item = &'a TelegramBotConfig>) -> String {
    bots.into_iter()
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

    // INVARIANT: `perform_send_image` must use only the async `reqwest::Client`
    // and `tokio::fs`; never `reqwest::blocking::*`. A blocking-reqwest call
    // from inside `block_on` would try to spin a nested runtime on this same
    // thread and panic with "Cannot start a runtime from within a runtime".
    // `new_current_thread` is the right choice for a one-shot CLI verb (cheap,
    // single-shot, no thread-pool overhead) but only as long as the helper
    // stays fully async.
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

    let network = match crate::network::OutboundNetwork::new() {
        Ok(network) => network,
        Err(e) => {
            eprintln!("Error: failed to build network client: {}", e);
            return 1;
        }
    };

    let result = rt.block_on(crate::commands::telegram::perform_send_image(
        &network,
        &bot,
        &args.path,
        args.caption.as_deref(),
    ));

    match result {
        Ok(()) => {
            log::info!(
                "[cli] telegram-send-image: bot={} path={} caption_present={}",
                bot.id,
                args.path,
                args.caption.is_some()
            );
            crate::cli_println!("Sent: bot={} path={}", bot.id, args.path);
            0
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            1
        }
    }
}

#[cfg(test)]
mod tests;
