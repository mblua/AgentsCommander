# Privacy Policy

**Agents Commander** is a local desktop application. It does not collect telemetry, analytics, or usage data. There are no tracking mechanisms and no crash reporting services. It makes a small number of automatic outbound requests. The [Network Features](#network-features) section describes them together with the features you start yourself.

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
- **When**: On startup. AC contacts the registry only when the cache is missing or older than 24 hours. The last check time and the last seen version are cached in `update-check.json` in your config directory, and AC reuses the cached result inside the 24-hour window without contacting the registry. A failed or offline check is not cached, so the next startup tries again.
- **Limits**: 10-second request timeout, and the response body is capped at 64 KB.
- **Data disclosed**: Your IP address, the request time, and a `User-Agent` header of `agentscommander/<version>`. No account, no identifier, and no session content.
- **Turn it off**: Clear **Notify me when a new version is available** in Settings, or set `npmUpdateNotificationsEnabled` to `false` in `settings.json` in your config directory. The setting is on by default and also silences the update notice that the CLI prints.

### Home Panel Markdown

**Automatic.** The Home panel shows a getting-started page maintained in the Agents Commander repository. The app downloads the page from GitHub when it first renders the panel with no copy loaded.

- **Endpoint**: `https://raw.githubusercontent.com/mblua/AgentsCommander/main/docs/home-en.md`
- **When**: When the Home panel is first shown with no copy loaded. The main window shows the Home panel at startup, so this request normally runs at app start. The page is kept in memory only and is not written to disk.
- **Limits**: 5-second request timeout, and the response body is capped at 256 KB.
- **Data disclosed**: Your IP address, the request time, and a `User-Agent` header of `agentscommander/<version>`. No account, no identifier, and no session content.
- **Turn it off**: There is no setting for this request.

### Remote Blocking-Menu Patterns

**Automatic.** On startup, in a detached background task, Agents Commander downloads the published blocking-menu pattern file from the Agents Commander repository. The task never blocks or delays startup, and it is fail-silent: a rejected or failed download shows nothing — no toast, no notice — and a previously downloaded copy stays in place and keeps applying. A validated copy takes effect at the next start; the running app never reloads patterns.

- **Endpoint**: `https://raw.githubusercontent.com/mblua/AgentsCommander/main/remote-resources/blocking-menus/v1/settings-blocking-menus.json`
- **When**: On startup. AC downloads at most once per 24 hours. The time of the last attempt is stored in `blocking-menus-remote-check.json` in your config directory, and every attempt counts, a failed or offline one included, so the next try is 24 hours later.
- **Limits**: 10-second timeout covering the request and the response body, and the response body is capped at 64 KB.
- **Validation**: The whole file is rejected, never one entry, when any check fails: an HTTP status other than 200, a body over 64 KB, invalid JSON, a `schemaVersion` other than 1, a non-empty `byAgent`, more than 200 entries, a pattern over 512 bytes, a notification over 200 bytes or containing a control character, a pattern that does not compile within its size limit, or a pattern matching an empty line or a built-in sample of ordinary terminal lines.
- **Data disclosed**: Your IP address, the request time, and a `User-Agent` header of `agentscommander/<version>`. No account, no identifier, and no session content.
- **Stored**: A file that passes every check is written to `settings-blocking-menus.remote.json` in your config directory, and only by the download. Its `note` records the source URL, the download time, and the source ref `main`. AC validates the stored copy again at every start; when it fails, AC ignores it (one warning line in the log) and the patterns shipped in the app apply.
- **Turn it off**: Clear **Download blocking-menu pattern updates from GitHub** in **Settings > General**, or set `remoteBlockingMenusEnabled` to `false` in `settings.json` in your config directory. The setting is on by default. Turning it off stops the download only: a file already downloaded keeps applying until you delete `settings-blocking-menus.remote.json`.

### Coding-Agent Auto-Update

**Automatic, opt-in.** When you allow auto-update for a registered coding agent, Agents Commander runs that agent's update command at every app startup. AC runs the command from your catalog; in the shipped catalog that command is the vendor's own CLI, and the CLI contacts its vendor. AC does not contact those vendors itself.

- **Commands**: The update command comes from your coding-agent catalog. The shipped catalog uses `claude --update` (Anthropic), `codex update` (OpenAI), `hermes update --yes` (Nous Research), `pi update` (Earendil), `opencode upgrade` (Anomaly), and `agy update` (Google).
- **When**: At every app startup, for each command you allowed. For each command, the first startup after you register it asks once whether to update it; the default answer is No. An unanswered or timed-out question updates nothing and is asked again at the next startup.
- **Data disclosed**: AC sends nothing. The vendor's CLI decides what it sends to its vendor, so the vendor's privacy policy applies.
- **Turn it off**: Set **Auto-update** to **No** in Settings. Answering No to the startup question also records the answer and stops the question.

### Agency Template Download

**User-initiated.** When you update the Agency templates from the app or run `agentscommander agency-templates update`, AC downloads the default Agency Agents repository from GitHub with your local `git` binary.

- **Endpoint**: `https://github.com/msitarzewski/agency-agents`, the default repository. The `--repo` option can point the download at another repository.
- **When**: Only during an Agency-template update. No Agency download runs at startup.
- **Data disclosed**: Your IP address and the request time. The download is a read-only `git` fetch of a public repository. AC sends no account, identifier, or session content.

### Room Repository Clone

**User-initiated.** When you create a Room from a team, or add a member to an existing Room, AC clones each repository URL in the team into that Room with your local `git` binary.

- **Command**: `git clone --depth 1 <url>`, once per repository that is not already in the Room.
- **Destination**: The host named by the team's repository URLs. You configure those URLs, and they can point at GitHub, GitLab, or any other server.
- **When**: When you create a Room from a team, or add a member to an existing Room. AC clones only the repositories that are not already in the Room.
- **Data disclosed**: Your IP address, the request time, and anything `git` sends to authenticate, such as a credential from your git configuration. The repository host is the one you configured, so its privacy policy applies.
- **Turn it off**: There is no separate setting: remove the repository URLs from the team, and AC clones nothing.

### Container Image Pull

**User-initiated.** When you start a session on the Container runtime and the Docker image is not on your machine, Docker pulls the image from its registry.

- **Image**: The Docker image you configure for the agent in Settings, or the `AGENTSCOMMANDER_CONTAINER_IMAGE` variable when you leave the field blank. There is no built-in image.
- **Destination**: The registry named by the image reference, for example Docker Hub.
- **When**: Only when a container session starts and the image is missing locally.
- **Data disclosed**: Docker sends the request, not AC. AC passes the image name to `docker run` and sends nothing itself. The registry is the one named by your image, so its privacy policy applies.
- **Turn it off**: There is no separate setting: Docker contacts a registry only when the image is missing locally, and it uses an image already on your machine as is.

## What Is NOT Transmitted

- No telemetry or analytics
- No crash reports
- No fingerprinting or device identification
- No data to Agents Commander developers or to any third party beyond the destinations described above
- No session content, prompts, or terminal output in the npm update check, the Home panel request, the blocking-menu pattern download, the coding-agent update commands, the Agency template download, the Room repository clone, or the container image pull
- No terminal snapshot content to a third-party snapshot or rendering service

## Credential Storage

API keys and tokens are stored in plaintext in `~/.agentscommander/settings.json`. This file is local to your machine. Users are responsible for securing access to their system account.

## Third-Party Services

When a network feature contacts a third-party service, the respective third-party privacy policy applies:

- [Telegram Privacy Policy](https://telegram.org/privacy)
- [Google API Privacy Policy](https://policies.google.com/privacy)
- [npm Privacy Policy](https://www.npmjs.com/policies/privacy)
- [GitHub Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-privacy-statement)
- Coding-agent vendors: when you allow startup auto-update, each coding agent's own CLI contacts its vendor. The shipped catalog covers Anthropic, OpenAI, Nous Research, Earendil, Anomaly, and Google. The vendor's privacy policy applies.
- Destinations you choose: your team repository hosts, your container registries, and the operator-configured `AGENTSCOMMANDER_API_URL`. These are not fixed services, so the privacy policy of the host you or your operator point AC at applies.

## Contact

For privacy questions or concerns, open an issue at [github.com/mblua/agentscommander](https://github.com/mblua/agentscommander/issues).
