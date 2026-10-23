#[cfg(test)]
use super::DEFAULT_ROOT_SKILLS;

/// #1005 S5: `role-skill-boundary-audit/SKILL.md` exactly as it shipped from
/// #654 through base commit 409b7f90, frozen so a pristine copy on disk keeps
/// self-repairing after the token-minimization rewrite. This makes the audit
/// skill's `legacy_snapshots` non-empty for the first time. Never edit.
///
/// Generated, never transcribed (G3):
/// git show 409b7f90:src-tauri/src/config/root_agent_defaults/role-skill-boundary-audit/SKILL.md
/// printed len 3199, sha256
/// b3237843e2a6e9ac3cb014735ab398ee552711b0c017f8858758c619b3344c3f. Raw string
/// literal, never `include_str!` (#914): LF everywhere. Pinned by
/// `role_skill_boundary_audit_before_token_minimization_snapshot_is_byte_exact`
/// against those externally captured values, never against this const itself.
pub(crate) const ROLE_SKILL_BOUNDARY_AUDIT_BEFORE_TOKEN_MINIMIZATION: &str = concat!(
    r#"---
name: role-skill-boundary-audit
description: Audit where governance instructions belong (Role.md, skill, global policy, workflow "#,
    r#"docs, memory, or an agent-boundary change) and enforce minimal verbosity in roles and "#,
    r#"skills. Diagnostic by default.
when_to_use: Before creating, modifying, approving, or auditing agents, Role.md files, skills, "#,
    r#"role templates, workflow instructions, or Agent Matrix structure. Also for matrix "#,
    r#"hygiene, oversized or bloated roles, authority language inside skills, "#,
    r#"duplicated instructions, and split/merge proposals.
---

# role-skill-boundary-audit

## Purpose

Audit the boundary between roles, skills, policies, process docs, memory, and agent shape.

```
Roles define who is responsible.
Skills define how to perform a reusable capability.
```

Diagnostic by default: recommend, do not rewrite, unless the user or active workflow asked for "#,
    r#"the refactor.

## Conciseness mandate (always)

Roles and skills spend context budget every time they load, so write the minimum that changes "#,
    r#"behavior. This mandate applies to every Role.md and skill you "#,
    r#"write, recommend, or rewrite.

- Add only what adds value; cut the rest. Every line must earn its place.
- Keep rationale to one line, and only where it guides a judgment the rule itself does not cover.
- No restatement: drop "Why" / "How to apply" / examples that repeat a rule without adding "#,
    r#"information.
- `Role.md` is the tightest surface (always loaded). Load-on-demand skills may hold more detail, "#,
    r#"but still no padding.
- When recommending or rewriting, target the smallest, least-verbose change that restores the "#,
    r#"boundary and preserves operative meaning.

## Classification

- Keep in Role: identity, ownership, authority, responsibilities, escalation, durable boundaries "#,
    r#"for one agent.
- Move to Skill: repeatable workflow, checklist, tool procedure, implementation pattern, domain "#,
    r#"method.
- Move to Global Policy: must constrain every agent or session regardless of role.
- Move to Workflow Docs: team process, operator/onboarding guide, durable docs humans browse "#,
    r#"outside startup context.
- Move to Memory: project fact, decision, preference, or status that persists but is not a "#,
    r#"standing instruction.
- Duplicate / Consolidate: same guidance in multiple places; pick one source of truth.
- Trim / Compress: content is in the right place but bloated; cut to the operative minimum.
- Split Agent: one role owns unrelated accountability surfaces.
- Merge Agent: agents differ mostly by wording or minor task variants.
- Needs Owner Decision: placement touches authority, access, team structure, or policy.

## Workflow

1. Identify the instruction or proposed change.
2. Name current vs proposed location.
3. Classify it.
4. Check for authority language, reusable procedure, duplicated guidance, agent-boundary drift, "#,
    r#"and verbosity.
5. Recommend the smallest, least-verbose change that restores a clear boundary and preserves "#,
    r#"meaning.
6. Stop at the recommendation if the change would rewrite files or split/merge agents, unless "#,
    r#"the user asked for "#,
    r#"the refactor.

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
"#
);

/// #1005 S5: `agency-agents-roles/SKILL.md` exactly as it shipped from #909
/// (the quoted-description fix) through base commit 409b7f90, frozen so a
/// pristine copy on disk keeps self-repairing after the token-minimization
/// rewrite. Never edit.
///
/// Generated, never transcribed (G3):
///     git show 409b7f90:src-tauri/src/config/root_agent_defaults/agency-agents-roles/SKILL.md
/// printed len 2578, sha256
/// 9bc8a2cd565357bfeb85efac224917331e52821aba0b36ae2b831da6aaf657e5. Raw string
/// literal, never `include_str!` (#914): LF everywhere. Pinned by
/// `agency_agents_roles_before_token_minimization_snapshot_is_byte_exact`
/// against those externally captured values, never against this const itself.
pub(crate) const AGENCY_AGENTS_ROLES_BEFORE_TOKEN_MINIMIZATION: &str = concat!(
    r#"---
name: agency-agents-roles
description: "How the Root Agent offers Agency Agents role templates before creating any "#,
    r#"specialist agent: the mandatory offer, identifying Agency Agents from real local data "#,
    r#"(its source repo and cached templates, never invented), the bounded skip exceptions, "#,
    r#"the agency-templates CLI flow, and handling a missing local template cache."
when_to_use: Load before creating any new specialist agent, i.e. before any role-defined "#,
    r#"create-agent-matrix. Also whenever the user asks to add, create, or set up a "#,
    r#"new specialist role or agent.
---

# agency-agents-roles

## Mandatory offer before creating a specialist agent

Before you create any new specialist agent (any role-defined `create-agent-matrix`), you MUST "#,
    r#"first offer Agency Agents role templates. This is mandatory, not "#,
    r#"discretionary.

Skip the offer ONLY if, in this session, the user already declined Agency templates or "#,
    r#"explicitly asked for a custom or from-scratch role.

## Say what Agency Agents is, from real data only

When you offer, briefly say what Agency Agents is, but state ONLY what real local data supports. "#,
    r#"Never invent a description or recall one from memory.

- Agency Agents is a collection of tested, shareable role templates published in a source "#,
    r#"repository. The real source is the `repo` value "#,
    r#"reported by `agency-templates status` (and stored "#,
    r#"in the cache manifest), not a URL you guess.
- There is no local one-line project description to quote. Describe Agency Agents concretely by "#,
    r#"its source repo plus the actual templates "#,
    r#"available (their real names and "#,
    r#"1-line descriptions from "#,
    r#"`agency-templates list`), not "#,
    r#"with invented prose.
- If the template cache is absent (status reports it unavailable), say so and offer to fetch it. "#,
    r#"Ask before "#,
    r#"downloading or "#,
    r#"updating, "#,
    r#"because "#,
    r#"it writes to the local template cache.

## On acceptance, use the CLI

Use the AgentsCommander CLI from `AGENTSCOMMANDER_BINARY_PATH`:

    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates update --ref main
    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates status --pretty
    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates list --pretty

`update` refreshes the local cache from the source repo (`--ref` selects the git ref, default "#,
    r#"`main`). `status` reports whether a cache is present and its repo, ref, and commit. "#,
    r#"`list` prints each cached template's real `id` and 1-line `description`.

Then present the candidate template(s) and create with `create-agent-matrix --role-template "#,
    r#"<id>`. Use only the IDs and descriptions that command returns; never invent template IDs "#,
    r#"or descriptions.
"#
);

/// #909: the `agency-agents-roles/SKILL.md` we shipped from `00eca16` (PR #682) through
/// `646aeac`. Its `description` is an unquoted YAML plain scalar containing an inner
/// `": "`, so `serde_yaml` rejects it at line 2 column 105 and `discover_skill_index`
/// drops the skill entirely. Frozen here so a pristine copy can be recognized and
/// repaired without ever touching a user-edited file.
///
/// Generated, never transcribed:
///     git show 646aeac:src-tauri/src/config/root_agent_defaults/agency-agents-roles/SKILL.md
///
/// A raw string literal rather than an `include_str!` of a frozen `.md`, because
/// `.gitattributes` pins `*.rs` to `text eol=lf` but carries no rule for `*.md` (#914).
/// An `include_str!`ed `.md` would be CRLF on a Windows `autocrlf` checkout and LF on CI,
/// so its raw digest would differ per machine. This literal is LF everywhere.
///
/// Pinned by `agency_agents_roles_pre_yaml_fix_snapshot_is_byte_exact`, on both the raw
/// form (2576 bytes) and the normalized form. The normalized pin alone is blind to
/// exactly what normalization erases.
pub(crate) const AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX: &str = concat!(
    r#"---
name: agency-agents-roles
description: How the Root Agent offers Agency Agents role templates before creating any "#,
    r#"specialist agent: the mandatory offer, identifying Agency Agents from real local data "#,
    r#"(its source repo and cached templates, never invented), the bounded skip exceptions, "#,
    r#"the agency-templates CLI flow, and handling a missing local template cache.
when_to_use: Load before creating any new specialist agent, i.e. before any role-defined "#,
    r#"create-agent-matrix. Also whenever the user asks to add, create, or set up a "#,
    r#"new specialist role or agent.
---

# agency-agents-roles

## Mandatory offer before creating a specialist agent

Before you create any new specialist agent (any role-defined `create-agent-matrix`), you MUST "#,
    r#"first offer Agency Agents role templates. This is mandatory, not "#,
    r#"discretionary.

Skip the offer ONLY if, in this session, the user already declined Agency templates or "#,
    r#"explicitly asked for a custom or from-scratch role.

## Say what Agency Agents is, from real data only

When you offer, briefly say what Agency Agents is, but state ONLY what real local data supports. "#,
    r#"Never invent a description or recall one from memory.

- Agency Agents is a collection of tested, shareable role templates published in a source "#,
    r#"repository. The real source is the `repo` value "#,
    r#"reported by `agency-templates status` (and stored "#,
    r#"in the cache manifest), not a URL you guess.
- There is no local one-line project description to quote. Describe Agency Agents concretely by "#,
    r#"its source repo plus the actual templates "#,
    r#"available (their real names and "#,
    r#"1-line descriptions from "#,
    r#"`agency-templates list`), not "#,
    r#"with invented prose.
- If the template cache is absent (status reports it unavailable), say so and offer to fetch it. "#,
    r#"Ask before "#,
    r#"downloading or "#,
    r#"updating, "#,
    r#"because "#,
    r#"it writes to the local template cache.

## On acceptance, use the CLI

Use the AgentsCommander CLI from `AGENTSCOMMANDER_BINARY_PATH`:

    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates update --ref main
    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates status --pretty
    "<AGENTSCOMMANDER_BINARY_PATH>" agency-templates list --pretty

`update` refreshes the local cache from the source repo (`--ref` selects the git ref, default "#,
    r#"`main`). `status` reports whether a cache is present and its repo, ref, and commit. "#,
    r#"`list` prints each cached template's real `id` and 1-line `description`.

Then present the candidate template(s) and create with `create-agent-matrix --role-template "#,
    r#"<id>`. Use only the IDs and descriptions that command returns; never invent template IDs "#,
    r#"or descriptions.
"#
);

/// Test-only view of the shipped default skill table, so the indexer gates in
/// `session_context::tests` can assert coverage without widening the table's visibility.
/// `root_agent` already answers questions about its private constants through accessors;
/// see `default_root_context_template` and `is_known_generated_root_context_template`.
#[cfg(test)]
pub(crate) fn default_root_skill_dir_names() -> Vec<&'static str> {
    DEFAULT_ROOT_SKILLS
        .iter()
        .map(|skill| skill.dir_name)
        .collect()
}

/// Test-only. `session_context::tests` writes this to disk, with `\r\n`, to model the state
/// every broken install on disk is actually in.
#[cfg(test)]
pub(crate) fn agency_pre_yaml_fix_snapshot() -> &'static str {
    AGENCY_AGENTS_ROLES_SKILL_PRE_YAML_FIX
}
