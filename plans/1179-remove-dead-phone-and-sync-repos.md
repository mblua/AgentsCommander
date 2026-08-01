# Implementation Plan: #1179 Remove the dead `phone` feature and the unreachable `sync_workgroup_repos` command

**Status:** Step 4 draft, authored by `architect`. **NOT certified `READY_FOR_IMPLEMENTATION`.**
Certification happens at Step 7, after enrichment by `dev-rust` (Step 5) and `dev-rust-grinch` (Step 6).

**Baseline commit:** `d7285ceb7bda5259e370cc25433d1aa3293c8628` (`d7285ce`)
**Branch:** `chore/1179-remove-dead-phone-and-sync-repos`, branched from `main` @ `d7285ce`
**Issue:** https://github.com/mblua/AgentsCommander/issues/1179

Every line coordinate in this plan was read directly out of `d7285ce` by the author. Where this plan and the issue body disagree, **this plan wins**; the disagreements are enumerated in Section 12.

---

## 1. Issue, baseline, and objective

### 1.1 Objective

Remove two independently verified, unreachable IPC surfaces from the codebase:

1. The **`phone` feature**: four `#[tauri::command]`s, their manager module, three exclusive types, the TypeScript wrapper and interfaces, and the documentation that describes them.
2. The **`sync_workgroup_repos` command**: a 32-line `#[tauri::command]` wrapper with zero callers.

Both are dead end to end. Nothing in the app, the CLI, the web dispatcher, the control-plane API, the tests, or the frontend reaches either of them.

### 1.2 What this change is not

This is a **deletion of unreachable code**. It is not a refactor, not a behaviour change, and not a feature removal in the user-visible sense: no user can currently reach any of the removed surface, so nothing a user can do today stops working.

The one live thing that changes is **documentation accuracy**: five documentation locations currently describe a `conversations/` store that only the deleted code writes. Those are corrected, not merely deleted, wherever they carry a claim that is still true of the live system.

### 1.3 Baseline state, verified

```
$ git rev-parse HEAD
d7285ceb7bda5259e370cc25433d1aa3293c8628
$ git branch --show-current
chore/1179-remove-dead-phone-and-sync-repos
$ git status --porcelain
(empty)
```

Baseline sizes of every file this plan touches, so the implementer can confirm they are working against the same bytes:

| File | Lines at `d7285ce` | Lines after this change |
| --- | ---: | ---: |
| `src-tauri/src/commands/phone.rs` | 33 | **deleted** |
| `src-tauri/src/phone/manager.rs` | 212 | **deleted** |
| `src-tauri/src/commands/mod.rs` | 23 | 22 |
| `src-tauri/src/phone/mod.rs` | 5 | 4 |
| `src-tauri/src/phone/types.rs` | 913 | 883 |
| `src-tauri/src/lib.rs` | 3594 | 3589 |
| `src-tauri/src/commands/entity_creation.rs` | 7826 | 7793 |
| `src/shared/ipc.ts` | 1252 | 1235 |
| `src/shared/types.ts` | 1463 | 1445 |
| `docs/reference/architecture.md` | 738 | 725 |
| `PRIVACY.md` | 54 | 54 |
| `docs/agents/inter-agent-messaging.md` | 218 | 214 |
| `docs/security.md` | 121 | 121 |

These counts are an **acceptance check**, not decoration. See Section 9.4.

---

## 2. Evidence and current-state gap

### 2.1 How the "dead" claim was established

Three independent passes agree, all run with `git grep` **against the commit object** (`git grep ... d7285ce`), never against a working tree:

1. `dev-rust`, report `20260801-140110-wg9-dev-rust-to-wg9-tech-lead-report-1179-rust-revalidation.md`, Rust side.
2. `dev-webpage-ui`, report `20260801-135703-wg9-dev-webpage-ui-to-wg9-revalidate-1179-ts-coords.md`, TypeScript and docs side.
3. This plan's author, re-running every load-bearing grep from scratch plus the checks in Section 2.3 that neither report covered.

### 2.2 The dead chain, verified

```
src/shared/ipc.ts:807  PhoneAPI                          <- 0 callers repo-wide
  -> commands/phone.rs   4 #[tauri::command]
       phone_send_message (:6)   registered lib.rs:2612
       phone_get_inbox    (:17)  registered lib.rs:2613
       phone_list_agents  (:22)  registered lib.rs:2614
       phone_ack_messages (:28)  registered lib.rs:2615
    -> phone/manager.rs   4 pub fn + 5 private helpers   <- only caller: commands/phone.rs:2
    -> phone/types.rs:779-808   PhoneMessage, Conversation, AgentInfo  <- only consumers: the two files above
```

```
src/shared/ipc.ts:1072  syncWorkgroupRepos                <- 1 repo-wide hit: its own definition
  -> commands/entity_creation.rs:3666  sync_workgroup_repos
       registered lib.rs:2667
```

Author's own re-verification, exact output:

```
$ git grep -n -E 'phone_send_message|phone_get_inbox|phone_list_agents|phone_ack_messages|PhoneAPI' d7285ce
docs/reference/architecture.md:230, :269
src-tauri/src/commands/phone.rs:6, :17, :22, :28
src-tauri/src/lib.rs:2612, :2613, :2614, :2615
src/shared/ipc.ts:807, :809, :811, :812, :814
                                            -> 15 hits: 4 definitions, 4 registrations,
                                               5 inside PhoneAPI, 2 doc lines. No consumer.

$ git grep -n -E 'phone::manager|commands::phone|super::manager' d7285ce
src-tauri/src/commands/phone.rs:2       (the single inbound reference to phone::manager)
src-tauri/src/lib.rs:2612 .. :2615      (the four registrations)

$ git grep -n -w syncWorkgroupRepos d7285ce
src/shared/ipc.ts:1072                  (definition only, zero call sites)
```

### 2.3 Four checks neither revalidation report ran, all clean

These close the remaining ways a "dead" claim can be wrong in a Tauri 2.x project.

**a. Tauri capability files.** Tauri 2.x can gate commands through `capabilities/*.json`. If a removed command were listed there, the build would fail or a stale entry would survive.

```
$ git ls-tree -r --name-only d7285ce -- src-tauri/ | grep -v '^src-tauri/src/'
   -> src-tauri/Cargo.toml, src-tauri/build.rs, src-tauri/capabilities/default.json, icons/...
$ git grep -n -i -E 'phone|sync_workgroup' d7285ce -- src-tauri/capabilities/ src-tauri/tauri.conf.json
   -> exit 1, no hits
```
**No capability or config file references either surface. No manifest edit is required.**

**b. Integration tests under `src-tauri/tests/`.** These link `agentscommander_lib` externally and can reach `pub` items the in-crate greps would still find, but they are easy to miss when reasoning about `src/` alone.

```
$ git grep -n -i phone d7285ce -- src-tauri/tests/
cli_task_logger.rs:91           -> crate::phone::messaging::workgroup_root   (LIVE, untouched)
pty_input_cross_process.rs:9    -> agentscommander_lib::phone::types::{canonical_pty_timestamp,
                                   pty_input_request_fingerprint, sha256_hex, PtyInputPublicStatus,
                                   PtyInputReasonCode, PtyInputSourcePlane}  (LIVE, all Block 1)
pty_writer_inventory.rs:74,:119,:187 -> string literals "src/phone/mailbox.rs" (LIVE, untouched)
```
**No integration test touches `phone/manager.rs`, `commands/phone.rs`, or any of the three deleted types.**

**c. Platform gating.** Plan #1177 established that no PR job compiles Rust for Linux or macOS, so a `#[cfg(unix)]`-only dependency can pass every PR gate and break the release build.

```
$ git grep -n 'cfg(' d7285ce -- src-tauri/src/phone/manager.rs src-tauri/src/commands/phone.rs
   -> exit 1, no hits
```
**Neither deleted file contains a single `cfg` attribute. That failure mode does not apply to this change.** This is the one meaningful risk from #1177 that #1179 does not inherit.

**d. `can_communicate` is not stranded.** `architecture.md:687` attributes `can_communicate()` to `phone/manager.rs`. That attribution is wrong and predates this change.

```
$ git grep -n -w can_communicate d7285ce -- src-tauri/
config/teams.rs:1391            <- the actual definition
api/actuation.rs:127, cli/list_peers.rs:673,:708,:802,:934, cli/send.rs:921,
phone/mailbox.rs:10362, config/teams.rs:2644,:2654 (tests), phone/manager.rs:98
```
`phone/manager.rs:98` is one caller among nine. **Deleting it strands nothing.** The inaccurate doc row is deleted as part of this change (Section 5.4), so the error disappears with it.

### 2.4 What the gates can and cannot catch

This is the single most important operational fact in this plan.

| Gate | Runs where | Catches a leftover? |
| --- | --- | --- |
| `cargo check --all-targets` | CI `rust-regression`, windows-latest | Only genuine compile errors |
| `cargo clippy --all-targets -- -D warnings` | CI `rust-regression`, windows-latest | Unused **imports** and private dead code: yes. **`pub` items in `pub mod`s: NO.** |
| `cargo test --lib --bins --tests` | CI `rust-regression`, windows-latest | Only behaviour under test |
| `npm run typecheck` (`tsc --noEmit`) | CI `frontend-regression`, ubuntu | **No.** `tsconfig.json` sets no `noUnusedLocals` / `noUnusedParameters`. |
| `npm test` (`vitest run`) | CI `frontend-regression`, ubuntu | **No.** |
| ESLint / Biome | **does not exist in this repo** | n/a |
| `npm run test:debt` | CI `test-debt`, ubuntu | Ignored/placeholder tests only. Unaffected by this change. |

Two consequences the implementer must internalise:

1. **`agentscommander_lib` is a `lib` crate.** `lib.rs` declares `pub mod commands;` and `pub mod phone;`, and each submodule is `pub mod` in turn, so every `pub` item in them is part of the crate's public surface. **rustc emits no `dead_code` warning for them no matter how few callers they have.** If a deletion strands another `pub` item, clippy will not say so, on any platform. `git grep` against the baseline commit is not a secondary check here; it is the **only** check.
2. **The frontend has no unused-symbol gate at all.** If the orphaned imports (`ipc.ts:28-29`) or the orphaned interfaces (`types.ts:981-996`) are left behind, `npm run typecheck` and `npm test` both pass. Their removal is mandatory and is verified by grep, not by a build.

### 2.5 The current-state gap this change closes

| Gap | Today | After |
| --- | --- | --- |
| IPC surface | 5 registered commands no client can reach | 0 |
| Rust | ~245 physical lines of unreachable code across 2 files plus 30 lines of exclusive types | removed |
| TypeScript | 1 unreachable API object, 1 unreachable wrapper, 2 orphan interfaces, 2 orphan imports | removed |
| Docs | 5 locations describe a `conversations/` store; after the deletion nothing writes it | corrected or removed |

---

## 3. Scope

### 3.1 In scope

**Rust**

| # | File | Action |
| --- | --- | --- |
| R1 | `src-tauri/src/commands/phone.rs` | delete the file (33 lines) |
| R2 | `src-tauri/src/phone/manager.rs` | delete the file (212 lines) |
| R3 | `src-tauri/src/commands/mod.rs:9` | delete `pub mod phone;` |
| R4 | `src-tauri/src/phone/mod.rs:3` | delete `pub mod manager;` |
| R5 | `src-tauri/src/lib.rs:2612-2615` | delete the 4 `invoke_handler` registrations |
| R6 | `src-tauri/src/phone/types.rs:779-808` | delete `PhoneMessage`, `Conversation`, `AgentInfo` (30 lines) |
| R7 | `src-tauri/src/commands/entity_creation.rs:3664-3696` | delete the `sync_workgroup_repos` wrapper (33 lines) |
| R8 | `src-tauri/src/lib.rs:2667` | delete the `sync_workgroup_repos` registration |
| R9 | `src-tauri/src/lib.rs:1254` | drop the stale `sync_workgroup_repos` mention from the comment |

**TypeScript**

| # | File | Action |
| --- | --- | --- |
| T1 | `src/shared/ipc.ts:806-815` | delete `PhoneAPI` |
| T2 | `src/shared/ipc.ts:28-29` | delete the orphaned `PhoneMessage,` and `AgentInfo,` import entries |
| T3 | `src/shared/types.ts:981-998` | delete `export interface PhoneMessage` and `export interface AgentInfo` |
| T4 | `src/shared/ipc.ts:1071-1075` | delete `syncWorkgroupRepos` |

**Documentation**

| # | File | Action |
| --- | --- | --- |
| D1 | `docs/reference/architecture.md` | 17 line-level edits across 5 sections (Section 5.4) |
| D2 | `PRIVACY.md:29-31` | retarget at the live `messaging/` directories; exact wording in Section 4.2 |
| D3 | `docs/agents/inter-agent-messaging.md:129-132` | delete the `## Conversation files` section |
| D4 | `docs/security.md:14` | drop `conversations,` from the disk-contents enumeration |

**D3 and D4 are not in the issue body.** They are added by this plan; the justification is in Section 12.2.

### 3.2 Out of scope, each with a reason

| Item | Why it stays |
| --- | --- |
| `src-tauri/src/lib.rs:11` `pub mod phone;` | The `phone/` module keeps `consumption`, `mailbox`, `messaging`, `types`. Removing it breaks the entire live CLI messaging system. |
| `phone/mailbox.rs`, `phone/messaging.rs`, `phone/consumption.rs` | All live. `mailbox.rs`'s only `crate::phone::` imports are `consumption::{verdict_to_result, ConsumptionVerdict}` and `types::OutboxMessage`, neither of which this change touches. |
| `phone/types.rs:1-777` and `:809-913` | Lines 1-777 are the live PTY-input protocol plus `OutboxMessage` (`:621`), consumed across `api/`, `web/`, `cli/`, `pty/` and `mailbox.rs`. Lines 809-913 are its `#[cfg(test)] mod tests`, which exercises only `OutboxMessage` and `PtyInput*`. |
| `phone/types.rs:4` `use serde::{Deserialize, Serialize};` | The `PtyInput*` types still need both. Removing it is a compile error. |
| `sync_workgroup_repos_inner` (`entity_creation.rs:3462`) | Live caller at `:3399` inside `update_team`. |
| `SyncResult` (`:91`), `SyncError` (`:84`) | `SyncResult` is still constructed at `:3471` and returned at `:3470`; `SyncError` is pushed at `:3598`. Their `#[derive(Serialize)]` and `#[serde(rename_all = "camelCase")]` become unnecessary once they leave the IPC surface, but they are harmless, emit no warning, and removing them is scope creep. **Leave the derives alone.** |
| `entity_creation.rs:3629` log string `"[entity_creation] sync_workgroup_repos for '{}': ..."` | It labels the inner helper, is user-invisible, and renaming it is unrelated churn. It is a known, accounted-for survivor of the acceptance grep (Section 9.5). |
| `entity_creation.rs:39` `pub struct AgentInfo` and `list_all_agents` | A different, live struct. See the homonym table in Section 6.3. |
| `docs/reference/architecture.md:34`, `:53`, `:60` | `PH["phone/<br/>Inter-agent messaging"]` describes the **directory**, which survives. `:60` `PH <-->\|"JSON files"\| FS` also stays: the live outbox writes `<file>.json` under `<replica>/<local-dir>/outbox/`, so `phone/` still exchanges JSON files with the filesystem. |
| `docs/agents/teams-and-workgroups.md:161` "Messages, `TASK.md`, and conversations are preserved." | **Decided out.** The sentence is about closing a workgroup, where the *workgroup directory* stays on disk. The deleted `conversations/` store lives under `<config-dir>`, not under the workgroup, so this sentence was never describing it. This change does not make the sentence false, and editing it would be an unrequested wording change. |
| `CHANGELOG.md` | No entry. See Section 4.5. |
| Missing `phone/mailbox.rs`, `phone/messaging.rs`, `phone/consumption.rs` rows in the `architecture.md` §10 file index | A pre-existing documentation gap, unrelated to this change. |
| `architecture.md` rows for `commands/dark_factory.rs` (`:281`, `:699`) | `commands/dark_factory.rs` does not exist in `commands/mod.rs`. A pre-existing inaccuracy, unrelated. |

### 3.3 Explicit non-goals

- No new tests. Nothing testable is added, and the deleted code has no tests to relocate (Section 9.1).
- No new dependency, no dependency removal, no module restructuring.
- No change to `sync_workgroup_repos_inner`'s behaviour, signature, or call site.
- No renaming of the surviving `phone/` module or its files.

---

## 4. Decided solution

Five decisions are the architect's, not the implementer's. All are closed here. Nothing in this plan is left to judgement at implementation time.

### 4.1 Decision: batching and commit order (four batches, four commits)

**Rationale.** Past experience in this repo is that a session can be lost during a long `cargo` gate, and a commit survives that where a working tree does not. So every batch closes with a commit, and the expensive Rust gate runs once per Rust batch rather than once for everything.

| Batch | Content | Gate cost | Commit message |
| --- | --- | --- | --- |
| **1** | TypeScript: T1, T2, T3, T4 | seconds | `chore(#1179): remove the dead PhoneAPI and syncWorkgroupRepos TS surface (batch 1)` |
| **2** | Rust, phone chain: R1-R6 | full Rust gate | `chore(#1179): delete the dead phone command chain and its exclusive types (batch 2)` |
| **3** | Rust, sync wrapper: R7, R8, R9 | full Rust gate | `chore(#1179): remove the unreachable sync_workgroup_repos command (batch 3)` |
| **4** | Docs: D1, D2, D3, D4 | greps only | `docs(#1179): drop the removed phone surface and retarget the conversations references (batch 4)` |

**Why TypeScript first, before Rust.** The two halves have no compile-time dependency, so the order is a coherence choice, not a correctness one. TypeScript first is strictly better because of what each interrupted state looks like:

- *TS landed, Rust not landed:* the backend registers four commands nobody invokes. That is exactly today's situation minus the frontend half. Fully coherent, compiles, no regression.
- *Rust landed, TS not landed:* `PhoneAPI` invokes command names that no longer exist. `tsc` still passes (the names are string literals) and nothing calls it, so it is inert, but the repo is momentarily in a state that would throw at runtime if anything ever did.

TypeScript first also costs seconds, so it de-risks the frontend half before the first long `cargo` run.

**Why batches 2 and 3 are separate.** Different files, different modules, no shared symbol. The issue and both revalidations independently confirm they are fully independent. Separating them costs one extra Rust gate run and buys two clean revert units and a failure in one that cannot force redoing the other.

**Why batch 2 cannot be split further: this is a hard constraint.** R1 through R6 are a single atomic unit. Every intermediate state fails to compile:

- R1 without R3 -> `E0583: file not found for module 'phone'`
- R2 without R4 -> `E0583: file not found for module 'manager'`
- R6 without R1 and R2 -> `E0432: unresolved imports` at `commands/phone.rs:3` and `phone/manager.rs:3`
- R1 without R5 -> `E0433`/unresolved path at `lib.rs:2612-2615`

**Do not run `cargo check` between the six edits of batch 2. It will fail, and that failure means nothing.** Apply all six, then gate once.

**Why docs are last.** They have no compile gate, so if a session is lost mid-plan the docs batch is the cheapest to redo. Landing it last also means the doc edits are written against a tree where the code deletions are already real, so a documentation claim can be checked against the code rather than against intent.

### 4.2 Decision: the exact `PRIVACY.md` replacement wording

Issue step 11 requires retargeting, not deletion, and leaves the wording to the architect. Here it is, verbatim.

**Current, `PRIVACY.md:29-31`:**

```markdown
### Inter-Agent Messaging (Phone)

The internal messaging system between agents is **entirely local**. Messages are stored as JSON files in `~/.agentscommander/conversations/`. No external network calls are made.
```

**Replacement, `PRIVACY.md:29-31`: use exactly this text, three lines, same position:**

```markdown
### Inter-Agent Messaging

The internal messaging system between agents is **entirely local**. Messages are written as Markdown files to `messaging/` directories on your own disk, inside each workgroup and inside the Root Agent directory. No external network calls are made.
```

**Why this wording, clause by clause:**

| Clause | Backing fact |
| --- | --- |
| Heading drops "(Phone)" | Required by the issue; the feature the qualifier named is gone. |
| "**entirely local** ... No external network calls are made" | Preserved verbatim. This is the privacy guarantee, and it is still true of the live messaging system. Losing it would be a regression in the privacy policy. |
| "Markdown files" | `phone/messaging.rs:199-207` `build_filename` produces `"{}-{}-to-{}-{}.md"`. Not JSON. The current text is wrong on this point too. |
| "`messaging/` directories" | `phone/messaging.rs:11` `pub const MESSAGING_DIR_NAME: &str = "messaging";` |
| "inside each workgroup" | `phone/messaging.rs:160-164` `messaging_dir(wg_root)` -> `wg_root.join(MESSAGING_DIR_NAME)` |
| "and inside the Root Agent directory" | `phone/messaging.rs:166-170` `root_messaging_dir(root_agent_dir)`; also `config/root_agent.rs:722` |
| No `~/.agentscommander/` path | **Deliberate.** Messaging files do **not** live under the config directory; they live in the project workspace. Repeating `~/.agentscommander/` would replace one factual error with another. `PRIVACY.md:5` already makes the separate, still-true claim about configuration and session data living under `~/.agentscommander/`; leave that line alone. |

The replacement is 3 lines for 3 lines, so `PRIVACY.md` stays at 54 lines.

### 4.3 Decision: `docs/agents/inter-agent-messaging.md` is a deletion, not a retarget

Unlike `PRIVACY.md`, this section carries **no guarantee worth preserving**: it is a pure factual claim about an artifact that ceases to exist:

```
129  ## Conversation files
130
131  Beyond `messaging/`, AC also persists a per-peer conversation snapshot at `<config-dir>/conversations/<NNNN>-<from>_<to>.json`. This is a structured copy of the back-and-forth, useful for offline analysis. It is **not** the canonical source — the messaging files are.
132
```

That snapshot is written **only** by `phone/manager.rs::save_conversation` (`:82`, reached from `:105`, `:155`, `:183`). After R2 nothing writes it. There is nothing to retarget the sentence at: the `messaging/` directory it contrasts itself against is already documented in the rest of the file. **Delete lines `129`-`132` (heading, blank, paragraph, blank).** The result reads `:127` paragraph, `:128` blank, then `## Remote logical PTY actions`. File goes 218 -> 214 lines.

### 4.4 Decision: `lib.rs:1254` is edited, not left alone

The issue calls this an "incidental mention" and does not say what to do with it. It must be edited: after R7 the comment names a command that does not exist.

**Current, `lib.rs:1253-1255`:**

```rust
            // Register for Tauri commands that take `State<'_, Arc<GitWatcher>>`
            // (e.g. `update_team`, `sync_workgroup_repos`). Must happen BEFORE the
            // `PtyManager::new(..., git_watcher, ...)` move below.
```

**Replacement: change line `:1254` only, deleting the two tokens `, `sync_workgroup_repos``:**

```rust
            // Register for Tauri commands that take `State<'_, Arc<GitWatcher>>`
            // (e.g. `update_team`). Must happen BEFORE the
            // `PtyManager::new(..., git_watcher, ...)` move below.
```

`update_team` genuinely takes `State<'_, Arc<GitWatcher>>`: it calls `git_watcher.inner()` at `entity_creation.rs:3404`, so the comment stays true and load-bearing. Line count unchanged.

### 4.5 Decision: no `CHANGELOG.md` entry

`CHANGELOG.md` has one section per **release** (`## 0.20.0 – 2026-07-23` is the most recent) with no `Unreleased` section, and its own preamble scopes it to "notable **user-facing** changes". Nothing removed here is user-reachable, so there is no user-facing change to record. Precedent confirms it: #1177, a strictly larger dead-code removal on the same codebase, added no CHANGELOG entry (`git grep -n '1177\|dead code' d7285ce -- CHANGELOG.md` -> exit 1). **Do not touch `CHANGELOG.md`.**

---

## 5. Exact affected surfaces

### 5.0 Two mechanical rules that prevent the most likely implementation error

**Rule A: apply multiple cuts in one file bottom-up.** Every coordinate in this section is a `d7285ce` coordinate. Deleting lines shifts everything below. Where a file gets more than one cut, apply the **highest line number first**:

| File | Cut order |
| --- | --- |
| `src/shared/ipc.ts` | `:1071-1075` -> `:806-815` -> `:28-29` |
| `src/shared/types.ts` | single cut |
| `src-tauri/src/lib.rs` | see Rule B |
| `docs/reference/architecture.md` | `:700` -> `:687` -> `:686` -> `:623` -> `:611` -> `:294` -> `:282` -> `:269` -> `:230` -> `:228` -> `:162` -> `:145` -> `:144` -> `:133` -> `:115` -> `:114` -> `:92` |

**Rule B: `lib.rs` is edited across two batches, so batch 3's coordinate has already moved.** Batch 2 (R5) deletes 4 lines at `:2612-2615`. Therefore in batch 3:

- **R8's registration is at `:2663`, not `:2667`.** Locate it by its exact text `commands::entity_creation::sync_workgroup_repos,`, not by line number.
- R9's comment at `:1254` is above every cut and does not move.

Throughout batch 3, prefer locating by exact anchor text over line number. The baseline coordinates in this section are hints for orientation; the anchor text is the contract.

### 5.1 Rust: the phone chain (batch 2, atomic)

**R1. Delete `src-tauri/src/commands/phone.rs` entirely.** 33 physical lines, 29 non-blank. Full content at `d7285ce`, for identity confirmation:

```rust
use crate::config::teams;
use crate::phone::manager;
use crate::phone::types::{AgentInfo, PhoneMessage};

#[tauri::command]
pub async fn phone_send_message(from: String, to: String, body: String, team: String) -> Result<String, String> { ... }   // :6
#[tauri::command]
pub async fn phone_get_inbox(agent_name: String) -> Result<Vec<PhoneMessage>, String> { ... }                            // :17
#[tauri::command]
pub async fn phone_list_agents() -> Result<Vec<AgentInfo>, String> { ... }                                               // :22
#[tauri::command]
pub async fn phone_ack_messages(agent_name: String, message_ids: Vec<String>) -> Result<(), String> { ... }              // :28
```

No `#[cfg(test)]` module, no `#[cfg(...)]` attribute anywhere in the file.

**R2. Delete `src-tauri/src/phone/manager.rs` entirely.** 212 physical lines, 187 non-blank. Contains:

| Kind | Symbol | Line |
| --- | --- | ---: |
| private | `conversations_dir` | 7 |
| **pub** | `list_agents` | 12 |
| private | `scan_files` | 41 |
| private | `next_id` | 62 |
| private | `find_existing` | 67 |
| private | `save_conversation` | 82 |
| **pub** | `send_message` | 90 |
| **pub** | `get_inbox` | 154 |
| **pub** | `ack_messages` | 182 |

Imports: `std::path::{Path, PathBuf}`, `super::types::{AgentInfo, Conversation, PhoneMessage}`, `crate::config::teams::DiscoveredTeam`. No `#[cfg(test)]` module, no `#[cfg(...)]` attribute.

**R3. `src-tauri/src/commands/mod.rs`: delete line `:9`.**

```
   8: pub mod non_stop;
   9: pub mod phone;              <-- DELETE this line only
  10: pub mod project_settings;
```
23 -> 22 lines. Leaving it produces `E0583: file not found for module 'phone'`.

**R4. `src-tauri/src/phone/mod.rs`: delete line `:3`.** The whole file is 5 lines:

```
  1: pub(crate) mod consumption;
  2: pub mod mailbox;
  3: pub mod manager;            <-- DELETE this line only
  4: pub mod messaging;
  5: pub mod types;
```
5 -> 4 lines. Leaving it produces `E0583: file not found for module 'manager'`.

**R5. `src-tauri/src/lib.rs`: delete lines `:2612-2615`.** Contiguous, bounded by unrelated commands on both sides:

```
2611:             commands::spec_board::spec_board_close,
2612:             commands::phone::phone_send_message,      <-- DELETE
2613:             commands::phone::phone_get_inbox,         <-- DELETE
2614:             commands::phone::phone_list_agents,       <-- DELETE
2615:             commands::phone::phone_ack_messages,      <-- DELETE
2616:             commands::voice::voice_transcribe,
```
A clean 4-line cut. **Do not touch `lib.rs:11 pub mod phone;`** or `lib.rs:3 pub mod commands;`.

**R6. `src-tauri/src/phone/types.rs`: delete lines `:779-808` inclusive.** 30 lines: three structs with their derives, plus the blank line at `:808`.

Exact boundaries, read from `d7285ce`:

```
 777: }                                          <- end of pty_input_host_request_fingerprint. KEEP.
 778: (blank)                                    <- KEEP, becomes the separator
 779: #[derive(Debug, Clone, Serialize, Deserialize)]     <-- FIRST DELETED LINE
 780: #[serde(rename_all = "camelCase")]
 781: pub struct PhoneMessage {
 782-788:   id, from, to, team, content, timestamp, status
 789: }
 790: (blank)
 791: #[derive(Debug, Clone, Serialize, Deserialize)]
 792: #[serde(rename_all = "camelCase")]
 793: pub struct Conversation {
 794-797:   id, participants, created_at, messages: Vec<PhoneMessage>
 798: }
 799: (blank)
 800: #[derive(Debug, Clone, Serialize, Deserialize)]
 801: #[serde(rename_all = "camelCase")]
 802: pub struct AgentInfo {
 803-806:   name, path, teams, is_coordinator_of
 807: }
 808: (blank)                                     <-- LAST DELETED LINE
 809: #[cfg(test)]                                <- KEEP
 810: mod tests {                                 <- KEEP
```

After the cut the file reads `:777` `}`, `:778` blank, `:779` `#[cfg(test)]`. **913 -> 883 lines.**

**Keep `:4` `use serde::{Deserialize, Serialize};`**: the `PtyInput*` types in lines 1-777 still need both traits. Removing it is a compile error, not a cleanup.

### 5.2 Rust: `sync_workgroup_repos` (batch 3)

**R7. `src-tauri/src/commands/entity_creation.rs`: delete lines `:3664-3696` inclusive.** 33 lines: doc comment through closing brace, plus the trailing blank.

```
3662: }                                                            <- end of previous fn. KEEP.
3663: (blank)                                                      <- KEEP
3664: /// Sync repo assignments and context tokens from team ...    <-- FIRST DELETED LINE
3665: #[tauri::command]
3666: pub async fn sync_workgroup_repos(
3667-3672:   app, session_mgr, git_watcher, discovery_watcher, project_path, team_name
3673: ) -> Result<SyncResult, String> {
3674-3694:   body: validate_existing_name, selected_workspace_dir, team_dir existence check,
             read_team_config(...).repos, then sync_workgroup_repos_inner(...).await
3695: }
3696: (blank)                                                      <-- LAST DELETED LINE
3697: /// Refresh `is_coordinator` on every live session and ...    <- KEEP
```

**7826 -> 7793 lines.**

**Do not touch:**
- `sync_workgroup_repos_inner` at `:3462`: live caller at `:3399` inside `update_team`.
- `SyncResult` at `:91`: still constructed at `:3471`, still the return type at `:3470`.
- `SyncError` at `:84`: still pushed at `:3598`.
- The log string at `:3629`: inside the inner helper, out of scope by Section 3.2.

**No orphaned import.** Every symbol the deleted wrapper used survives with many other users in the same file:

```
$ git grep -c -w <sym> d7285ce -- src-tauri/src/commands/entity_creation.rs
State 16   GitWatcher 6   DiscoveryBranchWatcher 6   AppHandle 12
validate_existing_name 15   selected_workspace_dir 11   read_team_config 11
SyncResult 4 (-> 3 after the cut)   SyncError 3 (unchanged)
```
**No `use` line in `entity_creation.rs` changes.**

**R8. `src-tauri/src/lib.rs`: delete the registration.** Baseline `:2667`; **after batch 2 it sits at `:2663`.** Locate by exact text:

```
              commands::entity_creation::delete_workgroup,
              commands::entity_creation::sync_workgroup_repos,     <-- DELETE this line only
              commands::role_templates::list_role_templates,
```

**R9. `src-tauri/src/lib.rs:1254`: edit the comment** exactly as specified in Section 4.4. Line count unchanged.

`lib.rs` total across batches 2 and 3: **3594 -> 3589 lines** (4 from R5, 1 from R8).

### 5.3 TypeScript (batch 1)

Apply bottom-up per Rule A: T4, then T1, then T2, then T3 (a different file).

**T4. `src/shared/ipc.ts`: delete lines `:1071-1075` inclusive** (the preceding blank line plus the 4-line entry), inside `EntityAPI`:

```
1069:   deleteWorkgroup: (projectPath: string, workgroupName: string, force?: boolean) =>
1070:     transport.invoke<void>("delete_workgroup", { projectPath, workgroupName, force: force ?? false }),
1071: (blank)                                                                        <-- FIRST DELETED
1072:   syncWorkgroupRepos: (projectPath: string, teamName: string) =>
1073:     transport.invoke<{ workgroupsUpdated: number; replicasUpdated: number; errors: { replica: string; error: string }[] }>(
1074:       "sync_workgroup_repos", { projectPath, teamName }
1075:     ),                                                                          <-- LAST DELETED
1076: };                                                                             <- KEEP, closes EntityAPI
```

Result: `:1070` ends with `),` and is immediately followed by `};`. A trailing comma before the closing brace is valid and matches the existing style elsewhere in the file (`PhoneAPI` had the same shape).

**`syncWorkgroupRepos` orphans no import.** Its return type is an **inline anonymous type**, not a named interface. This closes the issue's open "plus any import entry that becomes orphaned by it": **there is none.** Do not go looking.

**T1. `src/shared/ipc.ts`: delete lines `:806-815` inclusive** (the preceding blank line plus the 9-line block):

```
 805: }                                                                             <- KEEP
 806: (blank)                                                                       <-- FIRST DELETED
 807: export const PhoneAPI = {
 808:   sendMessage: (from: string, to: string, body: string, team: string) =>
 809:     transport.invoke<string>("phone_send_message", { from, to, body, team }),
 810:   getInbox: (agentName: string) =>
 811:     transport.invoke<PhoneMessage[]>("phone_get_inbox", { agentName }),
 812:   listAgents: () => transport.invoke<AgentInfo[]>("phone_list_agents"),
 813:   ackMessages: (agentName: string, messageIds: string[]) =>
 814:     transport.invoke<void>("phone_ack_messages", { agentName, messageIds }),
 815: };                                                                             <-- LAST DELETED
 816: (blank)                                                                        <- KEEP
 817: export const AcDiscoveryAPI = {                                                <- KEEP, do not disturb
```

`PhoneAPI` is a bare `export const` object literal: **no `interface PhoneAPI`, no `type PhoneAPI`, and no API registry, barrel, or aggregate to deregister from.** Verified: `git grep -n 'export \*' d7285ce -- src/` and `git grep -n 'import \* as' d7285ce -- src/` both return exit 1.

**T2. `src/shared/ipc.ts`: delete lines `:28-29`** from the `import type { ... }` block:

```
  26:   RepoMatch,
  27:   BridgeInfo,
  28:   PhoneMessage,          <-- DELETE
  29:   AgentInfo,             <-- DELETE
  30:   AcDiscoveryResult,
```

`ipc.ts` total: **1252 -> 1235 lines** (5 + 10 + 2).

**T3. `src/shared/types.ts`: delete lines `:981-998` inclusive** (both interfaces, the blank between them, and the two trailing blanks):

```
 978: }                                     <- end of the previous interface. KEEP.
 979: (blank)                               <- KEEP
 980: (blank)                               <- KEEP  (this file uses a two-blank-line group separator)
 981: export interface PhoneMessage {       <-- FIRST DELETED LINE
 982-988:   id, from, to, team, content, timestamp, status
 989: }
 990: (blank)
 991: export interface AgentInfo {
 992-995:   name, path, teams, isCoordinatorOf
 996: }
 997: (blank)
 998: (blank)                               <-- LAST DELETED LINE
 999: export interface AcAgentMatrix {      <- KEEP
```

After the cut: `:978` `}`, `:979` blank, `:980` blank, `:981` `export interface AcAgentMatrix {`. The two-blank separator is preserved. **1463 -> 1445 lines.**

`PhoneConversation` is **already gone** from this file (removed by #1177 commit `fdd4e20`). Lines `:1041-1046`, which the original issue named, now hold the **live** `AcWorkgroup` closing brace and the `WorkgroupGroup` body. **Do not go near them.**

### 5.4 Documentation (batch 4)

**D1. `docs/reference/architecture.md`: 17 edits.** Apply bottom-up (Rule A). 13 deletions and 4 label edits; 738 -> 725 lines.

| Line | Current | Action |
| ---: | --- | --- |
| 92 | `        C_PHONE["phone.rs<br/>send, inbox, list, ack"]` | **delete** |
| 114 | `        PH_MGR["manager.rs<br/>can_communicate()<br/>send, inbox, ack"]` | **delete** |
| 115 | `        PH_TYPES["types.rs<br/>PhoneMessage<br/>Conversation, AgentInfo"]` | **edit** -> `        PH_TYPES["types.rs<br/>PTY-input protocol<br/>OutboxMessage"]` |
| 133 | `    BOOTSTRAP --> C_PHONE` | **delete** |
| 144 | `    C_PHONE --> PH_MGR` | **delete** |
| 145 | `    C_PHONE --> CFG_DF` | **delete** |
| 162 | `    style C_PHONE fill:#0f3460,stroke:#53a8b6,color:#fff` | **delete** |
| 228 | `        TYPES["types.ts<br/>Session, AppSettings,<br/>AgentConfig, Team,<br/>PhoneMessage, BridgeInfo..."]` | **edit** -> drop `PhoneMessage, ` so the last fragment reads `<br/>BridgeInfo..."]` |
| 230 | `        IPC["ipc.ts<br/>...<br/>DarkFactoryAPI, PhoneAPI,<br/>DebugAPI, WindowAPI<br/>+ event listeners"]` | **edit** -> drop ` PhoneAPI,` so that fragment reads `<br/>DarkFactoryAPI,<br/>` |
| 269 | `        A10["PhoneAPI<br/>sendMessage, getInbox<br/>listAgents, ackMessages"]` | **delete** |
| 282 | `        R9["commands/phone.rs"]` | **delete** |
| 294 | `    A10 -->\|invoke\| R9` | **delete** |
| 611 | `        CONVDIR["conversations/<br/>NNNN-from_to.json<br/>Phone messages"]` | **delete** |
| **623** | `    style CONVDIR fill:#e94560,stroke:#fff,color:#fff` | **delete. MISSING FROM THE ISSUE. See below.** |
| 686 | `\| \`phone/types.rs\` \| \`PhoneMessage\`, \`Conversation\`, \`AgentInfo\` \|` | **edit** -> `\| \`phone/types.rs\` \| PTY-input protocol types, \`OutboxMessage\` \|` |
| 687 | `\| \`phone/manager.rs\` \| \`can_communicate()\`, \`send_message()\`, \`get_inbox()\`, \`ack_messages()\` \|` | **delete** |
| 700 | `\| \`commands/phone.rs\` \| send, inbox, list, ack \|` | **delete** |

> **`:623` is a defect in the issue's list.** `CONVDIR` is referenced **twice**: the node at `:611` and a `style CONVDIR` directive at `:623`. Deleting only `:611` leaves Mermaid styling a node that was never declared, which either resurrects an empty ghost node in the rendered diagram or errors, depending on the Mermaid version. `mermaid ^11.15.0` is a runtime dependency of this app and these diagrams render in-product. **Both lines must go.**

**Node-reference integrity after the edits: verified in advance, no dangling references remain:**

| Id | Remaining references | Verdict |
| --- | --- | --- |
| `C_PHONE` | `:92`, `:133`, `:144`, `:145`, `:162` -> **all five deleted** | clean |
| `PH_MGR` | `:114`, `:144` -> **both deleted** | clean |
| `A10` | `:269`, `:294` -> **both deleted** | clean |
| `R9` | `:282`, `:294` -> **both deleted** | clean |
| `CONVDIR` | `:611`, `:623` -> **both deleted** | clean |
| `PH_TYPES` | `:115` only, no edges | survives as a label edit; a subgraph member with no edges is valid Mermaid |
| `CFG_DF` | `:120` definition, `:143` `C_DF --> CFG_DF`, `:145` deleted | survives |
| `PH` | `:34`, `:53`, `:60` -> **all kept** | survives; describes the surviving directory |

The `subgraph "phone/"` at `:113-116` keeps `PH_TYPES` and stays valid after `PH_MGR` is removed.

**D2. `PRIVACY.md:29-31`**: replace with the exact three lines given in Section 4.2. 54 lines, unchanged.

**D3. `docs/agents/inter-agent-messaging.md`: delete lines `:129-132`** (heading, blank, paragraph, blank) per Section 4.3. 218 -> 214 lines.

**D4. `docs/security.md:14`**: one-token edit in the threat-model enumeration:

```
Current:  4. **The disk.** Configuration, sessions, teams, conversations, and messages all live as plain files under `~/.agentscommander/` (or the portable instance's `.agentscommander_<suffix>/`).
New:      4. **The disk.** Configuration, sessions, teams, and messages all live as plain files under `~/.agentscommander/` (or the portable instance's `.agentscommander_<suffix>/`).
```

Delete `conversations, ` only. Everything else on the line, including the portable-instance parenthetical, stays byte-identical. 121 lines, unchanged.

---

## 6. Required behaviour, edge cases, and failure behaviour

### 6.1 Required behaviour after the change

| Invariant | How it is guaranteed |
| --- | --- |
| CLI inter-agent messaging keeps working end to end | `phone/mailbox.rs`, `phone/messaging.rs`, `phone/consumption.rs` are byte-identical after the change. `mailbox.rs`'s only `crate::phone::` imports are `consumption::{verdict_to_result, ConsumptionVerdict}` (`:16`) and `types::OutboxMessage` (`:17`); neither is touched. |
| Workgroup repo syncing keeps working | `sync_workgroup_repos_inner` (`:3462`) survives untouched, reached from `update_team` at `:3399`. Only the unreachable wrapper is removed. |
| The privileged PTY-input protocol is unaffected | `phone/types.rs:1-777` and its test module `:809-913` are untouched. The cut is strictly between them. |
| `list_all_agents` and its call sites keep working | It uses `entity_creation::AgentInfo` (`:39`), a different struct. See Section 6.3. |
| The `phone/` module still exists and compiles | `lib.rs:11 pub mod phone;` untouched; `phone/mod.rs` keeps four of its five declarations. |
| `phone/types.rs`'s test module still passes | It exercises only `OutboxMessage` and `PtyInput*`. It references none of `PhoneMessage`, `Conversation`, `AgentInfo`. |
| No Tauri capability or config drift | Neither surface appears in `src-tauri/capabilities/default.json` or `src-tauri/tauri.conf.json` (Section 2.3a). |

### 6.2 Edge cases

| Edge case | Resolution |
| --- | --- |
| Users with an existing `~/.agentscommander/conversations/` directory | The directory is **left on disk untouched**. This change deletes no user data and adds no migration or cleanup. Nothing reads or writes it afterwards; it becomes an inert leftover. This is deliberate: deleting user files during a dead-code cleanup is out of proportion to the change and is not requested. |
| A stale build cache resolving `crate::phone::manager` | Not possible after `cargo check`: the module declaration is gone in the same batch as the file. |
| The four command names appearing in an untracked local `plans/` file | Verification greps run against the **baseline commit**, not the working tree, so plan text cannot pollute results (Section 9.4, Rule 1). |
| `Conversation` also occurring as an English word | It does: `pty/container_paths.rs:245` and `:592` contain "Conversation transcripts are container-ephemeral" inside a user-facing warning string, and `docs/comparison.md:43` uses it in prose. **An unscoped `git grep -w Conversation` will therefore return hits after a correct implementation.** The acceptance grep in Section 9.5 is scoped to `src-tauri/src/phone/` and `src/` precisely for this reason. |
| Mermaid rendering after the diagram edits | Covered by the node-reference integrity table in Section 5.4; no dangling id remains. |

### 6.3 The three homonym traps

**Disambiguate at the definition site. Never by match count. Never by name alone.**

| Trap | The dead one | The live one, which must survive |
| --- | --- | --- |
| **Rust `AgentInfo`** | `phone/types.rs:802`, fields `{ name, path, teams, is_coordinator_of }`. Consumers: `commands/phone.rs:3,:22` and `phone/manager.rs:3,:12,:13,:24`. Both files are deleted, so it is dead. | `commands/entity_creation.rs:39`, fields `{ name, description, path, project_name }`. A completely different struct, used at `:2705`, `:2706`, `:2756`. `entity_creation.rs` has **no** import of `crate::phone::types` at all; its only `phone` references are `crate::phone::messaging::MESSAGING_DIR_NAME` at `:1192` and `:2857`. |
| **`send_message`** | `phone/manager.rs:90`, whose only caller is `commands/phone.rs:13`. | `telegram::api::send_message`, **11 of the 13 repo-wide hits**: `commands/telegram.rs:343`, `loops/non_stop_watchdog.rs:354`, `telegram/api.rs:66,:94,:374`, `telegram/bridge.rs:912,:1139,:1147,:1156,:1164,:1169`. A different module that never resolves to `phone::manager`. |
| **TS `AgentInfo`** | `src/shared/types.ts:991-996`, fields `{ name, path, teams, isCoordinatorOf }`, mirroring the Rust phone struct field for field. Consumers: `ipc.ts:29` (import) and `ipc.ts:812` (`PhoneAPI.listAgents`), both deleted. | `EntityAPI.listAllAgents` (`ipc.ts:1015-1019`) does **not** use the interface. It inlines `{ name: string; description: string; path: string; projectName: string }[]`, mirroring the Rust `entity_creation::AgentInfo`. Deleting the interface cannot affect `list_all_agents` or its call sites. |

A fourth near-homonym, flagged so a wide search does not mislead: **`sync_workgroup_repos` vs `sync_workgroup_repos_inner`.** `git grep -w sync_workgroup_repos` does **not** match `sync_workgroup_repos_inner` (`_` is a word character), so `-w` is a reliable discriminator here. Note that `plans/1177-remove-dead-code.md:625` records the wrapper at `:3678`; **that coordinate is stale**. The real location is `:3666`, and anyone acting on the stale number lands 12 lines inside the body.

### 6.4 Failure behaviour: what each gate failure means

| Symptom | Cause | Fix |
| --- | --- | --- |
| `E0583: file not found for module 'phone'` | R1 applied, R3 skipped | Apply R3 |
| `E0583: file not found for module 'manager'` | R2 applied, R4 skipped | Apply R4 |
| `E0432: unresolved import` at `commands/phone.rs:3` or `phone/manager.rs:3` | R6 applied before R1/R2 | Complete all of batch 2, then gate once |
| Unresolved path at `lib.rs:2612-2615` | R1 applied, R5 skipped | Apply R5 |
| `error[E0412]: cannot find type Serialize` in `phone/types.rs` | `:4` `use serde::...` was removed | Restore it; it is out of scope by Section 3.2 |
| Any compile error in `entity_creation.rs` after R7 | The cut boundary was wrong | Re-check against the `:3662`/`:3663`/`:3697` anchors in Section 5.2 |
| `tsc` error naming `PhoneMessage` or `AgentInfo` | T3 applied, T1 or T2 skipped | Apply the missing one; the correct order is T1/T2 before or with T3 |
| `tsc` and `npm test` **pass** but the grep in Section 9.5 fails | The gates cannot see orphaned TS symbols (Section 2.4) | This is the expected failure signature for a skipped T2 or T3. Apply the missing edit. **A green gate is not evidence here.** |
| An acceptance grep returns a `plans/` hit | The grep was run against the working tree instead of `d7285ce`, or the exclusion was dropped | Re-run per Section 9.4 |

---

## 7. Compatibility, performance, and security

**IPC compatibility.** The Tauri IPC surface shrinks by five commands: `phone_send_message`, `phone_get_inbox`, `phone_list_agents`, `phone_ack_messages`, `sync_workgroup_repos`. All five are unreachable today from every client plane, verified independently for each: the desktop frontend (`src/`), the web dispatcher (`web/commands.rs`, zero case-insensitive `phone` hits), the control-plane API (`api/`), and the CLI (`cli/`). **No client contract is broken because no client holds one.**

**Rust API compatibility.** `agentscommander_lib` is a library crate, so removing `pub` items is technically a breaking change for any external consumer. There is none: the crate is consumed only by this workspace's own binaries and by `src-tauri/tests/`, and Section 2.3b confirms no test reaches the removed surface.

**Persistence and data.** Nothing writes `<config-dir>/conversations/` after this change. Existing directories are left untouched (Section 6.2). No schema, no migration, no config-format change. The live `messaging/` directories, the outbox `.json` files, `settings.json`, `sessions.json`, and `teams.json` are all unaffected.

**Performance.** Neutral. Five fewer `invoke_handler` entries is not a measurable difference. Marginally smaller binary.

**Security.** Net positive, though small. Each registered Tauri command is reachable from any page loaded in the webview; removing five unreachable ones removes five entry points. Of the five, `phone_send_message` is the one that actually mattered: it wrote attacker-influenced content to `<config-dir>/conversations/*.json` with a `can_communicate` check as the only gate, and nothing in the product ever needed it. Attack surface strictly shrinks; no permission, no authentication path, and no capability grant changes.

**Privacy documentation.** `PRIVACY.md` retains its guarantee that inter-agent messaging is entirely local with no external network calls, now pointed at the store that actually exists. The guarantee is preserved, not weakened, and it is now true rather than accidentally true.

---

## 8. Implementation order

Phase order per the planning rules: MVP -> Full Features -> Polish -> Extras. This change is small enough that MVP and Full Features are the same four batches; Polish and Extras are empty by design.

### MVP / Full Features

**Batch 1: TypeScript.** Edits T4, T1, T2 in `src/shared/ipc.ts` (in that order, bottom-up), then T3 in `src/shared/types.ts`.
Gate: `npm run typecheck`, `npm test`, plus the batch-1 greps in Section 9.4.
Commit: `chore(#1179): remove the dead PhoneAPI and syncWorkgroupRepos TS surface (batch 1)`

**Batch 2: Rust, the phone chain. Atomic.** Apply **all six** of R1, R2, R3, R4, R5, R6 before running anything. Intermediate states do not compile and their failures carry no information.
Gate: from `src-tauri/`: `cargo check --all-targets`, then `cargo clippy --all-targets -- -D warnings`, then `cargo test --lib --bins --tests`. Plus the batch-2 greps in Section 9.4.
Commit: `chore(#1179): delete the dead phone command chain and its exclusive types (batch 2)`

**Batch 3: Rust, `sync_workgroup_repos`.** Apply R7, then R8 (**locate by anchor text; the line is now `:2663`, not `:2667`**), then R9.
Gate: the same three `cargo` commands, plus the batch-3 greps.
Commit: `chore(#1179): remove the unreachable sync_workgroup_repos command (batch 3)`

**Batch 4: Documentation.** Apply D1 bottom-up (17 edits), then D2, D3, D4.
Gate: the batch-4 greps and the line-count check in Section 9.4. No build gate exists for documentation; the greps are the gate.
Commit: `docs(#1179): drop the removed phone surface and retarget the conversations references (batch 4)`

### Polish

None. There is no follow-up cleanup this change defers.

### Extras

None. Everything the issue asks for lands in the four batches above.

### Rules that hold across all batches

1. **Commit at the end of every batch, before starting the next.** A lost session costs at most one batch.
2. **Never fix a gate failure by widening the scope.** Every legitimate failure has a listed cause in Section 6.4. A failure that is not on that list is new information: stop and report it rather than improvising a fix.
3. **Do not reformat.** No `cargo fmt` over untouched regions, no prettier pass, no import reordering. Every edit in Section 5 is a deletion or a named-token replacement.
4. **Run every verification grep against `d7285ce`,** never the working tree (Section 9.4, Rule 1).

---

## 9. Tests and objective acceptance criteria

### 9.1 No new test is written, and none is deleted

The deleted code has no test of any kind: `commands/phone.rs` and `phone/manager.rs` contain no `#[cfg(test)]` module (verified by reading both files in full), no integration test under `src-tauri/tests/` reaches them (Section 2.3b), and no frontend test mentions any phone symbol:

```
$ git grep -n -i phone d7285ce -- '*.test.ts' '*.test.tsx' '*.spec.ts' '*.d.ts' '*.stories.tsx'
   -> exit 1, no hits
```

`phone/types.rs`'s test module at `:809-913` survives untouched and must keep passing; it exercises only `OutboxMessage` and `PtyInput*`. `npm run test:debt` is unaffected: no ignored or placeholder test is added or removed.

### 9.2 Verification method, and why it is grep-first

Per Section 2.4, neither the Rust nor the TypeScript gate can see a leftover `pub` item or an orphaned TS symbol. The build gates prove the tree still compiles; **the greps prove the deletion was complete.** Both are required. Neither substitutes for the other.

### 9.3 The six verification rules

1. **Run every grep against the baseline commit**: `git grep ... d7285ce`, not the working tree. Running against the working tree makes this plan file itself a match and turns every criterion into a false positive.
2. **Use `-w` (word-regexp) for every symbol name.** Substring matching produces `sync_workgroup_repos` -> `sync_workgroup_repos_inner` and `SessionGroup` -> `TeamSessionGroup` style false positives.
3. **Scope by pathspec where a name has an English-word homonym.** Specifically `Conversation` (Section 6.2).
4. **Disambiguate at the definition site.** For the three traps in Section 6.3, reading the struct body is the check. Match count is not.
5. **`plans/` is excluded from every criterion.** Plan prose legitimately names removed symbols, both this plan and `plans/1177-remove-dead-code.md`.
6. **A passing build is never evidence that a TS or `pub` Rust symbol was removed.** Only the grep is.

### 9.4 Verification commands, per batch

All greps are run from the repo root against the baseline commit.

**After batch 1 (TypeScript):**

```bash
npm run typecheck                 # must exit 0
npm test                          # must pass

# Completeness. Each must return exit 1 (no hits):
git grep -n -w PhoneAPI            d7285ce -- src/
git grep -n -w PhoneMessage        d7285ce -- src/
git grep -n -w AgentInfo           d7285ce -- src/
git grep -n -w syncWorkgroupRepos  d7285ce -- src/
git grep -n 'phone_'               d7285ce -- src/
git grep -n 'sync_workgroup_repos' d7285ce -- src/

# Nothing live was collateral damage. Each must still return hits:
git grep -n -w AcWorkgroup         d7285ce -- src/shared/types.ts
git grep -n -w WorkgroupGroup      d7285ce -- src/shared/types.ts
git grep -n -w listAllAgents       d7285ce -- src/shared/ipc.ts
git grep -n -w AcDiscoveryAPI      d7285ce -- src/shared/ipc.ts

# Line counts
git show d7285ce:src/shared/ipc.ts   | wc -l    # expect 1235
git show d7285ce:src/shared/types.ts | wc -l    # expect 1445
```

**After batch 2 (Rust, phone chain):**

```bash
cd src-tauri
cargo check  --all-targets
cargo clippy --all-targets -- -D warnings       # zero warnings
cargo test   --lib --bins --tests
cd ..

# Completeness. Each must return exit 1:
git grep -n -E 'phone_send_message|phone_get_inbox|phone_list_agents|phone_ack_messages' d7285ce -- src-tauri/
git grep -n -E 'phone::manager|commands::phone|super::manager'                           d7285ce -- src-tauri/
git grep -n -w Conversation   d7285ce -- src-tauri/src/phone/ src/
git grep -n -w PhoneMessage   d7285ce -- src-tauri/ src/
git ls-tree -r --name-only d7285ce -- src-tauri/src/phone/manager.rs
git ls-tree -r --name-only d7285ce -- src-tauri/src/commands/phone.rs

# AgentInfo: exactly 4 hits, all in entity_creation.rs (:39, :2705, :2706, :2756):
git grep -n -w AgentInfo d7285ce -- src-tauri/ src/

# Live surface intact. Each must still return hits:
git grep -n -w OutboxMessage             d7285ce -- src-tauri/src/phone/types.rs
git grep -n -w MESSAGING_DIR_NAME        d7285ce -- src-tauri/src/phone/messaging.rs
git grep -n -w can_communicate           d7285ce -- src-tauri/src/config/teams.rs
git grep -n 'pub mod phone;'             d7285ce -- src-tauri/src/lib.rs
git ls-tree -r --name-only d7285ce -- src-tauri/src/phone/
    # expect exactly: consumption.rs, mailbox.rs, messaging.rs, mod.rs, types.rs

# Line counts
git show d7285ce:src-tauri/src/phone/types.rs   | wc -l   # expect 883
git show d7285ce:src-tauri/src/phone/mod.rs     | wc -l   # expect 4
git show d7285ce:src-tauri/src/commands/mod.rs  | wc -l   # expect 22
```

**After batch 3 (Rust, sync wrapper):**

```bash
cd src-tauri
cargo check  --all-targets
cargo clippy --all-targets -- -D warnings
cargo test   --lib --bins --tests
cd ..

# sync_workgroup_repos: exactly ONE surviving hit outside plans/, the log string at :3629.
git grep -n -w sync_workgroup_repos d7285ce -- src-tauri/ src/

# The inner helper and its live caller survive.
git grep -n -w sync_workgroup_repos_inner d7285ce -- src-tauri/
    # expect exactly 3 hits after the cut:
    #   :3399  the live call inside update_team
    #   :3449  a code comment referencing it ("...emitted straight to the UI by
    #          `sync_workgroup_repos_inner`..."), out of scope and untouched
    #   :3462  the definition
    # The 4th baseline hit, :3685 inside the deleted wrapper, is gone.
git grep -n -w SyncResult  d7285ce -- src-tauri/   # expect 3 hits: :91, :3470, :3471
git grep -n -w SyncError   d7285ce -- src-tauri/   # expect 3 hits, unchanged

# Line counts
git show d7285ce:src-tauri/src/lib.rs                        | wc -l   # expect 3589
git show d7285ce:src-tauri/src/commands/entity_creation.rs   | wc -l   # expect 7793
```

**After batch 4 (docs):**

```bash
# Completeness. Each must return exit 1:
git grep -n -i -E 'phone|conversations' d7285ce -- PRIVACY.md
git grep -n -w -E 'PhoneAPI|PhoneMessage|Conversation|AgentInfo|CONVDIR|C_PHONE|PH_MGR|A10|R9' d7285ce -- docs/reference/architecture.md
git grep -n 'conversations' d7285ce -- docs/agents/inter-agent-messaging.md docs/security.md

# Kept lines. Each must still return a hit:
git grep -n 'PH\["phone/'   d7285ce -- docs/reference/architecture.md      # :34
git grep -n 'CMD --> PH'    d7285ce -- docs/reference/architecture.md      # :53
git grep -n 'PH <-->'       d7285ce -- docs/reference/architecture.md      # :60
git grep -n 'entirely local' d7285ce -- PRIVACY.md
git grep -n 'messaging/'    d7285ce -- PRIVACY.md

# Line counts
git show d7285ce:docs/reference/architecture.md         | wc -l   # expect 725
git show d7285ce:PRIVACY.md                             | wc -l   # expect 54
git show d7285ce:docs/agents/inter-agent-messaging.md   | wc -l   # expect 214
git show d7285ce:docs/security.md                       | wc -l   # expect 121

# Mermaid renders. Open docs/reference/architecture.md in a Mermaid-capable
# preview and confirm all five diagrams render with no ghost or unstyled node.
```

**Final, across the whole change:**

```bash
git grep -n -i -E 'phone_send_message|phone_get_inbox|phone_list_agents|phone_ack_messages|PhoneAPI|syncWorkgroupRepos' d7285ce -- . ':!plans/'
    # -> exit 1, zero hits repo-wide outside plans/
npm run typecheck && npm test && npm run test:debt
cd src-tauri && cargo check --all-targets \
  && cargo clippy --all-targets -- -D warnings \
  && cargo test --lib --bins --tests
```

### 9.5 Objective acceptance criteria

Every criterion is decidable by running a listed command and comparing the result. No judgement is required.

| # | Criterion | Decided by |
| ---: | --- | --- |
| 1 | `cargo check --all-targets` exits 0 | batch 2 and 3 gates |
| 2 | `cargo clippy --all-targets -- -D warnings` exits 0 with zero warnings | batch 2 and 3 gates |
| 3 | `cargo test --lib --bins --tests` passes, including `phone/types.rs`'s test module | batch 2 and 3 gates |
| 4 | `npm run typecheck` exits 0 | batch 1 gate |
| 5 | `npm test` passes | batch 1 gate |
| 6 | `npm run test:debt` passes | final gate |
| 7 | The four `phone_*` command names return **zero** hits repo-wide outside `plans/` | final grep |
| 8 | `PhoneAPI` returns **zero** hits outside `plans/` | final grep |
| 9 | `PhoneMessage` returns **zero** hits under `src/` and `src-tauri/` | batch 1 and 2 greps |
| 10 | `Conversation` returns **zero** hits under `src-tauri/src/phone/` and `src/` | batch 2 grep |
| 11 | `AgentInfo` returns **exactly 4** hits under `src-tauri/` and `src/`, all in `commands/entity_creation.rs` (`:39`, `:2705`, `:2706`, `:2756`) | batch 2 grep |
| 12 | `sync_workgroup_repos` (word-regexp) returns **exactly 1** hit under `src-tauri/` and `src/`: the log string in the inner helper | batch 3 grep |
| 13 | `syncWorkgroupRepos` returns **zero** hits under `src/` | batch 1 grep |
| 14 | `sync_workgroup_repos_inner` returns **exactly 3** hits under `src-tauri/`: the live call at `:3399`, the code comment at `:3449`, and the definition at `:3462` | batch 3 grep |
| 15 | `SyncResult` returns 3 hits, `SyncError` returns 3 hits, both in `entity_creation.rs` | batch 3 grep |
| 16 | `src-tauri/src/phone/` contains exactly `consumption.rs`, `mailbox.rs`, `messaging.rs`, `mod.rs`, `types.rs` | batch 2 `git ls-tree` |
| 17 | `phone/types.rs` still exports `OutboxMessage` and the `PtyInput*` surface; `lib.rs:11 pub mod phone;` is present | batch 2 greps |
| 18 | `docs/reference/architecture.md:34` is present; `:53` and `:60` are present | batch 4 greps |
| 19 | No removed symbol is documented anywhere: the architecture.md symbol grep returns exit 1 | batch 4 grep |
| 20 | `PRIVACY.md` still asserts inter-agent messaging is entirely local with no external network calls, and contains no `conversations` reference | batch 4 greps |
| 21 | `docs/agents/inter-agent-messaging.md` and `docs/security.md` contain no `conversations` reference | batch 4 grep |
| 22 | Every one of the 13 line counts in the Section 1.3 table matches | per-batch line-count checks |
| 23 | `CHANGELOG.md` is unmodified | `git diff --stat` shows no `CHANGELOG.md` entry |
| 24 | The branch contains the 4 implementation commits of Section 4.1, in that order and with those messages. Any `docs(#1179)` commit carrying this plan file is separate and does not count against this criterion (Section 9.6). | `git log --oneline d7285ce..HEAD` |

### 9.6 Note on this plan file and `.gitignore`

`.gitignore:11` ignores `plans/`, so **this file is untracked by default**. Several plan files are nevertheless in the repository (`plans/1038-*`, `1057-*`, `1070-*`, `1072-*`, `1171-*`, `1177-*`), added with `git add -f`. Precedent from the immediately preceding change is two dedicated commits: `092d85c docs(#1177): add implementation plan for dead-code removal`, then `c93bff0 docs(#1177): certify plan READY_FOR_IMPLEMENTATION (Step 7 consensus)`.

Two consequences:

1. **Committing this plan is not the implementer's job** and is not one of the four batches. It belongs to the plan-authoring and certification workflow, and it requires `git add -f` because of the ignore rule.
2. **The ignore rule is a second layer of protection for the verification greps.** Even a grep run against the working tree instead of `d7285ce` would not see this file, because `git grep` skips ignored paths. That does not make Rule 1 optional: `plans/1177-remove-dead-code.md` **is** tracked and does contain `sync_workgroup_repos` and `PhoneMessage`, so the `':!plans/'` exclusion in the final grep remains mandatory.

---

## 10. Notes for the implementer

You are working from a purged context. Everything you need is in this file; you do not need the issue body, the revalidation reports, or any prior conversation.

1. **The issue body contains errors this plan has already corrected.** If you read it and it disagrees with Section 5, **Section 5 wins**. The disagreements are listed in Section 12 so you can tell a correction from a mistake.
2. **`phone/types.rs` is not a phone-types file.** It is 913 lines of live PTY-input protocol with 30 lines of phone types appended at the end. You are removing `:779-808` and nothing else. Do not open the file expecting to delete it.
3. **The live surface under `phone/` is everything except `manager.rs`.** `consumption.rs`, `mailbox.rs` (21,707 lines, the real CLI messaging system), `messaging.rs`, `mod.rs`, `types.rs` all stay.
4. **Batch 2 is atomic.** Apply all six edits, then gate. A `cargo check` between them will fail and the failure means nothing.
5. **After batch 2, `lib.rs:2667` has become `:2663`.** In batch 3, locate by anchor text.
6. **A green build does not prove the TypeScript deletion was complete.** This repo has no `noUnusedLocals` and no linter. The greps in Section 9.4 are the real gate for T2 and T3.
7. **Run verification greps against `d7285ce`, never the working tree.** Otherwise this plan file matches every symbol you are checking for.
8. **Three homonym traps** (`AgentInfo`, `send_message`, TS `AgentInfo`) plus one near-homonym (`sync_workgroup_repos` vs `..._inner`). Section 6.3. Read the definition site; never trust a match count.
9. **`plans/1177-remove-dead-code.md:625` records the sync wrapper at `:3678`. That is stale.** The real location is `:3666`. Acting on the stale number lands you 12 lines inside the function body.
10. **Do not reformat anything.** Every edit is a deletion or a named-token replacement.
11. **Commit at the end of every batch.** If your session dies, a commit survives and a working tree does not.
12. If a gate fails in a way Section 6.4 does not describe, **stop and report it.** That is new information, not something to work around.

---

## 11. Decisions (all closed)

| # | Decision | Resolution | Where |
| ---: | --- | --- | --- |
| 1 | Batching and commit order | 4 batches, 4 commits: TS -> Rust phone -> Rust sync -> docs. Batch 2 is atomic. | §4.1 |
| 2 | Exact `PRIVACY.md` replacement wording | Specified verbatim, 3 lines for 3 lines, with a per-clause evidence table | §4.2 |
| 3 | `docs/agents/inter-agent-messaging.md:129-132` | **Delete** the `## Conversation files` section (no guarantee to preserve, unlike `PRIVACY.md`) | §4.3 |
| 4 | `docs/security.md:14` | **Edit**: drop `conversations, ` from the disk enumeration | §5.4 D4 |
| 5 | `docs/agents/teams-and-workgroups.md:161` | **Out of scope.** The sentence is about the workgroup directory, not the config-dir store; this change does not make it false. | §3.2 |
| 6 | `lib.rs:1254` incidental comment | **Edit**: drop `, `sync_workgroup_repos``; `update_team` keeps the comment true | §4.4 |
| 7 | `architecture.md:623` `style CONVDIR` | **Delete.** Missing from the issue; leaving it dangles a Mermaid style directive | §5.4 |
| 8 | `CHANGELOG.md` entry | **None.** Not user-facing; #1177 set the precedent | §4.5 |
| 9 | `SyncResult` / `SyncError` `Serialize` derives | **Keep.** Unnecessary after they leave the IPC surface, but harmless, warning-free, and removing them is scope creep | §3.2 |
| 10 | Does `syncWorkgroupRepos` orphan a TS import? | **No.** Its return type is inline and anonymous. The issue's open question is closed. | §5.3 T4 |
| 11 | Is `sync_workgroup_repos` documented in `docs/`? | **No.** `git grep -i -E 'sync_workgroup_repos\|syncWorkgroupRepos' d7285ce -- docs/ CHANGELOG.md README.md` -> exit 1. The issue's open check is closed; no doc edit is needed for it. | §2.3 |
| 12 | Which gate commands are binding | The CI-exact ones: `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib --bins --tests`, `npm run typecheck`, `npm test`, `npm run test:debt`. The issue's `--workspace --all-features` variant is a harmless superset; the CI form is what gates the PR. | §2.4, §9.4 |
| 13 | `architecture.md:60` `PH <-->\|"JSON files"\| FS` | **Keep.** The live outbox still writes `<file>.json`, so the edge stays accurate. | §3.2 |
| 14 | `architecture.md:687` misattributing `can_communicate()` to `phone/manager.rs` | No separate fix. The row is deleted by D1, so the pre-existing error disappears with it. | §2.3d, §5.4 |
| 15 | Existing user `~/.agentscommander/conversations/` directories | **Left on disk untouched.** No migration, no cleanup, no data deletion. | §6.2 |
| 16 | Committing this plan file, given that `.gitignore:11` ignores `plans/` | **Not the implementer's job and not one of the four batches.** It needs `git add -f` and belongs to the authoring/certification workflow, matching the #1177 precedent. | §9.6 |

---

## 12. Scope corrections relative to issue #1179

### 12.1 Corrections the issue body already carries

The issue body was rewritten on 2026-08-01 and already corrects nine claims from its original version, including the `types.ts:1041-1046` range that would have gutted the live `AcWorkgroup` and `WorkgroupGroup` interfaces. Those corrections are reflected throughout this plan and are not re-litigated here.

### 12.2 Corrections this plan adds on top of the corrected issue body

| # | Issue body says | This plan | Impact |
| ---: | --- | --- | --- |
| 1 | Step 10 lists `architecture.md` lines `:92, :114, :115, :133, :144, :145, :162, :228, :230, :269, :282, :294, :611, :686, :687, :700`, for a total of **16 lines** | **17 lines.** `:623` (`style CONVDIR ...`) is missing from the list. `CONVDIR` is referenced twice; deleting only the node leaves a Mermaid style directive pointing at an undeclared id. | **Rendering defect if missed.** These diagrams render in-product via `mermaid ^11.15.0`. |
| 2 | Step 11 scopes the documentation fallout to `PRIVACY.md:29-31` | **Two more locations describe the deleted `conversations/` store.** `docs/agents/inter-agent-messaging.md:129-132` is an entire `## Conversation files` section asserting AC "**also persists** a per-peer conversation snapshot at `<config-dir>/conversations/...`". `docs/security.md:14` lists `conversations` among what lives on disk. Both go stale. | **Documentation left factually wrong if missed.** The `inter-agent-messaging.md` one is the more serious: it is a positive claim about a feature that will no longer exist, in the document agents are pointed at. |
| 3 | Step 9 says "plus any import entry that becomes orphaned by it" (`syncWorkgroupRepos`) | **There is none.** The return type is an inline anonymous object type at `ipc.ts:1073`. | Closes an open instruction that would otherwise send the implementer looking for something that does not exist. |
| 4 | Step 10 says "Also check whether `sync_workgroup_repos` is documented there and update it if so" | **It is not documented anywhere.** `git grep -i -E 'sync_workgroup_repos\|syncWorkgroupRepos' d7285ce -- docs/ CHANGELOG.md README.md` -> exit 1. | Closes an open check. |
| 5 | The `lib.rs:1254` comment is called an "incidental mention" with no instruction | **It must be edited.** After R7 it names a command that does not exist. | Closes an undecided item. |
| 6 | Acceptance criterion: "`git grep -w -E 'PhoneMessage\|Conversation'` returns no hits under `src/` or `src-tauri/src/phone/types.rs`" | **The `Conversation` half is under-scoped as an unscoped grep.** `Conversation` also appears as an English word in `pty/container_paths.rs:245`, `:592` and `docs/comparison.md:43`. This plan scopes the criterion to `src-tauri/src/phone/` and `src/`. | Prevents a false acceptance failure. |
| 7 | Acceptance criteria use `cargo check --workspace --all-targets --all-features` and `cargo clippy --workspace --all-targets --all-features` | **CI runs `cargo check --all-targets` and `cargo clippy --all-targets -- -D warnings` from `src-tauri/`.** The issue's form is a harmless superset; this plan specifies the CI form as binding, since it is what gates the PR. | Removes ambiguity about which command decides acceptance. |
| 8 | Not addressed | **`src-tauri/capabilities/default.json` and `tauri.conf.json` contain no reference to either surface** (verified). No manifest edit is required. | Closes a Tauri-specific failure mode neither revalidation covered. |
| 9 | Not addressed | **No integration test under `src-tauri/tests/` reaches the removed surface** (verified; the three `phone` hits there are all live `messaging`/`types`/`mailbox` references). | Closes an external-linkage failure mode. |
| 10 | Not addressed | **Neither deleted Rust file contains a `cfg` attribute**, so #1177's "no PR job compiles Rust for Linux or macOS" hazard does not apply to this change. | Removes a risk the implementer might otherwise assume they inherit. |

### 12.3 No reason was found to stop

The change should proceed as specified. Every "dead" claim was re-verified independently against `d7285ce`; every one holds. The three additions in 12.2 items 1 and 2 widen the documentation scope by 6 lines across 2 files; they do not change the shape of the work or its risk profile.

---

## 13. Enrichment record

*Empty at Step 4. To be filled by `dev-rust` (Step 5) and `dev-rust-grinch` (Step 6), then resolved by the architect at Step 7 before certification.*

### dev-rust (Step 5)

Enrichment by `dev-rust`, from a purged context, re-deriving every coordinate from `d7285ce` rather than from the Step 4 text. Seven items follow. **E1 is blocking and plan-wide; E2 through E4 are confirmations with new evidence; E5 through E7 are additions.**

---

#### E1. BLOCKING: every verification command in §9.4 is written against the baseline commit and can never pass

**§9.3 rule 1, and therefore all of §9.4 and criteria 7 through 22 of §9.5, verify the state the change is meant to remove.** `git grep <sym> d7285ce` reads the commit object at `d7285ce`, which is the tree *before* any edit. It returns the same result whether the implementation is complete, half-applied, or never started.

Demonstrated, not inferred:

```
$ git grep -n -w PhoneAPI d7285ce -- src/
d7285ce:src/shared/ipc.ts:807:export const PhoneAPI = {
exit=0
```

§9.4 requires that command to return exit 1. It cannot, at any point in the implementation. The same holds for the line counts:

```
$ git show d7285ce:src/shared/ipc.ts   | wc -l   ->  1252   (§9.4 expects 1235)
$ git show d7285ce:src/shared/types.ts | wc -l   ->  1463   (§9.4 expects 1445)
```

**Root cause.** The `d7285ce` discipline is correct for *revalidation*, which is what the two Step 3 reports were doing: proving the baseline state before anything is touched. It was carried into *acceptance*, which proves the post-change state. The revision is inverted for that purpose.

**The stated justification does not hold for the per-batch greps.** §9.3 rule 1 defends pinning to `d7285ce` on the grounds that a working-tree grep would match this plan file. Every per-batch grep in §9.4 is pathspec-scoped to `src/`, `src-tauri/`, `docs/`, `PRIVACY.md` or `docs/security.md`. None of those pathspecs can reach `plans/`. The pollution risk exists only for the two repo-wide greps in §9.4 "Final", and both already carry `':!plans/'`, which is the correct and sufficient guard.

**Correction to apply.**

1. §9.3 rule 1 becomes: *run every completeness grep against the working tree (or `HEAD` once the batch is committed). Keep the `':!plans/'` exclusion on the two repo-wide greps in "Final"; the per-batch greps are pathspec-scoped and need no exclusion. Use `d7285ce` only to re-read the baseline for orientation, never as an acceptance check.*
2. In §9.4, delete the `d7285ce` argument from every `git grep` and every `git ls-tree`.
3. In §9.4, the line-count checks become `wc -l < <path>` against the working tree, or `git show HEAD:<path> | wc -l` after the batch commit.
4. §6.2's "stale plan text" row and §9.6's second consequence should be reworded to match: the protection is the pathspec, not the revision.

**Cost if missed.** The implementer applies six correct edits, runs the batch-2 completeness greps, sees every one of them report hits it was told must be absent, and concludes the deletion failed. The most likely reactions are reverting correct work or hunting a defect that does not exist. This is the single largest session-burn risk in the plan, it affects 16 of the 24 acceptance criteria, and the fix is one line in §9.3 plus deleting one token per command in §9.4.

---

#### E2. Batch 2 atomicity: confirmed. Six edits, genuinely complete, no seventh edit

The claim in §4.1 holds. Each of the four cited intermediate failures is real, and I found no additional edit needed to reach a compiling state. Evidence for every way a seventh edit could have been required:

| Closure | Evidence |
| --- | --- |
| Nothing outside the two deleted files reaches the deleted symbols | `git grep -E 'phone::manager\|commands::phone\|super::manager' -- src-tauri/` -> `commands/phone.rs:2` plus `lib.rs:2612-2615`, nothing else |
| `super::manager` cannot false-fail the batch-2 grep | **zero hits repo-wide under `src-tauri/`, even at baseline.** The Step 3 report only checked `src-tauri/src/phone/`; this plan widened the pathspec, and the wider form is still clean |
| `types.rs:4` `use serde::{Deserialize, Serialize};` must stay | `Deserialize` appears 12 times and `Serialize` 12 times in the surviving `:1-777`. Neither becomes an unused import, so clippy `-D warnings` is not tripped. §3.2 is right |
| No surviving item is stranded by R1/R2 | The two deleted files reach out to exactly four things: `config::config_dir` (~50 files use it), `config::teams::discover_teams`, `config::teams::can_communicate` (9 callers), `config::teams::DiscoveredTeam`. All survive with many other users |
| `--all-targets` holds no hidden target | `src-tauri/Cargo.toml` has no `[[bin]]` section; there is no `src-tauri/src/bin/`, no `benches/`, no `examples/`. `--all-targets` resolves to the lib, one default bin, and 19 integration tests |

**New check the plan does not run: source-introspection tests.** This crate contains several tests that read production `.rs` files off disk and assert on their content. They are invisible to symbol greps and are the most likely source of a batch-2 or batch-3 gate failure that §6.4 would not explain. All are clean, and this is worth recording so the next reader does not have to re-derive it:

| Test | What it reads | Why this change cannot break it |
| --- | --- | --- |
| `lib.rs:3241` `restore_loop_normalizes_archived_roots_before_persisted_session_loop` | `src/lib.rs`, split at `#[cfg(test)]\nmod tests` | Compares the byte offsets of `let archived_roots = sessions_persistence::normalize_project_roots` and `for ps in &persisted`. Both sit around `:1200`, far above the `invoke_handler` at `:2532`. Deleting 5 registrations does not reorder them |
| `config/local_config_io.rs:429` `agent_replica_root_config_writes_go_through_shared_helper` | walks `src/config` **and `src/commands`** on the live filesystem | Offender-detection (`assert!(offenders.is_empty())`), not exhaustiveness. Deleting a file cannot add an offender. Its allowlist at `:613` names `src/commands/entity_creation.rs` only for lines containing `write_team_config`, `create_new_team_config_on_disk` or `team_dir.join("config.json")`; the wrapper R7 deletes contains none of them, so R7 does not disturb it either |
| `tests/pty_writer_inventory.rs:67` `every_production_pty_writer_is_in_the_explicit_permit_inventory` | walks every `.rs` under `src/`; permits `src/phone/mailbox.rs` | Neither deleted file contains a PTY write, so the found set shrinks by nothing. `mailbox.rs`, the only phone entry in the permit list, is untouched |
| `pty/watchers/mod.rs:2470`, `commands/session.rs:7432/:7492/:7790/:8012`, `loops/scheduler.rs:644`, `session/selection.rs:3980`, `phone/mailbox.rs:11320` | each `include_str!`s or reads one specific unrelated file | None reads `lib.rs`, `commands/phone.rs`, `phone/manager.rs`, `phone/types.rs`, `commands/mod.rs` or `phone/mod.rs` |

There is no test asserting a registered-command count and no desktop/web command-parity test. `git grep -i -E 'generate_handler|command_count|registered_commands|invoke_handler' -- src-tauri/src/ src-tauri/tests/` returns three hits: the real handler at `lib.rs:2532`, an unrelated single-command mock at `commands/resource_monitor.rs:466`, and a prose comment at `web/commands.rs:442`.

**Verdict: R1 through R6 are complete and atomic as written. §10.4's "apply all six, then gate" is correct and should be followed literally.**

---

#### E3. Cut boundaries: all confirmed byte-exact, and all 13 line counts confirmed

Every boundary in §5.1, §5.2 and §5.3 was re-read out of `d7285ce`. No correction.

| Cut | Boundary confirmed | Arithmetic |
| --- | --- | --- |
| `phone/types.rs:779-808` | `:777` `}`, `:778` blank, `:779` `#[derive(Debug, Clone, Serialize, Deserialize)]`; `:807` `}`, `:808` blank, `:809` `#[cfg(test)]` | 913 - 30 = **883** |
| `entity_creation.rs:3664-3696` | `:3662` `}`, `:3663` blank, `:3664` `/// Sync repo assignments...`; `:3695` `}`, `:3696` blank, `:3697` `/// Refresh \`is_coordinator\`...` | 7826 - 33 = **7793** |
| `lib.rs:2612-2615` | `:2611` `spec_board_close,`, `:2616` `voice_transcribe,` | |
| `lib.rs:2667` | `:2666` `delete_workgroup,`, `:2668` `list_role_templates,` | 3594 - 4 - 1 = **3589** |
| `lib.rs:1254` | exactly ``// (e.g. `update_team`, `sync_workgroup_repos`). Must happen BEFORE the`` | unchanged |
| `commands/mod.rs:9` | `:8` `pub mod non_stop;`, `:10` `pub mod project_settings;` | 23 - 1 = **22** |
| `phone/mod.rs:3` | 5-line file, exactly as §5.1 R4 prints it | 5 - 1 = **4** |
| `ipc.ts:806-815`, `:1071-1075`, `:28-29` | `:805` `}`, `:816` blank, `:817` `AcDiscoveryAPI`; `:1070` `delete_workgroup` call, `:1076` `};` | 1252 - 17 = **1235** |
| `types.ts:981-998` | `:978` `}`, `:979`/`:980` the two-blank separator; `:999` `export interface AcAgentMatrix {` | 1463 - 18 = **1445** |
| `architecture.md` | 738 at baseline; 13 deletions + 4 label edits | 738 - 13 = **725** |
| `PRIVACY.md` / `inter-agent-messaging.md` / `security.md` | 54 / 218 / 121 at baseline | 54 / 214 / 121 |

The §1.3 table is correct in all 13 rows and is safe to use as an acceptance check, **once E1 is applied** so the counts are taken from the working tree instead of `d7285ce`.

One note on §5.1 R6: the plan is right that `use serde::{Deserialize, Serialize};` at `:4` must stay, and right about the reason. Also confirmed that `lib.rs:3 pub mod commands;`, `:4 pub mod config;` and `:11 pub mod phone;` are all plain `pub mod`, which is what makes §2.4's "no `dead_code` warning for `pub` items" true rather than merely likely.

---

#### E4. Grep audit: no criterion fails on correct work, and one grep must not be "improved"

Each completeness grep in §9.4 was run in its corrected (working-tree) form against the known post-change state. Beyond E1's revision-level defect, **I found no criterion that fails on a correct implementation.** The specific traps checked, with the ones worth knowing about called out:

| Grep | Result |
| --- | --- |
| `-w Conversation -- src-tauri/src/phone/ src/` | **Safe, but fragile. Do not add `-i`.** The surviving `phone/mailbox.rs` contains "conversation" on 3 lines, always lowercase, so the case-sensitive `-w Conversation` does not match them. Adding `-i` to "make it stricter" converts criterion 10 into a guaranteed false failure. §6.2 correctly identified the `pty/container_paths.rs` and `docs/comparison.md` prose; this is the same hazard one directory deeper, inside the pathspec the plan chose |
| `-w -E '...\|A10\|R9' -- docs/reference/architecture.md` | **Safe. This was the highest-risk item in batch 4.** `A10` and `R9` are short generic Mermaid ids that could easily have been reused by another diagram in a 738-line file. They are not: `A10` appears only at `:269` and `:294`, `R9` only at `:282` and `:294`, and all three lines are deleted by D1. The remaining terms resolve to `:115`, `:228`, `:230`, `:686`, all of which D1 edits |
| `-i -E 'phone\|conversations' -- PRIVACY.md` | Safe. Only `:29` and `:31`, both inside the block D2 replaces. No `telephone`/`headphone`/`microphone` substring trap in the file |
| `'conversations' -- docs/agents/inter-agent-messaging.md docs/security.md` | Safe. Exactly one hit each, `:131` and `:14`, both targeted by D3 and D4 |
| `-w PhoneMessage`, `-w AgentInfo`, `phone_`, `-w syncWorkgroupRepos` over `src/` | Safe. Baseline hit sets are exactly the lines T1 through T4 delete: `PhoneMessage` at `ipc.ts:28,:811` and `types.ts:981`; `AgentInfo` at `ipc.ts:29,:812` and `types.ts:991`; `phone_` only the four `invoke` string literals; `syncWorkgroupRepos` only `ipc.ts:1072`. No component, test or story mentions any of them |
| criterion 12, `-w sync_workgroup_repos -- src-tauri/ src/` | Safe. Baseline is 5 hits (`entity_creation.rs:3629,:3666`, `lib.rs:1254,:2667`, `ipc.ts:1074`); R7, R8, R9 and T4 remove four, leaving exactly `:3629`. Confirmed the `:3629` log string matches `-w` |
| criteria 11, 14, 15 | Safe, and none of the coordinates shift. `AgentInfo` -> 4 hits at `entity_creation.rs:39,:2705,:2706,:2756`; `sync_workgroup_repos_inner` 4 -> 3 at `:3399,:3449,:3462`; `SyncResult` 4 -> 3 at `:91,:3470,:3471`; `SyncError` 3 unchanged at `:84,:94,:3598`. Every one of these lines is above the `:3664` cut, so R7 moves none of them |
| final repo-wide sweep | Safe. Run at baseline it returns exactly 16 hits, and every one is a line this plan deletes or edits. Nothing outside `commands/phone.rs`, `lib.rs`, `ipc.ts` and `architecture.md` touches the surface. `src-tauri/src/web/`, `src-tauri/src/api/` and `src-tauri/src/cli/` return zero non-`messaging`/`mailbox`/`types` phone hits |

Two smaller items in §9.4 batch 2: the two `git ls-tree ... phone/manager.rs` and `... commands/phone.rs` lines carry no stated expectation. Under E1's fix they should read "must return exit 1, no output". As written against `d7285ce` they always print the path, which reads as a pass to a hurried eye.

---

#### E5. Gate commands are exact. Gate cost, and the one thing that would make the four-batch split expensive

**The commands are right.** Verified against `.github/workflows/pr-regression-gates.yml`: `:77 cargo check --all-targets`, `:81 cargo clippy --all-targets -- -D warnings`, `:85 cargo test --lib --bins --tests`. §11 decision 12 is correct and the issue's `--workspace --all-features` form is indeed a harmless superset.

**The batch-2 gate is a cold build in this replica, and it will be long.** `src-tauri/target/` currently contains only `fw/` (2.1 GB, written by something that set a non-default target directory). There is no `src-tauri/target/debug/`, and neither `src-tauri/.cargo/config.toml` nor `CARGO_TARGET_DIR` redirects the default. So `cd src-tauri && cargo check --all-targets` compiles the whole dependency tree from scratch, including `rusqlite` with `features = ["bundled"]`, which is a full C compile of SQLite, plus the tauri, axum and reqwest trees. `cargo test --lib --bins --tests` then pays full codegen and links 21 binaries on Windows. **Budget for it and do not read a long first gate as a hang.**

**Batch 3's gate is incremental and therefore cheap**, because only `entity_creation.rs` and `lib.rs` change and only `agentscommander-new` recompiles. §4.1's "separating them costs one extra Rust gate run" is accurate, **conditional on the target directory surviving between the two batches.** So, between batch 2 and batch 3: no `cargo clean`, no toolchain switch, and do not run the gates from a different directory or with a different `CARGO_TARGET_DIR`. If that condition breaks, the split stops costing one incremental run and starts costing a second cold build.

**§4.1's rationale and §8/§10.11's rule disagree, and the rule loses.** §4.1 justifies committing per batch as follows: "a session can be lost during a long `cargo` gate, and a commit survives that where a working tree does not." But §8 and §10.11 both instruct committing at the *end* of the batch, which is after the gate. That leaves the working tree unprotected for exactly the interval the rationale is about.

**Concrete correction: apply the batch's edits, commit immediately, then run the gate.** If the gate fails, fix and `git commit --amend`, or add a fixup and squash before the next batch. Same four commits, same messages, same revert granularity, and the risk window closes. This matters most for batch 2: it is the only batch whose intermediate state cannot be reconstructed by inspecting a partially-edited tree, it is six edits deep, and it is the one immediately followed by the cold build.

---

#### E6. Batch 1 has no owner, and it is not the Rust implementer's

§8 hands all four batches to a single "implementer" and §10 addresses one reader. Batch 1 is TypeScript (`src/shared/ipc.ts`, `src/shared/types.ts`) and batch 4 is documentation. The Rust owner's role boundary excludes `src/` TypeScript. Read cold, §8 tells whoever picks this up to start with edits they may not be permitted to make.

This is load-bearing rather than cosmetic, because §4.1 uses the ordering as an argument: "TypeScript first is strictly better" is a coherent sequencing choice for one person and a **blocking cross-agent dependency** for two. Someone has to say whether batch 2 may start before batch 1 has landed.

**It may, and the plan already contains the proof.** §4.1 establishes that the two halves have no compile-time dependency and describes the "Rust landed, TS not landed" state explicitly: `tsc` still passes because the command names are string literals, and nothing calls `PhoneAPI`, so the state is inert. The TypeScript-first ordering is therefore a preference about which interrupted state is tidier, not a constraint. Saying so out loud prevents a Rust implementer from idling on a batch that is not theirs.

**Requested at Step 7:** name an owner per batch in §8, and state explicitly that batches 2 and 3 may proceed independently of batch 1.

---

#### E7. Smaller items

1. **§3.1 R7 and §5.2 both say `:3664-3696`, 33 lines. That is correct.** `:3695` is the closing brace, `:3696` the trailing blank, `:3697` the next doc comment. Recorded because the Step 5 dispatch brief paraphrased the range as `:3664-3695`; the plan is right and the paraphrase is one line short. An implementer working from a 32-line cut leaves a double blank line.
2. **§9.5 criterion 22 is entirely downstream of E1.** As written, all 13 line counts are read out of `d7285ce` and all 13 will report the pre-change number. The counts themselves are correct (E3); only the revision they are read from is wrong.
3. **`architecture.md:230`'s edit resolves cleanly.** Dropping ` PhoneAPI,` turns `DarkFactoryAPI, PhoneAPI,<br/>DebugAPI` into `DarkFactoryAPI,<br/>DebugAPI`, which is what §5.4 specifies.
4. **§5.3 T4's resulting shape is fine.** After the cut, `:1070` ends `),` and is followed directly by `};` with no blank line. That matches `PhoneAPI`'s own shape at `:814`/`:815`, exactly as §5.3 claims.

---

#### Verdict: is this plan implementable cold-start as it stands?

**No, because of E1 alone.** The acceptance suite in §9.4 is unrunnable, and its failure mode is indistinguishable from broken work: an implementer who does everything right sees 16 criteria report the opposite of what they were told to expect. Nothing else in the plan comes close to that cost.

**Everything else holds.** Scope, cut boundaries, line counts, batch-2 atomicity, the homonym analysis, the gate commands, the Mermaid node-integrity table and the four checks in §2.3 all survive independent re-derivation from `d7285ce`. The six batch-2 edits are complete; no seventh edit exists. I found no step whose ordering breaks in practice, and no assumption in the plan that I know to be false other than E1's.

**With E1's correction applied to §9.3, §9.4 and §9.5, plus the per-batch ownership statement in E6, this plan is implementable cold-start.** E5's commit-before-gate change and the "do not disturb the target directory" note are improvements to resilience and cost, not preconditions.

### dev-rust-grinch (Step 6)

Adversarial review by `dev-rust-grinch`, reading this plan end to end and checking the production tree at `d7285ce`. The review deliberately concentrated on acceptance semantics, text/reflection-like test coupling, multi-owner recovery, documentation truthfulness, and claims that a normal compiler gate cannot prove. Findings G1 through G6 require Step 7 resolution; G7 closes the hidden-coupling audit with additional evidence. This is an enrichment, not a certification verdict.

---

#### G1. BLOCKING correction to E1: `git ls-tree` cannot be converted to a working-tree check by deleting `d7285ce`

- **What:** E1 correctly requires acceptance greps and line counts to inspect the post-change tree, but its instruction to delete `d7285ce` from **every `git ls-tree`** is not executable. `git ls-tree` always requires a tree-ish. Also, a valid `git ls-tree HEAD -- <missing-path>` exits **0**, not 1, so E4's proposed "must return exit 1" expectation for the two deleted files is wrong.
- **Why:** Demonstrated on this branch:

  ```text
  git ls-tree -r --name-only -- src-tauri/src/phone/
      -> exit 128: fatal: Not a valid object name src-tauri/src/phone/

  git ls-tree -r --name-only HEAD -- does/not/exist
      -> exit 0, zero output
  ```

  If Step 7 applies E1 mechanically, both deleted-file checks become syntax failures. A hurried implementer can mistake that non-zero syntax failure for the requested proof of absence, while the surviving-directory inventory cannot run at all.
- **Fix:** Keep E1's working-tree/`HEAD` correction for `git grep` and line counts, but replace the three `ls-tree` checks explicitly. Under E5's commit-before-gate order, use `HEAD` and assert output, not exit status:

  ```bash
  test ! -e src-tauri/src/phone/manager.rs
  test ! -e src-tauri/src/commands/phone.rs
  test -z "$(git ls-tree -r --name-only HEAD -- src-tauri/src/phone/manager.rs)"
  test -z "$(git ls-tree -r --name-only HEAD -- src-tauri/src/commands/phone.rs)"

  expected_phone_files="$(printf '%s\n' \
    src-tauri/src/phone/consumption.rs \
    src-tauri/src/phone/mailbox.rs \
    src-tauri/src/phone/messaging.rs \
    src-tauri/src/phone/mod.rs \
    src-tauri/src/phone/types.rs)"
  actual_phone_files="$(git ls-tree -r --name-only HEAD -- src-tauri/src/phone/)"
  test "$actual_phone_files" = "$expected_phone_files"
  ```

  If Step 7 chooses gate-before-commit instead, use filesystem checks for the working tree and do not use `ls-tree` until the commit exists. In either ordering, no acceptance rule may treat a Git usage error as proof of deletion.

---

#### G2. The coordinator's strict order is load-bearing for the current per-batch gates; E6's proposed independence is operationally false

- **What:** The Rust source edits are compile-independent of batch 1, but the **batch-2 and batch-3 acceptance commands are not**. They intentionally search both `src-tauri/` and `src/`. Therefore §4.1's statement that TypeScript-first is only a coherence preference, and E6's request to say Rust may proceed independently, conflict with §9.4.
- **Why:** If batch 2 lands before batch 1, a correct Rust deletion still fails its gate:
  - `PhoneMessage` has three surviving TypeScript hits (`ipc.ts:28,:811`, `types.ts:981`).
  - `AgentInfo` has three TypeScript hits, so the promised total is 7 rather than the required four live Rust homonym hits.

  If batch 3 also runs before batch 1, `git grep -w sync_workgroup_repos -- src-tauri/ src/` sees the live inner-helper log **and** `ipc.ts`'s invoke string, two hits rather than one. The implementer is told to stop on exactly those results.
- **Fix:** Apply the coordinator decision verbatim in §8: batch 1 `dev-webpage-ui` -> batches 2 and 3 `dev-rust` -> batch 4 `technical-writer`, strictly sequential, one owner in the shared clone at a time. Resolve E6 by saying only that the **source changes compile independently**; with the chosen cross-tree gates, their execution order is mandatory. Alternatively the per-batch greps could be narrowed by language and only the final gate made cross-tree, but that is not the chosen coordination model.

  The ownership split itself is sound. Keeping batches 2 and 3 with the same Rust owner also preserves the incremental Cargo target cache assumed by E5.

---

#### G3. D4's proposed `docs/security.md` sentence remains factually false after removing `conversations`

- **What:** D4 changes the line to say that configuration, sessions, teams **and messages all live under** `~/.agentscommander/` (or the portable config directory). The deleted word is stale, but the surviving location claim for messages is also false and directly contradicts §4.2's evidence.
- **Why:** Canonical inter-agent message bodies live under project-workspace directories:
  - `phone/messaging.rs:160-164` creates `<workgroup-root>/messaging/`.
  - `phone/messaging.rs:166-170` creates the Root Agent's `messaging/` directory.

  Delivery also uses JSON outbox/artifact directories under an agent root or the app-selected outbox (`cli/send.rs:1078-1091`), not universally the user's home config directory. Shipping D4 as written tells a security-conscious user to secure, back up, or erase the wrong location. The same plan correctly refuses to repeat this error in `PRIVACY.md`, so the two documents would disagree after batch 4.
- **Fix:** Step 7 must give D4 exact replacement wording that distinguishes config/session/team files in the instance config directory from Markdown messages in workspace `messaging/` directories (and, if the threat-model enumeration intends to be exhaustive, the local outbox artifacts). Do not merely delete `conversations, `. Add a positive batch-4 check for the corrected `messaging/` location in `docs/security.md`, as well as the existing negative `conversations` check. D2's retargeted privacy guarantee is otherwise accurate; its acceptance checks should additionally assert the literal `No external network calls are made.` because criterion 20 currently claims that clause without testing it.

---

#### G4. Scope has no binding final gate; an unrelated tracked edit can satisfy all 24 criteria

- **What:** §8 prohibits scope expansion and formatting churn, but §9.5 only checks that `CHANGELOG.md` is absent. It never asserts the complete changed-path set or a clean handoff tree.
- **Why:** For example, an editor can rewrite `docs/style-guide.md` during batch 4. Every symbol grep, line count, build, test, commit-count criterion, and the narrow `CHANGELOG.md` criterion still passes, so an unauthorized change ships despite §3's closed scope. The same problem applies to an unrelated source edit that happens to compile.
- **Fix:** Add a final scope gate against `d7285ce`, excluding the plan workflow itself:

  ```bash
  git diff --check d7285ce..HEAD -- . ':!plans/1179-remove-dead-phone-and-sync-repos.md'
  git diff --name-status d7285ce..HEAD -- . ':!plans/1179-remove-dead-phone-and-sync-repos.md'
  ```

  The second command must contain exactly these 13 paths and statuses, and nothing else:

  ```text
  M  PRIVACY.md
  M  docs/agents/inter-agent-messaging.md
  M  docs/reference/architecture.md
  M  docs/security.md
  M  src-tauri/src/commands/entity_creation.rs
  M  src-tauri/src/commands/mod.rs
  D  src-tauri/src/commands/phone.rs
  M  src-tauri/src/lib.rs
  D  src-tauri/src/phone/manager.rs
  M  src-tauri/src/phone/mod.rs
  M  src-tauri/src/phone/types.rs
  M  src/shared/ipc.ts
  M  src/shared/types.ts
  ```

  Require `git status --porcelain` to be empty before every owner handoff, and require the final production diff to be read against §5's exact cuts. The path whitelist catches unrelated files; the diff read catches same-file reformatting or an extra edit hidden behind a correct line count.

---

#### G5. Commit-before-gate needs an explicit durable handoff protocol

- **What:** E5's commit-before-gate correction protects batch bytes during a long build, but in a multi-owner plan the presence of the expected commit does not prove its gate completed. No durable rule currently distinguishes "committed and accepted" from "committed, then the session died during the gate."
- **Why:** A batch-1 owner can commit, start `npm test`, and lose the session. The next owner sees the exact expected commit and a clean tree and can begin batch 2 on code that was never accepted. The same risk is greatest after batch 2's pre-gate commit and cold Rust build. This defeats the reason for strict handoff and makes recovery state ambiguous from repository inspection alone.
- **Fix:** State in §8 that no next owner is dispatched until the prior owner reports the complete gate as passed. On a session loss after commit, the recovering owner reruns the entire batch gate on that `HEAD`; commit existence is never treated as gate evidence. On a gate failure, the same owner fixes and amends the batch commit (or squashes any temporary fixup **before handoff**) and reruns the full gate. Do not proceed or create a fifth implementation commit. A failure outside §6.4 remains stop-and-report, as the plan already requires.

---

#### G6. The Mermaid gate has the wrong count and no cold-start executable procedure

- **What:** §9.4 says to confirm "all five diagrams" in `architecture.md`. The file has 15 Mermaid fences, and D1 changes exactly four diagrams: §2 Rust Backend Modules, §3.3 Shared Layer, §4 IPC Contract, and §8 Persistence. The plan supplies neither a command nor a named preview environment; `package.json` has the Mermaid runtime library but no `mmdc`/documentation-render script.
- **Why:** A purged-context terminal agent cannot objectively decide which fifth diagram was intended or execute "open ... in a Mermaid-capable preview." This contradicts §9.5's claim that every acceptance item is command-decidable. A manual glance can also silently use a different Mermaid version from the in-product runtime.
- **Fix:** Correct the count to the four affected diagrams and either provide a reproducible parser/render command using an already available project harness, or label this explicitly as a non-binding manual QA item and retain the objective node-reference checks as the gate. Do not add a new dependency solely for this deletion without a separate scope decision.

---

#### G7. Additional reflection/generated/fixture coupling audit: three omitted readers are clean

- **What:** E2 correctly opened the source-introspection test class, but its table was not exhaustive. Three more tests read affected production files without naming the removed symbols in an ordinary grep:
  1. `session/selection.rs:3838-3899` recursively reads **every** Rust source file for lifecycle-ownership violations.
  2. `src-tauri/tests/cli_workgroup_team.rs:1781-1869` reads `commands/entity_creation.rs` and asserts the production activation-token count and call shapes.
  3. `testability/ui_automation.rs:2592-2638` reads `../src/shared/types.ts` and parses the `UiAutomationAction` union.
- **Why:** These are exactly the hidden-coupling class named in the Step 6 dispatch. They do not fail this change, but omitting them leaves the claim "all are clean" under-evidenced.
- **Fix/evidence:** No implementation edit is required:
  - Deleting Rust files cannot add an ownership violation, and R7 contains none of the ownership sentinel's event/mutator patterns.
  - R7 contains no `ManifestActivationToken::production()` construction, so the entity-creation source count remains exactly three.
  - `UiAutomationAction` starts at `types.ts:847` and its terminating semicolon is before the deleted interfaces at `:981-998`, so that parser reads byte-identical text.

  I also checked the adjacent variants: `ipc.transport.test.ts` dynamically imports the whole IPC module but never enumerates exports; the capability-reading integration test only asserts the `resource-monitor` window; no serialized JSON fixture/snapshot contains the deleted conversation shape; no Rust doctest names a removed item; and the companion `repo-agentscommander_webpage` contains no removed symbol or `conversations/` reference. This closes the hidden-coupling line of attack with no new batch edit.

---

#### Risk claims and mandatory disciplines that yielded no further defect

- **Capabilities/config:** The default capability, `tauri.conf.json`, `tauri.prod.conf.json`, and `tauri.stage.conf.json` contain no removed command name. No capability edit is needed.
- **Platform/Windows:** Neither deleted file nor its module declarations/registrations are cfg-gated. All inbound references are unconditional and deleted in the same batches. #1177's platform-only residue scenario does not apply.
- **Concurrency/resources:** `phone/manager.rs` is synchronous filesystem code with no spawned task, channel, process, lock, or handle lifetime; the sync wrapper only delegates to the retained inner helper. The deletion introduces no cancellation, PTY, lock-order, or cleanup obligation.
- **CHANGELOG/user surface:** I found no supported product caller and no separately documented public client contract. The Rust items are technically public, as §7 says; the strongest supportable wording is "no supported or known external consumer," not an absolute proof that no private Git dependency exists. That caveat does not overturn the decided no-CHANGELOG result under this repository's user-facing release-note policy.

---

#### Step 6 verdict

**As written, the plan is not cold-start implementable**, already because of E1. Applying E1 mechanically would still leave G1's broken `ls-tree` commands, and D4 would ship a false security location.

**Once the architect applies E1 with G1's command semantics; formalizes the strict owner/order and gate handoff in G2/G5; corrects D4; and closes the scope and Mermaid acceptance gaps in G4/G6, the plan is implementable from a purged context.** I found no missing production deletion, no seventh batch-2 edit, no hidden caller, and no platform/resource blocker.

**Ownership verdict:** approve `dev-webpage-ui` for batch 1, `dev-rust` for batches 2 and 3, and `technical-writer` for batch 4. Approve only the coordinator's **strict sequential** order. With the current cross-tree per-batch greps, allowing Rust to run independently of batch 1 is not valid.

### Architect resolution and certification (Step 7)

*pending: this plan is NOT certified `READY_FOR_IMPLEMENTATION`*
