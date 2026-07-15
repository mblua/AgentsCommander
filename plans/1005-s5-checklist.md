# #1005 S5 checklist (issue #1016)

Author: dev-rust (wg-25). Branch `feat/1016-1005-s5-root-durables`, base main @ 409b7f90.
Per plan Stage 5 + 6.3/6.4/6.5 + G3/G4/G8 + E5.x. Sources at base: `ROOT_ROLE_MD` (root_agent.rs:356-409), recognizer (:415-425), `migrate_root_role` pristine list (:714-717), `DEFAULT_ROOT_SKILLS` (:57-70), both `root_agent_defaults/*/SKILL.md`, `root_spec` (seeded_context_templates.rs:275-285, v4), `build_role_content` (entity_creation.rs:346-389).

## Grinch blind OLD-side inventory (4.5(a0)) - RESERVED

> Grinch: append your blind OLD-text inventory HERE, committed before reading the
> mapping tables below. Old texts: `git show 409b7f90:src-tauri/src/config/root_agent.rs`
> (ROOT_ROLE_MD + lists), `git show 409b7f90:src-tauri/src/config/root_agent_defaults/<skill>/SKILL.md`
> (both skills), `git show 409b7f90:src-tauri/src/commands/entity_creation.rs` (B3 fn + golden).

### Grinch a0 record (derived blind from `git show 409b7f90`, BEFORE reading the tables below)

Derived 2026-07-15 from base artifacts only: root_agent.rs (:356-409 ROOT_ROLE_MD, :415-425 recognizer, :683-738 migrate_root_role, :57-71 DEFAULT_ROOT_SKILLS, ~:2759 anti-drift test), both SKILL.md blobs, entity_creation.rs (:346-389 build_role_content). Independent G3 recomputes (python, no head consts consulted), all LF-only, no CR, no U+2014 in B1/B2:

- B1 ROOT_ROLE_MD raw-literal content: **2516 bytes, sha256 `f100cfcf4df40c0ce1e81b6bebc89f7eca79eb1d4cfef9298e8abd3da53c1e73`**
- B2 audit SKILL.md blob: **3199 bytes, sha256 `b3237843e2a6e9ac3cb014735ab398ee552711b0c017f8858758c619b3344c3f`**
- B2 agency SKILL.md blob: **2578 bytes, sha256 `9bc8a2cd565357bfeb85efac224917331e52821aba0b36ae2b831da6aaf657e5`**

Base machinery state: recognizer `is_known_generated_root_context_template` = 5 entries (OLD_ROOT_ROLE_MD, OLD_ROOT_CONTEXT_WITH_COORDINATION_MD, BEFORE_BOUNDARY_AUDIT, BEFORE_AGENCY_SKILL, current); `migrate_root_role` pristine list = 4 entries (same minus WITH_COORDINATION - recognizer-only, E5.4); ROOT_ROLE_MD is `LazyLock<String>` (E5.3 target); DEFAULT_ROOT_SKILLS legacy_snapshots: audit = EMPTY (never repair), agency = [PRE_YAML_FIX]; anti-drift test `shipped_agency_skill_is_the_snapshot_with_a_quoted_description` asserts shipped agency == PRE_YAML_FIX modulo quoted description (must be REPLACED when shipped text changes, E5.6).

#### B1 ROOT_ROLE_MD - rows R-1..13

- R-1 ANCHOR: frontmatter `name: 'agents-commander'` / `description: 'Static supplemental root context for AgentsCommander.'` / `type: agent`.
- R-2 ANCHOR: heading `# Agents Commander`.
- R-3 IDENTITY: "You are the AgentsCommander Root Agent. You are the top-level coordinator for this AgentsCommander binary."
- R-4 `## Responsibility` PROCEDURE: top-level planning/oversight for sessions, workgroups, agents of THIS instance; help user inspect available work, plan delegation, track status, synthesize results (4 duties).
- R-5 `## State` GRANT: durable state in canonical `ac-root-agent` dir; 4-bullet list memory/ plans/ skills/ Role.md.
- R-6 IDENTITY+RULE: NOT a workgroup replica, NO origin Agent Matrix; use canonical root dir for own durable state.
- R-7 `## Coordination` PROCEDURE: coordinate across workgroups at high level; delegate specialized implementation to appropriate team coordinators; synthesize their results for user.
- R-8 `## Team and workgroup setup` PROCEDURE (ordered): 1. create missing agents `create-agent-matrix`; 2. `team create` choosing ONE coordinator + worker agents; 3. `workgroup add` using ONLY `--project`, `--team`, `--title` (flag-restriction qualifier).
- R-9 RULE: agents must exist BEFORE team creation; team creation defines membership and repo access; workgroup activation uses the existing team definition.
- R-10 `## Governance Boundary Audits` RULE (trigger set 1): before finalizing any work that creates, modifies, approves, or audits agents, Role.md files, skills, role templates, workflow instructions, or Agent Matrix structure (4 verbs x 6 objects) -> load and apply `skills/role-skill-boundary-audit/SKILL.md`.
- R-11 RULE (trigger set 2): also when role grows unusually large; role contains repeatable operational procedure; skill contains authority/ownership language; similar instructions in multiple roles; another agent proposed for a bounded capability; periodic matrix hygiene requested (6 triggers).
- R-12 RULE: audit is a review lens; structured recommendation BEFORE any refactor; NOT silently rewrite roles, skills, or agent boundaries.
- R-13 `## Agency Agents Roles` RULE: before creating ANY new specialist agent (any role-defined `create-agent-matrix`) -> load and apply `skills/agency-agents-roles/SKILL.md`; names its 4 content areas (mandatory offer, real-local-data-never-invented, bounded skip exceptions, `agency-templates` CLI flow).

Plan-named directive to check: the two trigger paragraphs (R-10+R-11) compress into one sentence + trigger list WITHOUT dropping any of the 4 verbs, 6 objects, or 6 triggers.

#### B2a role-skill-boundary-audit SKILL.md - rows A-1..7

- A-1 frontmatter: name; description = audit where governance instructions belong (Role.md, skill, global policy, workflow docs, memory, agent-boundary change = 6 destinations) + enforce minimal verbosity + "Diagnostic by default."; when_to_use = before creating/modifying/approving/auditing agents, Role.md files, skills, role templates, workflow instructions, Agent Matrix structure; also matrix hygiene, oversized/bloated roles, authority language inside skills, duplicated instructions, split/merge proposals.
- A-2 `## Purpose`: boundary audit across roles/skills/policies/process docs/memory/agent shape + fenced couplet "Roles define who is responsible. / Skills define how to perform a reusable capability." (ANCHOR).
- A-3 RULE: diagnostic by default - recommend, do NOT rewrite, UNLESS the user or active workflow asked for the refactor.
- A-4 `## Conciseness mandate (always)` RULE: context-budget rationale; minimum that changes behavior; applies to EVERY Role.md and skill you write, recommend, or rewrite; 5 bullets (earn its place; rationale one line only where it guides judgment beyond the rule; no restatement of Why/How-to-apply/examples; Role.md tightest surface always loaded vs load-on-demand skills more detail but no padding; smallest least-verbose change preserving operative meaning).
- A-5 `## Classification`: 10 categories with definitions (Keep in Role: identity/ownership/authority/responsibilities/escalation/durable boundaries; Move to Skill: repeatable workflow/checklist/tool procedure/implementation pattern/domain method; Global Policy: constrains every agent or session regardless of role; Workflow Docs: team process/operator-onboarding/durable human docs; Memory: project fact/decision/preference/status, persists but not standing instruction; Duplicate-Consolidate: one source of truth; Trim-Compress: right place but bloated; Split Agent: unrelated accountability surfaces; Merge Agent: differ mostly by wording/minor variants; Needs Owner Decision: authority/access/team structure/policy).
- A-6 `## Workflow`: 6 ordered steps incl. step-4 check list (authority language, reusable procedure, duplicated guidance, agent-boundary drift, verbosity) and step-6 STOP rule (stop at recommendation if change would rewrite files or split/merge agents, unless the user asked).
- A-7 `## Output`: fenced md template (## Boundary Audit / Verdict: categories / Findings: / Recommended Changes: / Risk / Notes:) - shape ANCHOR.

#### B2b agency-agents-roles SKILL.md - rows G-1..8

- G-1 frontmatter: name; quoted description (mandatory offer; identifying from real local data - source repo and cached templates, never invented; bounded skip exceptions; agency-templates CLI flow; missing-cache handling); when_to_use (before any role-defined create-agent-matrix; also when user asks to add/create/set up new specialist role or agent).
- G-2 RULE: MUST first offer Agency Agents role templates before creating any new specialist agent; "mandatory, not discretionary".
- G-3 RULE (bounded skip): ONLY if IN THIS SESSION the user already declined Agency templates OR explicitly asked for custom/from-scratch role (session-scope qualifier + 2 exceptions).
- G-4 RULE: say what Agency Agents is stating ONLY what real local data supports; never invent a description or recall one from memory.
- G-5 RULE bullets: tested shareable role templates in a source repository; real source = `repo` value from `agency-templates status` (and cache manifest), NOT a guessed URL; no local one-line description exists - describe by source repo + actual templates (real names + 1-line descriptions from `agency-templates list`), not invented prose; cache absent -> say so + offer fetch + ASK before downloading/updating BECAUSE it writes to the local cache (consent qualifier).
- G-6 PROCEDURE+ANCHOR: three exact command lines (`agency-templates update --ref main` / `status --pretty` / `list --pretty`) via `"<AGENTSCOMMANDER_BINARY_PATH>"`.
- G-7 RULE: command semantics (update refreshes cache from source repo, `--ref` selects git ref default main; status reports cache presence + repo/ref/commit; list prints each cached template's real `id` + 1-line `description`).
- G-8 PROCEDURE+RULE: present candidate template(s); create with `create-agent-matrix --role-template <id>`; use ONLY IDs and descriptions that command returns; never invent template IDs or descriptions.

#### B3 build_role_content scaffold - rows S-1..4

- S-1 shape: frontmatter `name`/`description` (single-quote-escaped)/`type: agent` + `# {name}` + description + optional fenced `## Role Profile` (HTML-comment delimiters, provenance id, "mandatory sections stay last").
- S-2 `## Source of Truth` RULE: "This role is defined in Role.md of your Agent Matrix at: .ac/_agent_{name}/"; if running as a replica, this file was generated from that source; ALWAYS use memory/, plans/, skills/ from your Agent Matrix, treat Role.md there as canonical; "Never use external memory systems."
- S-3 `## Agent Memory Rule` RULE (replica-CONDITIONAL at base): IF running as a replica, single source of truth for persistent knowledge = Matrix memory/plans/skills/Role.md; replica folder ONLY for replica-local scratch, inbox/outbox, session artifacts; "NEVER use external memory systems from the coding agent (e.g., ~/.claude/projects/memory/)". Declared S5 widening drops the replica conditional - adjudication: check the unconditional rewrite stays TRUE for matrix-origin agents (no phantom "replica folder"/"your Agent Matrix" references that are false outside replicas) and keeps all three verbatim needles.
- S-4 keep-exact: the `<!-- ac:role-profile source="{}" - imported template body ... -->` delimiter comment contains a REAL U+2014 em-dash (keep-exact, provenance-commented); ordering guarantees (mandatory sections last; template body cannot push them off) pinned by existing tests :4980-:5110.

Danger rows (dropped-qualifier hunt on the new side): R-8 "using only --project --team --title"; R-12 "before any refactor" + "not silently rewrite"; A-3 + A-6 step 6 unless-asked stop conditions; A-4 "(always)" + "every Role.md and skill"; G-3 "in this session" scope; G-5 ask-before-download consent + never-invented triple; G-8 "use only ... never invent"; S-3 conditional-drop truth check.

---

## G3 freeze provenance (five mechanical parts)

- **B1 bytes**: `ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD` transplanted VERBATIM from the shipped raw literal (not retyped). Provenance: one-off scratch-test run of `default_root_context_template()` AT base 409b7f90 (added, run, removed pre-commit) printed len 2516, sha256 `f100cfcf4df40c0ce1e81b6bebc89f7eca79eb1d4cfef9298e8abd3da53c1e73`. Pin `root_context_before_token_minimization_snapshot_is_byte_exact` asserts those externally captured values.
- **B2 audit bytes**: `ROLE_SKILL_BOUNDARY_AUDIT_BEFORE_TOKEN_MINIMIZATION` generated from `git show 409b7f90:...role-skill-boundary-audit/SKILL.md` (blob is LF): len 3199, sha256 `b3237843e2a6e9ac3cb014735ab398ee552711b0c017f8858758c619b3344c3f`; normalized sha `b27e1a9a..07c3`. Pin asserts LF-folded + normalized forms (G8).
- **B2 agency bytes**: `AGENCY_AGENTS_ROLES_BEFORE_TOKEN_MINIMIZATION` from `git show 409b7f90:...agency-agents-roles/SKILL.md` (LF): len 2578, sha256 `9bc8a2cd565357bfeb85efac224917331e52821aba0b36ae2b831da6aaf657e5`; normalized sha `2bcb95d2..45a3a`. Same pin shape.
- Both skill consts are raw string literals, never `include_str!` (#914), LF everywhere; generated by script from the git blobs, never transcribed.
- **B3**: no freeze (no on-disk migration exists or is added); old golden was already an inline transcription in test #21, replaced by the new golden inline.

## Failing-first record (every mechanical part witnessed)

| run | state | result |
|---|---|---|
| FF1 | B1 const+tests added; lists/bump/rewrite absent | `frozen_v4_root_context_is_recognized_and_migrated_on_both_paths` FAILED at assert_ne (frozen == live); 58 passed / 1 failed |
| FF2 | B1 v5 rewrite applied; lists/bump still absent | same test FAILED at `is_known_generated_root_context_template(FROZEN)` (recognizer gap); `ensure_root_agent_dir_at_migrates_old_root_template_defaults` FAILED (pristine v4 template preserved as custom); 57/2 |
| FF3 | recognizer extended to 6 entries ONLY | same test FAILED at "pristine v4 Role.md must reduce to the minimal role" (migrate_root_role list gap witnessed live = the E5.4 silent stranding); 58/1 |
| FF4 | migrate list extended to 5 entries | same test FAILED at "root_spec current_version must be bumped to 5" (left 4, right 5); 58/1 |
| FF5 | version bumped 4 -> 5 | full lib suite GREEN 2304/0 |
| FF-B2a | skill consts+pins+repair tests added; files/lists untouched | both `frozen_*_skill_is_repaired_to_current` FAILED at assert_ne (frozen == shipped); 61/2 |
| FF-B2b | both SKILL.md rewritten + anti-drift test replaced | both repair tests FAILED at the self-repair assert (audit stranded via NoSnapshots, agency via UserEdit - both list gaps witnessed through the REAL `DEFAULT_ROOT_SKILLS` table via `ensure_root_agent_dir_at`); 61/2 |
| FF-B2c | legacy_snapshots extended (audit 0->1, agency 1->2) | full lib GREEN 2308/0 |
| FF-B3 | golden test #21 updated to new baseline; fn untouched | `build_role_content_no_template_matches_legacy` FAILED ("must be byte-identical to the #1005 S5 golden") |
| FF-B3b | fn bodies rewritten | GREEN (one unrelated `cli::task_ops` concurrency flake observed once; green alone and on full rerun 2308/0) |

E5.4 one-fixture rule: the SAME frozen const drives recognizer assert + Role.md-to-MINIMAL + template-auto-upgrade + state currentVersion==5, in one test.

## Harvested needles (4.4) - all kept verbatim

**B1** (pinned by `ensure_root_agent_dir_at_creates_layout_role_and_config` :1768-1785 and migration tests :2018/:2134-2135): positive `You are the AgentsCommander Root Agent`, `skills/agency-agents-roles/SKILL.md`, `create-agent-matrix`, `team create`, `workgroup add`, `Agents must exist before team creation`, `role-skill-boundary-audit`, `` `Role.md` files ``, `skills`, `Agent Matrix structure`; negative (stay absent) `verified workgroup coordinator replicas only`, `list-peers-lean`, `AGENTSCOMMANDER_TOKEN`, `agency-templates update`, `Do not invent Agency template IDs`, `workgroup add --coordinator`. Self-checked by script before writing.

**B2 audit** (session_context.rs:7664, A5-rendered): `Audit where governance instructions belong (Role.md` (description opener). `Scope: Root Agent durable skills` (:7663) is A5 render machinery, not skill text.

**B2 agency**: no external phrase pins (all tests compare whole content and auto-track); structural constraints kept: description stays double-quoted (inner `": "`), when_to_use stays an unquoted plain scalar with no inner `": "`.

**B3** (entity_creation.rs:5097/:5101): `This role is defined in Role.md of your Agent Matrix`, `NEVER use external memory systems from the coding agent`, plus the `~/.claude/projects/memory/` example (plan keep). Keep-exact: frontmatter shape, `# {name}` title, Role Profile fence pair incl. the provenance em-dash comment, headings `## Source of Truth` / `## Agent Memory Rule` (ordering tests :5033-5062 untouched; :5128 untouched per G9).

## Mapping tables

### B1 ROOT_ROLE_MD v5 (2,516 -> 2,469)

| # | Class | Old | New carrier |
|---|---|---|---|
| 1 | ANCHOR | YAML frontmatter (name/description/type) | byte-identical |
| 2 | ANCHOR | `# Agents Commander` title | byte-identical |
| 3 | IDENTITY | two identity sentences (Root Agent / top-level coordinator) | merged: "You are the AgentsCommander Root Agent, the top-level coordinator for this AgentsCommander binary." (needle prefix intact) |
| 4 | PROCEDURE | Responsibility two sentences | merged with colon; all four duties kept |
| 5 | RULE | State intro + four-item list | verbatim |
| 6 | RULE | not-a-replica/no-matrix + use-canonical-dir | merged with semicolon; both clauses kept |
| 7 | RULE | Coordination two sentences | merged with colon; delegation rule intact |
| 8 | PROCEDURE | Team setup intro + 3 ordered steps | verbatim (command anchors) |
| 9 | RULE | agents-before-team + team-defines-membership | verbatim |
| 10 | RULE | Governance trigger paragraph 1 (finalizing-work trigger list) | merged with row 11 per plan: one sentence + full trigger list; every trigger kept |
| 11 | RULE | Governance trigger paragraph 2 ("Also apply... requested") | folded into row 10 ("and when a role grows...") |
| 12 | RULE | review-lens / recommendation-not-rewrite | "The audit is a review lens: produce a structured recommendation before any refactor, never silently rewrite roles, skills, or agent boundaries." ("It should produce" -> "produce"; "not silently rewrite" -> "never silently rewrite") |
| 13 | ANCHOR | Agency Agents Roles pointer | byte-identical (plan: already compact) |

No row dropped. LazyLock -> plain `&str` const (E5.3); accessor signature unchanged; `OLD_ROOT_CONTEXT_WITH_COORDINATION_MD` remains a LazyLock (interpolates B5).

### B2 role-skill-boundary-audit (3,199 -> 3,061)

| # | Class | Old | New carrier |
|---|---|---|---|
| F1 | RULE (frontmatter) | description "...an agent-boundary change) and enforce minimal verbosity in roles and skills. Diagnostic by default." | "...); enforce minimal verbosity in roles and skills. Diagnostic by default." (connective only; needle opener verbatim; all six destinations kept) |
| F2 | RULE (frontmatter) | when_to_use (two trigger sentences) | UNCHANGED: every clause is a trigger condition; halving would drop triggers (deviation 2) |
| 1 | RULE | Purpose sentence "Audit the boundary between roles, skills, policies, process docs, memory, and agent shape." | DROPPED: restates the description's parenthetical (workflow docs/process docs; agent shape carried by the Split/Merge categories + description's "agent-boundary change") |
| 2 | ANCHOR | roles/skills definition code block | byte-identical |
| 3 | RULE | diagnostic-by-default + refactor exception | verbatim ("unless the user or active workflow asked for the refactor" kept) |
| 4 | RULE | mandate intro (context-budget rationale + scope) | reordered: "Write the minimum that changes behavior; roles and skills spend context budget every time they load. This applies to every Role.md and skill you write, recommend, or rewrite." (both clauses kept) |
| 5 | RULE | "Add only what adds value; cut the rest. Every line must earn its place." | "Add only what adds value; cut the rest." (third clause restates the first two) |
| 6-8 | RULE | rationale-one-line / no-restatement / tightest-surface bullets | verbatim |
| 9 | RULE | smallest-least-verbose-change bullet | verbatim (kept despite overlap with workflow step 5: different scopes - writing vs audit recommendation) |
| 10 | RULE | 10 classification categories | verbatim, all ten |
| 11 | PROCEDURE | 6 workflow steps | verbatim, all six |
| 12 | ANCHOR | output template | byte-identical |

### B2 agency-agents-roles (2,578 -> 2,464)

| # | Class | Old | New carrier |
|---|---|---|---|
| F1 | RULE (frontmatter) | description: "identifying Agency Agents from real local data (its source repo and cached templates, never invented)" + "handling a missing local template cache" | "describing Agency Agents from real local data only (never invented)" + "handling a missing template cache" (summary enumeration dropped; carriers = body bullets 1-2 which name source repo AND cached templates; "local" carried by body bullet 3) |
| F2 | RULE (frontmatter) | when_to_use "..., i.e. before any role-defined create-agent-matrix." | "(any role-defined create-agent-matrix)." (connective; both trigger sentences kept) |
| 1 | RULE | MUST-offer sentence | verbatim |
| 2 | RULE | "This is mandatory, not discretionary." | DROPPED: restates the MUST (carrier = row 1) |
| 3 | RULE | skip exceptions ("ONLY if, in this session, ... declined ... or explicitly asked ...") | verbatim, all qualifiers |
| 4 | RULE | real-data-only intro (state ONLY what real local data supports; never invent/recall) | verbatim |
| 5 | RULE | bullet 1 (collection + real source = repo value from status + cache manifest, not guessed URL) | verbatim |
| 6 | RULE | bullet 2 (no local one-liner; describe by repo + actual templates from list; not invented prose) | reordered: "Describe ... (their real names and 1-line descriptions from `agency-templates list`); there is no local one-line project description to quote." ("not with invented prose" dropped: carriers = row 4 "Never invent" + row 9 "never invent template IDs or descriptions") |
| 7 | RULE | bullet 3 (absent cache -> say so, offer fetch; ask before download because it writes) | verbatim |
| 8 | ANCHOR | CLI intro + 3 command lines + update/status/list explanation | byte-identical |
| 9 | RULE | present candidates + create with --role-template + IDs-from-list-only | verbatim |

### B3 build_role_content section bodies (fixed-args scaffold 873 -> 705)

| # | Class | Old | New carrier |
|---|---|---|---|
| 1 | RULE (SoT) | defined in Role.md at .ac/_agent_{name}/ | verbatim (needle) |
| 2 | RULE (SoT) | replica copy generated from that source | verbatim + "; the Agent Matrix is canonical." |
| 3 | RULE (SoT) | "Always use memory/, plans/, and skills/ from your Agent Matrix, and treat Role.md there as the canonical role definition. Never use external memory systems." | DROPPED: carriers = AMR row 4 (matrix dirs = single source of persistent knowledge), SoT row 2's new canonical clause, AMR row 6 (NEVER external + example) |
| 4 | RULE (AMR) | "If you are running as a replica, the single source of truth for persistent knowledge is your Agent Matrix's memory/, plans/, skills/, and Role.md." | "Your Agent Matrix's memory/, plans/, skills/, and Role.md are the single source of persistent knowledge." WIDENING declared: drops the replica conditional; the rule was already true unconditionally (a non-replica origin agent's matrix is its own dirs), and the replica-folder rule (row 5) keeps its own scope |
| 5 | RULE (AMR) | replica folder only for scratch/inbox-outbox/session artifacts | verbatim |
| 6 | RULE (AMR) | NEVER external memory + ~/.claude example | verbatim (needle) |

No on-disk migration (user-owned Role.md); only newly created agents get the smaller scaffold.

## G6 cross-stage anchor re-grep

All S1-S4 designated anchors mechanically re-grepped against the four new S5 texts (B1, both skills, B3 fn): zero hits ("Read-only operations on ANY path", the S3 root grant, "closed background", "not work to resume", A8/A9 needles, "Narrow exception", "GOLDEN RULE", "self-handoff-and-clear", F1/F2 sentences). B1 negative needles re-verified by script.

## Em-dash constraint map (4.2)

B1, both SKILL.md files, and the two rewritten B3 bodies: U+2014-free (script-asserted). Surviving keep-exact em-dash: the B3 `ac:role-profile source=` provenance comment (plan :360, template branch untouched, ordering test green). No pinned-free constraint existed on B1/B2 texts; policy applied anyway.

## Measurements

Baseline @ 409b7f90 (harness + supplement rows):

| item | chars | ~tokens |
|---|---|---|
| profile: Root Agent | 14339 | 3584 |
| profile: Root Agent + auto_self_clear | 16967 | 4241 |
| supplement: B1 root context template | 2516 | 629 |
| supplement: B3 created-agent Role.md scaffold | 873 | 218 |
| file: role-skill-boundary-audit/SKILL.md | 3199 | 799 |
| file: agency-agents-roles/SKILL.md | 2578 | 644 |

Head @ 56046d8 (fmt commit 56db797 changes no text):

| item | chars | ~tokens | delta chars |
|---|---|---|---|
| profile: Root Agent | 14285 | 3571 | -54 (boot-visible B2 frontmatter, G4) |
| profile: Root Agent + auto_self_clear | 16913 | 4228 | -54 |
| profile: WG replica / coordinator (+auto) | unchanged | | 0 |
| supplement: B1 root context template | 2469 | 617 | -47 (non-boot: context[] durable) |
| supplement: B3 created-agent Role.md scaffold | 705 | 176 | -168 (never boots) |
| file: role-skill-boundary-audit/SKILL.md | 3061 | 765 | -138 (body load-on-demand) |
| file: agency-agents-roles/SKILL.md | 2464 | 616 | -114 (body load-on-demand) |

G4 split: boot-visible S5 win = -13 tok/Root boot (frontmatter only). Durable wins: B1 -47, B2 bodies -252 combined file bytes, B3 -168 chars per created agent.

## Deviations / flags

1. **Win far below plan estimates** (B1 2526->~1500; frontmatter "halved"; issue "~350-400 tok boot-visible"): the plan keep-list retains every B1 section and nearly every sentence; both skills' frontmatter is trigger-dense (halving would drop trigger conditions, which the same sentence forbids); B2 bodies' keep-lists pin categories/steps/templates/commands. Executed every plan-named merge and drop; kept everything else uncut per STOP-and-flag. Same standard cause flagged in S3 deviation 1 and S4 deviation 1.
2. **when_to_use not halved** (both skills): every clause is a trigger condition; only connectives trimmed (agency) or nothing (audit). Declared in mapping tables F2 rows.
3. **fmt gate posture**: base main @ 409b7f90 is not fmt-clean (160 pre-existing diff blocks across 36 files; CI does not gate fmt per pr-regression-gates.yml). My new code made fmt-clean in `56db797`; pre-existing violations left untouched (S1 lesson: whole-crate fmt noise stays out of #1005 PRs).
4. **CRLF/CI posture (E5.8, for the PR description)**: `migrate_default_skill_file` normalizes both sides (:1141-1152); seeding writes verbatim `include_str!` bytes; frozen consts are raw LF literals; shipped-bytes pins fold CRLF->LF before hashing (G8). LF-checkout CI and autocrlf working copies both pass.
5. One unrelated `cli::task_ops::tests::concurrent_set_title_and_append_body_both_apply` flake observed once during B3; green alone and on immediate full rerun.
