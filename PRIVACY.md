# Privacy Policy

**Agents Commander** is a local desktop application. It does not collect telemetry, analytics, or usage data. There are no tracking mechanisms, no crash reporting services, and no automatic update checks.

All configuration and session data is stored locally on your machine in the
authoritative `<config-dir>`. For the canonical Linux DEB,
`<config-dir>` is `$XDG_CONFIG_HOME/agentscommander`, or
`$HOME/.config/agentscommander` when `XDG_CONFIG_HOME` is unavailable. Raw
portable binaries use the executable-relative instance directory documented in
[Portable instances](docs/features/portable-instances.md).

## Network Features

The following features transmit data to external services **only when explicitly enabled and initiated by the user**:

### Telegram Bridge

When the user attaches a Telegram bot to a terminal session:

- **Data sent**: Terminal output text (filtered and rate-limited) is sent to the [Telegram Bot API](https://core.telegram.org/bots/api) (`api.telegram.org`)
- **Data received**: Messages sent by the user via Telegram are written to the terminal session
- **When**: Only while a bot is actively attached to a session. Detaching the bot stops all communication
- **Credentials**: The Telegram bot token and chat ID are configured by the user and stored locally in `<config-dir>/settings.json`

### Voice-to-Text

When the user activates voice recording:

- **Data sent**: Audio recording (WebM/Opus format) is sent to the [Google Gemini API](https://ai.google.dev/) (`generativelanguage.googleapis.com`) for transcription
- **Data received**: Transcribed text, which is then written to the terminal session
- **When**: Only when the user explicitly presses the record button and stops recording
- **Credentials**: The Gemini API key is configured by the user and stored locally in `<config-dir>/settings.json`

### Inter-Agent Messaging (Phone)

The internal messaging system between agents is **entirely local**. Messages
are stored as JSON files in `<config-dir>/conversations/`. No external network
calls are made.

## What Is NOT Transmitted

- No telemetry or analytics
- No crash reports
- No automatic update checks
- No fingerprinting or device identification
- No data to Agents Commander developers or any third party beyond the services listed above

## Credential Storage

API keys and tokens are stored in plaintext in `<config-dir>/settings.json`.
On Linux, AgentsCommander requires the config root and private directories to
be owner-only (`0700`) and security-bearing files to be owner-only (`0600`).
These permissions do not protect against another process running as the same
user. Users are responsible for securing access to their system account.

## Third-Party Services

When the optional features above are enabled, the respective third-party privacy policies apply:

- [Telegram Privacy Policy](https://telegram.org/privacy)
- [Google API Privacy Policy](https://policies.google.com/privacy)

## Contact

For privacy questions or concerns, open an issue at [github.com/mblua/agentscommander](https://github.com/mblua/agentscommander/issues).
