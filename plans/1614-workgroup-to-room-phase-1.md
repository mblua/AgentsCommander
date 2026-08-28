# Plan #1614: Workgroup to Room, phase 1 (creation prefix, dual-prefix acceptance, visible text, CLI aliases, docs)

Author: ac-architect-v3, workgroup wg-24-ac-dev-team-v3. Full delivery: new entities are created as `room-<N>-<team>`, every existing `wg-*` directory keeps working untouched, the CLI gains canonical `room` / `purge-room` / `--room` names with the old ones kept as deprecated aliases, and every surface a human or an agent reads says Room. Five families of recognizer over user-owned files are protected: three seeded context templates gain a frozen pre-rename snapshot and a version bump, **three** dual-use items are split into a frozen half and a renamed half, and two more constants plus one whole function are frozen in place, so no existing installation loses its auto-update. The freeze is scoped to the whole classification chain rather than to one function, which is what round 2 got wrong. No identifier rename, no source-file rename, no persisted-key change, no on-disk migration, no new dependency.

Status: READY_FOR_IMPLEMENTATION

Revision: round 4 (2026-08-28). **Voids Plan-SHA256 `D0E2DF0F02A02271C51074DD040C117EA04B932592E6220CC493AE16CAF9EBC7`** (round 3, `PLAN_APPROVED` from `ac-dev-rust-grinch-v3` and `CHANGES_REQUIRED` from `ac-dev-rust-v3` with two blockers), which voided `1E8DF4AB4675B145015D3142802DCAD80FA2B2793DB40C73148E12B54A35BFD2` (round 2), which voided `72D58AF23AC0E35D07A268C5563F0BF564AC9B2734170B25677E8F8FDC9951CC` (round 1). Round 1 remains the consensus-progress baseline.

**Round 4 changes, all of them corrections to text, none to a decision. No design decision, no digest, no measured figure and no acceptance criterion's mechanism moves in this round.** The four owner decisions, both new digests, all fourteen Table A and Table B digests, D8f's step ordering and code, H1 to H6's arithmetic and §3.14's totals are all unchanged. Two blockers and seven notes are addressed, each named in the section that carries it:

- **§9.4 AC5** derives its three membership lookup keys from `root.get_name()` instead of hard-coding `"agentscommander ..."`, which returned `None` on a correct implementation because the root command name is `agentscommander-new`; the three failure messages are reworded to describe what they detect, and the prose now states that clap's synthetic `help` nodes recurse, so `walked.len()` is far above the floor and the floor is a vacuity guard only. **§14 item 5** follows.
- **§9.3** moves `config/injected_messages.rs:1331` from a deferred check into clause 1 with its producer `:78` named and `:1301-1304` quoted, and adds `config/injected_messages.rs:1671` to clause 2 (1534 to 1531) with the sweep that closes clause 2's completeness. **§14 item 12** follows: eight rows, not seven; twenty clause-1 sites, not nineteen.
- **§14 item 1** stated D8f's compare ordering backwards; it now says **before** `normalize_context_for_compat`, matching §5.10 D8f.
- **§5.10 D8f** restates its CRLF bullet as defence in depth, with the traced call path, and the code comment follows. The `.replace("\r\n", "\n")` itself does **not** change.
- **§10.2** renumbers a duplicated R12 to R13 and orders the list. **§3.12 Table B** relabels its D8f row to the frozen constant. **§9.1** restates what makes AC7.15 non-self-referential (the AC7.14 pair, not the fixture). **§3.7 family 3** carries the six chain functions that had no row. **§3.14** restates step 3 in the quote-presence form that produces its own 582, records the second `#[cfg(test)]` in `cli/terminal_snapshot.rs` and the reviewers' 592-versus-588 residual. **§5.9** and **§9.3** quote `session_context.rs:3531`'s U+2014 em-dash as the source carries it.
- The 26 pre-existing em-dashes are **not** swept; both round-3 reviewers asked explicitly that no round be spent on them.

**Round 3 changes, by round-2 blocker. Every claim below names the section that carries it; none summarizes a count.** H1, the family-3 freeze stopping one call short: §3.7 family 3 re-derives the closure over the whole `classify -> reconstruct -> is_provably_generated_*` chain as a per-item table, `render_skills_section:831` gets §5.10 D8f (a frozen copy, a stated compare extension and its ordering), §5.2 P3 gains two rows, §3.12 gains one Table A and one Table B row, and AC7.14 and AC7.15 are the two criteria, neither self-referential. H2, an unsatisfiable `StaleGenerated` assertion: §3.12 Table C captures both reconstructions on synthetic inputs for the digests, §7 item 10 splits into `Current` and `StaleGenerated`, and §9.1 replaces one test with five behavioral ones over real directories. H3, the preserve half of the scope gate: §6.1 restates the constraint on production lines and names the three permitted `#[cfg(test)]` edits, §6.4 places `api/identity.rs`, §6.7 splits into two parts, and AC10 and §13.2 gate 5 follow. H4, the weaker docs alternation: §9.4 AC1's third command is unified and now excludes `docs/assets`, with the fifteen differences classified and the nine Rule R ones added to §5.9 and §5.13. H5, a self-proving allowlist: §9.4 AC1 derives Part A at the base and commits it at new §12 step 0b, with Part B for what the change introduces and the closing arithmetic stated. H6, an undersized traversal floor: AC5 asserts membership of `team create`, `role-experiment` and `role-experiment variant set`, and its floor is derived rather than round.

All ten round-2 notes are addressed in place: §3.14's blind class (and its false `#[cfg(test)]` shape claim), §9.3's illustrative-versus-complete lists, §5.8's and AC5's CI-leg justification, §5.10 D8b's post-change headroom, §5.2's P3 row count and `MESSAGING_DIR_NAME`, §5.9's two live-renderer prose rows, §11.2's module count, AC7.7's reviewer-reproduction sentence, §12 step 10b's machine-classification split, and §5.8's partial enumerations. Three further defects found in round 3 while re-deriving rather than transcribing the findings are corrected and named in §14.

Round 2 changes, by blocker group. G1: `session_context.rs:3769-4025` is frozen in its entirety and its four edit sites are removed from §5.9 (§3.7 family 3, §5.2 P3, §5.10). G2: the three dual-use constants are split or frozen, each half pinned by a digest in §3.12, and §5.9 no longer touches `root_agent.rs` (§5.10 D8b-D8d). G3: §3.8 is re-derived from a reproducible sweep, seven missing production sites are added, and AC1's 36 needles are replaced by a total sweep plus a committed allowlist (§9.4). G4: §3.14 is new and §6.1 is regenerated from it; §13.2 gate 5 is reworded. G5: `role_experiment.rs:95` gains its `value_name`, `team.rs:61` is added, and AC5 walks `clap`'s command tree instead of a hand list. G6: 18 arcs, not 19. N1 and N2 are §9.3 clause 2 and §7.6. Three further recognizer defects that neither reviewer found are fixed: the second root wiring (§5.10 D8a), the injected-messages hash recognizer and its `%WORKGROUP%` token (§3.7 family 5), and the compactness budget that constrains `WORKGROUP_GIT_SCOPE`'s replacement text (§5.10 D8b).

Issue: [mblua/AgentsCommander#1614](https://github.com/mblua/AgentsCommander/issues/1614), "Phase 1: create rooms as room-N-<team> and rename Workgroup to Room in GUI, CLI surface and docs" (OPEN). Parent epic: [#1613](https://github.com/mblua/AgentsCommander/issues/1613) "Epic: rename the Workgroup concept to Room" (OPEN). Follow-up: [#1615](https://github.com/mblua/AgentsCommander/issues/1615) "Phase 2: retire Workgroup, drop wg-* support, deprecated CLI aliases and internal identifiers" (OPEN, out of scope).

Objective: AgentsCommander creates Rooms and never creates a Workgroup again, while every Workgroup that exists on disk is discovered, listed, addressed, operated and deleted exactly as it is today; and every string a person or an agent reads calls the concept Room, while every identifier, file name, serialized key, wire value and reason code stays byte-identical to what is on disk today.

---

## 1. Frozen authority and entry gate

### 1.1 Frozen base

| Fact | Value |
| --- | --- |
| Repo | `repo-AgentsCommander` at `D:\0_repos\AgentsCommander_iac\.ac\wg-24-ac-dev-team-v3\repo-AgentsCommander` |
| Branch (already created, do not recreate) | `refactor/1614-workgroup-to-room-phase-1` |
| Frozen base | `main` == `origin/main` == branch head == `d7008b34e155a8bd6481be5feecfc7d96575328f` |
| Live-fetch check at authoring time | `git fetch origin main` then `git rev-parse origin/main` returned the same SHA; `FETCH_HEAD` carries it |
| Working tree at authoring time | `git status --porcelain` empty |
| Delivery path | Full |
| Accepted task class | Routine application change, no release, no signing, no untrusted host (see §13.1) |

Codebase Memory gate `ready` at authoring time (2026-08-28 UTC): project `D-0_repos-AgentsCommander_iac-.ac-wg-24-ac-dev-team-v3-repo-AgentsCommander`, 25,291 nodes / 136,344 edges, `head_sha` `d7008b34e155a8bd6481be5feecfc7d96575328f`.

Every line number in this plan is at that SHA. Byte-level evidence (§3.12) was taken from `git cat-file blob d7008b34:<path>`, never from the working tree, because `core.autocrlf` is `true` and `*.md` / `*.tsx` / `*.ts` carry no `.gitattributes` `eol` rule, so a worktree digest is not reproducible. If a line number no longer matches the quoted text, re-anchor on the quoted text, never on the number.

### 1.2 Entry ritual for the implementer

Inside `repo-AgentsCommander`, before the first edit:

1. `git -C <repo> fetch origin main`.
2. `git -C <repo> rev-parse HEAD origin/main` and `git -C <repo> merge-base HEAD origin/main`. All three must equal `d7008b34e155a8bd6481be5feecfc7d96575328f`. If any differs, STOP and request a bounded-drift review under §13.5; do not rebase silently.
3. `git -C <repo> status --porcelain` must be empty.
4. `git -C <repo> rev-parse --abbrev-ref HEAD` must print `refactor/1614-workgroup-to-room-phase-1`.

Root `.gitignore` line 11 ignores `/plans/` while `plans/*.md` are tracked (23 tracked files at base). This plan file is staged with `git update-index --add plans/1614-workgroup-to-room-phase-1.md`; that is the only ignore-crossing this plan authorizes, the ignore rule is never widened, and `git add` under `plans/` can exit 1 while still staging, so trust `git diff --cached --name-only`, not the exit code.

---

## 2. Issue and objective

The product concept Workgroup becomes Room. Phase 1 delivers the whole perceived rename plus the one real behavioral change (new directories are `room-*`), with zero risk to existing installations: nothing on disk is renamed, converted or removed.

Required outcomes, each binding:

- **(A) Creation.** Every code path that creates an entity directory in a Project AC Root creates `room-<N>-<team>`. No production code path produces a `wg-*` directory.
- **(B) Independent numbering.** The Room slot counter considers only `room-<n>-<team>` directories. In a `.ac` root that already holds `wg-1-<team>`, the next Room is `room-1-<team>`. The existing lowest-free-positive-slot reuse semantics are preserved for Rooms.
- **(C) Dual-prefix acceptance.** Every site that today gates on `starts_with("wg-")` or `strip_prefix("wg-")`, in Rust and in TypeScript, accepts `room-` and `wg-` identically, so both kinds of directory are discovered, listed, grouped, addressed, operated and deleted the same way.
- **(D) Peer identity.** A peer FQN carries the literal directory name, so `project:room-1-team/agent` resolves through `list_peers`, `replica_identity`, `api/identity.rs` and `api/actuation.rs` exactly as `project:wg-1-team/agent` does today.
- **(E) Parent-repo safety.** A new `room-*` directory is excluded from parent-repository git tracking with the same guarantee `wg-*` has today, in every existing installation as well as in new ones.
- **(F) CLI.** `room`, `purge-room` and `--room` are the canonical names. `workgroup`, `purge-wg`, `--wg` and `--workgroup` remain accepted as deprecated aliases that parse to the identical value and produce identical side effects, exit codes and operational output.
- **(G) Visible text.** No string rendered by `src/`, printed by the CLI, injected into an agent's context, or written into `docs/` calls the concept Workgroup or WG, except where Rule P (§5.2) preserves it.
- **(H) Nothing else moves.** No file rename, no Rust or TypeScript identifier rename, no `data-ac-testid` change, no CSS class change, no JSON or serde key change, no event name change, no IPC command name change, no outbox action value change, no dependency-graph cycle, no on-disk migration.

The two hard correctness constraints are (E) and the recognizer half of (G). (E) is a data-loss class defect and is analysed in §3.6. The recognizers are five distinct families (§3.7), not the three round 1 counted, and every one of them carries the same failure: if a shipped byte moves without the previous bytes being frozen, every user whose file is currently pristine is reclassified as customized and silently stops auto-updating, forever, with no test and no gate able to see it. Two of the five families sit outside the `*_BEFORE_*` naming convention round 1's Rule P3 keyed on, which is how round 1 came to direct edits into both of them.

---

## 3. Evidence (measured at `d7008b34`, not predicted)

### 3.1 Baseline, and three corrections to the numbers the issue and the assignment carry

| Fact | Command | Value |
| --- | --- | --- |
| Tracked files | `git ls-files \| wc -l` | 844 |
| Files matching `workgroup` case-insensitively | `git grep -il workgroup \| wc -l` | 265 |
| Matching lines | `git grep -ic workgroup \| awk -F: '{s+=$NF}END{print s}'` | 4362 |
| Files carrying the literal `"wg-` | `git grep -l '"wg-' \| wc -l` | 118 |
| `src/` files (all) | `git grep -il workgroup -- src \| wc -l` | 110 |
| `src/` files, non-test | adding `':!src/**/*.test.ts' ':!src/**/*.test.tsx'` | **43** (803 lines) |
| `src/` files, test only | `git grep -il workgroup -- 'src/**/*.test.ts' 'src/**/*.test.tsx' \| wc -l` | **67** |
| `src-tauri/src/` files | `git grep -il workgroup -- src-tauri/src \| wc -l` | 62 (1286 lines) |
| `src-tauri/tests/` files | `git grep -il workgroup -- src-tauri/tests \| wc -l` | 8 |
| `docs/` files | `git grep -il workgroup -- docs \| wc -l` | 57 (445 lines) |
| `plans/` files | `git grep -il workgroup -- plans \| wc -l` | 14 |

**Correction 1.** The issue's "110 files under `src/` mention the concept" is true but 67 of those are `*.test.ts(x)`. The production frontend surface is **43 files and 803 lines**, not 110 files.

**Correction 2.** The assignment says the frontend carries no prefix predicate and that dual-prefix acceptance is a Rust-only concern. It is not: the frontend has **six** production prefix predicates (§3.3), one of which gates a button and two of which render the visible `WG` badge and the rail sort order.

**Correction 3.** The assignment's gate list names `phone/messaging.rs:177`. Line 177 is the *caller*; the two predicates are at `:373` and `:386`. The assignment's list also omits ten live gate sites: `commands/entity_creation.rs:1085`, `:2964`, `:3488`, `:4301`, `config/teams.rs:1661`, `phone/messaging.rs:373`, `:386`, `pty/container_paths.rs:289`, `pty/container_repos.rs:136`, `screenshot/windows.rs:1405` and `session/session.rs:249`. The complete set is §3.2 and it has exactly 40 lines.

**Correction 4 (round 2, against round 1's own §3.8).** Round 1 printed a frontend sweep command and a result (906 lines / 43 files) that do not correspond: the printed command omits lower-case `wg` and returns 706 lines across 40 files. The sweep that returns 906 is in §3.8 and its file count is **41**. This matters beyond arithmetic: §3.8's enumeration was not in fact derived from the command §3.8 printed, which is why seven production visible-text sites were missing from it.

**Correction 5 (round 2, re-measured in round 3).** There was no equivalent measurement for Rust. §3.14 supplies it: the same sweep over `src-tauri/src` returns **3090 lines across 94 files**, of which **1174 lines across 63 files** are production reader-facing candidates. Round 2 reported 544 across 55 for that inner figure; the file count is right and the line count was an artifact of a mis-stated `#[cfg(test)]` narrowing rule, corrected with the measurement in §3.14. Round 1's §6.1 listed 32 files and omitted nine that carry strings a user or an agent reads.

### 3.2 The Rust prefix gates: 40 lines, 41 occurrences, exhaustive

Produced by `git grep -n 'starts_with("wg-")\|strip_prefix("wg-")' -- src-tauri/src`, minus five hits that are doc comments or test assertions (`entity_creation.rs:3832`, `:6929`, `:6944`, `:7240`, `:7257`), which §5.2 classifies separately.

`strip_prefix("wg-")`, 10 lines:

| # | Site | Function / purpose |
| --- | --- | --- |
| S1 | `cli/list_peers.rs:912` | derive team name from the directory name during the cross-project replica scan |
| S2 | `cli/role_experiment.rs:2298` | `next_workgroup_number`, max+1 slot for a role-experiment run artifact |
| S3 | `commands/entity_creation.rs:1085` | `parse_team_from_workgroup_name`, the central name parser used by `list_workgroup_dirs` |
| S4 | `commands/entity_creation.rs:4301` | `determine_next_wg_number`, the product slot allocator |
| S5 | `config/teams.rs:212` | `is_valid_wg_local_shape`, the authorization shape check for a `<wg>/<agent>` local |
| S6 | `config/teams.rs:449` | `resolve_wg_coordinator_replica`, team derivation from the directory name |
| S7 | `config/teams.rs:649` | target validation, returns `invalid_target` |
| S8 | `config/teams.rs:1469` | `extract_wg_team` |
| S9 | `phone/messaging.rs:373` | `is_wg_dir`, used by `workgroup_root()` to walk up from an agent root |
| S10 | `phone/messaging.rs:386` | `parse_wg_prefix`, builds the `wgN` short token in every message filename |

`starts_with("wg-")`, 30 lines (`config/teams.rs:1661` carries two occurrences on one line):

| # | Site | Function / purpose |
| --- | --- | --- |
| P1 | `cli/list_peers.rs:707` | orchestrator sees orchestrators of other entities in the same AC root |
| P2 | `cli/list_peers.rs:907` | cross-project replica scan |
| P3 | `cli/list_peers.rs:1009` | orchestrator peer enumeration |
| P4 | `cli/role_experiment.rs:2328` | `validate_workgroup_under_ac_root` |
| P5 | `cli/role_experiment.rs:3185` | live-session scan across entity directories |
| P6 | `cli/role_experiment.rs:3204` | `replica_role_overrides`, excludes `-role-exp-` runs |
| P7 | `cli/role_experiment.rs:3224` | `is_replica_role_file`, 4-component window match |
| P8 | `commands/ac_discovery.rs:1182` | GUI discovery, primary scan |
| P9 | `commands/ac_discovery.rs:1957` | GUI discovery, secondary scan |
| P10 | `commands/config.rs:1527` | replica validation, user-visible error |
| P11 | `commands/config.rs:1561` | replica collection under an AC root |
| P12 | `commands/entity_creation.rs:2964` | `collect_team_workgroup_dirs`, team deletion |
| P13 | `commands/entity_creation.rs:3488` | inline twin of P12 in the team-repo update path |
| P14 | `commands/task.rs:104` | TASK.md write guard, user-visible error |
| P15 | `config/ac_root.rs:145` | AC-root resolution from a replica directory |
| P16 | `config/coding_agent_profiles.rs:235` | profile scope validation, user-visible error |
| P17 | `config/loops.rs:444` | Loop target validation, user-visible error |
| P18 | `config/placeholders.rs:273` | replica-directory detection for placeholder expansion |
| P19 | `config/replica_identity.rs:238` | replica identity resolution, user-visible error |
| P20 | `config/teams.rs:89` | `agent_fqn_from_path`, builds `project:<dir>/<agent>` |
| P21 | `config/teams.rs:121` | path split into (entity, agent) |
| P22 | `config/teams.rs:1137` | bounded target scan |
| P23 | `config/teams.rs:1465` | `extract_wg_team` guard (paired with S8) |
| P24 | `config/teams.rs:1661` (x2) | messaging Rule 2, "same entity" authorization |
| P25 | `phone/mailbox.rs:2157` | `resolve_wg_path_from_session_dirs` |
| P26 | `phone/mailbox.rs:11206` | mailbox Loop 4 replica fallback |
| P27 | `pty/container_paths.rs:289` | container transport refuses to bind-mount an entity root, user-visible error |
| P28 | `pty/container_repos.rs:136` | repo mount resolution |
| P29 | `screenshot/windows.rs:1405` | replica-root walk-up for screenshot attribution |
| P30 | `session/session.rs:249` | `find_workgroup_task_path_for_cwd`, TASK.md lookup |

**No other prefix-handling shape exists.** Verified by three negative sweeps that all return empty: `git grep -nE '"wg-"' -- src-tauri/src` minus the two known predicates; `git grep -nE 'Regex::new.*wg|split\("wg|find\("wg|trim_start_matches\("wg' -- src-tauri/src`; and `git grep -n '\[3\.\.' -- src-tauri/src`, whose only entity-name hits are `entity_creation.rs:2965` and `:3489`.

**Two hardcoded prefix lengths.** `entity_creation.rs:2965` and `:3489` both read `let middle = &name_str[3..name_str.len() - wg_suffix.len()];`. `3` is `"wg-".len()`. `"room-".len()` is 5. A dual-prefix edit that leaves the literal `3` silently mis-slices every Room name and, for a team name long enough, panics on a reversed range. This is the single most likely silent defect in the change.

### 3.3 The six frontend production prefix predicates

Produced by `git grep -nE 'wg-' -- 'src/**/*.tsx' 'src/**/*.ts' ':!src/**/*.test.tsx' ':!src/**/*.test.ts'`, keeping only regex or `startsWith` predicates. CSS class names (`ac-wg-*`, `workgroup-group-*`), `data-ac-testid` values and the `ui-harness.tsx` fixture are not predicates and are Rule P.

| # | Site | Predicate | What it decides |
| --- | --- | --- | --- |
| F1 | `src/shared/path-extractors.ts:25` | `/^wg-\d+/` case-sensitive, then `wg.toUpperCase()` | `extractWorkgroupName`, which feeds the `titlebar-wg-badge` in both `src/sidebar/components/Titlebar.tsx:218` and `src/terminal/components/Titlebar.tsx:83`. This is the literal "place that shows WG". |
| F2 | `src/shared/profile-utils.ts:124` | `/^wg-/` case-sensitive | replica-path parse for profile scope |
| F3 | `src/shared/profile-utils.ts:472` | `/^wg-/` case-sensitive | replica-path predicate |
| F4 | `src/sidebar/components/WorkgroupGroupRail.tsx:67` | `/^wg-(\d+)/i` | `wgNumber`, the rail sort key; non-matching names collapse to `Number.MAX_SAFE_INTEGER` and sort last |
| F5 | `src/sidebar/components/WorkgroupGroupRail.tsx:72` | `/^wg-(\d+)/i` then `` `WG${n}` `` | `wgTooltipLabel`, the visible rail tooltip rows `WG1:(agent)` |
| F6 | `src/terminal/components/WorkgroupTask.tsx:74` | `/[\/\\]wg-/` case-sensitive | `hasWorkgroupContext`, which gates the TASK.md Edit and Clean buttons |

F6 carries a comment at `:70` explaining that its case sensitivity is load-bearing: the backend uses a byte-exact `starts_with`, and a case-insensitive UX gate would enable buttons whose every click fails. F4 and F5 are display-only and are deliberately case-insensitive today. §5.4 preserves each site's exact case sensitivity.

Left unchanged, F1 renders no badge for a Room, F2/F3 refuse to recognise a Room replica for profile scope, F4 sorts every Room last, F5 shows the raw directory name instead of a short label, and F6 leaves the Task buttons permanently disabled in every Room. None of these fails loudly.

### 3.4 Creation sites and the two slot allocators

| Site | Code | Role |
| --- | --- | --- |
| `commands/entity_creation.rs:1180` | `let wg_name = format!("wg-{}-{}", wg_number, safe_team);` | `create_workgroup_on_disk`, the CLI creation path |
| `commands/entity_creation.rs:2844` | same expression | `create_workgroup`, the GUI Tauri command |
| `cli/role_experiment.rs:2209` | `let name = format!("wg-{}-role-exp-{}", next, sanitized_experiment);` | `create_run_workgroup_dirs`, hidden `role-experiment` run artifact |
| `phone/mailbox.rs:23734` | `let wg_dir = ac_root.join(format!("wg-1-{}", wg_suffix));` | `#[cfg(test)]` fixture |
| `screenshot/windows.rs:1576` | `std::env::temp_dir().join(format!("wg-6-team-{}", Uuid::new_v4()...))` | `#[cfg(test)]` fixture in the system temp dir, not an AC root |

`determine_next_wg_number()` (`entity_creation.rs:4291`) collects taken slots by `strip_prefix("wg-")` + `split_once('-')` + `parse::<u32>()`, is **global across teams**, returns the lowest free positive integer, and degrades to `1` when `read_dir` fails. `create_workgroup` then guards with `if wg_dir.exists() { return Err("Workgroup directory already exists: ...") }`, which is what surfaces the degraded case.

`next_workgroup_number()` in `cli/role_experiment.rs:2288` is a **different** allocator: `max + 1` over `wg-` names, not lowest-free.

Both creation paths call `crate::commands::ac_discovery::ensure_ac_root_gitignore(&base)` **before** the directory is created: `entity_creation.rs:1149` precedes `:1180`, and `:2832` precedes `:2844`. That ordering is what makes §3.6's fix effective on the very first Room.

### 3.5 The CLI surface

Subcommands (`src-tauri/src/cli/mod.rs`):

| Line | Declaration | Printed name |
| --- | --- | --- |
| 160-162 | `/// Purge every agent in the caller's own workgroup ...` + `#[command(name = "purge-wg")] PurgeWg(...)` | `purge-wg` |
| 163-164 | `/// Set the title field in the workgroup TASK.md frontmatter (orchestrator-only)` + `TaskSetTitle(...)` | `task-set-title` |
| 165-166 | `/// Append text to the body of the workgroup TASK.md (orchestrator-only)` + `TaskAppendBody(...)` | `task-append-body` |
| 173-174 | `/// Manage workgroups in an AC project` + `Workgroup(...)` | `workgroup` |
| 175-176 | `/// Manage teams and scoped workgroup membership` + `Team(...)` | `team` |

`clap`'s derive compiles a `#[derive(Subcommand)]` variant doc comment into that subcommand's `about`, printed twice (on the subcommand's own `--help` and in the top-level listing). All five lines above are therefore printed help, not source commentary. The same holds for `cli/workgroup.rs:26`, `:28`, `:30` (`List` / `Add` / `Remove`) and `cli/team.rs:29`, `:31` (`AddMember` / `RemoveMember`).

Flags whose long name is `wg` or `workgroup`:

| Site | Command | Flag |
| --- | --- | --- |
| `cli/purge_wg.rs:96` | `purge-wg` | `--wg` |
| `cli/workgroup.rs:65` | `workgroup remove` | `--workgroup` |
| `cli/loop_cmd.rs:77` | `loop create` | `--workgroup` |
| `cli/loop_cmd.rs:99` | `loop update` | `--workgroup` |
| `cli/team.rs:40` | `team list` | `--workgroup` |
| `cli/team.rs:81` | `team add-member` | `--workgroup` |
| `cli/team.rs:93` | `team remove-member` | `--workgroup` |
| `cli/role_experiment.rs:95` | `role-experiment` (hidden) | `--retain-workgroup` |

Free-text help blocks containing the word: `cli/purge_wg.rs:77-85` (`after_help`, four occurrences at `:78`, `:79`, `:80`, `:81`), `cli/team.rs:61`, `:66` and `:71` (**three** `help = ...` strings, not the two round 1 counted; `:61` reads `"Define a repo available to the team when workgroups are created. Repeat for multiple repos"` and is the only one whose wording round 1's `for workgroup creation` needle could not reach), plus the `after_help` / `long_about` bodies of `cli/list_peers.rs`, `cli/mod.rs`, `cli/self_switch.rs`, `cli/send.rs`, `cli/task_append_body.rs` and `cli/task_set_title.rs`. `src-tauri/tests/cli_behavior_contract.rs:306-331` and `:760-776` pin the printed help and the accepted flag names for `workgroup` and `team`, so those assertions are the executable oracle for this surface.

### 3.6 Parent-repository exclusion: the one hard blocker neither the issue nor the assignment names

`ensure_ac_root_gitignore()` at `commands/ac_discovery.rs:1495` writes and maintains the `.gitignore` inside every Project AC Root. Its `required_entries` table begins:

```
(
    "wg-*/",
    "# AgentsCommander: exclude workgroup cloned repos from parent git tracking.\n# Without this, parent repo operations (checkout, reset) corrupt child clones.",
),
```

There is no `room-*/` entry. If creation moves to `room-` without adding one, every Room created inside a project that is itself a git repository becomes tracked content of the parent repository: its cloned `repo-*` working copies, agent session state and generated context all enter `git status`, and a parent `git checkout` or `git reset --hard` corrupts the child clones. That is exactly the failure the existing comment documents, and it is a data-loss class defect, not a cosmetic one.

The function's shape makes the fix safe for existing installations: when the file exists it tests each required pattern with `content.lines().any(|line| line.trim() == *pattern)` and appends only the missing ones, then rewrites the file. So an existing `.ac/.gitignore` gains `room-*/` on the next call, and the next call always happens before the first Room directory is created (§3.4). The presence test is on the pattern line only, so an entry already present keeps whatever comment it has.

The repository's own root `.gitignore` line 18 carries `.ac/wg-*/` under the comment `# AgentsCommander workgroup replicas (always per-machine state).` and needs the matching `.ac/room-*/`.

`config/instance_gitignore.rs` carries no entity-prefix pattern (`git grep -in 'wg-\|workgroup' -- src-tauri/src/config/instance_gitignore.rs` is empty), so the instance-artifact ignore surface is not affected.

### 3.7 Recognizers over user-owned files, enumerated by behavior

Round 1 built this inventory by grepping the name `is_known_generated*` and found three seeded-template specs. That selection rule is wrong: the property that matters is not what a function is called, it is **whether its bytes are compared against, or searched inside, a file the user owns**. Rebuilt by that behavior, there are **five** families, and round 1's §5.9 directed Rule R edits into two of them.

The shared failure mode is identical in every family and is silent in all five: the shipped bytes move, a user's on-disk file stops matching, the file is reclassified as user-authored, and it **never auto-updates again**. No test catches it, because every existing test builds its expected value by calling the same function or reading the same constant it then classifies (§9.2 note).

#### Family 1: the three seeded context-template specs

`config/seeded_context_templates.rs` pairs each spec's `current_version`, `current_content` and `is_known_generated` recognizer. A user's file auto-updates only while the recognizer accepts it byte-for-byte; unknown content is preserved and backed up, never overwritten.

| Spec | `id` | `current_version` | Current content | Recognizer |
| --- | --- | --- | --- | --- |
| project | `global` | 4 (`:483`) | `session_context::get_default_agent_template` (`session_context.rs:2469`) | `is_known_generated_global_template` (`:535`) **and** `is_known_generated_standalone_global_template` (`:553`) |
| project | `coordinator` | 5 (`:493`) | `session_context::get_default_coordinator_template` (`session_context.rs:2508`) | `is_known_generated_coordinator_template` (`:561`) |
| root | `rootAgent` | 7 (`:507`) | `root_agent::default_root_context_template` returning `ROOT_ROLE_MD` (`root_agent.rs:675`) | `is_known_generated_root_context_template` (`root_agent.rs:729`) |

All three carry text this rename must change:

- `get_default_agent_template()` carries exactly one occurrence, the Core Concepts line `- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and \`repo-*\` working repos.`
- `get_default_coordinator_template()` carries exactly one occurrence, `- To reach another workgroup, message its orchestrator, never its members, ...`
- `ROOT_ROLE_MD` carries six, at absolute lines 687, 698, 702, 704, 710 and 712.

`STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS` proves the global template has two independent recognizers by design (`:545-556` documents why root retirement must not widen the project one), so the new global snapshot must be wired into **both**.

#### Family 2: the root `Role.md` pristine-generation list, a second consumer of the same constants

`root_agent::migrate_root_role_file` (`:1045-1051`) carries its **own** list of known root generations, independent of family 1's `old_generated` array, and it includes `ROOT_ROLE_MD` itself at `:1051`. A `Role.md` matching any entry is reduced to `MINIMAL_ROOT_ROLE_MD`; anything else is left alone forever.

`ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD` is wired into **both** lists (`:738` and `:1050`), and the repository states why: `frozen_v5_root_context_is_recognized_and_migrated_on_both_paths` (`root_agent.rs:2436`) exists precisely so that "a list edited in only one place cannot pass silently". **Round 1's §5.10 named only the `old_generated` array.** A snapshot wired into one list and not the other reclassifies every pristine pre-rename root `Role.md` on the migration path while the recognizer test stays green.

#### Family 3: the frozen legacy default-context renderer (the one round 1 directed edits into)

```
resolve_agent_context_with_activation (session_context.rs:2700, production)
  -> classify_legacy_rendered_default_context            (:4033)
       -> current_legacy_rendered_default_context        (:4057)
       -> looks_like_generated_legacy_default_context    (:4065)
            -> reconstruct_legacy_rendered_default_context (:4089)
                 -> has_legacy_default_tail                     (:4210)
                 -> has_unknown_legacy_default_heading          (:4216)
                      -> is_known_legacy_default_heading        (:4226)
                 -> extract_legacy_code_block_after             (:4166)
                 -> extract_legacy_skills_section               (:4175)
                 -> is_provably_generated_legacy_skills_section (:4183)
                      -> render_skills_section                  (:812-919)  <- LIVE renderer
                 -> legacy_rendered_default_context_for_compat          (:3743)
                 -> pre_1072_legacy_rendered_default_context_for_compat (:3756)
                      -> legacy_rendered_default_context_for_generation (:3769-4025)
```

**The recognizer is the chain, not the function, and round 2 froze only the function.** `reconstruct_legacy_rendered_default_context` does not merely call the two reconstructions at `:4152-4163`; before it reaches them it extracts the skills section out of the user's file (`:4146`) and hands it to `is_provably_generated_legacy_skills_section` (`:4148`), which recomputes the **live** `render_skills_section` over the user's on-disk skills and compares it, normalized, against what it extracted (`:4187-4207`). A failed compare returns `false`, `reconstruct` returns `None` at `:4149`, `looks_like` returns `false` at `:4081`, and the file classifies `NotLegacy` permanently. Two links of that chain sit outside `3769-4025`, so freezing the range alone does not protect it.

`legacy_rendered_default_context_for_generation` is **not a renderer**. It reconstructs the exact bytes a previous release wrote into a user's `Context.AgentsCommander.md` so the file can be compared for equality. The file's own header at `:3733` says so: *"Frozen warning bytes shipped before #1072. These constants are only for compatibility recognition and must never be used for current runtime output."* That it is a frozen older generation rather than a synced duplicate of the live renderer is provable in the tree: the live renderer at `:3630` says `orchestrator` while its legacy twin at `:3880` still says `coordinator`.

The classifier's three outcomes (`:4044-4054`), and what each does to the user's file:

| Outcome | Condition | Effect |
| --- | --- | --- |
| `Current` (`:4046`) | the file equals the legacy reconstruction under the current git-scope generation | returned as-is (`:2744`) |
| `StaleGenerated` (`:4050`) | the file equals either legacy reconstruction candidate | the #664 self-heal atomically rewrites it to the current format (`heal_stale_global_recorded`, `:2745-2757`) |
| `NotLegacy` (`:4054`) | neither matches | treated as a user-authored template and rendered as one, **forever** (`:2767`) |

Change one byte of prose inside `:3769-4025` and every pre-#1369-format `Context.AgentsCommander.md` on disk falls from `StaleGenerated` to `NotLegacy`, never self-heals again, and keeps receiving the pre-#1369 Golden Rule write restrictions permanently. That is the data-loss class §2 names, introduced by the change rather than guarded against.

**Why the current format is unaffected by the freeze.** `looks_like_generated_legacy_default_context` rejects early (`:4073-4078`) when the content contains `## Core Concepts`, `# Workspace Repos` or `# Agent Repos`. Files written by the current release all carry those headings, so they never reach this family at all. Freezing `:3769-4025` therefore costs nothing on the current path and is purely protective on the legacy one.

**Transitive literal closure, re-derived in round 3 over the whole chain rather than over one function.** Round 2 enumerated the closure of `:3769-4025` and stopped there. That enumeration is correct as far as it goes and both round-2 reviewers reproduced it exactly, but the frozen unit is every byte the *classification decision* depends on, which spans `classify -> looks_like -> reconstruct -> is_provably_generated_*`. The closure is therefore enumerated per participating item, with, for each, whether it carries an occurrence of the retired token. **Round 4 completes the table.** Round 3's version listed only the items that emit bytes or that a frozen constant is read into, which left the six chain functions themselves with no row while the heading claimed "per participating item"; they are added below and all six carry nothing, so the conclusion does not move. Each "sweep returns 0" is §3.14's raw sweep pattern run over that function's brace-matched body at `d7008b34`, which is stricter than a literal-only reading because it also catches an identifier or a comment. `ac-dev-rust-v3` found the omission in round 3 and read all six independently.

| Item | Site | Carries a retired-token literal? |
| --- | --- | --- |
| `classify_legacy_rendered_default_context` | `:4033-4055` | no. **Row added in round 4.** Control flow and the `:4039` whole-file normalization; the raw §3.14 sweep returns **0** lines over its body |
| `looks_like_generated_legacy_default_context` | `:4065-4087` | no. **Row added in round 4.** Its literals are `{{`, `}}`, `## Core Concepts`, `# Workspace Repos`, `# Agent Repos`; sweep returns **0** |
| `reconstruct_legacy_rendered_default_context` | `:4089-4164` | no. **Row added in round 4.** Headings, ordered markers, `assigned root:`, `3. **Your origin Agent Matrix` and the `list-peers-lean` tail; sweep returns **0** |
| `legacy_rendered_default_context_for_compat` | `:3743-3754` | no. **Row added in round 4.** A pure delegation with no literal; sweep returns **0** |
| `pre_1072_legacy_rendered_default_context_for_compat` | `:3756-3767` | no. **Row added in round 4.** Same shape; sweep returns **0** |
| `normalize_context_for_compat` | `:4259-4261` | no. **Row added in round 4.** Its only literals are `"\r\n"` and `"\n"`; sweep returns **0**. D8f extends its **caller**, not this function |
| `legacy_rendered_default_context_for_generation`, whole body | `:3769-4025` | yes, throughout. Frozen whole (§5.2 P3), pinned by §3.12 Table A `940FA357...` |
| `WORKGROUP_GIT_SCOPE` | `:3368` | yes. Read at `:3860`. **Dual-use, split in §5.10 D8b** |
| `DIRECT_MATRIX_GIT_SCOPE` | `:3369` | no. Read at `:3861`. Listed so the closure is complete rather than sampled |
| `LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072`, `LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072` | `:3735-3736` | already frozen by #1072; read at `:3862` and `:3864`. Pinned by Table A `DADA740F...` |
| `MESSAGING_DIR_NAME` | `phone/messaging.rs:11` | no. Its value is `"messaging"`, which Rule R cannot move. Listed for completeness and given a §5.2 P3 row |
| `display_path`, `is_root_agent_dir_name` | `:239`, helper | no literal in the emitted prose |
| `root_agency_cache_guidance` | `:3679` | no. Its second-order constant `role_templates::AGENCY_TEMPLATES_DIR` is a P0 on-disk directory name |
| `workgroup_root` | `phone/messaging.rs:141-157` | no. A pure path walk with no filesystem touch; §5.3 widens the predicate only |
| `extract_legacy_code_block_after` | `:4166` | no |
| `extract_legacy_skills_section` | `:4175` | no. Its `delegated` marker literal carries no occurrence |
| `has_legacy_default_tail` | `:4210` | no. The tail literal is the `list-peers-lean` command line |
| `has_unknown_legacy_default_heading`, `is_known_legacy_default_heading` | `:4216`, `:4226` | no. The eleven known headings carry no occurrence |
| `is_provably_generated_legacy_skills_section` | `:4183` | no in itself, but it **executes** the item below |
| `render_skills_section` | `:812-919` | **yes, exactly one, at `:831`.** See below |

**`render_skills_section:831` is the one live-rendered literal inside the recognizer's comparison path, and this plan's own rules force it to move.** The line is:

```
                "When running from a workgroup replica, resolve skills/... against the origin Agent Matrix path above, not against the replica CWD.\n",
```

It is emitted on the `(Some(_), Some(skills_root))` arm (`:823-839`), that is, for every replica that actually has a `skills/` directory, which is the common case. It is prose injected into every agent's live context, so requirement (G) covers it; §6.1 lists `config/session_context.rs` as edited; AC1's total Rust sweep returns it on the post-change tree; and it is not P1 (not a comment), not P2 (not a fixture), not P4 (not a `log::`). AC1 clause 3 then classifies it a Rule R miss rather than an allowlist entry, so an implementer following round 2's plan renames it, and nothing in round 2 catches the consequence:

- AC7.8's source digest covers `3769-4025`; `:831` is 2,938 lines outside it.
- AC7.7's Table C capture takes `skills_section` as an **input parameter**, so it never executes `render_skills_section` at all.
- The one test that pins the literal, `:6829` inside `generated_shaped_manual_legacy_skills_content_is_preserved` (`:6804`), builds its expected value from the same string it asserts. Measured in all four rename combinations it stays green, because its fixture carries manual skill entries that can never equal a fresh render, so it is neither a red test nor a backstop.
- The working legacy-classification tests (`:6180-6210` and siblings) build their matrix root with no `skills/` directory, so they take the `(Some(matrix_root), None)` arm at `:840` and never render `:831`. The regression is invisible to `cargo test`.

The repository documents this exact failure mode at `:4194-4200`, in the comment on the compare that consumes it: *"Every other literal in `render_skills_section` is frozen for this project (G2 scope rule); if one ever changes, this compare must extend with it or healing dies silently."* Round 2 quoted that comment as evidence for freezing `3769-4025` and never applied it to the function the comment is about. The fix precedent is in the tree: `LEGACY_GENERATED_SKILLS_SECTION_INTRO` (`:225-236`) exists because #1005 hit this on the intro, and the intro-swap fallback at `:4201-4207` swaps **only** the intro, so a changed body line is not recoverable by it.

**Decision: `:831` takes its Rule R substitution and gets the #1005 treatment.** Not renaming it would leave "workgroup replica" in the `## Skills` section of every Room agent's live context, which is precisely the visible-text defect this issue exists to remove, and would make AC7.13 pass only because its fixture has no skills directory. The frozen copy, the compare extension and the two non-self-referential criteria are specified in §5.10 D8f, and the bytes are pinned in §3.12 Table A and Table B.

`workgroup_root` is widened to two prefixes by §5.3. That cannot disturb this family: for a legacy `wg-*` replica the widened predicate still returns the same `wg-*` ancestor, and a `room-*` replica has no pre-#1369 context file to recognize, because Rooms do not exist before this change.

`session_context.rs:4198-4200` already warns about exactly this class: *"Every other literal in `render_skills_section` is frozen for this project (G2 scope rule); if one ever changes, this compare must extend with it or healing dies silently."* §5.10 D8f is that extension.

#### Family 4: `contains()` matchers against a user's live `Role.md`

`OLD_DEFERRED_MESSAGING_PARAGRAPH` (`root_agent.rs:290`) is not compared for equality; it is **searched for inside** the user's file at `:1058`, and on a hit the migration substitutes `ROOT_COORDINATION_MESSAGING_PARAGRAPH` for it at `:1059-1062`. Applying Rule R to `:290` silently disables that migration for every installation still on the deferred-messaging generation: the `contains` stops matching, the branch never fires, and nothing reports it.

`ROOT_COORDINATION_MESSAGING_PARAGRAPH` (`:292-306`) is **dual-use**: it is the paragraph the migration writes into the user's live file at `:1061`, and it is simultaneously interpolated at `:371` into `OLD_ROOT_CONTEXT_WITH_COORDINATION_MD` (`static`, `:340`), which is entry 2 of family 1's `old_generated` array at `:733`. Round 1's §5.9 named `root_agent.rs:297`, which is line 6 of that constant.

#### Family 5: the injected PTY message templates (unnamed by round 1 and by both reviewers)

`config/injected_messages.rs` manages `injected-messages.toml` in the user's config directory. `MessageSpec.known_default_sha256` (`:68`) holds "sha256 of every default ever shipped for this id"; reconciliation at `:975-980` hashes the user's `template` value and refreshes the entry only when the hash is recognized. The file header the product writes states the contract: *"An entry you have not edited is refreshed automatically when a new AgentsCommander version ships a better default; an entry you HAVE edited is never overwritten"* (`:104-106`).

| Item | Site | Classification |
| --- | --- | --- |
| `TOKEN_WORKGROUP = "%WORKGROUP%"` | `:46` | **Rule P0.** A placeholder token resolved by `expand_tokens`. It appears in every user-edited template on disk; renaming it silently stops expanding in each one. |
| `DEFAULT_CONTEXT_ALERT_TEMPLATE` | `:52` | **Rule P0, unchanged.** Its only occurrence of the concept is the `%WORKGROUP%` token. Its bytes are pinned by `known_default_sha256 = ["e672581d47e7e4a4749b510f23eff72982ff3fa5261109122b3bdf8fdfda153f"]` (`:85`) and by the comment at `:50-51` (125 characters, 125 UTF-8 bytes). Rule R moves nothing in it, so no new hash is appended. |
| `CONTEXT_ALERT_DOC_COMMENT:78` | `:73-80` | **Rule R.** `#   %WORKGROUP%   workgroup name, e.g. wg-2-dev-team` is prose written into a user-owned file. It is never compared (only `template` is hashed, `:975`), so renaming the prose around the token is safe. The token itself stays. |
| `docs/features/context-tracking.md:75`, `:78` | docs | **Mixed.** `%WORKGROUP%` stays; the surrounding prose takes Rule R. |

`PTY_INPUT_COORDINATOR_CONTEXT` (`session_context.rs:2495`) carries the word twice and belongs to **no** family: it is injected context, never written to a user-owned file, so it is renamed with no snapshot and no version bump.

### 3.8 GUI visible-text inventory, re-derived from a reproducible sweep

Round 1 presented this inventory as measured and exhaustive, and it was neither. Two things were wrong and both are corrected here.

**Correction A: the printed sweep does not produce the printed number.** Round 1's command omitted lower-case `wg`, so it returns **706 lines across 40 files**, not the 906 across 43 it claims. The sweep that actually returns 906 must include the lower-case token, and the file count is 41, not 43. Both numbers are re-measured below.

**Correction B: seven production sites were missing**, and none of them was reachable by any of round 1's 36 needles. They are listed in the class tables below and marked **NEW**.

#### The sweep, binding

```
git grep -nE '[Ww]orkgroup|WORKGROUP|(^|[^A-Za-z0-9_])([Ww][Gg])([^A-Za-z0-9_]|$)|(^|[^A-Za-z0-9_])wg-' \
  <rev> -- src \
  | sed 's/^<rev>://' | grep -E '^src/[^:]*\.tsx?:' | grep -vE '^src/[^:]*\.test\.tsx?:'
```

At `d7008b34` this returns **906 lines across 41 files**. Reproduced twice by two different pathspec routes (`src/**/*.tsx` glob magic and a post-filter over `-- src`), byte-identical output both times. The four alternatives exist because each covers a shape the others miss:

- `[Ww]orkgroup|WORKGROUP` has **no** word boundary on purpose. A boundary that excludes `-` loses `same-workgroup` (`SettingsModal.tsx:2089`) and `Cross-workgroup` (`AgentPickerModal.tsx:964`); a boundary that includes `-` as a separator loses nothing but drags in every identifier, which is what the allowlist is for.
- the `[Ww][Gg]` alternative is bounded on both sides, so it catches `WG` and the bare `wg` binding without matching `wgName`.
- `wg-` carries a leading boundary and **no** trailing one, because `placeholder="wg-2.*"` (`ProjectPanel.tsx:2542`) has a digit after the hyphen and a trailing-boundary form misses it.

Per-file distribution at base, measured, all 41 rows, paths relative to `src/` and summing to 906:

`sidebar/components/ProjectPanel.tsx` 314; `sidebar/components/WorkgroupGroupRail.tsx` 105; `sidebar/components/WorkgroupGroupsModal.tsx` 54; `sidebar/stores/workgroup-groups.ts` 42; `terminal/stores/terminal.ts` 26; `sidebar/components/AgentPickerModal.tsx` 25; `shared/types.ts` 25; `terminal/components/WorkgroupTask.tsx` 24; `sidebar/components/AcDiscoveryPanel.tsx` 22; `sidebar/components/workgroup-session.ts` 21; `sidebar/stores/sessions.ts` 20; `sidebar/stores/project.ts` 20; `shared/ipc.ts` 19; `resource-monitor/App.tsx` 18; `watchers/App.tsx` 17; `sidebar/components/NewLoopModal.tsx` 15; `sidebar/App.tsx` 15; `sidebar/components/EditLoopModal.tsx` 14; `sidebar/stores/project-merge.ts` 10; `terminal/App.tsx` 9; `sidebar/components/loop-modal-helpers.ts` 9; `shared/testing/ui-harness.tsx` 9; `watchers/activity.ts` 8; `sidebar/components/ActionBar.tsx` 8; `sidebar/watchdog/non-stop-watchdog-client.ts` 7; `sidebar/components/NewWorkgroupModal.tsx` 7; `sidebar/components/SettingsModal.tsx` 6; `sidebar/components/replica-dot.ts` 5; `sidebar/stores/team-idle-watcher.ts` 4; `sidebar/components/replica-repo-badges.ts` 4; `sidebar/components/ArchivedProjectsModal.tsx` 4; `terminal/components/Titlebar.tsx` 3; `sidebar/stores/project-collapse.ts` 3; `sidebar/components/Titlebar.tsx` 3; `shared/path-extractors.ts` 3; `sidebar/components/TeamContextAlertsEditor.tsx` 2; `shared/profile-utils.ts` 2; `terminal/components/TaskCleanConfirmModal.tsx` 1; `sidebar/components/workgroup-delete-diagnostics.ts` 1; `sidebar/components/SessionItem.tsx` 1; `guide/components/HintsTab.tsx` 1.

That distribution is the enumeration §6.2 and AC1 are derived from. It is not itself the Rule R set: the Rule R set is the four classes below, and everything else is Rule P and lands on AC1's committed allowlist.

#### (a) JSX text nodes, 24 lines

`resource-monitor/App.tsx:677`; `watchers/App.tsx:851`; `AcDiscoveryPanel.tsx:263`; `EditLoopModal.tsx:228`, `:242`; `NewLoopModal.tsx:171`, `:185`; `NewWorkgroupModal.tsx:59`, `:95`; `ProjectPanel.tsx:2616` **NEW**, `:2703`, `:2708`, `:2760`, `:2767`, `:2795` **NEW**, `:3117` **NEW**, `:3361` **NEW**, `:3773`, `:3777` **NEW**, `:3835` **NEW**, `:3899`, `:3998` **NEW**; `SettingsModal.tsx:2089` **NEW**, `:2263` **NEW**.

Plus three that a single-line `>text<` needle cannot see and that are therefore enumerated explicitly:

- `AgentPickerModal.tsx:944` — `Entire workgroup <span ...>`, split across a tag boundary.
- `AgentPickerModal.tsx:949` **NEW** — `Counts are read from the current workgroup; the backend re-enumerates targets before applying.`
- `AgentPickerModal.tsx:972` **NEW** — `{scopePreview()!.targetCount} replica(s) across {distinctWorkgroupCount()} workgroup(s) ·`.

And two files round 1 left out of §6.2 entirely:

- `TeamContextAlertsEditor.tsx:78-79` **NEW** — `Applies to every workgroup of this team. When a member ... sends that workgroup&apos;s orchestrator an informational notice ...`. Note `&apos;`: the possessive is HTML-escaped, so a `workgroup's` needle misses it.
- `TaskCleanConfirmModal.tsx:72` **NEW** — `This <strong>resets</strong> the workgroup TASK.md — all frontmatter fields ...`.

The exact texts of the seven **NEW** sites, quoted from the blob so a reviewer can re-anchor without the line numbers:

| Site | Text |
| --- | --- |
| `ProjectPanel.tsx:3117` | `Delete agent <strong>{...}</strong>? This will remove the agent matrix, its workgroup replicas, and team assignments.` |
| `ProjectPanel.tsx:3777` | `Delete workgroup <strong>{deletingWg()!.name}</strong>? This will remove the workgroup directory and all its contents.` |
| `ProjectPanel.tsx:3835` | `<strong>Cannot delete:</strong> Windows reported the workgroup is locked.` |
| `ProjectPanel.tsx:3998` | `Delete team <strong>{...}</strong>? This will remove the team configuration and all associated workgroups.` |
| `SettingsModal.tsx:2089` | `Allow authorized Root Agents and same-workgroup Orchestrators to capture live terminal` |
| `SettingsModal.tsx:2263` | `Load a project with workgroup replicas before minting an API client.` |
| `AgentPickerModal.tsx:949` | `Counts are read from the current workgroup; the backend re-enumerates targets before applying.` |

`ProjectPanel.tsx:2616`, `:2795` (`New Workgroup`) and `:3361` (`Delete Workgroup`) were caught by a round-1 needle but were absent from §3.8(a)'s own list, which is how a needle set can be green while the enumeration it is supposed to backstop is incomplete.

#### (b) Attribute and label-constant strings, 5 lines

`resource-monitor/App.tsx:673` (`aria-label="Filter by workgroup"`); `ActionBar.tsx:15` (`SELECTED_WORKGROUP_VISIBILITY_LABEL = "Always keep selected workgroup visible"`, consumed by `title` and `aria-label` at `:334`/`:335`); `ProjectPanel.tsx:1434` (`` title={`Watch this workgroup in the ${DEFAULT_NON_STOP_NAME} group`} ``); `ProjectPanel.tsx:2542` (`placeholder="wg-2.*"`); `SettingsModal.tsx:889` (`"Load a project with at least one workgroup replica before minting."`).

#### (c) Message and label string literals, 18 lines

`HintsTab.tsx:70`; `AgentPickerModal.tsx:436`; `NewWorkgroupModal.tsx:36`; `ProjectPanel.tsx:814`, `:827`, `:993`, `:995`, `:1451`, `:1455`, `:3949`, `:3961`; `workgroup-groups.ts:587`, `:597`, `:612`, `:637`, `:659`, `:675`, `:685`.

#### (d) The visible short label produced by code, 2 producing lines rendered at 3 places

`shared/path-extractors.ts:25` (`wg.toUpperCase()`, the uppercased directory name for the titlebar badge) and `WorkgroupGroupRail.tsx:73` (`` `WG${match[1]}` ``, the rail tooltip label). These are F1 and F5 of §3.3 and are the only places where the visible text is computed rather than written. They are rendered at three places: `sidebar/components/Titlebar.tsx:218`, `terminal/components/Titlebar.tsx:83` and the rail tooltip.

**Corrected in round 3.** Round 2's header said "3 sites" and enumerated two, and it cited `WorkgroupGroupRail.tsx:72`, which is F5's `` /^wg-(\d+)/i `` predicate line owned by §5.4, not the line that emits the label. The label is at `:73`. Both lines are in the base sweep; only `:73` is Rule R. The corrected Rule R line total for §3.8 is stated in the next paragraph and is what AC1 and §9.4's allowlist arithmetic use.

**The §3.8 Rule R set, counted as lines, so §9.4 AC1 can subtract it.** (a) 24 in the main list, plus 6 in the tag-boundary and missed-file lists (`AgentPickerModal.tsx:944`, `:949`, `:972`; `TeamContextAlertsEditor.tsx:78`, `:79`; `TaskCleanConfirmModal.tsx:72`); (b) 5; (c) 18; (d) 2. **Total 55 lines.** Every one of the 55 was checked to be present in the base sweep output and none is duplicated.

**Three further groups of frontend line move without being Rule R, and §9.4 AC1 subtracts them too.** They are named here because content is the allowlist key, so a line whose content this plan changes must not sit in a base-derived allowlist: the three R1 resolvers `ProjectPanel.tsx:1057`, `:1058` and `:1071` (the trap restated below), and the five §5.4 prefix-predicate lines not already inside the 55, namely `profile-utils.ts:124`, `:472`, `WorkgroupGroupRail.tsx:67`, `:72` and `WorkgroupTask.tsx:74`. All eight were likewise verified present in the base sweep. **63 frontend lines therefore move in total, and the base allowlist's frontend half is `906 - 63 = 843` rows** (§9.4 AC1 point 8).

#### The one trap that survives from round 1, restated

`ProjectPanel.tsx:993` and `:995` produce the row labels `"Selected Workgroup"` and `"Workgroups"`, and `:1057`, `:1058` and `:1071` pass those **same literals** back into `matchesFilterText(...)` and `workgroupMatches(wg, ...)` as the search text the sidebar filter matches against. §5.2 clause R1 covers it: both sides move together, in the same commit, or the filter silently stops matching the row it names.

`ProjectPanel.tsx:1447` (`removeExactGroupToken(group.regex, wg.name)`) and `WorkgroupGroupRail.tsx:76` (`groupMatches`) evaluate **user-authored** regexes against the directory name. A user whose saved group regex is `^wg-` will not match a Room. That is a behavior consequence of (A), not a defect, and §7 records it with the mitigation.

#### Why round 1's needle set was not a backstop, and what replaces it

A needle set can only re-find what the enumeration that produced it already found, so it cannot detect an enumeration miss; `git grep -F` is additionally case-sensitive, which is what let `Delete workgroup` (`:3777`) pass under a `Delete Workgroup` needle. The 36 needles are therefore **deleted**, not extended. AC1 (§9.4) replaces them with the total sweep above plus a committed, enumerated Rule P allowlist, where a miss appears as a line that is not on the allowlist rather than as a needle nobody wrote.

### 3.9 Documentation inventory

57 files, 445 matching lines, from `git grep -ic workgroup -- docs`. The ten densest: `docs/testing/04-team-and-workgroup-lifecycle.md` (60), `docs/reference/cli.md` (50), `docs/agent-matrix-conventions.md` (34), `docs/agents/teams-and-workgroups.md` (27), `docs/reference/architecture.md` (19), `docs/testing/08-inter-agent-messaging.md` (15), `docs/testing/05-end-to-end-user-journey.md` (14), `docs/features/sidebar-guide.md` (14), `docs/features/non-stop-mode.md` (14), `docs/agents/inter-agent-messaging.md` (14).

Outside `docs/`: `CHANGELOG.md` (12), `ROADMAP.md` (6), `README.md` (4), `src-tauri/src/api/README.md` (3), `PRIVACY.md` (2).

`docs/agent-matrix-conventions.md:548` carries the message-filename convention `YYYYMMDD-HHMMSS-<wgN>-<from>-to-<wgN>-<to>-<slug>.md`, which §5.8 changes in step with `parse_wg_prefix`.

Two documentation **file names** carry the word: `docs/testing/04-team-and-workgroup-lifecycle.md` and `docs/agents/teams-and-workgroups.md`. They are on-disk identifiers with inbound links and are Rule P (§5.2); #1615 owns them.

`docs/assets/og-card.svg` matches only inside a base64 image payload and is Rule P.

### 3.10 Machine-readable values that must not move

Each of these is resolved by code or crosses a process boundary, and each is Rule P:

| Value | Site | Why it cannot move |
| --- | --- | --- |
| `"purge-wg"` | `phone/mailbox.rs:1174` `PURGE_WG_ACTION`, read at `:6842`, written at `cli/purge_wg.rs:162` | Outbox `action` wire value. An in-flight message written by an older CLI must still be handled, and a message written by the new CLI must still be handled by a daemon that has not restarted. Requirement (F) says nothing in flight may break. |
| `"workgroupCreated"`, `"workgroupRemoved"` | `cli/workgroup.rs:240`, `:322`; `src/shared/types.ts:1475-1476` | `ProjectRefreshRequest` reason codes, matched on the frontend by exact string. |
| `"workgroup"` JSON key | `cli/workgroup.rs:325`; the serde fields of `TeamListItem`, `AddMemberResult`, `RemoveMemberResult` in `cli/team.rs`; `LoopTarget` | CLI stdout is a script contract, and persisted config keys are explicitly out of scope. |
| `"workgroupCoordinator"` | `src/shared/types.ts:1320` `LoopTargetKind` | Persisted Loop config value. |
| `"workgroup"` scope value | `src/shared/types.ts:1413` `ProfileAssignmentScope`, used at `AgentPickerModal.tsx:389`, `:477`, `:935-942`, `:1055` | Persisted profile-assignment scope, and the `data-ac-testid` `agentPicker.scope.workgroup` at `:936`. |
| `"workgroup_task_updated"` | `src/shared/ipc.ts:1371` | Tauri event name. |
| `"selected-workgroup"`, `"workgroups"`, `"workgroup"` | `src/sidebar/stores/project-collapse.ts:9-11`, used at `ProjectPanel.tsx:1872-1873`, `:2163`, `:2481`, `:2770` | Persisted collapse-state keys. Renaming them resets every user's collapsed sections. |
| `ac-wg-*`, `workgroup-group-*`, `workgroup-groups-*` | CSS class names throughout `src/` and `src/sidebar/styles/sidebar.css` | Rule P: class names are identifiers. |
| `"%WORKGROUP%"` | `config/injected_messages.rs:46` `TOKEN_WORKGROUP`, expanded by `expand_tokens`, documented at `:78` and in `docs/features/context-tracking.md:75` | A placeholder token in the user's `injected-messages.toml`. Every template a user has edited on disk contains it; renaming it makes each one silently stop expanding. Only the token's human *description* moves (§3.7 family 5, D17). |
| `DEFAULT_CONTEXT_ALERT_TEMPLATE`'s bytes | `config/injected_messages.rs:52`, hashed at `:85` as `known_default_sha256 = ["e672581d47e7e4a4749b510f23eff72982ff3fa5261109122b3bdf8fdfda153f"]`, pinned as 125 bytes at `:50-51` | The shipped-default hash that decides whether a user's entry auto-refreshes. Its only occurrence of the concept is the `%WORKGROUP%` token, so Rule R moves nothing in it and the hash list must stay a single entry. |
| `cli::workgroup::tests::*`, `...::measure_default_context_size_for_workgroup_replica`, `...::cli_workgroup_deletion_takes_project_gate_only_cross_process` | `test-debt.allowlist.json:157`, `:197`, `:205`, `:237` | The allowlist pins tests by fully-qualified Rust path. Renaming `cli/workgroup.rs` or any of those test functions breaks the `test-debt` CI job. This is the concrete reason source-file and identifier renames belong to #1615. |

### 3.11 CI and local gate inventory, derived from the target-base workflow files

`.github/workflows/pr-regression-gates.yml` triggers on `pull_request` with **no `paths` filter**, so every job runs on this PR. Eight jobs:

| Job | Runner | Command that matters here |
| --- | --- | --- |
| `test-debt` | ubuntu | `npm run test:debt`, `npm run test:classify:self`, `npm run test:report:self` |
| `rust-regression` | windows | `cargo test` (the only leg that actually runs the Rust test suite) |
| `rust-regression-linux` | ubuntu | build/clippy leg |
| `rust-regression-macos` | macos | build/clippy leg |
| `rust-fmt` | ubuntu | `cargo fmt --all -- --check` in `src-tauri` |
| `terminal-snapshot-portable` | matrix | unaffected by this change |
| `windows-release-cli-smoke` | windows | `npm run build:prod:no-bundle` then `npm run smoke:cli-release-windows` |
| `frontend-regression` | ubuntu | `npm run typecheck` then `npm test` with the #480 known-debt guard |

Plus `bundle-validation` (pull_request, windows), `lockfile-check`, `validate-branch-name` and `version-sync-check`.

`scripts/smoke-cli-release-windows.ps1` and `scripts/smoke-cli-powershell.ps1` exercise `list-peers`, `list-peers-lean` and `terminal-snapshot --help`; neither invokes `workgroup`, `purge-wg` or any renamed flag, so `windows-release-cli-smoke` is unaffected by the CLI rename. Verified by `git grep -n 'purge-wg\|workgroup' -- scripts/` returning no smoke-script hit.

Local-only gates, not run by any workflow:

- `npm run check:frontend-dependencies` (dependency-cruiser 18.0.0, config `dependency-cruiser.config.mjs`, rules `no-circular` and `no-terminal-helper-back-edge`). It is not referenced by any workflow file, so it is a local evidence step owned by the implementer (§13.3).
- `npm run record:arcs` (`scripts/02-module-arc-record.mjs`) plus the levelization detector in `repo-personal` (§3.13).

`no-terminal-helper-back-edge` constrains only `src/terminal/components/terminal-session-registry.ts` and `terminal-output-admission.ts`, neither of which this change touches.

### 3.12 Byte evidence for every frozen copy this plan makes (computed at `d7008b34`, not assumed)

Every value below is taken from the git blob at `d7008b34` (LF), never from the CRLF worktree. `core.autocrlf` is `true` and `*.rs` carries no `.gitattributes` `eol` rule, so a worktree digest is not reproducible.

Two kinds of value appear, and the difference is load-bearing:

- a **declaration-range digest** covers the source lines including the `const ... = r#"` opener and the `"#;` closer. It proves the implementer copied the base literal rather than retyping it. It is checkable by a reviewer with `git cat-file blob` and `awk`, and it is what §9.4 AC7's copy check uses.
- a **rendered-value digest** covers the bytes the constant evaluates to. It proves the *behavior* is preserved even if the declaration is reindented, and it is what a `#[test]` can assert directly with `Sha256::digest(CONST.as_bytes())`. The repository already uses exactly this form: `root_context_pre_orchestrator_rename_snapshot_is_byte_exact` (`root_agent.rs:2411`, body `:2410-2426`) pins `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD` at 2464 bytes and sha256 `e244249c...`, with a doc comment stating the value was "captured by a one-off run of the shipped constant AT ecc6527b ... never from this const". This plan follows that precedent for every new snapshot.

#### Table A: declaration-range digests

| Snapshot source | Blob path | Line range | Bytes | SHA-256 |
| --- | --- | --- | --- | --- |
| Global template declaration, `    r#"` through `"#` | `src-tauri/src/config/session_context.rs` | 2470-2492 | 549 | `991C5BA85173709234CC55E959D234116D4ACDA73153713303ACA22CAE85F0AC` |
| Coordinator template declaration | same | 2509-2529 | 2703 | `CC127468024A85C5863F693E770AEE9BBED82E61C3D28A7DF70B521877563ABF` |
| `ROOT_ROLE_MD` declaration, `const ... = r#"` through `"#;` | `src-tauri/src/config/root_agent.rs` | 675-723 | 2501 | `9713681065D83A8A73B05F07970C3176BB19E6914FDB1A1EE4DCD317AA8CA095` |
| **`WORKGROUP_GIT_SCOPE` declaration** (new, §5.10 split) | `src-tauri/src/config/session_context.rs` | 3368-3368 | 258 | `85876201638A24F13FAD76B7AEE0429489C785DE6EDC993798C59701DEE47451` |
| **`ROOT_COORDINATION_MESSAGING_PARAGRAPH` declaration** (new, §5.10 split) | `src-tauri/src/config/root_agent.rs` | 292-306 | 956 | `17D7303AB17923357842B7D6FF24B921CB8B6D027BBB06F4BC3A07E57177E517` |
| **`OLD_DEFERRED_MESSAGING_PARAGRAPH` declaration** (new, frozen in place) | `src-tauri/src/config/root_agent.rs` | 290-290 | 344 | `3DC963DB7B12E24D522BC981DB364E5E2BA1A656BF0EFE0DF23F25E86DB34E93` |
| **`legacy_rendered_default_context_for_generation`, whole function** (new, frozen in place, §3.7 family 3) | `src-tauri/src/config/session_context.rs` | 3769-4025 | 15027 | `940FA35733C78CDF513391E5AED64438AFD50FE6472A26E4D317270E5EE716C2` |
| **`LEGACY_GIT_SCOPE_*_BEFORE_1072` pair** (already frozen; in family 3's closure) | `src-tauri/src/config/session_context.rs` | 3735-3736 | 1289 | `DADA740FD5EE0EF3141E3AEE6C3920074F83776B2051F355C6D1B387781D0421` |
| **`render_skills_section`'s replica line** (new in round 3, §5.10 D8f, §3.7 family 3) | `src-tauri/src/config/session_context.rs` | 831-831 | 152 | `A9DC92441D915A1251CFC148431D87EEC2C7430A2BEC3AAA1714B9CD978CAFD3` |

Reproduce any row with:

```
git cat-file blob d7008b34:<path> | awk 'NR>=<a> && NR<=<b>' | sha256sum
```

#### Table B: rendered-value digests

These are the values a Rust test asserts. Round 1 declined to compute the coordinator one, calling it undecodable without a Rust string-literal decoder; it is supplied here, decoded twice by two independently written decoders (a JavaScript one and a Python one taking a different route: regex line-continuation collapse followed by the host language's own literal parser) that agree byte for byte. The same JavaScript decoder reproduces the global and root rendered digests below, which are independently verifiable as raw literals, so the decode path is validated against known-good values before being trusted on the escaped one.

| Value | Source | Bytes | SHA-256 |
| --- | --- | --- | --- |
| `get_default_agent_template()` | `session_context.rs` 2470 after `r#"` through 2491, plus the final newline | 539 | `F44065965F3C53C8B8D2C2E6B3D38C68B998F848AE893EDDB7E64085A3C5316A` |
| `get_default_coordinator_template()` | `session_context.rs` 2509-2529, escapes and `\`-continuations decoded | **2516** | `0B89EB38608F6272F0D8087FC7DF13ECC729FDA716ABA972673B15B734A2198E` |
| `ROOT_ROLE_MD` | `root_agent.rs` 675 after `r#"` through 722, plus the final newline | 2467 | `7F82F28C70221C8476BB957F5978433173F60E388A9F18DB729E5C2BF014C52D` |
| `WORKGROUP_GIT_SCOPE` | `session_context.rs:3368` string value | 220 | `A386B52DA8246826689215A8F07ABF3CB58D01EBCC18AFC530730157AA12566D` |
| `ROOT_COORDINATION_MESSAGING_PARAGRAPH` | `root_agent.rs` 292 after `r#"` through 306 before `"#;` | 897 | `FC2164A2A56957E481DEBCA460F9DF3CC681A634EDDA58F5270939C85668F207` |
| `OLD_DEFERRED_MESSAGING_PARAGRAPH` | `root_agent.rs:290` string value | 293 | `6E12E68E51C3C6DF2386728DFD0ED98BFE06A8A0C3F6383BFAF8FD4463C7A463` |
| `GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME` (new in round 3, D8f) | `session_context.rs:831` string value, the single `\n` escape decoded | 131 | `A5C74FD65F2C2562D4C651F1C6E972684DE9F2DBE1924E6153B26D4CB9C57EC9` |

**The last row names the frozen constant, not the live one, and round 4 corrected its label.** Round 3 wrote it as `GENERATED_SKILLS_SECTION_REPLICA_LINE`, which is the **post**-rename constant: after D8f, the constant of that name holds the Room text and is 126 bytes, while these 131 bytes are the **pre**-rename text that D8f freezes as `GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME`. The `WORKGROUP_GIT_SCOPE` row above follows a base-tree-name convention, but that convention does not carry here, because the frozen constant does not exist at `d7008b34` while `WORKGROUP_GIT_SCOPE` does. The `Source` column and both consumers (D8f step 2, and §9.1's `skills_section_replica_line_split_is_correct` and AC7.14) already bound the digest to the frozen half correctly, so nothing downstream moves. `ac-dev-rust-grinch-v3` found this in round 3.

The round-3 row is the decoded value of the one-line literal at `:831`: 131 bytes, 131 characters, one `\n` escape and no other escape, ending in that newline. **The trailing newline is part of the constant and part of the digest**; a copy that drops it hashes differently and `is_provably_generated_legacy_skills_section` stops matching. Reproduce with:

```
printf 'When running from a workgroup replica, resolve skills/... against the origin Agent Matrix path above, not against the replica CWD.\n' | sha256sum
```

**The coordinator row is the one value in this plan that was computed rather than read.** If the implementer's first run of `assert_eq!(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len(), 2516)` reports a different length, the decode above was wrong, and **the declaration-range digest `CC127468...` governs**: the copy is correct if and only if the declaration range matches, and the implementer then records the observed length and digest in the test and in the PR body, citing this paragraph. Both reviewers reproduced `CC127468...` independently in round 1, so that authority is already established.

#### Table C: the one value this plan cannot compute, and how it is captured

The rendered output of the two legacy reconstructions cannot be derived from source without executing them, and reimplementing 250 lines of Rust `format!` logic by hand would risk writing a wrong digest into a plan. They are therefore captured, not asserted here.

**Round 3 captures both reconstructions, not one.** Round 2 captured only `legacy_rendered_default_context_for_compat`. The `StaleGenerated` arm of `classify_legacy_rendered_default_context` accepts **either** candidate that `reconstruct_legacy_rendered_default_context` returns at `:4152-4163`, and the pre-#1072 one is the candidate that actually carries the `StaleGenerated` outcome, so pinning only the first leaves half the freeze unpinned.

| # | Function | Fixed inputs | Captured at step 0 |
| --- | --- | --- | --- |
| C1 | `legacy_rendered_default_context_for_compat` | as below | length + sha256 |
| C2 | `pre_1072_legacy_rendered_default_context_for_compat` | the same three | length + sha256 |

- **When.** Step 0 of §12, at `d7008b34`, **before the first product edit**.
- **How.** Add `legacy_rendered_default_context_is_frozen` (§9.1) with two deliberately wrong expected values, run it once, and read the actual lengths and digests out of the assertion failures. Then set them and record them, with the base SHA, in the test's doc comment and in the PR body.
- **Fixed inputs, so the captured values are reproducible:** `agent_root = "C:/fake/.ac/wg-7-dev-team/__agent_architect"`, `matrix_root = Some("C:/fake/.ac/_agent_architect")`, `skills_section = ""`. These are synthetic constants, never a `tempfile` path, and that is deliberate: a digest taken over a real temporary directory is not reproducible, because the path varies per run and is interpolated into the output. `workgroup_root` is a pure path walk with no filesystem touch (`phone/messaging.rs:141-157`), so `C:/fake/.ac/wg-7-dev-team/__agent_architect` reaches `MessagingContextMode::Workgroup` without either directory existing, and `root_agency_cache_guidance` returns `""` for a non-root root. Both captures are therefore deterministic.
- **This fixture is for the digests only.** The behavioral criteria that prove the classifier still reaches `StaleGenerated` and `Current` need real directories and a `render_skills_section`-derived skills section, and they get their own fixtures in §9.1, modelled on the repository's working `pre_1072_legacy_with_matrix_classifies_stale_and_heals_once` (`:6180-6210`) and `legacy_intro_skills_section_still_classifies_stale_generated_and_heals` (`:10016`). Round 2 used the digest fixture for both jobs and that is what made its behavioral assertion unsatisfiable (see §9.1 and §7 item 10).
- **Why this is not self-referential.** The values are taken at the frozen base and hard-coded. A later coordinated rename that moved both the functions and their test would change their output and the hard-coded digests would fail. That is the property round 1's AC7 lacked.
- **How a reviewer checks a deferred capture.** Check out `d7008b34`, apply only the test file, and run it: the two values it asserts must be green on the base tree. That is the whole verification, and it needs nothing from the change.
- **Redundancy.** Table A's `940FA357...` row already pins the same freeze from the source side and needs no execution, so the freeze has a checkable gate even if the capture is skipped. The two are complementary: the source digest proves no byte of the function moved; the output digests prove nothing in their closure moved either.

### 3.13 Dependency-cycle baseline, measured on the clean base tree

`node "<repo-personal>/ObsidianVault/Coding Agents/IA-Programming/rust/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json --quiet` exits **1**, which is the normal outcome when gating cycles exist, and still writes the graph (only exit 3 means no graph).

| Measure | Base value |
| --- | --- |
| Tool | `rust-module-dependency-cycles 1.1.0` |
| Files scanned / modules resolved | 219 / 191 |
| Module edges | 3741 |
| `summary.moduleCycles` (cyclic SCCs) | **1** |
| That SCC's size | **85 modules** |
| `functionCyclesCrossModule` | 0 |
| Committed `src-tauri/module-arcs.txt` | 1037 arcs, 82,149 bytes, and regenerating it from `pre.json` is **byte-identical** (`cmp` clean) |

The single cyclic SCC contains, among its 85 members, `agentscommander_lib`, `::cli`, `::cli::list_peers`, `::commands::ac_discovery`, `::commands::config`, `::commands::entity_creation`, `::config::coding_agent_profiles`, `::config::loops`, `::config::placeholders`, `::config::root_agent`, `::config::seeded_context_templates`, `::config::session_context`, `::config::teams`, `::phone::mailbox`, `::phone::messaging`, `::pty::container_paths`, `::pty::container_repos` and `::session::session`.

It does **not** contain `agentscommander_lib::config`, `::config::ac_root`, `::config::replica_identity`, `::commands::task`, `::cli::role_experiment`, `::cli::purge_wg`, `::cli::workgroup`, `::cli::team`, `::cli::loop_cmd` or `::screenshot::windows`. §11 uses that split.


### 3.14 Rust reader-facing string inventory, built the way §3.8 was built

Round 1 had a measured inventory for the GUI and none for Rust, while §6 called itself exhaustive and §13.2 gate 5 required "exactly the §6 paths changed". A correct edit therefore tripped the scope gate and an obedient one shipped the misses. Both halves are fixed: this section supplies the inventory, §6.1 is regenerated from it, and gate 5 is reworded (§13.2).

#### The sweep, binding

```
git grep -nE '[Ww]orkgroup|WORKGROUP|(^|[^A-Za-z0-9_])([Ww][Gg])([^A-Za-z0-9_]|$)|(^|[^A-Za-z0-9_])wg-' \
  <rev> -- src-tauri/src
```

At `d7008b34` this returns **3090 lines across 94 files**. Narrowed to production reader-facing candidates, that is **1174 lines across 63 files**, of which 582 are reachable by the line-based rule below and 592 are in its blind class. Round 2 stated this inner figure as 544 across 55; the file count was right and the line count was an artifact of step 1, as the correction after the rule sets out. The narrowing rule, stated so it is reproducible:

1. drop every line inside a `#[cfg(test)]` item. **A column-0 `#[cfg(test)]` attributes the single item that follows it, which may be a `mod`, a `use`, a `fn`, a `thread_local!` or a `#[derive]`ed type; brace-match when that item opens a block and stop at the terminating `;` when it does not.**
2. drop every non-doc `//` comment (Rule P1);
3. keep a line when it **carries a `"` character**, or when the line is a `///` or `//!` doc comment (Rule P1's `clap` carve-out makes doc comments load-bearing, and a doc comment that contradicts its code is a defect regardless).

**Step 3 is restated in round 4, because round 3's form did not produce round 3's own number.** Round 3 wrote step 3 as "keep a line when the token falls **inside a `"..."` literal**". Implemented literally, that returns **536 / 55**, not 582, and 536 + 592 = 1128 leaves 46 of the 1174 candidates in neither bucket. The quote-presence form above returns exactly **582 / 55** and closes the totals: 582 + 592 = 1174. It is also the exact complement of the blind class stated two paragraphs down, which was already written in the quote-presence form, so this restatement makes the section internally consistent rather than changing a measurement. **Both readings return the identical 55-file set**, so §6.1's partition is unaffected either way and nothing in the change depends on the choice. `ac-dev-rust-v3` found this in round 3 and measured both readings; `ac-dev-rust-grinch-v3` independently reproduced 582 / 55 from the quote-presence form.

**Step 1 is restated in round 3 because round 2's version was false and the falsehood was load-bearing for the distribution below.** Round 2 claimed "the crate uses only the column-0 `#[cfg(test)] mod ... {` shape". Measured at `d7008b34` across the 94 swept files, that is wrong: column-0 `#[cfg(test)]` also attributes `use` items, free functions, `thread_local!` blocks and `#[derive]`ed types. `config/root_agent.rs:2` is `#[cfg(test)]` on `use std::collections::HashMap;`, and it is the **first** such attribute in the file. Any implementation that reads step 1 as "everything from the first column-0 `#[cfg(test)]` onward is test code" therefore discards that whole file, which is exactly why round 2's distribution credited `config/root_agent.rs` with **3** candidate lines while the raw sweep returns **50** and all six `ROOT_ROLE_MD` occurrence lines (`:687`, `:698`, `:702`, `:704`, `:710`, `:712`) are outside it.

**The blind class, stated as a class and measured rather than enumerated by hand.** The rule keeps a line only when it carries a `"` or is a `///`/`//!` comment, so its blind class is **every line of a multi-line string literal that carries no quote character**, raw and `\`-continued literals alike. That is a class, not a short list, and round 2's claim that it had "two shapes ... enumerated by hand" is withdrawn. Measured at `d7008b34` over the swept lines, with `#[cfg(test)]` scoped per item as step 1 now states: **592 blind lines across 44 files**. Under round 2's whole-file-cut reading the same measurement returns 398 across 39, which is the number both round-2 reviewers reported; the difference is the reading, not the tree. **Recorded in round 4 so a third measurement is not surprised:** in round 3 `ac-dev-rust-v3` reproduced 592 / 44 exactly, while `ac-dev-rust-grinch-v3` measured 588 / 44 and attributed the 4 to its own `#[cfg(test)]` item-boundary heuristic, since the kept figure and all three file counts reproduced exactly. 592 stands on the reproduced measurement; a re-measurement landing within a few lines of it on the blind class alone is a boundary-heuristic difference and not a finding, and no figure §6.1 uses depends on it.

**What the correction does and does not move.** All four figures below are measured at `d7008b34` with step 1 scoped per item.

| Measure | Round 2 | Round 3, corrected |
| --- | --- | --- |
| lines the rule **keeps** (quote-bearing or `///`/`//!`) | 544 / 55 files | **582 / 55 files** |
| lines the rule is **blind** to | "two shapes", not counted | **592 / 44 files** |
| production reader-facing candidates, total | not stated | **1174 / 63 files** |

- The **file** partition, which is what §6.1 is built from, is **unchanged**, and the correction independently validates it. Exactly the same **55** files carry a kept line. Eight further files carry only blind-class lines: `cli/loop_cmd.rs` and `cli/self_switch.rs`, which §6.1 already names as two of its three edited-but-uncounted files, and six that are in neither §6.1 list, namely `cli/new_project.rs`, `cli/open_project.rs`, `commands/loops.rs`, `commands/project_settings.rs`, `lib.rs` and `resource_monitor/registry.rs`. Every hit in all six was read: every one is a Rule P0 identifier, struct field, function name, type name or `use` path (`pub workgroup`, `load_workgroup_groups`, `WorkgroupGroupsConfig`, `self.metadata.workgroup`, `use crate::...`), none is reader-facing text, and all of them belong to #1615. So the corrected reading adds **no** file to either §6.1 list, and it explains two of the three §6.1 exceptions as genuine blind-class members rather than as hand-waved ones.
- `cli/terminal_snapshot.rs` likewise stays out of both lists: its three hits (`:912`, `:981`, `:999`) are FQN fixtures inside the `mod tests` whose `#[cfg(test)]` attribute is at `:716` and whose `mod tests` line is `:717`, exactly like `api/identity.rs` (§6.4). **The file carries a second `#[cfg(test)]`, at `:398`, on `pub(crate) fn cancel_request_for_test`** (corrected in round 4; round 3 cited only the `:716` one). It is a working example of the per-item shape step 1 now describes, it precedes all three hits, and it changes nothing: all three sit inside the `:717` module either way, so the file stays out of both §6.1 lists. `ac-dev-rust-v3` found it in round 3.
- The per-file distribution printed below is round 2's, computed under the old reading, and is **superseded by the totals above**. It is retained because §6.2 and the round-2 reviews are anchored to it, and because nothing in the change depends on it: treat it as indicative and the sweep in §9.4 AC1 as binding.
- §3.14 is a **narrowing aid, not a gate**. AC1 sweeps raw lines with no narrowing at all, §5.10 owns every multi-line-literal edit by name, and §6.1 names every file. Nothing in the change depends on the 544-versus-628 figure; what depended on it was a reviewer's ability to use §3.14 as a completeness check, and that is what the correction restores.

#### Per-file distribution as round 2 computed it (544 lines, 55 files), superseded by the totals above and retained for anchoring

`commands/entity_creation.rs` 77; `phone/mailbox.rs` 40; `config/teams.rs` 39; `commands/ac_discovery.rs` 31; `cli/list_peers.rs` 29; `config/session_context.rs` 26; `commands/task.rs` 25; `pty/terminal_snapshot/acceptance_tests.rs` 24; `cli/role_experiment.rs` 22; `config/replica_identity.rs` 22; `config/coordinator_clocks.rs` 21; `cli/workgroup.rs` 20; `commands/wg_delete_diagnostic.rs` 12; `cli/team.rs` 11; `session/context_alerts.rs` 11; `cli/send.rs` 10; `config/coding_agent_profiles.rs` 8; `phone/messaging.rs` 8; `cli/mod.rs` 6; `cli/task_set_title.rs` 6; `commands/config.rs` 6; `config/ac_root.rs` 6; `config/loops.rs` 6; `screenshot/windows.rs` 6; `cli/purge_wg.rs` 5; `resource_monitor/types.rs` 5; `cli/task_append_body.rs` 4; `config/injected_messages.rs` 4; `phone/types.rs` 4; `api/actuation.rs` 3; `cli/close_session.rs` 3; `config/placeholders.rs` 3; `config/project_settings.rs` 3; `config/root_agent.rs` 3; `config/seed_manifest.rs` 3; `config/settings.rs` 3; `session/manager.rs` 3; `api/README.md` 2; `api/schema.rs` 2; `commands/pty.rs` 2; `config/seeded_context_templates.rs` 2; `loops/delivery.rs` 2; `pty/container_paths.rs` 2; `pty/terminal_snapshot/resource_tests.rs` 2; `session/session.rs` 2; `api/auth.rs` 1; `cli/task_ops.rs` 1; `commands/session.rs` 1; `config/sessions_persistence.rs` 1; `loops/non_stop_watchdog.rs` 1; `pty/container_repos.rs` 1; `pty/git_watcher.rs` 1; `screenshot/mod.rs` 1; `session/purge_guard.rs` 1; `web/commands.rs` 1.

#### Reader-facing string sites in files round 1's §6.1 did not list, enumerated

Every one of these is Rule R and none was matched by any round-1 needle.

| Site | String | Surface |
| --- | --- | --- |
| `loops/non_stop_watchdog.rs:342` | `"\u{26A0} Alert me! [{}]: {} {}/{} workgroups working. Not working: {}. Persisted >{}s."` | the Telegram alert the owner reads |
| `phone/types.rs:192` | `"An exact canonical workgroup target is required."` | API refusal text, crosses the HTTP boundary |
| `phone/types.rs:203` | `"The sender is not the verified workgroup orchestrator."` | same |
| `phone/types.rs:205` | `"The target is not a verified member of the sender workgroup."` | same |
| `phone/types.rs:224` | `"Workgroup purge temporarily blocks target preparation."` | same |
| `api/actuation.rs:52` | `"notification exceeds PTY-safe length; shorten the slug or use a shallower workgroup path"` | API error |
| `api/actuation.rs:95` | `format!("bound replica is not under a workgroup: {}", e)` | API error |
| `commands/pty.rs:46`, `:66` | `"purge-wg in progress for this session; input rejected"` | GUI-surfaced error naming the CLI command |
| `web/commands.rs:420` | same string | same |
| `loops/delivery.rs:69` | `"purge-wg in progress for '{}'; loop delivery skipped"` | operational log the owner reads |
| `session/context_alerts.rs:1399` | `"sampled CWD is not inside a lexical workgroup member replica"` | alert-path diagnostic returned to the caller |
| `session/context_alerts.rs:1404` | `"sampled lexical replica has no workgroup parent"` | same |
| `session/context_alerts.rs:1409` | `"sampled lexical workgroup has no Project AC Root parent"` | same |
| `session/context_alerts.rs:1414` | `(lexical_workgroup, "workgroup")` — the label interpolated into the message | same |
| `session/context_alerts.rs:1467` | `"sampled CWD is not inside a workgroup member replica"` | same |
| `session/context_alerts.rs:1488` | `"sampled replica does not have the canonical workgroup layout"` | same |
| `session/context_alerts.rs:1493`, `:1674` | `canonical_real_directory(..., "Workgroup")` — the label interpolated into the message | same |
| `session/context_alerts.rs:1618` | `"Orchestrator replica escapes the sampled workgroup"` | same |
| `session/context_alerts.rs:1634` | `"Resolved orchestrator identity does not match the exact sampled workgroup"` | same |
| `session/context_alerts.rs:1665` | `format!("purge-wg blocks '{}'", target.fqn())` | same |
| `config/seed_manifest.rs:1339` | `"config scope must be config:.ac/<workgroup>/<replica>/<dest>: {scope}"` | seed-manifest validation error |
| `config/seed_manifest.rs:3447` | `"lifecycle config prefix must include a workgroup component"` | same |
| `config/injected_messages.rs:78` | `#   %WORKGROUP%   workgroup name, e.g. wg-2-dev-team` | prose written into the user's `injected-messages.toml` (§3.7 family 5); the `%WORKGROUP%` token itself is Rule P |
| `commands/config.rs:1422` | `"Target replica has no workgroup parent"` | listed file, unenumerated line |
| `phone/messaging.rs:124` | `#[error("no workgroup ancestor found for '{0}'")]` | listed file, unenumerated line |

#### Sites in that sweep that are Rule P, with the clause that preserves each

| Site or class | Clause |
| --- | --- |
| `config/injected_messages.rs:46` `TOKEN_WORKGROUP = "%WORKGROUP%"`, and `:52` `DEFAULT_CONTEXT_ALERT_TEMPLATE` | P0. See §3.7 family 5 and the new §3.10 rows. |
| `config/project_settings.rs:170` `"At most {MAX_WORKGROUP_GROUPS} groups are allowed"` | P0. The only occurrence is the identifier inside the format placeholder; the rendered string carries no retired word. |
| `config/coordinator_clocks.rs:385`, `:440`, and every other `log::` macro body | **New clause P5** (§5.2). `log::` text is developer diagnostics keyed by a bracketed target tag, not a product surface. It is preserved so the diff stays inside the reader-facing surface, and because the tags (`[coordinator-clocks]`) are grep anchors operators already use. |
| `pty/terminal_snapshot/acceptance_tests.rs`, `pty/terminal_snapshot/resource_tests.rs` | P2. Test fixtures whose purpose is to represent a legacy Workgroup, so their production lines are Rule P and neither file takes a production edit. The two files differ and round 2 described only the first: `acceptance_tests.rs:74` is `const WORKGROUP: &str = "wg-1-dev-team"`, while `resource_tests.rs` has no such constant and carries two FQN literals instead, `:33` `"project:wg-1-team/coordinator"` and `:34` `"project:wg-1-team/member"`. All three stay as they are; §9.1 adds `room-` twins beside them, which is a `#[cfg(test)]` edit and is placed by §6.4, not a production-line edit. |
| `commands/wg_delete_diagnostic.rs` (12 doc-comment lines), `api/auth.rs:4`, `api/schema.rs:36`, `:41`, `cli/task_ops.rs:41`, `commands/session.rs:1099`, `config/settings.rs:80`, `:456`, `:506`, `config/sessions_persistence.rs:1081`, `pty/git_watcher.rs:612`, `screenshot/mod.rs:5`, `session/manager.rs:493`, `:518`, `:546`, `session/purge_guard.rs:5`, `resource_monitor/types.rs:131`, `:132`, `:139`, `:232`, `:233` | P1. Ordinary `///` and `//!` doc comments on non-`clap` items. They are not printed anywhere. Round 1 renamed the ones it happened to enumerate and left the rest; this plan preserves all of them uniformly, so the boundary is a rule rather than a list, and #1615 moves them with the identifiers they document. |

The one exception to that last row: a doc comment that **contradicts the code it documents after this change** is corrected, because a wrong comment is a defect. The closed set is the five in `commands/entity_creation.rs` that quote `starts_with("wg-")` or the `[3..]` slice as prose (`:3832`, `:6929`, `:6944`, `:7240`, `:7257`) and the `determine_next_wg_number` block at `:4280-4291`. Those are named in §5.3 and §5.5 and are the only doc comments in the Rust diff.

---

## 4. Scope

### In scope (binding)

1. The creation prefix and the Room slot allocator (§5.5).
2. Dual-prefix acceptance at the 40 Rust gate lines of §3.2 and the 6 frontend predicates of §3.3, through one shared helper on each side (§5.3, §5.4).
3. The `room-*/` parent-repository exclusion, in `ensure_ac_root_gitignore` and in the repository's own `.gitignore` (§5.6).
4. The CLI canonical names and deprecated aliases, and every `clap`-printed string (§5.7).
5. The message-filename short prefix (§5.8).
6. Three frozen seeded-template snapshots and three `current_version` bumps (§5.9).
7. Visible text in `src/`, in Rust user-facing and agent-facing strings, and in `docs/` plus the four root Markdown files (§5.11, §5.12).
8. Test coverage for the independent allocator, dual-prefix discovery, CLI alias equivalence, the mixed root, and the frozen recognizers (§9).

### Out of scope (binding)

1. **Every Rust and TypeScript identifier**: `wg_name`, `wg_dir`, `wg_number`, `determine_next_wg_number`, `collect_team_workgroup_dirs`, `parse_team_from_workgroup_name`, `is_wg_dir`, `parse_wg_prefix`, `extractWorkgroupName`, `wgNumber`, `wgTooltipLabel`, `hasWorkgroupContext`, `AcWorkgroup`, `WorkgroupGroup`, `PurgeWgArgs`, `WorkgroupArgs`, struct fields, enum variants, type parameters and local bindings. They keep their current spelling. #1615.
2. **Every source and documentation file name**: `cli/workgroup.rs`, `cli/purge_wg.rs`, `commands/wg_delete_diagnostic.rs`, `sidebar/stores/workgroup-groups.ts`, `sidebar/components/WorkgroupGroupRail.tsx`, `WorkgroupGroupsModal.tsx`, `NewWorkgroupModal.tsx`, `terminal/components/WorkgroupTask.tsx`, `sidebar/components/workgroup-session.ts`, `workgroup-delete-diagnostics.ts`, `docs/agents/teams-and-workgroups.md`, `docs/testing/04-team-and-workgroup-lifecycle.md`. #1615.
3. **Every persisted or wire value** in §3.10, including CSS class names, `data-ac-testid` values, event names, IPC command names, refresh reason codes, the outbox `action` value, Loop target kinds, profile scopes, collapse keys and the `%WORKGROUP%` injected-message placeholder token.
4. **Any migration**: no existing `wg-*` directory is renamed, moved, copied, converted, marked or deleted by this change; no config entry that names one is rewritten.
5. **Removing `wg-*` support or the deprecated CLI aliases.** #1615.
6. `repo-personal` and `repo-agentscommander_webpage`. The assignment measured 37 files / 954 lines and 13 files / 67 lines respectively. Neither is in this issue, and neither blocks it: no code in either repository parses an AgentsCommander entity directory name. Recorded as residual R7.
7. `plans/*.md`, `CHANGELOG.md` history entries, and every item in §5.2's Rule P3 table. Historical records and recognizer inputs are not rewritten.
8. Renaming the `.deleting-*` sentinel prefix or any other on-disk marker.

---

## 5. Decided solution

### 5.1 Rule R: the substitution, fixed and total

Rule R applies to an occurrence of `workgroup`, `Workgroup`, `WORKGROUP`, `workgroups`, `Workgroups`, `WG` or `wg` **only when the occurrence is a word in text that a human or an agent reads**. The substitution table is closed:

| Before | After |
| --- | --- |
| `workgroup` | `room` |
| `Workgroup` | `Room` |
| `WORKGROUP` | `ROOM` |
| `workgroups` | `rooms` |
| `Workgroups` | `Rooms` |
| `WG` as a standalone word | `Room` in prose, `ROOM` where the surrounding text is upper-case |
| `wg-*` / `wg-<N>-*` naming a directory shape in prose | `room-*` / `room-<N>-*`, and where the sentence is about what the product *accepts* rather than what it *creates*, `room-*` followed by the exact clause `` (legacy Workgroups keep their `wg-*` name and stay fully supported) `` |

The reader-facing carriers are exactly: JSX text, `title` / `aria-label` / `placeholder` / `alt` attributes, string literals rendered as labels, toasts, validation and error messages, `clap` `about` / `long_about` / `after_help` / `help` text and every `#[derive(Subcommand)]` or `#[derive(Args)]` doc comment `clap` compiles into printed help, `println!` / `eprintln!` / `cli_println!` prose, `Err(...)` and `format!` message prose, the seeded and generated context templates, the comments `ensure_ac_root_gitignore` writes into a user's `.gitignore`, `docs/**`, `README.md`, `ROADMAP.md`, `PRIVACY.md` and `src-tauri/src/api/README.md`.

**Article agreement does not move.** `workgroup` and `room` both begin with a consonant, and both `a WG` and `a Room` are correct, so Rule R introduces no `a`/`an` change anywhere. This is stated because the analogous #1571 rename did require an article sweep; here the sweep is provably empty and AC9 asserts it.

### 5.2 Rule P: where Rule R is not applied

Rule P is referential, not positional: the discriminator is whether something **resolves** the token, not where the token sits.

**P0.** An occurrence is preserved when code binds, reads, matches, parses or compares it: an identifier, a struct field, an enum variant, a file name, a module path, a CSS class, a `data-ac-testid`, a JSON or serde key, a persisted config value, an event name, an IPC command name, a reason code, a wire value, a template placeholder token, a test-id in `test-debt.allowlist.json`, or a directory name that exists on disk.

**P1.** A source comment or doc comment is preserved, **except** (a) a doc comment `clap` compiles into printed help, and (b) a comment that would contradict the code it documents after this change. The closed list of shapes where `clap` compiles a doc comment into printed help: a `#[derive(Subcommand)]` variant, a `#[derive(Args)]` or `#[derive(Parser)]` struct, a `#[derive(Args)]` field, and a `#[derive(ValueEnum)]` variant. This carve-out exists because #1571 shipped a round-7 blocker precisely here: five `///` lines that no enumeration named were printed help twice each. The closed list for (b) is in §3.14's last paragraph.

**P2.** A `wg-` literal inside a `#[cfg(test)]` module, a `*.test.ts(x)` file, a test fixture or `src/shared/testing/ui-harness.tsx` is preserved **when the fixture's purpose is to represent a legacy Workgroup**, which after this change is the only way dual-prefix acceptance can be tested at all. Existing fixtures therefore stay `wg-*` and §9 adds `room-*` twins beside them rather than converting them.

**P3 (extended in round 2).** A historical record is preserved: `plans/*.md`, existing `CHANGELOG.md` entries, and — this is the extension — **any constant, static, or function whose bytes are compared against, searched inside, or hashed against a file the user owns.** Round 1 wrote this clause as "every `*_BEFORE_*` frozen template constant", which is a naming convention, not a property. Two of the five recognizer families in §3.7 are outside that convention and round 1's §5.9 sent the implementer to edit both of them.

The closed P3 set at `d7008b34`, each with the consumer that makes it frozen:

| Frozen item | Site | Consumer that compares or searches it |
| --- | --- | --- |
| every `*_BEFORE_*` seeded-template constant | `seeded_context_templates.rs`, `root_agent.rs` | the `is_known_generated_*` recognizers (§3.7 family 1) |
| `OLD_ROOT_ROLE_MD` | `root_agent.rs:308-338` | `old_generated[0]` at `:732`, and the pristine list at `:1045` |
| `OLD_ROOT_CONTEXT_WITH_COORDINATION_MD` | `root_agent.rs:340-374` | `old_generated[1]` at `:733` |
| `ROOT_COORDINATION_MESSAGING_PARAGRAPH` | `root_agent.rs:292-306` | interpolated into the entry above at `:371` — **dual-use, split in §5.10** |
| `OLD_DEFERRED_MESSAGING_PARAGRAPH` | `root_agent.rs:290` | `existing.contains(...)` against the user's live `Role.md` at `:1058` |
| `legacy_rendered_default_context_for_generation`, **the whole function, lines 3769-4025** | `session_context.rs` | `classify_legacy_rendered_default_context` (§3.7 family 3) |
| `LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072`, `LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072` | `session_context.rs:3735-3736` | read at `:3862` and `:3864` inside the function above |
| `DIRECT_MATRIX_GIT_SCOPE` | `session_context.rs:3369` | read at `:3861` inside the function above. It carries no occurrence of the retired token, so Rule R would not move it anyway; it is listed so the closure is complete rather than sampled. |
| `WORKGROUP_GIT_SCOPE` | `session_context.rs:3368` | read at `:3860` inside the function above — **dual-use, split in §5.10** |
| `TOKEN_WORKGROUP`, `DEFAULT_CONTEXT_ALERT_TEMPLATE` | `injected_messages.rs:46`, `:52` | `expand_tokens`, and the `known_default_sha256` hash compare at `:975-980` (§3.7 family 5) |
| `MESSAGING_DIR_NAME` | `phone/messaging.rs:11` | read inside `legacy_rendered_default_context_for_generation`. Its value is `"messaging"`, so Rule R would not move it anyway; it is listed because §3.7 family 3's closure names it and this table's standard is completeness, not sampling. Round 2 omitted it while listing `DIRECT_MATRIX_GIT_SCOPE` for exactly that reason |
| `render_skills_section`'s replica line, **the literal only, not the function** | `session_context.rs:831` | `is_provably_generated_legacy_skills_section` (`:4183`) recomputes the live `render_skills_section` and compares it against the skills section extracted from the user's file. **Dual-use, split in §5.10 D8f** |

**This table has 12 rows.** The count is stated because round 2's cover note described it as 9 when it held 10, and because two rows are added here; a reviewer checking the closure against §3.7 should find the two enumerations agree as sets, not merely in length.

**Freezing a function is a stronger obligation than freezing a constant**, and it is stated so the implementer does not under-apply it: the frozen unit is the function's **transitive literal closure**, meaning the body plus every constant it reads. Round 3 makes that obligation wider still, because round 2 applied it to one function when the protected thing is a chain: **the frozen unit for a recognizer is every byte the classification decision reads, across every function the decision path enters.** For family 3 that closure is enumerated item by item in §3.7 and pinned by the digests in §3.12 Table A. The one item round 2's narrower reading missed, `render_skills_section:831`, is the reason this clause is restated rather than left as it was.

**P4 (new).** `log::trace!` / `debug!` / `info!` / `warn!` / `error!` message text is preserved. It is developer diagnostics keyed by a bracketed target tag operators grep for, not a product surface, and it is not covered by requirement (G)'s carriers. The boundary is exact: if the same string is also returned to a caller or rendered, the returning or rendering site takes Rule R and the log line does not. No such shared string exists at `d7008b34`; the check is `git grep` over the §3.14 inventory and it came back empty.

**R1 (a clause of Rule P, not an exception to it).** When the only thing that resolves an occurrence is **another occurrence of the same literal inside this change's own edit set**, and both sides move in the same commit, the occurrence is renamed. The one instance is `ProjectPanel.tsx:993`/`:995` against `:1057`/`:1058`/`:1071` (§3.8). A test expectation is never itself a resolver: an occurrence pinned only by an assertion is classified by what the pinned literal names, and the assertion moves with it under §9.3.

**Where an enumeration in this plan and Rule P disagree, Rule P governs and the enumeration is the defect to report.** Round 1 proved that clause is load-bearing: §5.9's enumeration named four lines inside a P3 function and one inside a P3 constant, and the tie-break is what a reviewer used to catch it.

### 5.3 D1: one shared Rust helper, and the 40 lines it replaces

**Decision.** A single shared helper, not per-site edits. Forty sites expressing one predicate is exactly the shape that drifts, and two of them (`entity_creation.rs:2965`, `:3489`) already encode `"wg-".len()` as the literal `3`, which a per-site edit reproduces silently for `room-`.

**New file `src-tauri/src/config/entity_prefix.rs`**, declared in `src-tauri/src/config/mod.rs` as `pub mod entity_prefix;` inserted alphabetically between `pub mod daemon_pid;` (line 13) and `pub mod injected_messages;` (line 14). The module imports nothing from the crate and nothing beyond `core`, which is what makes §11's cycle proof structural rather than statistical.

```rust
//! On-disk prefix of a Room directory, and of the legacy Workgroup directory
//! it replaces (#1614). Phase 2 (#1615) retires the legacy prefix; until then
//! every discovery, identity and authorization gate accepts both.

/// Prefix of every Room directory AgentsCommander creates.
pub const ROOM_DIR_PREFIX: &str = "room-";

/// Prefix of a legacy Workgroup directory. Never produced again (#1614); still
/// discovered, addressed and operated exactly like a Room.
pub const LEGACY_WORKGROUP_DIR_PREFIX: &str = "wg-";

/// The matched prefix, or `None` when `name` is neither.
pub fn entity_prefix_of(name: &str) -> Option<&'static str> {
    if name.starts_with(ROOM_DIR_PREFIX) {
        Some(ROOM_DIR_PREFIX)
    } else if name.starts_with(LEGACY_WORKGROUP_DIR_PREFIX) {
        Some(LEGACY_WORKGROUP_DIR_PREFIX)
    } else {
        None
    }
}

/// `name` with its Room or legacy Workgroup prefix removed.
pub fn strip_entity_prefix(name: &str) -> Option<&str> {
    name.strip_prefix(ROOM_DIR_PREFIX)
        .or_else(|| name.strip_prefix(LEGACY_WORKGROUP_DIR_PREFIX))
}

/// True when `name` carries a Room or legacy Workgroup prefix.
pub fn has_entity_prefix(name: &str) -> bool {
    entity_prefix_of(name).is_some()
}
```

The two prefixes are disjoint, so evaluation order is behaviorally irrelevant; it is fixed anyway so the function is deterministic under review.

**Mechanical mapping, binding.**

- Each of the 30 `starts_with("wg-")` lines P1 to P30 becomes `crate::config::entity_prefix::has_entity_prefix(<same expression>)`. Where the site is written as `.map(|name| name.starts_with("wg-"))` or `.is_some_and(|n| n.starts_with("wg-"))`, the closure body is the only thing that changes.
- Each of the 9 `strip_prefix("wg-")` lines S1, S2, S3, S5, S6, S7, S8, S9 and S10 becomes `crate::config::entity_prefix::strip_entity_prefix(<same expression>)`. **S4 (`entity_creation.rs:4301`) is the single exception and is covered by §5.5: it must stay Room-only.**
- `entity_creation.rs:2964-2966` and `:3488-3490` are rewritten so the middle segment comes from the stripped remainder, never from an index:

```rust
if let Some(rest) = crate::config::entity_prefix::strip_entity_prefix(&name_str) {
    if let Some(middle) = rest.strip_suffix(&wg_suffix) {
        if middle.parse::<u32>().is_ok() {
            wg_dirs.push(entry.path());
        }
    }
}
```

  This is behavior-preserving for `wg-`: `rest.strip_suffix(&wg_suffix)` yields exactly what `&name_str[3..name_str.len() - wg_suffix.len()]` yielded, and it additionally cannot panic when the suffix is longer than the remainder, which the index form could.
- `phone/messaging.rs:373` (`is_wg_dir`) keeps its digit validation and swaps only the strip; `:386` (`parse_wg_prefix`) is covered by §5.9 because its return value is visible.
- Every user-visible error string co-located with one of these gates is rewritten under Rule R using one canonical phrase: **`` a `room-*` or legacy `wg-*` Room directory ``**. The affected messages are `commands/config.rs:1520` and `:1528-1530`, `commands/task.rs:106-109`, `config/coding_agent_profiles.rs:236-239`, `config/loops.rs:445`, `config/replica_identity.rs:239-242` and `:246-249`, and `pty/container_paths.rs:290-293`.
- The five doc comments and one test assertion that quote `starts_with("wg-")` as prose (`entity_creation.rs:3832`, `:6929`, `:6944`, `:7240`, `:7257`) are updated to describe the new predicate. They are Rule P1 comments, but a comment that contradicts the code it documents is a defect, so they are corrected as part of the same edit; `:6944` is a live assertion and moves under §9.3.

### 5.4 D2: one shared frontend helper, and the 6 predicates it replaces

**New file `src/shared/entity-prefix.ts`**, importing nothing. `src/shared/constants.ts` was rejected as the home because it evaluates `window.location.search` at module load, and `profile-utils.ts` is exercised in Node-side vitest where that side effect has no business appearing.

```ts
/** On-disk prefix of a Room directory (#1614). */
export const ROOM_DIR_PREFIX = "room-";
/** On-disk prefix of a legacy Workgroup directory. Never produced again; still supported. */
export const LEGACY_WORKGROUP_DIR_PREFIX = "wg-";

/** Case-sensitive: does this directory name carry a Room or legacy Workgroup prefix? */
export function isEntityDirName(name: string): boolean {
  return /^(?:room|wg)-/.test(name);
}

/** Case-sensitive: prefix followed by at least one digit. */
export function isNumberedEntityDirName(name: string): boolean {
  return /^(?:room|wg)-\d+/.test(name);
}

/** Case-insensitive slot number, or null. */
export function entityDirNumber(name: string): number | null {
  const m = name.match(/^(?:room|wg)-(\d+)/i);
  return m ? Number.parseInt(m[1], 10) : null;
}

/** Case-insensitive short label: "ROOM1" for a Room, "WG1" for a legacy Workgroup. */
export function entityShortLabel(name: string): string | null {
  const m = name.match(/^(room|wg)-(\d+)/i);
  return m ? `${m[1].toUpperCase()}${m[2]}` : null;
}

/** Case-sensitive: does this path contain a Room or legacy Workgroup directory segment? */
export function pathHasEntityDirSegment(path: string): boolean {
  return /[\/\\](?:room|wg)-/.test(path);
}
```

Five functions rather than one, because each preserves its call site's exact case sensitivity, and that is deliberate: `WorkgroupTask.tsx:70` documents that its gate must stay case-sensitive or the buttons enable for clicks that always fail, while the rail's two predicates are display-only and have always been case-insensitive. Collapsing them would be a silent behavior change.

**Mechanical mapping, binding.**

| Site | Before | After |
| --- | --- | --- |
| F1 `shared/path-extractors.ts:25` | `return /^wg-\d+/.test(wg) ? wg.toUpperCase() : null;` | `return isNumberedEntityDirName(wg) ? wg.toUpperCase() : null;` |
| F2 `shared/profile-utils.ts:124` | `if (!/^__agent_/.test(leaf) \|\| !/^wg-/.test(parent)) return null;` | `... \|\| !isEntityDirName(parent)) return null;` |
| F3 `shared/profile-utils.ts:472` | `... && /^wg-/.test(parent);` | `... && isEntityDirName(parent);` |
| F4 `WorkgroupGroupRail.tsx:66-69` | `wgNumber` body | `return entityDirNumber(name) ?? Number.MAX_SAFE_INTEGER;` |
| F5 `WorkgroupGroupRail.tsx:71-74` | `wgTooltipLabel` body | `return entityShortLabel(wgName) ?? wgName;` |
| F6 `WorkgroupTask.tsx:73-75` | `return /[\/\\]wg-/.test(cwd);` | `return pathHasEntityDirSegment(cwd);` |

F1 and F5 also settle the visible-label question the assignment raises. **The badge and the short label are derived from the actual directory name, so a legacy Workgroup still reads `WG-1-TEAM` / `WG1` and a Room reads `ROOM-1-TEAM` / `ROOM1`.** Relabelling every entity `ROOM` uniformly was rejected: in the mixed root the assignment explicitly asks about, `wg-1-team` and `room-1-team` are two different directories that would then be indistinguishable in the titlebar badge and in the rail tooltip. The badge is an identity, not a concept noun; the concept noun is what Rule R renames.

### 5.5 D3: creation, and the independent Room slot namespace

`entity_creation.rs:1180` and `:2844` become:

```rust
let wg_name = format!("{}{}-{}", crate::config::entity_prefix::ROOM_DIR_PREFIX, wg_number, safe_team);
```

The local binding keeps the name `wg_name` (identifiers are out of scope). The `wg_dir.exists()` guard immediately below each is unchanged; its message is renamed under Rule R to `Room directory already exists: {}`.

`determine_next_wg_number()` (`entity_creation.rs:4291`) keeps its name, its signature, its lowest-free-positive-slot semantics and its `read_dir`-failure degradation to `1`. Only its scan changes: `name_str.strip_prefix("wg-")` at `:4301` becomes `name_str.strip_prefix(crate::config::entity_prefix::ROOM_DIR_PREFIX)`. It therefore considers only `room-<n>-<team>` directories, which is exactly decision 4: in a root holding `wg-1-<team>`, `taken` is empty and the allocation is `1`, producing `room-1-<team>`.

Its doc comment block (`:4280-4291`) is rewritten to describe the Room namespace and to say, in one sentence, that legacy `wg-*` directories are deliberately not counted because the two namespaces are independent. The three doc comments at `:7203`, `:7240` and `:7257` that reason about `[3..]` slices and the `starts_with("wg-")` filter move with the code they describe.

**Consequence, accepted and recorded as residual R1.** A root that already holds `wg-1-team` and gains `room-1-team` now has two entities whose slot number is `1`. They are distinguished everywhere by the full directory name, which is what every identity path already uses (§5.11). Nothing keys on the number alone.

### 5.6 D4: parent-repository exclusion for `room-*`

`ensure_ac_root_gitignore` (`commands/ac_discovery.rs:1506`) gains a `room-*/` entry as the **first** element of `required_entries`, immediately before the existing `wg-*/` entry, which stays:

```rust
(
    "room-*/",
    "# AgentsCommander: exclude room cloned repos from parent git tracking.\n# Without this, parent repo operations (checkout, reset) corrupt child clones.",
),
(
    "wg-*/",
    "# AgentsCommander: exclude legacy workgroup cloned repos from parent git tracking.\n# Without this, parent repo operations (checkout, reset) corrupt child clones.",
),
```

Because the presence test at `:1579` compares only the **pattern** line, an existing `.ac/.gitignore` gains the `room-*/` block on the next call and keeps whatever comment its `wg-*/` line already has. New roots get both new comments. That asymmetry is intended and is recorded as residual R2; rewriting existing comments would mean editing a user-owned file for cosmetics.

The repository's own root `.gitignore` gains `.ac/room-*/` beside `.ac/wg-*/` at line 18, under a comment renamed by Rule R.

`.deleting-*/`'s comment at `:1513` (`# AgentsCommander: exclude temporary workgroup delete sentinels/orphans.`) is written into user files and is therefore visible text: Rule R renames it. Same asymmetry, same residual.

### 5.7 D5: `role-experiment`

`cli/role_experiment.rs` is a `#[command(hide = true)]` developer tool whose run artifacts are disposable. Acceptance criterion (A) is unqualified ("No code path produces a `wg-*` directory"), and leaving one production `format!("wg-` behind would make AC2 unstatable as a grep. So:

- `:2209` becomes `format!("{}{}-role-exp-{}", ROOM_DIR_PREFIX, next, sanitized_experiment)`.
- `:2298` (`next_workgroup_number`, `max + 1`) becomes `strip_entity_prefix`, so it scans **both** prefixes. This is strictly safer than scanning one: a max-based allocator that ignored existing `wg-N-role-exp-*` names could collide with one. It is not the product allocator and decision 4 does not govern it; §10 records that as decision D6.
- `:2328`, `:3185`, `:3204`, `:3224` become `has_entity_prefix`. `:3204`'s `!name.contains("-role-exp-")` is untouched.
- `--retain-workgroup` (`:95`) becomes `#[arg(long = "retain-room", alias = "retain-workgroup", value_name = "RETAIN_ROOM", value_parser = BoolishValueParser::new())]`. **The `value_name` is not optional and round 1's table omitted it.** The field is `retain_workgroup: Option<bool>` (`:96`) with a `BoolishValueParser`, so it takes a value, and without `value_name` clap derives the placeholder from the field and prints `--retain-room <RETAIN_WORKGROUP>`. The command is `hide = true`, which hides it from listings but not from parsing or from its own `--help`; AC5 reaches it because `Command::get_subcommands` returns hidden subcommands (§9.4).

### 5.8 D6: the CLI canonical names and deprecated aliases

**The declaration form, settled empirically, not assumed.** A probe crate built offline against `clap` (probe resolved 4.6.6; the repository's `Cargo.lock` pins `clap 4.6.0`, same 4.x alias contract) established all five facts below.

1. `#[command(name = "purge-room", alias = "purge-wg")]` accepts **both** subcommand spellings and dispatches to the identical variant. Verified: `purge-wg --wg X` and `purge-room --room X` both produced `OK wg=Some("X")`.
2. `#[arg(long = "room", alias = "wg")]` on the existing field `wg` accepts **both** flag spellings and binds the identical field. Verified: all four combinations `{purge-wg, purge-room} x {--wg, --room}` parsed to `Some("X")`.
3. `alias` is **hidden** from help, `visible_alias` would not be. Help printed only `--room`. Hidden is the decided form: the old names are deprecated, and advertising them in help would contradict requirement (G).
4. Help and error output always render the **canonical** name, never the invoked alias: `purge-wg --help` printed `Usage: acprobe purge-room [OPTIONS]`, and `purge-wg --bogus` printed `error: unexpected argument '--bogus' found` above the same canonical usage line.
5. **The value placeholder is derived from the field name, not the long name.** Without a `value_name`, the probe printed `--room <WG>`, leaking the retired token into help.

Fact 4 is the precise reading of "behaviourally identical" this plan adopts, and it is stated so a reviewer does not report it as a defect: **identical means same parse result, same side effects, same exit code and same operational stdout/stderr; help and usage render the canonical name, which is how a deprecated alias steers a caller to the new one.**

Fact 5 makes D11 unqualified: **every renamed flag that takes a value carries an explicit `value_name`.** Round 1's own table then failed to apply it once, on the last row. That is corrected below.

#### The complete declaration set, binding

| Site | New declaration |
| --- | --- |
| `cli/mod.rs:161` | `#[command(name = "purge-room", alias = "purge-wg")]` |
| `cli/mod.rs:174` | `#[command(name = "room", alias = "workgroup")]` on `Workgroup(workgroup::WorkgroupArgs)` |
| `cli/purge_wg.rs:95-96` | `#[arg(long = "room", alias = "wg", value_name = "ROOM")] pub wg: Option<String>` |
| `cli/workgroup.rs:64-65` | `#[arg(long = "room", alias = "workgroup", value_name = "ROOM")] workgroup: String` |
| `cli/loop_cmd.rs:76-77` | same shape |
| `cli/loop_cmd.rs:98-99` | same shape |
| `cli/team.rs:39-40` | same shape |
| `cli/team.rs:80-81` | same shape |
| `cli/team.rs:92-93` | same shape |
| `cli/role_experiment.rs:95` | `#[arg(long = "retain-room", alias = "retain-workgroup", value_name = "RETAIN_ROOM", value_parser = BoolishValueParser::new())]` |

**The `role_experiment.rs:95` row is corrected in round 2.** The base declaration is `#[arg(long = "retain-workgroup", value_parser = BoolishValueParser::new())]` over the field `retain_workgroup: Option<bool>` (`:96`). It takes a value, so without `value_name` clap prints `--retain-room <RETAIN_WORKGROUP>` and the retired token ships in help. `RETAIN_ROOM` is the decided placeholder. The command is `#[command(hide = true)]`, which hides it from listings but not from parsing or from its own `--help`, and AC5 now reaches it by construction.

#### Every `clap`-printed string, corrected

Renamed under Rule R: the five `Commands` doc comments (`cli/mod.rs:160`, `:163`, `:165`, `:173`, `:175`), the three `WorkgroupCommand` doc comments (`cli/workgroup.rs:26`, `:28`, `:30`), the two `TeamCommand` doc comments (`cli/team.rs:29`, `:31`), the `purge_wg.rs:77-85` `after_help` block (four occurrences at `:78`, `:79`, `:80`, `:81`), the field docs at `purge_wg.rs:93-94`, `close_session.rs:135-137` and `send.rs:53`, the `after_help` / `long_about` bodies in `list_peers.rs`, `mod.rs`, `self_switch.rs` (including `:47`, `SCOPE: WG replicas only (__agent_* under a wg-* workgroup)`, which sits mid-string on a line carrying no quote character and is therefore invisible to a line-shaped sweep — see §3.14), `send.rs`, `task_append_body.rs` and `task_set_title.rs`, and **three** `help = ...` strings on `TeamCreateArgs`:

| Site | Base text |
| --- | --- |
| `cli/team.rs:61` | `"Define a repo available to the team when workgroups are created. Repeat for multiple repos"` |
| `cli/team.rs:66` | `"Define team repo access for workgroup creation as URL=agent-a,agent-b"` |
| `cli/team.rs:71` | `"Define team repo access for workgroup creation as URL=excluded-agent-a,excluded-agent-b"` |

**`team.rs:61` is new in round 2.** Round 1's §3.5 and §5.8 both said "two", and round 1's AC1 needle `for workgroup creation` catches `:66` and `:71` and structurally cannot catch `:61`, whose wording is `when workgroups are created`. `team create` was also absent from AC5's enumerated subcommand list, and it is the only command whose `--help` prints `:61`. Two independent gaps aligned on one line, which is why AC5 is rebuilt by construction in §9.4 rather than re-enumerated.

`cli/workgroup.rs`'s operational prose is renamed: `:187`, `:194`, `:197`, `:269` (the `validate_existing_name(&args.workgroup, "Workgroup")` label becomes `"Room"`), `:284`, `:330` (`Removed room {}`), `:346`, `:350` (`"Failed to fully delete workgroup directory; renamed workgroup to orphan '{}', ..."`), `:356`. Its log-target tags `[workgroup-remove]` (`:313`, `:319`) are Rule P4, and its refresh reason codes `"workgroupCreated"` / `"workgroupRemoved"` and JSON key `"workgroup"` are Rule P0.

**Every enumeration in §5.8 is illustrative, not closed, and Rule R is the gate.** §3.14 counts 20 candidate lines in `cli/workgroup.rs` against the nine named above, so the list names the ones a reviewer is most likely to want anchored and does not bound the edit. `commands/entity_creation.rs:3323` (`"Partial workgroup delete: renamed '{}' to orphan '{}', ..."`) is the same shape in a file §5.8 does not enumerate at all. Both are covered three times over, by Rule R, by §6.1's edited list and by AC1's un-narrowed sweep, so nothing ships wrong; what would ship wrong is a reviewer reading the list as complete. Both strings are additionally pinned by assertions that go red on a correct implementation, and both assertions are named in §9.3 clause 1.

The user-visible error text that names the command `purge-wg` is renamed to `purge-room` at `commands/pty.rs:46` and `:66`, `web/commands.rs:420`, `loops/delivery.rs:69` and `session/context_alerts.rs:1665` (§3.14). The wire value `PURGE_WG_ACTION = "purge-wg"` is untouched (§3.10, D13); these are prose that happens to quote a command name, and after this change the canonical command name is `purge-room`.

### 5.9 D7: the message-filename short prefix

`phone::messaging::parse_wg_prefix` (`:385`) produces the `wgN` token that appears in **every** inter-agent message filename, and `is_wg_dir` (`:372`) is what `workgroup_root()` walks up to find. Both must accept `room-`, and the short token must not say `wg` for a Room.

```rust
fn parse_wg_prefix(prefix: &str) -> Option<String> {
    let matched = crate::config::entity_prefix::entity_prefix_of(prefix)?;
    let rest = &prefix[matched.len()..];
    let n_end = rest.find('-')?;
    let digits = &rest[..n_end];
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("{}{}", matched.trim_end_matches('-'), digits))
}
```

`is_wg_dir` (`:372-383`) keeps its digit validation and swaps only the strip, becoming `strip_entity_prefix(name)`.

So a Room replica's short name is `room1-architect` and a legacy Workgroup replica's stays `wg1-architect`, unchanged. Legacy filenames are therefore byte-identical to today's and nothing in flight moves.

**`validate_filename_shape` needs no change, and that is measured, not assumed.** It requires each `-` separated segment after the timestamp to be non-empty and `[a-z0-9]+` (`messaging.rs:250-261`); `room1` satisfies it exactly as `wg1` does. The `to`-literal search at `:277-281` uses `rposition`, and `room1` contains no `to` segment.

#### The edit set for the pattern text, corrected

Round 1 named five `session_context.rs` lines and one `root_agent.rs` line here. **Four of the six were inside frozen recognizers** (§3.7 families 3 and 4) and are removed. What remains:

| Site | Status | New text |
| --- | --- | --- |
| `session_context.rs:3536` | **edit** — live renderer (`default_context_dynamic_values`, 3477-3677) | `` YYYYMMDD-HHMMSS-<roomN>-<you>-to-<roomN>-<peer>-<slug>.md `` followed by the exact clause `` (a replica in a legacy Workgroup uses `<wgN>`) `` |
| `session_context.rs:3545` | **edit** — live renderer, Root Agent variant | keeps its `root-to-` shape; only the `<wgN>` placeholder changes, same clause appended |
| `session_context.rs:3639` | **edit** — live renderer | same as `:3536` |
| `session_context.rs:3630` | **edit** — live renderer, Root Agent variant | same as `:3545` |
| `session_context.rs:3817` | **REMOVED from the edit set** | inside `legacy_rendered_default_context_for_generation` (3769-4025). Rule P3. Stays `<wgN>` byte for byte. |
| `session_context.rs:3826` | **REMOVED from the edit set** | same function. Rule P3. |
| `session_context.rs:3880` | **REMOVED from the edit set** | same function. Rule P3. Its `coordinator` spelling, which the live twin at `:3630` no longer has, is the proof it is a frozen older generation. |
| `session_context.rs:3893` | **REMOVED from the edit set** | same function. Rule P3. |
| `config/root_agent.rs:297` | **REMOVED from the edit set** | line 6 of `ROOT_COORDINATION_MESSAGING_PARAGRAPH` (`:292-306`), which feeds the frozen recognizer entry at `:371`/`:733`. Handled by the split in §5.10, not by an in-place edit here. |
| `session_context.rs:3531` | **edit** (new in round 3) | live renderer, the section heading `**Narrow exception — workgroup messaging directory:**` becomes `**Narrow exception — room messaging directory:**`. **The separator is U+2014 EM DASH, not an ASCII hyphen**, in the source and in both cells here; round 3 transcribed it as `-` and round 4 corrects it. Rule R changes `workgroup` to `room` and nothing else on the line, so the em-dash must be copied through unchanged. It is pinned by the live assertion at `:4793`, which carries the same em-dash and moves with it under §9.3 clause 1 |
| `session_context.rs:3639`, the `<workgroup-root>` placeholder | **edit** (named explicitly in round 3) | the same line already appears above for its `<wgN>` occurrences; it additionally carries `` `<workgroup-root>/messaging/` ``, which becomes `` `<room-root>/messaging/` ``, and the walk-up sentence covered by the paragraph below |
| `docs/agents/inter-agent-messaging.md:26` | **edit** (new in round 3) | same as `:3536`. Invisible to round 2's AC1 docs command, see §9.4 AC1 |
| `docs/concepts.md:81` | **edit** (new in round 3) | same as `:3536`. Invisible to round 2's AC1 docs command |
| `docs/agent-matrix-conventions.md:548` | **edit** | same as `:3536` |

**Round 2's table was the pattern text only.** `:3531` and `:3639`'s `<workgroup-root>` placeholder are live-renderer prose that Rule R also moves, and naming them costs one row each and removes exactly the ambiguity that produced round 1's G1. The two documentation rows are new because round 2's AC1 docs sweep could not see them (§9.4 AC1, H4).

The walk-up sentence `` walk up from your root to the parent `wg-<N>-*` folder `` becomes `` the parent `room-<N>-*` folder (or `wg-<N>-*` in a legacy Workgroup) `` **in the live renderer only**; the same sentence inside 3769-4025 is frozen.

**§5.9 and §5.10 no longer overlap.** §5.9 owns `phone/messaging.rs` and the live-renderer ranges of `config/session_context.rs`. §5.10 owns every frozen snapshot and every split, in both `config/session_context.rs` and `config/root_agent.rs`. **No line of `config/root_agent.rs` is edited by §5.9.** Round 1 had §5.9 naming `:297` while §5.10 scoped root edits to six lines in `ROOT_ROLE_MD`; that contradiction is resolved in favour of §5.10, which is the section that owns freezing.

The two live assertions that pin the renderer strings, `session_context.rs:5282` (`YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md`) and `:5490` (`YYYYMMDD-HHMMSS-root-to-<wgN>-<orchestrator>-<slug>.md`), move under §9.3. Neither asserts anything about the frozen function.

### 5.10 D8: frozen snapshots, dual-use splits, and the version bumps

This section owns every freeze and every split, in `config/session_context.rs`, `config/root_agent.rs` and `config/seeded_context_templates.rs`. No other section edits a frozen item.

#### D8a. Three new seeded-template snapshots and three version bumps

Exactly as #1571 did for the orchestrator rename, and for the same recognizer mechanism (§3.7 family 1).

| New constant | File | Value | Wired into | Version bump |
| --- | --- | --- | --- | --- |
| `GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME` | `config/seeded_context_templates.rs` | verbatim copy of `session_context.rs` blob lines 2470-2492 at `d7008b34` | `is_known_generated_global_template` **and** `is_known_generated_standalone_global_template` | `global` spec `current_version` 4 -> 5 |
| `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME` | `config/seeded_context_templates.rs` | verbatim copy of `session_context.rs` blob lines 2509-2529 | `is_known_generated_coordinator_template` | `coordinator` spec `current_version` 5 -> 6 |
| `ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD` | `config/root_agent.rs` | verbatim copy of `root_agent.rs` blob lines 675-723 | **two** lists: `is_known_generated_root_context_template`'s `old_generated` array (`:731-740`) **and** `migrate_root_role_file`'s pristine-generation list (`:1045-1051`) | `rootAgent` spec `current_version` 7 -> 8 |

**The root snapshot's second wiring is new in round 2 and is not optional.** §3.7 family 2 shows `migrate_root_role_file` carries an independent list that includes `ROOT_ROLE_MD` itself at `:1051`. `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD` is in both lists (`:738` and `:1050`) and the repository ships a test that exists solely to catch a one-sided wiring: `frozen_v5_root_context_is_recognized_and_migrated_on_both_paths` (`:2436-2441`), whose doc comment says "a list edited in only one place cannot pass silently". Round 1 named only `old_generated`. Insert `ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD` immediately after `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD` in each list, preserving newest-last order.

"Verbatim copy" means: take the literal body from `git cat-file blob d7008b34:<path>`, change only the constant's name and add its doc comment. Do not retype it, do not reflow it, do not let an editor touch its trailing whitespace. The declaration-range digests in §3.12 Table A are what a reviewer checks the copy against, and the rendered-value digests in Table B are what the tests assert.

#### D8b. The `WORKGROUP_GIT_SCOPE` split

`WORKGROUP_GIT_SCOPE` (`session_context.rs:3368`) is read by the **live** renderer at `:3612` and by the **frozen legacy** recognizer at `:3860`. Rule R forces the first to change; Rule P3 forces the second not to. The split is:

1. **New frozen constant**, declared immediately after `:3368`:

```rust
/// #1614: the exact `WORKGROUP_GIT_SCOPE` bytes shipped at d7008b34. Read only
/// by `legacy_rendered_default_context_for_generation`, which reconstructs a
/// pre-#1369 `Context.AgentsCommander.md` for byte comparison. Never used for
/// current runtime output. 220 bytes, sha256 A386B52D...566D (plan section 3.12).
const WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME: &str = "<the d7008b34 bytes, verbatim>";
```

2. `session_context.rs:3860` becomes `(LegacyGitScopeGeneration::Current, true) => WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME,`. That is the **only** line of `3769-4025` this plan changes, it changes an identifier and not a byte of prose, and §3.12 Table A's `940FA357...` digest is therefore stated over the base range for the purpose of verifying the *prose*; AC7 states the one-identifier exception explicitly so the digest check is runnable.

3. `WORKGROUP_GIT_SCOPE` itself takes its Rule R substitution and keeps its name and its use at `:3612`.

**The one permitted change inside the frozen range does not reflow.** `cargo fmt --all -- --check` is a required CI job (§3.11), and the `:3860` identifier switch lengthens that line from 73 to 92 characters, which stays under rustfmt's 100-column default. So the substitution cannot trigger a reflow that moves other bytes and fails AC7.8. Stated because AC7.8 tolerates exactly one token of difference and a reflow would break it silently.

**The replacement text is fixed, and it is fixed because a test enforces a size budget round 1 did not name.** `git_scope_copy_is_location_correct_and_compact` (`session_context.rs:6060-6108`) asserts:

```
assert_eq!(workgroup_chars, 220);        // :6104
assert_eq!(workgroup_words, 33);         // :6105
assert!(workgroup_chars * 2 <= 473);     // :6106  => chars <= 236
assert!(workgroup_words * 2 <= 68);      // :6107  => words <= 34
```

The two `assert_eq!` values are updated (they pin the current constant, which Rule R renames, so §9.3 authorizes the edit). The two `assert!` ceilings are **not** updated: they are a compactness budget on injected context, not a pin on this rename, and moving them would be scope drift. That leaves a headroom of 16 characters and **one word**. Measured candidates:

| Candidate leading clause | chars | words | Verdict |
| --- | --- | --- | --- |
| `` `room-*/` rooms are gitignored; `` | 217 | 33 | fits, but is incomplete for a legacy replica, which is also gitignored |
| `` `room-*/` and `wg-*/` are gitignored; `` | **223** | **34** | **chosen.** Correct for both kinds of replica; sits exactly at the word ceiling |
| `` `room-*/` and legacy `wg-*/` are gitignored; `` | 230 | 35 | over the word ceiling |
| `` `room-*/` and `wg-*/` roots are gitignored; `` | 229 | 35 | over the word ceiling |

**Decided text**, the whole constant, byte for byte:

```
`room-*/` and `wg-*/` are gitignored; origin Agent Matrices are not and can be tracked. Git discovery above replica and Matrix roots is blocked. State-changing Git belongs in `repo-*`; read-only Git is allowed within scope.
```

223 characters, 223 UTF-8 bytes, 34 whitespace-separated words. `:6104` becomes `223` and `:6105` becomes `34`. `:6041`'s `assert!(WORKGROUP_GIT_SCOPE.contains("wg-*/"))` stays green as written and gains a companion `assert!(WORKGROUP_GIT_SCOPE.contains("room-*/"))`, because a `contains` on one prefix is exactly the tripwire that would pass a half-done split. A new `assert_eq!` pins `WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME.len()` at 220 and its sha256 at `A386B52D...566D` (§3.12 Table B).

**After the change the headroom is 13 characters and zero words**, not the 16 characters and one word stated above, which are the **pre-change** figures. The post-change numbers are the ones the next editor needs: `chars <= 236` against 223 leaves 13, and `words <= 34` against 34 leaves **zero**. Any later edit to this constant must re-run the budget, and any edit that adds a word turns `session_context.rs:6107` red at edit time. That is recorded as residual R10, which both round-2 reviewers explicitly accepted.

#### D8c. The `ROOT_COORDINATION_MESSAGING_PARAGRAPH` split

The constant (`root_agent.rs:292-306`) is read at `:371` (frozen recognizer input) and at `:1061` (written into the user's live `Role.md` by the deferred-messaging migration). Same shape as D8b:

1. **New frozen constant** `ROOT_COORDINATION_MESSAGING_PARAGRAPH_BEFORE_ROOM_RENAME`, a verbatim copy of `:292-306`'s literal body, pinned by §3.12 Table A `17D7303A...` (declaration, 956 bytes) and Table B `FC2164A2...` (rendered, 897 bytes).
2. `root_agent.rs:371`'s `format!` interpolation switches to the frozen copy, so `OLD_ROOT_CONTEXT_WITH_COORDINATION_MD` and therefore `old_generated[1]` are byte-identical to today.
3. `ROOT_COORDINATION_MESSAGING_PARAGRAPH` keeps its name, takes Rule R (line 6, the `root-to-<wgN>-<coordinator>` filename pattern, becomes `root-to-<roomN>-<orchestrator>`; `workgroup coordinator replicas` at line 1 becomes `room orchestrator replicas`), and keeps its use at `:1061`.

**Consequence for a user mid-migration, stated rather than left to the implementer.** A Root Agent whose `Role.md` still carries `OLD_DEFERRED_MESSAGING_PARAGRAPH` gets the renamed paragraph substituted in on the next run, so its file says Room. That file then matches no entry of either list — exactly as today, because the same was true after the pre-rename substitution — so it is left alone from then on. No installation loses an update it would otherwise have received, because the `:1058` branch is terminal by construction.

#### D8d. `OLD_DEFERRED_MESSAGING_PARAGRAPH` is frozen in place, no split

`root_agent.rs:290` is a `contains()` matcher against the user's live file (`:1058`) and is never rendered. It has no live twin, so there is nothing to split: it takes **no** edit. Rule P3. A new test pins its length at 293 and its sha256 at `6E12E68E...A463` (§3.12 Table B), because the failure mode of editing it is silent and permanent.

Note that its prose also appears verbatim at `root_agent.rs:337`, inside `OLD_ROOT_ROLE_MD` (`:308-338`), which is `old_generated[0]` and is in the `:1045` pristine list. `OLD_ROOT_ROLE_MD` is frozen for the same reason and is named in §5.2's P3 table; round 1's P3 wording, keyed on `*_BEFORE_*`, did not cover either constant.

#### D8e. The three current defaults then take their Rule R edits

**`config/session_context.rs` gains one editable region in round 3.** §5.10's closed set for that file now reads: `:222` and the two new constants declared beside it, and `:831` (D8f); `:2470-2492`, `:2509-2529` and `:2495` (D8a and `PTY_INPUT_COORDINATOR_CONTEXT`); `:3368` and the new frozen constant declared after it (D8b); `:3860`, the single identifier, and no other line of `3769-4025` (D8b); `:4183-4208`, the compare extension and its comment (D8f). The live-renderer ranges `:3477-3677` belong to §5.9, not to §5.10.

One line in `get_default_agent_template()`, one in `get_default_coordinator_template()`, and six in `ROOT_ROLE_MD` at `root_agent.rs:687`, `:698`, `:702`, `:704`, `:710` and `:712`. `:710` also renames the CLI it names, becoming `` 3. Activate a room with `room add` using only `--project`, `--team`, and `--title`. ``

`ROOT_ROLE_MD:675-723` is the **only** editable region of `config/root_agent.rs` outside the three items D8c and D8d name and the two recognizer-list insertions of D8a. Stated as a closed set so §5.9 and this section cannot drift apart again:

- `:290` — frozen, no edit (D8d).
- `:292-306` — Rule R, split (D8c).
- `:308-338`, `:340-374`, `:376-674` — frozen, no edit.
- `:675-723` — Rule R, six lines (D8e), plus the new snapshot copy declared beside it.
- `:731-740` and `:1045-1051` — one inserted line each (D8a).

`PTY_INPUT_COORDINATOR_CONTEXT` (`session_context.rs:2495`) is renamed with **no** snapshot and **no** version bump: it has no recognizer and is never written to a user-owned file (§3.7).

`config/injected_messages.rs` takes exactly one Rule R edit, at `:78` inside `CONTEXT_ALERT_DOC_COMMENT`, and **no** version or hash change: the hashed value is `template`, not the doc comment (`:975-980`), and `DEFAULT_CONTEXT_ALERT_TEMPLATE`'s only occurrence of the concept is the `%WORKGROUP%` token, which is Rule P0. `known_default_sha256` is therefore unchanged and `e672581d...` stays the single entry. A reviewer who sees a second hash appended there should treat it as a defect.

#### D8f. The `render_skills_section` replica-line split (new in round 3)

`session_context.rs:831` is the one occurrence of the retired token inside `render_skills_section` (`:812-919`), and that function is read twice: it renders the live `## Skills` section into every agent's context, **and** `is_provably_generated_legacy_skills_section` (`:4183`) recomputes it and compares it against the skills section extracted from a user's on-disk `Context.AgentsCommander.md` (§3.7 family 3). Rule R forces the first to change; the second must keep recognizing a section that carries the old line, or every pre-rename generated context flips to `NotLegacy` and stops self-healing, permanently and silently. This is the same shape as #1005 and it takes the same shape of fix.

1. **The live line takes Rule R.** `:831` becomes:

```
                GENERATED_SKILLS_SECTION_REPLICA_LINE,
```

with, declared beside `GENERATED_SKILLS_SECTION_INTRO` at `:222`:

```rust
const GENERATED_SKILLS_SECTION_REPLICA_LINE: &str = "When running from a room replica, resolve skills/... against the origin Agent Matrix path above, not against the replica CWD.\n";
```

Hoisting the live text into a named constant is not cosmetic: the compare in step 3 needs both spellings as constants, and a bare literal at `:831` would leave the two sides of that compare able to drift apart, which is the defect this whole item exists to prevent.

2. **The pre-rename bytes are frozen**, modelled on `LEGACY_GENERATED_SKILLS_SECTION_INTRO` (`:225-236`) including its provenance doc comment:

```rust
/// #1614: `render_skills_section`'s replica line exactly as it shipped through
/// base commit d7008b34, frozen so a legacy rendered default context whose
/// embedded skills section carries THIS line keeps classifying StaleGenerated
/// and self-heals (#664) after the Room rename; consumed by
/// `is_provably_generated_legacy_skills_section`. Never edit.
/// Provenance: the d7008b34 blob line 831, decoded; 131 bytes, sha256
/// A5C74FD6...7EC9 (plan section 3.12 Table B); pinned by
/// `skills_section_replica_line_split_is_correct`.
const GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME: &str = "When running from a workgroup replica, resolve skills/... against the origin Agent Matrix path above, not against the replica CWD.\n";
```

Copy it from `git cat-file blob d7008b34:src-tauri/src/config/session_context.rs` line 831 verbatim, including the trailing `\n`. §3.12 Table A pins the source line at 152 bytes / `A9DC9244...AFD3` and Table B pins the decoded value at 131 bytes / `A5C74FD6...7EC9`.

3. **The two-sided compare is extended to a line swap, stated exactly.** `is_provably_generated_legacy_skills_section` (`:4183-4208`) replaces its `let normalized = normalize_context_for_compat(section);` at `:4190` with:

```rust
    // #1614: a section generated before the Room rename carries the frozen
    // pre-rename replica line while `render_skills_section` now emits the
    // current one. CRLF-normalize first as defence in depth, since the byte-
    // pinned constant carries LF; swap while the line still carries its
    // terminating newline; then finish normalizing. Both halves of the swap
    // are constants, so the two sides cannot drift apart.
    let normalized = normalize_context_for_compat(&section.replace("\r\n", "\n").replace(
        GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME,
        GENERATED_SKILLS_SECTION_REPLICA_LINE,
    ));
```

**The three-step order is load-bearing and each step earns its place**, so it is written out rather than left to the implementer:

- **CRLF first, and this one is defence in depth rather than load-bearing.** `normalize_context_for_compat` is `value.replace("\r\n", "\n").trim_end()` (`:4259-4261`) and the frozen constant carries LF, so a swap attempted against CRLF bytes would not match. **On this call path the section can never carry CRLF**, and round 3 was wrong to say the swap "would miss every file written on Windows, which is most of them". The call path, traced: `classify_legacy_rendered_default_context` normalizes the whole file at `:4039`; `looks_like_generated_legacy_default_context` receives that normalized string at `:4050`; `reconstruct_legacy_rendered_default_context` is called only from `:4080` with it; `extract_legacy_skills_section` (`:4175`) returns a slice of it; and `is_provably_generated_legacy_skills_section` has exactly one caller, at `:4148`. So the existing `normalize_context_for_compat(section)` at `:4190` is already a redundant CRLF pass and the new `.replace("\r\n", "\n")` is defensive. **The code does not change**: the `.replace` costs nothing, keeps the swap correct if a second caller ever hands in unnormalized bytes, and the three-step order is unaffected. Only the stated reason changes, so a reader does not inherit a false fact about the call path. `ac-dev-rust-grinch-v3` traced this in round 3; `ac-dev-rust-v3` had independently accepted the round-3 bullet as load-bearing, and the traced call path is the more specific evidence.
- **Swap before `trim_end`, not after.** The frozen constant ends in `\n` and that newline is inside the digest. If the replica line were ever the last content of the extracted section, a `trim_end` applied first would strip that newline and the swap would silently not match. The section normally continues into `### Available Skills` or the empty-skills message, so this is only reachable when `push_with_budget` (`:726-733`) refuses the rest against `SKILL_INDEX_TOTAL_MAX_BYTES`; ordering the swap first costs nothing and removes the case entirely.
- **Both compares see the swapped text.** The equality at `:4191` and the #1005 intro-swap fallback at `:4201-4207` are left byte for byte as they are and now operate on the line-swapped text, so all four combinations of {old, new} intro and {old, new} replica line resolve. Extending the fallback instead would have made the two swaps order-dependent on each other.

4. **The comment at `:4194-4200` is updated, because after this change it is false as written.** It says every other literal in `render_skills_section` is frozen; one of them is now swapped. The new text names both swaps and keeps the warning:

> `#1005 S1` swaps the byte-pinned legacy intro; `#1614` swaps the byte-pinned pre-Room-rename replica line. Every other literal in `render_skills_section` is frozen for this project; if one ever changes, this compare must extend with it or healing dies silently.

This is Rule P1 clause (b): a comment that would contradict the code it documents after the change.

5. **Two criteria, neither self-referential** (AC7.14, AC7.15). The digest pin is external, taken from this document. The behavioral pin is a fixture whose skills section carries the **pre-rename** bytes, asserted to still classify as a legacy generation after the whole change; it is modelled on the repository's own `legacy_intro_skills_section_still_classifies_stale_generated_and_heals` (`:10016`), which is the #1005 twin of exactly this test, and it gets the negative control that test has (`edited_legacy_intro_skills_section_is_preserved_not_healed`, `:10096`). Both are specified in §9.1.

**What does not change.** `GENERATED_SKILLS_SECTION_INTRO` (`:222`) and `LEGACY_GENERATED_SKILLS_SECTION_INTRO` (`:225-236`) are untouched; the other three arms of the `match` at `:816-847` carry no occurrence of the retired token; and `:6829` inside `generated_shaped_manual_legacy_skills_content_is_preserved` (`:6804`) is **not** edited. That test builds a manual skills section carrying the old line plus manual skill entries, and it asserts only that `KEEP_MANUAL_SKILLS_RULE_IN_CONTEXT` and `KEEP_MANUAL_WARNING_IN_CONTEXT` survive. Its fixture can never equal a fresh render, so it classifies `NotLegacy` and stays green in all four rename combinations. It is therefore neither a red test nor a backstop, and §9.3 says so explicitly rather than leaving a reviewer to work it out.

### 5.11 D9: the mixed root, surface by surface

The assignment asks what happens when a `.ac` root holds both `wg-1-<team>` and `room-1-<team>`. Every answer below follows from the fact that **every identity path already keys on the full directory name**, never on the prefix or the number.

| Surface | Behavior | Why |
| --- | --- | --- |
| GUI discovery | Both listed, in `read_dir` order then whatever the caller sorts by | `ac_discovery.rs:1182` and `:1957` accept both (P8, P9) |
| CLI `room list` / `workgroup list` | Both listed | `list_workgroup_dirs` filters through `parse_team_from_workgroup_name` (S3) |
| Team membership and team deletion | Both collected and both deleted with the team | `collect_team_workgroup_dirs` and its twin accept both (P12, P13); both derive team `<team>` from the suffix |
| Sidebar rail grouping | Both appear; a **user-authored** group regex matches whichever names it matches | `groupMatchId(wg)` is the directory name and the regex is user data (§3.8) |
| Rail sort order | By slot number, then the two `1`s tie and fall to the existing secondary sort | F4 now returns `1` for both instead of `MAX_SAFE_INTEGER` for the Room |
| Rail tooltip / titlebar badge | `WG1:(agent)` and `ROOM1:(agent)`; `WG-1-TEAM` and `ROOM-1-TEAM` | F1, F5 derive the label from the real name (§5.4) |
| Orchestrator clocks | Two independent key sets | `coordinator_clocks` is keyed by the full name |
| TASK.md | Each has its own; the walk-up finds the nearest ancestor | P30 / F6 accept both, and the ancestor walk is unchanged |
| Mailbox routing | Exact | `resolve_wg_path_from_session_dirs` builds the marker `/{wg_name}/` from the full name (P25) |
| Peer FQN | `project:wg-1-team/agent` and `project:room-1-team/agent` are two distinct peers | `teams.rs:89` builds `format!("{}:{}/{}", project, wg, agent)` from the directory name (P20) |
| Messaging authorization Rule 2 | Two replicas are "same entity" only when the full names are equal | `teams.rs:1661-1670` compares `from_wg == to_wg` (P24) |
| Message filenames | `wg1-...` and `room1-...` | §5.9 |
| `purge-room` scope | The caller's own entity only, resolved from its own path | P25, unchanged semantics |
| New Room allocation | Next is `room-2-<team>`, because `wg-1-<team>` is not counted | §5.5 |

`api/identity.rs` and `api/actuation.rs` carry **no** prefix predicate: `git grep -n 'wg-' -- src-tauri/src/api/identity.rs src-tauri/src/api/actuation.rs` returns only `#[cfg(test)]` literals (`identity.rs:341`, `:353`, `:370`; `actuation.rs:156-158`, `:169`, `:180`, `:196`, `:200`, `:207`, `:211`, `:218`, `:236`). They treat the FQN as opaque, so requirement (D) holds there with **zero production edits**; §9 adds `room-` twins to those fixtures to prove it rather than assert it.

### 5.12 D10: the GUI edits

Every occurrence in §3.8 classes (a), (b) and (c) takes its Rule R substitution, including the seven sites round 1's enumeration missed and the two files it left out of §6.2 entirely. Five of them need a decision recorded:

- `ProjectPanel.tsx:2542` `placeholder="wg-2.*"` becomes `placeholder="room-2.*"`. It is an example of the regex shape a user types for a group. New entities are Rooms, so the example names a Room. A user whose root holds only legacy Workgroups must type `wg-` themselves, which is the same thing the placeholder always required of them for any name that was not literally `wg-2`.
- `ProjectPanel.tsx:993`/`:995` and their three resolvers at `:1057`, `:1058`, `:1071` move together in one edit (Rule P clause R1). `"Selected Workgroup"` becomes `"Selected Room"` and `"Workgroups"` becomes `"Rooms"` in all five places.
- `guide/components/HintsTab.tsx:70` is a two-sentence hint that uses the word three times; it is renamed in full, including `AgentsCommander workgroup replicas` -> `AgentsCommander room replicas`.
- `TeamContextAlertsEditor.tsx:78-79` uses the possessive, HTML-escaped: `sends that workgroup&apos;s orchestrator`. It becomes `sends that room&apos;s orchestrator`. The escape is why a `workgroup's` needle could never have found it, and it is the reason §9.4 AC1 greps the concept word rather than phrases.
- `ProjectPanel.tsx:3777` reads `Delete workgroup <strong>...` in lower case while `:3773` and `:3361` read `Delete Workgroup` in title case. Both cases are renamed. Round 1's needle set had `Delete Workgroup` and not `Delete workgroup`, and `git grep -F` is case-sensitive, so `:3777` shipped green; AC1b pins the lower-case form explicitly as a regression check.

`AgentPickerModal.tsx:944`'s visible text `Entire workgroup` becomes `Entire room` while the scope **value** `"workgroup"` at `:389`, `:477`, `:935`, `:936`, `:941`, `:942` and `:1055` and the testid `agentPicker.scope.workgroup` stay (Rule P).

`resource-monitor/App.tsx:673`/`:677` and `watchers/App.tsx:851` rename the filter label and its `aria-label`; `App.tsx:92`, `:375`, `:389` read the `group.workgroup` field and are Rule P.

### 5.13 D11: documentation

**The `purge-wg` occurrences in documentation, enumerated (new in round 3).** `purge-wg` becomes `purge-room` in prose that names the command, with the deprecated alias documented once in `docs/reference/cli.md`'s section heading rather than repeated. The eight sites at `d7008b34`:

| Site | Visible to round 2's AC1 docs sweep? |
| --- | --- |
| `docs/reference/cli.md:703` (`## \`purge-wg\``) | no |
| `docs/reference/cli.md:708` (`agentscommander purge-wg \\`) | no |
| `docs/reference/cli.md:29`, `:30`, `:31` (exit-code rows) | no |
| `docs/reference/architecture.md:735` (the `purge_guard.rs` row) | no |
| `docs/reference/cli.md:19` | yes, it also says `workgroup` |
| `docs/features/project-loops.md:68` | yes, it also says `workgroup` |

Six of the eight were invisible to round 2's gate, which is H4's finding; §9.4 AC1 unifies the alternation so all eight are swept.

**`docs/features/session-auto-close.md:17`** is also new in round 3: the team key it documents as `` `<project>:<wg>` `` names the entity directory segment, which for a Room is `room-<N>-<team>`, so the placeholder becomes `` `<project>:<room>` `` with the standing legacy clause, exactly as §5.9 treats the `<wgN>` placeholders.

All 57 `docs/` files plus `README.md`, `ROADMAP.md`, `PRIVACY.md` and `src-tauri/src/api/README.md` take their Rule R substitution. `CHANGELOG.md`'s existing entries are Rule P3 and are not rewritten; one new entry is added describing the rename, the new directory prefix and the deprecated aliases.

**Five files must additionally carry the compatibility statement**, because they are where a reader looks for what the product accepts rather than what it calls things. Each gains, in its own voice, all three facts: new entities are created as `room-<N>-<team>`; existing `wg-*` directories are never renamed and remain fully supported; `workgroup`, `purge-wg`, `--wg` and `--workgroup` remain accepted as deprecated aliases and will be removed in a later release.

1. `docs/reference/cli.md` (50 lines), which documents the subcommands and flags.
2. `docs/reference/directory-layout.md` (3 lines), which is the authority on on-disk names.
3. `docs/agents/teams-and-workgroups.md` (27 lines), the concept page.
4. `docs/glossary.md` (9 lines), which defines the term.
5. `docs/concepts.md` (8 lines).

`docs/features/context-tracking.md` is a sixth file with a constraint of its own: `:75` and `:78` quote the injected-message default template and its placeholder list. `%WORKGROUP%` is a machine token (§3.10, D17) and stays byte for byte; only the prose around it, including "`%WORKGROUP%` for the workgroup it belongs to", takes Rule R. A documentation sweep that renamed the token would put the docs out of step with a file the product actually parses.

The two documentation **file names** and every intra-repository link that targets them are unchanged (Rule P0, residual R3).

---

## 6. Affected surfaces, exhaustively

Round 1 titled this section exhaustive while §6.1 was a hand-list and §13.2 gate 5 required "exactly the §6 paths changed", so a correct edit tripped the scope gate. §6.1 and §6.2 below are regenerated from the sweeps in §3.14 and §3.8, and gate 5 is reworded in §13.2 so that being right cannot be red.

### 6.1 `repo-AgentsCommander`, production Rust

Derived from §3.14's 55-file candidate set. A file is in the **edited** list when it carries at least one Rule R site or one dual-prefix gate; it is in the **preserved** list when every one of its hits is Rule P, and the clause is named in §3.14.

New file: `src-tauri/src/config/entity_prefix.rs`.

**Edited (41 files).** `config/mod.rs` (one `pub mod` line); `commands/entity_creation.rs`; `commands/ac_discovery.rs`; `commands/config.rs`; `commands/task.rs`; `commands/pty.rs`; `cli/mod.rs`; `cli/purge_wg.rs`; `cli/workgroup.rs`; `cli/team.rs`; `cli/loop_cmd.rs`; `cli/list_peers.rs`; `cli/role_experiment.rs`; `cli/close_session.rs`; `cli/send.rs`; `cli/self_switch.rs`; `cli/task_set_title.rs`; `cli/task_append_body.rs`; `config/ac_root.rs`; `config/coding_agent_profiles.rs`; `config/injected_messages.rs`; `config/loops.rs`; `config/placeholders.rs`; `config/replica_identity.rs`; `config/root_agent.rs`; `config/seed_manifest.rs`; `config/seeded_context_templates.rs`; `config/session_context.rs`; `config/teams.rs`; `loops/delivery.rs`; `loops/non_stop_watchdog.rs`; `phone/mailbox.rs`; `phone/messaging.rs`; `phone/types.rs`; `pty/container_paths.rs`; `pty/container_repos.rs`; `screenshot/windows.rs`; `session/context_alerts.rs`; `session/session.rs`; `web/commands.rs`; `api/actuation.rs`.

The nine new entries relative to round 1 are `commands/pty.rs`, `config/injected_messages.rs`, `config/seed_manifest.rs`, `loops/delivery.rs`, `loops/non_stop_watchdog.rs`, `phone/types.rs`, `session/context_alerts.rs`, `web/commands.rs` and `api/actuation.rs`; each carries a reader-facing string enumerated in §3.14 and none was reachable by any round-1 needle.

**Preserved, Rule P only (16 files). No production line in any of them changes.** `api/auth.rs`; `api/schema.rs`; `cli/task_ops.rs`; `commands/session.rs`; `commands/wg_delete_diagnostic.rs`; `config/coordinator_clocks.rs`; `config/project_settings.rs`; `config/sessions_persistence.rs`; `config/settings.rs`; `pty/git_watcher.rs`; `pty/terminal_snapshot/acceptance_tests.rs`; `pty/terminal_snapshot/resource_tests.rs`; `resource_monitor/types.rs`; `screenshot/mod.rs`; `session/manager.rs`; `session/purge_guard.rs`.

**The constraint on this list is on production lines, not on path presence, and round 3 corrects that.** Round 2's header read "none may appear in the diff", and §6.7, AC10 and §13.2 gate 5 repeated it as path presence. That is the wrong shape of constraint for this partition, because the partition is generated from §3.14, whose step 1 **drops every line inside a `#[cfg(test)]` item**. It answers "which production lines take Rule R". It cannot answer "which paths may appear in the diff", because the test assertions live in exactly the half it dropped, and §9.3 and §9.1 require some of them to move. Round 1's G4 was this defect on the *edited* half of the partition; round 2 fixed that half and left the preserve half hard-blocking. The constraint is therefore restated once, here, and §6.7, AC10 and gate 5 are reworded to match:

> **No line outside a `#[cfg(test)]` item changes in any of the 16 files.** A `#[cfg(test)]` edit inside one of them is permitted only when §9.3's clauses authorize it, and each such edit is named below.

The named `#[cfg(test)]` edits inside the preserved 16, and there are exactly three:

| Site | Why it must move | Authorized by |
| --- | --- | --- |
| `commands/session.rs:5392` | `assert!(err.contains("workgroup root"))`, inside `#[cfg(windows)] fn container_path_context_refuses_junction_targeting_workgroup_root` (`:5374-5395`). The string it matches is produced in a **different file**, `pty/container_paths.rs:293` (`"container transport refuses to bind-mount workgroup root '{}'"`), which §5.3 renames under Rule R. The assertion is red on a correct implementation | §9.3 clause 1 |
| `pty/terminal_snapshot/acceptance_tests.rs` | a `room-` twin is added beside `const WORKGROUP: &str = "wg-1-dev-team"` (`:74`); the existing constant is unchanged | §9.1, "API and snapshot fixture twins" |
| `pty/terminal_snapshot/resource_tests.rs` | `room-` twins are added beside the two FQN literals at `:33` and `:34`; both existing literals are unchanged | §9.1 |

That set is closed and it was measured, not assumed: all 16 preserved files were swept for an assertion that pins a string this plan renames, and `commands/session.rs:5392` is the only one that actually goes red. `commands/wg_delete_diagnostic.rs:1970-1975` pass their `"Workgroup"` label in themselves and assert `is_err`/`is_ok`, so they stay green; every other hit in the preserved set is a directory-name fixture, a JSON key or an FQN literal that Rule P keeps.

`api/README.md` appears in §3.14's 55-file count because the sweep is path-shaped; it is documentation and is owned by §6.5. That leaves 54 code files in the candidate set. 38 of the 41 edited files are among them; the other three are `config/mod.rs` (a `pub mod` line, no string), `cli/loop_cmd.rs` (two flag declarations, no free prose) and `cli/self_switch.rs` (whose only site, `:47`, is invisible to the line-based rule, as §3.14 states). 38 plus the 16 preserved is 54. The arithmetic is stated so a reviewer can check the partition is total rather than sampled, and it is **unchanged** by §3.14's round-3 correction: recomputed with `#[cfg(test)]` scoped per item, the candidate set is the same 55 files, so the same 38 and the same 16 fall out of it.

**Two file classes are in neither list, by construction, and are placed explicitly so nothing falls between them.** A file whose only hits are inside `#[cfg(test)]` never enters §3.14's candidate set at all, so it can be in neither §6.1 list and still need a test edit. There are two such files in this change, `src-tauri/src/api/identity.rs` (all three hits, `:341`, `:353`, `:370`, sit after the `#[cfg(test)]` at `:301`) and `src-tauri/src/cli/terminal_snapshot.rs` (three FQN fixtures inside the `mod tests` whose `#[cfg(test)]` is at `:716` and whose `mod tests` line is `:717`; the file's other `#[cfg(test)]`, at `:398`, precedes all three and changes nothing, see §3.14). The first takes fixture twins under §9.1 and is placed in §6.4; the second is untouched. Neither is a §6.1 omission and neither is a scope violation.

### 6.2 `repo-AgentsCommander`, production TypeScript

Derived from §3.8's 41-file sweep. Round 1 listed 16 edited files and missed two files entirely plus four visible-text sites inside two files it did list.

New file: `src/shared/entity-prefix.ts`.

**Edited (18 files).** `shared/path-extractors.ts`; `shared/profile-utils.ts`; `guide/components/HintsTab.tsx`; `resource-monitor/App.tsx`; `watchers/App.tsx`; `sidebar/components/AcDiscoveryPanel.tsx`; `ActionBar.tsx`; `AgentPickerModal.tsx`; `EditLoopModal.tsx`; `NewLoopModal.tsx`; `NewWorkgroupModal.tsx`; `ProjectPanel.tsx`; `SettingsModal.tsx`; `TeamContextAlertsEditor.tsx`; `WorkgroupGroupRail.tsx`; `sidebar/stores/workgroup-groups.ts`; `terminal/components/WorkgroupTask.tsx`; `terminal/components/TaskCleanConfirmModal.tsx`.

Two of those files are new relative to round 1: `TeamContextAlertsEditor.tsx` (`:78-79`) and `TaskCleanConfirmModal.tsx` (`:72`). Two more were already listed but had missed visible-text sites: `AgentPickerModal.tsx` (`:949`, `:972`) and `SettingsModal.tsx` (`:2089`, `:2263`).

18 edited plus 23 preserved is 41, which is exactly the sweep's file count. The arithmetic is stated so a reviewer can check the partition is total rather than sampled.

**Preserved, Rule P only (23 files). No line in any of them changes, and their co-located `*.test.ts(x)` files are not in this list and are owned by §6.4 (§6.7 Part 2).** `shared/types.ts`; `shared/ipc.ts`; `shared/testing/ui-harness.tsx`; `sidebar/App.tsx`; `sidebar/components/ArchivedProjectsModal.tsx`; `SessionItem.tsx`; `Titlebar.tsx`; `WorkgroupGroupsModal.tsx`; `loop-modal-helpers.ts`; `replica-dot.ts`; `replica-repo-badges.ts`; `workgroup-delete-diagnostics.ts`; `workgroup-session.ts`; `sidebar/stores/project.ts`; `project-collapse.ts`; `project-merge.ts`; `sessions.ts`; `team-idle-watcher.ts`; `sidebar/watchdog/non-stop-watchdog-client.ts`; `terminal/App.tsx`; `terminal/components/Titlebar.tsx`; `terminal/stores/terminal.ts`; `watchers/activity.ts`.

`Titlebar.tsx` in both trees renders the badge but does not compute it: the computation is `path-extractors.ts:25` (F1), which is in the edited list. `WorkgroupGroupsModal.tsx`'s 54 hits are all CSS classes and testids.

### 6.3 Repository configuration

`.gitignore` (one added line at 18).

### 6.4 Tests

`src-tauri/tests/cli_behavior_contract.rs` (help and flag assertions at `:306-331`, `:760-776`), `src-tauri/tests/cli_workgroup_team.rs`, and the `#[cfg(test)]` modules of every production file named in **either** §6.1 list whose assertions pin a renamed string, a bumped version or a widened predicate (§9.3). New tests per §9.1. Frontend: the vitest files whose expectations pin a renamed label.

**Plus `src-tauri/src/api/identity.rs`, which is in neither §6.1 list and takes a `#[cfg(test)]`-only edit.** Its three `wg-` hits (`:341`, `:353`, `:370`) are all after the `#[cfg(test)]` at `:301`, so §3.14's production narrowing excludes the file entirely and §6.1 never sees it. §9.1 directs `room-` fixture twins into it, and this is where that edit is placed. It takes **no** production edit, which is the point: requirement (D)'s "zero production edits for the API identity surface" is asserted by the twins rather than assumed.

"Every production file above" therefore means **both §6.1 lists plus `api/identity.rs`**, and the two `pty/terminal_snapshot/*_tests.rs` files, which are on §6.1's preserved list, take the twin edits named in §6.1's table. Every one of these is a `#[cfg(test)]` edit; no production line in any preserved file moves.

### 6.5 Documentation

57 `docs/` files, `README.md`, `ROADMAP.md`, `PRIVACY.md`, `src-tauri/src/api/README.md`, plus one new `CHANGELOG.md` entry.

### 6.6 Generated artifacts

`src-tauri/module-arcs.txt`, regenerated (§11). `scripts/room-rename-allowlist.tsv`, new (§9.4 AC1).

### 6.7 The preserve set, in two parts, because they take different constraints

Round 2 stated this as one list of "paths that must NOT appear in the diff". That is right for most of it and wrong for the part drawn from §6.1 and §6.2, for the reason §6.1 now gives: those two lists partition **production lines**, and a correct implementation must still edit `#[cfg(test)]` code inside three of them. The set is therefore split, and AC10 and §13.2 gate 5 test the two parts differently.

**Part 1, paths that must not appear in the diff at all.** `plans/*.md` other than this plan file; `test-debt.allowlist.json`; `docs/assets/og-card.svg`; `scripts/smoke-cli-powershell.ps1`; `scripts/smoke-cli-release-windows.ps1`; `scripts/smoke-current-app-mockup.mjs`; `src/sidebar/styles/sidebar.css`; `src/shared/constants.ts`; `dependency-cruiser.config.mjs`; every `.github/workflows/*.yml`; `package.json`; `Cargo.lock`; `src-tauri/Cargo.toml`; and every other file whose only match is a CSS class, a testid, a serde key or an identifier.

**Part 2, files whose production lines must not change.** The 16 Rust files named as Rule-P-only in §6.1 and the 23 TypeScript files named as Rule-P-only in §6.2. A path from this part appearing in the diff is not by itself a failure; a **production line** changing inside one of them is. For the Rust half, "production line" means a line outside a `#[cfg(test)]` item, and the three authorized `#[cfg(test)]` edits are named in §6.1's table. For the TypeScript half the repository keeps tests in separate `*.test.ts(x)` files, which the §3.8 sweep already excludes and which §6.2 never listed, so a `.test.tsx` edit beside a preserved production file is likewise outside this constraint and inside §6.4.

---

## 7. Required behavior, edge cases, failure behavior

1. **A new entity is `room-<N>-<team>`.** `N` is the lowest free positive integer among `room-<n>-<team>` directories in that AC root, across all teams, exactly as the Workgroup allocator was. Legacy `wg-*` directories are not counted.
2. **`read_dir` failure during allocation still degrades to slot `1`.** Unchanged. The `wg_dir.exists()` guard immediately after allocation still converts the degraded case into an `already exists` error, now worded for Rooms.
3. **A Room and a legacy Workgroup with the same slot number coexist.** Every surface distinguishes them by full directory name (§5.11). Nothing keys on the number alone.
4. **A user-authored sidebar group regex is user data and is not migrated.** A saved group whose regex is `^wg-` matches no Room. The mitigation is the renamed placeholder (`room-2.*`) and the documented behavior in `docs/features/sidebar-guide.md`; the "create group from this entity" menu action at `ProjectPanel.tsx:1526` already generates a regex from the actual name and therefore produces a Room-matching regex for a Room. Recorded as residual R4.
5. **A legacy Workgroup replica's message filenames do not change.** `agent_short_name("wg-7-dev-team/architect")` still returns `wg7-architect`. Only Room replicas produce `room7-architect`. No existing message file is renamed, and none becomes invalid: `validate_filename_shape` accepts both (§5.9).
6. **A Room replica's notification body grows by at most 8 bytes against the 1024-byte PTY budget.** `PTY_SAFE_MAX = 1024` (`phone/messaging.rs:12`) is enforced at `api/actuation.rs:50` as `body.len() + PTY_WRAP_FIXED + from.len() > PTY_SAFE_MAX`, and its own error text names that budget (`:52`). A Room grows the inputs in exactly four places, each by the 2 bytes that separate `room-` from `wg-` and `roomN` from `wgN`: the entity directory name inside the absolute message path (+2), the two short tokens inside the message filename (+2 each), and the entity directory name inside the sender FQN that `from` carries (+2). Total **+8 on a canonical shape**. At typical path lengths this is far from the limit and no behavior changes; it is stated rather than left unstated because the failure it moves toward is a refusal whose message the user reads, and because the failing case (a deep path plus a long slug) is the one the error already tells the user to shorten. No test is added for it: the existing `PTY_SAFE_MAX` clamp tests at `messaging.rs:854-883` are arithmetic over lengths and are prefix-agnostic. Recorded as residual R11.
7. **`purge-wg` and `purge-room` are the same command.** Identical parse, identical outbox message, identical `action` value `"purge-wg"` on the wire, identical exit codes 0/1/2/3/4, identical `print_status_prose` output. Help and usage render `purge-room` (§5.8 fact 4). The user-visible prose that names the command says `purge-room` (§5.8).
8. **An older CLI talking to a newer daemon, and the reverse, both work.** The outbox `action` value is unchanged, the message-filename shape is unchanged, and every FQN is still the literal directory name. In-flight messages are unaffected.
9. **An installation whose context template is pristine keeps auto-updating.** Guaranteed by the three frozen snapshots and the three dual-use splits (§5.10) and asserted by AC7. This holds for all five recognizer families of §3.7 and not only for the three seeded specs, which is what round 1's plan did not establish.
10. **An installation whose `Context.AgentsCommander.md` is a legacy generated default keeps being recognized, and the two outcomes are stated separately because only one of them heals.** The classifier (`:4033-4055`) compares against the **current** reconstruction first and returns `Current` on equality (`:4046`); only a file that fails that compare and is still reconstructible reaches `StaleGenerated` (`:4050-4051`). Both outcomes must survive this change, and they are different assertions over different fixtures:
    - **`Current`.** A file whose bytes are `legacy_rendered_default_context_for_compat` over the same inputs the classifier is given is returned as-is at `:2744`. It is not healed, because there is nothing stale about it. Round 2 asserted `StaleGenerated` over exactly this fixture, which the classifier cannot produce for it; that is corrected in §9.1.
    - **`StaleGenerated`.** Reached by a file that is a **pre-#1072** generation, or by one whose embedded skills section is an older generation than the live renderer emits. Both are rewritten to the current format by the #664 self-heal (`heal_stale_global_recorded`, `:2745-2757`).

    Three freezes hold those outcomes: `legacy_rendered_default_context_for_generation` byte for byte (§5.2 P3, §5.10 D8b), the pre-#1072 git-scope pair it reads, and, new in round 3, the pre-Room-rename replica line the skills-section compare needs (§5.10 D8f). Round 1's edit set would have reclassified every pre-#1369 file as `NotLegacy` permanently; round 2's would have done the same to every file whose skills section carries `:831`'s pre-rename line, which is every replica that has a `skills/` directory.
11. **An installation whose `Role.md` is a pristine pre-rename generation still reduces to `MINIMAL_ROOT_ROLE_MD`.** Guaranteed by wiring `ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD` into `migrate_root_role_file`'s list as well as into `old_generated` (§5.10 D8a), and asserted by AC7.10.
12. **An installation whose `Role.md` still carries the deferred-messaging paragraph still migrates.** `OLD_DEFERRED_MESSAGING_PARAGRAPH` is unchanged (§5.10 D8d), so the `contains()` still matches; the substituted paragraph now says Room (§5.10 D8c).
13. **An installation whose `injected-messages.toml` entry is pristine still auto-refreshes.** `DEFAULT_CONTEXT_ALERT_TEMPLATE` and `%WORKGROUP%` are unchanged, so its `known_default_sha256` still matches (§3.7 family 5). A user template that references `%WORKGROUP%` keeps expanding. The placeholder's *description* in the file header says Room from the next reseed onward; existing files keep the old header until they are reseeded, which is the same asymmetry as residual R2.
14. **An installation whose context template is customized is still preserved, not overwritten.** Unchanged: unknown content is backed up, never deleted.
15. **An existing `.ac/.gitignore` gains `room-*/` before the first Room exists.** Guaranteed by the call ordering measured in §3.4. If `ensure_ac_root_gitignore` fails, both creation paths log a warning and continue, which is the current behavior; the Room is then created into a root whose ignore file lacks the pattern. That pre-existing fail-soft is **not** changed by this plan, and it is recorded as residual R5 with its owner, because tightening it is a behavior change outside this issue.
16. **The TASK.md buttons work in a Room.** F6's gate is case-sensitive and now matches `/room-` as well as `/wg-`. An uppercase `ROOM-1-team` directory would still fail the gate, exactly as an uppercase `WG-1-team` does today, and for the same documented reason.
17. **Team deletion removes both kinds.** `delete_team` collects `room-*-<team>` and `wg-*-<team>` and removes both, which is the only coherent meaning of deleting a team.
18. **A team name that makes the directory name shorter than the prefix cannot panic.** The index arithmetic that could produce a reversed range is removed (§5.3).
19. **The `.deleting-*` sentinel still cannot surface as a ghost entity.** Its guard widens with the discovery filter (§9.3 clause 3), so a sentinel is excluded from both prefixes rather than only from `wg-`.
20. **Nothing renames, moves or deletes an existing `wg-*` directory.** Asserted negatively by AC10 and by the absence of any rename call in the diff.

---

## 8. Compatibility and security

**Compatibility.**

- On-disk: additive only. No directory is renamed. No config file is rewritten by this change other than a user's `.ac/.gitignore`, which only gains lines.
- CLI: additive only. Every command line that works today still works and produces the same result.
- IPC and events: unchanged. No Tauri command name, event name or payload key moves.
- Persistence: unchanged. Loop targets, profile scopes, collapse keys, project settings and team configs keep their current values and keys.
- Wire: unchanged. `PURGE_WG_ACTION` stays `"purge-wg"`.
- Context templates: three versions bump, three snapshots are frozen, so no installation regresses.
- Downgrade: an older binary run against a root containing `room-*` directories will not discover them (they are simply invisible to it) but will not corrupt them. It will also allocate `wg-` slots again. That is expected for a downgrade and is not a supported path.

**Security.**

- `is_valid_wg_local_shape` (S5) and `teams.rs:649` (S7) are **authorization** shape checks, and `teams.rs:1661` (P24) is the messaging authorization rule. Widening them to a second prefix widens the set of accepted names. The widening is exactly one additional literal prefix followed by the same digit-and-team validation, and the "same entity" comparison stays a full-name equality, so no cross-entity message becomes authorized that was not authorized before. AC4's negative case asserts this directly.
- `entity_prefix_of` and `strip_entity_prefix` are total functions over `&str` with no allocation, no filesystem access and no panic path.
- `role_experiment.rs:2328`'s containment check (`canonical.starts_with(ac_root)`) is unchanged; only the name predicate beside it widens.
- `path_identity::verify_directory` (P22) and `validate_delete_root_not_link_or_reparse` are untouched, so link and reparse-point defences are unchanged.
- No new dependency, no new process spawn, no new network call, no new filesystem write path.

---

## 9. Tests and objective acceptance criteria

### 9.1 New tests, and what each one actually proves

**Rust unit tests.**

| Test | Location | Proves |
| --- | --- | --- |
| `entity_prefix_accepts_room_and_legacy_and_rejects_others` | `config/entity_prefix.rs` | `has_entity_prefix` is true for `room-1-t` and `wg-1-t`, false for `roomx`, `wgx`, `_team_t`, `__agent_x`, `""`; `strip_entity_prefix` returns `1-t` for both |
| `room_allocator_ignores_legacy_workgroup_directories` | `commands/entity_creation.rs` | in a root holding `wg-1-t`, `wg-2-t`, `determine_next_wg_number` returns **1** (requirement B, AC3) |
| `room_allocator_reuses_lowest_free_room_slot` | same | with `room-1-t` and `room-3-t` present it returns **2**, so reuse semantics are preserved |
| `room_allocator_still_degrades_to_one_on_read_error` | same | the `read_dir`-failure path is unchanged |
| `create_room_on_disk_uses_room_prefix` | same | `create_workgroup_on_disk` in a root holding `wg-1-t` creates `room-1-t` and nothing else (AC3) |
| `collect_team_dirs_finds_room_and_legacy_and_survives_long_team_names` | same | mixed root returns both; a team name longer than the directory remainder returns neither and does not panic (§7.13) |
| `parse_team_from_entity_name_accepts_room` | same | `room-1-ac-devs` parses to team `ac-devs`; `room--devs`, `room-0-devs`, `room-1-` are still rejected |
| `agent_fqn_from_path_builds_a_room_fqn` | `config/teams.rs` | `.../.ac/room-1-t/__agent_dev` becomes `proj:room-1-t/dev` (requirement D) |
| `wg_local_shape_accepts_room_local` | same | `room-1-t/dev` valid, `room-x-t/dev` invalid, `room-1-/dev` invalid |
| `same_entity_rule_denies_cross_prefix_pairs` | same | `proj:room-1-t/a` to `proj:room-1-t/b` authorized; `proj:wg-1-t/a` to `proj:room-1-t/b` **denied** (§8 security) |
| `messaging_root_walks_up_to_a_room_directory` | `phone/messaging.rs` | `workgroup_root(".../room-3-t/__agent_x")` returns the `room-3-t` path |
| `room_short_name_and_filename_shape` | same | `agent_short_name("room-7-dev-team/architect") == "room7-architect"`, `agent_short_name("wg-7-dev-team/architect") == "wg7-architect"`, and `validate_filename_shape(build_filename(ts, "room7-a", "room7-b", "slug"))` is `Ok` |
| `ensure_ac_root_gitignore_creates_both_patterns` | `commands/ac_discovery.rs` | a fresh `.ac` gets both `room-*/` and `wg-*/` |
| `ensure_ac_root_gitignore_appends_room_to_a_legacy_only_file` | same | a `.gitignore` containing only `wg-*/` gains `room-*/` and keeps its existing `wg-*/` line and comment byte-for-byte (requirement E) |
| `frozen_pre_room_rename_global_template_is_recognized` | `config/seeded_context_templates.rs` | `is_known_generated_global_template(GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME)` and `is_known_generated_standalone_global_template(...)` are both true; the constant is `!=` the current default; the constant contains `**Workgroup**` and the current default does not contain `Workgroup` |
| `frozen_pre_room_rename_coordinator_template_is_recognized` | same | same three assertions for the coordinator spec |
| `frozen_pre_room_rename_root_context_is_recognized` | `config/root_agent.rs` | `is_known_generated_root_context_template(ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD)` is true; the constant is `!=` `ROOT_ROLE_MD`; the constant contains `workgroup add` and `ROOT_ROLE_MD` does not contain `workgroup` |
| `seeded_template_versions_were_bumped` | `config/seeded_context_templates.rs` | the three `current_version` values are 5, 6 and 8 |
| `frozen_pre_room_rename_root_context_migrates_on_the_role_path_too` | `config/root_agent.rs` | a `Role.md` whose bytes are `ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD` reduces to `MINIMAL_ROOT_ROLE_MD`. One fixture drives the recognizer list **and** `migrate_root_role_file`'s list, modelled on `frozen_v5_root_context_is_recognized_and_migrated_on_both_paths` (`:2436`), so a snapshot wired into only one of the two cannot pass (§3.7 family 2, AC7.10) |
| `frozen_snapshots_are_byte_exact_at_d7008b34` | `config/seeded_context_templates.rs` and `config/root_agent.rs` | `.len()` and `sha256` of each new snapshot equal the §3.12 Table B values, modelled on `root_context_pre_orchestrator_rename_snapshot_is_byte_exact` (`root_agent.rs:2411`). These are the criteria that are **not** self-referential: the expected values come from the frozen base and are written into this plan, so a coordinated rename that moved both sides fails (AC7.1-7.6) |
| `workgroup_git_scope_split_is_correct` | `config/session_context.rs` | `WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME` is 220 bytes with sha256 `A386B52D...566D`; the live `WORKGROUP_GIT_SCOPE` contains both `room-*/` and `wg-*/`, is 223 chars and 34 words, and the two are `!=` (§5.10 D8b, AC7.4) |
| `old_deferred_messaging_paragraph_is_frozen` | `config/root_agent.rs` | `OLD_DEFERRED_MESSAGING_PARAGRAPH` is 293 bytes with sha256 `6E12E68E...A463`. Its failure mode is a migration that silently never fires, so it gets a pin rather than a code comment (§5.10 D8d, AC7.6) |
| `legacy_rendered_default_context_is_frozen` | `config/session_context.rs` | **both** reconstructions over the fixed synthetic inputs of §3.12 Table C have the lengths and sha256s captured at `d7008b34` in step 0: C1 for `legacy_rendered_default_context_for_compat` and C2 for `pre_1072_legacy_rendered_default_context_for_compat`. The doc comment records the base SHA and states the values were captured by a one-off run at that SHA, never read back from the functions (AC7.7) |
| `pre_1072_context_still_self_heals` | `config/session_context.rs` | **replaces round 2's `pre_1369_context_still_self_heals`.** Real temp directories, `skills_section = render_skills_section(&discover_skill_index(Some(&matrix_root)))`, file bytes from `pre_1072_legacy_rendered_default_context_for_compat`; `classify_legacy_rendered_default_context` returns `StaleGenerated` and `resolve_agent_context` heals the file on disk exactly once. Modelled line for line on the repository's working `pre_1072_legacy_with_matrix_classifies_stale_and_heals_once` (`:6180-6210`) |
| `current_generation_legacy_context_classifies_current` | same | the second outcome of §7 item 10: a file whose bytes are `legacy_rendered_default_context_for_compat` over the same inputs the classifier is given returns `Current` (`:4046`) and is left on disk unmodified (`:2744`) |
| `pre_room_rename_skills_section_still_classifies_stale_generated_and_heals` | same | **the non-self-referential criterion for D8f (AC7.15).** A matrix root with real skills on disk (one valid `SKILL.md`, one with broken frontmatter so the warnings subsection is exercised); the embedded skills section is the current render with `GENERATED_SKILLS_SECTION_REPLICA_LINE` replaced by `GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME`; the file is built with `legacy_rendered_default_context_for_compat` from that section, and `classify_legacy_rendered_default_context` called with the **current** render as `skills_section` returns `StaleGenerated` and heals. Modelled on `legacy_intro_skills_section_still_classifies_stale_generated_and_heals` (`:10016-10090`), which is #1005's twin of this exact test. **What makes it non-self-referential is the pair, not the fixture, and round 4 corrected this sentence.** Round 3 said "its expected value comes from this plan's §3.12 Table B, not from the constant it checks", which is not true of the fixture: the fixture is built **from** `GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME`. The external anchor is supplied by AC7.14, which pins that same constant's length and sha256 to §3.12 Table B, a value taken from this document rather than from the source. So AC7.15 proves the production swap works over the frozen bytes, AC7.14 proves the frozen bytes are the right bytes, and the pair is external even though neither half is alone. D8f step 5 already says this correctly. `ac-dev-rust-grinch-v3` found the wording in round 3 |
| `edited_pre_room_rename_skills_line_is_preserved_not_healed` | same | the negative control, modelled on `edited_legacy_intro_skills_section_is_preserved_not_healed` (`:10096-10146`): one mutated byte inside the frozen pre-rename line means the section is no longer provably generated, so the file classifies `NotLegacy` and is preserved byte for byte, never healed. Without this, the swap in D8f step 3 could be made vacuously true by a substring match and nothing would notice |
| `skills_section_replica_line_split_is_correct` | same | `GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME` is 131 bytes with sha256 `A5C74FD6...7EC9` (§3.12 Table B, AC7.14); the live `GENERATED_SKILLS_SECTION_REPLICA_LINE` contains `room replica` and no case-insensitive `workgroup`; both end in `\n`; and the two are `!=`. The trailing-newline assertion is explicit because dropping it is the single most likely copy error and it breaks the compare silently |
| `injected_messages_recognizer_is_untouched` | `config/injected_messages.rs` | `known_default_sha256` for `context-alert` is exactly `["e672581d...153f"]`, `DEFAULT_CONTEXT_ALERT_TEMPLATE` is 125 bytes and still contains `%WORKGROUP%`, and `expand_tokens` still substitutes `TOKEN_WORKGROUP` (§3.7 family 5, AC7.12) |
| `no_clap_printed_help_carries_the_retired_token` | `cli/mod.rs` | AC5's tree walk, verbatim as written in §9.4. It needs no built binary and no process spawn, so it runs under `cargo test` in step 12 rather than needing step 13's built binary. It runs on the `rust-regression` (windows) leg, which §13.3 establishes is the only leg that runs the Rust test suite |
| `purge_args_accept_both_flag_spellings` | `cli/purge_wg.rs` | `Cli::try_parse_from` over the four combinations `{purge-wg, purge-room} x {--wg, --room}` yields the same `PurgeWgArgs.wg` (requirement F) |
| `room_subcommand_accepts_the_workgroup_alias` | `cli/workgroup.rs` | `room remove --room X` and `workgroup remove --workgroup X` parse to the same `WorkgroupRemoveArgs` |
| `default_context_for_a_room_replica_says_room` | `config/session_context.rs` | the rendered context for a `room-1-t` replica contains `` room-<N>-* ``, contains `<roomN>`, and contains no case-insensitive `workgroup` |

**Rust integration tests** in `src-tauri/tests/cli_workgroup_team.rs` (file name preserved, Rule P):

| Test | Proves |
| --- | --- |
| `room_add_creates_a_room_directory` | `room add --project P --team T --title X` creates `room-1-T` |
| `room_list_and_workgroup_list_produce_identical_stdout` | byte-identical stdout for the two spellings on the same root (requirement F) |
| `room_list_reports_a_mixed_root` | a root seeded with `wg-1-T` and `room-1-T` lists both, with both teams resolved (AC4) |
| `purge_room_and_purge_wg_produce_identical_outbox_messages` | the two spellings write outbox messages that differ only in `id`, `request_id` and `timestamp` |

**Frontend tests** (`vitest`):

| Test | Location | Proves |
| --- | --- | --- |
| `entity-prefix` unit suite | `src/shared/entity-prefix.test.ts` (new) | each of the five functions on `room-1-t`, `wg-1-t`, `ROOM-1-t`, `WG-1-t`, `roomx`, `""`; specifically that `isEntityDirName` and `pathHasEntityDirSegment` are **case-sensitive** and `entityDirNumber` / `entityShortLabel` are **case-insensitive** |
| `extractWorkgroupName` returns `ROOM-1-TEAM` | `src/shared/path-extractors.test.ts` | F1, and therefore the titlebar badge |
| rail sorts and labels a Room | `src/sidebar/components/WorkgroupGroupRail.test.tsx` | F4 returns 1 for `room-1-t` (not `MAX_SAFE_INTEGER`) and F5 returns `ROOM1`, while `wg-1-t` still returns `WG1` |
| Task buttons enable in a Room cwd | `src/terminal/components/WorkgroupTask.test.tsx` | F6 enables for `C:\P\.ac\room-1-t\__agent_x` and stays disabled for `C:\P\.ac\ROOM-1-t\__agent_x` |
| profile scope resolves a Room replica | `src/shared/profile-utils.test.ts` | F2 and F3 |

**Frontend additions beyond round 1.** `TeamContextAlertsEditor` and `TaskCleanConfirmModal` gain an expectation that their body text says Room, because both files were absent from round 1's §6.2 entirely and a file nobody listed is a file nobody tests.

**API and snapshot fixture twins.** Every edit in this group is inside a `#[cfg(test)]` module and no production line moves in any of these files. Add `room-` variants **beside** the existing `wg-` literals, never converting them, because a legacy fixture is the only way dual-prefix acceptance can be tested at all (Rule P2):

| File | Existing literals, unchanged | Where the edit is placed |
| --- | --- | --- |
| `src-tauri/src/api/identity.rs` | `:341`, `:353`, `:370` | §6.4. The file is in neither §6.1 list: all three hits are after the `#[cfg(test)]` at `:301`, so §3.14's production narrowing never sees it. It takes **no** production edit, which is what makes requirement (D)'s "zero production edits" claim asserted rather than assumed |
| `src-tauri/src/api/actuation.rs` | `:169`, `:180`, `:218`, `:236` | §6.1's edited list. The file separately takes two Rule R edits at `:52` and `:95` (§3.14); its fixtures are untouched by those |
| `src-tauri/src/pty/terminal_snapshot/acceptance_tests.rs` | `:74`, `const WORKGROUP: &str = "wg-1-dev-team"` | §6.1's **preserved** list. Authorized and named in §6.1's table of the three permitted `#[cfg(test)]` edits inside preserved files |
| `src-tauri/src/pty/terminal_snapshot/resource_tests.rs` | `:33`, `:34`, the two `"project:wg-1-team/..."` FQNs | same |

### 9.2 Existing tests that must stay green **without being edited** (negative evidence)

**Why this section is the weakest evidence in the plan, stated plainly.** Round 1's recognizer criteria were self-referential, and so is most of this suite: roughly twenty tests over `looks_like_generated_legacy_default_context` (`session_context.rs:6191`, `:6257`, `:6304`, `:6489`, `:6631`, `:6673`, `:6723`, `:6770`, `:6823`, `:6880`, `:6928`, `:6974`, `:7031`, `:7108`, `:7155`, `:7487`, `:7615`, `:7869`, `:10056`, `:10114`) build their expected value by calling `legacy_rendered_default_context_for_compat` and then classify that, so they stay green under **any** internally consistent rename. The same holds for the root tests at `root_agent.rs:2530`, `:2880`, `:2914`, `:2923`. They are kept because they still catch an inconsistent edit; they are not evidence of the freeze, and AC7's externally pinned criteria are what supply that.

- Every `#[cfg(test)]` fixture that builds a `wg-*` directory and asserts it is discovered, listed, addressed or deleted. If any of these needs an edit, dual-prefix acceptance is broken, not the test. This is the strongest single signal in the change and it covers `cli_workgroup_team.rs` (191 matching lines), `cli_behavior_contract.rs:135`, `:147`, `:741`, `:761`, `:775-776`, `:861`, `cli_loop.rs`, `cli_close_session.rs`, `cli_role_experiment.rs`, `pty_input_cross_process.rs`, `terminal_snapshot_host.rs`, and the 67 frontend `*.test.ts(x)` files.
- `test-debt.allowlist.json` must not appear in the diff (§6.7).
- `scripts/smoke-cli-*.ps1` must not appear in the diff.

### 9.3 The rule for updating an existing test expectation

An existing assertion is edited only under one of the three clauses below, and then only to the new value and nothing else. Every other assertion is untouched; if one goes red, the implementation is wrong.

**Clause 1: it pins a string this plan renames under Rule R.**

**Clause 1's list is illustrative; clause 2's is complete.** That difference is stated explicitly because §14 item 12 tells a reviewer the three clauses are the closed set of permitted test edits, and a reader can take "closed set of clauses" for "closed set of sites". Clause 2 enumerates every site because a version bump is a closed, countable set and the resize limb is closed by a measured sweep rather than by enumeration, which the paragraph below its table sets out; the list is **eight** rows after round 4 added `injected_messages.rs:1671`. Clause 1 cannot be enumerated the same way: Rule R is total over reader-facing carriers, so the red set is whatever pins one of them, and the **clause** is the gate. An assertion that goes red and satisfies clause 1 is authorized whether or not it appears below.

The sites known to go red at the time of writing, extended in round 3 from six to nineteen after both round-2 reviewers swept for more, and to **twenty** in round 4:

| Site | What it pins | Producer, where it is in another file |
| --- | --- | --- |
| `cli_behavior_contract.rs:314`, `:327`, `:331` | `--workgroup` appears in help; they become `--room` and each gains a companion assertion that the old spelling still parses | §5.8 |
| `cli_behavior_contract.rs:306-313` | subcommand help | §5.8 |
| `session_context.rs:5282`, `:5490` | the message-filename patterns | §5.9 |
| `session_context.rs:4793` | `"Narrow exception — workgroup messaging directory"`, the separator being U+2014 EM DASH (corrected in round 4) | `session_context.rs:3531` (§5.9) |
| `session_context.rs:5318` | `"<project>:<workgroup>/<agent>"` | the live renderer |
| `session_context.rs:5956`, `:8434` | `"**Workgroup**: a runtime replica of a team ..."` | the seeded templates (§5.10 D8e) |
| `session_context.rs:5992` | `"You are working inside a workgroup replica."` | the live renderer |
| `commands/session.rs:5392` | `err.contains("workgroup root")` | **`pty/container_paths.rs:293`**, a different file, renamed by §5.3. This is the only red assertion inside §6.1's preserved 16 and it is named in §6.1's table |
| `cli/workgroup.rs:545` | `"Failed to fully delete workgroup directory"` | `cli/workgroup.rs:350` (§5.8) |
| `commands/entity_creation.rs:5125` | `"Partial workgroup delete"` | `commands/entity_creation.rs:3323` (§5.8's note on partial enumerations) |
| `config/root_agent.rs:2113` | `ROOT_ROLE_MD.contains("workgroup add")` | `root_agent.rs:710`, edited by §5.10 D8e. It becomes `room add`; the pre-rename spelling is asserted against `ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD` by the new test in §9.1 |
| `tests/cli_workgroup_team.rs:374` | `"Manage teams and scoped workgroup membership"` | `cli/mod.rs:175` (§5.8) |
| `tests/cli_workgroup_team.rs:379` | `"Define a repo available to the team when workgroups are created"` | `cli/team.rs:61` (§5.8). This assertion is also the independent proof that `team create --help` prints `:61`, which is what AC5's membership assertion re-establishes by construction |
| `tests/cli_workgroup_team.rs:799` | `"Failed to delete workgroup"` | `cli/workgroup.rs` (§5.8) |
| `tests/cli_workgroup_team.rs:1337` | `"workgroup add` no longer updates team configuration"` | `cli/workgroup.rs` (§5.8) |
| `config/injected_messages.rs:1331` **(new in round 4)** | the `%WORKGROUP%` description line inside `EXPECTED_SEED`, byte-identical to its producer. It becomes `#   %WORKGROUP%   room name, e.g. room-2-dev-team` | **`config/injected_messages.rs:78`**, edited by §5.10 D8e. The assertion that goes red is `assert_eq!(written, EXPECTED_SEED)` at `:1674`, inside `missing_file_seeds_canonical_bytes` (`:1661`), where `written` is generated from `CONTEXT_ALERT_DOC_COMMENT`. See the paragraph below the table |
| every frontend expectation that pins a renamed label | | §5.12 |

**`config/injected_messages.rs:1331` is a determinate red, not a check, and round 3 was wrong to defer it.** Round 3 said its redness "depends on whether it asserts against the production constant or against its own copy". There is no depends: it is its own copy by deliberate construction, and the source says so. `:1331` sits inside `EXPECTED_SEED` (`:1305-1337`), whose doc comment at `:1301-1304` reads:

> The exact seeded bytes, transcribed from the plan rather than produced by the code under test. Comparing the written file against `canonical_seed_bytes` cannot fail, so an edit to a header constant would drift from the specification with nothing going red.

The transcription exists **precisely** so that an edit to a header constant goes red. Measured at `d7008b34`: `:1331` is byte-identical to `:78` (both 52 bytes), it occurs exactly once in `EXPECTED_SEED`, and `missing_file_seeds_canonical_bytes` asserts `assert_eq!(written, EXPECTED_SEED)` at `:1674` over a `written` the product generates from `CONTEXT_ALERT_DOC_COMMENT`, `:78` included. So §5.10 D8e's one Rule R edit at `:78` makes `:1674` red unconditionally, in every reading, and the twin edit at `:1331` is authorized by clause 1. **Both round-3 reviewers found this independently.** The same edit also resizes `EXPECTED_SEED`, which is a clause-2 site and is carried below.

**One site that looks like a clause-1 red and is not, checked so nobody edits it.** `session_context.rs:6829` reproduces `:831`'s literal verbatim inside `generated_shaped_manual_legacy_skills_content_is_preserved` (`:6804`), but its fixture carries manual skill entries that can never equal a fresh render, so the file classifies `NotLegacy` and the test asserts only that the manual markers survive. It is green in all four combinations of {renamed, not} for `:831` and `:6829`, so it is neither red nor a backstop, and it is **not** edited. Both round-3 reviewers verified this rebuttal independently and accepted it.

**Clause 2 (new in round 2): it pins a `current_version` this plan bumps, or a constant this plan's Rule R edits resize.** Round 1's rule forbade exactly these edits, and §5.10's three version bumps make all of them red on a correct implementation. **Read the second limb literally: a pin on the byte length, character count, word count or sha256 of a constant whose text Rule R changes is a clause-2 edit, exactly as a `current_version` pin is.** It is easy to miss because it pins a size, not a string, so clause 1 does not reach it. The complete set, measured, **eight rows after round 4 added `injected_messages.rs:1671`**:

| Site | Base | New |
| --- | --- | --- |
| `seeded_context_templates.rs:2058` | `assert_eq!(global.current_version, 4)` | `5` |
| `seeded_context_templates.rs:2068` | `assert_eq!(coordinator.current_version, 5)` | `6` |
| `seeded_context_templates.rs:3344` | `parsed[...]["coordinator"]["currentVersion"], 5` | `6`, and the failure message at `:3345` is reworded from "by the #1571 orchestrator rename" to "by the #1614 room rename" |
| `seeded_context_templates.rs:2055` | fn name `project_specs_bump_only_the_global_template_to_v4` | `..._to_v5` |
| `root_agent.rs:2402` | `parsed[...]["rootAgent"]["currentVersion"], 7` | `8`, message at `:2403` reworded the same way |
| `session_context.rs:6104` | `assert_eq!(workgroup_chars, 220)` | `223` (§5.10 D8b) |
| `session_context.rs:6105` | `assert_eq!(workgroup_words, 33)` | `34` (§5.10 D8b) |
| `config/injected_messages.rs:1671` **(new in round 4)** | `assert_eq!(EXPECTED_SEED.len(), 1534, "the pinned seed is 1534 bytes")` | **`1531`**, and the message becomes "the pinned seed is 1531 bytes". §5.10 D8e's Rule R edit at `:78` shortens the line from 52 bytes to 49, and its twin at `:1331` shortens `EXPECTED_SEED` by the same 3 (§9.3 clause 1). `1534 - 3 = 1531`. `:1672`'s `!contains('\r')` assertion is unaffected and is not edited |

**Why this list can be declared complete where clause 1's cannot.** A version bump is a closed, countable set of pinned integers, and the resize limb is closed by a sweep rather than by enumeration of carriers. Every constant this plan's Rule R resizes lives in `config/session_context.rs`, `config/root_agent.rs`, `config/seeded_context_templates.rs` or `config/injected_messages.rs`. Those four files were swept at `d7008b34` for every assertion pinning a `.len()`, a `chars().count()` or a word count of a **named constant**, which returns **nine** sites; the table's other five rows are `current_version` pins, which no size sweep returns.

- `session_context.rs:6104` and `:6105` are already in the table.
- `session_context.rs:6106-6107` are ceilings, not pins, and are excluded below.
- `session_context.rs:6161` (598) and `:6169` (570) pin `LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072` and `LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072`, which stay frozen and are not resized.
- `injected_messages.rs:1484`/`:1485` pin `DEFAULT_CONTEXT_ALERT_TEMPLATE` at 125. Its only retired-token carrier is the `%WORKGROUP%` placeholder, which §3.10 preserves under Rule P, so it is not resized.
- `injected_messages.rs:1671` is the ninth and is the row added above.

Every other `.len()` hit in those four files pins a collection length or a runtime value rather than a constant. **Both round-3 reviewers ran this sweep independently and both returned `:1671` as the one and only addition.** Digest pins over template snapshots are a different mechanism and are governed by §5.10 D8a's version bumps and §3.10's preserved machine values, not by this clause.

`test-debt.allowlist.json` was checked for all of these: it pins none of the four test names involved (`project_specs_bump_only_the_global_template_to_v4`, `root_context_pre_orchestrator_rename_snapshot_is_byte_exact`, `stage_e_git_scope_constants_are_distinct_and_location_specific`, `git_scope_copy_is_location_correct_and_compact` all return 0 occurrences at `d7008b34`), so renaming the one function name does not disturb §6.7's preserve set.

The two `assert!` ceilings at `session_context.rs:6106-6107` are **not** in this clause and are not edited. They are a compactness budget, not a pin on this rename.

**Clause 3 (new in round 2): it asserts a prefix predicate this plan widens.** One site: `entity_creation.rs:6943-6946` asserts `!temp_name.starts_with("wg-")` with the message "temp name must NOT match the wg- discovery filter (would surface as ghost workgroup)". After this change the `.deleting-*` sentinel must fail the **widened** filter, so the assertion becomes `!crate::config::entity_prefix::has_entity_prefix(temp_name)` and its message is reworded. Leaving it as written would let a `room-`-shaped sentinel pass, which is the exact defect the assertion exists to prevent. Round 1 listed this site in the known-red set but under clause 1, which does not apply: it pins no string.

**What is still forbidden.** An assertion that pins a directory name, an identifier, a serde or JSON key, a `data-ac-testid`, an event name or a wire value is never edited. In particular `PURGE_WG_ACTION`'s value, the `"workgroupCreated"` / `"workgroupRemoved"` reason codes, the collapse keys and `%WORKGROUP%` are all pinned by tests that must stay green untouched, and §9.2's negative evidence covers them.

### 9.4 Objective acceptance criteria

**AC1, visible text: total sweep plus a committed allowlist, zero unlisted lines.**

Round 1 offered 36 fixed-string needles. A needle set can only re-find what the enumeration that produced it already found, so it cannot detect an enumeration miss, and `git grep -F` is case-sensitive, which is how `Delete workgroup` passed under a `Delete Workgroup` needle. **The needles are deleted.** They are replaced by a criterion whose failure state is "a line nobody classified" rather than "a needle nobody wrote".

**The allowlist is derived at the base commit and committed before the first visible-text edit.** Round 2 generated it at step 10b **from the post-change sweep output**, and the gate was "every returned line has its pair in the allowlist". `sweep > allowlist` satisfies that mechanically and unconditionally, so for *this* change the only thing between the gate and a vacuous pass was clause 3, a human justification. The prospective value is real, a future regression does turn the gate red, and that value is kept. What is fixed is the direction of derivation: an unrenamed Rule R line must surface as a line **nobody listed**, not as a row the implementer wrote after seeing it.

1. **Part A, frozen at the base.** At `d7008b34`, before any product edit, run the three sweeps below and subtract the Rule R lines this plan is going to move. The remainder is `scripts/room-rename-allowlist.tsv`, committed at step 0b, **before step 9**. Each row is `<class>\t<path>\t<trimmed line content>`, where `<class>` is one of the Rule P clause names: `P0-identifier`, `P0-css`, `P0-testid`, `P0-key`, `P0-event`, `P0-wire`, `P0-token`, `P1-comment`, `P2-fixture`, `P3-frozen`, `P4-log`. Content, not line number, is the key, so the file survives line drift.

2. **The three sweeps, binding, and identical in alternation.**

```
# frontend
git grep -nE '[Ww]orkgroup|WORKGROUP|(^|[^A-Za-z0-9_])([Ww][Gg])([^A-Za-z0-9_]|$)|(^|[^A-Za-z0-9_])wg-' \
  -- src | grep -E '^src/[^:]*\.tsx?:' | grep -vE '^src/[^:]*\.test\.tsx?:'
# rust
git grep -nE '[Ww]orkgroup|WORKGROUP|(^|[^A-Za-z0-9_])([Ww][Gg])([^A-Za-z0-9_]|$)|(^|[^A-Za-z0-9_])wg-' \
  -- src-tauri/src
# docs and root markdown
git grep -nE '[Ww]orkgroup|WORKGROUP|(^|[^A-Za-z0-9_])([Ww][Gg])([^A-Za-z0-9_]|$)|(^|[^A-Za-z0-9_])wg-' \
  -- docs README.md ROADMAP.md PRIVACY.md src-tauri/src/api/README.md ':!docs/assets'
```

**Two corrections to the docs command, both round 3.** Round 2's third command matched `WG` upper-case only while its frontend and Rust twins matched `([Ww][Gg])`, with no stated reason for the difference. Measured at `d7008b34` over that path set: the upper-case form returns **548** lines, the unified form returns **563**, and all **15** differences are real. Nine of the fifteen are Rule R lines this plan moves, and round 2's gate could not see any of them: `docs/agents/inter-agent-messaging.md:26` and `docs/concepts.md:81` (the message-filename convention §5.9 changes, where round 2 named only `docs/agent-matrix-conventions.md:548`), `docs/reference/architecture.md:735` and `docs/reference/cli.md:29`, `:30`, `:31`, `:703`, `:708` (bare `purge-wg`, the command §5.8 renames), and `docs/features/session-auto-close.md:17` (the `` `<project>:<wg>` `` team key). All nine are now in §5.9's or §5.13's edit tables. The remaining six are Rule P and land on the allowlist: `docs/testing/destructive-filesystem-regression.md:242`, `:246`, `:259` (`$Wg`, a PowerShell variable, `P0-identifier`) and `docs/testing/semantic-ui-automation-affordance-matrix.md:65`, `:71`, `:72` (`<wg>` inside `data-ac-testid` patterns, `P0-testid`). Second, `':!docs/assets'` is now **in the command**. Round 2's prose claimed `docs/assets/` was "excluded from all three by pathspec" and no command excluded it; it contributes 14 lines including a binary (`docs/assets/og-card.png`), and `docs/assets/og-card.svg` is in §6.7 Part 1.

3. **The gate, run on the post-change tree.** Every line every one of the three commands returns must have its `(path, trimmed content)` pair present in the allowlist. **The count of returned lines whose pair is absent must be exactly zero.** `plans/` and `CHANGELOG.md` are outside all three path sets and `docs/assets/` is now excluded by pathspec.

4. **Part B, the additions, and why they exist.** Some lines carrying the retired token are *introduced* by this change and therefore cannot be in a base-derived list: the dual-prefix helpers (`config/entity_prefix.rs` and `shared/entity-prefix.ts` must both name `"wg-"`), the frozen `*_BEFORE_ROOM_RENAME` copies of §5.10, the `#[cfg(test)]` fixture twins of §9.1, and the compatibility sentences §5.13 adds to the documentation. These are appended at step 10b as **Part B**, each with a class and a one-line justification naming the plan section that requires it. Part B is expected to be short and every row of it is a deliberate decision; Part A is expected to be long and every row of it is a pre-existing fact. Keeping them apart is what makes the second reviewable at all.

5. **Each row must carry a class**, and a row whose class cannot be justified from §5.2 is a Rule R miss, not an allowlist entry.

6. **What this gate proves, stated exactly, because round 2 claimed more than it delivers.** "Zero by allowlist" proves that **every surviving occurrence is listed and visible in review**, and, now that Part A is frozen at the base, that **no line this plan was supposed to move is still there** (an unrenamed Rule R line is not in Part A, so it comes back unlisted). It does **not** prove that each Part A row is *correctly classified*: that is clause 5, a discipline statement backed by review, and AC1b's ten pinned strings cover part of the gap. Nobody should over-rely on it beyond that.

7. **How much may be machine-classified, and what a human reads.** Part A is large and the split is stated so the implementer does not either hand-audit 4,000 rows or rubber-stamp them. Machine-classifiable, by a script committed beside the allowlist: `P0-css` (the line's match is inside a `class`/`className` value or a `.ac-wg-`/`workgroup-group-` selector), `P0-testid` (inside a `data-ac-testid` value), `P0-key` (inside a `serde` attribute or a quoted JSON key), `P1-comment` (the trimmed line starts with `//`, `///` or `//!` and the file is Rust), `P4-log` (the line is inside a `log::` macro invocation), and `P0-identifier` where the only match is a Rust or TypeScript identifier with no adjacent quote. **Read by hand, every row:** `P0-wire`, `P0-event`, `P0-token`, `P2-fixture` and `P3-frozen`. Those five are the classes whose misclassification is a compatibility break rather than a cosmetic miss, they are the classes §3.10 and §5.2 P3 enumerate, and together they are a small fraction of the file. The machine classifier's own output is spot-checked against §3.10's table.

8. **Baseline arithmetic, so a reviewer has numbers rather than a procedure.** Measured at `d7008b34`:

**What gets subtracted is every base line this plan moves, which is more than the Rule R set.** A line that is not Rule R can still have its content changed by this plan, and content is the allowlist key, so leaving such a line in Part A would make it come back unlisted and turn the gate red on a correct implementation. For the frontend the subtraction is fully enumerated, and all 63 lines were verified present in the base sweep with no duplicates:

| Group | Lines | Why it moves |
| --- | --- | --- |
| §3.8's four visible-text classes | **55** | Rule R. 24 + 6 in (a), 5 in (b), 18 in (c), 2 in (d) |
| §3.8's R1 resolvers, `ProjectPanel.tsx:1057`, `:1058`, `:1071` | **3** | not visible text, but §5.2 clause R1 moves them in the same commit as `:993`/`:995` or the sidebar filter silently stops matching the row it names. §5.2 states this is the only R1 instance |
| §5.4's prefix predicates not already counted: `profile-utils.ts:124`, `:472`, `WorkgroupGroupRail.tsx:67`, `:72`, `WorkgroupTask.tsx:74` | **5** | D2 replaces each with a call to the shared helper, so the `` /^wg-/ `` literal leaves the line. F1 (`path-extractors.ts:25`) and F5's label (`:73`) are already inside the 55 |
| **frontend total moved** | **63** | |

| Surface | Base sweep | Lines this plan moves | Part A rows |
| --- | --- | --- | --- |
| frontend | **906** lines / 41 files (§3.8) | **63**, enumerated above | **843** |
| Rust | **3090** lines / 94 files (§3.14) | enumerated at step 0b | recorded in the PR body |
| docs and root markdown | **563** lines / 64 files | enumerated at step 0b, and at least the 9 named in point 2 | recorded in the PR body |
| **total** | **4559** | | |

The closing check a reviewer runs without redoing the classification: `rows(Part A) + lines moved = 4559`, with the three per-surface subtotals stated in the PR body. The frontend row is fully determined here and is the worked example.

9. **Backstops on the post-change counts, tightened.** Round 2 said only "the post-change frontend count must be lower" and "a count equal to 906 is a finding", which together admit any value in 1..905. The runnable form: **the post-change frontend sweep returns exactly `843 + <Part B frontend rows>` lines**, a number the implementer writes into the PR body before running the gate and then compares against. It is exact because Part A's 843 rows are lines whose content this change does not touch, which is what makes the enumeration above a subtraction rather than an estimate: any base line the change *does* touch is in the 63 and is therefore not in Part A. Additionally, each of the 63 must be absent from the post-change output, checked individually. A post-change count equal to 906, or equal to 843 when Part B has frontend rows, is a finding to report.

**AC1b, the seven round-1 misses, named.** Independently of the allowlist, the post-change tree must contain none of the following, matched case-insensitively so a case variant cannot pass: `Delete workgroup`, `the workgroup is locked`, `associated workgroups`, `its workgroup replicas`, `same-workgroup Orchestrators`, `workgroup replicas before minting`, `the current workgroup;`, `every workgroup of this team`, `the workgroup TASK.md`, `workgroup(s)`. This is a regression pin on the specific defects round 1 shipped, not a substitute for AC1.

**AC2, no `wg-*` producer.** `git grep -n 'format!("wg-' -- src-tauri/src` returns exactly two lines, `phone/mailbox.rs` and `screenshot/windows.rs`, and both are inside a `#[cfg(test)]` module (Rule P2). `git grep -n 'ROOM_DIR_PREFIX' -- src-tauri/src/commands/entity_creation.rs src-tauri/src/cli/role_experiment.rs` returns the three production construction sites. No other production expression concatenates an entity prefix.

**AC3, independent numbering.** `room_allocator_ignores_legacy_workgroup_directories` and `create_room_on_disk_uses_room_prefix` are green: in a `.ac` root holding `wg-1-<team>`, creation produces `room-1-<team>`.

**AC4, mixed root.** `room_list_reports_a_mixed_root` is green, and manually: a root seeded with `wg-1-t` and `room-1-t`, each with one `__agent_x` replica, produces two peers from `list-peers-lean` whose `name` fields are `<proj>:wg-1-t/x` and `<proj>:room-1-t/x`, and `same_entity_rule_denies_cross_prefix_pairs` is green.

**AC5, no retired token in printed CLI help, complete by construction.**

Round 1 enumerated 20 subcommands by hand and the enumeration was missing `team create` (the only command that prints `team.rs:61`) and `role-experiment` (the only command that prints the `--retain-room` placeholder). An enumeration cannot be the gate for a defect class whose failure mode is omission. The criterion is therefore a Rust test that walks `clap`'s own command tree:

```rust
#[test]
fn no_clap_printed_help_carries_the_retired_token() {
    use clap::CommandFactory;
    let mut root = Cli::command();
    root.build();
    // The root command name is the package name, `agentscommander-new`, because
    // `Cli` sets no `#[command(name = ...)]`. Take it before the walk consumes
    // `root`, and derive every lookup key from it rather than hard-coding it.
    let root_name = root.get_name().to_string();
    // Carry the invocation path, so an assertion can name a leaf unambiguously
    // ("create" alone is a name three different parents use).
    let mut stack = vec![(String::new(), root)];
    let mut walked: std::collections::BTreeMap<String, String> = Default::default();
    while let Some((prefix, mut cmd)) = stack.pop() {
        let path = if prefix.is_empty() {
            cmd.get_name().to_string()
        } else {
            format!("{prefix} {}", cmd.get_name())
        };
        for sub in cmd.get_subcommands().cloned() {
            stack.push((path.clone(), sub));
        }
        let help = cmd.render_long_help().to_string();
        for needle in ["workgroup", "Workgroup", "WORKGROUP"] {
            assert!(!help.contains(needle), "{path}: {needle}");
        }
        for token in help.split(|c: char| !c.is_ascii_alphanumeric()) {
            assert_ne!(token, "WG", "{path}");
        }
        walked.insert(path, help);
    }

    // The real guard: the two omissions that caused G5 must be reachable, and
    // the depth-2 one must have been rendered, not merely listed by its parent.
    let team_create = walked
        .get(&format!("{root_name} team create"))
        .expect("`<root> team create` is not in the walked set");
    assert!(
        team_create.contains("Define a repo available to the team when rooms are created"),
        "team create help did not render team.rs:61"
    );
    let role_exp = walked
        .get(&format!("{root_name} role-experiment"))
        .expect("`<root> role-experiment` is not in the walked set");
    assert!(
        role_exp.contains("--retain-room <RETAIN_ROOM>"),
        "role-experiment help did not render the corrected value placeholder"
    );
    assert!(
        walked.contains_key(&format!("{root_name} role-experiment variant set")),
        "`<root> role-experiment variant set` is not in the walked set"
    );

    // Vacuity floor, derived in the prose below, not a round number.
    let expected_min = if cfg!(target_os = "windows") { 75 } else { 73 };
    assert!(
        walked.len() >= expected_min,
        "walked {} commands, expected at least {expected_min}",
        walked.len()
    );
}
```

Six properties make this complete where the enumeration was not:

- `get_subcommands()` returns hidden subcommands too, so `role-experiment` is walked; `hide = true` suppresses listing, not membership.
- the recursion has no hand-written list, so a subcommand added later is covered without editing the test.
- `render_long_help()` renders `about`, `long_about`, `after_help`, every `help = ...` and every value placeholder, so `team.rs:61` and `--retain-room <RETAIN_ROOM>` are both in scope.
- **the three membership assertions are the guard, and they are new in round 3.** `team create` is a **depth-2 leaf** and is the only command that prints `team.rs:61`; `role-experiment` is hidden and is the only command that prints the corrected placeholder; `role-experiment variant set` is a depth-3 leaf. Asserting that each was walked, and that the first two actually **rendered** the strings whose omission caused G5, makes a depth truncation red rather than green. A `!contains` assertion over a set that was never populated is vacuously true, which is why membership has to be asserted separately from absence.
- **the lookup keys are derived from `root.get_name()`, not hard-coded, and this is corrected in round 4.** Every key in `walked` begins with the root command's own name. `Cli` (`cli/mod.rs:76-96`) carries `#[derive(Parser)]`, `#[command(about = ...)]` and `#[command(after_help = ...)]` and **no `#[command(name = ...)]`**, so clap derives the root name from `CARGO_PKG_NAME`. `src-tauri/Cargo.toml:2` is `name = "agentscommander-new"`, there is no `[[bin]]` section and no name override, and the repository's own integration tests reach the binary through `env!("CARGO_BIN_EXE_agentscommander-new")` (`tests/cli_behavior_contract.rs:30`). The root name is therefore `agentscommander-new`, not `agentscommander`. Round 3 wrote the three lookups as `walked.get("agentscommander team create")` and so on; all three would have returned `None` and the test would have panicked on a **correct** implementation, with failure messages blaming a traversal depth that was in fact reached. Deriving the prefix removes the literal and stays correct if a `#[command(name)]` is ever added. `ac-dev-rust-v3` found this in round 3 and settled it with an offline probe crate on `clap` 4.6, the same route §5.8 used for its five clap facts; the same probe confirmed that the traversal is otherwise exactly right (`team create` at depth 2 is walked and its long help carries the `help` string, `role-experiment` is `hide = true` and is walked and its help carries `--retain-room <RETAIN_ROOM>`, the hidden `retain-workgroup` alias does not appear in help, and `role-experiment variant set` at depth 3 is walked).
- **the floor is derived, and the derivation is stated so a reviewer can check it rather than trust it.** Measured at `d7008b34`: `Commands` (`cli/mod.rs:130-230`) has **39** variants, 37 unconditional plus `WindowList` and `WindowScreenshot` behind `#[cfg(target_os = "windows")]`. Nine nested `#[derive(Subcommand)]` enums add **35**: `agency_templates` 3, `api_client` 3, `coding_agent` 6, `injected_messages` 1, `loop_cmd` 6, `role_experiment` 7, `role_experiment::VariantCommand` 2, `team` 4, `workgroup` 3. With the root itself that is **75** nodes on Windows and 73 elsewhere. `clap` additionally injects synthetic `help` subcommands, which can only raise the count, so `>=` is safe in both directions. **State the effect plainly: the synthetic nodes mirror the sibling tree and recurse** (`help team create`, `role-experiment variant help set`, `help help`), and in the round-3 probe 8 real commands plus the root produced **27** walked keys. The recursion terminates, so the floor stays safe, but `walked.len()` on the real tree will be far above 75. The floor is therefore a vacuity guard and nothing more; the three membership assertions are the criterion.

**Round 2's `checked >= 25` is deleted, and the reason is worth stating because it is the same defect class as round 1's needle set.** A traversal that walked only the root and its direct children reports 40 and passes `>= 25`. `team create` is a depth-2 leaf, so that exact partial traversal is the omission round 1 shipped: the floor guarded "walked nothing" and the failure was "walked one level". Both round-2 reviewers found this independently and both proposed the membership form. The floor is kept only as a vacuity backstop; the membership assertions are the criterion.

`Cli` is `cli::Cli` and the test lives in `cli/mod.rs`'s `#[cfg(test)]` module. It needs no built binary and no process spawn, so it runs under `cargo test` in step 12 rather than needing step 13's built binary. **It runs on the `rust-regression` (windows) leg only, because §13.3 establishes that is the only leg that runs the Rust test suite.** Round 2 said "it runs on every leg, not only the Windows one", which contradicts §3.11's job table and §13.3; the decision to walk in-process is unaffected and stands on needing no binary and no spawn, but the stated reason was wrong.

**AC6, alias equivalence.** Against a debug binary built from the branch head, all of these produce the identical operational result, and the two `--help` invocations both print `purge-room` in their usage line:

```
<bin> purge-wg   --wg   <name> ...
<bin> purge-room --room <name> ...
<bin> purge-wg   --room <name> ...
<bin> purge-room --wg   <name> ...
<bin> purge-wg   --help
<bin> purge-room --help
```

plus `room list` and `workgroup list` producing byte-identical stdout on the same root, and `--room`'s help line reading `--room <ROOM>`, never `--room <WG>`.

**AC7, frozen recognizers, pinned externally.**

Round 1's recognizer criteria were self-referential: every one of them built its expected value by calling the function it then classified, so a coordinated rename that moved both sides went green. Every criterion below is pinned to a literal captured at `d7008b34` and written into this plan (§3.12), except the one Table C names, which is captured at step 0 before the first edit.

| # | Criterion | Expected |
| --- | --- | --- |
| 7.1 | `GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len()` and its sha256 | 539, `F4406596...316A` |
| 7.2 | `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len()` and its sha256 | 2516, `0B89EB38...198E` (see §3.12's note if the length differs) |
| 7.3 | `ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD.len()` and its sha256 | 2467, `7F82F28C...C52D` |
| 7.4 | `WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME.len()` and its sha256 | 220, `A386B52D...566D` |
| 7.5 | `ROOT_COORDINATION_MESSAGING_PARAGRAPH_BEFORE_ROOM_RENAME.len()` and its sha256 | 897, `FC2164A2...F207` |
| 7.6 | `OLD_DEFERRED_MESSAGING_PARAGRAPH.len()` and its sha256, unchanged | 293, `6E12E68E...A463` |
| 7.7 | `legacy_rendered_default_context_for_compat(<fixed inputs>)` length and sha256 | the step-0 capture (§3.12 Table C) |
| 7.8 | source-side freeze: `git cat-file blob HEAD:src-tauri/src/config/session_context.rs`, extract from the line matching `^fn legacy_rendered_default_context_for_generation($` through the next line matching `^}$`, replace the single token `WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME` with `WORKGROUP_GIT_SCOPE`, sha256 | `940FA357...16C2` (§3.12 Table A). The one-token substitution is the D8b identifier change and is the **only** difference this criterion tolerates. |
| 7.9 | each new snapshot is accepted by every recognizer it is wired into, and is `!=` the current default | green |
| 7.10 | `ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD` is accepted on **both** root paths | `is_known_generated_root_context_template(...)` is true **and** a pristine `Role.md` of those bytes reduces to `MINIMAL_ROOT_ROLE_MD` (§3.7 family 2) |
| 7.11 | version bumps | `global` 5, `coordinator` 6, `rootAgent` 8, asserted at both the spec and the persisted-state layer |
| 7.12 | the injected-messages recognizer is untouched | `known_default_sha256` for `context-alert` is still exactly `["e672581d47e7e4a4749b510f23eff72982ff3fa5261109122b3bdf8fdfda153f"]`, and `DEFAULT_CONTEXT_ALERT_TEMPLATE` is still 125 bytes |
| 7.13 | `default_context_for_a_room_replica_says_room` | the rendered live context for a `room-1-t` replica **whose matrix root has a real `skills/` directory**, so `render_skills_section` takes the `(Some(_), Some(_))` arm and `:831` is actually rendered, contains `room-<N>-*` and `<roomN>` and no case-insensitive `workgroup`. The skills directory is required: without it the arm at `:840` renders and the criterion passes without ever exercising the line D8f moves |
| 7.14 | `GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME.len()` and its sha256 | 131, `A5C74FD6...7EC9` (§3.12 Table B), and it ends in `\n` |
| 7.15 | the D8f behavioral pin | a `Context.AgentsCommander.md` whose embedded skills section carries the pre-rename replica line still classifies `StaleGenerated` and heals **after** the whole change, and one mutated byte inside that line classifies `NotLegacy` and is preserved (§9.1's two tests). Neither builds its fixture from the constant it is testing |

7.8 is the criterion round 1 most needed and did not have: it is a byte comparison against a value taken from the frozen base and written into this document, so it cannot move with the code.

**AC8, parent-repo exclusion.** `ensure_ac_root_gitignore_creates_both_patterns` and `ensure_ac_root_gitignore_appends_room_to_a_legacy_only_file` are green, and `git grep -n 'room-\*/' -- src-tauri/src/commands/ac_discovery.rs .gitignore` returns the two expected lines.

**AC9, article agreement is empty, stated as an absolute zero.** `git grep -inE '\ban (room|rooms)\b' -- src docs src-tauri/src '*.md' ':!plans' ':!CHANGELOG.md'` returns **zero** matches. Round 1 phrased this as "zero matches introduced by this change", which is not a runnable count; the absolute form is runnable and is correct, because `an room` is ungrammatical in every context, so the expected value is zero at base as well as after. Verified zero at `d7008b34`.

**AC10, scope, in two parts matching §6.7.**

1. **Part 1 is a path test.** `git diff --name-only origin/main...HEAD` contains no path from §6.7 Part 1.
2. **Part 2 is a production-line test, not a path test.** For each §6.7 Part 2 path that does appear in the diff, `git diff -U0 origin/main...HEAD -- <path>` must show changed hunks **only** inside a `#[cfg(test)]` item, and the path must be one of the three named in §6.1's table (`commands/session.rs`, `pty/terminal_snapshot/acceptance_tests.rs`, `pty/terminal_snapshot/resource_tests.rs`) or a `*.test.ts(x)` file. A changed **production** line in a Part 2 path blocks; the path's mere presence does not.
3. No rename: `git diff --find-renames --name-status origin/main...HEAD` shows no `R` line.
4. Exactly three new non-test files: `src-tauri/src/config/entity_prefix.rs`, `src/shared/entity-prefix.ts`, `scripts/room-rename-allowlist.tsv`, plus the new plan file and the new test files.

Round 2 stated clause 2 as "contains no path from §6.7's preserve set", which forbids a diff a correct implementation must produce. §6.1 gives the reasoning and names the three files.

**AC11, dependency cycle.** §11.5's criterion, run on a clean tree at base and at branch head.

**AC12, exact-head CI.** Every triggered check on the PR head SHA is green (§13.4).

**AC13, local frontend gates.** `npm run typecheck` exits 0, `npm test` passes with no new known-debt entry, and `npm run check:frontend-dependencies` exits 0.

---

## 10. Explicit decisions and accepted residuals

### 10.1 Decisions

- **D1. One shared helper per language, not per-site edits.** 40 Rust sites and 6 TypeScript sites express one predicate. Per-site edits would reproduce the `&name_str[3..]` bug twice and would drift at the next touch. Cost: two new leaf modules and 18 new module arcs, all into sinks (§11).
- **D2. The Rust helper lives in a new `config::entity_prefix`, not in `config::ac_root`.** `ac_root` already has a gate of its own and is not in the base cyclic SCC; putting shared predicates there would give 17 unrelated modules a reason to depend on AC-root resolution. A dependency-free sink is the smaller commitment.
- **D3. The frontend helper lives in a new `src/shared/entity-prefix.ts`, not in `src/shared/constants.ts`.** `constants.ts` reads `window.location.search` at module load; `profile-utils.ts` runs in Node-side vitest.
- **D4. Five frontend functions, not one.** Each call site's case sensitivity is preserved exactly. `WorkgroupTask.tsx:70` documents why its gate must stay case-sensitive.
- **D5. `determine_next_wg_number` becomes Room-only, not dual.** This is decision 4 of the assignment, taken verbatim. It is the single `strip_prefix` site that does **not** become dual, and §5.3 flags it as the exception so a mechanical sweep cannot swallow it.
- **D6. `role-experiment`'s allocator scans both prefixes while its constructor emits `room-`.** Its allocator is `max + 1`, not lowest-free, so scanning both is strictly collision-safer, and decision 4 governs only the product allocator. Its constructor must move because acceptance criterion (A) is unqualified.
- **D7. The visible short label follows the real directory prefix.** `WG1` for a legacy Workgroup, `ROOM1` for a Room (§5.4). Uniform relabelling was rejected because it makes `wg-1-team` and `room-1-team` indistinguishable in exactly the mixed root this issue creates. Round 1 flagged this as the decision most likely to draw pushback; both reviewers agreed with it and neither challenged it, so it is settled.
- **D8. The message-filename short token follows the real prefix too.** `room7-architect` for a Room, `wg7-architect` unchanged for a legacy Workgroup. `validate_filename_shape` needs no change, which was measured, not assumed (§5.9).
- **D9. Deprecated CLI aliases are hidden, not `visible_alias`.** Advertising the retired spelling in help contradicts requirement (G). The aliases still parse.
- **D10. "Behaviourally identical" excludes the rendered name in help and usage.** `clap` renders the canonical name for both spellings; that is measured (§5.8 fact 4) and is the correct behavior for a deprecated alias.
- **D11. Every renamed flag that takes a value carries an explicit `value_name`.** Without it `clap` derives the placeholder from the **field** name, which identifiers-out-of-scope leaves as `wg` / `workgroup` / `retain_workgroup`, and help would print `--room <WG>` or `--retain-room <RETAIN_WORKGROUP>`. Measured (§5.8 fact 5). All ten declaration rows apply it, including `role_experiment.rs:95`, which round 1's table omitted.
- **D12. Three frozen seeded snapshots and three version bumps, plus three dual-use splits and two in-place freezes.** Round 1 said "three snapshots" and stopped there, which understated the work by four items; round 2 found four of the five and missed the fifth. The full set is §5.10: three snapshots (D8a), the `WORKGROUP_GIT_SCOPE` split (D8b), the `ROOT_COORDINATION_MESSAGING_PARAGRAPH` split (D8c), `OLD_DEFERRED_MESSAGING_PARAGRAPH` frozen in place (D8d), `legacy_rendered_default_context_for_generation` frozen in place (§5.2 P3), and, added in round 3, the `render_skills_section` replica-line split (D8f).
- **D13. `PURGE_WG_ACTION` stays `"purge-wg"`.** Requirement (F) says nothing in flight may break, and this value crosses a process boundary between two independently-updated components. The **prose** that quotes the command name does move, to `purge-room` (§5.8).
- **D14. Machine-readable CLI stdout keys stay.** `"workgroup"` in `room list` / `team list` JSON is a script contract. Renaming it is not a visible-text change, it is an API break, and the issue puts persisted keys out of scope.
- **D15. Documentation file names stay.** `test-debt.allowlist.json` proves the repository pins paths by name; #1615 owns the moves.
- **D16. The `room-*/` gitignore entry is mandatory, not optional.** It is the one item in this plan that neither the issue nor the assignment names, and without it every Room created inside a git-tracked project is corrupted by the next parent `git checkout` (§3.6).
- **D17 (new). `%WORKGROUP%` stays.** It is a placeholder token that appears in every user-edited `injected-messages.toml` template on disk; renaming it silently stops expansion in each one, and `DEFAULT_CONTEXT_ALERT_TEMPLATE`'s bytes are pinned by a shipped-defaults hash (§3.7 family 5). Only the token's human description at `injected_messages.rs:78` and in `docs/features/context-tracking.md` moves.
- **D18 (new). `log::` message text is Rule P.** It is developer diagnostics keyed by bracketed target tags operators grep for, not a product surface, and requirement (G)'s carrier list does not include it. Stated as a rule (§5.2 P4) rather than decided per site, because round 1 renamed the log lines it happened to enumerate and left the rest, which is a boundary a reviewer cannot check.
- **D19 (new). Ordinary `///` and `//!` doc comments are Rule P.** Same reason: a rule beats a list. The only exception is a doc comment that would contradict its own code after this change, and that set is closed at six sites (§3.14).
- **D20 (new). The visible-text gate is a total sweep plus a committed allowlist, not a needle set.** Round 1's 36 needles went green with seven production Workgroup strings still shipping. A needle set can only re-find what its own enumeration found; an allowlist makes every survivor visible and finite, and turns a future regression red until someone writes a row a reviewer can see (§9.4 AC1).
- **D21 (new). AC5 walks `clap`'s command tree instead of enumerating subcommands.** Round 1's 20-item list was missing the only two commands that print the two defects round 1 shipped. An enumeration cannot gate a defect class whose failure mode is omission (§9.4 AC5).

### 10.2 Accepted residuals, each owned

- **R1.** A mixed root can hold `wg-1-t` and `room-1-t`, two entities whose slot number is `1`. Owned by decision 4; resolved permanently when #1615 retires `wg-*`.
- **R2.** An existing `.ac/.gitignore` keeps its old comment above `wg-*/` and above `.deleting-*/`, because the presence test compares only the pattern line. Cosmetic, in a user-owned file. Owned by #1615.
- **R3.** `docs/agents/teams-and-workgroups.md` and `docs/testing/04-team-and-workgroup-lifecycle.md` keep their file names while their content says Room. Owned by #1615.
- **R4.** A saved sidebar group regex written as `^wg-` matches no Room. User data is not migrated. Documented in `docs/features/sidebar-guide.md`.
- **R5.** `ensure_ac_root_gitignore`'s failure is fail-soft at both creation sites (`log::warn!` then continue). This plan does not tighten it, because doing so is a behavior change outside this issue. Owned by a separate issue; §7.15 records it.
- **R6.** Rust and TypeScript identifiers, struct fields, source file names and `wg_`-prefixed locals keep the retired word. Owned by #1615, and this is the issue's own scope decision.
- **R7.** `repo-personal` (37 files / 954 lines) and `repo-agentscommander_webpage` (13 files / 67 lines) still say Workgroup. Neither parses an entity directory name, so neither blocks this issue. No hard blocker was found in either.
- **R8.** A downgrade to a pre-#1614 binary does not see `room-*` directories. Not a supported path.
- **R9.** The `.deleting-*` sentinel prefix is unchanged.
- **R10 (new).** `WORKGROUP_GIT_SCOPE`'s new text sits at exactly the word ceiling the existing compactness test enforces (34 of 34, §5.10 D8b). There is zero word headroom. Any later edit to that constant must re-run the budget or the test goes red. Owned by whoever next edits it; the budget is stated in §5.10 so the failure is self-explaining.
- **R11 (new).** A Room replica's PTY notification body is up to 8 bytes longer than a legacy Workgroup replica's against the 1024-byte `PTY_SAFE_MAX` budget (§7.6). Harmless at typical lengths; unowned because it needs no action.
- **R12 (new).** An existing `injected-messages.toml` keeps the pre-rename description of `%WORKGROUP%` in its header comment until the entry is next reseeded, because only `template` is hashed and refreshed. Same class as R2, same reason: it is a user-owned file and rewriting it for cosmetics is out of scope.
- **R13 (new in round 3, renumbered from a duplicate R12 in round 4).** `is_provably_generated_legacy_skills_section` now carries **two** byte-pinned legacy constants and two swaps: #1005's intro and #1614's replica line (§5.10 D8f). Every future change to any literal in `render_skills_section` needs a third, and the compare grows one constant per rename forever. The comment at `:4194-4200` states the obligation and the two behavioral tests of AC7.15 make a missing swap red rather than silent, so the failure is loud; the growth itself is accepted because the alternative is losing #664 healing for every installation. Owned by whoever next edits `render_skills_section`.

---

## 11. Dependency-cycle and layering statement

### 11.1 Measured baseline (clean tree at `d7008b34`)

`rust-module-dependency-cycles 1.1.0`, exit **1** (the normal outcome when gating cycles exist; the graph is still written, only exit 3 means no graph):

- 219 files scanned, 191 modules, 3741 module edges;
- `summary.moduleCycles` = **1**, one cyclic SCC of **85 modules**;
- `functionCyclesCrossModule` = 0;
- `src-tauri/module-arcs.txt` = 1037 arcs, 82,149 bytes, and regenerating it from the base graph is **byte-identical** (`cmp` clean).

### 11.2 New arcs this plan adds, enumerated

Every new Rust module-to-module arc has the same target, the new module `agentscommander_lib::config::entity_prefix`. The sources are exactly the **18** modules that hold at least one of the 40 gate lines of §3.2:

`::cli::list_peers`, `::cli::role_experiment`, `::commands::ac_discovery`, `::commands::config`, `::commands::entity_creation`, `::commands::task`, `::config::ac_root`, `::config::coding_agent_profiles`, `::config::loops`, `::config::placeholders`, `::config::replica_identity`, `::config::teams`, `::phone::mailbox`, `::phone::messaging`, `::pty::container_paths`, `::pty::container_repos`, `::screenshot::windows`, `::session::session`.

That is **18 new arcs and zero removed arcs**.

**Correction to round 1.** Round 1 counted 19, adding `agentscommander_lib::config` as a nineteenth source "the `pub mod` line", and then contradicted itself one sentence later by arguing that a `pub mod` line "adds no `crate::` or `super::` token". The second sentence is the correct one, and it is measurable: on the committed `src-tauri/module-arcs.txt` at `d7008b34`, `agentscommander_lib::config` appears as a source in **zero** arcs while `src/config/mod.rs` declares **30** modules (27 `pub mod` plus 3 `pub(crate) mod`, at `:1-30`; round 2 said 28 and both round-2 reviewers verified the zero-arcs conclusion the count supports, which is unaffected). A module declaration produces no arc; only a `crate::` / `super::` / `self::`-anchored reference does. `agentscommander_lib::config` is therefore **not** a source and §11.5 criterion 3 is restated against the 18.

Verified per source: none of the 18 modules already has an arc to `agentscommander_lib::config::entity_prefix` (the module does not exist at base), so `post \ pre` is exactly 18 and no arc is double-counted. `config::ac_root` has out-degree **0** at base, so it is currently a sink and gains its first outgoing arc; that is noted because a reviewer scanning for "sink" properties will see its status change, and it does not affect the argument below.

### 11.3 Per-arc verdict

`agentscommander_lib::config::entity_prefix` is a **new sink**: it references nothing in the crate, so its out-degree is zero and it is its own singleton SCC. An arc terminating in a zero-out-degree node cannot lie on a cycle, cannot join two SCCs and cannot grow one. Therefore:

- every one of the 18 arcs is cycle-safe **by construction**, not by measurement;
- `cyclicSccs` stays 1;
- the 85-member SCC's membership is unchanged set-to-set, because no member gains a path to a member it did not already reach and `entity_prefix` cannot be pulled into it;
- no previously-clean SCC boundary is crossed in the sense the gate cares about, because a boundary crossing is only a risk when a reverse path exists, and a sink has no outgoing path at all.

The same argument holds for `src/shared/entity-prefix.ts`, which imports nothing: dependency-cruiser's `no-circular` cannot fire on an import into a module with no imports. `no-terminal-helper-back-edge` constrains only `terminal-session-registry.ts` and `terminal-output-admission.ts`, neither of which this change touches.

### 11.4 Role and layering hygiene

No lower layer gains a UI transport. `entity_prefix` takes and returns `&str`; it has no `AppHandle`, no `tauri` import, no `State`, no filesystem access, no async. It is strictly below every module that references it, so no co-location upward is required. The `pty`, `screenshot` and `session` modules gain a dependency on a `config` leaf, which is the direction that already exists (`pty/container_paths.rs:291` already calls `crate::config::ac_root::find_ac_root_ancestor`).

`instance_gitignore_layering`'s `ALLOWED_HOST_*` tables measure `crate::` / `super::` / `self::` spellings only and fix their dependency sets by equality, so `pub mod entity_prefix;` in `src/config/mod.rs` leaves them green. `loops_layering` scans `src/loops/`, not `src/config/loops.rs`. Neither guard has a table this change must extend; that is measured, not assumed, and criterion 5 below re-checks it.

### 11.5 The acceptance criterion the implementation reviewer runs (AC11)

On a clean tree, base SHA and final branch head:

```
node "<repo-personal>/ObsidianVault/Coding Agents/IA-Programming/rust/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph pre.json  --quiet
node "<repo-personal>/ObsidianVault/Coding Agents/IA-Programming/rust/01-rust_module-dependency-cycles.mjs" src-tauri --emit-graph post.json --quiet
node scripts/02-module-arc-record.mjs --graph post.json --out src-tauri/module-arcs.txt
```

Green if and only if:

1. `summary.moduleCycles` is **1** before and **1** after;
2. the cyclic SCC's member set is identical set-to-set, 85 modules, module by module;
3. the arc-record set difference `post \ pre` is exactly the **18** arcs of §11.2 — each of the 18 named sources paired with `agentscommander_lib::config::entity_prefix`, and nothing else — and `pre \ post` is empty. **`agentscommander_lib::config` must NOT appear as a source in `post \ pre`; if it does, the detector's `mod`-declaration behavior changed and that is a finding to report, not a pass;**
4. `git status --porcelain src-tauri/module-arcs.txt` is empty after the regeneration is committed;
5. `cargo test --test loops_layering --test instance_gitignore_layering --test project_settings_layering --test claude_watcher_layering` is green.

Exit code 1 from the detector is the expected outcome at both SHAs and must not be read as failure; only exit 3 means no graph was written.

---

## 12. Implementation order and ownership

Every step is the implementer's unless stated. Do not reorder: step 1 must land before any call site references the helper, and step 4 must land before step 5 edits the template it freezes.

- **Step 0. Entry gate, and the one capture that must happen before any edit.** §1.2; STOP on any mismatch. Then, still at `d7008b34` with a clean tree, capture **both** §3.12 Table C values: add `legacy_rendered_default_context_is_frozen` with two deliberately wrong expected lengths, run it once, read the actual lengths and sha256s for C1 (`legacy_rendered_default_context_for_compat`) and C2 (`pre_1072_legacy_rendered_default_context_for_compat`) out of the assertion failures, set them, and record them with the base SHA in the test's doc comment and in the PR body. **This is the only step that must run before the first product edit**, because the value it captures is the pre-change behavior of a function the rest of the change must not move.
- **Step 0b. Derive and commit the AC1 base allowlist.** Still at `d7008b34`, run the three sweeps of §9.4 AC1, subtract the Rule R lines this plan moves, classify the remainder, and commit `scripts/room-rename-allowlist.tsv` as Part A. Record the three per-surface subtotals and the closing check `rows(Part A) + lines subtracted = 4559` in the PR body. **This must land before step 9**, and it is placed here rather than beside step 9 because deriving it from the base is the whole point: a list generated after the edits cannot detect an edit that did not happen.
- **Step 1. The two helper modules.** Create `src-tauri/src/config/entity_prefix.rs` and its `pub mod` line; create `src/shared/entity-prefix.ts`. Add their unit tests. `cargo test --lib config::entity_prefix` and `npx vitest run src/shared/entity-prefix.test.ts` green before anything calls them.
- **Step 2. Parent-repository exclusion.** §5.6. Both tests of AC8 green. This lands early and independently because it is the one item that protects user data, and it is correct on its own even if the rest of the change were reverted.
- **Step 3. The 40 Rust gates and the 6 frontend predicates.** §5.3, §5.4. `determine_next_wg_number` (S4) is deliberately NOT in this step. Run `git grep -n 'starts_with("wg-")\|strip_prefix("wg-")' -- src-tauri/src` afterwards: the only remaining production hit must be `entity_creation.rs:4301`.
- **Step 4. Creation and the Room allocator.** §5.5 and §5.7. After this step `git grep -n 'format!("wg-' -- src-tauri/src` must return only the two `#[cfg(test)]` fixtures (AC2). Add the allocator and creation tests.
- **Step 5. Freeze everything, edit nothing.** §5.10 in full: the three seeded snapshots (D8a, wired into **both** root lists), the **three** dual-use frozen halves (D8b, D8c and D8f's `GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME`), and the `:3860` identifier switch. Copies only, no default or paragraph edited yet. Add `frozen_snapshots_are_byte_exact_at_d7008b34`, `workgroup_git_scope_split_is_correct`'s frozen half, `old_deferred_messaging_paragraph_is_frozen`, and AC7.8's source-digest check. The `assert_ne!` halves are deferred to step 6, where they become meaningful. Verify every copy against §3.12 Table A **before** proceeding; a copy that is wrong here is invisible from step 6 onward.
- **Step 6. Edit the three defaults, the two live halves, and bump the three versions.** §5.10 D8b (the live `WORKGROUP_GIT_SCOPE` text and its two `assert_eq!` counts), D8c (the live `ROOT_COORDINATION_MESSAGING_PARAGRAPH`), D8e (the three defaults, the six `ROOT_ROLE_MD` lines, `PTY_INPUT_COORDINATOR_CONTEXT`, and the one `injected_messages.rs:78` line), and **D8f in full**: the live `:831` line, the two new constants, the compare extension at `:4183-4208` and its corrected comment. Bump 4->5, 5->6, 7->8 and update the **eight** §9.3 clause-2 rows. **Two of them are the `injected_messages.rs:78` edit's twins and are not optional**: `:1331` inside `EXPECTED_SEED` takes the identical byte change under clause 1, and `:1671`'s `EXPECTED_SEED.len()` pin goes 1534 to 1531 under clause 2. Both are red on a correct implementation by construction, because `EXPECTED_SEED` is a hand transcription that exists to go red here (§9.3). Now the `assert_ne!` and `contains` halves are meaningful and must be green, and so must all five behavioral tests of §9.1: `pre_1072_context_still_self_heals`, `current_generation_legacy_context_classifies_current`, `pre_room_rename_skills_section_still_classifies_stale_generated_and_heals`, `edited_pre_room_rename_skills_line_is_preserved_not_healed` and `skills_section_replica_line_split_is_correct`. **D8f's compare extension must land in the same commit as the `:831` edit**, because between the two the recognizer is broken for every installation whose skills section carries the old line.
- **Step 7. The message-filename short prefix and its context/doc text.** §5.9.
- **Step 8. The CLI canonical names, aliases, `value_name`s and every printed string.** §5.8. Update `cli_behavior_contract.rs` under §9.3.
- **Step 9. GUI visible text.** §5.12. `ProjectPanel.tsx:993`/`:995` and their three resolvers move in one edit.
- **Step 10. Documentation.** §5.13, including the five compatibility statements and the new `CHANGELOG.md` entry. `docs/features/context-tracking.md:75` and `:78` keep the literal `%WORKGROUP%` and rename only the prose around it (D17).
- **Step 10b. Append Part B and run the gate.** Run the three sweeps of §9.4 AC1 over the post-change tree. Every unlisted line is either a Rule R miss to fix or a line this change legitimately introduced, and the second kind is appended to the committed allowlist as **Part B** with a class and a one-line justification naming the plan section that requires it (§9.4 AC1 point 4). Then confirm the unlisted count is zero. A row whose class cannot be justified from §5.2 is a Rule R miss to fix, not a row to write. §9.4 AC1 point 7 states which classes may be machine-classified and which five are read by hand.
- **Step 11. Regenerate `src-tauri/module-arcs.txt`** and run AC11.
- **Step 12. Local gates.** `cargo fmt --all -- --check` (in `src-tauri`; a required CI job), `cargo clippy`, `cargo test` (run from PowerShell, not from a Bash shell), `npm run typecheck`, `npm test`, `npm run test:debt`, `npm run check:frontend-dependencies`.
- **Step 13. AC1 through AC10 and AC13.** AC6 needs a built binary (`npm run build:testable` or a debug `cargo build`), so it runs here. AC5 does **not**: it is an in-process `clap` tree walk and runs with `cargo test` in step 12.
- **Step 14. PR.** Open against `main`, link #1614, and wait for every triggered check on the exact head SHA (§13.4). Owner for merge: tech lead.

Reviewer ownership: the architecture reviewer verifies §5, §10 and §11; the Rust reviewer verifies §5.3, §5.5, §5.9, §5.10 and §9.1's Rust half; the frontend reviewer verifies §5.4, §5.12 and §9.1's frontend half; the tech lead owns step 14 and the exact-head gate.

---

## 13. Delivery nonfunctional invariants

### 13.1 Accepted task class and threat model

**Routine application change.** No release, no packaging, no signing, no publishing, no untrusted build host, no security-boundary widening beyond the authorization-shape analysis in §8, no destructive or irreversible migration (this change performs no migration at all). The repository's pinned toolchain and locked dependency resolution are the trust anchor; GitHub Actions on the exact PR-head SHA is the authoritative host-dependent evidence.

Enhanced controls are therefore **not applicable** and are named explicitly so their absence is a decision rather than an omission: no independently anchored executable hashes, no DLL or helper closure inventory, no poisoned-`PATH` test, no SDK binary manifest, no runtime self-hash, no custom process-group runner, no exhaustive ancestor-configuration byte map, no bespoke transaction harness. Any finding that depends on one of these is advisory, not a readiness blocker.

Two controls that would normally be enhanced **are** applicable here and are justified individually:

1. **Byte evidence for every frozen copy** (§3.12): the three seeded snapshots, the three dual-use frozen halves, the one in-place frozen constant, and the one in-place frozen function. Justified because a frozen item's whole purpose is byte-exact equality with what a previous release shipped, and a copy that drifts by one byte silently disables auto-update for every pristine installation. The baseline control (a diff review) cannot detect a single-byte drift inside a 2.7 KB literal, and round 1 demonstrated that it cannot detect a *deliberate* edit into a frozen item either: two reviewers found it by tracing consumers, not by reading the diff.
2. **A measured dependency-cycle run** (§11). Justified because the repository maintains a committed arc record and four structural guard tests, so it is a repository contract, not an added control.

### 13.2 Baseline gate map

| # | Gate | Source of truth | Executable evidence | Expected result | Failure behavior | Owner / time |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | CI-to-plan parity | `.github/workflows/*.yml` at `d7008b34`, §3.11 | the 8 `pr-regression-gates` jobs plus `bundle-validation`, `lockfile-check`, `validate-branch-name`, `version-sync-check` | all green on the exact PR-head SHA | any red blocks merge; no waiver, no bypass | GitHub, step 14 |
| 2 | Deterministic toolchain | The repository pins no `rust-toolchain.toml`; it pins the setup action `dtolnay/rust-toolchain@4360b52568e2003a75bf9bc1d59f33a8e3fc893c` (`stable`) in every Rust job, `Cargo.lock` for dependency resolution, `npm@11.6.2` and Node 22 | `cargo --version`, `rustc --version`, `node --version` recorded in the PR body, and compared against the CI job log | recorded and comparable | a local/CI version mismatch downgrades the local evidence; CI stays authoritative | implementer, step 12 |
| 3 | Authorized, traceable Git | issue #1614, branch `refactor/1614-workgroup-to-room-phase-1`, base `d7008b34` | §1.2 entry ritual; `git diff --name-only origin/main...HEAD` | branch contains `origin/main`; delivery by PR, never a direct push to `main` | unknown or dirty base, or scope drift, blocks | implementer, steps 0 and 14 |
| 4 | Process state and working directory | the repo root | every command run with `-C <repo>` or from the verified repo root; `cargo` gates run from PowerShell, not from the Bash shell | reproducible | a cwd-sensitive command run elsewhere invalidates its evidence | implementer, throughout |
| 5 | Validation and scope | §6 path set, §6.7's two-part preserve set | AC10 | **no path from §6.7 Part 1 appears in the diff; no *production line* changes in any §6.7 Part 2 path.** Three new non-test files (`config/entity_prefix.rs`, `shared/entity-prefix.ts`, `scripts/room-rename-allowlist.tsv`), no rename | a Part 1 path in the diff blocks, and so does a changed production line in a Part 2 path; a Part 2 path appearing in the diff with only `#[cfg(test)]` hunks is permitted and the three such files are named in §6.1; a §6 path *absent* from the diff is a finding to investigate, not a failure, because §6 is the permitted set and not a quota | implementer, step 13 |
| 6 | Mutation ownership and recovery | branch head before each step | commit per step; recovery is `git restore --source=HEAD~1 -- <exact paths>` scoped to the paths that step wrote | recoverable per step | never `git reset --hard`, never a repository-wide clean | implementer, per step |
| 7 | Bounded execution and diagnostics | CI job timeouts; local runs with captured output | `cargo test` output redirected to a file (its stdout is otherwise swallowed and panic detail is lost); CI logs retained | a timed-out or failed command is reported as failed | a cleanup defect must not erase the primary failure | implementer, step 12 |
| 8 | Evidence discipline | this plan | every AC in §9.4 is a command with a stated expected result; zero and absence are typed states (AC2 expects exactly two lines, AC1 and AC9 expect zero, AC10 expects no `R` line) and every recognizer criterion is pinned to a literal captured at `d7008b34` and written into §3.12, never to a value the code recomputes | as stated | an AC that cannot be executed is a plan defect to report, not to skip | reviewer, step 13 |

### 13.3 Local versus CI evidence ownership

Local (implementer): `cargo fmt --all -- --check`, `cargo clippy`, `cargo test`, `npm run typecheck`, `npm test`, `npm run test:debt`, `npm run check:frontend-dependencies`, AC1 through AC11 and AC13, and the levelization run.

CI only (GitHub, authoritative): the Linux and macOS Rust legs, `windows-release-cli-smoke` (which needs a release build), `bundle-validation`, `lockfile-check`, `version-sync-check`, `validate-branch-name`, and the `#480` known-debt classifier in `frontend-regression`.

Note that `rust-regression` (windows) is the **only** leg that runs the Rust test suite, so a test added behind `#[cfg(unix)]` would run nowhere. Every test in §9.1 is platform-neutral.

`npm run check:frontend-dependencies` is **not** run by any workflow; it is local-only evidence and is named as such rather than claimed as CI coverage.

`validate-branch-name` can print its verdict and then crash: read the printed line, not the exit code.

### 13.4 Exact-head acceptance rule

Delivery requires every triggered and configured-required check to be green for the **exact PR-head SHA**. Evidence from another SHA, an unexplained skip, a waiver or an administrative bypass does not satisfy the gate. If the branch is updated, the gate is re-evaluated at the new head. The branch is brought up to date with `main` by **merge**, never by rebase, so the head keeps containing `origin/main`.

### 13.5 Bounded target-branch drift

The base is pinned at `d7008b34`. Later movement of `main` alone does not invalidate this plan and does not produce `CHANGES_REQUIRED`. Before the first product mutation and again before PR creation, fetch `origin/main` and classify the drift by changed paths:

- drift touching any §6 path, `.github/workflows/**`, `Cargo.lock`, `package.json`, `dependency-cruiser.config.mjs`, `test-debt.allowlist.json`, `src-tauri/module-arcs.txt` or any `*_BEFORE_*` frozen constant requires refreshing only the affected evidence (§3.2/§3.3 line anchors, §3.11 job list, §3.12 digests, §11.1 baseline) and re-reviewing that evidence;
- drift proven unrelated is recorded and synchronized at the next bounded gate.

Once the PR exists, exact-head GitHub checks and the repository merge policy are authoritative. Continuous pre-PR attestation that `main` never moved is forbidden.

---

## 14. What a reviewer should attack first

Round 1 was rejected on six blocker groups and round 2 on six more. Items 1 to 6 below are the round-1 blockers, each now also carrying its round-2 follow-on, and where the fixes live; a reviewer should confirm the fix rather than re-derive the finding. Items 7 to 12 are the standing hazards round 1 got right.

**The three round-2 blockers that do not map onto a round-1 group, and where they now live.** The preserve half of the scope gate is §6.1's restated constraint, §6.4's placement of `api/identity.rs`, §6.7's two parts, AC10 and §13.2 gate 5. The self-proving allowlist is §9.4 AC1's Part A / Part B split and §12 step 0b. The unsatisfiable `StaleGenerated` assertion is §3.12 Table C's two captures, §7 item 10's two outcomes and §9.1's five behavioral tests. A reviewer attacking this round should attack §9.4 AC1's arithmetic and §5.10 D8f's compare ordering first, because those are the two places where this round adds mechanism rather than correcting text.

1. **The frozen legacy recognizer, now the whole chain and not one function (§3.7 family 3, §5.2 P3, §5.10 D8b and D8f, AC7.8, AC7.14, AC7.15).** Round 1's §5.9 sent four Rule R edits into `session_context.rs:3769-4025`, which reconstructs a user's pre-#1369 context file for byte comparison. Confirm the four lines are gone from §5.9's edit set, that the only change inside the range is the single `WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME` identifier at `:3860`, and that AC7.8's source digest `940FA357...16C2` reproduces after that one-token substitution. **Then keep going past the end of that function**, which is what round 2 did not: `reconstruct_legacy_rendered_default_context` (`:4089`) also runs `is_provably_generated_legacy_skills_section` (`:4183`), which recomputes the **live** `render_skills_section` (`:812-919`), and that function carries the retired token at `:831`. Confirm D8f exists, that its frozen copy matches `A5C74FD6...7EC9` at 131 bytes **including the trailing newline**, that the compare extension is applied **before** `normalize_context_for_compat`, so the swap sees the line's terminating newline, and before both existing compares, and that AC7.15's two behavioral tests build their fixtures from the frozen constant while AC7.14 pins that constant to §3.12 Table B, so the **pair** supplies the external anchor. **Round 3 stated this ordering backwards** ("applied after `normalize_context_for_compat`"), contradicting §5.10 D8f on the single ordering question D8f devotes a bullet to; D8f's code block is authoritative and correct, and this item is corrected in round 4. `ac-dev-rust-grinch-v3` found it in round 3.
2. **The three dual-use constants (§5.10 D8b, D8c, D8d).** Each needs a split or an in-place freeze, and each frozen half needs its §3.12 digest. Check `WORKGROUP_GIT_SCOPE` first: its replacement text sits at exactly the word ceiling `session_context.rs:6107` enforces, and a text that reads better but adds one word turns the suite red.
3. **The GUI sweep and its allowlist (§3.8, §9.4 AC1).** Round 1's 36 needles went green with seven production Workgroup strings shipping. Confirm the sweep reproduces 906 lines / 41 files at base, that the allowlist is committed with a class per row, and that the post-change unlisted count is zero. A post-change count still equal to 906 is a finding.
4. **The Rust reader-facing inventory (§3.14, §6.1, §13.2 gate 5).** Confirm the nine files round 1 omitted are in §6.1's edited list, and that gate 5 no longer requires "exactly the §6 paths" in a way that punishes a correct edit.
5. **`clap` completeness (§5.8, §9.4 AC5).** `role_experiment.rs:95`'s `value_name` and `team.rs:61`'s third `help` string. Then check AC5 walks the tree rather than a list, and, more importantly, that it **asserts membership**: `team create` (a depth-2 leaf), `role-experiment` (hidden) and `role-experiment variant set` (depth 3) must each be in the walked set, and the first two must have rendered the strings whose omission caused G5. Round 2's `checked >= 25` guarded "walked nothing" while the failure round 1 shipped was "walked one level", which reports 40 and passes. The derived floor is 75 on Windows and its arithmetic is stated in AC5; because clap's synthetic `help` nodes mirror the sibling tree and recurse, the real `walked.len()` is far above it, so the floor is a vacuity guard and the membership assertions are the criterion. **Check the three membership assertions build their keys from `root.get_name()`.** Round 3 hard-coded `"agentscommander team create"`, which is wrong: the root name is `agentscommander-new`, from `CARGO_PKG_NAME`, because `Cli` sets no `#[command(name)]`. A reviewer seeing three green membership assertions cannot tell whether they were weakened or deleted to green a panicking test, and weakening them restores exactly the vacuity H6 removes, so confirm the assertions are **present, three in number, and prefix-derived**.
6. **The arc count (§11.2, §11.5).** 18, not 19. `agentscommander_lib::config` must not appear as a source in `post \ pre`.
7. **The `room-*/` gitignore entry (§3.6, §5.6, AC8).** Neither the issue nor the assignment names it. If it is missing or lands after step 4, a Room created inside a git-tracked project is corrupted by the next parent `git checkout`. This remains the highest-severity item in the change.
8. **The two `&name_str[3..]` slices (§3.2, §5.3).** A mechanical `starts_with` swap that leaves the literal `3` mis-slices every Room name and can panic. Check `entity_creation.rs:2965` and `:3489` first.
9. **`determine_next_wg_number` (S4).** It is the one `strip_prefix` site that must stay Room-only. A sweep that made it dual would silently break requirement (B) while every dual-prefix test stays green.
10. **The six frontend predicates (§3.3).** The assignment says the frontend is unaffected. Verify F1 through F6 individually; each fails silently.
11. **`PURGE_WG_ACTION` and the other §3.10 values, now including `%WORKGROUP%`.** Anything in that table appearing in the diff is a compatibility break dressed as a rename. `known_default_sha256` gaining a second entry is the specific shape to look for.
12. **The negative test evidence (§9.2).** Any existing `wg-*` fixture that had to be edited means dual-prefix acceptance is broken. Ask why, do not accept the edit. Conversely, §9.3's three **clauses** are the closed set of permitted test edits: an edit that satisfies none of them is a finding. The clauses are closed; the site lists are not uniformly so, and §9.3 now says which is which. Clause 2's list is complete at **eight** rows: four countable `current_version` pins, one function-name rename that follows one of them, and three size pins closed by a sweep of the four constant-bearing config files, which §9.3 states and both round-3 reviewers reproduced. **Round 3 said seven and was short by one**: `injected_messages.rs:1671` pins `EXPECTED_SEED.len()`, a size, so clause 1 did not reach it and clause 2's enumeration did not carry it, while a correct implementation makes it red. It is added in round 4, together with its clause-1 twin `injected_messages.rs:1331`. Clause 1's list of **twenty** sites is illustrative, because Rule R is total over reader-facing carriers and the red set is whatever pins one; the clause is the gate.

**Where this plan disagrees with a round-1 reviewer.** Nowhere on substance. Every blocker in both verdicts is accepted and fixed. Two round-1 reviewer statements are refined rather than contradicted, and both are noted here so the refinement is visible:

- `dev-rust` proposed freezing `3769-4025` under Rule P3 wholesale. This plan does that **and** adds the transitive-closure obligation, because freezing the body alone leaves `WORKGROUP_GIT_SCOPE` free to move underneath it and the freeze would be nominal.
- `grinch` recommended AC5 walk `Command::get_subcommands` recursively **or** drive `<bin> <sub> --help`. This plan takes the first, in-process, because it needs no built binary and no process spawn, so it runs under `cargo test` rather than needing step 13's built binary. **Round 2 justified that choice by saying the walk "runs on all three CI legs and not only the Windows one", and that reason is false by this plan's own measurement**: §3.11's job table calls the Linux and macOS legs build/clippy legs and §13.3 states that `rust-regression` (windows) is the only leg that runs the Rust test suite. §3.11 and §13.3 are the correct pair, the decision is unaffected, and the reason is corrected here, in §5.8, in §9.1 and in AC5. `ac-dev-rust-v3` found this in round 2.

**What this plan adds that neither reviewer found.** Three items from round 2, each a silent-failure class of the same shape as G1 and G2, all three since verified exact by both reviewers: the second root wiring in `migrate_root_role_file` (§3.7 family 2, §5.10 D8a), the injected-messages `known_default_sha256` recognizer and its `%WORKGROUP%` token (§3.7 family 5, D17), and the `WORKGROUP_GIT_SCOPE` compactness budget that constrains what the replacement text may say (§5.10 D8b, R10).

**Three more found in round 3, while re-deriving the reviewers' findings rather than transcribing them.** Each is a claim in round 2 that measurement contradicted, and each is corrected in place with the measurement stated: §3.14's step 1 asserted the crate uses only the column-0 `#[cfg(test)] mod ... {` shape, which is false and is the mechanical reason `config/root_agent.rs` appeared with 3 candidate lines against a raw sweep of 50; §3.8(d) declared "3 sites" over an enumeration of two and cited `WorkgroupGroupRail.tsx:72`, the predicate line, where the visible label is at `:73`; and §9.4 AC1's prose claimed `docs/assets/` was excluded by pathspec from all three sweeps when no command excluded it, so 14 lines including a binary file were inside the gate. None of the three changes a decision. All three are the kind of claim a reviewer uses as a completeness check, which is why they are corrected rather than left.
