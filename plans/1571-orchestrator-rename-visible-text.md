# Plan #1571: Coordinator to Orchestrator, phase 1 (user-visible and agent-facing text, docs, website)

Author: ac-architect-v3, workgroup wg-13-ac-dev-team-v3. Full delivery: a pure text rename across two repositories, with two new frozen pre-rename template snapshots and two seeded-template version bumps so no existing installation loses its context-template auto-update. No behavior change, no identifier change, no serialized value change, no schema migration, no new dependency, no version bump.

Status: READY_FOR_IMPLEMENTATION

Revision: round 7 (2026-08-27), an amendment over round 6. Six prior digests are void: `8FCE2703...` (round 1), `081E5D06...` (round 2), `99B462F6...` (round 3), `D7EB697A...` (round 4), `9E717F65...` (round 5) and `5D4D128C...` (round 6). **Round 6 was certified READY_FOR_IMPLEMENTATION with three reviewer approvals and the user's Step 7.5 approval, and steps 0 to 7 were implemented and committed against it; nothing already committed is wrong.** Round 7 exists because step 8 found a defect with a silent-pass shape: five printed `clap` help lines that no enumeration named, so AC1 through AC9 all go green while the shipped CLI prints the retired word ten times. Round 7's five amendments are listed at the end of this paragraph; it moves no rule, no byte value, and no classification other than the six occurrences A1 and A2 name. Round 5 reached consensus three to zero with both of round 4's blockers closed; round 6 changes no rule, no criterion, no classification, no enumeration's substance and no byte value, and its ten corrections are listed at the end of this paragraph. Round 4 had failed consensus one to two, and both Rust reviewers found the same blocker independently: its rewrite of §5.2's RENAME half had dropped round 3's trailing clause "or a test expectation that pins one of those" and reinstated that coverage for one sub-case only, so every other §6.2 test expectation came back PRESERVE under Rule P0's own gloss of "resolves" as "code binds or reads it"; and the one documentation line preserved because a later phase owns its referent, `terminal-snapshots.md:168`, had no clause left to preserve it. Round 5 states both as named clauses of Rule P0, in §5.2 and in AC7 step 2: **R1**, a test expectation is never itself a referent, so the occurrence is classified by what the pinned literal names and the expectation moves under §9.3, which retires round 4's template-body carve-out instead of adding a second exception beside it; and **R2**, a token whose referent a later phase creates or renames in step with this text is preserved though nothing resolves it today. D12's reopen condition gains §5.6.2, derived instance 3's universal quantifier is repaired, and §5.2.1's flip class is recounted from 13 occurrences to 15 with `session_context.rs:3534` and `:3619` added; round 6 recounts it once more, to 20, against a criterion §5.2.1 now states explicitly. **Rule P0 is referential, not positional**: the discriminator is whether something resolves the token, and position is a cheap spot-check that does not decide. That correction was made at round 4 and is recorded in D12; every earlier description of Rule P0 as a position-based classification is superseded. Round 1's frozen-snapshot core has been byte-unchanged through all five rounds, reproduced digit for digit by two independent Rust-literal decoders, and is not re-derived. The per-round history of rounds 2 to 4 is in D12, §9.4 AC6, AC7 and §5.2.1; this header is not the authority for entry, which is §1.2's gate on `ecc6527b` / `85f318d3` and `git status --porcelain`. **Round 6's ten corrections, none of which moves an edit line, changes a classification or moves a byte:** §5.2.1's flip class is recounted from 15 to **20** against a membership criterion now stated in one sentence, the five added occurrences being `session_context.rs:2523`, `:2524`, `:3179`, `:3622` and `:5450`, the last of which enters the class only because §6.2 gains that line below; §5.2's own account of the class is brought into line with §5.2.1; §5.2.1's terminal reopen condition gains `§5.6.2`, which D12 and §14 item 3 already carried; a quotation at §5.2.1 attributed to round 4's plan text, where that text does not occur, is removed and its substance kept; §6.2's Rust set gains the two live pins `session_context.rs:5450` and `:5483`, and §5.2.1's Rust-tests row moves with them; §6.2's no-oracle note grows from one line to four, the other three being negative assertions; `default_context_dynamic_values` is corrected to `:3466` to `:3666`; a sentence duplicated inside §5.2's disagreement paragraph is dropped from its first occurrence; and `tests/builder-lab.spec.ts:282` is scoped to the website repository in §5.2 and in AC7 step 2, because §6.2 is `repo-AgentsCommander` only. **Round 7's five amendments, one blocker and four corrections.** **A1, the blocker.** `clap` derive compiles a `#[derive(Subcommand)]` variant doc comment into that subcommand's `about` string, so five `///` lines in `src-tauri/src/cli/mod.rs` (`:140`, `:158`, `:160`, `:163`, `:165`) are printed help, twice each: on the subcommand's own `--help` and in the top-level `help` listing. §3.3 (e) excluded "every source comment and doc comment" without qualification and swallowed all five, putting (e) in direct contradiction with §12 step 8's gate and making that gate unreachable. Rule P0 classifies all five RENAME, because nothing resolves the role noun in any of them. §3.3 (a) gains the five sites and the measured bound on the class, §3.3 (e) gains a carve-out for doc comments `clap` consumes as printed help, §5.4 gains their exact after-text, §6.1 gains the file, §5.2.1's Rust production row moves from 80 / 88 to **85 / 93**, and AC1 gains needles **34 to 38**. **A2, a scope ruling.** `docs/reference/architecture.md:731` moves from RENAME to PRESERVE and §5.6.2 becomes **59** lines: the line describes Concept B's `session/selection.rs`, which Phase 2 renames to `SelectionArbiter` and not to Orchestrator, so renaming it here would ship documentation calling a type an orchestrator that never becomes one. Recorded as **D13**, residual **R12**, and it makes `terminal-snapshots.md:168` no longer the only occupant of Rule P0 clause R2. **A3, arithmetic.** §5.2.1 step 1 derived the documentation edit-line set as the 296 in-scope matching lines minus §5.6.2, which undercounts, because **17** preserve-list lines carry a renamed occurrence and do enter the diff; §5.6.1 already makes the preserve decision per occurrence and not per line, so the edits were always right and only the arithmetic was wrong. With A2 the figure is **254**, and all 17 are now named, including the six that carry exactly one T1 occurrence and so fell outside the "more than one occurrence" framing every earlier round used. **A4, anchor integrity.** §5.6.1's check needled `#coordinator` and structurally could not match the repository's one token-carrying anchor, `terminal-snapshots.md:433` into `container-coding-agents.md:32`; the needle is widened to `#[a-z0-9-]*coordinator` over every tracked file and the binding rule that a link and its target heading move in the same phase is stated, so that outcome is by construction instead of by luck. **A5, three quotations** at §5.6.1 and §5.6.2 carried a hyphen where the tree carries U+2014, which matters only because §1.1 tells an implementer to re-anchor on quoted text when a line number drifts; they are corrected to the source bytes, and that is the only place in this plan where U+2014 belongs.

Issue: [mblua/AgentsCommander#1571](https://github.com/mblua/AgentsCommander/issues/1571), "Coordinator to Orchestrator, phase 1: user-visible and agent-facing text" (OPEN). Parent epic: [#1570](https://github.com/mblua/AgentsCommander/issues/1570) (OPEN), phase 1 of 4. Phases 2 (#1572), 3 (#1573) and 4 (#1574) are out of scope.

Objective: every surface a human or an agent reads calls the role Orchestrator instead of Coordinator, while every serialized identifier, file name, JSON key, event name, IPC command name, CLI flag, testid, reason code and frozen historical byte sequence stays exactly as it is on disk today.

---

## 1. Frozen authority and entry gate

### 1.1 Frozen base

| Fact | Value |
| --- | --- |
| Primary repo | `repo-AgentsCommander` at `D:\0_repos\AgentsCommander_iac\.ac\wg-13-ac-dev-team-v3\repo-AgentsCommander` |
| Branch (already created, do not recreate) | `refactor/1571-orchestrator-rename-visible-text` |
| Frozen base | `main` == `origin/main` == branch head == `ecc6527b948fd4cc047f3636a3c9b9b1d88a677e` |
| Working tree at authoring time | `git status --porcelain` empty |
| Secondary repo | `repo-agentscommander_webpage` at `D:\0_repos\AgentsCommander_iac\.ac\wg-13-ac-dev-team-v3\repo-agentscommander_webpage` |
| Secondary frozen base | `main` == `origin/main` == `85f318d324075413b53428aa7f1a8df519446f82`, tree clean |
| Delivery path | Full |
| Accepted task class | Routine application-text change (see §13.1) |

Codebase Memory gate `ready` for both repositories at authoring time (2026-08-27 UTC):

- project `D-0_repos-AgentsCommander_iac-.ac-wg-13-ac-dev-team-v3-repo-AgentsCommander`, 25,147 nodes / 140,398 edges, `head_sha` `ecc6527b948fd4cc047f3636a3c9b9b1d88a677e`;
- project `D-0_repos-AgentsCommander_iac-.ac-wg-13-ac-dev-team-v3-repo-agentscommander_webpage`, 307 nodes / 541 edges, `head_sha` `85f318d324075413b53428aa7f1a8df519446f82`.

Every line number in this plan is at those two SHAs. Byte-level evidence (§3.6) was taken from `git show <sha>:<path>`, never from the working tree, because `core.autocrlf` is on and a worktree digest is not reproducible. If a line number no longer matches the quoted text, re-anchor on the quoted text, never on the number.

### 1.2 Entry ritual for the implementer

Before the first edit, and inside `repo-AgentsCommander`:

1. `git -C <repo> fetch origin main`.
2. `git -C <repo> rev-parse HEAD origin/main` and `git -C <repo> merge-base HEAD origin/main`. All three must equal `ecc6527b948fd4cc047f3636a3c9b9b1d88a677e`. If any differs, STOP and request a current-base review; do not rebase silently.
3. `git -C <repo> status --porcelain` must be empty.
4. Repeat 1 to 3 in `repo-agentscommander_webpage` against `85f318d324075413b53428aa7f1a8df519446f82`.

Root `.gitignore` line 11 ignores `/plans/`, while the `plans/*.md` files themselves are tracked. This plan file is added with `git add -f plans/1571-orchestrator-rename-visible-text.md`; that is the only `-f` this plan authorizes, the ignore rule is never widened, and `git add` under `plans/` can exit 1 while still staging, so trust `git diff --cached --name-only`, not the exit code.

---

## 2. Issue and objective

The product term Coordinator becomes Orchestrator. Phase 1 delivers the whole perceived rename with zero risk to stored data and zero risk to the IPC contract: it changes only text a person or an agent reads.

Required outcomes:

- **(A) Desktop UI.** No string rendered by `src/` (element text, `title`, `aria-label`, `placeholder`, `<option>` label, validation message, toast, filter token) contains `coordinator` in any capitalization.
- **(B) Agent-facing context.** The seeded coordinator context template, the `# Coordinator Context` heading AgentsCommander writes above it, the two privileged-PTY-input context blocks, the Root Agent context template, the Root authority section, the messaging blocks of the generated `CLAUDE.md`, and the injected-message file doc comment all say Orchestrator.
- **(C) CLI and IPC message text.** Every `clap` `after_help` / `long_about` block, `println!` / `eprintln!` line, `Err(...)` string and `format!` message that reaches a user or an agent says Orchestrator. Machine-readable reason codes do not change.
- **(D) No installation regresses.** An existing user whose `.ac/Context.coordinator.md` or Root `Role.md` / `Context.root-agent.md` is byte-identical to the shipped pre-rename default keeps being recognized as pristine, and therefore keeps auto-updating, after this change.
- **(E) Documentation.** Documentation prose says Orchestrator; documentation that names an on-disk identifier keeps naming the current on-disk value.
- **(F) Website.** The marketing site renders Orchestrator in English and orquestador in Spanish, with no `coordinator` or `coordinador` copy left, in every locale it ships.
- **(G) Nothing else moves.** No file rename, no identifier rename, no `data-ac-testid` change, no JSON key change, no event name change, no CLI flag change, no behavior change, no dependency-graph change.

The one hard correctness constraint is (D). Three frozen historical constants record what previous releases wrote to a user's `.ac/Context.coordinator.md`, and `is_known_generated_coordinator_template` compares the user's file against them plus the current default to decide whether the user customized it. Their bytes must not move, and because this phase changes the current default, the current default's pre-rename bytes must be added to that set. The same mechanism exists a second time for the Root Agent context template and is affected the same way (§3.4); Phase 1 therefore adds two frozen snapshots, not one.
---

## 3. Evidence (measured at `ecc6527b` / `85f318d3`, not predicted)

### 3.1 Baseline, and three corrections to the numbers in the issues

`git grep -l -i coordinator` in `repo-AgentsCommander` returns 228 tracked files, `git grep -n -i coordinator` returns 4272 lines. That total matches the epic. The per-area split measured now is:

| Area | Files | Note |
| --- | --- | --- |
| `src/` | 93 | epic says 92 |
| `src-tauri/` | 80 | as in the epic (70 `src-tauri/src` + 9 `src-tauri/tests` + 1 `src-tauri/src/api/README.md`) |
| `docs/` | 33 | |
| `crates/` | 5 | **the epic's area table has no row for this**; the 228 total is still correct |
| `plans/` | 10 | epic says 12 |
| `scripts/` | 3 | |
| `README.md`, `ROADMAP.md`, `PRIVACY.md`, `CHANGELOG.md` | 4 | |

Three corrections that matter for this plan:

1. **`crates/` was never listed.** `crates/session-bridge/src/bin/agentscommander-api-helper.rs:681` and `:684` hold a second copy of the PTY-input reason detail strings that `src-tauri/src/phone/types.rs:203` and `:206` hold. They are the container-side helper's answer to an agent and are in scope; the epic's area table simply omits the crate.
2. **Documentation in scope is 36 files / 296 lines, not 37 / 306.** The issue's 37/306 includes `CHANGELOG.md` (10 lines), which the same issue then puts out of scope. `git grep -c -i coordinator -- "*.md" ":(exclude)plans/" ":(exclude)CHANGELOG.md" ":(exclude)src-tauri/" ":(exclude)src/"` returns 36 files summing to 296 lines.
3. **The website is 47 lines / 14 files, not 41.** The issue counted `coordinator` only, which finds 41. Widening to `coordinator|coordinador` finds 43. Both undercount, because four rendered locale values use a translated form that matches neither needle. The derivation that actually holds is the multilingual sweep `git grep -n -i -E "coordin|coorden|koordin|协调"`, which returns 80 lines; subtracting the Rule-T3 words (`coordination`, `coordinate`, `coordina`, `Coordinación`) and the `Coordination*` component identifiers leaves **47 lines in 14 files**, reproduced exactly by the closed needle set `git grep -n -i -E "coordinator|coordinador|coordenador|coordinateur|koordinator|协调者"` (47 lines, 14 files, measured at `85f318d3`). `src/i18n/landing.ts` carries the term in **six** locales (en, es, pt, fr, de, zh) under **two** keys, not one (§3.7).

Of the 887 `src/` lines that match, only **40 lines in 12 files** are visible copy (§3.2). Everything else in `src/` is an identifier, a CSS class, a comment or test fixture data, and is Phase 2 or Phase 4 work.

### 3.2 Frontend visible copy: the complete inventory (12 files, 40 lines)

| File | Line | Current text (verbatim) |
| --- | --- | --- |
| `src/sidebar/components/AcDiscoveryPanel.tsx` | 237 | `<span class="ac-discovery-badge coord">C</span>` (agents list) |
| `src/sidebar/components/AcDiscoveryPanel.tsx` | 290 | `<span class="ac-discovery-badge coord">C</span>` (replica list) |
| `src/sidebar/components/ActionBar.tsx` | 299 | `title={sessionsStore.coordSortByActivity ? "Show recent coordinators first" : "Show coordinators in default order"}` |
| `src/sidebar/components/EditLoopModal.tsx` | 228 | `<label class="new-agent-label">Workgroup Coordinator</label>` |
| `src/sidebar/components/EditLoopModal.tsx` | 236 | `<option value="" disabled>Select a coordinator...</option>` |
| `src/sidebar/components/EditLoopModal.tsx` | 242 | `<div class="new-agent-error">A workgroup with a verified coordinator is required.</div>` |
| `src/sidebar/components/EditLoopModal.tsx` | 282 | `Force inject even if coordinator is busy` |
| `src/sidebar/components/EditTeamModal.tsx` | 374 | `<label class="wizard-coord-label" title="Set as coordinator">` |
| `src/sidebar/components/NewLoopModal.tsx` | 171 | `<label class="new-agent-label">Workgroup Coordinator</label>` |
| `src/sidebar/components/NewLoopModal.tsx` | 179 | `<option value="" disabled>Select a coordinator...</option>` |
| `src/sidebar/components/NewLoopModal.tsx` | 185 | `<div class="new-agent-error">A workgroup with a verified coordinator is required.</div>` |
| `src/sidebar/components/NewLoopModal.tsx` | 195 | `placeholder="Prompt to inject into the coordinator"` |
| `src/sidebar/components/NewLoopModal.tsx` | 220 | `Force inject even if coordinator is busy` |
| `src/sidebar/components/NewTeamModal.tsx` | 321 | `<label class="wizard-coord-label" title="Set as coordinator">` |
| `src/sidebar/components/ProjectPanel.tsx` | 975 | `replica.isCoordinator ? "coordinator" : null,` (filter-text token) |
| `src/sidebar/components/ProjectPanel.tsx` | 1016 | `agentName === team.coordinator ? "coordinator" : null` (filter-text token) |
| `src/sidebar/components/ProjectPanel.tsx` | 2197 | `"Time this team has been idle. Resets when you message the coordinator or any member is active (persists across restarts)."` |
| `src/sidebar/components/ProjectPanel.tsx` | 2368 | `title="This team's coordinator was closed manually. Reopen it to clear."` |
| `src/sidebar/components/ProjectPanel.tsx` | 2426 | `<span class="ac-discovery-badge coord">coordinator</span>` |
| `src/sidebar/components/ProjectPanel.tsx` | 2670 | `<span class="ac-wg-name">Coordinators</span>` |
| `src/sidebar/components/ProjectPanel.tsx` | 3260 | `<span class="ac-discovery-badge coord">coordinator</span>` |
| `src/sidebar/components/ProjectPanel.tsx` | 4201 | `<span class="agent-modal-title">Close coordinator?</span>` |
| `src/sidebar/components/SettingsModal.tsx` | 835 | `replica.isCoordinator ? " (coordinator)" : ""` |
| `src/sidebar/components/SettingsModal.tsx` | 1659 | `"Coordinator idle: badge yellow threshold must be a whole number of at least 1 minute"` |
| `src/sidebar/components/SettingsModal.tsx` | 1662 | `"Coordinator idle: badge red threshold must be a whole number of at least 1 minute"` |
| `src/sidebar/components/SettingsModal.tsx` | 1665 | `"Coordinator idle: yellow threshold must be below the red threshold"` |
| `src/sidebar/components/SettingsModal.tsx` | 1671 | `"Coordinator idle: auto-close minutes must be a whole number of at least 1"` |
| `src/sidebar/components/SettingsModal.tsx` | 1861 | `<span>On start, wake coordinators that were awake when the app closed</span>` |
| `src/sidebar/components/SettingsModal.tsx` | 1899 | `<div class="settings-section-title">Coordinator idle</div>` |
| `src/sidebar/components/SettingsModal.tsx` | 1984 | `The idle badge shows minutes since your last message to a coordinator` |
| `src/sidebar/components/SettingsModal.tsx` | 1987 | `configured minutes; the coordinator stays as a dormant row you can` |
| `src/sidebar/components/SettingsModal.tsx` | 2000 | `<span>Always close team members when manually closing Coordinator</span>` |
| `src/sidebar/components/SettingsModal.tsx` | 2026 | `coordinators and Root; other agents opt in per agent)` |
| `src/sidebar/components/SettingsModal.tsx` | 2089 | `Allow authorized Root Agents and same-workgroup Coordinators to capture live terminal` |
| `src/sidebar/components/TeamContextAlertsEditor.tsx` | 79 | `percentage, AgentsCommander sends that workgroup&apos;s coordinator an informational notice` |
| `src/sidebar/components/WorkgroupGroupRail.tsx` | 254 | `title="A coordinator raised its hand"` |
| `src/sidebar/components/WorkgroupGroupRail.tsx` | 255 | `aria-label="A coordinator raised its hand"` |
| `src/sidebar/loop-event-toast.ts` | 26 | ``message: data.message ?? `Loop "${name}" skipped because the coordinator is busy`,`` |
| `src/sidebar/loop-event-toast.ts` | 32 | ``message: data.message ?? `Loop "${name}" is pending until the coordinator is idle`,`` |
| `src/sidebar/stores/coordinator-close.ts` | 62 | `name: s?.name ?? "this coordinator",` |

Two files the issue names as owners carry **no** visible copy and are therefore not edited by this plan: `SessionItem.tsx` (one import identifier at `:9`, one code comment at `:364`) and `AcDiscoveryPanel.tsx` beyond the two `C` badges (`:42` comment, `:44`/`:221`/`:289`/`:292` identifiers). Verified by reading every `coordinator` line in both files.

One `src/` error string is deliberately **not** renamed: `src/shared/ipc.ts:1039`, `"Invalid get_team_config response: coordinator must be a string"`. It names the `coordinator` JSON key of `get_team_config`, which Phase 3 owns.

### 3.3 Rust and crate text that reaches a user or an agent

Measured by scanning every production line (before each file's `#[cfg(test)] mod ...` boundary) of every `src-tauri/src` and `crates` file that matches `coordinator`, excluding the four Concept-B files (`session/selection.rs`, `commands/resource_monitor.rs`, `resource_monitor/watchdog.rs`, `commands/window.rs`), then classifying each hit. The in-scope result, grouped by sink:

**(a) `clap` help, about and long-about text (CLI, printed on `--help`).**
`cli/close_session.rs:112`; `cli/list_peers.rs:41`, `:42`, `:43`, `:104`, `:105`, `:108`, `:109`, `:127`; `cli/mod.rs:140`, `:158`, `:160`, `:163`, `:165`; `cli/purge_wg.rs:80`; `cli/raise_hand.rs:12`; `cli/send.rs:22`, `:29`, `:30`, `:31`; `cli/task_append_body.rs:19`; `cli/task_set_title.rs:19`, `:26`.

**The five `cli/mod.rs` sites are `///` doc comments and are printed help all the same**, which is why they belong here and not under (e). They are variant doc comments on `#[derive(Subcommand)] pub enum Commands`, and `clap` derive compiles a variant doc comment into that subcommand's `about` string, so each prints twice: once in the subcommand's own `--help` and once in the top-level `help` listing. An `about` written as a doc comment and an `about` written as an attribute are the same printed string, which is the distinction (e) as written at round 6 did not draw. Added at round 7, after step 8 ran all eight subcommands' `--help` against a built binary and read the retired word ten times. Their exact after-text is in §5.4 and AC1 needles 34 to 38 pin them.

**The bound on that class, measured rather than asserted.** Every `///` doc comment carrying a Rule-T token in a file that has any `clap` derive, plus every `about`, `help`, `long_about` and `long_help` attribute value, was swept across `src-tauri/src` and `crates`: the printed set is exactly these five, and no attribute anywhere in either tree carries the token. `crates/` has no `clap` dependency at all, so nothing there can be printed help. The 11 remaining doc-comment hits under `src-tauri/src/cli` are `list_peers.rs:558`, `:559` and `:1041` and `list_sessions.rs:51`, `:53` and `:57`, all attached to plain `fn` items, and `task_ops.rs:24`, `:38`, `:42`, `:69` and `:139`, whose file has no `clap` derive at all (its types derive only `Debug`, `Clone`, `Copy` and `thiserror::Error`). None of the 11 is printed and all 11 stay under (e).

**(b) CLI stdout / stderr text.**
`cli/close_session.rs:214`, `:215` (`eprintln!`, twinned with the `log::error!` at `:209`); `cli/list_peers.rs:654` (`eprintln!`, twinned with the `log::warn!` at `:659`); `cli/send.rs:898`, `:911`, `:924`; `cli/task_append_body.rs:97`, `:98`; `cli/task_set_title.rs:94`, `:95`.

**(c) `Err(...)` / `format!` messages returned to the UI, the CLI, the mailbox or the API plane.**
`cli/loop_cmd.rs:326`; `cli/send.rs:143`, `:1278`; `cli/team.rs:235`, `:246`; `cli/workgroup.rs:194`; `commands/entity_creation.rs:1034`, `:1164`, `:1167`, `:2315`; `config/loops.rs:476`; `loops/delivery.rs:129`, `:140`, `:483`; `phone/mailbox.rs:668`, `:679`, `:7500`, `:7508`, `:7545`, `:7562`, `:7579`, `:7657`, `:7689`, `:7695`, `:7824`, `:7840`, `:7860`, `:7867`, `:7876`, `:7879`, `:7898`, `:7907`, `:7918`, `:7924`, `:8007`, `:8028`, `:9149`, `:9457`; `phone/types.rs:203`, `:206`; `pty/inject.rs:279`; `session/context_alerts.rs:1321`, `:1612`, `:1619`, `:1625`, `:1627`, `:1635`, `:1715`; `crates/session-bridge/src/bin/agentscommander-api-helper.rs:681`, `:684`.

**(d) Text written into an agent's context or into a user-editable template file.**
`config/session_context.rs:2166` (the `# Coordinator Context` heading), `:2495` to `:2506` (`PTY_INPUT_COORDINATOR_CONTEXT`), `:2509` to `:2529` (`get_default_coordinator_template`), `:3175` to `:3181` (`ROOT_PTY_INPUT_CONTEXT`), `:3310` to `:3312` (`DEFAULT_DELEGATED_TASK_REPORTING`), `:3332` (`ROOT_AUTHORITY_SECTION`), `:3534`, `:3607`, `:3613`, `:3619`, `:3622` (inside `default_context_dynamic_values`); `config/root_agent.rs:615` to `:663` (`ROOT_ROLE_MD`, which `default_root_context_template()` returns); `config/injected_messages.rs:74` (`CONTEXT_ALERT_DOC_COMMENT`); `config/seeded_context_templates.rs:460` (`label: "Coordinator context"`, the label the Context-template-update modal renders).

**(e) Deliberately not renamed in Phase 1**, with the reason recorded in §10:
every `log::*` payload that has no user-facing twin (for example the `[coordinator-clocks]`, `[auto-close]`, `[loops]`, `[mailbox]`, `[ac-discovery]` and `[session]` lines); every source comment, and every doc comment **except one `clap` derive consumes as printed help** (carve-out below); every identifier; the `.gitignore` marker comments at `config/instance_artifacts.rs:299` and `:305`; every test fixture, test name and `expect(...)` panic message except where it pins a renamed literal (§9.3); `scripts/*.ps1`.

**The doc-comment carve-out, stated so Phase 2 does not reopen the hole.** A `///` comment is out of scope when the only things that read it are a human reading the source and `rustdoc`. It is **in** scope when `clap` derive compiles it into user-visible help text, which happens in three places: on a `#[derive(Subcommand)]` variant, where it becomes that subcommand's `about`; on a `#[derive(Parser)]` or `#[derive(Args)]` struct, where it becomes that command's `about`; and on a field of such a struct, where it becomes that argument's `help`. The discriminator is not the comment syntax but whether `clap` prints it, and it is the same discriminator §3.3 already applies to `after_help` and `long_about`. Only the five `cli/mod.rs` sites listed in (a) meet it today, and the sweep that establishes that is stated there. A reviewer checking (e) should read it as "not printed", never as "not a doc comment".

### 3.4 The frozen-recognizer audit: four mechanisms, two of them affected

This is the load-bearing part of the plan. A "recognizer" here is a function that decides whether a file on the user's disk is still an unmodified copy of something AgentsCommander shipped. Each one compares against a set of byte sequences; editing any member of that set silently reclassifies every existing user's file.

| # | Recognizer | Compares against | Does Phase 1 change a member? |
| --- | --- | --- | --- |
| 1 | `config/seeded_context_templates.rs:529-534` `is_known_generated_coordinator_template` (the `fn` spans `:529-534`; the four arms are `:530-533` and the fourth, last arm is `:533`) | `get_default_coordinator_template()` (current) plus `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE` (`:251`), `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION` (`:213`), `OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND` (`:80`) | **YES**, the current default. A new frozen snapshot is mandatory. |
| 2 | `config/root_agent.rs:669-681` `is_known_generated_root_context_template` (**seven** entries, array elements `:672-678`), and the **overlapping but not identical** six-entry list inside `migrate_root_role` at `:984-989` | Both hold `ROOT_ROLE_MD` (`:615`, the current default returned by `default_root_context_template()`) plus `OLD_ROOT_ROLE_MD`, `ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD`, `ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD`, `ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD`, `ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD`. The recognizer additionally holds `OLD_ROOT_CONTEXT_WITH_COORDINATION_MD`; `migrate_root_role` deliberately omits it and handles that body through the `OLD_DEFERRED_MESSAGING_PARAGRAPH` replacement branch at `:996-1000` instead | **YES**, the current default. A second frozen snapshot is mandatory, wired into **both** lists. |
| 3 | `config/seeded_context_templates.rs:503-527` `is_known_generated_global_template` / `is_known_generated_standalone_global_template` | `get_default_agent_template()` plus four `GLOBAL_*` / `STANDALONE_*` snapshots | No. None of those byte sequences contains `coordinator`; the shared agent template does not name the role. |
| 4 | `config/session_context.rs:3732-4053` `legacy_rendered_default_context_for_compat` / `pre_1072_legacy_rendered_default_context_for_generation` / `classify_legacy_rendered_default_context`, and `extract_legacy_skills_section` (`:4164-4170`) | Reconstructions of what older releases rendered into `CLAUDE.md` | No, **and they must not be touched**. Their `coordinator` lines (`:3815`, `:3858`, `:3863`, `:3869`, `:3872`, `:3875`, `:3931`, `:4165`) are historical bytes, not current output. |

Two further byte-pinned mechanisms were checked and are unaffected:

- `config/injected_messages.rs:85` `known_default_sha256` pins `DEFAULT_CONTEXT_ALERT_TEMPLATE` (`:52`), which contains no `coordinator`. The `doc_comment` (`:73-80`) is regenerated into the file and is not hashed, so renaming it is safe.
- `config/seed_manifest.rs:1270` maps `".ac/Context.coordinator.md"` to the scope string `"context:coordinator"`. Both are serialized values and both stay.

One constant is affected but is deliberately **not** renamed, because renaming it would break recognizer 2: `config/root_agent.rs:292` `ROOT_COORDINATION_MESSAGING_PARAGRAPH`. It is interpolated into the frozen `OLD_ROOT_CONTEXT_WITH_COORDINATION_MD` (`:340-374`, used at `:371`) **and** used as live replacement text by `migrate_root_role` at `:999`. Editing it moves frozen bytes; leaving it means one legacy migration path still writes coordinator-era prose. Separating the two uses requires splitting the constant, which is Phase 4 work (§10, R3).

### 3.5 The `current_version` convention

`SeededContextTemplateSpec.current_version` is persisted into the seeded-template state file as `templates.<id>.currentVersion` (`seeded_context_templates.rs:392`, `:879`) and is reported to the UI as `currentDefaultVersion`. No sync decision branches on it; publication decisions are made on sha256 comparisons. The repository convention is that a rewrite of a current template bumps it, and the bump is pinned by a test:

- `seeded_context_templates.rs:3239-3242`, "coordinator current_version must be bumped to 4 by the v4 rewrite", with `coordinator.current_version` also asserted at `:2039`;
- `root_agent.rs:2339-2342` and `:2387-2390`, "root_spec current_version must be bumped to 6 by the #1370 workgroup-activation rewrite".

Current values: `project_specs()` coordinator `current_version: 4` (`:461`), global `4` (`:451`), `root_spec()` `6` (`:475`).
### 3.6 Byte evidence for the two frozen snapshots (computed, not assumed)

`core.autocrlf` is on, so a working-tree digest of a `.rs` file is not reproducible. Every value below was computed from the **blob** (`git show ecc6527b:<path>`, always LF) by decoding the Rust literal exactly as `rustc` does: for a normal literal, `\n`, `\"`, `\\` and the backslash-newline continuation that swallows the newline and all following leading whitespace; for a raw `r#"..."#` literal, the bytes verbatim (rustc normalizes CRLF to LF inside string literals, which is why the raw-literal control below reproduces its published length from an LF blob).

The decoder was validated first against constants whose expected `len()` and sha256 are already published in the repository, and it reproduced all three exactly:

| Control constant | Published expectation | Decoder result |
| --- | --- | --- |
| `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION` (`seeded_context_templates.rs:213`, test at `:2210-2221`) | len 2403, sha256 `92f3abfc108147b07f1c4a49e7062c0f4d0d9aae570b7e5195852c31bb8b0d02` | len 2403, sha256 `92f3abfc108147b07f1c4a49e7062c0f4d0d9aae570b7e5195852c31bb8b0d02` |
| `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE` (`:251`, test at `:2227-2238`) | len 2296, sha256 `9f72fa83ac2fafc73565f975a2bec936a09d0e6a410b1ee1a4a13952e694ec84` | len 2296, sha256 `9f72fa83ac2fafc73565f975a2bec936a09d0e6a410b1ee1a4a13952e694ec84` |
| `GLOBAL_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION` (`:113`, provenance comment at `:104-112`) | len 611, sha256 `c9de5b80ad99a5743ad20c3344e7dd03888792f4da175943bee72e3d7d91fb88` | len 611, sha256 `c9de5b80ad99a5743ad20c3344e7dd03888792f4da175943bee72e3d7d91fb88` |

With the decoder proven, the two subjects measure:

| Subject at `ecc6527b` | `len()` | sha256 |
| --- | --- | --- |
| `config/session_context.rs` `get_default_coordinator_template()` (`:2508-2530`) | **2509** | **`f6ef7894b9f0f606e945c282d769144e96487fcc01ab435c9aab8019bb3ce1f6`** |
| `config/root_agent.rs` `ROOT_ROLE_MD` (`:615-663`), returned by `default_root_context_template()` | **2464** | **`e244249ccd2fa832918d1f830939ca4dae37a3594461334fa6d7004ba430e74f`** |

For the record, the decoded coordinator template starts `"You are the coordinator for your team. You must:\n- Keep your base role; coordination is an additional assignment, not a "` and ends `" shows the Sidebar raised-hand indicator for your coordinator row; it clears when the user interacts with your session.\n"`. The decoded `ROOT_ROLE_MD` starts `"---\nname: 'agents-commander'\ndescription: 'Static supplemental root context for AgentsComm"` and ends `" data (never invented), the bounded skip exceptions, and the `agency-templates` CLI flow.\n"`.

Root comparisons run through `normalize_role_text` (`root_agent.rs:1897-1899`), which is `text.replace("\r\n", "\n").trim()`. The `len()`/sha256 above are of the constant itself, which is what the new test pins, exactly as `root_agent.rs`'s existing frozen constants are used.

### 3.7 Website inventory (14 files, 47 lines, six locales, two keys)

Derivation: the broad sweep `git grep -n -i -E "coordin|coorden|koordin|协调"` at `85f318d3` returns 80 lines; every line it returns that is not below is a Rule-T3 word (`coordination`, `coordinate`, `coordina`, `Coordinación`) or a `Coordination*` component identifier. The closed needle set `git grep -n -i -E "coordinator|coordinador|coordenador|coordinateur|koordinator|协调者"` reproduces exactly the in-scope set: **47 lines across 14 files**, of which `src/i18n/landing.ts` holds **12**.

`src/i18n/landing.ts` carries the term in six locale tables under **two** keys. Round 1 listed only the `composer.coordinator` half plus the two English and Spanish `workspace.checkout.role` rows; the four remaining `workspace.checkout.role` values are the round-2 addition, and they are rendered copy, not keys (`src/components/alternatives/WorkspaceMock.astro:68-69` renders `copy["workspace.checkout.role"]`). They were invisible to round 1 because the round-1 derivation grep was `coordinator|coordinador`, which none of `coordenador`, `coordinateur`, `Koordinator`, `协调者` matches; the four `composer.coordinator` rows in the same locales landed only because that key's own name sits on the same line as its value.

| Line | Key | Current value |
| --- | --- | --- |
| 55 | `workspace.checkout.role` (en) | `tech lead · coordinator` |
| 71 | `composer.coordinator` (en) | `COORDINATOR` |
| 160 | `workspace.checkout.role` (es) | `tech lead · coordinador` |
| 176 | `composer.coordinator` (es) | `COORDINADOR` |
| 262 | `workspace.checkout.role` (pt) | `tech lead · coordenador` |
| 278 | `composer.coordinator` (pt) | `COORDENADOR` |
| 365 | `workspace.checkout.role` (fr) | `tech lead · coordinateur` |
| 381 | `composer.coordinator` (fr) | `COORDINATEUR` |
| 470 | `workspace.checkout.role` (de) | `Tech Lead · Koordinator` |
| 486 | `composer.coordinator` (de) | `KOORDINATOR` |
| 570 | `workspace.checkout.role` (zh) | `技术负责人 · 协调者` |
| 585 | `composer.coordinator` (zh) | `协调者` |

The i18n **key** `composer.coordinator` is an identifier and is referenced as `data-i18n="composer.coordinator"` at `src/components/alternatives/TeamComposer.astro:24` and as `copy["composer.coordinator"]` at `:25`. It stays. `workspace.checkout.role` is likewise a key and does not contain the term at all; only its six values do.

The remaining 33 lines (47 minus `landing.ts`'s 12 minus `TeamComposer.astro`'s 2 key references) are English or Spanish copy, demo data or a test assertion:

`src/components/Capabilities.astro:14`, `:34`, `:35`, `:76`; `src/components/CoordinationDemo.tsx:15` (comment), `:19`, `:20`, `:21`, `:22`, `:23`, `:24` (message-file slugs shown in the animation), `:34`, `:36`, `:117` (`aria-label`); `src/components/CoordinationProof.astro:16`; `src/components/Handoff.astro:28`, `:35` (message file names shown in a code block); `src/components/Install.astro:22`; `src/components/TeamSetup.astro:8`, `:31`; `src/components/Workflows.astro:13`, `:14`, `:23`, `:24` (visible chips); `src/components/alternatives/WorkspaceMock.astro:116`; `src/content/builderLab.en.ts:92`, `:278`, `:343`; `src/content/builderLab.es.ts:93`, `:279`, `:345`; `src/pages/alternatives/workspace.astro:113`; `tests/builder-lab.spec.ts:282`.

The website has **no pull-request CI**: `.github/workflows/deploy.yml` runs only on push to `main` and on manual dispatch. Local `npm run check` (`astro check`), `npm run build` and `npm run smoke` (Playwright) are the only gates available before merge.

### 3.8 CI and local gate inventory (derived from the target base workflows)

`.github/workflows/pr-regression-gates.yml` runs on `pull_request` and on any non-`main` push, with no path filter, so **every job below is triggered by this change**:

| Job | Runner | Commands |
| --- | --- | --- |
| `test-debt` | ubuntu | `npm run test:debt`, `npm run test:classify:self`, `npm run test:report:self` |
| `rust-regression` | windows | `npm ci`, `npm run build`, `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --lib --bins --tests` (in `src-tauri`) |
| `rust-regression-linux` | ubuntu | `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings` |
| `rust-regression-macos` | macos | `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings` |
| `terminal-snapshot-portable` | windows, ubuntu, macos-15, macos-15-intel | `cargo test --locked -p terminal-snapshot-renderer`, `cargo test --locked -p session-bridge --bin agentscommander-api-helper terminal_snapshot` |
| `windows-release-cli-smoke` | windows | `npm run build:prod:no-bundle`, `npm run smoke:cli-release-windows` |
| `frontend-regression` | ubuntu | `npm run typecheck`, `npm test` behind the #480 known-debt guard |

`.github/workflows/validate-branch-name.yml` runs on the push and requires `^(bug|chore|ci|docs|feat|feature|fix|refactor|style|test)/([1-9][0-9]*)-([a-z0-9]+(?:-[a-z0-9]+)*)$`; `refactor/1571-orchestrator-rename-visible-text` satisfies it.

`bundle-validation.yml`, `lockfile-check.yml` and `version-sync-check.yml` are path-filtered to Tauri config, icons, `package.json`, `package-lock.json`, `Cargo.lock`, `Cargo.toml`, `tauri.conf.json` and their own workflow files. This change touches none of those paths, so those three do not trigger. If the diff ever grows to touch one of them, re-derive this table.

Two repository gates exist but are **not** in CI and must be run by hand: `npm run check:frontend-dependencies` (frontend cycle check) and `npm run record:arcs` (Rust module-arc record, whose committed output is `src-tauri/module-arcs.txt`, 1037 lines at the frozen base).

### 3.9 Dependency-cycle baseline

`src-tauri/module-arcs.txt` at `ecc6527b` has 1037 recorded arcs. The frontend cycle check reports 0 errors at the frozen base. This plan adds no `use`, no `mod`, no `import`, no `export` and no new call: every Rust edit is either a string literal body, a new `const` declared beside the existing frozen constants in the same module that already holds them, or an added entry in a list that is already inside the same function. §11 records the full argument and the executable criterion.
---

## 4. Scope

### In scope (binding)

1. Every string rendered by the desktop UI, including element text, `title`, `aria-label`, `placeholder`, `<option>` labels, validation messages, toast defaults and the synthetic filter-text tokens. Exactly the 40 lines of §3.2.
2. Every `clap` help / `after_help` / `long_about` block and every `println!` / `eprintln!` line of the CLI, plus the `log::*` line that is a verbatim twin of one of them. Exactly the sites of §3.3 (a) and (b).
3. Every `Err(...)` / `format!` message that a user or an agent receives through the CLI, the Tauri IPC surface, the WebSocket plane, the mailbox or the container API helper. Exactly the sites of §3.3 (c).
4. Every text AgentsCommander writes into an agent's context or into a user-editable template file. Exactly the sites of §3.3 (d).
5. The two new frozen pre-rename snapshots, their recognizer wiring, their byte-exactness tests, and the two `current_version` bumps (§5.5).
6. Documentation prose in 36 Markdown files (§5.6), including the `docs/glossary.md` entry, which is renamed and moved to its alphabetical position.
7. The website: 14 files, 47 lines, six locales, two keys (§5.7).
8. Every test expectation that pins a literal this plan renames, and nothing else in any test (§9.3).

### Out of scope (binding)

- **Every identifier, in every language**: Rust items, TypeScript symbols, CSS class names, file names, directory names, JSON keys, Tauri event names, IPC command names, CLI flags, `data-ac-testid` values, i18n keys, PTY-input reason codes (`sender_not_coordinator`, `target_is_coordinator`), seed-manifest scopes (`context:coordinator`), settings keys. Phase 2 and Phase 3 own these.
- **`SelectionCoordinator` and its machinery** in every form: `src-tauri/src/session/selection.rs`, `src-tauri/src/commands/resource_monitor.rs`, `src-tauri/src/resource_monitor/watchdog.rs`, `src-tauri/src/commands/window.rs`, and the `selectionCoordinatorBusy` / `selectionCoordinatorUnavailable` error strings wherever they appear. Phase 2 (#1572).
- **Every frozen historical byte sequence**: the three `*COORDINATOR_CONTEXT_TEMPLATE_BEFORE_*` constants, the four `GLOBAL_*` / `STANDALONE_*` snapshots, the six frozen Root context snapshots, `ROOT_COORDINATION_MESSAGING_PARAGRAPH`, and everything reachable from `legacy_rendered_default_context_for_generation`, `pre_1072_legacy_rendered_default_context_for_compat` and `extract_legacy_skills_section`.
- **Source comments and doc comments** in `src/`, `src-tauri/` and `crates/`, including `.gitignore` marker comments (`config/instance_artifacts.rs:299`, `:305`). They are neither UI copy nor agent-facing context.
- **`log::*` payloads with no user-facing twin**, and their bracketed module tags (`[coordinator-clocks]`, `[auto-close]`, `[loops]`, `[mailbox]`, `[ac-discovery]`, `[session]`, `[resource-watchdog]`, `[context-alert]`).
- **Test fixture data, test names and `expect(...)` panic messages**, except where a test asserts a literal this plan renames.
- `CHANGELOG.md`, `plans/`, `scripts/*.ps1`.
- Any behavior change, any dependency change, any version bump.

---

## 5. Decided solution

### 5.1 Rule T: the substitution, fixed and total

Every rename in this plan is one deterministic, case-preserving token substitution. There is no implementer choice about wording anywhere.

| Matched token | Replacement |
| --- | --- |
| `coordinator` | `orchestrator` |
| `Coordinator` | `Orchestrator` |
| `COORDINATOR` | `ORCHESTRATOR` |
| `coordinators` | `orchestrators` |
| `Coordinators` | `Orchestrators` |
| `coordinator's` | `orchestrator's` |
| `coordinador` (es) | `orquestador` |
| `Coordinador` / `COORDINADOR` (es) | `Orquestador` / `ORQUESTADOR` |

Four further rows apply **only inside `repo-agentscommander_webpage`**, where they are the sole occurrences of these tokens in either repository (verified: `git grep -i -E "coordenador|coordinateur|koordinator|协调者"` returns nothing in `repo-AgentsCommander` at `ecc6527b`). They are part of Rule T rather than an AC7 exception, so §5.7.1's localisation decision is mechanically checkable by the same rule as everything else:

| Matched token | Replacement |
| --- | --- |
| `coordenador` / `Coordenador` / `COORDENADOR` (pt) | `orquestrador` / `Orquestrador` / `ORQUESTRADOR` |
| `coordinateur` / `Coordinateur` / `COORDINATEUR` (fr) | `orchestrateur` / `Orchestrateur` / `ORCHESTRATEUR` |
| `Koordinator` / `KOORDINATOR` (de) | `Orchestrator` / `ORCHESTRATOR` |
| `协调者` (zh) | `编排者` |

Four binding qualifiers:

- **T1, token boundary.** An occurrence of a matched token matches when the character immediately before it and the character immediately after it are each **either absent (start or end of line) or outside `[A-Za-z0-9_]`**. Both boundaries must hold; one alphanumeric or `_` neighbour is enough to exclude the occurrence. Worked cases, stated so a reviewer can code this without judgement:
  - **Excluded, leading boundary fails:** `isCoordinator`, `SelectionCoordinator`, `sortCoordinators`, `close_coordinator`, `_agent_COORDINATOR`, `selectionCoordinatorBusy`.
  - **Excluded, trailing boundary fails:** `coordinatorAutoCloseEnabled`, `coordinatorIdleBadgeYellowMinutes`, `coordinator_clocks`, `coordinatorClose.modal`, `coordinator_name`.
  - **Matches under T1, then preserved by Rule P0 because something resolves the token:** `--coordinator` and `--busy-coordinator` (leading `-` and trailing end-of-token are both valid boundaries, so T1 matches the trailing `coordinator` in each; the flag names are preserved by Rule P0, not by T1, because `clap` accepts them), `Context.coordinator.md` (a file that exists on disk), `context:coordinator` (a seed-manifest scope a manifest consumer keys on), `"coordinator":` **in key position only** (`docs/reference/cli.md:395`, `docs/agent-matrix-conventions.md:213`, `docs/agents/teams-and-workgroups.md:23`), `` `coordinator` `` at `docs/reference/cli.md:461`, which names that key, `composer.coordinator` (an i18n key the build resolves), `project:wg-1-team/coordinator` (an example FQN whose fixture Phase 2 owns). Two corrections round 3 owed this bullet. First, the entry is the **quoted key form** `"coordinator":` and not the bare spelling `"coordinator"`: in **value** position the identical spelling is RENAME, which is D8 at `src/sidebar/components/ProjectPanel.tsx:975` and `:1016` and three website sites, so listing the bare spelling here contradicted §5.2, AC7 step 2, D8 and §5.3 in the one section AC7 tells a reviewer to implement `ruleT` from. Second, the derived-instance number for every entry in this bullet is §5.2 item **3**, not item 2; round 3 inserted code position ahead of it and three citations were left pointing at the old number.
  - **Matches and is renamed:** every ordinary prose occurrence, plus `<coordinator>`, `<coordinator cwd>`, `regression-coordinator-<timestamp>`, `coordinator-to-dev-rust-assign`.

  **T1 and non-Latin tokens, recorded for the phase that will need it.** The boundary class is `[A-Za-z0-9_]`, so every neighbour of a CJK token is outside it and `协调者` satisfies T1 **unconditionally, wherever it appears**, including mid-word in Chinese prose. That is safe in this phase only because its two occurrences in either repository are both the role noun (`src/i18n/landing.ts:570` and `:585`), which the webpage owner verified by census. A later phase that adds Chinese prose must give the zh row its own boundary rule; T1 will not do the work there that it does for the Latin rows.

  Round 1 stated that `--busy-coordinator` "never matches"; that was wrong, because its boundary structure is identical to `--coordinator`'s. The outcome for `cli/loop_cmd.rs:326` is unchanged (the line is still not edited) but the reason is Rule P0, which preserves the token because `clap` resolves the flag (§5.2 derived instance **3**), not T1. This restatement exists because AC7 asks a reviewer to implement `ruleT` directly from this qualifier.
- **T1a, the three enumerated inside-identifier renames.** Exactly three occurrences in this plan are excluded by T1 and are nevertheless renamed, because the surrounding text is a descriptive placeholder rather than an identifier any code reads. They are a closed list and there are no others:
  1. `src-tauri/src/config/session_context.rs:3179`, `<coordinator_name>` to `<orchestrator_name>`.
  2. `src-tauri/src/config/session_context.rs:3622`, `<coordinator_name>` to `<orchestrator_name>`.
  3. `docs/agent-matrix-conventions.md:421`, `_agent_COORDINATOR/` to `_agent_ORCHESTRATOR/`.

  Any other occurrence that T1 excludes stays byte-identical. AC7 folds T1a into `ruleT` so these three pairs do not need a special case there.
- **T2, article agreement.** When the indefinite article immediately precedes the token, it changes with it: `a coordinator` becomes `an orchestrator`, `A coordinator` becomes `An orchestrator`, `a Coordinator` becomes `an Orchestrator`. Spanish articles do not change (`un coordinador` becomes `un orquestador`). This qualifier applies only when the article is adjacent; `a workgroup coordinator` becomes `a workgroup orchestrator` unchanged.
- **T3, different word.** `coordination`, `Coordination`, `Coordinate`, `coordinates` and `CoordinationDemo` are different words and are never touched. The epic's zero-occurrence gate targets `coordinator`, which none of them contains.

### 5.2 Rule P: where Rule T is not applied

Rule T is applied **only to the lines this plan enumerates**. Nothing else in either repository is edited. Within an enumerated line, the fate of each occurrence is decided by the rule below, **not by a lookup**. Round 2 stated this as a closed list of identifier forms and three reviewers each found a different form missing from it; the list is now derived from the rule instead of standing in for it.

**Rule P0, the generating rule.** Phase 1 changes text a human or an agent *reads*. It changes **no token a machine resolves**: §4's out-of-scope clause puts every identifier, file name, JSON key, event name, IPC command name, CLI flag, testid, i18n key, settings key, seed-manifest scope and reason code in Phase 2 or Phase 3, without exception. So for every occurrence of a Rule-T token that T1 matches, **plus the three T1a sites**:

> **The discriminator is referential, not structural.** Ask one question about the occurrence: **does something resolve this token?** Something resolves it when code binds or reads it, when a file or directory with that literal name exists on disk, when a CLI parser accepts that flag, or when a manifest, event, settings or i18n consumer keys on it.
>
> **PRESERVE** when something resolves the token. It is then a name with a referent, and Phase 2 or Phase 3 owns moving the referent and the name together. Renaming it here would leave the text naming something that does not exist.
>
> **RENAME** when nothing resolves the token and it only illustrates a shape to a reader: it is the role noun a person or an agent reads. **This holds even when the illustration is shaped like a path, a filename, a directory, an agent name or a key.** A shape is not a referent.
>
> Both halves hold **wherever the token appears**, including quoted or backticked inside prose, and inside a string literal.
>
> **Clause R1, a test expectation is never itself a referent.** A test that asserts on a literal **pins** it. Reading by a test is therefore not a resolution: the occurrence inside the expectation is classified by **what the pinned literal names**, and the expectation moves with that literal under §9.3. `assert_eq!(..., "Coordinator must be one of the selected agents")` classifies exactly as the `Err` message it pins, and `toContainText("coordinator")` exactly as the rendered badge it pins.
>
> **Clause R1 classifies an occurrence; it never widens what may be edited.** It is stated as "a test expectation is not a referent", deliberately, and **not** as "a body a test reads is renamed": the second phrasing would reach a frozen body. Rule T still applies only to the lines this plan enumerates (the opening sentence of this section) and §3.4 recognizer 4 still marks the frozen bodies untouchable. The pair that makes the separation concrete: `seeded_context_templates.rs:265` and `:266` carry **byte-identical sentences** to `session_context.rs:2523` and `:2524`, the first frozen inside `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE` and the second renamed. Clause R1 does not touch that separation, and nothing else in this plan may either.
>
> **Clause R2, a referent a later phase brings into being.** A token whose referent **a later phase creates or renames in step with this text** is PRESERVE even though nothing resolves it today. This is a **phasing** ground, not a referential one, and it is named here rather than left implied because the rule is otherwise total. **Two** occurrences in this plan rest on it. The first is `docs/features/terminal-snapshots.md:168`, `"requester": "project:wg-1-team/coordinator"`, whose 16 Rust fixture occurrences across `crates/` and `src-tauri/` Phase 2 renames (R8, §10.2). The second was added at round 7: `docs/reference/architecture.md:731`, the Concept-B row `` | `session/selection.rs` | Selection contract, coordinator, ... | ``, whose referent Phase 2 renames to `SelectionArbiter` rather than to Orchestrator, so this phase would otherwise ship a line naming a type an orchestrator that never becomes one (D13, §5.6.2). Round 5 read this clause as covering exactly one occurrence; D13 supersedes that reading, and the clause's shape is unchanged. Nothing resolves that FQN today (no such project, workgroup or agent; no file, no parser, no manifest) and a parallel copy of the same spelling in a fixture is not a referent for this occurrence, so without clause R2 the rule would rename it and contradict §5.6.2.

**Clause R1 retires round 4's carve-out rather than adding a second exception beside it.** Round 4 rescued exactly one class, "an agent-facing template body", from its own gloss of "resolves" as "code binds or **reads** it", and left every other test expectation in §6.2 exposed to that gloss. Under clause R1, `# Coordinator Context` needs **no carve-out at all**: the literal names a separator heading in a body this plan rewrites, nothing else resolves it, and RENAME falls out. That the clause round 4 called load-bearing becomes unnecessary is the sign the generalization is the right shape. The three live assertions it cited, `contains("# Coordinator Context")` at `session_context.rs:8036`, `:8481` and `:8561`, are pins that move under §9.3, and AC1 needle 30 still reaches zero.

**What clause R1 decides, worked over the sites that made it necessary.** RENAME, because in each case the pinned literal is a message, a UI label or a badge that nothing resolves: `commands/entity_creation.rs:4492` (`"Coordinator must be one of the selected agents"`), `:6381` and `:6404` (`"coordinator of team(s): dev-team"`); `phone/mailbox.rs:15701`, `:15711`, `:15733` (`"Root Agent can only message verified WG coordinator replicas"`), `:15852` and `:15861` (`"Only verified WG coordinator replicas may message the Root Agent"`); `session/context_alerts.rs:2947` (`"Coordinator target changed"`); `config/seeded_context_templates.rs:3704`, `:3714` and `:3724` (`label: "Coordinator context"`, pinning the §5.4 `:460` UI label); and, in the **website** repository, `tests/builder-lab.spec.ts:282` (`toContainText("coordinator")`, pinning the rendered badge), which the key-versus-value bullet below already names as RENAME. That last site is **not** a §6.2 line: §6.2 is `repo-AgentsCommander`, tests, and this spec exists only in the website repository, where it is enumerated as the 33rd of §3.7's 47 lines and given its exact result in §5.7.2. The 16 frontend test lines of §6.2 are the same shape and classify the same way. PRESERVE, and for the referent rather than for the reading: `commands/ac_discovery.rs:4098` and `:4143` assert on `scope = "context:coordinator"`, preserved because the **seed manifest** keys on that scope; `config/root_agent.rs:2053` asserts `!ROOT_ROLE_MD.contains("workgroup add --coordinator")`, preserved because **`clap` accepts the flag**. In both the pinned literal is what carries the referent, which is exactly what clause R1 tells the reviewer to look at.

**Position is an aid, not the authority.** The two positions below are where a resolved token almost always sits, and a reviewer may use them to spot-check cheaply. They do **not** decide:

- **Code position**: a source identifier, being a binding, parameter, field, property or method access, or a path segment. `let coordinator = ...`, `team.coordinator`, `coordinator.contains(...)`, `coordinator.current_version`, `C::TargetIsCoordinator`.
- **Key or name position**: a key rather than a value (`"coordinator":`, `composer.coordinator`, `coordinatorCascadeCloseEnabled`), a file, directory or source path, a CLI flag, an event name, a seed-manifest scope, a reason code, a `data-ac-testid`, a CSS class, or an example FQN whose fixture a later phase owns.

**Round 3 stated the rule the other way round, and that was the round-3 blocker.** It opened with the referential test and then let the enumerated structural positions govern; AC7 step 2, the operative instruction at step 9, dropped the head clause and restated only the positions. On **seven occurrences this plan mandates renaming**, the structural reading returns PRESERVE, confidently and backwards, and none of the seven is on AC7's five-item exception list:

| # | Site | Text at base | Structural verdict | Referential verdict, which governs |
| --- | --- | --- | --- | --- |
| 1 | `docs/agent-matrix-conventions.md:421` | `├── _agent_COORDINATOR/` | directory name, PRESERVE | no such directory exists; it is a placeholder in a family with `_agent_WORKER_1` and `_agent_REVIEWER`. **RENAME**, and it is T1a item 3 |
| 2 | `src/components/Handoff.astro:28` | `<span class="code-path">20260628-213014-coordinator-to-dev-rust-assign.md</span>` | file name, PRESERVE | a picture of a message filename on a marketing page; no such file, nothing parses it. **RENAME** (§5.7.2) |
| 3 | `src/components/Handoff.astro:35` | `<span class="code-path">20260628-213122-dev-rust-to-coordinator-done.md</span>` | file name, PRESERVE | as above. **RENAME** (§5.7.2) |
| 4 | `src/components/alternatives/WorkspaceMock.astro:116` | `<span class="wm-dim">coordinator:</span> dev-ui is checking...` | key position, PRESERVE | a rendered mock transcript; the colon is punctuation in displayed copy, not a key separator. **RENAME** (§5.7.2) |
| 5 | `docs/testing/03-agent-lifecycle.md:112` | `` `regression-coordinator-<timestamp>` `` | agent or directory name, PRESERVE | a name the tester invents at run time; nothing resolves it. **RENAME** (§5.6.1) |
| 6 | `docs/features/session-auto-close.md:149` | `{ "replicaPath": "<coordinator cwd>", ... }` | **both halves fire with opposite verdicts**: path shape says PRESERVE, value position says RENAME | it is a placeholder *for* a path, not a path. **RENAME** (§5.6.1) |
| 7 | `src/components/CoordinationDemo.tsx:19` to `:24` | six `slug: 'coordinator-to-dev-rust-assign'` and siblings | path component (PRESERVE) sitting in value position (RENAME) | animation fixture data; no file, no route, no consumer. **RENAME** (§5.7.2) |

Item 6 satisfies D12's own round-3 reopen condition verbatim, because Rule P0 could not classify it by position. Items 1 to 5 are worse than that condition contemplated: position classifies them confidently and **wrongly**, which is why D12's condition is widened in §10.1.

**That class is not documentation-and-website only, and the table's file paths should not be read as saying it is.** It reaches the Rust half seven times, and §5.2.1 tabulates all seven with the flag that surfaces each. Two are `config/session_context.rs:2523` and `:2524`, colon-followed bullet labels inside `get_default_coordinator_template()` and so inside the mandated `:2509` to `:2529` range of §3.3 (d): they are structurally identical to item 4 and the structural reading would have preserved them; the referential rule renames them without being told, which is what §5.4 mandates. Two more are `<coordinator>` inside the Root message-filename pattern at `:3534` and `:3619`, which is item 6's construct sitting in a Rust template body. Two are the T1a placeholders `<coordinator_name>` at `:3179` and `:3622`, which sit with item 1 in the bucket T1 excludes and step 2 nevertheless enumerates. The seventh is `:5450`, a test expectation pinning the `:3534` and `:3619` pattern, which enters the class only because §6.2 gains that line. A sweep of the whole enumerated Rust set found no others, and `root_agent.rs:615` to `:663` has none.

**Why position cannot be patched into working, and adding exceptions would not have fixed it.** This plan preserves and renames pairs of occupants of the *identical* position:

| Position | Preserved | Renamed |
| --- | --- | --- |
| directory name | `_agent_coordinator` (derived instance 3) | `_agent_COORDINATOR/` at `agent-matrix-conventions.md:421` |
| file name | `Context.coordinator.md`, `coordinator_clocks.json` | `20260628-213014-coordinator-to-dev-rust-assign.md` |
| agent name or FQN | `project:wg-1-team/coordinator` (R8), preserved by **clause R2** rather than by resolution | `regression-coordinator-<timestamp>` |
| key position | `"coordinator":` at `cli.md:395` | `coordinator:` at `WorkspaceMock.astro:116` |

In every row both members sit in the same position. In the first, second and fourth rows they differ only in whether anything resolves the token. The third row is the exception and is the reason clause R2 is stated in the rule: nothing resolves either member, and the preserved one is preserved because Phase 2 renames its Rust fixtures in step with it. Position therefore cannot be the discriminator at any granularity, and round 3's totality claim, that the rule partitions by position rather than by spelling, was false as written. §5.6.1 and D9 already stated the correct test in their own words ("None of these is a value any code reads", "renamed because no code reads them"); round 4 promotes that sentence from a documentation aside to the operative rule, in AC7 step 2 as well as here.

**A latent case, recorded because it shows the other half was unsound too.** `seeded_context_templates.rs:2038`, `assert_eq!(coordinator.id, "coordinator")`: the string is a **value**, so the structural rule says RENAME, yet it is the spec `id` tied to the preserved seed scope `context:coordinator` and must not move. It is not an edit line and is correctly outside AC6's nine, so nothing in this plan acts on it; it is named here so a reviewer can see that the value-to-RENAME half failed in the same way as the key-to-PRESERVE half, and that the referential test answers both.

The rule is total because every occurrence either has a referent or does not, and that is a property of the repository rather than of the spelling or the surrounding punctuation. Phase 1's scope boundary then guarantees that nothing with a referent is edited. Two consequences, each of which was a round-2 blocker:

- **Key versus value settles the bare quoted spelling**, and no count needs asserting. The preserved form is the full quoted key `"coordinator":`, closing quote included, and not the shorthand "`"coordinator"` immediately followed by `:`" that round 3 used. The shorthand misreads `src/components/CoordinationDemo.tsx:36`, `label: 'coordinator: Codex profile, GPT 5.5 xhigh'`, where the token is quote-preceded and colon-followed inside a rendered label that §5.7.2 renames; the rule's intent gets `:36` right because nothing resolves that label, and the shorthand does not. So `"coordinator":` is a key and is PRESERVE: `docs/reference/cli.md:395`, `docs/agent-matrix-conventions.md:213`, `docs/agents/teams-and-workgroups.md:23`, and the key half of the six `src/i18n/landing.ts` `"composer.coordinator": "..."` lines. The identical spelling in **value** position is RENAME: `src/sidebar/components/ProjectPanel.tsx:975` and `:1016` (the synthetic filter token, D8), `src/content/builderLab.en.ts:92` (`badges: ["coordinator", ...]`), `tests/builder-lab.spec.ts:282` (`toContainText("coordinator")`), `src/components/CoordinationDemo.tsx:34` (`name: 'coordinator'`). A **backticked** span is not a quoted key and is classified by what it names: `docs/reference/cli.md:461` is `` `agents`, `coordinator`, and `repos` ``, a backticked key name, PRESERVE because it names a key, not because of the quoting.
- **A resolved identifier beats spelling.** An identifier spelled exactly `coordinator` or `Coordinator` is PRESERVE even though T1 matches it (its neighbours are punctuation, not word characters) and even though no enumeration happens to name it. That covers `team.coordinator` at `ProjectPanel.tsx:1016` (the serialized `AcTeam.coordinator` field, `src/shared/types.ts:1230`, fed by `commands/ac_discovery.rs:84`), `coordinator.contains(...)` at `session_context.rs:8137`, and `coordinator.current_version` at `seeded_context_templates.rs:2039`. All three sit on lines this plan **does** edit, for a different occurrence on the same line. The class is open by construction, because §9.3 makes the Rust suite the oracle for test expectations, so any future test line that both pins a renamed literal and uses a `coordinator` binding lands here; Rule P0 answers it without a new entry.

**Derived instances**, offered so a reviewer can spot-check the rule cheaply, and **not** authoritative over it. Where an instance and Rule P0 disagree, Rule P0 governs and the instance is the defect:

1. Any occurrence that T1 already excludes (a longer identifier), except the three T1a sites. T1 exclusion and Rule P0's first half overlap heavily; where they differ, both preserve.
2. Code position: `let coordinator = ...`, `team.coordinator`, `coordinator.contains(...)`, `coordinator.current_version`, `C::SenderNotCoordinator`, `C::TargetIsCoordinator`.
3. Key or name position: a CLI flag (`--coordinator`, `--busy-coordinator`), a file or directory name (`Context.coordinator.md`, `Context.coordinator.md.bak`, `coordinator_clocks.json`, `config/coordinator_clocks.rs`, `_agent_coordinator`), a JSON key in key position (`"coordinator":`) or named as a key (`` `coordinator` `` at `cli.md:461`), an event name (`coordinator_clock_updated`), a seed-manifest scope (`context:coordinator`), a reason code (`sender_not_coordinator`, `target_is_coordinator`), an i18n key (`composer.coordinator`), a `data-ac-testid` value (`actionBar.sortCoordinators`, `coordinatorClose.modal`, `coordinatorClose.cancel`), a CSS class (`coord`, `coord-idle`, `coord-autoclosed`, `team-group-coordinator`, `coord-quick-access`), or an example FQN (`project:wg-1-team/coordinator`). **Read this instance through Rule P0, not as a shape list.** Every entry but one is here because something resolves it: the code creates and reads `_agent_coordinator` replica directories, `Context.coordinator.md` exists on disk, `clap` accepts the flags, the manifest keys on the scope. The one exception is the example FQN `project:wg-1-team/coordinator`, which nothing resolves today and which **clause R2** preserves because Phase 2 renames its 16 Rust fixture occurrences in step with it. A token merely *shaped* like one of these and resolved by nothing is RENAME, which is why `_agent_COORDINATOR/` at `docs/agent-matrix-conventions.md:421` is renamed while `_agent_coordinator` is preserved, and why the six `CoordinationDemo.tsx` slugs are renamed while `coordinator_clocks.json` is preserved.
4. `src/shared/ipc.ts:1039`, in full: the message names the `coordinator` response key, and the line is not edited at all.
5. The documentation lines enumerated in §5.6.2.

Every preserved occurrence in this plan is a Phase 2, Phase 3 or Phase 4 residual, not an oversight. §10 records them.

#### 5.2.1 The completeness procedure, and its measured result

An enumeration that has been patched twice gets patched a third time. This is the procedure that proves the classification is complete over the lines AC6 and AC7 read, stated so a reviewer re-runs it rather than trusting a list. It is what found the third code-position site.

**Procedure.**

1. Build the enumerated edit-line set: the 40 frontend lines of §3.2, the 16 frontend test lines of §6.2, the §3.3 (a) to (d) production sites minus the two lines §5.4 marks "listed for completeness and not edited" (`cli/loop_cmd.rs:326`, `cli/workgroup.rs:194`), the Rust test lines of §6.2, the documentation edit lines (**254**: the 296 in-scope matching lines minus the 59 of §5.6.2, which is 237, **plus** the 17 preserve-list lines that carry a renamed occurrence beside the preserved one and therefore do enter the diff, enumerated in this section's last bullet. §5.6.1 makes the preserve decision per occurrence and not per line, so a preserve-list line is not automatically an unedited line; rounds 1 to 6 read it as one and published 238), and the website lines that change (the 47 of §3.7 minus `TeamComposer.astro`'s 2, which is 45, because the six `landing.ts` key lines change on their value half).
2. On each line, enumerate every Rule-T-family token occurrence that satisfies T1, **plus the three T1a sites of §5.1**, exactly as AC7 step 2.1 does. Round 3's step 2 said "every occurrence that satisfies T1" and stopped there, so `_agent_COORDINATOR/` at `docs/agent-matrix-conventions.md:421` was never enumerated: the completeness procedure structurally could not see the one rename T1 structurally blocks. That omission is why the round-3 sweep reported completeness over a set that excluded a mandated rename.
3. Flag four shapes. **F1**, every line carrying more than one occurrence. **F2**, every occurrence in apparent **code position**: immediately preceded by `.` or `::`, or a bare identifier immediately followed by `.`, `(` or `[`. **F3**, in Markdown, every occurrence inside a backticked span. **F4, new in round 4**, every occurrence inside a path-shaped, filename-shaped, key-shaped or placeholder-shaped token: a neighbouring character is `/`, `_`, `.` or `-`; or the occurrence is immediately followed by `:`; or the occurrence sits inside an angle-bracket placeholder, meaning the nearest `<` before it is unclosed and a `>` follows it before any further `<`.
4. Classify each flagged occurrence by Rule P0 and confirm this plan's own text agrees.

**The flags are deliberately over-inclusive triage, and a flag count far above the table is expected rather than a defect.** F2 has no "outside a string literal" qualifier and F4 has none either, so both fire on ordinary prose: `Select a coordinator...` trips F2 on the ellipsis, `non-coordinator` and a sentence-final `coordinator.` trip F4, and a literal re-run flags 72 occurrences across the documentation edit lines where the table's classified counts are far smaller. That is the intended behavior. Triage is cheap and a missed occurrence is not, so the flags are tuned to over-report and step 4 does the work. The table below therefore counts what **Rule P0 classifies**, not what step 3 flags; the defect condition is unchanged and is stated at the end of this section.

**Measured at `ecc6527b` / `85f318d3`, 2026-08-27, and re-run at round 4 with F4 and the T1a addition.** Each row counts only lines that carry at least one T1-satisfying occurrence. The **Code-position** column counts occurrences **Rule P0 classifies as code position**, which is the number a reviewer needs; it is not the F2 flag count, which is larger. Reading the column as a flag count is what makes the website row look wrong: `composer.coordinator` in the six `src/i18n/landing.ts` lines is `.`-preceded and does trip F2, so a literal F2 count for that row is 6, but Rule P0 classifies those six as **key or name position** (an i18n key the build resolves), not code position, so the classified count is 0. The classification, and therefore every downstream criterion, is unchanged either way.

| Edit-line set | Lines | Occurrences | Code-position (Rule P0) | Lines with >1 |
| --- | --- | --- | --- | --- |
| Rust and crate production single-line sites, §3.3 (a) to (d) | 85 | 93 | 0 | 8 |
| Rust agent-context template bodies, the §3.3 (d) line ranges | 18 | 26 | 0 | 5 |
| Rust tests, §6.2 | 32 | 35 | **2** | 3 |
| Frontend, §3.2 | 38 | 40 | **1** | 2 |
| Frontend tests, §6.2 | 16 | 16 | 0 | 0 |
| Website changed lines | 45 | 51 | 0 | 6 |
| Documentation edit lines | 254 | (n/a) | 11 backticked | (n/a) |

**One row moved at round 6 and no other did.** §6.2's Rust set gains `config/session_context.rs:5450` and `:5483`, two live pins round 5 omitted, so the Rust-tests row goes from 30 lines / 32 occurrences / 2 lines with more than one to **32 / 35 / 3**: `:5450` carries one T1 occurrence (`<coordinator>`) and `:5483` carries two (`coordinators` and `non-coordinator`). The code-position count stays **2**, because neither new line carries a code-position occurrence. The other five rows are unchanged and were not re-derived.

**One further row moves at round 7, and one figure is corrected.** §3.3 (a) gains the five `cli/mod.rs` `about` doc comments, so the Rust production row goes from 80 lines / 88 occurrences to **85 / 93**. Each of the five carries exactly one T1 occurrence, so "lines with >1" stays **8** and the code-position count stays **0**. **None of the five is a flip-class member**, so the flip class stays at 20: on all five the role noun sits in ordinary English prose with a space, a `(` or a `-` as its neighbours, and the structural and the referential readings both return RENAME. Separately, the documentation row is corrected from 238 to **254** under A3, which is an arithmetic correction and not a change to any edit; see step 1 above and this section's last bullet. The remaining four rows are unchanged and were not re-derived.

**What the round-4 re-run added, recounted at round 5.** F4 plus the T1a enumeration surface the **flip class**: the occurrences that classify **RENAME** under the referential Rule P0 and that the round-3 structural reading would have classified PRESERVE. **The membership criterion, stated once so the set is derived and not listed:** an occurrence belongs to the flip class exactly when all three of the following hold. First, **step 2 enumerates it**, meaning it satisfies **T1** or it is one of the three **T1a** sites. Second, **Rule P0 classifies it RENAME**. Third, **round 3 would have classified it PRESERVE**, which can happen in either of two ways: round 3's structural positions return PRESERVE on it, or round 3's step 2 never enumerated it and derived instance 1 preserved it inside a longer identifier. Applying that criterion to the step-1 edit-line set gives **20 occurrences on 20 lines**. Round 4 published 13, round 5 published 15, and the five added here are `config/session_context.rs:2523`, `:2524`, `:3179`, `:3622` and `:5450`. This is the check that had to pass before this plan could be certified again, and it is reproducible line by line:

| Site | Flag that surfaces it | Rule P0 |
| --- | --- | --- |
| `docs/agent-matrix-conventions.md:421`, `_agent_COORDINATOR/` | step 2's T1a addition, then F4 (`_` before, `/` after) | RENAME |
| `src/components/Handoff.astro:28` and `:35` | F4 (`-` on both sides) | RENAME |
| `src/components/alternatives/WorkspaceMock.astro:116` | F4 (followed by `:`) | RENAME |
| `docs/testing/03-agent-lifecycle.md:112` | F4 (`-` on both sides) and F3 | RENAME |
| `docs/features/session-auto-close.md:149`, `<coordinator cwd>` | F4 (angle-bracket placeholder) and F3 | RENAME |
| `config/session_context.rs:3534` and `:3619`, `` `YYYYMMDD-HHMMSS-root-to-<wgN>-<coordinator>-<slug>.md` `` | F4 twice over (angle-bracket placeholder, and `-` on both sides) | RENAME |
| `config/session_context.rs:2523` and `:2524`, the two colon-followed bullet labels of `get_default_coordinator_template()` | F4 (followed by `:`) | RENAME |
| `config/session_context.rs:3179` and `:3622`, `<coordinator_name>` | step 2's T1a addition, then F4 twice over (`_` after, angle-bracket placeholder) | RENAME |
| `config/session_context.rs:5450`, the test expectation pinning the `:3534` and `:3619` pattern | F4 twice over (angle-bracket placeholder, and `-` on both sides) | RENAME |
| `src/components/CoordinationDemo.tsx:19` to `:24`, six slugs | F4 (`-` neighbours) | RENAME |
| `src/components/CoordinationDemo.tsx:36`, `label: 'coordinator: ...'` | F4 (followed by `:`) | RENAME |

**Why the criterion admits both of the disputed pairs, and one occurrence neither reviewer named.** The third conjunct decides both disputed pairs, and it has two independent ways to hold. `:2523` and `:2524` hold it the first way: they satisfy T1, round 3 enumerated them, and round 3's key-or-name position returns PRESERVE on a colon-followed token exactly as it does on `WorkspaceMock.astro:116`, which is the table's item 4. `:3179` and `:3622` hold it the second way, which is the ground `_agent_COORDINATOR/` already stands on in this table: T1 excludes all three on a `_` neighbour, so round 3's step 2 never reached them and derived instance 1 preserved them as parts of a longer identifier. No criterion admits `_agent_COORDINATOR/` and rejects `<coordinator_name>`, because the two differ in nothing the criterion tests; a criterion narrow enough to reject the `<coordinator_name>` pair drops `_agent_COORDINATOR/` with them and the count falls to 16, contradicting a member verified at rounds 4 and 5. Both pairs are therefore in, which would give 19. The count is **20** because §6.2's round-6 correction adds `:5450`, whose `<coordinator>` sits inside a filename-shaped placeholder that the structural reading preserves and that clause R1 renames with the `:3534` and `:3619` pattern it pins. Nothing else in the Rust set qualifies: every other §6.2 Rust test line pins prose, a heading or a message that both readings rename, or a key, an identifier or a flag that both readings preserve. The one line where the two readings differ in the **other** direction is `seeded_context_templates.rs:3241`, whose pin message the structural reading renames and the referential rule preserves for the spec `id` it names (§5.5.3); a reverse flip is not a member of this class.

Only **two** of those 20 were visible to the round-3 flags: the two Markdown ones, which F3 caught and which §5.2.1 already classified correctly. The other **18** were invisible to all three, and for two different reasons. **Fifteen on fifteen lines** are single occurrences with `-`, `:` or angle-bracket neighbours, no dot and outside Markdown, so F1, F2 and F3 all pass over them: ten website occurrences on ten lines (`Handoff.astro:28` and `:35`, `WorkspaceMock.astro:116`, `CoordinationDemo.tsx:19` to `:24` and `:36`), plus five Rust lines, being the four agent-context template lines `session_context.rs:2523`, `:2524`, `:3534` and `:3619` and the test expectation `:5450` that pins the last two. `:3534` and `:3619` are precisely the construct F4's placeholder condition was added for: round 4 added that condition on the strength of `` `<coordinator cwd>` `` at `docs/features/session-auto-close.md:149`, which round 4 recorded as surfaced by F4's angle-bracket condition together with F3, and the same construct turns out to sit in a Rust template body, twice. Both are enumerated §3.3 (d) edit lines inside `default_context_dynamic_values` (`:3466` to `:3666`), which is live rendering and not a legacy reconstruction; both are mandated RENAME by §5.4; and the structural reading returns PRESERVE on them by this plan's own applied standard, since it classifies `Handoff.astro:28` and `:35` as "file name, PRESERVE". The remaining **three** are the three T1a sites, `_agent_COORDINATOR/` at `docs/agent-matrix-conventions.md:421` and `<coordinator_name>` at `session_context.rs:3179` and `:3622`: they were not merely unflagged but **never enumerated**, because round 3's step 2 enumerated only T1-satisfying occurrences and T1 excludes all three on a `_` neighbour. That is why the round-3 website row could report code-position 0 and the procedure could report completeness while the key-and-name half of the classification had never been tested on a single occurrence. F4 and the step-2 addition test it.

**The flip class is not the same set as "the occurrences F4 newly flags", and the two must not be conflated.** Over the frontend and website halves F4 newly surfaces **13** occurrences the round-3 flags missed. Ten of them are the flip-class members on those halves (`Handoff.astro:28` and `:35`, `WorkspaceMock.astro:116`, `CoordinationDemo.tsx:19` to `:24` and `:36`); the other three are not flips: `CoordinationDemo.tsx:117` (`aria-label` prose), and `EditTeamModal.tsx:374` and `NewTeamModal.tsx:321` (both `title="Set as coordinator"`). None of the three is a misclassification and none is a flip: all are rendered copy, Rule P0 returns RENAME, §5.7.2 and §5.3 already rename them, and `:374` and `:321` are pinned by AC1 needle 11. The flip set is the one tabulated above; over these two halves the newly-flagged set is strictly larger and contains it.

**F4's placeholder condition is line-local, which makes it look asymmetric when it is not.** "The nearest `<` before it is unclosed" is evaluated within the line, so the condition does **not** fire on JSX text (`>coordinator</span>`, where the nearest preceding `<` is already closed by the opening tag's `>`) and **does** fire inside a single-line tag attribute (`title="...coordinator..."`, where the opening `<` is still unclosed on that line). So `EditTeamModal.tsx:374` is flagged while `WorkgroupGroupRail.tsx:254` and `:255` are not, although all three are attribute values: the latter two are continuation lines of a `<span` opened at `:251` and carry no `<` at all. That asymmetry is not a defect and needs no repair, because **flags never discover lines**. Step 1 builds the line set from this plan's enumerations, so an unflagged line is still enumerated, still classified at step 4 and still renamed; a missed flag can cost triage attention but cannot hide a missed rename.

**One edit line judged and deliberately not counted, recorded so it is not re-found and called differently.** `docs/reference/settings.md:427`, "ON for coordinator/Root", is an edit line, is not on the 59 of §5.6.2, and does trip F4 on the `/`. It is prose disjunction rather than a path segment, it classifies RENAME like the rest of that line's prose, and it is not a member of the flip class.

Every flagged occurrence, classified:

- **Three code-position occurrences, all PRESERVE under Rule P0**: `session_context.rs:8137` (`coordinator.contains(...)`, the line changes because the string it holds pins decoded body line 24), `seeded_context_templates.rs:2039` (`coordinator.current_version`, the line changes because the integer moves 4 to 5), `ProjectPanel.tsx:1016` (`team.coordinator`, the line changes because the filter token moves). Round 2 named none of the three in §5.2: two were reported as blockers, and `:2039` was reachable only through AC7 exception 2, which excuses the pair rather than classifying the occurrence. Under Rule P0 all three pass on their own.
- **The six website lines with two occurrences** are exactly the `"composer.coordinator": "<value>"` pairs at `landing.ts:71`, `:176`, `:278`, `:381`, `:486`, `:585`: key PRESERVE, value RENAME, by the key/value half of Rule P0. There is no other multi-occurrence line and no code-position occurrence in the website repository.
- **The 17 multi-occurrence Rust and frontend lines** (8 + 5 + 2 + 2 across the four rows above) carry only prose, string-literal or template-body occurrences beside the flagged ones; every occurrence on them is RENAME. Two of those lines carry an identifier as well, `phone/types.rs:206`'s match arm `C::TargetIsCoordinator` and `agentscommander-api-helper.rs:684`'s reason code `"target_is_coordinator"`, but T1 **excludes** both, so neither is enumerated and neither shows in the counts above. Rule P0 would preserve them anyway.
- **The 11 documentation edit lines with a backticked occurrence** are all already decided as RENAME by §5.6.1: `project-loops.md:25`, `:28`, `:32`, `:78`, `:79`, `:91`, `:117` and `sidebar-guide.md:10`, `:38` are quoted UI copy that follows the UI; `session-auto-close.md:149` (`` `<coordinator cwd>` ``) and `03-agent-lifecycle.md:112` (`` `regression-coordinator-<timestamp>` ``) are illustrative placeholders. Nothing resolves any of them, so none is a Rule P0 PRESERVE and none needs an AC7 exception. Note that `session-auto-close.md:149` and `03-agent-lifecycle.md:112` are BL1 items 6 and 5: F3 saw them at round 3 and §5.2.1 classified them correctly here, while round 3's Rule P0 as written classified them backwards, which is precisely the split between the sweep's reach (BL2) and the rule's soundness (BL1).
- **In the other direction, 17 of the 59 preserve-list lines of §5.6.2 do enter the diff**, because §5.6.1 makes the preserve decision per occurrence and not per line. Round 7 corrects the arithmetic that had treated the preserve list as an unedited set; the edits themselves were always right, and rounds 1 to 6 named 11 of the 17. The 17 fall into three groups, and the third is the one no earlier round stated.

  **Six carry a T1-matching PRESERVE occurrence beside renamed text**: `agent-matrix-conventions.md:34` and `:232`, `teams-and-workgroups.md:182`, `cli.md:481`, `directory-layout.md:42` and `:43`. Those are exactly the six AC7's false-positive paragraph names, and the only six on which AC7 pairs a preserved occurrence against a renamed one. Say "renamed text" rather than "renamed prose": the phrase undercounts what AC7 will pair on one of them, because `agent-matrix-conventions.md:34` carries **four** T1 occurrences, being `Context.coordinator.md` (PRESERVE), two prose occurrences (RENAME) and the backticked `` `# Coordinator Context` `` (RENAME, quoted UI copy under §5.6.1), so one of its renamed occurrences is backticked rather than prose. Handled correctly by Rule P0 and by §5.6.1; named here so the pairing is not a surprise at step 9.

  **Five carry two prose T1 occurrences beside a T1-excluded settings key**: `teams-and-workgroups.md:199`, `session-auto-close.md:64` and `:83`, `settings.md:274` and `:285`. Both prose occurrences on each are RENAME; the line stays on the preserve list only because the T1-**excluded** camelCase or snake_case key elsewhere on it keeps it matching AC2's case-insensitive needle. Neither the line nor either occurrence needs an AC7 exception.

  **Six carry exactly one T1 occurrence, and it is RENAME.** Same mechanism as the previous group with one prose occurrence instead of two, and invisible to every earlier round because each of those rounds framed the class as "lines with more than one occurrence". Named at round 7:

  | Line | Renamed prose | Preserved on the same line, T1-excluded |
  | --- | --- | --- |
  | `session-auto-close.md:72` | `Auto-close closes coordinators and agent-owned...` | `coordinatorCascadeCloseEnabled` |
  | `architecture.md:777` | `Coordinator idle clocks` | `config/coordinator_clocks.rs` |
  | `directory-layout.md:76` | `Coordinator clock state` | `coordinator_clocks.json`, `config/coordinator_clocks.rs` |
  | `settings.md:286` | `the coordinator idle badge turns yellow` | `coordinatorIdleBadgeYellowMinutes` |
  | `settings.md:287` | `the coordinator idle badge turns red` | `coordinatorIdleBadgeRedMinutes` |
  | `semantic-ui-automation-affordance-matrix.md:34` | `Toggle coordinator sort` | `actionBar.sortCoordinators` |

  The **remaining 42** preserve-list lines carry no renamed occurrence and do not appear in the diff at all. Of the 59, **11** carry more than one T1 occurrence, which is the figure round 6 reported and which answers a different question from how many enter the diff; conflating the two is what produced the 238. A step-9 reviewer re-running this procedure over the actual diff should read the difference between 237 and 254 as this procedure's own arithmetic, not as 17 stray edits.

Re-run this procedure whenever a line is added to or removed from any enumeration this plan carries. A new flagged occurrence that Rule P0 **does not classify, or classifies against §5.6.1, §5.6.2, §5.7.2 or T1a**, is a defect in Rule P0 and is the one thing that should reopen §5.2. The second clause matches D12's widened condition and is the one that catches a confident misclassification; round 3 carried only the first and it was blind to six of the seven sites above.

### 5.3 Frontend edits (12 files, 40 lines): the exact result

Apply Rule T to each line of the §3.2 table. The three lines where the result is not a bare token substitution are decided here:

- **`AcDiscoveryPanel.tsx:237` and `:290`.** The one-letter role badge `<span class="ac-discovery-badge coord">C</span>` becomes `<span class="ac-discovery-badge coord">O</span>`. The letter is the initial of the role name; it is visible copy, not an identifier. The CSS class `coord` does not change.
- **`ProjectPanel.tsx:975` and `:1016`.** The synthetic filter token `"coordinator"` becomes `"orchestrator"`. This is the word a user types into the sidebar filter to match orchestrator rows, so it is user-visible vocabulary and must move with the badge text it mirrors.
- **`SettingsModal.tsx:1984`.** `your last message to a coordinator` becomes `your last message to an orchestrator` under T2.

The resulting text, for the lines where the whole string is worth stating:

| File:line | New text |
| --- | --- |
| `ActionBar.tsx:299` | `"Show recent orchestrators first"` / `"Show orchestrators in default order"` |
| `EditLoopModal.tsx:228`, `NewLoopModal.tsx:171` | `Workgroup Orchestrator` |
| `EditLoopModal.tsx:236`, `NewLoopModal.tsx:179` | `Select an orchestrator...` |
| `EditLoopModal.tsx:242`, `NewLoopModal.tsx:185` | `A workgroup with a verified orchestrator is required.` |
| `EditLoopModal.tsx:282`, `NewLoopModal.tsx:220` | `Force inject even if orchestrator is busy` |
| `NewLoopModal.tsx:195` | `placeholder="Prompt to inject into the orchestrator"` |
| `EditTeamModal.tsx:374`, `NewTeamModal.tsx:321` | `title="Set as orchestrator"` |
| `ProjectPanel.tsx:2197` | `"Time this team has been idle. Resets when you message the orchestrator or any member is active (persists across restarts)."` |
| `ProjectPanel.tsx:2368` | `title="This team's orchestrator was closed manually. Reopen it to clear."` |
| `ProjectPanel.tsx:2426`, `:3260` | `<span class="ac-discovery-badge coord">orchestrator</span>` |
| `ProjectPanel.tsx:2670` | `<span class="ac-wg-name">Orchestrators</span>` |
| `ProjectPanel.tsx:4201` | `<span class="agent-modal-title">Close orchestrator?</span>` |
| `SettingsModal.tsx:835` | `" (orchestrator)"` |
| `SettingsModal.tsx:1659` | `"Orchestrator idle: badge yellow threshold must be a whole number of at least 1 minute"` |
| `SettingsModal.tsx:1662` | `"Orchestrator idle: badge red threshold must be a whole number of at least 1 minute"` |
| `SettingsModal.tsx:1665` | `"Orchestrator idle: yellow threshold must be below the red threshold"` |
| `SettingsModal.tsx:1671` | `"Orchestrator idle: auto-close minutes must be a whole number of at least 1"` |
| `SettingsModal.tsx:1861` | `On start, wake orchestrators that were awake when the app closed` |
| `SettingsModal.tsx:1899` | `Orchestrator idle` |
| `SettingsModal.tsx:1984` / `:1987` | `The idle badge shows minutes since your last message to an orchestrator` / `configured minutes; the orchestrator stays as a dormant row you can` |
| `SettingsModal.tsx:2000` | `Always close team members when manually closing Orchestrator` |
| `SettingsModal.tsx:2026` | `orchestrators and Root; other agents opt in per agent)` |
| `SettingsModal.tsx:2089` | `Allow authorized Root Agents and same-workgroup Orchestrators to capture live terminal` |
| `TeamContextAlertsEditor.tsx:79` | `percentage, AgentsCommander sends that workgroup&apos;s orchestrator an informational notice` |
| `WorkgroupGroupRail.tsx:254`, `:255` | `"An orchestrator raised its hand"` |
| `loop-event-toast.ts:26` | `` `Loop "${name}" skipped because the orchestrator is busy` `` |
| `loop-event-toast.ts:32` | `` `Loop "${name}" is pending until the orchestrator is idle` `` |
| `stores/coordinator-close.ts:62` | `name: s?.name ?? "this orchestrator",` |

No file in `src/` is renamed, no `data-ac-testid` value changes, no CSS class changes, no signal, prop or store key changes.
### 5.4 Rust and crate edits

Apply Rule T to every site of §3.3 (a) to (d). The decisions that Rule T alone does not settle:

- **Placeholders inside generated Markdown become the new word.** `<coordinator>` in the Root message-filename pattern and `<coordinator_name>` in the two Root `send --to` examples are descriptive placeholders for a peer's name, not literals the CLI validates: the CLI validates the `YYYYMMDD-HHMMSS-root-to-<wgN>-<name>-<slug>.md` **shape**, not the word. `<coordinator>` becomes `<orchestrator>` at `config/session_context.rs:3534` and `:3619` (T1 matches both, since `<` and `>` are valid boundaries). `<coordinator_name>` becomes `<orchestrator_name>` at `:3179` (inside `ROOT_PTY_INPUT_CONTEXT`) and `:3622`; T1 excludes both because the next character is `_`, so they are the first two of the three T1a sites.
- **The `log::*` twins move with their `eprintln!`.** `cli/close_session.rs:209` (`log::error!`) carries the same sentence as the `eprintln!` at `:213-215`, and `cli/list_peers.rs:659` (`log::warn!`) carries the same sentence as the `eprintln!` whose text is at `:654` (the macro spans `:653-656`); §3.3(b) cites the same site by its text line `:654`. Each pair is one message with two sinks and is renamed together. Every other `log::*` payload is out of scope (§4).
- **`cli/loop_cmd.rs:326` and `cli/workgroup.rs:194` keep their flag names.** The result is `"--force-inject-when-busy conflicts with --busy-coordinator values other than force-inject"` and `"--coordinator is required when supplying team details on workgroup add"`, both **unchanged**. T1 does match the trailing `coordinator` of `--busy-coordinator` and of `--coordinator`; what preserves them is **Rule P0**, because the `clap` parser resolves both flag names (§5.2 derived instance **3**), and neither sentence contains any other match. Both lines are listed for completeness and are **not edited**; they are Phase 3 residuals.
- **The five `clap` `about` doc comments of `cli/mod.rs` (§3.3 (a)) are plain Rule T, and their exact after-text is:**

  | Line | After |
  | --- | --- |
  | `:140` | `/// Show this session's raised-hand communication indicator in the Sidebar orchestrator row` |
  | `:158` | `/// Close all sessions for a target agent (orchestrator authorization required)` |
  | `:160` | `/// Purge every agent in the caller's own workgroup (orchestrator-only, fail-closed busy gate)` |
  | `:163` | `/// Set the title field in the workgroup TASK.md frontmatter (orchestrator-only)` |
  | `:165` | `/// Append text to the body of the workgroup TASK.md (orchestrator-only)` |

  Each line carries exactly one T1 occurrence and nothing else on it moves: the leading indentation, the `///`, and every other word are byte-identical. T1 matches `coordinator-only` because the trailing `-` is outside `[A-Za-z0-9_]`, and T1 matches the other three on space and `(` neighbours. No indefinite article precedes any of the five, so **T2 does not fire on any of them**. The variant identifiers (`RaiseHand`, `CloseSession`, `PurgeWg`, `TaskSetTitle`, `TaskAppendBody`), every `#[command(...)]` attribute and every subcommand name (`raise-hand`, `close-session`, `purge-wg`, `task-set-title`, `task-append-body`) stay exactly as they are; `cli/mod.rs` contributes five changed lines to the diff and nothing else.
- **`phone/types.rs:206` and `agentscommander-api-helper.rs:684`** become `"An orchestrator cannot target an orchestrator on this route."` under T2. Their match arms `C::TargetIsCoordinator` and `"target_is_coordinator"` do not change.
- **`config/seeded_context_templates.rs:460`** becomes `label: "Orchestrator context"`. This label is rendered by `ContextTemplateUpdateModal` and by the sidebar update list. The spec `id` stays `"coordinator"` and the `filename` constant stays `Context.coordinator.md`.

The full replacement body of `get_default_coordinator_template()` (`config/session_context.rs:2508-2530`) is the current body with these five lines changed and nothing else:

| Body line | New text |
| --- | --- |
| 1 | `You are the orchestrator for your team. You must:` |
| 5 | `- To reach another workgroup, message its orchestrator, never its members, and only when your role, the user, or the Root Agent authorizes it; replying to an orchestrator who messaged you first is always authorized.` |
| 17 | `- Interactive desktop orchestrator: PowerShell System.Drawing / CopyFromScreen can work; cast Measure-Object results to [int] before passing dimensions to Bitmap.` |
| 18 | `- Sandboxed harness orchestrator: CopyFromScreen may return all-zero/black pixels; then ask the user to capture with Greenshot, use the latest file from C:\Users\maria\0_greenshot\, and visually inspect the image content before sending.` |
| 24 | `This shows the Sidebar raised-hand indicator for your orchestrator row; it clears when the user interacts with your session.` |

The resulting template measures **len 2516, sha256 `0b89eb38608f6272f0d8087fc7df13ecc729fda716aba972673b15b734a2198e`**. That is a checkable postcondition, not a pinned assertion: if the implementer's edit produces a different digest, the edit deviated from Rule T.

The full replacement body of `ROOT_ROLE_MD` (`config/root_agent.rs:615-663`) is the current body with three lines changed and nothing else:

| Body line | New text |
| --- | --- |
| 9 (file `:623`) | `You are the AgentsCommander Root Agent, the top-level orchestrator for this AgentsCommander binary.` |
| 28 (file `:642`) | `Coordinate across workgroups at a high level: delegate specialized implementation work to the appropriate team orchestrators and synthesize their results for the user.` |
| 35 (file `:649`) | ``2. Create the team with `team create`, choosing one orchestrator and the worker agents.`` |

The resulting constant measures **len 2467, sha256 `7f82f28c70221c8476bb957f5978433173f60e388a9f18db729e5c2bf014c52d`**. The `## Coordination` heading at file `:640` is Rule T3 and does not change.

Two existing test invariants must keep holding after these edits and are re-asserted in §9: the coordinator template must stay free of U+2014 (`session_context.rs:8125`, `:8167`) and `ROOT_AUTHORITY_SECTION` must stay free of U+2014 (`session_context.rs:5973`). Rule T introduces no U+2014.

### 5.5 The two frozen snapshots, the recognizer wiring, and the version bumps

This is the only part of the change that can silently damage an existing installation, so it is ordered first (§12) and specified exactly.

#### 5.5.1 `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME`

- **Where.** `src-tauri/src/config/seeded_context_templates.rs`, immediately after `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE` (which ends at `:280`), so the four coordinator snapshots stay adjacent and in chronological order.
- **Name.** `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME`. The issue permits the new terminology in the identifier; the plan chooses the family's existing prefix instead, because a single odd-one-out name inside a family of four frozen snapshots makes the family unsearchable and gains nothing: Phase 2 renames all four identifiers in one sweep. Recorded as decision D6 in §10.
- **Content.** The escaped-literal **source text** of `get_default_coordinator_template()`'s body, copied verbatim from `config/session_context.rs:2509-2529` **before** §5.4 edits it, in the same `"...\n\` continuation style the three siblings already use. Copying the source text (not retyping the prose) is what makes byte identity structural rather than clerical. Continuation indentation is irrelevant to the bytes, because a Rust line continuation swallows the newline and all following leading whitespace.
- **Provenance doc comment**, mirroring `:104-112` and `:206-208`: state that the values were captured by measuring the shipped accessor at base commit `ecc6527b`, never from this constant.
- **Wiring.** `is_known_generated_coordinator_template` (the `fn` spans `:529-534`; its fourth and last arm is `:533`) gains a fifth arm immediately after `:533`:
  `|| content == COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME`.
- **Assertions**, in the existing `mod tests`, mirroring `coordinator_pre_cross_workgroup_snapshot_is_byte_exact` (`:2226-2238`):

```rust
#[test]
fn coordinator_pre_orchestrator_rename_snapshot_is_byte_exact() {
    assert_eq!(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME.len(),
        2509,
        "frozen v4 coordinator snapshot must be the ecc6527b bytes"
    );
    assert_eq!(
        hash_text(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME),
        "f6ef7894b9f0f606e945c282d769144e96487fcc01ab435c9aab8019bb3ce1f6",
        "frozen v4 coordinator snapshot changed; it must stay byte-identical to what shipped"
    );
}
```

- **Migration proof**, mirroring `read_sync_updates_pre_token_minimization_coordinator_template` (`:2243`): a test named `read_sync_updates_pre_orchestrator_rename_coordinator_template`. **Assertion order is load-bearing and is specified here** (§9.1 BL4 explains why):
  1. `assert_ne!(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME, get_default_coordinator_template())`. First, because this is T2's failing-first property and it does not depend on the recognizer arm at all.
  2. Write the frozen bytes to a temp `.ac/Context.coordinator.md`, run the same `sync_for_read_at` helper, and assert the file **auto-upgraded** to the new default with exactly one publication.
  3. `assert!(is_known_generated_coordinator_template(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME))` **last**.

  Putting the direct recognizer assert last is a deliberate departure from the sibling at `:2243`, which asserts it first. With the fifth arm deleted, a first-position direct assert panics before the sync runs, so the probe would only prove that a predicate whose one matching arm was just removed returns false. Asserting the **behavior** first makes probe leg 3 prove that the sync path actually consumes the recognizer, which is the silent-half-migration risk the probe exists to close.

#### 5.5.2 `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD`

- **Where.** `src-tauri/src/config/root_agent.rs`, immediately after `ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD` (which ends at `:613`) and before `ROOT_ROLE_MD` (`:615`).
- **Content.** The raw-literal body of `ROOT_ROLE_MD` copied verbatim **before** §5.4 edits it, as `const ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD: &str = r#"..."#;`.
- **Wiring, both lists.** This is the trap: two overlapping lists of the same constants exist in two places, and they are **not** the same list. `is_known_generated_root_context_template` (`:669-681`) holds **seven** array entries at `:672-678`; `migrate_root_role` (`:984-989`) holds **six** disjuncts and deliberately omits `OLD_ROOT_CONTEXT_WITH_COORDINATION_MD`, whose body is instead handled by the `OLD_DEFERRED_MESSAGING_PARAGRAPH` replacement branch at `:996-1000`. Do not "correct" the six into a seven; the difference is intentional. Add one entry to each:
  1. `is_known_generated_root_context_template`: insert `normalize_role_text(ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD),` as a new array element immediately **before** the existing `normalize_role_text(ROOT_ROLE_MD),` entry at `:678`. After the edit the new element is at `:678` and `normalize_role_text(ROOT_ROLE_MD)` is at `:679`.
  2. `migrate_root_role`: insert `|| existing_normalized == normalize_role_text(ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD)` as a new disjunct immediately **before** the existing `|| existing_normalized == normalize_role_text(ROOT_ROLE_MD)` disjunct at `:989`. After the edit the new disjunct is at `:989` and the `ROOT_ROLE_MD` disjunct is at `:990`.

  Missing either one is a silent half-migration. Test T4 drives both paths from one fixture, exactly as `frozen_v5_root_context_is_recognized_and_migrated_on_both_paths` (`:2352-2391`) does, with **one deliberate deviation from that model: the assertion order**. The model puts `assert!(is_known_generated_root_context_template(...))` first, at `:2354-2356`, before the fixture is even written. T4 puts it **last**, after both migration assertions:

  1. write the frozen bytes as `Role.md` and as `Context.root-agent.md` in one temp fixture, then `ensure_root_agent_dir_at`;
  2. assert `Role.md` reduced to `MINIMAL_ROOT_ROLE_MD` (this is the `migrate_root_role` disjunct);
  3. assert `Context.root-agent.md` auto-upgraded to `default_root_context_template()` (this is the recognizer array element);
  4. `assert!(is_known_generated_root_context_template(ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD))` last.

  The reason is §9.1 BL4: with the array element deleted, a first-position direct assert panics before `ensure_root_agent_dir_at` runs, so probe leg 1 would report `assertion failed: is_known_generated_root_context_template(...)` and would prove only that a predicate whose one matching entry was just deleted returns false. In this order each probe leg fails on the behavioral assertion for the path it disabled, which is what §14 item 2 actually claims. Note also the timing caveat resolved in §9.1: **T4 can only discriminate after §5.4 has been applied**, because until then the new constant is byte-identical to `ROOT_ROLE_MD` and the pre-existing `ROOT_ROLE_MD` entries satisfy T4 on their own. The mutation probe in §12 step 2 is what actually proves both entries are load-bearing.
- **Assertions.** A byte-exactness test pinning `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD.len() == 2464` and its sha256 `e244249ccd2fa832918d1f830939ca4dae37a3594461334fa6d7004ba430e74f`. `root_agent.rs` has no `hash_text` helper; the test computes the digest with the `sha2::Sha256` already used by `short_sha256` (`:1370`), or reuses `short_sha256` and pins its prefix. Either is acceptable; the digest and the length must both be pinned.
- **Not affected, checked:** `max_migratable_len` / `legacy_snapshots` (`:61`, `:1278`) belong to the default Root **skill** files (`role-skill-boundary-audit`, `agency-agents-roles`), not to `Role.md` or the context template, and neither skill body contains `coordinator`. No change there.

#### 5.5.3 Version bumps

Following the convention of §3.5:

- `seeded_context_templates.rs:461`, coordinator spec `current_version: 4` becomes `5`. Update the two pins: `:2039` (`assert_eq!(coordinator.current_version, 4)` becomes `5`; the binding `coordinator` does **not** change, Rule P0) and `:3239-3242` (the expected `currentVersion` becomes `5`, and the message line changes as pinned below).
- `seeded_context_templates.rs:475`, `root_spec()` `current_version: 6` becomes `7`. Note that `root_spec()` is declared in `seeded_context_templates.rs`, not in `root_agent.rs`; AC6's G2 set depends on that. Update the two pins at `root_agent.rs:2339-2342` and `:2387-2390`, whose value and message lines both move.
- The global spec (`:451`) is unchanged at 4.

**The three pin messages, pinned exactly.** §5.1 claims nothing is left to the implementer, and round 2 left these three strings unstated, which was the one place that was false. Current text on the left, replacement on the right; nothing else on those lines changes.

| Site | Current message | New message |
| --- | --- | --- |
| `seeded_context_templates.rs:3241` | `"coordinator current_version must be bumped to 4 by the v4 rewrite"` | `"coordinator current_version must be bumped to 5 by the #1571 orchestrator rename"` |
| `root_agent.rs:2341` | `"root_spec current_version must be bumped to 6 by the #1370 workgroup-activation rewrite"` | `"root_spec current_version must be bumped to 7 by the #1571 orchestrator rename"` |
| `root_agent.rs:2389` | `"root_spec current_version must be bumped to 6 by the #1370 workgroup-activation rewrite"` | `"root_spec current_version must be bumped to 7 by the #1571 orchestrator rename"` |

The two root messages are byte-identical to each other before and after, as they are today. The leading `coordinator` and `root_spec` tokens in those messages name the spec `id` and the function, so Rule P0 preserves them; only the version number and the trailing provenance clause move. Note which half of Rule P0 does that work: it is the **referential** test, because the spec `id` and the function both resolve. Read positionally, an assertion message is prose a human reads and the whole message would be RENAME. This is a clean application of the rule and it is further evidence for the round-4 direction recorded in D12. These three lines are AC6's G3 set and AC7 exception 2.

`currentVersion` is a generation counter written into the seeded-template state file and reported to the UI; no code branches on it, so the bump changes no behavior. Not bumping would leave one number describing two different template bodies. Recorded as decision D7 in §10 with the rejected alternative.

#### 5.5.4 The four things that must all be true afterwards

1. A user's `.ac/Context.coordinator.md` that is byte-identical to the pre-rename default is still `is_known_generated_coordinator_template`, and therefore auto-upgrades to the new default instead of being treated as customized.
2. A user's Root `Context.root-agent.md` byte-identical to the pre-rename `ROOT_ROLE_MD` still auto-upgrades.
3. A user's Root `Role.md` byte-identical to the pre-rename `ROOT_ROLE_MD` is still reduced to `MINIMAL_ROOT_ROLE_MD` by `migrate_root_role`.
4. The three pre-existing coordinator snapshots and the six pre-existing Root snapshots are byte-identical to `main`, provable by `git diff` showing no change inside their literals.
### 5.6 Documentation (36 files, 296 matching lines)

#### 5.6.1 The rule

Apply Rule T to every `coordinator` occurrence in the 36 files below, **except** the occurrences enumerated in §5.6.2, which are preserved verbatim. The preserve decision is made per occurrence, not per line: a line may keep an identifier and still have its prose renamed.

Files and their current matching-line counts: `PRIVACY.md` 2, `README.md` 7, `ROADMAP.md` 5, `docs/agent-matrix-conventions.md` 13, `docs/agents/creating-agents.md` 1, `docs/agents/inter-agent-messaging.md` 11, `docs/agents/teams-and-workgroups.md` 21, `docs/assets/capture-guide.md` 2, `docs/concepts.md` 6, `docs/features/README.md` 2, `docs/features/container-coding-agents.md` 2, `docs/features/context-tracking.md` 3, `docs/features/control-plane-api.md` 2, `docs/features/project-loops.md` 26, `docs/features/seed-manifest.md` 2, `docs/features/session-auto-close.md` 34, `docs/features/sidebar-guide.md` 4, `docs/features/terminal-snapshots.md` 12, `docs/glossary.md` 9, `docs/home-en.md` 8, `docs/integrations/coding-agents.md` 1, `docs/quickstart.md` 11, `docs/reference/architecture.md` 3, `docs/reference/cli.md` 33, `docs/reference/directory-layout.md` 5, `docs/reference/settings.md` 14, `docs/security.md` 13, `docs/style-guide.md` 1, `docs/testing/03-agent-lifecycle.md` 2, `docs/testing/04-team-and-workgroup-lifecycle.md` 14, `docs/testing/05-end-to-end-user-journey.md` 3, `docs/testing/07-terminal-sessions.md` 1, `docs/testing/destructive-filesystem-regression.md` 2, `docs/testing/semantic-ui-automation-affordance-matrix.md` 5, `docs/troubleshooting.md` 2, `docs/use-cases.md` 14.

Four documentation decisions Rule T does not settle:

- **Quoted UI copy follows the UI.** Where a doc quotes a string this plan renames, the quote is updated with it. Exactly these: `docs/features/project-loops.md:25` (`` `Workgroup Coordinator` `` and `` `Select a coordinator...` ``), `:28` and `:91` (`` `Force inject even if coordinator is busy` ``), `:32` (`` `A workgroup with a verified coordinator is required.` ``), `:78` and `:117` (`` `Loop "<name>" is pending until the coordinator is idle` ``), `:79` (`` `Loop "<name>" skipped because the coordinator is busy` ``); `docs/features/sidebar-guide.md:10` (`` `Show recent coordinators first` ``, `` `Show coordinators in default order` ``) and `:38` (`` `A coordinator raised its hand` `` becomes `` `An orchestrator raised its hand` ``); `docs/agent-matrix-conventions.md:34` (`` `# Coordinator Context` `` becomes `` `# Orchestrator Context` ``, while `` `.ac/Context.coordinator.md` `` on the same line stays).
- **`docs/agent-matrix-conventions.md:232`.** The sentence "Shown with `COORDINATOR` badge in sidebar" describes the sidebar badge, whose rendered text is lowercase. It becomes "Shown with `orchestrator` badge in sidebar", matching what §5.3 makes the badge actually say. The `` `coordinator` `` key name on the same line stays.
- **Illustrative placeholder names are renamed.** `docs/agent-matrix-conventions.md:421` `_agent_COORDINATOR/` becomes `_agent_ORCHESTRATOR/` (a placeholder in a family with `_agent_WORKER_1`, `_agent_REVIEWER`); `docs/features/session-auto-close.md:149` `"<coordinator cwd>"` becomes `"<orchestrator cwd>"`; `docs/testing/03-agent-lifecycle.md:112` `` `regression-coordinator-<timestamp>` `` becomes `` `regression-orchestrator-<timestamp>` ``; `docs/agents/teams-and-workgroups.md:60` `# coordinator replica` and `docs/use-cases.md:13` / `:51` `# coordinator — Claude Code` are role annotations inside directory-tree blocks and are renamed. The dash in that last quotation is U+2014 in the tree, and the quotation is corrected to the source bytes at round 7 because §1.1 tells an implementer to re-anchor on quoted text when a line number drifts; `:51` continues `, runs for hours` after the quoted fragment. None of these is a value any code reads.
- **`docs/glossary.md`.** The entry `## Coordinator` (`:49-51`) is renamed to `## Orchestrator` and **moved** to its alphabetical position, between `## Non-stop mode` (`:77-79`) and `## Portable instance` (`:81-83`). Its body is rewritten by Rule T. No inbound anchor link to the glossary's `#coordinator` exists anywhere in the repository, so the heading may move without updating a link. **The check is `git grep -n -i -E '#[a-z0-9-]*coordinator' -- . ':(exclude)plans/'` over every tracked file, and not round 6's `git grep -n "#coordinator"` over `*.md`**, which is narrower than the risk: anchoring the needle at the token makes it structurally unable to match an anchor whose slug carries other words before it. Widened at round 7. At `ecc6527b` the widened needle returns exactly **one** line in the whole repository, and it is not a glossary link: `docs/features/terminal-snapshots.md:433`, `[Container coding agents](container-coding-agents.md#terminal-snapshots-from-a-container-coordinator)`, whose target heading is `docs/features/container-coding-agents.md:32`, `## Terminal snapshots from a container Coordinator`. **The rule this states, binding on every phase: a link and its target heading are renamed in the same phase, and neither moves without the other.** Both files are in this section's 36, so Rule T moves both here and the link still resolves; under round 6's text that was true by luck, because nothing checked that a link and its target land in the same phase. Nine documentation headings carry the token and this is the only one with an inbound anchor. After the change the widened needle must return **zero** lines, because its one match moved with its heading. This is a documentation-only check run at step 6; it does not become an acceptance criterion. The other **seven** glossary lines (`:39`, `:59`, `:71`, `:107`, `:123`, `:131`, `:143`, that is the file's 9 matching lines minus `:49` and `:51`) are Rule T prose. The epic's Definition of Done item 5 ("`docs/glossary.md` defines Orchestrator and no longer defines Coordinator") is satisfied here.

#### 5.6.2 The documentation preserve list: 59 lines, exhaustive

After the change, `git grep -n -i coordinator -- "*.md" ":(exclude)plans/" ":(exclude)CHANGELOG.md" ":(exclude)src-tauri/" ":(exclude)src/"` must return **exactly these 59 lines and no others**. Each keeps `coordinator` only inside the named identifier.

| File | Lines | Preserved because |
| --- | --- | --- |
| `docs/agent-matrix-conventions.md` | 31, 34, 54 | `Context.coordinator.md` file name |
| `docs/agent-matrix-conventions.md` | 199, 213, 232, 237, 430 | `coordinator` team-config JSON key |
| `docs/agents/teams-and-workgroups.md` | 23 | `"coordinator"` JSON key |
| `docs/agents/teams-and-workgroups.md` | 128, 144, 182 | `--coordinator` CLI flag |
| `docs/agents/teams-and-workgroups.md` | 199 | `restore_coordinator_wake_state` settings key |
| `docs/features/project-loops.md` | 49 | `--busy-coordinator` CLI flag |
| `docs/features/seed-manifest.md` | 17, 18 | `Context.coordinator.md`, `context:coordinator` scope |
| `docs/features/session-auto-close.md` | 50, 52, 64, 72, 80, 81, 82, 83, 84, 85, 87, 95, 103, 115, 159 | `coordinatorIdleBadgeYellowMinutes`, `coordinatorIdleBadgeRedMinutes`, `coordinatorAutoCloseEnabled`, `coordinatorAutoCloseMinutes`, `coordinatorAutoCloseSkipTelegramAssigned`, `coordinatorCascadeCloseEnabled` settings keys |
| `docs/features/session-auto-close.md` | 148 | `coordinator_clock_updated` event name |
| `docs/features/session-auto-close.md` | 151 | `coordinator_clocks.json` file name |
| `docs/features/terminal-snapshots.md` | 168 | `"project:wg-1-team/coordinator"` example FQN. Preserved by **Rule P0 clause R2** (§5.2), which is a **phasing** ground and not a referential one: nothing resolves this FQN today, and Phase 2 renames the 16 Rust fixture occurrences carrying the same spelling across `crates/` and `src-tauri/` in step with it (R8). `coordinator` is this line's only T1 occurrence, so the line does not enter the diff and AC7 never pairs it |
| `docs/reference/architecture.md` | 466 | `selectionCoordinatorBusy` (Concept B) |
| `docs/reference/architecture.md` | 731 | Concept B's own row in the module table. `session/selection.rs` is one of the four Concept-B files §3.3 excludes, and Phase 2 renames `SelectionCoordinator` to **`SelectionArbiter`**, not to Orchestrator. Preserved on the **phasing** ground of Rule P0 clause R2 rather than a referential one: nothing resolves the bare `coordinator` here, so Rule P0 alone renames it, and renaming it would ship a line calling a type an orchestrator that never becomes one and oblige Phase 2 to correct a word this phase changed. `coordinator` is this line's only T1 occurrence, so the line does not enter the diff and AC7 never pairs it. Added at round 7 as decision **D13** and residual **R12** |
| `docs/reference/architecture.md` | 777 | `config/coordinator_clocks.rs` source path |
| `docs/reference/cli.md` | 368, 382, 481 | `--coordinator` CLI flag |
| `docs/reference/cli.md` | 395, 461 | `coordinator` JSON key |
| `docs/reference/cli.md` | 879, 880 | `--busy-coordinator` CLI flag |
| `docs/reference/directory-layout.md` | 42, 43, 109 | `Context.coordinator.md`, `Context.coordinator.md.bak`, `context:coordinator` |
| `docs/reference/directory-layout.md` | 76 | `coordinator_clocks.json`, `config/coordinator_clocks.rs` |
| `docs/reference/settings.md` | 274, 486 | `restoreCoordinatorWakeState`, `startOnlyCoordinators` settings keys |
| `docs/reference/settings.md` | 282, 283, 284, 285, 286, 287 | `coordinator*` settings keys |
| `docs/testing/destructive-filesystem-regression.md` | 176, 235 | `--coordinator` CLI flag inside a command line |
| `docs/testing/semantic-ui-automation-affordance-matrix.md` | 34 | `actionBar.sortCoordinators` testid |

That is 59 lines. Everything else in the 296 loses its last `coordinator` occurrence. Of the 59, **17** still enter the diff, because a line may keep an identifier and have its prose renamed; §5.2.1's last bullet enumerates all 17 and derives the documentation edit-line figure of 254 from them.

**The two ambiguous lines, decided: `docs/agent-matrix-conventions.md:199` and `:430` are PRESERVED, unedited, and neither is one of the two lines round 7 moved.** Round 1 left this open. The two lines are

```
199: ├── config.json      # REQUIRED — defines members, coordinator, and repos
430:     ├── config.json               # Lists all agents, coordinator, repos
```

Both are directory-tree comments that enumerate the fields `config.json` contains, and in both the sibling terms in the same enumeration are the literal key names (`repos` at both sites, `agents` at `:430`, matching the JSON shown at `:213` and the key table at `:232`). A reader who follows either comment into the file is looking for keys, and the key is `coordinator` until Phase 3 renames it. `coordinator` is the only match on either line, so preserving it means **neither line appears in the diff at all**, which is the least ambiguous outcome for both AC2 and AC7. The looser word `members` at `:199` is prose and is not a key, but it is also not a term this plan renames, so it does not force the other reading. Recorded as decision D10 in §10.

### 5.7 Website (14 files, 47 lines)

Branch `refactor/1571-orchestrator-rename-visible-text` in `mblua/agentscommander_webpage`, off `85f318d3`, delivered as its own pull request against that repository's `main` and cross-linked to #1571. The website repository has no branch-name workflow and no pull-request CI (§3.7).

#### 5.7.1 The localisation decision (issue decision 4)

The issue's acceptance criterion 3 fixes the two primary answers: the site renders **Orchestrator** in English and **orquestador** in Spanish. This plan extends that to the four remaining locales that carry the word, and resolves the one ambiguous Spanish case:

| Locale | Key | Current | New |
| --- | --- | --- | --- |
| en | both | `coordinator` / `COORDINATOR` | `orchestrator` / `ORCHESTRATOR` |
| es, translated common noun | both | `coordinador` / `COORDINADOR` | `orquestador` / `ORQUESTADOR` |
| es, English role name quoted beside other untranslated English role names (`worker`) | prose | `coordinator` | `orchestrator` |
| pt | `composer.coordinator` | `COORDENADOR` | `ORQUESTRADOR` |
| pt | `workspace.checkout.role` | `tech lead · coordenador` | `tech lead · orquestrador` |
| fr | `composer.coordinator` | `COORDINATEUR` | `ORCHESTRATEUR` |
| fr | `workspace.checkout.role` | `tech lead · coordinateur` | `tech lead · orchestrateur` |
| de | `composer.coordinator` | `KOORDINATOR` | `ORCHESTRATOR` |
| de | `workspace.checkout.role` | `Tech Lead · Koordinator` | `Tech Lead · Orchestrator` |
| zh | `composer.coordinator` | `协调者` | `编排者` |
| zh | `workspace.checkout.role` | `技术负责人 · 协调者` | `技术负责人 · 编排者` |

The four `workspace.checkout.role` rows are the round-2 addition (§3.7). Each keeps its locale's existing case convention on that key: pt and fr are lower case beside a lower-case `tech lead`, de is title case beside `Tech Lead`, and zh keeps the person-form noun that `协调者` already used. Each of the four forms is the same word the same locale's `composer.coordinator` value now carries, so `WorkspaceMock.astro` renders one vocabulary in every locale rather than two. All eight substitutions are rows of Rule T (§5.1), not AC7 exceptions.

The split inside Spanish is deliberate and satisfies both halves of acceptance criterion 3. `src/i18n/landing.ts:160` (`tech lead · coordinador`) and `:176` (`COORDINADOR`) are translated Spanish nouns and become `orquestador` / `ORQUESTADOR`. `src/content/builderLab.es.ts:279` and `:345` ("roles de coordinator y worker") and `src/pages/alternatives/workspace.astro:113` ("Definí coordinator, workers, roles") quote the English role vocabulary next to `worker` / `workers`, which stays English; there the term becomes `orchestrator`, keeping the existing bilingual convention. `src/content/builderLab.es.ts:93` (`badges: ["coordinador", ...]`) is a rendered Spanish badge and becomes `"orquestador"`. After the change, none of `coordinator`, `coordinador`, `coordenador`, `coordinateur`, `koordinator` or `协调者` appears anywhere in the repository except the eight lines carrying the preserved i18n key `composer.coordinator`.

#### 5.7.2 The exact edits

| File | Line | Change |
| --- | --- | --- |
| `src/components/Capabilities.astro` | 14, 34, 35, 76 | Rule T on the prose (`a coordinator` at `:14` becomes `an orchestrator`) |
| `src/components/CoordinationDemo.tsx` | 15 | comment; renamed with the block for readability (the only comment this plan touches, because it sits inside a file whose visible strings all change) |
| `src/components/CoordinationDemo.tsx` | 19, 20, 21, 22, 23, 24 | demo message-file slugs shown in the animation: `coordinator-to-dev-rust-assign` becomes `orchestrator-to-dev-rust-assign`, and so on for the other five |
| `src/components/CoordinationDemo.tsx` | 34, 36 | node `name` and `label`: `'orchestrator'`, `'orchestrator: Codex profile, GPT 5.5 xhigh'` |
| `src/components/CoordinationDemo.tsx` | 117 | `aria-label` prose |
| `src/components/CoordinationProof.astro` | 16 | `A coordinator can send` becomes `An orchestrator can send` |
| `src/components/Handoff.astro` | 28, 35 | displayed message file names: `20260628-213014-orchestrator-to-dev-rust-assign.md`, `20260628-213122-dev-rust-to-orchestrator-done.md` |
| `src/components/Install.astro` | 22 | `add a coordinator and a worker` becomes `add an orchestrator and a worker` |
| `src/components/TeamSetup.astro` | 8, 31 | prose |
| `src/components/Workflows.astro` | 13, 23 | prose (`A coordinator splits` becomes `An orchestrator splits`) |
| `src/components/Workflows.astro` | 14, 24 | visible chips: `['orchestrator', 'dev-a', 'dev-b']`, `['orchestrator', 'worker', 'worker']` |
| `src/components/alternatives/TeamComposer.astro` | 24, 25 | **no change**: `composer.coordinator` is the i18n key |
| `src/components/alternatives/WorkspaceMock.astro` | 116 | mock label `orchestrator:` |
| `src/content/builderLab.en.ts` | 92 | `badges: ["orchestrator", "Codex", "profile B"]` |
| `src/content/builderLab.en.ts` | 278, 343 | prose |
| `src/content/builderLab.es.ts` | 93 | `badges: ["orquestador", "Codex", "perfil B"]` |
| `src/content/builderLab.es.ts` | 279, 345 | `roles de orchestrator y worker` |
| `src/i18n/landing.ts` | 55, 71, 160, 176, 262, 278, 365, 381, 470, 486, 570, 585 | 12 values per §5.7.1, six under `workspace.checkout.role` (55, 160, 262, 365, 470, 570) and six under `composer.coordinator` (71, 176, 278, 381, 486, 585); both **keys** stay |
| `src/pages/alternatives/workspace.astro` | 113 | `Definí orchestrator, workers, roles y perfiles de lanzamiento.` |
| `tests/builder-lab.spec.ts` | 282 | `await expect(evidence).toContainText("orchestrator");` |

No file in the website repository is renamed. `CoordinationDemo.tsx`, `CoordinationProof.astro` and the `Coordination*` component names are Rule T3 words and Phase 2 work.
---

## 6. Affected surfaces, exhaustively

### 6.1 `repo-AgentsCommander`, production

| File | What changes |
| --- | --- |
| `src/sidebar/components/AcDiscoveryPanel.tsx` | 2 badge letters |
| `src/sidebar/components/ActionBar.tsx` | 1 line, 2 tooltip strings |
| `src/sidebar/components/EditLoopModal.tsx` | 4 lines |
| `src/sidebar/components/EditTeamModal.tsx` | 1 tooltip |
| `src/sidebar/components/NewLoopModal.tsx` | 5 lines |
| `src/sidebar/components/NewTeamModal.tsx` | 1 tooltip |
| `src/sidebar/components/ProjectPanel.tsx` | 8 lines |
| `src/sidebar/components/SettingsModal.tsx` | 12 lines |
| `src/sidebar/components/TeamContextAlertsEditor.tsx` | 1 line |
| `src/sidebar/components/WorkgroupGroupRail.tsx` | 2 lines |
| `src/sidebar/loop-event-toast.ts` | 2 lines |
| `src/sidebar/stores/coordinator-close.ts` | 1 line (file name unchanged) |
| `src-tauri/src/cli/close_session.rs` | `after_help`, the `eprintln!` and its `log::error!` twin |
| `src-tauri/src/cli/list_peers.rs` | `after_help` / `long_about` notes, the `eprintln!` and its `log::warn!` twin |
| `src-tauri/src/cli/mod.rs` | 5 `#[derive(Subcommand)]` variant doc comments, which `clap` derive prints as `about` text |
| `src-tauri/src/cli/purge_wg.rs` | `after_help` |
| `src-tauri/src/cli/raise_hand.rs` | `after_help` |
| `src-tauri/src/cli/send.rs` | `after_help`, 4 error strings |
| `src-tauri/src/cli/task_append_body.rs` | `after_help`, 1 error string |
| `src-tauri/src/cli/task_set_title.rs` | `after_help`, 1 error string |
| `src-tauri/src/cli/team.rs` | 2 error strings |
| `src-tauri/src/commands/entity_creation.rs` | 4 error strings |
| `src-tauri/src/config/injected_messages.rs` | `CONTEXT_ALERT_DOC_COMMENT` |
| `src-tauri/src/config/loops.rs` | 1 error string |
| `src-tauri/src/config/root_agent.rs` | 3 lines of `ROOT_ROLE_MD`, 1 new frozen constant, 2 recognizer-list entries, **2** version pin pairs (`:2339-2342` and `:2387-2390`) |
| `src-tauri/src/config/seeded_context_templates.rs` | `label`, **both** `current_version` lines (`:461` coordinator spec and `:475` `root_spec()`), 1 new frozen constant, 1 recognizer arm |
| `src-tauri/src/config/session_context.rs` | heading, 2 PTY-input blocks, the coordinator template, delegated-reporting section, Root authority section, 5 lines of the current messaging renderer |
| `src-tauri/src/loops/delivery.rs` | 3 message strings |
| `src-tauri/src/phone/mailbox.rs` | 2 `Err(&str)` + 21 `format!` messages |
| `src-tauri/src/phone/types.rs` | 2 `safe_detail` arms |
| `src-tauri/src/pty/inject.rs` | 1 message |
| `src-tauri/src/session/context_alerts.rs` | 7 messages |
| `crates/session-bridge/src/bin/agentscommander-api-helper.rs` | 2 reason-detail strings |

### 6.2 `repo-AgentsCommander`, tests

Frontend, exactly 9 files / **16** lines: `src/sidebar/App.context-template-updates.test.tsx:28`, `:77`; `src/sidebar/components/ContextTemplateUpdateModal.test.tsx:21`, `:66`, `:71`; `src/sidebar/stores/project.context-template-updates.test.ts:19`; `src/sidebar/stores/coordinator-close.test.ts:112` (test name), `:124`; `src/sidebar/components/WorkgroupGroupRail.raise-hand.test.tsx:182`; `src/sidebar/components/NewLoopModal.test.ts:139`; `src/sidebar/components/ProjectPanel.idle-badge.test.tsx:32`; `src/sidebar/components/SettingsModal.automation.test.ts:1688`, `:1928`; `src/sidebar/components/ProjectPanel.collapse-state.test.tsx:301` (test name), `:312`, `:318`.

Rust, the known set (the suite is the oracle, §9.3): `config/session_context.rs:5450`, `:5482`, `:5483`, `:5834`, `:5863`, `:5900`, `:8025`, `:8036`, `:8137`, `:8153`, `:8331`, `:8481`, `:8482`, `:8561`, `:8562`, `:9611`, `:10416`; `config/seeded_context_templates.rs:2039`, `:3239-3242`, `:3704`, `:3714`, `:3724`, plus the two new tests of §5.5.1; `config/root_agent.rs:2339-2342`, `:2387-2390`, plus the two new tests of §5.5.2; `commands/entity_creation.rs:4492`, `:6381`, `:6404`; `phone/mailbox.rs:15701`, `:15711`, `:15733`, `:15852`, `:15861`; `session/context_alerts.rs:2947`.

Two of those, `:5450` and `:5483`, were added at round 6, and both are plain `#[test]` assertions over live `default_context(...)` output, so both go red and §9.3 closes them. `:5450` is `assert!(out.contains("YYYYMMDD-HHMMSS-root-to-<wgN>-<coordinator>-<slug>.md"))` inside `default_context_root_agent_renders_root_messaging_exception` and pins `:3534` and `:3619`. `:5483` is `assert!(out.contains("Origin coordinators and non-coordinator WG replicas are not valid Root Agent targets in #277"))` and pins `:3607`; it sits directly beneath the already-listed `:5482`, inside the same `fn default_context_root_agent_documents_verified_wg_coordinators_only`.

**Four lines in that set have no oracle behind them, and the reviewer should know which.** `config/session_context.rs:10416` hand-builds a `"# Coordinator Context"` separator inside `mod token_accounting`'s `#[test] #[ignore] fn token_accounting_report` (the module opens at `:10216`, the test at `:10353`). The suite never runs an `#[ignore]` test, and the test's only assertion is `assert!(!value.is_empty(), ...)` at `:10463`, so after §5.4 this line can silently disagree with production and **nothing goes red**. Three more have the same property for a different reason, being **negative** assertions that stay green whether or not the literal inside them is edited: `:5863`, `assert!(!out.contains("## Privileged PTY Input to Workgroup Coordinators"))`; `:8025`, `assert!(!non_coordinator_content.contains("# Coordinator Context"))`; and `:8331`, `assert!(!content.contains("# Coordinator Context"))`. §6.2 introduces the Rust set as "the suite is the oracle"; for these four the oracle cannot fire, and **AC1 is the only thing that catches them**: needle 30 reaches `:8025`, `:8331` and `:10416`, and needle 31 reaches `:5863`. All four are listed correctly and must still be edited; the point is that leaving any of them out would produce a green suite. A sweep of the whole Rust tree for negative assertions carrying a T1-matching literal returns six; the other three are `root_agent.rs:2041` and `:2746`, recorded in §9.2, and `:2053`, recorded in §5.2, and all three are correctly outside the edit set.

### 6.3 `repo-AgentsCommander`, documentation

36 Markdown files (§5.6.1). `docs/glossary.md` additionally has one section moved.

### 6.4 `repo-agentscommander_webpage`

13 files edited (§5.7.2). `src/components/alternatives/TeamComposer.astro` is listed in the issue but is **not** edited: both of its occurrences are the i18n key.

### 6.5 The preserve set, stated as files that must NOT appear in the diff

`src-tauri/src/session/selection.rs`, `src-tauri/src/commands/resource_monitor.rs`, `src-tauri/src/resource_monitor/watchdog.rs`, `src-tauri/src/commands/window.rs`, `CHANGELOG.md`, everything under `plans/` except this plan file, `scripts/*.ps1`, `src/shared/ipc.ts`, `src/shared/types.ts`, `src/shared/coordinator-badge.ts`, `src/sidebar/components/coordinator-badge-class.ts`, `src/sidebar/components/SessionItem.tsx`, `src/sidebar/styles/sidebar.css`, `src-tauri/src/config/teams.rs`, `src-tauri/src/config/settings.rs`, `src-tauri/src/config/coordinator_clocks.rs`, `src-tauri/src/config/seed_manifest.rs`, `src-tauri/src/config/instance_artifacts.rs`, `src-tauri/src/commands/ac_discovery.rs`, `src-tauri/src/commands/session.rs`, `src-tauri/src/commands/pty.rs`, `src-tauri/src/session/auto_close.rs`, `src-tauri/src/session/manager.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/cli/loop_cmd.rs`, `src-tauri/src/cli/workgroup.rs`, `src-tauri/src/testability/ui_automation.rs`.

If the implementer finds a reason to touch any file in §6.5, that is a scope question for the coordinator, not an implementer decision.

---

## 7. Required behavior, edge cases, failure behavior

1. **Nothing observable changes except the words.** No control flow, no condition, no data structure, no serialization, no timing. Every edit is a string literal body, a Markdown body, a new `const`, an added arm in an existing `match`/boolean chain, or an integer literal that is only ever written to state and reported.
2. **A pristine pre-rename coordinator template auto-upgrades.** Existing installation, `.ac/Context.coordinator.md` byte-identical to the shipped pre-rename default: `is_known_generated_coordinator_template` returns true through the new fifth arm, the sync path publishes the new default, the seeded-state entry records `currentVersion: 5` and the new `lastSeededSha256`. Without the new arm the file is classified as customized, auto-update stops forever and the user is silently stranded. That is the failure this plan exists to prevent.
3. **A customized coordinator template is still customized.** Any byte difference from all five recognized bodies keeps the file untouched and raises the usual pending-update notice; the new arm widens recognition by exactly one body and by nothing else.
4. **A pristine pre-rename Root context template auto-upgrades, and a pristine pre-rename Root `Role.md` still reduces to `MINIMAL_ROOT_ROLE_MD`.** Both paths go through the seventh list entry; both are driven from one fixture by the same test, so wiring only one of the two lists fails loudly.
5. **The frozen historical bodies keep their meaning.** The three older coordinator snapshots and the six older Root snapshots stay byte-identical, so a user still sitting on a v1/v2/v3 body upgrades exactly as before.
6. **Legacy `CLAUDE.md` recognition is untouched.** `legacy_rendered_default_context_for_generation` and `extract_legacy_skills_section` still reconstruct the pre-#1072 output byte for byte, because none of their literals is edited. A previously generated `CLAUDE.md` is still classified `Current` or `StaleGenerated` exactly as before; only newly rendered output carries the new word.
7. **Freshly materialized context.** After the change, every new or refreshed `CLAUDE.md` carries `# Orchestrator Context`, the orchestrator template body, and the orchestrator wording in the privileged-PTY-input, delegated-reporting, Root-authority and messaging blocks. Agents already running keep the context they were started with until their session context is regenerated; that is the existing behavior of every context change and needs no special handling.
8. **A user who edited `Context.coordinator.md` and left the word in it** keeps their file. No migration rewrites user prose. The epic's zero-occurrence gate applies to the repository, not to user data.
9. **The injected-message file.** `CONTEXT_ALERT_DOC_COMMENT` is a regenerated comment block and is not part of `known_default_sha256`, so renaming it cannot make an operator's customized template look default or vice versa.
10. **Failure behavior is unchanged everywhere.** Every renamed string is a message, never a discriminant: no code compares against any of them. The only comparison-bearing constants in the diff are the two new frozen snapshots, which only ever widen a recognizer.
11. **Filter vocabulary.** After the rename, typing `coordinator` in the sidebar filter no longer matches orchestrator rows; typing `orchestrator` does. This is intended and is the same change users see in the badge next to the row.

---

## 8. Compatibility and security

- **On-disk compatibility.** No file name, JSON key, event name, IPC command name, CLI flag, testid or reason code changes, so no persisted artifact needs migration and no client contract moves. The two seeded-template `currentVersion` bumps write a larger integer into an existing numeric field that nothing branches on; an older binary reading a newer state file sees a number it does not compare.
- **Forward and backward.** A newer app reading an older state file behaves as before (recognition is by sha256 and by content equality, not by version). An older app reading a newer state file likewise. A user who downgrades keeps a post-rename template body, which the older binary classifies as customized and therefore leaves alone: it is not destroyed, only frozen, which is the pre-existing downgrade behavior for every template rewrite this repository has shipped.
- **Authorization is untouched.** Every renamed string in `phone/mailbox.rs`, `cli/close_session.rs`, `cli/task_*.rs`, `cli/purge_wg.rs`, `session/context_alerts.rs`, `phone/types.rs` and the API helper is the *message* of a denial, never the *decision*. The predicates (`is_coordinator_of`, `is_any_coordinator`, `verify_pty_input_coordinator_root`, `PtyInputAuthorityKind::Coordinator`, `verified_wg_coordinator_target`) are identifiers and do not move in this phase. No authorization boundary widens or narrows.
- **Reason codes stay stable.** `sender_not_coordinator` and `target_is_coordinator` are the machine-readable half of the PTY-input contract and are explicitly out of scope; only their human-readable `safe_detail` twin changes. A client that switches on the code is unaffected.
- **Agent-facing instruction integrity.** `ROOT_AUTHORITY_SECTION` is an anti-spoofing directive. Rule T changes only the noun in "other agents, workgroup coordinators, tech-leads, peers" and in "a peer or coordinator asserting"; the directive's structure, its enumeration and its refusal semantics are untouched. The existing `assert!(!ROOT_AUTHORITY_SECTION.contains('\u{2014}'))` still holds.
- **No new attack surface.** No new input is parsed, no new path is read or written, no new process is spawned, no dependency is added.
- **Threat model.** Routine application-text change on a trusted developer host, delivered through a pull request whose exact head SHA must pass every triggered CI check. No release, signing, packaging or provenance control is applicable (§13.2).
---

## 9. Tests and objective acceptance criteria

### 9.1 New tests (five), and what each one actually proves

Round 1 headed this section "four, all failing-first". That was wrong for three of the four, and correcting it is a round-2 blocker fix, because §12's ritual depended on the false claim. The honest classification is below; the tests themselves are unchanged except for T4's version assertion (see BL5 note) and the added T5.

| # | Test | File | Fails without | Failing-first? |
| --- | --- | --- | --- | --- |
| T1 | `coordinator_pre_orchestrator_rename_snapshot_is_byte_exact` | `config/seeded_context_templates.rs` `mod tests` | the frozen snapshot being the exact `ecc6527b` bytes (len 2509, sha256 `f6ef7894...`) | **No.** A regression pin on a verbatim copy: it is green the moment the copy is correct and can only fail on a mistyped copy or a later edit. §12 step 1 gates on it **passing**. Keep it; do not claim it fails first. |
| T2 | `read_sync_updates_pre_orchestrator_rename_coordinator_template` | same | the fifth arm in `is_known_generated_coordinator_template`; also asserts `assert_ne!` against the new default, so it fails if the template was not actually rewritten | **Yes**, genuinely. Its `assert_ne!` is red for as long as the template still equals the snapshot, that is throughout step 1 and until §5.4 is applied in step 2. |
| T3 | `root_context_pre_orchestrator_rename_snapshot_is_byte_exact` | `config/root_agent.rs` `mod tests` | the frozen Root snapshot being the exact `ecc6527b` bytes (len 2464, sha256 `e244249c...`) | **No**, same shape as T1. |
| T4 | `frozen_v6_root_context_is_recognized_and_migrated_on_both_paths` | same, modelled on `frozen_v5_...` at `:2352-2391` **but with the direct recognizer assert moved last** (§5.5.2, and BL4 below) | **either** list entry, but only after §5.4 (see the resolution below): it writes the frozen bytes as `Role.md` **and** as `Context.root-agent.md` from one fixture, then asserts the role reduced to `MINIMAL_ROOT_ROLE_MD` (proves the `migrate_root_role` entry), then the template auto-upgraded to `default_root_context_template()` (proves the recognizer entry), then the recognizer predicate directly | **Not before §5.4.** Load-bearing only afterwards, and proved so by the mutation probe in §12 step 2, not by observing it red. |
| T5 | `old_coordinator_raise_hand_snapshot_is_byte_exact` | `config/seeded_context_templates.rs` `mod tests`, mirroring T1 | `OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.len() == 2066` and `hash_text(...) == "31d49d02c12fcc8cd2d5277455074dcae3fbc1a84f1f1a0cf0f37828e03f792f"` | **No**, a pin. Added because it is the one coordinator snapshot with no byte pin today, its first line is the very sentence Rule T rewrites 60 lines away, and the existing test §9.2 leans on cannot detect an edit to it (see §9.2). |

**BL5, step 2's exit gate.** T4 must **not** assert `templates.rootAgent.currentVersion == 7`, even though the `frozen_v5_...` test it is modelled on asserts `== 6`. The root version bump is §12 step 3, so a `== 7` assertion inside T4 made round 1's step 2 gate ("T1 to T4 green") unreachable by construction: at the end of step 2 the value is still 6 and T4 is red no matter what the recognizers do. Nothing is lost by dropping it: the bump is already pinned twice, at `root_agent.rs:2339-2342` and `:2387-2390`, and both pins are updated in step 3. T4 asserts only `is_known_generated_root_context_template(ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD)` and the two migration outcomes.

**BL4, why T2 and T4 assert in a different order than the tests they are modelled on.** Both models put a direct recognizer assert first in the body: `frozen_v5_...` at `root_agent.rs:2354-2356`, before the fixture is written, and `read_sync_updates_pre_token_minimization_coordinator_template` likewise. If T4 and T2 copied that, mutation-probe legs 1 and 3 would panic on that first assert and never reach the migration code, and two things would follow. The recorded PR evidence would not match what §12 tells the implementer to expect; and, more seriously, each leg would prove only that a predicate whose one matching entry was just deleted returns false. **It would not prove that the auto-upgrade path consumes that predicate**, which is precisely the silent-half-migration risk the probe exists to close, and the risk §14 item 2 names as the second of the three things a reviewer should attack. So T4 and T2 assert **behavior first, predicate last** (§5.5.2 and §5.5.1 give the exact orders). Nothing is lost: the direct assert still runs, still fails if the wiring is missing, and now fails after the behavioral assertion has already reported which path broke. Leg 2 was correct under either order and is unchanged.

**BL6, the T4 failing-first contradiction, resolved from the tree.** Two reviewers disagreed about whether T4 is failing-first. Both are describing the same code at different moments, and the plan asserted the property at the wrong one:

- **Before §5.4 is applied**, `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD` is byte-identical to `ROOT_ROLE_MD`, so `normalize_role_text` of the two is the same string. The pre-existing `normalize_role_text(ROOT_ROLE_MD)` array element (`root_agent.rs:678` before the edit) and the pre-existing `|| existing_normalized == normalize_role_text(ROOT_ROLE_MD)` disjunct (`:989` before the edit) satisfy both of T4's substantive assertions on their own. Deleting **both** new entries at that moment leaves T4 green. So T4 is not exercising the new wiring at all before §5.4, and "see T4 fail first" proves nothing about the trap §14 item 2 names.
- **After §5.4 is applied**, the two constants diverge by three lines, the `ROOT_ROLE_MD` entries no longer match the fixture, and the two new entries become the only members that can. From that point T4 genuinely cannot pass with either list unwired.

Therefore: the ritual moves. §12 step 2 no longer asks the implementer to watch T4 go red; it asks for an explicit mutation probe **after** §5.4, which is the only sequence that proves the claim. T2's failing-first property is unaffected and is still observed directly.

Expected values for T1, T3 and T5 come from §3.6 and Grinch's independently validated decoder, captured by measuring the shipped constants at `ecc6527b` from the git blob, never from the new constants. The implementer re-derives them once, before editing, with **two** throwaway tests, not one: `ROOT_ROLE_MD` is a private `const` in `config::root_agent` and is not reachable from `seeded_context_templates`'s `mod tests`, so one throwaway is needed per module. Both hosting modules already have what is needed (`hash_text` at `seeded_context_templates.rs:2052`; `sha2::Sha256` at `root_agent.rs:1370`, used by `short_sha256`), and `get_default_coordinator_template` is `pub fn` at `session_context.rs:2508`. Print `get_default_coordinator_template().len()` / digest, `OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND.len()` / digest and `ROOT_ROLE_MD.len()` / digest (`cargo test ... -- --nocapture`, output redirected to a file, because `cargo test` stdout is otherwise swallowed), confirm all three match, then delete both throwaways. If a value disagrees, STOP: the base is not `ecc6527b` or the copy is not verbatim.

### 9.2 Existing tests that must stay green **without being edited** (negative controls)

- `coordinator_pre_token_minimization_snapshot_is_byte_exact` and `coordinator_pre_cross_workgroup_snapshot_is_byte_exact` (`seeded_context_templates.rs:2209`, `:2226`). These two are real byte pins: a failure means a frozen snapshot was edited.
- `old_coordinator_default_is_known_generated_without_raise_hand` (`:2190`) must also stay green and unedited, but **do not treat it as a byte pin**. Its `is_known_generated_coordinator_template(OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND)` assertion is self-referential (the recognizer compares the constant against itself, so it is true for any content), and its three `contains()` assertions do not cover the first line, which is the very sentence Rule T rewrites elsewhere. A Rule-T slip inside that constant leaves all four assertions green. T5 (§9.1) closes that hole; this bullet records why T5 exists.
- **Measured but not pinned in this phase.** Four further frozen constants have no byte pin. AC6's diff reading covers them, and adding four more tests is out of proportion for Phase 1, so their values are recorded here instead so a later phase can pin them in one line each. Measured at `ecc6527b` with the decoder validated in §3.6 and independently by a reviewer against five published controls:

  | Constant | len | sha256 |
  | --- | --- | --- |
  | `OLD_ROOT_ROLE_MD` | 1399 | `6aa155d9a25615adca72005da78eff7348d39e0f1ab5d7be4db773fbabeb91b3` |
  | `ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD` | 2045 | `10b9475d2f0975b0495bb24b0bd88fa61e60be71a6210e1c37cde6abf994d738` |
  | `ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD` | 2763 | `b0e58c01608dddb7515dcf233bb679731a6f2a555ddffc95e513fd5af52fc655` |
  | `ROOT_COORDINATION_MESSAGING_PARAGRAPH` | 897 | `fc2164a2a56957e481debca460f9df3cc681a634edda58f5270939c85668f207` |

  The last of those is the D4 hazard, and it lives in a file this phase edits. If the implementer or the verification pass wants one more line of insurance for the cost of one assertion, that is the one to take.
- Every `legacy_rendered_default_context` / `classify_legacy_rendered_default_context` / `looks_like_generated_legacy_default_context` test in `session_context.rs`. A failure means a legacy reconstruction literal was edited. **A test that pins legacy-reconstruction output does not fail under this change and must not be edited**; only tests that pin *current* output do.
- The em-dash guards: `session_context.rs:8125`, `:8167` ("coordinator template must stay em-dash-free") and `:5973` (`ROOT_AUTHORITY_SECTION`).
- `root_agent.rs:2041` and `:2746`, which assert `ROOT_ROLE_MD` / the migrated role does **not** contain `"verified workgroup coordinator replicas only"`. They stay true and unedited: `ROOT_COORDINATION_MESSAGING_PARAGRAPH` is not renamed (§3.4).
- Every test in `src-tauri/src/session/selection.rs` and the other three Concept-B files.
- `config/injected_messages.rs:1484` (`DEFAULT_CONTEXT_ALERT_TEMPLATE.chars().count() == 125`) and its `known_default_sha256` reconciliation tests.

### 9.3 The rule for updating existing test expectations

After applying §5, run the full Rust suite and the full vitest suite. For each failure:

- If the failure is a literal-expectation mismatch on a string this plan renames, apply **Rule T to the expectation** and nothing else.
- If the failure is anything else, STOP and report it. It is not a rename failure.

No test is deleted, skipped, renamed beyond Rule T, or restructured. The only test edits allowed outside Rule T are the five new tests of §9.1, the two `currentVersion` pin values, and the two `it(...)` titles at `ProjectPanel.collapse-state.test.tsx:301` and `coordinator-close.test.ts:112`, which are updated so a test named after "Coordinators" is not asserting "Orchestrators".

### 9.4 Objective acceptance criteria

**AC1, no renamed literal survives.** Searched **case-sensitively** and as a fixed string, `git grep -F -- "<needle>" -- . ':(exclude)plans/' ':(exclude)CHANGELOG.md'` returns zero matches for each of these 38 needles. Each is written with enough surrounding punctuation to exclude the comments and identifiers this phase deliberately leaves alone, so a zero result is a real result and not an accident of a loose substring:

1. `A workgroup with a verified coordinator is required.`
2. `Always close team members when manually closing Coordinator`
3. `Close coordinator?`
4. `"Coordinator idle: `
5. `>Coordinator idle<`
6. `On start, wake coordinators that were awake when the app closed`
7. `Select a coordinator...`
8. `Workgroup Coordinator</label>`
9. `Force inject even if coordinator is busy`
10. `Prompt to inject into the coordinator`
11. `title="Set as coordinator"`
12. `A coordinator raised its hand`
13. `Show recent coordinators first`
14. `Show coordinators in default order`
15. `label: "Coordinator context"`
16. `" (coordinator)"`
17. `same-workgroup Coordinators`
18. `"this coordinator"`
19. `Coordinator must be one of the selected agents`
20. `no coordinator found for WG`
21. `Root Agent can only message verified WG coordinator replicas`
22. `Only verified WG coordinator replicas may message the Root Agent`
23. `Only coordinators of the target agent`
24. `Coordinator is busy; delivery`
25. `has no identity-verified coordinator`
26. `not the verified workgroup coordinator.`
27. `A coordinator cannot target a coordinator`
28. `Cannot remove the current coordinator`
29. `coordinator of team(s)`
30. `# Coordinator Context`
31. `Privileged PTY Input to Workgroup Coordinators`
32. `>coordinator</span>`
33. `>Coordinators</span>`
34. `Sidebar coordinator row`
35. `(coordinator authorization required)`
36. `(coordinator-only, fail-closed busy gate)`
37. `TASK.md frontmatter (coordinator-only)`
38. `body of the workgroup TASK.md (coordinator-only)`

**Needles 34 to 38 are the five `clap` `about` doc comments of `cli/mod.rs`, added at round 7 (§3.3 (a)), and they are the whole reason this plan needed a seventh round.** Nothing else in it reached those five lines: **zero** of needles 1 to 33 match any of them, AC2's path filter excludes `src-tauri/`, and AC4 through AC9 never look at printed help, so every criterion went green on round 6 while the shipped CLI printed the retired word ten times. That is the silent-pass shape §14 exists to prevent, caught only because §12 step 8 puts a human in front of a built binary. Each needle is written long enough to be unique, which is why 37 and 38 carry sentence context rather than the bare `(coordinator-only)` that `:163` and `:165` share. Measured at `d6f1b2d6`: each of the five returns **exactly one** match, all in `src-tauri/src/cli/mod.rs`, so each is a live needle that can fail rather than one that is already zero.

Two needles are deliberately **not** on that list because a zero result would be wrong, and they get their own expected counts instead:

- `You are the coordinator for your team` must return **exactly four** matches, all in `src-tauri/src/config/seeded_context_templates.rs`: `:80`, `:214`, `:252` and the new frozen snapshot. Its **three** occurrences in `session_context.rs` (`:2509`, `:8482`, `:8562`) must all be gone: `:2509` is the shipped template body, and `:8481-8482` and `:8561-8562` pin *current* rendered output from `materialize_agent_context_file`, not legacy reconstruction, so all three are renamed under §9.3 and none collides with §9.2.
- A bare `Coordinator idle` still matches source comments, test names and the `.gitignore` marker at `config/instance_artifacts.rs:299`, all of which are out of scope for this phase (§4). Needles 4 and 5 cover the two visible strings precisely.

**AC2, the documentation preserve set is exact.** `git grep -n -i coordinator -- "*.md" ":(exclude)plans/" ":(exclude)CHANGELOG.md" ":(exclude)src-tauri/" ":(exclude)src/"` returns exactly the 59 lines listed in §5.6.2, no more and no fewer. Every returned line carries the identifier named there.

**AC3, the website preserve set is exact.** In `repo-agentscommander_webpage`, the needle must be **multilingual**, because the round-1 needle `coordinator|coordinador` is blind to four rendered locale values and would go green while the site still shows `coordenador`, `coordinateur`, `Koordinator` and `协调者` (§3.7). The website has no pull-request CI, so this is the only load-bearing gate there and it must be able to fail. Run:

```
git grep -n -i -E "coordinator|coordinador|coordenador|coordinateur|koordinator|协调者"
```

It returns 47 lines across 14 files at `85f318d3` and must return **exactly 8** lines after the change, all of them the i18n key `composer.coordinator`: `src/i18n/landing.ts:71`, `:176`, `:278`, `:381`, `:486`, `:585` and `src/components/alternatives/TeamComposer.astro:24`, `:25`. The needle is the role noun `协调者`, not the bare `协调` prefix, so Rule-T3 "coordination"-sense Chinese prose is not swept in; there is none today.

As a second, wider control that must also hold: `git grep -n -i -E "coordin|coorden|koordin|协调"` returns 80 lines at `85f318d3` and must return 80 minus 39, that is **41** lines afterwards (the 47 in-scope lines fall to 8; the 33 Rule-T3 words and `Coordination*` identifiers are untouched). If that number moves differently, a Rule-T3 word was renamed or an in-scope line was missed.

**AC4, Concept B is untouched.** `git diff --name-only main...HEAD` contains none of `src-tauri/src/session/selection.rs`, `src-tauri/src/commands/resource_monitor.rs`, `src-tauri/src/resource_monitor/watchdog.rs`, `src-tauri/src/commands/window.rs`. Their match counts are still 482 / 63 / 61 / 22.

**AC5, no rename and no out-of-scope file.** `git diff --find-renames --summary main...HEAD` is empty (no `rename` line, no `create`, no `delete` except this plan file). `git diff --name-only main...HEAD` contains no file from §6.5 and no file under `plans/` except `plans/1571-orchestrator-rename-visible-text.md`.

**AC6, the frozen constants are provably untouched.**

Round 1 stated this criterion as a list and it omitted three lines. Round 2 patched the list and it omitted a fourth. So the criterion is stated here as a **derivation the reviewer re-runs**, and the enumeration below is its output, shown so a mismatch is visible without re-deriving.

*The derivation.* In each of the two files, the set of removed (`-`) lines is exactly the union of three generated sets, and nothing else:

- **G1, renamed literals in a line this plan enumerates**: every line at which §5.4, §5.5 or §6.2 mandates a Rule-T substitution in this file. Excludes every line inside a frozen constant, because §4 puts all of them out of scope.
- **G2, the `current_version` integers this plan bumps** (§5.5.3), being every `current_version: N,` line whose spec this plan bumps, **in whichever file the spec is declared in**. That last clause is the one both prior rounds lost: `root_spec()` is declared in `seeded_context_templates.rs`, not in `root_agent.rs`.
- **G3, the pins of those integers**: every assertion line that carries one of the bumped values or its message.

*The output at `ecc6527b`.* In `git diff main...HEAD -- src-tauri/src/config/seeded_context_templates.rs`, the removed lines are exactly **nine**:

| Line | Set | Removed text |
| --- | --- | --- |
| `:460` | G1 | `label: "Coordinator context",` (production, coordinator spec) |
| `:461` | G2 | `current_version: 4,` (coordinator spec) |
| `:475` | G2 | `current_version: 6,` (**`root_spec()`**, which lives in this file, not in `root_agent.rs`) |
| `:2039` | G3 | `assert_eq!(coordinator.current_version, 4);` |
| `:3240` | G3 | the `currentVersion` value line of the pin at `:3239-3242` |
| `:3241` | G3 | that pin's message line |
| `:3704`, `:3714`, `:3724` | G1 | `label: "Coordinator context".to_string(),`, three test fixtures |

Nothing inside `OLD_COORDINATOR_CONTEXT_TEMPLATE_BEFORE_RAISE_HAND`, `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION`, `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE` or the four `GLOBAL_*` / `STANDALONE_*` constants appears as a removed line.

In `git diff main...HEAD -- src-tauri/src/config/root_agent.rs`, the removed lines are exactly **seven**: the three `ROOT_ROLE_MD` body lines `:623`, `:642` and `:649` (G1), and the value and message line of **each** of the two `currentVersion` pin blocks, `:2340`/`:2341` and `:2388`/`:2389` (G3). `grep -n current_version src-tauri/src/config/root_agent.rs` returns only those two message lines, which is the check that the file holds no third pin. Nothing inside `OLD_ROOT_ROLE_MD`, `OLD_ROOT_CONTEXT_WITH_COORDINATION_MD`, `ROOT_COORDINATION_MESSAGING_PARAGRAPH`, `ROOT_CONTEXT_BEFORE_BOUNDARY_AUDIT_MD`, `ROOT_CONTEXT_BEFORE_AGENCY_SKILL_MD`, `ROOT_CONTEXT_BEFORE_TOKEN_MINIMIZATION_MD` or `ROOT_CONTEXT_BEFORE_WORKSPACE_PROSE_MD` appears as a removed line.

Both new frozen constants are pure insertions and contribute no removed line, which is why they cannot hide inside either count.

**AC7, the per-occurrence Rule-T diff proof: how the reviewer proves no serialized literal changed.**

Round 1 stated this as whole-line equality, `ruleT(old) === new`. That form is broken in both directions and is replaced here.

*Why the round-1 form had to go.* T1's boundary rule excludes an occurrence only when a neighbour is `[A-Za-z0-9_]`, and almost every preserved identifier has punctuation boundaries. So a whole-line `ruleT` rewrites the identifiers it was supposed to protect.

- **False negative, the serious half.** `docs/reference/directory-layout.md:42` is `` | `Context.coordinator.md` | Coordinator context template (seed scope `context:coordinator`) | Seeded by AC | ``. §5.6.1 renames the middle prose and §5.6.2 preserves both code spans, but a whole-line `ruleT` also rewrites `Context.coordinator.md` and `context:coordinator`. An implementer who renames the filename and the scope too produces a line that **equals** `ruleT(old)` exactly, so AC7 passes and the shipped documentation names an on-disk file and a seed-manifest scope that do not exist. The criterion sold in §14 as stronger than a grep waves through exactly the class of defect it exists to catch.
- **False positive, the noisy half.** Doing the same line correctly **fails** the round-1 form, and so does every other line that carries a T1-matching PRESERVE occurrence beside renamed prose. §5.2.1 measures that set rather than listing it from memory: in documentation it is exactly six, `directory-layout.md:42` and `:43`, `agent-matrix-conventions.md:34` and `:232`, `teams-and-workgroups.md:182`, `cli.md:481`; in the website repository it is the six `src/i18n/landing.ts` lines `:71`, `:176`, `:278`, `:381`, `:486` and `:585`, where the preserved key `composer.coordinator` sits on the same line as the renamed value; and in `src/` it is `ProjectPanel.tsx:1016`. Note what the round-1 form did **not** trip on, because it explains why counting by eye undercounts: five further preserve-list lines (`teams-and-workgroups.md:199`, `session-auto-close.md:64` and `:83`, `settings.md:274` and `:285`) also enter the diff with two occurrences each, but both of theirs are prose, and the identifier that keeps them on the preserve list is T1-**excluded** camelCase or snake_case.
- **Third class.** Where the neighbour *is* alphanumeric, a whole-line `ruleT` returns the line unchanged while the plan mandates a change, so the pair fails: the three T1a sites of §5.1.

*The criterion.* Run, in each repository:

```
git diff -U0 --no-color main...HEAD -- . ':(exclude)plans/'
```

**Step 1, hunk shape.** For each hunk, compare the count of `-` lines with the count of `+` lines. Pairing `-` and `+` in order is only sound when the counts are equal, so this is asserted rather than assumed. A hunk whose counts differ must be a pure insertion, a pure deletion, or one of the structural exceptions below; any other unequal hunk **fails AC7** and is not paired. (The plan's own edits never create a mixed unequal hunk, but the checker must not depend on that holding.)

**Step 2, pair and classify.** Inside each equal-count hunk, pair the `-` and `+` lines in order. For each pair `(old, new)`:

1. **Enumerate occurrences** in `old`: every occurrence of a Rule T matched token (§5.1, including the four website locale rows) that satisfies the **T1** boundary test, **plus** the three enumerated **T1a** sites.
2. **Classify** each enumerated occurrence by **Rule P0** (§5.2). **The operative test is referential, not structural.** Ask whether **something resolves the token**: code binds or reads it, a file or directory with that literal name exists on disk, a CLI parser accepts that flag, or a manifest, event, settings or i18n consumer keys on it. **PRESERVE** when something does. **RENAME** when nothing does and the token only illustrates a shape to a reader, **including when the illustration is shaped like a path, a filename, a directory, an agent name or a key**.

   Code position and key/name position (as listed in §5.2) are where resolved tokens usually sit, and they are a cheap spot-check, **but they do not decide, and a checker that classifies by position alone returns the wrong answer here**. This plan renames a directory name (`_agent_COORDINATOR/`), two file names (`Handoff.astro:28` and `:35`), an agent name (`regression-coordinator-<timestamp>`), a placeholder inside a JSON value (`<coordinator cwd>`), a key-shaped rendered label (`WorkspaceMock.astro:116`) and six path-shaped fixture slugs (`CoordinationDemo.tsx:19` to `:24`), while preserving other occupants of each of those same positions. §5.2 tabulates all seven and the preserved twin of each. A checker that used position would rebuild `expected` with `coordinator` intact on those lines, mismatch a correct implementation, find no exception, and then be told by the precedence rule below to report §5.7.2 and T1a as the defects. That is a wrong answer delivered with confidence, and it is what round 4 removed.

   **Clause R1, a test expectation is never itself a referent.** A test that asserts on a literal **pins** it, so reading by a test is not a resolution: classify the occurrence by **what the pinned literal names**, and the expectation moves with that literal under §9.3. Every §6.2 test line whose pinned literal is a message, a UI label, a badge or a template body is therefore RENAME: `entity_creation.rs:4492`, `:6381`, `:6404`; `phone/mailbox.rs:15701`, `:15711`, `:15733`, `:15852`, `:15861`; `session/context_alerts.rs:2947`; `seeded_context_templates.rs:3704`, `:3714`, `:3724`; the 16 frontend test lines; and the three `# Coordinator Context` assertions at `session_context.rs:8036`, `:8481` and `:8561`, which need no carve-out under this clause. The clause reaches one further test line that is **not** a §6.2 line, because §6.2 is `repo-AgentsCommander`, tests: in the **website** repository, `tests/builder-lab.spec.ts:282` (`toContainText("coordinator")`, pinning the rendered badge), enumerated as the 33rd of §3.7's 47 lines and given its exact result in §5.7.2. A test line is PRESERVE only when the **pinned literal itself** has a referent: `ac_discovery.rs:4098` and `:4143` (`scope = "context:coordinator"`, the seed manifest keys on it) and `root_agent.rs:2053` (`workgroup add --coordinator`, `clap` accepts it). The clause classifies an occurrence and never widens the edit set: Rule T reaches only the enumerated lines and §3.4 recognizer 4's frozen bodies stay untouched, including `seeded_context_templates.rs:265` and `:266`, whose sentences are byte-identical to the renamed `session_context.rs:2523` and `:2524`.

   **Clause R2, a referent a later phase brings into being.** A token whose referent a later phase creates or renames in step with this text is PRESERVE though nothing resolves it today. One occurrence rests on it, `docs/features/terminal-snapshots.md:168` (§5.6.2). It carries no other T1 occurrence, so the line never enters the diff and this step never pairs it; the clause is stated so a reviewer applying Rule P0 off-diff does not conclude §5.6.2 is the defect.

   The enumerations in §5.2's derived-instance list and in §5.6.2 are cross-checks a reviewer may use, not the authority: where an enumeration and Rule P0 disagree, Rule P0 governs and the enumeration is the defect to report. Round 2 stated this step as a closed lookup over those enumerations and three reviewers each found a form missing from it, so this step no longer asserts that any list is exhaustive, and it no longer asserts a collision count.

   The two positions that a checker most often gets wrong, both settled by Rule P0 rather than by a list:

   - **The quoted key form `"coordinator":`.** The test is the full form, closing quote included, and not the shorthand "followed by `:`": that shorthand misreads `src/components/CoordinationDemo.tsx:36`, `label: 'coordinator: Codex profile, GPT 5.5 xhigh'`, which is quote-preceded and colon-followed inside a rendered label that §5.7.2 renames, and which nothing resolves. In key position the form is PRESERVE: `docs/agents/teams-and-workgroups.md:23`, `docs/reference/cli.md:395`, `docs/agent-matrix-conventions.md:213` (each of those three is that line's only T1 occurrence, so none of the three lines enters the diff at all and classification never has to fire on them), and the key half of the six `src/i18n/landing.ts` `"composer.coordinator": "..."` lines, which do enter the diff on their value half. In **value** position it is RENAME: `ProjectPanel.tsx:975` and `:1016` (the synthetic sidebar filter token, D8), `src/content/builderLab.en.ts:92` (`badges: ["coordinator", ...]`), `tests/builder-lab.spec.ts:282` (`toContainText("coordinator")`), `src/components/CoordinationDemo.tsx:34` (`name: 'coordinator'`). `docs/reference/cli.md:461` is neither: its text is `` `agents`, `coordinator`, and `repos` ``, a **backticked** span naming a key, PRESERVE for that reason.
   - **A source identifier spelled exactly `coordinator` or `Coordinator`** is PRESERVE, because Phase 1 renames no identifiers. `team.coordinator` at `ProjectPanel.tsx:1016`, `coordinator.contains(...)` at `session_context.rs:8137` and `coordinator.current_version` at `seeded_context_templates.rs:2039` are the three sites where such an occurrence shares an edited line with a RENAME occurrence (§5.2.1 shows the sweep that found them, and the procedure that finds a fourth if a later edit creates one).
3. **Rebuild** `expected` from `old` by applying the Rule T substitution to exactly the RENAME occurrences (with T2 article agreement where the indefinite article is adjacent) and copying every PRESERVE occurrence, and every byte that is not an enumerated occurrence, **byte for byte**.
4. **Assert `expected === new`.**

This catches the false negative: in the `directory-layout.md:42` construction, `Context.coordinator.md` and `context:coordinator` are PRESERVE, so `expected` keeps them, and a `new` that renamed them mismatches. It removes all six false positives, because each of those lines now differs from `expected` in nothing. It absorbs the third class, because T1a occurrences are enumerated and classified RENAME. And it absorbs the website locales, because §5.7.1's forms are Rule T rows.

*The closed exception list.* A pair or hunk that still fails is a defect **unless** it is on this list, which is exhaustive:

1. `AcDiscoveryPanel.tsx:237` and `:290`, the badge letter `C` to `O`. Neither old line contains any enumerated occurrence, so `expected === old != new`. The exception is admitted **only when the pair differs in exactly that one character** (offset of the `>C<` byte); any other difference on either line is a defect, so an unrelated edit smuggled onto these two lines is still caught.
2. The version-bump pairs, which change an integer rather than a word: `seeded_context_templates.rs` `current_version: 4,` to `5,` at `:461` and `root_spec()` `current_version: 6,` to `7,` at `:475`; `assert_eq!(coordinator.current_version, 4)` to `5` at `:2039`; and the three `currentVersion` pin blocks (`seeded_context_templates.rs:3239-3242`, `root_agent.rs:2339-2342`, `:2387-2390`), each contributing a value line and a message line whose wording also moves (the exact new messages are in §5.5.3). The exception covers the **integer, and the message wording only as §5.5.3 pins it**: the three replacement messages are byte-exact there, so the admitted `new` message line must equal the pinned string exactly, and any other substitution inside a message is a defect rather than an excused difference. Round 3 excused the wording wholesale, which was broader than this plan's own specification and made a stray substitution inside a message unverifiable. The `coordinator` occurrence in `coordinator.current_version` at `:2039` is a Rule P0 PRESERVE, because the binding resolves, and is asserted byte-identical by step 3 rather than excused here.
3. `docs/agent-matrix-conventions.md:232`, `` `COORDINATOR` `` to `` `orchestrator` ``. The case does not follow Rule T, deliberately: the line describes the sidebar badge, whose rendered text §5.3 makes lowercase. The `` `coordinator` `` key on the same line is PRESERVE and needs no exception.
4. `docs/glossary.md`, the moved section: one pure-delete block and one pure-insert block, unequal by construction. **Redundant, kept for legibility**: step 1's pure-block carve-out already admits both blocks, so this entry names the case rather than creating an allowance. Removing it would change nothing.
5. Structural pure insertions with no counterpart: the two new frozen constants and their provenance comments, the fifth arm in `is_known_generated_coordinator_template`, the two new Root list entries, and the five new tests of §9.1.

Every other failing pair or hunk is a defect: it means the diff changed something other than the word, and the most likely something is a serialized literal. This is the mechanical proof the phase most needs, and over **paired** lines it inspects every changed byte and every preserved identifier inside a changed line, which is what a grep cannot do.

*What AC7 does not reach, stated so it is not over-claimed.* Step 1 admits a pure-insertion hunk without pairing, so nothing inside an inserted block is examined per occurrence. For the blocks that matter that is covered elsewhere: the two frozen constants are byte-pinned by T1 and T3, and inserted Rust is covered by compilation and the suite. The residual exposure is an inserted **documentation or website** line carrying a defect **other than** the retired word: wrong copy, a typo, a broken link. An inserted line containing `coordinator` is exactly what AC2 and AC3 do catch, since AC3 pins the website count at exactly 8 and an inserted line moves it to 9, and AC2 pins the documentation set at exactly 59 lines; round 3 gave the backwards reason here while reaching the right conclusion. Do not remove the carve-out; pairing an unequal hunk is unsound, which is why step 1 asserts the counts instead of assuming them. The correct reading is that AC7 inspects every changed byte **on a paired line**.

**AC8, the four compatibility statements of §5.5.4 hold**, proved by T1 to T5, the §12 step 2 mutation probe, and AC6.

**AC9, local gates.** At the repository root first, `npm ci` and `npm run build`: CI's `rust-regression` job runs both before any cargo command, and some cargo targets want `dist/` present, so skipping the build produces a spurious failure that is not a rename failure. Then in `src-tauri`: `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --lib --bins --tests` all pass. At the repository root: `npm run typecheck`, `npm test`, `npm run test:debt` all pass. Additionally, because they are real gates that CI does not run (§3.8): `npm run check:frontend-dependencies` reports 0 errors, and `npm run record:arcs` leaves `src-tauri/module-arcs.txt` byte-identical (§11).

**AC10, exact-head CI.** Every job triggered on the pull-request head SHA passes: `test-debt`, `rust-regression`, `rust-regression-linux`, `rust-regression-macos`, `terminal-snapshot-portable` on all four runners, `windows-release-cli-smoke`, `frontend-regression`, `validate-branch-name`. Evidence from any other SHA, a skip, a waiver or a bypass does not satisfy this. `bundle-validation`, `lockfile-check` and `version-sync-check` are path-filtered away by this diff; if the final diff touches any of their filtered paths, re-derive §3.8 before merging.

**AC11, website gates.** In `repo-agentscommander_webpage`: `npm run check` (astro check) and `npm run build` pass, and `npm run smoke` passes with the updated `tests/builder-lab.spec.ts:282`. The website repository has no pull-request CI, so this local evidence is the only evidence and must be captured in the pull request body. If Playwright browsers cannot be provisioned on the implementer's host, that is recorded as an explicit gap in the pull request, with `npm run check`, `npm run build` and AC3 standing as the substitute; the spec edit is then reviewed by diff under AC7.

**AC12, a visual check of the two new badge strings.** The role badge now renders `orchestrator` (11 to 12 characters) inside `.ac-discovery-badge.coord` and `O` inside the discovery panel. Confirm by launching the app and looking at one workgroup row and one discovery row, or by a `data-ac-testid`-driven UI-automation query on `replica.badges.*`. This is the only criterion that is not satisfiable from a diff, and it exists because the badge is the one renamed string whose width changed inside a fixed-size chip.

While the app is open, eyeball one further growing string in the same pass: `ProjectPanel.tsx:2670` `Coordinators` to `Orchestrators` sits inside a workgroup-section header that `src/sidebar/styles/sidebar.css:6531` describes as fixed-inset. It is one character longer and is not expected to wrap. No separate criterion; report it at step 5 if it does, since the remedy would be a CSS change and §6.5 forbids touching `sidebar.css` in this phase, making it a scope question for the coordinator.
---

## 10. Explicit decisions and accepted residuals

### 10.1 Decisions

**D1, one substitution rule instead of per-string copy.** Every rename is Rule T (§5.1), case-preserving, with article agreement and the "coordination is a different word" qualifier. Rejected: writing bespoke replacement copy per string. It would leave hundreds of micro-choices to the implementer, make the diff unreviewable by any mechanical test, and defeat AC7.

**D2, the scope boundary is "text a person or an agent reads at runtime".** Source comments, doc comments, `log::*` payloads without a user-facing twin, test names, fixture data, CSS class names and `scripts/*.ps1` are out. The issue's own qualifier for this phase is "error and log strings **that reach a user or an agent**"; a `log::warn!` written to `app.log` reaches neither. Bracketed log tags are module-derived (`[coordinator-clocks]` names `config/coordinator_clocks.rs`) and renaming them in Phase 1 would desynchronise a tag from the module Phase 2 renames, and would split one rename across two phases. Rejected: renaming all log payloads now.

**D3, `ROOT_ROLE_MD` and the Root recognizer are in Phase 1. SETTLED in round 1 review: keep, do not strike.** This began as an architect-added scope item beyond the issue's enumeration, offered for the coordinator to strike. All three round-1 reviewers formed the view independently and all three said keep, so the option is now closed and the round-1 strike recipe is retired (it was also incomplete: it missed AC6's `root_agent.rs` clause, AC8's reference to T3 and T4, §11's naming of the new constant, and §14 item 2). The decisive argument is Grinch's, and it is in the code rather than the prose: `ROOT_ROLE_MD` is **dual-purpose**, since `default_root_context_template()` returns it and `migrate_root_role` separately compares a user's `Role.md` against it, so the frozen-snapshot work is mandatory whenever its bytes move. Deferring does not avoid the work or the risk; it duplicates them into a phase whose reviewers are looking at identifiers (#1572) or persisted formats (#1573), not at byte-frozen recognizers. The original reasoning, unchanged, follows. The issue's section 2 names only `seeded_context_templates.rs`. The Root Agent context template is the same kind of artifact: agent-facing prose, shipped as a default, recognized by a byte-equality recognizer, migrated in place. Including it here is right because (a) Phase 1's stated requirement is every surface an agent reads, and `ROOT_ROLE_MD` tells the Root Agent it is "the top-level coordinator" and to "delegate to team coordinators", which is precisely the ambiguity the epic removes; (b) Phase 2 renames identifiers and Phase 3 renames persisted formats, so neither would touch this prose, leaving it to Phase 4 with the identical frozen-snapshot work still to do; (c) the frozen-snapshot mechanism is Phase 1's subject matter, and doing it twice in one review is far cheaper than doing it once here and once three phases later. The honest counterargument, raised and answered in review, is that D3 still leaves Root partly renamed because D4 keeps `ROOT_COORDINATION_MESSAGING_PARAGRAPH` coordinator-era; that is true with or without D3, so including D3 reduces the inconsistency surface rather than creating one.

**D4, `ROOT_COORDINATION_MESSAGING_PARAGRAPH` is not renamed.** It is interpolated into the frozen `OLD_ROOT_CONTEXT_WITH_COORDINATION_MD`, so editing it moves frozen bytes and breaks the Root recognizer, and it is also live replacement text in `migrate_root_role`. Separating the two uses means splitting one constant into a frozen copy and a live copy, which is a migration change and therefore Phase 3 or Phase 4 work. Rejected: editing it now (breaks recognition), and splitting it now (widens Phase 1 into migration code).

**D5, localisation, across both keys.** English `orchestrator`, Spanish `orquestador` for translated nouns and `orchestrator` where the Spanish copy already quotes the English role vocabulary next to `worker`, Portuguese `orquestrador`, French `orchestrateur`, German `Orchestrator`, Chinese `编排者` (§5.7.1). The issue fixed only English and Spanish; the four other locales carry the word and had to be decided rather than left. Round 2 extends the same six answers to the second key, `workspace.checkout.role`, whose pt, fr, de and zh values round 1 did not see (§3.7); each takes its locale's existing case convention on that key, and each is the same word that locale's `composer.coordinator` value now carries, so `WorkspaceMock.astro` renders one vocabulary per locale instead of two. Rejected: keeping `Orchestrator` untranslated everywhere, which contradicts acceptance criterion 3 of the issue and would leave five locales inconsistent with their own existing translation style. Also rejected: leaving the four `workspace.checkout.role` values for the implementer to translate, which is exactly the invent-copy case this plan exists to remove.

**D6, the frozen constants keep the family's existing prefix.** `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME` and `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD`. The issue permits the new terminology in the identifier; a lone `ORCHESTRATOR_*` name inside a family of four otherwise-`COORDINATOR_*` snapshots would make the family unsearchable for no gain, since Phase 2 renames the whole family at once. Rejected: `ORCHESTRATOR_CONTEXT_TEMPLATE_BEFORE_RENAME`.

**D7, `current_version` is bumped (coordinator 4 to 5, root 6 to 7).** Both prior template rewrites bumped it and pinned the bump in a test, and nothing branches on the value. Rejected: leaving it, which would make one generation number describe two different bodies and break the repository's own convention, for the sake of avoiding four one-character test edits.

**D8, the discovery badge letter and the filter token move with the word.** `C` becomes `O`; the synthetic filter token `"coordinator"` becomes `"orchestrator"` at `ProjectPanel.tsx:975` and `:1016`. Both are user-visible vocabulary. Rejected: leaving the letter (a badge reading `C` next to a row labelled `orchestrator` is a defect) and leaving the filter token (the filter would still answer to the retired word while the visible badge does not).

**No test is added for the filter token, and that is a decision, not an omission.** `ProjectPanel.regex-filter.test.tsx` never types the literal, so `:975` and `:1016` change with nothing red to confirm the new token works, and §7 item 11 records a real user-visible behavior change with no test behind it. One assertion would close it. It is not added because Phase 1 adds no behavior and changes no helper: the token is a literal argument to `matchesFilterText` / `joinSearchText`, both already covered, and the natural home for a filter-vocabulary test is Phase 2, where the surrounding identifiers move and the test would not have to be rewritten immediately. Recorded as residual R11 rather than left implied, so the coordinator can overrule it with one line of scope rather than discovering it at step 9. On `:1016` note that only the quoted token moves: `team.coordinator` on the same line is the serialized `AcTeam` field and is preserved by Rule P0.

**D9, documentation code spans are preserved unless they quote renamed UI copy.** The full list of both sides is §5.6.1 and §5.6.2. Illustrative placeholders (`_agent_COORDINATOR/`, `<coordinator cwd>`, `regression-coordinator-<timestamp>`, tree-comment role annotations) are renamed because no code reads them; every real identifier is preserved.

**D10, `docs/agent-matrix-conventions.md:199` and `:430` are preserved, not renamed.** Round 1 left these two ambiguous: they read as bare prose but §5.6.2 preserved them as the `coordinator` JSON key, so AC2 and AC7 disagreed about them either way. Decided in favour of preserving, on the ground that both are directory-tree comments enumerating the fields of `config.json` alongside the literal key names (`repos` at both, `agents` at `:430`), and a reader following either comment into the file is looking for keys. Because `coordinator` is the only match on either line, preserving means neither line appears in the diff at all, which is the least ambiguous outcome for both criteria and keeps both lines off the count round 7 moved from 58 to 59. Rejected: renaming them as prose, which would make AC2's set 56 and put two lines into the diff whose only purpose is to name a key that does not change until Phase 3. Detail in §5.6.2.

**D11, AC7 is a per-occurrence check, not whole-line equality.** Round 1's `ruleT(old) === new` form admitted a construction that renames `Context.coordinator.md` and `context:coordinator` in `docs/reference/directory-layout.md:42` and still passes, while failing six correct edits and three mandated ones. Rejected: keeping whole-line equality and growing the exception list, which would have meant enumerating every line that is both edited and carries a preserved identifier, an open-ended set that grows with every documentation edit, and which would still not have closed the false-negative hole. The chosen form composes Rule T with Rule P at the occurrence level, so a preserved identifier inside a changed line is asserted byte-identical rather than excused. Detail and the exception list in §9.4 AC7.

**D12, classification is generated by a rule, not read from a list.** Round 2 kept AC7's mechanism but had its classification step read from closed enumerations of identifier forms, and all three reviewers found a form missing: a source identifier in code position (`team.coordinator`, `coordinator.contains(...)`), and the bare quoted spelling in value position (`badges: ["coordinator", ...]`, `toContainText("coordinator")`). §5.2's own completeness sweep then found a third code-position site, `seeded_context_templates.rs:2039`, that no reviewer named. Three patches to the same enumeration in three rounds is the evidence that the enumeration was the wrong artifact. Rule P0 (§5.2) replaces it: classification is generated (a token something resolves is preserved wherever it appears; the role noun a human or an agent reads is renamed), the enumerations become derived cross-checks, and §5.2.1 states a procedure that re-derives them so a reviewer can prove closure instead of trusting it. Round 3 wrote that generating step as classification **by position**, which is the half round 4 replaced; see the correction below. Rejected: adding the three missing forms to the list, which is what round 2 did with round 1's findings and which would have left the class open for the fourth. Also rejected: dropping the enumerations entirely, since they are cheap to spot-check and make a rule violation visible without re-running the sweep. **Round 4 corrected Rule P0's own discriminator.** Round 3 opened the rule with a referential test and then let its enumerated structural positions govern, and AC7 step 2 restated only the positions, so the operative checker used the broken half. Seven occurrences this plan mandates renaming came back PRESERVE, none of them on AC7's exception list, and the positional reading cannot be repaired by adding exceptions because this plan preserves and renames pairs of occupants of the identical position (§5.2 tabulates both). Round 4's Rule P0 was therefore referential: PRESERVE when something resolves the token, RENAME when the token only illustrates a shape, including a path-shaped, filename-shaped or key-shaped shape, with one stated carve-out for agent-facing template bodies, which round 5 retired in favour of clause R1 below. Rejected: adding the seven sites as AC7 exceptions, which is the same move that failed in rounds 2 and 3, one level up. **Round 5 closed the two non-referential grounds round 4's rewrite had left unstated.** Round 4 replaced round 3's RENAME half and dropped its trailing clause "or a test expectation that pins one of those", reinstating the coverage for one sub-case only (agent-facing template bodies), so every other §6.2 test expectation fell back under Rule P0's own gloss of "resolves" as "code binds or **reads** it" and came back PRESERVE; and round 4 had no clause at all for the one occurrence preserved because a later phase owns its referent, so the rule contradicted §5.6.2 on `terminal-snapshots.md:168`. Both are now named clauses of Rule P0: **R1**, a test expectation is never itself a referent, which classifies the occurrence by what the pinned literal names and makes the template-body carve-out unnecessary rather than adding a second exception beside it; and **R2**, a token whose referent a later phase creates or renames in step with this text. Rejected: restoring round 3's clause verbatim, which classifies a test expectation by listing what it may pin and would need extending for each new pinned kind. Rejected: generalising the carve-out to "any text this plan renames", which is circular as a classification rule, since what this plan renames is Rule P0's output and cannot be its input. **Recorded as process, because round 4's regression was a deletion and not a decision:** when a rule is rewritten, the replacement is diffed against the text it replaces and every class the old text classified is confirmed still classified. Round 5's §5.2 was checked against both round 4's and round 3's before certification; the two classes round 4 dropped are the two clauses above, and no third class was lost.

**The reopen condition, widened twice.** This decision reopens on a flagged occurrence Rule P0 cannot classify, **or that it classifies against §5.6.1, §5.6.2, §5.7.2 or T1a**. The second clause was added at round 4 and is the one that matters: round 3's condition said only "cannot classify", which caught `session-auto-close.md:149` (where two positions fired with opposite verdicts) but was blind to the other six, where the rule answered confidently and backwards. A rule that returns a wrong answer without hesitating is the failure mode this condition has to catch. **§5.6.2 was added at round 5**, because round 4's list named §5.6.1, §5.7.2 and T1a and omitted it, and the single occurrence in this plan where the round-4 rule contradicted a preserve decision, `terminal-snapshots.md:168`, is a §5.6.2 line: the one condition that exists to reopen §5.2 was blind to the only site that had already tripped it. Clause R2 removes that contradiction, and the widened condition would catch a recurrence. §14 item 3 tells the reviewer to hunt for exactly this.

**D13, `docs/reference/architecture.md:731` is preserved, not renamed.** The line is `` | `session/selection.rs` | Selection contract, coordinator, eligibility policy, process epoch, revision, publication | `` and it describes Concept B. Rule P0 alone renames it: nothing resolves the bare lowercase `coordinator` there, it is not a key, a path or a flag, and round 6's §5.6.2 did not list it, so AC2 forced it to lose its match. Step 6 implemented that faithfully and then flagged the outcome, which is the right order and not implementer drift. Decided at round 7 in favour of preserving, on the ground that **Phase 2 renames `SelectionCoordinator` to `SelectionArbiter`, not to Orchestrator**: renaming here ships a documentation line calling a type an orchestrator that never becomes one, and obliges Phase 2 to correct a word this phase changed. Preserving keeps the line accurate today and lets Phase 2 move it to "arbiter" alongside the type. The ground is **phasing**, which is exactly Rule P0 clause R2, so this is an instance of an existing clause and not a new kind; the only consequential edit to the rule is that R2's occupancy goes from one occurrence to two. Rejected: renaming it as prose, which is what Rule P0 alone says and what round 6 shipped. Also rejected: adding a fresh Rule P0 clause for Concept B, which would duplicate R2 and reintroduce the position-shaped reasoning rounds 3 and 4 removed. Consequences: §5.6.2 becomes 59 lines, AC2 becomes 59, §12 step 6's gate becomes 59, `docs/reference/architecture.md` contributes one changed line to the diff instead of two, and the residual is R12. The other two `architecture.md` matches are unaffected: `:466` was already preserved and `:777` is one of §5.2.1's 17 lines that carry a renamed occurrence beside a preserved identifier.

### 10.2 Accepted residuals (each owned by a later phase, none an oversight)

| # | Residual | Owner |
| --- | --- | --- |
| R1 | 58 of §5.6.2's 59 documentation lines, still naming an on-disk identifier. The 59th is R12, which names no on-disk identifier | Phase 3 (#1573), then Phase 4 |
| R2 | 8 website lines carrying the i18n key `composer.coordinator` | Phase 2 (#1572), which the coordinator amended in round 1 to own them explicitly as its new section 3b. Round 1 review found the 8 lines owned by nobody and therefore certain to fail the epic's own zero-match gate; the routing was fixed at the issue, not here. The reason they belong with the identifier phase rather than the persisted-format phase is that an i18n key is resolved at build time and is never written into a user's config directory |
| R3 | `ROOT_COORDINATION_MESSAGING_PARAGRAPH` still writes coordinator-era prose on one legacy `Role.md` migration path (D4) | Phase 4 (#1574) |
| R4 | Every `coordinator` identifier, comment, CSS class, test name and fixture in `src/`, `src-tauri/` and `crates/`: the large majority of the 4272 baseline lines | Phase 2 and Phase 3 |
| R5 | `scripts/1283-local-session-proof-harness.ps1`, `scripts/1283-local-session-proof.psm1`, `scripts/1283-local-session-proof.tests.ps1` (56 lines) | Phase 2 or Phase 4 |
| R6 | `src/shared/ipc.ts:1039`, an error message naming the `coordinator` response key | Phase 3 |
| R7 | `cli/loop_cmd.rs:326` and `cli/workgroup.rs:194`, messages whose only occurrence is a flag name (`--busy-coordinator`, `--coordinator`) | Phase 3 |
| R8 | `docs/features/terminal-snapshots.md:168` and the `project:wg-1-team/coordinator` fixture FQNs in `crates/` and `src-tauri/` | Phase 2 |
| R9 | `config/instance_artifacts.rs:299` and `:305`, `.gitignore` marker comments written into an instance `.gitignore` | Phase 2 |
| R10 | `CHANGELOG.md` (10 lines) and `plans/` (10 files, 108 lines) | Out of the epic by epic-level decision; Phase 4 adds one new changelog entry |
| R11 | No test covers the renamed sidebar filter token (D8). The vocabulary change of §7 item 11 ships unpinned | Phase 2 (#1572), where the surrounding identifiers move and one assertion lands in its final form |
| R12 | `docs/reference/architecture.md:731`, the Concept-B module-table row, preserved by Rule P0 clause R2 so its wording moves with the type it describes (D13) | Phase 2 (#1572), which renames `SelectionCoordinator` to `SelectionArbiter` and moves this line to "arbiter" with it |

R9 is worth one sentence: those two strings are written into a generated `.gitignore` as comments, so they are technically text a user can read. They are excluded because they sit beside the artifact registry entries that Phase 2 renames with the module, and splitting a comment from the artifact it annotates is worse than leaving both for one phase.

---

## 11. Dependency-cycle and layering statement

**New module arcs added by this plan: zero.** Enumerated per edit type:

- Every frontend edit changes a string literal inside an existing JSX expression or an existing exported function. No `import`, no `export`, no new module.
- Every Rust message edit changes a string literal inside an existing function. No `use`, no `crate::` path, no new call.
- `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME` is a `const &str` declared in `config::seeded_context_templates`, referenced only from `is_known_generated_coordinator_template` and the tests in the same module. Source module == target module, so it is not a module-to-module reference at all.
- `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD` is a `const &str` declared in `config::root_agent`, referenced only from `is_known_generated_root_context_template`, `migrate_root_role` and the tests in the same module. Same argument.
- The two `current_version` literals are integers inside existing struct literals.
- The documentation and website edits are outside both graphs.

**Per-arc verdict:** there is no arc to classify. The plan neither adds nor removes a module-to-module reference, so no arc can be internal to a pre-existing SCC and none can cross a previously-clean SCC boundary. Role and layering hygiene is likewise unaffected: no lower layer gains a `tauri` / `AppHandle` / UI-transport dependency, because no dependency is added anywhere.

**Measured baseline** (clean tree at `ecc6527b`, instrument `repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs`, run 2026-08-27, exit code 1, which is the normal outcome when pre-existing gating cycles exist and does not mean the graph is missing):

| Measure | Base value |
| --- | --- |
| `summary.modulesResolved` | 191 |
| `summary.moduleEdges` | 3732 |
| `summary.functionsResolved` | 4338 |
| `summary.functionEdges` | 6837 |
| `summary.moduleCycles` | **1** |
| `summary.functionCyclesCrossModule` | 0 |
| `summary.functionCyclesIntraModule` | 1 |
| `src-tauri/module-arcs.txt` regenerated from that graph | 1037 arcs, 82149 bytes, sha256 `A93ED10E844CD18D3C2150AC53ACD8DFD704195D0783A5CA169CEB7B8C864D9E`, byte-identical to the committed file |
| `npm run check:frontend-dependencies` | PASS, 351 modules, 0 errors, 1535 dependencies |

**Acceptance criterion the implementation reviewer runs** (clean tree both times, base SHA and final branch head):

```
node "<wg>\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet
node "<wg>\repo-personal\ObsidianVault\Coding Agents\IA-Programming\rust\01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
npm run check:frontend-dependencies
```

Green if and only if:

1. `summary.moduleCycles` is 1 pre and post (it must not increase);
2. the cyclic SCC member set is identical set-to-set, module by module, not merely equal in count;
3. zero new `from -> to` pairs appear in the arc record, so the pre and post arc sets are equal;
4. the regenerated `src-tauri/module-arcs.txt` is byte-identical, sha256 still `A93ED10E844CD18D3C2150AC53ACD8DFD704195D0783A5CA169CEB7B8C864D9E`, and `git status --porcelain src-tauri/module-arcs.txt` is empty;
5. the structural layering guard tests stay green in `cargo test`;
6. `npm run check:frontend-dependencies` still reports 0 errors.

Any deviation means the diff did more than change words and must be investigated before merge. Note the caveat recorded in the repository's own instrument warnings: the detector emits `duplicate-function-node` warnings for 64 functions at the base; that count must also be unchanged, because a new duplicate would mean a function was added.
---

## 12. Implementation order and ownership

The order is load-bearing only in one place: **the two frozen snapshots must be created before the templates they freeze are edited**, because their whole value is that they are a verbatim copy of the pre-edit source text. Everything after step 2 is independent and can proceed in parallel.

| Step | Work | Owner | Gate before moving on |
| --- | --- | --- | --- |
| 0 | Entry ritual §1.2 in both repositories; re-derive the §3.6 values plus T5's with **two** throwaway tests, one per module (§9.1), and confirm they match exactly; delete both throwaways | `ac-dev-rust-v3` | all values match §3.6 and §9.1; both trees clean; both bases frozen |
| 1 | Create `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME` and `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD` as verbatim copies of the current template sources, with their provenance comments. **Do not yet edit the current templates.** Add T1, T3 and T5 | `ac-dev-rust-v3` | T1, T3 and T5 pass; `cargo check` passes |
| 2 | Wire both recognizers (one arm, two list entries) and add T2 and T4. Run: **T2 must be red** (its `assert_ne!` fails while the template still equals the snapshot) and **T4 will be green, which is expected and is not evidence** (§9.1 BL6: while the snapshot is byte-identical to `ROOT_ROLE_MD`, the pre-existing entries satisfy T4 on their own). Then apply §5.4 to `get_default_coordinator_template()` and `ROOT_ROLE_MD`. Then run the **mutation probe** below. The version bumps stay in step 3, and T4 does **not** assert `currentVersion` (§9.1 BL5), which is what makes this gate reachable | `ac-dev-rust-v3` | T1 to T5 green; the two new digests match §5.4 (2516 / `0b89eb38...`, 2467 / `7f82f28c...`); all three mutation-probe legs observed red and restored |
| 3 | Version bumps §5.5.3 and their four pin updates | `ac-dev-rust-v3` | `cargo test --lib --bins --tests` green |
| 4 | The remaining Rust and crate edits, §3.3 (a) to (d) minus the templates, plus the Rust test expectations that fail under §9.3 | `ac-dev-rust-v3` | `cargo check --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --lib --bins --tests` all green |
| 5 | The 40 frontend lines of §5.3 and the 16 frontend test lines of §6.2 | `ac-dev-webpage-ui-v3` | `npm run typecheck`, `npm test`, `npm run test:debt`, `npm run check:frontend-dependencies` all green |
| 6 | The 36 documentation files of §5.6, including the glossary rename and move | `ac-technical-writer-v3` | AC2 returns exactly the 59 lines of §5.6.2, and §5.6.1's widened anchor check returns zero |
| 7 | The website branch, the 13 files of §5.7.2, `npm run check`, `npm run build`, `npm run smoke` | `ac-dev-webpage-ui-v3` | AC3 returns exactly 8 lines |
| 8 | CLI behavior pass: run `--help` for `close-session`, `list-peers`, `list-peers-lean`, `purge-wg`, `raise-hand`, `send`, `task-append-body`, `task-set-title` and read the authorization paragraphs; provoke one denial per renamed error path where cheap | `ac-cli-tester-v3` | no `coordinator` in any printed help or error text |
| 9 | Verification pass: AC1 to AC9 and §11, plus the AC7 per-occurrence diff proof over the whole diff in **both** repositories, and §5.2.1's completeness procedure re-run over the **actual** diff rather than over the plan's enumerations | `ac-dev-rust-grinch-v3` | every criterion green; AC6's removed-line sets derive to nine and seven; every occurrence §5.2.1 flags is classified by Rule P0; the AC7 exceptions used are a subset of §9.4's five; no unexplained unequal hunk |
| 10 | Two pull requests: `mblua/AgentsCommander` (`refactor/1571-orchestrator-rename-visible-text` into `main`, closing #1571) and `mblua/agentscommander_webpage` (same branch name, into `main`, cross-linked). Exact-head CI, then merge in that order | `ac-shipper-v3` | AC10 and AC11 |

**The step 2 mutation probe (replaces the round-1 "see T4 fail first" ritual).** Run it only after §5.4 has been applied, because that is the first moment the two Root constants differ and the new entries can discriminate at all (§9.1 BL6). Three legs, each one edit, observe, restore:

1. Delete the array element `normalize_role_text(ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD),` from `is_known_generated_root_context_template`. Run T4. It must go **red on the template assertion** ("pristine Context.root-agent.md must auto-upgrade"), because the file is now classified as customized and left alone. Restore the element.
2. Delete the disjunct `|| existing_normalized == normalize_role_text(ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD)` from `migrate_root_role`. Run T4. It must go **red on the role assertion** ("pristine Role.md must reduce to MINIMAL"), because the body is no longer recognized as pristine and no migration is written. Restore the disjunct.
3. Delete the fifth arm `|| content == COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ORCHESTRATOR_RENAME` from `is_known_generated_coordinator_template`. Run T2. It must go **red on the auto-upgrade assertion**, because the pristine `.ac/Context.coordinator.md` is now classified as customized and `sync_for_read_at` publishes nothing. Name the expected failure: with nothing recognized there is no publication at all, so the panic should come from the `assert_one_publication` helper at `seeded_context_templates.rs:2079` rather than from a final content compare. §12's wording admits either, but the recorded evidence should be predictable, and a failure at the content compare instead means the publication happened and something else moved. Restore the arm.

Each leg names the assertion it must fail, and the named assertion is the **behavioral** one, not the direct recognizer predicate. That is only true because T4 and T2 assert their direct predicate last (§9.1 BL4, §5.5.2, §5.5.1). If a leg instead reports `assertion failed: is_known_generated_...`, the test was written in the model's assertion order rather than this plan's: **fix the test order and re-run the leg**, because as written that observation proves only that a predicate returns false, not that the migration path consumes it. Do not record it as a pass.

All three legs must be observed red, each on its named assertion. A leg that stays green means that entry is not load-bearing and the test is not driving the path it claims to, which is the exact silent-half-migration failure §14 item 2 exists to prevent. Record the three red runs, with the failing assertion message for each, in the pull-request body; they are the evidence for AC8, and they are not reproducible from the final diff.

Two ordering notes. Step 8 needs a build, so it runs after step 4 and can overlap steps 5 to 7. Step 10 merges the app repository first: the website is a marketing surface with no dependency on the app, but the reverse reading (site says Orchestrator, shipped app says Coordinator) is the worse intermediate state, and the app repository is the one whose CI is authoritative.

There is no broken intermediate state inside the app repository at any step boundary: steps 1 and 2 are the only ones that could produce one, and step 2 is a single commit that wires the recognizers and rewrites the templates together.

**Round 7 reopens two already-completed steps and adds none; the step numbers do not move.** Steps 0 to 3, 5 and 7 are committed and correct and are not revisited. **Step 4** reopens for the five `cli/mod.rs` lines of §3.3 (a), same owner (`ac-dev-rust-v3`), same gate, exact after-text in §5.4. **Step 6** reopens for one line, reverting `docs/reference/architecture.md:731` to its `ecc6527b` text under D13, same owner (`ac-technical-writer-v3`), against a gate that now reads 59 and adds §5.6.1's widened anchor check. **Step 8** then re-runs in full from a rebuilt binary, because its previous run is what found A1 and its gate was unreachable under round 6's §3.3 (e). **Step 9** runs over the amended plan and, per A3, should read the documentation edit-line figure as 254 rather than 238. Step 10 is unchanged.

---

## 13. Delivery nonfunctional invariants

### 13.1 Accepted task class and threat model

Routine application-text change on a trusted developer host, delivered by pull request. No release, no signing, no packaging, no publishing, no untrusted build host, no destructive or irreversible migration, no security-boundary change (§8). Tool trust is the repository-pinned toolchain: Node 22 with npm 11.6.2 as CI pins it, Rust stable via `dtolnay/rust-toolchain@stable`, `npm ci` against the committed `package-lock.json`, `cargo` against the committed `Cargo.lock`. Enhanced provenance controls (independently anchored executable hashes, DLL closure inventories, poisoned-`PATH` tests, SDK manifests) are **not applicable**; any finding that assumes a hostile host or ambient tool tampering is advisory here, not a readiness blocker.

The one place where byte-exactness genuinely matters is the two frozen snapshots, and it is answered with a trust source independent of the runtime being validated: the values in §3.6 come from the git blob at `ecc6527b` decoded by a validated decoder, cross-checked at step 0 against the shipped accessors' own output. That is a real anchor, not a self-hash.

### 13.2 Baseline gate map

| Gate | Evidence | Expected result | Failure behavior | Owner / time |
| --- | --- | --- | --- | --- |
| **1. CI-to-plan parity** | §3.8, derived from the target-base workflows and this exact diff | The 7 `pr-regression-gates` jobs (11 runner instances) plus `validate-branch-name` trigger and pass on the PR-head SHA; `bundle-validation`, `lockfile-check`, `version-sync-check` do not trigger (no filtered path is touched) | Any red or skipped required check blocks merge; evidence from another SHA does not count | `ac-shipper-v3`, step 10 |
| **2. Deterministic toolchain and build** | `npm ci` against the committed lockfile, `cargo` against the committed `Cargo.lock`, explicit `working-directory: src-tauri` for every cargo command, npm pinned to 11.6.2 as CI does | Same resolution locally and in CI; no lockfile is modified by this change | A lockfile diff means the change grew beyond its scope; stop and re-derive §3.8 | `ac-dev-rust-v3`, steps 0 to 4 |
| **3. Authorized, traceable Git** | Open issue #1571; branch `refactor/1571-orchestrator-rename-visible-text` already created at `ecc6527b` and matching the branch-name pattern; base verified by §1.2; all state-changing Git confined to `repo-AgentsCommander` and `repo-agentscommander_webpage`; delivery by PR | Base equals the frozen SHA at entry and at push; no direct push to `main` in either repository | Drifted base, dirty tree, or a push to `main` blocks readiness | `ac-dev-rust-v3` at entry, `ac-shipper-v3` at delivery |
| **4. Process state, configuration, working directory** | `core.autocrlf` is on, which is why §3.6 reads blobs and not the worktree; every cargo command runs with an explicit `-C` / `working-directory`; cargo gates are run from PowerShell, not the Bash tool, because the mingw64 git in that tool breaks the `file:///D:/...` remotes some CLI tests use | Reproducible measurements; no task-created artifact outside `target/` and the scratchpad | A digest computed from the worktree instead of the blob is not evidence and must be recomputed | `ac-dev-rust-v3` |
| **5. Validation and scope before acceptance** | Frozen base §1.1; intended path set §6.1 to §6.4; forbidden path set §6.5; expected diff shape AC5 to AC7 | `git diff --name-only` is a subset of §6.1 to §6.4 and disjoint from §6.5; `git diff --find-renames --summary` is empty; AC7 accounts for every changed line | Any file outside the intended set is a scope question for the coordinator, not an implementer decision | `ac-dev-rust-grinch-v3`, step 9 |
| **6. Mutation ownership and no-clobber recovery** | Immediately before each step's writes, re-check `git rev-parse HEAD` and `git status --porcelain`. The working repositories are shared inside the workgroup, so an unexpected modification is another agent's, not this run's | Only this plan's paths differ | On failure, restore only the paths this run wrote and only while their content is still this run's output; never `git reset --hard`, never `git restore` over the whole tree, never `git clean`. Report any externally changed path instead of overwriting it | each step's owner |
| **7. Bounded execution and durable diagnostics** | CI job timeouts for remote work; locally, `cargo test` output redirected to a file (its stdout is otherwise swallowed, which hides `--nocapture` output and panic detail); Playwright bounded by its own config | Every command reports a real exit status; diagnostics survive outside the scratchpad long enough to be reported | A timed-out or failed command is never reported as success | each step's owner |
| **8. Evidence discipline** | Zero and absence are typed: AC1 asserts zero matches for 38 exact needles **and** an expected non-zero count of 4 for the one needle where zero would be wrong; AC2 and AC3 assert exact preserve-set membership, not "few enough"; §11 asserts an unchanged cycle count rather than "no cycles" | Every criterion is executable and states its expected value | An unmeasurable criterion is a defect in this plan, not a pass | `ac-dev-rust-grinch-v3` |

### 13.3 Local versus CI evidence ownership

Local (developer host): `cargo check`, `cargo clippy`, `cargo test`, `npm run typecheck`, `npm test`, `npm run test:debt`, `npm run check:frontend-dependencies`, the arc-record regeneration, AC1 to AC9, and the AC12 visual check. Remote (CI, authoritative for the host-dependent half): the Linux and macOS `cargo check` / `cargo clippy`, the four-runner `terminal-snapshot-portable` matrix, and `windows-release-cli-smoke`, none of which is reproducible on the Windows developer host. Website: local only, because that repository has no pull-request CI (AC11).

Two known local-only caveats, recorded so they are not mistaken for evidence: the developer host has Windows Defender real-time protection off, so an antivirus or file-lock flake that appears in CI can never be reproduced locally and a green local run must never be quoted as a failure rate; and `cargo test` invoked through the Bash tool breaks the `file:///D:/...` remotes used by some CLI tests, so the Rust gates are run from PowerShell.

### 13.4 Exact-head acceptance rule

Delivery requires every triggered and configured-required check to be green **for the exact pull-request head SHA**. A green run on an earlier push, an unexplained skip, a waiver or an administrative bypass does not satisfy AC10. If the head moves for any reason, including a rebase or a review fix, the full set must be green again on the new head. Branch protection on `main` is configured partly through rulesets rather than the classic protection object, so the shipper resolves the required-check set from the repository rulesets, not only from `branches/main/protection`, before declaring the gate met.

---

## 14. What a reviewer should attack first

Three things carry all the risk in this plan, and a reviewer's time is best spent there:

1. **Did the frozen snapshots capture the right bytes?** Everything else is cosmetic; this is the one edit that can silently strand every existing installation. Check T1 and T3 against §3.6, and check that the snapshot literal was **copied** from the pre-edit source rather than retyped.
2. **Are both Root recognizer lists wired, and was that actually proved?** `is_known_generated_root_context_template` (seven entries) and `migrate_root_role` (six, deliberately) hold overlapping lists of the same constants. T4 drives both from one fixture so that wiring one and forgetting the other cannot pass, **but only after §5.4 has been applied**: before that the new constant is byte-identical to `ROOT_ROLE_MD` and the pre-existing entries satisfy T4 on their own, so a green or red T4 at step 2's midpoint proves nothing. The evidence is the three-leg mutation probe in §12 step 2, and it is not reconstructible from the final diff. If the pull-request body does not record three observed red runs, this item is unproved and the reviewer should ask for them rather than infer them. **Check what each red run failed on**, not only that it was red: each leg must fail on the behavioral assertion named in §12, not on the direct `is_known_generated_...` predicate. A leg that dies on the predicate proves that a function whose one matching entry was just deleted returns false, which is not the same claim, and it means T4 or T2 was written in the model's assertion order instead of §5.5.2's and §5.5.1's (§9.1 BL4).
3. **Did anything other than a word change?** AC7's per-occurrence proof answers this over every changed byte on every **paired** line in both repositories; pure-insertion hunks are admitted unpaired, and §9.4 AC7 states that limit rather than glossing it. Note what changed across the rounds: round 2 stopped comparing whole lines, because that form both waved through a renamed `Context.coordinator.md` and failed correct edits; round 3 stopped classifying by lookup, because three reviewers each found a form missing from the lookup. The thing to attack now is **Rule P0 itself** (§5.2), not the lists it generates: an occurrence wrongly classified PRESERVE hides a missed rename, and one wrongly classified RENAME re-opens the false negative. The lists in §5.2 and §5.6.2 are cross-checks, and Rule P0 governs when they disagree. The productive attack is not to hunt for another missing list entry; it is §5.2.1's completeness procedure, re-run over the actual diff. If it flags an occurrence Rule P0 cannot classify, **or that it classifies against §5.6.1, §5.6.2, §5.7.2 or T1a**, that is a real defect in this plan. Attack the referential test directly: for each occurrence the plan renames, try to name the thing that resolves it, and for each one it preserves, try to show nothing does. Round 4 replaced a structural discriminator that answered seven such occurrences confidently and backwards (§5.2, D12), so the question worth asking now is whether the referential test itself has a case it gets wrong, not whether some list is missing an entry. Two grounds in Rule P0 are **not** referential and are where that attack should start: clause R1 (a test expectation is never itself a referent) and clause R2 (a referent a later phase brings into being). Round 4 left both unstated and each cost a class, so the question for each is whether it is stated narrowly enough: R1 must not reach a body §3.4 freezes, and R2 must cover exactly the two occurrences §5.6.2 rests on it, `terminal-snapshots.md:168` and `architecture.md:731` (D13). Round 5 wrote "exactly one" here; round 7's D13 supersedes that reading and a reviewer should test the clause's shape, not its cardinality. If it flags only occurrences the rule answers, the classification step is sound whether or not any list happens to name them.

The scope decision that was worth a second opinion, D3, has had three independent ones and is settled as keep (§10.1). It is no longer separable and the strike recipe has been retired.
