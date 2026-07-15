# #1005 S6 checklist (issue #1021, FINAL stage)

Author: dev-rust (wg-25). Branch `feat/1021-1005-s6-global-skeleton`, base main @ ec660c17.
Per plan Stage 6 + 6.7 + G3/G12/E2.2. Sources at base: `get_default_agent_template` (session_context.rs:2113-2137), `CORE_CONCEPTS_SECTION` (:2633-2636), `ROOT_RUNTIME_PROLOGUE_HEADER` (:2621-2623), recognizers (seeded_context_templates.rs:308-310 project, :323-326 standalone), global spec v1 (:256).

## Grinch blind OLD-side inventory (4.5(a0)) - RESERVED

> Grinch: append your blind OLD-text inventory HERE, committed before reading the
> mapping tables below. Old texts: `git show ec660c17:src-tauri/src/config/session_context.rs`
> (template fn + the two consts), `git show ec660c17:src-tauri/src/config/seeded_context_templates.rs`
> (recognizers + spec).

---

## G3 freeze provenance

- `GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION` transplanted VERBATIM from the shipped escaped literal (not retyped). Provenance: one-off scratch-test run of `get_default_agent_template()` AT base ec660c17 (added, run, removed pre-commit; tree verified clean after) printed len 611, sha256 `c9de5b80ad99a5743ad20c3344e7dd03888792f4da175943bee72e3d7d91fb88`. Pin `global_pre_token_minimization_snapshot_is_byte_exact` asserts those externally captured values via the existing `hash_text`.
- `STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS` (307-byte snapshot) untouched; its pin untouched.

## Failing-first record (every mechanical part witnessed)

| run | state | result |
|---|---|---|
| FF1 | freeze const + pin + migration test added; recognizers/bump/rewrite absent | migration test FAILED at assert_ne (frozen == live); 39 passed / 1 failed (seeded filter) |
| FF2 | v2 rewrite + drift pair applied (template + CORE_CONCEPTS_SECTION + prologue header, one edit set); recognizers/bump absent | migration test FAILED at "project recognizer must accept the frozen v1 bytes"; REST OF SUITE GREEN 2309/1 (needle updates held; nothing else pinned the old wording) |
| FF3 | project recognizer extended (1 -> 2 entries) ONLY | FAILED at "standalone (retirement) recognizer must accept the frozen v1 bytes" (second recognizer gap witnessed separately, the :260 silent-strand case) |
| FF4 | standalone recognizer extended (2 -> 3) | FAILED at "global current_version must be bumped to 2" (left 1, right 2); the old-v1-on-disk auto-upgrade assert PASSED here, proving the seeded-state SHA flow with both recognizers in place |
| FF5 | version bumped 1 -> 2 | full lib GREEN 2310/0 |

## Harvested needles (4.4)

Kept verbatim (zero churn): `# AgentsCommander Context` title (legacy required_once marker + list_peers PREAMBLE_PREFIXES); preamble opening prefix `You are running inside an AgentsCommander session` (list_peers.rs:211 sentinel filter + its :2008 test); `## Core Concepts` + `# Workspace Repos` token (#664 not-legacy markers :3444); all seven `{{PLACEHOLDERS}}` byte-identical in original order (mechanically verified); `**Team**: the logical capability and organization.` INCLUDING the trailing period (:4731); `# AgentsCommander Root Runtime Context` (prologue heading, many tests).

**Declared needle rename** (6.1: renamed needle listed next to the rule it guards): Workgroup first clause `**Workgroup**: an operational runtime replica instance` -> `**Workgroup**: a runtime replica of a team`. Guarded rules: Workgroup definition present in the Root prologue (:4732, prefix form) and in the materialized replica context (:6470, full-clause form). Both assertions updated in the same commit; FF2's 2309-green run proves no third site pinned the old clause.

Not needles but verified: frozen fixtures carrying the OLD preamble stay untouched (seeded :89 standalone snapshot; session_context :3283 legacy compat renderer uses the em-dash variant and is frozen corpus).

## Mapping table: A1 skeleton (611 -> 567 chars; all three carriers changed together)

| # | Class | Old | New carrier |
|---|---|---|---|
| 1 | ANCHOR | `# AgentsCommander Context` | byte-identical |
| 2 | IDENTITY | preamble "...a terminal session manager that coordinates multiple AI agents." | "...a terminal session manager coordinating multiple AI agents." (prefix + full meaning kept; -6) |
| 3 | RULE | Team def clause 1 "the logical capability and organization." | verbatim (needle incl. period) |
| 4 | RULE | Team def clause 2 "It defines who can work together, who coordinates, and which repos are available." | "It defines membership, who coordinates, and which repos are available." ("who can work together" -> "membership": the established term from B1's "Team creation defines membership and repo access"; all three defined aspects kept; -11) |
| 5 | RULE | Workgroup def clause 1 "an operational runtime replica instance of a team for a specific task" | "a runtime replica of a team for a specific task" ("operational" carried by "runtime"; "instance" carried by "replica"; declared rename above; -27) |
| 6 | RULE | Workgroup def clause 2 "It contains replica agents and `repo-*` working repositories." | "...working repos." (A6's established short form; -8) |
| 7 | ANCHOR | seven placeholders + blank-line layout | byte-identical, same order |

Drift pair discipline: CORE_CONCEPTS_SECTION and ROOT_RUNTIME_PROLOGUE_HEADER updated byte-identically in the SAME edit set and commit (`3f7aa1f`); drift test :4749 (containment) green across the commit boundary; containment re-verified mechanically post-commit.

## G6 cross-stage anchor re-grep

All S1-S5 designated anchors re-grepped against the new template, CORE_CONCEPTS_SECTION, and prologue header: zero hits. Em-dash: all three U+2014-free (the " - " in the preamble is a plain hyphen, as before).

## Measurements (harness; baseline @ ec660c17 vs head @ 3f7aa1f)

| item | base chars | head chars | delta | ~tok delta |
|---|---|---|---|---|
| profile: WG replica | 9105 | 9061 | -44 | -11 |
| profile: coordinator | 11431 | 11387 | -44 | -11 |
| profile: coordinator + auto_self_clear | 14059 | 14015 | -44 | -11 |
| profile: Root Agent | 14241* | 14241 | -44 | -11 |
| profile: Root Agent + auto_self_clear | 16913 -> 16869 | | -44 | -11 |
| supplements B1/B3 | unchanged | | 0 | 0 |

*Root base 14285 -> head 14241. The skeleton is shared: every profile moves by the same -44 (template -44; Root via prologue header -6 + CORE_CONCEPTS_SECTION -38).

Cumulative #1005 (from 08897ef, chars/4): WG replica 2688 -> 2265 (-423 tok, -15.7%); coordinator+auto 4001 -> 3503 (-498, -12.4%); Root+auto 5006 -> 4217 (-789, -15.8%).

## Deviations / flags

1. **Win below the ~30% plan target and the issue's ~60-80 tok/agent** (-44 chars = -11 tok/agent): the compressible surface is only ~262 of the skeleton's 387 prose chars once the harvested needles pin the title, the preamble prefix, the Team first clause (with period), and the heading; the two definitions were already near-minimal. Executed the plan-named compressions (preamble + both definitions) plus one declared needle rename to unlock the Workgroup clause; nothing else cut per STOP-and-flag. Same standard cause as S3/S4/S5 deviation 1.
2. **E2.2 replica goal**: final replica profile 2265 tok - the <= ~2,000 tok goal is NOT reached (2688 baseline, -423 achieved). The remaining mass is needle-dense RULE rows across A2/A3 (write restrictions 1010 tok, messaging 571 tok); reaching 2,000 would need row drops the consensus plan does not authorize. Stated per G12 for the completion report.
3. fmt posture unchanged from S5: my files clean (one joined line in `64023a4`); the 160 pre-existing diff blocks on base main remain out of scope.
