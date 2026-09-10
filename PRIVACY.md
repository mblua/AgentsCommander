# Privacy Policy

**Agents Commander** is a local desktop application. It does not collect telemetry, analytics, or usage data. There are no tracking mechanisms and no crash reporting services. It does make two outbound requests automatically. The [Network Features](#network-features) section describes them together with the features you start yourself.

All configuration and session data is stored locally on your machine in `~/.agentscommander/`.

## Network Features

Agents Commander transmits data to external services in two ways: when you enable or start a feature, and through automatic requests. Each entry below states which it is, when it runs, and how to turn it off where an option exists.

### Telegram Bridge

**User-initiated.** When the user attaches a Telegram bot to a terminal session:

- **Data sent**: Terminal output text (filtered and rate-limited) is sent to the [Telegram Bot API](https://core.telegram.org/bots/api) (`api.telegram.org`)
- **Data received**: Messages sent by the user via Telegram are written to the terminal session
- **When**: Only while a bot is actively attached to a session. Detaching the bot stops all communication
- **Credentials**: The Telegram bot token and chat ID are configured by the user and stored locally in `~/.agentscommander/settings.json`

### Voice-to-Text

**User-initiated.** When the user activates voice recording:

- **Data sent**: Audio recording (WebM/Opus format) is sent to the [Google Gemini API](https://ai.google.dev/) (`generativelanguage.googleapis.com`) for transcription
- **Data received**: Transcribed text, which is then written to the terminal session
- **When**: Only when the user explicitly presses the record button and stops recording
- **Credentials**: The Gemini API key is configured by the user and stored locally in `~/.agentscommander/settings.json`

### Inter-Agent Messaging

The internal messaging system between agents is **local by default**: the file-based path writes Markdown files into `messaging/` directories inside each room and inside the Root Agent directory, and other delivery paths keep message content in queues of their own. AC sends message content to no service of its own. A destination you select yourself, such as `send --outbox`, can place a message outside those locations, including on another machine.

### Terminal Snapshots

**User-initiated.** Terminal snapshots are off by default. When the user enables `terminalSnapshotsEnabled`, an identity-authorized Root Agent or same-room Orchestrator can request the current backend terminal viewport as JSON or deterministic PNG.

- **Data processed locally**: Current visible backend rows, cells, text, colors, represented styles, cursor, dimensions, selected session metadata, and fidelity metadata. Terminal content can include passwords, tokens, source code, prompts, and personal data. Agents Commander does not redact it.
- **Host transport**: A host requester exchanges bounded transient files in dedicated requester-side terminal snapshot directories. Snapshot content does not enter ordinary messages, conversations, delivered or rejected message artifacts, or PTY-input state. The daemon normally removes identity-stable protocol files after use or 60 seconds. A crash plus removal of the only project registration can leave an undiscoverable residual.
- **Container API transmission**: An automatically bound container Orchestrator can send one authenticated request to the operator-configured `AGENTSCOMMANDER_API_URL`. The response can contain the JSON viewport or PNG base64. This is transmission between the local Agents Commander daemon and the user's container or configured private endpoint, not to Agents Commander developers or a third-party snapshot service. Whether HTTP is encrypted depends on the URL the operator configured.
- **Caller output**: A requested PNG is a caller-owned persistent file and remains until the caller deletes it. A failed write can leave an incomplete file. JSON is written to requester stdout.
- **Memory and deletion limits**: Snapshot buffers are bounded but are not locked or zeroized and can appear in swap or crash dumps. File cleanup is not forensic secure erasure. Windows inherited same-user ACLs are not a boundary against a compromised local account.
- **Audit**: Snapshot audit contains operational metadata only, such as verified identities, format, selected session/backend, dimensions, sequence, capture time, byte count, status, and fixed reason code. It excludes terminal text, JSON, PNG/base64, ANSI, title, credentials, nonce, output path, and content hash. Audit is fail-soft, not compliance-grade.

Agents Commander never captures an OS window, monitor, desktop, WebView, or unrelated pixel for this feature. See [Terminal snapshots](docs/features/terminal-snapshots.md) for the complete authorization, fidelity, output, and cleanup contract.

### npm Update Check

**Automatic.** On startup, in a detached background task, Agents Commander asks the npm registry whether a newer published version of `@mblua/agentscommander` exists, and shows an in-app notice when one does. The task never blocks or delays startup, and it is fail-silent: a timeout, a network error, or an unusable response produces no notice and no error.

- **Endpoint**: `https://registry.npmjs.org/-/package/@mblua%2Fagentscommander/dist-tags`
- **When**: At most once every 24 hours. The last check time and the last seen version are cached in `update-check.json` in your config directory, and AC reuses the cached result inside the 24-hour window without contacting the registry.
- **Limits**: 10-second request timeout, and the response body is capped at 64 KB.
- **Data disclosed**: Your IP address, the request time, and a `User-Agent` header of `agentscommander/<version>`. No account, no identifier, and no session content.
- **Turn it off**: Clear **Notify me when a new version is available** in Settings, or set `npmUpdateNotificationsEnabled` to `false` in `~/.agentscommander/settings.json`. The setting is on by default and also silences the update notice that the CLI prints.

### Home Panel Markdown

**Automatic.** The Home panel shows a getting-started page maintained in the Agents Commander repository. The app downloads the page from GitHub when it first renders the panel with no copy loaded.

- **Endpoint**: `https://raw.githubusercontent.com/mblua/AgentsCommander/main/docs/home-en.md`
- **When**: When the Home panel is first shown with no copy loaded. The main window shows the Home panel at startup, so this request normally runs at app start. The page is kept in memory only and is not written to disk.
- **Limits**: 5-second request timeout, and the response body is capped at 256 KB.
- **Data disclosed**: Your IP address, the request time, and a `User-Agent` header of `agentscommander/<version>`. No account, no identifier, and no session content.
- **Turn it off**: There is no setting for this request.

### Agency Template Download

**User-initiated.** When you update the Agency templates from the app or run `agentscommander agency-templates update`, AC downloads the default Agency Agents repository from GitHub with your local `git` binary.

- **Endpoint**: `https://github.com/msitarzewski/agency-agents`, the default repository. The `--repo` option can point the download at another repository.
- **When**: Only during an Agency-template update. No Agency download runs at startup.
- **Data disclosed**: Your IP address and the request time. The download is a read-only `git` fetch of a public repository. AC sends no account, identifier, or session content.

## What Is NOT Transmitted

- No telemetry or analytics
- No crash reports
- No fingerprinting or device identification
- No data to Agents Commander developers or any third party beyond the services listed above
- No session content, prompts, or terminal output in the npm update check, the Home panel request, or the Agency template download
- No terminal snapshot content to a third-party snapshot or rendering service

## Credential Storage

API keys and tokens are stored in plaintext in `~/.agentscommander/settings.json`. This file is local to your machine. Users are responsible for securing access to their system account.

## Third-Party Services

When Agents Commander contacts a third-party service, the respective third-party privacy policy applies:

- [Telegram Privacy Policy](https://telegram.org/privacy)
- [Google API Privacy Policy](https://policies.google.com/privacy)
- [npm Privacy Policy](https://www.npmjs.com/policies/privacy)
- [GitHub Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-privacy-statement)

## Contact

For privacy questions or concerns, open an issue at [github.com/mblua/agentscommander](https://github.com/mblua/agentscommander/issues).
