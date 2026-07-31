# Implementation Plan: #1177 Remove confirmed dead code (mechanical cleanup)

Status: READY_FOR_IMPLEMENTATION

Consensus pass complete (Step 7). Enrichment by `dev-rust`, `dev-webpage-ui` and `dev-rust-grinch` is integrated below. None of the three edited this file; the architect consolidated. Grinch's three blockers are resolved in Sections 5.2 (B1), 6.2 and 9.5 (B2), and 5.1 plus 9.5 (B3). `dev-rust`'s correction of the Section 2.3 premise changes the verification frame and is folded into Sections 2.3, 6.2, 6.3 and 9. `dev-webpage-ui`'s two content corrections are folded into Sections 2.4, 3.2 and 12. Every finding and its resolution is preserved in Section 13. No implementation decision is left open (Section 11).

Full path. This plan supersedes the Step 4 draft.

## 1. Issue, baseline, and objective

- Issue: https://github.com/mblua/AgentsCommander/issues/1177 (`chore: remove confirmed dead code (mechanical cleanup, ~1000 LOC)`).
- Branch: `chore/1177-remove-dead-code`, created from `main` at `fae6b09466db27c30fae404c04102e294cf1b5d6`.
- **Baseline commit for every coordinate and every verification command in this plan: `fae6b09`.** The only commit on the branch before implementation is `092d85c`, which adds this plan file and nothing else (`git diff --stat fae6b09..092d85c` is one file, insertions only), so all `fae6b09` coordinates remain valid at HEAD. Verified independently by `dev-rust`.
- Delivery classification: FULL. Not because of size, but because four properties make it non-mechanical: (a) PR CI compiles Rust only on Windows; (b) the confirmed items split into three distinct deletion classes, and deleting by coordinate without reading the context breaks the build or the tests; (c) `deliver_wake_via_api` needed an architectural decision, resolved in Section 4.4; (d) for most items in scope **the compiler cannot see the dead code at all** (Section 2.3), so text search is the only verification and it has to be specified precisely enough to be decidable.

Objective: remove production code that has no consumer, with zero behaviour change, zero test-coverage loss, and no regression of the current baseline.

Non-objective: this is not a refactor. Nothing is renamed, moved for style, or improved. Every edit either deletes an unreachable item or is a mandatory companion edit that keeps the build, the lints and the surrounding documentation truthful.

## 2. Verified current state

Every coordinate below was re-verified against the working tree at `fae6b09` by the architect, and independently re-verified by `dev-rust` (Rust) and `dev-webpage-ui` (frontend) during enrichment. Where the three disagreed, the disagreement and its resolution are in Section 13.

### 2.1 Baseline signals

| Signal | Command | Result at `fae6b09` |
| --- | --- | --- |
| Frontend typecheck | `npx tsc --noEmit` | exit 0, no diagnostics |
| Frontend unused check | `npx tsc --noEmit --noUnusedLocals --noUnusedParameters` | 9 errors in 8 files, reproduced verbatim in Section 5.4, F3 |
| Rust lint | `cargo clippy --workspace --all-targets --all-features` | zero warnings |
| Rust dead-code, lints forced | `RUSTFLAGS="--force-warn dead_code" cargo check --lib --bins` | a fixed set of warnings in the crate, of which exactly **three** are items this plan touches (Section 2.3) |

### 2.2 CI reality

`.github/workflows/pr-regression-gates.yml`, read at `fae6b09`:

| Job | Runner | Commands |
| --- | --- | --- |
| `test-debt` | `ubuntu-latest` | `npm run test:debt` |
| `rust-regression` | `windows-latest` | in `src-tauri`: `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib --bins --tests` |
| `windows-release-cli-smoke` | `windows-latest` | `npm run build:prod`, `npm run smoke:cli-release-windows` |
| `frontend-regression` | `ubuntu-latest` | `npm run typecheck`, `npm test` (with the #480 known-debt guard) |

`release.yml` is the only workflow that builds Rust on `ubuntu-22.04` and `macos-latest`. It builds through `tauri-action` and has **no clippy step and no `-D warnings`**. The repository has no `[lints.rust]` section, no `.cargo/config.toml`, and no `RUSTFLAGS` anywhere. Verified at plan time.

Three consequences the implementer must internalise:

1. **No PR job compiles Rust for Linux or macOS.** A deletion that is live only under `#[cfg(unix)]` / `#[cfg(not(windows))]` passes every PR gate and **fails to compile** at release. This is the severe failure mode, and it is closed by Rule 1 (Section 6.2).
2. **`-D warnings` exists only in the Windows clippy step.** A *new warning* that appears only on Linux would therefore not break a release build. It is a hygiene defect, not an outage. This distinction matters for the `allow` removals in batch 5b and is reflected in Section 6.3.
3. **`cargo clippy --all-targets -- -D warnings` still makes any new warning a hard PR failure on Windows.** A leftover unused import after a deletion is not a nit; it fails the build. The companion edits in Section 5 are mandatory, not optional.

### 2.3 What the compiler can and cannot see (corrected premise)

**The Step 4 draft said the 24 confirmed items are what "rustc reports as `dead_code` in a production build". That was wrong, and the correction changes how this cleanup must be verified.** The 24 came from the inventory's occurrence-counting sweep, not from the compiler.

Under `RUSTFLAGS="--force-warn dead_code"` (which overrides every `#[allow(dead_code)]` in the source without modifying a byte), the crate emits a fixed set of warnings, and **only three of them are items this plan touches**:

| Item | Visibility | Reported by rustc? |
| --- | --- | --- |
| `build_attempt_injection` | private `fn` | **yes** (its `#[allow]` is overridden) |
| `read_task_fields_at` | private `fn` | **yes** |
| `write_team_config` | `pub(crate) fn` | **yes** (not externally reachable) |
| `workspace_dir_label`, `read_instance_profile_override`, `read_origin_default_profile`, `validate_codex_home_value`, `product_name`, `is_stage`, `due_pty_input_ids_offloaded`, `deliver_wake_via_api` | `pub` in a `pub mod` reachable from the crate root | **no** |
| `_manifest_path_for_docs` | private `fn`, `_`-prefixed | **no** (the `_` prefix exempts it from the lint) |

The exemption is a language rule, not an accident: `lib.rs` declares `pub mod api`, `pub mod cli`, `pub mod commands`, `pub mod config`, `pub mod phone`, and each relevant submodule is `pub mod` in turn, so those eight items are part of the crate's public surface and rustc will not call them dead however few callers they have. `dev-rust` measured this; it is also derivable from the visibility of each item, which was checked symbol by symbol.

**The practical consequence, and it is the most important sentence in this plan: for those nine items the compiler is not a safety net.** If a deletion strands another `pub` item, clippy will not say so, on any platform. Text search is not an auxiliary check here. It is the only check. That is why Section 6.2 specifies it in enough detail to be decidable, and why Rule 6 (cascade) exists at all.

### 2.4 The three deletion classes

The confirmed items do not share one removal procedure:

- **Class A, atomic.** Exactly one occurrence in the code roots at `fae6b09`: its own definition. Deleting the definition cannot break compilation, and no `#[cfg(unix)]` caller can exist, because a gated caller would necessarily be a second occurrence. 10 items.
- **Class B, dead in production but exercised by inline `#[cfg(test)]` code.** Deleting the item alone breaks `cargo test`. This plan takes 2 of them and rejects the rest (Section 3.2).
- **Class C, the deletion drags live production code.** `role_path` is written at `commands/role_templates.rs:642`; `ReadOnlyCanonical` is matched at `config/seed_manifest.rs:267` and `:298`. Not dead. Excluded.

### 2.5 Cascades: what the deletions strand

These would break CI, or leave the tree less clean than it started, if the implementer deleted only what the issue lists.

| Deletion | Cascade | Consequence if ignored |
| --- | --- | --- |
| `deliver_wake_via_api` (`api/actuation.rs:186`) | sole caller of `build_inline_wake_message` (`:150`) and sole user of `DeliveryOutcome` (`:25`); `use crate::phone::types::OutboxMessage;` (`:19`) is used only by `build_inline_wake_message` | leftover unused import fails `-D warnings`; two new dead `pub` items that clippy cannot see stay in the tree |
| `deliver_wake_via_api` | the module doc at `api/actuation.rs:1` to `:11` describes this file as the actuation seam and names the `MailboxPoller::new()` call that disappears with it | the file's own documentation describes code that no longer exists |
| `deliver_wake_via_api` | `phone/mailbox.rs:6205` to `:6214` is a load-bearing backstop comment that enumerates "the two callers for which this Err is safe", one of which is the deleted inline API send, mapped to `DeliveryOutcome::Rejected` | a safety comment claims two safe callers when only one remains; it names a type that no longer exists |
| `_manifest_path_for_docs` (`cli/agency_templates.rs:1005`) | `agency_manifest_path` appears in that file only at the `use` list (`:11`) and inside this function (`:1006`) | leftover unused import fails `-D warnings` |
| `read_task_fields_at` demotion | the `mod tests` doc at `commands/task.rs:345` to `:348` describes the helper as production code that issue #301 "turns on"; #301 is CLOSED | the test module documents a production intent that no longer exists |
| `profileCellOrDefault` (`src/shared/profile-utils.ts:99`) | sole user of the module-private `EMPTY_CELL` (`:26`) | `tsc --noUnusedLocals` reports a new TS6133, so the issue's own acceptance criterion cannot be met |
| deleting the 2 stale `SessionList.tsx` / `Toolbar.tsx` doc lines | the mermaid graph in `docs/reference/architecture.md` hangs `SessionItem`, `SettingsModal` and `OpenAgentModal` off those two nodes (`:179`, `:181`, `:182`) | three live components lose their parent edge; the diagram becomes more wrong, not less |

`dev-rust` re-derived the cascade set independently by reading all ten Class A bodies and confirmed there is no eleventh. `dev-webpage-ui` swept every module-private declaration in the files F1 and F2 touch and confirmed `EMPTY_CELL` is the only frontend companion of that kind.

Verified as NOT cascading (each retains at least one live user after the deletion): `capitalize_suffix` (`config/profile.rs:88`, via `mutex_name`), `binary_suffix` (8 callers), `origin_matrix_dir_for_launch_path` (`config/coding_agent_profiles.rs:659`), `read_replica_profile_result` (5 live callers), `read_tooling_string`, `normalize_profile_letter`, `validate_expanded_codex_home_value` (6 live callers in `config/agent_command.rs` plus `config/settings.rs:1365`), `due_pty_input_ids` (`api/message_store.rs:3644`), `agency_manifest_path` in its own module (`commands/role_templates.rs`, 6 sites), `AC_WORKSPACE_DIR` (`src/shared/path-extractors.ts:9`, `src/sidebar/components/EditTeamModal.tsx:115`), `getConsoleText` (`src/shared/voice-recorder.ts:265,:333`), `PhoneMessage` (`src/shared/ipc.ts:811`), `ProfileCellConfig` (`profile-utils.ts:93,:107`), `cellForLetter` (`profile-utils.ts:174`), `ProjectState` (`WorkgroupGroupRail.favorites.test.tsx:44`), `FakeTransport` (`WorkgroupGroupsModal.nonstop.test.tsx:59,:86`), `AgentPickerModal` (`AcDiscoveryPanel.tsx:6`, `AgentPickerModal.test.tsx:4`), `otherProjectPath` (`WorkgroupGroupRail.favorites.test.tsx:217,:232,:435,:448`), `wg(...)` (5 uses in the same file), `TeamConfigReadError::class` (`src/session/context_alerts.rs:1579`, production: that file's only `#[cfg(test)]` starts at `:1720`).

**One correction to the Step 4 draft, found by `dev-webpage-ui` and re-verified here.** The draft listed `AgentCreatorAPI` as not cascading, citing `src/sidebar/stores/project.ts:494`. The *object* does survive, but that line uses `pickFolder`, not `createFolder`. `rg -w createFolder` over `src/` returns exactly two lines: the definition at `src/shared/ipc.ts:1082` and `NewAgentModal.tsx:52`, which F1 deletes. **The draft verified survival at symbol granularity and missed member granularity.** The consequences are handled in Section 3.2 and recorded in Section 12; they do not break any gate, because `--noUnusedLocals` does not see properties of exported objects.

## 3. Scope

### 3.1 In scope

Frontend (`src/`, `package.json`, `docs/`):

1. Delete 7 orphan component files.
2. Delete 8 unused type/function/const declarations plus the one cascade companion (`EMPTY_CELL`).
3. Resolve the 9 `--noUnusedLocals` / `--noUnusedParameters` findings.
4. Remove 2 unused npm dependencies.
5. Correct `docs/reference/architecture.md` so it documents the live tree.

Rust (`src-tauri/`):

6. Delete 10 Class A items plus their mandatory companion edits, including three documentation companions.
7. Demote 2 Class B items to test scope, plus one documentation companion.
8. Remove 2 Windows-gated unused imports and their `#[allow(unused_imports)]`.
9. Remove 6 `#[allow(dead_code)]` that suppress nothing.
10. Remove the redundant `tauri-build` entry from `[dependencies]`.

Plan artifact: `plans/1177-remove-dead-code.md`.

### 3.2 Out of scope, with reasons

**Removed from the issue's list by this plan:**

- **`wait_for_restore_or_session` + `RestoreWaitOutcome`** (`phone/mailbox.rs:1881` to `:1954`, 73 LOC, plus the D.5a tests at `:14935`, `:14960`, `:14967`, `:14979`, `:14986`). The doc block above it (`:1881` to `:1897`) is explicitly labelled `LOAD-BEARING COMMENT` and states: *"The inlined wait loop in `handle_close_session` has no direct unit test on Windows (D.5b is `#[ignore]`'d ...). Equivalence between the two implementations is enforced ONLY by this comment + the helper's D.5a unit tests."* The live production loop at `mailbox.rs:8264` to `:8283` points back at it (`:8263`: *"The helper's logic is unit-tested separately (D.5a)"*). The helper is a retained executable specification, and its tests are the only automated coverage of the inlined loop's semantics on the only platform CI compiles. Deleting the pair keeps the binary identical and silently destroys that coverage. `dev-rust`, whose inventory originally listed it for deletion, accepted the correction and added a supporting argument: the pair is reported dead only in the production lib target, so the real cost of retaining it is exactly the two `#[allow]` attributes already present at `:1898` and `:1918`, each with its reason written. Recommended follow-up: un-`#[ignore]` D.5b, then delete. Grinch found no other item of this class among the ten Class A deletions or the two demotions.

**New dead code that this cleanup creates and deliberately does not remove.** Both were found by `dev-webpage-ui` and re-verified here. Both are registered in Section 12 and go to a follow-up issue.

- **`AgentCreatorAPI.createFolder`** (`src/shared/ipc.ts:1082`) loses its only consumer when F1 deletes `NewAgentModal.tsx`. The Tauri command `create_agent_folder` (`lib.rs:2633`, `commands/agent_creator.rs:25`) is then registered with no frontend consumer. Removing it would be an IPC change, which Section 3.3 declares a non-goal, and it is the same class as `ac_discovery` (#1178) and `phone` (#1179). It belongs with them, not here.
- **A five-member cluster in `src/sidebar/stores/sessions.ts`**: `groupedSessions` (`:339`, which carries `groupedSessionsMemo`), `setTeamFilter` (`:534`), `toggleTeamCollapsed` (`:658`), the `collapsedTeams` getter (`:342`, which reads the module-private signal at `:226`), and the `teamFilter` getter (`:321`). `SessionList.tsx` and `TeamFilter.tsx` are their last consumers. This is excluded for two reasons. First, `groupedSessionsMemo` is grouping logic, not a line, so removing it exceeds "mechanical cleanup". Second, and decisively, **a partial deletion breaks CI**: `collapsedTeams` (`:342`, reader) and `toggleTeamCollapsed` (`:658`, writer via `setCollapsedTeams`) are the two halves of one `createSignal` destructuring at `:226`; deleting one and not the other leaves an unused half and a new TS6133. It is `EMPTY_CELL` at cluster scale, and it deserves its own issue where the whole cluster is taken at once.

**Already out of scope per the issue, confirmed here:**

- The 4 findings dropped by revalidation: `encode_lower_hex_bytes` (`seed_manifest.rs:870`, called at `:629` under `#[cfg(unix)]`), `remove_if_same_identity` (`:2791`, called at `:2812` under `#[cfg(not(windows))]`), `AtomicReplace` (`:171`, constructed at `:2892` and `:6851` under non-Windows gates), and the `file` field of `PinnedDirectory` (`:1570`, a `Drop`-load-bearing directory handle).
- `role_path` (`commands/role_templates.rs:86`) and `ReadOnlyCanonical` (`seed_manifest.rs:162`): Class C, drag live production code.
- The module-wide `#![allow(dead_code)]` at `seed_manifest.rs:19` and the 12 findings underneath it. Separate follow-up.
- `ac_discovery` (#1178), `phone` (#1179), the missing `pty_resized` / `telegram_incoming` listeners (#1180), and `sync_workgroup_repos`.
- `AcDiscoveryPanel.tsx` (419 LOC) and `AcDiscoveryAPI`: belong to #1178.
- **`phone/consumption.rs:27` (the `Observed` and `Pending` variants) and `:54` (`consumption_verdict`).** Reported by `dev-rust` during enrichment and registered here so the sweep is on record as complete. It is #1001 PR1 code: a pure decision core with an `#[ignore]` harness that has not chosen its signal yet, and the source comment says *"Remove when A wires it."* Same class as `wait_for_restore_or_session`. Its absence from the issue is an omission, not a decision. Not touched.
- Selector-level dead CSS: never measured. `sidebar.css` section headers that name deleted components (`:906`, `:5381`, `:5598`) and `variables.css:27` are comments, not code references. `sidebar.css:731` and `:3799` name `OpenAgentModal.tsx`, which is **not** deleted. Not touched.
- `scripts/kill-dev.sh` and `scripts/all_agentscommander_standalone_come_to_me.ps1` (D5/D6): operational scripts, owner decision.
- The superfluous `export` modifiers on internally-used frontend symbols: cosmetic, and removing them would break the tests that import them directly.
- `unreachable_pub` (19 cases) and the `[lints.rust]` workspace proposal: a real improvement, but a change to lint behaviour, not dead-code removal.

### 3.3 Explicit non-goals

No renames, no signature changes beyond F3.5, no module restructuring, no new crates, no new npm packages, no IPC change of any kind (no command added or removed, no event, no wire field, no `serde` rename, no `src/shared/types.ts` field that crosses the IPC boundary), no migration, no CSS change, no CI change.

## 4. Decided solution

### 4.1 Deletion taxonomy applied

Every item in scope is assigned exactly one treatment. No item is left to implementer judgement.

| Treatment | Definition | Verification |
| --- | --- | --- |
| **A. Atomic delete** | exactly one occurrence in the code roots at `fae6b09` | Rule 1 and Rule 6 (Section 6.2) |
| **A+. Atomic delete with companion** | as A, plus a named import, helper or doc comment that the deletion falsifies, edited in the same commit | as A, plus `cargo clippy --all-targets -- -D warnings` clean |
| **B. Demote to test scope** | dead in production, but its `#[cfg(test)]` callers cover live behaviour through it; move the item into the file's `#[cfg(test)] mod tests`, add whatever `use` the child module does not inherit, and drop its `#[allow(dead_code)]` | `cargo test --lib --bins --tests -- --list` diff shows no test lost |
| **D. Delete outright (frontend)** | orphan file or unreferenced declaration | `tsc` clean on both invocations, `npm test` green |
| **R. Rewrite in place** | the finding is not a deletion: the line stays and changes shape | per-item, Section 5 |

### 4.2 Why "demote to test scope" and not "delete item plus its tests"

For `read_task_fields_at` and `write_team_config`, the inline tests are not tests *of the dead item*. They use it as a reader or writer to assert live behaviour:

- `commands/task.rs`: of the 8 call sites, 4 (`:355`, `:364`, `:374`, `:386`) test the helper itself, and 4 (`:398`, `:414`, `:529`, `:629`) use it to read back the result of `cli::task_ops::perform(...)` and `validate_wg_root(...)`, which are live production functions. Deleting the helper and all 8 tests removes coverage of `task_ops::perform`'s `SetTitle` and `Clean` paths.
- `commands/entity_creation.rs`: both call sites use it as a writer. `:4791` asserts that a team config never persists absolute project paths (the `normalize_team_config_for_project` contract). `:5466` asserts that a canonical write sorts `context_alert_percentages`. Both cover live production behaviour.

`dev-rust`, whose inventory classified these as delete-with-tests, verified the call sites and accepted the correction without reservation.

Moving the item inside `#[cfg(test)] mod tests` achieves the actual goal (the symbol leaves the production binary, and the `#[allow(dead_code)]` goes with it) at zero coverage cost and with a much smaller diff than rewriting eight assertions. Both target modules already carry the surrounding scope: `task.rs:344` `mod tests` and `entity_creation.rs:4471` `mod tests` with `use super::*;` at `:4476`.

**A child module does not inherit the parent's `use` statements.** This is the mechanical trap in the move and is specified per item in Section 5.2.

### 4.3 Why the frontend doc edit is a rewrite, not a deletion

`docs/reference/architecture.md` section 3.1 renders `SA --> SL["SessionList.tsx"]` (`:176`) and `SA --> TL["Toolbar.tsx"]` (`:177`), and then hangs three live components off them: `SL --> SI["SessionItem.tsx"]` (`:179`), `TL --> SM["SettingsModal.tsx"]` (`:181`), `TL --> OA["OpenAgentModal.tsx"]` (`:182`). Deleting only lines 176 and 177 orphans three live nodes.

The live tree, verified by import graph and independently re-verified by `dev-webpage-ui`:

```
src/sidebar/App.tsx:67       -> ProjectPanel.tsx
src/sidebar/App.tsx:65,:858  -> ActionBar.tsx
ProjectPanel.tsx:54          -> SessionItem.tsx
SessionItem.tsx:13           -> OpenAgentModal.tsx
ActionBar.tsx:13             -> SettingsModal.tsx
```

`ActionBar.tsx` does **not** import `OpenAgentModal`; the only two importers in `src/` are `SessionItem.tsx:13` and `Toolbar.tsx:3` (deleted). So re-parenting `OA` under `SI` is required, and the old `Toolbar` label ("Open Agent + New Session + Settings") was wrong on a second count as well: what `ActionBar` does besides settings is project creation (`pickAndCheck():104`, `createAndLoad():107,:140`), not "Open Agent". Exact replacement text in Section 5.5.

### 4.4 Decision: `deliver_wake_via_api` is DELETED

`dev-rust` asked the architect to confirm whether this is reserved as a planned fallback. **It is not. It is deleted, together with the two symbols it is the sole user of, its import, and three documentation companions.** All three enrichers agree with the decision; Grinch confirmed independently that there is no caller, config, feature flag or fallback path.

Evidence read at `fae6b09`:

1. **Zero references.** Rule 1 over the code roots returns exactly one line: its own definition at `api/actuation.rs:186`.
2. **The path it implements has a live successor that does the same thing.** `deliver_wake_via_api` resolves the target (`resolve_api_send_target`, `:193`), builds an inline message (`:194`), and actuates through `MailboxPoller::new().deliver_wake_with_origin(app, &msg, WakeDeliveryOrigin::DbQueue)` (`:198` to `:204`). The dispatcher performs the identical actuation, with the identical origin tag, at `api/dispatcher.rs:184` to `:191`. Confirmed by `dev-rust`.
3. **The handler already migrated.** `api/handlers/send.rs:1` to `:3` states: *"The handler queues inline content durably and the dispatcher performs delivery."* The handler calls `actuation::resolve_api_send_target` (`send.rs:60`) and then enqueues; it does not call the inline path.
4. **Nothing preserves it as a fallback.** There is no feature flag, no `cfg`, no comment reserving it, and no configuration branch that could route back to it. `api/actuation.rs` contains zero `#[cfg]` platform gates, so there is no non-Windows caller either.

Design reasoning for the decision itself, not just the mechanics: a synchronous inline delivery path is not a viable fallback for the queue path, so keeping it costs correctness rather than buying safety. The queue exists because delivery must survive a process restart and must be retried under `max_attempts` with leasing and a purge guard (`dispatcher.rs:167` to `:198`). `deliver_wake_via_api` has no durability, no retry, no lease, and no purge interaction; a caller that fell back to it under load would silently drop the very guarantees the migration was made to obtain. If a future increment ever needs an inline send, it should be written against the contracts that exist then, not resurrected from a pre-queue snapshot. Keeping 32 unreachable lines to hedge against that is the worse trade: it is dead weight that reads like a supported path.

The deletion is A+ and takes five companions in the same commit, three of them code and two of them documentation. They are enumerated in Section 5.1 (R1.10c1 to R1.10c5). Two deserve their reasoning here:

- **The `actuation.rs` module doc (`:1` to `:11`)** presents this file as "the non-forking actuation seam" and explains that the bridge calls `deliver_wake` on a throwaway `MailboxPoller::new()`. After the deletion there is no `MailboxPoller::new()` left in the file, and the module no longer actuates anything; it resolves and routes. Leaving the doc is the exact defect that took `wait_for_restore_or_session` out of scope, applied in the other direction: a comment describing code that is not there. `dev-rust` raised this and it is accepted.
- **The `mailbox.rs` backstop comment (`:6205` to `:6214`)** enumerates "The two callers for which this Err is safe" and names the inline API send as one of them, mapped to `DeliveryOutcome::Rejected`. After the deletion only one safe caller remains, and the DB dispatcher, which the same comment already flags as the one that must skip its tick before leasing (`#885 F-5`), becomes the only other caller. Leaving the comment would have a safety-critical backstop claim two safe callers when there is one. Grinch raised this as blocker B3 and it is accepted; the exact replacement text is in Section 5.1, and Section 9.5 criterion 10 is narrowed accordingly so the two requirements no longer contradict each other.

### 4.5 Decision: `AgentPickerModal.tsx:371` is a rewrite, and its position is load-bearing

`const scope = selectedScope();` sits inside a `createEffect` (`:370`) whose next lines are `restartSessions();` (`:374`) and `targetReplicaPath();` (`:375`), bare calls that exist purely to register reactive dependencies. In SolidJS, reading a signal inside an effect is what subscribes the effect to it. Deleting the whole line would silently unsubscribe the effect from `selectedScope`, so the effect would stop re-running on a scope change. That is a behaviour change, and this issue promises none.

Required edit: replace `const scope = selectedScope();` with `selectedScope();`, **in place at `:371`**.

`dev-webpage-ui` added the precision that makes this safe: the line must stay **before the early return at `:386`** (`if (!agent || !isWgReplica()) return;`). A bare call with no assignment reads as surplus and invites relocation; moved below the return, the effect would stop subscribing to `selectedScope` whenever no agent is selected. That is the same bug the rewrite exists to prevent, in an intermittent form. Do not move the line.

Keep `:372` and `:373` (`agent`, `profile`) exactly as they are; `tsc` flags only `scope`, so those two bindings are read later in the effect body. `dev-webpage-ui` classified all 9 findings through this lens and confirmed F3.1 is the only one inside a reactive scope.

### 4.6 Decision: `WorkgroupGroupsModal.nonstop.test.tsx:20` is a signature edit

`fake` is an unused *parameter* of `function mountModal(fake: FakeTransport)`, not a local. The minimal correct fix removes the parameter and updates both call sites (`:66`, `:96`) to `mountModal()`. Underscore-prefixing (`_fake`) is rejected: it silences the compiler while leaving a parameter that lies about the function's inputs. `fake` remains live in both tests (`:59` to `:63`, `:71`; `:86` to `:93`), and `FakeTransport` remains imported for `new FakeTransport()`, so no new unused binding appears.

### 4.7 Decision: the Linux cross-compilation probe is time-boxed and non-blocking

`dev-rust` proposed attempting `rustup target add x86_64-unknown-linux-gnu` plus `cargo check --target x86_64-unknown-linux-gnu` to close the one risk PR CI cannot cover, with the reservation that `rusqlite` is vendored with `bundled` and its build script very likely needs a C cross-compiler. **Decision: attempt it once, time-boxed to 15 minutes, and record the outcome either way. It is not a gate and it must not block the batches.**

Reasoning, and this is why it is optional rather than required:

1. The severe failure mode is a **compilation** break under a platform gate. Rule 1 closes that by construction for all ten Class A items, and Rule 2 closes it by enumeration for the two demotions. A Linux `cargo check` would confirm what the occurrence count already proves.
2. For eight of the ten Class A items rustc cannot report dead code at all (Section 2.3), so a Linux build would add no dead-code signal for them either.
3. The residual risk it would actually close is a new `dead_code` **warning** on Linux from the `allow` removals. Per Section 2.2, `release.yml` has no `-D warnings`, so that is hygiene, not an outage. Section 6.2 Rule 4 already traces every covered item to ungated production use, and `dev-rust` confirmed with the compiler that `config_seed.rs` emits zero warnings under `--force-warn dead_code`.

So: run it, because a cheap confirmation of a proof is still worth 15 minutes. If the `rusqlite` build script fails, write that down in the Section 9.3 record and proceed. Do not install a C cross-toolchain for this.

## 5. Exact affected surfaces

Line numbers are as read at `fae6b09` and are unchanged at `092d85c`. **Within a single file, apply edits bottom-up (highest line first)** so earlier edits do not shift later coordinates. Anchor on the symbol, and where the symbol is ambiguous, on the full line text (Section 10). If an anchor does not match, stop and report rather than deleting by line number.

### 5.1 Rust, Class A and A+ (batch 3)

| # | Symbol | File | Range to delete | Treatment |
| --- | --- | --- | --- | --- |
| R1.1 | `workspace_dir_label` | `src-tauri/src/config/workspace.rs` | `:10` to `:12` plus the following blank line | A |
| R1.2 | `read_instance_profile_override` | `src-tauri/src/config/coding_agent_profiles.rs` | `:377` to `:379` plus blank | A |
| R1.3 | `read_origin_default_profile` | `src-tauri/src/config/coding_agent_profiles.rs` | `:427` to `:433` plus blank | A |
| R1.4 | `validate_codex_home_value` | `src-tauri/src/config/settings.rs` | `:1383` to `:1385` plus blank | A |
| R1.5 | `product_name` | `src-tauri/src/config/profile.rs` | `:156` to `:169` (doc `:156` to `:158` included) plus blank | A |
| R1.6 | `is_stage` | `src-tauri/src/config/profile.rs` | `:214` to `:217` (doc `:214` included) plus blank | A |
| R1.7 | `_manifest_path_for_docs` | `src-tauri/src/cli/agency_templates.rs` | `:1004` to `:1007` (the `#[allow(dead_code)]` at `:1004` included) plus blank | **A+** |
| R1.7c | companion: drop `agency_manifest_path,` from the `use crate::commands::role_templates::{ ... }` list | `src-tauri/src/cli/agency_templates.rs` | `:11` | A+ |
| R1.8 | `due_pty_input_ids_offloaded` | `src-tauri/src/api/message_store.rs` | `:2759` to `:2769` plus blank | A, see the anchoring warning in Section 10 |
| R1.9 | `build_attempt_injection` | `src-tauri/src/cli/role_experiment.rs` | `:2142` to `:2148` (the `#[allow(dead_code)]` at `:2142` included) plus blank | A |
| R1.10 | `deliver_wake_via_api` | `src-tauri/src/api/actuation.rs` | `:181` to `:212` (doc `:181` to `:185` included) plus blank | **A+** |
| R1.10c1 | companion: `build_inline_wake_message` | `src-tauri/src/api/actuation.rs` | `:150` to `:179` plus blank | A+ |
| R1.10c2 | companion: `DeliveryOutcome` | `src-tauri/src/api/actuation.rs` | `:23` to `:30` (doc `:23` and `#[derive]` `:24` included) plus blank | A+ |
| R1.10c3 | companion: `use crate::phone::types::OutboxMessage;` | `src-tauri/src/api/actuation.rs` | `:19` | A+ |
| R1.10c4 | companion: rewrite the module doc | `src-tauri/src/api/actuation.rs` | `:1` to `:11`, exact text below | A+ |
| R1.10c5 | companion: rewrite the backstop comment | `src-tauri/src/phone/mailbox.rs` | `:6211` to `:6214`, exact text below | A+ |

**R1.10c4, exact replacement for `api/actuation.rs:1` to `:11`:**

```rust
//! The resolution and routing seam for API sends (#791 §8, §0.5 HIGH-3).
//!
//! Both the filesystem poller and the API resolve and route through the SAME
//! `can_communicate` / `resolve_agent_target`, so the two planes cannot diverge
//! in canonicalization for the non-root verb set. Actuation is NOT performed
//! here: `api/handlers/send.rs` queues the send durably and `api/dispatcher.rs`
//! delivers it through `MailboxPoller::deliver_wake_with_origin` with
//! `WakeDeliveryOrigin::DbQueue` (#1177 removed the inline delivery path that
//! used to live in this file). Root-Agent routing is intentionally API-excluded
//! in increment 1 (`can_communicate` does not model the root branches) and
//! rejected at the boundary.
```

**R1.10c5, exact replacement for `phone/mailbox.rs:6211` to `:6214`** (keep `:6205` to `:6210` byte-unchanged):

```rust
        //   POISON the message. The one caller for which this Err is safe:
        //   - filesystem poller: non-permanent error, retried at the 3s poll
        //     interval up to MAX_DELIVERY_ATTEMPTS. Deferred, not lost.
        //   The inline API send that used to map this to a rejected outcome no
        //   longer exists (#1177), so the DB dispatcher is now the only other
        //   caller, and it is exactly the one F-5 must keep out of this window.
```

This preserves the load-bearing warning (this is a BACKSTOP, the dispatcher must skip its tick before leasing, reaching this Err from there burns an attempt and can poison the message) and sharpens it, because the reader is no longer told there is a second safe caller.

After batch 3, `src-tauri/src/api/actuation.rs` shrinks from 322 lines to roughly 250, and `cargo clippy --all-targets -- -D warnings` must still be clean.

### 5.2 Rust, Class B demotions (batch 4)

**R2.1 `read_task_fields_at`, `src-tauri/src/commands/task.rs`**

1. Cut `:174` to `:191` (doc comment `:174` to `:177`, `#[allow(dead_code)]` `:178`, function `:179` to `:191`).
2. Paste the function plus its doc comment inside `mod tests` (opens at `:343` to `:344`), **after the `//!` module doc and after the `use` lines**. `//!` is only valid before any item in the module; pasting the function above it is a compile error.
3. **Drop the `#[allow(dead_code)]`**: the item is used by the tests, so the lint is satisfied without it.
4. **Add `use std::path::Path;` inside `mod tests`**, or change the moved signature to `&std::path::Path`. The function takes `wg_root: &Path` and `Path` is imported at `task.rs:9`, in the **parent** module; a child module does not inherit it, and the move fails with `E0425 cannot find type Path in this scope`. Grinch reproduced this form with `rustc`. Prefer adding the `use`, so the moved body stays byte-identical.
5. Edit the test module's import at `:349` from `use super::{read_task_fields_at, validate_wg_root};` to `use super::validate_wg_root;`.
6. The function body calls `crate::commands::entity_creation::parse_task_title` by full path (`:183`), so it resolves unchanged from inside `mod tests`.
7. Companion: rewrite the `mod tests` doc at `:345` to `:348`. It currently says the module *"Covers the helper that issue #301 turns on"*, describing `read_task_fields_at` as intended production code. Issue #301 is CLOSED and the helper is now test-only. Exact replacement:

```rust
    //! Covers `cli::task_ops::perform` and `validate_wg_root` through the
    //! local `read_task_fields_at` reader: a single read of TASK.md must
    //! return BOTH the trimmed body and the parsed YAML `title:`. The reader
    //! is test-only (#1177); #301 shipped without a production caller, and the
    //! assertions it backs still cover the live save and clean paths.
```

8. No test is deleted or modified. All 8 call sites keep working.

**R2.2 `write_team_config`, `src-tauri/src/commands/entity_creation.rs`**

1. Cut `:895` to `:904` (comment `:895`, `#[allow(dead_code)]` `:896`, function `:897` to `:904`).
2. Paste the function inside `mod tests` (opens at `:4470` to `:4471`, closes at `:7632`), **after the `//!` module doc at `:4472` to `:4474` and after `use super::*;` at `:4476`**. Same `//!` rule as R2.1.
3. **Drop the `#[allow(dead_code)]`.** Replace the `:895` comment with one that states the new truth, for example: `// Test-only synchronous wrapper: acquires the mutation guard around write_team_config_guarded.`
4. No extra `use` is needed: `use super::*;` at `:4476` brings `TeamConfigMutationGuard` and `write_team_config_guarded` into scope, and `dev-rust` confirmed there is no homonym that could shadow them through the glob (`TeamConfigMutationGuard` only at `:592`, `write_team_config_guarded` only at `:906`). `Path` and `PathBuf` also arrive through the glob. This is the difference from R2.1, whose test module does not use a glob import.
5. No test is deleted or modified. Both call sites (`:4791`, `:5466`) are inside the target module and keep working.
6. **Guard interaction, verified twice.** The source-scraping guard in `src-tauri/src/config/local_config_io.rs` allowlists `entity_creation.rs` lines containing the substring `write_team_config` (`is_allowed_line`, `:611` to `:618`) and blanks `#[cfg(test)] mod tests` blocks before scanning (`strip_test_modules`, `:620`). The allowlist entry stays needed by `write_team_config_guarded` and `write_team_config_guarded_with_publisher`, which remain in production, and the moved copy lands inside a blanked block, because `:4470` is `#[cfg(test)]` and `:4471` is `mod tests`, exactly the pattern the stripper matches. `dev-rust` added one hazard: `brace_delta` counts braces per character without excluding strings or comments, so do **not** introduce an unbalanced brace inside a comment in the moved block. The guard test runs inside `cargo test --lib` and must pass.

### 5.3 Rust, imports, `allow`s and manifest (batches 5a, 5b, 6)

| # | Item | File | Coordinates | Batch | Note |
| --- | --- | --- | --- | --- | --- |
| R3.1 | `#[allow(unused_imports)]` + `use std::os::windows::process::CommandExt;` | `src-tauri/src/commands/entity_creation.rs` | `:4389` to `:4390` | 5a | inside `#[cfg(windows)] { ... }` (`:4387`); `creation_flags` at `:4391` comes from `tokio::process::Command`, not this trait |
| R3.2 | same pair | `src-tauri/src/commands/entity_creation.rs` | `:4427` to `:4428` | 5a | inside `#[cfg(windows)] { ... }` (`:4425`); `creation_flags` at `:4429` |
| R3.8 | `#[allow(dead_code)]` on `TeamConfigReadError::class` | `src-tauri/src/commands/entity_creation.rs` | `:831` | 5a | `allow` on a **method**, so no field lint is in play; `class()` is called in production at `src/session/context_alerts.rs:1579` |
| R3.3 | `#[allow(dead_code)]` on `ConfigSeedPublication` | `src-tauri/src/config/config_seed.rs` | `:76` | 5b | |
| R3.4 | `#[allow(dead_code)]` on `CollectedSeedFiles` | `src-tauri/src/config/config_seed.rs` | `:85` | 5b | |
| R3.5 | `#[allow(dead_code)]` on `ConfigSeedSkipReason` | `src-tauri/src/config/config_seed.rs` | `:95` | 5b | |
| R3.6 | `#[allow(dead_code)]` on `ConfigSeedFailure` | `src-tauri/src/config/config_seed.rs` | `:119` | 5b | |
| R3.7 | `#[allow(dead_code)]` on `ConfigSeedRollbackFailure` | `src-tauri/src/config/config_seed.rs` | `:134` | 5b | |
| R3.9 | `tauri-build = { version = "2", features = [] }` | `src-tauri/Cargo.toml` | `:10`, in `[dependencies]` | 6 | the `[build-dependencies]` copy at `:77` is what resolves `build.rs`; keep it |

Only the `#[allow(...)]` attribute lines are removed in R3.3 to R3.8. The types, the enum and the method they sit on are live and stay byte-identical.

5a and 5b are separated because they are different risk classes: R3.1 and R3.2 are Windows-gated and therefore fully verified by the Windows CI job, and R3.8 sits on a method where the `dead_code` lint has only one mode. R3.3 to R3.7 sit on types, where `#[allow(dead_code)]` suppresses both "never constructed" and "field is never read". Both classes are safe (Section 6.2, Rule 4), but if something goes wrong the bisect is immediate.

### 5.4 Frontend deletions (batches 1 and 2)

**F1: 7 orphan files, deleted whole.**

| File | Physical lines | Non-empty lines |
| --- | --- | --- |
| `src/sidebar/components/NewAgentModal.tsx` | 186 | 165 |
| `src/sidebar/components/Toolbar.tsx` | 92 | 84 |
| `src/sidebar/components/SessionList.tsx` | 86 | 83 |
| `src/guide/components/CatalystTab.tsx` | 49 | 43 |
| `src/sidebar/components/TeamFilter.tsx` | 35 | 32 |
| `src/sidebar/components/CollapsibleSection.tsx` | 30 | 26 |
| `src/sidebar/components/TeamGroupHeader.tsx` | 29 | 26 |
| **total** | **507** | **459** |

The two closed clusters must go together: `Toolbar.tsx` imports `NewAgentModal.tsx` (`:4`), and `SessionList.tsx` imports `TeamGroupHeader.tsx` (`:4`). Nothing outside the set imports any of the 7; the only importers in `src/` are those two internal edges, and there are no test importers.

**The risk is asymmetric, so if the batch is ever split, split it in the right direction.** Deleting the imported file while keeping the importer does not compile. Deleting the importer first compiles and only leaves an orphan. Therefore: the importer goes first, or both go together, never the importer last.

**F2: symbol deletions.**

| # | Symbol | File | Range |
| --- | --- | --- | --- |
| F2.1 | `SessionGroup` | `src/shared/types.ts` | `:320` to `:326` plus blank |
| F2.2 | `ShellProfile` | `src/shared/types.ts` | `:328` to `:336` plus blank |
| F2.3 | `AppConfig`, `GeneralConfig`, `SidebarConfig`, `TerminalConfig` | `src/shared/types.ts` | `:338` to `:368` plus blank (one rootless subgraph: the three children are referenced only from `AppConfig` at `:339` to `:341`) |
| F2.4 | `PhoneConversation` | `src/shared/types.ts` | `:1041` to `:1046` plus blank |
| F2.5 | `getErrorsOnly`, `copyConsoleLogs`, `copyErrors` | `src/shared/console-capture.ts` | `:107` to `:125` (closed cluster: `copyErrors` at `:122` is the only user of `getErrorsOnly`) |
| F2.6 | `profileCellOrDefault` | `src/shared/profile-utils.ts` | `:99` to `:105` plus blank |
| F2.6c | **companion:** `EMPTY_CELL` | `src/shared/profile-utils.ts` | `:26` to `:31` plus blank |
| F2.7 | `AC_WORKSPACE_DIRS` | `src/shared/constants.ts` | `:4` |
| F2.8 | `SESSION_C` | `src/shared/testing/session-selection.ts` | `:7` |

Do not touch `TeamSessionGroup` (`types.ts:986`): it is a different, live type that a substring search for `SessionGroup` also matches. Do not touch `AC_WORKSPACE_DIR` (`constants.ts:3`), `PhoneMessage` (`types.ts:1031`), `getConsoleText`, `getConsoleLogs`, `cellForLetter` (`profile-utils.ts:89`, live via `:174`), or `ProfileCellConfig`.

**F3: the 9 unused declarations.** Reproduced verbatim from `npx tsc --noEmit --noUnusedLocals --noUnusedParameters` at `fae6b09`. Of the nine: 5 are imports, 2 are locals, 1 is a function declaration, and 1 is a function **parameter**. Two are not removals.

| # | Finding | Treatment |
| --- | --- | --- |
| F3.1 | `AgentPickerModal.tsx(371,11)` `'scope'` | **rewrite in place** to `selectedScope();`, and keep it above the early return at `:386`. Section 4.5 |
| F3.2 | `ProjectPanel.collapse-state.test.tsx(16,1)` `'projectCollapseStore'` | delete the import statement (the only one of the nine where the whole statement goes) |
| F3.3 | `SettingsModal.tsx(668,9)` `'s'` | delete `const s = () => settings.data;`. **Anchor on the full line text, not on `const s`**: Section 10 |
| F3.4 | `WorkgroupGroupRail.favorites.test.tsx(56,10)` `'otherProject'` | delete the whole function `:56` to `:66`; `otherProjectPath`, `wg(...)` and `ProjectState` stay live |
| F3.5 | `WorkgroupGroupsModal.nonstop.test.tsx(20,21)` `'fake'` | **signature edit**, Section 4.6: drop the parameter at `:20`, update `:66` and `:96` to `mountModal()` |
| F3.6 | `spec-board/App.tsx(2,21)` `'createEffect'` | remove from the import list |
| F3.7 | `AskAgentPanel.tsx(2,35)` `'onMount'` | remove from the import list |
| F3.8 | `AskAgentPanel.tsx(2,44)` `'onCleanup'` | remove from the import list (same line as F3.7) |
| F3.9 | `MermaidPreview.tsx(2,44)` `'onCleanup'` | remove from the import list |

None of the four import lists in F3.6 to F3.9 becomes empty, so no import statement is deleted there.

`dev-webpage-ui` checked the inverse lens on F3.6 to F3.9, since an unused `onCleanup` import can be a cleanup lost in a refactor rather than dead code. A sweep of `addEventListener|setInterval|setTimeout|requestAnimationFrame|listen(|subscribe|ResizeObserver|MutationObserver` over the three files returns zero; `MermaidPreview`'s five handlers are inline JSX that SolidJS frees with the node, and stale renders are already discarded by its `renderId` guard. **No cleanup is missing.** The four imports are genuine surplus.

**F4: dependencies.** `package.json`: remove `"@tauri-apps/plugin-shell": "^2"` (`:32`, `dependencies`) and `"@types/dompurify": "^3.0.5"` (`:43`, `devDependencies`). Commit the resulting `package-lock.json` change. Leave `"kill-dev"` (`:12`) and every other script untouched.

### 5.5 Documentation (batch 7)

`docs/reference/architecture.md`, two edits. `dev-webpage-ui` verified this section applies as written, with no correction.

Mermaid graph, section 3.1:

| Line | From | To |
| --- | --- | --- |
| `:176` | `    SA --> SL["SessionList.tsx<br/>For each session → SessionItem"]` | `    SA --> PP["ProjectPanel.tsx<br/>Projects, workgroups, replicas → SessionItem"]` |
| `:177` | `    SA --> TL["Toolbar.tsx<br/>Open Agent + New Session + Settings"]` | `    SA --> AB["ActionBar.tsx<br/>Project creation + Settings gear"]` |
| `:179` | `    SL --> SI["SessionItem.tsx<br/>...` | `    PP --> SI["SessionItem.tsx<br/>...` (label unchanged) |
| `:181` | `    TL --> SM["SettingsModal.tsx<br/>...` | `    AB --> SM["SettingsModal.tsx<br/>...` (label unchanged) |
| `:182` | `    TL --> OA["OpenAgentModal.tsx<br/>...` | `    SI --> OA["OpenAgentModal.tsx<br/>...` (label unchanged) |

The `style` lines at `:194` to `:197` reference `SA`, `SM`, `OA`, `SI` only, so they need no change. `rg '\bSL\b|\bTL\b'` over the file returns nothing outside these five lines, so no `SL` or `TL` identifier may remain after the edit. After the rewrite every node has an incoming edge rooted at `SA`: `TB<-SA`, `PP<-SA`, `AB<-SA`, `SI<-PP`, `SM<-AB`, `OA<-SI`, `SS<-SA`, `BS<-SA`.

File table, section 8:

| Line | From | To |
| --- | --- | --- |
| `:719` | `| `sidebar/components/SessionList.tsx` | `<For>` over sessions → `SessionItem` |` | `| `sidebar/components/ProjectPanel.tsx` | Projects, workgroups and replicas → `SessionItem` |` |
| `:721` | `| `sidebar/components/Toolbar.tsx` | Open Agent + New Session + Settings gear |` | `| `sidebar/components/ActionBar.tsx` | Project creation + Settings gear |` |

`docs/brand.md:56` and `:57` say "Toolbar btn"; that is a CSS design-token name, not a component reference. Do not touch it.

## 6. Required behaviour, failure modes, and the verification protocol

### 6.1 Required behaviour

| Situation | Required behaviour |
| --- | --- |
| Any user-visible flow (sidebar, terminal, watchers, guide, spec-board, PTY, API send, CLI) | byte-identical to baseline; this change removes only code with no consumer |
| IPC surface | unchanged: no Tauri command added or removed, no event, no wire field, no `serde` attribute, no `src/shared/types.ts` type that crosses the boundary |
| `cargo test --lib --bins --tests -- --list` | identical to baseline, no test lost, none added, none newly `#[ignore]`d |
| `cargo clippy --all-targets -- -D warnings` | zero warnings, same as baseline |
| `RUSTFLAGS="--force-warn dead_code" cargo check --lib --bins` | exactly three items disappear (`build_attempt_injection`, `read_task_fields_at`, `write_team_config`); no item is added |
| `tsc --noEmit --noUnusedLocals --noUnusedParameters` | 0 errors after the change (9 before) |
| Reactive behaviour of `AgentPickerModal`'s scope effect | unchanged: the effect still re-runs on a `selectedScope` change, including when no agent is selected (Section 4.5) |
| Reactive behaviour of `SettingsModal`'s section effect (`:640` to `:643`) | unchanged: `:641` is a different, live `const s` and must not be touched |
| `local_config_io` source-scraping guard | still passes; the allowlist substring is still matched by the production `write_team_config_guarded*` functions |
| Non-Windows builds (`release.yml`: `ubuntu-22.04`, `macos-latest`) | still compile |

### 6.2 Verification protocol (six rules)

This is the part CI cannot do for you, and per Section 2.3 it is the *only* check for most items. For each Rust item, apply the applicable rules and record the result.

**Preconditions of Rule 1, verified once by `dev-rust` and recorded here.** Occurrence counting only proves absence of callers if callers must appear literally. In this crate they must: there is no `concat_idents!`, no `paste!`, and no identifier generation of any kind; the crate's only `macro_rules!` is `cli_println!` (`cli/mod.rs:42`), a printing macro that fabricates no function names; `build.rs` performs no codegen (no `include!`, no `OUT_DIR` emission); and none of the ten Class A items is a `#[tauri::command]`, which in any case would be listed literally by `generate_handler!`. If a future change introduces identifier-generating macros, Rule 1 stops being sound.

**Rule 1, no caller (closed by construction).** Run, at the baseline commit so the plan file itself is not scanned:

```
git grep -n --word-regexp -e '<symbol>' fae6b09 -- src-tauri/src src-tauri/tests crates src tests scripts .github package.json src-tauri/Cargo.toml
```

If this returns **exactly one occurrence, the definition itself**, then no `#[cfg(unix)]` or `#[cfg(not(windows))]` caller can exist, because such a caller would be a second occurrence. Count occurrences, not lines. This closes the "does the deletion break compilation on an uncompiled platform" question for R1.1 to R1.10 with no inspection needed. Verified at plan time by the architect and re-verified by `dev-rust`: all ten return exactly one.

Do **not** use a bare `rg ... .` over the working tree for this. The plan file names every symbol, so the working-tree form returns 2 for most items, 4 for `_manifest_path_for_docs` and 12 for `deliver_wake_via_api`, and the rule reads as failing when it is passing. This was Grinch's blocker B2.

**Rule 2, enumerated references.** If the symbol has references, every reference must be enumerated and each placed inside or outside a platform gate by reading the file. Applies to:
- `read_task_fields_at`: 8 references, all inside `#[cfg(test)] mod tests` (opens `task.rs:343`). That `#[cfg(test)]` is the only `#[cfg]` in the file. No platform gate involved.
- `write_team_config`: 2 references, both inside `#[cfg(test)] mod tests` (`entity_creation.rs:4470` to `:7632`), plus one string-literal mention in `local_config_io.rs:615` handled in Section 5.2.

**Rule 3, edits inside gated code.** R3.1 and R3.2 sit inside `#[cfg(windows)] { ... }` blocks (`entity_creation.rs:4387`, `:4425`). They compile only on Windows, so `rust-regression` on `windows-latest` is the exact right verifier. No non-Windows exposure.

**Rule 4, `allow` removals need the covered item traced.** Removing an `#[allow(dead_code)]` is only safe if the item it covers is live in code that compiles on **every** platform, and, for an `allow` on a type, if every field is read somewhere. Verified at plan time by source trace and independently by `dev-rust` with the compiler:
- The five `config_seed.rs` types are all reachable through `ConfigSeedReport` (`:147` to `:152`), which is ungated. Their construction and field reads live at `:385`, `:480`, `:508`, `:541`, `:552`, `:580`, `:600`, `:614`, `:645`, `:663`, `:686`, `:842`, `:855`, `:898`, `:903`, `:930`, `:932`, `:947`, `:952`, `:965`, `:980`, all ungated production code. The only platform gates in that file's production half are the two five-line `metadata_is_reparse` definitions (`:759` `#[cfg(windows)]`, `:766` `#[cfg(not(windows))]`), which touch none of these types. The file's `#[cfg(test)] mod tests` opens at `:1275`, so `:1742`, `:2189` and `:2193` are test-side and irrelevant. **`dev-rust` initially concluded by reading that several fields were never read (for example `CollectedSeedFiles::OverBound::observed_at_least`, since the only production destructuring at `:898` uses `{ reason, .. }`) and that removing these `allow`s would break the gate. The compiler refuted that: `config_seed.rs` emits zero warnings under `--force-warn dead_code` in both targets. The derives plus the `log::debug!("{:?}", ...)` at `:910` are enough for rustc to count those fields as read.** The five `allow`s are genuinely redundant.
- `TeamConfigReadError::class` is called at `src/session/context_alerts.rs:1579`, ungated production (that file's only `#[cfg(test)]` opens at `:1720`). The `allow` is on a method, so no field lint applies.

**Rule 5, companion imports.** For A+ deletions, confirm the companion import has no other user in the file **including inside platform-gated blocks**, before removing it. Verified: `agency_manifest_path` appears in `cli/agency_templates.rs` only at `:11` and `:1006`; `OutboxMessage` appears in `api/actuation.rs` only at `:19`, `:150` and `:155`, and that file has zero `#[cfg]` gates.

**Rule 6, cascade (what the deleted body strands).** Rules 1 to 5 answer "does anything call this". They say nothing about "what does this call that nothing else calls", which is where every cascade in Section 2.5 came from. For each deleted item, enumerate every symbol its **body** references and confirm each retains at least one live user after the deletion. `dev-rust` executed this for all ten Class A bodies and found no eleventh cascade; `dev-webpage-ui` executed the equivalent sweep over module-private declarations for F1 and F2 and found only `EMPTY_CELL`. Re-run it for anything the implementer deletes that is not in Section 5, and expand it to **member** granularity, not just symbol granularity: the `AgentCreatorAPI.createFolder` finding in Section 2.5 is exactly what symbol-level checking misses.

### 6.3 Failure modes and what each one means

| Symptom | Almost certainly means | Severity | Action |
| --- | --- | --- | --- |
| `unused import` error under `-D warnings` | a companion edit from Section 5 was skipped | PR blocked | apply the companion, do not add an `#[allow]` |
| `E0425 cannot find type Path` in `task.rs` tests | R2.1 step 4 was skipped | PR blocked | add `use std::path::Path;` inside `mod tests` |
| `//!` doc comment error in a moved block | the function was pasted above the `//!` | PR blocked | `//!` must precede every item in the module |
| `cannot find function ...` in a test | a Class B item was deleted instead of demoted | PR blocked | restore and follow Section 5.2 |
| a Linux or macOS **build fails to compile** | a deletion was live behind a platform gate | release blocked | revert that single item and report; Rule 1 exists to make this impossible, and the batch structure keeps it to one commit |
| a Linux or macOS build emits a **new `dead_code` warning** | an `allow` removal exposed a platform-gated item (Rule 4 violated) | hygiene only | `release.yml` builds through `tauri-action` with no `-D warnings`, and the repo has no `[lints.rust]`, no `.cargo/config.toml` and no `RUSTFLAGS`, so this does not break the release. Report it and fix it; do not re-add the `allow` silently, and do not panic |
| a new item appears in the `--force-warn dead_code` list | a cascade escaped Rule 6 | PR blocked | delete the newly-orphaned item in the same commit |
| `tsc --noUnusedLocals` reports a NEW error after a deletion | a frontend cascade like `EMPTY_CELL` was missed | PR blocked | delete the newly-orphaned declaration in the same commit |
| a SolidJS effect stops firing | F3.1 was deleted instead of rewritten, or was moved below the early return at `:386` | behaviour regression | apply Section 4.5 exactly |
| `SettingsModal` stops switching tabs on `props.section` | `:641` was edited instead of `:668` | behaviour regression | Section 10, anchor on full line text |
| the `local_config_io` guard test fails | the `write_team_config` move landed outside `#[cfg(test)] mod tests`, or an unbalanced brace was introduced in a comment | PR blocked | re-check Section 5.2 steps 2 and 6 |

## 7. Compatibility, performance, security

- **IPC**: unchanged in both directions. No Tauri command is registered or unregistered, no event name is added or removed, no `#[serde(rename_all)]` struct changes shape, and no `src/shared/types.ts` interface that crosses the boundary is touched. `AppConfig`, `ShellProfile`, `SessionGroup` and `PhoneConversation` are frontend-only types with zero references anywhere in the repo, so no Rust counterpart exists to drift from. The one IPC-adjacent consequence, `create_agent_folder` losing its frontend consumer, is deliberately left in place and registered for a follow-up (Section 3.2).
- **Persistence and migration**: none. No TOML or JSON schema, no on-disk layout, no config key.
- **Binary**: the Class A items are removed from the build. The Class B demotions move code from the production binary into `#[cfg(test)]`, so the release binary shrinks slightly. No behaviour depends on it.
- **`tauri-build` in `[dependencies]`**: removing it does not change what is compiled. `build.rs:28` resolves `tauri_build::build()` through `[build-dependencies]:77`. Nothing under `src-tauri/src` references `tauri_build`.
- **npm dependencies**: `@tauri-apps/plugin-shell` has no JS import and no Rust counterpart (`tauri_plugin_shell` appears nowhere in `src-tauri/` or `Cargo.toml`). `@types/dompurify` is redundant because `dompurify@3.4.2` ships its own types. Both removals are lockfile-affecting and are verified by `npm ci && npm run typecheck && npm run build`.
- **Performance**: no runtime path changes. `EMPTY_CELL` is a module-level constant object that is no longer allocated; immaterial.
- **Security**: no credential handling, no network, no auth, no path validation, and no trust boundary is touched. Two security-adjacent invariants are explicitly preserved and re-verified: the `local_config_io` scraping guard (Section 5.2 step 6) and the `mailbox.rs` purge backstop comment, whose warning is preserved and sharpened rather than deleted (Section 5.1, R1.10c5).
- **Windows and ConPTY**: no PTY code path, no `cmd.exe /C` wrapping, and no path handling is touched. The two `#[cfg(windows)]` edits (R3.1, R3.2) remove an import only; `creation_flags` continues to come from `tokio::process::Command`.

## 8. Implementation order

Batches are ordered by increasing risk, and **each batch is a separate commit** that must pass its own gate before the next one starts. If a late batch has to be reverted, the earlier ones survive.

### MVP: risk-free, verifiable in isolation

**Batch 1, frontend orphan files (F1).** Delete the 7 files, importer first or together (Section 5.4). Gate: `npm run typecheck`, `npm test`.

**Batch 2, frontend symbols and unused declarations (F2 + F3).** Includes the `EMPTY_CELL` companion, the `AgentPickerModal` rewrite, and the `mountModal` signature edit. Gate: `npm run typecheck`, `npx tsc --noEmit --noUnusedLocals --noUnusedParameters` reporting 0 errors, `npm test`.

**Batch 3, Rust Class A deletions (R1, all 10 plus the five companions).** Every item is closed by Rule 1, so it is the largest batch with the smallest risk. Gate: the three CI Rust commands plus the `--force-warn dead_code` list diff (Section 9.4).

### Full features: the batches that touch tests and lint signal

**Batch 4, Rust Class B demotions (R2.1, R2.2).** Two moves plus one doc companion. Gate: the three Rust commands, plus the `-- --list` diff of Section 9.2, plus the `--force-warn` list diff.

**Batch 5a, Windows-gated imports and the method `allow` (R3.1, R3.2, R3.8).** Fully verified by Windows CI.

**Batch 5b, the five type `allow`s (R3.3 to R3.7).** Separated from 5a because this is the only place a green Windows CI could hide a Linux warning, and because the `allow` on a type suppresses two lints rather than one. Gate: the three Rust commands, plus the `--force-warn` list diff, plus the Section 9.3 record for Rule 4.

### Polish

**Batch 6, npm dependencies (F4) and the `Cargo.toml` line (R3.9).** Both are manifest edits and both depend on network resolution. Kept away from batch 5 so a registry failure cannot contaminate the diagnosis of the most delicate lint batch, and away from batch 7 so it cannot revert zero-risk documentation. Gate: `npm ci && npm run typecheck && npm run build`, plus `cargo check --all-targets`.

**Batch 7, documentation (F5).** Zero risk, no network. Gate: a read of the rendered mermaid block confirming no orphan node, and `rg '\bSL\b|\bTL\b'` returning nothing in the file.

### Extras

None. Nothing beyond Sections 5.1 to 5.5 is authorised.

## 9. Tests and objective acceptance criteria

### 9.1 No new test is written

This change removes unreachable code. A test asserting that deleted code is absent would be a tautology. The verification instruments are the compiler, the linter, the two list diffs, and the existing suites, whose pass/fail is objective.

### 9.2 Test-identity invariant (the load-bearing check for batch 4)

Before batch 4, record the baseline:

```
cargo test --lib --bins --tests -- --list
```

After batch 4, run it again and `diff` the two outputs. They must be **identical**.

This replaces the Step 4 draft's test-*count* invariant, which `dev-rust` correctly identified as too weak: one test deleted plus one added leaves the count unchanged, and a target that stops emitting its `test result` line is invisible to a count comparison. The nominal list detects the swap, the vanished target and a newly `#[ignore]`d test, all with one `diff`. The same argument applies to the frontend: prefer comparing `npm test`'s reported suite and test names over comparing a single number.

One hazard `dev-rust` chased and closed: an implementer could paste the moved function *outside* the `#[cfg(test)]` block, passing the tests with the symbol still in the production binary. That does not slip through, because `--all-targets` compiles the lib without `cfg(test)` as a separate unit, so the stray symbol would emit `dead_code` there and `-D warnings` fails the build.

### 9.3 Platform and cascade inspection record

Before opening the PR, produce a written record, in the PR description or a commit message, stating for each Rust item which rule from Section 6.2 closed it and what was observed. The record is a deliverable, because no PR job can produce it. Minimum content:

- R1.1 to R1.10: Rule 1, with the occurrence count observed at `fae6b09` (must be 1), and Rule 6, naming what the body referenced and why each survivor is still live.
- R2.1, R2.2: Rule 2, with the reference list and the `#[cfg(test)]` range that contains it.
- R3.1, R3.2: Rule 3, naming the enclosing `#[cfg(windows)]` block line.
- R3.3 to R3.8: Rule 4, naming at least one ungated production site that keeps each covered item live.
- R1.7c, R1.10c3: Rule 5, with the in-file occurrence list for the removed import.
- The Linux probe of Section 4.7: whether it ran, and its outcome, including a `rusqlite` build-script failure if that is what happened. A failure here is an acceptable, recorded result, not a blocker.

### 9.4 Verification commands

Frontend, from the repository root:

```
npx tsc --noEmit
npx tsc --noEmit --noUnusedLocals --noUnusedParameters
npm test
npm run test:debt
npm ci && npm run typecheck && npm run build
```

Rust, from `src-tauri`, matching what CI runs:

```
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --lib --bins --tests
```

Rust, wider than CI, matching the stated baseline:

```
cargo clippy --workspace --all-targets --all-features
```

Rust, the two identity diffs (record before, compare after):

```
cargo test --lib --bins --tests -- --list
RUSTFLAGS="--force-warn dead_code" cargo check --lib --bins --message-format=short
```

For the second command, capture the **list of reported items**, not the count. Cargo does not replay diagnostics for a unit it considers fresh, so force a rebuild (a clean `--target-dir`, or `cargo clean -p` for the crate) when taking each measurement, and take both measurements the same way. Comparing lists rather than a hardcoded number is deliberate: it is self-baselining, it survives an unrelated change to the crate's warning set, and it distinguishes "three disappeared" from "three disappeared and one appeared", which a count cannot.

Diff hygiene:

```
git diff --check
git diff --stat
```

### 9.5 Objective acceptance criteria

Accepted only when every statement below is true.

1. `npx tsc --noEmit` exits 0 and reports no diagnostics. (Do not match on literal output text: it varies by invocation wrapper.)
2. `npx tsc --noEmit --noUnusedLocals --noUnusedParameters` reports **0 errors** (baseline: 9).
3. `npm test` passes, with the same suites and test names as baseline.
4. `npm run test:debt` passes.
5. `npm ci && npm run typecheck && npm run build` passes after batch 6, and `package-lock.json` is committed.
6. From `src-tauri`: `cargo check --all-targets` clean, `cargo clippy --all-targets -- -D warnings` clean, `cargo test --lib --bins --tests` passes.
7. `cargo clippy --workspace --all-targets --all-features` finishes with **zero warnings**.
8. `cargo test --lib --bins --tests -- --list` is byte-identical to the baseline capture. No test file is deleted. No assertion is changed except the mechanical edits F3.2, F3.4, F3.5.
9. The `--force-warn dead_code` item list loses exactly `build_attempt_injection`, `read_task_fields_at` and `write_team_config`, and gains **nothing**.
10. The Section 9.3 inspection record exists and covers every Rust item plus the Linux probe outcome.
11. No file listed in Section 3.2 is modified, with one narrowed exception. `src-tauri/src/config/seed_manifest.rs`, `src-tauri/src/commands/role_templates.rs` and `src-tauri/src/phone/consumption.rs` are byte-unchanged, and `src/sidebar/components/AcDiscoveryPanel.tsx` still exists. **`src-tauri/src/phone/mailbox.rs` is byte-unchanged except for lines `:6211` to `:6214`, which are replaced with the exact text in Section 5.1 (R1.10c5).** In particular `:1881` to `:1954` (`wait_for_restore_or_session`, `RestoreWaitOutcome` and their doc block) and their D.5a tests at `:14935` to `:14986` are byte-unchanged. This narrowing resolves the contradiction between the old criteria 10 and 12.
12. Post-deletion reference check, scoped to be decidable: for each deleted Rust symbol, `git grep --word-regexp` over `src-tauri/src src-tauri/tests crates` finds **zero definitions and zero call sites** in `*.rs`; for each deleted frontend symbol, the same over `src` finds zero in `*.ts` and `*.tsx`. Prose mentions outside those extensions are expected and are not failures: this plan names every symbol, and `Toolbar`, `SessionList` and `NewAgentModal` survive in CSS comments (`sidebar.css:906`, `:5381`, `:5598`, `variables.css:27`) and `docs/brand.md:56`, `:57`, none of which are code references. After R1.10c5 the token `DeliveryOutcome` no longer appears in `src-tauri/` at all.
13. No Tauri command, event name, IPC type, wire field, schema, migration, dependency (beyond the three removals), background task, network call, security boundary, or CSS rule is added or changed.
14. `git diff --stat` touches only the paths named in Section 5 plus `plans/1177-remove-dead-code.md` and `package-lock.json`.

## 10. Notes for the implementer

- Work bottom-up inside each file. Every coordinate in Section 5 is from `fae6b09` and earlier edits shift later lines.
- **Anchor on the symbol, not the number, and where the symbol is ambiguous, anchor on the full line text.** Two places where the symbol alone is not enough:
  - `SettingsModal.tsx` contains **two** `const s`. The dead one is `:668`, `const s = () => settings.data;`. The live one is `:641`, `const s = props.section;`, inside a `createEffect` at `:640` that drives `setActiveTab`; it is a genuine reactive read and is not among the nine findings precisely because it is used. A search for `const s` lands on `:641` first. Match the whole line.
  - `api/message_store.rs:2759` sits in an `impl` between two siblings of identical shape, `recover_pty_input_runtime_offloaded` at `:2748` and `due_container_pty_input_candidates_fair_offloaded` at `:2771`, all three-line `spawn_blocking` wrappers. `dev-rust` flagged this as the single easiest place in the batch to delete the wrong function silently. Read the name.
- Never resolve a warning by adding `#[allow(...)]`. This issue removes false lint signal; adding more would invert its purpose.
- `//!` module doc comments are only valid before any item. Both demotion targets open with one (`task.rs:345` to `:348`, `entity_creation.rs:4472` to `:4474`), so the moved function goes below them and below the `use` lines.
- `phone/manager.rs` (the dead feature, #1179) and `phone/mailbox.rs` (the live CLI messaging system) are different modules with confusable names. Only `mailbox.rs:6211` to `:6214` is touched by this plan.
- In `commands/ac_discovery.rs` and `commands/entity_creation.rs`, near-homonyms exist (`sync_workgroup_repos` at `:3678` versus `sync_workgroup_repos_inner` at `:3474`; `ensure_project_context_templates` versus `..._with_publications` and `..._with_clock`). None is in scope; they are named here only so a wide search does not lead anywhere by accident.
- The source inventories are `__agent_dev-rust/DEAD-CODE-RUST-20260731.md`, `__agent_dev-webpage-ui/DEAD-CODE-FRONTEND-20260731.md`, and their revalidations against `fae6b09`. The enrichment artifacts are `__agent_dev-webpage-ui/PLAN-1177-ENRICHMENT-FRONTEND.md` and the three Step 5 and Step 6 messages in the workgroup messaging directory. Section 5 of this plan supersedes all of them, because every coordinate in it was re-read from the tree and then re-verified by a second agent.

## 11. Decisions (all closed)

1. **`deliver_wake_via_api`**: CLOSED as DELETE, with `build_inline_wake_message`, `DeliveryOutcome`, the `OutboxMessage` import, the `actuation.rs` module doc and the `mailbox.rs` backstop comment as mandatory companions. Section 4.4. All three enrichers concur.
2. **`wait_for_restore_or_session` + `RestoreWaitOutcome`**: CLOSED as OUT OF SCOPE. Retained executable specification whose D.5a tests are the only automated coverage of the inlined loop in `handle_close_session` on the only platform CI compiles. Follow-up: un-`#[ignore]` D.5b first, then delete. Section 3.2.
3. **Class B treatment (`read_task_fields_at`, `write_team_config`)**: CLOSED as demote-to-test-scope, not delete-with-tests, with the child-module `use` requirement of Section 5.2 step 4 and the `//!` ordering rule. Section 4.2.
4. **`AgentPickerModal.tsx:371`**: CLOSED as rewrite in place to `selectedScope();`, above the early return at `:386`. Section 4.5.
5. **`WorkgroupGroupsModal.nonstop.test.tsx:20`**: CLOSED as remove-the-parameter plus update both call sites. Underscore-prefixing rejected. Section 4.6.
6. **`docs/reference/architecture.md`**: CLOSED as a rewrite of five mermaid lines and two table rows. Section 5.5.
7. **`EMPTY_CELL`**: CLOSED as delete alongside `profileCellOrDefault`.
8. **`AgentCreatorAPI.createFolder` and the `sessions.ts` five-member cluster**: CLOSED as REGISTERED, NOT REMOVED. Both are dead code this cleanup creates; removing the first is an IPC change and removing the second exceeds mechanical cleanup and breaks CI if done partially. Follow-up issue required. Section 3.2.
9. **`phone/consumption.rs`**: CLOSED as out of scope, registered so the sweep is on record as complete. Section 3.2.
10. **Batch granularity**: CLOSED as seven commits ordered by increasing risk, each with its own gate, with `Cargo.toml` moved out of the lint batch, batch 5 split by `allow` class, and the npm dependency work separated from documentation. Section 8.
11. **Verification protocol**: CLOSED as the six rules of Section 6.2, run at the `fae6b09` baseline with `git grep` over code roots so the plan file is not scanned, counting occurrences rather than lines, with a written record required by criterion 10.
12. **Gate form**: CLOSED as two identity diffs (`-- --list` for tests, item list for `--force-warn dead_code`) rather than counts, and criterion 1 as exit-code based rather than output-text based. Section 9.
13. **Linux cross-compilation probe**: CLOSED as a 15-minute, best-effort, non-blocking attempt whose outcome is recorded either way. Section 4.7.
14. **New tests**: CLOSED as none. Section 9.1.

No implementation decision remains open.

## 12. Scope corrections to issue #1177

These should be applied to the issue body before implementation starts, so the issue and the plan agree.

1. **Frontend item 1 header is wrong.** It says "Delete 8 orphan component files (878 LOC)" and then lists 7, correctly excluding `AcDiscoveryPanel.tsx`. The real figure is **7 files, 459 non-empty lines, 507 physical lines**; the difference is exactly 48 blank lines, and the 878 came from 459 plus `AcDiscoveryPanel.tsx`'s 419. Suggested wording: "Delete 7 orphan component files (459 non-empty lines, 507 physical lines)".
2. **Rust item 7 must drop `wait_for_restore_or_session` + `RestoreWaitOutcome`.** Section 3.2 gives the reasoning. That removes 73 LOC from the issue's stated volume and leaves item 7 with two entries.
3. **Rust item 7's framing is wrong for the two survivors.** `read_task_fields_at` and `write_team_config` are not "delete the symbol and its inline tests". Their tests cover live production code through them. The correct instruction is "move into `#[cfg(test)] mod tests`, add the `use` the child module does not inherit, and drop the `#[allow(dead_code)]`". Section 4.2.
4. **Rust item 6 is missing five companion edits.** Deleting `deliver_wake_via_api` also requires deleting `build_inline_wake_message`, `DeliveryOutcome` and the `OutboxMessage` import, or `-D warnings` fails; and rewriting the `actuation.rs` module doc (`:1` to `:11`) and the `mailbox.rs` backstop comment (`:6211` to `:6214`), both of which otherwise describe code that no longer exists. Deleting `_manifest_path_for_docs` also requires dropping `agency_manifest_path` from the `use` at `cli/agency_templates.rs:11`.
5. **Frontend item 2 is missing one companion edit.** Deleting `profileCellOrDefault` orphans the module-private `EMPTY_CELL` (`profile-utils.ts:26`), which `--noUnusedLocals` then reports, contradicting the issue's own acceptance criterion.
6. **Frontend item 3 is mischaracterised.** "Remove 9 unused locals/imports" covers 5 imports, 2 locals, 1 function declaration and 1 function **parameter**. Two of the nine are not removals: `AgentPickerModal.tsx:371` must be rewritten in place and kept above the early return at `:386`, and `WorkgroupGroupsModal.nonstop.test.tsx:20` is a signature change affecting two call sites.
7. **Frontend item 5 understates the doc work.** Deleting the two stale lines in `docs/reference/architecture.md` orphans `SessionItem`, `SettingsModal` and `OpenAgentModal` in the mermaid graph. Five mermaid lines plus two table rows must be rewritten. Section 5.5.
8. **Acceptance criteria: the `cargo` commands do not match CI.** The issue asks for `cargo check --workspace --all-targets --all-features`; CI runs `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` from `src-tauri`, with neither `--workspace` nor `--all-features`. Both should be listed.
9. **The issue's premise that removal "cannot change behaviour" is true for the binary but needs a caveat for verification.** For nine of the items in scope rustc reports nothing at all, because they are `pub` and reachable from the crate root, or `_`-prefixed. The compiler is not a safety net for them. Section 2.3.
10. **Two pieces of dead code that this cleanup creates should be named in the issue and moved to a follow-up:** `AgentCreatorAPI.createFolder` with the now-unconsumed `create_agent_folder` command, and the five-member cluster in `src/sidebar/stores/sessions.ts`. Section 3.2.
11. **`phone/consumption.rs:27,:54` is confirmed dead code that the issue does not mention.** Its absence is an omission rather than a decision; it stays out of scope here and should be recorded alongside the `seed_manifest.rs` follow-up.

## 13. Enrichment record and resolutions

This section preserves each enricher's findings and records where each is folded into Sections 1 to 12. None of the three edited the plan file; all three verified it byte-identical at `092d85c`.

### dev-rust-grinch (adversarial review)

Verdict at Step 6: **NOT implementable as written**, three blockers, with the deletion and demotion set judged correct once corrected. All three blockers are accepted and resolved.

- **B1 (BLOCKER, resolved). R2.1 does not compile as written.** `read_task_fields_at` takes `&Path` and `use std::path::Path;` lives at `task.rs:9` in the **parent** module; a child module does not inherit it, and Section 5.2 step 5 removes the only `use super::` entry. Grinch reproduced the shape with `rustc`: `E0425 cannot find type Path in this scope`. Re-verified here: `task.rs:9` is the parent import and the test module carries no `Path` import. Resolution: Section 5.2 step 4 now requires `use std::path::Path;` inside `mod tests` (preferred, so the moved body stays byte-identical) or `&std::path::Path` in the signature, and Section 6.3 lists the symptom.
- **B2 (BLOCKER, resolved). The `rg` protocol was unsatisfiable.** The Rule 1 command scanned the working tree including this plan, which names every symbol, so it returned 2 for most items, 4 for `_manifest_path_for_docs` and 12 for `deliver_wake_via_api`. Criterion 12 demanded 0 repo-wide hits afterwards, which can never hold while the plan exists, and would also fail on `DeliveryOutcome` in `mailbox.rs:6214` and on `Toolbar` in CSS comments. Resolution: Rule 1 is now a `git grep` at the `fae6b09` baseline scoped to code roots, counting occurrences rather than lines (Section 6.2); criterion 12 is now scoped to definitions and call sites in code extensions, with the expected surviving prose mentions enumerated (Section 9.5).
- **B3 (BLOCKER, resolved). Internal contradiction on `DeliveryOutcome`.** The plan deleted the enum and demanded zero hits while also demanding `phone/mailbox.rs` be byte-unchanged, and `mailbox.rs:6214` mentions it. Re-verified here: `:6205` to `:6214` is a purge backstop comment enumerating "the two callers for which this Err is safe". Resolution: R1.10c5 authorises an exact rewrite of `:6211` to `:6214` that preserves and sharpens the load-bearing warning, and criterion 11 narrows the `mailbox.rs` protection to everything except those four lines, with `wait_for_restore_or_session` and its D.5a tests still byte-protected by name.
- **Warning (accepted).** `npx tsc --noEmit` exits 0 and prints nothing in Grinch's environment; the architect's run printed a summary line, which indicates an invocation wrapper rather than a disagreement about the compiler. Either way, matching on literal output text is the wrong criterion. Resolution: criterion 1 is now exit-code and diagnostic based.
- **Warning (accepted).** `rg = 1` is not a general proof; the preconditions must be recorded. Resolution: Section 6.2 now opens with the explicit preconditions of Rule 1, sourced from `dev-rust`'s macro, codegen and `build.rs` sweep.
- **Confirmation.** No other RAII, specification or source-scrape load-bearing item exists among the ten Class A deletions or the two demotions; `deliver_wake_via_api` has no caller, config, feature or fallback, and DELETE stands.

### dev-rust (backend)

No dissent on any decision; accepted both corrections the architect made to its own inventory. Its contribution is a change to how the plan verifies, not to what it deletes.

- **Premise correction (accepted, the most consequential item of the round).** The draft's Section 2.3 claimed rustc reports the 24 confirmed items as `dead_code` in a production build. It does not: under `--force-warn dead_code` only three of the plan's items are reported, because the rest are `pub` and reachable from the crate root, or `_`-prefixed. Re-derived here from the visibility of each symbol and the `pub mod` chain in `lib.rs`, independent of the measurement. Resolution: Section 2.3 rewritten with a per-item visibility table and the explicit consequence that text search is the only check for those nine items; Section 6.2 Rule 6 and criterion 9 exist because of it.
- **Rule 1 conflated two questions (accepted).** "Is there a caller" and "what does the deleted body strand" are different, and every cascade came from the second. Resolution: split into Rule 1 and the new Rule 6 (Section 6.2), with `dev-rust`'s ten-body table recorded as already executed.
- **Severity of Section 6.3 overstated (accepted, with a refinement).** `release.yml` builds through `tauri-action` with no clippy step and no `-D warnings`, and the repo has no `[lints.rust]`, `.cargo/config.toml` or `RUSTFLAGS`; verified here. A new `dead_code` on Linux is therefore a warning, not a broken build. **Refinement the architect adds:** this applies to the `allow`-removal failure mode only. A deletion that was live behind a platform gate still fails to *compile* on Linux, which is release-blocking. Resolution: Section 6.3 now carries the two rows separately, with distinct severities.
- **Two documentation companions were missing (accepted).** The `actuation.rs` module doc (`:1` to `:11`) and the `task.rs` test-module doc (`:345` to `:348`) both describe code that the plan changes, by the same standard that took `wait_for_restore_or_session` out of scope. Resolution: R1.10c4 and R2.1 step 7, with exact replacement text.
- **`//!` ordering hazard (accepted).** Resolution: Section 5.2 steps 2, Section 6.3, Section 10.
- **Gate form (accepted, with one modification).** Replace the test-count invariant with a diffed `cargo test ... -- --list`, and add a `--force-warn dead_code` gate. **Modification:** `dev-rust` proposed hardcoding "20 warnings today, 17 after". The plan adopts the gate but as a **list diff**, not a count, for exactly the reason `dev-rust` gave for rejecting the test count: a count cannot distinguish three disappearing from three disappearing and one appearing. It is also self-baselining, so it survives an unrelated change to the crate's warning set, and it does not embed a number the architect did not measure. The three expected disappearances are named in criterion 9. Section 9.4 also records that cargo does not replay diagnostics for a fresh unit, so both measurements must force a rebuild.
- **Batch reordering (accepted in full).** R3.9 out of the lint batch, batch 5 split into 5a and 5b by `allow` class, F4 separated from F5. Resolution: Section 8, now seven batches.
- **Rule 4 confirmed with the compiler after reaching the opposite conclusion by reading (recorded).** `dev-rust`'s first pass argued that `#[allow(dead_code)]` on a type suppresses both "never constructed" and "field is never read", that several `config_seed.rs` fields are never read, and therefore that R3.3 to R3.7 would break the gate. `--force-warn dead_code` refuted it: `config_seed.rs` emits zero warnings in both targets, because the derives plus `log::debug!("{:?}", ...)` at `:910` count as reads. Recorded in Rule 4 because the field-lint concern is correct in general and will be raised again.
- **False cascade chased and closed (recorded).** `product_name` uses `capitalize_suffix`, whose only other user is `mutex_name()`, whose three production callers are all `#[cfg(target_os = "windows")]`. It looked like a Linux-only dead item invisible to Windows CI. It is not: `mutex_name` is `pub` in a `pub` module, so rustc never calls it dead, and `capitalize_suffix` keeps its user. Verified empirically under `--force-warn`. Recorded so it is not re-litigated.
- **Coordinate hazard at R1.8 (accepted).** Resolution: Section 10.
- **`phone/consumption.rs` reported as known dead code outside scope (accepted).** Resolution: Section 3.2 and Section 12 item 11.
- **Offer to edit the plan directly (declined).** Consolidation stays with the architect so the certified bytes have one author.

### dev-webpage-ui (frontend)

No dissent; reproduced the 9-error baseline exactly and confirmed `092d85c` moved no coordinate. Two findings change content, five are precisions on instructions that already existed.

- **`AgentCreatorAPI.createFolder` (accepted, content change).** The draft's Section 2.4 listed `AgentCreatorAPI` as not cascading, citing `project.ts:494`, but that line uses `pickFolder`. Re-verified here: `createFolder` has exactly two occurrences, its definition at `ipc.ts:1082` and `NewAgentModal.tsx:52`, which F1 deletes; the Tauri command `create_agent_folder` is then unconsumed. Resolution: Section 2.5 records the correction and the granularity lesson, Section 3.2 registers it as deliberately not removed, Section 6.2 Rule 6 now requires member granularity, Section 12 item 10 carries it to the issue.
- **The `sessions.ts` five-member cluster (accepted, content change).** `groupedSessions`, `setTeamFilter`, `toggleTeamCollapsed`, the `collapsedTeams` getter and the `teamFilter` getter lose their last consumers to F1. Re-verified here, including the decisive hazard: `collapsedTeams` (`:342`, reader) and `toggleTeamCollapsed` (`:658`, writer via `setCollapsedTeams`) are the two halves of one `createSignal` destructuring at `:226`, so a partial deletion produces a new TS6133. Resolution: Section 3.2 registers the whole cluster as out of scope with the partial-deletion warning, Section 12 item 10 carries it to the issue.
- **The `SettingsModal` homonym (accepted).** `:641` `const s = props.section;` is a live reactive read inside a `createEffect` at `:640`; the dead one is `:668`. The "anchor on the symbol" rule does not protect here because the symbol is identical. Re-verified here. Resolution: Section 5.4 F3.3, Section 6.1, Section 6.3, Section 10.
- **The position of the F3.1 rewrite is load-bearing (accepted).** `selectedScope();` must stay above the early return at `:386`, or the effect stops subscribing whenever no agent is selected. Re-verified here. Resolution: Section 4.5, Section 5.4 F3.1, Section 6.3.
- **Reactivity sweep of all nine (recorded).** F3.1 is the only finding inside a reactive scope. `SettingsModal:668` is an arrow function whose definition reads nothing and which is never invoked, so deleting it is safe.
- **Inverse lens on F3.6 to F3.9 (recorded).** No cleanup is missing; the four `solid-js` imports are genuine surplus. Section 5.4.
- **Companion sweep (recorded).** `EMPTY_CELL` is the only frontend companion of its kind; `cellForLetter` is a false alarm with its own live consumer at `:174`; `ProjectState` and `FakeTransport` both survive their F3 edits. Section 2.5.
- **Mermaid tree verified exact (recorded), with a bonus.** `ActionBar.tsx` does not import `OpenAgentModal`, which is what justifies re-parenting `OA` under `SI`, and the old `Toolbar` label was wrong about "Open Agent" as well. Resolution: Section 4.3, and the `:177` and `:721` replacement labels now say "Project creation + Settings gear".
- **F1 order asymmetry (accepted).** Deleting the imported file without the importer does not compile; the reverse only leaves an orphan. Resolution: Section 5.4.
- **Empty-import-list rule is inapplicable (accepted).** None of the four lists empties; only F3.2 deletes a whole statement. Resolution: Section 5.4 F3.
- **Count disambiguation (accepted).** 507 physical, 459 non-empty, difference exactly 48 blank lines. Resolution: Section 5.4 table and Section 12 item 1 now state both measures explicitly instead of "by inventory count".

Status: READY_FOR_IMPLEMENTATION
