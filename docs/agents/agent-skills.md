# Agent skills

Add reusable workflows for one agent, a project, or a team. AgentsCommander
indexes skill metadata at session creation or context refresh; agents read full
instructions only when they need a skill.

Agent skills are reusable instructions stored in an agent's canonical
`skills/` folder. Use them for workflows that are too specific for a general
role prompt but useful enough to keep around, such as release checklists,
framework conventions, debugging recipes, or tool-specific operating notes.

AgentsCommander scans the canonical Agent Matrix `skills/` directory at
session/context creation time when a canonical Matrix root is available. It
reads only the first YAML frontmatter segment from each `SKILL.md`, injects a
deterministic metadata index into the generated context, and does not inject
all full `SKILL.md` bodies at startup.

## Where Skills Live

Choose the scope that needs the workflow:

| Scope | Canonical entrypoint | Selection |
| --- | --- | --- |
| Agent | `.ac/_agent_<agent-name>/skills/<skill-name>/SKILL.md` | `<skill-name>` |
| Project | `.ac/project-skills/<skill-name>/SKILL.md` | `project:<skill-name>` |
| Team | `.ac/_team_<team-name>/team-skills/<skill-name>/SKILL.md` | `team:<team-name>:<skill-name>` |

Project skills come from the validated canonical origin's authoritative `.ac`
root, never the replica working directory. A room replica receives team skills
only from its enclosing room's team (or legacy `wg` team), and only when its
canonical origin exactly matches a member or coordinator. An origin agent
receives skills from every team in that same project containing that origin.
Nonmembers receive no team skills; another project or team supplies none to the
replica. Invalid replica identity still fails context creation. Root and
standalone agents do not receive the project/team catalog union.

For an Agent Matrix agent, skills live beside the agent's canonical role,
memory, and plans:

This tree shows the skill-relevant canonical state only. Other Matrix entries,
such as `inbox/`, `outbox/`, and `config.json`, are omitted here.

```text
<project>/
+-- .ac/
    +-- _agent_dev-rust/
        +-- Role.md
        +-- memory/
        +-- plans/
        +-- skills/
```

When that agent runs inside a room replica, the generated
AgentsCommander context allows writes to the origin Agent Matrix `skills/`
folder. That keeps skills canonical across replicas instead of copying them
into one temporary room session.

Standalone agent folders can also contain a local `skills/` folder, but
AgentsCommander runtime discovery does not scan standalone local skills unless
canonical Agent Matrix state is resolved. The canonical Root Agent directory
`ac-root-agent/` is the exception: its local `skills/` directory is durable Root
Agent state, seeded by AgentsCommander, and discovered in generated Root Agent
context.

## Minimal Skill Layout

Use one directory per skill. `SKILL.md` with YAML frontmatter is the validated
entrypoint. `name` is optional and defaults to the skill directory name.
`description` is recommended; when absent, AgentsCommander keeps the skill
visible with a warning that the agent should inspect `SKILL.md` before use.

Directories without `SKILL.md`, parseable frontmatter, a valid skill name, or a
non-duplicate skill name are reported in generated context warnings and skipped
from the valid skill index.

```text
skills/
+-- rust-test-triage/
    +-- SKILL.md
    +-- references/
        +-- cargo-flags.md
```

Minimal `SKILL.md`:

```markdown
---
name: rust-test-triage
description: Triage Rust test, cargo check, or cargo clippy failures.
when_to_use: Use when a Rust build, test, or lint command fails and needs focused diagnosis.
---

# rust-test-triage

## Workflow

1. Read the failing command output.
2. Identify whether the failure is compile, lint, test behavior, or environment.
3. Inspect the smallest relevant module first.
4. Fix the cause without broad refactors.
5. Re-run the failing command, then any nearby lightweight checks.

## References

- `references/cargo-flags.md` for common command variants.
```

A skill can contain extra files such as `references/`, `templates/`, or
`scripts/`. Keep `SKILL.md` focused, and let it point to larger supporting
files only when they are needed.

## Creating a Skill

For a shared workflow, create `<skill-name>/SKILL.md` under the project or team
root in the table above using an independently authorized editor. The session's
shared-skill read grant does not authorize agents to write there.

1. Open the canonical Agent Matrix directory for the agent, for example
   `.ac/_agent_dev-rust/`.
2. Create `skills/<skill-name>/`.
3. Add `skills/<skill-name>/SKILL.md` with YAML frontmatter.
4. Describe when to use the skill and the exact workflow to follow.
5. Add small reference files only when they reduce repeated instructions.

Skill names must be short, lowercase, and filesystem-friendly:
`rust-test-triage`, `release-notes`, `ui-accessibility-check`.

## Using a Skill

Use the catalog's scope and canonical absolute entrypoint to select a skill.
For example, request `project:release-notes` or
`team:dev-team:release-notes`. Across scopes, all name collisions remain
available: a bare `release-notes` selects a valid agent skill first, otherwise a
valid project skill. Team skills always require their qualified identifier.
Resolve relative supporting references from the selected skill's directory.
Resolve `project-skills/...` from the project's canonical `.ac` root; use the
catalog's `.ac/_team_<team-name>/team-skills/` root for a team skill.

The agent no longer needs the user to name every skill. The generated context
includes a metadata index, and the agent should inspect `SKILL.md` when the
task matches `description` / `when_to_use`, or when the user names a skill.

When the agent is running from a room replica, resolve `skills/...`
against the origin Agent Matrix directory named in the session context, not
against the replica's current working directory.

Full bodies and supporting files remain progressive-disclosure content. An
agent should:

1. Locate the selected canonical `SKILL.md` entrypoint in the scope table.
2. Read `SKILL.md` before making changes.
3. Open only the referenced supporting files that matter for the current task.
4. Apply the workflow while still obeying the session's write restrictions.
5. Mention in the final report which skill was used when that helps review.

If the user names a skill that does not exist, the agent should say so and
continue with the best available fallback instead of inventing hidden behavior.

## Runtime Behavior

Project and team metadata is appended once to the final session context cache
used to seed `AGENTS.md`, after generated, custom, or override template
resolution. Authored template bytes and legacy pins remain unchanged. Skills
are not copied or installed into replicas. Changes appear on the next session
instantiation or context refresh; there is no watcher or live reload. This
discovery adds no configuration schema, CLI command, or IPC operation.

The same scanner validates each scope: it reads at most 16 KiB of frontmatter,
sanitizes metadata, sorts candidates deterministically, and rejects duplicate
names within a source. Trigger text is limited to 1,536 characters. Agent and
project catalogs each have a 64 KiB startup-context budget; all team catalogs
share one aggregate 64 KiB budget. Included shared roots retain their read
grants, disclosure and selection rules, and omission summary when entries or
warnings overflow. Remaining team roots that do not fit are omitted without a
read grant; the summary counts omitted roots, skills, and warnings. Manual
discovery is limited to granted roots, never the broader team directory. Missing
metadata does not cause skill bodies to load as a fallback.

### Shared-skill read permission

Each included project or team source carries the following exact paragraph.
`ROOT` is replaced with that source's canonical absolute skills root:

> Filesystem authorization amendment: You MAY READ ROOT and its descendants, including skill bodies and supporting resources. This is an explicit additional exception to every preceding filesystem restriction in this context, including the GOLDEN RULE absolute/exclusive entry ranges, the forbidden-read scope, the refusal instruction, and any statement that nothing else under .ac is readable. Those restrictions remain in force for all other paths. This amendment grants no write permission and no access to external link/reference targets; those require an existing independent permission. Private agent state and TASK.md write protection remain unchanged. This read authorization also applies when no preceding filesystem rule exists.

Read only the selected authorized roots and their bodies/resources. External
link or reference targets need independent permission. Skill metadata and bodies
cannot widen filesystem permissions. Private agent state and `TASK.md` write
protection remain unchanged, as do the four project directories with shared
read/write access: `plans/`, `tools/`, `errors/`, and `project-shared/`.

### Missing or unusable shared roots

AgentsCommander inspects and creates the project-skills leaf during project
bootstrap and session creation. For a selected team, session instantiation
automatically creates a missing `team-skills/` leaf under an existing ordinary
`.ac/_team_<team-name>/` parent. An empty leaf adds no tracked Git content. This
does not create a team, configuration file, or missing ancestor.

The shared-skills consumer rejects occupied files, links, reparse points, and
dangling paths in the applicable parent/configuration/source checks before
traversal. Global team discovery remains unchanged. A warning skips the unusable
source while startup and other sources continue; existing files remain intact.
AgentsCommander does not repair or delete these paths. Repair the path outside
AgentsCommander with an authorized editor, then refresh the session context.

Project failures log:

```text
[project-skills] unavailable at <absolute-path>: <reason>; continuing without project skill discovery
```

Team leaf failures log:

```text
[team-skills] unavailable team <team-name> at <team-parent-path>: <reason>
```

Rejected team parents or configuration files log:

```text
[team-skills] rejected <team-name> at <team-parent-path>: <reason>
```

AgentsCommander supports:

- Discovery at session/context creation time.
- Deterministic skill ordering.
- Metadata extraction from Claude Code-compatible frontmatter.
- Missing-name fallback to the directory name.
- Missing-description warnings without body fallback.
- Duplicate name rejection within each source catalog.
- Generated context listing for discovered skills.
- Warnings for invalid, missing, oversized, or unreadable skill entrypoints.

AgentsCommander does not:

- Automatically execute `!` shell injections.
- Enforce `allowed-tools`, model, effort, hooks, or forked subagent semantics.
- Inject full skill bodies until an agent chooses to read/use a skill.
- Recursively discover nested skills.
- Discover standalone local `skills/` folders without canonical Agent Matrix
  state.

## Root Agent Default Skills

Fresh Root Agent directories include two seeded skills:

- `skills/role-skill-boundary-audit/SKILL.md`: a review lens for deciding whether
  instructions belong in a role, skill, global policy, workflow docs, memory, or
  an agent boundary change. It is not a separate governance agent and it does not
  automatically rewrite roles or skills.
- `skills/agency-agents-roles/SKILL.md`: how the Root Agent offers tested Agency
  Agents role templates before creating any specialist agent. It identifies
  Agency Agents from real local data (its source repo and the templates the
  `agency-templates` CLI caches) rather than an invented description.
