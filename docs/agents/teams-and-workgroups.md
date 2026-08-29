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

Room replicas give each team a separate operating space instead of sharing a plain disposable git worktree. A replica includes its own repository copy, agent directories, messaging area, filesystem write boundaries, and room-specific executable. This costs more disk space and setup time than a basic worktree, but it gives AgentsCommander stronger isolation for parallel teams, safer delegation boundaries, and cleaner test or build state per room.

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

AC restores sessions at startup based on the persisted state in each instance's `sessions.json`. If `restore_coordinator_wake_state` is true (Settings → General), orchestrators that were running at shutdown wake up; non-orchestrators stay asleep until you click them.

See [`docs/troubleshooting.md`](../troubleshooting.md) for what to do when a room gets stuck.

## See also

- [Inter-agent messaging](inter-agent-messaging.md) — the file protocol orchestrators use
- [Creating agents](creating-agents.md) — what to build before forming a team
- [CLI reference](../reference/cli.md) — full orchestrator-only verbs
