# CLI reference

For developers scripting AgentsCommander or invoking it from inside an agent session. Complete reference for every subcommand of the `agentscommander` binary.

The binary doubles as the GUI app (`--app` flag) and as a CLI. When no subcommand is given AC launches the GUI.

## Token model — read this first

`--token <TOKEN>` is required by every verb that touches per-session state. The CLI **shape-validates** the token (UUID, root token, or master token); the daemon mailbox does the authoritative per-session identity check. A valid UUID from a different binary instance passes the CLI's shape check but is rejected by the mailbox at delivery time.

Inside an agent session, pass `AGENTSCOMMANDER_TOKEN` from the environment:

```bash
agentscommander send --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT" ...
```

If token validation keeps failing, restart or respawn the session — live token refresh is not supported.

`list-peers`, `list-peers-lean`, `open-project`, `new-project`, and `telegram-send-image` read disk state directly and do not authorize per token at the CLI. `list-sessions` does not require a token at all.

## Exit codes

All subcommands return:

- `0` — success
- `1` — error (auth, IO, routing, validation)
- `2` — special: outcome unknown (used by `close-session` when delivery succeeded but no response landed in the poll window)

Exception: `harness` returns `0` for successful `--explain` and `--dry-run`, returns `1` for deny, validation, spawn, or audit-log failures, and propagates the child process exit code when it actually executes a command.

## Discoverability

```bash
agentscommander --help                  # list every subcommand
agentscommander <subcommand> --help     # full args + after-help block for one subcommand
```

The `--help` text is the source of truth; this page is a curated index.

---

## `agency-templates`

Manage the explicit Agency Agents role-template cache.

```bash
agentscommander agency-templates status --pretty
agentscommander agency-templates update --ref main
agentscommander agency-templates list --pretty
```

`update` resolves the ref to a commit and publishes `<config-dir>/agency-agents_templates` under a single-writer lock. The updater uses `git`, so `git` must be installed and available on `PATH`. `list`, `status`, and `create-agent-matrix --role-template agency:<id>` operate offline on the validated cache.

Status reasons include `missing`, `locked`, `manifestMissing`, `manifestMalformed`, `invalidCommit`, `templateCountMismatch`, and `cacheInvalid`.

---

## `harness`

Execute a command through the Phase 1 policy harness.

```bash
agentscommander harness -- git status --short
agentscommander harness --dry-run -- git branch risky-name
agentscommander harness --raw-command "echo first && echo second"
agentscommander harness --explain --raw-command "rm -rf /"
```

| Flag | Required | Description |
|---|---|---|
| `--dry-run` | No | Evaluate policy and write the audit log without spawning the command. Exits 0 unless policy denies or logging fails. |
| `--explain` | No | Print the policy decision without spawning the command. Exits 0 unless policy denies or logging fails. |
| `--raw-command` | * | Literal command string executed through the platform shell (`cmd.exe /C` on Windows, `sh -c` on Unix). Policy matching is best-effort. |
| `COMMAND...` | * | Command after `--`. Arguments are passed natively to the child process, preserving boundaries and quotes. |

\* Use either `--raw-command` or a command after `--`.

Audit log entries are JSON Lines at `<config_dir>/logs/harness.log`. The harness redacts token-like values before logging, caps logged command text, and treats `AGENTSCOMMANDER_ROOT` and `AGENTSCOMMANDER_TOKEN` only as unverified audit hints. Phase 1 is an obedient harness and does not prevent direct shell execution by agents.

See [AgentsCommander Harness Roadmap](../harness-roadmap.md) for the phase 1 through 4 roadmap.

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
| `--send` | * | Filename only — no path. The file must already exist in `<workgroup-root>/messaging/`. Mutually exclusive with `--command`. |
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
| `--token` | Yes | Session token. Shape-validated only — this verb reads disk state. |
| `--root` | Yes | Your agent root directory. |
| `--peer` | No | Filter to one or more exact peer FQNs. Repeat the flag for multiple. |

Each entry contains: `name`, `path`, `status`, `working`, `sessionStatus`, `sessionId`, `waitingForInput`, `exitCode`, `role`, `teams`, `reachable`, `lastCodingAgent`.

For automation in scripts, prefer `list-peers-lean` (smaller, stable shape).

---

## `list-peers-lean`

Compact JSON list of peers — ideal for in-session discovery.

```bash
agentscommander list-peers-lean --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT"
```

Each entry contains: `name`, `working`, `sessionStatus`, `waitingForInput`, `reachable`, `teams`, `roleSummary` (one-line, ≤80 chars).

Same peer set and `--peer` filter as `list-peers`. The `name` field is the canonical FQN — pass it verbatim to `send --to`.

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

No token required — reads `sessions.json` from the binary's config directory. Requires the AC app to be running (the file is kept up-to-date while AC is alive).

Each entry contains: `id`, `name`, `workingDirectory`, `status` (`"active" | "running" | "idle" | { "exited": <code> }`), `waitingForInput`, `createdAt` (ISO 8601).

---

## `workgroup list`

List workgroups in a registered project.

```bash
agentscommander workgroup list --project MyProject
```

| Flag | Required | Description |
|---|---|---|
| `--project` | Yes | Registered project name. |

Output is JSON. Each item includes `name`, `team`, `path`, `hasMessaging`, `hasTask`, and `replicas`.

---

## `team create`

Create a team configuration in a registered project.

```bash
agentscommander team create \
  --project MyProject \
  --team "Dev Team" \
  --coordinator architect \
  --agent dev-rust \
  --agent dev-ts

agentscommander workgroup add \
  --project MyProject \
  --team "Dev Team" \
  --title "Add OAuth2 login flow"
```

| Flag | Required | Description |
|---|---|---|
| `--project` | Yes | Registered project name. |
| `--team` | Yes | Team name. Sanitized for `_team_<name>`. |
| `--coordinator` | Yes | Existing agent matrix name or `_agent_<name>` reference. Automatically included in the roster. |
| `--agent` | No | Worker/member agent. Repeat for multiple agents. |
| `--repo` | No | Repo URL assigned to the full final roster. Repeat for multiple repos. |
| `--repo-agents` | No | `URL=agent-a,agent-b`; assigns only listed agents to that repo. |
| `--repo-exclude-agents` | No | `URL=agent-a,agent-b`; assigns the full final roster except listed agents. |

Output is JSON:

```json
{
  "team": "dev-team",
  "path": "C:\\...\\.ac\\_team_dev-team",
  "agents": ["_agent_architect", "_agent_dev-rust", "_agent_dev-ts"],
  "coordinator": "_agent_architect",
  "repos": []
}
```

Repo include and exclude forms are mutually exclusive for the same URL. `team create` refuses to overwrite an existing `_team_<name>` directory, even when it has no `config.json`.

---

## `workgroup add`

Create an auto-numbered workgroup for an existing team.

```bash
agentscommander workgroup add \
  --project MyProject \
  --team "Dev Team" \
  --title "Add OAuth2 login flow"
```

| Flag | Required | Description |
|---|---|---|
| `--project` | Yes | Registered project name. |
| `--team` | Yes | Team name. Sanitized for `_team_<name>` and `wg-<N>-<name>`. |
| `--title` | Yes | Initial `TASK.md` title. |

Workgroup numbers are allocated globally per project as the lowest free positive integer, across all teams. Deleted numbers are reused. There is no `--name` override.

`workgroup add` activates an existing team and refuses to update existing team configuration. Create or change the team with `team create` and `team add-member` first.

Deprecated compatibility flags are still accepted only when the target team is missing: `--coordinator`, `--agent`, `--repo`, `--repo-agents`, and `--repo-exclude-agents`. New scripts should not use them. When the team already exists, passing any of those flags fails before disk writes.

Output is JSON `{ path, cloneErrors }`. Clone failures are reported in `cloneErrors` and do not roll back workgroup creation.

---

## `workgroup remove`

Delete a workgroup directory.

```bash
agentscommander workgroup remove --project MyProject --workgroup wg-1-dev-team
```

| Flag | Required | Description |
|---|---|---|
| `--project` | Yes | Registered project name. |
| `--workgroup` | Yes | Existing `wg-<N>-<team>` directory name. |
| `--force-dirty` | No | Bypass dirty repo checks only. Live session checks still apply. |

Removal refuses to continue when any live session exists under the workgroup. Without `--force-dirty`, it also refuses dirty or unpushed repos under the workgroup.

---

## `team list`

List team configuration in a project, optionally scoped to one workgroup.

```bash
agentscommander team list --project MyProject
agentscommander team list --project MyProject --workgroup wg-1-dev-team
```

| Flag | Required | Description |
|---|---|---|
| `--project` | Yes | Registered project name. |
| `--workgroup` | No | Existing workgroup name. When provided, the team is derived from the workgroup suffix. |

Output is JSON. Each item includes `team`, `workgroup`, `agents`, `coordinator`, and `repos`.

---

## `team add-member`

Add an agent to a team config and create its physical replica in a workgroup.

```bash
agentscommander team add-member \
  --project MyProject \
  --workgroup wg-1-dev-team \
  --agent qa
```

| Flag | Required | Description |
|---|---|---|
| `--project` | Yes | Registered project name. |
| `--workgroup` | Yes | Existing workgroup name. |
| `--agent` | Yes | Existing agent matrix name or `_agent_<name>` reference. |
| `--coordinator` | No | Make the added agent the coordinator. |

The command writes the team config, creates `wg-.../__agent_<name>/`, applies replica settings, and clones missing assigned repos into the workgroup. Output is JSON.

---

## `team remove-member`

Remove a non-coordinator agent from a team config and delete its workgroup replica.

```bash
agentscommander team remove-member \
  --project MyProject \
  --workgroup wg-1-dev-team \
  --agent qa
```

| Flag | Required | Description |
|---|---|---|
| `--project` | Yes | Registered project name. |
| `--workgroup` | Yes | Existing workgroup name. |
| `--agent` | Yes | Existing agent matrix name or `_agent_<name>` reference. |

The command refuses to remove the current coordinator and refuses live sessions under the target replica. It also removes the agent from repo assignments in the team config.

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

Default behaviour: graceful shutdown — AC injects the coding agent's exit command (`/exit` for Claude Code, etc.) and waits up to `--timeout` seconds for clean exit, then force-kills.

| Flag | Required | Description |
|---|---|---|
| `--token` | Yes | Session token. |
| `--root` | Yes | Coordinator's root directory. |
| `--to` | Yes | Target agent name. |
| `--timeout` | No | Seconds to wait for graceful exit before force-kill. |

Exit codes:

- `0` — known status (`closed`, `already_closed`, `no_match`, `restore_in_progress`).
- `1` — auth or IO failure.
- `2` — outcome unknown (delivered, no response in the poll window).

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

The verb writes a timestamped `*.bak.md` of the previous `TASK.md` on every successful write. Concurrent writes are serialized via an advisory lockfile (5s timeout). External edits between read and write abort the verb.

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

Register an existing AC project (folder with `.ac/` inside) in the GUI sidebar's project list.

```bash
agentscommander open-project /path/to/project
```

| Argument | Description |
|---|---|
| `PATH` | Absolute or relative path. Relative is resolved against your CWD; the persisted entry is the absolute form. |

Idempotent — re-registering the same path is a no-op (`Project already registered`).

If the folder does not contain `.ac/`, the CLI suggests `new-project` instead.

**No token required** — project registration mutates the local `settings.json`, which any shell-capable process can already write to.

**GUI concurrency caveat**: when AC is running, the in-memory settings are authoritative; a subsequent GUI `update_settings` built from a stale snapshot can clobber a CLI-registered entry. A watcher/reload story is a follow-up issue.

---

## `new-project`

Create an AC project at PATH (mkdir `.ac/` if no Project AC Root exists) and register it.

```bash
agentscommander new-project /path/to/project
```

| Argument | Description |
|---|---|
| `PATH` | Absolute or relative. Folder created if it does not yet exist. |

Idempotent: re-running on a folder that already has `.ac/` only sweeps the Project AC Root gitignore and deduplicates the registration.

**No token required** — same reasoning as `open-project`.

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

**No token required** — outbound HTTP from a process the user already controls does not cross AC's per-session authorization boundary.

---

## Backwards compatibility

The CLI surface follows AC's project version (`agentscommander --version`). Flags may be added; existing flags will not silently change meaning. Output formats (`list-peers`, `list-sessions`, `create-agent`) are JSON — fields can be added but existing fields stay stable within a major version.

If you discover a regression, file an issue with the exact command, the output, and your version.

## See also

- [Settings reference](settings.md)
- [Log filtering](log-filtering.md)
- [Inter-agent messaging](../agents/inter-agent-messaging.md)
- [Teams and workgroups](../agents/teams-and-workgroups.md)
