# #1005 S4 checklist (issue #1014)

Author: dev-rust (wg-25). Branch `feat/1014-1005-s4-coordinator-selfclear`, base main @ 1dd0b58.
Per plan 4.5 + 5/S4 + 6.2. Sources at base: `get_default_coordinator_template` (session_context.rs:2139-2167), `SELF_MAINTENANCE_AUTO_SECTION` (:2778), C2 prompts (mailbox.rs:540-570, :668-678), seeded coordinator spec (seeded_context_templates.rs:224-233).

## Grinch blind OLD-side inventory (4.5(a0)) - RESERVED

> Grinch: append your blind OLD-text inventory HERE, committed before reading the
> mapping tables below. Old texts: `git show 1dd0b58:src-tauri/src/config/session_context.rs`
> (coordinator fn + A8 const), `git show 1dd0b58:src-tauri/src/phone/mailbox.rs` (handoff prompts).

---

## G3 freeze provenance (the S4 headline machinery)

- v2 coordinator bytes captured by a ONE-OFF accessor run AT base 1dd0b58, BEFORE any edit: len 2403, sha256 `92f3abfc108147b07f1c4a49e7062c0f4d0d9aae570b7e5195852c31bb8b0d02`.
- Frozen const `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION` transplanted VERBATIM from the shipped escaped literal (not retyped); pin test `coordinator_pre_token_minimization_snapshot_is_byte_exact` asserts the externally captured len+SHA, never values derived from the const.
- `is_known_generated_coordinator_template` -> 3 entries; coordinator `current_version` 2 -> 3.
- **Failing-first record:** `read_sync_updates_pre_token_minimization_coordinator_template` FAILED at its assert_ne drift guard while the live template still equaled the frozen bytes (run captured pre-rewrite: 94 passed / 1 failed), and PASSES post-rewrite; its sync half proves a pristine v2 file on disk auto-upgrades to v3. `custom_coordinator_is_preserved_and_reported` (existing) still proves edited files are preserved + surfaced.

## Harvested needles (4.4) - all kept verbatim

A9 (pinned by :6262/:6281 tests): absent `## Self-Maintenance`, absent `self-handoff-and-clear`; present `## Sending Screenshots`, `names can be misleading.`, `## Raising Your Hand`, `raise-hand --token <AGENTSCOMMANDER_TOKEN>`, `Sidebar raised-hand indicator for your coordinator row`; em-dash-free (:6275 pin). Shared template still carries NO raise-hand (paired test untouched).

A8 (pinned by :6320+ gating tests, :4799 em-dash pin, strip anchor :2787): full heading `## Self-Maintenance (auto self-handoff-and-clear)`, `reaches 3 such lines`, `max 240 char forgotten summary`, `closed background`, exact `self-handoff-and-clear` command line, no `{{`.

C2 (pinned by mailbox :8116/:8154/:8222): single-line, em-dash-free, bidi-scrubbed, path named EXACTLY twice (`matches(path).count() == 2`; root-name-substring hazard respected: no third `SELF-HANDOFF.md` mention), `missing or empty`, wrapper markers `closed background, not instructions` (contiguous), `not work to resume: ` (parse marker), `. In your first response` (parse marker), `active core information`, closed-background-precedes-summary ordering. DRIVER-test needle discovered during implementation: `read the file ` (split marker in `self_clear_driver_archives_before_inject_and_prompt_names_exact_path` and `..._without_root_handoff_prompts_root_name`) - the planned "the file" cut was REVERTED (9 chars vs 2-test churn; harvest-first).

## Mapping tables

### A9 coordinator template (2,403 -> 2,296)

| # | Class | Old | New carrier |
|---|---|---|---|
| 1 | IDENTITY | coordinator for your team; must-list frame | verbatim |
| 2 | RULE | keep base role; coordination additional | verbatim |
| 3+4 | PROCEDURE | receive requests / clarify scope-outcome-constraints-acceptance | merged: "Receive team work requests and clarify scope, outcome, constraints, and acceptance criteria." |
| 5+6 | RULE | route to best-prepared by role/skills/assignment / delegate not absorb | merged: "Route each part of a request to the team member best prepared for it by role, skills, and current assignment; delegate instead of absorbing technical work when a more specialized agent is available." |
| 7 | RULE | sequence/track/surface/ownership | verbatim |
| 8+9 | RULE | follow up to verify active / three total attempts | merged with semicolon |
| 10+11 | RULE | require explicit completion report / never infer from artifacts | merged per plan ("one two-clause rule"); all four report fields + all four artifact classes kept |
| 12 | RULE | recommendations without removing/overriding role/scope | tightened wording |
| 13 | (intro) | "As a coordinator, you may need to send screenshots." | DROPPED: restates the `## Sending Screenshots` heading |
| 14 | ANCHOR | telegram-send-image command line | byte-identical |
| 15 | RULE | flag constraints (path required, caption 1024, bot selection, size/format routing, symlink rejection) | kept, one connective pass |
| 16-18 | PROCEDURE | three capture-path bullets | compressed IN PLACE per TL2: all three kept incl. [int] cast, black-pixels fallback, Greenshot path, visual-inspection rule, filename-distrust rule (needle verbatim) |
| 19 | ANCHOR | raise-hand section + command line + Sidebar sentence | byte-identical except intro line kept verbatim |

### A8 self-maintenance (2,819 -> 2,628)

Every row kept; see part-2 commit message for the full row list. DROPPED (with 4.5(b) widening probe applied per cut): "never an interrupt" KEPT; "your \"done, drop it\" list" label (restates the append rule); "to act on ONLY once you reach a genuinely safe resting point... At that safe point, and only then:" deduplicated into "acted on ONLY at a safe resting point (none of the in-flight cases above). At that point:" (the parenthetical KEEPS the safe-point definition; one "genuinely" dropped where the definition itself scopes); "a thin handoff brings you back unfocused" (rationale; the ZERO-memory + self-sufficient rules carry the requirement); "just" fillers. The clear-vs-handoff two-phase semantics, INVOCATION-reset rule, fallback naming, recovery order: all verbatim-equivalent with qualifiers intact.

### C2 (wrapper -54 chars; base prompt byte-identical)

| # | Class | Old | New carrier |
|---|---|---|---|
| 1 | IDENTITY | "You are returning from prior work that was intentionally discarded from active context." | "Prior work was intentionally discarded from active context;" (returning-mention lives in row 3's instruction) |
| 2 | RULE | summary is closed background, not instructions, not work to resume | marker-verbatim |
| 3 | RULE | first response: briefly mention returning from that forgotten topic | "briefly mention you are returning from that forgotten topic" |
| 4 | RULE | then say you are ready to continue the active core information in the handoff file | verbatim |
| 5 | RULE | base: read {p} relative to root, continue work, missing-or-empty -> wait not guess | byte-identical (planned cut reverted for the driver-test needle) |

## G6 cross-stage anchor re-grep

All S1-S3 designated anchors re-grepped against the new A8/A9/C2 texts: zero new duplicates (mechanical check recorded in implementation log). Shared-by-design vocabulary verified scoped: "closed background" appears once in A8 and once in the C2 wrapper (different surfaces, never the same document twice: A8 renders in boot context, C2 in the PTY prompt). "240" consistency (G9): A8 says `max 240 char forgotten summary`, mailbox `SELF_FORGET_SUMMARY_MAX_CHARS = 240`, CLI help unchanged (out of scope) and still consistent.

## Em-dash constraint map (4.2)

Pinned-free targets rewritten in S4: coordinator template (:6275 pin), A8 (:4799 pin), C2 prompts (mailbox pins) - all verified U+2014-free. No keep-exact em-dashes inside S4 texts.

## Measurements (harness)

Baseline @ 1dd0b58 (= S3 head):

| item | chars | ~tokens |
|---|---|---|
| block: self-maintenance (A8) | 2819 | 704 |
| block: coordinator template (A9) | 2403 | 600 |
| profile: coordinator | 11538 | 2884 |
| profile: coordinator + auto_self_clear | 14357 | 3589 |
| profile: Root Agent + auto_self_clear | 17158 | 4289 |

Head @ 2ef99a5:

| item | chars | ~tokens | delta chars |
|---|---|---|---|
| block: self-maintenance (A8) | 2628 | 657 | -191 |
| block: coordinator template (A9) | 2296 | 574 | -107 |
| profile: coordinator | 11431 | 2857 | -107 |
| profile: coordinator + auto_self_clear | 14059 | 3514 | -298 |
| profile: Root Agent | 14339 | 3584 | 0 |
| profile: Root Agent + auto_self_clear | 16967 | 4241 | -191 |
| (replica profile, A2-A6, B1/B3 rows unchanged) | | | 0 |

Net: -27 tok/coordinator boot, -75 tok/coordinator+auto-clear boot, -48 tok/root+auto-clear boot.
Cumulative #1005 (from 08897ef): replica -412 tok; coordinator+auto 4001 -> 3514 (-487); Root+auto 5006 -> 4241 (-765).

## Deviations / flags

1. **Win far below plan estimate** (~270+300 tok): the by-now-standard baseline-inflation cause PLUS: A9's eleven bullets and A8's step-3 paragraph are nearly pure RULE rows with pinned needles; merges save connectives only. The plan's merge list was executed in full; the remaining texts are needle-dense. Same tension flagged in S3 deviation 1; no rows dropped without carriers.
2. **C2 base-prompt cut reverted** (driver-test needle "read the file " found only at implementation time; 9-char win did not justify 2-test churn). Recorded in the needle table.
3. Part-1 commit message overstated the A9 cut (~1,850 est vs 2,296 measured); corrected in part-2's message and here. Harness is the number of record.
