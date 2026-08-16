# Container coding agents

For developers running a coding agent under AC's **Container** runtime: what AC does with your host credentials at launch, what works today, and what does not.

> **Status: in progress.** Host login reuse works and is on by default: a container Claude Code session starts signed in with zero interaction. But **a container coding agent cannot reach its repos yet** ([#935](https://github.com/mblua/AgentsCommander/issues/935)). Read [Known limitations](#known-limitations) before you move an agent to the Container runtime.

## Why this exists

Before host login reuse, you authenticated every container coding agent by hand: run `claude setup-token`, paste the output into a `CLAUDE_CODE_OAUTH_TOKEN` env row, repeat per setup. You were already logged in on the host, and the container could not use that login.

Now you log in once on the host and containers reuse it.

## What AC does at launch

When Claude Code runs under the Container runtime and `containerCredentialsFromHost` is on (the default), AC:

1. **Copies the host credential file** for that coding agent into the container's config dir inside the bind mount. Claude Code: host `~/.claude/.credentials.json` → `<replica_root>/.claude/.credentials.json`, which the container reads as `/workspace/.claude/.credentials.json`.
2. **Sets `CLAUDE_CONFIG_DIR`** to that dir, so the CLI reads the copied token. AC injects this only when you have not set `CLAUDE_CONFIG_DIR` yourself. Your own value always wins.
3. **Stamps the container's first-run state** in the container's `.claude.json`, next to the copy: `hasCompletedOnboarding: true`, plus `hasTrustDialogAccepted` and `hasCompletedProjectOnboarding` for the `/workspace` project. Claude Code gates its onboarding wizard on these flags and checks neither against the credential, so without them a valid copied token still lands on "Select login method".
4. **Deletes the copied credential** when the session stops, on every teardown path.

AC never reads or modifies your host config: `~/.claude.json` is not touched. Only the credential file is copied out of `~/.claude/`.

Verified end to end: a cold replica reaches an authenticated Claude session with no human interaction, launching plain `claude`.

If AC finds no host credential, or if the destination directory or file is a symlink or junction, it skips the copy and stamps **nothing**. With no token, the login wizard is the correct behavior.

### AC answers the folder-trust prompt for you

First-run stamping means AC pre-accepts Claude Code's **"do you trust this folder?"** dialog for the container's `/workspace`. That is a safety prompt, and AC answers it on your behalf so the agent does not stall at launch. The folder is the agent's replica root, which AC already bind-mounts read-write. This is deliberate, it is stated in the Settings hint, and it is stated here.

## Terminal snapshots from a container Coordinator

An automatically bound live container Coordinator receives the distinct `terminal-snapshot` API scope in addition to its ordinary bridge scopes. It can read one verified non-Coordinator member in the same exact physical project and workgroup. A worker token never receives the scope, and a manual API client remains unauthorized even if its registry entry lists the string.

Enable the default-off **Settings > General > Terminal snapshots** gate first. The screen can contain credentials, source, prompts, and personal data, and AgentsCommander does not redact it.

Use the helper with the automatically injected environment:

```bash
agentscommander-api-helper terminal-snapshot \
  --to "project:wg-1-team/member"
```

For PNG:

```bash
agentscommander-api-helper terminal-snapshot \
  --to "project:wg-1-team/member" \
  --format png \
  --output "/workspace/evidence/snapshot.png" \
  --timeout 15
```

The helper reads only `AGENTSCOMMANDER_API_URL` and `AGENTSCOMMANDER_API_TOKEN` for authority. Do not pass `--token` or `--root`. It sends one non-idempotent `POST /api/v1/terminal-snapshot`, bypasses ambient proxies, disables redirect and retry behavior, requests identity encoding, and uses one absolute deadline. JSON is the default; PNG requires an absolute new `.png` path inside the container filesystem.

A successful JSON read writes one compact ASCII-only document plus LF. PNG writes a fully validated file and prints metadata only, never bytes or base64. The API caps the request and error body at 8 KiB, success transport at 24 MiB, and decoded JSON or PNG content at 16 MiB. Every route-produced response has `Content-Type: application/json; charset=utf-8`, `Cache-Control: no-store`, and `Pragma: no-cache`.

Root snapshot authority is deliberately host-only. Root must use `agentscommander terminal-snapshot` with a live Root session token and never receives an API identity.

This read does not depend on frontend visibility or access to the target's repository. The separate [repo mount limitation](#1-container-agents-cannot-reach-their-repos-935) still prevents normal container repo work. See [Terminal snapshots](terminal-snapshots.md) for authorization, schema, fidelity, renderer, privacy, errors, and cleanup.

## Known limitations

All of the following are current. Host login reuse does not fix any of them.

### 1. Container agents cannot reach their repos ([#935](https://github.com/mblua/AgentsCommander/issues/935))

**This is the blocker.** The container bind mount exposes **only the agent's replica root**. Your workspace repos (`repo-*`) are siblings of the replica inside the workgroup directory, so they fall **outside the mount**. From inside the container the agent sees its own replica root and `.agentscommander_ac`, and nothing else.

The injected "Workspace Repos" context makes it worse: it hands the agent Windows host paths (`C:\Users\...`) that do not exist inside the container, and promises read/write access the agent does not have.

**A container coding agent cannot do repo work today.** It runs, it authenticates, it reads its own replica, it messages peers. It cannot check out, edit, build, or commit your repos. Use the local-process runtime for repo work until #935 lands.

### 2. One-time "Bypass Permissions mode" consent

With `--dangerously-skip-permissions`, a brand-new replica still shows Claude Code's one-time bypass-mode acceptance screen. A human must accept it **once per replica, ever**. This is Anthropic's consent gate, not an AC bug, and AC deliberately does **not** auto-accept it. Host login reuse removes the login wizard, not this.

### 3. Credential reuse is Claude Code only

Codex, Gemini, and Pi have no credential descriptor. AC copies no credentials and stamps no provider-specific first-run state for them. Their container sessions authenticate with credentials you supply yourself.

For Pi, AC also does not copy or map host `~/.pi/agent/` state, translate `PI_CODING_AGENT_SESSION_DIR`, or provision a `--session-dir` path. Pi 0.80.10 accepts only the separated spelling `--session-dir <dir>`; it rejects `--session-dir=<dir>`. AC preserves the user-authored spelling and path during any eligible `--continue` insertion but does not make the path container-aware. Any custom session directory and its durable state must already be meaningful inside the container.

### 4. Deferred hardening ([#933](https://github.com/mblua/AgentsCommander/issues/933))

| Gap | Detail |
|---|---|
| **No owner-only ACL on Windows** | On Unix AC sets `0o600` on the copied credential. On Windows it sets no ACL, so the copy inherits the project tree's ACL, which for a user-chosen repo path can be broader than `~/.claude` (shared drives, `Everyone:R`). |
| **Crash residue** | Teardown deletes the copy, and the next launch of the same agent overwrites it. There is no boot-time sweep. An unclean host crash (SIGKILL, power loss) leaves the copied credential on disk until that replica's next same-agent launch. A replica you never relaunch keeps a live refresh token on disk indefinitely. |

### 5. No shared team container ([#936](https://github.com/mblua/AgentsCommander/issues/936), paused)

One container per session today, each mounting only its own agent's replica. One shared container for the whole workgroup is a recorded requirement, not a shipped feature, and its feasibility analysis is paused.

## What the copied file actually is

A **full-account credential in plaintext**: a short-lived access token plus a **long-lived refresh token**, account-scoped. It sits in the project tree, inside a read-write bind mount, for the lifetime of the session. See [Security model → Container coding agents](../security.md#container-coding-agents-copied-host-credentials) for the exposure this adds and how AC limits it.

Host and containers share **one login**. The copy is a snapshot, not a live mount. If the provider rotates the refresh token when it is used, whichever party refreshes first can invalidate the others: a container refresh can force a re-login on the host, or break a sibling container. This is the accepted tradeoff of copying in, which avoids a token-refresh write race between the host and every container.

## Turning it off

**Settings → General → Container Coding Agents → "Reuse host login for container coding agents"**, or in `settings.json`:

```json
{
  "containerCredentialsFromHost": false
}
```

With it off, AC copies nothing, injects no `CLAUDE_CONFIG_DIR`, and stamps no first-run state. You supply credentials yourself, for example a `CLAUDE_CODE_OAUTH_TOKEN` env row on the agent: see the [ac-claude image README](../../docker/ac-claude/README.md).

## Troubleshooting

Every step logs under the `[container-cred]` prefix. Set `logLevel` to `info` (the default) and read the log.

| What you see | What it means |
|---|---|
| `[container-cred] no host credential at <path>; container will start without host login` | You are not logged in on the host, or your credential lives elsewhere. Log in on the host, or point `CLAUDE_CONFIG_DIR` (an **absolute** path; a relative value is ignored) at the config dir that holds `.credentials.json`. |
| `[container-cred] copied host credential into <path>` | The copy succeeded. |
| `[container-cred] dest dir <path> is a symlink/reparse point; skipping copy-in` | AC refuses to write a token through a symlink or junction it did not create. Nothing was copied and nothing was stamped; the agent shows its login wizard. Remove the link. |
| `[container-cred] <path> is not valid JSON (...); skipping first-run state` | The container's `.claude.json` is unparseable, so AC left it byte for byte rather than clobber it. The token is in place, but you get the onboarding wizard. |
| The agent is signed in but sees no `repo-*` directory | Expected. This is [#935](https://github.com/mblua/AgentsCommander/issues/935), see [limitation 1](#1-container-agents-cannot-reach-their-repos-935). |

## See also

- [Settings reference → Container coding agents](../reference/settings.md#container-coding-agents): the `containerCredentialsFromHost` field
- [Security model](../security.md#container-coding-agents-copied-host-credentials): threat model for the copied credential
- [Coding agents](../integrations/coding-agents.md): which CLIs AC drives and how it finds them
- [Terminal snapshots](terminal-snapshots.md): the separate authorized backend-viewport read capability
- [ac-claude image README](../../docker/ac-claude/README.md): building the container image and supplying credentials by hand
