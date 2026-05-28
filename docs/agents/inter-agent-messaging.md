# Inter-agent messaging

For developers building or operating multi-agent workflows. The full file-based protocol AC uses to route messages between agents, with the exact filenames, paths, and `send` CLI usage.

## Why files

Messages between agents are plain markdown files. Files are inspectable with `cat`, version-controllable with `git`, and arbitrarily large; PTY truncation never applies because the recipient reads the file from disk, not from a PTY injection.

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
# bad: triggers "filename contains path separators or traversal"
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
| Coordinator | Any team member; other coordinators only via the Root Agent. |
| Root Agent | Verified WG coordinator replicas only. |

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

## Conversation files

Beyond `messaging/`, AC also persists a per-peer conversation snapshot at `<config-dir>/conversations/<NNNN>-<from>_<to>.json`. This is a structured copy of the back-and-forth, useful for offline analysis. It is **not** the canonical source; the messaging files are.

## Remote slash commands

`send` can also inject a slash command directly into the recipient's PTY (no file involved):

```bash
agentscommander send --to <peer> --command clear --mode wake
agentscommander send --to <peer> --command compact --mode wake
```

Whitelist: `clear`, `compact`. The recipient must be idle (green dot); the command is rejected otherwise.

This is for housekeeping (clear the screen, compact the context), not for content delivery.

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
- [CLI reference: `send`](../reference/cli.md#send)
- [Security model: inter-agent routing](../security.md#inter-agent-routing)
