# Telegram bridge setup

For developers who want a step-by-step from "no bot" to "session output streams to my phone." End-to-end in ~5 minutes.

For what the bridge does and when to use it, see [`docs/features/telegram-bridge.md`](../features/telegram-bridge.md).

## 1. Create a Telegram bot

In any Telegram client, talk to [@BotFather](https://t.me/BotFather):

1. Send `/newbot`.
2. Pick a display name (e.g. *My AC Bot*).
3. Pick a username ending in `bot` (e.g. *my_ac_bot*).
4. BotFather replies with a **bot token** - a string like `1234567890:ABCDEF...`.

Save the token. Anyone with it can control the bot.

## 2. Choose the chat

The bridge authorizes by Telegram `chat_id`. Choose one chat for the bot:

- **Recommended**: a private chat between you and the bot.
- **Also supported**: a trusted private group.

Do not bind the bot to a public group. In a group, every member whose message is delivered to the bot can send input to the attached session.

Open the intended chat now. For a private chat, click **Start**. For a group, add the bot to the private group and confirm every member is trusted. Wait to send a fresh message until step 3. The **Test** button discovers the chat ID from pending bot updates, so the newest eligible Telegram message should come from the chat you want to authorize. A stale update from another chat can be selected if it is the latest pending text or voice message.

## 3. Configure the bot in AC

In AgentsCommander:

1. Open **Settings → Integrations → Telegram**.
2. Click **+ Bot**.
3. Fill in:
   - **Label** - a human-friendly name (e.g. *Personal bot*).
   - **Token** - the BotFather token from step 1.
4. In Telegram, send a fresh text or voice message from the intended private chat or trusted group.
5. Click **Test**. AC reads recent bot updates, finds the latest text or voice message update, extracts that update's `chat_id`, and sends `agentscommander connected` back to that chat.
6. Confirm the modal shows **Connected** and a chat ID.
7. Click **Save**.

If **Test** reports that no messages were found, send another message to the bot in Telegram and click **Test** again.

**Test** writes the discovered chat ID into the Settings modal draft only. The bot configuration is persisted to `settings.json` after you click **Save**.

## Security model

Telegram bridge authorization is chat-level:

- AC stores a bot token and one configured `chat_id` for each Telegram bot.
- The listener skips every incoming update whose Telegram `chat_id` does not exactly match the configured chat ID.
- AC does not maintain an app-side whitelist of Telegram user `from_id` values.
- In a private chat, another person messaging the same bot has a different private `chat_id`, so that message is ignored.
- In a group, the configured `chat_id` is the group chat. Any group member whose text or voice message is delivered to the bot can send accepted input. Telegram privacy mode and group permissions can affect which messages the bot receives, but AC does not enforce a per-member allowlist after the group `chat_id` matches.
- Delivered text messages from the bound chat are written into the attached session. Delivered voice messages from the bound chat can be transcribed and injected when a Gemini API key is configured.

Protect the bot token. Anyone with the token can call Telegram Bot API methods for that bot. Keep `settings.json` and logs private, and rotate the token in BotFather if it is exposed.

## 4. Attach to a session

In the sidebar, right-click any session → **Telegram → Attach <bot label>**.

The Telegram icon appears on the session item. From this moment:

- Every "stable" line of PTY output goes to the chat (filtered, vt100-cleaned, rate-limited).
- Every message you send the bot is written into the session's PTY as if you typed it.

Detach with **right-click → Detach Telegram bot**.

## Sending photos, images, and screenshots from the CLI

You can send a one-off photo, image, screenshot, or file to a configured bot from any shell. Agents can use the same command to send screenshots they captured during a run:

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

Use it to end an overnight run with a screenshot of the test report, a UI state, or another image artifact.

## Privacy

The bridge sends terminal output through Telegram's servers - see [`PRIVACY.md`](../../PRIVACY.md). Telegram is not end-to-end encrypted outside Secret Chats. If you handle sensitive data, do not attach a bot to those sessions.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `Test` reports no messages or picks the wrong chat | The bot has no fresh pending text or voice message from the intended chat, or a stale update from another chat is newer | Send a new text or voice message in the intended private chat or trusted group, then click **Test** again. Confirm the shown chat ID before **Save** |
| Bot icon turns red on a session | Last poll or send failed | Open DevTools (**Help → Toggle DevTools**), look at `telegram_bridge_error` events |
| Chat floods with escape codes | The PTY emitted a TUI mode AC's cleaner does not recognize | Update AC; open an issue with the agent name and what you were running |
| `--bot-label` errors with "no bot matches" | Label mismatch (exact-match) | Check **Settings → Integrations → Telegram** for the canonical label |

More: [`docs/troubleshooting.md#telegram-bridge`](../troubleshooting.md#telegram-bridge).

## See also

- [Telegram bridge - feature page](../features/telegram-bridge.md)
- [CLI reference - `telegram-send-image`](../reference/cli.md#telegram-send-image)
- [Settings reference - `telegramBots`](../reference/settings.md)
