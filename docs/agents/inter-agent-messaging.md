# Inter-agent messaging

For developers building or operating multi-agent workflows. The full file-based protocol AC uses to route messages between agents, with the exact filenames, paths, and `send` CLI usage.

## Why files

Messages between agents are plain markdown files. Files are inspectable with `cat`, version-controllable with `git`, and arbitrarily large — PTY truncation never applies because the recipient reads the file from disk, not from a PTY injection.

> Every coordination step is a `git diff` you can audit.

## File layout

For a workgroup `wg-<N>-<team>`:

```
wg-<N>-<team>/
└── messaging/
    ├── 20260527-150000-wg1-tech-lead-to-wg1-dev-rust-kickoff.md
    ├── 20260527-150412-wg1-dev-rust-to-wg1-tech-lead-status.md
    └── 20260527-151205-wg1-tech-lead-to-wg1-dev-ts-design-question.md
```

Filename pattern (the CLI rejects anything else):

```
YYYYMMDD-HHMMSS-wg<N>-<from>-to-wg<N>-<to>-<slug>.md
```

| Field | Rules |
|---|---|
| `YYYYMMDD-HHMMSS` | UTC timestamp at write time. UTC on a non-UTC host will differ from the local wall clock. |
| `wg<N>` | The workgroup number (sender's). |
| `<from>` / `<to>` | Local agent name within the workgroup (e.g. `tech-lead`, `dev-rust`). |
| `<slug>` | Sanitised kebab-case, `[a-z0-9-]+`, ≤50 characters. |
| `.md` | Always markdown. |

Collisions (same second + same slug) get a numeric suffix: `…-status.1.md`, `…-status.2.md`, up to `.99`.

## The two-step protocol

Sending a message is always two steps:

1. **Write the file.** Put your message in a new file under the workgroup's `messaging/` directory using the filename pattern above.
2. **Fire `send`.**

   ```bash
   agentscommander send \
     --token "$AGENTSCOMMANDER_TOKEN" \
     --root "$AGENTSCOMMANDER_ROOT" \
     --to "<canonical-peer-name>" \
     --send "<filename>" \
     --mode wake
   ```

`--send` takes the **filename only**, never a path. The CLI resolves it against `<workgroup-root>/messaging/`:

```bash
# bad — triggers "filename contains path separators or traversal"
agentscommander send ... --send "C:/.../messaging/20260527-150000-wg1-a-to-wg1-b-hi.md"

# good
agentscommander send ... --send "20260527-150000-wg1-a-to-wg1-b-hi.md"
```

The recipient's PTY receives a short notification pointing to the file's absolute path:

```
[file notification] /path/to/wg-1-team/messaging/20260527-150000-wg1-a-to-wg1-b-hi.md
```

The agent reads the file from disk. PTY size limits do not apply.

## Discovering peers

Before sending, resolve the exact peer name via `list-peers-lean`:

```bash
agentscommander list-peers-lean --token "$AGENTSCOMMANDER_TOKEN" --root "$AGENTSCOMMANDER_ROOT"
```

JSON output (one entry per peer):

```json
[
  {
    "name": "my-project:wg-1-feature-x/dev-rust",
    "working": false,
    "sessionStatus": "idle",
    "waitingForInput": true,
    "reachable": true,
    "teams": ["feature-x"],
    "roleSummary": "Rust developer focused on the backend service"
  }
]
```

The `name` field is the canonical FQN. Pass it verbatim to `send --to`.

## Routing rules

The CLI validates routing **before** delivery. If the call would be rejected the CLI fails fast and never writes to the outbox.

| Sender | Allowed recipients |
|---|---|
| Worker (non-coordinator) | The team's coordinator + peers sharing a team. |
| Coordinator | Any team member; any other coordinator directly, with no Root Agent relay; the Root Agent directly, from a verified workgroup coordinator replica. |
| Root Agent | Verified WG coordinator replicas only. |

**Known deviation, tracked in #1041:** "sharing a team" currently ignores the workgroup number, so two replicas of the same team in different workgroups can address each other directly, bypassing both coordinators. This is a defect, not intended behavior, and it contradicts the coordinator-only rule above. #1041 makes the same-team rule workgroup-aware; when it lands, this note is removed. Reaching a *different* team's workgroup is already coordinator-to-coordinator only.

`reachable: false` peers appear in `list-peers-lean` (so you know they exist) but cannot be addressed directly.

## Delivery modes

There is one delivery mode today: `--mode wake`.

- If the recipient has an active session, the notification is written to its stdin. The agent picks it up on the next idle.
- If the recipient's session has exited, AC destroys it and respawns a fresh persistent one.
- If no session exists, AC spawns one with the recipient's saved `lastCodingAgent` (or what you pass via `--agent`).

A first `--mode wake --send` against a cold peer (`sessionStatus: "none"`) only spawns the session. The message is delivered on a second send once the session is up. Verify with `working: true` between sends.

## Waiting for replies

Conversational `--get-output` is reserved for future modes. Today, after `send`, the sender stays idle and waits for the recipient to write a reply file. The reply lands in the same `messaging/` directory and the sender's session receives its own file notification.

Build your turn-by-turn loop around that: send a message, wait for a notification, read the reply, repeat.

## Remote logical PTY actions

`send --command` carries a logical action, not literal slash-command text:

```bash
agentscommander send --to <peer> --command clear --mode wake
agentscommander send --to <peer> --command compact --mode wake
```

| Direct recipient shell | `clear` text | `compact` text |
|---|---|---|
| Claude, Codex, or Gemini filename stem/prefix | `/clear` | `/compact` |
| Cursor exact stem `agent` | `/clear` | `/compact` |
| Pi exact stem `pi` | `/new` | Unsupported |
| Other shells, including outer `cmd` or `pwsh` wrappers | Unsupported | Unsupported |

The mapped session must be idle. A supported action against a busy session remains retriable by the mailbox. An unknown logical value or a known action without a verified shell mapping is a terminal capability rejection on the first poll, before session settling, destroy, spawn, PTY writes, boundary bookkeeping, or follow-up work.

Matching is lexical against the trimmed direct shell's case-folded file stem. A directly configured `pi`, `pi.exe`, or npm `pi.cmd` shim therefore maps clear to `/new`; `pip`, `pi-agent`, and `cmd.exe /C pi` do not. Coding-agent commands are trusted configuration, so an arbitrary executable renamed to an exact `pi` stem also matches. AgentsCommander does not attest the binary or probe its version at runtime. Stock Pi 0.80.10 is the validated control, and production success means the PTY writes were accepted, not that Pi semantically acknowledged `/new`.

These actions are for conversation housekeeping, not content delivery.

## Privileged PTY actuation

Privileged PTY input is separate from file messaging and remote slash commands. It does not create a messaging file, does not use the standard message body or retry state, and does not call the ordinary `/api/v1/send` route. Accepted text goes only to one selected trusted coding-agent PTY. AC never supplies it to a host or container shell evaluator, argv, environment variable, command, or path.

Host form:

```bash
agentscommander send \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --to "<exact-name-from-list-peers-lean>" \
  --pty-input-stdin \
  --mode wake
```

Container coordinator form:

```bash
agentscommander-api-helper send \
  --to "<exact-name-from-list-peers-lean>" \
  --pty-input-stdin \
  --mode wake

agentscommander-api-helper pty-input-status --op-id "<operation-id>"
```

Authorization is narrower than ordinary messaging:

| Sender | Valid PTY-input target | Plane |
|---|---|---|
| Live verified workgroup coordinator replica | One verified non-coordinator member in the same exact project and workgroup | Local host filesystem, or automatically bound container API credential |
| Live canonical local Root Agent | One verified workgroup coordinator replica | Local host filesystem only |
| Worker, origin coordinator, manual API client, stale session, or master credential without a live session | None | None |

Coordinator-to-coordinator, coordinator-to-Root, Root-to-worker, cross-workgroup, cross-project, origin, self, wildcard, alias, filesystem-directory, and session-id targets are invalid. Resolve the exact canonical target with `list-peers-lean` and pass its `name` byte-for-byte.

Text must be valid UTF-8 and 1 through 65,536 bytes. Spaces, LF, TAB, Unicode, leading hyphens, quotes, and shell metacharacters are preserved. Control, bidi, CR, and Unicode line-separator scalars are rejected. Prefer stdin because the caller's shell processes an argument before AC sees it.

The operation holds one per-session writer permit across the exact text write and both Enter attempts, so a user write or another automated writer cannot splice bytes between phases. Target selection never fans out. Busy and unsupported live targets reject with zero writes; exited or missing targets use one verified persistent lifecycle path and wait for sustained readiness.

`Queued` is not `Injected`. A stable operation ID is printed before publication and remains the only safe lookup key after a timeout or ambiguous network result. Do not create a new ID to retry uncertain work. Terminal meanings are:

- `injected`: exact text and the required first Enter were accepted by the backend;
- `rejected`: zero PTY writes occurred before the no-replay boundary;
- `indeterminate`: the no-replay boundary committed, but complete submission cannot be proven and is never replayed automatically.

Host confirmation uses metadata-only artifacts in `outbox/delivered`, `outbox/rejected`, or `outbox/indeterminate`. Container confirmation uses `GET /api/v1/pty-input/{opId}` through `pty-input-status`. Neither terminal surface contains the text, bearer token, raw nonce, path, argv, or environment. The nonterminal host request and SQLite queue remain sensitive while they still retain the exact text.

## Terminal snapshots are a separate read plane

`terminal-snapshot` reads one authorized live backend terminal viewport as JSON or deterministic PNG. It is not a message, remote logical action, privileged PTY write, transcript, or OS screenshot.

A snapshot:

- does not create a workgroup `messaging/*.md` file;
- does not use ordinary outbox delivery, conversations, delivered or rejected message artifacts, the message database, or PTY-input queue state;
- does not wake, spawn, focus, select, resize, repaint, or write to the target;
- does not change ordinary messaging `reachable` semantics or PTY-input authority; and
- works independently of frontend visibility because it reads the backend VT mirror.

Discover the separate capability target set with:

```bash
agentscommander list-peers-lean \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --snapshot-targets
```

This is shape-only identity discovery, not authorization or a session-liveness view. Pass the returned canonical `name` exactly to the host read:

```bash
agentscommander terminal-snapshot \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --to "project:wg-1-team/member"
```

Root can read verified workgroup members and Coordinators in active registered projects through the host plane. A verified workgroup Coordinator can read a non-Coordinator member in the same exact project and workgroup. An automatically bound container Coordinator uses `agentscommander-api-helper terminal-snapshot` and the separate `terminal-snapshot` API scope. Root cannot use the API plane.

The host transport uses only these transient requester-side protocol directories:

```text
<requester-root>/<agent-local-dir>/outbox/terminal-snapshot-requests/
<requester-root>/<agent-local-dir>/terminal-snapshot-responses/
```

The CLI removes a consumed response, and the daemon performs identity-safe 60-second cleanup while files remain discoverable. It never places snapshot content in the canonical workgroup `messaging/` directory. See [Terminal snapshots](../features/terminal-snapshots.md) for the schema, renderer, privacy warning, stable errors, authorization checks, output-file contract, and crash residual.

## Common errors

| Error | Cause | Fix |
|---|---|---|
| `filename '...' contains path separators or traversal` | You passed a path to `--send` | Use the filename only |
| `routing rejected` | Sender cannot reach recipient (membership or coordinator check) | Verify the peer is `reachable: true` in `list-peers-lean` |
| `invalid token` | Token is not a UUID or root/master token | Set `AGENTSCOMMANDER_TOKEN` from your AC session env |
| `--get-output is non-functional under --mode wake` | You set `--get-output` | Remove it; use the reply-file pattern above |

More cases: [`docs/troubleshooting.md#inter-agent-messaging`](../troubleshooting.md#inter-agent-messaging).

## See also

- [Teams and workgroups](teams-and-workgroups.md)
- [CLI reference — `send`](../reference/cli.md#send)
- [Security model — inter-agent routing](../security.md#inter-agent-routing)
- [Terminal snapshots](../features/terminal-snapshots.md)
