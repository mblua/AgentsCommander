# Security model

For developers evaluating whether AgentsCommander is safe to put in their workflow. Threat model first, mitigations second, known gaps last.

For privacy and data-flow questions (what leaves the machine and when), see [`PRIVACY.md`](../PRIVACY.md). For how to report a vulnerability, see [`SECURITY.md`](../SECURITY.md).

## Threat model

AgentsCommander is a **local desktop application** that spawns coding-agent CLIs and routes messages between them. The trust boundaries are:

1. **The user.** Operates the GUI, picks coding agents, accepts the actions agents propose. Fully trusted.
2. **The coding agents.** Claude Code, Codex, Gemini. Run in real PTYs as full user-level processes. Trusted with the user's local file system to the same degree the user trusts them when launched directly.
3. **The optional network endpoints.** Telegram Bot API and the Google Gemini API for voice-to-text. Only contacted when the user explicitly enables those features.
4. **The disk.** Configuration, sessions, teams, conversations, and messages all live as plain files under `~/.agentscommander/` (or the portable instance's `.agentscommander_<suffix>/`).

AC does not:

- Send any telemetry. No analytics, no crash reports, no automatic update checks.
- Open inbound network ports unless you explicitly enable the embedded web server (off by default; loopback-only when on).
- Run any agent or shell process without the user starting it.

## What an agent can reach

When you launch a session, the coding agent inherits:

- The session's working directory and everything reachable from it.
- The environment variables of the AC process (including `PATH`, `HOME`, and any keys you may have exported).
- The user-level filesystem and network permissions you yourself have.

This is the same surface area the coding agent has when you launch it from your own terminal. AC adds visibility and coordination — it does **not** add a sandbox. If the underlying agent can `rm -rf ~/`, AC will let it.

If you need stricter isolation, run AC inside a virtualized environment (WSL2, Linux VM, devcontainer) and limit the VM's network and filesystem access there.

## Inter-agent routing

The `send` CLI enforces team-membership and coordinator-only routing at the daemon mailbox boundary. Highlights:

- A worker can only message peers it shares a team with, plus its coordinator.
- A coordinator can message any of its team members.
- Cross-team coordination requires the Root Agent (Project AC Root-level coordinator).

Token validation:

- The CLI validates token **shape** only (UUID, root token, or master token).
- The daemon mailbox validates **identity**: per-session token bound to the live session set.
- A valid UUID from a different binary instance will pass the CLI's shape check but be rejected at the mailbox.

The current ordinary-message and task-operation trust model accepts the caller's self-reported `--root` and per-session UUID. See the in-source notes in `task-set-title` and `task-append-body` for the known weakness and follow-up work. The privileged PTY-input operation does not use that weaker path.

## Privileged exact PTY input

PTY input is a dedicated actuation contract, not an ordinary message or shell-execution feature. Accepted text is validated UTF-8, capped at 65,536 bytes, and written only to one already running trusted coding-agent PTY. It is never supplied to `Command`, shell `-c` or `/C` evaluation, argv, environment variables, or a filesystem path. Shell metacharacters are ordinary text. The sender's own shell may still transform a command-line argument before AC receives it, so stdin is recommended for multiline and sensitive values.

Authority is based on live physical identity:

- A live identity-verified workgroup coordinator may target one verified non-coordinator member in the same exact project and workgroup.
- A live canonical local Root Agent may target one verified workgroup coordinator. Root PTY authority is host-only.
- A container coordinator requires a fresh automatically minted `pty-input` API scope bound to the exact live container session, credential generation, root filesystem object, transport route, and runtime-held credential hash.

Caller `from`, outbox location by itself, broad ordinary `can_communicate` results, a master credential, a manual API scope, cached registry state, role prose, and target spelling are not authority. Workers, origin coordinators, cross-workgroup or cross-project routes, coordinator-to-coordinator routes, Root-to-worker routes, aliases, wildcards, stale sessions, and handcrafted bindings fail before target lifecycle mutation or PTY input.

Security-bearing directories and files are read with bounded no-follow checks. AC rejects symlinks, Windows reparse points, hard-linked mutable files, directory-entry replacement, duplicate JSON keys, hierarchy ambiguity, and object-identity changes. Route entries retain a canonical CWD object identity and, for workgroup replicas, a replica-anchor fingerprint. Authority and target identity are checked at ingress, dispatch start, after long awaits, and again inside the final SessionManager and IdleDetector boundary immediately before the first backend write.

All PTY writers share a per-session permit. One privileged operation retains it across the exact text write, the required Enter, and the redundant Enter, preventing user or automated bytes from splicing between phases. A logical target gate also serializes missing/exited session lifecycle work across host/API operations and ordinary workgroup creates. Fixed OS lock stripes prevent a second daemon from reclaiming a suspended preparing or actuating owner.

The durable no-replay boundary is the transaction that changes the operation to `actuating`. That transaction returns payload bytes only after commit and clears payload, profile, mutable identity, and authority references from the live row. Before it, classified transient failures may retry under a bounded five-attempt policy. After it, no failure automatically replays text. A text-write failure, required-Enter failure, process restart, lost owner, final revalidation failure, or terminal-store failure is conservatively `indeterminate`.

Terminal rows, permanent idempotency tombstones, transition audit, API audit, events, reason files, and host artifacts retain metadata only: ids, verified sender/target, byte length, SHA-256, source plane, selected route, status, canonical timestamps, and fixed reason codes. They do not retain text, token, raw nonce, path, argv, environment, ticket, or arbitrary OS/parser error details. Nonterminal host requests, ignored request temporary files, and SQLite queue rows are sensitive until marker conversion, expiry, rejection, or actuation. SQLite WAL and SSD history are not claimed to provide forensic secure erasure.

`Injected` proves backend acceptance of the exact text and required first Enter only. It does not prove model consumption, understanding, or completion. `Rejected` proves zero writes before actuation. `Indeterminate` means replay is forbidden because actuation may have started. Keep the same operation ID for status lookup and never retry uncertainty under a new ID.

Residual boundaries remain explicit. A local administrator or fully compromised same-OS-user account that can inspect another process's memory or environment is outside this operation's defense. Ordinary messaging defects tracked under #139 do not widen PTY-input authority, but they remain relevant to the separate standard message plane. Model-consumption proof remains outside this feature.

## Container coding agents: copied host credentials

**Status: in progress. On by default** (`containerCredentialsFromHost`). The full feature and its limitations are in [Container coding agents](features/container-coding-agents.md).

When a coding agent runs under the Container runtime, AC copies the executing host user's credential file for that agent (Claude: `~/.claude/.credentials.json`) into the replica config dir, which the container reads at `/workspace/.claude`, and deletes it when the session stops.

What you accept when you leave this on:

- **The copied file is a full-account credential in plaintext**: a short-lived access token plus a long-lived refresh token, account-scoped. It sits in the workspace tree, inside a read-write bind mount, for the lifetime of the session.
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

Verify every downloaded release asset against the `SHASUMS256.txt` file attached to the GitHub release. On Windows, inspect Authenticode status with:

```powershell
Get-AuthenticodeSignature "Agents Commander_X.Y.Z_x64-setup.exe"
```

Linux and macOS builds are not signed today.

## Known gaps

- **macOS code signing** is not yet in place ([#320](https://github.com/mblua/AgentsCommander/issues/320)).
- **Windows code signing** is pending SignPath setup and approval ([#717](https://github.com/mblua/AgentsCommander/issues/717)).
- **`--root` is unverified** at the CLI boundary. A malicious local process with shell access can spoof its own root. Mitigated by the daemon-side per-session token check, but not eliminated.
- **No sandbox between agents.** Two agents in the same workgroup share filesystem access. If you need hard isolation, run each agent in its own VM or container.
- **API keys live in plaintext** at `~/.agentscommander/settings.json`. Protect your user account; if your account is compromised, the keys are.
- **Copied container credentials get no owner-only ACL on Windows** ([#933](https://github.com/mblua/AgentsCommander/issues/933)). The copy inherits the workspace tree's ACL, which for a user-chosen repo path can be broader than `~/.claude` (shared drives, `Everyone:R`). Unix gets `0o600`.
- **An unclean host crash can leave a copied container credential on disk** ([#933](https://github.com/mblua/AgentsCommander/issues/933)). Teardown deletes it and the next same-agent launch overwrites it, but there is no boot-time sweep, so a replica you never relaunch keeps a live refresh token indefinitely.

## Reporting vulnerabilities

See [`SECURITY.md`](../SECURITY.md) at the repo root for the disclosure channel and supported versions.
