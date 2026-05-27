# Agent skills

For developers ready to add reusable per-agent workflows. Skills are markdown documents AC discovers from each agent's canonical `skills/` directory.

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

For an Agent Matrix agent, skills live beside the agent's canonical role,
memory, and plans:

This tree shows the skill-relevant canonical state only. Other Matrix entries,
such as `inbox/`, `outbox/`, and `config.json`, are omitted here.

```text
<project>/
+-- .ac-new/
    +-- _agent_dev-rust/
        +-- Role.md
        +-- memory/
        +-- plans/
        +-- skills/
```

When that agent runs inside a workgroup replica, the generated
AgentsCommander context allows writes to the origin Agent Matrix `skills/`
folder. That keeps skills canonical across replicas instead of copying them
into one temporary workgroup session.

Standalone agent folders can also contain a local `skills/` folder, but
AgentsCommander runtime discovery does not scan standalone local skills unless
canonical Agent Matrix state is resolved.

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

1. Open the canonical Agent Matrix directory for the agent, for example
   `.ac-new/_agent_dev-rust/`.
2. Create `skills/<skill-name>/`.
3. Add `skills/<skill-name>/SKILL.md` with YAML frontmatter.
4. Describe when to use the skill and the exact workflow to follow.
5. Add small reference files only when they reduce repeated instructions.

Skill names must be short, lowercase, and filesystem-friendly:
`rust-test-triage`, `release-notes`, `ui-accessibility-check`.

## Using a Skill

The agent no longer needs the user to name every skill. The generated context
includes a metadata index, and the agent should inspect `SKILL.md` when the
task matches `description` / `when_to_use`, or when the user names a skill.

When the agent is running from a workgroup replica, resolve `skills/...`
against the origin Agent Matrix directory named in the session context, not
against the replica's current working directory.

Full bodies and supporting files remain progressive-disclosure content. An
agent should:

1. Locate the relevant canonical `skills/<skill-name>/SKILL.md`.
2. Read `SKILL.md` before making changes.
3. Open only the referenced supporting files that matter for the current task.
4. Apply the workflow while still obeying the session's write restrictions.
5. Mention in the final report which skill was used when that helps review.

If the user names a skill that does not exist, the agent should say so and
continue with the best available fallback instead of inventing hidden behavior.

## Runtime Behavior

AgentsCommander supports:

- Discovery at session/context creation time.
- Deterministic skill ordering.
- Metadata extraction from Claude Code-compatible frontmatter.
- Missing-name fallback to the directory name.
- Missing-description warnings without body fallback.
- Duplicate same-scope name rejection.
- Generated context listing for discovered skills.
- Warnings for invalid, missing, oversized, or unreadable skill entrypoints.

AgentsCommander does not:

- Automatically execute `!` shell injections.
- Enforce `allowed-tools`, model, effort, hooks, or forked subagent semantics.
- Inject full skill bodies until an agent chooses to read/use a skill.
- Recursively discover nested skills.
- Discover standalone local `skills/` folders without canonical Agent Matrix
  state.
