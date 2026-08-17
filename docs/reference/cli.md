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

`list-peers`, `list-peers-lean`, `open-project`, `new-project`, and `telegram-send-image` read disk state directly and do not authorize per token at the CLI. `list-sessions` does not require a token at all. `coding-agent`, `loop`, and `injected-messages` also need no token: they mutate the user-local `settings.json` or config directory, which any local process can already write. `api-client` requires host authority: every subcommand takes the master/root token and rejects session UUIDs. `purge-wg` requires the caller to be the identity-verified workgroup coordinator, and the master/root token does NOT bypass that check (a root token has no workgroup).

`terminal-snapshot` is a privileged exception. The host CLI requires a canonical UUID-v4 live-session token, rejects persisted Root or master credentials, and leaves final authorization to the daemon's live physical-identity checks. `list-peers-lean --snapshot-targets` remains shape-only, identity-only discovery and grants no snapshot authority.

## Exit codes

All subcommands return:

- `0` — success
- `1` — error (auth, IO, routing, validation)
- `2` — special: outcome unknown. Used by `close-session` when delivery succeeded but no response landed in the poll window, by `self-handoff-and-clear` / `self-handoff-and-switch` when the daemon never acknowledged the request, by `raise-hand` when the response is malformed or missing within the timeout, and by `purge-wg` when the response is unparseable.
- `3` — `purge-wg` only: gate rejected (one or more peers are busy)
- `4` — `purge-wg` only: a destroy failed after the gate passed

Exception: `harness` returns `0` for successful `--explain` and `--dry-run`, returns `1` for deny, validation, spawn, or audit-log failures, and propagates the child process exit code when it actually executes a command.

## Discoverability

```bash
agentscommander --help                  # list every subcommand
agentscommander <subcommand> --help     # full args + after-help block for one subcommand
```

The `--help` text is the source of truth; this page is a curated index. Internal test-only verbs (`role-experiment`, `test-reset`, `window-info`, `ui-*`) and test-only top-level flags are hidden from `--help` by design and are not documented here; the test harness invokes them by name.

---

## `agency-templates`

Manage the explicit Agency Agents role-template cache.

```bash
agentscommander agency-templates status --pretty
agentscommander agency-templates update --ref main
agentscommander agency-templates list --pretty
```

`update` resolves the ref to a commit and publishes `<config-dir>/agency-agents_templates` under a single-writer lock. The updater uses `git`, so `git` must be installed and available on `PATH`. `list`, `status`, and [`create-agent-matrix`](#create-agent-matrix) `--role-template agency:<id>` operate offline on the validated cache.

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

Send a file notification, a remote logical PTY action, or privileged exact PTY input.

```bash
# File notification
agentscommander send \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root  "$AGENTSCOMMANDER_ROOT" \
  --to    "<canonical-peer-name>" \
  --send  "<filename>" \
  --mode wake

# Exact text argument
agentscommander send \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --to "<canonical-peer-name>" \
  --pty-input "review the current diff" \
  --mode wake

# Exact stdin, recommended for multiline or sensitive text
printf '%s' "$PROMPT" | agentscommander send \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --to "<canonical-peer-name>" \
  --pty-input-stdin \
  --mode wake
```

| Flag | Required | Description |
|---|---|---|
| `--token` | Yes | Session token. Shape-validated. |
| `--root` | Yes | Sender's root directory (your CWD inside the workgroup or matrix). Used to derive your canonical name. |
| `--to` | Yes | Destination peer's canonical FQN. Get this from `list-peers-lean`. |
| `--send` | * | Filename only, not a path. The file must already exist in `<workgroup-root>/messaging/`. |
| `--command` | * | Logical PTY action: `clear` or `compact`. `clear` resolves to `/new` for an exact-stem direct Pi shell and `/clear` for direct Claude/Codex/Gemini-family or Cursor `agent` shells. Pi compact and outer `cmd`/`pwsh` wrappers are unsupported. The mapped session must be idle. Mutually exclusive with `--send`. |
| `--pty-input` | * | Exact UTF-8 text argument. Hyphen-leading values are accepted. The caller's shell applies quoting and expansion before AC receives the value. |
| `--pty-input-stdin` | * | Read exact UTF-8 bytes from stdin. Recommended for multiline, clipboard, hyphen-leading, process-list-sensitive, or otherwise sensitive text. |
| `--mode` | No | Delivery mode. Default and only supported value: `wake`. |
| `--agent` | No | Configured coding-agent id used only if PTY input must spawn or respawn the target. Default `auto`. It never selects a session, executable, or backend directly. |
| `--confirm-timeout` | No | PTY-input terminal confirmation wait, 0 through 3,600 seconds. Default 90. Timeout does not cancel the operation. |
| `--get-output` | No | Reserved for future modes. Conflicts with PTY input and is non-functional under `--mode wake`. |
| `--timeout` | No | Timeout in seconds for `--get-output`. Default 300. It does not control PTY-input confirmation. |
| `--outbox` | No | Write to a non-default outbox directory. Conflicts with PTY input. |

\* Exactly one of `--send`, `--command`, `--pty-input`, or `--pty-input-stdin` is required.

**Routing** is pre-validated against team membership and coordinator rules before delivery. PTY input uses a narrower identity-verified route described below. Failures exit 1 without writing to the outbox.

Logical values and missing mappings are validated after authorization/routing but before recipient actuation. Unknown values and unsupported mappings are terminal first-poll rejections. A supported action against a busy session remains retriable. Exact-stem matching is lexical trusted configuration, not binary attestation or a runtime version/semantic-success probe. See [Inter-agent messaging](../agents/inter-agent-messaging.md) for the full mapping and trust boundary.

### Privileged exact PTY input

PTY input writes validated text to one already trusted coding-agent PTY. It is not an ordinary message and never passes the accepted value to a host or container shell evaluator, command line, environment variable, or path.

The accepted text is 1 through 65,536 UTF-8 bytes, inclusive. AC preserves accepted bytes exactly, including spaces, LF, TAB, Unicode, quotes, leading hyphens, and shell metacharacters. It rejects CR, NUL, ESC, DEL, other C0 controls except LF and TAB, C1 controls, Unicode line and paragraph separators, and Unicode bidi controls. AC does not trim, normalize, wrap, or append Enter to the text write. After the one exact text write it waits 1,500 ms, writes the required Enter, waits 500 ms, then attempts one redundant Enter.

Only these routes are authorized:

- A live identity-verified workgroup coordinator can target one verified non-coordinator member in the same exact project and workgroup.
- A live local Root Agent can target one verified workgroup coordinator.
- A container coordinator uses the dedicated API helper and a live automatically bound container credential.

Workers, origin coordinators, coordinator-to-coordinator requests, cross-workgroup or cross-project requests, Root-to-worker requests, master credentials without a live session, manual API clients, and filesystem requests from container sessions are rejected before target lifecycle mutation or PTY input. `--to` must be the exact canonical name returned by `list-peers-lean`.

Target lifecycle is deterministic. One idle supported persistent session is selected. A busy or unsupported live session rejects with zero writes. An exited persistent session may be destroyed and respawned with its validated configured profile; a missing target may be spawned once. A newly spawned session must remain continuously ready before submission. No busy bypass, fan-out, broadcast, or deferred text exists.

CLI output has distinct meanings:

- `Operation ID` identifies the stable operation before publication.
- `Queued` means durable admission only. It does not mean bytes were injected.
- `Injected` means the backend accepted the exact text write and required first Enter. It does not prove model consumption or completion.
- `Rejected` means the operation stopped before the no-replay boundary and made zero PTY writes.
- `Indeterminate` means the no-replay boundary committed but complete text-plus-first-Enter submission cannot be proven. Do not submit the text again under a new ID.

A confirmation timeout exits 1 without canceling queued or actuating work. Keep the printed operation ID, do not resubmit under a new ID, and inspect the metadata-only `delivered/`, `rejected/`, and `indeterminate/` artifacts below the verified sender outbox. The raw host request, ignored temporary request, and queued SQLite payload are sensitive until marker conversion, expiry, or actuation; terminal artifacts contain metadata only.

See [Inter-agent messaging](../agents/inter-agent-messaging.md) for the ordinary file protocol and the separate privileged actuation contract.

### `self-handoff-and-clear`

`self-handoff-and-clear` is a token-authorized operation on the caller's own session. Write `SELF-HANDOFF.md` first. Phase 1 waits for 30 seconds of continuous idle, then injects provider-resolved logical-clear text: `/new` for an exact-stem direct Pi shell or `/clear` for direct Claude/Codex/Gemini-family and Cursor `agent` shells. Phase 2 starts only after the full phase-1 injection returns, waits for a fresh 30 seconds of sustained idle, archives the handoff into `self-clear/`, and injects a resume prompt naming that archive.

Both phases are best-effort. A busy transition resets the current sustained-idle window, and a daemon restart or failed phase-1 injection abandons the cycle. Outer `cmd`/`pwsh` wrappers remain unsupported.

### `self-handoff-and-switch`

Two-phase handoff that also switches the caller's OWN session coding agent and/or profile: phase 1 waits for 30 seconds of sustained idle, respawns the session fresh with the requested agent/profile (or the same recipe when both flags are omitted), phase 2 waits a fresh 30 seconds of idle in the new session, archives `SELF-HANDOFF.md` into `self-clear/`, and injects a resume prompt naming that exact archive.

```bash
agentscommander self-handoff-and-switch \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --coding-agent agent_1719526800000_3 \
  --profile C

agentscommander self-handoff-and-switch --list-coding-agents
```

| Flag | Required | Description |
|---|---|---|
| `--token` | Yes | Session token. Shape-validated. |
| `--root` | Yes | Caller's agent root directory. |
| `--coding-agent` | No | Configured coding-agent entry id from `settings.json → agents[]`, not a backend kind or AC peer name. Omit to keep the live session's agent. |
| `--profile` | No | Profile slot letter A through Z. Omit to keep the live session's effective profile. |
| `--list-coding-agents` | No | Print valid coding-agent ids and profile letters, then exit. Requires neither token nor root. |
| `--timeout` | No | Seconds to wait for the daemon's queue acknowledgement. Default 15. |

Before invoking, write `SELF-HANDOFF.md` in your own root with the notes you need to resume; if it is missing the daemon rejects the request (exit 1). If `SELF-FORGET.md` exists, the daemon captures a sanitized compact forgotten summary (max 240 chars), archives it into `self-clear/`, and the later resume prompt may include it only as closed background, never as instructions or work to resume. Scope is WG replicas only (`__agent_*` under a `wg-*` workgroup); Root Agent and origin matrix agents are rejected. Exit codes: `0` queued or already queued, `1` auth/IO/rejection, `2` delivered but no queue acknowledgement within the timeout.

---

## `raise-hand`

Raise the caller session's communication indicator in the Sidebar coordinator row.

```bash
agentscommander raise-hand --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT"
```

| Flag | Required | Description |
|---|---|---|
| `--token` | Yes | Session token. Shape-validated. |
| `--root` | Yes | Caller's agent root directory. |
| `--timeout` | No | Seconds to wait for the daemon's response. Default 15. |

The daemon raises the indicator only when the caller token belongs to a live coordinator session with a visible `TASK.md` title slot. The indicator persists across app restarts until cleared by real user input to the session. On success stdout is exactly `true` or `false`. Exit codes: `0` valid boolean response, `1` auth/IO/delivery failure, `2` malformed response or no response within the timeout.

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

Compact JSON list of peers, ideal for in-session discovery.

```bash
agentscommander list-peers-lean --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT"
```

Each default entry contains: `name`, `working`, `sessionStatus`, `waitingForInput`, `reachable`, `teams`, `roleSummary` (one line, at most 80 characters).

The default peer set and `--peer` filter match `list-peers`. The `name` field is the canonical FQN. Pass it verbatim to `send --to`.

For terminal snapshot target discovery only, add `--snapshot-targets`:

```bash
agentscommander list-peers-lean \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --snapshot-targets
```

This capability view returns every verified workgroup Coordinator and member in active registered projects for canonical Root, or same-workgroup non-Coordinator members for a verified Coordinator. Workers and origin agents receive `[]`. It reads no session index, reports fixed identity-only runtime fields, creates no peer directories, and grants no authority. `reachable` still means ordinary-message reachability. `--peer` applies its existing exact-FQN filter. Full `list-peers` does not accept `--snapshot-targets`.

---

## `terminal-snapshot`

Read one authorized live backend terminal viewport as versioned JSON or a deterministic PNG. The operation is read-only and never wakes, spawns, focuses, selects, resizes, writes to, or captures OS pixels from the target.

```bash
# JSON, the default
agentscommander terminal-snapshot \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --to "project:wg-1-team/member"

# PNG
agentscommander terminal-snapshot \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --to "project:wg-1-team/member" \
  --format png \
  --output "/absolute/new/snapshot.png" \
  --timeout 15
```

| Flag | Required | Description |
|---|---|---|
| `--token` | Yes | Canonical live-session UUID v4. A persisted Root or master token is rejected. |
| `--root` | Yes | Exact verified requester replica root or canonical Root Agent directory, not a descendant. |
| `--to` | Yes | Exact canonical target FQN from `list-peers-lean --snapshot-targets`. |
| `--format` | No | `json` or `png`. Default `json`. |
| `--output` | PNG only | Absolute path to a new `.png` file. JSON forbids it. Existing, linked, unsafe, or non-PNG paths are rejected. |
| `--timeout` | No | Whole seconds from 5 through 60. Default 15. |

JSON success writes exactly one compact ASCII-only `TerminalSnapshotDocument` plus LF to stdout. PNG success fully validates and creates the output first, then writes exactly one compact ASCII-only metadata receipt plus LF. PNG bytes and base64 never appear on stdout. The command never overwrites an existing output. A failed write or final identity check can leave an incomplete caller-owned file.

After Clap parses the command, success exits 0. Every semantic, authorization, unavailable, rate, timeout, transport, or output failure exits 1, writes no normal stdout, and writes one fixed line:

```text
terminal_snapshot_error code=<code> detail=<fixed-detail>
```

Standard `--help` and pre-dispatch Clap syntax failures keep normal Clap output and exit behavior. If an OS failure occurs after a stdout write begins, safe partial ASCII bytes cannot be retracted; the command reports `output_failed` without attempting a second document. See [Terminal snapshots](../features/terminal-snapshots.md#stable-errors) for every stable code, exact detail, and recovery step.

Authorized host routes are canonical live Root to verified workgroup Coordinators or members in active registered projects, and a live workgroup Coordinator to a verified non-Coordinator member in the same exact project and workgroup. Root is host-only. The feature must also be enabled by `terminalSnapshotsEnabled`.

A container Coordinator uses the API helper instead of the host mailbox:

```bash
agentscommander-api-helper terminal-snapshot \
  --to "project:wg-1-team/member"

agentscommander-api-helper terminal-snapshot \
  --to "project:wg-1-team/member" \
  --format png \
  --output "/workspace/evidence/snapshot.png" \
  --timeout 15
```

The helper reads authority only from `AGENTSCOMMANDER_API_URL` and `AGENTSCOMMANDER_API_TOKEN`. It has the same format, timeout, stdout, output, and post-parse error contract. It rejects `--token`, `--root`, duplicate flags, JSON output paths, and PNG without an output path. A manual API token cannot gain this live capability merely by listing the `terminal-snapshot` scope.

See [Terminal snapshots](../features/terminal-snapshots.md) for schema, renderer, fidelity, privacy, authorization, limits, and cleanup.

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

Create a team configuration in a registered project from existing agent matrices. Create the coordinator and member agents first, then create the team, then activate it with `workgroup add`.

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
| `--agent` | No | Existing agent matrix name or `_agent_<name>` reference. Repeat for multiple members. |
| `--repo` | No | Repo URL assigned to the full final roster when workgroups are created. Repeat for multiple repos. |
| `--repo-agents` | No | `URL=agent-a,agent-b`; defines repo access for only the listed team agents when workgroups are created. |
| `--repo-exclude-agents` | No | `URL=agent-a,agent-b`; defines repo access for the final team roster except listed agents when workgroups are created. |

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

`workgroup add` activates an existing team and refuses to update existing team configuration. Create the agents first, define the team with `team create`, then activate the workgroup with project, team, and title.

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

The command writes the team config used by the selected workgroup, creates `wg-.../__agent_<name>/`, applies replica settings, and clones missing assigned repos into that workgroup. Other existing workgroups for the same team are not updated globally; update or recreate them separately when they need the same roster change. Output is JSON.

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

The command refuses to remove the current coordinator and refuses live sessions under the target replica. It also removes the agent from repo assignments in the team config used by the selected workgroup. Other existing workgroups for the same team are not updated globally; update or recreate them separately when they need the same roster change.

---

## `create-agent`

Create a full Agent Matrix (`_agent_<id>/` with a `Role.md`) in a registered AC project; optionally launch it. A near-alias of [`create-agent-matrix`](#create-agent-matrix): identical flags and JSON output, differing only in that `create-agent` trims `--description` and rejects it when empty.

```bash
agentscommander create-agent --project MyProject --name "QA Bot" --description "Runs the integration suite and reports failures."
agentscommander create-agent --project MyProject --name "QA Bot" --description "Runs the integration suite." --role-template agency:dev-rust --launch claude
agentscommander create-agent --project MyProject --name "Pi Reviewer" --description "Reviews the current change." --launch pi
```

| Flag | Required | Description |
|---|---|---|
| `--project` | Yes | Registered AC project folder name from `settings.projectPaths`. Paths are not accepted; the project must contain `.ac`. |
| `--name` | Yes | Display/input name, sanitized into a lower-case `_agent_<id>` folder id (the same backend as the New Agent UI). |
| `--description` | Yes | Written into the `Role.md` frontmatter and body. Trimmed; rejected when empty after trim. |
| `--role-template` | No | Role template id from the New Agent picker source, e.g. `agency:dev-rust` or `local:my-template`. An invalid id fails before any directory is created. |
| `--launch` | No | Coding agent to launch after creation. Matches an `id`, `label`, or command prefix in `settings.json → agents[]`. |
| `--root` | No | Accepted for parity with `create-agent-matrix`; ignored by the handler. |
| `--token` | No | Accepted for parity with `create-agent-matrix`; ignored by the handler. |

Behaviour:

1. Resolves `--project` to a registered project path (paths are rejected).
2. Creates `<project>/.ac/_agent_<id>/` with the matrix layout (`memory/`, `plans/`, `skills/`, `inbox/`, `outbox/`) and writes `Role.md`. A picked role template's body becomes a `## Role Profile` section, and its `skills/` are copied in.
3. Requests a sidebar refresh in the running AC app.
4. If `--launch` is set, writes a session request the running AC app picks up within ~3s.

Output (stdout, JSON): `{ agentPath, agentName, rolePath, launched, launchAgent }`.

See [Creating agents](../agents/creating-agents.md) for richer agent layouts.

---

## `create-agent-matrix`

Create a full Agent Matrix in a registered AC project from a role template; optionally launch it. The sibling of [`create-agent`](#create-agent): same on-disk result, same JSON output, same flags. The only behavioral difference is that `create-agent` trims and rejects an empty `--description`, while `create-agent-matrix` passes `--description` through as given.

```bash
agentscommander create-agent-matrix --project MyProject --name "dev-rust" --description "Implements the Rust backend."
agentscommander create-agent-matrix --project MyProject --name "dev-rust" --description "Implements the Rust backend." --role-template agency:dev-rust --launch pi
```

| Flag | Required | Description |
|---|---|---|
| `--project` | Yes | Registered AC project folder name from `settings.projectPaths`. Paths are not accepted; the project must contain `.ac`. |
| `--name` | Yes | Display/input name, sanitized into a lower-case `_agent_<id>` folder id (the same backend as the New Agent UI). |
| `--description` | Yes | Written into the `Role.md` frontmatter and body. Passed through as given (no trim, no empty check in the handler). |
| `--role-template` | No | Role template id from the New Agent picker source, e.g. `agency:dev-rust` or `local:my-template`. An invalid id fails before any directory is created. |
| `--launch` | No | Coding agent to launch after creation. Matches an `id`, `label`, or command prefix in `settings.json → agents[]`. |
| `--root` | No | Accepted for parity with `create-agent`; ignored by the handler. |
| `--token` | No | Accepted for parity with `create-agent`; ignored by the handler. |

Behaviour is identical to [`create-agent`](#create-agent) above, minus the `--description` trim and empty-check.

Output (stdout, JSON): `{ agentPath, agentName, rolePath, launched, launchAgent }`.

> The CLI verb `create-agent-matrix` is distinct from the in-app New Agent command of the same name. The GUI command shares the same on-disk core but never launches a session and returns only `{ path }`.

---

## `coding-agent`

Scriptable create/inspect/update/remove of Coding Agent configurations (`settings.agents[]`) without the GUI. Agents created here are consumed by [`create-agent`](#create-agent) / [`create-agent-matrix`](#create-agent-matrix) `--launch`. No `--token`: this mutates the user-local `settings.json`, which any local process can already write (same boundary as [`open-project`](#open-project)).

```bash
agentscommander coding-agent list
agentscommander coding-agent show --id claude
agentscommander coding-agent show --id pi
agentscommander coding-agent catalog
agentscommander coding-agent add --from-catalog pi
agentscommander coding-agent add --label "My Claude" --command "claude" --color "#6366f1" --env FOO=bar
agentscommander coding-agent update --id agent_123_abc --command "claude --model opus" --clear-envs
agentscommander coding-agent remove --id agent_123_abc
```

Subcommands:

| Subcommand | Reads/Writes | Output (stdout JSON) |
|---|---|---|
| `list` | reads disk | array of `AgentConfig` |
| `show --id <id>` | reads disk | one `AgentConfig` (exact id) |
| `catalog` | reads disk | array of `CodingAgentDefinition` (read-only catalog) |
| `add` | writes | `{ "ok": true, "op": "add", "agent": { ... } }` |
| `update --id <id>` | writes | `{ "ok": true, "op": "update", "agent": { ... } }` |
| `remove --id <id>` | writes | `{ "ok": true, "op": "remove", "id": "<id>" }` |

`add` / `update` flags:

| Flag | Description |
|---|---|
| `--from-catalog <key>` | (add) Seed label/command/color/envs/isolatedHome (and optional instructions/seed) from a catalog entry; explicit flags below override. Without it, `--label` and `--command` are required. The final label must still be non-empty: a catalog entry with an empty label requires `--label`. |
| `--id <id>` | (add) Custom id, `^[a-z0-9][a-z0-9_-]{0,63}$`. Default: a minted `agent_<ms>_<hex>` id. Ids are unique case-insensitively. |
| `--label <s>` | Display label (non-empty, trimmed). |
| `--command <s>` | Launch command. Banned for AC-managed providers: Claude `--continue`/`-c`, Codex `resume`/`--last`, and Gemini `--resume`. Pi is the intentional exception: canonical Pi commands may contain `-c`, `-r`, `--continue`, `--resume`, `--session`, `--session-id`, `--fork`, or `--no-session`, including long `--name=value` forms. These user-authored controls remain configured and veto AC injection. See [Pi resume behavior](../integrations/coding-agents.md#pi-resume-behavior). |
| `--color <#rrggbb>` | Strict 6-digit hex (only enforced for the explicit flag; catalog-seeded colors are accepted as-is). Default `#6366f1` for custom agents. |
| `--env KEY=VALUE` | Repeatable. Split on the first `=` (`FOO=a=b` -> value `a=b`). All CLI envs are `source=user`, `enabled=true`. On `update`, any `--env` REPLACES the whole env list (including `source=system` rows). |
| `--clear-envs` | (update) Empty the env list. Conflicts with `--env`. |
| `--isolated-home <true\|false>` | Provide an isolated CODEX_HOME at spawn (Codex). |
| `--backend <local\|container>` | Runtime backend. `local` clears any per-agent container image; `container` uses Docker container transport. |
| `--container-image <image>` | Per-agent Docker image override. Implies `--backend container`; conflicts with `--backend local`; rejects empty or leading-dash values. |
| `--clear-container-image` | (update) Clear the per-agent Docker image override. Conflicts with `--container-image`; with `--backend container`, launch succeeds only if the AgentsCommander process has `AGENTSCOMMANDER_CONTAINER_IMAGE` set. There is no built-in image default. |
| `--instructions-filename <name.md>` | Bare `.md` filename AC writes into the agent root at launch. |
| `--clear-instructions-filename` | (update) Clear it. Conflicts with `--instructions-filename`. |
| `--config-seed-dest <folder>` | Config-folder seed destination NAME under the replica root. Implies enabled unless `--config-seed-enabled false`. |
| `--config-seed-enabled <true\|false>` | Toggle the seed. Enabling with no destination is an error. |
| `--clear-config-seed` | (update) Remove the seed. Conflicts with the other seed flags. |
| `--confirm-timeout <secs>` | Seconds to wait for the GUI to process the request (daemon path only). Default 30. |

`remove` leaves any `profilesByAgent[id]` / `profileLabelsByAgent[id]` entries in place (matching the GUI and settings repair), so a same-id re-add resurrects the old profile cells.

**GUI-running routing.** While an AgentsCommander GUI for this binary identity is running (detected via the single-instance mutex), mutations are NOT written to `settings.json` directly. They are queued and applied by the running GUI against its authoritative in-memory state, then a result is returned to the CLI. While the GUI is closed, mutations load `settings.json` strictly (a present-but-unparseable file is refused, not silently defaulted) then apply and save. GUI detection is Windows-only; off-Windows a running GUI is not detected and the direct write path is always used.

**Known limitation (documented for scripts).** The CLI does not clobber the GUI, but the reverse remains possible: a Settings dialog that is already open with an unsaved draft can, on its next Save, revert a concurrent CLI mutation (it writes a full snapshot). Run `--launch` (or re-`show`) right after `add` so consumption happens before a Settings Save can revert.

**Exit codes.** 0 on success, 1 on error. Both terminal (validation) and retryable (GUI busy) failures exit 1; the stderr message carries the distinction. Only a `cancelled (safe to retry)` message is blind-retry-safe. A `may or may not have applied` message is NOT: run `coding-agent show --id <id>` before retrying.

---

## `injected-messages`

Reset operator-editable injected PTY message templates to the defaults this binary ships.

```bash
agentscommander injected-messages reseed --id context-alert
agentscommander injected-messages reseed --all
```

| Flag | Description |
|---|---|
| `--id <id>` | Exact message id to reset (e.g. `context-alert`). Conflicts with `--all`. |
| `--all` | Reset every known message id. |

Edits `<config-dir>/injected-messages.toml` next to the executable; `injected-messages.default.toml` is the canonical reference set. A timestamped `.bak-` copy is written before anything is overwritten; comments, unknown keys, entry order, and untargeted entries are preserved. `--all` applies the same surgical writer to every known id; it is deliberately not a whole-file rewrite.

**No token required** — this touches the user-local config directory next to the executable, the same boundary as `open-project` and `coding-agent`. Exit 0 on success, 1 on error.

---

## `api-client`

Mint, revoke, and list control-plane API client tokens. Every subcommand requires HOST AUTHORITY: the master/root token. A container (which has no master token by construction) cannot self-mint.

```bash
agentscommander api-client mint \
  --token "$MASTER_TOKEN" \
  --root "D:\path\to\wg-8-dev-v5-team\__agent_dev-rust" \
  --scopes send,list-peers-lean \
  --label "CI bot"

agentscommander api-client list --token "$MASTER_TOKEN"
agentscommander api-client revoke --token "$MASTER_TOKEN" --client-id <uuid>
```

| Subcommand | Description |
|---|---|
| `mint` | Create a scoped, revocable client token bound to exactly one replica FQN derived from `--root`. Prints the secret exactly once as JSON on stdout; the registry stores only a SHA-256 hash. The Root Agent is rejected. |
| `revoke` | Revoke a client by `--client-id`. Takes effect on the next API request. |
| `list` | List registered clients. Secrets and hashes are never shown. |

Mint flags: `--token` (master/root token, required), `--root` (replica working directory to bind, the identity source), `--scopes` (comma-separated: `send`, `list-peers-lean`, `session-transport`, `pty-input`, `terminal-snapshot`), `--label` (optional human label), `--expires` (optional RFC3339, e.g. `2026-12-31T00:00:00Z`).

Manually minted privileged scopes (`pty-input`, `terminal-snapshot`) never gain live authority by themselves; both require a matching automatically bound container credential.

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

## `purge-wg`

Destroy every session of every peer in the caller's OWN workgroup. Coordinator-only.

```bash
agentscommander purge-wg \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --wg wg-8-dev-v5-team
```

| Flag | Required | Description |
|---|---|---|
| `--token` | Yes | Session token. The caller must be the identity-verified coordinator of its workgroup; the master/root token does NOT bypass this (a root token has no workgroup). |
| `--root` | Yes | Coordinator's root directory. |
| `--wg` | No | Safety assertion, not a scope selector: fail unless the resolved workgroup has exactly this name. |
| `--graceful` | No | Inject the exit command and wait per session instead of killing immediately. Warning: it stalls ALL inter-agent messaging daemon-wide for the duration of the purge (the message poller is sequential). |
| `--timeout` | No | Graceful shutdown timeout in seconds per session. Default 5. |
| `--dry-run` | No | Evaluate the gate and print the per-peer table. Destroy nothing. |
| `--quiet-period-ms` | No | Printable-silence a peer must show to be purgeable. Clamped daemon-side to a floor of 2500. Default 3000. |

The caller itself and the Root Agent are never purged; cross-workgroup purge is not supported. If ANY in-scope peer has produced printable output within `--quiet-period-ms`, the command purges NOBODY and exits 3.

Exit codes: `0` purged (or dry-run would pass), `1` auth or IO error, `2` outcome unknown, `3` gate rejected (a peer is busy), `4` a destroy failed after the gate passed.

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

On success, stdout is exactly:

```text
Updated
```

If the existing title starts with `USER:` (a human set it through the in-app title editor), the command exits `0`, leaves `TASK.md` unchanged, and stdout is exactly:

```text
Rejected: title set by user
```

If `--title` itself starts with the reserved `USER:` prefix and the current title is not already user-owned, the command exits `1`, writes no stdout, leaves `TASK.md` unchanged, and stderr starts with:

```text
Error: --title cannot start with reserved USER: prefix
```

Use Clean to reset a user-owned task before coordinator title updates resume. Operational audit details are written to the app log, not normal command output.

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
| `PATH` | Absolute or relative path. Relative is resolved against your current working directory. |

Idempotent — re-registering the same path is a no-op (`Project already registered`).

If the folder does not contain `.ac/`, the CLI suggests `new-project` instead.

**Persisted forms.** Relative `PATH` still resolves against your CWD, but the registration records two paths: the canonical absolute path and a portable companion relative to the AC binary's own directory (not your CWD). See [Portable instances](../features/portable-instances.md#portable-project-paths) and the [`projectPaths` schema](settings.md#projects). A project on a different drive or UNC share than the binary records a `null` companion and remains absolute-only.

**Strict settings write.** `open-project` loads `settings.json` strictly before writing: a present-but-unparseable file, or structurally malformed project metadata, is refused with an error and no changes, rather than being silently overwritten.

**No token required** — project registration mutates the local `settings.json`, which any shell-capable process can already write to.

**GUI concurrency caveat**: when AC is running, the in-memory settings are authoritative; a subsequent GUI `update_settings` built from a stale snapshot can clobber a CLI-registered entry. This last-writer race between separate CLI and GUI processes is unchanged; the dual-path work adds no interprocess lock. A watcher/reload story is a follow-up issue.

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

Registration records both the absolute path and the instance-relative companion, and uses the same strict settings-write behavior as [`open-project`](#open-project).

**No token required** — same reasoning as `open-project`.

---

## `loop`

Manage scheduled Project Loops for a registered AC project.

```bash
agentscommander loop list --project MyProject

agentscommander loop create \
  --project MyProject \
  --name "Daily sync" \
  --cron "0 9 * * 1-5" \
  --workgroup wg-1-dev-team \
  --prompt "Check status and ask for blockers."

agentscommander loop update \
  --project MyProject \
  --loop daily-sync \
  --cron "30 9 * * 1-5"

agentscommander loop disable --project MyProject --loop daily-sync
agentscommander loop enable  --project MyProject --loop daily-sync
agentscommander loop remove  --project MyProject --loop daily-sync
```

| Subcommand | Description |
|---|---|
| `list` | Print configured Loops and scheduler state for the project. |
| `create` | Create `_loop_<id>/config.toml` plus initial scheduler state. |
| `update` | Change metadata, cron, target workgroup, prompt, or busy policy. Name-only and no-op updates preserve pending state. |
| `enable` / `disable` | Toggle a Loop. Repeating the current state is a no-op for scheduler state. |
| `remove` | Delete the Loop directory. |

| Flag | Used by | Description |
|---|---|---|
| `--project` | all | Registered project name or project path. |
| `--loop` | update, enable, disable, remove | Existing Loop id. |
| `--id` | create | Optional id. Defaults to a sanitized form of `--name`. |
| `--name` | create, update | Human-readable Loop name. |
| `--cron` | create, update | Five-field cron expression: minute hour day-of-month month day-of-week. |
| `--workgroup` | create, update | Target `wg-<N>-<team>` directory whose coordinator receives the prompt. |
| `--prompt` / `--prompt-file` | create, update | Prompt text or UTF-8 prompt file. Use exactly one when setting a prompt. |
| `--busy-coordinator` | create, update | `wait-until-idle`, `force-inject`, or `skip`. |
| `--force-inject-when-busy` | create, update | Backward-compatible shortcut for `--busy-coordinator force-inject`. |

Output is JSON for list/create/update/enable/disable. `remove` prints a short message unless `AC_MACHINE_OUTPUT` is set, in which case it prints JSON.

**No token required**: Loop commands mutate project files on disk, which any shell-capable process can already write to.

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

## `window-list`

List live native windows as `id<TAB>title` lines so you can discover the canonical decimal `window_id` that `window-screenshot` requires. Windows only; the subcommand does not exist on other targets.

```bash
agentscommander window-list
```

The command takes no flags. Success exits 0 and prints one `id<TAB>title` line per enumerated live window on stdout, in xcap enumeration order (unsorted). A window whose id cannot be read is skipped; a window whose title cannot be read prints an empty title. Titles are printed verbatim, with no sanitization; a title containing a tab or newline breaks the line contract for downstream parsers.

Enumeration failure exits 1 with one stderr line:

```text
window_list_error code=window_list_unavailable detail=<error>
```

**No token required**: this is a local in-process enumeration of the invoking user's own desktop; there is no HTTP request, daemon, token, or audit. See [Window capture](../features/window-capture.md) for the capture flow, the API endpoint, and the shared window-id rule.

---

## `window-screenshot`

Capture exactly one live native window to a PNG file by its canonical decimal `window_id` as printed by `window-list`. Windows only; the subcommand does not exist on other targets.

```bash
agentscommander window-screenshot \
  --window-id 983044 \
  --output "C:\path\shot.png"
```

| Flag | Required | Description |
|---|---|---|
| `--window-id` | Yes | Canonical decimal window id as printed by `window-list`. |
| `--output` | Yes | Destination PNG file path. Overwritten if it exists; parent directories are not created. |

The capture runs fully in-process against the live Windows desktop, reusing the same bounded capture worker as the API route: no HTTP request, no daemon, no token. Success exits 0, writes no stdout, and leaves the output file containing exactly the raw PNG bytes.

Every failure exits 1, writes no normal stdout, and writes exactly one stderr line:

```text
window_screenshot_error code=<code> detail=<detail>
```

| Code | Condition |
|---|---|
| `invalid_window_id` | `--window-id` is empty, non-decimal, signed, whitespace-padded, leading-zero, over 20 digits, or over `u64::MAX`. |
| `window_not_found` | The canonical id matches no live window in the current enumeration snapshot. |
| `capture_busy` | Capture capacity is full (local one-shot limiter; kept for completeness). |
| `capture_too_large` | The window exceeds the advisory pixel limit or the encoded PNG exceeds the hard 16 MiB bound. |
| `capture_unavailable` | Enumeration, minimized/inaccessible window, capture, encode, or runtime failure. |
| `output_write_failed` | The output file could not be written (missing parent, permission, disk full). |

A minimized window yields `capture_unavailable`; that is documented behavior, not a bug. Capture-side failures leave an existing output file untouched; a failed write may leave a partial file and can destroy prior content of an existing `--output`. Standard `--help` and pre-dispatch Clap syntax failures keep normal Clap output and exit behavior.

**No token required**: the verb is a local in-process capture with the invoking process's own privileges; it reads no `--root`, token, registry, or config state and records no API audit. See [Window capture](../features/window-capture.md) for the API endpoint contract, limits, and audit model.

---

## Backwards compatibility

The CLI surface follows AC's project version (`agentscommander --version`). Flags may be added; existing flags will not silently change meaning. Output formats (`list-peers`, `list-sessions`, `create-agent`) are JSON — fields can be added but existing fields stay stable within a major version.

If you discover a regression, file an issue with the exact command, the output, and your version.

## See also

- [Settings reference](settings.md)
- [Log filtering](log-filtering.md)
- [Inter-agent messaging](../agents/inter-agent-messaging.md)
- [Terminal snapshots](../features/terminal-snapshots.md)
- [Window capture](../features/window-capture.md)
- [Teams and workgroups](../agents/teams-and-workgroups.md)
