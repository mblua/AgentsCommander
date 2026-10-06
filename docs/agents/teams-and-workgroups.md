# Teams and rooms

For developers ready to compose multiple agents around a shared goal. Teams define who works together; rooms are where they do the work.

> This concept used to be called a Workgroup. Activating a team now creates `room-<N>-<team>`. Any `wg-*` directory you already have keeps its name and keeps working exactly as before, and the CLI still accepts `workgroup`, `purge-wg`, `--wg` and `--workgroup` as deprecated aliases of `room`, `purge-room` and `--room`. A later release removes the aliases; nothing on disk is ever renamed.

## Team

A **team** is one orchestrator plus one or more worker agents. The orchestrator and every member must already exist as agent matrices before you create the team. The team's config lives at `.ac/_team_<name>/config.json` and lists members by their canonical names.

```
my-project/
└── .ac/
    ├── _agent_tech-lead/
    ├── _agent_dev-rust/
    ├── _agent_dev-ts/
    └── _team_feature-x/
        └── config.json
```

`config.json` (simplified):

```json
{
  "coordinator": "_agent_tech-lead",
  "agents": [
    "_agent_tech-lead",
    "_agent_dev-rust",
    "_agent_dev-ts"
  ],
  "repos": []
}
```

You create teams from the **Teams** UI in the sidebar, or from the CLI with `team create`. Pick an existing orchestrator agent, pick one or more existing member agents, optionally define repo access, then save. Rooms are created later when you activate the team for a task.

### Orchestrator authority

The orchestrator is the only team member that can:

- Send messages to any other team member (members can only message peers in the same team plus their orchestrator).
- Edit the room `TASK.md` through the CLI (`task-set-title`, `task-append-body`).
- Close other members' sessions (`close-session`).
- See the synthetic `agentscommander://root-agent` peer when verified.

This is enforced at the daemon mailbox boundary. Non-orchestrator attempts return an authorization error.

### One agent, many teams

The same agent matrix can belong to multiple teams. Each team that includes the agent gets its own replica when a room activates — replicas are independent working copies, so the agent runs separately in each team's room.

## Room

A **room** is a team's activation for one specific task. AC spins up a new room directory whenever the team is activated:

```
my-project/
└── .ac/
    └── room-1-feature-x/
        ├── TASK.md                       # canonical task file
        ├── messaging/                    # inter-agent messages (see below)
        ├── __agent_tech-lead/            # orchestrator replica
        ├── __agent_dev-rust/             # worker replica
        └── __agent_dev-ts/               # worker replica
```

The integer `<N>` is the lowest free positive number across the project, not per team. Deleted numbers are reused, so multiple teams still share one room number sequence.

### Why replicas?

Room replicas give each team a separate operating space instead of sharing a plain disposable git worktree. A room includes a clone of each assigned [work repo](../glossary.md#work-repo) beside its replicas, and each replica has its own agent directories, messaging area, filesystem write boundaries, and room-specific executable. This costs more disk space and setup time than a basic worktree, but it gives AgentsCommander stronger isolation for parallel teams, safer delegation boundaries, and cleaner test or build state per room.

### Replicas vs the matrix

| | Canonical matrix (`_agent_<name>`) | Room replica (`__agent_<name>`) |
|---|---|---|
| Holds `memory/`, `plans/`, `skills/`, `Role.md` | ✅ Canonical | Read-only mirror |
| Holds session scratch, inbox/outbox | ❌ | ✅ |
| Persists across rooms | ✅ | ❌ (one per room) |
| Edit directly? | ✅ | ❌ — write through the matrix |

Agents running in a replica should treat the canonical matrix as their source of truth and the replica as a session-local working copy.

## The task file (`TASK.md`)

Every room has a `TASK.md` at its root. YAML frontmatter for the title plus a freeform body:

```markdown
---
title: Add OAuth2 login flow
---

We want a working OAuth2 PKCE flow against the new identity service.
- Backend owns `/auth/*` routes.
- Frontend owns the redirect handling.
- Both must land behind feature flag `auth.oauth2`.
```

The orchestrator owns the task file. Workers reference it.

Orchestrators can edit `TASK.md` through the CLI:

```bash
# set/replace the title
agentscommander task-set-title --token "$TOKEN" --root "$ROOT" --title "New title"

# append a paragraph to the body
agentscommander task-append-body --token "$TOKEN" --root "$ROOT" --text "We dropped the legacy /login route."
```

Both verbs validate the caller is an orchestrator of any team in the project and create a timestamped `.bak.md` of the previous `TASK.md` before writing.

Orchestrator title updates do not overwrite titles that begin with `USER:` (a human set those through the in-app title editor). Orchestrator-supplied titles also cannot start with the reserved `USER:` prefix. Use Clean to reset a user-owned task before orchestrator auto-title updates resume.

## Current task status

Keep the human goal in `TASK.md` and publish the current work state separately. `TASK-status.jsonl` is an append-only history within a topic. Each status record is a complete snapshot: remaining tickets, follow-up (FUP), and where to continue. Updating status preserves the brief's title, context, links and constraints. AC does not infer status from the description or prune previous snapshots, and the UI has no history browser or status editor.

For example, keep the title “Resolve tickets” and the original login-failure brief while replacing the current status with “Two tickets remain; FUP: validate the fix; continue P3.” A later update supplies the entire new status, not an extra paragraph.

As the orchestrator, read the current revision before updating:

```bash
"$AGENTSCOMMANDER_BINARY_PATH" task-get \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT"
```

The handler exits `0` and returns a JSON snapshot containing the description, complete status and revision. If it returns `legacy:0` (no status history yet), a first update can be:

```bash
"$AGENTSCOMMANDER_BINARY_PATH" task-status-set \
  --token "$AGENTSCOMMANDER_TOKEN" \
  --root "$AGENTSCOMMANDER_ROOT" \
  --expected-revision "legacy:0" \
  --request-id "d07eedba-6c72-4c6d-8016-3488f0d00dc0" \
  --text "Two tickets remain; FUP: validate the fix; continue P3."
```

Success exits `0` with a JSON receipt containing the new revision and `replayed: false`. Use the exact revision you read and a fresh request UUID for each logical update. If the result is uncertain, retain and retry the same UUID, base revision, exact text and caller; replay works only while that request is the latest record. A `revision_conflict` exits `2`: reread, reconcile, then use the new revision and a new UUID. A `request_id_conflict` means the latest matching UUID has different request values; do not repurpose it. See [CLI flags, limits and errors](../reference/cli.md#task-status-set).

Use your own room's agent root and session credentials. The task verbs use trusted local token-shape/root-master and orchestrator-role checks, not cryptographic live-session binding. Workers can read their room's `TASK.md`; status access goes through the authorized CLI. Do not edit history, backups, the recovery journal or lockfile manually. Legacy `wg-*` rooms use the same task model.

### Reading status in the UI

Hover over or focus the task title in the terminal or sidebar to read the **complete status**. The tooltip uses status, not the description or its first line; the terminal continues to show the description separately. Null status produces no tooltip. Read failures remain errors rather than becoming an invented empty status.

The tooltip stays open while the title has focus or the pointer is over the title or tooltip. A 150 ms delay lets you cross the gap. Long text scrolls within the viewport; from the focused title, use Up/Down, Page Up/Page Down, Home or End. Escape closes it until you re-enter the title or blur and refocus. In the sidebar, a transition to null status also resets Escape dismissal for the next status.

CLI updates appear after the next successful task poll on the existing 15-second cadence; committed GUI changes trigger a refresh event. Errors or disconnection can delay refresh, so 15 seconds is not a guaranteed deadline. Reads reject stale results after the room, request generation or connection changes.

## Clean: start a new topic

Use **Clean** to archive the current description and history together and begin another topic. The paired files share a UTC timestamp and, if needed, the same collision suffix:

```text
TASK.<YYYYMMDD-HHMMSS>[.n].bak.md
TASK-status.<YYYYMMDD-HHMMSS>[.n].bak.jsonl
```

The history backup preserves the entire file, including an unfinished tail. If only one source file exists, its missing partner gets an empty backup; if both are absent, there is no prior pair to archive.

Clean resets the title to `Clean` and the body to `Ready to start a new topic`. It replaces the active history with one `topic_started` record: a new topic UUID, sequence `0`, and null status. The revision becomes that UUID plus `:0`. Repeating Clean on the canonical reset is a no-op when history is empty or contains only the complete `topic_started` seed.

The **sidebar broom** is disabled only when a snapshot is available, its title is empty or trims to `Clean`, status is null, the latest record is not `status`, and its description is canonical. That comparison normalizes CRLF and removes one final LF. A different description or a `status` record keeps the action available even with a `Clean` title; a missing snapshot does not prove the room is already clean. This rule describes the sidebar action, not the terminal button.

### Task history and recovery

`task-get` reads the latest complete row from a bounded history tail. `tailIncomplete: true` identifies an unfinished suffix; it does not certify every older row. An invalid latest complete row fails the read instead of silently falling back.

Before an accepted append repairs an unfinished suffix, AC writes and syncs its exact bytes to `TASK-status.partial.<UTCstamp>.<UUID>.bak`, then truncates the active history to the last complete row (offset zero for a partial-only file). A backup creation, write or sync failure before truncation leaves the original untruncated by that operation. A later failure, including sync after truncation, can leave it already truncated. Keep the synced backup, treat progress as uncertain, and reread/reconcile before continuing; do not assume success or restore/delete files manually.

Clean uses `TASK-clean.pending.json` to finish an interrupted pair automatically on subsequent task reads, writes or GUI access. Recovery syncs both targets, including targets already replaced, before declaring success and deleting the journal. An I/O failure preserves the journal for retry; a conflict preserves evidence and reports an error. Resolve an I/O failure before rereading; report conflicting edits for reconciliation rather than unconditionally restoring backups or deleting the journal.

Task operations share the stable internal `TASK.md.lock`; do not delete it. Cooperating readers get a coherent recovered pair. Direct file readers can observe an intermediate pair: Clean is not an atomic two-file rename or a power-loss guarantee. Concurrent writers from mixed product versions are unsupported.

## Activating a room

From the UI, click **Activate** on the team. From the CLI, use `room add`. AC creates the same disk layout:

1. Creates `.ac/room-<N>-<team>/`.
2. Copies the team config and member references.
3. Provisions each member's replica directory (`__agent_<name>/`, with the same double-underscore prefix for both workers and the orchestrator).
4. Generates `TASK.md`, `messaging/`, and per-replica session artifacts.

When activated from the UI, AC also launches the orchestrator's session. The CLI creates the room and requests a sidebar refresh; launch sessions separately as needed.

```bash
agentscommander team create \
  --project MyProject \
  --team "Feature X" \
  --coordinator tech-lead \
  --agent dev-rust \
  --agent dev-ts

agentscommander room add \
  --project MyProject \
  --team "Feature X" \
  --title "Add OAuth2 login flow"
```

Repository access is a team-level definition. Set it during team creation or editing; `room add` only activates an existing team and uses the repo access already defined on that team.

```bash
agentscommander team create \
  --project MyProject \
  --team "Feature X" \
  --coordinator tech-lead \
  --agent dev-rust \
  --agent dev-ts \
  --repo https://github.com/org/app.git \
  --repo-agents https://github.com/org/admin.git=tech-lead,dev-rust \
  --repo-exclude-agents https://github.com/org/docs.git=dev-ts

agentscommander room add \
  --project MyProject \
  --team "Feature X" \
  --title "Add OAuth2 login flow"
```

Plain `--repo` assigns the repo to the final team roster. `--repo-agents` includes only the named agents for that repo. `--repo-exclude-agents` assigns the repo to the final team roster minus the named agents. The include and exclude forms are mutually exclusive per repo URL.

## Closing a room

Right-click the room → **Close**. All sessions terminate cleanly and the directory stays on disk. Messages, `TASK.md`, and conversations are preserved.

If you want to delete a room entirely, use:

```bash
agentscommander room remove --project MyProject --room room-1-feature-x
```

Removal refuses live sessions. It also refuses dirty repos unless you pass `--force-dirty`, which bypasses only the dirty repo check.

## Editing team membership

You can add a member to an existing room:

```bash
agentscommander team add-member \
  --project MyProject \
  --room room-1-feature-x \
  --agent qa
```

This updates the team config used by `room-1-feature-x` and creates `room-1-feature-x/__agent_qa/` immediately. Use `--coordinator` to make the added agent the orchestrator.

Remove a non-orchestrator member with:

```bash
agentscommander team remove-member \
  --project MyProject \
  --room room-1-feature-x \
  --agent qa
```

Removal refuses live sessions under that member's replica.

Membership edits are scoped to the selected room. Other existing rooms for the same team are not updated globally, so update or recreate those rooms separately when they need the same roster change.

## Recovery

AC restores sessions at startup based on the persisted state in each instance's `sessions.json`; a session whose project root you archived, whose working directory no longer exists, or that AC had not yet reached when you last quit the app, is left out of everything below. If `restore_coordinator_wake_state` is true (Settings → General → On app restart), AC tries to wake orchestrators that were running at shutdown. Non-orchestrators stay asleep until you click them, unless `restartResumeWakeWorkingAgents` is also on, which makes AC try to wake replicas whose last recorded state was working. Whatever comes back awake and was working is then sent its configured restart line, `restartResumeOrchestratorPrompt` for orchestrators and `restartResumeAgentPrompt` for replicas, so the work that was in flight continues without a manual pass over the fleet, unless that line is empty, or you restarted that session or cleared its conversation from the phone, or it never gets back to its prompt in time, or that line cannot be typed into that particular session on that start; in any of those cases it is woken and left alone. See [Settings reference](../reference/settings.md) for these fields and their defaults.

See [`docs/troubleshooting.md`](../troubleshooting.md) for what to do when a room gets stuck.

## See also

- [Inter-agent messaging](inter-agent-messaging.md) — the file protocol orchestrators use
- [Creating agents](creating-agents.md) — what to build before forming a team
- [CLI reference](../reference/cli.md) — full orchestrator-only verbs
