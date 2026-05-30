# Telegram bridge

For developers who want to monitor a long-running agent from their phone, or kick off a task while away from the keyboard. The bridge attaches a Telegram bot to one session: PTY output streams to the chat, your replies stream back to the agent.

For end-to-end setup (creating the bot, configuring AC, attaching to a session) see [`docs/integrations/telegram.md`](../integrations/telegram.md). This page documents the feature surface and trade-offs.

## What it does

Once a bot is attached to a session:

- **Outbound**: PTY output is parsed by a vt100 cleaner, filtered (no spinners, box-drawing, low-alpha rows), chunked at 4000 characters, rate-limited, and sent to Telegram as plain text messages.
- **Inbound**: Telegram messages from the bot's authorized chat are written into the session's PTY as if you typed them.
- **Images and files**: AC can send photos/images and documents to a configured bot with `telegram-send-image`, including screenshots captured by agents during a run.

The bridge is per session. You can attach different bots to different sessions, or no bot at all.

## When to use it

- Overnight builds. Send the agent the prompt before bed; check the chat in the morning; reply if it asks a question.
- Long refactors. Watch periodic status updates from your phone while doing other work.
- Visual status. Ask an agent to capture a screenshot of a report, app state, or test result and send it to Telegram.
- Remote operations. Kick off a `gh pr review` from anywhere with a Telegram client.

## When not to use it

- High-volume real-time PTY output. Telegram rate-limits and chunking add latency; bursty TUI output may be heavily filtered before it reaches the chat.
- Sensitive content. Telegram is not end-to-end encrypted by default (only Secret Chats are). The bridge sends terminal output through Telegram's servers - see [`PRIVACY.md`](../../PRIVACY.md).
- Untrusted networks. The bot token grants control of the bot's chats; if you commit `settings.json` by accident you must rotate.

## Coding-agent reader modes

The bridge has agent-aware readers for specific coding agents that emit JSONL transcripts:

| Coding agent | Reader |
|---|---|
| Claude Code | JSONL transcript reader (cleaner output than raw PTY) |
| Codex | JSONL reader |
| Gemini | JSONL reader |
| Anything else | Generic vt100 PTY reader |

The reader is picked automatically based on the session's coding agent. Generic shells fall through to the PTY reader.

## What you see in Telegram

A typical message thread looks like:

```
[claude] Reading src/lib.rs ...
[claude] Patched 3 occurrences of `foo` to `bar`.
[claude] Running cargo check ...
[claude] cargo check succeeded.
```

Spinners, progress bars, and chrome are filtered out. Only "stable" output rows (rows that did not change for ~800ms) are emitted.

## What you can send back

Plain text. Your message is written verbatim into the session's PTY as if you typed it on the keyboard, followed by a newline. Multi-line messages are sent line by line.

There is no slash-command syntax - anything you would type in the terminal works.

Incoming messages are accepted by configured Telegram `chat_id`, not by a per-user allowlist. In a group chat, any group member whose message reaches the bot can send input to the attached session. Use a private chat or trusted private group only. For details, see [Telegram setup: Security model](../integrations/telegram.md#security-model).

## Sending images and screenshots

Use `agentscommander telegram-send-image` to send a photo, screenshot, or file from any shell that can access your configured bot. Images with `jpg`, `jpeg`, `png`, or `webp` extensions are sent as Telegram photos when they fit the size limit; other files are sent as documents.

For the full command and setup flow, see [Telegram setup](../integrations/telegram.md#sending-photos-images-and-screenshots-from-the-cli).

## State and indicators

The bridge state appears as a small Telegram icon on the session item:

| Indicator | Meaning |
|---|---|
| Solid icon | Attached and healthy. |
| Red dot | The last poll or send failed. Open DevTools (**Help → Toggle DevTools**) and look at `telegram_bridge_error` events. |
| No icon | No bot attached to this session. |

Detach with **right-click → Detach Telegram bot**. Detaching stops all bridge traffic.

## Privacy and code locations

- All bridge code lives in `src-tauri/src/telegram/`.
- Token, chat ID, and bot config are stored locally in `settings.json` (or the equivalent for your portable instance).
- The bridge contacts only `api.telegram.org`. No AC servers, no telemetry.

For the data-flow detail, see [`PRIVACY.md`](../../PRIVACY.md).

## See also

- [Telegram setup](../integrations/telegram.md) - step-by-step
- [`docs/troubleshooting.md#telegram-bridge`](../troubleshooting.md#telegram-bridge) - common failures
