# #1005 S3 checklist (issue #1012)

Author: dev-rust (wg-25). Branch `feat/1012-1005-s3-golden-rule`, base main @ 8342f7e.
Per plan 4.5 + 5/S3. Sources at base: `render_write_restrictions_block` (session_context.rs:2830), `default_context_dynamic_values` (:2911-3130), `ROOT_PROJECT_SCOPE_ENTRY` (:2759), `ROOT_PROJECT_SCOPE_ALLOWED` (:2763, DELETED), `ROOT_AUTHORITY_SECTION` (:2771), `root_agency_cache_guidance` (:3132).

## Grinch blind OLD-side inventory (4.5(a0)) - RESERVED

> Grinch: append your blind OLD-text inventory HERE, committed before reading the
> mapping tables below. Old texts: `git show 8342f7e:src-tauri/src/config/session_context.rs`
> lines 2759-2786 (root consts), 2830-2879 (shell), 2911-3130 (dynamic values), 3132-3146 (agency guidance).
> Row atomicity per 4.1: scoping qualifiers are their own rows.

---

## Harvested test needles (4.4) - 90 extracted, ~60 A2-specific

Full dump: 90 needles with owning tests were extracted mechanically from the test
module BEFORE any cut (harvest artifact). Kept verbatim in the rewrite: every
needle in these tests - `default_context_documents_agentscommander_cli_exception`
(6 needles), `entry_one_grants_workspace_root_listing_for_repo_discovery` (2),
`root_grant_renders_full_project_folder_write_scope` (7 of 8),
`root_grant_keeps_global_config_off_limits` (6), `root_authority_section_present_and_user_only`
(6), `root_git_scope_permits_project_repo_git_ops` (3), `root_read_scope_grants_settings_json_and_agency_cache`
(4), `default_context_root_agent_documents_agency_cache_cli_only` (4),
`default_context_matrix_section_lists_skills` (2 of 3), messaging-exception tests
(headers byte-exact incl. their em-dashes), `root_context_omits_peer_replica_prohibition_and_stale_quote`
(both), summary-line + FORBIDDEN needles, `Do NOT store canonical memory, plans, or skills here.`,
`any other files inside the Agent Matrix`, carve-out phrases.

SWAPPED needles (each = a deleted Allowed bullet whose rule moved carrier; plan-specified):

| Old needle | New carrier anchor | Tests |
|---|---|---|
| `- **Allowed (Root Agent)**: Full read/write across every project folder [registered in]` | `you may create, modify, and delete files anywhere under ANY project folder registered` (ROOT_PROJECT_SCOPE_ENTRY) | `root_grant_renders_full_project_folder_write_scope`, `stale_hybrid_root_keeps_authority_and_scope_grant`, `root_prologue_renders_every_mandatory_block_exactly_once` |
| `Allowed (Root Agent)` absence (x3, extinct phrase -> vacuous) | absence of the live grant anchor above, paired with root-side presence | `non_root_agent_has_no_root_grant_or_authority`, `root_never_...` x2 |
| `- **Allowed (narrow)**: Create canonical inter-agent message files` | `You MAY create message files inside this directory` (exception paragraph) | `default_context_replica_under_wg_includes_messaging_exception` |
| `- **Allowed (read-only)**: Read message files inside your workgroup messaging directory` | `You may also READ message files inside this directory, and list your workgroup root ...` (exception paragraph) | `read_bullet_carves_out_the_messaging_grant_in_every_mode` (rewritten per plan 6.1) |
| `` `memory/`, `plans/`, `skills/`, and `Role.md` `` (consolidated Allowed line) | entry-3 four-bullet list asserted in full | `default_context_matrix_section_lists_skills` |

New assertion added: root exception paragraph read sentence (`You may also READ message files inside this directory.`) in the carve-out test's Root arm.

## Em-dash constraint map (4.2)

- Pinned-free targets rewritten in S3: `ROOT_PROJECT_SCOPE_ENTRY`, `ROOT_AUTHORITY_SECTION` - both U+2014-free (pin test :4810 region passes; `ROOT_PROJECT_SCOPE_ALLOWED` line removed from that test with the const).
- Keep-exact em-dashes SURVIVING byte-identical: `Narrow exception — workgroup messaging directory` / `— Root Agent messaging directory` headers (carve-out referent anchors; the six G5 needle tests pass UNCHANGED because the headers were kept, resolving G5 without test churn), the `## GOLDEN RULE — Repository Access Restrictions` heading (prefix-pinned, descriptor kept), entry-2 `— your assigned root:` (#664 legacy extraction marker `assigned root:` kept), FORBIDDEN-bullet em-dashes (existing style, unpinned).
- Frozen legacy corpus untouched.

## Per-mode carrier matrix (the core S3 artifact)

Rule -> carrier, per render mode. Modes: WG replica (matrix+messaging), plain replica (no matrix, None messaging), Root. "entry N" = numbered grant entries.

| Rule / grant | WG replica | plain replica (None) | Root |
|---|---|---|---|
| repo-* read/write grant | entry 1 | entry 1 | entry 1 (waived by entry 3 clause) |
| workspace-root listing grant (names only) | entry 1 | entry 1 | entry 1 |
| replica-root read/write grant | entry 2 | entry 2 | entry 2 |
| replica-root usage limits + store-prohibition | entry 2 usage lines | same | same (no peer clause, D1) |
| peer-replica read/write prohibition | entry 2 usage line + FORBIDDEN write/read | same | NOT RENDERED (entry 3 grants it; "does not bind you" clause) |
| matrix four-dir grant | entry 3 bullet list (single carrier; Allowed line DELETED) | n/a | n/a |
| project-wide write grant | n/a | n/a | entry 3 = ROOT_PROJECT_SCOPE_ENTRY (single carrier; Allowed bullet DELETED) |
| messaging write grant + pattern + never-modify/delete + only-message-files | exception paragraph (single carrier; narrow bullet DELETED) | n/a (no directory; stated in A3 None variant) | root exception paragraph |
| messaging read grant | exception paragraph read sentence (separate sentence, G7b) | None-mode read bullet (KEPT between summary and FORBIDDEN) | root exception read sentence |
| wg-root listing grant (resolve path) | exception read sentence | n/a | n/a |
| inbound-file read grant | n/a | None bullet (sole carrier) | n/a |
| read carve-out phrase | "(other than the narrow messaging exception above)" | "(other than the inbound message file grant above)" | "(other than the narrow messaging exception above)" |
| git read-only allowance (`status/log/diff`) | git clarification final sentence (single carrier; Allowed-pair mention DELETED) | same | root git clarification final sentence |
| state-git prohibition + repo-cd rule | git clarification | same | root git variant (project-cd + repo-* waiver) |
| discovery ceiling | git clarification, stated ONCE | same | root git, stated ONCE |
| CLI exception (grant + boundary) | merged single paragraph | same | same |
| REFUSE rule | REFUSE line | same | same |
| settings.json / agency-cache read grants | n/a | n/a | root FORBIDDEN-read (single carrier; dropped from scope entry) |
| authority rules | n/a | n/a | ROOT_AUTHORITY_SECTION (7 rules kept) |
| agency-cache CLI-only management | n/a | n/a | agency guidance (path + 3 commands + 2 prohibitions) |

## Mapping notes per rewritten text (old row -> carrier; DROPPED rows named)

**Shell:** `allowed_places` fold (cosmetic). Static Allowed pair DROPPED: repo-* grant dup of entry 1; replica-root dup of entry 2; git-read mention dup of git clarification final sentence. `{matrix_allowed}`/`{root_scope_allowed}` placeholders removed; fine-grained `{{MATRIX_ALLOWED}}` hybrid token retires to "" (grant carrier = entry 3 inside `{{MATRIX_SECTION}}`).

**Entry #1:** second example DROPPED (illustrative); "These are the working repos you are meant to edit." DROPPED: restates the entry heading + the ONLY-read-or-modify frame. Listing needles verbatim.

**replica_usage:** "personal notes"/"role drafts" DROPPED from an illustrative example list; both prohibitions verbatim.

**matrix_allowed:** DELETED; carrier entry 3 (bullet list now asserted in full by the pivoted test).

**messaging_exception (WG/Root):** cross-reference sentence ("Used by the two-step protocol ... `send --send <filename>`") DROPPED: explanatory; the flow's carrier is the Inter-Agent Messaging section (A3). Read grants APPENDED as separate sentences so the write-scope framing ("Strictly limited ... Do NOT write any other kind of file here") cannot be read as narrowing reads (G7b). Carve-out referent noun "narrow messaging exception" intact (header byte-identical).

**messaging_allowed:** WG/Root arms now empty (bullets deleted); None arm byte-identical. `messaging_read_phrase` re-gated on the MODE ENUM in the same commit (plan 5/S3 gating fix); `workspace_root_phrase` untouched (G7: `has_messaging_exception` already enum-derived).

**forbidden_scope (non-root):** unchanged except unchanged... no cut (already minimal; every clause an exclusion class).

**forbidden_read (non-root):** three privacy sentences merged into one clause chain; all four verbs + message-alternative verbatim.

**Root forbidden write/read:** connective tissue cut; every exclusion class, both read grants, and all needles verbatim. settings.json-read grant now has its SINGLE carrier here (dropped from scope entry).

**ROOT_PROJECT_SCOPE_ENTRY (2,333 -> 1,782):** every rule kept as one clause incl. the G1 qualifier "always identified as the registered `settings.projectPaths` entry". DROPPED rows: "(you may edit source and run state-changing Git there)" (carrier: root git clarification); "reading that file to enumerate the current set is always allowed" (carrier: root FORBIDDEN-read); "(the portable directory next to the binary ...)" locator parenthetical (identity detail of the exclusion, kept as "app config directory itself (holding the global `settings.json` and the Agency template cache)"); "(as it does in dev and workgroup layouts)" (example); "non-root agents stay confined to `repo-*` working repos and their own replica directories" (describes OTHER agents' rules; their carrier is their own rendered document; the root-side sole-writer claim "You are the only agent permitted to write ..." KEPT); "as the user's task requires" (rationale); "Inside the `.ac` tree the Golden Rule does NOT confine you" reframed as the covers-list ("Inside each project it covers ... everything beneath, including other agents' canonical state").

**ROOT_AUTHORITY_SECTION (1,984 -> 1,713):** all seven rules kept; DROPPED: "This guardrail is deliberate." (rationale marker; the consequence clause itself kept); "(the app's prompt and dispatch interface)" (parenthetical restating "app UI"); "across many projects" tail merged.

**agency guidance:** "You may offer to manage" -> "Manage ... only through" (obligation equal-or-stronger); needles verbatim. G10 line: `legacy_rendered_default_context_for_compat` interpolates THIS live fn (:3281 at base); defused by the Root name gate (:2304-2309 guard + name-gated empty for non-root), so the rewrite needs no freeze; do not move those guards.

## Structure invariant check (plan 5/S3)

Rendered order verified by eyeball render + structure tests: numbered entries 1..2(..3) -> messaging exception -> summary OFF-LIMITS line -> (None: read bullet) -> FORBIDDEN write -> FORBIDDEN read -> git clarification -> CLI exception -> (root: agency guidance) -> REFUSE line -> (root) authority section. Item-"3." single-slot debug_assert untouched; "ABSOLUTE AND NON-NEGOTIABLE" verbatim; `{{AGENT_ROOT}}` token untouched; path code fences untouched; non-root render stays byte-identical across agents given identical inputs (all agent-specific content still in format placeholders).

## Measurements (harness)

Baseline @ 8342f7e (= S2 head):

| item | chars | ~tokens |
|---|---|---|
| block: write restrictions (A2, replica) | 5076 | 1269 |
| profile: WG replica | 10138 | 2534 |
| profile: coordinator (+auto) | 12571 / 15390 | 3142 / 3847 |
| profile: Root Agent (+auto) | 16656 / 19475 | 4164 / 4868 |

Head @ dee5caa:

| item | chars | ~tokens | delta chars |
|---|---|---|---|
| block: write restrictions (A2, replica) | 4043 | 1010 | -1033 |
| profile: WG replica | 9105 | 2276 | -1033 |
| profile: coordinator | 11538 | 2884 | -1033 |
| profile: coordinator + auto_self_clear | 14357 | 3589 | -1033 |
| profile: Root Agent | 14269 | 3567 | -2387 |
| profile: Root Agent + auto_self_clear | 17088 | 4272 | -2387 |
| (A3/A4/A5/A6/A8/A9/B1/B3 rows unchanged) | | | 0 |

Net: -258 tok per replica/coordinator boot; -597 tok per Root boot.
Cumulative #1005 so far (from 08897ef): replica 2688 -> 2276 (-412 tok), Root 4301 -> 3567 (-734 tok).

## Deviations / flags for tech-lead

1. **Char targets vs row preservation.** Plan targets said scope entry -> ~1,100 and authority -> ~1,100; I landed 1,782 and 1,713. Hitting ~1,100 requires DROPPING rows (e.g. the sole-writer clause, the covers-list detail, more authority qualifiers), which the same plan section forbids and section 1 subordinates to semantic preservation. Per the dispatch rule I did NOT improvise those cuts; flagging instead. If consensus wants deeper cuts, name the rows to drop.
2. **Replica win -258 tok vs plan's 450-550 band; Root -597 vs ~1,200.** Same baseline-inflation cause recorded in S1/S2, plus deviation 1.
3. Three vacuous absence assertions (extinct `Allowed (Root Agent)` phrase) pivoted to the live grant anchor; recorded in the needle table.
4. Test count unchanged (2300); no test deleted; one test (:4494) rewritten per plan, one absence set pivoted, five needle swaps.
