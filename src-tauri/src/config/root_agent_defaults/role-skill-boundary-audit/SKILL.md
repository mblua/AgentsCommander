---
name: role-skill-boundary-audit
description: Audit where governance instructions belong (Role.md, skill, global policy, workflow docs, memory, or an agent-boundary change) and enforce minimal verbosity in roles and skills. Diagnostic by default.
when_to_use: Before creating, modifying, approving, or auditing agents, Role.md files, skills, role templates, workflow instructions, or Agent Matrix structure. Also for matrix hygiene, oversized or bloated roles, authority language inside skills, duplicated instructions, and split/merge proposals.
---

# role-skill-boundary-audit

## Purpose

Audit the boundary between roles, skills, policies, process docs, memory, and agent shape.

```
Roles define who is responsible.
Skills define how to perform a reusable capability.
```

Diagnostic by default: recommend, do not rewrite, unless the user or active workflow asked for the refactor.

## Conciseness mandate (always)

Roles and skills spend context budget every time they load, so write the minimum that changes behavior. This mandate applies to every Role.md and skill you write, recommend, or rewrite.

- Add only what adds value; cut the rest. Every line must earn its place.
- Keep rationale to one line, and only where it guides a judgment the rule itself does not cover.
- No restatement: drop "Why" / "How to apply" / examples that repeat a rule without adding information.
- `Role.md` is the tightest surface (always loaded). Load-on-demand skills may hold more detail, but still no padding.
- When recommending or rewriting, target the smallest, least-verbose change that restores the boundary and preserves operative meaning.

## Classification

- Keep in Role: identity, ownership, authority, responsibilities, escalation, durable boundaries for one agent.
- Move to Skill: repeatable workflow, checklist, tool procedure, implementation pattern, domain method.
- Move to Global Policy: must constrain every agent or session regardless of role.
- Move to Workflow Docs: team process, operator/onboarding guide, durable docs humans browse outside startup context.
- Move to Memory: project fact, decision, preference, or status that persists but is not a standing instruction.
- Duplicate / Consolidate: same guidance in multiple places; pick one source of truth.
- Trim / Compress: content is in the right place but bloated; cut to the operative minimum.
- Split Agent: one role owns unrelated accountability surfaces.
- Merge Agent: agents differ mostly by wording or minor task variants.
- Needs Owner Decision: placement touches authority, access, team structure, or policy.

## Workflow

1. Identify the instruction or proposed change.
2. Name current vs proposed location.
3. Classify it.
4. Check for authority language, reusable procedure, duplicated guidance, agent-boundary drift, and verbosity.
5. Recommend the smallest, least-verbose change that restores a clear boundary and preserves meaning.
6. Stop at the recommendation if the change would rewrite files or split/merge agents, unless the user asked for the refactor.

## Output

```md
## Boundary Audit

Verdict: <one or more categories above>

Findings:
1. ...

Recommended Changes:
- ...

Risk / Notes:
- ...
```
