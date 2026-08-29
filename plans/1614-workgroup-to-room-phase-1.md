# Plan #1614: Workgroup to Room, phase 1 (creation prefix, dual-prefix acceptance, visible text, CLI aliases, docs)

Author: ac-architect-v3, workgroup wg-24-ac-dev-team-v3. Full delivery: new entities are created as `room-<N>-<team>`, every existing `wg-*` directory keeps working untouched, the CLI gains canonical `room` / `purge-room` / `--room` names with the old ones kept as deprecated aliases, and every surface a human or an agent reads says Room. Five families of recognizer over user-owned files are protected: three seeded context templates gain a frozen pre-rename snapshot and a version bump, **three** dual-use items are split into a frozen half and a renamed half, and two more constants plus one whole function are frozen in place, so no existing installation loses its auto-update. The freeze is scoped to the whole classification chain rather than to one function, which is what round 2 got wrong. No identifier rename, no source-file rename, no persisted-key change, no on-disk migration, no new dependency.

Status: READY_FOR_IMPLEMENTATION

Revision: round 8 (2026-08-28). **Voids Plan-SHA256 `9E078FADCC294999418D4A895B18DE7C9BC91DD0887C1DD8A6D6CDB18811BC94`** (round 7, 388,036 bytes, 2,465 lines), on which `ac-dev-rust-v3` returned **PLAN_APPROVED** with no blockers and no conditions and `ac-dev-rust-grinch-v3` returned **CHANGES_REQUIRED** with one. **M1 is closed**: both reviewers verified all five sub-items, the fourth site, both back-references and every figure independently, and both reproduced the whole diff. **Round 8 is that one blocker plus one note, and nothing else.** No design decision, no owner decision, no digest, no frozen byte, no sweep and no acceptance criterion other than the one named moves, and every item on the standing do-not-reopen list is untouched.

- **M2, point 10's self-test asserted an exact set that this plan's own classification rules do not determine (§9.4 AC1 point 10, the self-test row and class (e); §5.2 P1; §15.2 row 6).** Round 6 asserted a value the limb cannot produce; round 7 fixed the value and bolted an exact-set equality onto it, which is the same defect one turn further. Run at `bb2a5a65` the limb returns **77** absences: **9** `P0-identifier`, **3** `P1-comment`, **63** `P2-fixture`, **2** `P4-log`. Two of the three `P1-comment` absences, Part A rows **872** (`cli/list_peers.rs:910`) and **1411** (`commands/entity_creation.rs:2953`), are genuine Rule P1 clause (b) corrections that no round of this plan had named. Both comments describe §3.2 gate lines this plan moves (P2 and S1 at `:907`/`:912`, P12 at `:2964`), so an implementer reading clause (b) **by its name** authorizes them and gets six, while one reading it **by its closed list** gets eight and stops on a limb that is working perfectly, at the one run that proves the limb. The equality is deleted, the six become a **lower bound**, all three `P1-comment` absences are named with their disposition, and the root cause is fixed where it lives: **§5.2 P1 clause (b) is a predicate, not a closed list**, the same inversion K1 forced on point 10's classes and the dev's note forced on §1.1's exception column. `ac-dev-rust-grinch-v3` found it by running the limb rather than reading it; this round reproduced the run, the class histogram and both sites before editing.
- **N1, §14 item 3 sent a future reviewer to apply a test round 6 had already ruled out (§14).** It asked for the legitimate-absence classes to be checked for exhaustiveness against a count of **four**. There are **five** since round 6, and round 6 inverted that enumeration so the predicate is the gate and the classes are illustrative. The item now attacks the predicate. Both reviewers flagged the same sentence independently.

**Left alone on both reviewers' advice, and recorded so it is a decision rather than an oversight.** §15.6's stale ordinal on the em-dash bullet, and §1.1's exception cell saying "three" in a sentence that names two before the third arrives in the next sentence. Neither gates anything, neither carries a figure, and each would add a hunk to a section this round does not otherwise reach. The durable fix for the first is to drop the ordinal for an explicit round list whenever §15.6 is next opened for another reason.

Revision: round 7 (2026-08-28). **Voided Plan-SHA256 `F8341A66A37A67D6524CFFA5300CE5895467CE8245AE6778D275A6174D520A13`** (round 6, 378,291 bytes, 2,450 lines), on which `ac-dev-rust-v3` returned **PLAN_APPROVED** with no blockers and no conditions and `ac-dev-rust-grinch-v3` returned **CHANGES_REQUIRED** with one. **Round 7 is that one blocker plus three notes, and nothing else.** No design decision, no owner decision, no digest, no frozen byte, no sweep, no acceptance criterion other than the one named, and no figure outside those named below moves. Every item on the standing do-not-reopen list is untouched.

- **M1, point 10's self-test expected six items and a correct limb surfaces five (§9.4 AC1 point 10; §15.1 D1; §15.2 heading, row 6 and closing paragraph).** The reverse limb asks, for each Part A row, whether `(path, trimmed content)` is still present. **§15.2 row 6 is not a substitution**: the comment is *unchanged* at `bb2a5a65`, so its key is present and the limb has nothing to report; the row is a stale comment whose *referent* moved, which is a different kind from rows 1 to 5. Only rows 1 to 5 can surface, and because row 3 renamed the same warning code on two lines they surface as **exactly six Part A rows**, so a run showing six rows looked like a pass while the item the self-test was written around never appeared. The criterion now states **five of §15.2's six items, as six named Part A rows**, names its unit, excludes row 6 with the reason, and names the forward gate as what catches row 6 instead. This is K2's own defect shape one level down: round 5's self-test could not pass as ordered, round 6 fixed the ordering and left the expected value wrong. `ac-dev-rust-grinch-v3` found it by running the limb rather than reasoning about it, and this round reproduces its run independently.
- **The absent count is 77, not 78, and the difference is this plan's own normalization (§9.4 AC1 point 10).** Measured over all 2,988 committed Part A rows against the three sweeps at `bb2a5a65`: **77 absent across 16 paths** with AC1 point 3's `(docs/screenshots/hero.png, "<binary file>")` key applied, which is what the committed `scripts/room-rename-allowlist.mjs` does; **78** if that row is left unnormalized. The self-test's output goes in the PR body, so the plan carries the number its own instrument produces and names the fork.
- **§9.3 limb B's carve-out gains a fourth pre-existing #1571 message, `seeded_context_templates.rs:3710`.** `ac-dev-rust-grinch-v3` swept the class exhaustively (88 candidate lines) and found exactly one message the rule's boundary hides. Verified at `df494bfa`: it is the message argument of an `assert_ne!` over two frozen hashes inside `read_sync_updates_seeded_v3_coordinator_and_bumps_version` (`:3663`), it carries `:3736`'s and `:3751`'s exact "the current v4 default" shape about a coordinator whose live version is already 5, and nothing it says is executed. Named so a reviewer does not read it as a miss and an implementer does not bump it.
- **§1.1's exception column was an enumeration sitting beside a predicate, which is K1's shape (§1.1).** `ac-dev-rust-v3` raised it. The predicate is now stated as operative and the enumeration is explicitly illustrative, and the same measurement is restated in a form that does not drift: **37 distinct explicit `session_context.rs:<line>` citations, 37 of 37 resolving differently at `df494bfa`**, of which 31 are `d7008b34` anchors and six carry a visible local label at another base. The stale "36" was measured before round 6 itself added `:10615`, which is precisely how a bookkeeping count drifts away from the predicate beside it.

Revision: round 6 (2026-08-28). **Voided Plan-SHA256 `0FE4EBB8B2314147E8198782DF9D0EBC23FFB0BB43DD4500FCC2F22E8239448F`** (round 5, 345,950 bytes, 2,353 lines), which `ac-dev-rust-grinch-v3` and `ac-dev-rust-v3` both returned **CHANGES_REQUIRED** on, with three and two blockers. **Consensus progressed**: measured against the eleven plan defects and six code defects the implementation review produced, P2 through P11 and all six code-defect dispositions were resolved by both reviewers independently, P1's re-base passed both reviews end to end, and both reviewers said they expect to approve the next candidate. Round 6 is therefore a narrow correction pass, not a redesign.

**Round 6 changes, by the item that forced each. No design decision, no owner decision, no digest and no frozen byte moves in this round.** The four owner decisions, the whole re-base including every digest, sweep and control, D8b through D8f, 14 of the 16 Table A/B digests, AC7.8's `940FA357...`, P6's closure by literal and P3's limb B sweep are all untouched. Five blockers and eight notes are addressed, each named in the section that carries it:

- **K1, AC1 point 10's absence classes were an enumeration used as a gate, and the plan supplied its own counterexample (§9.4 AC1 points 8, 9 and 10; §3.8; §12 step 0b; §15.2; §15.5).** A Rule P1 clause (b) comment correction falls into none of the four classes, and §15.2 row 6 **mandates** one: `WorkgroupTask.tsx:70` at `df494bfa`, which is in the 906-line base sweep, is not in point 8's enumerated subtraction, and is on the committed Part A as `P1-comment`. Point 10 is inverted so the **predicate** is the gate and the classes are illustrative, exactly as §9.3 clause 2 was inverted in round 5; a fifth illustrative class (e) is added; the line joins the frontend subtraction (**64 to 65**, Part A's frontend half **842 to 841 lines / 775 rows**); §12 step 0b deletes both it and `ProjectPanel.tsx:2749` from Part A by name and row number; and **§15.2 row 6 now fixes the exact replacement bytes**, which is what makes point 9's count determinable at all. Point 9 predicts **862**. `ac-dev-rust-grinch-v3` found it; `ac-dev-rust-v3` reached the same gate from `:2749`.
- **K2, point 10's self-test was unsatisfiable as ordered (§9.4 AC1 point 10; §15.1; §15.2; §15.5).** §15.1 runs D1 and D2 before D5, so at D5 the limb surfaces zero of the six substitutions it was told to surface, plus one false positive. The self-test and the gate are now two different runs in a table: the self-test runs against `bb2a5a65` **before D1** and must surface the substitutions that are still on the tree; at D5 the expected result is an **empty** fourth column. **Round 7 corrects the expected count and names its unit**, because five of the six items surface, as six Part A rows.
- **K3, point 8's closing check was arithmetically false and it is what the implementer writes into the PR body (§9.4 AC1 point 8; §3.8; §12 step 0b; §15.5).** The fourth column was headed "Part A rows" and carried a **line** count, and the check read `rows(Part A) + lines moved = 4573`. It is now `` lines(Part A) + lines moved = 4573 ``, that is **3690 + 883**, with every per-surface figure measured and each surface closing on its own; the Part A **row** count is carried separately (**2,989** rows over **2,977** unique keys), together with the reason the two differ by 12. This already bit once: the committed allowlist's own header had to invent the disambiguation the plan did not give.
- **K4, the re-base was not propagated to the line numbers, and one stale value was the `global` version itself (§1.1; §3.7 family 1; §3.14; §5.9).** §1.1's document-wide rule is replaced by a measured per-file table: **no `session_context.rs` citation resolves to the same content at both bases** (round 7 restates the count as **37 distinct explicit citations, 37 of 37 differing**, of which 31 are `d7008b34` anchors), three explicit citations are at branch head `bb2a5a65` and are labelled there, and `seed_manifest.rs`'s two rows are re-anchored (`:1339`/`:3447` to `:1346`/`:3454`) because neither reviewer had spotted that this file drifted too. **§3.7 family 1's table is re-derived at `df494bfa`**: `global` `current_version` **5 at `:517`**, not 4 at `:483`, with all seven anchors moved. The table stays at three rows; both reviewers said the `platform.*` disposition in D8a is sufficient.
- **K5, "all 14 added lines are inside `#[cfg(test)]`" was false, stated twice, inside the section added to make the re-base auditable (§1.1; §3.14; §9.4 AC1 point 8).** Thirteen are. The fourteenth is `seeded_context_templates.rs:220`, production code inside `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES`; that file's only `#[cfg(test)]` is at `:2093`. It lands in §3.14's blind class, so the corrected inner numbers are **1175 / 63 files, 582 reachable, 593 blind**. No file count and no §6.1 figure moves.
- **Eight notes, all applied.** §1.1's 14-line delta is decomposed over **six** keys, three appended and three occurrence increases, so an implementer cannot append two duplicates (§1.1, §12 step 0b). §12 step -1 no longer asks git for a resolution it cannot offer: **(ii) moves to D3** because a branch-only addition merges silently, and step -1 gains **(v)**, the conflicted `"...current v5 default"` message whose assertion auto-merges to 5 and must become 6, plus the `let [global, coordinator, windows, linux, macos]` destructuring under (iv). §9.4 AC1 point 3 names the `Binary file docs/screenshots/hero.png matches` line and the `(path, "<binary file>")` key it takes. §10.2 R14 gains the third budget, `MAX_FULL_WG_PROFILE_BYTES = 8_313` over `full_wg`, and §12 step 12 records both figures. AC7.1b says that its `!=` limb, not its `||`-chain limb, is the guard for the silent case. §9.3 limb B says "17 version-pinning sites" rather than "assertion sites", and gains the note that its sweep returns assertion lines and never the message beside them.

Revision: round 5 (2026-08-28). **Voided Plan-SHA256 `0FA84D19D82E3779CB1F0C58F0C623DAAE8DB562A6AE5DA16EFF22386AE76728`** (round 4, three-party consensus, user-approved, and **implemented**: 12 commits landed steps 0 to 13 on `refactor/1614-workgroup-to-room-phase-1`, head `bb2a5a65dcf065bdf3304151045a9f946933550b`, pushed). `ac-dev-rust-grinch-v3` reviewed that implementation and returned **FAIL**. The round-4 consensus and the user's digest-bound approval are therefore void, and this round is a revision driven by what implementation and review proved on the tree, not a plan review.

**THE RE-BASE, FIRST, BECAUSE EVERYTHING ELSE IN THIS ROUND DEPENDS ON IT.** The frozen base moves from `d7008b34e155a8bd6481be5feecfc7d96575328f` to **`df494bfa04f7e14fa9a42f3b0d89ccbc2ce76e80`**, which is `origin/main` measured in this round. `d7008b34` is an **ancestor** of `df494bfa` (`git merge-base d7008b34 df494bfa` returns `d7008b34`), so this is a fast-forward re-pin, not a divergence. §1.1 carries the full re-derivation ledger, §13.5 carries the classification that forced it, and §15 carries the delta the implementer applies to the 12 commits that already exist. The one-line summary: **exactly one frozen value changes** (§3.12 Table A row 1 and Table B row 1, the global context template, which #1605 edited), **the `global` version bump becomes 5 to 6 rather than 4 to 5**, **AC1's Rust base sweep goes 3090 to 3104** (14 lines, 3 unique rows, all additive, zero base rows lost), and **every other digest in this plan reproduces byte-identically at the new base**. Every digest below is now labelled with the SHA it was taken at.

**Round 5 changes, by the item that forced each.** Eleven confirmed plan defects (P1 to P11 in the round-5 assignment) plus six code defects that need plan text before the code can be fixed. Sections touched: preamble, §1.1, §1.2, §3.8, §3.12, §5.2, §5.9, §5.10 D8a, §6.1, §9.1, §9.2, §9.3, §9.4 (AC1, AC1b, AC5, AC7, AC10), §10.2, §12, §13.5, §14, and a new §15.

- **P1, the drift (§1.1, §3.12, §5.10 D8a, §9.3, §13.5, §15).** `origin/main` moved four merges past the pinned base and #1605 landed inside this plan's frozen evidence: it bumped `global` 4 to 5, added `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES`, and edited the global template body with a new `{{HOST_PLATFORM_RULES}}` placeholder. §13.5 triggers on **paths touched**, and five trigger paths moved, so "proven unrelated" was not an available classification and the call was the tech lead's, not the implementer's. Re-pinned and re-derived here rather than left to a merge conflict.
- **P2 (§9.2, §9.3 clause 4).** D5 and requirement (A) force fixture edits that Rule P2, §9.2 and §9.3 all forbade. New clause 4 authorizes them; §9.2 gains the carve-out that distinguishes "moved because creation now produces a Room" from "moved because acceptance broke".
- **P3 (§9.3 clause 2, §14 item 12).** Clause 2 was declared complete at eight rows and is not. The **version limb** of the sweep the plan describes was never run by anyone. It is run here, at the new base, and clause 2 is restructured: the **clause** is the gate, as clause 1's is, and the enumeration is evidence with the sweep command stated so it is re-derivable.
- **P4 (§9.4 AC5).** `retain_workgroup` is a field of `RunArgs`, attached to `RoleExperimentCommand::Run`; `role-experiment` is a parent whose long help renders `<COMMAND>`, so AC5 as frozen panicked on a correct implementation. The rendering assertion moves to `role-experiment run`; the membership assertion stays on the hidden parent.
- **P5 (§5.9, §10.2).** §5.9's legacy clause was unsatisfiable against AC7.13. The substitute is decided here, in the plan, because an implementer is not authorized to decide it. The size half of the conflict is **relieved** by the re-base, and §10.2 records the corrected budget.
- **P6 (§3.8, §5.2 R1, §9.4 AC1).** A fourth R1 resolver, `ProjectPanel.tsx:2749`. The subtraction becomes 64 and the frontend Part A becomes 842, which disposes of P10. (**Round 6 moves both figures again**, to 65 and 841, for a different reason: the Rule P1 clause (b) comment correction §15.2 row 6 mandates is a tenth non-Rule-R line that moves. AC1 point 9's prediction is therefore **862**, not 863.)
- **P7 (§9.4 AC10 clause 4).** Four new non-test files, not three; the allowlist script is required by AC1 point 7.
- **P8 (§9.4 AC1b).** AC1b carried no pathspec and two of its ten needles collide with Rule P1 and Rule P4 preserved text. Restated with AC1's own path sets and a permitted-hit table.
- **P9 (§5.2 P4).** P4's stated premise ("no such shared string exists") is refuted by `cli/list_peers.rs:654`/`:659`. The false claim is deleted and the rule gains a carve-out.
- **P10.** No separate change: fixing P6 makes the arithmetic exact. The latent risk it exposed is recorded in AC1 point 9.
- **P11 (§3.12 preamble).** `.gitattributes:5` is `*.rs text eol=lf`. Corrected; no value moves.
- **Six code defects (§15, and §9.1, §6.1 where the plan had to say something first).** One live contract break (`commands/task.rs:167`), twelve §9.1 tests that were never written, a cross-process test pointing at a nonexistent FQN, and three low or trivial unauthorized substitutions.
- **A gate weakness, not a code defect (§9.4 AC1 point 10, new).** AC1 is blind in reverse: a renamed line stops matching the sweep and **disappears** rather than appearing unlisted. AC1 gains a reverse limb.

Revision: round 4 (2026-08-28). Voided Plan-SHA256 `D0E2DF0F02A02271C51074DD040C117EA04B932592E6220CC493AE16CAF9EBC7` (round 3, `PLAN_APPROVED` from `ac-dev-rust-grinch-v3` and `CHANGES_REQUIRED` from `ac-dev-rust-v3` with two blockers), which voided `1E8DF4AB4675B145015D3142802DCAD80FA2B2793DB40C73148E12B54A35BFD2` (round 2), which voided `72D58AF23AC0E35D07A268C5563F0BF564AC9B2734170B25677E8F8FDC9951CC` (round 1). Round 1 remains the consensus-progress baseline.

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
| **Frozen base (round 5)** | `origin/main` == **`df494bfa04f7e14fa9a42f3b0d89ccbc2ce76e80`** |
| Superseded base (rounds 1 to 4) | `d7008b34e155a8bd6481be5feecfc7d96575328f`, an **ancestor** of the above |
| Branch head at re-base time | `bb2a5a65dcf065bdf3304151045a9f946933550b`, 12 commits, pushed, built on `d7008b34` |
| Live-fetch check at round-5 authoring time | `git fetch origin main` then `git rev-parse origin/main` returned `df494bfa04f7e14fa9a42f3b0d89ccbc2ce76e80`; `FETCH_HEAD` carries it; `git merge-base d7008b34 df494bfa` returns `d7008b34` |
| Working tree at authoring time | `git status --porcelain` empty |
| Delivery path | Full |
| Accepted task class | Routine application change, no release, no signing, no untrusted host (see §13.1) |

Codebase Memory gate `ready` at round-1 authoring time (2026-08-28 UTC): project `D-0_repos-AgentsCommander_iac-.ac-wg-24-ac-dev-team-v3-repo-AgentsCommander`, 25,291 nodes / 136,344 edges, `head_sha` `d7008b34e155a8bd6481be5feecfc7d96575328f`.

#### The re-base ledger: what is re-derived at `df494bfa`, and what is carried from `d7008b34` with that label

**Why the base moves at all.** §13.5 triggers on **paths touched**, not on semantic relatedness. The drift `d7008b34..df494bfa` touches `.github/workflows/pr-regression-gates.yml`, `src-tauri/src/config/mod.rs` (a §6.1 **edited** path), `src-tauri/src/config/settings.rs` (a §6.1 **preserved-16** path), `scripts/smoke-cli-release-windows.ps1` (a §6.7 **Part 1** path), `src-tauri/src/lib.rs`, `src-tauri/src/main.rs`, `src-tauri/src/config/profile.rs`, `src-tauri/src/config/seed_manifest.rs`, `src-tauri/src/config/seeded_context_templates.rs` and `src-tauri/src/config/session_context.rs`. Five §13.5 trigger paths, and #1605 lands **inside the frozen evidence itself**. The correct bucket was "refresh the affected evidence and re-review it". Resolving it in the plan rather than in a merge conflict is what this section does.

**Rule for the whole document: every digest is labelled with the SHA it was taken at.** A row labelled `d7008b34` was re-run at `df494bfa` in round 5 and **reproduced byte-identically**; the label records provenance, not staleness. Only rows explicitly labelled `df494bfa` changed value.

| Evidence | Verdict at `df494bfa` | Where |
| --- | --- | --- |
| §3.12 Table A row 1, global template **declaration** | **RE-DERIVED.** `session_context.rs` 2470-2492 / 549 / `991C5BA8...` becomes **2513-2537 / 574 / `D9E93582...`** | §3.12 |
| §3.12 Table B row 1, global template **rendered value** | **RE-DERIVED.** 539 / `F4406596...` becomes **564 / `D094106B...`** | §3.12 |
| §3.12 Table A rows 2, 4, 7, 8, 9 and Table B rows 2, 4, 7 | **CARRIED, byte-identical, line ranges re-anchored.** Every digest reproduces; only the `session_context.rs` line numbers shift | §3.12 |
| §3.12 Table A rows 3, 5, 6 and Table B rows 3, 5, 6 (`root_agent.rs`) | **CARRIED, unchanged including line numbers.** `root_agent.rs` is not in the drift's changed-path set | §3.12 |
| §3.12 Table C, C1 and C2 | **CARRIED**, and re-verified at the new base as a step-0 gate rather than re-captured | §3.12, §12 step 0 |
| `global` spec `current_version` | **CHANGED.** 4 to 5 becomes **5 to 6**. #1605 already took 4 to 5 | §5.10 D8a, §9.1, §9.3 |
| `coordinator` spec `current_version` | **CARRIED.** 5 to 6, unchanged | §5.10 D8a |
| `rootAgent` spec `current_version` | **CARRIED.** 7 to 8, unchanged. `root_agent.rs` is untouched by the drift | §5.10 D8a |
| `is_known_generated_global_template` / `..._standalone_...` | **WIDENED.** Both must now accept `_BEFORE_HOST_PLATFORM_RULES` (main's) **and** `_BEFORE_ROOM_RENAME` (this plan's) | §5.10 D8a |
| §9.4 AC1 base sweeps | **RE-DERIVED.** 906 / 3090 / 563 = 4559 becomes **906 / 3104 / 563 = 4573**. File counts 41 / 94 / 64 are unchanged | §9.4 AC1 point 8 |
| §9.3 clause 2, the version limb | **RE-DERIVED at `df494bfa`**, and it is the limb nobody swept | §9.3 |
| §3.8's frontend inventory, §3.14's Rust inventory, §3.2, §3.3, §3.10, §11.1 | **CARRIED.** The frontend sweep is 906 at both SHAs and the frontend drift is empty; §11.1's graph baseline is re-measured at step 11 as it always was | as cited |
| §3.11 CI job table | **RE-CHECK REQUIRED at step -1.** `pr-regression-gates.yml` moved | §3.11, §12 |

**The global version collision, resolved.** Three specs, four columns:

| spec | base `d7008b34` | `origin/main` `df494bfa` | branch `bb2a5a65` | **correct after the merge** |
| --- | --- | --- | --- | --- |
| `global` | 4 | **5** (#1605) | **5** (#1614) | **6** |
| `coordinator` | 5 | 5 | 6 | 6 |
| `rootAgent` | 7 | 7 | 8 | 8 |

The branch and `main` both took `global` to 5, for different reasons, from the same 4. They do not compose; the merged tree must be **6**. Concretely, post-merge:

- `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES` (main's) keeps the **v4** body, 539 bytes, `F4406596...`, and is not touched by this plan.
- `GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME` (this plan's) is **re-based**: it must hold the **v5** body, that is main's template text **including** the `{{HOST_PLATFORM_RULES}}` placeholder, **564 bytes**, `D094106B...`. On the branch today it holds the v4 body, which is now a generation main has already superseded and already froze under its own name. Carrying it forward unchanged would ship two constants holding identical bytes under two names, and would leave the v5 generation unrecognized forever.
- `global.current_version` becomes **6**.
- Both global recognizers accept both frozen names.
- #1614's Rule R edit applies to main's **new** template text. The retired-token carrier is unchanged by #1605: it is still the single Core Concepts line `- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and \`repo-*\` working repos.`, and `{{HOST_PLATFORM_RULES}}` carries no occurrence of the retired token.

**The three new specs #1605 adds (`platform.windows`, `platform.linux`, `platform.macos`, all at `current_version: 1`) take no snapshot and no bump**, and that is measured, not assumed: `DEFAULT_HOST_PLATFORM_RULES_WINDOWS` / `_LINUX` / `_MACOS` and `docs/agents/host-platform-rules.md` contribute **zero** lines to any of AC1's three base sweeps at `df494bfa`, so Rule R never reaches them. The docs sweep is 563 at both SHAs over the same 64 files, which is the check.

**The Rust base sweep grows by 14 lines and loses none.** Measured at both SHAs with the §9.4 AC1 commands, keyed on `(path, trimmed content)`: **zero** base rows are absent at `df494bfa`, and the +14 decomposes over **six** distinct keys, not three. **Round 6 corrects the decomposition**, because rounds 5's wording ("three unique rows are new, contributing 14 lines") merges two different things and an implementer following it would append two rows that are already on Part A:

| Key | Occurrences | Lines | Class | Step 0b |
| --- | --- | --- | --- | --- |
| `seeded_context_templates.rs` `- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and \`repo-*\` working repos.` (the third occurrence is inside `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES`) | 2 to 3 | +1 | `P3-frozen` | **already on Part A. Do not append** |
| `session_context.rs` `"C:/fake/wg-7-dev-team/__agent_architect",` | 22 to 28 | +6 | `P2-fixture` | **already on Part A. Do not append** |
| `session_context.rs` `Path::new("C:/fake/wg-7-dev-team/__agent_architect"),` | 1 to 4 | +3 | `P2-fixture` | **already on Part A. Do not append** |
| `session_context.rs` `let replica = ac.join("wg-1-team").join("__agent_dev");` | 0 to 2 | +2 | `P2-fixture` | **APPEND** |
| `session_context.rs` `"the platform block must render in the WG profile"` | 0 to 1 | +1 | `P2-fixture` | **APPEND** |
| `session_context.rs` `// #1605: the platform block renders in the WG profile on every OS (linux/` | 0 to 1 | +1 | `P1-comment` | **APPEND** |

So the append at step 0b is **exactly the last three keys**, three rows contributing 4 lines. The first three are pre-existing Part A rows whose occurrence count rises, contributing 10 lines and needing no edit at all, because Part A is keyed on `(path, trimmed content)` and an occurrence increase changes no key. Round 5's conclusion ("an append of three rows") was right; its reason was not, and `ac-dev-rust-v3` found the difference.

**Thirteen of the fourteen added lines are inside `#[cfg(test)]`. The fourteenth is production, and round 5 said otherwise twice.** Measured: the thirteen are all in `session_context.rs` at `:4896` and above, inside its `#[cfg(test)] mod tests` and `#[cfg(test)] mod token_accounting`. The fourteenth is **`seeded_context_templates.rs:220`** (at `df494bfa`), the third Core Concepts occurrence, inside `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES` declared at `:213`; that file's **only** `#[cfg(test)]` is at `:2093`, so `:220` is production code. `ac-dev-rust-grinch-v3` found this. It is correctly classified `P3-frozen` above and correctly preserved, and nothing about the strategy moves, but §3.14 carried its inner numbers on the strength of the false half and is corrected there: **1175 / 63 files, 582 reachable, 593 blind**.

**The line-number rule, corrected in round 6, because round 5's was false for the one file that matters.** Round 5 wrote "every line number in this plan is at `df494bfa` unless the row or sentence says otherwise", and licensed carrying `d7008b34` numbers only "where `df494bfa` did not move the file". `session_context.rs` moved, by this plan's own +43 to +64. `ac-dev-rust-v3` measured the consequence and `ac-dev-rust-grinch-v3` confirmed it: every `session_context.rs` citation outside the four places named below resolves at `d7008b34` and lands on unrelated content at `df494bfa`. The corrected rule, measured over every `<file>:<line>` citation in this document:

**The predicate is the rule; the exception column is illustrative.** Round 7 states this explicitly, because `ac-dev-rust-v3` observed that an enumeration sitting beside a predicate is exactly the shape K1 was about, and that this column omits four exceptions it could name. **The operative rule, in both directions: a citation is read at the base its own row or its own visible local label names, and a number carrying no label follows its file's row below.** Every exception carries such a label at its point of use, so the predicate resolves all of them; the column names the ones a reviewer is most likely to trip over, and a label the column omits is a finding against **this** column, never against the citation.

| File | Base its line numbers are at | Exceptions, illustrative and not closed |
| --- | --- | --- |
| `session_context.rs` | **`d7008b34`** by default, which is **31** of the 37 distinct explicit `<file>:<line>` citations. Verified mechanically over all 37: **37 of 37 resolve to different content at `df494bfa`**, so no number in this file is safe to read at the wrong base, whichever base it belongs to | §3.12 Tables A and B (line **ranges**, labelled `df494bfa`); §15.3, which quotes `df494bfa` 2513-2537 explicitly. Three explicit citations are at branch head **`bb2a5a65`** and are labelled there: §5.9's `:3649` (with its bare `:3658`) and §15.2's `:1539`. Three more are at **`df494bfa`** and are labelled there: §3.7 family 1's `:2512` and `:2553` (under a table stamped `df494bfa`) and §10.2 R14's `:10615` (labelled inline). §3.12 Table C's `:10578` is a `bb2a5a65` anchor, labelled inline |
| `seeded_context_templates.rs` | **`df494bfa`** everywhere it is cited: §3.7 family 1's table (re-anchored in round 6) and §9.3 limb B's table (already labelled `@ df494bfa`) | none |
| `seed_manifest.rs` | **`df494bfa`**, re-anchored in round 6 (§3.14's two rows moved from `:1339`/`:3447` to `:1346`/`:3454`) | none |
| every other file this plan cites | identical at both bases | `config/settings.rs` drifted only from `:4413` onward and this plan cites `:80`; `root_agent.rs`, `injected_messages.rs`, `entity_creation.rs` and every frontend file are byte-identical at both bases. `§15.2`'s `WorkgroupTask.tsx:71` is a `bb2a5a65` number and is labelled there |

A number carrying no label follows its file's row above. Byte-level evidence (§3.12) is taken from `git cat-file blob <sha>:<path>`, never from the working tree, because `core.autocrlf` is `true` and `*.md` / `*.tsx` / `*.ts` carry no `.gitattributes` `eol` rule, so a worktree digest is not reproducible for those. **If a line number no longer matches the quoted text, re-anchor on the quoted text, never on the number.** That instruction is load-bearing this round: the re-base shifts `session_context.rs` by +43 to +64 lines depending on the region, and every §3.12 line range in that file moved.

### 1.2 Entry ritual for the implementer

**Round 5 replaces this ritual, because the branch already carries 12 commits and the base has moved.** The round-1 to round-4 form asserted a virgin branch at a single SHA; that is no longer the situation and asserting it would STOP unconditionally. Inside `repo-AgentsCommander`, before the first edit of this round:

1. `git -C <repo> fetch origin main`.
2. `git -C <repo> rev-parse --abbrev-ref HEAD` must print `refactor/1614-workgroup-to-room-phase-1`.
3. `git -C <repo> status --porcelain` must be empty.
4. `git -C <repo> rev-parse origin/main` must print `df494bfa04f7e14fa9a42f3b0d89ccbc2ce76e80`. **If it differs, STOP and request a §13.5 review; do not re-pin the plan yourself.** Round 4 shipped because that call was made by the implementer, and §13.5 makes it the tech lead's.
5. `git -C <repo> merge-base --is-ancestor d7008b34e155a8bd6481be5feecfc7d96575328f HEAD` must exit 0, that is, the branch still contains the superseded base and its 12 commits are intact.
6. Re-hash this plan file from the blob (`git cat-file blob HEAD:plans/1614-workgroup-to-room-phase-1.md | sha256sum`) and confirm it equals the round-5 `Plan-SHA256` the tech lead approved. Do not take the digest from the worktree.
7. Then, and only then, **step -1 of §12**: merge `origin/main` into the branch. Merge, never rebase (§13.4), so the head keeps containing `origin/main` and the 12 commits keep their identity.

Everything after step -1 runs against the merged tree at the new base. No product edit of this round happens before that merge, because every corrected value in this round is defined at `df494bfa`.

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

**This table is at `df494bfa`, re-derived in round 6.** Rounds 1 to 5 carried it at `d7008b34` with no label, which put a **wrong `global` version** in the one section a reviewer diffs against the merged tree: it read `global` = 4, and `df494bfa` is 5. Both reviewers independently reported that a reviewer would be misled, and `ac-dev-rust-grinch-v3` added the sharper half: the old anchors do not merely go stale, they land on *other specs*, so `:553` reads `is_known_generated: is_known_generated_platform_windows` and `:561` reads `current_version: 1` at the new base. Bare numbers below are `seeded_context_templates.rs`.

| Spec | `id` | `current_version` @ `df494bfa` | Current content | Recognizer |
| --- | --- | --- | --- | --- |
| project | `global` | **5** (`:517`) | `session_context::get_default_agent_template` (`session_context.rs:2512`) | `is_known_generated_global_template` (`:613`) **and** `is_known_generated_standalone_global_template` (`:632`) |
| project | `coordinator` | 5 (`:527`) | `session_context::get_default_coordinator_template` (`session_context.rs:2553`) | `is_known_generated_coordinator_template` (`:658`) |
| root | `rootAgent` | 7 (`:585`) | `root_agent::default_root_context_template` returning `ROOT_ROLE_MD` (`root_agent.rs:675`) | `is_known_generated_root_context_template` (`root_agent.rs:729`) |

**Two things a reviewer must read with this table.** First, `global` is **5** at the frozen base and must be **6** after the merge (§1.1's collision table, §5.10 D8a, AC7.11, §15.3); the `d7008b34` value of 4 appears nowhere in this table any more. Second, the table names **the three specs this plan touches**; `df494bfa` carries **six**, because #1605 added `platform.windows` (`:551`), `platform.linux` (`:561`) and `platform.macos` (`:571`), all at `current_version: 1`. Their disposition is decided and measured in §5.10 D8a, not omitted: they take no snapshot and no bump because they contribute zero lines to any AC1 base sweep. Both reviewers accepted that disposition and neither asked for the table to grow to six rows.

All three carry text this rename must change:

- `get_default_agent_template()` carries exactly one occurrence, the Core Concepts line `- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and \`repo-*\` working repos.`
- `get_default_coordinator_template()` carries exactly one occurrence, `- To reach another workgroup, message its orchestrator, never its members, ...`
- `ROOT_ROLE_MD` carries six, at absolute lines 687, 698, 702, 704, 710 and 712.

`STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS` proves the global template has two independent recognizers by design (`:624-635` at `df494bfa` documents why root retirement must not widen the project one), so the new global snapshot must be wired into **both**.

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

**Three further groups of frontend line move without being Rule R, and §9.4 AC1 subtracts them too.** They are named here because content is the allowlist key, so a line whose content this plan changes must not sit in a base-derived allowlist: the **four** R1 resolvers `ProjectPanel.tsx:1057`, `:1058`, `:1071` and **`:2749`** (the trap restated below), and the five §5.4 prefix-predicate lines not already inside the 55, namely `profile-utils.ts:124`, `:472`, `WorkgroupGroupRail.tsx:67`, `:72` and `WorkgroupTask.tsx:74`. All nine were likewise verified present in the base sweep. **Round 6 adds a tenth line to this group**, `WorkgroupTask.tsx:70`, the Rule P1 clause (b) comment §15.2 row 6 mandates correcting; it is in the 906, its correction drops its `wg-`, and the sentence above ("a line whose content this plan changes must not sit in a base-derived allowlist") is exactly why it belongs here. **65 frontend lines therefore move in total, and the base allowlist's frontend half is `906 - 65 = 841` lines, which is 775 rows** (§9.4 AC1 point 8). **Lines, not rows**: the two are different numbers and rounds 1 to 5 wrote "rows" here for a line count.

**`ProjectPanel.tsx:2749` is new in round 5 and it is the line that made AC1 point 9 predict 864 against a measured 863.** Rounds 1 to 4 enumerated three R1 resolvers and §5.2's R1 clause called the `:993`/`:995` pair "the only R1 instance". It is not. `:2749` is

```tsx
<Show when={sessionsStore.showCategories && (!filterActive() || matchesFilterText("Workgroups") || filteredWorkgroups().length > 0)}>
```

which gates the **whole category section**. Left alone while `:995` renames, the section hides itself even when its own row label matches the filter, which is exactly the R1 defect, one scope wider than the three the plan had. `ac-dev-rust-v3` found it during implementation and moved it with the other three; `ac-dev-rust-grinch-v3` confirmed it is in the base sweep and classified `P0-identifier` in the committed Part A allowlist, which is what made it come back as a **missing** line rather than an unlisted one.

**The R1 set is now closed by literal, not by enumeration, so a fifth cannot hide.** Swept at `d7008b34` and re-checked at `df494bfa` across `src`, `docs` and `src-tauri` for the two rendered labels as exact quoted literals: `"Workgroups"` occurs in production at `ProjectPanel.tsx:995` (the label), `:1057`, `:1058` and `:2749` (resolvers); `"Selected Workgroup"` occurs at `:993` (the label) and `:1071` (one line carrying two resolver occurrences). **Six production lines in one file, and no other production file carries either literal.** The remaining occurrences are 14 lines across three `*.test.tsx` files (`ProjectPanel.collapse-state.test.tsx`, `ProjectPanel.regex-filter.test.tsx`, `WorkgroupGroupRail.autofocus.test.tsx`); the frontend sweep excludes `.test.tsx`, so they are outside AC1 entirely, and the 10 of them that are assertions rather than comments move under §9.3 clause 1 while the 4 that are `//` comments stay under Rule P1.

#### The one trap that survives from round 1, restated

`ProjectPanel.tsx:993` and `:995` produce the row labels `"Selected Workgroup"` and `"Workgroups"`, and `:1057`, `:1058`, `:1071` and **`:2749`** pass those **same literals** back into `matchesFilterText(...)` and `workgroupMatches(wg, ...)` as the search text the sidebar filter matches against. §5.2 clause R1 covers it: all six lines move together, in the same commit, or the filter silently stops matching the row it names. `:2749` is the widest of the four resolvers, because it gates the section's visibility rather than its contents.

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

### 3.12 Byte evidence for every frozen copy this plan makes (computed, not assumed; re-derived at `df494bfa` in round 5)

Every value below is taken from the git blob (LF), never from the CRLF worktree. Each row carries the SHA it was taken at, per §1.1's labelling rule.

**Round 5 corrects the stated reason, which was false and backwards (P11).** Rounds 1 to 4 said `*.rs` carries no `.gitattributes` `eol` rule. It does: `.gitattributes:5` is `*.rs\ttext eol=lf`, and with `eol=lf` a `.rs` **worktree** digest would in fact reproduce. The correct reason to read from the blob anyway is that it is the only form a reviewer can re-run without a checkout, and that it is uniform with the `*.md` / `*.tsx` / `*.ts` rows, where §1.1's claim is true and the worktree genuinely is not reproducible. **The method and every value below are unaffected**, which both reviewers verified independently in round 4 by reproducing all nine Table A digests from the blob.

Two kinds of value appear, and the difference is load-bearing:

- a **declaration-range digest** covers the source lines including the `const ... = r#"` opener and the `"#;` closer. It proves the implementer copied the base literal rather than retyping it. It is checkable by a reviewer with `git cat-file blob` and `awk`, and it is what §9.4 AC7's copy check uses.
- a **rendered-value digest** covers the bytes the constant evaluates to. It proves the *behavior* is preserved even if the declaration is reindented, and it is what a `#[test]` can assert directly with `Sha256::digest(CONST.as_bytes())`. The repository already uses exactly this form: `root_context_pre_orchestrator_rename_snapshot_is_byte_exact` (`root_agent.rs:2411`, body `:2410-2426`) pins `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD` at 2464 bytes and sha256 `e244249c...`, with a doc comment stating the value was "captured by a one-off run of the shipped constant AT ecc6527b ... never from this const". This plan follows that precedent for every new snapshot.

#### Table A: declaration-range digests

| # | Snapshot source | Blob path | Line range @ `df494bfa` | (was @ `d7008b34`) | Bytes | SHA-256 | Taken at |
| --- | --- | --- | --- | --- | --- | --- | --- |
| A1 | Global template declaration, `    r#"` through `"#` | `src-tauri/src/config/session_context.rs` | **2513-2537** | 2470-2492 | **574** | **`D9E9358299E811FBFC4652530A8046CA3B4F02C0BFF38826C7A8CEE70B73A844`** | **`df494bfa`** |
| A2 | Coordinator template declaration | same | **2554-2574** | 2509-2529 | 2703 | `CC127468024A85C5863F693E770AEE9BBED82E61C3D28A7DF70B521877563ABF` | `d7008b34`, reproduced at `df494bfa` |
| A3 | `ROOT_ROLE_MD` declaration, `const ... = r#"` through `"#;` | `src-tauri/src/config/root_agent.rs` | 675-723 | 675-723 | 2501 | `9713681065D83A8A73B05F07970C3176BB19E6914FDB1A1EE4DCD317AA8CA095` | `d7008b34`, file untouched by the drift |
| A4 | **`WORKGROUP_GIT_SCOPE` declaration** (new, §5.10 split) | `src-tauri/src/config/session_context.rs` | **3432-3432** | 3368-3368 | 258 | `85876201638A24F13FAD76B7AEE0429489C785DE6EDC993798C59701DEE47451` | `d7008b34`, reproduced at `df494bfa` |
| A5 | **`ROOT_COORDINATION_MESSAGING_PARAGRAPH` declaration** (new, §5.10 split) | `src-tauri/src/config/root_agent.rs` | 292-306 | 292-306 | 956 | `17D7303AB17923357842B7D6FF24B921CB8B6D027BBB06F4BC3A07E57177E517` | `d7008b34`, file untouched by the drift |
| A6 | **`OLD_DEFERRED_MESSAGING_PARAGRAPH` declaration** (new, frozen in place) | `src-tauri/src/config/root_agent.rs` | 290-290 | 290-290 | 344 | `3DC963DB7B12E24D522BC981DB364E5E2BA1A656BF0EFE0DF23F25E86DB34E93` | `d7008b34`, file untouched by the drift |
| A7 | **`legacy_rendered_default_context_for_generation`, whole function** (new, frozen in place, §3.7 family 3) | `src-tauri/src/config/session_context.rs` | **3897-4153** | 3769-4025 | 15027 | `940FA35733C78CDF513391E5AED64438AFD50FE6472A26E4D317270E5EE716C2` | `d7008b34`, reproduced at `df494bfa` |
| A8 | **`LEGACY_GIT_SCOPE_*_BEFORE_1072` pair** (already frozen; in family 3's closure) | `src-tauri/src/config/session_context.rs` | **3863-3864** | 3735-3736 | 1289 | `DADA740FD5EE0EF3141E3AEE6C3920074F83776B2051F355C6D1B387781D0421` | `d7008b34`, reproduced at `df494bfa` |
| A9 | **`render_skills_section`'s replica line** (new in round 3, §5.10 D8f, §3.7 family 3) | `src-tauri/src/config/session_context.rs` | **874-874** | 831-831 | 152 | `A9DC92441D915A1251CFC148431D87EEC2C7430A2BEC3AAA1714B9CD978CAFD3` | `d7008b34`, reproduced at `df494bfa` |

Reproduce any row with:

```
git cat-file blob <sha>:<path> | awk 'NR>=<a> && NR<=<b>' | sha256sum
```

**Only A1 changed value.** A2, A4, A7, A8 and A9 were each re-run at `df494bfa` over the shifted range and returned the identical digest and identical byte count; A3, A5 and A6 live in `root_agent.rs`, which is not in the drift's changed-path set, so neither their values nor their line numbers move. **A reviewer should re-run all nine at `df494bfa` and confirm eight reproduce and one is A1's new value**; that is a cheaper and stronger check than trusting this paragraph.

**A1 is the whole of #1605's collision with this plan's frozen evidence.** #1605 inserted `{{HOST_PLATFORM_RULES}}` and its blank line into the global template body, which is why the declaration grows from 23 lines / 549 bytes to 25 lines / 574 bytes (`549 + 24 + 1 = 574`). The consequence is stated once, in §1.1's re-base ledger, and carried into §5.10 D8a, AC7.1 and §9.3: the constant this plan freezes must hold the **v5** body, not the v4 body `main` already froze under its own name.

#### Table B: rendered-value digests

These are the values a Rust test asserts. Round 1 declined to compute the coordinator one, calling it undecodable without a Rust string-literal decoder; it is supplied here, decoded twice by two independently written decoders (a JavaScript one and a Python one taking a different route: regex line-continuation collapse followed by the host language's own literal parser) that agree byte for byte. The same JavaScript decoder reproduces the global and root rendered digests below, which are independently verifiable as raw literals, so the decode path is validated against known-good values before being trusted on the escaped one.

| # | Value | Source @ `df494bfa` | Bytes | SHA-256 | Taken at |
| --- | --- | --- | --- | --- | --- |
| B1 | `get_default_agent_template()` | `session_context.rs` **2513** after `r#"` through **2536**, plus the final newline | **564** | **`D094106B386172E714512DBE1D18CC30A82FF2B25DF467F3A1BE1C328D464F77`** | **`df494bfa`** |
| B2 | `get_default_coordinator_template()` | `session_context.rs` **2554-2574**, escapes and `\`-continuations decoded | **2516** | `0B89EB38608F6272F0D8087FC7DF13ECC729FDA716ABA972673B15B734A2198E` | `d7008b34`, declaration reproduced at `df494bfa` |
| B3 | `ROOT_ROLE_MD` | `root_agent.rs` 675 after `r#"` through 722, plus the final newline | 2467 | `7F82F28C70221C8476BB957F5978433173F60E388A9F18DB729E5C2BF014C52D` | `d7008b34`, file untouched by the drift |
| B4 | `WORKGROUP_GIT_SCOPE` | `session_context.rs:`**`3432`** string value | 220 | `A386B52DA8246826689215A8F07ABF3CB58D01EBCC18AFC530730157AA12566D` | `d7008b34`, reproduced at `df494bfa` |
| B5 | `ROOT_COORDINATION_MESSAGING_PARAGRAPH` | `root_agent.rs` 292 after `r#"` through 306 before `"#;` | 897 | `FC2164A2A56957E481DEBCA460F9DF3CC681A634EDDA58F5270939C85668F207` | `d7008b34`, file untouched by the drift |
| B6 | `OLD_DEFERRED_MESSAGING_PARAGRAPH` | `root_agent.rs:290` string value | 293 | `6E12E68E51C3C6DF2386728DFD0ED98BFE06A8A0C3F6383BFAF8FD4463C7A463` | `d7008b34`, file untouched by the drift |
| B7 | `GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME` (new in round 3, D8f) | `session_context.rs:`**`874`** string value, the single `\n` escape decoded | 131 | `A5C74FD65F2C2562D4C651F1C6E972684DE9F2DBE1924E6153B26D4CB9C57EC9` | `d7008b34`, reproduced at `df494bfa` |

**Only B1 changed value, and B1 changed for exactly the same reason A1 did.** `564 = 539 + 25`, the `{{HOST_PLATFORM_RULES}}` line (24 bytes with its newline) plus the blank line that separates it. Reproduce B1 with:

```
git cat-file blob df494bfa:src-tauri/src/config/session_context.rs \
  | awk 'NR>=2513 && NR<=2536' | sed '1s/^    r#"//' | sha256sum
```

The same command over `d7008b34` with `2470`/`2491` returns `f4406596...` at 539 bytes, which is the control that validates the extraction; a reviewer should run both.

**The last row names the frozen constant, not the live one, and round 4 corrected its label.** Round 3 wrote it as `GENERATED_SKILLS_SECTION_REPLICA_LINE`, which is the **post**-rename constant: after D8f, the constant of that name holds the Room text and is 126 bytes, while these 131 bytes are the **pre**-rename text that D8f freezes as `GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME`. The `WORKGROUP_GIT_SCOPE` row above follows a base-tree-name convention, but that convention does not carry here, because the frozen constant does not exist at `d7008b34` while `WORKGROUP_GIT_SCOPE` does. The `Source` column and both consumers (D8f step 2, and §9.1's `skills_section_replica_line_split_is_correct` and AC7.14) already bound the digest to the frozen half correctly, so nothing downstream moves. `ac-dev-rust-grinch-v3` found this in round 3.

The round-3 row is the decoded value of the one-line literal at `:831`: 131 bytes, 131 characters, one `\n` escape and no other escape, ending in that newline. **The trailing newline is part of the constant and part of the digest**; a copy that drops it hashes differently and `is_provably_generated_legacy_skills_section` stops matching. Reproduce with:

```
printf 'When running from a workgroup replica, resolve skills/... against the origin Agent Matrix path above, not against the replica CWD.\n' | sha256sum
```

**The coordinator row is the one value in this plan that was computed rather than read.** If the implementer's first run of `assert_eq!(COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len(), 2516)` reports a different length, the decode above was wrong, and **the declaration-range digest `CC127468...` governs**: the copy is correct if and only if the declaration range matches, and the implementer then records the observed length and digest in the test and in the PR body, citing this paragraph. Both reviewers reproduced `CC127468...` independently in round 1, so that authority is already established.

#### Table C: the one value this plan cannot compute, and how it is captured

The rendered output of the two legacy reconstructions cannot be derived from source without executing them, and reimplementing 250 lines of Rust `format!` logic by hand would risk writing a wrong digest into a plan. They are therefore captured, not asserted here.

**Round 3 captures both reconstructions, not one.** Round 2 captured only `legacy_rendered_default_context_for_compat`. The `StaleGenerated` arm of `classify_legacy_rendered_default_context` accepts **either** candidate that `reconstruct_legacy_rendered_default_context` returns at `:4152-4163`, and the pre-#1072 one is the candidate that actually carries the `StaleGenerated` outcome, so pinning only the first leaves half the freeze unpinned.

**Round 5: both values are now known and are no longer deferred.** `ac-dev-rust-v3` captured them at step 0 at `d7008b34` before the first product edit, exactly as this section directed, and `ac-dev-rust-grinch-v3` verified the committed test asserts them and passes. They are written here so that the plan carries them and the capture cannot be redone differently:

| # | Function | Fixed inputs | Bytes | SHA-256 | Captured at |
| --- | --- | --- | --- | --- | --- |
| C1 | `legacy_rendered_default_context_for_compat` | as below | **8718** | `daf07cbc24e46988747385f1d622c0b3309d29fa1fda77964afb2892ef85275c` | `d7008b34` |
| C2 | `pre_1072_legacy_rendered_default_context_for_compat` | the same three | **9096** | `24b2c24ddef33c307fbb64734eee05c5baf60c2b6616e21ef1e56fc0f33999ef` | `d7008b34` |

Both values are transcribed here **from the committed test**, `legacy_rendered_default_context_is_frozen` at `session_context.rs:10578` at branch head `bb2a5a65` (the lengths at `:10588` and `:10605`, the digests at `:10594` and `:10611`), not from a report. The committed test remains authoritative: if this plan and the test ever disagree, the test governs and the disagreement is a finding to report.

- **Round 5 replaces the capture with a re-verification.** The capture already happened. What step 0 now does, at the **new** base and after the merge, is re-run the committed `legacy_rendered_default_context_is_frozen` and confirm it is still green. That is the whole gate. It should be green, and the reason is structural: A7, A8, A4 and A9 all reproduce byte-identically at `df494bfa`, so every byte the two reconstructions read is unmoved. **If it is red, STOP**: something in family 3's closure moved that this plan did not account for, and that is a §13.5 finding, not a value to update.
- **How the values were captured, retained for the record.** `legacy_rendered_default_context_is_frozen` (§9.1) was added with two deliberately wrong expected values, run once, and the actual lengths and digests read out of the assertion failures. They were then set and recorded, with the base SHA, in the test's doc comment and in the PR body.
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

At `d7008b34` this returns **3090 lines across 94 files**. Narrowed to production reader-facing candidates, that is **1174 lines across 63 files**, of which 582 are reachable by the line-based rule below and 592 are in its blind class.

**The re-base moves the outer number, and moves the inner one by exactly one line. Round 6 corrects this; round 5 said the inner numbers did not move at all and gave a false reason.** At `df494bfa` the same raw sweep returns **3104 lines across the same 94 files** (§1.1). Round 5 wrote "all 14 added lines are inside `#[cfg(test)]` modules ... and step 1 of the narrowing rule drops every line inside a `#[cfg(test)]` item". **Thirteen are; the fourteenth is not.** `seeded_context_templates.rs:220` is production code inside `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES` (declared `:213`; the file's only `#[cfg(test)]` is at `:2093`). Trace it through the rule below: step 1 does not drop it (production), step 2 does not drop it (not a `//` comment), step 3 does not keep it (a raw-literal line carrying no `"`). It lands in the **blind class**. The corrected production narrowing at `df494bfa` is therefore **1175 / 63 files, 582 reachable, 593 blind**, and 582 + 593 = 1175 still closes. `ac-dev-rust-grinch-v3` found this.

**Nothing §6.1 depends on moves**, and that is the reason this is a correction rather than a re-derivation: the **file** counts hold (`seeded_context_templates.rs` already contributes blind-class lines, so no file enters or leaves either list), the 55-file candidate set is unchanged, §6.1's 41 edited and 16 preserved are unchanged, and the row is correctly classified `P3-frozen` in §1.1 and correctly preserved. What was wrong was a measured claim inside the section round 5 added to make the re-base auditable, which is exactly the kind a reviewer checks. The AC1 base sweep in §9.4 uses the **outer** number and is therefore 3104; §3.14 and §6.1 use the **inner** ones and are therefore untouched. Confusing the two is the specific error to avoid. Round 2 stated this inner figure as 544 across 55; the file count was right and the line count was an artifact of step 1, as the correction after the rule sets out. The narrowing rule, stated so it is reproducible:

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
| `config/seed_manifest.rs:1346` | `"config scope must be config:.ac/<workgroup>/<replica>/<dest>: {scope}"` | seed-manifest validation error |
| `config/seed_manifest.rs:3454` | `"lifecycle config prefix must include a workgroup component"` | same |
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

**P1.** A source comment or doc comment is preserved, **except** (a) a doc comment `clap` compiles into printed help, and (b) a comment that would contradict the code it documents after this change. The closed list of shapes where `clap` compiles a doc comment into printed help: a `#[derive(Subcommand)]` variant, a `#[derive(Args)]` or `#[derive(Parser)]` struct, a `#[derive(Args)]` field, and a `#[derive(ValueEnum)]` variant. This carve-out exists because #1571 shipped a round-7 blocker precisely here: five `///` lines that no enumeration named were printed help twice each. **Clause (b) is a predicate, not a list**, for the reason this document states elsewhere: an enumeration cannot be the gate for a defect class whose failure mode is omission. §3.14's last paragraph enumerates the clause (b) corrections **visible at the base**, and that enumeration is illustrative. A comment that contradicts the code it documents after this change is corrected whether or not that paragraph names it, and a correction the paragraph omits is a finding against the paragraph, never against the correction. **Two such corrections are already on the branch at `bb2a5a65`, and rounds 1 to 7 named neither**: `cli/list_peers.rs:910`, the comment on §3.2's `:907` and `:912` gate lines (P2 and S1), and `commands/entity_creation.rs:2953`, the doc comment on P12's gate at `:2964`. Both are authorized by this clause, and §9.4 AC1 point 10's self-test names both Part A rows with their disposition. **§3.14's last paragraph closes by calling its six the only doc comments in the Rust diff, and `:2953` refutes that**; the claim is superseded here rather than edited there, because §3.14's bytes are evidence three parties have certified and the correction belongs with the rule. `ac-dev-rust-grinch-v3` found both sites by running §9.4 AC1 point 10's reverse limb against `bb2a5a65`, which is what makes this clause measurable rather than asserted.

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

**P4 (new in round 2; its factual premise corrected and a carve-out added in round 5).** `log::trace!` / `debug!` / `info!` / `warn!` / `error!` message text is preserved. It is developer diagnostics keyed by a bracketed target tag operators grep for, not a product surface, and it is not covered by requirement (G)'s carriers.

**P4a, the boundary.** If the same string is also returned to a caller or rendered, the returning or rendering site takes Rule R and the log line does not.

**P4b, the carve-out, new in round 5.** When one string literal appears **both** in a `log::` macro **and** in a user-visible carrier (`eprintln!`, `println!`, a returned error, a rendered string) as the two arms of a single branch, so that the two arms are alternative spellings of one message to one reader, **both arms take Rule R together**. Renaming only the visible arm would leave one message with two spellings depending on an environment variable, which is worse than either uniform outcome and is exactly what D18 exists to prevent.

**Round 5 deletes a false claim that rounds 2 to 4 carried.** P4 said: "No such shared string exists at `d7008b34`; the check is `git grep` over the §3.14 inventory and it came back empty." **That is refuted.** `cli/list_peers.rs:654` and `:659` are exactly such a pair at `d7008b34`, and they are still such a pair at `df494bfa`: the identical literal `"Warning: no orchestrator found for WG '{}', showing all replicas"` appears at `:654` inside `eprintln!` and at `:659` inside `log::warn!`, as the two arms of one `if std::env::var("AC_MACHINE_OUTPUT").is_err()` branch (`:651-662`). P4b is written for that pair and renames both. `ac-dev-rust-grinch-v3` found this during implementation review; it is the only instance measured, and P4b rather than an enumeration is what makes the finding safe, because P4b is a rule and a second such pair needs no plan revision.

**R1 (a clause of Rule P, not an exception to it).** When the only thing that resolves an occurrence is **another occurrence of the same literal inside this change's own edit set**, and both sides move in the same commit, the occurrence is renamed. A test expectation is never itself a resolver: an occurrence pinned only by an assertion is classified by what the pinned literal names, and the assertion moves with it under §9.3.

**The R1 instance, corrected in round 5.** Rounds 1 to 4 said the `:993`/`:995` pair against `:1057`/`:1058`/`:1071` was "the only R1 instance". It is not: there is a **fourth** resolver, `ProjectPanel.tsx:2749`, and §3.8 now carries it with the sweep that closes the set by literal rather than by enumeration. The instance is therefore `ProjectPanel.tsx:993`/`:995` against `:1057`, `:1058`, `:1071` and **`:2749`**, six production lines in one file, and no other production file in the repository carries either label literal.

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
| `session_context.rs:3536` | **edit** — live renderer (`default_context_dynamic_values`, 3477-3677) | `` YYYYMMDD-HHMMSS-<roomN>-<you>-to-<roomN>-<peer>-<slug>.md `` followed by the exact clause `` (legacy: `<wgN>`) ``. **The clause text changed in round 5; see "The legacy clause" below.** |
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

The walk-up sentence `` walk up from your root to the parent `wg-<N>-*` folder `` becomes `` the parent `room-<N>-*` folder (or `wg-<N>-*`) `` **in the live renderer only**; the same sentence inside the frozen `legacy_rendered_default_context_for_generation` range is frozen. **The trailing "in a legacy Workgroup" is removed in round 5, for the same reason the legacy clause changed; see below.**

#### The legacy clause: decided in round 5, because rounds 1 to 4 specified an unsatisfiable one

Rounds 1 to 4 required the exact clause `` (a replica in a legacy Workgroup uses `<wgN>`) ``. **That clause cannot be implemented.** Two independent contradictions, either one sufficient:

**(a) It contradicts AC7.13, and this needs no measurement.** AC7.13 requires the rendered live context for a `room-1-t` replica to contain **no case-insensitive `workgroup`**. The required clause contains "Workgroup". §5.9 places it in the live renderer, which a `room-1-t` replica renders. Two clauses of the same plan cannot both hold. This is a straightforward drafting contradiction and it is the architect's to own: AC7.13 was written in round 1 and §5.9's clause text in round 2, and no round checked them against each other.

**(b) It overran a byte ceiling this plan elsewhere refuses to move.** Implemented verbatim at the old base, the clause pushed `touched_owners` to about **6897** against `summarized_default_context_meets_size_budget`'s `MAX_TOUCHED_OWNERS_BYTES` of **6810** (46 characters into the write-restrictions block, widening the messaging clause from 17 to 46, and widening the walk-up parenthetical from 25 to 37). D8b explicitly refuses to move that class of ceiling, calling it scope drift.

**The decision: the clause is `` (legacy: `<wgN>`) ``.** It carries the same fact, that a legacy entity's token is `<wgN>`, without the forbidden word, and the write-restrictions block keeps its `<roomN>` form. `ac-dev-rust-v3` proposed this substitute during implementation and asked for confirmation rather than assuming it; `ac-dev-rust-grinch-v3` judged it meaning-preserving and would accept it on the merits, and so would this plan. **But an implementer is not authorized to decide it, which is why it is decided here and not left standing as an implementation note.**

**The clause is stated twice, not once, and both statements are required.** `session_context.rs:3649` is the Root Agent arm and `:3658` is the replica arm. **Both numbers are at branch head `bb2a5a65`, not at either base**, and they are correct there: they anchor the text the branch already wrote, which is what this subsection is deciding about (§1.1's line-number table records the exception). They are alternative arms of one profile, so exactly one of them renders in any given materialization, but both are in the source and both take the substituted text. The implementer's report described it as "stated once", which is a fair reading of "once per rendered profile" and a loose reading of the source. Six string literals carry the substitution in total and it is trivially reversible.

**(b) is relieved by the round-5 re-base, and this must be recorded rather than quietly dropped.** #1605 shortened three of the five blocks `touched_owners` sums: `DEFAULT_CLI_CONTEXT` by 38 bytes across three sentences, the `render_inter_agent_messaging_block` body by 72 across two, and `WINDOWS_SHELL_ROUTING` from 201 bytes to 49 as its Git Bash recipe moved into the new `{{HOST_PLATFORM_RULES}}` block. **That is 262 bytes returned to the budget on Windows**, derived from the `d7008b34..df494bfa` diff of `session_context.rs` and stated as a derivation, not as a `token_accounting_report` measurement. So the size half of the conflict would no longer bind on its own.

**The decision does not change because of that.** Contradiction (a) is independent of size and still holds: a Room replica's rendered live context may not contain a case-insensitive `workgroup`, and the round-1 clause does. `` (legacy: `<wgN>`) `` stands. What (b)'s relief does change is §10.2's residual, which is corrected there: the branch's measured 6809-against-6810 was taken at the **old** base and does not carry to the merged tree. **The implementer re-measures `touched_owners` post-merge with the repository's own `token_accounting_report` and records the actual figure in the PR body.** A derived 262 is good enough to lift a blocker; it is not good enough to be the number a future editor budgets against.

**§5.9 and §5.10 no longer overlap.** §5.9 owns `phone/messaging.rs` and the live-renderer ranges of `config/session_context.rs`. §5.10 owns every frozen snapshot and every split, in both `config/session_context.rs` and `config/root_agent.rs`. **No line of `config/root_agent.rs` is edited by §5.9.** Round 1 had §5.9 naming `:297` while §5.10 scoped root edits to six lines in `ROOT_ROLE_MD`; that contradiction is resolved in favour of §5.10, which is the section that owns freezing.

The two live assertions that pin the renderer strings, `session_context.rs:5282` (`YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md`) and `:5490` (`YYYYMMDD-HHMMSS-root-to-<wgN>-<orchestrator>-<slug>.md`), move under §9.3. Neither asserts anything about the frozen function.

### 5.10 D8: frozen snapshots, dual-use splits, and the version bumps

This section owns every freeze and every split, in `config/session_context.rs`, `config/root_agent.rs` and `config/seeded_context_templates.rs`. No other section edits a frozen item.

#### D8a. Three new seeded-template snapshots and three version bumps

Exactly as #1571 did for the orchestrator rename, and for the same recognizer mechanism (§3.7 family 1).

| New constant | File | Value | Wired into | Version bump |
| --- | --- | --- | --- | --- |
| `GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME` | `config/seeded_context_templates.rs` | **verbatim copy of `session_context.rs` blob lines 2513-2537 at `df494bfa`** (§3.12 A1, 574 declaration bytes, 564 rendered bytes). **Round 5 re-based this row; see below.** | `is_known_generated_global_template` **and** `is_known_generated_standalone_global_template`, **each of which must also keep accepting `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES`** | `global` spec `current_version` **5 -> 6** |
| `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME` | `config/seeded_context_templates.rs` | verbatim copy of `session_context.rs` blob lines 2554-2574 at `df494bfa`, byte-identical to 2509-2529 at `d7008b34` (§3.12 A2) | `is_known_generated_coordinator_template` | `coordinator` spec `current_version` 5 -> 6 |
| `ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD` | `config/root_agent.rs` | verbatim copy of `root_agent.rs` blob lines 675-723, identical at both SHAs (§3.12 A3) | **two** lists: `is_known_generated_root_context_template`'s `old_generated` array (`:731-740`) **and** `migrate_root_role_file`'s pristine-generation list (`:1045-1051`) | `rootAgent` spec `current_version` 7 -> 8 |

**Round 5 re-bases the global row, and this is the single largest consequence of the drift.** #1605 landed a global-template generation between this plan's old base and its new one: it bumped `global` 4 to 5, added its own frozen snapshot `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES` holding the **v4** body, and edited the live body with a new `{{HOST_PLATFORM_RULES}}` placeholder. Four consequences, all binding:

1. **The frozen bytes change.** `GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME` must hold the **v5** body (with `{{HOST_PLATFORM_RULES}}`), not the v4 body. The branch currently holds v4, which is the generation `main` already froze under its own name; carrying that forward would ship two constants with identical bytes under two names and would leave v5 unrecognized forever, so every user who reaches v5 before upgrading would have their global template permanently reclassified as user-authored. That is precisely the silent failure §3.7 exists to prevent, arriving through the merge rather than through an edit.
2. **The version bump becomes 5 to 6.** Both `main` and the branch took `global` from 4 to 5 for different reasons; the two do not compose.
3. **Both recognizers must accept both frozen names.** `is_known_generated_global_template` (`:613-619` at `df494bfa`) and `is_known_generated_standalone_global_template` (`:632-639`) each already list `_BEFORE_TOKEN_MINIMIZATION`, `_BEFORE_AGENT_REPOS`, `_BEFORE_SUMMARIZATION` and `_BEFORE_HOST_PLATFORM_RULES`; `_BEFORE_ROOM_RENAME` is **appended** to each, newest-last, and **nothing is removed**. Dropping `_BEFORE_HOST_PLATFORM_RULES` while adding `_BEFORE_ROOM_RENAME` is the specific merge-resolution error to look for, because a three-way merge over two adjacent single-line additions to the same `||` chain can produce exactly that.
4. **Rule R applies to main's new template text.** The retired-token carrier is unchanged: the single Core Concepts line `- **Workgroup**: a runtime replica of a team for a specific task. It contains replica agents and \`repo-*\` working repos.`. `{{HOST_PLATFORM_RULES}}` carries no occurrence of the retired token, so the Rule R edit itself is byte-for-byte what it always was.

**The three specs #1605 adds take no snapshot and no bump.** `platform.windows`, `platform.linux` and `platform.macos` sit at `current_version: 1` with recognizers `is_known_generated_platform_windows` / `_linux` / `_macos`. Their content constants contribute **zero** lines to any of AC1's three base sweeps at `df494bfa` (§1.1), so Rule R never reaches them and there is nothing to freeze. This is stated because a reviewer checking §3.7 family 1 against the merged tree will find six specs where the family table names three, and the difference must be a decision rather than an omission. **§3.7 family 1's table is not re-derived in this round**: its three rows are the three specs this plan touches, and the three it does not touch are dispositioned here.

**The root snapshot's second wiring is new in round 2 and is not optional.** §3.7 family 2 shows `migrate_root_role_file` carries an independent list that includes `ROOT_ROLE_MD` itself at `:1051`. `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD` is in both lists (`:738` and `:1050`) and the repository ships a test that exists solely to catch a one-sided wiring: `frozen_v5_root_context_is_recognized_and_migrated_on_both_paths` (`:2436-2441`), whose doc comment says "a list edited in only one place cannot pass silently". Round 1 named only `old_generated`. Insert `ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD` immediately after `ROOT_CONTEXT_BEFORE_ORCHESTRATOR_RENAME_MD` in each list, preserving newest-last order.

"Verbatim copy" means: take the literal body from `git cat-file blob <base-sha>:<path>` at the SHA §3.12 labels that row with, change only the constant's name and add its doc comment. Do not retype it, do not reflow it, do not let an editor touch its trailing whitespace. The declaration-range digests in §3.12 Table A are what a reviewer checks the copy against, and the rendered-value digests in Table B are what the tests assert.

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
- `ProjectPanel.tsx:993`/`:995` and their **four** resolvers at `:1057`, `:1058`, `:1071` and `:2749` move together in one edit (Rule P clause R1; `:2749` added in round 5). `"Selected Workgroup"` becomes `"Selected Room"` and `"Workgroups"` becomes `"Rooms"` in all five places.
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

`src-tauri/module-arcs.txt`, regenerated (§11). `scripts/room-rename-allowlist.tsv`, new (§9.4 AC1), and **`scripts/room-rename-allowlist.mjs`, new**, the machine classifier AC1 point 7 requires and AC10 clause 4 now lists (round 5).

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
| `seeded_template_versions_were_bumped` | `config/seeded_context_templates.rs` | the three `current_version` values are **6, 6 and 8** (`global`, `coordinator`, `rootAgent`). **Round 5 changed the first from 5 to 6**: #1605 already took `global` 4 to 5, so this plan's bump lands on 6 (§1.1, §5.10 D8a). The branch asserts `(5, 6, 8)` today and that becomes wrong at the merge |
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
| `purge_room_and_purge_wg_produce_identical_outbox_messages` | the two spellings write outbox messages that differ only in `id`, `request_id` and `timestamp`. **Requirement (F)'s committed regression gate. It does not exist on the branch and it is not optional; see the note below** |

**None of the four integration tests above exists on the branch, and round 5 says so plainly rather than relaxing them.** `ac-dev-rust-grinch-v3` measured **zero** new test functions in `src-tauri/tests/` across the whole 12-commit range. AC3, AC4 and AC6 therefore currently rest on unrecorded manual checks, which is not what this plan asked for and not what a later regression will catch.

**The AC6 substitution is rejected, and the reason matters more than the verdict.** The implementer substituted an in-process parse test for live invocation, reporting that the real `purge-*` command blocks waiting on a daemon response. **The obstacle is a waiting problem, not an observability one, and the plan already specified the instrument that avoids it.** `cli/purge_wg.rs` writes the outbox file at `:192` and only **then** enters the response wait at `:212`, and that wait is bounded by `args.timeout`. So the outbox artifact for all four spellings is observable without a daemon: write, read the artifact, let the bounded wait expire. The parse test plus a single shared handler is a strong structural argument that the side effects are identical and the residual risk is low, which is why this is a missing gate rather than a shipped defect. It is still a missing gate, and requirement (F) is binding.

**The four "API and snapshot fixture twins" below and four of the five frontend tests are likewise unwritten**, and two of the twins are among the **three** `#[cfg(test)]` edits §6.1's table **requires** rather than merely permits. §6.4 says the `api/identity.rs` twins are what make requirement (D)'s "zero production edits" claim asserted rather than assumed; until they exist it remains assumed. The implementer's report presented "the only preserved-16 file touched is `commands/session.rs`" as a scope success; it is half a scope success and half a coverage omission, and §15 lists all twelve.

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

- Every `#[cfg(test)]` fixture that builds a `wg-*` directory and asserts it is **discovered, listed, addressed or deleted**. If any of these needs an edit, dual-prefix acceptance is broken, not the test. This is the strongest single signal in the change and it covers `cli_workgroup_team.rs` (191 matching lines), `cli_behavior_contract.rs:135`, `:147`, `:741`, `:761`, `:775-776`, `:861`, `cli_loop.rs`, `cli_close_session.rs`, `cli_role_experiment.rs`, `pty_input_cross_process.rs`, `terminal_snapshot_host.rs`, and the 67 frontend `*.test.ts(x)` files.

**The carve-out, new in round 5, and it is narrow on purpose.** The bullet above is stated over fixtures that assert an **existing** `wg-*` directory is accepted. It does **not** reach a fixture whose subject is what creation **produces**, and rounds 1 to 4 did not distinguish the two, which made D5 and requirement (A) contradict this section on a correct implementation.

| Signal | What it means | Verdict |
| --- | --- | --- |
| A `wg-*` fixture that seeds a directory and asserts it is discovered, listed, addressed or deleted needs an edit | dual-prefix **acceptance** is broken | **Blocker.** Ask why; do not accept the edit |
| A `wg-*` fixture moves to `room-*` because the allocator no longer scans `wg-` (D5) or because creation now produces a Room (requirement A), **with no expected value changing** | the fixture's subject moved, not its contract | **Authorized** by §9.3 clause 4 |

The discriminator is mechanical and a reviewer can apply it without judgement: **did any asserted value change, or only the prefix of the directory the fixture creates?** If an expected slot number, an expected stdout string, an expected count or an expected error moved, it is the first row and it is a blocker. If the only difference is `wg-` to `room-` in a path the fixture itself constructs, it is the second row.

**`tests/cli_workgroup_team.rs` is the file this carve-out is really about.** It took 99 changed lines in the implementation, and this section named it as one that must stay green **without being edited**. Under the discriminator above those 99 lines are the second row: 18 integration fixtures whose subject is `room add` / `room list` creating and reporting a Room, plus the allocator group. **The file name stays `cli_workgroup_team.rs` under Rule P0**, and the `wg-*` fixtures that test *acceptance* stay `wg-*` inside it. A reviewer should confirm both halves are present in the same file, because a file in which every `wg-` became `room-` has lost the acceptance evidence entirely and is the first row wearing the second row's clothes.
- `test-debt.allowlist.json` must not appear in the diff (§6.7).
- `scripts/smoke-cli-*.ps1` must not appear in the diff.

### 9.3 The rule for updating an existing test expectation

An existing assertion is edited only under one of the **four** clauses below, and then only to the new value and nothing else. Every other assertion is untouched; if one goes red, the implementation is wrong.

**Clause 1: it pins a string this plan renames under Rule R.**

**Every clause's site list is illustrative, and in round 5 that now includes clause 2's. The clauses are the gate.** Rounds 2 to 4 said clause 1's list was illustrative and clause 2's complete, and §14 item 12 told a reviewer clause 2 was "complete at eight rows". **It was not, and the failure was not a miscount.** Clause 2 has two limbs, a **size** limb and a **version** limb. The plan described a sweep, the sweep was run for the size limb by the architect and by both reviewers independently, and it is sound. **Nobody ran the equivalent sweep for the version limb**, even though this plan's own AC7.11 requires the version be asserted "at both the spec and the persisted-state layer", which is a direct statement that the version limb has more than one layer. Measured properly (below), the version limb is **17 assertion sites plus 2 test-name renames**, not the four `current_version` rows the table carried.

The structural lesson is the one that changes the plan rather than the list: **an enumeration cannot be the gate for a defect class whose failure mode is omission.** That is the same sentence AC5 uses about round 1's needle set and the same sentence AC1 uses about round 1's 36 needles. Clause 2 was the last place in this document where an enumeration was still load-bearing, and it failed the same way. So clause 2 is restated in the form clause 1 already had: the **clause** authorizes the edit, the enumeration below is evidence a reviewer can check, and an assertion that goes red and satisfies the clause is authorized whether or not it appears in the table. **A row missing from the table is no longer a defect in the change; a red assertion that satisfies no clause still is.**

This was a three-party miss and round 5 records it as such: the architect wrote "the complete set, measured, eight rows"; `ac-dev-rust-v3` and `ac-dev-rust-grinch-v3` both certified it in round 4; the tech lead flagged clause 2's completeness to `ac-dev-rust-grinch-v3` specifically and it still came back green.

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

**Clause 2 (new in round 2; its version limb re-derived and its completeness claim withdrawn in round 5): it pins a `current_version` this plan bumps, or a constant this plan's Rule R edits resize.** Round 1's rule forbade exactly these edits, and §5.10's three version bumps make all of them red on a correct implementation. **Read the second limb literally: a pin on the byte length, character count, word count or sha256 of a constant whose text Rule R changes is a clause-2 edit, exactly as a `current_version` pin is.** It is easy to miss because it pins a size, not a string, so clause 1 does not reach it.

**Limb A, the size limb. Three rows, closed by a sweep that was run and is sound.**

| Site @ `d7008b34` | Base | New |
| --- | --- | --- |
| `session_context.rs:6104` | `assert_eq!(workgroup_chars, 220)` | `223` (§5.10 D8b) |
| `session_context.rs:6105` | `assert_eq!(workgroup_words, 33)` | `34` (§5.10 D8b) |
| `config/injected_messages.rs:1671` | `assert_eq!(EXPECTED_SEED.len(), 1534, "the pinned seed is 1534 bytes")` | **`1531`**, and the message becomes "the pinned seed is 1531 bytes". §5.10 D8e's Rule R edit at `:78` shortens the line from 52 bytes to 49, and its twin at `:1331` shortens `EXPECTED_SEED` by the same 3 (clause 1). `1534 - 3 = 1531`. `:1672`'s `!contains('\r')` assertion is unaffected and is not edited |

The size sweep, restated so it is re-runnable: every constant this plan's Rule R resizes lives in `config/session_context.rs`, `config/root_agent.rs`, `config/seeded_context_templates.rs` or `config/injected_messages.rs`. Those four files were swept for every assertion pinning a `.len()`, a `chars().count()` or a word count of a **named constant**, which returns **nine** sites, dispositioned by name below. Three of the nine are the rows above; the other six are not resized. **Both round-3 reviewers ran this sweep independently and both returned `:1671` as the one and only addition**, and round 5 does not disturb it.

- `session_context.rs:6104` and `:6105` are the first two rows above.
- `session_context.rs:6106-6107` are **ceilings, not pins**, and are excluded; see the note below the tables.
- `session_context.rs:6161` (598) and `:6169` (570) pin `LEGACY_GIT_SCOPE_WITH_MATRIX_BEFORE_1072` and `LEGACY_GIT_SCOPE_WITHOUT_MATRIX_BEFORE_1072`, which stay frozen and are not resized (§3.12 A8 reproduces at `df494bfa`).
- `injected_messages.rs:1484`/`:1485` pin `DEFAULT_CONTEXT_ALERT_TEMPLATE` at 125. Its only retired-token carrier is the `%WORKGROUP%` placeholder, which §3.10 preserves under Rule P, so it is not resized.
- `injected_messages.rs:1671` is the ninth and is the third row above.

**Limb B, the version limb. The sweep nobody ran, run here at `df494bfa`.** The command, so it is re-derivable rather than trusted:

```
git grep -nE 'current_version|currentVersion|current_default_version|_to_v[0-9]' df494bfa \
  -- src-tauri/src src-tauri/tests
```

Then keep a hit only when the pinned value is produced by a **live spec**, directly or through the persisted state the live sync writes. A hit is **not** a clause-2 site when it constructs a synthetic `SeededContextTemplateEntry`, when it is a fixture JSON string carrying an arbitrary version, or when it belongs to an unrelated migration (`seed_manifest`'s `try_upgrade_v1_to_v2`, `settings`'s `migrate_*_to_v2`, `update_check::current_version`, which is the application version).

Measured that way at `df494bfa`, the version limb is **17 version-pinning sites plus 2 test-name renames**, across three specs. **"Sites", not "assertions"**: sixteen are assertions and one (`:3982`) is a fixture value that mirrors the live version by design, as note 2 below sets out; round 5 called them all assertions two paragraphs before saying one is not.

| Spec | Site @ `df494bfa` | Shape | Base | New |
| --- | --- | --- | --- | --- |
| `global` | `seeded_context_templates.rs:2164` | **test name** `project_specs_bump_global_to_v5_and_add_platform_specs` | v5 | `..._to_v6_...` |
| `global` | `:2167` | `assert_eq!(global.current_version, 5)` | 5 | 6 |
| `global` | `:2612` | `parsed["templates"]["global"]["currentVersion"], 5` | 5 | 6 |
| `global` | `:2750` | `state["templates"]["global"]["currentVersion"], 5` | 5 | 6 |
| `global` | `:2809` | `state["templates"]["global"]["currentVersion"], 5` | 5 | 6 |
| `global` | `:2889` | `updates[0].current_default_version, 5` | 5 | 6 |
| `global` | `:2969` | `updates[0].current_default_version, 5` | 5 | 6 |
| `global` | `:3030` | `parsed["templates"]["global"]["currentVersion"], 5` | 5 | 6 |
| `global` | `:3084` | `parsed["templates"]["global"]["currentVersion"], 5` | 5 | 6 |
| `global` | `:3142` | `parsed["templates"]["global"]["currentVersion"], 5` | 5 | 6 |
| `global` | `:3185` | `update.current_default_version, 5` | 5 | 6 |
| `global` | `:3210` | `parsed["templates"]["global"]["currentVersion"], 5` | 5 | 6 |
| `global` | `:3912` | `updates[0].current_default_version, 5` | 5 | 6 |
| `global` | `:3968` / `:3982` | **test name** `ignored_current_v5_pair_remains_suppressed` **and** its fixture `current_version: 5` | v5 / 5 | `..._v6_...` / 6 |
| `coordinator` | `:2180` | `assert_eq!(coordinator.current_version, 5)` | 5 | 6 |
| `coordinator` | `:3743` + `:3744` | `parsed[...]["coordinator"]["currentVersion"], 5` and its message | 5 | 6, message reworded from "by the #1571 orchestrator rename" to "by the #1614 room rename" |
| `rootAgent` | `root_agent.rs:2402` + `:2403` | `parsed[...]["rootAgent"]["currentVersion"], 7` and its message | 7 | 8, message reworded the same way |
| `rootAgent` | `root_agent.rs:2471` + `:2472` | `parsed[...]["rootAgent"]["currentVersion"], 7` and its message | 7 | 8, message reworded the same way |

**Four things in that table are worth a reviewer's attention specifically.**

1. **`root_agent.rs:2471` is a second `rootAgent` site.** Rounds 2 to 4 named only `:2402`. Both sites assert the same thing after different fixtures (`:2402` after the pristine-v4 path, `:2471` after the pristine-v5 path) and both go red.
2. **`:3982` is a fixture value, not an assertion, and it is still a clause-2 site.** `ignored_current_v5_pair_remains_suppressed` constructs a persisted entry at `current_version: 5` **because that is the live version**, and then asserts `updates.is_empty()` at `:4000`. Leave it at 5 against a live spec of 6 and the entry is stale rather than current, an update is produced, and the assertion fails. The fixture mirrors the live version by design, so it moves with it, and the test name moves with the fixture.
3. **`:2214`, `:3486` and `:3554` pin the three new `platform.*` specs at version 1 and are NOT clause-2 sites.** This plan does not bump them (§5.10 D8a).
4. **The `global` count changed shape at the re-base.** At `d7008b34` this limb was four `global` rows; at `df494bfa` #1605 added test coverage that pins the version at more layers. That is a re-derivation, not a discovery of something that was always there, and it is why every row above carries a `df494bfa` line number.

**The sweep returns assertion lines and never the message beside them, and three global messages name the version the content lands on.** The command matches `current_version`, `currentVersion`, `current_default_version` and `_to_v[0-9]`; a failure message on the *next* line matches none of those. Three at `df494bfa` state the landing version and go stale at 6: `seeded_context_templates.rs:2613` (`"recognized v1 global content must land on the current v4 default"`, main's, already stale at v4 while its assertion says 5, and this is step -1's resolution (v) and the only one of the three git presents as a conflict), `:3024` (`"pristine v4 Context.AgentsCommander.md must auto-upgrade to v5"`) and `:3031` (`"recognized v4 global content must land on the current v5 default"`). **The last two auto-merge silently**, so nothing will show them to the implementer. All three become v6. The discriminator, so this is a rule rather than a list: **a message that names the version content *lands on* moves with the bump; a message that describes what a past edit *did* does not.** By that rule `:2690` and `:2988` ("the v5 placeholder insertion must differ from its frozen v4 operand", "the v5 rewrite must actually change the template") stay as they are, and `:3736`, `:3751` and **`:3710`** (which say "the v4 default" of a coordinator whose live version is already 5) are **pre-existing** staleness from #1571, out of scope here, and named so a reviewer does not read them as a miss. **`:3710` is added in round 7**, after `ac-dev-rust-grinch-v3` swept the class exhaustively rather than trusting this list: every line in `seeded_context_templates.rs` and `root_agent.rs` carrying a `v<digit>` inside a string literal and **not** returned by the limb B sweep, 88 lines, of which the three named above are the only global ones and `:3710` is the one further coordinator message the boundary hides. Verified at `df494bfa`: it reads `"row 3 requires the seeded hash to differ from the current v4 default"` and is the message argument of an `assert_ne!` over two frozen hashes inside `read_sync_updates_seeded_v3_coordinator_and_bumps_version` (`:3663`), so it is **inert**: editing it or not editing it changes nothing that runs. It is named here for the same reason the other two are, and because §9.3's own rule is that a table miss is a finding against **this plan** rather than against the implementation.

**Why this list is evidence and not the gate.** A reviewer should re-run the command above at the merged tree and reconcile it against this table. **A site the table missed is authorized by the clause and is not a finding against the implementation**; a discrepancy is a finding against **this plan**, to be reported so the table is corrected. That inversion is deliberate and it is the whole point of the round-5 change to this clause.

Every other `.len()` hit in those four files pins a collection length or a runtime value rather than a constant. Digest pins over template snapshots are a different mechanism and are governed by §5.10 D8a's version bumps and §3.10's preserved machine values, not by this clause.

The two `assert!` ceilings at `session_context.rs:6106-6107` are **not** in this clause and are not edited. They are a compactness budget, not a pin on this rename.

**`test-debt.allowlist.json` must not appear in the diff (§6.7 Part 1), so every renamed test name is checked against it.** Rounds 2 to 4 checked four names at `d7008b34` and all returned zero occurrences. **Round 5 adds two names to that check**, because limb B now renames two tests rather than one, and the names themselves changed at the re-base: `project_specs_bump_global_to_v5_and_add_platform_specs` (main's name; the branch renamed the old `project_specs_bump_only_the_global_template_to_v4` to `..._to_v5`, and the merged tree must carry **main's** name with `v6`) and `ignored_current_v5_pair_remains_suppressed`. **Re-run the check at the merged tree for all six names before renaming any of them**; a hit means `test-debt.allowlist.json` would have to move, which §6.7 Part 1 forbids, and that is a STOP rather than an edit. `ac-dev-rust-grinch-v3` found the analogous defect in the delivered code: `entity_creation.rs:7959` was pointed at `cli::room::tests::cli_room_lock_order_inversion_child` while `test-debt.allowlist.json:197` still pins the old FQN and the child test was never renamed (§15).

**Clause 3 (new in round 2): it asserts a prefix predicate this plan widens.** One site: `entity_creation.rs:6943-6946` asserts `!temp_name.starts_with("wg-")` with the message "temp name must NOT match the wg- discovery filter (would surface as ghost workgroup)". After this change the `.deleting-*` sentinel must fail the **widened** filter, so the assertion becomes `!crate::config::entity_prefix::has_entity_prefix(temp_name)` and its message is reworded. Leaving it as written would let a `room-`-shaped sentinel pass, which is the exact defect the assertion exists to prevent. Round 1 listed this site in the known-red set but under clause 1, which does not apply: it pins no string.

**Clause 4 (new in round 5): the fixture's own subject moved because creation or allocation now produces a Room, and no expected value changed.** D5 makes the allocator Room-only and requirement (A) changes the name creation produces. Both are decided design, so a fixture whose subject is "what gets created" must move its own `wg-` to `room-` or it is no longer testing the code under test. Rounds 1 to 4 authorized none of it: clause 1 pins no renamed string, clause 2 pins no version or size, clause 3's single named site is `entity_creation.rs:6943-6946`, and Rule P2 said fixtures "stay `wg-*`" and get twins. That was a genuine contradiction inside the plan, not an implementation error, and the implementation surfaced it.

The clause is narrow and its boundary is the discriminator §9.2 states: **only the prefix of a directory the fixture itself constructs may move, and no asserted value may change with it.** The two groups it authorizes, both measured in the delivered implementation:

| Group | Count | Why it moves |
| --- | --- | --- |
| `determine_next_wg_number_*` allocator fixtures in `commands/entity_creation.rs` | **10** | D5 makes the allocator Room-only, so a `wg-` fixture asserts allocator semantics over a prefix it no longer scans. Expected slot values are unchanged |
| Integration fixtures in `src-tauri/tests/cli_workgroup_team.rs` whose subject is what `room add` creates or `room list` reports | **18** | requirement (A). Expected stdout, counts and errors are unchanged |

**What clause 4 does not authorize, stated because this is the clause most likely to be over-applied.** It does not reach a fixture that seeds a **pre-existing** `wg-*` directory to prove it is still discovered, listed, addressed or deleted. Those are §9.2's negative evidence, they are the only way dual-prefix acceptance is testable at all, and converting one is the blocker §9.2 describes rather than a clause-4 edit. It does not reach `Rule P2`'s twin obligation either: where §9.1 directs a `room-` twin **beside** an existing `wg-` literal, the twin is still added and the original still stays.

**What is still forbidden.** An assertion that pins a directory name, an identifier, a serde or JSON key, a `data-ac-testid`, an event name or a wire value is never edited. In particular `PURGE_WG_ACTION`'s value, the `"workgroupCreated"` / `"workgroupRemoved"` reason codes, the collapse keys and `%WORKGROUP%` are all pinned by tests that must stay green untouched, and §9.2's negative evidence covers them.

### 9.4 Objective acceptance criteria

**AC1, visible text: total sweep plus a committed allowlist, zero unlisted lines.**

Round 1 offered 36 fixed-string needles. A needle set can only re-find what the enumeration that produced it already found, so it cannot detect an enumeration miss, and `git grep -F` is case-sensitive, which is how `Delete workgroup` passed under a `Delete Workgroup` needle. **The needles are deleted.** They are replaced by a criterion whose failure state is "a line nobody classified" rather than "a needle nobody wrote".

**The allowlist is derived at the base commit and committed before the first visible-text edit.** Round 2 generated it at step 10b **from the post-change sweep output**, and the gate was "every returned line has its pair in the allowlist". `sweep > allowlist` satisfies that mechanically and unconditionally, so for *this* change the only thing between the gate and a vacuous pass was clause 3, a human justification. The prospective value is real, a future regression does turn the gate red, and that value is kept. What is fixed is the direction of derivation: an unrenamed Rule R line must surface as a line **nobody listed**, not as a row the implementer wrote after seeing it.

1. **Part A, frozen at the base.** At the frozen base, **`df494bfa` from round 5 onward** (it was `d7008b34` in rounds 1 to 4, and §1.1 gives the exact 14-line delta between them), before any product edit, run the three sweeps below and subtract the Rule R lines this plan is going to move. The remainder is `scripts/room-rename-allowlist.tsv`, committed at step 0b, **before step 9**. Each row is `<class>\t<path>\t<trimmed line content>`, where `<class>` is one of the Rule P clause names: `P0-identifier`, `P0-css`, `P0-testid`, `P0-key`, `P0-event`, `P0-wire`, `P0-token`, `P1-comment`, `P2-fixture`, `P3-frozen`, `P4-log`. Content, not line number, is the key, so the file survives line drift.

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

   **One of the 563 docs lines has no `<path>:<line>:<content>` shape, and the key it takes is decided here rather than by the implementer.** `git grep` emits `Binary file docs/screenshots/hero.png matches` (prefixed with the rev when a rev is given). It is pre-existing, present at **both** SHAs, and it is one of the 563: drop it and the docs sweep is 562. It is keyed as `(docs/screenshots/hero.png, "<binary file>")` and carried on Part A with class `P2-fixture`, which is what the committed `scripts/room-rename-allowlist.mjs` already does; naming it here is what makes that an authorized normalization rather than an implementer decision. Note that it is **not** `docs/assets/og-card.png`: that one is excluded by the `':!docs/assets'` pathspec, this one is not. `ac-dev-rust-v3` found the gap.

4. **Part B, the additions, and why they exist.** Some lines carrying the retired token are *introduced* by this change and therefore cannot be in a base-derived list: the dual-prefix helpers (`config/entity_prefix.rs` and `shared/entity-prefix.ts` must both name `"wg-"`), the frozen `*_BEFORE_ROOM_RENAME` copies of §5.10, the `#[cfg(test)]` fixture twins of §9.1, and the compatibility sentences §5.13 adds to the documentation. These are appended at step 10b as **Part B**, each with a class and a one-line justification naming the plan section that requires it. Part B is expected to be short and every row of it is a deliberate decision; Part A is expected to be long and every row of it is a pre-existing fact. Keeping them apart is what makes the second reviewable at all.

5. **Each row must carry a class**, and a row whose class cannot be justified from §5.2 is a Rule R miss, not an allowlist entry.

6. **What this gate proves, stated exactly, because round 2 claimed more than it delivers.** "Zero by allowlist" proves that **every surviving occurrence is listed and visible in review**, and, now that Part A is frozen at the base, that **no line this plan was supposed to move is still there** (an unrenamed Rule R line is not in Part A, so it comes back unlisted). It does **not** prove that each Part A row is *correctly classified*: that is clause 5, a discipline statement backed by review, and AC1b's ten pinned strings cover part of the gap. Nobody should over-rely on it beyond that.

7. **How much may be machine-classified, and what a human reads.** Part A is large and the split is stated so the implementer does not either hand-audit 4,000 rows or rubber-stamp them. Machine-classifiable, by a script committed beside the allowlist: `P0-css` (the line's match is inside a `class`/`className` value or a `.ac-wg-`/`workgroup-group-` selector), `P0-testid` (inside a `data-ac-testid` value), `P0-key` (inside a `serde` attribute or a quoted JSON key), `P1-comment` (the trimmed line starts with `//`, `///` or `//!` and the file is Rust), `P4-log` (the line is inside a `log::` macro invocation), and `P0-identifier` where the only match is a Rust or TypeScript identifier with no adjacent quote. **Read by hand, every row:** `P0-wire`, `P0-event`, `P0-token`, `P2-fixture` and `P3-frozen`. Those five are the classes whose misclassification is a compatibility break rather than a cosmetic miss, they are the classes §3.10 and §5.2 P3 enumerate, and together they are a small fraction of the file. The machine classifier's own output is spot-checked against §3.10's table.

8. **Baseline arithmetic, so a reviewer has numbers rather than a procedure.** Measured at **`df494bfa`**, with the `d7008b34` figures kept beside them so the re-base is auditable:

**What gets subtracted is every base line this plan moves, which is more than the Rule R set.** A line that is not Rule R can still have its content changed by this plan, and content is the allowlist key, so leaving such a line in Part A would make it come back unlisted and turn the gate red on a correct implementation. For the frontend the subtraction is fully enumerated, and all 65 lines were verified present in the base sweep with no duplicates (the frontend sweep is 906 at both `d7008b34` and `df494bfa`, so the re-base does not touch this enumeration at all; the two rows that changed are P6's fourth R1 resolver in round 5 and round 6's Rule P1 clause (b) comment, neither of them the drift's):

| Group | Lines | Why it moves |
| --- | --- | --- |
| §3.8's four visible-text classes | **55** | Rule R. 24 + 6 in (a), 5 in (b), 18 in (c), 2 in (d) |
| §3.8's R1 resolvers, `ProjectPanel.tsx:1057`, `:1058`, `:1071`, **`:2749`** | **4** | not visible text, but §5.2 clause R1 moves them in the same commit as `:993`/`:995` or the sidebar filter silently stops matching the row it names. **Round 5 corrects this row from 3 to 4**; §3.8 closes the set by literal sweep rather than by enumeration |
| §5.4's prefix predicates not already counted: `profile-utils.ts:124`, `:472`, `WorkgroupGroupRail.tsx:67`, `:72`, `WorkgroupTask.tsx:74` | **5** | D2 replaces each with a call to the shared helper, so the `` /^wg-/ `` literal leaves the line. F1 (`path-extractors.ts:25`) and F5's label (`:73`) are already inside the 55 |
| **`WorkgroupTask.tsx:70`**, the Rule P1 clause (b) comment correction (**new in round 6**) | **1** | not Rule R and not a §5.4 predicate: §15.2 row 6 **mandates** that this comment be corrected because it contradicts its code after the change, and §15.2 now fixes the exact replacement bytes. The correction drops the line's `wg-`, so the line leaves the sweep. It is one of the 906, it is on the committed Part A at row 709 (file line 726) as `P1-comment`, and rounds 1 to 5 left it there. `ac-dev-rust-grinch-v3` found it. `:74` above is the regex line and is a different line |
| **frontend total moved** | **65** | |

**Round 6 corrects the fourth column's unit, and this defect already bit once.** The column was headed "Part A rows" and the closing check read `rows(Part A) + lines moved = 4573`. **Part A is a file of `(path, trimmed content)` rows; the three sweeps return lines**, and across this document the two differ by about 700. `842` was never a row count: it is `906 - 64`, a **line** count. The proof that this is not cosmetic is that the implementer had to invent the missing disambiguation to ship at all, writing into the committed allowlist's own header *"The closing arithmetic above counts LINES; the file below holds rows."* That is an implementer decision this plan did not authorize, which is the class of thing this round exists to remove. `ac-dev-rust-v3` found it. Point 9's round-5 note identified the same conflation and corrected only the Part B half; §3.8's "`906 - 64 = 842` **rows**" is the same slip and is corrected there.

**The table below is the state of Part A *after* §12 step 0b**, and every figure in it is measured at `df494bfa` against the committed `scripts/room-rename-allowlist.tsv` with step 0b's three appends and two removals applied:

| Surface | Base sweep @ `df494bfa` | (was @ `d7008b34`) | Lines this plan moves | **Part A lines** | (Part A keys) |
| --- | --- | --- | --- | --- | --- |
| frontend | **906** lines / 41 files (§3.8) | 906 / 41, unchanged | **65**, enumerated above | **841** | 775 |
| Rust | **3104** lines / 94 files (§3.14) | 3090 / 94 | **285**, enumerated at step 0b | **2819** | 2172 |
| docs and root markdown | **563** lines / 64 files | 563 / 64, unchanged | **533**, enumerated at step 0b, and at least the 9 named in point 2 | **30** | 30 |
| **total** | **4573** | 4559 | **883** | **3690** | 2977 |

**The closing check a reviewer runs without redoing the classification, corrected:** `` lines(Part A) + lines moved = 4573 ``, that is `3690 + 883`, with the three per-surface subtotals stated in the PR body. Every surface row closes on its own too: `841 + 65 = 906`, `2819 + 285 = 3104`, `30 + 533 = 563`.

**The Part A *row* count is carried separately, because the file is rows and the gate counts lines.** A reviewer needs both numbers and they are not interchangeable:

| | rows in the file | unique `(path, content)` keys |
| --- | --- | --- |
| committed at `bb2a5a65` | **2,988** | **2,976** |
| after step 0b (+3 appended, -2 removed) | **2,989** | **2,977** |

**The file holds 12 more rows than it holds keys, and that is correct, not a defect.** Twelve `(path, content)` pairs appear twice with **two different classes**, because the same trimmed content occurs on several lines of one file and the classifier reached different verdicts (for example `session_context.rs` `Workgroup(String),` is both `P0-identifier` and `P3-frozen`). The gate keys on `(path, content)` and ignores the class, so the duplicates are inert; they are recorded here so that a reviewer who counts unique keys and gets 2,976 does not report a 12-row discrepancy. The frontend row of the table above is fully determined and is the worked example.

**The Rust base moves by exactly 14 lines and loses nothing, which is why Part A is refreshed rather than rebuilt.** §1.1 decomposes the 14 lines over **six** keys: three genuinely new, contributing 4 lines and appended at step 0b, and three pre-existing Part A rows whose occurrence count merely rises, contributing 10 and needing no edit. **Thirteen of the fourteen lines are inside `#[cfg(test)]`; the fourteenth, `seeded_context_templates.rs:220`, is production** (round 6 corrects this; round 5's blanket claim was false and §3.14 carried its inner numbers on it). Keyed on `(path, trimmed content)`, **zero** rows present at `d7008b34` are absent at `df494bfa`. So the committed `scripts/room-rename-allowlist.tsv` Part A remains valid as a strict subset and the step-0b work at the new base is **three appends, two deletions and a subtotal correction** (§12 step 0b names all five operations), not a re-derivation of 2,988 rows. A reviewer should confirm that shape: a Part A that changed by more than those five operations means the sweep was re-run differently, not that the base moved.

9. **Backstops on the post-change counts, tightened.** Round 2 said only "the post-change frontend count must be lower" and "a count equal to 906 is a finding", which together admit any value in 1..905. The runnable form: **the post-change frontend sweep returns exactly `841 + <Part B frontend lines>` lines**, a number the implementer writes into the PR body before running the gate and then compares against. It is exact because Part A's 841 frontend **lines** are lines whose content this change does not touch, which is what makes the enumeration above a subtraction rather than an estimate: any base line the change *does* touch is in the 65 and is therefore not in Part A. Additionally, each of the 65 must be absent from the post-change output, checked individually. A post-change count equal to 906, or equal to 841 when Part B has frontend rows, is a finding to report.

   **Round 5 corrected this arithmetic once and round 6 corrects it again, for a different reason.** Rounds 1 to 4 predicted `843 + 21 = 864`; the sweep at branch head `bb2a5a65` measured **863**. The implementer diagnosed that as Part A rows versus Part A lines (777 unique rows for 843 base lines); **that diagnosis is refuted**, because the gate counts **lines that match rows**, not rows. Round 5 attributed the whole gap to P6's `ProjectPanel.tsx:2749` and landed on 863. That was right about the tree as it stands and wrong about the finished state, because `bb2a5a65` has not yet made the §15.2 row 6 comment correction. Both decompositions, so a reviewer can check the one that matches the tree in front of them:

   ```
   at branch head bb2a5a65, before D2 (measured)     after D2, the finished state (predicted)
   base frontend lines                    906        base frontend lines                    906
   base lines still present                842        base lines still present               841   <- Part A
   base lines MOVED                         64        base lines MOVED                        65
   Part B frontend contribution             21        Part B frontend contribution            21
   906 - 64 + 21                         = 863       906 - 65 + 21                         = 862
   ```

   **So the gate's expected value is 862, and it is determinate only because §15.2 row 6 now fixes the replacement bytes.** `ac-dev-rust-grinch-v3` raised exactly this: the round-5 plan mandated the correction, did not say what the corrected comment says, and then demanded an exact count that depends on it. §15.2 row 6 supplies the three replacement lines; two of them are byte-identical to the base and the one that changes drops its `wg-`, so exactly one line leaves the sweep and none enters it. If the implementer's replacement differs from the one §15.2 fixes, the plan is what must change, not the number.

   **One latent risk survives and is recorded rather than fixed, because fixing it is not worth a mechanism.** The formula adds a Part B **row** count to a Part A **line** count. It is correct here only because Part B's 21 frontend rows happen to contribute exactly 21 lines. It would drift the moment one Part B content string appeared on two lines of one file. The wording above now says "Part B frontend **lines**"; if the implementer's Part B row count and line count ever differ, the **line** count is the one this backstop uses, and the divergence is worth a sentence in the PR body.

10. **The reverse limb, new in round 5, because AC1 as written is blind in one direction.** Points 3 and 9 test that every line the post-change sweep **returns** is listed. They cannot see a line that the change **removed from the sweep by renaming it**, because a renamed line stops matching the sweep and disappears rather than appearing unlisted. That is not a hypothetical: `ac-dev-rust-grinch-v3` ran the audit this gate structurally cannot run, asking for each of the 2,988 Part A rows whether that `(path, trimmed content)` still exists at HEAD, and found **six Part A rows carrying genuine unauthorized substitutions** that the forward gate reported as zero unlisted. Those six rows are five of §15.2's six items, and all six items are in §15.

    **The absent total on that run is 77, and the number depends on this plan's own normalization, so it is fixed here rather than left to the audit.** Measured over all 2,988 committed Part A rows against the three sweeps at `bb2a5a65`: **77 absent rows across 16 paths** once point 3's `(docs/screenshots/hero.png, "<binary file>")` key is applied, which is what the committed `scripts/room-rename-allowlist.mjs` already does. An audit that leaves that one row unnormalized gets **78**. The self-test writes this count into the PR body, so the plan carries the figure its own instrument produces and names the fork rather than letting a reviewer and an implementer disagree by one. `ac-dev-rust-grinch-v3` re-measured it in round 6; round 7 reproduced it independently.

    **The criterion, and the gate is the predicate, not the list. Round 6 inverts this, because round 5 wrote it the wrong way round in the same document that spends a whole section explaining why.** For each Part A row, the pair `(path, trimmed content)` must still be present in the post-change tree. **An absence is authorized when, and only when, a named section of this plan requires that line to move.** The fourth column of the output means "no plan section authorizes this", and a row in it is a **Rule P violation to fix**, ranked with the same severity as an unlisted line.

    Round 5 wrote four enumerated absence classes and made the enumeration the gate. `ac-dev-rust-grinch-v3` found the counterexample **inside this document**: a Rule P1 clause (b) comment correction is in none of (a) to (d), and §15.2 row 6 mandates one. Its own words are the reason this is inverted rather than extended: *"This is the same structural error round 5 spent a whole section fixing in clause 2. The plan's own sentence, an enumeration cannot be the gate for a defect class whose failure mode is omission, applies verbatim to point 10's four classes and was not applied to them."* That is correct and it is the same sentence AC1's preamble, AC5 and §9.3 clause 2 already carry. So the classes below are **illustrative**, they are what the large majority will fall into, and a class the list omits is a finding against **this plan**, not against the implementation:

    | Class | Why the row legitimately disappears |
    | --- | --- |
    | (a) A clause-1 test assertion whose pinned string this plan renames | §9.3 clause 1 |
    | (b) A clause-4 allocator or creation fixture whose prefix moved | §9.3 clause 4 |
    | (c) A `rustfmt` reflow that re-wrapped the line without changing its tokens | `cargo fmt` is a required CI job (§3.11) |
    | (d) A line inside a range this plan deletes or replaces wholesale | must name the plan section that does so |
    | (e) **A Rule P1 clause (b) comment correction** (new in round 6) | §5.2 P1, whose clause (b) is a **predicate, not a list**: §3.14's last paragraph and `WorkgroupTask.tsx:70` are the corrections visible at the base, §15.2 row 6 is the frontend one, and the self-test row below names two more that the branch already carries. Note that the two Part A rows this class would otherwise produce are **removed from Part A at step 0b instead** (§12), which is stronger: an uncorrected comment then comes back **unlisted** and the forward gate catches it |

    **The output of this limb is a table in the PR body: the absent count, the per-class counts, and every row that no plan section authorizes, listed individually.** A non-empty fourth column is the finding. Run it with the committed Part A file against the post-change tree; it needs the same tooling the forward gate already uses and no new instrument.

    **The self-test is a different run from the D5 run, and round 5 conflated them into one criterion that cannot pass.** `ac-dev-rust-grinch-v3` traced it: §15.1 orders D1 and D2 (the six corrections) **before** D5, so by D5 five of the six have been restored and match Part A again and the sixth is the one deliberately updated. Run at D5, the limb surfaces **zero** of the Part A rows it was told to surface. As written, an implementer could satisfy the criterion only by **not** making the fixes this same document mandates. The two runs, separated:

    | Run | Tree | Expected result |
    | --- | --- | --- |
    | **Self-test**, once, before D1 | `bb2a5a65` as it stands; no work needed, it is the current branch head | **Five of §15.2's six items surface, as exactly six Part A rows**, each in no authorized class, inside a total of **77** absent rows. That is the confirmation that the limb works, and it is also the independent reproduction of `ac-dev-rust-grinch-v3`'s audit. **The unit of the stop-criterion is Part A rows, not §15.2 items, and the two are not equal**: §15.2 row 3 renamed the same warning code on two lines, so it alone contributes **two** rows, **977** and **979**. The six, by Part A row number: **1689** (item 1), **1605** (item 2), **977** and **979** (item 3), **843** (item 4), **1500** (item 5). **All six must surface, and that is a lower bound, not an equality: if any of the six is missing, the limb is mis-wired and nothing downstream from it means anything.** Round 7 also required the absent set to hold nothing else, and that requirement was wrong; `ac-dev-rust-grinch-v3` refuted it by running the limb. **Three Part A rows surface as `P1-comment` absences at `bb2a5a65`, and none of the three is mis-wiring.** **872** (`cli/list_peers.rs:910`) and **1411** (`commands/entity_creation.rs:2953`) are Rule P1 clause (b) corrections of comments sitting on §3.2 gate lines this plan moves, authorized by class (e) above and named in §5.2 P1, so they belong in the authorized bucket and not in the fourth column. **1492** (`commands/entity_creation.rs:3833`) is the same class and is adjacent to `:3832`, which §3.14's last paragraph does name. **A fourth-column row beyond the six, or a `P1-comment` absence beyond those three, is a finding to triage against the classification, not evidence the limb is broken.** Run it against the **committed** Part A, before step 0b touches it |
    | **The gate**, at D5 | the merged, corrected tree | **The fourth column is empty.** Every absence is authorized by a named section. `WorkgroupTask.tsx:70` and `ProjectPanel.tsx:2749` do not appear at all, because step 0b removed both rows from Part A |

    **§15.2 row 6 is excluded from the self-test's expected set, and the reason is the interesting part.** It is **not a substitution**: the comment at `WorkgroupTask.tsx:71` (`bb2a5a65`) is **unchanged** on the branch, and what moved is the code it describes. So its `(path, trimmed content)` key is still present at `bb2a5a65`, the reverse limb has nothing to report, and **no correct implementation of this limb can make it surface**. Measured: the frontend sweep at `bb2a5a65` returns `WorkgroupTask.tsx:71` with content byte-identical to Part A row 709, and that file contributes **zero** absent rows. **The forward gate is what catches this one**, once step 0b deletes row 709: an uncorrected comment still matches the base sweep, is no longer on Part A, and comes back **unlisted**. That is the same asymmetry class (e) above already relies on, stated once more here because this is where the count is asserted. **Round 6 wrote "six" against a limb that surfaces five items**, and the coincidence that they surface as six Part A rows is what made a wrong result look like a right one; `ac-dev-rust-grinch-v3` found it by running the limb rather than reasoning about it.

**AC1b, the round-1 misses, named. Restated in round 5, because as written it could not pass.**

The ten needles are unchanged: `Delete workgroup`, `the workgroup is locked`, `associated workgroups`, `its workgroup replicas`, `same-workgroup Orchestrators`, `workgroup replicas before minting`, `the current workgroup;`, `every workgroup of this team`, `the workgroup TASK.md`, `workgroup(s)`. They are a regression pin on the specific defects round 1 shipped, not a substitute for AC1. **What changes is the path set and the expected result**, because rounds 1 to 4 said "the post-change tree must contain none of the following" with **no pathspec at all**, which is unsatisfiable on three independent grounds:

1. **It is self-defeating.** This plan file lists all ten needles verbatim, and so does the committed `scripts/room-rename-allowlist.tsv`. The criterion forbids its own text.
2. **Two needles collide with text Rule P deliberately preserves.** `the workgroup TASK.md` hits five Rule P1 doc comments; `Delete workgroup` hits a Rule P4 `log::` line.
3. **One hit is in a §6.7 Part 1 path.** `scripts/smoke-current-app-mockup.mjs:353` carries `Delete Workgroup` inside an `assert.doesNotMatch(...)` regex, and Part 1 forbids that file from appearing in the diff at all. So under the round-1 wording that needle could not be satisfied without violating a different binding constraint.

**The restated criterion.** Run the ten needles case-insensitively over **AC1's own three path sets and no others** (`src` with the `.tsx?` filter and the `.test.tsx?` exclusion; `src-tauri/src`; `docs README.md ROADMAP.md PRIVACY.md src-tauri/src/api/README.md ':!docs/assets'`). `plans/`, `CHANGELOG.md`, `scripts/` and `src-tauri/tests/` are outside all three, which disposes of grounds 1 and 3 by construction. **Eight needles must return exactly zero. Two return exactly the six hits enumerated below and no others.**

| Needle | Permitted hits over the AC1 path sets | Rule | On the allowlist? |
| --- | --- | --- | --- |
| `the workgroup TASK.md` | `src-tauri/src/cli/task_set_title.rs:2` (`//!`) | P1 | yes, `P1-comment` |
| | `src-tauri/src/commands/task.rs:218` (`///`) | P1 | yes, `P1-comment` |
| | `src-tauri/src/commands/task.rs:260` (`///`) | P1 | yes, `P1-comment` |
| | `src-tauri/src/commands/task.rs:294` (`///`) | P1 | yes, `P1-comment` |
| | `src-tauri/src/commands/task.rs:334` (`///`) | P1 | yes, `P1-comment` |
| `Delete workgroup` | `src-tauri/src/commands/entity_creation.rs:3102`, the `log::` body `"[entity_creation] Failed to delete workgroup {}: {}"` | P4 | yes, `P4-log` |
| the other eight needles | none | | |

Measured at branch head `bb2a5a65`: eight needles return zero and the two return exactly those six lines. **Every one of the six is already on the committed Part A allowlist with the class shown**, which is the check that makes this table a disposition rather than an exemption: a permitted hit is permitted because Rule P classified it at the base, not because AC1b waived it. **A hit that is not in this table, or a permitted hit that is not on the allowlist, is a finding.**

**Two things this restatement deliberately does not do.** It does not delete the two colliding needles, because the visible-text defects they were written for are real and a future regression could reintroduce one on a **new** line, which this form still catches. And it does not extend the path set to `scripts/` or `src-tauri/tests/` to "be thorough": those surfaces are governed by §6.7 and §9.2 respectively, and pulling them into AC1b is what produced the contradiction.

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
    // The hidden PARENT must be walked: that is what guards H6.
    assert!(
        walked.contains_key(&format!("{root_name} role-experiment")),
        "`<root> role-experiment` is not in the walked set"
    );
    // ...but the placeholder renders on the CHILD. `retain_workgroup` is a field
    // of `RunArgs`, attached to `RoleExperimentCommand::Run`; `role-experiment`
    // is a parent whose `render_long_help()` renders `<COMMAND>` and its
    // subcommand list, never a child's arguments. Round 5 corrected this; see
    // the prose below.
    let role_exp_run = walked
        .get(&format!("{root_name} role-experiment run"))
        .expect("`<root> role-experiment run` is not in the walked set");
    assert!(
        role_exp_run.contains("--retain-room <RETAIN_ROOM>"),
        "role-experiment run help did not render the corrected value placeholder"
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
- **the placeholder assertion targets `role-experiment run`, not `role-experiment`, and round 5 corrects this.** `retain_workgroup` is a field of `RunArgs` (`cli/role_experiment.rs:96`), and `RunArgs` is attached to the `Run(RunArgs)` variant of `RoleExperimentCommand` (`:31`, enum at `:25`). `role-experiment` is a **parent**: `render_long_help()` on a parent renders its `about`, its `after_help` and its `<COMMAND>` subcommand list, and never a child's argument list. So `role_exp.contains("--retain-room <RETAIN_ROOM>")` **panics on a correct implementation**, which is what happened in delivery. The membership assertion stays on the hidden parent, because that is what actually guards H6 (a `hide = true` command must still be walked); only the **rendering** assertion moves down to the child. Intent is unchanged: the string whose omission caused G5 is still asserted to have been rendered, on the node that renders it. `ac-dev-rust-v3` found it and made exactly this fix; `ac-dev-rust-grinch-v3` judged it sound and intent-preserving.
- **why the round-3 probe could not have caught it, which is the part worth carrying forward.** The offline probe crate that settled AC5's other clap facts was a **synthetic flat tree with no parent/child argument split**, so it could not exhibit a parent that renders `<COMMAND>` instead of its child's arguments. `ac-dev-rust-grinch-v3`'s round-3 and round-4 verdicts verified the traversal and the key derivation in detail and explicitly recorded that it had **not** re-derived the value at the `role-experiment` key, only that the lookup would succeed. The gap was flagged at the time and is where the defect lived. The lesson for a future probe: a probe of a command tree must model at least one parent with arguments on a child, or it proves nothing about where help text renders.
- **the three membership assertions are the guard, and they are new in round 3.** `team create` is a **depth-2 leaf** and is the only command that prints `team.rs:61`; `role-experiment` is hidden and is the only command that prints the corrected placeholder **on its `run` child**; `role-experiment variant set` is a depth-3 leaf. Asserting that each was walked, and that the first two actually **rendered** the strings whose omission caused G5, makes a depth truncation red rather than green. A `!contains` assertion over a set that was never populated is vacuously true, which is why membership has to be asserted separately from absence.
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
| 7.1 | `GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len()` and its sha256 | **564, `D094106B...4F77`** (§3.12 B1, taken at `df494bfa`). **Round 5 re-derived this row.** `539` / `F4406596...316A` is now the **v4** body, which is `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES`, main's constant, and asserting it here would mean this plan froze a generation main already froze. A test still asserting 539 after the merge is the specific failure to look for |
| 7.1b | `GLOBAL_CONTEXT_TEMPLATE_BEFORE_HOST_PLATFORM_RULES` is still accepted by both global recognizers, and is `!=` `GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME` | green. **New in round 5; its second limb described properly in round 6.** This is the merge-resolution guard and it has two limbs that catch two different failures. The `\|\|`-chain limb catches a three-way merge over two adjacent single-line additions to one chain silently dropping one. **The `!=` limb is the one that catches the genuinely silent failure**: both recognizer lines kept but the frozen body **not** re-based, which leaves two constants holding byte-identical bytes under two names with v5 permanently unrecognized, and which no conflict, no compile error and no other criterion in this plan would surface. `ac-dev-rust-grinch-v3` measured that the two bodies really are byte-identical before the re-base (both 539 bytes, `F4406596...`), so the `!=` limb is red in exactly that case. AC7.1 (564 / `D094106B...`) catches it independently, and AC7.9 catches the mirror case where `_BEFORE_ROOM_RENAME` is the entry dropped |
| 7.2 | `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME.len()` and its sha256 | 2516, `0B89EB38...198E` (see §3.12's note if the length differs) |
| 7.3 | `ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD.len()` and its sha256 | 2467, `7F82F28C...C52D` |
| 7.4 | `WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME.len()` and its sha256 | 220, `A386B52D...566D` |
| 7.5 | `ROOT_COORDINATION_MESSAGING_PARAGRAPH_BEFORE_ROOM_RENAME.len()` and its sha256 | 897, `FC2164A2...F207` |
| 7.6 | `OLD_DEFERRED_MESSAGING_PARAGRAPH.len()` and its sha256, unchanged | 293, `6E12E68E...A463` |
| 7.7 | `legacy_rendered_default_context_for_compat(<fixed inputs>)` length and sha256 | the step-0 capture (§3.12 Table C) |
| 7.8 | source-side freeze: `git cat-file blob HEAD:src-tauri/src/config/session_context.rs`, extract from the line matching `^fn legacy_rendered_default_context_for_generation($` through the next line matching `^}$`, replace the single token `WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME` with `WORKGROUP_GIT_SCOPE`, sha256 | `940FA357...16C2` (§3.12 Table A). The one-token substitution is the D8b identifier change and is the **only** difference this criterion tolerates. |
| 7.9 | each new snapshot is accepted by every recognizer it is wired into, and is `!=` the current default | green |
| 7.10 | `ROOT_CONTEXT_BEFORE_ROOM_RENAME_MD` is accepted on **both** root paths | `is_known_generated_root_context_template(...)` is true **and** a pristine `Role.md` of those bytes reduces to `MINIMAL_ROOT_ROLE_MD` (§3.7 family 2) |
| 7.11 | version bumps | **`global` 6**, `coordinator` 6, `rootAgent` 8, asserted at both the spec and the persisted-state layer. **Round 5 changed `global` from 5 to 6** (§1.1, §5.10 D8a). "At both layers" is not decoration: §9.3 clause 2 limb B shows the persisted-state layer carries most of the sites, and it is the layer nobody swept in rounds 1 to 4 |
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
4. **Exactly four new non-test files** (corrected in round 5): `src-tauri/src/config/entity_prefix.rs`, `src/shared/entity-prefix.ts`, `scripts/room-rename-allowlist.tsv` and **`scripts/room-rename-allowlist.mjs`**, plus the new plan file and the new test files.

   **Rounds 1 to 4 named three and contradicted AC1 point 7, which requires the fourth.** Point 7 says Part A is machine-classified "by a script committed beside the allowlist", and that script is what makes the allowlist re-derivable and the gate runnable by a reviewer rather than by the implementer alone. Clause 4 capped the count at three and did not list it, so AC10 was red on an implementation that satisfied AC1. The script is genuinely required, so the cap moves rather than the script. **The cap is still a cap**: a fifth new non-test file is a finding, and a reviewer should read the list rather than the count.

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
- **R14 (new in round 5). The `touched_owners` budget is the second of two independent budgets this change parks near zero, and the next editor must be told.** `summarized_default_context_meets_size_budget` caps the five touched-owner blocks at `MAX_TOUCHED_OWNERS_BYTES = 6810`. Measured by `ac-dev-rust-grinch-v3` at branch head `bb2a5a65` from the repository's own `token_accounting_report`: write restrictions 3474 + messaging 2147 + CLI 726 + credentials 257 + delegated 205 = **6809**, that is **one byte of headroom**. Together with R10, which this change parks at exactly 34 words of 34, two independent budgets sit at zero or one unit of slack. Both fail loudly at edit time rather than silently, which is the only reason this is a residual and not a blocker.

  **There is a third budget in the same test, and no section of this plan mentioned it before round 6.** `summarized_default_context_meets_size_budget` also caps the whole rendered replica profile: `MAX_FULL_WG_PROFILE_BYTES = 8_313` over `full_wg` (`session_context.rs:10615` at `df494bfa`), with a paired minimum-reduction assertion against `V3_FULL_WG_PROFILE_BYTES = 9_070`. **That is where #1605's platform block actually lands** (main added `assert!(full_wg.contains("## Host Platform Rules"))` at `:10655`), and it is **not** in `touched_owners`: the sum at `:10626-10630` names exactly five terms and the platform block is not one of them. Both ceilings move in the same, favourable direction under this change, because Rule R shortens "Workgroup" to "Room" throughout the rendered profile and main's own tree is already green with the platform block in it; both fail loudly at `cargo test` rather than silently. `ac-dev-rust-grinch-v3` found the omission. **§12 step 12 records both figures**, not just `touched_owners`.

  **The re-base relieves it, and the corrected figure is the implementer's to produce.** #1605 shortened three of the five owner blocks by a derived **262 bytes** on Windows (§5.9's legacy-clause note has the derivation), so the merged tree should land near **6547 against 6810, about 263 bytes of headroom**, not one. **That is a derivation from the diff, not a measurement**, and it is written here as a direction rather than a number to budget against. §12 step 12 requires the implementer to run `token_accounting_report` on the merged tree and record the actual `touched_owners` figure in the PR body; **that figure supersedes both numbers in this bullet** and is what a future editor should read. Owned by whoever next edits any of the five blocks.

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

**Round 5 changes the shape of this section, because steps 0 to 13 already happened.** Twelve commits landed them on `refactor/1614-workgroup-to-room-phase-1` and `ac-dev-rust-grinch-v3` verified most of the result. Steps 0 to 14 below are retained as the **specification of the finished state**, which is what a reviewer checks the tree against and what a future re-run would follow. **What the implementer actually does next is §15**, which is the delta against the existing branch, expressed as new commits rather than as a redo. Two steps changed here regardless, because the base moved:

- **Step -1 (new in round 5; its scope corrected and its resolution list completed in round 6). Merge `origin/main` into the branch.** After §1.2's re-stated entry gate. `git merge origin/main` (**merge, never rebase**, §13.4), resolving conflicts against §1.1's re-base ledger. Land this as its own commit with nothing else in it, so a reviewer can read the merge resolution separately from the corrections that follow. Re-check §3.11's CI job table in the same step, because `pr-regression-gates.yml` moved.

  **What the merge actually produces, measured rather than predicted.** `git merge-tree --write-tree bb2a5a65 df494bfa` (read-only: it writes unreferenced objects and changes no ref, no branch and no working-tree file) conflicts in **exactly one file**, `src-tauri/src/config/seeded_context_templates.rs`, with **three** conflict regions. `config/mod.rs`, `config/seed_manifest.rs` and `config/session_context.rs` all auto-merge, and `session_context.rs`'s live global template auto-merges **correctly**, carrying both main's `{{HOST_PLATFORM_RULES}}` and the branch's Room line. `ac-dev-rust-grinch-v3` ran this first and the architect reproduced it.

  **The resolutions that are not mechanical. Round 6 splits them by whether git will show them to you, because that is the distinction that decides which commit owns each one.**

  *Conflicts git presents. These belong to step -1:*

  - **(i)** `global.current_version` becomes **6**, not 5. In the merged tree this is a **silent auto-merge to 5**, not a conflict, because both sides made the identical textual change 4 to 5. It is listed under conflicts because §15.3 item 2 owns the edit and it must be made here or the tree is wrong from the first commit; see the note below.
  - **(iii)** `is_known_generated_global_template`'s `||` chain: keep **both** `_BEFORE_HOST_PLATFORM_RULES` (main's) and `_BEFORE_ROOM_RENAME` (this plan's). This is a real conflict region. `is_known_generated_standalone_global_template` **auto-merges correctly, keeping both**, and needs no resolution.
  - **(iv)** the `project_specs_bump_*` test keeps **main's** name shape with `v6`, that is `project_specs_bump_global_to_v6_and_add_platform_specs`. **The same conflict region also carries the destructuring**: take main's `let [global, coordinator, windows, linux, macos] = project_specs();`, not the branch's two-element form. Compile-enforced, so a wrong choice is loud, but it is in the hunk and rounds 1 to 5 did not name it. `ac-dev-rust-grinch-v3` found it.
  - **(v), new in round 6 and named by no earlier round.** The assertion message `"recognized v1 global content must land on the current v5 default"` (branch) versus `"...v4 default"` (main) is the third conflict region. Its **assertion**, `parsed["templates"]["global"]["currentVersion"], 5`, auto-merges silently and must become **6**; the message becomes v6. Main's own message was already stale at v4 while the assertion said 5. `ac-dev-rust-grinch-v3` found it by running the merge.

  *A silent auto-merge git will never show you. This belongs to **D3**, not here:*

  - **(ii)** `GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME` re-based onto the **v5** body from §3.12 A1/B1. **Round 6 moves this out of step -1 and gives it to D3 outright.** `ac-dev-rust-v3` is right that the round-5 text asked for something git cannot offer: the constant is a **branch-only addition**, so it merges silently and there is no conflict to resolve. "Resolve (ii) in the merge" and "nothing else in this commit" could not both hold, and §15.3 item 1 asked for it again anyway, which left the implementer deciding which commit carries a 25-line constant rewrite. **D3 owns it.** Step -1 must leave the merged tree carrying two byte-identical 539-byte constants under two names; that state is expected, it is what D3 fixes, and AC7.1b is what makes it impossible to ship. The overlap between (i) and §15.3 item 2 is the same shape but harmless, because `:483` genuinely conflicts and the edit is idempotent: make it in whichever of the two commits, and the other is a no-op.
- **Step 0. Entry gate, and the Table C re-verification.** §1.2; STOP on any mismatch. Rounds 1 to 4 **captured** the two §3.12 Table C values here; they are captured, they are recorded in §3.12 and in the committed test, and round 5 replaces the capture with a **re-verification at the new base**: run `legacy_rendered_default_context_is_frozen` on the merged tree and confirm it is green. It should be, because §3.12 A4, A7, A8 and A9 all reproduce byte-identically at `df494bfa`, so nothing the two reconstructions read has moved. **If it is red, STOP and report a §13.5 finding**; do not update the expected values.
- **Step 0b. Refresh the AC1 base allowlist at the new base.** Run the three sweeps of §9.4 AC1 at **`df494bfa`**, and reconcile against the committed Part A rather than regenerating it. **Round 6 states the row edits explicitly, because rounds 1 to 5 gave only the header figures and left the row operations to a reviewer-facing aside**; `ac-dev-rust-v3` and `ac-dev-rust-grinch-v3` both found that a row left in place turns the new reverse limb red on a correct implementation. Five operations, no others:

  1. **Append three rows**, the three genuinely new `(path, trimmed content)` keys §1.1's table marks **APPEND**, contributing 4 lines. All three are `src-tauri/src/config/session_context.rs`: `let replica = ac.join("wg-1-team").join("__agent_dev");` (`P2-fixture`), `"the platform block must render in the WG profile"` (`P2-fixture`), `// #1605: the platform block renders in the WG profile on every OS (linux/` (`P1-comment`). **Do not append §1.1's other three keys**: they are already on Part A and only their occurrence count rose.
  2. **Delete the `ProjectPanel.tsx:2749` row.** It is **Part A row 366, file line 383**, class `P0-identifier`, content `<Show when={sessionsStore.showCategories && (!filterActive() || matchesFilterText("Workgroups") || filteredWorkgroups().length > 0)}>`, and it matches the base sweep byte for byte. It is P6's fourth R1 resolver, so §5.2 R1 moves it and it must leave Part A.
  3. **Delete the `WorkgroupTask.tsx:70` row.** It is **Part A row 709, file line 726**, class `P1-comment`, content `// Backend uses byte-exact \`name.starts_with("wg-")\` (session/session.rs).`. §15.2 row 6 corrects this comment under Rule P1 clause (b), so it must leave Part A for the same reason as the row above. Leaving either in place makes the reverse limb red on a correct implementation; deleting them makes the **forward** gate red if the edit is not made, which is the stronger guard.
  4. **Correct the header figures**: frontend subtraction 63 to **65**, Part A's frontend half 843 to **841 lines / 775 rows**, Rust to **2819 lines / 2172 keys** with 285 moved, docs to **30 lines / 30 rows** with 533 moved.
  5. **Record in the PR body** the three per-surface subtotals, the Part A file row count (**2,989**, over **2,977** unique keys) and the closing check `` lines(Part A) 3690 + lines moved 883 = 4573 ``. **Lines, not rows**; §9.4 AC1 point 8 carries both numbers and says why they differ.

  **This must land before step 9** in a from-scratch run; on the existing branch it is a correction commit, see §15. A Part A that changed by more than those five operations means the sweep was re-run differently, not that the base moved, and that is a finding.
- **Step 1. The two helper modules.** Create `src-tauri/src/config/entity_prefix.rs` and its `pub mod` line; create `src/shared/entity-prefix.ts`. Add their unit tests. `cargo test --lib config::entity_prefix` and `npx vitest run src/shared/entity-prefix.test.ts` green before anything calls them.
- **Step 2. Parent-repository exclusion.** §5.6. Both tests of AC8 green. This lands early and independently because it is the one item that protects user data, and it is correct on its own even if the rest of the change were reverted.
- **Step 3. The 40 Rust gates and the 6 frontend predicates.** §5.3, §5.4. `determine_next_wg_number` (S4) is deliberately NOT in this step. Run `git grep -n 'starts_with("wg-")\|strip_prefix("wg-")' -- src-tauri/src` afterwards: the only remaining production hit must be `entity_creation.rs:4301`.
- **Step 4. Creation and the Room allocator.** §5.5 and §5.7. After this step `git grep -n 'format!("wg-' -- src-tauri/src` must return only the two `#[cfg(test)]` fixtures (AC2). Add the allocator and creation tests.
- **Step 5. Freeze everything, edit nothing.** §5.10 in full: the three seeded snapshots (D8a, wired into **both** root lists), the **three** dual-use frozen halves (D8b, D8c and D8f's `GENERATED_SKILLS_SECTION_REPLICA_LINE_BEFORE_ROOM_RENAME`), and the `:3860` identifier switch. Copies only, no default or paragraph edited yet. Add `frozen_snapshots_are_byte_exact_at_d7008b34`, `workgroup_git_scope_split_is_correct`'s frozen half, `old_deferred_messaging_paragraph_is_frozen`, and AC7.8's source-digest check. The `assert_ne!` halves are deferred to step 6, where they become meaningful. Verify every copy against §3.12 Table A **before** proceeding; a copy that is wrong here is invisible from step 6 onward.
- **Step 6. Edit the three defaults, the two live halves, and bump the three versions.** §5.10 D8b (the live `WORKGROUP_GIT_SCOPE` text and its two `assert_eq!` counts), D8c (the live `ROOT_COORDINATION_MESSAGING_PARAGRAPH`), D8e (the three defaults, the six `ROOT_ROLE_MD` lines, `PTY_INPUT_COORDINATOR_CONTEXT`, and the one `injected_messages.rs:78` line), and **D8f in full**: the live `:831` line, the two new constants, the compare extension at `:4183-4208` and its corrected comment. Bump `global` **5->6** (round 5; #1605 already took it 4->5), `coordinator` 5->6 and `rootAgent` 7->8, then update the §9.3 clause-2 rows: **3 in limb A and 19 in limb B**, and re-run limb B's sweep at the merged tree rather than working from the table, because the clause and not the enumeration is the gate. **Two of limb A's rows are the `injected_messages.rs:78` edit's twins and are not optional**: `:1331` inside `EXPECTED_SEED` takes the identical byte change under clause 1, and `:1671`'s `EXPECTED_SEED.len()` pin goes 1534 to 1531 under clause 2. Both are red on a correct implementation by construction, because `EXPECTED_SEED` is a hand transcription that exists to go red here (§9.3). Now the `assert_ne!` and `contains` halves are meaningful and must be green, and so must all five behavioral tests of §9.1: `pre_1072_context_still_self_heals`, `current_generation_legacy_context_classifies_current`, `pre_room_rename_skills_section_still_classifies_stale_generated_and_heals`, `edited_pre_room_rename_skills_line_is_preserved_not_healed` and `skills_section_replica_line_split_is_correct`. **D8f's compare extension must land in the same commit as the `:831` edit**, because between the two the recognizer is broken for every installation whose skills section carries the old line.
- **Step 7. The message-filename short prefix and its context/doc text.** §5.9.
- **Step 8. The CLI canonical names, aliases, `value_name`s and every printed string.** §5.8. Update `cli_behavior_contract.rs` under §9.3.
- **Step 9. GUI visible text.** §5.12. `ProjectPanel.tsx:993`/`:995` and their **four** resolvers (`:1057`, `:1058`, `:1071`, `:2749`) move in one edit.
- **Step 10. Documentation.** §5.13, including the five compatibility statements and the new `CHANGELOG.md` entry. `docs/features/context-tracking.md:75` and `:78` keep the literal `%WORKGROUP%` and rename only the prose around it (D17).
- **Step 10b. Append Part B and run the gate.** Run the three sweeps of §9.4 AC1 over the post-change tree. Every unlisted line is either a Rule R miss to fix or a line this change legitimately introduced, and the second kind is appended to the committed allowlist as **Part B** with a class and a one-line justification naming the plan section that requires it (§9.4 AC1 point 4). Then confirm the unlisted count is zero. A row whose class cannot be justified from §5.2 is a Rule R miss to fix, not a row to write. §9.4 AC1 point 7 states which classes may be machine-classified and which five are read by hand.
- **Step 11. Regenerate `src-tauri/module-arcs.txt`** and run AC11.
- **Step 12. Local gates.** `cargo fmt --all -- --check` (in `src-tauri`; a required CI job), `cargo clippy`, `cargo test` (run from PowerShell, not from a Bash shell), `npm run typecheck`, `npm test`, `npm run test:debt`, `npm run check:frontend-dependencies`. **Plus, new in round 5:** run `token_accounting_report` (`cargo test --manifest-path src-tauri/Cargo.toml token_accounting_report -- --ignored --nocapture`) on the merged tree and record **both** budget figures in the PR body: `touched_owners` against its 6810 ceiling **and** `full_wg.len()` against its 8313 ceiling (R14).
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

The base is pinned at **`df494bfa`** (round 5; it was `d7008b34` in rounds 1 to 4). Later movement of `main` alone does not invalidate this plan and does not produce `CHANGES_REQUIRED`. Before the first product mutation and again before PR creation, fetch `origin/main` and classify the drift by changed paths:

- drift touching any §6 path, `.github/workflows/**`, `Cargo.lock`, `package.json`, `dependency-cruiser.config.mjs`, `test-debt.allowlist.json`, `src-tauri/module-arcs.txt` or any `*_BEFORE_*` frozen constant requires refreshing only the affected evidence (§3.2/§3.3 line anchors, §3.11 job list, §3.12 digests, §11.1 baseline) and re-reviewing that evidence;
- drift proven unrelated is recorded and synchronized at the next bounded gate.

**Round 5 tightens three things about how this clause is applied, because rounds 1 to 4 lost a round to it.**

1. **The trigger is paths touched, not semantic relatedness, and "proven unrelated" is the narrow bucket.** The round-4 drift was classified "proven unrelated" on the strength of three unchanged AC1 sweep totals, an unchanged CI job table, and no moved digest file. That evidence is real and it is sound, but it answers a **different question**: it shows AC1's baseline did not move, not that no trigger path moved. Five trigger paths had moved, and one of them, `config/seeded_context_templates.rs`, carried a version bump that collides head-on with this plan's. "Proven unrelated" means **no trigger path is in the changed-path set at all**. If a trigger path moved, the bucket is the first one, however unrelated the change looks.
2. **The classification is not the implementer's to make.** §1.2 already says STOP and request a review. Round 4's classification was made mid-implementation by `ac-dev-rust-v3` and the call belonged to the tech lead. This is restated here so the two sections agree rather than one deferring to the other: **the implementer STOPs and reports; the tech lead classifies; the architect re-derives whatever the classification says is affected, in the plan.**
3. **A refresh belongs in the plan, not in a merge conflict resolution.** The round-4 drift was left to be resolved at step 14's merge. A merge conflict resolution is invisible to plan review, is made under time pressure by one person, and in this case would have had to invent a version-collision policy, re-base a frozen constant and widen a recognizer, none of which a merge tool can surface as a decision. **When a refresh touches a frozen value, a version, an acceptance criterion or an allowlist baseline, it is a plan revision and a new digest**, which is what round 5 is.

Once the PR exists, exact-head GitHub checks and the repository merge policy are authoritative. Continuous pre-PR attestation that `main` never moved is forbidden.

---

## 14. What a reviewer should attack first

**Round 5 first, because the tree is no longer hypothetical.** Rounds 1 to 4 reviewed a plan against a base. Round 5 revises a plan against an **implemented branch** that a reviewer has already read, so the highest-value attacks are different from the ones below, and they are these five, in order:

1. **The re-base (§1.1, §3.12, §5.10 D8a).** Re-run all nine §3.12 Table A digests and all seven Table B digests at `df494bfa`. Exactly one Table A row and one Table B row may differ from rounds 1 to 4, and both must be A1/B1 with the values §3.12 states. If a second row differs, §1.1's ledger is wrong and every downstream claim about "the drift is bounded" is void. Then check the `global` collision resolves to **6** in all four places it appears (§5.10 D8a, §9.1's version test, §9.3 limb B, AC7.11) and that **both** global recognizers keep `_BEFORE_HOST_PLATFORM_RULES` while gaining `_BEFORE_ROOM_RENAME`.
2. **§9.3 clause 2 limb B.** Re-run the sweep command at the merged tree and reconcile it against the 19-item table. This is the clause three parties certified as complete when it was not, so it deserves the sweep rather than the table. A discrepancy is a finding against the plan.
3. **AC1's new reverse limb (§9.4 AC1 point 10).** It exists because the forward gate reported zero unlisted while six unauthorized substitutions shipped. Round 6 inverted the limb, so the **predicate** is the gate and the **five** absence classes are illustrative: do not attack the class list for exhaustiveness, which point 10 has already ruled out as a test. Attack the predicate instead. Check that "a named section of this plan requires that line to move" is decidable for every absence the self-test surfaces, and that no class swallows arbitrary absences, which would restore the blindness the limb is meant to remove.
4. **§9.2's discriminator and §9.3 clause 4.** Clause 4 authorizes 28 fixture edits that Rule P2 previously forbade. The risk is over-application: check the discriminator actually distinguishes the two cases, and check `tests/cli_workgroup_team.rs` still carries `wg-*` acceptance fixtures alongside its moved creation fixtures.
5. **§5.9's legacy clause and §10.2 R14.** The clause substitution is a decision this round makes on the implementer's behalf; check it preserves §5.9's meaning. The R14 figure is a **derivation**, not a measurement, and the plan says so; check the plan requires the real measurement at step 12 rather than shipping the derived number as fact.

Round 1 was rejected on six blocker groups and round 2 on six more. Items 1 to 6 below are the round-1 blockers, each now also carrying its round-2 follow-on, and where the fixes live; a reviewer should confirm the fix rather than re-derive the finding. Items 7 to 12 are the standing hazards round 1 got right.

**The three round-2 blockers that do not map onto a round-1 group, and where they now live.** The preserve half of the scope gate is §6.1's restated constraint, §6.4's placement of `api/identity.rs`, §6.7's two parts, AC10 and §13.2 gate 5. The self-proving allowlist is §9.4 AC1's Part A / Part B split and §12 step 0b. The unsatisfiable `StaleGenerated` assertion is §3.12 Table C's two captures, §7 item 10's two outcomes and §9.1's five behavioral tests. A reviewer attacking this round should attack §9.4 AC1's arithmetic and §5.10 D8f's compare ordering first, because those are the two places where this round adds mechanism rather than correcting text.

1. **The frozen legacy recognizer, now the whole chain and not one function (§3.7 family 3, §5.2 P3, §5.10 D8b and D8f, AC7.8, AC7.14, AC7.15).** Round 1's §5.9 sent four Rule R edits into `session_context.rs:3769-4025`, which reconstructs a user's pre-#1369 context file for byte comparison. Confirm the four lines are gone from §5.9's edit set, that the only change inside the range is the single `WORKGROUP_GIT_SCOPE_BEFORE_ROOM_RENAME` identifier at `:3860`, and that AC7.8's source digest `940FA357...16C2` reproduces after that one-token substitution. **Then keep going past the end of that function**, which is what round 2 did not: `reconstruct_legacy_rendered_default_context` (`:4089`) also runs `is_provably_generated_legacy_skills_section` (`:4183`), which recomputes the **live** `render_skills_section` (`:812-919`), and that function carries the retired token at `:831`. Confirm D8f exists, that its frozen copy matches `A5C74FD6...7EC9` at 131 bytes **including the trailing newline**, that the compare extension is applied **before** `normalize_context_for_compat`, so the swap sees the line's terminating newline, and before both existing compares, and that AC7.15's two behavioral tests build their fixtures from the frozen constant while AC7.14 pins that constant to §3.12 Table B, so the **pair** supplies the external anchor. **Round 3 stated this ordering backwards** ("applied after `normalize_context_for_compat`"), contradicting §5.10 D8f on the single ordering question D8f devotes a bullet to; D8f's code block is authoritative and correct, and this item is corrected in round 4. `ac-dev-rust-grinch-v3` found it in round 3.
2. **The three dual-use constants (§5.10 D8b, D8c, D8d).** Each needs a split or an in-place freeze, and each frozen half needs its §3.12 digest. Check `WORKGROUP_GIT_SCOPE` first: its replacement text sits at exactly the word ceiling `session_context.rs:6107` enforces, and a text that reads better but adds one word turns the suite red.
3. **The GUI sweep and its allowlist (§3.8, §9.4 AC1).** Round 1's 36 needles went green with seven production Workgroup strings shipping. Confirm the sweep reproduces 906 lines / 41 files at base, that the allowlist is committed with a class per row, and that the post-change unlisted count is zero. A post-change count still equal to 906 is a finding.
4. **The Rust reader-facing inventory (§3.14, §6.1, §13.2 gate 5).** Confirm the nine files round 1 omitted are in §6.1's edited list, and that gate 5 no longer requires "exactly the §6 paths" in a way that punishes a correct edit.
5. **`clap` completeness (§5.8, §9.4 AC5).** `role_experiment.rs:95`'s `value_name` and `team.rs:61`'s third `help` string. Then check AC5 walks the tree rather than a list, and, more importantly, that it **asserts membership**: `team create` (a depth-2 leaf), `role-experiment` (hidden) and `role-experiment variant set` (depth 3) must each be in the walked set, and the first two must have rendered the strings whose omission caused G5. Round 2's `checked >= 25` guarded "walked nothing" while the failure round 1 shipped was "walked one level", which reports 40 and passes. The derived floor is 75 on Windows and its arithmetic is stated in AC5; because clap's synthetic `help` nodes mirror the sibling tree and recurse, the real `walked.len()` is far above it, so the floor is a vacuity guard and the membership assertions are the criterion. **Check the three membership assertions build their keys from `root.get_name()`.** Round 3 hard-coded `"agentscommander team create"`, which is wrong: the root name is `agentscommander-new`, from `CARGO_PKG_NAME`, because `Cli` sets no `#[command(name)]`. A reviewer seeing three green membership assertions cannot tell whether they were weakened or deleted to green a panicking test, and weakening them restores exactly the vacuity H6 removes, so confirm the assertions are **present, three in number, and prefix-derived**. **Round 5 adds a fourth thing to confirm, and it is the one that actually panicked in delivery: the `--retain-room <RETAIN_ROOM>` rendering assertion must target `role-experiment run`, not `role-experiment`.** `retain_workgroup` is a field of `RunArgs` on the `Run` variant, and a clap parent renders `<COMMAND>` rather than a child's arguments, so asserting it on the parent fails on a **correct** implementation. The membership assertion stays on the hidden parent (that is what guards H6); only the rendering assertion moves down. Note what this means for the round-3 probe evidence: the probe was a synthetic flat tree with no parent/child argument split, so it could not have exhibited this, and a green probe was not evidence about where help text renders.
6. **The arc count (§11.2, §11.5).** 18, not 19. `agentscommander_lib::config` must not appear as a source in `post \ pre`.
7. **The `room-*/` gitignore entry (§3.6, §5.6, AC8).** Neither the issue nor the assignment names it. If it is missing or lands after step 4, a Room created inside a git-tracked project is corrupted by the next parent `git checkout`. This remains the highest-severity item in the change.
8. **The two `&name_str[3..]` slices (§3.2, §5.3).** A mechanical `starts_with` swap that leaves the literal `3` mis-slices every Room name and can panic. Check `entity_creation.rs:2965` and `:3489` first.
9. **`determine_next_wg_number` (S4).** It is the one `strip_prefix` site that must stay Room-only. A sweep that made it dual would silently break requirement (B) while every dual-prefix test stays green.
10. **The six frontend predicates (§3.3).** The assignment says the frontend is unaffected. Verify F1 through F6 individually; each fails silently.
11. **`PURGE_WG_ACTION` and the other §3.10 values, now including `%WORKGROUP%`.** Anything in that table appearing in the diff is a compatibility break dressed as a rename. `known_default_sha256` gaining a second entry is the specific shape to look for.
12. **The negative test evidence (§9.2) and the four clauses (§9.3).** An existing `wg-*` fixture that seeds a directory and asserts it is **discovered, listed, addressed or deleted** and that had to be edited means dual-prefix acceptance is broken. Ask why, do not accept the edit. **Apply §9.2's discriminator before treating a moved fixture as that finding**, because round 5 added clause 4 for the fixtures whose subject is what creation now **produces**: the question is whether any asserted value changed or only the prefix of a directory the fixture itself constructs. Conversely, §9.3's **four** clauses are the closed set of permitted test edits: an edit that satisfies none of them is a finding.

    **Round 5 withdrew clause 2's completeness claim, and a reviewer should read that as a change of gate, not a change of count.** Rounds 2 to 4 said clause 2's list was complete at eight rows; it was not, and the reason is structural rather than arithmetic. Clause 2 has a **size** limb and a **version** limb. The size sweep was run by three parties and is sound. **The version sweep was never run by anyone**, although this plan's own AC7.11 says the version is asserted "at both the spec and the persisted-state layer", which states outright that the version limb has more than one layer. §9.3 limb B now runs it, at `df494bfa`, with the command written out: **17 assertion sites plus 2 test-name renames**, including a **second** `rootAgent` site (`root_agent.rs:2471`) that rounds 2 to 4 never named, and a fixture value (`:3982`) that is a clause-2 site despite not being an assertion. **Every clause's list is now illustrative and the clause is the gate**, clause 2 included. So a site missing from §9.3's table is authorized by the clause and is **not** a finding against the implementation; it is a finding against the plan, worth reporting so the table is corrected. That inversion is the round-5 change and it is deliberate: an enumeration cannot be the gate for a defect class whose failure mode is omission, which is the same sentence AC1 and AC5 already use about round 1's needle sets.

**Where this plan disagrees with a round-1 reviewer.** Nowhere on substance. Every blocker in both verdicts is accepted and fixed. Two round-1 reviewer statements are refined rather than contradicted, and both are noted here so the refinement is visible:

- `dev-rust` proposed freezing `3769-4025` under Rule P3 wholesale. This plan does that **and** adds the transitive-closure obligation, because freezing the body alone leaves `WORKGROUP_GIT_SCOPE` free to move underneath it and the freeze would be nominal.
- `grinch` recommended AC5 walk `Command::get_subcommands` recursively **or** drive `<bin> <sub> --help`. This plan takes the first, in-process, because it needs no built binary and no process spawn, so it runs under `cargo test` rather than needing step 13's built binary. **Round 2 justified that choice by saying the walk "runs on all three CI legs and not only the Windows one", and that reason is false by this plan's own measurement**: §3.11's job table calls the Linux and macOS legs build/clippy legs and §13.3 states that `rust-regression` (windows) is the only leg that runs the Rust test suite. §3.11 and §13.3 are the correct pair, the decision is unaffected, and the reason is corrected here, in §5.8, in §9.1 and in AC5. `ac-dev-rust-v3` found this in round 2.

**What this plan adds that neither reviewer found.** Three items from round 2, each a silent-failure class of the same shape as G1 and G2, all three since verified exact by both reviewers: the second root wiring in `migrate_root_role_file` (§3.7 family 2, §5.10 D8a), the injected-messages `known_default_sha256` recognizer and its `%WORKGROUP%` token (§3.7 family 5, D17), and the `WORKGROUP_GIT_SCOPE` compactness budget that constrains what the replacement text may say (§5.10 D8b, R10).

**Three more found in round 3, while re-deriving the reviewers' findings rather than transcribing them.** Each is a claim in round 2 that measurement contradicted, and each is corrected in place with the measurement stated: §3.14's step 1 asserted the crate uses only the column-0 `#[cfg(test)] mod ... {` shape, which is false and is the mechanical reason `config/root_agent.rs` appeared with 3 candidate lines against a raw sweep of 50; §3.8(d) declared "3 sites" over an enumeration of two and cited `WorkgroupGroupRail.tsx:72`, the predicate line, where the visible label is at `:73`; and §9.4 AC1's prose claimed `docs/assets/` was excluded by pathspec from all three sweeps when no command excluded it, so 14 lines including a binary file were inside the gate. None of the three changes a decision. All three are the kind of claim a reviewer uses as a completeness check, which is why they are corrected rather than left.

**Eleven more found in round 5, by implementing the plan and reviewing the implementation.** These are P1 to P11 of the round-5 assignment and they are listed in the preamble with the section that now carries each. Two are worth calling out here because they are defects in this document's **method** rather than in its facts: clause 2's completeness claim, which three parties certified and which failed because nobody ran the version limb of a sweep the plan itself specified (§9.3); and AC1's one-directional blindness, which reported zero unlisted while six unauthorized substitutions shipped, because a renamed line leaves the sweep rather than entering it unlisted (§9.4 AC1 point 10). Both are now closed by mechanism rather than by a corrected list.

---

## 15. The delta against the existing branch (round 5)

**This section is what the implementer does next.** Twelve commits already exist on `refactor/1614-workgroup-to-room-phase-1` at `bb2a5a65`, they are pushed, and `ac-dev-rust-grinch-v3` verified most of the result at HEAD. **Nothing in this section asks for a redo.** §12 remains the specification of the finished state; this is the diff between that state and the tree as it stands.

**What is verified correct and must not be touched.** Re-deriving any of this is wasted work and re-editing it is a risk with no upside:

- All nine §3.12 Table A digests reproduce byte-exact at HEAD after every edit, including AC7.8's frozen function, which carries exactly one tolerated identifier substitution and hashes back to `940FA357...` at 15,027 bytes.
- The `ROOT_ROLE_MD` live half is exactly the six specified lines. The two pre-existing frozen root generations that were briefly broken and reverted are fully restored, differing from base by exactly the one authorized D8c line.
- D8f in full, with the compare extension in the same commit as the `:831` edit and the swap ordered before `trim_end`.
- §6.7 Part 1 clean. Of the preserved 16, only `commands/session.rs` appears, with its single authorized `#[cfg(test)]` hunk. Zero of the 23 preserved TypeScript files appear.
- AC2, AC8, AC9 and AC11 (18 arcs, 0 removed, SCC identical set-to-set), F1 to F6 all correctly rewired, and the AC1 base sweeps reproducing 906/41, 3090/94, 563/64 = 4559 at the old base.
- The nine unauthorized hunks from the killed first session are fully reverted, verified one by one.
- The two §3.12 Table C values, captured at step 0 at `d7008b34` and now recorded in §3.12.

### 15.1 Ordered work

Land these as new commits, in this order. The first is a merge and stands alone.

| # | Commit | What |
| --- | --- | --- |
| **D0** | merge | §12 step -1. `git merge origin/main` at `df494bfa`, resolving the **four conflict-side** resolutions (i), (iii), (iv) and (v). Nothing else in this commit. **Resolution (ii), re-basing the frozen snapshot, is deliberately NOT here**: it is a branch-only addition that merges silently, so there is nothing to resolve, and D3 owns it (round 6, `ac-dev-rust-v3`) |
| **D1** | fix | The HIGH code defect, one line. Restore `"workgroupRoot"` at `commands/task.rs:167`. **Run §9.4 AC1 point 10's self-test against `bb2a5a65` before this commit**, while §15.2 rows 1 to 5 are still on the tree as the six Part A rows point 10 names; it is the only point at which the limb can prove itself. **Row 6 is not among them and cannot be**: it is a stale comment rather than a substitution, its key is unchanged and therefore present, and the forward gate is what catches it |
| **D2** | fix | The five remaining unauthorized substitutions (15.2 rows 2 to 6), including row 6's fixed replacement bytes |
| **D3** | fix | Re-base the frozen global snapshot (step -1's resolution (ii), owned here) and the version pins to the merged tree (15.3) |
| **D4** | test | The twelve missing §9.1 tests (15.4) |
| **D5** | chore | Refresh the AC1 allowlist and run both limbs of the gate (15.5) |
| **D6** | gates | §12 steps 11 to 13 re-run on the merged tree, including `token_accounting_report` for R14 |

### 15.2 The six unauthorized substitutions that survive at HEAD

All six are lines the branch's **own** Part A allowlist classifies as preserved, and all six are invisible to the forward AC1 gate at `bb2a5a65` for the reason §9.4 AC1 point 10 now states. Ranked.

**Row 6 is a different kind from rows 1 to 5, and the difference is load-bearing rather than pedantic.** Rows 1 to 5 are substitutions: the branch changed the line's bytes, so the Part A key is **absent** at `bb2a5a65` and the reverse limb surfaces it. **Row 6 changed nothing**; the comment is byte-identical to its Part A row and what moved is the code it describes, so its key is **present** and the reverse limb cannot see it by construction. The title keeps "six" because six defects are listed and six corrections are mandated, but §9.4 AC1 point 10's self-test counts what the limb can surface, which is rows 1 to 5 only, landing as six Part A rows because row 3 spans two. Row 6 is caught by the **forward** gate instead, once step 0b deletes its Part A row.

| # | Severity | Site | Fix |
| --- | --- | --- | --- |
| 1 | **HIGH** | `commands/task.rs:167`. The Tauri event payload key `workgroupRoot` was renamed to `roomRoot`; **nothing consumes `roomRoot`**. `src/shared/types.ts:1559-1573` still declares `workgroupRoot` on both the `"manual"` and `"poll"` variants; `src/terminal/App.tsx:344-346` and `src/sidebar/App.tsx:792-795` read it, so a manual `TASK.md` edit no longer refreshes sibling sessions until the 15-second poll repairs the display. The **poll** variant, emitted from `ac_discovery.rs:945` through the typed `TaskUpdatedPayload`, is correct, so one event now has two payload spellings depending on which path emitted it. Violates requirement (H) and Rule P0 | Restore `"workgroupRoot"`. One line |
| 2 | MEDIUM | `commands/entity_creation.rs:7959` spawns `cli::room::tests::cli_room_lock_order_inversion_child`, but the child is still `cli_workgroup_lock_order_inversion_child` in `cli::workgroup` (`cli/workgroup.rs:624`, unchanged) and `test-debt.allowlist.json:197` still pins that FQN. Masked because the test is `#[ignore]`d, so `cargo test` stays green and `test:debt` still allowlists it. Rule P0, and it silently breaks the manual acceptance path | Restore the original FQN. **Do not** rename the child to match: `test-debt.allowlist.json` is §6.7 Part 1 and must not appear in the diff |
| 3 | LOW | `cli/role_experiment.rs:1402` and `:1439` renamed the warning **code** `run_workgroup_retained` to `run_room_retained`. It is serialized into `run.json`; both lines are `P0-identifier` in the branch's own allowlist. No consumer reads it and role-experiment artifacts are disposable, so runtime impact is nil | Restore both |
| 4 | LOW | `cli/list_peers.rs:575`'s `log::warn!` body was renamed while the identical string is correctly preserved at `commands/ac_discovery.rs:194` and `config/session_context.rs:1539`. One of three moved, which is exactly what D18 exists to prevent | Restore `:575`. This is Rule P4 and **not** the P4b pair: `:654`/`:659` are the P4b pair and correctly move together |
| 5 | TRIVIAL | `commands/entity_creation.rs:4883`'s `.expect("workgroup")` became `.expect("room")` on a fixture that creates `wg-1-dev-team`, so the panic label is now wrong about its own directory. Allowlisted `P2-fixture` | Restore |
| 6 | TRIVIAL | `src/terminal/components/WorkgroupTask.tsx:71` **at branch head `bb2a5a65`** (it is `:70` at `df494bfa`; `:71` at the base is the `WG-19` line, and §1.1's line-number table records this exception). Its comment still says the backend uses `name.starts_with("wg-")`, while `session/session.rs:249` now calls `has_entity_prefix` | **Update the comment**, do not restore it. This is Rule P1 clause (b), a comment that contradicts its code after the change. §3.14's list of six such comments is Rust-only and does not name this one; **§5.2 P1 clause (b) is a predicate rather than a list** and authorizes the correction regardless of what that list holds, and the comment's substance (keep the gate case-sensitive) stays correct. **The replacement bytes are fixed below, not left to the implementer** |

**Row 6's replacement bytes, decided here in round 6, because a numeric acceptance criterion depends on them.** `ac-dev-rust-grinch-v3` raised that the round-5 plan mandated this correction, did not say what the corrected comment says, and then demanded an exact post-change frontend count that the answer changes. Replace **line 71 only**, at branch head:

```
- // Backend uses byte-exact `name.starts_with("wg-")` (session/session.rs).
+ // Backend is byte-exact (`has_entity_prefix`); `pathHasEntityDirSegment` mirrors it.
```

Lines 72 and 73 (`// Keep this regex case-sensitive so the UX gate matches; matching \`WG-19\`` and `// here would render the buttons enabled but every click would fail.`) stay **byte-identical**, which is why the replacement names `pathHasEntityDirSegment`: it gives "this regex" on the next line an antecedent, since the function itself no longer holds one. Three properties make the arithmetic determinate, and all three are measured against the AC1 sweep regex: the old line **is** in the 906 base sweep, the replacement **is not** matched by it, and line 72 stays matched with unchanged content. So **exactly one base line leaves the frontend sweep and none enters it**, the subtraction is 65 (§9.4 AC1 point 8), and point 9's prediction is **862**.

**Why the gate could not see any of them, stated once so the fix is not mistaken for a gate that worked.** A renamed line stops matching the base sweep and **disappears** from the post-change output, so it never appears as unlisted. §9.4 AC1 point 10's reverse limb is the mechanism that catches this class. **It confirms itself against `bb2a5a65`, not at D5**, and round 5 got that backwards: run against the tree as it stands today the limb must surface **rows 1 to 5, as exactly six Part A rows** (row 3 spans two, Part A rows 977 and 979), as absences no plan section authorizes, and that is what proves the limb works; run at D5, after D1 and D2 have made the corrections, the expected result is an **empty** fourth column. **Row 6 is outside both runs**, for the reason stated above the table: its key never left the sweep, so the forward gate owns it. Point 10 carries the two runs as a table and states the six row numbers.

### 15.3 Re-basing the frozen global snapshot and the version pins

At the merged tree, and only there:

1. `GLOBAL_CONTEXT_TEMPLATE_BEFORE_ROOM_RENAME` holds the **v5** body, taken from `git cat-file blob df494bfa:src-tauri/src/config/session_context.rs` lines 2513 to 2537: declaration digest `D9E93582...` at 574 bytes, rendered value 564 bytes and `D094106B...` (§3.12 A1, B1). It currently holds the v4 body.
2. `global.current_version` becomes **6**.
3. Both `is_known_generated_global_template` and `is_known_generated_standalone_global_template` accept `_BEFORE_HOST_PLATFORM_RULES` **and** `_BEFORE_ROOM_RENAME`. Neither loses an entry.
4. AC7.1's expected values become 564 and `D094106B...`; AC7.1b is added.
5. `seeded_template_versions_were_bumped` asserts **(6, 6, 8)**.
6. Every `global` row of §9.3 limb B goes to 6, including the two test-name renames. The `project_specs_bump_*` test keeps **main's** name shape: `project_specs_bump_global_to_v6_and_add_platform_specs`.
7. Re-run the six-name `test-debt.allowlist.json` check of §9.3 before renaming either test.
8. **Three global assertion messages name the version the content lands on and go stale at 6** (§9.3 limb B's note): `seeded_context_templates.rs:2613`, `:3024` and `:3031` at `df494bfa`. Only the first is a merge conflict (step -1's resolution (v)); **the other two auto-merge silently** and nothing will present them. All three become v6.

### 15.4 The twelve missing tests

None of these exists on the branch and there are **zero** new test functions in `src-tauri/tests/` in the whole range. Four Rust integration tests: `room_add_creates_a_room_directory`, `room_list_and_workgroup_list_produce_identical_stdout`, `room_list_reports_a_mixed_root`, `purge_room_and_purge_wg_produce_identical_outbox_messages`. Four API and snapshot fixture twins: `api/identity.rs`, `api/actuation.rs`, `pty/terminal_snapshot/acceptance_tests.rs`, `pty/terminal_snapshot/resource_tests.rs`, all four of which carry zero `room-` fixtures today, and **two of which are among the three `#[cfg(test)]` edits §6.1's table requires** rather than merely permits. Four frontend tests: `path-extractors.test.ts` (F1), `WorkgroupGroupRail.test.tsx` (F4/F5), `WorkgroupTask.test.tsx` (F6), `profile-utils.test.ts` (F2/F3); only `entity-prefix.test.ts` landed, and it tests the helper in isolation rather than the six call sites.

All six frontend predicates **are** correctly rewired, so this is a coverage gap and not a live defect. It is still the difference between AC3, AC4, AC6 and requirement (D) resting on committed gates and resting on unrecorded manual checks, and §14 item 10 says each of F1 to F6 fails silently.

**On `purge_room_and_purge_wg_produce_identical_outbox_messages` specifically: write it, do not substitute for it.** §9.1's note carries the mechanism the implementer's report thought was unavailable: `cli/purge_wg.rs` writes the outbox file at `:192` and only then enters the response wait at `:212`, bounded by `args.timeout`. The artifact is observable without a daemon.

### 15.5 Allowlist and gate refresh

Per §12 step 0b, whose five row operations are now stated there explicitly: **append three rows** (the three keys §1.1 marks APPEND, 4 lines), **delete two** (`ProjectPanel.tsx:2749` at Part A row 366 and `WorkgroupTask.tsx:70` at Part A row 709), correct the frontend subtraction to **65** and Part A's frontend half to **841 lines / 775 rows**, and restate the closing check as `` lines(Part A) 3690 + lines moved 883 = 4573 `` with the Part A file row count (**2,989** rows over **2,977** keys) carried beside it. Then run **both** limbs of AC1: forward (zero unlisted) and the reverse limb (every Part A row still present, every absence authorized by a named plan section, the fourth column empty). **The reverse limb's self-test is a separate run against `bb2a5a65` and must be done before D1**, not at D5; §9.4 AC1 point 10 carries both runs. Then re-run AC1b in its restated form: eight needles at zero, two needles at exactly the six enumerated permitted hits.

### 15.6 What round 5 deliberately leaves alone

- **The freeze work.** Every frozen byte is where it should be and AC7.8 reproduces exactly. Nothing in §5.10 D8b, D8c, D8d, D8e or D8f changes except D8a's global row.
- **The nine reverted hunks.** Verified reverted one by one; no further action.
- **The `pty::container_backend` flake.** Seen once, passed 3 of 3 in isolation and in every later full run, and its file is not in this branch's diff at all. Not this change's.
- **§3.7 family 1's table stays at three rows**, and round 6 does not grow it to six. Both reviewers said so independently: the `platform.*` disposition in §5.10 D8a is genuinely measured and neither would re-derive the table. What round 6 **did** change there is the base it is stamped at, its `global` **value** (4 was the `d7008b34` figure; `df494bfa` is 5) and its seven line anchors, which is a different defect from the row count and is the one both reviewers actually reported.
- **The pre-existing em-dashes.** Not swept, for the fourth round running.
