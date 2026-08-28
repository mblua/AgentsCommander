# Creating agents

For developers ready to build their own agents instead of using only the templates. Three creation paths, four required fields, one rule to follow.

## What an agent is

An agent is a directory with a role-prompt file at its root. The directory IS the agent's identity. AgentsCommander selects the managed role filename for the coding agent it launches:

| Coding agent | Role file |
|---|---|
| Claude Code | `CLAUDE.md` |
| Codex | `AGENTS.md` |
| Antigravity | `AGENTS.md` |
| Pi | `AGENTS.md` |

When the agent dir lives inside an AC project at `.ac/_agent_<name>/`, AC promotes it to an **agent matrix** with optional `memory/`, `plans/`, `skills/`, and a canonical `Role.md`.

## The one rule

**One agent per directory tree.** No nested role files. Coding agents read freely inside their working directory; a second role prompt would leak into the first agent's context, drift its behavior, and waste tokens.

If you need two agents in the same project, give each one its own sibling directory.

## Path 1 — through the UI (recommended)

1. Open the **New Agent** modal from the sidebar (the **New Agent** button in the project header).
2. Type a name (no spaces, slashes, or NUL; lowercase kebab-case recommended).
3. Pick a role template — see [the role-template picker](../integrations/coding-agents.md#role-template-picker).
4. Optionally enable skills if the template ships them.
5. Click **Create**.

AC creates `<project>/.ac/_agent_<name>/` with the matrix layout, writes the chosen role into `Role.md`, copies skills if requested, and registers the agent. The coding-agent role file (`CLAUDE.md` or `AGENTS.md`) is materialized from `Role.md` plus AC context at launch.

## Path 2 — through the CLI

```bash
agentscommander create-agent --project MyProject --name "dev-rust" --description "Implements the Rust backend."
```

`--project` is a registered AC project folder name (paths are not accepted), `--name` is sanitized into a lowercase `_agent_<id>` folder id, and `--description` is written into the `Role.md` frontmatter and body. The command creates `<project>/.ac/_agent_<id>/` with the matrix layout (`memory/`, `plans/`, `skills/`, `inbox/`, `outbox/`) and writes `Role.md`.

Optional flags:

| Flag | Meaning |
|---|---|
| `--role-template <id>` | Seed the role from a template, e.g. `agency:dev-rust` or `local:my-template`. |
| `--launch claude` | Launch a coding agent in the new agent's directory immediately after creation. Matches an `id`, `label`, or command prefix in `settings.json → agents[]`. |
| `--root <PATH>` | Accepted for parity with `create-agent-matrix`; ignored by the handler. |
| `--token <TOKEN>` | Accepted for parity with `create-agent-matrix`; ignored by the handler. |

For a richer role, use the UI path which exposes the role-template picker, or edit `Role.md` afterwards.

Full reference: [`docs/reference/cli.md#create-agent`](../reference/cli.md#create-agent).

## Path 3 — by hand

You can also create an agent by writing files yourself. Minimum viable agent:

```
my-agent/
└── CLAUDE.md
```

`CLAUDE.md` contents:

```markdown
# my-agent

You are a backend developer focused on Rust. ...
```

For a full agent matrix, expand to:

```
_agent_my-agent/
├── Role.md            # canonical role profile (used by replicas)
├── CLAUDE.md          # generated from Role.md
├── memory/
│   └── MEMORY.md
├── plans/
└── skills/
    └── my-skill/
        └── SKILL.md
```

See [Agent skills](agent-skills.md) for the `skills/` layout.

## Required role-prompt fields

A useful role prompt names four things explicitly:

1. **Who the agent is.** One-line identity statement.
2. **What it owns.** Files, modules, services, scope.
3. **How it works with the team.** Who its orchestrator is, who its peers are.
4. **What it never does.** Hard boundaries — "never push to main", "never delete files outside `src/`".

Without #4, agents tend to over-reach. With it, they ask before touching anything ambiguous.

## Agent name (canonical form)

AC derives the agent's name from its filesystem path:

| Path shape | Canonical name |
|---|---|
| `<project>/.ac/_agent_<name>/` | `<project>/<name>` |
| `<project>/.ac/room-<N>-<team>/__agent_<name>/` (replica) | `<project>:room-<N>-<team>/<name>` |

Use this name verbatim with `send --to`. The CLI's `list-peers-lean` always emits the canonical form.

## Updating an agent

To edit a role, edit `Role.md` in the canonical agent matrix at `.ac/_agent_<name>/`. AC regenerates `CLAUDE.md` from it. Never edit `CLAUDE.md` directly on a room replica; the next sync will overwrite your changes.

## See also

- [Teams and rooms](teams-and-workgroups.md) — group agents into teams
- [Inter-agent messaging](inter-agent-messaging.md) — how agents talk
- [Agent skills](agent-skills.md) — reusable per-agent workflows
- [Coding agents](../integrations/coding-agents.md) — picking and configuring the CLI behind each agent
