# Telegram bridge setup

For developers who want a step-by-step from "no bot" to "session output streams to my phone." End-to-end in ~5 minutes.

For what the bridge does and when to use it, see [`docs/features/telegram-bridge.md`](../features/telegram-bridge.md).

## 1. Create a Telegram bot

In any Telegram client, talk to [@BotFather](https://t.me/BotFather):

1. Send `/newbot`.
2. Pick a display name (e.g. *My AC Bot*).
3. Pick a username ending in `bot` (e.g. *my_ac_bot*).
4. BotFather replies with a **bot token** — a string like `1234567890:ABCDEF...`.

Save the token. Anyone with it can control the bot.

## 2. Get your chat ID

The bridge must know which Telegram chat to send to. Easiest path:

1. Open a chat with your new bot in Telegram and click **Start**.
2. Send the bot any message (e.g. `/start`).
3. Visit `https://api.telegram.org/bot<TOKEN>/getUpdates` in your browser (replace `<TOKEN>` with the bot token).
4. Find your message in the JSON and copy the `chat.id` value (an integer; for group chats it is negative).

You can also use [@RawDataBot](https://t.me/raw_data_bot) to look up your numeric ID without curl.

## 3. Configure the bot in AC

In AgentsCommander:

1. Open **Settings → Integrations → Telegram**.
2. Click **+ Bot**.
3. Fill in:
   - **Label** — a human-friendly name (e.g. *Personal bot*).
   - **Token** — the BotFather token from step 1.
   - **Chat ID** — the integer from step 2.
4. Click **Test** to fire a hello message. If your phone buzzes, you are configured.
5. **Save**.

The bot configuration is stored locally in `settings.json` under `telegramBots[]`. The token is in plaintext — protect access to your user account.

## 4. Attach to a session

In the sidebar, right-click any session → **Telegram → Attach <bot label>**.

The Telegram icon appears on the session item. From this moment:

- Every "stable" line of PTY output goes to the chat (filtered, vt100-cleaned, rate-limited).
- Every message you send the bot is written into the session's PTY as if you typed it.

Detach with **right-click → Detach Telegram bot**.

## Sending an image from the CLI

You can send a one-off image or file to a configured bot from any shell:

```bash
agentscommander telegram-send-image \
  --path "C:/path/to/screenshot.png" \
  --caption "Build finished" \
  --bot-label "Personal bot"
```

| Flag | Meaning |
|---|---|
| `--path` | File to upload. Symlinks are rejected. |
| `--caption` | Optional caption (clamped to Telegram's 1024 UTF-16-code-unit limit). |
| `--bot-id` | Pick a bot by id (mutually exclusive with `--bot-label`). |
| `--bot-label` | Pick a bot by label. |

Files ≤10 MB with extensions `jpg/jpeg/png/webp` use `sendPhoto`. Everything else (including GIF) falls back to `sendDocument`, capped at 50 MB.

Useful for ending an overnight run with a screenshot of the test report.

## Privacy

The bridge sends terminal output through Telegram's servers — see [`PRIVACY.md`](../../PRIVACY.md). Telegram is not end-to-end encrypted outside Secret Chats. If you handle sensitive data, do not attach a bot to those sessions.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `Test` button shows a chat-id error | The bot has not received any message from you yet | Send the bot `/start` in Telegram first |
| Bot icon turns red on a session | Last poll or send failed | Open DevTools (**Help → Toggle DevTools**), look at `telegram_bridge_error` events |
| Chat floods with escape codes | The PTY emitted a TUI mode AC's cleaner does not recognize | Update AC; open an issue with the agent name and what you were running |
| `--bot-label` errors with "no bot matches" | Label mismatch (exact-match) | Check **Settings → Integrations → Telegram** for the canonical label |

More: [`docs/troubleshooting.md#telegram-bridge`](../troubleshooting.md#telegram-bridge).

## See also

- [Telegram bridge — feature page](../features/telegram-bridge.md)
- [CLI reference — `telegram-send-image`](../reference/cli.md#telegram-send-image)
- [Settings reference — `telegramBots`](../reference/settings.md)
