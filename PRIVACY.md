# Privacy Policy

**Agents Commander** is a local desktop application. It does not collect telemetry, analytics, or usage data. There are no tracking mechanisms, no crash reporting services, and no automatic update checks.

All configuration and session data is stored locally on your machine in `~/.agentscommander/`.

## Network Features

The following features transmit data to external services **only when explicitly enabled and initiated by the user**:

### Telegram Bridge

When the user attaches a Telegram bot to a terminal session:

- **Data sent**: Terminal output text (filtered and rate-limited) is sent to the [Telegram Bot API](https://core.telegram.org/bots/api) (`api.telegram.org`)
- **Data received**: Messages sent by the user via Telegram are written to the terminal session
- **When**: Only while a bot is actively attached to a session. Detaching the bot stops all communication
- **Credentials**: The Telegram bot token and chat ID are configured by the user and stored locally in `~/.agentscommander/settings.json`

### Voice-to-Text

When the user activates voice recording:

- **Data sent**: Audio recording (WebM/Opus format) is sent to the [Google Gemini API](https://ai.google.dev/) (`generativelanguage.googleapis.com`) for transcription
- **Data received**: Transcribed text, which is then written to the terminal session
- **When**: Only when the user explicitly presses the record button and stops recording
- **Credentials**: The Gemini API key is configured by the user and stored locally in `~/.agentscommander/settings.json`

### Inter-Agent Messaging

The internal messaging system between agents is **local by default**: the file-based path writes Markdown files into `messaging/` directories inside each workgroup and inside the Root Agent directory, and other delivery paths keep message content in queues of their own. AC sends message content to no service of its own. A destination you select yourself, such as `send --outbox`, can place a message outside those locations, including on another machine.

## What Is NOT Transmitted

- No telemetry or analytics
- No crash reports
- No automatic update checks
- No fingerprinting or device identification
- No data to Agents Commander developers or any third party beyond the services listed above

## Credential Storage

API keys and tokens are stored in plaintext in `~/.agentscommander/settings.json`. This file is local to your machine. Users are responsible for securing access to their system account.

## Third-Party Services

When the optional features above are enabled, the respective third-party privacy policies apply:

- [Telegram Privacy Policy](https://telegram.org/privacy)
- [Google API Privacy Policy](https://policies.google.com/privacy)

## Contact

For privacy questions or concerns, open an issue at [github.com/mblua/agentscommander](https://github.com/mblua/agentscommander/issues).
