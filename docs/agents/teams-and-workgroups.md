# Teams and workgroups

For developers ready to compose multiple agents around a shared goal. Teams define who works together; workgroups are the active workspace where they do the work.

## Team

A **team** is one coordinator plus one or more worker agents. The team's config lives at `.ac/_team_<name>/config.json` and lists members by their canonical names.

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

You create teams from the **Teams** UI in the sidebar, or from the CLI when creating a workgroup. Pick a coordinator (must already exist as an agent), pick one or more members, save.

### Coordinator authority

The coordinator is the only team member that can:

- Send messages to any other team member (members can only message peers in the same team plus their coordinator).
- Edit the workgroup `TASK.md` through the CLI (`task-set-title`, `task-append-body`).
- Close other members' sessions (`close-session`).
- See the synthetic `agentscommander://root-agent` peer when verified.

This is enforced at the daemon mailbox boundary. Non-coordinator attempts return an authorization error.

### One agent, many teams

The same agent matrix can belong to multiple teams. Each team that includes the agent gets its own replica when a workgroup activates — replicas are independent working copies, so the agent runs separately in each team's workgroup.

## Workgroup

A **workgroup** is a team's active workspace for one specific task. AC spins up a new workgroup directory whenever the team is activated:

```
my-project/
└── .ac/
    └── wg-1-feature-x/
        ├── TASK.md                       # canonical task file
        ├── messaging/                    # inter-agent messages (see below)
        ├── __agent_tech-lead/            # coordinator replica
        ├── __agent_dev-rust/             # worker replica
        └── __agent_dev-ts/               # worker replica
```

The integer `<N>` is the lowest free positive number across the project, not per team. Deleted numbers are reused, so multiple teams still share one workgroup number sequence.

### Replicas vs the matrix

| | Canonical matrix (`_agent_<name>`) | Workgroup replica (`__agent_<name>`) |
|---|---|---|
| Holds `memory/`, `plans/`, `skills/`, `Role.md` | ✅ Canonical | Read-only mirror |
| Holds session scratch, inbox/outbox | ❌ | ✅ |
| Persists across workgroups | ✅ | ❌ (one per workgroup) |
| Edit directly? | ✅ | ❌ — write through the matrix |

Agents running in a replica should treat the canonical matrix as their source of truth and the replica as a session-local working copy.

## The task file (`TASK.md`)

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

The coordinator owns the task file. Workers reference it.

Coordinators can edit `TASK.md` through the CLI:

```bash
# set/replace the title
agentscommander task-set-title --token "$TOKEN" --root "$ROOT" --title "New title"

# append a paragraph to the body
agentscommander task-append-body --token "$TOKEN" --root "$ROOT" --text "We dropped the legacy /login route."
```

Both verbs validate the caller is a coordinator of any team in the project and create a timestamped `.bak.md` of the previous `TASK.md` before writing.

## Activating a workgroup

From the UI, click **Activate** on the team. From the CLI, use `workgroup add`. AC creates the same disk layout:

1. Creates `.ac/wg-<N>-<team>/`.
2. Copies the team config and member references.
3. Provisions each member's replica directory (`__agent_<name>/`, with the same double-underscore prefix for both workers and the coordinator).
4. Generates `TASK.md`, `messaging/`, and per-replica session artifacts.

When activated from the UI, AC also launches the coordinator's session. The CLI creates the workgroup and requests a sidebar refresh; launch sessions separately as needed.

```bash
agentscommander workgroup add \
  --project MyProject \
  --team "Feature X" \
  --title "Add OAuth2 login flow" \
  --coordinator tech-lead \
  --agent dev-rust \
  --agent dev-ts
```

Repository access can be assigned at creation:

```bash
agentscommander workgroup add \
  --project MyProject \
  --team "Feature X" \
  --title "Add OAuth2 login flow" \
  --coordinator tech-lead \
  --agent dev-rust \
  --repo https://github.com/org/app.git \
  --repo-agents https://github.com/org/admin.git=tech-lead,dev-rust \
  --repo-exclude-agents https://github.com/org/docs.git=dev-ts
```

Plain `--repo` assigns the repo to the final team roster. `--repo-agents` includes only the named agents for that repo. `--repo-exclude-agents` assigns the repo to the final team roster minus the named agents. The include and exclude forms are mutually exclusive per repo URL.

## Closing a workgroup

Right-click the workgroup → **Close**. All sessions terminate cleanly and the directory stays on disk. Messages, `TASK.md`, and conversations are preserved.

If you want to delete a workgroup entirely, use:

```bash
agentscommander workgroup remove --project MyProject --workgroup wg-1-feature-x
```

Removal refuses live sessions. It also refuses dirty repos unless you pass `--force-dirty`, which bypasses only the dirty repo check.

## Editing team membership

You can add a member to an existing workgroup:

```bash
agentscommander team add-member \
  --project MyProject \
  --workgroup wg-1-feature-x \
  --agent qa
```

This updates the team config and creates `wg-1-feature-x/__agent_qa/` immediately. Use `--coordinator` to make the added agent the coordinator.

Remove a non-coordinator member with:

```bash
agentscommander team remove-member \
  --project MyProject \
  --workgroup wg-1-feature-x \
  --agent qa
```

Removal refuses live sessions under that member's replica.

## Recovery

AC restores sessions at startup based on the persisted state in each instance's `sessions.json`. If `restore_coordinator_wake_state` is true (Settings → General), coordinators that were running at shutdown wake up; non-coordinators stay asleep until you click them.

See [`docs/troubleshooting.md`](../troubleshooting.md) for what to do when a workgroup gets stuck.

## See also

- [Inter-agent messaging](inter-agent-messaging.md) — the file protocol coordinators use
- [Creating agents](creating-agents.md) — what to build before forming a team
- [CLI reference](../reference/cli.md) — full coordinator-only verbs
