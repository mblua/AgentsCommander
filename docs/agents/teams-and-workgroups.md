# Teams and workgroups

For developers ready to compose multiple agents around a shared goal. Teams define who works together; workgroups are the active workspace where they do the work.

## Team

A **team** is one coordinator plus one or more worker agents. The team's config lives at `.ac-new/_team_<name>/config.json` and lists members by their canonical names.

```
my-project/
└── .ac-new/
    ├── _agent_tech-lead/
    ├── _agent_dev-rust/
    ├── _agent_dev-ts/
    └── _team_feature-x/
        └── config.json
```

`config.json` (simplified):

```json
{
  "name": "feature-x",
  "coordinator": "my-project/tech-lead",
  "members": [
    "my-project/dev-rust",
    "my-project/dev-ts"
  ]
}
```

You create teams from the **Teams** UI in the sidebar. Pick a coordinator (must already exist as an agent), pick one or more members, save.

### Coordinator authority

The coordinator is the only team member that can:

- Send messages to any other team member (members can only message peers in the same team plus their coordinator).
- Edit the workgroup `TASK.md` brief through the CLI (`task-set-title`, `task-append-body`).
- Close other members' sessions (`close-session`).
- See the synthetic `agentscommander://root-agent` peer when verified.

This is enforced at the daemon mailbox boundary. Non-coordinator attempts return an authorization error.

### One agent, many teams

The same agent matrix can belong to multiple teams. Each team that includes the agent gets its own replica when a workgroup activates — replicas are independent working copies, so the agent runs separately in each team's workgroup.

## Workgroup

A **workgroup** is a team's active workspace for one specific task. AC spins up a new workgroup directory whenever the team is activated:

```
my-project/
└── .ac-new/
    └── wg-1-feature-x/
        ├── TASK.md                       # the brief
        ├── messaging/                    # inter-agent messages (see below)
        ├── __agent_tech-lead/            # coordinator replica
        ├── __agent_dev-rust/             # worker replica
        └── __agent_dev-ts/               # worker replica
```

The integer `<N>` is sequential per project. Multiple workgroups for the same team can run in parallel (`wg-1-feature-x`, `wg-2-feature-x`, …).

### Replicas vs the matrix

| | Canonical matrix (`_agent_<name>`) | Workgroup replica (`__agent_<name>`) |
|---|---|---|
| Holds `memory/`, `plans/`, `skills/`, `Role.md` | ✅ Canonical | Read-only mirror |
| Holds session scratch, inbox/outbox | ❌ | ✅ |
| Persists across workgroups | ✅ | ❌ (one per workgroup) |
| Edit directly? | ✅ | ❌ — write through the matrix |

Agents running in a replica should treat the canonical matrix as their source of truth and the replica as a session-local working copy.

## The brief (`TASK.md`)

Every workgroup has a `TASK.md` at its root. YAML frontmatter for the title plus a freeform body:

```markdown
---
title: Add OAuth2 login flow
---

We want a working OAuth2 PKCE flow against the new identity service.
- Backend owns `/auth/*` routes.
- Frontend owns the redirect handling.
- Both must land behind feature flag `auth.oauth2`.
```

The coordinator owns the brief. Workers reference it.

Coordinators can edit the brief through the CLI:

```bash
# set/replace the title
agentscommander task-set-title --token "$TOKEN" --root "$ROOT" --title "New title"

# append a paragraph to the body
agentscommander task-append-body --token "$TOKEN" --root "$ROOT" --text "We dropped the legacy /login route."
```

Both verbs validate the caller is a coordinator of any team in the project and create a timestamped `.bak.md` of the previous brief before writing.

## Activating a workgroup

From the UI, click **Activate** on the team. AC:

1. Creates `.ac-new/wg-<N>-<team>/`.
2. Copies the team config and member references.
3. Provisions each member's replica directory (`__agent_<name>/`, with the same double-underscore prefix for both workers and the coordinator).
4. Generates blank `TASK.md`, `messaging/`, and per-replica session artifacts.
5. Launches the coordinator's session, optionally auto-injecting a "set the brief" prompt if `auto_generate_task_title` is on.

You then write the brief and the coordinator takes over.

## Closing a workgroup

Right-click the workgroup → **Close**. All sessions terminate cleanly and the directory stays on disk. Messages, briefs, and conversations are preserved.

If you want to delete a workgroup entirely, do it from your shell — AC will not delete files for you.

## Recovery

AC restores sessions at startup based on the persisted state in each instance's `sessions.json`. If `restore_coordinator_wake_state` is true (Settings → General), coordinators that were running at shutdown wake up; non-coordinators stay asleep until you click them.

See [`docs/troubleshooting.md`](../troubleshooting.md) for what to do when a workgroup gets stuck.

## See also

- [Inter-agent messaging](inter-agent-messaging.md) — the file protocol coordinators use
- [Creating agents](creating-agents.md) — what to build before forming a team
- [CLI reference](../reference/cli.md) — full coordinator-only verbs
