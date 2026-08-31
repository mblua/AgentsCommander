# Security model

For developers evaluating whether AgentsCommander is safe to put in their workflow. Threat model first, mitigations second, known gaps last.

For privacy and data-flow questions (what leaves the machine and when), see [`PRIVACY.md`](../PRIVACY.md). For how to report a vulnerability, see [`SECURITY.md`](../SECURITY.md).

## Threat model

AgentsCommander is a **local desktop application** that spawns coding-agent CLIs and routes messages between them. The trust boundaries are:

1. **The user.** Operates the GUI, picks coding agents, accepts the actions agents propose. Fully trusted.
2. **The coding agents.** Claude Code, Codex, Antigravity, and Pi. They run in real PTYs as full user-level processes and are trusted with the user's local file system to the same degree the user trusts them when launched directly.
3. **The optional network endpoints.** Telegram Bot API and the Google Gemini API for voice-to-text. Only contacted when the user explicitly enables those features.
4. **The disk.** AC keeps its state in files, on this machine by default. Under the [application config directory selected by the exact binary version](features/portable-instances.md#config-directory-rule) you will find configuration, persisted sessions, the Root Agent directory `ac-root-agent/` including its `messaging/` Markdown message files, and the running instance's delivery queue at `instances/<instance-id>/outbox/`. In your Project AC Root you will find team configuration under `_team_<name>/`, each room's `messaging/` Markdown message files, and each agent replica's delivery queue at `<replica>/<instance-dir>/outbox/`, where `<instance-dir>` is that replica's dot-prefixed per-instance directory. **Message data is written under both roots**, optional features and per-call overrides can put it outside either root and need not keep it on this machine, and neither list is exhaustive, so treat this as where to start looking rather than as an inventory of everything AC writes.

AC does not:

- Send any telemetry. No analytics, no crash reports. The one automatic network check is the npm update check: on startup AC queries the npm registry for the latest published version (throttled to at most once per 24 hours, fail-silent, opt-out via `npmUpdateNotificationsEnabled: false`). No user data leaves the machine.
- Open inbound network ports unless you explicitly enable the embedded web server or the control-plane API server (both off by default; loopback-only when on).
- Run any agent or shell process without the user starting it.

## What an agent can reach

When you launch a session, the coding agent inherits:

- The session's working directory and everything reachable from it.
- The environment variables of the AC process (including `PATH`, `HOME`, and any keys you may have exported).
- The user-level filesystem and network permissions you yourself have.

Two opt-in listeners widen who can reach that surface from outside the machine: the [control-plane API](features/control-plane-api.md) and the [remote web UI](features/remote-web-ui.md). Both are off by default and both are described on their own pages.

This is the same surface area the coding agent has when you launch it from your own terminal. AC adds visibility and coordination; it does **not** add a sandbox. If the underlying agent can `rm -rf ~/`, AC will let it.

Pi auto-resume does not add a state-reading boundary. AC does not inspect or copy `~/.pi/agent/`, `PI_CODING_AGENT_SESSION_DIR`, or `--session-dir` paths, and a Pi option that names Claude does not trigger AC's Claude projects-directory probe. AC does not provision Pi credentials or map Pi state into containers. It only adds `--continue` to an eligible configured known-state launch; [Pi remains responsible for session lookup and errors](integrations/coding-agents.md#no-ac-side-pi-state-probe-or-fallback).

If you need stricter isolation, run AC inside a virtualized environment (WSL2, Linux VM, devcontainer) and limit the VM's network and filesystem access there.

## Inter-agent routing

The `send` CLI enforces team-membership and orchestrator-only routing at the daemon mailbox boundary. Highlights:

- A worker can only message peers it shares a team with, plus its orchestrator.
- An orchestrator can message any of its team members.
- Cross-team coordination requires the Root Agent (Project AC Root-level orchestrator).

Token validation:

- The CLI validates token **shape** only (UUID, root token, or master token).
- The daemon mailbox validates **identity**: per-session token bound to the live session set.
- A valid UUID from a different binary instance will pass the CLI's shape check but be rejected at the mailbox.

The current ordinary-message and task-operation trust model accepts the caller's self-reported `--root` and per-session UUID. See the in-source notes in `task-set-title` and `task-append-body` for the known weakness and follow-up work. The privileged PTY-input operation does not use that weaker path.

## Privileged exact PTY input

PTY input is a dedicated actuation contract, not an ordinary message or shell-execution feature. Accepted text is validated UTF-8, capped at 65,536 bytes, and written only to one already running trusted coding-agent PTY. It is never supplied to `Command`, shell `-c` or `/C` evaluation, argv, environment variables, or a filesystem path. Shell metacharacters are ordinary text. The sender's own shell may still transform a command-line argument before AC receives it, so stdin is recommended for multiline and sensitive values.

Authority is based on live physical identity:

- A live identity-verified room orchestrator may target one verified non-orchestrator member in the same exact project and room.
- A live canonical local Root Agent may target one verified room orchestrator. Root PTY authority is host-only.
- A container orchestrator requires a fresh automatically minted `pty-input` API scope bound to the exact live container session, credential generation, root filesystem object, transport route, and runtime-held credential hash.

Caller `from`, outbox location by itself, broad ordinary `can_communicate` results, a master credential, a manual API scope, cached registry state, role prose, and target spelling are not authority. Workers, origin orchestrators, cross-room or cross-project routes, orchestrator-to-orchestrator routes, Root-to-worker routes, aliases, wildcards, stale sessions, and handcrafted bindings fail before target lifecycle mutation or PTY input.

Security-bearing directories and files are read with bounded no-follow checks. AC rejects symlinks, Windows reparse points, hard-linked mutable files, directory-entry replacement, duplicate JSON keys, hierarchy ambiguity, and object-identity changes. Route entries retain a canonical CWD object identity and, for room replicas, a replica-anchor fingerprint. Authority and target identity are checked at ingress, dispatch start, after long awaits, and again inside the final SessionManager and IdleDetector boundary immediately before the first backend write.

All PTY writers share a per-session permit. One privileged operation retains it across the exact text write, the required Enter, and the redundant Enter, preventing user or automated bytes from splicing between phases. A logical target gate also serializes missing/exited session lifecycle work across host/API operations and ordinary room creates. Fixed OS lock stripes prevent a second daemon from reclaiming a suspended preparing or actuating owner.

The durable no-replay boundary is the transaction that changes the operation to `actuating`. That transaction returns payload bytes only after commit and clears payload, profile, mutable identity, and authority references from the live row. Before it, classified transient failures may retry under a bounded five-attempt policy. After it, no failure automatically replays text. A text-write failure, required-Enter failure, process restart, lost owner, final revalidation failure, or terminal-store failure is conservatively `indeterminate`.

Terminal rows, permanent idempotency tombstones, transition audit, API audit, events, reason files, and host artifacts retain metadata only: ids, verified sender/target, byte length, SHA-256, source plane, selected route, status, canonical timestamps, and fixed reason codes. They do not retain text, token, raw nonce, path, argv, environment, ticket, or arbitrary OS/parser error details. Nonterminal host requests, ignored request temporary files, and SQLite queue rows are sensitive until marker conversion, expiry, rejection, or actuation. SQLite WAL and SSD history are not claimed to provide forensic secure erasure.

`Injected` proves backend acceptance of the exact text and required first Enter only. It does not prove model consumption, understanding, or completion. `Rejected` proves zero writes before actuation. `Indeterminate` means replay is forbidden because actuation may have started. Keep the same operation ID for status lookup and never retry uncertainty under a new ID.

Residual boundaries remain explicit. A local administrator or fully compromised same-OS-user account that can inspect another process's memory or environment is outside this operation's defense. Ordinary messaging defects tracked under #139 do not widen PTY-input authority, but they remain relevant to the separate standard message plane. Model-consumption proof remains outside this feature.

## Authorized terminal snapshots

Terminal snapshots are a distinct read capability. They do not reuse ordinary message authorization, PTY-input actuation authority, or the interactive screenshot feature.

The allowed matrix is intentionally narrow:

- A live, physically verified canonical Root Agent may read a verified room Orchestrator or member in an active registered project. Root uses the host mailbox only.
- A live, physically verified room Orchestrator may read one verified non-Orchestrator member in the same exact project and room.
- An automatically bound live container Orchestrator requires the separate `terminal-snapshot` API scope and may read the same same-room member set through HTTP.
- Workers, origin agents, origin Orchestrators, manual API clients, stale sessions, static Root or master credentials, and cross-scope routes have no snapshot authority.

Target names must be exact canonical FQNs. Aliases, wildcards, filesystem directory names, Root and origin targets, self, session IDs, Orchestrator-to-Orchestrator, cross-room, and cross-project routes fail. `list-peers-lean --snapshot-targets` is identity-only discovery and grants no authority.

### No-liveness ordering

AgentsCommander first proves the live requester and the physical requester-to-target identity route. It does not query the target SessionManager, PTY route, backend, parser, or liveness before that route is authorized. A shape-valid missing, exited, live, or tampered unauthorized target therefore returns the same `not_authorized` status and body. This is a response, data, and lookup no-oracle property. Local filesystem and path-cache timing is not claimed to be constant-time.

Only after authorization can `target_unavailable` report that the verified target has no eligible persistent live session, or `snapshot_unavailable` report a temporary parser, route, liveness, restore, or purge condition.

### Point-in-time and TOCTOU checks

Capture copies one bounded active backend viewport under the parser lock at one output-sequence and timestamp boundary. It copies no inactive grid, title, icon name, raw ANSI stream, or parser escape buffer. Cell strings, serialization, rendering, file operations, and network work happen after parser and route locks are released.

Before disclosure, AgentsCommander rechecks both privacy gates, requester role and liveness, physical identities, team and project membership, selected session identity and backend, PTY route generation, restore and purge state, credential generation and binding on the API plane, and daemon shutdown. Any relevant change discards content as `authority_changed`. Normal terminal output or resize after capture does not invalidate the retained point-in-time model.

This design detects changes at the initial and final checks. It does not claim to detect an authority value that changes away and back entirely between those checks, or to make mutable authorization atomic with the final socket transmission after Axum accepts already-authorized bytes.

### Content confinement

Snapshots read the backend `vt100` 0.15.2 active viewport. They work when the frontend is hidden, minimized, detached, or never mounted. They never focus, select, wake, spawn, resize, repaint, or write to the target. They never call the Windows interactive screenshot path or any OS window, monitor, or desktop capture API.

JSON is compact ASCII-only and escapes terminal controls and non-ASCII text. PNG uses a bundled fixed font, fixed palette and metrics, bounded pure-Rust rasterization, and a strict RGB8 PNG validator. Neither format claims frontend scrollback, selection, theme, overlays, images, exact WebView pixels, or an application-frame boundary. See [Terminal snapshots](features/terminal-snapshots.md#what-the-snapshot-represents) for the complete fidelity contract.

There is no content redaction. The screen can contain credentials, source, prompts, and personal data. The disclosure setting is false by default, and both strict on-disk state and managed in-memory state must be true initially and finally. Missing, malformed, duplicated, linked, or unreadable security settings fail closed.

Host request and response content uses dedicated bounded transient directories, not room messaging, conversations, ordinary durable message artifacts, or PTY-input SQLite state. The daemon sweeps identity-stable protocol files after 60 seconds while they remain discoverable. A crash followed by removal of the only project registration can leave a file outside startup discovery. Operators must inspect only the exact requester-side snapshot request and response directories documented in [Output lifetime and cleanup](features/terminal-snapshots.md#output-lifetime-and-cleanup).

A caller-selected final PNG intentionally persists and can contain secrets. A failed post-create write can leave an incomplete file. The client never overwrites or path-deletes it because a replacement race cannot be cleaned safely. Unix private modes and Windows inherited same-user ACLs reduce casual exposure but are not a boundary against a compromised same-OS-user account. Memory is not locked or zeroized, and file deletion is not forensic secure erasure.

Snapshot audit is metadata-only and fail-soft. It is operational diagnostics, not compliance-grade fail-closed audit. It excludes cell text, JSON, PNG/base64, ANSI, title, credentials, nonce, output path, arbitrary parser or OS errors, and content hashes.

## Container coding agents: copied host credentials

**Status: in progress. On by default** (`containerCredentialsFromHost`). The full feature and its limitations are in [Container coding agents](features/container-coding-agents.md).

When Claude Code runs under the Container runtime, AC copies the executing host user's `~/.claude/.credentials.json` into the replica config dir, which the container reads at `/workspace/.claude`, and deletes it when the session stops. Claude Code is the only provider with a credential descriptor today. AC copies no Codex, Antigravity, or Pi credential or state.

What you accept when you leave this on:

- **The copied file is a full-account credential in plaintext**: a short-lived access token plus a long-lived refresh token, account-scoped. It sits in the project tree, inside a read-write bind mount, for the lifetime of the session.
- **AC pre-answers a safety dialog on your behalf.** It marks the container's `/workspace` as trusted (`hasTrustDialogAccepted`) so the agent does not stall on "do you trust this folder?". AC already bind-mounts that folder read-write, so this grants no access the mount did not already grant, but AC is answering a security prompt for you.
- **Host and containers share one login.** The copy is a snapshot, not a live mount. If the provider rotates the refresh token on use, whichever party refreshes first can invalidate the others: a container refresh can force a re-login on the host.

What AC does to contain it:

- **Your host config is never modified.** AC reads the credential file only. `~/.claude.json` is never read, copied, or written.
- **Teardown deletes the copy** on every session-stop path, including a spawn that fails and a teardown that races the copy itself.
- **AC refuses to write or delete through a symlink or junction**, on the destination directory and on the credential file. The bind mount is read-write, so a container can plant a link to redirect the token off-mount on the next write. AC skips the copy instead, and then stamps no first-run state: a container with no token must show its login wizard rather than pretend to be signed in.
- **On Unix the copy is `0o600`.** A failure to set the mode is logged, not swallowed.

Turn it off in **Settings → General → Container Coding Agents**, or set `containerCredentialsFromHost: false` in `settings.json`. Then AC copies nothing, injects no `CLAUDE_CONFIG_DIR`, stamps nothing, and you supply credentials yourself.

## Code signing

Windows code signing is planned through SignPath Foundation and is pending setup and approval. Current Windows release artifacts may be unsigned until [epic #717](https://github.com/mblua/AgentsCommander/issues/717) is complete. See [`CODE_SIGNING_POLICY.md`](../CODE_SIGNING_POLICY.md).

Verify every exact downloaded filename against the `SHASUMS256.txt` file attached to the same GitHub release. A checksum match does not protect against replacement of both files through a compromised publisher or repository account. On Windows, inspect Authenticode status separately with:

```powershell
Import-Module (Join-Path $PSHOME "Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1") -ErrorAction Stop
Get-AuthenticodeSignature -LiteralPath ".\Agents.Commander_<version>_x64-setup.exe"
```

Linux and macOS builds are not signed today. Build availability does not change the [platform support tiers](install-with-agent.md#support-gates); macOS is not supported yet.

## Known gaps

- **macOS support and code signing** are not yet in place. Use the explicit [tester/contributor path](install-with-agent.md#help-extend-linux-and-macos-support), not a normal install.
- **Windows code signing** is pending SignPath setup and approval ([#717](https://github.com/mblua/AgentsCommander/issues/717)).
- **`--root` is unverified** at the CLI boundary. A malicious local process with shell access can spoof its own root. Mitigated by the daemon-side per-session token check, but not eliminated.
- **No sandbox between agents.** Two agents in the same room share filesystem access. If you need hard isolation, run each agent in its own VM or container.
- **API keys live in plaintext** in `settings.json` under the version-selected configuration directory. Protect your user account and that exact path; if your account is compromised, the keys are.
- **Copied container credentials get no owner-only ACL on Windows** ([#933](https://github.com/mblua/AgentsCommander/issues/933)). The copy inherits the project tree's ACL, which for a user-chosen repo path can be broader than `~/.claude` (shared drives, `Everyone:R`). Unix gets `0o600`.
- **An unclean host crash can leave a copied container credential on disk** ([#933](https://github.com/mblua/AgentsCommander/issues/933)). Teardown deletes it and the next same-agent launch overwrites it, but there is no boot-time sweep, so a replica you never relaunch keeps a live refresh token indefinitely.
- **Snapshot transient cleanup is best-effort after crash and unregistration.** A compatible daemon normally removes dedicated protocol files after use or 60 seconds, but a crash followed by removal of the only active or archived project registration can leave an undiscoverable requester-side file. See [Output lifetime and cleanup](features/terminal-snapshots.md#output-lifetime-and-cleanup).

## Reporting vulnerabilities

See [`SECURITY.md`](../SECURITY.md) at the repo root for the disclosure channel and supported versions.
