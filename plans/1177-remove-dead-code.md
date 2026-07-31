# Implementation Plan: #1177 Remove confirmed dead code (mechanical cleanup)

Status: DRAFT (Step 4). Not yet READY_FOR_IMPLEMENTATION.

Full path. This is the architect draft. Enrichment by `dev-rust`, `dev-webpage-ui` and `dev-rust-grinch` follows; `Status: READY_FOR_IMPLEMENTATION` and `Plan-SHA256` are set at Step 7 after the consensus verdict.

## 1. Issue, baseline, and objective

- Issue: https://github.com/mblua/AgentsCommander/issues/1177 (`chore: remove confirmed dead code (mechanical cleanup, ~1000 LOC)`).
- Branch: `chore/1177-remove-dead-code`, created from `main` at `fae6b09466db27c30fae404c04102e294cf1b5d6`.
- Baseline verified at plan time: `git rev-parse HEAD` = `fae6b09466db27c30fae404c04102e294cf1b5d6`, `git status --porcelain` empty.
- Delivery classification: FULL. Not because of size, but because three properties make it non-mechanical: (a) PR CI compiles Rust only on Windows, so a wrong deletion behind a platform gate ships green and breaks at release; (b) the 24 compiler-reported items split into three distinct deletion classes, and deleting by coordinate without reading the context breaks the build or the tests; (c) `deliver_wake_via_api` needed an architectural decision, resolved in Section 4.4.

Objective: remove production code that has no consumer, with zero behaviour change, zero test-coverage loss, and no regression of the current baseline (`cargo clippy` at zero warnings, `tsc --noEmit` clean).

Non-objective: this is not a refactor. Nothing is renamed, moved for style, or improved. Every edit either deletes an unreachable item or is a mandatory companion edit that keeps the build and the lints at baseline.

## 2. Verified current state

Every coordinate below was re-verified against the working tree at `fae6b09` while writing this plan. None was taken on faith from the source inventories.

### 2.1 Baseline signals

| Signal | Command | Result at `fae6b09` |
| --- | --- | --- |
| Frontend typecheck | `npx tsc --noEmit` | clean (repo baseline) |
| Frontend unused check | `npx tsc --noEmit --noUnusedLocals --noUnusedParameters` | 9 errors in 8 files, reproduced verbatim in Section 5.3 |
| Rust lint | `cargo clippy --workspace --all-targets --all-features` | zero warnings (per `dev-rust` revalidation) |

### 2.2 CI reality (this is the risk driver)

`.github/workflows/pr-regression-gates.yml`, read at `fae6b09`:

| Job | Runner | Commands |
| --- | --- | --- |
| `test-debt` | `ubuntu-latest` | `npm run test:debt` |
| `rust-regression` | `windows-latest` | in `src-tauri`: `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib --bins --tests` |
| `windows-release-cli-smoke` | `windows-latest` | `npm run build:prod`, `npm run smoke:cli-release-windows` |
| `frontend-regression` | `ubuntu-latest` | `npm run typecheck`, `npm test` (with the #480 known-debt guard) |

`release.yml` is the only workflow that builds Rust on `ubuntu-22.04` and `macos-latest`.

Two consequences the implementer must internalise:

1. **No PR job compiles Rust for Linux or macOS.** A deletion that is only live under `#[cfg(unix)]` / `#[cfg(not(windows))]` passes every PR gate and breaks at release. Verification for those cases is source inspection, per Section 6.2.
2. **`cargo clippy --all-targets -- -D warnings` makes any new warning a hard CI failure.** A leftover unused import after a deletion is not a nit; it fails the build. This is why the companion edits in Section 5 are mandatory, not optional.

### 2.3 The three deletion classes

The 24 items rustc reports as `dead_code` in a production (`--lib --bins`) build do not share one removal procedure:

- **Class A, atomic.** Exactly one occurrence in the whole repository: its own definition. Deleting the definition cannot break anything, and no `#[cfg]` block can reference it, because a gated caller would necessarily be a second occurrence. 10 items.
- **Class B, dead in production but exercised by inline `#[cfg(test)]` code.** Deleting the item alone breaks `cargo test`. 13 items in the source inventory; this plan takes 2 of them and rejects the rest (Section 3.2).
- **Class C, the deletion drags live production code.** `role_path` is written at `commands/role_templates.rs:642`; `ReadOnlyCanonical` is matched at `config/seed_manifest.rs:267` and `:298`. Not dead. Already excluded by the issue and confirmed here.

### 2.4 Cascade findings that the issue does not carry

These were found while verifying, and each one would break CI if the implementer deleted only what the issue lists.

| Deletion | Cascade | Consequence if ignored |
| --- | --- | --- |
| `deliver_wake_via_api` (`api/actuation.rs:186`) | sole caller of `build_inline_wake_message` (`:150`) and sole user of `DeliveryOutcome` (`:25`); `use crate::phone::types::OutboxMessage;` (`:19`) is used only by `build_inline_wake_message` | leftover unused import fails `-D warnings`; two new dead `pub` items that clippy cannot see stay in the tree |
| `_manifest_path_for_docs` (`cli/agency_templates.rs:1005`) | `agency_manifest_path` appears in that file only at the `use` list (`:11`) and inside this function (`:1006`) | leftover unused import fails `-D warnings` |
| `profileCellOrDefault` (`src/shared/profile-utils.ts:99`) | sole user of the module-private `EMPTY_CELL` (`:26`) | `tsc --noUnusedLocals` reports a new TS6133, so the issue's own acceptance criterion cannot be met |
| deleting the 2 stale `SessionList.tsx` / `Toolbar.tsx` doc lines | the mermaid graph in `docs/reference/architecture.md` hangs `SessionItem`, `SettingsModal` and `OpenAgentModal` off those two nodes (`:179`, `:181`, `:182`) | three live components lose their parent edge; the diagram becomes more wrong, not less |

Verified as NOT cascading (each retains at least one live caller outside the deleted item): `capitalize_suffix` (`config/profile.rs:88`), `origin_matrix_dir_for_launch_path` (`config/coding_agent_profiles.rs:659`), `read_replica_profile_result` (5 live callers), `validate_expanded_codex_home_value` (6 live callers in `config/agent_command.rs` plus `config/settings.rs:1365`), `due_pty_input_ids` (`api/message_store.rs:3644`), `AC_WORKSPACE_DIR` (`src/shared/path-extractors.ts:9`, `src/sidebar/components/EditTeamModal.tsx:115`), `getConsoleText` (`src/shared/voice-recorder.ts:265,:333`), `PhoneMessage` (`src/shared/ipc.ts:811`), `ProfileCellConfig` (`profile-utils.ts:93,:107`), `AgentCreatorAPI` (`src/sidebar/stores/project.ts:494`), `AgentPickerModal` (`AcDiscoveryPanel.tsx:6`, `AgentPickerModal.test.tsx:4`), `otherProjectPath` (`WorkgroupGroupRail.favorites.test.tsx:217,:232,:435,:448`), `wg(...)` (5 uses in the same file), `TeamConfigReadError::class` (`src/session/context_alerts.rs:1579`, which is production: that file's only `#[cfg(test)]` starts at `:1720`).

### 2.5 The two items this plan removes from the issue's scope

Both are cases where the compiler says "dead" and the design says "load-bearing". Full reasoning in Section 3.2.

- `wait_for_restore_or_session` + `RestoreWaitOutcome` (`phone/mailbox.rs:1900`, `:1919`).
- (Scope corrections that are not deletions are collected in Section 12.)

## 3. Scope

### 3.1 In scope

Frontend (`src/`, `package.json`, `docs/`):

1. Delete 7 orphan component files.
2. Delete 8 unused type/function/const declarations plus the one cascade companion (`EMPTY_CELL`).
3. Resolve the 9 `--noUnusedLocals` / `--noUnusedParameters` findings.
4. Remove 2 unused npm dependencies.
5. Correct `docs/reference/architecture.md` so it documents the live tree.

Rust (`src-tauri/`):

6. Delete 10 Class A items plus their mandatory companion edits.
7. Demote 2 Class B items to test scope.
8. Remove 2 Windows-gated unused imports and their `#[allow(unused_imports)]`.
9. Remove 6 `#[allow(dead_code)]` that suppress nothing.
10. Remove the redundant `tauri-build` entry from `[dependencies]`.

Plan artifact: `plans/1177-remove-dead-code.md`.

### 3.2 Out of scope, with reasons

**Removed from the issue's list by this plan:**

- **`wait_for_restore_or_session` + `RestoreWaitOutcome`** (`phone/mailbox.rs:1881` to `:1954`, 73 LOC, plus the D.5a tests at `:14935`, `:14960`, `:14967`, `:14979`, `:14986`). The doc block above it (`:1881` to `:1897`) is explicitly labelled `LOAD-BEARING COMMENT` and states: *"The inlined wait loop in `handle_close_session` has no direct unit test on Windows (D.5b is `#[ignore]`'d ...). Equivalence between the two implementations is enforced ONLY by this comment + the helper's D.5a unit tests."* The live production loop at `mailbox.rs:8264` to `:8283` points back at it (`:8263`: *"The helper's logic is unit-tested separately (D.5a)"*). The helper is a retained executable specification, and its tests are the only automated coverage of the inlined loop's semantics on the only platform CI compiles. Deleting the pair keeps the binary identical and silently destroys that coverage, and leaves the production comment at `:8263` pointing at tests that no longer exist. That is a verification regression, not a mechanical cleanup. Same class as the `PinnedDirectory::file` RAII guard the revalidation already rescued. If the team wants the `#[allow(dead_code)]` noise gone, the correct move is a per-item `allow` with the reason written, which is exactly what is already there at `:1898` and `:1918`. Recommend a follow-up issue: un-`#[ignore]` D.5b, then delete the helper.

**Already out of scope per the issue, confirmed here:**

- The 4 findings dropped by revalidation: `encode_lower_hex_bytes` (`seed_manifest.rs:870`, called at `:629` under `#[cfg(unix)]`), `remove_if_same_identity` (`:2791`, called at `:2812` under `#[cfg(not(windows))]`), `AtomicReplace` (`:171`, constructed at `:2892` and `:6851` under non-Windows gates), and the `file` field of `PinnedDirectory` (`:1570`, a `Drop`-load-bearing directory handle).
- `role_path` (`commands/role_templates.rs:86`) and `ReadOnlyCanonical` (`seed_manifest.rs:162`): Class C, drag live production code.
- The module-wide `#![allow(dead_code)]` at `seed_manifest.rs:19` and the 12 findings underneath it. Separate follow-up.
- `ac_discovery` (#1178), `phone` (#1179), the missing `pty_resized` / `telegram_incoming` listeners (#1180), and `sync_workgroup_repos`.
- `AcDiscoveryPanel.tsx` (419 LOC) and `AcDiscoveryAPI`: belong to #1178.
- Selector-level dead CSS: never measured. `sidebar.css` section headers that name deleted components (`:906`, `:5381`, `:5598`) and `variables.css:27` are comments, not code references. Not touched.
- `scripts/kill-dev.sh` and `scripts/all_agentscommander_standalone_come_to_me.ps1` (D5/D6): operational scripts, owner decision.
- The ~50 superfluous `export` modifiers on internally-used frontend symbols: cosmetic, and removing them would break the tests that import them directly.
- `unreachable_pub` (19 cases) and the `[lints.rust]` workspace proposal: a real improvement, but a behaviour-of-the-lint change, not dead-code removal.

### 3.3 Explicit non-goals

No renames, no signature changes, no module restructuring, no new crates, no new npm packages, no IPC change of any kind (no command added or removed, no event, no wire field, no `serde` rename, no `src/shared/types.ts` field touched that crosses the IPC boundary), no migration, no CSS change, no CI change.

## 4. Decided solution

### 4.1 Deletion taxonomy applied

Every item in scope is assigned exactly one treatment. No item is left to implementer judgement.

| Treatment | Definition | Verification |
| --- | --- | --- |
| **A. Atomic delete** | exactly one repo-wide occurrence | `rg -w` returns 1 line before the edit, 0 after |
| **A+. Atomic delete with companion** | as A, plus a named import or helper that the deletion orphans, edited in the same commit | as A, plus `cargo clippy -- -D warnings` clean |
| **B. Demote to test scope** | dead in production, but its `#[cfg(test)]` callers cover live behaviour through it; move the item into the file's `#[cfg(test)] mod tests` and drop its `#[allow(dead_code)]` | `cargo test --lib --bins --tests` green with no test deleted |
| **D. Delete outright (frontend)** | orphan file or unreferenced declaration | `tsc --noEmit` and `tsc --noEmit --noUnusedLocals --noUnusedParameters` clean, `npm test` green |
| **R. Rewrite in place** | the finding is not a deletion: the line stays and changes shape | per-item, Section 5 |

### 4.2 Why "demote to test scope" and not "delete item plus its tests"

For `read_task_fields_at` and `write_team_config`, the inline tests are not tests *of the dead item*. They use it as a reader or writer to assert live behaviour:

- `commands/task.rs`: of the 8 call sites, 4 (`:355`, `:364`, `:374`, `:386`) test the helper itself, and 4 (`:398`, `:414`, `:529`, `:629`) use it to read back the result of `cli::task_ops::perform(...)` and `validate_wg_root(...)`, which are live production functions. Deleting the helper and all 8 tests removes coverage of `task_ops::perform`'s `SetTitle` and `Clean` paths.
- `commands/entity_creation.rs`: both call sites use it as a writer. `:4791` asserts that a team config never persists absolute project paths (the `normalize_team_config_for_project` contract). `:5466` asserts that a canonical write sorts `context_alert_percentages`. Both cover live production behaviour.

Moving the item inside `#[cfg(test)] mod tests` achieves the actual goal (the symbol leaves the production binary, and the `#[allow(dead_code)]` goes with it) at zero coverage cost and with a much smaller diff than rewriting eight assertions. Both target modules already carry the surrounding scope: `task.rs:344` `mod tests` and `entity_creation.rs:4471` `mod tests` with `use super::*;` at `:4476`.

### 4.3 Why the frontend doc edit is a rewrite, not a deletion

`docs/reference/architecture.md` section 3.1 renders `SA --> SL["SessionList.tsx"]` (`:176`) and `SA --> TL["Toolbar.tsx"]` (`:177`), and then hangs three live components off them: `SL --> SI["SessionItem.tsx"]` (`:179`), `TL --> SM["SettingsModal.tsx"]` (`:181`), `TL --> OA["OpenAgentModal.tsx"]` (`:182`). Deleting only lines 176 and 177 orphans three live nodes.

The live tree, verified by import graph:

```
src/sidebar/App.tsx:67       -> ProjectPanel.tsx
src/sidebar/App.tsx:65,:858  -> ActionBar.tsx
ProjectPanel.tsx:54          -> SessionItem.tsx
SessionItem.tsx:13           -> OpenAgentModal.tsx
ActionBar.tsx:13             -> SettingsModal.tsx
```

So the edit re-points the two stale nodes at the two real ones and re-parents `OpenAgentModal` under `SessionItem`. Exact replacement text in Section 5.5.

### 4.4 Decision: `deliver_wake_via_api` is DELETED

`dev-rust` asked the architect to confirm whether this is reserved as a planned fallback. **It is not. It is deleted, together with the two symbols it is the sole user of.**

Evidence read at `fae6b09`:

1. Zero references. `rg -w deliver_wake_via_api` over the whole repo (excluding `target/`, `node_modules/`, `dist/`, `.git/`, lockfiles) returns exactly one line: its own definition at `api/actuation.rs:186`.
2. The path it implements has a live successor that does the same thing. `deliver_wake_via_api` resolves the target (`resolve_api_send_target`, `:193`), builds an inline message (`:194`), and actuates through `MailboxPoller::new().deliver_wake_with_origin(app, &msg, WakeDeliveryOrigin::DbQueue)` (`:198` to `:204`). The dispatcher performs the identical actuation, with the identical origin tag, at `api/dispatcher.rs:184` to `:191`.
3. The handler already migrated. `api/handlers/send.rs:1` to `:3` states: *"The handler queues inline content durably and the dispatcher performs delivery."* The handler calls `actuation::resolve_api_send_target` (`send.rs:60`) and then enqueues; it does not call the inline path.
4. Nothing preserves it as a fallback. There is no feature flag, no `cfg`, no comment reserving it, and no configuration branch that could route back to it. `api/actuation.rs` contains zero `#[cfg]` platform gates, so there is no non-Windows caller either.

Design reasoning for the decision itself, not just the mechanics: a synchronous inline delivery path is not a viable fallback for the queue path, so keeping it costs correctness rather than buying safety. The queue exists because delivery must survive a process restart and must be retried under `max_attempts` with leasing and a purge guard (`dispatcher.rs:167` to `:198`). `deliver_wake_via_api` has no durability, no retry, no lease, and no purge interaction; a caller that fell back to it under load would silently drop the very guarantees the migration was made to obtain. If a future increment ever needs an inline send, it should be written against the contracts that exist then, not resurrected from a pre-queue snapshot. Keeping 32 unreachable lines to hedge against that is the worse trade: it is dead weight that reads like a supported path.

Consequently the deletion is A+ and takes three companions in the same commit:

- `build_inline_wake_message` (`:150` to `:179`): its only caller is `deliver_wake_via_api` (`:194`). Repo-wide `rg -w` returns exactly those two lines.
- `DeliveryOutcome` (`:23` to `:30`): referenced only at `:184` (doc), `:192` (return type), `:206` and `:207` (construction), all inside the deleted function. The only other repo mention is a comment at `phone/mailbox.rs:6214`, which is prose and is left alone.
- `use crate::phone::types::OutboxMessage;` (`:19`): used only by `build_inline_wake_message`.

Not touched in `actuation.rs`: `reject_if_root`, `build_send_body`, `resolve_send_file_path`, `resolve_api_send_target`, `lean_peers_via_api`, `SendFileContent`, the `INLINE_BODY_MAX_BYTES` import (still used at `:82`, `:85`, `:316`), and the entire `#[cfg(test)] mod tests` at `:223` onwards.

### 4.5 Decision: `AgentPickerModal.tsx:371` is a rewrite, not a deletion

`const scope = selectedScope();` sits inside a `createEffect` (`:370`) whose next lines are `restartSessions();` (`:374`) and `targetReplicaPath();` (`:375`), bare calls that exist purely to register reactive dependencies. In SolidJS, reading a signal inside an effect is what subscribes the effect to it. Deleting the whole line would silently unsubscribe the effect from `selectedScope`, so the effect would stop re-running on a scope change. That is a behaviour change, and this issue promises none.

Required edit: replace `const scope = selectedScope();` with `selectedScope();`. Keep `:372` and `:373` (`agent`, `profile`) exactly as they are; `tsc` flags only `scope`, so those two bindings are read later in the effect body.

### 4.6 Decision: `WorkgroupGroupsModal.nonstop.test.tsx:20` is a signature edit

`fake` is an unused *parameter* of `function mountModal(fake: FakeTransport)`, not a local. The minimal correct fix removes the parameter and updates both call sites (`:66`, `:96`) to `mountModal()`. Underscore-prefixing (`_fake`) is rejected: it silences the compiler while leaving a parameter that lies about the function's inputs. `fake` remains live in both tests (`:59` to `:63`, `:71`; `:86` to `:93`), so no new unused binding appears.

## 5. Exact affected surfaces

Line numbers are as read at `fae6b09`. **Within a single file, apply edits bottom-up (highest line first)** so earlier edits do not shift later coordinates. Anchor on the symbol name, and if a coordinate does not match the symbol, stop and report rather than deleting by line number.

### 5.1 Rust, Class A and A+ (batch R1)

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
| R1.8 | `due_pty_input_ids_offloaded` | `src-tauri/src/api/message_store.rs` | `:2759` to `:2769` plus blank | A |
| R1.9 | `build_attempt_injection` | `src-tauri/src/cli/role_experiment.rs` | `:2142` to `:2148` (the `#[allow(dead_code)]` at `:2142` included) plus blank | A |
| R1.10 | `deliver_wake_via_api` | `src-tauri/src/api/actuation.rs` | `:181` to `:212` (doc `:181` to `:185` included) plus blank | **A+** |
| R1.10c1 | companion: `build_inline_wake_message` | `src-tauri/src/api/actuation.rs` | `:150` to `:179` plus blank | A+ |
| R1.10c2 | companion: `DeliveryOutcome` | `src-tauri/src/api/actuation.rs` | `:23` to `:30` (doc `:23` and `#[derive]` `:24` included) plus blank | A+ |
| R1.10c3 | companion: `use crate::phone::types::OutboxMessage;` | `src-tauri/src/api/actuation.rs` | `:19` | A+ |

After R1, `src-tauri/src/api/actuation.rs` should shrink from 322 lines to roughly 250, and `cargo clippy --all-targets -- -D warnings` must still be clean.

### 5.2 Rust, Class B demotions (batch R2)

**R2.1 `read_task_fields_at`, `src-tauri/src/commands/task.rs`**

1. Cut `:174` to `:191` (doc comment `:174` to `:177`, `#[allow(dead_code)]` `:178`, function `:179` to `:191`).
2. Paste the function plus its doc comment inside `mod tests` (opens at `:343` to `:344`), immediately after the `use` lines. **Drop the `#[allow(dead_code)]`**: the item is used by the tests, so the lint is satisfied without it.
3. Edit the test module's import at `:349` from `use super::{read_task_fields_at, validate_wg_root};` to `use super::validate_wg_root;`.
4. The function body calls `crate::commands::entity_creation::parse_task_title` by full path (`:183`), so it resolves unchanged from inside `mod tests`.
5. No test is deleted or modified. All 8 call sites keep working.

**R2.2 `write_team_config`, `src-tauri/src/commands/entity_creation.rs`**

1. Cut `:895` to `:904` (comment `:895`, `#[allow(dead_code)]` `:896`, function `:897` to `:904`).
2. Paste the function inside `mod tests` (opens at `:4470` to `:4471`), after `use super::*;` (`:4476`). `use super::*;` brings `TeamConfigMutationGuard` and `write_team_config_guarded` into scope, so the body resolves unchanged.
3. **Drop the `#[allow(dead_code)]`.** Replace the `:895` comment with one that states the new truth, for example: `// Test-only synchronous wrapper: acquires the mutation guard around write_team_config_guarded.`
4. No test is deleted or modified. Both call sites (`:4791`, `:5466`) keep working.
5. Guard interaction, verified: the source-scraping guard in `src-tauri/src/config/local_config_io.rs` allowlists `entity_creation.rs` lines containing the substring `write_team_config` (`:611` to `:618`) and strips `#[cfg(test)] mod tests` blocks before scanning (`strip_test_modules`, `:620`). The allowlist entry stays needed by `write_team_config_guarded` and `write_team_config_guarded_with_publisher`, which remain in production, and the moved copy lands inside a stripped block. The guard test must be re-run to confirm (it runs inside `cargo test --lib`).

### 5.3 Rust, imports, `allow`s and manifest (batch R3)

| # | Item | File | Coordinates | Note |
| --- | --- | --- | --- | --- |
| R3.1 | `#[allow(unused_imports)]` + `use std::os::windows::process::CommandExt;` | `src-tauri/src/commands/entity_creation.rs` | `:4389` to `:4390` | inside `#[cfg(windows)] { ... }` (`:4387`); `creation_flags` at `:4391` comes from `tokio::process::Command`, not this trait |
| R3.2 | same pair | `src-tauri/src/commands/entity_creation.rs` | `:4427` to `:4428` | inside `#[cfg(windows)] { ... }` (`:4425`); `creation_flags` at `:4429` |
| R3.3 | `#[allow(dead_code)]` on `ConfigSeedPublication` | `src-tauri/src/config/config_seed.rs` | `:76` | |
| R3.4 | `#[allow(dead_code)]` on `CollectedSeedFiles` | `src-tauri/src/config/config_seed.rs` | `:85` | |
| R3.5 | `#[allow(dead_code)]` on `ConfigSeedSkipReason` | `src-tauri/src/config/config_seed.rs` | `:95` | |
| R3.6 | `#[allow(dead_code)]` on `ConfigSeedFailure` | `src-tauri/src/config/config_seed.rs` | `:119` | |
| R3.7 | `#[allow(dead_code)]` on `ConfigSeedRollbackFailure` | `src-tauri/src/config/config_seed.rs` | `:134` | |
| R3.8 | `#[allow(dead_code)]` on `TeamConfigReadError::class` | `src-tauri/src/commands/entity_creation.rs` | `:831` | `class()` is called in production at `src/session/context_alerts.rs:1579` |
| R3.9 | `tauri-build = { version = "2", features = [] }` | `src-tauri/Cargo.toml` | `:10`, in `[dependencies]` | the `[build-dependencies]` copy at `:77` is what resolves `build.rs`; keep it |

Only the `#[allow(...)]` attribute lines are removed in R3.3 to R3.8. The types, the enum, and the method they sit on are live and stay byte-identical.

### 5.4 Frontend deletions (batches F1 and F2)

**F1: 7 orphan files, deleted whole.**

| File | Physical lines |
| --- | --- |
| `src/sidebar/components/NewAgentModal.tsx` | 186 |
| `src/sidebar/components/Toolbar.tsx` | 92 |
| `src/sidebar/components/SessionList.tsx` | 86 |
| `src/guide/components/CatalystTab.tsx` | 49 |
| `src/sidebar/components/TeamFilter.tsx` | 35 |
| `src/sidebar/components/CollapsibleSection.tsx` | 30 |
| `src/sidebar/components/TeamGroupHeader.tsx` | 29 |
| **total** | **507** |

The two closed clusters must go together: `Toolbar.tsx` imports `NewAgentModal.tsx` (`:4`), and `SessionList.tsx` imports `TeamGroupHeader.tsx` (`:4`). Nothing outside the set imports any of the 7. Every symbol they import stays live for other consumers (verified in Section 2.4), so this creates no second-order orphan.

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

Do not touch `TeamSessionGroup` (`types.ts:986`): it is a different, live type that a substring search for `SessionGroup` also matches. Do not touch `AC_WORKSPACE_DIR` (`constants.ts:3`), `PhoneMessage` (`types.ts:1031`), `getConsoleText`, `getConsoleLogs`, or `ProfileCellConfig`.

**F3: the 9 unused declarations.** Reproduced verbatim from `npx tsc --noEmit --noUnusedLocals --noUnusedParameters` at `fae6b09`:

| # | Finding | Treatment |
| --- | --- | --- |
| F3.1 | `AgentPickerModal.tsx(371,11)` `'scope'` | **rewrite** to `selectedScope();`, see Section 4.5 |
| F3.2 | `ProjectPanel.collapse-state.test.tsx(16,1)` `'projectCollapseStore'` | delete the import |
| F3.3 | `SettingsModal.tsx(668,9)` `'s'` | delete `const s = () => settings.data;` (a plain unused closure, no reactive registration at definition) |
| F3.4 | `WorkgroupGroupRail.favorites.test.tsx(56,10)` `'otherProject'` | delete the whole function `:56` to `:66`; `otherProjectPath` and `wg(...)` stay live |
| F3.5 | `WorkgroupGroupsModal.nonstop.test.tsx(20,21)` `'fake'` | **signature edit**, see Section 4.6: drop the parameter at `:20`, update `:66` and `:96` to `mountModal()` |
| F3.6 | `spec-board/App.tsx(2,21)` `'createEffect'` | remove from the import list |
| F3.7 | `AskAgentPanel.tsx(2,35)` `'onMount'` | remove from the import list |
| F3.8 | `AskAgentPanel.tsx(2,44)` `'onCleanup'` | remove from the import list (same line as F3.7) |
| F3.9 | `MermaidPreview.tsx(2,44)` `'onCleanup'` | remove from the import list |

If removing a named import empties its `{ ... }` list, delete the whole import statement.

**F4: dependencies.** `package.json`: remove `"@tauri-apps/plugin-shell": "^2"` (`:32`, `dependencies`) and `"@types/dompurify": "^3.0.5"` (`:43`, `devDependencies`). Commit the resulting `package-lock.json` change. Leave `"kill-dev"` (`:12`) and every other script untouched.

### 5.5 Documentation (batch F5)

`docs/reference/architecture.md`, two edits.

Mermaid graph, section 3.1:

| Line | From | To |
| --- | --- | --- |
| `:176` | `    SA --> SL["SessionList.tsx<br/>For each session → SessionItem"]` | `    SA --> PP["ProjectPanel.tsx<br/>Projects, workgroups, replicas → SessionItem"]` |
| `:177` | `    SA --> TL["Toolbar.tsx<br/>Open Agent + New Session + Settings"]` | `    SA --> AB["ActionBar.tsx<br/>Sidebar actions + Settings gear"]` |
| `:179` | `    SL --> SI["SessionItem.tsx<br/>...` | `    PP --> SI["SessionItem.tsx<br/>...` (label unchanged) |
| `:181` | `    TL --> SM["SettingsModal.tsx<br/>...` | `    AB --> SM["SettingsModal.tsx<br/>...` (label unchanged) |
| `:182` | `    TL --> OA["OpenAgentModal.tsx<br/>...` | `    SI --> OA["OpenAgentModal.tsx<br/>...` (label unchanged) |

The `style` lines at `:194` to `:197` reference `SA`, `SM`, `OA`, `SI` only, so they need no change. No `SL` or `TL` identifier may remain anywhere in the block.

File table, section 8:

| Line | From | To |
| --- | --- | --- |
| `:719` | `| `sidebar/components/SessionList.tsx` | `<For>` over sessions → `SessionItem` |` | `| `sidebar/components/ProjectPanel.tsx` | Projects, workgroups and replicas → `SessionItem` |` |
| `:721` | `| `sidebar/components/Toolbar.tsx` | Open Agent + New Session + Settings gear |` | `| `sidebar/components/ActionBar.tsx` | Sidebar actions + Settings gear |` |

`docs/brand.md:56` and `:57` say "Toolbar btn"; that is a CSS design-token name, not a component reference. Do not touch it.

## 6. Required behaviour, failure modes, and the platform gate

### 6.1 Required behaviour

| Situation | Required behaviour |
| --- | --- |
| Any user-visible flow (sidebar, terminal, watchers, guide, spec-board, PTY, API send, CLI) | byte-identical to baseline; this change removes only code with no consumer |
| IPC surface | unchanged: no Tauri command added or removed, no event, no wire field, no `serde` attribute, no `src/shared/types.ts` type that crosses the boundary |
| `cargo test --lib --bins --tests` | same test count as baseline, minus zero; no test file deleted, no assertion changed, except the mechanical edits named in F3.2, F3.4 and F3.5 |
| `cargo clippy --all-targets -- -D warnings` | zero warnings, same as baseline |
| `tsc --noEmit --noUnusedLocals --noUnusedParameters` | 0 errors after the change (9 before) |
| Reactive behaviour of `AgentPickerModal`'s scope effect | unchanged: the effect still re-runs on a `selectedScope` change (Section 4.5) |
| `local_config_io` source-scraping guard | still passes; the allowlist substring is still matched by the production `write_team_config_guarded*` functions |
| Non-Windows builds (`release.yml`: `ubuntu-22.04`, `macos-latest`) | still compile |

### 6.2 Platform-gate verification protocol

This is the part CI cannot do for you. For each Rust deletion, apply the applicable rule and record the result:

**Rule 1, closed by construction.** If `rg --word-regexp --hidden --no-ignore <symbol> --glob '!**/target/**' --glob '!**/node_modules/**' --glob '!**/dist/**' --glob '!**/.git/**' --glob '!*.lock' .` returns **exactly one line, the definition itself**, then no `#[cfg(unix)]` or `#[cfg(not(windows))]` caller can exist, because such a caller would be a second occurrence. This closes the platform question for R1.1 to R1.10 with no inspection needed. Verified at plan time: all ten return exactly 1 line.

**Rule 2, enumerated references.** If the symbol has references, every reference must be enumerated and each one placed inside or outside a platform gate by reading the file. Applies to:
- `read_task_fields_at`: 8 references, all inside `#[cfg(test)] mod tests` (opens `task.rs:343`). `task.rs` has exactly one `#[cfg]` in the file and it is that `#[cfg(test)]`. No platform gate involved.
- `write_team_config`: 2 references, both inside `#[cfg(test)] mod tests` (opens `entity_creation.rs:4470`), plus one string-literal mention in `local_config_io.rs:615` handled in Section 5.2.

**Rule 3, gated-code edits.** R3.1 and R3.2 sit inside `#[cfg(windows)] { ... }` blocks (`entity_creation.rs:4387`, `:4425`). They compile only on Windows, so `rust-regression` on `windows-latest` is the exact right verifier. No non-Windows exposure.

**Rule 4, `allow` removals need the covered item traced.** Removing an `#[allow(dead_code)]` is only safe if the item it covers is live in code that compiles on **every** platform. If the item were live only under `#[cfg(windows)]`, removing the `allow` would produce a `dead_code` warning on Linux and macOS that Windows CI could never show. Verified at plan time for all six:
- The five `config_seed.rs` types are all reachable through `ConfigSeedReport` (`:147` to `:152`), which is ungated. Their construction and field reads live at `:385`, `:480`, `:508`, `:541`, `:552`, `:580`, `:600`, `:614`, `:645`, `:663`, `:686`, `:842`, `:855`, `:898`, `:903`, `:930`, `:932`, `:947`, `:952`, `:965`, `:980`, all in ungated production code. The only platform gates in that file's production half are the two five-line `metadata_is_reparse` definitions (`:759` `#[cfg(windows)]`, `:766` `#[cfg(not(windows))]`), which touch none of these types. The file's `#[cfg(test)] mod tests` opens at `:1275`, so `:1742`, `:2189` and `:2193` are test-side and irrelevant.
- `TeamConfigReadError::class` is called at `src/session/context_alerts.rs:1579`, which is ungated production (that file's only `#[cfg(test)]` opens at `:1720`).

**Rule 5, companion imports.** For A+ deletions, confirm the companion import has no other user in the file **including inside platform-gated blocks**, before removing it. Verified: `agency_manifest_path` appears in `cli/agency_templates.rs` only at `:11` and `:1006`; `OutboxMessage` appears in `api/actuation.rs` only at `:19`, `:150` and `:155`, and that file has zero `#[cfg]` gates.

### 6.3 Failure modes and what each one means

| Symptom | Almost certainly means | Action |
| --- | --- | --- |
| `unused import` error under `-D warnings` | a companion edit from Section 5 was skipped | apply the companion, do not add an `#[allow]` |
| `cannot find function ...` in a test | a Class B item was deleted instead of demoted | restore and follow Section 5.2 |
| `dead_code` warning after an `allow` removal | Rule 4 was violated, or the item genuinely became dead | stop; report, do not re-add the `allow` silently |
| a Linux or macOS release build breaks | a deletion was live behind a platform gate | revert that single item and report; the batch structure in Section 8 keeps this to one commit |
| `tsc --noUnusedLocals` reports a NEW error after a deletion | a frontend cascade like `EMPTY_CELL` was missed | delete the newly-orphaned declaration in the same commit |
| a SolidJS effect stops firing | F3.1 was deleted instead of rewritten | apply Section 4.5 |
| the `local_config_io` guard test fails | the `write_team_config` move landed outside `#[cfg(test)] mod tests` | re-check Section 5.2 step 2 |

## 7. Compatibility, performance, security

- **IPC**: unchanged in both directions. No Tauri command is registered or unregistered, no event name is added or removed, no `#[serde(rename_all)]` struct changes shape, and no `src/shared/types.ts` interface that crosses the boundary is touched. `AppConfig`, `ShellProfile`, `SessionGroup` and `PhoneConversation` are frontend-only types with zero references anywhere in the repo, so no Rust counterpart exists to drift from.
- **Persistence and migration**: none. No TOML or JSON schema, no on-disk layout, no config key.
- **Binary**: `deliver_wake_via_api` and the other Class A items are removed from the build. The Class B demotions move code from the production binary into `#[cfg(test)]`, so the release binary shrinks slightly. No behaviour depends on it.
- **`tauri-build` in `[dependencies]`**: removing it does not change what is compiled. `build.rs:28` resolves `tauri_build::build()` through `[build-dependencies]:77`. Nothing under `src-tauri/src` references `tauri_build`.
- **npm dependencies**: `@tauri-apps/plugin-shell` has no JS import and no Rust counterpart (`tauri_plugin_shell` appears nowhere in `src-tauri/` or `Cargo.toml`). `@types/dompurify` is redundant because `dompurify@3.4.2` ships its own types. Both removals are lockfile-affecting and are verified by `npm ci && npm run typecheck && npm run build`.
- **Performance**: no runtime path changes. `EMPTY_CELL` is a module-level constant object that is no longer allocated; immaterial.
- **Security**: no credential handling, no network, no auth, no path validation, and no trust boundary is touched. The `local_config_io` scraping guard, which is a security-adjacent invariant, is explicitly re-verified (Section 5.2 step 5).
- **Windows and ConPTY**: no PTY code path, no `cmd.exe /C` wrapping, and no path handling is touched. The two `#[cfg(windows)]` edits (R3.1, R3.2) remove an import only; `creation_flags` continues to come from `tokio::process::Command`.

## 8. Implementation order

Batches are ordered by increasing risk, and **each batch is a separate commit** that must pass its own gate before the next one starts. If a late batch has to be reverted, the earlier ones survive.

### MVP: risk-free, verifiable in isolation

**Batch 1, frontend orphan files (F1).** Delete the 7 files. Gate: `npm run typecheck`, `npm test`.

**Batch 2, frontend symbols and unused declarations (F2 + F3).** Includes the `EMPTY_CELL` companion, the `AgentPickerModal` rewrite, and the `mountModal` signature edit. Gate: `npm run typecheck`, `npx tsc --noEmit --noUnusedLocals --noUnusedParameters` reporting 0 errors, `npm test`.

**Batch 3, Rust Class A deletions (R1, all 10 plus companions).** Every item in this batch is closed by Rule 1, so it is the largest batch with the smallest risk. Gate: from `src-tauri`, `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib --bins --tests`.

### Full features: the batches that touch tests

**Batch 4, Rust Class B demotions (R2.1, R2.2).** Two moves. Gate: same three Rust commands, plus an explicit check that the test count did not drop (Section 9.2).

**Batch 5, Rust signal cleanup (R3.1 to R3.9).** Imports, the six `allow` removals, and the `Cargo.toml` line. This batch is last among the Rust batches because Rule 4 is the only place where a green Windows CI can hide a Linux failure. Gate: same three Rust commands, plus the Section 9.3 non-Windows inspection.

### Polish

**Batch 6, documentation (F5) and dependencies (F4).** Gate: `npm ci && npm run typecheck && npm run build`, and a read of the rendered mermaid block confirming no orphan node.

### Extras

None. Nothing beyond Sections 5.1 to 5.5 is authorised.

## 9. Tests and objective acceptance criteria

### 9.1 No new test is written

This change removes unreachable code. A test asserting that deleted code is absent would be a tautology. The verification instruments are the compiler, the linter, and the existing suites, whose pass/fail is objective.

### 9.2 Test-count invariant (the load-bearing check for batch 4)

Before batch 4, record the baseline: run `cargo test --lib --bins --tests` from `src-tauri` and save the summary line (`test result: ok. N passed; ...`) for each target. After batch 4, `N` must be **identical** for every target. A drop means a test was lost, which contradicts Section 4.2 and must be fixed, not accepted.

Equivalently for the frontend, `npm test` must report the same number of passing tests before and after batches 1, 2 and 6, allowing for zero change (the F3 edits modify test bodies but delete no test case).

### 9.3 Non-Windows inspection record (the load-bearing check for the platform risk)

Before opening the PR, produce a short written record, in the PR description or a commit message, stating for each Rust item which rule from Section 6.2 closed it and what was observed. The record is the deliverable, because no PR job can produce it. Minimum content:

- R1.1 to R1.10: Rule 1, with the `rg` count observed (must be 1 before each deletion).
- R2.1, R2.2: Rule 2, with the reference list and the `#[cfg(test)]` range that contains it.
- R3.1, R3.2: Rule 3, naming the enclosing `#[cfg(windows)]` block line.
- R3.3 to R3.8: Rule 4, naming at least one ungated production site that keeps each covered item live.
- R1.7c, R1.10c3: Rule 5, with the in-file occurrence list for the removed import.

### 9.4 Verification commands

Frontend, from the repository root:

```
npx tsc --noEmit
npx tsc --noEmit --noUnusedLocals --noUnusedParameters
npm test
npm run test:debt
npm ci && npm run typecheck && npm run build
```

Rust, from `src-tauri` (matching what CI runs):

```
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --lib --bins --tests
```

Rust, wider than CI (matching the stated baseline):

```
cargo clippy --workspace --all-targets --all-features
```

Diff hygiene:

```
git diff --check
git diff --stat
```

### 9.5 Objective acceptance criteria

Accepted only when every statement below is true.

1. `npx tsc --noEmit` exits 0 with `No errors found`.
2. `npx tsc --noEmit --noUnusedLocals --noUnusedParameters` reports **0 errors** (baseline: 9).
3. `npm test` passes, with the same number of test cases as baseline.
4. `npm run test:debt` passes.
5. `npm ci && npm run typecheck && npm run build` passes after F4, and `package-lock.json` is committed.
6. From `src-tauri`: `cargo check --all-targets` clean, `cargo clippy --all-targets -- -D warnings` clean, `cargo test --lib --bins --tests` passes.
7. `cargo clippy --workspace --all-targets --all-features` finishes with **zero warnings**.
8. The `cargo test` per-target pass counts are identical to baseline (Section 9.2). No test file is deleted. No assertion is changed except the mechanical edits F3.2, F3.4, F3.5.
9. The Section 9.3 inspection record exists and covers every Rust item.
10. No file listed in Section 3.2 is modified. In particular `src-tauri/src/config/seed_manifest.rs` is byte-unchanged, `src-tauri/src/phone/mailbox.rs` is byte-unchanged, `src-tauri/src/commands/role_templates.rs` is byte-unchanged, and `src/sidebar/components/AcDiscoveryPanel.tsx` still exists.
11. `docs/reference/architecture.md` contains no reference to `SessionList.tsx` or `Toolbar.tsx`, and its section 3.1 mermaid graph has no node without an incoming edge from `SA` other than `SA` itself.
12. `rg -w` for each deleted Rust symbol returns 0 hits repo-wide; `rg -w` for each deleted frontend symbol returns 0 hits under `src/`.
13. No Tauri command, event name, IPC type, wire field, schema, migration, dependency (beyond the three removals), background task, network call, security boundary, or CSS rule is added or changed.
14. `git diff --stat` touches only the paths named in Section 5 plus `plans/1177-remove-dead-code.md` and `package-lock.json`.

## 10. Notes for the implementer

- Work bottom-up inside each file. Every coordinate in Section 5 is from `fae6b09` and earlier edits shift later lines.
- Anchor on the symbol, not the number. If the symbol at a coordinate is not the one named, stop and report; do not delete by line number.
- Never resolve a warning by adding `#[allow(...)]`. This issue removes false lint signal; adding more would invert its purpose.
- `phone/manager.rs` (the dead feature, #1179) and `phone/mailbox.rs` (the live CLI messaging system, ~20k LOC) are different modules with confusable names. Neither is touched by this plan.
- In `commands/ac_discovery.rs` and `commands/entity_creation.rs`, near-homonyms exist (`sync_workgroup_repos` at `:3678` versus `sync_workgroup_repos_inner` at `:3474`; `ensure_project_context_templates` versus `..._with_publications` and `..._with_clock`). None of them is in this plan's scope; they are named here only so a wide search does not lead anywhere by accident.
- The source inventories are `__agent_dev-rust/DEAD-CODE-RUST-20260731.md`, `__agent_dev-webpage-ui/DEAD-CODE-FRONTEND-20260731.md`, and their revalidations against `fae6b09`. The revalidations supersede the originals. Section 5 of this plan supersedes both, because every coordinate in it was re-read from the tree.

## 11. Decisions (all closed)

1. **`deliver_wake_via_api`**: CLOSED as DELETE, together with `build_inline_wake_message`, `DeliveryOutcome` and the `OutboxMessage` import. Reasoning in Section 4.4. It is not reserved as a fallback, and it could not serve as one without giving up the durability, retry, leasing and purge-guard guarantees the queue migration was made to obtain.
2. **`wait_for_restore_or_session` + `RestoreWaitOutcome`**: CLOSED as OUT OF SCOPE. It is a retained executable specification whose D.5a tests are the only automated coverage of the inlined loop in `handle_close_session` on the only platform CI compiles. Recommended follow-up: un-`#[ignore]` D.5b first, then delete. Section 3.2.
3. **Class B treatment (`read_task_fields_at`, `write_team_config`)**: CLOSED as demote-to-test-scope, not delete-with-tests. Their inline callers cover live production behaviour. Section 4.2.
4. **`AgentPickerModal.tsx:371`**: CLOSED as rewrite to `selectedScope();`, not deletion. Deleting the line would unsubscribe a SolidJS effect and change behaviour. Section 4.5.
5. **`WorkgroupGroupsModal.nonstop.test.tsx:20`**: CLOSED as remove-the-parameter plus update both call sites. Underscore-prefixing is rejected. Section 4.6.
6. **`docs/reference/architecture.md`**: CLOSED as a rewrite of five mermaid lines and two table rows, not a deletion of two lines. Deleting only the two stale lines orphans three live components in the diagram. Sections 4.3 and 5.5.
7. **`EMPTY_CELL`**: CLOSED as delete alongside `profileCellOrDefault`. Leaving it fails the issue's own `--noUnusedLocals` acceptance criterion.
8. **Batch granularity**: CLOSED as six commits ordered by increasing risk, each with its own gate. Section 8.
9. **Platform verification**: CLOSED as the five-rule protocol in Section 6.2, with a written record required by criterion 9. CI cannot substitute for it.
10. **New tests**: CLOSED as none. Section 9.1.

No implementation decision remains open.

## 12. Scope corrections to issue #1177

These should be applied to the issue body before implementation starts, so the issue and the plan agree.

1. **Frontend item 1 header is wrong.** It says "Delete 8 orphan component files (878 LOC)" and then lists 7, correctly excluding `AcDiscoveryPanel.tsx`. The real figure is **7 files**. The 878 LOC total includes `AcDiscoveryPanel.tsx` (419); the remainder is 459 by the inventory's own metric, or 507 physical lines as counted here. Suggested wording: "Delete 7 orphan component files (459 LOC by inventory count, 507 physical lines)".
2. **Rust item 7 must drop `wait_for_restore_or_session` + `RestoreWaitOutcome`.** Section 3.2 gives the reasoning. That removes 73 LOC from the issue's stated volume and leaves item 7 with three entries.
3. **Rust item 7's framing is wrong for the two survivors.** `read_task_fields_at` and `write_team_config` are not "delete the symbol and its inline tests". Their tests cover live production code through them. The correct instruction is "move into `#[cfg(test)] mod tests` and drop the `#[allow(dead_code)]`". Section 4.2.
4. **Rust item 6 is missing three companion edits.** Deleting `deliver_wake_via_api` also requires deleting `build_inline_wake_message`, `DeliveryOutcome` and the `OutboxMessage` import, or `-D warnings` fails. Deleting `_manifest_path_for_docs` also requires dropping `agency_manifest_path` from the `use` at `cli/agency_templates.rs:11`.
5. **Frontend item 2 is missing one companion edit.** Deleting `profileCellOrDefault` orphans the module-private `EMPTY_CELL` (`profile-utils.ts:26`), which `--noUnusedLocals` then reports, contradicting the issue's own acceptance criterion.
6. **Frontend item 3 is mischaracterised.** "Remove 9 unused locals/imports" covers 5 imports, 2 locals, 1 function declaration, and 1 function **parameter**. Two of the nine are not removals: `AgentPickerModal.tsx:371` must be rewritten, not deleted (it would change SolidJS reactivity), and `WorkgroupGroupsModal.nonstop.test.tsx:20` is a signature change affecting two call sites.
7. **Frontend item 5 understates the doc work.** Deleting the two stale lines in `docs/reference/architecture.md` orphans `SessionItem`, `SettingsModal` and `OpenAgentModal` in the mermaid graph. Five lines plus two table rows must be rewritten. Section 5.5.
8. **Acceptance criteria: `cargo` commands do not match CI.** The issue asks for `cargo check --workspace --all-targets --all-features`; CI runs `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` from `src-tauri`, with neither `--workspace` nor `--all-features`. Both should be listed, because the wider form is the stated baseline and the narrower form is the actual gate.

Status: DRAFT (Step 4).
