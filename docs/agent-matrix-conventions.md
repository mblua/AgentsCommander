# Role: AC Builder — Creating Agents, Teams & Rooms in AgentsCommander

This document is the definitive guide for any AI agent tasked with creating or modifying the agent/team/room structure in an AgentsCommander project. It captures the conventions, file formats, and pitfalls learned from building real multi-agent teams.

---

## Core Concepts

| Concept | Prefix | Location | Purpose |
|---|---|---|---|
| **Agent** | `_agent_` | `.ac/_agent_NAME/` | A role definition: who this agent is, what it does, what it must never do |
| **Team** | `_team_` | `.ac/_team_NAME/` | A grouping of agents that can message each other via `list-peers` / `send` |
| **Room** | `room-` (legacy: `wg-`) | `.ac/room-N-TEAMNAME/` | An isolated working environment with cloned agents + cloned repo for parallel work |
| **Room Agent** | `__agent_` | `.ac/room-N-TEAMNAME/__agent_NAME/` | A replica of a project-level agent inside a room (double underscore) |

`.ac/` is the only supported Project AC Root directory.

The team is the logical capability and organization: it defines who can work together, who coordinates, and which repos are available. The room is an operational runtime replica instance of a team for a specific task: it contains replica agents and `repo-*` working repositories.

**Hierarchy:** Project → Agents + Teams → Rooms (with replicated agents + repo clones)

---

## Project Context Templates

Project `.ac` creation seeds the editable global and orchestrator context templates under:

```text
.ac/
├── Context.AgentsCommander.md
└── Context.coordinator.md
```

`.ac/Context.AgentsCommander.md` is **project-scoped**, and it is the base context used when AgentsCommander materializes managed context files such as `CLAUDE.md` and `AGENTS.md` for **Agent Matrix agents and room replicas only**. The `$AGENTSCOMMANDER_CONTEXT` token that resolves to it is likewise valid for matrix and room context only. `.ac/Context.coordinator.md` is appended only for orchestrator sessions. The separator and `# Orchestrator Context` heading are owned by AgentsCommander, so the orchestrator file should contain only the body text.

### The Root Agent does not use the global template (#979)

The canonical Root Agent (`ac-root-agent`) never reads `.ac/Context.AgentsCommander.md`, never resolves `$AGENTSCOMMANDER_CONTEXT`, and never falls back to the built-in global template. It receives an **unconditional, code-owned runtime prologue** instead, assembled directly in the backend and always placed first:

```text
Root Agent context
├── code-owned runtime prologue   (heading, Core Concepts, write scope and Root
│                                  authority, delegated-task reporting, skills,
│                                  workspace repos, CLI rules, credentials,
│                                  Root messaging)
├── ../Context.root-agent.md      (raw supplemental Root prose)
└── Role.md                       (raw)
```

Because the prologue is assembled from code rather than rendered from a template, **emptying, editing, or deleting any Root file cannot suppress mandatory Root governance**. There is no editable Root runtime template and no placeholder whose removal can erase a block.

`.ac/Context.root-agent.md` is appended only for the canonical `ac-root-agent` session. It is static supplemental root prose for identity, durable root state, and high-level coordination scope. It does not receive placeholder rendering: operational write restrictions, credentials, CLI usage, inter-agent messaging, workspace repo context, and skills all come from the code-owned prologue above.

Root provisioning seeds **only** Root-specific material (`Context.root-agent.md`, `Role.md`, Root skills, the Root messaging directory, and `config.json`). It never creates `Context.AgentsCommander.md` or `Context.coordinator.md`.

A `Context.AgentsCommander.md` left in the app config directory by an older build is **retired conservatively and best-effort** on the next Root provisioning: bytes AgentsCommander provably generated itself are deleted, and every other byte sequence, including custom edits and non-UTF-8 content, is moved to an inert timestamped `Context.AgentsCommander.md.retired-<timestamp>.bak` backup and preserved. Retirement never blocks the Root Agent from starting: a failure is logged as a warning and the file is left where it is, inert. Project globals and project template state are never touched by it.

Existing projects with a legacy `.ac/Context.agent.md` are migrated on demand to `.ac/Context.AgentsCommander.md` when the new file does not already exist. Existing projects that do not have these files keep using the built-in defaults. If the global template file exists and is empty, AgentsCommander still injects the mandatory safety and runtime blocks listed below. If a template exists but cannot be read, is not UTF-8, or is not a regular file, session context generation fails with a path-specific error instead of silently discarding the customization.

The global template must preserve these mandatory runtime tokens for matrix agents and room replicas. If any are missing, AgentsCommander appends them during rendering so critical safety and runtime data is not dropped. (This append fallback governs matrix and room rendering only; the Root Agent's blocks come from the code-owned prologue described above and cannot be dropped at all.)

| Token | Meaning |
|---|---|
| `{{WRITE_RESTRICTIONS}}` | Runtime write restrictions and allowed scopes |
| `{{DELEGATED_TASK_REPORTING}}` | Required completion/blocker reporting instructions |
| `{{SKILLS_SECTION}}` | Runtime skill index |
| `{{AGENT_REPOS}}` | Repo list rendered from the replica config `repos` field |
| `{{CLI_CONTEXT}}` | CLI binary and help usage rules |
| `{{SESSION_CREDENTIALS}}` | Environment-variable credential rules |
| `{{INTER_AGENT_MESSAGING}}` | Peer discovery and file-based messaging instructions |

Legacy custom templates also support these older runtime tokens:

| Token | Meaning |
|---|---|
| `{{AGENT_ROOT}}` | Current agent root path |
| `{{MATRIX_SECTION}}` | Agent Matrix write-scope section, when applicable |
| `{{MATRIX_ALLOWED}}` | Agent Matrix allowed-write bullet, when applicable |
| `{{MESSAGING_EXCEPTION}}` | Narrow messaging directory exception, when applicable |
| `{{MESSAGING_ALLOWED}}` | Narrow messaging allowed-write bullet, when applicable |
| `{{FORBIDDEN_SCOPE}}` | Runtime-specific forbidden write scope |
| `{{GIT_SCOPE}}` | Runtime-specific git operation clarification |
| `{{PEER_NAME_FORMAT}}` | Peer-name format for the current session type |
| `{{SEND_MESSAGE_INSTRUCTIONS}}` | File-based send instructions for the current session type |
| `{{SKILLS_SECTION}}` | Runtime skill index and warnings |
| `{{WORKSPACE_REPOS}}` | Pre-#1369 spelling of `{{AGENT_REPOS}}`; still rendered in place, with no duplicate block |

Removing a token is allowed if you intentionally do not want that dynamic section included in generated contexts.

---

## 1. Creating a Project-Level Agent (`_agent_*`)

Project-level agents appear in the **AGENTS** section of the AgentsCommander sidebar. They are the canonical definitions — room agents are replicas of these.

### Folder Structure

```text
.ac/_agent_NAME/
├── Role.md          # REQUIRED — the agent's identity, responsibilities, and rules
├── inbox/           # Created by AC on first use — incoming messages
├── outbox/          # Created by AC on first use — outgoing messages
├── memory/          # Created by AC on first use — persistent agent memory
├── plans/           # Created by AC on first use — plan files
├── skills/          # Created by AC on first use — reusable workflows
└── .<stem>/         # Created by AC — per-instance internal runtime state
    └── config.json  # Runtime config (tooling, session tracking)
```

**Minimum to create an agent:** A folder named `_agent_NAME/` containing a `Role.md` file. AgentsCommander creates the remaining directories (`inbox/`, `outbox/`, `memory/`, `plans/`, `skills/`, `.<stem>/`) automatically when the agent is first launched or used. This replica-local `.<stem>` directory is always named after the running binary (for example `.agentscommander/`); its naming is independent of the version-specific application config resolver.

### Role.md Format

```markdown
---
name: 'agent-name'
description: 'One-line description of what this agent does'
type: agent
---

# Role: Display Name — Project Context

## Source of Truth

This role is defined in Role.md of your Agent Matrix at: .ac/_agent_NAME/
If you are running as a replica, this file was generated from that source.
Always use memory/ and plans/ from your Agent Matrix, and treat Role.md there as the canonical role definition. Never use external memory systems.

## Agent Memory Rule

If you are running as a replica, the single source of truth for persistent knowledge is
your Agent Matrix's memory/, plans/, and Role.md. Use your replica folder only for
replica-local scratch, inbox/outbox, and session artifacts. NEVER use external memory
systems from the coding agent (e.g., ~/.claude/projects/memory/).

---

## Core Responsibility

[One paragraph: what this agent DOES and what it DOES NOT do]

---

## Project Context

[Project-specific knowledge the agent needs to do its job]

---

## [Domain-Specific Sections]

[Architecture, workflow, standards — whatever this agent needs to know]

---

## What You Must NEVER Do

[Hard rules — the guardrails that prevent disasters]
```

### Role.md Anatomy — What Makes a Good Role

A Role.md is NOT a job description. It's an **operational manual** that an AI agent reads cold and must be able to act on immediately. Every section must pass the test: "Could an agent who has never seen this project before do the right thing after reading this?"

**Required sections:**

| Section | Purpose | Bad Example | Good Example |
|---|---|---|---|
| Core Responsibility | What you do and DON'T do | "Help with the project" | "Design XPath modification plans. You are a planner, not an implementer — you never write XML yourself." |
| Project Context | Domain knowledge | "It's a game mod" | Repo URL, mod scope, what systems exist, what the goal is |
| Domain Knowledge | Technical reference | "Use XML" | XPath syntax with examples, file structure, game systems table |
| Workflow | Where you fit in the pipeline | "Work with the team" | "Step 2: You receive a requirement from the tech-lead. You produce a plan in `_plans/`. The dev implements it." |
| What You Must NEVER Do | Hard guardrails | Generic prohibitions | "Never commit to main. Never merge. Never instruct other agents to push to origin." |

**Principles for writing roles:**

1. **Be the domain expert, not the agent.** Write the Role.md as if you're a senior engineer briefing a new hire. Include the knowledge they need, not instructions on how to be an AI.

2. **Concrete over abstract.** Don't say "follow best practices." Say "Every XML file must have `<configs>` as its root element. XPath expressions must use `[@name='...']` selectors, never positional `[N]` selectors."

3. **Include examples.** If the agent writes XPath, show XPath. If it writes Rust, show Rust patterns. If it reviews code, show what a review finding looks like.

4. **Scope the agent tightly.** An agent that "helps with everything" helps with nothing. The best agents have a clear boundary: "I design, I don't implement." "I review, I don't fix." "I package, I don't modify."

5. **State the negative space.** "What You Must NEVER Do" is as important as the responsibilities. Without explicit guardrails, agents drift into doing things they shouldn't (merging to main, modifying files outside their scope, skipping review steps).

6. **Include the WHY.** Don't just say "never push to origin." Say "never push to origin — the merge/push decision belongs to the user, not to agents." The WHY helps the agent make judgment calls in edge cases.

---

## 2. Creating a Team (`_team_*`)

Teams define which agents can communicate with each other via `list-peers` and `send`. An agent that isn't part of a team will see an empty peers list.

### Folder Structure

```text
.ac/_team_NAME/
├── config.json      # REQUIRED — defines members, coordinator, and repos
├── conventions.md   # Optional — shared conventions across the team
└── memory/          # Optional — shared team memory
```

### config.json Format

```json
{
  "agents": [
    "_agent_one",
    "_agent_two",
    "_agent_three"
  ],
  "coordinator": "_agent_one",
  "repos": [
    {
      "agents": [
        "_agent_one",
        "_agent_two",
        "_agent_three"
      ],
      "url": "https://github.com/owner/repo.git"
    }
  ]
}
```

**Fields:**

| Field | Required | Description |
|---|---|---|
| `agents` | Yes | Array of portable agent references (`_agent_<name>` or bare `<name>`). These agents become peers and can message each other. |
| `coordinator` | Yes | The agent reference of the agent that coordinates work. Shown with `orchestrator` badge in sidebar. Must also be in `agents`. |
| `repos` | Yes | Array of repo objects. Each has `agents` (which agents work on this repo) and `url` (the git remote). |

### Critical Rules for Team Config

1. **Use portable agent references.** The `agents` array and `coordinator` must be `_agent_<name>` or bare `<name>` references to matrices inside the SAME project's `.ac/` directory. **Absolute filesystem paths are rejected** (`team configs must use portable refs like '_agent_name' or 'name', not filesystem paths`), so team configs stay portable across machines and room copies.

2. **Agents must exist.** Every reference in the `agents` array must resolve to an existing `_agent_<name>` matrix folder with a `Role.md`. If the folder doesn't exist, the agent won't appear.

3. **Don't reference external projects.** If you're building a team for project A, the agents must be `_agent_*` matrices inside project A's `.ac/`. Referencing agents from project B (for example `_agent_foo` from another project's root) makes them appear as belonging to the wrong project.

4. **The orchestrator must be in the agents list.** The orchestrator path must also appear in the `agents` array.

5. **Repos.agents can be a subset.** Not every team member needs access to every repo. The `repos[].agents` array specifies which agents work on which repo.

---

## 3. Room Structure (`room-*`)

Rooms are isolated working environments created when a team needs to work on a task in parallel. They contain **replicas** of agents (double underscore `__agent_*`) and **clones** of repositories (`repo-*`).

### Folder Structure

```text
.ac/room-N-TEAMNAME/
├── TASK.md                    # Objective, scope, and deliverables for this room
├── __agent_NAME/               # Replica of _agent_NAME (double underscore)
│   ├── config.json             # Points to parent agent's identity + local repo
│   ├── Role.md                 # Optional override — if absent, uses parent's Role.md via config
│   ├── inbox/
│   ├── outbox/
│   └── .<stem>/         # Per-instance dir named after the binary
│       └── config.json
├── __agent_OTHER/
│   └── ...
└── repo-REPONAME/              # Shallow clone of the team's repo
    ├── .git/
    ├── _plans/                 # Plans created during this room's work
    └── (repository contents)
```

### Room Agent config.json

```json
{
  "context": [
    "$AGENTSCOMMANDER_CONTEXT",
    "../../_agent_NAME/Role.md"
  ],
  "identity": "../../_agent_NAME",
  "repos": [
    "../repo-REPONAME"
  ]
}
```

| Field | Description |
|---|---|
| `context` | Array of context sources. `$AGENTSCOMMANDER_CONTEXT` is the AC-injected global template, and it is valid **only for Agent Matrix and room context**; the Root Agent ignores it (see #979 above) and any occurrence is stripped from the Root `config.json` during provisioning. The Role.md entry defines this agent's personality. `$REPOS_WORKSPACE_INFO` is deprecated; repo context is rendered through `{{AGENT_REPOS}}` inside `$AGENTSCOMMANDER_CONTEXT`. |
| `identity` | Path to the parent agent folder. This is the canonical identity — the room agent is a replica of this. |
| `repos` | Relative paths to the repo clones inside this room. |

### Key Conventions

1. **Naming:** `room-N-TEAMNAME` where N is sequential (1, 2, 3...) and TEAMNAME matches the team.
2. **Double underscore:** Room agents use `__agent_` (two underscores) to distinguish from project-level `_agent_` (one underscore).
3. **Repo prefix:** Cloned repos inside rooms use `repo-` prefix (e.g., `repo-AgentsCommander`). This is critical — the golden rule allows write access only to `repo-*` folders.
4. **Context paths:** Use relative paths (`../../_agent_NAME/Role.md`) so the room is portable.
5. **Role.md override:** If you place a Role.md inside the `__agent_*` folder, it overrides the parent's role. To use the parent's role, reference it in `context` instead.
6. **.gitignore:** The `.ac/.gitignore` MUST exclude `room-*/` to prevent the parent repo's git operations from corrupting room clones.

---

## 4. project-settings.json

Located at `.ac/project-settings.json`. Defines the coding agent configurations available for the project.

```json
{
  "agents": [
    {
      "id": "agent_TIMESTAMP_N",
      "label": "Claude Code",
      "command": "claude --dangerously-skip-permissions --effort max",
      "color": "#d97706"
    }
  ]
}
```

| Field | Description |
|---|---|
| `id` | Unique identifier (timestamp-based) |
| `label` | Display name in the UI |
| `command` | CLI command to launch this coding agent |
| `color` | UI color for this agent type |

---

## 5. Profile Path Placeholders

> These tokens are used **inside** a coding agent's own ENVIRONMENT rows, inside a profile cell's command or env, or inside the effective launch command. For the profile matrix itself (the lettered A/B/C launch variants per coding agent), see [Coding Agent Profiles](features/coding-agent-profiles.md).

Coding-agent command strings and `env` values may use a small set of `%...%` path placeholders that AgentsCommander expands to absolute paths **at launch**. Three command and env value surfaces are expanded: the effective launch command tokens, the coding agent's own ENVIRONMENT rows (`agents[].envs` in `settings.json`, edited in Settings → Coding Agents), and a profile cell's `env` map. The same three tokens are also substituted inside the **content** of seeded files, under different rules; see [Config seed](features/config-seed.md#token-substitution). Only the three tokens below are recognized. There is no shell to evaluate values, so `$`-style forms such as `$(pwd)` and `${VAR}` are **not** expanded; they pass through to the child process **verbatim**. Any other `%WORD%` marker (one that is not one of the three AC tokens) is **not** taken literally: it is **rejected** at launch as an unknown placeholder (fail-closed), and for `CODEX_HOME` it also fails at settings save-time.

The tokens map onto the matrix layout from the sections above: a replica is a `__agent_<name>` dir under a `room-*` room, the workspace is the project's `.ac` root, and the matrix is the canonical `_agent_<name>` dir.

| Token | Resolves to | Valid when |
|---|---|---|
| `%AC_REPLICA_ROOT%` | The **replica** dir — the launch working directory, canonicalized | A Room replica (`__agent_*` under `room-*`) **or** the `ac-root-agent` launch root |
| `%AC_WORKSPACE_ROOT%` | The **`.ac` workspace** root (the nearest `.ac` ancestor of the launch root) | Any launch root **inside** a `.ac` workspace — including non-replica roots (a `repo-*` checkout, a bare `room-*` dir, an `_agent_*` matrix dir) |
| `%AC_MATRIX_ROOT%` | The **matrix** dir `<workspace>\_agent_<name>` (the agent's canonical Agent Matrix) | **Only** a Room replica launch |

### Example resolutions

For a Room replica launched at `…\AgentsCommander_ac\.ac\room-6-dev-team\__agent_tech-lead`:

| Token | Expands to |
|---|---|
| `%AC_REPLICA_ROOT%` | `…\AgentsCommander_ac\.ac\room-6-dev-team\__agent_tech-lead` |
| `%AC_WORKSPACE_ROOT%` | `…\AgentsCommander_ac\.ac` |
| `%AC_MATRIX_ROOT%` | `…\AgentsCommander_ac\.ac\_agent_tech-lead` |

### Validity rules and the workspace/replica asymmetry

The three tokens have **different** validity gates. A value is rejected only when it uses a token that does not apply to the current launch root:

- **Room replica** (`__agent_*` under `room-*`): all three tokens resolve.
- **Root agent** (`ac-root-agent`): only `%AC_REPLICA_ROOT%` resolves (to the root-agent dir). `%AC_WORKSPACE_ROOT%` and `%AC_MATRIX_ROOT%` are unavailable — the root agent has no `.ac` workspace and no Agent Matrix — and error if used.
- **Non-replica launch root inside a `.ac` workspace** (a `repo-*` checkout at `…\.ac\room-6\repo-X`, a bare `room-*` dir, or an `_agent_*` matrix dir): **only** `%AC_WORKSPACE_ROOT%` resolves (its `.ac` ancestor exists); `%AC_REPLICA_ROOT%` and `%AC_MATRIX_ROOT%` still error there. This "workspace resolves but replica/matrix error" asymmetry is intentional.
- **Launch root outside any `.ac`** (a normal repo): none of the tokens resolve.

When a token is used where it does not apply, the launch fails with a specific error:

- `%AC_REPLICA_ROOT% requires an AC replica or root-agent launch root`
- `%AC_WORKSPACE_ROOT% requires a launch root inside an AC (.ac) workspace`
- `%AC_MATRIX_ROOT% requires an AC room replica launch root`

### Breaking change: `%AC_ROOT%` was removed

There is **no `%AC_ROOT%` token and no alias.** The former `%AC_ROOT%` (which resolved to the replica dir) was renamed to `%AC_REPLICA_ROOT%`. A profile that still contains the literal `%AC_ROOT%`:

- **fails at launch** — any unexpanded `%...%` marker is rejected with an "unknown placeholder marker" error; and
- for a `CODEX_HOME` value, **also fails at settings save-time** — validation reports `CODEX_HOME contains unknown placeholder %AC_ROOT%`.

Update any old configuration to `%AC_REPLICA_ROOT%` (or one of the new tokens).

### Usage examples

Use a placeholder as the **leading path segment** of an env value; the backend expands it and then validates the resulting absolute path:

```text
OPENCODE_CONFIG_DIR = %AC_REPLICA_ROOT%\.opencode
CODEX_HOME          = %AC_MATRIX_ROOT%\.codex
CLAUDE_CONFIG_DIR   = %AC_REPLICA_ROOT%\.claude
```

`CODEX_HOME` is validated more strictly than other keys: its value must **start** with a token as a complete leading path segment (or be a literal absolute path). A value like `prefix%AC_MATRIX_ROOT%\x`, where the token is not the leading segment, is rejected at save-time with a "must start with … as a complete path segment" error. `CLAUDE_CONFIG_DIR` and other env keys accept the tokens wherever a path is expected, with no special leading-segment rule.

The backend is the single authority for real expansion and absolute-path validation at launch; the sidebar's profile preview only mirrors these tokens for display.

For a worked end-to-end example that puts `%AC_MATRIX_ROOT%` in a coding agent's ENVIRONMENT row, see [RTK usage and per-agent statistics](integrations/rtk.md).

---

## 6. The .gitignore

**MANDATORY** at `.ac/.gitignore`:

```text
# AgentsCommander: exclude room cloned repos from parent git tracking.
# Without this, parent repo operations (checkout, reset) corrupt child clones.
room-*/
```

This is non-negotiable. Without it, `git checkout` or `git reset` on the parent repo will corrupt the room repo clones (which are independent git repositories nested inside the parent).

---

## 7. Complete Setup Checklist

When creating a full agent team for a new project:

### Step 1: Create `.ac/` structure

```text
.ac/
├── .gitignore                    # Must exclude room-*/
├── project-settings.json         # Coding agent config
├── _agent_ORCHESTRATOR/
│   └── Role.md
├── _agent_WORKER_1/
│   └── Role.md
├── _agent_WORKER_2/
│   └── Role.md
├── _agent_REVIEWER/
│   └── Role.md
└── _team_TEAMNAME/
    ├── config.json               # Lists all agents, coordinator, repos
    ├── conventions.md            # Shared conventions (optional)
    └── memory/                   # Shared memory (optional)
```

### Step 2 — Verify each agent has Role.md with frontmatter

```yaml
---
name: 'agent-name'
description: 'What this agent does — one line'
type: agent
---
```

### Step 3 — Verify team config uses portable agent references

All references in `_team_*/config.json` must be `_agent_<name>` (or bare `<name>`) refs to matrices **inside the same project's `.ac/`**. Never reference agents from other projects. Absolute paths are rejected by `team create` and by the discovery code.

### Step 4 — Verify in AgentsCommander

After setup, all agents should appear in the **AGENTS** section of the sidebar (not just under ROOMS). If an agent appears as `@other-project`, its team config is pointing to an external path.

### Step 5 — Test peer discovery

From any agent session, run:
```bash
"<BINARY_PATH>" list-peers --token <TOKEN> --root "<AGENT_ROOT>"
```
This should return all team members. If empty, the team config is misconfigured or the agent isn't listed in any team's `agents` array.

---

## 8. Common Mistakes & How to Avoid Them

### Mistake: Agents appear as `@other-project` in sidebar
**Cause:** Team config.json references `_agent_*` folders from a different project.
**Fix:** Create `_agent_*` folders inside THIS project's `.ac/` and update the team config paths.

### Mistake: `list-peers` returns empty
**Cause:** The calling agent isn't listed in any `_team_*/config.json` `agents` array, OR the team config paths don't match the agent's actual root path.
**Fix:** Verify the exact absolute path of the agent folder matches what's in the team config. Path mismatches (even trailing slashes or case differences on Windows) can cause failures.

### Mistake: Room agents load wrong Role.md
**Cause:** The `context` array in `__agent_*/config.json` still points to a generic role from another project.
**Fix:** Update the context path to reference the local project's `_agent_*/Role.md`:
```json
"context": [
  "$AGENTSCOMMANDER_CONTEXT",
  "../../_agent_NAME/Role.md"
]
```

### Mistake: Only creating agents in the room (double underscore)
**Cause:** Creating `__agent_*` folders inside `room-*/` but not `_agent_*` at the `.ac/` level.
**Fix:** Always create `_agent_*` (single underscore) at `.ac/` first. These are the canonical definitions. Room agents are replicas that reference them.

### Mistake: Agent folder has no Role.md
**Cause:** Creating the folder manually without the role file (or relying on an older `create-agent` behavior that wrote only `CLAUDE.md`).
**Fix:** Always create `Role.md` with proper frontmatter. This is the agent's identity. The current `create-agent` CLI creates the full matrix layout including `Role.md`.

### Mistake: Git operations corrupt room repos
**Cause:** Missing `.gitignore` at `.ac/` level that excludes `room-*/`.
**Fix:** Add `.gitignore` with `room-*/` before creating any rooms.

---

## 9. Agent Team Archetypes

These are proven team compositions. Adapt to your project's domain.

### Development Team (code projects)

| Agent | Role | Scope |
|---|---|---|
| **tech-lead** | Orchestrator | Breaks requirements into tasks, delegates, verifies, reports |
| **architect** | Planner | Designs implementation plans, maps affected files, flags cascading effects |
| **dev** | Implementer | Writes code, runs checks, commits to feature branches |
| **grinch** | Reviewer | Adversarial review — finds bugs, edge cases, security issues |
| **shipper** | Deployer | Builds, validates, packages, deploys |

### Minimal Team (small projects)

| Agent | Role | Scope |
|---|---|---|
| **lead** | Orchestrator + Planner | Plans and delegates (combines tech-lead + architect) |
| **dev** | Implementer | Writes code |
| **reviewer** | Quality gate | Reviews for correctness |

### Key design principle: **Separation of concerns**

- The one who plans should not implement (avoids blind spots)
- The one who implements should not review their own work (avoids confirmation bias)
- The one who coordinates should not merge/push (that's the user's decision)
- The one who reviews should never approve out of politeness

---

## 10. CLI Reference for Agent Management

### Create an agent programmatically

```bash
"<BINARY>" create-agent --project "<PROJECT>" --name "agent-name" --description "One-line description" [--role-template <id>] [--launch "Claude Code"]
```

`--project` is a registered AC project folder name (paths are not accepted). This creates `<project>/.ac/_agent_<id>/` with the matrix layout and writes `Role.md`. You can then edit `Role.md` to add your full role.

### Discover peers

```bash
"<BINARY>" list-peers --token <TOKEN> --root "<AGENT_ROOT>"
```

Returns JSON array of team peers with name, status, role, teams, reachability.

### Send a message

Messaging is file-based. Write your message to `<room-root>/messaging/YYYYMMDD-HHMMSS-<roomN>-<from>-to-<roomN>-<to>-<slug>.md`, then:

```bash
"<BINARY>" send --token <TOKEN> --root "<AGENT_ROOT>" --to "<peer_name>" --send <filename> --mode wake
```

The peer name comes from `list-peers` output. Use `--mode wake` for fire-and-forget.

---

## 11. Agent memory rotation at spawn

Every agent starts a fresh session with an empty `memory/`. AC does that for you: when it spawns a session for an agent, it rotates that agent's `memory/` directory in the **origin Agent Matrix** to `memory_YYYYMMDD_hhmmss/` and recreates an empty `memory/` next to it. The timestamp is local time.

Rotation always happens in the origin Agent Matrix, never in a room replica. If you launch the session from a replica (`room-*/__agent_<name>/`), AC reads the replica's own configuration to find the matrix it came from and rotates there. If that lookup lands on anything that is not a canonical Agent Matrix directory, AC rotates nothing and the session still launches.

### What happens in each case

| State of `memory/` | What AC does |
|---|---|
| Present and non-empty | Renames it to `memory_YYYYMMDD_hhmmss/`, then recreates an empty `memory/` |
| Present and empty | Nothing. An empty directory has no history to archive |
| Absent | Recreates it empty. Nothing is rotated, because there was nothing to move |
| A junction, a symlink, or any other reparse point | Refuses, with `memory is not a real directory`. Renaming the link would move the link entry itself and hand you a real directory in place of your link |
| Not a directory at all (a file named `memory`) | Refuses, with the same `memory is not a real directory` |
| A `memory_<timestamp>/` of the same name already exists | Refuses, with `memory rotation target already exists`. AC never overwrites an archive |

A refusal is not fatal. The session launches either way, and AC records the reason:

```text
[memory] #1172: memory rotation failed for <matrix root> (non-fatal; the session still launches): <error>
```

A successful rotation records:

```text
[memory] #1172: rotated <matrix>/memory -> <matrix>/memory_YYYYMMDD_hhmmss at fresh session spawn
```

The no-op cases (empty and absent) log nothing at this level, so silence in the log means "there was nothing to rotate", not "rotation broke".

### What this means for you as an agent

The `memory_YYYYMMDD_hhmmss/` directories are **read-only history**. Read them whenever you need what a previous session knew. Never edit one, never delete one, and never move files back out of one into `memory/`: your current `memory/` is the only directory you write to, and the archives are the record of how you got here.

If you find your `memory/` still holding the previous session's files, rotation refused. Check the log for the `[memory] #1172` line, and check whether `memory/` is a junction rather than a real directory.

See [Directory layout](reference/directory-layout.md) for where the Agent Matrix and its archives sit on disk.
