# CLI reference

For developers scripting AgentsCommander or invoking it from inside an agent session. Complete reference for every subcommand of the `agentscommander` binary.

The binary doubles as the GUI app (`--app` flag) and as a CLI. When no subcommand is given AC launches the GUI.

## Token model: read this first

`--token <TOKEN>` is required by every verb that touches per-session state. The CLI **shape-validates** the token (UUID, root token, or master token); the daemon mailbox does the authoritative per-session identity check. A valid UUID from a different binary instance passes the CLI's shape check but is rejected by the mailbox at delivery time.

Inside an agent session, pass `AGENTSCOMMANDER_TOKEN` from the environment:

```bash
agentscommander send --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT" ...
```

If token validation keeps failing, restart or respawn the session; live token refresh is not supported.

`list-peers`, `list-peers-lean`, `open-project`, `new-project`, and `telegram-send-image` read disk state directly and do not authorize per token at the CLI. `list-sessions` does not require a token at all.

## Exit codes

All subcommands return:

- `0`: success
- `1`: error (auth, IO, routing, validation)
- `2`: special case, outcome unknown (used by `close-session` when delivery succeeded but no response landed in the poll window)

## Discoverability

```bash
agentscommander --help                  # list every subcommand
agentscommander <subcommand> --help     # full args + after-help block for one subcommand
```

The `--help` text is the source of truth; this page is a curated index.

---

## `send`

Send a message to another agent. File-based (default) or remote slash command.

```bash
agentscommander send \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root  "$AGENTSCOMMANDER_ROOT" \
  --to    "<canonical-peer-name>" \
  --send  "<filename>" \
  --mode wake
```

| Flag | Required | Description |
|---|---|---|
| `--token` | Yes | Session token. Shape-validated. |
| `--root` | Yes | Sender's root directory (your CWD inside the workgroup or matrix). Used to derive your canonical name. |
| `--to` | Yes | Destination peer's canonical FQN. Get this from `list-peers-lean`. |
| `--send` | * | Filename only: no path. The file must already exist in `<workgroup-root>/messaging/`. Mutually exclusive with `--command`. |
| `--command` | * | Remote slash command. Whitelist: `clear`, `compact`. Recipient must be idle. Mutually exclusive with `--send`. |
| `--mode` | No | Delivery mode. Default and only supported value: `wake`. |
| `--agent` | No | Coding agent to use when `wake` spawns a new session for the recipient. Default `auto` (uses recipient's `lastCodingAgent`). |
| `--get-output` | No | Reserved for future modes. Non-functional under `--mode wake`. |
| `--timeout` | No | Timeout in seconds for `--get-output`. Default 300. |
| `--outbox` | No | Write to a non-default outbox directory. |

\* Exactly one of `--send` / `--command` is required.

**Routing** is pre-validated against team membership and coordinator rules before delivery. Failures exit 1 without writing to the outbox.

See [Inter-agent messaging](../agents/inter-agent-messaging.md) for the full protocol.

---

## `list-peers`

List reachable peers (rich JSON output with role, working state, last coding agent).

```bash
agentscommander list-peers --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT"
```

| Flag | Required | Description |
|---|---|---|
| `--token` | Yes | Session token. Shape-validated only; this verb reads disk state. |
| `--root` | Yes | Your agent root directory. |
| `--peer` | No | Filter to one or more exact peer FQNs. Repeat the flag for multiple. |

Each entry contains: `name`, `path`, `status`, `working`, `sessionStatus`, `sessionId`, `waitingForInput`, `exitCode`, `role`, `teams`, `reachable`, `lastCodingAgent`.

For automation in scripts, prefer `list-peers-lean` (smaller, stable shape).

---

## `list-peers-lean`

Compact JSON list of peers: ideal for in-session discovery.

```bash
agentscommander list-peers-lean --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT"
```

Each entry contains: `name`, `working`, `sessionStatus`, `waitingForInput`, `reachable`, `teams`, `roleSummary` (one-line, ≤80 chars).

Same peer set and `--peer` filter as `list-peers`. The `name` field is the canonical FQN; pass it verbatim to `send --to`.

---

## `list-sessions`

List sessions running in the current AC instance.

```bash
agentscommander list-sessions
agentscommander list-sessions --status active
```

| Flag | Description |
|---|---|
| `--status` | Filter by status. One of `active`, `running`, `idle`, `exited`. |

No token required. Reads `sessions.json` from the binary's config directory. Requires the AC app to be running (the file is kept up-to-date while AC is alive).

Each entry contains: `id`, `name`, `workingDirectory`, `status` (`"active" | "running" | "idle" | { "exited": <code> }`), `waitingForInput`, `createdAt` (ISO 8601).

---

## `create-agent`

Create an agent directory with a `CLAUDE.md` role prompt; optionally launch it.

```bash
agentscommander create-agent --parent "C:\path\to\folder" --name " MyAgent "
agentscommander create-agent --parent "C:\path\to\folder" --name " MyAgent " --launch claude
```

| Flag | Required | Description |
|---|---|---|
| `--parent` | Yes | Existing parent directory; the agent folder is created inside it. |
| `--name` | Yes | Agent name (trimmed). Cannot contain `/`, `\`, or NUL. |
| `--launch` | No | Coding agent id to launch (`claude`, `codex`, `gemini`). Must match an entry in `settings.json → agents[]`. |
| `--root` | No | Caller's root directory (logging context). |
| `--token` | No | Session token (auth context). |

Behaviour:

1. Creates `<parent>/<trimmed-name>/`.
2. Writes `CLAUDE.md` with `You are the agent <parentFolder>/<trimmed-name>`.
3. If `--launch` is set, writes a session request that the running AC app picks up within ~3s.

Output (stdout, JSON): `{ agentPath, agentName, claudeMd, launched, launchAgent }`.

See [Creating agents](../agents/creating-agents.md) for richer agent layouts.

---

## `close-session`

Close all sessions for a target agent. Coordinator-only.

```bash
agentscommander close-session \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root  "$AGENTSCOMMANDER_ROOT" \
  --to    "<target-agent-name>"
```

Default behaviour: graceful shutdown. AC injects the coding agent's exit command (`/exit` for Claude Code, etc.) and waits up to `--timeout` seconds for clean exit, then force-kills.

| Flag | Required | Description |
|---|---|---|
| `--token` | Yes | Session token. |
| `--root` | Yes | Coordinator's root directory. |
| `--to` | Yes | Target agent name. |
| `--timeout` | No | Seconds to wait for graceful exit before force-kill. |

Exit codes:

- `0`: known status (`closed`, `already_closed`, `no_match`, `restore_in_progress`).
- `1`: auth or IO failure.
- `2`: outcome unknown (delivered, no response in the poll window).

Only coordinators of the target's team can close. The master/root token bypasses the check.

---

## `task-set-title`

Set the YAML-frontmatter `title:` field of the workgroup `TASK.md`. Coordinator-only.

```bash
agentscommander task-set-title \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root  "$AGENTSCOMMANDER_ROOT" \
  --title "Add OAuth2 login flow"
```

| Flag | Required | Description |
|---|---|---|
| `--token` | Yes | Session token. |
| `--root` | Yes | Coordinator's root directory. |
| `--title` | Yes | New title. Single line. Embedded `\n`, `\r`, NUL, or other control chars (except tab) are rejected. |

The verb writes a timestamped `*.bak.md` of the previous brief on every successful write. Concurrent writes are serialized via an advisory lockfile (5s timeout). External edits between read and write abort the verb.

---

## `task-append-body`

Append a paragraph to the body of the workgroup `TASK.md`. Coordinator-only. Frontmatter is never touched.

```bash
agentscommander task-append-body \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root  "$AGENTSCOMMANDER_ROOT" \
  --text  "We dropped the legacy /login route."
```

| Flag | Required | Description |
|---|---|---|
| `--token` | Yes | Session token. |
| `--root` | Yes | Coordinator's root directory. |
| `--text` | Yes | Body text. Newline, carriage return, and tab are allowed. NUL and other control chars are rejected. |

Same locking + backup behaviour as `task-set-title`.

---

## `open-project`

Register an existing AC project (folder with `.ac-new/` inside) in the GUI sidebar's project list.

```bash
agentscommander open-project /path/to/project
```

| Argument | Description |
|---|---|
| `PATH` | Absolute or relative path. Relative is resolved against your CWD; the persisted entry is the absolute form. |

Idempotent: re-registering the same path is a no-op (`Project already registered`).

If the folder does not contain `.ac-new/` the CLI suggests `new-project` instead.

**No token required:** project registration mutates the local `settings.json`, which any shell-capable process can already write to.

**GUI concurrency caveat**: when AC is running, the in-memory settings are authoritative; a subsequent GUI `update_settings` built from a stale snapshot can clobber a CLI-registered entry. A watcher/reload story is a follow-up issue.

---

## `new-project`

Create an AC project at PATH (mkdir `.ac-new/` if missing) and register it.

```bash
agentscommander new-project /path/to/project
```

| Argument | Description |
|---|---|
| `PATH` | Absolute or relative. Folder created if it does not yet exist. |

Idempotent: re-running on a folder that already has `.ac-new/` only sweeps the gitignore (appending missing patterns) and deduplicates the registration.

**No token required:** same reasoning as `open-project`.

---

## `telegram-send-image`

Send a local file (image or document) to a configured Telegram bot from the terminal, bypassing the GUI.

```bash
agentscommander telegram-send-image \
  --path "C:\path\to\screenshot.png" \
  --caption "Build finished" \
  --bot-label "Personal bot"
```

| Flag | Required | Description |
|---|---|---|
| `--path` | Yes | File to send. Symlinks (and Windows reparse points / junctions) are rejected. |
| `--caption` | No | Caption, trimmed and clamped to Telegram's 1024 UTF-16-code-unit limit. |
| `--bot-id` | * | Pick bot by id from `settings.telegramBots[].id`. |
| `--bot-label` | * | Pick bot by exact label match. |

\* If exactly one bot is configured, both flags may be omitted. With multiple bots and neither flag set, the CLI errors and lists the available bots.

Files ≤10 MB with extensions `jpg/jpeg/png/webp` use `sendPhoto`. Everything else (including GIF) falls back to `sendDocument`, capped at 50 MB.

**No token required:** outbound HTTP from a process the user already controls does not cross AC's per-session authorization boundary.

---

## Backwards compatibility

The CLI surface follows AC's project version (`agentscommander --version`). Flags may be added; existing flags will not silently change meaning. Output formats (`list-peers`, `list-sessions`, `create-agent`) are JSON; fields can be added but existing fields stay stable within a major version.

If you discover a regression, file an issue with the exact command, the output, and your version.

## See also

- [Settings reference](settings.md)
- [Log filtering](log-filtering.md)
- [Inter-agent messaging](../agents/inter-agent-messaging.md)
- [Teams and workgroups](../agents/teams-and-workgroups.md)
