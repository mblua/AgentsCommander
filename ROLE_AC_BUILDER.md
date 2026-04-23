# Role: AC Builder — Creating Agents, Teams & Workgroups in AgentsCommander

This document is the definitive guide for any AI agent tasked with creating or modifying the agent/team/workgroup structure in an AgentsCommander project. It captures the conventions, file formats, and pitfalls learned from building real multi-agent teams.

---

## Core Concepts

| Concept | Prefix | Location | Purpose |
|---|---|---|---|
| **Agent** | `_agent_` | `.ac-new/_agent_NAME/` | A role definition — who this agent is, what it does, what it must never do |
| **Team** | `_team_` | `.ac-new/_team_NAME/` | A grouping of agents that can message each other via `list-peers` / `send` |
| **Workgroup** | `wg-` | `.ac-new/wg-N-TEAMNAME/` | An isolated working environment with cloned agents + cloned repo for parallel work |
| **Workgroup Agent** | `__agent_` | `.ac-new/wg-N-TEAMNAME/__agent_NAME/` | A replica of a project-level agent inside a workgroup (double underscore) |

**Hierarchy:** Project → Agents + Teams → Workgroups (with replicated agents + repo clones)

---

## 1. Creating a Project-Level Agent (`_agent_*`)

Project-level agents appear in the **AGENTS** section of the AgentsCommander sidebar. They are the canonical definitions — workgroup agents are replicas of these.

### Folder Structure

```
.ac-new/_agent_NAME/
├── Role.md          # REQUIRED — the agent's identity, responsibilities, and rules
├── inbox/           # Created by AC on first use — incoming messages
├── outbox/          # Created by AC on first use — outgoing messages
├── memory/          # Created by AC on first use — persistent agent memory
├── plans/           # Created by AC on first use — plan files
├── skills/          # Created by AC on first use — reusable workflows
└── .agentscommander_mb/   # Created by AC — internal runtime state
    └── config.json        # Runtime config (tooling, session tracking)
```

**Minimum to create an agent:** A folder named `_agent_NAME/` containing a `Role.md` file. AgentsCommander creates the remaining directories (`inbox/`, `outbox/`, `memory/`, `plans/`, `skills/`, `.agentscommander_mb/`) automatically when the agent is first launched or used.

### Role.md Format

```markdown
---
name: 'agent-name'
description: 'One-line description of what this agent does'
type: agent
---

# Role: Display Name — Project Context

## Source of Truth

This role is defined in Role.md of your Agent Matrix at: .ac-new/_agent_NAME/
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

```
.ac-new/_team_NAME/
├── config.json      # REQUIRED — defines members, coordinator, and repos
├── conventions.md   # Optional — shared conventions across the team
└── memory/          # Optional — shared team memory
```

### config.json Format

```json
{
  "agents": [
    "C:\\Users\\USER\\path\\to\\project\\.ac-new\\_agent_one",
    "C:\\Users\\USER\\path\\to\\project\\.ac-new\\_agent_two",
    "C:\\Users\\USER\\path\\to\\project\\.ac-new\\_agent_three"
  ],
  "coordinator": "C:\\Users\\USER\\path\\to\\project\\.ac-new\\_agent_one",
  "repos": [
    {
      "agents": [
        "C:\\Users\\USER\\path\\to\\project\\.ac-new\\_agent_one",
        "C:\\Users\\USER\\path\\to\\project\\.ac-new\\_agent_two",
        "C:\\Users\\USER\\path\\to\\project\\.ac-new\\_agent_three"
      ],
      "url": "https://github.com/owner/repo.git"
    }
  ]
}
```

**Fields:**

| Field | Required | Description |
|---|---|---|
| `agents` | Yes | Array of absolute paths to `_agent_*` folders. These agents become peers and can message each other. |
| `coordinator` | Yes | Absolute path to the agent that coordinates work. Shown with `COORDINATOR` badge in sidebar. |
| `repos` | Yes | Array of repo objects. Each has `agents` (who works on this repo) and `url` (the git remote). |

### Critical Rules for Team Config

1. **Use absolute paths.** The `agents` array and `coordinator` must be absolute filesystem paths to `_agent_*` folders within the SAME project's `.ac-new/` directory.

2. **Agents must exist.** Every path in the `agents` array must point to an existing `_agent_*` folder with a `Role.md`. If the folder doesn't exist, the agent won't appear.

3. **Don't reference external projects.** If you're building a team for project A, the agents must be `_agent_*` folders inside project A's `.ac-new/`. Referencing agents from project B (e.g., `C:\repos\other-project\.ac-new\_agent_foo`) makes them appear as `@other-project` in the sidebar — they belong to the wrong project.

4. **The coordinator must be in the agents list.** The coordinator path must also appear in the `agents` array.

5. **Repos.agents can be a subset.** Not every team member needs access to every repo. The `repos[].agents` array specifies which agents work on which repo.

---

## 3. Workgroup Structure (`wg-*`)

Workgroups are isolated working environments created when a team needs to work on a task in parallel. They contain **replicas** of agents (double underscore `__agent_*`) and **clones** of repositories (`repo-*`).

### Folder Structure

```
.ac-new/wg-N-TEAMNAME/
├── BRIEF.md                    # Objective, scope, and deliverables for this workgroup
├── __agent_NAME/               # Replica of _agent_NAME (double underscore)
│   ├── config.json             # Points to parent agent's identity + local repo
│   ├── Role.md                 # Optional override — if absent, uses parent's Role.md via config
│   ├── inbox/
│   ├── outbox/
│   └── .agentscommander_mb/
│       └── config.json
├── __agent_OTHER/
│   └── ...
└── repo-REPONAME/              # Shallow clone of the team's repo
    ├── .git/
    ├── _plans/                 # Plans created during this workgroup's work
    └── (repository contents)
```

### Workgroup Agent config.json

```json
{
  "context": [
    "$AGENTSCOMMANDER_CONTEXT",
    "$REPOS_WORKSPACE_INFO",
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
| `context` | Array of context sources. `$AGENTSCOMMANDER_CONTEXT` and `$REPOS_WORKSPACE_INFO` are AC-injected variables. The third entry is the path to the Role.md that defines this agent's personality. |
| `identity` | Path to the parent agent folder. This is the canonical identity — the workgroup agent is a replica of this. |
| `repos` | Relative paths to the repo clones inside this workgroup. |

### Key Conventions

1. **Naming:** `wg-N-TEAMNAME` where N is sequential (1, 2, 3...) and TEAMNAME matches the team.
2. **Double underscore:** Workgroup agents use `__agent_` (two underscores) to distinguish from project-level `_agent_` (one underscore).
3. **Repo prefix:** Cloned repos inside workgroups use `repo-` prefix (e.g., `repo-AgentsCommander`). This is critical — the golden rule allows write access only to `repo-*` folders.
4. **Context paths:** Use relative paths (`../../_agent_NAME/Role.md`) so the workgroup is portable.
5. **Role.md override:** If you place a Role.md inside the `__agent_*` folder, it overrides the parent's role. To use the parent's role, reference it in `context` instead.
6. **.gitignore:** The `.ac-new/.gitignore` MUST exclude `wg-*/` to prevent the parent repo's git operations from corrupting workgroup clones.

---

## 4. project-settings.json

Located at `.ac-new/project-settings.json`. Defines the coding agent configurations available for the project.

```json
{
  "agents": [
    {
      "id": "agent_TIMESTAMP_N",
      "label": "Claude Code",
      "command": "claude --dangerously-skip-permissions --effort max",
      "color": "#d97706",
      "gitPullBefore": false,
      "excludeGlobalClaudeMd": true
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
| `gitPullBefore` | Whether to `git pull` before starting a session |
| `excludeGlobalClaudeMd` | Whether to exclude the user's global CLAUDE.md from context |

---

## 5. The .gitignore

**MANDATORY** at `.ac-new/.gitignore`:

```
# AgentsCommander: exclude workgroup cloned repos from parent git tracking.
# Without this, parent repo operations (checkout, reset) corrupt child clones.
wg-*/
```

This is non-negotiable. Without it, `git checkout` or `git reset` on the parent repo will corrupt the workgroup repo clones (which are independent git repositories nested inside the parent).

---

## 6. Complete Setup Checklist

When creating a full agent team for a new project:

### Step 1 — Create `.ac-new/` structure

```
.ac-new/
├── .gitignore                    # Must exclude wg-*/
├── project-settings.json         # Coding agent config
├── _agent_COORDINATOR/
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

### Step 3 — Verify team config uses absolute paths to local agents

All paths in `_team_*/config.json` must point to `_agent_*` folders **inside the same project's `.ac-new/`**. Never reference agents from other projects.

### Step 4 — Verify in AgentsCommander

After setup, all agents should appear in the **AGENTS** section of the sidebar (not just under WORKGROUPS). If an agent appears as `@other-project`, its team config is pointing to an external path.

### Step 5 — Test peer discovery

From any agent session, run:
```bash
"<BINARY_PATH>" list-peers --token <TOKEN> --root "<AGENT_ROOT>"
```
This should return all team members. If empty, the team config is misconfigured or the agent isn't listed in any team's `agents` array.

---

## 7. Common Mistakes & How to Avoid Them

### Mistake: Agents appear as `@other-project` in sidebar
**Cause:** Team config.json references `_agent_*` folders from a different project.
**Fix:** Create `_agent_*` folders inside THIS project's `.ac-new/` and update the team config paths.

### Mistake: `list-peers` returns empty
**Cause:** The calling agent isn't listed in any `_team_*/config.json` `agents` array, OR the team config paths don't match the agent's actual root path.
**Fix:** Verify the exact absolute path of the agent folder matches what's in the team config. Path mismatches (even trailing slashes or case differences on Windows) can cause failures.

### Mistake: Workgroup agents load wrong Role.md
**Cause:** The `context` array in `__agent_*/config.json` still points to a generic role from another project.
**Fix:** Update the context path to reference the local project's `_agent_*/Role.md`:
```json
"context": [
  "$AGENTSCOMMANDER_CONTEXT",
  "$REPOS_WORKSPACE_INFO",
  "../../_agent_NAME/Role.md"
]
```

### Mistake: Only creating agents in the workgroup (double underscore)
**Cause:** Creating `__agent_*` folders inside `wg-*/` but not `_agent_*` at the `.ac-new/` level.
**Fix:** Always create `_agent_*` (single underscore) at `.ac-new/` first. These are the canonical definitions. Workgroup agents are replicas that reference them.

### Mistake: Agent folder has no Role.md
**Cause:** Using `create-agent` CLI which creates CLAUDE.md, or creating the folder manually without the role file.
**Fix:** Always create `Role.md` with proper frontmatter. This is the agent's identity.

### Mistake: Git operations corrupt workgroup repos
**Cause:** Missing `.gitignore` at `.ac-new/` level that excludes `wg-*/`.
**Fix:** Add `.gitignore` with `wg-*/` before creating any workgroups.

---

## 8. Agent Team Archetypes

These are proven team compositions. Adapt to your project's domain.

### Development Team (code projects)

| Agent | Role | Scope |
|---|---|---|
| **tech-lead** | Coordinator | Breaks requirements into tasks, delegates, verifies, reports |
| **architect** | Planner | Designs implementation plans, maps affected files, flags cascading effects |
| **dev** | Implementer | Writes code, runs checks, commits to feature branches |
| **grinch** | Reviewer | Adversarial review — finds bugs, edge cases, security issues |
| **shipper** | Deployer | Builds, validates, packages, deploys |

### Minimal Team (small projects)

| Agent | Role | Scope |
|---|---|---|
| **lead** | Coordinator + Planner | Plans and delegates (combines tech-lead + architect) |
| **dev** | Implementer | Writes code |
| **reviewer** | Quality gate | Reviews for correctness |

### Key design principle: **Separation of concerns**

- The one who plans should not implement (avoids blind spots)
- The one who implements should not review their own work (avoids confirmation bias)
- The one who coordinates should not merge/push (that's the user's decision)
- The one who reviews should never approve out of politeness

---

## 9. CLI Reference for Agent Management

### Create an agent programmatically

```bash
"<BINARY>" create-agent --parent "<.ac-new path>" --name "agent-name" [--launch "Claude Code"] --root "<caller root>" --token "<token>"
```

This creates the folder and a basic CLAUDE.md. You'll still need to write a proper Role.md.

### Discover peers

```bash
"<BINARY>" list-peers --token <TOKEN> --root "<AGENT_ROOT>"
```

Returns JSON array of team peers with name, status, role, teams, reachability.

### Send a message

Messaging is file-based. Write your message to `<workgroup-root>/messaging/YYYYMMDD-HHMMSS-<wgN>-<from>-to-<wgN>-<to>-<slug>.md`, then:

```bash
"<BINARY>" send --token <TOKEN> --root "<AGENT_ROOT>" --to "<peer_name>" --send <filename> --mode wake
```

The peer name comes from `list-peers` output. Use `--mode wake` for fire-and-forget.
