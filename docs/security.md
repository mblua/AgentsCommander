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

The current trust model accepts the caller's self-reported `--root` and per-session UUID — see the in-source notes in `task-set-title` and `task-append-body` for the known weakness and the follow-up issue this points to.

## RTK (Rust Token Killer) injection

When [RTK integration](features/rtk-integration.md) is enabled, AC writes a `PreToolUse` hook into each managed agent directory's `.claude/settings.local.json`. This is a Claude-only Bash-tool rewrite hook; it does not exfiltrate output and does not run untrusted code. AC re-sweeps every managed dir on startup so an obsolete hook is removed cleanly.

You can turn the integration off at any time via **Settings → General → RTK** — the next startup sweep removes every hook AC added.

## Code signing

Windows releases are digitally signed by SignPath. The private key never leaves SignPath's HSM, and every signing request requires manual approval. See [`CODE_SIGNING_POLICY.md`](../CODE_SIGNING_POLICY.md).

Linux and macOS builds are not signed today. Verify Linux assets with the SHA-256 sums attached to the GitHub release.

## Known gaps

- **macOS code signing** is not yet in place ([#320](https://github.com/mblua/AgentsCommander/issues/320)).
- **`--root` is unverified** at the CLI boundary. A malicious local process with shell access can spoof its own root. Mitigated by the daemon-side per-session token check, but not eliminated.
- **No sandbox between agents.** Two agents in the same workgroup share filesystem access. If you need hard isolation, run each agent in its own VM or container.
- **API keys live in plaintext** at `~/.agentscommander/settings.json`. Protect your user account; if your account is compromised, the keys are.

## Reporting vulnerabilities

See [`SECURITY.md`](../SECURITY.md) at the repo root for the disclosure channel and supported versions.
