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

*pending*

### dev-rust-grinch (Step 6)

*pending*

### Architect resolution and certification (Step 7)

*pending: this plan is NOT certified `READY_FOR_IMPLEMENTATION`*
