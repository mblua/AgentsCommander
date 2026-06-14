---
name: role-skill-boundary-audit
description: Audit whether governance instructions belong in Role.md, a skill, global policy, workflow docs, memory, or an agent boundary change.
when_to_use: Use before finalizing changes that create, modify, approve, or audit agents, Role.md files, skills, role templates, workflow instructions, or Agent Matrix structure; also use for matrix hygiene, oversized roles, authority language inside skills, duplicated instructions, and proposals to split or merge agents.
---

# role-skill-boundary-audit

## Purpose

Use this skill to audit the boundary between roles, skills, policies, process documentation, memory, and agent shape.

Core rule:

```text
Roles define who is responsible.
Skills define how to perform a reusable capability.
```

The audit is diagnostic by default. Do not rewrite roles, skills, or agent structure unless the user or the active workflow explicitly requests that refactor.

## When To Apply

Apply this skill before finalizing work that creates, modifies, approves, or audits:

- agents
- `Role.md` files
- skills
- role templates
- workflow instructions
- Agent Matrix structure

Also apply it when:

- a role grows unusually large
- a role contains repeatable operational procedure
- a skill contains authority or ownership language
- similar instructions appear in multiple roles
- someone proposes another agent for a bounded capability
- periodic matrix hygiene or audit is requested

## Classification Guide

Use these categories:

- Keep in `Role.md`: identity, ownership, authority, responsibilities, escalation rules, and durable boundaries for one agent.
- Move to Skill: repeatable workflow, checklist, tool procedure, implementation pattern, or domain method that can be reused by one or more tasks.
- Move to Global Policy: instruction that must constrain every agent or every session regardless of role.
- Move to Workflow Docs: team process, operator guide, onboarding guide, or durable documentation that humans should browse outside agent startup context.
- Move to Memory: project-specific fact, decision, preference, or status that should persist but is not a standing instruction.
- Duplicate / Consolidate: same or near-same guidance appears in multiple roles, skills, or docs and should have one source of truth.
- Split Agent: one role owns unrelated responsibilities that should be separate accountability surfaces.
- Merge Agent: multiple agents differ mostly by wording or minor task variants and should be one role plus skills.
- Needs Owner Decision: the placement affects authority, access, team structure, or policy and cannot be decided safely by the auditor alone.

## Workflow

1. Identify the instruction or proposed change.
2. Name the current location and proposed location.
3. Classify it using the categories above.
4. Check for authority language, reusable procedure, duplicated guidance, and agent-boundary drift.
5. Recommend the smallest change that restores a clear boundary.
6. If the change would rewrite files or merge or split agents, stop at the recommendation unless the user asked for the refactor.

## Output

```md
## Boundary Audit

Verdict:
- Keep in Role
- Move to Skill
- Move to Global Policy
- Move to Workflow Docs
- Move to Memory
- Duplicate / Consolidate
- Split Agent
- Merge Agent
- Needs Owner Decision

Findings:
1. ...
2. ...

Recommended Changes:
- ...

Risk / Notes:
- ...
```
